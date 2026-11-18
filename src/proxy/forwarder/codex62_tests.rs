#[tokio::test]
async fn codex62_output_budgets_survive_http_compact_and_websocket_boundaries() {
    let stored = stored_provider(
        AppKind::Codex,
        ProviderType::CodexOAuth,
        json!({}),
        Some("budget-account"),
    );
    for (field, count, expected) in [
        ("max_output_tokens", 1024, 1024),
        ("max_completion_tokens", 64, 64),
        ("max_tokens", 1, 16),
    ] {
        let mut request =
            json!({"model": "gpt-6.2-sol", "input": [{"role": "user", "content": "hi"}]});
        request[field] = json!(count);
        let bytes = Bytes::from(request.to_string());
        let http = normalize_codex_oauth_responses_body_bytes(
            &bytes,
            None,
            CodexImageToolStripPolicy::Never,
        )
        .unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&http).unwrap()["max_output_tokens"],
            expected
        );
        let compact = normalize_codex_oauth_compact_body_bytes(&bytes).unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&compact).unwrap()["max_output_tokens"],
            expected
        );
        let (message, _) = prepare_codex_responses_websocket_request(
            TungsteniteMessage::Text(
                json!({"type": "response.create", "response": request}).to_string(),
            ),
            &stored,
            false,
            None,
            &super::super::codex_request_policy::extract_intent(&request),
            None,
        )
        .await
        .unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(websocket_message_payload(&message).unwrap()).unwrap()
                ["response"]["max_output_tokens"],
            expected
        );
    }
    for value in [json!(-1), json!(0), json!("64"), json!(1.5)] {
        let bytes = Bytes::from(json!({"max_output_tokens": value}).to_string());
        assert!(normalize_codex_oauth_responses_body_bytes(
            &bytes,
            None,
            CodexImageToolStripPolicy::Never
        )
        .is_err());
        assert!(normalize_codex_oauth_compact_body_bytes(&bytes).is_err());
    }
}

#[test]
fn codex62_native_tools_encrypted_agents_and_client_ultra_markers_remain_opaque() {
    let request = json!({
        "model": "gpt-6.2-sol",
        "instructions": "client provided instructions",
        "input": [
            {"type": "message", "role": "developer", "content": [{"type": "input_text", "text": "<multi_agent_mode>client-defined</multi_agent_mode>"}]},
            {"type": "agent_message", "author": "agent-a", "recipient": "agent-b", "encrypted_content": "opaque-envelope", "content": [{"type": "opaque_payload", "cache_control": {"nested": true}}]},
            {"type": "custom_tool_call", "call_id": "call-custom", "name": "apply_patch", "input": "literal patch payload"},
            {"type": "custom_tool_call_output", "call_id": "call-custom", "output": "ok"}
        ],
        "tools": [
            {"type": "namespace", "name": "functions", "tools": [{"type": "function", "name": "lookup", "parameters": {"type": "object", "properties": {"query": {"type": "string"}}}}]},
            {"type": "custom", "name": "apply_patch", "format": {"type": "grammar", "syntax": "lark", "definition": "start: /.+/"}}
        ]
    });
    let normalized = normalize_codex_oauth_responses_body(
        request.clone(),
        None,
        CodexImageToolStripPolicy::Never,
    );
    assert_eq!(normalized["instructions"], request["instructions"]);
    assert_eq!(normalized["input"], request["input"]);
    assert_eq!(normalized["tools"], request["tools"]);
}

#[test]
fn codex62_capacity_hint_never_retries_before_the_upstream_delay() {
    assert!(codex::capacity_retry_delay_with_hint(10_000, 0, Some(326_000)).is_none());
    let delay = codex::capacity_retry_delay_with_hint(10_000, 0, Some(2_500)).unwrap();
    assert!(delay >= Duration::from_millis(2500));
    let failure = SemanticFailure {
        origin: FailureOrigin::Provider,
        code: "server_is_overloaded".into(),
        message: "wait".into(),
        retry_after_ms: Some(326_000),
    };
    assert_eq!(
        codex::capacity_shed_proxy_error(&failure).retry_after_seconds(),
        Some(326)
    );
    let json: Value =
        serde_json::from_str(&synthesize_openai_capacity_shed_failed_json(&failure)).unwrap();
    assert_eq!(json["headers"]["Retry-After"], "326");
    let raw = json!({"type": "response.failed",
        "headers": {"retry-after-ms": "250", "x-trace": "keep"}, "response": {
        "error": {"code": "server_is_overloaded", "message": "wait",
            "headers": {"Retry-After": "1"}}
    }})
    .to_string();
    let (encoded, _) = super::super::openai_capacity_shed::sanitize_openai_capacity_shed_json_text_with_retry_after(
        &raw, Some(326_000),
    );
    let encoded: Value = serde_json::from_str(&encoded).unwrap();
    assert_eq!(encoded["headers"]["Retry-After"], "326");
    assert!(encoded["headers"].get("retry-after-ms").is_none());
    assert_eq!(encoded["headers"]["x-trace"], "keep");
    assert_eq!(encoded["response"]["error"]["headers"]["Retry-After"], "1");
    let success = r#"{"type":"response.completed","response":{"status":"completed"}}"#;
    assert_eq!(
        super::super::openai_capacity_shed::sanitize_openai_capacity_shed_json_text_with_retry_after(success, Some(326_000)),
        (success.to_string(), false)
    );
}

