use crate::error::{AppError, AppResult};
use crate::models::{AppSettings, RoutingMode};
use serde_json::{json, Value};
use std::time::Duration;
use url::Url;

/// Non-domain rules can be normalized by Mihomo (CIDR, regex, rule-set etc.).
/// Compare their count, and compare the entire leading domain overlay exactly.
/// Domain routes are always prepended; disabling also checks the base rule count.
struct RuleExpectation {
    mode: String,
    count: usize,
    prefix: Vec<(String, String, String)>,
}
impl RuleExpectation {
    fn from_yaml(yaml: &str) -> AppResult<Self> {
        let root: serde_yaml::Value = serde_yaml::from_str(yaml)
            .map_err(|_| AppError::Config("路由验收配置格式无效".into()))?;
        let mode = root["mode"].as_str().unwrap_or("rule").to_string();
        let rules = root["rules"].as_sequence().cloned().unwrap_or_default();
        let mut prefix = vec![];
        for rule in &rules {
            let Some(text) = rule.as_str() else { break };
            let parts: Vec<_> = text.split(',').map(str::trim).collect();
            if parts.len() != 3 || !matches!(parts[0], "DOMAIN" | "DOMAIN-WILDCARD") {
                break;
            }
            prefix.push((
                parts[0].replace('-', "").to_lowercase(),
                parts[1].to_string(),
                parts[2].to_string(),
            ));
        }
        Ok(Self {
            mode,
            count: rules.len(),
            prefix,
        })
    }
    fn check(&self, config: &Value, response: &Value) -> AppResult<()> {
        let mismatch =
            || AppError::Conflict("路由规则与核心实际状态不一致，请重新应用或关闭路由".into());
        if config["mode"].as_str() != Some(self.mode.as_str()) {
            return Err(mismatch());
        }
        let rules = response["rules"].as_array().ok_or_else(mismatch)?;
        if rules.len() != self.count {
            return Err(mismatch());
        }
        for (actual, (kind, payload, target)) in rules.iter().zip(&self.prefix) {
            if actual["type"].as_str().map(str::to_lowercase).as_ref() != Some(kind)
                || actual["payload"].as_str() != Some(payload.as_str())
                || actual["proxy"].as_str() != Some(target.as_str())
            {
                return Err(mismatch());
            }
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct MihomoApiClient {
    client: reqwest::Client,
    base_url: Url,
    secret: String,
}

/// The controller transport is not the tested node. Only its documented
/// delay-test 503/504 responses are probe failures; auth/reload/JSON errors
/// and an unreachable controller remain unknown.
#[derive(Debug, PartialEq, Eq)]
pub enum DelayProbe {
    Passed(u64),
    Failed,
    Unknown,
}

impl MihomoApiClient {
    pub fn new(settings: &AppSettings) -> AppResult<Self> {
        Self::from_endpoint(settings.controller_port, settings.controller_secret.clone())
    }

    pub fn from_endpoint(controller_port: u16, secret: String) -> AppResult<Self> {
        let base_url = Url::parse(&format!("http://127.0.0.1:{controller_port}/"))
            .map_err(|error| AppError::Runtime(error.to_string()))?;
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|error| AppError::Runtime(error.to_string()))?;
        Ok(Self {
            client,
            base_url,
            secret,
        })
    }

    pub async fn wait_ready(&self, timeout: Duration) -> AppResult<Value> {
        let deadline = std::time::Instant::now() + timeout;
        let mut last_error = None;
        while std::time::Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            match tokio::time::timeout(remaining, self.get("version")).await {
                Err(_) => break,
                Ok(result) => match result {
                    Ok(value) => return Ok(value),
                    Err(error) => last_error = Some(error.to_string()),
                },
            }
            tokio::time::sleep(
                Duration::from_millis(150)
                    .min(deadline.saturating_duration_since(std::time::Instant::now())),
            )
            .await;
        }
        Err(AppError::Runtime(
            last_error.unwrap_or_else(|| "Mihomo API 启动超时".to_string()),
        ))
    }

    pub async fn proxies(&self) -> AppResult<Value> {
        self.get("proxies").await
    }

    #[cfg(windows)]
    pub(crate) async fn configs(&self) -> AppResult<Value> {
        self.get("configs").await
    }

    pub async fn proxy_providers(&self) -> AppResult<Value> {
        let url = self.path_url(&["providers", "proxies"])?;
        self.get_url(url).await
    }

    pub async fn rules(&self) -> AppResult<Value> {
        self.get("rules").await
    }

    /// Verify the rule count, domain-rule prefix and runtime mode, not merely a successful PUT.
    /// No configuration or controller credentials are included in failure messages.
    pub(crate) async fn verify_rule_config(&self, yaml: &str) -> AppResult<()> {
        let expected = RuleExpectation::from_yaml(yaml)?;
        let (config, rules) = tokio::time::timeout(Duration::from_secs(3), async {
            tokio::try_join!(self.get("configs"), self.rules())
        })
        .await
        .map_err(|_| AppError::Runtime("路由核对超时，请刷新状态后重试".into()))?
        .map_err(|_| AppError::Runtime("路由核心状态读取失败，请检查核心后刷新状态".into()))?;
        expected.check(&config, &rules)
    }

    pub async fn connections(&self) -> AppResult<Value> {
        self.get("connections").await
    }

    pub async fn select_proxy(&self, group: &str, proxy: &str) -> AppResult<()> {
        let url = self.path_url(&["proxies", group])?;
        let response = self
            .client
            .put(url)
            .bearer_auth(&self.secret)
            .json(&json!({ "name": proxy }))
            .send()
            .await
            .map_err(|error| AppError::Runtime(error.to_string()))?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(AppError::Runtime(format!(
                "Mihomo API HTTP {}",
                response.status().as_u16()
            )))
        }
    }

