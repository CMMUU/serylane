use super::codesign;
use super::protocol::*;
use super::xpc::*;
use super::TunRuntimeLog;
use block2::RcBlock;
use chrono::Utc;
use serde_yaml::Value;
use std::collections::VecDeque;
use std::ffi::{CStr, CString};
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

const ROOT_RUNTIME_BASE: &str = "/Library/Application Support/com.cmmuu.mihomodesktop/tun-runtime";
const LEASE_TIMEOUT: Duration = Duration::from_secs(18);
const VALIDATION_TIMEOUT: Duration = Duration::from_secs(120);
const SEED_ASSETS: &[&str] = &[
    "GeoSite.dat",
    "GeoIP.dat",
    "geoip.metadb",
    "country.mmdb",
    "ASN.mmdb",
];

#[derive(Debug, Clone)]
struct RuntimeSnapshot {
    running: bool,
    pid: Option<u32>,
    version: Option<String>,
    config_path: Option<String>,
    last_error: Option<String>,
}

struct HelperRuntime {
    child: Option<Child>,
    owner_uid: Option<u32>,
    lease: Option<String>,
    last_heartbeat: Option<Instant>,
    version: Option<String>,
    config_path: Option<PathBuf>,
    last_error: Option<String>,
    logs: Arc<Mutex<VecDeque<TunRuntimeLog>>>,
}

impl Default for HelperRuntime {
    fn default() -> Self {
        Self {
            child: None,
            owner_uid: None,
            lease: None,
            last_heartbeat: None,
            version: None,
            config_path: None,
            last_error: None,
            logs: Arc::new(Mutex::new(VecDeque::new())),
        }
    }
}

static RUNTIME: OnceLock<Mutex<HelperRuntime>> = OnceLock::new();

fn runtime() -> &'static Mutex<HelperRuntime> {
    RUNTIME.get_or_init(|| Mutex::new(HelperRuntime::default()))
}

pub fn run() -> ! {
    if unsafe { libc::geteuid() } != 0 {
        daemon_log(libc::LOG_ERR, "TUN Helper 必须由 launchd 以 root 运行");
        std::process::exit(1);
    }
    let app_binary = match codesign::sibling_executable(APP_BINARY_NAME) {
        Ok(path) => path,
        Err(error) => {
            daemon_log(libc::LOG_ERR, &error);
            std::process::exit(1);
        }
    };
    let requirement = match codesign::designated_requirement(&app_binary) {
        Ok(requirement) => requirement,
        Err(error) => {
            daemon_log(libc::LOG_ERR, &format!("读取主应用签名失败：{error}"));
            std::process::exit(1);
        }
    };
    let requirement = CString::new(requirement).expect("code requirement has no NUL");
    let name = CString::new(LABEL).expect("service label has no NUL");
    start_watchdog();

    unsafe {
        let listener = xpc_connection_create_mach_service(
            name.as_ptr(),
            std::ptr::null_mut(),
            XPC_CONNECTION_MACH_SERVICE_LISTENER,
        );
        if listener.is_null() {
            daemon_log(libc::LOG_ERR, "创建 TUN Helper Mach Service 失败");
            std::process::exit(1);
        }
        let on_peer = RcBlock::new(move |peer: XpcObject| {
            if !is_type(peer, type_connection()) {
                return;
            }
            let status = set_peer_requirement(peer, requirement.as_ptr());
            if status != 0 {
                daemon_log(
                    libc::LOG_ERR,
                    &format!("拒绝未通过签名约束的 XPC 客户端：OSStatus {status}"),
                );
                xpc_connection_cancel(peer);
                return;
            }
            let on_message = RcBlock::new(move |event: XpcObject| {
                if is_type(event, type_dictionary()) {
                    handle_request(event);
                }
            });
            xpc_connection_set_event_handler(peer, &on_message);
            xpc_connection_resume(peer);
        });
        xpc_connection_set_event_handler(listener, &on_peer);
        xpc_connection_resume(listener);
        daemon_log(
            libc::LOG_NOTICE,
            &format!("TUN Helper 已启动，协议版本 {PROTOCOL_VERSION}"),
        );
        dispatch_main()
    }
}

