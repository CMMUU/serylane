use crate::{
    error::{AppError, AppErrorDto, AppResult},
    models::AppSettings,
    storage::AppStorage,
};
use serde::Serialize;
use tauri::AppHandle;
use tauri_plugin_autostart::ManagerExt;

pub const AUTOSTART_ARG: &str = "--autostart";

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StartupStatus {
    pub launch_requested: bool,
    pub registered: Option<bool>,
    // Explicit OS disable state, not proof that a real login has been tested.
    pub system_allows: Option<bool>,
    pub desired_running: bool,
    pub message: String,
}

fn registration_message(
    requested: bool,
    registered: Option<bool>,
    allowed: Option<bool>,
) -> &'static str {
    match (requested, registered, allowed) {
        (_, None, _) => "系统登录项读取失败，请稍后核对；本次未修改设置。",
        (true, Some(false), _) => {
            "设置已开启，但系统登录项未登记。请先保存“不随系统启动”，再选择需要的启动模式。"
        }
        (false, Some(true), _) => {
            "系统仍存在登录项，与“不随系统启动”的设置不一致，请核对系统登录项。"
        }
        (false, Some(false), _) => "未登记登录项，不随系统启动。",
        (true, Some(true), Some(false)) => {
            "系统已禁用此登录项，请在系统登录项设置中允许；应用不会自行解除禁用。"
        }
        (true, Some(true), Some(true)) => {
            "已登记登录项，未发现系统禁用标记；实际登录运行仍以验收结果为准。"
        }
        (true, Some(true), None) => {
            "已登记登录项，系统允许状态尚未核实；登记成功不等于已完成重启验收。"
        }
    }
}

#[tauri::command]
pub async fn get_startup_status(app: AppHandle) -> Result<StartupStatus, AppErrorDto> {
    tauri::async_runtime::spawn_blocking(move || -> Result<StartupStatus, AppErrorDto> {
        let storage = AppStorage::from_app(&app).map_err(|error| error.dto())?;
        let settings = storage.settings().map_err(|error| error.dto())?;
        let registered = registration_present(&app).ok();
        let system_allows = if registered == Some(true) {
            system_allows_login()
        } else {
            None
        };
        Ok(StartupStatus {
            launch_requested: settings.launch_at_login,
            registered,
            system_allows,
            desired_running: storage
                .state()
                .map_err(|error| error.dto())?
                .desired_running,
            message: registration_message(settings.launch_at_login, registered, system_allows)
                .into(),
        })
    })
    .await
    .map_err(|_| AppError::Runtime("登录项状态检查未完成".into()).dto())?
}

// Only a changed login preference writes to the OS. Appearance, silence and
// restoration changes never re-enable an externally disabled login entry.
pub fn save_with_login_registration(
    current: &AppSettings,
    next: &AppSettings,
    read: impl Fn() -> AppResult<bool>,
    write: impl Fn(bool) -> AppResult<()>,
    rollback: impl FnOnce(bool) -> AppResult<()>,
    persist: impl FnOnce() -> AppResult<()>,
) -> AppResult<()> {
    if next.launch_at_login == current.launch_at_login {
        return persist();
    }
    let previous = read()?;
    let result = (|| {
        write(next.launch_at_login)?;
        if read()? != next.launch_at_login {
            return Err(AppError::Platform(
                "登录项写入后核对不一致，设置未保存".into(),
            ));
        }
        persist()
    })();
    if let Err(error) = result {
        if rollback(previous).is_err() {
            return Err(AppError::Platform(format!(
                "设置保存失败，登录项回退也未完成，请核对系统登录项：{error}"
            )));
        }
        return Err(error);
    }
    Ok(())
}

// auto-launch's Windows is_enabled() combines registration and Task Manager
// approval. Read presence separately so a disabled item is not shown as missing.
fn registration_present(app: &AppHandle) -> AppResult<bool> {
    #[cfg(windows)]
    {
        let _ = app;
        Ok(windows_registration_value(WINDOWS_RUN_KEY)?.is_some())
    }
    #[cfg(not(windows))]
    {
        app.autolaunch()
            .is_enabled()
            .map_err(|error| AppError::Platform(error.to_string()))
    }
}

