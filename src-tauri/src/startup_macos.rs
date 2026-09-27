//! Native, bundle-relative login registration. Legacy entries are migrated only
//! when identity, saved intent and the OS allow-state all agree.
use crate::{
    error::{AppError, AppResult},
    macos_service::{self, Service},
    models::AppSettings,
};
use objc2_foundation::{
    NSArray, NSData, NSDictionary, NSPropertyListMutabilityOptions, NSPropertyListSerialization,
    NSString,
};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::Mutex,
};

static SERVICE: Service = Service::new(c"com.cmmuu.mihomodesktop.login.plist", true);
static MUTATION: Mutex<()> = Mutex::new(());

fn bundle() -> AppResult<PathBuf> {
    let executable = std::env::current_exe()?;
    executable
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .filter(|p| p.extension().is_some_and(|ext| ext == "app"))
        .map(Path::to_path_buf)
        .ok_or_else(|| AppError::Platform("请使用已安装的 Serylane.app 配置登录启动".into()))
}

fn legacy_path() -> AppResult<PathBuf> {
    let home =
        std::env::var_os("HOME").ok_or_else(|| AppError::Platform("用户目录尚未就绪".into()))?;
    Ok(PathBuf::from(home).join("Library/LaunchAgents/Serylane.plist"))
}

