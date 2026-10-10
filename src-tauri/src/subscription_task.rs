//! Bounded, secret-free subscription task state. Cancellation stops before commit.
use crate::error::{AppError, AppErrorDto, AppResult};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Instant,
};
use tauri::{AppHandle, Emitter};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DownloadPath {
    #[default]
    FollowCore,
    Direct,
    LocalProxy,
}
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DownloadOptions {
    #[serde(default)]
    pub path: DownloadPath,
    pub proxy_port: Option<u16>,
}
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Preparing,
    Connecting,
    Downloading,
    Retrying,
    Checking,
    Waiting,
    Validating,
    Committing,
    Completed,
    Failed,
    Cancelled,
}
impl Phase {
    fn terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
    fn label(self) -> &'static str {
        match self {
            Self::Preparing => "正在准备订阅任务",
            Self::Connecting => "正在连接订阅服务",
            Self::Downloading => "正在下载订阅",
            Self::Retrying => "订阅请求暂未完成，正在有限重试",
            Self::Checking => "订阅已下载，正在检查内容",
            Self::Waiting => "正在等待当前配置操作完成，订阅内容已暂存",
            Self::Validating => "正在执行 Mihomo 配置校验，首次可能需要准备规则资源",
            Self::Committing => "正在保存配置，请等待完成",
            Self::Completed => "订阅任务已完成",
            Self::Failed => "订阅任务未完成",
            Self::Cancelled => "已取消订阅任务，原有配置未改变",
        }
    }
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub id: Uuid,
    pub operation: &'static str,
    pub profile_id: Option<Uuid>,
    pub phase: Phase,
    pub failure_phase: Option<Phase>,
    pub error_kind: Option<&'static str>,
    pub path: &'static str,
    pub sequence: u64,
    pub elapsed_ms: u64,
    pub attempt: u8,
    pub http_status: Option<u16>,
    pub retry_after_seconds: Option<u64>,
    pub bytes: usize,
    pub can_cancel: bool,
    pub message: String,
    pub error_code: Option<&'static str>,
}
#[derive(Debug)]
pub struct Task {
    pub cancel_flag: AtomicBool,
    notify: tokio::sync::Notify,
    started: Instant,
    state: Mutex<Snapshot>,
    app: Option<AppHandle>,
}
impl Task {
    fn new(
        id: Uuid,
        operation: &'static str,
        profile_id: Option<Uuid>,
        app: Option<AppHandle>,
    ) -> Self {
        Self {
            cancel_flag: AtomicBool::new(false),
            notify: tokio::sync::Notify::new(),
            started: Instant::now(),
            app,
            state: Mutex::new(Snapshot {
                id,
                operation,
                profile_id,
                phase: Phase::Preparing,
                failure_phase: None,
                error_kind: None,
                path: "pending",
                sequence: 0,
                elapsed_ms: 0,
                attempt: 0,
                http_status: None,
                retry_after_seconds: None,
                bytes: 0,
                can_cancel: true,
                message: Phase::Preparing.label().into(),
                error_code: None,
            }),
        }
    }
    pub fn snapshot(&self) -> Snapshot {
        let mut value = self.state.lock().unwrap().clone();
        if !value.phase.terminal() {
            value.elapsed_ms = self.started.elapsed().as_millis() as u64;
        }
        value
    }
    fn update(&self, change: impl FnOnce(&mut Snapshot)) {
        let value = {
            let mut state = self.state.lock().unwrap();
            if state.phase.terminal() {
                return;
            }
            change(&mut state);
            state.sequence += 1;
            state.elapsed_ms = self.started.elapsed().as_millis() as u64;
            state.clone()
        };
        // Only enums, counters and generated UUIDs: no hostname/URL/response/error text.
        crate::app_log::record(if value.phase == Phase::Failed { 2 } else if value.phase == Phase::Retrying { 1 } else { 0 }, crate::app_log::Area::Subscription, &format!(
            "subscription task={} operation={} phase={:?} path={} attempt={} status={:?} bytes={} elapsed_ms={} code={:?} failure_phase={:?} cause={:?}",
            value.id, value.operation, value.phase, value.path, value.attempt,
            value.http_status, value.bytes, value.elapsed_ms, value.error_code, value.failure_phase, value.error_kind));
        if let Some(app) = &self.app {
            let _ = app.emit("subscription-task", &value);
        }
    }
    pub fn phase(&self, phase: Phase) {
        self.update(|s| {
            s.phase = phase;
            s.message = phase.label().into();
        });
    }
    pub fn path(&self, path: &'static str) {
        self.update(|s| s.path = path);
    }
    pub fn request(&self) {
        self.update(|s| {
            s.phase = Phase::Connecting;
            s.message = s.phase.label().into();
            s.attempt += 1;
            s.http_status = None;
            s.retry_after_seconds = None;
            s.bytes = 0;
        });
    }
    pub fn response(&self, status: u16) {
        self.update(|s| s.http_status = Some(status));
    }
    pub fn retry_after(&self, seconds: u64) {
        self.update(|s| s.retry_after_seconds = Some(seconds));
    }
    pub fn bytes(&self, bytes: usize) {
        self.update(|s| s.bytes = bytes);
    }
    pub fn cancelled(&self) -> bool {
        self.cancel_flag.load(Ordering::Acquire)
    }
    pub fn check(&self) -> AppResult<()> {
        if self.cancelled() {
            Err(cancel_error())
        } else {
            Ok(())
        }
    }
    pub fn cancel(&self) -> bool {
        let mut state = self.state.lock().unwrap();
        if !state.can_cancel || state.phase.terminal() {
            return false;
        }
        self.cancel_flag.store(true, Ordering::Release);
        state.can_cancel = false;
        state.sequence += 1;
        state.message = "正在取消订阅任务".into();
        self.notify.notify_waiters();
        true
    }
    async fn cancellation(&self) {
        loop {
            let notified = self.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.cancelled() {
                return;
            }
            notified.await;
        }
    }
    pub async fn wait<T>(
        &self,
        future: impl std::future::Future<Output = AppResult<T>>,
    ) -> AppResult<T> {
        self.check()?;
        tokio::select! { biased; _ = self.cancellation() => Err(cancel_error()), result = future => result }
    }
    pub fn begin_commit(&self) -> AppResult<()> {
        // Same lock as cancel: exactly one side wins. Never interrupt a live reload/save transaction.
        {
            let mut s = self.state.lock().unwrap();
            self.check()?;
            s.can_cancel = false;
        }
        self.phase(Phase::Committing);
        Ok(())
    }
    pub fn finish(&self, error: Option<&AppError>) {
        let cancelled = self.cancelled() && error.is_some();
        self.update(|s| {
            let previous = s.phase;
            s.can_cancel = false;
            s.phase = if cancelled { Phase::Cancelled } else if error.is_some() { Phase::Failed } else { Phase::Completed };
            s.message = match error {
                None => s.phase.label().into(),
                Some(_) if cancelled => Phase::Cancelled.label().into(),
                Some(AppError::Subscription(message)) => {
                    let (title, detail, _) = crate::subscription::subscription_user_message(message);
                    format!("{title}。{detail}")
                }
                Some(AppError::Config(_)) => "订阅已下载，但配置校验未通过。请使用服务商的 Clash / Mihomo 格式；原有配置未改变。".into(),
                Some(AppError::Runtime(_)) if previous == Phase::Validating =>
                    "订阅已下载，但 Mihomo 校验未完成。请检查规则资源下载及本地核心；原有配置未改变。".into(),
                Some(AppError::Conflict(_)) => "配置状态正在变化，订阅未提交。请稍后重试；原有配置已保留。".into(),
                Some(AppError::InvalidInput(_)) => "请检查订阅地址与下载路径设置：远程订阅需使用 HTTPS，本地代理端口需有效。".into(),
                Some(AppError::NotFound(_)) => "订阅或本地版本已变化，请刷新列表后重试。".into(),
                Some(_) => "订阅提交未完成，请检查本地存储或核心状态后重试。".into(),
            };
            s.error_code = error.map(AppError::code);
            s.failure_phase = error.map(|_| previous);
            s.error_kind = error.map(failure_kind);
        });
    }
    pub fn error_dto(&self, error: &AppError) -> AppErrorDto {
        self.finish(Some(error));
        let mut dto = error.dto();
        dto.message = self.snapshot().message.clone();
        if let Some(seconds) = self.snapshot().retry_after_seconds {
            dto.message
                .push_str(&format!(" 服务商建议等待 {seconds} 秒后再试。"));
        }
        dto.user_message.title = if self.cancelled() {
            "订阅任务已取消"
        } else {
            "订阅任务未完成"
        }
        .into();
        dto.user_message.description = dto.message.clone();
        dto.user_message.action = "subscriptions".into();
        dto.user_message.details = format!(
            "任务 {}；阶段 {:?}；{}",
            self.snapshot().id,
            self.snapshot().failure_phase,
            dto.message
        );
        if self.cancelled() {
            dto.retryable = false;
        }
        dto
    }
}
fn failure_kind(error: &AppError) -> &'static str {
    match error {
        AppError::Subscription(message) => {
            if message.starts_with("HTTP 401") || message.starts_with("HTTP 403") {
                "denied"
            } else if message.starts_with("HTTP 429") {
                "rate_limited"
            } else if message.starts_with("HTTP 5") {
                "server"
            } else if message.starts_with("HTTP ") {
                "http"
            } else if message.contains("已取消") {
                "cancelled"
            } else if message.contains("规则资源") {
                "validation_resources"
            } else if message.contains("超时") {
                "timeout"
            } else if message.contains("域名解析") {
                "dns"
            } else if message.contains("安全连接") {
                "tls"
            } else if message.contains("连接意外中断") {
                "interrupted"
            } else if message.contains("服务器连接") {
                "connection"
            } else if message.contains("内容")
                || message.contains("格式")
                || message.contains("网页")
            {
                "content"
            } else {
                "transport"
            }
        }
        AppError::Config(_) => "configuration",
        AppError::Conflict(_) => "conflict",
        AppError::InvalidInput(_) => "input",
        AppError::Io(_) | AppError::NotFound(_) => "storage",
        _ => "runtime",
    }
}

