//! Opt-in measurements of production HTTP handlers and WebSocket bridges.
//! All traffic and credentials are synthetic and restricted to loopback. This
//! module lives in test assembly so proxy never depends on API or clients.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::body::{Body, Bytes};
use axum::extract::ws::{Message, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::Router;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::{client::IntoClientRequest, Message as ClientMessage};

use crate::clients::router::ingress;
use crate::domain::accounts::store::UpsertAccountInput;
use crate::domain::providers::model::{AppKind, Provider};
use crate::domain::settings::config::RouterIdentity;
use crate::domain::sharing::shares::UpsertShareInput;
use crate::infra::resident_memory::process_resident_memory;
use crate::state::{ServerState, ServerStateInner};

const SECRET: &str = "loopback-probe-control-secret-0000000000";
const SHARE: &str = "loopback-probe";
const WARMUP: usize = 5;
static SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug, Default)]
struct Samples(Arc<Mutex<BTreeMap<String, Vec<f64>>>>);

#[derive(Debug)]
struct SampleHandle {
    samples: Samples,
    key: String,
}

impl metrics::HistogramFn for SampleHandle {
    fn record(&self, value: f64) {
        self.samples
            .0
            .lock()
            .unwrap()
            .entry(self.key.clone())
            .or_default()
            .push(value);
    }
}

impl metrics::Recorder for Samples {
    fn describe_counter(
        &self,
        _: metrics::KeyName,
        _: Option<metrics::Unit>,
        _: metrics::SharedString,
    ) {
    }
    fn describe_gauge(
        &self,
        _: metrics::KeyName,
        _: Option<metrics::Unit>,
        _: metrics::SharedString,
    ) {
    }
    fn describe_histogram(
        &self,
        _: metrics::KeyName,
        _: Option<metrics::Unit>,
        _: metrics::SharedString,
    ) {
    }
    fn register_counter(&self, _: &metrics::Key, _: &metrics::Metadata<'_>) -> metrics::Counter {
        metrics::Counter::noop()
    }
    fn register_gauge(&self, _: &metrics::Key, _: &metrics::Metadata<'_>) -> metrics::Gauge {
        metrics::Gauge::noop()
    }
    fn register_histogram(
        &self,
        key: &metrics::Key,
        _: &metrics::Metadata<'_>,
    ) -> metrics::Histogram {
        if !key.name().starts_with("cc_switch_forward_")
            && key.name() != "cc_switch_request_memory_high_water_bytes"
        {
            return metrics::Histogram::noop();
        }
        let mut labels = key
            .labels()
            .map(|label| format!("{}={}", label.key(), label.value()))
            .collect::<Vec<_>>();
        labels.sort();
        metrics::Histogram::from_arc(Arc::new(SampleHandle {
            samples: self.clone(),
            key: format!("{}{{{}}}", key.name(), labels.join(",")),
        }))
    }
}

impl Samples {
    fn clear(&self) {
        self.0.lock().unwrap().clear();
    }

    fn release_count(&self) -> usize {
        self.0
            .lock()
            .unwrap()
            .iter()
            .filter(|(key, _)| {
                key.starts_with("cc_switch_forward_milestone_seconds{")
                    && key.contains("phase=owner_released,")
                    && key.contains("scope=request,")
            })
            .map(|(_, values)| values.len())
            .sum()
    }

    fn high_water_count(&self) -> usize {
        self.0
            .lock()
            .unwrap()
            .iter()
            .filter(|(key, _)| key.starts_with("cc_switch_request_memory_high_water_bytes{"))
            .map(|(_, values)| values.len())
            .sum()
    }

    async fn await_release(&self, count: usize) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while self.release_count() < count || self.high_water_count() < count {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .expect("all request owners must drain");
    }

    fn summary(&self) -> Value {
        let map = self.0.lock().unwrap();
        Value::Object(
            map.iter()
                .map(|(key, values)| (key.clone(), distribution(values)))
                .collect(),
        )
    }

    fn result_count(&self, result: &str) -> usize {
        let label = format!("result={result},");
        self.0
            .lock()
            .unwrap()
            .iter()
            .filter(|(key, _)| {
                key.starts_with("cc_switch_forward_duration_seconds{")
                    && key.contains(&label)
                    && key.contains("scope=request,")
            })
            .map(|(_, values)| values.len())
            .sum()
    }
}

