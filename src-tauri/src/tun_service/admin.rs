use super::client;
use super::codesign;
use super::launchd;
use super::lifecycle::{ProbeRead, SharedProbe};
use super::protocol::{PLIST_NAME, PROTOCOL_VERSION};
use super::{TunHelperState, TunHelperStatus};
use crate::macos_service::{self, Service};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

static SERVICE: Service = Service::new(PLIST_NAME, false);
static MUTATION: Mutex<()> = Mutex::new(());

fn installation_status() -> ProbeRead<()> {
    static PROBE: OnceLock<SharedProbe<()>> = OnceLock::new();
    PROBE.get_or_init(SharedProbe::default).read(
        Duration::from_millis(150),
        Duration::from_secs(60),
        codesign::validate_installation,
    )
}

fn reset_service_probes() {
    client::reset_probe_gate();
    launchd::reset();
}

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
    match installation_status() {
        ProbeRead::Complete(Ok(())) => {}
        ProbeRead::Checking => {
            let mut status = status_from_probe(ProbeRead::Checking);
            status.message = "正在核对安装包与辅助程序的开发者签名；尚未切换网络".into();
            return status;
        }
        ProbeRead::Complete(Err(error)) => {
            let mut status = status_from_probe(ProbeRead::Complete(Err(error.clone())));
            status.state = TunHelperState::InvalidInstallation;
            status.message = error;
            return status;
        }
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
    codesign::validate_installation()?;
    reset_service_probes();
    if !matches!(
        SERVICE.status()?,
        macos_service::ENABLED | macos_service::REQUIRES_APPROVAL
    ) {
        SERVICE.register()?;
    }
    // Registration is not readiness: require a fresh authenticated response.
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        // XPC can briefly reject while launchd is still bringing up a newly
        // registered job. Give that first handshake a bounded retry window;
        // never unregister or re-register as part of these retries.
        let observed = match SERVICE.status()? {
            macos_service::ENABLED => status_from_probe(client::status_probe()),
            _ => status(),
        };
        if !wait_for_handshake(observed.state) || Instant::now() >= deadline {
            reset_service_probes();
            return Ok(observed);
        }
        std::thread::sleep(Duration::from_millis(150));
    }
}

pub fn unregister() -> Result<(), String> {
    let _guard = MUTATION
        .try_lock()
        .map_err(|_| "辅助服务正在更新，请稍后核对状态".to_string())?;
    reset_service_probes();
    SERVICE.unregister()?;
    reset_service_probes();
    Ok(())
}

pub fn repair() -> Result<TunHelperStatus, String> {
    let _guard = MUTATION
        .try_lock()
        .map_err(|_| "辅助服务正在更新，请稍后核对状态".to_string())?;
    if SERVICE.status()? == macos_service::REQUIRES_APPROVAL {
        return Err("系统尚未允许 Serylane 后台服务，请先在系统设置中允许；未重复注册".into());
    }
    // A broken replacement package must not detach an existing working service.
    codesign::validate_installation()?;
    reset_service_probes();
    // Do not swallow failure or register while the old process is still exiting.
    SERVICE.unregister()?;
    reset_service_probes();
    register_inner()
}

pub fn open_approval_settings() -> Result<(), String> {
    macos_service::open_settings()
}

fn enabled_status() -> TunHelperStatus {
    if launchd::failure() == Some(launchd::Failure::SpawnFailed) {
        let mut status = status_from_probe(ProbeRead::Complete(Err(
            "launchd: job state=spawn failed, last exit code=78 (EX_CONFIG)".into(),
        )));
        status.state = TunHelperState::NeedsRepair;
        status.message = "系统保留了辅助服务登记，但辅助程序启动失败。请先停止代理，再点击“重新关联辅助服务”；这不是重复授权能解决的问题。".into();
        return status;
    }
    status_from_probe(client::status_probe())
}

fn wait_for_handshake(state: TunHelperState) -> bool {
    matches!(
        state,
        TunHelperState::Checking | TunHelperState::Unreachable
    )
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
    fn only_transient_handshake_states_are_retried_without_reregistering() {
        for state in [TunHelperState::Checking, TunHelperState::Unreachable] {
            assert!(wait_for_handshake(state));
        }
        for state in [
            TunHelperState::Ready,
            TunHelperState::RequiresApproval,
            TunHelperState::NotInstalled,
            TunHelperState::Outdated,
            TunHelperState::NeedsRepair,
            TunHelperState::InvalidInstallation,
            TunHelperState::Unsupported,
        ] {
            assert!(!wait_for_handshake(state));
        }
    }

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