unsafe fn handle_request(event: XpcObject) {
    let remote = xpc_dictionary_get_remote_connection(event);
    let reply = xpc_dictionary_create_reply(event);
    if reply.is_null() {
        return;
    }
    let uid = if remote.is_null() {
        u32::MAX
    } else {
        xpc_connection_get_euid(remote) as u32
    };
    let operation = read_cstr(event, KEY_OP).unwrap_or(c"");

    let result = if operation == OP_STATUS {
        runtime_status(uid).map(|snapshot| (snapshot, None))
    } else if operation == OP_PREPARE {
        read_config(event).and_then(|config| {
            runtime_prepare(uid, config)?;
            runtime_status(uid).map(|snapshot| (snapshot, None))
        })
    } else if operation == OP_START {
        read_config(event).and_then(|config| {
            let lease =
                read_string(event, KEY_LEASE).ok_or_else(|| "缺少 TUN 运行 lease".to_string())?;
            validate_lease(&lease)?;
            runtime_start(uid, config, lease)?;
            runtime_status(uid).map(|snapshot| (snapshot, None))
        })
    } else if operation == OP_HEARTBEAT {
        let lease = read_string(event, KEY_LEASE).ok_or_else(|| "缺少 TUN 运行 lease".to_string());
        lease.and_then(|lease| {
            runtime_heartbeat(uid, &lease)?;
            runtime_status(uid).map(|snapshot| (snapshot, None))
        })
    } else if operation == OP_STOP {
        runtime_stop(uid).and_then(|_| runtime_status(uid).map(|snapshot| (snapshot, None)))
    } else if operation == OP_STOP_LEASE {
        let lease = read_string(event, KEY_LEASE).ok_or_else(|| "缺少 TUN 运行 lease".to_string());
        lease.and_then(|lease| {
            validate_lease(&lease)?;
            runtime_stop_lease(uid, &lease)?;
            runtime_status(uid).map(|snapshot| (snapshot, None))
        })
    } else if operation == OP_LOGS {
        let limit = xpc_dictionary_get_uint64(event, KEY_LIMIT.as_ptr()) as usize;
        runtime_status(uid).and_then(|snapshot| {
            let logs = runtime_logs(uid, limit)?;
            let json = serde_json::to_string(&logs).map_err(|error| error.to_string())?;
            Ok((snapshot, Some(json)))
        })
    } else {
        Err("未知的 TUN Helper 操作".to_string())
    };

    match result {
        Ok((snapshot, logs)) => {
            xpc_dictionary_set_int64(reply, KEY_STATUS.as_ptr(), STATUS_OK);
            write_snapshot(reply, &snapshot);
            if let Some(logs) = logs {
                set_string(reply, KEY_LOGS, &logs);
            }
        }
        Err(error) => {
            xpc_dictionary_set_int64(reply, KEY_STATUS.as_ptr(), STATUS_ERROR);
            set_string(reply, KEY_MESSAGE, &crate::runtime::redact(&error));
        }
    }
    if !remote.is_null() {
        xpc_connection_send_message(remote, reply);
    }
    xpc_release(reply);
}

fn runtime_prepare(uid: u32, source: &[u8]) -> Result<(), String> {
    validate_config(source)?;
    let mut guard = runtime()
        .lock()
        .map_err(|_| "TUN Helper 运行时锁损坏".to_string())?;
    prepare_locked(&mut guard, uid, source)
}

fn prepare_locked(guard: &mut HelperRuntime, uid: u32, source: &[u8]) -> Result<(), String> {
    guard.refresh_child();
    ensure_prepare_allowed(guard.child.is_some(), guard.owner_uid, uid)?;
    let directory = secure_runtime_directory(uid)?;
    seed_runtime_assets(uid, &directory)?;
    let core = stage_core(&directory)?;
    let config = write_config(&directory, source)?;
    native_validate(&core, &directory, &config)?;
    guard.config_path = Some(config);
    guard.version = core_version(&core).ok();
    guard.push_log("info", "helper", "TUN 配置已在特权运行目录完成预检");
    Ok(())
}

