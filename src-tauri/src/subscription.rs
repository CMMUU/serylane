use crate::error::{AppError, AppResult};
use crate::models::{SubscriptionMetadata, SubscriptionUsage};
use crate::subscription_task::{DownloadOptions, DownloadPath, Phase, Task};
use futures_util::StreamExt;
use reqwest::header::{
    ACCEPT, ACCEPT_LANGUAGE, CACHE_CONTROL, CONTENT_TYPE, ETAG, IF_MODIFIED_SINCE, IF_NONE_MATCH,
    LAST_MODIFIED,
};
use serde::Serialize;
use std::sync::Arc;
use url::Url;

const MAX_SUBSCRIPTION_BYTES: usize = 4 * 1024 * 1024;
const DEFAULT_SUBSCRIPTION_USER_AGENT: &str = "clash.meta";
const SUBSCRIPTION_ACCEPT: &str =
    "application/yaml, text/yaml, text/plain, application/octet-stream;q=0.9, */*;q=0.5";
const COMPATIBLE_USER_AGENTS: [&str; 2] =
    [DEFAULT_SUBSCRIPTION_USER_AGENT, "ClashforWindows/0.20.39"];
const TOTAL_FETCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Keep only recognized numeric fields. Never persist/log the raw header, which
/// is untrusted and can contain account information or unexpected text.
fn parse_subscription_usage(value: &str) -> Option<SubscriptionUsage> {
    if value.len() > 4096 {
        return None;
    }
    let mut fields = [None; 4];
    let mut seen = [false; 4];
    for field in value.split(';') {
        let Some((name, value)) = field.split_once('=') else {
            continue;
        };
        let index = match name.trim().to_ascii_lowercase().as_str() {
            "upload" => 0,
            "download" => 1,
            "total" => 2,
            "expire" => 3,
            _ => continue,
        };
        if seen[index] {
            // Ambiguous duplicate fields must not produce a plausible quota.
            fields[index] = None;
            continue;
        }
        seen[index] = true;
        let value = value.trim();
        let limit = if index == 3 {
            253_402_300_799
        } else {
            9_007_199_254_740_991
        };
        // Byte counts and Unix seconds are integers. Reject signs, units,
        // decimals, overflow and unsafe JavaScript integer values.
        if !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()) {
            fields[index] = value.parse::<u64>().ok().filter(|number| *number <= limit);
        }
    }
    fields
        .iter()
        .any(Option::is_some)
        .then_some(SubscriptionUsage {
            upload_bytes: fields[0],
            download_bytes: fields[1],
            total_bytes: fields[2],
            expires_at: fields[3],
        })
}

fn response_usage(headers: &reqwest::header::HeaderMap) -> Option<SubscriptionUsage> {
    let mut values = headers.get_all("subscription-userinfo").iter();
    let value = values.next()?;
    if values.next().is_some() {
        return None;
    }
    value.to_str().ok().and_then(parse_subscription_usage)
}

