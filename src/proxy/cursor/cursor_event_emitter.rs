//! Cursor `agent.v1` event → OpenAI/Claude SSE adapter.
//!
//! The cursor side emits `InteractionDelta` (text/thinking/usage/turn) and
//! `ExecServerEvent` (built-in tool args, MCP tool args). This module turns
//! that stream into the three SSE shapes cc-switch hands back to clients:
//!   * Anthropic Messages — content_block_start/delta/stop, message_*  events
//!   * OpenAI Chat        — delta + finish_reason
//!   * OpenAI Responses   — response.output_item.added / .done family
//!
//! It also runs a `ComposerMarkerFilter` over text deltas to capture the
//! `<|tool_calls_begin|>…<|tool_call_begin|>name<|tool_sep|>args<|tool_call_end|>…`
//! escape sequence cursor's composer model occasionally uses instead of the
//! protobuf `mcp_args` channel — so cc-switch lifts those into real tool_use /
//! tool_calls events instead of leaking the markers as text.

use super::cursor_protocol::CursorResponseFormat;
use rand::RngCore;
use serde_json::{json, Value};
use std::collections::HashMap;

// ─── Internal event vocabulary ─────────────────────────────────────────────

/// Tool call captured either from cursor's `mcp_args` protobuf or from a
/// Composer text marker block.
#[derive(Debug, Clone)]
pub struct CapturedToolCall {
    pub id: String,
    pub name: String,
    pub arguments_json: String,
}

#[derive(Debug, Clone)]
pub enum AgentEvent {
    Text(String),
    Thinking(String),
    ThinkingComplete,
    ToolCall(CapturedToolCall),
    TurnEnded,
    Usage { input: u32, output: u32 },
    Error(String),
}

// ─── Composer marker filter ────────────────────────────────────────────────

const TOOL_CALLS_BEGIN: &str = "<|tool_calls_begin|>";
const TOOL_CALLS_END: &str = "<|tool_calls_end|>";
const TOOL_CALL_BEGIN: &str = "<|tool_call_begin|>";
const TOOL_CALL_END: &str = "<|tool_call_end|>";
const TOOL_SEP: &str = "<|tool_sep|>";

const MARKERS: &[&str] = &[
    TOOL_CALLS_BEGIN,
    TOOL_CALLS_END,
    TOOL_CALL_BEGIN,
    TOOL_CALL_END,
    TOOL_SEP,
];

fn random_id_hex() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// State machine that consumes raw text deltas and yields either `Text` or
/// `ToolCall` events. Holds partial markers in a buffer until they complete
/// (or until `flush()` is called at end-of-turn).
#[derive(Default)]
pub struct ComposerMarkerFilter {
    buffer: String,
}

#[derive(Debug, Clone)]
pub enum MarkerEvent {
    Text(String),
    ToolCall(CapturedToolCall),
}

impl ComposerMarkerFilter {
    pub fn push(&mut self, delta: &str) -> Vec<MarkerEvent> {
        self.buffer.push_str(delta);
        self.drain(false)
    }

    pub fn flush(&mut self) -> Vec<MarkerEvent> {
        let out = self.drain(true);
        self.buffer.clear();
        out
    }

    fn drain(&mut self, force: bool) -> Vec<MarkerEvent> {
        let mut out = Vec::new();
        loop {
            // Find a tool_calls_begin marker.
            match self.buffer.find(TOOL_CALLS_BEGIN) {
                Some(begin_idx) => {
                    if begin_idx > 0 {
                        let before = self.buffer[..begin_idx].to_string();
                        if !before.trim().is_empty() {
                            out.push(MarkerEvent::Text(before));
                        }
                        self.buffer = self.buffer[begin_idx..].to_string();
                        continue;
                    }
                    // begin at 0 — look for matching end
                    let search_from = TOOL_CALLS_BEGIN.len();
                    let Some(rel_end) = self.buffer[search_from..].find(TOOL_CALLS_END) else {
                        // No end yet — wait for more bytes unless we're forced.
                        if force {
                            // Truncated marker block at EOS — surface as text.
                            out.push(MarkerEvent::Text(std::mem::take(&mut self.buffer)));
                        }
                        break;
                    };
                    let block_end = search_from + rel_end + TOOL_CALLS_END.len();
                    let block = self.buffer[..block_end].to_string();
                    for call in parse_tool_call_block(&block) {
                        out.push(MarkerEvent::ToolCall(call));
                    }
                    // Strip the block + any leading whitespace from the buffer.
                    let rest = self.buffer[block_end..].trim_start().to_string();
                    self.buffer = rest;
                }
                None => {
                    // No begin marker. If the tail might be the start of a
                    // marker, hold it; otherwise emit everything as text.
                    if let Some(prefix_idx) = marker_prefix_index(&self.buffer) {
                        if !force {
                            let visible = self.buffer[..prefix_idx].to_string();
                            if !visible.is_empty() {
                                out.push(MarkerEvent::Text(visible));
                            }
                            self.buffer = self.buffer[prefix_idx..].to_string();
                            break;
                        }
                    }
                    if !self.buffer.is_empty() {
                        out.push(MarkerEvent::Text(std::mem::take(&mut self.buffer)));
                    }
                    break;
                }
            }
        }
        out
    }
}

/// Return the smallest index `i` such that `buf[i..]` is a *prefix* of any
/// known marker (i.e. text that may grow into a marker). `None` means no
/// pending prefix — safe to flush the whole buffer.
fn marker_prefix_index(buf: &str) -> Option<usize> {
    // Scan from the end: look for the last position where a marker prefix
    // could start. We only need to keep at most `max_marker_len - 1` chars.
    let max = MARKERS.iter().map(|m| m.len()).max().unwrap_or(0);
    let start = buf.len().saturating_sub(max);
    for (i, _) in buf.char_indices().filter(|(i, _)| *i >= start) {
        let tail = &buf[i..];
        for m in MARKERS {
            if m.starts_with(tail) {
                return Some(i);
            }
        }
    }
    None
}

