//! Startup restoration is a cancellable convenience, not permission to change
//! login registration, elevate Windows, reattach Codex, or launch other apps.
use crate::error::{AppError, AppResult};
use crate::models::{AppSettings, NetworkMode, PersistentAppState, RuntimePhase};
use crate::storage::AppStorage;
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::Notify;

#[derive(Debug, Clone, Copy, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ResumePhase {
    #[default]
    Idle,
    Pending,
    Restoring,
    WaitingNetwork,
    Restored,
    Paused,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionResumeStatus {
    pub phase: ResumePhase,
    pub message: String,
}

#[derive(Default)]
struct ResumeState {
    generation: u64,
    status: SessionResumeStatus,
}

#[derive(Default)]
pub struct SessionResumeManager {
    state: Mutex<ResumeState>,
    changed: Notify,
    shutting_down: AtomicBool,
    // Serialize the short process/proxy transitions against final cleanup. A
    // shutdown must not stop first and then let an in-flight start spawn again.
    runtime_transition: Mutex<()>,
}

#[derive(Clone)]
struct ResumePlan {
    generation: u64,
    settings: AppSettings,
    persistent: PersistentAppState,
}

impl ResumePlan {
    fn matches(&self, settings: &AppSettings, state: &PersistentAppState) -> bool {
        settings.restore_last_session
            && state.desired_running
            && self.persistent.active_profile_id == state.active_profile_id
            && self.persistent.active_revision_id == state.active_revision_id
            && self.settings.network_mode == settings.network_mode
            && self.settings.mixed_port == settings.mixed_port
            && self.settings.controller_port == settings.controller_port
    }
}

impl SessionResumeManager {
    pub fn snapshot(&self) -> SessionResumeStatus {
        self.state
            .lock()
            .map(|state| state.status.clone())
            .unwrap_or(SessionResumeStatus {
                phase: ResumePhase::Paused,
                message: "无法读取启动恢复状态，请手动启动。".into(),
            })
    }

    fn emit(&self, app: &AppHandle) {
        crate::app_log::record(0, crate::app_log::Area::Restore, &self.snapshot().message);
        let _ = app.emit("session-resume-status", self.snapshot());
    }

    fn prepare(
        &self,
        settings: &AppSettings,
        persistent: &PersistentAppState,
    ) -> Option<ResumePlan> {
        let mut state = self.state.lock().ok()?;
        if self.shutting_down.load(Ordering::Acquire) {
            return None;
        }
        state.generation = state.generation.wrapping_add(1);
        let (phase, message) = if !settings.restore_last_session {
            (ResumePhase::Idle, "自动恢复已关闭，应用打开后保持停止。")
        } else if !persistent.desired_running {
            (ResumePhase::Idle, "未记录需要恢复的运行状态。")
        } else if persistent.active_profile_id.is_none() || persistent.active_revision_id.is_none()
        {
            (
                ResumePhase::Paused,
                "上次配置不可用，请先选用配置再手动启动。",
            )
        } else {
            (ResumePhase::Pending, "正在准备恢复上次的代理设置。")
        };
        state.status = SessionResumeStatus {
            phase,
            message: message.into(),
        };
        (phase == ResumePhase::Pending).then(|| ResumePlan {
            generation: state.generation,
            settings: settings.clone(),
            persistent: persistent.clone(),
        })
    }

    fn current(&self, generation: u64) -> bool {
        !self.shutting_down.load(Ordering::Acquire)
            && self
                .state
                .lock()
                .is_ok_and(|state| state.generation == generation)
    }

    fn update(&self, app: &AppHandle, generation: u64, phase: ResumePhase, message: String) {
        if let Ok(mut state) = self.state.lock() {
            if state.generation != generation || self.shutting_down.load(Ordering::Acquire) {
                return;
            }
            state.status = SessionResumeStatus { phase, message };
        }
        self.emit(app);
    }

    fn cancel(&self) -> bool {
        let mut was_restoring = false;
        if let Ok(mut state) = self.state.lock() {
            // Pending includes the tiny preflight window that already owns the
            // configuration permit but has not published Restoring yet.
            was_restoring = matches!(
                state.status.phase,
                ResumePhase::Pending | ResumePhase::Restoring | ResumePhase::WaitingNetwork
            );
            state.generation = state.generation.wrapping_add(1);
            state.status = SessionResumeStatus {
                phase: ResumePhase::Idle,
                message: "当前运行状态由手动操作控制。".into(),
            };
        }
        self.changed.notify_waiters();
        was_restoring
    }

