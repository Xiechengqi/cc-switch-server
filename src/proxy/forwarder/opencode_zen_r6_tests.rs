fn zen_r6_events(bytes: &[u8]) -> Vec<Value> {
    String::from_utf8_lossy(bytes)
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .filter_map(|raw| serde_json::from_str(raw).ok())
        .collect()
}

fn zen_r6_reasoning(bytes: &[u8], route: ProxyRoute, stream: bool) -> String {
    if stream {
        return zen_r6_events(bytes)
            .iter()
            .filter_map(|event| {
                event
                    .pointer("/delta/thinking")
                    .and_then(Value::as_str)
                    .or_else(|| {
                        event
                            .pointer("/choices/0/delta/reasoning_content")
                            .and_then(Value::as_str)
                    })
                    .or_else(|| {
                        event["type"]
                            .as_str()
                            .filter(|kind| kind.contains("reasoning"))
                            .and_then(|_| event["delta"].as_str())
                    })
            })
            .collect();
    }
    let document: Value = serde_json::from_slice(bytes).unwrap();
    match route {
        ProxyRoute::ClaudeMessages => document["content"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|block| block["thinking"].as_str())
            .collect(),
        ProxyRoute::CodexChatCompletions => document["choices"][0]["message"]["reasoning_content"]
            .as_str()
            .unwrap()
            .to_owned(),
        ProxyRoute::CodexResponses => document["output"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|item| item["type"] == "reasoning")
            .flat_map(|item| item["summary"].as_array().into_iter().flatten())
            .filter_map(|part| part["text"].as_str())
            .collect(),
        _ => unreachable!(),
    }
}