fn parse_tool_call_block(block: &str) -> Vec<CapturedToolCall> {
    let begin = match block.find(TOOL_CALLS_BEGIN) {
        Some(b) => b + TOOL_CALLS_BEGIN.len(),
        None => return Vec::new(),
    };
    let end = match block.rfind(TOOL_CALLS_END) {
        Some(e) => e,
        None => return Vec::new(),
    };
    if end <= begin {
        return Vec::new();
    }
    let body = &block[begin..end];
    let mut calls = Vec::new();
    let mut offset = 0usize;
    while let Some(start) = body[offset..].find(TOOL_CALL_BEGIN) {
        let abs_start = offset + start + TOOL_CALL_BEGIN.len();
        let Some(rel_end) = body[abs_start..].find(TOOL_CALL_END) else {
            break;
        };
        let abs_end = abs_start + rel_end;
        let entry = &body[abs_start..abs_end];
        if let Some(call) = parse_tool_call_entry(entry) {
            calls.push(call);
        }
        offset = abs_end + TOOL_CALL_END.len();
    }
    calls
}

fn parse_tool_call_entry(entry: &str) -> Option<CapturedToolCall> {
    let trimmed = entry.trim();
    if trimmed.is_empty() {
        return None;
    }
    // Some composer emissions inline a JSON object — try that first.
    if trimmed.starts_with('{') {
        if let Ok(v) = serde_json::from_str::<Value>(trimmed) {
            let name = v.get("name").and_then(Value::as_str)?.to_string();
            let args = v.get("arguments").cloned().unwrap_or(json!({}));
            let arguments_json = if args.is_string() {
                args.as_str().unwrap_or("{}").to_string()
            } else {
                args.to_string()
            };
            return Some(CapturedToolCall {
                id: format!("call_{}", random_id_hex()),
                name,
                arguments_json,
            });
        }
    }
    // Otherwise, name<|tool_sep|>args (args may be JSON-stringified).
    let mut parts = trimmed.split(TOOL_SEP);
    let name = parts.next()?.trim().to_string();
    if name.is_empty() {
        return None;
    }
    let arguments_json = match parts.next() {
        Some(rest) => {
            let rest = rest.trim();
            if rest.starts_with('{') || rest.starts_with('[') {
                rest.to_string()
            } else if rest.is_empty() {
                "{}".to_string()
            } else {
                serde_json::Value::String(rest.to_string()).to_string()
            }
        }
        None => "{}".to_string(),
    };
    Some(CapturedToolCall {
        id: format!("call_{}", random_id_hex()),
        name,
        arguments_json,
    })
}

// ─── SSE writer ────────────────────────────────────────────────────────────

pub struct AgentSseWriter {
    model: String,
    format: CursorResponseFormat,
    msg_id: String,
    // Anthropic block bookkeeping
    next_block_idx: u32,
    text_block: Option<u32>,
    thinking_block: Option<u32>,
    chat_tool_call_count: u32,
    // OpenAI Responses output-item bookkeeping
    next_output_idx: u32,
    reasoning_item: Option<OutputRef>,
    text_item: Option<OutputRef>,
    tool_items: HashMap<String, OutputRef>, // by capture id
    // Anthropic message_start sent?
    started: bool,
    input_tokens: u32,
    output_tokens: u32,
    aggregate_text: String,
    aggregate_reasoning: String,
    aggregate_tool_calls: Vec<CapturedToolCall>,
    error_mode: bool,
}

#[derive(Clone)]
struct OutputRef {
    index: u32,
    item_id: String,
}

impl AgentSseWriter {
    pub fn new(model: String, format: CursorResponseFormat, input_tokens: u32) -> Self {
        let prefix = match format {
            CursorResponseFormat::AnthropicMessages => "msg",
            CursorResponseFormat::OpenAiChatCompletions => "chatcmpl",
            CursorResponseFormat::OpenAiResponses => "resp",
            CursorResponseFormat::GeminiGenerateContent => "gemini",
        };
        let msg_id = format!("{}_{}", prefix, random_id_hex());
        Self {
            model,
            format,
            msg_id,
            next_block_idx: 0,
            text_block: None,
            thinking_block: None,
            chat_tool_call_count: 0,
            next_output_idx: 0,
            reasoning_item: None,
            text_item: None,
            tool_items: HashMap::new(),
            started: false,
            input_tokens,
            output_tokens: 0,
            aggregate_text: String::new(),
            aggregate_reasoning: String::new(),
            aggregate_tool_calls: Vec::new(),
            error_mode: false,
        }
    }

    pub fn message_id(&self) -> &str {
        &self.msg_id
    }

    /// Current estimated input token count (set at construction, updated by
    /// Usage events). Used by the agent service to report meaningful usage.
    pub fn input_tokens(&self) -> u32 {
        self.input_tokens
    }

    pub fn output_tokens(&self) -> u32 {
        self.output_tokens
    }

    /// Clear per-turn output so a tool-call retry does not duplicate aggregated text.
    /// Preserves `msg_id` and `started` so OpenAI Responses session binding stays stable.
    pub fn reset_for_retry(&mut self) {
        self.next_block_idx = 0;
        self.text_block = None;
        self.thinking_block = None;
        self.chat_tool_call_count = 0;
        self.next_output_idx = 0;
        self.reasoning_item = None;
        self.text_item = None;
        self.tool_items.clear();
        self.aggregate_text.clear();
        self.aggregate_reasoning.clear();
        self.aggregate_tool_calls.clear();
        self.error_mode = false;
        self.output_tokens = 0;
        // input_tokens intentionally preserved across retries for stable usage.
    }

    pub fn start_events(&mut self) -> Vec<String> {
        if self.started {
            return Vec::new();
        }
        self.started = true;
        match self.format {
            CursorResponseFormat::AnthropicMessages => vec![anthropic_event(
                "message_start",
                json!({
                    "type": "message_start",
                    "message": {
                        "id": self.msg_id,
                        "type": "message",
                        "role": "assistant",
                        "model": self.model,
                        "content": [],
                        "stop_reason": null,
                        "stop_sequence": null,
                        "usage": {
                            "input_tokens": self.input_tokens,
                            "output_tokens": 0
                        }
                    }
                }),
            )],
            CursorResponseFormat::OpenAiResponses => vec![
                event(
                    "response.created",
                    json!({
                        "type": "response.created",
                        "response": self.responses_base(),
                    }),
                ),
                event(
                    "response.in_progress",
                    json!({
                        "type": "response.in_progress",
                        "response": self.responses_base(),
                    }),
                ),
            ],
            CursorResponseFormat::OpenAiChatCompletions => Vec::new(),
            CursorResponseFormat::GeminiGenerateContent => Vec::new(),
        }
    }

