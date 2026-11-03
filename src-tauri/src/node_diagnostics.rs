//! Probes one explicitly selected, currently loaded node. No selector writes,
//! interface overrides, credentials in errors, or synthetic UDP success.
use crate::{
    diagnostics::DiagnosticCheck,
    error::{AppError, AppResult},
    mihomo_api::MihomoApiClient,
    models::RuntimePhase,
    network_context,
    node_test::NodeState,
    runtime::MihomoRuntime,
    storage::AppStorage,
};
use serde_yaml::Value;
use std::{net::IpAddr, sync::OnceLock, time::Duration};
use tauri::{AppHandle, Manager};

fn result(stage: &str, outcome: &str, detail: &str, latency: Option<u128>) -> DiagnosticCheck {
    DiagnosticCheck {
        stage: stage.into(),
        success: outcome == "pass",
        latency_ms: latency,
        detail: detail.into(),
        outcome: Some(outcome.into()),
    }
}
#[derive(Debug)]
struct Endpoint {
    host: String,
    port: u16,
    tcp: bool,
    overridden: bool,
}
fn endpoint(source: &str, name: &str) -> Option<Endpoint> {
    let root: Value = serde_yaml::from_str(source).ok()?;
    let node = root["proxies"]
        .as_sequence()?
        .iter()
        .find(|n| n["name"].as_str() == Some(name))?;
    let host = node["server"]
        .as_str()?
        .trim_matches(['[', ']'])
        .to_string();
    if host.is_empty() || host.len() > 253 || host.contains(['/', '@', '\n', '\r', ' ']) {
        return None;
    }
    let port = node["port"]
        .as_u64()
        .or_else(|| node["port"].as_str()?.parse().ok())?;
    let port = u16::try_from(port).ok().filter(|p| *p > 0)?;
    // Unknown/UDP/native multiplexed transports are checked by the core only.
    let tcp = matches!(
        node["type"].as_str(),
        Some("ss" | "ssr" | "socks5" | "http" | "trojan" | "vmess" | "vless" | "anytls")
    ) && matches!(
        node["network"].as_str(),
        None | Some("tcp" | "ws" | "http" | "h2" | "grpc")
    ) && node["plugin"].is_null();
    let overridden = ["interface-name", "dialer-proxy", "routing-mark"]
        .iter()
        .any(|key| !node[*key].is_null() || !root[*key].is_null());
    Some(Endpoint {
        host,
        port,
        tcp,
        overridden,
    })
}
fn loaded_leaf(payload: &serde_json::Value, name: &str) -> bool {
    let n = &payload["proxies"][name];
    n.is_object()
        && !n["all"].is_array()
        && n["type"].as_str().is_some_and(|t| {
            !matches!(
                t,
                "Direct" | "Reject" | "RejectDrop" | "Pass" | "Compatible"
            )
        })
}
pub async fn run(app: &AppHandle, stage: &str, node: Option<&str>) -> AppResult<DiagnosticCheck> {
    tokio::time::timeout(Duration::from_secs(14), run_inner(app, stage, node))
        .await
        .unwrap_or_else(|_| {
            Ok(result(
                stage,
                "warn",
                "本项达到总时限，结果未采用；保持原出口，请稍后重试。",
                None,
            ))
        })
}
async fn run_inner(app: &AppHandle, stage: &str, node: Option<&str>) -> AppResult<DiagnosticCheck> {
    static SLOTS: OnceLock<tokio::sync::Semaphore> = OnceLock::new();
    let _slot = SLOTS
        .get_or_init(|| tokio::sync::Semaphore::new(2))
        .try_acquire()
        .map_err(|_| AppError::Conflict("诊断正在进行，请等待当前请求结束".into()))?;
    let node = match node.filter(|n| !n.is_empty() && n.len() <= 1024) {
        Some(n) => n,
        None => {
            return Ok(result(
                stage,
                "skipped",
                "请先选择要排查的节点；诊断不会改选实际出口。",
                None,
            ))
        }
    };
    let before = network_context::refresh(app).await;
    let app_copy = app.clone();
    let (settings, source, identity) = crate::ui_read::run(move || {
        let storage = AppStorage::from_app(&app_copy).map_err(|e| e.dto())?;
        let state = storage.state().map_err(|e| e.dto())?;
        let runtime = app_copy.state::<MihomoRuntime>().status(Some(&app_copy));
        if runtime.phase != RuntimePhase::Running {
            return Err(AppError::Conflict("请先启动核心；诊断不会自动开启代理".into()).dto());
        }
        let (Some(p), Some(r)) = (state.active_profile_id, state.active_revision_id) else {
            return Err(AppError::Conflict("请先选用配置".into()).dto());
        };
        Ok((
            storage.settings().map_err(|e| e.dto())?,
            storage.load_revision_source(p, r).map_err(|e| e.dto())?,
            (p, r, runtime.pid, runtime.started_at),
        ))
    })
    .await
    .map_err(|_| AppError::Conflict("配置或核心状态读取未完成，请刷新后诊断".into()))?;
    let api = MihomoApiClient::new(&settings)?;
    let started = std::time::Instant::now();
    let work = async {
        let proxies = api.proxies().await?;
        if !loaded_leaf(&proxies, node) {
            return Ok::<DiagnosticCheck, AppError>(result(
                stage,
                "skipped",
                "节点已变化或尚未由提供器加载，请刷新节点列表。",
                None,
            ));
        }
        if stage == "node_request" {
            let (state, delay) = api.node_latency_probe(node).await;
            let (outcome, detail) = match state {
                NodeState::Passed => ("pass", "核心使用该节点的实际协议完成测试请求；不代表账号、模型会话或长期连接已验证。"),
                NodeState::Failed | NodeState::Timeout => ("fail", "该节点的实际协议请求未通过；结合 DNS、出口节点及传输检查定位，不将节点永久判为不可用。"),
                _ => ("warn", "控制接口未返回有效测量结果；尚未确认该节点失败。"),
            };
            return Ok(result(stage, outcome, detail, delay.map(u128::from)));
        }
        let Some(ep) = endpoint(&source, node) else {
            return Ok(result(
                stage,
                "skipped",
                "节点由提供器或特殊配置提供，未取得可确认的服务器信息；请看实际协议请求结果。",
                None,
            ));
        };
        if ep.overridden {
            return Ok(result(stage, "skipped", "订阅有显式网卡、拨号链或路由标记；系统直拨探测不代表该路径，保留原设置并以核心协议检查为准。", None));
        }
        if stage == "node_transport" && !ep.tcp {
            return Ok(result(stage, "skipped", "该节点使用 UDP 或未确认的外层协议，不用 TCP/普通 UDP 发包冒充握手；请看实际协议请求结果。", None));
        }
        let addresses = if let Ok(ip) = ep.host.parse::<IpAddr>() {
            vec![std::net::SocketAddr::new(ip, ep.port)]
        } else {
            match resolve_host(ep.host.clone(), ep.port).await {
                Some(addresses) => addresses,
                _ => return Ok(result(stage, "fail", "节点地址经系统 DNS 解析未完成；Mihomo 自定义 DNS 可能不同，不据此认定节点失效。", Some(started.elapsed().as_millis()))),
            }
        };
        if addresses.is_empty() {
            return Ok(result(
                stage,
                "fail",
                "系统 DNS 未返回地址；请对照核心实际协议检查。",
                None,
            ));
        }
        if stage == "node_dns" {
            return Ok(result(
                stage,
                "pass",
                "系统解析路径取得节点地址（IP 节点无需 DNS）；未改变 MagicDNS 或 Mihomo DNS。",
                Some(started.elapsed().as_millis()),
            ));
        }
        // Run bounded parallel IPv4/IPv6 attempts, not the first address only.
        use futures_util::{stream, StreamExt};
        let connected = stream::iter(addresses.into_iter().map(|address| async move {
            tokio::time::timeout(
                Duration::from_secs(3),
                tokio::net::TcpStream::connect(address),
            )
            .await
            .is_ok_and(|r| r.is_ok())
        }))
        .buffer_unordered(4)
        .any(std::future::ready)
        .await;
        Ok(result(
            stage,
            if connected { "pass" } else { "fail" },
            if connected {
                "经系统路由建立了节点 TCP 连接；尚未验证代理认证、TLS、长期连接或实际出网接口。"
            } else {
                "经系统路由连接节点 TCP 端口未成功；可能涉及 Exit Node 到节点的链路，请对照实际协议检查。"
            },
            Some(started.elapsed().as_millis()),
        ))
    };
    let outcome = tokio::time::timeout(Duration::from_secs(12), work)
        .await
        .unwrap_or_else(|_| {
            Ok(result(
                stage,
                "warn",
                "达到本项诊断时限，未采用未完成结果；未切换出口。",
                None,
            ))
        })?;
    let after = network_context::refresh(app).await;
    let storage = AppStorage::from_app(app)?;
    let state = storage.state()?;
    let runtime = app.state::<MihomoRuntime>().status(Some(app));
    if before.revision != after.revision
        || after.stale
        || state.active_profile_id != Some(identity.0)
        || state.active_revision_id != Some(identity.1)
        || runtime.pid != identity.2
        || runtime.started_at != identity.3
        || runtime.phase != RuntimePhase::Running
    {
        return Ok(result(
            stage,
            "warn",
            "网络、出口或配置在检查期间变化，本项结果已丢弃，请重新诊断。",
            None,
        ));
    }
    Ok(outcome)
}

