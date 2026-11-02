// Role repair is limited to direct text parts of ordinary Responses messages.
#[test]
fn responses_message_text_types_sanitizer_is_scoped_and_idempotent() {
    let opaque = json!({"type":"input_text", "text":"do not rewrite"});
    let mut body = json!({"input":[
        {"role":"assistant", "content":[
            {"type":"input_text", "text":"answer", "annotations":[{"type":"citation"}]},
            {"type":"text", "text":"legacy"},
            {"type":"output_text", "text":"already correct"},
            {"type":"refusal", "refusal":"no"},
            {"type":"input_image", "image_url":"data:image/png;base64,AA=="}
        ]},
        {"type":"message", "role":"user", "content":[{"type":"output_text", "text":"question"}]},
        {"role":"system", "content":[{"type":"output_text", "text":"system"}]},
        {"role":"developer", "content":[{"type":"text", "text":"developer"}]},
        {"type":"agent_message", "role":"assistant", "author":"agent", "recipient":"all", "content":[opaque]},
        {"type":"future_message", "role":"assistant", "content":[opaque]},
        {"type":"function_call", "role":"assistant", "arguments":opaque, "content":[opaque]},
        {"type":"function_call_output", "role":"assistant", "output":[opaque], "content":[opaque]},
        {"type":"reasoning", "role":"assistant", "summary":[opaque], "content":[opaque]},
        {"role":"unknown", "content":[opaque]},
        {"content":[opaque]},
        {"role":"assistant", "content":"string content remains a string"},
        {"role":"user", "content":[{"type":"text", "text":"user text", "data":opaque}]}
    ]});
    let original = body.clone();
    sanitize_codex_oauth_request_body(&mut body);
    assert_eq!(body["input"][0]["content"][0]["type"], "output_text");
    assert_eq!(
        body["input"][0]["content"][0]["annotations"],
        original["input"][0]["content"][0]["annotations"]
    );
    assert_eq!(body["input"][0]["content"][1]["type"], "output_text");
    assert_eq!(
        body["input"][0]["content"].as_array().unwrap()[2..],
        original["input"][0]["content"].as_array().unwrap()[2..]
    );
    for index in [1, 2, 3, 12] {
        assert_eq!(body["input"][index]["content"][0]["type"], "input_text");
    }
    assert_eq!(body["input"][2]["role"], "developer");
    for index in 4..12 {
        assert_eq!(body["input"][index], original["input"][index]);
    }
    assert_eq!(body["input"][12]["content"][0]["data"], opaque);
    let once = body.clone();
    sanitize_codex_oauth_request_body(&mut body);
    assert_eq!(body, once);
}

#[test]
fn responses_message_text_types_leave_malformed_and_unknown_shapes_opaque() {
    let mut body = json!({"input":[
        {"type":17, "role":"assistant", "content":[{"type":"input_text", "text":"numeric item type"}]},
        {"type":null, "role":"assistant", "content":[{"type":"input_text", "text":"null item type"}]},
        {"role":"assistant", "content":[null, "literal", {"text":"missing type"},
            {"type":null, "text":"null part type"}, {"type":"future_text", "text":"future part"}]},
        null, 17, "opaque"
    ]});
    let original = body.clone();
    sanitize_codex_oauth_request_body(&mut body);
    assert_eq!(body, original);
}

#[tokio::test]
async fn responses_message_text_types_cover_all_codex_transports() {
    let request = json!({
        "model":"gpt-5.4",
        "input":[
            {"role":"user", "content":[{"type":"output_text", "text":"question"}]},
            {"role":"assistant", "content":[{"type":"input_text", "text":"answer"}]}
        ]
    });
    let assert_types = |body: &Value| {
        assert_eq!(body["input"][0]["content"][0]["type"], "input_text");
        assert_eq!(body["input"][1]["content"][0]["type"], "output_text");
    };
    let http = normalize_codex_oauth_responses_body_bytes(
        &Bytes::from(request.to_string()),
        None,
        CodexImageToolStripPolicy::Never,
    )
    .unwrap();
    assert_types(&serde_json::from_slice(&http).unwrap());
    let compact =
        normalize_codex_oauth_compact_body_bytes(&Bytes::from(request.to_string())).unwrap();
    assert_types(&serde_json::from_slice(&compact).unwrap());

    let provider = stored_provider(AppKind::Codex, ProviderType::CodexOAuth, json!({}), None);
    let intent = crate::proxy::codex_request_policy::extract_intent(&request);
    for nested in [false, true] {
        let frame = if nested {
            json!({"type":"response.create", "response":request})
        } else {
            let mut frame = request.clone();
            frame["type"] = json!("response.create");
            frame
        };
        let (prepared, _) = prepare_codex_responses_websocket_request(
            TungsteniteMessage::Text(frame.to_string()),
            &provider,
            false,
            None,
            &intent,
            None,
        )
        .await
        .unwrap();
        let TungsteniteMessage::Text(prepared) = prepared else {
            panic!("expected text frame");
        };
        let prepared: Value = serde_json::from_str(&prepared).unwrap();
        assert_types(if nested {
            &prepared["response"]
        } else {
            &prepared
        });
    }

    let (state, execution) = codex_bridge_test_context(
        "message-text-types-fallback",
        "http://127.0.0.1:9".to_string(),
    )
    .await;
    let fallback = prepare_codex_http_fallback_target(
        &state,
        &execution,
        &request,
        Some("message-types-session"),
        None,
        &intent,
    )
    .await
    .unwrap();
    assert_types(&serde_json::from_slice(&fallback.body).unwrap());
}
