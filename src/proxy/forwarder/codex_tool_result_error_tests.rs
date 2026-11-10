#[test]
fn responses_tool_result_error_sanitizer_is_scoped_and_idempotent() {
    let opaque = json!({"is_error":true,"output":"business data"});
    let mut body = json!({"input":[
        {"type":"function_call_output","call_id":"a","is_error":true,"output":"denied"},
        {"type":"custom_tool_call_output","call_id":"b","is_error":true,"output":[
            {"type":"input_text","text":"details","data":opaque},
            {"type":"input_image","image_url":"data:image/png;base64,AA=="},
            {"type":"input_file","file_id":"file_1"}
        ]},
        {"type":"function_call_output","call_id":"c","is_error":false,"output":opaque},
        {"type":"function_call","call_id":"d","name":"lookup","arguments":opaque},
        {"type":"agent_message","is_error":true,"content":[opaque]},
        {"type":"reasoning","is_error":true,"summary":[opaque]},
        {"type":"future_output","is_error":true,"output":opaque},
        {"type":"message","role":"user","is_error":true,"content":[{"type":"input_text","text":"is_error"}]},
        {"type":"function_call_output","call_id":"e","output":[opaque]},
        {"type":"function_call_output","call_id":"f","is_error":true,"output":opaque},
        {"type":null,"is_error":true,"output":"opaque"}, null
    ]});
    let original = body.clone();
    sanitize_codex_oauth_request_body(&mut body);
    assert_eq!(
        body["input"][0]["output"],
        "[Tool execution failed]\ndenied"
    );
    assert!(body["input"][0].get("is_error").is_none());
    let parts = body["input"][1]["output"].as_array().unwrap();
    assert_eq!(
        parts[0],
        json!({"type":"input_text","text":"[Tool execution failed]"})
    );
    assert_eq!(
        &parts[1..],
        original["input"][1]["output"].as_array().unwrap()
    );
    for index in [1, 2, 9] {
        assert!(body["input"][index].get("is_error").is_none());
    }
    for index in 3..9 {
        assert_eq!(body["input"][index], original["input"][index]);
    }
    for index in [2, 9] {
        assert_eq!(body["input"][index]["output"], opaque);
    }
    assert_eq!(body["input"][10], original["input"][10]);
    let once = body.clone();
    sanitize_codex_oauth_request_body(&mut body);
    assert_eq!(body, once);
}

#[test]
fn responses_tool_result_error_sanitizer_only_treats_boolean_true_as_failure() {
    for kind in ["function_call_output", "custom_tool_call_output"] {
        for flag in [
            json!(true),
            json!(false),
            Value::Null,
            json!("true"),
            json!(1),
        ] {
            for output in [None, Some(Value::Null), Some(json!("")), Some(json!("ok"))] {
                let mut item = json!({"type":kind,"call_id":"call","is_error":flag});
                if let Some(output) = &output {
                    item["output"] = output.clone();
                }
                let before = item.clone();
                crate::proxy::transforms::normalize_responses_tool_result_error(&mut item);
                assert!(item.get("is_error").is_none());
                if flag == true {
                    assert_eq!(
                        item["output"],
                        if output == Some(json!("ok")) {
                            "[Tool execution failed]\nok"
                        } else {
                            "[Tool execution failed]"
                        }
                    );
                } else {
                    assert_eq!(item.get("output"), before.get("output"));
                }
            }
        }
    }
}

#[tokio::test]
async fn responses_tool_result_error_covers_all_codex_transports() {
    let source = json!({"model":"gpt-5.4","messages":[
        {"role":"assistant","content":[{"type":"tool_use","id":"call_1","name":"lookup","input":{}}]},
        {"role":"user","content":[{"type":"tool_result","tool_use_id":"call_1","content":"denied","is_error":true}]}
    ]});
    let claude_provider =
        stored_provider(AppKind::Claude, ProviderType::CodexOAuth, json!({}), None);
    let translated = adapters::adapter_for(AppKind::Claude, ProviderType::CodexOAuth)
        .transform_request_for_route(
            Bytes::from(source.to_string()),
            &claude_provider,
            ProxyRoute::ClaudeMessages,
            None,
        )
        .unwrap();
    let converted: Value = serde_json::from_slice(&translated.body).unwrap();
    // Include both freshly translated history and legacy/native Responses input.
    for mut request in [
        converted,
        json!({"model":"gpt-5.4","input":[
            {"type":"function_call_output","call_id":"call_1","output":"denied","is_error":true},
            {"type":"custom_tool_call_output","call_id":"call_2","output":"ok","is_error":false}
        ]}),
    ] {
        request["model"] = json!("gpt-5.4");
        let assert_output = |body: &Value| {
            let results: Vec<_> = body["input"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|item| {
                    matches!(
                        item["type"].as_str(),
                        Some("function_call_output" | "custom_tool_call_output")
                    )
                })
                .collect();
            assert!(!results.is_empty());
            assert_eq!(results[0]["call_id"], "call_1");
            assert_eq!(results[0]["output"], "[Tool execution failed]\ndenied");
            assert!(results.iter().all(|item| item.get("is_error").is_none()));
        };
        let http = normalize_codex_oauth_responses_body_bytes(
            &Bytes::from(request.to_string()),
            None,
            CodexImageToolStripPolicy::Never,
        )
        .unwrap();
        assert_output(&serde_json::from_slice(&http).unwrap());
        let compact =
            normalize_codex_oauth_compact_body_bytes(&Bytes::from(request.to_string())).unwrap();
        assert_output(&serde_json::from_slice(&compact).unwrap());
        let provider = stored_provider(AppKind::Codex, ProviderType::CodexOAuth, json!({}), None);
        let intent = crate::proxy::codex_request_policy::extract_intent(&request);
        for nested in [false, true] {
            for binary in [false, true] {
                let frame = if nested {
                    json!({"type":"response.create","response":request})
                } else {
                    let mut frame = request.clone();
                    frame["type"] = json!("response.create");
                    frame
                };
                let message = if binary {
                    TungsteniteMessage::Binary(serde_json::to_vec(&frame).unwrap())
                } else {
                    TungsteniteMessage::Text(frame.to_string())
                };
                let (prepared, _) = prepare_codex_responses_websocket_request(
                    message, &provider, false, None, &intent, None,
                )
                .await
                .unwrap();
                let prepared: Value = match prepared {
                    TungsteniteMessage::Text(text) => serde_json::from_str(&text).unwrap(),
                    TungsteniteMessage::Binary(bytes) => serde_json::from_slice(&bytes).unwrap(),
                    _ => panic!("expected JSON frame"),
                };
                assert_output(if nested {
                    &prepared["response"]
                } else {
                    &prepared
                });
            }
        }
        let (state, execution) =
            codex_bridge_test_context("tool-result-error-fallback", "http://127.0.0.1:9".into())
                .await;
        let fallback = prepare_codex_http_fallback_target(
            &state,
            &execution,
            &request,
            Some("tool-result-session"),
            None,
            &intent,
        )
        .await
        .unwrap();
        assert_output(&serde_json::from_slice(&fallback.body).unwrap());
    }
}

#[tokio::test]
async fn responses_tool_result_error_claude_history_passes_strict_oauth_upstream() {
    assert_claude_history_passes_strict_oauth_upstream(true).await;
}
