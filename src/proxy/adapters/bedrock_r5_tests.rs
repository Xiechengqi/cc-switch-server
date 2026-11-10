fn typed_bedrock_r5_stored(profile: &str) -> StoredProvider {
    let mut stored = stored_provider(AppKind::Claude, ProviderType::AwsBedrock, json!({}));
    stored.resource.profile_id =
        Some(crate::domain::providers::registry::ProfileId::parse(profile).unwrap());
    stored
}

#[test]
fn bedrock_r5_converse_json_is_a_claude_message_on_both_static_rails() {
    for profile in ["claude.aws_bedrock_aksk", "claude.aws_bedrock_api_key"] {
        let stored = typed_bedrock_r5_stored(profile);
        let raw = serde_json::to_vec(&json!({"output":{"message":{"role":"assistant","content":[{"text":"hello"}]}},"stopReason":"end_turn","usage":{"inputTokens":11,"outputTokens":2,"totalTokens":13}})).unwrap();
        let response = adapter_for(AppKind::Claude, ProviderType::AwsBedrock)
            .transform_response(Bytes::from(raw), &stored, ProxyRoute::ClaudeMessages)
            .unwrap();
        let value: Value = serde_json::from_slice(&response).unwrap();
        assert_eq!(value["type"], "message", "profile={profile}");
        assert_eq!(value["content"][0]["text"], "hello");
        assert_eq!(value["stop_reason"], "end_turn");
        assert_eq!(value["usage"]["input_tokens"], 11);
    }
}

#[test]
fn bedrock_r5_converse_json_requires_an_official_terminal_and_valid_usage() {
    let stored = typed_bedrock_r5_stored("claude.aws_bedrock_aksk");
    for raw in [
        json!({"output":{"message":{"role":"assistant","content":[{"text":"partial"}]}},"usage":{"inputTokens":1,"outputTokens":1}}),
        json!({"output":{"message":{"role":"assistant","content":[]}},"stopReason":"end_turn","usage":{"inputTokens":-1,"outputTokens":1}}),
    ] {
        assert!(adapter_for(AppKind::Claude, ProviderType::AwsBedrock)
            .transform_response(
                Bytes::from(serde_json::to_vec(&raw).unwrap()),
                &stored,
                ProxyRoute::ClaudeMessages
            )
            .is_err());
    }
}

#[test]
fn bedrock_r5_request_keeps_opaque_thinking_history_and_explicit_model_fields() {
    let input = json!({"messages":[{"role":"assistant","content":[{"type":"thinking","thinking":"thought","signature":"opaque"},{"type":"redacted_thinking","data":"b3BhcXVl"}]}],"thinking":{"type":"enabled","budget_tokens":1024},"top_k":4});
    let mut input = input;
    input["stop_sequences"] = json!(["exact-stop"]);
    let output = bedrock_converse_body_from_anthropic(&input).unwrap();
    assert_eq!(
        output["additionalModelResponseFieldPaths"],
        json!(["/stop_sequence"])
    );
    assert_eq!(
        output["messages"][0]["content"][0]["reasoningContent"]["reasoningText"]["signature"],
        "opaque"
    );
    assert_eq!(
        output["messages"][0]["content"][1]["reasoningContent"]["redactedContent"],
        "b3BhcXVl"
    );
    assert_eq!(
        output["additionalModelRequestFields"]["thinking"],
        input["thinking"]
    );
    let mut bad = input.clone();
    bad["messages"][0]["content"][0]["signature"] = json!(false);
    assert!(bedrock_converse_body_from_anthropic(&bad).is_err());
    let mut conflict = input;
    conflict["additionalModelRequestFields"] = json!({"top_k":99});
    assert!(bedrock_converse_body_from_anthropic(&conflict).is_err());
}
