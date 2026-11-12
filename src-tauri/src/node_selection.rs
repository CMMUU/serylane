use crate::error::{AppError, AppResult};
use crate::mihomo_api::{MihomoApiClient, ProxyWriteState};
use crate::openai_stability::{StabilityManager, GROUP};
use crate::runtime::MihomoRuntime;
use crate::storage::AppStorage;
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;
use std::time::Duration;
use tauri::{AppHandle, Manager};
use uuid::Uuid;

const GROUP_READ_BUDGET: Duration = Duration::from_millis(1500);
type RunningIdentity = (u32, DateTime<Utc>);

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SelectionReceipt {
    write_state: ProxyWriteState,
    group: String,
    group_type: Option<String>,
    now: Option<String>,
    fixed: Option<String>,
    manual_node: Option<String>,
}

impl SelectionReceipt {
    fn new(
        write_state: ProxyWriteState,
        group: &str,
        observed: Option<&Value>,
        manual_node: Option<String>,
    ) -> Self {
        Self {
            write_state,
            group: group.into(),
            group_type: observed
                .and_then(|value| group_field(value, "type"))
                .map(str::to_owned),
            now: observed
                .and_then(|value| group_field(value, "now"))
                .map(str::to_owned),
            fixed: observed
                .and_then(|value| group_field(value, "fixed"))
                .map(str::to_owned),
            manual_node,
        }
    }
}

fn group_field<'a>(group: &'a Value, field: &str) -> Option<&'a str> {
    group[field].as_str().filter(|text| !text.is_empty())
}

fn verified_group_shape(value: &Value, expected: &Value) -> bool {
    let kind = group_field(value, "type");
    kind.is_some()
        && kind == group_field(expected, "type")
        && group_field(value, "now").is_some()
        // Missing fixed is not the same as an explicitly cleared preference.
        && (!matches!(kind, Some("Fallback" | "URLTest"))
            || value.get("fixed").and_then(Value::as_str).is_some())
}

fn confirms_requested_choice(group: &Value, node: Option<&str>) -> bool {
    match (group_field(group, "type"), node) {
        (Some("Selector"), Some(name)) => group_field(group, "now") == Some(name),
        (Some("Fallback" | "URLTest"), Some(name)) => group_field(group, "fixed") == Some(name),
        (Some("Fallback" | "URLTest"), None) => group["fixed"].as_str() == Some(""),
        _ => false,
    }
}

fn running_identity(app: &AppHandle) -> Option<RunningIdentity> {
    app.state::<MihomoRuntime>().running_identity()
}

fn scope_current(
    app: &AppHandle,
    storage: &AppStorage,
    id: Uuid,
    revision: Uuid,
    runtime: RunningIdentity,
) -> AppResult<bool> {
    let state = storage.state()?;
    Ok(state.active_profile_id == Some(id)
        && state.active_revision_id == Some(revision)
        && running_identity(app) == Some(runtime))
}

fn stale_selection() -> AppError {
    AppError::Conflict("配置或运行状态已变化，请刷新后重新选点".into())
}

pub(crate) fn validate_choice(group: &Value, node: &str) -> AppResult<()> {
    if !matches!(
        group["type"].as_str(),
        Some("Selector" | "Fallback" | "URLTest")
    ) {
        return Err(AppError::InvalidInput("该策略组不支持手动选点".into()));
    }
    if !group["all"]
        .as_array()
        .is_some_and(|all| all.iter().any(|v| v.as_str() == Some(node)))
    {
        return Err(AppError::Conflict(
            "节点已不在此策略组中，请刷新后重新选择".into(),
        ));
    }
    Ok(())
}

