use std::collections::{BTreeMap, BTreeSet};

use bytes::Bytes;
use serde_json::{json, Value};

use crate::domain::providers::store::StoredProvider;

use super::adapters::{
    downstream_format_for_route, encode_stream_frames, transform_stream_value,
    upstream_format_for_route, UpstreamFormat,
};
use super::transforms::StreamFrame;
use super::{ProxyError, ProxyRoute};

#[derive(Debug)]
pub(super) struct StreamEventTransformer {
    upstream: Option<UpstreamFormat>,
    downstream: UpstreamFormat,
    buffer: Vec<u8>,
    custom_tool_names: BTreeSet<String>,
    anthropic_bridge: Option<AnthropicBridgeState>,
}

impl StreamEventTransformer {
    pub(super) fn new(
        stored: &StoredProvider,
        route: ProxyRoute,
        custom_tool_names: BTreeSet<String>,
    ) -> Self {
        let upstream = upstream_format_for_route(stored, Some(route), &[]);
        let downstream = downstream_format_for_route(route);
        let anthropic_bridge = match (upstream, downstream) {
            (Some(UpstreamFormat::OpenAiResponses), UpstreamFormat::AnthropicMessages) => Some(
                AnthropicBridgeState::Responses(ResponsesAnthropicState::default()),
            ),
            (Some(UpstreamFormat::OpenAiChat), UpstreamFormat::AnthropicMessages) => {
                Some(AnthropicBridgeState::Chat(ChatAnthropicState::default()))
            }
            _ => None,
        };
        Self {
            upstream,
            downstream,
            buffer: Vec::new(),
            custom_tool_names,
            anthropic_bridge,
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
        let output = self.drain_complete_events(true)?;
        if self
            .anthropic_bridge
            .as_ref()
            .is_some_and(|bridge| !bridge.completed())
        {
            crate::metrics::record_stream_transform_protocol_error("unexpected_eof");
            return Err(ProxyError::bad_gateway(
                "upstream stream ended before a terminal event",
            ));
        }
        Ok(output)
    }

    fn drain_complete_events(&mut self, finish: bool) -> Result<Bytes, ProxyError> {
        let mut output = String::new();
        while let Some((event_end, delimiter_len)) = next_event_boundary(&self.buffer) {
            let event = self.buffer[..event_end].to_vec();
            self.buffer.drain(..event_end + delimiter_len);
            output.push_str(&self.transform_event(&event)?);
        }
        while let Some(line_end) = self.buffer.iter().position(|byte| *byte == b'\n') {
            let line = self.buffer[..line_end]
                .strip_suffix(b"\r")
                .unwrap_or(&self.buffer[..line_end]);
            if !standalone_line_is_ready(line) {
                break;
            }
            let event = line.to_vec();
            self.buffer.drain(..line_end + 1);
            output.push_str(&self.transform_event(&event)?);
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
            return Ok(if self.downstream == UpstreamFormat::AnthropicMessages {
                String::new()
            } else {
                encode_stream_frames(&[StreamFrame::done()])
            });
        }
        let value = serde_json::from_str::<Value>(&payload).map_err(|error| {
            crate::metrics::record_stream_transform_protocol_error("invalid_json");
            ProxyError::bad_gateway(format!("upstream SSE data is not valid JSON: {error}"))
        })?;
        let frames = match self.anthropic_bridge.as_mut() {
            Some(AnthropicBridgeState::Responses(state)) => state.transform(&value),
            Some(AnthropicBridgeState::Chat(state)) => state.transform(&value),
            None => transform_stream_value(
                self.upstream.expect("upstream format is present"),
                self.downstream,
                &value,
                &self.custom_tool_names,
            ),
        };
        Ok(encode_stream_frames(&frames))
    }
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
enum AnthropicBridgeState {
    Responses(ResponsesAnthropicState),
    Chat(ChatAnthropicState),
}

impl AnthropicBridgeState {
    fn completed(&self) -> bool {
        match self {
            Self::Responses(state) => state.completed,
            Self::Chat(state) => state.completed,
        }
    }
}

#[derive(Debug, Default)]
struct ResponsesAnthropicState {
    next_block_index: u64,
    message_started: bool,
    text_block: Option<BlockState>,
    tools: BTreeMap<i64, ToolBlockState>,
    saw_tool: bool,
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
}

impl ResponsesAnthropicState {
    fn transform(&mut self, input: &Value) -> Vec<StreamFrame> {
        if self.completed {
            return Vec::new();
        }
        match input.get("type").and_then(Value::as_str) {
            Some("response.created") => self.ensure_message_start(input),
            Some("response.output_text.delta") => self.text_delta(input),
            Some("response.output_item.added") => self.output_item_added(input),
            Some("response.function_call_arguments.delta") => self.argument_delta(input),
            Some("response.function_call_arguments.done") => self.argument_done(input),
            Some("response.output_item.done") => self.output_item_done(input),
            Some("response.completed") => self.complete(input),
            Some("response.failed") | Some("response.incomplete") => self.fail(input),
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
        let mut frames = self.ensure_text_block();
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

    fn output_item_added(&mut self, input: &Value) -> Vec<StreamFrame> {
        let Some(item) = input
            .get("item")
            .filter(|item| item.get("type").and_then(Value::as_str) == Some("function_call"))
        else {
            return Vec::new();
        };
        let Some(output_index) = input.get("output_index").and_then(Value::as_i64) else {
            protocol_error("missing_output_index");
            return Vec::new();
        };
        self.open_tool(output_index, item)
    }

    fn open_tool(&mut self, output_index: i64, item: &Value) -> Vec<StreamFrame> {
        if self.tools.contains_key(&output_index) {
            return Vec::new();
        }
        let index = self.allocate_index();
        self.saw_tool = true;
        self.tools.insert(
            output_index,
            ToolBlockState {
                block: BlockState { index, open: true },
                argument_delta_seen: false,
            },
        );
        vec![StreamFrame::event(
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
        )]
    }

    fn argument_delta(&mut self, input: &Value) -> Vec<StreamFrame> {
        let Some(output_index) = input.get("output_index").and_then(Value::as_i64) else {
            protocol_error("missing_output_index");
            return Vec::new();
        };
        let Some(arguments) = input.get("delta").and_then(Value::as_str) else {
            return Vec::new();
        };
        let Some(tool) = self
            .tools
            .get_mut(&output_index)
            .filter(|tool| tool.block.open)
        else {
            protocol_error("unknown_tool_index");
            return Vec::new();
        };
        tool.argument_delta_seen = true;
        vec![input_json_delta(tool.block.index, arguments)]
    }

    fn argument_done(&mut self, input: &Value) -> Vec<StreamFrame> {
        let Some(output_index) = input.get("output_index").and_then(Value::as_i64) else {
            protocol_error("missing_output_index");
            return Vec::new();
        };
        let Some(tool) = self
            .tools
            .get_mut(&output_index)
            .filter(|tool| tool.block.open)
        else {
            protocol_error("unknown_tool_index");
            return Vec::new();
        };
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

    fn output_item_done(&mut self, input: &Value) -> Vec<StreamFrame> {
        let Some(item) = input
            .get("item")
            .filter(|item| item.get("type").and_then(Value::as_str) == Some("function_call"))
        else {
            return Vec::new();
        };
        let Some(output_index) = input.get("output_index").and_then(Value::as_i64) else {
            protocol_error("missing_output_index");
            return Vec::new();
        };
        let mut frames = self.open_tool(output_index, item);
        let Some(tool) = self.tools.get_mut(&output_index) else {
            return frames;
        };
        if !tool.block.open {
            return frames;
        }
        if !tool.argument_delta_seen {
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
        frames
    }

    fn complete(&mut self, input: &Value) -> Vec<StreamFrame> {
        let mut frames = if self.text_block.is_none() && self.tools.is_empty() {
            self.ensure_text_block()
        } else {
            Vec::new()
        };
        frames.extend(self.close_open_blocks());
        let response = input.get("response").unwrap_or(input);
        let stop_reason = if self.saw_tool {
            "tool_use"
        } else {
            "end_turn"
        };
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

    fn fail(&mut self, input: &Value) -> Vec<StreamFrame> {
        let mut frames = self.close_open_blocks();
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
        if let Some(block) = self.text_block.as_mut().filter(|block| block.open) {
            block.open = false;
            frames.push(content_block_stop(block.index));
        }
        for tool in self.tools.values_mut().filter(|tool| tool.block.open) {
            tool.block.open = false;
            frames.push(content_block_stop(tool.block.index));
        }
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
        if let Some(text) = choice.pointer("/delta/content").and_then(Value::as_str) {
            if self.text_block.is_none() {
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
        }
        if let Some(tool_calls) = choice
            .pointer("/delta/tool_calls")
            .and_then(Value::as_array)
        {
            for tool_call in tool_calls {
                let Some(tool_index) = tool_call.get("index").and_then(Value::as_i64) else {
                    protocol_error("missing_tool_index");
                    continue;
                };
                if !self.tools.contains_key(&tool_index)
                    && (tool_call.get("id").is_some()
                        || tool_call.pointer("/function/name").is_some())
                {
                    let index = self.allocate_index();
                    self.saw_tool = true;
                    self.tools.insert(
                        tool_index,
                        ToolBlockState {
                            block: BlockState { index, open: true },
                            argument_delta_seen: false,
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
        if choice
            .get("finish_reason")
            .is_some_and(|value| !value.is_null())
        {
            frames.extend(self.finish(input));
        }
        frames
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
        if self.text_block.is_none() && self.tools.is_empty() {
            let index = self.allocate_index();
            self.text_block = Some(BlockState { index, open: true });
            frames.push(StreamFrame::event(
                "content_block_start",
                json!({"type": "content_block_start", "index": index, "content_block": {"type": "text", "text": ""}}),
            ));
        }
        if let Some(block) = self.text_block.as_mut().filter(|block| block.open) {
            block.open = false;
            frames.push(content_block_stop(block.index));
        }
        for tool in self.tools.values_mut().filter(|tool| tool.block.open) {
            tool.block.open = false;
            frames.push(content_block_stop(tool.block.index));
        }
        let output_tokens = input
            .pointer("/usage/completion_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        frames.push(StreamFrame::event(
            "message_delta",
            json!({
                "type": "message_delta",
                "delta": {"stop_reason": if self.saw_tool {"tool_use"} else {"end_turn"}, "stop_sequence": Value::Null},
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
            custom_tool_names: BTreeSet::new(),
            anthropic_bridge: Some(AnthropicBridgeState::Responses(
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

        assert_eq!(first[0].payload_json()["index"], json!(0));
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
}
