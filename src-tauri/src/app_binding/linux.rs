//! Desktop Entry / XDG application discovery and argv-only launch planning.
//! No shell expansion or application activation happens in this module.
//! https://specifications.freedesktop.org/desktop-entry/latest/
use super::desktop::{DesktopApplication, DesktopBinding};
use super::AppAvailability;
use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

const MAX_FILE_BYTES: u64 = 256 * 1024;
const MAX_ENTRIES: usize = 8192;
const MAX_DEPTH: usize = 8;
const MAX_ROOTS: usize = 64;

#[derive(Clone, Debug)]
struct Context {
    roots: Vec<PathBuf>,
    search_path: Vec<PathBuf>,
    locale: String,
    desktops: Vec<String>,
}

impl Context {
    fn current() -> Self {
        Self::from_values(|key| std::env::var(key).ok())
    }

    fn from_values(get: impl Fn(&str) -> Option<String>) -> Self {
        let nonempty = |key| get(key).filter(|s| !s.is_empty());
        let home = nonempty("HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute());
        let user = nonempty("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| home.map(|p| p.join(".local/share")));
        let system =
            nonempty("XDG_DATA_DIRS").unwrap_or_else(|| "/usr/local/share:/usr/share".into());
        let mut roots = Vec::new();
        for base in user.into_iter().chain(system.split(':').map(PathBuf::from)) {
            if base.is_absolute() {
                let root = base.join("applications");
                if !roots.contains(&root) {
                    roots.push(root);
                }
            }
        }
        let search_path = nonempty("PATH")
            .unwrap_or_else(|| "/usr/local/bin:/usr/bin:/bin".into())
            .split(':')
            .map(PathBuf::from)
            // Empty/relative PATH entries depend on cwd; do not bind to them.
            .filter(|p| p.is_absolute())
            .take(256)
            .collect();
        Self {
            roots,
            search_path,
            locale: nonempty("LC_ALL")
                .or_else(|| nonempty("LC_MESSAGES"))
                .or_else(|| nonempty("LANG"))
                .unwrap_or_else(|| "C".into()),
            desktops: nonempty("XDG_CURRENT_DESKTOP")
                .unwrap_or_default()
                .split(':')
                .filter(|v| !v.is_empty())
                .map(str::to_string)
                .collect(),
        }
    }
}

pub fn collect() -> Result<Vec<DesktopApplication>, String> {
    collect_with(&Context::current())
}

pub fn resolve(binding: &DesktopBinding) -> Result<DesktopApplication, String> {
    resolve_with(binding, &Context::current())
}

pub fn inspect(path: &Path) -> Result<DesktopApplication, String> {
    inspect_with(path, &Context::current())
}

/// Called on Tauri's GTK main thread. The reply is asynchronous; never run a
/// nested GTK event loop or block the main thread while waiting for selection.
#[cfg(target_os = "linux")]
pub fn choose(
    reply: std::sync::mpsc::SyncSender<Result<Option<String>, String>>,
) -> Result<(), String> {
    use gtk::prelude::*;
    use std::cell::RefCell;
    use std::rc::Rc;
    if !gtk::is_initialized_main_thread() {
        return Err("应用文件选择器需要在桌面主线程打开，请重试。".into());
    }
    let dialog = gtk::FileChooserNative::new(
        Some("选择应用入口或可执行文件"),
        None::<&gtk::Window>,
        gtk::FileChooserAction::Open,
        Some("选择"),
        Some("取消"),
    );
    dialog.set_select_multiple(false);
    dialog.set_local_only(true);
    let desktop = gtk::FileFilter::new();
    desktop.set_name(Some("桌面应用入口（.desktop）"));
    desktop.add_pattern("*.desktop");
    dialog.add_filter(&desktop);
    let all = gtk::FileFilter::new();
    all.set_name(Some("所有文件（含无扩展名的可执行程序）"));
    all.add_pattern("*");
    dialog.add_filter(&all);
    dialog.set_filter(&all);
    // Keep the native dialog alive until response. Break this cycle on every
    // response, including cancel/delete; Gtk owns the response handler itself.
    let pending = Rc::new(RefCell::new(Some(dialog.clone())));
    let completion = pending.clone();
    dialog.connect_response(move |dialog, response| {
        let result = if response == gtk::ResponseType::Accept {
            dialog
                .filename()
                .ok_or_else(|| "请选择本机应用文件。".into())
                .and_then(|path| {
                    path.to_str()
                        .map(|s| Some(s.to_string()))
                        .ok_or_else(|| "应用路径不是有效 UTF-8。".into())
                })
        } else {
            Ok(None)
        };
        let _ = reply.try_send(result);
        dialog.destroy();
        completion.borrow_mut().take();
    });
    dialog.show();
    Ok(())
}

fn desktop_id(root: &Path, path: &Path) -> Option<String> {
    let relative = path.strip_prefix(root).ok()?;
    let mut parts = Vec::new();
    for part in relative.components() {
        let Component::Normal(value) = part else {
            return None;
        };
        parts.push(value.to_str()?);
    }
    let id = parts.join("-");
    (!id.is_empty() && id.len() <= 1024).then_some(id)
}

/// Keep lexical paths, not canonical targets: a symlink updated by the package
/// manager at the same desktop-entry location remains the user's chosen entry.
fn absolute_desktop(path: &Path) -> Result<String, String> {
    if !path.is_absolute()
        || path.extension().and_then(|s| s.to_str()) != Some("desktop")
        || path
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    {
        return Err("请选择绝对路径下的 .desktop 应用入口。".into());
    }
    let value = path.to_str().ok_or("应用入口路径不是有效 UTF-8。")?;
    if value.len() > 16384 || value.chars().any(char::is_control) {
        return Err("应用入口路径格式不正确。".into());
    }
    Ok(value.into())
}

type Index = BTreeMap<String, Vec<PathBuf>>;

fn index(context: &Context) -> Result<Index, String> {
    if context.roots.len() > MAX_ROOTS {
        return Err("应用目录数量超过扫描上限，请检查 XDG_DATA_DIRS。".into());
    }
    let mut result = BTreeMap::new();
    let mut visited = 0;
    for root in &context.roots {
        let mut local: Index = BTreeMap::new();
        walk(root, root, 0, &mut visited, &mut local)?;
        // First root wins even when the entry is Hidden or malformed. Falling
        // through would resurrect an intentionally hidden system application.
        for (id, paths) in local {
            result.entry(id).or_insert(paths);
        }
    }
    Ok(result)
}

fn walk(
    root: &Path,
    dir: &Path,
    depth: usize,
    visited: &mut usize,
    out: &mut Index,
) -> Result<(), String> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err("部分应用目录读取失败，请检查目录权限后刷新。".into()),
    };
    for entry in entries {
        *visited += 1;
        if *visited > MAX_ENTRIES {
            return Err("应用目录内容超过扫描上限，请精简应用目录后刷新。".into());
        }
        let entry = entry.map_err(|_| "部分应用目录读取失败，请刷新重试。")?;
        let kind = entry
            .file_type()
            .map_err(|_| "应用入口状态读取失败，请刷新重试。")?;
        let path = entry.path();
        // Never follow directory links (cycles / unbounded external trees).
        if kind.is_dir() {
            if depth >= MAX_DEPTH {
                return Err("应用目录层级超过扫描上限，请直接选择所需 .desktop 文件。".into());
            }
            walk(root, &path, depth + 1, visited, out)?;
        } else if (kind.is_file() || kind.is_symlink())
            && path.extension().and_then(|s| s.to_str()) == Some("desktop")
        {
            if let Some(id) = desktop_id(root, &path) {
                out.entry(id).or_default().push(path);
            }
        }
    }
    Ok(())
}