    pub fn event(&mut self, ev: &AgentEvent) -> Vec<String> {
        match ev {
            AgentEvent::Text(t) => self.text_delta(t),
            AgentEvent::Thinking(t) => self.thinking_delta(t),
            AgentEvent::ThinkingComplete => self.close_thinking(),
            AgentEvent::ToolCall(tc) => self.tool_call(tc),
            AgentEvent::TurnEnded => Vec::new(),
            AgentEvent::Usage { input, output } => {
                if *input > 0 {
                    self.input_tokens = *input;
                }
                if *output > 0 {
                    self.output_tokens = self.output_tokens.saturating_add(*output);
                }
                Vec::new()
            }
            AgentEvent::Error(msg) => self.error_events(msg),
        }
    }

    pub fn done_events(&mut self) -> Vec<String> {
        // Close any still-open blocks then emit terminal events.
        let mut out = Vec::new();
        let thinking = self.thinking_block.take();
        let text = self.text_block.take();
        if let CursorResponseFormat::AnthropicMessages = self.format {
            if let Some(idx) = thinking {
                out.push(anthropic_event(
                    "content_block_stop",
                    json!({ "type": "content_block_stop", "index": idx }),
                ));
            }
            if let Some(idx) = text {
                out.push(anthropic_event(
                    "content_block_stop",
                    json!({ "type": "content_block_stop", "index": idx }),
                ));
            }
            let stop_reason = if self.aggregate_tool_calls.is_empty() {
                "end_turn"
            } else {
                "tool_use"
            };
            let stop_reason = if self.error_mode {
                "error"
            } else {
                stop_reason
            };
            out.push(anthropic_event(
                "message_delta",
                json!({
                    "type": "message_delta",
                    "delta": { "stop_reason": stop_reason, "stop_sequence": null },
                    "usage": { "output_tokens": self.output_tokens }
                }),
            ));
            out.push(anthropic_event(
                "message_stop",
                json!({ "type": "message_stop" }),
            ));
            return out;
        }

        if let CursorResponseFormat::OpenAiResponses = self.format {
            if self.error_mode {
                out.push("data: [DONE]\n\n".to_string());
                return out;
            }
            // Close reasoning item.
            if let Some(r) = self.reasoning_item.take() {
                out.push(event(
                    "response.output_item.done",
                    json!({
                        "type": "response.output_item.done",
                        "output_index": r.index,
                        "item": {
                            "id": r.item_id,
                            "type": "reasoning",
                            "summary": []
                        }
                    }),
                ));
            }
            if let Some(t) = self.text_item.take() {
                out.push(event(
                    "response.output_text.done",
                    json!({
                        "type": "response.output_text.done",
                        "item_id": t.item_id,
                        "output_index": t.index,
                        "content_index": 0,
                        "text": self.aggregate_text
                    }),
                ));
                out.push(event(
                    "response.output_item.done",
                    json!({
                        "type": "response.output_item.done",
                        "output_index": t.index,
                        "item": {
                            "id": t.item_id,
                            "type": "message",
                            "role": "assistant",
                            "content": [{
                                "type": "output_text",
                                "text": self.aggregate_text
                            }]
                        }
                    }),
                ));
            }
            for (_, oref) in std::mem::take(&mut self.tool_items) {
                out.push(event(
                    "response.function_call_arguments.done",
                    json!({
                        "type": "response.function_call_arguments.done",
                        "item_id": oref.item_id,
                        "output_index": oref.index,
                        "arguments": ""
                    }),
                ));
                out.push(event(
                    "response.output_item.done",
                    json!({
                        "type": "response.output_item.done",
                        "output_index": oref.index,
                        "item": {
                            "id": oref.item_id,
                            "type": "function_call",
                            "arguments": ""
                        }
                    }),
                ));
            }
            out.push(event(
                "response.completed",
                json!({
                    "type": "response.completed",
                    "response": {
                        "id": self.msg_id,
                        "object": "response",
                        "model": self.model,
                        "status": "completed",
                        "output": self.responses_output_snapshot(),
                        "usage": {
                            "input_tokens": self.input_tokens,
                            "output_tokens": self.output_tokens,
                            "total_tokens": self.input_tokens + self.output_tokens,
                        }
                    }
                }),
            ));
            out.push("data: [DONE]\n\n".to_string());
            return out;
        }

        if let CursorResponseFormat::GeminiGenerateContent = self.format {
            if !self.error_mode {
                out.push(gemini_chunk(
                    &self.msg_id,
                    &self.model,
                    None,
                    Some("STOP"),
                    Some(self.input_tokens),
                    Some(self.output_tokens),
                ));
            }
            return out;
        }

        // OpenAI Chat
        let finish_reason = if self.aggregate_tool_calls.is_empty() {
            "stop"
        } else {
            "tool_calls"
        };
        out.push(chat_chunk(
            &self.msg_id,
            &self.model,
            json!({}),
            Some(finish_reason),
        ));
        // Usage chunk (optional but useful).
        out.push(chat_chunk_usage(
            &self.msg_id,
            &self.model,
            self.input_tokens,
            self.output_tokens,
        ));
        out.push("data: [DONE]\n\n".to_string());
        out
    }