fn runtime_start(uid: u32, source: &[u8], lease: String) -> Result<(), String> {
    validate_config(source)?;
    let mut guard = runtime()
        .lock()
        .map_err(|_| "TUN Helper 运行时锁损坏".to_string())?;
    // Keep preparation and spawn one transaction. An overlapping prepare/start
    // must neither overwrite a running core nor swap its prepared configuration.
    prepare_locked(&mut guard, uid, source)?;
    if let Ok(mut logs) = guard.logs.lock() {
        logs.clear();
    }
    let config = guard
        .config_path
        .clone()
        .ok_or_else(|| "TUN 配置尚未准备".to_string())?;
    let directory = config
        .parent()
        .ok_or_else(|| "TUN 运行目录无效".to_string())?
        .to_path_buf();
    let core = directory.join(CORE_BINARY_NAME);
    codesign::validate(&core)?;
    let mut command = Command::new(&core);
    command
        .arg("-d")
        .arg(&directory)
        .arg("-f")
        .arg(&config)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|error| format!("启动特权 Mihomo 失败：{error}"))?;
    if let Some(stdout) = child.stdout.take() {
        spawn_log_reader(guard.logs.clone(), "stdout", stdout);
    }
    if let Some(stderr) = child.stderr.take() {
        spawn_log_reader(guard.logs.clone(), "stderr", stderr);
    }
    let pid = child.id();
    guard.child = Some(child);
    guard.owner_uid = Some(uid);
    guard.lease = Some(lease.clone());
    guard.last_heartbeat = Some(Instant::now());
    guard.last_error = None;
    guard.push_log("info", "helper", &format!("特权 Mihomo 已启动，PID {pid}"));
    drop(guard);

    std::thread::sleep(Duration::from_millis(950));
    let mut guard = runtime()
        .lock()
        .map_err(|_| "TUN Helper 运行时锁损坏".to_string())?;
    if guard.child.is_some() && !owns_lease(guard.owner_uid, guard.lease.as_deref(), uid, &lease) {
        return Err("TUN 会话已变化，已忽略旧会话的启动结果".to_string());
    }
    guard.refresh_child();
    if guard.child.is_none() {
        return Err(guard
            .last_error
            .clone()
            .unwrap_or_else(|| "特权 Mihomo 启动后立即退出".to_string()));
    }
    if let Some(error) = guard.recent_tun_error() {
        guard.last_error = Some(error.clone());
        guard.stop_child("TUN 初始化失败");
        return Err(error);
    }
    Ok(())
}

fn runtime_heartbeat(uid: u32, lease: &str) -> Result<(), String> {
    let mut guard = runtime()
        .lock()
        .map_err(|_| "TUN Helper 运行时锁损坏".to_string())?;
    guard.refresh_child();
    if guard.child.is_none() {
        return Err(guard
            .last_error
            .clone()
            .unwrap_or_else(|| "TUN 内核未运行".to_string()));
    }
    if guard.owner_uid != Some(uid) || guard.lease.as_deref() != Some(lease) {
        return Err("TUN lease 已失效".to_string());
    }
    guard.last_heartbeat = Some(Instant::now());
    Ok(())
}

fn runtime_stop(uid: u32) -> Result<(), String> {
    let mut guard = runtime()
        .lock()
        .map_err(|_| "TUN Helper 运行时锁损坏".to_string())?;
    guard.refresh_child();
    if guard.child.is_some() && guard.owner_uid != Some(uid) {
        return Err("另一个用户正在使用 TUN Helper".to_string());
    }
    guard.stop_child("应用请求停止 TUN");
    Ok(())
}

fn runtime_stop_lease(uid: u32, lease: &str) -> Result<(), String> {
    let mut guard = runtime()
        .lock()
        .map_err(|_| "TUN Helper 运行时锁损坏".to_string())?;
    guard.refresh_child();
    if guard.child.is_none() {
        return Ok(());
    }
    if !owns_lease(guard.owner_uid, guard.lease.as_deref(), uid, lease) {
        return Err("TUN 会话已变化，已忽略旧会话的停止请求".to_string());
    }
    guard.stop_child("拥有该会话的应用请求停止 TUN");
    Ok(())
}

