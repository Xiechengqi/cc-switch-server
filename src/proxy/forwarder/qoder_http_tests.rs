use super::*;

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::Response;
use axum::routing::{get, post};
use bytes::Bytes;
use serde_json::{json, Value};

use crate::domain::providers::model::{AppKind, AuthBinding, Provider, ProviderMeta, ProviderType};
use crate::domain::providers::registry::ProfileId;
use crate::domain::providers::store::{ProviderStore, StoredProvider};
use crate::domain::qoder::{
    md5_hex, qoder_decode, sign_qoder_request, QoderCredentialRail, QoderSite,
    QODER_GENERATION_PATH, QODER_GENERATION_SIGNATURE_PATH, QODER_MODEL_LIST_PATH,
    QODER_MODEL_LIST_SIGNATURE_PATH,
};

const MODEL_KEY: &str = "auto";
const MODEL_SOURCE: &str = "fixture-source";
const MODEL_VENDOR: &str = "fixture-vendor";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QoderFixtureRail {
    GlobalOauth,
    CnOauth,
    Pat,
}

impl QoderFixtureRail {
    fn site(self) -> QoderSite {
        match self {
            Self::GlobalOauth | Self::Pat => QoderSite::Global,
            Self::CnOauth => QoderSite::Cn,
        }
    }

    fn credential_rail(self) -> QoderCredentialRail {
        match self {
            Self::GlobalOauth => QoderCredentialRail::GlobalOauth,
            Self::CnOauth => QoderCredentialRail::CnOauth,
            Self::Pat => QoderCredentialRail::PatJobToken,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::GlobalOauth => "global-oauth",
            Self::CnOauth => "cn-oauth",
            Self::Pat => "pat",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QoderGenerationReply {
    Success,
    HttpUnauthorized,
    EmbeddedUnauthorized,
    FirstBusinessThenUnauthorized,
    Entitlement112,
    AgentLimit115,
}

#[derive(Debug, Clone)]
struct QoderRequestObservation {
    uri: String,
    headers: BTreeMap<String, String>,
    decoded_body: Option<Value>,
}

#[derive(Clone)]
struct QoderFixtureState {
    replies: Arc<Vec<QoderGenerationReply>>,
    generation_requests: Arc<AtomicUsize>,
    model_requests: Arc<AtomicUsize>,
    pat_exchanges: Arc<AtomicUsize>,
    refresh_requests: Arc<AtomicUsize>,
    observations: Arc<Mutex<Vec<QoderRequestObservation>>>,
}

impl QoderFixtureState {
    fn new(rail: QoderFixtureRail, replies: Vec<QoderGenerationReply>) -> Self {
        let _ = rail;
        Self {
            replies: Arc::new(replies),
            generation_requests: Default::default(),
            model_requests: Default::default(),
            pat_exchanges: Default::default(),
            refresh_requests: Default::default(),
            observations: Default::default(),
        }
    }

    fn count(&self, counter: &AtomicUsize) -> usize {
        counter.load(Ordering::SeqCst)
    }

    fn generation_observations(&self) -> Vec<QoderRequestObservation> {
        self.observations
            .lock()
            .unwrap()
            .iter()
            .filter(|observation| observation.decoded_body.is_some())
            .cloned()
            .collect()
    }
}

fn observed_headers(headers: &HeaderMap) -> BTreeMap<String, String> {
    headers
        .iter()
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|value| (name.as_str().to_string(), value.to_string()))
        })
        .collect()
}

fn record_observation(
    state: &QoderFixtureState,
    uri: Uri,
    headers: &HeaderMap,
    decoded_body: Option<Value>,
) {
    state
        .observations
        .lock()
        .unwrap()
        .push(QoderRequestObservation {
            uri: uri.to_string(),
            headers: observed_headers(headers),
            decoded_body,
        });
}

fn model_config() -> Value {
    json!({
        "key": MODEL_KEY,
        "source": MODEL_SOURCE,
        "vendor": MODEL_VENDOR,
        "max_output_tokens": 4096,
        "enable": true
    })
}

fn wrapped_json(value: Value) -> Response {
    Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({"statusCodeValue": 200, "body": value.to_string()}).to_string(),
        ))
        .unwrap()
}

async fn qoder_model_list(
    State(state): State<QoderFixtureState>,
    uri: Uri,
    headers: HeaderMap,
) -> Response {
    state.model_requests.fetch_add(1, Ordering::SeqCst);
    record_observation(&state, uri, &headers, None);
    wrapped_json(json!({"chat": [model_config()]}))
}

async fn qoder_pat_exchange(State(state): State<QoderFixtureState>) -> Response {
    let request = state.pat_exchanges.fetch_add(1, Ordering::SeqCst);
    Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({
                "access_token": format!("pat-job-token-{request}"),
                "refresh_token": "",
                "expires_in": 3600
            })
            .to_string(),
        ))
        .unwrap()
}

async fn qoder_global_refresh(State(state): State<QoderFixtureState>) -> Response {
    let request = state.refresh_requests.fetch_add(1, Ordering::SeqCst);
    wrapped_json(json!({
        "securityOauthToken": format!("global-refreshed-access-{request}"),
        "refreshToken": format!("global-refreshed-refresh-{request}"),
        "id": "fixture-user",
        "name": "Fixture User",
        "expireTime": chrono::Utc::now().timestamp_millis() + 3_600_000
    }))
}

