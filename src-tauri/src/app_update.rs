use crate::error::{AppError, AppErrorDto, AppResult};
use crate::models::RuntimePhase;
use crate::runtime::MihomoRuntime;
use crate::storage::AppStorage;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use tauri::{AppHandle, Manager, State};
use tauri_plugin_opener::OpenerExt;
use tauri_plugin_updater::{Update, UpdaterExt};
use url::Url;

const HK_MANIFEST: &str = "https://downloads.cmmuu.com/api/releases/serylane/latest";
const HK_RELEASES: &str = "https://files.cmmuu.com/releases/serylane";
const GITHUB_MANIFEST: &str =
    "https://github.com/CMMUU/serylane/releases/latest/download/latest-serylane.json";
const LEGACY_GITHUB_MANIFEST: &str =
    "https://github.com/CMMUU/serylane/releases/latest/download/latest.json";
// Published <= 0.7.6 clients require this exact URL in latest.json. Keep the
// signed compatibility alias while all visible release links use Serylane.
const LEGACY_GITHUB_RELEASES: &str = "https://github.com/CMMUU/routedeck/releases";
const LEGACY_GITEE_RELEASES: &str = "https://gitee.com/cmmuu/routedeck/releases";
const GITEE_RELEASE: &str = "https://gitee.com/api/v5/repos/cmmuu/serylane/releases/latest";
const MAX_METADATA_BYTES: usize = 512 * 1024;
const MAX_DOWNLOAD_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UpdateSource {
    #[default]
    Auto,
    Github,
    Gitee,
    Hk,
}

