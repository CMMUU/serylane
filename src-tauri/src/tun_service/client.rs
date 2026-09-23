use super::codesign;
use super::lifecycle::{ProbeRead, SharedProbe};
use super::protocol::*;
use super::xpc::*;
use super::{TunRuntimeLog, TunRuntimeStart};
use block2::RcBlock;
use std::ffi::{c_void, CStr, CString};
use std::sync::OnceLock;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct RuntimeSnapshot {
    pub protocol_version: u32,
    pub running: bool,
    pub pid: Option<u32>,
    pub version: Option<String>,
    pub config_path: Option<String>,
    pub last_error: Option<String>,
}

fn probe() -> &'static SharedProbe<RuntimeSnapshot> {
    static PROBE: OnceLock<SharedProbe<RuntimeSnapshot>> = OnceLock::new();
    PROBE.get_or_init(SharedProbe::default)
}

pub fn reset_probe_gate() {
    probe().reset();
}

pub fn status_probe() -> ProbeRead<RuntimeSnapshot> {
    probe().read(
        Duration::from_secs(3),
        Duration::from_millis(250),
        status_sync,
    )
}

pub fn prepare(source: &str) -> Result<(), String> {
    request(OP_PREPARE, |message| unsafe {
        xpc_dictionary_set_data(
            message,
            KEY_CONFIG.as_ptr(),
            source.as_ptr().cast::<c_void>(),
            source.len(),
        );
    })
    .map(|_| ())
}

pub fn start(source: &str, lease: &str) -> Result<TunRuntimeStart, String> {
    let lease = CString::new(lease).map_err(|_| "TUN lease 格式无效".to_string())?;
    let snapshot = request(OP_START, |message| unsafe {
        xpc_dictionary_set_data(
            message,
            KEY_CONFIG.as_ptr(),
            source.as_ptr().cast::<c_void>(),
            source.len(),
        );
        xpc_dictionary_set_string(message, KEY_LEASE.as_ptr(), lease.as_ptr());
    })?;
    let pid = snapshot
        .pid
        .ok_or_else(|| "TUN Helper 启动后没有返回 PID".to_string())?;
    Ok(TunRuntimeStart {
        pid,
        version: snapshot.version,
        config_path: snapshot.config_path.unwrap_or_default(),
    })
}

pub fn heartbeat(lease: &str) -> Result<(), String> {
    let lease = CString::new(lease).map_err(|_| "TUN lease 格式无效".to_string())?;
    request(OP_HEARTBEAT, |message| unsafe {
        xpc_dictionary_set_string(message, KEY_LEASE.as_ptr(), lease.as_ptr());
    })
    .map(|_| ())
}

pub fn stop() -> Result<(), String> {
    request(OP_STOP, |_| {}).map(|_| ())
}

pub fn stop_lease(lease: &str) -> Result<(), String> {
    let lease = CString::new(lease).map_err(|_| "TUN lease 格式无效".to_string())?;
    // A separate operation fails closed against old Helpers instead of being
    // interpreted as their unconditional stop operation.
    request(OP_STOP_LEASE, |message| unsafe {
        xpc_dictionary_set_string(message, KEY_LEASE.as_ptr(), lease.as_ptr());
    })
    .map(|_| ())
}

pub fn logs(limit: usize) -> Result<Vec<TunRuntimeLog>, String> {
    with_connection(|connection| unsafe {
        let message = new_message(OP_LOGS)?;
        xpc_dictionary_set_uint64(message, KEY_LIMIT.as_ptr(), limit.clamp(1, 2_000) as u64);
        let reply = xpc_connection_send_message_with_reply_sync(connection, message);
        xpc_release(message);
        let result = (|| {
            ensure_success(reply)?;
            let raw = read_string(reply, KEY_LOGS).unwrap_or_else(|| "[]".to_string());
            serde_json::from_str(&raw).map_err(|error| format!("解析 Helper 日志失败：{error}"))
        })();
        if !reply.is_null() {
            xpc_release(reply);
        }
        result
    })
}