    pub fn cancel_pending(&self, app: &AppHandle) -> bool {
        let result = self.cancel();
        self.emit(app);
        result
    }

    async fn cancelled(&self, generation: u64) {
        loop {
            let notification = self.changed.notified();
            tokio::pin!(notification);
            // Register before reading generation, including cancellation between
            // this read and await; a missed notification must not enable proxy.
            notification.as_mut().enable();
            if !self.current(generation) {
                return;
            }
            notification.await;
        }
    }

    pub fn while_open<T>(&self, operation: impl FnOnce() -> AppResult<T>) -> AppResult<T> {
        let _guard = self
            .runtime_transition
            .lock()
            .map_err(|_| AppError::Runtime("运行状态锁不可用".into()))?;
        if self.shutting_down.load(Ordering::Acquire) {
            return Err(AppError::Conflict(
                "应用正在退出，未执行启动或代理切换".into(),
            ));
        }
        operation()
    }

    pub fn is_shutting_down(&self) -> bool {
        self.shutting_down.load(Ordering::Acquire)
    }

    pub fn begin_shutdown(&self) -> bool {
        if self.shutting_down.swap(true, Ordering::AcqRel) {
            return false;
        }
        self.cancel();
        true
    }

    pub fn cleanup_transition(&self, operation: impl FnOnce()) {
        // Even a poisoned lock must not prevent best-effort shutdown cleanup.
        let _guard = self
            .runtime_transition
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        operation();
    }

    pub fn bootstrap(
        &self,
        app: &AppHandle,
        settings: &AppSettings,
        persistent: &PersistentAppState,
    ) {
        let plan = self.prepare(settings, persistent);
        self.emit(app);
        if let Some(plan) = plan {
            let app = app.clone();
            tauri::async_runtime::spawn(async move { restore(app, plan).await });
        }
    }
}

/// Manual actions invalidate queued work before taking the shared permit. When
/// restoration already owns it, cancellation interrupts awaited network checks
/// and releases the permit before the manual command starts/stops the core.
pub async fn acquire_manual_configuration(
    app: &AppHandle,
) -> AppResult<crate::user_rules::ConfigurationMutationPermit> {
    let restoring = app.state::<SessionResumeManager>().cancel_pending(app);
    wait_for_manual_configuration(restoring, || crate::user_rules::acquire_configuration(app)).await
}

// Privileged TUN preparation may still include two bounded validation passes.
// Keep enough margin for their cancellation boundary without blocking the UI.
const MANUAL_WAIT_ATTEMPTS: usize = 1_400;

async fn wait_for_manual_configuration<T>(
    restoring: bool,
    mut acquire: impl FnMut() -> AppResult<T>,
) -> AppResult<T> {
    for attempt in 0..MANUAL_WAIT_ATTEMPTS {
        match acquire() {
            Ok(permit) => return Ok(permit),
            Err(_) if restoring && attempt + 1 < MANUAL_WAIT_ATTEMPTS => {
                tokio::time::sleep(Duration::from_millis(50)).await
            }
            Err(error) => return Err(error),
        }
    }
    unreachable!()
}

async fn restore(app: AppHandle, plan: ResumePlan) {
    let manager = app.state::<SessionResumeManager>();
    let mut attempt = 0_usize;
    // A slow login network is not a user Stop. Keep the saved intent and retry
    // at most once per minute after the initial backoff; cancellation wins.
    loop {
        let delay = restore_delay(attempt);
        if attempt > 0 {
            manager.update(
                &app,
                plan.generation,
                ResumePhase::WaitingNetwork,
                format!("网络暂未就绪，{delay} 秒后继续恢复；保留上次模式，点击停止可取消。"),
            );
        }
        tokio::select! {
            biased;
            _ = manager.cancelled(plan.generation) => return,
            _ = tokio::time::sleep(Duration::from_secs(delay)) => {}
        }
        if !restore_attempt(&app, &plan).await {
            return;
        }
        attempt = attempt.saturating_add(1);
    }
}

