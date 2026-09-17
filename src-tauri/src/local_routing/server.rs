//! Loopback-only, fixed-upstream Responses relay. No provider switching or request replay.
use super::{RouteDocument, RouteMode, RouteStats};
use crate::error::{AppError, AppResult};
use crate::openai_stability::{Evidence, Observation, StabilityManager};
use crate::route_health::{Diagnostic, Failure, Target};
use bytes::Bytes;
use futures_util::{stream, StreamExt};
use http_body_util::{combinators::UnsyncBoxBody, BodyExt, Full, Limited, StreamBody};
use hyper::{
    body::{Frame, Incoming},
    header,
    server::conn::http1,
    service::service_fn,
    Method, Request, Response, StatusCode,
};
use hyper_util::rt::{TokioIo, TokioTimer};
use std::{
    convert::Infallible,
    net::{Ipv4Addr, TcpListener},
    sync::{atomic::Ordering, Arc},
    time::{Duration, Instant},
};
use tauri::Manager;
use tokio::sync::{watch, OwnedSemaphorePermit, Semaphore};

type BoxError = Box<dyn std::error::Error + Send + Sync>;
type Body = UnsyncBoxBody<Bytes, BoxError>;
const MAX_BODY: usize = 32 * 1024 * 1024;
const FIRST_RESPONSE_TIMEOUT: Duration = Duration::from_secs(60);
const STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(300);

pub struct RunningServer {
    stop: watch::Sender<bool>,
    pub stats: Arc<RouteStats>,
}

impl RunningServer {
    pub fn stop(&self) {
        let _ = self.stop.send(true);
    }
}

impl Drop for RunningServer {
    fn drop(&mut self) {
        self.stop();
    }
}

struct Relay {
    client: reqwest::Client,
    upstream: String,
    prefix: String,
    port: u16,
    stats: Arc<RouteStats>,
    requests: Arc<Semaphore>,
    mode: RouteMode,
    app: Option<tauri::AppHandle>,
    default_proxy: bool,
    stop: watch::Receiver<bool>,
    target: Target,
    proxy: Option<String>,
}

pub fn start(
    document: &RouteDocument,
    mixed_port: u16,
    app: Option<tauri::AppHandle>,
) -> AppResult<RunningServer> {
    let proxy = document.proxy_url(mixed_port)?;
    start_with_app(
        document,
        document.settings.upstream.base_url().into(),
        Some(proxy),
        app,
    )
}

#[cfg(test)]
fn start_inner(
    document: &RouteDocument,
    upstream: String,
    proxy: Option<String>,
) -> AppResult<RunningServer> {
    start_with_app(document, upstream, proxy, None)
}
fn start_with_app(
    document: &RouteDocument,
    upstream: String,
    proxy: Option<String>,
    app: Option<tauri::AppHandle>,
) -> AppResult<RunningServer> {
    let mut builder = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .http1_only()
        .connect_timeout(Duration::from_secs(10))
        .tcp_keepalive(Duration::from_secs(60))
        // A new model request must not reuse a tunnel pinned to a failed old node.
        .pool_max_idle_per_host(0)
        .no_gzip()
        .no_brotli()
        .no_deflate()
        .no_zstd();
    if let Some(proxy) = &proxy {
        builder = builder.proxy(
            reqwest::Proxy::all(proxy)
                .map_err(|_| AppError::InvalidInput("出站代理地址无效".into()))?,
        );
    }
    let client = builder
        .build()
        .map_err(|_| AppError::Runtime("无法初始化路由出站客户端".into()))?;
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, document.settings.listen_port))
        .map_err(|_| {
            AppError::Conflict("路由端口无法监听或已被占用；未停止占用端口的程序".into())
        })?;
    listener.set_nonblocking(true)?;
    let port = listener.local_addr()?.port();
    let stats = Arc::new(RouteStats::default());
    stats.running.store(true, Ordering::Release);
    stats.accepting.store(true, Ordering::SeqCst);
    let (stop, mut receiver) = watch::channel(false);
    let relay = Arc::new(Relay {
        client,
        upstream,
        prefix: format!("/rd/{}/v1", document.access_token),
        port,
        stats: stats.clone(),
        requests: Arc::new(Semaphore::new(8)),
        mode: document.settings.mode,
        app,
        default_proxy: document.settings.outbound_proxy.is_empty(),
        stop: receiver.clone(),
        target: document.settings.upstream.target(),
        proxy,
    });
    tauri::async_runtime::spawn(async move {
        struct RunningGuard(Arc<RouteStats>);
        impl Drop for RunningGuard {
            fn drop(&mut self) {
                self.0.running.store(false, Ordering::Release);
            }
        }
        let _running = RunningGuard(relay.stats.clone());
        let listener = match tokio::net::TcpListener::from_std(listener) {
            Ok(listener) => listener,
            Err(_) => {
                relay.stats.error("本地监听器启动失败");
                return;
            }
        };
        let sockets = Arc::new(Semaphore::new(32));
        let mut tasks = tokio::task::JoinSet::new();
        loop {
            tokio::select! {
                _ = receiver.changed() => break,
                Some(_) = tasks.join_next(), if !tasks.is_empty() => {},
                accepted = listener.accept() => {
                    let Ok((socket, address)) = accepted else {
                        relay.stats.error("本地监听器接受连接失败");
                        break;
                    };
                    if !address.ip().is_loopback() { continue; }
                    let Ok(permit) = sockets.clone().try_acquire_owned() else { continue; };
                    let relay = relay.clone();
                    tasks.spawn(async move {
                        let _permit = permit;
                        let _ = socket.set_nodelay(true);
                        let service = service_fn(move |request| handle(request, relay.clone()));
                        let _ = http1::Builder::new()
                            .timer(TokioTimer::new())
                            .header_read_timeout(Duration::from_secs(10))
                            .max_buf_size(32 * 1024)
                            .serve_connection(TokioIo::new(socket), service).with_upgrades().await;
                    });
                }
            }
        }
        // Explicit stop/exit cancels only this service's connections, never an application process.
        tasks.shutdown().await;
    });
    Ok(RunningServer { stop, stats })
}

