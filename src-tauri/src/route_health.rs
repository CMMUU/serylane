//! Bounded diagnostics: categories only, never serialize an error chain, URL,
//! authorization header, prompt or response. Probes do not execute models.
use serde::Serialize;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Target {
    #[default]
    Chatgpt,
    OpenaiApi,
}
impl Target {
    pub fn label(self) -> &'static str {
        match self {
            Self::Chatgpt => "ChatGPT",
            Self::OpenaiApi => "OpenAI API",
        }
    }
    pub fn probe(self) -> (&'static str, &'static str) {
        match self {
            Self::Chatgpt => ("https://chatgpt.com/robots.txt", "200"),
            Self::OpenaiApi => ("https://api.openai.com/v1/models", "401"),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeState {
    #[default]
    Unknown,
    Passed,
    HttpUnverified,
    Failed,
}
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeReport {
    pub state: ProbeState,
    pub checked_at: u64,
    pub latency_ms: Option<u64>,
}
impl ProbeReport {
    pub fn evidence(&self) -> Option<bool> {
        match self.state {
            ProbeState::Passed => Some(true),
            ProbeState::Failed => Some(false),
            _ => None,
        }
    }
    pub fn fresh(&self, now: u64) -> Self {
        if now.saturating_sub(self.checked_at) > 240 {
            Self::default()
        } else {
            self.clone()
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Failure {
    LocalProxyUnavailable,
    Dns,
    Tls,
    Certificate,
    ConnectTimeout,
    ConnectionReset,
    Connect,
    Send,
    ResponseTimeout,
    Authentication,
    Permission,
    RateOrQuota,
    UpstreamServer,
    UpstreamRequest,
    StreamInterrupted,
    StreamIdle,
    ModelRejected,
}
impl Failure {
    pub fn code(self) -> &'static str {
        match self {
            Self::LocalProxyUnavailable => "local_proxy_unavailable",
            Self::Dns => "upstream_dns_failed",
            Self::Tls => "upstream_tls_failed",
            Self::Certificate => "upstream_certificate_failed",
            Self::ConnectTimeout => "upstream_connect_timeout",
            Self::ConnectionReset => "upstream_connection_reset",
            Self::Connect => "upstream_connection_failed",
            Self::Send => "upstream_send_failed",
            Self::ResponseTimeout => "upstream_response_timeout",
            Self::Authentication => "upstream_authentication_required",
            Self::Permission => "upstream_access_denied",
            Self::RateOrQuota => "upstream_rate_or_quota_limit",
            Self::UpstreamServer => "upstream_server_error",
            Self::UpstreamRequest => "upstream_request_rejected",
            Self::StreamInterrupted => "upstream_stream_interrupted",
            Self::StreamIdle => "upstream_stream_idle_timeout",
            Self::ModelRejected => "upstream_model_not_completed",
        }
    }
    pub fn message(self) -> &'static str {
        match self {
            Self::LocalProxyUnavailable => "本地出站代理不可达，请检查核心与端口；未处罚节点",
            Self::Dns => "连接阶段 DNS 解析失败；等待复查，未直接归因节点",
            Self::Tls => "TLS 握手中断；等待复查，未直接归因节点",
            Self::Certificate => "TLS 证书校验失败，请检查系统时间或网络检查软件；不会跳过校验",
            Self::ConnectTimeout => "出站连接建立超时；等待复查，未直接归因节点",
            Self::ConnectionReset => "上游连接被重置；请求是否已执行未知，未重放请求",
            Self::Connect => "出站代理或上游连接失败；等待复查",
            Self::Send => "上游请求发送失败；请求是否已执行未知，未重放请求",
            Self::ResponseTimeout => "等待上游响应头超时；请求可能已执行，未重放请求",
            Self::Authentication => "上游要求认证，请核对登录；未因此切换节点",
            Self::Permission => "上游拒绝访问，可能为权限、区域限制或挑战；未直接归因节点",
            Self::RateOrQuota => "上游限流或额度受限，请核对服务提示；未因此切换节点",
            Self::UpstreamServer => "上游服务返回错误；未直接归因节点或重放请求",
            Self::UpstreamRequest => "上游拒绝请求，请核对客户端与服务提示；未处罚节点",
            Self::StreamInterrupted => "上游模型流提前结束或连接中断；未重放请求",
            Self::StreamIdle => "上游流超过空闲时限；未重放请求",
            Self::ModelRejected => "上游明确返回模型失败或未完成事件；未直接归因节点或重放请求",
        }
    }
    pub fn warrants_check(self) -> bool {
        matches!(
            self,
            Self::Dns
                | Self::Tls
                | Self::ConnectTimeout
                | Self::ConnectionReset
                | Self::Connect
                | Self::Send
                | Self::ResponseTimeout
                | Self::StreamInterrupted
                | Self::StreamIdle
        )
    }
    pub fn from_status(status: u16) -> Self {
        match status {
            401 => Self::Authentication,
            403 => Self::Permission,
            429 => Self::RateOrQuota,
            500..=599 => Self::UpstreamServer,
            _ => Self::UpstreamRequest,
        }
    }
    pub fn from_reqwest(error: &reqwest::Error) -> Self {
        // Only inspect a bounded chain in memory; no raw string escapes this function.
        let mut detail = String::new();
        // The outer display contains the request URL/query. Inspect causes only,
        // so user-supplied query text cannot masquerade as an error category.
        let mut cause = std::error::Error::source(error);
        for _ in 0..8 {
            let Some(current) = cause else { break };
            detail.extend(current.to_string().chars().take(512));
            cause = current.source();
        }
        classify_transport(&detail, error.is_connect(), error.is_timeout())
    }
}
fn classify_transport(detail: &str, connect: bool, timeout: bool) -> Failure {
    let detail = detail.to_ascii_lowercase();
    if detail.contains("certificate") || detail.contains("certvalid") || detail.contains("x509") {
        Failure::Certificate
    } else if detail.contains("dns")
        || detail.contains("resolve")
        || detail.contains("no such host")
    {
        Failure::Dns
    } else if detail.contains("tls") || detail.contains("ssl") || detail.contains("handshake") {
        Failure::Tls
    } else if timeout {
        if connect {
            Failure::ConnectTimeout
        } else {
            Failure::ResponseTimeout
        }
    } else if detail.contains("reset") || detail.contains("broken pipe") || detail.contains("eof") {
        Failure::ConnectionReset
    } else if connect {
        Failure::Connect
    } else {
        Failure::Send
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Diagnostic {
    pub timestamp: i64,
    pub category: Failure,
    pub code: &'static str,
    pub message: &'static str,
    pub target: Target,
    pub stage: &'static str,
    pub elapsed_ms: u64,
    pub http_status: Option<u16>,
    pub attribution: &'static str,
}
impl Diagnostic {
    pub fn new(
        category: Failure,
        target: Target,
        stage: &'static str,
        elapsed_ms: u64,
        http_status: Option<u16>,
        attributed: bool,
    ) -> Self {
        Self {
            timestamp: chrono::Utc::now().timestamp_millis(),
            category,
            code: category.code(),
            message: category.message(),
            target,
            stage,
            elapsed_ms,
            http_status,
            attribution: if attributed {
                "verified"
            } else {
                "unconfirmed"
            },
        }
    }
    pub fn log_message(&self) -> String {
        format!(
            "{} · {} · {} · {} ms · HTTP {} · 出口{}；{}",
            self.target.label(),
            self.stage,
            self.code,
            self.elapsed_ms,
            self.http_status
                .map(|v| v.to_string())
                .unwrap_or_else(|| "未收到".into()),
            if self.attribution == "verified" {
                "已核对"
            } else {
                "未确认"
            },
            self.message
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn transport_categories_never_expose_error_secrets() {
        let cause = "client Connect tls handshake eof https://secret.invalid/token?key=private";
        assert_eq!(classify_transport(cause, true, false), Failure::Tls);
        let d = Diagnostic::new(Failure::Tls, Target::Chatgpt, "request", 400, None, false);
        let encoded = serde_json::to_string(&d).unwrap();
        assert!(!encoded.contains("private") && !encoded.contains("secret.invalid"));
        assert_eq!(
            classify_transport("invalid peer certificate", true, false),
            Failure::Certificate
        );
        assert_eq!(classify_transport("dns error", true, false), Failure::Dns);
        assert_eq!(
            classify_transport("connect", true, true),
            Failure::ConnectTimeout
        );
        assert_eq!(
            classify_transport("body", false, true),
            Failure::ResponseTimeout
        );
    }
    #[test]
    fn account_and_local_failures_do_not_trigger_node_review() {
        for status in [400, 401, 403, 429, 500, 503] {
            assert!(!Failure::from_status(status).warrants_check());
        }
        assert!(!Failure::LocalProxyUnavailable.warrants_check());
        assert!(!Failure::Certificate.warrants_check());
    }
    #[test]
    fn unverified_http_and_stale_probes_are_not_success_or_node_failure() {
        let report = ProbeReport {
            state: ProbeState::HttpUnverified,
            checked_at: 20,
            latency_ms: Some(50),
        };
        assert_eq!(report.evidence(), None);
        assert_eq!(report.fresh(261).state, ProbeState::Unknown);
        assert_eq!(Target::Chatgpt.probe().1, "200");
        assert_eq!(Target::OpenaiApi.probe().1, "401");
    }
}