pub async fn proxies(app: &AppHandle) -> AppResult<Value> {
    let storage = AppStorage::from_app(app)?;
    let (generation, state, settings, runtime) =
        crate::user_rules::observe_configuration(app, |generation| {
            Ok((
                generation,
                storage.state()?,
                storage.settings()?,
                running_identity(app).ok_or_else(stale_selection)?,
            ))
        })?;
    // Do not hold the configuration permit through the controller request or
    // subscription metadata parsing. A writer can proceed during either.
    let mut payload = MihomoApiClient::new(&settings)?.proxies().await?;
    payload["modeRevision"] =
        serde_json::to_value(settings.proxy_mode_revision).unwrap_or(Value::Null);
    payload["profileId"] = serde_json::to_value(state.active_profile_id).unwrap_or(Value::Null);
    payload["revisionId"] = serde_json::to_value(state.active_revision_id).unwrap_or(Value::Null);
    if let Some(id) = state
        .active_profile_id
        .filter(|_| settings.proxy_mode == crate::manual_outbound::ProxyMode::Ai)
    {
        if let Some(revision) = state.active_revision_id {
            let source = storage.load_revision_source(id, revision)?;
            let preferences = storage.openai_costs(id)?;
            let costs = preferences.resolve(&source)?;
            let metadata: std::collections::BTreeMap<_, _> =
                crate::openai_cost::resolved_rows(&preferences, &source)?
                    .into_iter()
                    .map(|row| (row.name, row.metadata))
                    .collect();
            payload["costMode"] =
                serde_json::to_value(costs.preferences.mode).unwrap_or(Value::Null);
            if let Some(proxies) = payload["proxies"].as_object_mut() {
                for (name, node) in proxies {
                    if let Some(node) = node.as_object_mut() {
                        node.insert(
                            "trafficMultiplier".into(),
                            serde_json::to_value(costs.multiplier(name)).unwrap_or(Value::Null),
                        );
                        node.insert("withinCostBudget".into(), Value::Bool(costs.allowed(name)));
                        if let Some(metadata) = metadata.get(name) {
                            node.insert(
                                "metadata".into(),
                                serde_json::to_value(metadata).unwrap_or(Value::Null),
                            );
                        }
                    }
                }
            }
        }
        if let Some(group) = payload["proxies"][GROUP].as_object_mut() {
            if group.get("type").and_then(Value::as_str) == Some("Selector") {
                group.insert(
                    "manualNode".into(),
                    serde_json::to_value(storage.openai_manual_node(id)?).unwrap_or(Value::Null),
                );
            }
        }
        let snapshot = app.state::<StabilityManager>().snapshot();
        if snapshot.enabled
            && snapshot.profile_id == Some(id)
            && snapshot.revision_id == state.active_revision_id
        {
            payload["openaiRanking"] = serde_json::to_value(snapshot).unwrap_or(Value::Null);
        }
    }
    // No read from before a configuration transaction can be presented as
    // post-transaction state. Also catch runtime exit/restart and independent
    // network mode changes not reflected by the mutation generation.
    crate::user_rules::observe_configuration(app, |latest_generation| {
        if generation != latest_generation || running_identity(app) != Some(runtime) {
            return Err(stale_selection());
        }
        let latest_state = storage.state()?;
        let latest_settings = storage.settings()?;
        if state.active_profile_id != latest_state.active_profile_id
            || state.active_revision_id != latest_state.active_revision_id
            || settings.proxy_mode_revision != latest_settings.proxy_mode_revision
            || settings.proxy_mode != latest_settings.proxy_mode
            || settings.network_mode != latest_settings.network_mode
            || settings.controller_port != latest_settings.controller_port
            || settings.controller_secret != latest_settings.controller_secret
        {
            return Err(stale_selection());
        }
        Ok(payload)
    })
}

