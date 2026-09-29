//! Crash-recoverable service association transaction around a verified in-app
//! update. This never downloads packages, grants consent or starts a proxy.
use crate::{
    error::{AppError, AppResult},
    storage::AppStorage,
    tun_service::{self, TunHelperState, TunHelperStatus},
};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
};
use tauri::{AppHandle, Manager};

const IDENTIFIER: &str = "com.cmmuu.mihomodesktop";
static PENDING: AtomicBool = AtomicBool::new(false);
static TUN_PENDING: AtomicBool = AtomicBool::new(false);
static FAILURE: Mutex<Option<String>> = Mutex::new(None);

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Marker {
    schema: u32,
    identifier: String,
    bundle_path: PathBuf,
    source_version: String,
    target_version: String,
    tun_was_enabled: bool,
    login_was_enabled: bool,
}

impl Marker {
    fn validate(&self, identifier: &str, bundle: &Path, version: &str) -> AppResult<()> {
        if self.schema != 1
            || self.identifier != IDENTIFIER
            || identifier != IDENTIFIER
            || self.bundle_path != bundle
            || !valid_version(&self.source_version)
            || !valid_version(&self.target_version)
            || (version != self.source_version && version != self.target_version)
        {
            return Err(AppError::Platform(
                "待恢复的服务关联与当前应用位置、身份或版本不一致；请安装原位置的正确版本后重试"
                    .into(),
            ));
        }
        Ok(())
    }
}

fn valid_version(version: &str) -> bool {
    let parts: Vec<_> = version.split('.').collect();
    parts.len() == 3
        && parts.iter().all(|part| {
            !part.is_empty() && part.len() <= 10 && part.bytes().all(|byte| byte.is_ascii_digit())
        })
}

fn marker_path(app: &AppHandle) -> AppResult<PathBuf> {
    app.path()
        .app_data_dir()
        .map(|root| root.join("macos-service-upgrade-v1.json"))
        .map_err(|error| AppError::Io(error.to_string()))
}

fn load(path: &Path) -> AppResult<Option<Marker>> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 8_192 {
        return Err(AppError::Platform(
            "服务升级记录格式异常，请在设置中检查辅助服务".into(),
        ));
    }
    let mut bytes = Vec::new();
    fs::File::open(path)?.take(8_193).read_to_end(&mut bytes)?;
    if bytes.len() > 8_192 {
        return Err(AppError::Platform("服务升级记录超出大小限制".into()));
    }
    let marker = serde_json::from_slice(&bytes)
        .map_err(|_| AppError::Platform("服务升级记录格式异常，请在设置中检查辅助服务".into()))?;
    Ok(Some(marker))
}

fn persist_marker(path: &Path, marker: &Marker) -> AppResult<()> {
    let parent = path
        .parent()
        .ok_or_else(|| AppError::Io("升级记录目录缺失".into()))?;
    fs::create_dir_all(parent)?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    temp.write_all(&serde_json::to_vec(marker).map_err(|e| AppError::Io(e.to_string()))?)?;
    temp.as_file().sync_all()?;
    // NamedTempFile is private (0600). POSIX rename replaces atomically: do not
    // move the previous marker away first and create a crash window.
    temp.persist(path)
        .map_err(|error| AppError::Io(error.to_string()))?;
    fs::File::open(parent)?.sync_all()?;
    Ok(())
}

pub fn prepare(app: &AppHandle, target_version: &str) -> AppResult<()> {
    let path = marker_path(app)?;
    if load(&path)?.is_some() {
        return Err(AppError::Conflict(
            "上次服务关联尚未完成，请先修复服务后再安装更新".into(),
        ));
    }
    let settings = AppStorage::from_app(app)?.settings()?;
    let marker = Marker {
        schema: 1,
        identifier: IDENTIFIER.into(),
        bundle_path: tun_service::codesign::bundle_root()
            .map_err(AppError::Platform)?
            .canonicalize()?,
        source_version: env!("CARGO_PKG_VERSION").into(),
        target_version: target_version.into(),
        tun_was_enabled: tun_service::admin::registration_enabled().map_err(AppError::Platform)?,
        login_was_enabled: settings.launch_at_login && crate::startup_macos::native_enabled()?,
    };
    marker.validate(
        &app.config().identifier,
        &marker.bundle_path,
        env!("CARGO_PKG_VERSION"),
    )?;
    if !marker.tun_was_enabled && !marker.login_was_enabled {
        return Ok(());
    }
    // Atomic durable intent precedes either detach; any partial failure remains
    // recoverable after a process crash or an installer error.
    TUN_PENDING.store(marker.tun_was_enabled, Ordering::Release);
    detach_services(
        &marker,
        || {
            persist_marker(&path, &marker)?;
            set_pending(None);
            Ok(())
        },
        || tun_service::admin::unregister().map_err(AppError::Platform),
        crate::startup_macos::detach_for_upgrade,
    )
}

