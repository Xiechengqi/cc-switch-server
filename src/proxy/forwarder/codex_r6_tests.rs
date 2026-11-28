#[derive(Clone, Copy)]
enum CodexR6Behavior {
    Complete,
    ToolComplete,
    InvalidImage {
        placement: usize,
        sse: bool,
        fallback: bool,
    },
    UnauthorizedOnce,
    Fallback,
}

fn codex_r6_invalid_image(placement: usize) -> Value {
    let error = json!({"code":"invalid_value", "type":"invalid_request_error", "param":"input", "message":"The image data you provided does not represent a valid image. Please check your input and try again. Our servers are currently overloaded."});
    match placement {
        0 => json!({"type":"error", "error":error}),
        1 => json!({"type":"response.failed", "response":{"status":"failed", "error":error}}),
        2 => {
            json!({"type":"response.failed", "response":{"status":"failed", "status_details":{"error":error}}})
        }
        _ => error,
    }
}

fn codex_r6_completed(id: &str) -> Value {
    json!({"type":"response.completed","response":{"id":id,"object":"response","model":"gpt-5.4","status":"completed","output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"hello"}]}],"usage":{"input_tokens":1,"output_tokens":2,"total_tokens":3}}})
}

// Synthetic upstreams; exercise the public forwarding function after the
// trusted Router ingress boundary, never a fabricated caller-selected binding.
#[derive(Clone)]
struct CodexR6Observation {
    connection: usize,
    transport: &'static str,
    headers: HeaderMap,
    body: Value,
}

struct CodexR6Fixture {
    state: ServerState,
    execution: ProviderExecution,
    behavior: CodexR6Behavior,
    share_id: String,
    address: std::net::SocketAddr,
    observations: Arc<StdMutex<Vec<CodexR6Observation>>>,
    cache_keys: StdMutex<Vec<String>>,
    handshakes: Arc<StdMutex<Vec<HeaderMap>>>,
    server: tokio::task::JoinHandle<()>,
    upstream: tokio::task::JoinHandle<()>,
}

impl Drop for CodexR6Fixture {
    fn drop(&mut self) {
        let mut pool = responses_websocket_pool()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for key in self.cache_keys.lock().unwrap().iter() {
            drop(pool.acquire(key));
        }
        self.server.abort();
        self.upstream.abort();
    }
}

impl CodexR6Fixture {
    async fn new(name: &str) -> Self {
        Self::with_behavior(name, CodexR6Behavior::Complete).await
    }

    async fn with_behavior(name: &str, behavior: CodexR6Behavior) -> Self {
        let observations = Arc::new(StdMutex::new(Vec::new()));
        let observed = observations.clone();
        let http_observed = observations.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream_address = listener.local_addr().unwrap();
        let connections = Arc::new(AtomicUsize::new(0));
        let handshakes = Arc::new(StdMutex::new(Vec::new()));
        let captured_handshakes = handshakes.clone();
        let http_calls = Arc::new(AtomicUsize::new(0));
        let ws_route = axum::routing::get(move |ws: WebSocketUpgrade, headers: HeaderMap| {
            let observed = observed.clone();
            captured_handshakes.lock().unwrap().push(headers.clone());
            let connection = connections.fetch_add(1, Ordering::SeqCst) + 1;
            async move {
                if matches!(behavior, CodexR6Behavior::UnauthorizedOnce) && connection == 1 {
                    return (StatusCode::UNAUTHORIZED, "expired token").into_response();
                }
                if matches!(
                    behavior,
                    CodexR6Behavior::Fallback
                        | CodexR6Behavior::InvalidImage { fallback: true, .. }
                ) {
                    return (StatusCode::SERVICE_UNAVAILABLE, "handshake unavailable")
                        .into_response();
                }
                ws.on_upgrade(move |mut socket| async move {
                    while let Some(Ok(message)) = socket.next().await {
                        let payload = match message {
                            AxumWsMessage::Text(text) => text.into_bytes(),
                            AxumWsMessage::Binary(bytes) => bytes,
                            AxumWsMessage::Close(_) => break,
                            _ => continue,
                        };
                        let body: Value = serde_json::from_slice(&payload).unwrap();
                        let index = {
                            let mut observations = observed.lock().unwrap();
                            observations.push(CodexR6Observation {
                                connection, transport: "ws", headers: headers.clone(), body,
                            });
                            observations.len()
                        };
                        let id = format!("resp_r6_{connection}_{index}");
                        let events = match behavior {
                            CodexR6Behavior::InvalidImage { placement, .. } => vec![codex_r6_invalid_image(placement)],
                            _ => vec![json!({"type":"response.created","response":{"id":id,"status":"in_progress"}}), codex_r6_completed_for_behavior(&id, behavior)],
                        };
                        for event in events {
                            if socket.send(AxumWsMessage::Text(event.to_string())).await.is_err() { return; }
                        }
                        if matches!(behavior, CodexR6Behavior::InvalidImage { .. }) {
                            let _ = socket.send(AxumWsMessage::Close(None)).await;
                            return;
                        }
                    }
                }).into_response()
            }
        });
        let route = ws_route.post(move |headers: HeaderMap, body: Bytes| {
            let observed = http_observed.clone();
            let call = http_calls.fetch_add(1, Ordering::SeqCst) + 1;
            async move {
                let body: Value = serde_json::from_slice(&body).unwrap();
                observed.lock().unwrap().push(CodexR6Observation {
                    connection: 0,
                    transport: "http",
                    headers,
                    body,
                });
                if matches!(behavior, CodexR6Behavior::UnauthorizedOnce) && call == 1 {
                    return (
                        StatusCode::UNAUTHORIZED,
                        axum::Json(
                            json!({"error":{"code":"token_expired","message":"expired token"}}),
                        ),
                    )
                        .into_response();
                }
                match behavior {
                    CodexR6Behavior::InvalidImage {
                        placement,
                        sse: true,
                        ..
                    } => (
                        [(CONTENT_TYPE, "text/event-stream")],
                        format!(
                            "event: {}\ndata: {}\n\n",
                            if placement == 0 {
                                "error"
                            } else {
                                "response.failed"
                            },
                            codex_r6_invalid_image(placement)
                        ),
                    )
                        .into_response(),
                    CodexR6Behavior::InvalidImage { placement, .. } => (
                        StatusCode::BAD_REQUEST,
                        axum::Json(codex_r6_invalid_image(placement)),
                    )
                        .into_response(),
                    _ => (
                        [(CONTENT_TYPE, "text/event-stream")],
                        format!(
                            "event: response.completed\ndata: {}\n\ndata: [DONE]\n\n",
                            codex_r6_completed_for_behavior("resp_r6_http", behavior)
                        ),
                    )
                        .into_response(),
                }
            }
        });
        let app = axum::Router::new().fallback_service(route);
        let upstream = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let (state, execution) =
            codex_bridge_test_context(name, format!("http://{upstream_address}")).await;
        let mut providers = state.providers.read().await.clone();
        let mut plan = execution.plan.as_ref().clone();
        plan.driver_options.insert(
            "testCodexWebsocketUrl".into(),
            json!(format!("ws://{upstream_address}/responses")),
        );
        Arc::make_mut(&mut providers.runtime_index).insert_plan_for_test(plan);
        let execution =
            ProviderExecution::from_store(&providers, execution.stored.clone()).unwrap();
        state.replace_provider_store_for_test(providers).await;
        let share_id = format!("{name}-share");
        install_antigravity_test_share(
            &state,
            &share_id,
            AppKind::Codex,
            ProviderType::CodexOAuth,
            &execution.stored.provider.id,
        )
        .await;
        let bound_state = state.clone();
        let bound_share = share_id.clone();
        let bound_provider = execution.stored.provider.id.clone();
        let expected_ws_url = format!("ws://{upstream_address}/responses");
        let app = axum::Router::new().route(
            "/responses",
            axum::routing::get(move |ws: WebSocketUpgrade, mut headers: HeaderMap| {
                let state = bound_state.clone();
                let share = bound_share.clone();
                let provider = bound_provider.clone();
                let expected_ws_url = expected_ws_url.clone();
                async move {
                    // Account writes rebuild the index. Never let a fixture
                    // silently fall through to a production endpoint.
                    {
                        let providers = state.providers.read().await;
                        let plan = providers.runtime_plan(AppKind::Codex, &provider).unwrap();
                        assert_eq!(
                            plan.driver_options
                                .get("testCodexWebsocketUrl")
                                .and_then(Value::as_str),
                            Some(expected_ws_url.as_str())
                        );
                    }

                    // Fixed server-side fixture identity represents verified
                    // ingress. Client headers never choose this Share/user.
                    headers.insert(
                        "x-cc-switch-share-id",
                        HeaderValue::from_str(&share).unwrap(),
                    );
                    headers.insert(
                        "x-cc-switch-user-email",
                        HeaderValue::from_static("owner@example.com"),
                    );
                    match forward_codex_responses_ws(state, headers, ws).await {
                        Ok(response) => response,
                        Err(error) => {
                            (error.status, error.client_message().to_string()).into_response()
                        }
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Self {
            state,
            execution,
            behavior,
            share_id,
            address,
            observations,
            cache_keys: StdMutex::new(Vec::new()),
            handshakes,
            server,
            upstream,
        }
    }

    async fn turn(&self, beta: Option<&str>, previous: Option<&str>, trace: &str) -> Vec<Value> {
        self.turn_with_exclude(beta, None, previous, trace).await
    }

    async fn turn_with_exclude(
        &self,
        beta: Option<&str>,
        exclude: Option<&str>,
        previous: Option<&str>,
        trace: &str,
    ) -> Vec<Value> {
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        // Restore only this synthetic Provider's authored loopback plan after
        // a credential write rebuilt runtime plans. Production code receives
        // its binding through the normal Router Share selector below.
        let mut providers = self.state.providers.read().await.clone();
        Arc::make_mut(&mut providers.runtime_index)
            .insert_plan_for_test(self.execution.plan.as_ref().clone());
        self.state.replace_provider_store_for_test(providers).await;
        let mut request = format!("ws://{}/responses", self.address)
            .into_client_request()
            .unwrap();
        let headers = request.headers_mut();
        headers.insert(
            "x-session-affinity",
            HeaderValue::from_static("r6-native-session"),
        );
        headers.insert("x-client-request-id", HeaderValue::from_str(trace).unwrap());
        if let Some(beta) = beta {
            headers.insert(
                "x-codex-beta-features",
                HeaderValue::from_str(beta).unwrap(),
            );
        }
        if let Some(exclude) = exclude {
            headers.insert(
                super::super::codex_negotiation::EXCLUDE_HEADER,
                HeaderValue::from_str(exclude).unwrap(),
            );
        }
        let scope = UsageLogContext {
            share_id: Some(self.share_id.clone()),
            user_email: Some("owner@example.com".to_string()),
            ..Default::default()
        };
        let client_headers = codex_responses_client_headers(headers).unwrap();
        let cleanup = prepare_responses_websocket_target_with_client_headers(
            &self.state,
            &self.execution,
            ResponsesWebsocketMode::Codex,
            Some("r6-native-session"),
            None,
            &client_headers,
            Some(&scope),
        )
        .await
        .unwrap();
        if let Some(key) = cleanup.pool_key {
            self.cache_keys.lock().unwrap().push(key);
        }
        let (mut client, _) = tokio::time::timeout(
            Duration::from_secs(3),
            tokio_tungstenite::connect_async(request),
        )
        .await
        .unwrap()
        .unwrap();
        let mut body = json!({"type":"response.create","model":"gpt-5.4","input":"hello"});
        if let Some(previous) = previous {
            body["previous_response_id"] = json!(previous);
        }
        if matches!(self.behavior, CodexR6Behavior::ToolComplete) {
            body["tools"] = json!([{"type":"function","name":"lookup","parameters":{"type":"object","properties":{"id":{"type":"string"}}}}]);
        }

        client
            .send(TungsteniteMessage::Text(body.to_string()))
            .await
            .unwrap();
        let mut events = Vec::new();
        loop {
            let next = tokio::time::timeout(Duration::from_secs(3), client.next())
                .await
                .unwrap_or_else(|error| panic!("downstream event deadline: trace={trace}, generations={}, handshakes={}: {error}", self.observations.lock().unwrap().len(), self.handshakes.lock().unwrap().len()));
            let Some(Ok(message)) = next else { break };
            if matches!(message, TungsteniteMessage::Close(_)) {
                break;
            }
            let payload = message.into_data();
            if let Ok(event) = serde_json::from_slice::<Value>(&payload) {
                let done = matches!(
                    event["type"].as_str(),
                    Some("response.completed" | "response.failed" | "error")
                );
                events.push(event);
                if done {
                    break;
                }
            }
        }
        let _ = client.close(None).await;
        tokio::time::timeout(Duration::from_secs(3), async {
            while self.state.share_in_flight.has_in_flight(&self.share_id) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        // Authentication recovery can change the credential digest; only
        // clean entries owned by this fixture, under the final identity too.
        if let Ok(target) = prepare_responses_websocket_target_with_client_headers(
            &self.state,
            &self.execution,
            ResponsesWebsocketMode::Codex,
            Some("r6-native-session"),
            None,
            &client_headers,
            Some(&scope),
        )
        .await
        {
            if let Some(key) = target.pool_key {
                self.cache_keys.lock().unwrap().push(key);
            }
        }
        events
    }
}

#[tokio::test]
async fn codex_r6_native_entry_reuses_only_matching_handshake_options() {
    let fixture = CodexR6Fixture::new("r6-codex-native-handshake").await;
    for (index, beta) in [
        Some("feature-a"),
        Some("feature-b"),
        Some("feature-b"),
        None,
        Some("feature-a"),
    ]
    .into_iter()
    .enumerate()
    {
        let events = fixture.turn(beta, None, &format!("trace-{index}")).await;
        assert_eq!(events.last().unwrap()["type"], "response.completed");
        let observations = fixture.observations.lock().unwrap();
        let last = observations.last().unwrap();
        assert_eq!(
            last.headers
                .get("x-codex-beta-features")
                .and_then(|value| value.to_str().ok()),
            beta,
            "upstream used the wrong handshake on turn {index}"
        );
        assert_eq!(last.body["model"], "gpt-5.4");
    }
    let observations = fixture.observations.lock().unwrap();
    assert_eq!(observations.len(), 5);
    let ids: Vec<_> = observations.iter().map(|value| value.connection).collect();
    assert_ne!(ids[0], ids[1]);
    assert_eq!(
        ids[1], ids[2],
        "diagnostic request IDs must not fragment reuse"
    );
    assert_ne!(ids[3], ids[0]);
    assert_ne!(ids[3], ids[1]);
    assert_eq!(
        ids[0], ids[4],
        "matching options may reuse their original idle socket"
    );
}

#[tokio::test]
async fn codex_r6_native_entry_rejects_known_continuation_with_changed_handshake() {
    let fixture = CodexR6Fixture::new("r6-codex-continuation-options").await;
    let first = fixture.turn(Some("feature-a"), None, "first").await;
    let id = first.last().unwrap()["response"]["id"].as_str().unwrap();
    let second = fixture.turn(Some("feature-b"), Some(id), "second").await;
    assert!(
        second.iter().any(|event| event["type"] == "error"),
        "{second:?}"
    );
    assert_eq!(
        second.last().unwrap()["error"]["code"],
        "response_context_unavailable"
    );
    assert!(!second
        .iter()
        .any(|event| event["type"] == "response.completed"));
    assert_eq!(
        fixture.observations.lock().unwrap().len(),
        1,
        "changed continuation must not submit another generation"
    );
    let third = fixture.turn(Some("feature-a"), Some(id), "third").await;
    assert_eq!(third.last().unwrap()["type"], "response.completed");
    let observations = fixture.observations.lock().unwrap();
    assert_eq!(observations[0].connection, observations[1].connection);
}

#[tokio::test]
async fn codex_r6_native_exclusion_changes_and_omission_have_distinct_handshakes() {
    let fixture = CodexR6Fixture::new("r6-codex-exclude-ws").await;
    for (index, exclude) in [
        Some(" tools, usage.attribution, tools "),
        Some("usage.attribution,tools"),
        None,
        Some("  "),
        Some("tools"),
    ]
    .into_iter()
    .enumerate()
    {
        let events = fixture
            .turn_with_exclude(None, exclude, None, &format!("trace-{index}"))
            .await;
        assert_eq!(events.last().unwrap()["type"], "response.completed");
        assert_eq!(
            events.last().unwrap()["response"]["usage"]["input_tokens"],
            1
        );
        assert_eq!(
            events.last().unwrap()["response"]["usage"]["output_tokens"],
            2
        );
    }
    let observations = fixture.observations.lock().unwrap();
    let actual: Vec<_> = observations
        .iter()
        .map(|entry| {
            entry
                .headers
                .get("x-codex-exclude")
                .map(|value| value.to_str().unwrap())
        })
        .collect();
    assert_eq!(
        actual,
        [
            Some("tools,usage.attribution"),
            Some("tools,usage.attribution"),
            None,
            None,
            Some("tools")
        ]
    );
    assert_eq!(observations[0].connection, observations[1].connection);
    assert_eq!(observations[2].connection, observations[3].connection);
    assert_ne!(observations[0].connection, observations[2].connection);
    assert_ne!(observations[0].connection, observations[4].connection);
}

#[tokio::test]
async fn codex_r6_http_exclusion_is_managed_bounded_and_keeps_usage() {
    let fixture = CodexR6Fixture::new("r6-codex-exclude-http").await;
    for (exclude, expected) in [
        (
            Some(" usage.attribution,tools,tools "),
            Some("tools,usage.attribution"),
        ),
        (None, None),
        (Some("  "), None),
    ] {
        let mut headers = HeaderMap::new();
        if let Some(value) = exclude {
            headers.insert("x-codex-exclude", HeaderValue::from_str(value).unwrap());
        }
        let response = forward_with_attempt(
            fixture.state.clone(),
            ProxyRoute::CodexResponses,
            None,
            headers,
            Bytes::from_static(br#"{"model":"gpt-5.4","input":"hi","stream":false}"#),
            ForwardAttemptContext {
                execution: Some(fixture.execution.clone()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["usage"]["input_tokens"], 1, "{value}");
        assert_eq!(value["usage"]["output_tokens"], 2, "{value}");
        let observations = fixture.observations.lock().unwrap();
        assert_eq!(
            observations
                .last()
                .unwrap()
                .headers
                .get("x-codex-exclude")
                .map(|v| v.to_str().unwrap()),
            expected
        );
    }
    let before = fixture.observations.lock().unwrap().len();
    for invalid in ["usage", "output", "unknown"] {
        let mut headers = HeaderMap::new();
        headers.insert("x-codex-exclude", HeaderValue::from_str(invalid).unwrap());
        let error = forward_with_attempt(
            fixture.state.clone(),
            ProxyRoute::CodexResponses,
            None,
            headers,
            Bytes::from_static(br#"{"model":"gpt-5.4","input":"hi"}"#),
            ForwardAttemptContext {
                execution: Some(fixture.execution.clone()),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
        assert_eq!(error.status, StatusCode::BAD_REQUEST);
    }
    assert_eq!(fixture.observations.lock().unwrap().len(), before);
    assert!(
        super::super::account_headers::account_header_override_blocked(
            "X-Codex-Exclude",
            ProviderType::CodexOAuth
        )
    );
    let logs = fixture.state.usage_snapshot().await.logs;
    assert_eq!(logs.iter().filter(|log| log.status_code == 200).count(), 3);
    assert!(logs
        .iter()
        .filter(|log| log.status_code == 200)
        .all(|log| log.input_tokens == Some(1) && log.output_tokens == Some(2)));
}

#[tokio::test]
async fn codex_r6_fallback_preserves_final_negotiation_and_root_usage() {
    let fixture =
        CodexR6Fixture::with_behavior("r6-codex-exclude-fallback", CodexR6Behavior::Fallback).await;
    let events = fixture
        .turn_with_exclude(
            Some("feature-a"),
            Some("tools, usage.attribution"),
            None,
            "fallback-trace",
        )
        .await;
    assert_eq!(
        events.last().unwrap()["type"],
        "response.completed",
        "{events:?}"
    );
    assert_eq!(
        events.last().unwrap()["response"]["usage"]["total_tokens"],
        3
    );
    let observations = fixture.observations.lock().unwrap();
    assert_eq!(observations.len(), 1);
    let last = &observations[0];
    assert_eq!(last.transport, "http");
    assert_eq!(last.headers["x-codex-exclude"], "tools,usage.attribution");
    assert_eq!(last.headers["x-codex-beta-features"], "feature-a");
}

#[tokio::test]
async fn codex_r6_bad_image_with_overload_words_never_replays_http_or_ws() {
    for (placement, sse) in
        (0..3).flat_map(|placement| [false, true].map(move |sse| (placement, sse)))
    {
        let fixture = CodexR6Fixture::with_behavior(
            &format!("r6-codex-bad-image-{placement}-{sse}"),
            CodexR6Behavior::InvalidImage {
                placement,
                sse,
                fallback: false,
            },
        )
        .await;
        let result = forward_with_attempt(
            fixture.state.clone(),
            ProxyRoute::CodexResponses,
            None,
            HeaderMap::new(),
            Bytes::from_static(br#"{"model":"gpt-5.4","input":"hi"}"#),
            ForwardAttemptContext {
                execution: Some(fixture.execution.clone()),
                ..Default::default()
            },
        )
        .await;
        match result {
            Err(error) => assert!(
                error.status.is_client_error() || error.status.is_server_error(),
                "{error:?}"
            ),
            Ok(response) => {
                let status = response.status();
                let body = collect_response_body(response).await;
                let value: Value = serde_json::from_slice(&body).unwrap();
                assert!(
                    !status.is_success()
                        || value["error"].is_object()
                        || value["response"]["status"] == "failed",
                    "{status}: {value}"
                );
                assert_ne!(value["type"], "response.completed");
            }
        }
        assert_eq!(fixture.observations.lock().unwrap().len(), 1);
        let events = fixture.turn(None, None, "bad-image").await;
        assert!(
            !events
                .iter()
                .any(|event| event["type"] == "response.completed"),
            "{events:?}"
        );
        let observations = fixture.observations.lock().unwrap();
        assert_eq!(
            observations.len(),
            2,
            "no WS-to-HTTP replay on deterministic input failure"
        );
        assert_eq!(observations[1].transport, "ws");
    }
}

#[tokio::test]
async fn codex_r6_bad_image_fallback_sse_does_not_retry_its_overload_diagnostic() {
    for placement in 0..3 {
        let fixture = CodexR6Fixture::with_behavior(
            &format!("r6-bad-image-fallback-{placement}"),
            CodexR6Behavior::InvalidImage {
                placement,
                sse: true,
                fallback: true,
            },
        )
        .await;
        let events = fixture.turn(None, None, "fallback-bad-image").await;
        assert!(
            !events
                .iter()
                .any(|event| event["type"] == "response.completed"),
            "{events:?}"
        );
        let observations = fixture.observations.lock().unwrap();
        assert_eq!(observations.len(), 1);
        assert_eq!(observations[0].transport, "http");
    }
}

#[tokio::test]
async fn codex_r6_same_account_401_rebuild_keeps_exclusion_and_final_handshake_key() {
    for websocket in [false, true] {
        let name = format!("r6-codex-exclude-401-{websocket}");
        let fixture = CodexR6Fixture::with_behavior(&name, CodexR6Behavior::UnauthorizedOnce).await;
        let token = signed_openai_access_token(&format!("{name}-workspace")).await;
        let refresh = spawn_test_codex_refresh_endpoint(token.clone()).await;
        configure_codex_refresh_test_account(&fixture.state, &name, refresh.url.clone()).await;
        if websocket {
            let events = fixture
                .turn_with_exclude(
                    Some("feature-a"),
                    Some(" usage.attribution, tools "),
                    None,
                    "refresh-trace",
                )
                .await;
            assert_eq!(
                events.last().unwrap()["type"],
                "response.completed",
                "{events:?}"
            );
            let again = fixture
                .turn_with_exclude(
                    Some("feature-a"),
                    Some("tools,usage.attribution"),
                    None,
                    "another-trace",
                )
                .await;
            assert_eq!(again.last().unwrap()["type"], "response.completed");
            let observations = fixture.observations.lock().unwrap();
            assert_eq!(observations.len(), 2);
            assert_eq!(
                observations[0].connection, observations[1].connection,
                "refreshed final identity must match the next entry"
            );
            let handshakes = fixture.handshakes.lock().unwrap();
            assert_eq!(
                handshakes.len(),
                2,
                "one failed handshake followed by one reusable connection"
            );
            for headers in handshakes.iter() {
                assert_eq!(headers["x-codex-exclude"], "tools,usage.attribution");
            }
            assert_ne!(
                handshakes[0]["authorization"],
                handshakes[1]["authorization"]
            );
            assert_eq!(handshakes[1]["authorization"], format!("Bearer {token}"));
        } else {
            let mut headers = HeaderMap::new();
            headers.insert(
                "x-codex-exclude",
                HeaderValue::from_static(" usage.attribution, tools "),
            );
            let response = forward_with_attempt(
                fixture.state.clone(),
                ProxyRoute::CodexResponses,
                None,
                headers,
                Bytes::from_static(br#"{"model":"gpt-5.4","input":"hi","stream":false}"#),
                ForwardAttemptContext {
                    execution: Some(fixture.execution.clone()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
            let body = collect_response_body(response).await;
            assert_eq!(
                serde_json::from_slice::<Value>(&body).unwrap()["usage"]["total_tokens"],
                3
            );
            let observations = fixture.observations.lock().unwrap();
            assert_eq!(observations.len(), 2);
            for entry in observations.iter() {
                assert_eq!(entry.headers["x-codex-exclude"], "tools,usage.attribution");
            }
            assert_eq!(
                observations[1].headers["authorization"],
                format!("Bearer {token}")
            );
        }
        assert_eq!(refresh.requests.load(Ordering::SeqCst), 1);
        refresh.server.abort();
    }
}

fn codex_r6_completed_for_behavior(id: &str, behavior: CodexR6Behavior) -> Value {
    let mut event = codex_r6_completed(id);
    if matches!(behavior, CodexR6Behavior::ToolComplete) {
        event["response"]["output"].as_array_mut().unwrap().insert(0,json!({"id":"fc_lookup","type":"function_call","call_id":"call_lookup","name":"lookup","arguments":"{\"id\":\"business-id\"}","status":"completed"}));
    }
    event
}

fn codex_r6_assert_tool_and_usage(response: &Value) {
    let output = response["output"].as_array().unwrap();
    let tool = output
        .iter()
        .find(|item| item["type"] == "function_call")
        .expect("excluded tools definitions must not remove generated calls");
    assert_eq!(tool["call_id"], "call_lookup");
    assert_eq!(tool["name"], "lookup");
    assert_eq!(
        serde_json::from_str::<Value>(tool["arguments"].as_str().unwrap()).unwrap(),
        json!({"id":"business-id"})
    );
    assert_eq!(response["usage"]["input_tokens"], 1);
    assert_eq!(response["usage"]["output_tokens"], 2);
}

#[tokio::test]
async fn codex_r6_exclusion_preserves_generated_tools_and_usage_on_all_transports() {
    let fixture =
        CodexR6Fixture::with_behavior("r6-codex-exclude-tools", CodexR6Behavior::ToolComplete)
            .await;
    for stream in [false, true] {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-codex-exclude",
            HeaderValue::from_static("tools,usage.attribution"),
        );
        let body = json!({"model":"gpt-5.4","input":"hi","stream":stream,"tools":[{"type":"function","name":"lookup","parameters":{"type":"object","properties":{"id":{"type":"string"}}}}]});
        let response = forward_with_attempt(
            fixture.state.clone(),
            ProxyRoute::CodexResponses,
            None,
            headers,
            Bytes::from(body.to_string()),
            ForwardAttemptContext {
                execution: Some(fixture.execution.clone()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let bytes = collect_response_body(response).await;
        let response = if stream {
            String::from_utf8(bytes.to_vec())
                .unwrap()
                .lines()
                .filter_map(|line| line.strip_prefix("data:"))
                .filter_map(|raw| serde_json::from_str::<Value>(raw.trim()).ok())
                .find(|event| event["type"] == "response.completed")
                .unwrap()["response"]
                .clone()
        } else {
            serde_json::from_slice::<Value>(&bytes).unwrap()
        };
        codex_r6_assert_tool_and_usage(&response);
    }
    let events = fixture
        .turn_with_exclude(None, Some("tools,usage.attribution"), None, "tool-trace")
        .await;
    codex_r6_assert_tool_and_usage(&events.last().unwrap()["response"]);
}
