use std::collections::{BTreeMap, BTreeSet};

use bytes::Bytes;
use serde_json::{json, Value};

use crate::domain::providers::{model::ProviderType, store::StoredProvider};

use super::adapters::{
    downstream_format_for_route, encode_stream_frames, transform_stream_value,
    unwrap_gemini_v1internal_value, upstream_format_for_route, UpstreamFormat,
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
    unwrap_v1internal: bool,
    gemini_terminal: Option<GeminiStreamTerminalState>,
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
        let unwrap_v1internal =
            super::adapters::is_gemini_v1internal_provider_type(stored.provider_type);
        let gemini_terminal = unwrap_v1internal.then(GeminiStreamTerminalState::default);
        let bridge = match (upstream, downstream) {
            (Some(UpstreamFormat::OpenAiResponses), UpstreamFormat::OpenAiResponses)
                if stored.provider_type == ProviderType::GrokOAuth
                    && responses_tool_context.requires_grok_emulation() =>
            {
                Some(StreamBridgeState::GrokResponsesTools(
                    GrokResponsesToolsState::new(responses_tool_context.clone()),
                ))
            }
            (Some(UpstreamFormat::OpenAiResponses), UpstreamFormat::AnthropicMessages) => Some(
                StreamBridgeState::ResponsesAnthropic(ResponsesAnthropicState::default()),
            ),
            (Some(UpstreamFormat::OpenAiChat), UpstreamFormat::AnthropicMessages) => Some(
                StreamBridgeState::ChatAnthropic(ChatAnthropicState::default()),
            ),
            (Some(UpstreamFormat::GeminiNative), UpstreamFormat::AnthropicMessages) => Some(
                StreamBridgeState::GeminiAnthropic(GeminiAnthropicState::default()),
            ),
            (Some(UpstreamFormat::GeminiNative), UpstreamFormat::OpenAiResponses) => {
                Some(StreamBridgeState::GeminiOpenAi(Box::new(
                    GeminiOpenAiState::responses(responses_tool_context.clone()),
                )))
            }
            (Some(UpstreamFormat::GeminiNative), UpstreamFormat::OpenAiChat) => {
                Some(StreamBridgeState::GeminiOpenAi(Box::new(
                    GeminiOpenAiState::chat(responses_tool_context.clone()),
                )))
            }
            (Some(UpstreamFormat::OpenAiResponses), UpstreamFormat::OpenAiChat) => Some(
                StreamBridgeState::ResponsesChat(ResponsesChatState::default()),
            ),
            (Some(UpstreamFormat::AnthropicMessages), UpstreamFormat::OpenAiChat) => {
                Some(StreamBridgeState::AnthropicChat(Box::new(
                    AnthropicChatState::new(responses_tool_context.clone()),
                )))
            }
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
            (Some(UpstreamFormat::AnthropicMessages), UpstreamFormat::GeminiNative) => Some(
                StreamBridgeState::ToGemini(Box::new(ToGeminiState::anthropic())),
            ),
            (Some(UpstreamFormat::OpenAiResponses), UpstreamFormat::GeminiNative) => Some(
                StreamBridgeState::ToGemini(Box::new(ToGeminiState::responses())),
            ),
            (Some(UpstreamFormat::OpenAiChat), UpstreamFormat::GeminiNative) => {
                Some(StreamBridgeState::ToGemini(Box::new(ToGeminiState::chat())))
            }
            _ => None,
        };
        Self {
            upstream,
            downstream,
            buffer: Vec::new(),
            responses_tool_context,
            bridge,
            unwrap_v1internal,
            gemini_terminal,
        }
    }

    pub(super) fn push(&mut self, chunk: Bytes) -> Result<Bytes, ProxyError> {
        let Some(upstream) = self.upstream else {
            return Ok(chunk);
        };
        if upstream == self.downstream && !self.unwrap_v1internal && self.bridge.is_none() {
            return Ok(chunk);
        }
        self.buffer.extend_from_slice(&chunk);
        self.drain_complete_events(false)
    }

    pub(super) fn finish(&mut self) -> Result<Bytes, ProxyError> {
        let Some(upstream) = self.upstream else {
            return Ok(Bytes::new());
        };
        if upstream == self.downstream && !self.unwrap_v1internal && self.bridge.is_none() {
            return Ok(Bytes::new());
        }
        let mut output = self.drain_complete_events(true)?.to_vec();
        if let Some(terminal) = self.gemini_terminal.as_ref() {
            terminal.validate()?;
        }
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
            if let Some(terminal) = self.gemini_terminal.as_ref() {
                terminal.validate()?;
            }
            let frames = self
                .bridge
                .as_mut()
                .map(StreamBridgeState::upstream_done)
                .transpose()?;
            return Ok(match frames {
                Some(frames) => encode_stream_frames(&frames),
                None if self.downstream == UpstreamFormat::AnthropicMessages => String::new(),
                None => encode_stream_frames(&[StreamFrame::done()]),
            });
        }
        let mut value = serde_json::from_str::<Value>(&payload).map_err(|error| {
            crate::metrics::record_stream_transform_protocol_error("invalid_json");
            ProxyError::bad_gateway(format!("upstream SSE data is not valid JSON: {error}"))
        })?;
        if self.unwrap_v1internal {
            if let Some(error) = super::adapters::google_embedded_error(&value) {
                crate::metrics::record_stream_transform_protocol_error("embedded_google_error");
                return Err(error);
            }
            value = unwrap_gemini_v1internal_value(value);
        }
        if let Some(terminal) = self.gemini_terminal.as_mut() {
            terminal.observe(&value)?;
        }
        let frames = if self.upstream == Some(self.downstream) {
            vec![StreamFrame::json(value)]
        } else {
            match self.bridge.as_mut() {
                Some(bridge) => bridge.transform(&value)?,
                None => transform_stream_value(
                    self.upstream.expect("upstream format is present"),
                    self.downstream,
                    &value,
                    &self.responses_tool_context,
                ),
            }
        };
        Ok(encode_stream_frames(&frames))
    }
}

#[derive(Debug, Default)]
struct GeminiStreamTerminalState {
    candidates: BTreeMap<i64, bool>,
    blocked_prompt: bool,
}

impl GeminiStreamTerminalState {
    fn observe(&mut self, response: &Value) -> Result<(), ProxyError> {
        let response = response.as_object().ok_or_else(|| {
            ProxyError::bad_gateway("Gemini v1internal response must be a JSON object")
        })?;
        if response
            .get("candidates")
            .is_some_and(|value| !value.is_array())
        {
            return Err(ProxyError::bad_gateway(
                "Gemini v1internal candidates must be an array",
            ));
        }
        if let Some(candidates) = response.get("candidates").and_then(Value::as_array) {
            for (position, candidate) in candidates.iter().enumerate() {
                let candidate = candidate.as_object().ok_or_else(|| {
                    ProxyError::bad_gateway("Gemini v1internal candidate must be a JSON object")
                })?;
                let index = candidate
                    .get("index")
                    .and_then(Value::as_i64)
                    .unwrap_or(position as i64);
                let finished = candidate
                    .get("finishReason")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .is_some_and(|reason| !reason.is_empty());
                self.candidates
                    .entry(index)
                    .and_modify(|terminal| *terminal |= finished)
                    .or_insert(finished);
            }
        }
        self.blocked_prompt |= response
            .get("promptFeedback")
            .and_then(|feedback| feedback.get("blockReason"))
            .or_else(|| {
                response
                    .get("prompt_feedback")
                    .and_then(|feedback| feedback.get("block_reason"))
            })
            .and_then(Value::as_str)
            .map(str::trim)
            .is_some_and(|reason| !reason.is_empty());
        Ok(())
    }

    fn validate(&self) -> Result<(), ProxyError> {
        if !self.candidates.is_empty() && self.candidates.values().all(|finished| *finished) {
            return Ok(());
        }
        if self.candidates.is_empty() && self.blocked_prompt {
            return Ok(());
        }
        Err(ProxyError::bad_gateway(
            "Gemini v1internal stream ended before a terminal candidate or blocked prompt feedback",
        ))
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
    GrokResponsesTools(GrokResponsesToolsState),
    ResponsesAnthropic(ResponsesAnthropicState),
    ChatAnthropic(ChatAnthropicState),
    GeminiAnthropic(GeminiAnthropicState),
    GeminiOpenAi(Box<GeminiOpenAiState>),
    ResponsesChat(ResponsesChatState),
    AnthropicChat(Box<AnthropicChatState>),
    ChatResponses(Box<ChatResponsesState>),
    AnthropicResponses(AnthropicResponsesState),
    ToGemini(Box<ToGeminiState>),
}

impl StreamBridgeState {
    fn transform(&mut self, input: &Value) -> Result<Vec<StreamFrame>, ProxyError> {
        Ok(match self {
            Self::GrokResponsesTools(state) => state.transform(input),
            Self::ResponsesAnthropic(state) => state.transform(input),
            Self::ChatAnthropic(state) => state.transform(input),
            Self::GeminiAnthropic(state) => state.transform(input),
            Self::GeminiOpenAi(state) => return state.transform(input),
            Self::ResponsesChat(state) => state.transform(input),
            Self::AnthropicChat(state) => state.transform(input),
            Self::ChatResponses(state) => state.transform(input),
            Self::AnthropicResponses(state) => state.transform(input),
            Self::ToGemini(state) => return state.transform(input),
        })
    }

    fn upstream_done(&mut self) -> Result<Vec<StreamFrame>, ProxyError> {
        match self {
            Self::GrokResponsesTools(state) if state.completed => Ok(state.finish_stream()),
            Self::ChatResponses(state) => Ok(state.finish_stream()),
            Self::GeminiAnthropic(state) => state.finish_stream(),
            Self::GeminiOpenAi(state) => state.finish_stream(),
            Self::ToGemini(state) => state.finish_stream(),
            Self::ResponsesChat(state) if state.completed => Ok(Vec::new()),
            Self::AnthropicChat(state) if state.completed() => Ok(Vec::new()),
            Self::AnthropicResponses(state) if state.completed => Ok(Vec::new()),
            Self::ResponsesAnthropic(state) if state.completed => Ok(Vec::new()),
            Self::ChatAnthropic(state) if state.completed => Ok(Vec::new()),
            _ => {
                protocol_error("done_before_terminal");
                Err(ProxyError::bad_gateway(
                    "upstream stream sent DONE before a terminal event",
                ))
            }
        }
    }

    fn finish_eof(&mut self) -> Result<Vec<StreamFrame>, ProxyError> {
        if let Self::GrokResponsesTools(state) = self {
            let frames = state.finish_stream();
            if state.completed {
                return Ok(frames);
            }
        }
        if let Self::ChatResponses(state) = self {
            let frames = state.finish_stream();
            if state.completed {
                return Ok(frames);
            }
        }
        match self {
            Self::GeminiAnthropic(state) => return state.finish_stream(),
            Self::GeminiOpenAi(state) => return state.finish_stream(),
            Self::ToGemini(state) => return state.finish_stream(),
            _ => {}
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
            Self::GrokResponsesTools(state) => state.completed,
            Self::ResponsesAnthropic(state) => state.completed,
            Self::ChatAnthropic(state) => state.completed,
            Self::GeminiAnthropic(state) => state.completed,
            Self::GeminiOpenAi(state) => state.completed(),
            Self::ResponsesChat(state) => state.completed,
            Self::AnthropicChat(state) => state.completed(),
            Self::ChatResponses(state) => state.completed,
            Self::AnthropicResponses(state) => state.completed,
            Self::ToGemini(state) => state.completed(),
        }
    }
}

#[derive(Debug)]
pub(super) struct GrokResponsesToolsState {
    context: transforms::ResponsesToolContext,
    calls: BTreeMap<i64, GrokEmulatedToolCall>,
    completed: bool,
}

#[derive(Debug)]
struct GrokEmulatedToolCall {
    kind: transforms::ResponsesToolKind,
    name: String,
    item_id: String,
    call_id: String,
    arguments: String,
    added_event: Value,
}

impl GrokResponsesToolsState {
    pub(super) fn new(context: transforms::ResponsesToolContext) -> Self {
        Self {
            context,
            calls: BTreeMap::new(),
            completed: false,
        }
    }

    pub(super) fn transform(&mut self, input: &Value) -> Vec<StreamFrame> {
        if self.completed {
            return Vec::new();
        }
        match input.get("type").and_then(Value::as_str) {
            Some("response.output_item.added") => self.output_item_added(input),
            Some("response.function_call_arguments.delta") => self.arguments_delta(input),
            Some("response.function_call_arguments.done") => self.arguments_done(input),
            Some("response.output_item.done") => self.output_item_done(input),
            Some("response.completed" | "response.incomplete" | "response.failed") => {
                let mut restored = input.clone();
                transforms::restore_grok_responses_tool_items(&mut restored, &self.context);
                self.calls.clear();
                self.completed = true;
                vec![StreamFrame::json(restored)]
            }
            _ => vec![StreamFrame::json(input.clone())],
        }
    }

    fn output_item_added(&mut self, input: &Value) -> Vec<StreamFrame> {
        let Some(item) = input.get("item") else {
            return vec![StreamFrame::json(input.clone())];
        };
        if item.get("type").and_then(Value::as_str) != Some("function_call") {
            return vec![StreamFrame::json(input.clone())];
        }
        let name = item.get("name").and_then(Value::as_str).unwrap_or_default();
        let Some(kind) = self.context.tool_kind(name) else {
            return vec![StreamFrame::json(input.clone())];
        };
        if kind == transforms::ResponsesToolKind::Function {
            return vec![StreamFrame::json(input.clone())];
        }
        let index = input
            .get("output_index")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        let call_id = item
            .get("call_id")
            .or_else(|| item.get("id"))
            .and_then(Value::as_str)
            .unwrap_or("call_tool")
            .to_string();
        let item_id = item
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("fc_tool")
            .to_string();
        self.calls.insert(
            index,
            GrokEmulatedToolCall {
                kind,
                name: name.to_string(),
                item_id: item_id.clone(),
                call_id: call_id.clone(),
                arguments: item
                    .get("arguments")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                added_event: input.clone(),
            },
        );
        if matches!(
            kind,
            transforms::ResponsesToolKind::ApplyPatch | transforms::ResponsesToolKind::LocalShell
        ) {
            return Vec::new();
        }
        let mut output = input.clone();
        output["item"] = self
            .context
            .response_item(&item_id, "in_progress", &call_id, name, "");
        vec![StreamFrame::json(output)]
    }

    fn arguments_delta(&mut self, input: &Value) -> Vec<StreamFrame> {
        let index = input
            .get("output_index")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        let Some(call) = self.calls.get_mut(&index) else {
            return vec![StreamFrame::json(input.clone())];
        };
        if let Some(delta) = input.get("delta").and_then(Value::as_str) {
            call.arguments.push_str(delta);
        }
        if call.kind == transforms::ResponsesToolKind::Namespace {
            vec![StreamFrame::json(input.clone())]
        } else {
            Vec::new()
        }
    }

