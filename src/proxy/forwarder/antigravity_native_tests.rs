// Local HTTP contracts only. These do not prove live OAuth or model rights.
fn antigravity_native_tos_payload() -> Value {
    json!({"error":{"code":403,"status":"PERMISSION_DENIED","message":"private upstream explanation",
        "details":[{"@type":"type.googleapis.com/google.rpc.ErrorInfo","reason":"TOS_VIOLATION",
            "metadata":{"appeal_url":"https://example.test/private-appeal-token"}}]}})
}

#[tokio::test]
async fn antigravity_native_precommit_rejects_empty_and_abnormal_generation_on_both_rails() {
    for rail in [ProviderType::AntigravityOAuth, ProviderType::AgyOAuth] {
        for app in [AppKind::Gemini, AppKind::Claude] {
            for streaming in [false, true] {
                for payload in [
                    json!({"usageMetadata":{"totalTokenCount":1}}),
                    json!({"candidates":[{"finishReason":"STOP"}]}),
                    json!({"candidates":[{"content":{"parts":[{"thoughtSignature":"opaque"}]},"finishReason":"STOP"}]}),
                    json!({"candidates":[{"content":{"parts":[{"text":"partial"}]},"finishReason":"MALFORMED_FUNCTION_CALL"}]}),
                    json!({"candidates":[{"finishReason":"UNEXPECTED_TOOL_CALL"}]}),
                    json!({"candidates":[{"finishReason":"MALFORMED_RESPONSE"}]}),
                ] {
                    let wire = Bytes::from(format!(
                        "data: {}\n\ndata: [DONE]\n\n",
                        json!({"response":payload})
                    ));
                    let (endpoint, calls, server) =
                        antigravity_r5_probe(wire, StatusCode::OK).await;
                    let fixture = format!("native-empty-{rail:?}-{app:?}-{streaming}");
                    let state = forwarder_test_state(&fixture);
                    let (id, _) =
                        install_antigravity_r5_provider(&state, &fixture, app, rail, endpoint)
                            .await;
                    let (route, path, body) = antigravity_timing_input(app, streaming);
                    let error =
                        forward_for_test_surface(state, route, id, path, HeaderMap::new(), body)
                            .await
                            .unwrap_err();
                    assert_eq!(
                        error.status,
                        StatusCode::BAD_GATEWAY,
                        "{fixture}: {error:?}"
                    );
                    assert_eq!(
                        calls.lock().unwrap().len(),
                        1,
                        "no recovery replay for invalid generation"
                    );
                    server.abort();
                }
            }
        }
    }
}

