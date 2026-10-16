//! Native install acceptance runs the product coordinator, not Update::install alone.
use super::*;
use super::{
    contract_tests::Fixture,
    transaction::{run_install, InstallHost},
};
use std::{
    cell::RefCell,
    fs,
    path::{Path, PathBuf},
    process::Command,
};
fn command(c: &mut Command) -> std::process::Output {
    let o = c.output().unwrap();
    assert!(
        o.status.success(),
        "{c:?}: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    o
}
fn synthetic(parent: &Path, name: &str, version: &str) -> PathBuf {
    let root = parent.join(name);
    let exe = executable(&root);
    fs::create_dir_all(exe.parent().unwrap()).unwrap();
    let code = parent.join(format!("{name}.c"));
    fs::write(
        &code,
        format!("#include <stdio.h>\nint main(void) {{ puts(\"{version}\"); return 0; }}"),
    )
    .unwrap();
    command(Command::new("cc").arg(&code).arg("-o").arg(&exe));
    #[cfg(target_os = "macos")]
    {
        fs::create_dir_all(root.join("Contents/Resources")).unwrap();
        fs::write(root.join("Contents/Resources/sealed.txt"), "sealed").unwrap();
        fs::write(root.join("Contents/Info.plist"),format!(r#"<?xml version="1.0"?><plist version="1.0"><dict><key>CFBundleIdentifier</key><string>com.cmmuu.mihomodesktop</string><key>CFBundleExecutable</key><string>serylane</string><key>CFBundlePackageType</key><string>APPL</string><key>CFBundleShortVersionString</key><string>{version}</string></dict></plist>"#)).unwrap();
        command(
            Command::new("codesign")
                .args(["--force", "--sign", "-", "--timestamp=none"])
                .arg(&root),
        );
    }
    root
}
fn executable(root: &Path) -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        root.join("Contents/MacOS/serylane")
    }
    #[cfg(target_os = "linux")]
    {
        root.into()
    }
}
fn package(root: &Path) -> Vec<u8> {
    #[cfg(target_os = "macos")]
    {
        let archive = root.with_extension("app.tar.gz");
        command(
            Command::new("tar")
                .env("COPYFILE_DISABLE", "1")
                .arg("-czf")
                .arg(&archive)
                .arg("-C")
                .arg(root.parent().unwrap())
                .arg(root.file_name().unwrap()),
        );
        fs::read(archive).unwrap()
    }
    #[cfg(target_os = "linux")]
    {
        fs::read(root).unwrap()
    }
}
struct NativeFixtureHost {
    root: PathBuf,
    receipt: PathBuf,
    current: String,
    target: String,
    probe_json: bool,
    fail_restart: bool,
    calls: RefCell<Vec<String>>,
}
impl InstallHost for NativeFixtureHost {
    #[cfg(target_os = "macos")]
    type Prepared = crate::macos_update_install::RetainedBackup;
    #[cfg(target_os = "linux")]
    type Prepared = ();
    fn stage(&self, target: &str, stage: &str) -> AppResult<()> {
        self.calls.borrow_mut().push(stage.into());
        crate::update_receipt::write_at(&self.receipt, target, stage)
    }
    fn preflight(&self, _: &Candidate) -> AppResult<Self::Prepared> {
        #[cfg(target_os = "macos")]
        {
            let backup =
                crate::macos_update_install::Backup::prepare_at(&self.root, &self.current)?;
            crate::update_receipt::set_backup_at(&self.receipt, backup.path())?;
            backup.retain(&self.target)
        }
        #[cfg(target_os = "linux")]
        {
            let _ = &self.current;
            assert!(self.root.is_file());
            Ok(())
        }
    }
    fn stop(&self) -> AppResult<()> {
        self.calls.borrow_mut().push("stop isolated runtime".into());
        Ok(())
    }
    fn prepare_services(&self, _: &str) -> AppResult<()> {
        Ok(())
    }
    fn replace(&self, c: &Candidate, bytes: Vec<u8>, _prepared: Self::Prepared) -> AppResult<()> {
        let replace = || c.update.install(bytes).map_err(|e| failure(e.to_string()));
        #[cfg(target_os = "macos")]
        {
            _prepared.install(replace)
        }
        #[cfg(target_os = "linux")]
        {
            replace()
        }
    }
    fn rollback_services(&self) -> AppResult<()> {
        self.calls.borrow_mut().push("rollback services".into());
        Ok(())
    }
    fn restart(&self) -> AppResult<()> {
        if self.fail_restart {
            return Err(failure("fixture restart failure"));
        }
        let mut process = Command::new(executable(&self.root));
        if self.probe_json {
            #[cfg(target_os = "linux")]
            {
                process.arg("--appimage-extract-and-run");
            }
            process.arg("--serylane-update-probe");
        }
        let result = command(&mut process);
        let output = String::from_utf8_lossy(&result.stdout);
        let running = if self.probe_json {
            let value: serde_json::Value = serde_json::from_str(output.trim()).unwrap();
            assert_eq!(value["identifier"], "com.cmmuu.mihomodesktop");
            assert_eq!(value["arch"], std::env::consts::ARCH);
            value["version"].as_str().unwrap().to_owned()
        } else {
            output.trim().to_owned()
        };
        assert_eq!(running, self.target);
        assert!(crate::update_receipt::confirm_at(
            &self.receipt,
            &running,
            |_receipt| {
                #[cfg(target_os = "macos")]
                {
                    crate::tun_service::codesign::validate_bundle(&self.root, &running)
                        .map_err(AppError::Update)?;
                    if let Some(backup) = &_receipt.backup {
                        crate::macos_update_install::confirm_backup(
                            Path::new(backup),
                            &self.root,
                            &running,
                        )?;
                    }
                }
                Ok(())
            }
        )?);
        Ok(())
    }
    fn failed(&self, target: &str, stage: &str, reason: &str, disrupted: bool) -> AppResult<()> {
        crate::update_receipt::failed_at(&self.receipt, target, stage, reason, disrupted)
    }
}
fn execute(host: &NativeFixtureHost, bytes: Vec<u8>) -> AppResult<()> {
    let f = Fixture::new(bytes, &host.target, &executable(&host.root), None);
    let manager = AppUpdateManager {
        session: Arc::new(contract_tests::ready_session(&f.candidate, Vec::new())),
        ..Default::default()
    };
    {
        let mut session = manager.lock()?;
        session.ready = None;
        session.status.phase = "downloading".into();
    }
    tauri::async_runtime::block_on(transport::recover(
        &manager,
        vec![f.candidate.clone()],
        false,
        transport::Policy::default(),
        |_| async { Err(failure("unexpected fixture discovery")) },
        |_| f.transport.clone(),
    ))?;
    let selected = select_install(&mut *manager.lock()?, &format!("v{}", host.target))?;
    run_install(
        &manager.session,
        &selected,
        &host.current,
        &f.platform,
        host,
    )
}
#[test]
fn updater_native_product_chain_signed_download_replacement_confirmation_and_next_upgrade() {
    let d = tempfile::tempdir().unwrap();
    let dir = d.path().canonicalize().unwrap();
    let root = synthetic(&dir, "Installed.app", "9.0.0");
    let sentinel = dir.join("user-settings.json");
    fs::write(&sentinel, b"synthetic subscriptions and selection").unwrap();
    for (current, next) in [("9.0.0", "9.0.1"), ("9.0.1", "9.0.2")] {
        let new = synthetic(&dir, "Next.app", next);
        let host = NativeFixtureHost {
            root: root.clone(),
            receipt: dir.join("receipt.json"),
            current: current.into(),
            target: next.into(),
            probe_json: false,
            fail_restart: false,
            calls: RefCell::default(),
        };
        execute(&host, package(&new)).unwrap();
        let journal: serde_json::Value =
            serde_json::from_slice(&fs::read(&host.receipt).unwrap()).unwrap();
        assert_eq!(journal["stage"], "confirmed");
        assert!(journal.get("backup").is_none());
        assert_eq!(
            fs::read(&sentinel).unwrap(),
            b"synthetic subscriptions and selection"
        );
        assert_eq!(host.calls.borrow().last().unwrap(), "restarting");
    }
}
#[cfg(target_os = "macos")]
#[test]
fn updater_native_product_failure_rolls_back_and_restart_failure_keeps_evidence() {
    let d = tempfile::tempdir().unwrap();
    let dir = d.path().canonicalize().unwrap();
    let root = synthetic(&dir, "Installed.app", "9.0.0");
    let mut host = NativeFixtureHost {
        root: root.clone(),
        receipt: dir.join("receipt.json"),
        current: "9.0.0".into(),
        target: "9.0.1".into(),
        probe_json: false,
        fail_restart: false,
        calls: RefCell::default(),
    };
    assert!(execute(&host, b"signed but invalid archive".to_vec()).is_err());
    crate::tun_service::codesign::validate_bundle(&root, "9.0.0").unwrap();
    let old: serde_json::Value = serde_json::from_slice(&fs::read(&host.receipt).unwrap()).unwrap();
    assert_eq!(old["failure"]["stage"], "replacing");
    assert_eq!(old["failure"]["disruptionStarted"], true);
    assert!(Path::new(old["backup"].as_str().unwrap()).exists());
    let next = synthetic(&dir, "Next.app", "9.0.1");
    host.fail_restart = true;
    assert!(execute(&host, package(&next)).is_err());
    let old: serde_json::Value = serde_json::from_slice(&fs::read(&host.receipt).unwrap()).unwrap();
    assert_eq!(old["failure"]["stage"], "restarting");
    assert!(Path::new(old["backup"].as_str().unwrap())
        .join("previous.app")
        .exists());
    host.fail_restart = false;
    host.restart().unwrap();
    assert!(!Path::new(old["backup"].as_str().unwrap()).exists());
}
#[test]
#[ignore = "requires signed release archive and published baseline; mandatory release gate"]
fn updater_native_release_artifact_transaction() {
    let archive =
        PathBuf::from(std::env::var("SERYLANE_UPDATE_ARCHIVE").expect("candidate archive"));
    let baseline =
        PathBuf::from(std::env::var("SERYLANE_UPDATE_BASELINE").expect("published baseline"));
    let current = std::env::var("SERYLANE_UPDATE_BASELINE_VERSION").expect("baseline version");
    let d = tempfile::tempdir().unwrap();
    let dir = d.path().canonicalize().unwrap();
    let root = dir.join("Serylane.app");
    #[cfg(target_os = "macos")]
    command(Command::new("ditto").arg(&baseline).arg(&root));
    #[cfg(target_os = "linux")]
    fs::copy(&baseline, &root).unwrap();
    let sentinel = dir.join("settings.json");
    fs::write(&sentinel, b"synthetic settings remain intact").unwrap();
    let mut host = NativeFixtureHost {
        root,
        receipt: dir.join("receipt.json"),
        current,
        target: env!("CARGO_PKG_VERSION").into(),
        probe_json: true,
        fail_restart: false,
        calls: RefCell::default(),
    };
    execute(&host, fs::read(archive).unwrap()).unwrap();
    assert_eq!(
        fs::read(&sentinel).unwrap(),
        b"synthetic settings remain intact"
    );
    let journal: serde_json::Value =
        serde_json::from_slice(&fs::read(&host.receipt).unwrap()).unwrap();
    assert_eq!(journal["stage"], "confirmed");
    // A second signed, higher-version test update exercises the new coordinator
    // again. No public tag or production artifact is modified for this test.
    let next = synthetic(&dir, "Next.app", "99.0.0");
    host.current = host.target;
    host.target = "99.0.0".into();
    host.probe_json = false;
    execute(&host, package(&next)).unwrap();
    assert_eq!(
        fs::read(&sentinel).unwrap(),
        b"synthetic settings remain intact"
    );
}
