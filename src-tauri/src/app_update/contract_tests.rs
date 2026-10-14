use super::transaction::{run_install, InstallHost};
use super::*;
use std::{
    cell::RefCell,
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    process::Command,
    thread,
};

pub(super) struct Fixture {
    listener: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
    pub candidate: Candidate,
    pub transport: Update,
    pub bytes: Vec<u8>,
    pub platform: UpdatePlatform,
    _temp: tempfile::TempDir,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.listener.store(true, Ordering::Release);
        self.worker.take().unwrap().join().unwrap();
    }
}
impl Fixture {
    pub fn new(
        bytes: Vec<u8>,
        version: &str,
        executable: &Path,
        installer: Option<&'static str>,
    ) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let archive = temp.path().join("fixture");
        fs::write(&archive, &bytes).unwrap();
        let output = Command::new("node")
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../scripts/sign-update-fixture.mjs"))
            .arg(&archive)
            .output()
            .unwrap();
        assert!(output.status.success(), "fixture signer failed");
        let signed: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        let mut platform = UpdatePlatform::native().unwrap();
        platform.installer = installer;
        let key = platform.keys()[0].clone();
        let official = asset_url(
            UpdateSource::Github,
            version,
            &artifact_name(&key, version).unwrap(),
        )
        .unwrap();
        let mut manifest = serde_json::json!({"version": version, "platforms": {key.clone(): {
            "url": official, "signature": signed["signature"], "size": bytes.len(), "sha256": format!("{:x}",Sha256::digest(&bytes))
        }}});
        if platform.base.starts_with("windows-") && installer != Some("msi") {
            manifest["platforms"][&platform.base] = manifest["platforms"][&key].clone();
        }
        let server = TcpListener::bind("127.0.0.1:0").unwrap();
        server.set_nonblocking(true).unwrap();
        let address = server.local_addr().unwrap();
        let done = Arc::new(AtomicBool::new(false));
        let stop = done.clone();
        let body = bytes.clone();
        let worker = thread::spawn(move || {
            let manifest = serde_json::to_vec(&manifest).unwrap();
            while !stop.load(Ordering::Acquire) {
                let (mut stream, _) = match server.accept() {
                    Ok(value) => value,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                    Err(e) => panic!("{e}"),
                };
                // macOS accepted sockets can inherit O_NONBLOCK from the listener.
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                stream
                    .set_write_timeout(Some(Duration::from_secs(10)))
                    .unwrap();
                // Drain complete request headers before closing: closing with unread
                // bytes can reset the TCP connection and truncate a valid response.
                let mut request = Vec::new();
                while !request.windows(4).any(|value| value == b"\r\n\r\n") {
                    let mut buffer = [0_u8; 4096];
                    let count = stream.read(&mut buffer).unwrap_or_default();
                    if count == 0 || request.len() + count > 16 * 1024 {
                        break;
                    }
                    request.extend_from_slice(&buffer[..count]);
                }
                let request = String::from_utf8_lossy(&request);
                let data = if request.starts_with("GET /manifest ") {
                    &manifest
                } else {
                    &body
                };
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    data.len()
                );
                let _ = stream.write_all(data); // Cancellation may close the test connection.
            }
        });
        let mut context = tauri::test::mock_context(tauri::test::noop_assets());
        context.config_mut().plugins.0.insert(
            "updater".into(),
            serde_json::json!({
                "pubkey": signed["pubkey"], "dangerousInsecureTransportProtocol":true,
                "windows":{"installMode":"quiet"}
            }),
        );
        let app = tauri::test::mock_builder()
            .plugin(tauri_plugin_updater::Builder::new().build())
            .build(context)
            .unwrap();
        let mut builder = app
            .updater_builder()
            .executable_path(executable)
            .endpoints(vec![format!("http://{address}/manifest").parse().unwrap()])
            .unwrap()
            .no_proxy()
            .version_comparator(|_, _| true)
            .timeout(Duration::from_secs(15));
        // A test binary has no MSI bundle marker. Only the MSI scenario selects
        // it explicitly; the default OS field is tested unchanged on every OS.
        if installer == Some("msi") {
            builder = builder.target(&key);
        }
        #[cfg(windows)]
        {
            let parent = executable.parent().unwrap().to_string_lossy();
            // Rust canonical paths use the Win32 extended prefix; NSIS /D
            // expects the normal absolute spelling (including spaces).
            let parent = parent.strip_prefix(r"\\?\").unwrap_or(&parent);
            builder =
                builder
                    .restart_after_install(false)
                    .installer_arg(if installer == Some("msi") {
                        format!("INSTALLDIR=\"{parent}\"")
                    } else {
                        format!("/D={parent}")
                    });
        }
        let update = tauri::async_runtime::block_on(builder.build().unwrap().check())
            .unwrap()
            .unwrap();
        let candidate =
            Candidate::from_update(update.clone(), UpdateSource::Github, &platform).unwrap();
        let mut transport = update;
        transport.download_url = format!("http://{address}/archive").parse().unwrap();
        // Only test transport is rerouted. The stored identity keeps the exact
        // official origin, architecture, version, signature and original bytes.
        Self {
            listener: done,
            worker: Some(worker),
            candidate,
            transport,
            bytes,
            platform,
            _temp: temp,
        }
    }
    pub fn download(&self) -> Vec<u8> {
        tauri::async_runtime::block_on(download_with_transport(
            &self.candidate,
            &self.transport,
            &AppUpdateManager::default(),
        ))
        .unwrap()
    }
    pub fn session(&self) -> Mutex<Session> {
        ready_session(&self.candidate, self.download())
    }
}