/// Refresh history is shown long after the original request. Use only fixed
/// summaries and a strictly parsed HTTP status; never forward an error payload.
pub(crate) fn safe_subscription_error(error: &AppError) -> String {
    let summary = match error {
        AppError::Subscription(message) => {
            if let Some(status) = message
                .strip_prefix("HTTP ")
                .and_then(|tail| {
                    let code = tail.get(..3)?;
                    (code.bytes().all(|byte| byte.is_ascii_digit())
                        && tail.chars().nth(3).is_none_or(|ch| !ch.is_ascii_digit()))
                    .then(|| code.parse::<u16>().ok())
                    .flatten()
                })
                .filter(|code| (100..=599).contains(code))
            {
                return format!("订阅请求失败：HTTP {status}；已保留原配置和用量记录");
            }
            if message.contains("超时") || message.to_ascii_lowercase().contains("timed out") {
                "订阅请求超时；已保留原配置和用量记录"
            } else {
                "订阅请求失败；已保留原配置和用量记录"
            }
        }
        AppError::Config(_) | AppError::Runtime(_) => {
            "订阅配置校验或应用失败；已保留原配置和用量记录"
        }
        AppError::Io(_) | AppError::NotFound(_) => "订阅本地数据读写失败；请检查本地记录",
        AppError::Conflict(_) => "订阅更新遇到状态冲突；请稍后重试",
        AppError::InvalidInput(_) => "订阅地址或请求参数无效；请检查订阅设置",
        _ => "订阅更新未完成；请稍后重试",
    };
    summary.to_string()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TransportFailure {
    Timeout,
    Dns,
    Tls,
    Connection,
    Interrupted,
    Request,
}

impl TransportFailure {
    fn classify(error: &reqwest::Error) -> Self {
        // Inspect causes only for classification. Never expose cause text or URLs.
        let mut cause: Option<&(dyn std::error::Error + 'static)> = Some(error);
        let mut dns = false;
        let mut interrupted = false;
        while let Some(value) = cause {
            interrupted |= value.downcast_ref::<std::io::Error>().is_some_and(|e| {
                matches!(
                    e.kind(),
                    std::io::ErrorKind::UnexpectedEof
                        | std::io::ErrorKind::ConnectionReset
                        | std::io::ErrorKind::ConnectionAborted
                        | std::io::ErrorKind::BrokenPipe
                )
            });
            let text = value.to_string().to_ascii_lowercase();
            if text.contains("certificate") || text.contains("tls handshake") {
                return Self::Tls;
            }
            dns |= text.contains("dns error") || text.contains("failed to lookup address");
            interrupted |= text.contains("connection closed before message completed")
                || text.contains("connection reset")
                || text.contains("broken pipe");
            cause = value.source();
        }
        if error.is_timeout() {
            Self::Timeout
        } else if dns {
            Self::Dns
        } else if error.is_connect() {
            Self::Connection
        } else if interrupted || error.is_body() {
            Self::Interrupted
        } else {
            Self::Request
        }
    }

    fn retryable(self) -> bool {
        matches!(
            self,
            Self::Timeout | Self::Dns | Self::Connection | Self::Interrupted
        )
    }

    fn message(self) -> &'static str {
        match self {
            Self::Timeout => "订阅请求超时",
            Self::Dns => "订阅域名解析失败",
            Self::Tls => "订阅安全连接校验失败",
            Self::Connection => "订阅服务器连接失败",
            Self::Interrupted => "订阅连接意外中断",
            Self::Request => "订阅请求发送或读取失败",
        }
    }
}

fn subscription_transport_error(error: reqwest::Error) -> AppError {
    AppError::Subscription(TransportFailure::classify(&error).message().into())
}

pub(crate) fn subscription_user_message(
    message: &str,
) -> (&'static str, &'static str, &'static str) {
    let (title, description) = if message == "订阅任务已取消" {
        ("订阅任务已取消", "原有配置未改变，添加表单已保留。")
    } else if message.contains("规则资源未就绪") {
        (
            "订阅已下载，校验资源尚未就绪",
            "Mihomo 所需的地理规则资源缺失或下载失败。请检查资源下载网络后重试；原有配置未改变。",
        )
    } else if message.contains("网页") {
        (
            "服务商返回了网页内容",
            "请使用 Clash / Mihomo 订阅地址，而非登录页或分享页面；原有配置未改变。",
        )
    } else if message.contains("格式不是") || message.contains("首次添加") {
        (
            "订阅内容不符合配置格式",
            "请使用服务商提供的 Clash / Mihomo YAML 订阅；原有配置未改变。",
        )
    } else if message.starts_with("HTTP 401") || message.starts_with("HTTP 403") {
        ("订阅服务拒绝了请求", "请核对服务商要求的客户端标识（User-Agent）或重新复制订阅链接。其他客户端可用时，可填写相同的客户端标识后重试。")
    } else if message.starts_with("HTTP 429") {
        ("订阅请求过于频繁", "请稍等片刻再试，避免连续点击刷新。")
    } else if message.starts_with("HTTP 5") {
        ("订阅服务暂时异常", "服务商暂未返回可用结果，请稍后重试。")
    } else if message.contains("超时") {
        ("获取订阅超时", "服务商未在等待时间内完成响应。请重试；如其他客户端可用，请检查当前网络路径和客户端标识。")
    } else if message == "订阅域名解析失败" {
        ("订阅域名解析失败", "请检查 DNS 与当前网络连接后重试。")
    } else if message == "订阅安全连接校验失败" {
        (
            "订阅安全连接未通过校验",
            "请检查系统时间，或联系服务商检查 HTTPS 证书。",
        )
    } else if message == "订阅服务器连接失败" || message == "订阅连接意外中断" {
        (
            "连接订阅服务未成功",
            "连接失败或中途断开，请检查当前网络路径后重试。原有配置未受影响。",
        )
    } else if message.contains("为空") || message.contains("UTF-8") || message.contains("4 MiB") {
        (
            "订阅返回的内容需要检查",
            "请使用服务商提供的 Clash / Mihomo 订阅格式，并检查是否返回了空内容或超大文件。",
        )
    } else {
        (
            "订阅下载未完成",
            "请重试或核对服务商指定的订阅格式与客户端标识；原有配置未受影响。",
        )
    };
    (title, description, "subscriptions")
}

fn subscription_timeout_error() -> AppError {
    AppError::Subscription("订阅请求超时（兼容重试总计不超过 30 秒）".to_string())
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FetchedSubscription {
    pub content: Option<String>,
    pub metadata: SubscriptionMetadata,
    pub not_modified: bool,
}

#[derive(Debug, Clone)]
pub struct SubscriptionFetcher {
    task: Option<Arc<Task>>,
    client: reqwest::Client,
    total_timeout: std::time::Duration,
}

impl SubscriptionFetcher {
    /// One transport retry for the entire operation, sharing the existing deadline.
    /// Only GET/HEAD are passed here. HTTP compatibility attempts retain their own cap.
    async fn send_request(
        &self,
        request: reqwest::RequestBuilder,
        deadline: tokio::time::Instant,
        retry_available: &mut bool,
    ) -> AppResult<reqwest::Response> {
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return Err(subscription_timeout_error());
            }
            let attempt = request
                .try_clone()
                .ok_or_else(|| AppError::Subscription("订阅请求参数无效".into()))?;
            if let Some(task) = &self.task {
                task.check()?;
                task.request();
            }
            let send = async { Ok(tokio::time::timeout(remaining, attempt.send()).await) };
            let sent = if let Some(task) = &self.task {
                task.wait(send).await?
            } else {
                send.await?
            };
            match sent {
                Ok(Ok(response)) => return Ok(response),
                Err(_) => return Err(subscription_timeout_error()),
                Ok(Err(error)) => {
                    if !*retry_available || !TransportFailure::classify(&error).retryable() {
                        return Err(subscription_transport_error(error));
                    }
                    *retry_available = false;
                    let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
                    if remaining <= std::time::Duration::from_millis(200) {
                        return Err(subscription_transport_error(error));
                    }
                    self.pause(std::time::Duration::from_millis(200)).await?;
                }
            }
        }
    }

    /// Quota is response metadata, independent of the YAML/ETag. HEAD first;
    /// services without HEAD metadata get a bounded GET whose body is dropped.
    /// This path never validates, persists or applies a configuration.
    pub async fn fetch_usage(
        &self,
        url: &str,
        user_agent: &str,
    ) -> AppResult<Option<SubscriptionUsage>> {
        let parsed = validate_subscription_url(url)?;
        if is_loopback_host(&parsed) {
            if let Some(task) = &self.task {
                task.path("direct");
            }
        }
        let configured = if user_agent.trim().is_empty() {
            DEFAULT_SUBSCRIPTION_USER_AGENT
        } else {
            user_agent.trim()
        };
        let mut agents = vec![configured];
        for agent in COMPATIBLE_USER_AGENTS {
            if !agents.contains(&agent) {
                agents.push(agent);
            }
        }
        let deadline = tokio::time::Instant::now() + self.total_timeout;
        let mut retry_available = true;
        for method in [reqwest::Method::HEAD, reqwest::Method::GET] {
            'agents: for agent in &agents {
                loop {
                    let response = self
                        .send_request(
                            self.client
                                .request(method.clone(), parsed.clone())
                                .header(reqwest::header::USER_AGENT, *agent)
                                .header(ACCEPT, SUBSCRIPTION_ACCEPT)
                                .header(CACHE_CONTROL, "no-cache"),
                            deadline,
                            &mut retry_available,
                        )
                        .await?;
                    let status = response.status().as_u16();
                    if let Some(task) = &self.task {
                        task.response(status);
                    }
                    if status == 403 {
                        continue 'agents;
                    }
                    if method == reqwest::Method::HEAD && matches!(status, 405 | 501) {
                        break 'agents;
                    }
                    if matches!(status, 429 | 502 | 503 | 504) {
                        let delay = retry_delay(response.headers());
                        drop(response);
                        if self.retry(&mut retry_available, delay, deadline).await? {
                            continue;
                        }
                        return Err(AppError::Subscription(format!("HTTP {status}")));
                    }
                    if !response.status().is_success() {
                        return Err(AppError::Subscription(format!("HTTP {status}")));
                    }
                    let usage = response_usage(response.headers());
                    if usage.is_some() || method == reqwest::Method::GET {
                        return Ok(usage);
                    }
                    break 'agents;
                }
            }
        }
        Err(AppError::Subscription("HTTP 403".into()))
    }

    /// Reuse only this application's already-running core. This is a status read,
    /// not permission to start a core, change system proxy, or discover other apps.
    pub fn for_task(
        app: &tauri::AppHandle,
        options: &DownloadOptions,
        task: Option<Arc<Task>>,
    ) -> AppResult<Self> {
        use tauri::Manager;
        let running = app
            .state::<crate::runtime::MihomoRuntime>()
            .status(Some(app))
            .phase
            == crate::models::RuntimePhase::Running;
        let (port, label) = match options.path {
            DownloadPath::Direct => (None, "direct"),
            DownloadPath::LocalProxy => (
                Some(options.proxy_port.filter(|p| *p != 0).ok_or_else(|| {
                    AppError::InvalidInput("请填写已运行的本地 HTTP 代理端口".into())
                })?),
                "local_proxy",
            ),
            DownloadPath::FollowCore if running => (
                Some(
                    crate::storage::AppStorage::from_app(app)?
                        .settings()?
                        .mixed_port,
                ),
                "serylane",
            ),
            DownloadPath::FollowCore => (None, "direct"),
        };
        let mut fetcher = Self::with_running_proxy(port)?;
        if let Some(task) = &task {
            task.path(label);
            task.check()?;
        }
        fetcher.task = task;
        Ok(fetcher)
    }

    #[cfg(test)]
    pub fn new() -> AppResult<Self> {
        Self::with_running_proxy(None)
    }

    fn with_running_proxy(port: Option<u16>) -> AppResult<Self> {
        let mut builder = reqwest::Client::builder().no_proxy();
        if let Some(port) = port {
            // Never fall back to direct access after the selected core fails.
            // Local subscription fixtures remain local, avoiding a proxy loop.
            let endpoint = format!("http://127.0.0.1:{port}");
            builder = builder.no_proxy().proxy(reqwest::Proxy::custom(move |url| {
                (!is_loopback_host(url)).then(|| endpoint.clone())
            }));
        }
        let client = builder
            .referer(false)
            .connect_timeout(std::time::Duration::from_secs(8))
            .timeout(std::time::Duration::from_secs(20))
            .redirect(reqwest::redirect::Policy::custom(|attempt| {
                if attempt.previous().len() >= 5 {
                    return attempt.stop();
                }
                let target = attempt.url();
                if target.scheme() == "https"
                    || (target.scheme() == "http" && is_loopback_host(target))
                {
                    attempt.follow()
                } else {
                    attempt.stop()
                }
            }))
            .build()
            .map_err(subscription_transport_error)?;
        Ok(Self {
            client,
            task: None,
            total_timeout: TOTAL_FETCH_TIMEOUT,
        })
    }

    async fn pause(&self, duration: std::time::Duration) -> AppResult<()> {
        if let Some(task) = &self.task {
            task.phase(Phase::Retrying);
            task.wait(async {
                tokio::time::sleep(duration).await;
                Ok(())
            })
            .await
        } else {
            tokio::time::sleep(duration).await;
            Ok(())
        }
    }

    async fn retry(
        &self,
        available: &mut bool,
        delay: std::time::Duration,
        deadline: tokio::time::Instant,
    ) -> AppResult<bool> {
        if !*available || delay >= deadline.saturating_duration_since(tokio::time::Instant::now()) {
            return Ok(false);
        }
        *available = false;
        self.pause(delay).await?;
        Ok(true)
    }

    pub async fn fetch(
        &self,
        url: &str,
        user_agent: &str,
        etag: Option<&str>,
        last_modified: Option<&str>,
    ) -> AppResult<FetchedSubscription> {
        let parsed = validate_subscription_url(url)?;
        if is_loopback_host(&parsed) {
            if let Some(task) = &self.task {
                task.path("direct");
            }
        }
        let agent = if user_agent.trim().is_empty() {
            DEFAULT_SUBSCRIPTION_USER_AGENT
        } else {
            user_agent.trim()
        };
        let mut attempts = vec![(agent, true, false), (agent, false, true)];
        for fallback in COMPATIBLE_USER_AGENTS {
            if fallback != agent {
                attempts.push((fallback, false, true));
            }
        }
        let deadline = tokio::time::Instant::now() + self.total_timeout;
        // Shared across compatibility attempts, sends AND bodies; no multiplicative retry loops.
        let mut retry_available = true;
        for (agent, validators, compat) in attempts {
            loop {
                let mut request = self
                    .client
                    .get(parsed.clone())
                    .header(reqwest::header::USER_AGENT, agent);
                if compat {
                    request = request
                        .header(ACCEPT, SUBSCRIPTION_ACCEPT)
                        .header(ACCEPT_LANGUAGE, "zh-CN,zh;q=0.9,en;q=0.6")
                        .header(CACHE_CONTROL, "no-cache");
                }
                if validators {
                    if let Some(etag) = etag {
                        request = request.header(IF_NONE_MATCH, etag);
                    }
                    if let Some(modified) = last_modified {
                        request = request.header(IF_MODIFIED_SINCE, modified);
                    }
                }
                let response = self
                    .send_request(request, deadline, &mut retry_available)
                    .await?;
                let status = response.status().as_u16();
                if let Some(task) = &self.task {
                    task.response(status);
                }
                if status == 403 {
                    break;
                }
                if matches!(status, 429 | 502 | 503 | 504) {
                    let delay = retry_delay(response.headers());
                    if response
                        .headers()
                        .contains_key(reqwest::header::RETRY_AFTER)
                    {
                        if let Some(task) = &self.task {
                            task.retry_after(delay.as_secs().max(1));
                        }
                    }
                    drop(response);
                    if self.retry(&mut retry_available, delay, deadline).await? {
                        continue;
                    }
                    return Err(AppError::Subscription(format!("HTTP {status}")));
                }
                if status == 304 {
                    if etag.is_none() && last_modified.is_none() {
                        return Err(AppError::Subscription(
                            "订阅服务未返回首次添加所需内容".into(),
                        ));
                    }
                    return Ok(FetchedSubscription {
                        content: None,
                        not_modified: true,
                        metadata: SubscriptionMetadata {
                            content_type: None,
                            etag: etag.map(str::to_string),
                            last_modified: last_modified.map(str::to_string),
                            bytes: 0,
                            usage: response_usage(response.headers()),
                        },
                    });
                }
                if !response.status().is_success() {
                    return Err(AppError::Subscription(format!("HTTP {status}")));
                }
                if let Some(task) = &self.task {
                    task.phase(Phase::Downloading);
                }
                let body = self.read_response(response, deadline);
                let result = if let Some(task) = &self.task {
                    task.wait(body).await
                } else {
                    body.await
                };
                match result {
                    Err(AppError::Subscription(ref text))
                        if ["订阅请求超时", "订阅连接意外中断", "订阅服务器连接失败"]
                            .contains(&text.as_str()) =>
                    {
                        if self
                            .retry(
                                &mut retry_available,
                                std::time::Duration::from_millis(200),
                                deadline,
                            )
                            .await?
                        {
                            continue;
                        }
                        return result;
                    }
                    result => return result,
                }
            }
        }
        Err(AppError::Subscription("HTTP 403".into()))
    }

    async fn read_response(
        &self,
        response: reqwest::Response,
        deadline: tokio::time::Instant,
    ) -> AppResult<FetchedSubscription> {
        if response
            .content_length()
            .is_some_and(|s| s > MAX_SUBSCRIPTION_BYTES as u64)
        {
            return Err(AppError::Subscription("订阅内容超过 4 MiB 限制".into()));
        }
        let headers = response.headers().clone();
        let mut stream = response.bytes_stream();
        let mut bytes = Vec::new();
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return Err(subscription_timeout_error());
            }
            let Some(chunk) = tokio::time::timeout(remaining, stream.next())
                .await
                .map_err(|_| subscription_timeout_error())?
            else {
                break;
            };
            let chunk = chunk.map_err(subscription_transport_error)?;
            if bytes.len() + chunk.len() > MAX_SUBSCRIPTION_BYTES {
                return Err(AppError::Subscription("订阅内容超过 4 MiB 限制".into()));
            }
            bytes.extend_from_slice(&chunk);
        }
        if bytes.is_empty() {
            return Err(AppError::Subscription("订阅内容为空".into()));
        }
        let content = String::from_utf8(bytes)
            .map_err(|_| AppError::Subscription("订阅内容不是 UTF-8 文本".into()))?;
        if let Some(task) = &self.task {
            task.bytes(content.len());
            task.phase(Phase::Checking);
        }
        validate_content(&content)?;
        Ok(FetchedSubscription {
            metadata: SubscriptionMetadata {
                bytes: content.len(),
                usage: response_usage(&headers),
                content_type: headers
                    .get(CONTENT_TYPE)
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_string),
                etag: headers
                    .get(ETAG)
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_string),
                last_modified: headers
                    .get(LAST_MODIFIED)
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_string),
            },
            content: Some(content),
            not_modified: false,
        })
    }
}

