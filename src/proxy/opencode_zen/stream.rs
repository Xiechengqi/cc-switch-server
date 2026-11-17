//! Bounded Zen SSE framing and strict upstream terminal validation. The common
//! transformer owns all downstream protocol encoding; this layer restores names
//! before conversion and withholds terminal output until clean transport EOF.
use super::*;
use crate::proxy::{stream_transforms::StreamEventTransformer, ProxyError, ProxyRoute};
use bytes::Bytes;

const EVENT_LIMIT: usize = 1024 * 1024;

pub(in crate::proxy) struct ZenStream {
    wire: ZenWire,
    pending: Vec<u8>,
    held: Vec<u8>,
    rename: BTreeMap<String, String>,
    transformer: StreamEventTransformer,
    responses: crate::proxy::response_semantics::ResponsesSseInspector,
    anthropic: crate::proxy::anthropic_semantics::AnthropicSseInspector,
    tool_names: BTreeMap<u64, String>,
    finish_reason: bool,
    done: bool,
    terminal_candidate: bool,
    pub(in crate::proxy) semantic: bool,
    pub(in crate::proxy) incomplete: bool,
    timing: Option<crate::proxy::request_timing::ForwardTiming>,
}

impl ZenStream {
    pub(in crate::proxy) fn new(
        stored: &crate::domain::providers::store::StoredProvider,
        route: ProxyRoute,
        wire: ZenWire,
        rename: BTreeMap<String, String>,
        context: crate::proxy::transforms::ResponsesToolContext,
        include_usage: bool,
        timing: Option<crate::proxy::request_timing::ForwardTiming>,
    ) -> Self {
        Self {
            wire,
            pending: Vec::new(),
            held: Vec::new(),
            rename,
            transformer: StreamEventTransformer::new_with_formats(
                stored,
                route,
                context,
                include_usage,
                Some(wire.format()),
            ),
            responses: Default::default(),
            anthropic: Default::default(),
            tool_names: BTreeMap::new(),
            finish_reason: false,
            done: false,
            terminal_candidate: false,
            semantic: false,
            incomplete: false,
            timing,
        }
    }

    pub(in crate::proxy) fn retained_bytes(&self) -> usize {
        self.tool_names
            .values()
            .map(|name| name.capacity().saturating_add(64))
            .sum::<usize>()
            .saturating_add(self.pending.capacity())
            .saturating_add(self.held.capacity())
            .saturating_add(self.transformer.retained_bytes())
            .saturating_add(self.responses.retained_transport_bytes())
            .saturating_add(self.anthropic.retained_bytes())
    }

    pub(in crate::proxy) fn working_bytes(&self, incoming: &[u8]) -> usize {
        // A final byte may complete a large event accumulated over many tiny
        // chunks. Reserve its parser workspace before allocating the JSON tree.
        crate::proxy::request_memory::json_parse_working_bytes(&self.pending)
            .saturating_add(crate::proxy::request_memory::json_parse_working_bytes(
                incoming,
            ))
            .saturating_mul(4)
    }

    pub(in crate::proxy) fn push(&mut self, chunk: &[u8]) -> Result<Bytes, ProxyError> {
        // Apply the cap to each event rather than to an arbitrary network chunk.
        let mut output = Vec::new();
        for part in chunk.split_inclusive(|byte| *byte == b'\n') {
            if self.pending.len().saturating_add(part.len()) > EVENT_LIMIT {
                return Err(ProxyError::bad_gateway(
                    "OpenCode Zen SSE event exceeds 1 MiB",
                ));
            }
            self.pending.extend_from_slice(part);
            while let Some((at, delimiter)) =
                crate::proxy::anthropic_semantics::next_event_boundary(&self.pending)
            {
                let raw: Vec<u8> = self.pending.drain(..at + delimiter).collect();
                let frame = self.event(&raw)?;
                if self.terminal_candidate {
                    if self.held.len().saturating_add(frame.len()) > EVENT_LIMIT {
                        return Err(ProxyError::bad_gateway(
                            "OpenCode Zen terminal tail exceeds 1 MiB",
                        ));
                    }
                    self.held.extend_from_slice(&frame);
                } else {
                    output.extend_from_slice(&frame);
                }
            }
        }
        Ok(Bytes::from(output))
    }

