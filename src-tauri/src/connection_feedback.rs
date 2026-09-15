//! Local enablement and Internet health are independent. A probe never changes
//! OS settings, stops a core, replays requests or claims a model was verified.
use crate::{
    error::{AppError, AppErrorDto, AppResult, UserMessage},
    models::{AppSettings, NetworkMode},
    network_safety::{self, NetworkSafetyCheck, NetworkSafetyReport},
    runtime::MihomoRuntime,
    session_resume::SessionResumeManager,
    storage::AppStorage,
};
use serde::Serialize;
use std::{sync::Mutex, time::Instant};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::watch;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    #[default]
    Idle,
    Validating,
    Starting,
    Applying,
    Enabled,
    Failed,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Health {
    #[default]
    Unchecked,
    Checking,
    Healthy,
    Partial,
    Unavailable,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub revision: u64,
    pub operation: u64,
    pub phase: Phase,
    pub mode: NetworkMode,
    pub health: Health,
    pub retrying: bool,
    pub elapsed_ms: u64,
    pub issue: Option<UserMessage>,
    pub checks: Vec<NetworkSafetyCheck>,
}
impl Default for Snapshot {
    fn default() -> Self {
        Self {
            revision: 0,
            operation: 0,
            phase: Phase::Idle,
            mode: NetworkMode::Manual,
            health: Health::Unchecked,
            retrying: false,
            elapsed_ms: 0,
            issue: None,
            checks: vec![],
        }
    }
}
#[derive(Clone, PartialEq, Eq)]
struct Context {
    profile: Option<uuid::Uuid>,
    rules_revision: u64,
    profile_identity: [u8; 32],
    revision: Option<uuid::Uuid>,
    mode: NetworkMode,
    mixed: u16,
    controller: u16,
    run: (u32, chrono::DateTime<chrono::Utc>),
}
impl Context {
    fn read(app: &AppHandle) -> AppResult<Self> {
        let storage = AppStorage::from_app(app)?;
        let state = storage.state()?;
        let settings = storage.settings()?;
        use sha2::Digest;
        let profile = state
            .active_profile_id
            .map(|id| storage.load_profile(id))
            .transpose()?;
        let profile_identity = sha2::Sha256::digest(
            serde_json::to_vec(&profile).map_err(|e| AppError::Io(e.to_string()))?,
        )
        .into();
        let rules_revision = storage.user_rules()?.revision;
        let run = app
            .state::<MihomoRuntime>()
            .running_identity()
            .ok_or_else(|| AppError::Runtime("代理服务当前未运行".into()))?;
        Ok(Self {
            profile: state.active_profile_id,
            rules_revision,
            profile_identity,
            revision: state.active_revision_id,
            mode: settings.network_mode,
            mixed: settings.mixed_port,
            controller: settings.controller_port,
            run,
        })
    }
}
#[derive(Default)]
struct State {
    value: Snapshot,
    started: Option<Instant>,
    context: Option<Context>,
}
impl State {
    fn begin(&mut self, mode: NetworkMode, phase: Phase) -> u64 {
        let revision = self.value.revision + 1;
        let operation = self.value.operation + 1;
        self.value = Snapshot {
            revision,
            operation,
            mode,
            phase,
            ..Snapshot::default()
        };
        self.started = Some(Instant::now());
        self.context = None;
        operation
    }
    fn update(&mut self, operation: u64, change: impl FnOnce(&mut Snapshot)) -> bool {
        if operation != self.value.operation {
            return false;
        }
        change(&mut self.value);
        self.value.revision += 1;
        self.value.elapsed_ms = self
            .started
            .map(|t| t.elapsed().as_millis() as u64)
            .unwrap_or(0);
        true
    }
    fn snapshot(&self) -> Snapshot {
        let mut result = self.value.clone();
        if matches!(
            result.phase,
            Phase::Validating | Phase::Starting | Phase::Applying
        ) || result.health == Health::Checking
        {
            result.elapsed_ms = self
                .started
                .map(|t| t.elapsed().as_millis() as u64)
                .unwrap_or(0);
        }
        result
    }
}
pub struct ConnectionFeedback {
    inner: Mutex<State>,
    changed: watch::Sender<u64>,
}
impl Default for ConnectionFeedback {
    fn default() -> Self {
        let (changed, _) = watch::channel(0);
        Self {
            inner: Mutex::new(State::default()),
            changed,
        }
    }
}
impl ConnectionFeedback {
    pub fn snapshot(&self) -> Snapshot {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .snapshot()
    }
    fn emit(&self, app: &AppHandle) {
        let _ = app.emit("connection-feedback", self.snapshot());
    }
    pub fn begin(&self, app: &AppHandle, mode: NetworkMode, phase: Phase) -> u64 {
        let operation = {
            let mut state = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            let operation = state.begin(mode, phase);
            self.changed.send_replace(operation);
            operation
        };
        self.emit(app);
        operation
    }
    pub fn advance(&self, app: &AppHandle, operation: u64, phase: Phase) {
        if self
            .inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .update(operation, |s| s.phase = phase)
        {
            crate::app_log::record(
                0,
                crate::app_log::Area::Runtime,
                &format!(
                    "启动阶段：{phase:?}；累计 {} ms",
                    self.snapshot().elapsed_ms
                ),
            );
            self.emit(app);
        }
    }
    pub fn starting(&self, app: &AppHandle) {
        let id = self.snapshot().operation;
        self.advance(app, id, Phase::Starting);
    }
    pub fn fail(&self, app: &AppHandle, operation: u64, error: &AppErrorDto) {
        if self
            .inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .update(operation, |s| {
                s.phase = Phase::Failed;
                s.health = Health::Unchecked;
                s.issue = Some((*error.user_message).clone());
            })
        {
            self.emit(app);
        }
    }
    pub fn invalidate(&self, app: &AppHandle) {
        let mode = self.snapshot().mode;
        self.begin(app, mode, Phase::Idle);
    }
    fn invalidate_if(&self, app: &AppHandle, operation: u64) {
        {
            let mut state = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            if state.value.operation != operation {
                return;
            }
            let mode = state.value.mode;
            let next = state.begin(mode, Phase::Idle);
            self.changed.send_replace(next);
        }
        self.emit(app);
    }
    pub fn inspect_current(&self, app: &AppHandle) -> Snapshot {
        let (operation, previous) = {
            let state = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            (state.value.operation, state.context.clone())
        };
        if previous.is_some_and(|context| {
            Context::read(app).ok().as_ref() != Some(&context)
                || (context.mode == NetworkMode::SystemProxy
                    && !crate::platform::status(app).active)
        }) {
            self.invalidate_if(app, operation);
        }
        self.snapshot()
    }
    pub fn probe(&self, app: &AppHandle, operation: u64, settings: AppSettings) {
        if app.state::<SessionResumeManager>().is_shutting_down() {
            return;
        }
        let Ok(context) = Context::read(app) else {
            return;
        };
        let mut receiver = self.changed.subscribe();
        {
            let mut state = self.inner.lock().unwrap_or_else(|e| e.into_inner());
            if !state.update(operation, |s| {
                s.phase = Phase::Enabled;
                s.health = Health::Checking;
            }) {
                return;
            }
            state.context = Some(context.clone());
        }
        self.emit(app);
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            let manager = app.state::<ConnectionFeedback>();
            let work = async {
                let mut result = network_safety::inspect_local_proxy(&settings).await;
                if result.as_ref().is_ok_and(network_safety::should_retry) {
                    manager
                        .inner
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .update(operation, |s| s.retrying = true);
                    manager.emit(&app);
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    result = network_safety::inspect_local_proxy(&settings).await;
                }
                result
            };
            let result = tokio::select! { biased; _ = receiver.changed() => return, result = work => result };
            if app.state::<SessionResumeManager>().is_shutting_down() {
                return;
            }
            // Serialize attribution/readback with config mutations. A busy or
            // changed configuration invalidates this result, never stops a core.
            let waiting = Instant::now();
            let _permit = loop {
                if let Ok(permit) = crate::user_rules::acquire_configuration(&app) {
                    break permit;
                }
                if waiting.elapsed() >= std::time::Duration::from_secs(2) {
                    manager.invalidate_if(&app, operation);
                    return;
                }
                tokio::select! { biased;
                    _ = receiver.changed() => return,
                    _ = tokio::time::sleep(std::time::Duration::from_millis(25)) => {}
                }
            };
            if Context::read(&app).ok().as_ref() != Some(&context) {
                manager.invalidate_if(&app, operation);
                return;
            }
            let changed = manager
                .inner
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .update(operation, |s| {
                    s.retrying = false;
                    match result {
                        Ok(report) => {
                            s.health = classify(&report);
                            s.checks = report.checks;
                        }
                        Err(error) => {
                            s.health = Health::Unavailable;
                            s.issue = Some(*error.dto().user_message);
                        }
                    }
                });
            if changed {
                manager.emit(&app);
            }
        });
    }
}
fn classify(report: &NetworkSafetyReport) -> Health {
    if report.checks.is_empty() {
        Health::Unchecked
    } else if report.checks.iter().all(|check| check.success) {
        Health::Healthy
    } else if report.checks.iter().any(|check| check.success) {
        Health::Partial
    } else {
        Health::Unavailable
    }
}
#[tauri::command]
pub fn connection_feedback(app: AppHandle) -> Snapshot {
    app.state::<ConnectionFeedback>().inspect_current(&app)
}
#[tauri::command]
pub async fn recheck_connection(app: AppHandle) -> Result<Snapshot, AppErrorDto> {
    let _permit = crate::user_rules::acquire_configuration(&app).map_err(|e| e.dto())?;
    let storage = AppStorage::from_app(&app).map_err(|e| e.dto())?;
    let settings = storage.settings().map_err(|e| e.dto())?;
    Context::read(&app).map_err(|e| e.dto())?;
    if settings.network_mode == NetworkMode::Tun {
        return Err(AppError::InvalidInput("TUN 请使用诊断页执行完整检测".into()).dto());
    }
    let manager = app.state::<ConnectionFeedback>();
    let operation = manager.begin(&app, settings.network_mode, Phase::Enabled);
    manager.probe(&app, operation, settings);
    Ok(manager.snapshot())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn late_probe_never_overwrites_stop_or_a_new_operation() {
        let mut state = State::default();
        let first = state.begin(NetworkMode::SystemProxy, Phase::Enabled);
        state.update(first, |s| s.health = Health::Checking);
        let stopped = state.begin(NetworkMode::SystemProxy, Phase::Idle);
        assert!(!state.update(first, |s| s.health = Health::Healthy));
        assert_eq!(state.value.operation, stopped);
        assert_eq!(state.value.health, Health::Unchecked);
    }
    #[test]
    fn a_network_failure_does_not_revert_local_enablement() {
        let mut state = State::default();
        let id = state.begin(NetworkMode::SystemProxy, Phase::Enabled);
        state.update(id, |s| s.health = Health::Unavailable);
        assert_eq!(state.value.phase, Phase::Enabled);
        assert!(state.value.issue.is_none());
    }
    #[test]
    fn progress_revisions_are_monotonic_within_an_operation() {
        let mut state = State::default();
        let id = state.begin(NetworkMode::Manual, Phase::Validating);
        let old = state.snapshot().revision;
        state.update(id, |s| s.phase = Phase::Starting);
        assert!(state.snapshot().revision > old);
        assert_eq!(state.snapshot().operation, id);
    }
}