fn retry_delay(headers: &reqwest::header::HeaderMap) -> std::time::Duration {
    let value = headers
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok());
    if let Some(seconds) = value.and_then(|s| s.parse::<u64>().ok()) {
        return std::time::Duration::from_secs(seconds).max(std::time::Duration::from_millis(200));
    }
    if let Some(at) = value.and_then(|s| chrono::DateTime::parse_from_rfc2822(s).ok()) {
        return std::time::Duration::from_millis(
            (at.timestamp_millis() - chrono::Utc::now().timestamp_millis()).max(200) as u64,
        );
    }
    std::time::Duration::from_millis(500)
}

fn validate_content(content: &str) -> AppResult<()> {
    let text = content.trim_start_matches('\u{feff}').trim_start();
    let prefix = text
        .chars()
        .take(256)
        .collect::<String>()
        .to_ascii_lowercase();
    if prefix.starts_with("<!doctype html")
        || prefix.starts_with("<html")
        || prefix.starts_with("<head")
    {
        return Err(AppError::Subscription("订阅服务返回了网页而非配置".into()));
    }
    let value: serde_yaml::Value = serde_yaml::from_str(text)
        .map_err(|_| AppError::Subscription("订阅格式不是有效的 Clash / Mihomo YAML".into()))?;
    if !value.is_mapping() {
        return Err(AppError::Subscription(
            "订阅格式不是有效的 Clash / Mihomo YAML".into(),
        ));
    }
    Ok(())
}

