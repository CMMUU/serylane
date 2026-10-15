//! Real product compiler, transaction coordinator, storage and bundled core.
//! All ports, files and traffic are loopback-only, independent of the installed app.
use super::*;
use crate::{
    mihomo_api::MihomoApiClient,
    user_rules::{transact, ProfileReloader},
};
use std::{
    cell::Cell,
    net::TcpListener,
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::Duration,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

struct Core(Child);
impl Drop for Core {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
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
fn port() -> TcpListener {
    TcpListener::bind("127.0.0.1:0").unwrap()
}
fn start(binary: &std::path::Path, dir: &std::path::Path, config: &str) -> Core {
    let path = dir.join("isolated.yaml");
    std::fs::write(&path, config).unwrap();
    crate::runtime::validate_file(binary, dir, &path).unwrap();
    Core(
        Command::new(binary)
            .arg("-d")
            .arg(dir)
            .arg("-f")
            .arg(path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    )
}
async fn request(mixed: u16, web: u16, host: &str) -> bool {
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
        .is_ok_and(|r| r.status() == 200)
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
        while let Ok((mut s, _)) = web.accept().await {
            tokio::spawn(async move {
                let mut b = [0; 4096];
                let _ = s.read(&mut b).await;
                let _ = s
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
                    )
                    .await;
            });
        }
    });
    let mut settings = crate::models::AppSettings {
        mixed_port: mixed.local_addr().unwrap().port(),
        controller_port: control.local_addr().unwrap().port(),
        ..Default::default()
    };
    let source="dns: {enable: false}\nlog-level: silent\nhosts: {route.example.test: 127.0.0.1, sub.route.example.test: 127.0.0.1, deep.sub.route.example.test: 127.0.0.1, fixed.example.test: 127.0.0.1, notroute.example.test: 127.0.0.1}\nproxies: []\nproxy-groups: [{name: Exit, type: select, proxies: [REJECT]}]\nrules: ['MATCH,DIRECT']";
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
    drop(mixed);
    drop(control);
    let mut core = start(&binary, dir.path(), &base);
    let api = MihomoApiClient::new(&settings).unwrap();
    api.wait_ready(Duration::from_secs(5)).await.unwrap();
    api.verify_rule_config(&base).await.unwrap();
    assert!(!Document::read(&storage).unwrap().enabled);
    assert!(request(settings.mixed_port, web_port, "sub.route.example.test").await);
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
            !request(settings.mixed_port, web_port, host).await,
            "matched {host}"
        );
    }
    for host in ["route.example.test", "notroute.example.test"] {
        assert!(
            request(settings.mixed_port, web_port, host).await,
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
    assert!(request(settings.mixed_port, web_port, "sub.route.example.test").await);
    // Persisted disabled state remains disabled after a genuine core restart.
    core.0.kill().unwrap();
    core.0.wait().unwrap();
    drop(core);
    let _restarted = start(&binary, dir.path(), &base);
    api.wait_ready(Duration::from_secs(5)).await.unwrap();
    api.verify_rule_config(&base).await.unwrap();
    assert!(request(settings.mixed_port, web_port, "sub.route.example.test").await);
    // Missing destination compiles to REJECT, not silent direct; changing the original document is unnecessary.
    settings.domain_routing.enabled = true;
    settings.domain_routing.applied.as_mut().unwrap().target = "missing".into();
    let missing = build_effective_config_with_policy(source, &settings, RoutingMode::Rule, None)
        .unwrap()
        .yaml;
    transact(Some(&reloader), &missing, &base, || Ok(()), || Ok(()))
        .await
        .unwrap();
    assert!(!request(settings.mixed_port, web_port, "sub.route.example.test").await);
    assert!(request(settings.mixed_port, web_port, "route.example.test").await);
    server.abort();
}
