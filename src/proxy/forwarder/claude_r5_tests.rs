// Local synthetic forwarding evidence; never a vendor acceptance receipt.
type ClaudeR5Observed = Arc<StdMutex<Vec<(HeaderMap, Value)>>>;

fn claude_r5_fixture() -> Value {
    serde_json::from_str(include_str!(
        "../../../assets/contract/r5-claude-oauth.json"
    ))
    .unwrap()
}

fn claude_r5_sse(message: &Value) -> String {
    let mut start = message.clone();
    start["content"] = json!([]);
    start["stop_reason"] = Value::Null;
    let mut events = vec![json!({"type":"message_start","message":start})];
    for (index, block) in message["content"].as_array().unwrap().iter().enumerate() {
        let mut initial = block.clone();
        let delta = match block["type"].as_str().unwrap() {
            "tool_use" => {
                initial["input"] = json!({});
                json!({"type":"input_json_delta","partial_json":block["input"].to_string()})
            }
            "thinking" => {
                initial["thinking"] = json!("");
                json!({"type":"thinking_delta","thinking":block["thinking"]})
            }
            _ => {
                initial["text"] = json!("");
                json!({"type":"text_delta","text":block["text"]})
            }
        };
        events.push(json!({"type":"content_block_start","index":index,"content_block":initial}));
        events.push(json!({"type":"content_block_delta","index":index,"delta":delta}));
        events.push(json!({"type":"content_block_stop","index":index}));
    }
    events.push(json!({"type":"message_delta","delta":{"stop_reason":message["stop_reason"],"stop_sequence":null},"usage":message["usage"]}));
    events.push(json!({"type":"message_stop"}));
    events
        .iter()
        .map(|event| {
            format!(
                "event: {}\ndata: {event}\n\n",
                event["type"].as_str().unwrap()
            )
        })
        .collect()
}

fn claude_r5_events(bytes: &[u8]) -> Vec<Value> {
    std::str::from_utf8(bytes)
        .unwrap()
        .lines()
        .filter_map(|line| {
            line.strip_prefix("data: ")
                .and_then(|data| serde_json::from_str(data).ok())
        })
        .collect()
}

async fn claude_r5_probe(
    message: Value,
    tool_loop: bool,
) -> (String, ClaudeR5Observed, tokio::task::JoinHandle<()>) {
    let observed = Arc::new(StdMutex::new(Vec::new()));
    let capture = observed.clone();
    let app = axum::Router::new().fallback(axum::routing::post(move |headers: HeaderMap, body: Bytes| {
        let capture = capture.clone();
        let mut message = message.clone();
        async move {
            let body: Value = serde_json::from_slice(&body).unwrap();
            if tool_loop && body.pointer("/tools/0/name").and_then(Value::as_str).is_some() && !body["messages"].as_array().unwrap().iter().any(|turn| turn["role"] == "assistant") {
                let alias = &body["tools"][0]["name"];
                message["content"] = json!([{"type":"tool_use","id":"toolu_r5","name":alias,
                    "input":{"function":{"name":alias},"functionCall":{"name":alias,"args":{"name":alias}},
                    "nested":{"type":"function_call","name":alias}}}]);
                message["stop_reason"] = json!("tool_use");
            }
            let streaming = body["stream"] == true;
            capture.lock().unwrap().push((headers, body));
            if streaming {
                ([(CONTENT_TYPE,"text/event-stream")],claude_r5_sse(&message)).into_response()
            } else {
                axum::Json(message).into_response()
            }
        }
    }));
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{address}"), observed, server)
}

fn claude_r5_native_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    for (name, value) in [
        ("x-app", "cli"),
        (
            "user-agent",
            "claude-cli/2.1.258 (external, cli, linux-x64)",
        ),
        ("x-stainless-package-version", "0.112.1"),
        ("anthropic-beta", "claude-code-20250219"),
    ] {
        headers.insert(name, HeaderValue::from_static(value));
    }
    headers
}

