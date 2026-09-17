//! Sticky, evidence-based failover. Only selects a destination for NEW connections.
//! Never calls /connections DELETE, reloads the core from a timer, or replays model requests.
use crate::route_health::{ProbeReport, ProbeState, Target};
use crate::{
    error::{AppError, AppErrorDto, AppResult},
    mihomo_api::{DelayProbe, MihomoApiClient},
    models::RuntimePhase,
    runtime::MihomoRuntime,
    storage::AppStorage,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::sync::atomic::AtomicU64;
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tauri::{AppHandle, Manager};
use uuid::Uuid;
pub const GROUP: &str = "🤖 OpenAI 自动灾备";
const WINDOW: u64 = 900;
const COOLDOWN: u64 = 300;
fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

async fn probe_target(api: &MihomoApiClient, name: &str, target: Target) -> ProbeReport {
    let (url, expected) = target.probe();
    let (state, latency_ms) = match api.health_probe(name, url, Some(expected)).await {
        DelayProbe::Passed(ms) => (ProbeState::Passed, Some(ms)),
        DelayProbe::Unknown => (ProbeState::Unknown, None),
        DelayProbe::Failed => match api.health_probe(name, url, None).await {
            // HTTP worked but the expected response did not. This could be a
            // challenge, service error or transient recovery: neither model
            // success nor enough evidence to punish this node.
            DelayProbe::Passed(ms) => (ProbeState::HttpUnverified, Some(ms)),
            DelayProbe::Failed => (ProbeState::Failed, None),
            DelayProbe::Unknown => (ProbeState::Unknown, None),
        },
    };
    ProbeReport {
        state,
        checked_at: now(),
        latency_ms,
    }
}

fn all_exits_failed(checks: &[(String, ProbeReport, ProbeReport)], target: Target) -> bool {
    checks.len() >= 2
        && checks.iter().all(|(_, chatgpt, api)| {
            (match target {
                Target::Chatgpt => chatgpt,
                Target::OpenaiApi => api,
            })
            .state
                == ProbeState::Failed
        })
}

#[derive(Clone, Copy)]
pub enum Evidence {
    Probe(bool),
    ModelComplete,
    ModelInterrupted,
}
#[derive(Clone, Default)]
struct NodeHealth {
    samples: VecDeque<(u64, bool, bool)>,
    consecutive_failures: u32,
    recovery_passes: u32,
    cooldown_until: u64,
    last_probe: u64,
    model_completed: u64,
    model_interrupted: u64,
    chatgpt: ProbeReport,
    openai_api: ProbeReport,
}
impl NodeHealth {
    fn probes(
        &mut self,
        target: Target,
        chatgpt: ProbeReport,
        openai_api: ProbeReport,
        time: u64,
        shared_failure: bool,
    ) {
        let report = match target {
            Target::Chatgpt => &chatgpt,
            Target::OpenaiApi => &openai_api,
        };
        if let Some(ok) = report.evidence().filter(|_| !shared_failure) {
            self.record(time, Evidence::Probe(ok));
        } else {
            // Keep historical evidence/cooldown, but do not select an unverified
            // candidate using a success from a previous round.
            self.last_probe = 0;
            self.recovery_passes = 0;
        }
        self.chatgpt = chatgpt;
        self.openai_api = openai_api;
    }
    fn prune(&mut self, time: u64) {
        while self
            .samples
            .front()
            .is_some_and(|(t, _, _)| time.saturating_sub(*t) > WINDOW)
        {
            self.samples.pop_front();
        }
        while self.samples.len() > 90 {
            self.samples.pop_front();
        }
    }
    fn record(&mut self, time: u64, event: Evidence) {
        self.prune(time);
        let (ok, model) = match event {
            Evidence::Probe(ok) => {
                self.last_probe = time;
                (ok, false)
            }
            Evidence::ModelComplete => {
                self.model_completed += 1;
                (true, true)
            }
            Evidence::ModelInterrupted => {
                self.model_interrupted += 1;
                (false, true)
            }
        };
        self.samples.push_back((time, ok, model));
        if ok {
            // Probe successes cannot erase a streak of real model-stream failures.
            if model
                || !self
                    .samples
                    .iter()
                    .rev()
                    .take(6)
                    .any(|(_, good, m)| *m && !good)
            {
                self.consecutive_failures = 0;
            }
            if time >= self.cooldown_until {
                self.recovery_passes += 1;
            }
        } else {
            self.consecutive_failures += 1;
            self.recovery_passes = 0;
            if self.consecutive_failures >= 2 {
                self.cooldown_until = time + COOLDOWN;
            }
        }
    }
    fn usable(&self, time: u64) -> bool {
        self.last_probe > 0
            && time.saturating_sub(self.last_probe) <= 240
            && self.cooldown_until <= time
            && (self.cooldown_until == 0 || self.recovery_passes >= 3)
            && self
                .samples
                .iter()
                .rev()
                .find(|(_, _, model)| !model)
                .is_some_and(|(_, ok, _)| *ok)
    }
    fn score(&self) -> f64 {
        let (good, total) =
            self.samples
                .iter()
                .fold((0.0, 0.0), |(good, total), (_, ok, model)| {
                    let weight = if *model { 3.0 } else { 1.0 };
                    (good + if *ok { weight } else { 0.0 }, total + weight)
                });
        if total == 0.0 {
            0.0
        } else {
            good / total
        }
    }
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeSnapshot {
    name: String,
    probe_ok: bool,
    success_rate: Option<u64>,
    samples: usize,
    cooldown_seconds: u64,
    recovery_passes: u32,
    model_completed: u64,
    model_interrupted: u64,
    chatgpt: ProbeReport,
    openai_api: ProbeReport,
}
#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StabilitySnapshot {
    pub enabled: bool,
    pub running: bool,
    pub eligible: bool,
    pub profile_id: Option<Uuid>,
    pub revision_id: Option<Uuid>,
    pub current: Option<String>,
    pub last_switch: Option<u64>,
    pub message: String,
    pub nodes: Vec<NodeSnapshot>,
    pub selection_target: Target,
    pub last_check: Option<u64>,
    pub common_failure: bool,
}
#[derive(Default)]
struct HealthState {
    profile: Option<Uuid>,
    revision: Option<Uuid>,
    current: Option<String>,
    epoch: u64,
    nodes: BTreeMap<String, NodeHealth>,
    identities: BTreeMap<String, [u8; 32]>,
    snapshot: StabilitySnapshot,
}

impl HealthState {
    fn reconcile(
        &mut self,
        profile: Option<Uuid>,
        revision: Option<Uuid>,
        identities: BTreeMap<String, [u8; 32]>,
    ) {
        if self.profile == profile && self.revision == revision {
            return;
        }
        if self.profile != profile {
            self.nodes.clear();
        } else {
            // Regenerating a ranking/revision must not erase a failed node's
            // cooldown. Never carry observations to a renamed/replaced server.
            self.nodes.retain(|name, _| {
                self.identities
                    .get(name)
                    .zip(identities.get(name))
                    .is_some_and(|(old, new)| old == new)
            });
        }
        self.current = None;
        self.snapshot.last_check = None;
        self.snapshot.common_failure = false;
        self.epoch += 1;
        self.profile = profile;
        self.revision = revision;
        self.identities = identities;
    }
}

fn node_identities(source: &str) -> AppResult<BTreeMap<String, [u8; 32]>> {
    let document: serde_yaml::Value = serde_yaml::from_str(source)
        .map_err(|_| AppError::Config("无法读取稳定策略节点身份".into()))?;
    let mut identities = BTreeMap::new();
    for node in document["proxies"].as_sequence().into_iter().flatten() {
        if let Some(name) = node["name"].as_str() {
            let bytes = serde_yaml::to_string(node)
                .map_err(|_| AppError::Config("无法核对稳定策略节点身份".into()))?;
            identities.insert(name.to_string(), Sha256::digest(bytes.as_bytes()).into());
        }
    }
    Ok(identities)
}
#[derive(Clone, Default)]
pub struct StabilityManager {
    inner: Arc<Mutex<HealthState>>,
    stopped: Arc<AtomicBool>,
    wake: Arc<tokio::sync::Notify>,
    last_hint: Arc<AtomicU64>,
}
/// A request contributes model evidence only when its outbound is the default RouteDeck
/// proxy and the observed group selection stays unchanged throughout the request.
#[derive(Clone)]
pub struct Observation {
    manager: StabilityManager,
    node: String,
    epoch: u64,
}
impl Observation {
    pub fn matches_connection(
        &self,
        data: &serde_json::Value,
        source: std::net::SocketAddr,
        host: &str,
    ) -> bool {
        if !source.ip().is_loopback() {
            return false;
        }
        data["connections"].as_array().is_some_and(|connections| {
            connections.iter().any(|connection| {
                let meta = &connection["metadata"];
                let port = meta["sourcePort"]
                    .as_u64()
                    .or_else(|| meta["sourcePort"].as_str().and_then(|p| p.parse().ok()));
                let ip = meta["sourceIP"]
                    .as_str()
                    .and_then(|ip| ip.parse::<std::net::IpAddr>().ok());
                let chains = connection["chains"].as_array();
                port == Some(u64::from(source.port()))
                    && ip == Some(source.ip())
                    && meta["host"].as_str() == Some(host)
                    && chains.is_some_and(|v| {
                        v.iter().any(|c| c.as_str() == Some(GROUP))
                            && v.iter().any(|c| c.as_str() == Some(&self.node))
                    })
            })
        })
    }
    pub fn finish(&self, event: Evidence) {
        if let Ok(mut s) = self.manager.inner.lock() {
            if s.epoch == self.epoch && s.current.as_ref() == Some(&self.node) && s.snapshot.running
            {
                s.nodes
                    .entry(self.node.clone())
                    .or_default()
                    .record(now(), event);
            }
        }
    }
}
impl StabilityManager {
    pub fn invalidate_observations(&self) {
        if let Ok(mut s) = self.inner.lock() {
            s.epoch += 1;
            s.snapshot.running = false;
        }
    }
    pub fn update_policy_status(&self, app: &AppHandle) {
        let Ok(storage) = AppStorage::from_app(app) else {
            return;
        };
        let Ok(persistent) = storage.state() else {
            return;
        };
        let profile = persistent
            .active_profile_id
            .and_then(|id| storage.load_profile(id).ok());
        if let Ok(mut s) = self.inner.lock() {
            s.epoch += 1;
            s.snapshot.running = false;
            s.snapshot.profile_id = persistent.active_profile_id;
            s.snapshot.revision_id = profile.as_ref().and_then(|p| p.active_revision_id);
            s.snapshot.eligible = profile.as_ref().is_some_and(|p| {
                p.openai_policy.enabled && p.openai_policy.selected_nodes.len() >= 2
            });
            s.snapshot.enabled = profile
                .as_ref()
                .is_some_and(|p| p.openai_policy.enabled && p.openai_policy.stability_enabled);
            s.snapshot.message = "策略已应用，等待下一轮稳定性检查".into();
        }
    }
    pub fn snapshot(&self) -> StabilitySnapshot {
        self.inner
            .lock()
            .map(|mut s| {
                let mut out = s.snapshot.clone();
                let time = now();
                out.current = s.current.clone();
                for node in s.nodes.values_mut() {
                    node.prune(time);
                }
                out.nodes = s
                    .nodes
                    .iter()
                    .map(|(name, n)| NodeSnapshot {
                        name: name.clone(),
                        probe_ok: n.usable(time),
                        success_rate: (!n.samples.is_empty())
                            .then(|| (n.score() * 100.0).round() as u64),
                        samples: n.samples.len(),
                        cooldown_seconds: n.cooldown_until.saturating_sub(time),
                        recovery_passes: n.recovery_passes,
                        model_completed: n.model_completed,
                        model_interrupted: n.model_interrupted,
                        chatgpt: n.chatgpt.fresh(time),
                        openai_api: n.openai_api.fresh(time),
                    })
                    .collect();
                out
            })
            .unwrap_or_default()
    }
    pub fn observe(&self, target: Target) -> Option<Observation> {
        let s = self.inner.lock().ok()?;
        if !s.snapshot.running || s.snapshot.selection_target != target {
            return None;
        }
        Some(Observation {
            manager: self.clone(),
            node: s.current.clone().filter(|s| s != "REJECT")?,
            epoch: s.epoch,
        })
    }
    pub fn request_check(&self, target: Target) {
        if self.stopped.load(Ordering::Acquire) {
            return;
        }
        let permitted = self.inner.lock().is_ok_and(|s| {
            s.snapshot.enabled && s.snapshot.running && s.snapshot.selection_target == target
        });
        if !permitted {
            return;
        }
        let time = now();
        let previous = self.last_hint.load(Ordering::Relaxed);
        if time.saturating_sub(previous) >= 30
            && self
                .last_hint
                .compare_exchange(previous, time, Ordering::AcqRel, Ordering::Relaxed)
                .is_ok()
        {
            self.wake.notify_one();
        }
    }
    pub fn stop(&self) {
        self.stopped.store(true, Ordering::Release);
    }
    pub fn start(&self, app: AppHandle) {
        let manager = self.clone();
        tauri::async_runtime::spawn(async move {
            let mut timer = tokio::time::interval(Duration::from_secs(60));
            timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            let mut earliest = tokio::time::Instant::now();
            while !manager.stopped.load(Ordering::Acquire) {
                tokio::select! { _ = timer.tick() => {}, _ = manager.wake.notified() => {} }
                tokio::time::sleep_until(earliest).await;
                if manager.stopped.load(Ordering::Acquire) {
                    break;
                }
                if manager.tick(&app).await.is_err() {
                    crate::app_log::record(
                        1,
                        crate::app_log::Area::Stability,
                        "稳定性检查暂不可用，保留当前节点；未关闭连接",
                    );
                    if let Ok(mut s) = manager.inner.lock() {
                        s.snapshot.running = false;
                        s.snapshot.message =
                            "稳定性检查暂不可用，保留当前节点；没有关闭连接".into();
                    }
                }
                // No overlapping rounds or accumulated interval catch-up. A burst
                // of client retries can request one follow-up, not a probe storm.
                earliest = tokio::time::Instant::now() + Duration::from_secs(30);
                timer.reset();
            }
        });
    }
    async fn tick(&self, app: &AppHandle) -> AppResult<()> {
        let storage = AppStorage::from_app(app)?;
        let active = storage.state()?.active_profile_id;
        let profile = active.map(|id| storage.load_profile(id)).transpose()?;
        let policy = profile.as_ref().map(|p| &p.openai_policy);
        let target = crate::local_routing::stability_target(app)?;
        let revision = profile.as_ref().and_then(|p| p.active_revision_id);
        let identities_changed = {
            let s = self
                .inner
                .lock()
                .map_err(|_| AppError::Runtime("稳定性状态不可用".into()))?;
            s.profile != active || s.revision != revision
        };
        let identities = if identities_changed {
            match active.zip(revision) {
                Some((id, revision)) => {
                    node_identities(&storage.load_revision_source(id, revision)?)?
                }
                None => BTreeMap::new(),
            }
        } else {
            BTreeMap::new()
        };
        {
            let mut s = self
                .inner
                .lock()
                .map_err(|_| AppError::Runtime("稳定性状态不可用".into()))?;
            s.reconcile(active, revision, identities);
            if s.snapshot.selection_target != target {
                // Evidence from the other service must not make this target healthy.
                s.nodes.clear();
                s.epoch += 1;
                s.snapshot.last_check = None;
            }
            s.snapshot.selection_target = target;
            s.nodes.retain(|name, _| {
                policy.is_some_and(|p| p.selected_nodes.iter().any(|n| n.name == *name))
            });
            s.snapshot.profile_id = active;
            s.snapshot.revision_id = revision;
            s.snapshot.eligible = policy.is_some_and(|p| p.enabled && p.selected_nodes.len() >= 2);
            s.snapshot.enabled = policy.is_some_and(|p| p.enabled && p.stability_enabled);
            s.snapshot.running = false;
            s.snapshot.common_failure = false;
            s.snapshot.message = "请先生成 OpenAI 灾备并启用稳定策略".into();
            if policy.is_some_and(|p| p.enabled && !p.stability_enabled) {
                s.snapshot.message =
                    "基础 Fallback 生效；稳定策略未开启，双目标检测和故障反馈选点未运行".into();
            }
        }
        let Some(policy) = policy.filter(|p| p.enabled && p.stability_enabled) else {
            return Ok(());
        };
        if app.state::<MihomoRuntime>().status(Some(app)).phase != RuntimePhase::Running {
            if let Ok(mut s) = self.inner.lock() {
                s.snapshot.message = "稳定策略已开启，等待代理核心运行；未执行双目标检测".into();
            }
            return Ok(());
        }
        let settings = storage.settings()?;
        let api = MihomoApiClient::new(&settings)?;
        let proxies = api.proxies().await?;
        let group = &proxies["proxies"][GROUP];
        if group["type"].as_str() != Some("Selector") {
            return Err(AppError::Conflict("核心尚未加载稳定策略".into()));
        }
        let current = group["now"].as_str().unwrap_or("REJECT").to_string();
        let run_before = app.state::<MihomoRuntime>().status(Some(app));
        if let Some(node) = active
            .map(|id| storage.openai_manual_node(id))
            .transpose()?
            .flatten()
        {
            let _permit = crate::user_rules::acquire_configuration(app)?;
            let latest = active.map(|id| storage.load_profile(id)).transpose()?;
            if storage.state()?.active_profile_id != active
                || latest.as_ref().and_then(|p| p.active_revision_id) != revision
                || !latest
                    .as_ref()
                    .is_some_and(|p| p.openai_policy.enabled && p.openai_policy.stability_enabled)
                || active
                    .map(|id| storage.openai_manual_node(id))
                    .transpose()?
                    .flatten()
                    .as_deref()
                    != Some(&node)
                || app.state::<MihomoRuntime>().status(Some(app)).phase != RuntimePhase::Running
            {
                return Ok(());
            }
            let live = api.proxies().await?;
            let valid = live["proxies"][GROUP]["type"] == "Selector"
                && policy.selected_nodes.iter().any(|n| n.name == node)
                && crate::node_selection::validate_choice(&live["proxies"][GROUP], &node).is_ok();
            if valid && live["proxies"][GROUP]["now"].as_str() != Some(&node) {
                api.select_proxy(GROUP, &node).await?;
            }
            let mut s = self
                .inner
                .lock()
                .map_err(|_| AppError::Runtime("稳定性状态不可用".into()))?;
            s.epoch += 1;
            s.current = if valid {
                Some(node)
            } else {
                live["proxies"][GROUP]["now"].as_str().map(str::to_string)
            };
            s.snapshot.running = false;
            s.snapshot.message = if valid {
                "手动选点中，稳定策略已暂停自动切换；可在代理页恢复自动"
            } else {
                "手动节点已不在候选中，未恢复自动；请重新选点或恢复自动"
            }
            .into();
            return Ok(());
        }
        let cost_preferences =
            storage.openai_costs(active.ok_or_else(|| AppError::Conflict("无活动配置".into()))?)?;
        let costs = cost_preferences.resolve(&storage.load_revision_source(
            active.unwrap(),
            revision.ok_or_else(|| AppError::Conflict("无配置版本".into()))?,
        )?)?;
        let allowed: Vec<String> = policy
            .selected_nodes
            .iter()
            .filter(|n| {
                costs.allowed(&n.name)
                    && group["all"]
                        .as_array()
                        .is_some_and(|all| all.iter().any(|x| x.as_str() == Some(&n.name)))
            })
            .take(10)
            .map(|n| n.name.clone())
            .collect();
        {
            let mut s = self
                .inner
                .lock()
                .map_err(|_| AppError::Runtime("稳定性状态不可用".into()))?;
            if s.current.as_ref() != Some(&current) {
                s.epoch += 1;
                s.current = Some(current.clone());
            }
            s.snapshot.running = true;
            s.snapshot.message =
                "基础连通性与真实模型流分开统计；保持健康节点，不主动清空连接".into();
        }
        use futures_util::{stream, StreamExt};
        let checks = stream::iter(allowed.iter().cloned().map(|name| {
            let api = api.clone();
            async move {
                // Sequential per node, only two nodes in flight. No account credentials,
                // model requests, throughput tests or runtime selection changes.
                let chatgpt = probe_target(&api, &name, Target::Chatgpt).await;
                let openai_api = probe_target(&api, &name, Target::OpenaiApi).await;
                (name, chatgpt, openai_api)
            }
        }))
        .buffer_unordered(2)
        .collect::<Vec<_>>()
        .await;
        // A stopped/reloaded core is not evidence that every candidate is broken.
        // Gate both recording and selection, not just the eventual controller write.
        let _permit = crate::user_rules::acquire_configuration(app)?;
        let run_after = app.state::<MihomoRuntime>().status(Some(app));
        if self.stopped.load(Ordering::Acquire)
            || run_after.phase != RuntimePhase::Running
            || run_before.pid != run_after.pid
            || run_before.started_at != run_after.started_at
            || crate::local_routing::stability_target(app)? != target
            || storage.state()?.active_profile_id != active
        {
            self.invalidate_observations();
            return Ok(());
        }
        let latest = active.map(|id| storage.load_profile(id)).transpose()?;
        // A manual choice made while probes were in flight wins, even when it
        // selected the same node and therefore did not change `now`.
        if active
            .map(|id| storage.openai_manual_node(id))
            .transpose()?
            .flatten()
            .is_some()
        {
            self.invalidate_observations();
            return Ok(());
        }
        if latest.as_ref().and_then(|p| p.active_revision_id)
            != profile.as_ref().and_then(|p| p.active_revision_id)
            || !latest
                .as_ref()
                .is_some_and(|p| p.openai_policy.stability_enabled)
            || storage.openai_costs(active.unwrap())?.revision != cost_preferences.revision
        {
            self.invalidate_observations();
            return Ok(());
        }
        let live = api.proxies().await?;
        if live["proxies"][GROUP]["type"].as_str() != Some("Selector")
            || live["proxies"][GROUP]["now"].as_str() != Some(&current)
        {
            self.invalidate_observations();
            return Ok(());
        }
        let candidate = {
            let mut s = self
                .inner
                .lock()
                .map_err(|_| AppError::Runtime("稳定性状态不可用".into()))?;
            // When every probed exit fails together, do not churn through exits.
            // The current within-budget exit is held while the shared fault is checked.
            let common_failure = all_exits_failed(&checks, target);
            s.snapshot.common_failure = common_failure;
            s.snapshot.last_check = Some(now());
            for (name, chatgpt, openai_api) in checks {
                let health = s.nodes.entry(name).or_default();
                health.probes(target, chatgpt, openai_api, now(), common_failure);
            }
            if common_failure && allowed.contains(&current) && costs.allowed(&current) {
                s.snapshot.message = "多个出口的目标检测同时失败，可能是公共网络或目标服务异常；保留当前合预算出口复查，未反复切换".into();
                current.clone()
            } else {
                choose_with_costs(&s.nodes, &current, &allowed, now(), &costs)
            }
        };
        if candidate == current {
            if candidate == "REJECT" {
                if let Ok(mut s) = self.inner.lock() {
                    s.snapshot.message = if costs.value_mode() {
                        "没有符合预算的健康节点，后续新请求仍被拒绝；请调整倍率或预算"
                    } else {
                        "没有可验证的健康候选，后续新请求仍被拒绝；请核对网络与节点检测"
                    }
                    .into();
                }
            }
            return Ok(());
        }
        api.select_proxy(GROUP, &candidate).await?;
        crate::app_log::record(
            1,
            crate::app_log::Area::Stability,
            "依据健康状态与自动选点预算为后续新连接更新出口；未关闭现有连接或重放请求",
        );
        let mut s = self
            .inner
            .lock()
            .map_err(|_| AppError::Runtime("稳定性状态不可用".into()))?;
        s.epoch += 1;
        s.snapshot.message = if candidate == "REJECT" {
            if costs.value_mode() {
                "没有符合预算的健康节点，拒绝后续新请求；未使用超预算或未获允许的未知倍率节点"
            } else {
                "没有可验证的健康候选，拒绝后续新请求；未主动关闭现有连接"
            }
        } else if costs.value_mode() {
            "已按健康状态与倍率更新后续新连接出口；未主动关闭现有连接"
        } else {
            "原节点持续失败，已为后续新连接切换出口；原有连接未被主动关闭"
        }
        .into();
        s.current = Some(candidate);
        s.snapshot.last_switch = Some(now());
        Ok(())
    }
}
fn choose(
    nodes: &BTreeMap<String, NodeHealth>,
    current: &str,
    allowed: &[String],
    time: u64,
) -> String {
    // One failed probe must not flap a live connection. Recent model failures count too.
    if allowed.iter().any(|n| n == current)
        && nodes
            .get(current)
            .is_some_and(|n| n.consecutive_failures < 2 && n.cooldown_until <= time)
    {
        return current.into();
    }
    allowed
        .iter()
        .filter(|name| nodes.get(*name).is_some_and(|n| n.usable(time)))
        .max_by(|a, b| {
            nodes[*a]
                .score()
                .total_cmp(&nodes[*b].score())
                .then_with(|| b.cmp(a))
        })
        .cloned()
        .unwrap_or_else(|| "REJECT".into())
}

fn choose_with_costs(
    nodes: &BTreeMap<String, NodeHealth>,
    current: &str,
    allowed: &[String],
    time: u64,
    costs: &crate::openai_cost::ResolvedCosts,
) -> String {
    if !costs.value_mode() {
        return choose(nodes, current, allowed, time);
    }
    let allowed: Vec<String> = allowed
        .iter()
        .filter(|name| costs.allowed(name))
        .cloned()
        .collect();
    // Preserve an already healthy, within-budget selection. Saving costs is
    // not a reason to flap working streams simply for a small price saving.
    if allowed.iter().any(|name| name == current)
        && nodes
            .get(current)
            .is_some_and(|n| n.consecutive_failures < 2 && n.cooldown_until <= time)
    {
        return current.into();
    }
    let best = allowed
        .iter()
        .filter_map(|name| nodes.get(name))
        .filter(|n| n.usable(time))
        .map(NodeHealth::score)
        .fold(0.0_f64, f64::max);
    allowed
        .iter()
        .filter(|name| {
            nodes
                .get(*name)
                .is_some_and(|n| n.usable(time) && n.score() >= (best - 0.1).max(0.7))
        })
        .max_by(|a, b| {
            costs
                .utility(a, nodes[*a].score() * 100.0)
                .total_cmp(&costs.utility(b, nodes[*b].score() * 100.0))
                .then_with(|| b.cmp(a))
        })
        .cloned()
        .unwrap_or_else(|| "REJECT".into())
}
#[tauri::command]
pub async fn set_openai_stability(
    app: AppHandle,
    enabled: bool,
    profile_id: Uuid,
    revision_id: Uuid,
    confirmed: bool,
) -> Result<(), AppErrorDto> {
    let work = async {
        if !confirmed {
            return Err(AppError::InvalidInput("更新稳定灾备需要确认".into()));
        }
        let storage = AppStorage::from_app(&app)?;
        if storage.state()?.active_profile_id != Some(profile_id) {
            return Err(AppError::Conflict("活动配置已变化，请刷新".into()));
        }
        let profile = storage.load_profile(profile_id)?;
        if profile.active_revision_id != Some(revision_id) {
            return Err(AppError::Conflict("配置版本已变化，请刷新".into()));
        }
        let mut policy = profile.openai_policy;
        if !enabled && storage.openai_costs(profile_id)?.mode == crate::openai_cost::CostMode::Value
        {
            return Err(AppError::Conflict(
                "请先将成本策略改为质量优先；性价比策略需要稳定优先执行预算限制".into(),
            ));
        }
        if !policy.enabled || policy.selected_nodes.len() < 2 {
            return Err(AppError::InvalidInput(
                "请先在代理页生成 OpenAI 灾备".into(),
            ));
        }
        policy.stability_enabled = enabled;
        crate::openai_policy::apply_policy_revision(&app, profile_id, &policy).await?;
        app.state::<StabilityManager>().update_policy_status(&app);
        Ok(())
    };
    work.await.map_err(|e| e.dto())
}
#[cfg(test)]
mod tests {
    use super::*;
    fn report(state: ProbeState) -> ProbeReport {
        ProbeReport {
            state,
            checked_at: 100,
            latency_ms: None,
        }
    }
    #[test]
    fn dual_target_evidence_is_isolated_and_shared_failure_is_neutral() {
        let mut n = NodeHealth::default();
        n.probes(
            Target::Chatgpt,
            report(ProbeState::Failed),
            report(ProbeState::Passed),
            100,
            false,
        );
        assert_eq!(n.consecutive_failures, 1);
        assert!(!n.usable(100), "an API pass cannot validate ChatGPT");
        n.probes(
            Target::Chatgpt,
            report(ProbeState::HttpUnverified),
            report(ProbeState::Passed),
            110,
            false,
        );
        assert_eq!(
            n.consecutive_failures, 1,
            "a challenge is not a node failure"
        );
        n.probes(
            Target::Chatgpt,
            report(ProbeState::Failed),
            report(ProbeState::Failed),
            120,
            true,
        );
        assert_eq!(
            n.cooldown_until, 0,
            "shared outages must not penalize every node"
        );
        assert!(!n.usable(120));
        n.probes(
            Target::OpenaiApi,
            report(ProbeState::Failed),
            report(ProbeState::Passed),
            130,
            false,
        );
        assert!(n.usable(130));
        n.probes(
            Target::OpenaiApi,
            report(ProbeState::Passed),
            report(ProbeState::Unknown),
            140,
            false,
        );
        assert!(!n.usable(140), "unknown cannot reuse an earlier success");
        let checks = vec![
            (
                "a".into(),
                report(ProbeState::Failed),
                report(ProbeState::Passed),
            ),
            (
                "b".into(),
                report(ProbeState::Failed),
                report(ProbeState::Unknown),
            ),
        ];
        assert!(all_exits_failed(&checks, Target::Chatgpt));
        assert!(!all_exits_failed(&checks, Target::OpenaiApi));
        assert!(!all_exits_failed(&checks[..1], Target::Chatgpt));
    }

    #[tokio::test]
    async fn fault_hints_are_coalesced_target_bound_and_do_not_score_nodes() {
        let manager = StabilityManager::default();
        manager.request_check(Target::Chatgpt);
        assert_eq!(manager.last_hint.load(Ordering::Relaxed), 0);
        {
            let mut s = manager.inner.lock().unwrap();
            s.snapshot.enabled = true;
            s.snapshot.running = true;
        }
        manager.request_check(Target::OpenaiApi);
        assert_eq!(manager.last_hint.load(Ordering::Relaxed), 0);
        for _ in 0..50 {
            manager.request_check(Target::Chatgpt);
        }
        tokio::time::timeout(Duration::from_millis(30), manager.wake.notified())
            .await
            .unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(30), manager.wake.notified())
                .await
                .is_err()
        );
        assert!(manager.inner.lock().unwrap().nodes.is_empty());
        manager.stop();
        manager.last_hint.store(0, Ordering::Relaxed);
        manager.request_check(Target::Chatgpt);
        assert_eq!(manager.last_hint.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn probe_distinguishes_expected_http_challenge_failure_and_controller_error() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        for (responses, expected, count) in [
            (vec![(200, "{\"delay\":24}")], ProbeState::Passed, 1),
            (
                vec![(503, "{}"), (200, "{\"delay\":30}")],
                ProbeState::HttpUnverified,
                2,
            ),
            (vec![(504, "{}"), (503, "{}")], ProbeState::Failed, 2),
            (vec![(401, "{}")], ProbeState::Unknown, 1),
            (vec![(200, "{\"delay\":0}")], ProbeState::Unknown, 1),
            (vec![(503, "{}"), (404, "{}")], ProbeState::Unknown, 2),
        ] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let api = MihomoApiClient::from_endpoint(
                listener.local_addr().unwrap().port(),
                "fixture".into(),
            )
            .unwrap();
            let task = tokio::spawn(async move {
                let mut requests = Vec::new();
                for (status, body) in responses {
                    let (mut socket, _) = listener.accept().await.unwrap();
                    let mut bytes = vec![0; 4096];
                    let n = socket.read(&mut bytes).await.unwrap();
                    requests.push(String::from_utf8_lossy(&bytes[..n]).into_owned());
                    socket.write_all(format!("HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
                }
                requests
            });
            let result = probe_target(&api, "fixture-node", Target::Chatgpt).await;
            assert_eq!(result.state, expected);
            let requests = tokio::time::timeout(Duration::from_secs(2), task)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(requests.len(), count);
            assert!(requests[0].starts_with("GET /proxies/fixture-node/delay?"));
            assert!(requests[0].contains("expected=200"));
            assert!(requests[0].contains("chatgpt.com%2Frobots.txt"));
            if count == 2 {
                assert!(!requests[1].contains("expected="));
            }
        }
    }
    #[test]
    fn refresh_retains_cooldown_only_for_unchanged_node_identity() {
        let profile = Uuid::new_v4();
        let source = "proxies:\n  - {name: a, type: vless, server: example.invalid, port: 443, uuid: fixture}\n";
        let mut state = HealthState::default();
        state.reconcile(
            Some(profile),
            Some(Uuid::new_v4()),
            node_identities(source).unwrap(),
        );
        state
            .nodes
            .entry("a".into())
            .or_default()
            .record(10, Evidence::Probe(false));
        state
            .nodes
            .get_mut("a")
            .unwrap()
            .record(20, Evidence::Probe(false));
        state.reconcile(
            Some(profile),
            Some(Uuid::new_v4()),
            node_identities(source).unwrap(),
        );
        assert_eq!(state.nodes["a"].cooldown_until, 320);
        let changed = source.replace("example.invalid", "new.example.invalid");
        state.reconcile(
            Some(profile),
            Some(Uuid::new_v4()),
            node_identities(&changed).unwrap(),
        );
        assert!(state.nodes.is_empty());
        state.nodes.insert("a".into(), NodeHealth::default());
        state.reconcile(
            Some(Uuid::new_v4()),
            Some(Uuid::new_v4()),
            node_identities(&changed).unwrap(),
        );
        assert!(state.nodes.is_empty());
    }
    #[test]
    fn healthy_current_is_sticky_and_one_failure_does_not_switch() {
        let mut nodes = BTreeMap::new();
        let mut a = NodeHealth::default();
        a.record(100, Evidence::Probe(true));
        a.record(110, Evidence::Probe(false));
        let mut b = NodeHealth::default();
        b.record(110, Evidence::Probe(true));
        nodes.insert("a".into(), a);
        nodes.insert("b".into(), b);
        assert_eq!(choose(&nodes, "a", &["a".into(), "b".into()], 110), "a");
        nodes
            .get_mut("a")
            .unwrap()
            .record(120, Evidence::Probe(false));
        assert_eq!(choose(&nodes, "a", &["a".into(), "b".into()], 120), "b");
    }
    #[test]
    fn cost_aware_failover_keeps_quality_budget_and_stickiness() {
        use crate::openai_cost::{CostMode, CostPreferences, ResolvedCosts};
        let mut costs = ResolvedCosts {
            preferences: CostPreferences {
                mode: CostMode::Value,
                ..Default::default()
            },
            multipliers: [
                ("cheap".into(), Some(1.0)),
                ("premium".into(), Some(5.0)),
                ("unknown".into(), None),
            ]
            .into(),
        };
        let mut healthy = NodeHealth::default();
        healthy.record(100, Evidence::Probe(true));
        let mut nodes = BTreeMap::from([
            ("cheap".into(), healthy.clone()),
            ("premium".into(), healthy.clone()),
            ("unknown".into(), healthy),
        ]);
        let allowed = ["cheap".into(), "premium".into(), "unknown".into()];
        assert_eq!(
            choose_with_costs(&nodes, "REJECT", &allowed, 100, &costs),
            "cheap"
        );
        assert_eq!(
            choose_with_costs(&nodes, "premium", &allowed, 100, &costs),
            "premium"
        );
        costs.preferences.max_multiplier = Some(2.0);
        assert_eq!(
            choose_with_costs(&nodes, "premium", &allowed, 100, &costs),
            "cheap"
        );
        nodes
            .get_mut("cheap")
            .unwrap()
            .record(101, Evidence::ModelInterrupted);
        nodes
            .get_mut("cheap")
            .unwrap()
            .record(102, Evidence::ModelInterrupted);
        assert_eq!(
            choose_with_costs(&nodes, "cheap", &allowed, 102, &costs),
            "REJECT"
        );
        costs.preferences.max_multiplier = None;
        assert_eq!(
            choose_with_costs(&nodes, "cheap", &allowed, 102, &costs),
            "premium"
        );
        costs.preferences.max_multiplier = Some(2.0);
        costs.preferences.allow_unknown = true;
        assert_eq!(
            choose_with_costs(&nodes, "cheap", &allowed, 102, &costs),
            "unknown"
        );
    }
    #[test]
    fn failed_nodes_need_cooldown_and_three_recovery_probes() {
        let mut a = NodeHealth::default();
        a.record(10, Evidence::Probe(false));
        a.record(20, Evidence::Probe(false));
        a.record(100, Evidence::Probe(true));
        assert!(!a.usable(100));
        for time in [330, 390] {
            a.record(time, Evidence::Probe(true));
            assert!(!a.usable(time));
        }
        a.record(450, Evidence::Probe(true));
        assert!(a.usable(450));
    }
    #[test]
    fn model_interruptions_are_not_erased_by_a_401_probe() {
        let mut a = NodeHealth::default();
        a.record(100, Evidence::Probe(true));
        a.record(101, Evidence::ModelInterrupted);
        a.record(102, Evidence::Probe(true));
        a.record(103, Evidence::ModelInterrupted);
        assert!(!a.usable(103));
        assert_eq!(a.model_completed, 0);
        assert_eq!(a.model_interrupted, 2);
        assert!(a.score() < 0.5);
    }
    #[test]
    fn only_unchanged_observations_contribute_model_evidence() {
        let manager = StabilityManager::default();
        {
            let mut s = manager.inner.lock().unwrap();
            s.current = Some("a".into());
            s.snapshot.running = true;
        }
        let valid = manager.observe(Target::Chatgpt).unwrap();
        assert!(manager.observe(Target::OpenaiApi).is_none());
        let connections = serde_json::json!({"connections":[{"metadata":{"sourceIP":"127.0.0.1","sourcePort":"45678","host":"chatgpt.com"},"chains":["a",GROUP]}]});
        assert!(valid.matches_connection(
            &connections,
            "127.0.0.1:45678".parse().unwrap(),
            "chatgpt.com"
        ));
        assert!(!valid.matches_connection(
            &connections,
            "127.0.0.1:45679".parse().unwrap(),
            "chatgpt.com"
        ));
        assert!(!valid.matches_connection(
            &connections,
            "127.0.0.1:45678".parse().unwrap(),
            "api.openai.com"
        ));
        let direct = serde_json::json!({"connections":[{"metadata":{"sourceIP":"127.0.0.1","sourcePort":45678,"host":"chatgpt.com"},"chains":["DIRECT"]}]});
        assert!(!valid.matches_connection(
            &direct,
            "127.0.0.1:45678".parse().unwrap(),
            "chatgpt.com"
        ));
        valid.finish(Evidence::ModelComplete);
        assert_eq!(manager.inner.lock().unwrap().nodes["a"].model_completed, 1);
        let stale = manager.observe(Target::Chatgpt).unwrap();
        manager.invalidate_observations();
        stale.finish(Evidence::ModelInterrupted);
        assert_eq!(
            manager.inner.lock().unwrap().nodes["a"].model_interrupted,
            0
        );
        assert!(manager.observe(Target::Chatgpt).is_none());
    }
    #[test]
    fn no_healthy_candidate_fails_closed_and_stale_evidence_expires() {
        assert_eq!(choose(&BTreeMap::new(), "a", &["a".into()], 100), "REJECT");
        let mut a = NodeHealth::default();
        a.record(10, Evidence::Probe(true));
        assert!(!a.usable(251));
        a.prune(1000);
        assert!(a.samples.is_empty());
    }
}
