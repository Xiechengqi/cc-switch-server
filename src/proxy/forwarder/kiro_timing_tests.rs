// Synthetic EventStream fixtures exercise production forwarding; no vendor
// credentials or acceptance claims are involved.
fn kiro_core03_body(route: ProxyRoute, model: &str, streaming: bool) -> Bytes {
    let body = match route {
        ProxyRoute::CodexResponses => json!({"model":model,"stream":streaming,"input":"hi"}),
        _ => {
            json!({"model":model,"max_tokens":64,"stream":streaming,"messages":[{"role":"user","content":"hi"}]})
        }
    };
    Bytes::from(body.to_string())
}

async fn kiro_core03_provider(
    state: &ServerState,
    kind: ProviderType,
    route: ProxyRoute,
    endpoint: &str,
    token_url: &str,
) -> String {
    if kind == ProviderType::KiroOAuth {
        install_kiro_test_provider_for_app(
            state,
            route.app(),
            "kiro-core03",
            endpoint.into(),
            token_url.into(),
        )
        .await
    } else {
        let (claude, codex, _) = install_amazon_q_test_bundle(
            state,
            "amazon-q-core03",
            endpoint.into(),
            endpoint.into(),
            token_url.into(),
        )
        .await;
        if route.app() == AppKind::Claude {
            claude
        } else {
            codex
        }
    }
}

fn kiro_core03_model(kind: ProviderType) -> &'static str {
    if kind == ProviderType::KiroOAuth {
        "claude-sonnet-4-8"
    } else {
        "amazon-q-test-model"
    }
}

fn kiro_core03_catalog(headers: &HeaderMap) -> Option<Response> {
    (headers.get("x-amz-target").and_then(|value| value.to_str().ok()) == Some(crate::domain::providers::amazon_q::AMAZON_Q_LIST_MODELS_TARGET)).then(|| {
        axum::Json(json!({"models":[{"modelId":"amazon-q-test-model","modelName":"Timing fixture","tokenLimits":{"maxInputTokens":200000,"maxOutputTokens":8192}}],"defaultModel":{"modelId":"amazon-q-test-model"}})).into_response()
    })
}

#[tokio::test]
async fn kiro_core03_both_products_three_surfaces_stream_and_buffered_and_local_count() {
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let app = axum::Router::new().fallback(axum::routing::post(move |headers: HeaderMap| {
        let observed = observed.clone();
        async move {
            if let Some(catalog) = kiro_core03_catalog(&headers) {
                return catalog;
            }
            observed.fetch_add(1, Ordering::SeqCst);
            (
                [(CONTENT_TYPE, "application/vnd.amazon.eventstream")],
                kiro_text_eventstream("hello"),
            )
                .into_response()
        }
    }));
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    for kind in [ProviderType::KiroOAuth, ProviderType::AmazonQOAuth] {
        for route in [
            ProxyRoute::ClaudeMessages,
            ProxyRoute::CodexChatCompletions,
            ProxyRoute::CodexResponses,
            ProxyRoute::ClaudeCountTokens,
        ] {
            let state = forwarder_test_state(&format!("kiro-core03-{}-{route:?}", kind.as_str()));
            let id = kiro_core03_provider(
                &state,
                kind,
                route,
                &format!("http://{address}"),
                "http://127.0.0.1:9",
            )
            .await;
            let local = route == ProxyRoute::ClaudeCountTokens;
            for streaming in [false, true] {
                if local && streaming {
                    continue;
                }
                let attempt = claude_timing_attempt(&state, route, &id).await;
                let slot = attempt.timing.clone();
                let response = forward_with_attempt(
                    state.clone(),
                    route,
                    None,
                    HeaderMap::new(),
                    kiro_core03_body(route, kiro_core03_model(kind), streaming),
                    attempt,
                )
                .await
                .unwrap();
                assert_eq!(response.status(), StatusCode::OK);
                let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
                    .await
                    .unwrap();
                if !local {
                    assert!(String::from_utf8_lossy(&bytes).contains("hello"));
                }
                drop(bytes);
                let snapshot = slot.get().unwrap().request().snapshot();
                assert_eq!(snapshot["providerType"], kind.as_str());
                assert_eq!(snapshot["result"], "complete", "{snapshot}");
                for phase in [
                    "admission",
                    "terminal",
                    "http_body_handoff",
                    "owner_released",
                ] {
                    assert!(
                        snapshot["milestonesMs"][phase].is_number(),
                        "{phase}: {snapshot}"
                    );
                }
                for phase in ["first_semantic", "send_started", "headers"] {
                    assert_eq!(
                        snapshot["milestonesMs"][phase].is_number(),
                        !local,
                        "{phase}: {snapshot}"
                    );
                }
                assert!(snapshot["milestonesMs"]["downstream_write_completed"].is_null());
            }
        }
    }
    assert_eq!(calls.load(Ordering::SeqCst), 12);
    server.abort();
}

