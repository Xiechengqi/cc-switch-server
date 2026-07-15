// Integration tests for the HTTP API contract.
// Extracted from src/api/mod.rs as part of R3.6/R3.7.

use std::collections::BTreeMap;
use std::convert::Infallible;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::body::{to_bytes, Body, Bytes};
use axum::extract::State;
use axum::http::{HeaderMap, Method, Request, StatusCode};
use axum::response::Response;
use axum::routing::{get, patch, post};
use axum::Router;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tower::ServiceExt;

use cc_switch_server::api::*;
use cc_switch_server::api::{
    control_signature, control_signature_for_method, refresh_share_usage_items,
};
use cc_switch_server::cli::Cli;
use cc_switch_server::domain::accounts::store::{AccountQuota, UpsertAccountInput};
use cc_switch_server::domain::failover::UpdateFailoverAppInput;
use cc_switch_server::domain::providers::model::{
    AppKind, AuthBinding, Provider, ProviderMeta, ProviderType,
};
use cc_switch_server::domain::sharing::shares::{ShareBinding, UpsertShareInput};
use cc_switch_server::domain::usage::store::{
    TokenUsage, UsageLog, UsageLogContext, UsageModelMetadata,
};
use cc_switch_server::state::{ServerState, ServerStateInner};

async fn upsert_test_provider(state: &ServerState, app: AppKind, provider: Provider) {
    state
        .mutate_providers(|providers| {
            providers.upsert(app, provider);
        })
        .await;
}

async fn providers_snapshot(
    state: &ServerState,
) -> cc_switch_server::domain::providers::store::ProviderStore {
    state.mutate_providers(|providers| providers.clone()).await
}

