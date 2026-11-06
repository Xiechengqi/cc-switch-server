fn solo_wire(event: &str, value: Value) -> String {
    format!("event: {event}\ndata: {value}\n\n")
}

fn weather_contract(choice: &str) -> TraeToolContract {
    TraeToolContract::from_payload(&json!({
        "tools":[{"type":"function","function":{"name":"Weather","parameters":{"type":"object","properties":{"city":{"type":"string"}},"required":["city"]}}}],
        "tool_choice":choice
    })).unwrap()
}

#[test]
fn trae_queue_and_fragmented_parallel_tool_identity_placeholders() {
    let mut decoder = TraeSseDecoder::with_contract("fixture", weather_contract("required"));
    let wire = solo_wire("progress_notice", json!("Processing_fixture"))
        + &solo_wire(
            "output",
            json!({"tool_calls":[
                {"index":0,"id":"c0","type":"function","function_call":{"name":"Weather","arguments":"{\"city\":"}},
                {"index":1,"id":"c1","type":"function","function_call":{"name":"Weather","arguments":"{\"city\":"}}
            ]}),
        )
        + &solo_wire(
            "output",
            json!({"tool_calls":[
                {"index":1,"id":"","type":"","function_call":{"name":"","arguments":"\"北京\"}"}},
                {"index":0,"id":"","type":"","function_call":{"name":"","arguments":"\"南京\"}"}}
            ]}),
        )
        + &solo_wire("done", json!({"finish_reason":"stop"}));
    let mut streamed = Vec::new();
    // Includes splits inside UTF-8 scalars and inside SSE/JSON syntax.
    for byte in wire.as_bytes() {
        streamed.extend_from_slice(
            &decoder
                .push_classified(Bytes::copy_from_slice(&[*byte]))
                .unwrap(),
        );
    }
    assert!(!String::from_utf8_lossy(&streamed).contains("[DONE]"));
    decoder.finish_classified().unwrap();
    let response = decoder.into_chat_completion().unwrap();
    assert_eq!(response["choices"][0]["finish_reason"], "tool_calls");
    assert_eq!(
        response["choices"][0]["message"]["tool_calls"][1]["function"]["arguments"],
        "{\"city\":\"北京\"}"
    );
}

#[test]
fn trae_tool_completion_rejects_non_objects_truncation_and_undeclared_calls() {
    for (name, arguments, reason) in [
        ("Weather", "[]", "stop"),
        ("Weather", "null", "stop"),
        ("Weather", "{\"city\":\"x\"}", "length"),
        ("Weather", "", "length"),
        ("Weather", "{\"city\":", "stop"),
        ("Other", "{}", "stop"),
        ("Weather", "{}", "stop"),
    ] {
        let mut decoder = TraeSseDecoder::with_contract("fixture", weather_contract("auto"));
        let wire = solo_wire(
            "output",
            json!({"tool_calls":[{"index":0,"id":"c","function_call":{"name":name,"arguments":arguments}}]}),
        ) + &solo_wire("done", json!({"finish_reason":reason}));
        assert!(
            decoder
                .push_classified(Bytes::from(wire))
                .and_then(|_| decoder.finish_classified())
                .is_err(),
            "{name} / {arguments} / {reason}"
        );
    }
    let mut required = TraeSseDecoder::with_contract("fixture", weather_contract("required"));
    assert!(required
        .push_classified(Bytes::from(solo_wire("done", json!({}))))
        .is_err());
}

