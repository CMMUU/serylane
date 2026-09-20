use crate::config::is_subscription_metadata_node_name;
use crate::effective::{build_effective_config, build_effective_config_with_policy};
use crate::error::{AppError, AppResult};
use crate::mihomo_api::MihomoApiClient;
use crate::models::{
    AppSettings, NetworkMode, OpenAiNodeScore, OpenAiPolicy, RoutingMode, ValidationReport,
};
use crate::runtime;
use crate::storage::AppStorage;
use chrono::{DateTime, Utc};
use futures_util::{stream, StreamExt};
use serde::Serialize;
use serde_yaml::{Mapping, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Manager};
use uuid::Uuid;

const OPENAI_HEALTH_URL: &str = "https://api.openai.com/v1/models";
const OPENAI_EXPECTED_STATUS: &str = "401";
const BANDWIDTH_URL: &str = "https://speed.cloudflare.com/__down";
const BANDWIDTH_BYTES: usize = 2 * 1024 * 1024;
const BENCHMARK_GROUP_NAME: &str = "__MIHOMO_DESKTOP_OPENAI_BENCHMARK__";
const MAX_CANDIDATES: usize = 300;
const BANDWIDTH_CANDIDATES: usize = 15;
const SELECTED_NODES: usize = 10;
const MIN_HEALTHY_NODES: usize = 2;

#[derive(Debug, Clone, Copy, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OpenAiTaskPhase {
    #[default]
    Idle,
    Preparing,
    Checking,
    Bandwidth,
    Applying,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenAiPolicyTaskSnapshot {
    pub running: bool,
    pub profile_id: Option<Uuid>,
    pub phase: OpenAiTaskPhase,
    pub completed: usize,
    pub total: usize,
    pub message: String,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub error: Option<String>,
    pub result: Option<OpenAiPolicy>,
}

impl Default for OpenAiPolicyTaskSnapshot {
    fn default() -> Self {
        Self {
            running: false,
            profile_id: None,
            phase: OpenAiTaskPhase::Idle,
            completed: 0,
            total: 0,
            message: "尚未运行 OpenAI 节点筛选".to_string(),
            started_at: None,
            finished_at: None,
            error: None,
            result: None,
        }
    }
}

pub struct OpenAiPolicyTaskManager {
    snapshot: Mutex<OpenAiPolicyTaskSnapshot>,
    running: AtomicBool,
    cancelled: AtomicBool,
}

impl Default for OpenAiPolicyTaskManager {
    fn default() -> Self {
        Self {
            snapshot: Mutex::new(OpenAiPolicyTaskSnapshot::default()),
            running: AtomicBool::new(false),
            cancelled: AtomicBool::new(false),
        }
    }
}

impl OpenAiPolicyTaskManager {
    pub fn snapshot(&self) -> AppResult<OpenAiPolicyTaskSnapshot> {
        self.snapshot
            .lock()
            .map(|snapshot| snapshot.clone())
            .map_err(|_| AppError::Runtime("OpenAI 任务状态锁已损坏".to_string()))
    }

    pub fn cancel(&self) -> AppResult<OpenAiPolicyTaskSnapshot> {
        if self.running.load(Ordering::Acquire) {
            self.cancelled.store(true, Ordering::Release);
            let phase = self
                .snapshot
                .lock()
                .map(|snapshot| snapshot.phase)
                .map_err(|_| AppError::Runtime("OpenAI 任务状态锁已损坏".to_string()))?;
            self.update(phase, None, None, "正在停止 OpenAI 节点筛选…".to_string())?;
        }
        self.snapshot()
    }

    fn begin(&self, profile_id: Uuid) -> AppResult<OpenAiPolicyTaskSnapshot> {
        self.running
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| AppError::Conflict("已有 OpenAI 节点筛选任务正在运行".to_string()))?;
        self.cancelled.store(false, Ordering::Release);
        let snapshot = OpenAiPolicyTaskSnapshot {
            running: true,
            profile_id: Some(profile_id),
            phase: OpenAiTaskPhase::Preparing,
            completed: 0,
            total: 0,
            message: "正在准备独立 Mihomo 检测环境".to_string(),
            started_at: Some(Utc::now()),
            finished_at: None,
            error: None,
            result: None,
        };
        *self
            .snapshot
            .lock()
            .map_err(|_| AppError::Runtime("OpenAI 任务状态锁已损坏".to_string()))? =
            snapshot.clone();
        Ok(snapshot)
    }

    fn update(
        &self,
        phase: OpenAiTaskPhase,
        completed: Option<usize>,
        total: Option<usize>,
        message: String,
    ) -> AppResult<()> {
        let mut snapshot = self
            .snapshot
            .lock()
            .map_err(|_| AppError::Runtime("OpenAI 任务状态锁已损坏".to_string()))?;
        snapshot.phase = phase;
        if let Some(completed) = completed {
            snapshot.completed = completed;
        }
        if let Some(total) = total {
            snapshot.total = total;
        }
        snapshot.message = message;
        Ok(())
    }

    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    fn finish_success(&self, policy: OpenAiPolicy) -> AppResult<()> {
        let selected = policy.selected_nodes.len();
        let healthy = policy.healthy_count;
        {
            let mut snapshot = self
                .snapshot
                .lock()
                .map_err(|_| AppError::Runtime("OpenAI 任务状态锁已损坏".to_string()))?;
            snapshot.running = false;
            snapshot.phase = OpenAiTaskPhase::Completed;
            snapshot.completed = selected;
            snapshot.total = selected;
            snapshot.message = format!("已生成 {selected} 个节点，{healthy} 个候选通过检测");
            snapshot.finished_at = Some(Utc::now());
            snapshot.error = None;
            snapshot.result = Some(policy);
        }
        self.running.store(false, Ordering::Release);
        self.cancelled.store(false, Ordering::Release);
        Ok(())
    }

    fn finish_error(&self, error: String, cancelled: bool) -> AppResult<()> {
        {
            let mut snapshot = self
                .snapshot
                .lock()
                .map_err(|_| AppError::Runtime("OpenAI 任务状态锁已损坏".to_string()))?;
            snapshot.running = false;
            snapshot.phase = if cancelled {
                OpenAiTaskPhase::Cancelled
            } else {
                OpenAiTaskPhase::Failed
            };
            snapshot.message = if cancelled {
                "OpenAI 节点筛选已停止".to_string()
            } else {
                "OpenAI 节点筛选失败".to_string()
            };
            snapshot.finished_at = Some(Utc::now());
            snapshot.error = if cancelled { None } else { Some(error) };
            snapshot.result = None;
        }
        self.running.store(false, Ordering::Release);
        self.cancelled.store(false, Ordering::Release);
        Ok(())
    }
}