    pub fn json_response(&self) -> Value {
        match self.format {
            CursorResponseFormat::AnthropicMessages => {
                let mut content = Vec::new();
                if !self.aggregate_reasoning.is_empty() {
                    content.push(json!({
                        "type": "thinking",
                        "thinking": self.aggregate_reasoning
                    }));
                }
                if !self.aggregate_text.is_empty() {
                    content.push(json!({
                        "type": "text",
                        "text": self.aggregate_text
                    }));
                }
                for tc in &self.aggregate_tool_calls {
                    let input: Value =
                        serde_json::from_str(&tc.arguments_json).unwrap_or(json!({}));
                    content.push(json!({
                        "type": "tool_use",
                        "id": tc.id,
                        "name": tc.name,
                        "input": input
                    }));
                }
                json!({
                    "id": self.msg_id,
                    "type": "message",
                    "role": "assistant",
                    "model": self.model,
                    "content": content,
                    "stop_reason": if self.aggregate_tool_calls.is_empty() { "end_turn" } else { "tool_use" },
                    "stop_sequence": Value::Null,
                    "usage": {
                        "input_tokens": self.input_tokens,
                        "output_tokens": self.output_tokens
                    }
                })
            }
            CursorResponseFormat::OpenAiChatCompletions => {
                let mut message = json!({
                    "role": "assistant",
                    "content": if self.aggregate_tool_calls.is_empty() {
                        Value::String(self.aggregate_text.clone())
                    } else {
                        Value::Null
                    }
                });
                if !self.aggregate_tool_calls.is_empty() {
                    message["tool_calls"] = Value::Array(
                        self.aggregate_tool_calls
                            .iter()
                            .map(|tc| {
                                json!({
                                    "id": tc.id,
                                    "type": "function",
                                    "function": {
                                        "name": tc.name,
                                        "arguments": tc.arguments_json
                                    }
                                })
                            })
                            .collect(),
                    );
                }
                json!({
                    "id": self.msg_id,
                    "object": "chat.completion",
                    "created": chrono::Utc::now().timestamp(),
                    "model": self.model,
                    "choices": [{
                        "index": 0,
                        "message": message,
                        "finish_reason": if self.aggregate_tool_calls.is_empty() { "stop" } else { "tool_calls" }
                    }],
                    "usage": {
                        "prompt_tokens": self.input_tokens,
                        "completion_tokens": self.output_tokens,
                        "total_tokens": self.input_tokens + self.output_tokens
                    }
                })
            }
            CursorResponseFormat::OpenAiResponses => json!({
                "id": self.msg_id,
                "object": "response",
                "model": self.model,
                "status": "completed",
                "output": self.responses_output_snapshot(),
                "parallel_tool_calls": true,
                "previous_response_id": Value::Null,
                "usage": {
                    "input_tokens": self.input_tokens,
                    "output_tokens": self.output_tokens,
                    "total_tokens": self.input_tokens + self.output_tokens
                }
            }),
            CursorResponseFormat::GeminiGenerateContent => self.gemini_json_response(),
        }
    }

    pub fn error_events(&mut self, message: &str) -> Vec<String> {
        self.error_mode = true;
        match self.format {
            CursorResponseFormat::AnthropicMessages => vec![anthropic_event(
                "error",
                json!({
                    "type": "error",
                    "error": { "type": "upstream_error", "message": message }
                }),
            )],
            CursorResponseFormat::OpenAiResponses => vec![event(
                "response.failed",
                json!({
                    "type": "response.failed",
                    "response": {
                        "id": self.msg_id,
                        "object": "response",
                        "model": self.model,
                        "status": "failed",
                        "error": { "message": message }
                    }
                }),
            )],
            CursorResponseFormat::OpenAiChatCompletions => {
                vec![chat_chunk(
                    &self.msg_id,
                    &self.model,
                    json!({}),
                    Some("error"),
                )]
            }
            CursorResponseFormat::GeminiGenerateContent => vec![
                format!(
                    "data: {}\n\n",
                    json!({
                        "error": {
                            "message": message,
                            "type": "upstream_error",
                            "code": "cc_switch_stream_error"
                        }
                    })
                ),
                "data: [DONE]\n\n".to_string(),
            ],
        }
    }

    // ─── Per-event helpers ─────────────────────────────────────────────────

    fn text_delta(&mut self, text: &str) -> Vec<String> {
        if text.is_empty() {
            return Vec::new();
        }
        self.aggregate_text.push_str(text);
        let mut out = Vec::new();
        match self.format {
            CursorResponseFormat::AnthropicMessages => {
                if self.text_block.is_none() {
                    let idx = self.alloc_block();
                    self.text_block = Some(idx);
                    out.push(anthropic_event(
                        "content_block_start",
                        json!({
                            "type": "content_block_start",
                            "index": idx,
                            "content_block": { "type": "text", "text": "" }
                        }),
                    ));
                }
                let idx = self.text_block.unwrap();
                out.push(anthropic_event(
                    "content_block_delta",
                    json!({
                        "type": "content_block_delta",
                        "index": idx,
                        "delta": { "type": "text_delta", "text": text }
                    }),
                ));
            }
            CursorResponseFormat::OpenAiResponses => {
                if self.text_item.is_none() {
                    let idx = self.alloc_output();
                    let item_id = format!("msg_{}", random_id_hex());
                    out.push(event(
                        "response.output_item.added",
                        json!({
                            "type": "response.output_item.added",
                            "output_index": idx,
                            "item": {
                                "id": item_id,
                                "type": "message",
                                "role": "assistant",
                                "content": []
                            }
                        }),
                    ));
                    self.text_item = Some(OutputRef {
                        index: idx,
                        item_id,
                    });
                }
                let oref = self.text_item.as_ref().unwrap().clone();
                out.push(event(
                    "response.output_text.delta",
                    json!({
                        "type": "response.output_text.delta",
                        "item_id": oref.item_id,
                        "output_index": oref.index,
                        "content_index": 0,
                        "delta": text
                    }),
                ));
            }
            CursorResponseFormat::OpenAiChatCompletions => {
                out.push(chat_chunk(
                    &self.msg_id,
                    &self.model,
                    json!({ "content": text, "role": "assistant" }),
                    None,
                ));
            }
            CursorResponseFormat::GeminiGenerateContent => {
                out.push(gemini_chunk(
                    &self.msg_id,
                    &self.model,
                    Some(text),
                    None,
                    None,
                    None,
                ));
            }
        }
        self.output_tokens = self.output_tokens.saturating_add(estimate_tokens(text));
        out
    }

