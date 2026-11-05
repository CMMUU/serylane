use crate::error::{AppError, AppResult};
use crate::mihomo_api::MihomoApiClient;
use crate::storage::AppStorage;
use serde::Serialize;
use serde_json::Value as JsonValue;
use serde_yaml::{Mapping, Value as YamlValue};
use std::collections::HashSet;
use std::time::Duration;
use tauri::AppHandle;

const MAX_ROUTE_DEPTH: usize = 16;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct NodeDelaySample {
    pub time: Option<String>,
    pub delay_ms: u32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CurrentNodeDetails {
    pub provider_details_available: bool,
    pub group: String,
    pub node_name: String,
    pub route_chain: Vec<String>,
    pub node_type: String,
    pub alive: Option<bool>,
    pub udp: Option<bool>,
    pub uot: Option<bool>,
    pub xudp: Option<bool>,
    pub tfo: Option<bool>,
    pub mptcp: Option<bool>,
    pub smux: Option<bool>,
    pub provider_name: Option<String>,
    pub masked_server: Option<String>,
    pub port: Option<u16>,
    pub network: Option<String>,
    pub tls: Option<String>,
    pub dialer_proxy: Option<String>,
    pub interface: Option<String>,
    pub history: Vec<NodeDelaySample>,
    pub last_delay_ms: Option<u32>,
}

pub async fn get_current_node_details(
    app: &AppHandle,
    group: String,
) -> AppResult<CurrentNodeDetails> {
    let group = group.trim();
    if group.is_empty() || group.chars().count() > 256 {
        return Err(AppError::InvalidInput("代理组名称无效".to_string()));
    }

    let storage = AppStorage::from_app(app)?;
    let settings = storage.settings()?;
    let state = storage.state()?;
    let profile_id = state
        .active_profile_id
        .ok_or_else(|| AppError::NotFound("没有活动配置".to_string()))?;
    let revision_id = state
        .active_revision_id
        .ok_or_else(|| AppError::NotFound("没有活动配置版本".to_string()))?;
    let source = storage.load_revision_source(profile_id, revision_id)?;

    let api = MihomoApiClient::new(&settings)?;
    let (proxies, providers) = read_detail_payloads(api.proxies(), api.proxy_providers()).await?;
    let mut details = build_current_node_details(group, &proxies, &providers, &source)?;
    details.provider_details_available = !providers.is_null();
    Ok(details)
}

async fn read_detail_payloads(
    proxies: impl std::future::Future<Output = AppResult<JsonValue>>,
    providers: impl std::future::Future<Output = AppResult<JsonValue>>,
) -> AppResult<(JsonValue, JsonValue)> {
    // Optional provider metadata must not add another 10-second controller
    // timeout to the current-node interaction. Dropping timeout futures cancels
    // the HTTP reads; no detached retries or node changes are performed.
    let (proxies, providers) = tokio::join!(
        tokio::time::timeout(Duration::from_secs(4), proxies),
        tokio::time::timeout(Duration::from_millis(300), providers),
    );
    let proxies = proxies
        .map_err(|_| AppError::Runtime("节点信息读取超时，请重试；代理保持运行".into()))??;
    Ok((
        proxies,
        providers
            .ok()
            .and_then(Result::ok)
            .unwrap_or(JsonValue::Null),
    ))
}

fn build_current_node_details(
    group: &str,
    proxies_payload: &JsonValue,
    providers_payload: &JsonValue,
    source: &str,
) -> AppResult<CurrentNodeDetails> {
    let proxies = proxies_payload
        .get("proxies")
        .and_then(JsonValue::as_object)
        .ok_or_else(|| AppError::Runtime("Mihomo 没有返回代理列表".to_string()))?;
    if !proxies.contains_key(group) {
        return Err(AppError::NotFound(format!("代理组 {group}")));
    }

    let route_chain = resolve_route(proxies, group);
    let node_name = route_chain
        .last()
        .cloned()
        .ok_or_else(|| AppError::Runtime("当前代理链为空".to_string()))?;
    let runtime_node = proxies
        .get(&node_name)
        .ok_or_else(|| AppError::NotFound(format!("节点 {node_name}")))?;
    let source_node = find_source_node(source, &node_name)?;
    let history = collect_history(runtime_node);
    let last_delay_ms = history
        .iter()
        .rev()
        .find(|sample| sample.delay_ms > 0)
        .map(|sample| sample.delay_ms);
    let runtime_type = json_string(runtime_node, "type");
    let source_type = source_node
        .as_ref()
        .and_then(|node| yaml_string(node, "type"));
    let provider_name = non_empty_json_string(runtime_node, "provider-name")
        .or_else(|| find_provider_name(providers_payload, &node_name));
    let masked_server = source_node
        .as_ref()
        .and_then(|node| yaml_string(node, "server"))
        .map(|server| mask_host(&server));
    let port = source_node
        .as_ref()
        .and_then(|node| yaml_port(node, "port"));
    let network = source_node
        .as_ref()
        .and_then(|node| yaml_string(node, "network"));
    let tls = source_node.as_ref().map(tls_label);

    Ok(CurrentNodeDetails {
        provider_details_available: !providers_payload.is_null(),
        group: group.to_string(),
        node_name,
        route_chain,
        node_type: runtime_type
            .or(source_type)
            .unwrap_or_else(|| "Unknown".to_string()),
        alive: json_bool(runtime_node, "alive"),
        udp: json_bool(runtime_node, "udp"),
        uot: json_bool(runtime_node, "uot"),
        xudp: json_bool(runtime_node, "xudp"),
        tfo: json_bool(runtime_node, "tfo"),
        mptcp: json_bool(runtime_node, "mptcp"),
        smux: json_bool(runtime_node, "smux"),
        provider_name,
        masked_server,
        port,
        network,
        tls,
        dialer_proxy: non_empty_json_string(runtime_node, "dialer-proxy"),
        interface: non_empty_json_string(runtime_node, "interface"),
        history,
        last_delay_ms,
    })
}

fn resolve_route(proxies: &serde_json::Map<String, JsonValue>, group: &str) -> Vec<String> {
    let mut route = Vec::new();
    let mut seen = HashSet::new();
    let mut current = group.to_string();
    for _ in 0..MAX_ROUTE_DEPTH {
        if !seen.insert(current.clone()) {
            break;
        }
        route.push(current.clone());
        let Some(next) = proxies
            .get(&current)
            .and_then(|value| value.get("now"))
            .and_then(JsonValue::as_str)
            .filter(|value| !value.trim().is_empty() && *value != current)
        else {
            break;
        };
        current = next.to_string();
    }
    route
}

fn collect_history(node: &JsonValue) -> Vec<NodeDelaySample> {
    let mut samples = Vec::new();
    append_history(node.get("history"), &mut samples);
    if let Some(extra) = node.get("extra").and_then(JsonValue::as_object) {
        for value in extra.values() {
            append_history(value.get("history"), &mut samples);
        }
    }
    samples.sort_by(|left, right| left.time.cmp(&right.time));
    samples.dedup();
    if samples.len() > 10 {
        samples.drain(..samples.len() - 10);
    }
    samples
}

fn append_history(value: Option<&JsonValue>, samples: &mut Vec<NodeDelaySample>) {
    let Some(history) = value.and_then(JsonValue::as_array) else {
        return;
    };
    for entry in history {
        let Some(delay) = entry.get("delay").and_then(JsonValue::as_u64) else {
            continue;
        };
        samples.push(NodeDelaySample {
            time: entry
                .get("time")
                .and_then(JsonValue::as_str)
                .map(str::to_string),
            delay_ms: delay.min(u32::MAX as u64) as u32,
        });
    }
}

fn find_provider_name(payload: &JsonValue, node_name: &str) -> Option<String> {
    let providers = payload.get("providers")?.as_object()?;
    providers
        .iter()
        .filter(|(name, _)| name.as_str() != "default")
        .find_map(|(name, provider)| {
            provider
                .get("proxies")
                .and_then(JsonValue::as_array)
                .filter(|proxies| {
                    proxies.iter().any(|proxy| {
                        proxy.get("name").and_then(JsonValue::as_str) == Some(node_name)
                    })
                })
                .map(|_| name.clone())
        })
}

fn find_source_node(source: &str, node_name: &str) -> AppResult<Option<Mapping>> {
    let document: YamlValue = serde_yaml::from_str(source)
        .map_err(|error| AppError::Config(format!("Mihomo YAML 解析失败: {error}")))?;
    let Some(root) = document.as_mapping() else {
        return Ok(None);
    };
    let Some(proxies) = root
        .get(YamlValue::String("proxies".to_string()))
        .and_then(YamlValue::as_sequence)
    else {
        return Ok(None);
    };
    Ok(proxies.iter().find_map(|proxy| {
        let mapping = proxy.as_mapping()?;
        (yaml_string(mapping, "name").as_deref() == Some(node_name)).then(|| mapping.clone())
    }))
}

fn json_string(value: &JsonValue, key: &str) -> Option<String> {
    value.get(key)?.as_str().map(str::to_string)
}

fn non_empty_json_string(value: &JsonValue, key: &str) -> Option<String> {
    json_string(value, key).filter(|value| !value.trim().is_empty())
}

fn json_bool(value: &JsonValue, key: &str) -> Option<bool> {
    value.get(key)?.as_bool()
}

fn yaml_string(mapping: &Mapping, key: &str) -> Option<String> {
    let value = mapping.get(YamlValue::String(key.to_string()))?;
    value
        .as_str()
        .map(str::to_string)
        .or_else(|| value.as_i64().map(|value| value.to_string()))
}

fn yaml_port(mapping: &Mapping, key: &str) -> Option<u16> {
    let value = mapping.get(YamlValue::String(key.to_string()))?;
    value
        .as_u64()
        .and_then(|value| u16::try_from(value).ok())
        .or_else(|| value.as_str()?.parse().ok())
}

fn tls_label(mapping: &Mapping) -> String {
    if mapping.contains_key(YamlValue::String("reality-opts".to_string())) {
        return "Reality".to_string();
    }
    if yaml_string(mapping, "type").is_some_and(|value| {
        matches!(
            value.to_ascii_lowercase().as_str(),
            "trojan" | "hysteria" | "hysteria2" | "tuic" | "anytls"
        )
    }) {
        return "TLS".to_string();
    }
    match mapping
        .get(YamlValue::String("tls".to_string()))
        .and_then(YamlValue::as_bool)
    {
        Some(true) => "TLS".to_string(),
        Some(false) => "关闭".to_string(),
        None => "未声明".to_string(),
    }
}

fn mask_host(host: &str) -> String {
    let host = host.trim();
    if let Ok(address) = host.parse::<std::net::IpAddr>() {
        return match address {
            std::net::IpAddr::V4(address) => {
                let octets = address.octets();
                format!("{}.{}.•••.•••", octets[0], octets[1])
            }
            std::net::IpAddr::V6(address) => {
                let segments = address.segments();
                format!("{:x}:{:x}:••••", segments[0], segments[1])
            }
        };
    }
    let labels = host.split('.').collect::<Vec<_>>();
    if labels.len() >= 3 {
        let prefix = labels[0].chars().take(2).collect::<String>();
        format!("{prefix}••••.{}", labels[labels.len() - 2..].join("."))
    } else {
        let prefix = host.chars().take(2).collect::<String>();
        format!("{prefix}••••")
    }
}

#[cfg(test)]
mod tests {
    use super::{build_current_node_details, mask_host, resolve_route, tls_label};
    use serde_json::json;
    use serde_yaml::Value as YamlValue;

    #[test]
    fn resolves_nested_selector_route_without_looping() {
        let payload = json!({
            "GROUP": {"now": "AUTO"},
            "AUTO": {"now": "NODE"},
            "NODE": {"type": "Vless"}
        });
        let route = resolve_route(payload.as_object().expect("object"), "GROUP");
        assert_eq!(route, vec!["GROUP", "AUTO", "NODE"]);
    }

    #[test]
    fn returns_only_whitelisted_masked_node_fields() {
        let proxies = json!({"proxies": {
            "GROUP": {"type":"Selector","now":"NODE","alive":true},
            "NODE": {
                "type":"Vless","alive":true,"udp":true,
                "history":[{"time":"2026-08-19T10:00:00Z","delay":86}]
            }
        }});
        let providers = json!({"providers": {
            "subscription": {"proxies":[{"name":"NODE"}]}
        }});
        let source = r#"
proxies:
  - name: NODE
    type: vless
    server: sg01.example.net
    port: 443
    uuid: SECRET
    network: tcp
    tls: true
"#;
        let details =
            build_current_node_details("GROUP", &proxies, &providers, source).expect("details");
        assert_eq!(details.route_chain, vec!["GROUP", "NODE"]);
        assert_eq!(details.masked_server.as_deref(), Some("sg••••.example.net"));
        assert_eq!(details.port, Some(443));
        assert_eq!(details.tls.as_deref(), Some("TLS"));
        assert_eq!(details.last_delay_ms, Some(86));
        assert_eq!(details.provider_name.as_deref(), Some("subscription"));
    }

    #[test]
    fn masks_ip_and_domain_hosts() {
        assert_eq!(mask_host("192.168.10.20"), "192.168.•••.•••");
        assert_eq!(mask_host("edge.example.com"), "ed••••.example.com");
        assert_eq!(mask_host("example.com"), "ex••••");
    }

    #[test]
    fn recognizes_protocols_with_implicit_tls() {
        let value: YamlValue =
            serde_yaml::from_str("type: trojan\nserver: example.com\n").expect("yaml");
        assert_eq!(tls_label(value.as_mapping().expect("mapping")), "TLS");
    }
}

#[cfg(test)]
mod interaction_budget_tests {
    use super::*;
    #[tokio::test]
    async fn optional_provider_timeout_does_not_hide_available_node_data() {
        let result = read_detail_payloads(
            async { Ok(serde_json::json!({"proxies": {}})) },
            std::future::pending(),
        )
        .await
        .unwrap();
        assert_eq!(result.0, serde_json::json!({"proxies": {}}));
        assert!(result.1.is_null());
    }
    #[tokio::test]
    async fn essential_failure_is_not_reported_as_empty_success() {
        let result = read_detail_payloads(
            async { Err(AppError::Runtime("controller unavailable".into())) },
            async { Ok(JsonValue::Null) },
        )
        .await;
        assert!(result.is_err());
    }
    #[tokio::test]
    async fn available_provider_metadata_is_preserved() {
        let (_, providers) = read_detail_payloads(async { Ok(serde_json::json!({})) }, async {
            Ok(serde_json::json!({"providers": {}}))
        })
        .await
        .unwrap();
        assert!(!providers.is_null());
    }
}