    fn event(&mut self, raw: &[u8]) -> Result<Bytes, ProxyError> {
        let text = std::str::from_utf8(raw)
            .map_err(|_| ProxyError::bad_gateway("OpenCode Zen SSE is not UTF-8"))?;
        let mut data = Vec::new();
        let mut event = None;
        for line in text.lines() {
            if let Some(value) = line.strip_prefix("data:") {
                data.push(value.strip_prefix(' ').unwrap_or(value));
            }
            if let Some(value) = line.strip_prefix("event:") {
                event = Some(value.trim());
            }
        }
        if data.is_empty() {
            return Ok(Bytes::new());
        }
        let data = data.join("\n");
        if self.wire != ZenWire::Chat && self.terminal_candidate {
            return Err(ProxyError::bad_gateway("OpenCode Zen data after terminal"));
        }
        if data == "[DONE]" {
            if self.wire != ZenWire::Chat || !self.finish_reason || self.done {
                return Err(ProxyError::bad_gateway("OpenCode Zen unexpected [DONE]"));
            }
            self.done = true;
            self.terminal_candidate = true;
            return self
                .transformer
                .push(Bytes::from_static(b"data: [DONE]\n\n"));
        }
        let mut value: Value = serde_json::from_str(&data)
            .map_err(|_| ProxyError::bad_gateway("OpenCode Zen malformed SSE JSON"))?;
        if !value.is_object()
            || value.get("error").is_some_and(|error| !error.is_null())
            || value.get("type").and_then(Value::as_str) == Some("error")
        {
            return Err(ProxyError::bad_gateway("OpenCode Zen upstream error event"));
        }
        match self.wire {
            ZenWire::Chat => {
                if self.done {
                    return Err(ProxyError::bad_gateway("OpenCode Zen data after [DONE]"));
                }
                let semantic = value.pointer("/choices/0/delta").is_some_and(|delta| {
                    ["content", "reasoning_content", "reasoning"]
                        .iter()
                        .any(|key| {
                            delta
                                .get(*key)
                                .and_then(Value::as_str)
                                .is_some_and(|s| !s.is_empty())
                        })
                        || delta
                            .get("tool_calls")
                            .and_then(Value::as_array)
                            .is_some_and(|calls| !calls.is_empty())
                });
                if self.finish_reason && semantic {
                    return Err(ProxyError::bad_gateway(
                        "OpenCode Zen content after finish_reason",
                    ));
                }
                self.semantic |= semantic;
                if let Some(reason) = value
                    .pointer("/choices/0/finish_reason")
                    .filter(|value| !value.is_null())
                {
                    let reason = reason
                        .as_str()
                        .filter(|s| {
                            matches!(
                                *s,
                                "stop"
                                    | "length"
                                    | "tool_calls"
                                    | "function_call"
                                    | "content_filter"
                            )
                        })
                        .ok_or_else(|| {
                            ProxyError::bad_gateway("OpenCode Zen invalid Chat finish_reason")
                        })?;
                    self.finish_reason = true;
                    self.terminal_candidate = true;
                    self.incomplete = matches!(reason, "length" | "content_filter");
                }
                if let Some(timing) = &self.timing {
                    timing.observe_chat_value(&value);
                }
            }
            ZenWire::Responses => {
                let observations = self.responses.push(raw).map_err(|_| {
                    ProxyError::bad_gateway("OpenCode Zen invalid Responses stream")
                })?;
                for observation in observations {
                    if matches!(
                        observation,
                        crate::proxy::response_semantics::SemanticObservation::Failure(_)
                            | crate::proxy::response_semantics::SemanticObservation::ErrorFrame(_)
                    ) {
                        return Err(ProxyError::bad_gateway(
                            "OpenCode Zen Responses generation failed",
                        ));
                    }
                    self.semantic |= observation.counts_as_business_output();
                    if let Some(timing) = &self.timing {
                        timing.observe(&observation);
                    }
                }
                self.terminal_candidate = self.responses.terminal().is_some();
                self.incomplete = matches!(
                    self.responses.terminal(),
                    Some(crate::proxy::response_semantics::SemanticTerminal::Incomplete)
                );
            }
            ZenWire::Messages => {
                let observations = self
                    .anthropic
                    .push(raw)
                    .map_err(|_| ProxyError::bad_gateway("OpenCode Zen invalid Messages stream"))?;
                self.semantic |= observations.iter().any(|o| o.counts_as_business_output());
                if matches!(
                    self.anthropic.terminal(),
                    Some(crate::proxy::anthropic_semantics::AnthropicTerminal::Error(
                        _
                    ))
                ) {
                    return Err(ProxyError::bad_gateway(
                        "OpenCode Zen Messages generation failed",
                    ));
                }
                if let Some(timing) = &self.timing {
                    timing.observe_anthropic_value(&value);
                }
                self.terminal_candidate = self.anthropic.terminal().is_some();
                self.incomplete |= value
                    .pointer("/delta/stop_reason")
                    .and_then(Value::as_str)
                    .is_some_and(|s| matches!(s, "max_tokens" | "pause_turn"));
            }
        }
        // Chat servers may split a function name across deltas. Buffer only
        // prefixes of injected aliases; never expose `read_client` to callers.
        if self.wire == ZenWire::Chat {
            if let Some(calls) = value
                .pointer_mut("/choices/0/delta/tool_calls")
                .and_then(Value::as_array_mut)
            {
                for call in calls {
                    let index = call.get("index").and_then(Value::as_u64).unwrap_or(0);
                    if let Some(function) = call.get_mut("function").and_then(Value::as_object_mut)
                    {
                        if let Some(fragment) = function
                            .get("name")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                        {
                            if self.tool_names.len() >= 128 && !self.tool_names.contains_key(&index)
                            {
                                return Err(ProxyError::bad_gateway(
                                    "OpenCode Zen too many tool calls",
                                ));
                            }
                            let name = self.tool_names.entry(index).or_default();
                            if name.len().saturating_add(fragment.len()) > 256 {
                                return Err(ProxyError::bad_gateway(
                                    "OpenCode Zen tool name exceeds limit",
                                ));
                            }
                            let was_held = !name.is_empty()
                                && !self.rename.contains_key(name.as_str())
                                && self
                                    .rename
                                    .keys()
                                    .any(|alias| alias.starts_with(name.as_str()));
                            if self.rename.contains_key(name.as_str()) && !fragment.is_empty() {
                                return Err(ProxyError::bad_gateway(
                                    "OpenCode Zen tool name continued after alias",
                                ));
                            }
                            name.push_str(&fragment);
                            if let Some(original) = self.rename.get(name.as_str()) {
                                function
                                    .insert("name".to_string(), Value::String(original.clone()));
                            } else if self
                                .rename
                                .keys()
                                .any(|alias| alias.starts_with(name.as_str()))
                            {
                                function.remove("name");
                            } else if was_held {
                                function.insert("name".to_string(), Value::String(name.clone()));
                            }
                        }
                    }
                }
            }
        }
        restore_tool_names(&mut value, &self.rename);
        let prefix = event
            .map(|event| format!("event: {event}\n"))
            .unwrap_or_default();
        self.transformer
            .push(Bytes::from(format!("{prefix}data: {value}\n\n")))
    }