async fn qoder_cn_refresh(State(state): State<QoderFixtureState>) -> Response {
    let request = state.refresh_requests.fetch_add(1, Ordering::SeqCst);
    Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({
                "access_token": format!("cn-refreshed-access-{request}"),
                "refresh_token": format!("cn-refreshed-refresh-{request}"),
                "user_id": "fixture-user",
                "user_name": "Fixture User",
                "expires_in": 3600
            })
            .to_string(),
        ))
        .unwrap()
}

async fn qoder_user_info() -> axum::Json<Value> {
    axum::Json(json!({
        "id": "fixture-user",
        "user_id": "fixture-user",
        "account_id": "fixture-account",
        "name": "Fixture User",
        "user_type": "personal_standard",
        "organizationId": "fixture-org",
        "organizationName": "Fixture Org"
    }))
}

async fn qoder_cn_status() -> Response {
    wrapped_json(json!({
        "id": "fixture-user",
        "accountId": "fixture-account",
        "name": "Fixture User",
        "orgId": "fixture-org",
        "orgName": "Fixture Org",
        "userType": "personal_standard"
    }))
}

fn inner_success_chunk(content: Option<&str>, terminal: bool) -> Value {
    let delta = match content {
        Some(content) => json!({"content": content}),
        None => json!({}),
    };
    let mut value = json!({
        "id": "chat-qoder-fixture",
        "object": "chat.completion.chunk",
        "created": 1,
        "model": MODEL_KEY,
        "choices": [{
            "index": 0,
            "delta": delta,
            "finish_reason": terminal.then_some("stop")
        }]
    });
    if terminal {
        value["usage"] = json!({
            "prompt_tokens": 2,
            "completion_tokens": 1,
            "total_tokens": 3
        });
    }
    value
}

fn qoder_wrapper(status: Value, body: Value) -> Bytes {
    Bytes::from(format!(
        "data: {}\n\n",
        json!({"statusCode": status, "body": body.to_string()})
    ))
}

fn success_stream() -> Response {
    let chunks = vec![
        Ok::<_, std::convert::Infallible>(qoder_wrapper(
            json!("OK"),
            inner_success_chunk(Some("ready"), false),
        )),
        Ok(qoder_wrapper(json!("OK"), inner_success_chunk(None, true))),
    ];
    Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, "text/event-stream")
        .body(Body::from_stream(futures_util::stream::iter(chunks)))
        .unwrap()
}

fn embedded_error(status: &'static str, body: Value) -> Response {
    Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, "text/event-stream")
        .body(Body::from(qoder_wrapper(json!(status), body)))
        .unwrap()
}

fn business_then_unauthorized() -> Response {
    let chunks = vec![
        Ok::<_, std::convert::Infallible>(qoder_wrapper(
            json!("OK"),
            inner_success_chunk(Some("discard-me"), false),
        )),
        Ok(qoder_wrapper(
            json!("UNAUTHORIZED"),
            json!({"message": "expired after first business chunk"}),
        )),
    ];
    Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, "text/event-stream")
        .body(Body::from_stream(futures_util::stream::iter(chunks)))
        .unwrap()
}

async fn qoder_generation(
    State(state): State<QoderFixtureState>,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let decoded = qoder_decode(std::str::from_utf8(&body).unwrap()).unwrap();
    let decoded: Value = serde_json::from_slice(&decoded).unwrap();
    record_observation(&state, uri, &headers, Some(decoded));
    let request = state.generation_requests.fetch_add(1, Ordering::SeqCst);
    let reply = state
        .replies
        .get(request)
        .copied()
        .or_else(|| state.replies.last().copied())
        .unwrap_or(QoderGenerationReply::Success);
    match reply {
        QoderGenerationReply::Success => success_stream(),
        QoderGenerationReply::HttpUnauthorized => Response::builder()
            .status(StatusCode::UNAUTHORIZED)
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(json!({"message": "expired"}).to_string()))
            .unwrap(),
        QoderGenerationReply::EmbeddedUnauthorized => {
            embedded_error("UNAUTHORIZED", json!({"message": "embedded expired"}))
        }
        QoderGenerationReply::FirstBusinessThenUnauthorized => business_then_unauthorized(),
        QoderGenerationReply::Entitlement112 => {
            embedded_error("FORBIDDEN", json!({"code": 112, "message": "not entitled"}))
        }
        QoderGenerationReply::AgentLimit115 => embedded_error(
            "BAD_REQUEST",
            json!({
                "code": 115,
                "message": "agent limit",
                "agentLimitResetTime": chrono::Utc::now().timestamp_millis() + 60_000
            }),
        ),
    }
}

async fn spawn_qoder_upstream(
    rail: QoderFixtureRail,
    replies: Vec<QoderGenerationReply>,
) -> (
    std::net::SocketAddr,
    QoderFixtureState,
    tokio::task::JoinHandle<()>,
) {
    let state = QoderFixtureState::new(rail, replies);
    let app = axum::Router::new()
        .route(QODER_MODEL_LIST_PATH, get(qoder_model_list))
        .route(
            "/algo/api/v2/service/pro/sse/agent_chat_generation",
            post(qoder_generation),
        )
        .route(
            crate::domain::qoder::QODER_PAT_EXCHANGE_PATH,
            post(qoder_pat_exchange),
        )
        .route("/algo/api/v3/user/jobToken", post(qoder_global_refresh))
        .route("/api/v1/deviceToken/refresh", post(qoder_cn_refresh))
        .route("/api/v1/userinfo", get(qoder_user_info))
        .route("/algo/api/v3/user/status", post(qoder_cn_status))
        .with_state(state.clone());
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (address, state, server)
}

