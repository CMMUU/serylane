//! Exclusive proxy control: subscription groups or the AI policy overlay.
//! Codex route integration is a separate subsystem and is never changed here.
use crate::{
    error::{AppError, AppResult},
    models::AppSettings,
    storage::AppStorage,
};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProxyMode {
    Manual,
    Ai,
}
// v0.7.18 applied an enabled AI policy implicitly. Preserve that intent on upgrade;
// fresh installations explicitly default to Manual in AppSettings::default.
pub fn legacy_mode() -> ProxyMode {
    ProxyMode::Ai
}

// Read-only migration marker for the unreleased fixed-outbound preview. Do not
// silently turn its all-traffic route into subscription rules during an upgrade.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ManualOutbound {
    pub profile_id: Uuid,
    pub node_name: String,
    pub identity: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub mode: ProxyMode,
    pub revision: Uuid,
    pub active_profile_id: Option<Uuid>,
    pub legacy_selection: bool,
}
pub fn snapshot(storage: &AppStorage) -> AppResult<Snapshot> {
    let settings = storage.settings()?;
    Ok(Snapshot {
        mode: settings.proxy_mode,
        revision: settings.proxy_mode_revision,
        active_profile_id: storage.state()?.active_profile_id,
        legacy_selection: settings.manual_outbound.is_some(),
    })
}
pub fn require_proxy_mode(settings: &AppSettings) -> AppResult<()> {
    require_profile(settings, Uuid::nil())?;
    if settings.proxy_mode != ProxyMode::Ai {
        return Err(AppError::Conflict(
            "自选节点正在生效；请先在 AI 代理页确认切换，再生成或调整 AI 策略。".into(),
        ));
    }
    Ok(())
}
pub fn require_manual_mode(settings: &AppSettings) -> AppResult<()> {
    require_profile(settings, Uuid::nil())?;
    if settings.proxy_mode != ProxyMode::Manual {
        return Err(AppError::Conflict(
            "AI 代理正在生效；请先在自选节点页确认切换。".into(),
        ));
    }
    Ok(())
}
pub fn require_profile(settings: &AppSettings, _profile_id: Uuid) -> AppResult<()> {
    if settings.manual_outbound.is_some() {
        return Err(AppError::Conflict("检测到旧版固定出口设置，请在自选节点页确认迁移为订阅策略组选点；当前没有自动更换出口。".into()));
    }
    Ok(())
}
pub fn require_revision(settings: &AppSettings, expected: Uuid) -> AppResult<()> {
    if settings.proxy_mode_revision != expected {
        return Err(AppError::Conflict("选点方式已变化，请刷新后重试。".into()));
    }
    Ok(())
}
pub async fn set(
    app: &AppHandle,
    mode: ProxyMode,
    expected_revision: Uuid,
    confirmed: bool,
) -> AppResult<Snapshot> {
    if !confirmed {
        return Err(AppError::InvalidInput("请确认选点方式切换后再继续".into()));
    }
    let permit = crate::session_resume::acquire_manual_configuration(app).await?;
    let storage = AppStorage::from_app(app)?;
    let previous = storage.settings()?;
    require_revision(&previous, expected_revision)?;
    if previous.proxy_mode == mode && previous.manual_outbound.is_none() {
        return snapshot(&storage);
    }
    let mut next = previous.clone();
    next.proxy_mode = mode;
    next.proxy_mode_revision = Uuid::new_v4();
    next.manual_outbound = None;
    let state = storage.state()?;
    if state.active_profile_id.is_some() != state.active_revision_id.is_some() {
        return Err(AppError::Conflict(
            "活动配置状态不完整，请先重新选用配置。".into(),
        ));
    }
    if let (Some(id), Some(revision)) = (state.active_profile_id, state.active_revision_id) {
        let profile = storage.load_profile(id)?;
        let source = storage.load_revision_source(id, revision)?;
        let policy = storage.load_revision(id, revision)?.openai_policy;
        let candidate = crate::effective::build_effective_config_with_policy(
            &source,
            &next,
            profile.routing_mode,
            Some(&policy),
        )?;
        crate::user_rules::apply_profile_config(
            app,
            &storage,
            &candidate.yaml,
            || commit_selection(&storage, &previous, &next, id, revision, &candidate.yaml),
            &permit,
        )
        .await?;
    } else {
        storage.save_settings(&next)?;
    }
    // Invalidate both pending probes and a generate→apply operation from the old
    // mode. A fresh mode revision also rejects late writes after switching back.
    let _ = app
        .state::<crate::openai_policy::OpenAiPolicyTaskManager>()
        .cancel();
    app.state::<crate::openai_stability::StabilityManager>()
        .update_policy_status(app);
    snapshot(&storage)
}
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
    use crate::models::{
        OpenAiNodeScore, OpenAiPolicy, ProfileSource, PublicAppSettings, RoutingMode,
        ValidationReport,
    };
    const SOURCE: &str = "proxies:\n- {name: A, type: socks5, server: localhost, port: 1080}\n- {name: B, type: socks5, server: localhost, port: 1081}\nproxy-groups: [{name: Auto, type: select, proxies: [A, B]}]\nrules: ['MATCH,Auto']\n";
    #[test]
    fn fresh_installs_choose_groups_legacy_installs_preserve_policy_intent() {
        let fresh = AppSettings::default();
        assert_eq!(fresh.proxy_mode, ProxyMode::Manual);
        let mut json = serde_json::to_value(&fresh).unwrap();
        json.as_object_mut().unwrap().remove("proxyMode");
        json.as_object_mut().unwrap().remove("proxyModeRevision");
        let migrated: AppSettings = serde_json::from_value(json).unwrap();
        assert_eq!(migrated.proxy_mode, ProxyMode::Ai);
        assert_eq!(
            PublicAppSettings::from(&fresh)
                .merge_secret(&migrated)
                .proxy_mode,
            ProxyMode::Ai
        );
        assert!(require_proxy_mode(&fresh).is_err());
        assert!(require_manual_mode(&migrated).is_err());
    }
    #[test]
    fn manual_groups_preserve_rules_and_ai_preferences_without_applying_the_overlay() {
        let mut settings = AppSettings::default();
        settings.user_rules.push(crate::user_rules::UserRule {
            id: "rule".into(),
            enabled: true,
            rule: "DOMAIN,example.com,REJECT".into(),
            note: String::new(),
        });
        let policy = OpenAiPolicy {
            enabled: true,
            selected_nodes: ["A", "B"]
                .into_iter()
                .map(|name| OpenAiNodeScore {
                    name: name.into(),
                    latency_ms: 10,
                    jitter_ms: 1,
                    bandwidth_mbps: None,
                    score: 90.,
                    checked_at: chrono::Utc::now(),
                })
                .collect(),
            ..Default::default()
        };
        for mode in [RoutingMode::Rule, RoutingMode::Global, RoutingMode::Direct] {
            let build = crate::effective::build_effective_config_with_policy(
                SOURCE,
                &settings,
                mode,
                Some(&policy),
            )
            .unwrap();
            let doc: serde_yaml::Value = serde_yaml::from_str(&build.yaml).unwrap();
            assert_eq!(doc["rules"][0].as_str(), Some("DOMAIN,example.com,REJECT"));
            assert_eq!(doc["proxy-groups"][0]["name"].as_str(), Some("Auto"));
            assert!(!build.yaml.contains(crate::effective::OPENAI_GROUP_NAME));
            assert_eq!(doc["profile"]["store-selected"].as_bool(), Some(true));
            assert_eq!(doc["proxies"].as_sequence().unwrap().len(), 2);
            if let Some(binary) = std::env::var_os("MIHOMO_TEST_BINARY") {
                let dir = tempfile::tempdir().unwrap();
                let path = dir.path().join("manual.yaml");
                std::fs::write(&path, build.yaml).unwrap();
                crate::runtime::validate_file(std::path::Path::new(&binary), dir.path(), &path)
                    .unwrap();
            }
        }
        settings.proxy_mode = ProxyMode::Ai;
        let ai = crate::effective::build_effective_config_with_policy(
            SOURCE,
            &settings,
            RoutingMode::Rule,
            Some(&policy),
        )
        .unwrap();
        assert!(ai.yaml.contains(crate::effective::OPENAI_GROUP_NAME));
        assert!(policy.enabled); // preferences retained in both modes
    }
    #[test]
    fn old_tasks_are_rejected_even_after_switching_back_and_legacy_fixed_routes_need_confirmation()
    {
        let old = AppSettings::default();
        let mut next = old.clone();
        next.proxy_mode_revision = Uuid::new_v4();
        assert!(require_revision(&next, old.proxy_mode_revision).is_err());
        next.manual_outbound = Some(ManualOutbound {
            profile_id: Uuid::nil(),
            node_name: "A".into(),
            identity: "legacy".into(),
        });
        assert!(
            crate::effective::build_effective_config(SOURCE, &next, RoutingMode::Rule).is_err()
        );
    }
    #[test]
    fn failed_mode_commit_restores_disk_settings_and_runtime_yaml() {
        let dir = tempfile::tempdir().unwrap();
        let storage = AppStorage::from_root(dir.path().into()).unwrap();
        let profile = storage
            .create_profile(
                "test".into(),
                ProfileSource::Inline {
                    label: "test".into(),
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
        crate::profile_service::commit_active_selection(&storage, profile.id, revision.id, SOURCE)
            .unwrap();
        let previous = storage.settings().unwrap();
        let mut next = previous.clone();
        next.proxy_mode = ProxyMode::Ai;
        // An absent target revision fails after settings save and must roll back.
        assert!(commit_selection(
            &storage,
            &previous,
            &next,
            profile.id,
            Uuid::new_v4(),
            "new"
        )
        .is_err());
        assert_eq!(storage.settings().unwrap().proxy_mode, ProxyMode::Manual);
        assert_eq!(
            storage.active_runtime_config().unwrap().as_deref(),
            Some(SOURCE)
        );
    }
}