fn detach_services(
    marker: &Marker,
    persist: impl FnOnce() -> AppResult<()>,
    detach_tun: impl FnOnce() -> AppResult<()>,
    detach_login: impl FnOnce() -> AppResult<()>,
) -> AppResult<()> {
    if !marker.tun_was_enabled && !marker.login_was_enabled {
        return Ok(());
    }
    persist()?;
    if marker.tun_was_enabled {
        detach_tun()?;
    }
    if marker.login_was_enabled {
        detach_login()?;
    }
    Ok(())
}

pub fn recover(app: &AppHandle) -> AppResult<()> {
    let path = marker_path(app)?;
    let Some(marker) = load(&path)? else {
        clear_pending();
        return Ok(());
    };
    TUN_PENDING.store(marker.tun_was_enabled, Ordering::Release);
    let result = (|| -> AppResult<()> {
        let bundle = tun_service::codesign::bundle_root()
            .map_err(AppError::Platform)?
            .canonicalize()?;
        marker.validate(&app.config().identifier, &bundle, env!("CARGO_PKG_VERSION"))?;
        // Validate the app at the exact same installation location; signatures
        // and build-version handshake remain mandatory after replacement.
        tun_service::codesign::validate(&std::env::current_exe()?).map_err(AppError::Platform)?;
        if marker.tun_was_enabled {
            let status = tun_service::admin::register().map_err(AppError::Platform)?;
            if !status.ready() {
                return Err(AppError::Platform(status.message));
            }
        }
        let settings = AppStorage::from_app(app)?.settings()?;
        if marker.login_was_enabled && settings.launch_at_login {
            crate::startup_macos::restore_after_upgrade()?;
        }
        fs::remove_file(&path)?;
        // sync deletion as well: an old marker resurfacing after a power failure
        // is harmless/idempotent, but should not trigger another repair notice.
        if let Some(parent) = path.parent() {
            fs::File::open(parent)?.sync_all()?;
        }
        Ok(())
    })();
    match &result {
        Ok(()) => clear_pending(),
        Err(error) => set_pending(Some(format!(
            "新版服务关联尚未完成：{error}。请在设置中处理后重试关联；网络模式已保留。"
        ))),
    }
    result
}

pub fn forget_tun_intent(app: &AppHandle) -> AppResult<()> {
    let path = marker_path(app)?;
    let Some(mut marker) = load(&path)? else {
        return Ok(());
    };
    marker.tun_was_enabled = false;
    TUN_PENDING.store(false, Ordering::Release);
    if marker.login_was_enabled {
        persist_marker(&path, &marker)?;
    } else {
        fs::remove_file(path)?;
        clear_pending();
    }
    Ok(())
}

fn set_pending(error: Option<String>) {
    PENDING.store(true, Ordering::Release);
    if let Ok(mut failure) = FAILURE.lock() {
        *failure = error;
    }
}
fn clear_pending() {
    PENDING.store(false, Ordering::Release);
    TUN_PENDING.store(false, Ordering::Release);
    if let Ok(mut failure) = FAILURE.lock() {
        *failure = None;
    }
}

pub fn status_override() -> Option<TunHelperStatus> {
    if !PENDING.load(Ordering::Acquire) || !TUN_PENDING.load(Ordering::Acquire) {
        return None;
    }
    let failure = FAILURE.lock().ok().and_then(|value| value.clone());
    Some(TunHelperStatus {
        supported: true,
        state: if failure.is_some() {
            TunHelperState::Unreachable
        } else {
            TunHelperState::Checking
        },
        message: failure
            .clone()
            .unwrap_or_else(|| "正在与更新后的应用重新关联辅助服务，请稍候；尚未确认就绪".into()),
        protocol_version: 0,
        helper_version: None,
        runtime_running: false,
        runtime_pid: None,
        runtime_version: None,
        last_error: failure,
    })
}

