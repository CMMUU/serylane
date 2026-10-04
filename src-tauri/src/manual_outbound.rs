//! A single fixed outbound, mutually exclusive with subscription groups/policies.
use crate::{
    error::{AppError, AppResult},
    models::{AppSettings, ProfileSource},
    storage::AppStorage,
};
use serde::{Deserialize, Serialize};
use serde_yaml::{Mapping, Value};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Manager};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ManualOutbound {
    pub profile_id: Uuid,
    pub node_name: String,
    pub identity: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeOption {
    profile_id: Uuid,
    revision_id: Uuid,
    profile_name: String,
    name: String,
    protocol: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub selection: Option<ManualOutbound>,
    nodes: Vec<NodeOption>,
    notices: Vec<String>,
}

pub(crate) fn fixed_node(node: &Value) -> bool {
    let Some(name) = node["name"].as_str() else {
        return false;
    };
    !name.is_empty()
        && name.trim() == name
        && !name.contains(',')
        && !name.chars().any(char::is_control)
        && !crate::config::is_subscription_metadata_node_name(name)
        && !matches!(
            name,
            "DIRECT" | "REJECT" | "GLOBAL" | "PASS" | "REJECT-DROP"
        )
        && node["type"]
            .as_str()
            .is_some_and(|t| !matches!(t, "direct" | "reject" | "pass"))
        && node.get("dialer-proxy").is_none()
}

pub(crate) fn identity(node: &Value) -> AppResult<String> {
    Ok(format!(
        "{:x}",
        Sha256::digest(
            serde_json::to_value(node)
                .and_then(|value| serde_json::to_vec(&value))
                .map_err(|_| AppError::Config("节点身份读取失败".into()))?
        )
    ))
}

fn source_nodes(source: &str) -> AppResult<Value> {
    serde_yaml::from_str(source)
        .map_err(|_| AppError::Config("订阅配置读取失败，请更新订阅后重试".into()))
}

pub fn snapshot(storage: &AppStorage) -> AppResult<Snapshot> {
    let mut nodes = Vec::new();
    let mut notices = Vec::new();
    for profile in storage.list_profiles()? {
        if !matches!(profile.source, ProfileSource::RemoteSubscription { .. }) {
            continue;
        }
        let Some(revision_id) = profile.active_revision_id else {
            continue;
        };
        let document = match storage
            .load_revision_source(profile.id, revision_id)
            .and_then(|source| source_nodes(&source))
        {
            Ok(document) => document,
            Err(_) => {
                notices.push(format!(
                    "{}：本地订阅版本读取失败，请更新该订阅；其他订阅仍可选择。",
                    profile.display_name
                ));
                continue;
            }
        };
        if document["proxy-providers"]
            .as_mapping()
            .is_some_and(|v| !v.is_empty())
        {
            notices.push(format!(
                "{}：动态提供器节点仍在代理页管理，自选列表展示订阅中已保存的独立节点。",
                profile.display_name
            ));
        }
        if let Some(entries) = document["proxies"].as_sequence() {
            if entries
                .iter()
                .any(|node| node.get("dialer-proxy").is_some())
            {
                notices.push(format!(
                    "{}：链式代理节点在代理页管理，自选模式仅支持独立节点。",
                    profile.display_name
                ));
            }
            for node in entries.iter().filter(|node| fixed_node(node)) {
                nodes.push(NodeOption {
                    profile_id: profile.id,
                    revision_id,
                    profile_name: profile.display_name.clone(),
                    name: node["name"].as_str().unwrap().into(),
                    protocol: node["type"].as_str().unwrap().into(),
                });
            }
        }
    }
    Ok(Snapshot {
        selection: storage.settings()?.manual_outbound,
        nodes,
        notices,
    })
}

pub fn require_proxy_mode(settings: &AppSettings) -> AppResult<()> {
    if settings.manual_outbound.is_some() {
        return Err(AppError::Conflict(
            "自选节点正在生效；请先在自选节点页切回代理模式，再操作策略组或自动选点。".into(),
        ));
    }
    Ok(())
}

pub fn require_profile(settings: &AppSettings, profile_id: Uuid) -> AppResult<()> {
    if settings
        .manual_outbound
        .as_ref()
        .is_some_and(|m| m.profile_id != profile_id)
    {
        return Err(AppError::Conflict(
            "自选节点正在使用另一订阅；请在自选节点页重新选择，或先切回代理模式。".into(),
        ));
    }
    Ok(())
}

pub fn apply(root: &mut Mapping, selected: &ManualOutbound) -> AppResult<()> {
    let nodes = root
        .get("proxies")
        .and_then(Value::as_sequence)
        .ok_or_else(|| {
            AppError::Conflict("自选节点已不在订阅中，请重新选择；没有自动切换出口。".into())
        })?;
    let matches: Vec<_> = nodes
        .iter()
        .filter(|n| n["name"].as_str() == Some(&selected.node_name))
        .collect();
    if matches.len() != 1 || !fixed_node(matches[0]) || identity(matches[0])? != selected.identity {
        return Err(AppError::Conflict(
            "自选节点已移除或连接信息发生变化，请重新确认；没有自动切换出口。".into(),
        ));
    }
    let node = matches[0].clone();
    root.insert("proxies".into(), Value::Sequence(vec![node]));
    for key in [
        "proxy-groups",
        "proxy-providers",
        "rule-providers",
        "sub-rules",
    ] {
        root.remove(key);
    }
    root.insert("mode".into(), "rule".into());
    root.insert(
        "rules".into(),
        Value::Sequence(vec![format!("MATCH,{}", selected.node_name).into()]),
    );
    Ok(())
}

pub async fn set(
    app: &AppHandle,
    profile_id: Option<Uuid>,
    revision_id: Option<Uuid>,
    node_name: Option<String>,
    confirmed: bool,
) -> AppResult<Snapshot> {
    if !confirmed {
        return Err(AppError::InvalidInput("请确认出口切换后再继续".into()));
    }
    let permit = crate::user_rules::acquire_configuration(app)?;
    let storage = AppStorage::from_app(app)?;
    let previous = storage.settings()?;
    let state = storage.state()?;
    let mut next = previous.clone();
    let (id, revision) = if let Some(id) = profile_id {
        let profile = storage.load_profile(id)?;
        if !matches!(profile.source, ProfileSource::RemoteSubscription { .. }) {
            return Err(AppError::InvalidInput("请选择已添加订阅中的节点".into()));
        }
        let revision = revision_id
            .filter(|r| Some(*r) == profile.active_revision_id)
            .ok_or_else(|| AppError::Conflict("订阅版本已变化，请刷新节点列表后重选".into()))?;
        let name = node_name.ok_or_else(|| AppError::InvalidInput("请选择节点".into()))?;
        let document = source_nodes(&storage.load_revision_source(id, revision)?)?;
        let matches: Vec<_> = document["proxies"]
            .as_sequence()
            .into_iter()
            .flatten()
            .filter(|n| n["name"].as_str() == Some(&name) && fixed_node(n))
            .collect();
        if matches.len() != 1 {
            return Err(AppError::InvalidInput(
                "所选节点不是唯一的独立节点，请刷新后重新选择".into(),
            ));
        }
        next.manual_outbound = Some(ManualOutbound {
            profile_id: id,
            node_name: name,
            identity: identity(matches[0])?,
        });
        (id, revision)
    } else {
        if node_name.is_some() || revision_id.is_some() {
            return Err(AppError::InvalidInput("退出自选模式不接受节点参数".into()));
        }
        next.manual_outbound = None;
        (
            state
                .active_profile_id
                .ok_or_else(|| AppError::NotFound("当前订阅".into()))?,
            state
                .active_revision_id
                .ok_or_else(|| AppError::NotFound("当前订阅版本".into()))?,
        )
    };
    let profile = storage.load_profile(id)?;
    let source = storage.load_revision_source(id, revision)?;
    let policy = storage.load_revision(id, revision)?.openai_policy;
    let candidate = crate::effective::build_effective_config_with_policy(
        &source,
        &next,
        profile.routing_mode,
        Some(&policy),
    )?;
    let mut result = snapshot(&storage)?;
    result.selection = next.manual_outbound.clone();
    crate::user_rules::apply_profile_config(
        app,
        &storage,
        &candidate.yaml,
        || commit_selection(&storage, &previous, &next, id, revision, &candidate.yaml),
        &permit,
    )
    .await?;
    app.state::<crate::openai_stability::StabilityManager>()
        .update_policy_status(app);
    Ok(result)
}

// Runtime reload is handled by apply_profile_config; keep disk rollback together.
fn commit_selection(
    storage: &AppStorage,
    previous: &AppSettings,
    next: &AppSettings,
    id: Uuid,
    revision: Uuid,
    yaml: &str,
) -> AppResult<()> {
    storage.save_settings(next)?;
    if let Err(error) = crate::profile_service::commit_active_selection(storage, id, revision, yaml)
    {
        storage.save_settings(previous)?;
        return Err(error);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fixed_outbound_removes_automatic_and_user_rules_and_checks_identity() {
        let mut root: Mapping = serde_yaml::from_str("proxies:\n- {name: A, type: socks5, server: localhost, port: 1080}\nproxy-groups: [{name: Auto, type: url-test, proxies: [A]}]\nrules: [MATCH,DIRECT]\n").unwrap();
        let selected = ManualOutbound {
            profile_id: Uuid::nil(),
            node_name: "A".into(),
            identity: identity(&root["proxies"][0]).unwrap(),
        };
        apply(&mut root, &selected).unwrap();
        assert_eq!(root["rules"][0].as_str(), Some("MATCH,A"));
        assert!(!root.contains_key("proxy-groups"));
        root.get_mut("proxies").unwrap()[0]["server"] = "changed".into();
        assert!(apply(&mut root, &selected).is_err());
    }
    #[test]
    fn only_independent_nodes_can_be_fixed() {
        for value in [
            "{name: 'A,B', type: socks5}",
            "{name: A, type: socks5, dialer-proxy: Auto}",
            "{name: DIRECT, type: direct}",
        ] {
            assert!(!fixed_node(&serde_yaml::from_str::<Value>(value).unwrap()));
        }
    }
    const SOURCE: &str = "proxies:\n- {name: A, type: socks5, server: localhost, port: 1080, password: private-fixture}\n- {name: B, type: socks5, server: localhost, port: 1081}\nproxy-groups: [{name: Auto, type: select, proxies: [A, B]}]\nrules: ['MATCH,Auto']\n";

    fn fixture(storage: &AppStorage, name: &str) -> (Uuid, Uuid) {
        use crate::models::{OpenAiPolicy, ValidationReport};
        let profile = storage
            .create_profile(
                name.into(),
                ProfileSource::RemoteSubscription {
                    url: "https://example.invalid/subscribe?token=private-fixture".into(),
                    user_agent: "clash.meta".into(),
                },
            )
            .unwrap();
        let revision = storage
            .save_revision(
                profile.id,
                SOURCE,
                SOURCE,
                None,
                ValidationReport {
                    valid: true,
                    ..Default::default()
                },
                OpenAiPolicy::default(),
            )
            .unwrap();
        storage
            .update_profile_revision(profile.id, revision.id)
            .unwrap();
        (profile.id, revision.id)
    }

    fn selected(profile_id: Uuid) -> ManualOutbound {
        let doc: Value = serde_yaml::from_str(SOURCE).unwrap();
        ManualOutbound {
            profile_id,
            node_name: "A".into(),
            identity: identity(&doc["proxies"][0]).unwrap(),
        }
    }

    #[test]
    fn snapshot_is_private_and_a_broken_revision_does_not_hide_other_subscriptions() {
        let dir = tempfile::tempdir().unwrap();
        let storage = AppStorage::from_root(dir.path().into()).unwrap();
        let (good, _) = fixture(&storage, "good");
        let (broken, revision) = fixture(&storage, "broken");
        std::fs::remove_file(
            dir.path()
                .join("profiles")
                .join(broken.to_string())
                .join("revisions")
                .join(revision.to_string())
                .join("source.yaml"),
        )
        .unwrap();
        let result = snapshot(&storage).unwrap();
        assert_eq!(result.nodes.len(), 2);
        assert!(result.nodes.iter().all(|n| n.profile_id == good));
        assert_eq!(result.notices.len(), 1);
        let json = serde_json::to_string(&result).unwrap();
        for secret in ["private-fixture", "example.invalid", "localhost", "1080"] {
            assert!(!json.contains(secret));
        }
    }

    #[test]
    fn node_identity_ignores_yaml_key_order_not_connection_changes() {
        let a: Value =
            serde_yaml::from_str("{name: A, type: socks5, server: localhost, port: 1080}").unwrap();
        let b: Value =
            serde_yaml::from_str("{port: 1080, server: localhost, type: socks5, name: A}").unwrap();
        assert_eq!(identity(&a).unwrap(), identity(&b).unwrap());
    }

    #[test]
    fn duplicate_or_missing_nodes_fail_closed_without_mutating_the_document() {
        let selection = selected(Uuid::nil());
        for source in [
            SOURCE.replace("name: B", "name: A"),
            SOURCE.replace("name: A", "name: Removed"),
        ] {
            let mut root: Mapping = serde_yaml::from_str(&source).unwrap();
            let before = root.clone();
            assert!(apply(&mut root, &selection).is_err());
            assert_eq!(root, before);
        }
    }

    #[test]
    fn manual_mode_overrides_policy_and_rules_but_retains_original_settings() {
        use crate::models::{OpenAiPolicy, PublicAppSettings, RoutingMode};
        let settings = AppSettings {
            manual_outbound: Some(selected(Uuid::nil())),
            user_rules: vec![crate::user_rules::UserRule {
                id: "fixture".into(),
                enabled: true,
                rule: "DOMAIN,example.com,REJECT".into(),
                note: String::new(),
            }],
            ..Default::default()
        };
        let policy = OpenAiPolicy {
            enabled: true,
            stability_enabled: true,
            ..Default::default()
        };
        for mode in [RoutingMode::Rule, RoutingMode::Global, RoutingMode::Direct] {
            let built = crate::effective::build_effective_config_with_policy(
                SOURCE,
                &settings,
                mode,
                Some(&policy),
            )
            .unwrap();
            let doc: Value = serde_yaml::from_str(&built.yaml).unwrap();
            assert_eq!(doc["rules"], serde_yaml::to_value(["MATCH,A"]).unwrap());
            assert_eq!(doc["mode"].as_str(), Some("rule"));
            assert_eq!(doc["proxies"].as_sequence().unwrap().len(), 1);
            assert!(doc.get("proxy-groups").is_none());
            if let Some(binary) = std::env::var_os("MIHOMO_TEST_BINARY") {
                let dir = tempfile::tempdir().unwrap();
                let path = dir.path().join("manual.yaml");
                std::fs::write(&path, built.yaml).unwrap();
                crate::runtime::validate_file(std::path::Path::new(&binary), dir.path(), &path)
                    .unwrap();
            }
        }
        assert!(require_proxy_mode(&settings).is_err());
        assert!(require_profile(&settings, Uuid::nil()).is_ok());
        assert!(require_profile(&settings, Uuid::new_v4()).is_err());
        assert_eq!(
            PublicAppSettings::from(&settings)
                .merge_secret(&settings)
                .manual_outbound,
            settings.manual_outbound
        );
        let mut restored = settings.clone();
        restored.manual_outbound = None;
        let built =
            crate::effective::build_effective_config(SOURCE, &restored, RoutingMode::Rule).unwrap();
        let doc: Value = serde_yaml::from_str(&built.yaml).unwrap();
        assert_eq!(doc["rules"][0].as_str(), Some("DOMAIN,example.com,REJECT"));
        assert!(doc.get("proxy-groups").is_some());
        assert!(require_proxy_mode(&restored).is_ok());
    }

    #[test]
    fn failed_disk_selection_restores_mode_profile_and_runtime() {
        let dir = tempfile::tempdir().unwrap();
        let storage = AppStorage::from_root(dir.path().into()).unwrap();
        let (old, old_rev) = fixture(&storage, "old");
        let (new, new_rev) = fixture(&storage, "new");
        crate::profile_service::commit_active_selection(&storage, old, old_rev, SOURCE).unwrap();
        let before = storage.settings().unwrap();
        let next = AppSettings {
            manual_outbound: Some(selected(new)),
            ..before.clone()
        };
        // Force a file replacement failure after settings were persisted.
        std::fs::create_dir(dir.path().join(".state.json.backup")).unwrap();
        assert!(commit_selection(&storage, &before, &next, new, new_rev, "candidate").is_err());
        assert!(storage.settings().unwrap().manual_outbound.is_none());
        assert_eq!(storage.state().unwrap().active_profile_id, Some(old));
        assert_eq!(
            storage.active_runtime_config().unwrap().as_deref(),
            Some(SOURCE)
        );
    }
}