#[tokio::test]
async fn kiro_core03_control_frames_and_partial_wire_do_not_start_semantic_clock() {
    let release = Arc::new(tokio::sync::Notify::new());
    let upstream_release = release.clone();
    let app = axum::Router::new().fallback(axum::routing::post(move || {
        let release = upstream_release.clone();
        async move {
            let controls = [
                kiro::fixture_event_frame(
                    "metadataEvent",
                    &json!({"tokenUsage":{"inputTokens":0,"outputTokens":0}}),
                ),
                kiro::fixture_event_frame("assistantResponseEvent", &json!({"content":""})),
                kiro::fixture_event_frame("reasoningContentEvent", &json!({"signature":"opaque"})),
                kiro::fixture_event_frame("unknownEvent", &json!({"metadata":true})),
            ]
            .concat();
            let content = kiro_text_eventstream("hello");
            let chunks = async_stream::stream! {
                yield Ok::<_,std::convert::Infallible>(Bytes::from(controls));
                yield Ok(Bytes::copy_from_slice(&content[..8]));
                release.notified().await;
                yield Ok(Bytes::copy_from_slice(&content[8..]));
            };
            Response::builder()
                .header(CONTENT_TYPE, "application/vnd.amazon.eventstream")
                .body(Body::from_stream(chunks))
                .unwrap()
        }
    }));
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let state = forwarder_test_state("kiro-core03-controls");
    let id = kiro_core03_provider(
        &state,
        ProviderType::KiroOAuth,
        ProxyRoute::ClaudeMessages,
        &format!("http://{address}"),
        "http://127.0.0.1:9",
    )
    .await;
    let attempt = claude_timing_attempt(&state, ProxyRoute::ClaudeMessages, &id).await;
    let slot = attempt.timing.clone();
    let response = forward_with_attempt(
        state,
        ProxyRoute::ClaudeMessages,
        None,
        HeaderMap::new(),
        kiro_core03_body(ProxyRoute::ClaudeMessages, "claude-sonnet-4-8", true),
        attempt,
    )
    .await
    .unwrap();
    let request = slot.get().unwrap().request();
    let mut body = response.into_body().into_data_stream();
    let initial = body.next().await.unwrap().unwrap();
    assert!(String::from_utf8_lossy(&initial).contains("message_start"));
    drop(initial);
    assert!(
        tokio::time::timeout(Duration::from_millis(100), body.next())
            .await
            .is_err()
    );
    assert!(request.snapshot()["milestonesMs"]["first_semantic"].is_null());
    assert!(request.snapshot()["milestonesMs"]["terminal"].is_null());
    release.notify_one();
    while let Some(frame) = body.next().await {
        frame.unwrap();
    }
    drop(body);
    let snapshot = request.snapshot();
    assert_eq!(snapshot["result"], "complete");
    assert!(snapshot["milestonesMs"]["first_semantic"].is_number());
    assert!(snapshot["waitsMs"]["upstream_read"].as_f64().unwrap() > 0.0);
    server.abort();
}

#[tokio::test]
async fn kiro_core03_cancel_and_committed_truncation_keep_last_bytes_owner_and_do_not_replay() {
    for fail in [false, true] {
        let calls = Arc::new(AtomicUsize::new(0));
        let observed = calls.clone();
        let app = axum::Router::new().fallback(axum::routing::post(move || {
            observed.fetch_add(1,Ordering::SeqCst);
            async move {
                let chunks = async_stream::stream! {
                    yield Ok::<_,std::convert::Infallible>(Bytes::from(kiro::fixture_event_frame("assistantResponseEvent",&json!({"content":"hello"}))));
                    tokio::time::sleep(Duration::from_millis(30)).await;
                    if !fail { std::future::pending::<()>().await; }
                };
                Response::builder().header(CONTENT_TYPE,"application/vnd.amazon.eventstream").body(Body::from_stream(chunks)).unwrap()
            }
        }));
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let state = forwarder_test_state(&format!("kiro-core03-owner-{fail}"));
        let id = kiro_core03_provider(
            &state,
            ProviderType::KiroOAuth,
            ProxyRoute::ClaudeMessages,
            &format!("http://{address}"),
            "http://127.0.0.1:9",
        )
        .await;
        let attempt = claude_timing_attempt(&state, ProxyRoute::ClaudeMessages, &id).await;
        let slot = attempt.timing.clone();
        let response = forward_with_attempt(
            state,
            ProxyRoute::ClaudeMessages,
            None,
            HeaderMap::new(),
            kiro_core03_body(ProxyRoute::ClaudeMessages, "claude-sonnet-4-8", true),
            attempt,
        )
        .await
        .unwrap();
        let request = slot.get().unwrap().request();
        let mut body = response.into_body().into_data_stream();
        let retained = loop {
            let frame = body.next().await.unwrap().unwrap();
            if String::from_utf8_lossy(&frame).contains("hello") {
                break frame;
            }
        };
        if fail {
            while let Some(frame) = body.next().await {
                drop(frame);
            }
        }
        drop(body);
        assert_eq!(
            request.snapshot()["result"],
            if fail { "failed" } else { "cancelled" }
        );
        assert!(request.snapshot()["milestonesMs"]["owner_released"].is_null());
        drop(retained);
        assert!(request.snapshot()["milestonesMs"]["owner_released"].is_number());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        server.abort();
    }
}