/// Returns true when this function owns deferred session restoration. Read the
/// latest settings after recovery so manual Stop/preferences win over startup.
pub fn bootstrap(app: AppHandle) -> bool {
    // `Path::exists` hides access errors and follows dangling symlinks. Only a
    // definite missing entry permits normal startup/migration; all other cases
    // go through the conservative recovery path and surface their real error.
    let pending = marker_path(&app)
        .map(|path| marker_entry_requires_recovery(fs::symlink_metadata(path)))
        .unwrap_or(true);
    if !pending {
        return false;
    }
    let tun_pending = marker_path(&app)
        .and_then(|path| load(&path))
        .ok()
        .flatten()
        .is_none_or(|marker| marker.tun_was_enabled);
    TUN_PENDING.store(tun_pending, Ordering::Release);
    set_pending(None);
    let resume_generation = AppStorage::from_app(&app).ok().and_then(|storage| {
        let (settings, persistent) = (storage.settings().ok()?, storage.state().ok()?);
        app.state::<crate::session_resume::SessionResumeManager>()
            .reserve_deferred_bootstrap(&app, &settings, &persistent)
    });
    tauri::async_runtime::spawn_blocking(move || {
        let result =
            crate::user_rules::acquire_configuration(&app).and_then(|_permit| recover(&app));
        let recovered = result.is_ok();
        if let Err(error) = result {
            set_pending(Some(format!(
                "服务关联未完成：{error}；请在设置中重新关联辅助服务。"
            )));
            crate::app_log::record(
                1,
                crate::app_log::Area::Settings,
                "升级后的服务关联未完成；请在设置中核对状态",
            );
        }
        if let Ok(storage) = AppStorage::from_app(&app) {
            if let (Ok(settings), Ok(persistent)) = (storage.settings(), storage.state()) {
                if recovered {
                    let _ = crate::startup::migrate_login_entry(&app, &settings);
                }
                if let Some(generation) = resume_generation {
                    app.state::<crate::session_resume::SessionResumeManager>()
                        .bootstrap_reserved(&app, generation, &settings, &persistent);
                }
            }
        }
    });
    true
}

fn marker_entry_requires_recovery(metadata: std::io::Result<fs::Metadata>) -> bool {
    !matches!(metadata, Err(error) if error.kind() == std::io::ErrorKind::NotFound)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn marker() -> Marker {
        Marker {
            schema: 1,
            identifier: IDENTIFIER.into(),
            bundle_path: "/Applications/Serylane.app".into(),
            source_version: "0.7.17".into(),
            target_version: "0.7.18".into(),
            tun_was_enabled: true,
            login_was_enabled: false,
        }
    }
    #[test]
    fn durable_intent_precedes_detach_and_failures_stop_the_transaction() {
        use std::cell::RefCell;
        for fail in ["persist", "tun", "login", "none"] {
            let calls = RefCell::new(Vec::new());
            let run = |name| {
                calls.borrow_mut().push(name);
                if name == fail {
                    Err(AppError::Platform("fixture".into()))
                } else {
                    Ok(())
                }
            };
            let mut marker = marker();
            marker.login_was_enabled = true;
            let result =
                detach_services(&marker, || run("persist"), || run("tun"), || run("login"));
            assert_eq!(result.is_ok(), fail == "none");
            let expected: &[&str] = match fail {
                "persist" => &["persist"],
                "tun" => &["persist", "tun"],
                _ => &["persist", "tun", "login"],
            };
            assert_eq!(&*calls.borrow(), expected);
        }
        let mut marker = marker();
        marker.tun_was_enabled = false;
        detach_services(
            &marker,
            || panic!("disabled services need no transaction"),
            || panic!("do not touch TUN"),
            || panic!("do not touch login"),
        )
        .unwrap();
    }

    #[test]
    fn only_same_identity_location_and_transaction_versions_can_restore() {
        let marker = marker();
        for version in ["0.7.17", "0.7.18"] {
            assert!(marker
                .validate(IDENTIFIER, &marker.bundle_path, version)
                .is_ok());
        }
        assert!(marker
            .validate("other", &marker.bundle_path, "0.7.18")
            .is_err());
        assert!(marker
            .validate(IDENTIFIER, Path::new("/other/Serylane.app"), "0.7.18")
            .is_err());
        assert!(marker
            .validate(IDENTIFIER, &marker.bundle_path, "0.7.19")
            .is_err());
    }
    #[test]
    fn invalid_or_symlinked_marker_fails_closed_and_missing_is_idempotent() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("marker.json");
        assert!(load(&path).unwrap().is_none());
        fs::write(&path, serde_json::to_vec(&marker()).unwrap()).unwrap();
        assert!(load(&path).unwrap().unwrap().tun_was_enabled);
        fs::write(&path, b"{}").unwrap();
        assert!(load(&path).is_err());
        let link = root.path().join("link.json");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert!(load(&link).is_err());
    }
    #[test]
    fn only_definitely_missing_marker_allows_normal_startup() {
        assert!(!marker_entry_requires_recovery(Err(std::io::Error::from(
            std::io::ErrorKind::NotFound,
        ))));
        assert!(marker_entry_requires_recovery(Err(std::io::Error::from(
            std::io::ErrorKind::PermissionDenied,
        ))));
        let root = tempfile::tempdir().unwrap();
        let link = root.path().join("dangling.json");
        std::os::unix::fs::symlink(root.path().join("missing.json"), &link).unwrap();
        assert!(marker_entry_requires_recovery(fs::symlink_metadata(&link)));
        assert!(load(&link).is_err());
    }
    #[test]
    fn versions_are_bounded_plain_release_versions() {
        assert!(valid_version("0.7.18"));
        for version in [
            "",
            "v0.7.18",
            "0.7",
            "0.7.18/other",
            "0.7.18-beta",
            "0.7.9999999999999",
        ] {
            assert!(!valid_version(version));
        }
    }
}
