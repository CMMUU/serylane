mod app_binding;
mod app_log;
mod app_update;
mod appearance;
mod config;
mod connection_feedback;
mod diagnostics;
mod effective;
mod error;
mod local_routing;
mod mihomo_api;
mod models;
mod network_safety;
mod node_details;
mod node_selection;
mod openai_cost;
mod openai_policy;
mod openai_stability;
mod platform;
mod profile_service;
mod program_proxy;
mod runtime;
mod session_resume;
mod startup;
mod storage;
mod subscription;
mod traffic_monitor;
pub mod tun_service;
mod user_rules;
#[cfg(all(test, windows, target_env = "msvc"))]
mod windows_test_manifest;

use config::{inspect_profile, ProfileSummary};
use connection_feedback::{ConnectionFeedback, Phase as ConnectionPhase};
use diagnostics::DiagnosticCheck;
use error::{AppError, AppErrorDto, StartupReason};
use mihomo_api::MihomoApiClient;
use models::{AppSettings, NetworkMode, PublicAppSettings, PublicProfileRecord, RoutingMode};
use openai_policy::{OpenAiPolicyTaskManager, OpenAiPolicyTaskSnapshot};
use platform::SystemProxyStatus;
use profile_service::{
    ProfileDetails, ProfileOperationResult, SubscriptionImportResult, SubscriptionOverview,
};
use runtime::{BinaryInfo, MihomoRuntime, RuntimeLog, RuntimeStatus};
use serde::Serialize;
use serde_json::Value;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use storage::AppStorage;
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager, State, WindowEvent,
};
use traffic_monitor::{GlobalTrafficMonitor, GlobalTrafficSnapshot, TRAY_ID};
use tun_service::TunHelperStatus;
use user_rules::{UserRule, UserRulesState, UserRulesValidation};
use uuid::Uuid;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AppInfo {
    product_name: &'static str,
    version: &'static str,
    target_os: &'static str,
    target_arch: &'static str,
}

#[derive(Default)]
struct SubscriptionImportGuard {
    running: AtomicBool,
}

struct SubscriptionImportPermit<'a> {
    guard: &'a SubscriptionImportGuard,
}

impl SubscriptionImportGuard {
    fn acquire(&self) -> Result<SubscriptionImportPermit<'_>, AppError> {
        self.running
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| AppError::Conflict("已有订阅正在导入，请等待当前校验完成".to_string()))?;
        Ok(SubscriptionImportPermit { guard: self })
    }
}

impl Drop for SubscriptionImportPermit<'_> {
    fn drop(&mut self) {
        self.guard.running.store(false, Ordering::Release);
    }
}

fn dto(error: AppError) -> AppErrorDto {
    // Arbitrary error text can contain subscription/configuration secrets.
    // Network diagnostics log their separately sanitized, structured cause.
    app_log::record(
        2,
        app_log::Area::App,
        &format!("操作失败：{}（{}）", error.code(), error.stage()),
    );
    error.dto()
}

#[tauri::command]
fn app_info() -> AppInfo {
    AppInfo {
        product_name: "Serylane",
        version: env!("CARGO_PKG_VERSION"),
        target_os: std::env::consts::OS,
        target_arch: std::env::consts::ARCH,
    }
}

#[tauri::command]
fn get_settings(app: AppHandle) -> Result<PublicAppSettings, AppErrorDto> {
    AppStorage::from_app(&app)
        .and_then(|storage| storage.settings())
        .map(|settings| PublicAppSettings::from(&settings))
        .map_err(dto)
}

#[tauri::command]
fn get_user_rules(app: AppHandle) -> Result<UserRulesState, AppErrorDto> {
    user_rules::get(&app).map_err(dto)
}

#[tauri::command]
fn parse_user_rules_text(text: String) -> Result<Vec<UserRule>, AppErrorDto> {
    user_rules::parse_text(&text).map_err(dto)
}

#[tauri::command]
async fn validate_user_rules(
    app: AppHandle,
    rules: Vec<UserRule>,
) -> Result<UserRulesValidation, AppErrorDto> {
    user_rules::validate(&app, rules).await.map_err(dto)
}

#[tauri::command]
async fn save_user_rules(
    app: AppHandle,
    rules: Vec<UserRule>,
    expected_revision: u64,
) -> Result<UserRulesState, AppErrorDto> {
    user_rules::save(&app, rules, expected_revision)
        .await
        .map_err(dto)
}

#[tauri::command]
async fn rollback_user_rules(
    app: AppHandle,
    revision_id: String,
    expected_revision: u64,
) -> Result<UserRulesState, AppErrorDto> {
    user_rules::rollback(&app, &revision_id, expected_revision)
        .await
        .map_err(dto)
}

#[tauri::command]
fn set_app_theme(app: AppHandle, theme: String) -> Result<PublicAppSettings, AppErrorDto> {
    appearance::native_theme(&theme).map_err(dto)?;
    let storage = AppStorage::from_app(&app).map_err(dto)?;
    let settings = appearance::persist_theme(&storage, &theme).map_err(dto)?;
    appearance::apply_native_theme(&app, &settings.theme).map_err(dto)?;
    Ok(PublicAppSettings::from(&settings))
}

