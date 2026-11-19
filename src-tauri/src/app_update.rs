use crate::error::{AppError, AppErrorDto, AppResult};
use crate::models::RuntimePhase;
use crate::runtime::MihomoRuntime;
use crate::storage::AppStorage;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use tauri::Emitter;
use tauri::{AppHandle, Manager, State};
use tauri_plugin_opener::OpenerExt;
use tauri_plugin_updater::{Update, UpdaterExt};
use url::Url;

#[cfg(test)]
mod contract_tests;
#[cfg(all(test, unix))]
mod native_tests;
#[cfg(test)]
mod recovery_tests;
mod transaction;
mod transport;

const HK_MANIFEST: &str = "https://downloads.cmmuu.com/api/releases/serylane/latest";
const HK_RELEASES: &str = "https://files.cmmuu.com/releases/serylane";
const DOWNLOAD_CENTER: &str = "https://downloads.cmmuu.com/projects/serylane";
const GITHUB_REPOSITORY: &str = "https://github.com/CMMUU/serylane";
const GITHUB_LATEST: &str = "https://api.github.com/repos/CMMUU/serylane/releases/latest";
const GITHUB_ACCELERATOR: &str = "https://serylane.cmmuu.com/api/update/github";
const CHECK_BUDGET: Duration = Duration::from_secs(12);
const MAX_METADATA_BYTES: usize = 512 * 1024;
const MAX_DOWNLOAD_BYTES: u64 = 256 * 1024 * 1024;
const STARTUP_CHECK_DELAY: Duration = Duration::from_secs(3);
const UPDATE_CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateSource {
    Github,
    GithubAccelerator,
    GithubProxy,
    #[default]
    #[serde(alias = "auto", alias = "gitee")]
    Hk,
}

const SOURCE_ORDER: [UpdateSource; 4] = [
    UpdateSource::Github,
    UpdateSource::GithubAccelerator,
    UpdateSource::GithubProxy,
    UpdateSource::Hk,
];

impl UpdateSource {
    fn label(self) -> &'static str {
        match self {
            Self::Github => "GitHub 直连",
            Self::GithubAccelerator => "GitHub 加速代理",
            Self::GithubProxy => "GitHub（Serylane 代理）",
            Self::Hk => "下载中心",
        }
    }

    fn network_path(self, proxy: Option<&Url>) -> &'static str {
        if self == Self::Hk && proxy.is_some() {
            "下载中心（Serylane 代理）"
        } else {
            self.label()
        }
    }

    fn github(self) -> bool {
        self != Self::Hk
    }

    fn direct(self) -> bool {
        matches!(self, Self::Github | Self::GithubAccelerator)
    }
}

