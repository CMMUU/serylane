//! Preserve the installed bundle independently of the updater's temporary
//! directory. Never remove the only surviving copy on replacement failure.
use crate::error::{AppError, AppResult};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

pub struct Backup {
    root: PathBuf,
    directory: tempfile::TempDir,
}
impl Backup {
    pub fn prepare() -> AppResult<Self> {
        let root = crate::tun_service::codesign::bundle_root().map_err(AppError::Update)?;
        if fs::symlink_metadata(&root)?.file_type().is_symlink() {
            return Err(error("应用位置是链接，请使用完整安装包升级"));
        }
        let parent = root.parent().ok_or_else(|| error("应用父目录缺失"))?;
        // Same filesystem, also checks replacement permissions before stopping
        // the proxy. Managed/read-only installs use their original installer.
        let directory = tempfile::Builder::new()
            .prefix(".serylane-update-")
            .tempdir_in(parent)
            .map_err(|_| error("应用目录不可写，请使用完整安装包或原管理工具升级；尚未停止代理"))?;
        let mut child = Command::new("/usr/bin/ditto")
            .arg(&root)
            .arg(directory.path().join("previous.app"))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        let deadline = Instant::now() + Duration::from_secs(90);
        loop {
            let status = match child.try_wait() {
                Ok(status) => status,
                Err(error) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(error.into());
                }
            };
            if let Some(status) = status {
                if !status.success() {
                    return Err(error("旧应用备份未完成，尚未停止代理"));
                }
                break;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error("旧应用备份超时，尚未停止代理"));
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        crate::tun_service::codesign::validate(&directory.path().join("previous.app"))
            .map_err(AppError::Update)?;
        Ok(Self { root, directory })
    }
    pub fn install(self, install: impl FnOnce() -> AppResult<()>) -> AppResult<()> {
        let Self { root, directory } = self;
        let result = transaction(&root, directory.path(), install, || {
            crate::tun_service::codesign::validate(&root).map_err(AppError::Update)
        });
        if result.as_ref().is_err_and(|_| !root.exists())
            || directory.path().join("previous.app").exists() && result.is_err()
        {
            let kept = directory.keep();
            return Err(error(format!(
                "安装及自动回退尚未完成；旧应用保留在 {}，请使用完整安装包恢复。",
                kept.display()
            )));
        }
        result
    }
}
fn error(message: impl Into<String>) -> AppError {
    AppError::Update(message.into())
}
fn transaction(
    root: &Path,
    directory: &Path,
    install: impl FnOnce() -> AppResult<()>,
    verify: impl FnOnce() -> AppResult<()>,
) -> AppResult<()> {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        install()?;
        verify()
    }))
    .unwrap_or_else(|_| Err(error("安装过程意外中断")));
    if let Err(cause) = result {
        // Quarantine the failed replacement in our private transaction folder.
        if fs::symlink_metadata(root).is_ok() {
            fs::rename(root, directory.join("failed.app"))?;
        }
        fs::rename(directory.join("previous.app"), root)?;
        fs::File::open(root.parent().ok_or_else(|| error("应用父目录缺失"))?)?.sync_all()?;
        return Err(error(format!(
            "{cause}；已回退旧应用，请重新打开后核对状态"
        )));
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn failed_replacement_validation_and_panic_restore_old_bundle() {
        for kind in ["deleted", "invalid", "panic"] {
            let d = tempfile::tempdir().unwrap();
            let root = d.path().join("app");
            let backup = d.path().join("backup");
            fs::create_dir_all(&root).unwrap();
            fs::create_dir_all(backup.join("previous.app")).unwrap();
            fs::write(backup.join("previous.app/version"), "old").unwrap();
            assert!(transaction(
                &root,
                &backup,
                || {
                    fs::remove_dir_all(&root)?;
                    if kind == "panic" {
                        panic!("fixture");
                    }
                    if kind == "deleted" {
                        return Err(error("fixture"));
                    }
                    fs::create_dir_all(&root)?;
                    fs::write(root.join("version"), "new")?;
                    Ok(())
                },
                || Err(error("invalid bundle"))
            )
            .is_err());
            assert_eq!(fs::read_to_string(root.join("version")).unwrap(), "old");
        }
    }
}
