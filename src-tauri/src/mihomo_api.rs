use crate::error::{AppError, AppResult};
use crate::models::{AppSettings, RoutingMode};
use serde_json::{json, Value};
use std::time::Duration;
use url::Url;

#[derive(Clone)]
pub struct MihomoApiClient {
    client: reqwest::Client,
    base_url: Url,
    secret: String,
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

    pub async fn proxy_providers(&self) -> AppResult<Value> {
        let url = self.path_url(&["providers", "proxies"])?;
        self.get_url(url).await
    }

    pub async fn rules(&self) -> AppResult<Value> {
        self.get("rules").await
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