#[derive(Debug, Clone)]
struct CandidateNode {
    name: String,
    server_group: String,
}

#[derive(Debug, Clone)]
struct ReachableNode {
    candidate: CandidateNode,
    first_delay_ms: u32,
}

#[derive(Debug, Clone)]
struct BandwidthListener {
    port: u16,
    proxy: String,
}

pub fn start_generation(
    app: &AppHandle,
    profile_id: Uuid,
    auto_maintain: bool,
) -> AppResult<OpenAiPolicyTaskSnapshot> {
    let storage = AppStorage::from_app(app)?;
    let profile = storage.load_profile(profile_id)?;
    if profile.active_revision_id.is_none() {
        return Err(AppError::NotFound("配置没有活动版本".to_string()));
    }

    let manager = app.state::<OpenAiPolicyTaskManager>();
    let snapshot = manager.begin(profile_id)?;
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let result = generate_and_apply(&app, profile_id, auto_maintain).await;
        let manager = app.state::<OpenAiPolicyTaskManager>();
        match result {
            Ok(policy) => {
                let _ = manager.finish_success(policy);
            }
            Err(error) => {
                let cancelled = manager.is_cancelled() || error.to_string().contains("任务已取消");
                let _ = manager.finish_error(runtime::redact(&error.to_string()), cancelled);
            }
        }
    });
    Ok(snapshot)
}

pub async fn disable_policy(app: &AppHandle, profile_id: Uuid) -> AppResult<OpenAiPolicy> {
    let storage = AppStorage::from_app(app)?;
    let mut policy = storage.load_profile(profile_id)?.openai_policy;
    policy.enabled = false;
    policy.auto_maintain = false;
    apply_policy_revision(app, profile_id, &policy).await?;
    Ok(policy)
}

async fn generate_and_apply(
    app: &AppHandle,
    profile_id: Uuid,
    auto_maintain: bool,
) -> AppResult<OpenAiPolicy> {
    update_progress(
        app,
        OpenAiTaskPhase::Preparing,
        0,
        0,
        "正在读取订阅并提取节点".to_string(),
    )?;
    let storage = AppStorage::from_app(app)?;
    let profile = storage.load_profile(profile_id)?;
    let revision_id = profile
        .active_revision_id
        .ok_or_else(|| AppError::NotFound("配置没有活动版本".to_string()))?;
    let source = storage.load_revision_source(profile_id, revision_id)?;
    let cost_preferences = storage.openai_costs(profile_id)?;
    let costs = cost_preferences.resolve(&source)?;
    let candidates: Vec<_> = extract_candidates(&source)?
        .into_iter()
        .filter(|node| costs.allowed(&node.name) && crate::node_metadata::eligible(&node.name))
        .take(MAX_CANDIDATES)
        .collect();
    if candidates.len() < MIN_HEALTHY_NODES {
        return Err(AppError::Config(
            if costs.value_mode() {
                "符合地区及成本策略的显式节点少于 2 个；请检查名称地区、倍率与上限"
            } else {
                "支持地区的显式节点少于 2 个；未知、多地区冲突或受限制地区不进入 OpenAI 灾备"
            }
            .to_string(),
        ));
    }
    check_cancelled(app)?;

    let excluded = extract_candidates(&source)?
        .iter()
        .filter(|n| !crate::node_metadata::eligible(&n.name))
        .count();
    update_progress(
        app,
        OpenAiTaskPhase::Preparing,
        0,
        candidates.len(),
        format!("地区筛选排除 {excluded} 个节点；继续按质量和预算筛选"),
    )?;
    let mut policy = benchmark_nodes(app, &source, candidates, auto_maintain, &costs).await?;
    if !costs.value_mode() && profile.openai_policy.last_benchmarked_at.is_some() {
        policy.stability_enabled = profile.openai_policy.stability_enabled;
    }
    check_cancelled(app)?;
    update_progress(
        app,
        OpenAiTaskPhase::Applying,
        0,
        1,
        "正在生成配置版本并执行 Mihomo 原生校验".to_string(),
    )?;
    apply_policy_revision_checked(
        app,
        profile_id,
        &policy,
        Some((revision_id, cost_preferences.revision)),
    )
    .await?;
    update_progress(
        app,
        OpenAiTaskPhase::Applying,
        1,
        1,
        "OpenAI 自动灾备配置已经生成".to_string(),
    )?;
    Ok(policy)
}