#[tauri::command]
fn update_settings(
    app: AppHandle,
    state: State<'_, MihomoRuntime>,
    traffic: State<'_, GlobalTrafficMonitor>,
    settings: PublicAppSettings,
) -> Result<PublicAppSettings, AppErrorDto> {
    let _configuration = user_rules::acquire_configuration(&app).map_err(dto)?;
    let storage = AppStorage::from_app(&app).map_err(dto)?;
    let current = storage.settings().map_err(dto)?;
    let changes_runtime_network = settings.network_mode != current.network_mode
        || settings.mixed_port != current.mixed_port
        || settings.controller_port != current.controller_port;
    if state.status(Some(&app)).phase == models::RuntimePhase::Running && changes_runtime_network {
        return Err(dto(AppError::Conflict(
            "请先停止 Mihomo 再修改网络模式或运行端口".to_string(),
        )));
    }
    if settings.network_mode == NetworkMode::Tun
        && settings.network_mode != current.network_mode
        && !tun_service::status().ready()
    {
        return Err(dto(AppError::Platform(
            "TUN Helper 尚未授权或不可用".to_string(),
        )));
    }
    if settings.mixed_port == settings.controller_port {
        return Err(dto(AppError::InvalidInput(
            "mixed port 与 controller port 不能相同".to_string(),
        )));
    }
    if settings.mixed_port < 1024 || settings.controller_port < 1024 {
        return Err(dto(AppError::InvalidInput(
            "代理端口和控制端口必须在 1024 到 65535 范围".to_string(),
        )));
    }
    appearance::native_theme(&settings.theme).map_err(dto)?;
    if !(1..=90).contains(&settings.diagnostics_retention_days) {
        return Err(dto(AppError::InvalidInput(
            "诊断报告保留天数必须在 1 到 90 之间".to_string(),
        )));
    }
    app_log::validate_retention_days(settings.app_log_retention_days).map_err(dto)?;
    let settings = settings.merge_secret(&current);
    startup::save_startup_settings(&app, &storage, &current, &settings).map_err(dto)?;
    app_log::set_retention_days(settings.app_log_retention_days);
    if startup::migrate_login_entry(&app, &settings).is_err() {
        app_log::record(
            1,
            app_log::Area::Settings,
            "设置已保存，但旧登录启动项迁移未完成；请核对系统启动应用列表",
        );
    }
    app_log::record(
        0,
        app_log::Area::Settings,
        &format!(
            "设置已保存：模式={:?}；登录启动={}；静默={}；恢复={}",
            settings.network_mode,
            settings.launch_at_login,
            settings.silent_startup,
            settings.restore_last_session
        ),
    );
    if settings.restore_last_session != current.restore_last_session {
        app.state::<session_resume::SessionResumeManager>()
            .cancel_pending(&app);
    }
    appearance::apply_native_theme(&app, &settings.theme).map_err(dto)?;
    traffic.set_enabled(&app, settings.show_global_traffic);
    Ok(PublicAppSettings::from(&settings))
}

#[tauri::command]
fn global_traffic_snapshot(state: State<'_, GlobalTrafficMonitor>) -> GlobalTrafficSnapshot {
    state.snapshot()
}

#[tauri::command]
fn inspect_mihomo_yaml(source: String) -> Result<ProfileSummary, AppErrorDto> {
    inspect_profile(&source)
        .map_err(AppError::Config)
        .map_err(dto)
}

#[tauri::command]
fn list_profiles(app: AppHandle) -> Result<Vec<PublicProfileRecord>, AppErrorDto> {
    profile_service::list_profiles(&app).map_err(dto)
}

#[tauri::command]
fn list_subscriptions(app: AppHandle) -> Result<Vec<SubscriptionOverview>, AppErrorDto> {
    profile_service::list_subscriptions(&app).map_err(dto)
}

#[tauri::command]
fn get_profile_details(app: AppHandle, profile_id: Uuid) -> Result<ProfileDetails, AppErrorDto> {
    profile_service::profile_details(&app, profile_id).map_err(dto)
}

#[tauri::command]
fn get_active_profile(app: AppHandle) -> Result<Option<ProfileDetails>, AppErrorDto> {
    let storage = AppStorage::from_app(&app).map_err(dto)?;
    storage
        .state()
        .map_err(dto)?
        .active_profile_id
        .map(|profile_id| profile_service::profile_details(&app, profile_id))
        .transpose()
        .map_err(dto)
}

#[tauri::command]
async fn create_inline_profile(
    app: AppHandle,
    display_name: String,
    source: String,
) -> Result<ProfileOperationResult, AppErrorDto> {
    profile_service::create_inline_profile(&app, display_name, source)
        .await
        .map_err(dto)
}

#[tauri::command]
async fn create_subscription_profile(
    app: AppHandle,
    guard: State<'_, SubscriptionImportGuard>,
    display_name: String,
    url: String,
    user_agent: String,
    generate_openai: Option<bool>,
    activate_after_import: Option<bool>,
) -> Result<SubscriptionImportResult, AppErrorDto> {
    let _permit = guard.acquire().map_err(dto)?;
    let mut result = profile_service::create_subscription_profile(
        &app,
        display_name,
        url,
        user_agent,
        activate_after_import.unwrap_or(false),
    )
    .await
    .map_err(dto)?;
    let profile_id = result.operation.profile.id;
    result.start_openai_generation(generate_openai.unwrap_or(false), || {
        openai_policy::start_generation(&app, profile_id, true)
    });
    Ok(result)
}

