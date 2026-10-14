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
        Self::prepare_at(&root, env!("CARGO_PKG_VERSION"))
    }
    pub(crate) fn prepare_at(root: &Path, version: &str) -> AppResult<Self> {
        crate::tun_service::codesign::validate_bundle(root, version).map_err(AppError::Update)?;
        let root = root.to_path_buf();
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
        crate::tun_service::codesign::validate_bundle(
            &directory.path().join("previous.app"),
            version,
        )
        .map_err(AppError::Update)?;
        Ok(Self { root, directory })
    }
    pub fn path(&self) -> &Path {
        self.directory.path()
    }

    // Arm durable recovery before stopping processes, not just before replacement.
    pub fn retain(self, target: &str) -> AppResult<RetainedBackup> {
        let marker = BackupMarker {
            root: self.root.canonicalize()?,
            target: target.into(),
        };
        let bytes = serde_json::to_vec(&marker).map_err(|e| error(e.to_string()))?;
        let path = self.directory.path().join("backup.json");
        crate::storage::write_private_atomic(&path, &bytes)?;
        fs::File::open(self.directory.path())?.sync_all()?;
        Ok(RetainedBackup {
            root: self.root,
            directory: self.directory.keep(),
            target: target.into(),
        })
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct BackupMarker {
    root: PathBuf,
    target: String,
}

pub struct RetainedBackup {
    root: PathBuf,
    directory: PathBuf,
    target: String,
}
impl RetainedBackup {
    pub fn install(self, install: impl FnOnce() -> AppResult<()>) -> AppResult<()> {
        let result = transaction(&self.root, &self.directory, install, || {
            crate::tun_service::codesign::validate_bundle(&self.root, &self.target)
                .map_err(AppError::Update)
        });
        // Kept on success until the new process confirms its bundle/version.
        // Kept on recovery failure (and process crash); never lose the only copy.
        result.map_err(|cause| {
            error(format!(
                "{cause}；恢复资料保留在 {}",
                self.directory.display()
            ))
        })
    }
}

/// Only remove our exact private transaction after validating the running bundle.
/// An edited/linked/mismatched receipt leaves everything intact for diagnosis.
pub fn confirm_backup(directory: &Path, root: &Path, target: &str) -> AppResult<()> {
    if !directory.exists() {
        return Ok(());
    }
    let parent = root
        .parent()
        .ok_or_else(|| error("应用父目录缺失"))?
        .canonicalize()?;
    let m = fs::symlink_metadata(directory)?;
    if !m.is_dir()
        || m.file_type().is_symlink()
        || directory
            .parent()
            .map(Path::canonicalize)
            .transpose()?
            .as_ref()
            != Some(&parent)
        || !directory
            .file_name()
            .and_then(|s| s.to_str())
            .is_some_and(|s| s.starts_with(".serylane-update-"))
    {
        return Err(error("更新备份位置异常，已保留供诊断"));
    }
    let marker_path = directory.join("backup.json");
    let m = fs::symlink_metadata(&marker_path)?;
    if !m.is_file() || m.file_type().is_symlink() || m.len() > 4096 {
        return Err(error("更新备份标识异常，已保留"));
    }
    let marker: BackupMarker = serde_json::from_slice(&fs::read(marker_path)?)
        .map_err(|_| error("更新备份标识读取失败"))?;
    if marker.root != root.canonicalize()? || marker.target != target {
        return Err(error("更新备份身份不匹配，已保留"));
    }
    crate::tun_service::codesign::validate_bundle(root, target).map_err(AppError::Update)?;
    fs::remove_dir_all(directory)?;
    fs::File::open(parent)?.sync_all()?;
    Ok(())
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
        let recovery = (|| -> AppResult<()> {
            if fs::symlink_metadata(root).is_ok() {
                fs::rename(root, directory.join("failed.app"))?;
            }
            fs::rename(directory.join("previous.app"), root)?;
            fs::File::open(root.parent().ok_or_else(|| error("应用父目录缺失"))?)?.sync_all()?;
            Ok(())
        })();
        if let Err(recovery) = recovery {
            return Err(error(format!(
                "{cause}；自动回退未完成：{recovery}；请保留备份并使用完整安装包恢复"
            )));
        }
        return Err(error(format!(
            "{cause}；已回退旧应用，请重新打开后核对状态"
        )));
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    fn run(command: &mut Command) -> std::process::Output {
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{:?}: {}",
            command,
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }
    fn fixture(parent: &Path, name: &str, version: &str) -> PathBuf {
        let root = parent.join(name);
        fs::create_dir_all(root.join("Contents/MacOS")).unwrap();
        fs::create_dir_all(root.join("Contents/Resources")).unwrap();
        fs::write(
            root.join("Contents/Resources/sealed.txt"),
            "sealed resource",
        )
        .unwrap();
        fs::write(root.join("Contents/Info.plist"), format!(r#"<?xml version="1.0"?><plist version="1.0"><dict><key>CFBundleIdentifier</key><string>com.cmmuu.mihomodesktop</string><key>CFBundleExecutable</key><string>serylane</string><key>CFBundlePackageType</key><string>APPL</string><key>CFBundleShortVersionString</key><string>{version}</string></dict></plist>"#)).unwrap();
        let source = parent.join(format!("{name}.c"));
        fs::write(
            &source,
            format!("#include <stdio.h>\nint main(void) {{ puts(\"{version}\"); return 0; }}"),
        )
        .unwrap();
        run(Command::new("/usr/bin/clang")
            .arg(&source)
            .arg("-o")
            .arg(root.join("Contents/MacOS/serylane")));
        run(Command::new("/usr/bin/codesign")
            .args(["--force", "--sign", "-"])
            .arg(&root));
        root
    }
    #[test]
    fn macos_native_bundle_integrity_and_backup_failures() {
        use crate::tun_service::codesign::{validate, validate_bundle};
        let d = tempfile::tempdir().unwrap();
        let d = d.path().canonicalize().unwrap();
        let root = fixture(&d, "Serylane.app", "9.0.0");
        validate_bundle(&root, "9.0.0").unwrap();
        assert!(
            validate(&root).is_err(),
            "IPC executable trust must stay file-only"
        );
        assert!(validate_bundle(&root, "9.0.1").is_err());
        let link = d.join("Linked.app");
        std::os::unix::fs::symlink(&root, &link).unwrap();
        assert!(Backup::prepare_at(&link, "9.0.0").is_err());
        for failure in ["tampered", "missing", "panic", "wrong-version"] {
            let backup = Backup::prepare_at(&root, "9.0.0").unwrap();
            let retained = backup.path().to_path_buf();
            let backup = backup.retain("9.0.1").unwrap();
            let result = backup.install(|| {
                match failure {
                    "tampered" => {
                        fs::write(root.join("Contents/Resources/sealed.txt"), "tampered")?
                    }
                    "missing" => fs::remove_dir_all(&root)?,
                    "panic" => panic!("injected native replacement panic"),
                    _ => (),
                }
                Ok(())
            });
            assert!(result.is_err());
            validate_bundle(&root, "9.0.0").unwrap();
            assert!(retained.exists());
            assert!(confirm_backup(&retained, &root, "9.0.1").is_err());
        }
        fs::write(root.join("Contents/Resources/sealed.txt"), "tampered").unwrap();
        assert!(validate_bundle(&root, "9.0.0").is_err());
        assert!(Backup::prepare_at(&root, "9.0.0").is_err());
    }
    #[test]
    fn macos_native_unconfirmed_backup_survives_and_cleanup_is_scoped() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().canonicalize().unwrap();
        let root = fixture(&dir, "Serylane.app", "9.0.0");
        let backup = Backup::prepare_at(&root, "9.0.0").unwrap();
        let kept = backup.path().to_path_buf();
        drop(backup.retain("9.0.1").unwrap()); // process/command can end before replacement
        assert!(kept.join("previous.app").exists());
        assert!(confirm_backup(&kept, &root, "9.0.0").is_err());
        let linked = dir.join(".serylane-update-linked");
        std::os::unix::fs::symlink(&kept, &linked).unwrap();
        assert!(confirm_backup(&linked, &root, "9.0.1").is_err());
        assert!(kept.join("previous.app").exists());
        fs::remove_file(kept.join("backup.json")).unwrap();
        assert!(confirm_backup(&kept, &root, "9.0.1").is_err());
        assert!(kept.join("previous.app").exists());
    }
    #[test]
    fn failed_rollback_preserves_original_cause_and_only_surviving_copy() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("app");
        let backup = d.path().join("backup");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(backup.join("previous.app")).unwrap();
        fs::create_dir_all(backup.join("failed.app")).unwrap();
        fs::write(backup.join("failed.app/collision"), "keep").unwrap();
        fs::write(backup.join("previous.app/version"), "old").unwrap();
        let result = transaction(
            &root,
            &backup,
            || Err(error("original install failure")),
            || Ok(()),
        );
        let message = result.unwrap_err().to_string();
        assert!(message.contains("original install failure"));
        assert!(message.contains("自动回退未完成"));
        assert_eq!(
            fs::read_to_string(backup.join("previous.app/version")).unwrap(),
            "old"
        );
    }
    #[tokio::test]
    async fn macos_native_signed_download_replace_and_new_process_confirmation() {
        use std::io::{Read, Write};
        use tauri_plugin_updater::UpdaterExt;
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().canonicalize().unwrap();
        let root = fixture(&dir, "Installed.app", "9.0.0");
        let next = fixture(&dir, "Next.app", "9.0.1");
        let archive = dir.join("fixture.app.tar.gz");
        run(Command::new("/usr/bin/tar")
            .env("COPYFILE_DISABLE", "1")
            .arg("-czf")
            .arg(&archive)
            .arg("-C")
            .arg(&dir)
            .arg("Next.app"));
        let bytes = fs::read(&archive).unwrap();
        let signed = run(Command::new("node")
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../scripts/sign-update-fixture.mjs"))
            .arg(&archive));
        let signed: serde_json::Value = serde_json::from_slice(&signed.stdout).unwrap();
        let server = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        server.set_nonblocking(true).unwrap();
        let address = server.local_addr().unwrap();
        let manifest = serde_json::to_vec(&serde_json::json!({"version":"9.0.1", "url":format!("http://{address}/archive"), "signature":signed["signature"]})).unwrap();
        let server_thread = std::thread::spawn(move || {
            let until = Instant::now() + Duration::from_secs(30);
            let mut count = 0;
            while count < 2 && Instant::now() < until {
                let (mut stream, _) = match server.accept() {
                    Ok(s) => s,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(10));
                        continue;
                    }
                    Err(e) => panic!("{e}"),
                };
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut request = [0; 4096];
                let n = stream.read(&mut request).unwrap();
                let body = if String::from_utf8_lossy(&request[..n]).starts_with("GET /manifest ") {
                    &manifest
                } else {
                    &bytes
                };
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .unwrap();
                stream.write_all(body).unwrap();
                count += 1;
            }
            assert_eq!(count, 2);
        });
        let mut context = tauri::test::mock_context(tauri::test::noop_assets());
        context.config_mut().plugins.0.insert(
            "updater".into(),
            serde_json::json!({
                "pubkey": signed["pubkey"], "dangerousInsecureTransportProtocol": true
            }),
        ); // Only the ephemeral loopback test server; never production config.
        let app = tauri::test::mock_builder()
            .plugin(
                tauri_plugin_updater::Builder::new()
                    .pubkey(signed["pubkey"].as_str().unwrap())
                    .build(),
            )
            .build(context)
            .unwrap();
        let update = app
            .updater_builder()
            .endpoints(vec![format!("http://{address}/manifest").parse().unwrap()])
            .unwrap()
            .executable_path(root.join("Contents/MacOS/serylane"))
            .no_proxy()
            .timeout(Duration::from_secs(15))
            .build()
            .unwrap()
            .check()
            .await
            .unwrap()
            .unwrap();
        let downloaded = update.download(|_, _| {}, || {}).await.unwrap();
        server_thread.join().unwrap();
        let backup = Backup::prepare_at(&root, "9.0.0").unwrap();
        let retained = backup.path().to_path_buf();
        backup
            .retain("9.0.1")
            .unwrap()
            .install(|| update.install(downloaded).map_err(|e| error(e.to_string())))
            .unwrap();
        assert!(
            retained.join("previous.app").exists(),
            "retain until new process is verified"
        );
        let output = run(&mut Command::new(root.join("Contents/MacOS/serylane")));
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "9.0.1");
        confirm_backup(&retained, &root, "9.0.1").unwrap();
        assert!(!retained.exists());
        assert!(next.exists());
    }
    #[test]
    #[ignore = "requires built release bundle; enforced by release workflow"]
    fn macos_native_release_artifact_uses_product_backup_validator() {
        let path = std::env::var("SERYLANE_UPDATE_BUNDLE").expect("release bundle path required");
        let original = PathBuf::from(path);
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("Serylane.app");
        run(Command::new("/usr/bin/ditto").arg(&original).arg(&root));
        let backup = Backup::prepare_at(&root, env!("CARGO_PKG_VERSION")).unwrap();
        let retained = backup.path().to_path_buf();
        backup
            .retain(env!("CARGO_PKG_VERSION"))
            .unwrap()
            .install(|| {
                fs::remove_dir_all(&root)?;
                run(Command::new("/usr/bin/ditto").arg(&original).arg(&root));
                Ok(())
            })
            .unwrap();
        confirm_backup(&retained, &root, env!("CARGO_PKG_VERSION")).unwrap();
        assert!(!retained.exists());
    }
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