pub fn save_startup_settings(
    app: &AppHandle,
    storage: &AppStorage,
    current: &AppSettings,
    next: &AppSettings,
) -> AppResult<()> {
    if current.launch_at_login == next.launch_at_login {
        return storage.save_settings(next);
    }
    #[cfg(windows)]
    let snapshot = (
        windows_registration_value(WINDOWS_RUN_KEY)?,
        windows_registration_value(WINDOWS_APPROVAL_KEY)?,
    );
    let autostart = app.autolaunch();
    save_with_login_registration(
        current,
        next,
        || registration_present(app),
        |enabled| {
            (if enabled {
                autostart.enable()
            } else {
                autostart.disable()
            })
            .map_err(|error| AppError::Platform(error.to_string()))
        },
        |previous| {
            #[cfg(windows)]
            {
                let _ = previous;
                // Restore exact values instead of enable(), which resets a
                // previously disabled StartupApproved entry.
                let run = restore_windows_registration(WINDOWS_RUN_KEY, snapshot.0.as_ref());
                let approval =
                    restore_windows_registration(WINDOWS_APPROVAL_KEY, snapshot.1.as_ref());
                run.and(approval).map_err(AppError::from)
            }
            #[cfg(not(windows))]
            {
                (if previous {
                    autostart.enable()
                } else {
                    autostart.disable()
                })
                .map_err(|error| AppError::Platform(error.to_string()))
            }
        },
        || storage.save_settings(next),
    )
}

#[cfg(windows)]
const WINDOWS_RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
#[cfg(windows)]
const WINDOWS_APPROVAL_KEY: &str =
    r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";

