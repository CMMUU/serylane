//! Bounded, local-only diagnostic events. Never pass request bodies, headers,
//! configuration YAML, subscription URLs or model output to this journal.
use crate::{
    error::{AppError, AppResult},
    storage::write_private_atomic,
};
use chrono::Utc;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::{
    collections::VecDeque,
    fs,
    path::PathBuf,
    sync::{Mutex, OnceLock},
};

const DAY_MS: i64 = 24 * 60 * 60 * 1000;
#[cfg(test)]
const RETENTION_MS: i64 = 3 * DAY_MS;
const MAX_BYTES: usize = 256 * 1024;
const MAX_ENTRIES: usize = 2000;
static JOURNAL: OnceLock<Mutex<Journal>> = OnceLock::new();

#[derive(Clone, Copy)]
pub enum Area {
    App = 0,
    Runtime = 1,
    Network = 2,
    Restore = 3,
    Stability = 4,
    Settings = 5,
    CoreNetwork = 6,
    Routing = 7,
}

// Compact on-disk tuple: first occurrence of a <=60s group, severity, area,
// message, count. Expire by its first timestamp so aggregation cannot retain
// expired events indefinitely during a repeated failure.
#[derive(Clone, Serialize, Deserialize)]
struct Entry(i64, u8, u8, String, u32);

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogEntry {
    timestamp: i64,
    level: &'static str,
    source: &'static str,
    message: String,
    count: u32,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    entries: Vec<LogEntry>,
    retention_hours: u32,
    max_bytes: usize,
    storage_error: Option<String>,
}

struct Journal {
    path: PathBuf,
    retention_days: u16,
    entries: VecDeque<Entry>,
    storage_error: Option<String>,
}

impl Journal {
    fn open(path: PathBuf, now: i64, retention_days: u16) -> Self {
        let loaded = (|| -> AppResult<VecDeque<Entry>> {
            if !path.exists() {
                return Ok(VecDeque::new());
            }
            if fs::metadata(&path)?.len() > MAX_BYTES as u64 {
                return Err(AppError::Io("日志文件超过容量限制".into()));
            }
            serde_json::from_slice(&fs::read(&path)?)
                .map_err(|_| AppError::Io("日志文件损坏".into()))
        })();
        let (entries, storage_error) = match loaded {
            Ok(entries) => (entries, None),
            Err(_) => (
                VecDeque::new(),
                Some("旧日志无法读取；已建立新的有界日志，不影响代理运行".into()),
            ),
        };
        let mut journal = Self {
            path,
            retention_days: retention_days.clamp(1, 90),
            entries,
            storage_error,
        };
        if journal.prune(now) {
            journal.persist();
        }
        journal
    }

    fn prune(&mut self, now: i64) -> bool {
        let before = self.entries.len();
        let retention_ms = i64::from(self.retention_days) * DAY_MS;
        self.entries.retain(|entry| {
            entry.0 >= now.saturating_sub(retention_ms) && entry.0 <= now.saturating_add(60_000)
        });
        while self.entries.len() > MAX_ENTRIES {
            self.entries.pop_front();
        }
        before != self.entries.len()
    }

    fn persist(&mut self) {
        let result = (|| -> AppResult<()> {
            let mut bytes =
                serde_json::to_vec(&self.entries).map_err(|e| AppError::Io(e.to_string()))?;
            while bytes.len() > MAX_BYTES && !self.entries.is_empty() {
                self.entries.pop_front();
                bytes =
                    serde_json::to_vec(&self.entries).map_err(|e| AppError::Io(e.to_string()))?;
            }
            write_private_atomic(&self.path, &bytes)
        })();
        if result.is_err() {
            self.storage_error =
                Some("应用日志写入失败，当前仅在内存中保留；请检查磁盘空间或目录权限".into());
        }
    }

    fn record(&mut self, now: i64, level: u8, area: Area, message: &str) {
        self.prune(now);
        let message = sanitize(message);
        if let Some(last) = self.entries.back_mut().filter(|last| {
            last.1 == level
                && last.2 == area as u8
                && last.3 == message
                && (0..60_000).contains(&(now - last.0))
        }) {
            last.4 = last.4.saturating_add(1);
        } else {
            self.entries
                .push_back(Entry(now, level, area as u8, message, 1));
        }
        self.prune(now);
        self.persist();
    }

