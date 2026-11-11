//! Read-only network epochs. Raw routes, addresses and Tailscale identities stay
//! in this worker; IPC receives only a random revision and coarse observations.
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Seek},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime},
};
use tauri::{AppHandle, Emitter, Manager};
use uuid::Uuid;

const INTERVAL: Duration = Duration::from_secs(6);
const MAX_AGE: Duration = Duration::from_secs(20);
const COMMAND_BUDGET: Duration = Duration::from_millis(1200);
const OUTPUT_LIMIT: u64 = 1024 * 1024;

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TailscaleState {
    Unknown,
    Stopped,
    Running,
    ExitNode,
    ExitNodeOffline,
    ExitNodePending,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub revision: Uuid,
    pub generation: u64,
    pub observed: bool,
    pub stale: bool,
    pub tailscale: TailscaleState,
    pub route_known: bool,
    pub dns_known: bool,
    pub policy: &'static str,
    pub independent_egress_available: bool,
    pub tun_coexistence_verified: bool,
    pub detail: String,
}
impl Default for Snapshot {
    fn default() -> Self {
        Self {
            revision: Uuid::new_v4(),
            generation: 0,
            observed: false,
            stale: true,
            tailscale: TailscaleState::Unknown,
            route_known: false,
            dns_known: false,
            policy: "system",
            independent_egress_available: false,
            tun_coexistence_verified: false,
            detail: "正在识别网络环境；保持系统出口，不修改 Tailscale。".into(),
        }
    }
}
#[derive(Default)]
struct State {
    snapshot: Snapshot,
    digest: Option<Vec<u8>>,
    checked: Option<SystemTime>,
}
impl State {
    fn apply(&mut self, observation: Observation, now: SystemTime) -> bool {
        let resumed = self
            .checked
            .is_some_and(|last| now.duration_since(last).map_or(true, |d| d > MAX_AGE));
        let changed = self.digest.as_ref() != Some(&observation.digest) || resumed;
        if changed {
            self.snapshot.revision = Uuid::new_v4();
            self.snapshot.generation += 1;
        }
        self.snapshot.observed = true;
        self.snapshot.stale = false;
        self.snapshot.tailscale = observation.tailscale;
        self.snapshot.route_known = observation.route_known;
        self.snapshot.dns_known = observation.dns_known;
        self.snapshot.detail = describe(&self.snapshot);
        self.digest = Some(observation.digest);
        self.checked = Some(now);
        changed
    }
    fn snapshot(&self) -> Snapshot {
        let mut s = self.snapshot.clone();
        s.stale = self.checked.is_none_or(|t| {
            SystemTime::now()
                .duration_since(t)
                .map_or(true, |d| d > MAX_AGE)
        });
        if s.stale {
            s.detail = "网络环境待重新识别，旧测速暂不采用；保持原出口与代理设置。".into();
        }
        s
    }
}
#[derive(Default)]
pub struct NetworkContext {
    state: Arc<Mutex<State>>,
    refresh: Arc<tokio::sync::Mutex<()>>,
}
impl NetworkContext {
    pub fn snapshot(&self) -> Snapshot {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .snapshot()
    }
    pub async fn refresh(&self) -> Snapshot {
        // Concurrent callers share the same physical observation, never queue a
        // chain of process probes. Cached observation is at most two seconds old.
        let lock = self.refresh.clone().lock_owned().await;
        if self
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .checked
            .is_some_and(|t| t.elapsed().is_ok_and(|d| d < Duration::from_secs(2)))
        {
            return self.snapshot();
        }
        if !crate::lifecycle::PROCESS_LIFECYCLE.admitting() {
            return self.snapshot();
        }
        let state = self.state.clone();
        let _ = tauri::async_runtime::spawn_blocking(move || {
            let _lock = lock;
            let observation = observe();
            state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .apply(observation, SystemTime::now());
        })
        .await;
        self.snapshot()
    }
}
pub fn revision(app: &AppHandle) -> Option<Uuid> {
    let s = app.state::<NetworkContext>().snapshot();
    (s.observed && !s.stale).then_some(s.revision)
}
pub async fn refresh(app: &AppHandle) -> Snapshot {
    let before = app.state::<NetworkContext>().snapshot().revision;
    let s = app.state::<NetworkContext>().refresh().await;
    if before != s.revision {
        crate::node_test::invalidate_network(app);
        let _ = app.emit("network-context-changed", &s);
    }
    s
}
pub fn start(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        loop {
            if crate::lifecycle::PROCESS_LIFECYCLE.snapshot().phase == "closing" {
                break;
            }
            if crate::lifecycle::PROCESS_LIFECYCLE.admitting() {
                refresh(&app).await;
            }
            tokio::time::sleep(INTERVAL).await;
        }
    });
}
#[tauri::command]
pub async fn network_context_status(app: AppHandle) -> Result<Snapshot, crate::error::AppErrorDto> {
    crate::lifecycle::PROCESS_LIFECYCLE
        .ticket()
        .map_err(|e| crate::error::AppError::Conflict(e.to_string()).dto())?;
    Ok(refresh(&app).await)
}
fn describe(s: &Snapshot) -> String {
    let state = match s.tailscale {
        TailscaleState::ExitNode => "Tailscale 已选择出口节点；代理节点拨号默认跟随系统出口。",
        TailscaleState::ExitNodeOffline => {
            "Tailscale 所选出口节点离线；先检查出口节点状态，未自动绕开它。"
        }
        TailscaleState::ExitNodePending => {
            "已配置 Tailscale 出口节点，但当前状态未确认生效；保持原路径，未自动直连。"
        }
        TailscaleState::Running => "Tailscale 正在运行，当前未使用出口节点。",
        TailscaleState::Stopped => "Tailscale 当前未运行。",
        TailscaleState::Unknown => {
            "Tailscale 状态未确认（命令缺失、无权限或读取超时），不等于已关闭。"
        }
    };
    format!(
        "{state}{} 保留订阅中的显式拨号设置；独立出口和 TUN 共存自动配置尚未通过平台实测，未启用。",
        if s.route_known && s.dns_known {
            ""
        } else {
            " 部分路由或 DNS 状态未读到，诊断信息不完整。"
        }
    )
}
struct Observation {
    digest: Vec<u8>,
    tailscale: TailscaleState,
    route_known: bool,
    dns_known: bool,
}
fn tailscale_view(status: Option<&str>, prefs: Option<&str>) -> (TailscaleState, Value) {
    let status: Value = status
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or(Value::Null);
    let prefs: Value = prefs
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or(Value::Null);
    let state = match status["BackendState"].as_str() {
        Some("Running") if status["ExitNodeStatus"].is_object() => {
            if status["ExitNodeStatus"]["Online"].as_bool() == Some(false) {
                TailscaleState::ExitNodeOffline
            } else {
                TailscaleState::ExitNode
            }
        }
        Some("Running")
            if ["ExitNodeID", "ExitNodeIP", "AutoExitNode"]
                .iter()
                .any(|key| prefs[*key].as_str().is_some_and(|v| !v.is_empty())) =>
        {
            TailscaleState::ExitNodePending
        }
        Some("Running") => TailscaleState::Running,
        Some("Stopped" | "NeedsLogin" | "NeedsMachineAuth") => TailscaleState::Stopped,
        _ => TailscaleState::Unknown,
    };
    // Do not hash peers' packet counters, handshake times, health text, auth URLs
    // or user identities. Those vary without a change to this device's path.
    (
        state,
        json!({"state":status["BackendState"],"ips":status["TailscaleIPs"],
        "exit":status["ExitNodeStatus"], "dnsSuffix":status["CurrentTailnet"]["MagicDNSSuffix"],
        "prefs":{"known":prefs.is_object(),"dns":prefs["CorpDNS"],"routes":prefs["RouteAll"],
        "exit":prefs["ExitNodeID"],"exitIp":prefs["ExitNodeIP"],"autoExit":prefs["AutoExitNode"],
        "lan":prefs["ExitNodeAllowLANAccess"],"running":prefs["WantRunning"]}}),
    )
}
thread_local! { static DEADLINE: std::cell::Cell<Option<Instant>> = const { std::cell::Cell::new(None) }; }
struct ObservationBudget;
impl Drop for ObservationBudget {
    fn drop(&mut self) {
        DEADLINE.with(|d| d.set(None));
    }
}
fn observe() -> Observation {
    DEADLINE.with(|d| d.set(Some(Instant::now() + Duration::from_secs(3))));
    let _budget = ObservationBudget;
    let (routes, dns) = platform_observation();
    let (status, prefs) = if let Some(cli) = tailscale_cli() {
        (
            read_command(&cli, &["status", "--json"], COMMAND_BUDGET),
            read_command(&cli, &["debug", "prefs"], COMMAND_BUDGET),
        )
    } else {
        (None, None)
    };
    let (tailscale, ts) = tailscale_view(status.as_deref(), prefs.as_deref());
    let networks = sysinfo::Networks::new_with_refreshed_list();
    let mut interfaces: Vec<_> = networks
        .iter()
        .map(|(name, net)| {
            let mut addresses: Vec<_> = net.ip_networks().iter().map(ToString::to_string).collect();
            addresses.sort();
            format!("{name}:{addresses:?}")
        })
        .collect();
    interfaces.sort();
    let value = json!({"routes":routes,"dns":dns,"tailscale":ts,"interfaces":interfaces});
    Observation {
        digest: Sha256::digest(value.to_string().as_bytes()).to_vec(),
        tailscale,
        route_known: routes.is_some(),
        dns_known: dns.is_some(),
    }
}
fn tailscale_cli() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    let paths = vec![
        PathBuf::from("/Applications/Tailscale.app/Contents/MacOS/Tailscale"),
        PathBuf::from("/opt/homebrew/bin/tailscale"),
        PathBuf::from("/usr/local/bin/tailscale"),
    ];
    #[cfg(target_os = "linux")]
    let paths = vec![
        PathBuf::from("/usr/bin/tailscale"),
        PathBuf::from("/usr/local/bin/tailscale"),
    ];
    #[cfg(windows)]
    let paths = vec![PathBuf::from(
        std::env::var_os("ProgramFiles").unwrap_or_else(|| r"C:\Program Files".into()),
    )
    .join("Tailscale/tailscale.exe")];
    #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
    let paths: Vec<PathBuf> = vec![];
    paths.into_iter().find(|p| p.is_file())
}
fn platform_observation() -> (Option<String>, Option<String>) {
    #[cfg(target_os = "macos")]
    {
        let nwi = read_command(Path::new("/usr/sbin/scutil"), &["--nwi"], COMMAND_BUDGET);
        let route = |family| {
            read_command(
                Path::new("/sbin/route"),
                &["-n", "get", family, "default"],
                COMMAND_BUDGET,
            )
            .map(|s| {
                s.lines()
                    .map(str::trim)
                    .filter(|l| {
                        l.starts_with("gateway:")
                            || l.starts_with("interface:")
                            || l.starts_with("flags:")
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            })
        };
        let v4 = route("-inet");
        let v6 = route("-inet6");
        (
            nwi.filter(|_| v4.is_some() || v6.is_some())
                .map(|n| json!({"nwi":n,"v4":v4,"v6":v6}).to_string()),
            read_command(Path::new("/usr/sbin/scutil"), &["--dns"], COMMAND_BUDGET),
        )
    }
    #[cfg(target_os = "linux")]
    {
        let ip = ["/usr/sbin/ip", "/usr/bin/ip", "/sbin/ip"]
            .into_iter()
            .find(|p| Path::new(p).is_file());
        let routes = ip.and_then(|p| {
            let read = |args: &[&str]| {
                read_command(Path::new(p), args, COMMAND_BUDGET)
                    .and_then(|s| serde_json::from_str::<Value>(&s).ok())
            };
            let mut v4 = read(&["-j", "-4", "route", "show", "table", "all"])?;
            let mut v6 = read(&["-j", "-6", "route", "show", "table", "all"])?;
            let rules = read(&["-j", "rule", "show"])?;
            for v in [&mut v4, &mut v6] {
                if let Some(rows) = v.as_array_mut() {
                    for row in rows.iter_mut() {
                        if let Some(o) = row.as_object_mut() {
                            o.remove("expires");
                            o.remove("cache");
                        }
                    }
                    rows.sort_by_key(Value::to_string);
                }
            }
            Some(json!({"v4":v4,"v6":v6,"rules":rules}).to_string())
        });
        let dns = std::fs::File::open("/etc/resolv.conf").ok().and_then(|f| {
            let mut s = String::new();
            f.take(65536).read_to_string(&mut s).ok().map(|_| s)
        });
        (routes, dns)
    }
    #[cfg(windows)]
    {
        let system = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
        let shell = PathBuf::from(system).join("System32/WindowsPowerShell/v1.0/powershell.exe");
        let output = read_command(&shell, &["-NoProfile", "-NonInteractive", "-Command",
            "$ErrorActionPreference='Stop'; [Console]::OutputEncoding=[System.Text.Encoding]::UTF8; @{routes=@(Get-NetRoute | Where-Object {$_.DestinationPrefix -in @('0.0.0.0/0','::/0') -or $_.InterfaceAlias -like '*Tailscale*'} | Sort-Object InterfaceIndex,DestinationPrefix,NextHop | Select-Object InterfaceIndex,DestinationPrefix,NextHop,RouteMetric); dns=@(Get-DnsClientServerAddress | Sort-Object InterfaceIndex,AddressFamily | Select-Object InterfaceIndex,AddressFamily,ServerAddresses)} | ConvertTo-Json -Depth 5 -Compress"], Duration::from_secs(3))
            .and_then(|s| serde_json::from_str::<Value>(s.trim_start_matches('\u{feff}')).ok());
        (
            output.as_ref().map(|v| v["routes"].to_string()),
            output.as_ref().map(|v| v["dns"].to_string()),
        )
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
    {
        (None, None)
    }
}
// Fixed readonly commands only. Private temporary files avoid pipe deadlock;
// output is bounded, never logged, and every timeout kills/reaps the exact child.
fn read_command(program: &Path, args: &[&str], budget: Duration) -> Option<String> {
    let budget = DEADLINE.with(|d| d.get()).map_or(budget, |end| {
        budget.min(end.saturating_duration_since(Instant::now()))
    });
    if budget.is_zero() {
        return None;
    }
    let ticket = crate::lifecycle::PROCESS_LIFECYCLE.ticket().ok()?;
    let mut out = tempfile::tempfile().ok()?;
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(out.try_clone().ok()?)
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
    }
    let mut child = {
        let _guard =
            crate::lifecycle::admission(crate::lifecycle::Purpose::NetworkObservation).ok()?;
        command.spawn().ok()?
    };
    let until = Instant::now() + budget;
    let success = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s.success(),
            Ok(None)
                if Instant::now() < until
                    && crate::lifecycle::PROCESS_LIFECYCLE.check(ticket).is_ok()
                    && out.metadata().is_ok_and(|m| m.len() <= OUTPUT_LIMIT) =>
            {
                std::thread::sleep(Duration::from_millis(10))
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    if !success || out.metadata().ok()?.len() > OUTPUT_LIMIT {
        return None;
    }
    out.rewind().ok()?;
    let mut s = String::new();
    out.take(OUTPUT_LIMIT).read_to_string(&mut s).ok()?;
    Some(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sample(key: u8) -> Observation {
        Observation {
            digest: vec![key],
            tailscale: TailscaleState::Running,
            route_known: true,
            dns_known: true,
        }
    }
    #[test]
    fn epochs_change_for_topology_failure_recovery_and_wake_not_for_refresh() {
        let mut s = State::default();
        let now = SystemTime::now();
        assert!(s.apply(sample(1), now));
        let rev = s.snapshot.revision;
        assert!(!s.apply(sample(1), now + Duration::from_secs(3)));
        assert_eq!(rev, s.snapshot.revision);
        assert!(s.apply(sample(2), now + Duration::from_secs(4)));
        assert_ne!(rev, s.snapshot.revision);
        assert!(s.apply(sample(1), now + Duration::from_secs(5)));
        assert!(s.apply(sample(1), now + Duration::from_secs(60)));
    }
    #[test]
    fn absent_or_malformed_cli_is_unknown_not_disabled() {
        for data in [
            None,
            Some("bad"),
            Some("{}"),
            Some(r#"{"BackendState":"Starting"}"#),
        ] {
            assert_eq!(tailscale_view(data, None).0, TailscaleState::Unknown);
        }
        assert_eq!(
            tailscale_view(Some(r#"{"BackendState":"Stopped"}"#), None).0,
            TailscaleState::Stopped
        );
    }
    #[test]
    fn exit_node_and_offline_are_distinct_and_no_peer_counters_are_hashed() {
        let a = r#"{"BackendState":"Running","ExitNodeStatus":{"ID":"fixture","Online":true},"Peer":{"other":{"TxBytes":1}},"AuthURL":"secret"}"#;
        let b = a.replace("\"TxBytes\":1", "\"TxBytes\":999");
        assert_eq!(tailscale_view(Some(a), None).0, TailscaleState::ExitNode);
        assert_eq!(
            tailscale_view(Some(a), None).1,
            tailscale_view(Some(&b), None).1
        );
        assert!(!tailscale_view(Some(a), None)
            .1
            .to_string()
            .contains("secret"));
        assert_eq!(
            tailscale_view(Some(&a.replace("true", "false")), None).0,
            TailscaleState::ExitNodeOffline
        );
        assert_ne!(
            tailscale_view(Some(a), Some(r#"{"CorpDNS":true}"#)).1,
            tailscale_view(Some(a), Some(r#"{"CorpDNS":false}"#)).1
        );
    }
    #[test]
    fn ipc_has_no_addresses_and_unverified_policies_stay_closed() {
        let s = serde_json::to_value(Snapshot::default()).unwrap();
        assert_eq!(s["policy"], "system");
        assert_eq!(s["independentEgressAvailable"], false);
        assert_eq!(s["tunCoexistenceVerified"], false);
        assert!(s.get("digest").is_none());
    }
    #[cfg(unix)]
    #[test]
    fn commands_are_bounded_and_reaped() {
        let t = Instant::now();
        assert!(read_command(Path::new("/bin/sleep"), &["2"], Duration::from_millis(30)).is_none());
        assert!(t.elapsed() < Duration::from_secs(1));
        assert_eq!(
            read_command(Path::new("/bin/echo"), &["fixture"], Duration::from_secs(1)).unwrap(),
            "fixture\n"
        );
    }
}