#[tauri::command]
async fn refresh_profile(
    app: AppHandle,
    profile_id: Uuid,
) -> Result<ProfileOperationResult, AppErrorDto> {
    let result = profile_service::refresh_profile(&app, profile_id)
        .await
        .map_err(dto)?;
    if result.updated && result.profile.openai_policy.auto_maintain {
        let _ = openai_policy::start_generation(&app, profile_id, true);
    }
    Ok(result)
}

#[tauri::command]
async fn activate_profile(
    app: AppHandle,
    profile_id: Uuid,
    revision_id: Option<Uuid>,
) -> Result<ProfileDetails, AppErrorDto> {
    profile_service::activate_profile(&app, profile_id, revision_id)
        .await
        .map_err(dto)
}

#[tauri::command]
async fn rollback_profile(app: AppHandle, profile_id: Uuid) -> Result<ProfileDetails, AppErrorDto> {
    profile_service::rollback_profile(&app, profile_id)
        .await
        .map_err(dto)
}

#[tauri::command]
fn delete_profile(app: AppHandle, profile_id: Uuid) -> Result<(), AppErrorDto> {
    let _configuration = user_rules::acquire_configuration(&app).map_err(dto)?;
    profile_service::delete_profile(&app, profile_id).map_err(dto)
}

#[tauri::command]
fn probe_mihomo(app: AppHandle) -> BinaryInfo {
    runtime::probe_binary(Some(&app))
}

#[tauri::command]
fn runtime_status(app: AppHandle, state: State<'_, MihomoRuntime>) -> RuntimeStatus {
    state.status(Some(&app))
}

#[tauri::command]
async fn start_active_profile(
    app: AppHandle,
    state: State<'_, MihomoRuntime>,
) -> Result<RuntimeStatus, AppErrorDto> {
    let _configuration = session_resume::acquire_manual_configuration(&app)
        .await
        .map_err(dto)?;
    let storage = AppStorage::from_app(&app).map_err(dto)?;
    let settings = storage.settings().map_err(dto)?;
    start_runtime_for_settings(&app, &state, &settings, None).await?;
    if let Err(error) = app
        .state::<session_resume::SessionResumeManager>()
        .while_open(|| storage.set_desired_running(true))
    {
        let _ = platform::restore_system_proxy(&app);
        let _ = state.stop(Some(&app));
        return Err(dto(error));
    }
    Ok(state.status(Some(&app)))
}

async fn start_runtime_for_settings(
    app: &AppHandle,
    state: &State<'_, MihomoRuntime>,
    settings: &AppSettings,
    automatic_proxy: Option<&platform::ResumeProxyGuard>,
) -> Result<(), AppErrorDto> {
    let feedback = app.state::<ConnectionFeedback>();
    let operation = feedback.begin(app, settings.network_mode, ConnectionPhase::Validating);
    let result = start_runtime_inner(app, state, settings, automatic_proxy).await;
    match &result {
        Ok(()) => {
            feedback.advance(app, operation, ConnectionPhase::Enabled);
            if settings.network_mode != NetworkMode::Tun {
                feedback.probe(app, operation, settings.clone());
            }
        }
        Err(error) => {
            if app
                .state::<session_resume::SessionResumeManager>()
                .is_shutting_down()
            {
                return result;
            }
            // cleanup may invalidate a background probe; publish failure on the
            // current generation, with no stale old network result attached.
            let current = feedback.snapshot().operation;
            feedback.fail(app, current, error);
        }
    }
    result
}

async fn start_runtime_inner(
    app: &AppHandle,
    state: &State<'_, MihomoRuntime>,
    settings: &AppSettings,
    automatic_proxy: Option<&platform::ResumeProxyGuard>,
) -> Result<(), AppErrorDto> {
    let storage = AppStorage::from_app(app).map_err(dto)?;
    let effective = active_effective_config(app, settings).map_err(dto)?;
    app_log::record(
        0,
        app_log::Area::Runtime,
        &format!(
            "准备启动：模式={:?}；本地端口={}；控制端口={}",
            settings.network_mode, settings.mixed_port, settings.controller_port
        ),
    );
    #[cfg(not(windows))]
    if settings.network_mode == NetworkMode::Tun {
        runtime::validate_source(app, &effective.yaml).map_err(dto)?;
    }
    app.state::<session_resume::SessionResumeManager>()
        .while_open(|| {
            storage.mark_clean_shutdown(false)?;
            if settings.network_mode == NetworkMode::Tun {
                state.start_tun(app, &effective.yaml)
            } else {
                state.start(app, &effective.yaml)
            }
        })
        .map_err(dto)?;
    finish_runtime_start(app, state, settings, automatic_proxy).await?;
    app_log::record(
        0,
        app_log::Area::Runtime,
        "核心启动验收通过；不代表 Codex 模型请求已验证",
    );
    if app
        .state::<session_resume::SessionResumeManager>()
        .is_shutting_down()
    {
        return Err(dto(AppError::Conflict("应用正在退出，启动已取消".into())));
    }
    Ok(())
}

