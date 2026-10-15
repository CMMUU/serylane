//! Real product compiler, transaction coordinator, storage and bundled core.
//! All ports, files and traffic are loopback-only, independent of the installed app.
use super::*;
use crate::{
    mihomo_api::MihomoApiClient,
    user_rules::{transact, ProfileReloader},
};
use rand::Rng;
use std::{
    cell::Cell,
    net::{TcpListener, UdpSocket},
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::Duration,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

struct Core {
    child: Child,
    log: PathBuf,
}
impl Drop for Core {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if std::thread::panicking() {
            // Synthetic loopback fixture only; never read the installed app's log.
            eprintln!(
                "isolated core: {}",
                std::fs::read_to_string(&self.log).unwrap_or_default()
            );
        }
    }
}
fn binary() -> PathBuf {
    let target = format!(
        "{}-{}",
        std::env::consts::ARCH,
        if cfg!(windows) {
            "pc-windows-msvc"
        } else if cfg!(target_os = "macos") {
            "apple-darwin"
        } else {
            "unknown-linux-gnu"
        }
    );
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("binaries")
        .join(format!("mihomo-{target}{}", std::env::consts::EXE_SUFFIX))
}
struct ReservedPort {
    tcp: TcpListener,
    _udp: UdpSocket,
}
impl ReservedPort {
    fn number(&self) -> u16 {
        self.tcp.local_addr().unwrap().port()
    }
}
fn port() -> ReservedPort {
    // The external core cannot inherit our listener. Avoid the OS ephemeral
    // range, where parallel fixture clients may claim a just-released port.
    // Reserve both protocols through validation; no retry after core launch.
    for _ in 0..100 {
        let number = rand::rng().random_range(20000..30000);
        if let Ok(tcp) = TcpListener::bind(("127.0.0.1", number)) {
            if let Ok(udp) = UdpSocket::bind(("127.0.0.1", number)) {
                return ReservedPort { tcp, _udp: udp };
            }
        }
    }
    panic!("no isolated fixture port available");
}
fn start(binary: &std::path::Path, dir: &std::path::Path, config: &str) -> Core {
    let path = dir.join("isolated.yaml");
    std::fs::write(&path, config).unwrap();
    let log = dir.join("isolated-core.log");
    let output = std::fs::File::create(&log).unwrap();
    Core {
        child: Command::new(binary)
            .arg("-d")
            .arg(dir)
            .arg("-f")
            .arg(path)
            .stdout(Stdio::from(output.try_clone().unwrap()))
            .stderr(Stdio::from(output))
            .spawn()
            .unwrap(),
        log,
    }
}
async fn wait_proxy_ready(port: u16) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if tokio::net::TcpStream::connect(("127.0.0.1", port))
                .await
                .is_ok()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("isolated mixed proxy listener did not become ready");
}
async fn respond(mut socket: tokio::net::TcpStream) -> std::io::Result<()> {
    tokio::time::timeout(Duration::from_secs(5), async {
        let mut header = Vec::new();
        while !header.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
            let mut chunk = [0; 1024];
            let size = socket.read(&mut chunk).await?;
            if size == 0 || header.len() + size > 8192 {
                return Err(std::io::Error::other(
                    "incomplete or oversized fixture header",
                ));
            }
            header.extend_from_slice(&chunk[..size]);
        }
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
            .await?;
        socket.shutdown().await
    })
    .await
    .map_err(|_| std::io::Error::other("fixture header deadline"))?
}
async fn request(mixed: u16, web: u16, host: &str) -> Result<u16, String> {
    reqwest::Client::builder()
        .no_proxy()
        .proxy(reqwest::Proxy::http(format!("http://127.0.0.1:{mixed}")).unwrap())
        .timeout(Duration::from_secs(2))
        .build()
        .unwrap()
        .get(format!("http://{host}:{web}/fixture"))
        .header("Connection", "close")
        .send()
        .await
        .map(|r| r.status().as_u16())
        .map_err(|error| format!("isolated request: {error:?}"))
}