fn json(status: StatusCode, code: &'static str) -> Response<Body> {
    let text =
        serde_json::json!({"error": {"type": "routedeck_routing", "code": code}}).to_string();
    let body = Full::new(Bytes::from(text))
        .map_err(|never: Infallible| match never {})
        .boxed_unsync();
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::CACHE_CONTROL, "no-store")
        .body(body)
        .expect("static response headers")
}

fn allowed_endpoint(method: &Method, suffix: &str) -> bool {
    matches!(
        (method, suffix),
        (
            &Method::POST,
            "/responses" | "/responses/compact" | "/responses/input_tokens"
        ) | (&Method::GET, "/models")
    )
}

fn safe_header(name: &str, connection_tokens: &[String]) -> bool {
    !matches!(
        name,
        "host"
            | "connection"
            | "proxy-connection"
            | "proxy-authorization"
            | "proxy-authenticate"
            | "keep-alive"
            | "transfer-encoding"
            | "te"
            | "trailer"
            | "upgrade"
            | "content-length"
            | "cookie"
            | "set-cookie"
            | "origin"
            | "referer"
            | "location"
    ) && !name.starts_with("sec-")
        && !name.starts_with("access-control-")
        && !connection_tokens.iter().any(|token| token == name)
}

fn connection_tokens(headers: &header::HeaderMap) -> Vec<String> {
    headers
        .get_all(header::CONNECTION)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(',').map(|v| v.trim().to_ascii_lowercase()))
        .collect()
}

struct RequestGuard {
    stats: Arc<RouteStats>,
    _permit: OwnedSemaphorePermit,
    finished: bool,
    observation: Option<Observation>,
    relay: Arc<Relay>,
    started: Instant,
}
impl RequestGuard {
    fn diagnose(&self, failure: Failure, stage: &'static str, status: Option<u16>) {
        self.stats.diagnostic(Diagnostic::new(
            failure,
            self.relay.target,
            stage,
            self.started.elapsed().as_millis() as u64,
            status,
            self.observation.is_some(),
        ));
        // A hint only wakes bounded probes. Never invent per-node evidence when
        // connection metadata was unavailable (including TLS/CONNECT failures).
        if failure.warrants_check() && self.relay.default_proxy {
            if let Some(app) = &self.relay.app {
                app.state::<StabilityManager>()
                    .request_check(self.relay.target);
            }
        }
    }
    fn evidence(&mut self, event: Evidence) {
        if let Some(o) = self.observation.take() {
            o.finish(event);
        }
    }
    fn finish(&mut self, success: bool) {
        if !self.finished {
            if success {
                self.stats.completed.fetch_add(1, Ordering::Relaxed);
            } else {
                self.stats.failed.fetch_add(1, Ordering::Relaxed);
            }
            self.finished = true;
        }
    }
}
impl Drop for RequestGuard {
    fn drop(&mut self) {
        // A disconnected downstream drops the upstream stream and releases its slot.
        self.finish(false);
        self.stats.active.fetch_sub(1, Ordering::Relaxed);
    }
}

