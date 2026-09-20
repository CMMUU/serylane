use crate::error::{AppError, AppResult};
use crate::models::{SubscriptionMetadata, SubscriptionUsage};
use futures_util::StreamExt;
use reqwest::header::{
    ACCEPT, ACCEPT_LANGUAGE, CACHE_CONTROL, CONTENT_TYPE, ETAG, IF_MODIFIED_SINCE, IF_NONE_MATCH,
    LAST_MODIFIED,
};
use serde::Serialize;
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

fn subscription_transport_error(error: reqwest::Error) -> AppError {
    // Reqwest attaches the request URL to transport/body errors. Subscription
    // URLs commonly carry credentials in their query, so never forward it to
    // logs, IPC, or the user-visible toast.
    AppError::Subscription(error.without_url().to_string())
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
    client: reqwest::Client,
    total_timeout: std::time::Duration,
}

impl SubscriptionFetcher {
    /// Quota is response metadata, independent of the YAML/ETag. HEAD first;
    /// services without HEAD metadata get a bounded GET whose body is dropped.
    /// This path never validates, persists or applies a configuration.
    pub async fn fetch_usage(
        &self,
        url: &str,
        user_agent: &str,
    ) -> AppResult<Option<SubscriptionUsage>> {
        let parsed = validate_subscription_url(url)?;
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
        for method in [reqwest::Method::HEAD, reqwest::Method::GET] {
            for agent in &agents {
                let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
                if remaining.is_zero() {
                    return Err(subscription_timeout_error());
                }
                let response = tokio::time::timeout(
                    remaining,
                    self.client
                        .request(method.clone(), parsed.clone())
                        .header(reqwest::header::USER_AGENT, *agent)
                        .header(ACCEPT, SUBSCRIPTION_ACCEPT)
                        .header(CACHE_CONTROL, "no-cache")
                        .send(),
                )
                .await
                .map_err(|_| subscription_timeout_error())?
                .map_err(subscription_transport_error)?;
                if response.status() == reqwest::StatusCode::FORBIDDEN {
                    continue;
                }
                if method == reqwest::Method::HEAD
                    && matches!(response.status().as_u16(), 405 | 501)
                {
                    break;
                }
                if !response.status().is_success() {
                    return Err(AppError::Subscription(format!(
                        "HTTP {}",
                        response.status().as_u16()
                    )));
                }
                let usage = response_usage(response.headers());
                if usage.is_some() || method == reqwest::Method::GET {
                    return Ok(usage);
                }
                break;
            }
        }
        Err(AppError::Subscription("HTTP 403".into()))
    }

