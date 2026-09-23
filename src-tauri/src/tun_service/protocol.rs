use std::ffi::CStr;

pub const LABEL: &str = "com.cmmuu.mihomodesktop.tun-helper";
pub const PLIST_NAME: &CStr = c"com.cmmuu.mihomodesktop.tun-helper.plist";
pub const APP_BINARY_NAME: &str = "serylane";
pub const HELPER_BINARY_NAME: &str = "mihomo-tun-helper";
pub const CORE_BINARY_NAME: &str = "mihomo";
// v2 requires lease-scoped stop and refuses preparation over an active core.
pub const PROTOCOL_VERSION: u32 = 2;
pub const MAX_CONFIG_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_LOG_LINES: usize = 2_000;

pub const KEY_OP: &CStr = c"op";
pub const KEY_CONFIG: &CStr = c"config";
pub const KEY_LEASE: &CStr = c"lease";
pub const KEY_LIMIT: &CStr = c"limit";
pub const KEY_STATUS: &CStr = c"status";
pub const KEY_MESSAGE: &CStr = c"message";
pub const KEY_PROTOCOL_VERSION: &CStr = c"protocol_version";
pub const KEY_RUNNING: &CStr = c"running";
pub const KEY_PID: &CStr = c"pid";
pub const KEY_VERSION: &CStr = c"version";
pub const KEY_CONFIG_PATH: &CStr = c"config_path";
pub const KEY_LAST_ERROR: &CStr = c"last_error";
pub const KEY_LOGS: &CStr = c"logs";

pub const OP_STATUS: &CStr = c"status";
pub const OP_PREPARE: &CStr = c"prepare";
pub const OP_START: &CStr = c"start";
pub const OP_HEARTBEAT: &CStr = c"heartbeat";
pub const OP_STOP: &CStr = c"stop";
pub const OP_STOP_LEASE: &CStr = c"stop_lease";
pub const OP_LOGS: &CStr = c"logs";

pub const STATUS_OK: i64 = 0;
pub const STATUS_ERROR: i64 = 1;