fn binding_for(path: &Path, context: &Context) -> Result<DesktopBinding, String> {
    Ok(DesktopBinding::Linux {
        desktop_id: context
            .roots
            .iter()
            .find_map(|root| desktop_id(root, path))
            .unwrap_or_default(),
        location: absolute_desktop(path)?,
    })
}

fn placeholder(
    binding: DesktopBinding,
    availability: AppAvailability,
    detail: impl Into<String>,
) -> DesktopApplication {
    let location = match &binding {
        DesktopBinding::Linux { location, .. } => location,
        _ => unreachable!("Linux-only binding"),
    };
    DesktopApplication {
        name: Path::new(location)
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
        version: String::new(),
        root: Path::new(location)
            .parent()
            .unwrap_or_else(|| Path::new("/"))
            .to_string_lossy()
            .into_owned(),
        executable: String::new(),
        availability,
        detail: detail.into(),
        arguments: Vec::new(),
        working_directory: None,
        binding,
    }
}

fn collect_with(context: &Context) -> Result<Vec<DesktopApplication>, String> {
    let mut applications = Vec::new();
    for (id, paths) in index(context)? {
        let binding = DesktopBinding::Linux {
            desktop_id: id,
            location: absolute_desktop(&paths[0])?,
        };
        if paths.len() != 1 {
            applications.push(placeholder(
                binding,
                AppAvailability::NeedsRelink,
                "发现重名的桌面入口，请先调整冲突的 .desktop 文件。",
            ));
            continue;
        }
        match read_entry(&paths[0]) {
            Ok(entry) => {
                let visible = (|| -> Result<bool, String> {
                    Ok(!entry.boolean("Hidden")?
                        && !entry.boolean("NoDisplay")?
                        && entry.visible(context)?)
                })();
                match visible {
                    Ok(false) => continue,
                    Err(message) => {
                        applications.push(placeholder(
                            binding,
                            AppAvailability::ReadError,
                            message,
                        ));
                        continue;
                    }
                    Ok(true) => {}
                }
                if entry.value("Type") != Some("Application") {
                    continue;
                }
                applications.push(build(entry, binding, context));
            }
            Err(message) => {
                applications.push(placeholder(binding, AppAvailability::ReadError, message))
            }
        }
    }
    applications.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then(a.root.cmp(&b.root))
    });
    Ok(applications)
}

fn inspect_with(path: &Path, context: &Context) -> Result<DesktopApplication, String> {
    let binding = binding_for(path, context)?;
    // Registered entries must obey the same precedence rules as list/launch.
    if let DesktopBinding::Linux { desktop_id, .. } = &binding {
        if !desktop_id.is_empty() {
            return resolve_with(&binding, context);
        }
    }
    Ok(load_bound(binding, context))
}