    fn arguments_done(&mut self, input: &Value) -> Vec<StreamFrame> {
        let index = input
            .get("output_index")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        let Some(call) = self.calls.get_mut(&index) else {
            return vec![StreamFrame::json(input.clone())];
        };
        if let Some(arguments) = input.get("arguments").and_then(Value::as_str) {
            call.arguments = arguments.to_string();
        }
        if call.kind == transforms::ResponsesToolKind::Namespace {
            return vec![StreamFrame::json(input.clone())];
        }
        if call.kind != transforms::ResponsesToolKind::Custom {
            return Vec::new();
        }
        let custom_input = custom_tool_input_from_function_arguments(&call.arguments);
        let mut frames = Vec::new();
        if !custom_input.is_empty() {
            frames.push(StreamFrame::json(json!({
                "type": "response.custom_tool_call_input.delta",
                "item_id": call.item_id,
                "output_index": index,
                "delta": custom_input
            })));
        }
        frames.push(StreamFrame::json(json!({
            "type": "response.custom_tool_call_input.done",
            "item_id": call.item_id,
            "output_index": index,
            "input": custom_input
        })));
        frames
    }

    fn output_item_done(&mut self, input: &Value) -> Vec<StreamFrame> {
        let index = input
            .get("output_index")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        let Some(call) = self.calls.remove(&index) else {
            return vec![StreamFrame::json(input.clone())];
        };
        let arguments = input
            .pointer("/item/arguments")
            .and_then(Value::as_str)
            .filter(|arguments| !arguments.is_empty())
            .unwrap_or(&call.arguments)
            .to_string();
        let mut frames = Vec::new();
        if matches!(
            call.kind,
            transforms::ResponsesToolKind::ApplyPatch | transforms::ResponsesToolKind::LocalShell
        ) {
            let mut added = call.added_event;
            added["item"] = self.context.response_item(
                &call.item_id,
                "in_progress",
                &call.call_id,
                &call.name,
                &arguments,
            );
            frames.push(StreamFrame::json(added));
        }
        let mut output = input.clone();
        output["item"] = self.context.response_item(
            &call.item_id,
            "completed",
            &call.call_id,
            &call.name,
            &arguments,
        );
        frames.push(StreamFrame::json(output));
        frames
    }

