use crate::models::AppSettings;
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticStage {
    LocalProxy,
    Controller,
    HttpProxy,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticCheck {
    pub stage: String,
    pub success: bool,
    pub latency_ms: Option<u128>,
    pub detail: String,
}

pub async fn run(settings: &AppSettings) -> Vec<DiagnosticCheck> {
    vec![
        run_step(settings, DiagnosticStage::LocalProxy).await,
        run_step(settings, DiagnosticStage::Controller).await,
        run_step(settings, DiagnosticStage::HttpProxy).await,
    ]
}

/// One bounded, read-only probe per IPC lets the UI display actual progress.
pub async fn run_step(settings: &AppSettings, stage: DiagnosticStage) -> DiagnosticCheck {
    match stage {
        DiagnosticStage::LocalProxy => {
            check_tcp_port("local_proxy", settings.mixed_port, Duration::from_secs(2)).await
        }
        DiagnosticStage::Controller => {
            check_tcp_port(
                "controller",
                settings.controller_port,
                Duration::from_secs(2),
            )
            .await
        }
        DiagnosticStage::HttpProxy => check_http_proxy(settings).await,
    }
}

async fn check_tcp_port(stage: &str, port: u16, timeout: Duration) -> DiagnosticCheck {
    let started = Instant::now();
    let address = SocketAddr::from(([127, 0, 0, 1], port));
    match tokio::time::timeout(timeout, tokio::net::TcpStream::connect(address)).await {
        Ok(Ok(_)) => DiagnosticCheck {
            stage: stage.to_string(),
            success: true,
            latency_ms: Some(started.elapsed().as_millis()),
            detail: format!("127.0.0.1:{port} 可连接"),
        },
        Ok(Err(error)) => DiagnosticCheck {
            stage: stage.to_string(),
            success: false,
            latency_ms: None,
            detail: error.to_string(),
        },
        Err(_) => DiagnosticCheck {
            stage: stage.to_string(),
            success: false,
            latency_ms: Some(started.elapsed().as_millis()),
            detail: "本地端口连接超时".to_string(),
        },
    }
}

async fn check_http_proxy(settings: &AppSettings) -> DiagnosticCheck {
    let started = Instant::now();
    let proxy = match reqwest::Proxy::all(format!("http://127.0.0.1:{}", settings.mixed_port)) {
        Ok(proxy) => proxy,
        Err(error) => {
            return DiagnosticCheck {
                stage: "http_proxy".to_string(),
                success: false,
                latency_ms: None,
                detail: error.to_string(),
            };
        }
    };
    let client = match reqwest::Client::builder()
        .proxy(proxy)
        .timeout(Duration::from_secs(10))
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            return DiagnosticCheck {
                stage: "http_proxy".to_string(),
                success: false,
                latency_ms: None,
                detail: error.to_string(),
            };
        }
    };
    match client
        .get("https://www.gstatic.com/generate_204")
        .send()
        .await
    {
        Ok(response) => DiagnosticCheck {
            stage: "http_proxy".to_string(),
            success: response.status().is_success(),
            latency_ms: Some(started.elapsed().as_millis()),
            detail: format!("HTTP {}", response.status().as_u16()),
        },
        Err(error) => DiagnosticCheck {
            stage: "http_proxy".to_string(),
            success: false,
            latency_ms: Some(started.elapsed().as_millis()),
            detail: error.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[test]
    fn stage_input_only_accepts_the_three_known_probes() {
        for stage in ["local_proxy", "controller", "http_proxy"] {
            assert!(serde_json::from_value::<DiagnosticStage>(serde_json::json!(stage)).is_ok());
        }
        for stage in ["all", "unknown", "https://example.invalid/"] {
            assert!(serde_json::from_value::<DiagnosticStage>(serde_json::json!(stage)).is_err());
        }
    }

    #[tokio::test]
    async fn port_steps_probe_only_the_selected_configured_port() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        for (stage, mixed_port, controller_port, expected) in [
            (DiagnosticStage::LocalProxy, port, 0, "local_proxy"),
            (DiagnosticStage::Controller, 0, port, "controller"),
        ] {
            let settings = AppSettings {
                mixed_port,
                controller_port,
                ..AppSettings::default()
            };
            let check = run_step(&settings, stage).await;
            assert!(check.success, "{}", check.detail);
            assert_eq!(check.stage, expected);
            assert!(check.latency_ms.is_some());
        }
        let failed = run_step(
            &AppSettings {
                mixed_port: 0,
                ..AppSettings::default()
            },
            DiagnosticStage::LocalProxy,
        )
        .await;
        assert!(!failed.success);
        assert_eq!(failed.stage, "local_proxy");
    }

    #[tokio::test]
    async fn proxy_step_uses_the_local_proxy_and_reports_its_failure_independently() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let peer = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = [0; 4096];
            let count = socket.read(&mut bytes).await.unwrap();
            let request = String::from_utf8_lossy(&bytes[..count]);
            assert!(request.starts_with("CONNECT www.gstatic.com:443 "));
            socket
                .write_all(
                    b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .await
                .unwrap();
        });
        let settings = AppSettings {
            mixed_port: port,
            controller_port: 0,
            ..AppSettings::default()
        };
        let check = tokio::time::timeout(
            Duration::from_secs(3),
            run_step(&settings, DiagnosticStage::HttpProxy),
        )
        .await
        .unwrap();
        assert_eq!(check.stage, "http_proxy");
        assert!(!check.success);
        assert!(check.latency_ms.is_some());
        peer.await.unwrap();
    }
}
