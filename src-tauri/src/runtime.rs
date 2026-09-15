use crate::error::{AppError, AppResult};
use crate::models::RuntimePhase;
use crate::tun_service;
use chrono::{DateTime, Utc};
#[cfg(not(windows))]
use rand::RngCore;
use regex::Regex;
use serde::Serialize;
use std::collections::VecDeque;
use std::env;
use std::fs;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
#[cfg(not(windows))]
use std::process::Child;
use std::process::{Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager};

#[cfg(windows)]
#[path = "windows_process.rs"]
mod windows_process;
#[cfg(windows)]
use windows_process::Child;

const MAX_LOG_LINES: usize = 2_000;
const VALIDATION_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_VALIDATION_LOG_BYTES: u64 = 2 * 1024 * 1024;
const VALIDATION_DIAGNOSTIC_BYTES: u64 = 32 * 1024;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BinaryInfo {
    pub available: bool,
    pub path: Option<String>,
    pub version: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeLog {
    pub timestamp: DateTime<Utc>,
    pub level: String,
    pub source: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeStatus {
    pub state: String,
    pub phase: RuntimePhase,
    pub binary_available: bool,
    pub binary_path: Option<String>,
    pub version: Option<String>,
    pub config_path: Option<String>,
    pub message: String,
    pub pid: Option<u32>,
    pub started_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
}

#[derive(Default)]
pub struct MihomoRuntime {
    child: Mutex<Option<Child>>,
    phase: Mutex<RuntimePhase>,
    binary_path: Mutex<Option<PathBuf>>,
    binary_version: Mutex<Option<String>>,
    config_path: Mutex<Option<PathBuf>>,
    started_at: Mutex<Option<DateTime<Utc>>>,
    last_error: Mutex<Option<String>>,
    logs: Arc<Mutex<VecDeque<RuntimeLog>>>,
    tun_lease: Mutex<Option<String>>,
    tun_pid: Mutex<Option<u32>>,
    tun_heartbeat_stop: Mutex<Option<Arc<AtomicBool>>>,
    tun_heartbeat_error: Arc<Mutex<Option<String>>>,
}

impl MihomoRuntime {
    pub fn start(&self, app: &AppHandle, source: &str) -> AppResult<RuntimeStatus> {
        self.start_attempt(|| self.start_process(app, source))
    }

    fn start_attempt<T>(&self, operation: impl FnOnce() -> AppResult<T>) -> AppResult<T> {
        if self.is_running()? {
            return Err(AppError::Conflict("Mihomo 已经在运行".to_string()));
        }
        {
            let mut phase = lock(&self.phase, "phase")?;
            if matches!(
                *phase,
                RuntimePhase::Validating
                    | RuntimePhase::Starting
                    | RuntimePhase::Running
                    | RuntimePhase::Stopping
                    | RuntimePhase::Recovering
            ) {
                return Err(AppError::Conflict(
                    "Mihomo 正在处理运行状态，请稍后重试".to_string(),
                ));
            }
            *phase = RuntimePhase::Validating;
        }
        let result = operation();
        if let Err(error) = &result {
            // Only this failed attempt with no owned process/lease can become
            // retryable. Never stop a live session or relax in-flight guards.
            if let (Ok(child), Ok(lease)) = (self.child.lock(), self.tun_lease.lock()) {
                if child.is_none() && lease.is_none() {
                    if let Ok(mut phase) = self.phase.lock() {
                        if matches!(*phase, RuntimePhase::Validating | RuntimePhase::Starting) {
                            if let Ok(mut last_error) = self.last_error.lock() {
                                *last_error = Some(redact(&error.to_string()));
                            }
                            *phase = RuntimePhase::Crashed;
                        }
                    }
                }
            }
        }
        result
    }

    fn start_process(&self, app: &AppHandle, source: &str) -> AppResult<RuntimeStatus> {
        let binary = resolve_binary(Some(app))
            .ok_or_else(|| AppError::Runtime("未找到 Mihomo sidecar".to_string()))?;
        preflight_ports(source)?;
        let profile_dir = runtime_directory(app)?;
        let config_path = profile_dir.join("active.yaml");
        write_private_file(&config_path, source.as_bytes())?;
        validate_file(&binary, &profile_dir, &config_path)?;

        self.set_phase(RuntimePhase::Starting);
        if let Some(feedback) = app.try_state::<crate::connection_feedback::ConnectionFeedback>() {
            feedback.starting(app);
        }
        let version = binary_version(&binary).ok();
        let mut command = Command::new(&binary);
        command
            .arg("-d")
            .arg(&profile_dir)
            .arg("-f")
            .arg(&config_path)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // Timestamp the run before the core can emit any startup diagnostics.
        let started_at = Utc::now();
        let mut child = spawn_core_command(&mut command, None)
            .map_err(|error| AppError::Runtime(error.to_string()))?;
        if let Some(stdout) = child.stdout.take() {
            spawn_log_reader(self.logs.clone(), "stdout", stdout);
        }
        if let Some(stderr) = child.stderr.take() {
            spawn_log_reader(self.logs.clone(), "stderr", stderr);
        }
        let pid = child.id();
        *lock(&self.child, "child")? = Some(child);
        *lock(&self.binary_path, "binary path")? = Some(binary);
        *lock(&self.binary_version, "binary version")? = version;
        *lock(&self.config_path, "config path")? = Some(config_path);
        *lock(&self.started_at, "started at")? = Some(started_at);
        *lock(&self.last_error, "last error")? = None;
        self.set_phase(RuntimePhase::Running);
        self.push_log("info", "runtime", format!("Mihomo started with pid {pid}"));
        Ok(self.status(Some(app)))
    }

    pub fn start_tun(&self, app: &AppHandle, source: &str) -> AppResult<RuntimeStatus> {
        #[cfg(windows)]
        {
            let session = tun_service::status();
            if !session.ready() {
                return Err(AppError::Platform(session.message));
            }
            // Elevated Windows sessions own the core directly. They do not
            // create a macOS helper lease or claim a persistent service exists.
            self.start(app, source)
        }
        #[cfg(not(windows))]
        {
            self.start_attempt(|| self.start_privileged_tun(app, source))
        }
    }

    #[cfg(not(windows))]
    fn start_privileged_tun(&self, app: &AppHandle, source: &str) -> AppResult<RuntimeStatus> {
        let helper = tun_service::status();
        if !helper.ready() {
            return Err(AppError::Platform(helper.message));
        }
        preflight_ports(source)?;
        validate_source(app, source)?;
        self.set_phase(RuntimePhase::Starting);

        let mut lease_bytes = [0_u8; 32];
        rand::rng().fill_bytes(&mut lease_bytes);
        let lease = lease_bytes
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let started_at = Utc::now();
        let started = tun_service::start(source, &lease)?;
        let config_path = PathBuf::from(&started.config_path);
        let binary_path = config_path.parent().map(|parent| parent.join("mihomo"));

        *lock(&self.tun_lease, "tun lease")? = Some(lease.clone());
        *lock(&self.tun_pid, "tun pid")? = Some(started.pid);
        *lock(&self.binary_path, "binary path")? = binary_path;
        *lock(&self.binary_version, "binary version")? = started.version;
        *lock(&self.config_path, "config path")? = Some(config_path);
        *lock(&self.started_at, "started at")? = Some(started_at);
        *lock(&self.last_error, "last error")? = None;
        if let Ok(mut error) = self.tun_heartbeat_error.lock() {
            *error = None;
        }
        self.start_tun_heartbeat(lease);
        self.set_phase(RuntimePhase::Running);
        self.push_log(
            "info",
            "runtime",
            format!("Privileged TUN Mihomo started with pid {}", started.pid),
        );
        Ok(self.status(Some(app)))
    }

    pub fn stop(&self, app: Option<&AppHandle>) -> AppResult<RuntimeStatus> {
        if let Some(app) = app {
            if let Some(feedback) =
                app.try_state::<crate::connection_feedback::ConnectionFeedback>()
            {
                feedback.invalidate(app);
            }
        }
        self.set_phase(RuntimePhase::Stopping);
        self.stop_tun_heartbeat();
        let had_tun = lock(&self.tun_lease, "tun lease")?.take().is_some();
        let tun_stop_result = if had_tun { tun_service::stop() } else { Ok(()) };
        *lock(&self.tun_pid, "tun pid")? = None;
        if let Ok(mut error) = self.tun_heartbeat_error.lock() {
            *error = None;
        }
        {
            let mut child_guard = lock(&self.child, "child")?;
            if let Some(child) = child_guard.as_mut() {
                let pid = child.id();
                let _ = child.kill();
                let _ = child.wait();
                self.push_log("info", "runtime", format!("Mihomo stopped with pid {pid}"));
            }
            *child_guard = None;
        }
        *lock(&self.started_at, "started at")? = None;
        self.set_phase(RuntimePhase::Stopped);
        let status = self.status(app);
        tun_stop_result?;
        Ok(status)
    }

    pub fn status(&self, app: Option<&AppHandle>) -> RuntimeStatus {
        let mut phase = self
            .phase
            .lock()
            .map(|value| *value)
            .unwrap_or(RuntimePhase::Crashed);
        let mut pid = None;
        if let Ok(mut child_guard) = self.child.lock() {
            if let Some(child) = child_guard.as_mut() {
                pid = Some(child.id());
                match child.try_wait() {
                    Ok(Some(exit)) => {
                        let message = format!("Mihomo exited with {exit}");
                        if let Ok(mut error) = self.last_error.lock() {
                            *error = Some(message.clone());
                        }
                        self.push_log("error", "runtime", message);
                        *child_guard = None;
                        phase = RuntimePhase::Crashed;
                        self.set_phase(phase);
                        pid = None;
                    }
                    Ok(None) => {
                        phase = RuntimePhase::Running;
                    }
                    Err(error) => {
                        if let Ok(mut last_error) = self.last_error.lock() {
                            *last_error = Some(error.to_string());
                        }
                        phase = RuntimePhase::Crashed;
                        self.set_phase(phase);
                    }
                }
            }
        }

        let tun_active = self
            .tun_lease
            .lock()
            .ok()
            .is_some_and(|lease| lease.is_some());
        if tun_active {
            pid = self.tun_pid.lock().ok().and_then(|value| *value);
            let heartbeat_error = self
                .tun_heartbeat_error
                .lock()
                .ok()
                .and_then(|value| value.clone());
            if let Some(error) = heartbeat_error {
                if let Ok(mut last_error) = self.last_error.lock() {
                    *last_error = Some(error);
                }
                phase = RuntimePhase::Crashed;
                self.set_phase(phase);
            } else {
                phase = RuntimePhase::Running;
            }
        }

        let cached_path = self.binary_path.lock().ok().and_then(|p| p.clone());
        let cached_version = self.binary_version.lock().ok().and_then(|v| v.clone());
        let binary_info = if let Some(path) = cached_path.filter(|p| p.is_file()) {
            BinaryInfo {
                available: true,
                path: Some(path.display().to_string()),
                version: cached_version,
                message: "代理服务文件已就绪".into(),
            }
        } else {
            probe_binary(app)
        };
        let binary_path = self
            .binary_path
            .lock()
            .ok()
            .and_then(|value| value.as_ref().map(|path| path.display().to_string()))
            .or(binary_info.path.clone());
        let version = self
            .binary_version
            .lock()
            .ok()
            .and_then(|value| value.clone())
            .or(binary_info.version.clone());
        let config_path = self
            .config_path
            .lock()
            .ok()
            .and_then(|value| value.as_ref().map(|path| path.display().to_string()));
        let started_at = self.started_at.lock().ok().and_then(|value| *value);
        let last_error = self.last_error.lock().ok().and_then(|value| value.clone());

        let message = match phase {
            RuntimePhase::Running => "Mihomo 正在运行".to_string(),
            RuntimePhase::Validating => "正在校验配置".to_string(),
            RuntimePhase::Starting => "正在启动 Mihomo".to_string(),
            RuntimePhase::Stopping => "正在停止 Mihomo".to_string(),
            RuntimePhase::Crashed => last_error
                .clone()
                .unwrap_or_else(|| "Mihomo 异常退出".to_string()),
            _ if binary_info.available => "Mihomo 已停止".to_string(),
            _ => binary_info.message.clone(),
        };
        RuntimeStatus {
            state: if phase == RuntimePhase::Running {
                "running"
            } else {
                "stopped"
            }
            .to_string(),
            phase,
            binary_available: binary_info.available,
            binary_path,
            version,
            config_path,
            message,
            pid,
            started_at,
            last_error,
        }
    }

    pub fn logs(&self, limit: usize) -> Vec<RuntimeLog> {
        let limit = limit.clamp(1, MAX_LOG_LINES);
        let mut logs: Vec<RuntimeLog> = self
            .logs
            .lock()
            .map(|logs| {
                logs.iter()
                    .rev()
                    .take(limit)
                    .cloned()
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect()
            })
            .unwrap_or_default();
        let include_tun = self
            .tun_lease
            .lock()
            .ok()
            .is_some_and(|lease| lease.is_some())
            || self.tun_pid.lock().ok().is_some_and(|pid| pid.is_some());
        if include_tun {
            logs.extend(tun_service::logs(limit).into_iter().map(|log| RuntimeLog {
                timestamp: log.timestamp,
                level: log.level,
                source: format!("tun-helper/{}", log.source),
                message: log.message,
            }));
            logs.sort_by_key(|log| log.timestamp);
            if logs.len() > limit {
                logs.drain(..logs.len() - limit);
            }
        }
        logs
    }

    pub(crate) fn running_identity(&self) -> Option<(u32, DateTime<Utc>)> {
        if !self.is_running().ok()? {
            return None;
        }
        let started = (*self.started_at.lock().ok()?)?;
        let pid = self
            .child
            .lock()
            .ok()?
            .as_ref()
            .map(|child| child.id())
            .or_else(|| self.tun_pid.lock().ok().and_then(|pid| *pid))?;
        Some((pid, started))
    }

    pub fn clear_logs(&self) {
        if let Ok(mut logs) = self.logs.lock() {
            logs.clear();
        }
    }

    fn is_running(&self) -> AppResult<bool> {
        if lock(&self.tun_lease, "tun lease")?.is_some()
            && self
                .tun_heartbeat_error
                .lock()
                .map_err(|_| AppError::Runtime("tun heartbeat lock poisoned".to_string()))?
                .is_none()
        {
            return Ok(true);
        }
        let mut child_guard = lock(&self.child, "child")?;
        match child_guard.as_mut() {
            Some(child) => match child
                .try_wait()
                .map_err(|error| AppError::Runtime(error.to_string()))?
            {
                Some(exit) => {
                    *child_guard = None;
                    self.set_phase(RuntimePhase::Crashed);
                    *lock(&self.last_error, "last error")? =
                        Some(format!("Mihomo exited with {exit}"));
                    Ok(false)
                }
                None => Ok(true),
            },
            None => Ok(false),
        }
    }

    fn set_phase(&self, phase: RuntimePhase) {
        if let Ok(mut value) = self.phase.lock() {
            *value = phase;
        }
    }

    fn push_log(&self, level: &str, source: &str, message: String) {
        push_log(&self.logs, level, source, message);
    }

    #[cfg(not(windows))]
    fn start_tun_heartbeat(&self, lease: String) {
        self.stop_tun_heartbeat();
        let stop = Arc::new(AtomicBool::new(false));
        if let Ok(mut guard) = self.tun_heartbeat_stop.lock() {
            *guard = Some(stop.clone());
        }
        let error_slot = self.tun_heartbeat_error.clone();
        std::thread::spawn(move || {
            let mut failures = 0_u8;
            while !stop.load(Ordering::Acquire) {
                std::thread::sleep(std::time::Duration::from_secs(4));
                if stop.load(Ordering::Acquire) {
                    break;
                }
                match tun_service::heartbeat(&lease) {
                    Ok(()) => failures = 0,
                    Err(error) => {
                        failures = failures.saturating_add(1);
                        if failures >= 3 {
                            if let Ok(mut slot) = error_slot.lock() {
                                *slot = Some(format!("TUN Helper 心跳失败：{error}"));
                            }
                            let _ = tun_service::stop();
                            break;
                        }
                    }
                }
            }
        });
    }

    fn stop_tun_heartbeat(&self) {
        if let Ok(mut guard) = self.tun_heartbeat_stop.lock() {
            if let Some(stop) = guard.take() {
                stop.store(true, Ordering::Release);
            }
        }
    }
}

pub fn probe_binary(app: Option<&AppHandle>) -> BinaryInfo {
    let Some(path) = resolve_binary(app) else {
        return BinaryInfo {
            available: false,
            path: None,
            version: None,
            message: "未找到 Mihomo sidecar".to_string(),
        };
    };
    match binary_version(&path) {
        Ok(version) => BinaryInfo {
            available: true,
            path: Some(path.display().to_string()),
            version: Some(version),
            message: "已找到 Mihomo 内核".to_string(),
        },
        Err(error) => BinaryInfo {
            available: false,
            path: Some(path.display().to_string()),
            version: None,
            message: error.to_string(),
        },
    }
}

pub fn validate_source(app: &AppHandle, source: &str) -> AppResult<()> {
    let binary = resolve_binary(Some(app))
        .ok_or_else(|| AppError::Runtime("未找到 Mihomo sidecar".to_string()))?;
    let profile_dir = runtime_directory(app)?;
    let candidate = profile_dir.join("candidate.yaml");
    write_private_file(&candidate, source.as_bytes())?;
    let result = validate_file(&binary, &profile_dir, &candidate);
    let _ = fs::remove_file(candidate);
    result
}

fn runtime_directory(app: &AppHandle) -> AppResult<PathBuf> {
    let directory = app
        .path()
        .app_data_dir()
        .map_err(|error| AppError::Io(error.to_string()))?
        .join("runtime");
    fs::create_dir_all(&directory)?;
    set_private_directory_permissions(&directory)?;
    Ok(directory)
}

pub(crate) fn validate_file(binary: &Path, data_dir: &Path, config_path: &Path) -> AppResult<()> {
    let mut command = Command::new(binary);
    command
        .arg("-t")
        .arg("-d")
        .arg(data_dir)
        .arg("-f")
        .arg(config_path);
    run_validation_command(&mut command, data_dir, VALIDATION_TIMEOUT)
}

struct ValidationLogCleanup(PathBuf);

impl Drop for ValidationLogCleanup {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn run_validation_command(
    command: &mut Command,
    data_dir: &Path,
    timeout: Duration,
) -> AppResult<()> {
    let log_path = data_dir.join(format!("validation-output-{}.log", uuid::Uuid::new_v4()));
    let _cleanup = ValidationLogCleanup(log_path.clone());
    let mut options = fs::OpenOptions::new();
    options.read(true).write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut log = options.open(&log_path)?;
    command
        .stdin(Stdio::null())
        .stdout(Stdio::from(log.try_clone()?))
        .stderr(Stdio::from(log.try_clone()?));
    let spawned = spawn_core_command(command, Some(&log));
    // Release the Command's copies as well as the child's handles before the
    // cleanup guard removes its private file, including on Windows.
    command.stdout(Stdio::null()).stderr(Stdio::null());
    let mut child = spawned.map_err(|error| AppError::Runtime(error.to_string()))?;
    let status = wait_for_validation(&mut child, &log, timeout)?;
    if status.success() {
        return Ok(());
    }
    let length = log.metadata()?.len();
    log.seek(SeekFrom::Start(
        length.saturating_sub(VALIDATION_DIAGNOSTIC_BYTES),
    ))?;
    let mut bytes = Vec::new();
    log.take(VALIDATION_DIAGNOSTIC_BYTES)
        .read_to_end(&mut bytes)?;
    Err(AppError::Config(validation_diagnostic(&bytes)))
}

fn wait_for_validation(
    child: &mut Child,
    log: &fs::File,
    timeout: Duration,
) -> AppResult<ExitStatus> {
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) => {}
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(AppError::Runtime(error.to_string()));
            }
        }
        let exceeded_log = match log.metadata() {
            Ok(metadata) => metadata.len() > MAX_VALIDATION_LOG_BYTES,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(AppError::Io(error.to_string()));
            }
        };
        if started.elapsed() >= timeout || exceeded_log {
            let _ = child.kill();
            let _ = child.wait();
            return Err(AppError::Runtime(if exceeded_log {
                "Mihomo 校验日志超过 2 MiB，已终止校验进程".to_string()
            } else {
                format!(
                    "Mihomo 配置校验超时（{} 秒），已终止校验进程",
                    timeout.as_secs_f64()
                )
            }));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn validation_diagnostic(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let detail = text
        .lines()
        .rev()
        .find(|line| {
            let line = line.to_ascii_lowercase();
            line.contains("level=fatal") || line.contains("level=error") || line.contains("error:")
        })
        .or_else(|| text.lines().rev().find(|line| !line.trim().is_empty()))
        .unwrap_or("配置校验失败");
    let redacted = redact(detail);
    let mut end = redacted.len().min(2048);
    while !redacted.is_char_boundary(end) {
        end -= 1;
    }
    redacted[..end].to_string()
}

fn preflight_ports(source: &str) -> AppResult<()> {
    let document: serde_yaml::Value =
        serde_yaml::from_str(source).map_err(|error| AppError::Config(error.to_string()))?;
    let root = document
        .as_mapping()
        .ok_or_else(|| AppError::Config("配置根节点必须是 YAML 对象".to_string()))?;
    let mixed_port = root
        .get(serde_yaml::Value::String("mixed-port".to_string()))
        .and_then(serde_yaml::Value::as_u64)
        .and_then(|value| u16::try_from(value).ok());
    let controller_port = root
        .get(serde_yaml::Value::String("external-controller".to_string()))
        .and_then(serde_yaml::Value::as_str)
        .and_then(|value| value.rsplit_once(':'))
        .and_then(|(_, port)| port.parse::<u16>().ok());
    for (label, port) in [
        ("mixed port", mixed_port),
        ("controller port", controller_port),
    ] {
        if let Some(port) = port {
            TcpListener::bind(("127.0.0.1", port)).map_err(|error| {
                AppError::startup(
                    crate::error::StartupReason::PortInUse,
                    format!("{label} {port}: {error}"),
                    None,
                )
            })?;
        }
    }
    Ok(())
}

fn binary_version(path: &Path) -> AppResult<String> {
    #[cfg(not(windows))]
    let output = Command::new(path)
        .arg("-v")
        .output()
        .map_err(|error| AppError::Runtime(error.to_string()))?;
    #[cfg(windows)]
    let output = windows_process::output(Command::new(path).arg("-v"))
        .map_err(|error| AppError::Runtime(error.to_string()))?;
    if !output.status.success() {
        return Err(AppError::Runtime(
            first_non_empty_line(&output.stderr).unwrap_or_else(|| "Mihomo 无法执行".to_string()),
        ));
    }
    first_non_empty_line(&output.stdout)
        .or_else(|| first_non_empty_line(&output.stderr))
        .ok_or_else(|| AppError::Runtime("Mihomo 未返回版本信息".to_string()))
}

fn spawn_core_command(command: &mut Command, log: Option<&fs::File>) -> std::io::Result<Child> {
    #[cfg(windows)]
    {
        windows_process::spawn(command, log)
    }
    #[cfg(not(windows))]
    {
        let _ = log;
        command.spawn()
    }
}

pub(crate) fn resolve_binary(app: Option<&AppHandle>) -> Option<PathBuf> {
    if let Ok(value) = env::var("MIHOMO_BIN") {
        let path = PathBuf::from(value);
        if is_executable_file(&path) {
            return Some(path);
        }
    }
    if let Some(app) = app {
        if let Ok(resource_dir) = app.path().resource_dir() {
            for candidate in binary_candidates(&resource_dir) {
                if is_executable_file(&candidate) {
                    return Some(candidate);
                }
            }
        }
    }
    if let Some(directory) = env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf))
    {
        for candidate in binary_candidates(&directory) {
            if is_executable_file(&candidate) {
                return Some(candidate);
            }
        }
    }
    let path_variable = env::var_os("PATH")?;
    for directory in env::split_paths(&path_variable) {
        for candidate in binary_candidates(&directory) {
            if is_executable_file(&candidate) {
                return Some(candidate);
            }
        }
    }
    None
}