fn distribution(values: &[f64]) -> Value {
    if values.is_empty() {
        return Value::Null;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let percentile = |fraction: f64| {
        sorted[((fraction * sorted.len() as f64).ceil() as usize)
            .saturating_sub(1)
            .min(sorted.len() - 1)]
    };
    json!({"count":sorted.len(), "min":sorted[0], "p50":percentile(0.50),
        "p95":percentile(0.95), "p99":percentile(0.99), "max":sorted[sorted.len()-1]})
}

fn rss_bytes() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .unwrap()
        .lines()
        .find_map(|line| {
            line.strip_prefix("VmRSS:")
                .and_then(|value| value.split_whitespace().next())
                .and_then(|value| value.parse::<u64>().ok())
        })
        .unwrap()
        * 1024
}

fn cpu_seconds() -> f64 {
    // Linux process CPU, including the fake upstream, clients and sampler.
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
    // SAFETY: getrusage initializes the supplied structure on successful return.
    let result = unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) };
    assert_eq!(result, 0);
    // SAFETY: successful getrusage initialized usage above.
    let usage = unsafe { usage.assume_init() };
    (usage.ru_utime.tv_sec + usage.ru_stime.tv_sec) as f64
        + (usage.ru_utime.tv_usec + usage.ru_stime.tv_usec) as f64 / 1_000_000.0
}

#[derive(Clone, Default)]
struct UpstreamStats {
    requests: Arc<AtomicUsize>,
    request_bytes: Arc<AtomicUsize>,
}

struct SyntheticResponse {
    id: String,
    chunks: usize,
    chunk: String,
    pause: Duration,
    split_items: bool,
}

impl SyntheticResponse {
    fn new(body: &Value, stats: &UpstreamStats, bytes: usize) -> Self {
        let index = stats.requests.fetch_add(1, Ordering::Relaxed);
        stats.request_bytes.fetch_add(bytes, Ordering::Relaxed);
        let mode = body.get("input").and_then(Value::as_str).unwrap_or("");
        let (chunks, size, pause_ms) = match mode {
            "probe:cancel" => (4096, 1024, 1),
            "probe:slow" => (128, 64 * 1024, 0),
            _ => (6, 4096, 1),
        };
        Self {
            id: format!("probe-{index}"),
            chunks,
            chunk: "x".repeat(size),
            pause: Duration::from_millis(pause_ms),
            split_items: mode == "probe:slow",
        }
    }

    fn opening(&self) -> Vec<Value> {
        vec![
            json!({"type":"response.metadata", "headers":{}}),
            json!({"type":"response.created", "response":{"id":self.id,"status":"in_progress","output":[]}}),
            self.item_started(0),
        ]
    }

    fn parts_per_item(&self) -> usize {
        if self.split_items {
            8
        } else {
            self.chunks
        }
    }

    fn item_started(&self, index: usize) -> Value {
        json!({"type":"response.output_item.added","output_index":index,"item":{
            "type":"message","id":format!("probe-message-{index}"),"role":"assistant","status":"in_progress","content":[]}})
    }

    fn completed_item(&self, index: usize) -> Value {
        let content = (0..self.parts_per_item())
            .map(|_| json!({"type":"output_text","text":self.chunk}))
            .collect::<Vec<_>>();
        json!({"type":"message","id":format!("probe-message-{index}"),"role":"assistant","phase":"final_answer",
            "status":"completed","content":content})
    }

    fn content_events(&self, index: usize) -> Vec<Value> {
        let item = index / self.parts_per_item();
        let part = index % self.parts_per_item();
        let mut events = Vec::new();
        if part == 0 && item > 0 {
            events.push(self.item_started(item));
        }
        events.push(
            json!({"type":"response.output_text.delta","item_id":format!("probe-message-{item}"),
            "output_index":item,"content_index":part,"delta":self.chunk}),
        );
        if part + 1 == self.parts_per_item() {
            events.push(json!({"type":"response.output_item.done","output_index":item,"item":self.completed_item(item)}));
        }
        events
    }

    fn terminal(&self) -> Value {
        // Large streams use sixteen completed 512 KiB items and the existing
        // sparse-terminal reconstruction contract. Every HTTP SSE event stays
        // below the 2 MiB pending-event bound, regardless of TCP chunking.
        let output = if self.split_items {
            Vec::new()
        } else {
            vec![self.completed_item(0)]
        };
        json!({"type":"response.completed","response":{"id":self.id,"status":"completed","model":"gpt-5.4-mini",
            "output":output,
            "usage":{"input_tokens":8,"output_tokens":8,"total_tokens":16}}})
    }
}