async fn zen_r6_origin(wire: String) -> (String, tokio::task::JoinHandle<()>) {
    let router = axum::Router::new().fallback(axum::routing::post(move || {
        let wire = wire.clone();
        async move {
            Response::builder()
                .header(CONTENT_TYPE, "text/event-stream")
                .body(Body::from(wire))
                .unwrap()
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    (origin, task)
}

fn zen_r6_wire(delta: Value, finish: Option<&str>) -> String {
    format!(
        "data: {}\n\n",
        json!({"id":"chat-r6","model":"chat-free","choices":[{"index":0,"delta":delta,"finish_reason":finish}]})
    )
}

async fn zen_r6_options(
    state: &ServerState,
    route: ProxyRoute,
    stream: bool,
) -> OpencodeZenForwardOptions {
    let app = if route == ProxyRoute::ClaudeMessages {
        AppKind::Claude
    } else {
        AppKind::Codex
    };
    let body = if route == ProxyRoute::CodexResponses {
        json!({"model":"chat-free","stream":stream,"input":"hello"})
    } else {
        json!({"model":"chat-free","stream":stream,"max_tokens":1024,"messages":[{"role":"user","content":"hello"}]})
    };
    let mut options = zen_test_options_for(state, app, body).await;
    options.route = route;
    super::super::opencode_zen::store_catalog_payload(&json!({"data":[{
        "id":"chat-free","type":"chat","limit":{"output":8192},
        "cost":{"text":{"input":{"price":0},"output":{"price":0}}},
        "api":{"chat-completions":{"base_url":"https://opencode.ai/inference/openai/v1"}},
        "capabilities":{"reasoning":{"supported":true},"tools":{"supported":true}}
    }]}));
    options
}

#[tokio::test]
async fn opencode_zen_r6_aliases_survive_formal_forwarder_on_three_surfaces_and_both_modes() {
    use super::super::opencode_zen as zen;
    let _lock = zen::CATALOG_TEST_LOCK.lock().await;
    for route in [
        ProxyRoute::ClaudeMessages,
        ProxyRoute::CodexChatCompletions,
        ProxyRoute::CodexResponses,
    ] {
        for stream in [false, true] {
            for fields in [
                json!({"reasoning_text":"TRACE_ALPHA"}),
                json!({"reasoning_content":"","reasoning_text":"TRACE_ALPHA"}),
                json!({"reasoning_content":" ","reasoning_text":"TRACE_ALPHA"}),
                json!({"reasoning_content":"","reasoning_text":"","reasoning":"TRACE_ALPHA"}),
                json!({"reasoning_content":"TRACE_ALPHA","reasoning_text":"TRACE_ALPHA","reasoning":"TRACE_ALPHA"}),
                json!({"reasoning_content":"TRACE_ALPHA","reasoning_text":"discard-lower-priority","reasoning":"discard-lowest-priority"}),
            ] {
                let wire = [
                    zen_r6_wire(fields, None),
                    zen_r6_wire(json!({"reasoning_text":" "}), None),
                    zen_r6_wire(json!({"reasoning":"BETA","content":"ANSWER"}), Some("stop")),
                    "data: [DONE]\n\n".into(),
                ]
                .concat();
                let (origin, server) = zen_r6_origin(wire).await;
                let state = forwarder_test_state("zen-r6-alias");
                let options = zen_r6_options(&state, route, stream).await;
                let response = forward_opencode_zen_at(options, &origin).await.unwrap();
                assert_eq!(response.status(), StatusCode::OK);
                let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
                    .await
                    .unwrap();
                let text = String::from_utf8_lossy(&bytes);
                assert!(
                    text.contains("TRACE_ALPHA"),
                    "{route:?} stream={stream}: {text}"
                );
                assert_eq!(
                    zen_r6_reasoning(&bytes, route, stream),
                    "TRACE_ALPHA BETA",
                    "{route:?} stream={stream}"
                );
                assert!(text.contains("ANSWER"));
                assert!(!text.contains("discard-lower-priority"));
                assert!(!text.contains("discard-lowest-priority"));
                assert!(!zen_r6_events(&bytes)
                    .iter()
                    .any(|value| value.get("error").is_some() || value["type"] == "error"));
                let usage = state.usage_snapshot().await;
                let log = usage.request_detail("zen-receipt").unwrap();
                assert_eq!(log.attempt_count, 1);
                assert!(log.first_token_ms.is_some());
                server.abort();
            }
        }
    }
}

#[tokio::test]
async fn opencode_zen_r6_alias_normalization_preserves_native_tool_argument_values() {
    use super::super::opencode_zen as zen;
    let _lock = zen::CATALOG_TEST_LOCK.lock().await;
    let arguments = json!({"reasoning_text":"literal", "nested":{"reasoning_content":"opaque", "thinking":"opaque", "reasoning":{"encrypted_content":"unaltered"}}});
    let encoded = arguments.to_string();
    let (first, last) = encoded.split_at(20);
    for route in [
        ProxyRoute::ClaudeMessages,
        ProxyRoute::CodexChatCompletions,
        ProxyRoute::CodexResponses,
    ] {
        for stream in [false, true] {
            let wire = [
                zen_r6_wire(json!({"reasoning_content":"","reasoning_text":"TRACE_ALPHA","tool_calls":[{"index":0,"id":"call-r6","type":"function","function":{"name":"lookup","arguments":first}}]}), None),
                zen_r6_wire(json!({"tool_calls":[{"index":0,"function":{"arguments":last}}]}), Some("tool_calls")),
                "data: [DONE]\n\n".into(),
            ].concat();
            let (origin, server) = zen_r6_origin(wire).await;
            let state = forwarder_test_state("zen-r6-tool-opaque");
            let mut options = zen_r6_options(&state, route, stream).await;
            let mut request: Value = serde_json::from_slice(&options.body).unwrap();
            let parameters = json!({"type":"object","properties":{"reasoning_text":{"type":"string"},"nested":{"type":"object"}}});
            request["tools"] = match route {
                ProxyRoute::ClaudeMessages => json!([{"name":"lookup","input_schema":parameters}]),
                ProxyRoute::CodexChatCompletions => {
                    json!([{"type":"function","function":{"name":"lookup","parameters":parameters}}])
                }
                _ => json!([{"type":"function","name":"lookup","parameters":parameters}]),
            };
            options.body = Bytes::from(request.to_string());
            let response = forward_opencode_zen_at(options, &origin).await.unwrap();
            let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
                .await
                .unwrap();
            assert_eq!(zen_r6_reasoning(&bytes, route, stream), "TRACE_ALPHA");
            let actual = if stream {
                let raw: String = zen_r6_events(&bytes)
                    .iter()
                    .filter_map(|event| {
                        event
                            .pointer("/delta/partial_json")
                            .and_then(Value::as_str)
                            .or_else(|| {
                                event
                                    .pointer("/choices/0/delta/tool_calls/0/function/arguments")
                                    .and_then(Value::as_str)
                            })
                            .or_else(|| {
                                (event["type"] == "response.function_call_arguments.delta")
                                    .then(|| event["delta"].as_str())
                                    .flatten()
                            })
                    })
                    .collect();
                serde_json::from_str::<Value>(&raw).unwrap()
            } else {
                let value: Value = serde_json::from_slice(&bytes).unwrap();
                match route {
                    ProxyRoute::ClaudeMessages => value["content"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .find(|block| block["type"] == "tool_use")
                        .unwrap()["input"]
                        .clone(),
                    ProxyRoute::CodexChatCompletions => serde_json::from_str(
                        value["choices"][0]["message"]["tool_calls"][0]["function"]["arguments"]
                            .as_str()
                            .unwrap(),
                    )
                    .unwrap(),
                    _ => serde_json::from_str(
                        value["output"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .find(|item| item["type"] == "function_call")
                            .unwrap()["arguments"]
                            .as_str()
                            .unwrap(),
                    )
                    .unwrap(),
                }
            };
            assert_eq!(actual, arguments, "{route:?} stream={stream}");
            server.abort();
        }
    }
}

#[tokio::test]
async fn opencode_zen_r6_reasoning_text_after_finish_fails_on_formal_forwarder() {
    use super::super::opencode_zen as zen;
    let _lock = zen::CATALOG_TEST_LOCK.lock().await;
    for stream in [false, true] {
        let wire = [
            zen_r6_wire(json!({"content":"ANSWER"}), Some("stop")),
            zen_r6_wire(json!({"reasoning_text":"LATE"}), None),
            "data: [DONE]\n\n".into(),
        ]
        .concat();
        let (origin, server) = zen_r6_origin(wire).await;
        let state = forwarder_test_state("zen-r6-late");
        let options = zen_r6_options(&state, ProxyRoute::CodexChatCompletions, stream).await;
        match forward_opencode_zen_at(options, &origin).await {
            Err(error) => assert_eq!(error.status, StatusCode::BAD_GATEWAY),
            Ok(response) => {
                let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
                    .await
                    .unwrap();
                assert!(
                    zen_r6_events(&bytes)
                        .iter()
                        .any(|value| value.get("error").is_some()),
                    "late reasoning must not finish successfully: {}",
                    String::from_utf8_lossy(&bytes)
                );
            }
        }
        server.abort();
    }
}

#[tokio::test]
async fn opencode_zen_r6_reasoning_text_checkpoint_uses_one_existing_continuation() {
    use super::super::opencode_zen as zen;
    let _lock = zen::CATALOG_TEST_LOCK.lock().await;
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let router = axum::Router::new().fallback(axum::routing::post(move |body: Bytes| {
        let turn = count.fetch_add(1, Ordering::SeqCst);
        async move {
            assert!(turn < 2);
            let request: Value = serde_json::from_slice(&body).unwrap();
            if turn == 1 { assert!(request["messages"].to_string().contains("CHECKPOINT")); }
            let delta = if turn == 0 { json!({"reasoning_text":"CHECKPOINT"}) } else { json!({"content":"FINAL_ANSWER"}) };
            let data = [zen_r6_wire(delta, Some("stop")), format!("data: {}\n\n", json!({"choices":[],"usage":{"prompt_tokens":2,"completion_tokens":3,"total_tokens":5}})), "data: [DONE]\n\n".into()].concat();
            Response::builder().header(CONTENT_TYPE,"text/event-stream").body(Body::from(data)).unwrap()
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let state = forwarder_test_state("zen-r6-checkpoint");
    let options = zen_r6_options(&state, ProxyRoute::CodexChatCompletions, false).await;
    let response = forward_opencode_zen_at(options, &origin).await.unwrap();
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["choices"][0]["message"]["content"], "FINAL_ANSWER");
    assert_eq!(value["usage"]["prompt_tokens"], 4);
    assert_eq!(value["usage"]["completion_tokens"], 6);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    let usage = state.usage_snapshot().await;
    assert_eq!(
        usage.request_detail("zen-receipt").unwrap().attempt_count,
        2
    );
    server.abort();
}

#[tokio::test]
async fn opencode_zen_r6_reasoning_text_satisfies_semantic_deadline_in_both_modes() {
    use super::super::opencode_zen as zen;
    let _lock = zen::CATALOG_TEST_LOCK.lock().await;
    let router = axum::Router::new().fallback(axum::routing::post(|| async {
        Response::builder().header(CONTENT_TYPE,"text/event-stream").body(Body::from_stream(async_stream::stream! {
            yield Ok::<Bytes,std::io::Error>(Bytes::from(zen_r6_wire(json!({"reasoning_text":"EARLY"}),None)));
            tokio::time::sleep(Duration::from_millis(500)).await;
            yield Ok(Bytes::from([zen_r6_wire(json!({"content":"LATE_ANSWER"}),Some("stop")),"data: [DONE]\n\n".into()].concat()));
        })).unwrap()
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    for stream in [false, true] {
        let state = forwarder_test_state("zen-r6-first");
        let mut options = zen_r6_options(&state, ProxyRoute::CodexResponses, stream).await;
        Arc::make_mut(&mut options.execution.plan)
            .transport_policy
            .stream_first_byte_timeout_ms = Some(250);
        options.started = Instant::now();
        let response = forward_opencode_zen_at(options, &origin).await.unwrap();
        let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        assert!(String::from_utf8_lossy(&bytes).contains("LATE_ANSWER"));
        let usage = state.usage_snapshot().await;
        let log = usage.request_detail("zen-receipt").unwrap();
        assert!(log.first_token_ms.is_some());
        assert_ne!(log.stream_status.as_deref(), Some("failed"));
    }
    server.abort();
}