    fn thinking_delta(&mut self, text: &str) -> Vec<String> {
        if text.is_empty() {
            return Vec::new();
        }
        self.aggregate_reasoning.push_str(text);
        let mut out = Vec::new();
        match self.format {
            CursorResponseFormat::AnthropicMessages => {
                if self.thinking_block.is_none() {
                    let idx = self.alloc_block();
                    self.thinking_block = Some(idx);
                    out.push(anthropic_event(
                        "content_block_start",
                        json!({
                            "type": "content_block_start",
                            "index": idx,
                            "content_block": { "type": "thinking", "thinking": "" }
                        }),
                    ));
                }
                let idx = self.thinking_block.unwrap();
                out.push(anthropic_event(
                    "content_block_delta",
                    json!({
                        "type": "content_block_delta",
                        "index": idx,
                        "delta": { "type": "thinking_delta", "thinking": text }
                    }),
                ));
            }
            CursorResponseFormat::OpenAiResponses => {
                if self.reasoning_item.is_none() {
                    let idx = self.alloc_output();
                    let item_id = format!("rs_{}", random_id_hex());
                    out.push(event(
                        "response.output_item.added",
                        json!({
                            "type": "response.output_item.added",
                            "output_index": idx,
                            "item": {
                                "id": item_id,
                                "type": "reasoning",
                                "summary": [{
                                    "type": "summary_text",
                                    "text": self.aggregate_reasoning
                                }]
                            }
                        }),
                    ));
                    self.reasoning_item = Some(OutputRef {
                        index: idx,
                        item_id,
                    });
                }
                let oref = self.reasoning_item.as_ref().unwrap().clone();
                out.push(event(
                    "response.reasoning_summary_text.delta",
                    json!({
                        "type": "response.reasoning_summary_text.delta",
                        "item_id": oref.item_id,
                        "output_index": oref.index,
                        "summary_index": 0,
                        "delta": text
                    }),
                ));
            }
            CursorResponseFormat::OpenAiChatCompletions => {
                // OpenAI Chat doesn't have native reasoning channels; surface
                // as a custom delta field so cc-switch's downstream can pass
                // it through if interested.
                out.push(chat_chunk(
                    &self.msg_id,
                    &self.model,
                    json!({ "reasoning_content": text }),
                    None,
                ));
            }
            CursorResponseFormat::GeminiGenerateContent => {}
        }
        self.output_tokens = self.output_tokens.saturating_add(estimate_tokens(text));
        out
    }

    fn close_thinking(&mut self) -> Vec<String> {
        let mut out = Vec::new();
        match self.format {
            CursorResponseFormat::AnthropicMessages => {
                if let Some(idx) = self.thinking_block.take() {
                    out.push(anthropic_event(
                        "content_block_stop",
                        json!({ "type": "content_block_stop", "index": idx }),
                    ));
                }
            }
            CursorResponseFormat::OpenAiResponses => {
                if let Some(oref) = self.reasoning_item.take() {
                    out.push(event(
                        "response.output_item.done",
                        json!({
                            "type": "response.output_item.done",
                            "output_index": oref.index,
                            "item": {
                                "id": oref.item_id,
                                "type": "reasoning",
                                "summary": [{
                                    "type": "summary_text",
                                    "text": self.aggregate_reasoning
                                }]
                            }
                        }),
                    ));
                }
            }
            CursorResponseFormat::OpenAiChatCompletions => {}
            CursorResponseFormat::GeminiGenerateContent => {}
        }
        out
    }

    fn tool_call(&mut self, tc: &CapturedToolCall) -> Vec<String> {
        self.aggregate_tool_calls.push(tc.clone());
        // Close any currently open text/thinking block first — tool calls
        // can interleave but most consumers expect text to flush before a
        // tool block opens.
        let mut out = self.close_thinking();
        if let Some(idx) = self.text_block.take() {
            if let CursorResponseFormat::AnthropicMessages = self.format {
                out.push(anthropic_event(
                    "content_block_stop",
                    json!({ "type": "content_block_stop", "index": idx }),
                ));
            }
        }
        if let Some(oref) = self.text_item.take() {
            if let CursorResponseFormat::OpenAiResponses = self.format {
                out.push(event(
                    "response.output_text.done",
                    json!({
                        "type": "response.output_text.done",
                        "item_id": oref.item_id,
                        "output_index": oref.index,
                        "content_index": 0,
                            "text": self.aggregate_text
                    }),
                ));
                out.push(event(
                    "response.output_item.done",
                    json!({
                        "type": "response.output_item.done",
                        "output_index": oref.index,
                        "item": {
                            "id": oref.item_id,
                            "type": "message",
                            "role": "assistant",
                            "content": [{
                                "type": "output_text",
                                "text": self.aggregate_text
                            }]
                        }
                    }),
                ));
            }
        }

        match self.format {
            CursorResponseFormat::AnthropicMessages => {
                let idx = self.alloc_block();
                let input: Value = serde_json::from_str(&tc.arguments_json).unwrap_or(json!({}));
                out.push(anthropic_event(
                    "content_block_start",
                    json!({
                        "type": "content_block_start",
                        "index": idx,
                        "content_block": {
                            "type": "tool_use",
                            "id": tc.id,
                            "name": tc.name,
                            "input": {}
                        }
                    }),
                ));
                // Stream arguments as a single input_json_delta then close.
                let json_text = serde_json::to_string(&input).unwrap_or_else(|_| "{}".to_string());
                out.push(anthropic_event(
                    "content_block_delta",
                    json!({
                        "type": "content_block_delta",
                        "index": idx,
                        "delta": { "type": "input_json_delta", "partial_json": json_text }
                    }),
                ));
                out.push(anthropic_event(
                    "content_block_stop",
                    json!({ "type": "content_block_stop", "index": idx }),
                ));
                // Don't remove from tool_blocks — done_events uses it to set stop_reason.
            }
            CursorResponseFormat::OpenAiResponses => {
                let idx = self.alloc_output();
                let item_id = format!("fc_{}", random_id_hex());
                self.tool_items.insert(
                    tc.id.clone(),
                    OutputRef {
                        index: idx,
                        item_id: item_id.clone(),
                    },
                );
                out.push(event(
                    "response.output_item.added",
                    json!({
                        "type": "response.output_item.added",
                        "output_index": idx,
                        "item": {
                            "id": item_id,
                            "type": "function_call",
                            "name": tc.name,
                            "call_id": tc.id,
                            "arguments": ""
                        }
                    }),
                ));
                out.push(event(
                    "response.function_call_arguments.delta",
                    json!({
                        "type": "response.function_call_arguments.delta",
                        "item_id": item_id,
                        "output_index": idx,
                        "delta": tc.arguments_json
                    }),
                ));
                out.push(event(
                    "response.function_call_arguments.done",
                    json!({
                        "type": "response.function_call_arguments.done",
                        "item_id": item_id,
                        "output_index": idx,
                        "arguments": tc.arguments_json
                    }),
                ));
                out.push(event(
                    "response.output_item.done",
                    json!({
                        "type": "response.output_item.done",
                        "output_index": idx,
                        "item": {
                            "id": item_id,
                            "type": "function_call",
                            "name": tc.name,
                            "call_id": tc.id,
                            "arguments": tc.arguments_json
                        }
                    }),
                ));
                // After done, remove from open items (already emitted final
                // form). Keep the entry in `tool_items` so `done_events` can
                // see how many were emitted? — no, remove because we already
                // closed it inline.
                self.tool_items.remove(&tc.id);
            }
            CursorResponseFormat::OpenAiChatCompletions => {
                let tool_idx = self.chat_tool_call_count as u64;
                self.chat_tool_call_count = self.chat_tool_call_count.saturating_add(1);
                out.push(chat_chunk(
                    &self.msg_id,
                    &self.model,
                    json!({
                        "role": "assistant",
                        "tool_calls": [{
                            "index": tool_idx,
                            "id": tc.id,
                            "type": "function",
                            "function": {
                                "name": tc.name,
                                "arguments": tc.arguments_json
                            }
                        }]
                    }),
                    None,
                ));
            }
            CursorResponseFormat::GeminiGenerateContent => {
                let args: Value = serde_json::from_str(&tc.arguments_json).unwrap_or(json!({}));
                out.push(gemini_function_call_chunk(
                    &self.msg_id,
                    &self.model,
                    &tc.name,
                    args,
                ));
            }
        }
        out
    }