fn owns_lease(owner_uid: Option<u32>, active_lease: Option<&str>, uid: u32, lease: &str) -> bool {
    owner_uid == Some(uid) && active_lease == Some(lease)
}

fn ensure_prepare_allowed(running: bool, owner_uid: Option<u32>, uid: u32) -> Result<(), String> {
    if !running {
        return Ok(());
    }
    if owner_uid != Some(uid) {
        return Err("另一个用户正在使用 TUN Helper".to_string());
    }
    Err("TUN 正在运行，请先停止后再预检或启动新会话；当前连接未更改".to_string())
}

fn runtime_status(uid: u32) -> Result<RuntimeSnapshot, String> {
    let mut guard = runtime()
        .lock()
        .map_err(|_| "TUN Helper 运行时锁损坏".to_string())?;
    guard.refresh_child();
    if guard.child.is_some() && guard.owner_uid != Some(uid) {
        return Err("另一个用户正在使用 TUN Helper".to_string());
    }
    Ok(guard.snapshot())
}

fn runtime_logs(uid: u32, limit: usize) -> Result<Vec<TunRuntimeLog>, String> {
    let guard = runtime()
        .lock()
        .map_err(|_| "TUN Helper 运行时锁损坏".to_string())?;
    if guard.child.is_some() && guard.owner_uid != Some(uid) {
        return Err("另一个用户正在使用 TUN Helper".to_string());
    }
    let logs = guard
        .logs
        .lock()
        .map_err(|_| "TUN Helper 日志锁损坏".to_string())?;
    Ok(logs
        .iter()
        .rev()
        .take(limit.clamp(1, MAX_LOG_LINES))
        .cloned()
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect())
}

impl HelperRuntime {
    fn snapshot(&self) -> RuntimeSnapshot {
        RuntimeSnapshot {
            running: self.child.is_some(),
            pid: self.child.as_ref().map(Child::id),
            version: self.version.clone(),
            config_path: self
                .config_path
                .as_ref()
                .map(|path| path.display().to_string()),
            last_error: self.last_error.clone(),
        }
    }

    fn refresh_child(&mut self) {
        let Some(child) = self.child.as_mut() else {
            return;
        };
        match child.try_wait() {
            Ok(Some(status)) => {
                let message = format!("特权 Mihomo 已退出：{status}");
                self.last_error = Some(message.clone());
                self.push_log("error", "helper", &message);
                self.child = None;
                self.owner_uid = None;
                self.lease = None;
                self.last_heartbeat = None;
            }
            Ok(None) => {
                if let Some(error) = self.recent_tun_error() {
                    self.last_error = Some(error);
                    self.stop_child("检测到 TUN 运行错误");
                }
            }
            Err(error) => {
                self.last_error = Some(error.to_string());
            }
        }
    }

    fn stop_child(&mut self, reason: &str) {
        if let Some(child) = self.child.as_mut() {
            let pid = child.id();
            let _ = child.kill();
            let _ = child.wait();
            self.push_log("info", "helper", &format!("{reason}，PID {pid}"));
        }
        self.child = None;
        self.owner_uid = None;
        self.lease = None;
        self.last_heartbeat = None;
    }

    fn recent_tun_error(&self) -> Option<String> {
        let logs = self.logs.lock().ok()?;
        logs.iter().rev().take(80).find_map(|log| {
            let lower = log.message.to_lowercase();
            (log.level == "error"
                && (lower.contains("tun")
                    || lower.contains("operation not permitted")
                    || lower.contains("permission denied")
                    || lower.contains("configure tun interface")))
            .then(|| log.message.clone())
        })
    }

    fn push_log(&self, level: &str, source: &str, message: &str) {
        push_log(&self.logs, level, source, message);
    }
}

fn start_watchdog() {
    std::thread::spawn(|| loop {
        std::thread::sleep(Duration::from_secs(2));
        let Ok(mut guard) = runtime().lock() else {
            continue;
        };
        guard.refresh_child();
        if guard.child.is_some()
            && guard
                .last_heartbeat
                .is_some_and(|heartbeat| heartbeat.elapsed() > LEASE_TIMEOUT)
        {
            guard.last_error = Some("TUN 控制端心跳超时，已安全停止内核".to_string());
            guard.stop_child("TUN 控制端心跳超时");
        }
    });
}

