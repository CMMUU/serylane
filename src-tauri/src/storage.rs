use crate::error::{AppError, AppResult};
use crate::models::{
    AppSettings, ConfigRevision, OpenAiPolicy, PersistentAppState, ProfileRecord, ProfileSource,
    SubscriptionMetadata, SubscriptionStatus, SubscriptionUsage, ValidationReport,
    CURRENT_SCHEMA_VERSION,
};
use crate::user_rules::UserRulesDocument;
use chrono::Utc;
use serde::{de::DeserializeOwned, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct AppStorage {
    root: PathBuf,
}

impl AppStorage {
    pub(crate) fn app_log_path(&self) -> PathBuf {
        self.root.join("app-log-v1.json")
    }
    pub(crate) fn routing_dir(&self) -> PathBuf {
        self.root.join("local-routing")
    }

    pub fn from_app(app: &AppHandle) -> AppResult<Self> {
        let root = app
            .path()
            .app_data_dir()
            .map_err(|error| AppError::Io(error.to_string()))?;
        Self::from_root(root)
    }

    pub fn from_root(root: PathBuf) -> AppResult<Self> {
        fs::create_dir_all(&root)?;
        set_private_directory_permissions(&root)?;
        let storage = Self { root };
        storage.ensure_layout()?;
        Ok(storage)
    }

    pub fn settings(&self) -> AppResult<AppSettings> {
        let path = self.root.join("settings.json");
        if !path.exists() {
            let mut settings = AppSettings::default();
            self.save_settings(&settings)?;
            settings.user_rules = self.user_rules()?.rules;
            return Ok(settings);
        }
        let mut settings: AppSettings = read_json(&path)?;
        if settings.schema_version < CURRENT_SCHEMA_VERSION {
            settings.schema_version = CURRENT_SCHEMA_VERSION;
            self.save_settings(&settings)?;
        }
        settings.user_rules = self.user_rules()?.rules;
        Ok(settings)
    }

    pub fn save_settings(&self, settings: &AppSettings) -> AppResult<()> {
        write_json_atomic(&self.root.join("settings.json"), settings)
    }

    pub fn programs(&self) -> AppResult<crate::program_proxy::ProgramDocument> {
        let path = self.root.join("proxy-programs.json");
        if !path.exists() {
            return Ok(crate::program_proxy::ProgramDocument::default());
        }
        if fs::metadata(&path)?.len() > 4 * 1024 * 1024 {
            return Err(AppError::Io("程序代理清单超过大小限制".into()));
        }
        let document: crate::program_proxy::ProgramDocument = read_json(&path)?;
        document.validate()?;
        Ok(document)
    }

    pub fn save_programs(&self, document: &crate::program_proxy::ProgramDocument) -> AppResult<()> {
        let mut document = document.clone();
        document.schema_version = 2;
        document.validate()?;
        if serde_json::to_vec_pretty(&document)
            .map_err(|error| AppError::Io(error.to_string()))?
            .len()
            > 4 * 1024 * 1024
        {
            return Err(AppError::Io("程序代理清单超过大小限制".into()));
        }
        let path = self.root.join("proxy-programs.json");
        if path.exists() {
            let old = self.programs()?;
            if old.schema_version == 1 {
                // Preserve the exact old bytes before the first v2 write. A
                // failed backup aborts migration; an existing backup is retained.
                let backup = self.root.join("proxy-programs.v1.backup.json");
                if !backup.exists() {
                    let bytes = fs::read(&path)?;
                    let mut file = tempfile::Builder::new()
                        .prefix(".programs-v1-")
                        .tempfile_in(&self.root)?;
                    file.write_all(&bytes)?;
                    file.as_file().sync_all()?;
                    set_private_file_permissions(file.path())?;
                    file.persist_noclobber(&backup)
                        .map_err(|e| AppError::Io(e.to_string()))?;
                }
            }
        }
        write_json_atomic(&path, &document)
    }

    pub fn user_rules(&self) -> AppResult<UserRulesDocument> {
        let path = self.root.join("user-rules.json");
        if !path.exists() {
            return Ok(UserRulesDocument::default());
        }
        if fs::metadata(&path)?.len() > 16 * 1024 * 1024 {
            return Err(AppError::Io("用户规则存储超过大小限制".to_string()));
        }
        let document: UserRulesDocument = read_json(&path)?;
        document.validate_storage()?;
        Ok(document)
    }

    pub fn save_user_rules(&self, document: &UserRulesDocument) -> AppResult<()> {
        document.validate_storage()?;
        if serde_json::to_vec_pretty(document)
            .map_err(|error| AppError::Io(error.to_string()))?
            .len()
            > 16 * 1024 * 1024
        {
            return Err(AppError::Io("用户规则及历史总大小超过 16 MiB".to_string()));
        }
        write_json_atomic(&self.root.join("user-rules.json"), document)
    }

    pub fn state(&self) -> AppResult<PersistentAppState> {
        let path = self.root.join("state.json");
        if !path.exists() {
            return Ok(PersistentAppState {
                schema_version: CURRENT_SCHEMA_VERSION,
                clean_shutdown: true,
                ..Default::default()
            });
        }
        read_json(&path)
    }

    pub fn save_state(&self, state: &PersistentAppState) -> AppResult<()> {
        write_json_atomic(&self.root.join("state.json"), state)
    }

    pub fn list_profiles(&self) -> AppResult<Vec<ProfileRecord>> {
        let mut profiles: Vec<ProfileRecord> = Vec::new();
        for entry in fs::read_dir(self.root.join("profiles"))? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let metadata = entry.path().join("metadata.json");
            if metadata.exists() {
                profiles.push(read_json(&metadata)?);
            }
        }
        profiles.sort_by_key(|profile| std::cmp::Reverse(profile.updated_at));
        Ok(profiles)
    }

    pub fn create_profile(
        &self,
        display_name: String,
        source: ProfileSource,
    ) -> AppResult<ProfileRecord> {
        let display_name = display_name.trim();
        if display_name.is_empty() {
            return Err(AppError::InvalidInput("配置名称为空".to_string()));
        }
        let profile = ProfileRecord::new(display_name.chars().take(128).collect(), source);
        let directory = self.profile_dir(profile.id);
        let revisions = directory.join("revisions");
        fs::create_dir_all(&revisions)?;
        set_private_directory_permissions(&directory)?;
        set_private_directory_permissions(&revisions)?;
        self.save_profile(&profile)?;
        Ok(profile)
    }

    pub fn load_profile(&self, profile_id: Uuid) -> AppResult<ProfileRecord> {
        let path = self.profile_dir(profile_id).join("metadata.json");
        if !path.exists() {
            return Err(AppError::NotFound(format!("profile {profile_id}")));
        }
        read_json(&path)
    }

    pub fn save_profile(&self, profile: &ProfileRecord) -> AppResult<()> {
        write_json_atomic(&self.profile_dir(profile.id).join("metadata.json"), profile)
    }

    // User intent is separate from generated/immutable policy revisions. A
    // subscription refresh must not silently turn a manual choice back to auto.
    pub fn openai_manual_node(&self, profile_id: Uuid) -> AppResult<Option<String>> {
        let path = self.profile_dir(profile_id).join("openai-selection.json");
        if !path.exists() {
            return Ok(None);
        }
        if fs::metadata(&path)?.len() > 4096 {
            return Err(AppError::Io("OpenAI 手动选择记录超过大小限制".into()));
        }
        read_json(&path)
    }

    pub fn openai_costs(&self, profile_id: Uuid) -> AppResult<crate::openai_cost::CostPreferences> {
        let path = self.profile_dir(profile_id).join("openai-costs.json");
        if !path.exists() {
            return Ok(Default::default());
        }
        if fs::metadata(&path)?.len() > 1024 * 1024 {
            return Err(AppError::Io("节点成本清单过大".into()));
        }
        let prefs: crate::openai_cost::CostPreferences = read_json(&path)?;
        prefs.validate()?;
        Ok(prefs)
    }
    pub fn save_openai_costs(
        &self,
        profile_id: Uuid,
        prefs: &crate::openai_cost::CostPreferences,
    ) -> AppResult<()> {
        self.load_profile(profile_id)?;
        prefs.validate()?;
        write_json_atomic(
            &self.profile_dir(profile_id).join("openai-costs.json"),
            prefs,
        )
    }

    pub fn save_openai_manual_node(&self, profile_id: Uuid, node: Option<&str>) -> AppResult<()> {
        self.load_profile(profile_id)?;
        if node.is_some_and(|name| {
            name.is_empty() || name.len() > 2048 || name.chars().any(char::is_control)
        }) {
            return Err(AppError::InvalidInput("节点名称无效".into()));
        }
        write_json_atomic(
            &self.profile_dir(profile_id).join("openai-selection.json"),
            &node,
        )
    }

    pub fn subscription_status(&self, profile_id: Uuid) -> Option<SubscriptionStatus> {
        let path = self
            .profile_dir(profile_id)
            .join("subscription-status.json");
        // Observations are optional cache data. A missing/corrupt cache must not
        // prevent listing, selecting, or recovering the actual subscription.
        if fs::metadata(&path).ok()?.len() > 16 * 1024 {
            return None;
        }
        read_json(&path).ok()
    }

    pub fn record_subscription_check(
        &self,
        profile_id: Uuid,
        usage: Option<&SubscriptionUsage>,
        error: Option<&AppError>,
    ) -> AppResult<()> {
        self.record_subscription_check_since(profile_id, usage, error, Utc::now())
    }

    pub fn record_subscription_check_since(
        &self,
        profile_id: Uuid,
        usage: Option<&SubscriptionUsage>,
        error: Option<&AppError>,
        request_started: chrono::DateTime<Utc>,
    ) -> AppResult<()> {
        static OBSERVATION_WRITE: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _permit = OBSERVATION_WRITE
            .lock()
            .map_err(|_| AppError::Io("订阅检查记录暂时无法保存".into()))?;
        self.load_profile(profile_id)?;
        let mut status = self.subscription_status(profile_id).unwrap_or_default();
        if status
            .request_started_at
            .or(status.checked_at)
            .is_some_and(|started| started > request_started)
        {
            return Ok(());
        }
        let now = Utc::now();
        status.checked_at = Some(now);
        status.request_started_at = Some(request_started);
        status.last_error = error.map(crate::subscription::safe_subscription_error);
        if error.is_none() {
            if let Some(usage) = usage {
                status.usage = Some(usage.clone());
                status.usage_updated_at = Some(now);
            }
        }
        // Separate from immutable revisions, active selection and runtime YAML:
        // refreshing a quota must never restart/hot-reload an existing stream.
        write_json_atomic(
            &self
                .profile_dir(profile_id)
                .join("subscription-status.json"),
            &status,
        )
    }

    pub fn delete_profile(&self, profile_id: Uuid) -> AppResult<()> {
        let state = self.state()?;
        if state.active_profile_id == Some(profile_id) {
            return Err(AppError::Conflict("正在使用的配置不能删除".to_string()));
        }
        let directory = self.profile_dir(profile_id);
        if !directory.exists() {
            return Err(AppError::NotFound(format!("profile {profile_id}")));
        }
        fs::remove_dir_all(directory)?;
        Ok(())
    }

    pub fn save_revision(
        &self,
        profile_id: Uuid,
        source: &str,
        effective: &str,
        subscription: Option<SubscriptionMetadata>,
        validation: ValidationReport,
        openai_policy: OpenAiPolicy,
    ) -> AppResult<ConfigRevision> {
        let _profile = self.load_profile(profile_id)?;
        let revision = ConfigRevision {
            schema_version: CURRENT_SCHEMA_VERSION,
            id: Uuid::new_v4(),
            profile_id,
            source_sha256: digest(source.as_bytes()),
            effective_sha256: digest(effective.as_bytes()),
            fetched_at: Utc::now(),
            subscription,
            validation,
            openai_policy,
        };
        let directory = self.revision_dir(profile_id, revision.id);
        fs::create_dir_all(&directory)?;
        set_private_directory_permissions(&directory)?;
        write_private_atomic(&directory.join("source.yaml"), source.as_bytes())?;
        write_private_atomic(&directory.join("effective.yaml"), effective.as_bytes())?;
        write_json_atomic(&directory.join("validation.json"), &revision)?;
        Ok(revision)
    }

    pub fn list_revisions(&self, profile_id: Uuid) -> AppResult<Vec<ConfigRevision>> {
        let revisions_dir = self.profile_dir(profile_id).join("revisions");
        if !revisions_dir.exists() {
            return Ok(Vec::new());
        }
        let mut revisions: Vec<ConfigRevision> = Vec::new();
        for entry in fs::read_dir(revisions_dir)? {
            let path = entry?.path().join("validation.json");
            if path.exists() {
                revisions.push(read_json(&path)?);
            }
        }
        revisions.sort_by_key(|revision| std::cmp::Reverse(revision.fetched_at));
        Ok(revisions)
    }

    pub fn load_revision_source(&self, profile_id: Uuid, revision_id: Uuid) -> AppResult<String> {
        self.read_revision_file(profile_id, revision_id, "source.yaml")
    }

    pub fn load_revision(&self, profile_id: Uuid, revision_id: Uuid) -> AppResult<ConfigRevision> {
        let path = self
            .revision_dir(profile_id, revision_id)
            .join("validation.json");
        if !path.exists() {
            return Err(AppError::NotFound(format!(
                "revision {revision_id} for profile {profile_id}"
            )));
        }
        read_json(&path)
    }

    pub fn load_revision_effective(
        &self,
        profile_id: Uuid,
        revision_id: Uuid,
    ) -> AppResult<String> {
        self.read_revision_file(profile_id, revision_id, "effective.yaml")
    }

    #[cfg(test)]
    pub fn activate_revision(
        &self,
        profile_id: Uuid,
        revision_id: Uuid,
    ) -> AppResult<ProfileRecord> {
        self.select_revision(profile_id, revision_id, true, None)
    }

    pub fn activate_revision_with_config(
        &self,
        profile_id: Uuid,
        revision_id: Uuid,
        effective: &str,
    ) -> AppResult<ProfileRecord> {
        self.select_revision(profile_id, revision_id, true, Some(effective))
    }

    pub fn update_profile_revision(
        &self,
        profile_id: Uuid,
        revision_id: Uuid,
    ) -> AppResult<ProfileRecord> {
        self.select_revision(profile_id, revision_id, false, None)
    }

    fn select_revision(
        &self,
        profile_id: Uuid,
        revision_id: Uuid,
        activate_app: bool,
        effective_override: Option<&str>,
    ) -> AppResult<ProfileRecord> {
        let effective = match effective_override {
            Some(value) => value.to_string(),
            None => self.load_revision_effective(profile_id, revision_id)?,
        };
        let revision = self.load_revision(profile_id, revision_id)?;
        let mut profile = self.load_profile(profile_id)?;
        if profile.active_revision_id != Some(revision_id) {
            profile.last_known_good_revision_id = profile.active_revision_id;
            profile.active_revision_id = Some(revision_id);
            profile.updated_at = Utc::now();
        }
        profile.schema_version = CURRENT_SCHEMA_VERSION;
        profile.openai_policy = revision.openai_policy;
        self.save_profile(&profile)?;
        if !activate_app {
            return Ok(profile);
        }
        write_private_atomic(
            &self.root.join("runtime").join("active.yaml"),
            effective.as_bytes(),
        )?;

        let mut state = self.state()?;
        state.schema_version = CURRENT_SCHEMA_VERSION;
        state.active_profile_id = Some(profile_id);
        state.active_revision_id = Some(revision_id);
        state.updated_at = Some(Utc::now());
        self.save_state(&state)?;
        Ok(profile)
    }

    pub fn active_runtime_config(&self) -> AppResult<Option<String>> {
        let path = self.root.join("runtime").join("active.yaml");
        if path.exists() {
            Ok(Some(fs::read_to_string(path)?))
        } else {
            Ok(None)
        }
    }

    pub fn restore_active_runtime_config(&self, config: Option<&str>) -> AppResult<()> {
        let path = self.root.join("runtime").join("active.yaml");
        match config {
            Some(config) => write_private_atomic(&path, config.as_bytes()),
            None if path.exists() => fs::remove_file(path).map_err(AppError::from),
            None => Ok(()),
        }
    }

    #[cfg(test)]
    pub fn rollback_profile(&self, profile_id: Uuid) -> AppResult<ProfileRecord> {
        let profile = self.load_profile(profile_id)?;
        let revision_id = profile
            .last_known_good_revision_id
            .ok_or_else(|| AppError::Conflict("没有可回滚的稳定版本".to_string()))?;
        self.activate_revision(profile_id, revision_id)
    }

    #[cfg(test)]
    pub fn active_effective_config(&self) -> AppResult<String> {
        let path = self.root.join("runtime").join("active.yaml");
        if !path.exists() {
            return Err(AppError::NotFound("active configuration".to_string()));
        }
        fs::read_to_string(path).map_err(AppError::from)
    }

    pub fn mark_clean_shutdown(&self, clean: bool) -> AppResult<()> {
        let mut state = self.state()?;
        state.clean_shutdown = clean;
        state.updated_at = Some(Utc::now());
        self.save_state(&state)
    }

    pub fn set_desired_running(&self, desired: bool) -> AppResult<()> {
        let mut state = self.state()?;
        state.desired_running = desired;
        state.updated_at = Some(Utc::now());
        self.save_state(&state)
    }

    fn ensure_layout(&self) -> AppResult<()> {
        for directory in ["profiles", "runtime", "logs", "diagnostics"] {
            let path = self.root.join(directory);
            fs::create_dir_all(&path)?;
            set_private_directory_permissions(&path)?;
        }
        let permission_marker = self.root.join(".permissions-v1");
        if !permission_marker.exists() {
            secure_existing_tree(&self.root)?;
            write_private_atomic(&permission_marker, b"ok\n")?;
        }
        Ok(())
    }

    fn profile_dir(&self, profile_id: Uuid) -> PathBuf {
        self.root.join("profiles").join(profile_id.to_string())
    }

    fn revision_dir(&self, profile_id: Uuid, revision_id: Uuid) -> PathBuf {
        self.profile_dir(profile_id)
            .join("revisions")
            .join(revision_id.to_string())
    }

    fn read_revision_file(
        &self,
        profile_id: Uuid,
        revision_id: Uuid,
        filename: &str,
    ) -> AppResult<String> {
        let path = self.revision_dir(profile_id, revision_id).join(filename);
        if !path.exists() {
            return Err(AppError::NotFound(format!(
                "revision {revision_id} for profile {profile_id}"
            )));
        }
        fs::read_to_string(path).map_err(AppError::from)
    }
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn read_json<T: DeserializeOwned>(path: &Path) -> AppResult<T> {
    let content = fs::read(path)?;
    serde_json::from_slice(&content).map_err(|error| AppError::Io(error.to_string()))
}

fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> AppResult<()> {
    let content =
        serde_json::to_vec_pretty(value).map_err(|error| AppError::Io(error.to_string()))?;
    write_private_atomic(path, &content)
}

pub(crate) fn write_private_atomic(path: &Path, bytes: &[u8]) -> AppResult<()> {
    let parent = path
        .parent()
        .ok_or_else(|| AppError::Io("文件没有父目录".to_string()))?;
    fs::create_dir_all(parent)?;
    let temp = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name().unwrap_or_default().to_string_lossy(),
        Uuid::new_v4()
    ));
    let backup = parent.join(format!(
        ".{}.backup",
        path.file_name().unwrap_or_default().to_string_lossy()
    ));
    {
        let mut file = fs::File::create(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    set_private_file_permissions(&temp)?;
    if path.exists() {
        if backup.exists() {
            fs::remove_file(&backup)?;
        }
        fs::rename(path, &backup)?;
    }
    if let Err(error) = fs::rename(&temp, path) {
        if backup.exists() {
            let _ = fs::rename(&backup, path);
        }
        return Err(AppError::Io(error.to_string()));
    }
    if backup.exists() {
        fs::remove_file(backup)?;
    }
    Ok(())
}

fn set_private_file_permissions(path: &Path) -> AppResult<()> {
    #[cfg(not(unix))]
    let _ = path;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

fn set_private_directory_permissions(path: &Path) -> AppResult<()> {
    #[cfg(not(unix))]
    let _ = path;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn secure_existing_tree(path: &Path) -> AppResult<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        return Ok(());
    }
    if metadata.is_dir() {
        set_private_directory_permissions(path)?;
        for entry in fs::read_dir(path)? {
            secure_existing_tree(&entry?.path())?;
        }
    } else if metadata.is_file() {
        set_private_file_permissions(path)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::AppStorage;
    use crate::error::AppError;
    use crate::models::{OpenAiPolicy, ProfileSource, SubscriptionUsage, ValidationReport};
    use std::fs;
    use uuid::Uuid;

    fn temp_root() -> std::path::PathBuf {
        std::env::temp_dir().join(format!("routedeck-storage-{}", Uuid::new_v4()))
    }

    #[test]
    fn cleanup_preserves_running_intent_but_explicit_stop_clears_it() {
        let directory = tempfile::tempdir().expect("isolated storage");
        let storage = AppStorage::from_root(directory.path().join("session")).unwrap();
        let profile_id = Uuid::new_v4();
        let revision_id = Uuid::new_v4();
        let mut state = storage.state().unwrap();
        assert!(!state.desired_running);
        state.active_profile_id = Some(profile_id);
        state.active_revision_id = Some(revision_id);
        storage.save_state(&state).unwrap();
        storage.set_desired_running(true).unwrap();
        storage.mark_clean_shutdown(true).unwrap();
        let reopened = AppStorage::from_root(directory.path().join("session")).unwrap();
        assert!(reopened.state().unwrap().desired_running);
        reopened.mark_clean_shutdown(false).unwrap();
        assert!(reopened.state().unwrap().desired_running);
        reopened.set_desired_running(false).unwrap();
        reopened.mark_clean_shutdown(true).unwrap();
        let stopped = reopened.state().unwrap();
        assert!(!stopped.desired_running);
        assert_eq!(stopped.active_profile_id, Some(profile_id));
        assert_eq!(stopped.active_revision_id, Some(revision_id));
        assert!(!reopened.settings().unwrap().launch_at_login);
    }

    #[test]
    fn subscription_observations_preserve_quota_on_missing_headers_and_failure() {
        let root = temp_root();
        let storage = AppStorage::from_root(root.clone()).expect("storage");
        let profile = storage
            .create_profile(
                "quota fixture".into(),
                ProfileSource::RemoteSubscription {
                    url: "https://subscription.example.invalid/config?token=secret".into(),
                    user_agent: "clash.meta".into(),
                },
            )
            .expect("profile");
        let revision = storage
            .save_revision(
                profile.id,
                "proxies: []",
                "proxies: []",
                None,
                ValidationReport::default(),
                OpenAiPolicy::default(),
            )
            .expect("revision");
        storage
            .activate_revision(profile.id, revision.id)
            .expect("activate fixture only");
        let revision_path = storage
            .revision_dir(profile.id, revision.id)
            .join("validation.json");
        let profile_path = storage.profile_dir(profile.id).join("metadata.json");
        let original_revision = fs::read(&revision_path).expect("revision bytes");
        let original_profile = fs::read(&profile_path).expect("profile bytes");
        let original_runtime = storage.active_runtime_config().expect("runtime");
        assert!(storage.subscription_status(profile.id).is_none());
        let usage = SubscriptionUsage {
            upload_bytes: Some(0),
            download_bytes: Some(200),
            total_bytes: Some(1000),
            expires_at: None,
        };
        storage
            .record_subscription_check(profile.id, Some(&usage), None)
            .expect("initial usage");
        let initial = storage.subscription_status(profile.id).expect("status");
        assert_eq!(initial.usage, Some(usage.clone()));
        assert!(initial.last_error.is_none());
        storage
            .record_subscription_check(profile.id, None, None)
            .expect("200/304 without header");
        let unchanged = storage
            .subscription_status(profile.id)
            .expect("unchanged status");
        assert_eq!(unchanged.usage, initial.usage);
        assert_eq!(unchanged.usage_updated_at, initial.usage_updated_at);
        assert!(unchanged.checked_at >= initial.checked_at);
        let failure =
            AppError::Subscription("HTTP 403 https://private.example/sub?token=hidden".into());
        storage
            .record_subscription_check(profile.id, None, Some(&failure))
            .expect("failure status");
        // Late quota responses must not overwrite a newer manual observation.
        storage
            .record_subscription_check_since(
                profile.id,
                Some(&SubscriptionUsage {
                    total_bytes: Some(999),
                    ..Default::default()
                }),
                None,
                chrono::Utc::now() - chrono::Duration::minutes(1),
            )
            .unwrap();
        let failed = storage.subscription_status(profile.id).expect("failure");
        assert!(failed
            .last_error
            .as_ref()
            .expect("error")
            .contains("HTTP 403"));
        assert_eq!(failed.usage, initial.usage);
        assert_eq!(failed.usage_updated_at, initial.usage_updated_at);
        let serialized = fs::read_to_string(
            storage
                .profile_dir(profile.id)
                .join("subscription-status.json"),
        )
        .expect("cache");
        assert!(!serialized.contains("private.example"));
        assert!(!serialized.contains("token="));
        storage
            .record_subscription_check(profile.id, None, None)
            .expect("successful check clears failure");
        assert!(storage
            .subscription_status(profile.id)
            .expect("recovered")
            .last_error
            .is_none());
        assert_eq!(
            fs::read(revision_path).expect("unchanged revision"),
            original_revision
        );
        assert_eq!(
            fs::read(profile_path).expect("unchanged profile"),
            original_profile
        );
        assert_eq!(
            storage.list_revisions(profile.id).expect("versions").len(),
            1
        );
        assert_eq!(
            storage.active_runtime_config().expect("unchanged runtime"),
            original_runtime
        );
        fs::remove_dir_all(root).expect("cleanup fixture");
    }

    #[test]
    fn damaged_optional_observation_does_not_hide_subscriptions() {
        let root = temp_root();
        let storage = AppStorage::from_root(root.clone()).expect("storage");
        let profile = storage
            .create_profile(
                "fixture".into(),
                ProfileSource::Inline {
                    label: "fixture".into(),
                },
            )
            .expect("profile");
        let path = storage
            .profile_dir(profile.id)
            .join("subscription-status.json");
        fs::write(&path, b"not JSON").expect("corrupt optional cache");
        assert!(storage.subscription_status(profile.id).is_none());
        assert_eq!(
            storage.list_profiles().expect("profiles available").len(),
            1
        );
        fs::write(&path, b"{}").expect("old optional data");
        let old = storage
            .subscription_status(profile.id)
            .expect("default fields");
        assert!(old.checked_at.is_none() && old.usage.is_none() && old.last_error.is_none());
        fs::write(&path, vec![b' '; 16 * 1024 + 1]).expect("oversized cache");
        assert!(storage.subscription_status(profile.id).is_none());
        fs::remove_dir_all(root).expect("cleanup fixture");
    }

    #[test]
    fn stores_versions_and_rolls_back() {
        let root = temp_root();
        let storage = AppStorage::from_root(root.clone()).expect("storage");
        let profile = storage
            .create_profile(
                "测试配置".to_string(),
                ProfileSource::Inline {
                    label: "fixture".to_string(),
                },
            )
            .expect("create");
        let first = storage
            .save_revision(
                profile.id,
                "source: one",
                "effective: one",
                None,
                ValidationReport {
                    valid: true,
                    ..Default::default()
                },
                Default::default(),
            )
            .expect("first revision");
        storage
            .activate_revision(profile.id, first.id)
            .expect("activate first");
        let second = storage
            .save_revision(
                profile.id,
                "source: two",
                "effective: two",
                None,
                ValidationReport {
                    valid: true,
                    ..Default::default()
                },
                OpenAiPolicy {
                    enabled: true,
                    ..Default::default()
                },
            )
            .expect("second revision");
        storage
            .activate_revision(profile.id, second.id)
            .expect("activate second");
        storage.rollback_profile(profile.id).expect("rollback");
        assert_eq!(
            storage.active_effective_config().expect("active"),
            "effective: one"
        );
        assert!(
            !storage
                .load_profile(profile.id)
                .expect("profile")
                .openai_policy
                .enabled
        );

        let backup = storage
            .create_profile(
                "备用订阅".to_string(),
                ProfileSource::Inline {
                    label: "backup".to_string(),
                },
            )
            .expect("create backup");
        let backup_revision = storage
            .save_revision(
                backup.id,
                "source: backup",
                "effective: backup",
                None,
                ValidationReport {
                    valid: true,
                    ..Default::default()
                },
                OpenAiPolicy {
                    enabled: true,
                    auto_maintain: true,
                    ..Default::default()
                },
            )
            .expect("backup revision");
        storage
            .update_profile_revision(backup.id, backup_revision.id)
            .expect("update backup revision");
        assert_eq!(
            storage.state().expect("state").active_profile_id,
            Some(profile.id)
        );
        assert_eq!(
            storage.active_effective_config().expect("active"),
            "effective: one"
        );
        assert_eq!(
            storage
                .load_profile(backup.id)
                .expect("backup profile")
                .active_revision_id,
            Some(backup_revision.id)
        );
        assert!(
            storage
                .load_profile(backup.id)
                .expect("backup profile")
                .openai_policy
                .enabled
        );
        let _ = fs::remove_dir_all(root);
    }
}
