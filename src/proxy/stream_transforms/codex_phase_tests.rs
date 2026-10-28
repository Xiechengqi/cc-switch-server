use super::super::adapters::ProviderAdapter;
use super::*;

fn payload(frame: &StreamFrame) -> Option<&Value> {
    match &frame.payload {
        transforms::StreamPayload::Json(value) => Some(value),
        transforms::StreamPayload::Done => None,
    }
}

fn text_blocks(frames: &[StreamFrame]) -> Vec<String> {
    let mut blocks = BTreeMap::<u64, String>::new();
    let mut open = BTreeSet::new();
    for value in frames.iter().filter_map(payload) {
        let index = value.get("index").and_then(Value::as_u64);
        match value["type"].as_str() {
            Some("content_block_start") if value["content_block"]["type"] == "text" => {
                let index = index.unwrap();
                assert!(open.insert(index));
                blocks.insert(index, String::new());
            }
            Some("content_block_delta") if value["delta"]["type"] == "text_delta" => {
                let index = index.unwrap();
                assert!(open.contains(&index), "text must belong to an open block");
                blocks
                    .get_mut(&index)
                    .unwrap()
                    .push_str(value["delta"]["text"].as_str().unwrap());
            }
            Some("content_block_stop") => {
                open.remove(&index.unwrap());
            }
            _ => {}
        }
    }
    assert!(open.is_empty());
    blocks.into_values().collect()
}

#[test]
fn codex_r4_phase_message_boundaries_match_json_without_reasoning_forgery() {
    for second_added in [false, true] {
        let commentary = json!({"id":"msg_c", "type":"message", "phase":"commentary", "role":"assistant",
            "content":[{"type":"output_text", "text":"Checking."}]});
        let final_answer = json!({"id":"msg_f", "type":"message", "phase":"final_answer", "role":"assistant",
            "content":[{"type":"output_text", "text":"Done."}]});
        let response =
            json!({"id":"resp_phase", "status":"completed", "output":[commentary, final_answer]});
        let mut state = ResponsesAnthropicState::default();
        let mut frames =
            state.transform(&json!({"type":"response.created", "response":{"id":"resp_phase"}}));
        frames.extend(state.transform(
            &json!({"type":"response.output_item.added", "output_index":0, "item":commentary}),
        ));
        frames.extend(state.transform(&json!({"type":"response.output_text.delta", "output_index":0, "item_id":"msg_c", "delta":"Checking."})));
        frames.extend(state.transform(
            &json!({"type":"response.output_item.done", "output_index":0, "item":commentary}),
        ));
        if second_added {
            frames.extend(state.transform(&json!({"type":"response.output_item.added", "output_index":1, "item":final_answer})));
        }
        frames.extend(state.transform(
            &json!({"type":"response.output_item.done", "output_index":1, "item":final_answer}),
        ));
        frames.extend(state.transform(&json!({"type":"response.completed", "response":response})));
        let json_response = transforms::openai_responses_response_to_anthropic(&response).unwrap();
        let expected = json_response["content"]
            .as_array()
            .unwrap()
            .iter()
            .map(|block| block["text"].as_str().unwrap().to_string())
            .collect::<Vec<_>>();
        assert_eq!(
            text_blocks(&frames),
            expected,
            "second_added={second_added}"
        );
        assert_eq!(expected, ["Checking.", "Done."]);
        assert!(!frames.iter().filter_map(payload).any(|value| {
            value.pointer("/content_block/type").and_then(Value::as_str) == Some("thinking")
                || value.pointer("/delta/type").and_then(Value::as_str) == Some("signature_delta")
        }));
        assert_eq!(
            frames
                .iter()
                .filter_map(payload)
                .filter(|value| value["type"] == "message_stop")
                .count(),
            1
        );
    }
}

#[test]
fn codex_r4_phase_bridge_retained_state_is_charged() {
    let id = "message".repeat(512);
    let mut state = ResponsesAnthropicState::default();
    state.transform(
        &json!({"type":"response.output_item.added", "output_index":0,
        "item":{"id":id, "type":"message", "phase":"commentary"}}),
    );
    state.transform(&json!({"type":"response.output_text.delta", "output_index":0, "item_id":id, "delta":"Checking."}));
    assert_eq!(
        state.message_phases.for_key("output:0"),
        Some(super::super::responses_phase::MessagePhase::Commentary)
    );
    let bridge = StreamBridgeState::ResponsesAnthropic(state);
    assert!(
        bridge.retained_bytes() >= id.len(),
        "retained message identity/phase must enter the request and process budgets"
    );
    let mut gemini = ToGeminiState::responses();
    gemini
        .transform(
            &json!({"type":"response.output_item.added", "output_index":0,
            "item":{"id":id, "type":"message", "phase":"commentary"}}),
        )
        .unwrap();
    assert!(gemini.retained_bytes() >= id.len());
}

