use super::*;

fn wrapped(value: Value) -> Bytes {
    Bytes::from(format!(
        "data: {}\n\n",
        json!({"statusCodeValue": 200, "body": value.to_string()})
    ))
}

fn tool_chunk(calls: Value, finish: Option<&str>) -> Bytes {
    wrapped(json!({"choices":[{"index":0,"delta":{"tool_calls":calls},"finish_reason":finish}]}))
}

#[test]
fn qoder_stream_rejects_incomplete_tools_even_with_clean_terminal_and_eof() {
    for call in [
        json!({"index":0,"id":"call-a","function":{"name":"lookup","arguments":"{"}}),
        json!({"index":0,"function":{"name":"lookup","arguments":"{}"}}),
        json!({"index":0,"id":"call-a","function":{"arguments":"{}"}}),
    ] {
        let mut decoder = QoderSseDecoder::default();
        let result = decoder.push(tool_chunk(json!([call]), Some("tool_calls")));
        assert!(result.is_err() || decoder.finish().is_err());
        assert!(!decoder.is_terminal());
        assert!(decoder.finish().is_err(), "failure must be sticky");
    }
}

#[test]
fn qoder_stream_rejects_changed_or_reused_tool_identity() {
    for second in [
        json!({"index":0,"id":"different","function":{"arguments":"1}"}}),
        json!({"index":1,"id":"call-a","function":{"name":"second","arguments":"{}"}}),
    ] {
        let mut decoder = QoderSseDecoder::default();
        decoder
            .push(tool_chunk(
                json!([{"index":0,"id":"call-a","function":{"name":"lookup","arguments":"{\"q\":"}}]),
                None,
            ))
            .unwrap();
        let result = decoder.push(tool_chunk(json!([second]), Some("tool_calls")));
        assert!(result.is_err() || decoder.finish().is_err());
        assert!(!decoder.is_terminal());
    }
}

#[test]
fn qoder_stream_accepts_complete_fragmented_tool_arguments() {
    let mut decoder = QoderSseDecoder::default();
    let mut canonical = decoder
        .push(tool_chunk(
            json!([{"index":0,"id":"call-a","function":{"name":"lookup","arguments":"{\"q\":"}}]),
            None,
        ))
        .unwrap()
        .to_vec();
    canonical.extend_from_slice(
        &decoder
            .push(tool_chunk(
                json!([{"index":0,"function":{"arguments":"1}"}}]),
                Some("tool_calls"),
            ))
            .unwrap(),
    );
    canonical.extend_from_slice(&decoder.finish().unwrap());
    let mut aggregator = QoderChatSseAggregator::default();
    aggregator.push(Bytes::from(canonical)).unwrap();
    let response = aggregator.finish("auto", 1).unwrap();
    assert_eq!(
        response["choices"][0]["message"]["tool_calls"][0]["function"]["arguments"],
        "{\"q\":1}"
    );
}

fn decoder_for(names: &[&str], require_tool: bool) -> QoderSseDecoder {
    let mut decoder = QoderSseDecoder::default();
    decoder
        .set_output_contract(&QoderOutputContract {
            allowed_names: names.iter().map(|name| name.to_string()).collect(),
            require_tool,
            recover_text_tools: false,
        })
        .unwrap();
    decoder
}

#[test]
fn qoder_stream_holds_identity_until_declared_name_and_id_are_complete() {
    let mut decoder = decoder_for(&["read_file"], false);
    assert!(decoder
        .push(tool_chunk(
            json!([{"index":0,"function":{"name":"read_","arguments":"{"}}]),
            None
        ))
        .unwrap()
        .is_empty());
    let output = decoder
        .push(tool_chunk(
            json!([{"index":0,"id":"a","function":{"name":"file","arguments":"}"}}]),
            Some("tool_calls"),
        ))
        .unwrap();
    let text = std::str::from_utf8(&output).unwrap();
    assert!(text.contains("read_file"));
    assert!(!text.contains("read_filefile"));
    decoder.finish().unwrap();
}