fn validate_config(source: &[u8]) -> Result<(), String> {
    if source.is_empty() || source.len() > MAX_CONFIG_BYTES {
        return Err(format!(
            "TUN 配置大小必须在 1 到 {MAX_CONFIG_BYTES} 字节之间"
        ));
    }
    if source.contains(&0) {
        return Err("TUN 配置包含 NUL 字节".to_string());
    }
    let source = std::str::from_utf8(source).map_err(|_| "TUN 配置不是 UTF-8".to_string())?;
    let document: Value = serde_yaml::from_str(source).map_err(|error| error.to_string())?;
    let root = document
        .as_mapping()
        .ok_or_else(|| "TUN 配置根节点必须是对象".to_string())?;
    for blocked in [
        "script",
        "listeners",
        "tunnels",
        "external-ui",
        "external-ui-url",
        "authentication",
    ] {
        if root.contains_key(Value::String(blocked.to_string())) {
            return Err(format!("特权 TUN 配置禁止字段：{blocked}"));
        }
    }
    if root
        .get(Value::String("allow-lan".to_string()))
        .and_then(Value::as_bool)
        != Some(false)
    {
        return Err("特权 TUN 配置必须关闭 allow-lan".to_string());
    }
    if root
        .get(Value::String("bind-address".to_string()))
        .and_then(Value::as_str)
        != Some("127.0.0.1")
    {
        return Err("特权 TUN 配置只能绑定 127.0.0.1".to_string());
    }
    let controller = root
        .get(Value::String("external-controller".to_string()))
        .and_then(Value::as_str)
        .ok_or_else(|| "特权 TUN 配置缺少本地控制地址".to_string())?;
    if !controller.starts_with("127.0.0.1:") {
        return Err("特权 TUN 控制端只能绑定 127.0.0.1".to_string());
    }
    let tun = root
        .get(Value::String("tun".to_string()))
        .and_then(Value::as_mapping)
        .ok_or_else(|| "特权 TUN 配置缺少 tun 节点".to_string())?;
    if tun
        .get(Value::String("enable".to_string()))
        .and_then(Value::as_bool)
        != Some(true)
        || tun
            .get(Value::String("auto-route".to_string()))
            .and_then(Value::as_bool)
            != Some(true)
    {
        return Err("特权 TUN 配置必须显式启用 TUN 与自动路由".to_string());
    }
    reject_unsafe_paths(&document, None)
}

fn reject_unsafe_paths(value: &Value, parent_key: Option<&str>) -> Result<(), String> {
    match value {
        Value::Mapping(mapping) => {
            for (key, child) in mapping {
                reject_unsafe_paths(child, key.as_str())?;
            }
        }
        Value::Sequence(sequence) => {
            for child in sequence {
                reject_unsafe_paths(child, parent_key)?;
            }
        }
        Value::String(text) => {
            let key = parent_key.unwrap_or_default().to_ascii_lowercase();
            let path_like = key.contains("path")
                || matches!(
                    key.as_str(),
                    "certificate" | "private-key" | "ca" | "client-certificate"
                );
            if path_like
                && (text.starts_with('/')
                    || text.starts_with('~')
                    || text.split(['/', '\\']).any(|segment| segment == ".."))
            {
                return Err(format!("特权 TUN 配置拒绝不安全路径字段：{key}"));
            }
        }
        _ => {}
    }
    Ok(())
}

fn secure_runtime_directory(uid: u32) -> Result<PathBuf, String> {
    let base = PathBuf::from(ROOT_RUNTIME_BASE);
    fs::create_dir_all(&base).map_err(|error| error.to_string())?;
    secure_directory(&base)?;
    let directory = base.join(uid.to_string());
    fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    secure_directory(&directory)?;
    Ok(directory)
}

fn secure_directory(path: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() || metadata.uid() != 0 {
        return Err(format!("拒绝不安全的特权运行目录：{}", path.display()));
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|error| error.to_string())
}

