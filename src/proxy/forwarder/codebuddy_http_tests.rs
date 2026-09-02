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

use crate::domain::codebuddy::{
    CodeBuddySite, CODEBUDDY_ACCOUNTS_PATH, CODEBUDDY_CHAT_PATH, CODEBUDDY_CONFIG_PATH,
    CODEBUDDY_REFRESH_PATH,
};
use crate::domain::providers::model::{AppKind, AuthBinding, Provider, ProviderMeta, ProviderType};
use crate::domain::providers::registry::ProfileId;
use crate::domain::providers::store::{ProviderStore, StoredProvider};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CodeBuddyGenerationReply {
    Success,
    HttpUnauthorized,
    LateUnauthorized,
    DuplicateDone,
    DataAfterDone,
}

#[derive(Debug, Clone)]
struct CodeBuddyRequestObservation {
    uri: String,
    headers: BTreeMap<String, String>,
    body: Value,
}

#[derive(Clone)]
struct CodeBuddyFixtureState {
    site: CodeBuddySite,
    replies: Arc<Vec<CodeBuddyGenerationReply>>,
    generation_requests: Arc<AtomicUsize>,
    config_requests: Arc<AtomicUsize>,
    refresh_requests: Arc<AtomicUsize>,
    account_requests: Arc<AtomicUsize>,
    observations: Arc<Mutex<Vec<CodeBuddyRequestObservation>>>,
}

impl CodeBuddyFixtureState {
    fn new(site: CodeBuddySite, replies: Vec<CodeBuddyGenerationReply>) -> Self {
        Self {
            site,
            replies: Arc::new(replies),
            generation_requests: Default::default(),
            config_requests: Default::default(),
            refresh_requests: Default::default(),
            account_requests: Default::default(),
            observations: Default::default(),
        }
    }

    fn count(&self, counter: &AtomicUsize) -> usize {
        counter.load(Ordering::SeqCst)
    }

    fn generation_observations(&self) -> Vec<CodeBuddyRequestObservation> {
        self.observations.lock().unwrap().clone()
    }
}

fn site_model(site: CodeBuddySite) -> &'static str {
    match site {
        CodeBuddySite::Intl => "default-model",
        CodeBuddySite::Cn => "default",
    }
}

fn site_domain(site: CodeBuddySite) -> &'static str {
    match site {
        CodeBuddySite::Intl => "www.codebuddy.ai",
        CodeBuddySite::Cn => "copilot.tencent.com",
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

async fn codebuddy_config(
    State(state): State<CodeBuddyFixtureState>,
    headers: HeaderMap,
) -> Response {
    state.config_requests.fetch_add(1, Ordering::SeqCst);
    let headers = observed_headers(&headers);
    assert_eq!(
        headers.get("x-domain").map(String::as_str),
        Some(site_domain(state.site))
    );
    Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({
                "code": 0,
                "data": {
                    "enterpriseId": "",
                    "models": [{
                        "id": site_model(state.site),
                        "name": "Fixture model",
                        "supportsToolCall": true,
                        "supportsReasoning": true,
                        "reasoning": {"supportedEfforts": ["low", "high"]},
                        "maxInputTokens": 200000,
                        "maxOutputTokens": 8192
                    }]
                }
            })
            .to_string(),
        ))
        .unwrap()
}

async fn codebuddy_accounts(State(state): State<CodeBuddyFixtureState>) -> Response {
    state.account_requests.fetch_add(1, Ordering::SeqCst);
    Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({"code": 0, "data": {"uid": "fixture-user"}}).to_string(),
        ))
        .unwrap()
}