#[test]
fn qoder_stream_name_prefix_ambiguity_is_resolved_without_guessing() {
    for (fragment, expected) in [("", "read"), ("_file", "read_file")] {
        let mut decoder = decoder_for(&["read", "read_file"], false);
        assert!(decoder
            .push(tool_chunk(
                json!([{"index":0,"id":"a","function":{"name":"read","arguments":"{"}}]),
                None
            ))
            .unwrap()
            .is_empty());
        let output = decoder
            .push(tool_chunk(
                json!([{"index":0,"function":{"name":fragment,"arguments":"}"}}]),
                Some("tool_calls"),
            ))
            .unwrap();
        let text = std::str::from_utf8(&output).unwrap();
        assert!(text.contains(&format!("\"name\":\"{expected}\"")));
        decoder.finish().unwrap();
    }
}

#[test]
fn qoder_identical_name_fragments_are_not_mistaken_for_a_repeated_complete_name() {
    for (name, fragment) in [("aa", "a"), ("查查", "查")] {
        let mut decoder = decoder_for(&[name], true);
        assert!(decoder.push(tool_chunk(json!([{"index":0,"id":"repeated-fragment","function":{"name":fragment,"arguments":"{"}}]),None)).unwrap().is_empty());
        let output = decoder
            .push(tool_chunk(
                json!([{"index":0,"function":{"name":fragment,"arguments":"}"}}]),
                Some("tool_calls"),
            ))
            .unwrap();
        assert!(String::from_utf8_lossy(&output).contains(name));
        decoder.finish().unwrap();
    }
}

#[test]
fn qoder_stream_enforces_none_named_required_and_repeated_full_name() {
    let mut none = decoder_for(&[], false);
    assert!(none
        .push(tool_chunk(
            json!([{"index":0,"id":"a","function":{"name":"read","arguments":"{}"}}]),
            Some("tool_calls")
        ))
        .is_err());
    let mut required = decoder_for(&["read"], true);
    required
        .push(wrapped(
            json!({"choices":[{"delta":{"content":"not a tool"},"finish_reason":"stop"}]}),
        ))
        .unwrap();
    assert!(required.finish().is_err());
    let mut named = decoder_for(&["read"], true);
    assert!(named
        .push(tool_chunk(
            json!([{"index":0,"id":"a","function":{"name":"write","arguments":"{}"}}]),
            Some("tool_calls")
        ))
        .is_err());
    let mut repeated = decoder_for(&["read"], true);
    repeated
        .push(tool_chunk(
            json!([{"index":0,"id":"a","function":{"name":"read","arguments":"{"}}]),
            None,
        ))
        .unwrap();
    let output = repeated
        .push(tool_chunk(
            json!([{"index":0,"id":"a","function":{"name":"read","arguments":"}"}}]),
            Some("tool_calls"),
        ))
        .unwrap();
    assert!(!std::str::from_utf8(&output)
        .unwrap()
        .contains("\"name\":\"read\""));
    repeated.finish().unwrap();
}