fn qoder_test_state(name: &str) -> ServerState {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    crate::state::ServerStateInner::load(
        crate::cli::Cli {
            host: std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
            port: 0,
            config_dir: Some(
                std::env::temp_dir().join(format!("cc-switch-server-qoder-{name}-{nanos}")),
            ),
            web_dist_dir: None,
            log_level: "warn".to_string(),
            command: None,
        },
        Arc::new(crate::logging::LogCapture::new(
            crate::logging::RING_BUFFER_CAPACITY,
        )),
    )
    .unwrap()
}

fn qoder_profile_id(app: AppKind) -> &'static str {
    match app {
        AppKind::Claude => "claude.qoder_cosy",
        AppKind::Codex => "codex.qoder_cosy",
        AppKind::Gemini => "gemini.qoder_cosy",
    }
}

async fn install_qoder_provider(
    state: &ServerState,
    name: &str,
    app: AppKind,
    rail: QoderFixtureRail,
    origin: &str,
) -> (String, String) {
    let account_id = format!("{name}-account");
    let provider_id = format!("{name}-provider");
    let account_id_for_state = account_id.clone();
    let origin = origin.trim_end_matches('/').to_string();
    let origin_for_account = origin.clone();
    state
        .mutate_accounts_immediate(move |accounts| {
            let site = rail.site();
            let credential_rail = rail.credential_rail();
            let machine_id = match site {
                QoderSite::Global => "0123456789abcdef0123456789abcdef0123",
                QoderSite::Cn => "018f47ec-51d8-4c2a-9c2b-4f859709c9e7",
            };
            let (access_token, refresh_token, api_key) = match rail {
                QoderFixtureRail::GlobalOauth => (
                    Some("global-initial-access"),
                    Some("global-initial-refresh"),
                    None,
                ),
                QoderFixtureRail::CnOauth => {
                    (Some("cn-initial-access"), Some("cn-initial-refresh"), None)
                }
                QoderFixtureRail::Pat => (None, None, Some("pt-fixture")),
            };
            accounts.upsert(
                serde_json::from_value(json!({
                    "id": account_id_for_state,
                    "providerType": "qoder_cosy",
                    "accessToken": access_token,
                    "refreshToken": refresh_token,
                    "apiKey": api_key,
                    "tokenType": "Bearer",
                    "expiresAt": i64::MAX / 2,
                    "profile": {
                        "site": site.as_str(),
                        "credentialRail": credential_rail.as_str(),
                        "refreshMode": if site == QoderSite::Cn { "qodercn20" } else { "cosy" },
                        "uid": "fixture-user",
                        "aid": "fixture-account",
                        "name": "Fixture User",
                        "organizationId": "fixture-org",
                        "organizationName": "Fixture Org",
                        "userType": "personal_standard",
                        "machineId": machine_id,
                        "machineType": "5"
                    },
                    "raw": {
                        "loginMethod": rail.label(),
                        "qoderSecrets": {"machineToken": format!("machine-token-{}", rail.label())},
                        "testQoderEndpoints": {
                            "openapiBaseUrl": origin_for_account,
                            "centerBaseUrl": origin_for_account,
                            "gatewayBaseUrl": origin_for_account,
                            "jobGatewayBaseUrl": origin_for_account
                        }
                    }
                }))
                .unwrap(),
            );
        })
        .await
        .unwrap();

    let mut stored = StoredProvider {
        app,
        provider: Provider {
            id: provider_id.clone(),
            name: format!("Qoder {name}"),
            settings_config: json!({}),
            category: None,
            meta: Some(ProviderMeta {
                auth_binding: Some(AuthBinding {
                    source: Some("account_store".to_string()),
                    auth_provider: Some("qoder_cosy".to_string()),
                    account_id: Some(account_id.clone()),
                    auth_identity_generation: Some(1),
                }),
                provider_type: Some("qoder_cosy".to_string()),
                ..ProviderMeta::default()
            }),
            extra: Default::default(),
        },
        provider_type: ProviderType::QoderCosy,
        provider_type_id: "qoder_cosy".to_string(),
        resource: Default::default(),
    };
    stored.resource.profile_id = Some(ProfileId::parse(qoder_profile_id(app)).unwrap());
    stored.resource.profile_schema_revision = Some(1);
    stored.resource.revision = 1;
    let accounts = state.accounts_snapshot().await;
    let mut providers = ProviderStore {
        providers: vec![stored],
        ..ProviderStore::default()
    };
    providers.rebuild_runtime_index(&accounts).unwrap();
    state.replace_provider_store_for_test(providers).await;
    (provider_id, account_id)
}

