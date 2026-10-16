//! Bounded transport recovery. Never changes the user's network or trusts another build.
use super::*;
use std::error::Error;
use std::future::Future;
use std::sync::atomic::AtomicU64;
use tokio::time::Instant;

pub(super) const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
pub(super) const METADATA_TIMEOUT: Duration = Duration::from_secs(25);
#[derive(Clone, Copy)]
pub(super) struct Policy {
    pub idle: Duration,
    pub total: Duration,
    pub backoff: Duration,
    pub attempts: u8,
}
impl Default for Policy {
    fn default() -> Self {
        Self {
            idle: Duration::from_secs(45),
            total: Duration::from_secs(600),
            backoff: Duration::from_secs(1),
            attempts: 2,
        }
    }
}
#[derive(Debug)]
pub(super) struct TransportFailure {
    pub kind: &'static str,
    pub message: String,
    pub detail: String,
    pub retryable: bool,
    pub fatal: bool,
    pub status: Option<u16>,
}
impl TransportFailure {
    fn new(kind: &'static str, message: impl Into<String>, retryable: bool, fatal: bool) -> Self {
        Self {
            kind,
            message: message.into(),
            detail: String::new(),
            retryable,
            fatal,
            status: None,
        }
    }
    pub fn from_updater(error: tauri_plugin_updater::Error) -> Self {
        let mut chain = vec![crate::app_log::sanitize(&error.to_string())];
        let mut next = error.source();
        while let Some(e) = next {
            chain.push(
                crate::app_log::sanitize(&e.to_string())
                    .chars()
                    .take(240)
                    .collect(),
            );
            next = e.source();
            if chain.len() >= 6 {
                break;
            }
        }
        let detail = chain.join(" → ");
        let lower = detail.to_ascii_lowercase();
        let certificate = [
            "certificate",
            "certvalid",
            "unknownissuer",
            "invalidpeer",
            "invalid peer",
            "notvalidforname",
        ]
        .iter()
        .any(|v| lower.contains(v));
        let mut result = match &error {
            tauri_plugin_updater::Error::Reqwest(_) if certificate => Self::new(
                "certificate",
                "下载连接的证书验证未通过；请检查系统时间或网络证书",
                false,
                true,
            ),
            tauri_plugin_updater::Error::Reqwest(e) if e.is_timeout() => {
                Self::new("timeout", "连接或传输超时", true, false)
            }
            tauri_plugin_updater::Error::Reqwest(e)
                if e.is_connect() || e.is_body() || e.is_request() =>
            {
                Self::new("connection", "下载连接暂时中断", true, false)
            }
            tauri_plugin_updater::Error::Network(text) => {
                let status = text
                    .strip_prefix("Download request failed with status: ")
                    .and_then(|s| s.split_whitespace().next())
                    .and_then(|s| s.parse::<u16>().ok());
                let retryable = matches!(status, Some(408 | 429 | 500 | 502 | 503 | 504));
                let mut f = Self::new(
                    "http",
                    status
                        .map(|s| format!("下载服务返回 HTTP {s}"))
                        .unwrap_or_else(|| "下载服务暂不可用".into()),
                    retryable,
                    false,
                );
                f.status = status;
                f
            }
            _ => Self::new(
                "signature",
                "更新文件签名验证未通过；未开始安装",
                false,
                true,
            ),
        };
        result.detail = detail.chars().take(1200).collect();
        result
    }
    fn app_error(&self) -> AppError {
        failure(&self.message)
    }
}