#[test]
fn trae_tool_history_backfills_names_without_trimming_ids_or_losing_empty_results() {
    let request = json!({"messages":[
        {"role":"developer","content":"first"}, {"role":"system","content":"second"},
        {"role":"assistant","content":null,"tool_calls":[{"id":"c","function":{"name":"Weather","arguments":"{}"}}]},
        {"role":"tool","tool_call_id":"c","content":""},
        {"role":"user","content":"continue"}
    ]});
    let payload = build_trae_payload(&request, &capability().id, &capability()).unwrap();
    assert_eq!(payload["messages"][0]["role"], "system");
    assert_eq!(payload["messages"][1]["content"][0]["text"], "second");
    assert_eq!(payload["messages"][3]["name"], "Weather");
    assert_eq!(payload["messages"][3]["content"][0]["text"], "");
    for id in [" c", "c ", "unknown"] {
        let mut invalid = request.clone();
        invalid["messages"][3]["tool_call_id"] = json!(id);
        assert!(build_trae_payload(&invalid, &capability().id, &capability()).is_err());
    }
}

#[test]
fn trae_parameter_aliases_and_constraints_have_explicit_semantics() {
    for alias in ["max_tokens", "max_completion_tokens", "max_output_tokens"] {
        let mut request =
            json!({"messages":[{"role":"user","content":"hi"}],"reasoning":{"effort":"high"}});
        request[alias] = json!(32);
        let payload = build_trae_payload(&request, &capability().id, &capability()).unwrap();
        assert_eq!(payload["max_tokens"], 32);
        assert_eq!(payload["reasoning_effort_level"], "high");
        assert!(payload.get("reasoning").is_none());
    }
    for controls in [
        json!({"max_tokens":32,"max_completion_tokens":16}),
        json!({"max_tokens":0}),
        json!({"temperature":3}),
        json!({"response_format":{"type":"json_schema","schema":{}}}),
        json!({"tool_choice":"required"}),
        json!({"reasoning_effort":"high","reasoning":{"effort":"low"}}),
    ] {
        let mut request = json!({"messages":[{"role":"user","content":"hi"}]});
        request
            .as_object_mut()
            .unwrap()
            .extend(controls.as_object().unwrap().clone());
        assert!(
            build_trae_payload(&request, &capability().id, &capability()).is_err(),
            "{controls}"
        );
    }
}

#[test]
fn trae_sse_transport_supports_multiline_data_and_all_line_endings() {
    for newline in ["\n", "\r\n", "\r"] {
        let wire = [
            "id: fixture",
            "event: output",
            "data: {",
            "data: \"response\":\"hi\"}",
            "",
            "event: done",
            "data: {}",
            "",
            "",
        ]
        .join(newline);
        let mut decoder = TraeSseDecoder::new("fixture");
        decoder.push_classified(Bytes::from(wire)).unwrap();
        decoder.finish_classified().unwrap();
        assert_eq!(
            decoder.into_chat_completion().unwrap()["choices"][0]["message"]["content"],
            "hi"
        );
    }
}

#[test]
fn trae_raw_images_and_opt_in_tool_contract_preserve_history_and_parameters() {
    let mut caps = capability();
    caps.max_mode = false;
    let request = json!({"messages":[{"role":"system","content":"first"},{"role":"developer","content":"second"},{"role":"assistant","tool_calls":[{"id":"call-1","function":{"name":"lookup","arguments":"{\"n\":1}"}}]},{"role":"tool","tool_call_id":"call-1","content":""},{"role":"user","content":[{"type":"image_url","image_url":{"url":"data:image/png;base64,AA=="}}]}],"tools":[{"type":"function","function":{"name":"lookup","parameters":{"type":"object","properties":{"n":{"type":"integer"}},"required":["n"]}}}],"tool_choice":"required","max_output_tokens":128,"temperature":0.5,"top_p":0.9,"stop":"end","reasoning":{"effort":"high"}});
    assert!(build_raw_payload(&request, &caps.id, &caps, false)
        .unwrap_err()
        .contains("explicit"));
    let (body, contract) = build_raw_payload(&request, &caps.id, &caps, true).unwrap();
    assert!(body.get("function").is_none());
    assert!(body.get("config_name").is_none());
    assert!(body.get("tools").is_none());
    assert_eq!(body["parameters"]["max_tokens"], 128);
    assert_eq!(body["parameters"]["reasoning_effort_level"], "high");
    let serialized = body.to_string();
    assert!(
        serialized.contains("first")
            && serialized.contains("second")
            && serialized.contains("data:image/png")
    );
    assert!(serialized.contains("call-1"));
    let mut decoder = TraeSseDecoder::with_raw(&caps.id, contract, true);
    let text =
        "<|FunctionCallBegin|>[{\"name\":\"lookup\",\"parameters\":{\"n\":2}}]<|FunctionCallEnd|>";
    let mut emitted = String::new();
    for character in text.chars() {
        let chunk = format!("data: {}\n\n", json!({"response":character.to_string()}));
        emitted.push_str(
            std::str::from_utf8(&decoder.push_classified(Bytes::from(chunk)).unwrap()).unwrap(),
        );
    }
    decoder
        .push_classified(Bytes::from_static(b"data: [DONE]\n\n"))
        .unwrap();
    decoder.finish_classified().unwrap();
    assert!(!emitted.contains("FunctionCallBegin"));
    let result = decoder.into_chat_completion().unwrap();
    assert_eq!(result["choices"][0]["finish_reason"], "tool_calls");
    assert_eq!(
        result["choices"][0]["message"]["tool_calls"][0]["function"]["arguments"],
        "{\"n\":2}"
    );
}

