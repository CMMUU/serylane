use crate::config::{inspect_profile, is_subscription_metadata_node_name, ProfileSummary};
use crate::error::{AppError, AppResult};
use crate::models::{AppSettings, NetworkMode, OpenAiPolicy, RoutingMode};
use serde_yaml::{Mapping, Number, Value};
use std::collections::BTreeSet;

pub const OPENAI_GROUP_NAME: &str = "🤖 OpenAI 自动灾备";

const OPENAI_RULES: &[(&str, &str)] = &[
    ("DOMAIN-SUFFIX", "openai.com"),
    ("DOMAIN-SUFFIX", "chatgpt.com"),
    ("DOMAIN-SUFFIX", "oaistatic.com"),
    ("DOMAIN-SUFFIX", "oaiusercontent.com"),
    ("DOMAIN-SUFFIX", "oaistatsig.com"),
    ("DOMAIN-SUFFIX", "openaimerge.com"),
    ("DOMAIN", "cdn.workos.com"),
    ("DOMAIN", "forwarder.workos.com"),
    ("DOMAIN", "images.workoscdn.com"),
    ("DOMAIN", "setup.workos.com"),
    ("DOMAIN", "workos.imgix.net"),
    ("DOMAIN", "challenges.cloudflare.com"),
    ("DOMAIN", "js.intercomcdn.com"),
    ("DOMAIN", "js.stripe.com"),
];

#[derive(Debug, Clone)]
pub struct EffectiveConfig {
    pub yaml: String,
    pub summary: ProfileSummary,
}

pub fn build_effective_config(
    source: &str,
    settings: &AppSettings,
    routing_mode: RoutingMode,
) -> AppResult<EffectiveConfig> {
    build_effective_config_with_policy(source, settings, routing_mode, None)
}

pub fn build_effective_config_with_policy(
    source: &str,
    settings: &AppSettings,
    routing_mode: RoutingMode,
    openai_policy: Option<&OpenAiPolicy>,
) -> AppResult<EffectiveConfig> {
    if settings.mixed_port == settings.controller_port {
        return Err(AppError::Config(
            "mixed port 与 controller port 不能相同".to_string(),
        ));
    }
    let mut summary = inspect_profile(source).map_err(AppError::Config)?;
    let mut document: Value =
        serde_yaml::from_str(source).map_err(|error| AppError::Config(error.to_string()))?;
    let root = document
        .as_mapping_mut()
        .ok_or_else(|| AppError::Config("配置根节点必须是 YAML 对象".to_string()))?;

    if root.contains_key(Value::String("script".to_string())) {
        return Err(AppError::Config(
            "MVP 不接受包含 script 的远程配置".to_string(),
        ));
    }

    for controlled in [
        "port",
        "socks-port",
        "redir-port",
        "tproxy-port",
        "external-controller-unix",
        "external-controller-pipe",
        "external-controller-tls",
        "external-controller-cors",
        "external-ui",
        "external-ui-url",
        "authentication",
        "skip-auth-prefixes",
        "lan-allowed-ips",
        "lan-disallowed-ips",
        "listeners",
        "tunnels",
    ] {
        root.remove(Value::String(controlled.to_string()));
    }

    insert(
        root,
        "mixed-port",
        Value::Number(Number::from(settings.mixed_port)),
    );
    insert(root, "allow-lan", Value::Bool(false));
    insert(root, "bind-address", Value::String("127.0.0.1".to_string()));
    insert(
        root,
        "external-controller",
        Value::String(format!("127.0.0.1:{}", settings.controller_port)),
    );
    insert(
        root,
        "secret",
        Value::String(settings.controller_secret.clone()),
    );
    insert(
        root,
        "mode",
        Value::String(routing_mode.as_mihomo_mode().to_string()),
    );
    enable_selection_cache(root)?;
    // Desktop long-lived CONNECT streams should not depend on OS-specific TCP
    // idle defaults. Respect explicit subscription choices, including disable.
    for (key, value) in [("keep-alive-idle", 30), ("keep-alive-interval", 15)] {
        root.entry(Value::String(key.into()))
            .or_insert(Value::Number(Number::from(value)));
    }
    root.entry(Value::String("disable-keep-alive".into()))
        .or_insert(Value::Bool(false));

    root.remove(Value::String("tun".to_string()));
    let mut tun = Mapping::new();
    match settings.network_mode {
        NetworkMode::Tun => {
            insert(&mut tun, "enable", Value::Bool(true));
            insert(&mut tun, "stack", Value::String("mixed".to_string()));
            insert(&mut tun, "auto-route", Value::Bool(true));
            insert(&mut tun, "auto-detect-interface", Value::Bool(true));
            insert(
                &mut tun,
                "dns-hijack",
                Value::Sequence(vec![
                    Value::String("any:53".to_string()),
                    Value::String("tcp://any:53".to_string()),
                ]),
            );
        }
        NetworkMode::Manual | NetworkMode::SystemProxy => {
            insert(&mut tun, "enable", Value::Bool(false));
        }
    }
    insert(root, "tun", Value::Mapping(tun));

    normalize_proxy_groups(root, &mut summary)?;

    if let Some(policy) = openai_policy {
        apply_openai_policy(root, policy, &mut summary)?;
    }

    crate::user_rules::merge_into_config(root, &settings.user_rules, &mut summary)?;

    let yaml =
        serde_yaml::to_string(&document).map_err(|error| AppError::Config(error.to_string()))?;
    Ok(EffectiveConfig { yaml, summary })
}