pub(crate) async fn apply_policy_revision(
    app: &AppHandle,
    profile_id: Uuid,
    policy: &OpenAiPolicy,
) -> AppResult<()> {
    apply_policy_revision_checked(app, profile_id, policy, None).await
}

async fn apply_policy_revision_checked(
    app: &AppHandle,
    profile_id: Uuid,
    policy: &OpenAiPolicy,
    expected: Option<(Uuid, u64)>,
) -> AppResult<()> {
    let permit = crate::user_rules::acquire_configuration(app)?;
    let storage = AppStorage::from_app(app)?;
    let profile = storage.load_profile(profile_id)?;
    let previous_revision_id = profile
        .active_revision_id
        .ok_or_else(|| AppError::NotFound("配置没有活动版本".to_string()))?;
    if let Some((revision, costs)) = expected {
        if previous_revision_id != revision || storage.openai_costs(profile_id)?.revision != costs {
            return Err(AppError::Conflict(
                "筛选期间配置或成本策略已变化，请重新生成".into(),
            ));
        }
    }
    // Recheck under the configuration permit: a concurrent cost save must not
    // leave value mode backed by a core-managed Fallback without budget checks.
    if policy.enabled
        && !policy.stability_enabled
        && storage.openai_costs(profile_id)?.mode == crate::openai_cost::CostMode::Value
    {
        return Err(AppError::Conflict(
            "性价比策略需要稳定优先；请先将成本策略改为质量优先".into(),
        ));
    }
    let previous_revision = storage.load_revision(profile_id, previous_revision_id)?;
    let source = storage.load_revision_source(profile_id, previous_revision_id)?;
    let settings = storage.settings()?;
    let effective =
        build_effective_config_with_policy(&source, &settings, profile.routing_mode, Some(policy))?;
    let validation = ValidationReport {
        valid: true,
        warnings: effective.summary.warnings.clone(),
        errors: Vec::new(),
        native_core_validated: true,
    };
    let applies_to_runtime = storage.state()?.active_profile_id == Some(profile_id);
    if applies_to_runtime {
        crate::user_rules::apply_profile_config(
            app,
            &storage,
            &effective.yaml,
            || {
                let revision = storage.save_revision(
                    profile_id,
                    &source,
                    &effective.yaml,
                    previous_revision.subscription.clone(),
                    validation,
                    policy.clone(),
                )?;
                crate::profile_service::commit_active_selection(
                    &storage,
                    profile_id,
                    revision.id,
                    &effective.yaml,
                )?;
                Ok(())
            },
            &permit,
        )
        .await?;
    } else {
        crate::user_rules::validate_config(app, &effective.yaml).await?;
        let revision = storage.save_revision(
            profile_id,
            &source,
            &effective.yaml,
            previous_revision.subscription,
            validation,
            policy.clone(),
        )?;
        if let Err(error) = storage.update_profile_revision(profile_id, revision.id) {
            storage.save_profile(&profile)?;
            return Err(error);
        }
    }
    Ok(())
}

