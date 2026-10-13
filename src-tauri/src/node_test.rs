//! Read-only, bounded latency jobs. No selection/config writes or persistence.
use crate::{
    error::{AppError, AppResult},
    manual_outbound,
    mihomo_api::MihomoApiClient,
    models::{RoutingMode, RuntimePhase},
    runtime::MihomoRuntime,
    storage::AppStorage,
};
use futures_util::{stream, StreamExt};
use serde::Serialize;
use serde_json::Value;
use std::{
    collections::{BTreeSet, VecDeque},
    future::Future,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tauri::{AppHandle, Manager};
use uuid::Uuid;

const MAX_NODES: usize = 1000;
const CONCURRENCY: usize = 4;
const TOTAL_BUDGET: Duration = Duration::from_secs(15 * 60);

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Scope {
    pub profile_id: Uuid,
    pub revision_id: Uuid,
    pub mode_revision: Uuid,
    pub runtime_pid: Option<u32>,
    pub runtime_started_at: Option<String>,
    pub routing_mode: RoutingMode,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeState {
    Pending,
    Testing,
    Passed,
    Timeout,
    Failed,
    Unavailable,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeResult {
    pub name: String,
    pub state: NodeState,
    pub delay_ms: Option<u64>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Running,
    Cancelling,
    Completed,
    Cancelled,
    Failed,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub id: Uuid,
    pub scope: Scope,
    pub sequence: u64,
    pub phase: Phase,
    pub valid: bool,
    pub message: String,
    pub nodes: Vec<NodeResult>,
}
struct Task {
    state: Mutex<Snapshot>,
    cancel: tokio::sync::watch::Sender<bool>,
    active: AtomicBool,
}
impl Task {
    fn snapshot(&self) -> Snapshot {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
    fn change(&self, f: impl FnOnce(&mut Snapshot)) {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        f(&mut s);
        s.sequence += 1;
    }
    fn cancel(&self, invalidate: bool, message: &str) {
        self.change(|s| {
            if invalidate {
                s.valid = false;
            }
            if self.active.load(Ordering::Acquire) {
                s.phase = Phase::Cancelling;
            }
            s.message = message.into();
        });
        self.cancel.send_replace(true);
    }
    fn result(&self, name: &str, state: NodeState, delay: Option<u64>) {
        self.change(|s| {
            if s.phase != Phase::Running || !s.valid {
                return;
            }
            if let Some(n) = s.nodes.iter_mut().find(|n| n.name == name) {
                n.state = state;
                n.delay_ms = delay;
            }
        });
    }
}
// This guard keeps the slot occupied until all in-flight HTTP futures are dropped,
// including on panic/abort. Rapid cancel/restart never multiplies concurrency.
struct Worker(Arc<Task>);
impl Drop for Worker {
    fn drop(&mut self) {
        self.0.change(|s| {
            s.phase = match s.phase {
                Phase::Cancelling => Phase::Cancelled,
                Phase::Running => Phase::Failed,
                phase => phase,
            };
            if s.phase == Phase::Failed && s.message == "正在测试延迟与连通性" {
                s.message = "测试已中断，请重试".into();
            }
            for n in &mut s.nodes {
                if n.state == NodeState::Testing {
                    n.state = NodeState::Pending;
                }
            }
        });
        self.0.active.store(false, Ordering::Release);
    }
}
#[derive(Default)]
pub struct NodeTestManager {
    current: Mutex<Option<Arc<Task>>>,
}
impl NodeTestManager {
    fn begin(&self, scope: Scope, names: Vec<String>) -> AppResult<(Arc<Task>, bool)> {
        let mut slot = self
            .current
            .lock()
            .map_err(|_| AppError::Runtime("节点测试状态暂不可用".into()))?;
        if let Some(task) = slot.as_ref().filter(|t| t.active.load(Ordering::Acquire)) {
            let snapshot = task.snapshot();
            if snapshot.scope == scope && snapshot.valid && snapshot.phase == Phase::Running {
                return Ok((task.clone(), false));
            }
            return Err(AppError::Conflict(
                "上一轮节点测试正在结束，请稍后重试".into(),
            ));
        }
        let (cancel, _) = tokio::sync::watch::channel(false);
        let task = Arc::new(Task {
            state: Mutex::new(Snapshot {
                id: Uuid::new_v4(),
                scope,
                sequence: 0,
                phase: Phase::Running,
                valid: true,
                message: "正在测试延迟与连通性".into(),
                nodes: names
                    .into_iter()
                    .map(|name| NodeResult {
                        name,
                        state: NodeState::Pending,
                        delay_ms: None,
                    })
                    .collect(),
            }),
            cancel,
            active: AtomicBool::new(true),
        });
        *slot = Some(task.clone());
        Ok((task, true))
    }
    pub fn snapshot(&self) -> Option<Snapshot> {
        self.current.lock().ok()?.as_ref().map(|t| t.snapshot())
    }
    pub fn cancel(&self, id: Uuid) -> AppResult<Snapshot> {
        let slot = self
            .current
            .lock()
            .map_err(|_| AppError::Runtime("节点测试状态暂不可用".into()))?;
        let task = slot
            .as_ref()
            .filter(|t| t.snapshot().id == id)
            .ok_or_else(|| AppError::Conflict("测试任务已变化，请刷新状态".into()))?;
        if task.active.load(Ordering::Acquire) {
            task.cancel(false, "已停止派发测试，正在结束在途请求");
        }
        Ok(task.snapshot())
    }
    fn invalidate(&self) {
        if let Ok(slot) = self.current.lock() {
            if let Some(task) = slot.as_ref() {
                task.cancel(true, "配置、选点方式或核心状态已变化，本轮测试已结束");
            }
        }
    }
}
pub fn invalidate(app: &AppHandle) {
    if let Some(manager) = app.try_state::<NodeTestManager>() {
        manager.invalidate();
    }
}
fn scope(app: &AppHandle) -> AppResult<Scope> {
    let storage = AppStorage::from_app(app)?;
    let settings = storage.settings()?;
    manual_outbound::require_manual_mode(&settings)?;
    let state = storage.state()?;
    let runtime = app.state::<MihomoRuntime>().status(Some(app));
    if runtime.phase != RuntimePhase::Running {
        return Err(AppError::Conflict(
            "请先启动核心，再测试节点；测试不会自动启动代理".into(),
        ));
    }
    let (Some(profile_id), Some(revision_id)) = (state.active_profile_id, state.active_revision_id)
    else {
        return Err(AppError::Conflict("请先选用订阅或本地配置".into()));
    };
    let routing_mode = storage.load_profile(profile_id)?.routing_mode;
    if routing_mode == RoutingMode::Direct {
        return Err(AppError::Conflict(
            "直连模式不使用代理节点，请切回规则或全局模式后测试".into(),
        ));
    }
    Ok(Scope {
        profile_id,
        revision_id,
        mode_revision: settings.proxy_mode_revision,
        runtime_pid: runtime.pid,
        runtime_started_at: runtime.started_at.map(|v| v.to_rfc3339()),
        routing_mode,
    })
}
pub fn status(app: &AppHandle) -> Option<Snapshot> {
    let manager = app.state::<NodeTestManager>();
    if let Some(old) = manager.snapshot().filter(|s| s.valid) {
        if scope(app).ok().as_ref() != Some(&old.scope) {
            manager.invalidate();
        }
    }
    manager.snapshot()
}

// Iterative traversal handles nested/cyclic groups and provider nodes already
// present in the controller map. Never test groups or built-in pseudo-outbounds.
fn collect_nodes(payload: &Value, mode: RoutingMode) -> AppResult<Vec<String>> {
    let map = payload["proxies"]
        .as_object()
        .ok_or_else(|| AppError::Runtime("核心节点列表格式异常，请刷新后重试".into()))?;
    if map.len() > 20_000 {
        return Err(AppError::InvalidInput(
            "核心节点图过大，请拆分配置后测试".into(),
        ));
    }
    if mode == RoutingMode::Direct {
        return Ok(Vec::new());
    }
    let mut queue: VecDeque<String> = if mode == RoutingMode::Global {
        VecDeque::from(["GLOBAL".into()])
    } else {
        map.iter()
            .filter(|(n, v)| {
                !matches!(
                    n.as_str(),
                    "GLOBAL" | "COMPATIBLE" | "PASS" | crate::openai_stability::GROUP
                ) && v["all"].is_array()
            })
            .map(|(n, _)| n.clone())
            .collect()
    };
    let mut visited = BTreeSet::new();
    let mut nodes = BTreeSet::new();
    let mut edges = 0usize;
    while let Some(name) = queue.pop_front() {
        if !visited.insert(name.clone()) {
            continue;
        }
        let Some(node) = map.get(&name) else {
            continue;
        };
        if let Some(all) = node["all"].as_array() {
            edges += all.len();
            if edges > 100_000 {
                return Err(AppError::InvalidInput(
                    "策略组引用过多，请拆分配置后测试".into(),
                ));
            }
            queue.extend(all.iter().filter_map(Value::as_str).map(str::to_owned));
            continue;
        }
        if matches!(
            name.as_str(),
            "DIRECT" | "REJECT" | "REJECT-DROP" | "PASS" | "COMPATIBLE"
        ) || crate::config::is_subscription_metadata_node_name(&name)
        {
            continue;
        }
        if matches!(
            node["type"].as_str(),
            None | Some(
                "Direct"
                    | "Reject"
                    | "RejectDrop"
                    | "Pass"
                    | "Compatible"
                    | "Selector"
                    | "Fallback"
                    | "URLTest"
                    | "LoadBalance"
                    | "Relay"
            )
        ) {
            continue;
        }
        nodes.insert(name);
        if nodes.len() > MAX_NODES {
            return Err(AppError::InvalidInput(format!(
                "当前配置超过 {MAX_NODES} 个测试节点，请拆分配置后测试；本次未开始"
            )));
        }
    }
    Ok(nodes.into_iter().collect())
}

async fn execute<F, Fut, C>(task: Arc<Task>, probe: F, current: C, budget: Duration)
where
    F: Fn(String) -> Fut,
    Fut: Future<Output = (NodeState, Option<u64>)>,
    C: Fn() -> bool,
{
    let _worker = Worker(task.clone());
    let mut cancelled = task.cancel.subscribe();
    if *cancelled.borrow() {
        return;
    }
    let names: Vec<_> = task.snapshot().nodes.into_iter().map(|n| n.name).collect();
    let requests = stream::iter(names.into_iter().map(|name| {
        let task = task.clone();
        let probe = &probe;
        async move {
            task.result(&name, NodeState::Testing, None);
            let result = probe(name.clone()).await;
            (name, result)
        }
    }))
    .buffer_unordered(CONCURRENCY);
    tokio::pin!(requests);
    let deadline = tokio::time::sleep(budget);
    tokio::pin!(deadline);
    let mut monitor = tokio::time::interval(Duration::from_millis(250));
    loop {
        tokio::select! { biased;
            _ = cancelled.changed() => break,
            _ = monitor.tick() => { if !current() { task.cancel(true, "配置或核心状态已变化，本轮测试已结束"); break; } }
            _ = &mut deadline => { task.cancel(false, "达到本轮总时限，已保留完成结果，其余节点未测试"); break; }
            result = requests.next() => match result {
                Some((name, (state, delay))) => {
                    if !current() { task.cancel(true, "配置或核心状态已变化，本轮测试已结束"); break; }
                    task.result(&name, state, delay);
                }
                None => { task.change(|s| { if s.phase == Phase::Running { s.phase = Phase::Completed; s.message = "测试完成；仅检查延迟与连通性，未主动改选节点".into(); } }); break; }
            }
        }
    }
}

pub async fn start(
    app: AppHandle,
    profile_id: Uuid,
    revision_id: Uuid,
    mode_revision: Uuid,
) -> AppResult<Snapshot> {
    let _permit = crate::user_rules::acquire_configuration(&app)?;
    let expected = scope(&app)?;
    if (
        expected.profile_id,
        expected.revision_id,
        expected.mode_revision,
    ) != (profile_id, revision_id, mode_revision)
    {
        return Err(AppError::Conflict("配置已变化，请刷新后重新测试".into()));
    }
    let manager = app.state::<NodeTestManager>();
    if let Some(existing) = manager
        .snapshot()
        .filter(|s| s.scope == expected && s.valid && s.phase == Phase::Running)
    {
        return Ok(existing);
    }
    let api = MihomoApiClient::new(&AppStorage::from_app(&app)?.settings()?)?;
    let payload = api.proxies().await?;
    let names = collect_nodes(&payload, expected.routing_mode)?;
    if names.is_empty() {
        return Err(AppError::InvalidInput(
            "当前配置没有可测试的实际节点；请检查策略组及提供器是否已加载".into(),
        ));
    }
    if scope(&app)? != expected {
        return Err(AppError::Conflict("核心状态已变化，请重新测试".into()));
    }
    let (task, fresh) = manager.begin(expected.clone(), names)?;
    let result = task.snapshot();
    if fresh {
        tauri::async_runtime::spawn(async move {
            execute(
                task,
                |name| {
                    let api = api.clone();
                    async move { api.node_latency_probe(&name).await }
                },
                || scope(&app).ok().as_ref() == Some(&expected),
                TOTAL_BUDGET,
            )
            .await;
        });
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::AtomicUsize;

    fn context() -> Scope {
        Scope {
            profile_id: Uuid::new_v4(),
            revision_id: Uuid::new_v4(),
            mode_revision: Uuid::new_v4(),
            runtime_pid: Some(7),
            runtime_started_at: Some("fixture".into()),
            routing_mode: RoutingMode::Rule,
        }
    }
    fn names(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("node-{i}")).collect()
    }
    #[test]
    fn graph_walk_is_cycle_safe_deduplicated_and_only_tests_real_loaded_members() {
        let payload = json!({"proxies": {
            "GLOBAL":{"all":["G","only-global"]}, "G":{"all":["nested","A","DIRECT","REJECT","PASS","missing","剩余流量：100 GB"]},
            "nested":{"all":["G","A","B"]}, "A":{"type":"Trojan"}, "B":{"type":"Shadowsocks"}, "orphan":{"type":"Trojan"},
            "only-global":{"type":"Socks5"}, "DIRECT":{"type":"Direct"}, "REJECT":{"type":"Reject"}, "PASS":{"type":"Pass"},
            "剩余流量：100 GB":{"type":"Socks5"}, "🤖 OpenAI 自动灾备":{"all":["ai-only"]}, "ai-only":{"type":"Trojan"}
        }});
        assert_eq!(
            collect_nodes(&payload, RoutingMode::Rule).unwrap(),
            ["A", "B"]
        );
        assert_eq!(
            collect_nodes(&payload, RoutingMode::Global).unwrap(),
            ["A", "B", "only-global"]
        );
        assert!(collect_nodes(&payload, RoutingMode::Direct)
            .unwrap()
            .is_empty());
        assert!(collect_nodes(&json!({}), RoutingMode::Rule).is_err());
    }
    #[test]
    fn bounded_pool_has_no_silent_truncation() {
        for count in [0, 1, 2, 1000, 1001] {
            let mut map = serde_json::Map::new();
            map.insert("G".into(), json!({"all":names(count)}));
            for name in names(count) {
                map.insert(name, json!({"type":"Socks5"}));
            }
            let result = collect_nodes(&json!({"proxies":map}), RoutingMode::Rule);
            if count <= MAX_NODES {
                assert_eq!(result.unwrap().len(), count);
            } else {
                assert!(result.is_err());
            }
        }
    }
    #[test]
    fn duplicate_start_reuses_task_and_cancel_waits_for_worker_drop() {
        let m = NodeTestManager::default();
        let scope = context();
        let (t, fresh) = m.begin(scope.clone(), names(2)).unwrap();
        assert!(fresh);
        let (same, fresh) = m.begin(scope.clone(), names(2)).unwrap();
        assert!(!fresh);
        assert_eq!(t.snapshot().id, same.snapshot().id);
        assert!(m.cancel(Uuid::new_v4()).is_err());
        t.result("node-0", NodeState::Passed, Some(50));
        m.cancel(t.snapshot().id).unwrap();
        t.result("node-1", NodeState::Passed, Some(60));
        assert!(m.begin(scope.clone(), names(2)).is_err());
        drop(Worker(t.clone()));
        assert_eq!(t.snapshot().phase, Phase::Cancelled);
        assert_eq!(t.snapshot().nodes[1].state, NodeState::Pending);
        assert!(m.begin(scope, names(2)).unwrap().1);
    }
    struct InFlight(Arc<AtomicUsize>);
    impl Drop for InFlight {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }
    #[tokio::test]
    async fn executor_enforces_four_parallel_probes_and_completes() {
        let m = NodeTestManager::default();
        let (t, _) = m.begin(context(), names(17)).unwrap();
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        execute(
            t.clone(),
            |_| {
                let active = active.clone();
                let peak = peak.clone();
                async move {
                    let n = active.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(n, Ordering::SeqCst);
                    let _guard = InFlight(active);
                    tokio::time::sleep(Duration::from_millis(2)).await;
                    (NodeState::Passed, Some(30))
                }
            },
            || true,
            Duration::from_secs(2),
        )
        .await;
        assert_eq!(peak.load(Ordering::SeqCst), 4);
        assert_eq!(active.load(Ordering::SeqCst), 0);
        assert_eq!(t.snapshot().phase, Phase::Completed);
        assert!(t.snapshot().nodes.iter().all(|n| n.delay_ms == Some(30)));
        assert!(!t.active.load(Ordering::Acquire));
    }
    #[tokio::test]
    async fn cancellation_drops_inflight_requests_and_keeps_partial_results() {
        let m = NodeTestManager::default();
        let (t, _) = m.begin(context(), names(20)).unwrap();
        let active = Arc::new(AtomicUsize::new(0));
        let live = active.clone();
        let work = execute(
            t.clone(),
            move |name| {
                let active = live.clone();
                async move {
                    if name == "node-0" {
                        return (NodeState::Passed, Some(10));
                    }
                    active.fetch_add(1, Ordering::SeqCst);
                    let _guard = InFlight(active);
                    std::future::pending().await
                }
            },
            || true,
            Duration::from_secs(2),
        );
        let cancel = async {
            while active.load(Ordering::SeqCst) < 4 {
                tokio::task::yield_now().await;
            }
            m.cancel(t.snapshot().id).unwrap();
        };
        tokio::join!(work, cancel);
        assert_eq!(active.load(Ordering::SeqCst), 0);
        let s = t.snapshot();
        assert_eq!(s.phase, Phase::Cancelled);
        assert_eq!(
            s.nodes
                .iter()
                .filter(|n| n.state == NodeState::Passed)
                .count(),
            1
        );
        assert_eq!(
            s.nodes
                .iter()
                .filter(|n| n.state == NodeState::Pending)
                .count(),
            19
        );
    }
    #[tokio::test]
    async fn scope_changes_invalidate_and_total_deadline_preserves_unmeasured_state() {
        for valid in [false, true] {
            let m = NodeTestManager::default();
            let (t, _) = m.begin(context(), names(2)).unwrap();
            execute(
                t.clone(),
                |_| std::future::pending(),
                || valid,
                Duration::from_millis(10),
            )
            .await;
            let s = t.snapshot();
            assert_eq!(s.phase, Phase::Cancelled);
            assert_eq!(s.valid, valid);
            assert!(s
                .nodes
                .iter()
                .all(|n| n.state == NodeState::Pending && n.delay_ms.is_none()));
            m.invalidate();
            assert!(!t.snapshot().valid);
        }
    }
    #[tokio::test]
    async fn cancelled_before_worker_starts_does_not_send_requests() {
        let m = NodeTestManager::default();
        let (t, _) = m.begin(context(), names(2)).unwrap();
        m.invalidate();
        execute(
            t.clone(),
            |_| async { panic!("cancelled job dispatched a request") },
            || true,
            Duration::from_secs(1),
        )
        .await;
        assert_eq!(t.snapshot().phase, Phase::Cancelled);
        assert!(!t.active.load(Ordering::Acquire));
    }
}