fn validate_subscription_url(url: &str) -> AppResult<Url> {
    let parsed = Url::parse(url).map_err(|error| AppError::InvalidInput(error.to_string()))?;
    match parsed.scheme() {
        "https" => Ok(parsed),
        "http" if is_loopback_host(&parsed) => Ok(parsed),
        "http" => Err(AppError::InvalidInput(
            "远程订阅必须使用 HTTPS；HTTP 仅允许本机地址".to_string(),
        )),
        _ => Err(AppError::InvalidInput(
            "订阅地址只支持 HTTPS，或本机 HTTP".to_string(),
        )),
    }
}

fn is_loopback_host(url: &Url) -> bool {
    matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "::1"))
}

#[cfg(test)]
mod tests {
    use super::{
        parse_subscription_usage, response_usage, retry_delay, safe_subscription_error,
        validate_content, validate_subscription_url, SubscriptionFetcher,
    };
    use crate::error::AppError;
    use crate::models::SubscriptionMetadata;

    #[test]
    fn parses_usage_as_bytes_and_unix_seconds_without_defaulting_missing_fields() {
        let usage = parse_subscription_usage(
            " Upload = 0; DOWNLOAD=2048; total=4096; expire=1893456000; token=private",
        )
        .expect("usage");
        assert_eq!(usage.upload_bytes, Some(0));
        assert_eq!(usage.download_bytes, Some(2048));
        assert_eq!(usage.total_bytes, Some(4096));
        assert_eq!(usage.expires_at, Some(1893456000));
        let partial = parse_subscription_usage("download=12").expect("partial");
        assert_eq!(partial.upload_bytes, None);
        assert_eq!(partial.total_bytes, None);
        assert_eq!(partial.expires_at, None);
        assert!(!serde_json::to_string(&usage)
            .expect("json")
            .contains("private"));
    }