pub fn cancel_error() -> AppError {
    AppError::Subscription("订阅任务已取消".into())
}

#[derive(Default)]
pub struct TaskManager {
    tasks: Mutex<HashMap<Uuid, Arc<Task>>>,
}
impl TaskManager {
    pub fn start(
        &self,
        id: Option<Uuid>,
        operation: &'static str,
        profile_id: Option<Uuid>,
        app: Option<AppHandle>,
    ) -> AppResult<Arc<Task>> {
        let id = id.unwrap_or_else(Uuid::new_v4);
        let mut tasks = self.tasks.lock().unwrap();
        if tasks.contains_key(&id) {
            return Err(AppError::Conflict("任务编号已使用，请重新发起".into()));
        }
        if tasks.values().any(|t| {
            let s = t.snapshot();
            !s.phase.terminal() && s.operation == operation && s.profile_id == profile_id
        }) {
            return Err(AppError::Conflict("该订阅已有任务正在运行".into()));
        }
        if tasks.len() >= 32 {
            let oldest = tasks
                .iter()
                .filter(|(_, t)| t.snapshot().phase.terminal())
                .min_by_key(|(_, t)| t.started)
                .map(|(id, _)| *id);
            if let Some(id) = oldest {
                tasks.remove(&id);
            } else {
                return Err(AppError::Conflict("订阅任务较多，请稍后重试".into()));
            }
        }
        let task = Arc::new(Task::new(id, operation, profile_id, app));
        tasks.insert(id, task.clone());
        Ok(task)
    }
    pub fn get(&self, id: Uuid) -> Option<Arc<Task>> {
        self.tasks.lock().unwrap().get(&id).cloned()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn cancellation_interrupts_wait_and_never_crosses_commit() {
        let task = Task::new(Uuid::new_v4(), "import", None, None);
        assert!(task.cancel());
        assert!(task
            .wait(std::future::pending::<AppResult<()>>())
            .await
            .is_err());
        assert!(task.begin_commit().is_err());
        let task = Task::new(Uuid::new_v4(), "import", None, None);
        task.begin_commit().unwrap();
        assert!(!task.cancel());
        task.finish(None);
        let s = task.snapshot();
        assert_eq!(s.phase, Phase::Completed);
        task.phase(Phase::Downloading);
        assert_eq!(task.snapshot().phase, Phase::Completed);
    }
    #[test]
    fn registry_rejects_duplicate_active_tasks_and_is_bounded() {
        let manager = TaskManager::default();
        let task = manager.start(None, "import", None, None).unwrap();
        assert!(manager.start(None, "import", None, None).is_err());
        let id = task.snapshot().id;
        task.finish(None);
        assert!(manager.start(Some(id), "import", None, None).is_err());
        for _ in 0..40 {
            manager
                .start(None, "import", None, None)
                .unwrap()
                .finish(None);
        }
        assert_eq!(manager.tasks.lock().unwrap().len(), 32);
    }
    #[test]
    fn diagnostics_do_not_copy_secrets_from_error() {
        let t = Task::new(Uuid::new_v4(), "refresh", None, None);
        let dto = t.error_dto(&AppError::Config(
            "https://private.invalid/?token=SECRET".into(),
        ));
        assert!(!serde_json::to_string(&t.snapshot())
            .unwrap()
            .contains("SECRET"));
        assert!(!serde_json::to_string(&dto).unwrap().contains("SECRET"));
    }
    #[test]
    fn terminal_elapsed_time_is_frozen_and_failure_is_classified_without_payload() {
        let t = Task::new(Uuid::new_v4(), "import", None, None);
        t.phase(Phase::Connecting);
        t.finish(Some(&AppError::Subscription("订阅域名解析失败".into())));
        let first = t.snapshot();
        std::thread::sleep(std::time::Duration::from_millis(5));
        assert_eq!(first.elapsed_ms, t.snapshot().elapsed_ms);
        assert_eq!(first.failure_phase, Some(Phase::Connecting));
        assert_eq!(first.error_kind, Some("dns"));
    }
}