async fn benchmark_nodes(
    app: &AppHandle,
    source: &str,
    candidates: Vec<CandidateNode>,
    auto_maintain: bool,
    costs: &crate::openai_cost::ResolvedCosts,
) -> AppResult<OpenAiPolicy> {
    let candidate_count = candidates.len();
    update_progress(
        app,
        OpenAiTaskPhase::Preparing,
        0,
        candidate_count,
        format!("正在启动独立检测核心，共 {candidate_count} 个候选节点"),
    )?;
    let core = BenchmarkCore::start(app, source, &candidates).await?;
    check_cancelled(app)?;

    let all_candidates = candidates.clone();
    let api = core.api.clone();
    let mut checks = stream::iter(candidates.into_iter().map(|candidate| {
        let api = api.clone();
        async move {
            let result = api
                .delay_expected(
                    &candidate.name,
                    OPENAI_HEALTH_URL,
                    8_000,
                    Some(OPENAI_EXPECTED_STATUS),
                )
                .await;
            (candidate, result)
        }
    }))
    .buffer_unordered(8);
    let mut reachable = Vec::new();
    let mut completed = 0usize;
    while let Some((candidate, result)) = checks.next().await {
        check_cancelled(app)?;
        completed += 1;
        if let Ok(payload) = result {
            if let Some(delay) = payload
                .get("delay")
                .and_then(serde_json::Value::as_u64)
                .and_then(|delay| u32::try_from(delay).ok())
            {
                reachable.push(ReachableNode {
                    candidate,
                    first_delay_ms: delay,
                });
            }
        }
        update_progress(
            app,
            OpenAiTaskPhase::Checking,
            completed,
            candidate_count,
            format!(
                "OpenAI 可达性检测 {completed}/{candidate_count}，已通过 {} 个",
                reachable.len()
            ),
        )?;
    }

    if reachable.len() < MIN_HEALTHY_NODES {
        return Err(AppError::Runtime(format!(
            "仅 {} 个节点通过 OpenAI 可达性检测，至少需要 2 个",
            reachable.len()
        )));
    }
    let healthy_count = reachable.len();
    reachable.sort_by_key(|node| node.first_delay_ms);
    if !costs.value_mode() {
        reachable.truncate(BANDWIDTH_CANDIDATES);
    }

    let bandwidth_total = reachable.len();
    update_progress(
        app,
        OpenAiTaskPhase::Bandwidth,
        0,
        bandwidth_total,
        if costs.value_mode() {
            format!("省流检测：复测 {bandwidth_total} 个节点延迟，不下载带宽测试文件")
        } else {
            format!("正在创建 {bandwidth_total} 个隔离带宽测试入口")
        },
    )?;
    let listener_ports: Vec<Option<u16>> = if costs.value_mode() {
        vec![None; reachable.len()]
    } else {
        core.configure_bandwidth_listeners(app, source, &all_candidates, &reachable)
            .await?
            .into_iter()
            .map(Some)
            .collect()
    };
    let mut ranked = Vec::with_capacity(bandwidth_total);
    let api = core.api.clone();
    let mut bandwidth_checks = stream::iter(reachable.into_iter().zip(listener_ports).map(
        |(node, port)| {
            let api = api.clone();
            async move {
                let (delay_result, bandwidth_result) = tokio::join!(
                    api.delay_expected(
                        &node.candidate.name,
                        OPENAI_HEALTH_URL,
                        8_000,
                        Some(OPENAI_EXPECTED_STATUS),
                    ),
                    async {
                        match port {
                            Some(port) => measure_bandwidth(port).await.ok(),
                            None => None,
                        }
                    },
                );
                (node, delay_result, bandwidth_result)
            }
        },
    ))
    .buffer_unordered(3);
    let mut bandwidth_completed = 0usize;
    while let Some((node, delay_result, bandwidth_result)) = bandwidth_checks.next().await {
        check_cancelled(app)?;
        update_progress(
            app,
            OpenAiTaskPhase::Bandwidth,
            bandwidth_completed,
            bandwidth_total,
            format!(
                "正在评估候选节点质量 {}/{}",
                bandwidth_completed + 1,
                bandwidth_total
            ),
        )?;
        let second_delay = delay_result
            .ok()
            .and_then(|payload| payload.get("delay").and_then(serde_json::Value::as_u64))
            .and_then(|delay| u32::try_from(delay).ok())
            .filter(|delay| *delay > 0);
        if costs.value_mode() && second_delay.is_none() {
            bandwidth_completed += 1;
            continue;
        }
        let second_delay = second_delay.unwrap_or(node.first_delay_ms.saturating_add(1_000));
        let bandwidth_mbps = bandwidth_result;
        let latency_ms = (node.first_delay_ms.saturating_add(second_delay)) / 2;
        let jitter_ms = node.first_delay_ms.abs_diff(second_delay);
        let score = if costs.value_mode() {
            node_score(latency_ms, jitter_ms, None) / 0.65
        } else {
            node_score(latency_ms, jitter_ms, bandwidth_mbps)
        };
        let candidate = node.candidate;
        let name = candidate.name.clone();
        ranked.push((
            candidate,
            OpenAiNodeScore {
                name,
                latency_ms,
                jitter_ms,
                bandwidth_mbps: bandwidth_mbps.map(|value| round(value, 2)),
                score: round(score, 1),
                checked_at: Utc::now(),
            },
        ));
        bandwidth_completed += 1;
        update_progress(
            app,
            OpenAiTaskPhase::Bandwidth,
            bandwidth_completed,
            bandwidth_total,
            format!(
                "质量评估 {}/{}，准备选出最多 10 个节点",
                bandwidth_completed, bandwidth_total
            ),
        )?;
    }

    if costs.value_mode() {
        let best = ranked.iter().map(|(_, n)| n.score).fold(0.0_f64, f64::max);
        ranked.retain(|(_, n)| n.score >= (best - 15.0).max(50.0));
    }
    ranked.sort_by(|left, right| {
        costs
            .utility(&right.1.name, right.1.score)
            .total_cmp(&costs.utility(&left.1.name, left.1.score))
            .then_with(|| left.1.latency_ms.cmp(&right.1.latency_ms))
    });
    let selected_nodes = select_diverse_nodes(ranked, SELECTED_NODES);
    if selected_nodes.len() < MIN_HEALTHY_NODES {
        return Err(AppError::Runtime("综合检测后少于 2 个可用节点".to_string()));
    }
    Ok(OpenAiPolicy {
        stability_enabled: true,
        enabled: true,
        auto_maintain,
        max_nodes: SELECTED_NODES as u8,
        selected_nodes,
        candidate_count,
        healthy_count,
        last_benchmarked_at: Some(Utc::now()),
        benchmark_version: 1,
    })
}