pub(super) async fn download_once(
    candidate: &Candidate,
    transport: &Update,
    state: &AppUpdateManager,
    policy: Policy,
) -> Result<Vec<u8>, TransportFailure> {
    if state.cancelled.load(Ordering::Acquire) {
        return Err(TransportFailure::new(
            "cancelled",
            "下载已取消",
            false,
            true,
        ));
    }
    let oversized = AtomicBool::new(false);
    let received = AtomicU64::new(0);
    let progress = AtomicU64::new(0);
    let started = Instant::now();
    let download = transport.download(
        |length, total| {
            let count = received
                .fetch_add(length as u64, Ordering::Relaxed)
                .saturating_add(length as u64);
            if length > 0 {
                progress.store(started.elapsed().as_millis() as u64, Ordering::Relaxed);
            }
            if count > candidate.size || total.is_some_and(|size| size != candidate.size) {
                oversized.store(true, Ordering::Release);
            }
            if let Ok(mut session) = state.lock() {
                session.status.downloaded_bytes = count;
            }
        },
        || {},
    );
    tokio::pin!(download);
    let bytes = loop {
        tokio::select! {
            biased;
            _ = tokio::time::sleep(Duration::from_millis(50)) => {
                if state.cancelled.load(Ordering::Acquire) { return Err(TransportFailure::new("cancelled", "下载已取消", false, true)); }
                if oversized.load(Ordering::Acquire) { return Err(TransportFailure::new("integrity", "更新包大小与发布清单不符；未开始安装", false, true)); }
                if started.elapsed().saturating_sub(Duration::from_millis(progress.load(Ordering::Relaxed))) >= policy.idle { return Err(TransportFailure::new("idle", "下载暂时没有进展", true, false)); }
            }
            result = &mut download => break result.map_err(TransportFailure::from_updater)?,
        }
    };
    if state.cancelled.load(Ordering::Acquire) {
        return Err(TransportFailure::new(
            "cancelled",
            "下载已取消",
            false,
            true,
        ));
    }
    state
        .lock()
        .map_err(|e| TransportFailure::new("state", e.to_string(), false, true))?
        .status
        .stage = "verify".into();
    if oversized.load(Ordering::Acquire) {
        return Err(TransportFailure::new(
            "integrity",
            "更新包大小与发布清单不符；未开始安装",
            false,
            true,
        ));
    }
    verify_download(candidate, &bytes)
        .map_err(|e| TransportFailure::new("integrity", e.to_string(), false, true))?;
    Ok(bytes)
}

fn record(
    state: &AppUpdateManager,
    source: UpdateSource,
    number: u8,
    stage: &str,
    started: Instant,
    error: Option<&TransportFailure>,
    message: &str,
) -> AppResult<()> {
    let mut session = state.lock()?;
    let attempt = UpdateAttempt {
        source,
        result: message.into(),
        stage: stage.into(),
        number,
        elapsed_ms: started.elapsed().as_millis().min(u64::MAX as u128) as u64,
        received_bytes: session.status.downloaded_bytes,
        error_kind: error.map(|e| e.kind.into()),
        http_status: error.and_then(|e| e.status),
        detail: error.map(|e| e.detail.clone()).filter(|s| !s.is_empty()),
    };
    session.status.error_kind = attempt.error_kind.clone();
    crate::app_log::record(if error.is_some() { 2 } else { 1 }, crate::app_log::Area::App,
        &format!("update task={} source={:?} phase={} attempt={} elapsed_ms={} bytes={} code={} http={:?} {} {}", session.status.task_id, source, stage, number, attempt.elapsed_ms, attempt.received_bytes, attempt.error_kind.as_deref().unwrap_or("ok"), attempt.http_status, message, attempt.detail.as_deref().unwrap_or("")));
    session.status.attempts.push(attempt);
    Ok(())
}