#[cfg(windows)]
fn windows_registration_value(path: &str) -> std::io::Result<Option<winreg::RegValue>> {
    use winreg::{
        enums::{HKEY_CURRENT_USER, KEY_READ},
        RegKey,
    };
    let key = match RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(path, KEY_READ) {
        Ok(key) => key,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    match key.get_raw_value("Serylane") {
        Ok(value) => Ok(Some(value)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

#[cfg(windows)]
fn restore_windows_registration(
    path: &str,
    value: Option<&winreg::RegValue>,
) -> std::io::Result<()> {
    use winreg::{
        enums::{HKEY_CURRENT_USER, KEY_SET_VALUE},
        RegKey,
    };
    let root = RegKey::predef(HKEY_CURRENT_USER);
    if let Some(value) = value {
        return root.create_subkey(path)?.0.set_raw_value("Serylane", value);
    }
    let key = match root.open_subkey_with_flags(path, KEY_SET_VALUE) {
        Ok(key) => key,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    match key.delete_value("Serylane") {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

#[cfg(windows)]
fn system_allows_login() -> Option<bool> {
    use winreg::{
        enums::{HKEY_CURRENT_USER, KEY_READ},
        RegKey,
    };
    let key = match RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(
        r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run",
        KEY_READ,
    ) {
        Ok(key) => key,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Some(true),
        Err(_) => return None,
    };
    match key.get_raw_value("Serylane") {
        Ok(value) => match value.bytes.first() {
            Some(2) => Some(true),
            Some(3) => Some(false),
            _ => None,
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Some(true),
        Err(_) => None,
    }
}

#[cfg(any(target_os = "macos", test))]
fn launchd_allows_login(output: &str) -> Option<bool> {
    let output = output.trim();
    if !output.starts_with("disabled services = {") || !output.ends_with('}') {
        return None;
    }
    for line in output.lines() {
        let Some((name, value)) = line.split_once("=>") else {
            continue;
        };
        if name.trim() == "\"Serylane\"" {
            return match value.trim().trim_end_matches(',') {
                "true" => Some(false),
                "false" => Some(true),
                _ => None,
            };
        }
    }
    Some(true)
}

#[cfg(target_os = "macos")]
fn system_allows_login() -> Option<bool> {
    use std::io::Read;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    // Read only launchd's disable flags. Never run load/enable/bootout, persist
    // another application's entries, or include command output in diagnostics.
    let uid = unsafe { libc::getuid() };
    let mut child = Command::new("/bin/launchctl")
        .args(["print-disabled", &format!("gui/{uid}")])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let Some(stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return None;
    };
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout.take(65_537).read_to_end(&mut bytes).map(|_| bytes)
    });
    let deadline = Instant::now() + Duration::from_secs(2);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    let bytes = reader.join().ok()?.ok()?;
    if !status?.success() || bytes.len() > 65_536 {
        return None;
    }
    launchd_allows_login(std::str::from_utf8(&bytes).ok()?)
}

#[cfg(not(any(windows, target_os = "macos")))]
fn system_allows_login() -> Option<bool> {
    None
}

#[cfg(any(windows, test))]
pub fn installer_cleanup_requested(args: &[String]) -> bool {
    args == ["--installer-remove-login"]
}

// The MSI uninstall hook runs this before removing our executable. No Tauri
// window, proxy, user settings or other installation's login entries are touched.
#[cfg(windows)]
pub fn remove_owned_login_entries() -> std::io::Result<()> {
    use winreg::{
        enums::{HKEY_CURRENT_USER, KEY_READ, KEY_SET_VALUE},
        RegKey,
    };
    let key = match RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(
        r"Software\Microsoft\Windows\CurrentVersion\Run",
        KEY_READ | KEY_SET_VALUE,
    ) {
        Ok(key) => key,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    let executable = std::env::current_exe()?;
    for name in ["Serylane", "RouteDeck"] {
        let command: Option<String> = key.get_value(name).ok();
        if command
            .as_deref()
            .is_some_and(|command| owns_legacy_entry(command, &executable))
        {
            match key.delete_value(name) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
    }
    Ok(())
}

pub fn show_initial_window(settings: &AppSettings, args: &[String]) -> bool {
    !(settings.silent_startup && args.iter().any(|arg| arg == AUTOSTART_ARG))
}

pub fn show_existing_window(args: &[String]) -> bool {
    // A duplicate OS login invocation is never a user request to bring the
    // application to the foreground, regardless of the initial-window option.
    !args.iter().any(|arg| arg == AUTOSTART_ARG)
}

// Migrate only our own existing login entry. Never enable a missing/externally
// disabled entry at startup, or delete an entry pointing at a different copy.
#[cfg(windows)]
pub fn migrate_login_entry(_app: &AppHandle, settings: &AppSettings) -> AppResult<()> {
    use winreg::{
        enums::{HKEY_CURRENT_USER, KEY_READ, KEY_SET_VALUE},
        RegKey,
    };
    let key = match RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(
        r"Software\Microsoft\Windows\CurrentVersion\Run",
        KEY_READ | KEY_SET_VALUE,
    ) {
        Ok(key) => key,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    let executable = std::env::current_exe()?;
    let existing: Option<String> = key.get_value("Serylane").ok();
    if existing
        .as_deref()
        .is_some_and(|command| !owns_legacy_entry(command, &executable))
    {
        return Ok(());
    }
    // auto-launch 0.5 wrote unquoted paths. Normalize only an existing owned
    // entry; do not create or enable a missing OS registration here.
    if existing.is_some() {
        key.set_value("Serylane", &quoted_login_command(&executable))?;
    }
    let legacy: Option<String> = key.get_value("RouteDeck").ok();
    if legacy
        .as_deref()
        .is_some_and(|value| owns_legacy_entry(value, &executable))
    {
        if settings.launch_at_login {
            let approvals = RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(
                r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run",
                KEY_READ | KEY_SET_VALUE,
            );
            match approvals {
                Ok(approvals) => {
                    // Keep an existing new-name approval untouched. Otherwise
                    // copy the old disabled/enabled bytes without calling
                    // auto-launch.enable(), which would reset them to enabled.
                    match approvals.get_raw_value("Serylane") {
                        Ok(_) => {}
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                            match approvals.get_raw_value("RouteDeck") {
                                Ok(value) => approvals.set_raw_value("Serylane", &value)?,
                                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                                Err(error) => return Err(error.into()),
                            }
                        }
                        Err(error) => return Err(error.into()),
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
            key.set_value("Serylane", &quoted_login_command(&executable))?;
        }
        key.delete_value("RouteDeck")?;
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn migrate_login_entry(_app: &AppHandle, _settings: &AppSettings) -> AppResult<()> {
    Ok(())
}

#[cfg(any(windows, test))]
fn quoted_login_command(executable: &std::path::Path) -> String {
    format!("\"{}\" {AUTOSTART_ARG}", executable.display())
}

#[cfg(any(windows, test))]
fn owns_legacy_entry(command: &str, executable: &std::path::Path) -> bool {
    let Some(directory) = executable.parent() else {
        return false;
    };
    ["routedeck.exe", "serylane.exe"].iter().any(|name| {
        let path = directory.join(name).display().to_string();
        // Exact serializations used by our current and legacy autostart
        // dependencies, including the old no-argument trailing space. Never
        // accept prefixes, arbitrary arguments or another installation.
        [
            format!("\"{path}\""),
            format!("\"{path}\" {AUTOSTART_ARG}"),
            path.clone(),
            format!("{path} "),
            format!("{path} {AUTOSTART_ARG}"),
        ]
        .iter()
        .any(|expected| command.eq_ignore_ascii_case(expected))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registration_feedback_never_confuses_intent_with_os_state() {
        assert!(registration_message(true, Some(false), None).contains("未登记"));
        assert!(registration_message(true, Some(true), Some(false)).contains("系统已禁用"));
        assert!(registration_message(true, Some(true), None).contains("尚未核实"));
        assert!(registration_message(false, Some(true), Some(true)).contains("不一致"));
        assert!(registration_message(false, Some(false), None).contains("不随系统启动"));
    }

    #[test]
    fn launchd_disable_parser_is_exact_and_unknown_on_invalid_output() {
        assert_eq!(
            launchd_allows_login("disabled services = {\n \"Serylane\" => true\n}"),
            Some(false)
        );
        assert_eq!(
            launchd_allows_login("disabled services = {\n \"Serylane\" => false\n}"),
            Some(true)
        );
        assert_eq!(
            launchd_allows_login("disabled services = {\n \"Serylane-other\" => true\n}"),
            Some(true)
        );
        assert_eq!(
            launchd_allows_login("disabled services = {\n \"Serylane\" => unknown\n}"),
            None
        );
        assert_eq!(launchd_allows_login("permission denied"), None);
    }

    #[test]
    fn silence_and_restore_changes_never_rewrite_os_registration() {
        let current = AppSettings {
            launch_at_login: true,
            ..AppSettings::default()
        };
        let next = AppSettings {
            silent_startup: true,
            restore_last_session: false,
            ..current.clone()
        };
        let saved = std::cell::Cell::new(false);
        save_with_login_registration(
            &current,
            &next,
            || panic!("unchanged login preference must not read/write registration"),
            |_| panic!("never re-enable an externally disabled login entry"),
            |_| panic!("unchanged login preference needs no rollback"),
            || {
                saved.set(true);
                Ok(())
            },
        )
        .unwrap();
        assert!(saved.get());
    }

    #[test]
    fn login_save_verifies_and_rolls_back_after_storage_failure() {
        let current = AppSettings::default();
        let next = AppSettings {
            launch_at_login: true,
            silent_startup: true,
            ..current.clone()
        };
        let registered = std::cell::Cell::new(false);
        let writes = std::cell::RefCell::new(Vec::new());
        let result = save_with_login_registration(
            &current,
            &next,
            || Ok(registered.get()),
            |value| {
                writes.borrow_mut().push(value);
                registered.set(value);
                Ok(())
            },
            |value| {
                writes.borrow_mut().push(value);
                registered.set(value);
                Ok(())
            },
            || Err(AppError::Io("fixture disk failure".into())),
        );
        assert!(result.is_err());
        assert!(!registered.get());
        assert_eq!(*writes.borrow(), [true, false]);
    }

    #[test]
    fn rollback_restores_disabled_override_instead_of_calling_enable_again() {
        let current = AppSettings::default();
        let next = AppSettings {
            launch_at_login: true,
            ..current.clone()
        };
        let registered = std::cell::Cell::new(true);
        let allowed = std::cell::Cell::new(false);
        let result = save_with_login_registration(
            &current,
            &next,
            || Ok(registered.get()),
            |value| {
                registered.set(value);
                allowed.set(true);
                Ok(())
            },
            |previous| {
                registered.set(previous);
                allowed.set(false);
                Ok(())
            },
            || Err(AppError::Io("fixture write failure".into())),
        );
        assert!(result.is_err());
        assert!(registered.get());
        assert!(!allowed.get());
    }

    #[test]
    fn registration_mismatch_does_not_persist_a_successful_setting() {
        let current = AppSettings::default();
        let next = AppSettings {
            launch_at_login: true,
            ..current.clone()
        };
        let saved = std::cell::Cell::new(false);
        let result = save_with_login_registration(
            &current,
            &next,
            || Ok(false),
            |_| Ok(()),
            |_| Ok(()),
            || {
                saved.set(true);
                Ok(())
            },
        );
        assert!(result.is_err());
        assert!(!saved.get());
    }

    #[test]
    fn login_registration_change_preserves_saved_proxy_intent() {
        let directory = tempfile::tempdir().unwrap();
        let storage = AppStorage::from_root(directory.path().join("app")).unwrap();
        let current = storage.settings().unwrap();
        storage.set_desired_running(false).unwrap();
        let next = AppSettings {
            launch_at_login: true,
            silent_startup: true,
            ..current.clone()
        };
        let registered = std::cell::Cell::new(false);
        save_with_login_registration(
            &current,
            &next,
            || Ok(registered.get()),
            |value| {
                registered.set(value);
                Ok(())
            },
            |previous| {
                registered.set(previous);
                Ok(())
            },
            || storage.save_settings(&next),
        )
        .unwrap();
        assert!(!storage.state().unwrap().desired_running);
        assert!(storage.settings().unwrap().launch_at_login);
    }

    #[test]
    fn uninstall_command_is_exact_and_never_matches_normal_launch() {
        assert!(installer_cleanup_requested(&[
            "--installer-remove-login".into()
        ]));
        assert!(!installer_cleanup_requested(&[]));
        assert!(!installer_cleanup_requested(&[AUTOSTART_ARG.into()]));
        assert!(!installer_cleanup_requested(&[
            "--installer-remove-login-other".into()
        ]));
        assert!(!installer_cleanup_requested(&[
            "--installer-remove-login".into(),
            "extra".into()
        ]));
    }
    #[test]
    fn silent_login_does_not_hide_manual_launch() {
        let mut settings = AppSettings::default();
        assert!(!show_initial_window(&settings, &[AUTOSTART_ARG.into()]));
        assert!(show_initial_window(&settings, &[]));
        assert!(show_initial_window(
            &settings,
            &["--autostart-unknown".into()]
        ));
        settings.silent_startup = false;
        assert!(show_initial_window(&settings, &[AUTOSTART_ARG.into()]));
    }
    #[test]
    fn duplicate_login_never_reveals_window_but_explicit_open_does() {
        assert!(!show_existing_window(&[
            "serylane.exe".into(),
            AUTOSTART_ARG.into()
        ]));
        assert!(show_existing_window(&["serylane.exe".into()]));
        assert!(show_existing_window(&[]));
    }
    #[test]
    fn legacy_missing_silent_setting_defaults_to_tray_only_without_replacing_preferences() {
        let original = AppSettings {
            launch_at_login: true,
            ..Default::default()
        };
        let mut old = serde_json::to_value(&original).unwrap();
        old.as_object_mut().unwrap().remove("silentStartup");
        let restored: AppSettings = serde_json::from_value(old.clone()).unwrap();
        assert!(!show_initial_window(&restored, &[AUTOSTART_ARG.into()]));
        assert!(show_initial_window(&restored, &[]));
        assert_eq!(restored.network_mode, original.network_mode);
        assert_eq!(restored.controller_secret, original.controller_secret);
        assert!(restored.launch_at_login);
        let public: crate::models::PublicAppSettings = serde_json::from_value(old.clone()).unwrap();
        assert!(public.silent_startup);
        old["silentStartup"] = serde_json::json!(false);
        let explicit: AppSettings = serde_json::from_value(old).unwrap();
        assert!(show_initial_window(&explicit, &[AUTOSTART_ARG.into()]));
    }
    #[test]
    fn migration_matches_exact_owned_executable_not_prefixes_or_other_copies() {
        let directory = std::path::Path::new("fixture").join("app");
        let current = directory.join("serylane.exe");
        let old = format!("\"{}\"", directory.join("routedeck.exe").display());
        assert!(owns_legacy_entry(&old, &current));
        assert!(owns_legacy_entry(&format!("{old} --autostart"), &current));
        assert!(!owns_legacy_entry(&format!("{old} --unexpected"), &current));
        assert!(!owns_legacy_entry("\"other/routedeck.exe\"", &current));
        let current_command = format!("\"{}\" {AUTOSTART_ARG}", current.display());
        assert!(owns_legacy_entry(&current_command, &current));
        assert!(!owns_legacy_entry(
            &format!("{current_command} extra"),
            &current
        ));
    }
    #[test]
    fn old_unquoted_login_paths_with_spaces_are_normalized_without_extra_arguments() {
        let directory = std::path::Path::new("fixture").join("Program Files");
        let current = directory.join("serylane.exe");
        let old = directory.join("routedeck.exe").display().to_string();
        for command in [&old, &format!("{old} "), &format!("{old} {AUTOSTART_ARG}")] {
            assert!(owns_legacy_entry(command, &current));
        }
        assert_eq!(
            quoted_login_command(&current),
            format!("\"{}\" --autostart", current.display())
        );
        assert!(!owns_legacy_entry(
            &format!("{old}.other --autostart"),
            &current
        ));
        assert!(!owns_legacy_entry(
            &format!("{old} --autostart extra"),
            &current
        ));
        assert!(!owns_legacy_entry(&format!("{old} --other"), &current));
    }
}