async fn install_qoder_share(
    state: &ServerState,
    share_id: &str,
    app: AppKind,
    provider_id: &str,
    owner: &str,
) {
    let share_id = share_id.to_string();
    let provider_id = provider_id.to_string();
    let owner = owner.to_string();
    state
        .mutate_shares_immediate(move |shares| {
            shares.shares.push(
                serde_json::from_value(json!({
                    "id": share_id,
                    "app": app,
                    "providerId": provider_id,
                    "providerType": "qoder_cosy",
                    "ownerEmail": owner,
                    "enabled": true,
                    "status": "active",
                    "bindings": [{
                        "app": app,
                        "providerId": provider_id,
                        "providerType": "qoder_cosy"
                    }],
                    "userGrants": {
                        owner.clone(): {
                            "email": owner,
                            "role": "owner",
                            "active": true
                        }
                    }
                }))
                .unwrap(),
            );
        })
        .await
        .unwrap();
}

fn qoder_headers(share_id: &str, user: &str, session: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    headers.insert(
        "x-cc-switch-share-id",
        HeaderValue::from_str(share_id).unwrap(),
    );
    headers.insert(
        "x-cc-switch-user-email",
        HeaderValue::from_str(user).unwrap(),
    );
    headers.insert(
        "x-cc-switch-session-id",
        HeaderValue::from_str(session).unwrap(),
    );
    headers
}

fn surface_request(app: AppKind, stream_requested: bool) -> (ProxyRoute, Option<String>, Bytes) {
    match app {
        AppKind::Claude => (
            ProxyRoute::ClaudeMessages,
            None,
            Bytes::from(
                json!({
                    "model": MODEL_KEY,
                    "system": "claude-system",
                    "max_tokens": 64,
                    "messages": [{"role": "user", "content": "claude-user"}],
                    "stream": stream_requested
                })
                .to_string(),
            ),
        ),
        AppKind::Codex => (
            ProxyRoute::CodexResponses,
            None,
            Bytes::from(
                json!({
                    "model": MODEL_KEY,
                    "instructions": "codex-system",
                    "input": "codex-user",
                    "stream": stream_requested
                })
                .to_string(),
            ),
        ),
        AppKind::Gemini => (
            ProxyRoute::Gemini,
            Some(format!(
                "models/{MODEL_KEY}:{}",
                if stream_requested {
                    "streamGenerateContent"
                } else {
                    "generateContent"
                }
            )),
            Bytes::from(
                json!({
                    "systemInstruction": {"parts": [{"text": "gemini-system"}]},
                    "contents": [{"role": "user", "parts": [{"text": "gemini-user"}]}]
                })
                .to_string(),
            ),
        ),
    }
}

async fn forward_qoder_surface(
    state: ServerState,
    app: AppKind,
    provider_id: String,
    share_id: &str,
    user: &str,
    session: &str,
    stream_requested: bool,
) -> Result<Response, ProxyError> {
    let (route, gemini_path, body) = surface_request(app, stream_requested);
    super::forward_for_test_surface(
        state,
        route,
        provider_id,
        gemini_path,
        qoder_headers(share_id, user, session),
        body,
    )
    .await
}

async fn collect(response: Response) -> Vec<u8> {
    axum::body::to_bytes(response.into_body(), 4 * 1024 * 1024)
        .await
        .unwrap()
        .to_vec()
}

fn run_qoder_async_test<F>(future: F)
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    std::thread::Builder::new()
        .name("qoder-http-fixture".to_string())
        .stack_size(16 * 1024 * 1024)
        .spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(future);
        })
        .unwrap()
        .join()
        .unwrap();
}

fn assert_signed_request(observation: &QoderRequestObservation, expected_signature_path: &str) {
    let authorization = observation.headers.get("authorization").unwrap();
    let parts = authorization
        .strip_prefix("Bearer COSY.")
        .unwrap()
        .split('.')
        .collect::<Vec<_>>();
    assert_eq!(parts.len(), 2);
    let cosy_key = observation.headers.get("cosy-key").unwrap();
    let cosy_date = observation.headers.get("cosy-date").unwrap();
    let body = serde_json::to_vec(observation.decoded_body.as_ref().unwrap()).unwrap();
    let encoded = crate::domain::qoder::qoder_encode(&body);
    assert_eq!(
        parts[1],
        sign_qoder_request(
            parts[0],
            cosy_key,
            cosy_date,
            &encoded,
            expected_signature_path
        )
    );
    assert_eq!(
        observation.headers.get("cosy-bodyhash").map(String::as_str),
        Some(md5_hex(encoded.as_bytes()).as_str())
    );
    assert_eq!(
        observation
            .headers
            .get("cosy-bodylength")
            .and_then(|value| value.parse::<usize>().ok()),
        Some(encoded.len())
    );
}

fn assert_model_list_observation(observation: &QoderRequestObservation) {
    assert_eq!(observation.uri, QODER_MODEL_LIST_PATH);
    assert_eq!(
        observation.headers.get("cosy-sigpath").map(String::as_str),
        Some(QODER_MODEL_LIST_SIGNATURE_PATH)
    );
    assert!(observation
        .headers
        .get("authorization")
        .is_some_and(|value| value.starts_with("Bearer COSY.")));
}