    pub async fn clear_proxy_selection(&self, group: &str) -> AppResult<()> {
        let url = self.path_url(&["proxies", group])?;
        let response = self
            .client
            .delete(url)
            .bearer_auth(&self.secret)
            .send()
            .await
            .map_err(|error| AppError::Runtime(error.to_string()))?;
        ensure_success(response.status())
    }

    pub async fn delay(&self, proxy: &str, test_url: &str, timeout_ms: u32) -> AppResult<Value> {
        self.delay_expected(proxy, test_url, timeout_ms, None).await
    }

    pub async fn delay_expected(
        &self,
        proxy: &str,
        test_url: &str,
        timeout_ms: u32,
        expected_status: Option<&str>,
    ) -> AppResult<Value> {
        let mut url = self.path_url(&["proxies", proxy, "delay"])?;
        url.query_pairs_mut()
            .append_pair("url", test_url)
            .append_pair("timeout", &timeout_ms.clamp(1000, 30_000).to_string());
        if let Some(expected_status) = expected_status {
            url.query_pairs_mut()
                .append_pair("expected", expected_status);
        }
        self.get_url(url).await
    }

    /// A delay-test failure differs from a broken/auth-failed controller. Do not
    /// turn an unavailable controller into a measured zero or failed node.
    pub async fn node_latency_probe(
        &self,
        proxy: &str,
    ) -> (crate::node_test::NodeState, Option<u64>) {
        self.measured_probe(proxy, "https://www.gstatic.com/generate_204", "204", 5000)
            .await
    }

    pub async fn ai_health_probe(&self, proxy: &str) -> (crate::node_test::NodeState, Option<u64>) {
        self.measured_probe(proxy, "https://api.openai.com/v1/models", "401", 8000)
            .await
    }

    async fn measured_probe(
        &self,
        proxy: &str,
        target: &str,
        expected: &str,
        timeout_ms: u64,
    ) -> (crate::node_test::NodeState, Option<u64>) {
        use crate::node_test::NodeState;
        let Ok(mut url) = self.path_url(&["proxies", proxy, "delay"]) else {
            return (NodeState::Unavailable, None);
        };
        url.query_pairs_mut()
            .append_pair("url", target)
            .append_pair("timeout", &timeout_ms.to_string())
            .append_pair("expected", expected);
        let response = self
            .client
            .get(url)
            .bearer_auth(&self.secret)
            .timeout(Duration::from_millis(timeout_ms + 1000))
            .send()
            .await;
        match response {
            Ok(response) => match response.status().as_u16() {
                200 => match response
                    .json::<Value>()
                    .await
                    .ok()
                    .and_then(|v| v["delay"].as_u64())
                    .filter(|d| *d > 0 && *d <= timeout_ms)
                {
                    Some(delay) => (NodeState::Passed, Some(delay)),
                    _ => (NodeState::Unavailable, None),
                },
                504 => (NodeState::Timeout, None),
                503 => (NodeState::Failed, None),
                _ => (NodeState::Unavailable, None),
            },
            // A controller transport timeout is not proof of a node timeout.
            Err(_) => (NodeState::Unavailable, None),
        }
    }

