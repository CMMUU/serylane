//! Read-only ingress observations, independent of saved domain-rule intent.
use crate::models::{NetworkMode, RuntimePhase};
use serde::Serialize;
use std::time::Duration;
use tauri::{AppHandle, Manager};

#[derive(Debug, Clone, Serialize)]
pub struct Ingress {
    pub state: IngressState,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IngressState {
    Stopped,
    SystemProxy,
    Tun,
    LocalOnly,
    Unavailable,
    Unknown,
}

impl Ingress {
    fn new(state: IngressState, message: &str) -> Self {
        Self {
            state,
            message: message.into(),
        }
    }

    pub(super) fn stopped() -> Self {
        Self::new(IngressState::Stopped, "代理核心已停止，路由当前不处理流量")
    }

    pub(super) fn unknown() -> Self {
        Self::new(
            IngressState::Unknown,
            "代理入口状态待核对，请刷新；未自动改变网络",
        )
    }

    pub(super) fn inactive(phase: RuntimePhase) -> Self {
        match phase {
            RuntimePhase::Stopped | RuntimePhase::Uninitialized | RuntimePhase::Crashed => {
                Self::stopped()
            }
            _ => Self::new(
                IngressState::Unknown,
                "代理正在启停或恢复，入口核验尚未完成",
            ),
        }
    }
}

/// Unknown observations cannot become a fabricated "off" or a system takeover.
fn classify_ingress(
    mode: NetworkMode,
    system_proxy: Option<bool>,
    tun_enabled: Option<bool>,
    tun_verified: Option<bool>,
) -> Ingress {
    match tun_enabled {
        Some(true) => match tun_verified {
            Some(true) => Ingress::new(
                IngressState::Tun,
                "已核对本次 TUN 入口；不代表外网或模型请求成功",
            ),
            Some(false) => Ingress::new(
                IngressState::Unavailable,
                "TUN 入口未通过核对，路由暂不能视为已接管",
            ),
            None => Ingress::unknown(),
        },
        Some(false) => match system_proxy {
            Some(true) => Ingress::new(
                IngressState::SystemProxy,
                "已核对系统代理指向本应用；不代表外网或模型请求成功",
            ),
            Some(false) if mode == NetworkMode::Manual => Ingress::new(
                IngressState::LocalOnly,
                "仅本地代理入口可用；仅处理明确送入核心的连接",
            ),
            Some(false) => Ingress::new(
                IngressState::Unavailable,
                "代理尚未接管系统流量；已保存的模式不代表实际开启",
            ),
            None => Ingress::unknown(),
        },
        None => Ingress::unknown(),
    }
}

async fn inspect_tun_ingress(
    config: &serde_json::Value,
    _runtime_pid: Option<u32>,
    deadline: tokio::time::Instant,
) -> Option<bool> {
    if tokio::time::Instant::now() >= deadline {
        return None;
    }
    #[cfg(windows)]
    {
        let device = config["tun"]["device"].as_str()?;
        if device != crate::effective::WINDOWS_TUN_DEVICE {
            return Some(false);
        }
        let require_ipv6 = match &config["tun"]["inet6-address"] {
            serde_json::Value::Null => false,
            serde_json::Value::Array(addresses) => !addresses.is_empty(),
            _ => return None,
        };
        tokio::time::timeout_at(
            deadline,
            tokio::task::spawn_blocking(move || {
                crate::tun_readiness::verify_current_routes(
                    crate::effective::WINDOWS_TUN_DEVICE,
                    require_ipv6,
                )
            }),
        )
        .await
        .ok()?
        .ok()
        .map(|result| result.is_ok())
    }
    #[cfg(target_os = "macos")]
    {
        let _ = config;
        let helper = tokio::time::timeout_at(
            deadline,
            tokio::task::spawn_blocking(crate::tun_service::status),
        )
        .await
        .ok()?
        .ok()?;
        trusted_helper_ingress(&helper, _runtime_pid)
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = (config, deadline);
        Some(false)
    }
}

#[cfg(any(target_os = "macos", test))]
fn trusted_helper_ingress(
    helper: &crate::tun_service::TunHelperStatus,
    runtime_pid: Option<u32>,
) -> Option<bool> {
    let expected_pid = runtime_pid?;
    match helper.state {
        crate::tun_service::TunHelperState::Checking
        | crate::tun_service::TunHelperState::Unreachable => None,
        _ => Some(
            helper.supported
                && helper.ready()
                && helper.runtime_running
                && helper.runtime_pid == Some(expected_pid),
        ),
    }
}

pub(super) async fn read_ingress(
    app: &AppHandle,
    settings: &crate::models::AppSettings,
    running: bool,
) -> Ingress {
    if !running {
        return Ingress::inactive(
            app.state::<crate::runtime::MihomoRuntime>()
                .status(Some(app))
                .phase,
        );
    }
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    let api = match crate::mihomo_api::MihomoApiClient::new(settings) {
        Ok(api) => api,
        Err(_) => return Ingress::unknown(),
    };
    let proxy_app = app.clone();
    let proxy = tokio::task::spawn_blocking(move || crate::platform::checked_status(&proxy_app));
    let (proxy, config) = tokio::join!(
        tokio::time::timeout_at(deadline, proxy),
        api.runtime_config(Duration::from_secs(2)),
    );
    let proxy = proxy
        .ok()
        .and_then(Result::ok)
        .and_then(Result::ok)
        .map(|value| value.active);
    let config = match config {
        Ok(value) => value,
        Err(_) => return Ingress::unknown(),
    };
    let tun_enabled = config["tun"]["enable"].as_bool();
    let tun_verified = if tun_enabled == Some(true) {
        let pid = app
            .state::<crate::runtime::MihomoRuntime>()
            .running_identity()
            .map(|identity| identity.0);
        inspect_tun_ingress(&config, pid, deadline).await
    } else {
        None
    };
    classify_ingress(settings.network_mode, proxy, tun_enabled, tun_verified)
}

pub(super) fn route_status(verification: &str, ingress: &Ingress) -> String {
    match verification {
        "off" => "已关闭".into(),
        "missing_target" => "出口已失效 · 请重新选择出口或关闭路由".into(),
        "waiting_core" => "已启用 · 暂停，等待代理启动".into(),
        "wrong_mode" => "已配置 · 当前非规则模式，不生效".into(),
        "verified" => match ingress.state {
            IngressState::SystemProxy | IngressState::Tun => {
                "规则已生效 · 仅处理进入核心的新连接；不代表模型请求成功".into()
            }
            IngressState::LocalOnly => "规则已载入 · 仅本地入口，系统流量未接管".into(),
            IngressState::Unavailable => "规则已核对 · 代理入口未就绪，路由暂未接管".into(),
            IngressState::Stopped => "已启用 · 暂停，等待代理启动".into(),
            IngressState::Unknown => "规则已核对 · 代理入口待核对".into(),
        },
        "mismatch" => "尚未生效 · 核心规则或模式不一致，请重新应用或关闭路由".into(),
        "unavailable" => "生效状态待核对 · 核心读取未完成，请刷新状态；未自动改变网络".into(),
        "changed" => "配置正在变化 · 请刷新状态重新核对".into(),
        _ => "已配置 · 等待核对核心规则".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observations_not_saved_preferences_determine_ingress() {
        use IngressState::*;
        for (mode, proxy, tun, verified, want) in [
            (
                NetworkMode::Manual,
                Some(false),
                Some(false),
                None,
                LocalOnly,
            ),
            (
                NetworkMode::SystemProxy,
                Some(false),
                Some(false),
                None,
                Unavailable,
            ),
            (
                NetworkMode::Tun,
                Some(false),
                Some(false),
                None,
                Unavailable,
            ),
            (
                NetworkMode::Manual,
                Some(true),
                Some(false),
                None,
                SystemProxy,
            ),
            (NetworkMode::Tun, Some(true), Some(false), None, SystemProxy),
            (
                NetworkMode::Manual,
                Some(false),
                Some(true),
                Some(true),
                Tun,
            ),
            (
                NetworkMode::Tun,
                Some(false),
                Some(true),
                Some(false),
                Unavailable,
            ),
            (NetworkMode::Tun, Some(false), Some(true), None, Unknown),
            (NetworkMode::Manual, None, Some(false), None, Unknown),
            (NetworkMode::SystemProxy, Some(true), None, None, Unknown),
            (NetworkMode::Tun, None, Some(true), Some(true), Tun),
        ] {
            assert_eq!(classify_ingress(mode, proxy, tun, verified).state, want);
        }
    }

    #[test]
    fn route_intent_and_rule_verification_do_not_fabricate_system_takeover() {
        let local = classify_ingress(NetworkMode::Manual, Some(false), Some(false), None);
        assert_eq!(
            route_status("verified", &local),
            "规则已载入 · 仅本地入口，系统流量未接管"
        );
        assert_eq!(
            route_status("waiting_core", &Ingress::stopped()),
            "已启用 · 暂停，等待代理启动"
        );
        assert_eq!(route_status("off", &local), "已关闭");
        assert_eq!(
            route_status("verified", &Ingress::unknown()),
            "规则已核对 · 代理入口待核对"
        );
        for verification in [
            "missing_target",
            "wrong_mode",
            "mismatch",
            "unavailable",
            "changed",
        ] {
            assert!(!route_status(verification, &local).contains("已生效"));
        }
    }

    #[test]
    fn starting_or_stopping_is_not_a_confirmed_stopped_snapshot() {
        for phase in [
            RuntimePhase::Validating,
            RuntimePhase::Starting,
            RuntimePhase::Stopping,
            RuntimePhase::Recovering,
        ] {
            assert_eq!(Ingress::inactive(phase).state, IngressState::Unknown);
        }
        assert_eq!(
            Ingress::inactive(RuntimePhase::Stopped).state,
            IngressState::Stopped
        );
    }

    #[test]
    fn tun_helper_identity_must_be_ready_running_and_owned_by_this_runtime() {
        use crate::tun_service::{TunHelperState, TunHelperStatus};
        let mut helper = TunHelperStatus::unsupported("fixture");
        helper.supported = true;
        helper.state = TunHelperState::Ready;
        helper.runtime_running = true;
        helper.runtime_pid = Some(42);
        assert_eq!(trusted_helper_ingress(&helper, Some(42)), Some(true));
        assert_eq!(trusted_helper_ingress(&helper, Some(43)), Some(false));
        assert_eq!(trusted_helper_ingress(&helper, None), None);
        helper.state = TunHelperState::InvalidInstallation;
        assert_eq!(trusted_helper_ingress(&helper, Some(42)), Some(false));
        helper.state = TunHelperState::Checking;
        assert_eq!(trusted_helper_ingress(&helper, Some(42)), None);
        helper.state = TunHelperState::Ready;
        helper.runtime_running = false;
        assert_eq!(trusted_helper_ingress(&helper, Some(42)), Some(false));
    }
}
