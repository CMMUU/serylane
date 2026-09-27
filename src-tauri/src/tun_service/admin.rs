use super::client;
use super::codesign;
use super::lifecycle::ProbeRead;
use super::protocol::{PLIST_NAME, PROTOCOL_VERSION};
use super::{TunHelperState, TunHelperStatus};
use crate::macos_service::{self, Service};
use std::sync::Mutex;
use std::time::{Duration, Instant};

static SERVICE: Service = Service::new(PLIST_NAME, false);
static MUTATION: Mutex<()> = Mutex::new(());

pub fn registration_enabled() -> Result<bool, String> {
    SERVICE.ensure_settled()?;
    Ok(SERVICE.status()? == macos_service::ENABLED)
}

pub fn status() -> TunHelperStatus {
    if SERVICE.is_unregistering() {
        let mut status = status_from_probe(ProbeRead::Checking);
        status.message = "系统仍在退出旧辅助服务；尚未确认新版关联，请稍后核对状态".into();
        return status;
    }
    if !codesign::bundle_layout_ready() {
        return TunHelperStatus::unsupported("当前应用包缺少 TUN 辅助服务，请安装完整版本");
    }
    match SERVICE.status() {
        Ok(macos_service::ENABLED) => enabled_status(),
        Ok(macos_service::REQUIRES_APPROVAL) => TunHelperStatus {
            supported: true,
            state: TunHelperState::RequiresApproval,
            message: "请在系统设置 → 通用 → 登录项与扩展中允许 Serylane 后台服务".into(),
            protocol_version: 0,
            helper_version: None,
            runtime_running: false,
            runtime_pid: None,
            runtime_version: None,
            last_error: None,
        },
        Ok(_) => not_installed(),
        Err(error) => TunHelperStatus::unsupported(error),
    }
}

pub fn register() -> Result<TunHelperStatus, String> {
    let _guard = MUTATION
        .try_lock()
        .map_err(|_| "辅助服务正在更新，请稍后核对状态".to_string())?;
    register_inner()
}

fn register_inner() -> Result<TunHelperStatus, String> {
    // An OS Enabled value can outlive a timed-out unregister request. Do not
    // accept its old handshake and clear an upgrade marker before completion.
    SERVICE.ensure_settled()?;
    if !codesign::bundle_layout_ready() {
        return Err("应用包缺少辅助服务，请重新安装完整版本".into());
    }
    // Validate actual signed executables before mutating registration. This is
    // integrity verification, not a claim of Developer ID or notarization.
    codesign::validate(&codesign::sibling_executable(
        super::protocol::HELPER_BINARY_NAME,
    )?)?;
    codesign::validate(&codesign::sibling_executable(
        super::protocol::APP_BINARY_NAME,
    )?)?;
    client::reset_probe_gate();
    if !matches!(
        SERVICE.status()?,
        macos_service::ENABLED | macos_service::REQUIRES_APPROVAL
    ) {
        SERVICE.register()?;
    }
    // Registration is not readiness: require a fresh authenticated response.
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        let observed = status();
        if observed.state != TunHelperState::Checking || Instant::now() >= deadline {
            return Ok(observed);
        }
        std::thread::sleep(Duration::from_millis(150));
    }
}

pub fn unregister() -> Result<(), String> {
    let _guard = MUTATION
        .try_lock()
        .map_err(|_| "辅助服务正在更新，请稍后核对状态".to_string())?;
    client::reset_probe_gate();
    SERVICE.unregister()?;
    client::reset_probe_gate();
    Ok(())
}

pub fn repair() -> Result<TunHelperStatus, String> {
    let _guard = MUTATION
        .try_lock()
        .map_err(|_| "辅助服务正在更新，请稍后核对状态".to_string())?;
    if SERVICE.status()? == macos_service::REQUIRES_APPROVAL {
        return Err("系统尚未允许 Serylane 后台服务，请先在系统设置中允许；未重复注册".into());
    }
    client::reset_probe_gate();
    // Do not swallow failure or register while the old process is still exiting.
    SERVICE.unregister()?;
    client::reset_probe_gate();
    register_inner()
}

pub fn open_approval_settings() -> Result<(), String> {
    macos_service::open_settings()
}

fn enabled_status() -> TunHelperStatus {
    status_from_probe(client::status_probe())
}