async fn upstream_http(stats: UpstreamStats, body: Bytes) -> impl IntoResponse {
    let value: Value = serde_json::from_slice(&body).unwrap();
    let response = SyntheticResponse::new(&value, &stats, body.len());
    let stream = async_stream::stream! {
        for event in response.opening() {
            yield Ok::<_, std::convert::Infallible>(Bytes::from(format!("data: {event}\n\n")));
        }
        for index in 0..response.chunks {
            if !response.pause.is_zero() { tokio::time::sleep(response.pause).await; }
            for event in response.content_events(index) {
                yield Ok(Bytes::from(format!("data: {event}\n\n")));
            }
        }
        // SSE permits multiple data lines; keep the full event bounded too.
        let terminal = serde_json::to_string_pretty(&response.terminal()).unwrap();
        let mut wire = String::with_capacity(terminal.len() + 4096);
        for line in terminal.lines() { wire.push_str("data: ");wire.push_str(line);wire.push('\n'); }
        wire.push('\n');
        yield Ok(Bytes::from(wire));
    };
    (
        [("content-type", "text/event-stream")],
        Body::from_stream(stream),
    )
}

async fn upstream_ws(stats: UpstreamStats, ws: WebSocketUpgrade) -> impl IntoResponse {
    ws.on_upgrade(move |mut socket| async move {
        while let Some(Ok(message)) = socket.recv().await {
            let Message::Text(text) = message else {
                break;
            };
            let body: Value = serde_json::from_str(&text).unwrap();
            let response = SyntheticResponse::new(&body, &stats, text.len());
            for event in response.opening() {
                if socket.send(Message::Text(event.to_string())).await.is_err() {
                    return;
                }
            }
            for index in 0..response.chunks {
                if !response.pause.is_zero() {
                    tokio::time::sleep(response.pause).await;
                }
                for event in response.content_events(index) {
                    if socket.send(Message::Text(event.to_string())).await.is_err() {
                        return;
                    }
                }
            }
            if socket
                .send(Message::Text(response.terminal().to_string()))
                .await
                .is_err()
            {
                return;
            }
        }
    })
}

struct Directory(PathBuf);
impl Drop for Directory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Fixture {
    state: ServerState,
    server: JoinHandle<()>,
    upstream: JoinHandle<()>,
    address: std::net::SocketAddr,
    upstream_address: std::net::SocketAddr,
    stats: UpstreamStats,
    _directory: Directory,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
        self.upstream.abort();
    }
}

