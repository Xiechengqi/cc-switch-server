use super::*;

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::Response;
use axum::routing::{get, post};
use bytes::Bytes;
use futures_util::StreamExt;
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
    ToolCall,
    TerminalThenUnauthorized,
    UsageThenReadFailure,
    LargeAggregate,
    LargeToolStream,
    HttpUnauthorized,
    OversizedHttpUnauthorized,
    EmbeddedUnauthorized,
    FirstBusinessThenUnauthorized,
    MalformedWrapper,
    InvalidToolIndex,
    ConflictingToolAlias,
    DuplicateChoice,
    HeartbeatOnly,
    InvalidSemanticText,
    InvalidUsageTotal,
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

fn tool_call_stream() -> Response {
    let call = json!({
        "id": "chat-qoder-fixture",
        "object": "chat.completion.chunk",
        "created": 1,
        "model": MODEL_KEY,
        "choices": [{
            "index": 0,
            "delta": {"tool_calls": [{
                "index": 0,
                "id": "call_lookup",
                "type": "function",
                "function": {"name": "lookup", "arguments": "{\"key\":\"one\"}"}
            }]},
            "finish_reason": "tool_calls"
        }]
    });
    let body = qoder_wrapper(json!("OK"), call);
    Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, "text/event-stream")
        .body(Body::from(body))
        .unwrap()
}

fn terminal_then_unauthorized_stream() -> Response {
    let chunks = vec![
        Ok::<_, std::convert::Infallible>(qoder_wrapper(
            json!("OK"),
            inner_success_chunk(Some("ready"), false),
        )),
        Ok(qoder_wrapper(json!("OK"), inner_success_chunk(None, true))),
        Ok(qoder_wrapper(
            json!("UNAUTHORIZED"),
            json!({"message": "late error after terminal"}),
        )),
    ];
    Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, "text/event-stream")
        .body(Body::from_stream(futures_util::stream::iter(chunks)))
        .unwrap()
}

fn usage_then_read_failure_stream() -> Response {
    let mut usage = inner_success_chunk(None, false);
    usage["usage"] = json!({
        "prompt_tokens": 2,
        "completion_tokens": 1,
        "total_tokens": 3
    });
    let chunks = vec![
        Ok(qoder_wrapper(
            json!("OK"),
            inner_success_chunk(Some("ready"), false),
        )),
        Ok(qoder_wrapper(json!("OK"), usage)),
        Err(std::io::Error::other("fixture upstream read failure")),
    ];
    Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, "text/event-stream")
        .body(Body::from_stream(
            futures_util::stream::iter(chunks)
                .enumerate()
                .then(|(index, chunk)| async move {
                    if index == 2 {
                        tokio::time::sleep(Duration::from_millis(25)).await;
                    }
                    chunk
                }),
        ))
        .unwrap()
}

fn large_aggregate_stream() -> Response {
    let content = "a".repeat(4 * 1024);
    let mut chunks = (0..96)
        .map(|_| {
            Ok::<_, std::convert::Infallible>(qoder_wrapper(
                json!("OK"),
                inner_success_chunk(Some(&content), false),
            ))
        })
        .collect::<Vec<_>>();
    chunks.push(Ok(qoder_wrapper(
        json!("OK"),
        inner_success_chunk(None, true),
    )));
    Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, "text/event-stream")
        .body(Body::from_stream(futures_util::stream::iter(chunks)))
        .unwrap()
}

fn tool_argument_chunk(fragment: &str, first: bool, terminal: bool) -> Value {
    let tool_call = if first {
        json!({
            "index": 0,
            "id": "call_memory",
            "type": "function",
            "function": {"name": "bounded_tool", "arguments": fragment}
        })
    } else {
        json!({"index": 0, "function": {"arguments": fragment}})
    };
    json!({
        "id": "chat-qoder-memory",
        "object": "chat.completion.chunk",
        "created": 1,
        "model": MODEL_KEY,
        "choices": [{
            "index": 0,
            "delta": {"tool_calls": [tool_call]},
            "finish_reason": terminal.then_some("tool_calls")
        }]
    })
}

fn large_tool_stream() -> Response {
    let fragment = "x".repeat(2 * 1024);
    let mut chunks = (0..128)
        .map(|index| {
            Ok::<_, std::convert::Infallible>(qoder_wrapper(
                json!("OK"),
                tool_argument_chunk(&fragment, index == 0, false),
            ))
        })
        .collect::<Vec<_>>();
    chunks.push(Ok(qoder_wrapper(
        json!("OK"),
        tool_argument_chunk("", false, true),
    )));
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
        QoderGenerationReply::ToolCall => tool_call_stream(),
        QoderGenerationReply::TerminalThenUnauthorized => terminal_then_unauthorized_stream(),
        QoderGenerationReply::UsageThenReadFailure => usage_then_read_failure_stream(),
        QoderGenerationReply::LargeAggregate => large_aggregate_stream(),
        QoderGenerationReply::LargeToolStream => large_tool_stream(),
        QoderGenerationReply::HttpUnauthorized => Response::builder()
            .status(StatusCode::UNAUTHORIZED)
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(json!({"message": "expired"}).to_string()))
            .unwrap(),
        QoderGenerationReply::OversizedHttpUnauthorized => Response::builder()
            .status(StatusCode::UNAUTHORIZED)
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({"message": "x".repeat(512 * 1024)}).to_string(),
            ))
            .unwrap(),
        QoderGenerationReply::EmbeddedUnauthorized => {
            embedded_error("UNAUTHORIZED", json!({"message": "embedded expired"}))
        }
        QoderGenerationReply::FirstBusinessThenUnauthorized => business_then_unauthorized(),
        QoderGenerationReply::MalformedWrapper => Response::builder()
            .status(StatusCode::OK)
            .header(CONTENT_TYPE, "text/event-stream")
            .body(Body::from("data: null\n\ndata: [DONE]\n\n"))
            .unwrap(),
        QoderGenerationReply::InvalidToolIndex => Response::builder()
            .status(StatusCode::OK)
            .header(CONTENT_TYPE, "text/event-stream")
            .body(Body::from(qoder_wrapper(
                json!("OK"),
                json!({"choices":[{"index":0,"delta":{"tool_calls":[
                    {"index":null,"id":"call_1","type":"function","function":{"name":"lookup","arguments":"{}"}}
                ]},"finish_reason":"tool_calls"}]}),
            )))
            .unwrap(),
        QoderGenerationReply::ConflictingToolAlias => Response::builder()
            .status(StatusCode::OK)
            .header(CONTENT_TYPE, "text/event-stream")
            .body(Body::from(qoder_wrapper(
                json!("OK"),
                json!({"choices":[{"index":0,"delta":{"tool_calls":[
                    {"index":0,"id":"call-a","call_id":"call-b","function":{"name":"lookup","arguments":"{}"}}
                ]},"finish_reason":"tool_calls"}]}),
            )))
            .unwrap(),
        QoderGenerationReply::DuplicateChoice => Response::builder()
            .status(StatusCode::OK)
            .header(CONTENT_TYPE, "text/event-stream")
            .body(Body::from(qoder_wrapper(
                json!("OK"),
                json!({"choices": [
                    {"index": 0, "delta": {"content": "left"}, "finish_reason": null},
                    {"index": 0, "delta": {"content": "right"}, "finish_reason": "stop"}
                ]}),
            )))
            .unwrap(),
        QoderGenerationReply::HeartbeatOnly => {
            let mut body = qoder_wrapper(
                json!("OK"),
                json!({"choices":[{"index":0,"delta":{},"finish_reason":null}]}),
            )
            .to_vec();
            body.extend_from_slice(b"data: [DONE]\n\n");
            Response::builder()
                .status(StatusCode::OK)
                .header(CONTENT_TYPE, "text/event-stream")
                .body(Body::from(body))
                .unwrap()
        }
        QoderGenerationReply::InvalidSemanticText => Response::builder()
            .status(StatusCode::OK)
            .header(CONTENT_TYPE, "text/event-stream")
            .body(Body::from(qoder_wrapper(
                json!("OK"),
                json!({"choices":[{"index":0,"delta":{"reasoning_content":42},"finish_reason":"stop"}]}),
            )))
            .unwrap(),
        QoderGenerationReply::InvalidUsageTotal => {
            let mut chunk = inner_success_chunk(Some("ready"), true);
            chunk["usage"] = json!({
                "prompt_tokens": 4,
                "completion_tokens": 3,
                "total_tokens": 2
            });
            Response::builder()
                .status(StatusCode::OK)
                .header(CONTENT_TYPE, "text/event-stream")
                .body(Body::from(qoder_wrapper(json!("OK"), chunk)))
                .unwrap()
        }
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