fn legacy_owned() -> AppResult<bool> {
    let path = legacy_path()?;
    let metadata = match fs::symlink_metadata(&path) {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 65_536 {
        return Ok(false);
    }
    let mut bytes = Vec::new();
    fs::File::open(path)?.take(65_537).read_to_end(&mut bytes)?;
    if bytes.len() > 65_536 {
        return Ok(false);
    }
    let value = unsafe {
        NSPropertyListSerialization::propertyListWithData_options_format_error(
            &NSData::with_bytes(&bytes),
            NSPropertyListMutabilityOptions::Immutable,
            std::ptr::null_mut(),
        )
    }
    .map_err(|_| AppError::Platform("旧登录项格式异常，请在系统设置中核对；未修改该文件".into()))?;
    let Some(dict) = value.downcast_ref::<NSDictionary>() else {
        return Ok(false);
    };
    let Some(label) = dict.objectForKey(&NSString::from_str("Label")) else {
        return Ok(false);
    };
    let Some(args) = dict.objectForKey(&NSString::from_str("ProgramArguments")) else {
        return Ok(false);
    };
    let Some(args) = args.downcast_ref::<NSArray>() else {
        return Ok(false);
    };
    let args: Option<Vec<String>> = args
        .iter()
        .map(|value| value.downcast_ref::<NSString>().map(ToString::to_string))
        .collect();
    let Some(args) = args else {
        return Ok(false);
    };
    let label = label.downcast_ref::<NSString>().map(ToString::to_string);
    Ok(owns_legacy(label.as_deref(), &args, &bundle()?))
}

fn owns_legacy(label: Option<&str>, args: &[String], bundle: &Path) -> bool {
    let expected = bundle
        .join("Contents/MacOS/serylane")
        .to_string_lossy()
        .into_owned();
    label == Some("Serylane")
        && (args == [expected.clone()] || args == [expected, "--autostart".into()])
}

fn validate_layout() -> AppResult<()> {
    let root = bundle()?;
    for relative in [
        "Contents/MacOS/serylane-login-helper",
        "Contents/Library/LaunchAgents/com.cmmuu.mihomodesktop.login.plist",
    ] {
        if !root.join(relative).is_file() {
            return Err(AppError::Platform(
                "此应用包缺少登录启动组件，请安装完整的新版 Serylane".into(),
            ));
        }
    }
    Ok(())
}

pub fn native_enabled() -> AppResult<bool> {
    SERVICE.ensure_settled().map_err(AppError::Platform)?;
    Ok(SERVICE.status().map_err(AppError::Platform)? == macos_service::ENABLED)
}

pub fn detach_for_upgrade() -> AppResult<()> {
    let _guard = MUTATION
        .try_lock()
        .map_err(|_| AppError::Conflict("登录项正在更新，请稍后重试".into()))?;
    SERVICE.unregister().map_err(AppError::Platform)
}

pub fn restore_after_upgrade() -> AppResult<()> {
    if SERVICE.status().map_err(AppError::Platform)? == macos_service::REQUIRES_APPROVAL {
        return Err(AppError::Platform(
            "请在系统登录项中允许 Serylane；应用保持系统的禁用选择".into(),
        ));
    }
    write(true)?;
    if !native_enabled()? {
        return Err(AppError::Platform(
            "登录项仍待系统允许，请在系统设置中核对".into(),
        ));
    }
    Ok(())
}

pub fn registration_present() -> AppResult<bool> {
    SERVICE.ensure_settled().map_err(AppError::Platform)?;
    let raw = SERVICE.status().map_err(AppError::Platform)?;
    if matches!(
        raw,
        macos_service::ENABLED | macos_service::REQUIRES_APPROVAL
    ) {
        return Ok(true);
    }
    legacy_owned()
}

pub fn system_allows() -> Option<bool> {
    if SERVICE.is_unregistering() {
        return None;
    }
    match SERVICE.status().ok()? {
        macos_service::ENABLED => Some(true),
        macos_service::REQUIRES_APPROVAL => Some(false),
        _ if legacy_owned().ok()? => super::startup::legacy_system_allows_login(),
        _ => None,
    }
}

pub fn needs_migration() -> bool {
    legacy_owned().unwrap_or(false)
}

pub fn write(enabled: bool) -> AppResult<()> {
    let _guard = MUTATION
        .try_lock()
        .map_err(|_| AppError::Conflict("登录项正在更新，请稍后核对状态".into()))?;
    if enabled {
        SERVICE.ensure_settled().map_err(AppError::Platform)?;
        validate_layout()?;
        let raw = SERVICE.status().map_err(AppError::Platform)?;
        // OS approval/disable is distinct from registration. Never overwrite it.
        if !matches!(
            raw,
            macos_service::ENABLED | macos_service::REQUIRES_APPROVAL
        ) {
            if legacy_owned()? && super::startup::legacy_system_allows_login() != Some(true) {
                return Err(AppError::Platform(
                    "请先在系统登录项中允许原 Serylane 登录项，再迁移关联".into(),
                ));
            }
            SERVICE.register().map_err(AppError::Platform)?;
        }
        remove_legacy_after_registration()?;
    } else {
        SERVICE.unregister().map_err(AppError::Platform)?;
        if legacy_owned()? {
            fs::remove_file(legacy_path()?)?;
        }
    }
    Ok(())
}

fn remove_legacy_after_registration() -> AppResult<()> {
    // Retain an owned previous entry if approval has not completed; never delete
    // a disabled entry or one belonging to a different copy of the app.
    if SERVICE.status().map_err(AppError::Platform)? == macos_service::ENABLED
        && legacy_owned()?
        && super::startup::legacy_system_allows_login() == Some(true)
    {
        fs::remove_file(legacy_path()?)?;
    }
    Ok(())
}

pub fn repair() -> AppResult<()> {
    let _guard = MUTATION
        .try_lock()
        .map_err(|_| AppError::Conflict("登录项正在更新，请稍后核对状态".into()))?;
    validate_layout()?;
    if system_allows() == Some(false) {
        return Err(AppError::Platform(
            "系统已禁用登录启动，请先在系统登录项中允许；应用保持该选择".into(),
        ));
    }
    SERVICE.unregister().map_err(AppError::Platform)?;
    // Work continues on a worker after the completion callback, not inside it.
    SERVICE.register().map_err(AppError::Platform)?;
    remove_legacy_after_registration()
}

pub fn migrate(settings: &AppSettings) -> AppResult<()> {
    if !settings.launch_at_login
        || !legacy_owned()?
        || super::startup::legacy_system_allows_login() != Some(true)
    {
        return Ok(());
    }
    if SERVICE.status().map_err(AppError::Platform)? == macos_service::REQUIRES_APPROVAL {
        return Ok(());
    }
    write(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn migration_identity_is_exact_and_rejects_other_installs_and_extra_arguments() {
        let app = Path::new("/Applications/Serylane.app");
        let executable = app
            .join("Contents/MacOS/serylane")
            .to_string_lossy()
            .into_owned();
        assert!(owns_legacy(
            Some("Serylane"),
            &[executable.clone(), "--autostart".into()],
            app
        ));
        assert!(owns_legacy(
            Some("Serylane"),
            std::slice::from_ref(&executable),
            app
        ));
        assert!(!owns_legacy(
            Some("Other"),
            std::slice::from_ref(&executable),
            app
        ));
        assert!(!owns_legacy(
            Some("Serylane"),
            &[executable, "--other".into()],
            app
        ));
        assert!(!owns_legacy(
            Some("Serylane"),
            &["/other/Serylane.app/Contents/MacOS/serylane".into()],
            app
        ));
    }
}
