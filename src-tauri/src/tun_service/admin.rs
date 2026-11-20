use super::client;
use super::codesign;
use super::launchd;
use super::lifecycle::{ProbeRead, SharedProbe};
use super::protocol::{PLIST_NAME, PROTOCOL_VERSION};
use super::{TunHelperState, TunHelperStatus};
use crate::macos_service::{self, Service};
use crate::macos_upgrade::record_stage as record_recovery;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Mutex, MutexGuard, OnceLock,
};
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
        Ok(macos_service::REQUIRES_APPROVAL) => requires_approval(),
        Ok(_) => not_installed(),
        Err(error) => TunHelperStatus::unsupported(error),
    }
}

pub fn register() -> Result<TunHelperStatus, String> {
    let _guard = mutation()?;
    register_inner(Registration::Install)
}

/// Discover an existing service after a Finder replacement. The launchd
/// observation is a hint. The signed, installed application must also have a
/// registration recognized by Service Management.
pub(crate) fn overwrite_source() -> Result<Option<String>, String> {
    let version = launchd::older_version_now();
    record_recovery(
        "overwrite_scan",
        &format!("source={}", version.as_deref().unwrap_or("none")),
    );
    let Some(version) = version else {
        return Ok(None);
    };
    codesign::validate_installation()?;
    let status = recovery_service_status()?;
    record_recovery("overwrite_permission", &format!("sm_status={status}"));
    Ok(matches!(
        status,
        macos_service::ENABLED | macos_service::REQUIRES_APPROVAL
    )
    .then_some(version))
}

pub(crate) fn recovery_service_status() -> Result<isize, String> {
    SERVICE.ensure_settled()?;
    SERVICE.status()
}

pub(crate) fn register_after_upgrade(
    source_version: &str,
    attempted: &AtomicBool,
) -> Result<TunHelperStatus, String> {
    let _guard = mutation()?;
    // A confirmed failed launch can hold XPC replies until timeout. Use
    // fresh launchd evidence and consent before probing the replacement.
    if let Some(repaired) = repair_stale_binding(attempted)? {
        return Ok(repaired);
    }
    let observed = register_inner(Registration::Upgrade)?;
    record_recovery("handshake", &format!("state={:?}", observed.state));
    if idle_upgrade_binding(&observed, source_version) && !attempted.load(Ordering::Acquire) {
        codesign::validate_installation()?;
        // A successfully authenticated older service can still be running
        // after Finder replaced its bundle. Recheck consent, launchd's
        // registration and the absence of an active core before detaching it.
        let consent = recovery_service_status()?;
        if consent == macos_service::REQUIRES_APPROVAL {
            return Ok(requires_approval());
        }
        let confirmed = consent == macos_service::ENABLED
            && launchd::older_version_now().as_deref() == Some(source_version)
            && idle_upgrade_binding(&enabled_status(), source_version);
        if let Some(repaired) = retry_stale_binding_once(confirmed, attempted, rebind_enabled)? {
            return Ok(repaired);
        }
    }
    if matches!(
        observed.state,
        TunHelperState::NeedsRepair | TunHelperState::Checking
    ) && !observed.runtime_running
        && observed.runtime_pid.is_none()
    {
        if let Some(repaired) = repair_stale_binding(attempted)? {
            return Ok(repaired);
        }
    }
    Ok(observed)
}

fn idle_upgrade_binding(status: &TunHelperStatus, source_version: &str) -> bool {
    status.state == TunHelperState::Outdated
        && status.helper_version.as_deref() == Some(source_version)
        && !status.runtime_running
        && status.runtime_pid.is_none()
}

fn rebind_enabled() -> Result<TunHelperStatus, String> {
    rebind(|| {
        record_recovery("unregister", "request");
        SERVICE.unregister_enabled()?;
        record_recovery("unregister", "completed");
        Ok(())
    })
}

fn repair_stale_binding(attempted: &AtomicBool) -> Result<Option<TunHelperStatus>, String> {
    if attempted.load(Ordering::Acquire) || launchd::stale_version_now().is_none() {
        return Ok(None);
    }
    codesign::validate_installation()?;
    let status = recovery_service_status()?;
    if status == macos_service::REQUIRES_APPROVAL {
        record_recovery("permission", &format!("sm_status={status}"));
        return Ok(Some(requires_approval()));
    }
    // Re-read launchd after verifying the signing identity and consent.
    let version = launchd::stale_version_now();
    record_recovery(
        "stale_binding",
        &format!(
            "source={} sm_status={status}",
            version.as_deref().unwrap_or("unknown")
        ),
    );
    retry_stale_binding_once(
        version.is_some() && status == macos_service::ENABLED,
        attempted,
        rebind_enabled,
    )
}