async fn codebuddy_refresh(
    State(state): State<CodeBuddyFixtureState>,
    headers: HeaderMap,
) -> Response {
    let request = state.refresh_requests.fetch_add(1, Ordering::SeqCst);
    let headers = observed_headers(&headers);
    assert!(headers
        .get("x-refresh-token")
        .is_some_and(|value| !value.is_empty()));
    assert_eq!(
        headers.get("x-auth-refresh-source").map(String::as_str),
        Some("plugin")
    );
    assert_eq!(
        headers.get("x-domain").map(String::as_str),
        Some(site_domain(state.site))
    );
    Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({
                "code": 0,
                "data": {
                    "accessToken": format!("refreshed-access-{request}"),
                    "refreshToken": format!("refreshed-refresh-{request}"),
                    "tokenType": "Bearer",
                    "expiresIn": 3600,
                    "domain": site_domain(state.site)
                }
            })
            .to_string(),
        ))
        .unwrap()
}

fn chat_chunk(content: &str, finish_reason: Option<&str>) -> Bytes {
    Bytes::from(format!(
        "data: {}\n\n",
        json!({
            "id": "chatcmpl-codebuddy-fixture",
            "object": "chat.completion.chunk",
            "created": 1,
            "model": "fixture-model",
            "choices": [{
                "index": 0,
                "delta": {"content": content, "reasoning_content": if content.is_empty() { "" } else { "think" }},
                "finish_reason": finish_reason
            }]
        })
    ))
}

fn tool_chunks() -> Vec<Bytes> {
    vec![
        Bytes::from(format!(
            "data: {}\n\n",
            json!({
                "id": "chatcmpl-codebuddy-fixture",
                "object": "chat.completion.chunk",
                "created": 1,
                "model": "fixture-model",
                "choices": [{
                    "index": 0,
                    "delta": {
                        "content": "ready",
                        "reasoning_content": "think",
                        "tool_calls": [{
                            "index": 0,
                            "id": "call-fixture",
                            "type": "function",
                            "function": {"name": "lookup", "arguments": "{\"q\":"}
                        }]
                    },
                    "finish_reason": null
                }]
            })
        )),
        Bytes::from(format!(
            "data: {}\n\n",
            json!({
                "id": "chatcmpl-codebuddy-fixture",
                "object": "chat.completion.chunk",
                "created": 1,
                "model": "fixture-model",
                "choices": [{
                    "index": 0,
                    "delta": {"tool_calls": [{"index": 0, "function": {"arguments": "\"fixture\"}"}}]},
                    "finish_reason": "tool_calls"
                }]
            })
        )),
        Bytes::from(format!(
            "data: {}\n\n",
            json!({
                "id": "chatcmpl-codebuddy-fixture",
                "object": "chat.completion.chunk",
                "created": 1,
                "model": "fixture-model",
                "choices": [],
                "usage": {
                    "prompt_tokens": 10,
                    "completion_tokens": 3,
                    "total_tokens": 13,
                    "credit": 0.02,
                    "completion_thinking_tokens": 2,
                    "cached_tokens": 1,
                    "cache_read_input_tokens": 1,
                    "cache_creation_input_tokens": 0,
                    "prompt_cache_hit_tokens": 1,
                    "prompt_cache_miss_tokens": 9,
                    "prompt_cache_write_tokens": 0,
                    "completion_tokens_details": {},
                    "prompt_tokens_details": {}
                }
            })
        )),
    ]
}

fn sse_response(chunks: Vec<Bytes>) -> Response {
    let stream = async_stream::stream! {
        for chunk in chunks {
            yield Ok::<Bytes, std::convert::Infallible>(chunk);
            tokio::task::yield_now().await;
        }
    };
    Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, "text/event-stream")
        .body(Body::from_stream(stream))
        .unwrap()
}

fn success_response() -> Response {
    let mut chunks = tool_chunks();
    // Split the terminal across network body frames. The decoder must not
    // publish it until the following EOF validates the complete lifecycle.
    chunks.push(Bytes::from_static(b"data: [DO"));
    chunks.push(Bytes::from_static(b"NE]\n\n"));
    sse_response(chunks)
}

