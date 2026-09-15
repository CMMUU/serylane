use crate::error::{AppError, AppErrorDto, AppResult};
use crate::storage::AppStorage;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use tauri::{AppHandle, Manager};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CostMode {
    #[default]
    Quality,
    Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeCost {
    pub identity: String,
    pub multiplier: f64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CostPreferences {
    pub revision: u64,
    pub mode: CostMode,
    pub max_multiplier: Option<f64>,
    pub allow_unknown: bool,
    pub nodes: BTreeMap<String, NodeCost>,
}
impl CostPreferences {
    pub fn validate(&self) -> AppResult<()> {
        if self.nodes.len() > 300
            || self.max_multiplier.is_some_and(|n| !valid_rate(n))
            || self.nodes.iter().any(|(name, n)| {
                name.is_empty()
                    || name.len() > 2048
                    || !valid_rate(n.multiplier)
                    || n.identity.len() != 64
                    || !n.identity.bytes().all(|b| b.is_ascii_hexdigit())
            })
        {
            return Err(AppError::InvalidInput(
                "倍率需在 0.01～1000 之间，最多设置 300 个显式节点".into(),
            ));
        }
        Ok(())
    }
    pub fn resolve(&self, source: &str) -> AppResult<ResolvedCosts> {
        self.validate()?;
        let identities = identities(source)?;
        Ok(ResolvedCosts {
            preferences: self.clone(),
            multipliers: identities
                .iter()
                .map(|(name, identity)| {
                    let rate = self
                        .nodes
                        .get(name)
                        .filter(|n| &n.identity == identity)
                        .map(|n| n.multiplier);
                    (name.clone(), rate)
                })
                .collect(),
        })
    }
}
fn valid_rate(n: f64) -> bool {
    n.is_finite() && (0.01..=1000.0).contains(&n)
}

// Only the name and an opaque fingerprint leave this parsing layer. Names are
// not parsed for "1x" or prices; quota headers cannot supply per-node rates.
fn identities(source: &str) -> AppResult<BTreeMap<String, String>> {
    let doc: serde_yaml::Value = serde_yaml::from_str(source)
        .map_err(|_| AppError::Config("无法读取节点成本清单".into()))?;
    let candidates = crate::openai_policy::candidate_names(source)?;
    let mut result = BTreeMap::new();
    for node in doc["proxies"].as_sequence().into_iter().flatten() {
        if let Some(name) = node["name"]
            .as_str()
            .filter(|name| candidates.iter().any(|c| c == name))
        {
            let bytes = serde_yaml::to_string(node)
                .map_err(|_| AppError::Config("无法识别节点身份".into()))?;
            result.insert(
                name.into(),
                format!("{:x}", Sha256::digest(bytes.as_bytes())),
            );
        }
    }
    Ok(result)
}

#[derive(Clone)]
pub struct ResolvedCosts {
    pub preferences: CostPreferences,
    pub multipliers: BTreeMap<String, Option<f64>>,
}
impl ResolvedCosts {
    pub fn value_mode(&self) -> bool {
        self.preferences.mode == CostMode::Value
    }
    pub fn multiplier(&self, name: &str) -> Option<f64> {
        self.multipliers.get(name).copied().flatten()
    }
    pub fn allowed(&self, name: &str) -> bool {
        !self.value_mode()
            || match self.multiplier(name) {
                Some(rate) => self
                    .preferences
                    .max_multiplier
                    .is_none_or(|max| rate <= max),
                None => self.preferences.allow_unknown,
            }
    }
    pub fn utility(&self, name: &str, quality: f64) -> f64 {
        if !self.value_mode() {
            return quality;
        }
        // Unknown prices never gain a fabricated 1x discount. They form a
        // separate fallback tier after sufficiently healthy, known-price nodes.
        self.multiplier(name)
            .map_or(-1.0 + quality.clamp(0.0, 100.0) / 101.0, |rate| {
                quality / rate.sqrt()
            })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CostRow {
    pub name: String,
    pub multiplier: Option<f64>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CostSnapshot {
    pub profile_id: Uuid,
    pub profile_revision: Uuid,
    pub revision: u64,
    pub mode: CostMode,
    pub max_multiplier: Option<f64>,
    pub allow_unknown: bool,
    pub nodes: Vec<CostRow>,
}
fn snapshot(storage: &AppStorage, id: Uuid) -> AppResult<CostSnapshot> {
    let profile = storage.load_profile(id)?;
    let revision = profile
        .active_revision_id
        .ok_or_else(|| AppError::Conflict("请先选用配置版本".into()))?;
    let prefs = storage.openai_costs(id)?;
    let resolved = prefs.resolve(&storage.load_revision_source(id, revision)?)?;
    Ok(CostSnapshot {
        profile_id: id,
        profile_revision: revision,
        revision: prefs.revision,
        mode: prefs.mode,
        max_multiplier: prefs.max_multiplier,
        allow_unknown: prefs.allow_unknown,
        nodes: resolved
            .multipliers
            .into_iter()
            .map(|(name, multiplier)| CostRow { name, multiplier })
            .collect(),
    })
}

#[tauri::command]
pub fn get_openai_costs(app: AppHandle, profile_id: Uuid) -> Result<CostSnapshot, AppErrorDto> {
    AppStorage::from_app(&app)
        .and_then(|s| snapshot(&s, profile_id))
        .map_err(|e| e.dto())
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CostInput {
    pub profile_id: Uuid,
    pub profile_revision: Uuid,
    pub revision: u64,
    pub mode: CostMode,
    pub max_multiplier: Option<f64>,
    pub allow_unknown: bool,
    pub nodes: Vec<CostRow>,
}
#[tauri::command]
pub fn save_openai_costs(
    app: AppHandle,
    input: CostInput,
    confirmed: bool,
) -> Result<CostSnapshot, AppErrorDto> {
    let work = || -> AppResult<CostSnapshot> {
        if !confirmed {
            return Err(AppError::InvalidInput(
                "修改自动选点成本策略需要确认".into(),
            ));
        }
        let _permit = crate::user_rules::acquire_configuration(&app)?;
        let storage = AppStorage::from_app(&app)?;
        let current = snapshot(&storage, input.profile_id)?;
        let policy = storage.load_profile(input.profile_id)?.openai_policy;
        if input.mode == CostMode::Value && policy.enabled && !policy.stability_enabled {
            return Err(AppError::Conflict(
                "请先启用稳定优先，再保存性价比策略；基础 Fallback 无法保证成本限制".into(),
            ));
        }
        if storage.state()?.active_profile_id != Some(input.profile_id)
            || current.profile_revision != input.profile_revision
            || current.revision != input.revision
        {
            return Err(AppError::Conflict(
                "配置或成本设置已变化，请重新读取后编辑".into(),
            ));
        }
        let identities =
            identities(&storage.load_revision_source(input.profile_id, input.profile_revision)?)?;
        let mut prefs = CostPreferences {
            revision: current
                .revision
                .checked_add(1)
                .ok_or_else(|| AppError::Conflict("成本版本已达上限".into()))?,
            mode: input.mode,
            max_multiplier: input.max_multiplier,
            allow_unknown: input.allow_unknown,
            nodes: BTreeMap::new(),
        };
        if input.nodes.len() > 300 {
            return Err(AppError::InvalidInput("节点清单过长".into()));
        }
        let mut seen = std::collections::BTreeSet::new();
        for row in &input.nodes {
            let identity = identities
                .get(&row.name)
                .ok_or_else(|| AppError::Conflict("节点清单已变化".into()))?;
            if !seen.insert(&row.name) {
                return Err(AppError::InvalidInput("节点倍率重复".into()));
            }
            if let Some(multiplier) = row.multiplier {
                prefs.nodes.insert(
                    row.name.clone(),
                    NodeCost {
                        identity: identity.clone(),
                        multiplier,
                    },
                );
            }
        }
        prefs.validate()?;
        storage.save_openai_costs(input.profile_id, &prefs)?;
        app.state::<crate::openai_stability::StabilityManager>()
            .invalidate_observations();
        snapshot(&storage, input.profile_id)
    };
    work().map_err(|e| e.dto())
}

#[cfg(test)]
mod tests {
    use super::*;
    const SOURCE: &str = "proxies:\n - {name: good, type: socks5, server: example.invalid, port: 1080}\n - {name: premium, type: socks5, server: other.invalid, port: 1080}\n - {name: 'unknown 1x', type: socks5, server: third.invalid, port: 1080}";
    fn preferences() -> CostPreferences {
        let ids = identities(SOURCE).unwrap();
        CostPreferences {
            mode: CostMode::Value,
            nodes: [("good", 1.0), ("premium", 5.0)]
                .into_iter()
                .map(|(n, multiplier)| {
                    (
                        n.into(),
                        NodeCost {
                            identity: ids[n].clone(),
                            multiplier,
                        },
                    )
                })
                .collect(),
            ..Default::default()
        }
    }
    #[test]
    fn comparable_quality_rewards_lower_multiplier_without_guessing_names() {
        let resolved = preferences().resolve(SOURCE).unwrap();
        assert!(resolved.utility("good", 90.0) > resolved.utility("premium", 98.0));
        assert_eq!(resolved.multiplier("unknown 1x"), None);
        assert!(!resolved.allowed("unknown 1x"));
        let mut prefs = preferences();
        prefs.allow_unknown = true;
        assert!(
            prefs.resolve(SOURCE).unwrap().utility("unknown 1x", 100.0)
                < resolved.utility("premium", 80.0)
        );
    }
    #[test]
    fn cap_invalid_input_and_identity_changes_are_fail_closed() {
        let mut prefs = preferences();
        prefs.max_multiplier = Some(2.0);
        assert!(!prefs.resolve(SOURCE).unwrap().allowed("premium"));
        let changed = SOURCE.replace("server: example.invalid", "server: changed.invalid");
        assert_eq!(prefs.resolve(&changed).unwrap().multiplier("good"), None);
        for n in [f64::NAN, f64::INFINITY, 0.0, -1.0, 1001.0] {
            prefs.max_multiplier = Some(n);
            assert!(prefs.validate().is_err());
        }
        assert!(CostPreferences::default()
            .resolve(SOURCE)
            .unwrap()
            .allowed("unknown 1x"));
    }

    #[test]
    fn cost_storage_roundtrip_is_profile_scoped_and_rejects_corruption() {
        let directory = tempfile::tempdir().unwrap();
        let storage = AppStorage::from_root(directory.path().join("data")).unwrap();
        let profile = storage
            .create_profile(
                "cost fixture".into(),
                crate::models::ProfileSource::Inline {
                    label: "fixture".into(),
                },
            )
            .unwrap();
        let other = storage
            .create_profile(
                "other fixture".into(),
                crate::models::ProfileSource::Inline {
                    label: "other".into(),
                },
            )
            .unwrap();
        let before = serde_json::to_value(storage.load_profile(profile.id).unwrap()).unwrap();
        let mut prefs = preferences();
        prefs.revision = 7;
        prefs.max_multiplier = Some(2.0);
        storage.save_openai_costs(profile.id, &prefs).unwrap();
        let reopened = AppStorage::from_root(directory.path().join("data")).unwrap();
        let saved = reopened.openai_costs(profile.id).unwrap();
        assert_eq!(saved.revision, 7);
        assert_eq!(saved.mode, CostMode::Value);
        assert_eq!(saved.max_multiplier, Some(2.0));
        assert!(!saved.allow_unknown);
        assert_eq!(saved.resolve(SOURCE).unwrap().multiplier("good"), Some(1.0));
        assert_eq!(
            reopened.openai_costs(other.id).unwrap().mode,
            CostMode::Quality
        );
        assert_eq!(
            serde_json::to_value(reopened.load_profile(profile.id).unwrap()).unwrap(),
            before
        );
        prefs.max_multiplier = Some(0.0);
        assert!(reopened.save_openai_costs(profile.id, &prefs).is_err());
        assert_eq!(
            reopened.openai_costs(profile.id).unwrap().max_multiplier,
            Some(2.0)
        );
        // This is an isolated disposable fixture, never the user's storage.
        std::fs::write(
            directory
                .path()
                .join("data")
                .join("profiles")
                .join(profile.id.to_string())
                .join("openai-costs.json"),
            b"{broken",
        )
        .unwrap();
        assert!(reopened.openai_costs(profile.id).is_err());
    }
}