fn resolve_with(binding: &DesktopBinding, context: &Context) -> Result<DesktopApplication, String> {
    let DesktopBinding::Linux {
        desktop_id: id,
        location,
    } = binding
    else {
        return Err("此应用关联属于其他操作系统，请重新选择 Linux 应用。".into());
    };
    absolute_desktop(Path::new(location))?;
    if id.contains(['/', '\\', '\0', '\r', '\n'])
        || id.len() > 1024
        || (!id.is_empty() && !id.ends_with(".desktop"))
    {
        return Err("桌面应用标识格式不正确，请重新关联。".into());
    }
    if !id.is_empty() {
        let entries = index(context)?;
        let Some(paths) = entries.get(id) else {
            return Ok(placeholder(
                binding.clone(),
                AppAvailability::NotInstalled,
                "当前应用目录中没有此入口，原代理配置已保留。",
            ));
        };
        if paths.len() != 1 || paths[0] != Path::new(location) {
            return Ok(placeholder(
                binding.clone(),
                AppAvailability::NeedsRelink,
                "桌面入口的来源或优先级已变化，请重新选择应用；原配置已保留。",
            ));
        }
    } else if context
        .roots
        .iter()
        .any(|root| desktop_id(root, Path::new(location)).is_some())
    {
        // A previously manual file now belongs to the XDG catalog: do not evade
        // a newly introduced override/tombstone by treating it as still manual.
        return Ok(placeholder(
            binding.clone(),
            AppAvailability::NeedsRelink,
            "此文件现在属于已安装应用目录，请重新选择以确认入口身份。",
        ));
    }
    Ok(load_bound(binding.clone(), context))
}

fn load_bound(binding: DesktopBinding, context: &Context) -> DesktopApplication {
    let location = match &binding {
        DesktopBinding::Linux { location, .. } => location,
        _ => unreachable!(),
    };
    match fs::metadata(location) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return placeholder(
                binding,
                AppAvailability::NotInstalled,
                "应用入口已移除，原代理配置已保留。",
            );
        }
        Err(_) => {
            return placeholder(
                binding,
                AppAvailability::ReadError,
                "应用入口读取失败，请检查文件权限。",
            )
        }
        Ok(_) => {}
    }
    match read_entry(Path::new(location)) {
        Ok(entry) => build(entry, binding, context),
        Err(message) => placeholder(binding, AppAvailability::ReadError, message),
    }
}

#[derive(Debug)]
struct Entry(BTreeMap<String, String>);

impl Entry {
    fn value(&self, key: &str) -> Option<&str> {
        self.0.get(key).map(String::as_str)
    }
    fn text(&self, key: &str) -> Result<String, String> {
        unescape(self.value(key).unwrap_or_default())
    }
    fn boolean(&self, key: &str) -> Result<bool, String> {
        match self.value(key) {
            None | Some("false") => Ok(false),
            Some("true") => Ok(true),
            _ => Err(format!("桌面入口的 {key} 字段格式不正确。")),
        }
    }
    fn list(&self, key: &str) -> Result<Vec<String>, String> {
        let mut values = vec![String::new()];
        let mut chars = self.value(key).unwrap_or_default().chars();
        while let Some(c) = chars.next() {
            if c == ';' {
                values.push(String::new());
            } else if c == '\\' {
                let next = chars.next().ok_or("桌面入口列表转义不完整。")?;
                values.last_mut().unwrap().push(match next {
                    ';' => ';',
                    's' => ' ',
                    't' => '\t',
                    'n' => '\n',
                    'r' => '\r',
                    '\\' => '\\',
                    _ => return Err("桌面入口列表含未知转义。".into()),
                });
            } else {
                values.last_mut().unwrap().push(c);
            }
        }
        values.retain(|v| !v.is_empty());
        Ok(values)
    }
    fn visible(&self, context: &Context) -> Result<bool, String> {
        let only = self.list("OnlyShowIn")?;
        let not = self.list("NotShowIn")?;
        if only.iter().any(|v| not.contains(v)) {
            return Err("桌面环境显示规则互相冲突。".into());
        }
        for desktop in &context.desktops {
            if only.contains(desktop) {
                return Ok(true);
            }
            if not.contains(desktop) {
                return Ok(false);
            }
        }
        Ok(self.value("OnlyShowIn").is_none())
    }
    fn localized_name(&self, context: &Context) -> Result<String, String> {
        if self.value("Name").is_none() {
            return Err("桌面入口缺少应用名称。".into());
        }
        for locale in locale_candidates(&context.locale) {
            if let Some(value) = self.value(&format!("Name[{locale}]")) {
                return unescape(value);
            }
        }
        self.text("Name")
    }
}

fn read_entry(path: &Path) -> Result<Entry, String> {
    let metadata = fs::metadata(path).map_err(|_| "应用入口读取失败，请检查文件权限。")?;
    if !metadata.is_file() || metadata.len() > MAX_FILE_BYTES {
        return Err("应用入口不是普通文件或内容超过大小上限。".into());
    }
    let mut bytes = Vec::new();
    fs::File::open(path)
        .map_err(|_| "应用入口读取失败，请检查文件权限。")?
        .take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "应用入口内容读取失败。")?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err("应用入口内容超过大小上限。".into());
    }
    let content = std::str::from_utf8(&bytes).map_err(|_| "桌面入口不是有效 UTF-8 文件。")?;
    parse_entry(content)
}

fn parse_entry(content: &str) -> Result<Entry, String> {
    if content.contains('\0') {
        return Err("桌面入口含无效字符。".into());
    }
    let mut values = BTreeMap::new();
    let mut in_entry = false;
    let mut found = false;
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            if !line.ends_with(']') {
                return Err("桌面入口分组格式不正确。".into());
            }
            in_entry = line == "[Desktop Entry]";
            if in_entry && found {
                return Err("桌面入口包含重复的主分组。".into());
            }
            found |= in_entry;
        } else if in_entry {
            let (key, value) = line.split_once('=').ok_or("桌面入口字段缺少等号。")?;
            let key = key.trim();
            if key.is_empty()
                || values
                    .insert(key.to_string(), value.trim().to_string())
                    .is_some()
            {
                return Err("桌面入口包含空白或重复字段。".into());
            }
        }
    }
    if !found {
        return Err("文件缺少 Desktop Entry 主分组。".into());
    }
    Ok(Entry(values))
}