struct BenchmarkCore {
    child: Child,
    directory: PathBuf,
    settings: AppSettings,
    api: MihomoApiClient,
}

impl BenchmarkCore {
    async fn start(app: &AppHandle, source: &str, candidates: &[CandidateNode]) -> AppResult<Self> {
        let mixed_reservation = TcpListener::bind(("127.0.0.1", 0))?;
        let controller_reservation = TcpListener::bind(("127.0.0.1", 0))?;
        let mixed_port = mixed_reservation.local_addr()?.port();
        let controller_port = controller_reservation.local_addr()?.port();
        let settings = AppSettings {
            network_mode: NetworkMode::Manual,
            mixed_port,
            controller_port,
            ..Default::default()
        };
        let config = build_benchmark_config(source, &settings, candidates)?;

        let app_data = app
            .path()
            .app_data_dir()
            .map_err(|error| AppError::Io(error.to_string()))?;
        let directory = app_data
            .join("benchmarks")
            .join(format!("openai-{}", Uuid::new_v4()));
        fs::create_dir_all(&directory)?;
        runtime::set_private_directory_permissions(&directory)?;
        copy_runtime_assets(&app_data.join("runtime"), &directory)?;
        let config_path = directory.join("benchmark.yaml");
        runtime::write_private_file(&config_path, config.as_bytes())?;
        let binary = runtime::resolve_binary(Some(app))
            .ok_or_else(|| AppError::Runtime("未找到 Mihomo sidecar".to_string()))?;
        runtime::validate_file(&binary, &directory, &config_path)?;
        drop(mixed_reservation);
        drop(controller_reservation);

        let mut command = Command::new(binary);
        command
            .arg("-d")
            .arg(&directory)
            .arg("-f")
            .arg(&config_path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let child = spawn_benchmark_process(&mut command)
            .map_err(|error| AppError::Runtime(error.to_string()))?;
        let api =
            MihomoApiClient::from_endpoint(controller_port, settings.controller_secret.clone())?;
        let mut core = Self {
            child,
            directory,
            settings,
            api,
        };
        if let Err(error) = core.api.wait_ready(Duration::from_secs(20)).await {
            let _ = core.child.kill();
            let _ = core.child.wait();
            return Err(error);
        }
        Ok(core)
    }

    async fn configure_bandwidth_listeners(
        &self,
        app: &AppHandle,
        source: &str,
        candidates: &[CandidateNode],
        reachable: &[ReachableNode],
    ) -> AppResult<Vec<u16>> {
        let mut reservations = Vec::with_capacity(reachable.len());
        let mut listeners = Vec::with_capacity(reachable.len());
        for node in reachable {
            let reservation = TcpListener::bind(("127.0.0.1", 0))?;
            let port = reservation.local_addr()?.port();
            listeners.push(BandwidthListener {
                port,
                proxy: node.candidate.name.clone(),
            });
            reservations.push(reservation);
        }
        let config =
            build_benchmark_config_with_listeners(source, &self.settings, candidates, &listeners)?;
        let config_path = self.directory.join("bandwidth.yaml");
        runtime::write_private_file(&config_path, config.as_bytes())?;
        let binary = runtime::resolve_binary(Some(app))
            .ok_or_else(|| AppError::Runtime("未找到 Mihomo sidecar".to_string()))?;
        runtime::validate_file(&binary, &self.directory, &config_path)?;
        drop(reservations);
        self.api.reload_config(&config).await?;
        tokio::time::sleep(Duration::from_millis(250)).await;
        Ok(listeners
            .into_iter()
            .map(|listener| listener.port)
            .collect())
    }
}

impl Drop for BenchmarkCore {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_dir_all(&self.directory);
    }
}

fn spawn_benchmark_process(command: &mut Command) -> std::io::Result<Child> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // Redirecting stdio alone does not prevent Windows from creating a
        // console for the isolated OpenAI benchmark core.
        command.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
    }
    command.spawn()
}

fn build_benchmark_config(
    source: &str,
    settings: &AppSettings,
    candidates: &[CandidateNode],
) -> AppResult<String> {
    build_benchmark_config_with_listeners(source, settings, candidates, &[])
}

