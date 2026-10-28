// CORE-03 fixtures use only existing cfg(test) loopback endpoint overrides.
// They do not demonstrate acceptance by either real Antigravity rail.

const ANTIGRAVITY_TIMING_CONTENT: &str = concat!(
    ": keepalive\n\n",
    "data: {\"response\":{\"usageMetadata\":{\"promptTokenCount\":1,\"candidatesTokenCount\":0}}}\n\n",
    "data: {\"response\":{\"candidates\":[{\"index\":0,\"content\":{\"role\":\"model\",\"parts\":[{\"thought\":true,\"text\":\"\",\"thoughtSignature\":\"opaque-timing-carrier\"}]}}]}}\n\n",
    "data: {\"response\":{\"candidates\":[{\"index\":0,\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"hello\"}]}}]}}\n\n",
);
const ANTIGRAVITY_TIMING_TERMINAL: &str = "data: {\"response\":{\"candidates\":[{\"index\":0,\"finishReason\":\"STOP\"}],\"usageMetadata\":{\"promptTokenCount\":1,\"candidatesTokenCount\":1,\"totalTokenCount\":2}}}\n\n";
const ANTIGRAVITY_TIMING_ERROR: &str = "data: {\"response\":{\"error\":{\"code\":503,\"status\":\"UNAVAILABLE\",\"message\":\"fixture failure\"}}}\n\n";

fn antigravity_timing_input(app: AppKind, streaming: bool) -> (ProxyRoute, Option<String>, Bytes) {
    match app {
        AppKind::Claude => (
            ProxyRoute::ClaudeMessages,
            None,
            Bytes::from(json!({"model":"claude-sonnet-4-6","max_tokens":64,"stream":streaming,"messages":[{"role":"user","content":"hi"}]}).to_string()),
        ),
        AppKind::Gemini => (
            ProxyRoute::Gemini,
            Some(format!("models/gemini-3.5-flash-medium:{}", if streaming {"streamGenerateContent"} else {"generateContent"})),
            Bytes::from(json!({"contents":[{"role":"user","parts":[{"text":"hi"}]}],"stream":streaming}).to_string()),
        ),
        _ => panic!("Antigravity has no declared Codex App Surface"),
    }
}