    #[test]
    fn malformed_ambiguous_and_out_of_range_usage_stays_unknown() {
        for header in [
            "",
            "token=secret",
            "upload=-1",
            "total=1 GiB",
            "download=NaN",
            "upload=1.5",
            "upload=18446744073709551616",
            "total=9007199254740992",
            "expire=253402300800",
            "upload=1;upload=2;upload=3",
        ] {
            assert!(parse_subscription_usage(header).is_none(), "{header}");
        }
        assert!(parse_subscription_usage(&"x".repeat(4097)).is_none());
        let usage = parse_subscription_usage("upload=1;UPLOAD=2;download=0;total=0;expire=0")
            .expect("valid remaining fields");
        assert_eq!(usage.upload_bytes, None);
        assert_eq!(usage.download_bytes, Some(0));
        assert_eq!(usage.total_bytes, Some(0));
        assert_eq!(usage.expires_at, Some(0));
    }

    #[test]
    fn legacy_metadata_does_not_invent_usage() {
        let metadata: SubscriptionMetadata = serde_json::from_str(
            r#"{"contentType":null,"etag":null,"lastModified":null,"bytes":2048}"#,
        )
        .expect("legacy metadata");
        assert!(metadata.usage.is_none());
        assert_eq!(metadata.bytes, 2048);
    }

    #[test]
    fn missing_or_duplicate_http_usage_headers_are_unknown() {
        use reqwest::header::{HeaderMap, HeaderValue};
        let mut headers = HeaderMap::new();
        assert!(response_usage(&headers).is_none());
        headers.append(
            "subscription-userinfo",
            HeaderValue::from_static("download=100"),
        );
        assert_eq!(
            response_usage(&headers).expect("one header").download_bytes,
            Some(100)
        );
        headers.append(
            "subscription-userinfo",
            HeaderValue::from_static("download=200"),
        );
        assert!(response_usage(&headers).is_none());
    }

    #[test]
    fn persistent_error_summaries_never_include_error_payloads() {
        let secret = "https://private.example/sub?token=not-for-display";
        for error in [
            AppError::Subscription(format!("HTTP 403 ({secret})")),
            AppError::Subscription(secret.into()),
            AppError::Config(secret.into()),
            AppError::Runtime(secret.into()),
            AppError::Io(secret.into()),
            AppError::Conflict(secret.into()),
            AppError::InvalidInput(secret.into()),
        ] {
            let safe = safe_subscription_error(&error);
            assert!(!safe.contains("private.example"));
            assert!(!safe.contains("not-for-display"));
        }
        assert!(
            safe_subscription_error(&AppError::Subscription("HTTP 403 rejected".into()))
                .contains("HTTP 403")
        );
        assert!(
            !safe_subscription_error(&AppError::Subscription("HTTP 403123 secret".into()))
                .contains("HTTP")
        );
    }

    #[test]
    fn allows_https_and_local_http() {
        assert!(validate_subscription_url("https://example.com/sub").is_ok());
        assert!(validate_subscription_url("http://127.0.0.1:8080/sub").is_ok());
    }

    #[test]
    fn rejects_remote_http_and_file_urls() {
        assert!(validate_subscription_url("http://example.com/sub").is_err());
        assert!(validate_subscription_url("file:///tmp/sub.yaml").is_err());
    }