fn status_sync() -> Result<RuntimeSnapshot, String> {
    request(OP_STATUS, |_| {})
}

fn request(operation: &CStr, configure: impl FnOnce(XpcObject)) -> Result<RuntimeSnapshot, String> {
    with_connection(|connection| unsafe {
        let message = new_message(operation)?;
        configure(message);
        let reply = xpc_connection_send_message_with_reply_sync(connection, message);
        xpc_release(message);
        let result = parse_snapshot(reply);
        if !reply.is_null() {
            xpc_release(reply);
        }
        result
    })
}

fn with_connection<T>(
    operation: impl FnOnce(XpcConnection) -> Result<T, String>,
) -> Result<T, String> {
    let helper = codesign::sibling_executable(HELPER_BINARY_NAME)?;
    let requirement = codesign::designated_requirement(&helper)?;
    let requirement =
        CString::new(requirement).map_err(|_| "Helper 代码签名要求包含非法字符".to_string())?;
    let name = CString::new(LABEL).map_err(|_| "Helper 服务名无效".to_string())?;
    unsafe {
        let connection = xpc_connection_create_mach_service(
            name.as_ptr(),
            std::ptr::null_mut(),
            XPC_CONNECTION_MACH_SERVICE_PRIVILEGED,
        );
        if connection.is_null() {
            return Err("连接 TUN Helper 失败".to_string());
        }
        let handler = RcBlock::new(move |_event: XpcObject| {});
        xpc_connection_set_event_handler(connection, &handler);
        let requirement_status = set_peer_requirement(connection, requirement.as_ptr());
        if requirement_status != 0 {
            xpc_connection_cancel(connection);
            xpc_release(connection);
            return Err(format!(
                "设置 TUN Helper 身份校验失败：OSStatus {requirement_status}"
            ));
        }
        xpc_connection_resume(connection);
        let result = operation(connection);
        xpc_connection_cancel(connection);
        xpc_release(connection);
        result
    }
}

unsafe fn new_message(operation: &CStr) -> Result<XpcObject, String> {
    let message = xpc_dictionary_create(std::ptr::null(), std::ptr::null(), 0);
    if message.is_null() {
        return Err("创建 TUN Helper 请求失败".to_string());
    }
    xpc_dictionary_set_string(message, KEY_OP.as_ptr(), operation.as_ptr());
    Ok(message)
}

unsafe fn ensure_success(reply: XpcObject) -> Result<(), String> {
    if reply.is_null() {
        return Err("TUN Helper 没有返回结果".to_string());
    }
    if is_type(reply, type_error()) {
        return Err("TUN Helper 连接被拒绝或服务未运行".to_string());
    }
    if !is_type(reply, type_dictionary()) {
        return Err("TUN Helper 返回了未知数据".to_string());
    }
    if xpc_dictionary_get_int64(reply, KEY_STATUS.as_ptr()) == STATUS_OK {
        return Ok(());
    }
    Err(read_string(reply, KEY_MESSAGE).unwrap_or_else(|| "TUN Helper 执行失败".to_string()))
}

unsafe fn parse_snapshot(reply: XpcObject) -> Result<RuntimeSnapshot, String> {
    ensure_success(reply)?;
    let pid = xpc_dictionary_get_uint64(reply, KEY_PID.as_ptr());
    Ok(RuntimeSnapshot {
        protocol_version: xpc_dictionary_get_uint64(reply, KEY_PROTOCOL_VERSION.as_ptr()) as u32,
        running: xpc_dictionary_get_bool(reply, KEY_RUNNING.as_ptr()),
        pid: u32::try_from(pid).ok().filter(|value| *value > 0),
        version: read_string(reply, KEY_VERSION),
        config_path: read_string(reply, KEY_CONFIG_PATH),
        last_error: read_string(reply, KEY_LAST_ERROR),
    })
}

unsafe fn read_string(dictionary: XpcObject, key: &CStr) -> Option<String> {
    let value = xpc_dictionary_get_string(dictionary, key.as_ptr());
    (!value.is_null()).then(|| CStr::from_ptr(value).to_string_lossy().into_owned())
}