    fn snapshot(&mut self, now: i64) -> Snapshot {
        if self.prune(now) {
            self.persist();
        }
        Snapshot {
            entries: self
                .entries
                .iter()
                .rev()
                .map(|e| LogEntry {
                    timestamp: e.0,
                    level: match e.1 {
                        2 => "error",
                        1 => "warn",
                        _ => "info",
                    },
                    source: match e.2 {
                        1 => "运行",
                        2 => "网络预检",
                        3 => "状态恢复",
                        4 => "节点稳定性",
                        5 => "设置",
                        6 => "核心网络",
                        7 => "本地路由",
                        _ => "应用",
                    },
                    message: sanitize(&e.3),
                    count: e.4,
                })
                .collect(),
            retention_hours: u32::from(self.retention_days) * 24,
            max_bytes: MAX_BYTES,
            storage_error: self.storage_error.clone(),
        }
    }
}

pub fn initialize(path: PathBuf, retention_days: u16) {
    let _ = JOURNAL.set(Mutex::new(Journal::open(
        path,
        Utc::now().timestamp_millis(),
        retention_days,
    )));
}

pub fn validate_retention_days(days: u16) -> AppResult<()> {
    if !(1..=90).contains(&days) {
        return Err(AppError::InvalidInput(
            "应用日志保留天数必须在 1 到 90 之间".into(),
        ));
    }
    Ok(())
}

// Called only after settings were persisted. A journal write failure is exposed
// by Snapshot.storage_error, not a reason to roll back unrelated preferences.
pub fn set_retention_days(days: u16) {
    if let Some(mut journal) = JOURNAL.get().and_then(|j| j.lock().ok()) {
        journal.retention_days = days.clamp(1, 90);
        if journal.prune(Utc::now().timestamp_millis()) {
            journal.persist();
        }
    }
}

pub fn record(level: u8, area: Area, message: &str) {
    if let Some(mut journal) = JOURNAL.get().and_then(|j| j.lock().ok()) {
        journal.record(Utc::now().timestamp_millis(), level, area, message);
    }
}

// Persist only an allowlisted category, never a raw core traffic line. Such
// lines can contain node credentials, request URLs and private destination IPs.
pub(crate) fn core_network_summary(level: &str, message: &str) -> Option<&'static str> {
    if !matches!(level, "warn" | "warning" | "error") {
        return None;
    }
    let message: String = message.chars().take(4096).collect();
    let lower = message.to_ascii_lowercase();
    if lower.contains("certificate") || lower.contains("x509") {
        Some("核心 TLS/证书校验失败；请核对时间、节点证书或网络检查软件")
    } else if lower.contains("no such host")
        || lower.contains("dns resolve")
        || lower.contains("dns lookup")
    {
        Some("核心 DNS 解析失败；请检查节点域名和订阅 DNS 配置")
    } else if lower.contains("timeout") || lower.contains("timed out") {
        Some("核心网络连接或读取超时；不等同于模型流中断")
    } else if lower.contains("connection reset")
        || lower.contains("broken pipe")
        || lower.contains("unexpected eof")
    {
        Some("核心连接被重置或提前关闭；不等同于模型流中断")
    } else if lower.contains("connection refused") || lower.contains("actively refused") {
        Some("核心连接被拒绝；请核对节点服务、端口及本机防火墙")
    } else if level == "error" {
        Some("核心报告错误；请结合当前会话的 Mihomo 日志检查（应用日志不持久化原始流量明细）")
    } else {
        None
    }
}

pub fn prune() {
    if let Some(mut journal) = JOURNAL.get().and_then(|j| j.lock().ok()) {
        if journal.prune(Utc::now().timestamp_millis()) {
            journal.persist();
        }
    }
}

pub fn snapshot() -> AppResult<Snapshot> {
    JOURNAL
        .get()
        .and_then(|j| j.lock().ok())
        .map(|mut journal| journal.snapshot(Utc::now().timestamp_millis()))
        .ok_or_else(|| AppError::Io("应用日志尚未初始化".into()))
}

pub fn clear() -> AppResult<()> {
    let mut journal = JOURNAL
        .get()
        .and_then(|j| j.lock().ok())
        .ok_or_else(|| AppError::Io("应用日志尚未初始化".into()))?;
    // Only the owned journal is replaced; subscriptions and core logs are untouched.
    write_private_atomic(&journal.path, b"[]")?;
    journal.entries.clear();
    journal.storage_error = None;
    Ok(())
}

