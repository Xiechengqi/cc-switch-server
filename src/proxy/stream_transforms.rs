use std::collections::{BTreeMap, BTreeSet};

use bytes::Bytes;
use serde_json::{json, Value};

use crate::domain::providers::store::StoredProvider;

use super::adapters::{
    downstream_format_for_route, encode_stream_frames, transform_stream_value,
    upstream_format_for_route, UpstreamFormat,
};
use super::reasoning_bridge::{
    anthropic_block_from_openai_reasoning_item, responses_reasoning_item_from_anthropic_block,
    unsigned_responses_reasoning_item,
};
use super::transforms;
use super::transforms::StreamFrame;
use super::{ProxyError, ProxyRoute};

const MAX_STREAM_TRANSFORM_SSE_EVENT_BYTES: usize = 128 * 1024 * 1024;

#[derive(Debug)]
pub(super) struct StreamEventTransformer {
    upstream: Option<UpstreamFormat>,
    downstream: UpstreamFormat,
    buffer: Vec<u8>,
    responses_tool_context: transforms::ResponsesToolContext,
    bridge: Option<StreamBridgeState>,
}

impl StreamEventTransformer {
    pub(super) fn new<T>(
        stored: &StoredProvider,
        route: ProxyRoute,
        responses_tool_context: T,
    ) -> Self
    where
        T: Into<transforms::ResponsesToolContext>,
    {
        let responses_tool_context = responses_tool_context.into();
        let upstream = upstream_format_for_route(stored, Some(route), &[]);
        let downstream = downstream_format_for_route(route);
        let bridge = match (upstream, downstream) {
            (Some(UpstreamFormat::OpenAiResponses), UpstreamFormat::AnthropicMessages) => Some(
                StreamBridgeState::ResponsesAnthropic(ResponsesAnthropicState::default()),
            ),
            (Some(UpstreamFormat::OpenAiChat), UpstreamFormat::AnthropicMessages) => Some(
                StreamBridgeState::ChatAnthropic(ChatAnthropicState::default()),
            ),
            (Some(UpstreamFormat::OpenAiResponses), UpstreamFormat::OpenAiChat) => Some(
                StreamBridgeState::ResponsesChat(ResponsesChatState::default()),
            ),
            (Some(UpstreamFormat::OpenAiChat), UpstreamFormat::OpenAiResponses) => {
                Some(StreamBridgeState::ChatResponses(Box::new(
                    ChatResponsesState::new(responses_tool_context.clone()),
                )))
            }
            (Some(UpstreamFormat::AnthropicMessages), UpstreamFormat::OpenAiResponses) => {
                Some(StreamBridgeState::AnthropicResponses(
                    AnthropicResponsesState::new(responses_tool_context.clone()),
                ))
            }
            _ => None,
        };
        Self {
            upstream,
            downstream,
            buffer: Vec::new(),
            responses_tool_context,
            bridge,
        }
    }

    pub(super) fn push(&mut self, chunk: Bytes) -> Result<Bytes, ProxyError> {
        let Some(upstream) = self.upstream else {
            return Ok(chunk);
        };
        if upstream == self.downstream {
            return Ok(chunk);
        }
        self.buffer.extend_from_slice(&chunk);
        self.drain_complete_events(false)
    }

    pub(super) fn finish(&mut self) -> Result<Bytes, ProxyError> {
        let Some(upstream) = self.upstream else {
            return Ok(Bytes::new());
        };
        if upstream == self.downstream {
            return Ok(Bytes::new());
        }
        let mut output = self.drain_complete_events(true)?.to_vec();
        if let Some(bridge) = self.bridge.as_mut() {
            output.extend_from_slice(&encode_stream_frames(&bridge.finish_eof()?).into_bytes());
        }
        Ok(Bytes::from(output))
    }

    fn drain_complete_events(&mut self, finish: bool) -> Result<Bytes, ProxyError> {
        self.drain_complete_events_with_limit(finish, MAX_STREAM_TRANSFORM_SSE_EVENT_BYTES)
    }

    fn drain_complete_events_with_limit(
        &mut self,
        finish: bool,
        max_event_bytes: usize,
    ) -> Result<Bytes, ProxyError> {
        let mut output = String::new();
        while let Some((event_end, delimiter_len)) = next_event_boundary(&self.buffer) {
            if event_end > max_event_bytes {
                return Err(stream_transform_sse_event_too_large());
            }
            let event = self.buffer[..event_end].to_vec();
            self.buffer.drain(..event_end + delimiter_len);
            output.push_str(&self.transform_event(&event)?);
        }
        while let Some(line_end) = self.buffer.iter().position(|byte| *byte == b'\n') {
            let line = self.buffer[..line_end]
                .strip_suffix(b"\r")
                .unwrap_or(&self.buffer[..line_end]);
            if line.len() > max_event_bytes {
                return Err(stream_transform_sse_event_too_large());
            }
            if !standalone_line_is_ready(line) {
                break;
            }
            let event = line.to_vec();
            self.buffer.drain(..line_end + 1);
            output.push_str(&self.transform_event(&event)?);
        }
        let pending_event_bytes = if finish {
            self.buffer.len()
        } else {
            self.buffer
                .len()
                .saturating_sub(sse_delimiter_prefix_len(&self.buffer))
        };
        if pending_event_bytes > max_event_bytes {
            return Err(stream_transform_sse_event_too_large());
        }
        if finish && !self.buffer.is_empty() {
            let event = std::mem::take(&mut self.buffer);
            output.push_str(&self.transform_event(&event)?);
        }
        Ok(Bytes::from(output))
    }

    fn transform_event(&mut self, event: &[u8]) -> Result<String, ProxyError> {
        let text = std::str::from_utf8(event).map_err(|error| {
            crate::metrics::record_stream_transform_protocol_error("invalid_utf8");
            ProxyError::bad_gateway(format!("upstream SSE event is not UTF-8: {error}"))
        })?;
        let Some(payload) = sse_data_payload(text) else {
            return Ok(String::new());
        };
        if payload == "[DONE]" {
            let frames = self.bridge.as_mut().map(StreamBridgeState::upstream_done);
            return Ok(match frames {
                Some(frames) => encode_stream_frames(&frames),
                None if self.downstream == UpstreamFormat::AnthropicMessages => String::new(),
                None => encode_stream_frames(&[StreamFrame::done()]),
            });
        }
        let value = serde_json::from_str::<Value>(&payload).map_err(|error| {
            crate::metrics::record_stream_transform_protocol_error("invalid_json");
            ProxyError::bad_gateway(format!("upstream SSE data is not valid JSON: {error}"))
        })?;
        let frames = match self.bridge.as_mut() {
            Some(bridge) => bridge.transform(&value),
            None => transform_stream_value(
                self.upstream.expect("upstream format is present"),
                self.downstream,
                &value,
                &self.responses_tool_context,
            ),
        };
        Ok(encode_stream_frames(&frames))
    }
}

fn stream_transform_sse_event_too_large() -> ProxyError {
    crate::metrics::record_stream_transform_protocol_error("event_too_large");
    ProxyError {
        status: axum::http::StatusCode::PAYLOAD_TOO_LARGE,
        message: "upstream SSE event exceeded 128 MiB".to_string(),
    }
}

fn sse_delimiter_prefix_len(buffer: &[u8]) -> usize {
    [b"\r\n\r".as_slice(), b"\r\n", b"\r", b"\n"]
        .into_iter()
        .find(|prefix| buffer.ends_with(prefix))
        .map_or(0, <[u8]>::len)
}

fn next_event_boundary(buffer: &[u8]) -> Option<(usize, usize)> {
    for index in 0..buffer.len() {
        if buffer.get(index..index + 2) == Some(b"\n\n") {
            return Some((index, 2));
        }
        if buffer.get(index..index + 4) == Some(b"\r\n\r\n") {
            return Some((index, 4));
        }
    }
    None
}

fn sse_data_payload(event: &str) -> Option<String> {
    let lines = event
        .lines()
        .filter_map(|line| line.trim_end_matches('\r').strip_prefix("data:"))
        .map(str::trim_start)
        .collect::<Vec<_>>();
    if !lines.is_empty() {
        return Some(lines.join("\n"));
    }
    let event = event.trim();
    event.starts_with('{').then(|| event.to_string())
}

fn standalone_line_is_ready(line: &[u8]) -> bool {
    let Ok(line) = std::str::from_utf8(line) else {
        return false;
    };
    let line = line.trim();
    if line.is_empty() || line.starts_with("event:") || line.starts_with(':') {
        return true;
    }
    let payload = line.strip_prefix("data:").map(str::trim).unwrap_or(line);
    payload == "[DONE]" || serde_json::from_str::<Value>(payload).is_ok()
}

#[derive(Debug)]
enum StreamBridgeState {
    ResponsesAnthropic(ResponsesAnthropicState),
    ChatAnthropic(ChatAnthropicState),
    ResponsesChat(ResponsesChatState),
    ChatResponses(Box<ChatResponsesState>),
    AnthropicResponses(AnthropicResponsesState),
}

impl StreamBridgeState {
    fn transform(&mut self, input: &Value) -> Vec<StreamFrame> {
        match self {
            Self::ResponsesAnthropic(state) => state.transform(input),
            Self::ChatAnthropic(state) => state.transform(input),
            Self::ResponsesChat(state) => state.transform(input),
            Self::ChatResponses(state) => state.transform(input),
            Self::AnthropicResponses(state) => state.transform(input),
        }
    }

    fn upstream_done(&mut self) -> Vec<StreamFrame> {
        match self {
            Self::ChatResponses(state) => state.finish_stream(),
            Self::ResponsesChat(state) if state.completed => Vec::new(),
            Self::AnthropicResponses(state) if state.completed => Vec::new(),
            Self::ResponsesAnthropic(state) if state.completed => Vec::new(),
            Self::ChatAnthropic(state) if state.completed => Vec::new(),
            _ => {
                protocol_error("done_before_terminal");
                Vec::new()
            }
        }
    }

    fn finish_eof(&mut self) -> Result<Vec<StreamFrame>, ProxyError> {
        if let Self::ChatResponses(state) = self {
            let frames = state.finish_stream();
            if state.completed {
                return Ok(frames);
            }
        }
        if self.completed() {
            return Ok(Vec::new());
        }
        crate::metrics::record_stream_transform_protocol_error("unexpected_eof");
        Err(ProxyError::bad_gateway(
            "upstream stream ended before a terminal event",
        ))
    }

    fn completed(&self) -> bool {
        match self {
            Self::ResponsesAnthropic(state) => state.completed,
            Self::ChatAnthropic(state) => state.completed,
            Self::ResponsesChat(state) => state.completed,
            Self::ChatResponses(state) => state.completed,
            Self::AnthropicResponses(state) => state.completed,
        }
    }
}

#[derive(Debug, Default)]
struct ResponsesAnthropicState {
    next_block_index: u64,
    message_started: bool,
    text_block: Option<BlockState>,
    reasoning_block: Option<BlockState>,
    tools: BTreeMap<i64, ToolBlockState>,
    item_ids: BTreeMap<i64, String>,
    emitted_text_items: BTreeSet<String>,
    completed_reasoning_items: BTreeSet<String>,
    completed_tool_items: BTreeSet<String>,
    pending_tool_arguments: BTreeMap<i64, String>,
    pending_custom_inputs: BTreeMap<i64, CustomToolInputState>,
    saw_tool: bool,
    saw_reasoning: bool,
    completed: bool,
}

#[derive(Debug, Clone, Copy)]
struct BlockState {
    index: u64,
    open: bool,
}

#[derive(Debug)]
struct ToolBlockState {
    block: BlockState,
    argument_delta_seen: bool,
    custom: bool,
    custom_input: CustomToolInputState,
}

#[derive(Debug, Default)]
struct CustomToolInputState {
    delta: String,
    done: Option<Value>,
}

impl ResponsesAnthropicState {
    fn transform(&mut self, input: &Value) -> Vec<StreamFrame> {
        if self.completed {
            return Vec::new();
        }
        match input.get("type").and_then(Value::as_str) {
            Some("response.created") => self.ensure_message_start(input),
            Some("response.output_text.delta") => self.text_delta(input),
            Some(
                "response.reasoning_summary_text.delta"
                | "response.reasoning_text.delta"
                | "response.reasoning.delta",
            ) => self.reasoning_delta(input),
            Some("response.output_item.added") => self.output_item_added(input),
            Some("response.function_call_arguments.delta") => self.argument_delta(input),
            Some("response.function_call_arguments.done") => self.argument_done(input),
            Some("response.custom_tool_call_input.delta") => self.custom_input_delta(input),
            Some("response.custom_tool_call_input.done") => self.custom_input_done(input),
            Some("response.output_item.done") => self.output_item_done(input),
            Some("response.completed") => self.complete(input),
            Some("response.incomplete") => self.complete(input),
            Some("response.failed") | Some("error") => self.fail(input),
            _ => Vec::new(),
        }
    }

    fn ensure_message_start(&mut self, input: &Value) -> Vec<StreamFrame> {
        if self.message_started {
            return Vec::new();
        }
        self.message_started = true;
        vec![StreamFrame::event(
            "message_start",
            json!({
                "type": "message_start",
                "message": {
                    "id": input.pointer("/response/id").and_then(Value::as_str).unwrap_or("resp"),
                    "type": "message",
                    "role": "assistant",
                    "model": input.pointer("/response/model").and_then(Value::as_str).unwrap_or_default(),
                    "content": [],
                    "stop_reason": Value::Null,
                    "stop_sequence": Value::Null,
                    "usage": {"input_tokens": 0, "output_tokens": 0}
                }
            }),
        )]
    }

    fn ensure_text_block(&mut self) -> Vec<StreamFrame> {
        if self.text_block.is_some_and(|block| block.open) {
            return Vec::new();
        }
        let index = self.allocate_index();
        self.text_block = Some(BlockState { index, open: true });
        vec![StreamFrame::event(
            "content_block_start",
            json!({
                "type": "content_block_start",
                "index": index,
                "content_block": {"type": "text", "text": ""}
            }),
        )]
    }

    fn text_delta(&mut self, input: &Value) -> Vec<StreamFrame> {
        let Some(text) = input.get("delta").and_then(Value::as_str) else {
            return Vec::new();
        };
        mark_response_item_emitted(&mut self.emitted_text_items, input, None, &self.item_ids);
        let mut frames = self.ensure_message_start(input);
        frames.extend(self.close_reasoning_block(None));
        frames.extend(self.ensure_text_block());
        let index = self.text_block.expect("text block was opened").index;
        frames.push(StreamFrame::event(
            "content_block_delta",
            json!({
                "type": "content_block_delta",
                "index": index,
                "delta": {"type": "text_delta", "text": text}
            }),
        ));
        frames
    }

    fn reasoning_delta(&mut self, input: &Value) -> Vec<StreamFrame> {
        let Some(text) = input.get("delta").and_then(Value::as_str) else {
            return Vec::new();
        };
        let mut frames = self.ensure_message_start(input);
        if !self.reasoning_block.is_some_and(|block| block.open) {
            if let Some(block) = self.text_block.as_mut().filter(|block| block.open) {
                block.open = false;
                frames.push(content_block_stop(block.index));
            }
            let index = self.allocate_index();
            self.reasoning_block = Some(BlockState { index, open: true });
            frames.push(StreamFrame::event(
                "content_block_start",
                json!({
                    "type": "content_block_start",
                    "index": index,
                    "content_block": {"type": "thinking", "thinking": "", "signature": ""}
                }),
            ));
        }
        self.saw_reasoning = true;
        let index = self.reasoning_block.expect("reasoning block exists").index;
        frames.push(StreamFrame::event(
            "content_block_delta",
            json!({
                "type": "content_block_delta",
                "index": index,
                "delta": {"type": "thinking_delta", "thinking": text}
            }),
        ));
        frames
    }

    fn output_item_added(&mut self, input: &Value) -> Vec<StreamFrame> {
        let Some(item) = input.get("item") else {
            return Vec::new();
        };
        self.remember_item_identity(input, item);
        if item.get("type").and_then(Value::as_str) == Some("reasoning") {
            return self.ensure_message_start(input);
        }
        if !matches!(
            item.get("type").and_then(Value::as_str),
            Some("function_call" | "custom_tool_call" | "tool_search_call")
        ) {
            return Vec::new();
        }
        let Some(output_index) = response_output_index(input) else {
            protocol_error("missing_output_index");
            return Vec::new();
        };
        let output_index = self.resolve_tool_index(output_index, item);
        let mut frames = self.ensure_message_start(input);
        frames.extend(self.close_reasoning_block(None));
        frames.extend(self.close_text_block());
        frames.extend(self.open_tool(output_index, item));
        frames
    }

    fn open_tool(&mut self, output_index: i64, item: &Value) -> Vec<StreamFrame> {
        if self.tools.contains_key(&output_index) {
            return Vec::new();
        }
        let index = self.allocate_index();
        self.saw_tool = true;
        let custom = item.get("type").and_then(Value::as_str) == Some("custom_tool_call");
        let pending_arguments = self
            .pending_tool_arguments
            .remove(&output_index)
            .unwrap_or_default();
        let pending_custom_input = self
            .pending_custom_inputs
            .remove(&output_index)
            .unwrap_or_default();
        self.tools.insert(
            output_index,
            ToolBlockState {
                block: BlockState { index, open: true },
                argument_delta_seen: !custom && !pending_arguments.is_empty(),
                custom,
                custom_input: pending_custom_input,
            },
        );
        let mut frames = vec![StreamFrame::event(
            "content_block_start",
            json!({
                "type": "content_block_start",
                "index": index,
                "content_block": {
                    "type": "tool_use",
                    "id": item.get("call_id").or_else(|| item.get("id")).and_then(Value::as_str).unwrap_or("tool"),
                    "name": item.get("name").and_then(Value::as_str).unwrap_or("tool"),
                    "input": {}
                }
            }),
        )];
        if !custom && !pending_arguments.is_empty() {
            frames.push(input_json_delta(index, &pending_arguments));
        }
        if custom {
            frames.extend(self.emit_custom_input(output_index, false));
        }
        frames
    }

