use crate::error::{AppError, AppResult};
use crate::models::AppSettings;
use serde::Serialize;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CheckPurpose {
    Connectivity,
    OpenAi,
}

#[derive(Clone, Copy)]
struct SafetyTarget<'a> {
    name: &'a str,
    url: &'a str,
    expected_status: u16,
    purpose: CheckPurpose,
}

const SAFETY_TARGETS: [SafetyTarget<'static>; 3] = [
    SafetyTarget {
        name: "google",
        url: "https://www.google.com/generate_204",
        expected_status: 204,
        purpose: CheckPurpose::Connectivity,
    },
    SafetyTarget {
        name: "cloudflare",
        url: "https://cp.cloudflare.com",
        expected_status: 204,
        purpose: CheckPurpose::Connectivity,
    },
    SafetyTarget {
        name: "openai",
        url: "https://api.openai.com/v1/models",
        expected_status: 401,
        purpose: CheckPurpose::OpenAi,
    },
];

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkSafetyCheck {
    pub target: String,
    pub url: String,
    pub success: bool,
    pub expected_status: u16,
    pub actual_status: Option<u16>,
    pub latency_ms: u128,
    pub detail: String,
    pub failure_kind: Option<TransportFailure>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportFailure {
    Timeout,
    Certificate,
    Tls,
    Dns,
    Tunnel,
    Connect,
    Other,
}

impl TransportFailure {
    fn transient(self) -> bool {
        matches!(
            self,
            Self::Timeout | Self::Dns | Self::Tunnel | Self::Connect
        )
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkSafetyReport {
    pub success: bool,
    pub proxy_endpoint: String,
    pub checks: Vec<NetworkSafetyCheck>,
    pub warnings: Vec<String>,
}

pub async fn verify_local_proxy(settings: &AppSettings) -> AppResult<NetworkSafetyReport> {
    let mut report = inspect_local_proxy(settings).await?;
    if should_retry(&report) {
        crate::app_log::record(
            1,
            crate::app_log::Area::Network,
            "连接检查用时较长，等待 1 秒后重试",
        );
        tokio::time::sleep(Duration::from_secs(1)).await;
        report = inspect_local_proxy(settings).await?;
    }
    require_connectivity(report, "连接检查暂未通过")
}

pub async fn inspect_local_proxy(settings: &AppSettings) -> AppResult<NetworkSafetyReport> {
    let endpoint = format!("http://127.0.0.1:{}", settings.mixed_port);
    let proxy =
        reqwest::Proxy::all(&endpoint).map_err(|error| AppError::Runtime(error.to_string()))?;
    let client = reqwest::Client::builder()
        .no_proxy()
        .proxy(proxy)
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(12))
        .redirect(reqwest::redirect::Policy::limited(3))
        .build()
        .map_err(|error| AppError::Runtime(error.to_string()))?;

    Ok(run_checks(&client, endpoint, SAFETY_TARGETS).await)
}

pub(crate) fn should_retry(report: &NetworkSafetyReport) -> bool {
    !report.success
        && report.checks.iter().any(|check| {
            check.target != "openai" && check.failure_kind.is_some_and(TransportFailure::transient)
        })
}

fn transport_error(error: &reqwest::Error) -> (TransportFailure, String) {
    use std::error::Error;
    let mut causes = Vec::new();
    let mut source = error.source();
    while let Some(cause) = source {
        causes.push(cause.to_string());
        if causes.len() >= 5 {
            break;
        }
        source = cause.source();
    }
    let chain = causes.join(" → ");
    let lower = chain.to_ascii_lowercase();
    let (kind, category) = if lower.contains("certificate") || lower.contains("invalid peer") {
        (TransportFailure::Certificate, "安全连接校验未通过")
    } else if error.is_timeout() {
        (TransportFailure::Timeout, "连接或响应超时")
    } else if lower.contains("dns") || lower.contains("resolve") {
        (TransportFailure::Dns, "DNS 解析失败")
    } else if lower.contains("tunnel") {
        (TransportFailure::Tunnel, "代理 CONNECT 隧道建立失败")
    } else if lower.contains("tls") {
        (TransportFailure::Tls, "连接建立时中断")
    } else if error.is_connect()
        || ["connection reset", "broken pipe", "unexpected eof"]
            .iter()
            .any(|term| lower.contains(term))
    {
        (TransportFailure::Connect, "TCP/代理连接失败")
    } else {
        (TransportFailure::Other, "网络传输失败")
    };
    let detail = if chain.is_empty() {
        category.to_string()
    } else {
        format!("{category}：{}", crate::app_log::sanitize(&chain))
    };
    (kind, detail)
}

pub async fn verify_tun_route() -> AppResult<NetworkSafetyReport> {
    let client = reqwest::Client::builder()
        .no_proxy()
        .connect_timeout(Duration::from_secs(6))
        .timeout(Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::limited(3))
        .build()
        .map_err(|error| AppError::Runtime(error.to_string()))?;
    require_connectivity(
        run_checks(&client, "system-tun".to_string(), SAFETY_TARGETS).await,
        "TUN 基础连通性验收失败",
    )
}

async fn run_checks(
    client: &reqwest::Client,
    endpoint: String,
    targets: [SafetyTarget<'_>; 3],
) -> NetworkSafetyReport {
    let (first, second, third) = tokio::join!(
        check_target(client, targets[0]),
        check_target(client, targets[1]),
        check_target(client, targets[2]),
    );
    let checks = vec![first, second, third];
    for check in &checks {
        crate::app_log::record(
            if check.success { 0 } else { 1 },
            crate::app_log::Area::Network,
            &format!(
                "{}；{}；{} ms",
                check.target, check.detail, check.latency_ms
            ),
        );
    }
    // OpenAI can reject an otherwise working route because of its own service
    // policy. Only the general connectivity targets decide whether it is usable.
    let success = checks
        .iter()
        .zip(targets)
        .any(|(check, target)| target.purpose == CheckPurpose::Connectivity && check.success);
    let warnings = checks
        .iter()
        .zip(targets)
        .filter(|(check, _)| !check.success)
        .map(|(check, target)| match target.purpose {
            CheckPurpose::Connectivity => {
                format!(
                    "基础连通性检测目标 {} 未通过: {}",
                    check.target, check.detail
                )
            }
            CheckPurpose::OpenAi => format!(
                "OpenAI 专项检测未通过: {}；不影响基础连通性判定",
                check.detail
            ),
        })
        .collect();
    NetworkSafetyReport {
        success,
        proxy_endpoint: endpoint,
        checks,
        warnings,
    }
}

fn require_connectivity(
    report: NetworkSafetyReport,
    failure_context: &str,
) -> AppResult<NetworkSafetyReport> {
    if report.success {
        Ok(report)
    } else {
        let failed = report
            .checks
            .iter()
            .filter(|check| !check.success)
            .map(|check| format!("{}: {}", check.target, check.detail))
            .collect::<Vec<_>>()
            .join("; ");
        Err(AppError::NetworkPreflight {
            retryable: should_retry(&report),
            message: format!(
                "{failure_context}，本地核心已启动，但基础连通性目标均未通过: {failed}。请在“日志 → 应用日志”查看原因，并检查所选节点、系统时间及安全软件；未跳过安全预检"
            ),
        })
    }
}

async fn check_target(
    client: &reqwest::Client,
    SafetyTarget {
        name: target,
        url,
        expected_status,
        ..
    }: SafetyTarget<'_>,
) -> NetworkSafetyCheck {
    let started = Instant::now();
    match client.get(url).send().await {
        Ok(response) => {
            let actual_status = response.status().as_u16();
            let success = actual_status == expected_status;
            NetworkSafetyCheck {
                target: target.to_string(),
                url: url.to_string(),
                success,
                expected_status,
                actual_status: Some(actual_status),
                latency_ms: started.elapsed().as_millis(),
                failure_kind: None,
                detail: if success {
                    if expected_status == 401 {
                        "HTTP 401：仅证明接口可达，未验证登录或模型请求".into()
                    } else {
                        format!("HTTP {actual_status}")
                    }
                } else {
                    format!("期望 HTTP {expected_status}，实际 HTTP {actual_status}")
                },
            }
        }
        Err(error) => {
            let (kind, detail) = transport_error(&error);
            NetworkSafetyCheck {
                target: target.to_string(),
                url: url.to_string(),
                success: false,
                expected_status,
                actual_status: None,
                latency_ms: started.elapsed().as_millis(),
                detail,
                failure_kind: Some(kind),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[test]
    fn safety_targets_cover_google_cloudflare_and_openai() {
        assert_eq!(SAFETY_TARGETS.len(), 3);
        assert_eq!(SAFETY_TARGETS[0].expected_status, 204);
        assert_eq!(SAFETY_TARGETS[1].expected_status, 204);
        assert_eq!(SAFETY_TARGETS[2].expected_status, 401);
        assert_eq!(SAFETY_TARGETS[0].purpose, CheckPurpose::Connectivity);
        assert_eq!(SAFETY_TARGETS[1].purpose, CheckPurpose::Connectivity);
        assert_eq!(SAFETY_TARGETS[2].purpose, CheckPurpose::OpenAi);
        assert!(SAFETY_TARGETS[2].url.contains("api.openai.com"));
    }

    async fn check_mock_statuses(statuses: [u16; 3]) -> NetworkSafetyReport {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            // Receive all three requests before replying. A sequential checker
            // would time out here, so these tests also cover parallel requests.
            let mut pending = Vec::new();
            for _ in 0..3 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                let mut buffer = [0; 1024];
                while !request.windows(4).any(|part| part == b"\r\n\r\n") {
                    let count = stream.read(&mut buffer).await.unwrap();
                    assert_ne!(count, 0, "request ended before its headers");
                    request.extend_from_slice(&buffer[..count]);
                    assert!(request.len() < 8192);
                }
                let request = String::from_utf8(request).unwrap();
                let path = request.split_whitespace().nth(1).unwrap();
                let index: usize = path.trim_start_matches('/').parse().unwrap();
                pending.push((stream, statuses[index]));
            }
            for (mut stream, status) in pending {
                let response = format!(
                    "HTTP/1.1 {status} Mock\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                );
                stream.write_all(response.as_bytes()).await.unwrap();
                stream.shutdown().await.unwrap();
            }
        });
        let urls = [0, 1, 2].map(|index| format!("http://{address}/{index}"));
        let targets = std::array::from_fn(|index| SafetyTarget {
            url: &urls[index],
            ..SAFETY_TARGETS[index]
        });
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(3))
            .build()
            .unwrap();
        let report = run_checks(&client, "local-test".to_string(), targets).await;
        tokio::time::timeout(Duration::from_secs(3), server)
            .await
            .expect("mock server did not receive all requests")
            .unwrap();
        assert_eq!(report.checks.len(), 3);
        for (check, status) in report.checks.iter().zip(statuses) {
            assert_eq!(check.actual_status, Some(status));
        }
        report
    }

    #[tokio::test]
    async fn loopback_connect_failure_retries_get_probes_only_and_reports_network_stage() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            for _ in 0..6 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut buffer = [0; 2048];
                let count = stream.read(&mut buffer).await.unwrap();
                let request = String::from_utf8_lossy(&buffer[..count]);
                assert!(request.starts_with("CONNECT "));
                assert!([
                    "www.google.com:443",
                    "cp.cloudflare.com:443",
                    "api.openai.com:443"
                ]
                .iter()
                .any(|host| request.contains(host)));
                assert!(!request.to_ascii_lowercase().contains("authorization:"));
                stream.write_all(b"HTTP/1.1 503 Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await.unwrap();
            }
        });
        let settings = AppSettings {
            mixed_port: port,
            ..Default::default()
        };
        let error = tokio::time::timeout(Duration::from_secs(8), verify_local_proxy(&settings))
            .await
            .unwrap()
            .unwrap_err()
            .dto();
        assert_eq!(error.code, "NETWORK_CHECK_FAILED");
        assert_eq!(error.stage, "network");
        assert!(error.retryable);
        tokio::time::timeout(Duration::from_secs(2), server)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn one_connectivity_target_and_openai_403_allow_the_route() {
        for statuses in [[204, 503, 403], [503, 204, 403]] {
            let report = check_mock_statuses(statuses).await;
            assert!(report.success);
            assert!(!report.checks[2].success);
            assert_eq!(report.warnings.len(), 2);
            assert!(report.warnings.iter().any(|warning| {
                warning.contains("OpenAI") && warning.contains("实际 HTTP 403")
            }));
            assert!(require_connectivity(report, "预检失败").is_ok());
        }
    }

    #[tokio::test]
    async fn failed_connectivity_targets_cannot_be_rescued_by_openai() {
        let report = check_mock_statuses([503, 502, 401]).await;
        assert!(!report.success);
        assert!(report.checks[2].success);
        assert_eq!(report.warnings.len(), 2);
        let error = require_connectivity(report, "代理基础连通性预检失败")
            .unwrap_err()
            .to_string();
        assert!(error.contains("基础连通性目标均未通过"));
        assert!(error.contains("google"));
        assert!(error.contains("cloudflare"));
        assert!(!error.contains("未接管"));
        assert!(!error.contains("已停止"));
    }

    #[tokio::test]
    async fn all_successful_targets_produce_no_warnings() {
        let report = check_mock_statuses([204, 204, 401]).await;
        assert!(report.success);
        assert!(report.checks.iter().all(|check| check.success));
        assert!(report.warnings.is_empty());
        assert!(require_connectivity(report, "验收失败").is_ok());
    }

    #[tokio::test]
    async fn retries_only_failed_transport_not_success_or_http_rejections() {
        let mut report = check_mock_statuses([503, 403, 401]).await;
        assert!(!should_retry(&report));
        report.checks[0].actual_status = None;
        report.checks[0].failure_kind = Some(TransportFailure::Timeout);
        assert!(should_retry(&report));
        report.checks[0].failure_kind = Some(TransportFailure::Tls);
        assert!(!should_retry(&report));
        report.success = true;
        assert!(!should_retry(&report));
        let report = check_mock_statuses([204, 204, 401]).await;
        assert!(report.checks[2].detail.contains("未验证登录或模型请求"));
    }

    #[tokio::test]
    async fn connection_errors_have_a_cause_without_leaking_url_credentials() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        let error = reqwest::Client::builder()
            .no_proxy()
            .build()
            .unwrap()
            .get(format!("http://{address}/private?token=do-not-log"))
            .send()
            .await
            .unwrap_err();
        let (_, detail) = transport_error(&error);
        assert!(detail.contains("连接失败"));
        assert!(!detail.contains("private"));
        assert!(!detail.contains("do-not-log"));
    }

    #[tokio::test]
    async fn openai_failure_is_retained_as_a_nonblocking_warning() {
        let report = check_mock_statuses([204, 204, 500]).await;
        assert!(report.success);
        assert!(!report.checks[2].success);
        assert_eq!(report.warnings.len(), 1);
        assert!(report.warnings[0].contains("OpenAI 专项检测未通过"));
        assert!(report.warnings[0].contains("实际 HTTP 500"));
        let serialized = serde_json::to_value(&report).unwrap();
        assert_eq!(serialized["proxyEndpoint"], "local-test");
        assert_eq!(serialized["checks"][2]["actualStatus"], 500);
        assert_eq!(serialized["warnings"].as_array().unwrap().len(), 1);
        assert!(require_connectivity(report, "TUN 基础连通性验收失败").is_ok());
    }
}