async fn codebuddy_generation(
    State(state): State<CodeBuddyFixtureState>,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let body: Value = serde_json::from_slice(&body).unwrap();
    state
        .observations
        .lock()
        .unwrap()
        .push(CodeBuddyRequestObservation {
            uri: uri.to_string(),
            headers: observed_headers(&headers),
            body,
        });
    let request = state.generation_requests.fetch_add(1, Ordering::SeqCst);
    let reply = state
        .replies
        .get(request)
        .copied()
        .or_else(|| state.replies.last().copied())
        .unwrap_or(CodeBuddyGenerationReply::Success);
    match reply {
        CodeBuddyGenerationReply::Success => success_response(),
        CodeBuddyGenerationReply::HttpUnauthorized => Response::builder()
            .status(StatusCode::UNAUTHORIZED)
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(json!({"message": "expired"}).to_string()))
            .unwrap(),
        CodeBuddyGenerationReply::LateUnauthorized => sse_response(vec![
            chat_chunk("discard-me", None),
            Bytes::from_static(b"data: {\"code\":12005,\"message\":\"expired\"}\n\n"),
        ]),
        CodeBuddyGenerationReply::DuplicateDone => {
            let mut chunks = tool_chunks();
            chunks.push(Bytes::from_static(b"data: [DONE]\n\n"));
            chunks.push(Bytes::from_static(b"data: [DONE]\n\n"));
            sse_response(chunks)
        }
        CodeBuddyGenerationReply::DataAfterDone => {
            let mut chunks = tool_chunks();
            chunks.push(Bytes::from_static(b"data: [DONE]\n\n"));
            chunks.push(chat_chunk("too-late", Some("stop")));
            sse_response(chunks)
        }
    }
}

