// Independent synthetic history only; no private transcript or upstream credential.
#[derive(Clone, Copy)]
enum ClaudeThinkingHistoryProbeMode {
    AcceptRepaired,
    AcceptForeignHistory,
    RejectHttp,
    RejectSse,
    RejectAfterBusiness,
}

fn claude_thinking_history_fixture() -> Value {
    serde_json::from_str(include_str!(
        "../../../assets/contract/claude-thinking-history.json"
    ))
    .unwrap()
}

async fn claude_thinking_history_probe(
    mode: ClaudeThinkingHistoryProbeMode,
    response: Value,
) -> (String, ClaudeR5Observed, tokio::task::JoinHandle<()>) {
    let observed = Arc::new(StdMutex::new(Vec::new()));
    let capture = observed.clone();
    let app = axum::Router::new().fallback(axum::routing::post(
        move |uri: axum::http::Uri, headers: HeaderMap, axum::Json(body): axum::Json<Value>| {
            let capture = capture.clone();
            let response = response.clone();
            async move {
                // The mock rejects the actual invalid shape instead of accepting
                // every request and merely asserting that forwarding succeeded.
                let invalid = body["messages"].as_array().unwrap().iter().any(|message| {
                    message["role"] == "assistant"
                        && message["content"].as_array().is_some_and(|blocks| {
                            blocks.iter().any(|block| {
                                block["type"] == "thinking"
                                    && block["thinking"].as_str().is_some_and(|text| text.trim().is_empty())
                                    && block.get("signature").and_then(Value::as_str).is_none_or(|signature| signature.trim().is_empty())
                            })
                        })
                });
                let streaming = body["stream"] == true;
                capture.lock().unwrap().push((headers, body));
                let error = json!({"type":"error","error":{"type":"invalid_request_error",
                    "message":"messages.93.content.0.thinking: each thinking block must contain non-whitespace thinking; signature is empty"}});
                let sse_error = format!("event: error\ndata: {error}\n\n");
                match mode {
                    ClaudeThinkingHistoryProbeMode::RejectHttp => {
                        (StatusCode::BAD_REQUEST, axum::Json(error)).into_response()
                    }
                    ClaudeThinkingHistoryProbeMode::RejectSse => {
                        ([(CONTENT_TYPE, "text/event-stream")], sse_error).into_response()
                    }
                    ClaudeThinkingHistoryProbeMode::RejectAfterBusiness => {
                        let success = claude_r5_sse(&response);
                        let prefix = success.split("event: message_delta").next().unwrap();
                        ([(CONTENT_TYPE, "text/event-stream")], format!("{prefix}{sse_error}")).into_response()
                    }
                    ClaudeThinkingHistoryProbeMode::AcceptRepaired if invalid => {
                        (StatusCode::BAD_REQUEST, axum::Json(error)).into_response()
                    }
                    ClaudeThinkingHistoryProbeMode::AcceptRepaired | ClaudeThinkingHistoryProbeMode::AcceptForeignHistory if uri.path().ends_with("count_tokens") => {
                        axum::Json(json!({"input_tokens":3})).into_response()
                    }
                    ClaudeThinkingHistoryProbeMode::AcceptRepaired | ClaudeThinkingHistoryProbeMode::AcceptForeignHistory if streaming => {
                        ([(CONTENT_TYPE, "text/event-stream")], claude_r5_sse(&response)).into_response()
                    }
                    ClaudeThinkingHistoryProbeMode::AcceptRepaired | ClaudeThinkingHistoryProbeMode::AcceptForeignHistory => axum::Json(response).into_response(),
                }
            }
        },
    ));
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{address}"), observed, server)
}