    fn argument_delta(&mut self, input: &Value) -> Vec<StreamFrame> {
        let Some(output_index) = input.get("output_index").and_then(Value::as_i64) else {
            protocol_error("missing_output_index");
            return Vec::new();
        };
        let Some(arguments) = input.get("delta").and_then(Value::as_str) else {
            return Vec::new();
        };
        let Some(tool) = self.tools.get_mut(&output_index) else {
            self.pending_tool_arguments
                .entry(output_index)
                .or_default()
                .push_str(arguments);
            return Vec::new();
        };
        if !tool.block.open {
            protocol_error("closed_tool_index");
            return Vec::new();
        }
        tool.argument_delta_seen = true;
        vec![input_json_delta(tool.block.index, arguments)]
    }

    fn argument_done(&mut self, input: &Value) -> Vec<StreamFrame> {
        let Some(output_index) = input.get("output_index").and_then(Value::as_i64) else {
            protocol_error("missing_output_index");
            return Vec::new();
        };
        let Some(tool) = self.tools.get_mut(&output_index) else {
            if let Some(arguments) = input
                .get("arguments")
                .and_then(Value::as_str)
                .filter(|arguments| !arguments.is_empty())
            {
                self.pending_tool_arguments
                    .insert(output_index, arguments.to_string());
            }
            return Vec::new();
        };
        if !tool.block.open {
            protocol_error("closed_tool_index");
            return Vec::new();
        }
        if tool.argument_delta_seen {
            return Vec::new();
        }
        let frames = input
            .get("arguments")
            .and_then(Value::as_str)
            .filter(|arguments| !arguments.is_empty())
            .map(|arguments| vec![input_json_delta(tool.block.index, arguments)])
            .unwrap_or_default();
        if !frames.is_empty() {
            tool.argument_delta_seen = true;
        }
        frames
    }

    fn custom_input_delta(&mut self, input: &Value) -> Vec<StreamFrame> {
        let Some(output_index) = response_output_index(input) else {
            protocol_error("missing_output_index");
            return Vec::new();
        };
        let Some(delta) = input.get("delta").and_then(Value::as_str) else {
            return Vec::new();
        };
        if let Some(tool) = self.tools.get_mut(&output_index) {
            if !tool.custom || !tool.block.open || tool.argument_delta_seen {
                return Vec::new();
            }
            tool.custom_input.delta.push_str(delta);
        } else {
            self.pending_custom_inputs
                .entry(output_index)
                .or_default()
                .delta
                .push_str(delta);
        }
        Vec::new()
    }

    fn custom_input_done(&mut self, input: &Value) -> Vec<StreamFrame> {
        let Some(output_index) = response_output_index(input) else {
            protocol_error("missing_output_index");
            return Vec::new();
        };
        let done = input.get("input");
        if let Some(tool) = self.tools.get_mut(&output_index) {
            if !tool.custom || !tool.block.open || tool.argument_delta_seen {
                return Vec::new();
            }
            tool.custom_input.done =
                Some(preferred_custom_tool_input(done, &tool.custom_input.delta));
            return self.emit_custom_input(output_index, true);
        }
        let pending = self.pending_custom_inputs.entry(output_index).or_default();
        pending.done = Some(preferred_custom_tool_input(done, &pending.delta));
        Vec::new()
    }

    fn emit_custom_input(&mut self, output_index: i64, force: bool) -> Vec<StreamFrame> {
        let Some(tool) = self.tools.get_mut(&output_index) else {
            return Vec::new();
        };
        if !tool.custom || !tool.block.open || tool.argument_delta_seen {
            return Vec::new();
        }
        let input = tool
            .custom_input
            .done
            .clone()
            .or_else(|| force.then(|| Value::String(tool.custom_input.delta.clone())));
        let Some(input) = input else {
            return Vec::new();
        };
        let arguments = custom_tool_arguments(input);
        tool.argument_delta_seen = true;
        vec![input_json_delta(tool.block.index, &arguments)]
    }

    fn output_item_done(&mut self, input: &Value) -> Vec<StreamFrame> {
        let Some(item) = input.get("item") else {
            return Vec::new();
        };
        self.remember_item_identity(input, item);
        if item.get("type").and_then(Value::as_str) == Some("reasoning") {
            if response_item_was_emitted(
                &self.completed_reasoning_items,
                input,
                Some(item),
                &self.item_ids,
            ) {
                return Vec::new();
            }
            let mut frames = self.ensure_message_start(input);
            frames.extend(self.close_text_block());
            frames.extend(self.close_reasoning_block(Some(item)));
            mark_response_item_emitted(
                &mut self.completed_reasoning_items,
                input,
                Some(item),
                &self.item_ids,
            );
            return frames;
        }
        if item.get("type").and_then(Value::as_str) == Some("message") {
            if response_item_was_emitted(
                &self.emitted_text_items,
                input,
                Some(item),
                &self.item_ids,
            ) {
                return Vec::new();
            }
            let mut frames = Vec::new();
            if let Some(text) = response_message_text(item) {
                let mut packed = input.clone();
                packed["delta"] = Value::String(text);
                frames.extend(self.text_delta(&packed));
            }
            mark_response_item_emitted(
                &mut self.emitted_text_items,
                input,
                Some(item),
                &self.item_ids,
            );
            return frames;
        }
        if !matches!(
            item.get("type").and_then(Value::as_str),
            Some("function_call" | "custom_tool_call" | "tool_search_call")
        ) {
            return Vec::new();
        }
        if response_item_was_emitted(
            &self.completed_tool_items,
            input,
            Some(item),
            &self.item_ids,
        ) {
            return Vec::new();
        }
        let Some(output_index) = response_output_index(input) else {
            protocol_error("missing_output_index");
            return Vec::new();
        };
        let output_index = self.resolve_tool_index(output_index, item);
        let mut frames = self.ensure_message_start(input);
        frames.extend(self.close_reasoning_block(None));
        frames.extend(self.close_text_block());
        frames.extend(self.open_tool(output_index, item));
        if item.get("type").and_then(Value::as_str) == Some("custom_tool_call") {
            if let Some(tool) = self.tools.get_mut(&output_index) {
                if tool.custom_input.done.is_none() {
                    tool.custom_input.done = Some(preferred_custom_tool_input(
                        item.get("input"),
                        &tool.custom_input.delta,
                    ));
                }
            }
            frames.extend(self.emit_custom_input(output_index, true));
        }
        let Some(tool) = self.tools.get_mut(&output_index) else {
            return frames;
        };
        if !tool.block.open {
            return frames;
        }
        if !tool.custom && !tool.argument_delta_seen {
            if let Some(arguments) = item
                .get("arguments")
                .and_then(Value::as_str)
                .filter(|arguments| !arguments.is_empty())
            {
                frames.push(input_json_delta(tool.block.index, arguments));
            }
        }
        tool.block.open = false;
        frames.push(content_block_stop(tool.block.index));
        mark_response_item_emitted(
            &mut self.completed_tool_items,
            input,
            Some(item),
            &self.item_ids,
        );
        mark_response_item_emitted(
            &mut self.completed_tool_items,
            &json!({"output_index": output_index}),
            Some(item),
            &self.item_ids,
        );
        frames
    }

    fn complete(&mut self, input: &Value) -> Vec<StreamFrame> {
        let response = input.get("response").unwrap_or(input);
        let mut frames = self.ensure_message_start(input);
        if let Some(output) = response.get("output").and_then(Value::as_array) {
            for (output_index, item) in output.iter().enumerate() {
                frames.extend(self.output_item_done(&json!({
                    "output_index": output_index,
                    "item": item
                })));
            }
        }
        frames.extend(self.close_reasoning_block(None));
        if self.text_block.is_none() && self.tools.is_empty() && !self.saw_reasoning {
            frames.extend(self.ensure_text_block());
        }
        frames.extend(self.close_open_blocks());
        let stop_reason =
            transforms::openai_response_to_anthropic_stop_with_tools(response, self.saw_tool);
        let usage = response.get("usage").cloned().unwrap_or_else(|| json!({}));
        frames.push(StreamFrame::event(
            "message_delta",
            json!({
                "type": "message_delta",
                "delta": {"stop_reason": stop_reason, "stop_sequence": Value::Null},
                "usage": {
                    "input_tokens": usage.get("input_tokens").and_then(Value::as_u64).unwrap_or(0),
                    "output_tokens": usage.get("output_tokens").and_then(Value::as_u64).unwrap_or(0)
                }
            }),
        ));
        frames.push(StreamFrame::event(
            "message_stop",
            json!({"type": "message_stop"}),
        ));
        self.completed = true;
        frames
    }

    fn remember_item_identity(&mut self, input: &Value, item: &Value) {
        let (Some(output_index), Some(item_id)) = (
            response_output_index(input),
            item.get("id")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty()),
        ) else {
            return;
        };
        self.item_ids.insert(output_index, item_id.to_string());
    }

    fn resolve_tool_index(&self, output_index: i64, item: &Value) -> i64 {
        let Some(item_id) = item
            .get("id")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        else {
            return output_index;
        };
        self.item_ids
            .iter()
            .find_map(|(known_index, known_id)| {
                (known_id == item_id && self.tools.contains_key(known_index))
                    .then_some(*known_index)
            })
            .unwrap_or(output_index)
    }

    fn fail(&mut self, input: &Value) -> Vec<StreamFrame> {
        let mut frames = self.close_reasoning_block(None);
        frames.extend(self.close_open_blocks());
        let message = input
            .pointer("/response/error/message")
            .or_else(|| input.pointer("/error/message"))
            .and_then(Value::as_str)
            .unwrap_or("upstream response stream failed");
        frames.push(StreamFrame::event(
            "error",
            json!({"type": "error", "error": {"type": "upstream_error", "message": message}}),
        ));
        frames.push(StreamFrame::event(
            "message_stop",
            json!({"type": "message_stop"}),
        ));
        self.completed = true;
        frames
    }

    fn close_open_blocks(&mut self) -> Vec<StreamFrame> {
        let mut frames = Vec::new();
        frames.extend(self.close_text_block());
        for tool in self.tools.values_mut().filter(|tool| tool.block.open) {
            tool.block.open = false;
            frames.push(content_block_stop(tool.block.index));
        }
        frames
    }

    fn close_text_block(&mut self) -> Vec<StreamFrame> {
        let Some(block) = self.text_block.as_mut().filter(|block| block.open) else {
            return Vec::new();
        };
        block.open = false;
        vec![content_block_stop(block.index)]
    }

    fn close_reasoning_block(&mut self, item: Option<&Value>) -> Vec<StreamFrame> {
        let bridge_block = item.and_then(anthropic_block_from_openai_reasoning_item);
        let Some(mut block) = self.reasoning_block.take() else {
            if let Some(block) = bridge_block {
                self.saw_reasoning = true;
                let index = self.allocate_index();
                return vec![
                    StreamFrame::event(
                        "content_block_start",
                        json!({"type": "content_block_start", "index": index, "content_block": block}),
                    ),
                    content_block_stop(index),
                ];
            }
            return Vec::new();
        };
        if !block.open {
            return Vec::new();
        }
        block.open = false;
        let mut frames = Vec::new();
        if let Some(signature) = bridge_block
            .as_ref()
            .and_then(|value| value.get("signature"))
            .and_then(Value::as_str)
        {
            frames.push(StreamFrame::event(
                "content_block_delta",
                json!({
                    "type": "content_block_delta",
                    "index": block.index,
                    "delta": {"type": "signature_delta", "signature": signature}
                }),
            ));
        }
        frames.push(content_block_stop(block.index));
        frames
    }

    fn allocate_index(&mut self) -> u64 {
        let index = self.next_block_index;
        self.next_block_index = self.next_block_index.saturating_add(1);
        index
    }
}

#[derive(Debug, Default)]
struct ChatAnthropicState {
    next_block_index: u64,
    message_started: bool,
    text_block: Option<BlockState>,
    reasoning_block: Option<BlockState>,
    tools: BTreeMap<i64, ToolBlockState>,
    saw_tool: bool,
    completed: bool,
}

impl ChatAnthropicState {
    fn transform(&mut self, input: &Value) -> Vec<StreamFrame> {
        if self.completed {
            return Vec::new();
        }
        let Some(choice) = input.pointer("/choices/0") else {
            return Vec::new();
        };
        let mut frames = self.ensure_message_start(input);
        let delta = transforms::openai_chat_choice_payload(choice);
        if let Some(reasoning) = chat_reasoning_delta(delta) {
            frames.extend(self.reasoning_delta(reasoning));
        }
        for text in transforms::openai_chat_visible_text_fragments(delta) {
            frames.extend(self.text_delta(text));
        }
        let tool_calls = delta.get("tool_calls").and_then(Value::as_array);
        if let Some(tool_calls) = tool_calls {
            for tool_call in tool_calls {
                let Some(tool_index) = tool_call.get("index").and_then(Value::as_i64) else {
                    protocol_error("missing_tool_index");
                    continue;
                };
                if !self.tools.contains_key(&tool_index)
                    && (tool_call.get("id").is_some()
                        || tool_call.pointer("/function/name").is_some())
                {
                    frames.extend(self.close_text_block());
                    frames.extend(self.close_reasoning_block());
                    let index = self.allocate_index();
                    self.saw_tool = true;
                    self.tools.insert(
                        tool_index,
                        ToolBlockState {
                            block: BlockState { index, open: true },
                            argument_delta_seen: false,
                            custom: false,
                            custom_input: CustomToolInputState::default(),
                        },
                    );
                    frames.push(StreamFrame::event(
                        "content_block_start",
                        json!({
                            "type": "content_block_start",
                            "index": index,
                            "content_block": {
                                "type": "tool_use",
                                "id": tool_call.get("id").and_then(Value::as_str).unwrap_or("tool"),
                                "name": tool_call.pointer("/function/name").and_then(Value::as_str).unwrap_or("tool"),
                                "input": {}
                            }
                        }),
                    ));
                }
                if let Some(arguments) = tool_call
                    .pointer("/function/arguments")
                    .and_then(Value::as_str)
                    .filter(|arguments| !arguments.is_empty())
                {
                    if let Some(tool) = self
                        .tools
                        .get_mut(&tool_index)
                        .filter(|tool| tool.block.open)
                    {
                        tool.argument_delta_seen = true;
                        frames.push(input_json_delta(tool.block.index, arguments));
                    } else {
                        protocol_error("unknown_tool_index");
                    }
                }
            }
        }
        if tool_calls.is_none_or(|tool_calls| tool_calls.is_empty()) {
            if let Some(tool_call) = transforms::openai_chat_legacy_tool_delta(delta) {
                let tool_index = 0;
                if !self.tools.contains_key(&tool_index) {
                    frames.extend(self.close_text_block());
                    frames.extend(self.close_reasoning_block());
                    let index = self.allocate_index();
                    self.saw_tool = true;
                    self.tools.insert(
                        tool_index,
                        ToolBlockState {
                            block: BlockState { index, open: true },
                            argument_delta_seen: false,
                            custom: false,
                            custom_input: CustomToolInputState::default(),
                        },
                    );
                    frames.push(StreamFrame::event(
                        "content_block_start",
                        json!({
                            "type": "content_block_start",
                            "index": index,
                            "content_block": {
                                "type": "tool_use",
                                "id": tool_call.get("id").and_then(Value::as_str).unwrap_or("call_0"),
                                "name": tool_call.pointer("/function/name").and_then(Value::as_str).unwrap_or("tool"),
                                "input": {}
                            }
                        }),
                    ));
                }
                if let Some(arguments) = tool_call
                    .pointer("/function/arguments")
                    .and_then(Value::as_str)
                    .filter(|arguments| !arguments.is_empty())
                {
                    if let Some(tool) = self
                        .tools
                        .get_mut(&tool_index)
                        .filter(|tool| tool.block.open)
                    {
                        tool.argument_delta_seen = true;
                        frames.push(input_json_delta(tool.block.index, arguments));
                    }
                }
            }
        }
        if choice
            .get("finish_reason")
            .is_some_and(|value| !value.is_null())
        {
            frames.extend(self.finish(input));
        }
        frames
    }

    fn text_delta(&mut self, text: &str) -> Vec<StreamFrame> {
        let mut frames = self.close_reasoning_block();
        if !self.text_block.is_some_and(|block| block.open) {
            let index = self.allocate_index();
            self.text_block = Some(BlockState { index, open: true });
            frames.push(StreamFrame::event(
                "content_block_start",
                json!({"type": "content_block_start", "index": index, "content_block": {"type": "text", "text": ""}}),
            ));
        }
        let index = self.text_block.expect("text block exists").index;
        frames.push(StreamFrame::event(
            "content_block_delta",
            json!({"type": "content_block_delta", "index": index, "delta": {"type": "text_delta", "text": text}}),
        ));
        frames
    }

    fn reasoning_delta(&mut self, reasoning: &str) -> Vec<StreamFrame> {
        let mut frames = self.close_text_block();
        if !self.reasoning_block.is_some_and(|block| block.open) {
            let index = self.allocate_index();
            self.reasoning_block = Some(BlockState { index, open: true });
            frames.push(StreamFrame::event(
                "content_block_start",
                json!({"type": "content_block_start", "index": index, "content_block": {"type": "thinking", "thinking": ""}}),
            ));
        }
        let index = self.reasoning_block.expect("reasoning block exists").index;
        frames.push(StreamFrame::event(
            "content_block_delta",
            json!({"type": "content_block_delta", "index": index, "delta": {"type": "thinking_delta", "thinking": reasoning}}),
        ));
        frames
    }