    fn alloc_block(&mut self) -> u32 {
        let i = self.next_block_idx;
        self.next_block_idx += 1;
        i
    }

    fn alloc_output(&mut self) -> u32 {
        let i = self.next_output_idx;
        self.next_output_idx += 1;
        i
    }

    fn responses_base(&self) -> Value {
        json!({
            "id": self.msg_id,
            "object": "response",
            "model": self.model,
            "status": "in_progress",
            "output": [],
            "usage": null
        })
    }

    fn responses_output_snapshot(&self) -> Value {
        let mut output = Vec::new();
        if !self.aggregate_reasoning.is_empty() {
            output.push(json!({
                "id": format!("rs_{}", self.msg_id),
                "type": "reasoning",
                "summary": [{
                    "type": "summary_text",
                    "text": self.aggregate_reasoning
                }]
            }));
        }
        if !self.aggregate_text.is_empty() {
            output.push(json!({
                "id": format!("msg_{}", self.msg_id),
                "type": "message",
                "role": "assistant",
                "content": [{
                    "type": "output_text",
                    "text": self.aggregate_text
                }]
            }));
        }
        for tc in &self.aggregate_tool_calls {
            output.push(json!({
                "id": format!("fc_{}", tc.id),
                "type": "function_call",
                "name": tc.name,
                "call_id": tc.id,
                "arguments": tc.arguments_json
            }));
        }
        Value::Array(output)
    }

    fn gemini_json_response(&self) -> Value {
        let mut parts = Vec::new();
        if !self.aggregate_text.is_empty() {
            parts.push(json!({"text": self.aggregate_text}));
        } else if !self.aggregate_reasoning.is_empty() {
            parts.push(json!({"text": self.aggregate_reasoning}));
        }
        for tc in &self.aggregate_tool_calls {
            let args: Value = serde_json::from_str(&tc.arguments_json).unwrap_or(json!({}));
            parts.push(json!({
                "functionCall": {
                    "name": tc.name,
                    "args": args
                }
            }));
        }
        if parts.is_empty() {
            parts.push(json!({"text": ""}));
        }
        json!({
            "candidates": [{
                "content": {
                    "role": "model",
                    "parts": parts
                },
                "finishReason": if self.aggregate_tool_calls.is_empty() { "STOP" } else { "FUNCTION_CALL" }
            }],
            "usageMetadata": {
                "promptTokenCount": self.input_tokens,
                "candidatesTokenCount": self.output_tokens,
                "totalTokenCount": self.input_tokens + self.output_tokens
            },
            "modelVersion": self.model,
            "responseId": self.msg_id
        })
    }
}

// ─── SSE encoding helpers ──────────────────────────────────────────────────

fn anthropic_event(event_name: &str, data: Value) -> String {
    format!("event: {event_name}\ndata: {}\n\n", data)
}

fn event(event_name: &str, data: Value) -> String {
    format!("event: {event_name}\ndata: {}\n\n", data)
}

fn chat_chunk(id: &str, model: &str, delta: Value, finish: Option<&str>) -> String {
    let body = json!({
        "id": id,
        "object": "chat.completion.chunk",
        "model": model,
        "created": chrono::Utc::now().timestamp(),
        "choices": [{
            "index": 0,
            "delta": delta,
            "finish_reason": finish
        }]
    });
    format!("data: {}\n\n", body)
}

fn chat_chunk_usage(id: &str, model: &str, input: u32, output: u32) -> String {
    let body = json!({
        "id": id,
        "object": "chat.completion.chunk",
        "model": model,
        "created": chrono::Utc::now().timestamp(),
        "choices": [],
        "usage": {
            "prompt_tokens": input,
            "completion_tokens": output,
            "total_tokens": input + output
        }
    });
    format!("data: {}\n\n", body)
}