fn restore_delay(attempt: usize) -> u64 {
    const DELAYS: [u64; 5] = [1, 5, 15, 30, 60];
    DELAYS[attempt.min(DELAYS.len() - 1)]
}

fn retry_restore(error: &crate::error::AppErrorDto) -> bool {
    error.code == "NETWORK_CHECK_FAILED" && error.retryable
}

async fn restore_attempt(app: &AppHandle, plan: &ResumePlan) -> bool {
    let manager = app.state::<SessionResumeManager>();
    let result = restore_preflight(app, plan);
    let (settings, proxy_guard, _configuration) = match result {
        Ok(value) => value,
        Err(error) => {
            manager.update(
                app,
                plan.generation,
                ResumePhase::Paused,
                format!(
                    "自动恢复已暂停：{}。请检查后手动启动。",
                    error.user_message().title
                ),
            );
            return false;
        }
    };
    if !manager.current(plan.generation) {
        return false;
    }
    manager.update(
        app,
        plan.generation,
        ResumePhase::Restoring,
        "正在恢复上次的代理设置。".into(),
    );
    let runtime = app.state::<crate::runtime::MihomoRuntime>();
    let result = tokio::select! {
        biased;
        _ = manager.cancelled(plan.generation) => {
            // This restore owns the configuration permit, so no subsequent
            // manual start can have created the process being cleaned up here.
            manager.cleanup_transition(|| {
                let _ = crate::platform::restore_system_proxy(app);
                let _ = runtime.stop(Some(app));
            });
            return false;
        }
        result = crate::start_runtime_for_settings(app, &runtime, &settings, proxy_guard.as_ref()) => result,
    };
    match result {
        Ok(()) if manager.current(plan.generation) => {
            // Already true; intentionally do not rewrite a possibly newer
            // manual stop intent from a late startup task.
            manager.update(
                app,
                plan.generation,
                ResumePhase::Restored,
                "已恢复上次的代理设置，网络检查单独显示。".into(),
            );
        }
        Ok(()) => manager.cleanup_transition(|| {
            let _ = crate::platform::restore_system_proxy(app);
            let _ = runtime.stop(Some(app));
        }),
        Err(error) if manager.current(plan.generation) && retry_restore(&error) => {
            // finish_runtime_start has already stopped the failed owned core.
            // It has not enabled System Proxy on a failed connectivity report.
            crate::app_log::record(
                1,
                crate::app_log::Area::Restore,
                "临时联网预检失败，保留开启意图并等待网络恢复；未变更运行模式",
            );
            return true;
        }
        Err(error) => manager.update(
            app,
            plan.generation,
            ResumePhase::Paused,
            format!(
                "自动恢复已暂停：{}；未自动降级模式，请检查后手动启动。",
                error.user_message.title
            ),
        ),
    }
    false
}

type ResumePreflight = (
    AppSettings,
    Option<crate::platform::ResumeProxyGuard>,
    crate::user_rules::ConfigurationMutationPermit,
);

fn restore_preflight(app: &AppHandle, plan: &ResumePlan) -> AppResult<ResumePreflight> {
    let configuration = crate::user_rules::acquire_configuration(app)?;
    let storage = AppStorage::from_app(app)?;
    let settings = storage.settings()?;
    if !plan.matches(&settings, &storage.state()?) {
        return Err(AppError::Conflict("启动前设置或配置已变化".into()));
    }
    if app
        .state::<crate::runtime::MihomoRuntime>()
        .status(Some(app))
        .phase
        == RuntimePhase::Running
    {
        return Err(AppError::Conflict("已有运行中的核心，未重复启动".into()));
    }
    if settings.network_mode == NetworkMode::Tun && !crate::tun_service::status().ready() {
        return Err(AppError::Platform(
            "TUN 当前权限未就绪，不会后台提权或改用系统代理".into(),
        ));
    }
    let proxy = (settings.network_mode == NetworkMode::SystemProxy)
        .then(|| crate::platform::prepare_automatic_proxy_resume(app))
        .transpose()?;
    Ok((settings, proxy, configuration))
}