    fn close_text_block(&mut self) -> Vec<StreamFrame> {
        let Some(block) = self.text_block.as_mut().filter(|block| block.open) else {
            return Vec::new();
        };
        block.open = false;
        vec![content_block_stop(block.index)]
    }

    fn close_reasoning_block(&mut self) -> Vec<StreamFrame> {
        let Some(block) = self.reasoning_block.as_mut().filter(|block| block.open) else {
            return Vec::new();
        };
        block.open = false;
        vec![content_block_stop(block.index)]
    }

    fn ensure_message_start(&mut self, input: &Value) -> Vec<StreamFrame> {
        if self.message_started {
            return Vec::new();
        }
        self.message_started = true;
        vec![StreamFrame::event(
            "message_start",
            json!({
                "type": "message_start",
                "message": {
                    "id": input.get("id").and_then(Value::as_str).unwrap_or("chatcmpl"),
                    "type": "message", "role": "assistant",
                    "model": input.get("model").and_then(Value::as_str).unwrap_or_default(),
                    "content": [],
                    "stop_reason": Value::Null,
                    "stop_sequence": Value::Null,
                    "usage": {"input_tokens": 0, "output_tokens": 0}
                }
            }),
        )]
    }

    fn finish(&mut self, input: &Value) -> Vec<StreamFrame> {
        let mut frames = Vec::new();
        if self.text_block.is_none() && self.reasoning_block.is_none() && self.tools.is_empty() {
            let index = self.allocate_index();
            self.text_block = Some(BlockState { index, open: true });
            frames.push(StreamFrame::event(
                "content_block_start",
                json!({"type": "content_block_start", "index": index, "content_block": {"type": "text", "text": ""}}),
            ));
        }
        frames.extend(self.close_text_block());
        frames.extend(self.close_reasoning_block());
        for tool in self.tools.values_mut().filter(|tool| tool.block.open) {
            tool.block.open = false;
            frames.push(content_block_stop(tool.block.index));
        }
        let output_tokens = input
            .pointer("/usage/completion_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let finish_reason = input
            .pointer("/choices/0/finish_reason")
            .and_then(Value::as_str);
        let stop_reason = match finish_reason {
            Some(reason @ ("length" | "content_filter")) => {
                transforms::openai_finish_reason_to_anthropic(reason)
            }
            _ if self.saw_tool => "tool_use",
            Some(reason) => transforms::openai_finish_reason_to_anthropic(reason),
            None => "end_turn",
        };
        frames.push(StreamFrame::event(
            "message_delta",
            json!({
                "type": "message_delta",
                "delta": {"stop_reason": stop_reason, "stop_sequence": Value::Null},
                "usage": {"output_tokens": output_tokens}
            }),
        ));
        frames.push(StreamFrame::event(
            "message_stop",
            json!({"type": "message_stop"}),
        ));
        self.completed = true;
        frames
    }

    fn allocate_index(&mut self) -> u64 {
        let index = self.next_block_index;
        self.next_block_index = self.next_block_index.saturating_add(1);
        index
    }
}

#[derive(Debug, Default)]
struct ResponsesChatState {
    response_id: String,
    model: String,
    role_sent: bool,
    next_tool_index: u64,
    tools: BTreeMap<i64, ResponsesChatToolState>,
    item_ids: BTreeMap<i64, String>,
    emitted_text_items: BTreeSet<String>,
    emitted_reasoning_items: BTreeSet<String>,
    completed: bool,
}

#[derive(Debug, Default)]
struct ResponsesChatToolState {
    downstream_index: Option<u64>,
    call_id: String,
    name: String,
    arguments: String,
    emitted_arguments: usize,
    custom_input: String,
    custom: bool,
    added: bool,
    done: bool,
}

impl ResponsesChatState {
    fn transform(&mut self, input: &Value) -> Vec<StreamFrame> {
        if self.completed {
            return Vec::new();
        }
        match input.get("type").and_then(Value::as_str) {
            Some("response.created" | "response.in_progress" | "response.queued") => {
                self.capture_response_identity(input);
                self.ensure_role_chunk()
            }
            Some("response.output_text.delta") => self.text_delta(input),
            Some(
                "response.reasoning_summary_text.delta"
                | "response.reasoning_text.delta"
                | "response.reasoning.delta",
            ) => self.reasoning_delta(input),
            Some("response.output_item.added") => self.output_item_added(input),
            Some("response.function_call_arguments.delta") => self.argument_delta(input),
            Some("response.function_call_arguments.done") => self.argument_done(input),
            Some("response.custom_tool_call_input.delta") => self.custom_input_delta(input),
            Some("response.custom_tool_call_input.done") => self.custom_input_done(input),
            Some("response.output_item.done") => self.output_item_done(input),
            Some("response.completed" | "response.incomplete") => self.complete(input),
            Some("response.failed" | "error") => self.fail(input),
            _ => Vec::new(),
        }
    }

    fn capture_response_identity(&mut self, input: &Value) {
        let response = input.get("response").unwrap_or(input);
        if let Some(id) = response.get("id").and_then(Value::as_str) {
            self.response_id = id.to_string();
        }
        if let Some(model) = response.get("model").and_then(Value::as_str) {
            self.model = model.to_string();
        }
    }

    fn ensure_role_chunk(&mut self) -> Vec<StreamFrame> {
        if self.role_sent {
            return Vec::new();
        }
        self.role_sent = true;
        vec![chat_stream_chunk(
            &self.response_id,
            &self.model,
            json!({"role": "assistant"}),
            Value::Null,
            None,
        )]
    }

    fn text_delta(&mut self, input: &Value) -> Vec<StreamFrame> {
        let Some(delta) = input.get("delta").and_then(Value::as_str) else {
            return Vec::new();
        };
        let mut frames = self.ensure_role_chunk();
        mark_response_item_emitted(&mut self.emitted_text_items, input, None, &self.item_ids);
        frames.push(chat_stream_chunk(
            &self.response_id,
            &self.model,
            json!({"content": delta}),
            Value::Null,
            None,
        ));
        frames
    }

    fn reasoning_delta(&mut self, input: &Value) -> Vec<StreamFrame> {
        let Some(delta) = input.get("delta").and_then(Value::as_str) else {
            return Vec::new();
        };
        let mut frames = self.ensure_role_chunk();
        mark_response_item_emitted(
            &mut self.emitted_reasoning_items,
            input,
            None,
            &self.item_ids,
        );
        frames.push(chat_stream_chunk(
            &self.response_id,
            &self.model,
            json!({"reasoning_content": delta}),
            Value::Null,
            None,
        ));
        frames
    }

    fn output_item_added(&mut self, input: &Value) -> Vec<StreamFrame> {
        let Some(item) = input.get("item") else {
            return Vec::new();
        };
        self.remember_item_identity(input, item);
        if !is_responses_tool_call(item) {
            return Vec::new();
        }
        let Some(output_index) = response_output_index(input) else {
            protocol_error("missing_output_index");
            return Vec::new();
        };
        let output_index = self.resolve_tool_index(output_index, item);
        self.update_tool_identity(output_index, item);
        self.open_tool(output_index)
    }

    fn argument_delta(&mut self, input: &Value) -> Vec<StreamFrame> {
        let Some(output_index) = response_output_index(input) else {
            protocol_error("missing_output_index");
            return Vec::new();
        };
        let Some(delta) = input.get("delta").and_then(Value::as_str) else {
            return Vec::new();
        };
        let state = self.tools.entry(output_index).or_default();
        state.arguments.push_str(delta);
        let mut frames = self.open_tool(output_index);
        let Some(state) = self.tools.get_mut(&output_index) else {
            return frames;
        };
        if state.added && state.emitted_arguments < state.arguments.len() {
            let delta = state.arguments[state.emitted_arguments..].to_string();
            state.emitted_arguments = state.arguments.len();
            frames.push(chat_tool_arguments_chunk(
                &self.response_id,
                &self.model,
                state.downstream_index.unwrap_or(0),
                &delta,
            ));
        }
        frames
    }

    fn argument_done(&mut self, input: &Value) -> Vec<StreamFrame> {
        let Some(output_index) = response_output_index(input) else {
            return Vec::new();
        };
        if let Some(arguments) = input.get("arguments").and_then(Value::as_str) {
            let state = self.tools.entry(output_index).or_default();
            if state.arguments.is_empty() {
                state.arguments = arguments.to_string();
            }
        }
        self.emit_unseen_tool_arguments(output_index)
    }

    fn custom_input_delta(&mut self, input: &Value) -> Vec<StreamFrame> {
        let Some(output_index) = response_output_index(input) else {
            return Vec::new();
        };
        let Some(delta) = input.get("delta").and_then(Value::as_str) else {
            return Vec::new();
        };
        let state = self.tools.entry(output_index).or_default();
        state.custom = true;
        state.custom_input.push_str(delta);
        Vec::new()
    }

    fn custom_input_done(&mut self, input: &Value) -> Vec<StreamFrame> {
        let Some(output_index) = response_output_index(input) else {
            return Vec::new();
        };
        let state = self.tools.entry(output_index).or_default();
        state.custom = true;
        let custom_input = preferred_custom_tool_input(input.get("input"), &state.custom_input);
        if state.arguments.is_empty() {
            state.arguments = custom_tool_arguments(custom_input);
        }
        self.emit_unseen_tool_arguments(output_index)
    }

    fn output_item_done(&mut self, input: &Value) -> Vec<StreamFrame> {
        let Some(item) = input.get("item") else {
            return Vec::new();
        };
        self.remember_item_identity(input, item);
        if item.get("type").and_then(Value::as_str) == Some("reasoning") {
            if !response_item_was_emitted(
                &self.emitted_reasoning_items,
                input,
                Some(item),
                &self.item_ids,
            ) {
                let summary = super::reasoning_bridge::reasoning_summary_text(item);
                if !summary.is_empty() {
                    let mut packed = input.clone();
                    packed["delta"] = Value::String(summary);
                    return self.reasoning_delta(&packed);
                }
            }
            return Vec::new();
        }
        if item.get("type").and_then(Value::as_str) == Some("message") {
            if !response_item_was_emitted(
                &self.emitted_text_items,
                input,
                Some(item),
                &self.item_ids,
            ) {
                if let Some(text) = response_message_text(item) {
                    let mut packed = input.clone();
                    packed["delta"] = Value::String(text);
                    return self.text_delta(&packed);
                }
            }
            return Vec::new();
        }
        if !is_responses_tool_call(item) {
            return Vec::new();
        }
        let Some(output_index) = response_output_index(input) else {
            return Vec::new();
        };
        let output_index = self.resolve_tool_index(output_index, item);
        self.update_tool_identity(output_index, item);
        let custom_input = self
            .tools
            .get(&output_index)
            .map(|state| state.custom_input.as_str())
            .unwrap_or_default();
        if let Some(arguments) = responses_tool_arguments_for_chat(item, custom_input) {
            let state = self.tools.entry(output_index).or_default();
            if state.arguments.is_empty() {
                state.arguments = arguments;
            }
        }
        let frames = self.emit_unseen_tool_arguments(output_index);
        if let Some(state) = self.tools.get_mut(&output_index) {
            state.done = true;
        }
        frames
    }

    fn update_tool_identity(&mut self, output_index: i64, item: &Value) {
        let state = self.tools.entry(output_index).or_default();
        state.custom = item.get("type").and_then(Value::as_str) == Some("custom_tool_call");
        if let Some(call_id) = item
            .get("call_id")
            .or_else(|| item.get("id"))
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            state.call_id = call_id.to_string();
        }
        if let Some(name) = item
            .get("name")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            state.name = if item.get("type").and_then(Value::as_str) == Some("tool_search_call") {
                "tool_search".to_string()
            } else {
                name.to_string()
            };
        }
    }

    fn remember_item_identity(&mut self, input: &Value, item: &Value) {
        let (Some(output_index), Some(item_id)) = (
            response_output_index(input),
            item.get("id").and_then(Value::as_str),
        ) else {
            return;
        };
        self.item_ids.insert(output_index, item_id.to_string());
    }

    fn resolve_tool_index(&self, output_index: i64, item: &Value) -> i64 {
        let identity = item
            .get("call_id")
            .or_else(|| item.get("id"))
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty());
        identity
            .and_then(|identity| {
                self.tools
                    .iter()
                    .find_map(|(index, state)| (state.call_id == identity).then_some(*index))
            })
            .unwrap_or(output_index)
    }

    fn open_tool(&mut self, output_index: i64) -> Vec<StreamFrame> {
        let ready = self.tools.get(&output_index).is_some_and(|state| {
            !state.added && !state.call_id.is_empty() && !state.name.is_empty()
        });
        if !ready {
            return Vec::new();
        }
        let downstream_index = self.next_tool_index;
        self.next_tool_index = self.next_tool_index.saturating_add(1);
        let (call_id, name, arguments) = {
            let state = self
                .tools
                .get_mut(&output_index)
                .expect("tool state exists");
            state.downstream_index = Some(downstream_index);
            state.added = true;
            let arguments = state.arguments.clone();
            state.emitted_arguments = arguments.len();
            (state.call_id.clone(), state.name.clone(), arguments)
        };
        let mut frames = self.ensure_role_chunk();
        frames.push(chat_stream_chunk(
            &self.response_id,
            &self.model,
            json!({
                "tool_calls": [{
                    "index": downstream_index,
                    "id": call_id,
                    "type": "function",
                    "function": {"name": name, "arguments": arguments}
                }]
            }),
            Value::Null,
            None,
        ));
        frames
    }

    fn emit_unseen_tool_arguments(&mut self, output_index: i64) -> Vec<StreamFrame> {
        let mut frames = self.open_tool(output_index);
        let Some(state) = self.tools.get_mut(&output_index) else {
            return frames;
        };
        if state.added && state.emitted_arguments < state.arguments.len() {
            let delta = state.arguments[state.emitted_arguments..].to_string();
            state.emitted_arguments = state.arguments.len();
            frames.push(chat_tool_arguments_chunk(
                &self.response_id,
                &self.model,
                state.downstream_index.unwrap_or(0),
                &delta,
            ));
        }
        frames
    }

    fn complete(&mut self, input: &Value) -> Vec<StreamFrame> {
        self.capture_response_identity(input);
        let response = input.get("response").unwrap_or(input);
        let mut frames = self.ensure_role_chunk();
        if let Some(output) = response.get("output").and_then(Value::as_array) {
            for (index, item) in output.iter().enumerate() {
                frames.extend(self.output_item_done(&json!({
                    "output_index": index,
                    "item": item
                })));
            }
        }
        let has_tools = self.tools.values().any(|state| state.added);
        let finish_reason = transforms::openai_response_finish_reason_to_chat(response, has_tools);
        let usage = response
            .get("usage")
            .map(|usage| transforms::openai_chat_usage_from_responses_usage(Some(usage)));
        frames.push(chat_stream_chunk(
            &self.response_id,
            &self.model,
            json!({}),
            finish_reason,
            usage,
        ));
        frames.push(StreamFrame::done());
        self.completed = true;
        frames
    }

    fn fail(&mut self, input: &Value) -> Vec<StreamFrame> {
        let error = input
            .pointer("/response/error")
            .or_else(|| input.get("error"))
            .cloned()
            .unwrap_or_else(
                || json!({"type": "upstream_error", "message": "upstream response failed"}),
            );
        self.completed = true;
        vec![StreamFrame::json(json!({"error": error}))]
    }
}

#[derive(Debug)]
struct ChatResponsesState {
    responses_tool_context: transforms::ResponsesToolContext,
    response_id: String,
    model: String,
    created_at: Value,
    started: bool,
    next_output_index: u64,
    text: ChatResponsesTextState,
    reasoning: ChatResponsesReasoningState,
    tools: BTreeMap<i64, ChatResponsesToolState>,
    output_items: Vec<(u64, Value)>,
    finish_reason: Option<String>,
    usage: Option<Value>,
    completed: bool,
}

#[derive(Debug, Default)]
struct ChatResponsesTextState {
    output_index: Option<u64>,
    item_id: String,
    text: String,
    added: bool,
    done: bool,
}

#[derive(Debug, Default)]
struct ChatResponsesReasoningState {
    output_index: Option<u64>,
    item_id: String,
    text: String,
    added: bool,
    done: bool,
}

#[derive(Debug, Default)]
struct ChatResponsesToolState {
    output_index: Option<u64>,
    item_id: String,
    call_id: String,
    name: String,
    arguments: String,
    emitted_arguments: usize,
    added: bool,
    done: bool,
    custom: bool,
}

impl ChatResponsesState {
    fn new<T>(responses_tool_context: T) -> Self
    where
        T: Into<transforms::ResponsesToolContext>,
    {
        Self {
            responses_tool_context: responses_tool_context.into(),
            response_id: "resp_ccswitch".to_string(),
            model: String::new(),
            created_at: Value::Null,
            started: false,
            next_output_index: 0,
            text: ChatResponsesTextState::default(),
            reasoning: ChatResponsesReasoningState::default(),
            tools: BTreeMap::new(),
            output_items: Vec::new(),
            finish_reason: None,
            usage: None,
            completed: false,
        }
    }

