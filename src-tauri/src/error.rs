use serde::Serialize;
use thiserror::Error;

pub type AppResult<T> = Result<T, AppError>;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("{detail}")]
    Startup {
        reason: StartupReason,
        detail: String,
        restored: Option<bool>,
    },
    #[error("输入错误: {0}")]
    InvalidInput(String),
    #[error("资源不存在: {0}")]
    NotFound(String),
    #[error("状态冲突: {0}")]
    Conflict(String),
    #[error("文件操作失败: {0}")]
    Io(String),
    #[error("订阅请求失败: {0}")]
    Subscription(String),
    #[error("应用更新失败: {0}")]
    Update(String),
    #[error("配置处理失败: {0}")]
    Config(String),
    #[error("Mihomo 运行失败: {0}")]
    Runtime(String),
    #[error("网络预检失败: {message}")]
    NetworkPreflight { message: String, retryable: bool },
    #[error("系统网络设置失败: {0}")]
    Platform(String),
}

impl AppError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Startup { reason, .. } => match reason {
                StartupReason::PortInUse | StartupReason::CoreNotReady => "CORE_ERROR",
                StartupReason::ProxyChanged => "STATE_CONFLICT",
                StartupReason::ProxyApply => "PLATFORM_ERROR",
            },
            Self::InvalidInput(_) => "INVALID_INPUT",
            Self::NotFound(_) => "NOT_FOUND",
            Self::Conflict(_) => "STATE_CONFLICT",
            Self::Io(_) => "IO_ERROR",
            Self::Subscription(_) => "SUBSCRIPTION_ERROR",
            Self::Update(_) => "UPDATE_ERROR",
            Self::Config(_) => "CONFIG_ERROR",
            Self::Runtime(_) => "CORE_ERROR",
            Self::NetworkPreflight { .. } => "NETWORK_CHECK_FAILED",
            Self::Platform(_) => "PLATFORM_ERROR",
        }
    }

    pub fn stage(&self) -> &'static str {
        match self {
            Self::Startup { reason, .. } => match reason {
                StartupReason::PortInUse | StartupReason::CoreNotReady => "runtime",
                StartupReason::ProxyChanged | StartupReason::ProxyApply => "platform",
            },
            Self::InvalidInput(_) => "input",
            Self::NotFound(_) | Self::Io(_) => "storage",
            Self::Conflict(_) => "state",
            Self::Subscription(_) => "subscription",
            Self::Update(_) => "update",
            Self::Config(_) => "config",
            Self::Runtime(_) => "runtime",
            Self::NetworkPreflight { .. } => "network",
            Self::Platform(_) => "platform",
        }
    }

    pub fn retryable(&self) -> bool {
        if let Self::NetworkPreflight { retryable, .. } = self {
            return *retryable;
        }
        matches!(
            self,
            Self::Subscription(_) | Self::Update(_) | Self::Runtime(_) | Self::Platform(_)
        )
    }

    pub fn dto(&self) -> AppErrorDto {
        AppErrorDto {
            code: self.code().to_string(),
            stage: self.stage().to_string(),
            message: crate::app_log::sanitize(&self.to_string()),
            user_message: Box::new(self.user_message()),
            retryable: self.retryable(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppErrorDto {
    pub code: String,
    pub stage: String,
    pub message: String,
    pub retryable: bool,
    pub user_message: Box<UserMessage>,
}

impl From<std::io::Error> for AppError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value.to_string())
    }
}

#[derive(Debug, Clone, Copy)]
pub enum StartupReason {
    PortInUse,
    CoreNotReady,
    ProxyChanged,
    ProxyApply,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UserMessage {
    pub title: String,
    pub description: String,
    pub action: String,
    pub details: String,
}
impl AppError {
    pub fn startup(reason: StartupReason, error: impl ToString, restored: Option<bool>) -> Self {
        Self::Startup {
            reason,
            detail: error.to_string(),
            restored,
        }
    }
    pub fn user_message(&self) -> UserMessage {
        let (title, description, action) = match self {
            Self::Startup {
                reason: StartupReason::PortInUse,
                ..
            } => (
                "代理端口正在被使用",
                "请关闭占用该端口的程序，或在设置中更换端口。",
                "settings",
            ),
            Self::Startup {
                reason: StartupReason::CoreNotReady,
                ..
            } => (
                "代理服务尚未就绪",
                "本次未完成开启，请重试或查看详情。",
                "retry",
            ),
            Self::Startup {
                reason: StartupReason::ProxyChanged,
                ..
            } => (
                "当前代理设置需要确认",
                "检测到其他代理设置或操作期间设置发生变化，本次操作已暂停。",
                "details",
            ),
            Self::Startup {
                reason: StartupReason::ProxyApply,
                restored,
                ..
            } => (
                "系统代理设置未完成",
                match restored {
                    Some(true) => "已恢复操作前的代理设置。",
                    Some(false) => "恢复原设置时也遇到问题，请查看系统代理设置。",
                    None => "请检查系统权限和当前网络设置。",
                },
                "settings",
            ),
            Self::InvalidInput(_) => (
                "请检查填写的内容",
                "输入内容需要调整，详情中可查看具体原因。",
                "details",
            ),
            Self::NotFound(_) => ("配置或所需文件未找到", "请检查当前选用的配置。", "profiles"),
            Self::Conflict(_) => (
                "当前操作暂未完成",
                "配置或运行状态可能正在变化，请刷新后重试。",
                "refresh",
            ),
            Self::Io(_) => (
                "本地数据读写未完成",
                "请检查磁盘空间及文件访问权限。",
                "details",
            ),
            Self::Subscription(_) => (
                "订阅更新未完成",
                "请检查网络和订阅地址后重试。",
                "subscriptions",
            ),
            Self::Update(_) => (
                "软件更新未完成",
                "请稍后重试，详情中可查看具体原因。",
                "details",
            ),
            Self::Config(_) => (
                "代理配置需要检查",
                "请查看配置中的问题，再重新开启代理。",
                "profiles",
            ),
            Self::Runtime(_) => ("代理服务操作未完成", "请重试或查看详情。", "retry"),
            Self::NetworkPreflight { .. } => (
                "连接检查暂未通过",
                "检测请求未收到预期响应，请查看诊断结果。",
                "diagnostics",
            ),
            Self::Platform(_) => (
                "系统设置操作未完成",
                "请检查系统权限和当前网络设置。",
                "settings",
            ),
        };
        UserMessage {
            title: title.into(),
            description: description.into(),
            action: action.into(),
            details: crate::app_log::sanitize(&self.to_string()),
        }
    }
}

#[cfg(test)]
mod friendly_tests {
    use super::*;
    #[test]
    fn restored_and_unrestored_failures_have_distinct_user_messages() {
        let restored = AppError::startup(StartupReason::ProxyApply, "fixture", Some(true)).dto();
        let failed = AppError::startup(StartupReason::ProxyApply, "fixture", Some(false)).dto();
        assert!(restored.user_message.description.contains("已恢复"));
        assert!(!failed.user_message.description.contains("已恢复"));
    }
    #[test]
    fn generic_conflict_does_not_invent_an_external_proxy_owner() {
        let dto = AppError::Conflict("a stale revision".into()).dto();
        assert_eq!(dto.user_message.action, "refresh");
        assert!(!dto.user_message.description.contains("其他代理"));
    }
}