pub(crate) fn sanitize(text: &str) -> String {
    static URL: OnceLock<Regex> = OnceLock::new();
    static CREDENTIAL: OnceLock<Regex> = OnceLock::new();
    static PATH: OnceLock<Regex> = OnceLock::new();
    let text: String = text.chars().take(4096).collect();
    let text = URL
        .get_or_init(|| Regex::new(r"(?i)(?:https?|socks5h?)://\S+").unwrap())
        .replace_all(&text, "[地址已隐藏]");
    let text = CREDENTIAL.get_or_init(|| Regex::new(r#"(?i)(?:bearer\s+\S+|(?:authorization|api[_-]?key|token|secret|password|passwd)["']?\s*[:=]\s*["']?[^\s"',}]+|sk-[a-zA-Z0-9_-]+)"#).unwrap()).replace_all(&text, "[凭据已隐藏]");
    let text = PATH
        .get_or_init(|| Regex::new(r#"(?:[A-Za-z]:\\|/(?:Users|home)/)[^\r\n"<>]+"#).unwrap())
        .replace_all(&text, "[本地路径已隐藏]");
    text.chars().filter(|c| !c.is_control()).take(640).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn compact_roundtrip_dedup_and_exact_three_day_expiry() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log.json");
        let now = RETENTION_MS * 2;
        let mut journal = Journal::open(path.clone(), now, 3);
        journal.record(now, 0, Area::App, "应用已启动");
        journal.record(now + 1, 0, Area::App, "应用已启动");
        let mut reopened = Journal::open(path.clone(), now + 1, 3);
        assert_eq!(reopened.snapshot(now + 1).entries[0].count, 2);
        assert!(!fs::read_to_string(&path).unwrap().contains("timestamp"));
        assert!(reopened.snapshot(now + 2 + RETENTION_MS).entries.is_empty());
        assert_eq!(fs::read_to_string(path).unwrap(), "[]");
    }

    #[test]
    fn continuous_repeats_cannot_keep_old_events_alive_forever() {
        let dir = tempfile::tempdir().unwrap();
        let mut journal = Journal::open(dir.path().join("log.json"), 0, 3);
        journal.record(1, 2, Area::App, "同一错误");
        journal.record(59_999, 2, Area::App, "同一错误");
        journal.record(60_001, 2, Area::App, "同一错误");
        assert_eq!(journal.entries.len(), 2);
        assert_eq!(journal.entries[0].4, 2);
        assert_eq!(journal.snapshot(RETENTION_MS + 2).entries.len(), 1);
    }
    #[test]
    fn capacity_corruption_clear_and_secrets_are_bounded() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log.json");
        let mut journal = Journal::open(path.clone(), 10, 3);
        for n in 0..450 {
            journal
                .entries
                .push_back(Entry(10, 2, 2, format!("{n}{}", "错".repeat(640)), 1));
        }
        journal.persist();
        assert!(fs::metadata(&path).unwrap().len() <= MAX_BYTES as u64);
        assert!(journal.entries.len() < 450);
        let text = sanitize("https://user:pass@example.com/private-secret token: abc Bearer XYZ sk-test-secret C:\\Users\\Alice\\secret.json");
        for secret in ["private-secret", "abc", "XYZ", "sk-test", "Alice"] {
            assert!(!text.contains(secret));
        }
        fs::write(&path, b"broken").unwrap();
        assert!(Journal::open(path, 10, 3).storage_error.is_some());
    }

    #[test]
    fn configured_retention_survives_reopen_and_shortening_prunes_immediately() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log.json");
        let mut journal = Journal::open(path.clone(), 0, 7);
        journal.record(1, 0, Area::App, "第一天");
        journal.record(4 * DAY_MS, 0, Area::App, "第五天");
        let mut journal = Journal::open(path.clone(), 4 * DAY_MS, 7);
        assert_eq!(journal.snapshot(4 * DAY_MS).entries.len(), 2);
        assert_eq!(journal.snapshot(4 * DAY_MS).retention_hours, 168);
        journal.retention_days = 1;
        assert_eq!(journal.snapshot(4 * DAY_MS).entries.len(), 1);
        let mut journal = Journal::open(path, 4 * DAY_MS, 90);
        assert_eq!(journal.snapshot(4 * DAY_MS).entries.len(), 1);
        assert_eq!(journal.snapshot(4 * DAY_MS).max_bytes, MAX_BYTES);
        for days in [1, 3, 90] {
            assert!(validate_retention_days(days).is_ok());
        }
        for days in [0, 91, u16::MAX] {
            assert!(validate_retention_days(days).is_err());
        }
    }

    #[test]
    fn core_log_summary_does_not_persist_traffic_or_claim_model_failure() {
        let raw =
            "level=warning [TCP] dial private.example:443 error: i/o timeout token=private-key";
        let summary = core_network_summary("warn", raw).unwrap();
        assert!(summary.contains("超时"));
        assert!(summary.contains("不等同于模型流中断"));
        assert!(!summary.contains("private"));
        assert!(core_network_summary("info", raw).is_none());
        assert!(core_network_summary("warn", "regular warning").is_none());
        assert!(core_network_summary("error", "unknown failure private-key")
            .unwrap()
            .contains("核心报告错误"));
    }
}