#[tokio::test]
async fn antigravity_native_tos_is_scoped_redacted_and_blocks_without_auth_invalidation() {
    use crate::domain::accounts::antigravity_access::{
        service_blocked, AntigravityService as Service,
    };
    for rail in [ProviderType::AntigravityOAuth, ProviderType::AgyOAuth] {
        for app in [AppKind::Gemini, AppKind::Claude] {
            for streaming in [false, true] {
                for scenario in ["http", "http_200", "prelude", "midstream", "eof_tail"] {
                    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
                    let count = calls.clone();
                    let upstream = axum::Router::new().fallback(axum::routing::post(move || {
                        count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        async move {
                            let error = antigravity_native_tos_payload();
                            if scenario == "http" || scenario == "http_200" {
                                return Response::builder().status(if scenario == "http_200" { StatusCode::OK } else { StatusCode::FORBIDDEN }).header(CONTENT_TYPE,"application/json")
                                    .body(Body::from(error.to_string())).unwrap();
                            }
                            let mut chunks = Vec::new();
                            if scenario == "midstream" {
                                chunks.push(Bytes::from_static(b"data: {\"response\":{\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"partial\"}]}}]}}\n\n"));
                            }
                            chunks.push(Bytes::from(format!("data: {}{}", json!({"response":error}), if scenario == "eof_tail" { "" } else { "\n\n" })));
                            let body = futures_util::stream::iter(chunks).then(|chunk| async move {
                                tokio::time::sleep(Duration::from_millis(20)).await;
                                Ok::<_, std::convert::Infallible>(chunk)
                            });
                            Response::builder().status(StatusCode::OK).header(CONTENT_TYPE,"text/event-stream").body(Body::from_stream(body)).unwrap()
                        }
                    }));
                    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                    let endpoint = format!("http://{}", listener.local_addr().unwrap());
                    let server = tokio::spawn(async move {
                        axum::serve(listener, upstream).await.unwrap();
                    });
                    let fixture = format!("native-tos-{rail:?}-{app:?}-{streaming}-{scenario}");
                    let state = forwarder_test_state(&fixture);
                    let (id, account_id) =
                        install_antigravity_r5_provider(&state, &fixture, app, rail, endpoint)
                            .await;
                    let (route, path, body) = antigravity_timing_input(app, streaming);
                    let result = forward_for_test_surface(
                        state.clone(),
                        route,
                        id.clone(),
                        path.clone(),
                        HeaderMap::new(),
                        body.clone(),
                    )
                    .await;
                    match result {
                        Ok(response) => {
                            let status = response.status();
                            let text =
                                String::from_utf8(collect_response_body(response).await.to_vec())
                                    .unwrap();
                            assert!(
                                status == StatusCode::FORBIDDEN
                                    || text.contains("cc_switch_stream_error"),
                                "{fixture}: {text}"
                            );
                            if scenario == "http" || scenario == "http_200" {
                                assert_eq!(status, StatusCode::FORBIDDEN, "{fixture}");
                            }
                            assert!(!text.contains("event: message_stop"), "{fixture}: {text}");
                            assert!(
                                !text.contains("\"finishReason\":\"STOP\""),
                                "{fixture}: {text}"
                            );
                            assert!(!text.contains("private-appeal-token"), "{fixture}: {text}");
                            assert!(
                                !text.contains("private upstream explanation"),
                                "{fixture}: {text}"
                            );
                        }
                        Err(error) => {
                            assert_eq!(error.status, StatusCode::FORBIDDEN, "{fixture}: {error:?}");
                        }
                    }
                    let account = state.find_account_by_id(&account_id).await.unwrap();
                    assert!(service_blocked(&account, Service::Inference), "{fixture}");
                    assert!(!service_blocked(&account, Service::CountTokens));
                    assert!(!account.needs_relogin);
                    assert_eq!(account.access_token.as_deref(), Some("antigravity-access"));
                    let persisted = crate::domain::accounts::store::AccountStore::load_or_default(
                        &state.config_dir,
                    )
                    .unwrap();
                    let persisted = persisted
                        .find_for_provider(rail, Some(&account_id))
                        .unwrap();
                    assert!(service_blocked(persisted, Service::Inference));
                    assert!(!serde_json::to_string(persisted)
                        .unwrap()
                        .contains("private-appeal-token"));
                    // A service block is checked before any proactive OAuth
                    // refresh, including when the stored token has expired.
                    state
                        .mutate_accounts_immediate(|accounts| {
                            accounts
                                .accounts
                                .iter_mut()
                                .find(|account| account.id == account_id)
                                .unwrap()
                                .expires_at = Some(0);
                        })
                        .await
                        .unwrap();
                    let error = forward_for_test_surface(
                        state.clone(),
                        route,
                        id,
                        path,
                        HeaderMap::new(),
                        body,
                    )
                    .await
                    .unwrap_err();
                    assert_eq!(error.status, StatusCode::FORBIDDEN);
                    assert_eq!(
                        calls.load(std::sync::atomic::Ordering::SeqCst),
                        1,
                        "blocked request must not send or refresh"
                    );
                    server.abort();
                }
            }
        }
    }
}

#[tokio::test]
async fn antigravity_tos_after_proactive_refresh_uses_the_outbound_token_generation() {
    use crate::domain::accounts::antigravity_access::{
        service_blocked, AntigravityService as Service,
    };
    for rail in [ProviderType::AntigravityOAuth, ProviderType::AgyOAuth] {
        for app in [AppKind::Gemini, AppKind::Claude] {
            let requests = Arc::new(StdMutex::new(Vec::new()));
            let observed = requests.clone();
            let upstream = axum::Router::new().fallback(axum::routing::post(
                move |uri: axum::http::Uri, headers: HeaderMap| {
                    observed
                        .lock()
                        .unwrap()
                        .push((uri.path().to_string(), headers));
                    async move {
                        if uri.path() == "/token" {
                            return (
                                StatusCode::OK,
                                axum::Json(json!({
                                    "access_token":"refreshed-antigravity-access",
                                    "token_type":"Bearer", "expires_in":3600
                                })),
                            );
                        }
                        assert_eq!(uri.path(), "/v1internal:streamGenerateContent");
                        (
                            StatusCode::FORBIDDEN,
                            axum::Json(antigravity_native_tos_payload()),
                        )
                    }
                },
            ));
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let endpoint = format!("http://{}", listener.local_addr().unwrap());
            let server = tokio::spawn(async move {
                axum::serve(listener, upstream).await.unwrap();
            });
            let fixture = format!("tos-proactive-refresh-{rail:?}-{app:?}");
            let state = forwarder_test_state(&fixture);
            let (id, account_id) =
                install_antigravity_r5_provider(&state, &fixture, app, rail, endpoint.clone())
                    .await;
            let before = state
                .mutate_accounts_immediate(|accounts| {
                    let account = accounts
                        .accounts
                        .iter_mut()
                        .find(|account| account.id == account_id)
                        .unwrap();
                    account.expires_at = Some(0);
                    account.refresh_token = Some("fixture-refresh-token".to_string());
                    account.raw = Some(json!({
                        "clientId":"fixture-google-client", "clientSecret":"fixture-google-secret",
                        "testOAuthTokenUrl":format!("{endpoint}/token")
                    }));
                    account.clone()
                })
                .await
                .unwrap();
            let (route, path, body) = antigravity_timing_input(app, false);
            let response =
                forward_for_test_surface(state.clone(), route, id, path, HeaderMap::new(), body)
                    .await
                    .unwrap();
            assert_eq!(response.status(), StatusCode::FORBIDDEN, "{fixture}");
            let _ = collect_response_body(response).await;
            let account = state.find_account_by_id(&account_id).await.unwrap();
            assert_eq!(
                account.auth_identity_generation,
                before.auth_identity_generation
            );
            assert!(account.token_refresh_generation > before.token_refresh_generation);
            assert_eq!(
                account.access_token.as_deref(),
                Some("refreshed-antigravity-access")
            );
            assert!(service_blocked(&account, Service::Inference), "{fixture}");
            assert!(!service_blocked(&account, Service::CountTokens));
            assert!(!account.needs_relogin);
            let persisted =
                crate::domain::accounts::store::AccountStore::load_or_default(&state.config_dir)
                    .unwrap();
            assert!(service_blocked(
                persisted
                    .find_for_provider(rail, Some(&account_id))
                    .unwrap(),
                Service::Inference,
            ));
            let requests = requests.lock().unwrap();
            assert_eq!(requests.len(), 2, "{fixture}: no extra refresh or replay");
            assert_eq!(requests[0].0, "/token");
            assert_eq!(
                requests[1].1[axum::http::header::AUTHORIZATION],
                "Bearer refreshed-antigravity-access"
            );
            drop(requests);
            server.abort();
        }
    }
}