fn retry_stale_binding_once(
    confirmed: bool,
    attempted: &AtomicBool,
    rebind: impl FnOnce() -> Result<TunHelperStatus, String>,
) -> Result<Option<TunHelperStatus>, String> {
    if confirmed && !attempted.swap(true, Ordering::AcqRel) {
        return rebind().map(Some);
    }
    Ok(None)
}

fn mutation() -> Result<MutexGuard<'static, ()>, String> {
    MUTATION
        .try_lock()
        .map_err(|_| "辅助服务正在更新，请稍后核对状态".to_string())
}

#[derive(Clone, Copy)]
enum Registration {
    Install,
    Upgrade,
    AfterRemoval,
}

fn register_inner(registration: Registration) -> Result<TunHelperStatus, String> {
    // An OS Enabled value can outlive a timed-out unregister request. Do not
    // accept its old handshake and clear an upgrade marker before completion.
    SERVICE.ensure_settled()?;
    codesign::validate_installation()?;
    reset_service_probes();
    let status = SERVICE.status()?;
    if matches!(
        registration,
        Registration::Upgrade | Registration::AfterRemoval
    ) {
        record_recovery("permission", &format!("sm_status={status}"));
    }
    match (registration, status) {
        (Registration::AfterRemoval, _) => register_after_confirmed_removal()?,
        (_, macos_service::REQUIRES_APPROVAL) => return Ok(requires_approval()),
        (_, macos_service::ENABLED) => {}
        (Registration::Upgrade, state) if macos_service::registration_absent(state) => {
            // Recovery may have been interrupted after unregister returned.
            // Check that launchd no longer has the old job before registering.
            register_after_confirmed_removal()?;
        }
        (Registration::Install, state) if macos_service::registration_absent(state) => {
            SERVICE.register()?;
        }
        _ => return Err("系统返回未知的辅助服务登记状态；未重新注册".into()),
    }
    // Registration is not readiness: require a fresh authenticated response.
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        // XPC can briefly reject while launchd is still bringing up a newly
        // registered job. Give that first handshake a bounded retry window;
        // never unregister or re-register as part of these retries.
        let observed = match SERVICE.status()? {
            macos_service::ENABLED => enabled_status(),
            macos_service::REQUIRES_APPROVAL => requires_approval(),
            state if macos_service::registration_absent(state) => not_installed(),
            _ => return Err("系统返回未知的辅助服务登记状态；尚未确认连接".into()),
        };
        if !wait_for_handshake(observed.state) || Instant::now() >= deadline {
            reset_service_probes();
            return Ok(observed);
        }
        std::thread::sleep(Duration::from_millis(150));
    }
}

pub fn unregister() -> Result<(), String> {
    let _guard = mutation()?;
    reset_service_probes();
    SERVICE.unregister()?;
    reset_service_probes();
    Ok(())
}

pub fn repair() -> Result<TunHelperStatus, String> {
    let _guard = mutation()?;
    if SERVICE.status()? == macos_service::REQUIRES_APPROVAL {
        return Err("系统尚未允许 Serylane 后台服务，请先在系统设置中允许；未重复注册".into());
    }
    // A broken replacement package must not detach an existing working service.
    codesign::validate_installation()?;
    rebind(|| SERVICE.unregister())
}

fn rebind(detach: impl FnOnce() -> Result<(), String>) -> Result<TunHelperStatus, String> {
    reset_service_probes();
    // Do not swallow failure or register while the old process is still exiting.
    detach()?;
    register_inner(Registration::AfterRemoval)
}

fn register_after_confirmed_removal() -> Result<(), String> {
    register_when_removed(
        Duration::from_secs(4),
        || {
            SERVICE.ensure_settled()?;
            Ok(macos_service::registration_absent(SERVICE.status()?)
                && launchd::confirms_absence()
                && macos_service::registration_absent(SERVICE.status()?))
        },
        || {
            record_recovery(
                "removal_confirmed",
                "confirmed by Service Management and launchd",
            );
            record_recovery("register", "request");
            SERVICE.register()?;
            record_recovery("register", "completed");
            Ok(())
        },
    )
}

fn register_when_removed(
    timeout: Duration,
    mut removed: impl FnMut() -> Result<bool, String>,
    register: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    let started = Instant::now();
    loop {
        if removed()? {
            return register();
        }
        let remaining = timeout.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            return Err("系统尚未确认旧辅助服务已退出并移除；未注册新的辅助服务".into());
        }
        std::thread::sleep(remaining.min(Duration::from_millis(150)));
    }
}

pub fn open_approval_settings() -> Result<(), String> {
    macos_service::open_settings()
}

fn enabled_status() -> TunHelperStatus {
    let probe = client::status_probe();
    let failure = if matches!(&probe, ProbeRead::Complete(Err(_))) {
        launchd::failure()
    } else {
        None
    };
    status_from_enabled_probe(probe, failure)
}