#[tokio::test]
async fn kiro_core03_pending_headers_cancel_precedes_budget_release() {
    let app = axum::Router::new().fallback(axum::routing::post(|| async {
        std::future::pending::<StatusCode>().await
    }));
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let state = forwarder_test_state("kiro-core03-header-cancel");
    let id = kiro_core03_provider(
        &state,
        ProviderType::KiroOAuth,
        ProxyRoute::ClaudeMessages,
        &format!("http://{address}"),
        "http://127.0.0.1:9",
    )
    .await;
    let attempt = claude_timing_attempt(&state, ProxyRoute::ClaudeMessages, &id).await;
    let slot = attempt.timing.clone();
    let mut pending = Box::pin(forward_with_attempt(
        state,
        ProxyRoute::ClaudeMessages,
        None,
        HeaderMap::new(),
        kiro_core03_body(ProxyRoute::ClaudeMessages, "claude-sonnet-4-8", true),
        attempt,
    ));
    assert!(
        tokio::time::timeout(Duration::from_millis(100), pending.as_mut())
            .await
            .is_err()
    );
    let request = slot.get().unwrap().request();
    assert!(request.snapshot()["milestonesMs"]["send_started"].is_number());
    assert!(request.snapshot()["milestonesMs"]["headers"].is_null());
    drop(pending);
    let snapshot = request.snapshot();
    assert_eq!(snapshot["result"], "cancelled");
    assert!(snapshot["milestonesMs"]["owner_released"].is_number());
    server.abort();
}

#[tokio::test]
async fn kiro_core03_auth_refresh_starts_one_new_attempt_and_records_only_existing_wait() {
    let samples = GrokTimingSamples::default();
    let _recorder = metrics::set_default_local_recorder(&samples);
    let calls = Arc::new(AtomicUsize::new(0));
    let refreshes = Arc::new(AtomicUsize::new(0));
    let observed_calls = calls.clone();
    let observed_refreshes = refreshes.clone();
    let app = axum::Router::new()
        .route("/token",axum::routing::post(move || {
            observed_refreshes.fetch_add(1,Ordering::SeqCst);
            async {
                tokio::time::sleep(Duration::from_millis(10)).await;
                axum::Json(json!({"accessToken":"kiro-new-access","refreshToken":"kiro-new-refresh","tokenType":"Bearer","expiresIn":3600}))
            }
        }))
        .fallback(axum::routing::post(move |headers: HeaderMap| {
            observed_calls.fetch_add(1,Ordering::SeqCst);
            async move {
                if headers.get("authorization").and_then(|value| value.to_str().ok()) == Some("Bearer kiro-old-access") {
                    StatusCode::UNAUTHORIZED.into_response()
                } else { ([(CONTENT_TYPE,"application/vnd.amazon.eventstream")],kiro_text_eventstream("hello")).into_response() }
            }
        }));
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let state = forwarder_test_state("kiro-core03-refresh");
    let id = kiro_core03_provider(
        &state,
        ProviderType::KiroOAuth,
        ProxyRoute::ClaudeMessages,
        &format!("http://{address}"),
        &format!("http://{address}/token"),
    )
    .await;
    let attempt = claude_timing_attempt(&state, ProxyRoute::ClaudeMessages, &id).await;
    let slot = attempt.timing.clone();
    let response = forward_with_attempt(
        state,
        ProxyRoute::ClaudeMessages,
        None,
        HeaderMap::new(),
        kiro_core03_body(ProxyRoute::ClaudeMessages, "claude-sonnet-4-8", true),
        attempt,
    )
    .await
    .unwrap();
    drop(
        axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap(),
    );
    assert_eq!(
        slot.get().unwrap().request().snapshot()["result"],
        "complete"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(refreshes.load(Ordering::SeqCst), 1);
    assert_eq!(
        samples.count(
            "cc_switch_forward_duration_seconds",
            &[
                ("provider_type", "kiro_oauth"),
                ("scope", "attempt"),
                ("result", "retried")
            ]
        ),
        1
    );
    assert_eq!(
        samples.count(
            "cc_switch_forward_duration_seconds",
            &[
                ("provider_type", "kiro_oauth"),
                ("scope", "request"),
                ("result", "complete")
            ]
        ),
        1
    );
    assert!(
        slot.get().unwrap().request().snapshot()["waitsMs"]["retry"]
            .as_f64()
            .unwrap()
            > 0.0
    );
    server.abort();
}
