use crate::proxy::adapters::ProviderAdapter;

fn gemini_r6_frames(bytes: &[u8]) -> Vec<Value> {
    String::from_utf8_lossy(bytes)
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .filter_map(|raw| serde_json::from_str(raw).ok())
        .collect()
}

fn gemini_r6_body() -> Value {
    json!({"candidates":[{"content":{"role":"model","parts":[{"text":"hello"}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":1,"candidatesTokenCount":2}})
}

fn gemini_r6_stored(product: crate::domain::providers::model::ProviderType) -> StoredProvider {
    gemini_v1internal_stored_provider(crate::domain::providers::model::AppKind::Claude, product)
}

fn gemini_r6_stream(stored: &StoredProvider, values: &[Value]) -> Vec<Value> {
    let mut transformer = StreamEventTransformer::new(
        stored,
        ProxyRoute::ClaudeMessages,
        transforms::ResponsesToolContext::default(),
    );
    let mut output = Vec::new();
    for value in values {
        let wire = format!("data: {}\n\n", json!({"response":value}));
        for bytes in wire.as_bytes().chunks(7) {
            output.extend_from_slice(&transformer.push(Bytes::copy_from_slice(bytes)).unwrap());
        }
    }
    output.extend_from_slice(&transformer.finish().unwrap());
    gemini_r6_frames(&output)
}

fn gemini_r6_message_id(frames: &[Value]) -> &str {
    frames
        .iter()
        .find(|frame| frame["type"] == "message_start")
        .unwrap()["message"]["id"]
        .as_str()
        .unwrap()
}

#[test]
fn gemini_r6_adapter_missing_and_blank_ids_are_unique_and_snake_ids_survive() {
    use crate::domain::providers::model::{AppKind, ProviderType};
    for product in [ProviderType::AntigravityOAuth, ProviderType::AgyOAuth] {
        let stored = gemini_r6_stored(product);
        let adapter = crate::proxy::adapters::adapter_for(AppKind::Claude, product);
        for id in [
            None,
            Some(Value::Null),
            Some(json!("")),
            Some(json!(" \t ")),
        ] {
            let mut value = gemini_r6_body();
            if let Some(id) = id {
                value["responseId"] = id;
            }
            let mut ids = Vec::new();
            for _ in 0..2 {
                let body = adapter
                    .transform_response(
                        Bytes::from(json!({"response":value}).to_string()),
                        &stored,
                        ProxyRoute::ClaudeMessages,
                    )
                    .unwrap();
                let response: Value = serde_json::from_slice(&body).unwrap();
                let id = response["id"].as_str().unwrap();
                assert!(!id.trim().is_empty());
                ids.push(id.to_string());
            }
            assert_ne!(
                ids[0], ids[1],
                "different responses cannot share the fallback message ID"
            );
        }
        let mut value = gemini_r6_body();
        value["responseId"] = Value::Null;
        value["response_id"] = json!(" exact-snake-id ");
        let body = adapter
            .transform_response(
                Bytes::from(json!({"response":value}).to_string()),
                &stored,
                ProxyRoute::ClaudeMessages,
            )
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&body).unwrap()["id"],
            " exact-snake-id "
        );
    }
}

#[test]
fn gemini_r6_stream_missing_ids_are_per_response_and_late_ids_never_rebind() {
    use crate::domain::providers::model::ProviderType;
    for product in [ProviderType::AntigravityOAuth, ProviderType::AgyOAuth] {
        let stored = gemini_r6_stored(product);
        let first = gemini_r6_stream(&stored, &[gemini_r6_body()]);
        let second = gemini_r6_stream(&stored, &[gemini_r6_body()]);
        assert_ne!(gemini_r6_message_id(&first), gemini_r6_message_id(&second));
        let events = gemini_r6_stream(
            &stored,
            &[
                json!({"candidates":[{"content":{"parts":[{"text":"first"}]}}]}),
                json!({"responseId":"late-id","candidates":[{"content":{"parts":[{"text":"last"}]},"finishReason":"STOP"}]}),
            ],
        );
        assert_eq!(
            events
                .iter()
                .filter(|e| e["type"] == "message_start")
                .count(),
            1
        );
        assert_ne!(gemini_r6_message_id(&events), "late-id");
    }
}

#[test]
fn gemini_r6_unidentified_argument_continuation_never_becomes_an_executable_tool() {
    use crate::domain::providers::model::ProviderType;
    for product in [ProviderType::AntigravityOAuth, ProviderType::AgyOAuth] {
        let stored = gemini_r6_stored(product);
        let mut transformer = StreamEventTransformer::new(
            &stored,
            ProxyRoute::ClaudeMessages,
            transforms::ResponsesToolContext::default(),
        );
        let first = format!(
            "data: {}\n\n",
            json!({"response":{"candidates":[{"content":{"parts":[{"functionCall":{"name":"lookup","args":{"q":"a"}}}]}}]}})
        );
        let mut output = transformer.push(Bytes::from(first)).unwrap().to_vec();
        let wire = format!(
            "data: {}\n\n",
            json!({"response":{"candidates":[{"content":{"parts":[{"functionCall":{"args":{"limit":2}}}]},"finishReason":"STOP"}]}})
        );
        match transformer.push(Bytes::from(wire)) {
            Err(_) => assert!(
                transformer.finish().is_err(),
                "failed identity must not publish buffered tools later"
            ),
            Ok(bytes) => {
                output.extend_from_slice(&bytes);
                output.extend_from_slice(&transformer.finish().unwrap());
                let text = String::from_utf8(output).unwrap();
                assert!(
                    text.contains("\"name\":\"tool\""),
                    "baseline counterexample did not reach default tool construction: {text}"
                );
                panic!("unidentified args were turned into an executable default tool: {text}");
            }
        }
    }
}

#[test]
fn gemini_r6_identity_shapes_blocked_refusal_and_early_metadata_are_consistent() {
    use crate::domain::providers::model::{AppKind, ProviderType};
    for product in [ProviderType::AntigravityOAuth, ProviderType::AgyOAuth] {
        let stored = gemini_r6_stored(product);
        let adapter = crate::proxy::adapters::adapter_for(AppKind::Claude, product);
        for blocked in [false, true] {
            for id in [
                None,
                Some(Value::Null),
                Some(json!("")),
                Some(json!(" \n\t ")),
                Some(json!(17)),
            ] {
                let mut value = if blocked {
                    json!({"candidates":[],"promptFeedback":{"blockReason":"SAFETY"},"usageMetadata":{"promptTokenCount":1}})
                } else {
                    gemini_r6_body()
                };
                if let Some(id) = id {
                    value["responseId"] = id;
                }
                let raw = adapter
                    .transform_response(
                        Bytes::from(json!({"response":value}).to_string()),
                        &stored,
                        ProxyRoute::ClaudeMessages,
                    )
                    .unwrap();
                let response: Value = serde_json::from_slice(&raw).unwrap();
                assert!(response["id"].as_str().unwrap().starts_with("msg_"));
                let stream = gemini_r6_stream(&stored, &[value]);
                assert!(gemini_r6_message_id(&stream).starts_with("msg_"));
                if blocked {
                    assert_eq!(response["stop_reason"], "refusal");
                    assert!(stream.iter().any(
                        |frame| frame.pointer("/delta/stop_reason") == Some(&json!("refusal"))
                    ));
                }
            }
        }
        let mut value = gemini_r6_body();
        value["responseId"] = json!("  exact-camel  ");
        value["response_id"] = json!("ignored-snake");
        let raw = adapter
            .transform_response(
                Bytes::from(json!({"response":value}).to_string()),
                &stored,
                ProxyRoute::ClaudeMessages,
            )
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&raw).unwrap()["id"],
            "  exact-camel  "
        );
        let stream = gemini_r6_stream(
            &stored,
            &[
                json!({"responseId":null,"response_id":" early-snake ","usageMetadata":{"promptTokenCount":1}}),
                gemini_r6_body(),
            ],
        );
        assert_eq!(gemini_r6_message_id(&stream), " early-snake ");
    }
}

#[test]
fn gemini_r6_grounding_tool_ids_are_unique_and_pair_on_late_metadata() {
    use crate::domain::providers::model::{AppKind, ProviderType};
    for product in [ProviderType::AntigravityOAuth, ProviderType::AgyOAuth] {
        let stored = gemini_r6_stored(product);
        let adapter = crate::proxy::adapters::adapter_for(AppKind::Claude, product);
        let metadata = json!({"webSearchQueries":["hello"],"groundingChunks":[{"web":{"uri":"https://example.com","title":"Example"}}],"groundingSupports":[{"segment":{"startIndex":0,"endIndex":5,"text":"hello"},"groundingChunkIndices":[0]}]});
        let mut ids = std::collections::BTreeSet::new();
        for _ in 0..2 {
            let mut value = gemini_r6_body();
            value["candidates"][0]["groundingMetadata"] = metadata.clone();
            let raw = adapter
                .transform_response(
                    Bytes::from(json!({"response":value}).to_string()),
                    &stored,
                    ProxyRoute::ClaudeMessages,
                )
                .unwrap();
            let response: Value = serde_json::from_slice(&raw).unwrap();
            let blocks = response["content"].as_array().unwrap();
            let use_id = blocks
                .iter()
                .find(|block| block["type"] == "server_tool_use")
                .unwrap()["id"]
                .as_str()
                .unwrap();
            let result_id = blocks
                .iter()
                .find(|block| block["type"] == "web_search_tool_result")
                .unwrap()["tool_use_id"]
                .as_str()
                .unwrap();
            assert_eq!(use_id, result_id);
            assert!(ids.insert(use_id.to_string()));
            assert_eq!(
                response["usage"]["server_tool_use"]["web_search_requests"],
                1
            );
            let events = gemini_r6_stream(
                &stored,
                &[
                    json!({"candidates":[{"content":{"parts":[{"text":"hello"}]}}]}),
                    json!({"responseId":"late-ignored","candidates":[{"groundingMetadata":metadata,"finishReason":"STOP"}]}),
                ],
            );
            let use_id = events
                .iter()
                .filter_map(|frame| frame.get("content_block"))
                .find(|block| block["type"] == "server_tool_use")
                .unwrap()["id"]
                .as_str()
                .unwrap();
            let result_id = events
                .iter()
                .filter_map(|frame| frame.get("content_block"))
                .find(|block| block["type"] == "web_search_tool_result")
                .unwrap()["tool_use_id"]
                .as_str()
                .unwrap();
            assert_eq!(use_id, result_id);
            assert!(ids.insert(use_id.to_string()));
            assert_eq!(
                events
                    .iter()
                    .filter(|frame| frame["type"] == "message_start")
                    .count(),
                1
            );
            assert_ne!(gemini_r6_message_id(&events), "late-ignored");
        }
    }
}

#[test]
fn gemini_r6_explicit_tool_ids_continue_without_a_name_and_empty_calls_remain_valid() {
    use crate::domain::providers::model::ProviderType;
    for product in [ProviderType::AntigravityOAuth, ProviderType::AgyOAuth] {
        let stored = gemini_r6_stored(product);
        let events = gemini_r6_stream(
            &stored,
            &[
                json!({"candidates":[{"content":{"parts":[{"functionCall":{"id":"call-a","name":"lookup","args":{"q":"a"}}},{"functionCall":{"id":"call-b","name":"lookup","args":{}}}]}}]}),
                json!({"candidates":[{"content":{"parts":[{"functionCall":{"id":"call-b","args":{"limit":2}}},{"functionCall":{"id":"call-a","args":{"limit":3}},"thoughtSignature":"opaque-signature"}]},"finishReason":"STOP"}]}),
            ],
        );
        let tools: Vec<_> = events
            .iter()
            .filter(|frame| frame.pointer("/content_block/type") == Some(&json!("tool_use")))
            .collect();
        assert_eq!(tools.len(), 2);
        for (id, expected) in [
            ("call-a", json!({"q":"a","limit":3})),
            ("call-b", json!({"limit":2})),
        ] {
            let start = tools
                .iter()
                .find(|frame| frame["content_block"]["id"] == id)
                .unwrap();
            assert_eq!(start["content_block"]["name"], "lookup");
            if id == "call-a" {
                assert_eq!(start["content_block"]["signature"], "opaque-signature");
            }
            let index = &start["index"];
            let args = events
                .iter()
                .filter(|frame| &frame["index"] == index)
                .filter_map(|frame| frame.pointer("/delta/partial_json").and_then(Value::as_str))
                .collect::<String>();
            assert_eq!(serde_json::from_str::<Value>(&args).unwrap(), expected);
        }
        let events = gemini_r6_stream(
            &stored,
            &[
                json!({"candidates":[{"content":{"parts":[{"functionCall":{"name":"tool","args":{}}}]},"finishReason":"STOP"}]}),
            ],
        );
        assert!(
            events
                .iter()
                .any(|frame| frame.pointer("/content_block/name") == Some(&json!("tool"))),
            "a real explicitly declared tool named tool is valid"
        );
    }
}

#[test]
fn gemini_r6_unknown_and_conflicting_tool_identities_fail_closed_on_both_adapters() {
    use crate::domain::providers::model::{AppKind, ProviderType};
    for product in [ProviderType::AntigravityOAuth, ProviderType::AgyOAuth] {
        let stored = gemini_r6_stored(product);
        let adapter = crate::proxy::adapters::adapter_for(AppKind::Claude, product);
        for call in [
            json!({"args":{}}),
            json!({"id":"unknown","args":{"a":1}}),
            json!({"name":" ","args":{}}),
            json!({"name":7,"args":{}}),
        ] {
            let value = json!({"response":{"candidates":[{"content":{"parts":[{"functionCall":call}]},"finishReason":"STOP"}]}});
            assert!(adapter
                .transform_response(
                    Bytes::from(value.to_string()),
                    &stored,
                    ProxyRoute::ClaudeMessages
                )
                .is_err());
            let mut transformer = StreamEventTransformer::new(
                &stored,
                ProxyRoute::ClaudeMessages,
                transforms::ResponsesToolContext::default(),
            );
            assert!(transformer
                .push(Bytes::from(format!("data: {value}\n\n")))
                .is_err());
            assert!(transformer.finish().is_err());
        }
        let mut transformer = StreamEventTransformer::new(
            &stored,
            ProxyRoute::ClaudeMessages,
            transforms::ResponsesToolContext::default(),
        );
        transformer.push(Bytes::from(format!("data: {}\n\n",json!({"response":{"candidates":[{"content":{"parts":[{"functionCall":{"id":"same-id","name":"lookup","args":{}}}]}}]}})))).unwrap();
        let conflict = json!({"response":{"candidates":[{"content":{"parts":[{"functionCall":{"id":"same-id","name":"other","args":{}}}]},"finishReason":"STOP"}]}});
        assert!(transformer
            .push(Bytes::from(format!("data: {conflict}\n\n")))
            .is_err());
        assert!(transformer.finish().is_err());
    }
}

#[test]
fn gemini_r6_concurrent_missing_id_responses_never_share_a_synthetic_identity() {
    use crate::domain::providers::model::{AppKind, ProviderType};
    let ids = std::thread::scope(|scope| {
        let tasks: Vec<_> = (0..8)
            .map(|_| {
                scope.spawn(|| {
                    let stored = gemini_r6_stored(ProviderType::AntigravityOAuth);
                    let adapter = crate::proxy::adapters::adapter_for(
                        AppKind::Claude,
                        ProviderType::AntigravityOAuth,
                    );
                    let body = adapter
                        .transform_response(
                            Bytes::from(json!({"response":gemini_r6_body()}).to_string()),
                            &stored,
                            ProxyRoute::ClaudeMessages,
                        )
                        .unwrap();
                    serde_json::from_slice::<Value>(&body).unwrap()["id"]
                        .as_str()
                        .unwrap()
                        .to_string()
                })
            })
            .collect();
        tasks
            .into_iter()
            .map(|task| task.join().unwrap())
            .collect::<std::collections::BTreeSet<_>>()
    });
    assert_eq!(ids.len(), 8);
}

#[test]
fn gemini_r6_optional_null_call_keeps_text_and_late_arguments_fail_closed() {
    use crate::domain::providers::model::{AppKind, ProviderType};
    for product in [ProviderType::AntigravityOAuth, ProviderType::AgyOAuth] {
        let stored = gemini_r6_stored(product);
        let adapter = crate::proxy::adapters::adapter_for(AppKind::Claude, product);
        let value = json!({"candidates":[{"content":{"parts":[{"text":"hello","functionCall":null}]},"finishReason":"STOP"}]});
        let raw = adapter
            .transform_response(
                Bytes::from(json!({"response":value}).to_string()),
                &stored,
                ProxyRoute::ClaudeMessages,
            )
            .unwrap();
        let response: Value = serde_json::from_slice(&raw).unwrap();
        assert_eq!(response["stop_reason"], "end_turn");
        assert_eq!(response["content"][0]["text"], "hello");
        let events = gemini_r6_stream(&stored, &[value]);
        assert!(!events
            .iter()
            .any(|frame| frame.pointer("/content_block/type") == Some(&json!("tool_use"))));
        let mut transformer = StreamEventTransformer::new(
            &stored,
            ProxyRoute::ClaudeMessages,
            transforms::ResponsesToolContext::default(),
        );
        let terminal = json!({"response":{"candidates":[{"content":{"parts":[{"functionCall":{"id":"call-a","name":"lookup","args":{}}}]},"finishReason":"STOP"}]}});
        transformer
            .push(Bytes::from(format!("data: {terminal}\n\n")))
            .unwrap();
        let late = json!({"response":{"candidates":[{"content":{"parts":[{"functionCall":{"args":{"late":true}}}]}}]}});
        assert!(transformer
            .push(Bytes::from(format!("data: {late}\n\n")))
            .is_err());
        assert!(transformer.finish().is_err());
    }
}