fn available_sources(proxy_ready: bool) -> impl Iterator<Item = UpdateSource> {
    SOURCE_ORDER
        .into_iter()
        .filter(move |source| *source != UpdateSource::GithubProxy || proxy_ready)
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
    Ok(if source.github() {
        format!("{GITHUB_REPOSITORY}/releases/tag/{tag}")
    } else {
        DOWNLOAD_CENTER.into()
    })
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
pub struct UpdateAttempt {
    source: UpdateSource,
    result: String,
    stage: String,
    number: u8,
    elapsed_ms: u64,
    received_bytes: u64,
    error_kind: Option<String>,
    http_status: Option<u16>,
    detail: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppUpdateStatus {
    pub task_id: String,
    pub message: Option<String>,
    pub attempt: u8,
    pub max_attempts: u8,
    pub error_kind: Option<String>,
    pub phase: String,
    pub stage: String,
    pub network_path: String,
    pub attempts: Vec<UpdateAttempt>,
    pub last_install: Option<String>,
    pub retry_install: bool,
    pub info: Option<AppUpdateInfo>,
    pub notice_version: Option<String>,
    pub downloaded_bytes: u64,
    pub total_bytes: u64,
    pub error: Option<String>,
}

impl Default for AppUpdateStatus {
    fn default() -> Self {
        Self {
            task_id: String::new(),
            message: None,
            attempt: 0,
            max_attempts: 2,
            error_kind: None,
            phase: "idle".into(),
            stage: String::new(),
            network_path: "待确定".into(),
            attempts: Vec::new(),
            last_install: None,
            retry_install: false,
            info: None,
            notice_version: None,
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
    transport_url: Url,
    version: StableVersion,
    sha256: String,
    size: u64,
    // This is the validated manifest key, not Update.target (an OS name).
    platform_key: String,
}

#[derive(Clone, Debug)]
struct UpdatePlatform {
    base: String,
    installer: Option<&'static str>,
}
impl UpdatePlatform {
    fn native() -> AppResult<Self> {
        use tauri::utils::config::BundleType;
        Ok(Self {
            base: tauri_plugin_updater::target().ok_or_else(|| failure("此平台不支持更新"))?,
            installer: match tauri::utils::platform::bundle_type() {
                Some(BundleType::Msi) => Some("msi"),
                Some(BundleType::Nsis) => Some("nsis"),
                _ => None,
            },
        })
    }
    fn keys(&self) -> Vec<String> {
        if self.base.starts_with("windows-") {
            if self.installer == Some("msi") {
                // Our unqualified Windows alias is NSIS, never an MSI fallback.
                vec![format!("{}-msi", self.base)]
            } else {
                vec![format!("{}-nsis", self.base), self.base.clone()]
            }
        } else {
            vec![self.base.clone()]
        }
    }
}

impl Candidate {
    fn from_update(
        mut update: Update,
        source: UpdateSource,
        platform: &UpdatePlatform,
    ) -> AppResult<Self> {
        let version = parse_version(&update.version)?;
        if update.signature.len() > 4096
            || update.signature.trim().is_empty()
            || update.raw_json.to_string().len() > MAX_METADATA_BYTES
            || update.raw_json["version"].as_str() != Some(update.version.as_str())
        {
            return Err(failure("更新清单的版本或签名无效，请重新检查更新"));
        }
        let platform_key = platform
            .keys()
            .into_iter()
            .find(|key| {
                update.raw_json["platforms"][key]["url"].as_str()
                    == Some(update.download_url.as_str())
                    && update.raw_json["platforms"][key]["signature"].as_str()
                        == Some(update.signature.as_str())
            })
            .ok_or_else(|| {
                failure("更新包的平台、架构或安装格式与当前应用不匹配，请使用对应系统的完整安装包")
            })?;
        let os = platform.base.split('-').next().unwrap_or_default();
        if update.target != os && update.target != platform.base && update.target != platform_key {
            return Err(failure(
                "更新器的平台信息不一致，请重新检查更新或使用完整安装包",
            ));
        }
        let (sha256, size) = validate_asset(
            source,
            &platform_key,
            &update.version,
            &update.download_url,
            &update.raw_json,
        )?;
        let transport_url = if source == UpdateSource::GithubAccelerator {
            // Map the already validated GitHub asset. The proxy never chooses
            // the artifact or supplies its own package identity.
            let filename = update.download_url.path().rsplit('/').next().unwrap();
            accelerator_asset(&update.version, filename)?
        } else {
            update.download_url.clone()
        };
        update.timeout = Some(Duration::from_secs(600));
        Ok(Self {
            update,
            source,
            transport_url,
            version,
            sha256,
            size,
            platform_key,
        })
    }
    fn verify_for_install(
        &self,
        selected: &Self,
        bytes: &[u8],
        current: &str,
        platform: &UpdatePlatform,
    ) -> AppResult<()> {
        let checked = Self::from_update(self.update.clone(), self.source, platform)?;
        if self.version <= parse_version(current)? {
            return Err(failure("此安装包不是更新版本，请重新检查更新；未执行安装"));
        }
        if !same_build(self, selected)
            || !same_build(self, &checked)
            || self.source != selected.source
            || self.update.download_url != selected.update.download_url
            || self.transport_url != checked.transport_url
            || self.transport_url != selected.transport_url
        {
            return Err(failure(
                "待安装版本或平台信息已变化，请重新检查更新；未执行安装",
            ));
        }
        verify_download(self, bytes)
    }
}

fn verify_download(candidate: &Candidate, bytes: &[u8]) -> AppResult<()> {
    if bytes.len() as u64 != candidate.size {
        return Err(failure("更新包大小与清单不符，请重新下载；未执行安装"));
    }
    if format!("{:x}", Sha256::digest(bytes)) != candidate.sha256 {
        return Err(failure("更新包 SHA-256 校验失败，请重新下载；未执行安装"));
    }
    Ok(())
}

#[derive(Default)]
struct Session {
    status: AppUpdateStatus,
    candidates: Vec<Candidate>,
    // Only the native, signature-verified download path can populate this.
    // The frontend never supplies installation bytes, a path, or a download URL.
    ready: Option<(Candidate, Vec<u8>)>,
    handled_notices: HashSet<String>,
}

impl Session {
    fn update_notice(&mut self, info: &AppUpdateInfo, background: bool) {
        let version = &info.latest_version;
        let pending = self.status.notice_version.as_ref() == Some(version);
        self.status.notice_version =
            (info.available && (background || pending) && !self.handled_notices.contains(version))
                .then(|| version.clone());
    }

    fn dismiss_notice(&mut self, version: &str) {
        if self.status.notice_version.as_deref() == Some(version) {
            self.handled_notices.insert(version.into());
            self.status.notice_version = None;
        }
    }
}

#[derive(Default)]
pub struct AppUpdateManager {
    session: Arc<Mutex<Session>>,
    busy: Arc<AtomicBool>,
    cancelled: AtomicBool,
    receipt_checked: AtomicBool,
    checked_this_launch: AtomicBool,
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
                session.status.retry_install =
                    session.ready.is_some() && session.status.stage == "preflight";
                session.status.error = Some(
                    if session.status.stage == "restarting" {
                        "应用已替换，请重新打开 Serylane 核对版本"
                    } else {
                        "操作已中断，请核对状态后重试"
                    }
                    .into(),
                );
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
            session.status.retry_install =
                session.ready.is_some() && session.status.stage == "preflight";
            session.status.error = Some(crate::app_log::sanitize(&error.to_string()));
            crate::app_log::record(
                2,
                crate::app_log::Area::App,
                &format!(
                    "软件更新失败 [{}] task={} kind={}：{}",
                    session.status.stage,
                    session.status.task_id,
                    session.status.error_kind.as_deref().unwrap_or("operation"),
                    error
                ),
            );
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
    parse_version(version)?;
    Ok(if source.github() {
        format!("{GITHUB_REPOSITORY}/releases/download/v{version}/{filename}")
    } else {
        format!("{HK_RELEASES}/v{version}/{filename}")
    })
}

fn accelerator_asset(version: &str, filename: &str) -> AppResult<Url> {
    Url::parse(&format!("{GITHUB_ACCELERATOR}/v{version}/{filename}"))
        .map_err(|_| failure("GitHub 加速代理地址无效"))
}

fn validate_asset(
    source: UpdateSource,
    target: &str,
    version: &str,
    url: &Url,
    raw: &serde_json::Value,
) -> AppResult<(String, u64)> {
    let expected = asset_url(source, version, &artifact_name(target, version)?)?;
    let legacy_channel = asset_url(source, version, &legacy_artifact_name(target, version)?)?;
    if url.as_str() != expected && url.as_str() != legacy_channel {
        return Err(failure(
            "更新包地址与更新来源、版本或架构不一致；请稍后重试",
        ));
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
    let (address, expected_version) = if let Some(tag) = tag {
        official_release_url(source, tag)?;
        let file = if source.github() {
            "latest-serylane.json"
        } else {
            "latest-serylane-hk.json"
        };
        let address = if source == UpdateSource::GithubAccelerator {
            accelerator_asset(&tag[1..], file)?.to_string()
        } else {
            asset_url(source, &tag[1..], file)?
        };
        (address, Some(parse_version(&tag[1..])?))
    } else if source.github() {
        return Err(failure("GitHub 更新清单需要已确认的稳定发布标签"));
    } else {
        (HK_MANIFEST.to_owned(), None)
    };
    Ok(ManifestLocation {
        endpoints: vec![Url::parse(&address).map_err(|_| failure("更新清单地址无效"))?],
        expected_version,
    })
}

fn github_release_client(
    source: UpdateSource,
    proxy: Option<&Url>,
) -> AppResult<updater_http::Client> {
    // Release discovery runs before the updater plugin's first check. Select
    // the same provider the plugin uses before creating its reqwest client.
    if rustls::crypto::CryptoProvider::get_default().is_none() {
        let _ = rustls::crypto::ring::default_provider().install_default();
    }
    let mut client = updater_http::Client::builder()
        .https_only(true)
        .no_proxy()
        .redirect(updater_http::redirect::Policy::none())
        .connect_timeout(transport::CONNECT_TIMEOUT)
        .timeout(CHECK_BUDGET);
    if source == UpdateSource::GithubProxy {
        let proxy = proxy.ok_or_else(|| failure("Serylane 本地代理未运行"))?;
        client = client.proxy(
            updater_http::Proxy::all(proxy.as_str()).map_err(|_| failure("本地代理地址无效"))?,
        );
    }
    client
        .build()
        .map_err(|_| failure("无法创建 GitHub 更新连接"))
}

async fn github_release_tag(source: UpdateSource, proxy: Option<&Url>) -> AppResult<String> {
    let client = github_release_client(source, proxy)?;
    let endpoint = if source == UpdateSource::GithubAccelerator {
        format!("{GITHUB_ACCELERATOR}/latest")
    } else {
        GITHUB_LATEST.to_owned()
    };
    let mut response = client
        .get(endpoint)
        .header("User-Agent", "Serylane")
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|error| {
            failure(format!(
                "GitHub 发行查询未完成：{}",
                crate::app_log::sanitize(&error.to_string())
            ))
        })?;
    if !response.status().is_success() {
        return Err(failure(format!(
            "GitHub 发行查询返回 HTTP {}",
            response.status()
        )));
    }
    let mut content = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| failure("GitHub 发行读取未完成"))?
    {
        if content.len().saturating_add(chunk.len()) > MAX_METADATA_BYTES {
            return Err(failure("GitHub 发行数据超过大小限制"));
        }
        content.extend_from_slice(&chunk);
    }
    let release: serde_json::Value =
        serde_json::from_slice(&content).map_err(|_| failure("GitHub 发行数据格式无效"))?;
    let assets = release["assets"]
        .as_array()
        .ok_or_else(|| failure("GitHub 发行缺少工件列表"))?;
    if release["draft"].as_bool() != Some(false)
        || release["prerelease"].as_bool() != Some(false)
        || !assets
            .iter()
            .any(|asset| asset["name"].as_str() == Some("latest-serylane.json"))
    {
        return Err(failure("GitHub 当前稳定发行的更新清单尚未就绪"));
    }
    let tag = release["tag_name"]
        .as_str()
        .ok_or_else(|| failure("GitHub 发行标签无效"))?;
    official_release_url(source, tag)?;
    Ok(tag.to_owned())
}

async fn check_source_at(
    app: &AppHandle,
    source: UpdateSource,
    proxy: Option<&Url>,
    tag: Option<&str>,
) -> AppResult<Candidate> {
    #[cfg(target_os = "linux")]
    if tauri::utils::platform::bundle_type() != Some(tauri::utils::config::BundleType::AppImage)
        || app.env().appimage.is_none()
    {
        return Err(failure(
            "此 Linux 安装不是 AppImage，请从下载中心下载对应安装包后升级",
        ));
    }
    let resolved;
    let tag = if source.github() && tag.is_none() {
        resolved = github_release_tag(source, proxy).await?;
        Some(resolved.as_str())
    } else {
        tag
    };
    let manifest = manifest_location(source, tag)?;
    let platform = UpdatePlatform::native()?;
    let mut builder = app
        .updater_builder()
        .timeout(transport::METADATA_TIMEOUT)
        // Inspect equal/older versions to report unpublished local builds, but
        // keep installation strictly newer-only in the native command below.
        .version_comparator(|_, _| true)
        .endpoints(manifest.endpoints)
        .map_err(|_| failure("更新清单地址无效"))?
        .configure_client(move |client| {
            client
                .https_only(true)
                .redirect(transport::redirect_policy(source))
                .connect_timeout(transport::CONNECT_TIMEOUT)
        });
    if source.direct() {
        builder = builder.no_proxy();
    } else if let Some(proxy) = proxy {
        builder = builder.proxy(proxy.clone());
    } else if source == UpdateSource::GithubProxy {
        return Err(failure("Serylane 本地代理未运行"));
    }
    let updater = builder
        .build()
        .map_err(|_| failure("此安装方式不支持内置更新；Linux 请使用 AppImage"))?;
    let update = tokio::time::timeout(CHECK_BUDGET, updater.check())
        .await
        .map_err(|_| failure("读取更新清单超时，请稍后重试"))?
        .map_err(|e| {
            let detail = transport::TransportFailure::from_updater(e);
            failure(format!(
                "{} 更新清单读取或验证未完成，请稍后重试：{}",
                source.label(),
                detail.detail
            ))
        })?
        .ok_or_else(|| failure("更新来源没有返回更新清单，请稍后重试"))?;
    let candidate = Candidate::from_update(update, source, &platform)?;
    if manifest
        .expected_version
        .is_some_and(|expected| expected != candidate.version)
    {
        return Err(failure("所选发布标签与更新清单版本不一致"));
    }
    Ok(candidate)
}

async fn check_source(
    app: &AppHandle,
    source: UpdateSource,
    proxy: Option<&Url>,
    tag: Option<&str>,
) -> AppResult<Candidate> {
    tokio::time::timeout(CHECK_BUDGET, check_source_at(app, source, proxy, tag))
        .await
        .map_err(|_| failure(format!("{} 检查更新超时，请稍后重试", source.label())))?
}

fn same_build(a: &Candidate, b: &Candidate) -> bool {
    a.version == b.version
        && a.platform_key == b.platform_key
        && a.sha256 == b.sha256
        && a.size == b.size
        && a.update.signature == b.update.signature
}

async fn first_update_candidate<F: Future<Output = AppResult<Candidate>>>(
    proxy_ready: bool,
    task_id: &str,
    mut check: impl FnMut(UpdateSource) -> F,
) -> (Option<Candidate>, Vec<UpdateChannelStatus>) {
    let mut channels = Vec::new();
    for source in available_sources(proxy_ready) {
        let result = check(source).await;
        let detail = match &result {
            Ok(candidate) => format!(
                "version={} platform={}",
                candidate.update.version, candidate.platform_key
            ),
            Err(error) => crate::app_log::sanitize(&error.to_string()),
        };
        crate::app_log::record(
            if result.is_ok() { 1 } else { 2 },
            crate::app_log::Area::App,
            &format!("update task={task_id} source={source:?} phase=check result={detail}"),
        );
        match result {
            Ok(candidate) => {
                channels.push(UpdateChannelStatus {
                    source,
                    version: Some(format!("v{}", candidate.update.version)),
                    error: None,
                });
                return (Some(candidate), channels);
            }
            Err(_) => channels.push(UpdateChannelStatus {
                source,
                version: None,
                error: Some(detail),
            }),
        }
    }
    (None, channels)
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
    let enable_first_check = auto_check && !settings.auto_check_updates;
    settings.update_source = source;
    settings.auto_check_updates = auto_check;
    settings.auto_download_updates = auto_download;
    storage.save_settings(&settings).map_err(|e| e.dto())?;
    {
        let mut session = state.lock().map_err(|e| e.dto())?;
        if source_changed {
            session.status = AppUpdateStatus::default();
            session.candidates.clear();
            session.ready = None;
        }
        if !auto_check {
            session.status.notice_version = None;
        }
    }
    drop(_operation);
    if enable_first_check {
        tauri::async_runtime::spawn(async move { background_check(&app, Check::Startup).await });
    }
    Ok(crate::models::PublicAppSettings::from(&settings))
}

#[tauri::command]
pub fn app_update_status(
    app: AppHandle,
    state: State<'_, AppUpdateManager>,
) -> Result<AppUpdateStatus, AppErrorDto> {
    // Do not confirm/clean an old backup concurrently with a new transaction.
    if !state.receipt_checked.load(Ordering::Acquire) {
        if let Ok(_operation) = state.begin() {
            if !state.receipt_checked.swap(true, Ordering::AcqRel) {
                let summary =
                    crate::update_receipt::summary(&app).unwrap_or_else(|e| Some(e.to_string()));
                state.lock().map_err(|e| e.dto())?.status.last_install = summary;
            }
        }
    }
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
    check_update(&app, &state, Check::Manual).await
}

#[tauri::command]
pub fn dismiss_app_update_notice(
    state: State<'_, AppUpdateManager>,
    version_tag: String,
) -> Result<(), AppErrorDto> {
    state
        .lock()
        .map_err(|e| e.dto())?
        .dismiss_notice(&version_tag);
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Check {
    Manual,
    Startup,
    Periodic,
}

pub(super) fn schedule_checks(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(STARTUP_CHECK_DELAY).await;
        background_check(&app, Check::Startup).await;
        loop {
            tokio::time::sleep(UPDATE_CHECK_INTERVAL).await;
            if crate::lifecycle::PROCESS_LIFECYCLE.snapshot().phase == "closing" {
                break;
            }
            background_check(&app, Check::Periodic).await;
        }
    });
}

async fn background_check(app: &AppHandle, check: Check) {
    let state = app.state::<AppUpdateManager>();
    let _ = check_update(app, &state, check).await;
    let _ = app.emit("app-update-changed", ());
}

async fn check_update(
    app: &AppHandle,
    state: &AppUpdateManager,
    check: Check,
) -> Result<AppUpdateStatus, AppErrorDto> {
    let background = check != Check::Manual;
    let _operation = state.begin().map_err(|e| e.dto())?;
    {
        let mut session = state.lock().map_err(|e| e.dto())?;
        if background
            && (crate::lifecycle::PROCESS_LIFECYCLE.snapshot().phase != "open"
                || session.status.phase == "ready"
                || session.status.retry_install
                || check == Check::Startup && state.checked_this_launch.load(Ordering::Acquire)
                || !AppStorage::from_app(app)
                    .and_then(|storage| storage.settings())
                    .is_ok_and(|settings| settings.auto_check_updates))
        {
            return Ok(session.status.clone());
        }
        state.checked_this_launch.store(true, Ordering::Release);
        session.candidates.clear();
        session.ready = None;
        session.status = AppUpdateStatus {
            last_install: session.status.last_install.take(),
            info: session.status.info.take(),
            notice_version: session.status.notice_version.take(),
            phase: "checking".into(),
            task_id: uuid::Uuid::new_v4().to_string(),
            ..AppUpdateStatus::default()
        };
    }
    if background {
        let _ = app.emit("app-update-changed", ());
    }
    let result = async {
        let proxy = proxy_for_update(app)?;
        let task_id = state.lock()?.status.task_id.clone();
        let (latest, channels) = first_update_candidate(proxy.is_some(), &task_id, |source| {
            check_source(app, source, proxy.as_ref(), None)
        })
        .await;
        let latest = latest.ok_or_else(|| {
            failure(
                channels
                    .iter()
                    .filter_map(|c| c.error.clone())
                    .collect::<Vec<_>>()
                    .join("；"),
            )
        })?;
        let current = parse_version(env!("CARGO_PKG_VERSION"))?;
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
        let mut session = state.lock()?;
        session.status.phase = if info.available {
            "available"
        } else if info.ahead {
            "ahead"
        } else {
            "current"
        }
        .into();
        session.status.network_path = info.source.network_path(proxy.as_ref()).into();
        session.update_notice(&info, background);
        session.status.info = Some(info);
        session.candidates = vec![latest];
        Ok(session.status.clone())
    }
    .await;
    result.map_err(|error| {
        if let Ok(mut session) = state.lock() {
            session.status.info = None;
            session.status.notice_version = None;
        }
        state.finish_error(&error)
    })
}

// Test transport injection retains the real plugin signature verification.
#[cfg(test)]
async fn download_with_transport(
    candidate: &Candidate,
    transport: &Update,
    state: &AppUpdateManager,
) -> AppResult<Vec<u8>> {
    transport::download_once(candidate, transport, state, transport::Policy::default())
        .await
        .map_err(|e| failure(e.message))
}

#[tauri::command]
pub async fn download_app_update(
    app: AppHandle,
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
        session.status.retry_install = false;
        session.status.task_id = uuid::Uuid::new_v4().to_string();
        session.status.message = None;
        session.status.error_kind = None;
        session.status.phase = "downloading".into();
        session.status.stage = "download".into();
        session.status.attempts.clear();
        session.status.error = None;
        session.candidates.clone()
    };
    let result = async {
        let [selected] = candidates.as_slice() else {
            return Err(failure("更新目标不唯一，请重新检查更新"));
        };
        let proxy = proxy_for_update(&app)?;
        let start = tokio::time::Instant::now();
        let policy = transport::Policy::default();
        let mut last = failure("更新路径暂不可用，请重新检查更新");
        let routes = SOURCE_ORDER
            .into_iter()
            .skip_while(|source| *source != selected.source)
            .filter(|source| *source != UpdateSource::GithubProxy || proxy.is_some());
        for source in routes {
            if state.cancelled.load(Ordering::Acquire) {
                return Err(failure("下载已取消；当前应用与代理保持运行"));
            }
            let candidate = if source == selected.source {
                selected.clone()
            } else {
                state.lock()?.status.stage = "recovering".into();
                let candidate =
                    match check_source(&app, source, proxy.as_ref(), Some(&version_tag)).await {
                        Ok(candidate) => candidate,
                        Err(error) => {
                            if let Some(info) = &mut state.lock()?.status.info {
                                info.channels.push(UpdateChannelStatus {
                                    source,
                                    version: None,
                                    error: Some(crate::app_log::sanitize(&error.to_string())),
                                });
                            }
                            last = error;
                            continue;
                        }
                    };
                if !same_build(selected, &candidate) {
                    state.lock()?.status.error_kind = Some("identity".into());
                    return Err(failure(
                        "备用路径的更新包与已核对版本不一致；请重新检查更新",
                    ));
                }
                candidate
            };
            let total = policy.total.saturating_sub(start.elapsed());
            if total.is_zero() {
                return Err(failure("本次下载等待时间已到，请稍后重试"));
            }
            state.lock()?.status.network_path = source.network_path(proxy.as_ref()).into();
            let result = transport::recover(
                &state,
                vec![candidate],
                transport::Policy {
                    total,
                    attempts: if source.github() { 1 } else { policy.attempts },
                    ..policy
                },
                |candidate| {
                    let mut update = candidate.update.clone();
                    update.download_url = candidate.transport_url.clone();
                    update.proxy = if source.direct() { None } else { proxy.clone() };
                    update.no_proxy = source.direct();
                    update
                },
            )
            .await;
            match result {
                Ok(status) => return Ok(status),
                Err(error) => {
                    if !transport::permits_fallback(state.lock()?.status.error_kind.as_deref()) {
                        return Err(error);
                    }
                    last = error;
                }
            }
        }
        Err(last)
    }
    .await;
    result.map_err(|error| state.finish_error(&error))
}

#[tauri::command]
pub fn cancel_app_update(state: State<'_, AppUpdateManager>) -> Result<(), AppErrorDto> {
    if state.lock().map_err(|e| e.dto())?.status.phase == "downloading" {
        state.cancelled.store(true, Ordering::Release);
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn bundle_size(root: &std::path::Path) -> AppResult<u64> {
    let mut pending = vec![root.to_path_buf()];
    let mut size = 0_u64;
    let mut count = 0;
    while let Some(path) = pending.pop() {
        count += 1;
        if count > 100_000 {
            return Err(failure("应用文件数量异常，请使用完整安装包升级"));
        }
        let metadata = std::fs::symlink_metadata(&path)?;
        if metadata.is_dir() {
            for entry in std::fs::read_dir(path)? {
                pending.push(entry?.path());
            }
        } else if metadata.is_file() {
            size = size.saturating_add(metadata.len());
        }
    }
    Ok(size)
}

fn installation_reserve(compressed: u64, installed: u64) -> u64 {
    compressed
        .saturating_mul(8)
        .saturating_add(installed.saturating_mul(2))
        .saturating_add(128 * 1024 * 1024)
}

fn installation_preflight(app: &AppHandle, size: u64) -> AppResult<()> {
    let directory = app
        .path()
        .app_data_dir()
        .map_err(|e| AppError::Io(e.to_string()))?;
    let _probe = tempfile::NamedTempFile::new_in(&directory)
        .map_err(|_| failure("更新目录不可写；尚未停止代理，请检查磁盘权限"))?;
    let disks = sysinfo::Disks::new_with_refreshed_list();
    let executable = std::env::current_exe()?;
    #[cfg(target_os = "macos")]
    if executable.starts_with("/Volumes")
        || executable.to_string_lossy().contains("/AppTranslocation/")
    {
        return Err(failure(
            "请先把 Serylane 完整应用移到应用程序目录，再启动并更新；尚未停止代理",
        ));
    }
    #[cfg(target_os = "macos")]
    let installed = bundle_size(&crate::tun_service::codesign::bundle_root().map_err(failure)?)?;
    #[cfg(not(target_os = "macos"))]
    let installed = std::fs::metadata(&executable)?.len();
    let reserve = installation_reserve(size, installed);
    for path in [&directory, &executable, &std::env::temp_dir()] {
        if let Some(disk) = disks
            .iter()
            .filter(|d| path.starts_with(d.mount_point()))
            .max_by_key(|d| d.mount_point().as_os_str().len())
        {
            if disk.available_space() < reserve {
                return Err(failure("安装预留空间不足；请清理磁盘后重试，尚未停止代理"));
            }
        }
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

fn select_install(session: &mut Session, version_tag: &str) -> AppResult<Candidate> {
    if !(session.status.phase == "ready"
        || session.status.phase == "failed" && session.status.retry_install)
        || !session
            .status
            .info
            .as_ref()
            .is_some_and(|i| i.available && i.latest_version == version_tag)
    {
        return Err(failure("请先下载并验证当前选定的新版本"));
    }
    let candidate = session
        .ready
        .as_ref()
        .ok_or_else(|| failure("已验证安装包已失效，请重新下载"))?
        .0
        .clone();
    session.status.phase = "installing".into();
    session.status.stage = "preflight".into();
    session.status.error = None;
    session.status.retry_install = false;
    Ok(candidate)
}

// Restore the frontend as well as the native gate on every error/unwind path.
struct InstallLifecyclePause {
    pause: Option<crate::lifecycle::UpdatePause<'static>>,
    app: AppHandle,
}
impl InstallLifecyclePause {
    fn begin(app: &AppHandle) -> AppResult<Self> {
        let pause = crate::lifecycle::PROCESS_LIFECYCLE.pause_update()?;
        let guard = Self {
            pause: Some(pause),
            app: app.clone(),
        };
        let _ = app.emit(
            "app-lifecycle",
            crate::lifecycle::PROCESS_LIFECYCLE.snapshot(),
        );
        Ok(guard)
    }
}
impl Drop for InstallLifecyclePause {
    fn drop(&mut self) {
        drop(self.pause.take());
        let _ = self.app.emit(
            "app-lifecycle",
            crate::lifecycle::PROCESS_LIFECYCLE.snapshot(),
        );
    }
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
    let platform = UpdatePlatform::native().map_err(|e| e.dto())?;
    // Reserve before marking installing: a competing shutdown must leave the
    // verified package ready instead of stranding the UI in preflight.
    let pause = InstallLifecyclePause::begin(&app).map_err(|e| e.dto())?;
    let candidate = {
        let mut session = state.lock().map_err(|e| e.dto())?;
        select_install(&mut session, &version_tag).map_err(|e| e.dto())?
    };
    let worker_status = state.session.clone();
    let worker_operation = _operation.clone();
    let worker_configuration = _configuration.clone();
    tauri::async_runtime::spawn_blocking(move || -> AppResult<()> {
        let _operation = worker_operation;
        let _configuration = worker_configuration;
        let _pause = pause;
        let host = transaction::NativeHost { app };
        transaction::run_install(
            &worker_status,
            &candidate,
            env!("CARGO_PKG_VERSION"),
            &platform,
            &host,
        )
    })
    .await
    .map_err(|_| failure("安装工作线程意外结束，请重新打开应用核对状态").dto())?
    .map_err(|e| e.dto())
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
    fn github_discovery_builds_https_client_before_updater() {
        for source in [UpdateSource::Github, UpdateSource::GithubAccelerator] {
            github_release_client(source, None).expect("release index HTTPS client");
        }
    }

    #[test]
    fn space_budget_includes_uncompressed_backup_and_does_not_overflow() {
        assert_eq!(installation_reserve(10, 100), 128 * 1024 * 1024 + 280);
        assert_eq!(installation_reserve(u64::MAX, u64::MAX), u64::MAX);
    }
    #[test]
    fn legacy_source_settings_normalize_without_resetting_other_preferences() {
        assert_eq!(UpdateSource::default(), UpdateSource::Hk);
        for key in ["auto", "github", "gitee", "hk"] {
            let mut saved = serde_json::to_value(crate::models::AppSettings::default()).unwrap();
            saved["updateSource"] = key.into();
            saved["mixedPort"] = 17990.into();
            saved["theme"] = "dark".into();
            saved["silentStartup"] = false.into();
            saved["autoCheckUpdates"] = false.into();
            saved["autoDownloadUpdates"] = true.into();
            let migrated: crate::models::AppSettings =
                serde_json::from_value(saved.clone()).unwrap();
            assert_eq!(
                migrated.update_source,
                if key == "github" {
                    UpdateSource::Github
                } else {
                    UpdateSource::Hk
                }
            );
            saved["updateSource"] = serde_json::to_value(migrated.update_source).unwrap();
            assert_eq!(serde_json::to_value(migrated).unwrap(), saved);
        }
        assert!(serde_json::from_str::<UpdateSource>("\"untrusted\"").is_err());
        assert_eq!(UpdateSource::Hk.label(), "下载中心");
        assert_eq!(
            asset_url(UpdateSource::Hk, "1.2.3", "fixture").unwrap(),
            format!("{HK_RELEASES}/v1.2.3/fixture")
        );
    }

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
    fn manifest_and_download_actions_only_use_the_download_center() {
        let latest = manifest_location(UpdateSource::Hk, None).unwrap();
        assert_eq!(latest.endpoints.len(), 1);
        assert_eq!(latest.endpoints[0].as_str(), HK_MANIFEST);
        assert_eq!(latest.expected_version, None);
        let frozen = manifest_location(UpdateSource::Hk, Some("v1.2.3")).unwrap();
        assert_eq!(frozen.endpoints.len(), 1);
        assert_eq!(
            frozen.endpoints[0].as_str(),
            "https://files.cmmuu.com/releases/serylane/v1.2.3/latest-serylane-hk.json"
        );
        assert_eq!(frozen.expected_version, Some(StableVersion(1, 2, 3)));
        assert_eq!(
            official_release_url(UpdateSource::Hk, "v1.2.3").unwrap(),
            DOWNLOAD_CENTER
        );
        for tag in ["main", "v1.2.3/other", "v1.2.3?x=1", "v1.02.3"] {
            assert!(manifest_location(UpdateSource::Hk, Some(tag)).is_err());
            assert!(official_release_url(UpdateSource::Hk, tag).is_err());
        }
    }

    #[test]
    fn center_assets_bind_all_architectures_versions_and_exact_origins() {
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
            let raw =
                serde_json::json!({"platforms": {target: {"sha256": "a".repeat(64), "size": 123}}});
            let filename = artifact_name(target, "1.2.3").unwrap();
            let address = asset_url(UpdateSource::Hk, "1.2.3", &filename).unwrap();
            assert!(validate_asset(
                UpdateSource::Hk,
                target,
                "1.2.3",
                &address.parse().unwrap(),
                &raw
            )
            .is_ok());
            for invalid in [
                address.replace("Serylane_", "Serylane-fake_"),
                address.replace("https:", "http:"),
                address.replace("v1.2.3/", "v1.2.4/"),
                address.replace("files.cmmuu.com", "files.cmmuu.com.attacker"),
                address.replace("files.cmmuu.com", "files.cmmuu.com@attacker.test"),
                format!("{address}#other"),
                format!("{address}?fallback=github"),
                format!("https://github.com/CMMUU/serylane/releases/download/v1.2.3/{filename}"),
                format!("https://gitee.com/cmmuu/serylane/releases/download/v1.2.3/{filename}"),
            ] {
                assert!(
                    validate_asset(
                        UpdateSource::Hk,
                        target,
                        "1.2.3",
                        &invalid.parse().unwrap(),
                        &raw
                    )
                    .is_err(),
                    "{invalid}"
                );
            }
        }
    }

    #[test]
    fn previous_center_filename_remains_bound_to_the_center_and_integrity_metadata() {
        let raw = serde_json::json!({"platforms": {"windows-x86_64": {"sha256": "a".repeat(64), "size": 100}}});
        let address =
            "https://files.cmmuu.com/releases/serylane/v1.2.3/RouteDeck_1.2.3_x64-setup.exe";
        let url = address.parse().unwrap();
        assert!(validate_asset(UpdateSource::Hk, "windows-x86_64", "1.2.3", &url, &raw).is_ok());
        for target in ["windows-aarch64", "linux-x86_64", "darwin-aarch64"] {
            assert!(validate_asset(UpdateSource::Hk, target, "1.2.3", &url, &raw).is_err());
        }
        assert!(validate_asset(UpdateSource::Hk, "windows-x86_64", "1.2.4", &url, &raw).is_err());
        for field in ["sha256", "size"] {
            let mut changed = raw.clone();
            changed["platforms"]["windows-x86_64"][field] = serde_json::Value::Null;
            assert!(
                validate_asset(UpdateSource::Hk, "windows-x86_64", "1.2.3", &url, &changed)
                    .is_err()
            );
        }
        let mut changed = raw.clone();
        changed["platforms"]["windows-x86_64"]["size"] = (MAX_DOWNLOAD_BYTES + 1).into();
        assert!(
            validate_asset(UpdateSource::Hk, "windows-x86_64", "1.2.3", &url, &changed).is_err()
        );
        for origin in [
            "https://github.com/CMMUU/routedeck/releases/download",
            "https://gitee.com/cmmuu/routedeck/releases/download",
        ] {
            let foreign = address.replace(HK_RELEASES, origin).parse().unwrap();
            assert!(
                validate_asset(UpdateSource::Hk, "windows-x86_64", "1.2.3", &foreign, &raw)
                    .is_err()
            );
        }
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
