use chrono::{DateTime, Utc};
use rand::{distr::Alphanumeric, Rng};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const CURRENT_SCHEMA_VERSION: u32 = 3;

fn default_true() -> bool {
    true
}

fn default_openai_max_nodes() -> u8 {
    10
}

fn default_openai_benchmark_version() -> u32 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenAiNodeScore {
    pub name: String,
    pub latency_ms: u32,
    pub jitter_ms: u32,
    pub bandwidth_mbps: Option<f64>,
    pub score: f64,
    pub checked_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenAiPolicy {
    #[serde(default)]
    pub stability_enabled: bool,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub auto_maintain: bool,
    #[serde(default = "default_openai_max_nodes")]
    pub max_nodes: u8,
    #[serde(default)]
    pub selected_nodes: Vec<OpenAiNodeScore>,
    #[serde(default)]
    pub candidate_count: usize,
    #[serde(default)]
    pub healthy_count: usize,
    #[serde(default)]
    pub last_benchmarked_at: Option<DateTime<Utc>>,
    #[serde(default = "default_openai_benchmark_version")]
    pub benchmark_version: u32,
}

impl Default for OpenAiPolicy {
    fn default() -> Self {
        Self {
            stability_enabled: false,
            enabled: false,
            auto_maintain: false,
            max_nodes: default_openai_max_nodes(),
            selected_nodes: Vec::new(),
            candidate_count: 0,
            healthy_count: 0,
            last_benchmarked_at: None,
            benchmark_version: default_openai_benchmark_version(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NetworkMode {
    #[default]
    Manual,
    SystemProxy,
    Tun,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RoutingMode {
    #[default]
    Rule,
    Global,
    Direct,
}

impl RoutingMode {
    pub fn as_mihomo_mode(self) -> &'static str {
        match self {
            Self::Rule => "rule",
            Self::Global => "global",
            Self::Direct => "direct",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    pub schema_version: u32,
    pub locale: String,
    pub theme: String,
    pub launch_at_login: bool,
    // Older installs have no field: login should be tray-only without requiring
    // users to open Settings and save once. An explicit false remains respected.
    #[serde(default = "default_true")]
    pub silent_startup: bool,
    #[serde(default = "default_true")]
    pub restore_last_session: bool,
    #[serde(default = "default_true")]
    pub show_global_traffic: bool,
    pub network_mode: NetworkMode,
    pub mixed_port: u16,
    pub controller_port: u16,
    pub controller_secret: String,
    pub update_channel: String,
    #[serde(default = "default_true")]
    pub auto_check_updates: bool,
    #[serde(default)]
    pub update_source: crate::app_update::UpdateSource,
    #[serde(default)]
    pub auto_download_updates: bool,
    pub diagnostics_retention_days: u16,
    #[serde(default = "default_app_log_retention_days")]
    pub app_log_retention_days: u16,
    /// Runtime overlay from the independent user-rules store; never serialized
    /// into settings.json or included in the public settings API.
    #[serde(skip)]
    pub user_rules: Vec<crate::user_rules::UserRule>,
}

impl Default for AppSettings {
    fn default() -> Self {
        let controller_secret: String = rand::rng()
            .sample_iter(&Alphanumeric)
            .take(48)
            .map(char::from)
            .collect();
        Self {
            schema_version: CURRENT_SCHEMA_VERSION,
            locale: "zh-CN".to_string(),
            theme: "system".to_string(),
            launch_at_login: false,
            silent_startup: true,
            restore_last_session: true,
            show_global_traffic: true,
            network_mode: NetworkMode::Manual,
            mixed_port: 7890,
            controller_port: 9090,
            controller_secret,
            update_channel: "stable".to_string(),
            auto_check_updates: true,
            update_source: crate::app_update::UpdateSource::Auto,
            auto_download_updates: false,
            diagnostics_retention_days: 7,
            app_log_retention_days: default_app_log_retention_days(),
            user_rules: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicAppSettings {
    pub schema_version: u32,
    pub locale: String,
    pub theme: String,
    pub launch_at_login: bool,
    #[serde(default = "default_true")]
    pub silent_startup: bool,
    #[serde(default = "default_true")]
    pub restore_last_session: bool,
    pub show_global_traffic: bool,
    pub network_mode: NetworkMode,
    pub mixed_port: u16,
    pub controller_port: u16,
    pub update_channel: String,
    #[serde(default = "default_true")]
    pub auto_check_updates: bool,
    #[serde(default)]
    pub update_source: crate::app_update::UpdateSource,
    #[serde(default)]
    pub auto_download_updates: bool,
    pub diagnostics_retention_days: u16,
    #[serde(default = "default_app_log_retention_days")]
    pub app_log_retention_days: u16,
}

impl From<&AppSettings> for PublicAppSettings {
    fn from(value: &AppSettings) -> Self {
        Self {
            schema_version: value.schema_version,
            locale: value.locale.clone(),
            theme: value.theme.clone(),
            launch_at_login: value.launch_at_login,
            silent_startup: value.silent_startup,
            restore_last_session: value.restore_last_session,
            show_global_traffic: value.show_global_traffic,
            network_mode: value.network_mode,
            mixed_port: value.mixed_port,
            controller_port: value.controller_port,
            update_channel: value.update_channel.clone(),
            auto_check_updates: value.auto_check_updates,
            update_source: value.update_source,
            auto_download_updates: value.auto_download_updates,
            diagnostics_retention_days: value.diagnostics_retention_days,
            app_log_retention_days: value.app_log_retention_days,
        }
    }
}

impl PublicAppSettings {
    pub fn merge_secret(self, current: &AppSettings) -> AppSettings {
        AppSettings {
            schema_version: self.schema_version,
            locale: self.locale,
            theme: self.theme,
            launch_at_login: self.launch_at_login,
            silent_startup: self.silent_startup,
            restore_last_session: self.restore_last_session,
            show_global_traffic: self.show_global_traffic,
            network_mode: self.network_mode,
            mixed_port: self.mixed_port,
            controller_port: self.controller_port,
            controller_secret: current.controller_secret.clone(),
            update_channel: self.update_channel,
            auto_check_updates: self.auto_check_updates,
            update_source: self.update_source,
            auto_download_updates: self.auto_download_updates,
            diagnostics_retention_days: self.diagnostics_retention_days,
            app_log_retention_days: self.app_log_retention_days,
            user_rules: current.user_rules.clone(),
        }
    }
}

pub(crate) fn default_app_log_retention_days() -> u16 {
    3
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProfileSource {
    RemoteSubscription { url: String, user_agent: String },
    LocalFile { source_path: String },
    Inline { label: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileRecord {
    pub schema_version: u32,
    pub id: Uuid,
    pub display_name: String,
    pub source: ProfileSource,
    #[serde(default)]
    pub routing_mode: RoutingMode,
    #[serde(default)]
    pub openai_policy: OpenAiPolicy,
    pub active_revision_id: Option<Uuid>,
    pub last_known_good_revision_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PublicProfileSource {
    RemoteSubscription { host: String, user_agent: String },
    LocalFile { source_path: String },
    Inline { label: String },
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicProfileRecord {
    pub schema_version: u32,
    pub id: Uuid,
    pub display_name: String,
    pub source: PublicProfileSource,
    pub routing_mode: RoutingMode,
    pub openai_policy: OpenAiPolicy,
    pub active_revision_id: Option<Uuid>,
    pub last_known_good_revision_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<&ProfileRecord> for PublicProfileRecord {
    fn from(value: &ProfileRecord) -> Self {
        let source = match &value.source {
            ProfileSource::RemoteSubscription { url, user_agent } => {
                let host = url::Url::parse(url)
                    .ok()
                    .and_then(|url| url.host_str().map(str::to_string))
                    .unwrap_or_else(|| "remote-subscription".to_string());
                PublicProfileSource::RemoteSubscription {
                    host,
                    user_agent: user_agent.clone(),
                }
            }
            ProfileSource::LocalFile { source_path } => PublicProfileSource::LocalFile {
                source_path: source_path.clone(),
            },
            ProfileSource::Inline { label } => PublicProfileSource::Inline {
                label: label.clone(),
            },
        };
        Self {
            schema_version: value.schema_version,
            id: value.id,
            display_name: value.display_name.clone(),
            source,
            routing_mode: value.routing_mode,
            openai_policy: value.openai_policy.clone(),
            active_revision_id: value.active_revision_id,
            last_known_good_revision_id: value.last_known_good_revision_id,
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}

impl ProfileRecord {
    pub fn new(display_name: String, source: ProfileSource) -> Self {
        let now = Utc::now();
        Self {
            schema_version: CURRENT_SCHEMA_VERSION,
            id: Uuid::new_v4(),
            display_name,
            source,
            routing_mode: RoutingMode::Rule,
            openai_policy: OpenAiPolicy::default(),
            active_revision_id: None,
            last_known_good_revision_id: None,
            created_at: now,
            updated_at: now,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionMetadata {
    pub content_type: Option<String>,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub bytes: usize,
    #[serde(default)]
    pub usage: Option<SubscriptionUsage>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SubscriptionUsage {
    pub upload_bytes: Option<u64>,
    pub download_bytes: Option<u64>,
    pub total_bytes: Option<u64>,
    /// Unix seconds supplied by the subscription server, not milliseconds.
    pub expires_at: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SubscriptionStatus {
    pub request_started_at: Option<DateTime<Utc>>,
    pub checked_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    pub usage: Option<SubscriptionUsage>,
    pub usage_updated_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigRevision {
    pub schema_version: u32,
    pub id: Uuid,
    pub profile_id: Uuid,
    pub source_sha256: String,
    pub effective_sha256: String,
    pub fetched_at: DateTime<Utc>,
    pub subscription: Option<SubscriptionMetadata>,
    pub validation: ValidationReport,
    #[serde(default)]
    pub openai_policy: OpenAiPolicy,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ValidationReport {
    pub valid: bool,
    pub warnings: Vec<String>,
    pub errors: Vec<String>,
    pub native_core_validated: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersistentAppState {
    pub schema_version: u32,
    pub active_profile_id: Option<Uuid>,
    pub active_revision_id: Option<Uuid>,
    pub system_proxy_snapshot_present: bool,
    pub clean_shutdown: bool,
    /// User intent, not whether cleanup left a child process running. Legacy
    /// state must never infer this from clean_shutdown or an active profile.
    #[serde(default)]
    pub desired_running: bool,
    pub updated_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RuntimePhase {
    Uninitialized,
    #[default]
    Stopped,
    Validating,
    Starting,
    Running,
    Stopping,
    Crashed,
    Recovering,
}

#[cfg(test)]
mod tests {
    use super::AppSettings;

    #[test]
    fn global_traffic_monitor_is_enabled_by_default() {
        assert!(AppSettings::default().show_global_traffic);
        assert!(AppSettings::default().auto_check_updates);
        assert!(AppSettings::default().restore_last_session);
        assert!(!AppSettings::default().launch_at_login);
    }

    #[test]
    fn older_settings_enable_global_traffic_during_deserialization() {
        let settings: AppSettings = serde_json::from_str(
            r#"{
              "schemaVersion": 2,
              "locale": "zh-CN",
              "theme": "system",
              "launchAtLogin": false,
              "networkMode": "manual",
              "mixedPort": 7895,
              "controllerPort": 9090,
              "controllerSecret": "fixture",
              "updateChannel": "stable",
              "diagnosticsRetentionDays": 7
            }"#,
        )
        .expect("legacy settings");
        assert!(settings.show_global_traffic);
        assert!(settings.auto_check_updates);
        assert!(settings.restore_last_session);
        assert!(!settings.launch_at_login);
        assert_eq!(settings.app_log_retention_days, 3);
    }

    #[test]
    fn older_public_settings_enable_update_checks_during_deserialization() {
        let settings: super::PublicAppSettings = serde_json::from_str(
            r#"{
              "schemaVersion": 3,
              "locale": "zh-CN",
              "theme": "system",
              "launchAtLogin": false,
              "showGlobalTraffic": true,
              "networkMode": "manual",
              "mixedPort": 7895,
              "controllerPort": 9090,
              "updateChannel": "stable",
              "diagnosticsRetentionDays": 7
            }"#,
        )
        .expect("legacy public settings");
        assert!(settings.auto_check_updates);
        assert!(settings.restore_last_session);
    }

    #[test]
    fn log_retention_roundtrips_without_changing_other_preferences() {
        let current = AppSettings::default();
        let mut public = super::PublicAppSettings::from(&current);
        public.app_log_retention_days = 14;
        let merged = public.merge_secret(&current);
        assert_eq!(merged.app_log_retention_days, 14);
        assert_eq!(merged.diagnostics_retention_days, 7);
        assert_eq!(merged.network_mode, current.network_mode);
        assert_eq!(merged.controller_secret, current.controller_secret);
        let json = serde_json::to_string(&merged).unwrap();
        assert_eq!(
            serde_json::from_str::<AppSettings>(&json)
                .unwrap()
                .app_log_retention_days,
            14
        );
    }

    #[test]
    fn resume_preference_survives_public_settings_without_changing_login_or_secret() {
        let current = AppSettings::default();
        let mut public = super::PublicAppSettings::from(&current);
        public.restore_last_session = false;
        let merged = public.merge_secret(&current);
        assert!(!merged.restore_last_session);
        assert_eq!(merged.launch_at_login, current.launch_at_login);
        assert_eq!(merged.controller_secret, current.controller_secret);
        let serialized = serde_json::to_value(super::PublicAppSettings::from(&merged)).unwrap();
        assert_eq!(serialized["restoreLastSession"], false);
        assert!(serialized.get("controllerSecret").is_none());
    }
}