fn status_from_probe(probe: ProbeRead<client::RuntimeSnapshot>) -> TunHelperStatus {
    match probe {
        ProbeRead::Complete(Ok(snapshot)) if snapshot.protocol_version == PROTOCOL_VERSION
            && snapshot.helper_version.as_deref() == Some(env!("CARGO_PKG_VERSION")) => {
            TunHelperStatus {
                supported: true,
                state: TunHelperState::Ready,
                message: if snapshot.running {
                    "辅助服务已通过身份核对，TUN 内核正在运行".to_string()
                } else {
                    "辅助服务已通过身份核对，可以启动 TUN".to_string()
                },
                protocol_version: snapshot.protocol_version,
                helper_version: snapshot.helper_version,
                runtime_running: snapshot.running,
                runtime_pid: snapshot.pid,
                runtime_version: snapshot.version,
                last_error: snapshot.last_error,
            }
        }
        ProbeRead::Complete(Ok(snapshot)) => TunHelperStatus {
            supported: true,
            state: TunHelperState::Outdated,
            message: "辅助服务需要与新版应用重新关联；请先停止代理，再点击“重新关联辅助服务”"
                .to_string(),
            protocol_version: snapshot.protocol_version,
                helper_version: snapshot.helper_version,
            runtime_running: snapshot.running,
            runtime_pid: snapshot.pid,
            runtime_version: snapshot.version,
            last_error: snapshot.last_error,
        },
        ProbeRead::Checking => TunHelperStatus {
            supported: true,
            state: TunHelperState::Checking,
            message: "正在等待系统辅助服务启动；暂时无需重新授权或修复".to_string(),
            protocol_version: 0,
            helper_version: None,
            runtime_running: false,
            runtime_pid: None,
            runtime_version: None,
            last_error: None,
        },
        ProbeRead::Complete(Err(error)) => TunHelperStatus {
            supported: true,
            state: TunHelperState::Unreachable,
            message: "辅助服务已登记，但暂未建立安全连接。升级后可先停止代理，再点击“重新关联辅助服务”；详细原因见诊断。".into(),
            protocol_version: 0,
            helper_version: None,
            runtime_running: false,
            runtime_pid: None,
            runtime_version: None,
            last_error: Some(error),
        },
    }
}

fn not_installed() -> TunHelperStatus {
    TunHelperStatus {
        supported: true,
        state: TunHelperState::NotInstalled,
        message: "首次使用 TUN，请安装辅助服务并在系统设置中允许后台运行".to_string(),
        protocol_version: 0,
        helper_version: None,
        runtime_running: false,
        runtime_pid: None,
        runtime_version: None,
        last_error: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_probe_is_not_a_connection_failure() {
        let status = status_from_probe(ProbeRead::Checking);
        assert_eq!(status.state, TunHelperState::Checking);
        assert!(status.last_error.is_none());
        assert!(!status.ready());
    }

    #[test]
    fn old_helper_requires_upgrade_before_lease_scoped_operations() {
        let status = status_from_probe(ProbeRead::Complete(Ok(client::RuntimeSnapshot {
            protocol_version: 1,
            helper_version: None,
            running: false,
            pid: None,
            version: None,
            config_path: None,
            last_error: None,
        })));
        assert_eq!(status.state, TunHelperState::Outdated);
        assert!(!status.ready());
    }

    #[test]
    fn readiness_requires_the_current_helper_build_not_just_matching_protocol() {
        for version in [None, Some("0.0.0"), Some(env!("CARGO_PKG_VERSION"))] {
            let status = status_from_probe(ProbeRead::Complete(Ok(client::RuntimeSnapshot {
                protocol_version: PROTOCOL_VERSION,
                helper_version: version.map(str::to_string),
                running: false,
                pid: None,
                version: None,
                config_path: None,
                last_error: None,
            })));
            assert_eq!(status.ready(), version == Some(env!("CARGO_PKG_VERSION")));
        }
        let status = status_from_probe(ProbeRead::Checking);
        assert_eq!(
            status.protocol_version, 0,
            "expected protocol is not an observed handshake"
        );
        assert!(status.helper_version.is_none());
    }

    #[test]
    fn real_connection_failure_is_unreachable() {
        let status = status_from_probe(ProbeRead::Complete(Err("connection rejected".to_string())));
        assert_eq!(status.state, TunHelperState::Unreachable);
        assert_eq!(status.last_error.as_deref(), Some("connection rejected"));
    }
}