fn enable_selection_cache(root: &mut Mapping) -> AppResult<()> {
    let profile = root
        .entry(Value::String("profile".to_string()))
        .or_insert_with(|| Value::Mapping(Mapping::new()));
    if profile.is_null() {
        *profile = Value::Mapping(Mapping::new());
    }
    let profile = profile
        .as_mapping_mut()
        .ok_or_else(|| AppError::Config("profile 必须是 YAML 对象".to_string()))?;
    // Mihomo persists API group selections in its stable -d runtime directory.
    // Keep other cache choices (especially store-fake-ip) from the source.
    // This remembers a selection, not the health of its node or an active stream.
    insert(profile, "store-selected", Value::Bool(true));
    Ok(())
}

fn normalize_proxy_groups(root: &mut Mapping, summary: &mut ProfileSummary) -> AppResult<()> {
    let proxies_key = Value::String("proxies".to_string());
    let mut metadata_nodes = BTreeSet::new();
    if let Some(proxies) = root.get_mut(&proxies_key).and_then(Value::as_sequence_mut) {
        proxies.retain(|proxy| {
            let Some(name) = proxy
                .as_mapping()
                .and_then(|mapping| mapping.get(Value::String("name".to_string())))
                .and_then(Value::as_str)
            else {
                return true;
            };
            if is_subscription_metadata_node_name(name) {
                metadata_nodes.insert(name.to_string());
                false
            } else {
                true
            }
        });
    }

    let Some(groups) = root
        .get_mut(Value::String("proxy-groups".to_string()))
        .and_then(Value::as_sequence_mut)
    else {
        if !metadata_nodes.is_empty() {
            summary.warnings.push(format!(
                "已从有效配置排除 {} 个订阅状态伪节点",
                metadata_nodes.len()
            ));
        }
        return Ok(());
    };

    let mut automatic_groups = 0usize;
    for group in groups.iter_mut().filter_map(Value::as_mapping_mut) {
        if let Some(members) = group
            .get_mut(Value::String("proxies".to_string()))
            .and_then(Value::as_sequence_mut)
        {
            members.retain(|member| {
                member
                    .as_str()
                    .is_none_or(|name| !metadata_nodes.contains(name))
            });
        }
        let group_type = group
            .get(Value::String("type".to_string()))
            .and_then(Value::as_str);
        if !matches!(group_type, Some("url-test" | "fallback")) {
            continue;
        }
        let is_url_test = group_type == Some("url-test");
        automatic_groups += 1;
        let url_key = Value::String("url".into());
        // Subscription-specific probe targets/statuses may be essential on a
        // different network. Fill missing defaults, never replace valid choices.
        if !group.contains_key(&url_key) {
            group.insert(
                url_key,
                Value::String("https://www.gstatic.com/generate_204".into()),
            );
            group
                .entry(Value::String("expected-status".into()))
                .or_insert(Value::Number(Number::from(204)));
        }
        for (key, value) in [
            ("interval", 300),
            ("timeout", 8_000),
            ("max-failed-times", 2),
        ] {
            group
                .entry(Value::String(key.into()))
                .or_insert(Value::Number(Number::from(value)));
        }
        if is_url_test {
            group
                .entry(Value::String("tolerance".into()))
                .or_insert(Value::Number(Number::from(150)));
        }
        group
            .entry(Value::String("lazy".into()))
            .or_insert(Value::Bool(false));
    }

    if !metadata_nodes.is_empty() {
        summary.warnings.push(format!(
            "已从有效配置排除 {} 个订阅状态伪节点",
            metadata_nodes.len()
        ));
    }
    if automatic_groups > 0 {
        summary.warnings.push(format!(
            "已为 {automatic_groups} 个自动代理组补齐健康检查默认值；保留订阅自定义检测参数"
        ));
    }
    Ok(())
}