fn assert_generation_observation(observation: &QoderRequestObservation) {
    assert_eq!(observation.uri, QODER_GENERATION_PATH);
    assert_eq!(
        observation.headers.get("cosy-sigpath").map(String::as_str),
        Some(QODER_GENERATION_SIGNATURE_PATH)
    );
    assert_eq!(
        observation.headers.get("x-model-key").map(String::as_str),
        Some(MODEL_KEY)
    );
    assert_eq!(
        observation
            .headers
            .get("x-model-source")
            .map(String::as_str),
        Some(MODEL_SOURCE)
    );
    assert_eq!(
        observation.headers.get("user-agent").map(String::as_str),
        Some(crate::domain::qoder::QODER_COSY_USER_AGENT)
    );
    assert_eq!(
        observation.headers.get("cosy-version").map(String::as_str),
        Some("1.24.2")
    );
    let body = observation.decoded_body.as_ref().unwrap();
    let mut expected_model_config = model_config();
    expected_model_config["max_input_tokens"] = json!(180_000);
    assert_eq!(body["model_config"], expected_model_config);
    assert_eq!(
        body["chat_context"]["extra"]["modelConfig"]["key"],
        MODEL_KEY
    );
    assert_eq!(
        body["chat_context"]["extra"]["modelConfig"]["source"],
        MODEL_SOURCE
    );
    assert_eq!(
        body["chat_context"]["extra"]["modelConfig"]["max_input_tokens"],
        180_000
    );
    assert!(body["parameters"].get("context_length").is_none());
    assert_eq!(body["stream"], true);
    assert_signed_request(observation, QODER_GENERATION_SIGNATURE_PATH);
}

#[test]
fn qoder_http_fixture_covers_all_credential_rails_and_client_surfaces() {
    run_qoder_async_test(async {
        for rail in [
            QoderFixtureRail::GlobalOauth,
            QoderFixtureRail::CnOauth,
            QoderFixtureRail::Pat,
        ] {
            for app in [AppKind::Claude, AppKind::Codex, AppKind::Gemini] {
                for stream_requested in [false, true] {
                    let name = format!(
                        "qoder-{}-{}-{}",
                        rail.label(),
                        app.as_str(),
                        if stream_requested { "stream" } else { "json" }
                    );
                    let (address, fixture, server) =
                        spawn_qoder_upstream(rail, vec![QoderGenerationReply::Success]).await;
                    let state = qoder_test_state(&name);
                    let origin = format!("http://{address}");
                    let (provider_id, account_id) =
                        install_qoder_provider(&state, &name, app, rail, &origin).await;
                    let share_id = format!("{name}-share");
                    install_qoder_share(&state, &share_id, app, &provider_id, "owner@example.com")
                        .await;

                    let response = forward_qoder_surface(
                        state.clone(),
                        app,
                        provider_id,
                        &share_id,
                        "owner@example.com",
                        "surface-session",
                        stream_requested,
                    )
                    .await
                    .unwrap();
                    assert_eq!(response.status(), StatusCode::OK, "{name}");
                    let body = String::from_utf8(collect(response).await).unwrap();
                    if stream_requested {
                        assert!(body.contains("ready"), "{name}: {body}");
                        assert!(
                            body.contains("message_stop")
                                || body.contains("response.completed")
                                || body.contains("finishReason"),
                            "{name}: {body}"
                        );
                    } else {
                        let body: Value = serde_json::from_str(&body).unwrap();
                        match app {
                            AppKind::Claude => assert_eq!(body["content"][0]["text"], "ready"),
                            AppKind::Codex => {
                                assert_eq!(body["object"], "response");
                                assert_eq!(body["status"], "completed");
                            }
                            AppKind::Gemini => {
                                assert_eq!(
                                    body["candidates"][0]["content"]["parts"][0]["text"],
                                    "ready"
                                )
                            }
                        }
                    }

                    assert_eq!(fixture.count(&fixture.generation_requests), 1, "{name}");
                    assert_eq!(fixture.count(&fixture.model_requests), 1, "{name}");
                    assert_eq!(
                        fixture.count(&fixture.pat_exchanges),
                        usize::from(rail == QoderFixtureRail::Pat),
                        "{name}"
                    );
                    let observations = fixture.observations.lock().unwrap().clone();
                    assert_model_list_observation(&observations[0]);
                    let generations = fixture.generation_observations();
                    assert_eq!(generations.len(), 1);
                    assert_generation_observation(&generations[0]);
                    assert!(generations[0].decoded_body.as_ref().unwrap()["session_id"]
                        .as_str()
                        .is_some_and(|value| !value.is_empty()));
                    let account = state.find_account_by_id(&account_id).await.unwrap();
                    if rail == QoderFixtureRail::Pat {
                        assert_eq!(account.api_key.as_deref(), Some("pt-fixture"));
                        assert!(account.access_token.is_none());
                        assert!(account.refresh_token.is_none());
                    }
                    server.abort();
                }
            }
        }
    });
}