async fn resolve_host(host: String, port: u16) -> Option<Vec<std::net::SocketAddr>> {
    use std::{net::ToSocketAddrs, sync::Arc};
    static DNS_SLOTS: OnceLock<Arc<tokio::sync::Semaphore>> = OnceLock::new();
    let permit = DNS_SLOTS
        .get_or_init(|| Arc::new(tokio::sync::Semaphore::new(2)))
        .clone()
        .try_acquire_owned()
        .ok()?;
    // OS resolver calls are not cancellable. Keep the slot until they actually
    // return, even if the UI's three-second budget has already expired.
    let job = tauri::async_runtime::spawn_blocking(move || {
        let _permit = permit;
        (host.as_str(), port)
            .to_socket_addrs()
            .ok()
            .map(|a| a.take(16).collect())
    });
    tokio::time::timeout(Duration::from_secs(3), job)
        .await
        .ok()?
        .ok()?
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explicit_bindings_and_udp_are_not_reported_as_default_tcp_handshake() {
        for kind in ["hysteria2", "tuic", "wireguard", "unknown"] {
            let s =
                format!("proxies: [{{name: A, type: {kind}, server: example.invalid, port: 443}}]");
            assert!(!endpoint(&s, "A").unwrap().tcp);
        }
        let s = "interface-name: fixture0\nproxies: [{name: A, type: trojan, server: example.invalid, port: 443}]";
        assert!(endpoint(s, "A").unwrap().overridden);
        assert!(endpoint(s, "missing").is_none());
        assert!(endpoint(
            "proxies: [{name: A, server: example.invalid, port: 0}]",
            "A"
        )
        .is_none());
    }
    #[test]
    fn only_loaded_real_nodes_can_be_probed() {
        let map = serde_json::json!({"proxies":{"A":{"type":"Trojan"},"G":{"type":"Selector","all":["A"]},"DIRECT":{"type":"Direct"}}});
        assert!(loaded_leaf(&map, "A"));
        for name in ["G", "DIRECT", "missing"] {
            assert!(!loaded_leaf(&map, name));
        }
    }
}
