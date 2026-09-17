//! Opt-in proxy launching, not process interception. No system environment,
//! registry, DNS, existing process, or proxy mode is modified here.
use crate::app_binding::{
    self, AppAvailability, AppBinding, ApplicationCatalog, ApplicationResolution, CatalogSnapshot,
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
            schema_version: 2,
            revision: 0,
            programs: Vec::new(),
        }
    }
}

impl ProgramDocument {
    pub fn validate(&self) -> AppResult<()> {
        if !matches!(self.schema_version, 1 | 2) || self.programs.len() > MAX_PROGRAMS {
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
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgramState {
    revision: u64,
    supported: bool,
    proxy_endpoint: String,
    core_running: bool,
    programs: Vec<ProgramEntry>,
}

#[derive(Default)]
pub struct ProgramProxyManager {
    // Serializes document mutations and starts, including double-clicks.
    children: Mutex<BTreeMap<Uuid, Child>>,
    catalog: ApplicationCatalog,
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
        || !local_windows_path(&program.executable)
        || !program.executable.to_ascii_lowercase().ends_with(".exe")
    {
        return Err(invalid("程序路径无效"));
    }
    if let Some(relative) = &program.working_directory_relative {
        if program.binding.is_none()
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
    if program.working_directory.as_ref().is_some_and(|p| {
        p.len() > 32760 || p.contains(['\0', '\r', '\n']) || !local_windows_path(p)
    }) {
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
    if program.binding.is_some() {
        // A saved package binding never pins a versioned executable path.
        program.executable.clear();
        return Ok(program);
    }
    // Reject UNC/device/relative paths before any metadata lookup so the picker
    // and manual input cannot accidentally initiate a network-share connection.
    if !local_windows_path(&program.executable) {
        return Err(invalid(
            "请选择本机磁盘上的程序，不支持相对路径、网络共享或设备路径",
        ));
    }
    let path = Path::new(&program.executable);
    if !path.is_absolute()
        || !path.is_file()
        || !path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("exe"))
    {
        return Err(invalid(
            "请选择存在的 .exe 文件，不能使用相对路径、脚本或快捷方式",
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
        if !local_windows_path(directory) {
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
        children.retain(|_, child| matches!(child.try_wait(), Ok(None)));
        children
            .iter()
            .map(|(id, child)| (*id, child.id()))
            .collect::<BTreeMap<_, _>>()
    };
    let mut families = BTreeMap::new();
    let programs = document
        .programs
        .into_iter()
        .map(|mut program| {
            let resolution = if let Some(binding) = &program.binding {
                let result = families
                    .entry(binding.package_family_name.to_lowercase())
                    .or_insert_with(|| {
                        manager
                            .catalog
                            .query(Some(&binding.package_family_name), fresh)
                    });
                match result {
                    Ok(catalog) => app_binding::resolve(binding, catalog),
                    Err(error) => {
                        ApplicationResolution::state(AppAvailability::ReadError, error.clone())
                    }
                }
            } else if Path::new(&program.executable).is_file() {
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
                    "程序文件已移动或删除，请重新选择文件。",
                )
            };
            if let Some(application) = &resolution.application {
                program.executable.clone_from(&application.executable);
            }
            ProgramEntry {
                available: resolution.availability == AppAvailability::Ready,
                running_pid: running.get(&program.id).copied(),
                program,
                resolution,
            }
        })
        .collect();
    Ok(ProgramState {
        revision: document.revision,
        supported: cfg!(windows),
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
        let mut changed = document.schema_version == 1;
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
                document.schema_version = 2;
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
) -> Result<CatalogSnapshot, AppErrorDto> {
    tauri::async_runtime::spawn_blocking(move || {
        app.state::<ProgramProxyManager>()
            .catalog
            .query(None, true)
            .map_err(AppError::Platform)
    })
    .await
    .map_err(|e| AppError::Runtime(e.to_string()).dto())?
    .map_err(|e| e.dto())
}

#[tauri::command]
pub async fn save_proxy_program(
    app: AppHandle,
    input: ProgramInput,
    expected_revision: u64,
) -> Result<ProgramState, AppErrorDto> {
    tauri::async_runtime::spawn_blocking(move || -> AppResult<ProgramState> {
        if !cfg!(windows) {
            return Err(invalid("程序代理启动目前仅支持 Windows"));
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
                let catalog = manager
                    .catalog
                    .query(Some(&binding.package_family_name), true)
                    .map_err(AppError::Platform)?;
                let resolved = app_binding::resolve(binding, &catalog);
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
            document.schema_version = 2;
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

fn proxy_command(program: &ProxyProgram, port: u16) -> Command {
    let endpoint = format!("http://127.0.0.1:{port}");
    let mut command = Command::new(&program.executable);
    if let Some(directory) = &program.working_directory {
        command.current_dir(directory);
    } else if let Some(directory) = Path::new(&program.executable).parent() {
        command.current_dir(directory);
    }
    // Child-only overrides. Do not use set_var, setx, shell profiles or registry.
    for key in [
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
    ] {
        command.env(key, &endpoint);
    }
    for key in ["NO_PROXY", "no_proxy"] {
        command.env(key, "localhost,127.0.0.1,::1");
    }
    if program.mode == ProgramProxyMode::Chromium {
        command
            .arg(format!("--proxy-server={endpoint}"))
            .arg("--disable-quic");
    }
    // Keep managed flags before a user-supplied `--` end-of-options separator.
    command.args(&program.arguments);
    command
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

#[cfg(windows)]
fn running_program(executable: &str) -> Option<u32> {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing().with_exe(UpdateKind::Always),
    );
    let key = |path: &Path| {
        path.to_string_lossy()
            .trim_start_matches("\\\\?\\")
            .to_lowercase()
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
        let snapshot = catalog
            .query(Some(&binding.package_family_name), true)
            .map_err(AppError::Platform)?;
        let resolution = app_binding::resolve(binding, &snapshot);
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
    normalized.binding.clone_from(&program.binding);
    Ok(normalized)
}

#[tauri::command]
pub async fn launch_proxy_program(
    app: AppHandle,
    program_id: Uuid,
    expected_revision: u64,
) -> Result<ProgramState, AppErrorDto> {
    tauri::async_runtime::spawn_blocking(move || -> AppResult<ProgramState> {
        if !cfg!(windows) { return Err(invalid("程序代理启动目前仅支持 Windows")); }
        let storage = AppStorage::from_app(&app)?;
        let manager = app.state::<ProgramProxyManager>();
        for attempt in 0..2 {
            let document = storage.programs()?;
            check_revision(&document, expected_revision)?;
            let saved = document.programs.iter().find(|p| p.id == program_id).ok_or_else(|| invalid("程序条目不存在"))?;
            // WinRT I/O occurs before acquiring either global configuration or
            // child-process locks. Resolution is always fresh at launch time.
            let program = resolve_launch_program(saved, &manager.catalog)?;
            let permit = crate::user_rules::acquire_configuration(&app)?;
            if app.state::<MihomoRuntime>().status(Some(&app)).phase != RuntimePhase::Running {
                return Err(AppError::Conflict("请先启动 Serylane 本地核心，再启动应用。".into()));
            }
            let mut children = manager.children.lock().map_err(|_| invalid("程序管理器繁忙"))?;
            check_revision(&storage.programs()?, expected_revision)?;
            if children.get_mut(&program_id).is_some_and(|c| matches!(c.try_wait(), Ok(None))) {
                return Err(AppError::Conflict("此应用已由 Serylane 启动。请先退出应用，再重新启动以应用新的代理设置。".into()));
            }
            #[cfg(windows)]
            {
                let bound_pid = program.binding.as_ref().and_then(app_binding::running_bound);
                if let Some(pid) = bound_pid.or_else(|| running_program(&program.executable)) {
                    return Err(AppError::Conflict(format!("此应用或同一应用包仍有后台进程（PID {pid}）。请先从应用中退出，再通过 Serylane 启动，让新的代理设置生效。")));
                }
            }
            let port = storage.settings()?.mixed_port;
            let endpoint = std::net::SocketAddr::from(([127, 0, 0, 1], port));
            std::net::TcpStream::connect_timeout(&endpoint, std::time::Duration::from_secs(2))
                .map_err(|_| AppError::Runtime("本地代理尚未就绪，请检查核心状态后重试；应用尚未启动。".into()))?;
            match proxy_command(&program, port).spawn() {
                Ok(child) => { children.insert(program_id, child); drop(children); drop(permit); return snapshot(&app, &storage, false); }
                Err(error) if attempt == 0 && saved.binding.is_some() && error.kind() == std::io::ErrorKind::NotFound => {
                    // Only a definitely failed spawn is retried. Never replay an
                    // activation whose process creation may already have succeeded.
                    drop(children); drop(permit); continue;
                }
                Err(error) => return Err(AppError::Runtime(format!("应用启动未完成，请刷新安装信息后重试。详情：{error}"))),
            }
        }
        Err(invalid("应用安装信息正在变化，请稍后刷新重试。"))
    }).await.map_err(|e| AppError::Runtime(e.to_string()).dto())?.map_err(|e| e.dto())
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
    #[cfg(not(windows))]
    {
        let _ = window;
        Err(invalid("文件选择目前仅支持 Windows").dto())
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
        assert_eq!(storage.programs().unwrap().schema_version, 2);
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
    fn migration_preserves_parameters_and_only_rebases_package_directories() {
        use crate::app_binding::{InstalledApplication, PackageRecord};
        let mut program = entry();
        program.mode = ProgramProxyMode::Chromium;
        program.working_directory = Some("C:\\Program Files\\Example\\work".into());
        let id = program.id;
        let binding = AppBinding {
            package_family_name: "Example.App_123456789abcd".into(),
            application_id: "App".into(),
        };
        let application = InstalledApplication {
            binding: binding.clone(),
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
        program.binding = Some(AppBinding {
            package_family_name: "Example.App_123456789abcd".into(),
            application_id: "App".into(),
        });
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
        let binding = AppBinding {
            package_family_name: family,
            application_id: "App".into(),
        };
        let catalog = ApplicationCatalog::default();
        let snapshot = catalog
            .query(Some(&binding.package_family_name), true)
            .unwrap();
        let application = app_binding::resolve(&binding, &snapshot)
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
        program.binding = Some(binding);
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
    #[cfg(windows)]
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
        assert!(unsafe { windows_sys::Win32::System::Console::GetConsoleWindow() }.is_null());
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