async fn handle(
    mut request: Request<Incoming>,
    relay: Arc<Relay>,
) -> Result<Response<Body>, Infallible> {
    let host = request
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok());
    if host != Some(format!("127.0.0.1:{}", relay.port).as_str())
        || request.headers().contains_key(header::ORIGIN)
        || request.uri().scheme().is_some()
    {
        return Ok(json(StatusCode::FORBIDDEN, "local_client_required"));
    }
    let Some(suffix) = request.uri().path().strip_prefix(&relay.prefix) else {
        return Ok(json(StatusCode::NOT_FOUND, "route_not_found"));
    };
    let websocket = request
        .headers()
        .get(header::UPGRADE)
        .is_some_and(|v| v.as_bytes().eq_ignore_ascii_case(b"websocket"));
    if request.headers().contains_key(header::UPGRADE) && relay.mode != RouteMode::Native {
        return Ok(json(
            StatusCode::UPGRADE_REQUIRED,
            "use_http_streaming_compatibility_mode",
        ));
    }
    if !allowed_endpoint(request.method(), suffix)
        && !(websocket && request.method() == Method::GET && suffix == "/responses")
    {
        return Ok(json(StatusCode::NOT_FOUND, "endpoint_not_supported"));
    }
    if !request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("Bearer ") && v.len() > 7)
    {
        return Ok(json(
            StatusCode::UNAUTHORIZED,
            "codex_authentication_required",
        ));
    }
    let query = request.uri().query().unwrap_or("");
    if query.len() > 8192 {
        return Ok(json(StatusCode::URI_TOO_LONG, "query_too_large"));
    }
    let target = format!(
        "{}{}{}{}",
        relay.upstream,
        suffix,
        if query.is_empty() { "" } else { "?" },
        query
    );
    let model_request = suffix == "/responses";
    let upgrade = websocket.then(|| hyper::upgrade::on(&mut request));
    let Ok(permit) = relay.requests.clone().try_acquire_owned() else {
        return Ok(json(StatusCode::SERVICE_UNAVAILABLE, "local_request_limit"));
    };
    relay.stats.requests.fetch_add(1, Ordering::Relaxed);
    relay.stats.active.fetch_add(1, Ordering::SeqCst);
    let observation = if relay.default_proxy && model_request {
        relay
            .app
            .as_ref()
            .and_then(|app| app.state::<StabilityManager>().observe(relay.target))
    } else {
        None
    };
    let mut guard = RequestGuard {
        stats: relay.stats.clone(),
        _permit: permit,
        finished: false,
        observation: None,
        relay: relay.clone(),
        started: Instant::now(),
    };
    if !relay.stats.accepting.load(Ordering::SeqCst) {
        return Ok(json(StatusCode::SERVICE_UNAVAILABLE, "route_stopping"));
    }
    let (parts, body) = request.into_parts();
    let body = match tokio::time::timeout(
        Duration::from_secs(30),
        Limited::new(body, MAX_BODY).collect(),
    )
    .await
    {
        Ok(Ok(body)) => body.to_bytes(),
        Ok(Err(_)) => {
            relay.stats.error("请求正文超过限制或读取失败");
            return Ok(json(StatusCode::PAYLOAD_TOO_LARGE, "request_body_rejected"));
        }
        Err(_) => {
            relay.stats.error("读取本地请求超时");
            return Ok(json(StatusCode::REQUEST_TIMEOUT, "request_body_timeout"));
        }
    };
    let mut outgoing = relay.client.request(parts.method, target);
    let hop_headers = connection_tokens(&parts.headers);
    for (name, value) in &parts.headers {
        if name != header::ACCEPT_ENCODING && safe_header(name.as_str(), &hop_headers) {
            outgoing = outgoing.header(name, value);
        }
    }
    // Keep SSE event inspection meaningful without changing response bytes or buffering.
    outgoing = outgoing.header(header::ACCEPT_ENCODING, "identity");
    if websocket {
        if parts
            .headers
            .get("sec-websocket-version")
            .and_then(|v| v.to_str().ok())
            != Some("13")
            || parts.headers.get("sec-websocket-key").is_none()
        {
            return Ok(json(StatusCode::BAD_REQUEST, "invalid_websocket_handshake"));
        }
        outgoing = outgoing
            .header(header::CONNECTION, "upgrade")
            .header(header::UPGRADE, "websocket");
        for name in [
            "sec-websocket-key",
            "sec-websocket-version",
            "sec-websocket-protocol",
            "sec-websocket-extensions",
        ] {
            if let Some(value) = parts.headers.get(name) {
                outgoing = outgoing.header(name, value);
            }
        }
    }
    // Never replay a POST: even an error before the first response can follow upstream execution.
    let upstream =
        match tokio::time::timeout(FIRST_RESPONSE_TIMEOUT, outgoing.body(body).send()).await {
            Ok(Ok(response)) => response,
            Ok(Err(error)) => {
                let mut failure = Failure::from_reqwest(&error);
                if error.is_connect() && !local_proxy_available(relay.proxy.as_deref()).await {
                    failure = Failure::LocalProxyUnavailable;
                }
                guard.diagnose(failure, "request", None);
                relay.stats.last_status.store(502, Ordering::Relaxed);
                return Ok(json(StatusCode::BAD_GATEWAY, failure.code()));
            }
            Err(_) => {
                guard.diagnose(Failure::ResponseTimeout, "response_headers", None);
                relay.stats.last_status.store(504, Ordering::Relaxed);
                return Ok(json(
                    StatusCode::GATEWAY_TIMEOUT,
                    "upstream_response_timeout",
                ));
            }
        };
    let status = upstream.status();
    // Attribute only when the actual TCP source port and controller chain agree.
    // A manual rule/global mode/another outbound must not punish the selected node.
    // Missing metadata or a slow controller skips evidence; forwarding continues.
    if status.is_success() {
        if let (Some(observation), Some(app), Some(info)) = (
            observation,
            relay.app.as_ref(),
            upstream
                .extensions()
                .get::<hyper_util::client::legacy::connect::HttpInfo>(),
        ) {
            let settings = crate::storage::AppStorage::from_app(app).and_then(|s| s.settings());
            if let Ok(settings) = settings {
                if info.remote_addr()
                    == std::net::SocketAddr::from((Ipv4Addr::LOCALHOST, settings.mixed_port))
                {
                    if let Ok(api) = crate::mihomo_api::MihomoApiClient::new(&settings) {
                        if let Ok(Ok(connections)) =
                            tokio::time::timeout(Duration::from_millis(250), api.connections())
                                .await
                        {
                            let host = url::Url::parse(&relay.upstream)
                                .ok()
                                .and_then(|u| u.host_str().map(str::to_string));
                            if host.is_some_and(|host| {
                                observation.matches_connection(
                                    &connections,
                                    info.local_addr(),
                                    &host,
                                )
                            }) {
                                guard.observation = Some(observation);
                            }
                        }
                    }
                }
            }
        }
    }
    relay
        .stats
        .last_status
        .store(status.as_u16() as u64, Ordering::Relaxed);
    if websocket && status == StatusCode::SWITCHING_PROTOCOLS {
        let mut response = Response::builder()
            .status(StatusCode::SWITCHING_PROTOCOLS)
            .header(header::CONNECTION, "upgrade")
            .header(header::UPGRADE, "websocket");
        for name in [
            "sec-websocket-accept",
            "sec-websocket-protocol",
            "sec-websocket-extensions",
        ] {
            if let Some(value) = upstream.headers().get(name) {
                response = response.header(name, value);
            }
        }
        let mut stop = relay.stop.clone();
        tauri::async_runtime::spawn(async move {
            let mut guard = guard;
            let work = async {
                let downstream = upgrade.expect("websocket upgrade").await.map_err(|_| ())?;
                let mut downstream = TokioIo::new(downstream);
                let mut upstream = upstream.upgrade().await.map_err(|_| ())?;
                tokio::io::copy_bidirectional(&mut downstream, &mut upstream)
                    .await
                    .map_err(|_| ())
            };
            tokio::select! {
                result = work => {
                    // Raw WS frames are passed through, not interpreted or called "model verified".
                    if result.is_err() { guard.stats.error("WebSocket 隧道关闭或中断；未自动重放请求"); }
                    guard.finish(result.is_ok());
                }
                _ = stop.changed() => {}
            }
        });
        return Ok(response
            .body(
                Full::new(Bytes::new())
                    .map_err(|never: Infallible| match never {})
                    .boxed_unsync(),
            )
            .expect("websocket response"));
    }
    if status.is_redirection() {
        relay
            .stats
            .error("上游重定向被拒绝，未向其他站点转发认证信息");
        return Ok(json(StatusCode::BAD_GATEWAY, "upstream_redirect_rejected"));
    }
    if !status.is_success() {
        guard.diagnose(
            Failure::from_status(status.as_u16()),
            "response_headers",
            Some(status.as_u16()),
        );
    }
    let mut response = Response::builder().status(status);
    let hop_headers = connection_tokens(upstream.headers());
    for (name, value) in upstream.headers() {
        if safe_header(name.as_str(), &hop_headers) {
            response = response.header(name, value);
        }
    }
    response = response.header(header::CACHE_CONTROL, "no-store");
    let sse = model_request
        && status.is_success()
        && upstream
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.starts_with("text/event-stream"))
        && upstream
            .headers()
            .get(header::CONTENT_ENCODING)
            .is_none_or(|v| v.as_bytes().eq_ignore_ascii_case(b"identity"));
    let stream = stream::unfold(
        (
            upstream.bytes_stream(),
            guard,
            false,
            SseCompletion::default(),
        ),
        move |(mut source, mut guard, done, mut tracker)| async move {
            if done {
                return None;
            }
            match tokio::time::timeout(STREAM_IDLE_TIMEOUT, source.next()).await {
                Ok(Some(Ok(bytes))) => {
                    if sse {
                        let was_failed = tracker.failed;
                        tracker.feed(&bytes);
                        if tracker.failed && !was_failed {
                            guard.diagnose(
                                Failure::ModelRejected,
                                "stream_event",
                                Some(status.as_u16()),
                            );
                        }
                        // Codex may stop reading immediately after this event, without EOF.
                        // Count the confirmed terminal event once, not a subsequent cancellation.
                        if tracker.completed {
                            guard.evidence(Evidence::ModelComplete);
                            guard.finish(true);
                        }
                    }
                    Some((
                        Ok::<_, BoxError>(Frame::data(bytes)),
                        (source, guard, false, tracker),
                    ))
                }
                Ok(None) => {
                    if sse {
                        if tracker.completed {
                            guard.evidence(Evidence::ModelComplete);
                        } else if !tracker.failed && !tracker.unverified {
                            guard.diagnose(
                                Failure::StreamInterrupted,
                                "stream",
                                Some(status.as_u16()),
                            );
                            guard.evidence(Evidence::ModelInterrupted);
                            guard
                                .stats
                                .error("模型流未收到完成事件，连接已结束；未重放请求");
                        }
                    }
                    guard.finish(status.is_success() && (!sse || tracker.completed));
                    None
                }
                result => {
                    if !tracker.completed && !tracker.failed {
                        guard.diagnose(
                            if result.is_err() {
                                Failure::StreamIdle
                            } else {
                                Failure::StreamInterrupted
                            },
                            "stream",
                            Some(status.as_u16()),
                        );
                    }
                    if sse && !tracker.completed && !tracker.failed {
                        guard.evidence(Evidence::ModelInterrupted);
                    }
                    guard.finish(false);
                    Some((
                        Err(Box::new(std::io::Error::new(
                            std::io::ErrorKind::ConnectionAborted,
                            "upstream stream ended",
                        )) as BoxError),
                        (source, guard, true, tracker),
                    ))
                }
            }
        },
    );
    Ok(response
        .body(StreamBody::new(stream).boxed_unsync())
        .expect("validated upstream headers"))
}