async fn finish_runtime_start(
    app: &AppHandle,
    state: &State<'_, MihomoRuntime>,
    settings: &AppSettings,
    automatic_proxy: Option<&platform::ResumeProxyGuard>,
) -> Result<(), AppErrorDto> {
    let api = MihomoApiClient::new(settings).map_err(dto)?;
    let ready_timeout = if settings.network_mode == NetworkMode::Tun {
        Duration::from_secs(30)
    } else {
        Duration::from_secs(8)
    };
    if let Err(error) = api.wait_ready(ready_timeout).await {
        let _ = state.stop(Some(app));
        return Err(dto(AppError::startup(
            StartupReason::CoreNotReady,
            error,
            None,
        )));
    }
    if settings.network_mode == NetworkMode::SystemProxy {
        let feedback = app.state::<ConnectionFeedback>();
        feedback.advance(
            app,
            feedback.snapshot().operation,
            ConnectionPhase::Applying,
        );
        let proxy_result = app
            .state::<session_resume::SessionResumeManager>()
            .while_open(|| match automatic_proxy {
                Some(guard) => {
                    platform::enable_automatic_system_proxy(app, settings.mixed_port, guard)?;
                    if let Err(error) = platform::verify_system_proxy(settings.mixed_port) {
                        let restored = platform::restore_system_proxy(app).is_ok();
                        return Err(AppError::startup(
                            StartupReason::ProxyApply,
                            error,
                            Some(restored),
                        ));
                    }
                    Ok(())
                }
                None => platform::proxy_transaction(app, || {
                    platform::enable_system_proxy(app, settings.mixed_port)?;
                    platform::verify_system_proxy(settings.mixed_port)
                }),
            });
        if let Err(error) = proxy_result {
            let _ = state.stop(Some(app));
            return Err(dto(error));
        }
        // Internet health is observed after local enablement; it never gates
        // this command or tears down an already enabled system proxy.
    } else if settings.network_mode == NetworkMode::Tun {
        tokio::time::sleep(Duration::from_millis(900)).await;
        #[cfg(not(windows))]
        {
            let helper = tun_service::status();
            if !helper.ready() || !helper.runtime_running {
                let _ = state.stop(Some(app));
                return Err(dto(AppError::Runtime(
                    helper
                        .last_error
                        .unwrap_or_else(|| helper.message.to_string()),
                )));
            }
        }
        #[cfg(windows)]
        {
            let status = state.status(Some(app));
            if status.phase != models::RuntimePhase::Running {
                let _ = state.stop(Some(app));
                return Err(dto(AppError::Runtime(status.message)));
            }
        }
        let started_at = state.status(Some(app)).started_at;
        if let Some(error_log) = state.logs(100).into_iter().rev().find(|log| {
            started_at.is_some_and(|started| log.timestamp >= started)
                && log.level == "error"
                && (log.message.to_lowercase().contains("tun")
                    || log.message.to_lowercase().contains("permission")
                    || log.message.to_lowercase().contains("route"))
        }) {
            let _ = state.stop(Some(app));
            return Err(dto(AppError::Runtime(error_log.message)));
        }
        match network_safety::verify_tun_route().await {
            Ok(report) => {
                let _ = app.emit("network-safety-report", &report);
            }
            Err(error) => {
                let _ = state.stop(Some(app));
                return Err(dto(error));
            }
        }
    }
    Ok(())
}

fn active_effective_config(
    app: &AppHandle,
    settings: &AppSettings,
) -> Result<effective::EffectiveConfig, AppError> {
    let storage = AppStorage::from_app(app)?;
    let persistent = storage.state()?;
    let profile_id = persistent
        .active_profile_id
        .ok_or_else(|| AppError::NotFound("没有活动配置".to_string()))?;
    let revision_id = persistent
        .active_revision_id
        .ok_or_else(|| AppError::NotFound("没有活动配置版本".to_string()))?;
    let source = storage.load_revision_source(profile_id, revision_id)?;
    let profile = storage.load_profile(profile_id)?;
    effective::build_effective_config_with_policy(
        &source,
        settings,
        profile.routing_mode,
        Some(&profile.openai_policy),
    )
}

#[tauri::command]
fn tun_helper_status(app: AppHandle, state: State<'_, MihomoRuntime>) -> TunHelperStatus {
    let status = tun_service::status();
    #[cfg(windows)]
    {
        let mut status = status;
        if AppStorage::from_app(&app)
            .and_then(|storage| storage.settings())
            .is_ok_and(|settings| settings.network_mode == NetworkMode::Tun)
        {
            let runtime = state.status(Some(&app));
            status.runtime_running = runtime.phase == models::RuntimePhase::Running;
            status.runtime_pid = runtime.pid;
            status.runtime_version = runtime.version;
            status.last_error = runtime.last_error;
        }
        status
    }
    #[cfg(not(windows))]
    {
        let _ = (app, state);
        status
    }
}

#[tauri::command]
async fn install_tun_helper() -> Result<TunHelperStatus, AppErrorDto> {
    tauri::async_runtime::spawn_blocking(tun_service::install)
        .await
        .map_err(|error| dto(AppError::Platform(error.to_string())))?
        .map_err(dto)
}

#[tauri::command]
async fn repair_tun_helper() -> Result<TunHelperStatus, AppErrorDto> {
    tauri::async_runtime::spawn_blocking(tun_service::repair)
        .await
        .map_err(|error| dto(AppError::Platform(error.to_string())))?
        .map_err(dto)
}

