//! Subscription-independent local rules. The JSON document is authoritative;
//! AppSettings only carries an in-memory overlay into every config builder.
use crate::config::ProfileSummary;
use crate::effective::{build_effective_config_with_policy, EffectiveConfig};
use crate::error::{AppError, AppResult};
use crate::mihomo_api::MihomoApiClient;
use crate::models::{AppSettings, OpenAiPolicy, RoutingMode, RuntimePhase};
use crate::runtime::{self, MihomoRuntime};
use crate::storage::AppStorage;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_yaml::{Mapping, Value};
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::{AppHandle, Manager};
use uuid::Uuid;

const MAX_RULES: usize = 1_000;
const MAX_TEXT_BYTES: usize = 512 * 1024;
const MAX_HISTORY: usize = 20;
// Keep the pre-RouteDeck wire format so existing rule exports still round-trip.
const METADATA_PREFIX: &str = "# mihomo-codex-rule:";
const EMPTY_SOURCE: &str = "proxies: []\nproxy-groups: []\nrules:\n  - MATCH,DIRECT\n";
const BUILTIN_TARGETS: &[&str] = &[
    "DIRECT",
    "REJECT",
    "REJECT-DROP",
    "COMPATIBLE",
    "PASS",
    "PASS-RULE",
    "GLOBAL",
];
const RULE_TYPES: &[&str] = &[
    "DOMAIN",
    "DOMAIN-SUFFIX",
    "DOMAIN-KEYWORD",
    "DOMAIN-REGEX",
    "DOMAIN-WILDCARD",
    "GEOSITE",
    "GEOIP",
    "SRC-GEOIP",
    "IP-ASN",
    "SRC-IP-ASN",
    "IP-CIDR",
    "IP-CIDR6",
    "SRC-IP-CIDR",
    "IP-SUFFIX",
    "SRC-IP-SUFFIX",
    "SRC-PORT",
    "DST-PORT",
    "IN-PORT",
    "DSCP",
    "PROCESS-NAME",
    "PROCESS-PATH",
    "PROCESS-NAME-REGEX",
    "PROCESS-PATH-REGEX",
    "PROCESS-NAME-WILDCARD",
    "PROCESS-PATH-WILDCARD",
    "NETWORK",
    "UID",
    "IN-TYPE",
    "IN-USER",
    "IN-NAME",
    "REMATCH-NAME",
    "SUB-RULE",
    "AND",
    "OR",
    "NOT",
    "RULE-SET",
    "MATCH",
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UserRule {
    pub id: String,
    pub enabled: bool,
    pub rule: String,
    #[serde(default)]
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UserRulesRevision {
    pub id: String,
    pub created_at: DateTime<Utc>,
    pub rules: Vec<UserRule>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UserRulesDocument {
    pub schema_version: u32,
    pub revision: u64,
    pub updated_at: DateTime<Utc>,
    pub rules: Vec<UserRule>,
    pub history: Vec<UserRulesRevision>,
}

impl Default for UserRulesDocument {
    fn default() -> Self {
        Self {
            schema_version: 1,
            revision: 0,
            updated_at: Utc::now(),
            rules: Vec::new(),
            history: Vec::new(),
        }
    }
}

impl UserRulesDocument {
    pub fn validate_storage(&self) -> AppResult<()> {
        if self.schema_version != 1 || self.history.len() > MAX_HISTORY {
            return Err(AppError::Io(
                "用户规则存储版本或历史记录无效；已保留原文件".to_string(),
            ));
        }
        let mut ids = BTreeSet::new();
        if normalize_rules(&self.rules)? != self.rules {
            return Err(AppError::Io(
                "用户规则存储格式无效；已保留原文件".to_string(),
            ));
        }
        for revision in &self.history {
            let id = revision
                .id
                .parse::<u64>()
                .map_err(|_| AppError::Io("用户规则历史编号无效；已保留原文件".to_string()))?;
            if id >= self.revision || !ids.insert(id) {
                return Err(AppError::Io(
                    "用户规则历史编号冲突；已保留原文件".to_string(),
                ));
            }
            if normalize_rules(&revision.rules)? != revision.rules {
                return Err(AppError::Io(
                    "用户规则历史格式无效；已保留原文件".to_string(),
                ));
            }
        }
        Ok(())
    }

    fn next(&self, rules: Vec<UserRule>, expected_revision: u64) -> AppResult<Self> {
        if self.revision != expected_revision {
            return Err(AppError::Conflict(
                "用户规则已被更新，请刷新后重新保存".to_string(),
            ));
        }
        let revision = self
            .revision
            .checked_add(1)
            .ok_or_else(|| AppError::Conflict("用户规则版本号已达到上限".to_string()))?;
        let mut history = self.history.clone();
        history.insert(
            0,
            UserRulesRevision {
                id: self.revision.to_string(),
                created_at: self.updated_at,
                rules: self.rules.clone(),
            },
        );
        history.truncate(MAX_HISTORY);
        Ok(Self {
            schema_version: 1,
            revision,
            updated_at: Utc::now(),
            rules,
            history,
        })
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UserRulesHistoryItem {
    pub id: String,
    pub created_at: DateTime<Utc>,
    pub count: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UserRulesState {
    pub revision: u64,
    pub rules: Vec<UserRule>,
    pub history: Vec<UserRulesHistoryItem>,
    pub targets: Vec<String>,
    pub warnings: Vec<String>,
    pub routing_mode: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UserRulesValidation {
    pub valid: bool,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
    pub normalized_rules: Vec<UserRule>,
    pub preview: String,
}

/// Shared by configuration mutations (including async policy reloads), not by
/// appearance edits. A permit owns its Arc and can safely span an await.
#[derive(Default)]
pub struct ConfigurationMutationGuard {
    running: Arc<AtomicBool>,
}

pub struct ConfigurationMutationPermit {
    running: Arc<AtomicBool>,
}

impl ConfigurationMutationGuard {
    fn acquire(&self) -> AppResult<ConfigurationMutationPermit> {
        self.running
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| AppError::Conflict("配置正在保存或切换，请完成后重试".to_string()))?;
        Ok(ConfigurationMutationPermit {
            running: self.running.clone(),
        })
    }
}

impl Drop for ConfigurationMutationPermit {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Release);
    }
}

pub fn acquire_configuration(app: &AppHandle) -> AppResult<ConfigurationMutationPermit> {
    app.state::<ConfigurationMutationGuard>().acquire()
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TextMetadata {
    #[serde(default)]
    id: String,
    #[serde(default = "enabled_by_default")]
    enabled: bool,
    #[serde(default)]
    note: String,
}

fn enabled_by_default() -> bool {
    true
}

pub fn parse_text(text: &str) -> AppResult<Vec<UserRule>> {
    if text.len() > MAX_TEXT_BYTES || text.lines().count() > MAX_RULES * 3 + 20 {
        return Err(AppError::InvalidInput(
            "规则文本超过 512 KiB 或行数限制".to_string(),
        ));
    }
    let first = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('#'))
        .unwrap_or("");
    if first.is_empty() {
        if text
            .lines()
            .any(|line| line.trim().starts_with(METADATA_PREFIX))
        {
            return Err(AppError::InvalidInput("规则元数据缺少对应规则".to_string()));
        }
        return Ok(Vec::new());
    }
    let yaml_mode = first.starts_with("rules:")
        || first.starts_with("- ")
        || first.starts_with('[')
        || first.starts_with('{')
        || first == "---";
    let raw_rules = if yaml_mode {
        let value: Value = serde_yaml::from_str(text).map_err(|_| {
            AppError::InvalidInput(
                "规则 YAML 解析失败；仅接受规则字符串数组或 rules 字段".to_string(),
            )
        })?;
        let sequence = match value {
            Value::Sequence(sequence) => sequence,
            Value::Mapping(mut mapping) if mapping.len() == 1 => mapping
                .remove(Value::String("rules".to_string()))
                .and_then(|value| value.as_sequence().cloned())
                .ok_or_else(|| {
                    AppError::InvalidInput(
                        "仅接受包含 rules 数组的 YAML；不接受其他配置字段".to_string(),
                    )
                })?,
            _ => {
                return Err(AppError::InvalidInput(
                    "仅接受规则数组或单独 rules 字段；不接受完整配置".to_string(),
                ))
            }
        };
        sequence
            .into_iter()
            .map(|value| match value {
                Value::String(rule) => Ok(rule),
                _ => Err(AppError::InvalidInput(
                    "rules 中每项必须为单行规则字符串".to_string(),
                )),
            })
            .collect::<AppResult<Vec<_>>>()?
    } else {
        text.lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .map(str::to_string)
            .collect()
    };
    if raw_rules.len() > MAX_RULES {
        return Err(AppError::InvalidInput("用户规则最多 1000 条".to_string()));
    }
    let mut metadata = Vec::new();
    let mut pending = None;
    for line in text.lines().map(str::trim) {
        if let Some(json) = line.strip_prefix(METADATA_PREFIX) {
            if pending.is_some() {
                return Err(AppError::InvalidInput(
                    "规则元数据重复或未跟随规则".to_string(),
                ));
            }
            pending = Some(
                serde_json::from_str::<TextMetadata>(json.trim())
                    .map_err(|_| AppError::InvalidInput("规则元数据无效".to_string()))?,
            );
        } else if !line.is_empty() && !line.starts_with('#') && line != "rules:" && line != "---" {
            metadata.push(pending.take());
        }
    }
    if pending.is_some() {
        return Err(AppError::InvalidInput(
            "最后一条规则元数据缺少对应规则".to_string(),
        ));
    }
    if metadata.iter().any(Option::is_some) && metadata.len() != raw_rules.len() {
        return Err(AppError::InvalidInput(
            "带元数据的文本需每行一条规则".to_string(),
        ));
    }
    let rules = raw_rules
        .into_iter()
        .enumerate()
        .map(|(index, rule)| {
            let meta = metadata
                .get_mut(index)
                .and_then(Option::take)
                .unwrap_or(TextMetadata {
                    enabled: true,
                    ..Default::default()
                });
            UserRule {
                id: meta.id,
                enabled: meta.enabled,
                rule,
                note: meta.note,
            }
        })
        .collect::<Vec<_>>();
    normalize_rules(&rules)
}

pub fn normalize_rules(rules: &[UserRule]) -> AppResult<Vec<UserRule>> {
    if rules.len() > MAX_RULES {
        return Err(AppError::InvalidInput("用户规则最多 1000 条".to_string()));
    }
    let mut ids = BTreeSet::new();
    let mut bytes = 0;
    rules
        .iter()
        .enumerate()
        .map(|(index, rule)| {
            let mut normalized = rule.clone();
            normalized.rule = rule.rule.trim().to_string();
            if normalized.id.is_empty() {
                normalized.id = Uuid::new_v4().to_string();
            }
            bytes += normalized.rule.len() + normalized.note.len() + normalized.id.len();
            if normalized.id.len() > 128
                || !normalized
                    .id
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
                || !ids.insert(normalized.id.clone())
            {
                return Err(AppError::InvalidInput(format!(
                    "第 {} 条规则编号无效或重复",
                    index + 1
                )));
            }
            if normalized.rule.len() > 4096
                || normalized.note.len() > 512
                || bytes > MAX_TEXT_BYTES
                || normalized.rule.chars().any(char::is_control)
                || normalized.note.contains('\0')
            {
                return Err(AppError::InvalidInput(format!(
                    "第 {} 条规则或备注超过长度限制或包含控制字符",
                    index + 1
                )));
            }
            let parsed = parse_rule(&normalized.rule)
                .map_err(|error| AppError::InvalidInput(format!("第 {} 条：{error}", index + 1)))?;
            if let Some((_, tail)) = normalized.rule.split_once(',') {
                normalized.rule = format!("{},{}", parsed.kind, tail);
            }
            Ok(normalized)
        })
        .collect()
}

struct ParsedRule<'a> {
    kind: String,
    payload: String,
    target: &'a str,
}

/// Match the bundled Mihomo 1.19.30 ParseRulePayload contract: regex/logical
/// payloads may contain commas; their target is the final token, without params.
fn parse_rule(rule: &str) -> Result<ParsedRule<'_>, &'static str> {
    let items: Vec<&str> = rule.split(',').map(str::trim).collect();
    let kind = items.first().unwrap_or(&"").to_ascii_uppercase();
    if !RULE_TYPES.contains(&kind.as_str()) {
        return Err("不支持的规则类型");
    }
    let special = matches!(
        kind.as_str(),
        "AND"
            | "OR"
            | "NOT"
            | "SUB-RULE"
            | "DOMAIN-REGEX"
            | "PROCESS-NAME-REGEX"
            | "PROCESS-PATH-REGEX"
    );
    let (payload, target) = if kind == "MATCH" {
        if items.len() != 2 {
            return Err("MATCH 格式应为 MATCH,目标策略");
        }
        (String::new(), items[1])
    } else if special {
        if items.len() < 3 {
            return Err("缺少匹配内容或目标策略");
        }
        (items[1..items.len() - 1].join(","), items[items.len() - 1])
    } else {
        if items.len() < 3
            || items[3..]
                .iter()
                .any(|value| !matches!(*value, "no-resolve" | "src"))
        {
            return Err("规则需包含类型、匹配内容和目标策略；附加参数仅接受 no-resolve/src");
        }
        (items[1].to_string(), items[2])
    };
    if target.is_empty() || (kind != "MATCH" && payload.is_empty()) {
        return Err("匹配内容或目标策略为空");
    }
    Ok(ParsedRule {
        kind,
        payload,
        target,
    })
}

fn names(root: &Mapping, key: &str) -> BTreeSet<String> {
    root.get(Value::String(key.to_string()))
        .and_then(Value::as_sequence)
        .into_iter()
        .flatten()
        .filter_map(Value::as_mapping)
        .filter_map(|item| {
            item.get(Value::String("name".to_string()))
                .and_then(Value::as_str)
        })
        .map(str::to_string)
        .collect()
}

fn target_names(root: &Mapping, active: bool) -> BTreeSet<String> {
    let builtins = if active {
        BUILTIN_TARGETS
    } else {
        &BUILTIN_TARGETS[..3]
    };
    let mut targets: BTreeSet<String> = builtins.iter().map(|name| (*name).to_string()).collect();
    targets.extend(names(root, "proxies"));
    targets.extend(names(root, "proxy-groups"));
    targets
}

fn missing_target(rule: &ParsedRule<'_>, root: &Mapping, targets: &BTreeSet<String>) -> bool {
    if rule.kind == "SUB-RULE" {
        !root
            .get(Value::String("sub-rules".to_string()))
            .and_then(Value::as_mapping)
            .is_some_and(|subrules| subrules.contains_key(Value::String(rule.target.to_string())))
    } else {
        !targets.contains(rule.target)
    }
}

pub fn merge_into_config(
    root: &mut Mapping,
    rules: &[UserRule],
    summary: &mut ProfileSummary,
) -> AppResult<()> {
    if rules.is_empty() {
        return Ok(());
    }
    let normalized = normalize_rules(rules)?;
    let targets = target_names(root, true);
    let mut enabled = Vec::new();
    let mut process_rules = false;
    for (index, rule) in normalized
        .iter()
        .enumerate()
        .filter(|(_, rule)| rule.enabled)
    {
        let parsed = parse_rule(&rule.rule).map_err(|error| AppError::Config(error.to_string()))?;
        if missing_target(&parsed, root, &targets) {
            return Err(AppError::Config(format!(
                "本地规则第 {} 条的目标在当前订阅中不存在；请停用或修改该规则",
                index + 1
            )));
        }
        process_rules |= parsed.kind.starts_with("PROCESS-")
            || parsed.payload.to_ascii_uppercase().contains("PROCESS-");
        enabled.push(Value::String(rule.rule.clone()));
    }
    let count = enabled.len();
    let existing = root
        .entry(Value::String("rules".to_string()))
        .or_insert_with(|| Value::Sequence(Vec::new()))
        .as_sequence_mut()
        .ok_or_else(|| AppError::Config("rules 必须是数组".to_string()))?;
    enabled.append(existing);
    *existing = enabled;
    if process_rules {
        root.insert(
            Value::String("find-process-mode".to_string()),
            Value::String("strict".to_string()),
        );
    }
    summary.rule_count += count;
    if count > 0 {
        summary.warnings.push(format!(
            "已在订阅和 OpenAI 规则之前应用 {count} 条本地用户规则"
        ));
    }
    Ok(())
}

#[derive(Clone)]
struct ActiveContext {
    source: String,
    settings: AppSettings,
    routing_mode: RoutingMode,
    policy: Option<OpenAiPolicy>,
    active: bool,
    running: bool,
    fingerprint: String,
}

fn capture_context(app: &AppHandle, storage: &AppStorage) -> AppResult<ActiveContext> {
    let settings = storage.settings()?;
    let state = storage.state()?;
    let runtime = app.state::<MihomoRuntime>().status(Some(app));
    if state.active_profile_id.is_some() != state.active_revision_id.is_some() {
        return Err(AppError::Config(
            "活动订阅状态不完整，请重新选择配置".to_string(),
        ));
    }
    let active = state.active_profile_id.zip(state.active_revision_id);
    let (source, routing_mode, policy, profile_marker) = if let Some((profile_id, revision_id)) =
        active
    {
        let profile = storage.load_profile(profile_id)?;
        (
            storage.load_revision_source(profile_id, revision_id)?,
            profile.routing_mode,
            Some(profile.openai_policy.clone()),
            serde_json::to_string(&profile).map_err(|error| AppError::Config(error.to_string()))?,
        )
    } else {
        (
            EMPTY_SOURCE.to_string(),
            RoutingMode::Rule,
            None,
            String::new(),
        )
    };
    let fingerprint = serde_json::json!({
        "active": active, "profile": profile_marker,
        "networkMode": settings.network_mode, "mixedPort": settings.mixed_port,
        "controllerPort": settings.controller_port, "secret": settings.controller_secret,
        "manualOutbound": settings.manual_outbound,
        "phase": runtime.phase, "pid": runtime.pid, "startedAt": runtime.started_at,
    })
    .to_string();
    Ok(ActiveContext {
        source,
        settings,
        routing_mode,
        policy,
        active: active.is_some(),
        running: runtime.phase == RuntimePhase::Running,
        fingerprint,
    })
}

fn effective_for(context: &ActiveContext, rules: &[UserRule]) -> AppResult<EffectiveConfig> {
    let mut settings = context.settings.clone();
    settings.user_rules = rules.to_vec();
    build_effective_config_with_policy(
        &context.source,
        &settings,
        context.routing_mode,
        context.policy.as_ref(),
    )
}

fn base_document(context: &ActiveContext) -> AppResult<Mapping> {
    let yaml = effective_for(context, &[])?.yaml;
    serde_yaml::from_str::<Value>(&yaml)
        .map_err(|error| AppError::Config(error.to_string()))?
        .as_mapping()
        .cloned()
        .ok_or_else(|| AppError::Config("配置根节点无效".to_string()))
}

fn warnings(context: &ActiveContext, rules: &[UserRule]) -> Vec<String> {
    let mut warnings = vec![
        "用户规则只处理进入 Mihomo 的连接；DIRECT 仍经过本地核心，不修改系统代理例外或 PAC。"
            .to_string(),
    ];
    if context.settings.manual_outbound.is_some() {
        warnings.push(
            "自选节点模式下用户分流规则暂不参与；保存不会切换出口，切回代理模式后恢复。".into(),
        );
    }
    if !context.active {
        warnings.push(
            "尚无活动订阅；仅校验 DIRECT/REJECT/REJECT-DROP 规则，选择订阅并启动后生效。"
                .to_string(),
        );
    } else if context.routing_mode != RoutingMode::Rule {
        warnings.push(
            "当前为 Global/Direct 模式，规则不会按 Rule 模式参与匹配；保存不会自动切换模式。"
                .to_string(),
        );
    }
    if context.active && !context.running {
        warnings.push("核心当前未运行；保存的用户规则将在下次启动时加载。".to_string());
    }
    if rules
        .iter()
        .any(|rule| rule.enabled && rule.rule.starts_with("MATCH,"))
    {
        warnings.push("用户 MATCH 规则会截断后续用户、OpenAI 及订阅规则。".to_string());
    }
    if rules
        .iter()
        .any(|rule| rule.enabled && rule.rule.contains("PROCESS-"))
    {
        warnings.push(
            "进程规则已启用 strict 查找；仅匹配本机可识别进程，不代表网关上的远端应用识别。"
                .to_string(),
        );
    }
    if rules.iter().any(|rule| !rule.enabled) {
        warnings.push("停用规则保留但不加载到核心；重新启用时会再次校验目标和语法。".to_string());
    }
    warnings
}

fn validate_in_context(
    context: &ActiveContext,
    rules: &[UserRule],
) -> AppResult<UserRulesValidation> {
    let normalized = match normalize_rules(rules) {
        Ok(rules) => rules,
        Err(error) => {
            return Ok(UserRulesValidation {
                valid: false,
                errors: vec![error.to_string()],
                warnings: Vec::new(),
                normalized_rules: Vec::new(),
                preview: String::new(),
            })
        }
    };
    let root = base_document(context)?;
    let targets = target_names(&root, context.active);
    let mut errors = Vec::new();
    let mut warnings = warnings(context, &normalized);
    for (index, rule) in normalized.iter().enumerate() {
        let parsed =
            parse_rule(&rule.rule).map_err(|error| AppError::InvalidInput(error.to_string()))?;
        if missing_target(&parsed, &root, &targets) {
            let message = format!(
                "第 {} 条规则的目标不存在于当前订阅；请选择现有策略或停用该规则",
                index + 1
            );
            if rule.enabled {
                errors.push(message);
            } else {
                warnings.push(message);
            }
        }
    }
    let preview = serde_yaml::to_string(&serde_json::json!({ "rules": normalized.iter()
        .filter(|rule| rule.enabled).map(|rule| rule.rule.as_str()).collect::<Vec<_>>() }))
    .map_err(|error| AppError::Config(error.to_string()))?;
    Ok(UserRulesValidation {
        valid: errors.is_empty(),
        errors,
        warnings,
        normalized_rules: normalized,
        preview,
    })
}

fn native_validate(app: &AppHandle, yaml: &str) -> AppResult<()> {
    let directory = app
        .path()
        .app_data_dir()
        .map_err(|error| AppError::Io(error.to_string()))?
        .join("runtime");
    std::fs::create_dir_all(&directory)?;
    runtime::set_private_directory_permissions(&directory)?;
    let candidate = directory.join(format!("user-rules-validation-{}.yaml", Uuid::new_v4()));
    let binary = runtime::resolve_binary(Some(app))
        .ok_or_else(|| AppError::Runtime("未找到 Mihomo sidecar".to_string()))?;
    runtime::write_private_file(&candidate, yaml.as_bytes())?;
    let result = runtime::validate_file(&binary, &directory, &candidate);
    let _ = std::fs::remove_file(candidate);
    result
}

fn state_for(document: &UserRulesDocument, context: &ActiveContext) -> AppResult<UserRulesState> {
    let validation = validate_in_context(context, &document.rules)?;
    let mut state_warnings = validation.warnings;
    state_warnings.extend(validation.errors);
    Ok(UserRulesState {
        revision: document.revision,
        rules: document.rules.clone(),
        history: document
            .history
            .iter()
            .map(|revision| UserRulesHistoryItem {
                id: revision.id.clone(),
                created_at: revision.created_at,
                count: revision.rules.len(),
            })
            .collect(),
        targets: target_names(&base_document(context)?, context.active)
            .into_iter()
            .collect(),
        warnings: state_warnings,
        routing_mode: context
            .active
            .then(|| context.routing_mode.as_mihomo_mode().to_string()),
    })
}

pub fn get(app: &AppHandle) -> AppResult<UserRulesState> {
    let storage = AppStorage::from_app(app)?;
    let document = storage.user_rules()?;
    state_for(&document, &capture_context(app, &storage)?)
}

pub async fn validate(app: &AppHandle, rules: Vec<UserRule>) -> AppResult<UserRulesValidation> {
    let storage = AppStorage::from_app(app)?;
    let context = capture_context(app, &storage)?;
    let mut validation = validate_in_context(&context, &rules)?;
    if validation.valid {
        let yaml = effective_for(&context, &validation.normalized_rules)?.yaml;
        let app = app.clone();
        let result = tauri::async_runtime::spawn_blocking(move || native_validate(&app, &yaml))
            .await
            .map_err(|error| AppError::Runtime(error.to_string()))?;
        if let Err(error) = result {
            validation.valid = false;
            validation.errors.push(runtime::redact(&error.to_string()));
        }
    }
    Ok(validation)
}

/// Callers hold the shared configuration permit. Validation and live reload
/// happen before selection changes; a failed disk commit restores the old core.
pub(crate) async fn apply_profile_config(
    app: &AppHandle,
    storage: &AppStorage,
    candidate: &str,
    commit: impl FnOnce() -> AppResult<()>,
    _permit: &ConfigurationMutationPermit,
) -> AppResult<()> {
    let context = capture_context(app, storage)?;
    let previous = match effective_for(&context, &context.settings.user_rules) {
        Ok(config) => config.yaml,
        Err(error) if context.settings.manual_outbound.is_some() => {
            // An interrupted selection or damaged source must not trap the user
            // in manual mode. Only an explicit, validated switch can use the last
            // saved runtime config as rollback; startup still fails closed.
            storage.active_runtime_config()?.ok_or(error)?
        }
        Err(error) => return Err(error),
    };
    validate_config(app, candidate).await?;
    let reloader = (context.active && context.running)
        .then(|| MihomoApiClient::new(&context.settings))
        .transpose()?;
    transact(reloader.as_ref(), candidate, &previous, commit, || {
        if capture_context(app, storage)?.fingerprint != context.fingerprint {
            return Err(AppError::Conflict(
                "活动订阅或核心状态已变化，请重试".to_string(),
            ));
        }
        Ok(())
    })
    .await
}

pub(crate) async fn validate_config(app: &AppHandle, candidate: &str) -> AppResult<()> {
    let app = app.clone();
    let candidate = candidate.to_string();
    tauri::async_runtime::spawn_blocking(move || native_validate(&app, &candidate))
        .await
        .map_err(|error| AppError::Runtime(error.to_string()))?
}

trait RuleReloader {
    async fn reload(&self, yaml: &str) -> AppResult<()>;
}

impl RuleReloader for MihomoApiClient {
    async fn reload(&self, yaml: &str) -> AppResult<()> {
        self.reload_config(yaml).await
    }
}

async fn restore_after_failure(
    reloader: &impl RuleReloader,
    previous: &str,
    error: AppError,
) -> AppError {
    match reloader.reload(previous).await {
        Ok(()) => error,
        Err(rollback) => AppError::Runtime(format!(
            "规则保存失败，且核心回滚失败：{}；{}",
            runtime::redact(&error.to_string()),
            runtime::redact(&rollback.to_string())
        )),
    }
}

async fn transact(
    reloader: Option<&impl RuleReloader>,
    candidate: &str,
    previous: &str,
    commit: impl FnOnce() -> AppResult<()>,
    check_context: impl Fn() -> AppResult<()>,
) -> AppResult<()> {
    check_context()?;
    if let Some(reloader) = reloader {
        if let Err(error) = reloader.reload(candidate).await {
            return Err(restore_after_failure(reloader, previous, error).await);
        }
        if let Err(error) = check_context().and_then(|()| commit()) {
            return Err(restore_after_failure(reloader, previous, error).await);
        }
    } else {
        check_context()?;
        commit()?;
    }
    Ok(())
}

pub async fn save(
    app: &AppHandle,
    rules: Vec<UserRule>,
    expected_revision: u64,
) -> AppResult<UserRulesState> {
    let _permit = acquire_configuration(app)?;
    save_with_permit(app, rules, expected_revision).await
}

pub async fn rollback(
    app: &AppHandle,
    revision_id: &str,
    expected_revision: u64,
) -> AppResult<UserRulesState> {
    let _permit = acquire_configuration(app)?;
    let document = AppStorage::from_app(app)?.user_rules()?;
    if document.revision != expected_revision {
        return Err(AppError::Conflict(
            "用户规则已被更新，请刷新后回滚".to_string(),
        ));
    }
    let rules = document
        .history
        .iter()
        .find(|revision| revision.id == revision_id)
        .ok_or_else(|| AppError::NotFound("所选用户规则历史已不存在".to_string()))?
        .rules
        .clone();
    save_with_permit(app, rules, expected_revision).await
}

async fn save_with_permit(
    app: &AppHandle,
    rules: Vec<UserRule>,
    expected_revision: u64,
) -> AppResult<UserRulesState> {
    let storage = AppStorage::from_app(app)?;
    let current = storage.user_rules()?;
    if current.revision != expected_revision {
        return Err(AppError::Conflict(
            "用户规则已被更新，请刷新后重新保存".to_string(),
        ));
    }
    let context = capture_context(app, &storage)?;
    let validation = validate_in_context(&context, &rules)?;
    if !validation.valid {
        return Err(AppError::InvalidInput(validation.errors.join("；")));
    }
    let next = current.next(validation.normalized_rules, expected_revision)?;
    let candidate = effective_for(&context, &next.rules)?.yaml;
    let previous = effective_for(&context, &current.rules)?.yaml;
    let previous_snapshot = storage.active_runtime_config()?;
    let native_app = app.clone();
    let native_yaml = candidate.clone();
    tauri::async_runtime::spawn_blocking(move || native_validate(&native_app, &native_yaml))
        .await
        .map_err(|error| AppError::Runtime(error.to_string()))??;
    let result = state_for(&next, &context)?;
    let reloader = (context.active && context.running)
        .then(|| MihomoApiClient::new(&context.settings))
        .transpose()?;
    transact(
        reloader.as_ref(),
        &candidate,
        &previous,
        || {
            let persist = storage.save_user_rules(&next).and_then(|()| {
                if context.active {
                    storage.restore_active_runtime_config(Some(&candidate))
                } else {
                    Ok(())
                }
            });
            if let Err(write_error) = persist {
                // Atomic writers may fail during post-replacement backup cleanup.
                // Restore the previous authoritative document before runtime rollback.
                let rollback_errors = [
                    storage.save_user_rules(&current),
                    storage.restore_active_runtime_config(previous_snapshot.as_deref()),
                ]
                .into_iter()
                .filter_map(Result::err)
                .map(|error| error.to_string())
                .collect::<Vec<_>>();
                if !rollback_errors.is_empty() {
                    return Err(AppError::Io(format!(
                        "规则持久化失败，恢复原文件也失败：{write_error}；{}",
                        rollback_errors.join("；")
                    )));
                }
                return Err(write_error);
            }
            Ok(())
        },
        || {
            let now = capture_context(app, &storage)?;
            if now.fingerprint != context.fingerprint
                || storage.user_rules()?.revision != expected_revision
            {
                return Err(AppError::Conflict(
                    "活动订阅、运行状态或规则版本已变化；请重新校验后保存".to_string(),
                ));
            }
            Ok(())
        },
    )
    .await?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effective::OPENAI_GROUP_NAME;
    use crate::models::{OpenAiNodeScore, PublicAppSettings};
    use std::cell::Cell;
    use std::path::PathBuf;
    use std::sync::Mutex;

    const SOURCE: &str = "proxies:\n  - {name: sample-a, type: socks5, server: 127.0.0.1, port: 1080}\n  - {name: sample-b, type: socks5, server: 127.0.0.1, port: 1081}\nproxy-groups:\n  - {name: PROXY, type: select, proxies: [sample-a, sample-b]}\nrules:\n  - MATCH,PROXY\n";

    fn rule(id: &str, text: &str) -> UserRule {
        UserRule {
            id: id.to_string(),
            enabled: true,
            rule: text.to_string(),
            note: String::new(),
        }
    }

    fn context(active: bool) -> ActiveContext {
        ActiveContext {
            source: if active { SOURCE } else { EMPTY_SOURCE }.to_string(),
            settings: AppSettings::default(),
            routing_mode: RoutingMode::Rule,
            policy: None,
            active,
            running: false,
            fingerprint: "fixture".to_string(),
        }
    }

    struct Fixture {
        root: PathBuf,
        storage: AppStorage,
    }
    impl Fixture {
        fn new() -> Self {
            let root =
                std::env::temp_dir().join(format!("routedeck-user-rules-{}", Uuid::new_v4()));
            let storage = AppStorage::from_root(root.clone()).expect("storage");
            Self { root, storage }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn parses_plain_and_rules_only_yaml_without_inserting_config() {
        for text in [
            "DOMAIN,example.com,DIRECT\nMATCH,REJECT",
            "rules:\n  - DOMAIN,example.com,DIRECT\n  - MATCH,REJECT",
            "- DOMAIN,example.com,DIRECT\n- MATCH,REJECT",
        ] {
            let rules = parse_text(text).expect("rule text");
            assert_eq!(rules.len(), 2);
            assert_eq!(rules[0].rule, "DOMAIN,example.com,DIRECT");
            assert!(rules.iter().all(|rule| rule.enabled));
        }
        for text in [
            "rules: []\nmixed-port: 7890",
            "{rules: [], proxies: []}",
            "rules: [123]",
            "rules:\n  - {name: anything}",
        ] {
            assert!(parse_text(text).is_err(), "reject full config: {text}");
        }
    }

    #[test]
    fn metadata_roundtrips_disabled_notes_ids_and_order() {
        let text = "# mihomo-codex-rule: {\"id\":\"one\",\"enabled\":false,\"note\":\"先保留\"}\nDOMAIN,example.com,NOT_PRESENT\n# mihomo-codex-rule: {\"id\":\"two\",\"enabled\":true,\"note\":\"常用\"}\nPROCESS-NAME,example.exe,DIRECT";
        let rules = parse_text(text).expect("metadata");
        assert_eq!(rules[0].id, "one");
        assert!(!rules[0].enabled);
        assert_eq!(rules[0].note, "先保留");
        assert_eq!(rules[1].id, "two");
        assert!(rules[1].enabled);
        assert!(parse_text("# mihomo-codex-rule: {\"id\":\"one\",\"enabled\":false}").is_err());
        assert!(parse_text("# mihomo-codex-rule: {bad}\nMATCH,DIRECT").is_err());
    }

    #[test]
    fn metadata_preserved_in_block_yaml() {
        let rules = parse_text("rules:\n  # mihomo-codex-rule: {\"id\":\"one\",\"enabled\":false,\"note\":\"memo\"}\n  - 'DOMAIN,example.com,DIRECT'\n").expect("yaml metadata");
        assert_eq!(
            rules,
            vec![UserRule {
                id: "one".into(),
                enabled: false,
                rule: "DOMAIN,example.com,DIRECT".into(),
                note: "memo".into()
            }]
        );
    }

    #[test]
    fn enforces_limits_ids_and_rule_grammar() {
        assert!(parse_text(&"x".repeat(MAX_TEXT_BYTES + 1)).is_err());
        assert!(normalize_rules(&vec![rule("same", "MATCH,DIRECT"); MAX_RULES + 1]).is_err());
        assert!(
            normalize_rules(&[rule("same", "MATCH,DIRECT"), rule("same", "MATCH,REJECT")]).is_err()
        );
        for text in [
            "",
            "UNKNOWN,a,DIRECT",
            "MATCH,",
            "MATCH,a,b",
            "DOMAIN,,DIRECT",
            "DOMAIN,a,DIRECT,bad-flag",
            "DOMAIN,a,DIRECT\nMATCH,REJECT",
        ] {
            assert!(normalize_rules(&[rule("id", text)]).is_err(), "{text}");
        }
        let normalized =
            normalize_rules(&[rule("", "  domain,example.com,DIRECT  ")]).expect("normalize");
        assert!(!normalized[0].id.is_empty());
        assert_eq!(normalized[0].rule, "DOMAIN,example.com,DIRECT");
    }

    #[test]
    fn preserves_advanced_regex_and_logical_payload_commas() {
        for text in [
            "AND,((DOMAIN,example.com),(NETWORK,UDP)),DIRECT",
            "DOMAIN-REGEX,^(a|b){1,2}\\.example$,PROXY",
            "PROCESS-PATH-REGEX,(?i).*Application\\\\chrome.*,DIRECT",
            "IP-CIDR,203.0.113.0/24,DIRECT,no-resolve",
        ] {
            let normalized = normalize_rules(&[rule("id", text)]).expect("advanced");
            assert_eq!(normalized[0].rule, text);
            assert_eq!(
                parse_rule(text).expect("parse").target,
                if text.contains(",PROXY") {
                    "PROXY"
                } else {
                    "DIRECT"
                }
            );
        }
    }

    #[test]
    fn validates_active_targets_and_warns_for_disabled_missing_target() {
        let active = context(true);
        assert!(
            validate_in_context(&active, &[rule("id", "DOMAIN,example.com,PROXY")])
                .expect("validate")
                .valid
        );
        let missing = rule("id", "DOMAIN,example.com,not-in-this-subscription");
        assert!(
            !validate_in_context(&active, std::slice::from_ref(&missing))
                .expect("missing")
                .valid
        );
        let disabled = UserRule {
            enabled: false,
            ..missing
        };
        let result = validate_in_context(&active, &[disabled]).expect("disabled");
        assert!(result.valid);
        assert!(result
            .warnings
            .iter()
            .any(|message| message.contains("目标不存在")));
    }

    #[test]
    fn no_active_profile_only_exposes_standalone_targets_and_rule_only_preview() {
        let context = context(false);
        let valid = validate_in_context(
            &context,
            &[
                rule("one", "DOMAIN,example.com,DIRECT"),
                rule("two", "MATCH,REJECT"),
            ],
        )
        .expect("no active");
        assert!(valid.valid);
        assert!(!valid.preview.contains("secret"));
        assert!(!valid.preview.contains("proxy-groups"));
        assert!(valid.preview.starts_with("rules:"));
        assert!(
            !validate_in_context(&context, &[rule("id", "MATCH,GLOBAL")])
                .expect("no group")
                .valid
        );
        let state = state_for(&UserRulesDocument::default(), &context).expect("state");
        assert_eq!(state.targets, vec!["DIRECT", "REJECT", "REJECT-DROP"]);
        assert_eq!(state.routing_mode, None);
    }

    #[test]
    fn rule_order_precedes_ai_and_subscription_without_mutating_source() {
        let mut context = context(true);
        context.policy = Some(OpenAiPolicy {
            enabled: true,
            selected_nodes: ["sample-a", "sample-b"]
                .into_iter()
                .map(|name| OpenAiNodeScore {
                    name: name.into(),
                    latency_ms: 10,
                    jitter_ms: 1,
                    bandwidth_mbps: None,
                    score: 90.0,
                    checked_at: Utc::now(),
                })
                .collect(),
            ..Default::default()
        });
        let entries = [
            rule("first", "DOMAIN,openai.com,DIRECT"),
            rule("second", "DOMAIN,example.com,REJECT"),
        ];
        let effective = effective_for(&context, &entries).expect("merge");
        let yaml: Value = serde_yaml::from_str(&effective.yaml).expect("yaml");
        let rules = yaml["rules"].as_sequence().expect("rules");
        assert_eq!(rules[0].as_str(), Some(entries[0].rule.as_str()));
        assert_eq!(rules[1].as_str(), Some(entries[1].rule.as_str()));
        assert_eq!(
            rules[2].as_str(),
            Some(format!("DOMAIN-SUFFIX,openai.com,{OPENAI_GROUP_NAME}").as_str())
        );
        assert_eq!(rules.last().and_then(Value::as_str), Some("MATCH,PROXY"));
        assert_eq!(context.source, SOURCE);
        assert_eq!(
            effective_for(&context, &entries).expect("rebuild").yaml,
            effective.yaml
        );
    }

    #[test]
    fn reordered_enabled_rules_and_strict_process_lookup_are_applied() {
        let context = context(true);
        let mut disabled = rule("off", "PROCESS-NAME,off.exe,DIRECT");
        disabled.enabled = false;
        let entries = [
            rule("second", "DOMAIN,b.example,DIRECT"),
            disabled.clone(),
            rule("first", "DOMAIN,a.example,DIRECT"),
        ];
        let config = effective_for(&context, &entries).expect("reorder");
        let yaml: Value = serde_yaml::from_str(&config.yaml).expect("yaml");
        assert!(yaml["find-process-mode"].is_null());
        assert_eq!(yaml["rules"][0].as_str(), Some("DOMAIN,b.example,DIRECT"));
        assert_eq!(yaml["rules"][1].as_str(), Some("DOMAIN,a.example,DIRECT"));
        let config = effective_for(
            &context,
            &[rule(
                "process",
                "AND,((process-name,game.exe),(NETWORK,UDP)),DIRECT",
            )],
        )
        .expect("process");
        let yaml: Value = serde_yaml::from_str(&config.yaml).expect("yaml");
        assert_eq!(yaml["find-process-mode"].as_str(), Some("strict"));
    }

    #[test]
    fn global_rules_survive_subscription_changes_and_missing_targets_fail_closed() {
        let first = context(true);
        let mut second = first.clone();
        second.source = SOURCE.replace("PROXY", "SECOND");
        let rules = [rule("app", "PROCESS-NAME,game.exe,DIRECT")];
        for context in [&first, &second] {
            assert!(effective_for(context, &rules)
                .expect("subscription")
                .yaml
                .contains("PROCESS-NAME,game.exe,DIRECT"));
        }
        let missing = [rule("group", "DOMAIN,example.com,PROXY")];
        assert!(effective_for(&second, &missing).is_err());
    }

    #[test]
    fn global_and_direct_modes_warn_without_being_changed() {
        for mode in [RoutingMode::Global, RoutingMode::Direct] {
            let mut context = context(true);
            context.routing_mode = mode;
            let result =
                validate_in_context(&context, &[rule("id", "MATCH,DIRECT")]).expect("validate");
            assert!(result
                .warnings
                .iter()
                .any(|warning| warning.contains("不会自动切换")));
            assert_eq!(context.routing_mode, mode);
        }
    }

    #[test]
    fn history_caps_twenty_and_rollback_creates_a_new_revision() {
        let mut document = UserRulesDocument::default();
        for number in 0..25 {
            document = document
                .next(vec![rule(&format!("row-{number}"), "MATCH,DIRECT")], number)
                .expect("next");
        }
        assert_eq!(document.history.len(), 20);
        assert_eq!(document.history[0].id, "24");
        assert_eq!(document.history.last().expect("oldest").id, "5");
        let restored = document
            .next(document.history[0].rules.clone(), 25)
            .expect("rollback");
        assert_eq!(restored.revision, 26);
        assert_eq!(restored.rules[0].id, "row-23");
        assert_eq!(restored.history[0].rules[0].id, "row-24");
        restored.validate_storage().expect("history schema");
        assert!(restored.next(vec![], 25).is_err());
    }

    #[test]
    fn storage_overlay_preserves_settings_and_secret_without_duplicate_authority() {
        let fixture = Fixture::new();
        let original = AppSettings {
            mixed_port: 17890,
            controller_port: 19090,
            controller_secret: "fixture-private-control-value".into(),
            ..Default::default()
        };
        fixture.storage.save_settings(&original).expect("settings");
        let document = UserRulesDocument::default()
            .next(vec![rule("app", "PROCESS-NAME,game.exe,DIRECT")], 0)
            .expect("document");
        fixture
            .storage
            .save_user_rules(&document)
            .expect("rules store");
        let settings = fixture.storage.settings().expect("overlay");
        assert_eq!(settings.user_rules, document.rules);
        assert_eq!(settings.controller_secret, original.controller_secret);
        let merged = PublicAppSettings::from(&settings).merge_secret(&settings);
        fixture
            .storage
            .save_settings(&merged)
            .expect("save unrelated settings");
        assert_eq!(
            fixture.storage.user_rules().expect("rules remain").revision,
            1
        );
        let json: serde_json::Value = serde_json::from_slice(
            &std::fs::read(fixture.root.join("settings.json")).expect("read"),
        )
        .expect("json");
        assert!(json.get("userRules").is_none());
        assert_eq!(json["mixedPort"], 17890);
        assert_eq!(json["controllerSecret"], original.controller_secret);
        let public = serde_json::to_value(PublicAppSettings::from(&settings)).expect("public");
        assert!(public.get("controllerSecret").is_none());
        assert!(public.get("userRules").is_none());
    }

    #[test]
    fn corrupted_store_errors_without_resetting_or_rewriting() {
        let fixture = Fixture::new();
        let path = fixture.root.join("user-rules.json");
        let bytes = b"{broken";
        std::fs::write(&path, bytes).expect("corrupt fixture");
        assert!(fixture.storage.user_rules().is_err());
        assert_eq!(std::fs::read(&path).expect("retained"), bytes);
    }

    #[test]
    fn history_roundtrips_and_rejects_invalid_schema_or_duplicate_history() {
        let fixture = Fixture::new();
        let document = UserRulesDocument::default()
            .next(vec![rule("one", "MATCH,DIRECT")], 0)
            .expect("first")
            .next(vec![rule("two", "MATCH,REJECT")], 1)
            .expect("second");
        fixture.storage.save_user_rules(&document).expect("save");
        let loaded = fixture.storage.user_rules().expect("load");
        assert_eq!(loaded.revision, 2);
        assert_eq!(loaded.history.len(), 2);
        assert_eq!(loaded.history[0].rules[0].id, "one");
        let mut invalid = loaded.clone();
        invalid.schema_version = 9;
        assert!(fixture.storage.save_user_rules(&invalid).is_err());
        invalid = loaded;
        invalid.history.push(invalid.history[0].clone());
        assert!(fixture.storage.save_user_rules(&invalid).is_err());
        assert_eq!(fixture.storage.user_rules().expect("unchanged").revision, 2);
    }

    #[test]
    fn shared_guard_blocks_both_directions_and_releases_on_drop() {
        let guard = ConfigurationMutationGuard::default();
        let permit = guard.acquire().expect("first mutation");
        assert!(guard.acquire().is_err());
        drop(permit);
        let _next = guard.acquire().expect("next mutation");
    }

    struct FakeReloader {
        calls: Mutex<Vec<String>>,
        failures: BTreeSet<usize>,
    }
    impl FakeReloader {
        fn new(failures: &[usize]) -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                failures: failures.iter().copied().collect(),
            }
        }
    }
    impl RuleReloader for FakeReloader {
        async fn reload(&self, yaml: &str) -> AppResult<()> {
            let mut calls = self.calls.lock().expect("calls");
            calls.push(yaml.to_string());
            if self.failures.contains(&calls.len()) {
                Err(AppError::Runtime("fixture apply failure".into()))
            } else {
                Ok(())
            }
        }
    }

    #[tokio::test]
    async fn failed_reload_restores_old_runtime_without_committing() {
        let runtime = FakeReloader::new(&[1]);
        let committed = Cell::new(false);
        let result = transact(
            Some(&runtime),
            "new",
            "old",
            || {
                committed.set(true);
                Ok(())
            },
            || Ok(()),
        )
        .await;
        assert!(result.is_err());
        assert!(!committed.get());
        assert_eq!(*runtime.calls.lock().expect("calls"), ["new", "old"]);
    }

    #[tokio::test]
    async fn persistence_failure_restores_runtime_and_reports_failed_rollback() {
        for failures in [&[][..], &[2][..]] {
            let runtime = FakeReloader::new(failures);
            let result = transact(
                Some(&runtime),
                "new",
                "old",
                || Err(AppError::Io("fixture disk full".into())),
                || Ok(()),
            )
            .await
            .expect_err("commit fail");
            assert_eq!(*runtime.calls.lock().expect("calls"), ["new", "old"]);
            assert_eq!(
                result.to_string().contains("核心回滚失败"),
                !failures.is_empty()
            );
        }
    }

    #[tokio::test]
    async fn changed_context_prevents_commit_and_restores_old_runtime() {
        let runtime = FakeReloader::new(&[]);
        let checks = Cell::new(0);
        let committed = Cell::new(false);
        let result = transact(
            Some(&runtime),
            "new",
            "old",
            || {
                committed.set(true);
                Ok(())
            },
            || {
                checks.set(checks.get() + 1);
                if checks.get() == 2 {
                    Err(AppError::Conflict("fixture changed profile".into()))
                } else {
                    Ok(())
                }
            },
        )
        .await;
        assert!(result.is_err());
        assert!(!committed.get());
        assert_eq!(*runtime.calls.lock().expect("calls"), ["new", "old"]);
    }

    #[tokio::test]
    async fn successful_commit_occurs_only_after_runtime_acceptance() {
        let runtime = FakeReloader::new(&[]);
        let committed = Cell::new(false);
        transact(
            Some(&runtime),
            "new",
            "old",
            || {
                assert_eq!(*runtime.calls.lock().expect("calls"), ["new"]);
                committed.set(true);
                Ok(())
            },
            || Ok(()),
        )
        .await
        .expect("transaction");
        assert!(committed.get());
        assert_eq!(*runtime.calls.lock().expect("calls"), ["new"]);
    }

    #[test]
    #[ignore = "requires explicit MIHOMO_TEST_BINARY; executes -t only in a temporary fixture directory"]
    fn native_core_validates_isolated_rule_fixtures() {
        let binary = PathBuf::from(
            std::env::var("MIHOMO_TEST_BINARY").expect("explicit MIHOMO_TEST_BINARY"),
        );
        let fixture = Fixture::new();
        for (index, (text, valid)) in [
            ("DOMAIN,example.com,DIRECT", true),
            ("PROCESS-NAME,fixture.exe,DIRECT", true),
            ("AND,((DOMAIN,example.com),(NETWORK,UDP)),DIRECT", true),
            ("DOMAIN-REGEX,^(a|b){1,2}\\.example$,DIRECT", true),
            ("IP-CIDR,203.0.113.0/24,DIRECT,no-resolve", true),
            ("IP-CIDR,not-an-ip,DIRECT", false),
        ]
        .into_iter()
        .enumerate()
        {
            let yaml = effective_for(&context(true), &[rule("native", text)])
                .expect("build fixture")
                .yaml;
            let path = fixture.root.join(format!("native-{index}.yaml"));
            runtime::write_private_file(&path, yaml.as_bytes()).expect("write fixture");
            assert_eq!(
                runtime::validate_file(&binary, &fixture.root, &path).is_ok(),
                valid,
                "native rule {index}"
            );
        }
    }
}