impl UpdateSource {
    fn label(self) -> &'static str {
        match self {
            Self::Auto => "自动",
            Self::Github => "GitHub",
            Self::Gitee => "Gitee",
            Self::Hk => "香港下载中心",
        }
    }
    fn release_base(self) -> AppResult<&'static str> {
        match self {
            Self::Github => Ok("https://github.com/CMMUU/serylane/releases"),
            Self::Gitee => Ok("https://gitee.com/cmmuu/serylane/releases"),
            Self::Hk => Ok(HK_RELEASES),
            Self::Auto => Err(failure("请指定实际发布渠道")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct StableVersion(u64, u64, u64);

fn parse_version(value: &str) -> AppResult<StableVersion> {
    let parts: Vec<_> = value.split('.').collect();
    if parts.len() != 3
        || parts.iter().any(|p| {
            p.is_empty()
                || (p.len() > 1 && p.starts_with('0'))
                || !p.bytes().all(|b| b.is_ascii_digit())
        })
    {
        return Err(failure("版本号必须是严格的 X.Y.Z 稳定版本"));
    }
    let number = |p: &str| p.parse::<u64>().map_err(|_| failure("版本号超出范围"));
    Ok(StableVersion(
        number(parts[0])?,
        number(parts[1])?,
        number(parts[2])?,
    ))
}

fn official_release_url(source: UpdateSource, tag: &str) -> AppResult<String> {
    parse_version(
        tag.strip_prefix('v')
            .ok_or_else(|| failure("发布标签必须以 v 开头"))?,
    )?;
    // The archive is a download source; release notes remain authoritative on GitHub.
    let source = if source == UpdateSource::Hk {
        UpdateSource::Github
    } else {
        source
    };
    Ok(format!("{}/tag/{tag}", source.release_base()?))
}

fn failure(message: impl Into<String>) -> AppError {
    AppError::Update(message.into())
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateChannelStatus {
    source: UpdateSource,
    version: Option<String>,
    error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppUpdateInfo {
    pub current_version: String,
    pub latest_version: String,
    pub available: bool,
    pub ahead: bool,
    pub notes: String,
    pub published_at: Option<String>,
    pub release_url: String,
    pub source: UpdateSource,
    pub channels: Vec<UpdateChannelStatus>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppUpdateStatus {
    pub phase: String,
    pub info: Option<AppUpdateInfo>,
    pub downloaded_bytes: u64,
    pub total_bytes: u64,
    pub error: Option<String>,
}

impl Default for AppUpdateStatus {
    fn default() -> Self {
        Self {
            phase: "idle".into(),
            info: None,
            downloaded_bytes: 0,
            total_bytes: 0,
            error: None,
        }
    }
}

#[derive(Clone)]
struct Candidate {
    update: Update,
    source: UpdateSource,
    version: StableVersion,
    sha256: String,
    size: u64,
}

#[derive(Default)]
struct Session {
    status: AppUpdateStatus,
    candidates: Vec<Candidate>,
    // Only the native, signature-verified download path can populate this.
    // The frontend never supplies installation bytes, a path, or a download URL.
    ready: Option<(Candidate, Vec<u8>)>,
}

#[derive(Default)]
pub struct AppUpdateManager {
    session: Arc<Mutex<Session>>,
    busy: Arc<AtomicBool>,
    cancelled: AtomicBool,
}

struct Operation {
    session: Arc<Mutex<Session>>,
    busy: Arc<AtomicBool>,
}
impl Drop for Operation {
    fn drop(&mut self) {
        if let Ok(mut session) = self.session.lock() {
            if matches!(
                session.status.phase.as_str(),
                "checking" | "downloading" | "installing"
            ) {
                session.status.phase = "failed".into();
                session.status.error = Some("操作已中断，请重试".into());
            }
        }
        self.busy.store(false, Ordering::Release);
    }
}

impl AppUpdateManager {
    fn lock(&self) -> AppResult<MutexGuard<'_, Session>> {
        self.session.lock().map_err(|_| failure("更新状态锁不可用"))
    }
    fn begin(&self) -> AppResult<Operation> {
        self.busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| AppError::Conflict("已有更新操作正在进行".into()))?;
        self.cancelled.store(false, Ordering::Release);
        Ok(Operation {
            session: self.session.clone(),
            busy: self.busy.clone(),
        })
    }
    fn finish_error(&self, error: &AppError) -> AppErrorDto {
        if let Ok(mut session) = self.lock() {
            session.status.phase = if self.cancelled.load(Ordering::Acquire) {
                "cancelled"
            } else {
                "failed"
            }
            .into();
            session.status.error = Some(error.to_string());
        }
        error.dto()
    }
}

fn legacy_artifact_name(target: &str, version: &str) -> AppResult<String> {
    parse_version(version)?;
    let suffix = match target {
        "windows-x86_64" | "windows-x86_64-nsis" => "x64-setup.exe",
        "windows-aarch64" | "windows-aarch64-nsis" => "arm64-setup.exe",
        "windows-x86_64-msi" => "x64_en-US.msi",
        "windows-aarch64-msi" => "arm64_en-US.msi",
        "darwin-x86_64" => "x64.app.tar.gz",
        "darwin-aarch64" => "aarch64.app.tar.gz",
        "linux-x86_64" => "amd64.AppImage",
        "linux-aarch64" => "aarch64.AppImage",
        _ => return Err(failure("此系统架构暂不支持内置更新")),
    };
    Ok(format!("RouteDeck_{version}_{suffix}"))
}

fn artifact_name(target: &str, version: &str) -> AppResult<String> {
    Ok(legacy_artifact_name(target, version)?.replacen("RouteDeck_", "Serylane_", 1))
}

fn asset_url(source: UpdateSource, version: &str, filename: &str) -> AppResult<String> {
    let base = source.release_base()?;
    Ok(if source == UpdateSource::Hk {
        format!("{base}/v{version}/{filename}")
    } else {
        format!("{base}/download/v{version}/{filename}")
    })
}

fn validate_asset(
    source: UpdateSource,
    target: &str,
    version: &str,
    url: &Url,
    raw: &serde_json::Value,
) -> AppResult<(String, u64)> {
    let expected = asset_url(source, version, &artifact_name(target, version)?)?;
    let legacy_github = matches!(source, UpdateSource::Github)
        && url.as_str()
            == format!(
                "{LEGACY_GITHUB_RELEASES}/download/v{version}/{}",
                legacy_artifact_name(target, version)?
            );
    let legacy_channel = asset_url(source, version, &legacy_artifact_name(target, version)?)?;
    let legacy_gitee = matches!(source, UpdateSource::Gitee)
        && url.as_str()
            == format!(
                "{LEGACY_GITEE_RELEASES}/download/v{version}/{}",
                legacy_artifact_name(target, version)?
            );
    if url.as_str() != expected && url.as_str() != legacy_channel && !legacy_github && !legacy_gitee
    {
        return Err(failure("更新包地址与官方渠道、版本或架构不一致"));
    }
    let platform = &raw["platforms"][target];
    let hash = platform["sha256"]
        .as_str()
        .filter(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or_else(|| failure("更新清单缺少有效 SHA-256 摘要"))?;
    let size = platform["size"]
        .as_u64()
        .filter(|n| *n > 0 && *n <= MAX_DOWNLOAD_BYTES)
        .ok_or_else(|| failure("更新包大小无效或超过 256 MiB 限制"))?;
    Ok((hash.to_ascii_lowercase(), size))
}

fn proxy_for_update(app: &AppHandle) -> AppResult<Option<Url>> {
    let settings = AppStorage::from_app(app)?.settings()?;
    if app.state::<MihomoRuntime>().status(Some(app)).phase == RuntimePhase::Running {
        Ok(Some(
            Url::parse(&format!("http://127.0.0.1:{}", settings.mixed_port))
                .map_err(|_| failure("本地代理地址无效"))?,
        ))
    } else {
        Ok(None)
    }
}

struct ManifestLocation {
    endpoints: Vec<Url>,
    expected_version: Option<StableVersion>,
}

fn manifest_location(source: UpdateSource, tag: Option<&str>) -> AppResult<ManifestLocation> {
    let (addresses, expected_version) = match source {
        UpdateSource::Hk => (vec![HK_MANIFEST.to_owned()], None),
        UpdateSource::Github => (
            vec![
                GITHUB_MANIFEST.to_owned(),
                LEGACY_GITHUB_MANIFEST.to_owned(),
            ],
            None,
        ),
        UpdateSource::Gitee => {
            let tag = tag.ok_or_else(|| failure("Gitee 缺少发布标签"))?;
            official_release_url(source, tag)?;
            let version = parse_version(&tag[1..])?;
            let base = source.release_base()?;
            (
                vec![
                    format!("{base}/download/{tag}/latest-serylane-gitee.json"),
                    format!("{base}/download/{tag}/latest-gitee.json"),
                ],
                Some(version),
            )
        }
        UpdateSource::Auto => return Err(failure("请指定实际发布渠道")),
    };
    Ok(ManifestLocation {
        endpoints: addresses
            .iter()
            .map(|address| Url::parse(address).map_err(|_| failure("更新清单地址无效")))
            .collect::<AppResult<Vec<_>>>()?,
        expected_version,
    })
}

async fn gitee_manifest(proxy: Option<&Url>) -> AppResult<ManifestLocation> {
    let mut builder = reqwest::Client::builder()
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(3))
        .timeout(Duration::from_secs(8));
    if let Some(proxy) = proxy {
        builder = builder
            .proxy(reqwest::Proxy::all(proxy.as_str()).map_err(|_| failure("更新代理无效"))?);
    }
    let response = builder
        .build()
        .map_err(|_| failure("无法创建 Gitee 更新客户端"))?
        .get(GITEE_RELEASE)
        .header(
            "User-Agent",
            concat!("Serylane/", env!("CARGO_PKG_VERSION")),
        )
        .send()
        .await
        .map_err(|_| failure("无法连接 Gitee 发布服务"))?;
    if !response.status().is_success() {
        return Err(failure(format!(
            "Gitee 发布服务返回 HTTP {}",
            response.status().as_u16()
        )));
    }
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| failure("读取 Gitee 发布信息失败"))?;
        if body.len().saturating_add(chunk.len()) > MAX_METADATA_BYTES {
            return Err(failure("Gitee 发布信息过大"));
        }
        body.extend_from_slice(&chunk);
    }
    let release: serde_json::Value =
        serde_json::from_slice(&body).map_err(|_| failure("Gitee 发布信息格式无效"))?;
    if release["prerelease"].as_bool() == Some(true) || release["draft"].as_bool() == Some(true) {
        return Err(failure("Gitee 尚无稳定正式版"));
    }
    let tag = release["tag_name"]
        .as_str()
        .ok_or_else(|| failure("Gitee 缺少发布标签"))?;
    manifest_location(UpdateSource::Gitee, Some(tag))
}

async fn check_source(
    app: &AppHandle,
    source: UpdateSource,
    proxy: Option<&Url>,
) -> AppResult<Candidate> {
    #[cfg(target_os = "linux")]
    if tauri::utils::platform::bundle_type() != Some(tauri::utils::config::BundleType::AppImage)
        || app.env().appimage.is_none()
    {
        return Err(failure(
            "此 Linux 安装不是 AppImage，请通过原 deb/rpm 包管理方式升级",
        ));
    }
    let manifest = match source {
        UpdateSource::Github | UpdateSource::Hk => manifest_location(source, None)?,
        UpdateSource::Gitee => gitee_manifest(proxy).await?,
        UpdateSource::Auto => return Err(failure("请指定实际发布渠道")),
    };
    let target = tauri_plugin_updater::target().ok_or_else(|| failure("此平台不支持更新"))?;
    let mut builder = app
        .updater_builder()
        .timeout(Duration::from_secs(10))
        // Inspect equal/older versions to report unpublished local builds, but
        // keep installation strictly newer-only in the native command below.
        .version_comparator(|_, _| true)
        // Serylane manifests come first. A pre-migration release may only
        // expose latest.json; the updater tries the bounded legacy fallback.
        .endpoints(manifest.endpoints)
        .map_err(|_| failure("更新清单地址无效"))?
        .configure_client(|client| {
            client
                .https_only(true)
                .connect_timeout(Duration::from_secs(3))
        });
    if let Some(proxy) = proxy {
        builder = builder.proxy(proxy.clone());
    }
    let mut update = builder
        .build()
        .map_err(|_| failure("此安装方式不支持内置更新；Linux 请使用 AppImage"))?
        .check()
        .await
        .map_err(|e| failure(format!("{} 更新清单不可用：{e}", source.label())))?
        .ok_or_else(|| failure("发布服务没有返回更新清单"))?;
    let version = parse_version(&update.version)?;
    if update.signature.len() > 4096
        || update.signature.trim().is_empty()
        || update.raw_json.to_string().len() > MAX_METADATA_BYTES
    {
        return Err(failure("更新清单或签名无效"));
    }
    let manifest_target = [
        format!("{target}-msi"),
        format!("{target}-nsis"),
        target.clone(),
    ]
    .into_iter()
    .find(|key| {
        update.raw_json["platforms"][key]["url"].as_str() == Some(update.download_url.as_str())
    })
    .ok_or_else(|| failure("更新包不属于当前系统架构"))?;
    let (sha256, size) = validate_asset(
        source,
        &manifest_target,
        &update.version,
        &update.download_url,
        &update.raw_json,
    )?;
    if manifest
        .expected_version
        .is_some_and(|expected| expected != version)
    {
        return Err(failure("Gitee 发布标签与更新清单版本不一致"));
    }
    update.timeout = Some(Duration::from_secs(600));
    Ok(Candidate {
        update,
        source,
        version,
        sha256,
        size,
    })
}

fn same_build(a: &Candidate, b: &Candidate) -> bool {
    a.version == b.version
        && a.sha256 == b.sha256
        && a.size == b.size
        && a.update.signature == b.update.signature
}

fn candidate_priority(
    version: StableVersion,
    source: UpdateSource,
) -> (std::cmp::Reverse<StableVersion>, u8) {
    // Prefer the domestic mirror for the newest stable build. A lagging mirror
    // must not hide a newer GitHub release or become a downgrade fallback.
    let priority = match source {
        UpdateSource::Hk => 0,
        UpdateSource::Gitee => 1,
        UpdateSource::Github => 2,
        UpdateSource::Auto => 3,
    };
    (std::cmp::Reverse(version), priority)
}

#[tauri::command]
pub fn save_update_preferences(
    app: AppHandle,
    state: State<'_, AppUpdateManager>,
    source: UpdateSource,
    auto_check: bool,
    auto_download: bool,
) -> Result<crate::models::PublicAppSettings, AppErrorDto> {
    let _operation = state.begin().map_err(|e| e.dto())?;
    let _configuration = crate::user_rules::acquire_configuration(&app).map_err(|e| e.dto())?;
    let storage = AppStorage::from_app(&app).map_err(|e| e.dto())?;
    let mut settings = storage.settings().map_err(|e| e.dto())?;
    let source_changed = settings.update_source != source;
    settings.update_source = source;
    settings.auto_check_updates = auto_check;
    settings.auto_download_updates = auto_download;
    storage.save_settings(&settings).map_err(|e| e.dto())?;
    if source_changed {
        *state.lock().map_err(|e| e.dto())? = Session::default();
    }
    Ok(crate::models::PublicAppSettings::from(&settings))
}

#[tauri::command]
pub fn app_update_status(
    state: State<'_, AppUpdateManager>,
) -> Result<AppUpdateStatus, AppErrorDto> {
    state
        .lock()
        .map(|session| session.status.clone())
        .map_err(|e| e.dto())
}

#[tauri::command]
pub async fn check_app_update(
    app: AppHandle,
    state: State<'_, AppUpdateManager>,
) -> Result<AppUpdateStatus, AppErrorDto> {
    let _operation = state.begin().map_err(|e| e.dto())?;
    {
        let mut session = state.lock().map_err(|e| e.dto())?;
        *session = Session::default();
        session.status.phase = "checking".into();
    }
    let result = async {
        let source = AppStorage::from_app(&app)?.settings()?.update_source;
        let proxy = proxy_for_update(&app)?;
        let results = match source {
            UpdateSource::Auto => {
                let (hk, gitee, github) = tokio::join!(
                    check_source(&app, UpdateSource::Hk, proxy.as_ref()),
                    check_source(&app, UpdateSource::Gitee, proxy.as_ref()),
                    check_source(&app, UpdateSource::Github, proxy.as_ref())
                );
                vec![
                    (UpdateSource::Hk, hk),
                    (UpdateSource::Gitee, gitee),
                    (UpdateSource::Github, github),
                ]
            }
            single => vec![(single, check_source(&app, single, proxy.as_ref()).await)],
        };
        let mut channels = Vec::new();
        let mut candidates = Vec::new();
        for (source, result) in results {
            match result {
                Ok(candidate) => {
                    channels.push(UpdateChannelStatus {
                        source,
                        version: Some(format!("v{}", candidate.update.version)),
                        error: None,
                    });
                    candidates.push(candidate);
                }
                Err(error) => channels.push(UpdateChannelStatus {
                    source,
                    version: None,
                    error: Some(error.to_string()),
                }),
            }
        }
        candidates.sort_by_key(|candidate| candidate_priority(candidate.version, candidate.source));
        let latest = candidates.first().ok_or_else(|| {
            failure(
                channels
                    .iter()
                    .filter_map(|c| c.error.clone())
                    .collect::<Vec<_>>()
                    .join("；"),
            )
        })?;
        let current = parse_version(env!("CARGO_PKG_VERSION"))?;
        for candidate in &candidates {
            if candidate.version == latest.version && !same_build(latest, candidate) {
                if let Some(channel) = channels
                    .iter_mut()
                    .find(|channel| channel.source == candidate.source)
                {
                    channel.error =
                        Some("此渠道同版本的安装包或签名不一致，已排除跨渠道下载回退".into());
                }
            }
        }
        let info = AppUpdateInfo {
            current_version: env!("CARGO_PKG_VERSION").into(),
            latest_version: format!("v{}", latest.update.version),
            available: latest.version > current,
            ahead: latest.version < current,
            notes: latest
                .update
                .body
                .as_deref()
                .unwrap_or_default()
                .chars()
                .take(20_000)
                .collect(),
            published_at: latest.update.raw_json["pub_date"]
                .as_str()
                .map(str::to_owned),
            release_url: official_release_url(
                latest.source,
                &format!("v{}", latest.update.version),
            )?,
            source: latest.source,
            channels,
        };
        let selected = latest.clone();
        // Never silently substitute a different build/version on a mirror.
        candidates.retain(|candidate| same_build(&selected, candidate));
        let mut session = state.lock()?;
        session.status.phase = if info.available {
            "available"
        } else if info.ahead {
            "ahead"
        } else {
            "current"
        }
        .into();
        session.status.info = Some(info);
        session.candidates = candidates;
        Ok(session.status.clone())
    }
    .await;
    result.map_err(|e| state.finish_error(&e))
}

async fn download_candidate(candidate: &Candidate, state: &AppUpdateManager) -> AppResult<Vec<u8>> {
    let oversized = AtomicBool::new(false);
    let mut received = 0_u64;
    let download = candidate.update.download(
        |length, total| {
            received = received.saturating_add(length as u64);
            if received > candidate.size || total.is_some_and(|size| size != candidate.size) {
                oversized.store(true, Ordering::Release);
            }
            if let Ok(mut session) = state.lock() {
                session.status.downloaded_bytes = received;
            }
        },
        || {},
    );
    tokio::pin!(download);
    let bytes = loop {
        tokio::select! {
            result = &mut download => break result.map_err(|e| failure(format!("下载或签名校验失败：{e}")))?,
            _ = tokio::time::sleep(Duration::from_millis(50)) => {
                if state.cancelled.load(Ordering::Acquire) { return Err(failure("下载已取消")); }
                if oversized.load(Ordering::Acquire) { return Err(failure("更新包大小与发布清单不符")); }
            }
        }
    };
    if state.cancelled.load(Ordering::Acquire) {
        return Err(failure("下载已取消"));
    }
    if oversized.load(Ordering::Acquire)
        || bytes.len() as u64 != candidate.size
        || format!("{:x}", Sha256::digest(&bytes)) != candidate.sha256
    {
        return Err(failure("更新包大小或 SHA-256 校验失败，已拒绝安装"));
    }
    Ok(bytes)
}

#[tauri::command]
pub async fn download_app_update(
    state: State<'_, AppUpdateManager>,
    version_tag: String,
) -> Result<AppUpdateStatus, AppErrorDto> {
    let _operation = state.begin().map_err(|e| e.dto())?;
    let candidates = {
        let mut session = state.lock().map_err(|e| e.dto())?;
        if !session
            .status
            .info
            .as_ref()
            .is_some_and(|info| info.available && info.latest_version == version_tag)
        {
            return Err(failure("更新版本已变化或不是新版本，请重新检查").dto());
        }
        session.ready = None;
        session.status.phase = "downloading".into();
        session.status.error = None;
        session.candidates.clone()
    };
    let mut errors = Vec::new();
    for candidate in candidates {
        {
            let mut session = state.lock().map_err(|e| e.dto())?;
            session.status.downloaded_bytes = 0;
            session.status.total_bytes = candidate.size;
            if let Some(info) = &mut session.status.info {
                info.source = candidate.source;
                info.release_url =
                    official_release_url(candidate.source, &version_tag).map_err(|e| e.dto())?;
            }
        }
        match download_candidate(&candidate, &state).await {
            Ok(bytes) => {
                let mut session = state.lock().map_err(|e| e.dto())?;
                session.status.phase = "ready".into();
                session.ready = Some((candidate, bytes));
                return Ok(session.status.clone());
            }
            Err(error) => {
                errors.push(format!("{}：{error}", candidate.source.label()));
                if state.cancelled.load(Ordering::Acquire) {
                    break;
                }
            }
        }
    }
    Err(state.finish_error(&failure(errors.join("；"))))
}

#[tauri::command]
pub fn cancel_app_update(state: State<'_, AppUpdateManager>) -> Result<(), AppErrorDto> {
    if state.lock().map_err(|e| e.dto())?.status.phase == "downloading" {
        state.cancelled.store(true, Ordering::Release);
    }
    Ok(())
}

// Injectable boundary keeps service mutation ordered with the installer. Tests
// exercise failure/panic paths without touching launchd or installing an app.
fn install_with_service_transaction(
    prepare: impl FnOnce() -> AppResult<()>,
    install: impl FnOnce() -> AppResult<()>,
    rollback: impl FnOnce() -> AppResult<()>,
) -> AppResult<()> {
    let prepare = std::panic::catch_unwind(std::panic::AssertUnwindSafe(prepare))
        .unwrap_or_else(|_| Err(failure("升级准备意外中断；尚未启动安装")));
    let result = match prepare {
        Ok(()) => std::panic::catch_unwind(std::panic::AssertUnwindSafe(install))
            .unwrap_or_else(|_| Err(failure("安装过程意外中断"))),
        Err(error) => Err(failure(format!(
            "辅助服务升级准备未完成，已停止安装：{error}"
        ))),
    };
    if let Err(error) = result {
        let recovery = std::panic::catch_unwind(std::panic::AssertUnwindSafe(rollback))
            .unwrap_or_else(|_| Err(failure("辅助服务恢复意外中断")));
        return Err(failure(match recovery {
            Ok(()) => format!("{error}。升级状态恢复步骤已完成；代理保持停止，请重新打开应用后核对状态。"),
            Err(recovery) => format!("{error}；辅助服务恢复尚未完成：{recovery}。请重新打开应用，按提示重新关联辅助服务。"),
        }));
    }
    Ok(())
}

#[tauri::command]
pub async fn install_app_update(
    app: AppHandle,
    state: State<'_, AppUpdateManager>,
    version_tag: String,
    confirmed: bool,
) -> Result<(), AppErrorDto> {
    if !confirmed {
        return Err(failure("安装需要用户确认；安装期间代理会暂时停止").dto());
    }
    let _operation = Arc::new(state.begin().map_err(|e| e.dto())?);
    let _configuration =
        Arc::new(crate::user_rules::acquire_configuration(&app).map_err(|e| e.dto())?);
    let result = async {
        let (candidate, bytes) = {
            let mut session = state.lock()?;
            if session.status.phase != "ready"
                || !session
                    .status
                    .info
                    .as_ref()
                    .is_some_and(|info| info.available && info.latest_version == version_tag)
            {
                return Err(failure("请先下载并验证当前选定的新版本"));
            }
            let (candidate, bytes) = session
                .ready
                .as_ref()
                .ok_or_else(|| failure("已验证的安装包不可用，请重新下载"))?;
            if candidate.version <= parse_version(env!("CARGO_PKG_VERSION"))?
                || bytes.len() as u64 != candidate.size
                || format!("{:x}", Sha256::digest(bytes)) != candidate.sha256
            {
                return Err(failure("安装包版本、大小或摘要校验失败"));
            }
            let ready = session
                .ready
                .take()
                .ok_or_else(|| failure("安装包已失效"))?;
            session.status.phase = "installing".into();
            ready
        };
        // No session MutexGuard is held across await. Blocking lifecycle waits
        // and archive extraction must not block the async runtime/UI executor.
        let install_app = app.clone();
        let worker_operation = _operation.clone();
        let worker_configuration = _configuration.clone();
        tauri::async_runtime::spawn_blocking(move || -> AppResult<()> {
            // Blocking work outlives cancellation of its awaiting command. Keep
            // both gates owned by the worker until rollback/install really ends.
            let _operation = worker_operation;
            let _configuration = worker_configuration;
            // No disruptive work before explicit confirmation and verified bytes.
            install_app
                .state::<crate::local_routing::LocalRoutingManager>()
                .shutdown(&install_app)?;
            install_app
                .state::<crate::openai_stability::StabilityManager>()
                .stop();
            crate::platform::restore_system_proxy(&install_app)?;
            install_app
                .state::<MihomoRuntime>()
                .stop(Some(&install_app))?;
            install_app
                .state::<crate::OpenAiPolicyTaskManager>()
                .cancel()?;
            install_app.state::<crate::GlobalTrafficMonitor>().stop();
            AppStorage::from_app(&install_app)?.mark_clean_shutdown(true)?;
            let target_version = candidate.update.version.clone();
            install_with_service_transaction(
                || crate::tun_service::prepare_app_upgrade(&install_app, &target_version),
                || {
                    candidate
                        .update
                        .install(bytes)
                        .map_err(|error| failure(format!("安装启动失败：{error}")))
                },
                || crate::tun_service::rollback_app_upgrade(&install_app),
            )
        })
        .await
        .map_err(|_| failure("安装任务意外结束；请重新打开应用核对辅助服务状态"))??;
        // Windows exits from install(); its installer relaunches Serylane.
        #[cfg(not(windows))]
        {
            app.restart()
        }
        #[cfg(windows)]
        {
            Ok(())
        }
    }
    .await;
    result.map_err(|e| state.finish_error(&e))
}

#[tauri::command]
pub fn open_official_release(
    app: AppHandle,
    source: UpdateSource,
    version_tag: String,
) -> Result<(), AppErrorDto> {
    let url = official_release_url(source, &version_tag).map_err(|e| e.dto())?;
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| failure(format!("无法打开浏览器：{e}")).dto())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn install_transaction_requires_preparation_and_rolls_back_failed_phases() {
        use std::cell::RefCell;
        for fail in [
            "none",
            "prepare",
            "prepare_panic",
            "install",
            "rollback",
            "panic",
            "rollback_panic",
        ] {
            let calls = RefCell::new(Vec::new());
            let result = install_with_service_transaction(
                || {
                    calls.borrow_mut().push("prepare");
                    if fail == "prepare_panic" {
                        panic!("fixture prepare panic")
                    }
                    if fail == "prepare" {
                        Err(failure("fixture prepare failure"))
                    } else {
                        Ok(())
                    }
                },
                || {
                    calls.borrow_mut().push("install");
                    if fail == "panic" {
                        panic!("fixture install panic")
                    }
                    if matches!(fail, "install" | "rollback" | "rollback_panic") {
                        Err(failure("fixture install failure"))
                    } else {
                        Ok(())
                    }
                },
                || {
                    calls.borrow_mut().push("rollback");
                    if fail == "rollback_panic" {
                        panic!("fixture rollback panic")
                    }
                    if fail == "rollback" {
                        Err(failure("fixture rollback failure"))
                    } else {
                        Ok(())
                    }
                },
            );
            match fail {
                "none" => {
                    assert!(result.is_ok());
                    assert_eq!(*calls.borrow(), ["prepare", "install"]);
                }
                "prepare" | "prepare_panic" => {
                    assert!(result.is_err());
                    assert_eq!(*calls.borrow(), ["prepare", "rollback"]);
                }
                _ => {
                    assert!(result.is_err());
                    assert_eq!(*calls.borrow(), ["prepare", "install", "rollback"]);
                }
            }
            if fail == "rollback" {
                let error = result.unwrap_err().to_string();
                assert!(error.contains("fixture install failure"));
                assert!(error.contains("fixture rollback failure"));
            }
        }
    }

    #[test]
    fn newest_build_prefers_hk_and_keeps_identical_mirrors_as_download_fallback() {
        let version = StableVersion(1, 2, 3);
        let mut sources = [UpdateSource::Github, UpdateSource::Gitee, UpdateSource::Hk];
        sources.sort_by_key(|source| candidate_priority(version, *source));
        assert_eq!(
            sources,
            [UpdateSource::Hk, UpdateSource::Gitee, UpdateSource::Github]
        );
        sources.reverse();
        sources.sort_by_key(|source| candidate_priority(version, *source));
        assert_eq!(
            sources,
            [UpdateSource::Hk, UpdateSource::Gitee, UpdateSource::Github]
        );
    }
    #[test]
    fn lagging_domestic_mirror_never_hides_a_newer_official_release() {
        let mut releases = [
            (StableVersion(1, 2, 3), UpdateSource::Hk),
            (StableVersion(1, 2, 3), UpdateSource::Gitee),
            (StableVersion(1, 2, 4), UpdateSource::Github),
        ];
        releases.sort_by_key(|(version, source)| candidate_priority(*version, *source));
        assert_eq!(releases[0], (StableVersion(1, 2, 4), UpdateSource::Github));
    }
    #[test]
    fn automatic_updates_remain_the_default_and_either_single_source_is_usable() {
        assert_eq!(UpdateSource::default(), UpdateSource::Auto);
        for source in [UpdateSource::Hk, UpdateSource::Gitee, UpdateSource::Github] {
            let mut sources = [source];
            sources.sort_by_key(|source| candidate_priority(StableVersion(1, 2, 3), *source));
            assert_eq!(sources, [source]);
        }
    }
    #[test]
    fn versions_are_strict_numeric_and_never_allow_prereleases_or_paths() {
        assert!(parse_version("0.10.0").unwrap() > parse_version("0.9.9").unwrap());
        for version in [
            "v1.2.3",
            "1.02.3",
            "1.2.03",
            "1.2",
            "1.2.3.4",
            "1.2.3-beta",
            "1.2.3/other",
            "1.2.18446744073709551616",
        ] {
            assert!(parse_version(version).is_err(), "{version}");
        }
    }
    #[test]
    fn official_urls_are_derived_from_source_and_strict_tag() {
        assert_eq!(
            official_release_url(UpdateSource::Github, "v0.7.7").unwrap(),
            "https://github.com/CMMUU/serylane/releases/tag/v0.7.7"
        );
        assert_eq!(
            official_release_url(UpdateSource::Gitee, "v1.2.3").unwrap(),
            "https://gitee.com/cmmuu/serylane/releases/tag/v1.2.3"
        );
        assert!(official_release_url(UpdateSource::Auto, "v1.2.3").is_err());
        assert!(official_release_url(UpdateSource::Github, "v1.2.3?redirect=evil").is_err());
    }
    #[test]
    fn branded_manifest_precedes_legacy_fallback_and_gitee_is_bound_to_tag() {
        let hk = manifest_location(UpdateSource::Hk, None).unwrap();
        assert_eq!(hk.endpoints.len(), 1);
        assert_eq!(hk.endpoints[0].as_str(), HK_MANIFEST);
        assert_eq!(
            official_release_url(UpdateSource::Hk, "v0.7.18").unwrap(),
            "https://github.com/CMMUU/serylane/releases/tag/v0.7.18"
        );
        let github = manifest_location(UpdateSource::Github, None).unwrap();
        assert_eq!(github.endpoints.len(), 2);
        assert_eq!(github.endpoints[0].as_str(), GITHUB_MANIFEST);
        assert!(github.endpoints[0]
            .path()
            .ends_with("/latest-serylane.json"));
        assert_eq!(github.endpoints[1].as_str(), LEGACY_GITHUB_MANIFEST);
        assert_eq!(github.expected_version, None);
        let gitee = manifest_location(UpdateSource::Gitee, Some("v0.7.7")).unwrap();
        assert_eq!(gitee.expected_version, Some(StableVersion(0, 7, 7)));
        assert_eq!(gitee.endpoints.len(), 2);
        assert_eq!(
            gitee.endpoints[0].as_str(),
            "https://gitee.com/cmmuu/serylane/releases/download/v0.7.7/latest-serylane-gitee.json"
        );
        assert_eq!(
            gitee.endpoints[1].as_str(),
            "https://gitee.com/cmmuu/serylane/releases/download/v0.7.7/latest-gitee.json"
        );
        assert!(manifest_location(UpdateSource::Auto, None).is_err());
        for tag in [
            None,
            Some("main"),
            Some("v0.7.7/other"),
            Some("v0.7.7?x=1"),
            Some("v0.7.07"),
        ] {
            assert!(manifest_location(UpdateSource::Gitee, tag).is_err());
        }
    }

    #[test]
    fn serylane_downloads_validate_all_architectures_and_all_channels() {
        for source in [UpdateSource::Hk, UpdateSource::Github, UpdateSource::Gitee] {
            for target in [
                "windows-x86_64",
                "windows-aarch64",
                "windows-x86_64-nsis",
                "windows-aarch64-nsis",
                "windows-x86_64-msi",
                "windows-aarch64-msi",
                "darwin-x86_64",
                "darwin-aarch64",
                "linux-x86_64",
                "linux-aarch64",
            ] {
                let mut raw = serde_json::json!({"platforms": {}});
                raw["platforms"][target] =
                    serde_json::json!({"sha256": "a".repeat(64), "size": 123});
                let address =
                    asset_url(source, "0.7.7", &artifact_name(target, "0.7.7").unwrap()).unwrap();
                assert!(address.contains("/Serylane_0.7.7_"));
                assert!(validate_asset(
                    source,
                    target,
                    "0.7.7",
                    &Url::parse(&address).unwrap(),
                    &raw
                )
                .is_ok());
                for invalid in [
                    address.replace("Serylane_", "Serylane-fake_"),
                    address.replace("https:", "http:"),
                    address.replace("v0.7.7/", "v0.7.8/"),
                    format!("{address}#other"),
                    format!("{address}?token=other"),
                ] {
                    assert!(validate_asset(
                        source,
                        target,
                        "0.7.7",
                        &Url::parse(&invalid).unwrap(),
                        &raw
                    )
                    .is_err());
                }
            }
        }
    }
    #[test]
    fn gitee_legacy_manifest_accepts_only_the_exact_old_asset_alias() {
        let raw = serde_json::json!({ "platforms": { "windows-x86_64": { "sha256": "a".repeat(64), "size": 100 } } });
        let legacy = "https://gitee.com/cmmuu/routedeck/releases/download/v0.7.6/RouteDeck_0.7.6_x64-setup.exe";
        assert!(validate_asset(
            UpdateSource::Gitee,
            "windows-x86_64",
            "0.7.6",
            &Url::parse(legacy).unwrap(),
            &raw
        )
        .is_ok());
        for invalid in [
            legacy.replace("cmmuu/", "other/"),
            legacy.replace("RouteDeck_", "Serylane_"),
            legacy.replace("v0.7.6/", "v0.7.7/"),
        ] {
            assert!(validate_asset(
                UpdateSource::Gitee,
                "windows-x86_64",
                "0.7.6",
                &Url::parse(&invalid).unwrap(),
                &raw
            )
            .is_err());
        }
    }

    #[test]
    fn github_rename_accepts_only_exact_current_and_legacy_update_assets() {
        let raw = serde_json::json!({ "platforms": { "windows-x86_64": { "sha256": "a".repeat(64), "size": 100 } } });
        for repo in ["serylane", "routedeck"] {
            let url = format!("https://github.com/CMMUU/{repo}/releases/download/v0.7.7/RouteDeck_0.7.7_x64-setup.exe");
            assert!(validate_asset(
                UpdateSource::Github,
                "windows-x86_64",
                "0.7.7",
                &Url::parse(&url).unwrap(),
                &raw
            )
            .is_ok());
            for invalid in [
                url.replace("CMMUU", "other"),
                url.replace("github.com", "github.com.attacker"),
                url.replace(repo, "other-repository"),
                url.replace("v0.7.7/", "v0.7.8/"),
                url.replace("_x64-", "_arm64-"),
                format!("{url}?redirect=other"),
            ] {
                assert!(validate_asset(
                    UpdateSource::Github,
                    "windows-x86_64",
                    "0.7.7",
                    &Url::parse(&invalid).unwrap(),
                    &raw
                )
                .is_err());
            }
        }
    }
    #[test]
    fn asset_validation_binds_channel_version_arch_hash_and_size() {
        let raw = serde_json::json!({ "platforms": { "windows-x86_64": { "sha256": "a".repeat(64), "size": 100 } } });
        let url = Url::parse("https://gitee.com/cmmuu/serylane/releases/download/v1.2.3/RouteDeck_1.2.3_x64-setup.exe").unwrap();
        assert!(validate_asset(UpdateSource::Gitee, "windows-x86_64", "1.2.3", &url, &raw).is_ok());
        assert!(
            validate_asset(UpdateSource::Github, "windows-x86_64", "1.2.3", &url, &raw).is_err()
        );
        assert!(
            validate_asset(UpdateSource::Gitee, "windows-aarch64", "1.2.3", &url, &raw).is_err()
        );
        assert!(
            validate_asset(UpdateSource::Gitee, "windows-x86_64", "1.2.4", &url, &raw).is_err()
        );
        let mut altered = raw.clone();
        altered["platforms"]["windows-x86_64"]["size"] = (MAX_DOWNLOAD_BYTES + 1).into();
        assert!(validate_asset(
            UpdateSource::Gitee,
            "windows-x86_64",
            "1.2.3",
            &url,
            &altered
        )
        .is_err());
        assert!(validate_asset(
            UpdateSource::Gitee,
            "windows-x86_64",
            "1.2.3",
            &url,
            &serde_json::json!({})
        )
        .is_err());
    }
    #[test]
    fn cancelled_install_caller_cannot_release_the_blocking_workers_busy_lease() {
        let manager = AppUpdateManager::default();
        let caller = Arc::new(manager.begin().unwrap());
        let worker = caller.clone();
        manager.lock().unwrap().status.phase = "installing".into();
        drop(caller); // the async command is cancelled, worker is still alive
        assert!(manager.begin().is_err());
        assert_eq!(manager.lock().unwrap().status.phase, "installing");
        drop(worker);
        assert_eq!(manager.lock().unwrap().status.phase, "failed");
        assert!(manager.begin().is_ok());
    }

    #[test]
    fn updater_operations_are_exclusive_and_cancellation_resets_on_retry() {
        let manager = AppUpdateManager::default();
        let operation = manager.begin().unwrap();
        assert!(manager.begin().is_err());
        manager.cancelled.store(true, Ordering::Release);
        manager.lock().unwrap().status.phase = "downloading".into();
        drop(operation);
        assert_eq!(manager.lock().unwrap().status.phase, "failed");
        let _retry = manager.begin().unwrap();
        assert!(!manager.cancelled.load(Ordering::Acquire));
    }
}