fn locale_candidates(locale: &str) -> Vec<String> {
    let (base, modifier) = locale
        .split_once('@')
        .map_or((locale, None), |(b, m)| (b, Some(m)));
    let base = base.split('.').next().unwrap_or_default();
    let language = base.split('_').next().unwrap_or_default();
    if language.is_empty() || matches!(language, "C" | "POSIX") {
        return Vec::new();
    }
    let mut result = Vec::new();
    if let Some(modifier) = modifier {
        result.push(format!("{base}@{modifier}"));
    }
    result.push(base.into());
    if let Some(modifier) = modifier {
        result.push(format!("{language}@{modifier}"));
    }
    result.push(language.into());
    result
}

fn unescape(value: &str) -> Result<String, String> {
    let mut result = String::new();
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        result.push(if c == '\\' {
            match chars.next() {
                Some('s') => ' ',
                Some('n') => '\n',
                Some('t') => '\t',
                Some('r') => '\r',
                Some('\\') => '\\',
                _ => return Err("桌面入口含未知或不完整的转义。".into()),
            }
        } else {
            c
        });
    }
    Ok(result)
}

fn build(entry: Entry, binding: DesktopBinding, context: &Context) -> DesktopApplication {
    let mut app = placeholder(
        binding,
        AppAvailability::Ready,
        "启动前会重新读取桌面入口；代理设置仅传给本次启动的进程。",
    );
    if let Ok(name) = entry.localized_name(context) {
        if !name.is_empty() {
            app.name = name;
        }
    }
    let state = (|| -> Result<(), (AppAvailability, String)> {
        let invalid = |message| (AppAvailability::ReadError, message);
        if entry.boolean("Hidden").map_err(invalid)? {
            return Err((
                AppAvailability::NotInstalled,
                "此应用入口已在当前用户下隐藏或移除，原配置已保留。".into(),
            ));
        }
        if entry.value("Type") != Some("Application") {
            return Err((
                AppAvailability::UnsupportedLaunch,
                "此文件不是可启动的应用入口，请选择 Type=Application 的 .desktop 文件。".into(),
            ));
        }
        let name = entry.localized_name(context).map_err(invalid)?;
        if name.is_empty() {
            return Err(invalid("桌面入口的应用名称为空。".into()));
        }
        app.name = name;
        for key in ["DBusActivatable", "Terminal"] {
            if entry.boolean(key).map_err(invalid)? {
                return Err((
                    AppAvailability::UnsupportedLaunch,
                    if key == "Terminal" {
                        "此入口需要终端会话，暂不支持通过应用代理启动。"
                    } else {
                        "此入口使用 D-Bus 激活，暂不支持为新进程传入代理设置。"
                    }
                    .into(),
                ));
            }
        }
        if entry.value("X-Flatpak").is_some() || entry.value("X-SnapInstanceName").is_some() {
            return Err((
                AppAvailability::UnsupportedLaunch,
                "此沙盒应用需要专用启动适配，目前保留选择信息但不执行代理启动。".into(),
            ));
        }
        let location = match &app.binding {
            DesktopBinding::Linux { location, .. } => location,
            _ => unreachable!(),
        };
        let argv = exec_arguments(
            entry
                .value("Exec")
                .ok_or_else(|| invalid("桌面入口缺少 Exec 启动命令。".into()))?,
            &app.name,
            location,
            &entry.text("Icon").map_err(invalid)?,
        )
        .map_err(invalid)?;
        let command = &argv[0];
        let base = Path::new(command)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        if matches!(
            base,
            "flatpak"
                | "snap"
                | "env"
                | "sh"
                | "bash"
                | "dash"
                | "zsh"
                | "fish"
                | "pkexec"
                | "sudo"
        ) || command.starts_with("/snap/")
            || command.starts_with("/var/lib/snapd/")
        {
            return Err((
                AppAvailability::UnsupportedLaunch,
                "此入口使用沙盒、脚本或环境包装器，请选择应用的直接启动入口。".into(),
            ));
        }
        if let Some(try_exec) = entry.value("TryExec") {
            let candidate = unescape(try_exec).map_err(invalid)?;
            if find_executable(&candidate, context).is_none() {
                return Err((
                    AppAvailability::MissingFile,
                    "应用的安装检测程序不存在或没有执行权限，请检查安装后刷新。".into(),
                ));
            }
        }
        let executable = find_executable(command, context).ok_or_else(|| {
            (
                AppAvailability::MissingFile,
                "应用启动文件不存在或没有执行权限，请检查安装后刷新。".into(),
            )
        })?;
        app.executable = executable
            .to_str()
            .ok_or_else(|| invalid("应用启动路径不是有效 UTF-8。".into()))?
            .to_string();
        app.arguments = argv.into_iter().skip(1).collect();
        let directory = entry.text("Path").map_err(invalid)?;
        if !directory.is_empty() {
            if directory.chars().any(char::is_control) || !Path::new(&directory).is_absolute() {
                return Err(invalid("应用工作目录需要使用绝对路径。".into()));
            }
            if !Path::new(&directory).is_dir() {
                return Err((
                    AppAvailability::MissingFile,
                    "应用工作目录已移除，请检查桌面入口后刷新。".into(),
                ));
            }
            app.working_directory = Some(directory);
        }
        Ok(())
    })();
    if let Err((availability, detail)) = state {
        app.availability = availability;
        app.detail = detail;
    }
    app
}