#[tokio::test]
async fn share_router_health_is_hidden_without_probe_header() {
    let state = test_state();
    configure_share_router_identity(&state).await;
    let app = app_router(state);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/_share-router/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/_share-router/health")
                .header("X-Share-Router-Probe", "1")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let response = app
        .oneshot(share_router_request(
            Method::GET,
            "/_share-router/health",
            &[],
            "nonce-health",
            Vec::new(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(body["ok"].as_bool(), Some(true));
    assert_eq!(body["status"].as_str(), Some("healthy"));
}

#[tokio::test]
async fn share_router_request_logs_are_scoped_to_tunnel_share_header() {
    let state = test_state();
    configure_share_router_identity(&state).await;
    for share_id in ["share-a", "share-b"] {
        let provider_id = format!("provider-{share_id}");
        state
            .mutate_shares_immediate(|store| {
                store
                    .upsert(test_share_input(
                        share_id,
                        &provider_id,
                        ProviderType::Codex,
                    ))
                    .unwrap()
            })
            .await
            .unwrap();

        let mut log = UsageLog::new(
            AppKind::Codex,
            provider_id,
            "Provider Logs".to_string(),
            ProviderType::Codex,
            200,
            10,
            UsageModelMetadata {
                model: Some("gpt-5.5".to_string()),
                ..Default::default()
            },
            TokenUsage::default(),
        );
        log.apply_context(UsageLogContext {
            request_id: Some(format!("req_{share_id}-new")),
            share_id: Some(share_id.to_string()),
            data_source: Some("direct".to_string()),
            ..Default::default()
        });
        log.created_at_ms = if share_id == "share-a" { 2_000 } else { 3_000 };
        state.push_usage_log(log).await.unwrap();
    }
    let mut older_share_a = state
        .usage_snapshot()
        .await
        .logs
        .into_iter()
        .find(|log| log.share_id.as_deref() == Some("share-a"))
        .unwrap();
    older_share_a.request_id = "req_share-a-old".to_string();
    older_share_a.created_at_ms = 1_000;
    state.push_usage_log(older_share_a).await.unwrap();
    let app = app_router(state);

    let missing_header = app
        .clone()
        .oneshot(share_router_request(
            Method::GET,
            "/_share-router/request-logs",
            &[],
            "nonce-logs-missing-share",
            Vec::new(),
        ))
        .await
        .unwrap();
    assert_eq!(missing_header.status(), StatusCode::NOT_FOUND);

    let duplicated_header = app
        .clone()
        .oneshot(share_router_request(
            Method::GET,
            "/_share-router/request-logs",
            &["share-b", "share-a"],
            "nonce-logs-duplicate-share",
            Vec::new(),
        ))
        .await
        .unwrap();
    assert_eq!(duplicated_header.status(), StatusCode::NOT_FOUND);

    let mismatched_query = app
        .clone()
        .oneshot(share_router_request(
            Method::GET,
            "/_share-router/request-logs?shareId=share-b",
            &["share-a"],
            "nonce-logs-mismatched-share",
            Vec::new(),
        ))
        .await
        .unwrap();
    assert_eq!(mismatched_query.status(), StatusCode::NOT_FOUND);

    let scoped = app
        .oneshot(share_router_request(
            Method::GET,
            "/_share-router/request-logs",
            &["share-a"],
            "nonce-logs-scoped",
            Vec::new(),
        ))
        .await
        .unwrap();
    assert_eq!(scoped.status(), StatusCode::OK);
    let scoped = json_body(scoped).await;
    assert_eq!(scoped["shareId"], "share-a");
    assert_eq!(scoped["logs"].as_array().map(Vec::len), Some(2));
    assert_eq!(scoped["logs"][0]["shareId"], "share-a");
    assert_eq!(scoped["logs"][0]["requestId"], "req_share-a-new");
    assert_eq!(scoped["logs"][1]["requestId"], "req_share-a-old");
}

#[tokio::test]
async fn share_router_runtime_reports_health_for_requested_share_binding() {
    let state = test_state();
    configure_share_router_identity(&state).await;
    for (provider_id, name) in [
        ("provider-other", "Other Provider"),
        ("provider-bound", "Bound Provider"),
    ] {
        upsert_test_provider(
            &state,
            AppKind::Codex,
            Provider {
                id: provider_id.to_string(),
                name: name.to_string(),
                settings_config: json!({
                    "env": {"OPENAI_API_KEY": "sk-test"},
                    "models": ["gpt-5.5"]
                }),
                category: None,
                meta: None,
                extra: Default::default(),
            },
        )
        .await;
    }
    state
        .mutate_shares_immediate(|store| {
            store
                .upsert(test_share_input(
                    "share-runtime-health",
                    "provider-bound",
                    ProviderType::Codex,
                ))
                .unwrap()
        })
        .await
        .unwrap();

    let mut bound_log = UsageLog::new(
        AppKind::Codex,
        "provider-bound".to_string(),
        "Bound Provider".to_string(),
        ProviderType::Codex,
        599,
        250,
        UsageModelMetadata {
            model: Some("gpt-5.5".to_string()),
            requested_model: Some("gpt-5.5".to_string()),
            ..Default::default()
        },
        TokenUsage::default(),
    );
    bound_log.apply_context(UsageLogContext {
        share_id: Some("share-runtime-health".to_string()),
        data_source: Some("cc-switch-scheduled".to_string()),
        is_health_check: true,
        is_streaming: true,
        stream_status: Some("failed".to_string()),
        ..Default::default()
    });
    bound_log.error_message = Some("upstream connection timed out".to_string());
    bound_log.created_at_ms = 1_783_917_271_880;
    state.push_usage_log(bound_log).await.unwrap();

    let app = app_router(state);
    let response = app
        .oneshot(share_router_request(
            Method::GET,
            "/_share-router/share-runtime?shareId=share-runtime-health",
            &[],
            "nonce-runtime-health",
            Vec::new(),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(body["shareId"], "share-runtime-health");
    let results = body["modelHealth"]["codex"].as_array().unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["providerId"], "provider-bound");
    assert_eq!(results[0]["providerName"], "Bound Provider");
    assert_eq!(results[0]["status"], "failed");
    assert_eq!(results[0]["checkedAt"], 1_783_917_271_i64);
    assert_eq!(results[0]["source"], "cc-switch-scheduled");
    assert_eq!(results[0]["errorMessage"], "upstream connection timed out");
}

#[tokio::test]
async fn share_router_model_health_stream_probe_persists_bound_provider_result() {
    let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let upstream_addr = listener.local_addr().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let upstream_calls = calls.clone();
    let upstream = Router::new().route(
        "/v1/responses",
        post(move || {
            let calls = upstream_calls.clone();
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                (
                    StatusCode::OK,
                    "event: response.completed\ndata: {\"type\":\"response.completed\"}\n\n",
                )
            }
        }),
    );
    tokio::spawn(async move {
        axum::serve(listener, upstream).await.unwrap();
    });

    let state = test_state();
    configure_share_router_identity(&state).await;
    upsert_test_provider(
        &state,
        AppKind::Codex,
        Provider {
            id: "provider-health-probe".to_string(),
            name: "Health Probe Provider".to_string(),
            settings_config: json!({
                "env": {
                    "OPENAI_BASE_URL": format!("http://{upstream_addr}"),
                    "OPENAI_API_KEY": "sk-local-secret"
                },
                "models": ["gpt-5.5"]
            }),
            category: None,
            meta: None,
            extra: Default::default(),
        },
    )
    .await;
    state
        .mutate_shares_immediate(|store| {
            store
                .upsert(test_share_input(
                    "share-health-probe",
                    "provider-health-probe",
                    ProviderType::Codex,
                ))
                .unwrap()
        })
        .await
        .unwrap();

    let app = app_router(state.clone());
    let body = serde_json::to_vec(&json!({"appType": "codex"})).unwrap();
    let response = app
        .clone()
        .oneshot(share_router_request(
            Method::POST,
            "/_share-router/model-health",
            &["share-health-probe"],
            "nonce-model-health-probe",
            body,
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let response = json_body(response).await;
    assert_eq!(response["success"], true);
    assert_eq!(response["status"], "healthy");
    assert_eq!(response["statusCode"], 200);
    assert_eq!(response["modelUsed"], "gpt-5.5");
    assert_eq!(response["providerId"], "provider-health-probe");
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    let usage = state.usage_snapshot().await;
    let log = usage
        .logs
        .iter()
        .find(|log| log.share_id.as_deref() == Some("share-health-probe"))
        .unwrap();
    assert!(log.is_health_check);
    assert!(log.is_streaming);
    assert_eq!(log.provider_id, "provider-health-probe");
    assert_eq!(log.data_source.as_deref(), Some("cc-switch-router-probe"));
    assert_eq!(log.stream_status.as_deref(), Some("completed"));
    assert_eq!(log.status_code, 200);
    assert!(log.error_message.is_none());

    let response = app
        .oneshot(share_router_request(
            Method::GET,
            "/_share-router/share-runtime?shareId=share-health-probe",
            &[],
            "nonce-runtime-after-health-probe",
            Vec::new(),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let response = json_body(response).await;
    assert_eq!(
        response["modelHealth"]["codex"][0]["providerId"],
        "provider-health-probe"
    );
    assert_eq!(response["modelHealth"]["codex"][0]["status"], "success");
}

#[tokio::test]
async fn control_apply_share_settings_rejects_replayed_nonce() {
    let state = test_state();
    let mut config = state.config_snapshot().await;
    config.router.identity = Some(cc_switch_server::domain::settings::config::RouterIdentity {
        installation_id: "inst-ctl".to_string(),
        public_key: "public-key".to_string(),
        private_key: "private-key".to_string(),
        control_secret: Some("control-secret".to_string()),
    });
    state.replace_config(config).await.unwrap();
    upsert_test_provider(
        &state,
        AppKind::Codex,
        Provider {
            id: "p-ctl".to_string(),
            name: "Control Provider".to_string(),
            settings_config: json!({"env": {"OPENAI_API_KEY": "sk-test"}}),
            category: None,
            meta: None,
            extra: Default::default(),
        },
    )
    .await;
    state
        .mutate_shares_immediate(|store| {
            let _ = store.upsert(test_share_input("share-ctl", "p-ctl", ProviderType::Codex));
        })
        .await
        .unwrap();
    let app = app_router(state);
    let body = serde_json::to_vec(&json!({
        "shareId": "share-ctl",
        "patch": {"description": "updated by control"}
    }))
    .unwrap();
    let timestamp_ms = now_ms() as i64;
    let signature = BASE64_STANDARD.encode(
        control_signature(
            APPLY_SHARE_SETTINGS_PATH,
            "control-secret",
            &body,
            timestamp_ms,
            "nonce-ctl",
        )
        .unwrap(),
    );

    let response = app
        .clone()
        .oneshot(control_request(
            APPLY_SHARE_SETTINGS_PATH,
            body.clone(),
            timestamp_ms,
            "nonce-ctl",
            &signature,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .oneshot(control_request(
            APPLY_SHARE_SETTINGS_PATH,
            body,
            timestamp_ms,
            "nonce-ctl",
            &signature,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let body = json_body(response).await;
    assert!(body["error"]
        .as_str()
        .unwrap()
        .contains("replay control request"));
}

#[tokio::test]
async fn control_refresh_share_usage_reports_bound_account_snapshot() {
    let state = test_state();
    upsert_test_provider(
        &state,
        AppKind::Codex,
        Provider {
            id: "p-refresh".to_string(),
            name: "Refresh Provider".to_string(),
            settings_config: json!({}),
            category: None,
            meta: Some(cc_switch_server::domain::providers::model::ProviderMeta {
                auth_binding: Some(cc_switch_server::domain::providers::model::AuthBinding {
                    source: Some("managed_account".to_string()),
                    auth_provider: Some("cursor_oauth".to_string()),
                    account_id: Some("acct-cursor".to_string()),
                }),
                provider_type: Some("cursor_oauth".to_string()),
                ..Default::default()
            }),
            extra: Default::default(),
        },
    )
    .await;
    state
        .mutate_accounts_immediate(|accounts| {
            accounts.upsert(UpsertAccountInput {
                id: Some("acct-cursor".to_string()),
                provider_type: ProviderType::CursorOAuth,
                email: Some("cursor@example.com".to_string()),
                access_token: None,
                refresh_token: None,
                id_token: None,
                token_type: None,
                api_key: None,
                extra_headers: None,
                scopes: Vec::new(),
                profile: None,
                raw: Some(json!({
                    "billingOrQuotaSnapshot": {
                        "stripeStatus": {"membershipType": "pro_plus"},
                        "currentPeriodUsage": {
                            "billingCycleEnd": 1774000000000i64,
                            "planUsage": {
                                "limit": 2000.0,
                                "used": 500.0,
                                "totalPercentUsed": 25.0
                            }
                        }
                    }
                })),
                subscription_level: None,
                entitlement_status: None,
                quota_percent: None,
                quota: None,
                quota_refreshed_at: None,
                quota_next_refresh_at: None,
                expires_at: None,
                rate_limited_until: None,
                last_refresh_error: None,
            });
        })
        .await
        .unwrap();
    let share = {
        let mut input = test_share_input("share-refresh", "p-refresh", ProviderType::CursorOAuth);
        input.bindings = vec![ShareBinding {
            app: AppKind::Codex,
            provider_id: "p-refresh".to_string(),
            provider_type: ProviderType::CursorOAuth,
        }];
        input
    };
    let share = state
        .mutate_shares_immediate(|store| store.upsert(share))
        .await
        .unwrap()
        .unwrap();
    let providers = providers_snapshot(&state).await;

    let refreshed = refresh_share_usage_items(&state, &share, Some("codex"), &providers).await;

    assert_eq!(refreshed.len(), 1);
    assert_eq!(refreshed[0].account_id.as_deref(), Some("acct-cursor"));
    assert!(refreshed[0].refreshed);
    assert!(refreshed[0].error.is_none());
    let account = state
        .find_account_for_provider(ProviderType::CursorOAuth, Some("acct-cursor"))
        .await
        .unwrap();
    assert_eq!(account.quota_percent, Some(25.0));
}

#[tokio::test]
async fn auth_routes_cover_password_api_token_and_email_paths() {
    let state = test_state();
    let app = app_router(state.clone());

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/auth/login",
            json!({"method": "password", "password": "password123"}),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    let response = app
        .clone()
        .oneshot(json_request(Method::GET, "/api/config", json!(null), None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/setup",
            json!({
                "password": "password123",
                "ownerEmail": "owner@example.com",
                "routerUrl": "http://127.0.0.1:9",
                "clientTunnelSubdomain": "ownerabcde"
            }),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/auth/login",
            json!({"method": "password", "password": "bad"}),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/auth/login",
            json!({"method": "password", "password": "password123"}),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let token = json_body(response).await["token"]
        .as_str()
        .unwrap()
        .to_string();

    let response = app
        .clone()
        .oneshot(json_request(
            Method::GET,
            "/api/auth/me",
            json!(null),
            Some(&token),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/auth/api-token",
            json!(null),
            Some(&token),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let api_token = json_body(response).await["apiToken"]
        .as_str()
        .unwrap()
        .to_string();

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/auth/login",
            json!({"method": "api_token", "apiToken": api_token}),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/auth/login",
            json!({"method": "api_token", "apiToken": "bad"}),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/auth/login",
            json!({"method": "email", "email": "owner@example.com"}),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/auth/login",
            json!({"method": "email", "email": "OWNER@example.com", "code": "123456"}),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
}

#[tokio::test]
async fn oauth_login_cancel_is_authenticated_idempotent_and_terminal() {
    let state = test_state();
    let app = app_router(state);
    let token = setup_and_login(&app).await;

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/accounts/login/start",
            json!({
                "providerType": "claude_oauth",
                "redirectUri": "http://localhost:15721/api/accounts/login/callback"
            }),
            Some(&token),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let login = json_body(response).await["login"].clone();
    let session_id = login["sessionId"].as_str().unwrap();
    let state = login["state"].as_str().unwrap();

    for _ in 0..2 {
        let response = app
            .clone()
            .oneshot(json_request(
                Method::POST,
                "/api/accounts/login/cancel",
                json!({"sessionId": session_id, "state": state}),
                Some(&token),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(json_body(response).await["login"]["status"], "cancelled");
    }

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/accounts/login/finish",
            json!({
                "sessionId": session_id,
                "state": state,
                "code": "unused-auth-code",
                "executeTokenExchange": true
            }),
            Some(&token),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/accounts/login/cancel",
            json!({"state": "unknown-oauth-state"}),
            Some(&token),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/web-api/invoke/auth_start_login",
            json!({"authProvider": "claude_oauth"}),
            Some(&token),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let device_code = json_body(response).await["device_code"]
        .as_str()
        .unwrap()
        .to_string();
    let response = app
        .oneshot(json_request(
            Method::POST,
            "/web-api/invoke/auth_cancel_login",
            json!({
                "authProvider": "claude_oauth",
                "deviceCode": device_code
            }),
            Some(&token),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(json_body(response).await["cancelled"], true);
}

#[tokio::test]
async fn share_market_grant_route_updates_snapshot_and_can_clear_status() {
    let state = test_state();
    let app = app_router(state.clone());
    let token = setup_and_login(&app).await;

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/shares",
            json!({
                "id": "share-grant",
                "app": "codex",
                "providerId": "p1",
                "providerType": "codex",
                "displayName": "Grant Test"
            }),
            Some(&token),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/shares/share-grant/market-grant",
            json!({
                "marketGrant": {
                    "status": "Applied",
                    "grantId": "grant-1",
                    "lastError": ""
                }
            }),
            Some(&token),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;

    assert_eq!(body["share"]["marketGrant"]["status"], "applied");
    assert_eq!(body["share"]["marketGrant"]["grantId"], "grant-1");
    assert!(body["share"]["marketGrant"]["lastError"].is_null());
    assert!(body["share"]["marketGrant"]["updatedAtMs"].is_u64());
    assert_eq!(
        body["share"]["runtimeSnapshot"]["marketGrant"]["status"],
        "applied"
    );

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/shares/share-grant/market-grant",
            json!({"marketGrant": {"status": "unknown"}}),
            Some(&token),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/shares/share-grant/market-grant",
            json!({"marketGrant": null}),
            Some(&token),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert!(body["share"]["marketGrant"].is_null());
    assert!(body["share"]["runtimeSnapshot"]["marketGrant"].is_null());
}

#[tokio::test]
async fn provider_network_test_reports_redacted_upstream_4xx_body() {
    let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let upstream_addr = listener.local_addr().unwrap();
    let upstream = Router::new().route(
        "/v1/responses",
        post(|| async {
            (
                StatusCode::UNAUTHORIZED,
                r#"{"error":"bad sk-abc1234567890 Bearer abc.def"}"#,
            )
        }),
    );
    tokio::spawn(async move {
        axum::serve(listener, upstream).await.unwrap();
    });

    let state = test_state();
    let app = app_router(state.clone());

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/setup",
            json!({
                "password": "password123",
                "ownerEmail": "owner@example.com",
                "routerUrl": "http://127.0.0.1:9",
                "clientTunnelSubdomain": "ownerabcde"
            }),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/auth/login",
            json!({"method": "password", "password": "password123"}),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let token = json_body(response).await["token"]
        .as_str()
        .unwrap()
        .to_string();

    upsert_test_provider(
        &state,
        AppKind::Codex,
        Provider {
            id: "codex-network-test".to_string(),
            name: "Codex Network Test".to_string(),
            settings_config: json!({
                "env": {
                    "OPENAI_BASE_URL": format!("http://{upstream_addr}"),
                    "OPENAI_API_KEY": "sk-local-secret"
                },
                "testConfig": {
                    "model": "test-model"
                }
            }),
            category: None,
            meta: None,
            extra: Default::default(),
        },
    )
    .await;

    let response = app
        .oneshot(json_request(
            Method::POST,
            "/api/providers/codex-network-test/test?network=true",
            json!(null),
            Some(&token),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;

    assert_eq!(body["networkChecked"].as_bool(), Some(true));
    assert_eq!(body["networkStatusCode"].as_u64(), Some(401));
    let error = body["networkError"].as_str().unwrap();
    assert!(error.contains("[REDACTED]"));
    assert!(!error.contains("sk-abc"));
    assert!(!error.contains("Bearer abc"));
}

#[tokio::test]
async fn provider_network_test_covers_4xx_5xx_and_empty_bodies() {
    let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let upstream_addr = listener.local_addr().unwrap();
    let upstream = Router::new()
        .route(
            "/v1/responses",
            post(|| async {
                (
                    StatusCode::FORBIDDEN,
                    r#"{"error":"forbidden sk-secret-1234567890"}"#,
                )
            }),
        )
        .route(
            "/v1/chat/completions",
            post(|| async { (StatusCode::INTERNAL_SERVER_ERROR, "") }),
        );
    tokio::spawn(async move {
        axum::serve(listener, upstream).await.unwrap();
    });

    let state = test_state();
    let app = app_router(state.clone());
    let token = setup_and_login(&app).await;

    upsert_test_provider(
        &state,
        AppKind::Codex,
        Provider {
            id: "codex-provider-test".to_string(),
            name: "Codex Provider Test".to_string(),
            settings_config: json!({
                "env": {
                    "OPENAI_BASE_URL": format!("http://{upstream_addr}"),
                    "OPENAI_API_KEY": "sk-local-secret"
                }
            }),
            category: None,
            meta: None,
            extra: Default::default(),
        },
    )
    .await;

    let response = app
        .oneshot(json_request(
            Method::POST,
            "/api/providers/codex-provider-test/test?network=true",
            json!(null),
            Some(&token),
        ))
        .await
        .unwrap();
    let body = json_body(response).await;

    assert_eq!(body["networkStatusCode"].as_u64(), Some(403));
    let error = body["networkError"].as_str().unwrap();
    assert!(error.contains("[REDACTED]"));
    assert!(!error.contains("sk-secret"));
}

#[tokio::test]
async fn provider_network_test_timeout_is_configurable_and_redacted() {
    let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let upstream_addr = listener.local_addr().unwrap();
    let upstream = Router::new().route(
        "/v1/responses",
        post(|| async {
            tokio::time::sleep(Duration::from_millis(200)).await;
            (StatusCode::OK, "{}")
        }),
    );
    tokio::spawn(async move {
        axum::serve(listener, upstream).await.unwrap();
    });

    let state = test_state();
    let app = app_router(state.clone());
    let token = setup_and_login(&app).await;

    upsert_test_provider(
        &state,
        AppKind::Codex,
        Provider {
            id: "codex-provider-timeout".to_string(),
            name: "Codex Provider Timeout".to_string(),
            settings_config: json!({
                "env": {
                    "OPENAI_BASE_URL": format!("http://{upstream_addr}"),
                    "OPENAI_API_KEY": "sk-local-secret"
                }
            }),
            category: None,
            meta: None,
            extra: Default::default(),
        },
    )
    .await;

    let response = app
        .oneshot(json_request(
            Method::POST,
            "/api/providers/codex-provider-timeout/test?network=true&timeoutMs=25",
            json!(null),
            Some(&token),
        ))
        .await
        .unwrap();
    let body = json_body(response).await;

    assert_eq!(body["networkChecked"].as_bool(), Some(true));
    assert_eq!(body["networkStatusCode"], serde_json::Value::Null);
    let error = body["networkError"].as_str().unwrap();
    assert!(!error.trim().is_empty());
    assert!(!error.contains("sk-local-secret"));
}

#[tokio::test]
async fn non_stream_proxy_preserves_upstream_error_status_body_and_usage() {
    let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let upstream_addr = listener.local_addr().unwrap();
    let upstream = Router::new().route(
        "/v1/responses",
        post(|| async {
            (
                StatusCode::TOO_MANY_REQUESTS,
                [(axum::http::header::CONTENT_TYPE, "text/plain")],
                "quota_exhausted",
            )
        }),
    );
    tokio::spawn(async move {
        axum::serve(listener, upstream).await.unwrap();
    });

    let state = test_state();
    let app = app_router(state.clone());
    upsert_test_provider(
        &state,
        AppKind::Codex,
        Provider {
            id: "codex-proxy-error".to_string(),
            name: "Codex Proxy Error".to_string(),
            settings_config: json!({
                "env": {
                    "OPENAI_BASE_URL": format!("http://{upstream_addr}"),
                    "OPENAI_API_KEY": "sk-local-secret"
                }
            }),
            category: None,
            meta: None,
            extra: Default::default(),
        },
    )
    .await;

    let response = app
        .oneshot(json_request(
            Method::POST,
            "/v1/responses",
            json!({"model":"gpt-5.5","input":"ping","stream":false}),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    let body = body_text(response).await;
    assert_eq!(body, "quota_exhausted");

    let usage = state.usage_snapshot().await;
    assert_eq!(usage.logs.len(), 1);
    assert_eq!(usage.logs[0].provider_id, "codex-proxy-error");
    assert_eq!(usage.logs[0].status_code, 429);
    assert!(!usage.logs[0].is_streaming);
}

#[tokio::test]
async fn copilot_managed_account_uses_cached_internal_token_and_endpoint() {
    let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let upstream_addr = listener.local_addr().unwrap();
    let seen = Arc::new(AtomicUsize::new(0));
    let upstream = Router::new()
        .route(
            "/chat/completions",
            post(
                |State(seen): State<Arc<AtomicUsize>>, headers: HeaderMap| async move {
                    assert_eq!(
                        headers.get("authorization").and_then(|v| v.to_str().ok()),
                        Some("Bearer cached-copilot-token")
                    );
                    seen.fetch_add(1, Ordering::SeqCst);
                    axum::Json(json!({
                        "id": "chatcmpl-copilot",
                        "object": "chat.completion",
                        "choices": [{
                            "index": 0,
                            "message": {
                                "role": "assistant",
                                "content": "ok"
                            },
                            "finish_reason": "stop"
                        }],
                        "usage": {
                            "prompt_tokens": 3,
                            "completion_tokens": 2,
                            "total_tokens": 5
                        }
                    }))
                },
            ),
        )
        .with_state(seen.clone());
    tokio::spawn(async move {
        axum::serve(listener, upstream).await.unwrap();
    });

    let state = test_state();
    let app = app_router(state.clone());
    upsert_test_provider(
        &state,
        AppKind::Codex,
        Provider {
            id: "copilot-managed".to_string(),
            name: "Copilot Managed".to_string(),
            settings_config: json!({}),
            category: None,
            meta: Some(ProviderMeta {
                provider_type: Some("github_copilot".to_string()),
                auth_binding: Some(AuthBinding {
                    source: Some("managed_account".to_string()),
                    auth_provider: Some("github_copilot".to_string()),
                    account_id: Some("acct-copilot".to_string()),
                }),
                ..Default::default()
            }),
            extra: Default::default(),
        },
    )
    .await;
    state
        .mutate_accounts_immediate(|accounts| {
            accounts.upsert(UpsertAccountInput {
                id: Some("acct-copilot".to_string()),
                provider_type: ProviderType::GitHubCopilot,
                email: Some("octo@example.com".to_string()),
                access_token: Some("cached-copilot-token".to_string()),
                refresh_token: Some("github-token".to_string()),
                id_token: None,
                token_type: Some("Bearer".to_string()),
                api_key: None,
                extra_headers: None,
                scopes: Vec::new(),
                profile: Some(json!({"githubDomain": "github.com", "ghes": false})),
                raw: Some(json!({
                    "githubDomain": "github.com",
                    "githubToken": "github-token",
                    "copilotUsage": {
                        "endpoints": {
                            "api": format!("http://{upstream_addr}")
                        }
                    }
                })),
                subscription_level: None,
                entitlement_status: None,
                quota: None,
                quota_percent: None,
                quota_refreshed_at: None,
                quota_next_refresh_at: None,
                expires_at: Some(4_102_444_800_000),
                rate_limited_until: None,
                last_refresh_error: None,
            });
        })
        .await
        .unwrap();

    let response = app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/v1/chat/completions")
                .header("x-cc-provider-id", "copilot-managed")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "model": "gpt-5",
                        "messages": [{"role": "user", "content": "hello"}]
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    let status = response.status();
    let body = json_body(response).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body["choices"][0]["message"]["content"].as_str(),
        Some("ok")
    );
    assert_eq!(seen.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn claude_kiro_managed_account_bridges_non_stream_response() {
    let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let upstream_addr = listener.local_addr().unwrap();
    let seen = Arc::new(AtomicUsize::new(0));
    let upstream = Router::new()
        .route(
            "/generateAssistantResponse",
            post(
                |State(seen): State<Arc<AtomicUsize>>,
                 headers: HeaderMap,
                 axum::Json(body): axum::Json<Value>| async move {
                    assert_eq!(
                        headers.get("authorization").and_then(|v| v.to_str().ok()),
                        Some("Bearer kiro-access-token")
                    );
                    assert_eq!(
                        headers
                            .get("x-amzn-kiro-agent-mode")
                            .and_then(|v| v.to_str().ok()),
                        Some("vibe")
                    );
                    assert_eq!(
                        body.pointer("/profileArn").and_then(Value::as_str),
                        Some("arn:aws:codewhisperer:us-east-1:123456789012:profile/profile-id")
                    );
                    assert_eq!(
                        body.pointer("/conversationState/currentMessage/userInputMessage/modelId")
                            .and_then(Value::as_str),
                        Some("claude-sonnet-4.8")
                    );
                    let request_index = seen.fetch_add(1, Ordering::SeqCst);
                    let events = if request_index == 0 {
                        vec![(
                            "assistantResponseEvent",
                            json!({"content": "hello from kiro"}),
                        )]
                    } else {
                        vec![(
                            "toolUseEvent",
                            json!({
                                "toolUseId": "toolu_incomplete",
                                "name": "Read",
                                "input": "{\"file_path\":",
                                "stop": false
                            }),
                        )]
                    };
                    Response::builder()
                        .status(StatusCode::OK)
                        .header("content-type", "application/vnd.amazon.eventstream")
                        .body(Body::from(event_stream_bytes(events)))
                        .unwrap()
                },
            ),
        )
        .with_state(seen.clone());
    tokio::spawn(async move {
        axum::serve(listener, upstream).await.unwrap();
    });

    let state = test_state();
    let app = app_router(state.clone());
    upsert_test_provider(
        &state,
        AppKind::Claude,
        Provider {
            id: "kiro-managed".to_string(),
            name: "Kiro Managed".to_string(),
            settings_config: json!({
                "env": {
                    "KIRO_API_BASE_URL": format!("http://{upstream_addr}")
                }
            }),
            category: None,
            meta: Some(ProviderMeta {
                provider_type: Some("kiro_oauth".to_string()),
                auth_binding: Some(AuthBinding {
                    source: Some("managed_account".to_string()),
                    auth_provider: Some("kiro_oauth".to_string()),
                    account_id: Some("acct-kiro".to_string()),
                }),
                ..Default::default()
            }),
            extra: Default::default(),
        },
    )
    .await;
    state
        .mutate_accounts_immediate(|accounts| {
            accounts.upsert(UpsertAccountInput {
                id: Some("acct-kiro".to_string()),
                provider_type: ProviderType::KiroOAuth,
                email: Some("kiro@example.com".to_string()),
                access_token: Some("kiro-access-token".to_string()),
                refresh_token: Some("kiro-refresh-token".to_string()),
                id_token: None,
                token_type: Some("Bearer".to_string()),
                api_key: None,
                extra_headers: None,
                scopes: Vec::new(),
                profile: Some(json!({
                    "profileArn": "arn:aws:codewhisperer:us-east-1:123456789012:profile/profile-id",
                    "authRegion": "us-east-1",
                    "apiRegion": "us-east-1",
                    "machineId": "machine-test",
                    "authMethod": "builder-id",
                    "provider": "BuilderId"
                })),
                raw: Some(json!({
                    "clientId": "client-id",
                    "clientSecret": "client-secret",
                    "clientSecretExpiresAt": 4_102_444_800_000_i64,
                    "importedAtMs": 1000
                })),
                subscription_level: Some("Kiro Pro".to_string()),
                entitlement_status: None,
                quota: None,
                quota_percent: None,
                quota_refreshed_at: None,
                quota_next_refresh_at: None,
                expires_at: Some(4_102_444_800_000),
                rate_limited_until: None,
                last_refresh_error: None,
            });
        })
        .await
        .unwrap();

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/v1/messages")
                .header("x-cc-provider-id", "kiro-managed")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "model": "claude-sonnet-4-8",
                        "max_tokens": 64,
                        "stream": false,
                        "messages": [{"role": "user", "content": "hello"}]
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    let status = response.status();
    let body = json_body(response).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["type"].as_str(), Some("message"));
    assert_eq!(body["content"][0]["text"].as_str(), Some("hello from kiro"));
    assert_eq!(body["stop_reason"].as_str(), Some("end_turn"));
    assert_eq!(seen.load(Ordering::SeqCst), 1);

    let response = app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/v1/messages")
                .header("x-cc-provider-id", "kiro-managed")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "model": "claude-sonnet-4-8",
                        "max_tokens": 64,
                        "stream": false,
                        "messages": [{"role": "user", "content": "incomplete tool"}]
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = json_body(response).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY, "{body}");
    assert_eq!(body["code"].as_str(), Some("TOOL_JSON_INCOMPLETE"));
    assert_eq!(body["type"].as_str(), Some("upstream_tool_json_error"));
    assert_eq!(body["retryable"].as_bool(), Some(false));
    assert_eq!(seen.load(Ordering::SeqCst), 2);

    let usage = state.usage_snapshot().await;
    let status_codes = usage
        .logs
        .iter()
        .filter(|log| log.provider_id == "kiro-managed")
        .map(|log| log.status_code)
        .collect::<Vec<_>>();
    assert_eq!(
        status_codes,
        vec![StatusCode::OK.as_u16(), StatusCode::BAD_GATEWAY.as_u16()]
    );
}

#[tokio::test]
async fn claude_kiro_managed_account_bridges_stream_response() {
    let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let upstream_addr = listener.local_addr().unwrap();
    let seen = Arc::new(AtomicUsize::new(0));
    let upstream = Router::new()
        .route(
            "/generateAssistantResponse",
            post(
                |State(seen): State<Arc<AtomicUsize>>, headers: HeaderMap| async move {
                    assert_eq!(
                        headers.get("authorization").and_then(|v| v.to_str().ok()),
                        Some("Bearer kiro-access-token")
                    );
                    seen.fetch_add(1, Ordering::SeqCst);
                    Response::builder()
                        .status(StatusCode::OK)
                        .header("content-type", "application/vnd.amazon.eventstream")
                        .body(Body::from(event_stream_bytes(vec![(
                            "assistantResponseEvent",
                            json!({"content": "hello streaming kiro"}),
                        )])))
                        .unwrap()
                },
            ),
        )
        .with_state(seen.clone());
    tokio::spawn(async move {
        axum::serve(listener, upstream).await.unwrap();
    });

    let state = test_state();
    let app = app_router(state.clone());
    upsert_test_provider(
        &state,
        AppKind::Claude,
        Provider {
            id: "kiro-managed-stream".to_string(),
            name: "Kiro Managed Stream".to_string(),
            settings_config: json!({
                "env": {
                    "KIRO_API_BASE_URL": format!("http://{upstream_addr}")
                }
            }),
            category: None,
            meta: Some(ProviderMeta {
                provider_type: Some("kiro_oauth".to_string()),
                auth_binding: Some(AuthBinding {
                    source: Some("managed_account".to_string()),
                    auth_provider: Some("kiro_oauth".to_string()),
                    account_id: Some("acct-kiro-stream".to_string()),
                }),
                ..Default::default()
            }),
            extra: Default::default(),
        },
    )
    .await;
    state
        .mutate_accounts_immediate(|accounts| {
            accounts.upsert(UpsertAccountInput {
                id: Some("acct-kiro-stream".to_string()),
                provider_type: ProviderType::KiroOAuth,
                email: Some("kiro@example.com".to_string()),
                access_token: Some("kiro-access-token".to_string()),
                refresh_token: Some("kiro-refresh-token".to_string()),
                id_token: None,
                token_type: Some("Bearer".to_string()),
                api_key: None,
                extra_headers: None,
                scopes: Vec::new(),
                profile: Some(json!({
                    "profileArn": "arn:aws:codewhisperer:us-east-1:123456789012:profile/profile-id",
                    "authRegion": "us-east-1",
                    "apiRegion": "us-east-1",
                    "machineId": "machine-test",
                    "authMethod": "builder-id",
                    "provider": "BuilderId"
                })),
                raw: Some(json!({
                    "clientId": "client-id",
                    "clientSecret": "client-secret",
                    "clientSecretExpiresAt": 4_102_444_800_000_i64,
                    "importedAtMs": 1000
                })),
                subscription_level: Some("Kiro Pro".to_string()),
                entitlement_status: None,
                quota: None,
                quota_percent: None,
                quota_refreshed_at: None,
                quota_next_refresh_at: None,
                expires_at: Some(4_102_444_800_000),
                rate_limited_until: None,
                last_refresh_error: None,
            });
        })
        .await
        .unwrap();

    let response = app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/v1/messages")
                .header("x-cc-provider-id", "kiro-managed-stream")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "model": "claude-sonnet-4-8",
                        "max_tokens": 64,
                        "stream": true,
                        "messages": [{"role": "user", "content": "hello"}]
                    }))
                    .unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains("event: message_start"));
    assert!(text.contains("event: content_block_delta"));
    assert!(text.contains("hello streaming kiro"));
    assert!(text.contains("event: message_stop"));
    assert_eq!(seen.load(Ordering::SeqCst), 1);

    let usage = state.usage_snapshot().await;
    let log = usage
        .logs
        .iter()
        .find(|log| log.provider_id == "kiro-managed-stream")
        .unwrap();
    assert!(log.is_streaming);
    assert_eq!(log.stream_status.as_deref(), Some("completed"));
}

#[tokio::test]
async fn non_stream_proxy_timeout_records_bad_gateway() {
    let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let upstream_addr = listener.local_addr().unwrap();
    let upstream = Router::new().route(
        "/v1/responses",
        post(|| async {
            tokio::time::sleep(Duration::from_millis(200)).await;
            (StatusCode::OK, "{}")
        }),
    );
    tokio::spawn(async move {
        axum::serve(listener, upstream).await.unwrap();
    });

    let state = test_state();
    let app = app_router(state.clone());
    upsert_test_provider(
        &state,
        AppKind::Codex,
        Provider {
            id: "codex-proxy-timeout".to_string(),
            name: "Codex Proxy Timeout".to_string(),
            settings_config: json!({
                "env": {
                    "OPENAI_BASE_URL": format!("http://{upstream_addr}"),
                    "OPENAI_API_KEY": "sk-local-secret",
                    "UPSTREAM_TIMEOUT_MS": "25"
                }
            }),
            category: None,
            meta: None,
            extra: Default::default(),
        },
    )
    .await;

    let response = app
        .oneshot(json_request(
            Method::POST,
            "/v1/responses",
            json!({"model":"gpt-5.5","input":"ping","stream":false}),
            None,
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    let text = body_text(response).await;
    assert!(text.contains("proxy upstream request failed"));
}

#[tokio::test]
async fn claude_transport_failure_retries_unpinned_provider_before_response_commit() {
    let closed_listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let closed_addr = closed_listener.local_addr().unwrap();
    drop(closed_listener);

    let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let upstream_addr = listener.local_addr().unwrap();
    let upstream = Router::new().route(
        "/v1/messages",
        post(|| async {
            (
                StatusCode::OK,
                [
                    ("content-type", "application/json"),
                    ("x-request-id", "req-failover"),
                    ("anthropic-ratelimit-requests-remaining", "42"),
                    ("set-cookie", "must-not-pass=1"),
                ],
                json!({
                    "id": "msg-ok",
                    "type": "message",
                    "role": "assistant",
                    "model": "claude-sonnet-4",
                    "content": [{"type": "text", "text": "ok"}],
                    "stop_reason": "end_turn",
                    "usage": {"input_tokens": 2, "output_tokens": 1}
                })
                .to_string(),
            )
        }),
    );
    tokio::spawn(async move {
        axum::serve(listener, upstream).await.unwrap();
    });

    let state = test_state();
    for (id, base_url) in [
        ("claude-dead", format!("http://{closed_addr}")),
        ("claude-live", format!("http://{upstream_addr}")),
    ] {
        upsert_test_provider(
            &state,
            AppKind::Claude,
            Provider {
                id: id.to_string(),
                name: id.to_string(),
                settings_config: json!({
                    "env": {
                        "ANTHROPIC_BASE_URL": base_url,
                        "ANTHROPIC_API_KEY": "sk-local-secret"
                    }
                }),
                category: None,
                meta: None,
                extra: Default::default(),
            },
        )
        .await;
    }
    let providers = providers_snapshot(&state).await;
    state
        .mutate_failover(|failover| {
            failover.update_app_config(
                AppKind::Claude,
                UpdateFailoverAppInput {
                    enabled: Some(true),
                    provider_queue: Some(vec![
                        "claude-dead".to_string(),
                        "claude-live".to_string(),
                    ]),
                    failure_threshold: Some(2),
                    open_duration_ms: Some(60_000),
                    half_open_max_probes: Some(1),
                },
                &providers,
            );
        })
        .await;

    let response = app_router(state.clone())
        .oneshot(json_request(
            Method::POST,
            "/v1/messages",
            json!({
                "model": "claude-sonnet-4",
                "max_tokens": 16,
                "messages": [{"role": "user", "content": "ping"}],
                "stream": false
            }),
            None,
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get("x-request-id").unwrap(),
        "req-failover"
    );
    assert_eq!(
        response
            .headers()
            .get("anthropic-ratelimit-requests-remaining")
            .unwrap(),
        "42"
    );
    assert!(!response.headers().contains_key("set-cookie"));
    let body = json_body(response).await;
    assert_eq!(body["id"], "msg-ok");

    let usage = state.usage_snapshot().await;
    assert_eq!(usage.logs.len(), 1);
    assert_eq!(usage.logs[0].provider_id, "claude-live");
    assert_eq!(usage.logs[0].input_tokens, Some(2));
}

#[tokio::test]
async fn claude_rate_limit_body_read_failure_retries_before_response_commit() {
    let broken_addr =
        spawn_broken_chunked_status_upstream("429 Too Many Requests", "application/json").await;
    let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let live_addr = listener.local_addr().unwrap();
    let upstream = Router::new().route(
        "/v1/messages",
        post(|| async {
            (
                StatusCode::OK,
                [("content-type", "application/json")],
                json!({
                    "id": "msg-after-429-read-error",
                    "type": "message",
                    "role": "assistant",
                    "model": "claude-sonnet-4",
                    "content": [{"type": "text", "text": "ok"}],
                    "stop_reason": "end_turn",
                    "usage": {"input_tokens": 2, "output_tokens": 1}
                })
                .to_string(),
            )
        }),
    );
    tokio::spawn(async move {
        axum::serve(listener, upstream).await.unwrap();
    });

    let state = test_state();
    for (id, base_url) in [
        ("claude-broken-429", format!("http://{broken_addr}")),
        ("claude-live-after-429", format!("http://{live_addr}")),
    ] {
        upsert_test_provider(
            &state,
            AppKind::Claude,
            Provider {
                id: id.to_string(),
                name: id.to_string(),
                settings_config: json!({
                    "env": {
                        "ANTHROPIC_BASE_URL": base_url,
                        "ANTHROPIC_API_KEY": "sk-local-secret"
                    }
                }),
                category: None,
                meta: None,
                extra: Default::default(),
            },
        )
        .await;
    }
    let providers = providers_snapshot(&state).await;
    state
        .mutate_failover(|failover| {
            failover.update_app_config(
                AppKind::Claude,
                UpdateFailoverAppInput {
                    enabled: Some(true),
                    provider_queue: Some(vec![
                        "claude-broken-429".to_string(),
                        "claude-live-after-429".to_string(),
                    ]),
                    failure_threshold: Some(2),
                    open_duration_ms: Some(60_000),
                    half_open_max_probes: Some(1),
                },
                &providers,
            );
        })
        .await;

    let response = app_router(state.clone())
        .oneshot(json_request(
            Method::POST,
            "/v1/messages",
            json!({
                "model": "claude-sonnet-4",
                "max_tokens": 16,
                "messages": [{"role": "user", "content": "ping"}],
                "stream": false
            }),
            None,
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(json_body(response).await["id"], "msg-after-429-read-error");
    let usage = state.usage_snapshot().await;
    assert_eq!(usage.logs.len(), 1);
    assert_eq!(usage.logs[0].provider_id, "claude-live-after-429");
}

#[tokio::test]
async fn claude_split_first_error_event_retries_before_stream_commit() {
    let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let upstream_addr = listener.local_addr().unwrap();
    let seen = Arc::new(AtomicUsize::new(0));
    let upstream = Router::new()
        .route(
            "/v1/messages",
            post(|State(seen): State<Arc<AtomicUsize>>| async move {
                let attempt = seen.fetch_add(1, Ordering::SeqCst);
                let chunks: Vec<Result<Bytes, Infallible>> = if attempt == 0 {
                    vec![
                        Ok(Bytes::from_static(
                            b"event: error\ndata: {\"type\":\"overloaded_",
                        )),
                        Ok(Bytes::from_static(
                            b"error\",\"message\":\"retry me\"}\n\n",
                        )),
                    ]
                } else {
                    vec![
                        Ok(Bytes::from_static(b"event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg-ok\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"claude-sonnet-4\",\"content\":[],\"usage\":{\"input_tokens\":2,\"output_tokens\":0}}}\n\n")),
                        Ok(Bytes::from_static(b"event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"success\"}}\n\nevent: message_stop\ndata: {\"type\":\"message_stop\"}\n\n")),
                    ]
                };
                Response::builder()
                    .status(StatusCode::OK)
                    .header("content-type", "text/event-stream")
                    .body(Body::from_stream(futures_util::stream::iter(chunks)))
                    .unwrap()
            }),
        )
        .with_state(seen.clone());
    tokio::spawn(async move {
        axum::serve(listener, upstream).await.unwrap();
    });

    let state = test_state();
    upsert_test_provider(
        &state,
        AppKind::Claude,
        Provider {
            id: "claude-sse-retry".to_string(),
            name: "Claude SSE Retry".to_string(),
            settings_config: json!({
                "env": {
                    "ANTHROPIC_BASE_URL": format!("http://{upstream_addr}"),
                    "ANTHROPIC_API_KEY": "sk-local-secret"
                }
            }),
            category: None,
            meta: None,
            extra: Default::default(),
        },
    )
    .await;

    let response = app_router(state.clone())
        .oneshot(json_request(
            Method::POST,
            "/v1/messages",
            json!({
                "model": "claude-sonnet-4",
                "max_tokens": 16,
                "messages": [{"role": "user", "content": "ping"}],
                "stream": true
            }),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_text(response).await;
    assert!(body.contains("success"));
    assert!(!body.contains("retry me"));
    assert_eq!(seen.load(Ordering::SeqCst), 2);

    let usage = state.usage_snapshot().await;
    assert_eq!(usage.logs.len(), 1);
    assert_eq!(usage.logs[0].provider_id, "claude-sse-retry");
}

#[tokio::test]
async fn native_claude_signature_error_does_not_run_oauth_body_retry() {
    let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let upstream_addr = listener.local_addr().unwrap();
    let seen = Arc::new(AtomicUsize::new(0));
    let upstream = Router::new()
        .route(
            "/v1/messages",
            post(|State(seen): State<Arc<AtomicUsize>>| async move {
                seen.fetch_add(1, Ordering::SeqCst);
                Response::builder()
                    .status(StatusCode::OK)
                    .header("content-type", "text/event-stream")
                    .body(Body::from(
                        "event: error\ndata: {\"type\":\"invalid_request_error\",\"message\":\"invalid thinking signature\"}\n\n",
                    ))
                    .unwrap()
            }),
        )
        .with_state(seen.clone());
    tokio::spawn(async move {
        axum::serve(listener, upstream).await.unwrap();
    });

    let state = test_state();
    upsert_test_provider(
        &state,
        AppKind::Claude,
        Provider {
            id: "native-claude-signature-error".to_string(),
            name: "Native Claude Signature Error".to_string(),
            settings_config: json!({
                "env": {
                    "ANTHROPIC_BASE_URL": format!("http://{upstream_addr}"),
                    "ANTHROPIC_API_KEY": "sk-local-secret"
                }
            }),
            category: None,
            meta: None,
            extra: Default::default(),
        },
    )
    .await;

    let response = app_router(state)
        .oneshot(json_request(
            Method::POST,
            "/v1/messages",
            json!({
                "model": "claude-sonnet-4",
                "max_tokens": 16,
                "messages": [{"role": "user", "content": "ping"}],
                "stream": true
            }),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(body_text(response)
        .await
        .contains("invalid thinking signature"));
    assert_eq!(seen.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn stream_proxy_marks_upstream_chunk_error() {
    let upstream_addr = spawn_broken_chunked_upstream().await;
    let state = test_state();
    let app = app_router(state.clone());
    upsert_test_provider(
        &state,
        AppKind::Codex,
        Provider {
            id: "codex-stream-error".to_string(),
            name: "Codex Stream Error".to_string(),
            settings_config: json!({
                "env": {
                    "OPENAI_BASE_URL": format!("http://{upstream_addr}"),
                    "OPENAI_API_KEY": "sk-local-secret"
                }
            }),
            category: None,
            meta: None,
            extra: Default::default(),
        },
    )
    .await;

    let response = app
        .oneshot(json_request(
            Method::POST,
            "/v1/responses",
            json!({"model":"gpt-5.5","input":"ping","stream":true}),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    let body_text = String::from_utf8_lossy(&body);
    assert!(body_text.contains("response.failed"));
    assert!(body_text.contains("cc_switch_stream_error"));

    for _ in 0..20 {
        let usage = state.usage_snapshot().await;
        if usage
            .logs
            .iter()
            .any(|log| log.stream_status.as_deref() == Some("upstream_error"))
        {
            let log = usage
                .logs
                .iter()
                .find(|log| log.provider_id == "codex-stream-error")
                .unwrap();
            assert_eq!(log.status_code, 502);
            assert!(log.is_streaming);
            assert!(log.first_token_ms.is_some());
            return;
        }
        drop(usage);
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    panic!("stream upstream_error usage log was not recorded");
}

#[test]
fn codex_oauth_schema_fixture_preserves_future_native_fields() {
    let mut store = cc_switch_server::domain::accounts::store::AccountStore::default();
    let account = store.upsert(UpsertAccountInput {
        id: Some("acct-codex".to_string()),
        provider_type: ProviderType::CodexOAuth,
        email: Some("owner@example.com".to_string()),
        access_token: Some("access-token".to_string()),
        refresh_token: Some("refresh-token".to_string()),
        id_token: None,
        token_type: Some("Bearer".to_string()),
        api_key: None,
        extra_headers: None,
        scopes: vec!["openid".to_string(), "profile".to_string()],
        profile: Some(json!({"plan":"pro"})),
        raw: Some(json!({"source":"mock"})),
        subscription_level: Some("pro".to_string()),
        entitlement_status: None,
        quota_percent: Some(12.5),
        quota: Some(AccountQuota {
            success: true,
            credential_message: Some("ok".to_string()),
            tiers: vec![
                cc_switch_server::domain::accounts::store::AccountQuotaTier {
                    name: "codex".to_string(),
                    utilization: Some(0.125),
                    used: Some(125.0),
                    limit: Some(1000.0),
                    unit: Some("requests".to_string()),
                    resets_at: Some(123456),
                },
            ],
            extra_usage: None,
        }),
        quota_refreshed_at: Some(1000),
        quota_next_refresh_at: Some(2000),
        expires_at: Some(3000),
        rate_limited_until: None,
        last_refresh_error: None,
    });

    assert_eq!(account.provider_type, ProviderType::CodexOAuth);
    assert_eq!(account.refresh_token.as_deref(), Some("refresh-token"));
    assert_eq!(account.subscription_level.as_deref(), Some("pro"));
    assert_eq!(account.quota_percent, Some(12.5));
    assert_eq!(account.quota.unwrap().tiers[0].name, "codex");
}

#[test]
fn mock_codex_refresh_lock_allows_one_refresh_per_account() {
    #[derive(Default)]
    struct RefreshLocks(std::sync::Mutex<std::collections::HashSet<String>>);

    impl RefreshLocks {
        fn try_lock(&self, account_id: &str) -> bool {
            self.0.lock().unwrap().insert(account_id.to_string())
        }

        fn unlock(&self, account_id: &str) {
            self.0.lock().unwrap().remove(account_id);
        }
    }

    let locks = RefreshLocks::default();
    assert!(locks.try_lock("acct-codex"));
    assert!(!locks.try_lock("acct-codex"));
    assert!(locks.try_lock("acct-other"));
    locks.unlock("acct-codex");
    assert!(locks.try_lock("acct-codex"));

    let capability =
        cc_switch_server::domain::accounts::managers::capability_for(ProviderType::CodexOAuth);
    assert_eq!(
        capability.support,
        cc_switch_server::domain::accounts::managers::AccountManagerSupport::ManualTokenStore
    );
    assert!(capability.supports_refresh);
}

#[tokio::test]
async fn router_heartbeat_probes_router_before_marking_online() {
    let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let router_addr = listener.local_addr().unwrap();
    let seen = Arc::new(AtomicUsize::new(0));
    let router = Router::new()
        .route(
            "/v1/shares/pending-edits",
            post(
                |State(seen): State<Arc<AtomicUsize>>, axum::Json(body): axum::Json<Value>| async move {
                    assert_eq!(body["installationId"].as_str(), Some("inst-heartbeat"));
                    assert_eq!(body["shareIds"].as_array().map(Vec::len), Some(0));
                    seen.fetch_add(1, Ordering::SeqCst);
                    axum::Json(json!({"edits": []}))
                },
            ),
        )
        .with_state(seen.clone());
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });

    let state = test_state();
    let app = app_router(state.clone());
    let token = setup_and_login(&app).await;
    let mut config = state.config_snapshot().await;
    config.router.url = Some(format!("http://{router_addr}"));
    config.router.identity = Some(cc_switch_server::domain::settings::config::RouterIdentity {
        installation_id: "inst-heartbeat".to_string(),
        public_key: BASE64_STANDARD.encode([8_u8; 32]),
        private_key: BASE64_STANDARD.encode([7_u8; 32]),
        control_secret: Some("control-secret".to_string()),
    });
    state.replace_config(config).await.unwrap();

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/router/heartbeat",
            json!(null),
            Some(&token),
        ))
        .await
        .unwrap();

    let status = response.status();
    let body = json_body(response).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["registered"].as_bool(), Some(true));
    assert!(body["lastError"].is_null());
    assert!(body["lastHeartbeatMs"].as_u64().is_some());
    assert_eq!(seen.load(Ordering::SeqCst), 1);
    assert!(state
        .config_snapshot()
        .await
        .client
        .last_heartbeat_ms
        .is_some());
}

#[tokio::test]
async fn router_heartbeat_records_probe_failure_without_marking_online() {
    let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let router_addr = listener.local_addr().unwrap();
    let router = Router::new().route(
        "/v1/shares/pending-edits",
        post(|| async { (StatusCode::SERVICE_UNAVAILABLE, "router down") }),
    );
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });

    let state = test_state();
    let app = app_router(state.clone());
    let token = setup_and_login(&app).await;
    let mut config = state.config_snapshot().await;
    config.router.url = Some(format!("http://{router_addr}"));
    config.router.identity = Some(cc_switch_server::domain::settings::config::RouterIdentity {
        installation_id: "inst-heartbeat".to_string(),
        public_key: BASE64_STANDARD.encode([8_u8; 32]),
        private_key: BASE64_STANDARD.encode([7_u8; 32]),
        control_secret: Some("control-secret".to_string()),
    });
    state.replace_config(config).await.unwrap();
    state
        .mutate_shares_immediate(|shares| {
            shares.router_registered = true;
            shares.last_router_error = None;
        })
        .await
        .unwrap();

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/router/heartbeat",
            json!(null),
            Some(&token),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    let body = json_body(response).await;
    assert!(body["error"]
        .as_str()
        .unwrap()
        .contains("router heartbeat probe failed"));

    let status = app
        .oneshot(json_request(
            Method::GET,
            "/api/router/status",
            json!(null),
            Some(&token),
        ))
        .await
        .unwrap();
    assert_eq!(status.status(), StatusCode::OK);
    let body = json_body(status).await;
    assert_eq!(body["registered"].as_bool(), Some(false));
    assert!(body["lastError"]
        .as_str()
        .unwrap()
        .contains("router pending share edits failed"));
    assert!(body["lastHeartbeatMs"].is_null());
}

#[tokio::test]
async fn client_tunnel_status_queries_remote_tunnel_when_registered() {
    let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let router_addr = listener.local_addr().unwrap();
    let seen = Arc::new(AtomicUsize::new(0));
    let router =
        Router::new()
            .route(
                "/v1/installations/client-tunnel",
                get(
                    |State(seen): State<Arc<AtomicUsize>>,
                     axum::extract::Query(query): axum::extract::Query<
                        BTreeMap<String, String>,
                    >| async move {
                        assert_eq!(
                            query.get("installationId").map(String::as_str),
                            Some("inst-tunnel")
                        );
                        assert!(query.contains_key("timestampMs"));
                        assert!(query.contains_key("nonce"));
                        assert!(query.contains_key("signature"));
                        seen.fetch_add(1, Ordering::SeqCst);
                        axum::Json(json!({
                            "tunnel": {
                                "ownerEmail": "owner@example.com",
                                "subdomain": "ownerabcde",
                                "enabled": true,
                                "tunnelUrl": "https://ownerabcde.example.test"
                            }
                        }))
                    },
                ),
            )
            .with_state(seen.clone());
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });

    let state = test_state();
    let app = app_router(state.clone());
    let token = setup_and_login(&app).await;
    let mut config = state.config_snapshot().await;
    config.router.url = Some(format!("http://{router_addr}"));
    config.router.identity = Some(cc_switch_server::domain::settings::config::RouterIdentity {
        installation_id: "inst-tunnel".to_string(),
        public_key: BASE64_STANDARD.encode([8_u8; 32]),
        private_key: BASE64_STANDARD.encode([7_u8; 32]),
        control_secret: Some("control-secret".to_string()),
    });
    state.replace_config(config).await.unwrap();

    let response = app
        .clone()
        .oneshot(json_request(
            Method::GET,
            "/api/router/client-tunnel",
            json!(null),
            Some(&token),
        ))
        .await
        .unwrap();

    let status = response.status();
    let body = json_body(response).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["remoteTunnel"]["enabled"].as_bool(), Some(true));
    assert_eq!(
        body["remoteTunnel"]["tunnelUrl"].as_str(),
        Some("https://ownerabcde.example.test")
    );
    assert_eq!(seen.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn stop_client_tunnel_releases_remote_tunnel_without_blocking_local_stop() {
    let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let router_addr = listener.local_addr().unwrap();
    let seen = Arc::new(AtomicUsize::new(0));
    let router = Router::new()
        .route(
            "/v1/installations/client-tunnel",
            patch(
                |State(seen): State<Arc<AtomicUsize>>, axum::Json(body): axum::Json<Value>| async move {
                    assert_eq!(body["installationId"].as_str(), Some("inst-tunnel"));
                    assert_eq!(body["tunnel"]["ownerEmail"].as_str(), Some("owner@example.com"));
                    assert_eq!(body["tunnel"]["subdomain"].as_str(), Some("ownerabcde"));
                    assert_eq!(body["tunnel"]["enabled"].as_bool(), Some(false));
                    seen.fetch_add(1, Ordering::SeqCst);
                    axum::Json(json!({
                        "tunnel": {
                            "ownerEmail": "owner@example.com",
                            "subdomain": "ownerabcde",
                            "enabled": false
                        }
                    }))
                },
            ),
        )
        .with_state(seen.clone());
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });

    let state = test_state();
    let app = app_router(state.clone());
    let token = setup_and_login(&app).await;
    let mut config = state.config_snapshot().await;
    config.router.url = Some(format!("http://{router_addr}"));
    config.router.identity = Some(cc_switch_server::domain::settings::config::RouterIdentity {
        installation_id: "inst-tunnel".to_string(),
        public_key: BASE64_STANDARD.encode([8_u8; 32]),
        private_key: BASE64_STANDARD.encode([7_u8; 32]),
        control_secret: Some("control-secret".to_string()),
    });
    config.client.tunnel_status = Some("connected".to_string());
    state.replace_config(config).await.unwrap();

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/router/client-tunnel/stop",
            json!(null),
            Some(&token),
        ))
        .await
        .unwrap();

    let status = response.status();
    let body = json_body(response).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["tunnelStatus"].as_str(), Some("stopped"));
    assert_eq!(seen.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn automatic_router_reconcile_upserts_installation_shares_without_delete_all() {
    let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let router_addr = listener.local_addr().unwrap();
    let batch_seen = Arc::new(AtomicUsize::new(0));
    let router = Router::new()
        .route(
            "/v1/shares/batch-sync",
            post(
                |State(batch_seen): State<Arc<AtomicUsize>>,
                 axum::Json(body): axum::Json<Value>| async move {
                    assert_eq!(body["installationId"].as_str(), Some("inst-runtime"));
                    assert_eq!(body["ops"].as_array().map(Vec::len), Some(1));
                    assert_eq!(body["ops"][0]["kind"].as_str(), Some("upsert"));
                    assert_eq!(
                        body["ops"][0]["share"]["shareId"].as_str(),
                        Some("share-runtime")
                    );
                    batch_seen.fetch_add(1, Ordering::SeqCst);
                    axum::Json(json!({"ok": true}))
                },
            ),
        )
        .with_state(batch_seen.clone());
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });

    let state = test_state();
    let mut config = state.config_snapshot().await;
    config.router.url = Some(format!("http://{router_addr}"));
    config.router.identity = Some(cc_switch_server::domain::settings::config::RouterIdentity {
        installation_id: "inst-runtime".to_string(),
        public_key: BASE64_STANDARD.encode([8_u8; 32]),
        private_key: BASE64_STANDARD.encode([7_u8; 32]),
        control_secret: Some("control-secret".to_string()),
    });
    state.replace_config(config).await.unwrap();
    let mut share = test_share_input("share-runtime", "provider-runtime", ProviderType::Codex);
    share.tunnel_subdomain = Some("runtime-sub".to_string());
    state
        .mutate_shares_immediate(|store| {
            let _ = store.upsert(share);
        })
        .await
        .unwrap();

    let reconciled = cc_switch_server::state::reconcile_all_shares_to_router(state)
        .await
        .unwrap();
    assert_eq!(reconciled, 1);
    assert_eq!(batch_seen.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn router_log_retry_syncs_canonical_uuid_request_ids() {
    const ROUTER_REQUEST_ID: &str = "550e8400-e29b-41d4-a716-446655440000";

    let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let router_addr = listener.local_addr().unwrap();
    let batch_seen = Arc::new(AtomicUsize::new(0));
    let router = Router::new()
        .route(
            "/v1/share-request-logs/batch-sync",
            post(
                |State(batch_seen): State<Arc<AtomicUsize>>,
                 axum::Json(body): axum::Json<Value>| async move {
                    assert_eq!(body["installationId"], "inst-request-logs");
                    assert_eq!(body["logs"].as_array().map(Vec::len), Some(1));
                    assert_eq!(body["logs"][0]["requestId"], ROUTER_REQUEST_ID);
                    assert_eq!(body["logs"][0]["shareId"], "share-request-logs");
                    batch_seen.fetch_add(1, Ordering::SeqCst);
                    axum::Json(json!({"ok": true}))
                },
            ),
        )
        .with_state(batch_seen.clone());
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });

    let state = test_state();
    let app = app_router(state.clone());
    let token = setup_and_login(&app).await;
    let mut config = state.config_snapshot().await;
    config.router.url = Some(format!("http://{router_addr}"));
    config.router.identity = Some(cc_switch_server::domain::settings::config::RouterIdentity {
        installation_id: "inst-request-logs".to_string(),
        public_key: BASE64_STANDARD.encode([8_u8; 32]),
        private_key: BASE64_STANDARD.encode([7_u8; 32]),
        control_secret: Some("control-secret".to_string()),
    });
    state.replace_config(config).await.unwrap();
    state
        .mutate_shares_immediate(|store| {
            store
                .upsert(test_share_input(
                    "share-request-logs",
                    "provider-request-logs",
                    ProviderType::Codex,
                ))
                .unwrap()
        })
        .await
        .unwrap();

    let mut log = UsageLog::new(
        AppKind::Codex,
        "provider-request-logs".to_string(),
        "Provider Request Logs".to_string(),
        ProviderType::Codex,
        200,
        25,
        UsageModelMetadata {
            model: Some("gpt-5.5".to_string()),
            ..Default::default()
        },
        TokenUsage::default(),
    );
    log.apply_context(UsageLogContext {
        request_id: Some(ROUTER_REQUEST_ID.to_string()),
        share_id: Some("share-request-logs".to_string()),
        data_source: Some("direct".to_string()),
        ..Default::default()
    });
    state.push_usage_log(log).await.unwrap();

    let response = app
        .oneshot(json_request(
            Method::POST,
            "/api/usage/router-sync/retry",
            json!(null),
            Some(&token),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let response = json_body(response).await;
    assert_eq!(response["attempted"], 1);
    assert_eq!(response["synced"], 1);
    assert_eq!(response["failed"], 0);
    assert_eq!(batch_seen.load(Ordering::SeqCst), 1);

    let usage = state.usage_snapshot().await;
    let synced = usage
        .logs
        .iter()
        .find(|log| log.request_id == ROUTER_REQUEST_ID)
        .unwrap();
    assert!(synced.router_last_synced_at_ms.is_some());
    assert_eq!(synced.router_sync_attempt_count, 1);
}

#[tokio::test]
async fn provider_share_settings_are_saved_atomically() {
    let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let router_addr = listener.local_addr().unwrap();
    let router = Router::new()
        .route(
            "/v1/shares/claim-subdomain",
            post(|| async { StatusCode::OK }),
        )
        .route("/v1/shares/batch-sync", post(|| async { StatusCode::OK }));
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let state = test_state();
    let app = app_router(state.clone());
    let token = setup_and_login(&app).await;
    let mut config = state.config_snapshot().await;
    config.router.url = Some(format!("http://{router_addr}"));
    config.router.identity = Some(cc_switch_server::domain::settings::config::RouterIdentity {
        installation_id: "inst-provider-save".to_string(),
        public_key: BASE64_STANDARD.encode([8_u8; 32]),
        private_key: BASE64_STANDARD.encode([7_u8; 32]),
        control_secret: Some("control-secret".to_string()),
    });
    state.replace_config(config).await.unwrap();

    let mut input = test_share_input("share-provider-save", "provider-save", ProviderType::Codex);
    input.tunnel_subdomain = Some("before-save".to_string());
    state
        .mutate_shares_immediate(|store| store.upsert(input).unwrap())
        .await
        .unwrap();

    let response = app
        .oneshot(json_request(
            Method::POST,
            "/web-api/invoke/save_provider_share",
            json!({
                "params": {
                    "shareId": "share-provider-save",
                    "ownerEmail": "forged@example.com",
                    "subdomain": "after-save",
                    "description": "Provider-scoped share",
                    "forSale": "Yes",
                    "saleMarketKind": "token",
                    "marketAccessMode": "selected",
                    "sharedWithEmails": ["friend@example.com"],
                    "accessByApp": {
                        "codex": {
                            "sharedWithEmails": ["friend@example.com"],
                            "marketAccessMode": "selected"
                        }
                    },
                    "appSettings": {
                        "codex": {
                            "forSale": "Yes",
                            "saleMarketKind": "token",
                            "marketAccessMode": "selected",
                            "sharedWithEmails": ["friend@example.com"],
                            "tokenLimit": 123,
                            "parallelLimit": 4,
                            "expiresAt": "2030-01-01T00:00:00Z"
                        }
                    },
                    "tokenLimit": 123,
                    "parallelLimit": 4,
                    "expiresAt": "2030-01-01T00:00:00Z"
                }
            }),
            Some(&token),
        ))
        .await
        .unwrap();
    let status = response.status();
    let saved = json_body(response).await;
    assert_eq!(status, StatusCode::OK, "response body: {saved}");
    assert_eq!(saved["ownerEmail"].as_str(), Some("owner@example.com"));
    assert_eq!(saved["tunnelSubdomain"].as_str(), Some("after-save"));
    assert_eq!(saved["description"].as_str(), Some("Provider-scoped share"));
    assert_eq!(saved["forSale"].as_bool(), Some(true));
    assert_eq!(saved["saleMarketKind"].as_str(), Some("token"));
    assert_eq!(saved["tokenLimit"].as_u64(), Some(123));
    assert_eq!(saved["parallelLimit"].as_u64(), Some(4));
    assert_eq!(saved["expiresAt"].as_i64(), Some(1_893_456_000_000));
    assert_eq!(
        saved["acl"]["sharedWithEmails"][0].as_str(),
        Some("friend@example.com")
    );
    assert_eq!(saved["bindings"].as_array().map(Vec::len), Some(1));
    assert_eq!(
        saved["bindings"][0]["providerId"].as_str(),
        Some("provider-save")
    );
    assert_eq!(
        saved["appSettings"]["codex"]["expiresAt"].as_str(),
        Some("2030-01-01T00:00:00+00:00")
    );
    let stored = state
        .mutate_shares(|store| store.get("share-provider-save").cloned())
        .await
        .unwrap();
    assert_eq!(stored.router_synced_revision, stored.config_revision);
    assert!(stored.router_last_sync_error.is_none());
}

#[tokio::test]
async fn web_runtime_context_reports_setup_and_authenticated_admin() {
    let app = app_router(test_state());

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/web-api/context")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(body["mode"].as_str(), Some("client-login"));
    assert_eq!(body["status"].as_str(), Some("setup-required"));
    assert_eq!(body["uiAutomation"]["allowed"].as_bool(), Some(false));

    let token = setup_and_login(&app).await;
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/web-api/context")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(body["mode"].as_str(), Some("client-login"));
    assert_eq!(body["status"].as_str(), Some("auth-required"));

    let response = app
        .oneshot(json_request(
            Method::GET,
            "/web-api/context",
            Value::Null,
            Some(&token),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(body["mode"].as_str(), Some("local-admin"));
    assert_eq!(body["status"].as_str(), Some("authenticated"));
    assert_eq!(body["apps"].as_array().unwrap().len(), 3);
    assert!(body["commands"].as_array().unwrap().len() > 10);
}

#[tokio::test]
async fn web_runtime_context_treats_invalid_token_as_auth_required() {
    let app = app_router(test_state());
    let _token = setup_and_login(&app).await;

    let response = app
        .oneshot(json_request(
            Method::GET,
            "/web-api/context",
            Value::Null,
            Some("invalid-token"),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(body["mode"].as_str(), Some("client-login"));
    assert_eq!(body["status"].as_str(), Some("auth-required"));
}

#[tokio::test]
async fn web_invoke_complete_server_setup_works_without_session_before_setup() {
    let app = app_router(test_state());

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/web-api/invoke/complete_server_setup",
            json!({
                "password": "password123",
                "ownerEmail": "owner@example.com",
                "routerUrl": "http://127.0.0.1:9",
                "clientTunnelSubdomain": "ownerabcde"
            }),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .oneshot(json_request(
            Method::POST,
            "/web-api/invoke/complete_server_setup",
            json!({
                "password": "password456",
                "ownerEmail": "other@example.com",
                "routerUrl": "http://127.0.0.1:10"
            }),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn setup_bootstrap_issues_session_token_without_prior_login() {
    let app = app_router(test_state());

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/setup/bootstrap",
            json!({
                "password": "password123",
                "ownerEmail": "owner@example.com",
                "routerUrl": "http://127.0.0.1:9",
                "clientTunnelSubdomain": "ownerabcde"
            }),
            None,
        ))
        .await
        .unwrap();
    let body = json_body(response).await;
    assert_eq!(body["ok"].as_bool(), Some(true));
    assert!(body["sessionToken"]
        .as_str()
        .is_some_and(|value| !value.is_empty()));
}

#[tokio::test]
async fn setup_validate_is_dry_run_without_persisting_config() {
    let app = app_router(test_state());

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/setup/validate",
            json!({
                "password": "password123",
                "ownerEmail": "owner@example.com",
                "routerUrl": "http://127.0.0.1:9",
                "clientTunnelSubdomain": "ownerabcde"
            }),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(body["dryRun"].as_bool(), Some(true));

    let status = json_body(
        app.oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/api/setup/status")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap(),
    )
    .await;
    assert_eq!(status["needsSetup"].as_bool(), Some(true));
}

#[tokio::test]
async fn setup_rejects_repeat_initialization_with_code() {
    let app = app_router(test_state());
    let _ = setup_and_login(&app).await;

    let response = app
        .oneshot(json_request(
            Method::POST,
            "/api/setup",
            json!({
                "password": "password456",
                "ownerEmail": "other@example.com",
                "routerUrl": "http://127.0.0.1:10"
            }),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let body = json_body(response).await;
    assert_eq!(body["code"].as_str(), Some("setup_already_complete"));
}

#[tokio::test]
async fn web_invoke_email_auth_works_without_session_after_setup() {
    let app = app_router(test_state());

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/setup",
            json!({
                "password": "password123",
                "ownerEmail": "owner@example.com",
                "routerUrl": "http://127.0.0.1:9",
                "clientTunnelSubdomain": "ownerabcde"
            }),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/web-api/invoke/request_admin_email_login_code",
            json!({ "email": "owner@example.com" }),
            None,
        ))
        .await
        .unwrap();
    assert_ne!(response.status(), StatusCode::UNAUTHORIZED);
    let body = json_body(response).await;
    assert_ne!(
        body["error"].as_str(),
        Some("missing or invalid bearer token")
    );
}

#[tokio::test]
async fn web_invoke_registry_returns_stable_errors() {
    let app = app_router(test_state());
    let token = setup_and_login(&app).await;

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/web-api/invoke/get_build_info",
            json!({}),
            Some(&token),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(body["processId"].as_u64(), Some(std::process::id() as u64));
    assert_eq!(body["processInstanceId"].as_str().map(str::len), Some(32));

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/web-api/invoke/apply_claude_plugin_config",
            json!({ "official": true }),
            Some(&token),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/web-api/invoke/set_window_theme",
            json!({ "theme": "dark" }),
            Some(&token),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let body = json_body(response).await;
    assert_eq!(body["code"].as_str(), Some("cc_switch_feature_disabled"));
    assert_eq!(body["type"].as_str(), Some("feature_disabled"));

    let response = app
        .oneshot(json_request(
            Method::POST,
            "/web-api/invoke/not_a_desktop_command",
            json!({}),
            Some(&token),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);
    let body = json_body(response).await;
    assert_eq!(body["code"].as_str(), Some("cc_switch_web_invoke_unknown"));
    assert_eq!(body["type"].as_str(), Some("web_invoke_unknown"));
}

#[tokio::test]
async fn web_invoke_request_logs_matches_desktop_filter_and_pagination_contract() {
    const START_SECONDS: u128 = 1_700_000_000;

    let state = test_state();
    let app = app_router(state.clone());
    let token = setup_and_login(&app).await;

    let make_log = |request_id: &str, created_at_ms: u128| {
        let mut log = UsageLog::new(
            AppKind::Codex,
            "provider-1".to_string(),
            "Provider One".to_string(),
            ProviderType::Codex,
            200,
            321,
            UsageModelMetadata {
                model: Some("request-alias".to_string()),
                requested_model: Some("request-alias".to_string()),
                actual_model: Some("gpt-5.5".to_string()),
                actual_model_source: Some("response".to_string()),
                pricing_model: Some("gpt-5.5-priced".to_string()),
            },
            TokenUsage {
                input_tokens: Some(11),
                output_tokens: Some(7),
                cache_read_tokens: Some(3),
                cache_creation_tokens: Some(2),
                total_tokens: Some(23),
                ..Default::default()
            },
        );
        log.request_id = request_id.to_string();
        log.request_agent = Some("codex-cli".to_string());
        log.first_token_ms = Some(42);
        log.cost_multiplier = Some(1.25);
        log.input_cost_usd = Some(0.1);
        log.output_cost_usd = Some(0.2);
        log.cache_read_cost_usd = Some(0.03);
        log.cache_creation_cost_usd = Some(0.04);
        log.total_cost_usd = Some(0.37);
        log.created_at_ms = created_at_ms;
        log.apply_context(UsageLogContext {
            share_id: Some("share-1".to_string()),
            share_name: Some("Share One".to_string()),
            user_email: Some("reader@example.com".to_string()),
            data_source: Some("proxy".to_string()),
            ..Default::default()
        });
        log
    };

    let old = make_log("req-old", START_SECONDS * 1_000);
    let new = make_log("req-new", (START_SECONDS + 1) * 1_000 + 999);

    let mut wrong_share = make_log("req-wrong-share", START_SECONDS * 1_000 + 100);
    wrong_share.share_id = Some("share-2".to_string());
    let mut wrong_app = make_log("req-wrong-app", START_SECONDS * 1_000 + 200);
    wrong_app.app = AppKind::Claude;
    let mut wrong_provider = make_log("req-wrong-provider", START_SECONDS * 1_000 + 300);
    wrong_provider.provider_name = "Provider Two".to_string();
    let mut wrong_model = make_log("req-wrong-model", START_SECONDS * 1_000 + 400);
    wrong_model.pricing_model = Some("gpt-other-priced".to_string());
    let mut wrong_status = make_log("req-wrong-status", START_SECONDS * 1_000 + 500);
    wrong_status.status_code = 429;
    let too_early = make_log("req-too-early", (START_SECONDS - 1) * 1_000 + 999);

    for log in [
        too_early,
        new,
        wrong_share,
        wrong_app,
        wrong_provider,
        wrong_model,
        wrong_status,
        old,
    ] {
        state.push_usage_log(log).await.unwrap();
    }

    let filters = json!({
        "shareId": "share-1",
        "appType": "codex",
        "providerName": "Provider One",
        "model": "gpt-5.5-priced",
        "statusCode": 200,
        "startDate": START_SECONDS as u64,
        "endDate": (START_SECONDS + 1) as u64
    });
    let first_page = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/web-api/invoke/get_request_logs",
            json!({"filters": filters, "page": 0, "pageSize": 1}),
            Some(&token),
        ))
        .await
        .unwrap();
    assert_eq!(first_page.status(), StatusCode::OK);
    let first_page = json_body(first_page).await;
    assert_eq!(first_page["total"], 2);
    assert_eq!(first_page["page"], 0);
    assert_eq!(first_page["pageSize"], 1);
    assert_eq!(first_page["data"].as_array().map(Vec::len), Some(1));
    let newest = &first_page["data"][0];
    assert_eq!(newest["requestId"], "req-new");
    assert_eq!(newest["appType"], "codex");
    assert_eq!(newest["providerName"], "Provider One");
    assert_eq!(newest["latencyMs"], 321);
    assert_eq!(newest["firstTokenMs"], 42);
    assert_eq!(newest["createdAt"], (START_SECONDS + 1) as u64);
    assert_eq!(newest["inputTokens"], 11);
    assert_eq!(newest["totalCostUsd"], "0.370000");
    assert_eq!(newest["costMultiplier"], "1.25");
    assert_eq!(newest["shareId"], "share-1");
    assert_eq!(newest["userEmail"], "reader@example.com");

    let second_page = app
        .oneshot(json_request(
            Method::POST,
            "/web-api/invoke/get_request_logs",
            json!({"filters": filters, "page": 1, "pageSize": 1}),
            Some(&token),
        ))
        .await
        .unwrap();
    assert_eq!(second_page.status(), StatusCode::OK);
    let second_page = json_body(second_page).await;
    assert_eq!(second_page["total"], 2);
    assert_eq!(second_page["data"][0]["requestId"], "req-old");
}

#[tokio::test]
async fn web_invoke_get_providers_returns_desktop_record_shape() {
    let state = test_state();
    upsert_test_provider(
        &state,
        AppKind::Codex,
        Provider {
            id: "codex-web".to_string(),
            name: "Codex Web".to_string(),
            settings_config: json!({"env": {"OPENAI_API_KEY": "sk-test"}}),
            category: None,
            meta: None,
            extra: Default::default(),
        },
    )
    .await;
    let app = app_router(state);
    let token = setup_and_login(&app).await;

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/web-api/invoke/get_providers",
            json!({"app": "codex"}),
            Some(&token),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(body["codex-web"]["name"].as_str(), Some("Codex Web"));

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/web-api/invoke/get_current_provider",
            json!({"app": "codex"}),
            Some(&token),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(json_body(response).await.as_str(), Some("codex-web"));

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/web-api/invoke/switch_provider",
            json!({"app": "codex", "id": "codex-web"}),
            Some(&token),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/web-api/invoke/clear_current_provider",
            json!({"app": "codex"}),
            Some(&token),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .oneshot(json_request(
            Method::POST,
            "/web-api/invoke/get_current_provider",
            json!({"app": "codex"}),
            Some(&token),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(json_body(response).await.as_str(), Some(""));
}

#[tokio::test]
async fn admin_logs_tail_honors_api_management_gate() {
    let state = test_state();
    let app = app_router(state);
    let token = setup_and_login(&app).await;

    let disabled = app
        .clone()
        .oneshot(json_request(
            Method::GET,
            "/web-api/admin/logs/tail",
            json!(null),
            Some(&token),
        ))
        .await
        .unwrap();
    assert_eq!(disabled.status(), StatusCode::FORBIDDEN);

    let saved = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/web-api/invoke/set_api_management",
            json!({
                "config": {
                    "diagnosticsEnabled": true,
                    "logEnabled": true,
                    "restartEnabled": false,
                    "upgradeEnabled": false,
                    "logTailLines": 25
                }
            }),
            Some(&token),
        ))
        .await
        .unwrap();
    assert_eq!(saved.status(), StatusCode::OK);

    let enabled = app
        .clone()
        .oneshot(json_request(
            Method::GET,
            "/web-api/admin/logs/tail?lines=5",
            json!(null),
            Some(&token),
        ))
        .await
        .unwrap();
    assert_eq!(enabled.status(), StatusCode::OK);
    let body = json_body(enabled).await;
    assert!(body["lines"].as_u64().is_some());
    assert!(body["lines"].as_u64().unwrap() <= 5);
    assert!(body["content"].is_string());
    assert!(body["path"].is_string());
    assert!(body["source"].is_string());
}

#[tokio::test]
async fn debug_api_uses_scoped_expiring_token_without_admin_session() {
    let state = test_state();
    let app = app_router(state);
    let admin_token = setup_and_login(&app).await;

    let configured = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/web-api/invoke/set_api_management",
            json!({
                "config": {
                    "diagnosticsEnabled": true,
                    "logEnabled": true,
                    "restartEnabled": false,
                    "upgradeEnabled": false,
                    "logTailLines": 10
                }
            }),
            Some(&admin_token),
        ))
        .await
        .unwrap();
    assert_eq!(configured.status(), StatusCode::OK);

    let generated = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/web-api/invoke/generate_debug_token",
            json!({ "ttlHours": 1 }),
            Some(&admin_token),
        ))
        .await
        .unwrap();
    assert_eq!(generated.status(), StatusCode::OK);
    let debug_token = json_body(generated).await["token"]
        .as_str()
        .unwrap()
        .to_string();

    let anonymous = app
        .clone()
        .oneshot(json_request(
            Method::GET,
            "/web-api/debug/runtime",
            json!(null),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);

    let runtime = app
        .clone()
        .oneshot(json_request(
            Method::GET,
            "/web-api/debug/runtime",
            json!(null),
            Some(&debug_token),
        ))
        .await
        .unwrap();
    assert_eq!(runtime.status(), StatusCode::OK);
    assert_eq!(
        json_body(runtime).await["processId"].as_u64(),
        Some(std::process::id() as u64)
    );

    let revoked = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/web-api/invoke/revoke_debug_token",
            json!({}),
            Some(&admin_token),
        ))
        .await
        .unwrap();
    assert_eq!(revoked.status(), StatusCode::OK);

    let after_revoke = app
        .oneshot(json_request(
            Method::GET,
            "/web-api/debug/runtime",
            json!(null),
            Some(&debug_token),
        ))
        .await
        .unwrap();
    assert_eq!(after_revoke.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn client_web_streams_require_authorization_headers() {
    let state = test_state();
    let app = app_router(state);
    let token = setup_and_login(&app).await;

    let query_token = app
        .clone()
        .oneshot(json_request(
            Method::GET,
            &format!("/web-api/events?token={token}"),
            json!(null),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(query_token.status(), StatusCode::UNAUTHORIZED);

    let authorized = app
        .clone()
        .oneshot(json_request(
            Method::GET,
            "/web-api/events",
            json!(null),
            Some(&token),
        ))
        .await
        .unwrap();
    assert_eq!(authorized.status(), StatusCode::OK);

    let upgrade_status = app
        .oneshot(json_request(
            Method::GET,
            "/web-api/admin/upgrade/status?taskId=missing",
            json!(null),
            Some(&token),
        ))
        .await
        .unwrap();
    assert_eq!(upgrade_status.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn payout_profile_admin_write_public_read_and_clear_contract() {
    let state = test_state();
    let app = app_router(state);
    let token = setup_and_login(&app).await;

    let unauthorized = app
        .clone()
        .oneshot(json_request(
            Method::GET,
            "/api/settings/payout-profile",
            json!(null),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

    let saved = app
        .clone()
        .oneshot(json_request(
            Method::PUT,
            "/api/settings/payout-profile",
            json!({
                "address": "0x5aaeb6053f3e94c9b9a09f33669435e7ef1beaed",
                "token": "USDC",
                "networks": ["eip155:8453", "eip155:56", "eip155:56"]
            }),
            Some(&token),
        ))
        .await
        .unwrap();
    assert_eq!(saved.status(), StatusCode::OK);
    let saved = json_body(saved).await;
    assert_eq!(saved["configured"], true);
    assert_eq!(saved["revision"], 1);
    assert_eq!(
        saved["profile"]["address"],
        "0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed"
    );
    assert_eq!(
        saved["profile"]["networks"],
        json!(["eip155:56", "eip155:8453"])
    );
    assert!(saved["sync"]["lastError"].as_str().is_some());

    let public = app
        .clone()
        .oneshot(json_request(
            Method::GET,
            "/.well-known/cc-switch/payout-profile",
            json!(null),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(public.status(), StatusCode::OK);
    assert_eq!(public.headers()["cache-control"], "public, max-age=60");
    assert!(public.headers().get("etag").is_some());
    let public = json_body(public).await;
    assert_eq!(public["configured"], true);
    assert!(public.get("sync").is_none());

    let cleared = app
        .clone()
        .oneshot(json_request(
            Method::DELETE,
            "/api/settings/payout-profile",
            json!(null),
            Some(&token),
        ))
        .await
        .unwrap();
    assert_eq!(cleared.status(), StatusCode::OK);
    let cleared = json_body(cleared).await;
    assert_eq!(cleared["configured"], false);
    assert_eq!(cleared["revision"], 2);
    assert!(cleared["profile"].is_null());
}

fn test_state() -> ServerState {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let config_dir = std::env::temp_dir().join(format!("cc-switch-server-http-test-{nanos}"));
    let log_capture = Arc::new(cc_switch_server::logging::LogCapture::new(
        cc_switch_server::logging::RING_BUFFER_CAPACITY,
    ));
    ServerStateInner::load(
        Cli {
            host: IpAddr::V4(Ipv4Addr::LOCALHOST),
            port: 0,
            config_dir: Some(config_dir),
            web_dist_dir: None,
            log_level: "warn".to_string(),
            command: None,
        },
        log_capture,
    )
    .unwrap()
}

async fn setup_and_login(app: &Router) -> String {
    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/setup",
            json!({
                "password": "password123",
                "ownerEmail": "owner@example.com",
                "routerUrl": "http://127.0.0.1:9",
                "clientTunnelSubdomain": "ownerabcde"
            }),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/auth/login",
            json!({"method": "password", "password": "password123"}),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    json_body(response).await["token"]
        .as_str()
        .unwrap()
        .to_string()
}

async fn spawn_broken_chunked_upstream() -> std::net::SocketAddr {
    spawn_broken_chunked_status_upstream("200 OK", "text/event-stream").await
}

async fn spawn_broken_chunked_status_upstream(
    status: &'static str,
    content_type: &'static str,
) -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buffer = [0_u8; 2048];
        let _ = socket.read(&mut buffer).await;
        let response = format!(
            "HTTP/1.1 {status}\r\ncontent-type: {content_type}\r\ntransfer-encoding: chunked\r\n\r\n5\r\nhello\r\nZZ\r\n"
        );
        socket.write_all(response.as_bytes()).await.unwrap();
        let _ = socket.shutdown().await;
    });
    addr
}

#[tokio::test]
async fn web_invoke_direct_owner_update_is_compatibility_noop_only() {
    let state = test_state();
    let app = app_router(state.clone());
    let token = setup_and_login(&app).await;
    state
        .mutate_shares_immediate(|store| {
            let _ = store.upsert(test_share_input(
                "share-owner-gate",
                "p-owner",
                ProviderType::Codex,
            ));
        })
        .await
        .unwrap();

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/web-api/invoke/update_share_owner_email",
            json!({
                "params": {
                    "shareId": "share-owner-gate",
                    "ownerEmail": "new-owner@example.com"
                }
            }),
            Some(&token),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CONFLICT);

    let response = app
        .oneshot(json_request(
            Method::POST,
            "/web-api/invoke/update_share_owner_email",
            json!({
                "params": {
                    "shareId": "share-owner-gate",
                    "ownerEmail": "owner@example.com"
                }
            }),
            Some(&token),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(body["ownerEmail"].as_str(), Some("owner@example.com"));
}

#[tokio::test]
async fn web_invoke_email_auth_owner_change_updates_config_and_shares() {
    let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let router_addr = listener.local_addr().unwrap();
    let verify_seen = Arc::new(AtomicUsize::new(0));
    let change_seen = Arc::new(AtomicUsize::new(0));
    let router = Router::new()
        .route(
            "/v1/client-web/auth/email/verify-code",
            post(
                |State((verify_seen, _change_seen)): State<(
                    Arc<AtomicUsize>,
                    Arc<AtomicUsize>,
                )>,
                 axum::Json(body): axum::Json<Value>| async move {
                    assert_eq!(body["installationId"].as_str(), Some("inst-owner-change"));
                    assert_eq!(body["email"].as_str(), Some("new-owner@example.com"));
                    assert_eq!(body["code"].as_str(), Some("123456"));
                    verify_seen.fetch_add(1, Ordering::SeqCst);
                    axum::Json(json!({
                        "user": {"id": "user-new", "email": "new-owner@example.com"},
                        "accessToken": "new-owner-access",
                        "refreshToken": "new-owner-refresh",
                        "expiresAt": "2099-01-01T00:00:00Z",
                        "refreshExpiresAt": "2099-02-01T00:00:00Z"
                    }))
                },
            ),
        )
        .route(
            "/v1/installations/change-owner-email",
            post(
                |State((_verify_seen, change_seen)): State<(
                    Arc<AtomicUsize>,
                    Arc<AtomicUsize>,
                )>,
                 headers: HeaderMap,
                 axum::Json(body): axum::Json<Value>| async move {
                    assert_eq!(
                        headers
                            .get(axum::http::header::AUTHORIZATION)
                            .and_then(|value| value.to_str().ok()),
                        Some("Bearer new-owner-access")
                    );
                    assert_eq!(body["installationId"].as_str(), Some("inst-owner-change"));
                    assert_eq!(body["oldEmail"].as_str(), Some("owner@example.com"));
                    assert_eq!(body["newEmail"].as_str(), Some("new-owner@example.com"));
                    assert!(body["timestampMs"].is_number());
                    assert!(body["nonce"]
                        .as_str()
                        .is_some_and(|value| !value.is_empty()));
                    assert!(body["signature"]
                        .as_str()
                        .is_some_and(|value| !value.is_empty()));
                    change_seen.fetch_add(1, Ordering::SeqCst);
                    axum::Json(json!({
                        "ok": true,
                        "oldEmail": "owner@example.com",
                        "newEmail": "new-owner@example.com",
                        "updatedShares": 1
                    }))
                },
            ),
        )
        .with_state((verify_seen.clone(), change_seen.clone()));
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });

    let state = test_state();
    let app = app_router(state.clone());
    let token = setup_and_login(&app).await;
    let mut config = state.config_snapshot().await;
    config.router.url = Some(format!("http://{router_addr}"));
    config.router.identity = Some(cc_switch_server::domain::settings::config::RouterIdentity {
        installation_id: "inst-owner-change".to_string(),
        public_key: BASE64_STANDARD.encode([8_u8; 32]),
        private_key: BASE64_STANDARD.encode([7_u8; 32]),
        control_secret: Some("control-secret".to_string()),
    });
    state.replace_config(config).await.unwrap();
    cc_switch_server::clients::router::email_auth::save_state(
        &state.config_dir,
        &cc_switch_server::clients::router::email_auth::EmailAuthState {
            email: "owner@example.com".to_string(),
            router_domain: None,
            access_token: Some("owner-access".to_string()),
            refresh_token: Some("owner-refresh".to_string()),
            expires_at: Some(4_102_444_800),
            refresh_expires_at: Some(4_105_123_200),
            verified_at: now_ms() as i64 / 1000,
        },
    )
    .unwrap();
    state
        .mutate_shares_immediate(|store| {
            let _ = store.upsert(test_share_input(
                "share-owner-change",
                "p-owner",
                ProviderType::Codex,
            ));
        })
        .await
        .unwrap();

    let response = app
        .oneshot(json_request(
            Method::POST,
            "/web-api/invoke/email_auth_change_owner_email",
            json!({
                "currentEmail": "owner@example.com",
                "newEmail": "new-owner@example.com",
                "code": "123456"
            }),
            Some(&token),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(body["authenticated"].as_bool(), Some(true));
    assert_eq!(body["email"].as_str(), Some("new-owner@example.com"));
    assert_eq!(verify_seen.load(Ordering::SeqCst), 1);
    assert_eq!(change_seen.load(Ordering::SeqCst), 1);
    assert_eq!(
        state.config_snapshot().await.owner.email.as_deref(),
        Some("new-owner@example.com")
    );
    assert_eq!(
        state
            .mutate_shares(|store| {
                store
                    .shares
                    .iter()
                    .find(|share| share.id == "share-owner-change")
                    .and_then(|share| share.owner_email.clone())
            })
            .await
            .as_deref(),
        Some("new-owner@example.com")
    );
}

#[tokio::test]
async fn web_password_login_authenticates_invoke() {
    let app = app_router(test_state());
    let _ = setup_and_login(&app).await;

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/web-api/auth/password/login",
            json!({ "password": "password123" }),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    let access_token = body["accessToken"].as_str().unwrap();

    let response = app
        .oneshot(json_request(
            Method::POST,
            "/web-api/invoke/get_tool_versions",
            json!({ "tools": [] }),
            Some(access_token),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn web_password_change_updates_login_password() {
    let app = app_router(test_state());
    let token = setup_and_login(&app).await;

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/web-api/auth/password/change",
            json!({
                "currentPassword": "password123",
                "newPassword": "newpassword9"
            }),
            Some(&token),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/auth/login",
            json!({"method": "password", "password": "newpassword9"}),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/auth/login",
            json!({"method": "password", "password": "password123"}),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn router_identity_headers_authenticate_invoke() {
    let app = app_router(test_state());
    let _ = setup_and_login(&app).await;

    let response = app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/web-api/invoke/get_tool_versions")
                .header("content-type", "application/json")
                .header("x-cc-switch-web-user-email", "owner@example.com")
                .header("x-cc-switch-web-role", "owner")
                .body(Body::from(r#"{"tools":[]}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

fn json_request(
    method: Method,
    uri: &str,
    value: serde_json::Value,
    bearer: Option<&str>,
) -> Request<Body> {
    let body = if value.is_null() {
        Body::empty()
    } else {
        Body::from(serde_json::to_vec(&value).unwrap())
    };
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header(axum::http::header::CONTENT_TYPE, "application/json");
    if let Some(token) = bearer {
        builder = builder.header(axum::http::header::AUTHORIZATION, format!("Bearer {token}"));
    }
    builder.body(body).unwrap()
}

fn control_request(
    path: &str,
    body: Vec<u8>,
    timestamp_ms: i64,
    nonce: &str,
    signature: &str,
) -> Request<Body> {
    Request::builder()
        .method(Method::POST)
        .uri(path)
        .header(axum::http::header::CONTENT_TYPE, "application/json")
        .header("x-ctl-installation-id", "inst-ctl")
        .header("x-ctl-timestamp-ms", timestamp_ms.to_string())
        .header("x-ctl-nonce", nonce)
        .header("x-ctl-signature", signature)
        .body(Body::from(body))
        .unwrap()
}

async fn configure_share_router_identity(state: &ServerState) {
    let mut config = state.config_snapshot().await;
    config.router.identity = Some(cc_switch_server::domain::settings::config::RouterIdentity {
        installation_id: "inst-share-router".to_string(),
        public_key: "public-key".to_string(),
        private_key: "private-key".to_string(),
        control_secret: Some("share-router-control-secret".to_string()),
    });
    state.replace_config(config).await.unwrap();
}

fn share_router_request(
    method: Method,
    uri: &str,
    share_ids: &[&str],
    nonce: &str,
    body: Vec<u8>,
) -> Request<Body> {
    let timestamp_ms = now_ms() as i64;
    let signature = BASE64_STANDARD.encode(
        control_signature_for_method(
            method.as_str(),
            uri,
            "share-router-control-secret",
            &body,
            timestamp_ms,
            nonce,
        )
        .unwrap(),
    );
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("x-ctl-installation-id", "inst-share-router")
        .header("x-ctl-timestamp-ms", timestamp_ms.to_string())
        .header("x-ctl-nonce", nonce)
        .header("x-ctl-signature", signature);
    for share_id in share_ids {
        builder = builder.header("x-cc-switch-share-id", *share_id);
    }
    builder.body(Body::from(body)).unwrap()
}

fn test_share_input(id: &str, provider_id: &str, provider_type: ProviderType) -> UpsertShareInput {
    UpsertShareInput {
        id: Some(id.to_string()),
        owner_email: Some("owner@example.com".to_string()),
        app: AppKind::Codex,
        provider_id: provider_id.to_string(),
        provider_type,
        display_name: Some(id.to_string()),
        enabled: None,
        status: None,
        subscription_level: None,
        account_email: None,
        quota_percent: None,
        tunnel_subdomain: None,
        acl: None,
        token_limit: None,
        parallel_limit: None,
        expires_at: None,
        for_sale: None,
        free_access: None,
        sale_market_kind: None,
        access_by_app: BTreeMap::new(),
        app_settings: BTreeMap::new(),
        for_sale_official_price_percent_by_app: BTreeMap::new(),
        official_price_percent: None,
        auto_start: None,
        description: None,
        bindings: Vec::new(),
        runtime_snapshot: None,
        market_grant: None,
    }
}

fn event_stream_bytes(events: Vec<(&str, Value)>) -> Vec<u8> {
    events
        .into_iter()
        .flat_map(|(event_type, payload)| event_frame(event_type, payload))
        .collect()
}

fn event_frame(event_type: &str, payload: Value) -> Vec<u8> {
    let mut headers = Vec::new();
    push_event_string_header(&mut headers, ":event-type", event_type);
    let payload = serde_json::to_vec(&payload).unwrap();
    let total_len = 12 + headers.len() + payload.len() + 4;
    let mut frame = Vec::with_capacity(total_len);
    frame.extend_from_slice(&(total_len as u32).to_be_bytes());
    frame.extend_from_slice(&(headers.len() as u32).to_be_bytes());
    let prelude_crc = event_crc32(&frame[..8]);
    frame.extend_from_slice(&prelude_crc.to_be_bytes());
    frame.extend_from_slice(&headers);
    frame.extend_from_slice(&payload);
    let message_crc = event_crc32(&frame);
    frame.extend_from_slice(&message_crc.to_be_bytes());
    frame
}

fn event_crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for byte in bytes {
        crc ^= *byte as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    !crc
}

fn push_event_string_header(out: &mut Vec<u8>, name: &str, value: &str) {
    out.push(name.len() as u8);
    out.extend_from_slice(name.as_bytes());
    out.push(7);
    out.extend_from_slice(&(value.len() as u16).to_be_bytes());
    out.extend_from_slice(value.as_bytes());
}

async fn json_body(response: Response) -> serde_json::Value {
    let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    serde_json::from_slice(&body).unwrap()
}

async fn body_text(response: Response) -> String {
    let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    String::from_utf8(body.to_vec()).unwrap()
}