    fn transform(&mut self, input: &Value) -> Vec<StreamFrame> {
        if self.completed {
            return Vec::new();
        }
        if input.get("error").is_some_and(|error| !error.is_null())
            || input.get("type").and_then(Value::as_str) == Some("error")
        {
            return self.fail(input);
        }
        self.capture_identity(input);
        let mut frames = self.ensure_started();
        if let Some(usage) = input.get("usage") {
            self.usage = Some(usage.clone());
        }
        let Some(choice) = input.pointer("/choices/0") else {
            return frames;
        };
        let delta = transforms::openai_chat_choice_payload(choice);
        if let Some(reasoning) = chat_reasoning_delta(delta) {
            frames.extend(self.reasoning_delta(reasoning));
        }
        for content in transforms::openai_chat_visible_text_fragments(delta) {
            frames.extend(self.text_delta(content));
        }
        let tool_calls = delta.get("tool_calls").and_then(Value::as_array);
        if let Some(tool_calls) = tool_calls {
            for tool_call in tool_calls {
                frames.extend(self.tool_delta(tool_call));
            }
        }
        if tool_calls.is_none_or(|tool_calls| tool_calls.is_empty()) {
            if let Some(tool_call) = transforms::openai_chat_legacy_tool_delta(delta) {
                frames.extend(self.tool_delta(&tool_call));
            }
        }
        if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
            self.finish_reason = Some(reason.to_string());
        }
        frames
    }

    fn capture_identity(&mut self, input: &Value) {
        if let Some(id) = input.get("id").and_then(Value::as_str) {
            self.response_id = if let Some(suffix) = id.strip_prefix("chatcmpl_") {
                format!("resp_{suffix}")
            } else if id.starts_with("resp_") {
                id.to_string()
            } else {
                format!("resp_{id}")
            };
        }
        if let Some(model) = input.get("model").and_then(Value::as_str) {
            self.model = model.to_string();
        }
        if let Some(created) = input.get("created") {
            self.created_at = created.clone();
        }
    }

    fn ensure_started(&mut self) -> Vec<StreamFrame> {
        if self.started {
            return Vec::new();
        }
        self.started = true;
        let response = self.response_snapshot("in_progress");
        vec![
            StreamFrame::json(json!({"type": "response.created", "response": response})),
            StreamFrame::json(json!({
                "type": "response.in_progress",
                "response": self.response_snapshot("in_progress")
            })),
        ]
    }

    fn reasoning_delta(&mut self, delta: &str) -> Vec<StreamFrame> {
        let mut frames = Vec::new();
        if !self.reasoning.added {
            let output_index = self.allocate_output_index();
            let item_id = format!("rs_{}", response_id_suffix(&self.response_id));
            self.reasoning.output_index = Some(output_index);
            self.reasoning.item_id = item_id.clone();
            self.reasoning.added = true;
            frames.push(StreamFrame::json(json!({
                "type": "response.output_item.added",
                "output_index": output_index,
                "item": {"id": item_id, "type": "reasoning", "status": "in_progress", "summary": []}
            })));
            frames.push(StreamFrame::json(json!({
                "type": "response.reasoning_summary_part.added",
                "item_id": self.reasoning.item_id,
                "output_index": output_index,
                "summary_index": 0,
                "part": {"type": "summary_text", "text": ""}
            })));
        }
        self.reasoning.text.push_str(delta);
        frames.push(StreamFrame::json(json!({
            "type": "response.reasoning_summary_text.delta",
            "item_id": self.reasoning.item_id,
            "output_index": self.reasoning.output_index.unwrap_or(0),
            "summary_index": 0,
            "delta": delta
        })));
        frames
    }

    fn text_delta(&mut self, delta: &str) -> Vec<StreamFrame> {
        let mut frames = Vec::new();
        if !self.text.added {
            let output_index = self.allocate_output_index();
            let item_id = format!("msg_{}", response_id_suffix(&self.response_id));
            self.text.output_index = Some(output_index);
            self.text.item_id = item_id.clone();
            self.text.added = true;
            frames.push(StreamFrame::json(json!({
                "type": "response.output_item.added",
                "output_index": output_index,
                "item": {"id": item_id, "type": "message", "status": "in_progress", "role": "assistant", "content": []}
            })));
            frames.push(StreamFrame::json(json!({
                "type": "response.content_part.added",
                "item_id": self.text.item_id,
                "output_index": output_index,
                "content_index": 0,
                "part": {"type": "output_text", "text": "", "annotations": []}
            })));
        }
        self.text.text.push_str(delta);
        frames.push(StreamFrame::json(json!({
            "type": "response.output_text.delta",
            "item_id": self.text.item_id,
            "output_index": self.text.output_index.unwrap_or(0),
            "content_index": 0,
            "delta": delta
        })));
        frames
    }

    fn tool_delta(&mut self, tool_call: &Value) -> Vec<StreamFrame> {
        let upstream_index = tool_call.get("index").and_then(Value::as_i64).unwrap_or(0);
        let state = self.tools.entry(upstream_index).or_default();
        if let Some(call_id) = tool_call
            .get("id")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            state.call_id = call_id.to_string();
        }
        if let Some(name) = tool_call
            .pointer("/function/name")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            state.name = name.to_string();
            state.custom = self.responses_tool_context.is_custom_tool_chat_name(name);
        }
        if let Some(arguments) = tool_call
            .pointer("/function/arguments")
            .and_then(Value::as_str)
        {
            state.arguments.push_str(arguments);
        }
        let mut frames = self.flush_ready_tools();
        if let Some(state) = self.tools.get_mut(&upstream_index) {
            if !state.custom && state.added && state.emitted_arguments < state.arguments.len() {
                let delta = state.arguments[state.emitted_arguments..].to_string();
                state.emitted_arguments = state.arguments.len();
                frames.push(tool_arguments_delta_frame(state, &delta));
            }
        }
        frames
    }

    fn flush_ready_tools(&mut self) -> Vec<StreamFrame> {
        let mut frames = Vec::new();
        loop {
            let next = self
                .tools
                .iter()
                .find(|(_, state)| !state.added && !state.done)
                .map(|(index, state)| {
                    (*index, !state.call_id.is_empty() && !state.name.is_empty())
                });
            let Some((upstream_index, true)) = next else {
                break;
            };
            let output_index = self.allocate_output_index();
            let state = self
                .tools
                .get_mut(&upstream_index)
                .expect("pending tool exists");
            state.output_index = Some(output_index);
            state.item_id = self
                .responses_tool_context
                .response_item_id(&state.call_id, &state.name);
            state.added = true;
            frames.push(StreamFrame::json(json!({
                "type": "response.output_item.added",
                "output_index": output_index,
                "item": tool_response_item(
                    state,
                    "in_progress",
                    "",
                    &self.responses_tool_context,
                )
            })));
            if !state.custom && !state.arguments.is_empty() {
                let arguments = state.arguments.clone();
                state.emitted_arguments = arguments.len();
                frames.push(tool_arguments_delta_frame(state, &arguments));
            }
        }
        frames
    }

    fn finish_stream(&mut self) -> Vec<StreamFrame> {
        if self.completed {
            return Vec::new();
        }
        if !self.started {
            return self.fail(&json!({
                "error": {"type": "upstream_error", "message": "chat stream ended without output"}
            }));
        }
        let mut frames = self.flush_ready_tools();
        frames.extend(self.finalize_reasoning());
        frames.extend(self.finalize_text());
        frames.extend(self.finalize_tools());
        if self.finish_reason.is_none() {
            let mut response = self.response_snapshot("failed");
            response["output"] = Value::Array(self.completed_output_items());
            response["usage"] =
                transforms::openai_responses_usage_from_chat_usage(self.usage.as_ref());
            response["error"] = json!({
                "type": "upstream_error",
                "code": "stream_incomplete",
                "message": "chat stream ended before finish_reason"
            });
            frames.push(StreamFrame::json(json!({
                "type": "response.failed",
                "response": response
            })));
            frames.push(StreamFrame::done());
            self.completed = true;
            return frames;
        }
        if self.finish_reason.as_deref() == Some("content_filter") {
            let mut response = self.response_snapshot("failed");
            response["output"] = Value::Array(self.completed_output_items());
            response["usage"] =
                transforms::openai_responses_usage_from_chat_usage(self.usage.as_ref());
            response["error"] = json!({
                "type": "content_filter_error",
                "code": "content_filter",
                "message": "upstream response was blocked by content filtering"
            });
            frames.push(StreamFrame::json(json!({
                "type": "response.failed",
                "response": response
            })));
            frames.push(StreamFrame::done());
            self.completed = true;
            return frames;
        }
        let status = if self.finish_reason.as_deref() == Some("length") {
            "incomplete"
        } else {
            "completed"
        };
        let mut response = self.response_snapshot(status);
        response["output"] = Value::Array(self.completed_output_items());
        response["usage"] = transforms::openai_responses_usage_from_chat_usage(self.usage.as_ref());
        if status == "incomplete" {
            response["incomplete_details"] = json!({"reason": "max_output_tokens"});
        }
        frames.push(StreamFrame::json(json!({
            "type": if status == "incomplete" {"response.incomplete"} else {"response.completed"},
            "response": response
        })));
        frames.push(StreamFrame::done());
        self.completed = true;
        frames
    }

    fn finalize_reasoning(&mut self) -> Vec<StreamFrame> {
        if !self.reasoning.added || self.reasoning.done {
            return Vec::new();
        }
        let output_index = self.reasoning.output_index.unwrap_or(0);
        let item = json!({
            "id": self.reasoning.item_id,
            "type": "reasoning",
            "status": "completed",
            "summary": [{"type": "summary_text", "text": self.reasoning.text}]
        });
        self.reasoning.done = true;
        self.output_items.push((output_index, item.clone()));
        vec![
            StreamFrame::json(json!({
                "type": "response.reasoning_summary_text.done",
                "item_id": self.reasoning.item_id,
                "output_index": output_index,
                "summary_index": 0,
                "text": self.reasoning.text
            })),
            StreamFrame::json(json!({
                "type": "response.reasoning_summary_part.done",
                "item_id": self.reasoning.item_id,
                "output_index": output_index,
                "summary_index": 0,
                "part": {"type": "summary_text", "text": self.reasoning.text}
            })),
            StreamFrame::json(json!({
                "type": "response.output_item.done",
                "output_index": output_index,
                "item": item
            })),
        ]
    }

    fn finalize_text(&mut self) -> Vec<StreamFrame> {
        if !self.text.added || self.text.done {
            return Vec::new();
        }
        let output_index = self.text.output_index.unwrap_or(0);
        let part = json!({"type": "output_text", "text": self.text.text, "annotations": []});
        let item = json!({
            "id": self.text.item_id,
            "type": "message",
            "status": "completed",
            "role": "assistant",
            "content": [part.clone()]
        });
        self.text.done = true;
        self.output_items.push((output_index, item.clone()));
        vec![
            StreamFrame::json(json!({
                "type": "response.output_text.done",
                "item_id": self.text.item_id,
                "output_index": output_index,
                "content_index": 0,
                "text": self.text.text
            })),
            StreamFrame::json(json!({
                "type": "response.content_part.done",
                "item_id": self.text.item_id,
                "output_index": output_index,
                "content_index": 0,
                "part": part
            })),
            StreamFrame::json(json!({
                "type": "response.output_item.done",
                "output_index": output_index,
                "item": item
            })),
        ]
    }

    fn finalize_tools(&mut self) -> Vec<StreamFrame> {
        let mut frames = Vec::new();
        let indexes = self.tools.keys().copied().collect::<Vec<_>>();
        for index in indexes {
            let missing_identity = self.tools.get(&index).is_some_and(|state| {
                !state.added && (state.call_id.is_empty() || state.name.is_empty())
            });
            if missing_identity {
                if let Some(state) = self.tools.get_mut(&index) {
                    state.done = true;
                }
                protocol_error("tool_identity_missing");
                continue;
            }
            frames.extend(self.flush_ready_tools());
            let Some(state) = self.tools.get_mut(&index) else {
                continue;
            };
            if state.done || !state.added {
                continue;
            }
            let arguments = if state.custom {
                transforms::unwrap_custom_tool_input(&state.arguments)
            } else {
                canonicalize_arguments(&state.arguments)
            };
            if !state.custom && state.emitted_arguments < state.arguments.len() {
                let delta = state.arguments[state.emitted_arguments..].to_string();
                state.emitted_arguments = state.arguments.len();
                frames.push(tool_arguments_delta_frame(state, &delta));
            } else if state.custom && !arguments.is_empty() {
                state.emitted_arguments = state.arguments.len();
                frames.push(tool_arguments_delta_frame(state, &arguments));
            }
            let item =
                tool_response_item(state, "completed", &arguments, &self.responses_tool_context);
            let output_index = state.output_index.unwrap_or(0);
            frames.push(tool_arguments_done_frame(state, &arguments));
            frames.push(StreamFrame::json(json!({
                "type": "response.output_item.done",
                "output_index": output_index,
                "item": item
            })));
            state.done = true;
            self.output_items.push((output_index, item));
        }
        frames
    }

    fn fail(&mut self, input: &Value) -> Vec<StreamFrame> {
        let error = input.get("error").cloned().unwrap_or_else(|| input.clone());
        let mut response = self.response_snapshot("failed");
        response["error"] = error.clone();
        self.completed = true;
        vec![
            StreamFrame::json(json!({"type": "response.failed", "response": response})),
            StreamFrame::done(),
        ]
    }

    fn response_snapshot(&self, status: &str) -> Value {
        json!({
            "id": self.response_id,
            "object": "response",
            "created_at": self.created_at,
            "status": status,
            "model": self.model,
            "output": [],
            "usage": Value::Null
        })
    }

    fn completed_output_items(&self) -> Vec<Value> {
        let mut output = self.output_items.clone();
        output.sort_by_key(|(index, _)| *index);
        output.into_iter().map(|(_, item)| item).collect()
    }

    fn allocate_output_index(&mut self) -> u64 {
        let index = self.next_output_index;
        self.next_output_index = self.next_output_index.saturating_add(1);
        index
    }
}

fn chat_stream_chunk(
    response_id: &str,
    model: &str,
    delta: Value,
    finish_reason: Value,
    usage: Option<Value>,
) -> StreamFrame {
    let id = if let Some(suffix) = response_id.strip_prefix("resp_") {
        format!("chatcmpl_{suffix}")
    } else if response_id.starts_with("chatcmpl_") {
        response_id.to_string()
    } else {
        format!("chatcmpl_{response_id}")
    };
    let mut chunk = json!({
        "id": id,
        "object": "chat.completion.chunk",
        "model": model,
        "choices": [{"index": 0, "delta": delta, "finish_reason": finish_reason}]
    });
    if let Some(usage) = usage {
        chunk["usage"] = usage;
    }
    StreamFrame::json(chunk)
}

fn chat_tool_arguments_chunk(
    response_id: &str,
    model: &str,
    index: u64,
    arguments: &str,
) -> StreamFrame {
    chat_stream_chunk(
        response_id,
        model,
        json!({"tool_calls": [{"index": index, "function": {"arguments": arguments}}]}),
        Value::Null,
        None,
    )
}

fn response_output_index(input: &Value) -> Option<i64> {
    input
        .get("output_index")
        .and_then(Value::as_i64)
        .or_else(|| {
            input
                .get("output_index")
                .and_then(Value::as_u64)
                .and_then(|value| i64::try_from(value).ok())
        })
}

fn response_item_keys(
    input: &Value,
    item: Option<&Value>,
    item_ids: &BTreeMap<i64, String>,
) -> BTreeSet<String> {
    let mut keys = BTreeSet::new();
    if let Some(output_index) = response_output_index(input) {
        keys.insert(format!("output:{output_index}"));
        if let Some(item_id) = item_ids.get(&output_index) {
            keys.insert(format!("item:{item_id}"));
        }
    }
    if let Some(item_id) = input
        .get("item_id")
        .or_else(|| item.and_then(|item| item.get("id")))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    {
        keys.insert(format!("item:{item_id}"));
    }
    keys
}

fn mark_response_item_emitted(
    emitted: &mut BTreeSet<String>,
    input: &Value,
    item: Option<&Value>,
    item_ids: &BTreeMap<i64, String>,
) {
    emitted.extend(response_item_keys(input, item, item_ids));
}

fn response_item_was_emitted(
    emitted: &BTreeSet<String>,
    input: &Value,
    item: Option<&Value>,
    item_ids: &BTreeMap<i64, String>,
) -> bool {
    response_item_keys(input, item, item_ids)
        .iter()
        .any(|key| emitted.contains(key))
}

fn is_responses_tool_call(item: &Value) -> bool {
    matches!(
        item.get("type").and_then(Value::as_str),
        Some("function_call" | "custom_tool_call" | "tool_search_call")
    )
}

fn response_message_text(item: &Value) -> Option<String> {
    let text = item
        .get("content")
        .and_then(Value::as_array)?
        .iter()
        .filter_map(|part| {
            part.get("text")
                .or_else(|| part.get("refusal"))
                .and_then(Value::as_str)
        })
        .collect::<String>();
    (!text.is_empty()).then_some(text)
}

fn chat_reasoning_delta(delta: &Value) -> Option<&str> {
    ["reasoning_content", "reasoning_text", "reasoning"]
        .into_iter()
        .find_map(|key| delta.get(key).and_then(Value::as_str))
        .filter(|value| !value.is_empty())
}

fn response_id_suffix(value: &str) -> &str {
    value
        .strip_prefix("resp_")
        .or_else(|| value.strip_prefix("chatcmpl_"))
        .unwrap_or(value)
}

fn tool_response_item(
    state: &ChatResponsesToolState,
    status: &str,
    arguments: &str,
    tool_context: &transforms::ResponsesToolContext,
) -> Value {
    tool_context.response_item(
        &state.item_id,
        status,
        &state.call_id,
        &state.name,
        arguments,
    )
}

fn tool_arguments_delta_frame(state: &ChatResponsesToolState, delta: &str) -> StreamFrame {
    StreamFrame::json(json!({
        "type": if state.custom {"response.custom_tool_call_input.delta"} else {"response.function_call_arguments.delta"},
        "item_id": state.item_id,
        "output_index": state.output_index.unwrap_or(0),
        "delta": delta
    }))
}

