//! Sticky, evidence-based failover. Only selects a destination for NEW connections.
//! Never calls /connections DELETE, reloads the core from a timer, or replays model requests.
use crate::route_health::{ProbeReport, ProbeState, Target};
use crate::{
    error::{AppError, AppErrorDto, AppResult},
    mihomo_api::{DelayProbe, MihomoApiClient},
    models::{OpenAiNodeScore, OpenAiPolicy, RoutingMode, RuntimePhase},
    openai_cost::ResolvedCosts,
    openai_ranking::{is_ranked, score, RankSignals},
    runtime::MihomoRuntime,
    storage::AppStorage,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use std::{future::Future, net::IpAddr, sync::atomic::AtomicU64};
use tauri::{AppHandle, Manager};
use tokio::time::Instant;
use uuid::Uuid;
pub const GROUP: &str = "🤖 OpenAI 自动灾备";
const WINDOW: u64 = 900;
const COOLDOWN: u64 = 300;
const ACTIVE_INTERVAL: Duration = Duration::from_secs(8);
const RANKED_MIN_GAP: Duration = Duration::from_secs(2);
const STANDBY_INTERVAL: Duration = Duration::from_secs(60);
const CONFIRM_INTERVAL: Duration = Duration::from_secs(24);
const CONTROLLER_BUDGET: Duration = Duration::from_millis(1500);
const RANKED_DELAY_MS: u64 = 2500;
const RANKED_REQUEST_BUDGET: Duration = Duration::from_secs(3);
const SWEEP_BUDGET: Duration = Duration::from_millis(6500);
const STANDBY_CONCURRENCY: usize = 3;

pub(crate) fn manages(policy: &OpenAiPolicy) -> bool {
    policy.enabled
        && (is_ranked(policy)
            || (crate::capabilities::ENHANCED_STABILITY && policy.stability_enabled))
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub(crate) async fn probe_target(api: &MihomoApiClient, name: &str, target: Target) -> ProbeReport {
    probe_target_with_budget(api, name, target, 5000, Duration::from_secs(7)).await
}

async fn probe_target_with_budget(
    api: &MihomoApiClient,
    name: &str,
    target: Target,
    delay_ms: u64,
    request_budget: Duration,
) -> ProbeReport {
    let (url, expected) = target.probe();
    let (state, latency_ms) = match api
        .health_probe(name, url, Some(expected), delay_ms, request_budget)
        .await
    {
        DelayProbe::Passed(ms) => (ProbeState::Passed, Some(ms)),
        DelayProbe::Unknown => (ProbeState::Unknown, None),
        DelayProbe::Failed => match api
            .health_probe(name, url, None, delay_ms, request_budget)
            .await
        {
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

type NodeCheck = (String, ProbeReport, ProbeReport);

struct RankedRound {
    checks: Vec<NodeCheck>,
    confirmed_failure: bool,
    checked_confirmation: bool,
}

async fn ranked_node_check<F, P>(probe: &F, name: String, dual_target: bool) -> NodeCheck
where
    F: Fn(String, Target) -> P,
    P: Future<Output = ProbeReport>,
{
    let (chatgpt, openai) = tokio::join!(probe(name.clone(), Target::Chatgpt), async {
        if dual_target {
            probe(name.clone(), Target::OpenaiApi).await
        } else {
            ProbeReport::default()
        }
    });
    (name, chatgpt, openai)
}

async fn ranked_sweep<F, P>(probe: &F, standby: &[String], dual_target: bool) -> Vec<NodeCheck>
where
    F: Fn(String, Target) -> P,
    P: Future<Output = ProbeReport>,
{
    use futures_util::{stream, StreamExt};
    let count = if dual_target {
        STANDBY_CONCURRENCY
    } else {
        standby.len()
    };
    stream::iter(
        standby
            .iter()
            .take(count)
            .cloned()
            .map(|name| ranked_node_check(probe, name, dual_target)),
    )
    .buffer_unordered(STANDBY_CONCURRENCY)
    .take_until(tokio::time::sleep(SWEEP_BUDGET))
    .collect()
    .await
}

async fn ranked_round<F, P>(
    probe: &F,
    current: Option<&str>,
    standby: &[String],
    periodic: bool,
    can_confirm: bool,
) -> RankedRound
where
    F: Fn(String, Target) -> P,
    P: Future<Output = ProbeReport>,
{
    let (primary, mut checks) = tokio::join!(
        async {
            match current {
                Some(name) => Some(ranked_node_check(probe, name.to_owned(), periodic).await),
                None => None,
            }
        },
        async {
            if periodic {
                ranked_sweep(probe, standby, true).await
            } else {
                Vec::new()
            }
        }
    );
    let mut confirmed_failure = false;
    let mut checked_confirmation = false;
    if let Some((name, mut chatgpt, openai)) = primary {
        if chatgpt.state == ProbeState::Failed && can_confirm {
            checked_confirmation = true;
            let (confirmation, urgent) =
                tokio::join!(probe(name.clone(), Target::Chatgpt), async {
                    if checks
                        .iter()
                        .any(|(_, chatgpt, _)| chatgpt.state == ProbeState::Passed)
                    {
                        Vec::new()
                    } else {
                        let remaining: Vec<_> = standby
                            .iter()
                            .filter(|candidate| {
                                !checks.iter().any(|(checked, _, _)| checked == *candidate)
                            })
                            .cloned()
                            .collect();
                        ranked_sweep(probe, &remaining, false).await
                    }
                });
            confirmed_failure = confirmation.state == ProbeState::Failed;
            chatgpt = confirmation;
            checks.extend(urgent);
        }
        checks.push((name, chatgpt, openai));
    }
    RankedRound {
        checks,
        confirmed_failure,
        checked_confirmation,
    }
}

fn all_exits_failed(checks: &[NodeCheck], target: Target) -> bool {
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
    latencies: VecDeque<(u64, u32)>,
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
            if ok {
                if let Some(delay) = report.latency_ms.and_then(|ms| u32::try_from(ms).ok()) {
                    self.latencies.push_back((time, delay));
                }
            }
        } else {
            // Keep historical evidence/cooldown, but do not select an unverified
            // candidate using a success from a previous round.
            self.last_probe = 0;
            if !shared_failure {
                self.recovery_passes = 0;
            }
        }
        self.chatgpt = chatgpt;
        if openai_api.checked_at != 0 {
            self.openai_api = openai_api;
        }
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
        while self
            .latencies
            .front()
            .is_some_and(|(t, _)| time.saturating_sub(*t) > WINDOW)
        {
            self.latencies.pop_front();
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

struct RankedNode {
    name: String,
    score: f64,
    stream_samples: usize,
    connection_samples: usize,
    latency_ms: u32,
}

fn rank_node(
    baseline: &OpenAiNodeScore,
    health: Option<&NodeHealth>,
    costs: &ResolvedCosts,
    time: u64,
) -> RankedNode {
    let mut signals = RankSignals {
        stream_successes: 0,
        stream_samples: 0,
        connection_successes: baseline.connection_successes,
        connection_samples: baseline.connection_samples,
        latency_ms: baseline.latency_ms,
        jitter_ms: baseline.jitter_ms,
        traffic_multiplier: costs.multiplier(&baseline.name),
    };
    if let Some(health) = health {
        let mut connection_samples = 0;
        let mut connection_successes = 0;
        for &(_, ok, model) in health
            .samples
            .iter()
            .filter(|(t, _, _)| time.saturating_sub(*t) <= WINDOW)
        {
            if model {
                signals.stream_samples += 1;
                signals.stream_successes += usize::from(ok);
            } else {
                connection_samples += 1;
                connection_successes += usize::from(ok);
            }
        }
        if connection_samples > 0 {
            signals.connection_samples = connection_samples;
            signals.connection_successes = connection_successes;
        }
        let mut delays: Vec<_> = health
            .latencies
            .iter()
            .filter(|(t, _)| time.saturating_sub(*t) <= WINDOW)
            .map(|(_, delay)| *delay)
            .collect();
        if delays.len() >= 3 {
            delays.sort_unstable();
            signals.latency_ms = delays[(delays.len() * 9).div_ceil(10) - 1];
            signals.jitter_ms = signals.latency_ms - delays[delays.len() / 2];
        }
    }
    RankedNode {
        name: baseline.name.clone(),
        score: score(signals),
        stream_samples: signals.stream_samples,
        connection_samples: signals.connection_samples,
        latency_ms: signals.latency_ms,
    }
}

fn rank_nodes(
    policy: &OpenAiPolicy,
    nodes: &BTreeMap<String, NodeHealth>,
    allowed: &[String],
    costs: &ResolvedCosts,
    time: u64,
) -> Vec<RankedNode> {
    let mut ranking: Vec<_> = policy
        .selected_nodes
        .iter()
        .filter(|node| allowed.contains(&node.name))
        .map(|node| rank_node(node, nodes.get(&node.name), costs, time))
        .collect();
    ranking.sort_by(|left, right| {
        right
            .score
            .total_cmp(&left.score)
            .then_with(|| right.connection_samples.cmp(&left.connection_samples))
            .then_with(|| left.latency_ms.cmp(&right.latency_ms))
            .then_with(|| left.name.cmp(&right.name))
    });
    ranking
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeSnapshot {
    name: String,
    rank: Option<usize>,
    recommendation_score: Option<f64>,
    stream_samples: usize,
    connection_samples: usize,
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

impl NodeSnapshot {
    fn new(
        name: &str,
        n: &NodeHealth,
        time: u64,
        rank: Option<(Option<usize>, &RankedNode)>,
    ) -> Self {
        Self {
            name: name.to_owned(),
            rank: rank.and_then(|(position, _)| position),
            recommendation_score: rank.map(|(_, row)| row.score),
            stream_samples: rank.map_or(0, |(_, row)| row.stream_samples),
            connection_samples: rank.map_or(0, |(_, row)| row.connection_samples),
            probe_ok: n.usable(time),
            success_rate: (!n.samples.is_empty()).then(|| (n.score() * 100.0).round() as u64),
            samples: n.samples.len(),
            cooldown_seconds: n.cooldown_until.saturating_sub(time),
            recovery_passes: n.recovery_passes,
            model_completed: n.model_completed,
            model_interrupted: n.model_interrupted,
            chatgpt: n.chatgpt.fresh(time),
            openai_api: n.openai_api.fresh(time),
        }
    }
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
    identities: BTreeMap<String, NodeIdentity>,
    ranking: Vec<RankedNode>,
    ranked_run: Option<(u32, chrono::DateTime<chrono::Utc>)>,
    ranked_poll: bool,
    last_ranked_sweep: Option<Instant>,
    last_ranked_confirmation: Option<Instant>,
    resolved_costs: Option<Arc<ResolvedCosts>>,
    reselect: bool,
    snapshot: StabilitySnapshot,
}

impl HealthState {
    fn reconcile(
        &mut self,
        profile: Option<Uuid>,
        revision: Option<Uuid>,
        identities: BTreeMap<String, NodeIdentity>,
    ) {
        if self.profile == profile && self.revision == revision {
            return;
        }
        if self.profile != profile {
            self.nodes.clear();
            self.ranked_run = None;
            self.reselect = false;
            self.snapshot.last_switch = None;
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
        if !self
            .current
            .as_ref()
            .is_some_and(|name| self.nodes.contains_key(name))
        {
            self.current = None;
        }
        self.ranking.clear();
        self.resolved_costs = None;
        self.last_ranked_sweep = None;
        self.last_ranked_confirmation = None;
        self.snapshot.last_check = None;
        self.snapshot.common_failure = false;
        self.epoch += 1;
        self.profile = profile;
        self.revision = revision;
        self.identities = identities;
    }

    // Distinct configured server addresses are independent candidates for
    // this check, without asserting distinct public egress IPs.
    fn independent_servers(&self, left: &str, right: &str) -> bool {
        self.identities
            .get(left)
            .and_then(|node| node.server.as_ref())
            .zip(
                self.identities
                    .get(right)
                    .and_then(|node| node.server.as_ref()),
            )
            .is_some_and(|(a, b)| a != b)
    }

    fn distinct_nodes(&self, left: &str, right: &str) -> bool {
        self.identities
            .get(left)
            .zip(self.identities.get(right))
            .is_some_and(|(a, b)| a.route != b.route)
    }
}

#[derive(PartialEq, Eq)]
struct NodeIdentity {
    config: [u8; 32],
    route: [u8; 32],
    server: Option<[u8; 32]>,
}

fn node_identities(source: &str) -> AppResult<BTreeMap<String, NodeIdentity>> {
    let document: serde_yaml::Value = serde_yaml::from_str(source)
        .map_err(|_| AppError::Config("无法读取稳定策略节点身份".into()))?;
    let mut identities = BTreeMap::new();
    for node in document["proxies"].as_sequence().into_iter().flatten() {
        if let Some(name) = node["name"].as_str() {
            let bytes = serde_yaml::to_string(node)
                .map_err(|_| AppError::Config("无法核对稳定策略节点身份".into()))?;
            let mut route = node.clone();
            if let Some(mapping) = route.as_mapping_mut() {
                mapping.remove(serde_yaml::Value::String("name".into()));
            }
            let route = serde_yaml::to_string(&route)
                .map_err(|_| AppError::Config("无法核对稳定策略节点配置".into()))?;
            let server = node["server"]
                .as_str()
                .map(|server| server.trim().trim_end_matches('.'))
                .filter(|server| !server.is_empty())
                .map(|server| {
                    let server = server.trim_start_matches('[').trim_end_matches(']');
                    let normalized = server
                        .parse::<IpAddr>()
                        .map_or_else(|_| server.to_ascii_lowercase(), |ip| ip.to_string());
                    Sha256::digest(normalized.as_bytes()).into()
                });
            identities.insert(
                name.to_string(),
                NodeIdentity {
                    config: Sha256::digest(bytes.as_bytes()).into(),
                    route: Sha256::digest(route.as_bytes()).into(),
                    server,
                },
            );
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

    pub(crate) fn schedule_check(&self) {
        self.wake.notify_one();
    }

    pub(crate) fn resume_ranked_selection(&self) {
        if let Ok(mut s) = self.inner.lock() {
            s.reselect = true;
            s.last_ranked_sweep = None;
        }
        self.schedule_check();
    }

    pub fn update_policy_status(&self, app: &AppHandle) {
        let Ok(storage) = AppStorage::from_app(app) else {
            return;
        };
        let Ok(persistent) = storage.state() else {
            return;
        };
        let proxy_mode = storage
            .settings()
            .is_ok_and(|s| crate::manual_outbound::require_proxy_mode(&s).is_ok());
        let profile = persistent
            .active_profile_id
            .filter(|_| proxy_mode)
            .and_then(|id| storage.load_profile(id).ok());
        if let Ok(mut s) = self.inner.lock() {
            s.epoch += 1;
            s.snapshot.running = false;
            s.snapshot.profile_id = persistent.active_profile_id;
            s.snapshot.revision_id = profile.as_ref().and_then(|p| p.active_revision_id);
            s.snapshot.enabled = profile
                .as_ref()
                .is_some_and(|p| p.routing_mode == RoutingMode::Rule && manages(&p.openai_policy));
            s.snapshot.eligible = s.snapshot.enabled
                && profile
                    .as_ref()
                    .is_some_and(|p| !p.openai_policy.selected_nodes.is_empty());
            s.snapshot.message = "策略已应用，等待下一轮稳定性检查".into();
            if s.snapshot.enabled {
                self.schedule_check();
            } else {
                s.ranked_run = None;
                s.ranked_poll = false;
                s.reselect = false;
            }
        }
    }
    pub fn snapshot(&self) -> StabilitySnapshot {
        self.inner
            .lock()
            .map(|mut s| {
                let mut out = s.snapshot.clone();
                let time = now();
                if s.profile != out.profile_id || s.revision != out.revision_id || !out.enabled {
                    out.current = None;
                    out.nodes.clear();
                    out.last_check = None;
                    out.last_switch = None;
                    return out;
                }
                out.current = s.current.clone();
                for node in s.nodes.values_mut() {
                    node.prune(time);
                }
                let empty = NodeHealth::default();
                out.nodes = if s.ranking.is_empty() {
                    s.nodes
                        .iter()
                        .map(|(name, health)| NodeSnapshot::new(name, health, time, None))
                        .collect()
                } else {
                    let mut rank = 0;
                    s.ranking
                        .iter()
                        .map(|row| {
                            let health = s.nodes.get(&row.name).unwrap_or(&empty);
                            let position = health.usable(time).then(|| {
                                rank += 1;
                                rank
                            });
                            NodeSnapshot::new(&row.name, health, time, Some((position, row)))
                        })
                        .collect()
                };
                out
            })
            .unwrap_or_default()
    }
    pub fn observe(&self, target: Target) -> Option<Observation> {
        if !crate::capabilities::ENHANCED_STABILITY {
            return None;
        }
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
            let mut next = Instant::now();
            let mut earliest = next;
            while !manager.stopped.load(Ordering::Acquire) {
                tokio::select! { _ = tokio::time::sleep_until(next) => {}, _ = manager.wake.notified() => {} }
                tokio::time::sleep_until(earliest).await;
                if manager.stopped.load(Ordering::Acquire) {
                    break;
                }
                let started = Instant::now();
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
                let finished = Instant::now();
                let ranked = manager.inner.lock().is_ok_and(|s| s.ranked_poll);
                if ranked {
                    earliest = finished + RANKED_MIN_GAP;
                    next = (started + ACTIVE_INTERVAL).max(earliest);
                } else {
                    earliest = finished + Duration::from_secs(30);
                    next = finished + STANDBY_INTERVAL;
                }
            }
        });
    }
    async fn tick(&self, app: &AppHandle) -> AppResult<()> {
        let storage = AppStorage::from_app(app)?;
        let settings = storage.settings()?;
        if crate::manual_outbound::require_proxy_mode(&settings).is_err() {
            self.update_policy_status(app);
            return Ok(());
        }
        let active = storage.state()?.active_profile_id;
        let profile = active.map(|id| storage.load_profile(id)).transpose()?;
        let Some(policy) = profile
            .as_ref()
            .filter(|p| p.routing_mode == RoutingMode::Rule && manages(&p.openai_policy))
            .map(|p| &p.openai_policy)
        else {
            self.update_policy_status(app);
            return Ok(());
        };
        let ranked = is_ranked(policy);
        let target = if ranked {
            Target::Chatgpt
        } else {
            crate::local_routing::stability_target(app)?
        };
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
            s.nodes
                .retain(|name, _| policy.selected_nodes.iter().any(|n| n.name == *name));
            s.snapshot.profile_id = active;
            s.snapshot.revision_id = revision;
            s.snapshot.enabled = true;
        }
        let run_before = app.state::<MihomoRuntime>().status(Some(app));
        if run_before.phase != RuntimePhase::Running {
            if let Ok(mut s) = self.inner.lock() {
                s.snapshot.running = false;
                s.snapshot.message = "等待代理核心运行，尚未进行节点检查".into();
                s.current = None;
                s.ranked_run = None;
                s.ranked_poll = false;
            }
            return Ok(());
        }
        let api = if ranked {
            MihomoApiClient::with_timeout(&settings, CONTROLLER_BUDGET)?
        } else {
            MihomoApiClient::new(&settings)?
        };
        let proxies = api.proxies().await?;
        let group = &proxies["proxies"][GROUP];
        if group["type"].as_str() != Some("Selector") {
            return Err(AppError::Conflict("核心尚未加载稳定策略".into()));
        }
        let current = group["now"].as_str().unwrap_or("REJECT").to_string();
        if let Some(node) = active
            .map(|id| storage.openai_manual_node(id))
            .transpose()?
            .flatten()
        {
            let _permit = crate::user_rules::acquire_configuration(app)?;
            let live_settings = storage.settings()?;
            crate::manual_outbound::require_proxy_mode(&live_settings)?;
            crate::manual_outbound::require_revision(&live_settings, settings.proxy_mode_revision)?;
            let latest = active.map(|id| storage.load_profile(id)).transpose()?;
            let live_run = app.state::<MihomoRuntime>().status(Some(app));
            if live_settings.controller_port != settings.controller_port
                || live_settings.controller_secret != settings.controller_secret
                || storage.state()?.active_profile_id != active
                || latest.as_ref().and_then(|p| p.active_revision_id) != revision
                || !latest.as_ref().is_some_and(|p| {
                    p.routing_mode == RoutingMode::Rule && manages(&p.openai_policy)
                })
                || active
                    .map(|id| storage.openai_manual_node(id))
                    .transpose()?
                    .flatten()
                    .as_deref()
                    != Some(&node)
                || live_run.phase != RuntimePhase::Running
                || live_run.pid != run_before.pid
                || live_run.started_at != run_before.started_at
            {
                return Ok(());
            }
            let live = api.proxies().await?;
            let valid = live["proxies"][GROUP]["type"] == "Selector"
                && crate::node_metadata::eligible(&node)
                && policy.selected_nodes.iter().any(|n| n.name == node)
                && crate::node_selection::validate_choice(&live["proxies"][GROUP], &node).is_ok();
            if valid && live["proxies"][GROUP]["now"].as_str() != Some(&node) {
                api.select_proxy(GROUP, &node).await?;
            }
            // Keep the invalid manual intent, but never route new requests via
            // an unqualified historical pin or silently resume auto-selection.
            if !valid
                && live["proxies"][GROUP]["type"] == "Selector"
                && live["proxies"][GROUP]["now"] != "REJECT"
            {
                crate::node_selection::validate_choice(&live["proxies"][GROUP], "REJECT")?;
                api.select_proxy(GROUP, "REJECT").await?;
            }
            let mut s = self
                .inner
                .lock()
                .map_err(|_| AppError::Runtime("稳定性状态不可用".into()))?;
            s.epoch += 1;
            s.current = if valid {
                Some(node)
            } else {
                Some("REJECT".into())
            };
            s.snapshot.running = false;
            s.ranked_poll = false;
            s.snapshot.message = if valid {
                "手动选点中，稳定策略已暂停自动切换；可在代理页恢复自动"
            } else {
                "手动节点地区或候选资格失效，新请求已拒绝；请重新选点或恢复自动"
            }
            .into();
            return Ok(());
        }
        let cost_preferences =
            storage.openai_costs(active.ok_or_else(|| AppError::Conflict("无活动配置".into()))?)?;
        let cached = self
            .inner
            .lock()
            .map_err(|_| AppError::Runtime("稳定性状态不可用".into()))?
            .resolved_costs
            .as_ref()
            .filter(|costs| costs.preferences.revision == cost_preferences.revision)
            .cloned();
        let costs = match cached {
            Some(costs) => costs,
            None => {
                let costs = Arc::new(cost_preferences.resolve(&storage.load_revision_source(
                    active.unwrap(),
                    revision.ok_or_else(|| AppError::Conflict("无配置版本".into()))?,
                )?)?);
                self.inner
                    .lock()
                    .map_err(|_| AppError::Runtime("稳定性状态不可用".into()))?
                    .resolved_costs = Some(costs.clone());
                costs
            }
        };
        let mut allowed: Vec<String> = policy
            .selected_nodes
            .iter()
            .filter(|n| {
                costs.allowed(&n.name)
                    && crate::node_metadata::eligible(&n.name)
                    && group["all"]
                        .as_array()
                        .is_some_and(|all| all.iter().any(|x| x.as_str() == Some(&n.name)))
            })
            .take(10)
            .map(|n| n.name.clone())
            .collect();
        let regional_candidates = policy
            .selected_nodes
            .iter()
            .filter(|n| {
                crate::node_metadata::eligible(&n.name)
                    && group["all"]
                        .as_array()
                        .is_some_and(|all| all.iter().any(|x| x.as_str() == Some(&n.name)))
            })
            .count();
        if !ranked && regional_candidates < 2 {
            allowed.clear();
        }
        let current_eligible = allowed.contains(&current);
        let (probe_epoch, ranked_plan) = {
            let mut s = self
                .inner
                .lock()
                .map_err(|_| AppError::Runtime("稳定性状态不可用".into()))?;
            let changed_outlet = s.current.as_ref() != Some(&current);
            if changed_outlet {
                s.epoch += 1;
                s.current = Some(current.clone());
                s.last_ranked_confirmation = None;
            }
            let lost_eligibility = s.ranked_poll && !current_eligible;
            s.snapshot.running = true;
            s.snapshot.eligible = !allowed.is_empty();
            s.ranked_poll = ranked && current_eligible;
            let plan = ranked.then(|| {
                let periodic = changed_outlet
                    || lost_eligibility
                    || s.last_ranked_sweep
                        .is_none_or(|last| last.elapsed() >= STANDBY_INTERVAL);
                let mut standby: Vec<_> = allowed
                    .iter()
                    .filter(|name| **name != current)
                    .cloned()
                    .collect();
                standby.sort_by_key(|name| {
                    let same_server = current_eligible && !s.independent_servers(name, &current);
                    (
                        !periodic && same_server,
                        s.nodes.get(name).map_or(0, |n| n.chatgpt.checked_at),
                        same_server,
                    )
                });
                let can_confirm = s
                    .last_ranked_confirmation
                    .is_none_or(|last| last.elapsed() >= CONFIRM_INTERVAL);
                (standby, periodic, can_confirm)
            });
            (s.epoch, plan)
        };
        let (checks, periodic, double_failure, checked_confirmation) =
            if let Some((standby, periodic, can_confirm)) = ranked_plan {
                let result = ranked_round(
                    &|name, target| {
                        let api = api.clone();
                        async move {
                            probe_target_with_budget(
                                &api,
                                &name,
                                target,
                                RANKED_DELAY_MS,
                                RANKED_REQUEST_BUDGET,
                            )
                            .await
                        }
                    },
                    current_eligible.then_some(current.as_str()),
                    &standby,
                    periodic,
                    can_confirm,
                )
                .await;
                (
                    result.checks,
                    periodic,
                    result.confirmed_failure,
                    result.checked_confirmation,
                )
            } else {
                use futures_util::{stream, StreamExt};
                let checks = stream::iter(allowed.iter().cloned().map(|name| {
                    let api = api.clone();
                    async move {
                        let chatgpt = probe_target(&api, &name, Target::Chatgpt).await;
                        let openai_api = probe_target(&api, &name, Target::OpenaiApi).await;
                        (name, chatgpt, openai_api)
                    }
                }))
                .buffer_unordered(2)
                .collect::<Vec<_>>()
                .await;
                (checks, false, false, false)
            };
        // A stopped/reloaded core is not evidence that every candidate is broken.
        // Gate both recording and selection, not just the eventual controller write.
        let _permit = crate::user_rules::acquire_configuration(app)?;
        let live_settings = storage.settings()?;
        crate::manual_outbound::require_proxy_mode(&live_settings)?;
        crate::manual_outbound::require_revision(&live_settings, settings.proxy_mode_revision)?;
        let run_after = app.state::<MihomoRuntime>().status(Some(app));
        if self.stopped.load(Ordering::Acquire)
            || run_after.phase != RuntimePhase::Running
            || run_before.pid != run_after.pid
            || run_before.started_at != run_after.started_at
            || live_settings.controller_port != settings.controller_port
            || live_settings.controller_secret != settings.controller_secret
            || (!ranked && crate::local_routing::stability_target(app)? != target)
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
        if latest.as_ref().and_then(|p| p.active_revision_id) != revision
            || !latest
                .as_ref()
                .is_some_and(|p| p.routing_mode == RoutingMode::Rule && manages(&p.openai_policy))
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
        let run = run_after.pid.zip(run_after.started_at);
        let (candidate, ranked_choice_verified) = {
            let mut s = self
                .inner
                .lock()
                .map_err(|_| AppError::Runtime("稳定性状态不可用".into()))?;
            if s.epoch != probe_epoch {
                return Ok(());
            }
            // When every probed exit fails together, do not churn through exits.
            // The current within-budget exit is held while the shared fault is checked.
            let common_failure = all_exits_failed(&checks, target)
                && (!ranked
                    || checks.iter().any(|(left, _, _)| {
                        checks
                            .iter()
                            .any(|(right, _, _)| s.distinct_nodes(left, right))
                    }));
            let time = now();
            let observed_at = Instant::now();
            if periodic {
                s.last_ranked_sweep = Some(observed_at);
            }
            if checked_confirmation {
                s.last_ranked_confirmation = Some(observed_at);
            }
            if checks
                .iter()
                .any(|(name, chatgpt, _)| *name == current && chatgpt.state == ProbeState::Passed)
            {
                s.last_ranked_confirmation = None;
            }
            let has_pass = checks
                .iter()
                .any(|(_, chatgpt, _)| chatgpt.state == ProbeState::Passed);
            if !checks.is_empty() {
                s.snapshot.last_check = Some(time);
                s.snapshot.common_failure =
                    common_failure || (ranked && s.snapshot.common_failure && !has_pass);
            }
            let verified_fault = ranked
                && double_failure
                && checks.iter().any(|(name, chatgpt, _)| {
                    chatgpt.state == ProbeState::Passed && s.distinct_nodes(name, &current)
                });
            let mut verified: Vec<_> = checks
                .iter()
                .filter(|(name, chatgpt, _)| {
                    chatgpt.state == ProbeState::Passed
                        && (!verified_fault || s.distinct_nodes(name, &current))
                })
                .map(|(name, _, _)| name.clone())
                .collect();
            for (name, chatgpt, openai_api) in checks {
                let primary = name == current;
                let neutral = common_failure
                    || (ranked
                        && chatgpt.state == ProbeState::Failed
                        && (!has_pass || (primary && !verified_fault)));
                let health = s.nodes.entry(name).or_default();
                health.probes(target, chatgpt, openai_api, time, neutral);
                if primary && verified_fault {
                    health.record(time, Evidence::Probe(false));
                }
            }
            if ranked {
                s.ranking = rank_nodes(policy, &s.nodes, &allowed, &costs, time);
            }
            s.snapshot.message = "根据目标连通性和节点权重选择后续新连接的出口".into();
            if s.snapshot.common_failure && current_eligible {
                s.snapshot.message = "多个出口的目标检测同时失败，可能是公共网络或目标服务异常；保留当前合预算出口复查，未反复切换".into();
                (current.clone(), false)
            } else if ranked {
                if verified_fault
                    && verified.iter().any(|name| {
                        s.independent_servers(name, &current)
                            && s.nodes.get(name).is_some_and(|node| node.usable(time))
                    })
                {
                    verified.retain(|name| s.independent_servers(name, &current));
                }
                let initial_choice = s.ranked_run != run && verified.contains(&current);
                let reselect = s.reselect || initial_choice;
                let candidate = if reselect || verified_fault || !current_eligible {
                    choose_ranked(&s.nodes, &s.ranking, &current, reselect, time, &verified)
                } else {
                    current.clone()
                };
                let choice_verified = verified.contains(&candidate)
                    && s.nodes.get(&candidate).is_some_and(|n| n.usable(time));
                if double_failure && candidate == current {
                    s.snapshot.message =
                        "当前节点探测异常，等待备用节点核验，暂时保留当前出口".into();
                }
                (candidate, choice_verified)
            } else {
                (
                    choose_with_costs(&s.nodes, &current, &allowed, time, &costs),
                    false,
                )
            }
        };
        if candidate == current {
            if let Ok(mut s) = self.inner.lock() {
                if ranked_choice_verified {
                    s.ranked_run = run;
                    s.reselect = false;
                }
                if candidate == "REJECT" {
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
        crate::node_selection::validate_choice(&live["proxies"][GROUP], &candidate)?;
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
        if ranked_choice_verified {
            s.ranked_run = run;
            s.reselect = false;
        }
        s.snapshot.message = if candidate == "REJECT" {
            if costs.value_mode() {
                "没有符合预算的健康节点，拒绝后续新请求；未使用超预算或未获允许的未知倍率节点"
            } else {
                "没有可验证的健康候选，拒绝后续新请求；未主动关闭现有连接"
            }
        } else if ranked {
            "已按最新权重为后续新连接选择合格出口，保留已有连接"
        } else if costs.value_mode() {
            "已按健康状态与倍率更新后续新连接出口；未主动关闭现有连接"
        } else {
            "原节点持续失败，已为后续新连接切换出口；原有连接未被主动关闭"
        }
        .into();
        s.ranked_poll = ranked && candidate != "REJECT";
        s.last_ranked_confirmation = None;
        s.current = Some(candidate);
        s.snapshot.last_switch = Some(now());
        Ok(())
    }
}

fn choose_ranked(
    nodes: &BTreeMap<String, NodeHealth>,
    ranking: &[RankedNode],
    current: &str,
    reselect: bool,
    time: u64,
    verified: &[String],
) -> String {
    let current_eligible = ranking.iter().any(|node| node.name == current);
    let keep_current = current_eligible
        && nodes.get(current).is_some_and(|node| {
            node.consecutive_failures < 2
                && node.cooldown_until <= time
                && (node.cooldown_until == 0 || node.recovery_passes >= 3)
        });
    if keep_current && !reselect {
        return current.into();
    }
    ranking
        .iter()
        .find(|node| {
            verified.contains(&node.name)
                && nodes
                    .get(&node.name)
                    .is_some_and(|health| health.usable(time))
        })
        .map(|node| node.name.clone())
        .unwrap_or_else(|| {
            if current_eligible {
                current.into()
            } else {
                "REJECT".into()
            }
        })
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
        if !crate::capabilities::ENHANCED_STABILITY {
            return Err(AppError::Conflict(
                "增强稳定灾备暂时停用；基础 Fallback 保持可用".into(),
            ));
        }
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
    fn ranked_selection_is_sticky_and_skips_cooling_or_unverified_nodes() {
        let costs = ResolvedCosts {
            preferences: Default::default(),
            multipliers: [
                ("low-cost".into(), Some(1.0)),
                ("costly".into(), Some(5.0)),
                ("unverified".into(), Some(0.2)),
            ]
            .into(),
        };
        let baseline = |name: &str| OpenAiNodeScore {
            name: name.into(),
            latency_ms: 300,
            jitter_ms: 15,
            bandwidth_mbps: None,
            connection_samples: 3,
            connection_successes: 3,
            quality_score: None,
            score: 75.0,
            checked_at: chrono::Utc::now(),
        };
        let policy = OpenAiPolicy {
            selected_nodes: vec![
                baseline("low-cost"),
                baseline("costly"),
                baseline("unverified"),
            ],
            ..Default::default()
        };
        let mut healthy = NodeHealth::default();
        healthy.record(100, Evidence::Probe(true));
        let mut nodes = BTreeMap::from([
            ("low-cost".into(), healthy.clone()),
            ("costly".into(), healthy),
            ("unverified".into(), NodeHealth::default()),
        ]);
        let allowed = ["low-cost".into(), "costly".into(), "unverified".into()];
        let verified = ["low-cost".into(), "costly".into()];
        let ranking = rank_nodes(&policy, &nodes, &allowed, &costs, 100);
        assert_eq!(ranking[0].name, "unverified");
        assert_eq!(
            choose_ranked(&nodes, &ranking, "REJECT", true, 100, &verified),
            "low-cost"
        );
        assert_eq!(
            choose_ranked(&nodes, &ranking, "costly", false, 100, &verified),
            "costly"
        );
        assert_eq!(
            choose_ranked(&nodes, &ranking, "costly", true, 100, &verified),
            "low-cost"
        );
        let low_cost = nodes.get_mut("low-cost").unwrap();
        low_cost.record(110, Evidence::Probe(false));
        assert_eq!(
            choose_ranked(&nodes, &ranking, "low-cost", false, 110, &verified),
            "low-cost"
        );
        nodes
            .get_mut("low-cost")
            .unwrap()
            .record(120, Evidence::Probe(false));
        assert_eq!(
            choose_ranked(&nodes, &ranking, "low-cost", false, 120, &verified),
            "costly"
        );
        assert_eq!(
            choose_ranked(&nodes, &ranking, "low-cost", false, 120, &[]),
            "low-cost",
            "a previous pass is insufficient to switch during ambiguous failure"
        );
    }

    #[test]
    fn standby_can_share_a_host_but_a_renamed_alias_is_not_corroboration() {
        let source = r#"
proxies:
  - {name: main, type: socks5, server: sample.invalid, port: 1080}
  - {name: alias, type: socks5, server: sample.invalid, port: 1080}
  - {name: same, type: socks5, server: SAMPLE.invalid., port: 1081}
  - {name: separate, type: socks5, server: other.invalid, port: 1080}
"#;
        let mut state = HealthState::default();
        state.reconcile(
            Some(Uuid::new_v4()),
            Some(Uuid::new_v4()),
            node_identities(source).unwrap(),
        );
        assert!(!state.distinct_nodes("main", "alias"));
        assert!(state.distinct_nodes("main", "same"));
        assert!(!state.independent_servers("main", "same"));
        assert!(state.independent_servers("main", "separate"));
    }

    #[tokio::test]
    async fn ranked_steady_state_only_checks_the_active_target() {
        let calls = Mutex::new(Vec::new());
        let probe = |name, target| {
            calls.lock().unwrap().push((name, target));
            std::future::ready(report(ProbeState::Passed))
        };
        let standby = ["backup".into()];
        let round = ranked_round(&probe, Some("active"), &standby, false, true).await;
        assert!(!round.checked_confirmation);
        assert_eq!(
            calls.into_inner().unwrap(),
            [("active".into(), Target::Chatgpt)]
        );
    }

    #[tokio::test]
    async fn ranked_periodic_sweep_limits_backup_traffic() {
        let calls = Mutex::new(Vec::new());
        let probe = |name, target| {
            calls.lock().unwrap().push((name, target));
            std::future::ready(report(ProbeState::Passed))
        };
        let standby = ["b".into(), "c".into(), "d".into(), "e".into()];
        let round = ranked_round(&probe, Some("a"), &standby, true, true).await;
        assert!(!round.checked_confirmation);
        let calls = calls.into_inner().unwrap();
        assert_eq!(calls.len(), (1 + STANDBY_CONCURRENCY) * 2);
        assert!(!calls.iter().any(|(name, _)| name == "e"));
    }

    #[tokio::test]
    async fn ranked_failure_confirmation_and_standby_check_overlap() {
        use std::sync::atomic::AtomicUsize;
        let initial = AtomicUsize::new(0);
        let concurrent = tokio::sync::Barrier::new(2);
        let probe = |name: String, target| {
            let initial = &initial;
            let concurrent = &concurrent;
            async move {
                assert_eq!(target, Target::Chatgpt);
                if name == "active" {
                    if initial.fetch_add(1, Ordering::Relaxed) == 1 {
                        concurrent.wait().await;
                    }
                    report(ProbeState::Failed)
                } else {
                    concurrent.wait().await;
                    report(ProbeState::Passed)
                }
            }
        };
        let standby = ["backup".into()];
        let round = tokio::time::timeout(
            Duration::from_secs(2),
            ranked_round(&probe, Some("active"), &standby, false, true),
        )
        .await
        .expect("confirmation and standby probe run in parallel");
        assert!(round.confirmed_failure && round.checked_confirmation);
        assert_eq!(initial.load(Ordering::Relaxed), 2);
        assert_eq!(round.checks[0].0, "backup");
        assert_eq!(round.checks[0].1.state, ProbeState::Passed);
    }

    #[tokio::test]
    async fn ranked_unknown_never_starts_confirmation() {
        for first in [ProbeState::Unknown, ProbeState::HttpUnverified] {
            let probe = |name: String, _| {
                assert_eq!(name, "active");
                std::future::ready(report(first))
            };
            let round = ranked_round(&probe, Some("active"), &["backup".into()], false, true).await;
            assert!(!round.confirmed_failure && !round.checked_confirmation);
            assert_eq!(round.checks[0].1.state, first);
        }
    }

    #[test]
    fn ranked_steady_state_control_plane_budget_fits_30_seconds() {
        let previous_periodic_round = CONTROLLER_BUDGET * 2 + SWEEP_BUDGET;
        let detection_wait = ACTIVE_INTERVAL.max(previous_periodic_round + RANKED_MIN_GAP);
        let new_round = SWEEP_BUDGET * 2 + CONTROLLER_BUDGET * 3;
        assert!(RANKED_REQUEST_BUDGET * 2 <= SWEEP_BUDGET);
        assert!(detection_wait + new_round <= Duration::from_secs(30));
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
        assert!(
            manager.observe(Target::Chatgpt).is_none(),
            "feature gate blocks new observations"
        );
        let valid = Observation {
            manager: manager.clone(),
            node: "a".into(),
            epoch: manager.inner.lock().unwrap().epoch,
        };
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
        let stale = Observation {
            manager: manager.clone(),
            node: "a".into(),
            epoch: manager.inner.lock().unwrap().epoch,
        };
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