fn stage_core(directory: &Path) -> Result<PathBuf, String> {
    let source = codesign::sibling_executable(CORE_BINARY_NAME)?;
    codesign::validate(&source)?;
    let target = directory.join(CORE_BINARY_NAME);
    let temporary = directory.join(format!(".{CORE_BINARY_NAME}.next"));
    let _ = fs::remove_file(&temporary);
    let mut input = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&source)
        .map_err(|error| error.to_string())?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o700)
        .open(&temporary)
        .map_err(|error| error.to_string())?;
    std::io::copy(&mut input, &mut output).map_err(|error| error.to_string())?;
    output.sync_all().map_err(|error| error.to_string())?;
    drop(output);
    fs::set_permissions(&temporary, fs::Permissions::from_mode(0o500))
        .map_err(|error| error.to_string())?;
    codesign::validate(&temporary)?;
    if target.exists() {
        let metadata = fs::symlink_metadata(&target).map_err(|error| error.to_string())?;
        if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.uid() != 0 {
            let _ = fs::remove_file(&temporary);
            return Err("拒绝覆盖不安全的特权 Mihomo 文件".to_string());
        }
        fs::remove_file(&target).map_err(|error| error.to_string())?;
    }
    fs::rename(&temporary, &target).map_err(|error| error.to_string())?;
    Ok(target)
}

fn seed_runtime_assets(uid: u32, directory: &Path) -> Result<(), String> {
    let Some(home) = home_for_uid(uid) else {
        return Ok(());
    };
    let source_dir = home.join("Library/Application Support/com.cmmuu.mihomodesktop/runtime");
    for name in SEED_ASSETS {
        let source = source_dir.join(name);
        if !source.exists() {
            continue;
        }
        let metadata = fs::symlink_metadata(&source).map_err(|error| error.to_string())?;
        if metadata.file_type().is_symlink()
            || !metadata.is_file()
            || metadata.len() > 256 * 1024 * 1024
        {
            continue;
        }
        copy_regular_file(&source, &directory.join(name))?;
    }
    Ok(())
}

fn copy_regular_file(source: &Path, target: &Path) -> Result<(), String> {
    let mut input = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(source)
        .map_err(|error| error.to_string())?;
    let temporary = target.with_extension("next");
    let _ = fs::remove_file(&temporary);
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary)
        .map_err(|error| error.to_string())?;
    std::io::copy(&mut input, &mut output).map_err(|error| error.to_string())?;
    output.sync_all().map_err(|error| error.to_string())?;
    drop(output);
    if target.exists() {
        let metadata = fs::symlink_metadata(target).map_err(|error| error.to_string())?;
        if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.uid() != 0 {
            let _ = fs::remove_file(&temporary);
            return Err(format!("拒绝覆盖不安全的运行资产：{}", target.display()));
        }
        fs::remove_file(target).map_err(|error| error.to_string())?;
    }
    fs::rename(&temporary, target).map_err(|error| error.to_string())
}

fn write_config(directory: &Path, source: &[u8]) -> Result<PathBuf, String> {
    let target = directory.join("active.yaml");
    let temporary = directory.join(".active.yaml.next");
    let _ = fs::remove_file(&temporary);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary)
        .map_err(|error| error.to_string())?;
    file.write_all(source).map_err(|error| error.to_string())?;
    file.sync_all().map_err(|error| error.to_string())?;
    drop(file);
    if target.exists() {
        let metadata = fs::symlink_metadata(&target).map_err(|error| error.to_string())?;
        if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.uid() != 0 {
            let _ = fs::remove_file(&temporary);
            return Err("拒绝覆盖不安全的特权配置文件".to_string());
        }
        fs::remove_file(&target).map_err(|error| error.to_string())?;
    }
    fs::rename(&temporary, &target).map_err(|error| error.to_string())?;
    Ok(target)
}

fn native_validate(core: &Path, directory: &Path, config: &Path) -> Result<(), String> {
    let mut command = Command::new(core);
    command
        .arg("-t")
        .arg("-d")
        .arg(directory)
        .arg("-f")
        .arg(config);
    // Share the bounded private output file path used by ordinary validation.
    // Waiting for a process with unread stdout/stderr pipes can deadlock once
    // the native validator emits more than a pipe's capacity.
    crate::runtime::run_validation_command(&mut command, directory, VALIDATION_TIMEOUT)
        .map_err(|error| error.to_string())
}