#[test]
fn codex_r4_phase_sparse_done_and_conflicts_do_not_create_new_phase_labels() {
    use super::super::responses_phase::{MessagePhase, MessagePhases};
    let mut phases = MessagePhases::default();
    let keys = BTreeSet::from(["output:0".to_string(), "item:msg_c".to_string()]);
    phases.observe(
        &json!({"type":"message", "phase":"commentary"}),
        &keys,
        "chat",
        "response",
    );
    phases.observe(&json!({"type":"message"}), &keys, "chat", "response");
    assert_eq!(phases.for_key("item:msg_c"), Some(MessagePhase::Commentary));
    phases.observe(
        &json!({"type":"message", "phase":"final_answer"}),
        &keys,
        "chat",
        "response",
    );
    assert_eq!(phases.for_key("output:0"), Some(MessagePhase::Unrecognized));
    phases.observe(
        &json!({"type":"message", "phase":"private-future-phase".repeat(1000)}),
        &keys,
        "chat",
        "response",
    );
    assert_eq!(
        phases.for_key("item:msg_c"),
        Some(MessagePhase::Unrecognized)
    );
    assert!(
        phases.retained_bytes() < 1024,
        "unknown phase strings must not be retained or used as labels"
    );
}

#[test]
fn codex_r4_phase_commentary_tool_final_and_real_reasoning_stay_separate() {
    let reasoning = json!({"id":"rs_real", "type":"reasoning", "encrypted_content":"real-opaque-content",
        "summary":[{"type":"summary_text", "text":"Real thought."}]});
    let commentary = json!({"id":"msg_c", "type":"message", "role":"assistant", "phase":"commentary",
        "content":[{"type":"output_text", "text":"Checking."}]});
    let tool = json!({"id":"fc", "type":"function_call", "call_id":"call_r4", "name":"read", "arguments":"{\"path\":\"a\"}"});
    let final_answer = json!({"id":"msg_f", "type":"message", "role":"assistant", "phase":"final_answer",
        "content":[{"type":"output_text", "text":"Done."}]});
    let response = json!({"id":"resp_phase", "status":"completed", "output":[reasoning,commentary,tool,final_answer],
        "usage":{"input_tokens":2,"output_tokens":3}});
    let events = [
        json!({"type":"response.created", "response":{"id":"resp_phase"}}),
        json!({"type":"response.output_item.done", "output_index":0, "item":reasoning}),
        json!({"type":"response.output_item.added", "output_index":1, "item":commentary}),
        json!({"type":"response.output_text.delta", "output_index":1, "item_id":"msg_c", "delta":"Checking."}),
        json!({"type":"response.output_item.done", "output_index":1, "item":{"id":"msg_c","type":"message"}}),
        json!({"type":"response.output_item.done", "output_index":2, "item":tool}),
        json!({"type":"response.output_item.done", "output_index":3, "item":final_answer}),
        json!({"type":"response.completed", "response":response}),
    ];
    let mut messages = ResponsesAnthropicState::default();
    let mut chat = ResponsesChatState::default();
    let mut message_frames = Vec::new();
    let mut chat_frames = Vec::new();
    for event in &events {
        message_frames.extend(messages.transform(event));
        chat_frames.extend(chat.transform(event));
    }
    assert_eq!(text_blocks(&message_frames), ["Checking.", "Done."]);
    assert_eq!(
        messages.message_phases.for_key("output:1"),
        Some(super::super::responses_phase::MessagePhase::Commentary)
    );
    assert_eq!(
        chat.message_phases.for_key("output:3"),
        Some(super::super::responses_phase::MessagePhase::FinalAnswer)
    );
    assert_eq!(
        message_frames
            .iter()
            .filter_map(payload)
            .filter(
                |value| value.pointer("/content_block/type").and_then(Value::as_str)
                    == Some("thinking")
            )
            .count(),
        1
    );
    let json_chat = transforms::openai_responses_response_to_chat(&response).unwrap();
    let chat_text = chat_frames
        .iter()
        .filter_map(payload)
        .filter_map(|value| {
            value
                .pointer("/choices/0/delta/content")
                .and_then(Value::as_str)
        })
        .collect::<String>();
    assert_eq!(
        chat_text,
        json_chat["choices"][0]["message"]["content"]
            .as_str()
            .unwrap()
    );
    assert_eq!(chat_text, "Checking.Done.");
    assert_eq!(
        json_chat["choices"][0]["message"]["reasoning_content"],
        "Real thought."
    );
    let message_response = transforms::openai_responses_response_to_anthropic(&response).unwrap();
    let replay = transforms::anthropic_to_openai_responses(&json!({"model":"test", "max_tokens":1024,
        "messages":[{"role":"assistant", "content":message_response["content"]},{"role":"user","content":"next"}]})).unwrap();
    assert!(replay["input"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["type"] == "reasoning"
            && item["encrypted_content"] == "real-opaque-content"));
    assert_eq!(
        replay["input"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|item| item["type"] == "reasoning")
            .count(),
        1
    );
    let serialized = serde_json::to_string(&replay).unwrap();
    assert!(serialized.contains("Checking.") && serialized.contains("Done."));
    assert!(!serialized.contains("cpa:commentary:") && !serialized.contains("\"phase\""));
    let cross_provider =
        transforms::openai_responses_to_chat(&json!({"input":response["output"]})).unwrap();
    let cross_text = serde_json::to_string(&cross_provider["messages"]).unwrap();
    assert!(cross_text.contains("Checking.") && cross_text.contains("Done."));
    assert!(!cross_text.contains("cpa:commentary:"));
}

