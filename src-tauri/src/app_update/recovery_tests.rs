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
        Self::with_redirect(responses, body, None)
    }
    fn with_redirect(
        responses: Vec<(u16, Duration)>,
        body: Vec<u8>,
        redirect: Option<String>,
    ) -> Self {
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
                let location = redirect
                    .as_ref()
                    .map(|url| format!("Location: {url}\r\n"))
                    .unwrap_or_default();
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status} Test\r\n{location}Content-Length: {}\r\nConnection: close\r\n\r\n",
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
            policy(),
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
fn updater_center_down_retries_only_frozen_center_build_then_fails_closed() {
    let f = fixture();
    let unused_alternative = Server::new(vec![(200, Duration::ZERO)], f.bytes.clone());
    for (status, expected) in [(404, 1), (503, 2)] {
        let m = AppUpdateManager::default();
        let center = Server::new(vec![(status, Duration::ZERO)], vec![]);
        let result = tauri::async_runtime::block_on(recover(
            &m,
            vec![f.candidate.clone()],
            policy(),
            |candidate| {
                assert!(same_build(candidate, &f.candidate));
                assert_eq!(
                    candidate.update.download_url,
                    f.candidate.update.download_url
                );
                let mut transport = f.transport.clone();
                transport.download_url = center.url.clone();
                transport
            },
        ));
        assert!(result.unwrap_err().to_string().contains("请稍后重试"));
        assert_eq!(center.calls.load(Ordering::SeqCst), expected);
        assert_eq!(unused_alternative.calls.load(Ordering::SeqCst), 0);
        assert!(m.lock().unwrap().ready.is_none());
        assert!(m
            .lock()
            .unwrap()
            .status
            .attempts
            .iter()
            .all(|attempt| attempt.source == UpdateSource::Hk));
    }
    for candidates in [vec![], vec![f.candidate.clone(), f.candidate.clone()]] {
        assert!(tauri::async_runtime::block_on(recover(
            &AppUpdateManager::default(),
            candidates,
            policy(),
            |_| panic!("ambiguous targets must fail before connecting"),
        ))
        .is_err());
    }
}

#[test]
fn updater_center_redirects_never_follow_external_or_mirror_locations() {
    let f = fixture();
    let alternative = Server::new(vec![(200, Duration::ZERO)], f.bytes.clone());
    for location in [
        alternative.url.to_string(),
        "https://github.com/CMMUU/serylane/releases/latest".into(),
        "https://gitee.com/cmmuu/serylane/releases/latest".into(),
    ] {
        let redirect = Server::with_redirect(vec![(302, Duration::ZERO)], vec![], Some(location));
        let m = AppUpdateManager::default();
        let result = tauri::async_runtime::block_on(recover(
            &m,
            vec![f.candidate.clone()],
            policy(),
            |_| {
                let mut transport = f.transport.clone();
                transport.download_url = redirect.url.clone();
                transport
            },
        ));
        assert!(result.is_err());
        assert_eq!(redirect.calls.load(Ordering::SeqCst), 1);
        assert_eq!(alternative.calls.load(Ordering::SeqCst), 0);
        assert!(m.lock().unwrap().ready.is_none());
    }
}

#[test]
fn updater_recovery_idle_budget_and_cancel_are_bounded_without_ready_bytes() {
    let f = fixture();
    for mode in ["idle", "budget", "cancel-backoff", "cancel-download"] {
        let m = AppUpdateManager::default();
        let s = Server::new(
            vec![(
                if mode == "cancel-backoff" { 503 } else { 200 },
                if mode == "idle" || mode == "budget" || mode == "cancel-download" {
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
                recover(&m, vec![f.candidate.clone()], p, |_| {
                    let mut t = f.transport.clone();
                    t.download_url = s.url.clone();
                    t
                }),
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
        let r = tauri::async_runtime::block_on(recover(&m, vec![c], policy(), |_| {
            let mut t = f.transport.clone();
            t.download_url = s.url.clone();
            if mode == "signature" {
                t.signature = "invalid".into();
            }
            t
        }));
        assert!(r.is_err());
        assert_eq!(
            s.calls.load(Ordering::SeqCst),
            if mode == "signature" { 1 } else { 0 }
        );
        assert!(m.lock().unwrap().ready.is_none());
    }
}
#[test]
fn updater_recovery_fixed_manifest_urls_and_status_policy_are_strict() {
    let source = UpdateSource::Hk;
    let m = manifest_location(source, Some("v0.7.29")).unwrap();
    assert_eq!(m.expected_version, Some(StableVersion(0, 7, 29)));
    assert!(m.endpoints.iter().all(|u| u.path().contains("/v0.7.29/")));
    assert!(manifest_location(source, Some("v0.7.29?token=secret")).is_err());
    for status in [401, 403, 404, 410] {
        let e = TransportFailure::from_updater(tauri_plugin_updater::Error::Network(format!(
            "Download request failed with status: {status} Error"
        )));
        assert!(!e.retryable);
        assert_eq!(e.status, Some(status));
    }
}