fn status_from_enabled_probe(
    probe: ProbeRead<client::RuntimeSnapshot>,
    failure: Option<launchd::Failure>,
) -> TunHelperStatus {
    let mut status = status_from_probe(probe);
    // An authenticated response beats launchd diagnostics from an old spawn.
    // A pending XPC response is not itself a failed launch.
    if status.state == TunHelperState::Unreachable && failure == Some(launchd::Failure::SpawnFailed)
    {
        status.state = TunHelperState::NeedsRepair;
        status.message = "系统有辅助服务登记，但未启动辅助程序。可在停止代理后点击“重新关联辅助服务”；不必重复授权。".into();
        status.last_error =
            Some("launchd: job state=spawn failed, last exit code=78 (EX_CONFIG)".into());
    }
    status
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

fn requires_approval() -> TunHelperStatus {
    TunHelperStatus {
        state: TunHelperState::RequiresApproval,
        message: "请在系统设置 → 通用 → 登录项与扩展中允许 Serylane 后台服务".into(),
        ..not_installed()
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

    fn failed_handshake() -> TunHelperStatus {
        status_from_enabled_probe(
            ProbeRead::Complete(Err("connection rejected".into())),
            Some(launchd::Failure::SpawnFailed),
        )
    }

    #[test]
    fn live_authenticated_response_beats_historical_launchd_diagnosis() {
        let stale = Some(launchd::Failure::SpawnFailed);
        let snapshot = client::RuntimeSnapshot {
            protocol_version: PROTOCOL_VERSION,
            helper_version: Some(env!("CARGO_PKG_VERSION").into()),
            running: true,
            pid: Some(123),
            version: None,
            config_path: None,
            last_error: None,
        };
        let status = status_from_enabled_probe(ProbeRead::Complete(Ok(snapshot)), stale);
        assert!(status.ready());
        assert!(status.runtime_running);
        assert_eq!(
            status_from_enabled_probe(ProbeRead::Checking, stale).state,
            TunHelperState::Checking
        );
        assert_eq!(failed_handshake().state, TunHelperState::NeedsRepair);
        assert!(failed_handshake().last_error.unwrap().contains("EX_CONFIG"));
    }

    #[test]
    fn upgrade_requires_a_confirmed_failed_and_enabled_job() {
        let attempted = AtomicBool::new(false);
        assert!(
            retry_stale_binding_once(false, &attempted, || panic!("must not rebind"))
                .unwrap()
                .is_none()
        );
        assert!(!attempted.load(Ordering::Acquire));
    }

    #[test]
    fn upgrade_accepts_only_the_expected_authenticated_idle_helper() {
        let mut previous = client::RuntimeSnapshot {
            protocol_version: 2,
            helper_version: Some("0.7.48".into()),
            running: false,
            pid: None,
            version: None,
            config_path: None,
            last_error: None,
        };
        let status = status_from_probe(ProbeRead::Complete(Ok(previous.clone())));
        assert!(idle_upgrade_binding(&status, "0.7.48"));
        assert!(!idle_upgrade_binding(&status, "0.7.47"));
        previous.running = true;
        previous.pid = Some(123);
        let status = status_from_probe(ProbeRead::Complete(Ok(previous)));
        assert!(!idle_upgrade_binding(&status, "0.7.48"));
    }

    #[test]
    fn stale_job_is_rebound_at_most_once_even_after_an_error() {
        for fails in [true, false] {
            let attempted = AtomicBool::new(false);
            let observed = retry_stale_binding_once(true, &attempted, || {
                if fails {
                    Err("unregister failed".into())
                } else {
                    // A successful SMAppService call is not a handshake.
                    Ok(status_from_probe(ProbeRead::Checking))
                }
            });
            assert_eq!(observed.is_err(), fails);
            if let Ok(Some(status)) = observed {
                assert!(!status.ready());
            }
            assert!(attempted.load(Ordering::Acquire));
            let status =
                retry_stale_binding_once(true, &attempted, || panic!("must not repeat")).unwrap();
            assert!(status.is_none());
        }
    }

    #[test]
    fn registration_is_not_called_when_removal_is_unconfirmed_or_failed() {
        for unconfirmed in [Ok(false), Err("permission or command failure".into())] {
            assert!(register_when_removed(
                Duration::ZERO,
                || unconfirmed.clone(),
                || panic!("old removal unverified")
            )
            .is_err());
        }
        let confirmed = std::cell::Cell::new(false);
        register_when_removed(
            Duration::ZERO,
            || {
                confirmed.set(true);
                Ok(true)
            },
            || {
                assert!(confirmed.get());
                Ok(())
            },
        )
        .unwrap();
    }

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