#[tokio::test]
async fn claude_r5_native_effort_and_whitespace_reasoning_survive_json_and_sse() {
    let fixture = claude_r5_fixture();
    for streaming in [false, true] {
        let (url, observed, server) =
            claude_r5_probe(fixture["nativeResponse"].clone(), false).await;
        let name = format!("claude-r5-native-{streaming}");
        let state = forwarder_test_state(&name);
        let id = install_claude_oauth_forwarder_test_provider(&state, &name, url).await;
        let mut input = fixture["nativeRequest"].clone();
        input["stream"] = json!(streaming);
        let response = forward_for_test_surface(
            state,
            ProxyRoute::ClaudeMessages,
            id,
            None,
            claude_r5_native_headers(),
            Bytes::from(input.to_string()),
        )
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = collect_response_body(response).await;
        if streaming {
            let events = claude_r5_events(&bytes);
            let thinking = events
                .iter()
                .find(|e| e["delta"]["type"] == "thinking_delta")
                .unwrap();
            assert_eq!(thinking["delta"]["thinking"], " \t\n ");
            assert!(events.iter().any(|e| e["type"] == "message_stop"));
            assert!(!events.iter().any(|e| e["type"] == "error"));
        } else {
            let body: Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(body["content"][0]["thinking"], " \t\n ");
            assert_eq!(body["content"][1]["text"], "ok");
            assert_eq!(body["stop_reason"], "end_turn");
            assert_eq!(body["usage"], fixture["nativeResponse"]["usage"]);
        }
        let observed = observed.lock().unwrap();
        assert_eq!(observed.len(), 1);
        assert_eq!(
            observed[0].0["authorization"],
            "Bearer compressed-sse-access"
        );
        let body = &observed[0].1;
        assert_eq!(body["model"], input["model"]);
        assert_eq!(body["output_config"], input["output_config"]);
        assert_eq!(body["system"][0]["text"], "Keep style.");
        let mut expected = input["messages"].clone();
        expected[0]["content"] = json!([]);
        assert_eq!(body["messages"], expected);
        server.abort();
    }
}

#[tokio::test]
async fn claude_r5_cache_betas_and_two_turn_alias_keep_business_payload_opaque() {
    let fixture = claude_r5_fixture();
    for streaming in [false, true] {
        let mut reply = fixture["nativeResponse"].clone();
        reply["content"] = json!([{"type":"text","text":"ok"}]);
        reply["model"] = json!("claude-sonnet-4-6");
        let (url, observed, server) = claude_r5_probe(reply, true).await;
        let name = format!("claude-r5-alias-{streaming}");
        let state = forwarder_test_state(&name);
        let id = install_claude_oauth_forwarder_test_provider(&state, &name, url).await;
        let response = forward_for_test_surface(
            state.clone(),
            ProxyRoute::ClaudeMessages,
            id.clone(),
            None,
            HeaderMap::new(),
            Bytes::from(fixture["cacheRequest"].to_string()),
        )
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        collect_response_body(response).await;
        // Start an independent captured tool loop, using the same Account/session.
        let mut input = fixture["toolRequest"].clone();
        input["stream"] = json!(streaming);
        let response = forward_for_test_surface(
            state.clone(),
            ProxyRoute::ClaudeMessages,
            id.clone(),
            None,
            HeaderMap::new(),
            Bytes::from(input.to_string()),
        )
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = collect_response_body(response).await;
        let (name, input_value) = if streaming {
            let events = claude_r5_events(&bytes);
            assert!(!events.iter().any(|e| e["type"] == "error"));
            let tool = events
                .iter()
                .find(|e| e["content_block"]["type"] == "tool_use")
                .unwrap();
            let args: String = events
                .iter()
                .filter_map(|e| e["delta"]["partial_json"].as_str())
                .collect();
            (
                tool["content_block"]["name"].clone(),
                serde_json::from_str::<Value>(&args).unwrap(),
            )
        } else {
            let body: Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(body["stop_reason"], "tool_use");
            (
                body["content"][0]["name"].clone(),
                body["content"][0]["input"].clone(),
            )
        };
        assert_eq!(name, "custom.lookup");
        let alias = observed.lock().unwrap()[1].1["tools"][0]["name"].clone();
        assert!(alias.as_str().unwrap().starts_with("cc_tool_"));
        for pointer in [
            "/function/name",
            "/functionCall/name",
            "/functionCall/args/name",
            "/nested/name",
        ] {
            assert_eq!(input_value.pointer(pointer), Some(&alias));
        }
        input["messages"].as_array_mut().unwrap().extend([
            json!({"role":"assistant","content":[{"type":"tool_use","id":"toolu_r5","name":name,"input":input_value}]}),
            json!({"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_r5","content":"result custom.lookup"}]})
        ]);
        let response = forward_for_test_surface(
            state,
            ProxyRoute::ClaudeMessages,
            id,
            None,
            HeaderMap::new(),
            Bytes::from(input.to_string()),
        )
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = collect_response_body(response).await;
        assert!(std::str::from_utf8(&bytes).unwrap().contains("ok"));
        let observed = observed.lock().unwrap();
        assert_eq!(
            observed.len(),
            3,
            "each turn is sent once on the same binding"
        );
        let betas = observed[0].0["anthropic-beta"]
            .to_str()
            .unwrap()
            .split(',')
            .collect::<Vec<_>>();
        for beta in fixture["cacheExpectedBetas"].as_array().unwrap() {
            assert!(betas.contains(&beta.as_str().unwrap()));
        }
        assert_eq!(observed[0].1["diagnostics"], json!({}));
        assert_eq!(
            observed[0].1["messages"][0]["content"][0]["cache_control"]["evict_on_complete"],
            true
        );
        for (headers, _) in &observed[1..] {
            let betas = headers["anthropic-beta"]
                .to_str()
                .unwrap()
                .split(',')
                .collect::<Vec<_>>();
            for beta in fixture["cacheExpectedBetas"].as_array().unwrap() {
                assert!(
                    !betas.contains(&beta.as_str().unwrap()),
                    "cache controls cannot leak to a different turn"
                );
            }
        }
        let followup = &observed[2].1;
        assert_eq!(followup["tools"][0]["name"], alias);
        assert_eq!(followup["messages"][1]["content"][0]["name"], alias);
        assert_eq!(followup["messages"][1]["content"][0]["input"], input_value);
        assert_eq!(
            followup["messages"][2]["content"][0]["content"],
            "result custom.lookup"
        );
        server.abort();
    }
}

