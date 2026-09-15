use crate::error::{AppError, AppResult};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::process::Command;
use tauri::{AppHandle, Manager};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyProtocolState {
    pub enabled: bool,
    pub server: String,
    pub port: u16,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MacServiceProxyState {
    pub service: String,
    pub http: ProxyProtocolState,
    pub https: ProxyProtocolState,
    pub socks: ProxyProtocolState,
    pub bypass_domains: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "platform", rename_all = "snake_case")]
pub enum SystemProxySnapshot {
    Macos {
        services: Vec<MacServiceProxyState>,
    },
    Linux {
        values: BTreeMap<String, String>,
    },
    Windows {
        proxy_enable: u32,
        proxy_server: Option<String>,
        proxy_override: Option<String>,
        auto_config_url: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemProxyStatus {
    pub active: bool,
    pub snapshot_path: Option<String>,
    pub platform: String,
}

/// A read-only baseline for automatic recovery. Unlike an explicit Start click,
/// recovery may not take over another client's proxy or a newly chosen PAC.
pub(crate) struct ResumeProxyGuard {
    baseline: SystemProxySnapshot,
}

pub(crate) fn prepare_automatic_proxy_resume(app: &AppHandle) -> AppResult<ResumeProxyGuard> {
    let baseline = capture_system_proxy()?;
    #[cfg(windows)]
    let owned = {
        let path = snapshot_path(app)?;
        if path.exists() {
            let saved: WindowsProxyLease = serde_json::from_slice(&fs::read(&path)?)
                .map_err(|error| AppError::Platform(error.to_string()))?;
            let port = crate::storage::AppStorage::from_app(app)?
                .settings()?
                .mixed_port;
            // Ownership and availability must describe the same observation.
            // status(app) would capture again and can refer to a different client.
            saved.owns(&baseline, port)
        } else {
            false
        }
    };
    #[cfg(not(windows))]
    let owned = {
        let _ = app;
        // Non-Windows legacy snapshots do not track ownership. A snapshot file
        // alone is not sufficient authority to overwrite an active proxy.
        false
    };
    if !resume_proxy_available(&baseline, owned) {
        return Err(AppError::startup(
            crate::error::StartupReason::ProxyChanged,
            "系统代理或 PAC 已由其他配置占用，自动恢复不会覆盖它",
            None,
        ));
    }
    #[cfg(target_os = "macos")]
    if let SystemProxySnapshot::Macos { services } = &baseline {
        for service in services {
            for query in ["-getautoproxyurl", "-getproxyautodiscovery"] {
                let value = run_command("networksetup", &[query, &service.service])?;
                if value.lines().any(|line| {
                    line.trim().eq_ignore_ascii_case("Enabled: Yes")
                        || line.trim().eq_ignore_ascii_case("Auto Proxy Discovery: On")
                }) {
                    return Err(AppError::startup(
                        crate::error::StartupReason::ProxyChanged,
                        "系统已启用 PAC 或自动代理发现，自动恢复不会覆盖它",
                        None,
                    ));
                }
            }
        }
    }
    Ok(ResumeProxyGuard { baseline })
}

pub(crate) fn enable_automatic_system_proxy(
    app: &AppHandle,
    port: u16,
    guard: &ResumeProxyGuard,
) -> AppResult<SystemProxyStatus> {
    // Re-read immediately before applying, after the asynchronous core and
    // connectivity checks. If ownership changed meanwhile, leave it untouched.
    let current = prepare_automatic_proxy_resume(app)?;
    if current.baseline != guard.baseline {
        return Err(AppError::startup(
            crate::error::StartupReason::ProxyChanged,
            "启动检查期间系统代理设置已变化，已取消自动接管",
            None,
        ));
    }
    let mut mutation_started = false;
    let result =
        enable_system_proxy_guarded(app, port, Some(&guard.baseline), &mut mutation_started);
    if result.is_err() && mutation_started {
        // A baseline refusal performed no write and must never restore a stale
        // snapshot over the client whose new configuration caused that refusal.
        let _ = restore_system_proxy(app);
    }
    result
}

fn resume_proxy_available(snapshot: &SystemProxySnapshot, owned: bool) -> bool {
    if owned {
        return true;
    }
    match snapshot {
        SystemProxySnapshot::Windows {
            proxy_enable,
            auto_config_url,
            ..
        } => {
            *proxy_enable == 0
                && auto_config_url
                    .as_deref()
                    .is_none_or(|value| value.trim().is_empty())
        }
        SystemProxySnapshot::Macos { services } => services.iter().all(|service| {
            !service.http.enabled && !service.https.enabled && !service.socks.enabled
        }),
        SystemProxySnapshot::Linux { values } => values
            .get("org.gnome.system.proxy|mode")
            .is_some_and(|mode| mode.trim().trim_matches('\'') == "none"),
    }
}

#[cfg(target_os = "macos")]
#[derive(Debug, Clone, PartialEq, Eq)]
struct MacNetworkServiceEntry {
    service: String,
    device: String,
    enabled: bool,
}

pub fn enable_system_proxy(app: &AppHandle, port: u16) -> AppResult<SystemProxyStatus> {
    enable_system_proxy_guarded(app, port, None, &mut false)
}

fn check_resume_baseline(
    current: &SystemProxySnapshot,
    expected: Option<&SystemProxySnapshot>,
) -> AppResult<()> {
    if expected.is_some_and(|value| current != value) {
        return Err(AppError::startup(
            crate::error::StartupReason::ProxyChanged,
            "接入前系统代理设置已变化，已取消自动接管",
            None,
        ));
    }
    Ok(())
}

fn enable_system_proxy_guarded(
    app: &AppHandle,
    port: u16,
    expected: Option<&SystemProxySnapshot>,
    mutation_started: &mut bool,
) -> AppResult<SystemProxyStatus> {
    #[cfg(windows)]
    {
        let path = snapshot_path(app)?;
        let current = capture_system_proxy()?;
        // Compare the exact capture used to construct the new lease. Do not
        // validate one observation and then unconditionally accept a newer one.
        check_resume_baseline(&current, expected)?;
        let original = if path.exists() {
            let saved: WindowsProxyLease = serde_json::from_slice(&fs::read(&path)?)
                .map_err(|error| AppError::Platform(error.to_string()))?;
            if saved.owns(&current, port) {
                saved.original().clone()
            } else {
                current.clone()
            }
        } else {
            current.clone()
        };
        let applied = windows_applied_proxy(&current, port);
        // Save both sides before changing the registry, including interrupted writes.
        let mut lease = WindowsProxyLease::Tracked {
            original,
            before: Some(current),
            applied: applied.clone(),
            committed: false,
        };
        write_snapshot(&path, &lease)?;
        *mutation_started = true;
        restore_snapshot(&applied)?;
        if let WindowsProxyLease::Tracked { committed, .. } = &mut lease {
            *committed = true;
        }
        write_snapshot(&path, &lease)?;
        Ok(status(app))
    }
    #[cfg(not(windows))]
    {
        let snapshot_path = snapshot_path(app)?;
        if expected.is_some() || !snapshot_path.exists() {
            let snapshot = capture_system_proxy()?;
            check_resume_baseline(&snapshot, expected)?;
            write_snapshot(&snapshot_path, &snapshot)?;
        }
        *mutation_started = true;
        apply_system_proxy(port)?;
        Ok(status(app))
    }
}

pub fn verify_system_proxy(port: u16) -> AppResult<()> {
    verify_system_proxy_inner(port)
}

pub fn restore_system_proxy(app: &AppHandle) -> AppResult<SystemProxyStatus> {
    let path = snapshot_path(app)?;
    if !path.exists() {
        return Ok(status(app));
    }
    #[cfg(not(windows))]
    {
        let snapshot: SystemProxySnapshot = serde_json::from_slice(&fs::read(&path)?)
            .map_err(|error| AppError::Platform(error.to_string()))?;
        restore_snapshot(&snapshot)?;
    }
    #[cfg(windows)]
    {
        let saved: WindowsProxyLease = serde_json::from_slice(&fs::read(&path)?)
            .map_err(|error| AppError::Platform(error.to_string()))?;
        let current = capture_system_proxy()?;
        let port = crate::storage::AppStorage::from_app(app)?
            .settings()?
            .mixed_port;
        if let Some(original) = saved.restoration(&current, port) {
            // Recovery itself can be interrupted. Persist its transition before
            // the first write so a later launch can finish either direction.
            let restoring = WindowsProxyLease::Tracked {
                original: original.clone(),
                before: Some(current),
                applied: original.clone(),
                committed: false,
            };
            write_snapshot(&path, &restoring)?;
            restore_snapshot(&original)?;
        }
        // Another client may have taken over. Discard our stale lease without
        // overwriting the user's newer proxy selection.
    }
    fs::remove_file(path)?;
    Ok(status(app))
}

pub fn status(app: &AppHandle) -> SystemProxyStatus {
    let path = snapshot_path(app).ok();
    #[cfg(not(windows))]
    let active = path.as_ref().is_some_and(|value| value.exists())
        && (|| -> AppResult<bool> {
            let port = crate::storage::AppStorage::from_app(app)?
                .settings()?
                .mixed_port;
            Ok(proxy_matches_port(&capture_system_proxy()?, port))
        })()
        .unwrap_or(false);
    #[cfg(windows)]
    let active = (|| -> Option<bool> {
        let saved: WindowsProxyLease =
            serde_json::from_slice(&fs::read(path.as_ref()?).ok()?).ok()?;
        let current = capture_system_proxy().ok()?;
        let port = crate::storage::AppStorage::from_app(app)
            .ok()?
            .settings()
            .ok()?
            .mixed_port;
        Some(saved.owns(&current, port))
    })()
    .unwrap_or(false);
    SystemProxyStatus {
        active,
        snapshot_path: path.map(|value| value.display().to_string()),
        platform: std::env::consts::OS.to_string(),
    }
}

fn snapshot_path(app: &AppHandle) -> AppResult<PathBuf> {
    let directory = app
        .path()
        .app_data_dir()
        .map_err(|error| AppError::Io(error.to_string()))?
        .join("runtime");
    fs::create_dir_all(&directory)?;
    Ok(directory.join("system-proxy-snapshot.json"))
}

fn write_snapshot(path: &Path, snapshot: &impl Serialize) -> AppResult<()> {
    let bytes = serde_json::to_vec_pretty(snapshot)
        .map_err(|error| AppError::Platform(error.to_string()))?;
    let temporary = path.with_extension("tmp");
    fs::write(&temporary, bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&temporary, fs::Permissions::from_mode(0o600))?;
    }
    fs::rename(&temporary, path)?;
    Ok(())
}

#[cfg(any(windows, test))]
#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
enum WindowsProxyLease {
    Tracked {
        original: SystemProxySnapshot,
        #[serde(default)]
        before: Option<SystemProxySnapshot>,
        applied: SystemProxySnapshot,
        #[serde(default)]
        committed: bool,
    },
    // Read snapshots written by 0.4.0 without losing recovery support.
    Legacy(SystemProxySnapshot),
}

#[cfg(any(windows, test))]
impl WindowsProxyLease {
    fn original(&self) -> &SystemProxySnapshot {
        match self {
            Self::Tracked { original, .. } | Self::Legacy(original) => original,
        }
    }