fn qoder_responses_id(body: &str, stream_requested: bool) -> String {
    if stream_requested {
        body.lines()
            .filter_map(|line| line.strip_prefix("data: "))
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .find(|event| event["type"] == "response.completed")
            .and_then(|event| event["response"]["id"].as_str().map(str::to_string))
            .unwrap()
    } else {
        serde_json::from_str::<Value>(body).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string()
    }
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
fn qoder_unknown_responses_continuation_never_reaches_generation() {
    run_qoder_async_test(async {
        for rail in [
            QoderFixtureRail::GlobalOauth,
            QoderFixtureRail::CnOauth,
            QoderFixtureRail::Pat,
        ] {
            for stream_requested in [false, true] {
                let name = format!(
                    "qoder-unknown-continuation-{}-{stream_requested}",
                    rail.label()
                );
                let (address, fixture, server) =
                    spawn_qoder_upstream(rail, vec![QoderGenerationReply::Success]).await;
                let state = qoder_test_state(&name);
                let (provider_id, _) = install_qoder_provider(
                    &state,
                    &name,
                    AppKind::Codex,
                    rail,
                    &format!("http://{address}"),
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
                let body = Bytes::from(
                    json!({
                        "model": MODEL_KEY,
                        "input": "follow up",
                        "previous_response_id": "resp_unknown",
                        "stream": stream_requested
                    })
                    .to_string(),
                );
                let error = super::forward_for_test_surface(
                    state,
                    ProxyRoute::CodexResponses,
                    provider_id,
                    None,
                    qoder_headers(&share_id, "owner@example.com", "continuation-session"),
                    body,
                )
                .await
                .unwrap_err();
                assert_eq!(error.status, StatusCode::CONFLICT, "{name}");
                assert_eq!(fixture.count(&fixture.generation_requests), 0, "{name}");
                assert_eq!(fixture.count(&fixture.refresh_requests), 0, "{name}");
                server.abort();
            }
        }
    });
}

#[test]
fn qoder_responses_ids_are_unique_across_requests_and_stream_modes() {
    run_qoder_async_test(async {
        for rail in [
            QoderFixtureRail::GlobalOauth,
            QoderFixtureRail::CnOauth,
            QoderFixtureRail::Pat,
        ] {
            for stream_requested in [false, true] {
                let name = format!("qoder-response-id-{}-{stream_requested}", rail.label());
                let (address, fixture, server) =
                    spawn_qoder_upstream(rail, vec![QoderGenerationReply::Success]).await;
                let state = qoder_test_state(&name);
                let (provider_id, _) = install_qoder_provider(
                    &state,
                    &name,
                    AppKind::Codex,
                    rail,
                    &format!("http://{address}"),
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
                let mut response_ids = Vec::new();
                for _ in 0..2 {
                    let response = forward_qoder_surface(
                        state.clone(),
                        AppKind::Codex,
                        provider_id.clone(),
                        &share_id,
                        "owner@example.com",
                        "response-id-session",
                        stream_requested,
                    )
                    .await
                    .unwrap();
                    assert_eq!(response.status(), StatusCode::OK);
                    let body = String::from_utf8(collect(response).await).unwrap();
                    let id = qoder_responses_id(&body, stream_requested);
                    assert!(id.starts_with("resp_"), "{name}: {id}");
                    assert!(!id.contains("chat-qoder-fixture"), "{name}: {id}");
                    response_ids.push(id);
                }
                assert_ne!(response_ids[0], response_ids[1], "{name}");
                assert_eq!(fixture.count(&fixture.generation_requests), 2, "{name}");
                let generations = fixture.generation_observations();
                assert_ne!(
                    generations[0].decoded_body.as_ref().unwrap()["session_id"],
                    generations[1].decoded_body.as_ref().unwrap()["session_id"],
                    "independent Responses calls cannot append to the same COSY session"
                );
                server.abort();
            }
        }
    });
}

#[test]
fn qoder_responses_previous_id_reuses_only_the_bound_cosy_session() {
    run_qoder_async_test(async {
        for rail in [
            QoderFixtureRail::GlobalOauth,
            QoderFixtureRail::CnOauth,
            QoderFixtureRail::Pat,
        ] {
            for first_stream in [false, true] {
                for second_stream in [false, true] {
                    let name = format!(
                        "qoder-responses-continuation-{}-{first_stream}-{second_stream}",
                        rail.label()
                    );
                    let (address, fixture, server) =
                        spawn_qoder_upstream(rail, vec![QoderGenerationReply::Success]).await;
                    let state = qoder_test_state(&name);
                    let (provider_id, _) = install_qoder_provider(
                        &state,
                        &name,
                        AppKind::Codex,
                        rail,
                        &format!("http://{address}"),
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
                    let first = forward_qoder_surface(
                        state.clone(),
                        AppKind::Codex,
                        provider_id.clone(),
                        &share_id,
                        "owner@example.com",
                        "same-downstream-session",
                        first_stream,
                    )
                    .await
                    .unwrap();
                    let first_body = String::from_utf8(collect(first).await).unwrap();
                    let first_id = qoder_responses_id(&first_body, first_stream);
                    let second = super::forward_for_test_surface(
                        state.clone(),
                        ProxyRoute::CodexResponses,
                        provider_id.clone(),
                        None,
                        qoder_headers(&share_id, "owner@example.com", "same-downstream-session"),
                        Bytes::from(
                            json!({
                                "model": MODEL_KEY,
                                "input": "follow up",
                                "previous_response_id": first_id,
                                "stream": second_stream
                            })
                            .to_string(),
                        ),
                    )
                    .await
                    .unwrap();
                    assert_eq!(second.status(), StatusCode::OK, "{name}");
                    let second_body = String::from_utf8(collect(second).await).unwrap();
                    let second_id = qoder_responses_id(&second_body, second_stream);
                    assert_ne!(first_id, second_id, "{name}");
                    let generations = fixture.generation_observations();
                    assert_eq!(generations.len(), 2, "{name}");
                    let first_wire = generations[0].decoded_body.as_ref().unwrap();
                    let second_wire = generations[1].decoded_body.as_ref().unwrap();
                    assert_eq!(
                        first_wire["session_id"], second_wire["session_id"],
                        "{name}"
                    );
                    assert_eq!(
                        second_wire["messages"].as_array().unwrap().len(),
                        1,
                        "{name}"
                    );
                    assert_eq!(second_wire["messages"][0]["role"], "user", "{name}");
                    assert_eq!(
                        second_wire["chat_context"]["text"]["text"], "follow up",
                        "{name}"
                    );
                    let replay = super::forward_for_test_surface(
                        state,
                        ProxyRoute::CodexResponses,
                        provider_id,
                        None,
                        qoder_headers(&share_id, "owner@example.com", "same-downstream-session"),
                        Bytes::from(
                            json!({
                                "model": MODEL_KEY,
                                "input": "duplicate follow up",
                                "previous_response_id": first_id,
                                "stream": false
                            })
                            .to_string(),
                        ),
                    )
                    .await
                    .unwrap_err();
                    assert_eq!(replay.status, StatusCode::CONFLICT, "{name}");
                    assert_eq!(fixture.count(&fixture.generation_requests), 2, "{name}");
                    server.abort();
                }
            }
        }
    });
}

#[test]
fn qoder_responses_concurrent_followups_claim_one_alias_once() {
    run_qoder_async_test(async {
        let (address, fixture, server) = spawn_qoder_upstream(
            QoderFixtureRail::GlobalOauth,
            vec![QoderGenerationReply::Success],
        )
        .await;
        let state = qoder_test_state("qoder-concurrent-followups");
        let (provider_id, _) = install_qoder_provider(
            &state,
            "qoder-concurrent-followups",
            AppKind::Codex,
            QoderFixtureRail::GlobalOauth,
            &format!("http://{address}"),
        )
        .await;
        let share_id = "qoder-concurrent-followups-share";
        install_qoder_share(
            &state,
            share_id,
            AppKind::Codex,
            &provider_id,
            "owner@example.com",
        )
        .await;
        let first = forward_qoder_surface(
            state.clone(),
            AppKind::Codex,
            provider_id.clone(),
            share_id,
            "owner@example.com",
            "concurrent-session",
            false,
        )
        .await
        .unwrap();
        let first_body = String::from_utf8(collect(first).await).unwrap();
        let response_id = qoder_responses_id(&first_body, false);
        let followup = |text: &'static str| {
            super::forward_for_test_surface(
                state.clone(),
                ProxyRoute::CodexResponses,
                provider_id.clone(),
                None,
                qoder_headers(share_id, "owner@example.com", "concurrent-session"),
                Bytes::from(
                    json!({
                        "model": MODEL_KEY,
                        "input": text,
                        "previous_response_id": response_id,
                        "stream": false
                    })
                    .to_string(),
                ),
            )
        };
        let (left, right) = tokio::join!(followup("left"), followup("right"));
        let successes = usize::from(left.is_ok()) + usize::from(right.is_ok());
        assert_eq!(successes, 1);
        let failure = if let Err(error) = left {
            error
        } else {
            right.unwrap_err()
        };
        assert_eq!(failure.status, StatusCode::CONFLICT);
        assert_eq!(fixture.count(&fixture.generation_requests), 2);
        server.abort();
    });
}

#[test]
fn qoder_responses_continuation_pairs_prior_tool_call_with_new_result() {
    run_qoder_async_test(async {
        for rail in [
            QoderFixtureRail::GlobalOauth,
            QoderFixtureRail::CnOauth,
            QoderFixtureRail::Pat,
        ] {
            for first_stream in [false, true] {
                let name = format!("qoder-tool-continuation-{}-{first_stream}", rail.label());
                let (address, fixture, server) = spawn_qoder_upstream(
                    rail,
                    vec![
                        QoderGenerationReply::ToolCall,
                        QoderGenerationReply::Success,
                    ],
                )
                .await;
                let state = qoder_test_state(&name);
                let (provider_id, _) = install_qoder_provider(
                    &state,
                    &name,
                    AppKind::Codex,
                    rail,
                    &format!("http://{address}"),
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
                let first = super::forward_for_test_surface(
                    state.clone(),
                    ProxyRoute::CodexResponses,
                    provider_id.clone(),
                    None,
                    qoder_headers(&share_id, "owner@example.com", "tool-session"),
                    Bytes::from(
                        json!({
                            "model": MODEL_KEY,
                            "input": "look up one",
                            "tools": [{
                                "type": "function",
                                "name": "lookup",
                                "parameters": {"type": "object", "properties": {"key": {"type": "string"}}}
                            }],
                            "stream": first_stream
                        })
                        .to_string(),
                    ),
                )
                .await
                .unwrap();
                assert_eq!(first.status(), StatusCode::OK, "{name}");
                let first_body = String::from_utf8(collect(first).await).unwrap();
                let first_id = qoder_responses_id(&first_body, first_stream);
                let second = super::forward_for_test_surface(
                    state,
                    ProxyRoute::CodexResponses,
                    provider_id,
                    None,
                    qoder_headers(&share_id, "owner@example.com", "tool-session"),
                    Bytes::from(
                        json!({
                            "model": MODEL_KEY,
                            "input": [{
                                "type": "function_call_output",
                                "call_id": "call_lookup",
                                "output": "result-one"
                            }],
                            "previous_response_id": first_id,
                            "stream": false
                        })
                        .to_string(),
                    ),
                )
                .await
                .unwrap();
                assert_eq!(second.status(), StatusCode::OK, "{name}");
                let second_body: Value = serde_json::from_slice(&collect(second).await).unwrap();
                assert_eq!(second_body["status"], "completed", "{name}");
                let generations = fixture.generation_observations();
                assert_eq!(generations.len(), 2, "{name}");
                let first_wire = generations[0].decoded_body.as_ref().unwrap();
                let second_wire = generations[1].decoded_body.as_ref().unwrap();
                assert_eq!(
                    first_wire["session_id"], second_wire["session_id"],
                    "{name}"
                );
                assert_eq!(
                    second_wire["messages"].as_array().unwrap().len(),
                    1,
                    "{name}"
                );
                assert_eq!(second_wire["messages"][0]["role"], "tool", "{name}");
                assert_eq!(
                    second_wire["messages"][0]["tool_call_id"], "call_lookup",
                    "{name}"
                );
                assert_eq!(second_wire["messages"][0]["name"], "lookup", "{name}");
                assert_eq!(second_wire["tools"], json!([]), "{name}");
                server.abort();
            }
        }
    });
}

#[test]
fn qoder_cancelled_responses_stream_does_not_publish_continuation() {
    run_qoder_async_test(async {
        for rail in [
            QoderFixtureRail::GlobalOauth,
            QoderFixtureRail::CnOauth,
            QoderFixtureRail::Pat,
        ] {
            let name = format!("qoder-cancelled-continuation-{}", rail.label());
            let (address, fixture, server) =
                spawn_qoder_upstream(rail, vec![QoderGenerationReply::Success]).await;
            let state = qoder_test_state(&name);
            let (provider_id, _) = install_qoder_provider(
                &state,
                &name,
                AppKind::Codex,
                rail,
                &format!("http://{address}"),
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
                state.clone(),
                AppKind::Codex,
                provider_id.clone(),
                &share_id,
                "owner@example.com",
                "cancelled-session",
                true,
            )
            .await
            .unwrap();
            let mut stream = response.into_body().into_data_stream();
            let first = stream.next().await.unwrap().unwrap();
            let first = std::str::from_utf8(&first).unwrap();
            let response_id = first
                .lines()
                .filter_map(|line| line.strip_prefix("data: "))
                .filter_map(|line| serde_json::from_str::<Value>(line).ok())
                .find(|event| event["type"] == "response.created")
                .and_then(|event| event["response"]["id"].as_str().map(str::to_string))
                .unwrap();
            drop(stream);
            let error = super::forward_for_test_surface(
                state,
                ProxyRoute::CodexResponses,
                provider_id,
                None,
                qoder_headers(&share_id, "owner@example.com", "cancelled-session"),
                Bytes::from(
                    json!({
                        "model": MODEL_KEY,
                        "input": "follow up",
                        "previous_response_id": response_id,
                        "stream": false
                    })
                    .to_string(),
                ),
            )
            .await
            .unwrap_err();
            assert_eq!(error.status, StatusCode::CONFLICT, "{name}");
            assert_eq!(fixture.count(&fixture.generation_requests), 1, "{name}");
            server.abort();
        }
    });
}

#[test]
fn qoder_final_terminal_chunk_commits_and_allows_immediate_continuation() {
    run_qoder_async_test(async {
        for rail in [
            QoderFixtureRail::GlobalOauth,
            QoderFixtureRail::CnOauth,
            QoderFixtureRail::Pat,
        ] {
            for app in [AppKind::Claude, AppKind::Codex] {
                let name = format!(
                    "qoder-final-terminal-drop-{}-{}",
                    rail.label(),
                    app.as_str()
                );
                let (address, fixture, server) =
                    spawn_qoder_upstream(rail, vec![QoderGenerationReply::Success]).await;
                let state = qoder_test_state(&name);
                let (provider_id, account_id) =
                    install_qoder_provider(&state, &name, app, rail, &format!("http://{address}"))
                        .await;
                let share_id = format!("{name}-share");
                install_qoder_share(&state, &share_id, app, &provider_id, "owner@example.com")
                    .await;
                let response = forward_qoder_surface(
                    state.clone(),
                    app,
                    provider_id.clone(),
                    &share_id,
                    "owner@example.com",
                    "final-terminal-session",
                    true,
                )
                .await
                .unwrap();
                let terminal = match app {
                    AppKind::Claude => "message_stop",
                    AppKind::Codex => "response.completed",
                    AppKind::Gemini => unreachable!("Gemini finishReason may precede EOF"),
                };
                let mut body = response.into_body().into_data_stream();
                let terminal_chunk = tokio::time::timeout(Duration::from_secs(1), async {
                    loop {
                        let chunk = body
                            .next()
                            .await
                            .expect("Qoder stream ended before terminal")
                            .unwrap();
                        if String::from_utf8_lossy(&chunk).contains(terminal) {
                            break chunk;
                        }
                    }
                })
                .await
                .expect("Qoder terminal chunk must arrive");
                drop(body);
                tokio::time::timeout(Duration::from_secs(1), async {
                    loop {
                        let done = state.usage_snapshot().await.logs.last().is_some_and(|log| {
                            log.stream_status.as_deref() == Some("completed")
                                && log.outcome == UsageOutcome::Success
                        });
                        if done
                            && !state.share_in_flight.has_in_flight(&share_id)
                            && state
                                .account_in_flight
                                .snapshot()
                                .current(ProviderType::QoderCosy, &account_id)
                                == 0
                        {
                            break;
                        }
                        tokio::task::yield_now().await;
                    }
                })
                .await
                .unwrap_or_else(|_| {
                    panic!("{name}: a delivered final terminal was not recorded as success")
                });
                if app == AppKind::Codex {
                    let terminal_text = std::str::from_utf8(&terminal_chunk).unwrap();
                    let response_id = qoder_responses_id(terminal_text, true);
                    let second = super::forward_for_test_surface(
                        state,
                        ProxyRoute::CodexResponses,
                        provider_id,
                        None,
                        qoder_headers(&share_id, "owner@example.com", "final-terminal-session"),
                        Bytes::from(
                            json!({
                                "model": MODEL_KEY,
                                "input": "follow up",
                                "previous_response_id": response_id,
                                "stream": false
                            })
                            .to_string(),
                        ),
                    )
                    .await
                    .unwrap();
                    assert_eq!(second.status(), StatusCode::OK, "{name}");
                    let second_body: Value =
                        serde_json::from_slice(&collect(second).await).unwrap();
                    assert_eq!(second_body["status"], "completed", "{name}");
                }
                assert_eq!(
                    fixture.count(&fixture.generation_requests),
                    if app == AppKind::Codex { 2 } else { 1 },
                    "{name}"
                );
                server.abort();
            }
        }
    });
}

#[test]
fn qoder_late_error_after_terminal_never_exposes_success_before_failure() {
    run_qoder_async_test(async {
        for rail in [
            QoderFixtureRail::GlobalOauth,
            QoderFixtureRail::CnOauth,
            QoderFixtureRail::Pat,
        ] {
            for app in [AppKind::Claude, AppKind::Codex] {
                let name = format!("qoder-late-error-{}-{}", rail.label(), app.as_str());
                let (address, fixture, server) = spawn_qoder_upstream(
                    rail,
                    vec![QoderGenerationReply::TerminalThenUnauthorized],
                )
                .await;
                let state = qoder_test_state(&name);
                let (provider_id, _) =
                    install_qoder_provider(&state, &name, app, rail, &format!("http://{address}"))
                        .await;
                let share_id = format!("{name}-share");
                install_qoder_share(&state, &share_id, app, &provider_id, "owner@example.com")
                    .await;
                let response = forward_qoder_surface(
                    state.clone(),
                    app,
                    provider_id,
                    &share_id,
                    "owner@example.com",
                    "late-error-session",
                    true,
                )
                .await
                .unwrap();
                let body = String::from_utf8(collect(response).await).unwrap();
                assert!(body.contains("ready"), "{name}: {body}");
                assert!(!body.contains("response.completed"), "{name}: {body}");
                if app == AppKind::Claude {
                    let error_at = body.find("event: error").expect("Claude error event");
                    assert!(
                        !body[..error_at].contains("event: message_delta"),
                        "{name}: {body}"
                    );
                    assert!(
                        body.find("event: message_stop")
                            .is_some_and(|stop_at| stop_at > error_at),
                        "{name}: {body}"
                    );
                }
                let usage = state.usage_snapshot().await;
                let log = usage.logs.last().expect("Qoder usage log");
                assert_eq!(
                    log.stream_status.as_deref(),
                    Some("upstream_error"),
                    "{name}"
                );
                assert_eq!(log.input_tokens, Some(2), "{name}");
                assert_eq!(log.output_tokens, Some(1), "{name}");
                assert_eq!(log.total_tokens, Some(3), "{name}");
                assert_ne!(log.outcome, UsageOutcome::Success, "{name}");
                assert_eq!(fixture.count(&fixture.generation_requests), 1, "{name}");
                assert_eq!(fixture.count(&fixture.refresh_requests), 0, "{name}");
                server.abort();
            }
        }
    });
}

#[test]
fn qoder_stream_read_failure_preserves_observed_usage_without_success() {
    run_qoder_async_test(async {
        for rail in [
            QoderFixtureRail::GlobalOauth,
            QoderFixtureRail::CnOauth,
            QoderFixtureRail::Pat,
        ] {
            for app in [AppKind::Claude, AppKind::Codex, AppKind::Gemini] {
                let name = format!("qoder-read-failure-{}-{}", rail.label(), app.as_str());
                let (address, fixture, server) =
                    spawn_qoder_upstream(rail, vec![QoderGenerationReply::UsageThenReadFailure])
                        .await;
                let state = qoder_test_state(&name);
                let (provider_id, _) =
                    install_qoder_provider(&state, &name, app, rail, &format!("http://{address}"))
                        .await;
                let share_id = format!("{name}-share");
                install_qoder_share(&state, &share_id, app, &provider_id, "owner@example.com")
                    .await;
                let response = forward_qoder_surface(
                    state.clone(),
                    app,
                    provider_id.clone(),
                    &share_id,
                    "owner@example.com",
                    "read-failure-session",
                    true,
                )
                .await
                .unwrap();
                let body = String::from_utf8(collect(response).await).unwrap();
                assert!(body.contains("ready"), "{name}: {body}");
                assert!(!body.contains("response.completed"), "{name}: {body}");
                let usage = state.usage_snapshot().await;
                let log = usage.logs.last().expect("Qoder usage log");
                assert_eq!(
                    log.stream_status.as_deref(),
                    Some("upstream_error"),
                    "{name}"
                );
                assert_eq!(log.input_tokens, Some(2), "{name}");
                assert_eq!(log.output_tokens, Some(1), "{name}");
                assert_eq!(log.total_tokens, Some(3), "{name}");
                assert_ne!(log.outcome, UsageOutcome::Success, "{name}");
                if app == AppKind::Codex {
                    let response_id = body
                        .lines()
                        .filter_map(|line| line.strip_prefix("data: "))
                        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
                        .find(|event| event["type"] == "response.created")
                        .and_then(|event| event["response"]["id"].as_str().map(str::to_string))
                        .expect("Qoder response.created ID");
                    let followup = super::forward_for_test_surface(
                        state.clone(),
                        ProxyRoute::CodexResponses,
                        provider_id,
                        None,
                        qoder_headers(&share_id, "owner@example.com", "read-failure-session"),
                        Bytes::from(
                            json!({
                                "model": MODEL_KEY,
                                "input": "follow up",
                                "previous_response_id": response_id,
                                "stream": false
                            })
                            .to_string(),
                        ),
                    )
                    .await
                    .unwrap_err();
                    assert_eq!(followup.status, StatusCode::CONFLICT, "{name}");
                }
                assert_eq!(fixture.count(&fixture.generation_requests), 1, "{name}");
                assert_eq!(fixture.count(&fixture.refresh_requests), 0, "{name}");
                server.abort();
            }
        }
    });
}

#[test]
fn qoder_responses_alias_rejects_share_user_model_and_account_drift_before_network() {
    run_qoder_async_test(async {
        let (address, fixture, server) = spawn_qoder_upstream(
            QoderFixtureRail::GlobalOauth,
            vec![QoderGenerationReply::Success],
        )
        .await;
        let state = qoder_test_state("qoder-response-scope-drift");
        let (provider_id, account_id) = install_qoder_provider(
            &state,
            "qoder-response-scope-drift",
            AppKind::Codex,
            QoderFixtureRail::GlobalOauth,
            &format!("http://{address}"),
        )
        .await;
        let share_id = "qoder-response-scope-drift-share";
        let other_share = "qoder-response-scope-drift-other";
        for share in [share_id, other_share] {
            install_qoder_share(
                &state,
                share,
                AppKind::Codex,
                &provider_id,
                "owner@example.com",
            )
            .await;
        }
        let first = forward_qoder_surface(
            state.clone(),
            AppKind::Codex,
            provider_id.clone(),
            share_id,
            "owner@example.com",
            "scope-session",
            false,
        )
        .await
        .unwrap();
        let first_body = String::from_utf8(collect(first).await).unwrap();
        let response_id = qoder_responses_id(&first_body, false);
        let first_share_id = share_id.to_string();
        state
            .mutate_shares_immediate(move |shares| {
                let share = shares
                    .shares
                    .iter_mut()
                    .find(|share| share.id == first_share_id)
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
        for (share, user, model) in [
            (other_share, "owner@example.com", MODEL_KEY),
            (share_id, "other@example.com", MODEL_KEY),
            (share_id, "owner@example.com", "different-model"),
        ] {
            let result = super::forward_for_test_surface(
                state.clone(),
                ProxyRoute::CodexResponses,
                provider_id.clone(),
                None,
                qoder_headers(share, user, "scope-session"),
                Bytes::from(
                    json!({
                        "model": model,
                        "input": "follow up",
                        "previous_response_id": response_id,
                        "stream": false
                    })
                    .to_string(),
                ),
            )
            .await;
            let error = result.err().unwrap_or_else(|| {
                panic!("{share}/{user}/{model} unexpectedly accepted a foreign response ID")
            });
            assert_eq!(error.status, StatusCode::CONFLICT, "{share}/{user}/{model}");
            assert_eq!(fixture.count(&fixture.generation_requests), 1);
        }
        let mut accounts = state.accounts_snapshot().await;
        accounts
            .accounts
            .iter_mut()
            .find(|account| account.id == account_id)
            .unwrap()
            .token_refresh_generation += 1;
        state.replace_account_store_for_test(accounts).await;
        let error = super::forward_for_test_surface(
            state,
            ProxyRoute::CodexResponses,
            provider_id,
            None,
            qoder_headers(share_id, "owner@example.com", "scope-session"),
            Bytes::from(
                json!({
                    "model": MODEL_KEY,
                    "input": "follow up",
                    "previous_response_id": response_id,
                    "stream": false
                })
                .to_string(),
            ),
        )
        .await
        .unwrap_err();
        assert_eq!(error.status, StatusCode::CONFLICT);
        assert_eq!(fixture.count(&fixture.generation_requests), 1);
        assert_eq!(fixture.count(&fixture.refresh_requests), 0);
        server.abort();
    });
}

#[test]
fn qoder_r4_review_discovery_capacity_is_local_and_zero_outbound_on_every_rail_surface() {
    run_qoder_async_test(async {
        for rail in [
            QoderFixtureRail::GlobalOauth,
            QoderFixtureRail::CnOauth,
            QoderFixtureRail::Pat,
        ] {
            for app in [AppKind::Claude, AppKind::Codex, AppKind::Gemini] {
                let name = format!("qoder-r4-flight-{}-{}", rail.label(), app.as_str());
                let (address, fixture, server) =
                    spawn_qoder_upstream(rail, vec![QoderGenerationReply::Success]).await;
                let parent = crate::infra::resident_memory::ResidentMemoryBudget::new(Some(0));
                let mut state = qoder_test_state(&name);
                Arc::get_mut(&mut state)
                    .expect("unique fixture state")
                    .qoder_runtime =
                    crate::proxy::qoder_runtime::QoderRuntimeCache::with_parent(parent.clone());
                let (provider_id, account_id) =
                    install_qoder_provider(&state, &name, app, rail, &format!("http://{address}"))
                        .await;
                let share_id = format!("{name}-share");
                install_qoder_share(&state, &share_id, app, &provider_id, "owner@example.com")
                    .await;
                let error = forward_qoder_surface(
                    state.clone(),
                    app,
                    provider_id,
                    &share_id,
                    "owner@example.com",
                    "capacity",
                    false,
                )
                .await
                .unwrap_err();
                assert!(
                    error.is_request_memory_exhausted(),
                    "{rail:?}/{app:?}: {error:?}"
                );
                assert_eq!(fixture.count(&fixture.model_requests), 0);
                assert_eq!(fixture.count(&fixture.generation_requests), 0);
                assert_eq!(fixture.count(&fixture.pat_exchanges), 0);
                assert_eq!(fixture.count(&fixture.refresh_requests), 0);
                assert_eq!(parent.snapshot().used_bytes, 0);
                assert!(!state.share_in_flight.has_in_flight(&share_id));
                assert_eq!(
                    state
                        .account_in_flight
                        .snapshot()
                        .current(ProviderType::QoderCosy, &account_id),
                    0
                );
                let account = state
                    .find_account_for_provider(ProviderType::QoderCosy, &account_id)
                    .await
                    .unwrap();
                assert!(!account.needs_relogin);
                let usage = state.usage_snapshot().await;
                assert_eq!(
                    usage.logs.last().unwrap().failure_kind.as_deref(),
                    Some("memory_capacity")
                );
                assert!(!QoderRuntimeError::MemoryCapacity.is_authentication_failure());
                server.abort();
            }
        }
    });
}

#[test]
fn qoder_r4_review_optional_alias_pressure_keeps_completed_response_successful() {
    run_qoder_async_test(async {
        for streaming in [false, true] {
            let name = format!("qoder-r4-alias-pressure-{streaming}");
            let (address, fixture, server) = spawn_qoder_upstream(
                QoderFixtureRail::GlobalOauth,
                vec![QoderGenerationReply::Success],
            )
            .await;
            let parent = crate::infra::resident_memory::ResidentMemoryBudget::new(None);
            let mut state = qoder_test_state(&name);
            Arc::get_mut(&mut state).unwrap().qoder_runtime =
                crate::proxy::qoder_runtime::QoderRuntimeCache::with_parent(parent.clone());
            let (provider_id, _) = install_qoder_provider(
                &state,
                &name,
                AppKind::Codex,
                QoderFixtureRail::GlobalOauth,
                &format!("http://{address}"),
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
            let first = forward_qoder_surface(
                state.clone(),
                AppKind::Codex,
                provider_id.clone(),
                &share_id,
                "owner@example.com",
                "first",
                false,
            )
            .await
            .unwrap();
            assert_eq!(first.status(), StatusCode::OK);
            collect(first).await;
            parent.configure_limit(Some(0));
            let next = forward_qoder_surface(
                state.clone(),
                AppKind::Codex,
                provider_id,
                &share_id,
                "owner@example.com",
                "next",
                streaming,
            )
            .await
            .unwrap();
            assert_eq!(next.status(), StatusCode::OK);
            let body = String::from_utf8(collect(next).await).unwrap();
            assert!(!body.contains("cc_switch_request_memory_exhausted"));
            let usage = state.usage_snapshot().await;
            assert_eq!(usage.logs.last().unwrap().outcome, UsageOutcome::Success);
            assert_eq!(fixture.count(&fixture.model_requests), 1);
            assert_eq!(fixture.count(&fixture.generation_requests), 2);
            assert_eq!(fixture.count(&fixture.refresh_requests), 0);
            assert!(parent.snapshot().rejections > 0);
            server.abort();
        }
    });
}

#[test]
fn qoder_request_memory_rejects_all_surfaces_before_any_upstream_request() {
    run_qoder_async_test(async {
        for app in [AppKind::Claude, AppKind::Codex, AppKind::Gemini] {
            let name = format!("qoder-memory-pre-network-{}", app.as_str());
            let (address, fixture, server) = spawn_qoder_upstream(
                QoderFixtureRail::GlobalOauth,
                vec![QoderGenerationReply::Success],
            )
            .await;
            let (_, _, body) = surface_request(app, false);
            let mut state = qoder_test_state(&name);
            Arc::get_mut(&mut state)
                .expect("Qoder test state must be uniquely owned")
                .request_body_limits
                .memory_budget_bytes = body.len().saturating_add(1);
            let origin = format!("http://{address}");
            let (provider_id, _) =
                install_qoder_provider(&state, &name, app, QoderFixtureRail::GlobalOauth, &origin)
                    .await;
            let share_id = format!("{name}-share");
            install_qoder_share(&state, &share_id, app, &provider_id, "owner@example.com").await;

            let error = forward_qoder_surface(
                state,
                app,
                provider_id,
                &share_id,
                "owner@example.com",
                "memory-pre-network",
                false,
            )
            .await
            .unwrap_err();
            assert!(error.is_request_memory_exhausted(), "{app:?}: {error:?}");
            assert_eq!(error.error_code(), "cc_switch_request_memory_exhausted");
            assert_eq!(fixture.count(&fixture.model_requests), 0, "{app:?}");
            assert_eq!(fixture.count(&fixture.generation_requests), 0, "{app:?}");
            server.abort();
        }
    });
}

#[test]
fn qoder_nonstream_aggregate_is_bounded_by_request_memory() {
    run_qoder_async_test(async {
        let name = "qoder-memory-nonstream-aggregate";
        let (address, fixture, server) = spawn_qoder_upstream(
            QoderFixtureRail::GlobalOauth,
            vec![QoderGenerationReply::LargeAggregate],
        )
        .await;
        let mut state = qoder_test_state(name);
        Arc::get_mut(&mut state)
            .expect("Qoder test state must be uniquely owned")
            .request_body_limits
            .memory_budget_bytes = 192 * 1024;
        let origin = format!("http://{address}");
        let (provider_id, _) = install_qoder_provider(
            &state,
            name,
            AppKind::Claude,
            QoderFixtureRail::GlobalOauth,
            &origin,
        )
        .await;
        let share_id = format!("{name}-share");
        install_qoder_share(
            &state,
            &share_id,
            AppKind::Claude,
            &provider_id,
            "owner@example.com",
        )
        .await;

        let error = forward_qoder_surface(
            state.clone(),
            AppKind::Claude,
            provider_id,
            &share_id,
            "owner@example.com",
            "memory-aggregate",
            false,
        )
        .await
        .unwrap_err();
        assert!(error.is_request_memory_exhausted(), "{error:?}");
        assert_eq!(fixture.count(&fixture.model_requests), 1);
        assert_eq!(fixture.count(&fixture.generation_requests), 1);
        let usage = state.usage.read().await.logs.last().cloned().unwrap();
        assert_eq!(usage.stream_status, None);
        assert_eq!(usage.failure_kind.as_deref(), Some("memory_capacity"));
        assert_eq!(usage.outcome, UsageOutcome::InternalError);
        server.abort();
    });
}

#[test]
fn qoder_stream_tool_state_exhaustion_emits_one_redacted_terminal() {
    run_qoder_async_test(async {
        let name = "qoder-memory-stream-tools";
        let (address, fixture, server) = spawn_qoder_upstream(
            QoderFixtureRail::GlobalOauth,
            vec![QoderGenerationReply::LargeToolStream],
        )
        .await;
        let mut state = qoder_test_state(name);
        Arc::get_mut(&mut state)
            .expect("Qoder test state must be uniquely owned")
            .request_body_limits
            .memory_budget_bytes = 192 * 1024;
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
            state.clone(),
            AppKind::Codex,
            provider_id,
            &share_id,
            "owner@example.com",
            "memory-stream-tools",
            true,
        )
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = String::from_utf8(collect(response).await).unwrap();
        assert_eq!(
            body.matches("cc_switch_request_memory_exhausted").count(),
            1,
            "{body}"
        );
        assert_eq!(body.matches("event: response.failed").count(), 1, "{body}");
        assert!(
            !body.contains("resident-memory budget exhausted for"),
            "{body}"
        );
        assert_eq!(fixture.count(&fixture.model_requests), 1);
        assert_eq!(fixture.count(&fixture.generation_requests), 1);
        let usage = state.usage.read().await.logs.last().cloned().unwrap();
        assert_eq!(usage.stream_status.as_deref(), Some("memory_capacity"));
        assert_eq!(usage.failure_kind.as_deref(), Some("memory_capacity"));
        assert_eq!(usage.outcome, UsageOutcome::InternalError);
        server.abort();
    });
}

#[test]
fn qoder_memory_exhaustion_never_replays_unauthorized_on_any_rail() {
    run_qoder_async_test(async {
        for rail in [
            QoderFixtureRail::GlobalOauth,
            QoderFixtureRail::CnOauth,
            QoderFixtureRail::Pat,
        ] {
            let name = format!("qoder-memory-auth-no-replay-{}", rail.label());
            let (address, fixture, server) =
                spawn_qoder_upstream(rail, vec![QoderGenerationReply::OversizedHttpUnauthorized])
                    .await;
            let mut state = qoder_test_state(&name);
            Arc::get_mut(&mut state)
                .expect("Qoder test state must be uniquely owned")
                .request_body_limits
                .memory_budget_bytes = 128 * 1024;
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

            let error = forward_qoder_surface(
                state,
                AppKind::Codex,
                provider_id,
                &share_id,
                "owner@example.com",
                "memory-auth-no-replay",
                false,
            )
            .await
            .unwrap_err();
            assert!(error.is_request_memory_exhausted(), "{rail:?}: {error:?}");
            assert_eq!(fixture.count(&fixture.model_requests), 1, "{rail:?}");
            assert_eq!(fixture.count(&fixture.generation_requests), 1, "{rail:?}");
            assert_eq!(fixture.count(&fixture.refresh_requests), 0, "{rail:?}");
            assert_eq!(
                fixture.count(&fixture.pat_exchanges),
                usize::from(rail == QoderFixtureRail::Pat),
                "{rail:?}"
            );
            server.abort();
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
fn qoder_malformed_sse_wrapper_fails_all_bound_rails_and_surfaces() {
    run_qoder_async_test(async {
        for rail in [
            QoderFixtureRail::GlobalOauth,
            QoderFixtureRail::CnOauth,
            QoderFixtureRail::Pat,
        ] {
            for app in [AppKind::Claude, AppKind::Codex, AppKind::Gemini] {
                let name = format!("qoder-malformed-wrapper-{}-{app:?}", rail.label());
                let (address, fixture, server) =
                    spawn_qoder_upstream(rail, vec![QoderGenerationReply::MalformedWrapper]).await;
                let state = qoder_test_state(&name);
                let origin = format!("http://{address}");
                let (provider_id, _) =
                    install_qoder_provider(&state, &name, app, rail, &origin).await;
                let share_id = format!("{name}-share");
                install_qoder_share(&state, &share_id, app, &provider_id, "owner@example.com")
                    .await;

                let error = forward_qoder_surface(
                    state,
                    app,
                    provider_id,
                    &share_id,
                    "owner@example.com",
                    "malformed-wrapper-session",
                    false,
                )
                .await
                .unwrap_err();
                assert_eq!(error.status, StatusCode::BAD_GATEWAY, "{name}: {error:?}");
                assert_eq!(fixture.count(&fixture.generation_requests), 1, "{name}");
                assert_eq!(fixture.count(&fixture.refresh_requests), 0, "{name}");
                server.abort();
            }
        }
    });
}

#[test]
fn qoder_r4_ambiguous_or_history_only_selectors_fail_before_any_upstream_request() {
    run_qoder_async_test(async {
        for rail in [
            QoderFixtureRail::GlobalOauth,
            QoderFixtureRail::CnOauth,
            QoderFixtureRail::Pat,
        ] {
            let name = format!("qoder-r4-identities-{}", rail.label());
            let (address, fixture, server) =
                spawn_qoder_upstream(rail, vec![QoderGenerationReply::Success]).await;
            let state = qoder_test_state(&name);
            let (provider_id, _) = install_qoder_provider(
                &state,
                &name,
                AppKind::Codex,
                rail,
                &format!("http://{address}"),
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
            for stream in [false, true] {
                for mut request in [
                    json!({"tools":[
                        {"type":"namespace","name":"a","tools":[{"type":"function","name":"b__c"}]},
                        {"type":"namespace","name":"a__b","tools":[{"type":"function","name":"c"}]}
                    ],"input":"read"}),
                    json!({"tools":[{"type":"custom","name":"read"}],"tool_choice":{"type":"function","name":"read"},"input":"read"}),
                    json!({"tools":[],"tool_choice":{"type":"function","namespace":"retired","name":"read"},"input":[
                        {"type":"function_call","call_id":"old","namespace":"retired","name":"read","arguments":"{}"},
                        {"type":"function_call_output","call_id":"old","output":"old result"}
                    ]}),
                ] {
                    request["model"] = json!(MODEL_KEY);
                    request["stream"] = json!(stream);
                    for _ in 0..2 {
                        let error = super::forward_for_test_surface(
                            state.clone(),
                            ProxyRoute::CodexResponses,
                            provider_id.clone(),
                            None,
                            qoder_headers(&share_id, "owner@example.com", "identity-session"),
                            Bytes::from(request.to_string()),
                        )
                        .await
                        .expect_err("ambiguous or undeclared identity must fail locally");
                        assert_eq!(error.status, StatusCode::BAD_REQUEST, "{name}: {error:?}");
                        request["tools"].as_array_mut().unwrap().reverse();
                    }
                }
            }
            assert_eq!(fixture.count(&fixture.generation_requests), 0, "{name}");
            assert_eq!(fixture.count(&fixture.model_requests), 0, "{name}");
            assert_eq!(fixture.count(&fixture.pat_exchanges), 0, "{name}");
            assert_eq!(fixture.count(&fixture.refresh_requests), 0, "{name}");
            server.abort();
        }
    });
}

#[test]
fn qoder_r4_declared_history_survives_choice_on_every_rail_and_surface() {
    run_qoder_async_test(async {
        for rail in [
            QoderFixtureRail::GlobalOauth,
            QoderFixtureRail::CnOauth,
            QoderFixtureRail::Pat,
        ] {
            for app in [AppKind::Claude, AppKind::Codex, AppKind::Gemini] {
                let name = format!("qoder-r4-history-{}-{app:?}", rail.label());
                let (address, fixture, server) =
                    spawn_qoder_upstream(rail, vec![QoderGenerationReply::Success]).await;
                let state = qoder_test_state(&name);
                let (provider_id, _) =
                    install_qoder_provider(&state, &name, app, rail, &format!("http://{address}"))
                        .await;
                let share_id = format!("{name}-share");
                install_qoder_share(&state, &share_id, app, &provider_id, "owner@example.com")
                    .await;
                for stream in [false, true] {
                    for named in [false, true] {
                        let (route, gemini_path, body) = surface_request(app, stream);
                        let mut request: Value = serde_json::from_slice(&body).unwrap();
                        match app {
                            AppKind::Claude => {
                                request["tools"] = json!([{"name":"read","input_schema":{"type":"object"}},{"name":"write","input_schema":{"type":"object"}}]);
                                request["tool_choice"] = if named {
                                    json!({"type":"tool","name":"write"})
                                } else {
                                    json!({"type":"none"})
                                };
                                request["messages"] = json!([
                                    {"role":"assistant","content":[{"type":"tool_use","id":"old-read","name":"read","input":{"path":"original"}}]},
                                    {"role":"user","content":[{"type":"tool_result","tool_use_id":"old-read","content":"original result"}]},
                                    {"role":"user","content":"continue"}
                                ]);
                            }
                            AppKind::Codex => {
                                request["tools"] = json!([{"type":"function","name":"read","parameters":{"type":"object"}},{"type":"function","name":"write","parameters":{"type":"object"}}]);
                                request["tool_choice"] = if named {
                                    json!({"type":"function","name":"write"})
                                } else {
                                    json!("none")
                                };
                                request["input"] = json!([
                                    {"type":"function_call","call_id":"old-read","name":"read","arguments":"{\"path\":\"original\"}"},
                                    {"type":"function_call_output","call_id":"old-read","output":"original result"},
                                    {"role":"user","content":"continue"}
                                ]);
                            }
                            AppKind::Gemini => {
                                request["tools"] = json!([{"functionDeclarations":[{"name":"read","parameters":{"type":"object"}},{"name":"write","parameters":{"type":"object"}}]}]);
                                request["toolConfig"] = if named {
                                    json!({"functionCallingConfig":{"mode":"ANY","allowedFunctionNames":["write"]}})
                                } else {
                                    json!({"functionCallingConfig":{"mode":"NONE"}})
                                };
                                request["contents"] = json!([
                                    {"role":"model","parts":[{"functionCall":{"name":"read","args":{"path":"original"}}}]},
                                    {"role":"user","parts":[{"functionResponse":{"name":"read","response":{"result":"original result"}}}]},
                                    {"role":"user","parts":[{"text":"continue"}]}
                                ]);
                            }
                        }
                        let response = super::forward_for_test_surface(
                            state.clone(),
                            route,
                            provider_id.clone(),
                            gemini_path,
                            qoder_headers(
                                &share_id,
                                "owner@example.com",
                                &format!("history-{stream}-{named}"),
                            ),
                            Bytes::from(request.to_string()),
                        )
                        .await
                        .unwrap();
                        assert_eq!(response.status(), StatusCode::OK, "{name}");
                        collect(response).await;
                        let observations = fixture.generation_observations();
                        let payload = observations.last().unwrap().decoded_body.as_ref().unwrap();
                        let messages = payload["messages"].as_array().unwrap();
                        let call = messages
                            .iter()
                            .find_map(|message| {
                                message
                                    .get("tool_calls")
                                    .and_then(Value::as_array)
                                    .and_then(|calls| calls.first())
                            })
                            .expect("declared history was removed from final COSY payload");
                        assert_eq!(call["function"]["name"], "read", "{name}");
                        let arguments: Value =
                            serde_json::from_str(call["function"]["arguments"].as_str().unwrap())
                                .unwrap();
                        assert_eq!(arguments, json!({"path":"original"}));
                        let result = messages
                            .iter()
                            .find(|message| message["role"] == "tool")
                            .unwrap();
                        assert_eq!(result["tool_call_call_id"], call["id"]);
                        assert!(result["contents"][0]["text"]
                            .as_str()
                            .unwrap()
                            .contains("original result"));
                        let tools = payload["tools"].as_array().unwrap();
                        assert_eq!(tools.len(), usize::from(named), "{name}");
                        if named {
                            assert_eq!(tools[0]["function"]["name"], "write");
                        }
                    }
                }
                assert_eq!(fixture.count(&fixture.generation_requests), 4, "{name}");
                server.abort();
            }
        }
    });
}

#[test]
fn qoder_undeclared_forced_tool_fails_before_generation_on_all_rails_and_surfaces() {
    run_qoder_async_test(async {
        for rail in [
            QoderFixtureRail::GlobalOauth,
            QoderFixtureRail::CnOauth,
            QoderFixtureRail::Pat,
        ] {
            for app in [AppKind::Claude, AppKind::Codex, AppKind::Gemini] {
                let name = format!("qoder-undeclared-choice-{}-{app:?}", rail.label());
                let (address, fixture, server) =
                    spawn_qoder_upstream(rail, vec![QoderGenerationReply::Success]).await;
                let state = qoder_test_state(&name);
                let origin = format!("http://{address}");
                let (provider_id, _) =
                    install_qoder_provider(&state, &name, app, rail, &origin).await;
                let share_id = format!("{name}-share");
                install_qoder_share(&state, &share_id, app, &provider_id, "owner@example.com")
                    .await;

                let (route, gemini_path, body) = surface_request(app, false);
                let mut request: Value = serde_json::from_slice(&body).unwrap();
                match app {
                    AppKind::Claude => {
                        request["tools"] =
                            json!([{"name":"read","input_schema":{"type":"object"}}]);
                        request["tool_choice"] = json!({"type":"tool","name":"missing"});
                    }
                    AppKind::Codex => {
                        request["tools"] = json!([{"type":"function","name":"read","parameters":{"type":"object"}}]);
                        request["tool_choice"] = json!({"type":"function","name":"missing"});
                    }
                    AppKind::Gemini => {
                        request["tools"] = json!([{"functionDeclarations":[{"name":"read","parameters":{"type":"object"}}]}]);
                        request["toolConfig"] = json!({"functionCallingConfig":{"mode":"ANY","allowedFunctionNames":["missing"]}});
                    }
                }
                let error = super::forward_for_test_surface(
                    state,
                    route,
                    provider_id,
                    gemini_path,
                    qoder_headers(&share_id, "owner@example.com", "forced-choice-session"),
                    Bytes::from(request.to_string()),
                )
                .await
                .unwrap_err();
                assert_eq!(error.status, StatusCode::BAD_REQUEST, "{name}: {error:?}");
                assert_eq!(fixture.count(&fixture.generation_requests), 0, "{name}");
                assert_eq!(fixture.count(&fixture.refresh_requests), 0, "{name}");
                server.abort();
            }
        }
    });
}

#[test]
fn qoder_invalid_tool_index_fails_all_bound_rails_and_surfaces() {
    run_qoder_async_test(async {
        for rail in [
            QoderFixtureRail::GlobalOauth,
            QoderFixtureRail::CnOauth,
            QoderFixtureRail::Pat,
        ] {
            for app in [AppKind::Claude, AppKind::Codex, AppKind::Gemini] {
                let name = format!("qoder-invalid-tool-index-{}-{app:?}", rail.label());
                let (address, fixture, server) =
                    spawn_qoder_upstream(rail, vec![QoderGenerationReply::InvalidToolIndex]).await;
                let state = qoder_test_state(&name);
                let origin = format!("http://{address}");
                let (provider_id, _) =
                    install_qoder_provider(&state, &name, app, rail, &origin).await;
                let share_id = format!("{name}-share");
                install_qoder_share(&state, &share_id, app, &provider_id, "owner@example.com")
                    .await;

                let error = forward_qoder_surface(
                    state,
                    app,
                    provider_id,
                    &share_id,
                    "owner@example.com",
                    "invalid-tool-index-session",
                    false,
                )
                .await
                .unwrap_err();
                assert_eq!(error.status, StatusCode::BAD_GATEWAY, "{name}: {error:?}");
                assert_eq!(fixture.count(&fixture.generation_requests), 1, "{name}");
                assert_eq!(fixture.count(&fixture.refresh_requests), 0, "{name}");
                server.abort();
            }
        }
    });
}

#[test]
fn qoder_conflicting_tool_alias_fails_all_bound_rails_and_surfaces() {
    run_qoder_async_test(async {
        for rail in [
            QoderFixtureRail::GlobalOauth,
            QoderFixtureRail::CnOauth,
            QoderFixtureRail::Pat,
        ] {
            for app in [AppKind::Claude, AppKind::Codex, AppKind::Gemini] {
                let name = format!("qoder-conflicting-tool-alias-{}-{app:?}", rail.label());
                let (address, fixture, server) =
                    spawn_qoder_upstream(rail, vec![QoderGenerationReply::ConflictingToolAlias])
                        .await;
                let state = qoder_test_state(&name);
                let origin = format!("http://{address}");
                let (provider_id, _) =
                    install_qoder_provider(&state, &name, app, rail, &origin).await;
                let share_id = format!("{name}-share");
                install_qoder_share(&state, &share_id, app, &provider_id, "owner@example.com")
                    .await;

                let error = forward_qoder_surface(
                    state,
                    app,
                    provider_id,
                    &share_id,
                    "owner@example.com",
                    "conflicting-tool-alias-session",
                    false,
                )
                .await
                .unwrap_err();
                assert_eq!(error.status, StatusCode::BAD_GATEWAY, "{name}: {error:?}");
                assert_eq!(fixture.count(&fixture.generation_requests), 1, "{name}");
                assert_eq!(fixture.count(&fixture.refresh_requests), 0, "{name}");
                server.abort();
            }
        }
    });
}

#[test]
fn qoder_duplicate_choice_fails_all_bound_rails_and_surfaces() {
    run_qoder_async_test(async {
        for rail in [
            QoderFixtureRail::GlobalOauth,
            QoderFixtureRail::CnOauth,
            QoderFixtureRail::Pat,
        ] {
            for app in [AppKind::Claude, AppKind::Codex, AppKind::Gemini] {
                let name = format!("qoder-duplicate-choice-{}-{app:?}", rail.label());
                let (address, fixture, server) =
                    spawn_qoder_upstream(rail, vec![QoderGenerationReply::DuplicateChoice]).await;
                let state = qoder_test_state(&name);
                let origin = format!("http://{address}");
                let (provider_id, _) =
                    install_qoder_provider(&state, &name, app, rail, &origin).await;
                let share_id = format!("{name}-share");
                install_qoder_share(&state, &share_id, app, &provider_id, "owner@example.com")
                    .await;

                let error = forward_qoder_surface(
                    state,
                    app,
                    provider_id,
                    &share_id,
                    "owner@example.com",
                    "duplicate-choice-session",
                    false,
                )
                .await
                .unwrap_err();
                assert_eq!(error.status, StatusCode::BAD_GATEWAY, "{name}: {error:?}");
                assert_eq!(fixture.count(&fixture.generation_requests), 1, "{name}");
                assert_eq!(fixture.count(&fixture.refresh_requests), 0, "{name}");
                server.abort();
            }
        }
    });
}

#[test]
fn qoder_invalid_semantic_text_fails_all_bound_rails_and_surfaces() {
    run_qoder_async_test(async {
        for rail in [
            QoderFixtureRail::GlobalOauth,
            QoderFixtureRail::CnOauth,
            QoderFixtureRail::Pat,
        ] {
            for app in [AppKind::Claude, AppKind::Codex, AppKind::Gemini] {
                for stream in [false, true] {
                    let name = format!(
                        "qoder-invalid-semantic-text-{}-{app:?}-{stream}",
                        rail.label()
                    );
                    let (address, fixture, server) =
                        spawn_qoder_upstream(rail, vec![QoderGenerationReply::InvalidSemanticText])
                            .await;
                    let state = qoder_test_state(&name);
                    let origin = format!("http://{address}");
                    let (provider_id, _) =
                        install_qoder_provider(&state, &name, app, rail, &origin).await;
                    let share_id = format!("{name}-share");
                    install_qoder_share(&state, &share_id, app, &provider_id, "owner@example.com")
                        .await;

                    let error = forward_qoder_surface(
                        state,
                        app,
                        provider_id,
                        &share_id,
                        "owner@example.com",
                        "invalid-semantic-text-session",
                        stream,
                    )
                    .await
                    .unwrap_err();
                    assert_eq!(error.status, StatusCode::BAD_GATEWAY, "{name}: {error:?}");
                    assert_eq!(fixture.count(&fixture.generation_requests), 1, "{name}");
                    assert_eq!(fixture.count(&fixture.refresh_requests), 0, "{name}");
                    server.abort();
                }
            }
        }
    });
}

#[test]
fn qoder_heartbeat_only_success_status_fails_before_commit_on_every_bound_surface() {
    run_qoder_async_test(async {
        for rail in [
            QoderFixtureRail::GlobalOauth,
            QoderFixtureRail::CnOauth,
            QoderFixtureRail::Pat,
        ] {
            for app in [AppKind::Claude, AppKind::Codex, AppKind::Gemini] {
                for stream in [false, true] {
                    let name = format!("qoder-empty-stream-{}-{app:?}-{stream}", rail.label());
                    let (address, fixture, server) =
                        spawn_qoder_upstream(rail, vec![QoderGenerationReply::HeartbeatOnly]).await;
                    let state = qoder_test_state(&name);
                    let origin = format!("http://{address}");
                    let (provider_id, _) =
                        install_qoder_provider(&state, &name, app, rail, &origin).await;
                    let share_id = format!("{name}-share");
                    install_qoder_share(&state, &share_id, app, &provider_id, "owner@example.com")
                        .await;

                    let error = forward_qoder_surface(
                        state,
                        app,
                        provider_id,
                        &share_id,
                        "owner@example.com",
                        "heartbeat-only-session",
                        stream,
                    )
                    .await
                    .unwrap_err();
                    assert_eq!(error.status, StatusCode::BAD_GATEWAY, "{name}: {error:?}");
                    assert_eq!(fixture.count(&fixture.generation_requests), 1, "{name}");
                    assert_eq!(fixture.count(&fixture.refresh_requests), 0, "{name}");
                    server.abort();
                }
            }
        }
    });
}

#[test]
fn qoder_invalid_usage_total_fails_all_bound_rails_and_surfaces() {
    run_qoder_async_test(async {
        for rail in [
            QoderFixtureRail::GlobalOauth,
            QoderFixtureRail::CnOauth,
            QoderFixtureRail::Pat,
        ] {
            for app in [AppKind::Claude, AppKind::Codex, AppKind::Gemini] {
                let name = format!("qoder-invalid-usage-total-{}-{app:?}", rail.label());
                let (address, fixture, server) =
                    spawn_qoder_upstream(rail, vec![QoderGenerationReply::InvalidUsageTotal]).await;
                let state = qoder_test_state(&name);
                let origin = format!("http://{address}");
                let (provider_id, _) =
                    install_qoder_provider(&state, &name, app, rail, &origin).await;
                let share_id = format!("{name}-share");
                install_qoder_share(&state, &share_id, app, &provider_id, "owner@example.com")
                    .await;

                let error = forward_qoder_surface(
                    state,
                    app,
                    provider_id,
                    &share_id,
                    "owner@example.com",
                    "invalid-usage-total-session",
                    false,
                )
                .await
                .unwrap_err();
                assert_eq!(error.status, StatusCode::BAD_GATEWAY, "{name}: {error:?}");
                assert_eq!(fixture.count(&fixture.generation_requests), 1, "{name}");
                assert_eq!(fixture.count(&fixture.refresh_requests), 0, "{name}");
                server.abort();
            }
        }
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
fn qoder_responses_root_conversations_are_independent_and_generation_fenced() {
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
        assert_ne!(before[0], before[1]);
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
fn qoder_catalog_cache_refetches_after_same_generation_request_identity_change() {
    run_qoder_async_test(async {
        for changed in ["credential", "machine"] {
            let (address, fixture, server) = spawn_qoder_upstream(
                QoderFixtureRail::GlobalOauth,
                vec![QoderGenerationReply::Success],
            )
            .await;
            let state = qoder_test_state(&format!("qoder-cache-{changed}"));
            let origin = format!("http://{address}");
            let (provider_id, account_id) = install_qoder_provider(
                &state,
                &format!("qoder-cache-{changed}"),
                AppKind::Codex,
                QoderFixtureRail::GlobalOauth,
                &origin,
            )
            .await;
            let share_id = format!("qoder-cache-{changed}-share");
            install_qoder_share(
                &state,
                &share_id,
                AppKind::Codex,
                &provider_id,
                "owner@example.com",
            )
            .await;
            let first = forward_qoder_surface(
                state.clone(),
                AppKind::Codex,
                provider_id.clone(),
                &share_id,
                "owner@example.com",
                "same-session",
                false,
            )
            .await
            .unwrap();
            let _ = collect(first).await;
            assert_eq!(fixture.count(&fixture.model_requests), 1);
            let original_account = state.find_account_by_id(&account_id).await.unwrap();

            state
                .mutate_accounts_immediate(|accounts| {
                    let account = accounts
                        .accounts
                        .iter_mut()
                        .find(|account| account.id == account_id)
                        .unwrap();
                    match changed {
                        "credential" => {
                            account.access_token = Some("same-generation-new-token".to_string())
                        }
                        "machine" => {
                            account.profile.as_mut().unwrap()["machineId"] =
                                json!("abcdef0123456789abcdef0123456789abcd")
                        }
                        _ => unreachable!(),
                    }
                })
                .await
                .unwrap();
            let second = forward_qoder_surface(
                state.clone(),
                AppKind::Codex,
                provider_id.clone(),
                &share_id,
                "owner@example.com",
                "same-session",
                false,
            )
            .await
            .unwrap();
            let _ = collect(second).await;
            assert_eq!(fixture.count(&fixture.model_requests), 2, "{changed}");
            assert_eq!(fixture.count(&fixture.generation_requests), 2, "{changed}");
            let account = state.find_account_by_id(&account_id).await.unwrap();
            assert_eq!(
                account.auth_identity_generation, original_account.auth_identity_generation,
                "{changed}"
            );
            assert_eq!(
                account.token_refresh_generation, original_account.token_refresh_generation,
                "{changed}"
            );
            server.abort();
        }
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

// These fixtures observe the production COSY loops on local listeners. Vendor
// acceptance and per-rail credential validation remain separate live gates.
async fn qoder_core03_attempt(
    state: &ServerState,
    route: ProxyRoute,
    id: &str,
) -> ForwardAttemptContext {
    let accounts = state.accounts_snapshot().await;
    let providers = state.providers.read().await;
    let in_flight = state.account_in_flight.snapshot();
    let execution = select_test_provider(&providers, &accounts, route.app(), id, Some(&in_flight))
        .unwrap()
        .execution;
    ForwardAttemptContext {
        execution: Some(execution),
        provider_binding_pinned: true,
        ..Default::default()
    }
}

#[test]
fn qoder_core03_three_rails_four_surfaces_stream_and_nonstream() {
    run_qoder_async_test(async {
        for rail in [
            QoderFixtureRail::GlobalOauth,
            QoderFixtureRail::CnOauth,
            QoderFixtureRail::Pat,
        ] {
            for route in [
                ProxyRoute::ClaudeMessages,
                ProxyRoute::CodexResponses,
                ProxyRoute::CodexChatCompletions,
                ProxyRoute::Gemini,
            ] {
                for streaming in [false, true] {
                    let (address, fixture, server) =
                        spawn_qoder_upstream(rail, vec![QoderGenerationReply::Success]).await;
                    let state = qoder_test_state("qoder-core03-surfaces");
                    let (id, _) = install_qoder_provider(
                        &state,
                        "qoder-core03",
                        route.app(),
                        rail,
                        &format!("http://{address}"),
                    )
                    .await;
                    let share = "qoder-core03-share";
                    install_qoder_share(&state, share, route.app(), &id, "owner@example.com").await;
                    let (_, path, mut body) = surface_request(route.app(), streaming);
                    if route == ProxyRoute::CodexChatCompletions {
                        body = Bytes::from(json!({"model":MODEL_KEY,"stream":streaming,"messages":[{"role":"user","content":"hi"}]}).to_string());
                    }
                    let attempt = qoder_core03_attempt(&state, route, &id).await;
                    let slot = attempt.timing.clone();
                    let response = forward_with_attempt(
                        state,
                        route,
                        path,
                        qoder_headers(share, "owner@example.com", "core03"),
                        body,
                        attempt,
                    )
                    .await
                    .unwrap();
                    assert_eq!(response.status(), StatusCode::OK);
                    let bytes = collect(response).await;
                    assert!(String::from_utf8_lossy(&bytes).contains("ready"));
                    let snapshot = slot.get().unwrap().request().snapshot();
                    assert_eq!(snapshot["providerType"], "qoder_cosy");
                    assert_eq!(snapshot["result"], "complete", "{snapshot}");
                    for phase in [
                        "admission",
                        "send_started",
                        "headers",
                        "first_semantic",
                        "terminal",
                        "http_body_handoff",
                        "owner_released",
                    ] {
                        assert!(
                            snapshot["milestonesMs"][phase].is_number(),
                            "{phase}: {snapshot}"
                        );
                    }
                    assert!(snapshot["milestonesMs"]["downstream_write_completed"].is_null());
                    assert_eq!(fixture.count(&fixture.generation_requests), 1);
                    server.abort();
                }
            }
        }
    });
}

#[test]
fn qoder_core03_decoder_observes_content_before_eof_but_defers_terminal() {
    let slot = TimingSlot::default();
    let timing = slot
        .begin_for_provider(ProviderType::QoderCosy, "messages", Instant::now())
        .unwrap();
    let memory = RequestMemoryBudget::new(1024 * 1024);
    memory.set_timing(timing.clone());
    let mut decoder =
        super::super::qoder::QoderSseDecoder::with_request_memory(Some(memory)).unwrap();
    decoder
        .push(Bytes::from_static(b": keepalive\n\n"))
        .unwrap();
    let mut control = inner_success_chunk(None, false);
    control["choices"][0]["delta"] = json!({"role":"assistant","content":""});
    control["usage"] = json!({"prompt_tokens":0,"completion_tokens":0,"total_tokens":0});
    decoder.push(qoder_wrapper(json!("OK"), control)).unwrap();
    assert!(timing.request().snapshot()["milestonesMs"]["first_semantic"].is_null());
    let content = qoder_wrapper(json!("OK"), inner_success_chunk(Some("ready"), false));
    decoder.push(content.slice(..8)).unwrap();
    assert!(timing.request().snapshot()["milestonesMs"]["first_semantic"].is_null());
    decoder.push(content.slice(8..)).unwrap();
    assert!(timing.request().snapshot()["milestonesMs"]["first_semantic"].is_number());
    let mut terminal = inner_success_chunk(None, true);
    terminal["choices"][0]["finish_reason"] = json!("length");
    decoder.push(qoder_wrapper(json!("OK"), terminal)).unwrap();
    assert!(timing.request().snapshot()["milestonesMs"]["terminal"].is_null());
    decoder.finish().unwrap();
    timing.finish_delivery(true);
    assert_eq!(timing.request().snapshot()["result"], "incomplete");
}

#[derive(Clone, Default)]
struct QoderCore03Samples(Arc<Mutex<Vec<metrics::Key>>>);
struct QoderCore03Histogram(QoderCore03Samples, metrics::Key);
impl metrics::HistogramFn for QoderCore03Histogram {
    fn record(&self, _: f64) {
        self.0 .0.lock().unwrap().push(self.1.clone());
    }
}
impl metrics::Recorder for QoderCore03Samples {
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
        metrics::Histogram::from_arc(Arc::new(QoderCore03Histogram(self.clone(), key.clone())))
    }
}

#[test]
fn qoder_core03_original_auth_recovery_keeps_separate_attempts_on_every_rail() {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            for rail in [
                QoderFixtureRail::GlobalOauth,
                QoderFixtureRail::CnOauth,
                QoderFixtureRail::Pat,
            ] {
                let samples = QoderCore03Samples::default();
                let _recorder = metrics::set_default_local_recorder(&samples);
                let (address, fixture, server) = spawn_qoder_upstream(
                    rail,
                    vec![
                        QoderGenerationReply::HttpUnauthorized,
                        QoderGenerationReply::Success,
                    ],
                )
                .await;
                let state = qoder_test_state("qoder-core03-retry");
                let (id, _) = install_qoder_provider(
                    &state,
                    "qoder-core03",
                    AppKind::Codex,
                    rail,
                    &format!("http://{address}"),
                )
                .await;
                install_qoder_share(
                    &state,
                    "core03-share",
                    AppKind::Codex,
                    &id,
                    "owner@example.com",
                )
                .await;
                let attempt = qoder_core03_attempt(&state, ProxyRoute::CodexResponses, &id).await;
                let slot = attempt.timing.clone();
                let (_, path, body) = surface_request(AppKind::Codex, true);
                let response = forward_with_attempt(
                    state,
                    ProxyRoute::CodexResponses,
                    path,
                    qoder_headers("core03-share", "owner@example.com", "retry"),
                    body,
                    attempt,
                )
                .await
                .unwrap();
                collect(response).await;
                let snapshot = slot.get().unwrap().request().snapshot();
                assert_eq!(snapshot["result"], "complete");
                assert!(snapshot["waitsMs"]["retry"].as_f64().unwrap() > 0.0);
                let samples = samples.0.lock().unwrap();
                for (scope, result) in [
                    ("attempt", "retried"),
                    ("attempt", "complete"),
                    ("request", "complete"),
                ] {
                    assert_eq!(
                        samples
                            .iter()
                            .filter(|key| key.name() == "cc_switch_forward_duration_seconds"
                                && [
                                    ("provider_type", "qoder_cosy"),
                                    ("scope", scope),
                                    ("result", result)
                                ]
                                .iter()
                                .all(|(name, value)| key
                                    .labels()
                                    .any(|label| label.key() == *name && label.value() == *value)))
                            .count(),
                        1
                    );
                }
                assert_eq!(fixture.count(&fixture.generation_requests), 2);
                assert_eq!(
                    fixture.count(&fixture.refresh_requests),
                    usize::from(rail != QoderFixtureRail::Pat)
                );
                server.abort();
            }
        });
}

#[test]
fn qoder_core03_headers_cancel_and_committed_abort_keep_real_owner_and_no_replay() {
    run_qoder_async_test(async {
        for mode in 0..3 {
            let fixture = QoderFixtureState::new(QoderFixtureRail::GlobalOauth, vec![]);
            let calls = Arc::new(AtomicUsize::new(0));
            let observed = calls.clone();
            let app=axum::Router::new().route(QODER_MODEL_LIST_PATH,get(qoder_model_list))
                .route(QODER_GENERATION_PATH.split('?').next().unwrap(),post(move || {
                    observed.fetch_add(1,Ordering::SeqCst);
                    async move {
                        if mode==0 {std::future::pending::<()>().await;}
                        let chunks=async_stream::stream! {
                            yield Ok::<_,std::convert::Infallible>(qoder_wrapper(json!("OK"),inner_success_chunk(Some("ready"),false)));
                            if mode==1 {std::future::pending::<()>().await;}
                        };
                        Response::builder().header(CONTENT_TYPE,"text/event-stream").body(Body::from_stream(chunks)).unwrap()
                    }
                })).with_state(fixture);
            let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
                .await
                .unwrap();
            let address = listener.local_addr().unwrap();
            let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
            let state = qoder_test_state("qoder-core03-cancel");
            let (id, _) = install_qoder_provider(
                &state,
                "qoder-core03",
                AppKind::Claude,
                QoderFixtureRail::GlobalOauth,
                &format!("http://{address}"),
            )
            .await;
            install_qoder_share(
                &state,
                "core03-share",
                AppKind::Claude,
                &id,
                "owner@example.com",
            )
            .await;
            let attempt = qoder_core03_attempt(&state, ProxyRoute::ClaudeMessages, &id).await;
            let slot = attempt.timing.clone();
            let (_, path, body) = surface_request(AppKind::Claude, true);
            let mut pending = Box::pin(forward_with_attempt(
                state,
                ProxyRoute::ClaudeMessages,
                path,
                qoder_headers("core03-share", "owner@example.com", "cancel"),
                body,
                attempt,
            ));
            if mode == 0 {
                assert!(
                    tokio::time::timeout(Duration::from_millis(100), pending.as_mut())
                        .await
                        .is_err()
                );
                let request = slot.get().unwrap().request();
                assert!(request.snapshot()["milestonesMs"]["send_started"].is_number());
                assert!(request.snapshot()["milestonesMs"]["headers"].is_null());
                drop(pending);
                assert_eq!(request.snapshot()["result"], "cancelled");
                assert!(request.snapshot()["milestonesMs"]["owner_released"].is_number());
            } else {
                let response = pending.await.unwrap();
                let request = slot.get().unwrap().request();
                let mut stream = response.into_body().into_data_stream();
                let retained = loop {
                    let frame = stream.next().await.unwrap().unwrap();
                    if String::from_utf8_lossy(&frame).contains("ready") {
                        break frame;
                    }
                };
                if mode == 2 {
                    while let Some(frame) = stream.next().await {
                        drop(frame);
                    }
                }
                drop(stream);
                assert_eq!(
                    request.snapshot()["result"],
                    if mode == 2 { "failed" } else { "cancelled" }
                );
                assert!(request.snapshot()["milestonesMs"]["owner_released"].is_null());
                drop(retained);
                assert!(request.snapshot()["milestonesMs"]["owner_released"].is_number());
            }
            assert_eq!(calls.load(Ordering::SeqCst), 1);
            server.abort();
        }
    });
}
