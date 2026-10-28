use axum::response::IntoResponse;

// Real forwarding fixtures for CORE-03. All endpoints are loopback-only cfg(test)
// overrides; these observations neither prove vendor acceptance nor alter usage.

const CLAUDE_TIMING_CONTENT: &str = concat!(
    "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_timing\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"claude-sonnet-4-6\",\"content\":[],\"stop_reason\":null,\"usage\":{\"input_tokens\":1,\"output_tokens\":0}}}\n\n",
    "event: ping\ndata: {\"type\":\"ping\"}\n\n",
    "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
    "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"hello\"}}\n\n",
);
const CLAUDE_TIMING_TERMINAL: &str = concat!(
    "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
    "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\",\"stop_sequence\":null},\"usage\":{\"output_tokens\":1}}\n\n",
    "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
);

async fn claude_timing_attempt(
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

#[tokio::test]
async fn claude_core03_timing_real_json_and_sse_cover_all_declared_surfaces() {
    let requests = Arc::new(AtomicUsize::new(0));
    let observed = requests.clone();
    let app = axum::Router::new().fallback(axum::routing::post(move |uri: axum::http::Uri, axum::Json(body): axum::Json<Value>| {
        let requests = observed.clone();
        async move {
            requests.fetch_add(1, Ordering::SeqCst);
            if uri.path().ends_with("count_tokens") {
                return ([(CONTENT_TYPE, "application/json")], "{\"input_tokens\":0}".to_string()).into_response();
            }
            if body.get("stream").and_then(Value::as_bool) == Some(true) {
                ([(CONTENT_TYPE, "text/event-stream")], format!("{CLAUDE_TIMING_CONTENT}{CLAUDE_TIMING_TERMINAL}")).into_response()
            } else {
                axum::Json(json!({"id":"msg_timing", "type":"message", "role":"assistant", "model":"claude-sonnet-4-6",
                    "content":[{"type":"text","text":"hello"}], "stop_reason":"end_turn", "stop_sequence":null,
                    "usage":{"input_tokens":1,"output_tokens":1}})).into_response()
            }
        }
    }));
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let state = forwarder_test_state("claude-core03-surfaces");
    let id = install_claude_oauth_forwarder_test_provider(
        &state,
        "claude-core03-surfaces",
        format!("http://{address}"),
    )
    .await;
    let provider = state
        .providers
        .read()
        .await
        .providers
        .iter()
        .find(|stored| stored.app == AppKind::Claude && stored.provider.id == id)
        .unwrap()
        .provider
        .clone();
    state
        .mutate_providers_immediate(move |providers| {
            providers.upsert(AppKind::Codex, provider.clone());
            providers.upsert(AppKind::Gemini, provider);
        })
        .await
        .unwrap();
    for app in [AppKind::Codex, AppKind::Gemini] {
        state
            .override_provider_runtime_endpoint_for_test(app, &id, format!("http://{address}"))
            .await
            .unwrap();
    }
    let mut expected_requests = 0;
    for streaming in [false, true] {
        let cases = [
            (
                ProxyRoute::ClaudeMessages,
                None,
                json!({"model":"claude-sonnet-4-6", "stream":streaming,"max_tokens":64,"messages":[{"role":"user","content":"hello"}]}),
            ),
            (
                ProxyRoute::CodexChatCompletions,
                None,
                json!({"model":"claude-sonnet-4-6", "stream":streaming,"messages":[{"role":"user","content":"hello"}]}),
            ),
            (
                ProxyRoute::CodexResponses,
                None,
                json!({"model":"claude-sonnet-4-6", "stream":streaming,"input":"hello"}),
            ),
            (
                ProxyRoute::Gemini,
                Some(format!(
                    "models/claude-sonnet-4-6:{}",
                    if streaming {
                        "streamGenerateContent"
                    } else {
                        "generateContent"
                    }
                )),
                json!({"contents":[{"role":"user","parts":[{"text":"hello"}]}]}),
            ),
        ];
        for (route, path, body) in cases {
            let attempt = claude_timing_attempt(&state, route, &id).await;
            let slot = attempt.timing.clone();
            let response = forward_with_attempt(
                state.clone(),
                route,
                path,
                HeaderMap::new(),
                Bytes::from(body.to_string()),
                attempt,
            )
            .await
            .unwrap();
            assert_eq!(
                response.status(),
                StatusCode::OK,
                "{route:?} stream={streaming}"
            );
            let timing = slot.get().expect("Claude must initialize timing").request();
            let before = timing.snapshot();
            assert_eq!(before["providerType"], "claude_oauth");
            for phase in ["admission", "send_started", "headers"] {
                assert!(
                    before["milestonesMs"][phase].is_number(),
                    "{route:?} {phase}: {before}"
                );
            }
            assert!(before["milestonesMs"]["http_body_handoff"].is_null());
            let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
                .await
                .unwrap();
            assert!(
                std::str::from_utf8(&bytes).unwrap().contains("hello"),
                "{route:?}"
            );
            drop(bytes);
            let after = timing.snapshot();
            assert_eq!(
                after["result"], "complete",
                "{route:?} stream={streaming}: {after}"
            );
            for phase in [
                "first_semantic",
                "http_body_handoff",
                "terminal",
                "owner_released",
            ] {
                assert!(
                    after["milestonesMs"][phase].is_number(),
                    "{route:?} stream={streaming} {phase}: {after}"
                );
            }
            expected_requests += 1;
        }
    }
    let route = ProxyRoute::ClaudeCountTokens;
    let attempt = claude_timing_attempt(&state, route, &id).await;
    let slot = attempt.timing.clone();
    let response = forward_with_attempt(
        state,
        route,
        None,
        HeaderMap::new(),
        Bytes::from_static(
            br#"{"model":"claude-sonnet-4-6","messages":[{"role":"user","content":"hi"}]}"#,
        ),
        attempt,
    )
    .await
    .unwrap();
    let bytes = axum::body::to_bytes(response.into_body(), 1024)
        .await
        .unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&bytes).unwrap()["input_tokens"],
        0
    );
    drop(bytes);
    let after = slot.get().unwrap().request().snapshot();
    assert_eq!(after["surface"], "anthropic_count_tokens");
    assert_eq!(after["result"], "complete");
    assert!(after["milestonesMs"]["first_semantic"].is_number());
    assert_eq!(requests.load(Ordering::SeqCst), expected_requests + 1);
    server.abort();
}

