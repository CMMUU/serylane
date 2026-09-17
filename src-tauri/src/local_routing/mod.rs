//! Opt-in local model routing. Saving preferences never launches or takes over an application.
pub(crate) mod codex;
mod files;
mod server;

use crate::{
    error::{AppError, AppErrorDto, AppResult},
    storage::AppStorage,
};
use serde::{Deserialize, Serialize};
use std::{
    path::Path,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Mutex,
    },
};
use tauri::{AppHandle, Manager, State};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RouteMode {
    Native,
    #[default]
    Compatible,
}
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Upstream {
    #[default]
    Chatgpt,
    OpenaiApi,
}
impl Upstream {
    pub fn target(self) -> crate::route_health::Target {
        match self {
            Self::Chatgpt => crate::route_health::Target::Chatgpt,
            Self::OpenaiApi => crate::route_health::Target::OpenaiApi,
        }
    }
    pub fn base_url(self) -> &'static str {
        match self {
            Self::Chatgpt => "https://chatgpt.com/backend-api/codex",
            Self::OpenaiApi => "https://api.openai.com/v1",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RouteSettings {
    pub listen_port: u16,
    pub mode: RouteMode,
    pub upstream: Upstream,
    /// Empty means the current RouteDeck mixed port. Never inherit ambient proxy settings.
    pub outbound_proxy: String,
}
impl Default for RouteSettings {
    fn default() -> Self {
        Self {
            listen_port: 15731,
            mode: RouteMode::Compatible,
            upstream: Upstream::Chatgpt,
            outbound_proxy: String::new(),
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RouteDocument {
    schema_version: u32,
    revision: u64,
    enabled: bool,
    pub settings: RouteSettings,
    pub access_token: String,
}
impl Default for RouteDocument {
    fn default() -> Self {
        Self {
            schema_version: 1,
            revision: 0,
            enabled: false,
            settings: RouteSettings::default(),
            access_token: Uuid::new_v4().simple().to_string(),
        }
    }
}
impl RouteDocument {
    fn read(dir: &Path) -> AppResult<Self> {
        let Some(text) = files::read_optional(&dir.join("settings.json"))? else {
            return Ok(Self::default());
        };
        let doc: Self = serde_json::from_str(&text)
            .map_err(|_| AppError::Config("路由设置损坏，未自动重置".into()))?;
        if doc.schema_version != 1
            || doc.access_token.len() != 32
            || !doc.access_token.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err(AppError::Config("路由设置版本或访问令牌无效".into()));
        }
        Ok(doc)
    }
    fn write(&self, dir: &Path) -> AppResult<()> {
        files::private_directory(dir)?;
        files::atomic_write(
            &dir.join("settings.json"),
            &serde_json::to_string_pretty(self)
                .map_err(|_| AppError::Config("无法保存路由设置".into()))?,
        )
    }
    pub fn proxy_url(&self, mixed: u16) -> AppResult<String> {
        if self.settings.listen_port < 1024 || self.settings.listen_port == mixed {
            return Err(AppError::InvalidInput(
                "路由端口应为 1024–65535，且不能与代理端口相同".into(),
            ));
        }
        let value = if self.settings.outbound_proxy.is_empty() {
            format!("http://127.0.0.1:{mixed}")
        } else {
            self.settings.outbound_proxy.clone()
        };
        let url = url::Url::parse(&value)
            .map_err(|_| AppError::InvalidInput("出站代理地址无效".into()))?;
        let local = matches!(
            url.host_str(),
            Some("127.0.0.1" | "[::1]" | "::1" | "localhost")
        );
        if !local
            || !matches!(url.scheme(), "http" | "https" | "socks5" | "socks5h")
            || url.port().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || !matches!(url.path(), "" | "/")
            || url.port() == Some(self.settings.listen_port)
        {
            return Err(AppError::InvalidInput(
                "仅支持带明确端口的本机 HTTP(S)/SOCKS5 代理；不能含账号、路径或指向路由自身".into(),
            ));
        }
        Ok(value)
    }
    pub fn endpoint(&self) -> String {
        format!(
            "http://127.0.0.1:{}/rd/{}/v1",
            self.settings.listen_port, self.access_token
        )
    }
    fn expect_revision(&self, expected: u64) -> AppResult<()> {
        if self.revision != expected {
            return Err(AppError::Conflict("路由设置已变化，请刷新后重试".into()));
        }
        Ok(())
    }
    fn bump(&mut self) -> AppResult<()> {
        self.revision = self
            .revision
            .checked_add(1)
            .ok_or_else(|| AppError::Conflict("路由设置版本已达上限".into()))?;
        Ok(())
    }
}

#[derive(Default)]
pub struct RouteStats {
    pub accepting: AtomicBool,
    pub running: AtomicBool,
    pub requests: AtomicU64,
    pub active: AtomicU64,
    pub completed: AtomicU64,
    pub failed: AtomicU64,
    pub last_status: AtomicU64,
    last_error: Mutex<Option<String>>,
    last_diagnostic: Mutex<Option<crate::route_health::Diagnostic>>,
    diagnostic_logged: Mutex<std::collections::BTreeMap<&'static str, std::time::Instant>>,
}
impl RouteStats {
    pub fn diagnostic(&self, value: crate::route_health::Diagnostic) {
        self.error(value.message);
        // Bounded by the fixed category enum, not arbitrary URLs or node names.
        // Client retry bursts update the UI but cannot flood the disk journal.
        let should_log = self.diagnostic_logged.lock().is_ok_and(|mut logged| {
            if logged
                .get(value.code)
                .is_some_and(|t| t.elapsed().as_secs() < 30)
            {
                return false;
            }
            logged.insert(value.code, std::time::Instant::now());
            true
        });
        if should_log {
            crate::app_log::record(1, crate::app_log::Area::Routing, &value.log_message());
        }
        if let Ok(mut last) = self.last_diagnostic.lock() {
            *last = Some(value);
        }
    }
    pub fn error(&self, value: &str) {
        if let Ok(mut e) = self.last_error.lock() {
            *e = Some(value.into());
        }
    }
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RouteSnapshot {
    revision: u64,
    enabled: bool,
    running: bool,
    settings: RouteSettings,
    endpoint: String,
    requests: u64,
    active: u64,
    completed: u64,
    failed: u64,
    last_status: u64,
    last_error: Option<String>,
    last_diagnostic: Option<crate::route_health::Diagnostic>,
    codex: codex::CodexStatus,
    stability: crate::openai_stability::StabilitySnapshot,
}
#[derive(Default)]
pub struct LocalRoutingManager {
    server: Mutex<Option<server::RunningServer>>,
    error: Mutex<Option<String>>,
}
impl LocalRoutingManager {
    fn lock(&self) -> AppResult<std::sync::MutexGuard<'_, Option<server::RunningServer>>> {
        self.server
            .lock()
            .map_err(|_| AppError::Runtime("路由状态锁不可用".into()))
    }
    fn snapshot(&self, app: &AppHandle) -> AppResult<RouteSnapshot> {
        let storage = AppStorage::from_app(app)?;
        let dir = storage.routing_dir();
        let doc = RouteDocument::read(&dir)?;
        let server = self.lock()?;
        let stats = server.as_ref().map(|s| s.stats.clone()).unwrap_or_default();
        Ok(RouteSnapshot {
            revision: doc.revision,
            enabled: doc.enabled,
            running: stats.running.load(Ordering::Acquire),
            endpoint: format!("http://127.0.0.1:{}", doc.settings.listen_port),
            settings: doc.settings,
            requests: stats.requests.load(Ordering::Relaxed),
            active: stats.active.load(Ordering::Relaxed),
            completed: stats.completed.load(Ordering::Relaxed),
            failed: stats.failed.load(Ordering::Relaxed),
            last_status: stats.last_status.load(Ordering::Relaxed),
            last_diagnostic: stats.last_diagnostic.lock().ok().and_then(|v| v.clone()),
            last_error: self
                .error
                .lock()
                .ok()
                .and_then(|v| v.clone())
                .or_else(|| stats.last_error.lock().ok().and_then(|v| v.clone())),
            codex: codex::status(&codex::config_path(app)?, &dir)?,
            stability: app
                .state::<crate::openai_stability::StabilityManager>()
                .snapshot(),
        })
    }
    pub fn bootstrap(&self, app: &AppHandle) -> AppResult<()> {
        let storage = AppStorage::from_app(app)?;
        let doc = RouteDocument::read(&storage.routing_dir())?;
        if doc.enabled {
            let settings = storage.settings()?;
            let running = if doc.settings.listen_port == settings.controller_port {
                Err(AppError::InvalidInput("路由端口不能占用控制端口".into()))
            } else {
                server::start(&doc, settings.mixed_port, Some(app.clone()))
            };
            match running {
                Ok(server) => *self.lock()? = Some(server),
                Err(e) => {
                    // A failed restart must not silently leave an owned dead Codex endpoint.
                    let restore =
                        codex::restore(&codex::config_path(app)?, &storage.routing_dir(), None);
                    *self
                        .error
                        .lock()
                        .map_err(|_| AppError::Runtime("路由状态锁不可用".into()))? =
                        Some(format!(
                            "路由未启动：{e}。{}",
                            if restore.is_ok() {
                                "已恢复原接入配置；如原配置为 CC Switch，请保持其服务开启"
                            } else {
                                "配置恢复有冲突，请在路由页面检查备份"
                            }
                        ));
                }
            }
        } else {
            // Recover an interrupted stop, but only when the lease still owns the fields.
            codex::restore(&codex::config_path(app)?, &storage.routing_dir(), None)?;
        }
        Ok(())
    }
    pub fn shutdown(&self, app: &AppHandle) -> AppResult<()> {
        let storage = AppStorage::from_app(app)?;
        codex::restore(&codex::config_path(app)?, &storage.routing_dir(), None)?;
        if let Some(server) = self.lock()?.take() {
            server.stop();
        }
        Ok(())
    }
}

// General system-proxy use targets ChatGPT. An explicitly enabled API route
// targets the API instead; this reads preferences without enabling any service.
pub(crate) fn stability_target(app: &AppHandle) -> AppResult<crate::route_health::Target> {
    let storage = AppStorage::from_app(app)?;
    let doc = RouteDocument::read(&storage.routing_dir())?;
    Ok(if doc.enabled {
        doc.settings.upstream.target()
    } else {
        crate::route_health::Target::Chatgpt
    })
}

#[tauri::command]
pub fn local_route_status(
    app: AppHandle,
    state: State<'_, LocalRoutingManager>,
) -> Result<RouteSnapshot, AppErrorDto> {
    state.snapshot(&app).map_err(|e| e.dto())
}
#[tauri::command]
pub fn save_local_route(
    app: AppHandle,
    state: State<'_, LocalRoutingManager>,
    settings: RouteSettings,
    expected_revision: u64,
) -> Result<RouteSnapshot, AppErrorDto> {
    (|| -> AppResult<_> {
        let guard = state.lock()?;
        let storage = AppStorage::from_app(&app)?;
        let dir = storage.routing_dir();
        let mut doc = RouteDocument::read(&dir)?;
        doc.expect_revision(expected_revision)?;
        if doc.enabled
            || guard
                .as_ref()
                .is_some_and(|s| s.stats.running.load(Ordering::Acquire))
            || codex::has_lease(&dir)?
        {
            return Err(AppError::Conflict(
                "请先恢复 Codex 接入并关闭路由，再修改端口或模式".into(),
            ));
        }
        doc.settings = settings;
        doc.proxy_url(storage.settings()?.mixed_port)?;
        if doc.settings.listen_port == storage.settings()?.controller_port {
            return Err(AppError::InvalidInput("路由端口不能占用控制端口".into()));
        }
        doc.bump()?;
        doc.write(&dir)?;
        drop(guard);
        state.snapshot(&app)
    })()
    .map_err(|e| e.dto())
}
#[tauri::command]
pub fn set_local_route_enabled(
    app: AppHandle,
    state: State<'_, LocalRoutingManager>,
    enabled: bool,
    expected_revision: u64,
    confirmed: bool,
) -> Result<RouteSnapshot, AppErrorDto> {
    (|| -> AppResult<_> {
        if !confirmed {
            return Err(AppError::InvalidInput("启停路由需要确认".into()));
        }
        let mut guard = state.lock()?;
        let storage = AppStorage::from_app(&app)?;
        let dir = storage.routing_dir();
        let mut doc = RouteDocument::read(&dir)?;
        doc.expect_revision(expected_revision)?;
        if enabled {
            if guard
                .as_ref()
                .is_some_and(|s| s.stats.running.load(Ordering::Acquire))
            {
                drop(guard);
                return state.snapshot(&app);
            }
            if doc.settings.listen_port == storage.settings()?.controller_port {
                return Err(AppError::InvalidInput("路由端口不能占用控制端口".into()));
            }
            let server = server::start(&doc, storage.settings()?.mixed_port, Some(app.clone()))?;
            doc.enabled = true;
            doc.bump()?;
            doc.write(&dir)?;
            *guard = Some(server);
        } else {
            if let Some(server) = guard.as_ref() {
                server.stats.accepting.store(false, Ordering::SeqCst);
            }
            if guard
                .as_ref()
                .is_some_and(|s| s.stats.active.load(Ordering::SeqCst) > 0)
            {
                if let Some(server) = guard.as_ref() {
                    server.stats.accepting.store(true, Ordering::SeqCst);
                }
                return Err(AppError::Conflict(
                    "仍有进行中的路由请求，请等待结束后关闭；未中断请求".into(),
                ));
            }
            let save = (|| -> AppResult<()> {
                codex::restore(&codex::config_path(&app)?, &dir, None)?;
                doc.enabled = false;
                doc.bump()?;
                doc.write(&dir)
            })();
            if let Err(e) = save {
                if let Some(server) = guard.as_ref() {
                    server.stats.accepting.store(true, Ordering::SeqCst);
                }
                return Err(e);
            }
            if let Some(server) = guard.take() {
                server.stop();
            }
        }
        if let Ok(mut error) = state.error.lock() {
            *error = None;
        }
        drop(guard);
        state.snapshot(&app)
    })()
    .map_err(|e| e.dto())
}
#[tauri::command]
pub fn set_codex_route(
    app: AppHandle,
    state: State<'_, LocalRoutingManager>,
    attach: bool,
    expected_config_revision: String,
    confirmed: bool,
) -> Result<RouteSnapshot, AppErrorDto> {
    (|| -> AppResult<_> {
        if !confirmed {
            return Err(AppError::InvalidInput("修改 Codex 接入需要确认".into()));
        }
        let guard = state.lock()?;
        let storage = AppStorage::from_app(&app)?;
        let dir = storage.routing_dir();
        let config = codex::config_path(&app)?;
        if guard
            .as_ref()
            .is_some_and(|s| s.stats.active.load(Ordering::Acquire) > 0)
        {
            return Err(AppError::Conflict(
                "当前仍有模型请求，请结束后再更改接入".into(),
            ));
        }
        if attach {
            if !guard
                .as_ref()
                .is_some_and(|s| s.stats.running.load(Ordering::Acquire))
            {
                return Err(AppError::Conflict("请先启动 Serylane 路由服务".into()));
            }
            codex::attach(
                &config,
                &dir,
                &RouteDocument::read(&dir)?,
                &expected_config_revision,
            )?;
        } else {
            codex::restore(&config, &dir, Some(&expected_config_revision))?;
        }
        drop(guard);
        state.snapshot(&app)
    })()
    .map_err(|e| e.dto())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn defaults_are_off_and_proxy_validation_fails_closed() {
        let mut d = RouteDocument::default();
        assert!(!d.enabled);
        let root = tempfile::tempdir().unwrap();
        // macOS TMPDIR can have a /var symlink ancestor. Normalize only the
        // test fixture; production configuration paths still reject links.
        let root_path = root.path().canonicalize().unwrap();
        let absent = root_path.join("never-created");
        assert!(!RouteDocument::read(&absent).unwrap().enabled);
        assert!(!absent.exists());
        for proxy in [
            "http://example.com:7890",
            "http://127.0.0.1:15731",
            "http://a:b@localhost:7890",
            "http://localhost:7890/path",
            "http://localhost:7890?token=x",
            "file:///x",
        ] {
            d.settings.outbound_proxy = proxy.into();
            assert!(d.proxy_url(7890).is_err(), "{proxy}");
        }
        for proxy in ["", "http://127.0.0.1:7890", "socks5h://localhost:7890"] {
            d.settings.outbound_proxy = proxy.into();
            assert!(d.proxy_url(7890).is_ok());
        }
    }
    #[test]
    fn settings_survive_round_trip_and_corruption_is_not_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let dir_path = dir.path().canonicalize().unwrap();
        let doc = RouteDocument::default();
        doc.write(&dir_path).unwrap();
        assert_eq!(
            RouteDocument::read(&dir_path).unwrap().access_token,
            doc.access_token
        );
        std::fs::write(dir_path.join("settings.json"), "{}").unwrap();
        assert!(RouteDocument::read(&dir_path).is_err());
        assert_eq!(
            std::fs::read_to_string(dir_path.join("settings.json")).unwrap(),
            "{}"
        );
    }
}