fn gemini_chunk(
    id: &str,
    model: &str,
    text: Option<&str>,
    finish_reason: Option<&str>,
    input_tokens: Option<u32>,
    output_tokens: Option<u32>,
) -> String {
    let mut body = json!({
        "modelVersion": model,
        "responseId": id
    });
    if text.is_some() || finish_reason.is_some() {
        let mut candidate = json!({});
        if let Some(text) = text {
            candidate["content"] = json!({
                "role": "model",
                "parts": [{"text": text}]
            });
        }
        if let Some(reason) = finish_reason {
            candidate["finishReason"] = json!(reason);
        }
        body["candidates"] = json!([candidate]);
    }
    if input_tokens.is_some() || output_tokens.is_some() {
        let input = input_tokens.unwrap_or(0);
        let output = output_tokens.unwrap_or(0);
        body["usageMetadata"] = json!({
            "promptTokenCount": input,
            "candidatesTokenCount": output,
            "totalTokenCount": input + output
        });
    }
    format!("data: {}\n\n", body)
}

fn gemini_function_call_chunk(id: &str, model: &str, name: &str, args: Value) -> String {
    let body = json!({
        "modelVersion": model,
        "responseId": id,
        "candidates": [{
            "content": {
                "role": "model",
                "parts": [{
                    "functionCall": {
                        "name": name,
                        "args": args
                    }
                }]
            },
            "finishReason": "FUNCTION_CALL"
        }]
    });
    format!("data: {}\n\n", body)
}