fn core_version(core: &Path) -> Result<String, String> {
    let output = Command::new(core)
        .arg("-v")
        .output()
        .map_err(|error| error.to_string())?;
    let text = if output.stdout.is_empty() {
        &output.stderr
    } else {
        &output.stdout
    };
    let text = String::from_utf8_lossy(text);
    text.lines()
        .find(|line| !line.trim().is_empty())
        .map(str::to_string)
        .ok_or_else(|| "Mihomo 未返回版本信息".to_string())
}

fn home_for_uid(uid: u32) -> Option<PathBuf> {
    unsafe {
        let mut password: libc::passwd = std::mem::zeroed();
        let mut result: *mut libc::passwd = std::ptr::null_mut();
        let mut buffer = vec![0_i8; 16 * 1024];
        if libc::getpwuid_r(
            uid,
            &mut password,
            buffer.as_mut_ptr(),
            buffer.len(),
            &mut result,
        ) != 0
            || result.is_null()
            || password.pw_dir.is_null()
        {
            return None;
        }
        Some(PathBuf::from(
            CStr::from_ptr(password.pw_dir)
                .to_string_lossy()
                .into_owned(),
        ))
    }
}

fn validate_lease(lease: &str) -> Result<(), String> {
    if lease.len() != 64 || !lease.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("TUN lease 必须是 64 位十六进制字符串".to_string());
    }
    Ok(())
}

fn spawn_log_reader<R: Read + Send + 'static>(
    logs: Arc<Mutex<VecDeque<TunRuntimeLog>>>,
    source: &'static str,
    reader: R,
) {
    std::thread::spawn(move || {
        for line in BufReader::new(reader).lines().map_while(Result::ok) {
            let lower = line.to_lowercase();
            let level = if lower.contains("level=error") || lower.contains("level=fatal") {
                "error"
            } else if lower.contains("level=warn") {
                "warn"
            } else {
                "info"
            };
            push_log(&logs, level, source, &crate::runtime::redact(&line));
        }
    });
}

fn push_log(logs: &Arc<Mutex<VecDeque<TunRuntimeLog>>>, level: &str, source: &str, message: &str) {
    if let Ok(mut logs) = logs.lock() {
        logs.push_back(TunRuntimeLog {
            timestamp: Utc::now(),
            level: level.to_string(),
            source: source.to_string(),
            message: message.to_string(),
        });
        while logs.len() > MAX_LOG_LINES {
            logs.pop_front();
        }
    }
}

unsafe fn read_config<'a>(event: XpcObject) -> Result<&'a [u8], String> {
    let mut length = 0_usize;
    let data = xpc_dictionary_get_data(event, KEY_CONFIG.as_ptr(), &mut length);
    if data.is_null() {
        return Err("缺少 TUN 配置".to_string());
    }
    if length > MAX_CONFIG_BYTES {
        return Err("TUN 配置超过大小限制".to_string());
    }
    Ok(std::slice::from_raw_parts(data.cast::<u8>(), length))
}

unsafe fn read_cstr<'a>(dictionary: XpcObject, key: &CStr) -> Option<&'a CStr> {
    let value = xpc_dictionary_get_string(dictionary, key.as_ptr());
    (!value.is_null()).then(|| CStr::from_ptr(value))
}

unsafe fn read_string(dictionary: XpcObject, key: &CStr) -> Option<String> {
    read_cstr(dictionary, key).map(|value| value.to_string_lossy().into_owned())
}

