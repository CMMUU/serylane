//! Opt-in proxy launching, not process interception. No system environment,
//! registry, DNS, existing process, or proxy mode is modified here.
use crate::app_binding::{
    self, AppAvailability, AppBinding, ApplicationCatalog, ApplicationResolution, CatalogSnapshot,
};
use crate::app_binding::{
    desktop,
    picker_cache::{PickerCache, PickerSnapshot},
};
use crate::error::{AppError, AppErrorDto, AppResult};
use crate::models::RuntimePhase;
use crate::runtime::MihomoRuntime;
use crate::storage::AppStorage;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use tauri::{AppHandle, Manager};
use uuid::Uuid;

const MAX_PROGRAMS: usize = 100;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProgramProxyMode {
    Environment,
    Chromium,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProxyProgram {
    pub id: Uuid,
    pub name: String,
    pub executable: String,
    #[serde(default)]
    pub binding: Option<AppBinding>,
    #[serde(default)]
    pub working_directory_relative: Option<String>,
    pub arguments: Vec<String>,
    pub working_directory: Option<String>,
    pub mode: ProgramProxyMode,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProgramDocument {
    pub schema_version: u32,
    pub revision: u64,
    pub programs: Vec<ProxyProgram>,
}

impl Default for ProgramDocument {
    fn default() -> Self {
        Self {
            schema_version: 3,
            revision: 0,
            programs: Vec::new(),
        }
    }
}

impl ProgramDocument {
    pub fn validate(&self) -> AppResult<()> {
        if !matches!(self.schema_version, 1..=3) || self.programs.len() > MAX_PROGRAMS {
            return Err(AppError::InvalidInput(
                "程序代理清单版本或条目数量无效".into(),
            ));
        }
        let mut ids = HashSet::new();
        for program in &self.programs {
            validate_fields(program)?;
            if !ids.insert(program.id) {
                return Err(AppError::InvalidInput("程序清单包含重复 ID".into()));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProgramInput {
    pub id: Option<Uuid>,
    pub name: String,
    pub executable: String,
    #[serde(default)]
    pub binding: Option<AppBinding>,
    #[serde(default)]
    pub working_directory_relative: Option<String>,
    pub arguments: Vec<String>,
    pub working_directory: Option<String>,
    pub mode: ProgramProxyMode,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgramEntry {
    #[serde(flatten)]
    program: ProxyProgram,
    available: bool,
    resolution: ApplicationResolution,
    running_pid: Option<u32>,
    launch_pending: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgramState {
    revision: u64,
    supported: bool,
    platform: &'static str,
    proxy_endpoint: String,
    core_running: bool,
    programs: Vec<ProgramEntry>,
}

#[derive(Default)]
pub struct ProgramProxyManager {
    // Serializes document mutations and starts, including double-clicks.
    children: Mutex<BTreeMap<Uuid, RunningProgram>>,
    catalog: ApplicationCatalog,
    picker: Mutex<PickerCache>,
    display_cache: Mutex<BTreeMap<String, (std::time::Instant, ApplicationResolution)>>,
}

/// Native GUI launchers are not necessarily children of Serylane. Keep their
/// verified instance identity separate from a direct process handle.
enum RunningProgram {
    Child(Child),
    #[cfg(target_os = "macos")]
    Mac(app_binding::macos::RunningApplication),
    #[cfg(target_os = "macos")]
    MacPending(app_binding::macos::LaunchTicket),
    Pending,
}
impl RunningProgram {
    fn alive(&mut self) -> bool {
        #[cfg(target_os = "macos")]
        if let Self::MacPending(ticket) = self {
            match app_binding::macos::launch_status(ticket) {
                app_binding::macos::LaunchStatus::Complete(Ok(app)) => *self = Self::Mac(app),
                app_binding::macos::LaunchStatus::Complete(Err(_)) => return false,
                _ => return true,
            }
        }
        match self {
            Self::Child(child) => matches!(child.try_wait(), Ok(None)),
            #[cfg(target_os = "macos")]
            Self::Mac(app) => app.is_running(),
            Self::Pending => true,
            #[cfg(target_os = "macos")]
            Self::MacPending(_) => true,
        }
    }
    fn pid(&self) -> u32 {
        match self {
            Self::Child(child) => child.id(),
            #[cfg(target_os = "macos")]
            Self::Mac(app) => app.pid(),
            Self::Pending => 0,
            #[cfg(target_os = "macos")]
            Self::MacPending(_) => 0,
        }
    }
}
fn supported_platform() -> bool {
    cfg!(any(windows, target_os = "macos", target_os = "linux"))
}
fn valid_stored_path(path: &str) -> bool {
    path.len() <= 32760
        && !path.contains(['\0', '\r', '\n'])
        && (local_windows_path(path) || (path.starts_with('/') && !path.starts_with("//")))
}
fn local_path(path: &str) -> bool {
    if cfg!(windows) {
        local_windows_path(path)
    } else {
        path.starts_with('/') && !path.starts_with("//")
    }
}
fn is_executable(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata()
            .is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
            && path.extension().is_none_or(|s| s != "desktop")
    }
    #[cfg(not(unix))]
    {
        path.extension()
            .is_some_and(|s| s.eq_ignore_ascii_case("exe"))
    }
}
fn resolve_binding(
    binding: &AppBinding,
    catalog: &ApplicationCatalog,
    fresh: bool,
) -> ApplicationResolution {
    if binding.platform() != std::env::consts::OS {
        return ApplicationResolution::state(
            AppAvailability::UnsupportedLaunch,
            "此条目属于其他平台，原设置已保留。请重新选择本机应用。",
        );
    }
    match binding {
        AppBinding::Windows(w) => match catalog.query(Some(&w.package_family_name), fresh) {
            Ok(snapshot) => app_binding::resolve(binding, &snapshot),
            Err(e) => ApplicationResolution::state(AppAvailability::ReadError, e),
        },
        AppBinding::Desktop(d) => match desktop::resolve(d) {
            Ok(app) => ApplicationResolution {
                availability: app.availability,
                detail: app.detail.clone(),
                application: Some(app.into()),
            },
            Err(e) => desktop_read_error(d, e),
        },
    }
}

fn desktop_read_error(binding: &desktop::DesktopBinding, detail: String) -> ApplicationResolution {
    // macOS discovery has no OS-maintained installation state enum. Classify
    // only our adapter's explicit identity/missing diagnoses; IO errors stay
    // retryable reads rather than falsely claiming the app was uninstalled.
    let availability = if matches!(binding, desktop::DesktopBinding::Macos { .. }) {
        if detail.starts_with("未找到原关联的应用") {
            AppAvailability::NotInstalled
        } else if detail.starts_with("应用身份或签名方式已变化")
            || detail.starts_with("应用签名已变化")
            || detail.starts_with("原安装位置未找到应用")
            || detail.starts_with("发现多个相同身份的应用")
        {
            AppAvailability::NeedsRelink
        } else {
            AppAvailability::ReadError
        }
    } else {
        AppAvailability::ReadError
    };
    ApplicationResolution::state(availability, detail)
}

// Display observations are short-lived and never authorize a launch. Native
// reads happen outside this mutex; actual launch always uses resolve directly.
fn resolve_display_binding(
    binding: &AppBinding,
    manager: &ProgramProxyManager,
    fresh: bool,
) -> ApplicationResolution {
    let key = binding.aumid();
    if !fresh {
        if let Ok(cache) = manager.display_cache.lock() {
            if let Some((at, value)) = cache.get(&key) {
                if at.elapsed() < std::time::Duration::from_secs(30) {
                    return value.clone();
                }
            }
        }
    }
    #[cfg(target_os = "macos")]
    let native = match binding {
        AppBinding::Desktop(d @ desktop::DesktopBinding::Macos { .. }) => {
            Some(app_binding::macos::resolve_display(d))
        }
        _ => None,
    };
    #[cfg(not(target_os = "macos"))]
    let native: Option<Result<desktop::DesktopApplication, String>> = None;
    let value = match native {
        Some(Ok(app)) => ApplicationResolution {
            availability: app.availability,
            detail: app.detail.clone(),
            application: Some(app.into()),
        },
        Some(Err(e)) => match binding {
            AppBinding::Desktop(d) => desktop_read_error(d, e),
            _ => ApplicationResolution::state(AppAvailability::ReadError, e),
        },
        None => resolve_binding(binding, &manager.catalog, fresh),
    };
    if let Ok(mut cache) = manager.display_cache.lock() {
        if cache.len() > MAX_PROGRAMS {
            cache.clear();
        }
        cache.insert(key, (std::time::Instant::now(), value.clone()));
    }
    value
}

fn invalid(message: &str) -> AppError {
    AppError::InvalidInput(message.into())
}

fn local_windows_path(value: &str) -> bool {
    let path = value.strip_prefix("\\\\?\\").unwrap_or(value).as_bytes();
    path.len() > 3
        && path[0].is_ascii_alphabetic()
        && path[1] == b':'
        && matches!(path[2], b'\\' | b'/')
}

fn validate_fields(program: &ProxyProgram) -> AppResult<()> {
    if program.name.trim().is_empty()
        || program.name.chars().count() > 128
        || program.name.contains(['\0', '\n', '\r'])
    {
        return Err(invalid("程序名称不能为空，且不能超过 128 字"));
    }
    if let Some(binding) = &program.binding {
        if !binding.valid() {
            return Err(invalid("应用身份无效，请重新选择已安装应用"));
        }
    } else if program.executable.is_empty()
        || program.executable.len() > 32760
        || program.executable.contains(['\0', '\n', '\r'])
        || !valid_stored_path(&program.executable)
        || (local_windows_path(&program.executable)
            && !program.executable.to_ascii_lowercase().ends_with(".exe"))
    {
        return Err(invalid("程序路径无效"));
    }
    if let Some(relative) = &program.working_directory_relative {
        if !matches!(program.binding, Some(AppBinding::Windows(_)))
            || program.working_directory.is_some()
            || !(relative == "." || app_binding::relative_windows_path(relative))
        {
            return Err(invalid("包内工作目录应为相对目录，且仅用于已关联应用"));
        }
    }
    if program.arguments.len() > 64
        || program.arguments.iter().map(String::len).sum::<usize>() > 8192
        || program
            .arguments
            .iter()
            .any(|arg| arg.contains(['\0', '\r', '\n']))
    {
        return Err(invalid(
            "启动参数最多 64 项、总长 8192 字节，不能包含换行或空字符",
        ));
    }
    if program
        .working_directory
        .as_ref()
        .is_some_and(|p| !valid_stored_path(p))
    {
        return Err(invalid("工作目录无效"));
    }
    if program.mode == ProgramProxyMode::Chromium
        && program.arguments.iter().any(|arg| {
            let arg = arg.trim().to_ascii_lowercase();
            [
                "--proxy-",
                "--no-proxy-server",
                "--system-proxy",
                "--disable-quic",
            ]
            .iter()
            .any(|prefix| arg.starts_with(prefix))
        })
    {
        return Err(invalid(
            "Chromium 模式会管理代理参数，请移除自定义参数中的代理或 QUIC 开关",
        ));
    }
    Ok(())
}

fn normalize_input(input: ProgramInput) -> AppResult<ProxyProgram> {
    let mut program = ProxyProgram {
        id: input.id.unwrap_or_else(Uuid::new_v4),
        name: input.name.trim().into(),
        executable: input.executable.trim().into(),
        binding: input.binding,
        working_directory_relative: input.working_directory_relative,
        arguments: input.arguments,
        working_directory: input
            .working_directory
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
        mode: input.mode,
    };
    validate_fields(&program)?;
    if let Some(binding) = &program.binding {
        if binding.platform() != std::env::consts::OS {
            return Err(invalid("请选择当前平台的应用。"));
        }
        if matches!(
            binding,
            AppBinding::Desktop(desktop::DesktopBinding::Macos { .. })
        ) && program.working_directory.is_some()
        {
            return Err(invalid(
                "macOS 应用使用系统原生工作目录；请清空自定义工作目录后保存，原配置尚未改变。",
            ));
        }
        // A saved package binding never pins a versioned executable path.
        program.executable.clear();
        return Ok(program);
    }
    // Reject UNC/device/relative paths before any metadata lookup so the picker
    // and manual input cannot accidentally initiate a network-share connection.
    if !local_path(&program.executable) {
        return Err(invalid(
            "请选择本机磁盘上的程序，不支持相对路径、网络共享或设备路径",
        ));
    }
    let path = Path::new(&program.executable);
    if !path.is_absolute() || !is_executable(path) {
        return Err(invalid(
            "请选择本机可执行文件；.app 或 .desktop 请通过应用选择入口关联。",
        ));
    }
    let canonical = path.canonicalize()?;
    let display = canonical.to_string_lossy();
    if display.starts_with("\\\\?\\UNC\\")
        || display.starts_with("\\\\") && !display.starts_with("\\\\?\\")
    {
        return Err(invalid("请使用本机程序，不支持网络共享中的可执行文件"));
    }
    program.executable = display.strip_prefix("\\\\?\\").unwrap_or(&display).into();
    if let Some(directory) = &program.working_directory {
        if !local_path(directory) {
            return Err(invalid(
                "工作目录必须位于本机磁盘，不支持网络共享或相对路径",
            ));
        }
        let directory = Path::new(directory);
        if !directory.is_absolute() || !directory.is_dir() {
            return Err(invalid("工作目录必须是存在的绝对路径"));
        }
    }
    Ok(program)
}

fn check_revision(document: &ProgramDocument, expected: u64) -> AppResult<()> {
    if document.revision != expected {
        return Err(AppError::Conflict(
            "程序清单已更新，请刷新列表后重试；编辑内容仍保留".into(),
        ));
    }
    Ok(())
}

fn update_document(
    document: &mut ProgramDocument,
    program: ProxyProgram,
    editing: bool,
) -> AppResult<()> {
    if editing {
        let current = document
            .programs
            .iter_mut()
            .find(|p| p.id == program.id)
            .ok_or_else(|| AppError::NotFound("程序条目已被删除".into()))?;
        *current = program;
    } else {
        document.programs.push(program);
    }
    document.revision = document
        .revision
        .checked_add(1)
        .ok_or_else(|| invalid("程序清单版本溢出"))?;
    document.validate()
}

fn snapshot(app: &AppHandle, storage: &AppStorage, fresh: bool) -> AppResult<ProgramState> {
    let manager = app.state::<ProgramProxyManager>();
    let document = storage.programs()?;
    let running = {
        let mut children = manager
            .children
            .lock()
            .map_err(|_| invalid("程序管理器繁忙"))?;
        children.retain(|_, child| child.alive());
        children
            .iter()
            .map(|(id, child)| (*id, child.pid()))
            .collect::<BTreeMap<_, _>>()
    };
    let programs = document
        .programs
        .into_iter()
        .map(|mut program| {
            let resolution = if let Some(binding) = &program.binding {
                resolve_display_binding(binding, &manager, fresh)
            } else if local_path(&program.executable)
                && is_executable(Path::new(&program.executable))
            {
                ApplicationResolution::state(
                    AppAvailability::Ready,
                    "普通文件关联；移动文件后需重新选择。",
                )
            } else if program
                .executable
                .replace('/', "\\")
                .to_lowercase()
                .contains("\\windowsapps\\")
            {
                ApplicationResolution::state(
                    AppAvailability::NeedsRelink,
                    "原安装路径已失效，请重新关联已安装应用；原代理设置和参数已保留。",
                )
            } else {
                ApplicationResolution::state(
                    AppAvailability::MissingFile,
                    "程序文件不存在或暂不可执行，请检查位置和文件权限。",
                )
            };
            if let Some(application) = &resolution.application {
                program.executable.clone_from(&application.executable);
            }
            ProgramEntry {
                available: resolution.availability == AppAvailability::Ready,
                running_pid: running.get(&program.id).copied().filter(|pid| *pid != 0),
                launch_pending: running.get(&program.id) == Some(&0),
                program,
                resolution,
            }
        })
        .collect();
    Ok(ProgramState {
        revision: document.revision,
        supported: supported_platform(),
        platform: std::env::consts::OS,
        proxy_endpoint: format!("http://127.0.0.1:{}", storage.settings()?.mixed_port),
        core_running: app.state::<MihomoRuntime>().status(Some(app)).phase == RuntimePhase::Running,
        programs,
    })
}

fn migrate_legacy_program(program: &mut ProxyProgram, catalog: &CatalogSnapshot) -> bool {
    if program.binding.is_some() {
        return false;
    }
    let Some(app) = app_binding::exact_legacy_match(&program.executable, catalog) else {
        return false;
    };
    if let Some(directory) = &program.working_directory {
        let relative =
            if app_binding::path_key(directory) == app_binding::path_key(&app.package_root) {
                Some(".".into())
            } else {
                app_binding::install_relative(&app.package_root, directory)
            };
        if let Some(relative) = relative {
            program.working_directory_relative = Some(relative);
            program.working_directory = None;
        }
    }
    program.binding = Some(app.binding.clone());
    program.executable.clear();
    true
}

#[tauri::command]
pub async fn list_proxy_programs(
    app: AppHandle,
    refresh: Option<bool>,
) -> Result<ProgramState, AppErrorDto> {
    tauri::async_runtime::spawn_blocking(move || -> AppResult<ProgramState> {
        let storage = AppStorage::from_app(&app)?;
        let manager = app.state::<ProgramProxyManager>();
        let mut document = storage.programs()?;
        let original_revision = document.revision;
        let mut changed = document.schema_version < 3;
        if cfg!(windows) && document.programs.iter().any(|p| p.binding.is_none()) {
            if let Ok(catalog) = manager.catalog.query(None, refresh.unwrap_or(false)) {
                for program in &mut document.programs {
                    changed |= migrate_legacy_program(program, &catalog);
                }
            }
        }
        if changed {
            let _lock = manager
                .children
                .lock()
                .map_err(|_| invalid("程序管理器繁忙"))?;
            let current = storage.programs()?;
            // Never overwrite a concurrently edited document with scan results.
            if current.revision == original_revision {
                document.schema_version = 3;
                document.revision = document
                    .revision
                    .checked_add(1)
                    .ok_or_else(|| invalid("程序清单版本溢出"))?;
                storage.save_programs(&document)?;
            }
        }
        snapshot(&app, &storage, refresh.unwrap_or(false))
    })
    .await
    .map_err(|e| AppError::Runtime(e.to_string()).dto())?
    .map_err(|e| e.dto())
}

#[tauri::command]
pub async fn list_installed_proxy_applications(
    app: AppHandle,
    refresh: Option<bool>,
) -> Result<PickerSnapshot, AppErrorDto> {
    let manager = app.state::<ProgramProxyManager>();
    let mut cache = manager
        .picker
        .lock()
        .map_err(|_| invalid("应用列表正在刷新").dto())?;
    let start = cache.begin(refresh.unwrap_or(false));
    let snapshot = cache.snapshot.clone();
    drop(cache);
    if start {
        let handle = app.clone();
        tauri::async_runtime::spawn_blocking(move || {
            let manager = handle.state::<ProgramProxyManager>();
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                manager.catalog.query(None, true)
            }))
            .unwrap_or_else(|_| Err("应用读取中断，旧清单已保留，请重新扫描。".into()));
            if let Ok(mut cache) = manager.picker.lock() {
                cache.finish(result);
            };
        });
    }
    Ok(snapshot)
}

#[tauri::command]
pub async fn inspect_proxy_application(
    path: String,
) -> Result<app_binding::InstalledApplication, AppErrorDto> {
    tauri::async_runtime::spawn_blocking(move || {
        if !valid_stored_path(&path) {
            return Err(invalid("应用路径格式不正确").dto());
        }
        desktop::inspect(Path::new(&path))
            .map(Into::into)
            .map_err(|e| invalid(&e).dto())
    })
    .await
    .map_err(|e| AppError::Runtime(e.to_string()).dto())?
}

#[tauri::command]
pub async fn save_proxy_program(
    app: AppHandle,
    input: ProgramInput,
    expected_revision: u64,
) -> Result<ProgramState, AppErrorDto> {
    tauri::async_runtime::spawn_blocking(move || -> AppResult<ProgramState> {
        if !supported_platform() {
            return Err(invalid("此平台暂未适配程序代理启动"));
        }
        let storage = AppStorage::from_app(&app)?;
        let manager = app.state::<ProgramProxyManager>();
        let mut document = storage.programs()?;
        check_revision(&document, expected_revision)?;
        let editing = input.id.is_some();
        let program = normalize_input(input)?;
        if let Some(binding) = &program.binding {
            let unchanged = document
                .programs
                .iter()
                .any(|old| old.id == program.id && old.binding.as_ref() == Some(binding));
            if !unchanged {
                let resolved = resolve_binding(binding, &manager.catalog, true);
                if resolved.application.is_none() {
                    return Err(invalid(&resolved.detail));
                }
            }
        }
        {
            let _lock = manager
                .children
                .lock()
                .map_err(|_| invalid("程序管理器繁忙"))?;
            check_revision(&storage.programs()?, expected_revision)?;
            document.schema_version = 3;
            update_document(&mut document, program, editing)?;
            storage.save_programs(&document)?;
        }
        snapshot(&app, &storage, false)
    })
    .await
    .map_err(|e| AppError::Runtime(e.to_string()).dto())?
    .map_err(|e| e.dto())
}

#[tauri::command]
pub async fn delete_proxy_program(
    app: AppHandle,
    program_id: Uuid,
    expected_revision: u64,
) -> Result<ProgramState, AppErrorDto> {
    tauri::async_runtime::spawn_blocking(move || -> AppResult<ProgramState> {
        let storage = AppStorage::from_app(&app)?;
        let manager = app.state::<ProgramProxyManager>();
        let mut children = manager
            .children
            .lock()
            .map_err(|_| AppError::Conflict("程序管理器繁忙".into()))?;
        let mut document = storage.programs()?;
        check_revision(&document, expected_revision)?;
        let index = document
            .programs
            .iter()
            .position(|p| p.id == program_id)
            .ok_or_else(|| AppError::NotFound("程序条目不存在".into()))?;
        document.programs.remove(index);
        document.revision = document
            .revision
            .checked_add(1)
            .ok_or_else(|| invalid("程序清单版本溢出"))?;
        storage.save_programs(&document)?;
        // Drop only our handle: never kill the program or delete its executable.
        children.remove(&program_id);
        drop(children);
        snapshot(&app, &storage, false)
    })
    .await
    .map_err(|e| AppError::Runtime(e.to_string()).dto())?
    .map_err(|e| e.dto())
}

fn proxy_environment(port: u16) -> Vec<(String, String)> {
    let endpoint = format!("http://127.0.0.1:{port}");
    let mut vars: Vec<_> = [
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "WS_PROXY",
        "WSS_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
        "ws_proxy",
        "wss_proxy",
    ]
    .into_iter()
    .map(|key| (key.to_string(), endpoint.clone()))
    .collect();
    vars.extend(["NO_PROXY", "no_proxy"].map(|key| (key.into(), "localhost,127.0.0.1,::1".into())));
    vars
}
fn proxy_arguments(program: &ProxyProgram, port: u16) -> Vec<String> {
    let mut args = vec![];
    if program.mode == ProgramProxyMode::Chromium {
        args.extend([
            format!("--proxy-server=http://127.0.0.1:{port}"),
            "--disable-quic".into(),
        ]);
    }
    args.extend(program.arguments.clone());
    args
}
fn proxy_command(program: &ProxyProgram, port: u16) -> Command {
    let mut command = Command::new(&program.executable);
    if let Some(directory) = &program.working_directory {
        command.current_dir(directory);
    } else if let Some(directory) = Path::new(&program.executable).parent() {
        command.current_dir(directory);
    }
    // Child-only overrides. Never mutate the global environment or shell config.
    command
        .envs(proxy_environment(port))
        .args(proxy_arguments(program, port))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
    }
    command
}

fn running_program(executable: &str) -> Option<u32> {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing().with_exe(UpdateKind::Always),
    );
    let key = |path: &Path| {
        if cfg!(windows) {
            app_binding::path_key(&path.to_string_lossy())
        } else {
            path.to_string_lossy().into_owned()
        }
    };
    let expected = key(Path::new(executable));
    system
        .processes()
        .values()
        .find(|process| process.exe().is_some_and(|p| key(p) == expected))
        .map(|p| p.pid().as_u32())
}

fn resolve_launch_program(
    program: &ProxyProgram,
    catalog: &ApplicationCatalog,
) -> AppResult<ProxyProgram> {
    let mut resolved = program.clone();
    if let Some(binding) = &program.binding {
        if binding.platform() != std::env::consts::OS {
            return Err(invalid("此条目属于其他平台，请重新选择本机应用。"));
        }
        match binding {
            AppBinding::Windows(_) => {
                let resolution = resolve_binding(binding, catalog, true);
                if resolution.availability != AppAvailability::Ready {
                    return Err(invalid(&resolution.detail));
                }
                let app = resolution
                    .application
                    .ok_or_else(|| invalid("应用信息尚未就绪，请刷新"))?;
                resolved.executable = app.executable;
                if let Some(relative) = &program.working_directory_relative {
                    resolved.working_directory = Some(
                        Path::new(&app.package_root)
                            .join(relative)
                            .to_string_lossy()
                            .into_owned(),
                    );
                }
            }
            AppBinding::Desktop(binding) => {
                let app = desktop::resolve(binding).map_err(|e| invalid(&e))?;
                if app.availability != AppAvailability::Ready {
                    return Err(invalid(&app.detail));
                }
                if matches!(binding, desktop::DesktopBinding::Macos { .. })
                    && program.working_directory.is_some()
                {
                    return Err(invalid(
                        "此 macOS 应用采用系统原生工作目录，请先编辑并清空自定义目录。",
                    ));
                }
                resolved.executable = app.executable;
                resolved.binding = Some(AppBinding::Desktop(app.binding));
                resolved.arguments = app
                    .arguments
                    .into_iter()
                    .chain(program.arguments.clone())
                    .collect();
                if resolved.working_directory.is_none() {
                    resolved.working_directory = app.working_directory;
                }
                validate_fields(&resolved)?;
            }
        }
    }
    // Apply the same file, argument and working-directory checks to the resolved
    // path as to a portable executable; keep the binding for instance detection.
    let mut normalized = normalize_input(ProgramInput {
        id: Some(resolved.id),
        name: resolved.name,
        executable: resolved.executable,
        binding: None,
        working_directory_relative: None,
        arguments: resolved.arguments,
        working_directory: resolved.working_directory,
        mode: resolved.mode,
    })?;
    normalized.binding.clone_from(&resolved.binding);
    Ok(normalized)
}

fn launch_resolved(program: &ProxyProgram, port: u16) -> Result<RunningProgram, String> {
    #[cfg(target_os = "macos")]
    if let Some(AppBinding::Desktop(binding @ desktop::DesktopBinding::Macos { .. })) =
        &program.binding
    {
        return match app_binding::macos::launch_tracked(
            binding,
            &proxy_arguments(program, port),
            &proxy_environment(port),
        ) {
            app_binding::macos::NativeLaunchOutcome::Complete(result) => {
                result.map(RunningProgram::Mac)
            }
            app_binding::macos::NativeLaunchOutcome::Pending(ticket) => {
                Ok(RunningProgram::MacPending(ticket))
            }
        };
    }
    proxy_command(program, port)
        .spawn()
        .map(RunningProgram::Child)
        .map_err(|error| format!("应用尚未启动，请刷新安装信息后重试。详情：{error}"))
}

#[tauri::command]
pub async fn launch_proxy_program(
    app: AppHandle,
    program_id: Uuid,
    expected_revision: u64,
) -> Result<ProgramState, AppErrorDto> {
    tauri::async_runtime::spawn_blocking(move || -> AppResult<ProgramState> {
        if !supported_platform() {
            return Err(invalid("此平台暂未适配程序代理启动"));
        }
        let storage = AppStorage::from_app(&app)?;
        let manager = app.state::<ProgramProxyManager>();
        let document = storage.programs()?;
        check_revision(&document, expected_revision)?;
        let saved = document
            .programs
            .iter()
            .find(|p| p.id == program_id)
            .ok_or_else(|| invalid("程序条目不存在"))?;
        let program = resolve_launch_program(saved, &manager.catalog)?;
        // Native discovery and existing-instance checks occur outside locks.
        #[allow(unused_mut)] // Additional identity checks on Windows and macOS.
        let mut existing = running_program(&program.executable);
        #[cfg(windows)]
        {
            existing = program
                .binding
                .as_ref()
                .and_then(AppBinding::windows)
                .and_then(app_binding::running_bound)
                .or(existing);
        }
        #[cfg(target_os = "macos")]
        if let Some(AppBinding::Desktop(binding)) = &program.binding {
            existing = app_binding::macos::running(binding).or(existing);
        }
        if let Some(pid) = existing {
            return Err(AppError::Conflict(format!(
                "此应用仍在运行（PID {pid}）。请先自行退出，再从这里启动以应用代理设置。"
            )));
        }
        let permit = crate::user_rules::acquire_configuration(&app)?;
        if app.state::<MihomoRuntime>().status(Some(&app)).phase != RuntimePhase::Running {
            return Err(AppError::Conflict(
                "请先启动 Serylane 本地核心，再启动应用。".into(),
            ));
        }
        let port = storage.settings()?.mixed_port;
        let endpoint = std::net::SocketAddr::from(([127, 0, 0, 1], port));
        std::net::TcpStream::connect_timeout(&endpoint, std::time::Duration::from_secs(2))
            .map_err(|_| {
                AppError::Runtime("本地代理尚未就绪，请检查核心状态后重试；应用尚未启动。".into())
            })?;
        {
            let mut children = manager
                .children
                .lock()
                .map_err(|_| invalid("程序管理器繁忙"))?;
            check_revision(&storage.programs()?, expected_revision)?;
            if children
                .get_mut(&program_id)
                .is_some_and(RunningProgram::alive)
            {
                return Err(AppError::Conflict(
                    "此应用正在启动或已启动，请先核对运行状态，不必重复点击。".into(),
                ));
            }
            children.insert(program_id, RunningProgram::Pending);
        }
        // Do not hold global configuration/child locks while macOS waits for
        // native activation. A per-entry reservation prevents duplicate starts.
        let native_gui = matches!(
            &program.binding,
            Some(AppBinding::Desktop(desktop::DesktopBinding::Macos { .. }))
        );
        let mut permit = Some(permit);
        if native_gui {
            permit.take();
        }
        let outcome = launch_resolved(&program, port);
        drop(permit);
        let mut children = manager
            .children
            .lock()
            .map_err(|_| invalid("程序管理器繁忙"))?;
        match outcome {
            Ok(running) => {
                // A concurrent deletion must not resurrect the removed entry.
                if children.contains_key(&program_id) {
                    children.insert(program_id, running);
                }
            }
            Err(error) => {
                children.remove(&program_id);
                return Err(AppError::Runtime(error));
            }
        }
        drop(children);
        snapshot(&app, &storage, false)
    })
    .await
    .map_err(|e| AppError::Runtime(e.to_string()).dto())?
    .map_err(|e| e.dto())
}

#[tauri::command]
pub async fn choose_proxy_program(
    window: tauri::WebviewWindow,
) -> Result<Option<String>, AppErrorDto> {
    #[cfg(windows)]
    {
        let owner = window
            .hwnd()
            .map_err(|e| AppError::Platform(e.to_string()).dto())?
            .0 as isize;
        tauri::async_runtime::spawn_blocking(move || {
            use windows_sys::Win32::UI::Controls::Dialogs::*;
            let mut buffer = vec![0u16; 32768];
            let filter: Vec<u16> = "Windows 程序 (*.exe)\0*.exe\0\0".encode_utf16().collect();
            let title: Vec<u16> = "选择要通过代理启动的程序\0".encode_utf16().collect();
            let mut options: OPENFILENAMEW = unsafe { std::mem::zeroed() };
            options.lStructSize = std::mem::size_of::<OPENFILENAMEW>() as u32;
            options.hwndOwner = owner as _;
            options.lpstrFilter = filter.as_ptr();
            options.lpstrTitle = title.as_ptr();
            options.lpstrFile = buffer.as_mut_ptr();
            options.nMaxFile = buffer.len() as u32;
            options.Flags = OFN_EXPLORER
                | OFN_FILEMUSTEXIST
                | OFN_PATHMUSTEXIST
                | OFN_NOCHANGEDIR
                | OFN_DONTADDTORECENT;
            if unsafe { GetOpenFileNameW(&mut options) } != 0 {
                let len = buffer.iter().position(|c| *c == 0).unwrap_or(buffer.len());
                Ok(Some(String::from_utf16_lossy(&buffer[..len])))
            } else {
                let code = unsafe { CommDlgExtendedError() };
                if code == 0 {
                    Ok(None)
                } else {
                    Err(AppError::Platform(format!("文件选择失败（{code}）")).dto())
                }
            }
        })
        .await
        .map_err(|e| AppError::Platform(e.to_string()).dto())?
    }
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        window
            .run_on_main_thread(move || {
                #[cfg(target_os = "macos")]
                {
                    let _ = tx.send(app_binding::macos::choose());
                }
                #[cfg(target_os = "linux")]
                {
                    if let Err(error) = app_binding::linux::choose(tx.clone()) {
                        let _ = tx.send(Err(error));
                    }
                }
            })
            .map_err(|e| AppError::Platform(e.to_string()).dto())?;
        tauri::async_runtime::spawn_blocking(move || {
            rx.recv()
                .map_err(|_| invalid("文件选择窗口已关闭").dto())?
                .map_err(|e| invalid(&e).dto())
        })
        .await
        .map_err(|e| AppError::Runtime(e.to_string()).dto())?
    }
    #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
    {
        let _ = window;
        Err(invalid("此平台尚未适配文件选择").dto())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn legacy_schema_is_backed_up_once_without_resetting_arguments() {
        let root =
            std::env::temp_dir().join(format!("serylane-program-migration-{}", Uuid::new_v4()));
        let storage = AppStorage::from_root(root.clone()).unwrap();
        let mut old = serde_json::to_value(ProgramDocument::default()).unwrap();
        old["schemaVersion"] = 1.into();
        let mut value = serde_json::to_value(entry()).unwrap();
        value.as_object_mut().unwrap().remove("binding");
        value
            .as_object_mut()
            .unwrap()
            .remove("workingDirectoryRelative");
        old["programs"] = serde_json::json!([value]);
        let bytes = serde_json::to_vec(&old).unwrap();
        std::fs::write(root.join("proxy-programs.json"), &bytes).unwrap();
        let mut document = storage.programs().unwrap();
        assert_eq!(document.programs[0].binding, None);
        storage.save_programs(&document).unwrap();
        assert_eq!(storage.programs().unwrap().schema_version, 3);
        assert_eq!(
            std::fs::read(root.join("proxy-programs.v1.backup.json")).unwrap(),
            bytes
        );
        document.programs[0].name = "changed".into();
        storage.save_programs(&document).unwrap();
        assert_eq!(
            std::fs::read(root.join("proxy-programs.v1.backup.json")).unwrap(),
            bytes
        );
        assert_eq!(
            storage.programs().unwrap().programs[0].arguments,
            entry().arguments
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn desktop_read_failures_do_not_masquerade_as_identity_changes() {
        let binding = desktop::DesktopBinding::Macos {
            bundle_id: "test.app".into(),
            location: "/Applications/Test.app".into(),
            requirement: None,
        };
        assert_eq!(
            desktop_read_error(
                &binding,
                "应用所在磁盘未连接，请连接磁盘后刷新；原设置已保留".into()
            )
            .availability,
            AppAvailability::ReadError
        );
        assert_eq!(
            desktop_read_error(&binding, "应用身份或签名方式已变化，请重新关联".into())
                .availability,
            AppAvailability::NeedsRelink
        );
        assert_eq!(
            desktop_read_error(
                &binding,
                "未找到原关联的应用，请确认已安装或重新关联".into()
            )
            .availability,
            AppAvailability::NotInstalled
        );
    }

    #[test]
    fn v2_backup_keeps_legacy_windows_json_and_every_user_setting() {
        let root = tempfile::tempdir().unwrap();
        let storage = AppStorage::from_root(root.path().to_owned()).unwrap();
        let mut program = entry();
        program.binding = Some(
            app_binding::WindowsBinding {
                package_family_name: "Example.App_123456789abcd".into(),
                application_id: "App".into(),
            }
            .into(),
        );
        program.working_directory = Some("D:\\Projects".into());
        program.mode = ProgramProxyMode::Chromium;
        let document = ProgramDocument {
            schema_version: 2,
            revision: 19,
            programs: vec![program.clone()],
        };
        let bytes = serde_json::to_vec_pretty(&document).unwrap();
        let wire: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(wire["programs"][0]["binding"]["applicationId"], "App");
        assert!(wire["programs"][0]["binding"].get("kind").is_none());
        std::fs::write(root.path().join("proxy-programs.json"), &bytes).unwrap();
        storage.save_programs(&storage.programs().unwrap()).unwrap();
        assert_eq!(
            std::fs::read(root.path().join("proxy-programs.v2.backup.json")).unwrap(),
            bytes
        );
        let migrated = storage.programs().unwrap();
        assert_eq!(migrated.schema_version, 3);
        assert_eq!(migrated.revision, 19);
        assert_eq!(migrated.programs, vec![program]);
    }

    #[cfg(unix)]
    #[test]
    fn unix_child_receives_proxy_and_working_directory_without_parent_changes() {
        let root = tempfile::tempdir().unwrap();
        let before = std::env::var_os("HTTPS_PROXY");
        let cwd = std::env::current_dir().unwrap();
        let mut program = entry();
        program.executable = std::env::current_exe()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        program.working_directory = Some(root.path().to_string_lossy().into_owned());
        program.arguments = vec![
            "--exact".into(),
            "program_proxy::tests::proxy_child_helper".into(),
            "--nocapture".into(),
        ];
        let resolved = resolve_launch_program(&program, &ApplicationCatalog).unwrap();
        let output = proxy_command(&resolved, 17892)
            .env("ROUTEDECK_PROXY_TEST_HELPER", "1")
            .env("SERYLANE_PROXY_TEST_CWD", root.path())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("proxy-child-ok"));
        assert_eq!(std::env::var_os("HTTPS_PROXY"), before);
        assert_eq!(std::env::current_dir().unwrap(), cwd);
        let plain = root.path().join("not-executable");
        std::fs::write(&plain, b"fixture").unwrap();
        assert!(!is_executable(&plain));
    }

    #[test]
    fn migration_preserves_parameters_and_only_rebases_package_directories() {
        use crate::app_binding::{InstalledApplication, PackageRecord};
        let mut program = entry();
        program.mode = ProgramProxyMode::Chromium;
        program.working_directory = Some("C:\\Program Files\\Example\\work".into());
        let id = program.id;
        let binding = app_binding::WindowsBinding {
            package_family_name: "Example.App_123456789abcd".into(),
            application_id: "App".into(),
        };
        let application = InstalledApplication {
            binding: binding.clone().into(),
            name: "Example".into(),
            version: "1.0.0.0".into(),
            package_full_name: "registered-package".into(),
            package_root: "C:\\Program Files\\Example".into(),
            executable: program.executable.clone(),
            availability: AppAvailability::Ready,
            detail: String::new(),
        };
        let catalog = CatalogSnapshot {
            applications: vec![application],
            packages: vec![PackageRecord {
                family: binding.package_family_name.clone(),
                full_name: "registered-package".into(),
                availability: AppAvailability::Ready,
                detail: String::new(),
            }],
            warnings: vec![],
        };
        assert!(migrate_legacy_program(&mut program, &catalog));
        assert_eq!(program.id, id);
        assert_eq!(program.arguments, entry().arguments);
        assert_eq!(program.mode, ProgramProxyMode::Chromium);
        assert_eq!(program.working_directory_relative.as_deref(), Some("work"));
        assert_eq!(program.working_directory, None);
        assert!(!migrate_legacy_program(&mut program, &catalog));
        let mut custom = entry();
        custom.working_directory = Some("D:\\Projects".into());
        assert!(migrate_legacy_program(&mut custom, &catalog));
        assert_eq!(custom.working_directory.as_deref(), Some("D:\\Projects"));
    }

    #[test]
    fn bound_program_does_not_require_an_obsolete_executable_to_save() {
        let mut program = entry();
        program.binding = Some(
            app_binding::WindowsBinding {
                package_family_name: "Example.App_123456789abcd".into(),
                application_id: "App".into(),
            }
            .into(),
        );
        program.executable.clear();
        assert!(validate_fields(&program).is_ok());
        program.working_directory_relative = Some("..\\another-app".into());
        assert!(validate_fields(&program).is_err());
        program.working_directory_relative = Some("app\\work".into());
        assert!(validate_fields(&program).is_ok());
        program.working_directory = Some("C:\\Projects".into());
        assert!(validate_fields(&program).is_err());
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "registered disposable MSIX fixture; run scripts/test-windows-app-binding.ps1"]
    fn registered_app_upgrade_receives_proxy_environment() {
        let family = std::env::var("SERYLANE_BINDING_TEST_FAMILY").unwrap();
        let version = std::env::var("SERYLANE_BINDING_TEST_VERSION").unwrap();
        let binding = app_binding::WindowsBinding {
            package_family_name: family,
            application_id: "App".into(),
        };
        let catalog = ApplicationCatalog::default();
        let snapshot = catalog
            .query(Some(&binding.package_family_name), true)
            .unwrap();
        let application = app_binding::resolve(&binding.clone().into(), &snapshot)
            .application
            .unwrap();
        assert_eq!(application.version, version);
        assert_eq!(application.availability, AppAvailability::Ready);
        let installed_root = std::env::var("SERYLANE_BINDING_TEST_ROOT").unwrap();
        assert_eq!(
            app_binding::path_key(&application.executable),
            app_binding::path_key(&format!("{installed_root}\\app.exe"))
        );
        assert!(application
            .package_full_name
            .contains(&format!("_{version}_")));
        let mut program = entry();
        program.binding = Some(binding.into());
        program.executable = "C:\\old-version-removed\\app.exe".into();
        program.arguments = vec![
            "--exact".into(),
            "program_proxy::tests::proxy_child_helper".into(),
            "--nocapture".into(),
        ];
        let resolved = resolve_launch_program(&program, &catalog).unwrap();
        assert_eq!(
            app_binding::path_key(&resolved.executable),
            app_binding::path_key(&application.executable)
        );
        let output = proxy_command(&resolved, 17892)
            .env("ROUTEDECK_PROXY_TEST_HELPER", "1")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "relocated package child exited with {:?}: {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("proxy-child-ok"));
    }
    fn entry() -> ProxyProgram {
        ProxyProgram {
            id: Uuid::new_v4(),
            name: "示例".into(),
            executable: "C:\\Program Files\\Example\\app.exe".into(),
            binding: None,
            working_directory_relative: None,
            arguments: vec![
                "an argument with spaces".into(),
                "& not-a-shell-command".into(),
            ],
            working_directory: None,
            mode: ProgramProxyMode::Environment,
        }
    }
    #[test]
    fn validation_rejects_unknown_versions_duplicates_and_unsafe_fields() {
        let mut document = ProgramDocument::default();
        document.programs.push(entry());
        assert!(document.validate().is_ok());
        document.programs.push(document.programs[0].clone());
        assert!(document.validate().is_err());
        document.programs.pop();
        document.schema_version = 999;
        assert!(document.validate().is_err());
        document.schema_version = 1;
        document.programs[0].arguments.push("x\0y".into());
        assert!(document.validate().is_err());
    }
    #[test]
    fn mutations_are_revision_checked_and_missing_edits_do_not_create_entries() {
        let mut document = ProgramDocument::default();
        let program = entry();
        assert!(check_revision(&document, 1).is_err());
        update_document(&mut document, program.clone(), false).unwrap();
        assert_eq!(document.revision, 1);
        assert!(check_revision(&document, 0).is_err());
        let mut changed = program;
        changed.name = "编辑".into();
        update_document(&mut document, changed, true).unwrap();
        assert_eq!(document.programs.len(), 1);
        assert!(update_document(&mut document, entry(), true).is_err());
    }
    #[test]
    fn command_uses_literal_arguments_and_child_only_environment() {
        let program = entry();
        let before = std::env::var_os("HTTP_PROXY");
        let command = proxy_command(&program, 17890);
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            program
                .arguments
                .iter()
                .map(std::ffi::OsStr::new)
                .collect::<Vec<_>>()
        );
        assert!(command.get_envs().any(|(k, v)| k
            .to_string_lossy()
            .eq_ignore_ascii_case("HTTPS_PROXY")
            && v == Some(std::ffi::OsStr::new("http://127.0.0.1:17890"))));
        assert_eq!(std::env::var_os("HTTP_PROXY"), before);
    }
    #[test]
    fn chromium_proxy_flags_are_opt_in_and_cannot_be_overridden() {
        let mut program = entry();
        program.mode = ProgramProxyMode::Chromium;
        let command = proxy_command(&program, 17891);
        let args = command
            .get_args()
            .map(|a| a.to_string_lossy())
            .collect::<Vec<_>>();
        assert!(args.contains(&"--proxy-server=http://127.0.0.1:17891".into()));
        assert!(args.contains(&"--disable-quic".into()));
        assert_eq!(args[0], "--proxy-server=http://127.0.0.1:17891");
        program
            .arguments
            .push("--proxy-server=http://untrusted.invalid".into());
        assert!(validate_fields(&program).is_err());
    }
    #[cfg(windows)]
    #[test]
    fn existing_instance_detection_finds_this_process_without_changing_it() {
        let executable = std::env::current_exe().unwrap();
        assert!(running_program(&executable.to_string_lossy()).is_some());
    }
    #[test]
    fn corrupt_program_store_is_not_reset_or_rewritten() {
        let root = std::env::temp_dir().join(format!("routedeck-program-test-{}", Uuid::new_v4()));
        let storage = AppStorage::from_root(root.clone()).unwrap();
        let path = root.join("proxy-programs.json");
        let invalid = br#"{"schemaVersion":999,"revision":3,"programs":[]}"#;
        std::fs::write(&path, invalid).unwrap();
        assert!(storage.programs().is_err());
        assert_eq!(std::fs::read(&path).unwrap(), invalid);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn network_and_shell_paths_are_rejected_before_filesystem_access() {
        for path in [
            "\\\\server\\share\\app.exe",
            "\\\\?\\UNC\\server\\app.exe",
            "\\\\.\\pipe\\app.exe",
            "app.exe",
            "C:app.exe",
            "https://example.invalid/app.exe",
        ] {
            assert!(!local_windows_path(path));
        }
        for path in [
            "C:\\Apps\\app.exe",
            "D:/Apps/app.exe",
            "\\\\?\\C:\\Apps\\app.exe",
        ] {
            assert!(local_windows_path(path));
        }
    }
    #[cfg(windows)]
    #[test]
    fn normalization_accepts_existing_executable_and_rejects_missing_or_script_paths() {
        let executable = std::env::current_exe()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let input = ProgramInput {
            id: None,
            name: "  test  ".into(),
            executable,
            binding: None,
            working_directory_relative: None,
            arguments: vec![],
            working_directory: None,
            mode: ProgramProxyMode::Environment,
        };
        let normalized = normalize_input(input.clone()).unwrap();
        assert_eq!(normalized.name, "test");
        let mut missing = input.clone();
        missing.executable = format!("C:\\routedeck-missing-{}.exe", Uuid::new_v4());
        assert!(normalize_input(missing).is_err());
        let mut script = input;
        script.executable = "C:\\Windows\\test.cmd".into();
        assert!(normalize_input(script).is_err());
    }
    #[test]
    fn storage_round_trip_does_not_touch_program_files() {
        let root = std::env::temp_dir().join(format!("routedeck-program-test-{}", Uuid::new_v4()));
        let storage = AppStorage::from_root(root.clone()).unwrap();
        assert!(storage.programs().unwrap().programs.is_empty());
        let mut document = ProgramDocument::default();
        update_document(&mut document, entry(), false).unwrap();
        storage.save_programs(&document).unwrap();
        assert_eq!(storage.programs().unwrap().programs.len(), 1);
        document.programs.clear();
        storage.save_programs(&document).unwrap();
        assert!(storage.programs().unwrap().programs.is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }
    #[cfg(windows)]
    #[test]
    fn hidden_child_receives_proxy_without_opening_a_console() {
        let mut program = entry();
        program.executable = std::env::current_exe().unwrap().to_string_lossy().into();
        program.arguments = vec![
            "--exact".into(),
            "program_proxy::tests::proxy_child_helper".into(),
            "--nocapture".into(),
        ];
        let output = proxy_command(&program, 17892)
            .env("ROUTEDECK_PROXY_TEST_HELPER", "1")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("proxy-child-ok"));
    }
    #[cfg(windows)]
    #[test]
    fn relocated_test_executable_keeps_its_activation_manifest() {
        let root = std::env::temp_dir().join(format!("serylane-relocated-test-{}", Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let source = std::env::current_exe().unwrap();
        let target = root.join("app.exe");
        std::fs::copy(&source, &target).unwrap();
        let manifest = source.with_extension("exe.manifest");
        if manifest.exists() {
            std::fs::copy(manifest, root.join("app.exe.manifest")).unwrap();
        }
        let mut program = entry();
        program.executable = target.to_string_lossy().into();
        program.arguments = vec![
            "--exact".into(),
            "program_proxy::tests::proxy_child_helper".into(),
            "--nocapture".into(),
        ];
        let output = proxy_command(&program, 17892)
            .env("ROUTEDECK_PROXY_TEST_HELPER", "1")
            .env_remove("SERYLANE_BINDING_TEST_FAMILY")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "relocated helper status {:?}: {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("proxy-child-ok"));
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn proxy_child_helper() {
        if std::env::var_os("ROUTEDECK_PROXY_TEST_HELPER").is_none() {
            return;
        }
        assert_eq!(
            std::env::var("HTTPS_PROXY").unwrap(),
            "http://127.0.0.1:17892"
        );
        assert_eq!(
            std::env::var("NO_PROXY").unwrap(),
            "localhost,127.0.0.1,::1"
        );
        if let Some(expected) = std::env::var_os("SERYLANE_PROXY_TEST_CWD") {
            assert_eq!(
                std::env::current_dir().unwrap().canonicalize().unwrap(),
                Path::new(&expected).canonicalize().unwrap()
            );
        }
        #[cfg(windows)]
        assert!(unsafe { windows_sys::Win32::System::Console::GetConsoleWindow() }.is_null());
        #[cfg(windows)]
        if let Ok(expected) = std::env::var("SERYLANE_BINDING_TEST_FAMILY") {
            use windows::core::PWSTR;
            use windows::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, ERROR_SUCCESS};
            use windows::Win32::Storage::Packaging::Appx::GetCurrentPackageFamilyName;
            let mut size = 0;
            assert_eq!(
                unsafe { GetCurrentPackageFamilyName(&mut size, None) },
                ERROR_INSUFFICIENT_BUFFER
            );
            let mut value = vec![0u16; size as usize];
            assert_eq!(
                unsafe { GetCurrentPackageFamilyName(&mut size, Some(PWSTR(value.as_mut_ptr()))) },
                ERROR_SUCCESS
            );
            assert_eq!(
                String::from_utf16_lossy(&value[..size as usize - 1]),
                expected
            );
        }
        println!("proxy-child-ok");
    }
}
