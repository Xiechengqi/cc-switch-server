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
    client_tools: Option<std::collections::BTreeSet<String>>,
    finish_reason: Option<String>,
    chat_usage_tail: bool,
    done: bool,
    terminal_candidate: bool,
    pub(in crate::proxy) semantic: bool,
    pub(in crate::proxy) incomplete: bool,
    timing: Option<crate::proxy::request_timing::ForwardTiming>,
    aggregation: Option<super::aggregate::ZenAggregation>,
    aggregate_input_bytes: usize,
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
            responses: crate::proxy::response_semantics::ResponsesSseInspector::with_repeat_guard_and_visibility(
                true, crate::proxy::response_semantics::ResponsesEventVisibility::StandardClient),
            anthropic: Default::default(),
            tool_names: BTreeMap::new(),
            client_tools: None,
            finish_reason: None,
            chat_usage_tail: false,
            done: false,
            terminal_candidate: false,
            semantic: false,
            incomplete: false,
            timing,
            aggregation: None,
            aggregate_input_bytes: 0,
        }
    }

    pub(in crate::proxy) fn with_client_tools(
        mut self,
        names: std::collections::BTreeSet<String>,
    ) -> Self {
        self.client_tools = Some(names);
        self
    }

    pub(in crate::proxy) fn retained_bytes(&self) -> usize {
        self.tool_names
            .values()
            .map(|name| name.capacity().saturating_add(64))
            .sum::<usize>()
            .saturating_add(self.client_tools.as_ref().map_or(0, |names| {
                names.iter().map(|n| n.capacity().saturating_add(64)).sum()
            }))
            .saturating_add(self.pending.capacity())
            .saturating_add(self.held.capacity())
            .saturating_add(self.finish_reason.as_ref().map_or(0, String::capacity))
            .saturating_add(self.transformer.retained_bytes())
            .saturating_add(self.responses.retained_transport_bytes())
            .saturating_add(self.anthropic.retained_bytes())
            .saturating_add(self.aggregation.as_ref().map_or(0, |a| a.retained_bytes()))
    }

    pub(in crate::proxy) fn with_aggregation(mut self) -> Self {
        self.aggregation = Some(super::aggregate::ZenAggregation::new(self.wire));
        self
    }

    pub(in crate::proxy) fn finish_document(mut self) -> Result<Value, ProxyError> {
        self.finish()?;
        self.aggregation
            .take()
            .ok_or_else(|| ProxyError::bad_gateway("OpenCode Zen missing aggregation"))?
            .finish()
    }

    pub(in crate::proxy) fn working_bytes(&self, incoming: &[u8]) -> usize {
        // A final byte may complete a large event accumulated over many tiny
        // chunks. Reserve its parser workspace before allocating the JSON tree.
        crate::proxy::request_memory::json_parse_working_bytes(&self.pending)
            .saturating_add(crate::proxy::request_memory::json_parse_working_bytes(
                incoming,
            ))
            .saturating_mul(4)
            .saturating_add(self.aggregation.as_ref().map_or(0, |a| a.working_bytes()))
            .saturating_add(if self.aggregation.is_some() {
                // An encoded tool-argument string may become a dense JSON
                // collection when a later block_stop completes this chunk.
                self.pending
                    .len()
                    .saturating_add(incoming.len())
                    .saturating_mul(128)
            } else {
                0
            })
    }

    pub(in crate::proxy) fn push(&mut self, chunk: &[u8]) -> Result<Bytes, ProxyError> {
        if self.aggregation.is_some() {
            self.aggregate_input_bytes = self.aggregate_input_bytes.saturating_add(chunk.len());
            if self.aggregate_input_bytes > 8 * 1024 * 1024 {
                return Err(ProxyError::bad_gateway(
                    "OpenCode Zen aggregate exceeds 8 MiB",
                ));
            }
        }
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
            if self.wire != ZenWire::Chat || self.finish_reason.is_none() || self.done {
                return Err(ProxyError::bad_gateway("OpenCode Zen unexpected [DONE]"));
            }
            self.done = true;
            self.terminal_candidate = true;
            if self.aggregation.is_some() {
                return Ok(Bytes::new());
            }
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
        super::validation::stream_usage(&value)?;
        match self.wire {
            ZenWire::Chat => {
                if value.pointer("/choices/0/delta/function_call").is_some() {
                    return Err(ProxyError::bad_gateway(
                        "OpenCode Zen returned an unsupported legacy function delta",
                    ));
                }
                if self.done {
                    return Err(ProxyError::bad_gateway("OpenCode Zen data after [DONE]"));
                }
                let semantic = value.pointer("/choices/0/delta").is_some_and(|delta| {
                    ["content", "reasoning_content", "reasoning", "refusal"]
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
                if self.finish_reason.is_some() && semantic {
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
                    if let Some(previous) = &self.finish_reason {
                        let empty_delta = value
                            .pointer("/choices/0/delta")
                            .and_then(Value::as_object)
                            .is_some_and(|delta| {
                                delta.iter().all(|(key, value)| match key.as_str() {
                                    "role" => value == "assistant",
                                    "content" => value.is_null() || value.as_str() == Some(""),
                                    _ => false,
                                })
                            });
                        let usage_tail = value["choices"]
                            .as_array()
                            .is_some_and(|choices| choices.len() == 1)
                            && value.pointer("/choices/0/index").and_then(Value::as_u64) == Some(0)
                            && ["prompt_tokens", "completion_tokens"]
                                .iter()
                                .all(|key| value["usage"][*key].as_u64().is_some());
                        if previous != reason || self.chat_usage_tail || !empty_delta || !usage_tail
                        {
                            return Err(ProxyError::bad_gateway(
                                "OpenCode Zen duplicate Chat finish_reason",
                            ));
                        }
                        self.chat_usage_tail = true;
                        value["choices"] = serde_json::json!([]);
                    } else {
                        self.finish_reason = Some(reason.to_string());
                        self.terminal_candidate = true;
                        self.incomplete = matches!(reason, "length" | "content_filter");
                    }
                }
                if let Some(timing) = &self.timing {
                    timing.observe_chat_value(&value);
                }
            }
            ZenWire::Responses => {
                let batch = self.responses.push_normalized(raw).map_err(|_| {
                    ProxyError::bad_gateway("OpenCode Zen invalid Responses stream")
                })?;
                if batch.normalized.is_empty() {
                    return Ok(Bytes::new());
                }
                for observation in batch.observations {
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
                if self.terminal_candidate {
                    if let Some(response) = value.get("response") {
                        validate_json(response, ZenWire::Responses)?;
                    }
                }
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
        if let Some(names) = &self.client_tools {
            validate_client_tools(&value, names)?;
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
                            if self.client_tools.as_ref().is_some_and(|names| {
                                !names
                                    .iter()
                                    .any(|allowed| allowed.starts_with(name.as_str()))
                            }) {
                                return Err(ProxyError::bad_gateway(
                                    "OpenCode Zen called a tool unavailable to the client",
                                ));
                            }
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
        if let Some(aggregation) = &mut self.aggregation {
            aggregation.observe(&value, event)?;
            return Ok(Bytes::new());
        }
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
        if self
            .client_tools
            .as_ref()
            .is_some_and(|names| self.tool_names.values().any(|name| !names.contains(name)))
        {
            return Err(ProxyError::bad_gateway(
                "OpenCode Zen incomplete or unavailable client tool name",
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
        if self.aggregation.is_some() {
            return Ok(Bytes::new());
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
    fn zen_nonstream_aggregation_preserves_all_protocols_and_requires_clean_eof() {
        for wire in [ZenWire::Chat, ZenWire::Responses, ZenWire::Messages] {
            let bytes = upstream(wire);
            let mut aggregate = decoder(wire, ProxyRoute::CodexChatCompletions).with_aggregation();
            for byte in bytes.as_bytes() {
                assert!(aggregate.push(&[*byte]).unwrap().is_empty());
            }
            let result = aggregate.finish_document().unwrap();
            validate_json(&result, wire).unwrap();
            assert!(result.to_string().contains("hello世界"));
            assert_eq!(result["usage"], document(wire)["usage"]);
            let mut truncated = decoder(wire, ProxyRoute::CodexChatCompletions).with_aggregation();
            truncated
                .push(&bytes.as_bytes()[..bytes.len() - 10])
                .unwrap();
            assert!(truncated.finish_document().is_err(), "{wire:?}");
            let mut trailing = decoder(wire, ProxyRoute::CodexChatCompletions).with_aggregation();
            trailing.push(bytes.as_bytes()).unwrap();
            assert!(trailing.push(b"data: {\"type\":\"error\"}\n\n").is_err());
        }
    }

    #[test]
    fn zen_nonstream_aggregation_preserves_split_tool_arguments_and_reasoning() {
        let mut chat = decoder(ZenWire::Chat, ProxyRoute::CodexChatCompletions).with_aggregation();
        for value in [
            json!({"id":"chat_test","model":"test-free","choices":[{"index":0,"delta":{"reasoning_content":"think","tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"rea","arguments":"{\"path\":"}}]},"finish_reason":null}]}),
            json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"name":"d","arguments":"\"a\"}"}}]},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":2,"completion_tokens":1}}),
        ] {
            chat.push(frame(value).as_bytes()).unwrap();
        }
        chat.push(b"data: [DONE]\n\n").unwrap();
        let doc = chat.finish_document().unwrap();
        validate_json(&doc, ZenWire::Chat).unwrap();
        assert_eq!(doc["choices"][0]["message"]["reasoning_content"], "think");
        assert_eq!(
            doc["choices"][0]["message"]["tool_calls"][0]["function"]["name"],
            "read"
        );
        assert_eq!(
            doc["choices"][0]["message"]["tool_calls"][0]["function"]["arguments"],
            "{\"path\":\"a\"}"
        );
        let mut messages =
            decoder(ZenWire::Messages, ProxyRoute::ClaudeMessages).with_aggregation();
        for value in [
            json!({"type":"message_start","message":{"id":"msg_test","model":"test-free","type":"message","role":"assistant","content":[],"stop_reason":null,"usage":{"input_tokens":2,"output_tokens":0}}}),
            json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"call_1","name":"read","input":{}}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"path\":"}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"\"a\"}"}}),
            json!({"type":"content_block_stop","index":0}),
            json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":1}}),
            json!({"type":"message_stop"}),
        ] {
            messages.push(frame(value).as_bytes()).unwrap();
        }
        let doc = messages.finish_document().unwrap();
        validate_json(&doc, ZenWire::Messages).unwrap();
        assert_eq!(doc["content"][0]["input"], json!({"path":"a"}));
    }

    #[test]
    fn zen_nonstream_responses_accepts_sse_event_name_without_payload_type() {
        let mut aggregate =
            decoder(ZenWire::Responses, ProxyRoute::CodexResponses).with_aggregation();
        let frame = format!(
            "event: response.completed\ndata: {}\n\n",
            serde_json::json!({"response":document(ZenWire::Responses)})
        );
        aggregate.push(frame.as_bytes()).unwrap();
        assert_eq!(
            aggregate.finish_document().unwrap(),
            document(ZenWire::Responses)
        );
    }

    #[test]
    fn zen_hy4_usage_tail_is_normalized_once_in_all_routes_and_modes() {
        let first = frame(
            serde_json::json!({"id":"chat_tail","model":"test-free","choices":[{"index":0,"delta":{"content":"hello"},"finish_reason":null}]}),
        );
        let terminal = frame(
            serde_json::json!({"choices":[{"index":0,"delta":{"role":"assistant","content":""},"finish_reason":"stop"}]}),
        );
        let tail = serde_json::json!({"choices":[{"index":0,"delta":{"role":"assistant","content":""},"finish_reason":"stop"}],"usage":{"prompt_tokens":2,"completion_tokens":1,"total_tokens":3}});
        for route in [
            ProxyRoute::ClaudeMessages,
            ProxyRoute::CodexChatCompletions,
            ProxyRoute::CodexResponses,
        ] {
            for aggregate in [false, true] {
                let mut decoder = decoder(ZenWire::Chat, route);
                if aggregate {
                    decoder = decoder.with_aggregation();
                }
                let mut output = Vec::new();
                for byte in [
                    first.clone(),
                    terminal.clone(),
                    frame(tail.clone()),
                    "data: [DONE]\n\n".into(),
                ]
                .concat()
                .as_bytes()
                {
                    output.extend_from_slice(&decoder.push(&[*byte]).unwrap());
                }
                if aggregate {
                    let doc = decoder.finish_document().unwrap();
                    validate_json(&doc, ZenWire::Chat).unwrap();
                    assert_eq!(doc["usage"]["prompt_tokens"], 2);
                    assert_eq!(doc["choices"][0]["message"]["content"], "hello");
                } else {
                    output.extend_from_slice(&decoder.finish().unwrap());
                    let output = String::from_utf8(output).unwrap();
                    assert!(output.contains("hello"));
                    let terminal = match route {
                        ProxyRoute::ClaudeMessages => "\"type\":\"message_stop\"",
                        ProxyRoute::CodexResponses => "\"type\":\"response.completed\"",
                        _ => "\"finish_reason\":\"stop\"",
                    };
                    assert_eq!(output.matches(terminal).count(), 1, "{route:?}: {output}");
                }
            }
        }
        for variant in [0, 1, 2, 3] {
            let mut decoder =
                decoder(ZenWire::Chat, ProxyRoute::CodexChatCompletions).with_aggregation();
            decoder.push(first.as_bytes()).unwrap();
            decoder.push(terminal.as_bytes()).unwrap();
            let mut invalid = tail.clone();
            match variant {
                0 => invalid["choices"][0]["finish_reason"] = serde_json::json!("length"),
                1 => invalid["choices"][0]["delta"]["content"] = serde_json::json!("extra"),
                2 => {
                    invalid.as_object_mut().unwrap().remove("usage");
                }
                _ => {
                    decoder.push(frame(tail.clone()).as_bytes()).unwrap();
                }
            }
            assert!(
                decoder.push(frame(invalid).as_bytes()).is_err(),
                "variant {variant}"
            );
        }
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
            let prepared = prepare_for_route(
                &body,
                ProxyRoute::ClaudeMessages,
                CatalogEndpoint::legacy(model),
            )
            .unwrap();
            let raw = String::from_utf8(prepared.body).unwrap();
            assert!(raw.contains("call_1"));
            assert!(raw.contains("read_client"));
            assert!(raw.contains("cache_control")); // schema property survives
            assert!(!raw.contains("ephemeral"));
            assert_eq!(prepared.wire, wire);
        }
        assert!(prepare_for_route(
            &json!({"model":"chat-free","messages":[],"tool_choice":"none"}),
            ProxyRoute::CodexChatCompletions,
            CatalogEndpoint::LegacyChat
        )
        .is_err());
    }
    #[test]
    fn zen_chat_usage_tail_reaches_messages_before_terminal() {
        let mut decoder = decoder(ZenWire::Chat, ProxyRoute::ClaudeMessages);
        let content = frame(
            json!({"id":"c","model":"test-free","choices":[{"index":0,"delta":{"role":"assistant","content":"hello"},"finish_reason":null}]}),
        );
        let finish = frame(json!({"choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}));
        let usage = frame(
            json!({"choices":[],"usage":{"prompt_tokens":31,"completion_tokens":17,"total_tokens":48}}),
        );
        let mut output = decoder.push(content.as_bytes()).unwrap().to_vec();
        output.extend_from_slice(&decoder.push(finish.as_bytes()).unwrap());
        assert!(!String::from_utf8_lossy(&output).contains("message_stop"));
        output.extend_from_slice(&decoder.push(usage.as_bytes()).unwrap());
        output.extend_from_slice(&decoder.push(b"data: [DONE]\n\n").unwrap());
        output.extend_from_slice(&decoder.finish().unwrap());
        let text = String::from_utf8(output).unwrap();
        assert!(text.contains("\"output_tokens\":17"), "{text}");
        assert_eq!(text.matches("event: message_stop").count(), 1);
    }

    #[test]
    fn zen_responses_filters_private_events_and_guards_repeated_output() {
        let mut d = decoder(ZenWire::Responses, ProxyRoute::CodexResponses);
        for event in [
            "responsesapi.private",
            "codex.rate_limits",
            "unreviewed.future",
        ] {
            assert!(d
                .push(frame(json!({"type":event,"secret":"do-not-expose"})).as_bytes())
                .unwrap()
                .is_empty());
        }
        let repeated = frame(
            json!({"type":"response.output_text.delta","output_index":0,"content_index":0,"delta":"repeated"}),
        );
        let mut failed = false;
        for _ in 0..300 {
            if d.push(repeated.as_bytes()).is_err() {
                failed = true;
                break;
            }
        }
        assert!(failed);
    }

    #[test]
    fn zen_placeholder_tools_fail_in_json_and_all_stream_protocols() {
        let names = std::collections::BTreeSet::from(["lookup".to_string()]);
        for (wire, event) in [
            (
                ZenWire::Chat,
                json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"shell","arguments":"{}"}}]}}]}),
            ),
            (
                ZenWire::Responses,
                json!({"type":"response.output_item.added","output_index":0,"item":{"type":"function_call","id":"fc","call_id":"c","name":"shell","arguments":""}}),
            ),
            (
                ZenWire::Messages,
                json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"c","name":"shell","input":{}}}),
            ),
        ] {
            let mut d =
                decoder(wire, ProxyRoute::CodexChatCompletions).with_client_tools(names.clone());
            if wire == ZenWire::Messages {
                d.push(frame(json!({"type":"message_start","message":{"id":"m","type":"message","role":"assistant","model":"test-free","content":[],"usage":{"input_tokens":1,"output_tokens":0}}})).as_bytes()).unwrap();
            }
            assert!(d.push(frame(event).as_bytes()).is_err(), "{wire:?}");
        }
        assert!(validate_client_tools(
            &json!({"choices":[{"message":{"tool_calls":[{"function":{"name":"shell"}}]}}]}),
            &names
        )
        .is_err());
    }

    #[test]
    fn zen_native_messages_alias_history_and_opaque_arguments_are_preserved() {
        let request = json!({"model":"union-alpha","messages":[{"role":"assistant","content":[{"type":"tool_use","id":"call_1","name":"read","input":{"name":"read_client","type":"tool_use"}}]},{"role":"user","content":[{"type":"tool_result","tool_use_id":"call_1","content":"ok"}]}],"tools":[{"name":"read","description":"caller","input_schema":{"type":"object"}}]});
        let prepared = prepare_for_route(
            &request,
            ProxyRoute::ClaudeMessages,
            CatalogEndpoint::LegacyMessages,
        )
        .unwrap();
        let sent: Value = serde_json::from_slice(&prepared.body).unwrap();
        assert_eq!(sent["messages"][0]["content"][0]["name"], "read_client");
        assert_eq!(
            sent["messages"][0]["content"][0]["input"]["name"],
            "read_client"
        );
        let input = json!({"type":"tool_use","name":"read_client","function":{"name":"read_client"},"content":[{"type":"tool_use","name":"read_client"}]});
        let mut response =
            json!({"content":[{"type":"tool_use","name":"read_client","input":input}]});
        restore_tool_names(&mut response, &prepared.rename);
        assert_eq!(response["content"][0]["name"], "read");
        assert_eq!(response["content"][0]["input"], input);
    }
    #[test]
    fn zen_media_validation_keeps_tool_arguments_opaque() {
        let tool = json!({"model":"union-alpha","messages":[{"role":"assistant","content":[{"type":"tool_use","id":"c","name":"lookup","input":{"type":"input_image","input":{"content":"opaque"}}}]}],"tools":[{"name":"lookup","input_schema":{"type":"object"}}]});
        assert!(validate_source_request(&tool).is_ok());
        assert!(validate_source_request(&json!({"messages":[{"role":"user","content":[{"type":"image_url","image_url":{"url":"data:image/png;base64,eA=="}}]}]})).is_err());
    }
}