#[tauri::command]
pub fn get_session_resume_status(state: State<'_, SessionResumeManager>) -> SessionResumeStatus {
    state.snapshot()
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn login_retry_waits_for_transient_network_without_a_busy_loop() {
        let transient = AppError::NetworkPreflight {
            message: "fixture".into(),
            retryable: true,
        }
        .dto();
        assert!(retry_restore(&transient));
        assert_eq!(
            (0..7).map(restore_delay).collect::<Vec<_>>(),
            [1, 5, 15, 30, 60, 60, 60]
        );
        assert_eq!(restore_delay(usize::MAX), 60);
        for error in [
            AppError::NetworkPreflight {
                message: "TLS".into(),
                retryable: false,
            },
            AppError::Runtime("binary missing".into()),
            AppError::Platform("not authorized".into()),
            AppError::Conflict("new owner".into()),
        ] {
            assert!(!retry_restore(&error.dto()));
        }
    }

    fn state() -> PersistentAppState {
        PersistentAppState {
            desired_running: true,
            active_profile_id: Some(Uuid::new_v4()),
            active_revision_id: Some(Uuid::new_v4()),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn waiting_for_network_is_cancelled_without_waiting_for_the_next_retry() {
        let manager = SessionResumeManager::default();
        let plan = manager.prepare(&AppSettings::default(), &state()).unwrap();
        manager.state.lock().unwrap().status.phase = ResumePhase::WaitingNetwork;
        assert!(manager.cancel());
        tokio::time::timeout(
            Duration::from_millis(100),
            manager.cancelled(plan.generation),
        )
        .await
        .expect("manual Stop interrupts the waiting generation");
        assert!(!manager.current(plan.generation));
        assert_eq!(manager.snapshot().phase, ResumePhase::Idle);
    }

    #[test]
    fn stopped_and_legacy_state_never_queue_start() {
        let manager = SessionResumeManager::default();
        let legacy: PersistentAppState = serde_json::from_str(r#"{"schemaVersion":4,"activeProfileId":null,"activeRevisionId":null,"systemProxySnapshotPresent":false,"cleanShutdown":false,"updatedAt":null}"#).unwrap();
        assert!(!legacy.desired_running);
        assert!(manager.prepare(&AppSettings::default(), &legacy).is_none());
        assert_eq!(manager.snapshot().phase, ResumePhase::Idle);
        let stopped = PersistentAppState {
            desired_running: false,
            ..state()
        };
        assert!(manager.prepare(&AppSettings::default(), &stopped).is_none());
    }

    #[test]
    fn explicit_setting_controls_restore_not_login_registration() {
        let manager = SessionResumeManager::default();
        let settings = AppSettings::default();
        assert!(!settings.launch_at_login);
        assert!(settings.restore_last_session);
        let plan = manager.prepare(&settings, &state()).unwrap();
        assert!(!plan.settings.launch_at_login);
        let disabled = AppSettings {
            restore_last_session: false,
            ..settings
        };
        assert!(manager.prepare(&disabled, &state()).is_none());
    }

    #[test]
    fn preserves_each_mode_and_pauses_when_inputs_change() {
        for mode in [
            NetworkMode::Manual,
            NetworkMode::SystemProxy,
            NetworkMode::Tun,
        ] {
            let settings = AppSettings {
                network_mode: mode,
                ..Default::default()
            };
            let persistent = state();
            let manager = SessionResumeManager::default();
            let plan = manager.prepare(&settings, &persistent).unwrap();
            assert_eq!(plan.settings.network_mode, mode);
            assert!(plan.matches(&settings, &persistent));
            let stopped = PersistentAppState {
                desired_running: false,
                ..persistent.clone()
            };
            assert!(!plan.matches(&settings, &stopped));
            let new_profile = PersistentAppState {
                active_profile_id: Some(Uuid::new_v4()),
                ..persistent.clone()
            };
            assert!(!plan.matches(&settings, &new_profile));
            let changed = AppSettings {
                mixed_port: settings.mixed_port + 1,
                ..settings.clone()
            };
            assert!(!plan.matches(&changed, &persistent));
        }
    }

    #[tokio::test]
    async fn manual_action_cancels_queued_and_running_generation() {
        let manager = SessionResumeManager::default();
        let plan = manager.prepare(&AppSettings::default(), &state()).unwrap();
        assert!(manager.current(plan.generation));
        manager.cancel();
        assert!(!manager.current(plan.generation));
        tokio::time::timeout(
            Duration::from_millis(50),
            manager.cancelled(plan.generation),
        )
        .await
        .unwrap();
        assert_eq!(manager.snapshot().phase, ResumePhase::Idle);
    }

    #[tokio::test]
    async fn pending_preflight_permit_is_waited_out_before_manual_stop() {
        let manager = SessionResumeManager::default();
        let plan = manager.prepare(&AppSettings::default(), &state()).unwrap();
        // Preflight acquired its permit while the published phase is Pending.
        let should_wait = manager.cancel();
        assert!(should_wait);
        let mut attempts = 0;
        let acquired = wait_for_manual_configuration(should_wait, || {
            attempts += 1;
            if attempts < 3 {
                Err(AppError::Conflict(
                    "synthetic preflight still owns permit".into(),
                ))
            } else {
                Ok("manual stop permit")
            }
        })
        .await
        .unwrap();
        assert_eq!(acquired, "manual stop permit");
        assert_eq!(attempts, 3);
        assert!(!manager.current(plan.generation));
    }

    #[tokio::test]
    async fn unrelated_configuration_conflicts_do_not_wait_or_claim_stop_succeeded() {
        let mut attempts = 0;
        let result: AppResult<()> = wait_for_manual_configuration(false, || {
            attempts += 1;
            Err(AppError::Conflict("synthetic profile save".into()))
        })
        .await;
        assert!(result.is_err());
        assert_eq!(attempts, 1);
    }

    #[test]
    fn concurrent_shutdown_cannot_overwrite_an_accepted_stop_intent() {
        use std::sync::{mpsc, Arc};
        let temporary = tempfile::tempdir().unwrap();
        let storage = AppStorage::from_root(temporary.path().join("state")).unwrap();
        storage.set_desired_running(true).unwrap();
        let manager = Arc::new(SessionResumeManager::default());
        let (stop_started, started) = mpsc::channel();
        let (release_stop, stop_released) = mpsc::channel();
        let stop_manager = manager.clone();
        let stop_storage = storage.clone();
        let stopping = std::thread::spawn(move || {
            stop_manager
                .while_open(|| {
                    stop_storage.set_desired_running(false)?;
                    stop_started.send(()).unwrap();
                    stop_released.recv().unwrap();
                    stop_storage.mark_clean_shutdown(true)
                })
                .unwrap();
        });
        started.recv().unwrap();
        let (cleanup_started, cleaning) = mpsc::channel();
        let cleanup_manager = manager.clone();
        let cleanup_storage = storage.clone();
        let shutting_down = std::thread::spawn(move || {
            assert!(cleanup_manager.begin_shutdown());
            cleanup_started.send(()).unwrap();
            cleanup_manager
                .cleanup_transition(|| cleanup_storage.mark_clean_shutdown(true).unwrap());
        });
        cleaning.recv().unwrap();
        release_stop.send(()).unwrap();
        stopping.join().unwrap();
        shutting_down.join().unwrap();
        let saved = storage.state().unwrap();
        assert!(!saved.desired_running);
        assert!(saved.clean_shutdown);
    }

    #[test]
    fn cleanup_is_idempotent_and_prevents_late_starts() {
        let manager = SessionResumeManager::default();
        let persistent = state();
        let plan = manager
            .prepare(&AppSettings::default(), &persistent)
            .unwrap();
        assert!(manager.begin_shutdown());
        assert!(!manager.begin_shutdown());
        assert!(!manager.current(plan.generation));
        assert!(manager.while_open(|| Ok(())).is_err());
        assert!(manager
            .prepare(&AppSettings::default(), &persistent)
            .is_none());
        assert!(persistent.desired_running);
    }

    #[test]
    fn missing_configuration_pauses_without_guessing_a_subscription() {
        let manager = SessionResumeManager::default();
        let missing = PersistentAppState {
            active_revision_id: None,
            ..state()
        };
        assert!(manager.prepare(&AppSettings::default(), &missing).is_none());
        assert_eq!(manager.snapshot().phase, ResumePhase::Paused);
    }
}
