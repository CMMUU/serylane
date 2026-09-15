use crate::error::{AppError, AppResult};
use crate::mihomo_api::MihomoApiClient;
use crate::models::{ProfileRecord, RuntimePhase};
use crate::openai_stability::{StabilityManager, GROUP};
use crate::runtime::MihomoRuntime;
use crate::storage::AppStorage;
use serde_json::Value;
use tauri::{AppHandle, Manager};
use uuid::Uuid;

fn active_profile(
    app: &AppHandle,
    storage: &AppStorage,
    id: Uuid,
    revision: Uuid,
) -> AppResult<ProfileRecord> {
    let state = storage.state()?;
    if state.active_profile_id != Some(id)
        || state.active_revision_id != Some(revision)
        || app.state::<MihomoRuntime>().status(Some(app)).phase != RuntimePhase::Running
    {
        return Err(AppError::Conflict(
            "配置或运行状态已变化，请刷新后重新选点".into(),
        ));
    }
    storage.load_profile(id)
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
    let _permit = crate::user_rules::acquire_configuration(app)?;
    let storage = AppStorage::from_app(app)?;
    let state = storage.state()?;
    let mut payload = MihomoApiClient::new(&storage.settings()?)?
        .proxies()
        .await?;
    payload["profileId"] = serde_json::to_value(state.active_profile_id).unwrap_or(Value::Null);
    payload["revisionId"] = serde_json::to_value(state.active_revision_id).unwrap_or(Value::Null);
    if let Some(id) = state.active_profile_id {
        if let Some(revision) = state.active_revision_id {
            let costs = storage
                .openai_costs(id)?
                .resolve(&storage.load_revision_source(id, revision)?)?;
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
    }
    Ok(payload)
}

// This changes only the selected outbound. No config reload, connection close,
// core restart or request replay is performed by either command.
pub async fn select(
    app: &AppHandle,
    group: &str,
    node: Option<&str>,
    id: Uuid,
    revision: Uuid,
) -> AppResult<()> {
    let _permit = crate::user_rules::acquire_configuration(app)?;
    let storage = AppStorage::from_app(app)?;
    let profile = active_profile(app, &storage, id, revision)?;
    let api = MihomoApiClient::new(&storage.settings()?)?;
    let payload = api.proxies().await?;
    let runtime_group = &payload["proxies"][group];
    let managed = group == GROUP
        && profile.openai_policy.enabled
        && profile.openai_policy.stability_enabled
        && runtime_group["type"] == "Selector";
    if let Some(node) = node {
        validate_choice(runtime_group, node)?;
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
    let previous_manual = if managed {
        storage.openai_manual_node(id)?
    } else {
        None
    };
    if managed {
        // Persist first. On failure there is no controller write; if the
        // controller rejects the change, roll the intent back explicitly.
        storage.save_openai_manual_node(id, node)?;
    }
    let result = match node {
        Some(node) => api.select_proxy(group, node).await,
        None if managed => Ok(()), // Selector DELETE is rejected by Mihomo.
        None => api.clear_proxy_selection(group).await,
    };
    if let Err(error) = result {
        if managed {
            storage.save_openai_manual_node(id, previous_manual.as_deref())?;
        }
        return Err(error);
    }
    if group == GROUP {
        app.state::<StabilityManager>().invalidate_observations();
    }
    Ok(())
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
