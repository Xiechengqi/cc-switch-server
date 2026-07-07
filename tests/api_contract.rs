// Integration tests for the HTTP API contract.
// Extracted from src/api/mod.rs as part of R3.6/R3.7.

use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::body::{to_bytes, Body};
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
use cc_switch_server::api::{control_signature, refresh_share_usage_items};
use cc_switch_server::cli::Cli;
use cc_switch_server::domain::accounts::store::{AccountQuota, UpsertAccountInput};
use cc_switch_server::domain::providers::model::{
    AppKind, AuthBinding, Provider, ProviderMeta, ProviderType,
};
use cc_switch_server::domain::sharing::shares::{ShareBinding, UpsertShareInput};
use cc_switch_server::state::{ServerState, ServerStateInner};

#[tokio::test]
async fn share_router_health_is_hidden_without_probe_header() {
    let state = test_state();
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
    assert_eq!(response.status(), StatusCode::OK);
    let body = json_body(response).await;
    assert_eq!(body["ok"].as_bool(), Some(true));
    assert_eq!(body["status"].as_str(), Some("healthy"));
}

#[tokio::test]
async fn control_apply_share_settings_rejects_replayed_nonce() {
    let state = test_state();
    state.config.write().await.router.identity =
        Some(cc_switch_server::domain::settings::config::RouterIdentity {
            installation_id: "inst-ctl".to_string(),
            public_key: "public-key".to_string(),
            private_key: "private-key".to_string(),
            control_secret: Some("control-secret".to_string()),
        });
    state.providers.write().await.upsert(
        AppKind::Codex,
        Provider {
            id: "p-ctl".to_string(),
            name: "Control Provider".to_string(),
            settings_config: json!({"env": {"OPENAI_API_KEY": "sk-test"}}),
            category: None,
            meta: None,
            extra: Default::default(),
        },
    );
    state
        .mutate_shares_immediate(|store| {
            store.upsert(test_share_input("share-ctl", "p-ctl", ProviderType::Codex));
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
    state.providers.write().await.upsert(
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
    );
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
                quota_percent: None,
                quota: None,
                quota_refreshed_at: None,
                quota_next_refresh_at: None,
                expires_at: None,
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
        .unwrap();
    let providers = state.providers.read().await.clone();

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

    state.providers.write().await.upsert(
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
    );

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

    state.providers.write().await.upsert(
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
    );

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

    state.providers.write().await.upsert(
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
    );

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
    state.providers.write().await.upsert(
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
    );

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

    let usage = state.usage.read().await;
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
    state.providers.write().await.upsert(
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
    );
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
                quota: None,
                quota_percent: None,
                quota_refreshed_at: None,
                quota_next_refresh_at: None,
                expires_at: Some(4_102_444_800_000),
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
    state.providers.write().await.upsert(
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
    );

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
async fn stream_proxy_marks_upstream_chunk_error() {
    let upstream_addr = spawn_broken_chunked_upstream().await;
    let state = test_state();
    let app = app_router(state.clone());
    state.providers.write().await.upsert(
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
    );

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
        let usage = state.usage.read().await;
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
        scopes: vec!["openid".to_string(), "profile".to_string()],
        profile: Some(json!({"plan":"pro"})),
        raw: Some(json!({"source":"mock"})),
        subscription_level: Some("pro".to_string()),
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
    let mut config = state.config.read().await.clone();
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
    assert!(state.config.read().await.client.last_heartbeat_ms.is_some());
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
    let mut config = state.config.read().await.clone();
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
    let mut config = state.config.read().await.clone();
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
    let mut config = state.config.read().await.clone();
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
async fn router_batch_sync_notifies_runtime_refresh_after_remote_sync() {
    let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let router_addr = listener.local_addr().unwrap();
    let batch_seen = Arc::new(AtomicUsize::new(0));
    let refresh_seen = Arc::new(AtomicUsize::new(0));
    let router = Router::new()
        .route(
            "/v1/shares/batch-sync",
            post(
                |State((batch_seen, _refresh_seen)): State<(
                    Arc<AtomicUsize>,
                    Arc<AtomicUsize>,
                )>,
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
        .route(
            "/v1/shares/runtime-refresh",
            post(
                |State((_batch_seen, refresh_seen)): State<(
                    Arc<AtomicUsize>,
                    Arc<AtomicUsize>,
                )>,
                 axum::Json(body): axum::Json<Value>| async move {
                    assert_eq!(body["installationId"].as_str(), Some("inst-runtime"));
                    assert_eq!(body["refresh"]["shareId"].as_str(), Some("share-runtime"));
                    assert_eq!(body["refresh"]["subdomain"].as_str(), Some("runtime-sub"));
                    refresh_seen.fetch_add(1, Ordering::SeqCst);
                    axum::Json(json!({"ok": true}))
                },
            ),
        )
        .with_state((batch_seen.clone(), refresh_seen.clone()));
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });

    let state = test_state();
    let app = app_router(state.clone());
    let token = setup_and_login(&app).await;
    let mut config = state.config.read().await.clone();
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
            store.upsert(share);
        })
        .await
        .unwrap();

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/router/batch-sync",
            json!(null),
            Some(&token),
        ))
        .await
        .unwrap();

    let status = response.status();
    let body = json_body(response).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["remoteSynced"].as_bool(), Some(true));
    assert_eq!(batch_seen.load(Ordering::SeqCst), 1);
    assert_eq!(refresh_seen.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn router_batch_sync_records_runtime_refresh_failure_without_failing_local_response() {
    let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let router_addr = listener.local_addr().unwrap();
    let refresh_seen = Arc::new(AtomicUsize::new(0));
    let router = Router::new()
        .route(
            "/v1/shares/batch-sync",
            post(|| async { axum::Json(json!({"ok": true})) }),
        )
        .route(
            "/v1/shares/runtime-refresh",
            post(
                |State(refresh_seen): State<Arc<AtomicUsize>>,
                 axum::Json(body): axum::Json<Value>| async move {
                    assert_eq!(
                        body["refresh"]["shareId"].as_str(),
                        Some("share-runtime-fail")
                    );
                    refresh_seen.fetch_add(1, Ordering::SeqCst);
                    (StatusCode::SERVICE_UNAVAILABLE, "refresh unavailable")
                },
            ),
        )
        .with_state(refresh_seen.clone());
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });

    let state = test_state();
    let app = app_router(state.clone());
    let token = setup_and_login(&app).await;
    let mut config = state.config.read().await.clone();
    config.router.url = Some(format!("http://{router_addr}"));
    config.router.identity = Some(cc_switch_server::domain::settings::config::RouterIdentity {
        installation_id: "inst-runtime-fail".to_string(),
        public_key: BASE64_STANDARD.encode([8_u8; 32]),
        private_key: BASE64_STANDARD.encode([7_u8; 32]),
        control_secret: Some("control-secret".to_string()),
    });
    state.replace_config(config).await.unwrap();
    let mut share = test_share_input(
        "share-runtime-fail",
        "provider-runtime",
        ProviderType::Codex,
    );
    share.tunnel_subdomain = Some("runtime-fail".to_string());
    state
        .mutate_shares_immediate(|store| {
            store.upsert(share);
        })
        .await
        .unwrap();

    let response = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/router/batch-sync",
            json!(null),
            Some(&token),
        ))
        .await
        .unwrap();

    let status = response.status();
    let body = json_body(response).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["remoteSynced"].as_bool(), Some(false));
    assert!(body["message"]
        .as_str()
        .unwrap()
        .contains("runtime refresh failed"));
    assert_eq!(refresh_seen.load(Ordering::SeqCst), 1);

    let status = app
        .oneshot(json_request(
            Method::GET,
            "/api/router/status",
            json!(null),
            Some(&token),
        ))
        .await
        .unwrap();
    let body = json_body(status).await;
    assert!(body["lastError"]
        .as_str()
        .unwrap()
        .contains("runtime refresh failed"));
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
async fn web_invoke_registry_returns_stable_errors() {
    let app = app_router(test_state());
    let token = setup_and_login(&app).await;

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
async fn web_invoke_get_providers_returns_desktop_record_shape() {
    let state = test_state();
    state.providers.write().await.upsert(
        AppKind::Codex,
        Provider {
            id: "codex-web".to_string(),
            name: "Codex Web".to_string(),
            settings_config: json!({"env": {"OPENAI_API_KEY": "sk-test"}}),
            category: None,
            meta: None,
            extra: Default::default(),
        },
    );
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
}

fn test_state() -> ServerState {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let config_dir = std::env::temp_dir().join(format!("cc-switch-server-http-test-{nanos}"));
    ServerStateInner::load(Cli {
        host: IpAddr::V4(Ipv4Addr::LOCALHOST),
        port: 0,
        config_dir: Some(config_dir),
        web_dist_dir: None,
        log_level: "warn".to_string(),
        command: None,
    })
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
    let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buffer = [0_u8; 2048];
        let _ = socket.read(&mut buffer).await;
        socket
            .write_all(
                b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\n\r\n5\r\nhello\r\nZZ\r\n",
            )
            .await
            .unwrap();
        let _ = socket.shutdown().await;
    });
    addr
}

#[tokio::test]
async fn web_invoke_direct_owner_update_requires_verified_target_owner() {
    let state = test_state();
    let app = app_router(state.clone());
    let token = setup_and_login(&app).await;
    state
        .mutate_shares_immediate(|store| {
            store.upsert(test_share_input(
                "share-owner-gate",
                "p-owner",
                ProviderType::Codex,
            ));
        })
        .await
        .unwrap();

    let response = app
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

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let body = json_body(response).await;
    assert!(body["error"]
        .as_str()
        .unwrap()
        .contains("email_auth_change_owner_email"));
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
    let mut config = state.config.read().await.clone();
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
            store.upsert(test_share_input(
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
        state.config.read().await.owner.email.as_deref(),
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

async fn json_body(response: Response) -> serde_json::Value {
    let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    serde_json::from_slice(&body).unwrap()
}

async fn body_text(response: Response) -> String {
    let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    String::from_utf8(body.to_vec()).unwrap()
}