#[tokio::test]
async fn native_http_fixture_waits_for_complete_fragmented_headers() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut client = tokio::net::TcpStream::connect(listener.local_addr().unwrap())
        .await
        .unwrap();
    let (server, _) = listener.accept().await.unwrap();
    let task = tokio::spawn(respond(server));
    client
        .write_all(b"GET /fixture HTTP/1.1\r\nHo")
        .await
        .unwrap();
    let mut first = [0];
    assert!(
        tokio::time::timeout(Duration::from_millis(30), client.read_exact(&mut first))
            .await
            .is_err()
    );
    client
        .write_all(b"st: fixture\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut response = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), client.read_to_end(&mut response))
        .await
        .unwrap()
        .unwrap();
    assert!(response.starts_with(b"HTTP/1.1 200 OK\r\n"));
    assert!(response.ends_with(b"\r\n\r\nok"));
    task.await.unwrap().unwrap();
}
#[tokio::test]
async fn native_routing_transaction_load_verify_rollback_restart_and_match() {
    let binary = binary();
    assert!(
        binary.is_file(),
        "run npm run core:prepare before native routing tests"
    );
    let dir = tempfile::tempdir().unwrap();
    let storage = AppStorage::from_root(dir.path().join("storage")).unwrap();
    let mixed = port();
    let control = port();
    let web = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let web_port = web.local_addr().unwrap().port();
    let server = tokio::spawn(async move {
        while let Ok((s, _)) = web.accept().await {
            tokio::spawn(async move {
                let _ = respond(s).await;
            });
        }
    });
    let mut settings = crate::models::AppSettings {
        mixed_port: mixed.number(),
        controller_port: control.number(),
        ..Default::default()
    };
    let source="dns: {enable: false}\nlog-level: info\nhosts: {route.example.test: 127.0.0.1, sub.route.example.test: 127.0.0.1, deep.sub.route.example.test: 127.0.0.1, fixed.example.test: 127.0.0.1, notroute.example.test: 127.0.0.1}\nproxies: []\nproxy-groups: [{name: Exit, type: select, proxies: [REJECT]}]\nrules: ['MATCH,DIRECT']";
    let base = build_effective_config_with_policy(source, &settings, RoutingMode::Rule, None)
        .unwrap()
        .yaml;
    let mut doc = Document {
        draft: expand(Selection {
            tools: vec![],
            custom_domains: vec!["*.route.example.test".into(), "fixed.example.test".into()],
            target: "Exit".into(),
        })
        .unwrap(),
        ..Default::default()
    };
    doc.bump(0).unwrap();
    doc.write(&storage).unwrap();
    let validation = dir.path().join("validated.yaml");
    std::fs::write(&validation, &base).unwrap();
    crate::runtime::validate_file(&binary, dir.path(), &validation).unwrap();
    // Only relinquish ports immediately before the actual listener starts.
    drop(mixed);
    drop(control);
    let mut core = start(&binary, dir.path(), &base);
    let api = MihomoApiClient::new(&settings).unwrap();
    api.wait_ready(Duration::from_secs(5)).await.unwrap();
    wait_proxy_ready(settings.mixed_port).await;
    api.verify_rule_config(&base).await.unwrap();
    assert!(!Document::read(&storage).unwrap().enabled);
    assert_eq!(
        request(settings.mixed_port, web_port, "sub.route.example.test").await,
        Ok(200)
    );
    doc.bump(1).unwrap();
    doc.enabled = true;
    doc.applied = Some(doc.draft.clone());
    settings.domain_routing = doc.clone();
    let enabled = build_effective_config_with_policy(source, &settings, RoutingMode::Rule, None)
        .unwrap()
        .yaml;
    let reloader = ProfileReloader {
        client: api.clone(),
        verify_routing: true,
        operation: None,
    };
    transact(
        Some(&reloader),
        &enabled,
        &base,
        || doc.write(&storage),
        || Ok(()),
    )
    .await
    .unwrap();
    assert!(Document::read(&storage).unwrap().enabled);
    for host in [
        "sub.route.example.test",
        "deep.sub.route.example.test",
        "fixed.example.test",
    ] {
        assert!(
            request(settings.mixed_port, web_port, host).await != Ok(200),
            "matched {host}"
        );
    }
    for host in ["route.example.test", "notroute.example.test"] {
        assert_eq!(
            request(settings.mixed_port, web_port, host).await,
            Ok(200),
            "unmatched {host}"
        );
    }
    api.set_mode(RoutingMode::Global).await.unwrap();
    assert!(api.verify_rule_config(&enabled).await.is_err());
    transact(Some(&reloader), &enabled, &base, || Ok(()), || Ok(()))
        .await
        .unwrap();
    let committed = Cell::new(false);
    assert!(transact(
        Some(&reloader),
        "invalid: [",
        &enabled,
        || {
            committed.set(true);
            Ok(())
        },
        || Ok(())
    )
    .await
    .is_err());
    assert!(!committed.get());
    api.verify_rule_config(&enabled).await.unwrap();
    assert!(transact(
        Some(&reloader),
        &base,
        &enabled,
        || Err(AppError::Io("synthetic disk full".into())),
        || Ok(())
    )
    .await
    .is_err());
    api.verify_rule_config(&enabled).await.unwrap();
    let checks = Cell::new(0);
    assert!(transact(
        Some(&reloader),
        &base,
        &enabled,
        || Ok(()),
        || {
            checks.set(checks.get() + 1);
            if checks.get() > 1 {
                Err(AppError::Conflict("synthetic profile changed".into()))
            } else {
                Ok(())
            }
        }
    )
    .await
    .is_err());
    api.verify_rule_config(&enabled).await.unwrap();
    doc.bump(2).unwrap();
    doc.enabled = false;
    transact(
        Some(&reloader),
        &base,
        &enabled,
        || doc.write(&storage),
        || Ok(()),
    )
    .await
    .unwrap();
    assert!(!Document::read(&storage).unwrap().enabled);
    assert_eq!(
        request(settings.mixed_port, web_port, "sub.route.example.test").await,
        Ok(200)
    );
    // Persisted disabled state remains disabled after a genuine core restart.
    core.child.kill().unwrap();
    core.child.wait().unwrap();
    drop(core);
    let _restarted = start(&binary, dir.path(), &base);
    api.wait_ready(Duration::from_secs(5)).await.unwrap();
    wait_proxy_ready(settings.mixed_port).await;
    api.verify_rule_config(&base).await.unwrap();
    assert_eq!(
        request(settings.mixed_port, web_port, "sub.route.example.test").await,
        Ok(200)
    );
    // Missing destination compiles to REJECT, not silent direct; changing the original document is unnecessary.
    settings.domain_routing.enabled = true;
    settings.domain_routing.applied.as_mut().unwrap().target = "missing".into();
    let missing = build_effective_config_with_policy(source, &settings, RoutingMode::Rule, None)
        .unwrap()
        .yaml;
    transact(Some(&reloader), &missing, &base, || Ok(()), || Ok(()))
        .await
        .unwrap();
    assert_ne!(
        request(settings.mixed_port, web_port, "sub.route.example.test").await,
        Ok(200)
    );
    assert_eq!(
        request(settings.mixed_port, web_port, "route.example.test").await,
        Ok(200)
    );
    server.abort();
}
