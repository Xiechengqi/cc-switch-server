// Independent synthetic requests through the production forwarder and WS bridge.
fn grok_r6_business_schema() -> Value {
    json!({
        "type":"object",
        "properties":{
            "encrypted_content":{"type":"string"},
            "external_web_access":{"type":"boolean"},
            "background":{"type":"boolean"},
            "stream_options":{"type":"object"},
            "record":{
                "type":"object",
                "properties":{"id":{"type":"string"}},
                "default":{"id":"msg_invoice_42"},
                "examples":[{"id":"resp_customer_42"}]
            }
        },
        "required":["encrypted_content","external_web_access"],
        "additionalProperties":false
    })
}

#[tokio::test]
async fn grok_r6_http_preserves_schema_business_keys_on_all_surfaces() {
    let observations = Arc::new(StdMutex::new(Vec::<Value>::new()));
    let seen = observations.clone();
    let app = axum::Router::new().fallback(axum::routing::post(
        move |axum::Json(body): axum::Json<Value>| {
            let seen = seen.clone();
            async move {
                seen.lock().unwrap().push(body.clone());
                if body.get("stream").and_then(Value::as_bool) == Some(true) {
                    (
                        [(CONTENT_TYPE, "text/event-stream")],
                        format!("{GROK_CORE03_CONTENT}{GROK_CORE03_TERMINAL}"),
                    )
                        .into_response()
                } else {
                    axum::Json(json!({"id":"resp_r6_schema","object":"response","status":"completed","model":"grok-4.6","output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"hello"}]}],"usage":{"input_tokens":1,"output_tokens":1}})).into_response()
                }
            }
        },
    ));
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let state = forwarder_test_state("grok-r6-schema-surfaces");
    let execution = install_grok_test_execution(
        &state,
        "grok-r6-schema-surfaces",
        format!("http://{address}/v1"),
        None,
        "synthetic-access",
        None,
        &[],
    )
    .await;
    let id = execution.stored.provider.id.clone();
    let provider = execution.stored.provider.clone();
    state
        .mutate_providers_immediate(move |providers| {
            providers.upsert(AppKind::Claude, provider.clone());
            providers.upsert(AppKind::Gemini, provider);
        })
        .await
        .unwrap();
    for app in [AppKind::Claude, AppKind::Gemini] {
        state
            .override_provider_runtime_endpoint_for_test(app, &id, format!("http://{address}/v1"))
            .await
            .unwrap();
    }
    let schema = grok_r6_business_schema();
    for route in [
        ProxyRoute::ClaudeMessages,
        ProxyRoute::CodexChatCompletions,
        ProxyRoute::CodexResponses,
        ProxyRoute::Gemini,
    ] {
        for streaming in [false, true] {
            let attempt = claude_timing_attempt(&state, route, &id).await;
            let (path, body) = grok_core03_body(route, streaming);
            let mut body: Value = serde_json::from_slice(&body).unwrap();
            body["store"] = json!(false);
            let opaque_history = json!([
                {"type":"reasoning","encrypted_content":"opaque-future-format==","summary":[]},
                {"type":"compaction","encrypted_content":"opaque-compaction"},
                {"type":"message","role":"user","content":"hello"}
            ]);
            if route == ProxyRoute::CodexResponses {
                body["input"] = opaque_history.clone();
            }
            body["tools"] = match route {
                ProxyRoute::ClaudeMessages => {
                    json!([{"name":"save_record","input_schema":schema}])
                }
                ProxyRoute::CodexChatCompletions => {
                    json!([{"type":"function","function":{"name":"save_record","parameters":schema}}])
                }
                ProxyRoute::Gemini => {
                    json!([{"functionDeclarations":[{"name":"save_record","parameters":schema}]}])
                }
                _ => json!([{"type":"function","name":"save_record","parameters":schema}]),
            };
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
            assert_eq!(response.status(), StatusCode::OK);
            let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
                .await
                .unwrap();
            assert!(String::from_utf8_lossy(&bytes).contains("hello"));
            let observed = observations.lock().unwrap();
            let sent = observed.last().unwrap();
            assert_eq!(
                sent.pointer("/tools/0/parameters"),
                Some(&schema),
                "{route:?}, stream={streaming}: {sent}"
            );
            if route == ProxyRoute::CodexResponses {
                assert_eq!(sent["input"], opaque_history);
            }
        }
    }
    assert_eq!(observations.lock().unwrap().len(), 8);
    server.abort();
}

#[tokio::test]
async fn grok_r6_websocket_preserves_business_keys_and_opaque_history() {
    let observations = Arc::new(StdMutex::new(Vec::<Value>::new()));
    let seen = observations.clone();
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let address = listener.local_addr().unwrap();
    let upstream = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
        for _ in 0..3 {
            let payload = socket.next().await.unwrap().unwrap().into_data();
            seen.lock()
                .unwrap()
                .push(serde_json::from_slice(&payload).unwrap());
            socket
                .send(TungsteniteMessage::Text(
                    json!({
                        "type":"response.completed",
                        "response":{
                            "id":"resp_r6_ws",
                            "status":"completed",
                            "output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"hello"}]}],
                            "usage":{"input_tokens":1,"output_tokens":1}
                        }
                    })
                    .to_string(),
                ))
                .await
                .unwrap();
        }
    });
    let state = forwarder_test_state("grok-r6-ws-opaque");
    let execution = install_grok_test_execution(
        &state,
        "grok-r6-ws-opaque",
        format!("http://{address}/v1"),
        Some(format!("ws://{address}/ws")),
        "synthetic-access",
        None,
        &[GrokAccountCapability::Websocket],
    )
    .await;
    let (bridge, bridge_server) =
        spawn_test_grok_responses_bridge(state, execution, "grok-r6-ws", None).await;
    let (mut client, _) = tokio_tungstenite::connect_async(format!("ws://{bridge}/bridge"))
        .await
        .unwrap();
    let schema = grok_r6_business_schema();
    let history = json!([
        {"type":"reasoning","encrypted_content":"opaque-future-format==","summary":[]},
        {"type":"compaction","encrypted_content":"opaque-compaction"},
        {"type":"message","role":"user","content":"hello"}
    ]);
    for envelope in ["bare", "nested", "flat"] {
        let mut request = json!({"model":"grok-4.6","store":false,"stream":true,"background":true,"stream_options":{"include_usage":true},"input":history,"tools":[{"type":"function","name":"save_record","parameters":schema}],"tool_choice":{"type":"function","name":"save_record"}});
        if envelope == "nested" {
            request = json!({"type":"response.create","response":request});
        } else if envelope == "flat" {
            request["type"] = json!("response.create");
        }
        client
            .send(TungsteniteMessage::Text(request.to_string()))
            .await
            .unwrap();
        loop {
            let payload = tokio::time::timeout(Duration::from_secs(3), client.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap()
                .into_data();
            let value: Value = serde_json::from_slice(&payload).unwrap();
            assert_ne!(value["type"], "error", "{value}");
            if value["type"] == "response.completed" {
                break;
            }
        }
        let observed = observations.lock().unwrap();
        let sent = observed.last().unwrap();
        let sent = sent.get("response").unwrap_or(sent);
        assert_eq!(sent["input"], history, "{envelope}: {sent}");
        assert_eq!(sent.pointer("/tools/0/parameters"), Some(&schema));
        assert_eq!(sent["tool_choice"]["name"], "save_record");
        assert!(sent.get("stream_options").is_none());
        assert!(sent.get("background").is_none());
        assert_eq!(sent["store"], true);
    }
    assert_eq!(observations.lock().unwrap().len(), 3);
    client.close(None).await.unwrap();
    bridge_server.abort();
    upstream.await.unwrap();
}

#[tokio::test]
async fn grok_r6_invalid_native_controls_fail_before_network() {
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    let app = axum::Router::new().fallback(axum::routing::post(move || {
        let seen = seen.clone();
        async move {
            seen.fetch_add(1, Ordering::SeqCst);
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }));
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let state = forwarder_test_state("grok-r6-local-control-rejection");
    let execution = install_grok_test_execution(
        &state,
        "grok-r6-local-control-rejection",
        format!("http://{address}/v1"),
        None,
        "synthetic-access",
        None,
        &[],
    )
    .await;
    for (input, tools, expected) in [
        (
            json!("hello"),
            json!([{"type":"web_search","external_web_access":false}]),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            json!("hello"),
            json!([{"type":"web_search_preview","external_web_access":false}]),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            json!([{"type":"reasoning","encrypted_content":null}]),
            json!([]),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!([{"type":"compaction","encrypted_content":" \t"}]),
            json!([]),
            StatusCode::BAD_REQUEST,
        ),
    ] {
        let error = forward_for_test_surface(
            state.clone(),
            ProxyRoute::CodexResponses,
            execution.stored.provider.id.clone(),
            None,
            HeaderMap::new(),
            Bytes::from(json!({"model":"grok-4.6","input":input,"tools":tools}).to_string()),
        )
        .await
        .unwrap_err();
        assert_eq!(error.status, expected, "{error:?}");
    }
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let account = state
        .find_account_by_id(execution.managed_account_id().unwrap())
        .await
        .unwrap();
    assert!(!account.needs_relogin);
    assert!(account.rate_limited_until.is_none());
    server.abort();
}

#[tokio::test]
async fn grok_r6_upstream_errors_do_not_replay_or_infer_model_permissions() {
    // Build text is deliberately an untrusted diagnostic on this direct-XAI
    // rail, not authority to shrink an account limit or to rotate credentials.
    for (suffix, status, error_body) in [
        (
            "client-cipher",
            StatusCode::BAD_REQUEST,
            json!({"error":{"code":"invalid_encrypted_content","message":"could not decrypt the provided encrypted_content"}}),
        ),
        (
            "unknown-429",
            StatusCode::TOO_MANY_REQUESTS,
            json!({"error":{"code":"rate_limit_exceeded","message":"rate limited"}}),
        ),
        (
            "build-diagnostic",
            StatusCode::TOO_MANY_REQUESTS,
            json!({"code":"subscription:free-usage-exhausted","error":"You've used all the included free usage for model grok-4.6"}),
        ),
    ] {
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = calls.clone();
        let identity = Arc::new(StdMutex::new(None::<HeaderMap>));
        let captured = identity.clone();
        let app = axum::Router::new().fallback(axum::routing::post(
            move |headers: HeaderMap, body: Bytes| {
                let seen = seen.clone();
                let captured = captured.clone();
                let error_body = error_body.clone();
                async move {
                    seen.fetch_add(1, Ordering::SeqCst);
                    *captured.lock().unwrap() = Some(headers);
                    let body: Value = serde_json::from_slice(&body).unwrap();
                    assert_eq!(
                        body["input"][0]["encrypted_content"],
                        "opaque-client-history=="
                    );
                    (status, [("retry-after", "60")], axum::Json(error_body))
                }
            },
        ));
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let state = forwarder_test_state(&format!("grok-r6-{suffix}"));
        let execution = install_grok_test_execution(
            &state,
            &format!("grok-r6-{suffix}"),
            format!("http://{address}/v1"),
            None,
            "synthetic-access",
            None,
            &[],
        )
        .await;
        let result = forward_for_test_surface(state.clone(), ProxyRoute::CodexResponses, execution.stored.provider.id.clone(), None, HeaderMap::new(), Bytes::from_static(br#"{"model":"grok-4.6","input":[{"type":"reasoning","encrypted_content":"opaque-client-history==","summary":[]},{"type":"message","role":"user","content":"hello"}]}"#)).await;
        let actual_status = match result {
            Ok(response) => {
                let actual = response.status();
                let _ = axum::body::to_bytes(response.into_body(), 1024 * 1024)
                    .await
                    .unwrap();
                actual
            }
            Err(error) => error.status,
        };
        assert_eq!(actual_status, status);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let account = state
            .find_account_by_id(execution.managed_account_id().unwrap())
            .await
            .unwrap();
        assert!(!account.needs_relogin);
        assert_eq!(
            account.rate_limited_until.is_some(),
            status == StatusCode::TOO_MANY_REQUESTS
        );
        let captured = identity.lock().unwrap();
        let captured = captured.as_ref().unwrap();
        assert_eq!(captured["x-grok-client-identifier"], "grok-shell");
        assert_eq!(
            captured["x-grok-client-version"],
            crate::domain::grok_cli::grok_cli_version()
        );
        assert!(!captured.contains_key("x-grok-conv-group-id"));
        assert!(!captured.contains_key("x-grok-transient-retry"));
        assert!(!captured.contains_key("idempotency-key"));
        server.abort();
    }
}