#[test]
fn codex_r4_phase_missing_and_unknown_still_emit_all_text_once() {
    for phase in [Value::Null, json!("future_phase")] {
        let mut state = ResponsesAnthropicState::default();
        let mut frames = Vec::new();
        for index in 0..2 {
            frames.extend(state.transform(&json!({"type":"response.output_item.done", "output_index":index,
                "item":{"type":"message","phase":phase,"content":[{"type":"output_text","text":format!("text-{index}")}]}})));
        }
        frames.extend(
            state.transform(
                &json!({"type":"response.completed", "response":{"status":"completed"}}),
            ),
        );
        assert_eq!(text_blocks(&frames), ["text-0", "text-1"]);
    }
}

#[test]
fn codex_r4_phase_native_responses_request_and_sse_preserve_original_fields() {
    use crate::domain::providers::model::{AppKind, Provider};
    let stored = StoredProvider {
        app: AppKind::Codex,
        provider: Provider {
            id: "codex-r4-phase-native".into(),
            name: "Codex OAuth".into(),
            settings_config: json!({"env":{"OPENAI_API_KEY":"fixture-token"}}),
            category: None,
            meta: None,
            extra: Default::default(),
        },
        provider_type: ProviderType::CodexOAuth,
        provider_type_id: "codex_oauth".into(),
        resource: Default::default(),
    };
    let items = json!([
        {"id":"msg_c","type":"message","role":"assistant","phase":"commentary","content":[{"type":"output_text","text":"Checking."}]},
        {"id":"msg_f","type":"message","role":"assistant","phase":"final_answer","content":[{"type":"output_text","text":"Done."}]}
    ]);
    let adapter = super::super::adapters::adapter_for(AppKind::Codex, ProviderType::CodexOAuth);
    let request = adapter
        .transform_request(
            Bytes::from(
                serde_json::to_vec(&json!({"model":"gpt-5.5","input":items,"stream":true}))
                    .unwrap(),
            ),
            &stored,
        )
        .unwrap();
    let request_value: Value = serde_json::from_slice(&request.body).unwrap();
    assert_eq!(request_value["input"], items);
    let mut transformer = StreamEventTransformer::new(
        &stored,
        ProxyRoute::CodexResponses,
        transforms::ResponsesToolContext::default(),
    );
    for (index, item) in items.as_array().unwrap().iter().enumerate() {
        let event = json!({"type":"response.output_item.done","output_index":index,"item":item});
        let wire = format!("event: response.output_item.done\ndata: {event}\n\n");
        let output = transformer.push(Bytes::from(wire)).unwrap();
        let decoded = std::str::from_utf8(&output)
            .unwrap()
            .lines()
            .find_map(|line| line.strip_prefix("data: "))
            .unwrap();
        assert_eq!(serde_json::from_str::<Value>(decoded).unwrap(), event);
    }
}