fn tool_arguments_done_frame(state: &ChatResponsesToolState, arguments: &str) -> StreamFrame {
    let mut frame = json!({
        "type": if state.custom {"response.custom_tool_call_input.done"} else {"response.function_call_arguments.done"},
        "item_id": state.item_id,
        "output_index": state.output_index.unwrap_or(0)
    });
    frame[if state.custom { "input" } else { "arguments" }] = json!(arguments);
    StreamFrame::json(frame)
}

fn custom_tool_arguments(input: Value) -> String {
    serde_json::to_string(&json!({"input": input})).unwrap_or_else(|_| "{}".to_string())
}

fn preferred_custom_tool_input(input: Option<&Value>, streamed_input: &str) -> Value {
    input
        .filter(|value| !custom_tool_input_is_empty(value) || streamed_input.is_empty())
        .cloned()
        .unwrap_or_else(|| Value::String(streamed_input.to_string()))
}

fn custom_tool_input_is_empty(input: &Value) -> bool {
    match input {
        Value::Null => true,
        Value::String(value) => value.is_empty(),
        Value::Array(value) => value.is_empty(),
        Value::Object(value) => value.is_empty(),
        Value::Bool(_) | Value::Number(_) => false,
    }
}

fn responses_tool_arguments_for_chat(item: &Value, streamed_custom_input: &str) -> Option<String> {
    match item.get("type").and_then(Value::as_str) {
        Some("custom_tool_call") => Some(custom_tool_arguments(preferred_custom_tool_input(
            item.get("input"),
            streamed_custom_input,
        ))),
        Some("tool_search_call") => item.get("arguments").map(|arguments| {
            arguments
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| serde_json::to_string(arguments).unwrap_or_default())
        }),
        _ => item
            .get("arguments")
            .and_then(Value::as_str)
            .map(str::to_string),
    }
}

fn canonicalize_arguments(arguments: &str) -> String {
    serde_json::from_str::<Value>(arguments)
        .ok()
        .and_then(|value| serde_json::to_string(&value).ok())
        .unwrap_or_else(|| arguments.to_string())
}

#[derive(Debug, Default)]
struct AnthropicResponsesState {
    responses_tool_context: transforms::ResponsesToolContext,
    response_id: String,
    model: String,
    started: bool,
    next_output_index: u64,
    blocks: BTreeMap<i64, AnthropicResponsesBlock>,
    output_items: Vec<(u64, Value)>,
    stop_reason: Option<String>,
    usage: Option<Value>,
    completed: bool,
}

#[derive(Debug)]
enum AnthropicResponsesBlock {
    Text {
        output_index: u64,
        item_id: String,
        text: String,
        done: bool,
    },
    Reasoning {
        output_index: u64,
        item_id: String,
        text: String,
        signature: String,
        redacted_data: Option<String>,
        done: bool,
    },
    Tool {
        output_index: u64,
        item_id: String,
        call_id: String,
        name: String,
        arguments: String,
        custom: bool,
        done: bool,
    },
}

impl AnthropicResponsesState {
    fn new<T>(responses_tool_context: T) -> Self
    where
        T: Into<transforms::ResponsesToolContext>,
    {
        Self {
            responses_tool_context: responses_tool_context.into(),
            ..Self::default()
        }
    }

    fn transform(&mut self, input: &Value) -> Vec<StreamFrame> {
        if self.completed {
            return Vec::new();
        }
        match input.get("type").and_then(Value::as_str) {
            Some("message_start") => self.message_start(input),
            Some("content_block_start") => self.content_block_start(input),
            Some("content_block_delta") => self.content_block_delta(input),
            Some("content_block_stop") => self.content_block_stop(input),
            Some("message_delta") => self.message_delta(input),
            Some("message_stop") => self.complete(),
            Some("error") => self.fail(input),
            _ => Vec::new(),
        }
    }

    fn message_start(&mut self, input: &Value) -> Vec<StreamFrame> {
        let message = input.get("message").unwrap_or(input);
        if let Some(id) = message.get("id").and_then(Value::as_str) {
            self.response_id = if id.starts_with("resp_") {
                id.to_string()
            } else {
                format!("resp_{id}")
            };
        }
        if let Some(model) = message.get("model").and_then(Value::as_str) {
            self.model = model.to_string();
        }
        if let Some(usage) = message.get("usage") {
            self.usage = Some(usage.clone());
        }
        self.ensure_started()
    }

    fn ensure_started(&mut self) -> Vec<StreamFrame> {
        if self.started {
            return Vec::new();
        }
        self.started = true;
        vec![
            StreamFrame::json(json!({
                "type": "response.created",
                "response": self.response_snapshot("in_progress")
            })),
            StreamFrame::json(json!({
                "type": "response.in_progress",
                "response": self.response_snapshot("in_progress")
            })),
        ]
    }

    fn content_block_start(&mut self, input: &Value) -> Vec<StreamFrame> {
        let Some(index) = input.get("index").and_then(Value::as_i64) else {
            protocol_error("missing_content_block_index");
            return Vec::new();
        };
        let Some(block) = input.get("content_block") else {
            return Vec::new();
        };
        let mut frames = self.ensure_started();
        let output_index = self.allocate_output_index();
        match block.get("type").and_then(Value::as_str) {
            Some("text") => {
                let item_id = format!("msg_{}_{}", response_id_suffix(&self.response_id), index);
                let initial = block
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                self.blocks.insert(
                    index,
                    AnthropicResponsesBlock::Text {
                        output_index,
                        item_id: item_id.clone(),
                        text: initial.clone(),
                        done: false,
                    },
                );
                frames.push(StreamFrame::json(json!({
                    "type": "response.output_item.added",
                    "output_index": output_index,
                    "item": {"id": item_id, "type": "message", "status": "in_progress", "role": "assistant", "content": []}
                })));
                frames.push(StreamFrame::json(json!({
                    "type": "response.content_part.added",
                    "item_id": item_id,
                    "output_index": output_index,
                    "content_index": 0,
                    "part": {"type": "output_text", "text": "", "annotations": []}
                })));
                if !initial.is_empty() {
                    frames.push(StreamFrame::json(json!({
                        "type": "response.output_text.delta",
                        "item_id": item_id,
                        "output_index": output_index,
                        "content_index": 0,
                        "delta": initial
                    })));
                }
            }
            Some("thinking" | "redacted_thinking") => {
                let item_id = format!("rs_{}_{}", response_id_suffix(&self.response_id), index);
                let text = block
                    .get("thinking")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let signature = block
                    .get("signature")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let redacted_data = block
                    .get("data")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                self.blocks.insert(
                    index,
                    AnthropicResponsesBlock::Reasoning {
                        output_index,
                        item_id: item_id.clone(),
                        text: text.clone(),
                        signature,
                        redacted_data,
                        done: false,
                    },
                );
                frames.push(StreamFrame::json(json!({
                    "type": "response.output_item.added",
                    "output_index": output_index,
                    "item": {"id": item_id, "type": "reasoning", "status": "in_progress", "summary": []}
                })));
                frames.push(StreamFrame::json(json!({
                    "type": "response.reasoning_summary_part.added",
                    "item_id": item_id,
                    "output_index": output_index,
                    "summary_index": 0,
                    "part": {"type": "summary_text", "text": ""}
                })));
                if !text.is_empty() {
                    frames.push(StreamFrame::json(json!({
                        "type": "response.reasoning_summary_text.delta",
                        "item_id": item_id,
                        "output_index": output_index,
                        "summary_index": 0,
                        "delta": text
                    })));
                }
            }
            Some("tool_use") => {
                let call_id = block
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or("tool")
                    .to_string();
                let name = block
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("tool")
                    .to_string();
                let arguments = block
                    .get("input")
                    .filter(|value| value.is_object())
                    .and_then(|value| serde_json::to_string(value).ok())
                    .filter(|value| value != "{}")
                    .unwrap_or_default();
                let custom = self.responses_tool_context.is_custom_tool_chat_name(&name);
                let item_id = self
                    .responses_tool_context
                    .response_item_id(&call_id, &name);
                self.blocks.insert(
                    index,
                    AnthropicResponsesBlock::Tool {
                        output_index,
                        item_id: item_id.clone(),
                        call_id: call_id.clone(),
                        name: name.clone(),
                        arguments: arguments.clone(),
                        custom,
                        done: false,
                    },
                );
                let item = self.responses_tool_context.response_item(
                    &item_id,
                    "in_progress",
                    &call_id,
                    &name,
                    "",
                );
                frames.push(StreamFrame::json(json!({
                    "type": "response.output_item.added",
                    "output_index": output_index,
                    "item": item
                })));
                if !custom && !arguments.is_empty() {
                    frames.push(StreamFrame::json(json!({
                        "type": "response.function_call_arguments.delta",
                        "item_id": item_id,
                        "output_index": output_index,
                        "delta": arguments
                    })));
                }
            }
            _ => {
                self.next_output_index = self.next_output_index.saturating_sub(1);
            }
        }
        frames
    }

    fn content_block_delta(&mut self, input: &Value) -> Vec<StreamFrame> {
        let Some(index) = input.get("index").and_then(Value::as_i64) else {
            return Vec::new();
        };
        let Some(delta) = input.get("delta") else {
            return Vec::new();
        };
        let Some(block) = self.blocks.get_mut(&index) else {
            protocol_error("unknown_content_block_index");
            return Vec::new();
        };
        match block {
            AnthropicResponsesBlock::Text {
                output_index,
                item_id,
                text,
                ..
            } => {
                let Some(value) = delta.get("text").and_then(Value::as_str) else {
                    return Vec::new();
                };
                text.push_str(value);
                vec![StreamFrame::json(json!({
                    "type": "response.output_text.delta",
                    "item_id": item_id,
                    "output_index": output_index,
                    "content_index": 0,
                    "delta": value
                }))]
            }
            AnthropicResponsesBlock::Reasoning {
                output_index,
                item_id,
                text,
                signature,
                ..
            } => {
                if let Some(value) = delta.get("thinking").and_then(Value::as_str) {
                    text.push_str(value);
                    return vec![StreamFrame::json(json!({
                        "type": "response.reasoning_summary_text.delta",
                        "item_id": item_id,
                        "output_index": output_index,
                        "summary_index": 0,
                        "delta": value
                    }))];
                }
                if let Some(value) = delta.get("signature").and_then(Value::as_str) {
                    signature.push_str(value);
                }
                Vec::new()
            }
            AnthropicResponsesBlock::Tool {
                output_index,
                item_id,
                arguments,
                custom,
                ..
            } => {
                let Some(value) = delta.get("partial_json").and_then(Value::as_str) else {
                    return Vec::new();
                };
                arguments.push_str(value);
                if *custom {
                    return Vec::new();
                }
                vec![StreamFrame::json(json!({
                    "type": "response.function_call_arguments.delta",
                    "item_id": item_id,
                    "output_index": output_index,
                    "delta": value
                }))]
            }
        }
    }

    fn content_block_stop(&mut self, input: &Value) -> Vec<StreamFrame> {
        let Some(index) = input.get("index").and_then(Value::as_i64) else {
            return Vec::new();
        };
        self.finalize_block(index)
    }

    fn finalize_block(&mut self, index: i64) -> Vec<StreamFrame> {
        let Some(block) = self.blocks.get_mut(&index) else {
            return Vec::new();
        };
        match block {
            AnthropicResponsesBlock::Text {
                output_index,
                item_id,
                text,
                done,
            } => {
                if *done {
                    return Vec::new();
                }
                *done = true;
                let part = json!({"type": "output_text", "text": text, "annotations": []});
                let item = json!({
                    "id": item_id,
                    "type": "message",
                    "status": "completed",
                    "role": "assistant",
                    "content": [part.clone()]
                });
                self.output_items.push((*output_index, item.clone()));
                vec![
                    StreamFrame::json(json!({
                        "type": "response.output_text.done",
                        "item_id": item_id,
                        "output_index": output_index,
                        "content_index": 0,
                        "text": text
                    })),
                    StreamFrame::json(json!({
                        "type": "response.content_part.done",
                        "item_id": item_id,
                        "output_index": output_index,
                        "content_index": 0,
                        "part": part
                    })),
                    StreamFrame::json(json!({
                        "type": "response.output_item.done",
                        "output_index": output_index,
                        "item": item
                    })),
                ]
            }
            AnthropicResponsesBlock::Reasoning {
                output_index,
                item_id,
                text,
                signature,
                redacted_data,
                done,
            } => {
                if *done {
                    return Vec::new();
                }
                *done = true;
                let anthropic_block = if let Some(data) = redacted_data.as_deref() {
                    json!({"type": "redacted_thinking", "data": data})
                } else {
                    json!({"type": "thinking", "thinking": text, "signature": signature})
                };
                let item = responses_reasoning_item_from_anthropic_block(item_id, &anthropic_block)
                    .or_else(|| unsigned_responses_reasoning_item(item_id, text))
                    .unwrap_or_else(|| json!({"id": item_id, "type": "reasoning", "summary": []}));
                self.output_items.push((*output_index, item.clone()));
                vec![
                    StreamFrame::json(json!({
                        "type": "response.reasoning_summary_text.done",
                        "item_id": item_id,
                        "output_index": output_index,
                        "summary_index": 0,
                        "text": text
                    })),
                    StreamFrame::json(json!({
                        "type": "response.reasoning_summary_part.done",
                        "item_id": item_id,
                        "output_index": output_index,
                        "summary_index": 0,
                        "part": {"type": "summary_text", "text": text}
                    })),
                    StreamFrame::json(json!({
                        "type": "response.output_item.done",
                        "output_index": output_index,
                        "item": item
                    })),
                ]
            }
            AnthropicResponsesBlock::Tool {
                output_index,
                item_id,
                call_id,
                name,
                arguments,
                custom,
                done,
            } => {
                if *done {
                    return Vec::new();
                }
                *done = true;
                let arguments = canonicalize_arguments(arguments);
                if *custom {
                    let item = self.responses_tool_context.response_item(
                        item_id,
                        "completed",
                        call_id,
                        name,
                        &arguments,
                    );
                    let input = item
                        .get("input")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    self.output_items.push((*output_index, item.clone()));
                    let mut frames = Vec::new();
                    if !input.is_empty() {
                        frames.push(StreamFrame::json(json!({
                            "type": "response.custom_tool_call_input.delta",
                            "item_id": item_id,
                            "output_index": output_index,
                            "delta": input
                        })));
                    }
                    frames.push(StreamFrame::json(json!({
                        "type": "response.custom_tool_call_input.done",
                        "item_id": item_id,
                        "output_index": output_index,
                        "input": input
                    })));
                    frames.push(StreamFrame::json(json!({
                        "type": "response.output_item.done",
                        "output_index": output_index,
                        "item": item
                    })));
                    return frames;
                }
                let item = self.responses_tool_context.response_item(
                    item_id,
                    "completed",
                    call_id,
                    name,
                    &arguments,
                );
                self.output_items.push((*output_index, item.clone()));
                vec![
                    StreamFrame::json(json!({
                        "type": "response.function_call_arguments.done",
                        "item_id": item_id,
                        "output_index": output_index,
                        "arguments": arguments
                    })),
                    StreamFrame::json(json!({
                        "type": "response.output_item.done",
                        "output_index": output_index,
                        "item": item
                    })),
                ]
            }
        }
    }

    fn message_delta(&mut self, input: &Value) -> Vec<StreamFrame> {
        if let Some(reason) = input.pointer("/delta/stop_reason").and_then(Value::as_str) {
            self.stop_reason = Some(reason.to_string());
        }
        if let Some(usage) = input.get("usage") {
            self.usage = Some(usage.clone());
        }
        Vec::new()
    }

    fn complete(&mut self) -> Vec<StreamFrame> {
        let mut frames = Vec::new();
        let indexes = self.blocks.keys().copied().collect::<Vec<_>>();
        for index in indexes {
            frames.extend(self.finalize_block(index));
        }
        let mut output = self.output_items.clone();
        output.sort_by_key(|(index, _)| *index);
        let output = Value::Array(output.into_iter().map(|(_, item)| item).collect());
        let usage = transforms::openai_responses_usage_from_anthropic_usage(self.usage.as_ref());
        if self.stop_reason.is_none() {
            let mut response = self.response_snapshot("failed");
            response["output"] = output;
            response["usage"] = usage;
            response["error"] = json!({
                "type": "upstream_error",
                "code": "stream_incomplete",
                "message": "Anthropic stream ended before message_delta.stop_reason"
            });
            frames.push(StreamFrame::json(json!({
                "type": "response.failed",
                "response": response
            })));
            frames.push(StreamFrame::done());
            self.completed = true;
            return frames;
        }
        let incomplete_reason = match self.stop_reason.as_deref() {
            Some("refusal") => Some("content_filter"),
            Some("max_tokens" | "model_context_window_exceeded") => Some("max_output_tokens"),
            _ => None,
        };
        let incomplete = incomplete_reason.is_some();
        let status = if incomplete {
            "incomplete"
        } else {
            "completed"
        };
        let mut response = self.response_snapshot(status);
        response["output"] = output;
        response["usage"] = usage;
        if let Some(reason) = incomplete_reason {
            response["incomplete_details"] = json!({"reason": reason});
        }
        frames.push(StreamFrame::json(json!({
            "type": if incomplete {"response.incomplete"} else {"response.completed"},
            "response": response
        })));
        frames.push(StreamFrame::done());
        self.completed = true;
        frames
    }

    fn fail(&mut self, input: &Value) -> Vec<StreamFrame> {
        let error = input.get("error").cloned().unwrap_or_else(|| input.clone());
        let mut response = self.response_snapshot("failed");
        response["error"] = error;
        self.completed = true;
        vec![
            StreamFrame::json(json!({"type": "response.failed", "response": response})),
            StreamFrame::done(),
        ]
    }

    fn response_snapshot(&self, status: &str) -> Value {
        json!({
            "id": if self.response_id.is_empty() {"resp_ccswitch"} else {self.response_id.as_str()},
            "object": "response",
            "status": status,
            "model": self.model,
            "output": [],
            "usage": Value::Null
        })
    }