    pub async fn health_probe(
        &self,
        proxy: &str,
        test_url: &str,
        expected: Option<&str>,
    ) -> DelayProbe {
        let Ok(mut url) = self.path_url(&["proxies", proxy, "delay"]) else {
            return DelayProbe::Unknown;
        };
        url.query_pairs_mut()
            .append_pair("url", test_url)
            .append_pair("timeout", "5000");
        if let Some(expected) = expected {
            url.query_pairs_mut().append_pair("expected", expected);
        }
        let Ok(response) = self
            .client
            .get(url)
            .bearer_auth(&self.secret)
            .timeout(Duration::from_secs(7))
            .send()
            .await
        else {
            return DelayProbe::Unknown;
        };
        match response.status().as_u16() {
            200 => match response
                .json::<Value>()
                .await
                .ok()
                .and_then(|v| v["delay"].as_u64())
                .filter(|v| *v > 0 && *v <= u16::MAX as u64)
            {
                Some(delay) => DelayProbe::Passed(delay),
                None => DelayProbe::Unknown,
            },
            503 | 504 => DelayProbe::Failed,
            _ => DelayProbe::Unknown,
        }
    }

    pub async fn group_delay(
        &self,
        group: &str,
        test_url: &str,
        timeout_ms: u32,
        expected_status: Option<&str>,
    ) -> AppResult<Value> {
        let mut url = self.path_url(&["group", group, "delay"])?;
        url.query_pairs_mut()
            .append_pair("url", test_url)
            .append_pair("timeout", &timeout_ms.clamp(1000, 30_000).to_string());
        if let Some(expected_status) = expected_status {
            url.query_pairs_mut()
                .append_pair("expected", expected_status);
        }
        self.get_url(url).await
    }

    pub async fn reload_config(&self, payload: &str) -> AppResult<()> {
        let mut url = self.path_url(&["configs"])?;
        url.query_pairs_mut().append_pair("force", "true");
        let response = self
            .client
            .put(url)
            .bearer_auth(&self.secret)
            .json(&json!({ "path": "", "payload": payload }))
            .send()
            .await
            .map_err(|error| AppError::Runtime(error.to_string()))?;
        ensure_success(response.status())
    }

    pub async fn close_connection(&self, connection_id: &str) -> AppResult<()> {
        let url = self.path_url(&["connections", connection_id])?;
        let response = self
            .client
            .delete(url)
            .bearer_auth(&self.secret)
            .send()
            .await
            .map_err(|error| AppError::Runtime(error.to_string()))?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(AppError::Runtime(format!(
                "Mihomo API HTTP {}",
                response.status().as_u16()
            )))
        }
    }

    pub async fn set_mode(&self, mode: RoutingMode) -> AppResult<()> {
        let url = self.path_url(&["configs"])?;
        let response = self
            .client
            .patch(url)
            .bearer_auth(&self.secret)
            .json(&json!({ "mode": mode.as_mihomo_mode() }))
            .send()
            .await
            .map_err(|error| AppError::Runtime(error.to_string()))?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(AppError::Runtime(format!(
                "Mihomo API HTTP {}",
                response.status().as_u16()
            )))
        }
    }

    async fn get(&self, path: &str) -> AppResult<Value> {
        let url = self.path_url(&[path])?;
        self.get_url(url).await
    }

    async fn get_url(&self, url: Url) -> AppResult<Value> {
        let response = self
            .client
            .get(url)
            .bearer_auth(&self.secret)
            .send()
            .await
            .map_err(|error| AppError::Runtime(error.to_string()))?;
        let status = response.status();
        if !status.is_success() {
            return Err(AppError::Runtime(format!(
                "Mihomo API HTTP {}",
                status.as_u16()
            )));
        }
        response
            .json()
            .await
            .map_err(|error| AppError::Runtime(error.to_string()))
    }

    fn path_url(&self, segments: &[&str]) -> AppResult<Url> {
        let mut url = self.base_url.clone();
        {
            let mut path = url
                .path_segments_mut()
                .map_err(|_| AppError::Runtime("无效 Mihomo API URL".to_string()))?;
            path.clear();
            for segment in segments {
                path.push(segment);
            }
        }
        Ok(url)
    }
}

fn ensure_success(status: reqwest::StatusCode) -> AppResult<()> {
    if status.is_success() {
        Ok(())
    } else {
        Err(AppError::Runtime(format!(
            "Mihomo API HTTP {}",
            status.as_u16()
        )))
    }
}

#[cfg(test)]
mod readiness_deadline_tests {
    use super::*;
    #[tokio::test]
    async fn readiness_deadline_bounds_a_silent_controller_request() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client =
            MihomoApiClient::from_endpoint(listener.local_addr().unwrap().port(), "fixture".into())
                .unwrap();
        let server = tokio::spawn(async move {
            let (_socket, _) = listener.accept().await.unwrap();
            std::future::pending::<()>().await;
        });
        let started = std::time::Instant::now();
        assert!(client.wait_ready(Duration::from_millis(80)).await.is_err());
        assert!(
            started.elapsed() < Duration::from_millis(800),
            "must not inherit the 10s per-request timeout"
        );
        server.abort();
    }
}