fn apply_openai_policy(
    root: &mut Mapping,
    policy: &OpenAiPolicy,
    summary: &mut ProfileSummary,
) -> AppResult<()> {
    if !policy.enabled {
        return Ok(());
    }

    let available_nodes: BTreeSet<String> = root
        .get(Value::String("proxies".to_string()))
        .and_then(Value::as_sequence)
        .into_iter()
        .flatten()
        .filter_map(Value::as_mapping)
        .filter_map(|node| {
            node.get(Value::String("name".to_string()))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect();
    let mut selected = Vec::new();
    let mut seen = BTreeSet::new();
    for node in &policy.selected_nodes {
        if selected.len() >= usize::from(policy.max_nodes.clamp(2, 10)) {
            break;
        }
        if available_nodes.contains(&node.name)
            && crate::node_metadata::eligible(&node.name)
            && seen.insert(node.name.clone())
        {
            selected.push(node.name.clone());
        }
    }

    let insufficient = selected.len() < 2;
    if insufficient {
        summary.warnings.push(
            "OpenAI 支持地区的有效节点少于 2 个；托管路径拒绝新请求，请重新生成灾备".to_string(),
        );
        selected.clear();
    }

    let groups_key = Value::String("proxy-groups".to_string());
    let groups = root
        .entry(groups_key)
        .or_insert_with(|| Value::Sequence(Vec::new()))
        .as_sequence_mut()
        .ok_or_else(|| AppError::Config("proxy-groups 必须是数组".to_string()))?;
    groups.retain(|group| {
        group
            .as_mapping()
            .and_then(|mapping| mapping.get(Value::String("name".to_string())))
            .and_then(Value::as_str)
            != Some(OPENAI_GROUP_NAME)
    });

    let mut group = Mapping::new();
    insert(
        &mut group,
        "name",
        Value::String(OPENAI_GROUP_NAME.to_string()),
    );
    insert(
        &mut group,
        "type",
        Value::String(
            if policy.stability_enabled || insufficient {
                "select"
            } else {
                "fallback"
            }
            .to_string(),
        ),
    );
    insert(
        &mut group,
        "proxies",
        Value::Sequence(
            selected
                .into_iter()
                .chain((policy.stability_enabled || insufficient).then(|| "REJECT".to_string()))
                .map(Value::String)
                .collect(),
        ),
    );
    insert(
        &mut group,
        "url",
        Value::String("https://api.openai.com/v1/models".to_string()),
    );
    insert(&mut group, "interval", Value::Number(Number::from(300)));
    insert(&mut group, "lazy", Value::Bool(false));
    insert(&mut group, "timeout", Value::Number(Number::from(8_000)));
    insert(
        &mut group,
        "max-failed-times",
        Value::Number(Number::from(2)),
    );
    insert(
        &mut group,
        "expected-status",
        Value::Number(Number::from(401)),
    );
    insert(
        &mut group,
        "empty-fallback",
        Value::String("REJECT".to_string()),
    );
    groups.insert(0, Value::Mapping(group));

    for existing in groups.iter_mut().filter_map(Value::as_mapping_mut) {
        let is_global = existing
            .get(Value::String("name".to_string()))
            .and_then(Value::as_str)
            == Some("GLOBAL");
        if !is_global {
            continue;
        }
        let proxies = existing
            .entry(Value::String("proxies".to_string()))
            .or_insert_with(|| Value::Sequence(Vec::new()))
            .as_sequence_mut()
            .ok_or_else(|| AppError::Config("GLOBAL.proxies 必须是数组".to_string()))?;
        if !proxies
            .iter()
            .any(|value| value.as_str() == Some(OPENAI_GROUP_NAME))
        {
            proxies.insert(0, Value::String(OPENAI_GROUP_NAME.to_string()));
        }
    }

    let rules = root
        .entry(Value::String("rules".to_string()))
        .or_insert_with(|| Value::Sequence(Vec::new()))
        .as_sequence_mut()
        .ok_or_else(|| AppError::Config("rules 必须是数组".to_string()))?;
    rules.retain(|rule| {
        rule.as_str()
            .and_then(|value| value.rsplit(',').next())
            .map(str::trim)
            != Some(OPENAI_GROUP_NAME)
    });
    for (index, (rule_type, payload)) in OPENAI_RULES.iter().enumerate() {
        rules.insert(
            index,
            Value::String(format!("{rule_type},{payload},{OPENAI_GROUP_NAME}")),
        );
    }
    Ok(())
}

fn insert(mapping: &mut Mapping, key: &str, value: Value) {
    mapping.insert(Value::String(key.to_string()), value);
}

#[cfg(test)]
mod tests {
    use super::{build_effective_config, build_effective_config_with_policy, OPENAI_GROUP_NAME};
    use crate::models::{AppSettings, NetworkMode, OpenAiNodeScore, OpenAiPolicy, RoutingMode};
    use chrono::Utc;
    use serde_yaml::Value;

    const SOURCE: &str = r#"
mixed-port: 8888
allow-lan: true
external-controller: 0.0.0.0:9090
external-controller-unix: /tmp/untrusted.sock
listeners:
  - name: untrusted
    type: mixed
    port: 9999
proxies:
  - name: sample
    type: socks5
    server: 127.0.0.1
    port: 1080
proxy-groups:
  - name: PROXY
    type: select
    proxies: [sample]
rules:
  - MATCH,PROXY
"#;

    #[test]
    fn overrides_local_control_fields() {
        let settings = AppSettings {
            mixed_port: 7895,
            controller_port: 9095,
            ..Default::default()
        };
        let effective =
            build_effective_config(SOURCE, &settings, RoutingMode::Rule).expect("effective config");
        let document: Value = serde_yaml::from_str(&effective.yaml).expect("yaml");
        let root = document.as_mapping().expect("mapping");
        assert_eq!(
            root.get(Value::String("mixed-port".to_string()))
                .and_then(Value::as_u64),
            Some(7895)
        );
        assert_eq!(
            root.get(Value::String("allow-lan".to_string()))
                .and_then(Value::as_bool),
            Some(false)
        );
        assert_eq!(
            root.get(Value::String("external-controller".to_string()))
                .and_then(Value::as_str),
            Some("127.0.0.1:9095")
        );
        assert!(!root.contains_key(Value::String("external-controller-unix".to_string())));
        assert!(!root.contains_key(Value::String("listeners".to_string())));
    }

    #[test]
    fn only_enables_tun_for_tun_mode() {
        let settings = AppSettings {
            network_mode: NetworkMode::Tun,
            ..Default::default()
        };
        let effective =
            build_effective_config(SOURCE, &settings, RoutingMode::Rule).expect("effective config");
        let document: Value = serde_yaml::from_str(&effective.yaml).expect("yaml");
        let tun = document
            .as_mapping()
            .and_then(|root| root.get(Value::String("tun".to_string())))
            .and_then(Value::as_mapping)
            .expect("tun");
        assert_eq!(
            tun.get(Value::String("enable".to_string()))
                .and_then(Value::as_bool),
            Some(true)
        );
    }

    #[test]
    fn overrides_subscription_routing_mode() {
        let effective =
            build_effective_config(SOURCE, &AppSettings::default(), RoutingMode::Global)
                .expect("effective config");
        let document: Value = serde_yaml::from_str(&effective.yaml).expect("yaml");
        assert_eq!(
            document
                .as_mapping()
                .and_then(|root| root.get(Value::String("mode".to_string())))
                .and_then(Value::as_str),
            Some("global")
        );
    }

    #[test]
    fn tcp_keepalive_defaults_preserve_explicit_subscription_choices() {
        let effective =
            build_effective_config(SOURCE, &AppSettings::default(), RoutingMode::Rule).unwrap();
        let document: Value = serde_yaml::from_str(&effective.yaml).unwrap();
        assert_eq!(document["keep-alive-idle"].as_i64(), Some(30));
        assert_eq!(document["keep-alive-interval"].as_i64(), Some(15));
        assert_eq!(document["disable-keep-alive"].as_bool(), Some(false));
        let source = format!(
            "{SOURCE}\nkeep-alive-idle: 45\nkeep-alive-interval: 20\ndisable-keep-alive: true\n"
        );
        let effective =
            build_effective_config(&source, &AppSettings::default(), RoutingMode::Rule).unwrap();
        let document: Value = serde_yaml::from_str(&effective.yaml).unwrap();
        assert_eq!(document["keep-alive-idle"].as_i64(), Some(45));
        assert_eq!(document["keep-alive-interval"].as_i64(), Some(20));
        assert_eq!(document["disable-keep-alive"].as_bool(), Some(true));
    }

    #[test]
    fn remembers_api_selections_in_every_network_mode() {
        for network_mode in [
            NetworkMode::Manual,
            NetworkMode::SystemProxy,
            NetworkMode::Tun,
        ] {
            let settings = AppSettings {
                network_mode,
                ..Default::default()
            };
            let effective = build_effective_config(SOURCE, &settings, RoutingMode::Rule).unwrap();
            let document: Value = serde_yaml::from_str(&effective.yaml).unwrap();
            assert_eq!(document["profile"]["store-selected"].as_bool(), Some(true));
            assert!(document["profile"]["store-fake-ip"].is_null());
            // Enabling persistence must not invent a successful or fixed node.
            assert_eq!(
                document["proxy-groups"][0]["proxies"][0].as_str(),
                Some("sample")
            );
            assert!(document["proxy-groups"][0]["default-selected"].is_null());
        }
    }

    #[test]
    fn selection_cache_preserves_other_profile_options() {
        for store_fake_ip in [true, false] {
            let source = format!(
                "{SOURCE}\nprofile:\n  store-selected: false\n  store-fake-ip: {store_fake_ip}\n"
            );
            let effective =
                build_effective_config(&source, &AppSettings::default(), RoutingMode::Rule)
                    .unwrap();
            let document: Value = serde_yaml::from_str(&effective.yaml).unwrap();
            assert_eq!(document["profile"]["store-selected"].as_bool(), Some(true));
            assert_eq!(
                document["profile"]["store-fake-ip"].as_bool(),
                Some(store_fake_ip)
            );
        }
    }

    #[test]
    fn selection_cache_accepts_empty_profile_and_is_idempotent() {
        let source = format!("{SOURCE}\nprofile:\n");
        let settings = AppSettings::default();
        let first = build_effective_config(&source, &settings, RoutingMode::Rule).unwrap();
        let second = build_effective_config(&first.yaml, &settings, RoutingMode::Rule).unwrap();
        assert_eq!(first.yaml, second.yaml);
        let document: Value = serde_yaml::from_str(&second.yaml).unwrap();
        assert_eq!(document["profile"]["store-selected"].as_bool(), Some(true));
    }

    #[test]
    fn selection_cache_rejects_malformed_profile_instead_of_dropping_it() {
        for malformed in ["false", "[]", "unexpected"] {
            let source = format!("{SOURCE}\nprofile: {malformed}\n");
            let result =
                build_effective_config(&source, &AppSettings::default(), RoutingMode::Rule);
            assert!(
                result.is_err(),
                "invalid profile must not be silently replaced"
            );
        }
    }

    #[test]
    fn injects_openai_fallback_and_priority_rules() {
        let mut policy = OpenAiPolicy {
            enabled: true,
            auto_maintain: true,
            selected_nodes: vec![node("JP sample", 96.0), node("JP sample-2", 88.0)],
            candidate_count: 2,
            healthy_count: 2,
            last_benchmarked_at: Some(Utc::now()),
            ..Default::default()
        };
        let source = SOURCE.replace(
            "  - name: sample\n    type: socks5\n    server: 127.0.0.1\n    port: 1080",
            "  - name: sample\n    type: socks5\n    server: 127.0.0.1\n    port: 1080\n  - name: sample-2\n    type: socks5\n    server: 127.0.0.1\n    port: 1081",
        );
        let source = source
            .replace("name: sample", "name: JP sample")
            .replace("proxies: [sample]", "proxies: [JP sample]");
        let effective = build_effective_config_with_policy(
            &source,
            &AppSettings::default(),
            RoutingMode::Rule,
            Some(&policy),
        )
        .expect("effective config");
        let document: Value = serde_yaml::from_str(&effective.yaml).expect("yaml");
        let root = document.as_mapping().expect("root");
        let groups = root
            .get(Value::String("proxy-groups".to_string()))
            .and_then(Value::as_sequence)
            .expect("groups");
        let managed = groups[0].as_mapping().expect("managed group");
        assert_eq!(
            managed
                .get(Value::String("name".to_string()))
                .and_then(Value::as_str),
            Some(OPENAI_GROUP_NAME)
        );
        assert_eq!(
            managed
                .get(Value::String("type".to_string()))
                .and_then(Value::as_str),
            Some("fallback")
        );
        assert_eq!(
            managed
                .get(Value::String("expected-status".to_string()))
                .and_then(Value::as_u64),
            Some(401)
        );
        assert_eq!(
            managed
                .get(Value::String("proxies".to_string()))
                .and_then(Value::as_sequence)
                .map(Vec::len),
            Some(2)
        );
        let first_rule = root
            .get(Value::String("rules".to_string()))
            .and_then(Value::as_sequence)
            .and_then(|rules| rules.first())
            .and_then(Value::as_str)
            .expect("first rule");
        assert_eq!(
            first_rule,
            format!("DOMAIN-SUFFIX,openai.com,{OPENAI_GROUP_NAME}")
        );
        policy.stability_enabled = true;
        let stable = build_effective_config_with_policy(
            &source,
            &AppSettings::default(),
            RoutingMode::Rule,
            Some(&policy),
        )
        .unwrap();
        let stable_doc: Value = serde_yaml::from_str(&stable.yaml).unwrap();
        assert_eq!(
            stable_doc["proxy-groups"][0]["type"].as_str(),
            Some("select")
        );
        let candidates = stable_doc["proxy-groups"][0]["proxies"]
            .as_sequence()
            .unwrap();
        assert_eq!(candidates.len(), 3);
        assert_eq!(candidates.last().and_then(Value::as_str), Some("REJECT"));
        if let Some(binary) = std::env::var_os("MIHOMO_TEST_BINARY") {
            let root = tempfile::tempdir().unwrap();
            let path = root.path().join("stable-selector.yaml");
            std::fs::write(&path, stable.yaml).unwrap();
            crate::runtime::validate_file(std::path::Path::new(&binary), root.path(), &path)
                .expect("native stable selector");
        }
    }

    #[test]
    fn rejects_new_requests_when_fewer_than_two_eligible_nodes_still_exist() {
        let policy = OpenAiPolicy {
            enabled: true,
            selected_nodes: vec![node("sample", 90.0), node("removed", 80.0)],
            ..Default::default()
        };
        let effective = build_effective_config_with_policy(
            SOURCE,
            &AppSettings::default(),
            RoutingMode::Rule,
            Some(&policy),
        )
        .expect("effective config");
        assert!(effective
            .summary
            .warnings
            .iter()
            .any(|warning| warning.contains("少于 2 个")));
        assert!(effective.yaml.contains(OPENAI_GROUP_NAME));
        let doc: Value = serde_yaml::from_str(&effective.yaml).unwrap();
        assert_eq!(
            doc["proxy-groups"][0]["proxies"],
            serde_yaml::to_value(vec!["REJECT"]).unwrap()
        );
        assert!(doc["rules"][0]
            .as_str()
            .unwrap()
            .contains(OPENAI_GROUP_NAME));
    }

    #[test]
    fn filters_metadata_nodes_without_overriding_subscription_probes() {
        let source = r#"
proxies:
  - name: 剩余流量：15.61 GB
    type: socks5
    server: status.example.com
    port: 1080
  - name: node-a
    type: socks5
    server: a.example.com
    port: 1081
proxy-groups:
  - name: AUTO
    type: url-test
    proxies: [剩余流量：15.61 GB, node-a]
    url: http://www.gstatic.com/generate_204
    expected-status: 200
    tolerance: 80
rules:
  - MATCH,AUTO
"#;
        let effective = build_effective_config(source, &AppSettings::default(), RoutingMode::Rule)
            .expect("effective config");
        let document: Value = serde_yaml::from_str(&effective.yaml).expect("yaml");
        let root = document.as_mapping().expect("root");
        let proxies = root
            .get(Value::String("proxies".to_string()))
            .and_then(Value::as_sequence)
            .expect("proxies");
        assert_eq!(proxies.len(), 1);
        let group = root
            .get(Value::String("proxy-groups".to_string()))
            .and_then(Value::as_sequence)
            .and_then(|groups| groups.first())
            .and_then(Value::as_mapping)
            .expect("group");
        assert_eq!(
            group
                .get(Value::String("proxies".to_string()))
                .and_then(Value::as_sequence)
                .map(Vec::len),
            Some(1)
        );
        assert_eq!(
            group
                .get(Value::String("url".to_string()))
                .and_then(Value::as_str),
            Some("http://www.gstatic.com/generate_204")
        );
        assert_eq!(
            group
                .get(Value::String("expected-status".to_string()))
                .and_then(Value::as_u64),
            Some(200)
        );
        assert_eq!(
            group
                .get(Value::String("lazy".to_string()))
                .and_then(Value::as_bool),
            Some(false)
        );
        assert!(effective
            .summary
            .warnings
            .iter()
            .any(|warning| warning.contains("订阅状态伪节点")));
    }

    fn node(name: &str, score: f64) -> OpenAiNodeScore {
        OpenAiNodeScore {
            name: name.to_string(),
            latency_ms: 100,
            jitter_ms: 5,
            bandwidth_mbps: Some(50.0),
            score,
            checked_at: Utc::now(),
        }
    }
}