#[tauri::command]
async fn uninstall_tun_helper(
    app: AppHandle,
    state: State<'_, MihomoRuntime>,
) -> Result<(), AppErrorDto> {
    if state.status(Some(&app)).phase == models::RuntimePhase::Running {
        return Err(dto(AppError::Conflict(
            "请先停止 Mihomo 再卸载 TUN Helper".to_string(),
        )));
    }
    tauri::async_runtime::spawn_blocking(tun_service::uninstall)
        .await
        .map_err(|error| dto(AppError::Platform(error.to_string())))?
        .map_err(dto)
}

#[tauri::command]
fn open_tun_helper_settings() -> Result<(), AppErrorDto> {
    tun_service::open_approval_settings().map_err(dto)
}

#[tauri::command]
async fn prepare_tun_active_profile(app: AppHandle) -> Result<(), AppErrorDto> {
    let helper = tun_service::status();
    if !helper.ready() {
        return Err(dto(AppError::Platform(helper.message)));
    }
    let storage = AppStorage::from_app(&app).map_err(dto)?;
    let mut settings = storage.settings().map_err(dto)?;
    settings.network_mode = NetworkMode::Tun;
    let effective = active_effective_config(&app, &settings).map_err(dto)?;
    runtime::validate_source(&app, &effective.yaml).map_err(dto)?;
    #[cfg(windows)]
    {
        Ok(())
    }
    #[cfg(not(windows))]
    {
        tauri::async_runtime::spawn_blocking(move || tun_service::prepare(&effective.yaml))
            .await
            .map_err(|error| dto(AppError::Runtime(error.to_string())))?
            .map_err(dto)
    }
}

#[tauri::command]
async fn stop_mihomo(app: AppHandle) -> Result<RuntimeStatus, AppErrorDto> {
    stop_runtime_by_user(&app).await
}

async fn stop_runtime_by_user(app: &AppHandle) -> Result<RuntimeStatus, AppErrorDto> {
    let _configuration = session_resume::acquire_manual_configuration(app)
        .await
        .map_err(dto)?;
    let storage = AppStorage::from_app(app).map_err(dto)?;
    // Persist the accepted Stop intent before cleanup. Even an interrupted
    // cleanup or failed proxy restoration must not turn it into an auto-start.
    let state = app.state::<MihomoRuntime>();
    app.state::<session_resume::SessionResumeManager>()
        .while_open(|| {
            // The state.json read/modify/write shares cleanup's transition lock.
            // Shutdown cannot read the old true intent and overwrite this Stop.
            storage.set_desired_running(false)?;
            app_log::record(
                0,
                app_log::Area::Runtime,
                "用户停止代理；下次启动应用时保持停止",
            );
            let proxy_result = platform::restore_system_proxy(app);
            let status = state.stop(Some(app));
            storage.mark_clean_shutdown(proxy_result.is_ok() && status.is_ok())?;
            let status = status?;
            proxy_result?;
            Ok(status)
        })
        .map_err(dto)
}

#[tauri::command]
fn runtime_logs(state: State<'_, MihomoRuntime>, limit: Option<usize>) -> Vec<RuntimeLog> {
    state.logs(limit.unwrap_or(300))
}

#[tauri::command]
fn application_logs() -> Result<app_log::Snapshot, AppErrorDto> {
    app_log::snapshot().map_err(|error| error.dto())
}

#[tauri::command]
fn clear_application_logs() -> Result<(), AppErrorDto> {
    app_log::clear().map_err(|error| error.dto())
}

#[tauri::command]
fn clear_runtime_logs(state: State<'_, MihomoRuntime>) {
    state.clear_logs();
}

#[tauri::command]
fn system_proxy_status(app: AppHandle) -> SystemProxyStatus {
    platform::status(&app)
}

#[tauri::command]
fn check_system_proxy_compatibility(
    app: AppHandle,
) -> Result<platform::ProxyCompatibility, AppErrorDto> {
    let settings = AppStorage::from_app(&app)
        .and_then(|storage| storage.settings())
        .map_err(dto)?;
    platform::proxy_compatibility(settings.mixed_port).map_err(dto)
}

fn can_switch_without_restart(previous: NetworkMode, next: NetworkMode) -> bool {
    previous != NetworkMode::Tun && next != NetworkMode::Tun
}