async fn spawn_codebuddy_upstream(
    site: CodeBuddySite,
    replies: Vec<CodeBuddyGenerationReply>,
) -> (
    std::net::SocketAddr,
    CodeBuddyFixtureState,
    tokio::task::JoinHandle<()>,
) {
    let state = CodeBuddyFixtureState::new(site, replies);
    let app = axum::Router::new()
        .route(CODEBUDDY_CONFIG_PATH, get(codebuddy_config))
        .route(CODEBUDDY_ACCOUNTS_PATH, get(codebuddy_accounts))
        .route(CODEBUDDY_REFRESH_PATH, post(codebuddy_refresh))
        .route(CODEBUDDY_CHAT_PATH, post(codebuddy_generation))
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

fn codebuddy_test_state(name: &str) -> ServerState {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    crate::state::ServerStateInner::load(
        crate::cli::Cli {
            host: std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
            port: 0,
            config_dir: Some(
                std::env::temp_dir().join(format!("cc-switch-server-codebuddy-{name}-{nanos}")),
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

fn codebuddy_profile_id(app: AppKind) -> &'static str {
    match app {
        AppKind::Claude => "claude.codebuddy_oauth",
        AppKind::Codex => "codex.codebuddy_oauth",
        AppKind::Gemini => "gemini.codebuddy_oauth",
    }
}

fn codebuddy_stored_provider(name: &str, app: AppKind, account_id: &str) -> StoredProvider {
    let mut stored = StoredProvider {
        app,
        provider: Provider {
            id: format!("{name}-provider"),
            name: format!("CodeBuddy {name}"),
            settings_config: json!({}),
            category: None,
            meta: Some(ProviderMeta {
                auth_binding: Some(AuthBinding {
                    source: Some("account_store".to_string()),
                    auth_provider: Some("codebuddy_oauth".to_string()),
                    account_id: Some(account_id.to_string()),
                    auth_identity_generation: Some(1),
                }),
                provider_type: Some("codebuddy_oauth".to_string()),
                ..ProviderMeta::default()
            }),
            extra: Default::default(),
        },
        provider_type: ProviderType::CodeBuddyOAuth,
        provider_type_id: "codebuddy_oauth".to_string(),
        resource: Default::default(),
    };
    stored.resource.profile_id = Some(ProfileId::parse(codebuddy_profile_id(app)).unwrap());
    stored.resource.profile_schema_revision = Some(1);
    stored.resource.revision = 1;
    stored
}

async fn install_codebuddy_account(
    state: &ServerState,
    name: &str,
    site: CodeBuddySite,
    origin: &str,
) -> String {
    let account_id = format!("{name}-account");
    let account_id_for_state = account_id.clone();
    let origin = origin.trim_end_matches('/').to_string();
    state
        .mutate_accounts_immediate(move |accounts| {
            accounts.upsert(
                serde_json::from_value(json!({
                    "id": account_id_for_state,
                    "providerType": "codebuddy_oauth",
                    "accessToken": "initial-access",
                    "refreshToken": "initial-refresh",
                    "tokenType": "Bearer",
                    "expiresAt": i64::MAX / 2,
                    "profile": {
                        "site": site.as_str(),
                        "domain": site_domain(site),
                        "uid": "fixture-user",
                        "enterpriseId": "",
                        "name": "Fixture User",
                        "clientVersion": crate::domain::codebuddy::CODEBUDDY_CLIENT_VERSION,
                        "productPlatform": crate::domain::codebuddy::CODEBUDDY_PLATFORM
                    },
                    "raw": {
                        "observedAtMs": chrono::Utc::now().timestamp_millis(),
                        "codeBuddyRefreshReceipt": {
                            "site": site.as_str(),
                            "domain": site_domain(site),
                            "receivedAtMs": chrono::Utc::now().timestamp_millis()
                        },
                        "testCodeBuddyBaseUrl": origin
                    }
                }))
                .unwrap(),
            );
        })
        .await
        .unwrap();
    account_id
}

async fn install_codebuddy_provider(
    state: &ServerState,
    name: &str,
    app: AppKind,
    site: CodeBuddySite,
    origin: &str,
) -> (String, String) {
    let account_id = install_codebuddy_account(state, name, site, origin).await;
    let stored = codebuddy_stored_provider(name, app, &account_id);
    let provider_id = stored.provider.id.clone();
    let accounts = state.accounts_snapshot().await;
    let mut providers = ProviderStore {
        providers: vec![stored],
        ..ProviderStore::default()
    };
    providers.rebuild_runtime_index(&accounts).unwrap();
    state.replace_provider_store_for_test(providers).await;
    (provider_id, account_id)
}

async fn install_codebuddy_decoy(
    state: &ServerState,
    name: &str,
    app: AppKind,
    site: CodeBuddySite,
    origin: &str,
) {
    let account_id = install_codebuddy_account(state, name, site, origin).await;
    let stored = codebuddy_stored_provider(name, app, &account_id);
    let accounts = state.accounts_snapshot().await;
    let mut providers = state.providers.read().await.clone();
    providers.providers.push(stored);
    providers.rebuild_runtime_index(&accounts).unwrap();
    state.replace_provider_store_for_test(providers).await;
}

async fn install_codebuddy_share(
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
                    "providerType": "codebuddy_oauth",
                    "ownerEmail": owner,
                    "enabled": true,
                    "status": "active",
                    "bindings": [{
                        "app": app,
                        "providerId": provider_id,
                        "providerType": "codebuddy_oauth"
                    }],
                    "userGrants": {
                        owner.clone(): {"email": owner, "role": "owner", "active": true}
                    }
                }))
                .unwrap(),
            );
        })
        .await
        .unwrap();
}

fn codebuddy_headers(share_id: &str, user: &str, session: &str) -> HeaderMap {
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
                    "model": "auto",
                    "system": "claude-system",
                    "max_tokens": 64,
                    "thinking": {"type": "enabled", "budget_tokens": 1024},
                    "tools": [{"name": "lookup", "description": "Lookup", "input_schema": {"type": "object", "properties": {"q": {"type": "string"}}}}],
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
                    "model": "auto",
                    "instructions": "codex-system",
                    "input": "codex-user",
                    "reasoning": {"effort": "high"},
                    "tools": [{"type": "function", "name": "lookup", "description": "Lookup", "parameters": {"type": "object", "properties": {"q": {"type": "string"}}}}],
                    "stream": stream_requested
                })
                .to_string(),
            ),
        ),
        AppKind::Gemini => (
            ProxyRoute::Gemini,
            Some(format!(
                "models/auto:{}",
                if stream_requested {
                    "streamGenerateContent"
                } else {
                    "generateContent"
                }
            )),
            Bytes::from(
                json!({
                    "systemInstruction": {"parts": [{"text": "gemini-system"}]},
                    "contents": [{"role": "user", "parts": [{"text": "gemini-user"}]}],
                    "tools": [{"functionDeclarations": [{"name": "lookup", "description": "Lookup", "parameters": {"type": "object", "properties": {"q": {"type": "string"}}}}]}]
                })
                .to_string(),
            ),
        ),
    }
}