#[tokio::test]
async fn claude_r5_refusal_keeps_terminal_semantics_on_all_compatibility_routes() {
    let fixture = claude_r5_fixture();
    let (url, observed, server) = claude_r5_probe(fixture["refusalResponse"].clone(), false).await;
    let state = forwarder_test_state("claude-r5-refusal");
    let id = install_claude_oauth_forwarder_test_provider(&state, "claude-r5-refusal", url.clone())
        .await;
    let provider = state
        .providers
        .read()
        .await
        .providers
        .iter()
        .find(|p| p.provider.id == id)
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
            .override_provider_runtime_endpoint_for_test(app, &id, url.clone())
            .await
            .unwrap();
    }
    for streaming in [false, true] {
        for route in [
            ProxyRoute::ClaudeMessages,
            ProxyRoute::CodexResponses,
            ProxyRoute::CodexChatCompletions,
            ProxyRoute::Gemini,
        ] {
            let input = match route {
                ProxyRoute::CodexResponses => {
                    json!({"model":"claude-sonnet-4-6","input":"reply","stream":streaming})
                }
                ProxyRoute::Gemini => {
                    json!({"contents":[{"role":"user","parts":[{"text":"reply"}]}]})
                }
                _ => {
                    json!({"model":"claude-sonnet-4-6","max_tokens":64,"messages":[{"role":"user","content":"reply"}],"stream":streaming})
                }
            };
            let path = (route == ProxyRoute::Gemini).then(|| {
                format!(
                    "models/claude-sonnet-4-6:{}",
                    if streaming {
                        "streamGenerateContent"
                    } else {
                        "generateContent"
                    }
                )
            });
            let response = forward_for_test_surface(
                state.clone(),
                route,
                id.clone(),
                path,
                HeaderMap::new(),
                Bytes::from(input.to_string()),
            )
            .await
            .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let bytes = collect_response_body(response).await;
            let text = std::str::from_utf8(&bytes).unwrap();
            assert!(
                text.contains("Cannot comply."),
                "{route:?} {streaming}: {text}"
            );
            if streaming {
                let marker = match route {
                    ProxyRoute::ClaudeMessages => "\"stop_reason\":\"refusal\"",
                    ProxyRoute::CodexResponses => "response.incomplete",
                    ProxyRoute::CodexChatCompletions => "\"finish_reason\":\"content_filter\"",
                    _ => "\"finishReason\":\"SAFETY\"",
                };
                assert!(text.contains(marker), "{route:?}: {text}");
                assert!(!text.contains("response.completed"), "{route:?}: {text}");
            } else {
                let value: Value = serde_json::from_slice(&bytes).unwrap();
                match route {
                    ProxyRoute::ClaudeMessages => assert_eq!(value["stop_reason"], "refusal"),
                    ProxyRoute::CodexResponses => {
                        assert_eq!(value["status"], "incomplete");
                        assert_eq!(value["incomplete_details"]["reason"], "content_filter");
                    }
                    ProxyRoute::CodexChatCompletions => {
                        assert_eq!(value["choices"][0]["finish_reason"], "content_filter")
                    }
                    _ => assert_eq!(value["candidates"][0]["finishReason"], "SAFETY"),
                }
            }
        }
    }
    let observed = observed.lock().unwrap();
    assert_eq!(observed.len(), 8, "refusal must not trigger any replay");
    for (headers, body) in observed.iter() {
        assert_eq!(headers["authorization"], "Bearer compressed-sse-access");
        assert_eq!(body["model"], "claude-sonnet-4-6");
    }
    server.abort();
}