fn build_benchmark_config_with_listeners(
    source: &str,
    settings: &AppSettings,
    candidates: &[CandidateNode],
    listeners: &[BandwidthListener],
) -> AppResult<String> {
    // This isolated core measures candidate nodes rather than user traffic.
    // A rule targeting a managed group must not break policy regeneration before
    // that group exists; the main runtime still receives the persisted overlay.
    let mut benchmark_settings = settings.clone();
    benchmark_settings.user_rules.clear();
    let effective = build_effective_config(source, &benchmark_settings, RoutingMode::Rule)?;
    let mut document: Value = serde_yaml::from_str(&effective.yaml)
        .map_err(|error| AppError::Config(error.to_string()))?;
    let root = document
        .as_mapping_mut()
        .ok_or_else(|| AppError::Config("配置根节点必须是对象".to_string()))?;
    insert(root, "log-level", Value::String("warning".to_string()));
    root.remove(Value::String("rule-providers".to_string()));
    root.remove(Value::String("sub-rules".to_string()));

    let groups = root
        .entry(Value::String("proxy-groups".to_string()))
        .or_insert_with(|| Value::Sequence(Vec::new()))
        .as_sequence_mut()
        .ok_or_else(|| AppError::Config("proxy-groups 必须是数组".to_string()))?;
    groups.retain(|group| {
        group
            .as_mapping()
            .and_then(|mapping| mapping.get(Value::String("name".to_string())))
            .and_then(Value::as_str)
            != Some(BENCHMARK_GROUP_NAME)
    });
    let mut benchmark_group = Mapping::new();
    insert(
        &mut benchmark_group,
        "name",
        Value::String(BENCHMARK_GROUP_NAME.to_string()),
    );
    insert(
        &mut benchmark_group,
        "type",
        Value::String("select".to_string()),
    );
    insert(
        &mut benchmark_group,
        "proxies",
        Value::Sequence(
            candidates
                .iter()
                .map(|candidate| Value::String(candidate.name.clone()))
                .collect(),
        ),
    );
    groups.insert(0, Value::Mapping(benchmark_group));
    insert(
        root,
        "rules",
        Value::Sequence(vec![Value::String(format!("MATCH,{BENCHMARK_GROUP_NAME}"))]),
    );
    if !listeners.is_empty() {
        let listeners = listeners
            .iter()
            .enumerate()
            .map(|(index, listener)| {
                let mut mapping = Mapping::new();
                insert(
                    &mut mapping,
                    "name",
                    Value::String(format!("openai-bandwidth-{index}")),
                );
                insert(&mut mapping, "type", Value::String("mixed".to_string()));
                insert(
                    &mut mapping,
                    "listen",
                    Value::String("127.0.0.1".to_string()),
                );
                insert(&mut mapping, "port", Value::Number(listener.port.into()));
                insert(&mut mapping, "proxy", Value::String(listener.proxy.clone()));
                insert(&mut mapping, "udp", Value::Bool(false));
                Value::Mapping(mapping)
            })
            .collect();
        insert(root, "listeners", Value::Sequence(listeners));
    }
    serde_yaml::to_string(&document).map_err(|error| AppError::Config(error.to_string()))
}

pub(crate) fn candidate_names(source: &str) -> AppResult<Vec<String>> {
    Ok(extract_candidates(source)?
        .into_iter()
        .map(|node| node.name)
        .collect())
}

fn extract_candidates(source: &str) -> AppResult<Vec<CandidateNode>> {
    let document: Value =
        serde_yaml::from_str(source).map_err(|error| AppError::Config(error.to_string()))?;
    let root = document
        .as_mapping()
        .ok_or_else(|| AppError::Config("配置根节点必须是对象".to_string()))?;
    let proxies = root
        .get(Value::String("proxies".to_string()))
        .and_then(Value::as_sequence)
        .ok_or_else(|| AppError::Config("订阅中没有显式 proxies 节点".to_string()))?;
    let mut seen = BTreeSet::new();
    let mut candidates = Vec::new();
    for proxy in proxies {
        let Some(mapping) = proxy.as_mapping() else {
            continue;
        };
        let Some(name) = string_field(mapping, "name") else {
            continue;
        };
        if name.trim().is_empty()
            || matches!(
                name,
                "DIRECT" | "REJECT" | "REJECT-DROP" | "PASS" | "COMPATIBLE"
            )
            || is_subscription_metadata_node_name(name)
            || !seen.insert(name.to_string())
        {
            continue;
        }
        let server_group = string_field(mapping, "server")
            .unwrap_or(name)
            .trim()
            .to_ascii_lowercase();
        candidates.push(CandidateNode {
            name: name.to_string(),
            server_group,
        });
    }
    Ok(candidates)
}

async fn measure_bandwidth(mixed_port: u16) -> AppResult<f64> {
    let proxy = reqwest::Proxy::all(format!("http://127.0.0.1:{mixed_port}"))
        .map_err(|error| AppError::Runtime(error.to_string()))?;
    let client = reqwest::Client::builder()
        .proxy(proxy)
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|error| AppError::Runtime(error.to_string()))?;
    let started = std::time::Instant::now();
    let response = client
        .get(BANDWIDTH_URL)
        .query(&[
            ("bytes", BANDWIDTH_BYTES.to_string()),
            ("nonce", Uuid::new_v4().to_string()),
        ])
        .header(reqwest::header::ACCEPT_ENCODING, "identity")
        .header(reqwest::header::CONNECTION, "close")
        .send()
        .await
        .map_err(|error| AppError::Runtime(error.to_string()))?;
    if !response.status().is_success() {
        return Err(AppError::Runtime(format!(
            "带宽测试 HTTP {}",
            response.status().as_u16()
        )));
    }
    let mut downloaded = 0usize;
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| AppError::Runtime(error.to_string()))?;
        downloaded = downloaded.saturating_add(chunk.len());
        if downloaded > BANDWIDTH_BYTES + 64 * 1024 {
            break;
        }
    }
    if downloaded < 256 * 1024 {
        return Err(AppError::Runtime("带宽测试返回数据不足".to_string()));
    }
    let seconds = started.elapsed().as_secs_f64().max(0.001);
    Ok(downloaded as f64 * 8.0 / seconds / 1_000_000.0)
}