// This changes only the selected outbound. No config reload, connection close,
// core restart or request replay is performed by either command.
pub(crate) async fn select(
    app: &AppHandle,
    group: &str,
    node: Option<&str>,
    id: Uuid,
    revision: Uuid,
    mode_revision: Uuid,
) -> AppResult<SelectionReceipt> {
    let runtime = running_identity(app).ok_or_else(stale_selection)?;
    // Only the pre-write wait is retried. After a request is sent, no retry
    // or replay of that mutation is allowed.
    let _permit = crate::user_rules::acquire_configuration_for_node_selection(app).await?;
    let storage = AppStorage::from_app(app)?;
    let settings = storage.settings()?;
    crate::manual_outbound::require_revision(&settings, mode_revision)?;
    if group == GROUP {
        crate::manual_outbound::require_proxy_mode(&settings)?;
    } else {
        crate::manual_outbound::require_manual_mode(&settings)?;
    }
    if !scope_current(app, &storage, id, revision, runtime)? {
        return Err(stale_selection());
    }
    let profile = storage.load_profile(id)?;
    let api = MihomoApiClient::for_node_selection(&settings)?;
    let runtime_group = api.proxy_group(group, GROUP_READ_BUDGET).await?;
    let managed = group == GROUP
        && crate::openai_stability::manages(&profile.openai_policy)
        && runtime_group["type"] == "Selector";
    if let Some(node) = node {
        validate_choice(&runtime_group, node)?;
        if group == GROUP
            && !storage
                .openai_costs(id)?
                .resolve(&storage.load_revision_source(id, revision)?)?
                .allowed(node)
        {
            return Err(AppError::InvalidInput("所选节点不符合当前预算限制".into()));
        }
        if group == GROUP && !crate::node_metadata::eligible(node) {
            return Err(AppError::InvalidInput(
                crate::node_metadata::parse(node).region_reason,
            ));
        }
        if group == GROUP
            && (!profile.openai_policy.enabled
                || !profile
                    .openai_policy
                    .selected_nodes
                    .iter()
                    .any(|n| n.name == node))
        {
            return Err(AppError::InvalidInput(
                "请选择已筛选的 OpenAI 候选节点".into(),
            ));
        }
    } else if !managed && !matches!(runtime_group["type"].as_str(), Some("Fallback" | "URLTest")) {
        return Err(AppError::InvalidInput("该策略组没有自动选择模式".into()));
    }
    if !scope_current(app, &storage, id, revision, runtime)? {
        return Err(stale_selection());
    }
    let remember = group == GROUP && profile.openai_policy.enabled;
    let previous_manual = if remember {
        storage.openai_manual_node(id)?
    } else {
        None
    };
    if remember {
        // Persist first. If the controller refuses or does not confirm an
        // ambiguous write, restore prior intent so auto-maintenance won't
        // accidentally retry the unconfirmed selection.
        storage.save_openai_manual_node(id, node)?;
    }
    let result = if node.is_none() && managed {
        // Selector DELETE is rejected by Mihomo; clear the stored intent.
        Ok(ProxyWriteState::Accepted)
    } else {
        api.write_proxy_choice(group, node).await
    };
    let write_state = match result {
        Ok(state) => state,
        Err(error) => {
            if remember {
                storage.save_openai_manual_node(id, previous_manual.as_deref())?;
            }
            return Err(error);
        }
    };

    // A timeout or transport error does not show whether the core received
    // the request. In both cases inspect only this group once, never PUT again.
    let observed = if scope_current(app, &storage, id, revision, runtime).unwrap_or(false) {
        api.proxy_group(group, GROUP_READ_BUDGET)
            .await
            .ok()
            .filter(|value| {
                verified_group_shape(value, &runtime_group)
                    && scope_current(app, &storage, id, revision, runtime).unwrap_or(false)
            })
    } else {
        None
    };
    let confirmed = write_state == ProxyWriteState::Accepted
        || observed
            .as_ref()
            .is_some_and(|value| confirms_requested_choice(value, node));
    let manual_node = if !remember {
        None
    } else if confirmed {
        node.map(str::to_owned)
    } else {
        storage
            .save_openai_manual_node(id, previous_manual.as_deref())
            .map_err(|_| {
                AppError::Runtime(
                    "节点提交结果未知，且未能恢复上次手动选择记录，请核对当前出口".into(),
                )
            })?;
        previous_manual
    };

    if group == GROUP {
        let manager = app.state::<StabilityManager>();
        manager.invalidate_observations();
        if node.is_none() && managed {
            manager.resume_ranked_selection();
        }
    }
    Ok(SelectionReceipt::new(
        write_state,
        group,
        observed.as_ref(),
        manual_node,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn only_supported_groups_and_current_members_are_selectable() {
        for kind in ["Selector", "Fallback", "URLTest"] {
            let group = json!({"type":kind,"all":["东京 / A", "DIRECT"]});
            assert!(validate_choice(&group, "东京 / A").is_ok());
            assert!(validate_choice(&group, "missing").is_err());
        }
        for kind in ["LoadBalance", "Relay", "Direct", "Unknown"] {
            assert!(validate_choice(&json!({"type":kind,"all":["A"]}), "A").is_err());
        }
        assert!(validate_choice(&Value::Null, "A").is_err());
    }

    #[test]
    fn receipt_keeps_write_acknowledgement_separate_from_observed_state() {
        let observed = json!({"type":"Fallback","now":"实际出口","fixed":"优先节点"});
        let result = SelectionReceipt::new(ProxyWriteState::Unknown, "组", Some(&observed), None);
        assert_eq!(
            serde_json::to_value(result).unwrap(),
            json!({
                "writeState": "unknown",
                "group": "组",
                "groupType": "Fallback",
                "now": "实际出口",
                "fixed": "优先节点",
                "manualNode": null,
            })
        );
        let result =
            SelectionReceipt::new(ProxyWriteState::Accepted, "组", None, Some("意图".into()));
        assert_eq!(
            serde_json::to_value(result).unwrap(),
            json!({
                "writeState": "accepted",
                "group": "组",
                "groupType": null,
                "now": null,
                "fixed": null,
                "manualNode": "意图",
            })
        );
    }

    #[test]
    fn incomplete_fallback_readback_cannot_confirm_cleared_preference() {
        let expected = json!({"type":"Fallback"});
        assert!(verified_group_shape(
            &json!({"type":"Fallback","now":"A","fixed":""}),
            &expected
        ));
        for reply in [
            json!({"type":"Fallback","now":"A"}),
            json!({"type":"Fallback","now":"A","fixed":null}),
            json!({"type":"Fallback","fixed":""}),
            json!({"type":"Selector","now":"A"}),
        ] {
            assert!(!verified_group_shape(&reply, &expected));
        }
    }

    #[test]
    fn unknown_outcome_can_only_preserve_new_intent_with_matching_evidence() {
        let selector = json!({"type":"Selector","now":"B"});
        assert!(confirms_requested_choice(&selector, Some("B")));
        assert!(!confirms_requested_choice(&selector, Some("A")));
        let fallback = json!({"type":"Fallback","now":"A","fixed":"B"});
        assert!(confirms_requested_choice(&fallback, Some("B")));
        assert!(!confirms_requested_choice(&fallback, Some("A")));
        assert!(!confirms_requested_choice(&fallback, None));
        assert!(confirms_requested_choice(
            &json!({"type":"URLTest","fixed":""}),
            None
        ));
        assert!(!confirms_requested_choice(
            &json!({"type":"URLTest","now":"A"}),
            None
        ));
    }

    #[test]
    fn manual_intent_is_profile_scoped_and_survives_reopening() {
        let dir = tempfile::tempdir().unwrap();
        let storage = AppStorage::from_root(dir.path().to_path_buf()).unwrap();
        let make = |label: &str| {
            storage
                .create_profile(
                    label.into(),
                    crate::models::ProfileSource::Inline {
                        label: label.into(),
                    },
                )
                .unwrap()
        };
        let first = make("first");
        let second = make("second");
        assert_eq!(storage.openai_manual_node(first.id).unwrap(), None);
        storage
            .save_openai_manual_node(first.id, Some("东京 / A"))
            .unwrap();
        let reopened = AppStorage::from_root(dir.path().to_path_buf()).unwrap();
        assert_eq!(
            reopened.openai_manual_node(first.id).unwrap().as_deref(),
            Some("东京 / A")
        );
        assert_eq!(reopened.openai_manual_node(second.id).unwrap(), None);
        reopened.save_openai_manual_node(first.id, None).unwrap();
        assert_eq!(storage.openai_manual_node(first.id).unwrap(), None);
        for invalid in ["".to_string(), "\n".to_string(), "x".repeat(2049)] {
            assert!(storage
                .save_openai_manual_node(first.id, Some(&invalid))
                .is_err());
            assert_eq!(storage.openai_manual_node(first.id).unwrap(), None);
        }
    }
}