    fn allocate_output_index(&mut self) -> u64 {
        let index = self.next_output_index;
        self.next_output_index = self.next_output_index.saturating_add(1);
        index
    }
}

fn input_json_delta(index: u64, arguments: &str) -> StreamFrame {
    StreamFrame::event(
        "content_block_delta",
        json!({
            "type": "content_block_delta",
            "index": index,
            "delta": {"type": "input_json_delta", "partial_json": arguments}
        }),
    )
}

fn content_block_stop(index: u64) -> StreamFrame {
    StreamFrame::event(
        "content_block_stop",
        json!({"type": "content_block_stop", "index": index}),
    )
}

fn protocol_error(kind: &'static str) {
    crate::metrics::record_stream_transform_protocol_error(kind);
    tracing::debug!(kind, "ignoring invalid upstream stream lifecycle event");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn responses_transformer() -> StreamEventTransformer {
        StreamEventTransformer {
            upstream: Some(UpstreamFormat::OpenAiResponses),
            downstream: UpstreamFormat::AnthropicMessages,
            buffer: Vec::new(),
            responses_tool_context: Default::default(),
            bridge: Some(StreamBridgeState::ResponsesAnthropic(
                ResponsesAnthropicState::default(),
            )),
        }
    }

    #[test]
    fn event_boundary_supports_lf_and_crlf() {
        assert_eq!(next_event_boundary(b"data: {}\n\nrest"), Some((8, 2)));
        assert_eq!(next_event_boundary(b"data: {}\r\n\r\nrest"), Some((8, 4)));
        assert_eq!(next_event_boundary(b"data: {}\n"), None);
    }

    #[test]
    fn complete_single_line_data_frames_do_not_wait_for_eof() {
        let mut transformer = responses_transformer();
        let output = transformer
            .push(Bytes::from_static(
                b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"hi\"}\n",
            ))
            .unwrap();
        assert!(String::from_utf8_lossy(&output).contains("text_delta"));

        let mut split = responses_transformer();
        assert!(split
            .push(Bytes::from_static(
                b"data: {\"type\":\"response.output_text.delta\",\"delta\":\""
            ))
            .unwrap()
            .is_empty());
        assert!(!split
            .push(Bytes::from_static(b"hi\"}\n"))
            .unwrap()
            .is_empty());
    }

    #[test]
    fn stream_event_buffer_enforces_per_event_byte_bound() {
        let mut complete = responses_transformer();
        complete.buffer = b"123456789\n\n".to_vec();
        let error = complete
            .drain_complete_events_with_limit(false, 8)
            .unwrap_err();
        assert_eq!(error.status, axum::http::StatusCode::PAYLOAD_TOO_LARGE);

        let mut fragmented_delimiter = responses_transformer();
        fragmented_delimiter.buffer = b"abcdefgh\r\n\r".to_vec();
        assert!(fragmented_delimiter
            .drain_complete_events_with_limit(false, 8)
            .is_ok());
        let error = fragmented_delimiter
            .drain_complete_events_with_limit(true, 8)
            .unwrap_err();
        assert_eq!(error.status, axum::http::StatusCode::PAYLOAD_TOO_LARGE);

        let mut pending = responses_transformer();
        pending.buffer = b"123456789".to_vec();
        let error = pending
            .drain_complete_events_with_limit(false, 8)
            .unwrap_err();
        assert_eq!(error.status, axum::http::StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[test]
    fn responses_parallel_tools_preserve_packed_done_arguments() {
        let mut state = ResponsesAnthropicState::default();
        let first = state.transform(&json!({
            "type": "response.output_item.added", "output_index": 3,
            "item": {"type": "function_call", "call_id": "a", "name": "first"}
        }));
        let second = state.transform(&json!({
            "type": "response.output_item.added", "output_index": 7,
            "item": {"type": "function_call", "call_id": "b", "name": "second"}
        }));
        let packed = state.transform(&json!({
            "type": "response.function_call_arguments.done", "output_index": 7,
            "arguments": "{\"value\":2}"
        }));
        let done = state.transform(&json!({
            "type": "response.output_item.done", "output_index": 7,
            "item": {"type": "function_call", "call_id": "b", "name": "second", "arguments": "{\"value\":2}"}
        }));

        assert_eq!(
            first
                .iter()
                .find(|frame| frame.payload_json()["type"] == "content_block_start")
                .unwrap()
                .payload_json()["index"],
            json!(0)
        );
        assert_eq!(second[0].payload_json()["index"], json!(1));
        assert_eq!(packed[0].payload_json()["index"], json!(1));
        assert_eq!(done.len(), 1, "packed arguments are emitted only by done");
        assert_eq!(done[0].payload_json()["index"], json!(1));
    }

    #[test]
    fn responses_done_does_not_duplicate_streamed_arguments() {
        let mut state = ResponsesAnthropicState::default();
        state.transform(&json!({
            "type": "response.output_item.added", "output_index": 0,
            "item": {"type": "function_call", "call_id": "a", "name": "first"}
        }));
        state.transform(&json!({
            "type": "response.function_call_arguments.delta", "output_index": 0,
            "delta": "{\"value\":"
        }));
        let packed = state.transform(&json!({
            "type": "response.function_call_arguments.done", "output_index": 0,
            "arguments": "{\"value\":1}"
        }));
        let done = state.transform(&json!({
            "type": "response.output_item.done", "output_index": 0,
            "item": {"type": "function_call", "arguments": "{\"value\":1}"}
        }));
        assert!(packed.is_empty());
        assert_eq!(done.len(), 1);
        assert_eq!(done[0].payload_json()["type"], json!("content_block_stop"));
    }

    #[test]
    fn responses_buffers_tool_arguments_until_item_identity_arrives() {
        let mut state = ResponsesAnthropicState::default();
        assert!(state
            .transform(&json!({
                "type": "response.function_call_arguments.delta",
                "output_index": 4,
                "delta": "{\"value\":"
            }))
            .is_empty());
        assert!(state
            .transform(&json!({
                "type": "response.function_call_arguments.done",
                "output_index": 4,
                "arguments": "{\"value\":1}"
            }))
            .is_empty());
        let opened = state.transform(&json!({
            "type": "response.output_item.added",
            "output_index": 4,
            "item": {"type": "function_call", "call_id": "call_4", "name": "lookup"}
        }));

        assert_eq!(opened.len(), 3);
        assert_eq!(
            opened[1].payload_json()["type"],
            json!("content_block_start")
        );
        assert_eq!(
            opened[2].payload_json()["delta"]["partial_json"],
            json!("{\"value\":1}")
        );
    }

    #[test]
    fn responses_closes_text_before_starting_tool_block() {
        let mut state = ResponsesAnthropicState::default();
        state.transform(&json!({
            "type": "response.output_text.delta",
            "output_index": 0,
            "delta": "answer"
        }));
        let frames = state.transform(&json!({
            "type": "response.output_item.added",
            "output_index": 1,
            "item": {"type": "function_call", "call_id": "call_1", "name": "lookup"}
        }));

        assert_eq!(frames[0].payload_json()["type"], "content_block_stop");
        assert_eq!(frames[0].payload_json()["index"], 0);
        assert_eq!(frames[1].payload_json()["type"], "content_block_start");
        assert_eq!(frames[1].payload_json()["index"], 1);
        assert_eq!(
            frames[1].payload_json()["content_block"]["type"],
            "tool_use"
        );
    }

    #[test]
    fn responses_custom_tool_input_is_wrapped_and_emitted_once() {
        let mut state = ResponsesAnthropicState::default();
        state.transform(&json!({
            "type": "response.output_item.added",
            "output_index": 2,
            "item": {"type": "custom_tool_call", "call_id": "call_exec", "name": "exec"}
        }));
        assert!(state
            .transform(&json!({
                "type": "response.custom_tool_call_input.delta",
                "output_index": 2,
                "delta": "pwd"
            }))
            .is_empty());
        let packed = state.transform(&json!({
            "type": "response.custom_tool_call_input.done",
            "output_index": 2,
            "input": "pwd"
        }));
        let done = state.transform(&json!({
            "type": "response.output_item.done",
            "output_index": 2,
            "item": {
                "type": "custom_tool_call",
                "call_id": "call_exec",
                "name": "exec",
                "input": "pwd"
            }
        }));

        assert_eq!(packed.len(), 1);
        assert_eq!(
            packed[0].payload_json()["delta"]["partial_json"],
            json!("{\"input\":\"pwd\"}")
        );
        assert_eq!(done.len(), 1);
        assert_eq!(done[0].payload_json()["type"], "content_block_stop");
    }

    #[test]
    fn responses_packed_terminal_replays_text_reasoning_and_tools() {
        let mut state = ResponsesAnthropicState::default();
        let frames = state.transform(&json!({
            "type": "response.completed",
            "response": {
                "id": "resp_packed",
                "model": "gpt-packed",
                "status": "completed",
                "output": [
                    {
                        "id": "rs_packed",
                        "type": "reasoning",
                        "summary": [{"type": "summary_text", "text": "check"}]
                    },
                    {
                        "id": "msg_packed",
                        "type": "message",
                        "role": "assistant",
                        "content": [{"type": "output_text", "text": "answer"}]
                    },
                    {
                        "id": "fc_packed",
                        "type": "function_call",
                        "call_id": "call_lookup",
                        "name": "lookup",
                        "arguments": "{\"value\":1}"
                    },
                    {
                        "id": "ctc_packed",
                        "type": "custom_tool_call",
                        "call_id": "call_exec",
                        "name": "exec",
                        "input": "pwd"
                    }
                ],
                "usage": {"input_tokens": 3, "output_tokens": 4}
            }
        }));

        let starts = frames
            .iter()
            .filter(|frame| frame.payload_json()["type"] == "content_block_start")
            .map(|frame| frame.payload_json()["content_block"].clone())
            .collect::<Vec<_>>();
        assert_eq!(starts.len(), 4);
        assert_eq!(starts[0]["type"], "thinking");
        assert_eq!(starts[0]["thinking"], "check");
        assert_eq!(starts[1]["type"], "text");
        assert_eq!(starts[2]["name"], "lookup");
        assert_eq!(starts[3]["name"], "exec");
        let argument_deltas = frames
            .iter()
            .filter_map(|frame| {
                if frame
                    .payload_json()
                    .pointer("/delta/type")
                    .and_then(Value::as_str)
                    == Some("input_json_delta")
                {
                    Some(frame.payload_json()["delta"]["partial_json"].clone())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        assert_eq!(
            argument_deltas,
            vec![json!("{\"value\":1}"), json!("{\"input\":\"pwd\"}")]
        );
        assert!(frames.iter().any(|frame| {
            frame
                .payload_json()
                .pointer("/delta/text")
                .and_then(Value::as_str)
                == Some("answer")
        }));
        assert_eq!(
            frames
                .iter()
                .filter(|frame| frame.payload_json()["type"] == "content_block_stop")
                .count(),
            4
        );
        assert_eq!(
            frames
                .iter()
                .find(|frame| frame.payload_json()["type"] == "message_delta")
                .unwrap()
                .payload_json()["delta"]["stop_reason"],
            "tool_use"
        );
    }

    #[test]
    fn responses_packed_terminal_deduplicates_sparse_streamed_items() {
        let mut state = ResponsesAnthropicState::default();
        state.transform(&json!({
            "type": "response.output_item.added",
            "output_index": 9,
            "item": {"id": "fc_sparse", "type": "function_call", "call_id": "call_9", "name": "lookup"}
        }));
        state.transform(&json!({
            "type": "response.output_item.done",
            "output_index": 9,
            "item": {
                "id": "fc_sparse",
                "type": "function_call",
                "call_id": "call_9",
                "name": "lookup",
                "arguments": "{\"value\":9}"
            }
        }));
        state.transform(&json!({
            "type": "response.output_item.added",
            "output_index": 7,
            "item": {"id": "msg_sparse", "type": "message", "role": "assistant", "content": []}
        }));
        state.transform(&json!({
            "type": "response.output_text.delta",
            "output_index": 7,
            "item_id": "msg_sparse",
            "delta": "answer"
        }));

        let terminal = state.transform(&json!({
            "type": "response.completed",
            "response": {
                "status": "completed",
                "output": [
                    {
                        "id": "msg_sparse",
                        "type": "message",
                        "content": [{"type": "output_text", "text": "answer"}]
                    },
                    {
                        "id": "rs_terminal",
                        "type": "reasoning",
                        "summary": [{"type": "summary_text", "text": "late check"}]
                    },
                    {
                        "id": "fc_sparse",
                        "type": "function_call",
                        "call_id": "call_9",
                        "name": "lookup",
                        "arguments": "{\"value\":9}"
                    }
                ],
                "usage": {}
            }
        }));

        assert!(!terminal.iter().any(|frame| {
            frame
                .payload_json()
                .pointer("/delta/type")
                .and_then(Value::as_str)
                == Some("text_delta")
        }));
        assert!(!terminal.iter().any(|frame| {
            frame.payload_json()["type"] == "content_block_start"
                && frame.payload_json()["content_block"]["type"] == "tool_use"
        }));
        let text_stop = terminal
            .iter()
            .position(|frame| frame.payload_json()["type"] == "content_block_stop")
            .unwrap();
        let reasoning_start = terminal
            .iter()
            .position(|frame| {
                frame.payload_json()["type"] == "content_block_start"
                    && frame.payload_json()["content_block"]["type"] == "thinking"
            })
            .unwrap();
        assert!(text_stop < reasoning_start);
    }

    #[test]
    fn empty_anthropic_stream_emits_a_balanced_text_block_and_null_start_state() {
        let mut responses = ResponsesAnthropicState::default();
        let start = responses.transform(&json!({
            "type": "response.created",
            "response": {"id": "resp-empty", "model": "empty-model"}
        }));
        let done = responses.transform(&json!({
            "type": "response.completed",
            "response": {"status": "completed", "usage": {"output_tokens": 0}}
        }));

        assert_eq!(
            start[0].payload_json()["message"]["stop_reason"],
            Value::Null
        );
        assert_eq!(
            start[0].payload_json()["message"]["stop_sequence"],
            Value::Null
        );
        assert_eq!(done[0].payload_json()["type"], json!("content_block_start"));
        assert_eq!(done[0].payload_json()["content_block"]["text"], json!(""));
        assert_eq!(done[1].payload_json()["type"], json!("content_block_stop"));
        assert_eq!(
            done.last().unwrap().payload_json()["type"],
            json!("message_stop")
        );

        let mut chat = ChatAnthropicState::default();
        let frames = chat.transform(&json!({
            "id": "chatcmpl-empty",
            "model": "empty-model",
            "choices": [{"delta": {}, "finish_reason": "stop"}]
        }));
        assert_eq!(
            frames[0].payload_json()["message"]["stop_reason"],
            Value::Null
        );
        assert!(frames
            .iter()
            .any(|frame| frame.payload_json()["type"] == "content_block_start"));
        assert!(frames
            .iter()
            .any(|frame| frame.payload_json()["type"] == "content_block_stop"));
    }

    #[test]
    fn chat_full_message_legacy_function_call_bridges_to_both_protocols() {
        let event = json!({
            "id": "chatcmpl-legacy",
            "model": "legacy-model",
            "choices": [{
                "delta": {},
                "message": {
                    "role": "assistant",
                    "content": null,
                    "function_call": {
                        "name": "get_weather",
                        "arguments": "{\"city\":\"Tokyo\"}"
                    }
                },
                "finish_reason": "function_call"
            }]
        });

        let anthropic = ChatAnthropicState::default().transform(&event);
        assert!(anthropic.iter().any(|frame| {
            frame.payload_json()["content_block"]["type"] == "tool_use"
                && frame.payload_json()["content_block"]["name"] == "get_weather"
        }));
        assert!(anthropic.iter().any(|frame| {
            frame.payload_json()["delta"]["partial_json"] == "{\"city\":\"Tokyo\"}"
        }));

        let mut responses_state = ChatResponsesState::new(BTreeSet::new());
        let mut responses = responses_state.transform(&event);
        responses.extend(responses_state.finish_stream());
        assert!(responses.iter().any(|frame| {
            frame.payload_json()["item"]["type"] == "function_call"
                && frame.payload_json()["item"]["name"] == "get_weather"
        }));
        let completed = responses
            .iter()
            .find(|frame| frame.payload_json()["type"] == "response.completed")
            .unwrap();
        assert_eq!(
            completed.payload_json()["response"]["output"][0]["arguments"],
            json!("{\"city\":\"Tokyo\"}")
        );
    }

    #[test]
    fn chat_full_message_refusal_remains_visible_in_stream_bridges() {
        let event = json!({
            "id": "chatcmpl-refusal",
            "model": "guarded-model",
            "choices": [{
                "delta": {},
                "message": {
                    "role": "assistant",
                    "content": [{"type": "refusal", "refusal": "part refusal"}],
                    "refusal": "message refusal"
                },
                "finish_reason": "content_filter"
            }]
        });

        let anthropic = ChatAnthropicState::default().transform(&event);
        let anthropic_text = anthropic
            .iter()
            .filter_map(|frame| frame.payload_json()["delta"]["text"].as_str())
            .collect::<Vec<_>>();
        assert_eq!(anthropic_text, vec!["part refusal", "message refusal"]);

        let mut responses_state = ChatResponsesState::new(BTreeSet::new());
        let responses = responses_state.transform(&event);
        let responses_text = responses
            .iter()
            .filter_map(|frame| frame.payload_json()["delta"].as_str())
            .collect::<Vec<_>>();
        assert_eq!(responses_text, vec!["part refusal", "message refusal"]);
    }

    #[test]
    fn framing_is_stable_across_every_chunk_boundary_and_crlf() {
        let wire = concat!(
            "data: {\"type\":\"response.created\",\"response\":{\"id\":\"r\",\"model\":\"m\"}}\r\n\r\n",
            "data: {\"type\":\"response.output_item.added\",\"output_index\":4,\"item\":{\"type\":\"function_call\",\"call_id\":\"c\",\"name\":\"lookup\"}}\r\n\r\n",
            "data: {\"type\":\"response.function_call_arguments.done\",\"output_index\":4,\"arguments\":\"{\\\"q\\\":1}\"}\r\n\r\n",
            "data: {\"type\":\"response.output_item.done\",\"output_index\":4,\"item\":{\"type\":\"function_call\",\"call_id\":\"c\",\"name\":\"lookup\",\"arguments\":\"{\\\"q\\\":1}\"}}\r\n\r\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"usage\":{\"input_tokens\":3,\"output_tokens\":2}}}\r\n\r\n",
            "data: [DONE]\r\n\r\n"
        );
        let mut baseline = responses_transformer();
        let expected = baseline.push(Bytes::from_static(wire.as_bytes())).unwrap();
        assert_eq!(
            String::from_utf8_lossy(&expected)
                .matches("{\\\"q\\\":1}")
                .count(),
            1
        );

        for split in 1..wire.len() {
            let mut transformer = responses_transformer();
            let first = transformer
                .push(Bytes::copy_from_slice(&wire.as_bytes()[..split]))
                .unwrap();
            let second = transformer
                .push(Bytes::copy_from_slice(&wire.as_bytes()[split..]))
                .unwrap();
            let tail = transformer.finish().unwrap();
            assert_eq!(
                join_test_bytes(&[first, second, tail]),
                expected,
                "split={split}"
            );
        }
    }

    #[test]
    fn eof_half_json_is_a_protocol_error() {
        let mut transformer = responses_transformer();
        assert!(transformer
            .push(Bytes::from_static(b"data: {\"type\":\"response.created\""))
            .unwrap()
            .is_empty());
        let error = transformer.finish().unwrap_err();
        assert!(error.message.contains("not valid JSON"));
    }

    #[test]
    fn eof_without_terminal_event_is_a_protocol_error() {
        let mut transformer = responses_transformer();
        let output = transformer
            .push(Bytes::from_static(
                b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"hi\"}\n\n",
            ))
            .unwrap();
        assert!(String::from_utf8_lossy(&output).contains("text_delta"));

        let error = transformer.finish().unwrap_err();
        assert!(error.message.contains("before a terminal event"));
    }

    #[test]
    fn chat_to_responses_waits_for_earlier_parallel_tool_identity() {
        let mut state = ChatResponsesState::new(BTreeSet::new());
        let first = state.transform(&json!({
            "id": "chatcmpl_parallel",
            "model": "reasoner",
            "choices": [{"delta": {"tool_calls": [{
                "index": 0,
                "id": "call_first",
                "function": {"name": "", "arguments": "{"}
            }]}}]
        }));
        let second = state.transform(&json!({
            "choices": [{"delta": {"tool_calls": [{
                "index": 1,
                "id": "call_second",
                "function": {"name": "second", "arguments": "{\"value\":2}"}
            }]}}]
        }));
        assert!(!first.iter().any(is_output_item_added));
        assert!(!second.iter().any(is_output_item_added));

        let ready = state.transform(&json!({
            "choices": [{
                "delta": {"tool_calls": [{
                    "index": 0,
                    "function": {"name": "first", "arguments": "\"value\":1}"}
                }]},
                "finish_reason": "tool_calls"
            }]
        }));
        let added = ready
            .iter()
            .filter(|frame| is_output_item_added(frame))
            .map(|frame| frame.payload_json()["item"]["name"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(added, vec!["first", "second"]);

        let done = state.finish_stream();
        let completed = done
            .iter()
            .find(|frame| frame.payload_json()["type"] == "response.completed")
            .unwrap();
        assert_eq!(
            completed.payload_json()["response"]["output"][0]["name"],
            "first"
        );
        assert_eq!(
            completed.payload_json()["response"]["output"][1]["name"],
            "second"
        );
    }

    #[test]
    fn chat_content_filter_finishes_as_failed_response() {
        let mut state = ChatResponsesState::new(BTreeSet::new());
        state.transform(&json!({
            "id": "chatcmpl_filtered",
            "model": "guarded",
            "choices": [{
                "delta": {"content": "partial"},
                "finish_reason": "content_filter"
            }]
        }));
        let frames = state.finish_stream();
        let failed = frames
            .iter()
            .find(|frame| frame.payload_json()["type"] == "response.failed")
            .unwrap();

        assert_eq!(failed.payload_json()["response"]["status"], "failed");
        assert_eq!(
            failed.payload_json()["response"]["error"]["code"],
            "content_filter"
        );
    }

    #[test]
    fn chat_done_without_finish_reason_finishes_as_failed_response() {
        let mut state = ChatResponsesState::new(BTreeSet::new());
        state.transform(&json!({
            "id": "chatcmpl_truncated",
            "model": "chat",
            "choices": [{"delta": {"content": "partial"}, "finish_reason": null}]
        }));

        let frames = state.finish_stream();
        let failed = frames
            .iter()
            .find(|frame| frame.payload_json()["type"] == "response.failed")
            .unwrap();

        assert_eq!(failed.payload_json()["response"]["status"], "failed");
        assert_eq!(
            failed.payload_json()["response"]["error"]["code"],
            "stream_incomplete"
        );
        assert_eq!(
            failed.payload_json()["response"]["output"][0]["content"][0]["text"],
            "partial"
        );
        assert!(!frames.iter().any(|frame| matches!(
            &frame.payload,
            transforms::StreamPayload::Json(payload)
                if matches!(
                    payload["type"].as_str(),
                    Some("response.completed" | "response.incomplete")
                )
        )));
    }

    #[test]
    fn anthropic_message_stop_without_stop_reason_finishes_as_failed_response() {
        let mut state = AnthropicResponsesState::default();
        state.transform(&json!({
            "type": "message_start",
            "message": {"id": "msg_truncated", "model": "claude", "usage": {"input_tokens": 2}}
        }));
        state.transform(&json!({
            "type": "content_block_start",
            "index": 0,
            "content_block": {"type": "text", "text": "partial"}
        }));

        let frames = state.transform(&json!({"type": "message_stop"}));
        let failed = frames
            .iter()
            .find(|frame| frame.payload_json()["type"] == "response.failed")
            .unwrap();

        assert_eq!(failed.payload_json()["response"]["status"], "failed");
        assert_eq!(
            failed.payload_json()["response"]["error"]["code"],
            "stream_incomplete"
        );
        assert_eq!(
            failed.payload_json()["response"]["output"][0]["content"][0]["text"],
            "partial"
        );
        assert!(!frames.iter().any(|frame| matches!(
            &frame.payload,
            transforms::StreamPayload::Json(payload)
                if matches!(
                    payload["type"].as_str(),
                    Some("response.completed" | "response.incomplete")
                )
        )));
    }

    #[test]
    fn responses_to_chat_preserves_multiple_packed_text_and_reasoning_items() {
        let mut state = ResponsesChatState::default();
        state.transform(&json!({
            "type": "response.output_item.added",
            "output_index": 4,
            "item": {"id": "rs_first", "type": "reasoning"}
        }));
        let mut frames = state.transform(&json!({
            "type": "response.reasoning_summary_text.delta",
            "item_id": "rs_first",
            "output_index": 4,
            "delta": "first thought"
        }));
        state.transform(&json!({
            "type": "response.output_item.added",
            "output_index": 8,
            "item": {"id": "msg_first", "type": "message"}
        }));
        frames.extend(state.transform(&json!({
            "type": "response.output_text.delta",
            "item_id": "msg_first",
            "output_index": 8,
            "delta": "first answer"
        })));
        frames.extend(state.transform(&json!({
            "type": "response.completed",
            "response": {
                "status": "completed",
                "output": [
                    {"id": "rs_first", "type": "reasoning", "summary": [{"type": "summary_text", "text": "first thought"}]},
                    {"id": "rs_second", "type": "reasoning", "summary": [{"type": "summary_text", "text": "second thought"}]},
                    {"id": "msg_first", "type": "message", "content": [{"type": "output_text", "text": "first answer"}]},
                    {"id": "msg_second", "type": "message", "content": [{"type": "output_text", "text": "second answer"}]}
                ]
            }
        })));

        let reasoning = frames
            .iter()
            .filter_map(|frame| match &frame.payload {
                transforms::StreamPayload::Json(payload) => payload
                    .pointer("/choices/0/delta/reasoning_content")
                    .and_then(Value::as_str),
                transforms::StreamPayload::Done => None,
            })
            .collect::<Vec<_>>();
        let text = frames
            .iter()
            .filter_map(|frame| match &frame.payload {
                transforms::StreamPayload::Json(payload) => payload
                    .pointer("/choices/0/delta/content")
                    .and_then(Value::as_str),
                transforms::StreamPayload::Done => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(reasoning, vec!["first thought", "second thought"]);
        assert_eq!(text, vec!["first answer", "second answer"]);
    }

    #[test]
    fn responses_to_chat_reuses_streamed_tool_identity_in_packed_terminal() {
        let mut state = ResponsesChatState::default();
        let first = state.transform(&json!({
            "type": "response.output_item.added",
            "output_index": 7,
            "item": {"id": "fc_7", "type": "function_call", "call_id": "call_7", "name": "lookup"}
        }));
        let terminal = state.transform(&json!({
            "type": "response.completed",
            "response": {
                "status": "completed",
                "output": [{
                    "id": "fc_7",
                    "type": "function_call",
                    "call_id": "call_7",
                    "name": "lookup",
                    "arguments": "{}"
                }]
            }
        }));

        assert_eq!(
            first
                .iter()
                .chain(&terminal)
                .filter(|frame| {
                    matches!(
                        &frame.payload,
                        transforms::StreamPayload::Json(payload)
                            if payload.pointer("/choices/0/delta/tool_calls").is_some()
                    )
                })
                .count(),
            2,
            "one identity chunk and one arguments chunk are expected"
        );
        assert_eq!(
            first
                .iter()
                .chain(&terminal)
                .filter(|frame| {
                    matches!(
                        &frame.payload,
                        transforms::StreamPayload::Json(payload)
                            if payload.pointer("/choices/0/delta/tool_calls/0/id").is_some()
                    )
                })
                .count(),
            1,
            "packed terminal output must not reopen the tool"
        );
    }

    #[test]
    fn responses_content_filter_maps_to_anthropic_refusal_and_chat_filter() {
        let terminal = json!({
            "type": "response.incomplete",
            "response": {
                "status": "incomplete",
                "incomplete_details": {"reason": "content_filter"},
                "output": [],
                "usage": {}
            }
        });
        let anthropic = ResponsesAnthropicState::default().transform(&terminal);
        let chat = ResponsesChatState::default().transform(&terminal);

        assert!(anthropic.iter().any(|frame| {
            frame
                .payload_json()
                .pointer("/delta/stop_reason")
                .and_then(Value::as_str)
                == Some("refusal")
        }));
        assert!(chat.iter().any(|frame| {
            frame
                .payload_json()
                .pointer("/choices/0/finish_reason")
                .and_then(Value::as_str)
                == Some("content_filter")
        }));
    }

    #[test]
    fn responses_incomplete_reason_wins_over_partial_tool_output() {
        for (reason, anthropic_stop, chat_finish) in [
            ("content_filter", "refusal", "content_filter"),
            ("max_output_tokens", "max_tokens", "length"),
        ] {
            let terminal = json!({
                "type": "response.incomplete",
                "response": {
                    "status": "incomplete",
                    "incomplete_details": {"reason": reason},
                    "output": [{
                        "type": "function_call",
                        "call_id": "call_partial",
                        "name": "lookup",
                        "arguments": "{"
                    }],
                    "usage": {}
                }
            });

            let anthropic = ResponsesAnthropicState::default().transform(&terminal);
            let chat = ResponsesChatState::default().transform(&terminal);

            assert!(anthropic.iter().any(|frame| {
                frame
                    .payload_json()
                    .pointer("/delta/stop_reason")
                    .and_then(Value::as_str)
                    == Some(anthropic_stop)
            }));
            assert!(chat.iter().any(|frame| {
                frame
                    .payload_json()
                    .pointer("/choices/0/finish_reason")
                    .and_then(Value::as_str)
                    == Some(chat_finish)
            }));
        }
    }

    #[test]
    fn anthropic_refusal_maps_to_responses_content_filter() {
        let mut state = AnthropicResponsesState::default();
        state.transform(&json!({
            "type": "message_start",
            "message": {"id": "msg_refused", "model": "claude"}
        }));
        state.transform(&json!({
            "type": "message_delta",
            "delta": {"stop_reason": "refusal"},
            "usage": {"output_tokens": 0}
        }));
        let frames = state.transform(&json!({"type": "message_stop"}));
        let incomplete = frames
            .iter()
            .find(|frame| frame.payload_json()["type"] == "response.incomplete")
            .unwrap();
        assert_eq!(
            incomplete.payload_json()["response"]["incomplete_details"]["reason"],
            "content_filter"
        );
    }

    #[test]
    fn anthropic_custom_tool_stream_restores_native_responses_events() {
        let mut state = AnthropicResponsesState::new(BTreeSet::from(["exec".to_string()]));
        let added = state.transform(&json!({
            "type": "content_block_start",
            "index": 0,
            "content_block": {
                "type": "tool_use",
                "id": "call_exec",
                "name": "exec",
                "input": {}
            }
        }));
        assert_eq!(
            added
                .iter()
                .find(|frame| frame.payload_json()["type"] == "response.output_item.added")
                .unwrap()
                .payload_json()["item"]["type"],
            "custom_tool_call"
        );

        assert!(state
            .transform(&json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": {"type": "input_json_delta", "partial_json": "{\"input\":\"pwd\"}"}
            }))
            .is_empty());
        let done = state.transform(&json!({"type": "content_block_stop", "index": 0}));

        assert_eq!(
            done[0].payload_json()["type"],
            "response.custom_tool_call_input.delta"
        );
        assert_eq!(done[0].payload_json()["delta"], "pwd");
        assert_eq!(
            done[1].payload_json()["type"],
            "response.custom_tool_call_input.done"
        );
        assert_eq!(done[1].payload_json()["input"], "pwd");
        assert_eq!(done[2].payload_json()["item"]["type"], "custom_tool_call");
        assert_eq!(done[2].payload_json()["item"]["input"], "pwd");
    }

    #[test]
    fn anthropic_stream_restores_namespace_and_tool_search_items() {
        let context = transforms::responses_tool_context(&json!({
            "tools": [
                {"type": "tool_search"},
                {
                    "type": "namespace",
                    "name": "mcp_mail",
                    "tools": [{
                        "type": "function",
                        "name": "search",
                        "parameters": {"type": "object", "properties": {}}
                    }]
                }
            ]
        }));
        let mut state = AnthropicResponsesState::new(context);
        let mut frames = state.transform(&json!({
            "type": "message_start",
            "message": {"id": "msg_tools", "model": "claude", "usage": {"input_tokens": 1}}
        }));
        frames.extend(state.transform(&json!({
            "type": "content_block_start",
            "index": 0,
            "content_block": {"type": "tool_use", "id": "call_mail", "name": "mcp_mail__search", "input": {}}
        })));
        frames.extend(state.transform(&json!({
            "type": "content_block_delta",
            "index": 0,
            "delta": {"type": "input_json_delta", "partial_json": "{\"query\":\"status\"}"}
        })));
        frames.extend(state.transform(&json!({"type": "content_block_stop", "index": 0})));
        frames.extend(state.transform(&json!({
            "type": "content_block_start",
            "index": 1,
            "content_block": {"type": "tool_use", "id": "call_search", "name": "tool_search", "input": {}}
        })));
        frames.extend(state.transform(&json!({
            "type": "content_block_delta",
            "index": 1,
            "delta": {"type": "input_json_delta", "partial_json": "{\"query\":\"mail tools\"}"}
        })));
        frames.extend(state.transform(&json!({"type": "content_block_stop", "index": 1})));
        frames.extend(state.transform(&json!({
            "type": "message_delta",
            "delta": {"stop_reason": "tool_use"},
            "usage": {"output_tokens": 2}
        })));
        frames.extend(state.transform(&json!({"type": "message_stop"})));

        let done_items = frames
            .iter()
            .filter_map(|frame| match &frame.payload {
                transforms::StreamPayload::Json(payload)
                    if payload["type"] == "response.output_item.done" =>
                {
                    Some(payload["item"].clone())
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        let namespace_item = done_items
            .iter()
            .find(|item| item.get("namespace").is_some())
            .unwrap();
        assert_eq!(namespace_item["name"], "search");
        assert_eq!(namespace_item["namespace"], "mcp_mail");
        assert_eq!(namespace_item["arguments"], "{\"query\":\"status\"}");

        let tool_search_item = done_items
            .iter()
            .find(|item| item["type"] == "tool_search_call")
            .unwrap();
        assert_eq!(tool_search_item["call_id"], "call_search");
        assert_eq!(
            tool_search_item["arguments"],
            json!({"query": "mail tools"})
        );
        assert!(frames.iter().any(|frame| matches!(
            &frame.payload,
            transforms::StreamPayload::Json(payload)
                if payload["type"] == "response.function_call_arguments.delta"
        )));
    }

    #[test]
    fn responses_to_chat_keeps_streamed_custom_input_when_done_is_empty() {
        let mut state = ResponsesChatState::default();
        state.transform(&json!({
            "type": "response.output_item.added",
            "output_index": 0,
            "item": {
                "type": "custom_tool_call",
                "call_id": "call_exec",
                "name": "exec",
                "input": ""
            }
        }));
        state.transform(&json!({
            "type": "response.custom_tool_call_input.delta",
            "output_index": 0,
            "delta": "pwd"
        }));
        let frames = state.transform(&json!({
            "type": "response.custom_tool_call_input.done",
            "output_index": 0,
            "input": ""
        }));

        assert!(frames.iter().any(|frame| {
            frame
                .payload_json()
                .pointer("/choices/0/delta/tool_calls/0/function/arguments")
                .and_then(Value::as_str)
                == Some(r#"{"input":"pwd"}"#)
        }));
    }

    #[test]
    fn responses_to_chat_keeps_streamed_custom_input_when_packed_item_is_empty() {
        let mut state = ResponsesChatState::default();
        state.transform(&json!({
            "type": "response.output_item.added",
            "output_index": 0,
            "item": {
                "type": "custom_tool_call",
                "call_id": "call_exec",
                "name": "exec",
                "input": ""
            }
        }));
        state.transform(&json!({
            "type": "response.custom_tool_call_input.delta",
            "output_index": 0,
            "delta": "pwd"
        }));
        let frames = state.transform(&json!({
            "type": "response.output_item.done",
            "output_index": 0,
            "item": {
                "type": "custom_tool_call",
                "call_id": "call_exec",
                "name": "exec",
                "input": ""
            }
        }));

        assert!(frames.iter().any(|frame| {
            frame
                .payload_json()
                .pointer("/choices/0/delta/tool_calls/0/function/arguments")
                .and_then(Value::as_str)
                == Some(r#"{"input":"pwd"}"#)
        }));
    }

    #[test]
    fn responses_to_anthropic_keeps_streamed_custom_input_when_done_is_empty() {
        let mut state = ResponsesAnthropicState::default();
        state.transform(&json!({
            "type": "response.output_item.added",
            "output_index": 0,
            "item": {
                "type": "custom_tool_call",
                "call_id": "call_exec",
                "name": "exec",
                "input": ""
            }
        }));
        state.transform(&json!({
            "type": "response.custom_tool_call_input.delta",
            "output_index": 0,
            "delta": "pwd"
        }));
        let frames = state.transform(&json!({
            "type": "response.custom_tool_call_input.done",
            "output_index": 0,
            "input": ""
        }));

        assert!(frames.iter().any(|frame| {
            frame
                .payload_json()
                .pointer("/delta/partial_json")
                .and_then(Value::as_str)
                == Some(r#"{"input":"pwd"}"#)
        }));
    }

    #[test]
    fn chat_null_error_field_is_not_a_terminal_failure() {
        let mut state = ChatResponsesState::new(BTreeSet::new());
        let frames = state.transform(&json!({
            "id": "chatcmpl_null_error",
            "model": "chat",
            "error": null,
            "choices": [{"delta": {"content": "ok"}, "finish_reason": null}]
        }));

        assert!(frames
            .iter()
            .any(|frame| frame.payload_json()["type"] == "response.output_text.delta"));
        assert!(!frames
            .iter()
            .any(|frame| frame.payload_json()["type"] == "response.failed"));
    }

    #[test]
    fn responses_to_chat_uses_dense_tool_indexes_after_reasoning_items() {
        let mut state = ResponsesChatState::default();
        state.transform(&json!({
            "type": "response.reasoning_summary_text.delta",
            "output_index": 0,
            "delta": "think"
        }));
        let frames = state.transform(&json!({
            "type": "response.output_item.added",
            "output_index": 7,
            "item": {"type": "function_call", "call_id": "call_7", "name": "lookup"}
        }));
        let tool = frames
            .iter()
            .find(|frame| {
                frame
                    .payload_json()
                    .pointer("/choices/0/delta/tool_calls")
                    .is_some()
            })
            .unwrap();
        assert_eq!(
            tool.payload_json()["choices"][0]["delta"]["tool_calls"][0]["index"],
            0
        );
    }

    #[test]
    fn anthropic_signed_thinking_stream_gets_authenticated_responses_replay() {
        let mut state = AnthropicResponsesState::default();
        state.transform(&json!({
            "type": "message_start",
            "message": {"id": "msg_1", "model": "claude", "usage": {"input_tokens": 2}}
        }));
        state.transform(&json!({
            "type": "content_block_start",
            "index": 0,
            "content_block": {"type": "thinking", "thinking": ""}
        }));
        state.transform(&json!({
            "type": "content_block_delta",
            "index": 0,
            "delta": {"type": "thinking_delta", "thinking": "check"}
        }));
        state.transform(&json!({
            "type": "content_block_delta",
            "index": 0,
            "delta": {"type": "signature_delta", "signature": "provider-signature"}
        }));
        let done = state.transform(&json!({"type": "content_block_stop", "index": 0}));
        let item = done
            .iter()
            .find(|frame| frame.payload_json()["type"] == "response.output_item.done")
            .unwrap();
        assert!(item.payload_json()["item"]["encrypted_content"]
            .as_str()
            .is_some_and(|value| value.starts_with("ccswitch-server-reasoning-v1:")));
    }

    #[test]
    fn responses_reasoning_stream_emits_thinking_and_authenticated_signature() {
        let mut state = ResponsesAnthropicState::default();
        state.transform(&json!({
            "type": "response.created",
            "response": {"id": "resp_1", "model": "gpt"}
        }));
        let delta = state.transform(&json!({
            "type": "response.reasoning_summary_text.delta",
            "output_index": 0,
            "delta": "check"
        }));
        assert!(delta.iter().any(|frame| {
            frame
                .payload_json()
                .pointer("/delta/type")
                .and_then(Value::as_str)
                == Some("thinking_delta")
        }));
        let done = state.transform(&json!({
            "type": "response.output_item.done",
            "output_index": 0,
            "item": {
                "id": "rs_1",
                "type": "reasoning",
                "summary": [{"type": "summary_text", "text": "check"}],
                "encrypted_content": "provider-opaque"
            }
        }));
        assert!(done.iter().any(|frame| {
            frame
                .payload_json()
                .pointer("/delta/signature")
                .and_then(Value::as_str)
                .is_some_and(|value| value.starts_with("ccswitch-server-reasoning-v1:"))
        }));
    }

    #[test]
    fn responses_reasoning_only_stream_closes_before_terminal_without_empty_text() {
        let mut state = ResponsesAnthropicState::default();
        state.transform(&json!({
            "type": "response.reasoning_summary_text.delta",
            "output_index": 0,
            "delta": "check"
        }));
        let done = state.transform(&json!({
            "type": "response.completed",
            "response": {"status": "completed", "usage": {}}
        }));

        assert_eq!(done[0].payload_json()["type"], "content_block_stop");
        assert!(!done.iter().any(|frame| {
            frame.payload_json()["type"] == "content_block_start"
                && frame.payload_json()["content_block"]["type"] == "text"
        }));
        assert_eq!(done.last().unwrap().payload_json()["type"], "message_stop");
    }

    #[test]
    fn responses_multiple_reasoning_items_reopen_distinct_blocks() {
        let mut state = ResponsesAnthropicState::default();
        state.transform(&json!({
            "type": "response.reasoning_summary_text.delta",
            "output_index": 0,
            "delta": "first"
        }));
        state.transform(&json!({
            "type": "response.output_item.done",
            "output_index": 0,
            "item": {"type": "reasoning", "summary": [{"type": "summary_text", "text": "first"}]}
        }));
        let second = state.transform(&json!({
            "type": "response.reasoning_summary_text.delta",
            "output_index": 1,
            "delta": "second"
        }));

        let start = second
            .iter()
            .find(|frame| frame.payload_json()["type"] == "content_block_start")
            .unwrap();
        assert_eq!(start.payload_json()["index"], 1);
        assert_eq!(start.payload_json()["content_block"]["type"], "thinking");
    }

    #[test]
    fn chat_reasoning_closes_before_text_and_maps_length_terminal() {
        let mut state = ChatAnthropicState::default();
        let reasoning = state.transform(&json!({
            "id": "chatcmpl_reasoning",
            "model": "reasoner",
            "choices": [{"delta": {"reasoning_content": "think"}, "finish_reason": null}]
        }));
        assert!(reasoning.iter().any(|frame| {
            frame
                .payload_json()
                .pointer("/delta/type")
                .and_then(Value::as_str)
                == Some("thinking_delta")
        }));

        let text = state.transform(&json!({
            "choices": [{"delta": {"content": "answer"}, "finish_reason": "length"}]
        }));
        let reasoning_stop = text
            .iter()
            .position(|frame| frame.payload_json()["type"] == "content_block_stop")
            .unwrap();
        let text_start = text
            .iter()
            .position(|frame| {
                frame.payload_json()["type"] == "content_block_start"
                    && frame.payload_json()["content_block"]["type"] == "text"
            })
            .unwrap();
        assert!(reasoning_stop < text_start);
        assert!(text.iter().any(|frame| {
            frame
                .payload_json()
                .pointer("/delta/stop_reason")
                .and_then(Value::as_str)
                == Some("max_tokens")
        }));
    }

    #[test]
    fn chat_truncation_reason_wins_over_partial_tool_output() {
        for (finish_reason, expected_stop) in
            [("content_filter", "refusal"), ("length", "max_tokens")]
        {
            let mut state = ChatAnthropicState::default();
            state.transform(&json!({
                "id": "chatcmpl_partial_tool",
                "model": "chat",
                "choices": [{
                    "delta": {"tool_calls": [{
                        "index": 0,
                        "id": "call_partial",
                        "function": {"name": "lookup", "arguments": "{"}
                    }]},
                    "finish_reason": null
                }]
            }));

            let terminal = state.transform(&json!({
                "choices": [{"delta": {}, "finish_reason": finish_reason}]
            }));
            assert!(terminal.iter().any(|frame| {
                frame
                    .payload_json()
                    .pointer("/delta/stop_reason")
                    .and_then(Value::as_str)
                    == Some(expected_stop)
            }));
        }
    }

    #[test]
    fn chat_custom_tool_stream_unwraps_native_input_once() {
        let mut state = ChatResponsesState::new(BTreeSet::from(["exec".to_string()]));
        let first = state.transform(&json!({
            "id": "chatcmpl_custom",
            "model": "chat",
            "choices": [{"delta": {"tool_calls": [{
                "index": 0,
                "id": "call_exec",
                "function": {"name": "exec", "arguments": "{\"input\":\"p"}
            }]}}]
        }));
        assert!(first.iter().any(|frame| {
            frame
                .payload_json()
                .pointer("/item/type")
                .and_then(Value::as_str)
                == Some("custom_tool_call")
        }));
        assert!(!first.iter().any(|frame| {
            frame
                .payload_json()
                .pointer("/item/cc_switch_custom_bridge")
                .is_some()
        }));
        assert!(!first.iter().any(|frame| {
            frame.payload_json()["type"] == "response.custom_tool_call_input.delta"
        }));

        state.transform(&json!({
            "choices": [{
                "delta": {"tool_calls": [{
                    "index": 0,
                    "function": {"arguments": "wd\"}"}
                }]},
                "finish_reason": "tool_calls"
            }]
        }));
        let done = state.finish_stream();
        let deltas = done
            .iter()
            .filter_map(|frame| match &frame.payload {
                transforms::StreamPayload::Json(payload)
                    if payload["type"] == "response.custom_tool_call_input.delta" =>
                {
                    Some(payload)
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(deltas.len(), 1);
        assert_eq!(deltas[0]["delta"], "pwd");
        assert_eq!(
            done.iter()
                .filter(|frame| matches!(
                    &frame.payload,
                    transforms::StreamPayload::Json(payload)
                        if payload["type"] == "response.custom_tool_call_input.done"
                ))
                .count(),
            1
        );
        assert_eq!(
            done.iter()
                .filter(|frame| matches!(
                    &frame.payload,
                    transforms::StreamPayload::Json(payload)
                        if payload["type"] == "response.output_item.done"
                ))
                .count(),
            1
        );
        let completed = done
            .iter()
            .find(|frame| frame.payload_json()["type"] == "response.completed")
            .unwrap();
        assert_eq!(
            completed.payload_json()["response"]["output"][0]["input"],
            "pwd"
        );
    }

    #[test]
    fn chat_stream_restores_namespace_and_tool_search_items() {
        let context = transforms::responses_tool_context(&json!({
            "tools": [
                {"type": "tool_search"},
                {
                    "type": "namespace",
                    "name": "mcp_mail",
                    "tools": [{
                        "type": "function",
                        "name": "search",
                        "parameters": {"type": "object", "properties": {}}
                    }]
                }
            ]
        }));
        let mut state = ChatResponsesState::new(context);
        let mut frames = state.transform(&json!({
            "id": "chatcmpl_tools",
            "model": "chat",
            "choices": [{
                "delta": {"tool_calls": [
                    {
                        "index": 0,
                        "id": "call_mail",
                        "type": "function",
                        "function": {"name": "mcp_mail__search", "arguments": "{\"query\":\"status\"}"}
                    },
                    {
                        "index": 1,
                        "id": "call_search",
                        "type": "function",
                        "function": {"name": "tool_search", "arguments": "{\"query\":\"mail tools\"}"}
                    }
                ]},
                "finish_reason": "tool_calls"
            }]
        }));
        frames.extend(state.finish_stream());

        let done_items = frames
            .iter()
            .filter_map(|frame| match &frame.payload {
                transforms::StreamPayload::Json(payload)
                    if payload["type"] == "response.output_item.done" =>
                {
                    Some(payload["item"].clone())
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        let namespace_item = done_items
            .iter()
            .find(|item| item.get("namespace").is_some())
            .unwrap();
        assert_eq!(namespace_item["name"], "search");
        assert_eq!(namespace_item["namespace"], "mcp_mail");

        let tool_search_item = done_items
            .iter()
            .find(|item| item["type"] == "tool_search_call")
            .unwrap();
        assert_eq!(tool_search_item["call_id"], "call_search");
        assert_eq!(
            tool_search_item["arguments"],
            json!({"query": "mail tools"})
        );
        assert!(frames.iter().any(|frame| matches!(
            &frame.payload,
            transforms::StreamPayload::Json(payload)
                if payload["type"] == "response.function_call_arguments.done"
        )));
    }

    #[test]
    fn terminal_state_ignores_late_responses_events() {
        let mut state = ResponsesChatState::default();
        let terminal = state.transform(&json!({
            "type": "response.completed",
            "response": {"id": "resp_done", "status": "completed", "output": []}
        }));
        assert!(terminal
            .iter()
            .any(|frame| matches!(frame.payload, transforms::StreamPayload::Done)));
        assert!(state
            .transform(&json!({"type": "response.output_text.delta", "delta": "late"}))
            .is_empty());
    }

    #[test]
    fn proxy_bridge_contract_fixture_preserves_streaming_lifecycle_order() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/proxy_bridge/streaming_lifecycle.json"
        ))
        .unwrap();
        assert_eq!(fixture["id"], "streaming-lifecycle-ordering");
        assert_eq!(fixture["category"], "streaming_lifecycle");
        assert_eq!(fixture["source"], "open_ai_responses");
        assert_eq!(fixture["target"], "anthropic_messages");

        let mut state = ResponsesAnthropicState::default();
        let frames = fixture["events"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|event| state.transform(event))
            .collect::<Vec<_>>();
        let payloads = frames
            .iter()
            .filter_map(|frame| match &frame.payload {
                transforms::StreamPayload::Json(value) => Some(value),
                transforms::StreamPayload::Done => None,
            })
            .collect::<Vec<_>>();
        let tool_starts = payloads
            .iter()
            .filter(|payload| {
                payload["type"] == "content_block_start"
                    && payload["content_block"]["type"] == "tool_use"
            })
            .copied()
            .collect::<Vec<_>>();
        assert_eq!(
            Value::Array(
                tool_starts
                    .iter()
                    .map(|payload| payload["index"].clone())
                    .collect()
            ),
            fixture["expected"]["toolStartIndexes"]
        );
        assert_eq!(
            Value::Array(
                tool_starts
                    .iter()
                    .map(|payload| payload["content_block"]["name"].clone())
                    .collect()
            ),
            fixture["expected"]["toolNames"]
        );

        let argument_deltas = payloads
            .iter()
            .filter(|payload| {
                payload.pointer("/delta/type").and_then(Value::as_str) == Some("input_json_delta")
            })
            .copied()
            .collect::<Vec<_>>();
        assert_eq!(
            Value::Array(
                argument_deltas
                    .iter()
                    .map(|payload| payload["index"].clone())
                    .collect()
            ),
            fixture["expected"]["jsonDeltaIndexes"]
        );
        assert_eq!(
            argument_deltas
                .iter()
                .filter(|payload| {
                    payload.pointer("/delta/partial_json")
                        == Some(&fixture["expected"]["arguments"])
                })
                .count() as u64,
            fixture["expected"]["argumentOccurrences"].as_u64().unwrap()
        );

        let event_types = payloads
            .iter()
            .filter_map(|payload| payload["type"].as_str().map(str::to_string))
            .map(Value::String)
            .collect::<Vec<_>>();
        let terminal_types = fixture["expected"]["terminalTypes"].as_array().unwrap();
        assert_eq!(
            &event_types[event_types.len() - terminal_types.len()..],
            terminal_types
        );
    }

    fn join_test_bytes(chunks: &[Bytes]) -> Bytes {
        let mut result = Vec::new();
        for chunk in chunks {
            result.extend_from_slice(chunk);
        }
        Bytes::from(result)
    }

    trait FramePayloadExt {
        fn payload_json(&self) -> &Value;
    }

    impl FramePayloadExt for StreamFrame {
        fn payload_json(&self) -> &Value {
            match &self.payload {
                super::super::transforms::StreamPayload::Json(value) => value,
                super::super::transforms::StreamPayload::Done => {
                    panic!("expected JSON frame")
                }
            }
        }
    }

    fn is_output_item_added(frame: &StreamFrame) -> bool {
        frame.payload_json()["type"] == "response.output_item.added"
    }
}