#[test]
fn qoder_nonstream_replays_only_prebusiness_auth_once_on_the_same_account() {
    run_qoder_async_test(async {
        for (rail, failure) in [
            (
                QoderFixtureRail::GlobalOauth,
                QoderGenerationReply::HttpUnauthorized,
            ),
            (
                QoderFixtureRail::CnOauth,
                QoderGenerationReply::EmbeddedUnauthorized,
            ),
            (
                QoderFixtureRail::Pat,
                QoderGenerationReply::HttpUnauthorized,
            ),
            (
                QoderFixtureRail::Pat,
                QoderGenerationReply::EmbeddedUnauthorized,
            ),
        ] {
            let name = format!("qoder-replay-{}-{failure:?}", rail.label());
            let (address, fixture, server) =
                spawn_qoder_upstream(rail, vec![failure, QoderGenerationReply::Success]).await;
            let state = qoder_test_state(&name);
            let origin = format!("http://{address}");
            let (provider_id, account_id) =
                install_qoder_provider(&state, &name, AppKind::Codex, rail, &origin).await;
            let share_id = format!("{name}-share");
            install_qoder_share(
                &state,
                &share_id,
                AppKind::Codex,
                &provider_id,
                "owner@example.com",
            )
            .await;

            let response = forward_qoder_surface(
                state.clone(),
                AppKind::Codex,
                provider_id,
                &share_id,
                "owner@example.com",
                "replay-session",
                false,
            )
            .await
            .unwrap();
            assert_eq!(response.status(), StatusCode::OK, "{name}");
            let body: Value = serde_json::from_slice(&collect(response).await).unwrap();
            assert_eq!(body["status"], "completed", "{name}: {body}");
            assert_eq!(fixture.count(&fixture.generation_requests), 2, "{name}");
            assert_eq!(fixture.count(&fixture.model_requests), 2, "{name}");
            assert_eq!(
                fixture.count(&fixture.pat_exchanges),
                if rail == QoderFixtureRail::Pat { 2 } else { 0 },
                "{name}"
            );
            assert_eq!(
                fixture.count(&fixture.refresh_requests),
                if rail == QoderFixtureRail::Pat { 0 } else { 1 },
                "{name}"
            );
            let generations = fixture.generation_observations();
            assert_eq!(generations.len(), 2);
            let first_session = &generations[0].decoded_body.as_ref().unwrap()["session_id"];
            let second_session = &generations[1].decoded_body.as_ref().unwrap()["session_id"];
            if rail == QoderFixtureRail::Pat {
                assert_eq!(
                    first_session, second_session,
                    "PAT re-exchange must preserve the account credential generation"
                );
            } else {
                assert_ne!(
                    first_session, second_session,
                    "rotated OAuth credentials must fence the new COSY conversation"
                );
            }
            assert!(generations.iter().all(|observation| {
                observation.headers.get("cosy-user").map(String::as_str) == Some("fixture-user")
            }));
            let account = state.find_account_by_id(&account_id).await.unwrap();
            assert_eq!(account.auth_identity_generation, 1);
            assert_eq!(
                account.raw.as_ref().and_then(|raw| raw
                    .pointer("/qoderSecrets/machineToken")
                    .and_then(Value::as_str)),
                Some(format!("machine-token-{}", rail.label()).as_str())
            );
            assert_eq!(
                account.raw.as_ref().and_then(|raw| raw
                    .pointer("/testQoderEndpoints/gatewayBaseUrl")
                    .and_then(Value::as_str)),
                Some(origin.as_str())
            );
            server.abort();
        }

        let name = "qoder-nonstream-postbusiness-401";
        let (address, fixture, server) = spawn_qoder_upstream(
            QoderFixtureRail::GlobalOauth,
            vec![QoderGenerationReply::FirstBusinessThenUnauthorized],
        )
        .await;
        let state = qoder_test_state(name);
        let origin = format!("http://{address}");
        let (provider_id, _) = install_qoder_provider(
            &state,
            name,
            AppKind::Codex,
            QoderFixtureRail::GlobalOauth,
            &origin,
        )
        .await;
        let share_id = format!("{name}-share");
        install_qoder_share(
            &state,
            &share_id,
            AppKind::Codex,
            &provider_id,
            "owner@example.com",
        )
        .await;

        let error = forward_qoder_surface(
            state,
            AppKind::Codex,
            provider_id,
            &share_id,
            "owner@example.com",
            "postbusiness-session",
            false,
        )
        .await
        .unwrap_err();
        assert_eq!(error.status, StatusCode::UNAUTHORIZED);
        assert_eq!(fixture.count(&fixture.generation_requests), 1);
        assert_eq!(fixture.count(&fixture.model_requests), 1);
        assert_eq!(fixture.count(&fixture.refresh_requests), 0);
        server.abort();
    });
}