fn binary_candidates(directory: &Path) -> Vec<PathBuf> {
    if cfg!(windows) {
        vec![directory.join("mihomo.exe"), directory.join("mihomo")]
    } else {
        vec![directory.join("mihomo")]
    }
}

fn spawn_log_reader<R>(logs: Arc<Mutex<VecDeque<RuntimeLog>>>, source: &'static str, reader: R)
where
    R: std::io::Read + Send + 'static,
{
    std::thread::spawn(move || {
        for line in BufReader::new(reader).lines().map_while(Result::ok) {
            let level = if line.contains("level=error") || line.contains("level=fatal") {
                "error"
            } else if line.contains("level=warn") {
                "warn"
            } else {
                "info"
            };
            push_log(&logs, level, source, line);
        }
    });
}

fn push_log(logs: &Arc<Mutex<VecDeque<RuntimeLog>>>, level: &str, source: &str, message: String) {
    if source != "runtime" {
        if let Some(summary) = crate::app_log::core_network_summary(level, &message) {
            crate::app_log::record(
                if level == "error" { 2 } else { 1 },
                crate::app_log::Area::CoreNetwork,
                summary,
            );
        }
    }
    if source == "runtime" {
        crate::app_log::record(
            if level == "error" { 2 } else { 0 },
            crate::app_log::Area::Runtime,
            &message,
        );
    }
    if let Ok(mut lines) = logs.lock() {
        lines.push_back(RuntimeLog {
            timestamp: Utc::now(),
            level: level.to_string(),
            source: source.to_string(),
            message: redact(&message),
        });
        while lines.len() > MAX_LOG_LINES {
            lines.pop_front();
        }
    }
}

pub(crate) fn redact(value: &str) -> String {
    static URL_QUERY: OnceLock<Regex> = OnceLock::new();
    static UUID: OnceLock<Regex> = OnceLock::new();
    static SECRET_FIELD: OnceLock<Regex> = OnceLock::new();
    let value = URL_QUERY
        .get_or_init(|| Regex::new(r"(https?://[^\s?]+)\?[^\s]+").expect("valid regex"))
        .replace_all(value, "$1?[REDACTED]")
        .to_string();
    let value = UUID
        .get_or_init(|| {
            Regex::new(r"(?i)\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b")
                .expect("valid regex")
        })
        .replace_all(&value, "[UUID]")
        .to_string();
    SECRET_FIELD
        .get_or_init(|| {
            Regex::new(r#"(?i)\b(token|secret|password|passwd|uuid)(\s*[:=]\s*["']?)[^\s"',}]+"#)
                .expect("valid regex")
        })
        .replace_all(&value, "$1$2[REDACTED]")
        .to_string()
}

fn is_executable_file(path: &Path) -> bool {
    path.is_file()
}

fn first_non_empty_line(bytes: &[u8]) -> Option<String> {
    String::from_utf8_lossy(bytes)
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_string)
}

pub(crate) fn write_private_file(path: &Path, bytes: &[u8]) -> AppResult<()> {
    fs::write(path, bytes)?;
    set_private_file_permissions(path)
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

pub(crate) fn set_private_directory_permissions(path: &Path) -> AppResult<()> {
    #[cfg(not(unix))]
    let _ = path;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn lock<'a, T>(mutex: &'a Mutex<T>, label: &str) -> AppResult<std::sync::MutexGuard<'a, T>> {
    mutex
        .lock()
        .map_err(|_| AppError::Runtime(format!("{label} state lock poisoned")))
}

#[cfg(test)]
mod tests {
    use super::{
        preflight_ports, redact, run_validation_command, spawn_core_command, validation_diagnostic,
        MihomoRuntime,
    };
    use crate::error::{AppError, AppResult};
    use crate::models::RuntimePhase;
    use std::process::Command;
    use std::time::{Duration, Instant};

    #[test]
    fn redacts_urls_and_uuids() {
        let input = "https://example.com/sub?token=secret 123e4567-e89b-12d3-a456-426614174000";
        let output = redact(input);
        assert!(!output.contains("secret"));
        assert!(!output.contains("123e4567"));
        assert!(!redact("password: top-secret").contains("top-secret"));
    }

    #[test]
    fn rejects_an_occupied_port() {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("listener");
        let port = listener.local_addr().expect("local address").port();
        let source =
            format!("mixed-port: {port}\nexternal-controller: 127.0.0.1:19090\nproxies: []\n");
        assert!(preflight_ports(&source).is_err());
    }

    #[test]
    fn failed_port_preflight_converges_to_crashed_and_can_retry() {
        let runtime = MihomoRuntime::default();
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("listener");
        let port = listener.local_addr().expect("address").port();
        let source = format!("mixed-port: {port}\nproxies: []\n");
        runtime
            .start_attempt(|| preflight_ports(&source))
            .expect_err("occupied port");
        assert_eq!(*runtime.phase.lock().unwrap(), RuntimePhase::Crashed);
        assert!(runtime.last_error.lock().unwrap().is_some());
        drop(listener);
        runtime
            .start_attempt(|| {
                preflight_ports(&source)?;
                runtime.set_phase(RuntimePhase::Stopped);
                Ok(())
            })
            .expect("failed attempt must be retryable");
    }

    #[test]
    fn failed_process_spawn_converges_to_crashed_and_can_retry() {
        let runtime = MihomoRuntime::default();
        let missing = std::env::temp_dir()
            .join(format!("routedeck-missing-core-{}", uuid::Uuid::new_v4()))
            .join("mihomo.exe");
        assert!(!missing.exists());
        let result: AppResult<()> = runtime.start_attempt(|| {
            runtime.set_phase(RuntimePhase::Starting);
            spawn_core_command(&mut Command::new(&missing), None)
                .map_err(|error| AppError::Runtime(error.to_string()))?;
            Ok(())
        });
        assert!(result.is_err());
        assert_eq!(*runtime.phase.lock().unwrap(), RuntimePhase::Crashed);
        assert!(runtime.child.lock().unwrap().is_none());
        assert!(runtime.last_error.lock().unwrap().is_some());
        runtime
            .start_attempt(|| {
                runtime.set_phase(RuntimePhase::Stopped);
                Ok(())
            })
            .expect("failed spawn must be retryable");
    }

    #[test]
    fn start_attempt_keeps_inflight_and_existing_lease_guards() {
        let runtime = MihomoRuntime::default();
        let result: AppResult<()> = runtime.start_attempt(|| {
            assert!(runtime.start_attempt(|| Ok(())).is_err());
            assert_eq!(*runtime.phase.lock().unwrap(), RuntimePhase::Validating);
            Err(AppError::Runtime(
                "synthetic validation failure".to_string(),
            ))
        });
        assert!(result.is_err());
        assert_eq!(*runtime.phase.lock().unwrap(), RuntimePhase::Crashed);
        *runtime.tun_lease.lock().unwrap() = Some("synthetic-owned-lease".to_string());
        runtime.set_phase(RuntimePhase::Running);
        assert!(runtime.start_attempt(|| Ok(())).is_err());
        assert_eq!(*runtime.phase.lock().unwrap(), RuntimePhase::Running);
        assert_eq!(
            runtime.tun_lease.lock().unwrap().as_deref(),
            Some("synthetic-owned-lease")
        );
    }

    #[test]
    fn validation_timeout_child_fixture() {
        if std::env::var("MIHOMO_VALIDATION_TIMEOUT_FIXTURE").as_deref() == Ok("1") {
            loop {
                std::thread::sleep(Duration::from_secs(1));
            }
        }
    }

    #[test]
    fn validation_timeout_kills_reaps_and_cleans_up_synthetic_child() {
        let root = std::env::temp_dir().join(format!(
            "routedeck-validation-timeout-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).expect("fixture directory");
        let mut command = Command::new(std::env::current_exe().expect("test binary"));
        command
            .args([
                "--exact",
                "runtime::tests::validation_timeout_child_fixture",
                "--nocapture",
            ])
            .env("MIHOMO_VALIDATION_TIMEOUT_FIXTURE", "1");
        let started = Instant::now();
        let result = run_validation_command(&mut command, &root, Duration::from_millis(100))
            .expect_err("synthetic child must time out");
        assert!(result.to_string().contains("校验超时"));
        assert!(started.elapsed() < Duration::from_secs(5));
        assert_eq!(
            std::fs::read_dir(&root).expect("remaining files").count(),
            0
        );
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn validation_diagnostics_choose_error_and_redact_bounded_output() {
        let text =
            b"level=info starting\nlevel=error invalid rule token=fixture-secret\nlast info line\n";
        let result = validation_diagnostic(text);
        assert!(result.contains("invalid rule"));
        assert!(!result.contains("fixture-secret"));
        assert!(validation_diagnostic("x".repeat(4096).as_bytes()).len() <= 2048);
        assert!(validation_diagnostic("错".repeat(4096).as_bytes()).len() <= 2048);
    }
}