fn estimate_tokens(text: &str) -> u32 {
    ((text.len() as f64) / 3.6).ceil() as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marker_filter_passes_plain_text() {
        let mut f = ComposerMarkerFilter::default();
        let events = f.push("hello world");
        match &events[0] {
            MarkerEvent::Text(t) => assert_eq!(t, "hello world"),
            _ => panic!("expected text"),
        }
    }

    #[test]
    fn marker_filter_extracts_tool_call() {
        let mut f = ComposerMarkerFilter::default();
        let mut evs = Vec::new();
        evs.extend(f.push("prelude "));
        evs.extend(f.push(
            "<|tool_calls_begin|><|tool_call_begin|>weather<|tool_sep|>{\"city\":\"BJ\"}<|tool_call_end|><|tool_calls_end|>",
        ));
        evs.extend(f.flush());
        let mut text = String::new();
        let mut tool: Option<CapturedToolCall> = None;
        for e in evs {
            match e {
                MarkerEvent::Text(t) => text.push_str(&t),
                MarkerEvent::ToolCall(tc) => tool = Some(tc),
            }
        }
        assert!(text.contains("prelude"));
        let tc = tool.unwrap();
        assert_eq!(tc.name, "weather");
        assert!(tc.arguments_json.contains("BJ"));
    }

    #[test]
    fn marker_filter_holds_partial_prefix() {
        let mut f = ComposerMarkerFilter::default();
        let evs = f.push("text and then <|tool_calls_b");
        // Should emit "text and then " only; the partial marker stays buffered.
        let mut texts: Vec<String> = Vec::new();
        for e in evs {
            if let MarkerEvent::Text(t) = e {
                texts.push(t);
            }
        }
        assert_eq!(texts.join(""), "text and then ");
    }

    #[test]
    fn marker_filter_handles_non_ascii_plain_text() {
        let mut f = ComposerMarkerFilter::default();
        let evs = f.push("我是 Composer");
        let mut texts: Vec<String> = Vec::new();
        for e in evs {
            if let MarkerEvent::Text(t) = e {
                texts.push(t);
            }
        }
        assert_eq!(texts.join(""), "我是 Composer");
    }

    #[test]
    fn marker_filter_holds_partial_prefix_after_non_ascii_text() {
        let mut f = ComposerMarkerFilter::default();
        let evs = f.push("我是 <|tool_calls_b");
        let mut texts: Vec<String> = Vec::new();
        for e in evs {
            if let MarkerEvent::Text(t) = e {
                texts.push(t);
            }
        }
        assert_eq!(texts.join(""), "我是 ");
        assert_eq!(f.flush().len(), 1);
    }

    #[test]
    fn anthropic_emits_message_start() {
        let mut w = AgentSseWriter::new(
            "claude".to_string(),
            CursorResponseFormat::AnthropicMessages,
            42,
        );
        let s = w.start_events();
        assert_eq!(s.len(), 1);
        assert!(s[0].contains("message_start"));
        assert!(s[0].contains("\"input_tokens\":42"));
    }

    #[test]
    fn anthropic_tool_call_round_trip() {
        let mut w = AgentSseWriter::new(
            "claude".to_string(),
            CursorResponseFormat::AnthropicMessages,
            0,
        );
        w.start_events();
        let mut events = Vec::new();
        events.extend(w.event(&AgentEvent::Text("I'll check.".to_string())));
        events.extend(w.event(&AgentEvent::ToolCall(CapturedToolCall {
            id: "tc_1".to_string(),
            name: "weather".to_string(),
            arguments_json: "{\"city\":\"BJ\"}".to_string(),
        })));
        events.extend(w.done_events());
        let joined = events.join("");
        assert!(joined.contains("tool_use"));
        assert!(joined.contains("tc_1"));
        assert!(joined.contains("weather"));
        assert!(joined.contains("message_stop"));
        assert!(joined.contains("\"stop_reason\":\"tool_use\""));
    }

    #[test]
    fn openai_chat_finish_reason_tool_calls() {
        let mut w = AgentSseWriter::new(
            "gpt-5".to_string(),
            CursorResponseFormat::OpenAiChatCompletions,
            0,
        );
        let mut events = Vec::new();
        events.extend(w.event(&AgentEvent::ToolCall(CapturedToolCall {
            id: "call_1".to_string(),
            name: "weather".to_string(),
            arguments_json: "{}".to_string(),
        })));
        events.extend(w.done_events());
        let joined = events.join("");
        assert!(joined.contains("finish_reason\":\"tool_calls"));
        assert!(joined.contains("[DONE]"));
    }

    #[test]
    fn responses_tool_call_emits_function_call_arguments() {
        let mut w = AgentSseWriter::new(
            "gpt-5".to_string(),
            CursorResponseFormat::OpenAiResponses,
            0,
        );
        w.start_events();
        let mut events = Vec::new();
        events.extend(w.event(&AgentEvent::ToolCall(CapturedToolCall {
            id: "call_1".to_string(),
            name: "weather".to_string(),
            arguments_json: "{\"city\":\"BJ\"}".to_string(),
        })));
        events.extend(w.done_events());
        let joined = events.join("");
        assert!(joined.contains("function_call_arguments.delta"));
        assert!(joined.contains("function_call_arguments.done"));
        assert!(joined.contains("response.completed"));
    }

    #[test]
    fn non_stream_chat_json_preserves_text() {
        let mut w = AgentSseWriter::new(
            "gpt-5".to_string(),
            CursorResponseFormat::OpenAiChatCompletions,
            3,
        );
        w.event(&AgentEvent::Text("hello".to_string()));
        let body = w.json_response();
        assert_eq!(body["choices"][0]["message"]["content"], "hello");
        assert_eq!(body["choices"][0]["finish_reason"], "stop");
        assert_eq!(body["usage"]["prompt_tokens"], 3);
    }

    #[test]
    fn responses_completed_snapshot_preserves_output_text() {
        let mut w = AgentSseWriter::new(
            "gpt-5".to_string(),
            CursorResponseFormat::OpenAiResponses,
            0,
        );
        w.start_events();
        let mut events = Vec::new();
        events.extend(w.event(&AgentEvent::Text("final answer".to_string())));
        events.extend(w.done_events());
        let joined = events.join("");
        assert!(joined.contains("\"text\":\"final answer\""));
        let body = w.json_response();
        assert_eq!(body["output"][0]["content"][0]["text"], "final answer");
    }

    #[test]
    fn responses_output_text_done_preserves_text() {
        let mut w = AgentSseWriter::new(
            "gpt-5".to_string(),
            CursorResponseFormat::OpenAiResponses,
            0,
        );
        w.start_events();
        let mut events = Vec::new();
        events.extend(w.event(&AgentEvent::Text("final answer".to_string())));
        events.extend(w.done_events());
        let joined = events.join("");
        assert!(joined.contains("\"type\":\"response.output_text.done\""));
        assert!(joined.contains("\"text\":\"final answer\""));
    }

    #[test]
    fn responses_text_before_tool_call_closes_with_text() {
        let mut w = AgentSseWriter::new(
            "gpt-5".to_string(),
            CursorResponseFormat::OpenAiResponses,
            0,
        );
        w.start_events();
        let mut events = Vec::new();
        events.extend(w.event(&AgentEvent::Text("checking".to_string())));
        events.extend(w.event(&AgentEvent::ToolCall(CapturedToolCall {
            id: "call_1".to_string(),
            name: "weather".to_string(),
            arguments_json: "{}".to_string(),
        })));
        let joined = events.join("");
        assert!(joined.contains("\"type\":\"response.output_text.done\""));
        assert!(joined.contains("\"text\":\"checking\""));
        assert!(joined.contains("\"type\":\"function_call\""));
    }

    #[test]
    fn gemini_json_response_preserves_text_and_usage() {
        let mut w = AgentSseWriter::new(
            "gemini-2.5-pro".to_string(),
            CursorResponseFormat::GeminiGenerateContent,
            5,
        );
        w.event(&AgentEvent::Text("hello gemini".to_string()));
        let body = w.json_response();
        assert_eq!(
            body.pointer("/candidates/0/content/parts/0/text"),
            Some(&Value::String("hello gemini".to_string()))
        );
        assert_eq!(body["usageMetadata"]["promptTokenCount"], 5);
        assert_eq!(body["modelVersion"], "gemini-2.5-pro");
    }

    #[test]
    fn gemini_stream_events_are_data_frames() {
        let mut w = AgentSseWriter::new(
            "gemini-2.5-pro".to_string(),
            CursorResponseFormat::GeminiGenerateContent,
            2,
        );
        let mut events = Vec::new();
        events.extend(w.event(&AgentEvent::Text("hi".to_string())));
        events.extend(w.done_events());
        let joined = events.join("");
        assert!(joined.contains("data: {"));
        assert!(joined.contains("\"parts\":[{\"text\":\"hi\"}]"));
        assert!(joined.contains("\"usageMetadata\""));
        assert!(!joined.contains("event:"));
    }

    // ── Regression: first-frame timeout terminal events ───────────────────

    #[test]
    fn responses_error_then_done_emits_response_failed_and_done() {
        let mut w = AgentSseWriter::new(
            "gpt-5".to_string(),
            CursorResponseFormat::OpenAiResponses,
            0,
        );
        w.start_events();
        let mut events = Vec::new();
        events.extend(w.error_events("首帧超时"));
        events.extend(w.done_events());
        let joined = events.join("");
        // Must contain response.failed (the terminal event for Codex CLI).
        assert!(joined.contains("event: response.failed"));
        assert!(joined.contains("\"type\":\"response.failed\""));
        assert!(joined.contains("\"status\":\"failed\""));
        // Must NOT contain response.completed (error mode skips it).
        assert!(!joined.contains("response.completed"));
        // Must end with [DONE].
        assert!(joined.contains("data: [DONE]"));
    }

    #[test]
    fn anthropic_error_then_done_emits_message_stop_with_error_stop_reason() {
        let mut w = AgentSseWriter::new(
            "claude-3".to_string(),
            CursorResponseFormat::AnthropicMessages,
            0,
        );
        w.start_events();
        let mut events = Vec::new();
        events.extend(w.error_events("首帧超时"));
        events.extend(w.done_events());
        let joined = events.join("");
        // Must contain message_stop (the terminal event for Claude CLI).
        assert!(joined.contains("event: message_stop"));
        // stop_reason must be "error", not "end_turn".
        assert!(joined.contains("\"stop_reason\":\"error\""));
    }
}
