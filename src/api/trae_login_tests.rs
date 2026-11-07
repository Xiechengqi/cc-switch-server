// Exercise the real app router, invoke bridge, OAuth transport and account writer.
async fn trae_test_request(
    state: &ServerState,
    method: &str,
    path: &str,
    body: Option<Value>,
    authenticated: bool,
) -> (StatusCode, HeaderMap, String) {
    use tower::ServiceExt;
    let mut request = axum::http::Request::builder()
        .method(method)
        .uri(path)
        .header("host", "127.0.0.1:9911")
        .header("content-type", "application/json");
    if authenticated {
        request = request.header("authorization", "Bearer trae-test-admin");
    }
    let response = crate::api::app_router(state.clone())
        .oneshot(
            request
                .body(axum::body::Body::from(
                    body.map(|value| value.to_string()).unwrap_or_default(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = axum::body::to_bytes(response.into_body(), 256 * 1024)
        .await
        .unwrap();
    (status, headers, String::from_utf8(bytes.to_vec()).unwrap())
}

async fn trae_test_oauth_server() -> (
    String,
    std::sync::Arc<std::sync::atomic::AtomicUsize>,
    tokio::task::JoinHandle<()>,
) {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    let attempts = Arc::new(AtomicUsize::new(0));
    let counter = attempts.clone();
    let router = axum::Router::new().route("/trae/api/v3/oauth/ExchangeToken", axum::routing::post(move |Json(body): Json<Value>| {
        let counter = counter.clone();
        async move {
            counter.fetch_add(1, Ordering::SeqCst);
            assert_eq!(body["ClientID"], "ono9krqynydwx5");
            assert_eq!(body["AuthCode"], "callback-test-code");
            assert_eq!(body["CodeVerifier"].as_str().unwrap().len(), 43);
            assert!(body["DeviceInfo"]["DevicePublicKey"].as_str().unwrap().contains("BEGIN PUBLIC KEY"));
            Json(json!({"Result":{"Token":"test-access-secret","RefreshToken":"test-refresh-secret","TokenExpireDuration":3600}}))
        }
    })).route("/cloudide/api/v3/trae/GetUserInfo", axum::routing::post(|| async {
        Json(json!({"Result":{"UserID":"test-uid","EnterpriseID":"test-enterprise","ScreenName":"Test User"}}))
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (format!("http://{address}"), attempts, task)
}

async fn trae_test_pending_flow(
    state: &ServerState,
    provider_type: ProviderType,
    origin: &str,
) -> (String, String) {
    let now = crate::infra::time::now_ms() as i64;
    let (start, flow) = crate::clients::oauth::trae::start_test_login(
        "http://127.0.0.1:9911/authorize",
        provider_type,
        origin,
        now,
    );
    let mut callback = url::Url::parse("http://127.0.0.1:9911/authorize").unwrap();
    callback
        .query_pairs_mut()
        .append_pair("loginTraceID", &start.flow_id)
        .append_pair("scope", "trae")
        .append_pair("authCodeInfo", r#"{"AuthCode":"callback-test-code"}"#)
        .append_pair("host", "https://attacker.invalid");
    state
        .insert_trae_login_flow(start.flow_id.clone(), flow, now)
        .await;
    state
        .bind_device_flow_principal(
            provider_type,
            start.flow_id.clone(),
            "local-admin@cc-switch.local:admin".to_string(),
            start.expires_at,
            now,
        )
        .await;
    (start.flow_id, callback.into())
}

#[tokio::test]
async fn trae_http_start_and_invoke_publish_the_official_callback_and_pkce() {
    let state = account_api_test_state_with_port("trae-http-start", 9911);
    state
        .push_session(crate::state::Session {
            token: "trae-test-admin".into(),
        })
        .await;
    let config_dir = state.config_dir.clone();
    for provider in ["trae_solo", "trae_raw_chat", "trae_raw_chat_intl"] {
        let (status, _, body) = trae_test_request(
            &state,
            "POST",
            "/web-api/invoke/auth_start_login",
            Some(json!({"authProvider":provider})),
            true,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let response: Value = serde_json::from_str(&body).unwrap();
        let auth = url::Url::parse(response["verification_uri"].as_str().unwrap()).unwrap();
        let query = auth
            .query_pairs()
            .collect::<std::collections::BTreeMap<_, _>>();
        assert_eq!(
            query["auth_callback_url"],
            "http://127.0.0.1:9911/authorize"
        );
        assert_eq!(query["code_challenge_method"], "S256");
        assert_eq!(
            query["login_trace_id"],
            response["device_code"].as_str().unwrap()
        );
        assert!(!body.contains("CodeVerifier"));
    }
    drop(state);
    std::fs::remove_dir_all(config_dir).unwrap();
}

#[tokio::test]
async fn trae_manual_and_loopback_callbacks_persist_once_for_all_channels() {
    use std::sync::atomic::Ordering;
    for provider in [
        ProviderType::TraeSolo,
        ProviderType::TraeRaw,
        ProviderType::TraeIntl,
    ] {
        for manual in [false, true] {
            let state = account_api_test_state_with_port("trae-browser-roundtrip", 9911);
            state
                .push_session(crate::state::Session {
                    token: "trae-test-admin".into(),
                })
                .await;
            let config_dir = state.config_dir.clone();
            let (origin, attempts, task) = trae_test_oauth_server().await;
            let (flow_id, callback) = trae_test_pending_flow(&state, provider, &origin).await;
            if manual {
                // Remote browser cannot connect to loopback; the originating UI
                // submits the full URL through its authenticated invoke command.
                let (status, _, body) = trae_test_request(&state, "POST", "/web-api/invoke/auth_submit_oauth_code", Some(json!({"authProvider":provider.as_str(),"deviceCode":flow_id,"callbackUrl":callback})), true).await;
                assert_eq!(status, StatusCode::OK, "{body}");
                assert!(!body.contains("test-access-secret"));
                assert!(!body.contains("PRIVATE KEY"));
            } else {
                let path = callback.strip_prefix("http://127.0.0.1:9911").unwrap();
                let (a, b) = tokio::join!(
                    trae_test_request(&state, "GET", path, None, false),
                    trae_test_request(&state, "GET", path, None, false)
                );
                assert_eq!(
                    usize::from(a.2.contains("OAuth completed"))
                        + usize::from(b.2.contains("OAuth completed")),
                    1
                );
                assert_eq!(a.1["cache-control"], "no-store");
                assert_eq!(a.1["referrer-policy"], "no-referrer");
                assert!(a.1["content-security-policy"]
                    .to_str()
                    .unwrap()
                    .contains("frame-ancestors 'none'"));
                assert!(a.2.contains("history.replaceState"));
                assert!(!a.2.contains("callback-test-code"));
                assert!(state.accounts_snapshot().await.accounts.is_empty());
                let (status, _, body) = trae_test_request(
                    &state,
                    "POST",
                    "/api/accounts/trae/login/status",
                    Some(json!({"flowId":flow_id})),
                    true,
                )
                .await;
                assert_eq!(status, StatusCode::OK, "{body}");
                assert_eq!(
                    serde_json::from_str::<Value>(&body).unwrap()["pending"],
                    false
                );
            }
            let accounts = state.accounts_snapshot().await;
            assert_eq!(accounts.accounts.len(), 1);
            assert_eq!(accounts.accounts[0].provider_type, provider);
            assert_eq!(
                accounts.accounts[0].raw.as_ref().unwrap()["oauthClientId"],
                "ono9krqynydwx5"
            );
            assert!(
                accounts.accounts[0].raw.as_ref().unwrap()["devicePrivateKeyPem"]
                    .as_str()
                    .unwrap()
                    .contains("PRIVATE KEY")
            );
            assert!(state
                .trae_login_status(&flow_id, crate::infra::time::now_ms() as i64)
                .await
                .is_none());
            let (_, _, replay) = trae_test_request(
                &state,
                "GET",
                callback.strip_prefix("http://127.0.0.1:9911").unwrap(),
                None,
                false,
            )
            .await;
            assert!(replay.contains("OAuth failed"));
            assert_eq!(attempts.load(Ordering::SeqCst), 1);
            task.abort();
            drop(state);
            std::fs::remove_dir_all(config_dir).unwrap();
        }
    }
}

#[tokio::test]
async fn trae_wrong_session_provider_owner_and_cancel_never_redeem_the_code() {
    use std::sync::atomic::Ordering;
    let state = account_api_test_state_with_port("trae-rejected-callbacks", 9911);
    state
        .push_session(crate::state::Session {
            token: "trae-test-admin".into(),
        })
        .await;
    let config_dir = state.config_dir.clone();
    let (origin, attempts, task) = trae_test_oauth_server().await;
    let (flow_id, callback) = trae_test_pending_flow(&state, ProviderType::TraeSolo, &origin).await;
    for body in [
        json!({"authProvider":"trae_solo","deviceCode":"different-flow","callbackUrl":callback}),
        json!({"authProvider":"trae_raw_chat","deviceCode":flow_id,"callbackUrl":callback}),
    ] {
        let (status, _, _) = trae_test_request(
            &state,
            "POST",
            "/web-api/invoke/auth_submit_oauth_code",
            Some(body),
            true,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
    let (status, _, _) = trae_test_request(
        &state,
        "POST",
        "/api/accounts/trae/login/complete",
        Some(json!({"flowId":flow_id,"callbackUrl":callback})),
        false,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    // Binding a different principal must deny this admin without consuming state.
    state
        .bind_device_flow_principal(
            ProviderType::TraeSolo,
            flow_id.clone(),
            "other@example.com:admin".into(),
            i64::MAX,
            0,
        )
        .await;
    let (status, _, _) = trae_test_request(
        &state,
        "POST",
        "/api/accounts/trae/login/complete",
        Some(json!({"flowId":flow_id,"callbackUrl":callback})),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    state
        .bind_device_flow_principal(
            ProviderType::TraeSolo,
            flow_id.clone(),
            "local-admin@cc-switch.local:admin".into(),
            i64::MAX,
            0,
        )
        .await;
    let (status, _, _) = trae_test_request(
        &state,
        "POST",
        "/api/accounts/trae/login/cancel",
        Some(json!({"flowId":flow_id})),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, _, body) = trae_test_request(
        &state,
        "GET",
        callback.strip_prefix("http://127.0.0.1:9911").unwrap(),
        None,
        false,
    )
    .await;
    assert!(body.contains("OAuth failed"));
    assert_eq!(attempts.load(Ordering::SeqCst), 0);
    assert!(state.accounts_snapshot().await.accounts.is_empty());
    task.abort();
    drop(state);
    std::fs::remove_dir_all(config_dir).unwrap();
}

#[tokio::test]
async fn trae_exchange_redirects_and_reflected_failures_never_retry_or_leak_codes() {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    for code in [307_u16, 502, 200] {
        let state = account_api_test_state_with_port("trae-exchange-errors", 9911);
        state
            .push_session(crate::state::Session {
                token: "trae-test-admin".into(),
            })
            .await;
        let config_dir = state.config_dir.clone();
        let attempts = Arc::new(AtomicUsize::new(0));
        let redirects = Arc::new(AtomicUsize::new(0));
        let counter = attempts.clone();
        let redirected = redirects.clone();
        let router = axum::Router::new()
            .route(
                "/trae/api/v3/oauth/ExchangeToken",
                axum::routing::post(move |Json(body): Json<Value>| {
                    let counter = counter.clone();
                    async move {
                        counter.fetch_add(1, Ordering::SeqCst);
                        (
                            StatusCode::from_u16(code).unwrap(),
                            [("location", "/unexpected")],
                            Json(json!({"Code":5001,"Message":body.to_string()})),
                        )
                    }
                }),
            )
            .fallback(axum::routing::any(move || {
                let redirected = redirected.clone();
                async move {
                    redirected.fetch_add(1, Ordering::SeqCst);
                    StatusCode::BAD_GATEWAY
                }
            }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let (flow_id, callback) =
            trae_test_pending_flow(&state, ProviderType::TraeSolo, &origin).await;
        for _ in 0..2 {
            let (status, _, body) = trae_test_request(
                &state,
                "POST",
                "/api/accounts/trae/login/complete",
                Some(json!({"flowId":flow_id,"callbackUrl":callback})),
                true,
            )
            .await;
            assert_ne!(status, StatusCode::OK);
            assert!(!body.contains("callback-test-code"));
            assert!(!body.contains("CodeVerifier"));
        }
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
        assert_eq!(redirects.load(Ordering::SeqCst), 0);
        assert!(state.accounts_snapshot().await.accounts.is_empty());
        task.abort();
        drop(state);
        std::fs::remove_dir_all(config_dir).unwrap();
    }
}

#[tokio::test]
async fn trae_cancel_during_code_exchange_discards_the_result() {
    use std::sync::Arc;
    let state = account_api_test_state_with_port("trae-cancel-inflight", 9911);
    state
        .push_session(crate::state::Session {
            token: "trae-test-admin".into(),
        })
        .await;
    let config_dir = state.config_dir.clone();
    let started = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let signal_started = started.clone();
    let wait_release = release.clone();
    let router = axum::Router::new()
        .route("/trae/api/v3/oauth/ExchangeToken", axum::routing::post(move || {
            let started = signal_started.clone();
            let release = wait_release.clone();
            async move {
                started.notify_one();
                release.notified().await;
                Json(json!({"Result":{"Token":"cancelled-access","RefreshToken":"cancelled-refresh","TokenExpireDuration":3600}}))
            }
        }))
        .route("/cloudide/api/v3/trae/GetUserInfo", axum::routing::post(|| async {
            Json(json!({"Result":{"UserID":"test-uid","EnterpriseID":"test-enterprise"}}))
        }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let (flow_id, callback) = trae_test_pending_flow(&state, ProviderType::TraeSolo, &origin).await;
    let request_state = state.clone();
    let callback_task = tokio::spawn(async move {
        trae_test_request(
            &request_state,
            "GET",
            callback.strip_prefix("http://127.0.0.1:9911").unwrap(),
            None,
            false,
        )
        .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), started.notified())
        .await
        .unwrap();
    let (status, _, _) = trae_test_request(
        &state,
        "POST",
        "/api/accounts/trae/login/cancel",
        Some(json!({"flowId":flow_id})),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    release.notify_one();
    let (_, _, html) = callback_task.await.unwrap();
    assert!(html.contains("OAuth failed"));
    assert!(state.accounts_snapshot().await.accounts.is_empty());
    assert!(state
        .trae_login_status(&flow_id, crate::infra::time::now_ms() as i64)
        .await
        .is_none());
    server.abort();
    drop(state);
    std::fs::remove_dir_all(config_dir).unwrap();
}