    #[tokio::test]
    async fn quota_head_and_get_fallback_do_not_require_or_consume_yaml() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        for head_has_metadata in [true, false] {
            let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
                .await
                .unwrap();
            let address = listener.local_addr().unwrap();
            let server = tokio::spawn(async move {
                for method in if head_has_metadata {
                    vec!["HEAD"]
                } else {
                    vec!["HEAD", "GET"]
                } {
                    let (mut stream, _) = listener.accept().await.unwrap();
                    let mut request = [0; 2048];
                    let n = stream.read(&mut request).await.unwrap();
                    let request = String::from_utf8_lossy(&request[..n]);
                    assert!(request.starts_with(method));
                    assert!(!request.to_lowercase().contains("if-none-match"));
                    let headers = if method == "HEAD" && !head_has_metadata {
                        "HTTP/1.1 405 Method Not Allowed\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    } else {
                        // Deliberately no body: a quota-only GET must return on headers.
                        "HTTP/1.1 200 OK\r\nSubscription-Userinfo: upload=2; download=28; total=100\r\nContent-Length: 99999\r\nConnection: close\r\n\r\n"
                    };
                    stream.write_all(headers.as_bytes()).await.unwrap();
                }
            });
            let usage = SubscriptionFetcher::new()
                .unwrap()
                .fetch_usage(&format!("http://{address}/quota"), "clash.meta")
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                usage.total_bytes.unwrap()
                    - usage.upload_bytes.unwrap()
                    - usage.download_bytes.unwrap(),
                70
            );
            tokio::time::timeout(std::time::Duration::from_secs(3), server)
                .await
                .expect("expected request was not sent")
                .unwrap();
        }
    }

    #[tokio::test]
    async fn fetches_a_local_subscription_with_metadata() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("listener");
        let address = listener.local_addr().expect("address");
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept");
            let mut request = [0u8; 1024];
            let _ = stream.read(&mut request).await;
            let body = "proxies: []\nrules: []\n";
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: text/yaml\r\nETag: fixture\r\nSubscription-Userinfo: upload=0; download=2048; total=4096; expire=1893456000\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).await.expect("write");
        });
        let result = SubscriptionFetcher::new()
            .expect("fetcher")
            .fetch(
                &format!("http://127.0.0.1:{}/subscription", address.port()),
                "clash.meta",
                None,
                None,
            )
            .await
            .expect("fetch");
        assert_eq!(result.metadata.etag.as_deref(), Some("fixture"));
        assert_eq!(
            result
                .metadata
                .usage
                .as_ref()
                .expect("usage")
                .download_bytes,
            Some(2048)
        );
        assert!(result.content.expect("content").contains("proxies"));
    }

    #[tokio::test]
    async fn not_modified_response_can_still_supply_fresh_quota() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("listener");
        let address = listener.local_addr().expect("address");
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept");
            let mut request = [0u8; 1024];
            let _ = stream.read(&mut request).await;
            stream.write_all(b"HTTP/1.1 304 Not Modified\r\nSubscription-Userinfo: upload=5;download=100;total=1000\r\nConnection: close\r\n\r\n").await.expect("write");
        });
        let result = SubscriptionFetcher::new()
            .expect("fetcher")
            .fetch(
                &format!("http://127.0.0.1:{}/subscription", address.port()),
                "clash.meta",
                Some("cached"),
                None,
            )
            .await
            .expect("fetch");
        assert!(result.not_modified);
        assert!(result.content.is_none());
        assert_eq!(
            result.metadata.usage.expect("fresh quota").download_bytes,
            Some(100)
        );
    }

    #[tokio::test]
    async fn retries_a_forbidden_subscription_with_compatible_headers() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("listener");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move {
            let mut requests = Vec::new();
            for index in 0..3 {
                let (mut stream, _) = listener.accept().await.expect("accept");
                let mut request = [0u8; 2048];
                let size = stream.read(&mut request).await.expect("read");
                requests.push(String::from_utf8_lossy(&request[..size]).to_string());
                let response = if index < 2 {
                    "HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                        .to_string()
                } else {
                    let body = "proxies: []\nrules: []\n";
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: text/yaml\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    )
                };
                stream.write_all(response.as_bytes()).await.expect("write");
            }
            requests
        });

        let result = SubscriptionFetcher::new()
            .expect("fetcher")
            .fetch(
                &format!("http://127.0.0.1:{}/subscription", address.port()),
                "provider-blocked-client",
                Some("old-etag"),
                Some("Wed, 21 Oct 2015 07:28:00 GMT"),
            )
            .await
            .expect("compatible fallback");
        assert!(result.content.expect("content").contains("proxies"));

        let requests = server.await.expect("server");
        assert!(requests[0].contains("user-agent: provider-blocked-client"));
        assert!(requests[0].contains("if-none-match: old-etag"));
        assert!(requests[0].contains("if-modified-since: Wed, 21 Oct 2015 07:28:00 GMT"));
        assert!(!requests[0].contains("accept: application/yaml"));
        assert!(requests[1].contains("user-agent: provider-blocked-client"));
        assert!(!requests[1].contains("if-none-match:"));
        assert!(!requests[1].contains("if-modified-since:"));
        assert!(requests[1].contains("accept: application/yaml"));
        assert!(requests[2].contains("user-agent: clash.meta"));
    }

    #[tokio::test]
    async fn retries_an_initial_import_with_the_configured_user_agent_first() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("listener");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move {
            let mut requests = Vec::new();
            for index in 0..2 {
                let (mut stream, _) = listener.accept().await.expect("accept");
                let mut request = [0u8; 2048];
                let size = stream.read(&mut request).await.expect("read");
                requests.push(String::from_utf8_lossy(&request[..size]).to_string());
                let response = if index == 0 {
                    "HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                        .to_string()
                } else {
                    let body = "proxies: []\nrules: []\n";
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    )
                };
                stream.write_all(response.as_bytes()).await.expect("write");
            }
            requests
        });

        SubscriptionFetcher::new()
            .expect("fetcher")
            .fetch(
                &format!("http://127.0.0.1:{}/subscription", address.port()),
                "provider-required-client",
                None,
                None,
            )
            .await
            .expect("compatible configured user agent");
        let requests = server.await.expect("server");
        assert!(requests[0].contains("user-agent: provider-required-client"));
        assert!(!requests[0].contains("accept: application/yaml"));
        assert!(requests[1].contains("user-agent: provider-required-client"));
        assert!(requests[1].contains("accept: application/yaml"));
    }

    #[tokio::test]
    async fn bounds_forbidden_compatibility_attempts() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("listener");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move {
            let mut count = 0;
            for _ in 0..4 {
                let (mut stream, _) = listener.accept().await.expect("accept");
                let mut request = [0u8; 2048];
                let _ = stream.read(&mut request).await.expect("read");
                count += 1;
                stream
                    .write_all(
                        b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    )
                    .await
                    .expect("write");
            }
            count
        });

        let error = SubscriptionFetcher::new()
            .expect("fetcher")
            .fetch(
                &format!("http://127.0.0.1:{}/subscription", address.port()),
                "provider-blocked-client",
                Some("old-etag"),
                None,
            )
            .await
            .expect_err("all attempts should remain forbidden");
        assert!(error.to_string().contains("HTTP 403"));
        assert_eq!(server.await.expect("server"), 4);
    }

    #[tokio::test]
    async fn transport_errors_never_expose_subscription_urls() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("listener");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move {
            for index in 0..2 {
                let (mut stream, _) = listener.accept().await.expect("accept");
                let mut request = [0u8; 2048];
                let _ = stream.read(&mut request).await.expect("read");
                if index == 0 {
                    stream
                        .write_all(
                            b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                        )
                        .await
                        .expect("write");
                }
            }
        });

        let secret = "must-not-appear-in-errors";
        let error = SubscriptionFetcher::new()
            .expect("fetcher")
            .fetch(
                &format!(
                    "http://127.0.0.1:{}/subscription?token={secret}",
                    address.port()
                ),
                "provider-required-client",
                None,
                None,
            )
            .await
            .expect_err("closed connection");
        server.await.expect("server");
        let message = error.to_string();
        assert!(!message.contains(secret));
        assert!(!message.contains("/subscription?"));
    }

    #[tokio::test]
    async fn total_deadline_also_bounds_a_slow_response_body() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("listener");
        let address = listener.local_addr().expect("address");
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept");
            let mut request = [0u8; 2048];
            let _ = stream.read(&mut request).await.expect("read");
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 24\r\nContent-Type: text/yaml\r\nConnection: close\r\n\r\n",
                )
                .await
                .expect("headers");
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        });

        let mut fetcher = SubscriptionFetcher::new().expect("fetcher");
        fetcher.total_timeout = std::time::Duration::from_millis(60);
        let started = tokio::time::Instant::now();
        let error = fetcher
            .fetch(
                &format!("http://127.0.0.1:{}/subscription", address.port()),
                "clash.meta",
                None,
                None,
            )
            .await
            .expect_err("body must respect the total deadline");
        assert!(error.to_string().contains("总计不超过 30 秒"));
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
        server.await.expect("server");
    }

    #[tokio::test]
    async fn retries_one_interrupted_get_or_head_without_exposing_url() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        for quota in [false, true] {
            let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
                .await
                .unwrap();
            let address = listener.local_addr().unwrap();
            let server = tokio::spawn(async move {
                for attempt in 0..2 {
                    let (mut stream, _) = listener.accept().await.unwrap();
                    let mut buffer = [0; 2048];
                    let read = stream.read(&mut buffer).await.unwrap();
                    assert!(read > 0);
                    if attempt == 1 {
                        stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 12\r\nSubscription-Userinfo: upload=1; download=2; total=9\r\nConnection: close\r\n\r\nproxies: []\n").await.unwrap();
                    }
                }
            });
            let fetcher = SubscriptionFetcher::new().unwrap();
            let url = format!("http://{address}/sub?token=fixture-secret");
            if quota {
                assert!(fetcher
                    .fetch_usage(&url, "clash.meta")
                    .await
                    .unwrap()
                    .is_some());
            } else {
                assert!(fetcher
                    .fetch(&url, "clash.meta", None, None)
                    .await
                    .unwrap()
                    .content
                    .is_some());
            }
            tokio::time::timeout(std::time::Duration::from_secs(3), server)
                .await
                .expect("expected request was not sent")
                .unwrap();
        }
    }

    #[tokio::test]
    async fn stops_after_one_transport_retry() {
        use tokio::io::AsyncReadExt;
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for _ in 0..2 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut buffer = [0; 2048];
                let read = stream.read(&mut buffer).await.unwrap();
                assert!(read > 0);
            }
        });
        let error = SubscriptionFetcher::new()
            .unwrap()
            .fetch(
                &format!("http://{address}/sub?token=fixture-secret"),
                "clash.meta",
                None,
                None,
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("中断"));
        assert!(!error.to_string().contains("fixture-secret"));
        tokio::time::timeout(std::time::Duration::from_secs(3), server)
            .await
            .expect("expected request was not sent")
            .unwrap();
    }

    #[tokio::test]
    async fn running_proxy_carries_https_connect_without_sending_subscription_credentials() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        for quota in [false, true] {
            let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
                .await
                .unwrap();
            let port = listener.local_addr().unwrap().port();
            let server = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut buffer = [0; 4096];
                let read = stream.read(&mut buffer).await.unwrap();
                let request = String::from_utf8_lossy(&buffer[..read]);
                assert!(request.starts_with("CONNECT subscription.example.invalid:443 "));
                assert!(!request.contains("fixture-secret"));
                stream.write_all(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await.unwrap();
            });
            let mut fetcher = SubscriptionFetcher::with_running_proxy(Some(port)).unwrap();
            fetcher.total_timeout = std::time::Duration::from_secs(2);
            let url = "https://subscription.example.invalid/subscribe?token=fixture-secret";
            let error = if quota {
                fetcher.fetch_usage(url, "clash.meta").await.unwrap_err()
            } else {
                fetcher
                    .fetch(url, "clash.meta", None, None)
                    .await
                    .unwrap_err()
            };
            assert!(!error.to_string().contains("fixture-secret"));
            tokio::time::timeout(std::time::Duration::from_secs(2), server)
                .await
                .unwrap()
                .unwrap();
        }
    }

    #[tokio::test]
    async fn local_subscriptions_bypass_the_running_proxy_to_avoid_loops() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buffer = [0; 2048];
            let read = stream.read(&mut buffer).await.unwrap();
            assert!(read > 0);
            stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 12\r\nConnection: close\r\n\r\nproxies: []\n").await.unwrap();
        });
        let fetched = SubscriptionFetcher::with_running_proxy(Some(1))
            .unwrap()
            .fetch(&format!("http://{address}/sub"), "clash.meta", None, None)
            .await
            .unwrap();
        assert!(fetched.content.is_some());
        tokio::time::timeout(std::time::Duration::from_secs(3), server)
            .await
            .expect("expected request was not sent")
            .unwrap();
    }

    #[test]
    fn friendly_subscription_errors_distinguish_causes_without_forwarding_payloads() {
        for (message, expected) in [
            ("HTTP 403 token=secret", "拒绝"),
            ("HTTP 429", "频繁"),
            ("订阅请求超时", "超时"),
            ("订阅安全连接校验失败", "校验"),
            ("订阅域名解析失败", "解析"),
        ] {
            let dto = crate::error::AppError::Subscription(message.into()).dto();
            assert!(dto.user_message.title.contains(expected));
            assert!(!dto.user_message.description.contains("secret"));
        }
        assert!(!super::TransportFailure::Tls.retryable());
        assert!(!super::TransportFailure::Request.retryable());
    }

    #[tokio::test]
    #[ignore = "requires TEST_SUBSCRIPTION_URL and network access"]
    async fn validates_an_external_subscription_fixture() {
        use crate::effective::build_effective_config;
        use crate::models::{AppSettings, RoutingMode};
        let Ok(url) = std::env::var("TEST_SUBSCRIPTION_URL") else {
            return;
        };
        let port = std::env::var("TEST_SUBSCRIPTION_PROXY_PORT")
            .ok()
            .map(|value| value.parse::<u16>().expect("numeric local proxy port"));
        let fetched = SubscriptionFetcher::with_running_proxy(port)
            .expect("fetcher")
            .fetch(&url, "clash.meta", None, None)
            .await
            .expect("fetch");
        let content = fetched.content.expect("content");
        let effective =
            build_effective_config(&content, &AppSettings::default(), RoutingMode::Rule);
        assert!(effective.is_ok(), "effective config generation failed");
        let effective = effective.unwrap();
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let target = std::process::Command::new("rustc")
            .args(["--print", "host-tuple"])
            .output()
            .expect("target");
        let target = String::from_utf8(target.stdout)
            .expect("utf8")
            .trim()
            .to_string();
        let binary = root.join("binaries").join(format!("mihomo-{target}"));
        let directory = tempfile::tempdir().expect("private fixture directory");
        if let Ok(cache) = std::env::var("TEST_SUBSCRIPTION_DATA_DIR") {
            for name in ["GeoSite.dat", "geoip.metadb", "GeoIP.dat", "Country.mmdb"] {
                let source = std::path::Path::new(&cache).join(name);
                if source.is_file() {
                    std::fs::copy(source, directory.path().join(name))
                        .expect("geographic data copy");
                }
            }
        }
        let config = directory.path().join("effective.yaml");
        crate::runtime::write_private_file(&config, effective.yaml.as_bytes())
            .expect("private config");
        assert!(
            crate::runtime::validate_file(&binary, directory.path(), &config).is_ok(),
            "isolated native validation failed"
        );
        // Validate regular group selection modes against the same isolated source.
        for mode in [RoutingMode::Global, RoutingMode::Direct] {
            let config_value =
                build_effective_config(&content, &AppSettings::default(), mode).unwrap();
            crate::runtime::write_private_file(&config, config_value.yaml.as_bytes()).unwrap();
            assert!(crate::runtime::validate_file(&binary, directory.path(), &config).is_ok());
        }
    }
    async fn response_sequence(responses: Vec<Vec<u8>>) -> (String, tokio::task::JoinHandle<()>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let url = format!(
            "http://{}/sub?token=fixture-only",
            listener.local_addr().unwrap()
        );
        let server = tokio::spawn(async move {
            for response in responses {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = vec![0; 4096];
                assert!(socket.read(&mut request).await.unwrap() > 0);
                socket.write_all(&response).await.unwrap();
            }
        });
        (url, server)
    }
    fn good_response() -> Vec<u8> {
        b"HTTP/1.1 200 OK\r\nContent-Length: 12\r\nETag: second\r\nConnection: close\r\n\r\nproxies: []\n".to_vec()
    }
    #[tokio::test]
    async fn retries_complete_body_after_200_disconnect_without_appending_partial_bytes() {
        let (url, server) = response_sequence(vec![
            b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\nETag: first\r\nConnection: close\r\n\r\nproxies: [partial".to_vec(),
            good_response(),
        ]).await;
        let result = SubscriptionFetcher::new()
            .unwrap()
            .fetch(&url, "clash.meta", None, None)
            .await
            .unwrap();
        assert_eq!(result.content.as_deref(), Some("proxies: []\n"));
        assert_eq!(result.metadata.etag.as_deref(), Some("second"));
        tokio::time::timeout(std::time::Duration::from_secs(3), server)
            .await
            .expect("expected request was not sent")
            .unwrap();
    }
    #[tokio::test]
    async fn a_second_interrupted_body_exhausts_shared_retry_budget() {
        let broken =
            b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\nproxies:".to_vec();
        let (url, server) = response_sequence(vec![broken.clone(), broken]).await;
        let result = SubscriptionFetcher::new()
            .unwrap()
            .fetch(&url, "clash.meta", None, None)
            .await;
        assert!(result.is_err());
        let error = result.unwrap_err();
        assert!(!error.to_string().contains("fixture-only"));
        tokio::time::timeout(std::time::Duration::from_secs(3), server)
            .await
            .expect("expected request was not sent")
            .unwrap();
    }
    #[tokio::test]
    async fn retries_transient_http_once_but_honors_long_retry_after() {
        for status in [429, 502, 503, 504] {
            let response=format!("HTTP/1.1 {status} Temporary\r\nRetry-After: 0\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").into_bytes();
            let (url, server) = response_sequence(vec![response, good_response()]).await;
            assert!(SubscriptionFetcher::new()
                .unwrap()
                .fetch(&url, "clash.meta", None, None)
                .await
                .is_ok());
            tokio::time::timeout(std::time::Duration::from_secs(3), server)
                .await
                .expect("expected request was not sent")
                .unwrap();
        }
        let (url,server)=response_sequence(vec![
            b"HTTP/1.1 429 Limited\r\nRetry-After: 3600\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec()
        ]).await;
        let started = std::time::Instant::now();
        let error = SubscriptionFetcher::new()
            .unwrap()
            .fetch(&url, "clash.meta", None, None)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("HTTP 429"));
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
        tokio::time::timeout(std::time::Duration::from_secs(3), server)
            .await
            .expect("expected request was not sent")
            .unwrap();
    }
    #[tokio::test]
    async fn content_rejection_is_not_retried_and_never_exposes_response() {
        for (body, kind) in [
            ("<html>secret response</html>", "网页"),
            ("vmess://secret", "格式"),
            ("", "为空"),
            ("a: [secret", "格式"),
        ] {
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .into_bytes();
            let (url, server) = response_sequence(vec![response]).await;
            let error = SubscriptionFetcher::new()
                .unwrap()
                .fetch(&url, "clash.meta", None, None)
                .await
                .unwrap_err();
            assert!(error.to_string().contains(kind));
            assert!(!error.to_string().contains("secret"));
            tokio::time::timeout(std::time::Duration::from_secs(3), server)
                .await
                .expect("expected request was not sent")
                .unwrap();
        }
    }
    #[tokio::test]
    async fn cancelling_body_read_closes_request_promptly() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let url = format!("http://{}/sub", listener.local_addr().unwrap());
        let (ready, received) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.unwrap();
            let mut b = [0; 2048];
            assert!(s.read(&mut b).await.unwrap() > 0);
            s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\nproxies:")
                .await
                .unwrap();
            ready.send(()).unwrap();
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
        });
        let task = crate::subscription_task::TaskManager::default()
            .start(None, "import", None, None)
            .unwrap();
        let mut fetcher = SubscriptionFetcher::new().unwrap();
        fetcher.task = Some(task.clone());
        let future =
            tokio::spawn(async move { fetcher.fetch(&url, "clash.meta", None, None).await });
        received.await.unwrap();
        assert!(task.cancel());
        let result = tokio::time::timeout(std::time::Duration::from_secs(1), future)
            .await
            .unwrap()
            .unwrap();
        assert!(result.unwrap_err().to_string().contains("已取消"));
        server.abort();
    }
    #[test]
    fn retry_after_http_date_and_content_mapping_are_bounded() {
        let mut headers = reqwest::header::HeaderMap::new();
        let future = (chrono::Utc::now() + chrono::Duration::seconds(90)).to_rfc2822();
        headers.insert(reqwest::header::RETRY_AFTER, future.parse().unwrap());
        assert!(retry_delay(&headers).as_secs() >= 88);
        headers.insert(reqwest::header::RETRY_AFTER, "invalid".parse().unwrap());
        assert_eq!(retry_delay(&headers).as_millis(), 500);
        assert!(validate_content("\u{feff}proxies: []").is_ok());
        assert!(validate_content("- not-a-mapping").is_err());
    }
}