#[test]
fn qoder_opt_in_text_tools_recover_only_whole_declared_valid_calls_across_chunks() {
    for text in [
        "<tool_call>read {\"path\":\"a\"}</tool_call>",
        "[assistant 请求调用工具][{\"name\":\"read\",\"arguments\":{\"path\":\"a\"}}]",
        "<|DSML|calls><|DSML|invoke name=\"read\"><|DSML|parameter name=\"path\">a</|DSML|parameter></|DSML|invoke></|DSML|calls>",
        "<｜DSML｜calls><｜DSML｜invoke name=\"read\"><｜DSML｜parameter name=\"path\">a</｜DSML｜parameter></｜DSML｜invoke></｜DSML｜calls>",
    ] {
        let mut decoder = decoder_for(&["read"], true);
        decoder.set_output_contract(&QoderOutputContract { allowed_names:["read".to_string()].into_iter().collect(), require_tool:true, recover_text_tools:true }).unwrap();
        let mut output = Vec::new();
        for ch in text.chars() {
            output.extend_from_slice(&decoder.push(wrapped(json!({"choices":[{"delta":{"content":ch.to_string()},"finish_reason":null}]}))).unwrap());
        }
        output.extend_from_slice(&decoder.push(wrapped(json!({"choices":[{"delta":{},"finish_reason":"stop"}]}))).unwrap());
        output.extend_from_slice(&decoder.finish().unwrap());
        let mut aggregator = QoderChatSseAggregator::default();
        aggregator.push(Bytes::from(output)).unwrap();
        let response = aggregator.finish("auto",1).unwrap();
        let calls = &response["choices"][0]["message"]["tool_calls"];
        assert_eq!(calls[0]["function"]["name"],"read");
        assert_eq!(serde_json::from_str::<Value>(calls[0]["function"]["arguments"].as_str().unwrap()).unwrap(),json!({"path":"a"}));
        assert_eq!(response["choices"][0]["finish_reason"],"tool_calls");
    }
}

#[test]
fn qoder_text_tool_recovery_never_executes_prose_or_repairs_malformed_json() {
    for (text, valid_text) in [
        (
            "Example: <tool_call>read {\"path\":\"a\"}</tool_call>",
            true,
        ),
        ("```<tool_call>read {\"path\":\"a\"}</tool_call>```", true),
        (
            "<tool_call>read {\"path\":\"unescaped \"quote\"\"}</tool_call>",
            false,
        ),
        ("<tool_call>unknown {}</tool_call>", false),
        ("<tool_call>read {}</tool_call> prose", false),
        ("<tool_call>read {", false),
        (
            "<tool_call>read {\"path\":\"a\",\"path\":\"b\"}</tool_call>",
            false,
        ),
        (
            "[assistant 请求调用工具][{\"name\":\"read\",\"name\":\"read\",\"arguments\":{}}]",
            false,
        ),
        (
            "<tool_call>{\"name\":\"read\",\"arguments\":{},\"extra\":true}</tool_call>",
            false,
        ),
    ] {
        let mut decoder = decoder_for(&["read"], false);
        decoder
            .set_output_contract(&QoderOutputContract {
                allowed_names: ["read".to_string()].into_iter().collect(),
                require_tool: false,
                recover_text_tools: true,
            })
            .unwrap();
        let result = decoder.push(wrapped(
            json!({"choices":[{"delta":{"content":text},"finish_reason":"stop"}]}),
        ));
        if valid_text {
            let result = result.unwrap();
            assert!(!std::str::from_utf8(&result)
                .unwrap()
                .contains("call_qoder_echo_"));
            decoder.finish().unwrap();
        } else {
            assert!(result.is_err());
            assert!(!decoder.is_terminal());
        }
    }
    let mut decoder = decoder_for(&["read"], false);
    let result = decoder.push(wrapped(json!({"choices":[{"delta":{"content":"<tool_call>read {}</tool_call>"},"finish_reason":"stop"}]}))).unwrap();
    assert!(!std::str::from_utf8(&result)
        .unwrap()
        .contains("call_qoder_echo_"));
    decoder.finish().unwrap();
}