#[tokio::test]
async fn claude_unsigned_blank_thinking_history_survives_parallel_tools_and_repeated_sessions() {
    let fixture = claude_thinking_history_fixture();
    for model in ["claude-opus-5-5", "claude-fable-5-2"] {
        for native in [false, true] {
            for streaming in [false, true] {
                for display in [false, true] {
                    let mut reply = fixture["response"].clone();
                    reply["model"] = json!(model);
                    let (url, observed, server) = claude_thinking_history_probe(
                        ClaudeThinkingHistoryProbeMode::AcceptRepaired,
                        reply,
                    )
                    .await;
                    let name = format!("claude-history-{model}-{native}-{streaming}-{display}");
                    let state = forwarder_test_state(&name);
                    let id = install_claude_oauth_forwarder_test_provider(&state, &name, url).await;
                    let headers = if native {
                        let mut headers = claude_r5_native_headers();
                        headers.insert(
                            "user-agent",
                            HeaderValue::from_static("claude-cli/2.1.329 (external, cli)"),
                        );
                        headers.insert(
                            "anthropic-beta",
                            HeaderValue::from_static(if display {
                                "claude-code-20250219,thinking-display-updates-2026-08-18"
                            } else {
                                "claude-code-20250219,redact-thinking-2026-02-12"
                            }),
                        );
                        headers
                    } else {
                        HeaderMap::new()
                    };
                    let mut input = fixture["request"].clone();
                    input["model"] = json!(model);
                    input["stream"] = json!(streaming);
                    if !display {
                        input["thinking"].as_object_mut().unwrap().remove("display");
                    }
                    // Put the offending assistant at the exact reported index,
                    // while preserving valid alternating role boundaries.
                    let messages = input["messages"].as_array_mut().unwrap();
                    messages.splice(
                        1..1,
                        (0..92).map(|index| {
                            json!({
                                "role":if index % 2 == 0 {"assistant"} else {"user"},
                                "content":"synthetic historical turn"
                            })
                        }),
                    );
                    // Keep genuine signed/redacted blocks in an earlier turn.
                    // The rejected turn itself must remain exactly the observed
                    // shape: one unsigned space followed by four tool calls.
                    let mut preserved = fixture["preservedBlocks"].as_array().unwrap().clone();
                    preserved.push(json!({"type":"text","text":"synthetic historical turn"}));
                    messages[1]["content"] = Value::Array(preserved);
                    for turn in 0..2 {
                        let mut expected = input["messages"].clone();
                        expected[93]["content"].as_array_mut().unwrap().remove(0);
                        let response = forward_for_test_surface(
                            state.clone(),
                            ProxyRoute::ClaudeMessages,
                            id.clone(),
                            None,
                            headers.clone(),
                            Bytes::from(input.to_string()),
                        )
                        .await
                        .unwrap();
                        assert_eq!(response.status(), StatusCode::OK);
                        let bytes = collect_response_body(response).await;
                        if streaming {
                            let events = claude_r5_events(&bytes);
                            assert!(events.iter().any(|event| event["type"] == "message_stop"));
                            assert!(!events.iter().any(|event| event["type"] == "error"));
                        } else {
                            let body: Value = serde_json::from_slice(&bytes).unwrap();
                            assert_eq!(body["stop_reason"], "end_turn");
                        }
                        {
                            let captured = observed.lock().unwrap();
                            assert_eq!(captured.len(), turn + 1, "must not replay generation");
                            let (headers, sent) = captured.last().unwrap();
                            assert_eq!(headers["authorization"], "Bearer compressed-sse-access");
                            assert_eq!(sent["messages"], expected);
                            assert_eq!(sent["model"], model);
                            assert_eq!(sent["thinking"]["type"], "adaptive");
                            assert_eq!(
                                sent["thinking"].get("display"),
                                input["thinking"].get("display")
                            );
                            assert_eq!(sent["output_config"], input["output_config"]);
                        }
                        // The client's persisted history is not edited. A later
                        // call must independently repair the same old placeholder.
                        assert_eq!(input["messages"][93]["content"][0]["thinking"], " ");
                        if turn == 0 {
                            input["messages"].as_array_mut().unwrap().extend([
                                json!({"role":"assistant","content":"ok"}),
                                json!({"role":"user","content":[{"type":"text","text":"continue","cache_control":{"type":"ephemeral"}}]}),
                            ]);
                        }
                    }
                    let response = forward_for_test_surface(
                        state,
                        ProxyRoute::ClaudeCountTokens,
                        id,
                        None,
                        headers,
                        Bytes::from(input.to_string()),
                    )
                    .await
                    .unwrap();
                    assert_eq!(response.status(), StatusCode::OK);
                    collect_response_body(response).await;
                    let captured = observed.lock().unwrap();
                    assert_eq!(captured.len(), 3);
                    assert_eq!(captured[2].1["messages"], captured[1].1["messages"]);
                    assert!(!captured[2].0["anthropic-beta"]
                        .to_str()
                        .unwrap()
                        .contains("redact-thinking"));
                    server.abort();
                }
            }
        }
    }
}