#[test]
fn qoder_stream_replays_auth_before_commit_but_never_after_business_output() {
    run_qoder_async_test(async {
        for (rail, failure) in [
            (
                QoderFixtureRail::GlobalOauth,
                QoderGenerationReply::HttpUnauthorized,
            ),
            (
                QoderFixtureRail::Pat,
                QoderGenerationReply::EmbeddedUnauthorized,
            ),
        ] {
            let name = format!("qoder-stream-precommit-{}-{failure:?}", rail.label());
            let (address, fixture, server) =
                spawn_qoder_upstream(rail, vec![failure, QoderGenerationReply::Success]).await;
            let state = qoder_test_state(&name);
            let origin = format!("http://{address}");
            let (provider_id, _) =
                install_qoder_provider(&state, &name, AppKind::Codex, rail, &origin).await;
            let share_id = format!("{name}-share");
            install_qoder_share(
                &state,
                &share_id,
                AppKind::Codex,
                &provider_id,
                "owner@example.com",
            )
            .await;

            let response = forward_qoder_surface(
                state,
                AppKind::Codex,
                provider_id,
                &share_id,
                "owner@example.com",
                "precommit-session",
                true,
            )
            .await
            .unwrap();
            let body = String::from_utf8(collect(response).await).unwrap();
            assert!(body.contains("ready"), "{name}: {body}");
            assert_eq!(
                body.matches("event: response.completed").count(),
                1,
                "{name}: {body}"
            );
            assert_eq!(fixture.count(&fixture.generation_requests), 2, "{name}");
            assert_eq!(
                fixture.count(&fixture.refresh_requests),
                usize::from(rail != QoderFixtureRail::Pat),
                "{name}"
            );
            assert_eq!(
                fixture.count(&fixture.pat_exchanges),
                if rail == QoderFixtureRail::Pat { 2 } else { 0 },
                "{name}"
            );
            server.abort();
        }

        let name = "qoder-stream-postcommit-401";
        let (address, fixture, server) = spawn_qoder_upstream(
            QoderFixtureRail::GlobalOauth,
            vec![QoderGenerationReply::FirstBusinessThenUnauthorized],
        )
        .await;
        let state = qoder_test_state(name);
        let origin = format!("http://{address}");
        let (provider_id, _) = install_qoder_provider(
            &state,
            name,
            AppKind::Codex,
            QoderFixtureRail::GlobalOauth,
            &origin,
        )
        .await;
        let share_id = format!("{name}-share");
        install_qoder_share(
            &state,
            &share_id,
            AppKind::Codex,
            &provider_id,
            "owner@example.com",
        )
        .await;

        let response = forward_qoder_surface(
            state,
            AppKind::Codex,
            provider_id,
            &share_id,
            "owner@example.com",
            "postcommit-session",
            true,
        )
        .await
        .unwrap();
        let body = String::from_utf8(collect(response).await).unwrap();
        assert!(body.contains("discard-me"), "{body}");
        assert!(body.contains("response.failed"), "{body}");
        assert_eq!(fixture.count(&fixture.generation_requests), 1);
        assert_eq!(fixture.count(&fixture.refresh_requests), 0);
        server.abort();
    });
}

#[test]
fn qoder_112_does_not_refresh_and_115_cools_only_the_bound_account() {
    run_qoder_async_test(async {
        for (reply, expected_status) in [
            (QoderGenerationReply::Entitlement112, StatusCode::FORBIDDEN),
            (
                QoderGenerationReply::AgentLimit115,
                StatusCode::TOO_MANY_REQUESTS,
            ),
        ] {
            let name = format!("qoder-error-{reply:?}");
            let (address, fixture, server) =
                spawn_qoder_upstream(QoderFixtureRail::GlobalOauth, vec![reply]).await;
            let state = qoder_test_state(&name);
            let origin = format!("http://{address}");
            let (provider_id, account_id) = install_qoder_provider(
                &state,
                &name,
                AppKind::Codex,
                QoderFixtureRail::GlobalOauth,
                &origin,
            )
            .await;
            let other_account_id = format!("{name}-other-account");
            let mut accounts = state.accounts_snapshot().await;
            let mut other = accounts
                .find_for_provider(ProviderType::QoderCosy, Some(&account_id))
                .unwrap()
                .clone();
            other.id = other_account_id.clone();
            other.rate_limited_until = None;
            accounts.accounts.push(other);
            state.replace_account_store_for_test(accounts).await;
            let share_id = format!("{name}-share");
            install_qoder_share(
                &state,
                &share_id,
                AppKind::Codex,
                &provider_id,
                "owner@example.com",
            )
            .await;

            let error = forward_qoder_surface(
                state.clone(),
                AppKind::Codex,
                provider_id,
                &share_id,
                "owner@example.com",
                "error-session",
                false,
            )
            .await
            .unwrap_err();
            assert_eq!(error.status, expected_status);
            assert_eq!(fixture.count(&fixture.generation_requests), 1);
            assert_eq!(fixture.count(&fixture.refresh_requests), 0);
            let bound = state.find_account_by_id(&account_id).await.unwrap();
            let other = state.find_account_by_id(&other_account_id).await.unwrap();
            if reply == QoderGenerationReply::AgentLimit115 {
                assert!(bound.rate_limited_until.is_some());
            } else {
                assert!(bound.rate_limited_until.is_none());
            }
            assert!(other.rate_limited_until.is_none());
            server.abort();
        }
    });
}