#[tokio::test]
async fn antigravity_semantic_admission_deadline_is_not_extended_by_usage_heartbeats() {
    for rail in [ProviderType::AntigravityOAuth, ProviderType::AgyOAuth] {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observed = calls.clone();
        let upstream = axum::Router::new().fallback(axum::routing::post(move || {
            observed.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            async {
                let chunks = futures_util::stream::repeat_with(|| ()).then(|_| async {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                    Ok::<_, std::convert::Infallible>(Bytes::from_static(
                        b"data: {\"response\":{\"usageMetadata\":{\"totalTokenCount\":1}}}\n\n",
                    ))
                });
                Response::builder()
                    .status(StatusCode::OK)
                    .header(CONTENT_TYPE, "text/event-stream")
                    .body(Body::from_stream(chunks))
                    .unwrap()
            }
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, upstream).await.unwrap();
        });
        let fixture = format!("semantic-deadline-{rail:?}");
        let state = forwarder_test_state(&fixture);
        let (id, _) =
            install_antigravity_r5_provider(&state, &fixture, AppKind::Gemini, rail, endpoint)
                .await;
        let mut providers = state.providers.read().await.clone();
        let mut plan = providers
            .runtime_plan(AppKind::Gemini, &id)
            .unwrap()
            .as_ref()
            .clone();
        plan.transport_policy.stream_first_byte_timeout_ms = Some(100);
        Arc::make_mut(&mut providers.runtime_index).insert_plan_for_test(plan);
        state.replace_provider_store_for_test(providers).await;
        let (route, path, body) = antigravity_timing_input(AppKind::Gemini, true);
        let error = tokio::time::timeout(
            Duration::from_secs(2),
            forward_for_test_surface(state, route, id, path, HeaderMap::new(), body),
        )
        .await
        .expect("usage must not indefinitely extend semantic admission")
        .unwrap_err();
        assert_eq!(error.status, StatusCode::GATEWAY_TIMEOUT);
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        server.abort();
    }
}

#[tokio::test]
async fn antigravity_native_count_tokens_tos_does_not_block_inference() {
    use crate::domain::accounts::antigravity_access::{
        service_blocked, AntigravityService as Service,
    };
    for rail in [ProviderType::AntigravityOAuth, ProviderType::AgyOAuth] {
        let upstream = axum::Router::new().fallback(axum::routing::post(|| async {
            (
                StatusCode::FORBIDDEN,
                axum::Json(antigravity_native_tos_payload()),
            )
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, upstream).await.unwrap();
        });
        let fixture = format!("native-count-tos-{rail:?}");
        let state = forwarder_test_state(&fixture);
        let (id, account_id, _) = install_antigravity_test_provider(
            &state,
            &fixture,
            AppKind::Gemini,
            rail,
            endpoint.clone(),
        )
        .await;
        state
            .mutate_accounts_immediate(|accounts| {
                accounts
                    .accounts
                    .iter_mut()
                    .find(|a| a.id == account_id)
                    .unwrap()
                    .raw = Some(json!({"testGeminiAiStudioBaseUrl":endpoint}));
            })
            .await
            .unwrap();
        let response = forward_for_test_surface(
            state.clone(),
            ProxyRoute::Gemini,
            id,
            Some("models/gemini-3.5-flash-medium:countTokens".into()),
            HeaderMap::new(),
            Bytes::from_static(br#"{"contents":[{"parts":[{"text":"count"}]}]}"#),
        )
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let body = collect_response_body(response).await;
        assert!(!String::from_utf8_lossy(&body).contains("private-appeal-token"));
        let account = state.find_account_by_id(&account_id).await.unwrap();
        assert!(service_blocked(&account, Service::CountTokens));
        assert!(!service_blocked(&account, Service::Inference));
        server.abort();
    }
}
