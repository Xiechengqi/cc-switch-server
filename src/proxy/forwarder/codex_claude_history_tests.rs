// A strict loopback upstream rejects invalid roles, cache metadata and tool flags without
// using a real OAuth credential or contacting OpenAI.
#[tokio::test]
async fn responses_message_text_types_claude_history_passes_strict_oauth_upstream() {
    assert_claude_history_passes_strict_oauth_upstream(false).await;
}

async fn assert_claude_history_passes_strict_oauth_upstream(tool_result_errors: bool) {
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let address = listener.local_addr().unwrap();
    let observations = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let observed = std::sync::Arc::clone(&observations);
    let upstream = axum::Router::new().fallback(move |headers: HeaderMap, body: Bytes| {
        let observed = std::sync::Arc::clone(&observed);
        async move {
            let body: Value = serde_json::from_slice(&body).unwrap();
            let invalid = body["input"].as_array().unwrap().iter().any(|item| {
                item["role"] == "assistant"
                    && item["content"].as_array().is_some_and(|parts| {
                        parts.iter().any(|part| {
                            !matches!(part["type"].as_str(), Some("output_text" | "refusal"))
                        })
                    })
            });
            let invalid_error_flag = body["input"].as_array().unwrap().iter().enumerate().find_map(|(index, item)| {
                (matches!(item["type"].as_str(), Some("function_call_output" | "custom_tool_call_output")) && item.get("is_error").is_some()).then_some(index)
            });
            let invalid_cache = body["input"].as_array().unwrap().iter().enumerate().find_map(|(index,item)| {
                item["content"].as_array().and_then(|parts| parts.iter().position(|part| part.get("cache_control").is_some()))
                    .map(|part| format!("input[{index}].content[{part}].cache_control"))
            });
            observed.lock().unwrap().push((headers, body));
            if let Some(path) = invalid_cache {
                return Response::builder().status(StatusCode::BAD_REQUEST)
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(json!({"error":{"message":format!("Unknown parameter: '{path}'."),"type":"invalid_request_error"}}).to_string())).unwrap();
            }
            if let Some(index) = invalid_error_flag {
                return Response::builder().status(StatusCode::BAD_REQUEST)
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(json!({"error": {"message":format!("Unknown parameter: 'input[{index}].is_error'."), "type":"invalid_request_error"}}).to_string())).unwrap();
            }
            if invalid {
                return Response::builder()
                    .status(StatusCode::BAD_REQUEST)
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(json!({"error":{
                        "message":"Invalid value: 'input_text'. Supported values are: 'output_text' and 'refusal'.",
                        "type":"invalid_request_error"
                    }}).to_string()))
                    .unwrap();
            }
            let item = json!({
                "id":"msg_history", "type":"message", "role":"assistant",
                "content":[{"type":"output_text", "text":"pong", "annotations":[]}]
            });
            let events = [
                json!({"type":"response.created", "response":{
                    "id":"resp_history", "object":"response", "status":"in_progress", "output":[]
                }}),
                json!({"type":"response.output_item.added", "output_index":0, "item":{
                    "id":"msg_history", "type":"message", "role":"assistant", "content":[]
                }}),
                json!({"type":"response.content_part.added", "output_index":0, "item_id":"msg_history",
                    "content_index":0, "part":{"type":"output_text", "text":"", "annotations":[]}}),
                json!({"type":"response.output_text.delta", "output_index":0, "item_id":"msg_history",
                    "content_index":0, "delta":"pong"}),
                json!({"type":"response.output_text.done", "output_index":0, "item_id":"msg_history",
                    "content_index":0, "text":"pong"}),
                json!({"type":"response.content_part.done", "output_index":0, "item_id":"msg_history",
                    "content_index":0, "part":{"type":"output_text", "text":"pong", "annotations":[]}}),
                json!({"type":"response.output_item.done", "output_index":0, "item":item}),
                json!({"type":"response.completed", "response":{
                    "id":"resp_history", "object":"response", "status":"completed", "model":"gpt-6-astra",
                    "output":[item], "usage":{"input_tokens":12, "output_tokens":3, "total_tokens":15}
                }}),
            ];
            let mut wire = events.iter().map(|event| format!("data: {event}\n\n")).collect::<String>();
            wire.push_str("data: [DONE]\n\n");
            Response::builder()
                .status(StatusCode::OK)
                .header(CONTENT_TYPE, "text/event-stream")
                .body(Body::from(wire))
                .unwrap()
        }
    });
    let server = tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });

    // Prove that this upstream actually rejects the pre-fix wire shape.
    let rejected = crate::infra::http::direct_client().unwrap()
        .post(format!("http://{address}/v1/responses"))
        .json(&json!({"input":[{"role":"assistant", "content":[{"type":"input_text", "text":"old"}]}]}))
        .send().await.unwrap();
    assert_eq!(rejected.status(), StatusCode::BAD_REQUEST);
    assert!(rejected.text().await.unwrap().contains("output_text"));

    let rejected = crate::infra::http::direct_client().unwrap()
        .post(format!("http://{address}/v1/responses"))
        .json(&json!({"input":[{"role":"user","content":[{"type":"input_text","text":"old","cache_control":{"type":"ephemeral"}}]}]}))
        .send().await.unwrap();
    assert_eq!(rejected.status(), StatusCode::BAD_REQUEST);
    assert!(rejected
        .text()
        .await
        .unwrap()
        .contains("input[0].content[0].cache_control"));

    if tool_result_errors {
        let rejected = crate::infra::http::direct_client().unwrap()
            .post(format!("http://{address}/v1/responses"))
            .json(&json!({"input":[{"type":"function_call_output","call_id":"a","output":"denied","is_error":false}]}))
            .send().await.unwrap();
        assert_eq!(rejected.status(), StatusCode::BAD_REQUEST);
        assert!(rejected.text().await.unwrap().contains("input[0].is_error"));
    }
    let name = "claude-oauth-message-text-types";
    let endpoint = format!("http://{address}");
    let (state, execution) = codex_bridge_test_context(name, endpoint.clone()).await;
    let mut stored = execution.stored.clone();
    drop(execution);
    stored.app = AppKind::Claude;
    let provider_id = stored.provider.id.clone();
    let accounts = state.accounts_snapshot().await;
    let mut providers = ProviderStore {
        providers: vec![stored],
        ..ProviderStore::default()
    };
    providers.rebuild_runtime_index(&accounts).unwrap();
    state.replace_provider_store_for_test(providers).await;
    state
        .override_provider_runtime_endpoint_for_test(AppKind::Claude, &provider_id, endpoint)
        .await
        .unwrap();

    for stream in [false, true] {
        for block_content in [false, true] {
            let content = |text: &str| {
                if block_content {
                    json!([{"type":"text", "text":text, "cache_control":{"type":"ephemeral"}}])
                } else {
                    json!(text)
                }
            };
            let mut headers = HeaderMap::new();
            headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
            let mut request = json!({
                "model":"gpt-6-astra", "max_tokens":64, "stream":stream, "system":"be helpful",
                "messages":[
                    {"role":"user", "content":content("first")},
                    {"role":"assistant", "content":content("previous answer")},
                    {"role":"user", "content":content("next")}
                ]
            });
            if tool_result_errors {
                request["tools"] =
                    json!([{"name":"lookup","input_schema":{"type":"object","properties":{}}}]);
                request["messages"].as_array_mut().unwrap().extend([
                    json!({"role":"assistant","content":[
                        {"type":"tool_use","id":"a","name":"lookup","input":{}},
                        {"type":"tool_use","id":"b","name":"lookup","input":{}}
                    ]}),
                    json!({"role":"user","content":[
                        {"type":"tool_result","tool_use_id":"a","content":content("denied"),"is_error":true},
                        {"type":"tool_result","tool_use_id":"b","content":"ok","is_error":false}
                    ]})
                ]);
            }
            let response = forward_for_test_surface(
                state.clone(),
                ProxyRoute::ClaudeMessages,
                provider_id.clone(),
                None,
                headers,
                Bytes::from(request.to_string()),
            )
            .await
            .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let content_type = response
                .headers()
                .get(CONTENT_TYPE)
                .unwrap()
                .to_str()
                .unwrap()
                .to_string();
            let wire = collect_response_body(response).await;
            if stream {
                assert!(content_type.starts_with("text/event-stream"));
                let events = std::str::from_utf8(&wire)
                    .unwrap()
                    .lines()
                    .filter_map(|line| line.strip_prefix("data: "))
                    .filter_map(|data| serde_json::from_str::<Value>(data).ok())
                    .collect::<Vec<_>>();
                assert!(events
                    .iter()
                    .any(|event| event.pointer("/delta/text") == Some(&json!("pong"))));
                assert!(events.iter().any(|event| event["type"] == "message_stop"));
            } else {
                assert_eq!(content_type, "application/json");
                let body: Value = serde_json::from_slice(&wire).unwrap();
                assert_eq!(body["type"], "message");
                assert_eq!(body["role"], "assistant");
                assert_eq!(body["content"][0]["text"], "pong");
                assert_eq!(body["stop_reason"], "end_turn");
                assert_eq!(body["usage"]["input_tokens"], 12);
                assert_eq!(body["usage"]["output_tokens"], 3);
            }
        }
    }
    let observations = observations.lock().unwrap();
    assert_eq!(
        observations.len(),
        if tool_result_errors { 7 } else { 6 },
        "rejected probes plus exactly four forwarded calls"
    );
    for (headers, body) in observations
        .iter()
        .skip(if tool_result_errors { 3 } else { 2 })
    {
        assert_eq!(
            headers.get("authorization").unwrap(),
            format!("Bearer {name}-access-token").as_str()
        );
        assert_eq!(
            headers.get("chatgpt-account-id").unwrap(),
            format!("{name}-workspace").as_str()
        );
        if tool_result_errors {
            let items = body["input"].as_array().unwrap();
            assert_eq!(items[5]["type"], "function_call_output");
            assert_eq!(items[5]["call_id"], "a");
            let output = items[5]["output"].as_str().unwrap();
            assert!(output.starts_with("[Tool execution failed]\n"));
            assert!(output.contains("denied"));
            assert_eq!(items[6]["call_id"], "b");
            assert_eq!(items[6]["output"], "ok");
            assert!(items.iter().all(|item| item.get("is_error").is_none()));
        }
        assert_eq!(body["instructions"], "be helpful");
        assert_eq!(body["store"], false);
        assert_eq!(body["stream"], true);
        for (index, text_type) in ["input_text", "output_text", "input_text"]
            .into_iter()
            .enumerate()
        {
            assert_eq!(body["input"][index]["content"][0]["type"], text_type);
        }
    }
    server.abort();
}