fn find_executable(command: &str, context: &Context) -> Option<PathBuf> {
    if command.is_empty() || command.contains('=') || command.chars().any(char::is_control) {
        return None;
    }
    let path = Path::new(command);
    if path.is_absolute() {
        return executable(path).then(|| path.to_path_buf());
    }
    if path.components().count() != 1 {
        return None;
    }
    context
        .search_path
        .iter()
        .map(|root| root.join(path))
        .find(|path| executable(path))
}

fn executable(path: &Path) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        use std::os::unix::fs::PermissionsExt;
        unsafe extern "C" {
            fn access(path: *const std::ffi::c_char, mode: std::ffi::c_int) -> std::ffi::c_int;
        }
        let Ok(path) = CString::new(path.as_os_str().as_bytes()) else {
            return false;
        };
        // X_OK checks this user's access (including parent directory/ACL
        // permissions), not just whether *someone* has an executable bit.
        // Actual spawn still revalidates; never chmod user applications.
        metadata.permissions().mode() & 0o111 != 0 && unsafe { access(path.as_ptr(), 1) == 0 }
    }
    #[cfg(not(unix))]
    {
        true
    }
}

#[derive(Debug)]
struct Token {
    value: String,
    quoted: bool,
}

fn tokenize(value: &str) -> Result<Vec<Token>, String> {
    let mut chars = value.chars().peekable();
    let mut tokens = Vec::new();
    while chars.peek().is_some() {
        while chars.peek().is_some_and(|c| *c == ' ' || *c == '\t') {
            chars.next();
        }
        if chars.peek().is_none() {
            break;
        }
        let quoted = chars.peek() == Some(&'"');
        if quoted {
            chars.next();
        }
        let mut value = String::new();
        let mut closed = !quoted;
        while let Some(c) = chars.next() {
            if quoted && c == '"' {
                closed = true;
                break;
            }
            if !quoted && matches!(c, ' ' | '\t') {
                break;
            }
            if c.is_control() {
                return Err("启动命令含控制字符。".into());
            }
            if quoted && c == '\\' {
                let escaped = chars.next().ok_or("启动命令转义不完整。")?;
                if !matches!(escaped, '"' | '`' | '$' | '\\') {
                    return Err("启动命令含未知引号转义。".into());
                }
                value.push(escaped);
            } else {
                if (!quoted
                    && matches!(
                        c,
                        '"' | '\''
                            | '\\'
                            | '>'
                            | '<'
                            | '~'
                            | '|'
                            | '&'
                            | ';'
                            | '$'
                            | '*'
                            | '?'
                            | '#'
                            | '('
                            | ')'
                            | '`'
                    ))
                    || (quoted && matches!(c, '$' | '`'))
                {
                    return Err("启动命令含未正确引用的保留字符。".into());
                }
                value.push(c);
            }
        }
        if !closed || (quoted && chars.peek().is_some_and(|c| !matches!(*c, ' ' | '\t'))) {
            return Err("启动命令的引号需要完整包围一个参数。".into());
        }
        tokens.push(Token { value, quoted });
        if tokens.len() > 512 {
            return Err("启动参数数量超过上限。".into());
        }
    }
    Ok(tokens)
}