pub(super) fn ready_session(candidate: &Candidate, bytes: Vec<u8>) -> Mutex<Session> {
    Mutex::new(Session {
        status: AppUpdateStatus {
            phase: "ready".into(),
            stage: "verified".into(),
            info: Some(AppUpdateInfo {
                current_version: "9.0.0".into(),
                latest_version: format!("v{}", candidate.update.version),
                available: true,
                ahead: false,
                notes: String::new(),
                published_at: None,
                release_url: String::new(),
                source: candidate.source,
                channels: vec![],
            }),
            ..Default::default()
        },
        ready: Some((candidate.clone(), bytes)),
        ..Default::default()
    })
}
fn test_executable(root: &Path) -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        root.join("Serylane.app/Contents/MacOS/serylane")
    }
    #[cfg(windows)]
    {
        root.join("serylane.exe")
    }
    #[cfg(target_os = "linux")]
    {
        root.join("Serylane.AppImage")
    }
}
#[derive(Default)]
struct RecordingHost {
    calls: RefCell<Vec<String>>,
    failure: &'static str,
}
impl RecordingHost {
    fn call(&self, name: &str) -> AppResult<()> {
        self.calls.borrow_mut().push(name.into());
        if name == self.failure {
            return Err(failure(format!("fixture {name}")));
        }
        if self.failure == "panic" && name == "replace" {
            panic!("fixture replacement panic");
        }
        Ok(())
    }
}
impl InstallHost for RecordingHost {
    type Prepared = ();
    fn stage(&self, _: &str, stage: &str) -> AppResult<()> {
        self.call(&format!("journal:{stage}"))
    }
    fn preflight(&self, _: &Candidate) -> AppResult<()> {
        self.call("preflight")
    }
    fn stop(&self) -> AppResult<()> {
        self.call("stop")
    }
    fn prepare_services(&self, _: &str) -> AppResult<()> {
        self.call("services")
    }
    fn replace(&self, _: &Candidate, _: Vec<u8>, _: ()) -> AppResult<()> {
        self.call("replace")
    }
    fn rollback_services(&self) -> AppResult<()> {
        self.call("rollback")
    }
    fn restart(&self) -> AppResult<()> {
        self.call("restart")
    }
    fn failed(&self, _: &str, _: &str, _: &str, _: bool) -> AppResult<()> {
        self.call("failure-receipt")
    }
}
#[test]
fn updater_contract_real_tauri_os_field_passes_product_preflight() {
    let d = tempfile::tempdir().unwrap();
    let f = Fixture::new(
        b"signed fixture bytes".to_vec(),
        "9.0.1",
        &test_executable(d.path()),
        None,
    );
    assert_eq!(
        f.candidate.update.target,
        f.platform.base.split('-').next().unwrap()
    );
    assert_ne!(
        f.candidate.update.target,
        tauri_plugin_updater::target().unwrap()
    ); // Previous regression.
    let session = f.session();
    let candidate = select_install(&mut session.lock().unwrap(), "v9.0.1").unwrap();
    let host = RecordingHost::default();
    run_install(&session, &candidate, "9.0.0", &f.platform, &host).unwrap();
    assert_eq!(
        *host.calls.borrow(),
        [
            "journal:preflight",
            "preflight",
            "journal:stopping",
            "stop",
            "journal:services",
            "services",
            "journal:replacing",
            "replace",
            "journal:restarting",
            "restart"
        ]
    );
    assert!(session.lock().unwrap().ready.is_none());
}
#[test]
fn updater_contract_identity_tampering_is_rejected_before_any_stop() {
    let d = tempfile::tempdir().unwrap();
    let f = Fixture::new(
        b"signed fixture bytes".to_vec(),
        "9.0.1",
        &test_executable(d.path()),
        None,
    );
    for field in [
        "version",
        "raw-version",
        "platform",
        "target",
        "url",
        "signature",
        "hash",
        "size",
        "bytes",
        "same-version",
        "downgrade",
        "manifest-signature",
    ] {
        let session = ready_session(&f.candidate, f.bytes.clone());
        let selected = select_install(&mut session.lock().unwrap(), "v9.0.1").unwrap();
        {
            let mut s = session.lock().unwrap();
            let (c, bytes) = s.ready.as_mut().unwrap();
            match field {
                "version" => c.version = StableVersion(9, 0, 2),
                "raw-version" => c.update.raw_json["version"] = "9.0.2".into(),
                "platform" => c.platform_key = "other-aarch64".into(),
                "target" => c.update.target = "other-os".into(),
                "url" => {
                    c.update.download_url = "https://example.invalid/wrong-package".parse().unwrap()
                }
                "signature" => c.update.signature.push('A'),
                "manifest-signature" => {
                    c.update.raw_json["platforms"][&c.platform_key]["signature"] = "changed".into()
                }
                "hash" => c.sha256 = "0".repeat(64),
                "size" => c.size += 1,
                "bytes" => bytes[0] ^= 1,
                _ => {}
            }
        }
        let host = RecordingHost::default();
        let current = match field {
            "same-version" => "9.0.1",
            "downgrade" => "9.0.2",
            _ => "9.0.0",
        };
        assert!(
            run_install(&session, &selected, current, &f.platform, &host).is_err(),
            "{field}"
        );
        let state = session.lock().unwrap();
        assert!(
            !state.status.retry_install && state.ready.is_none(),
            "{field}"
        );
        assert_eq!(
            *host.calls.borrow(),
            ["journal:preflight", "failure-receipt"],
            "{field}"
        );
    }
}
#[test]
fn updater_contract_transient_preflight_retries_without_download_but_never_retries_partial_install()
{
    let d = tempfile::tempdir().unwrap();
    let f = Fixture::new(
        b"signed fixture bytes".to_vec(),
        "9.0.1",
        &test_executable(d.path()),
        None,
    );
    for problem in [
        "preflight",
        "journal:preflight",
        "journal:stopping",
        "stop",
        "journal:services",
        "services",
        "replace",
        "panic",
        "journal:restarting",
        "restart",
    ] {
        let session = f.session();
        let selected = select_install(&mut session.lock().unwrap(), "v9.0.1").unwrap();
        let host = RecordingHost {
            failure: problem,
            ..Default::default()
        };
        assert!(
            run_install(&session, &selected, "9.0.0", &f.platform, &host).is_err(),
            "{problem}"
        );
        let retry = ["preflight", "journal:preflight", "journal:stopping"].contains(&problem);
        assert_eq!(
            session.lock().unwrap().status.retry_install,
            retry,
            "{problem}"
        );
        if retry {
            assert!(!host.calls.borrow().iter().any(|x| x == "stop"));
            let selected = select_install(&mut session.lock().unwrap(), "v9.0.1").unwrap();
            run_install(
                &session,
                &selected,
                "9.0.0",
                &f.platform,
                &RecordingHost::default(),
            )
            .unwrap();
        } else {
            assert!(select_install(&mut session.lock().unwrap(), "v9.0.1").is_err());
        }
        if ["services", "replace", "panic"].contains(&problem) {
            assert!(host.calls.borrow().iter().any(|x| x == "rollback"));
        }
    }
}
#[test]
fn updater_contract_download_cancellation_bad_signature_and_size_never_become_ready() {
    let d = tempfile::tempdir().unwrap();
    let f = Fixture::new(
        b"signed fixture bytes".to_vec(),
        "9.0.1",
        &test_executable(d.path()),
        None,
    );
    for problem in ["cancel", "signature", "size", "hash"] {
        let manager = AppUpdateManager::default();
        let mut c = f.candidate.clone();
        let mut transport = f.transport.clone();
        match problem {
            "cancel" => manager.cancelled.store(true, Ordering::Release),
            "signature" => transport.signature = "invalid-signature".into(),
            "size" => c.size += 1,
            "hash" => c.sha256 = "0".repeat(64),
            _ => unreachable!(),
        }
        assert!(
            tauri::async_runtime::block_on(download_with_transport(&c, &transport, &manager))
                .is_err(),
            "{problem}"
        );
        assert!(manager.lock().unwrap().ready.is_none());
    }
}
#[test]
fn updater_contract_platform_keys_keep_architecture_and_windows_format_separate() {
    for arch in ["x86_64", "aarch64"] {
        for os in ["darwin", "linux"] {
            let base = format!("{os}-{arch}");
            let p = UpdatePlatform {
                base: base.clone(),
                installer: None,
            };
            assert_eq!(p.keys(), [base]);
        }
        let base = format!("windows-{arch}");
        assert_eq!(
            UpdatePlatform {
                base: base.clone(),
                installer: Some("msi")
            }
            .keys(),
            [format!("{base}-msi")]
        );
        assert_eq!(
            UpdatePlatform {
                base: base.clone(),
                installer: Some("nsis")
            }
            .keys(),
            [format!("{base}-nsis"), base]
        );
    }
}