#[tauri::command]
async fn set_network_mode(
    app: AppHandle,
    state: State<'_, MihomoRuntime>,
    mode: NetworkMode,
) -> Result<PublicAppSettings, AppErrorDto> {
    let _configuration = session_resume::acquire_manual_configuration(&app)
        .await
        .map_err(dto)?;
    let storage = AppStorage::from_app(&app).map_err(dto)?;
    let current = storage.settings().map_err(dto)?;
    if state.running_identity().is_some() {
        if !can_switch_without_restart(current.network_mode, mode) {
            return Err(dto(AppError::Conflict(
                "请先停止代理服务，再切换 TUN 模式".into(),
            )));
        }
        let feedback = app.state::<ConnectionFeedback>();
        let operation = feedback.begin(&app, mode, ConnectionPhase::Applying);
        if mode == NetworkMode::SystemProxy {
            let result = MihomoApiClient::new(&current)
                .map_err(dto)?
                .wait_ready(Duration::from_secs(2))
                .await;
            if let Err(error) = result {
                let error = dto(AppError::startup(StartupReason::CoreNotReady, error, None));
                feedback.fail(&app, operation, &error);
                return Err(error);
            }
        }
        let result = app
            .state::<session_resume::SessionResumeManager>()
            .while_open(|| {
                platform::proxy_transaction(&app, || {
                    if mode == NetworkMode::SystemProxy {
                        platform::enable_system_proxy(&app, current.mixed_port)?;
                        platform::verify_system_proxy(current.mixed_port)?;
                    } else {
                        platform::restore_system_proxy(&app)?;
                    }
                    let mut next = current.clone();
                    next.network_mode = mode;
                    storage.save_settings(&next)?;
                    Ok(next)
                })
            });
        return match result {
            Ok(settings) => {
                feedback.advance(&app, operation, ConnectionPhase::Enabled);
                feedback.probe(&app, operation, settings.clone());
                Ok(PublicAppSettings::from(&settings))
            }
            Err(error) => {
                let error = dto(error);
                feedback.fail(&app, operation, &error);
                Err(error)
            }
        };
    }
    app.state::<ConnectionFeedback>().invalidate(&app);
    if mode == NetworkMode::Tun {
        let helper = tun_service::status();
        if !helper.ready() {
            return Err(dto(AppError::Platform(helper.message)));
        }
    }
    if mode != NetworkMode::SystemProxy {
        platform::restore_system_proxy(&app).map_err(dto)?;
    }
    let storage = AppStorage::from_app(&app).map_err(dto)?;
    let mut settings = storage.settings().map_err(dto)?;
    settings.network_mode = mode;
    storage.save_settings(&settings).map_err(dto)?;
    Ok(PublicAppSettings::from(&settings))
}

#[tauri::command]
async fn set_profile_routing_mode(
    app: AppHandle,
    state: State<'_, MihomoRuntime>,
    profile_id: Uuid,
    mode: RoutingMode,
) -> Result<ProfileDetails, AppErrorDto> {
    let _configuration = user_rules::acquire_configuration(&app).map_err(dto)?;
    let storage = AppStorage::from_app(&app).map_err(dto)?;
    let old_mode = storage.load_profile(profile_id).map_err(dto)?.routing_mode;
    let details = profile_service::set_routing_mode(&app, profile_id, mode).map_err(dto)?;
    let active = storage.state().map_err(dto)?.active_profile_id == Some(profile_id);
    if active && state.status(Some(&app)).phase == models::RuntimePhase::Running {
        if let Err(error) = api_client(&app)?.set_mode(mode).await {
            let _ = profile_service::set_routing_mode(&app, profile_id, old_mode);
            return Err(dto(error));
        }
    }
    Ok(details)
}

#[tauri::command]
async fn get_proxies(app: AppHandle) -> Result<Value, AppErrorDto> {
    node_selection::proxies(&app).await.map_err(dto)
}

#[tauri::command]
async fn get_current_node_details(
    app: AppHandle,
    group: String,
) -> Result<node_details::CurrentNodeDetails, AppErrorDto> {
    node_details::get_current_node_details(&app, group)
        .await
        .map_err(dto)
}

#[tauri::command]
async fn get_rules(app: AppHandle) -> Result<Value, AppErrorDto> {
    api_client(&app)?.rules().await.map_err(dto)
}

#[tauri::command]
async fn get_connections(app: AppHandle) -> Result<Value, AppErrorDto> {
    api_client(&app)?.connections().await.map_err(dto)
}

#[tauri::command]
async fn select_proxy(
    app: AppHandle,
    group: String,
    proxy: String,
    profile_id: Uuid,
    revision_id: Uuid,
) -> Result<(), AppErrorDto> {
    node_selection::select(&app, &group, Some(&proxy), profile_id, revision_id)
        .await
        .map_err(dto)
}

#[tauri::command]
async fn clear_proxy_selection(
    app: AppHandle,
    group: String,
    profile_id: Uuid,
    revision_id: Uuid,
) -> Result<(), AppErrorDto> {
    node_selection::select(&app, &group, None, profile_id, revision_id)
        .await
        .map_err(dto)
}

#[tauri::command]
fn start_openai_policy_generation(
    app: AppHandle,
    profile_id: Uuid,
    auto_maintain: Option<bool>,
) -> Result<OpenAiPolicyTaskSnapshot, AppErrorDto> {
    openai_policy::start_generation(&app, profile_id, auto_maintain.unwrap_or(true)).map_err(dto)
}

#[tauri::command]
fn get_openai_policy_task(
    manager: State<'_, OpenAiPolicyTaskManager>,
) -> Result<OpenAiPolicyTaskSnapshot, AppErrorDto> {
    manager.snapshot().map_err(dto)
}

#[tauri::command]
fn cancel_openai_policy_generation(
    manager: State<'_, OpenAiPolicyTaskManager>,
) -> Result<OpenAiPolicyTaskSnapshot, AppErrorDto> {
    manager.cancel().map_err(dto)
}

#[tauri::command]
async fn disable_openai_policy(
    app: AppHandle,
    profile_id: Uuid,
) -> Result<models::OpenAiPolicy, AppErrorDto> {
    openai_policy::disable_policy(&app, profile_id)
        .await
        .map_err(dto)
}

#[tauri::command]
async fn test_proxy_delay(
    app: AppHandle,
    proxy: String,
    url: Option<String>,
    timeout_ms: Option<u32>,
) -> Result<Value, AppErrorDto> {
    api_client(&app)?
        .delay(
            &proxy,
            url.as_deref()
                .unwrap_or("https://www.gstatic.com/generate_204"),
            timeout_ms.unwrap_or(5_000),
        )
        .await
        .map_err(dto)
}

