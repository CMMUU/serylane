use super::contract_tests::Fixture;
use super::transport::{recover, Policy, TransportFailure};
use super::*;
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::atomic::AtomicUsize,
    thread,
};

struct Server {
    url: Url,
    calls: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}
impl Server {
    fn new(responses: Vec<(u16, Duration)>, body: Vec<u8>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}/archive", listener.local_addr().unwrap())
            .parse()
            .unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let done = stop.clone();
        let worker = thread::spawn(move || {
            while !done.load(Ordering::Acquire) {
                let (mut stream, _) = match listener.accept() {
                    Ok(v) => v,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                    Err(e) => panic!("{e}"),
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                stream
                    .set_write_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut request = vec![];
                let mut buf = [0u8; 1024];
                while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                    let n = stream.read(&mut buf).unwrap_or(0);
                    if n == 0 || request.len() > 16384 {
                        break;
                    }
                    request.extend_from_slice(&buf[..n]);
                }
                let i = count.fetch_add(1, Ordering::SeqCst);
                let (status, delay) = responses.get(i).unwrap_or(responses.last().unwrap());
                thread::sleep(*delay);
                if *status == 0 {
                    continue;
                }
                let data = if *status == 200 {
                    body.as_slice()
                } else {
                    b"error"
                };
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    data.len()
                );
                let _ = stream.write_all(data);
            }
        });
        Self {
            url,
            calls,
            stop,
            worker: Some(worker),
        }
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.worker.take().unwrap().join().unwrap();
    }
}
fn policy() -> Policy {
    Policy {
        idle: Duration::from_secs(2),
        total: Duration::from_secs(8),
        backoff: Duration::from_millis(10),
        attempts: 2,
    }
}
fn mirror(c: &Candidate, source: UpdateSource) -> Candidate {
    let mut u = c.update.clone();
    u.download_url = asset_url(
        source,
        &u.version,
        &artifact_name(&c.platform_key, &u.version).unwrap(),
    )
    .unwrap()
    .parse()
    .unwrap();
    u.raw_json["platforms"][&c.platform_key]["url"] = u.download_url.as_str().into();
    Candidate::from_update(u, source, &UpdatePlatform::native().unwrap()).unwrap()
}
fn fixture() -> Fixture {
    Fixture::new(
        b"signed recovery fixture".to_vec(),
        "9.0.1",
        &std::env::temp_dir().join("never-install/serylane"),
        None,
    )
}
#[test]
fn updater_recovery_transient_status_and_disconnect_retry_same_signed_build() {
    let f = fixture();
    for status in [0, 408, 429, 500, 502, 503, 504] {
        let s = Server::new(
            vec![(status, Duration::ZERO), (200, Duration::ZERO)],
            f.bytes.clone(),
        );
        let m = AppUpdateManager::default();
        let result = tauri::async_runtime::block_on(recover(
            &m,
            vec![f.candidate.clone()],
            false,
            policy(),
            |_| async { panic!("no discovery in explicit-source mode") },
            |_| {
                let mut t = f.transport.clone();
                t.download_url = s.url.clone();
                t
            },
        ))
        .unwrap();
        assert_eq!(result.phase, "ready");
        assert_eq!(s.calls.load(Ordering::SeqCst), 2);
        assert_eq!(result.attempts.len(), 2);
        assert!(m.lock().unwrap().ready.is_some());
    }
}
#[test]
fn updater_recovery_rediscovers_missing_backup_and_excludes_different_build() {
    let f = fixture();
    let primary = mirror(&f.candidate, UpdateSource::Hk);
    for mismatch in ["none", "hash", "version", "platform", "size", "signature"] {
        let m = AppUpdateManager::default();
        let s = Server::new(vec![(404, Duration::ZERO)], vec![]);
        let found = AtomicUsize::new(0);
        let r = tauri::async_runtime::block_on(recover(
            &m,
            vec![primary.clone()],
            true,
            policy(),
            |source| {
                found.fetch_add(1, Ordering::SeqCst);
                let mut c = mirror(&f.candidate, source);
                match mismatch {
                    "hash" => c.sha256 = "0".repeat(64),
                    "version" => c.version = StableVersion(9, 0, 2),
                    "platform" => c.platform_key = "other-platform".into(),
                    "size" => c.size += 1,
                    "signature" => c.update.signature = "other-signature".into(),
                    _ => {}
                }
                async move {
                    if source == UpdateSource::Gitee {
                        Err(failure("HTTP 404"))
                    } else {
                        Ok(c)
                    }
                }
            },
            |c| {
                let mut t = f.transport.clone();
                if c.source == UpdateSource::Hk {
                    t.download_url = s.url.clone();
                }
                t
            },
        ));
        assert_eq!(s.calls.load(Ordering::SeqCst), 1, "404 must not retry");
        assert!(found.load(Ordering::SeqCst) > 0);
        if mismatch != "none" {
            assert!(r.is_err());
            assert!(m.lock().unwrap().ready.is_none());
        } else {
            assert_eq!(r.unwrap().phase, "ready");
            let session = m.lock().unwrap();
            assert_eq!(session.candidates[0].source, UpdateSource::Github);
            assert!(same_build(&primary, &session.ready.as_ref().unwrap().0));
        }
    }
}
#[test]
fn updater_recovery_idle_budget_and_cancel_are_bounded_without_ready_bytes() {
    let f = fixture();
    for mode in ["idle", "budget", "cancel-backoff", "cancel-discovery"] {
        let m = AppUpdateManager::default();
        let s = Server::new(
            vec![(
                if mode == "cancel-discovery" {
                    404
                } else if mode == "cancel-backoff" {
                    503
                } else {
                    200
                },
                if mode == "idle" || mode == "budget" {
                    Duration::from_millis(350)
                } else {
                    Duration::ZERO
                },
            )],
            f.bytes.clone(),
        );
        let p = Policy {
            idle: if mode == "idle" {
                Duration::from_millis(90)
            } else {
                Duration::from_secs(2)
            },
            total: if mode == "budget" {
                Duration::from_millis(90)
            } else {
                Duration::from_secs(4)
            },
            backoff: Duration::from_secs(2),
            attempts: if mode == "idle" { 1 } else { 2 },
        };
        let began = std::time::Instant::now();
        let (r, _) = tauri::async_runtime::block_on(async {
            tokio::join!(
                recover(
                    &m,
                    vec![f.candidate.clone()],
                    mode == "cancel-discovery",
                    p,
                    |_| async {
                        tokio::time::sleep(Duration::from_secs(10)).await;
                        Err(failure("late discovery"))
                    },
                    |_| {
                        let mut t = f.transport.clone();
                        t.download_url = s.url.clone();
                        t
                    }
                ),
                async {
                    if mode.starts_with("cancel") {
                        tokio::time::sleep(Duration::from_millis(150)).await;
                        m.cancelled.store(true, Ordering::Release);
                    }
                }
            )
        });
        assert!(r.is_err(), "{mode}");
        assert!(began.elapsed() < Duration::from_secs(2), "{mode}");
        assert!(m.lock().unwrap().ready.is_none());
        if mode == "idle" {
            assert_eq!(m.lock().unwrap().status.error_kind.as_deref(), Some("idle"));
        }
        if mode == "budget" {
            assert_eq!(
                m.lock().unwrap().status.error_kind.as_deref(),
                Some("budget")
            );
        }
    }
}
#[test]
fn updater_recovery_signature_or_identity_failure_never_retries_or_falls_back() {
    let f = fixture();
    for mode in ["signature", "hash", "size"] {
        let m = AppUpdateManager::default();
        let s = Server::new(vec![(200, Duration::ZERO)], f.bytes.clone());
        let mut c = f.candidate.clone();
        if mode == "hash" {
            c.sha256 = "0".repeat(64);
        }
        if mode == "size" {
            c.size += 1;
        }
        let r = tauri::async_runtime::block_on(recover(
            &m,
            vec![c],
            true,
            policy(),
            |_| async { panic!("security failure must not fall back") },
            |_| {
                let mut t = f.transport.clone();
                t.download_url = s.url.clone();
                if mode == "signature" {
                    t.signature = "invalid".into();
                }
                t
            },
        ));
        assert!(r.is_err());
        assert_eq!(s.calls.load(Ordering::SeqCst), 1);
        assert!(m.lock().unwrap().ready.is_none());
    }
}
#[test]
fn updater_recovery_fixed_manifest_urls_and_status_policy_are_strict() {
    for source in [UpdateSource::Hk, UpdateSource::Github, UpdateSource::Gitee] {
        let m = manifest_location(source, Some("v0.7.29")).unwrap();
        assert_eq!(m.expected_version, Some(StableVersion(0, 7, 29)));
        assert!(m.endpoints.iter().all(|u| u.path().contains("/v0.7.29/")));
        assert!(manifest_location(source, Some("v0.7.29?token=secret")).is_err());
    }
    for status in [401, 403, 404, 410] {
        let e = TransportFailure::from_updater(tauri_plugin_updater::Error::Network(format!(
            "Download request failed with status: {status} Error"
        )));
        assert!(!e.retryable);
        assert_eq!(e.status, Some(status));
    }
}