#[tokio::test]
async fn claude_core03_timing_header_cancel_precedes_last_request_owner_release() {
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
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let state = forwarder_test_state("claude-core03-cancel");
    let id = install_claude_oauth_forwarder_test_provider(
        &state,
        "claude-core03-cancel",
        format!("http://{address}"),
    )
    .await;
    let attempt = claude_timing_attempt(&state, ProxyRoute::ClaudeMessages, &id).await;
    let slot = attempt.timing.clone();
    let mut forwarding = Box::pin(forward_with_attempt(state, ProxyRoute::ClaudeMessages, None, HeaderMap::new(), Bytes::from_static(br#"{"model":"claude-sonnet-4-6","max_tokens":64,"stream":true,"messages":[{"role":"user","content":"hi"}]}"#), attempt));
    tokio::select! {
        _ = entered.notified() => {},
        result = &mut forwarding => panic!("unexpected completion: {result:?}"),
        _ = tokio::time::sleep(Duration::from_secs(5)) => panic!("loopback request was not received"),
    }
    let timing = slot.get().unwrap().request();
    assert!(timing.snapshot()["milestonesMs"]["owner_released"].is_null());
    drop(forwarding);
    let after = timing.snapshot();
    assert_eq!(after["result"], "cancelled", "{after}");
    assert!(after["milestonesMs"]["headers"].is_null());
    assert!(
        after["milestonesMs"]["owner_released"].is_number(),
        "{after}"
    );
    assert!(after["waitsMs"]["cancel_drain"].is_number());
    server.abort();
}

#[tokio::test]
async fn claude_core03_timing_committed_sse_cancel_and_provider_error_never_replay() {
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
                let content = futures_util::stream::once(async {
                    Ok::<_, std::convert::Infallible>(Bytes::from_static(
                        CLAUDE_TIMING_CONTENT.as_bytes(),
                    ))
                });
                let error = futures_util::stream::once(async move {
                    release.notified().await;
                    Ok::<_, std::convert::Infallible>(Bytes::from_static(
                        b"event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"fixture failure\"}}\n\n",
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
        let state = forwarder_test_state("claude-core03-committed");
        let id = install_claude_oauth_forwarder_test_provider(
            &state,
            "claude-core03-committed",
            format!("http://{address}"),
        )
        .await;
        let attempt = claude_timing_attempt(&state, ProxyRoute::ClaudeMessages, &id).await;
        let slot = attempt.timing.clone();
        let response = forward_with_attempt(
            state,
            ProxyRoute::ClaudeMessages,
            None,
            HeaderMap::new(),
            Bytes::from_static(br#"{"model":"claude-sonnet-4-6","max_tokens":64,"stream":true,"messages":[{"role":"user","content":"hi"}]}"#),
            attempt,
        )
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let timing = slot.get().unwrap().request();
        let mut stream = response.into_body().into_data_stream();
        let retained = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let bytes = stream.next().await.unwrap().unwrap();
                if String::from_utf8_lossy(&bytes).contains("hello") {
                    break bytes;
                }
            }
        })
        .await
        .expect("business content must be delivered before terminal");
        let before = timing.snapshot();
        assert!(
            before["milestonesMs"]["first_semantic"].is_number(),
            "{before}"
        );
        assert!(before["milestonesMs"]["terminal"].is_null(), "{before}");
        assert!(
            before["milestonesMs"]["owner_released"].is_null(),
            "{before}"
        );
        assert!(before["result"].is_null(), "{before}");
        if provider_error {
            release.notify_one();
            let mut rest = Vec::new();
            tokio::time::timeout(Duration::from_secs(5), async {
                while let Some(chunk) = stream.next().await {
                    rest.extend_from_slice(&chunk.unwrap());
                }
            })
            .await
            .expect("provider error must terminate the committed stream");
            assert!(String::from_utf8_lossy(&rest).contains("error"));
            assert_eq!(timing.snapshot()["result"], "failed");
            assert!(timing.snapshot()["milestonesMs"]["terminal"].is_number());
        }
        drop(retained);
        drop(stream);
        if !provider_error {
            assert_eq!(timing.snapshot()["result"], "cancelled");
        }
        assert!(timing.snapshot()["milestonesMs"]["owner_released"].is_number());
        assert_eq!(requests.load(Ordering::SeqCst), 1);
        server.abort();
    }
}

#[test]
fn claude_core03_timing_excludes_empty_control_and_retains_incomplete_hint() {
    let slot = TimingSlot::default();
    let timing = slot
        .begin_for_provider(
            ProviderType::ClaudeOAuth,
            "anthropic_messages",
            Instant::now(),
        )
        .unwrap();
    for value in [
        json!({"type":"ping"}),
        json!({"type":"content_block_start","content_block":{"type":"text","text":""}}),
        json!({"type":"content_block_start","content_block":{"type":"tool_use","id":" ","name":"tool","input":{}}}),
        json!({"type":"content_block_start","content_block":{"type":"tool_use","id":"t1","name":"tool","input":null}}),
        json!({"type":"content_block_delta","delta":{"type":"signature_delta","signature":"opaque-proof"}}),
        json!({"type":"content_block_stop","index":0}),
        json!({"type":"message_delta","delta":{"stop_reason":"pause_turn"},"usage":{"output_tokens":0}}),
    ] {
        timing.observe_anthropic_value(&value);
    }
    assert!(timing.request().snapshot()["milestonesMs"]["first_semantic"].is_null());
    assert!(timing.request().snapshot()["milestonesMs"]["terminal"].is_null());
    timing.observe_anthropic_value(&json!({"type":"content_block_delta","delta":{"type":"thinking_delta","thinking":"reason"}}));
    timing.observe_terminal(TimingResult::Complete);
    timing.finish_delivery(true);
    assert_eq!(timing.request().snapshot()["result"], "incomplete");
    let next = slot
        .begin_for_provider(
            ProviderType::ClaudeOAuth,
            "anthropic_messages",
            Instant::now(),
        )
        .unwrap();
    next.observe_terminal(TimingResult::Complete);
    next.finish_delivery(true);
    assert_eq!(next.attempt_snapshot()["result"], "complete");
    assert!(slot
        .begin_for_provider(
            ProviderType::CodexOAuth,
            "anthropic_messages",
            Instant::now()
        )
        .is_none());
    assert!(TimingSlot::default()
        .begin_for_provider(ProviderType::Claude, "anthropic_messages", Instant::now())
        .is_none());
}

#[test]
fn claude_core03_timing_retries_do_not_inherit_partial_completion() {
    let slot = TimingSlot::default();
    let first = slot
        .begin_for_provider(
            ProviderType::ClaudeOAuth,
            "anthropic_messages",
            Instant::now(),
        )
        .unwrap();
    first.observe_anthropic_value(
        &json!({"type":"message_delta","delta":{"stop_reason":"max_tokens"}}),
    );
    first.observe_terminal(TimingResult::Complete);
    assert!(first.request().snapshot()["result"].is_null());
    let next = slot
        .begin_for_provider(
            ProviderType::ClaudeOAuth,
            "anthropic_messages",
            Instant::now(),
        )
        .unwrap();
    assert_eq!(first.attempt_snapshot()["result"], "retried");
    next.observe_anthropic_value(
        &json!({"type":"content_block_delta","delta":{"type":"text_delta","text":"done"}}),
    );
    next.observe_terminal(TimingResult::Complete);
    next.finish_delivery(true);
    assert_eq!(next.request().snapshot()["result"], "complete");
}