#[tauri::command]
async fn test_proxy_group(
    app: AppHandle,
    group: String,
    url: Option<String>,
    expected_status: Option<String>,
    timeout_ms: Option<u32>,
) -> Result<Value, AppErrorDto> {
    api_client(&app)?
        .group_delay(
            &group,
            url.as_deref()
                .unwrap_or("https://www.gstatic.com/generate_204"),
            timeout_ms.unwrap_or(5_000),
            expected_status.as_deref(),
        )
        .await
        .map_err(dto)
}

#[tauri::command]
async fn close_connection(app: AppHandle, connection_id: String) -> Result<(), AppErrorDto> {
    api_client(&app)?
        .close_connection(&connection_id)
        .await
        .map_err(dto)
}

#[tauri::command]
async fn run_connectivity_diagnostics(app: AppHandle) -> Result<Vec<DiagnosticCheck>, AppErrorDto> {
    let settings = AppStorage::from_app(&app)
        .and_then(|storage| storage.settings())
        .map_err(dto)?;
    Ok(diagnostics::run(&settings).await)
}

#[tauri::command]
async fn run_network_safety_check(
    app: AppHandle,
) -> Result<network_safety::NetworkSafetyReport, AppErrorDto> {
    let settings = AppStorage::from_app(&app)
        .and_then(|storage| storage.settings())
        .map_err(dto)?;
    if settings.network_mode == NetworkMode::Tun {
        network_safety::verify_tun_route().await.map_err(dto)
    } else {
        network_safety::verify_local_proxy(&settings)
            .await
            .map_err(dto)
    }
}

fn api_client(app: &AppHandle) -> Result<MihomoApiClient, AppErrorDto> {
    AppStorage::from_app(app)
        .and_then(|storage| storage.settings())
        .and_then(|settings| MihomoApiClient::new(&settings))
        .map_err(dto)
}

fn cleanup_app(app: &AppHandle) {
    let resume = app.state::<session_resume::SessionResumeManager>();
    if !resume.begin_shutdown() {
        return;
    }
    app.state::<ConnectionFeedback>().invalidate(app);
    app_log::record(
        0,
        app_log::Area::App,
        "应用退出：清理本次代理接管，保留最后的运行意图与网络模式",
    );
    app.state::<openai_stability::StabilityManager>().stop();
    let _ = app
        .state::<local_routing::LocalRoutingManager>()
        .shutdown(app);
    app.state::<GlobalTrafficMonitor>().stop();
    let _ = app.state::<OpenAiPolicyTaskManager>().cancel();
    resume.cleanup_transition(|| {
        let proxy = platform::restore_system_proxy(app);
        let stopped = app.state::<MihomoRuntime>().stop(Some(app));
        if let Ok(storage) = AppStorage::from_app(app) {
            // Shutdown/upgrade cleanup is not a user Stop command. Preserve
            // desired_running for the next launch, including clean shutdowns.
            let _ = storage.mark_clean_shutdown(proxy.is_ok() && stopped.is_ok());
        }
    });
}