#[tokio::test]
async fn codex62_manifest_refresh_is_bound_singleflight_and_uses_one_identity_version() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let address = listener.local_addr().unwrap();
    let calls = std::sync::Arc::new(AtomicUsize::new(0));
    let route_calls = calls.clone();
    let app = axum::Router::new().route(
        "/models",
        axum::routing::get(move |headers: HeaderMap, uri: axum::http::Uri| {
            let calls = route_calls.clone();
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                assert_eq!(
                    headers["authorization"],
                    "Bearer codex62-manifest-access-token"
                );
                assert_eq!(headers["chatgpt-account-id"], "codex62-manifest-workspace");
                let version = crate::codex_identity::configured_version();
                assert_eq!(headers["version"], version.as_str());
                assert!(headers["user-agent"]
                    .to_str()
                    .unwrap()
                    .contains(&format!("/{version} ")));
                assert_eq!(
                    url::form_urlencoded::parse(uri.query().unwrap().as_bytes())
                        .find(|(key, _)| key == "client_version")
                        .unwrap()
                        .1,
                    version
                );
                axum::Json(
                    json!({"models":[{"slug":"account-only-model","use_responses_lite":true}]}),
                )
            }
        }),
    );
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let (state, mut execution) =
        codex_bridge_test_context("codex62-manifest", format!("http://{address}")).await;
    std::sync::Arc::make_mut(&mut execution.plan)
        .driver_options
        .insert(
            "testCodexModelsUrl".into(),
            json!(format!("http://{address}/models")),
        );
    tokio::join!(
        state.ensure_codex_model_capabilities(&execution),
        state.ensure_codex_model_capabilities(&execution)
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    state.ensure_codex_model_capabilities(&execution).await;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    server.abort();
}

#[tokio::test]
async fn codex62_capacity_http_and_sse_return_long_wait_without_replaying() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    for sse in [false, true] {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let calls = std::sync::Arc::new(AtomicUsize::new(0));
        let route_calls = calls.clone();
        let app = axum::Router::new().fallback(move || {
            let calls = route_calls.clone();
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                let body = if sse {
                    format!("event: response.failed\ndata: {}\n\n", json!({"type":"response.failed","response":{"status":"failed","error":{"code":"server_is_overloaded","message":"wait","headers":{"Retry-After":"326"}}}}))
                } else {
                    json!({"error":{"code":"server_is_overloaded","message":"wait"}}).to_string()
                };
                let mut response = Response::builder()
                    .status(if sse {StatusCode::OK} else {StatusCode::SERVICE_UNAVAILABLE})
                    .header(CONTENT_TYPE, if sse {"text/event-stream"} else {"application/json"});
                if !sse { response = response.header("retry-after", "326"); }
                response.body(Body::from(body)).unwrap()
            }
        });
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let (state, execution) = codex_bridge_test_context(
            &format!("codex62-capacity-{sse}"),
            format!("http://{address}"),
        )
        .await;
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            forward_with_attempt(
                state,
                ProxyRoute::CodexResponses,
                None,
                HeaderMap::new(),
                Bytes::from_static(br#"{"model":"gpt-6.2-sol","input":"hi","stream":false}"#),
                ForwardAttemptContext {
                    execution: Some(execution),
                    ..Default::default()
                },
            ),
        )
        .await
        .expect("long upstream delays must be returned to the caller");
        let error = result.unwrap_err();
        assert_eq!(
            error.retry_after_seconds(),
            Some(326),
            "SSE={sse}: {error:?}"
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "SSE={sse}: must not replay early"
        );
        server.abort();
    }
}