    fn finish_stream(&mut self) -> Vec<StreamFrame> {
        self.calls.clear();
        Vec::new()
    }
}

fn custom_tool_input_from_function_arguments(arguments: &str) -> String {
    serde_json::from_str::<Value>(arguments)
        .ok()
        .and_then(|value| {
            value
                .get("input")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| arguments.to_string())
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
    pending_tool_items: BTreeMap<i64, Value>,
    defer_tool_blocks: bool,
    saw_tool: bool,
    saw_reasoning: bool,
    web_search_requests: u64,
    x_search_requests: u64,
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
    fn deferred_for_gemini() -> Self {
        Self {
            defer_tool_blocks: true,
            ..Self::default()
        }
    }

    fn transform(&mut self, input: &Value) -> Vec<StreamFrame> {
        if self.completed {
            return Vec::new();
        }
        match input.get("type").and_then(Value::as_str) {
            Some("response.created") => self.ensure_message_start(input),
            Some("response.output_text.delta") => self.text_delta(input),
            Some("response.output_text.annotation.added") => self.annotation_added(input),
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
        if hosted_search_name(item).is_some() {
            let output_index = response_output_index(input).unwrap_or_else(|| {
                -(i64::try_from(self.item_ids.len())
                    .unwrap_or(i64::MAX)
                    .saturating_add(1))
            });
            if let Some(item_id) = item
                .get("id")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
            {
                self.item_ids.insert(output_index, item_id.to_string());
            }
            merge_bridge_item(
                self.pending_tool_items.entry(output_index).or_default(),
                item,
            );
            return self.ensure_message_start(input);
        }
        if !matches!(
            item.get("type").and_then(Value::as_str),
            Some("function_call" | "custom_tool_call" | "tool_search_call")
        ) {
            return Vec::new();
        }
        let Some(output_index) = self.event_output_index(input) else {
            protocol_error("missing_output_index");
            return Vec::new();
        };
        let output_index = self.resolve_tool_index(output_index, item);
        let mut frames = self.ensure_message_start(input);
        frames.extend(self.close_reasoning_block(None));
        frames.extend(self.close_text_block());
        if self.defer_tool_blocks {
            self.saw_tool = true;
            merge_bridge_item(
                self.pending_tool_items.entry(output_index).or_default(),
                item,
            );
            return frames;
        }
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
        let mut content_block = json!({
            "type": "tool_use",
            "id": item.get("call_id").or_else(|| item.get("id")).and_then(Value::as_str).unwrap_or("tool"),
            "name": item.get("name").and_then(Value::as_str).unwrap_or("tool"),
            "input": {}
        });
        if let Some(signature) = bridge_thought_signature(item) {
            content_block["signature"] = Value::String(signature.to_string());
        }
        let mut frames = vec![StreamFrame::event(
            "content_block_start",
            json!({
                "type": "content_block_start",
                "index": index,
                "content_block": content_block
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
        self.remember_deferred_tool_metadata(output_index, input);
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
        self.remember_deferred_tool_metadata(output_index, input);
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
        let Some(output_index) = self.event_output_index(input) else {
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
        let Some(output_index) = self.event_output_index(input) else {
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
        if hosted_search_name(item).is_some() {
            return self.finish_hosted_search(input, item);
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
        let mut effective_item = self
            .pending_tool_items
            .remove(&output_index)
            .unwrap_or_else(|| json!({}));
        merge_bridge_item(&mut effective_item, item);
        let item = if self.defer_tool_blocks {
            &effective_item
        } else {
            item
        };
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
        if self.text_block.is_none()
            && self.tools.is_empty()
            && !self.saw_reasoning
            && self.web_search_requests == 0
            && self.x_search_requests == 0
        {
            frames.extend(self.ensure_text_block());
        }
        frames.extend(self.close_open_blocks());
        let stop_reason =
            transforms::openai_response_to_anthropic_stop_with_tools(response, self.saw_tool);
        let mut usage = transforms::anthropic_usage_from_openai_usage(response.get("usage"));
        let hosted_search_requests = self
            .web_search_requests
            .saturating_add(self.x_search_requests);
        if hosted_search_requests > 0 {
            usage["server_tool_use"] = json!({
                "web_search_requests": hosted_search_requests,
                "x_search_requests": self.x_search_requests
            });
        }
        frames.push(StreamFrame::event(
            "message_delta",
            json!({
                "type": "message_delta",
                "delta": {"stop_reason": stop_reason, "stop_sequence": Value::Null},
                "usage": usage
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

    fn event_output_index(&self, input: &Value) -> Option<i64> {
        response_output_index(input).or_else(|| {
            let item_id = input
                .get("item_id")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())?;
            self.item_ids
                .iter()
                .find_map(|(index, known_id)| (known_id == item_id).then_some(*index))
        })
    }

    fn item_output_index(&self, input: &Value, item: &Value) -> Option<i64> {
        self.event_output_index(input).or_else(|| {
            let item_id = item
                .get("id")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())?;
            self.item_ids
                .iter()
                .find_map(|(index, known_id)| (known_id == item_id).then_some(*index))
        })
    }

    fn annotation_added(&mut self, input: &Value) -> Vec<StreamFrame> {
        let Some(annotation) = input.get("annotation") else {
            return Vec::new();
        };
        if annotation.get("type").and_then(Value::as_str) != Some("url_citation") {
            return Vec::new();
        }
        let Some(block) = self.text_block.filter(|block| block.open) else {
            return Vec::new();
        };
        vec![StreamFrame::event(
            "content_block_delta",
            json!({
                "type": "content_block_delta",
                "index": block.index,
                "delta": {
                    "type": "citations_delta",
                    "citation": hosted_search_citation(annotation)
                }
            }),
        )]
    }

    fn finish_hosted_search(&mut self, input: &Value, item: &Value) -> Vec<StreamFrame> {
        if response_item_was_emitted(
            &self.completed_tool_items,
            input,
            Some(item),
            &self.item_ids,
        ) {
            return Vec::new();
        }
        let Some(output_index) = self.item_output_index(input, item) else {
            protocol_error("missing_output_index");
            return Vec::new();
        };
        let output_index = self.resolve_tool_index(output_index, item);
        let mut effective_item = self
            .pending_tool_items
            .remove(&output_index)
            .unwrap_or_else(|| json!({}));
        merge_bridge_item(&mut effective_item, item);
        let Some(name) = hosted_search_name(&effective_item) else {
            return Vec::new();
        };
        let query = if name == "web_search" {
            effective_item
                .pointer("/action/query")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        } else {
            let pending = self
                .pending_custom_inputs
                .remove(&output_index)
                .unwrap_or_default();
            hosted_search_query(
                pending
                    .done
                    .or_else(|| effective_item.get("input").cloned())
                    .unwrap_or_else(|| Value::String(pending.delta)),
            )
        };
        let raw_id = effective_item
            .get("id")
            .or_else(|| effective_item.get("call_id"))
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .unwrap_or("search");
        let tool_use_id = if raw_id.starts_with("srvtoolu_") {
            raw_id.to_string()
        } else {
            format!("srvtoolu_{raw_id}")
        };
        let mut frames = self.ensure_message_start(input);
        frames.extend(self.close_reasoning_block(None));
        frames.extend(self.close_text_block());
        let tool_index = self.allocate_index();
        let result_index = self.allocate_index();
        frames.push(StreamFrame::event(
            "content_block_start",
            json!({
                "type": "content_block_start",
                "index": tool_index,
                "content_block": {
                    "type": "server_tool_use",
                    "id": tool_use_id,
                    "name": name,
                    "input": {}
                }
            }),
        ));
        frames.push(input_json_delta(
            tool_index,
            &json!({"query": query}).to_string(),
        ));
        frames.push(content_block_stop(tool_index));
        frames.push(StreamFrame::event(
            "content_block_start",
            json!({
                "type": "content_block_start",
                "index": result_index,
                "content_block": {
                    "type": format!("{name}_tool_result"),
                    "tool_use_id": tool_use_id,
                    "content": []
                }
            }),
        ));
        frames.push(content_block_stop(result_index));
        if name == "web_search" {
            self.web_search_requests = self.web_search_requests.saturating_add(1);
        } else {
            self.x_search_requests = self.x_search_requests.saturating_add(1);
        }
        mark_response_item_emitted(
            &mut self.completed_tool_items,
            input,
            Some(&effective_item),
            &self.item_ids,
        );
        frames
    }

    fn remember_deferred_tool_metadata(&mut self, output_index: i64, input: &Value) {
        if !self.defer_tool_blocks {
            return;
        }
        let Some(signature) = bridge_thought_signature(input) else {
            return;
        };
        self.pending_tool_items
            .entry(output_index)
            .or_insert_with(|| json!({}))["thought_signature"] =
            Value::String(signature.to_string());
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
                (known_id == item_id
                    && (self.tools.contains_key(known_index)
                        || self.pending_tool_items.contains_key(known_index)))
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
        let bridge_block = item.and_then(|item| {
            let signature = bridge_thought_signature(item);
            let mut block = anthropic_block_from_openai_reasoning_item(item).or_else(|| {
                signature.map(|_| {
                    json!({
                        "type": "thinking",
                        "thinking": super::reasoning_bridge::reasoning_summary_text(item)
                    })
                })
            })?;
            if let Some(signature) = signature {
                block["signature"] = Value::String(signature.to_string());
            }
            Some(block)
        });
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
struct GeminiAnthropicState {
    next_block_index: u64,
    selected_candidate_index: Option<i64>,
    message_started: bool,
    text_block: Option<BlockState>,
    text_seen: String,
    thinking_block: Option<BlockState>,
    thinking_seen: String,
    thinking_signature: Option<String>,
    tools: BTreeMap<GeminiAnthropicToolKey, GeminiAnthropicToolState>,
    tool_order: Vec<GeminiAnthropicToolKey>,
    usage: BTreeMap<String, Value>,
    pending_finish_reason: Option<String>,
    pending_blocked_prompt: bool,
    saw_content: bool,
    saw_tool: bool,
    completed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct GeminiAnthropicToolKey {
    candidate_index: i64,
    part_index: usize,
    occurrence: usize,
}

#[derive(Debug)]
struct GeminiAnthropicToolState {
    id: String,
    name: String,
    arguments: Value,
    signature: Option<String>,
    emitted: bool,
}

impl GeminiAnthropicState {
    fn transform(&mut self, input: &Value) -> Vec<StreamFrame> {
        self.observe_usage(input);
        if self.completed {
            return Vec::new();
        }
        if self.pending_finish_reason.is_some() {
            return Vec::new();
        }
        let candidates = input.get("candidates").and_then(Value::as_array);
        let candidate = candidates.and_then(|candidates| {
            let selected = self.selected_candidate_index;
            candidates
                .iter()
                .enumerate()
                .find(|(position, candidate)| {
                    let index = gemini_candidate_index(candidate, *position);
                    selected.map_or(index == 0, |selected| selected == index)
                })
                .or_else(|| {
                    selected
                        .is_none()
                        .then(|| candidates.iter().enumerate().next())
                        .flatten()
                })
                .map(|(position, candidate)| {
                    let index = gemini_candidate_index(candidate, position);
                    self.selected_candidate_index.get_or_insert(index);
                    (index, candidate)
                })
        });
        let blocked_prompt = candidate
            .is_none()
            .then(|| {
                input
                    .get("promptFeedback")
                    .and_then(|feedback| feedback.get("blockReason"))
                    .or_else(|| {
                        input
                            .get("prompt_feedback")
                            .and_then(|feedback| feedback.get("block_reason"))
                    })
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|reason| !reason.is_empty())
            })
            .flatten();
        if let Some(block_reason) = blocked_prompt {
            let frames = self.ensure_message_start(input);
            self.pending_finish_reason = Some(block_reason.to_string());
            self.pending_blocked_prompt = true;
            return frames;
        }
        let parts = candidate
            .and_then(|(_, candidate)| candidate.pointer("/content/parts"))
            .and_then(Value::as_array);
        let finish_reason = candidate.and_then(|(_, candidate)| {
            candidate
                .get("finishReason")
                .or_else(|| candidate.get("finish_reason"))
                .and_then(Value::as_str)
        });
        if parts.is_none_or(|parts| parts.is_empty()) && finish_reason.is_none() {
            return Vec::new();
        }

        let mut frames = self.ensure_message_start(input);
        if let Some(parts) = parts {
            let candidate_index = candidate.map(|(index, _)| index).unwrap_or_default();
            let mut occurrences = BTreeMap::<String, usize>::new();
            for (part_index, part) in parts.iter().enumerate() {
                if part.get("thought").and_then(Value::as_bool) == Some(true) {
                    if part
                        .get("text")
                        .and_then(Value::as_str)
                        .is_some_and(|text| !text.is_empty())
                        || gemini_thought_signature(part).is_some()
                    {
                        frames.extend(self.thinking_delta(part));
                    }
                } else if let Some(text) = part
                    .get("text")
                    .and_then(Value::as_str)
                    .filter(|text| !text.is_empty())
                {
                    frames.extend(self.text_delta(text));
                }
                if let Some(function_call) = part
                    .get("functionCall")
                    .or_else(|| part.get("function_call"))
                {
                    let name = function_call
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("tool")
                        .to_string();
                    let occurrence = occurrences.entry(name).or_default();
                    frames.extend(self.function_call(
                        candidate_index,
                        part_index,
                        *occurrence,
                        part,
                        function_call,
                    ));
                    *occurrence = occurrence.saturating_add(1);
                }
            }
        }
        if let Some(finish_reason) = finish_reason {
            self.pending_finish_reason = Some(finish_reason.to_string());
        }
        frames
    }

    fn ensure_message_start(&mut self, input: &Value) -> Vec<StreamFrame> {
        if self.message_started {
            return Vec::new();
        }
        self.message_started = true;
        let response_id = input
            .get("responseId")
            .or_else(|| input.get("response_id"))
            .and_then(Value::as_str)
            .unwrap_or("gemini");
        let model = input
            .get("modelVersion")
            .or_else(|| input.get("model_version"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        vec![StreamFrame::event(
            "message_start",
            json!({
                "type": "message_start",
                "message": {
                    "id": response_id,
                    "type": "message",
                    "role": "assistant",
                    "model": model,
                    "content": [],
                    "stop_reason": Value::Null,
                    "stop_sequence": Value::Null,
                    "usage": {"input_tokens": 0, "output_tokens": 0}
                }
            }),
        )]
    }

    fn text_delta(&mut self, text: &str) -> Vec<StreamFrame> {
        let mut frames = self.close_thinking_block();
        frames.extend(self.close_tool_blocks());
        if !self.text_block.is_some_and(|block| block.open) {
            let index = self.allocate_index();
            self.text_block = Some(BlockState { index, open: true });
            self.text_seen.clear();
            self.saw_content = true;
            frames.push(StreamFrame::event(
                "content_block_start",
                json!({
                    "type": "content_block_start",
                    "index": index,
                    "content_block": {"type": "text", "text": ""}
                }),
            ));
        }
        let text = gemini_incremental_text(&mut self.text_seen, text);
        if text.is_empty() {
            return frames;
        }
        let index = self.text_block.expect("text block exists").index;
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

    fn thinking_delta(&mut self, part: &Value) -> Vec<StreamFrame> {
        let mut frames = self.close_text_block();
        frames.extend(self.close_tool_blocks());
        let signature = gemini_thought_signature(part);
        if self.thinking_signature.is_some()
            && signature.is_some()
            && self.thinking_signature.as_deref() != signature
        {
            frames.extend(self.close_thinking_block());
        }
        if !self.thinking_block.is_some_and(|block| block.open) {
            let index = self.allocate_index();
            self.thinking_block = Some(BlockState { index, open: true });
            self.thinking_seen.clear();
            self.saw_content = true;
            frames.push(StreamFrame::event(
                "content_block_start",
                json!({
                    "type": "content_block_start",
                    "index": index,
                    "content_block": {"type": "thinking", "thinking": "", "signature": ""}
                }),
            ));
        }
        if let Some(text) = part
            .get("text")
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
        {
            let text = gemini_incremental_text(&mut self.thinking_seen, text);
            if text.is_empty() {
                if let Some(signature) = signature {
                    self.thinking_signature = Some(signature.to_string());
                }
                return frames;
            }
            let index = self.thinking_block.expect("thinking block exists").index;
            frames.push(StreamFrame::event(
                "content_block_delta",
                json!({
                    "type": "content_block_delta",
                    "index": index,
                    "delta": {"type": "thinking_delta", "thinking": text}
                }),
            ));
        }
        if let Some(signature) = signature {
            self.thinking_signature = Some(signature.to_string());
        }
        frames
    }

    fn function_call(
        &mut self,
        candidate_index: i64,
        part_index: usize,
        occurrence: usize,
        part: &Value,
        function_call: &Value,
    ) -> Vec<StreamFrame> {
        let mut frames = self.close_text_block();
        frames.extend(self.close_thinking_block());
        let name = function_call
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("tool");
        let explicit_id = function_call
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty());
        let slot = GeminiAnthropicToolKey {
            candidate_index,
            part_index,
            occurrence,
        };
        let key = explicit_id
            .and_then(|id| {
                self.tools
                    .iter()
                    .find_map(|(key, tool)| (!tool.emitted && tool.id == id).then(|| key.clone()))
            })
            .or_else(|| {
                self.tools
                    .get(&slot)
                    .filter(|tool| !tool.emitted && tool.name == name)
                    .map(|_| slot.clone())
            })
            .or_else(|| {
                self.tools.iter().find_map(|(key, tool)| {
                    (!tool.emitted
                        && key.candidate_index == candidate_index
                        && key.occurrence == occurrence
                        && tool.name == name)
                        .then(|| key.clone())
                })
            })
            .unwrap_or(slot);
        if !self.tools.contains_key(&key) {
            let id = explicit_id.map(str::to_string).unwrap_or_else(|| {
                format!(
                    "{name}_{}_{}_{}",
                    key.candidate_index, key.part_index, key.occurrence
                )
            });
            self.tool_order.push(key.clone());
            self.tools.insert(
                key.clone(),
                GeminiAnthropicToolState {
                    id,
                    name: name.to_string(),
                    arguments: json!({}),
                    signature: None,
                    emitted: false,
                },
            );
        }
        let tool = self.tools.get_mut(&key).expect("tool exists");
        if let Some(explicit_id) = explicit_id {
            tool.id = explicit_id.to_string();
        }
        if let Some(arguments) = function_call
            .get("args")
            .or_else(|| function_call.get("arguments"))
            .filter(|arguments| !arguments.is_null())
        {
            merge_gemini_tool_arguments(&mut tool.arguments, arguments);
        }
        if let Some(signature) = gemini_thought_signature(part) {
            tool.signature = Some(signature.to_string());
        }
        self.saw_content = true;
        self.saw_tool = true;
        frames
    }

    fn close_tool_blocks(&mut self) -> Vec<StreamFrame> {
        let mut frames = Vec::new();
        for key in self.tool_order.clone() {
            let Some(tool) = self.tools.get_mut(&key).filter(|tool| !tool.emitted) else {
                continue;
            };
            tool.emitted = true;
            let id = tool.id.clone();
            let name = tool.name.clone();
            let arguments = tool.arguments.clone();
            let signature = tool.signature.clone();
            let index = self.allocate_index();
            let mut content_block = json!({
                "type": "tool_use",
                "id": id,
                "name": name,
                "input": {}
            });
            if let Some(signature) = signature {
                content_block["signature"] = Value::String(signature);
            }
            frames.push(StreamFrame::event(
                "content_block_start",
                json!({
                    "type": "content_block_start",
                    "index": index,
                    "content_block": content_block
                }),
            ));
            if !arguments.is_null() {
                let arguments =
                    serde_json::to_string(&arguments).unwrap_or_else(|_| "{}".to_string());
                frames.push(input_json_delta(index, &arguments));
            }
            frames.push(content_block_stop(index));
        }
        frames
    }

    fn close_text_block(&mut self) -> Vec<StreamFrame> {
        let Some(block) = self.text_block.as_mut().filter(|block| block.open) else {
            return Vec::new();
        };
        block.open = false;
        self.text_seen.clear();
        vec![content_block_stop(block.index)]
    }

    fn close_thinking_block(&mut self) -> Vec<StreamFrame> {
        let Some(block) = self.thinking_block.as_mut().filter(|block| block.open) else {
            self.thinking_signature = None;
            return Vec::new();
        };
        block.open = false;
        self.thinking_seen.clear();
        let mut frames = Vec::new();
        if let Some(signature) = self.thinking_signature.take() {
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

    fn finish_stream(&mut self) -> Result<Vec<StreamFrame>, ProxyError> {
        if self.completed {
            return Ok(Vec::new());
        }
        let finish_reason = self.pending_finish_reason.take().ok_or_else(|| {
            ProxyError::bad_gateway("Gemini stream ended before candidate.finishReason")
        })?;
        Ok(self.finish(&finish_reason))
    }

    fn finish(&mut self, finish_reason: &str) -> Vec<StreamFrame> {
        let mut frames = Vec::new();
        if !self.saw_content && !self.pending_blocked_prompt {
            let index = self.allocate_index();
            frames.push(StreamFrame::event(
                "content_block_start",
                json!({
                    "type": "content_block_start",
                    "index": index,
                    "content_block": {"type": "text", "text": ""}
                }),
            ));
            frames.push(content_block_stop(index));
            self.saw_content = true;
        } else {
            frames.extend(self.close_text_block());
            frames.extend(self.close_thinking_block());
        }
        frames.extend(self.close_tool_blocks());
        let stop_reason = if self.pending_blocked_prompt {
            "refusal"
        } else {
            let mapped_stop_reason =
                transforms::gemini_finish_reason_to_anthropic(Some(finish_reason));
            match mapped_stop_reason {
                reason @ ("refusal" | "max_tokens") => reason,
                _ if self.saw_tool => "tool_use",
                reason => reason,
            }
        };
        let usage = self.normalized_usage();
        frames.push(StreamFrame::event(
            "message_delta",
            json!({
                "type": "message_delta",
                "delta": {"stop_reason": stop_reason, "stop_sequence": Value::Null},
                "usage": transforms::anthropic_usage_from_gemini_usage(Some(&usage))
            }),
        ));
        frames.push(StreamFrame::event(
            "message_stop",
            json!({"type": "message_stop"}),
        ));
        self.completed = true;
        frames
    }

    fn observe_usage(&mut self, input: &Value) {
        let Some(usage) = input
            .get("usageMetadata")
            .or_else(|| input.get("usage_metadata"))
            .and_then(Value::as_object)
        else {
            return;
        };
        for (key, value) in usage {
            self.usage.insert(key.clone(), value.clone());
        }
    }

    fn normalized_usage(&self) -> Value {
        let mut usage = self
            .usage
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect::<serde_json::Map<_, _>>();
        let prompt_tokens = usage_value_u64(&usage, &["promptTokenCount", "prompt_token_count"]);
        let total_tokens = usage_value_u64(&usage, &["totalTokenCount", "total_token_count"]);
        if let (Some(prompt_tokens), Some(total_tokens)) = (prompt_tokens, total_tokens) {
            let output_tokens = total_tokens.saturating_sub(prompt_tokens);
            let candidate_tokens =
                usage_value_u64(&usage, &["candidatesTokenCount", "candidates_token_count"]);
            let thought_tokens =
                usage_value_u64(&usage, &["thoughtsTokenCount", "thoughts_token_count"]);
            match (candidate_tokens, thought_tokens) {
                (None, Some(thought_tokens)) => {
                    usage.insert(
                        "candidatesTokenCount".to_string(),
                        json!(output_tokens.saturating_sub(thought_tokens)),
                    );
                }
                (Some(candidate_tokens), None) => {
                    usage.insert(
                        "thoughtsTokenCount".to_string(),
                        json!(output_tokens.saturating_sub(candidate_tokens)),
                    );
                }
                (None, None) => {
                    usage.insert("candidatesTokenCount".to_string(), json!(output_tokens));
                }
                (Some(_), Some(_)) => {}
            }
        }
        Value::Object(usage)
    }

    fn allocate_index(&mut self) -> u64 {
        let index = self.next_block_index;
        self.next_block_index = self.next_block_index.saturating_add(1);
        index
    }
}

fn gemini_candidate_index(candidate: &Value, position: usize) -> i64 {
    candidate
        .get("index")
        .and_then(Value::as_i64)
        .unwrap_or(position as i64)
}

fn merge_gemini_tool_arguments(current: &mut Value, incoming: &Value) {
    match (current, incoming) {
        (Value::Object(current), Value::Object(incoming)) => {
            for (key, value) in incoming {
                match current.get_mut(key) {
                    Some(current) => merge_gemini_tool_arguments(current, value),
                    None => {
                        current.insert(key.clone(), value.clone());
                    }
                }
            }
        }
        (current, incoming) => *current = incoming.clone(),
    }
}

fn usage_value_u64(usage: &serde_json::Map<String, Value>, keys: &[&str]) -> Option<u64> {
    keys.iter()
        .find_map(|key| usage.get(*key).and_then(Value::as_u64))
}

fn gemini_incremental_text(seen: &mut String, incoming: &str) -> String {
    let incoming = incoming.trim_end_matches('\0');
    if incoming.is_empty() {
        return String::new();
    }
    if incoming.starts_with(seen.as_str()) {
        let delta = incoming[seen.len()..].to_string();
        *seen = incoming.to_string();
        return delta;
    }
    if seen.starts_with(incoming) {
        return String::new();
    }
    seen.push_str(incoming);
    incoming.to_string()
}

fn gemini_thought_signature(part: &Value) -> Option<&str> {
    part.get("thoughtSignature")
        .or_else(|| part.get("thought_signature"))
        .and_then(Value::as_str)
        .filter(|signature| !signature.is_empty())
}

#[derive(Debug)]
struct GeminiOpenAiState {
    source: GeminiAnthropicState,
    target: GeminiOpenAiTarget,
}

#[derive(Debug)]
enum GeminiOpenAiTarget {
    Responses(AnthropicResponsesState),
    Chat {
        responses: AnthropicResponsesState,
        chat: ResponsesChatState,
    },
}

impl GeminiOpenAiState {
    fn responses(responses_tool_context: transforms::ResponsesToolContext) -> Self {
        Self {
            source: GeminiAnthropicState::default(),
            target: GeminiOpenAiTarget::Responses(AnthropicResponsesState::new(
                responses_tool_context,
            )),
        }
    }

    fn chat(responses_tool_context: transforms::ResponsesToolContext) -> Self {
        Self {
            source: GeminiAnthropicState::default(),
            target: GeminiOpenAiTarget::Chat {
                responses: AnthropicResponsesState::new(responses_tool_context),
                chat: ResponsesChatState::default(),
            },
        }
    }

    fn transform(&mut self, input: &Value) -> Result<Vec<StreamFrame>, ProxyError> {
        let anthropic = self.source.transform(input);
        Ok(self.relay(anthropic))
    }

    fn finish_stream(&mut self) -> Result<Vec<StreamFrame>, ProxyError> {
        if self.completed() {
            return Ok(Vec::new());
        }
        let anthropic = self.source.finish_stream()?;
        let frames = self.relay(anthropic);
        if self.completed() {
            Ok(frames)
        } else {
            Err(ProxyError::bad_gateway(
                "Gemini stream ended before the downstream bridge completed",
            ))
        }
    }

    fn relay(&mut self, anthropic: Vec<StreamFrame>) -> Vec<StreamFrame> {
        match &mut self.target {
            GeminiOpenAiTarget::Responses(responses) => {
                relay_json_frames(anthropic, |event| responses.transform(event))
            }
            GeminiOpenAiTarget::Chat { responses, chat } => {
                let response_events =
                    relay_json_frames(anthropic, |event| responses.transform(event));
                relay_json_frames(response_events, |event| chat.transform(event))
            }
        }
    }

    fn completed(&self) -> bool {
        match &self.target {
            GeminiOpenAiTarget::Responses(state) => state.completed,
            GeminiOpenAiTarget::Chat { chat, .. } => chat.completed,
        }
    }
}

#[derive(Debug)]
struct AnthropicChatState {
    responses: AnthropicResponsesState,
    chat: ResponsesChatState,
}

impl AnthropicChatState {
    fn new(responses_tool_context: transforms::ResponsesToolContext) -> Self {
        Self {
            responses: AnthropicResponsesState::new(responses_tool_context),
            chat: ResponsesChatState::default(),
        }
    }

    fn transform(&mut self, input: &Value) -> Vec<StreamFrame> {
        let response_events = self.responses.transform(input);
        relay_json_frames(response_events, |event| self.chat.transform(event))
    }

    fn completed(&self) -> bool {
        self.chat.completed
    }
}

#[derive(Debug)]
struct ToGeminiState {
    source: ToAnthropicSource,
    target: AnthropicGeminiState,
}

#[derive(Debug)]
enum ToAnthropicSource {
    Anthropic,
    Responses(ResponsesAnthropicState),
    Chat(ChatAnthropicState),
}

impl ToGeminiState {
    fn anthropic() -> Self {
        Self {
            source: ToAnthropicSource::Anthropic,
            target: AnthropicGeminiState::default(),
        }
    }

    fn responses() -> Self {
        Self {
            source: ToAnthropicSource::Responses(ResponsesAnthropicState::deferred_for_gemini()),
            target: AnthropicGeminiState::default(),
        }
    }

    fn chat() -> Self {
        Self {
            source: ToAnthropicSource::Chat(ChatAnthropicState::deferred_for_gemini()),
            target: AnthropicGeminiState::default(),
        }
    }

    fn transform(&mut self, input: &Value) -> Result<Vec<StreamFrame>, ProxyError> {
        let anthropic = match &mut self.source {
            ToAnthropicSource::Anthropic => vec![StreamFrame::json(input.clone())],
            ToAnthropicSource::Responses(state) => state.transform(input),
            ToAnthropicSource::Chat(state) => state.transform(input),
        };
        relay_json_frames_fallible(anthropic, |event| self.target.transform(event))
    }

    fn finish_stream(&mut self) -> Result<Vec<StreamFrame>, ProxyError> {
        if self.completed() {
            return Ok(Vec::new());
        }
        let anthropic = match &mut self.source {
            ToAnthropicSource::Chat(state) => state.finish_stream()?,
            ToAnthropicSource::Anthropic => {
                return Err(ProxyError::bad_gateway(
                    "Anthropic stream ended before message_stop",
                ))
            }
            ToAnthropicSource::Responses(state) if !state.completed => {
                return Err(ProxyError::bad_gateway(
                    "OpenAI Responses stream ended before a terminal response event",
                ))
            }
            ToAnthropicSource::Responses(_) => Vec::new(),
        };
        let frames = relay_json_frames_fallible(anthropic, |event| self.target.transform(event))?;
        if self.completed() {
            Ok(frames)
        } else {
            Err(ProxyError::bad_gateway(
                "upstream stream ended before the Gemini bridge completed",
            ))
        }
    }

    fn completed(&self) -> bool {
        self.target.completed
    }
}

fn relay_json_frames(
    frames: Vec<StreamFrame>,
    mut transform: impl FnMut(&Value) -> Vec<StreamFrame>,
) -> Vec<StreamFrame> {
    let mut output = Vec::new();
    for frame in frames {
        if let transforms::StreamPayload::Json(value) = frame.payload {
            output.extend(transform(&value));
        }
    }
    output
}

fn relay_json_frames_fallible(
    frames: Vec<StreamFrame>,
    mut transform: impl FnMut(&Value) -> Result<Vec<StreamFrame>, ProxyError>,
) -> Result<Vec<StreamFrame>, ProxyError> {
    let mut output = Vec::new();
    for frame in frames {
        if let transforms::StreamPayload::Json(value) = frame.payload {
            output.extend(transform(&value)?);
        }
    }
    Ok(output)
}

#[derive(Debug, Default)]
struct AnthropicGeminiState {
    response_id: String,
    model: String,
    blocks: BTreeMap<i64, AnthropicGeminiBlock>,
    usage: BTreeMap<String, Value>,
    stop_reason: Option<String>,
    completed: bool,
}

#[derive(Debug)]
enum AnthropicGeminiBlock {
    Text,
    Thinking {
        signature: String,
    },
    Tool {
        id: String,
        name: String,
        arguments: String,
        signature: Option<String>,
        saw_delta: bool,
    },
}

impl AnthropicGeminiState {
    fn transform(&mut self, input: &Value) -> Result<Vec<StreamFrame>, ProxyError> {
        if self.completed {
            return Ok(Vec::new());
        }
        match input.get("type").and_then(Value::as_str) {
            Some("message_start") => {
                self.message_start(input);
                Ok(Vec::new())
            }
            Some("content_block_start") => self.content_block_start(input),
            Some("content_block_delta") => self.content_block_delta(input),
            Some("content_block_stop") => self.content_block_stop(input),
            Some("message_delta") => {
                self.message_delta(input);
                Ok(Vec::new())
            }
            Some("message_stop") => self.complete(),
            Some("error") => Err(ProxyError::bad_gateway(format!(
                "upstream stream failed: {}",
                input
                    .pointer("/error/message")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown upstream error")
            ))),
            _ => Ok(Vec::new()),
        }
    }

    fn message_start(&mut self, input: &Value) {
        let message = input.get("message").unwrap_or(input);
        if let Some(id) = message.get("id").and_then(Value::as_str) {
            self.response_id = id.to_string();
        }
        if let Some(model) = message.get("model").and_then(Value::as_str) {
            self.model = model.to_string();
        }
        self.observe_usage(message.get("usage"));
    }

    fn content_block_start(&mut self, input: &Value) -> Result<Vec<StreamFrame>, ProxyError> {
        let index = anthropic_block_index(input)?;
        if self.blocks.contains_key(&index) {
            return Err(ProxyError::bad_gateway(format!(
                "Anthropic stream reopened content block index {index}"
            )));
        }
        let block = input.get("content_block").ok_or_else(|| {
            ProxyError::bad_gateway("Anthropic content_block_start is missing content_block")
        })?;
        let frames = match block.get("type").and_then(Value::as_str) {
            Some("text") => {
                self.blocks.insert(index, AnthropicGeminiBlock::Text);
                block
                    .get("text")
                    .and_then(Value::as_str)
                    .filter(|text| !text.is_empty())
                    .map(|text| vec![self.content_frame(json!({"text": text}))])
                    .unwrap_or_default()
            }
            Some("thinking") => {
                let signature = bridge_thought_signature(block)
                    .unwrap_or_default()
                    .to_string();
                self.blocks
                    .insert(index, AnthropicGeminiBlock::Thinking { signature });
                block
                    .get("thinking")
                    .and_then(Value::as_str)
                    .filter(|text| !text.is_empty())
                    .map(|text| vec![self.content_frame(json!({"text": text, "thought": true}))])
                    .unwrap_or_default()
            }
            Some("redacted_thinking") => {
                let signature = block
                    .get("data")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                self.blocks
                    .insert(index, AnthropicGeminiBlock::Thinking { signature });
                Vec::new()
            }
            Some("tool_use") => {
                let id = block
                    .get("id")
                    .and_then(Value::as_str)
                    .filter(|id| !id.is_empty())
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("tool_{index}"));
                let name = block
                    .get("name")
                    .and_then(Value::as_str)
                    .filter(|name| !name.is_empty())
                    .unwrap_or("tool")
                    .to_string();
                let arguments = block
                    .get("input")
                    .filter(|value| {
                        value.is_object() && value.as_object().is_some_and(|v| !v.is_empty())
                    })
                    .map(Value::to_string)
                    .unwrap_or_default();
                let signature = bridge_thought_signature(block).map(str::to_string);
                self.blocks.insert(
                    index,
                    AnthropicGeminiBlock::Tool {
                        id,
                        name,
                        arguments,
                        signature,
                        saw_delta: false,
                    },
                );
                Vec::new()
            }
            Some(kind) => {
                return Err(ProxyError::bad_gateway(format!(
                    "unsupported Anthropic streaming content block type: {kind}"
                )));
            }
            None => {
                return Err(ProxyError::bad_gateway(
                    "Anthropic streaming content block is missing type",
                ));
            }
        };
        Ok(frames)
    }

    fn content_block_delta(&mut self, input: &Value) -> Result<Vec<StreamFrame>, ProxyError> {
        let index = anthropic_block_index(input)?;
        let delta = input.get("delta").ok_or_else(|| {
            ProxyError::bad_gateway("Anthropic content_block_delta is missing delta")
        })?;
        let Some(block) = self.blocks.get_mut(&index) else {
            return Err(ProxyError::bad_gateway(format!(
                "Anthropic stream referenced unknown content block index {index}"
            )));
        };
        let part = match block {
            AnthropicGeminiBlock::Text => delta
                .get("text")
                .and_then(Value::as_str)
                .filter(|text| !text.is_empty())
                .map(|text| json!({"text": text})),
            AnthropicGeminiBlock::Thinking { signature } => {
                if let Some(fragment) = delta
                    .get("signature")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
                {
                    signature.push_str(fragment);
                }
                delta
                    .get("thinking")
                    .and_then(Value::as_str)
                    .filter(|text| !text.is_empty())
                    .map(|text| json!({"text": text, "thought": true}))
            }
            AnthropicGeminiBlock::Tool {
                arguments,
                saw_delta,
                ..
            } => {
                let Some(fragment) = delta
                    .get("partial_json")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
                else {
                    return Ok(Vec::new());
                };
                if !*saw_delta {
                    arguments.clear();
                    *saw_delta = true;
                }
                arguments.push_str(fragment);
                None
            }
        };
        Ok(part
            .map(|part| vec![self.content_frame(part)])
            .unwrap_or_default())
    }

    fn content_block_stop(&mut self, input: &Value) -> Result<Vec<StreamFrame>, ProxyError> {
        let index = anthropic_block_index(input)?;
        let block = self.blocks.remove(&index).ok_or_else(|| {
            ProxyError::bad_gateway(format!(
                "Anthropic stream closed unknown content block index {index}"
            ))
        })?;
        let part = match block {
            AnthropicGeminiBlock::Text => return Ok(Vec::new()),
            AnthropicGeminiBlock::Thinking { signature } => {
                if signature.is_empty() {
                    return Ok(Vec::new());
                }
                json!({"text": "", "thought": true, "thoughtSignature": signature})
            }
            AnthropicGeminiBlock::Tool {
                id,
                name,
                arguments,
                signature,
                ..
            } => {
                let arguments = if arguments.is_empty() {
                    json!({})
                } else {
                    serde_json::from_str::<Value>(&arguments).map_err(|error| {
                        ProxyError::bad_gateway(format!(
                            "Anthropic tool arguments are not complete JSON: {error}"
                        ))
                    })?
                };
                if !arguments.is_object() {
                    return Err(ProxyError::bad_gateway(
                        "Anthropic tool arguments must decode to a JSON object",
                    ));
                }
                let mut part = json!({
                    "functionCall": {"id": id, "name": name, "args": arguments}
                });
                if let Some(signature) = signature.filter(|value| !value.is_empty()) {
                    part["thoughtSignature"] = Value::String(signature);
                }
                part
            }
        };
        Ok(vec![self.content_frame(part)])
    }

    fn message_delta(&mut self, input: &Value) {
        if let Some(reason) = input.pointer("/delta/stop_reason").and_then(Value::as_str) {
            self.stop_reason = Some(reason.to_string());
        }
        self.observe_usage(input.get("usage"));
    }

    fn complete(&mut self) -> Result<Vec<StreamFrame>, ProxyError> {
        if !self.blocks.is_empty() {
            return Err(ProxyError::bad_gateway(
                "Anthropic stream ended with open content blocks",
            ));
        }
        let stop_reason = self.stop_reason.as_deref().ok_or_else(|| {
            ProxyError::bad_gateway("Anthropic stream ended before message_delta.stop_reason")
        })?;
        let finish_reason = match stop_reason {
            "max_tokens" | "model_context_window_exceeded" => "MAX_TOKENS",
            "refusal" => "SAFETY",
            _ => "STOP",
        };
        let mut terminal = json!({
            "candidates": [{"index": 0, "finishReason": finish_reason}],
            "usageMetadata": gemini_usage_from_anthropic_stream_usage(&self.usage)
        });
        self.attach_identity(&mut terminal);
        self.completed = true;
        Ok(vec![StreamFrame::json(terminal)])
    }

    fn observe_usage(&mut self, usage: Option<&Value>) {
        let Some(object) = usage.and_then(Value::as_object) else {
            return;
        };
        for (key, value) in object {
            if value.is_number() {
                self.usage.insert(key.clone(), value.clone());
            }
        }
    }

    fn content_frame(&self, part: Value) -> StreamFrame {
        let mut frame = json!({
            "candidates": [{
                "index": 0,
                "content": {"role": "model", "parts": [part]}
            }]
        });
        self.attach_identity(&mut frame);
        StreamFrame::json(frame)
    }

    fn attach_identity(&self, frame: &mut Value) {
        if !self.response_id.is_empty() {
            frame["responseId"] = Value::String(self.response_id.clone());
        }
        if !self.model.is_empty() {
            frame["modelVersion"] = Value::String(self.model.clone());
        }
    }
}

fn anthropic_block_index(input: &Value) -> Result<i64, ProxyError> {
    input
        .get("index")
        .and_then(Value::as_i64)
        .ok_or_else(|| ProxyError::bad_gateway("Anthropic stream event is missing block index"))
}

fn gemini_usage_from_anthropic_stream_usage(usage: &BTreeMap<String, Value>) -> Value {
    let fresh_input = usage
        .get("input_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let cache_read = usage
        .get("cache_read_input_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let cache_creation = usage
        .get("cache_creation_input_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let input = fresh_input
        .saturating_add(cache_read)
        .saturating_add(cache_creation);
    let output = usage
        .get("output_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    json!({
        "promptTokenCount": input,
        "candidatesTokenCount": output,
        "totalTokenCount": input.saturating_add(output),
        "cachedContentTokenCount": cache_read
    })
}

fn bridge_thought_signature(value: &Value) -> Option<&str> {
    value
        .get("signature")
        .or_else(|| value.get("thoughtSignature"))
        .or_else(|| value.get("thought_signature"))
        .or_else(|| value.pointer("/extra_content/google/thought_signature"))
        .or_else(|| value.pointer("/function/extra_content/google/thought_signature"))
        .and_then(Value::as_str)
        .filter(|signature| !signature.is_empty())
}

fn merge_bridge_item(target: &mut Value, incoming: &Value) {
    let Some(incoming) = incoming.as_object() else {
        return;
    };
    if !target.is_object() {
        *target = json!({});
    }
    let target = target.as_object_mut().expect("bridge item is an object");
    for (key, value) in incoming {
        if !value.is_null() {
            target.insert(key.clone(), value.clone());
        }
    }
}

fn attach_responses_thought_signature(item: &mut Value, signature: Option<&str>) {
    if let Some(signature) = signature.filter(|signature| !signature.is_empty()) {
        item["thought_signature"] = Value::String(signature.to_string());
    }
}

fn attach_chat_thought_signature(tool_call: &mut Value, signature: Option<&str>) {
    if let Some(signature) = signature.filter(|signature| !signature.is_empty()) {
        tool_call["extra_content"] = json!({"google": {"thought_signature": signature}});
    }
}

#[derive(Debug, Default)]
struct ChatAnthropicState {
    next_block_index: u64,
    message_started: bool,
    text_block: Option<BlockState>,
    reasoning_block: Option<BlockState>,
    reasoning_signature: String,
    tools: BTreeMap<i64, ToolBlockState>,
    deferred_tools: BTreeMap<i64, DeferredChatToolState>,
    usage: Option<Value>,
    pending_finish_reason: Option<String>,
    defer_terminal: bool,
    saw_tool: bool,
    completed: bool,
}

#[derive(Debug, Default)]
struct DeferredChatToolState {
    id: String,
    name: String,
    arguments: String,
    signature: Option<String>,
}

impl ChatAnthropicState {
    fn deferred_for_gemini() -> Self {
        Self {
            defer_terminal: true,
            ..Self::default()
        }
    }

    fn transform(&mut self, input: &Value) -> Vec<StreamFrame> {
        if self.completed {
            return Vec::new();
        }
        let usage_only = input.get("usage").is_some()
            && input
                .get("choices")
                .and_then(Value::as_array)
                .is_some_and(Vec::is_empty);
        if let Some(usage) = input.get("usage") {
            self.usage = Some(usage.clone());
        }
        let Some(choice) = input.pointer("/choices/0") else {
            if self.defer_terminal && usage_only && self.pending_finish_reason.is_some() {
                return self.finish_pending();
            }
            return Vec::new();
        };
        let mut frames = self.ensure_message_start(input);
        let delta = transforms::openai_chat_choice_payload(choice);
        if let Some(reasoning) = chat_reasoning_delta(delta) {
            frames.extend(self.reasoning_delta(reasoning));
        }
        if let Some(signature) = bridge_thought_signature(delta) {
            frames.extend(self.reasoning_signature(signature));
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
                if self.defer_terminal {
                    frames.extend(self.deferred_tool_delta(tool_index, tool_call));
                    continue;
                }
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
                    let mut content_block = json!({
                        "type": "tool_use",
                        "id": tool_call.get("id").and_then(Value::as_str).unwrap_or("tool"),
                        "name": tool_call.pointer("/function/name").and_then(Value::as_str).unwrap_or("tool"),
                        "input": {}
                    });
                    if let Some(signature) = bridge_thought_signature(tool_call) {
                        content_block["signature"] = Value::String(signature.to_string());
                    }
                    frames.push(StreamFrame::event(
                        "content_block_start",
                        json!({
                            "type": "content_block_start",
                            "index": index,
                            "content_block": content_block
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
                if self.defer_terminal {
                    frames.extend(self.deferred_tool_delta(tool_index, &tool_call));
                } else {
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
                        let mut content_block = json!({
                            "type": "tool_use",
                            "id": tool_call.get("id").and_then(Value::as_str).unwrap_or("call_0"),
                            "name": tool_call.pointer("/function/name").and_then(Value::as_str).unwrap_or("tool"),
                            "input": {}
                        });
                        if let Some(signature) = bridge_thought_signature(&tool_call) {
                            content_block["signature"] = Value::String(signature.to_string());
                        }
                        frames.push(StreamFrame::event(
                            "content_block_start",
                            json!({
                                "type": "content_block_start",
                                "index": index,
                                "content_block": content_block
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
        }
        if choice
            .get("finish_reason")
            .is_some_and(|value| !value.is_null())
        {
            self.pending_finish_reason = choice
                .get("finish_reason")
                .and_then(Value::as_str)
                .map(str::to_string);
            if !self.defer_terminal {
                frames.extend(self.finish_pending());
            }
        }
        frames
    }

    fn deferred_tool_delta(&mut self, tool_index: i64, tool_call: &Value) -> Vec<StreamFrame> {
        let mut frames = self.close_text_block();
        frames.extend(self.close_reasoning_block());
        let tool = self.deferred_tools.entry(tool_index).or_default();
        if let Some(id) = tool_call
            .get("id")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            tool.id = id.to_string();
        }
        if let Some(name) = tool_call
            .pointer("/function/name")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            tool.name = name.to_string();
        }
        if let Some(arguments) = tool_call
            .pointer("/function/arguments")
            .and_then(Value::as_str)
        {
            tool.arguments.push_str(arguments);
        }
        if let Some(signature) = bridge_thought_signature(tool_call) {
            tool.signature = Some(signature.to_string());
        }
        self.saw_tool = true;
        frames
    }

    fn flush_deferred_tools(&mut self) -> Vec<StreamFrame> {
        let mut frames = Vec::new();
        let tools = std::mem::take(&mut self.deferred_tools);
        for (tool_index, tool) in tools {
            let index = self.allocate_index();
            let mut content_block = json!({
                "type": "tool_use",
                "id": if tool.id.is_empty() {format!("call_{tool_index}")} else {tool.id},
                "name": if tool.name.is_empty() {"tool".to_string()} else {tool.name},
                "input": {}
            });
            if let Some(signature) = tool.signature {
                content_block["signature"] = Value::String(signature);
            }
            frames.push(StreamFrame::event(
                "content_block_start",
                json!({
                    "type": "content_block_start",
                    "index": index,
                    "content_block": content_block
                }),
            ));
            if !tool.arguments.is_empty() {
                frames.push(input_json_delta(index, &tool.arguments));
            }
            frames.push(content_block_stop(index));
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

    fn reasoning_signature(&mut self, signature: &str) -> Vec<StreamFrame> {
        let mut frames = self.close_text_block();
        if !self.reasoning_block.is_some_and(|block| block.open) {
            let index = self.allocate_index();
            self.reasoning_block = Some(BlockState { index, open: true });
            frames.push(StreamFrame::event(
                "content_block_start",
                json!({"type": "content_block_start", "index": index, "content_block": {"type": "thinking", "thinking": "", "signature": ""}}),
            ));
        }
        self.reasoning_signature.push_str(signature);
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
            self.reasoning_signature.clear();
            return Vec::new();
        };
        block.open = false;
        let mut frames = Vec::new();
        if !self.reasoning_signature.is_empty() {
            frames.push(StreamFrame::event(
                "content_block_delta",
                json!({
                    "type": "content_block_delta",
                    "index": block.index,
                    "delta": {"type": "signature_delta", "signature": std::mem::take(&mut self.reasoning_signature)}
                }),
            ));
        }
        frames.push(content_block_stop(block.index));
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

    fn finish_stream(&mut self) -> Result<Vec<StreamFrame>, ProxyError> {
        if self.completed {
            return Ok(Vec::new());
        }
        if self.pending_finish_reason.is_none() {
            return Err(ProxyError::bad_gateway(
                "OpenAI Chat stream ended before choices[0].finish_reason",
            ));
        }
        Ok(self.finish_pending())
    }

    fn finish_pending(&mut self) -> Vec<StreamFrame> {
        let finish_reason = self
            .pending_finish_reason
            .take()
            .unwrap_or_else(|| "stop".to_string());
        let mut frames = Vec::new();
        if self.text_block.is_none()
            && self.reasoning_block.is_none()
            && self.tools.is_empty()
            && self.deferred_tools.is_empty()
        {
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
        if !matches!(finish_reason.as_str(), "length" | "content_filter") {
            frames.extend(self.flush_deferred_tools());
        } else {
            self.deferred_tools.clear();
        }
        let stop_reason = match finish_reason.as_str() {
            reason @ ("length" | "content_filter") => {
                transforms::openai_finish_reason_to_anthropic(reason)
            }
            _ if self.saw_tool => "tool_use",
            reason => transforms::openai_finish_reason_to_anthropic(reason),
        };
        frames.push(StreamFrame::event(
            "message_delta",
            json!({
                "type": "message_delta",
                "delta": {"stop_reason": stop_reason, "stop_sequence": Value::Null},
                "usage": transforms::anthropic_usage_from_openai_usage(self.usage.as_ref())
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
    thought_signature: Option<String>,
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
            let mut frames = Vec::new();
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
                    frames.extend(self.reasoning_delta(&packed));
                }
            }
            if let Some(signature) = bridge_thought_signature(item) {
                let mut delta = json!({
                    "reasoning_signature": signature,
                    "extra_content": {"google": {"thought_signature": signature}}
                });
                delta["role"] = Value::String("assistant".to_string());
                frames.push(chat_stream_chunk(
                    &self.response_id,
                    &self.model,
                    delta,
                    Value::Null,
                    None,
                ));
            }
            return frames;
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
        if let Some(signature) = bridge_thought_signature(item) {
            state.thought_signature = Some(signature.to_string());
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
        let (call_id, name, arguments, thought_signature) = {
            let state = self
                .tools
                .get_mut(&output_index)
                .expect("tool state exists");
            state.downstream_index = Some(downstream_index);
            state.added = true;
            let arguments = state.arguments.clone();
            state.emitted_arguments = arguments.len();
            (
                state.call_id.clone(),
                state.name.clone(),
                arguments,
                state.thought_signature.clone(),
            )
        };
        let mut tool_call = json!({
            "index": downstream_index,
            "id": call_id,
            "type": "function",
            "function": {"name": name, "arguments": arguments}
        });
        attach_chat_thought_signature(&mut tool_call, thought_signature.as_deref());
        let mut frames = self.ensure_role_chunk();
        frames.push(chat_stream_chunk(
            &self.response_id,
            &self.model,
            json!({"tool_calls": [tool_call]}),
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

fn hosted_search_name(item: &Value) -> Option<&'static str> {
    match item.get("type").and_then(Value::as_str) {
        Some("web_search_call") => Some("web_search"),
        Some("custom_tool_call")
            if item
                .get("name")
                .and_then(Value::as_str)
                .is_some_and(|name| name == "x_search" || name.starts_with("x_")) =>
        {
            Some("x_search")
        }
        _ => None,
    }
}

fn hosted_search_query(input: Value) -> String {
    let input = match input {
        Value::String(value) => {
            serde_json::from_str::<Value>(&value).unwrap_or_else(|_| Value::String(value))
        }
        value => value,
    };
    input
        .get("query")
        .and_then(Value::as_str)
        .or_else(|| input.as_str())
        .unwrap_or_default()
        .to_string()
}

fn hosted_search_citation(annotation: &Value) -> Value {
    json!({
        "type": "web_search_result_location",
        "url": annotation.get("url").and_then(Value::as_str).unwrap_or_default(),
        "title": annotation.get("title").and_then(Value::as_str).unwrap_or_default(),
        "cited_text": annotation.get("text").and_then(Value::as_str).unwrap_or_default()
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
        signature: Option<String>,
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
                let signature = bridge_thought_signature(block).map(str::to_string);
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
                        signature: signature.clone(),
                        custom,
                        done: false,
                    },
                );
                let mut item = self.responses_tool_context.response_item(
                    &item_id,
                    "in_progress",
                    &call_id,
                    &name,
                    "",
                );
                attach_responses_thought_signature(&mut item, signature.as_deref());
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
                let mut item =
                    responses_reasoning_item_from_anthropic_block(item_id, &anthropic_block)
                        .or_else(|| unsigned_responses_reasoning_item(item_id, text))
                        .unwrap_or_else(
                            || json!({"id": item_id, "type": "reasoning", "summary": []}),
                        );
                attach_responses_thought_signature(
                    &mut item,
                    (!signature.is_empty()).then_some(signature.as_str()),
                );
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
                signature,
                custom,
                done,
            } => {
                if *done {
                    return Vec::new();
                }
                *done = true;
                let arguments = canonicalize_arguments(arguments);
                if *custom {
                    let mut item = self.responses_tool_context.response_item(
                        item_id,
                        "completed",
                        call_id,
                        name,
                        &arguments,
                    );
                    attach_responses_thought_signature(&mut item, signature.as_deref());
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
                let mut item = self.responses_tool_context.response_item(
                    item_id,
                    "completed",
                    call_id,
                    name,
                    &arguments,
                );
                attach_responses_thought_signature(&mut item, signature.as_deref());
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

    fn gemini_v1internal_stored_provider(
        app: crate::domain::providers::model::AppKind,
        provider_type: crate::domain::providers::model::ProviderType,
    ) -> StoredProvider {
        StoredProvider {
            app,
            provider: crate::domain::providers::model::Provider {
                id: format!("{}-stream-fixture", provider_type.as_str()),
                name: provider_type.as_str().to_string(),
                settings_config: json!({}),
                category: None,
                meta: None,
                extra: Default::default(),
            },
            provider_type,
            provider_type_id: provider_type.as_str().to_string(),
            resource: Default::default(),
        }
    }

    fn responses_transformer() -> StreamEventTransformer {
        StreamEventTransformer {
            upstream: Some(UpstreamFormat::OpenAiResponses),
            downstream: UpstreamFormat::AnthropicMessages,
            buffer: Vec::new(),
            responses_tool_context: Default::default(),
            bridge: Some(StreamBridgeState::ResponsesAnthropic(
                ResponsesAnthropicState::default(),
            )),
            unwrap_v1internal: false,
            gemini_terminal: None,
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
    fn gemini_v1internal_gemini_cli_stream_unwraps_at_every_chunk_boundary() {
        let wire = concat!(
            "data: {\"response\":{\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"hello\"}]},\"finishReason\":\"STOP\"}],",
            "\"usageMetadata\":{\"promptTokenCount\":2,\"candidatesTokenCount\":1}}}\r\n\r\n"
        );
        let stored = gemini_v1internal_stored_provider(
            crate::domain::providers::model::AppKind::Gemini,
            crate::domain::providers::model::ProviderType::GeminiCli,
        );

        for split in 1..wire.len() {
            let mut transformer = StreamEventTransformer::new(
                &stored,
                ProxyRoute::Gemini,
                transforms::ResponsesToolContext::default(),
            );
            let mut output = transformer
                .push(Bytes::copy_from_slice(&wire.as_bytes()[..split]))
                .unwrap()
                .to_vec();
            output.extend_from_slice(
                &transformer
                    .push(Bytes::copy_from_slice(&wire.as_bytes()[split..]))
                    .unwrap(),
            );
            output.extend_from_slice(&transformer.finish().unwrap());
            let output = String::from_utf8(output).unwrap();
            assert!(output.contains("\"text\":\"hello\""), "split={split}");
            assert!(
                output.contains("\"finishReason\":\"STOP\""),
                "split={split}"
            );
            assert!(output.contains("\"promptTokenCount\":2"), "split={split}");
            assert!(!output.contains("\"response\":"), "split={split}");
        }
    }

    #[test]
    fn gemini_v1internal_same_format_rejects_truncated_eof_and_done() {
        let stored = gemini_v1internal_stored_provider(
            crate::domain::providers::model::AppKind::Gemini,
            crate::domain::providers::model::ProviderType::GeminiCli,
        );
        let partial = Bytes::from_static(
            b"data: {\"response\":{\"candidates\":[{\"index\":0,\"content\":{\"parts\":[{\"text\":\"partial\"}]}}]}}\n\n",
        );

        let mut eof = StreamEventTransformer::new(
            &stored,
            ProxyRoute::Gemini,
            transforms::ResponsesToolContext::default(),
        );
        assert!(!eof.push(partial.clone()).unwrap().is_empty());
        let error = eof.finish().unwrap_err();
        assert_eq!(error.status, axum::http::StatusCode::BAD_GATEWAY);
        assert!(error.message.contains("terminal candidate"));

        let mut done = StreamEventTransformer::new(
            &stored,
            ProxyRoute::Gemini,
            transforms::ResponsesToolContext::default(),
        );
        assert!(!done.push(partial).unwrap().is_empty());
        let error = done
            .push(Bytes::from_static(b"data: [DONE]\n\n"))
            .unwrap_err();
        assert_eq!(error.status, axum::http::StatusCode::BAD_GATEWAY);
        assert!(error.message.contains("terminal candidate"));
    }

    #[test]
    fn gemini_v1internal_same_format_accepts_blocked_prompt_terminal() {
        let stored = gemini_v1internal_stored_provider(
            crate::domain::providers::model::AppKind::Gemini,
            crate::domain::providers::model::ProviderType::GeminiCli,
        );
        let mut transformer = StreamEventTransformer::new(
            &stored,
            ProxyRoute::Gemini,
            transforms::ResponsesToolContext::default(),
        );
        let output = transformer
            .push(Bytes::from_static(
                b"data: {\"response\":{\"promptFeedback\":{\"blockReason\":\"SAFETY\"}}}\n\n",
            ))
            .unwrap();
        assert!(String::from_utf8_lossy(&output).contains("\"blockReason\":\"SAFETY\""));
        assert!(transformer.finish().unwrap().is_empty());
    }

    #[test]
    fn gemini_v1internal_blocked_prompt_bridges_to_claude_refusal() {
        let stored = gemini_v1internal_stored_provider(
            crate::domain::providers::model::AppKind::Claude,
            crate::domain::providers::model::ProviderType::AntigravityOAuth,
        );
        let mut transformer = StreamEventTransformer::new(
            &stored,
            ProxyRoute::ClaudeMessages,
            transforms::ResponsesToolContext::default(),
        );
        let mut output = transformer
            .push(Bytes::from_static(
                b"data: {\"response\":{\"responseId\":\"blocked\",\"modelVersion\":\"gemini-2.5-flash\",\"promptFeedback\":{\"blockReason\":\"SAFETY\"},\"usageMetadata\":{\"promptTokenCount\":4,\"candidatesTokenCount\":0}}}\n\n",
            ))
            .unwrap()
            .to_vec();
        output.extend_from_slice(&transformer.finish().unwrap());
        let output = String::from_utf8(output).unwrap();

        assert!(output.contains("event: message_start"));
        assert!(output.contains("\"stop_reason\":\"refusal\""));
        assert!(output.contains("event: message_stop"));
        assert!(output.contains("\"input_tokens\":4"));
        assert!(!output.contains("event: content_block_start"));
    }

    #[test]
    fn gemini_v1internal_candidate_safety_stop_bridges_to_claude_refusal() {
        let stored = gemini_v1internal_stored_provider(
            crate::domain::providers::model::AppKind::Claude,
            crate::domain::providers::model::ProviderType::AntigravityOAuth,
        );
        let mut transformer = StreamEventTransformer::new(
            &stored,
            ProxyRoute::ClaudeMessages,
            transforms::ResponsesToolContext::default(),
        );
        let mut output = transformer
            .push(Bytes::from_static(
                b"data: {\"response\":{\"responseId\":\"candidate-blocked\",\"modelVersion\":\"gemini-2.5-flash\",\"candidates\":[{\"finishReason\":\"SAFETY\"}],\"usageMetadata\":{\"promptTokenCount\":4,\"candidatesTokenCount\":0}}}\n\n",
            ))
            .unwrap()
            .to_vec();
        output.extend_from_slice(&transformer.finish().unwrap());
        let output = String::from_utf8(output).unwrap();

        assert!(output.contains("event: message_start"));
        assert!(output.contains("\"stop_reason\":\"refusal\""));
        assert!(output.contains("event: message_stop"));
        assert!(output.contains("\"input_tokens\":4"));
        assert!(!output.contains("\"finishReason\":\"SAFETY\""));
    }

    #[test]
    fn antigravity_v1internal_gemini_stream_bridges_to_claude_after_unwrap() {
        let stored = gemini_v1internal_stored_provider(
            crate::domain::providers::model::AppKind::Claude,
            crate::domain::providers::model::ProviderType::AntigravityOAuth,
        );
        let wire = concat!(
            "data: {\"response\":{\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"bridged\"}]},\"finishReason\":\"STOP\"}],",
            "\"usageMetadata\":{\"promptTokenCount\":4,\"candidatesTokenCount\":3,\"thoughtsTokenCount\":2}}}\n\n"
        );
        let mut transformer = StreamEventTransformer::new(
            &stored,
            ProxyRoute::ClaudeMessages,
            transforms::ResponsesToolContext::default(),
        );
        let mut output = Vec::new();
        for chunk in wire.as_bytes().chunks(11) {
            output.extend_from_slice(&transformer.push(Bytes::copy_from_slice(chunk)).unwrap());
        }
        output.extend_from_slice(&transformer.finish().unwrap());
        let output = String::from_utf8(output).unwrap();

        let message_start = output.find("event: message_start").unwrap();
        let content_start = output.find("event: content_block_start").unwrap();
        let content_delta = output.find("event: content_block_delta").unwrap();
        let content_stop = output.find("event: content_block_stop").unwrap();
        let message_delta = output.find("event: message_delta").unwrap();
        let message_stop = output.find("event: message_stop").unwrap();
        assert!(message_start < content_start);
        assert!(content_start < content_delta);
        assert!(content_delta < content_stop);
        assert!(content_stop < message_delta);
        assert!(message_delta < message_stop);
        assert!(output.contains("\"type\":\"text_delta\""));
        assert!(output.contains("\"text\":\"bridged\""));
        assert!(output.contains("\"input_tokens\":4"));
        assert!(output.contains("\"output_tokens\":5"));
        assert!(!output.contains("\"response\":"));
    }

    #[test]
    fn gemini_v1internal_embedded_errors_fail_before_stream_unwrap() {
        let stored = gemini_v1internal_stored_provider(
            crate::domain::providers::model::AppKind::Claude,
            crate::domain::providers::model::ProviderType::AntigravityOAuth,
        );
        for event in [
            "data: {\"error\":{\"code\":429,\"status\":\"RESOURCE_EXHAUSTED\",\"message\":\"busy\"}}\n\n",
            "data: {\"response\":{\"error\":{\"code\":403,\"status\":\"PERMISSION_DENIED\",\"message\":\"denied\"}}}\n\n",
        ] {
            let mut transformer = StreamEventTransformer::new(
                &stored,
                ProxyRoute::ClaudeMessages,
                transforms::ResponsesToolContext::default(),
            );
            let error = transformer.push(Bytes::from(event)).unwrap_err();
            assert_eq!(error.status, axum::http::StatusCode::BAD_GATEWAY);
            assert!(error.message.contains("embedded error"));
        }
    }

    #[test]
    fn non_gemini_error_events_keep_their_protocol_bridge() {
        let mut transformer = responses_transformer();
        let output = transformer
            .push(Bytes::from_static(
                b"data: {\"type\":\"response.failed\",\"response\":{\"id\":\"resp-failed\",\"status\":\"failed\",\"error\":{\"message\":\"upstream failed\"}}}\n\n",
            ))
            .unwrap();
        let output = String::from_utf8(output.to_vec()).unwrap();

        assert!(output.contains("\"type\":\"upstream_error\""));
        assert!(output.contains("upstream failed"));
        assert!(!output.contains("Google upstream embedded error"));
    }

    #[test]
    fn gemini_anthropic_bridge_closes_tool_block_and_reports_tool_stop() {
        let mut state = GeminiAnthropicState::default();
        let mut frames = state.transform(&json!({
            "responseId": "gem-tool",
            "modelVersion": "gemini-test",
            "candidates": [{
                "content": {"parts": [{
                    "functionCall": {
                        "id": "call_lookup",
                        "name": "lookup",
                        "args": {"query": "server"}
                    }
                }]},
                "finishReason": "STOP"
            }],
            "usageMetadata": {"promptTokenCount": 2, "candidatesTokenCount": 1}
        }));
        frames.extend(state.finish_stream().unwrap());
        let types = frames
            .iter()
            .map(|frame| frame.payload_json()["type"].as_str().unwrap().to_string())
            .collect::<Vec<_>>();
        assert_eq!(
            types,
            [
                "message_start",
                "content_block_start",
                "content_block_delta",
                "content_block_stop",
                "message_delta",
                "message_stop"
            ]
        );
        assert_eq!(frames[4].payload_json()["delta"]["stop_reason"], "tool_use");
        assert!(state.completed);
    }

    #[test]
    fn gemini_anthropic_bridge_separates_signed_thinking_text_and_tool_blocks() {
        let mut state = GeminiAnthropicState::default();
        let mut frames = state.transform(&json!({
            "responseId": "gem-thought",
            "modelVersion": "gemini-3-pro-preview",
            "candidates": [{
                "content": {"parts": [{"text": "private ", "thought": true}]}
            }]
        }));
        frames.extend(state.transform(&json!({
            "candidates": [{
                "content": {"parts": [{
                    "text": "plan",
                    "thought": true,
                    "thoughtSignature": "thought-signature"
                }]}
            }]
        })));
        frames.extend(state.transform(&json!({
            "candidates": [{
                "content": {"parts": [{"text": "visible answer"}]}
            }]
        })));
        frames.extend(state.transform(&json!({
            "candidates": [{
                "content": {"parts": [{
                    "functionCall": {
                        "id": "call_lookup",
                        "name": "lookup",
                        "args": {"query": "server"}
                    },
                    "thoughtSignature": "tool-signature"
                }]},
                "finishReason": "STOP"
            }],
            "usageMetadata": {"promptTokenCount": 3, "candidatesTokenCount": 2}
        })));
        frames.extend(state.finish_stream().unwrap());

        let starts = frames
            .iter()
            .filter(|frame| frame.payload_json()["type"] == "content_block_start")
            .map(|frame| frame.payload_json()["content_block"].clone())
            .collect::<Vec<_>>();
        assert_eq!(
            starts
                .iter()
                .map(|block| block["type"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["thinking", "text", "tool_use"]
        );
        assert_eq!(starts[2]["id"], "call_lookup");
        assert_eq!(starts[2]["signature"], "tool-signature");

        let thinking = frames
            .iter()
            .filter(|frame| {
                frame
                    .payload_json()
                    .pointer("/delta/type")
                    .and_then(Value::as_str)
                    == Some("thinking_delta")
            })
            .map(|frame| frame.payload_json()["delta"]["thinking"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(thinking, ["private ", "plan"]);

        let visible = frames
            .iter()
            .filter(|frame| {
                frame
                    .payload_json()
                    .pointer("/delta/type")
                    .and_then(Value::as_str)
                    == Some("text_delta")
            })
            .map(|frame| frame.payload_json()["delta"]["text"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(visible, ["visible answer"]);
        assert!(frames.iter().any(|frame| {
            frame
                .payload_json()
                .pointer("/delta/signature")
                .and_then(Value::as_str)
                == Some("thought-signature")
        }));

        let thinking_stop = frames
            .iter()
            .position(|frame| {
                frame.payload_json()["type"] == "content_block_stop"
                    && frame.payload_json()["index"] == 0
            })
            .unwrap();
        let text_start = frames
            .iter()
            .position(|frame| {
                frame.payload_json()["type"] == "content_block_start"
                    && frame.payload_json()["content_block"]["type"] == "text"
            })
            .unwrap();
        assert!(thinking_stop < text_start);
        assert_eq!(
            frames
                .iter()
                .find(|frame| frame.payload_json()["type"] == "message_delta")
                .unwrap()
                .payload_json()["delta"]["stop_reason"],
            "tool_use"
        );
        assert!(state.completed);
    }

    #[test]
    fn gemini_cross_protocol_routes_use_stateful_bridges() {
        use crate::domain::providers::model::{AppKind, ProviderType};

        let cases = [
            (
                AppKind::Codex,
                ProviderType::Gemini,
                ProxyRoute::CodexResponses,
                "gemini_openai",
            ),
            (
                AppKind::Codex,
                ProviderType::Gemini,
                ProxyRoute::CodexChatCompletions,
                "gemini_openai",
            ),
            (
                AppKind::Gemini,
                ProviderType::Claude,
                ProxyRoute::Gemini,
                "to_gemini",
            ),
            (
                AppKind::Gemini,
                ProviderType::Codex,
                ProxyRoute::Gemini,
                "to_gemini",
            ),
            (
                AppKind::Gemini,
                ProviderType::OpenRouter,
                ProxyRoute::Gemini,
                "to_gemini",
            ),
            (
                AppKind::Gemini,
                ProviderType::GitHubCopilot,
                ProxyRoute::Gemini,
                "to_gemini",
            ),
        ];
        for (app, provider_type, route, expected) in cases {
            let stored = gemini_v1internal_stored_provider(app, provider_type);
            let transformer = StreamEventTransformer::new(
                &stored,
                route,
                transforms::ResponsesToolContext::default(),
            );
            assert!(matches!(
                (expected, transformer.bridge.as_ref()),
                ("gemini_openai", Some(StreamBridgeState::GeminiOpenAi(_)))
                    | ("to_gemini", Some(StreamBridgeState::ToGemini(_)))
            ));
        }
    }

    #[test]
    fn gemini_cross_protocol_to_openai_preserves_content_tools_usage_and_terminal() {
        let responses = gemini_openai_fixture(GeminiOpenAiState::responses(
            transforms::ResponsesToolContext::default(),
        ));
        let response_json = json_stream_frames(&responses);
        assert_eq!(
            response_json
                .iter()
                .filter_map(
                    |value| (value["type"] == "response.reasoning_summary_text.delta")
                        .then(|| value["delta"].as_str())
                        .flatten()
                )
                .collect::<String>(),
            "private plan"
        );
        assert_eq!(
            response_json
                .iter()
                .filter_map(|value| (value["type"] == "response.output_text.delta")
                    .then(|| value["delta"].as_str())
                    .flatten())
                .collect::<String>(),
            "hello"
        );
        let reasoning = response_json
            .iter()
            .find(|value| {
                value["type"] == "response.output_item.done" && value["item"]["type"] == "reasoning"
            })
            .unwrap();
        assert_eq!(reasoning["item"]["thought_signature"], "thought-signature");
        let tool = response_json
            .iter()
            .find(|value| {
                value["type"] == "response.output_item.done"
                    && value["item"]["type"] == "function_call"
            })
            .unwrap();
        assert_eq!(tool["output_index"], 2);
        assert_eq!(tool["item"]["call_id"], "call_lookup");
        assert_eq!(tool["item"]["arguments"], r#"{"query":"server"}"#);
        assert_eq!(tool["item"]["thought_signature"], "tool-signature");
        let terminal = response_json
            .iter()
            .find(|value| value["type"] == "response.completed")
            .unwrap();
        assert_eq!(terminal["response"]["usage"]["input_tokens"], 8);
        assert_eq!(terminal["response"]["usage"]["output_tokens"], 5);
        assert_eq!(done_frame_count(&responses), 1);

        let chat = gemini_openai_fixture(GeminiOpenAiState::chat(
            transforms::ResponsesToolContext::default(),
        ));
        let chat_json = json_stream_frames(&chat);
        assert_eq!(
            chat_json
                .iter()
                .filter_map(|value| value
                    .pointer("/choices/0/delta/content")
                    .and_then(Value::as_str))
                .collect::<String>(),
            "hello"
        );
        assert_eq!(
            chat_json
                .iter()
                .filter_map(|value| value
                    .pointer("/choices/0/delta/reasoning_content")
                    .and_then(Value::as_str))
                .collect::<String>(),
            "private plan"
        );
        assert!(chat_json.iter().any(|value| {
            value
                .pointer("/choices/0/delta/reasoning_signature")
                .and_then(Value::as_str)
                == Some("thought-signature")
        }));
        let tool_call = chat_json
            .iter()
            .find_map(|value| value.pointer("/choices/0/delta/tool_calls/0"))
            .unwrap();
        assert_eq!(tool_call["index"], 0);
        assert_eq!(tool_call["id"], "call_lookup");
        assert_eq!(
            tool_call.pointer("/extra_content/google/thought_signature"),
            Some(&json!("tool-signature"))
        );
        assert_eq!(
            chat_json
                .iter()
                .filter_map(|value| value
                    .pointer("/choices/0/delta/tool_calls/0/function/arguments")
                    .and_then(Value::as_str))
                .collect::<String>(),
            r#"{"query":"server"}"#
        );
        let terminal = chat_json
            .iter()
            .find(|value| value.pointer("/choices/0/finish_reason") == Some(&json!("tool_calls")))
            .unwrap();
        assert_eq!(terminal["usage"]["prompt_tokens"], 8);
        assert_eq!(terminal["usage"]["completion_tokens"], 5);
        assert_eq!(done_frame_count(&chat), 1);
    }

    #[test]
    fn gemini_cross_protocol_from_all_sources_preserves_native_semantics() {
        let mut anthropic = ToGeminiState::anthropic();
        let anthropic_frames = drive_to_gemini(
            &mut anthropic,
            &[
                json!({"type":"message_start","message":{"id":"gem-a","model":"claude","usage":{"input_tokens":4,"cache_read_input_tokens":1}}}),
                json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}),
                json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"plan"}}),
                json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"anthropic-signature"}}),
                json!({"type":"content_block_stop","index":0}),
                json!({"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}),
                json!({"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"answer"}}),
                json!({"type":"content_block_stop","index":1}),
                json!({"type":"content_block_start","index":2,"content_block":{"type":"tool_use","id":"call_a","name":"lookup","input":{},"signature":"tool-a"}}),
                json!({"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"{\"q\":1}"}}),
                json!({"type":"content_block_stop","index":2}),
                json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":3}}),
                json!({"type":"message_stop"}),
            ],
        );
        assert_native_gemini_frames(
            &anthropic_frames,
            NativeGeminiExpectation {
                thinking: "plan",
                text: "answer",
                call_id: "call_a",
                thinking_signature: "anthropic-signature",
                tool_signature: "tool-a",
                input_tokens: 5,
                output_tokens: 3,
            },
        );

        let mut responses = ToGeminiState::responses();
        let responses_frames = drive_to_gemini(
            &mut responses,
            &[
                json!({"type":"response.created","response":{"id":"resp-g","model":"gpt"}}),
                json!({"type":"response.reasoning_summary_text.delta","output_index":0,"delta":"plan"}),
                json!({"type":"response.output_item.done","output_index":0,"item":{"id":"rs_0","type":"reasoning","summary":[{"type":"summary_text","text":"plan"}],"thought_signature":"responses-signature"}}),
                json!({"type":"response.output_text.delta","output_index":1,"delta":"answer"}),
                json!({"type":"response.output_item.added","output_index":2,"item":{"type":"function_call","call_id":"call_r","name":"lookup","thought_signature":"tool-r"}}),
                json!({"type":"response.function_call_arguments.delta","output_index":2,"delta":"{\"q\":2}"}),
                json!({"type":"response.output_item.done","output_index":2,"item":{"type":"function_call","call_id":"call_r","name":"lookup","arguments":"{\"q\":2}","thought_signature":"tool-r"}}),
                json!({"type":"response.completed","response":{"status":"completed","usage":{"input_tokens":6,"output_tokens":4}}}),
            ],
        );
        assert_native_gemini_frames(
            &responses_frames,
            NativeGeminiExpectation {
                thinking: "plan",
                text: "answer",
                call_id: "call_r",
                thinking_signature: "responses-signature",
                tool_signature: "tool-r",
                input_tokens: 6,
                output_tokens: 4,
            },
        );

        let mut chat = ToGeminiState::chat();
        let chat_frames = drive_to_gemini(
            &mut chat,
            &[
                json!({"id":"chat-g","model":"chat","choices":[{"delta":{"reasoning_content":"plan","extra_content":{"google":{"thought_signature":"chat-signature"}}},"finish_reason":null}]}),
                json!({"choices":[{"delta":{"content":"answer"},"finish_reason":null}]}),
                json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_c","type":"function","function":{"name":"lookup","arguments":"{\"q\":"},"extra_content":{"google":{"thought_signature":"tool-c"}}}]},"finish_reason":null}]}),
                json!({"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"3}"}}]},"finish_reason":null}]}),
                json!({"choices":[{"delta":{},"finish_reason":"tool_calls"}]}),
                json!({"choices":[],"usage":{"prompt_tokens":7,"completion_tokens":5,"total_tokens":12,"prompt_tokens_details":{"cached_tokens":2}}}),
            ],
        );
        assert_native_gemini_frames(
            &chat_frames,
            NativeGeminiExpectation {
                thinking: "plan",
                text: "answer",
                call_id: "call_c",
                thinking_signature: "chat-signature",
                tool_signature: "tool-c",
                input_tokens: 7,
                output_tokens: 5,
            },
        );
    }

    #[test]
    fn copilot_chat_stream_to_gemini_preserves_tool_usage_and_single_terminal() {
        use crate::domain::providers::model::{AppKind, ProviderType};

        let stored =
            gemini_v1internal_stored_provider(AppKind::Gemini, ProviderType::GitHubCopilot);
        let mut transformer = StreamEventTransformer::new(
            &stored,
            ProxyRoute::Gemini,
            transforms::ResponsesToolContext::default(),
        );
        let chunks = [
            Bytes::from_static(b"data: {\"id\":\"chat-copilot\",\"model\":\"gemini-3.5-flash\",\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_lookup\",\"type\":\"function\",\"function\":{\"name\":\"lookup\",\"arguments\":\"{\\\"q\\\":\"}}]},\"finish_reason\":null}]}\n\n"),
            Bytes::from_static(b"data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"\\\"rust\\\"}\"}}]},\"finish_reason\":null}]}\n\n"),
            Bytes::from_static(b"data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n"),
            Bytes::from_static(b"data: {\"choices\":[],\"usage\":{\"prompt_tokens\":7,\"completion_tokens\":5,\"total_tokens\":12,\"prompt_tokens_details\":{\"cached_tokens\":2}}}\n\n"),
            Bytes::from_static(b"data: [DONE]\n\n"),
        ];
        let mut output = Vec::new();
        for chunk in chunks {
            output.extend_from_slice(&transformer.push(chunk).unwrap());
        }
        output.extend_from_slice(&transformer.finish().unwrap());
        let output = String::from_utf8(output).unwrap();
        let payloads = output
            .lines()
            .filter_map(|line| line.strip_prefix("data: "))
            .filter(|payload| *payload != "[DONE]")
            .map(|payload| serde_json::from_str::<Value>(payload).unwrap())
            .collect::<Vec<_>>();
        let tool = payloads
            .iter()
            .find_map(|payload| payload.pointer("/candidates/0/content/parts/0/functionCall"))
            .unwrap();
        assert_eq!(tool["id"], "call_lookup");
        assert_eq!(tool["name"], "lookup");
        assert_eq!(tool["args"]["q"], "rust");
        let terminals = payloads
            .iter()
            .filter(|payload| payload.pointer("/candidates/0/finishReason") == Some(&json!("STOP")))
            .collect::<Vec<_>>();
        assert_eq!(terminals.len(), 1);
        assert_eq!(terminals[0]["usageMetadata"]["promptTokenCount"], 7);
        assert_eq!(terminals[0]["usageMetadata"]["cachedContentTokenCount"], 2);
        assert_eq!(terminals[0]["usageMetadata"]["candidatesTokenCount"], 5);
    }

    #[test]
    fn gemini_cross_protocol_refusal_terminal_and_unexpected_eof_fail_closed() {
        for mut state in [
            GeminiOpenAiState::responses(transforms::ResponsesToolContext::default()),
            GeminiOpenAiState::chat(transforms::ResponsesToolContext::default()),
        ] {
            let initial = state
                .transform(&json!({
                    "responseId":"blocked",
                    "promptFeedback":{"blockReason":"SAFETY"}
                }))
                .unwrap();
            assert_eq!(done_frame_count(&initial), 0);
            assert!(!json_stream_frames(&initial).iter().any(|value| {
                value
                    .get("type")
                    .and_then(Value::as_str)
                    .is_some_and(|event| {
                        matches!(event, "response.incomplete" | "response.completed")
                    })
                    || value
                        .pointer("/choices/0/finish_reason")
                        .is_some_and(|reason| !reason.is_null())
            }));
            assert!(state
                .transform(&json!({
                    "usageMetadata":{"promptTokenCount":2,"totalTokenCount":7}
                }))
                .unwrap()
                .is_empty());
            let frames = state.finish_stream().unwrap();
            let payloads = json_stream_frames(&frames);
            assert!(payloads.iter().any(|value| {
                value.pointer("/response/incomplete_details/reason")
                    == Some(&json!("content_filter"))
                    || value.pointer("/choices/0/finish_reason") == Some(&json!("content_filter"))
            }));
            assert!(payloads.iter().any(|value| {
                value.pointer("/response/usage/input_tokens") == Some(&json!(2))
                    && value.pointer("/response/usage/output_tokens") == Some(&json!(5))
                    || value.pointer("/usage/prompt_tokens") == Some(&json!(2))
                        && value.pointer("/usage/completion_tokens") == Some(&json!(5))
            }));
            assert_eq!(done_frame_count(&frames), 1);
            assert!(state.finish_stream().unwrap().is_empty());
        }

        for mut state in [
            ToGeminiState::anthropic(),
            ToGeminiState::responses(),
            ToGeminiState::chat(),
        ] {
            let events = match &state.source {
                ToAnthropicSource::Anthropic => {
                    vec![json!({"type":"message_start","message":{"id":"partial"}})]
                }
                ToAnthropicSource::Responses(_) => {
                    vec![json!({"type":"response.output_text.delta","delta":"partial"})]
                }
                ToAnthropicSource::Chat(_) => {
                    vec![json!({"choices":[{"delta":{"content":"partial"},"finish_reason":null}]})]
                }
            };
            drive_to_gemini(&mut state, &events);
            let mut bridge = StreamBridgeState::ToGemini(Box::new(state));
            assert!(bridge.finish_eof().is_err());
        }

        let mut gemini = StreamBridgeState::GeminiOpenAi(Box::new(GeminiOpenAiState::responses(
            transforms::ResponsesToolContext::default(),
        )));
        gemini
            .transform(&json!({"candidates":[{"content":{"parts":[{"text":"partial"}]}}]}))
            .unwrap();
        assert!(gemini.finish_eof().is_err());
    }

    #[test]
    fn gemini_anthropic_tracks_same_name_idless_tools_and_merges_nested_arguments() {
        let mut state = GeminiAnthropicState::default();
        let first = json!({
            "responseId":"gem-parallel",
            "candidates":[{"index":0,"content":{"parts":[
                {"functionCall":{"name":"lookup","args":{"query":{"left":"a"}}}},
                {"functionCall":{"name":"lookup","args":{"query":{"left":"b"}}}}
            ]}}]
        });
        let second = json!({
            "candidates":[{"index":0,"content":{"parts":[
                {"functionCall":{"name":"lookup","args":{"query":{"right":1}}}},
                {"functionCall":{"name":"lookup","args":{"query":{"right":2}}}}
            ]},"finishReason":"STOP"}]
        });
        let mut frames = state.transform(&first);
        frames.extend(state.transform(&second));
        frames.extend(state.finish_stream().unwrap());

        let starts = json_stream_frames(&frames)
            .into_iter()
            .filter(|value| {
                value["type"] == "content_block_start"
                    && value["content_block"]["type"] == "tool_use"
            })
            .map(|value| value["content_block"].clone())
            .collect::<Vec<_>>();
        assert_eq!(starts.len(), 2);
        assert_ne!(starts[0]["id"], starts[1]["id"]);

        let arguments = json_stream_frames(&frames)
            .into_iter()
            .filter_map(|value| {
                (value.pointer("/delta/type") == Some(&json!("input_json_delta")))
                    .then(|| value.pointer("/delta/partial_json")?.as_str())
                    .flatten()
            })
            .map(|arguments| serde_json::from_str::<Value>(arguments).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            arguments,
            [
                json!({"query":{"left":"a","right":1}}),
                json!({"query":{"left":"b","right":2}}),
            ]
        );
    }

    #[test]
    fn gemini_truncation_reason_wins_over_tool_output() {
        for (finish_reason, expected_stop) in [("SAFETY", "refusal"), ("MAX_TOKENS", "max_tokens")]
        {
            let mut state = GeminiAnthropicState::default();
            let mut frames = state.transform(&json!({
                "candidates":[{
                    "content":{"parts":[{"functionCall":{"name":"lookup","args":{"q":1}}}]},
                    "finishReason":finish_reason
                }]
            }));
            frames.extend(state.finish_stream().unwrap());
            assert!(json_stream_frames(&frames).iter().any(|value| {
                value.pointer("/delta/stop_reason") == Some(&json!(expected_stop))
            }));
        }

        let mut responses =
            GeminiOpenAiState::responses(transforms::ResponsesToolContext::default());
        responses
            .transform(&json!({
                "candidates":[{
                    "content":{"parts":[{"functionCall":{"name":"lookup","args":{"q":1}}}]},
                    "finishReason":"MAX_TOKENS"
                }]
            }))
            .unwrap();
        let terminal = responses.finish_stream().unwrap();
        assert!(json_stream_frames(&terminal)
            .iter()
            .any(|value| value["type"] == "response.incomplete"));
        assert!(!json_stream_frames(&terminal)
            .iter()
            .any(|value| value["type"] == "response.completed"));
    }

    #[test]
    fn gemini_usage_only_frames_merge_around_finish_and_emit_one_terminal() {
        for usage_after_finish in [false, true] {
            let mut state = GeminiAnthropicState::default();
            let usage = json!({
                "usageMetadata":{"promptTokenCount":3,"totalTokenCount":8}
            });
            if !usage_after_finish {
                assert!(state.transform(&usage).is_empty());
            }
            let mut frames = state.transform(&json!({
                "responseId":"gem-usage",
                "candidates":[{"finishReason":"STOP"}]
            }));
            if usage_after_finish {
                assert!(state.transform(&usage).is_empty());
            }
            frames.extend(state.finish_stream().unwrap());
            frames.extend(state.finish_stream().unwrap());

            let terminals = json_stream_frames(&frames)
                .into_iter()
                .filter(|value| value["type"] == "message_delta")
                .collect::<Vec<_>>();
            assert_eq!(terminals.len(), 1);
            assert_eq!(terminals[0]["usage"]["input_tokens"], 3);
            assert_eq!(terminals[0]["usage"]["output_tokens"], 5);
        }
    }

    #[test]
    fn to_gemini_waits_for_chat_usage_tail_and_late_tool_signatures() {
        let mut state = ToGeminiState::chat();
        let first = state
            .transform(&json!({
                "id":"chat-late",
                "choices":[{"delta":{"tool_calls":[{
                    "index":0,
                    "id":"call_late",
                    "function":{"name":"lookup","arguments":"{\"q\":"}
                }]},"finish_reason":null}]
            }))
            .unwrap();
        assert!(!state.completed());
        let finish = state
            .transform(&json!({
                "choices":[{"delta":{"tool_calls":[{
                    "index":0,
                    "function":{"arguments":"1}"},
                    "extra_content":{"google":{"thought_signature":"late-chat-signature"}}
                }]},"finish_reason":"tool_calls"}]
            }))
            .unwrap();
        assert!(!state.completed());
        assert!(!json_stream_frames(&first)
            .iter()
            .chain(json_stream_frames(&finish).iter())
            .any(|value| value.pointer("/candidates/0/finishReason").is_some()));

        let terminal = state
            .transform(&json!({
                "choices":[],
                "usage":{"prompt_tokens":4,"completion_tokens":2,"total_tokens":6}
            }))
            .unwrap();
        assert!(state.completed());
        let payloads = json_stream_frames(&terminal);
        let tool = payloads
            .iter()
            .find_map(|value| value.pointer("/candidates/0/content/parts/0"))
            .unwrap();
        assert_eq!(tool["functionCall"]["id"], "call_late");
        assert_eq!(tool["functionCall"]["args"], json!({"q":1}));
        assert_eq!(tool["thoughtSignature"], "late-chat-signature");
        let terminals = payloads
            .iter()
            .filter(|value| value.pointer("/candidates/0/finishReason").is_some())
            .collect::<Vec<_>>();
        assert_eq!(terminals.len(), 1);
        assert_eq!(terminals[0]["usageMetadata"]["promptTokenCount"], 4);
        assert_eq!(terminals[0]["usageMetadata"]["candidatesTokenCount"], 2);
        assert!(state.finish_stream().unwrap().is_empty());
    }

    #[test]
    fn to_gemini_uses_signature_from_responses_item_done() {
        let mut state = ToGeminiState::responses();
        let frames = drive_to_gemini(
            &mut state,
            &[
                json!({"type":"response.created","response":{"id":"resp-late"}}),
                json!({"type":"response.output_item.added","output_index":0,"item":{"type":"function_call","call_id":"call_late","name":"lookup"}}),
                json!({"type":"response.function_call_arguments.delta","output_index":0,"delta":"{\"q\":1}"}),
                json!({"type":"response.output_item.done","output_index":0,"item":{"type":"function_call","call_id":"call_late","name":"lookup","arguments":"{\"q\":1}","thought_signature":"late-responses-signature"}}),
                json!({"type":"response.completed","response":{"status":"completed","usage":{"input_tokens":3,"output_tokens":1}}}),
            ],
        );
        let tool = json_stream_frames(&frames)
            .into_iter()
            .find_map(|value| value.pointer("/candidates/0/content/parts/0"))
            .unwrap();
        assert_eq!(tool["functionCall"]["id"], "call_late");
        assert_eq!(tool["thoughtSignature"], "late-responses-signature");
    }

    #[test]
    fn gemini_bridge_done_and_eof_are_fail_closed_and_idempotent_after_terminal() {
        let mut premature_done =
            StreamBridgeState::GeminiAnthropic(GeminiAnthropicState::default());
        premature_done
            .transform(&json!({"candidates":[{"content":{"parts":[{"text":"partial"}]}}]}))
            .unwrap();
        assert!(premature_done.upstream_done().is_err());

        let mut premature_eof = StreamBridgeState::GeminiAnthropic(GeminiAnthropicState::default());
        premature_eof
            .transform(&json!({"candidates":[{"content":{"parts":[{"text":"partial"}]}}]}))
            .unwrap();
        assert!(premature_eof.finish_eof().is_err());

        let mut completed = StreamBridgeState::GeminiOpenAi(Box::new(
            GeminiOpenAiState::responses(transforms::ResponsesToolContext::default()),
        ));
        completed
            .transform(&json!({"candidates":[{"finishReason":"STOP"}]}))
            .unwrap();
        let terminal = completed.upstream_done().unwrap();
        assert_eq!(done_frame_count(&terminal), 1);
        assert!(completed.upstream_done().unwrap().is_empty());
        assert!(completed.finish_eof().unwrap().is_empty());
    }

    fn gemini_openai_fixture(mut state: GeminiOpenAiState) -> Vec<StreamFrame> {
        let events = [
            json!({"responseId":"gem-cross","modelVersion":"gemini-test","candidates":[{"content":{"parts":[{"text":"private ","thought":true}]}}]}),
            json!({"candidates":[{"content":{"parts":[{"text":"private plan","thought":true,"thoughtSignature":"thought-signature"}]}}]}),
            json!({"candidates":[{"content":{"parts":[{"text":"hel"}]}}]}),
            json!({"candidates":[{"content":{"parts":[{"text":"hello"}]}}]}),
            json!({"candidates":[{"content":{"parts":[{"functionCall":{"id":"call_lookup","name":"lookup","args":{"query":"ser"}}}]}}]}),
            json!({"candidates":[{"content":{"parts":[{"functionCall":{"id":"call_lookup","name":"lookup","args":{"query":"server"}},"thoughtSignature":"tool-signature"}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":8,"cachedContentTokenCount":2,"candidatesTokenCount":3,"thoughtsTokenCount":2}}),
        ];
        let mut frames = Vec::new();
        for event in events {
            frames.extend(state.transform(&event).unwrap());
        }
        frames.extend(state.finish_stream().unwrap());
        assert!(state.completed());
        frames
    }

    fn drive_to_gemini(state: &mut ToGeminiState, events: &[Value]) -> Vec<StreamFrame> {
        let mut frames = Vec::new();
        for event in events {
            frames.extend(state.transform(event).unwrap());
        }
        frames
    }

    struct NativeGeminiExpectation<'a> {
        thinking: &'a str,
        text: &'a str,
        call_id: &'a str,
        thinking_signature: &'a str,
        tool_signature: &'a str,
        input_tokens: u64,
        output_tokens: u64,
    }

    fn assert_native_gemini_frames(frames: &[StreamFrame], expected: NativeGeminiExpectation<'_>) {
        let payloads = json_stream_frames(frames);
        assert_eq!(
            payloads
                .iter()
                .filter_map(|value| value
                    .pointer("/candidates/0/content/parts/0")
                    .filter(|part| part["thought"] == true)
                    .and_then(|part| part["text"].as_str())
                    .filter(|text| !text.is_empty()))
                .collect::<String>(),
            expected.thinking
        );
        assert_eq!(
            payloads
                .iter()
                .filter_map(|value| value
                    .pointer("/candidates/0/content/parts/0")
                    .filter(|part| part.get("thought").is_none())
                    .and_then(|part| part["text"].as_str()))
                .collect::<String>(),
            expected.text
        );
        assert!(payloads.iter().any(|value| {
            value.pointer("/candidates/0/content/parts/0/thoughtSignature")
                == Some(&json!(expected.thinking_signature))
        }));
        let tool = payloads
            .iter()
            .find_map(|value| {
                value
                    .pointer("/candidates/0/content/parts/0")
                    .filter(|part| part.get("functionCall").is_some())
            })
            .unwrap();
        assert_eq!(tool["functionCall"]["id"], expected.call_id);
        assert_eq!(tool["thoughtSignature"], expected.tool_signature);
        let terminals = payloads
            .iter()
            .filter(|value| value.pointer("/candidates/0/finishReason").is_some())
            .collect::<Vec<_>>();
        assert_eq!(terminals.len(), 1);
        assert_eq!(terminals[0]["candidates"][0]["finishReason"], "STOP");
        assert_eq!(
            terminals[0]["usageMetadata"]["promptTokenCount"],
            expected.input_tokens
        );
        assert_eq!(
            terminals[0]["usageMetadata"]["candidatesTokenCount"],
            expected.output_tokens
        );
        assert_eq!(done_frame_count(frames), 0);
    }

    fn json_stream_frames(frames: &[StreamFrame]) -> Vec<&Value> {
        frames
            .iter()
            .filter_map(|frame| match &frame.payload {
                transforms::StreamPayload::Json(value) => Some(value),
                transforms::StreamPayload::Done => None,
            })
            .collect()
    }

    fn done_frame_count(frames: &[StreamFrame]) -> usize {
        frames
            .iter()
            .filter(|frame| matches!(frame.payload, transforms::StreamPayload::Done))
            .count()
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
    fn grok_function_stream_is_restored_to_codex_custom_tool_events() {
        let context = transforms::responses_tool_context(&json!({
            "tools": [{"type": "custom", "name": "exec"}],
            "input": "run pwd"
        }));
        let mut state = GrokResponsesToolsState::new(context);

        let added = state.transform(&json!({
            "type": "response.output_item.added",
            "output_index": 1,
            "item": {
                "id": "fc_exec", "type": "function_call", "status": "in_progress",
                "call_id": "call_exec", "name": "exec", "arguments": ""
            }
        }));
        assert_eq!(added[0].payload_json()["item"]["type"], "custom_tool_call");

        assert!(state
            .transform(&json!({
                "type": "response.function_call_arguments.delta",
                "output_index": 1,
                "item_id": "fc_exec",
                "delta": "{\"input\":\"pwd\"}"
            }))
            .is_empty());
        let arguments_done = state.transform(&json!({
            "type": "response.function_call_arguments.done",
            "output_index": 1,
            "item_id": "fc_exec",
            "arguments": "{\"input\":\"pwd\"}"
        }));
        assert_eq!(arguments_done.len(), 2);
        assert_eq!(
            arguments_done[0].payload_json()["type"],
            "response.custom_tool_call_input.delta"
        );
        assert_eq!(arguments_done[0].payload_json()["delta"], "pwd");
        assert_eq!(
            arguments_done[1].payload_json()["type"],
            "response.custom_tool_call_input.done"
        );

        let item_done = state.transform(&json!({
            "type": "response.output_item.done",
            "output_index": 1,
            "item": {
                "id": "fc_exec", "type": "function_call", "status": "completed",
                "call_id": "call_exec", "name": "exec", "arguments": "{\"input\":\"pwd\"}"
            }
        }));
        assert_eq!(
            item_done[0].payload_json()["item"]["type"],
            "custom_tool_call"
        );
        assert_eq!(item_done[0].payload_json()["item"]["input"], "pwd");

        let completed = state.transform(&json!({
            "type": "response.completed",
            "response": {"status": "completed", "output": [{
                "id": "fc_exec", "type": "function_call", "status": "completed",
                "call_id": "call_exec", "name": "exec", "arguments": "{\"input\":\"pwd\"}"
            }]}
        }));
        assert_eq!(
            completed[0].payload_json()["response"]["output"][0]["type"],
            "custom_tool_call"
        );
        assert!(state.completed);
    }

    #[test]
    fn grok_apply_patch_stream_waits_for_complete_operation_before_added_event() {
        let context = transforms::responses_tool_context(&json!({
            "tools": [{"type": "apply_patch"}],
            "input": "update the readme"
        }));
        let mut state = GrokResponsesToolsState::new(context);

        assert!(state
            .transform(&json!({
                "type": "response.output_item.added",
                "output_index": 0,
                "sequence_number": 7,
                "item": {"id": "fc_patch", "type": "function_call", "status": "in_progress", "call_id": "call_patch", "name": "cc_switch_apply_patch"}
            }))
            .is_empty());
        assert!(state
            .transform(&json!({
                "type": "response.function_call_arguments.delta",
                "output_index": 0,
                "delta": "{\"operation\":{\"type\":\"delete_file\",\"path\":\"old.txt\"}}"
            }))
            .is_empty());
        let frames = state.transform(&json!({
            "type": "response.output_item.done",
            "output_index": 0,
            "sequence_number": 9,
            "item": {"id": "fc_patch", "type": "function_call", "status": "completed", "call_id": "call_patch", "name": "cc_switch_apply_patch", "arguments": "{\"operation\":{\"type\":\"delete_file\",\"path\":\"old.txt\"}}"}
        }));

        assert_eq!(frames.len(), 2);
        assert_eq!(
            frames[0].payload_json()["type"],
            "response.output_item.added"
        );
        assert_eq!(frames[0].payload_json()["sequence_number"], 7);
        assert_eq!(frames[0].payload_json()["item"]["type"], "apply_patch_call");
        assert_eq!(
            frames[0].payload_json()["item"]["operation"]["path"],
            "old.txt"
        );
        assert_eq!(
            frames[1].payload_json()["type"],
            "response.output_item.done"
        );
        assert_eq!(frames[1].payload_json()["sequence_number"], 9);
    }

    #[test]
    fn responses_hosted_searches_emit_server_tools_citations_and_usage() {
        let mut state = ResponsesAnthropicState::default();
        state.transform(&json!({
            "type": "response.output_item.added",
            "item": {"type": "web_search_call", "id": "ws_1"}
        }));
        let hosted = state.transform(&json!({
            "type": "response.output_item.done",
            "item": {
                "type": "web_search_call",
                "id": "ws_1",
                "action": {"query": "rust news"}
            }
        }));
        assert!(hosted
            .iter()
            .any(|frame| { frame.payload_json()["content_block"]["type"] == "server_tool_use" }));
        assert!(hosted.iter().any(|frame| {
            frame.payload_json()["content_block"]["type"] == "web_search_tool_result"
        }));

        state.transform(&json!({
            "type": "response.output_item.added",
            "output_index": 4,
            "item": {"type": "custom_tool_call", "id": "xs_1", "name": "x_search"}
        }));
        state.transform(&json!({
            "type": "response.custom_tool_call_input.delta",
            "item_id": "xs_1",
            "delta": "{\"query\":\"release notes\"}"
        }));
        let x_hosted = state.transform(&json!({
            "type": "response.output_item.done",
            "item": {"type": "custom_tool_call", "id": "xs_1", "name": "x_search"}
        }));
        assert!(x_hosted.iter().any(|frame| {
            frame.payload_json()["content_block"]["type"] == "x_search_tool_result"
        }));

        state.transform(&json!({
            "type": "response.output_text.delta",
            "delta": "Result"
        }));
        let citation = state.transform(&json!({
            "type": "response.output_text.annotation.added",
            "annotation": {
                "type": "url_citation",
                "url": "https://example.com",
                "title": "Example",
                "text": "cited"
            }
        }));
        assert_eq!(
            citation[0].payload_json()["delta"]["type"],
            "citations_delta"
        );
        assert_eq!(
            citation[0].payload_json()["delta"]["citation"]["url"],
            "https://example.com"
        );

        let completed = state.transform(&json!({
            "type": "response.completed",
            "response": {"status": "completed", "usage": {"input_tokens": 4, "output_tokens": 3}}
        }));
        let delta = completed
            .iter()
            .find(|frame| frame.payload_json()["type"] == "message_delta")
            .unwrap()
            .payload_json();
        assert_eq!(delta["delta"]["stop_reason"], "end_turn");
        assert_eq!(delta["usage"]["server_tool_use"]["web_search_requests"], 2);
        assert_eq!(delta["usage"]["server_tool_use"]["x_search_requests"], 1);
    }

    #[test]
    fn responses_x_search_correlates_done_input_by_item_id() {
        let mut state = ResponsesAnthropicState::default();
        state.transform(&json!({
            "type": "response.output_item.added",
            "item": {"type": "custom_tool_call", "id": "xs_done", "name": "x_search"}
        }));
        state.transform(&json!({
            "type": "response.custom_tool_call_input.done",
            "item_id": "xs_done",
            "input": {"query": "done-only query"}
        }));

        let frames = state.transform(&json!({
            "type": "response.output_item.done",
            "item": {"type": "custom_tool_call", "id": "xs_done", "name": "x_search"}
        }));
        let input_delta = frames
            .iter()
            .find(|frame| frame.payload_json()["delta"]["type"] == "input_json_delta")
            .unwrap()
            .payload_json();

        assert_eq!(
            input_delta["delta"]["partial_json"],
            json!("{\"query\":\"done-only query\"}")
        );
    }

    #[test]
    fn responses_x_search_keeps_synthetic_index_input_when_done_adds_index() {
        let mut state = ResponsesAnthropicState::default();
        state.transform(&json!({
            "type": "response.output_item.added",
            "item": {"type": "custom_tool_call", "id": "xs_synthetic", "name": "x_search"}
        }));
        state.transform(&json!({
            "type": "response.custom_tool_call_input.done",
            "item_id": "xs_synthetic",
            "input": {"query": "done-only query"}
        }));

        let frames = state.transform(&json!({
            "type": "response.output_item.done",
            "output_index": 7,
            "item": {"type": "custom_tool_call", "id": "xs_synthetic", "name": "x_search"}
        }));
        let input_delta = frames
            .iter()
            .find(|frame| frame.payload_json()["delta"]["type"] == "input_json_delta")
            .unwrap()
            .payload_json();

        assert_eq!(
            input_delta["delta"]["partial_json"],
            json!("{\"query\":\"done-only query\"}")
        );
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