#[cfg(test)]
mod node_latency_tests {
    use super::*;
    use crate::node_test::NodeState;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    #[tokio::test]
    async fn ai_health_uses_expected_401_not_controller_auth_status() {
        for (status, body, want) in [
            (200, r#"{"delay":75}"#, (NodeState::Passed, Some(75))),
            (200, r#"{"delay":0}"#, (NodeState::Unavailable, None)),
            (200, r#"{"delay":8001}"#, (NodeState::Unavailable, None)),
            (401, "{}", (NodeState::Unavailable, None)),
            (503, "{}", (NodeState::Failed, None)),
            (504, "{}", (NodeState::Timeout, None)),
        ] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let client = MihomoApiClient::from_endpoint(
                listener.local_addr().unwrap().port(),
                "fixture".into(),
            )
            .unwrap();
            let server = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut data = vec![0; 8192];
                let count = socket.read(&mut data).await.unwrap();
                let request = String::from_utf8_lossy(&data[..count]);
                assert!(request.starts_with("GET /proxies/A/delay?"));
                assert!(request.contains("timeout=8000") && request.contains("expected=401"));
                assert!(request.contains("api.openai.com"));
                socket.write_all(format!("HTTP/1.1 {status} Fixture\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
            });
            assert_eq!(client.ai_health_probe("A").await, want);
            server.await.unwrap();
        }
    }
    #[tokio::test]
    async fn latency_protocol_keeps_unknown_distinct_and_uses_only_encoded_get() {
        for (status, body, want) in [
            (200, r#"{"delay":42}"#, (NodeState::Passed, Some(42))),
            (200, r#"{"delay":0}"#, (NodeState::Unavailable, None)),
            (200, r#"{"delay":5001}"#, (NodeState::Unavailable, None)),
            (200, r#"{"delay":-1}"#, (NodeState::Unavailable, None)),
            (200, "not-json", (NodeState::Unavailable, None)),
            (401, "{}", (NodeState::Unavailable, None)),
            (503, "{}", (NodeState::Failed, None)),
            (504, "{}", (NodeState::Timeout, None)),
        ] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            let server = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut data = vec![0; 8192];
                let count = socket.read(&mut data).await.unwrap();
                let request = String::from_utf8_lossy(&data[..count]);
                assert!(request.starts_with("GET /proxies/A%2FB%20%3F/delay?"));
                assert!(request.contains("timeout=5000"));
                assert!(request.contains("expected=204"));
                assert!(request
                    .to_lowercase()
                    .contains("authorization: bearer fixture"));
                socket.write_all(format!("HTTP/1.1 {status} Fixture\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
            });
            let client = MihomoApiClient::from_endpoint(port, "fixture".into()).unwrap();
            assert_eq!(client.node_latency_probe("A/B ?").await, want);
            server.await.unwrap();
        }
    }
}

#[cfg(test)]
mod routing_verification_tests {
    use super::*;
    #[test]
    fn routing_readback_requires_mode_count_order_domains_and_targets() {
        let expected = RuleExpectation::from_yaml("mode: rule\nrules: ['DOMAIN-WILDCARD,*.example.com,Exit','DOMAIN,example.com,Exit','MATCH,DIRECT']").unwrap();
        let actual = json!({"rules":[{"type":"DomainWildcard","payload":"*.example.com","proxy":"Exit"},{"type":"Domain","payload":"example.com","proxy":"Exit"},{"type":"Match","payload":"","proxy":"DIRECT"}]});
        expected.check(&json!({"mode":"rule"}), &actual).unwrap();
        assert!(expected.check(&json!({"mode":"global"}), &actual).is_err());
        for field in ["type", "payload", "proxy"] {
            let mut wrong = actual.clone();
            wrong["rules"][0][field] = json!("wrong");
            assert!(expected.check(&json!({"mode":"rule"}), &wrong).is_err());
        }
        let mut stale = actual.clone();
        stale["rules"].as_array_mut().unwrap().pop();
        assert!(expected.check(&json!({"mode":"rule"}), &stale).is_err());
        let mut reordered = actual.clone();
        reordered["rules"].as_array_mut().unwrap().swap(0, 1);
        assert!(expected.check(&json!({"mode":"rule"}), &reordered).is_err());
        let off = RuleExpectation::from_yaml("mode: rule\nrules: ['MATCH,DIRECT']").unwrap();
        assert!(off.check(&json!({"mode":"rule"}), &actual).is_err());
    }
    #[tokio::test]
    async fn routing_readback_connection_failure_is_not_success() {
        let socket = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = socket.local_addr().unwrap().port();
        drop(socket);
        let api = MihomoApiClient::from_endpoint(port, "fixture".into()).unwrap();
        assert!(api
            .verify_rule_config("mode: rule\nrules: []")
            .await
            .is_err());
    }
}