fn exec_arguments(
    value: &str,
    name: &str,
    location: &str,
    icon: &str,
) -> Result<Vec<String>, String> {
    let decoded = unescape(value)?;
    let tokens = tokenize(&decoded)?;
    if tokens.is_empty() {
        return Err("启动命令为空。".into());
    }
    let mut arguments = Vec::new();
    let mut files = 0;
    for (position, token) in tokens.into_iter().enumerate() {
        if token.value == "%i" && !token.quoted && position != 0 {
            if !icon.is_empty() {
                arguments.extend(["--icon".into(), icon.into()]);
            }
            continue;
        }
        let mut output = String::new();
        let mut chars = token.value.chars();
        let mut removed = false;
        while let Some(c) = chars.next() {
            if c != '%' {
                output.push(c);
                continue;
            }
            let code = chars.next().ok_or("启动命令含不完整的百分号字段。")?;
            if code == '%' {
                output.push('%');
                continue;
            }
            if token.quoted || position == 0 {
                return Err("启动命令在程序名或引号内含字段占位符。".into());
            }
            match code {
                'f' | 'u' | 'F' | 'U' => {
                    files += 1;
                    if files > 1 || (matches!(code, 'F' | 'U') && token.value.len() != 2) {
                        return Err("启动命令的文件字段格式不正确。".into());
                    }
                    removed = true; // This launcher does not supply files/URLs.
                }
                'c' => output.push_str(name),
                'k' => output.push_str(location),
                'd' | 'D' | 'n' | 'N' | 'v' | 'm' => removed = true,
                _ => return Err("启动命令含未知或位置不正确的字段。".into()),
            }
        }
        if !output.is_empty() || !removed {
            arguments.push(output);
        }
    }
    if arguments
        .first()
        .is_none_or(|a| a.is_empty() || a.contains('='))
    {
        return Err("启动程序名称格式不正确。".into());
    }
    Ok(arguments)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    struct Fixture {
        directory: TempDir,
        context: Context,
    }
    impl Fixture {
        fn new() -> Self {
            let directory = tempfile::tempdir().unwrap();
            let roots = ["user/applications", "system/applications"]
                .map(|p| directory.path().join(p))
                .to_vec();
            for root in &roots {
                fs::create_dir_all(root).unwrap();
            }
            let bin = directory.path().join("bin");
            fs::create_dir(&bin).unwrap();
            fs::write(bin.join("fixture-app"), "not executed").unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(bin.join("fixture-app"), fs::Permissions::from_mode(0o755))
                    .unwrap();
            }
            let context = Context {
                roots,
                search_path: vec![bin],
                locale: "zh_CN.UTF-8".into(),
                desktops: vec!["GNOME".into()],
            };
            Self { directory, context }
        }
        fn entry(&self, root: usize, name: &str, body: &str) -> PathBuf {
            let path = self.context.roots[root].join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, body).unwrap();
            path
        }
        fn ordinary(&self, root: usize, name: &str, extra: &str) -> PathBuf {
            self.entry(root, name, &format!("[Desktop Entry]\nType=Application\nName=Fixture App\nExec=fixture-app --ordinary %U\n{extra}"))
        }
    }

    #[test]
    fn xdg_defaults_ignore_relative_values_and_empty_path_entries() {
        let values = BTreeMap::from([
            ("HOME", "/home/fixture"),
            ("XDG_DATA_HOME", "relative"),
            ("XDG_DATA_DIRS", "/one:relative:/two:/one"),
            ("PATH", ":relative:/bin:"),
            ("LC_ALL", "zh_CN"),
            ("LC_MESSAGES", "en_US"),
        ]);
        let context = Context::from_values(|key| values.get(key).map(|v| (*v).into()));
        assert_eq!(
            context.roots,
            vec![
                PathBuf::from("/home/fixture/.local/share/applications"),
                PathBuf::from("/one/applications"),
                PathBuf::from("/two/applications")
            ]
        );
        assert_eq!(context.search_path, vec![PathBuf::from("/bin")]);
        assert_eq!(context.locale, "zh_CN");
    }

    #[test]
    fn hidden_user_tombstone_masks_system_app() {
        let f = Fixture::new();
        f.ordinary(1, "fixture.desktop", "");
        f.entry(0, "fixture.desktop", "[Desktop Entry]\nHidden=true\n");
        assert!(collect_with(&f.context).unwrap().is_empty());
        let binding = binding_for(&f.context.roots[0].join("fixture.desktop"), &f.context).unwrap();
        assert_eq!(
            resolve_with(&binding, &f.context).unwrap().availability,
            AppAvailability::NotInstalled
        );
    }

    #[test]
    fn no_display_and_desktop_rules_filter_catalog_not_manual_entries() {
        let f = Fixture::new();
        let path = f.ordinary(0, "hidden-menu.desktop", "NoDisplay=true\n");
        f.ordinary(0, "kde.desktop", "OnlyShowIn=KDE;\n");
        f.ordinary(0, "not-gnome.desktop", "NotShowIn=GNOME;\n");
        f.ordinary(0, "gnome.desktop", "OnlyShowIn=GNOME;\n");
        let list = collect_with(&f.context).unwrap();
        assert_eq!(list.len(), 1);
        assert!(
            matches!(&list[0].binding, DesktopBinding::Linux { desktop_id, .. } if desktop_id == "gnome.desktop")
        );
        assert_eq!(
            inspect_with(&path, &f.context).unwrap().availability,
            AppAvailability::Ready
        );
    }

    #[test]
    fn duplicate_ids_in_one_root_are_not_arbitrarily_selected() {
        let f = Fixture::new();
        let path = f.ordinary(0, "foo-bar.desktop", "");
        f.ordinary(0, "foo/bar.desktop", "");
        let list = collect_with(&f.context).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].availability, AppAvailability::NeedsRelink);
        assert_eq!(
            resolve_with(&binding_for(&path, &f.context).unwrap(), &f.context)
                .unwrap()
                .availability,
            AppAvailability::NeedsRelink
        );
    }

    #[test]
    fn update_follows_same_entry_but_new_override_requires_confirmation() {
        let f = Fixture::new();
        let path = f.ordinary(1, "org.fixture.App.desktop", "");
        let binding = binding_for(&path, &f.context).unwrap();
        fs::write(&path, "[Desktop Entry]\nType=Application\nName=New Version\nExec=fixture-app --new\nVersion=1.5\n").unwrap();
        let resolved = resolve_with(&binding, &f.context).unwrap();
        assert_eq!(resolved.availability, AppAvailability::Ready);
        assert_eq!(resolved.name, "New Version");
        assert_eq!(resolved.arguments, vec!["--new"]);
        assert_eq!(
            resolved.version, "",
            "Desktop Version is a schema version, not the application's version"
        );
        f.ordinary(0, "org.fixture.App.desktop", "");
        assert_eq!(
            resolve_with(&binding, &f.context).unwrap().availability,
            AppAvailability::NeedsRelink
        );
        fs::remove_file(&path).unwrap();
        assert_eq!(
            resolve_with(&binding, &f.context).unwrap().availability,
            AppAvailability::NeedsRelink
        );
    }

    #[test]
    fn deleted_override_does_not_rebind_system_copy() {
        let f = Fixture::new();
        let path = f.ordinary(0, "app.desktop", "");
        f.ordinary(1, "app.desktop", "");
        let binding = binding_for(&path, &f.context).unwrap();
        fs::remove_file(path).unwrap();
        assert_eq!(
            resolve_with(&binding, &f.context).unwrap().availability,
            AppAvailability::NeedsRelink
        );
    }

    #[test]
    fn manual_desktop_keeps_path_only_identity() {
        let f = Fixture::new();
        let path = f.directory.path().join("我的 应用.desktop");
        fs::write(
            &path,
            "[Desktop Entry]\nType=Application\nName=手动应用\nExec=fixture-app\n",
        )
        .unwrap();
        let app = inspect_with(&path, &f.context).unwrap();
        assert!(
            matches!(&app.binding, DesktopBinding::Linux { desktop_id, .. } if desktop_id.is_empty())
        );
        assert_eq!(app.name, "手动应用");
        assert_eq!(app.availability, AppAvailability::Ready);
        fs::write(
            &path,
            "[Desktop Entry]\nType=Application\nName=手动应用 新版\nExec=fixture-app --upgraded\n",
        )
        .unwrap();
        assert_eq!(
            resolve_with(&app.binding, &f.context).unwrap().arguments,
            vec!["--upgraded"]
        );
        fs::remove_file(path).unwrap();
        assert_eq!(
            resolve_with(&app.binding, &f.context).unwrap().availability,
            AppAvailability::NotInstalled
        );
    }

    #[test]
    fn malformed_override_still_masks_valid_system_app() {
        let f = Fixture::new();
        f.ordinary(1, "app.desktop", "");
        f.entry(0, "app.desktop", "broken file");
        let list = collect_with(&f.context).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].availability, AppAvailability::ReadError);
    }

    #[test]
    fn malformed_boolean_is_local_to_one_entry() {
        let f = Fixture::new();
        f.ordinary(0, "good.desktop", "");
        f.ordinary(0, "broken.desktop", "NoDisplay=not-a-boolean\n");
        let list = collect_with(&f.context).unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(
            list.iter()
                .filter(|app| app.availability == AppAvailability::Ready)
                .count(),
            1
        );
        assert_eq!(
            list.iter()
                .filter(|app| app.availability == AppAvailability::ReadError)
                .count(),
            1
        );
    }

    #[test]
    fn current_desktop_precedence_and_list_escaping_are_respected() {
        let f = Fixture::new();
        let entry = parse_entry("[Desktop Entry]\nOnlyShowIn=KDE;\nNotShowIn=GNOME;\n").unwrap();
        let mut context = f.context;
        context.desktops = vec!["GNOME".into(), "KDE".into()];
        assert!(!entry.visible(&context).unwrap());
        context.desktops.reverse();
        assert!(entry.visible(&context).unwrap());
        let escaped = parse_entry("[Desktop Entry]\nOnlyShowIn=Example\\;Desktop;\n").unwrap();
        assert_eq!(escaped.list("OnlyShowIn").unwrap(), vec!["Example;Desktop"]);
    }

    #[test]
    fn locale_uses_spec_order_and_preserves_unicode() {
        let context = Context {
            roots: vec![],
            search_path: vec![],
            locale: "sr_YU.UTF-8@Latn".into(),
            desktops: vec![],
        };
        let entry = parse_entry("[Desktop Entry]\nName=Default\nName[sr]=Language\nName[sr@Latn]=Modifier\nName[sr_YU]=国家 名称\n").unwrap();
        assert_eq!(entry.localized_name(&context).unwrap(), "国家 名称");
        assert_eq!(
            locale_candidates("sr_YU.UTF-8@Latn"),
            vec!["sr_YU@Latn", "sr_YU", "sr@Latn", "sr"]
        );
    }

    #[test]
    fn localized_name_icon_and_location_are_single_literal_arguments() {
        let argv = exec_arguments(
            "fixture-app %c %k %i %U %%",
            "中文 %U 名称",
            "/tmp/My App.desktop",
            "my icon",
        )
        .unwrap();
        assert_eq!(
            argv,
            vec![
                "fixture-app",
                "中文 %U 名称",
                "/tmp/My App.desktop",
                "--icon",
                "my icon",
                "%"
            ]
        );
        assert_eq!(
            exec_arguments("fixture-app %i %d %D %n %N %v %m", "", "", "").unwrap(),
            vec!["fixture-app"]
        );
    }

    #[test]
    fn quoted_paths_and_two_level_escape_preserve_exact_argv() {
        let argv = exec_arguments(
            r#""/opt/My App/bin" "argument with spaces" "" "a\\$b" "a\\\\b""#,
            "",
            "",
            "",
        )
        .unwrap();
        assert_eq!(
            argv,
            vec!["/opt/My App/bin", "argument with spaces", "", "a$b", "a\\b"]
        );
    }

    #[test]
    fn invalid_shell_syntax_and_field_expansions_are_rejected() {
        for value in [
            "app ; touch /tmp/file",
            "app $(touch)",
            "app 'single quotes'",
            "app %Z",
            "app %",
            "app --files=%F",
            "app --icon=%i",
            "app %U %f",
            "app \"%c\"",
            "app \"unclosed",
            "app \"first\"suffix",
            "VAR=value app",
            "app \\q",
            "app \"$VAR\"",
            "%c arg",
        ] {
            assert!(
                exec_arguments(value, "name", "/tmp/file", "icon").is_err(),
                "accepted {value:?}"
            );
        }
    }

    #[test]
    fn argv_metacharacters_are_literals_not_shell_instructions() {
        let argv = exec_arguments("fixture-app \"a; b | c & d\"", "", "", "").unwrap();
        assert_eq!(argv, vec!["fixture-app", "a; b | c & d"]);
    }

    #[test]
    fn unsupported_activations_are_visible_but_not_launchable() {
        let f = Fixture::new();
        for (n, extra) in [
            ("dbus", "DBusActivatable=true\n"),
            ("terminal", "Terminal=true\n"),
            ("flatpak", "X-Flatpak=org.fixture.App\n"),
            ("snap", "X-SnapInstanceName=fixture\n"),
        ] {
            f.ordinary(0, &format!("{n}.desktop"), extra);
        }
        let list = collect_with(&f.context).unwrap();
        assert_eq!(list.len(), 4);
        assert!(list
            .iter()
            .all(|app| app.availability == AppAvailability::UnsupportedLaunch));
        for command in [
            "flatpak run org.fixture.App",
            "env ABC=test fixture-app",
            "bash -c \"exit 0\"",
            "snap run fixture",
            "/snap/bin/fixture",
        ] {
            let entry = parse_entry(&format!(
                "[Desktop Entry]\nType=Application\nName=Fixture\nExec={command}\n"
            ))
            .unwrap();
            let binding =
                binding_for(&f.context.roots[0].join("wrapper.desktop"), &f.context).unwrap();
            assert_eq!(
                build(entry, binding, &f.context).availability,
                AppAvailability::UnsupportedLaunch
            );
        }
    }

    #[test]
    fn tryexec_missing_or_nonexecutable_prevents_launch() {
        let f = Fixture::new();
        let path = f.ordinary(0, "app.desktop", "TryExec=absent-program\n");
        assert_eq!(
            inspect_with(&path, &f.context).unwrap().availability,
            AppAvailability::MissingFile
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(
                f.context.search_path[0].join("fixture-app"),
                fs::Permissions::from_mode(0o644),
            )
            .unwrap();
            f.ordinary(0, "app.desktop", "");
            assert_eq!(
                inspect_with(&path, &f.context).unwrap().availability,
                AppAvailability::MissingFile
            );
        }
    }

    #[test]
    fn working_directory_is_absolute_existing_and_preserved() {
        let f = Fixture::new();
        let cwd = f.directory.path().join("工作 目录");
        fs::create_dir(&cwd).unwrap();
        let path = f.ordinary(0, "app.desktop", &format!("Path={}\n", cwd.display()));
        assert_eq!(
            inspect_with(&path, &f.context)
                .unwrap()
                .working_directory
                .as_deref(),
            cwd.to_str()
        );
        fs::remove_dir(cwd).unwrap();
        assert_eq!(
            inspect_with(&path, &f.context).unwrap().availability,
            AppAvailability::MissingFile
        );
        f.ordinary(0, "app.desktop", "Path=relative\n");
        assert_eq!(
            inspect_with(&path, &f.context).unwrap().availability,
            AppAvailability::ReadError
        );
    }

    #[test]
    fn malformed_groups_duplicates_utf8_and_oversize_are_rejected() {
        for content in [
            "[Desktop Entry]\nName=A\nName=B",
            "[Desktop Entry]\n[Desktop Entry]",
            "[Desktop Entry\nName=A",
            "[Desktop Entry]\nName\0=A",
        ] {
            assert!(parse_entry(content).is_err());
        }
        let f = Fixture::new();
        let path = f.context.roots[0].join("bad.desktop");
        fs::write(&path, [0xff, 0xfe]).unwrap();
        assert!(read_entry(&path).is_err());
        fs::write(&path, vec![b' '; MAX_FILE_BYTES as usize + 1]).unwrap();
        assert!(read_entry(&path).is_err());
    }

    #[test]
    fn scan_limits_are_explicit_instead_of_returning_incomplete_identity() {
        let f = Fixture::new();
        let mut path = f.context.roots[0].clone();
        for _ in 0..MAX_DEPTH + 2 {
            path = path.join("nested");
        }
        fs::create_dir_all(path).unwrap();
        assert!(index(&f.context).is_err());
        let mut context = f.context.clone();
        context.roots = vec![PathBuf::from("/not-an-app-dir"); MAX_ROOTS + 1];
        assert!(index(&context).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_entries_update_in_place_but_directory_cycles_are_not_scanned() {
        use std::os::unix::fs::symlink;
        let f = Fixture::new();
        let target = f.directory.path().join("real.desktop");
        fs::write(
            &target,
            "[Desktop Entry]\nType=Application\nName=Linked\nExec=fixture-app\n",
        )
        .unwrap();
        let link = f.context.roots[0].join("app.desktop");
        symlink(&target, &link).unwrap();
        symlink(&f.context.roots[0], f.context.roots[0].join("cycle")).unwrap();
        let list = collect_with(&f.context).unwrap();
        assert_eq!(list.len(), 1);
        assert!(
            matches!(&list[0].binding, DesktopBinding::Linux { location, .. } if Path::new(location) == link)
        );
        fs::write(
            &target,
            "[Desktop Entry]\nType=Application\nName=Updated\nExec=fixture-app\n",
        )
        .unwrap();
        assert_eq!(
            resolve_with(&list[0].binding, &f.context).unwrap().name,
            "Updated"
        );
    }

    #[test]
    fn binding_validation_rejects_relative_paths_and_wrong_platform() {
        let f = Fixture::new();
        let binding = DesktopBinding::Linux {
            desktop_id: "../bad.desktop".into(),
            location: "/tmp/good.desktop".into(),
        };
        assert!(resolve_with(&binding, &f.context).is_err());
        assert!(inspect_with(Path::new("relative.desktop"), &f.context).is_err());
        assert!(inspect_with(Path::new("/tmp/../bad.desktop"), &f.context).is_err());
        let binding = DesktopBinding::Macos {
            bundle_id: "fixture".into(),
            location: "/Applications/Fixture.app".into(),
            requirement: None,
        };
        assert!(resolve_with(&binding, &f.context).is_err());
    }
}
