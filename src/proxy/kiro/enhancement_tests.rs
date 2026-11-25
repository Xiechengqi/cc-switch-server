fn enhancement_registry(body: &Value, mode: ToolCompatibilityMode) -> KiroToolNameRegistry {
    let mut registry = anthropic_to_kiro_request(body, &test_account(), mode)
        .unwrap()
        .tool_name_registry;
    registry.configure_response(body);
    registry
}

fn enhancement_body() -> Value {
    json!({"model":"claude-sonnet-4-8","messages":[{"role":"user","content":"hello"}]})
}

#[test]
fn enhancement_raw_tool_inputs_preserve_every_field_on_both_outputs() {
    for (name, input) in [
        ("lookup", json!({"explanation":"keep me","value":7})),
        (
            "read_file",
            json!({"path":"notes.txt","start_line":2,"end_line":4,"explanation":"keep me"}),
        ),
    ] {
        let mut body = enhancement_body();
        body["tools"] = json!([{"name":name,"input_schema":{"type":"object"}}]);
        let registry = enhancement_registry(&body, ToolCompatibilityMode::Raw);
        let frames = event_stream_bytes(vec![(
            "toolUseEvent",
            json!({"toolUseId":"call","name":name,"input":input.to_string(),"stop":true}),
        )]);
        let message = kiro_event_bytes_to_anthropic_json_with_context_window(
            &frames,
            "claude-sonnet-4-8",
            &registry,
            KiroPromptCacheUsage::default(),
            200_000,
        )
        .unwrap();
        assert_eq!(message["content"][0]["input"], input);
        let mut builder =
            SseBuilder::new_with_registry("claude-sonnet-4-8".into(), registry, 200_000);
        let events = builder
            .tool_delta("call", name, &input.to_string(), true)
            .unwrap();
        let data = events
            .iter()
            .find_map(|event| {
                let text = std::str::from_utf8(event).unwrap();
                let value: Value = serde_json::from_str(
                    text.lines().find_map(|line| line.strip_prefix("data: "))?,
                )
                .unwrap();
                value
                    .pointer("/delta/partial_json")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .unwrap();
        assert_eq!(serde_json::from_str::<Value>(&data).unwrap(), input);
    }
}

#[test]
fn enhancement_metadata_stop_reasons_agree_on_stream_and_json() {
    for (upstream, expected) in [
        ("MAX_TOKENS", "max_tokens"),
        ("CONTENT_FILTERED", "refusal"),
        (
            "MODEL_CONTEXT_WINDOW_EXCEEDED",
            "model_context_window_exceeded",
        ),
        ("STOP_SEQUENCE", "stop_sequence"),
    ] {
        let payload = json!({"stopReason":upstream,"stopSequence":"END"});
        let bytes = event_stream_bytes(vec![
            ("assistantResponseEvent", json!({"content":"answer"})),
            ("metadataEvent", payload.clone()),
        ]);
        let message = kiro_event_bytes_to_anthropic_json(
            &bytes,
            "claude-sonnet-4-8",
            &HashMap::new(),
            KiroPromptCacheUsage::default(),
        )
        .unwrap();
        assert_eq!(message["stop_reason"], expected);
        let mut builder = SseBuilder::new("claude-sonnet-4-8".into(), HashMap::new());
        builder.usage_event("metadataEvent", &payload);
        let output = builder
            .final_events()
            .into_iter()
            .map(|bytes| String::from_utf8(bytes.to_vec()).unwrap())
            .collect::<String>();
        assert!(
            output.contains(&format!("\"stop_reason\":\"{expected}\"")),
            "{output}"
        );
    }
    let mut reason = response::StopReason::default();
    reason.observe(&json!({"stopReason":"CONTENT_FILTERED"}));
    reason.observe(&json!({"stopReason":"MAX_TOKENS"}));
    assert_eq!(reason.finish(true), "refusal");
}

#[test]
fn enhancement_disabled_thinking_never_appears_as_answer_at_any_tag_split() {
    let text = "before<thinking>private thought</thinking>after";
    let mut body = enhancement_body();
    body["thinking"] = json!({"type":"disabled"});
    let registry = enhancement_registry(&body, ToolCompatibilityMode::Raw);
    for split in 0..=text.len() {
        let bytes = event_stream_bytes(vec![
            ("assistantResponseEvent", json!({"content":&text[..split]})),
            ("assistantResponseEvent", json!({"content":&text[split..]})),
            (
                "reasoningContentEvent",
                json!({"text":"native private","signature":"sig","redactedContent":"b3BhcXVl"}),
            ),
        ]);
        let message = kiro_event_bytes_to_anthropic_json_with_context_window(
            &bytes,
            "claude-sonnet-4-8",
            &registry,
            KiroPromptCacheUsage::default(),
            200_000,
        )
        .unwrap();
        assert_eq!(
            message["content"],
            json!([{"type":"text","text":"beforeafter"}]),
            "split {split}"
        );
        let mut builder =
            SseBuilder::new_with_registry("claude-sonnet-4-8".into(), registry.clone(), 200_000);
        let mut output = builder.assistant_delta(&text[..split]);
        output.extend(builder.assistant_delta(&text[split..]));
        output.extend(builder.final_events());
        let output = output
            .into_iter()
            .map(|bytes| String::from_utf8(bytes.to_vec()).unwrap())
            .collect::<String>();
        assert!(
            !output.contains("private") && !output.contains("<thinking>"),
            "split {split}: {output}"
        );
    }
}

#[test]
fn enhancement_read_pages_and_consecutive_tool_results_reach_current_message() {
    let body = json!({"model":"claude-sonnet-4-8","tools":[{"name":"Read","input_schema":{"type":"object","properties":{"file_path":{"type":"string"},"pages":{"type":"string"}}}}],
        "messages":[{"role":"user","content":"read"},{"role":"assistant","content":[{"type":"tool_use","id":"read","name":"Read","input":{"file_path":"a.pdf","pages":"1-3"}}]},
        {"role":"user","content":[{"type":"tool_result","tool_use_id":"read","content":"pages"}]},{"role":"user","content":"summarize"}]});
    let prepared =
        anthropic_to_kiro_request(&body, &test_account(), ToolCompatibilityMode::ClaudeCode)
            .unwrap();
    let current = &prepared.body["conversationState"]["currentMessage"]["userInputMessage"];
    assert_eq!(current["content"], "summarize");
    assert_eq!(
        current["userInputMessageContext"]["toolResults"][0]["toolUseId"],
        "read"
    );
    assert_eq!(
        current["userInputMessageContext"]["tools"][0]["toolSpecification"]["name"],
        "Read"
    );
    assert_eq!(
        prepared.body["conversationState"]["history"][1]["assistantResponseMessage"]["toolUses"][0]
            ["input"]["pages"],
        "1-3"
    );
    assert!(!prepared.body.to_string().contains("\"OK\""));
}

#[test]
fn enhancement_signed_history_uses_native_union_and_orphans_remain_visible() {
    let mut body = enhancement_body();
    body["messages"] = json!([{"role":"user","content":"read"},{"role":"assistant","content":[
        {"type":"thinking","thinking":"private","signature":"upstream-signature"},
        {"type":"tool_use","id":"lost","name":"Read","input":{"file_path":"a"}}]}, {"role":"user","content":"continue"}]);
    let prepared =
        anthropic_to_kiro_request(&body, &test_account(), ToolCompatibilityMode::Raw).unwrap();
    let assistant = &prepared.body["conversationState"]["history"][1]["assistantResponseMessage"];
    assert!(!assistant["content"].as_str().unwrap().contains("private"));
    assert_eq!(
        assistant["reasoningContent"]["reasoningText"]["signature"],
        "upstream-signature"
    );
    assert_eq!(assistant["toolUses"][0]["toolUseId"], "lost");
    assert_eq!(
        prepared.body["conversationState"]["currentMessage"]["userInputMessage"]
            ["userInputMessageContext"]["toolResults"][0]["status"],
        "error"
    );
}

#[test]
fn enhancement_tool_text_recovery_requires_an_entire_declared_call() {
    let mut body = enhancement_body();
    body["tools"] = json!([{"name":"lookup","input_schema":{"type":"object","properties":{"code":{"type":"string"}},"required":["code"],"additionalProperties":false}}]);
    let registry = enhancement_registry(&body, ToolCompatibilityMode::Raw);
    let invoke = "<invoke name=\"lookup\"><parameter name=\"code\">123</parameter></invoke>";
    let mut filter = ToolLeakFilter::with_registry(&registry);
    assert!(filter.push_text(invoke, false).is_empty());
    assert!(filter.push_text("", true).is_empty());
    let recovered = filter.take_deduped(&mut HashSet::new());
    assert_eq!(recovered[0].input, json!({"code":"123"}));
    for text in [
        format!("For example {invoke}"),
        format!("```xml\n{invoke}\n```"),
        format!("{invoke} is an example"),
        invoke.replace("lookup", "unknown"),
    ] {
        let mut filter = ToolLeakFilter::with_registry(&registry);
        let mut visible = filter.push_text(&text, false);
        visible.extend(filter.push_text("", true));
        assert_eq!(visible.concat(), text);
        assert!(filter.take_deduped(&mut HashSet::new()).is_empty());
    }
    // A complete textual invocation must remain inert if later metadata refuses
    // the response, just as native calls do on both output paths.
    let refusal = json!({"stopReason":"CONTENT_FILTERED"});
    let frames = event_stream_bytes(vec![
        ("assistantResponseEvent", json!({"content":invoke})),
        ("metadataEvent", refusal.clone()),
    ]);
    let message = kiro_event_bytes_to_anthropic_json_with_context_window(
        &frames,
        "claude-sonnet-4-8",
        &registry,
        KiroPromptCacheUsage::default(),
        200_000,
    )
    .unwrap();
    assert_eq!(message["stop_reason"], "refusal");
    assert_eq!(message["content"], json!([{"type":"text","text":invoke}]));
    let mut builder = SseBuilder::new_with_registry("claude-sonnet-4-8".into(), registry, 200_000);
    let mut output = builder.assistant_delta(invoke);
    output.extend(
        process_event_to_sse(
            &mut builder,
            wire::Event::Usage {
                event_type: "metadataEvent".into(),
                payload: refusal,
            },
        )
        .unwrap(),
    );
    output.extend(builder.finish_events().unwrap());
    let output = output
        .into_iter()
        .map(|bytes| String::from_utf8(bytes.to_vec()).unwrap())
        .collect::<String>();
    assert!(output.contains("\"stop_reason\":\"refusal\""));
    assert!(!output.contains("\"type\":\"tool_use\""), "{output}");
}

#[test]
fn enhancement_raw_arguments_stream_once_and_partial_failure_stays_incomplete() {
    let mut body = enhancement_body();
    body["stream"] = json!(true);
    body["tools"] = json!([{"name":"lookup","input_schema":{"type":"object"}}]);
    let registry = enhancement_registry(&body, ToolCompatibilityMode::Raw);
    let mut builder =
        SseBuilder::new_with_registry("claude-sonnet-4-8".into(), registry.clone(), 200_000);
    let early = builder
        .tool_delta("call", "lookup", "{\"value\":", false)
        .unwrap();
    let early = early
        .into_iter()
        .map(|b| String::from_utf8(b.to_vec()).unwrap())
        .collect::<String>();
    assert!(early.contains("input_json_delta") && !early.contains("content_block_stop"));
    let late = builder.tool_delta("call", "lookup", "7}", true).unwrap();
    let late = late
        .into_iter()
        .map(|b| String::from_utf8(b.to_vec()).unwrap())
        .collect::<String>();
    assert!(late.contains("content_block_stop"));
    assert!(!late.contains("content_block_start"));
    let mut invalid = SseBuilder::new_with_registry("claude-sonnet-4-8".into(), registry, 200_000);
    invalid.tool_delta("bad", "lookup", "{", false).unwrap();
    assert!(invalid.tool_delta("bad", "lookup", "oops", true).is_err());
}

#[test]
fn enhancement_structured_output_extracts_last_root_value_and_counts_instruction() {
    let schema = json!({"type":"object","properties":{"ok":{"type":"boolean"}},"required":["ok"],"additionalProperties":false});
    assert_eq!(
        structured::extract(
            "first {\"ok\":false}\nanswer ```json\n{\"ok\":true}\n```",
            &schema
        )
        .as_deref(),
        Some("{\"ok\":true}")
    );
    assert!(structured::extract("{\"other\":{\"ok\":true}}", &schema).is_none());
    let mut body = enhancement_body();
    let before = count_input_tokens(&body).unwrap();
    structured::prepare_request(&mut body, Some(schema), true);
    assert!(count_input_tokens(&body).unwrap() > before);
    let mut refusal = json!({"stop_reason":"refusal","content":[{"type":"text","text":"Cannot comply. Example: {\"ok\":true}"}]});
    let unchanged = refusal.clone();
    client_features::finish_structured(&mut refusal, &body);
    assert_eq!(refusal, unchanged);
}

#[test]
fn enhancement_compaction_round_trip_rejects_unrecognized_payloads() {
    let compact=client_features::compact_response(&json!({"model":"claude-sonnet-4-8","content":[{"type":"text","text":"progress and next steps"}],"stop_reason":"end_turn","usage":{}})).unwrap();
    let mut request = json!({"input":compact["output"]});
    client_features::restore_compaction(&mut request).unwrap();
    assert!(request.to_string().contains("progress and next steps"));
    assert!(client_features::restore_compaction(
        &mut json!({"input":[{"type":"compaction","encrypted_content":"other-vendor"}]})
    )
    .is_err());
}

#[tokio::test]
async fn enhancement_clean_eof_requires_complete_nonempty_valid_frames() {
    let registry = KiroToolNameRegistry {
        allow_clean_eof: true,
        ..Default::default()
    };
    let body = enhancement_body();
    let bytes = fixture_event_frame("assistantResponseEvent", &json!({"content":"answer"}));
    assert!(kiro_event_bytes_to_anthropic_json_with_context_window(
        &bytes,
        "claude-sonnet-4-8",
        &registry,
        KiroPromptCacheUsage::default(),
        200_000
    )
    .is_ok());
    assert!(kiro_event_bytes_to_anthropic_json_with_context_window(
        &[],
        "claude-sonnet-4-8",
        &registry,
        KiroPromptCacheUsage::default(),
        200_000
    )
    .is_err());
    let mut corrupt = bytes.clone();
    let last = corrupt.len() - 1;
    corrupt[last] ^= 1;
    assert!(kiro_event_bytes_to_anthropic_json_with_context_window(
        &corrupt,
        "claude-sonnet-4-8",
        &registry,
        KiroPromptCacheUsage::default(),
        200_000
    )
    .is_err());
    let stream = kiro_event_stream_to_claude_sse_scoped_with_timeouts_and_context_window(
        futures_util::stream::iter([Ok(Bytes::from(bytes))]),
        "claude-sonnet-4-8".into(),
        registry,
        &body,
        "clean-eof-fixture",
        None,
        None,
        200_000,
    );
    tokio::pin!(stream);
    let mut text = String::new();
    while let Some(chunk) = stream.next().await {
        text.push_str(std::str::from_utf8(&chunk.unwrap()).unwrap());
    }
    assert!(text.contains("message_stop"));
}

#[test]
fn enhancement_hosted_search_filters_domains_and_bounds_declared_budget() {
    let policy=client_features::SearchPolicy::from_request(&json!({"tools":[{"type":"web_search_20250305","allowed_domains":["example.com"],"blocked_domains":["blocked.example.com"],"max_uses":2}]})).unwrap();
    assert_eq!(policy.max_uses, 2);
    let mut results = vec![
        json!({"url":"https://example.com/a"}),
        json!({"url":"https://sub.example.com/a"}),
        json!({"url":"https://example.com.attacker.test/a"}),
        json!({"url":"https://blocked.example.com/a"}),
    ];
    policy.filter(&mut results);
    assert_eq!(results.len(), 2);
    assert!(client_features::SearchPolicy::from_request(
        &json!({"tools":[{"type":"web_search","max_uses":6}]})
    )
    .is_err());
}

#[test]
fn enhancement_domain_families_keep_runtime_and_management_hosts_distinct() {
    use crate::domain::providers::kiro::KiroApiDomain;
    assert_eq!(
        KiroApiDomain::Kiro.runtime_host("eu-central-1"),
        "runtime.eu-central-1.kiro.dev"
    );
    assert_eq!(
        KiroApiDomain::Kiro.management_host("eu-central-1"),
        "management.eu-central-1.kiro.dev"
    );
    assert_eq!(
        KiroApiDomain::Amazonaws.runtime_host("us-east-1"),
        "q.us-east-1.amazonaws.com"
    );
}

#[test]
fn enhancement_signed_whitespace_and_aggregated_usage_keep_authority() {
    let message =
        json!({"content":[{"type":"thinking","thinking":" ","signature":"vendor-signed"}]});
    assert!(history_reasoning_content(&[&message]).is_some());
    let mut total = json!({});
    for credits in [0.0, 0.25] {
        client_features::append_usage(
            &mut total,
            &json!({"input_tokens":0,"output_tokens":2,"cache_read_input_tokens":4,"cache_creation_input_tokens":0,"cache_usage_source":"upstream_token_usage","credit_usage":credits,"credit_unit":"credits"}),
        );
    }
    assert_eq!(total["input_tokens"], 0);
    assert_eq!(total["cache_read_input_tokens"], 8);
    assert_eq!(total["cache_usage_source"], "upstream_token_usage");
    assert_eq!(total["credit_usage"], 0.25);
}