async fn antigravity_timing_attempt(
    state: &ServerState,
    route: ProxyRoute,
    id: &str,
) -> ForwardAttemptContext {
    let providers = state.providers_snapshot().await;
    let accounts = state.accounts_snapshot().await;
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

#[tokio::test]
async fn antigravity_core03_both_rails_cover_declared_surfaces_and_zero_count() {
    let requests = Arc::new(AtomicUsize::new(0));
    let handler_requests = requests.clone();
    let app = axum::Router::new().fallback(axum::routing::post(
        move |uri: axum::http::Uri, headers: HeaderMap| {
            let requests = handler_requests.clone();
            async move {
                requests.fetch_add(1, Ordering::SeqCst);
                assert_eq!(
                    headers[axum::http::header::AUTHORIZATION],
                    "Bearer antigravity-access"
                );
                if uri.path().ends_with(":countTokens") {
                    return (
                        [(CONTENT_TYPE, "application/json")],
                        "{\"totalTokens\":0}".to_string(),
                    )
                        .into_response();
                }
                (
                    [(CONTENT_TYPE, "text/event-stream")],
                    format!("{ANTIGRAVITY_TIMING_CONTENT}{ANTIGRAVITY_TIMING_TERMINAL}"),
                )
                    .into_response()
            }
        },
    ));
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    for provider_type in [ProviderType::AntigravityOAuth, ProviderType::AgyOAuth] {
        for surface in [AppKind::Claude, AppKind::Gemini] {
            let fixture = format!("core03-surfaces-{}-{surface:?}", provider_type.as_str());
            let state = forwarder_test_state(&fixture);
            let (id, account_id, _) = install_antigravity_test_provider(
                &state,
                &fixture,
                surface,
                provider_type,
                format!("http://{address}"),
            )
            .await;
            for streaming in [false, true] {
                let (route, path, body) = antigravity_timing_input(surface, streaming);
                let attempt = antigravity_timing_attempt(&state, route, &id).await;
                let slot = attempt.timing.clone();
                let response = forward_with_attempt(
                    state.clone(),
                    route,
                    path,
                    HeaderMap::new(),
                    body,
                    attempt,
                )
                .await
                .unwrap();
                assert_eq!(response.status(), StatusCode::OK);
                let timing = slot.get().unwrap().request();
                let before = timing.snapshot();
                assert_eq!(before["providerType"], provider_type.as_str());
                for phase in ["admission", "send_started", "headers"] {
                    assert!(before["milestonesMs"][phase].is_number(), "{before}");
                }
                assert!(
                    before["milestonesMs"]["http_body_handoff"].is_null(),
                    "{before}"
                );
                let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
                    .await
                    .unwrap();
                assert!(String::from_utf8_lossy(&bytes).contains("hello"));
                drop(bytes);
                let after = timing.snapshot();
                assert_eq!(
                    after["result"], "complete",
                    "{surface:?} stream={streaming}: {after}"
                );
                for phase in [
                    "first_semantic",
                    "http_body_handoff",
                    "terminal",
                    "owner_released",
                ] {
                    assert!(after["milestonesMs"][phase].is_number(), "{phase}: {after}");
                }
                assert!(after["milestonesMs"]["downstream_write_completed"].is_null());
            }
            if surface == AppKind::Gemini {
                state
                    .mutate_accounts_immediate(move |accounts| {
                        let account = accounts
                            .accounts
                            .iter_mut()
                            .find(|account| {
                                account.id == account_id && account.provider_type == provider_type
                            })
                            .unwrap();
                        account.raw =
                            Some(json!({"testGeminiAiStudioBaseUrl":format!("http://{address}")}));
                    })
                    .await
                    .unwrap();
                let attempt = antigravity_timing_attempt(&state, ProxyRoute::Gemini, &id).await;
                let slot = attempt.timing.clone();
                let response = forward_with_attempt(
                    state,
                    ProxyRoute::Gemini,
                    Some("models/gemini-3.5-flash-medium:countTokens".to_string()),
                    HeaderMap::new(),
                    Bytes::from_static(
                        br#"{"contents":[{"role":"user","parts":[{"text":"count"}]}]}"#,
                    ),
                    attempt,
                )
                .await
                .unwrap();
                assert_eq!(response.status(), StatusCode::OK);
                let bytes = axum::body::to_bytes(response.into_body(), 1024)
                    .await
                    .unwrap();
                assert_eq!(
                    serde_json::from_slice::<Value>(&bytes).unwrap()["totalTokens"],
                    0
                );
                drop(bytes);
                let after = slot.get().unwrap().request().snapshot();
                assert_eq!(after["surface"], "gemini_count_tokens");
                assert_eq!(after["result"], "complete", "{after}");
                assert!(after["milestonesMs"]["first_semantic"].is_number());
                assert!(after["milestonesMs"]["owner_released"].is_number());
            }
        }
    }
    assert_eq!(requests.load(Ordering::SeqCst), 10);
    server.abort();
}

#[tokio::test]
async fn antigravity_core03_aggregation_observes_content_before_eof_and_cancellation() {
    for provider_type in [ProviderType::AntigravityOAuth, ProviderType::AgyOAuth] {
        let app = axum::Router::new().fallback(axum::routing::post(|| async {
            let content = stream::once(async {
                Ok::<_, std::convert::Infallible>(Bytes::from_static(
                    ANTIGRAVITY_TIMING_CONTENT.as_bytes(),
                ))
            });
            Response::builder()
                .header(CONTENT_TYPE, "text/event-stream")
                .body(Body::from_stream(content.chain(stream::pending())))
                .unwrap()
        }));
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let fixture = format!("core03-aggregation-cancel-{}", provider_type.as_str());
        let state = forwarder_test_state(&fixture);
        let (id, _, _) = install_antigravity_test_provider(
            &state,
            &fixture,
            AppKind::Gemini,
            provider_type,
            format!("http://{address}"),
        )
        .await;
        let (route, path, body) = antigravity_timing_input(AppKind::Gemini, false);
        let attempt = antigravity_timing_attempt(&state, route, &id).await;
        let slot = attempt.timing.clone();
        let mut forwarding = Box::pin(forward_with_attempt(
            state,
            route,
            path,
            HeaderMap::new(),
            body,
            attempt,
        ));
        let observed = async {
            loop {
                if slot.get().is_some_and(|timing| {
                    timing.request().snapshot()["milestonesMs"]["first_semantic"].is_number()
                }) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        };
        tokio::select! {
            _ = observed => {},
            result = &mut forwarding => panic!("aggregation must wait for EOF: {result:?}"),
            _ = tokio::time::sleep(Duration::from_secs(5)) => panic!("first semantic observation was postponed until EOF"),
        }
        let timing = slot.get().unwrap().request();
        let before = timing.snapshot();
        for phase in ["terminal", "http_body_handoff", "owner_released"] {
            assert!(before["milestonesMs"][phase].is_null(), "{before}");
        }
        drop(forwarding);
        let after = timing.snapshot();
        assert_eq!(after["result"], "cancelled", "{after}");
        assert!(
            after["milestonesMs"]["owner_released"].is_number(),
            "{after}"
        );
        assert!(after["waitsMs"]["upstream_read"].as_f64().unwrap() > 0.0);
        server.abort();
    }
}

#[tokio::test]
async fn antigravity_core03_header_cancellation_precedes_owner_release() {
    for provider_type in [ProviderType::AntigravityOAuth, ProviderType::AgyOAuth] {
        let entered = Arc::new(tokio::sync::Notify::new());
        let handler_entered = entered.clone();
        let app = axum::Router::new().fallback(axum::routing::post(move || {
            let entered = handler_entered.clone();
            async move {
                entered.notify_one();
                std::future::pending::<StatusCode>().await
            }
        }));
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let fixture = format!("core03-headers-cancel-{}", provider_type.as_str());
        let state = forwarder_test_state(&fixture);
        let (id, _, _) = install_antigravity_test_provider(
            &state,
            &fixture,
            AppKind::Gemini,
            provider_type,
            format!("http://{address}"),
        )
        .await;
        let (route, path, body) = antigravity_timing_input(AppKind::Gemini, true);
        let attempt = antigravity_timing_attempt(&state, route, &id).await;
        let slot = attempt.timing.clone();
        let mut forwarding = Box::pin(forward_with_attempt(
            state,
            route,
            path,
            HeaderMap::new(),
            body,
            attempt,
        ));
        tokio::select! {
            _ = entered.notified() => {},
            result = &mut forwarding => panic!("unexpected completion: {result:?}"),
            _ = tokio::time::sleep(Duration::from_secs(5)) => panic!("loopback upstream was not reached"),
        }
        let timing = slot.get().unwrap().request();
        assert!(timing.snapshot()["milestonesMs"]["owner_released"].is_null());
        drop(forwarding);
        let after = timing.snapshot();
        assert_eq!(after["result"], "cancelled", "{after}");
        assert!(after["milestonesMs"]["headers"].is_null());
        assert!(after["milestonesMs"]["owner_released"].is_number());
        server.abort();
    }
}

#[tokio::test]
async fn antigravity_core03_committed_cancellation_and_error_do_not_replay() {
    for provider_type in [ProviderType::AntigravityOAuth, ProviderType::AgyOAuth] {
        for surface in [AppKind::Claude, AppKind::Gemini] {
            for provider_error in [false, true] {
                let release = Arc::new(tokio::sync::Notify::new());
                let handler_release = release.clone();
                let requests = Arc::new(AtomicUsize::new(0));
                let handler_requests = requests.clone();
                let app = axum::Router::new().fallback(axum::routing::post(move || {
                    let release = handler_release.clone();
                    let requests = handler_requests.clone();
                    async move {
                        requests.fetch_add(1, Ordering::SeqCst);
                        let content = stream::once(async {
                            Ok::<_, std::convert::Infallible>(Bytes::from_static(
                                ANTIGRAVITY_TIMING_CONTENT.as_bytes(),
                            ))
                        });
                        let error = stream::once(async move {
                            release.notified().await;
                            Ok::<_, std::convert::Infallible>(Bytes::from_static(
                                ANTIGRAVITY_TIMING_ERROR.as_bytes(),
                            ))
                        });
                        Response::builder()
                            .header(CONTENT_TYPE, "text/event-stream")
                            .body(Body::from_stream(content.chain(error)))
                            .unwrap()
                    }
                }));
                let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
                    .await
                    .unwrap();
                let address = listener.local_addr().unwrap();
                let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
                let fixture = format!(
                    "core03-committed-{}-{surface:?}-{provider_error}",
                    provider_type.as_str()
                );
                let state = forwarder_test_state(&fixture);
                let (id, _, _) = install_antigravity_test_provider(
                    &state,
                    &fixture,
                    surface,
                    provider_type,
                    format!("http://{address}"),
                )
                .await;
                let (route, path, body) = antigravity_timing_input(surface, true);
                let attempt = antigravity_timing_attempt(&state, route, &id).await;
                let slot = attempt.timing.clone();
                let response =
                    forward_with_attempt(state, route, path, HeaderMap::new(), body, attempt)
                        .await
                        .unwrap();
                assert_eq!(response.status(), StatusCode::OK);
                let timing = slot.get().unwrap().request();
                let mut stream = response.into_body().into_data_stream();
                tokio::time::timeout(Duration::from_secs(5), async {
                    loop {
                        let chunk = stream.next().await.unwrap().unwrap();
                        if String::from_utf8_lossy(&chunk).contains("hello") {
                            break;
                        }
                    }
                })
                .await
                .expect("content must precede terminal");
                assert!(timing.snapshot()["milestonesMs"]["first_semantic"].is_number());
                assert!(timing.snapshot()["milestonesMs"]["terminal"].is_null());
                if provider_error {
                    release.notify_one();
                    let mut rest = Vec::new();
                    tokio::time::timeout(Duration::from_secs(5), async {
                        while let Some(chunk) = stream.next().await {
                            rest.extend_from_slice(&chunk.unwrap());
                        }
                    })
                    .await
                    .expect("error must terminate the stream");
                    assert!(String::from_utf8_lossy(&rest).contains("error"));
                    assert_eq!(timing.snapshot()["result"], "failed");
                    assert!(timing.snapshot()["milestonesMs"]["terminal"].is_number());
                }
                drop(stream);
                if !provider_error {
                    assert_eq!(timing.snapshot()["result"], "cancelled");
                }
                assert!(timing.snapshot()["milestonesMs"]["owner_released"].is_number());
                assert_eq!(requests.load(Ordering::SeqCst), 1);
                server.abort();
            }
        }
    }
}

#[tokio::test]
async fn antigravity_core03_short_retry_keeps_binding_and_separate_attempts() {
    for provider_type in [ProviderType::AntigravityOAuth, ProviderType::AgyOAuth] {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let fixture = format!("core03-retry-{}", provider_type.as_str());
        let state = forwarder_test_state(&fixture);
        let (id, _, _) = install_antigravity_test_provider(
            &state,
            &fixture,
            AppKind::Gemini,
            provider_type,
            format!("http://{address}"),
        )
        .await;
        let (route, path, body) = antigravity_timing_input(AppKind::Gemini, false);
        let attempt = antigravity_timing_attempt(&state, route, &id).await;
        let slot = attempt.timing.clone();
        let handler_slot = slot.clone();
        let attempts = Arc::new(StdMutex::new(Vec::<ForwardTiming>::new()));
        let handler_attempts = attempts.clone();
        let app = axum::Router::new().fallback(axum::routing::post(move |headers: HeaderMap| {
            let slot = handler_slot.clone();
            let attempts = handler_attempts.clone();
            async move {
                assert_eq!(headers[axum::http::header::AUTHORIZATION],"Bearer antigravity-access");
                let first = { let mut attempts=attempts.lock().unwrap(); attempts.push(slot.get().unwrap()); attempts.len()==1 };
                if first {
                    (StatusCode::TOO_MANY_REQUESTS, axum::Json(json!({"error":{"status":"RESOURCE_EXHAUSTED","details":[{"@type":"type.googleapis.com/google.rpc.ErrorInfo","reason":"RATE_LIMIT_EXCEEDED","metadata":{"model":"gemini-3.5-flash-medium"}},{"@type":"type.googleapis.com/google.rpc.RetryInfo","retryDelay":"0.001s"}]}}))).into_response()
                } else {
                    assert!(slot.get().unwrap().request().snapshot()["milestonesMs"]["terminal"].is_null());
                    ([(CONTENT_TYPE,"text/event-stream")],format!("{ANTIGRAVITY_TIMING_CONTENT}{ANTIGRAVITY_TIMING_TERMINAL}")).into_response()
                }
            }
        }));
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let response = forward_with_attempt(state, route, path, HeaderMap::new(), body, attempt)
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        drop(
            axum::body::to_bytes(response.into_body(), 1024 * 1024)
                .await
                .unwrap(),
        );
        let attempts = attempts.lock().unwrap();
        assert_eq!(attempts.len(), 2);
        assert_eq!(attempts[0].attempt_snapshot()["result"], "retried");
        assert_eq!(attempts[1].attempt_snapshot()["result"], "complete");
        let after = slot.get().unwrap().request().snapshot();
        assert_eq!(after["providerType"], provider_type.as_str());
        assert_eq!(after["result"], "complete");
        assert!(after["waitsMs"]["retry"].as_f64().unwrap() > 0.0, "{after}");
        drop(attempts);
        server.abort();
    }
}

#[test]
fn antigravity_core03_semantic_content_excludes_metadata_and_retains_attempt_local_incomplete() {
    for provider_type in [ProviderType::AntigravityOAuth, ProviderType::AgyOAuth] {
        let slot = TimingSlot::default();
        let timing = slot
            .begin_for_provider(provider_type, "gemini_generate_content", Instant::now())
            .unwrap();
        for value in [
            json!({"response":{"usageMetadata":{"totalTokenCount":0}}}),
            json!({"response":{"candidates":[{"content":{"parts":[{"thought":true,"text":"","thoughtSignature":"opaque"}]}}]}}),
            json!({"candidates":[{"content":{"parts":[{"functionCall":{"name":" ","args":{}}}]}}]}),
            json!({"candidates":[{"finishReason":"MAX_TOKENS"}]}),
        ] {
            timing.observe_gemini_value(&value);
        }
        assert!(timing.request().snapshot()["milestonesMs"]["first_semantic"].is_null());
        assert!(timing.request().snapshot()["milestonesMs"]["terminal"].is_null());
        timing.observe_terminal(TimingResult::Complete);
        timing.finish_delivery(true);
        assert_eq!(timing.request().snapshot()["result"], "incomplete");
        let next = slot
            .begin_for_provider(provider_type, "gemini_generate_content", Instant::now())
            .unwrap();
        next.observe_gemini_document(
            &json!({"response":{"promptFeedback":{"blockReason":"SAFETY"}}}),
            false,
        );
        next.finish_delivery(true);
        assert_eq!(next.attempt_snapshot()["result"], "complete");
        assert!(next.attempt_snapshot()["milestonesMs"]["first_semantic"].is_null());
        for part in [
            json!({"text":"visible"}),
            json!({"thought":true,"text":"thinking"}),
            json!({"functionCall":{"name":"lookup","args":{}}}),
            json!({"inlineData":{"mimeType":"image/png","data":"YWJj"}}),
            json!({"fileData":{"fileUri":"https://example.invalid/media"}}),
            json!({"executableCode":{"code":"print(1)"}}),
            json!({"codeExecutionResult":{"output":"1"}}),
        ] {
            let value = json!({"response":{"candidates":[{"content":{"parts":[part]},"finishReason":"STOP"}]}});
            let timing = TimingSlot::default()
                .begin_for_provider(provider_type, "gemini_generate_content", Instant::now())
                .unwrap();
            timing.observe_gemini_document(&value, false);
            timing.finish_delivery(true);
            assert!(
                timing.request().snapshot()["milestonesMs"]["first_semantic"].is_number(),
                "{value}"
            );
            assert_eq!(timing.request().snapshot()["result"], "complete");
        }
        let timing = TimingSlot::default()
            .begin_for_provider(provider_type, "gemini_count_tokens", Instant::now())
            .unwrap();
        timing.observe_gemini_document(&json!({"totalTokens":-1}), true);
        assert!(timing.request().snapshot()["milestonesMs"]["first_semantic"].is_null());
        timing.observe_gemini_document(
            &json!({"response":{"error":{"status":"UNAVAILABLE"}}}),
            false,
        );
        timing.finish_delivery(true);
        assert_eq!(timing.request().snapshot()["result"], "failed");
    }
    assert!(TimingSlot::default()
        .begin_for_provider(
            ProviderType::GeminiCli,
            "gemini_generate_content",
            Instant::now()
        )
        .is_none());
}