/// The same coordinator drives IPC and fault-injection tests. Discovery may only
/// supply a source for the frozen build, never silently follow a newer latest.
pub(super) async fn recover<F, Fut, T>(
    state: &AppUpdateManager,
    candidates: Vec<Candidate>,
    auto: bool,
    policy: Policy,
    mut discover: F,
    transport: T,
) -> AppResult<AppUpdateStatus>
where
    F: FnMut(UpdateSource) -> Fut,
    Fut: Future<Output = AppResult<Candidate>>,
    T: Fn(&Candidate) -> Update,
{
    let selected = candidates
        .first()
        .cloned()
        .ok_or_else(|| failure("缺少已确认的更新目标，请重新检查"))?;
    let mut sources: Vec<_> = candidates.iter().map(|c| c.source).collect();
    if auto {
        for s in [UpdateSource::Hk, UpdateSource::Gitee, UpdateSource::Github] {
            if !sources.contains(&s) {
                sources.push(s);
            }
        }
    }
    let task = async {
        let mut last = "所有下载来源暂不可用，请稍后重试；当前应用与代理保持运行".to_string();
        for source in sources {
            let candidate = if let Some(c) = candidates.iter().find(|c| c.source == source) {
                c.clone()
            } else {
                {
                    let mut session = state.lock()?;
                    session.status.stage = "recovering".into();
                    session.status.message =
                        Some(format!("正在重新确认 {} 的同版本备用包", source.label()));
                    session.status.attempt = 0;
                    session.status.downloaded_bytes = 0;
                }
                let start = Instant::now();
                match discover(source).await {
                    Ok(c) if same_build(&selected, &c) => {
                        if let Some(info) = &mut state.lock()?.status.info {
                            if let Some(channel) = info
                                .channels
                                .iter_mut()
                                .find(|channel| channel.source == source)
                            {
                                channel.version = Some(format!("v{}", c.update.version));
                                channel.error = None;
                            } else {
                                info.channels.push(UpdateChannelStatus {
                                    source,
                                    version: Some(format!("v{}", c.update.version)),
                                    error: None,
                                });
                            }
                        }
                        record(
                            state,
                            source,
                            0,
                            "recovering",
                            start,
                            None,
                            "已确认同版本、同平台和同内容的备用包",
                        )?;
                        c
                    }
                    Ok(_) => {
                        let e = TransportFailure::new(
                            "identity",
                            "备用源版本、架构或内容不一致，已排除",
                            false,
                            false,
                        );
                        record(state, source, 0, "recovering", start, Some(&e), &e.message)?;
                        continue;
                    }
                    Err(e) => {
                        let mut error = TransportFailure::new(
                            "metadata",
                            "此备用源暂不可用，已跳过",
                            false,
                            false,
                        );
                        error.detail = crate::app_log::sanitize(&e.to_string());
                        record(
                            state,
                            source,
                            0,
                            "recovering",
                            start,
                            Some(&error),
                            &error.message,
                        )?;
                        continue;
                    }
                }
            };
            for number in 1..=policy.attempts {
                let started = Instant::now();
                {
                    let mut session = state.lock()?;
                    session.status.stage = "download".into();
                    session.status.message = Some(format!(
                        "正在从 {} 下载（第 {number}/{} 次）",
                        source.label(),
                        policy.attempts
                    ));
                    session.status.attempt = number;
                    session.status.max_attempts = policy.attempts;
                    session.status.downloaded_bytes = 0;
                    session.status.total_bytes = candidate.size;
                    if let Some(info) = &mut session.status.info {
                        info.source = source;
                        info.release_url = official_release_url(
                            source,
                            &format!("v{}", candidate.update.version),
                        )?;
                    }
                }
                let transport = transport(&candidate);
                match download_once(&candidate, &transport, state, policy).await {
                    Ok(bytes) => {
                        record(
                            state,
                            source,
                            number,
                            "verified",
                            started,
                            None,
                            "更新签名与 SHA-256 验证通过",
                        )?;
                        let mut session = state.lock()?;
                        // Installation must compare against this recovered candidate too.
                        session.candidates = vec![candidate.clone()];
                        session.ready = Some((candidate, bytes));
                        session.status.phase = "ready".into();
                        session.status.stage = "verified".into();
                        session.status.message = None;
                        session.status.error_kind = None;
                        return Ok(session.status.clone());
                    }
                    Err(e) => {
                        let stage = if matches!(e.kind, "signature" | "integrity") {
                            "verify"
                        } else {
                            "download"
                        };
                        state.lock()?.status.stage = stage.into();
                        record(state, source, number, stage, started, Some(&e), &e.message)?;
                        last = format!(
                            "{}：{}；尚未开始安装，当前应用与代理保持运行",
                            source.label(),
                            e.message
                        );
                        if e.fatal {
                            return Err(e.app_error());
                        }
                        if !e.retryable || number == policy.attempts {
                            break;
                        }
                        {
                            let mut session = state.lock()?;
                            session.status.stage = "retrying".into();
                            session.status.message = Some(format!(
                                "{}，即将重试 {}（{}/{}）",
                                e.message,
                                source.label(),
                                number + 1,
                                policy.attempts
                            ));
                        }
                        tokio::time::sleep(policy.backoff * u32::from(number)).await;
                    }
                }
            }
        }
        Err(failure(last))
    };
    tokio::pin!(task);
    let total = tokio::time::sleep(policy.total);
    tokio::pin!(total);
    loop {
        tokio::select! {
            biased;
            _ = tokio::time::sleep(Duration::from_millis(50)) => {
                if state.cancelled.load(Ordering::Acquire) { return Err(failure("下载已取消；当前应用与代理保持运行")); }
            }
            _ = &mut total => { state.lock()?.status.error_kind = Some("budget".into()); return Err(failure("本次下载等待时间已到，请稍后重试；当前应用与代理保持运行")); }
            result = &mut task => return result,
        }
    }
}
