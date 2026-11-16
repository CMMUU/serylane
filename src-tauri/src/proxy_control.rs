//! The toolbar and the macOS status panel submit the same explicit intents.
//! An action owns the admission slot through readback; background observations
//! never acquire that slot or overwrite a newer in-flight/completed action.
use crate::domain_routing::ingress::IngressState;
use crate::{
    domain_routing, dto,
    error::{AppError, AppErrorDto},
    models::{AppSettings, NetworkMode, RuntimePhase},
    runtime::{MihomoRuntime, RuntimeStatus},
    session_resume::{self, ResumePhase, SessionResumeManager},
    storage::AppStorage,
    tun_service::{self, TunHelperState, TunHelperStatus},
    ui_read,
};
use serde::{Deserialize, Serialize};
use std::sync::{Mutex, MutexGuard};
use tauri::{AppHandle, Emitter, Manager};
use uuid::Uuid;

pub(crate) const EVENT: &str = "proxy-control-state";

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Request {
    Refresh,
    Start { mode: Option<NetworkMode> },
    Switch { mode: NetworkMode },
    Stop { mode: Option<NetworkMode> },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RouteIntent {
    pub enabled: bool,
    pub expected_revision: u64,
    pub profile_id: Option<Uuid>,
    pub profile_revision: Option<Uuid>,
    pub confirmed: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Presentation {
    pub state: &'static str,
    pub label: &'static str,
    pub message: String,
    pub active: bool,
    pub system_proxy_active: bool,
    pub tun_active: bool,
}

impl Presentation {
    fn new(state: &'static str, label: &'static str, message: impl Into<String>) -> Self {
        let system_proxy_active = state == "system_proxy";
        let tun_active = state == "tun";
        Self {
            state,
            label,
            message: message.into(),
            active: system_proxy_active || tun_active,
            system_proxy_active,
            tun_active,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RouteControl {
    pub active: bool,
    pub enabled: bool,
    pub label: &'static str,
    pub message: String,
    pub ready_to_toggle: bool,
    pub intent: RouteIntent,
    pub domains: usize,
    pub target: String,
}

impl RouteControl {
    fn from_route(route: &domain_routing::Snapshot) -> Self {
        let enabled = route.document.enabled;
        let ingress = route.ingress.state;
        let verified = route.verification.as_str();
        let (label, active) = if !enabled {
            ("路由", false)
        } else if matches!(
            verified,
            "missing_target" | "wrong_mode" | "mismatch" | "unavailable" | "changed"
        ) {
            ("路由 · 未就绪", false)
        } else if ingress == IngressState::Stopped {
            ("路由 · 待启动", false)
        } else if ingress == IngressState::Unavailable {
            ("路由 · 未接管", false)
        } else if ingress == IngressState::Unknown {
            ("路由 · 待核对", false)
        } else if verified != "verified" {
            ("路由 · 核对中", false)
        } else if ingress == IngressState::LocalOnly {
            ("路由 · 本地", false)
        } else {
            (
                "路由",
                matches!(ingress, IngressState::Tun | IngressState::SystemProxy),
            )
        };
        let message = if !enabled && route.readiness != "ready" {
            match route.readiness.as_str() {
                "no_profile" => "请先在主窗口选用订阅，再设置路由出口。",
                "empty_domains" => "请在主窗口添加路由域名并保存草稿。",
                "missing_target" => "请在主窗口选择有效的路由出口并保存。",
                "wrong_mode" => "请先切换为规则模式。",
                _ => "请在主窗口核对路由设置。",
            }
            .to_string()
        } else {
            route.status.clone()
        };
        Self {
            active,
            enabled,
            label,
            message,
            ready_to_toggle: enabled || route.readiness == "ready",
            intent: RouteIntent {
                enabled: !enabled,
                expected_revision: route.document.revision,
                profile_id: route.profile_id,
                profile_revision: route.profile_revision,
                confirmed: true,
            },
            domains: route.document.draft.domains.len(),
            target: route.document.draft.target.clone(),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Snapshot {
    pub serial: u64,
    /// Counts completed write attempts, not reads. The WebView invalidates
    /// profile/node/route reads for a native write even if it partly failed.
    pub completed: u64,
    pub busy: Option<&'static str>,
    pub network: Presentation,
    pub route: Option<RouteControl>,
    pub can_start: bool,
    pub can_stop: bool,
    pub can_switch: bool,
    pub tun_supported: bool,
    pub tun_hint: String,
    pub detail: String,
    #[serde(skip)]
    pub action_serial: u64,
    #[serde(skip)]
    selected_mode: Option<NetworkMode>,
}

impl Default for Snapshot {
    fn default() -> Self {
        Self {
            serial: 0,
            completed: 0,
            busy: None,
            network: Presentation::new("checking", "状态待确认", "正在读取代理状态"),
            route: None,
            can_start: false,
            can_stop: false,
            can_switch: false,
            tun_supported: cfg!(any(target_os = "macos", windows)),
            tun_hint: String::new(),
            detail: String::new(),
            action_serial: 0,
            selected_mode: None,
        }
    }
}

#[derive(Default)]
pub(crate) struct Service {
    actions: tokio::sync::Mutex<()>,
    reads: tokio::sync::Mutex<()>,
    state: Mutex<Snapshot>,
}

impl Service {
    fn state(&self) -> MutexGuard<'_, Snapshot> {
        self.state.lock().unwrap_or_else(|error| error.into_inner())
    }

    fn begin(&self, phase: &'static str) -> Snapshot {
        let mut state = self.state();
        state.serial += 1;
        state.action_serial = state.serial;
        state.busy = Some(phase);
        state.detail.clear();
        state.clone()
    }

    fn accept_read(&self, serial: u64, mut next: Snapshot) -> Option<Snapshot> {
        let mut state = self.state();
        if state.serial != serial || state.busy.is_some() {
            return None;
        }
        next.serial = state.serial + 1;
        next.completed = state.completed;
        next.action_serial = state.action_serial;
        // A timer should not erase a failed cleanup while ownership is still
        // unknown or active. A verified off read can clear it.
        if state.network.label == "关闭未完成" {
            if next.network.state != "closed" {
                next.network = state.network.clone();
                next.can_stop = true;
                next.detail = state.detail.clone();
            }
        } else {
            next.detail = state.detail.clone();
        }
        *state = next;
        Some(state.clone())
    }

    fn finish<T>(
        &self,
        mut next: Snapshot,
        operation: &'static str,
        result: &Result<T, AppErrorDto>,
    ) -> Snapshot {
        let mut state = self.state();
        next.serial = state.serial + 1;
        next.action_serial = next.serial;
        next.completed = state.completed + if operation == "refreshing" { 0 } else { 1 };
        if let Err(error) = result {
            next.detail = [
                error.user_message.title.as_str(),
                error.user_message.description.as_str(),
            ]
            .join("：");
            if operation == "stopping" {
                next.network = Presentation::new("error", "关闭未完成", &next.detail);
                next.can_stop = true;
            }
        }
        *state = next;
        state.clone()
    }
}

struct Observed {
    settings: AppSettings,
    runtime: RuntimeStatus,
    system: Option<bool>,
    profile: (Option<Uuid>, Option<Uuid>),
    restoring: bool,
    admitting: bool,
}

fn idle(phase: RuntimePhase) -> bool {
    matches!(
        phase,
        RuntimePhase::Stopped | RuntimePhase::Uninitialized | RuntimePhase::Crashed
    )
}

fn network_view(raw: &Observed, ingress: Option<IngressState>) -> Presentation {
    let p = Presentation::new;
    if raw.restoring {
        return p("checking", "正在恢复代理", "");
    }
    match raw.runtime.phase {
        RuntimePhase::Validating | RuntimePhase::Starting => {
            return p("checking", "正在开启代理", "")
        }
        RuntimePhase::Stopping => return p("checking", "正在关闭代理", ""),
        RuntimePhase::Recovering => return p("checking", "正在恢复代理", ""),
        RuntimePhase::Crashed => return p("error", "代理异常退出", "请查看应用日志或重试启动"),
        RuntimePhase::Running => {}
        _ => {
            return match raw.system {
                None => p("checking", "关闭状态待确认", "尚未读到系统代理状态"),
                Some(true) => p("error", "关闭未完成", "核心已停止，系统代理仍指向本应用"),
                Some(false) => p("closed", "已关闭", ""),
            }
        }
    }
    if ingress == Some(IngressState::Tun) {
        return p("tun", "TUN 运行中", "");
    }
    if raw.settings.network_mode == NetworkMode::Tun {
        return match ingress {
            None | Some(IngressState::Unknown | IngressState::Stopped) => {
                p("checking", "TUN 接管待确认", "")
            }
            _ => p("unavailable", "TUN 尚未接管", ""),
        };
    }
    if raw.system == Some(true) {
        return p("system_proxy", "系统代理运行中", "");
    }
    if raw.settings.network_mode == NetworkMode::SystemProxy {
        return match raw.system {
            None => p("checking", "系统代理待确认", ""),
            _ => p(
                "unavailable",
                "系统代理未接管",
                "核心仍在运行，系统代理设置未指向本应用",
            ),
        };
    }
    p("local_only", "本地代理运行中", "仅供明确使用本地端口的应用")
}

fn project(
    raw: Observed,
    route: Option<&domain_routing::Snapshot>,
    ingress: Option<IngressState>,
    helper: Option<TunHelperStatus>,
) -> Snapshot {
    let phase = raw.runtime.phase;
    let has_profile = raw.profile.0.is_some() && raw.profile.1.is_some();
    let tun_supported = helper
        .as_ref()
        .map_or(cfg!(any(target_os = "macos", windows)), |h| h.supported);
    Snapshot {
        network: network_view(&raw, ingress),
        selected_mode: Some(raw.settings.network_mode),
        route: route.map(RouteControl::from_route),
        can_start: raw.admitting
            && has_profile
            && !raw.restoring
            && idle(phase)
            && raw.system.is_some(),
        can_stop: raw.admitting && (raw.restoring || !idle(phase) || raw.system == Some(true)),
        can_switch: raw.admitting
            && has_profile
            && !raw.restoring
            && (idle(phase) || phase == RuntimePhase::Running),
        tun_supported,
        tun_hint: helper.map_or_else(|| "TUN 权限与接管状态待核对".into(), |h| h.message),
        ..Snapshot::default()
    }
}

/// Only explicit refresh/observation needs route rules and idle helper details.
enum ReadDetail<'a> {
    Quick(Option<&'a domain_routing::Snapshot>),
    Full,
}

async fn read(app: &AppHandle, detail: ReadDetail<'_>) -> Snapshot {
    let handle = app.clone();
    let os = ui_read::run(move || {
        let storage = AppStorage::from_app(&handle).map_err(dto)?;
        let persistent = storage.state().map_err(dto)?;
        let settings = storage.settings().map_err(dto)?;
        let runtime = handle.state::<MihomoRuntime>().status(Some(&handle));
        let restoring = matches!(
            handle.state::<SessionResumeManager>().snapshot().phase,
            ResumePhase::Pending | ResumePhase::Restoring | ResumePhase::WaitingNetwork
        );
        let system = crate::platform::checked_status(&handle)
            .ok()
            .map(|status| status.active);
        Ok(Observed {
            settings,
            runtime,
            system,
            profile: (persistent.active_profile_id, persistent.active_revision_id),
            restoring,
            admitting: crate::lifecycle::PROCESS_LIFECYCLE.ticket().is_ok(),
        })
    });
    let details = async {
        match detail {
            ReadDetail::Quick(_) => (None, None),
            ReadDetail::Full => {
                let (route, helper) = tokio::join!(
                    domain_routing::verified_state(app),
                    crate::read_tun_helper_status(),
                );
                (route.ok(), helper.ok())
            }
        }
    };
    let (raw, (route, helper)) = tokio::join!(os, details);
    let Ok(raw) = raw else {
        return Snapshot::default();
    };
    let profile = raw.profile;
    let route = match detail {
        ReadDetail::Quick(known) => known,
        ReadDetail::Full => route.as_ref(),
    }
    .filter(|r| (r.profile_id, r.profile_revision) == profile);
    let ingress = match route {
        Some(route) => Some(route.ingress.state),
        None if raw.settings.network_mode == NetworkMode::Tun
            && raw.runtime.phase == RuntimePhase::Running =>
        {
            // TUN requires independent verified ownership. Do not use the
            // routing policy read as a prerequisite for network mutations.
            Some(domain_routing::proxy_ingress(app, &raw.settings).await)
        }
        None => None,
    };
    let handle = app.clone();
    let settings = raw.settings.clone();
    let identity = (raw.runtime.phase, raw.runtime.pid, raw.runtime.started_at);
    // A status read may have started before a profile change, automatic
    // recovery or a core exit. Match the observations again before publishing.
    let still_current = ui_read::run(move || {
        let storage = AppStorage::from_app(&handle).map_err(dto)?;
        let state = storage.state().map_err(dto)?;
        let runtime = handle.state::<MihomoRuntime>().status(Some(&handle));
        let latest = storage.settings().map_err(dto)?;
        Ok(latest.network_mode == settings.network_mode
            && latest.mixed_port == settings.mixed_port
            && latest.controller_port == settings.controller_port
            && latest.controller_secret == settings.controller_secret
            && (state.active_profile_id, state.active_revision_id) == profile
            && (runtime.phase, runtime.pid, runtime.started_at) == identity)
    })
    .await
    .unwrap_or(false);
    if !still_current {
        return Snapshot::default();
    }
    let mut next = project(raw, route, ingress, helper);
    if matches!(detail, ReadDetail::Quick(_)) {
        let service = app.state::<Service>();
        let previous = service.state();
        next.tun_supported = previous.tun_supported;
        next.tun_hint = previous.tun_hint.clone();
    }
    next
}

fn publish(app: &AppHandle, snapshot: &Snapshot) {
    let _ = app.emit(EVENT, snapshot.clone());
    #[cfg(target_os = "macos")]
    crate::update_macos_proxy_controls(app, snapshot);
}

pub(crate) async fn observe(app: &AppHandle) -> Snapshot {
    let service = app.state::<Service>();
    let Ok(_read) = service.reads.try_lock() else {
        return service.state().clone();
    };
    // An observation already in flight when a write finishes owns the read
    // slot. Retry it once, not a queue of stale observations.
    for _ in 0..2 {
        let serial = {
            let state = service.state();
            if state.busy.is_some() {
                return state.clone();
            }
            state.serial
        };
        let next = read(app, ReadDetail::Full).await;
        if let Some(snapshot) = service.accept_read(serial, next) {
            publish(app, &snapshot);
            return snapshot;
        }
    }
    let snapshot = service.state().clone();
    snapshot
}

#[cfg(target_os = "macos")]
pub(crate) fn current(app: &AppHandle) -> Snapshot {
    app.state::<Service>().state().clone()
}

fn busy_error() -> AppErrorDto {
    dto(AppError::Conflict(
        "代理正在执行另一项操作，请等待结果。".into(),
    ))
}

fn ensure_admitting() -> Result<(), AppErrorDto> {
    crate::lifecycle::PROCESS_LIFECYCLE
        .ticket()
        .map(|_| ())
        .map_err(|err| dto(err.into()))
}

fn observe_after_action(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move { observe(&app).await });
}

pub(crate) async fn execute(app: &AppHandle, request: Request) -> Result<Snapshot, AppErrorDto> {
    ensure_admitting()?;
    let service = app.state::<Service>();
    let slot = service.actions.try_lock().map_err(|_| busy_error())?;
    let phase = match request {
        Request::Refresh => "refreshing",
        Request::Start { .. } => "starting",
        Request::Switch { .. } => "switching",
        Request::Stop { .. } => "stopping",
    };
    publish(app, &service.begin(phase));
    let refresh = matches!(request, Request::Refresh);
    let (result, next) = if refresh {
        (Ok(Expected::Read), read(app, ReadDetail::Full).await)
    } else {
        // Hold the existing configuration permit across both the mutation
        // and its local readback; no other config writer can interleave.
        match session_resume::acquire_manual_configuration(app).await {
            Ok(_configuration) => {
                let result = perform_network(app, request).await;
                (result, read(app, ReadDetail::Quick(None)).await)
            }
            Err(error) => (Err(dto(error)), read(app, ReadDetail::Quick(None)).await),
        }
    };
    let result = result.and_then(|expected| expected.verify(&next));
    let finished = service.finish(next, phase, &result);
    publish(app, &finished);
    drop(slot);
    if !refresh {
        observe_after_action(app);
    }
    result.map(|()| finished)
}

pub(crate) async fn apply_route(
    app: &AppHandle,
    intent: RouteIntent,
) -> Result<domain_routing::Snapshot, AppErrorDto> {
    ensure_admitting()?;
    let service = app.state::<Service>();
    let slot = service.actions.try_lock().map_err(|_| busy_error())?;
    let phase = "routing";
    publish(app, &service.begin(phase));
    let result = domain_routing::apply_confirmed(app, intent).await;
    let next = read(app, ReadDetail::Quick(result.as_ref().ok())).await;
    let finished = service.finish(next, phase, &result);
    publish(app, &finished);
    drop(slot);
    observe_after_action(app);
    result
}

#[tauri::command]
pub(crate) async fn proxy_control_status(app: AppHandle) -> Snapshot {
    observe(&app).await
}

#[tauri::command]
pub(crate) async fn proxy_control_action(
    app: AppHandle,
    request: Request,
) -> Result<Snapshot, AppErrorDto> {
    // The outcome belongs to the service, not to a particular WebView or
    // a visible pop-up. Hiding either surface never abandons a transaction.
    tauri::async_runtime::spawn(async move { execute(&app, request).await })
        .await
        .map_err(|error| dto(AppError::Runtime(error.to_string())))?
}

#[derive(Debug, Eq, PartialEq)]
enum Transition {
    NoChange,
    Hot,
    Cold { stop_first: bool, start: bool },
}

fn plan(
    request: Request,
    before: NetworkMode,
    phase: RuntimePhase,
    system_active: bool,
) -> Result<(NetworkMode, Transition), AppErrorDto> {
    let running = phase == RuntimePhase::Running;
    let (next, explicit_switch) = match request {
        Request::Start { mode } => (mode.unwrap_or(before), false),
        Request::Switch { mode } => (mode, true),
        _ => return Ok((before, Transition::NoChange)),
    };
    if running
        && (!explicit_switch
            || (before == next && (next != NetworkMode::SystemProxy || system_active)))
    {
        return Ok((next, Transition::NoChange));
    }
    if !running && !idle(phase) {
        return Err(busy_error());
    }
    if running && crate::can_switch_without_restart(before, next) {
        return Ok((next, Transition::Hot));
    }
    Ok((
        next,
        Transition::Cold {
            stop_first: running,
            start: !explicit_switch || running || next != NetworkMode::Manual,
        },
    ))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Step {
    PreflightTun,
    StopPrevious,
    SelectMode(NetworkMode),
    StartCore,
}

fn transition_steps(before: NetworkMode, after: NetworkMode, transition: &Transition) -> Vec<Step> {
    let mut steps = Vec::with_capacity(4);
    if matches!(transition, Transition::NoChange) {
        return steps;
    }
    if after == NetworkMode::Tun {
        // The same plan is consumed for app and panel. A failed preflight
        // ends the sequence before any active proxy can be stopped.
        steps.push(Step::PreflightTun);
    }
    match transition {
        Transition::NoChange => {}
        Transition::Hot => steps.push(Step::SelectMode(after)),
        Transition::Cold { stop_first, start } => {
            if *stop_first {
                steps.push(Step::StopPrevious);
            }
            if before != after {
                steps.push(Step::SelectMode(after));
            }
            if *start {
                steps.push(Step::StartCore);
            }
        }
    }
    steps
}

async fn check_tun(app: &AppHandle) -> Result<(), AppErrorDto> {
    let mut helper = crate::read_tun_helper_status().await?;
    if cfg!(target_os = "macos") && helper.state == TunHelperState::NotInstalled {
        helper = tauri::async_runtime::spawn_blocking(tun_service::install)
            .await
            .map_err(|error| dto(AppError::Platform(error.to_string())))?
            .map_err(dto)?;
    }
    if !helper.ready() {
        if cfg!(target_os = "macos") && helper.state == TunHelperState::RequiresApproval {
            tauri::async_runtime::spawn_blocking(tun_service::open_approval_settings)
                .await
                .map_err(|error| dto(AppError::Platform(error.to_string())))?
                .map_err(dto)?;
        }
        let mut error = dto(AppError::Platform(helper.message.clone()));
        error.user_message.title = match helper.state {
            TunHelperState::InvalidInstallation => "此安装包暂未满足 TUN 要求",
            TunHelperState::RequiresApproval => "TUN 还需要系统授权",
            TunHelperState::NeedsRepair => "辅助服务需要重新关联",
            _ if !helper.supported => "当前平台尚未支持 TUN",
            _ => "TUN 服务尚未就绪",
        }
        .into();
        error.user_message.description = format!("{} 本次未切换网络模式。", helper.message);
        error.user_message.action = match helper.state {
            TunHelperState::InvalidInstallation => "diagnostics",
            TunHelperState::Unreachable | TunHelperState::Checking => "refresh",
            _ => "settings",
        }
        .into();
        if let Some(detail) = helper.last_error {
            error.user_message.details = crate::app_log::sanitize(&detail);
        }
        return Err(error);
    }
    crate::prepare_tun_with_permit(app.clone())
        .await
        .map_err(|mut error| {
            error.user_message.title = "TUN 预检尚未通过".into();
            error
                .user_message
                .description
                .push_str(" 本次未切换网络模式。");
            error
        })
}

async fn stop_with_permit(app: &AppHandle, notify: bool) -> Result<RuntimeStatus, AppErrorDto> {
    let handle = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        crate::stop_runtime_with_permit(&handle, false, notify).map_err(dto)
    })
    .await
    .map_err(|error| dto(AppError::Runtime(error.to_string())))?
}

#[derive(Debug, Eq, PartialEq)]
enum Expected {
    Read,
    Unchanged,
    Active(NetworkMode),
    Saved(NetworkMode),
    Closed,
}

impl Expected {
    fn verify(self, snapshot: &Snapshot) -> Result<(), AppErrorDto> {
        let valid = match self {
            Self::Read => snapshot.network.state != "checking",
            Self::Unchanged => true,
            Self::Active(NetworkMode::SystemProxy) => snapshot.network.system_proxy_active,
            Self::Active(NetworkMode::Tun) => snapshot.network.tun_active,
            Self::Active(NetworkMode::Manual) => snapshot.network.state == "local_only",
            Self::Saved(mode) => snapshot.selected_mode == Some(mode),
            Self::Closed => snapshot.network.state == "closed",
        };
        if valid {
            return Ok(());
        }
        let mut error = dto(AppError::Conflict("本次操作的实际网络状态待核对。".into()));
        error.user_message.title = "状态待核对".into();
        error.user_message.description = format!(
            "{}。请刷新状态；未自动再次执行操作。",
            snapshot.network.label
        );
        error.user_message.action = "refresh".into();
        Err(error)
    }
}

async fn perform_network(app: &AppHandle, request: Request) -> Result<Expected, AppErrorDto> {
    ensure_admitting()?;
    let core = app.state::<MihomoRuntime>();
    let storage = AppStorage::from_app(app).map_err(dto)?;
    let persistent = storage.state().map_err(dto)?;
    let before = storage.settings().map_err(dto)?.network_mode;
    let phase = core.status(Some(app)).phase;
    if let Request::Stop { mode } = request {
        if let Some(expected) = mode {
            if expected != before
                && !(expected == NetworkMode::SystemProxy
                    && before == NetworkMode::Manual
                    && crate::system_proxy_status(app.clone()).await?.active)
            {
                return Ok(Expected::Unchanged);
            }
        }
        // A second Stop after a successful first has no running/restore work.
        if idle(phase)
            && !persistent.desired_running
            && persistent.clean_shutdown
            && crate::system_proxy_status(app.clone())
                .await
                .is_ok_and(|status| !status.active)
        {
            return Ok(Expected::Closed);
        }
        stop_with_permit(app, true).await?;
        return Ok(Expected::Closed);
    }
    if persistent.active_profile_id.is_none() || persistent.active_revision_id.is_none() {
        let mut error = dto(AppError::Conflict("请先创建并激活一个配置档案".into()));
        error.user_message.action = "profiles".into();
        return Err(error);
    }
    let system_active = before == NetworkMode::SystemProxy
        && phase == RuntimePhase::Running
        && crate::system_proxy_status(app.clone()).await?.active;
    let (mode, transition) = plan(request, before, phase, system_active)?;
    transact(&mut NativeDriver(app, &core), before, mode, transition).await
}

trait NetworkDriver {
    fn admit(&self) -> Result<(), AppErrorDto>;
    fn phase(&self) -> RuntimePhase;
    async fn step(&mut self, step: Step) -> Result<(), AppErrorDto>;
}

struct NativeDriver<'a>(&'a AppHandle, &'a MihomoRuntime);

impl NetworkDriver for NativeDriver<'_> {
    fn admit(&self) -> Result<(), AppErrorDto> {
        ensure_admitting()
    }

    fn phase(&self) -> RuntimePhase {
        self.1.status(Some(self.0)).phase
    }

    async fn step(&mut self, step: Step) -> Result<(), AppErrorDto> {
        match step {
            Step::PreflightTun => check_tun(self.0).await?,
            Step::StopPrevious => {
                stop_with_permit(self.0, false).await?;
            }
            Step::SelectMode(mode) => {
                crate::set_network_mode_with_permit(self.0, self.1, mode).await?;
            }
            Step::StartCore => {
                let status = crate::start_active_with_permit(self.0, self.1).await?;
                if status.phase != RuntimePhase::Running {
                    return Err(dto(AppError::Runtime(
                        "代理尚未进入运行状态，请检查启动记录。".into(),
                    )));
                }
            }
        }
        Ok(())
    }
}

async fn transact(
    driver: &mut impl NetworkDriver,
    before: NetworkMode,
    mode: NetworkMode,
    transition: Transition,
) -> Result<Expected, AppErrorDto> {
    let mut stopped = false;
    let mut attempted_mode = false;
    let result = async {
        for step in transition_steps(before, mode, &transition) {
            driver.admit()?;
            attempted_mode |= matches!(step, Step::SelectMode(_));
            driver.step(step).await?;
            stopped |= step == Step::StopPrevious;
        }
        Ok(())
    }
    .await;
    let Err(mut error): Result<(), AppErrorDto> = result else {
        return Ok(match transition {
            Transition::Hot | Transition::Cold { start: true, .. } => Expected::Active(mode),
            Transition::Cold { start: false, .. } => Expected::Saved(mode),
            Transition::NoChange => Expected::Active(before),
        });
    };
    if matches!(transition, Transition::Cold { .. })
        && (stopped || attempted_mode)
        && driver.admit().is_ok()
    {
        if !idle(driver.phase()) {
            error
                .user_message
                .description
                .push_str(" 核心仍在运行或状态变化中，未再修改网络，请刷新确认。");
            error.user_message.details.push_str("\n已保留现场。");
            return Err(error);
        }
        let recovery = async {
            if attempted_mode {
                // Even a failed save might have taken effect. Restore using
                // the original mode read under the configuration permit.
                driver.step(Step::SelectMode(before)).await?;
            }
            if stopped {
                driver.admit()?;
                driver.step(Step::StartCore).await?;
            }
            Ok::<_, AppErrorDto>(())
        }
        .await;
        match recovery {
            Ok(()) => error.user_message.description.push_str(if stopped {
                " 已恢复之前的网络模式与运行状态。"
            } else {
                " 已恢复之前的网络模式，代理保持停止。"
            }),
            Err(restore) => {
                error
                    .user_message
                    .description
                    .push_str(" 恢复原模式时遇到问题，请刷新状态并查看详情。");
                error
                    .user_message
                    .details
                    .push_str(&format!("\n原模式恢复：{}", restore.message));
            }
        }
    }
    Err(error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_observations_are_dropped_during_and_after_mutation() {
        let s = Service::default();
        let rev = s.state().serial;
        let start = s.begin("starting");
        assert!(s.accept_read(rev, Snapshot::default()).is_none());
        assert!(s.accept_read(start.serial, Snapshot::default()).is_none());
        let end = s.finish(Snapshot::default(), "starting", &Ok(()));
        assert!(s.accept_read(rev, Snapshot::default()).is_none());
        let observed = s.accept_read(end.serial, Snapshot::default()).unwrap();
        assert!(observed.serial > end.serial && observed.completed == 1 && observed.busy.is_none());
        assert_eq!(start.action_serial, start.serial);
        assert_eq!(end.action_serial, end.serial);
        assert_eq!(observed.action_serial, end.action_serial);
    }

    #[test]
    fn a_second_surface_gets_busy_instead_of_queueing_another_write() {
        let s = Service::default();
        let first = s.actions.try_lock().unwrap();
        assert!(s.actions.try_lock().is_err());
        drop(first);
        assert!(s.actions.try_lock().is_ok());
    }

    #[test]
    fn start_does_not_restart_a_running_core_and_tun_changes_are_cold() {
        use NetworkMode::{Manual, SystemProxy, Tun};
        use RuntimePhase::{Running, Stopped};
        assert_eq!(
            plan(
                Request::Start { mode: Some(Tun) },
                SystemProxy,
                Running,
                true
            )
            .unwrap()
            .1,
            Transition::NoChange
        );
        assert_eq!(
            plan(Request::Switch { mode: Tun }, SystemProxy, Running, true)
                .unwrap()
                .1,
            Transition::Cold {
                stop_first: true,
                start: true
            }
        );
        assert_eq!(
            plan(
                Request::Switch { mode: SystemProxy },
                SystemProxy,
                Running,
                false
            )
            .unwrap()
            .1,
            Transition::Hot
        );
        assert_eq!(
            plan(Request::Switch { mode: Manual }, Tun, Stopped, false)
                .unwrap()
                .1,
            Transition::Cold {
                stop_first: false,
                start: false
            }
        );
        assert!(plan(
            Request::Start { mode: None },
            Manual,
            RuntimePhase::Stopping,
            false
        )
        .is_err());
    }

    #[test]
    fn native_transport_success_is_not_a_successful_takeover_without_readback() {
        let unknown = Snapshot::default();
        assert!(Expected::Active(NetworkMode::SystemProxy)
            .verify(&unknown)
            .is_err());
        assert!(Expected::Active(NetworkMode::Tun).verify(&unknown).is_err());
        assert!(Expected::Closed.verify(&unknown).is_err());
        let local = Snapshot {
            network: Presentation::new("local_only", "本地代理运行中", ""),
            ..unknown
        };
        assert!(Expected::Active(NetworkMode::SystemProxy)
            .verify(&local)
            .is_err());
        assert!(Expected::Active(NetworkMode::Manual).verify(&local).is_ok());
    }

    use NetworkMode::{Manual, SystemProxy, Tun};
    use RuntimePhase::{Crashed, Running, Stopped};
    use Step::{PreflightTun, SelectMode, StartCore, StopPrevious};

    struct Script {
        phase: RuntimePhase,
        mode: NetworkMode,
        calls: Vec<Step>,
        // A write may fail after it changes mode/phase, e.g. lost readback.
        faults: Vec<(usize, RuntimePhase, &'static str)>,
    }

    impl Script {
        fn new(mode: NetworkMode, phase: RuntimePhase) -> Self {
            Self {
                mode,
                phase,
                calls: vec![],
                faults: vec![],
            }
        }

        async fn switch(&mut self, mode: NetworkMode) -> Result<Expected, AppErrorDto> {
            let before = self.mode;
            let (_, transition) = plan(
                Request::Switch { mode },
                before,
                self.phase,
                before == SystemProxy && self.phase == Running,
            )?;
            transact(self, before, mode, transition).await
        }
    }

    impl NetworkDriver for Script {
        fn admit(&self) -> Result<(), AppErrorDto> {
            Ok(())
        }
        fn phase(&self) -> RuntimePhase {
            self.phase
        }

        async fn step(&mut self, step: Step) -> Result<(), AppErrorDto> {
            self.calls.push(step);
            if let Step::SelectMode(mode) = step {
                self.mode = mode;
            }
            if let Some((_, phase, error)) =
                self.faults.iter().find(|(i, _, _)| *i == self.calls.len())
            {
                self.phase = *phase;
                return Err(AppError::Runtime((*error).into()).dto());
            }
            self.phase = match step {
                StopPrevious => Stopped,
                StartCore => Running,
                _ => self.phase,
            };
            Ok(())
        }
    }

    #[tokio::test]
    async fn tun_preflight_precedes_every_mutation_on_all_supported_boundaries() {
        for rejection in [
            "invalid installation",
            "helper needs repair",
            "permissions required",
            "unsupported",
            "preflight timed out",
        ] {
            let mut d = Script::new(SystemProxy, Running);
            d.faults = vec![(1, Running, rejection)];
            assert!(d.switch(Tun).await.unwrap_err().message.contains(rejection));
            assert_eq!(d.calls, [PreflightTun]);
            assert_eq!((d.mode, d.phase), (SystemProxy, Running));
        }
        let mut d = Script::new(SystemProxy, Running);
        assert_eq!(d.switch(Tun).await.unwrap(), Expected::Active(Tun));
        assert_eq!(
            d.calls,
            [PreflightTun, StopPrevious, SelectMode(Tun), StartCore]
        );
        assert_eq!((d.mode, d.phase), (Tun, Running));
        d.calls.clear();
        assert_eq!(d.switch(Tun).await.unwrap(), Expected::Active(Tun));
        assert!(d.calls.is_empty());
    }

    #[tokio::test]
    async fn failed_mode_save_or_start_restores_the_original_running_mode_once() {
        for (failed_step, detail) in [(3, "save interrupted"), (4, "core crashed")] {
            let mut d = Script::new(SystemProxy, Running);
            d.faults = vec![(failed_step, Crashed, detail)];
            let error = d.switch(Tun).await.unwrap_err();
            assert_eq!((d.mode, d.phase), (SystemProxy, Running));
            let mut expected = vec![PreflightTun, StopPrevious, SelectMode(Tun)];
            if failed_step == 4 {
                expected.push(StartCore);
            }
            expected.extend([SelectMode(SystemProxy), StartCore]);
            assert_eq!(d.calls, expected);
            assert!(error.user_message.details.contains(detail));
            assert!(error
                .user_message
                .description
                .contains("已恢复之前的网络模式与运行状态"));
        }
    }

    #[tokio::test]
    async fn rollback_failure_keeps_both_diagnostics_and_never_claims_recovery() {
        for (failed_step, detail) in [(5, "rollback save failed"), (6, "rollback start failed")] {
            let mut d = Script::new(SystemProxy, Running);
            d.faults = vec![(4, Crashed, "TUN rejected"), (failed_step, Crashed, detail)];
            let error = d.switch(Tun).await.unwrap_err();
            assert_eq!(d.calls.len(), failed_step);
            assert!(error.user_message.details.contains("TUN rejected"));
            assert!(error.user_message.details.contains(detail));
            assert!(error
                .user_message
                .description
                .contains("恢复原模式时遇到问题"));
            assert!(!error.user_message.description.contains("已恢复之前"));
        }
    }

    #[tokio::test]
    async fn active_transitions_and_uncertain_stop_never_send_a_second_start() {
        for phase in [
            Running,
            RuntimePhase::Starting,
            RuntimePhase::Validating,
            RuntimePhase::Stopping,
            RuntimePhase::Recovering,
        ] {
            let mut d = Script::new(SystemProxy, Running);
            d.faults = vec![(4, phase, "start acknowledgement lost")];
            let error = d.switch(Tun).await.unwrap_err();
            assert_eq!((d.mode, d.phase), (Tun, phase));
            assert_eq!(
                d.calls,
                [PreflightTun, StopPrevious, SelectMode(Tun), StartCore]
            );
            assert!(error.user_message.details.contains("已保留现场"));
            if phase != Running {
                d.calls.clear();
                assert!(d.switch(Manual).await.is_err());
                assert!(d.calls.is_empty());
            }
        }
        let mut d = Script::new(SystemProxy, Running);
        d.faults = vec![(2, Running, "stop acknowledgement lost")];
        let error = d.switch(Tun).await.unwrap_err();
        assert_eq!(d.calls, [PreflightTun, StopPrevious]);
        assert_eq!((d.mode, d.phase), (SystemProxy, Running));
        assert!(!error.user_message.description.contains("已恢复"));
    }

    #[tokio::test]
    async fn previously_idle_proxy_recovers_setting_only_and_hot_switch_stays_live() {
        let mut d = Script::new(Manual, Stopped);
        d.faults = vec![(3, Crashed, "spawn failed")];
        let error = d.switch(Tun).await.unwrap_err();
        assert_eq!(
            d.calls,
            [PreflightTun, SelectMode(Tun), StartCore, SelectMode(Manual)]
        );
        assert_eq!((d.mode, d.phase), (Manual, Crashed));
        assert!(error.user_message.description.contains("代理保持停止"));

        let mut d = Script::new(Manual, Running);
        assert_eq!(
            d.switch(SystemProxy).await.unwrap(),
            Expected::Active(SystemProxy)
        );
        assert_eq!(d.calls, [SelectMode(SystemProxy)]);
        assert_eq!(d.phase, Running);
    }

    #[test]
    fn route_enabled_is_distinct_from_verified_diversion() {
        use crate::domain_routing::{ingress::Ingress, Document, Snapshot as RouteSnapshot};
        let profile = Some(Uuid::new_v4());
        let mut route = RouteSnapshot {
            document: Document {
                enabled: true,
                revision: 42,
                ..Document::default()
            },
            presets: vec![],
            targets: vec![],
            profile_id: profile,
            profile_revision: None,
            status: "已启用 · 暂停".into(),
            readiness: "missing_target".into(),
            verification: "waiting_core".into(),
            ingress: Ingress {
                state: IngressState::Stopped,
                message: String::new(),
            },
            warning: String::new(),
            user_rule_count: 0,
        };
        let paused = RouteControl::from_route(&route);
        assert!(paused.enabled && paused.ready_to_toggle && !paused.active);
        assert_eq!(paused.label, "路由 · 待启动");
        assert!(
            !paused.intent.enabled
                && paused.intent.expected_revision == 42
                && paused.intent.profile_id == profile
        );
        route.verification = "verified".into();
        route.ingress.state = IngressState::LocalOnly;
        assert!(!RouteControl::from_route(&route).active);
        route.ingress.state = IngressState::SystemProxy;
        assert!(RouteControl::from_route(&route).active);
        route.verification = "mismatch".into();
        assert!(!RouteControl::from_route(&route).active);
    }

    #[test]
    fn failed_stop_is_not_reclassified_as_off_by_unverified_reads() {
        let s = Service::default();
        let start = s.begin("stopping");
        let outcome: Result<(), _> = Err(AppError::Runtime("test failure".into()).dto());
        let failed = s.finish(Snapshot::default(), "stopping", &outcome);
        let read = s.accept_read(failed.serial, Snapshot::default()).unwrap();
        assert_eq!(read.network.label, "关闭未完成");
        let off = Snapshot {
            network: Presentation::new("closed", "已关闭", ""),
            ..Snapshot::default()
        };
        assert_eq!(
            s.accept_read(read.serial, off).unwrap().network.label,
            "已关闭"
        );
        assert!(failed.serial > start.serial && failed.can_stop);
    }
}