#[test]
fn qoder_conversation_scope_isolates_share_user_session_and_account_generation() {
    run_qoder_async_test(async {
        let (address, fixture, server) = spawn_qoder_upstream(
            QoderFixtureRail::GlobalOauth,
            vec![QoderGenerationReply::Success],
        )
        .await;
        let state = qoder_test_state("qoder-conversation-isolation");
        let origin = format!("http://{address}");
        let (provider_id, account_id) = install_qoder_provider(
            &state,
            "qoder-conversation-isolation",
            AppKind::Codex,
            QoderFixtureRail::GlobalOauth,
            &origin,
        )
        .await;
        for share in ["qoder-share-a", "qoder-share-b"] {
            install_qoder_share(
                &state,
                share,
                AppKind::Codex,
                &provider_id,
                "owner@example.com",
            )
            .await;
        }
        for (share, user, session) in [
            ("qoder-share-a", "owner@example.com", "session-a"),
            ("qoder-share-a", "owner@example.com", "session-a"),
            ("qoder-share-a", "owner@example.com", "session-b"),
            ("qoder-share-a", "other@example.com", "session-a"),
            ("qoder-share-b", "owner@example.com", "session-a"),
        ] {
            if user == "other@example.com" {
                let share = share.to_string();
                state
                    .mutate_shares_immediate(move |shares| {
                        let share = shares
                            .shares
                            .iter_mut()
                            .find(|item| item.id == share)
                            .unwrap();
                        share.user_grants.insert(
                            "other@example.com".to_string(),
                            serde_json::from_value(json!({
                                "email": "other@example.com",
                                "role": "user",
                                "active": true
                            }))
                            .unwrap(),
                        );
                    })
                    .await
                    .unwrap();
            }
            let response = forward_qoder_surface(
                state.clone(),
                AppKind::Codex,
                provider_id.clone(),
                share,
                user,
                session,
                false,
            )
            .await
            .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let _ = collect(response).await;
        }
        let before = fixture
            .generation_observations()
            .iter()
            .map(|observation| {
                observation.decoded_body.as_ref().unwrap()["session_id"]
                    .as_str()
                    .unwrap()
                    .to_string()
            })
            .collect::<Vec<_>>();
        assert_eq!(before[0], before[1]);
        assert_ne!(before[0], before[2]);
        assert_ne!(before[0], before[3]);
        assert_ne!(before[0], before[4]);

        let mut accounts = state.accounts_snapshot().await;
        let account = accounts
            .accounts
            .iter_mut()
            .find(|account| account.id == account_id)
            .unwrap();
        account.token_refresh_generation += 1;
        state.replace_account_store_for_test(accounts).await;
        let response = forward_qoder_surface(
            state,
            AppKind::Codex,
            provider_id,
            "qoder-share-a",
            "owner@example.com",
            "session-a",
            false,
        )
        .await
        .unwrap();
        let _ = collect(response).await;
        let after = fixture.generation_observations();
        assert_ne!(
            before[0],
            after.last().unwrap().decoded_body.as_ref().unwrap()["session_id"]
        );
        server.abort();
    });
}

#[test]
fn qoder_generation_fences_account_provider_runtime_and_binding_drift_before_outbound() {
    run_qoder_async_test(async {
        for drift in ["account", "provider", "runtime", "binding"] {
            let (address, fixture, server) = spawn_qoder_upstream(
                QoderFixtureRail::GlobalOauth,
                vec![QoderGenerationReply::Success],
            )
            .await;
            let state = qoder_test_state(&format!("qoder-{drift}-drift"));
            let origin = format!("http://{address}");
            let (provider_id, account_id) = install_qoder_provider(
                &state,
                &format!("qoder-{drift}-drift"),
                AppKind::Codex,
                QoderFixtureRail::GlobalOauth,
                &origin,
            )
            .await;
            let share_id = format!("qoder-{drift}-share");
            install_qoder_share(
                &state,
                &share_id,
                AppKind::Codex,
                &provider_id,
                "owner@example.com",
            )
            .await;
            let execution = {
                let providers = state.providers.read().await;
                let stored = providers.providers[0].clone();
                ProviderExecution::from_store(&providers, stored).unwrap()
            };

            match drift {
                "account" => {
                    let mut accounts = state.accounts_snapshot().await;
                    accounts
                        .accounts
                        .iter_mut()
                        .find(|account| account.id == account_id)
                        .unwrap()
                        .auth_identity_generation += 1;
                    state.replace_account_store_for_test(accounts).await;
                }
                "provider" => {
                    let mut providers = state.providers_snapshot().await;
                    providers.providers[0].resource.revision += 1;
                    state.replace_provider_store_for_test(providers).await;
                }
                "runtime" => {
                    let mut providers = state.providers_snapshot().await;
                    let mut plan = providers
                        .runtime_plan(AppKind::Codex, &provider_id)
                        .unwrap()
                        .as_ref()
                        .clone();
                    plan.runtime_fingerprint = "qoder-drifted-runtime".to_string();
                    Arc::make_mut(&mut providers.runtime_index).insert_plan_for_test(plan);
                    state.replace_provider_store_for_test(providers).await;
                }
                "binding" => {
                    let mut providers = state.providers_snapshot().await;
                    providers.providers[0]
                        .provider
                        .meta
                        .as_mut()
                        .unwrap()
                        .auth_binding
                        .as_mut()
                        .unwrap()
                        .account_id = Some("other-account".to_string());
                    let accounts = state.accounts_snapshot().await;
                    providers.rebuild_runtime_index(&accounts).unwrap();
                    state.replace_provider_store_for_test(providers).await;
                }
                _ => unreachable!(),
            }

            let result = forward_with_attempt(
                state,
                ProxyRoute::CodexResponses,
                None,
                qoder_headers(&share_id, "owner@example.com", "drift-session"),
                surface_request(AppKind::Codex, false).2,
                ForwardAttemptContext {
                    execution: Some(execution),
                    provider_binding_pinned: true,
                    ..ForwardAttemptContext::default()
                },
            )
            .await;
            assert!(result.is_err(), "{drift} drift must be fenced");
            assert_eq!(fixture.count(&fixture.model_requests), 0, "{drift}");
            assert_eq!(fixture.count(&fixture.generation_requests), 0, "{drift}");
            server.abort();
        }
    });
}