unsafe fn write_snapshot(reply: XpcObject, snapshot: &RuntimeSnapshot) {
    xpc_dictionary_set_uint64(
        reply,
        KEY_PROTOCOL_VERSION.as_ptr(),
        PROTOCOL_VERSION as u64,
    );
    xpc_dictionary_set_bool(reply, KEY_RUNNING.as_ptr(), snapshot.running);
    xpc_dictionary_set_uint64(
        reply,
        KEY_PID.as_ptr(),
        snapshot.pid.unwrap_or_default() as u64,
    );
    if let Some(version) = snapshot.version.as_deref() {
        set_string(reply, KEY_VERSION, version);
    }
    if let Some(path) = snapshot.config_path.as_deref() {
        set_string(reply, KEY_CONFIG_PATH, path);
    }
    if let Some(error) = snapshot.last_error.as_deref() {
        set_string(reply, KEY_LAST_ERROR, error);
    }
}

unsafe fn set_string(dictionary: XpcObject, key: &CStr, value: &str) {
    if let Ok(value) = CString::new(value) {
        xpc_dictionary_set_string(dictionary, key.as_ptr(), value.as_ptr());
    }
}

extern "C" {
    fn syslog(priority: i32, format: *const i8, ...);
}

fn daemon_log(priority: i32, message: &str) {
    eprintln!("{message}");
    if let Ok(message) = CString::new(message) {
        unsafe { syslog(priority, c"%s".as_ptr(), message.as_ptr()) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn safe_config() -> Vec<u8> {
        br#"mixed-port: 7895
allow-lan: false
bind-address: 127.0.0.1
external-controller: 127.0.0.1:9090
secret: local-secret
tun:
  enable: true
  auto-route: true
proxies: []
proxy-groups: []
rules: []
"#
        .to_vec()
    }

    #[test]
    fn privileged_config_requires_tun_and_loopback_controls() {
        assert!(validate_config(&safe_config()).is_ok());
        let unsafe_config = String::from_utf8(safe_config())
            .unwrap()
            .replace("allow-lan: false", "allow-lan: true");
        assert!(validate_config(unsafe_config.as_bytes()).is_err());
    }

    #[test]
    fn privileged_config_rejects_scripts_and_absolute_paths() {
        let mut script = safe_config();
        script.extend_from_slice(b"script: { code: test }\n");
        assert!(validate_config(&script).is_err());

        let mut path = safe_config();
        path.extend_from_slice(b"rule-providers:\n  unsafe:\n    path: /etc/passwd\n");
        assert!(validate_config(&path).is_err());
    }

    #[test]
    fn lease_is_bounded_hex() {
        assert!(validate_lease(&"a".repeat(64)).is_ok());
        assert!(validate_lease("not-a-lease").is_err());
    }

    #[test]
    fn late_stop_lease_does_not_match_replacement_or_other_user() {
        let old = "a".repeat(64);
        let current = "b".repeat(64);
        assert!(!owns_lease(Some(501), Some(&current), 501, &old));
        assert!(!owns_lease(Some(501), Some(&current), 502, &current));
        assert!(!owns_lease(None, None, 501, &current));
        assert!(owns_lease(Some(501), Some(&current), 501, &current));
    }

    #[test]
    fn prepare_requires_idle_helper_even_for_same_user() {
        assert!(ensure_prepare_allowed(false, None, 501).is_ok());
        assert!(ensure_prepare_allowed(true, Some(501), 501).is_err());
        assert!(ensure_prepare_allowed(true, Some(502), 501).is_err());
    }

    #[test]
    fn native_validation_drains_large_stdout_and_stderr_without_pipe_stall() {
        let directory =
            std::env::temp_dir().join(format!("serylane-helper-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&directory).unwrap();
        let fixture = directory.join("fixture-core");
        fs::write(
            &fixture,
            b"#!/bin/sh\nhead -c 196608 /dev/zero\nhead -c 196608 /dev/zero >&2\nexit 0\n",
        )
        .unwrap();
        fs::set_permissions(&fixture, fs::Permissions::from_mode(0o700)).unwrap();
        let started = Instant::now();
        let result = native_validate(&fixture, &directory, &directory.join("unused.yaml"));
        let files: Vec<_> = fs::read_dir(&directory).unwrap().collect();
        fs::remove_dir_all(&directory).unwrap();
        assert!(result.is_ok(), "{result:?}");
        assert!(started.elapsed() < Duration::from_secs(10));
        assert_eq!(
            files.len(),
            1,
            "private validation output should be cleaned up"
        );
    }
}