async fn local_proxy_available(proxy: Option<&str>) -> bool {
    let Some(url) = proxy.and_then(|p| url::Url::parse(p).ok()) else {
        return true;
    };
    let Some(port) = url.port() else { return true };
    // Inspect only the configured loopback proxy, never the remote target.
    let host = url.host_str().unwrap_or("");
    let address = match host {
        "127.0.0.1" => std::net::SocketAddr::from(([127, 0, 0, 1], port)),
        "::1" | "[::1]" => std::net::SocketAddr::from((std::net::Ipv6Addr::LOCALHOST, port)),
        // localhost may be either family. Do not mislabel a v6 service as down.
        _ => return true,
    };
    matches!(
        tokio::time::timeout(
            Duration::from_millis(500),
            tokio::net::TcpStream::connect(address)
        )
        .await,
        Ok(Ok(_))
    )
}

/// Bounded event-name parser; never stores a prompt/response body or invents completion.
#[derive(Default)]
struct SseCompletion {
    line: Vec<u8>,
    overflow: bool,
    event: String,
    completed: bool,
    failed: bool,
    unverified: bool,
}
// Read only a top-level type from a bounded prefix; never search inside model text.
fn event_type_prefix(text: &str) -> Option<String> {
    use serde::de::{IgnoredAny, MapAccess, Visitor};
    struct TypeVisitor<'a>(&'a mut Option<String>);
    impl<'de> Visitor<'de> for TypeVisitor<'_> {
        type Value = ();
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("event object")
        }
        fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
            while let Some(key) = map.next_key::<String>()? {
                if key == "type" {
                    *self.0 = Some(map.next_value::<String>()?);
                    // Intentionally stop before the (potentially huge) response payload.
                    return Err(serde::de::Error::custom("type inspected"));
                }
                map.next_value::<IgnoredAny>()?;
            }
            Ok(())
        }
    }
    let mut kind = None;
    let _ = serde::Deserializer::deserialize_map(
        &mut serde_json::Deserializer::from_str(text),
        TypeVisitor(&mut kind),
    );
    kind
}
impl SseCompletion {
    fn feed(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            if byte != b'\n' {
                if self.line.len() < 1024 && !self.overflow {
                    self.line.push(byte);
                } else {
                    self.overflow = true;
                }
                continue;
            }
            if self.overflow {
                if self.event.is_empty() {
                    let line = String::from_utf8_lossy(&self.line);
                    if let Some(kind) = line.strip_prefix("data:").and_then(event_type_prefix) {
                        self.event = kind;
                    } else {
                        // Unknown oversized data cannot establish success OR a missing terminal event.
                        self.unverified = true;
                    }
                }
            } else {
                let line = String::from_utf8_lossy(&self.line);
                let line = line.trim_end_matches('\r');
                if line.is_empty() {
                    self.completed |= self.event == "response.completed";
                    self.failed |= matches!(
                        self.event.as_str(),
                        "response.failed" | "response.incomplete" | "error"
                    );
                    self.event.clear();
                } else if let Some(event) = line.strip_prefix("event:") {
                    self.event = event.trim().to_string();
                } else if let Some(data) = line.strip_prefix("data:") {
                    if let Ok(value) = serde_json::from_str::<serde_json::Value>(data) {
                        if let Some(kind) = value["type"].as_str() {
                            self.event = kind.into();
                        }
                    }
                }
            }
            self.line.clear();
            self.overflow = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[test]
    fn completion_requires_a_complete_event_not_just_http_200() {
        let mut parser = SseCompletion::default();
        parser.feed(b"event: response.comp");
        parser.feed(b"leted\r\ndata: {}\r\n");
        assert!(!parser.completed);
        parser.feed(b"\r\n");
        assert!(parser.completed);
        let mut parser = SseCompletion::default();
        parser.feed(b"data: {\"type\":\"response.completed\"}\n\n");
        assert!(parser.completed);
        let mut parser = SseCompletion::default();
        let big = format!(
            "data: {{\"type\":\"response.completed\",\"response\":\"{}\"}}\n\n",
            "x".repeat(20_000)
        );
        for chunk in big.as_bytes().chunks(123) {
            parser.feed(chunk);
        }
        assert!(parser.completed);
        assert!(!parser.unverified);
        assert_eq!(
            event_type_prefix(
                "{\"text\":\"response.completed\",\"type\":\"response.output_text.delta\"}"
            )
            .as_deref(),
            Some("response.output_text.delta")
        );
        let mut parser = SseCompletion::default();
        parser.feed(
            format!(
                "data: {{\"response\":\"{}\",\"type\":\"response.completed\"}}\n\n",
                "x".repeat(20_000)
            )
            .as_bytes(),
        );
        assert!(parser.unverified);
        assert!(!parser.completed);
        let mut parser = SseCompletion::default();
        parser.feed(&vec![b'x'; 10_000]);
        assert!(parser.line.len() <= 1024);
        parser.feed(b"\nevent: response.failed\n\n");
        assert!(parser.failed);
        assert!(!parser.completed);
    }

    #[tokio::test]
    async fn terminal_event_counts_before_eof_even_if_client_then_stops_reading() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut input = [0; 8192];
            let _ = socket.read(&mut input).await.unwrap();
            let terminal = "event: response.completed\ndata: {}\n\n";
            let reply = format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n{:x}\r\n{}\r\n", terminal.len(), terminal);
            socket.write_all(reply.as_bytes()).await.unwrap();
            let _ = socket.read(&mut input).await;
        });
        let doc = document(free_port());
        let server = start_inner(&doc, upstream, None).unwrap();
        let response = reqwest::Client::builder()
            .no_proxy()
            .build()
            .unwrap()
            .post(endpoint(&doc, "/responses"))
            .bearer_auth("fixture")
            .send()
            .await
            .unwrap();
        let mut stream = response.bytes_stream();
        assert!(stream.next().await.unwrap().is_ok());
        assert_eq!(server.stats.completed.load(Ordering::Relaxed), 1);
        drop(stream);
        tokio::time::timeout(Duration::from_secs(3), task)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(server.stats.failed.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn explicit_outbound_proxy_is_used_without_replaying_or_leaking_hop_headers() {
        let (proxy, task) =
            fixture("HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}").await;
        let doc = document(free_port());
        let _server = start_inner(
            &doc,
            "http://official-fixture.invalid/v1".into(),
            Some(proxy),
        )
        .unwrap();
        let response = reqwest::Client::builder()
            .no_proxy()
            .build()
            .unwrap()
            .post(endpoint(&doc, "/responses?fixture=1"))
            .bearer_auth("fixture-only")
            .header("proxy-authorization", "do-not-forward")
            .header("cookie", "do-not-forward")
            .header("Connection", "x-hop")
            .header("x-hop", "do-not-forward")
            .header("Accept-Encoding", "gzip")
            .body("{\"input\":\"test\"}")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let request = task.await.unwrap();
        assert!(request.starts_with("POST http://official-fixture.invalid/v1/responses?fixture=1"));
        assert!(request.contains("Bearer fixture-only"));
        assert!(!request.contains("do-not-forward"));
        assert!(request.contains("accept-encoding: identity"));
        assert!(request.ends_with("{\"input\":\"test\"}"));
    }

    #[tokio::test]
    async fn native_mode_tunnels_bytes_in_both_directions_and_stop_releases_listener() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream = format!("http://{}", listener.local_addr().unwrap());
        let (done, wait) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut all = Vec::new();
            loop {
                let mut b = [0; 1024];
                let n = socket.read(&mut b).await.unwrap();
                all.extend_from_slice(&b[..n]);
                if all.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            let request = String::from_utf8_lossy(&all);
            assert!(request.to_ascii_lowercase().contains("upgrade: websocket"));
            socket.write_all(b"HTTP/1.1 101 Switching Protocols\r\nConnection: upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Accept: fixture\r\n\r\n").await.unwrap();
            let mut b = [0; 4];
            socket.read_exact(&mut b).await.unwrap();
            assert_eq!(&b, b"ping");
            socket.write_all(b"pong").await.unwrap();
            let _ = done.send(());
        });
        let mut doc = document(free_port());
        doc.settings.mode = RouteMode::Native;
        let server = start_inner(&doc, upstream, None).unwrap();
        let response = reqwest::Client::builder()
            .no_proxy()
            .build()
            .unwrap()
            .get(endpoint(&doc, "/responses"))
            .bearer_auth("fixture")
            .header("Connection", "upgrade")
            .header("Upgrade", "websocket")
            .header("Sec-WebSocket-Version", "13")
            .header("Sec-WebSocket-Key", "fixture")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 101);
        let mut upgraded = response.upgrade().await.unwrap();
        upgraded.write_all(b"ping").await.unwrap();
        let mut bytes = [0; 4];
        tokio::time::timeout(Duration::from_secs(3), upgraded.read_exact(&mut bytes))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&bytes, b"pong");
        wait.await.unwrap();
        task.await.unwrap();
        drop(upgraded);
        server.stop();
        tokio::time::timeout(Duration::from_secs(3), async {
            while server.stats.running.load(Ordering::Acquire) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let _listener = TcpListener::bind((Ipv4Addr::LOCALHOST, doc.settings.listen_port)).unwrap();
    }

    #[tokio::test]
    async fn abrupt_sse_end_is_not_reported_as_success_or_replayed() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream = format!("http://{}", listener.local_addr().unwrap());
        let (disconnect, wait) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            while !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                let mut bytes = [0; 4096];
                let count = socket.read(&mut bytes).await.unwrap();
                assert_ne!(count, 0);
                request.extend_from_slice(&bytes[..count]);
            }
            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n9\r\ndata: 1\n\n\r\n").await.unwrap();
            // Close only after the client observes a real stream chunk. Otherwise
            // Hyper may legally fail before forwarding headers on fast runners.
            wait.await.unwrap();
        });
        let doc = document(free_port());
        let server = start_inner(&doc, upstream, None).unwrap();
        let response = reqwest::Client::builder()
            .no_proxy()
            .build()
            .unwrap()
            .post(endpoint(&doc, "/responses"))
            .bearer_auth("fixture")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let mut stream = response.bytes_stream();
        let first = tokio::time::timeout(Duration::from_secs(3), stream.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(&first[..], b"data: 1\n\n");
        disconnect.send(()).unwrap();
        assert!(tokio::time::timeout(Duration::from_secs(3), stream.next())
            .await
            .unwrap()
            .unwrap()
            .is_err());
        task.await.unwrap();
        assert_eq!(server.stats.requests.load(Ordering::Relaxed), 1);
        assert_eq!(server.stats.completed.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn downstream_cancellation_releases_upstream_and_active_slot() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream = format!("http://{}", listener.local_addr().unwrap());
        let (closed, wait) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buffer = [0; 8192];
            let _ = socket.read(&mut buffer).await.unwrap();
            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n9\r\ndata: 1\n\n\r\n").await.unwrap();
            let n = socket.read(&mut buffer).await.unwrap_or(0);
            assert_eq!(n, 0);
            let _ = closed.send(());
        });
        let doc = document(free_port());
        let server = start_inner(&doc, upstream, None).unwrap();
        let response = reqwest::Client::builder()
            .no_proxy()
            .build()
            .unwrap()
            .post(endpoint(&doc, "/responses"))
            .bearer_auth("fixture")
            .send()
            .await
            .unwrap();
        let mut stream = response.bytes_stream();
        assert!(stream.next().await.unwrap().is_ok());
        drop(stream);
        tokio::time::timeout(Duration::from_secs(3), wait)
            .await
            .unwrap()
            .unwrap();
        task.await.unwrap();
        assert_eq!(server.stats.active.load(Ordering::Relaxed), 0);
    }

    fn document(port: u16) -> RouteDocument {
        let mut doc = RouteDocument::default();
        doc.settings.listen_port = port;
        doc
    }

    async fn fixture(response: &'static str) -> (String, tokio::task::JoinHandle<String>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut all = Vec::new();
            loop {
                let mut chunk = [0; 4096];
                let n = socket.read(&mut chunk).await.unwrap();
                if n == 0 {
                    break;
                }
                all.extend_from_slice(&chunk[..n]);
                if let Some(end) = all.windows(4).position(|w| w == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&all[..end]).to_ascii_lowercase();
                    let length = headers
                        .lines()
                        .find_map(|line| line.strip_prefix("content-length:"))
                        .and_then(|v| v.trim().parse::<usize>().ok())
                        .unwrap_or(0);
                    if all.len() >= end + 4 + length {
                        break;
                    }
                }
            }
            socket.write_all(response.as_bytes()).await.unwrap();
            String::from_utf8_lossy(&all).into_owned()
        });
        (url, task)
    }

    fn free_port() -> u16 {
        TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }
    fn endpoint(doc: &RouteDocument, suffix: &str) -> String {
        format!(
            "http://127.0.0.1:{}/rd/{}/v1{}",
            doc.settings.listen_port, doc.access_token, suffix
        )
    }

    #[test]
    fn fixed_endpoint_allowlist_and_hop_headers() {
        assert!(allowed_endpoint(&Method::POST, "/responses"));
        assert!(!allowed_endpoint(&Method::CONNECT, "/responses"));
        assert!(!allowed_endpoint(&Method::POST, "/responses/../admin"));
        assert!(!safe_header("proxy-authorization", &[]));
        assert!(!safe_header("x-hidden", &["x-hidden".into()]));
        assert!(safe_header("chatgpt-account-id", &[]));
    }

    #[tokio::test]
    async fn http_errors_are_preserved_and_never_retried() {
        let (url, task) = fixture("HTTP/1.1 429 Too Many Requests\r\nContent-Length: 2\r\nRetry-After: 5\r\nConnection: close\r\n\r\n{}").await;
        let doc = document(free_port());
        let server = start_inner(&doc, url, None).unwrap();
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let response = client
            .post(endpoint(&doc, "/responses"))
            .bearer_auth("test-only")
            .body("{}")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 429);
        assert_eq!(response.headers()["retry-after"], "5");
        assert_eq!(response.text().await.unwrap(), "{}");
        let request = task.await.unwrap();
        assert!(request.starts_with("POST /responses HTTP/1.1"));
        assert_eq!(server.stats.requests.load(Ordering::Relaxed), 1);
        let diagnostic = server
            .stats
            .last_diagnostic
            .lock()
            .unwrap()
            .clone()
            .unwrap();
        assert_eq!(diagnostic.category, Failure::RateOrQuota);
        assert_eq!(diagnostic.http_status, Some(429));
        assert!(!diagnostic.category.warrants_check());
        assert!(!diagnostic.log_message().contains("test-only"));
    }

    #[tokio::test]
    async fn missing_local_proxy_gets_distinct_diagnostic_without_model_replay() {
        // Keep an unlistened port reserved to avoid a free-port/rebind race.
        let reserved = tokio::net::TcpSocket::new_v4().unwrap();
        reserved.bind("127.0.0.1:0".parse().unwrap()).unwrap();
        let port = reserved.local_addr().unwrap().port();
        let doc = document(free_port());
        let server = start_inner(
            &doc,
            "https://official-fixture.invalid/v1".into(),
            Some(format!("http://127.0.0.1:{port}")),
        )
        .unwrap();
        let response = reqwest::Client::builder()
            .no_proxy()
            .build()
            .unwrap()
            .post(endpoint(&doc, "/responses?secret=fixture-sensitive"))
            .bearer_auth("fixture-private-token")
            .body("private-prompt")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 502);
        assert_eq!(
            response.json::<serde_json::Value>().await.unwrap()["error"]["code"],
            "local_proxy_unavailable"
        );
        let d = server
            .stats
            .last_diagnostic
            .lock()
            .unwrap()
            .clone()
            .unwrap();
        assert_eq!(d.http_status, None);
        assert_eq!(d.attribution, "unconfirmed");
        let serialized = serde_json::to_string(&d).unwrap();
        for secret in [
            "fixture-sensitive",
            "private-prompt",
            "fixture-private-token",
            "official-fixture.invalid",
        ] {
            assert!(!serialized.contains(secret));
        }
        assert_eq!(server.stats.requests.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn upstream_503_is_passed_through_without_replay_or_node_penalty() {
        let (url, task) =
            fixture("HTTP/1.1 503 Unavailable\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}")
                .await;
        let doc = document(free_port());
        let server = start_inner(&doc, url, None).unwrap();
        let response = reqwest::Client::builder()
            .no_proxy()
            .build()
            .unwrap()
            .post(endpoint(&doc, "/responses"))
            .bearer_auth("fixture")
            .body("{}")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 503);
        assert_eq!(response.text().await.unwrap(), "{}");
        assert!(task.await.unwrap().starts_with("POST /responses"));
        let d = server
            .stats
            .last_diagnostic
            .lock()
            .unwrap()
            .clone()
            .unwrap();
        assert_eq!(d.category, Failure::UpstreamServer);
        assert!(!d.category.warrants_check());
        assert_eq!(server.stats.requests.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn rejects_browser_wrong_token_upgrade_and_unsupported_paths_before_forwarding() {
        let doc = document(free_port());
        let _server = start_inner(&doc, "http://127.0.0.1:1".into(), None).unwrap();
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        for (suffix, status) in [("/responses", 401), ("/admin", 404)] {
            assert_eq!(
                client
                    .post(endpoint(&doc, suffix))
                    .send()
                    .await
                    .unwrap()
                    .status(),
                status
            );
        }
        assert_eq!(
            client
                .post(endpoint(&doc, "/responses"))
                .header("Origin", "https://example.invalid")
                .send()
                .await
                .unwrap()
                .status(),
            403
        );
        assert_eq!(
            client
                .get(endpoint(&doc, "/responses"))
                .header("Upgrade", "websocket")
                .send()
                .await
                .unwrap()
                .status(),
            426
        );
        assert_eq!(
            client
                .post(format!(
                    "http://127.0.0.1:{}/rd/wrong/v1/responses",
                    doc.settings.listen_port
                ))
                .send()
                .await
                .unwrap()
                .status(),
            404
        );
    }

    #[tokio::test]
    async fn streams_first_chunk_before_upstream_completes() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let (release, wait) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buffer = [0; 8192];
            let _ = socket.read(&mut buffer).await.unwrap();
            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n9\r\ndata: 1\n\n\r\n").await.unwrap();
            wait.await.unwrap();
            socket
                .write_all(b"9\r\ndata: 2\n\n\r\n0\r\n\r\n")
                .await
                .unwrap();
        });
        let doc = document(free_port());
        let server = start_inner(&doc, url, None).unwrap();
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let response = client
            .post(endpoint(&doc, "/responses"))
            .bearer_auth("test-only")
            .send()
            .await
            .unwrap();
        let mut body = response.bytes_stream();
        let first = tokio::time::timeout(Duration::from_secs(2), body.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(first, Bytes::from_static(b"data: 1\n\n"));
        assert_eq!(server.stats.active.load(Ordering::Relaxed), 1);
        release.send(()).unwrap();
        let mut rest = Vec::new();
        while let Some(chunk) = body.next().await {
            rest.extend_from_slice(&chunk.unwrap());
        }
        assert_eq!(rest, b"data: 2\n\n");
        task.await.unwrap();
    }

    #[tokio::test]
    async fn redirects_are_not_followed() {
        let (url, task) = fixture("HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/private\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
        let doc = document(free_port());
        let _server = start_inner(&doc, url, None).unwrap();
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let response = client
            .post(endpoint(&doc, "/responses"))
            .bearer_auth("test-only")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 502);
        assert!(response.headers().get("location").is_none());
        task.await.unwrap();
    }
}