fn node_score(latency_ms: u32, jitter_ms: u32, bandwidth_mbps: Option<f64>) -> f64 {
    let latency = 40.0 * (1.0 - (f64::from(latency_ms) / 3_000.0).clamp(0.0, 1.0));
    let jitter = 15.0 * (1.0 - (f64::from(jitter_ms) / 1_500.0).clamp(0.0, 1.0));
    let bandwidth = 35.0 * (bandwidth_mbps.unwrap_or(0.0) / 200.0).clamp(0.0, 1.0);
    (10.0 + latency + jitter + bandwidth).clamp(0.0, 100.0)
}

fn select_diverse_nodes(
    ranked: Vec<(CandidateNode, OpenAiNodeScore)>,
    limit: usize,
) -> Vec<OpenAiNodeScore> {
    let mut selected = Vec::new();
    let mut selected_names = BTreeSet::new();
    let mut server_counts: BTreeMap<String, usize> = BTreeMap::new();
    for max_per_server in [2usize, usize::MAX] {
        for (candidate, score) in &ranked {
            if selected.len() >= limit {
                return selected;
            }
            if selected_names.contains(&score.name) {
                continue;
            }
            let count = server_counts
                .get(&candidate.server_group)
                .copied()
                .unwrap_or_default();
            if count >= max_per_server {
                continue;
            }
            selected_names.insert(score.name.clone());
            *server_counts
                .entry(candidate.server_group.clone())
                .or_default() += 1;
            selected.push(score.clone());
        }
    }
    selected
}

fn update_progress(
    app: &AppHandle,
    phase: OpenAiTaskPhase,
    completed: usize,
    total: usize,
    message: String,
) -> AppResult<()> {
    app.state::<OpenAiPolicyTaskManager>()
        .update(phase, Some(completed), Some(total), message)
}

fn check_cancelled(app: &AppHandle) -> AppResult<()> {
    if app.state::<OpenAiPolicyTaskManager>().is_cancelled() {
        Err(AppError::Conflict("OpenAI 节点筛选任务已取消".to_string()))
    } else {
        Ok(())
    }
}

fn copy_runtime_assets(source: &Path, destination: &Path) -> AppResult<()> {
    for name in ["GeoSite.dat", "geoip.metadb", "GeoIP.dat", "Country.mmdb"] {
        let from = source.join(name);
        if from.is_file() {
            fs::copy(&from, destination.join(name))?;
        }
    }
    Ok(())
}

fn string_field<'a>(mapping: &'a Mapping, key: &str) -> Option<&'a str> {
    mapping
        .get(Value::String(key.to_string()))
        .and_then(Value::as_str)
}

fn insert(mapping: &mut Mapping, key: &str, value: Value) {
    mapping.insert(Value::String(key.to_string()), value);
}

fn round(value: f64, digits: i32) -> f64 {
    let factor = 10f64.powi(digits);
    (value * factor).round() / factor
}

#[cfg(test)]
mod tests {
    use super::{
        build_benchmark_config, build_benchmark_config_with_listeners, extract_candidates,
        node_score, select_diverse_nodes, BandwidthListener, CandidateNode, BENCHMARK_GROUP_NAME,
    };
    use crate::models::{AppSettings, OpenAiNodeScore};
    use chrono::Utc;
    use serde_yaml::Value;

    const SOURCE: &str = r#"
proxies:
  - name: node-a
    type: socks5
    server: a.example.com
    port: 1080
  - name: node-b
    type: socks5
    server: b.example.com
    port: 1081
  - name: 剩余流量：100 GB
    type: socks5
    server: status.example.com
    port: 1082
proxy-groups:
  - name: PROXY
    type: select
    proxies: [node-a, node-b]
rules:
  - MATCH,PROXY
"#;