impl Fixture {
    async fn new(fallback: bool) -> Self {
        let stats = UpstreamStats::default();
        let http_stats = stats.clone();
        let ws_stats = stats.clone();
        let upstream_app = Router::new()
            .route(
                "/v1/responses",
                post(move |body: Bytes| upstream_http(http_stats.clone(), body)),
            )
            .route(
                "/ws",
                get(move |ws: WebSocketUpgrade| upstream_ws(ws_stats.clone(), ws)),
            )
            .route("/fallback", get(|| async { StatusCode::BAD_GATEWAY }))
            .layer(axum::extract::DefaultBodyLimit::max(4 * 1024 * 1024));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream_address = listener.local_addr().unwrap();
        let upstream = tokio::spawn(async move {
            axum::serve(listener, upstream_app).await.unwrap();
        });
        let directory = Directory(std::env::temp_dir().join(format!(
            "cc-switch-codex-server-probe-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        )));
        let state = ServerStateInner::load(
            crate::cli::Cli {
                host: "127.0.0.1".parse().unwrap(),
                port: 0,
                config_dir: Some(directory.0.clone()),
                web_dist_dir: None,
                log_level: "warn".into(),
                command: None,
            },
            Arc::new(crate::logging::LogCapture::new(
                crate::logging::RING_BUFFER_CAPACITY,
            )),
        )
        .unwrap();
        let mut config = state.config_snapshot().await;
        config.router.domain = Some("router.test".into());
        config.router.identity = Some(RouterIdentity {
            installation_id: "probe-installation".into(),
            public_key: "fixture".into(),
            private_key: "fixture".into(),
            control_secret: Some(SECRET.into()),
        });
        state.replace_config(config).await.unwrap();
        let account: UpsertAccountInput = serde_json::from_value(json!({"id":"probe-account","providerType":"codex_oauth",
            "accessToken":"loopback-only","expiresAt":i64::MAX/2,"profile":{
                "verifiedOpenAiClaims":{"subject":"probe-subject","chatgpt_account_id":"probe-workspace"},
                "codexWorkspaceProvenance":{"workspaceId":"probe-workspace","source":"test_fixture"}}})).unwrap();
        state
            .mutate_accounts_immediate(move |accounts| {
                accounts.upsert(account);
            })
            .await
            .unwrap();
        let provider: Provider = serde_json::from_value(json!({"id":"probe-provider","name":"Local measurement",
            "settingsConfig":{},
            "meta":{"providerType":"codex_oauth","authBinding":{"source":"account_store","authProvider":"codex_oauth",
                "accountId":"probe-account","authIdentityGeneration":1}}})).unwrap();
        state
            .mutate_providers_immediate(move |providers| providers.upsert(AppKind::Codex, provider))
            .await
            .unwrap();
        state
            .override_provider_runtime_endpoint_for_test(
                AppKind::Codex,
                "probe-provider",
                format!("http://{upstream_address}"),
            )
            .await
            .unwrap();
        let share: UpsertShareInput = serde_json::from_value(json!({"id":SHARE,"ownerEmail":"owner@example.com","app":"codex",
            "providerId":"probe-provider","providerType":"codex_oauth","enabled":true,"status":"active","autoStart":true,
            "tunnelSubdomain":"probe","freeAccess":false,"previousResponseCacheEnabled":false,
            "bindings":[{"app":"codex","providerId":"probe-provider","providerType":"codex_oauth"}]})).unwrap();
        state
            .mutate_shares_immediate(move |shares| shares.upsert(share).unwrap())
            .await
            .unwrap();
        // Control-plane writers deliberately discard arbitrary driver options.
        // Install only this fixture's loopback WS URL in test assembly, using
        // the existing state boundary; never add a production endpoint switch.
        let websocket_url = format!(
            "ws://{upstream_address}/{}",
            if fallback { "fallback" } else { "ws" }
        );
        let mut providers = state.providers_snapshot().await;
        let mut plan = providers
            .runtime_plan(AppKind::Codex, "probe-provider")
            .unwrap()
            .as_ref()
            .clone();
        plan.driver_options
            .insert("testCodexWebsocketUrl".into(), json!(websocket_url));
        Arc::make_mut(&mut providers.runtime_index).insert_plan_for_test(plan);
        state.replace_provider_store_for_test(providers).await;
        let providers = state.providers_snapshot().await;
        let plan = providers
            .runtime_plan(AppKind::Codex, "probe-provider")
            .unwrap();
        for (raw, scheme) in [
            (plan.endpoint.as_str(), "http"),
            (
                plan.driver_options["testCodexWebsocketUrl"]
                    .as_str()
                    .unwrap(),
                "ws",
            ),
        ] {
            let url = url::Url::parse(raw).unwrap();
            assert_eq!(url.scheme(), scheme);
            assert_eq!(url.host_str(), Some("127.0.0.1"));
            assert_eq!(url.port(), Some(upstream_address.port()));
            assert!(
                url.username().is_empty()
                    && url.password().is_none()
                    && url.query().is_none()
                    && url.fragment().is_none()
            );
        }
        drop(providers);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = crate::api::app_router(state.clone());
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self {
            state,
            server,
            upstream,
            address,
            upstream_address,
            stats,
            _directory: directory,
        }
    }

    async fn shutdown(&mut self) {
        self.state.begin_shutdown();
        self.server.abort();
        self.upstream.abort();
        let _ = (&mut self.server).await;
        let _ = (&mut self.upstream).await;
        assert!(tokio::net::TcpStream::connect(self.address).await.is_err());
        assert!(tokio::net::TcpStream::connect(self.upstream_address)
            .await
            .is_err());
    }
}

fn signed_headers(method: &str, body: &[u8]) -> HeaderMap {
    let context = ingress::IngressContext {
        signature_version: 2,
        protocol_epoch: ingress::PROTOCOL_EPOCH.into(),
        router_id: "router.test".into(),
        route_id: format!("share:{SHARE}"),
        installation_id: "probe-installation".into(),
        target_lane_id: "probe-installation".into(),
        public_host: "probe--client.router.test".into(),
        share_id: Some(SHARE.into()),
        request_id: format!("probe-{}", SEQUENCE.fetch_add(1, Ordering::Relaxed)),
        user_email: Some("owner@example.com".into()),
        user_role: None,
        user_country: None,
        is_health_check: false,
        method: method.into(),
        path_and_query: "/v1/responses".into(),
        body_sha256: ingress::body_sha256_hex(body),
        issued_at_ms: chrono::Utc::now().timestamp_millis(),
    };
    let encoded = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&context).unwrap());
    let mut mac = Hmac::<Sha256>::new_from_slice(SECRET.as_bytes()).unwrap();
    mac.update(b"cc-switch-router-ingress-v2\n");
    mac.update(ingress::PROTOCOL_EPOCH.as_bytes());
    mac.update(b"\n");
    mac.update(encoded.as_bytes());
    let mut headers = HeaderMap::new();
    headers.insert(ingress::INGRESS_CONTEXT_HEADER, encoded.parse().unwrap());
    headers.insert(
        ingress::INGRESS_SIGNATURE_HEADER,
        URL_SAFE_NO_PAD
            .encode(mac.finalize().into_bytes())
            .parse()
            .unwrap(),
    );
    headers.insert("content-type", "application/json".parse().unwrap());
    headers
}

async fn http_request(
    client: &reqwest::Client,
    fixture: &Fixture,
    wire: Bytes,
    cancel: bool,
    slow: bool,
) -> (f64, f64) {
    let started = Instant::now();
    let response = client
        .post(format!("http://{}/v1/responses", fixture.address))
        .headers(signed_headers("POST", &wire))
        .body(wire)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let mut stream = response.bytes_stream();
    let mut first = None;
    let mut total = 0;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.unwrap();
        total += chunk.len();
        // Client arrival of any nonempty response body is deliberately distinct
        // from the Server's first_semantic milestone.
        first.get_or_insert_with(|| started.elapsed().as_secs_f64());
        if cancel {
            drop(stream);
            return (first.unwrap(), started.elapsed().as_secs_f64());
        }
        if slow {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }
    assert!(total > 0);
    (first.unwrap(), started.elapsed().as_secs_f64())
}

async fn websocket_request(
    fixture: &Fixture,
    mut body: Value,
    cancel: bool,
    slow: bool,
) -> (f64, f64) {
    let mut request = format!("ws://{}/v1/responses", fixture.address)
        .into_client_request()
        .unwrap();
    request.headers_mut().extend(signed_headers("GET", b""));
    let (mut socket, _) = tokio_tungstenite::connect_async(request).await.unwrap();
    body["type"] = json!("response.create");
    let started = Instant::now();
    socket
        .send(ClientMessage::Text(body.to_string()))
        .await
        .unwrap();
    let mut first = None;
    while let Some(message) = socket.next().await {
        let message = message.unwrap();
        let ClientMessage::Text(text) = message else {
            continue;
        };
        let event: Value = serde_json::from_str(&text).unwrap();
        let kind = event["type"].as_str().unwrap_or("");
        assert!(
            !matches!(kind, "error" | "response.failed"),
            "unexpected local probe failure: kind={kind}, code={:?}",
            event
                .pointer("/error/code")
                .or_else(|| event.pointer("/response/error/code"))
                .and_then(Value::as_str)
        );
        if kind == "response.output_text.delta" {
            first.get_or_insert_with(|| started.elapsed().as_secs_f64());
        }
        if cancel {
            if let Some(first) = first {
                // Measure an explicit client Close, independently of transport
                // resets. Keep reading until the peer closes so dropping the
                // socket cannot race the server's observation of this Close.
                socket.close(None).await.unwrap();
                tokio::time::timeout(Duration::from_secs(2), async {
                    while let Some(Ok(message)) = socket.next().await {
                        if matches!(message, ClientMessage::Close(_)) {
                            break;
                        }
                    }
                })
                .await
                .expect("bounded cancellation close handshake");
                drop(socket);
                return (first, started.elapsed().as_secs_f64());
            }
        }
        if kind == "response.completed" {
            assert_eq!(event["response"]["usage"]["total_tokens"], 16);
            let elapsed = started.elapsed().as_secs_f64();
            socket.close(None).await.unwrap();
            return (first.unwrap(), elapsed);
        }
        if slow {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }
    panic!("probe websocket ended without terminal");
}

#[derive(Clone, Copy)]
struct Measurement<'a> {
    name: &'a str,
    path: &'a str,
    count: usize,
    cancel: bool,
    slow: bool,
}

async fn measure_row(
    samples: &Samples,
    fixture: &Fixture,
    client: &reqwest::Client,
    body: &Value,
    measurement: Measurement<'_>,
) -> Value {
    let Measurement {
        name,
        path,
        count,
        cancel,
        slow,
    } = measurement;
    eprintln!("probe start {path} {name} samples={count}");
    let is_ws = path.ends_with("ws");
    let mut body = body.clone();
    body["stream"] = json!(path != "http_json");
    let wire = Bytes::from(serde_json::to_vec(&body).unwrap());
    let run = || async {
        tokio::time::timeout(Duration::from_secs(20), async {
            if is_ws {
                websocket_request(fixture, body.clone(), cancel, slow).await
            } else {
                http_request(client, fixture, wire.clone(), cancel, slow).await
            }
        })
        .await
        .expect("bounded loopback probe operation")
    };
    samples.clear();
    for index in 1..=WARMUP {
        run().await;
        samples.await_release(index).await;
    }
    samples.clear();
    let rss_before = rss_bytes();
    let parent_before = process_resident_memory().snapshot();
    let stats_before = fixture.stats.request_bytes.load(Ordering::Relaxed);
    let calls_before = fixture.stats.requests.load(Ordering::Relaxed);
    let peaks = Arc::new((
        AtomicU64::new(rss_before),
        AtomicUsize::new(parent_before.used_bytes),
    ));
    let sampled = peaks.clone();
    let sampler = tokio::spawn(async move {
        loop {
            sampled.0.fetch_max(rss_bytes(), Ordering::Relaxed);
            sampled.1.fetch_max(
                process_resident_memory().snapshot().used_bytes,
                Ordering::Relaxed,
            );
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    });
    let cpu_start = cpu_seconds();
    let wall_start = Instant::now();
    let mut first = Vec::new();
    let mut finished = Vec::new();
    let mut cancellation_release = Vec::new();
    for index in 1..=count {
        let (a, b) = run().await;
        first.push(a);
        finished.push(b);
        let drop_at = Instant::now();
        samples.await_release(index).await;
        if cancel {
            cancellation_release.push(drop_at.elapsed().as_secs_f64());
        }
    }
    let cpu = cpu_seconds() - cpu_start;
    let wall = wall_start.elapsed().as_secs_f64();
    sampler.abort();
    let _ = sampler.await;
    assert_eq!(
        fixture.stats.requests.load(Ordering::Relaxed) - calls_before,
        count,
        "one upstream generation per request"
    );
    assert_eq!(
        samples.result_count(if cancel { "cancelled" } else { "complete" }),
        count,
        "unexpected delivery results: {}",
        samples.summary()
    );
    let parent_after = process_resident_memory().snapshot();
    if is_ws {
        let metrics = samples.0.lock().unwrap();
        assert!(metrics
            .keys()
            .any(|key| key.starts_with("cc_switch_forward_wait_seconds{")
                && key.contains("phase=downstream_write,")
                && key.contains("scope=request,")));
        assert!(metrics.iter().any(|(key, values)| key
            .starts_with("cc_switch_forward_milestone_seconds{")
            && key.contains("phase=downstream_write_completed,")
            && key.contains("scope=request,")
            && values.len() == count));
    }
    assert_eq!(
        parent_after.rejections, 0,
        "observation-only workload cannot shed capacity"
    );
    eprintln!("probe complete {path} {name}");
    json!({"case":name,"path":path,"samples":count,"warmup":WARMUP,"inputSha256":hex::encode(Sha256::digest(&wire)),
        "clientBodyBytes":wire.len(),"upstreamApplicationRequestBytes":fixture.stats.request_bytes.load(Ordering::Relaxed)-stats_before,
        "upstreamGenerations":count,"clientFirstArrivalSeconds":distribution(&first),"clientCompletionOrDropSeconds":distribution(&finished),
        "clientReturnToObservedOwnerReleaseSeconds":distribution(&cancellation_release),"serverMetrics":samples.summary(),
        "processCpuSeconds":cpu,"wallSeconds":wall,"rssBeforeBytes":rss_before,"rssAfterBytes":rss_bytes(),
        "rssSampledPeakBytes":peaks.0.load(Ordering::Relaxed),"logicalUsedBeforeBytes":parent_before.used_bytes,
        "logicalUsedAfterBytes":parent_after.used_bytes,"logicalSampledPeakBytes":peaks.1.load(Ordering::Relaxed),
        "processLogicalCumulativeHighWaterBytes":parent_after.high_water_bytes,"processLogicalRejections":parent_after.rejections})
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "opt-in Linux loopback measurement; use scripts/bench/codex-server-probe.py"]
async fn codex_server_loopback_measurement() {
    let input = PathBuf::from(
        std::env::var_os("CC_SWITCH_CODEX_PROBE_INPUT_DIR").expect("probe fixture directory"),
    );
    let output =
        PathBuf::from(std::env::var_os("CC_SWITCH_CODEX_PROBE_OUTPUT").expect("probe output file"));
    let count = std::env::var("CC_SWITCH_CODEX_PROBE_SAMPLES")
        .unwrap()
        .parse::<usize>()
        .unwrap();
    assert!((3..=1000).contains(&count));
    let samples = Samples::default();
    metrics::set_global_recorder(samples.clone()).unwrap();
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(15))
        .build()
        .unwrap();
    let mut results = Vec::new();
    for path in ["native_ws", "fallback_ws", "http_sse", "http_json"] {
        let mut fixture = Fixture::new(path == "fallback_ws").await;
        for name in ["short", "long_history", "tool_history", "base64_entropy"] {
            let body: Value =
                serde_json::from_slice(&std::fs::read(input.join(format!("{name}.json"))).unwrap())
                    .unwrap();
            results.push(
                measure_row(
                    &samples,
                    &fixture,
                    &client,
                    &body,
                    Measurement {
                        name,
                        path,
                        count,
                        cancel: false,
                        slow: false,
                    },
                )
                .await,
            );
        }
        if path != "http_json" {
            for (name, cancel, slow) in [("cancel", true, false), ("slow_reader", false, true)] {
                let body = json!({"model":"gpt-5.4-mini","input":if cancel {"probe:cancel"} else {"probe:slow"},"stream":true});
                results.push(
                    measure_row(
                        &samples,
                        &fixture,
                        &client,
                        &body,
                        Measurement {
                            name,
                            path,
                            count: if slow { count.min(31) } else { count },
                            cancel,
                            slow,
                        },
                    )
                    .await,
                );
            }
        }
        fixture.shutdown().await;
    }
    let binary = std::env::current_exe().unwrap();
    let report = json!({"format":"cc-switch-codex-server-loopback-probe","schemaVersion":1,
        "capturedAt":chrono::Utc::now().to_rfc3339(),"profile":"cargo-test debug unoptimized with debuginfo",
        "binarySha256":sha_file(&binary),"testBinary":binary,"workerThreads":4,
        "scope":"production API router and signed Share ingress; loopback CodexOAuth HTTP/WS endpoint overrides exist only under cfg(test)",
        "client":"in-process reqwest HTTP/1 and tokio-tungstenite; HTTP client reused, WS handshake per request excluded from turn latency; HTTP cancellation drops body, WS cancellation sends Close and awaits bounded peer closure before dropping socket",
        "rssScope":"entire Linux test process: server, fake upstream, clients, metrics collector and 1ms sampler; allocator retention included",
        "limitations":["not vendor acceptance, deployment or production capacity evidence","sequential workload; sampled RSS can miss shorter peaks",
            "no allocation-count instrumentation; logical budget bytes are not allocations or RSS","wait samples are operation intervals and may overlap",
            "normal production background schedulers and Router tunnels are not started by this test assembly",
            "HTTP first arrival is any body bytes; use Server first_semantic separately","no baseline implementation speedup claim",
            "no new outbound, downstream or WS compression enabled; bytes exclude transport framing, headers and TLS",
            "slow reader applies 5ms delay per client chunk/frame; HTTP transport decides chunk boundaries"],
        "percentile":"nearest-rank on measured samples; warmups excluded","results":results,"listenersClosed":true});
    std::fs::write(output, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
}

fn sha_file(path: &Path) -> String {
    use std::io::Read;
    let mut file = std::fs::File::open(path).unwrap();
    let mut hash = Sha256::new();
    let mut buffer = [0; 65536];
    loop {
        let n = file.read(&mut buffer).unwrap();
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
    }
    hex::encode(hash.finalize())
}