    pub fn new() -> AppResult<Self> {
        let client = reqwest::Client::builder()
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
            total_timeout: TOTAL_FETCH_TIMEOUT,
        })
    }

    pub async fn fetch(
        &self,
        url: &str,
        user_agent: &str,
        etag: Option<&str>,
        last_modified: Option<&str>,
    ) -> AppResult<FetchedSubscription> {
        let parsed = validate_subscription_url(url)?;
        let configured_user_agent = match user_agent.trim() {
            "" => DEFAULT_SUBSCRIPTION_USER_AGENT,
            value => value,
        };
        let mut attempts = vec![
            (configured_user_agent, true, false),
            (configured_user_agent, false, true),
        ];
        for fallback in COMPATIBLE_USER_AGENTS {
            if fallback != configured_user_agent {
                attempts.push((fallback, false, true));
            }
        }

        let mut response = None;
        let deadline = tokio::time::Instant::now() + self.total_timeout;
        for (attempt_user_agent, include_validators, include_compatibility_headers) in attempts {
            let mut request = self
                .client
                .get(parsed.clone())
                .header(reqwest::header::USER_AGENT, attempt_user_agent);
            if include_compatibility_headers {
                request = request
                    .header(ACCEPT, SUBSCRIPTION_ACCEPT)
                    .header(ACCEPT_LANGUAGE, "zh-CN,zh;q=0.9,en;q=0.6")
                    .header(CACHE_CONTROL, "no-cache");
            }
            if include_validators {
                if let Some(etag) = etag {
                    request = request.header(IF_NONE_MATCH, etag);
                }
                if let Some(last_modified) = last_modified {
                    request = request.header(IF_MODIFIED_SINCE, last_modified);
                }
            }
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return Err(subscription_timeout_error());
            }
            let candidate = tokio::time::timeout(remaining, request.send())
                .await
                .map_err(|_| subscription_timeout_error())?
                .map_err(subscription_transport_error)?;
            if candidate.status() != reqwest::StatusCode::FORBIDDEN {
                response = Some(candidate);
                break;
            }
        }
        let response = response.ok_or_else(|| {
            AppError::Subscription(
                "HTTP 403（订阅服务拒绝访问；已自动尝试兼容请求头。请确认链接或令牌未过期，或填写服务商指定的 User-Agent）"
                    .to_string(),
            )
        })?;
        if response.status() == reqwest::StatusCode::NOT_MODIFIED {
            let usage = response_usage(response.headers());
            return Ok(FetchedSubscription {
                content: None,
                metadata: SubscriptionMetadata {
                    content_type: None,
                    etag: etag.map(str::to_string),
                    last_modified: last_modified.map(str::to_string),
                    bytes: 0,
                    usage,
                },
                not_modified: true,
            });
        }
        if !response.status().is_success() {
            return Err(AppError::Subscription(format!(
                "HTTP {}",
                response.status().as_u16()
            )));
        }
        if response
            .content_length()
            .is_some_and(|size| size > MAX_SUBSCRIPTION_BYTES as u64)
        {
            return Err(AppError::Subscription(
                "订阅内容超过 4 MiB 限制".to_string(),
            ));
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
                return Err(AppError::Subscription(
                    "订阅内容超过 4 MiB 限制".to_string(),
                ));
            }
            bytes.extend_from_slice(&chunk);
        }
        if bytes.is_empty() {
            return Err(AppError::Subscription("订阅内容为空".to_string()));
        }
        let content = String::from_utf8(bytes)
            .map_err(|_| AppError::Subscription("订阅内容不是 UTF-8 文本".to_string()))?;
        Ok(FetchedSubscription {
            metadata: SubscriptionMetadata {
                bytes: content.len(),
                usage: response_usage(&headers),
                content_type: headers
                    .get(CONTENT_TYPE)
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_string),
                etag: headers
                    .get(ETAG)
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_string),
                last_modified: headers
                    .get(LAST_MODIFIED)
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_string),
            },
            content: Some(content),
            not_modified: false,
        })
    }
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
        parse_subscription_usage, response_usage, safe_subscription_error,
        validate_subscription_url, SubscriptionFetcher,
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
            server.await.unwrap();
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
    #[ignore = "requires TEST_SUBSCRIPTION_URL and network access"]
    async fn validates_an_external_subscription_fixture() {
        use crate::effective::build_effective_config;
        use crate::models::{AppSettings, RoutingMode};
        use std::process::Command;
        let Ok(url) = std::env::var("TEST_SUBSCRIPTION_URL") else {
            return;
        };
        let fetched = SubscriptionFetcher::new()
            .expect("fetcher")
            .fetch(&url, "clash.meta", None, None)
            .await
            .expect("fetch");
        let effective = build_effective_config(
            &fetched.content.expect("content"),
            &AppSettings::default(),
            RoutingMode::Rule,
        )
        .expect("effective");
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
        let directory = std::env::temp_dir().join(format!(
            "mihomo-subscription-pipeline-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&directory).expect("directory");
        let config = directory.join("effective.yaml");
        std::fs::write(&config, effective.yaml).expect("write");
        let output = Command::new(binary)
            .args(["-t", "-d"])
            .arg(&directory)
            .arg("-f")
            .arg(&config)
            .output()
            .expect("mihomo");
        if !output.status.success() {
            panic!("{}", String::from_utf8_lossy(&output.stderr));
        }
        let _ = std::fs::remove_dir_all(directory);
    }
}