#[test]
fn qoder_image_conversion_timeout_is_not_auth_or_permission_or_a_replay_trigger() {
    let timeout = QoderUpstreamError::from_status_body(502, br#"{"message":"Images timeout"}"#);
    assert_eq!(
        timeout.downstream_status(),
        axum::http::StatusCode::GATEWAY_TIMEOUT
    );
    assert!(!timeout.is_authentication_failure());
    for (status, body) in [
        (401, br#"{"message":"Images timeout"}"#.as_slice()),
        (403, br#"{"message":"Images timeout"}"#.as_slice()),
        (
            502,
            br#"{"code":"112","message":"Images timeout"}"#.as_slice(),
        ),
    ] {
        assert!(!QoderUpstreamError::from_status_body(status, body).is_image_conversion_timeout());
    }
}

#[test]
fn qoder_media_errors_redact_signed_urls_and_inline_images_before_truncation() {
    for message in [
        "Images timeout https://images.example.invalid/a?signature=private-value",
        "Images timeout data:image/png;base64,aW1hZ2Utc2VjcmV0",
        r#"{"message":"image conversion timeout https://images.example.invalid/a?signature=private-value"}"#,
    ] {
        let error = QoderUpstreamError::from_status_body(
            502,
            &serde_json::to_vec(&json!({"message":message})).unwrap(),
        );
        assert!(error.is_image_conversion_timeout());
        assert!(error.message.contains("[REDACTED_MEDIA]"));
        for secret in [
            "private-value",
            "aW1hZ2Utc2VjcmV0",
            "images.example.invalid",
        ] {
            assert!(!error.message.contains(secret), "{}", error.message);
        }
    }
}

#[test]
fn qoder_native_empty_object_tools_are_valid_without_inventing_missing_arguments() {
    for with_contract in [false, true] {
        let mut decoder = if with_contract {
            decoder_for(&["read"], true)
        } else {
            QoderSseDecoder::default()
        };
        let mut output = decoder
            .push(wrapped(json!({"type":"content_block_start","index":0,
            "content_block":{"type":"tool_use","id":"empty-call","name":"read","input":{}}})))
            .unwrap()
            .to_vec();
        output.extend_from_slice(
            &decoder
                .push(wrapped(
                    json!({"type":"message_delta","delta":{"stop_reason":"tool_use"}}),
                ))
                .unwrap(),
        );
        output.extend_from_slice(
            &decoder
                .push(wrapped(json!({"type":"message_stop"})))
                .unwrap(),
        );
        output.extend_from_slice(&decoder.finish().unwrap());
        assert!(!String::from_utf8_lossy(&output).contains("_qoder_empty_object_input"));
        let mut aggregator = QoderChatSseAggregator::default();
        aggregator.push(Bytes::from(output)).unwrap();
        let response = aggregator.finish("auto", 1).unwrap();
        assert_eq!(
            response["choices"][0]["message"]["tool_calls"][0]["function"]["arguments"],
            "{}"
        );
    }
    let mut decoder = decoder_for(&["read"], true);
    decoder
        .push(tool_chunk(
            json!([{"index":0,"id":"no-arguments","function":{"name":"read"}}]),
            Some("tool_calls"),
        ))
        .unwrap();
    assert!(decoder.finish().is_err());
}

#[test]
fn qoder_delayed_tools_and_echo_parser_obey_shared_memory_and_release_on_drop() {
    for echo in [false, true] {
        let budget = RequestMemoryBudget::new(128 * 1024);
        {
            let mut decoder = QoderSseDecoder::with_request_memory(Some(budget.clone())).unwrap();
            decoder
                .set_output_contract(&QoderOutputContract {
                    allowed_names: ["read".to_string()].into_iter().collect(),
                    require_tool: false,
                    recover_text_tools: echo,
                })
                .unwrap();
            let event = if echo {
                wrapped(
                    json!({"choices":[{"delta":{"content":format!("<tool_call>read {{\"payload\":\"{}\"}}</tool_call>", "x".repeat(2048))},"finish_reason":"stop"}]}),
                )
            } else {
                tool_chunk(
                    json!([{"index":0,"function":{"name":"r","arguments":"x".repeat(48 * 1024)}}]),
                    None,
                )
            };
            let error = decoder.push(event).unwrap_err();
            assert!(error.is_request_memory_exhausted(), "{echo}: {error:?}");
            assert!(!decoder.is_terminal());
        }
        assert_eq!(budget.snapshot().used_bytes, 0);
        assert!(budget.snapshot().exhausted);
    }
}