#[tokio::test]
async fn claude_unsigned_blank_thinking_upstream_rejection_never_replays_or_downgrades() {
    let fixture = claude_thinking_history_fixture();
    for (index, mode) in [
        ClaudeThinkingHistoryProbeMode::RejectHttp,
        ClaudeThinkingHistoryProbeMode::RejectSse,
        ClaudeThinkingHistoryProbeMode::RejectAfterBusiness,
    ]
    .into_iter()
    .enumerate()
    {
        let (url, observed, server) =
            claude_thinking_history_probe(mode, fixture["response"].clone()).await;
        let name = format!("claude-history-rejected-{index}");
        let state = forwarder_test_state(&name);
        let id = install_claude_oauth_forwarder_test_provider(&state, &name, url).await;
        let mut input = fixture["request"].clone();
        input["stream"] = json!(index != 0);
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
        let status = response.status();
        let bytes = collect_response_body(response).await;
        if index == 0 {
            assert_eq!(status, StatusCode::BAD_REQUEST);
        } else {
            let events = claude_r5_events(&bytes);
            assert!(events.iter().any(|event| event["type"] == "error"));
            assert!(!events.iter().any(|event| event["type"] == "message_stop"));
            if index == 2 {
                assert!(events.iter().any(|event| event["delta"]["text"] == "ok"));
            }
        }
        let captured = observed.lock().unwrap();
        assert_eq!(
            captured.len(),
            1,
            "empty-thinking rejection must not degrade signed history"
        );
        assert_eq!(captured[0].1["thinking"]["type"], "adaptive");
        assert_eq!(
            captured[0].1["messages"][1]["content"]
                .as_array()
                .unwrap()
                .len(),
            4
        );
        server.abort();
    }
}

#[tokio::test]
async fn claude_unsigned_blank_thinking_empty_turn_fails_before_network() {
    let fixture = claude_thinking_history_fixture();
    let (url, observed, server) = claude_thinking_history_probe(
        ClaudeThinkingHistoryProbeMode::AcceptRepaired,
        fixture["response"].clone(),
    )
    .await;
    let name = "claude-history-empty-turn";
    let state = forwarder_test_state(name);
    let id = install_claude_oauth_forwarder_test_provider(&state, name, url).await;
    let mut input = fixture["request"].clone();
    input["messages"][1]["content"]
        .as_array_mut()
        .unwrap()
        .truncate(1);
    let error = forward_for_test_surface(
        state,
        ProxyRoute::ClaudeMessages,
        id,
        None,
        claude_r5_native_headers(),
        Bytes::from(input.to_string()),
    )
    .await
    .expect_err("reject an empty assistant turn");
    assert_eq!(error.status, StatusCode::BAD_REQUEST);
    assert!(error.message.contains("messages.1.content"));
    assert!(observed.lock().unwrap().is_empty());
    server.abort();
}

#[tokio::test]
async fn claude_unsigned_blank_thinking_repair_does_not_change_kimi_history() {
    let fixture = claude_thinking_history_fixture();
    let mut reply = fixture["response"].clone();
    reply["model"] = json!("kimi-k3-1");
    let (url, observed, server) =
        claude_thinking_history_probe(ClaudeThinkingHistoryProbeMode::AcceptForeignHistory, reply)
            .await;
    let name = "claude-history-kimi-boundary";
    let state = forwarder_test_state(name);
    let (id, _) = install_kimi_replay_test_provider(&state, name, url).await;
    let mut input = fixture["request"].clone();
    input["model"] = json!("kimi-for-coding");
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
    collect_response_body(response).await;
    let captured = observed.lock().unwrap();
    assert_eq!(captured.len(), 1);
    assert_eq!(captured[0].1["messages"], input["messages"]);
    server.abort();
}