    pub(in crate::proxy) fn finish(&mut self) -> Result<Bytes, ProxyError> {
        if self.pending.iter().any(|b| !b.is_ascii_whitespace()) {
            return Err(ProxyError::bad_gateway(
                "OpenCode Zen incomplete SSE event at EOF",
            ));
        }
        if self.tool_names.values().any(|name| {
            !self.rename.contains_key(name)
                && self.rename.keys().any(|alias| alias.starts_with(name))
        }) {
            return Err(ProxyError::bad_gateway("OpenCode Zen incomplete tool name"));
        }
        match self.wire {
            ZenWire::Chat if !self.done => {
                return Err(ProxyError::bad_gateway(
                    "OpenCode Zen stream ended before [DONE]",
                ))
            }
            ZenWire::Responses => {
                self.responses.finish().map_err(|_| {
                    ProxyError::bad_gateway("OpenCode Zen missing Responses terminal")
                })?;
            }
            ZenWire::Messages => {
                self.anthropic.finish().map_err(|_| {
                    ProxyError::bad_gateway("OpenCode Zen missing Messages terminal")
                })?;
            }
            _ => {}
        }
        let tail = self.transformer.finish()?;
        let mut output = std::mem::take(&mut self.held);
        output.extend_from_slice(&tail);
        Ok(Bytes::from(output))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn stored() -> crate::domain::providers::store::StoredProvider {
        use crate::domain::providers::{model::*, store::*};
        StoredProvider {
            app: AppKind::Codex,
            provider_type: ProviderType::OpenCodeZen,
            provider_type_id: "opencode_zen".into(),
            resource: Default::default(),
            provider: Provider {
                id: "zen-test".into(),
                name: "Zen".into(),
                settings_config: json!({}),
                category: None,
                meta: None,
                extra: Default::default(),
            },
        }
    }
    fn frame(value: Value) -> String {
        format!("data: {value}\n\n")
    }
    fn document(wire: ZenWire) -> Value {
        match wire {
            ZenWire::Chat => {
                json!({"id":"chat-test","object":"chat.completion","model":"test-free","choices":[{"index":0,"message":{"role":"assistant","content":"hello世界"},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":2,"total_tokens":12}})
            }
            ZenWire::Responses => {
                json!({"id":"resp_test","object":"response","model":"test-free","status":"completed","output":[{"id":"msg_test","type":"message","role":"assistant","status":"completed","content":[{"type":"output_text","text":"hello世界","annotations":[]}]}],"usage":{"input_tokens":10,"output_tokens":2,"total_tokens":12}})
            }
            ZenWire::Messages => {
                json!({"id":"msg_test","type":"message","model":"test-free","role":"assistant","content":[{"type":"text","text":"hello世界"}],"stop_reason":"end_turn","usage":{"input_tokens":10,"output_tokens":2}})
            }
        }
    }
    fn upstream(wire: ZenWire) -> String {
        match wire {
            ZenWire::Chat => [frame(json!({"id":"chat-test","model":"test-free","choices":[{"index":0,"delta":{"role":"assistant","content":"hello世界"},"finish_reason":null}]})), frame(json!({"id":"chat-test","choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":2,"total_tokens":12}})), "data: [DONE]\n\n".into()].concat(),
            ZenWire::Responses => [frame(json!({"type":"response.created","response":{"id":"resp_test","model":"test-free","status":"in_progress","output":[]}})), frame(json!({"type":"response.output_text.delta","item_id":"msg_test","output_index":0,"content_index":0,"delta":"hello世界"})), frame(json!({"type":"response.completed","response":document(wire)}))].concat(),
            ZenWire::Messages => [frame(json!({"type":"message_start","message":{"id":"msg_test","model":"test-free","type":"message","role":"assistant","content":[],"stop_reason":null,"usage":{"input_tokens":10,"output_tokens":0}}})), frame(json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}})), frame(json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hello世界"}})), frame(json!({"type":"content_block_stop","index":0})), frame(json!({"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null},"usage":{"output_tokens":2}})), frame(json!({"type":"message_stop"}))].concat(),
        }
    }
    fn decoder(wire: ZenWire, route: ProxyRoute) -> ZenStream {
        ZenStream::new(
            &stored(),
            route,
            wire,
            BTreeMap::new(),
            Default::default(),
            true,
            None,
        )
    }
    #[test]
    fn zen_all_eighteen_protocol_and_mode_combinations() {
        for wire in [ZenWire::Chat, ZenWire::Responses, ZenWire::Messages] {
            for route in [
                ProxyRoute::ClaudeMessages,
                ProxyRoute::CodexChatCompletions,
                ProxyRoute::CodexResponses,
            ] {
                let json =
                    convert_response(&document(wire), wire, route, &Default::default()).unwrap();
                match route {
                    ProxyRoute::ClaudeMessages => assert!(json["content"].is_array()),
                    ProxyRoute::CodexChatCompletions => assert!(json["choices"].is_array()),
                    _ => assert!(json["output"].is_array()),
                }
                let mut decoder = decoder(wire, route);
                let mut output = Vec::new();
                // Every byte boundary, including UTF-8 and JSON escapes.
                for byte in upstream(wire).as_bytes() {
                    output.extend_from_slice(&decoder.push(&[*byte]).unwrap());
                }
                assert!(
                    String::from_utf8_lossy(&output).contains("hello世界"),
                    "{wire:?} {route:?}"
                );
                output.extend_from_slice(&decoder.finish().unwrap());
                let text = String::from_utf8(output).unwrap();
                match route {
                    ProxyRoute::ClaudeMessages => assert!(text.contains("message_stop")),
                    ProxyRoute::CodexChatCompletions => assert!(text.contains("[DONE]")),
                    _ => assert!(text.contains("response.completed")),
                }
            }
        }
    }
    #[test]
    fn zen_missing_terminal_malformed_utf8_and_post_terminal_data_fail() {
        for wire in [ZenWire::Chat, ZenWire::Responses, ZenWire::Messages] {
            let mut d = decoder(wire, ProxyRoute::CodexChatCompletions);
            assert!(d.finish().is_err());
            let mut d = decoder(wire, ProxyRoute::CodexChatCompletions);
            assert!(d.push(b"data: {bad}\n\n").is_err());
            let mut d = decoder(wire, ProxyRoute::CodexChatCompletions);
            assert!(d.push(b"data: \xff\n\n").is_err());
            let mut d = decoder(wire, ProxyRoute::CodexChatCompletions);
            d.push(upstream(wire).as_bytes()).unwrap();
            assert!(d.push(frame(json!({"type":"response.output_text.delta","choices":[{"delta":{"content":"late"}}]})).as_bytes()).is_err());
        }
    }
    #[test]
    fn zen_terminal_is_held_until_clean_eof_and_events_are_bounded() {
        let mut d = decoder(ZenWire::Chat, ProxyRoute::CodexChatCompletions);
        let output = d.push(upstream(ZenWire::Chat).as_bytes()).unwrap();
        assert!(!String::from_utf8_lossy(&output).contains("[DONE]"));
        assert!(String::from_utf8_lossy(&d.finish().unwrap()).contains("[DONE]"));
        let mut d = decoder(ZenWire::Chat, ProxyRoute::CodexChatCompletions);
        assert!(d.push(&vec![b'x'; EVENT_LIMIT + 1]).is_err());
    }
    #[test]
    fn zen_failure_events_never_become_success() {
        for wire in [ZenWire::Chat, ZenWire::Responses, ZenWire::Messages] {
            let mut d = decoder(wire, ProxyRoute::CodexResponses);
            assert!(d
                .push(
                    frame(json!({"type":"error","error":{"message":"private upstream body"}}))
                        .as_bytes()
                )
                .is_err());
        }
    }
    #[test]
    fn zen_request_conversion_preserves_tool_round_trips_and_cache_schema_fields() {
        for (model, wire) in [
            ("chat-free", ZenWire::Chat),
            ("muse-spark-free", ZenWire::Responses),
            ("union-alpha", ZenWire::Messages),
        ] {
            let body = json!({"model":model,"stream":true,"system":[{"type":"text","text":"rules","cache_control":{"type":"ephemeral"}}],"messages":[{"role":"assistant","content":[{"type":"tool_use","id":"call_1","name":"read","input":{"path":"x"}}]},{"role":"user","content":[{"type":"tool_result","tool_use_id":"call_1","content":"ok"}]}],"tools":[{"name":"read","description":"client read","input_schema":{"type":"object","properties":{"cache_control":{"type":"string"}}}}]});
            let prepared = prepare_for_route(&body, ProxyRoute::ClaudeMessages).unwrap();
            let raw = String::from_utf8(prepared.body).unwrap();
            assert!(raw.contains("call_1"));
            assert!(raw.contains("read_client"));
            assert!(raw.contains("cache_control")); // schema property survives
            assert!(!raw.contains("ephemeral"));
            assert_eq!(wire_for(model), wire);
        }
        assert!(prepare_for_route(
            &json!({"model":"chat-free","messages":[],"tool_choice":"none"}),
            ProxyRoute::CodexChatCompletions
        )
        .is_err());
    }
}