#[test]
fn trae_raw_truncated_or_unknown_tool_markers_never_complete() {
    for text in [
        "<|FunctionCallBegin|>[",
        "<|FunctionCallBegin|>[{\"name\":\"unknown\",\"parameters\":{}}]<|FunctionCallEnd|>",
        "<|FunctionCallBegin|>[{\"name\":\"lookup\",\"parameters\":null}]<|FunctionCallEnd|>",
    ] {
        let contract=TraeToolContract::from_payload(&json!({"tools":[{"function":{"name":"lookup","parameters":{"type":"object","properties":{}}}}]})).unwrap();
        let mut decoder = TraeSseDecoder::with_raw("model", contract, true);
        let wire = format!(
            "event: output\ndata: {}\n\nevent: done\ndata: {{}}\n\n",
            json!({"response":text})
        );
        assert!(decoder.push_classified(Bytes::from(wire)).is_err());
        assert!(!decoder.is_terminal());
    }
}

#[test]
fn trae_usage_details_and_memory_exhaustion_remain_explicit() {
    let mut decoder = TraeSseDecoder::new("model");
    decoder.push_classified(Bytes::from_static(b"event: token_usage\ndata: {\"prompt_tokens\":10,\"completion_tokens\":4,\"cache_read_input_tokens\":3,\"cache_creation_input_tokens\":2,\"reasoning_tokens\":1}\n\nevent: done\ndata: {}\n\n")).unwrap();
    decoder.finish_classified().unwrap();
    let result = decoder.into_chat_completion().unwrap();
    assert_eq!(result["usage"]["total_tokens"], 14);
    assert_eq!(result["usage"]["prompt_tokens_details"]["cached_tokens"], 3);
    assert_eq!(
        result["usage"]["completion_tokens_details"]["reasoning_tokens"],
        1
    );
    let budget = super::super::request_memory::RequestMemoryBudget::new(4096);
    {
        let mut decoder = TraeSseDecoder::new("model");
        decoder.attach_memory(Some(budget.clone())).unwrap();
        let error = decoder
            .push_classified(Bytes::from_static(
                b"event: output\ndata: {\"response\":\"hello\"}\n\n",
            ))
            .unwrap_err()
            .into_proxy_error();
        assert_eq!(error.status, axum::http::StatusCode::SERVICE_UNAVAILABLE);
    }
    assert_eq!(budget.snapshot().used_bytes, 0);
    for value in [
        json!({"prompt_tokens":-1}),
        json!({"completion_tokens":1.5}),
        json!({"prompt_tokens":1,"cache_read_input_tokens":2}),
    ] {
        let mut decoder = TraeSseDecoder::new("model");
        assert!(decoder
            .push_classified(Bytes::from(format!(
                "event: token_usage\ndata: {value}\n\n"
            )))
            .is_err());
    }
}