    #[cfg(windows)]
    #[test]
    fn benchmark_window_child_fixture() {
        if std::env::var("ROUTEDECK_BENCHMARK_WINDOW_FIXTURE").as_deref() != Ok("1") {
            return;
        }
        use windows_sys::Win32::System::Console::GetConsoleWindow;
        // CREATE_NO_WINDOW can retain console code-page state. What matters is
        // that there is no window; this query does not interact with the desktop.
        // SAFETY: this function only queries the calling process's console.
        unsafe {
            assert!(
                GetConsoleWindow().is_null(),
                "benchmark created a console window"
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn benchmark_process_starts_without_a_console_window() {
        use std::io::Read;
        use std::process::{Child, Command, Stdio};
        use std::time::{Duration, Instant};

        struct FixtureChild(Child);
        impl Drop for FixtureChild {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }

        let mut command = Command::new(std::env::current_exe().expect("test executable"));
        command
            .args([
                "--exact",
                "openai_policy::tests::benchmark_window_child_fixture",
                "--nocapture",
            ])
            .env("ROUTEDECK_BENCHMARK_WINDOW_FIXTURE", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        let mut child = FixtureChild(
            super::spawn_benchmark_process(&mut command).expect("hidden benchmark fixture"),
        );
        let started = Instant::now();
        loop {
            if let Some(status) = child.0.try_wait().expect("fixture status") {
                let mut diagnostic = String::new();
                child
                    .0
                    .stderr
                    .take()
                    .expect("fixture stderr")
                    .take(4096)
                    .read_to_string(&mut diagnostic)
                    .expect("fixture diagnostic");
                assert!(
                    status.success(),
                    "benchmark console checks failed: {status}\n{diagnostic}"
                );
                break;
            }
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "fixture timed out"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn extracts_explicit_unique_candidates() {
        let candidates = extract_candidates(SOURCE).expect("candidates");
        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0].name, "node-a");
        assert_eq!(candidates[1].server_group, "b.example.com");
    }

    #[test]
    fn region_and_budget_filters_precede_the_candidate_limit() {
        let mut source = "proxies:\n".to_string();
        for i in 0..305 {
            source.push_str(&format!(
                " - {{name: 'HK-{i} 0.1x', type: socks5, server: example.invalid, port: 1080}}\n"
            ));
        }
        source.push_str(" - {name: 'JP-01 1x', type: socks5, server: example.invalid, port: 1080}\n - {name: 'US-01 2x', type: socks5, server: example.invalid, port: 1080}\n");
        let candidates: Vec<_> = extract_candidates(&source)
            .unwrap()
            .into_iter()
            .filter(|n| crate::node_metadata::eligible(&n.name))
            .take(super::MAX_CANDIDATES)
            .collect();
        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0].name, "JP-01 1x");
        assert_eq!(super::candidate_names(&source).unwrap().len(), 307);
    }

    #[test]
    fn creates_isolated_benchmark_selector() {
        let settings = AppSettings {
            mixed_port: 17890,
            controller_port: 19090,
            ..Default::default()
        };
        let candidates = extract_candidates(SOURCE).expect("candidates");
        let yaml = build_benchmark_config(SOURCE, &settings, &candidates).expect("config");
        let document: Value = serde_yaml::from_str(&yaml).expect("yaml");
        let root = document.as_mapping().expect("root");
        let groups = root
            .get(Value::String("proxy-groups".to_string()))
            .and_then(Value::as_sequence)
            .expect("groups");
        assert_eq!(
            groups[0]
                .as_mapping()
                .and_then(|group| group.get(Value::String("name".to_string())))
                .and_then(Value::as_str),
            Some(BENCHMARK_GROUP_NAME)
        );
        let expected_rule = format!("MATCH,{BENCHMARK_GROUP_NAME}");
        assert_eq!(
            root.get(Value::String("rules".to_string()))
                .and_then(Value::as_sequence)
                .and_then(|rules| rules.first())
                .and_then(Value::as_str),
            Some(expected_rule.as_str())
        );

        let listeners = vec![BandwidthListener {
            port: 17891,
            proxy: "node-a".to_string(),
        }];
        let yaml =
            build_benchmark_config_with_listeners(SOURCE, &settings, &candidates, &listeners)
                .expect("listener config");
        let document: Value = serde_yaml::from_str(&yaml).expect("listener yaml");
        let listener = document
            .as_mapping()
            .and_then(|root| root.get(Value::String("listeners".to_string())))
            .and_then(Value::as_sequence)
            .and_then(|listeners| listeners.first())
            .and_then(Value::as_mapping)
            .expect("listener");
        assert_eq!(
            listener
                .get(Value::String("listen".to_string()))
                .and_then(Value::as_str),
            Some("127.0.0.1")
        );
        assert_eq!(
            listener
                .get(Value::String("proxy".to_string()))
                .and_then(Value::as_str),
            Some("node-a")
        );
    }

    #[test]
    fn benchmark_ignores_runtime_user_rules_without_mutating_the_overlay() {
        let settings = AppSettings {
            user_rules: vec![crate::user_rules::UserRule {
                id: "fixture-rule".into(),
                enabled: true,
                rule: format!("DOMAIN,example.com,{}", crate::effective::OPENAI_GROUP_NAME),
                note: String::new(),
            }],
            ..Default::default()
        };
        let candidates = extract_candidates(SOURCE).expect("candidates");
        let yaml =
            build_benchmark_config(SOURCE, &settings, &candidates).expect("isolated benchmark");
        assert!(!yaml.contains("DOMAIN,example.com"));
        assert_eq!(settings.user_rules.len(), 1);
    }

    #[test]
    fn score_rewards_lower_latency_and_higher_bandwidth() {
        assert!(node_score(100, 10, Some(100.0)) > node_score(800, 300, Some(20.0)));
    }

    #[test]
    fn selection_limits_same_server_before_filling() {
        let ranked = vec![
            ranked("a1", "shared", 99.0),
            ranked("a2", "shared", 98.0),
            ranked("a3", "shared", 97.0),
            ranked("b1", "other", 80.0),
        ];
        let selected = select_diverse_nodes(ranked, 3);
        assert_eq!(selected.len(), 3);
        assert!(selected.iter().any(|node| node.name == "b1"));
    }

    fn ranked(name: &str, server: &str, score: f64) -> (CandidateNode, OpenAiNodeScore) {
        (
            CandidateNode {
                name: name.to_string(),
                server_group: server.to_string(),
            },
            OpenAiNodeScore {
                name: name.to_string(),
                latency_ms: 100,
                jitter_ms: 10,
                bandwidth_mbps: Some(50.0),
                score,
                checked_at: Utc::now(),
            },
        )
    }
}