#[cfg(windows)]
#[test]
#[ignore = "runs only as a disposable Windows installer child; parent must verify completion"]
fn updater_native_windows_installer_child() {
    assert_eq!(std::env::var("GITHUB_ACTIONS").as_deref(), Ok("true"));
    assert_eq!(
        std::env::var("RUNNER_ENVIRONMENT").as_deref(),
        Ok("github-hosted")
    );
    assert_eq!(
        std::env::var("GITHUB_REPOSITORY").as_deref(),
        Ok("CMMUU/serylane")
    );
    let root = PathBuf::from(std::env::var("SERYLANE_UPDATE_INSTALL_DIR").unwrap())
        .canonicalize()
        .unwrap();
    assert!(root.starts_with(
        PathBuf::from(std::env::var("RUNNER_TEMP").unwrap())
            .canonicalize()
            .unwrap()
    ));
    let archive = PathBuf::from(std::env::var("SERYLANE_UPDATE_ARCHIVE").unwrap());
    let installer = if archive.extension().unwrap() == "msi" {
        "msi"
    } else {
        "nsis"
    };
    let receipt = PathBuf::from(std::env::var("SERYLANE_UPDATE_RECEIPT").unwrap());
    assert!(receipt
        .parent()
        .unwrap()
        .canonicalize()
        .unwrap()
        .starts_with(root.parent().unwrap()));
    struct WindowsHost {
        root: PathBuf,
        receipt: PathBuf,
    }
    impl InstallHost for WindowsHost {
        type Prepared = ();
        fn stage(&self, target: &str, stage: &str) -> AppResult<()> {
            crate::update_receipt::write_at(&self.receipt, target, stage)
        }
        fn preflight(&self, _: &Candidate) -> AppResult<()> {
            assert!(self.root.join("routedeck.exe").is_file());
            Ok(())
        }
        fn stop(&self) -> AppResult<()> {
            Ok(())
        }
        fn prepare_services(&self, _: &str) -> AppResult<()> {
            Ok(())
        }
        fn replace(&self, c: &Candidate, bytes: Vec<u8>, _: ()) -> AppResult<()> {
            c.update.install(bytes).map_err(|e| failure(e.to_string()))
        }
        fn rollback_services(&self) -> AppResult<()> {
            Ok(())
        }
        fn restart(&self) -> AppResult<()> {
            Err(failure("Windows installer should have exited the child"))
        }
        fn failed(&self, t: &str, s: &str, r: &str, d: bool) -> AppResult<()> {
            crate::update_receipt::failed_at(&self.receipt, t, s, r, d)
        }
    }
    let f = Fixture::new(
        fs::read(archive).unwrap(),
        env!("CARGO_PKG_VERSION"),
        &root.join("routedeck.exe"),
        Some(installer),
    );
    let session = f.session();
    let selected = select_install(
        &mut session.lock().unwrap(),
        &format!("v{}", env!("CARGO_PKG_VERSION")),
    )
    .unwrap();
    let result = run_install(
        &session,
        &selected,
        "0.7.6",
        &f.platform,
        &WindowsHost { root, receipt },
    );
    panic!("installer did not transfer control: {result:?}");
}
