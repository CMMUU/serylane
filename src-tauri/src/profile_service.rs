use crate::config::{inspect_profile, ProfileSummary};
use crate::effective::build_effective_config_with_policy;
use crate::error::{AppError, AppResult};
use crate::models::{
    ConfigRevision, ProfileRecord, ProfileSource, PublicProfileRecord, RoutingMode,
    SubscriptionMetadata, SubscriptionStatus, SubscriptionUsage, ValidationReport,
};
use crate::storage::AppStorage;
use crate::subscription::SubscriptionFetcher;
use crate::user_rules;
use chrono::Utc;
use serde::Serialize;
use std::future::Future;
use tauri::AppHandle;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileOperationResult {
    pub profile: PublicProfileRecord,
    pub revision: ConfigRevision,
    pub summary: ProfileSummary,
    pub updated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OpenAiGenerationStatus {
    NotRequested,
    Started,
    Failed,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionImportResult {
    #[serde(flatten)]
    pub operation: ProfileOperationResult,
    pub created: bool,
    /// True only when this import explicitly selected the subscription.
    pub activated: bool,
    pub open_ai_generation: OpenAiGenerationStatus,
    pub open_ai_error: Option<String>,
    pub observation_error: Option<String>,
}

impl SubscriptionImportResult {
    fn new(operation: ProfileOperationResult, created: bool, activated: bool) -> Self {
        Self {
            operation,
            created,
            activated,
            open_ai_generation: OpenAiGenerationStatus::NotRequested,
            open_ai_error: None,
            observation_error: None,
        }
    }

    pub(crate) fn start_openai_generation<T>(
        &mut self,
        requested: bool,
        start: impl FnOnce() -> AppResult<T>,
    ) {
        // Re-adding an existing URL is not permission to mutate its policy.
        if !requested || !(self.created || self.activated) {
            return;
        }
        match start() {
            Ok(_) => self.open_ai_generation = OpenAiGenerationStatus::Started,
            Err(error) => {
                self.open_ai_generation = OpenAiGenerationStatus::Failed;
                // Do not expose arbitrary paths, subscription URLs or tokens from
                // an underlying error. The successful import remains successful.
                let detail = match error {
                    AppError::Conflict(_) => "已有任务正在运行或配置状态已变化，请稍后重试",
                    AppError::NotFound(_) => "订阅或可用版本不存在，请刷新列表后重试",
                    AppError::Io(_) => "本地订阅或任务数据无法读取，请检查本地存储",
                    _ => "任务初始化失败，请稍后重试",
                };
                self.open_ai_error = Some(format!("{}：{detail}", error.code()));
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum CandidateActivation {
    Never,
    Always,
    IfCurrent,
}

impl CandidateActivation {
    fn for_import(activate_after_import: bool) -> Self {
        if activate_after_import {
            Self::Always
        } else {
            Self::Never
        }
    }

    fn should_activate(self, profile_id: Uuid, active_profile_id: Option<Uuid>) -> bool {
        match self {
            Self::Never => false,
            Self::Always => true,
            Self::IfCurrent => active_profile_id == Some(profile_id),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileDetails {
    pub profile: PublicProfileRecord,
    pub revisions: Vec<ConfigRevision>,
    pub summary: Option<ProfileSummary>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionOverview {
    pub profile: PublicProfileRecord,
    pub summary: Option<ProfileSummary>,
    pub revision_count: usize,
    pub latest_fetched_at: Option<chrono::DateTime<Utc>>,
    pub latest_metadata: Option<SubscriptionMetadata>,
    pub latest_validation: Option<ValidationReport>,
    pub active: bool,
    pub status: Option<SubscriptionStatus>,
}

pub fn list_profiles(app: &AppHandle) -> AppResult<Vec<PublicProfileRecord>> {
    Ok(AppStorage::from_app(app)?
        .list_profiles()?
        .iter()
        .map(PublicProfileRecord::from)
        .collect())
}

pub fn list_subscriptions(app: &AppHandle) -> AppResult<Vec<SubscriptionOverview>> {
    let storage = AppStorage::from_app(app)?;
    let active_profile_id = storage.state()?.active_profile_id;
    storage
        .list_profiles()?
        .into_iter()
        .filter(|profile| matches!(profile.source, ProfileSource::RemoteSubscription { .. }))
        .map(|profile| {
            let revisions = storage.list_revisions(profile.id)?;
            let latest = revisions.first();
            let summary = match profile.active_revision_id {
                Some(revision_id) => storage
                    .load_revision_source(profile.id, revision_id)
                    .ok()
                    .and_then(|source| inspect_profile(&source).ok()),
                None => None,
            };
            Ok(SubscriptionOverview {
                profile: PublicProfileRecord::from(&profile),
                summary,
                revision_count: revisions.len(),
                latest_fetched_at: latest.map(|revision| revision.fetched_at),
                latest_metadata: latest.and_then(|revision| revision.subscription.clone()),
                latest_validation: latest.map(|revision| revision.validation.clone()),
                active: active_profile_id == Some(profile.id),
                status: storage.subscription_status(profile.id),
            })
        })
        .collect()
}

pub fn profile_details(app: &AppHandle, profile_id: Uuid) -> AppResult<ProfileDetails> {
    let storage = AppStorage::from_app(app)?;
    let profile = storage.load_profile(profile_id)?;
    let revisions = storage.list_revisions(profile_id)?;
    let summary = match profile.active_revision_id {
        Some(revision_id) => {
            let source = storage.load_revision_source(profile_id, revision_id)?;
            Some(inspect_profile(&source).map_err(AppError::Config)?)
        }
        None => None,
    };
    Ok(ProfileDetails {
        profile: PublicProfileRecord::from(&profile),
        revisions,
        summary,
    })
}

pub async fn create_inline_profile(
    app: &AppHandle,
    display_name: String,
    source: String,
) -> AppResult<ProfileOperationResult> {
    let storage = AppStorage::from_app(app)?;
    let profile = storage.create_profile(
        display_name.clone(),
        ProfileSource::Inline {
            label: display_name,
        },
    )?;
    match persist_candidate(
        app,
        &storage,
        profile.clone(),
        source,
        None,
        CandidateActivation::Always,
    )
    .await
    {
        Ok(result) => Ok(result),
        Err(error) => {
            let _ = storage.delete_profile(profile.id);
            Err(error)
        }
    }
}

pub async fn create_subscription_profile(
    app: &AppHandle,
    display_name: String,
    url: String,
    user_agent: String,
    activate_after_import: bool,
) -> AppResult<SubscriptionImportResult> {
    let storage = AppStorage::from_app(app)?;
    if let Some(existing) = existing_subscription_result(&storage, &url)? {
        return reuse_subscription(
            existing,
            activate_after_import,
            |profile_id, revision_id| async move {
                activate_profile(app, profile_id, Some(revision_id))
                    .await
                    .map(|details| details.profile)
            },
        )
        .await;
    }
    let fetcher = SubscriptionFetcher::new()?;
    let fetched = fetcher.fetch(&url, &user_agent, None, None).await?;
    let source = fetched
        .content
        .ok_or_else(|| AppError::Subscription("订阅没有返回内容".to_string()))?;
    let permit = user_rules::acquire_configuration(app)?;
    persist_new_subscription(
        &storage,
        display_name,
        ProfileSource::RemoteSubscription { url, user_agent },
        source,
        fetched.metadata,
        activate_after_import,
        &AppProfileConfiguration {
            app,
            permit: &permit,
        },
    )
    .await
}

async fn persist_new_subscription(
    storage: &AppStorage,
    display_name: String,
    subscription: ProfileSource,
    source: String,
    metadata: SubscriptionMetadata,
    activate_after_import: bool,
    configuration: &impl ProfileConfiguration,
) -> AppResult<SubscriptionImportResult> {
    // The initial read is only a fast path: fetching happens before acquiring
    // the configuration permit. Recheck under that permit so another import
    // cannot create a second profile for the same URL while this one fetches.
    if let ProfileSource::RemoteSubscription { url, .. } = &subscription {
        if let Some(existing) = existing_subscription_result(storage, url)? {
            return reuse_subscription_with_configuration(
                storage,
                existing,
                activate_after_import,
                configuration,
            )
            .await;
        }
    }
    let usage = metadata.usage.clone();
    let profile = storage.create_profile(display_name, subscription)?;
    match persist_candidate_with_configuration(
        storage,
        profile.clone(),
        source,
        Some(metadata),
        CandidateActivation::for_import(activate_after_import),
        configuration,
    )
    .await
    {
        Ok(result) => Ok(subscription_import_receipt(
            storage,
            result,
            usage.as_ref(),
            activate_after_import,
        )),
        Err(error) => {
            let _ = storage.delete_profile(profile.id);
            Err(error)
        }
    }
}

fn subscription_import_receipt(
    storage: &AppStorage,
    operation: ProfileOperationResult,
    usage: Option<&SubscriptionUsage>,
    activated: bool,
) -> SubscriptionImportResult {
    let mut result = SubscriptionImportResult::new(operation, true, activated);
    if save_subscription_observation(storage, result.operation.profile.id, usage, None).is_err() {
        // The profile transaction has already committed. An optional observation
        // failure must not turn this into an apparent failed / retryable import.
        result.observation_error =
            Some("订阅已保存，但检查记录未能保存；请检查本地存储后重试".into());
    }
    result
}

fn existing_subscription_result(
    storage: &AppStorage,
    url: &str,
) -> AppResult<Option<ProfileOperationResult>> {
    let Some(profile) = storage.list_profiles()?.into_iter().find(|profile| {
        matches!(
            &profile.source,
            ProfileSource::RemoteSubscription { url: existing_url, .. } if existing_url == url
        )
    }) else {
        return Ok(None);
    };
    // Reuse the selected revision, including an explicit rollback. Neither a
    // network refresh nor selecting the newest revision is implied by adding.
    let revision_id = profile.active_revision_id.ok_or_else(|| {
        AppError::Conflict("该订阅已存在，但没有可用版本；请在订阅列表中更新或删除后重试".into())
    })?;
    let revision = storage.load_revision(profile.id, revision_id)?;
    let source = storage.load_revision_source(profile.id, revision_id)?;
    unchanged_operation_result(&profile, revision, &source).map(Some)
}

async fn reuse_subscription<F, Fut>(
    mut existing: ProfileOperationResult,
    activate_after_import: bool,
    activate: F,
) -> AppResult<SubscriptionImportResult>
where
    F: FnOnce(Uuid, Uuid) -> Fut,
    Fut: Future<Output = AppResult<PublicProfileRecord>>,
{
    if activate_after_import {
        existing.profile = activate(existing.profile.id, existing.revision.id).await?;
    }
    Ok(SubscriptionImportResult::new(
        existing,
        false,
        activate_after_import,
    ))
}

async fn reuse_subscription_with_configuration(
    storage: &AppStorage,
    existing: ProfileOperationResult,
    activate_after_import: bool,
    configuration: &impl ProfileConfiguration,
) -> AppResult<SubscriptionImportResult> {
    // This path already holds the shared permit; calling activate_profile here
    // would attempt to acquire it again and incorrectly report a conflict.
    reuse_subscription(
        existing,
        activate_after_import,
        |id, revision_id| async move {
            activate_profile_with_configuration(storage, id, revision_id, configuration).await?;
            storage
                .load_profile(id)
                .map(|profile| PublicProfileRecord::from(&profile))
        },
    )
    .await
}

fn unchanged_operation_result(
    profile: &ProfileRecord,
    revision: ConfigRevision,
    source: &str,
) -> AppResult<ProfileOperationResult> {
    Ok(ProfileOperationResult {
        profile: PublicProfileRecord::from(profile),
        revision,
        summary: inspect_profile(source).map_err(AppError::Config)?,
        updated: false,
    })
}

fn unchanged_operation_if_source_matches(
    storage: &AppStorage,
    profile: &ProfileRecord,
    revision: &ConfigRevision,
    candidate_source: &str,
) -> AppResult<Option<ProfileOperationResult>> {
    let previous_source = storage.load_revision_source(profile.id, revision.id)?;
    if previous_source != candidate_source {
        return Ok(None);
    }
    unchanged_operation_result(profile, revision.clone(), &previous_source).map(Some)
}

pub async fn refresh_profile(
    app: &AppHandle,
    profile_id: Uuid,
) -> AppResult<ProfileOperationResult> {
    let storage = AppStorage::from_app(app)?;
    let profile = storage.load_profile(profile_id)?;
    let ProfileSource::RemoteSubscription { url, user_agent } = &profile.source else {
        return Err(AppError::Conflict("该配置不是远程订阅".to_string()));
    };
    let request_started = chrono::Utc::now();
    match refresh_subscription_candidate(app, &storage, &profile, url, user_agent).await {
        Ok((result, usage)) => {
            storage.record_subscription_check_since(
                profile_id,
                usage.as_ref(),
                None,
                request_started,
            )?;
            Ok(result)
        }
        Err(error) => {
            storage.record_subscription_check_since(
                profile_id,
                None,
                Some(&error),
                request_started,
            )?;
            Err(error)
        }
    }
}

fn save_subscription_observation(
    storage: &AppStorage,
    profile_id: Uuid,
    usage: Option<&SubscriptionUsage>,
    error: Option<&AppError>,
) -> AppResult<()> {
    storage
        .record_subscription_check(profile_id, usage, error)
        .map_err(|_| {
            AppError::Io(
                if error.is_some() {
                    "订阅更新失败，且检查记录未能保存；请检查本地存储后重试"
                } else {
                    "订阅已获取，但检查记录未能保存；已保留配置结果，请检查本地存储"
                }
                .into(),
            )
        })
}

async fn refresh_subscription_candidate(
    app: &AppHandle,
    storage: &AppStorage,
    profile: &ProfileRecord,
    url: &str,
    user_agent: &str,
) -> AppResult<(ProfileOperationResult, Option<SubscriptionUsage>)> {
    let profile_id = profile.id;
    let latest = storage.list_revisions(profile_id)?.into_iter().next();
    let etag = latest
        .as_ref()
        .and_then(|revision| revision.subscription.as_ref())
        .and_then(|metadata| metadata.etag.as_deref());
    let last_modified = latest
        .as_ref()
        .and_then(|revision| revision.subscription.as_ref())
        .and_then(|metadata| metadata.last_modified.as_deref());
    let fetched = SubscriptionFetcher::new()?
        .fetch(url, user_agent, etag, last_modified)
        .await?;
    let usage = fetched.metadata.usage.clone();
    if fetched.not_modified {
        let revision = latest.ok_or_else(|| AppError::NotFound("当前订阅版本".to_string()))?;
        let source = storage.load_revision_source(profile_id, revision.id)?;
        return unchanged_operation_result(profile, revision, &source)
            .map(|result| (result, usage));
    }
    let source = fetched
        .content
        .ok_or_else(|| AppError::Subscription("订阅内容为空".to_string()))?;
    // The fetch stays outside the configuration transaction. Once mutation is
    // serialized, re-read the latest revision so concurrent refreshes cannot
    // both persist and hot-reload the same response body.
    let permit = user_rules::acquire_configuration(app)?;
    let profile = storage.load_profile(profile_id)?;
    if let Some(revision) = storage.list_revisions(profile_id)?.into_iter().next() {
        if let Some(result) =
            unchanged_operation_if_source_matches(storage, &profile, &revision, &source)?
        {
            return Ok((result, usage));
        }
    }
    persist_candidate_with_permit(
        app,
        storage,
        profile,
        source,
        Some(fetched.metadata),
        CandidateActivation::IfCurrent,
        &permit,
    )
    .await
    .map(|result| (result, usage))
}

pub async fn activate_profile(
    app: &AppHandle,
    profile_id: Uuid,
    revision_id: Option<Uuid>,
) -> AppResult<ProfileDetails> {
    let permit = user_rules::acquire_configuration(app)?;
    let storage = AppStorage::from_app(app)?;
    let profile = storage.load_profile(profile_id)?;
    let revision_id = revision_id
        .or(profile.active_revision_id)
        .ok_or_else(|| AppError::NotFound("配置没有可激活版本".to_string()))?;
    activate_profile_with_configuration(
        &storage,
        profile_id,
        revision_id,
        &AppProfileConfiguration {
            app,
            permit: &permit,
        },
    )
    .await?;
    profile_details(app, profile_id)
}

async fn activate_profile_with_configuration(
    storage: &AppStorage,
    profile_id: Uuid,
    revision_id: Uuid,
    configuration: &impl ProfileConfiguration,
) -> AppResult<()> {
    let effective = activation_candidate(storage, profile_id, revision_id)?;
    configuration
        .apply(storage, &effective.yaml, || {
            commit_active_selection(storage, profile_id, revision_id, &effective.yaml).map(|_| ())
        })
        .await
}

fn activation_candidate(
    storage: &AppStorage,
    profile_id: Uuid,
    revision_id: Uuid,
) -> AppResult<crate::effective::EffectiveConfig> {
    let profile = storage.load_profile(profile_id)?;
    let revision = storage.load_revision(profile_id, revision_id)?;
    let source = storage.load_revision_source(profile_id, revision_id)?;
    build_effective_config_with_policy(
        &source,
        &storage.settings()?,
        profile.routing_mode,
        Some(&revision.openai_policy),
    )
}

pub async fn rollback_profile(app: &AppHandle, profile_id: Uuid) -> AppResult<ProfileDetails> {
    let profile = AppStorage::from_app(app)?.load_profile(profile_id)?;
    let revision_id = profile
        .last_known_good_revision_id
        .ok_or_else(|| AppError::Conflict("没有可回滚的稳定版本".to_string()))?;
    activate_profile(app, profile_id, Some(revision_id)).await
}

pub fn delete_profile(app: &AppHandle, profile_id: Uuid) -> AppResult<()> {
    AppStorage::from_app(app)?.delete_profile(profile_id)
}

pub fn set_routing_mode(
    app: &AppHandle,
    profile_id: Uuid,
    mode: RoutingMode,
) -> AppResult<ProfileDetails> {
    let storage = AppStorage::from_app(app)?;
    let mut profile = storage.load_profile(profile_id)?;
    profile.routing_mode = mode;
    profile.updated_at = Utc::now();
    storage.save_profile(&profile)?;
    profile_details(app, profile_id)
}

/// The production adapter retains native validation and the existing reload /
/// rollback transaction. Tests provide an isolated adapter without a Tauri app
/// or a real core process; neither interface offers a start/proxy operation.
trait ProfileConfiguration {
    async fn validate(&self, candidate: &str) -> AppResult<()>;

    async fn apply(
        &self,
        storage: &AppStorage,
        candidate: &str,
        commit: impl FnOnce() -> AppResult<()>,
    ) -> AppResult<()>;
}

struct AppProfileConfiguration<'a> {
    app: &'a AppHandle,
    permit: &'a user_rules::ConfigurationMutationPermit,
}

impl ProfileConfiguration for AppProfileConfiguration<'_> {
    async fn validate(&self, candidate: &str) -> AppResult<()> {
        user_rules::validate_config(self.app, candidate).await
    }

    async fn apply(
        &self,
        storage: &AppStorage,
        candidate: &str,
        commit: impl FnOnce() -> AppResult<()>,
    ) -> AppResult<()> {
        user_rules::apply_profile_config(self.app, storage, candidate, commit, self.permit).await
    }
}

async fn persist_candidate(
    app: &AppHandle,
    storage: &AppStorage,
    profile: ProfileRecord,
    source: String,
    metadata: Option<crate::models::SubscriptionMetadata>,
    activation: CandidateActivation,
) -> AppResult<ProfileOperationResult> {
    let permit = user_rules::acquire_configuration(app)?;
    persist_candidate_with_permit(app, storage, profile, source, metadata, activation, &permit)
        .await
}

async fn persist_candidate_with_permit(
    app: &AppHandle,
    storage: &AppStorage,
    profile: ProfileRecord,
    source: String,
    metadata: Option<crate::models::SubscriptionMetadata>,
    activation: CandidateActivation,
    permit: &user_rules::ConfigurationMutationPermit,
) -> AppResult<ProfileOperationResult> {
    persist_candidate_with_configuration(
        storage,
        profile,
        source,
        metadata,
        activation,
        &AppProfileConfiguration { app, permit },
    )
    .await
}

async fn persist_candidate_with_configuration(
    storage: &AppStorage,
    profile: ProfileRecord,
    source: String,
    metadata: Option<crate::models::SubscriptionMetadata>,
    activation: CandidateActivation,
    configuration: &impl ProfileConfiguration,
) -> AppResult<ProfileOperationResult> {
    // Fetching can overlap unrelated work; take profile policy/settings only
    // after entering the short mutation transaction, never from the fetch start.
    let profile = storage.load_profile(profile.id)?;
    let activate_app = activation.should_activate(profile.id, storage.state()?.active_profile_id);
    let settings = storage.settings()?;
    let effective = build_effective_config_with_policy(
        &source,
        &settings,
        profile.routing_mode,
        Some(&profile.openai_policy),
    )?;
    let validation = ValidationReport {
        valid: true,
        warnings: effective.summary.warnings.clone(),
        errors: Vec::new(),
        native_core_validated: true,
    };
    let mut revision = None;
    if activate_app {
        configuration
            .apply(storage, &effective.yaml, || {
                let saved = storage.save_revision(
                    profile.id,
                    &source,
                    &effective.yaml,
                    metadata,
                    validation,
                    profile.openai_policy.clone(),
                )?;
                commit_active_selection(storage, profile.id, saved.id, &effective.yaml)?;
                revision = Some(saved);
                Ok(())
            })
            .await?;
    } else {
        configuration.validate(&effective.yaml).await?;
        let saved = storage.save_revision(
            profile.id,
            &source,
            &effective.yaml,
            metadata,
            validation,
            profile.openai_policy.clone(),
        )?;
        if let Err(error) = storage.update_profile_revision(profile.id, saved.id) {
            storage.save_profile(&profile)?;
            return Err(error);
        }
        revision = Some(saved);
    }
    let revision = revision.ok_or_else(|| AppError::Runtime("配置事务未生成版本".to_string()))?;
    let profile = storage.load_profile(profile.id)?;
    Ok(ProfileOperationResult {
        profile: PublicProfileRecord::from(&profile),
        revision,
        summary: effective.summary,
        updated: true,
    })
}

/// Roll back all selection files if any atomic file replacement fails. Source
/// revisions stay immutable; active.yaml uses a freshly merged local overlay.
pub(crate) fn commit_active_selection(
    storage: &AppStorage,
    profile_id: Uuid,
    revision_id: Uuid,
    effective: &str,
) -> AppResult<ProfileRecord> {
    let previous_profile = storage.load_profile(profile_id)?;
    let previous_state = storage.state()?;
    let previous_config = storage.active_runtime_config()?;
    match storage.activate_revision_with_config(profile_id, revision_id, effective) {
        Ok(profile) => Ok(profile),
        Err(error) => {
            let rollback_errors = [
                storage.save_profile(&previous_profile),
                storage.save_state(&previous_state),
                storage.restore_active_runtime_config(previous_config.as_deref()),
            ]
            .into_iter()
            .filter_map(Result::err)
            .map(|error| error.to_string())
            .collect::<Vec<_>>();
            if rollback_errors.is_empty() {
                Err(error)
            } else {
                Err(AppError::Io(format!(
                    "配置选择失败且部分文件恢复失败：{error}；{}",
                    rollback_errors.join("；")
                )))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::OpenAiPolicy;
    use crate::user_rules::{UserRule, UserRulesDocument};
    use std::cell::{Cell, RefCell};

    const IMPORT_SOURCE: &str = "proxies: []\nproxy-groups: []\nrules: ['MATCH,DIRECT']\n";
    const IMPORT_URL: &str =
        "https://subscription.example.invalid/private-path?token=fixture-secret";

    #[derive(Default)]
    struct FakeConfiguration {
        validations: Cell<usize>,
        applications: Cell<usize>,
        reject_validation: bool,
        reject_reload: bool,
        live_config: RefCell<Option<String>>,
    }

    impl ProfileConfiguration for FakeConfiguration {
        async fn validate(&self, _candidate: &str) -> AppResult<()> {
            self.validations.set(self.validations.get() + 1);
            if self.reject_validation {
                return Err(AppError::Config("fixture native validation failure".into()));
            }
            Ok(())
        }

        async fn apply(
            &self,
            _storage: &AppStorage,
            candidate: &str,
            commit: impl FnOnce() -> AppResult<()>,
        ) -> AppResult<()> {
            self.applications.set(self.applications.get() + 1);
            self.validate(candidate).await?;
            if self.reject_reload {
                return Err(AppError::Runtime("fixture reload failure".into()));
            }
            let previous = self.live_config.borrow().clone();
            // A stopped core remains stopped even when selecting a profile.
            if previous.is_some() {
                *self.live_config.borrow_mut() = Some(candidate.to_owned());
            }
            if let Err(error) = commit() {
                *self.live_config.borrow_mut() = previous;
                return Err(error);
            }
            Ok(())
        }
    }

    async fn import_fixture(
        storage: &AppStorage,
        url: &str,
        activate: bool,
        configuration: &FakeConfiguration,
    ) -> AppResult<SubscriptionImportResult> {
        persist_new_subscription(
            storage,
            "fixture subscription".into(),
            ProfileSource::RemoteSubscription {
                url: url.into(),
                user_agent: "clash.meta".into(),
            },
            IMPORT_SOURCE.into(),
            SubscriptionMetadata {
                content_type: Some("text/yaml".into()),
                etag: None,
                last_modified: None,
                bytes: IMPORT_SOURCE.len(),
                usage: Some(SubscriptionUsage {
                    download_bytes: Some(1024),
                    total_bytes: Some(2048),
                    ..Default::default()
                }),
            },
            activate,
            configuration,
        )
        .await
    }

    fn state_snapshot(storage: &AppStorage) -> serde_json::Value {
        serde_json::to_value(storage.state().expect("state")).expect("state JSON")
    }

    async fn reuse_fixture(
        storage: &AppStorage,
        existing: ProfileOperationResult,
        configuration: &FakeConfiguration,
    ) -> AppResult<SubscriptionImportResult> {
        reuse_subscription_with_configuration(storage, existing, true, configuration).await
    }

    #[tokio::test]
    async fn concurrent_fetches_deduplicate_inside_the_serialized_configuration_transaction() {
        let root = tempfile::tempdir().expect("isolated directory");
        let storage = AppStorage::from_root(root.path().to_owned()).expect("storage");
        let configuration = FakeConfiguration::default();
        let fetched = tokio::sync::Barrier::new(2);
        let permit = tokio::sync::Mutex::new(());
        let import = || async {
            assert!(existing_subscription_result(&storage, IMPORT_URL)
                .expect("initial lookup")
                .is_none());
            // Both requests finish their first lookup / fetch before either
            // acquires the same mutation permit used by production.
            fetched.wait().await;
            let _permit = permit.lock().await;
            import_fixture(&storage, IMPORT_URL, false, &configuration)
                .await
                .expect("import")
        };
        let (first, second) = tokio::join!(import(), import());
        assert_ne!(first.created, second.created);
        assert!(!first.activated && !second.activated);
        assert_eq!(first.operation.profile.id, second.operation.profile.id);
        assert_eq!(first.operation.revision.id, second.operation.revision.id);
        assert_eq!(storage.list_profiles().expect("profiles").len(), 1);
        assert_eq!(
            storage
                .list_revisions(first.operation.profile.id)
                .expect("revisions")
                .len(),
            1
        );
        assert_eq!(configuration.validations.get(), 1);
        assert_eq!(configuration.applications.get(), 0);
        assert!(storage.state().expect("state").active_profile_id.is_none());
        assert!(storage
            .active_runtime_config()
            .expect("runtime config")
            .is_none());
    }

    #[tokio::test]
    async fn stale_initial_lookup_can_explicitly_select_the_existing_subscription_without_refresh()
    {
        let root = tempfile::tempdir().expect("isolated directory");
        let storage = AppStorage::from_root(root.path().to_owned()).expect("storage");
        let configuration = FakeConfiguration::default();
        assert!(existing_subscription_result(&storage, IMPORT_URL)
            .expect("first lookup")
            .is_none());
        let first = import_fixture(&storage, IMPORT_URL, false, &configuration)
            .await
            .expect("first import");
        let status = serde_json::to_value(storage.subscription_status(first.operation.profile.id))
            .expect("status");
        // The pending import commits after another request has saved this URL.
        let result = import_fixture(&storage, IMPORT_URL, true, &configuration)
            .await
            .expect("reuse and select");
        assert!(!result.created && result.activated && !result.operation.updated);
        assert_eq!(result.operation.profile.id, first.operation.profile.id);
        assert_eq!(result.operation.revision.id, first.operation.revision.id);
        assert_eq!(storage.list_profiles().expect("profiles").len(), 1);
        assert_eq!(
            storage
                .list_revisions(first.operation.profile.id)
                .expect("revisions")
                .len(),
            1
        );
        assert_eq!(
            serde_json::to_value(storage.subscription_status(first.operation.profile.id))
                .expect("status"),
            status
        );
        assert_eq!(configuration.applications.get(), 1);
        assert_eq!(
            storage.state().expect("state").active_profile_id,
            Some(first.operation.profile.id)
        );
    }

    #[tokio::test]
    async fn first_subscription_save_only_validates_without_selecting_or_starting() {
        let root = tempfile::tempdir().expect("isolated directory");
        let storage = AppStorage::from_root(root.path().to_owned()).expect("storage");
        let configuration = FakeConfiguration::default();
        let before = state_snapshot(&storage);
        let result = import_fixture(&storage, IMPORT_URL, false, &configuration)
            .await
            .expect("import");
        assert!(result.created && result.operation.updated);
        assert!(!result.activated);
        assert_eq!(configuration.validations.get(), 1);
        assert_eq!(configuration.applications.get(), 0);
        assert!(configuration.live_config.borrow().is_none());
        assert_eq!(state_snapshot(&storage), before);
        assert!(storage
            .active_runtime_config()
            .expect("runtime config")
            .is_none());
        assert_eq!(
            result.operation.profile.active_revision_id,
            Some(result.operation.revision.id)
        );
        assert!(result.operation.revision.validation.native_core_validated);
        assert!(storage
            .subscription_status(result.operation.profile.id)
            .expect("observation")
            .usage
            .is_some());
        let json = serde_json::to_value(&result).expect("result JSON");
        for key in [
            "profile",
            "revision",
            "summary",
            "updated",
            "created",
            "activated",
            "openAiGeneration",
            "openAiError",
            "observationError",
        ] {
            assert!(json.get(key).is_some(), "missing flat field {key}");
        }
        assert!(json.get("operation").is_none());
        assert_eq!(json["openAiGeneration"], "not_requested");
        assert!(json["openAiError"].is_null());
        assert!(json["observationError"].is_null());
        assert!(!json.to_string().contains("fixture-secret"));
    }

    #[tokio::test]
    async fn saving_another_subscription_preserves_active_selection_and_running_config() {
        let root = tempfile::tempdir().expect("isolated directory");
        let storage = AppStorage::from_root(root.path().to_owned()).expect("storage");
        let configuration = FakeConfiguration::default();
        let current = import_fixture(
            &storage,
            "https://first.example.invalid/config",
            true,
            &configuration,
        )
        .await
        .expect("current");
        let before = state_snapshot(&storage);
        let runtime = storage.active_runtime_config().expect("runtime config");
        *configuration.live_config.borrow_mut() = runtime.clone();
        let applications = configuration.applications.get();
        let result = import_fixture(&storage, IMPORT_URL, false, &configuration)
            .await
            .expect("save only");
        assert!(result.created && !result.activated);
        assert_eq!(state_snapshot(&storage), before);
        assert_eq!(
            storage.active_runtime_config().expect("runtime config"),
            runtime
        );
        assert_eq!(*configuration.live_config.borrow(), runtime);
        assert_eq!(configuration.applications.get(), applications);
        assert_eq!(
            storage.state().expect("state").active_profile_id,
            Some(current.operation.profile.id)
        );
    }

    #[tokio::test]
    async fn explicit_import_selection_validates_and_selects_without_starting_a_stopped_core() {
        let root = tempfile::tempdir().expect("isolated directory");
        let storage = AppStorage::from_root(root.path().to_owned()).expect("storage");
        let configuration = FakeConfiguration::default();
        let result = import_fixture(&storage, IMPORT_URL, true, &configuration)
            .await
            .expect("select");
        assert!(result.created && result.activated);
        assert_eq!(configuration.applications.get(), 1);
        assert_eq!(configuration.validations.get(), 1);
        assert!(configuration.live_config.borrow().is_none());
        assert_eq!(
            storage.state().expect("state").active_profile_id,
            Some(result.operation.profile.id)
        );
        assert_eq!(
            storage.state().expect("state").active_revision_id,
            Some(result.operation.revision.id)
        );
        assert!(storage
            .active_runtime_config()
            .expect("runtime config")
            .is_some());
    }

    #[tokio::test]
    async fn failed_native_validation_or_reload_keeps_selection_and_removes_incomplete_import() {
        for (activate, reject_validation, reject_reload) in [
            (false, true, false),
            (true, true, false),
            (true, false, true),
        ] {
            let root = tempfile::tempdir().expect("isolated directory");
            let storage = AppStorage::from_root(root.path().to_owned()).expect("storage");
            import_fixture(
                &storage,
                "https://first.example.invalid/config",
                true,
                &FakeConfiguration::default(),
            )
            .await
            .expect("current");
            let before = state_snapshot(&storage);
            let runtime = storage.active_runtime_config().expect("runtime config");
            let configuration = FakeConfiguration {
                reject_validation,
                reject_reload,
                live_config: RefCell::new(runtime.clone()),
                ..Default::default()
            };
            assert!(
                import_fixture(&storage, IMPORT_URL, activate, &configuration)
                    .await
                    .is_err()
            );
            assert_eq!(storage.list_profiles().expect("profiles").len(), 1);
            assert_eq!(state_snapshot(&storage), before);
            assert_eq!(
                storage.active_runtime_config().expect("runtime config"),
                runtime
            );
            assert_eq!(*configuration.live_config.borrow(), runtime);
        }
    }

    #[tokio::test]
    async fn failed_selection_storage_commit_preserves_previous_configuration() {
        let root = tempfile::tempdir().expect("isolated directory");
        let storage = AppStorage::from_root(root.path().to_owned()).expect("storage");
        import_fixture(
            &storage,
            "https://first.example.invalid/config",
            true,
            &FakeConfiguration::default(),
        )
        .await
        .expect("current");
        let before = state_snapshot(&storage);
        let runtime = storage.active_runtime_config().expect("runtime config");
        let configuration = FakeConfiguration {
            live_config: RefCell::new(runtime.clone()),
            ..Default::default()
        };
        std::fs::create_dir(root.path().join("runtime/.active.yaml.backup"))
            .expect("block replacement");
        assert!(import_fixture(&storage, IMPORT_URL, true, &configuration)
            .await
            .is_err());
        assert_eq!(state_snapshot(&storage), before);
        assert_eq!(
            storage.active_runtime_config().expect("runtime config"),
            runtime
        );
        assert_eq!(*configuration.live_config.borrow(), runtime);
        assert_eq!(storage.list_profiles().expect("profiles").len(), 1);
    }

    #[tokio::test]
    async fn duplicate_save_only_is_read_only_even_for_the_current_subscription() {
        for active in [false, true] {
            let root = tempfile::tempdir().expect("isolated directory");
            let storage = AppStorage::from_root(root.path().to_owned()).expect("storage");
            let imported =
                import_fixture(&storage, IMPORT_URL, active, &FakeConfiguration::default())
                    .await
                    .expect("fixture");
            let profile_id = imported.operation.profile.id;
            let directory = root.path().join("profiles").join(profile_id.to_string());
            let metadata = std::fs::read(directory.join("metadata.json")).expect("metadata");
            let observation =
                std::fs::read(directory.join("subscription-status.json")).expect("observation");
            let before = state_snapshot(&storage);
            let runtime = storage.active_runtime_config().expect("runtime config");
            let existing = existing_subscription_result(&storage, IMPORT_URL)
                .expect("lookup")
                .expect("duplicate");
            let mut result = reuse_subscription(existing, false, |_, _| async {
                panic!("save-only duplicate must never call activation")
            })
            .await
            .expect("reuse");
            result.start_openai_generation(true, || -> AppResult<()> {
                panic!("save-only duplicate must never generate a policy")
            });
            assert!(!result.created && !result.activated && !result.operation.updated);
            assert_eq!(result.operation.revision.id, imported.operation.revision.id);
            assert_eq!(
                result.open_ai_generation,
                OpenAiGenerationStatus::NotRequested
            );
            assert_eq!(
                std::fs::read(directory.join("metadata.json")).expect("metadata"),
                metadata
            );
            assert_eq!(
                std::fs::read(directory.join("subscription-status.json")).expect("observation"),
                observation
            );
            assert_eq!(state_snapshot(&storage), before);
            assert_eq!(
                storage.active_runtime_config().expect("runtime config"),
                runtime
            );
            assert_eq!(storage.list_profiles().expect("profiles").len(), 1);
            assert_eq!(
                storage.list_revisions(profile_id).expect("revisions").len(),
                1
            );
        }
    }

    #[tokio::test]
    async fn duplicate_explicit_selection_reuses_the_selected_revision_without_refreshing() {
        let root = tempfile::tempdir().expect("isolated directory");
        let storage = AppStorage::from_root(root.path().to_owned()).expect("storage");
        let configuration = FakeConfiguration::default();
        let imported = import_fixture(&storage, IMPORT_URL, false, &configuration)
            .await
            .expect("fixture");
        let profile_id = imported.operation.profile.id;
        // A more recent revision exists, but re-adding must not undo a rollback.
        storage
            .save_revision(
                profile_id,
                IMPORT_SOURCE,
                IMPORT_SOURCE,
                None,
                ValidationReport::default(),
                OpenAiPolicy::default(),
            )
            .expect("newer unselected revision");
        let observation =
            serde_json::to_value(storage.subscription_status(profile_id)).expect("observation");
        let existing = existing_subscription_result(&storage, IMPORT_URL)
            .expect("lookup")
            .expect("duplicate");
        let mut result = reuse_fixture(&storage, existing, &configuration)
            .await
            .expect("select existing");
        assert!(!result.created && result.activated && !result.operation.updated);
        assert_eq!(
            storage.state().expect("state").active_revision_id,
            Some(imported.operation.revision.id)
        );
        assert_eq!(configuration.applications.get(), 1);
        assert_eq!(
            storage.list_revisions(profile_id).expect("revisions").len(),
            2
        );
        assert_eq!(
            serde_json::to_value(storage.subscription_status(profile_id)).expect("observation"),
            observation
        );
        result.start_openai_generation(true, || Ok(()));
        assert_eq!(result.open_ai_generation, OpenAiGenerationStatus::Started);
    }

    #[tokio::test]
    async fn duplicate_selection_failure_returns_error_without_claiming_activation() {
        let root = tempfile::tempdir().expect("isolated directory");
        let storage = AppStorage::from_root(root.path().to_owned()).expect("storage");
        let imported = import_fixture(&storage, IMPORT_URL, false, &FakeConfiguration::default())
            .await
            .expect("fixture");
        let configuration = FakeConfiguration {
            reject_validation: true,
            ..Default::default()
        };
        let existing = existing_subscription_result(&storage, IMPORT_URL)
            .expect("lookup")
            .expect("duplicate");
        assert!(reuse_fixture(&storage, existing, &configuration)
            .await
            .is_err());
        assert!(storage.state().expect("state").active_profile_id.is_none());
        assert_eq!(
            storage
                .list_revisions(imported.operation.profile.id)
                .expect("revisions")
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn generation_start_failure_is_safe_and_does_not_undo_successful_import() {
        let root = tempfile::tempdir().expect("isolated directory");
        let storage = AppStorage::from_root(root.path().to_owned()).expect("storage");
        let mut result = import_fixture(&storage, IMPORT_URL, false, &FakeConfiguration::default())
            .await
            .expect("fixture");
        result.start_openai_generation(false, || -> AppResult<()> { panic!("not requested") });
        assert_eq!(
            result.open_ai_generation,
            OpenAiGenerationStatus::NotRequested
        );
        result.start_openai_generation(true, || {
            Err::<(), _>(AppError::Conflict(format!("busy {IMPORT_URL}")))
        });
        assert!(result.created && !result.activated);
        assert_eq!(result.open_ai_generation, OpenAiGenerationStatus::Failed);
        let error = result.open_ai_error.as_deref().expect("visible error");
        assert!(error.contains("STATE_CONFLICT"));
        for private in [
            "fixture-secret",
            "private-path",
            "subscription.example.invalid",
        ] {
            assert!(!error.contains(private));
        }
        assert_eq!(storage.list_profiles().expect("profiles").len(), 1);
        assert_eq!(
            storage
                .list_revisions(result.operation.profile.id)
                .expect("revisions")
                .len(),
            1
        );
        assert_eq!(
            serde_json::to_value(&result).expect("JSON")["openAiGeneration"],
            "failed"
        );
    }

    #[tokio::test]
    async fn observation_write_failure_returns_successful_import_with_a_safe_warning() {
        for activated in [false, true] {
            let root = tempfile::tempdir().expect("isolated directory");
            let storage = AppStorage::from_root(root.path().to_owned()).expect("storage");
            let imported = import_fixture(
                &storage,
                IMPORT_URL,
                activated,
                &FakeConfiguration::default(),
            )
            .await
            .expect("committed import");
            let profile_id = imported.operation.profile.id;
            let directory = root.path().join("profiles").join(profile_id.to_string());
            let before_profile =
                std::fs::read(directory.join("metadata.json")).expect("profile bytes");
            let before_status =
                std::fs::read(directory.join("subscription-status.json")).expect("status bytes");
            let before_state = state_snapshot(&storage);
            let before_runtime = storage.active_runtime_config().expect("runtime config");
            std::fs::create_dir(directory.join(".subscription-status.json.backup"))
                .expect("block observation replacement");
            let result = subscription_import_receipt(&storage, imported.operation, None, activated);
            assert!(result.created && result.operation.updated);
            assert_eq!(result.activated, activated);
            assert_eq!(
                result.open_ai_generation,
                OpenAiGenerationStatus::NotRequested
            );
            assert!(result.open_ai_error.is_none());
            let warning = result
                .observation_error
                .as_deref()
                .expect("visible observation warning");
            assert_eq!(
                warning,
                "订阅已保存，但检查记录未能保存；请检查本地存储后重试"
            );
            for secret in ["fixture-secret", "private-path"] {
                assert!(!warning.contains(secret));
            }
            assert!(!warning.contains(root.path().to_str().expect("fixture path")));
            assert_eq!(state_snapshot(&storage), before_state);
            assert_eq!(
                storage.active_runtime_config().expect("runtime config"),
                before_runtime
            );
            assert_eq!(
                std::fs::read(directory.join("metadata.json")).expect("profile bytes"),
                before_profile
            );
            assert_eq!(
                std::fs::read(directory.join("subscription-status.json")).expect("status bytes"),
                before_status
            );
            assert_eq!(storage.list_profiles().expect("profiles").len(), 1);
            assert_eq!(
                storage.list_revisions(profile_id).expect("revisions").len(),
                1
            );
            assert_eq!(
                serde_json::to_value(&result).expect("receipt JSON")["observationError"],
                warning
            );
        }
    }

    #[test]
    fn save_only_and_refresh_have_distinct_activation_intents() {
        let profile = Uuid::new_v4();
        for active in [None, Some(profile), Some(Uuid::new_v4())] {
            assert!(!CandidateActivation::for_import(false).should_activate(profile, active));
            assert!(CandidateActivation::for_import(true).should_activate(profile, active));
            assert_eq!(
                CandidateActivation::IfCurrent.should_activate(profile, active),
                active == Some(profile)
            );
        }
    }

    #[test]
    fn subscription_observation_write_failure_is_explicit_and_preserves_saved_data() {
        let root = tempfile::tempdir().expect("isolated directory");
        let storage = AppStorage::from_root(root.path().to_owned()).expect("storage");
        let profile = storage
            .create_profile(
                "fixture".into(),
                ProfileSource::RemoteSubscription {
                    url: "https://subscription.example.invalid/config".into(),
                    user_agent: "clash.meta".into(),
                },
            )
            .expect("profile");
        save_subscription_observation(&storage, profile.id, None, None).expect("initial record");
        let directory = root.path().join("profiles").join(profile.id.to_string());
        let status = directory.join("subscription-status.json");
        let before = std::fs::read(&status).expect("saved status");
        let before_profile = std::fs::read(directory.join("metadata.json")).expect("profile bytes");
        std::fs::create_dir(directory.join(".subscription-status.json.backup"))
            .expect("block atomic replacement");
        let error = save_subscription_observation(&storage, profile.id, None, None)
            .expect_err("must report cache write failure");
        assert!(error.to_string().contains("订阅已获取，但检查记录未能保存"));
        assert!(!error
            .to_string()
            .contains(&root.path().to_string_lossy().to_string()));
        assert_eq!(std::fs::read(&status).expect("unchanged status"), before);
        assert_eq!(
            std::fs::read(directory.join("metadata.json")).expect("unchanged profile"),
            before_profile
        );
    }

    #[test]
    fn activating_old_revision_rebuilds_current_global_overrides() {
        let root = std::env::temp_dir().join(format!("routedeck-activation-{}", Uuid::new_v4()));
        let storage = AppStorage::from_root(root.clone()).expect("storage");
        let profile = storage
            .create_profile(
                "fixture".into(),
                ProfileSource::Inline {
                    label: "fixture".into(),
                },
            )
            .expect("profile");
        let source = "proxies: []\nproxy-groups: []\nrules: [MATCH,DIRECT]\n";
        // Use a quoted one-line sequence: each rule is a single YAML scalar.
        let source = source.replace("[MATCH,DIRECT]", "['MATCH,DIRECT']");
        let revision = storage
            .save_revision(
                profile.id,
                &source,
                "stale-cache: true\n",
                None,
                ValidationReport {
                    valid: true,
                    ..Default::default()
                },
                OpenAiPolicy::default(),
            )
            .expect("revision");
        let mut document = UserRulesDocument::default();
        document.rules.push(UserRule {
            id: "current".into(),
            enabled: true,
            rule: "DOMAIN,example.com,REJECT".into(),
            note: String::new(),
        });
        storage
            .save_user_rules(&document)
            .expect("current global rule");
        let candidate = activation_candidate(&storage, profile.id, revision.id).expect("candidate");
        assert!(!candidate.yaml.contains("stale-cache"));
        let yaml: serde_yaml::Value = serde_yaml::from_str(&candidate.yaml).expect("yaml");
        assert_eq!(yaml["rules"][0].as_str(), Some("DOMAIN,example.com,REJECT"));
        commit_active_selection(&storage, profile.id, revision.id, &candidate.yaml)
            .expect("select");
        assert_eq!(
            storage.active_runtime_config().expect("snapshot"),
            Some(candidate.yaml)
        );
        assert_eq!(
            storage
                .load_revision_effective(profile.id, revision.id)
                .expect("immutable cache"),
            "stale-cache: true\n"
        );
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn identical_refresh_source_reuses_the_existing_revision() {
        let root = std::env::temp_dir().join(format!("routedeck-refresh-{}", Uuid::new_v4()));
        let storage = AppStorage::from_root(root.clone()).expect("storage");
        let profile = storage
            .create_profile(
                "fixture".into(),
                ProfileSource::RemoteSubscription {
                    url: "https://subscription.example.invalid/config".into(),
                    user_agent: "clash.meta".into(),
                },
            )
            .expect("profile");
        let source = "proxies: []\nproxy-groups: []\nrules: ['MATCH,DIRECT']\n";
        let revision = storage
            .save_revision(
                profile.id,
                source,
                source,
                None,
                ValidationReport {
                    valid: true,
                    ..Default::default()
                },
                OpenAiPolicy::default(),
            )
            .expect("revision");

        let result = unchanged_operation_if_source_matches(&storage, &profile, &revision, source)
            .expect("comparison")
            .expect("unchanged result");
        assert!(!result.updated);
        assert_eq!(result.revision.id, revision.id);
        assert_eq!(
            storage.list_revisions(profile.id).expect("revisions").len(),
            1
        );
        assert!(unchanged_operation_if_source_matches(
            &storage,
            &profile,
            &revision,
            "proxies: []\nrules: []\n",
        )
        .expect("comparison")
        .is_none());
        std::fs::remove_dir_all(root).expect("cleanup");
    }
}
