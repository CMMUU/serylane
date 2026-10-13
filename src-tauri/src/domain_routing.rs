//! Domain rules are a separate local overlay; TLS and subscription source stay untouched.
use crate::{
    effective::build_effective_config_with_policy,
    error::{AppError, AppErrorDto, AppResult},
    local_routing::files,
    models::{RoutingMode, RuntimePhase},
    storage::AppStorage,
};
use serde::{Deserialize, Serialize};
use serde_yaml::{Mapping, Value};
use std::collections::BTreeSet;
use tauri::{AppHandle, Manager};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Preset {
    pub id: String,
    pub name: String,
    pub domains: Vec<String>,
    pub note: String,
    pub sources: Vec<String>,
}
pub fn presets() -> Vec<Preset> {
    serde_json::from_str(include_str!("../ai-route-presets.json"))
        .expect("bundled AI route registry is tested")
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Policy {
    /// Frozen expanded list: a future catalog update is applied only on explicit save/apply.
    pub domains: Vec<String>,
    pub target: String,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub custom_domains: Vec<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Selection {
    pub tools: Vec<String>,
    pub custom_domains: Vec<String>,
    pub target: String,
}
fn expand(input: Selection) -> AppResult<Policy> {
    let catalog = presets();
    if input.tools.len() > catalog.len() {
        return Err(AppError::InvalidInput("AI 工具数量无效".into()));
    }
    let mut seen = BTreeSet::new();
    let mut domains = vec![];
    for id in &input.tools {
        if !seen.insert(id) {
            return Err(AppError::InvalidInput("AI 工具重复".into()));
        }
        let preset = catalog
            .iter()
            .find(|p| &p.id == id)
            .ok_or_else(|| AppError::InvalidInput("AI 工具清单已变化，请重新读取".into()))?;
        domains.extend(preset.domains.clone());
    }
    let custom = normalize(Policy {
        domains: input.custom_domains,
        target: input.target,
        tools: vec![],
        custom_domains: vec![],
    })?;
    domains.extend(custom.domains.clone());
    let mut unique = BTreeSet::new();
    domains.retain(|d| unique.insert(d.clone()));
    normalize(Policy {
        domains,
        target: custom.target,
        tools: input.tools,
        custom_domains: custom.domains,
    })
}
impl Default for Policy {
    fn default() -> Self {
        expand(Selection {
            tools: vec!["openai".into()],
            custom_domains: vec![],
            target: String::new(),
        })
        .expect("default registry is valid")
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Document {
    pub schema_version: u32,
    pub revision: u64,
    pub enabled: bool,
    pub draft: Policy,
    pub applied: Option<Policy>,
}
impl Default for Document {
    fn default() -> Self {
        Self {
            schema_version: 1,
            revision: 0,
            enabled: false,
            draft: Policy::default(),
            applied: None,
        }
    }
}
impl Document {
    pub fn read(storage: &AppStorage) -> AppResult<Self> {
        let Some(text) = files::read_optional(&storage.routing_dir().join("domains.json"))? else {
            return Ok(Self::default());
        };
        let mut doc: Self = serde_json::from_str(&text)
            .map_err(|_| AppError::Config("域名路由设置损坏，未自动重置".into()))?;
        if doc.schema_version != 1 || doc.enabled && doc.applied.is_none() {
            return Err(AppError::Config("域名路由版本或状态无效".into()));
        }
        doc.draft = normalize(doc.draft)?;
        if doc.draft.tools.is_empty() && doc.draft.custom_domains.is_empty() {
            doc.draft.custom_domains = doc.draft.domains.clone();
        }
        doc.applied = doc.applied.map(normalize).transpose()?;
        Ok(doc)
    }
    fn write(&self, storage: &AppStorage) -> AppResult<()> {
        files::private_directory(&storage.routing_dir())?;
        files::atomic_write(
            &storage.routing_dir().join("domains.json"),
            &serde_json::to_string_pretty(self)
                .map_err(|_| AppError::Io("保存域名路由失败".into()))?,
        )
    }
    fn bump(&mut self, expected: u64) -> AppResult<()> {
        if self.revision != expected {
            return Err(AppError::Conflict(
                "路由设置已变化，请重新读取；草稿未覆盖".into(),
            ));
        }
        self.revision = self
            .revision
            .checked_add(1)
            .ok_or_else(|| AppError::Conflict("路由修订号已达上限".into()))?;
        Ok(())
    }
}
pub fn normalize(mut policy: Policy) -> AppResult<Policy> {
    if policy.domains.len() > 100
        || policy.target.len() > 2048
        || policy.target.chars().any(char::is_control)
    {
        return Err(AppError::InvalidInput(
            "域名最多 100 条，目标组名称须有效".into(),
        ));
    }
    let mut seen = BTreeSet::new();
    for row in &mut policy.domains {
        let raw = row.trim().to_ascii_lowercase();
        let raw = raw.strip_prefix("https://").unwrap_or(&raw);
        let raw = raw
            .strip_suffix("/*")
            .or_else(|| raw.strip_suffix('/'))
            .unwrap_or(raw);
        let host = raw.strip_prefix("*.").unwrap_or(raw);
        if host.len() > 253
            || !host.contains('.')
            || host.parse::<std::net::IpAddr>().is_ok()
            || host.split('.').any(|part| {
                part.is_empty()
                    || part.len() > 63
                    || part.starts_with('-')
                    || part.ends_with('-')
                    || !part.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
            })
        {
            return Err(AppError::InvalidInput("请填域名或 *.子域通配符；不支持 IP、端口、账号、查询参数或路径筛选。HTTPS 下按域名覆盖所有路径".into()));
        }
        *row = raw.to_string();
        if !seen.insert(row.clone()) {
            return Err(AppError::InvalidInput("域名名单重复，请合并后保存".into()));
        }
    }
    policy.target = policy.target.trim().to_string();
    Ok(policy)
}
pub fn targets(root: &Mapping) -> Vec<String> {
    root.get(Value::String("proxy-groups".into()))
        .and_then(Value::as_sequence)
        .into_iter()
        .flatten()
        .filter_map(|g| g.get("name").and_then(Value::as_str))
        .filter(|n| !matches!(*n, "DIRECT" | "REJECT" | "GLOBAL"))
        .map(str::to_string)
        .collect()
}
pub fn merge(
    root: &mut Mapping,
    doc: &Document,
    summary: &mut crate::config::ProfileSummary,
) -> AppResult<()> {
    if !doc.enabled {
        return Ok(());
    }
    let policy = doc
        .applied
        .as_ref()
        .ok_or_else(|| AppError::Config("启用的域名路由缺少已应用设置".into()))?;
    let valid = targets(root).contains(&policy.target);
    let target = if valid { &policy.target } else { "REJECT" };
    if !valid {
        summary.warnings.push(
            "域名路由目标组在当前配置中不存在；匹配流量拒绝连接，请重新选择出口或关闭域名路由"
                .into(),
        );
    }
    let rules = root
        .entry(Value::String("rules".into()))
        .or_insert_with(|| Value::Sequence(vec![]))
        .as_sequence_mut()
        .ok_or_else(|| AppError::Config("rules 必须是数组".into()))?;
    let prefix = policy.domains.iter().map(|domain| {
        Value::String(format!(
            "{},{},{}",
            if domain.starts_with("*.") {
                "DOMAIN-WILDCARD"
            } else {
                "DOMAIN"
            },
            domain,
            target
        ))
    });
    *rules = prefix.chain(std::mem::take(rules)).collect();
    summary.rule_count = rules.len();
    Ok(())
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub document: Document,
    pub presets: Vec<Preset>,
    pub targets: Vec<String>,
    pub profile_id: Option<Uuid>,
    pub profile_revision: Option<Uuid>,
    pub status: String,
    pub warning: String,
    pub user_rule_count: usize,
}
fn state(app: &AppHandle) -> AppResult<Snapshot> {
    let storage = AppStorage::from_app(app)?;
    let document = Document::read(&storage)?;
    let current = storage.state()?;
    let mut groups = vec![];
    let mut mode = RoutingMode::Rule;
    if let (Some(id), Some(revision)) = (current.active_profile_id, current.active_revision_id) {
        let profile = storage.load_profile(id)?;
        mode = profile.routing_mode;
        let source = storage.load_revision_source(id, revision)?;
        let mut settings = storage.settings()?;
        settings.domain_routing.enabled = false;
        let config = build_effective_config_with_policy(
            &source,
            &settings,
            mode,
            Some(&profile.openai_policy),
        )?;
        let root: Value = serde_yaml::from_str(&config.yaml)
            .map_err(|_| AppError::Config("读取有效配置失败".into()))?;
        groups = targets(
            root.as_mapping()
                .ok_or_else(|| AppError::Config("配置不是对象".into()))?,
        );
    }
    let valid = document
        .applied
        .as_ref()
        .is_some_and(|p| groups.contains(&p.target));
    let running = app
        .state::<crate::runtime::MihomoRuntime>()
        .status(Some(app))
        .phase
        == RuntimePhase::Running;
    let status = if !document.enabled {
        "已关闭"
    } else if !valid {
        "需要重新选择出口 · 匹配流量拒绝连接"
    } else if !running {
        "已配置 · 等待核心启动"
    } else if mode != RoutingMode::Rule {
        "已配置 · 当前非规则模式，不生效"
    } else {
        "已应用 · 仅处理进入代理核心的流量"
    }
    .to_string();
    Ok(Snapshot { document, presets: presets(), targets: groups, profile_id: current.active_profile_id, profile_revision: current.active_revision_id,
        status, warning: "域名路由优先于本地规则、AI 规则与订阅规则；未匹配流量保持原规则。*.example.com 仅匹配子域，根域须另列；覆盖 HTTPS/WSS 的所有路径，不解密内容，不修改 Codex 配置。".into(), user_rule_count: storage.user_rules()?.rules.len() })
}
#[tauri::command]
pub fn domain_route_status(app: AppHandle) -> Result<Snapshot, AppErrorDto> {
    state(&app).map_err(|e| e.dto())
}
#[tauri::command]
pub fn save_domain_route(
    app: AppHandle,
    policy: Selection,
    expected_revision: u64,
) -> Result<Snapshot, AppErrorDto> {
    (|| -> AppResult<_> {
        let _permit = crate::user_rules::acquire_configuration(&app)?;
        let storage = AppStorage::from_app(&app)?;
        let mut document = Document::read(&storage)?;
        document.bump(expected_revision)?;
        document.draft = expand(policy)?;
        document.write(&storage)?;
        state(&app)
    })()
    .map_err(|e| e.dto())
}
#[tauri::command]
pub async fn apply_domain_route(
    app: AppHandle,
    enabled: bool,
    expected_revision: u64,
    profile_id: Option<Uuid>,
    profile_revision: Option<Uuid>,
    confirmed: bool,
) -> Result<Snapshot, AppErrorDto> {
    let work = async {
        if !confirmed {
            return Err(AppError::InvalidInput("应用域名路由需要确认".into()));
        }
        let permit = crate::user_rules::acquire_configuration(&app)?;
        let storage = AppStorage::from_app(&app)?;
        let old = Document::read(&storage)?;
        let mut next = old.clone();
        next.bump(expected_revision)?;
        let current = state(&app)?;
        if (current.profile_id, current.profile_revision) != (profile_id, profile_revision) {
            return Err(AppError::Conflict(
                "活动配置已变化，请刷新后再应用路由".into(),
            ));
        }
        if enabled
            && (next.draft.domains.is_empty() || !current.targets.contains(&next.draft.target))
        {
            return Err(AppError::InvalidInput(
                "请添加域名并明确选择当前配置中的出口策略组".into(),
            ));
        }
        next.enabled = enabled;
        if enabled {
            next.applied = Some(next.draft.clone());
        }
        if let (Some(id), Some(revision)) = (profile_id, profile_revision) {
            let profile = storage.load_profile(id)?;
            if enabled && profile.routing_mode != RoutingMode::Rule {
                return Err(AppError::Conflict(
                    "请先切换为规则模式，再启用域名路由".into(),
                ));
            }
            let source = storage.load_revision_source(id, revision)?;
            let mut settings = storage.settings()?;
            settings.domain_routing = next.clone();
            let candidate = build_effective_config_with_policy(
                &source,
                &settings,
                profile.routing_mode,
                Some(&profile.openai_policy),
            )?;
            let previous_config = storage.active_runtime_config()?;
            crate::user_rules::apply_profile_config(
                &app,
                &storage,
                &candidate.yaml,
                || {
                    if let Err(error) = next
                        .write(&storage)
                        .and_then(|()| storage.restore_active_runtime_config(Some(&candidate.yaml)))
                    {
                        old.write(&storage)?;
                        storage.restore_active_runtime_config(previous_config.as_deref())?;
                        return Err(error);
                    }
                    Ok(())
                },
                &permit,
            )
            .await?;
        } else if enabled {
            return Err(AppError::Conflict("请先选用配置".into()));
        } else {
            next.write(&storage)?;
        }
        state(&app)
    };
    work.await.map_err(|e: AppError| e.dto())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn presets_and_custom_share_one_deduplicated_overlay() {
        for preset in presets() {
            normalize(Policy {
                domains: preset.domains,
                target: String::new(),
                tools: vec![],
                custom_domains: vec![],
            })
            .unwrap();
        }
        let p = expand(Selection {
            tools: vec!["openai".into(), "claude".into()],
            custom_domains: vec!["OPENAI.com".into(), "*.example.com".into()],
            target: "PROXY".into(),
        })
        .unwrap();
        assert_eq!(p.domains.len(), 19);
        assert_eq!(p.custom_domains, vec!["openai.com", "*.example.com"]);
        let off = expand(Selection {
            tools: vec![],
            custom_domains: p.custom_domains.clone(),
            target: p.target,
        })
        .unwrap();
        assert_eq!(
            off.domains, p.custom_domains,
            "removing a tool preserves overlapping custom entries"
        );
        assert!(expand(Selection {
            tools: vec!["unknown".into()],
            custom_domains: vec![],
            target: String::new()
        })
        .is_err());
        assert!(expand(Selection {
            tools: vec!["openai".into(), "openai".into()],
            custom_domains: vec![],
            target: String::new()
        })
        .is_err());
    }
    #[test]
    fn route_overlay_roundtrips_without_enabling_on_draft_save() {
        let root = tempfile::tempdir().unwrap();
        let storage = AppStorage::from_root(root.path().to_path_buf()).unwrap();
        let mut doc = Document::read(&storage).unwrap();
        assert!(!doc.enabled);
        doc.bump(0).unwrap();
        doc.draft.target = "PROXY".into();
        doc.write(&storage).unwrap();
        let next = Document::read(&storage).unwrap();
        assert!(!next.enabled);
        assert!(next.applied.is_none());
        assert_eq!(next.revision, 1);
        assert!(doc.bump(0).is_err());
    }
    #[test]
    fn native_domain_rules_validate_without_starting_a_core() {
        let Some(binary) = std::env::var_os("MIHOMO_TEST_BINARY") else {
            return;
        };
        let source="proxies: []\nproxy-groups: [{name: PROXY, type: select, proxies: [DIRECT]}]\nrules: ['MATCH,DIRECT']";
        let mut settings = crate::models::AppSettings::default();
        settings.domain_routing.enabled = true;
        settings.domain_routing.applied = Some(Policy {
            target: "PROXY".into(),
            ..Policy::default()
        });
        let config =
            build_effective_config_with_policy(source, &settings, RoutingMode::Rule, None).unwrap();
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("domain-fixture.yaml");
        std::fs::write(&path, config.yaml).unwrap();
        crate::runtime::validate_file(std::path::Path::new(&binary), root.path(), &path).unwrap();
    }
    #[test]
    fn normalization_is_domain_only_and_bounded() {
        assert_eq!(
            normalize(Policy {
                domains: vec!["https://*.OpenAI.com/*".into()],
                target: "PROXY".into(),
                tools: vec![],
                custom_domains: vec![]
            })
            .unwrap()
            .domains,
            vec!["*.openai.com"]
        );
        for domain in [
            "*",
            "*.com",
            "https://api.openai.com/v1/*",
            "openai.com.evil/path",
            "127.0.0.1",
            "openai.com:443",
            "openai.com?token=x",
            "a@openai.com",
            "*openai.com",
            "a..com",
            "https://例子.com",
        ] {
            assert!(
                normalize(Policy {
                    domains: vec![domain.into()],
                    target: "P".into(),
                    tools: vec![],
                    custom_domains: vec![]
                })
                .is_err(),
                "{domain}"
            );
        }
    }
    #[test]
    fn off_is_noop_missing_target_rejects_and_original_rules_survive() {
        let source = "proxies: []\nproxy-groups: []\nrules: ['MATCH,DIRECT']";
        let mut root = serde_yaml::from_str::<Value>(source)
            .unwrap()
            .as_mapping()
            .unwrap()
            .clone();
        let original = root.clone();
        let mut summary = crate::config::inspect_profile(source).unwrap();
        let mut doc = Document::default();
        merge(&mut root, &doc, &mut summary).unwrap();
        assert_eq!(root, original);
        doc.enabled = true;
        doc.applied = Some(doc.draft.clone());
        merge(&mut root, &doc, &mut summary).unwrap();
        let rules = root[Value::String("rules".into())].as_sequence().unwrap();
        assert_eq!(rules.last().unwrap().as_str(), Some("MATCH,DIRECT"));
        assert_eq!(rules[0].as_str(), Some("DOMAIN,openai.com,REJECT"));
        assert_eq!(
            rules[1].as_str(),
            Some("DOMAIN-WILDCARD,*.openai.com,REJECT")
        );
    }
}