async fn forward_codebuddy_surface(
    state: ServerState,
    app: AppKind,
    provider_id: String,
    share_id: &str,
    stream_requested: bool,
) -> Result<Response, ProxyError> {
    let (route, gemini_path, body) = surface_request(app, stream_requested);
    super::forward_for_test_surface(
        state,
        route,
        provider_id,
        gemini_path,
        codebuddy_headers(share_id, "owner@example.com", "fixture-session"),
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

fn run_codebuddy_async_test<F>(future: F)
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    std::thread::Builder::new()
        .name("codebuddy-http-fixture".to_string())
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

fn assert_generation_observation(observation: &CodeBuddyRequestObservation, site: CodeBuddySite) {
    assert_eq!(observation.uri, CODEBUDDY_CHAT_PATH);
    assert_eq!(
        observation.headers.get("x-user-id").map(String::as_str),
        Some("fixture-user")
    );
    assert_eq!(
        observation.headers.get("x-domain").map(String::as_str),
        Some(site_domain(site))
    );
    assert_eq!(
        observation.headers.get("x-product").map(String::as_str),
        Some("SaaS")
    );
    assert_eq!(
        observation
            .headers
            .get("x-no-enterprise-id")
            .map(String::as_str),
        Some("true")
    );
    assert_eq!(
        observation.headers.get("user-agent").map(String::as_str),
        Some("CLI/2.142.0 CodeBuddy/2.142.0")
    );
    assert!(observation
        .headers
        .get("authorization")
        .is_some_and(|value| value.starts_with("Bearer ")));
    let request_id = observation.headers.get("x-request-id").unwrap();
    for header in [
        "x-conversation-id",
        "x-conversation-request-id",
        "x-conversation-message-id",
        "x-root-request-id",
    ] {
        assert_eq!(observation.headers.get(header), Some(request_id));
    }
    match site {
        CodeBuddySite::Intl => {
            assert!(!observation.headers.contains_key("origin"));
            assert!(!observation.headers.contains_key("referer"));
        }
        CodeBuddySite::Cn => {
            assert_eq!(
                observation.headers.get("origin").map(String::as_str),
                Some("https://www.codebuddy.cn")
            );
            assert_eq!(
                observation.headers.get("referer").map(String::as_str),
                Some("https://www.codebuddy.cn/")
            );
        }
    }
    assert_eq!(observation.body["model"], site_model(site));
    assert_eq!(observation.body["stream"], true);
    assert_eq!(observation.body["stream_options"]["include_usage"], true);
    assert_eq!(observation.body["messages"][0]["role"], "system");
    assert_eq!(observation.body["tools"][0]["function"]["name"], "lookup");
}

#[test]
fn codebuddy_http_fixture_covers_both_sites_all_surfaces_and_response_modes() {
    run_codebuddy_async_test(async {
        for site in [CodeBuddySite::Intl, CodeBuddySite::Cn] {
            for app in [AppKind::Claude, AppKind::Codex, AppKind::Gemini] {
                for stream_requested in [false, true] {
                    let name = format!(
                        "codebuddy-{}-{}-{}",
                        site.as_str(),
                        app.as_str(),
                        if stream_requested { "stream" } else { "json" }
                    );
                    let (address, fixture, server) =
                        spawn_codebuddy_upstream(site, vec![CodeBuddyGenerationReply::Success])
                            .await;
                    let state = codebuddy_test_state(&name);
                    let origin = format!("http://{address}");
                    let (provider_id, _) =
                        install_codebuddy_provider(&state, &name, app, site, &origin).await;
                    let share_id = format!("{name}-share");
                    install_codebuddy_share(
                        &state,
                        &share_id,
                        app,
                        &provider_id,
                        "owner@example.com",
                    )
                    .await;

                    let response = forward_codebuddy_surface(
                        state,
                        app,
                        provider_id,
                        &share_id,
                        stream_requested,
                    )
                    .await
                    .unwrap();
                    assert_eq!(response.status(), StatusCode::OK, "{name}");
                    let body = String::from_utf8(collect(response).await).unwrap();
                    assert!(body.contains("ready"), "{name}: {body}");
                    assert!(body.contains("lookup"), "{name}: {body}");
                    assert!(body.contains("think"), "{name}: {body}");
                    if stream_requested {
                        assert!(
                            body.contains("message_stop")
                                || body.contains("response.completed")
                                || body.contains("finishReason"),
                            "{name}: {body}"
                        );
                    }
                    assert_eq!(fixture.count(&fixture.generation_requests), 1, "{name}");
                    assert_eq!(fixture.count(&fixture.config_requests), 1, "{name}");
                    assert_eq!(fixture.count(&fixture.refresh_requests), 0, "{name}");
                    let observations = fixture.generation_observations();
                    assert_eq!(observations.len(), 1, "{name}");
                    assert_generation_observation(&observations[0], site);
                    server.abort();
                }
            }
        }
    });
}

#[test]
fn codebuddy_nonstream_recovers_late_auth_once_on_only_the_bound_account() {
    run_codebuddy_async_test(async {
        let (address, fixture, server) = spawn_codebuddy_upstream(
            CodeBuddySite::Intl,
            vec![
                CodeBuddyGenerationReply::LateUnauthorized,
                CodeBuddyGenerationReply::Success,
            ],
        )
        .await;
        let (decoy_address, decoy, decoy_server) =
            spawn_codebuddy_upstream(CodeBuddySite::Cn, vec![CodeBuddyGenerationReply::Success])
                .await;
        let state = codebuddy_test_state("codebuddy-late-auth");
        let origin = format!("http://{address}");
        let (provider_id, account_id) = install_codebuddy_provider(
            &state,
            "codebuddy-late-auth",
            AppKind::Codex,
            CodeBuddySite::Intl,
            &origin,
        )
        .await;
        install_codebuddy_decoy(
            &state,
            "codebuddy-decoy",
            AppKind::Codex,
            CodeBuddySite::Cn,
            &format!("http://{decoy_address}"),
        )
        .await;
        let share_id = "codebuddy-late-auth-share";
        install_codebuddy_share(
            &state,
            share_id,
            AppKind::Codex,
            &provider_id,
            "owner@example.com",
        )
        .await;
        let initial_token_generation = state
            .find_account_by_id(&account_id)
            .await
            .unwrap()
            .token_refresh_generation;

        let response =
            forward_codebuddy_surface(state.clone(), AppKind::Codex, provider_id, share_id, false)
                .await
                .unwrap();
        let body = String::from_utf8(collect(response).await).unwrap();
        assert!(body.contains("ready"), "{body}");
        assert!(!body.contains("discard-me"), "{body}");
        assert_eq!(fixture.count(&fixture.generation_requests), 2);
        assert_eq!(fixture.count(&fixture.refresh_requests), 1);
        assert_eq!(decoy.count(&decoy.generation_requests), 0);
        assert_eq!(decoy.count(&decoy.config_requests), 0);
        assert_eq!(decoy.count(&decoy.refresh_requests), 0);
        let account = state.find_account_by_id(&account_id).await.unwrap();
        assert_eq!(account.auth_identity_generation, 1);
        assert_eq!(
            account.token_refresh_generation,
            initial_token_generation + 1
        );
        let observations = fixture.generation_observations();
        assert_eq!(observations.len(), 2);
        assert_eq!(
            observations[0]
                .headers
                .get("authorization")
                .map(String::as_str),
            Some("Bearer initial-access")
        );
        assert_eq!(
            observations[1]
                .headers
                .get("authorization")
                .map(String::as_str),
            Some("Bearer refreshed-access-0")
        );
        server.abort();
        decoy_server.abort();
    });
}

#[test]
fn codebuddy_rejects_duplicate_or_post_terminal_data_across_http_chunks() {
    run_codebuddy_async_test(async {
        for reply in [
            CodeBuddyGenerationReply::DuplicateDone,
            CodeBuddyGenerationReply::DataAfterDone,
        ] {
            let name = format!("codebuddy-terminal-{reply:?}");
            let (address, fixture, server) =
                spawn_codebuddy_upstream(CodeBuddySite::Intl, vec![reply]).await;
            let state = codebuddy_test_state(&name);
            let (provider_id, _) = install_codebuddy_provider(
                &state,
                &name,
                AppKind::Codex,
                CodeBuddySite::Intl,
                &format!("http://{address}"),
            )
            .await;
            let share_id = format!("{name}-share");
            install_codebuddy_share(
                &state,
                &share_id,
                AppKind::Codex,
                &provider_id,
                "owner@example.com",
            )
            .await;
            let error =
                forward_codebuddy_surface(state, AppKind::Codex, provider_id, &share_id, false)
                    .await
                    .unwrap_err();
            assert_eq!(error.status, StatusCode::BAD_GATEWAY, "{name}");
            assert_eq!(fixture.count(&fixture.generation_requests), 1, "{name}");
            assert_eq!(fixture.count(&fixture.refresh_requests), 0, "{name}");
            server.abort();
        }
    });
}

#[test]
fn codebuddy_stream_never_refreshes_after_first_business_output_and_second_auth_is_terminal() {
    run_codebuddy_async_test(async {
        let (address, fixture, server) = spawn_codebuddy_upstream(
            CodeBuddySite::Intl,
            vec![CodeBuddyGenerationReply::LateUnauthorized],
        )
        .await;
        let state = codebuddy_test_state("codebuddy-stream-postcommit-auth");
        let (provider_id, _) = install_codebuddy_provider(
            &state,
            "codebuddy-stream-postcommit-auth",
            AppKind::Codex,
            CodeBuddySite::Intl,
            &format!("http://{address}"),
        )
        .await;
        let share_id = "codebuddy-stream-postcommit-auth-share";
        install_codebuddy_share(
            &state,
            share_id,
            AppKind::Codex,
            &provider_id,
            "owner@example.com",
        )
        .await;
        let response =
            forward_codebuddy_surface(state, AppKind::Codex, provider_id, share_id, true)
                .await
                .unwrap();
        let body = String::from_utf8(collect(response).await).unwrap();
        assert!(body.contains("discard-me"), "{body}");
        assert!(!body.contains("response.completed"), "{body}");
        assert_eq!(fixture.count(&fixture.generation_requests), 1);
        assert_eq!(fixture.count(&fixture.refresh_requests), 0);
        server.abort();

        let (address, fixture, server) = spawn_codebuddy_upstream(
            CodeBuddySite::Intl,
            vec![
                CodeBuddyGenerationReply::HttpUnauthorized,
                CodeBuddyGenerationReply::HttpUnauthorized,
            ],
        )
        .await;
        let state = codebuddy_test_state("codebuddy-second-auth");
        let (provider_id, _) = install_codebuddy_provider(
            &state,
            "codebuddy-second-auth",
            AppKind::Codex,
            CodeBuddySite::Intl,
            &format!("http://{address}"),
        )
        .await;
        let share_id = "codebuddy-second-auth-share";
        install_codebuddy_share(
            &state,
            share_id,
            AppKind::Codex,
            &provider_id,
            "owner@example.com",
        )
        .await;
        let error = forward_codebuddy_surface(state, AppKind::Codex, provider_id, share_id, false)
            .await
            .unwrap_err();
        assert_eq!(error.status, StatusCode::UNAUTHORIZED);
        assert_eq!(fixture.count(&fixture.generation_requests), 2);
        assert_eq!(fixture.count(&fixture.refresh_requests), 1);
        server.abort();
    });
}