fn show_home_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
        let _ = app.emit("navigate-view", "overview");
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    #[cfg(windows)]
    if startup::installer_cleanup_requested(&std::env::args().skip(1).collect::<Vec<_>>()) {
        std::process::exit(if startup::remove_owned_login_entries().is_ok() {
            0
        } else {
            1
        });
    }
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            if startup::show_existing_window(&args) {
                show_home_window(app);
            }
        }))
        .plugin(
            tauri_plugin_autostart::Builder::new()
                .app_name("Serylane")
                .arg(startup::AUTOSTART_ARG)
                .build(),
        )
        .manage(MihomoRuntime::default())
        .manage(ConnectionFeedback::default())
        .manage(session_resume::SessionResumeManager::default())
        .manage(app_update::AppUpdateManager::default())
        .manage(program_proxy::ProgramProxyManager::default())
        .manage(local_routing::LocalRoutingManager::default())
        .manage(openai_stability::StabilityManager::default())
        .manage(SubscriptionImportGuard::default())
        .manage(OpenAiPolicyTaskManager::default())
        .manage(GlobalTrafficMonitor::default())
        .manage(user_rules::ConfigurationMutationGuard::default())
        .setup(|app| {
            let storage = AppStorage::from_app(app.handle()).map_err(|error| error.to_string())?;
            let settings = storage.settings().map_err(|error| error.to_string())?;
            app_log::initialize(storage.app_log_path(), settings.app_log_retention_days);
            app_log::record(
                0,
                app_log::Area::App,
                &format!(
                    "Serylane {} 已启动（{} / {}）",
                    env!("CARGO_PKG_VERSION"),
                    std::env::consts::OS,
                    std::env::consts::ARCH
                ),
            );
            if startup::migrate_login_entry(app.handle(), &settings).is_err() {
                app_log::record(
                    1,
                    app_log::Area::Settings,
                    "登录启动项迁移失败；请在设置中重新保存登录启动选项",
                );
            }
            // Unknown legacy values should not prevent startup. Leave the stored
            // value untouched and use the system appearance until it is changed.
            app.set_theme(appearance::native_theme(&settings.theme).unwrap_or(None));
            let persistent = storage.state().map_err(|error| error.to_string())?;
            if !persistent.clean_shutdown {
                let _ = platform::restore_system_proxy(app.handle());
            }
            if tun_service::status().runtime_running {
                let _ = tun_service::stop();
            }
            storage
                .mark_clean_shutdown(false)
                .map_err(|error| error.to_string())?;
            let show_item = MenuItem::with_id(app, "show", "显示主窗口", true, None::<&str>)?;
            let stop_item = MenuItem::with_id(app, "stop", "停止 Mihomo", true, None::<&str>)?;
            let quit_item = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show_item, &stop_item, &quit_item])?;
            let mut tray = TrayIconBuilder::with_id(TRAY_ID).tooltip("Serylane");
            if let Some(icon) = app.default_window_icon() {
                tray = tray.icon(icon.clone());
            }
            tray.menu(&menu)
                .show_menu_on_left_click(false)
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        show_home_window(tray.app_handle());
                    }
                })
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => {
                        show_home_window(app);
                    }
                    "stop" => {
                        let app = app.clone();
                        tauri::async_runtime::spawn(async move {
                            let _ = stop_runtime_by_user(&app).await;
                        });
                    }
                    "quit" => {
                        cleanup_app(app);
                        app.exit(0);
                    }
                    _ => {}
                })
                .build(app)?;
            app.state::<GlobalTrafficMonitor>()
                .start(app.handle().clone(), settings.show_global_traffic);
            let _ = app
                .state::<local_routing::LocalRoutingManager>()
                .bootstrap(app.handle());
            app.state::<openai_stability::StabilityManager>()
                .start(app.handle().clone());
            app.state::<session_resume::SessionResumeManager>()
                .bootstrap(app.handle(), &settings, &persistent);
            // The native window starts hidden to avoid a visible flash at login.
            if startup::show_initial_window(&settings, &std::env::args().collect::<Vec<_>>()) {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                }
            }
            let pruning_app = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                loop {
                    tokio::time::sleep(Duration::from_secs(60)).await;
                    if pruning_app
                        .state::<session_resume::SessionResumeManager>()
                        .is_shutting_down()
                    {
                        break;
                    }
                    app_log::prune();
                }
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            } else if window.label() == "main" && matches!(event, WindowEvent::Destroyed) {
                cleanup_app(window.app_handle());
            }
        })
        .invoke_handler(tauri::generate_handler![
            app_info,
            connection_feedback::connection_feedback,
            connection_feedback::recheck_connection,
            session_resume::get_session_resume_status,
            startup::get_startup_status,
            local_routing::local_route_status,
            local_routing::save_local_route,
            local_routing::set_local_route_enabled,
            local_routing::set_codex_route,
            openai_stability::set_openai_stability,
            app_update::check_app_update,
            app_update::app_update_status,
            app_update::save_update_preferences,
            app_update::download_app_update,
            app_update::cancel_app_update,
            app_update::install_app_update,
            app_update::open_official_release,
            get_settings,
            get_user_rules,
            parse_user_rules_text,
            validate_user_rules,
            save_user_rules,
            rollback_user_rules,
            set_app_theme,
            update_settings,
            global_traffic_snapshot,
            inspect_mihomo_yaml,
            list_profiles,
            list_subscriptions,
            get_profile_details,
            get_active_profile,
            create_inline_profile,
            create_subscription_profile,
            refresh_profile,
            activate_profile,
            rollback_profile,
            delete_profile,
            probe_mihomo,
            runtime_status,
            start_active_profile,
            stop_mihomo,
            runtime_logs,
            application_logs,
            clear_application_logs,
            clear_runtime_logs,
            system_proxy_status,
            check_system_proxy_compatibility,
            program_proxy::list_proxy_programs,
            program_proxy::save_proxy_program,
            program_proxy::delete_proxy_program,
            program_proxy::launch_proxy_program,
            program_proxy::choose_proxy_program,
            program_proxy::list_installed_proxy_applications,
            tun_helper_status,
            install_tun_helper,
            repair_tun_helper,
            uninstall_tun_helper,
            open_tun_helper_settings,
            prepare_tun_active_profile,
            set_network_mode,
            set_profile_routing_mode,
            get_proxies,
            get_current_node_details,
            get_rules,
            get_connections,
            select_proxy,
            clear_proxy_selection,
            test_proxy_delay,
            test_proxy_group,
            start_openai_policy_generation,
            get_openai_policy_task,
            cancel_openai_policy_generation,
            disable_openai_policy,
            openai_cost::get_openai_costs,
            openai_cost::save_openai_costs,
            close_connection,
            run_connectivity_diagnostics,
            run_network_safety_check,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            // Tao maps confirmed Windows WM_ENDSESSION to LoopDestroyed,
            // which reaches Tauri as Exit (not necessarily Window::Destroyed).
            if matches!(event, tauri::RunEvent::Exit) {
                cleanup_app(app);
            }
        });
}

#[cfg(test)]
mod startup_transition_tests {
    use super::*;
    #[test]
    fn only_manual_system_proxy_changes_keep_the_core_alive() {
        for from in [
            NetworkMode::Manual,
            NetworkMode::SystemProxy,
            NetworkMode::Tun,
        ] {
            for to in [
                NetworkMode::Manual,
                NetworkMode::SystemProxy,
                NetworkMode::Tun,
            ] {
                assert_eq!(
                    can_switch_without_restart(from, to),
                    from != NetworkMode::Tun && to != NetworkMode::Tun
                );
            }
        }
    }
}