    fn owns(&self, current: &SystemProxySnapshot, port: u16) -> bool {
        match self {
            Self::Tracked {
                original,
                before,
                applied,
                committed,
            } => {
                if *committed {
                    windows_same_route(current, applied)
                } else {
                    windows_transition_matches(
                        current,
                        before.as_ref().unwrap_or(original),
                        applied,
                    )
                }
            }
            Self::Legacy(original) => {
                // Older builds did not clear PAC. A newly selected PAC is a
                // different owner even when the manual proxy entry is unchanged.
                windows_endpoint_owned(current, port)
                    && matches!((current, original),
                        (SystemProxySnapshot::Windows { auto_config_url: current, .. },
                         SystemProxySnapshot::Windows { auto_config_url: original, .. }) if current == original)
            }
        }
    }

    fn restoration(&self, current: &SystemProxySnapshot, port: u16) -> Option<SystemProxySnapshot> {
        if !self.owns(current, port) {
            return None;
        }
        let mut restored = self.original().clone();
        if let (
            Self::Tracked {
                applied:
                    SystemProxySnapshot::Windows {
                        proxy_override: applied,
                        ..
                    },
                committed: true,
                ..
            },
            SystemProxySnapshot::Windows {
                proxy_override: current,
                ..
            },
            SystemProxySnapshot::Windows {
                proxy_override: restored,
                ..
            },
        ) = (self, current, &mut restored)
        {
            if current != applied {
                *restored = current.clone();
            }
        }
        Some(restored)
    }
}

#[cfg(any(windows, test))]
fn windows_proxy_server(port: u16) -> String {
    // A single HTTP CONNECT endpoint works for HTTPS too. Some clients treat
    // WinINET's protocol-map syntax as one URI and silently fall back to DIRECT.
    format!("127.0.0.1:{port}")
}

#[cfg(any(windows, test))]
fn windows_endpoint_owned(current: &SystemProxySnapshot, port: u16) -> bool {
    matches!(current, SystemProxySnapshot::Windows { proxy_enable: 1, proxy_server: Some(server), .. }
        if server == &windows_proxy_server(port)
            || server == &format!("http=127.0.0.1:{port};https=127.0.0.1:{port};socks=127.0.0.1:{port}")
            || server == &format!("http=127.0.0.1:{port};https=127.0.0.1:{port}"))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyCompatibility {
    pub supported: bool,
    pub system_configured: bool,
    pub compatible: bool,
    pub expected_proxy: String,
    pub resolved_http: Option<String>,
    pub resolved_https: Option<String>,
    pub detail: String,
}

/// Read-only: feed the current Windows proxy address to the real HTTP parser.
/// Do not enable hyper-util's system feature just for diagnostics: Cargo feature
/// unification would silently change the routing of unrelated reqwest clients.
/// This validates the address format, not any other process's effective route.
pub fn proxy_compatibility(port: u16) -> AppResult<ProxyCompatibility> {
    let expected_proxy = format!("http://127.0.0.1:{port}");
    #[cfg(windows)]
    {
        use hyper_util::client::proxy::matcher::Matcher;
        let current = capture_system_proxy()?;
        let system_configured =
            windows_same_route(&current, &windows_applied_proxy(&current, port));
        let server = match &current {
            SystemProxySnapshot::Windows {
                proxy_enable: 1,
                proxy_server: Some(server),
                ..
            } => server.as_str(),
            _ => "",
        };
        let matcher = Matcher::builder().http(server).https(server).build();
        let resolve = |target: &str| {
            let uri = target.parse().ok()?;
            let proxy = matcher.intercept(&uri)?;
            let parsed = url::Url::parse(&proxy.uri().to_string()).ok()?;
            // Never expose proxy credentials, paths or query strings to the UI.
            Some(format!(
                "{}://{}:{}",
                parsed.scheme(),
                parsed.host_str()?,
                parsed.port_or_known_default()?
            ))
        };
        let resolved_http = resolve("http://chatgpt.com/");
        let resolved_https = resolve("https://chatgpt.com/backend-api/codex/responses");
        let compatible = system_configured
            && resolved_http.as_deref() == Some(expected_proxy.as_str())
            && resolved_https.as_deref() == Some(expected_proxy.as_str());
        let detail = if compatible {
            "Windows 代理地址通过 HTTP/HTTPS 格式校验。目标应用仍可能受环境变量或内部代理设置影响；WebSocket 和长连接需实际验证。"
        } else if !system_configured {
            "系统代理未使用 Serylane 的兼容格式，或已由其他程序接管。此检查不会更改设置。"
        } else {
            "Windows 代理地址未通过 HTTP/HTTPS 格式校验；此检查不会修改系统设置。"
        };
        Ok(ProxyCompatibility {
            supported: true,
            system_configured,
            compatible,
            expected_proxy,
            resolved_http,
            resolved_https,
            detail: detail.into(),
        })
    }
    #[cfg(not(windows))]
    Ok(ProxyCompatibility {
        supported: false,
        system_configured: false,
        compatible: false,
        expected_proxy,
        resolved_http: None,
        resolved_https: None,
        detail: "此兼容性检查目前针对 Windows 系统代理。".into(),
    })
}

#[cfg(any(windows, test))]
fn windows_applied_proxy(current: &SystemProxySnapshot, port: u16) -> SystemProxySnapshot {
    let existing = match current {
        SystemProxySnapshot::Windows { proxy_override, .. } => {
            proxy_override.as_deref().unwrap_or("")
        }
        _ => "",
    };
    let mut bypass: Vec<&str> = existing
        .split(';')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .collect();
    for required in ["<local>", "localhost", "127.*", "[::1]"] {
        if !bypass
            .iter()
            .any(|value| value.eq_ignore_ascii_case(required))
        {
            bypass.push(required);
        }
    }
    SystemProxySnapshot::Windows {
        proxy_enable: 1,
        proxy_server: Some(windows_proxy_server(port)),
        proxy_override: Some(bypass.join(";")),
        auto_config_url: None,
    }
}

#[cfg(any(windows, test))]
fn windows_same_route(left: &SystemProxySnapshot, right: &SystemProxySnapshot) -> bool {
    matches!((left, right), (
        SystemProxySnapshot::Windows { proxy_enable: le, proxy_server: ls, auto_config_url: la, .. },
        SystemProxySnapshot::Windows { proxy_enable: re, proxy_server: rs, auto_config_url: ra, .. }
    ) if le == re && ls == rs && la == ra)
}

#[cfg(any(windows, test))]
fn windows_transition_matches(
    current: &SystemProxySnapshot,
    original: &SystemProxySnapshot,
    applied: &SystemProxySnapshot,
) -> bool {
    matches!((current, original, applied), (
        SystemProxySnapshot::Windows { proxy_enable: ce, proxy_server: cs, proxy_override: cb, auto_config_url: ca },
        SystemProxySnapshot::Windows { proxy_enable: oe, proxy_server: os, proxy_override: ob, auto_config_url: oa },
        SystemProxySnapshot::Windows { proxy_enable: ae, proxy_server: as_, proxy_override: ab, auto_config_url: aa }
    ) if (ce == oe || ce == ae) && (cs == os || cs == as_) && (cb == ob || cb == ab) && (ca == oa || ca == aa))
}

#[cfg(target_os = "macos")]
fn capture_system_proxy() -> AppResult<SystemProxySnapshot> {
    let services = mac_active_physical_network_services()?
        .into_iter()
        .map(|service| {
            Ok(MacServiceProxyState {
                http: mac_get_proxy(&service, "-getwebproxy")?,
                https: mac_get_proxy(&service, "-getsecurewebproxy")?,
                socks: mac_get_proxy(&service, "-getsocksfirewallproxy")?,
                bypass_domains: mac_get_bypass(&service)?,
                service,
            })
        })
        .collect::<AppResult<Vec<_>>>()?;
    Ok(SystemProxySnapshot::Macos { services })
}

#[cfg(target_os = "macos")]
fn apply_system_proxy(port: u16) -> AppResult<()> {
    let services = mac_active_physical_network_services()?;
    if services.is_empty() {
        return Err(AppError::Platform(
            "没有检测到可设置代理的活动物理网络服务".to_string(),
        ));
    }
    for service in services {
        for command in [
            "-setwebproxy",
            "-setsecurewebproxy",
            "-setsocksfirewallproxy",
        ] {
            run_command(
                "networksetup",
                &[command, &service, "127.0.0.1", &port.to_string()],
            )?;
        }
        for command in [
            "-setwebproxystate",
            "-setsecurewebproxystate",
            "-setsocksfirewallproxystate",
        ] {
            run_command("networksetup", &[command, &service, "on"])?;
        }
        let mut bypass = mac_get_bypass(&service)?;
        for required in ["localhost", "127.0.0.1", "::1", "*.local"] {
            if !bypass.iter().any(|value| value == required) {
                bypass.push(required.to_string());
            }
        }
        let mut args = vec!["-setproxybypassdomains", service.as_str()];
        args.extend(bypass.iter().map(String::as_str));
        run_command("networksetup", &args)?;
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn verify_system_proxy_inner(port: u16) -> AppResult<()> {
    let services = mac_active_physical_network_services()?;
    if services.is_empty() {
        return Err(AppError::Platform(
            "没有检测到可验证的活动物理网络服务".to_string(),
        ));
    }
    for service in services {
        for command in [
            "-getwebproxy",
            "-getsecurewebproxy",
            "-getsocksfirewallproxy",
        ] {
            let state = mac_get_proxy(&service, command)?;
            if !state.enabled || state.server != "127.0.0.1" || state.port != port {
                return Err(AppError::Platform(format!(
                    "网络服务 {service} 的系统代理未正确应用"
                )));
            }
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn restore_snapshot(snapshot: &SystemProxySnapshot) -> AppResult<()> {
    let SystemProxySnapshot::Macos { services } = snapshot else {
        return Err(AppError::Platform("系统代理快照平台不匹配".to_string()));
    };
    for service in services {
        mac_set_proxy(
            &service.service,
            "-setwebproxy",
            "-setwebproxystate",
            &service.http,
        )?;
        mac_set_proxy(
            &service.service,
            "-setsecurewebproxy",
            "-setsecurewebproxystate",
            &service.https,
        )?;
        mac_set_proxy(
            &service.service,
            "-setsocksfirewallproxy",
            "-setsocksfirewallproxystate",
            &service.socks,
        )?;
        if service.bypass_domains.is_empty() {
            run_command(
                "networksetup",
                &["-setproxybypassdomains", &service.service, "Empty"],
            )?;
        } else {
            let mut args = vec!["-setproxybypassdomains", service.service.as_str()];
            args.extend(service.bypass_domains.iter().map(String::as_str));
            run_command("networksetup", &args)?;
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn mac_active_physical_network_services() -> AppResult<Vec<String>> {
    let output = run_command("networksetup", &["-listnetworkserviceorder"])?;
    let physical = parse_mac_network_service_order(&output)
        .into_iter()
        .filter(|entry| entry.enabled && !entry.device.is_empty())
        .collect::<Vec<_>>();
    let default_route_device = run_command("route", &["-n", "get", "default"])
        .ok()
        .and_then(|output| parse_mac_default_route_device(&output));
    if let Some(device) = default_route_device {
        let routed = physical
            .iter()
            .filter(|entry| entry.device == device)
            .map(|entry| entry.service.clone())
            .collect::<Vec<_>>();
        if !routed.is_empty() {
            return Ok(routed);
        }
    }
    let mut active = Vec::new();
    for entry in &physical {
        let info = run_command("networksetup", &["-getinfo", &entry.service])?;
        if mac_service_has_address(&info) {
            active.push(entry.service.clone());
        }
    }
    if active.is_empty() {
        Ok(physical.into_iter().map(|entry| entry.service).collect())
    } else {
        Ok(active)
    }
}

#[cfg(target_os = "macos")]
fn parse_mac_default_route_device(output: &str) -> Option<String> {
    output.lines().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        (key.trim() == "interface")
            .then(|| value.trim().to_string())
            .filter(|value| !value.is_empty())
    })
}

#[cfg(target_os = "macos")]
fn parse_mac_network_service_order(output: &str) -> Vec<MacNetworkServiceEntry> {
    let mut result = Vec::new();
    let mut pending: Option<(String, bool)> = None;
    for line in output.lines().map(str::trim) {
        if line.starts_with('(') && !line.starts_with("(Hardware Port:") {
            let Some((_, service)) = line.split_once(')') else {
                continue;
            };
            let service = service.trim();
            let enabled = !service.starts_with('*');
            pending = Some((service.trim_start_matches('*').trim().to_string(), enabled));
            continue;
        }
        if !line.starts_with("(Hardware Port:") {
            continue;
        }
        let Some((service, enabled)) = pending.take() else {
            continue;
        };
        let device = line
            .split_once(", Device:")
            .map(|(_, value)| value.trim().trim_end_matches(')').trim().to_string())
            .unwrap_or_default();
        result.push(MacNetworkServiceEntry {
            service,
            device,
            enabled,
        });
    }
    result
}

#[cfg(target_os = "macos")]
fn mac_service_has_address(output: &str) -> bool {
    output.lines().any(|line| {
        let Some((key, value)) = line.split_once(':') else {
            return false;
        };
        let value = value.trim();
        match key.trim() {
            "IP address" => {
                !value.is_empty()
                    && !value.eq_ignore_ascii_case("none")
                    && !value.starts_with("169.254.")
            }
            "IPv6 IP address" => {
                !value.is_empty()
                    && !value.eq_ignore_ascii_case("none")
                    && !value.to_ascii_lowercase().starts_with("fe80:")
            }
            _ => false,
        }
    })
}

#[cfg(target_os = "macos")]
fn mac_get_proxy(service: &str, command: &str) -> AppResult<ProxyProtocolState> {
    let output = run_command("networksetup", &[command, service])?;
    Ok(parse_mac_proxy(&output))
}

#[cfg(target_os = "macos")]
fn parse_mac_proxy(output: &str) -> ProxyProtocolState {
    let mut state = ProxyProtocolState::default();
    for line in output.lines() {
        let (key, value) = line.split_once(':').unwrap_or((line, ""));
        match key.trim() {
            "Enabled" => state.enabled = value.trim().eq_ignore_ascii_case("yes"),
            "Server" => state.server = value.trim().to_string(),
            "Port" => state.port = value.trim().parse().unwrap_or(0),
            _ => {}
        }
    }
    state
}

#[cfg(target_os = "macos")]
fn mac_get_bypass(service: &str) -> AppResult<Vec<String>> {
    let output = run_command("networksetup", &["-getproxybypassdomains", service])?;
    if output.contains("There aren't any") {
        return Ok(Vec::new());
    }
    Ok(output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect())
}

#[cfg(target_os = "macos")]
fn mac_set_proxy(
    service: &str,
    set_command: &str,
    state_command: &str,
    state: &ProxyProtocolState,
) -> AppResult<()> {
    if !state.server.is_empty() && state.port > 0 {
        run_command(
            "networksetup",
            &[set_command, service, &state.server, &state.port.to_string()],
        )?;
    }
    run_command(
        "networksetup",
        &[
            state_command,
            service,
            if state.enabled { "on" } else { "off" },
        ],
    )?;
    Ok(())
}

#[cfg(target_os = "linux")]
fn capture_system_proxy() -> AppResult<SystemProxySnapshot> {
    let mut values = BTreeMap::new();
    for (schema, key) in linux_proxy_keys() {
        let value = run_command("gsettings", &["get", schema, key])?;
        values.insert(format!("{schema}|{key}"), value.trim().to_string());
    }
    Ok(SystemProxySnapshot::Linux { values })
}

#[cfg(target_os = "linux")]
fn apply_system_proxy(port: u16) -> AppResult<()> {
    linux_set("org.gnome.system.proxy", "mode", "'manual'")?;
    linux_set("org.gnome.system.proxy.http", "host", "'127.0.0.1'")?;
    linux_set("org.gnome.system.proxy.http", "port", &port.to_string())?;
    linux_set("org.gnome.system.proxy.https", "host", "'127.0.0.1'")?;
    linux_set("org.gnome.system.proxy.https", "port", &port.to_string())?;
    linux_set("org.gnome.system.proxy.socks", "host", "'127.0.0.1'")?;
    linux_set("org.gnome.system.proxy.socks", "port", &port.to_string())?;
    linux_set(
        "org.gnome.system.proxy",
        "ignore-hosts",
        "['localhost', '127.0.0.0/8', '::1']",
    )
}

#[cfg(target_os = "linux")]
fn verify_system_proxy_inner(port: u16) -> AppResult<()> {
    let mode = run_command("gsettings", &["get", "org.gnome.system.proxy", "mode"])?;
    let http = run_command("gsettings", &["get", "org.gnome.system.proxy.http", "port"])?;
    let https = run_command(
        "gsettings",
        &["get", "org.gnome.system.proxy.https", "port"],
    )?;
    let socks = run_command(
        "gsettings",
        &["get", "org.gnome.system.proxy.socks", "port"],
    )?;
    if !mode.contains("manual")
        || http.trim() != port.to_string()
        || https.trim() != port.to_string()
        || socks.trim() != port.to_string()
    {
        return Err(AppError::Platform("GNOME 系统代理未正确应用".to_string()));
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn restore_snapshot(snapshot: &SystemProxySnapshot) -> AppResult<()> {
    let SystemProxySnapshot::Linux { values } = snapshot else {
        return Err(AppError::Platform("系统代理快照平台不匹配".to_string()));
    };
    for (compound, value) in values {
        let (schema, key) = compound
            .split_once('|')
            .ok_or_else(|| AppError::Platform("无效 Linux 代理快照".to_string()))?;
        linux_set(schema, key, value)?;
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn linux_proxy_keys() -> Vec<(&'static str, &'static str)> {
    vec![
        ("org.gnome.system.proxy", "mode"),
        ("org.gnome.system.proxy", "ignore-hosts"),
        ("org.gnome.system.proxy.http", "host"),
        ("org.gnome.system.proxy.http", "port"),
        ("org.gnome.system.proxy.https", "host"),
        ("org.gnome.system.proxy.https", "port"),
        ("org.gnome.system.proxy.socks", "host"),
        ("org.gnome.system.proxy.socks", "port"),
    ]
}

#[cfg(target_os = "linux")]
fn linux_set(schema: &str, key: &str, value: &str) -> AppResult<()> {
    run_command("gsettings", &["set", schema, key, value]).map(|_| ())
}

#[cfg(windows)]
fn capture_system_proxy() -> AppResult<SystemProxySnapshot> {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;
    let key = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Internet Settings")
        .map_err(|error| AppError::Platform(error.to_string()))?;
    Ok(SystemProxySnapshot::Windows {
        proxy_enable: key.get_value("ProxyEnable").unwrap_or(0),
        proxy_server: key.get_value("ProxyServer").ok(),
        proxy_override: key.get_value("ProxyOverride").ok(),
        auto_config_url: key.get_value("AutoConfigURL").ok(),
    })
}

#[cfg(windows)]
fn verify_system_proxy_inner(port: u16) -> AppResult<()> {
    let current = capture_system_proxy()?;
    if !windows_same_route(&current, &windows_applied_proxy(&current, port)) {
        return Err(AppError::Platform("Windows 系统代理未正确应用".to_string()));
    }
    Ok(())
}

#[cfg(windows)]
fn restore_snapshot(snapshot: &SystemProxySnapshot) -> AppResult<()> {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;
    let SystemProxySnapshot::Windows {
        proxy_enable,
        proxy_server,
        proxy_override,
        auto_config_url,
    } = snapshot
    else {
        return Err(AppError::Platform("系统代理快照平台不匹配".to_string()));
    };
    let key = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey_with_flags(
            "Software\\Microsoft\\Windows\\CurrentVersion\\Internet Settings",
            winreg::enums::KEY_SET_VALUE,
        )
        .map_err(|error| AppError::Platform(error.to_string()))?;
    windows_restore_string(&key, "ProxyServer", proxy_server.as_deref())?;
    windows_restore_string(&key, "ProxyOverride", proxy_override.as_deref())?;
    windows_restore_string(&key, "AutoConfigURL", auto_config_url.as_deref())?;
    key.set_value("ProxyEnable", proxy_enable)
        .map_err(|error| AppError::Platform(error.to_string()))?;
    windows_refresh_proxy()
}

#[cfg(windows)]
fn windows_restore_string(key: &winreg::RegKey, name: &str, value: Option<&str>) -> AppResult<()> {
    match value {
        Some(value) => key
            .set_value(name, &value)
            .map_err(|error| AppError::Platform(error.to_string())),
        None => match key.delete_value(name) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(AppError::Platform(error.to_string())),
        },
    }
}

#[cfg(windows)]
fn windows_refresh_proxy() -> AppResult<()> {
    use windows_sys::Win32::Networking::WinInet::{
        InternetSetOptionW, INTERNET_OPTION_REFRESH, INTERNET_OPTION_SETTINGS_CHANGED,
    };
    unsafe {
        let changed = InternetSetOptionW(
            std::ptr::null_mut(),
            INTERNET_OPTION_SETTINGS_CHANGED,
            std::ptr::null_mut(),
            0,
        );
        let refreshed = InternetSetOptionW(
            std::ptr::null_mut(),
            INTERNET_OPTION_REFRESH,
            std::ptr::null_mut(),
            0,
        );
        if changed == 0 || refreshed == 0 {
            return Err(AppError::Platform(
                std::io::Error::last_os_error().to_string(),
            ));
        }
    }
    Ok(())
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn run_command(program: &str, args: &[&str]) -> AppResult<String> {
    let output = Command::new(program)
        .args(args)
        .output()
        .map_err(|error| AppError::Platform(format!("{program}: {error}")))?;
    if !output.status.success() {
        return Err(AppError::Platform(format!(
            "{} {}",
            program,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::{
        mac_service_has_address, parse_mac_default_route_device, parse_mac_network_service_order,
        parse_mac_proxy,
    };

    #[test]
    fn parses_networksetup_proxy_state() {
        let state = parse_mac_proxy("Enabled: Yes\nServer: 127.0.0.1\nPort: 7895\n");
        assert!(state.enabled);
        assert_eq!(state.server, "127.0.0.1");
        assert_eq!(state.port, 7895);
    }

    #[test]
    fn identifies_physical_and_virtual_network_services() {
        let entries = parse_mac_network_service_order(
            "An asterisk (*) denotes that a network service is disabled.\n\
             (1) Wi-Fi\n\
             (Hardware Port: Wi-Fi, Device: en0)\n\n\
             (2) Shadowrocket\n\
             (Hardware Port: com.example.vpn, Device: )\n\n\
             (3) *Thunderbolt Bridge\n\
             (Hardware Port: Thunderbolt Bridge, Device: bridge0)\n",
        );
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].service, "Wi-Fi");
        assert_eq!(entries[0].device, "en0");
        assert!(entries[0].enabled);
        assert!(entries[1].device.is_empty());
        assert!(!entries[2].enabled);
    }

    #[test]
    fn detects_configured_network_addresses() {
        assert!(mac_service_has_address(
            "IP address: 192.168.1.10\nIPv6 IP address: none\n"
        ));
        assert!(!mac_service_has_address(
            "IPv6 IP address: none\nRouter: none\n"
        ));
        assert!(!mac_service_has_address(
            "IP address: none\nIPv6 IP address: fe80::1234\n"
        ));
    }

    #[test]
    fn parses_default_route_interface() {
        let output = "   route to: default\ninterface: en0\n      flags: <UP,GATEWAY>\n";
        assert_eq!(
            parse_mac_default_route_device(output).as_deref(),
            Some("en0")
        );
    }
}

/// Roll back exactly this operation's OS state and ownership file, including
/// persistence failures. Never expose snapshots or their possible credentials.
pub(crate) fn proxy_transaction<T>(
    app: &AppHandle,
    change: impl FnOnce() -> AppResult<T>,
) -> AppResult<T> {
    let before = capture_system_proxy()?;
    let path = snapshot_path(app)?;
    let lease = if path.exists() {
        Some(fs::read(&path)?)
    } else {
        None
    };
    complete_proxy_transaction(change(), || {
        restore_snapshot(&before)?;
        if capture_system_proxy()? != before {
            return Err(AppError::Platform(
                "恢复后读回的系统代理设置与原状态不一致".into(),
            ));
        }
        if let Some(bytes) = lease {
            crate::storage::write_private_atomic(&path, &bytes)
        } else if path.exists() {
            fs::remove_file(&path).map_err(AppError::from)
        } else {
            Ok(())
        }
    })
}
fn complete_proxy_transaction<T>(
    result: AppResult<T>,
    restore: impl FnOnce() -> AppResult<()>,
) -> AppResult<T> {
    result.map_err(|error| {
        AppError::startup(
            crate::error::StartupReason::ProxyApply,
            error,
            Some(restore().is_ok()),
        )
    })
}

#[cfg(any(not(windows), test))]
fn proxy_matches_port(snapshot: &SystemProxySnapshot, port: u16) -> bool {
    match snapshot {
        SystemProxySnapshot::Macos { services } => {
            !services.is_empty()
                && services.iter().all(|service| {
                    [&service.http, &service.https, &service.socks]
                        .iter()
                        .all(|p| p.enabled && p.server == "127.0.0.1" && p.port == port)
                })
        }
        SystemProxySnapshot::Linux { values } => {
            values
                .get("org.gnome.system.proxy|mode")
                .is_some_and(|v| v.trim().trim_matches('\'') == "manual")
                && ["http", "https", "socks"].iter().all(|p| {
                    values
                        .get(&format!("org.gnome.system.proxy.{p}|host"))
                        .is_some_and(|v| v.trim().trim_matches('\'') == "127.0.0.1")
                        && values
                            .get(&format!("org.gnome.system.proxy.{p}|port"))
                            .is_some_and(|v| v.trim() == port.to_string())
                })
        }
        SystemProxySnapshot::Windows { .. } => false,
    }
}

#[cfg(test)]
mod windows_proxy_tests {
    use super::*;

    #[test]
    fn automatic_resume_never_takes_over_external_proxy_or_pac() {
        let mut current = original();
        assert!(!resume_proxy_available(&current, false));
        if let SystemProxySnapshot::Windows {
            proxy_enable,
            auto_config_url,
            ..
        } = &mut current
        {
            *proxy_enable = 0;
            *auto_config_url = None;
        }
        assert!(resume_proxy_available(&current, false));
        if let SystemProxySnapshot::Windows {
            auto_config_url, ..
        } = &mut current
        {
            *auto_config_url = Some("https://company.example.invalid/proxy.pac".into());
        }
        assert!(!resume_proxy_available(&current, false));
        assert!(resume_proxy_available(
            &windows_applied_proxy(&original(), 7890),
            true
        ));
        assert!(!resume_proxy_available(
            &windows_applied_proxy(&original(), 7890),
            false
        ));
    }

    #[test]
    fn actual_lease_capture_rejects_changes_after_initial_baseline_check() {
        let baseline = SystemProxySnapshot::Windows {
            proxy_enable: 0,
            proxy_server: None,
            proxy_override: None,
            auto_config_url: None,
        };
        assert!(check_resume_baseline(&baseline, Some(&baseline)).is_ok());
        // A later capture is the one used to create a lease; an earlier match
        // must not authorize this newly observed external proxy.
        assert!(check_resume_baseline(&original(), Some(&baseline)).is_err());
        assert!(check_resume_baseline(&original(), None).is_ok());
    }

    #[test]
    fn automatic_resume_requires_inactive_non_windows_proxy() {
        let mut service = MacServiceProxyState::default();
        assert!(resume_proxy_available(
            &SystemProxySnapshot::Macos {
                services: vec![service.clone()]
            },
            false
        ));
        service.https.enabled = true;
        assert!(!resume_proxy_available(
            &SystemProxySnapshot::Macos {
                services: vec![service]
            },
            false
        ));
        for (mode, allowed) in [("'none'", true), ("'manual'", false), ("'auto'", false)] {
            assert_eq!(
                resume_proxy_available(
                    &SystemProxySnapshot::Linux {
                        values: BTreeMap::from([(
                            "org.gnome.system.proxy|mode".into(),
                            mode.into()
                        )]),
                    },
                    false
                ),
                allowed
            );
        }
        assert!(!resume_proxy_available(
            &SystemProxySnapshot::Linux {
                values: BTreeMap::new()
            },
            false
        ));
    }

    fn original() -> SystemProxySnapshot {
        SystemProxySnapshot::Windows {
            proxy_enable: 1,
            proxy_server: Some("127.0.0.1:7897".into()),
            proxy_override: Some("*.company.test;<local>".into()),
            auto_config_url: Some("https://example.test/proxy.pac".into()),
        }
    }

    fn lease(committed: bool) -> WindowsProxyLease {
        let original = original();
        let applied = windows_applied_proxy(&original, 7890);
        WindowsProxyLease::Tracked {
            before: Some(original.clone()),
            original,
            applied,
            committed,
        }
    }

    #[test]
    fn compatible_format_and_legacy_leases_restore_original_owner() {
        assert_eq!(windows_proxy_server(7890), "127.0.0.1:7890");
        for server in [
            "http=127.0.0.1:7890;https=127.0.0.1:7890;socks=127.0.0.1:7890",
            "http=127.0.0.1:7890;https=127.0.0.1:7890",
        ] {
            let mut old_current = original();
            if let SystemProxySnapshot::Windows { proxy_server, .. } = &mut old_current {
                *proxy_server = Some(server.into());
            }
            let legacy = WindowsProxyLease::Legacy(original());
            assert_eq!(legacy.restoration(&old_current, 7890), Some(original()));
            assert!(!legacy.owns(&old_current, 789));
            let tracked = WindowsProxyLease::Tracked {
                original: original(),
                before: Some(original()),
                applied: old_current.clone(),
                committed: true,
            };
            assert!(tracked.owns(&old_current, 7890));
            let upgraded = windows_applied_proxy(&old_current, 7890);
            let migrated = WindowsProxyLease::Tracked {
                original: tracked.original().clone(),
                before: Some(old_current),
                applied: upgraded.clone(),
                committed: true,
            };
            assert_eq!(migrated.restoration(&upgraded, 7890), Some(original()));
            assert!(migrated.restoration(&original(), 7890).is_none());
        }
    }

    #[cfg(windows)]
    #[test]
    fn network_library_recognizes_new_format_without_registry_mutation() {
        use hyper_util::client::proxy::matcher::Matcher;
        let destination = "https://chatgpt.com/backend-api/codex/responses"
            .parse()
            .unwrap();
        let value = windows_proxy_server(7890);
        let matcher = Matcher::builder().http(&value).https(&value).build();
        assert_eq!(
            matcher.intercept(&destination).unwrap().uri().to_string(),
            "http://127.0.0.1:7890/"
        );
        let old = "http=127.0.0.1:7890;https=127.0.0.1:7890;socks=127.0.0.1:7890";
        assert!(Matcher::builder()
            .https(old)
            .build()
            .intercept(&destination)
            .is_none());
    }

    #[test]
    fn takeover_preserves_bypass_and_temporarily_clears_pac() {
        let applied = windows_applied_proxy(&original(), 7890);
        if let SystemProxySnapshot::Windows {
            proxy_override,
            auto_config_url,
            ..
        } = &applied
        {
            assert_eq!(
                proxy_override.as_deref(),
                Some("*.company.test;<local>;localhost;127.*;[::1]")
            );
            assert!(auto_config_url.is_none());
        }
        assert_eq!(lease(true).restoration(&applied, 7890), Some(original()));
    }

    #[test]
    fn stopping_does_not_overwrite_another_proxy_client() {
        assert!(lease(true).restoration(&original(), 7890).is_none());
        let different_port = windows_applied_proxy(&original(), 7890);
        assert!(!windows_endpoint_owned(&different_port, 789));
        let mut disabled = windows_applied_proxy(&original(), 7890);
        if let SystemProxySnapshot::Windows { proxy_enable, .. } = &mut disabled {
            *proxy_enable = 0;
        }
        assert!(lease(true).restoration(&disabled, 7890).is_none());
    }

    #[test]
    fn interrupted_write_can_restore_and_bypass_edits_are_preserved() {
        let mut partial = original();
        if let SystemProxySnapshot::Windows { proxy_server, .. } = &mut partial {
            *proxy_server = Some(windows_proxy_server(7890));
        }
        assert_eq!(lease(false).restoration(&partial, 7890), Some(original()));
        let mut edited = windows_applied_proxy(&original(), 7890);
        if let SystemProxySnapshot::Windows { proxy_override, .. } = &mut edited {
            *proxy_override = Some("*.new.test".into());
        }
        let restored = lease(true).restoration(&edited, 7890).unwrap();
        assert!(windows_same_route(&restored, &original()));
        if let SystemProxySnapshot::Windows { proxy_override, .. } = restored {
            assert_eq!(proxy_override.as_deref(), Some("*.new.test"));
        }
    }

    #[test]
    fn reads_legacy_snapshot_but_does_not_restore_over_new_pac() {
        let saved: WindowsProxyLease =
            serde_json::from_str(&serde_json::to_string(&original()).unwrap()).unwrap();
        let mut current = original();
        if let SystemProxySnapshot::Windows { proxy_server, .. } = &mut current {
            *proxy_server = Some(windows_proxy_server(7890));
        }
        assert!(saved.owns(&current, 7890));
        if let SystemProxySnapshot::Windows {
            auto_config_url, ..
        } = &mut current
        {
            *auto_config_url = Some("https://new.test/proxy.pac".into());
        }
        assert!(!saved.owns(&current, 7890));
        let tracked = serde_json::to_vec(&lease(true)).unwrap();
        assert!(matches!(
            serde_json::from_slice::<WindowsProxyLease>(&tracked).unwrap(),
            WindowsProxyLease::Tracked {
                committed: true,
                ..
            }
        ));
    }

    #[test]
    fn interrupted_restoration_remains_retryable_at_each_write() {
        let target = original();
        let applied = windows_applied_proxy(&target, 7890);
        let restoring = WindowsProxyLease::Tracked {
            original: target.clone(),
            before: Some(applied.clone()),
            applied: target.clone(),
            committed: false,
        };
        let mut partial = applied;
        assert_eq!(restoring.restoration(&partial, 7890), Some(target.clone()));
        if let SystemProxySnapshot::Windows { proxy_server, .. } = &mut partial {
            *proxy_server = Some("127.0.0.1:7897".into());
        }
        assert_eq!(restoring.restoration(&partial, 7890), Some(target.clone()));
        if let SystemProxySnapshot::Windows { proxy_override, .. } = &mut partial {
            *proxy_override = Some("*.company.test;<local>".into());
        }
        assert_eq!(restoring.restoration(&partial, 7890), Some(target.clone()));
        if let SystemProxySnapshot::Windows {
            auto_config_url, ..
        } = &mut partial
        {
            *auto_config_url = Some("https://example.test/proxy.pac".into());
        }
        assert_eq!(restoring.restoration(&partial, 7890), Some(target));
    }

    #[test]
    fn failed_port_change_recovers_from_the_actual_previous_endpoint() {
        let original = original();
        let before = windows_applied_proxy(&original, 7890);
        let applied = windows_applied_proxy(&before, 7891);
        let saved = WindowsProxyLease::Tracked {
            original: original.clone(),
            before: Some(before.clone()),
            applied,
            committed: false,
        };
        assert_eq!(saved.restoration(&before, 7891), Some(original));
    }
}

#[cfg(test)]
mod local_proxy_transaction_tests {
    use super::*;
    #[test]
    fn successful_changes_do_not_run_rollback() {
        assert_eq!(
            complete_proxy_transaction(Ok(42), || panic!("unexpected rollback")).unwrap(),
            42
        );
    }
    #[test]
    fn failed_setting_or_persistence_rolls_back_and_reports_the_actual_outcome() {
        for restore_ok in [true, false] {
            let called = std::cell::Cell::new(0);
            let error = complete_proxy_transaction::<()>(
                Err(AppError::Io("fixture save error".into())),
                || {
                    called.set(called.get() + 1);
                    if restore_ok {
                        Ok(())
                    } else {
                        Err(AppError::Io("fixture restore error".into()))
                    }
                },
            )
            .unwrap_err();
            assert_eq!(called.get(), 1);
            assert!(
                matches!(error, AppError::Startup { restored: Some(ok), .. } if ok == restore_ok)
            );
        }
    }
    #[test]
    fn snapshot_file_alone_is_not_proxy_enablement_evidence() {
        let proxy = ProxyProtocolState {
            enabled: true,
            server: "127.0.0.1".into(),
            port: 7895,
        };
        let mut service = MacServiceProxyState {
            service: "fixture".into(),
            http: proxy.clone(),
            https: proxy.clone(),
            socks: proxy,
            bypass_domains: vec![],
        };
        assert!(proxy_matches_port(
            &SystemProxySnapshot::Macos {
                services: vec![service.clone()]
            },
            7895
        ));
        service.https.port = 7999;
        assert!(!proxy_matches_port(
            &SystemProxySnapshot::Macos {
                services: vec![service]
            },
            7895
        ));
        assert!(!proxy_matches_port(
            &SystemProxySnapshot::Macos { services: vec![] },
            7895
        ));
    }
}
