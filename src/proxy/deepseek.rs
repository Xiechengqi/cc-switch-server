use crate::proxy::ProxyError;
use async_stream::stream;
use bytes::Bytes;
use futures_util::{Stream, StreamExt};
use rand::RngCore;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU64, Ordering};

const MAX_STREAM_BYTES: usize = 64 * 1024 * 1024;
const MAX_SSE_LINE_BYTES: usize = 2 * 1024 * 1024;
const MAX_TOOL_SCHEMA_BYTES: usize = 512 * 1024;
const MAX_TOOL_CALLS: usize = 64;
const MAX_TOOL_NAME_BYTES: usize = 128;

static MESSAGE_ID_COUNTER: AtomicU64 = AtomicU64::new(1);
static TOOL_CALL_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeepSeekReviewedModel {
    pub id: &'static str,
    pub display_name: &'static str,
    pub thinking: bool,
    pub search: bool,
}

const DEEPSEEK_REVIEWED_MODELS: &[DeepSeekReviewedModel] = &[
    DeepSeekReviewedModel {
        id: "deepseek-v4-flash",
        display_name: "DeepSeek V4 Flash",
        thinking: true,
        search: false,
    },
    DeepSeekReviewedModel {
        id: "deepseek-v4-pro",
        display_name: "DeepSeek V4 Pro",
        thinking: true,
        search: false,
    },
    DeepSeekReviewedModel {
        id: "deepseek-v4-pro-nothinking",
        display_name: "DeepSeek V4 Pro (No Thinking)",
        thinking: false,
        search: false,
    },
    DeepSeekReviewedModel {
        id: "deepseek-v4-pro-search",
        display_name: "DeepSeek V4 Pro Search",
        thinking: true,
        search: true,
    },
    DeepSeekReviewedModel {
        id: "deepseek-v4-pro-search-nothinking",
        display_name: "DeepSeek V4 Pro Search (No Thinking)",
        thinking: false,
        search: true,
    },
];

pub fn reviewed_model_catalog() -> &'static [DeepSeekReviewedModel] {
    DEEPSEEK_REVIEWED_MODELS
}

#[derive(Debug, Clone)]
pub struct PreparedDeepSeekRequest {
    pub prompt: String,
    pub thinking_enabled: bool,
    pub search_enabled: bool,
    tool_contract: Option<DeepSeekToolContract>,
}

impl PreparedDeepSeekRequest {
    pub fn has_tools(&self) -> bool {
        self.tool_contract.is_some()
    }
}

#[derive(Debug, Clone)]
struct DeepSeekToolContract {
    nonce: String,
    allowed_names: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DeepSeekToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct DeepSeekCollectedResponse {
    pub thinking: String,
    pub content: String,
    pub tool_calls: Vec<DeepSeekToolCall>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutputLane {
    Thinking,
    Content,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum DeepSeekDelta {
    Thinking(String),
    Content(String),
}

#[derive(Debug, Clone, Default)]
struct SearchResult {
    cite_index: Option<u64>,
    title: String,
    url: String,
}

#[derive(Debug)]
struct DeepSeekSseDecoder {
    current_lane: Option<OutputLane>,
    content_started: bool,
    finished_marker: bool,
    done_marker: bool,
    search_results: Vec<SearchResult>,
}

impl DeepSeekSseDecoder {
    fn new(thinking_expected: bool) -> Self {
        Self {
            current_lane: thinking_expected.then_some(OutputLane::Thinking),
            content_started: false,
            finished_marker: false,
            done_marker: false,
            search_results: Vec::new(),
        }
    }

    fn consume_line(&mut self, line: &str) -> Result<Vec<DeepSeekDelta>, String> {
        let Some(data) = line.trim_start().strip_prefix("data:") else {
            return Ok(Vec::new());
        };
        let data = data.trim();
        if data.is_empty() {
            return Ok(Vec::new());
        }
        if self.done_marker {
            return Err("DeepSeek stream contains data after [DONE]".to_string());
        }
        if data == "[DONE]" {
            self.done_marker = true;
            return Ok(Vec::new());
        }
        let value: Value = serde_json::from_str(data)
            .map_err(|error| format!("DeepSeek SSE data is not valid JSON: {error}"))?;
        let path = value.get("p").and_then(Value::as_str).unwrap_or("").trim();
        let operation = value.get("o").and_then(Value::as_str).unwrap_or("");
        let payload = value.get("v").unwrap_or(&Value::Null);

        if path == "response/status"
            && payload
                .as_str()
                .is_some_and(|status| status.eq_ignore_ascii_case("FINISHED"))
        {
            if self.finished_marker {
                return Err("DeepSeek stream contains a duplicate FINISHED marker".to_string());
            }
            self.finished_marker = true;
            return Ok(Vec::new());
        }
        if self.finished_marker
            && !matches!(path, "response/search_results" | "response/search_status")
        {
            return Err(format!(
                "DeepSeek stream contains business data after FINISHED: {path:?}"
            ));
        }
        if path == "response/search_status" || should_ignore_path(path) {
            return Ok(Vec::new());
        }
        if path == "response/search_results" {
            self.apply_search_results(payload, operation);
            return Ok(Vec::new());
        }

        let mut deltas = Vec::new();
        if path == "response/fragments" {
            for fragment in values(payload) {
                self.push_fragment(fragment, &mut deltas)?;
            }
            return Ok(deltas);
        }
        if path == "response/content" || path.ends_with("/content") {
            if let Some(text) = string_payload(payload) {
                self.push_text(OutputLane::Content, text, &mut deltas)?;
            }
            return Ok(deltas);
        }
        if path.contains("thinking") || path.contains("reasoning") {
            if let Some(text) = string_payload(payload) {
                self.push_text(OutputLane::Thinking, text, &mut deltas)?;
            }
            return Ok(deltas);
        }
        if path == "response" {
            self.consume_response_value(payload, &mut deltas)?;
            return Ok(deltas);
        }
        if path.is_empty() {
            if let Some(text) = value
                .pointer("/choices/0/delta/content")
                .or_else(|| value.pointer("/choices/0/message/content"))
                .and_then(Value::as_str)
            {
                self.push_text(OutputLane::Content, text.to_string(), &mut deltas)?;
                return Ok(deltas);
            }
        }
        Ok(deltas)
    }

    fn consume_response_value(
        &mut self,
        payload: &Value,
        deltas: &mut Vec<DeepSeekDelta>,
    ) -> Result<(), String> {
        if let Some(response) = payload.get("response") {
            if response.get("thinking_enabled").and_then(Value::as_bool) == Some(true) {
                self.current_lane = Some(OutputLane::Thinking);
            } else if response.get("thinking_enabled").and_then(Value::as_bool) == Some(false) {
                self.current_lane = Some(OutputLane::Content);
            }
            if let Some(fragments) = response.get("fragments") {
                for fragment in values(fragments) {
                    self.push_fragment(fragment, deltas)?;
                }
            }
            return Ok(());
        }
        for entry in values(payload) {
            if entry.get("p").and_then(Value::as_str) == Some("response") {
                if entry
                    .pointer("/v/thinking_enabled")
                    .and_then(Value::as_bool)
                    == Some(true)
                {
                    self.current_lane = Some(OutputLane::Thinking);
                }
                if let Some(fragments) = entry.pointer("/v/fragments") {
                    for fragment in values(fragments) {
                        self.push_fragment(fragment, deltas)?;
                    }
                }
            } else if let Some(fragments) = entry.get("v").and_then(Value::as_array) {
                for fragment in fragments {
                    self.push_fragment(fragment, deltas)?;
                }
            }
        }
        Ok(())
    }

    fn push_fragment(
        &mut self,
        fragment: &Value,
        deltas: &mut Vec<DeepSeekDelta>,
    ) -> Result<(), String> {
        let Some(object) = fragment.as_object() else {
            return Err("DeepSeek response fragment must be an object".to_string());
        };
        let kind = object
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_ascii_uppercase();
        let lane = match kind.as_str() {
            "THINK" | "THINKING" | "REASONING" => OutputLane::Thinking,
            "ANSWER" | "RESPONSE" | "CONTENT" => OutputLane::Content,
            "" => self.current_lane.unwrap_or(OutputLane::Content),
            _ => return Ok(()),
        };
        self.current_lane = Some(lane);
        if let Some(text) = object.get("content").and_then(Value::as_str) {
            self.push_text(lane, text.to_string(), deltas)?;
        }
        Ok(())
    }

    fn push_text(
        &mut self,
        lane: OutputLane,
        text: String,
        deltas: &mut Vec<DeepSeekDelta>,
    ) -> Result<(), String> {
        if text.is_empty() || text.eq_ignore_ascii_case("FINISHED") {
            return Ok(());
        }
        if lane == OutputLane::Thinking && self.content_started {
            return Err("DeepSeek stream returned thinking after answer content".to_string());
        }
        self.current_lane = Some(lane);
        self.content_started |= lane == OutputLane::Content;
        deltas.push(match lane {
            OutputLane::Thinking => DeepSeekDelta::Thinking(text),
            OutputLane::Content => DeepSeekDelta::Content(text),
        });
        Ok(())
    }

    fn apply_search_results(&mut self, payload: &Value, operation: &str) {
        let Some(entries) = payload.as_array() else {
            return;
        };
        if operation.eq_ignore_ascii_case("BATCH") {
            for entry in entries {
                let path = entry.get("p").and_then(Value::as_str).unwrap_or("");
                let Some((index, field)) = path.split_once('/') else {
                    continue;
                };
                let Ok(index) = index.parse::<usize>() else {
                    continue;
                };
                let Some(result) = self.search_results.get_mut(index) else {
                    continue;
                };
                if field == "cite_index" {
                    result.cite_index = entry.get("v").and_then(Value::as_u64);
                }
            }
            return;
        }
        self.search_results = entries
            .iter()
            .filter_map(|entry| {
                let object = entry.as_object()?;
                Some(SearchResult {
                    cite_index: object
                        .get("cite_index")
                        .or_else(|| object.get("citeIndex"))
                        .and_then(Value::as_u64),
                    title: bounded_string(object.get("title"), 1_024),
                    url: bounded_https_url(object.get("url")),
                })
            })
            .take(128)
            .collect();
    }

    fn finish(self) -> Result<String, String> {
        if !self.finished_marker && !self.done_marker {
            return Err("DeepSeek stream ended without FINISHED or [DONE]".to_string());
        }
        let mut results = self
            .search_results
            .into_iter()
            .filter(|result| result.cite_index.is_some() && !result.url.is_empty())
            .collect::<Vec<_>>();
        results.sort_by_key(|result| result.cite_index);
        results.dedup_by_key(|result| result.cite_index);
        Ok(results
            .into_iter()
            .map(|result| {
                let index = result.cite_index.unwrap_or_default();
                let title = if result.title.is_empty() {
                    result.url.clone()
                } else {
                    result.title
                };
                format!("[{index}]: [{title}]({})", result.url)
            })
            .collect::<Vec<_>>()
            .join("\n"))
    }
}

#[derive(Debug, Default)]
struct StrictLineBuffer {
    pending: Vec<u8>,
}

impl StrictLineBuffer {
    fn push(&mut self, bytes: &[u8]) -> Result<Vec<String>, String> {
        if self.pending.len().saturating_add(bytes.len()) > MAX_SSE_LINE_BYTES {
            return Err("DeepSeek SSE line exceeds 2 MiB".to_string());
        }
        self.pending.extend_from_slice(bytes);
        let mut lines = Vec::new();
        while let Some(index) = self.pending.iter().position(|byte| *byte == b'\n') {
            let mut line = self.pending.drain(..=index).collect::<Vec<_>>();
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            let line = String::from_utf8(line)
                .map_err(|_| "DeepSeek SSE contains invalid UTF-8".to_string())?;
            if !line.trim().is_empty() {
                lines.push(line);
            }
        }
        Ok(lines)
    }

    fn finish(self) -> Result<Option<String>, String> {
        if self.pending.is_empty() {
            return Ok(None);
        }
        let line = String::from_utf8(self.pending)
            .map_err(|_| "DeepSeek SSE tail contains invalid UTF-8".to_string())?;
        Ok((!line.trim().is_empty()).then_some(line))
    }
}

pub fn resolve_model(model: &str) -> Result<String, ProxyError> {
    let model = model.trim();
    let resolved = match model {
        "claude-sonnet-4-5" | "claude-sonnet-4-6" | "claude-sonnet-4-7" | "claude-3-5-sonnet" => {
            "deepseek-v4-flash"
        }
        "claude-opus-4-5" | "claude-opus-4-6" | "claude-opus-4-7" | "claude-3-opus" => {
            "deepseek-v4-pro"
        }
        reviewed
            if DEEPSEEK_REVIEWED_MODELS
                .iter()
                .any(|descriptor| descriptor.id == reviewed) =>
        {
            reviewed
        }
        _ => {
            return Err(ProxyError::bad_request(format!(
                "DeepSeek Web model is not in the reviewed catalog: {model}"
            )))
        }
    };
    Ok(resolved.to_string())
}

pub fn map_model(model: &str) -> String {
    resolve_model(model).unwrap_or_else(|_| "deepseek-v4-flash".to_string())
}

pub fn request_scoped_session_id() -> String {
    format!("request:{}", random_nonce())
}

pub fn prepare_request(
    body: &Value,
    resolved_model: &str,
) -> Result<PreparedDeepSeekRequest, ProxyError> {
    let messages = body
        .get("messages")
        .and_then(Value::as_array)
        .ok_or_else(|| ProxyError::bad_request("messages must be an array"))?;
    if messages.len() > 1_024 {
        return Err(ProxyError::bad_request(
            "DeepSeek message history exceeds 1024 entries",
        ));
    }
    let (tool_prompt, tool_contract, web_search_tool) = prepare_tool_contract(body.get("tools"))?;
    let mut system_parts = Vec::new();
    if let Some(system) = body.get("system") {
        let text = text_from_content(system);
        if !text.trim().is_empty() {
            system_parts.push(text.trim().to_string());
        }
    }
    if let Some(tool_prompt) = tool_prompt {
        system_parts.push(tool_prompt);
    }

    let mut transcript = Vec::new();
    let mut tool_names = BTreeMap::<String, String>::new();
    for message in messages {
        let role = message.get("role").and_then(Value::as_str).unwrap_or("");
        let content = message.get("content").unwrap_or(&Value::Null);
        match role {
            "user" => {
                for item in content_items(content) {
                    match item {
                        ContentItem::Text(text) if !text.trim().is_empty() => {
                            transcript.push(format!("User: {}", text.trim()));
                        }
                        ContentItem::ToolResult { id, content } => {
                            let name = tool_names.get(&id).map(String::as_str).unwrap_or("tool");
                            transcript.push(format!("Tool result ({name}): {}", content.trim()));
                        }
                        _ => {}
                    }
                }
            }
            "assistant" => {
                let mut parts = Vec::new();
                for item in content_items(content) {
                    match item {
                        ContentItem::Text(text) | ContentItem::Thinking(text)
                            if !text.trim().is_empty() =>
                        {
                            parts.push(text.trim().to_string());
                        }
                        ContentItem::ToolUse { id, name, input } => {
                            tool_names.insert(id, name.clone());
                            parts.push(format!(
                                "<tool>{{\"name\":{},\"arguments\":{}}}</tool>",
                                serde_json::to_string(&name)
                                    .unwrap_or_else(|_| "\"tool\"".to_string()),
                                input
                            ));
                        }
                        _ => {}
                    }
                }
                if !parts.is_empty() {
                    transcript.push(format!("Assistant: {}", parts.join("\n")));
                }
            }
            _ => {}
        }
    }
    let mut prompt_parts = Vec::new();
    if !system_parts.is_empty() {
        prompt_parts.push(format!(
            "<system>\n{}\n</system>",
            system_parts.join("\n\n")
        ));
    }
    if !transcript.is_empty() {
        prompt_parts.push(transcript.join("\n\n"));
    }
    if transcript
        .iter()
        .any(|line| line.starts_with("Tool result"))
    {
        prompt_parts.push(
            "Continue from the tool results above. Do not repeat a successful tool call."
                .to_string(),
        );
    }
    let prompt = prompt_parts.join("\n\n");
    if prompt.trim().is_empty() {
        return Err(ProxyError::bad_request("text prompt is empty"));
    }
    let explicit_thinking = body
        .get("thinking")
        .and_then(Value::as_object)
        .and_then(|thinking| thinking.get("type"))
        .and_then(Value::as_str)
        .is_some_and(|kind| matches!(kind, "enabled" | "adaptive"));
    let nothinking = resolved_model.contains("nothinking");
    let thinking_enabled = !nothinking
        && (explicit_thinking
            || resolved_model.contains("reason")
            || resolved_model.contains("thinking"));
    let search_enabled = resolved_model.contains("search") || web_search_tool;
    Ok(PreparedDeepSeekRequest {
        prompt,
        thinking_enabled,
        search_enabled,
        tool_contract,
    })
}

pub fn build_prompt(body: &Value) -> Result<String, ProxyError> {
    Ok(prepare_request(body, "deepseek-v4-flash")?.prompt)
}

pub fn estimate_billable_user_input_tokens(body: &Value) -> u32 {
    let Some(messages) = body.get("messages").and_then(Value::as_array) else {
        return 0;
    };
    messages
        .iter()
        .rev()
        .filter(|message| message.get("role").and_then(Value::as_str) == Some("user"))
        .map(|message| text_from_content(message.get("content").unwrap_or(&Value::Null)))
        .find(|text| !text.trim().is_empty())
        .map(|text| estimate_tokens(&text))
        .unwrap_or(0)
}

pub fn estimate_tokens(text: &str) -> u32 {
    let chars = text
        .chars()
        .filter(|character| !character.is_whitespace())
        .count() as u32;
    if chars == 0 {
        0
    } else {
        chars.div_ceil(4).max(1)
    }
}

pub async fn collect_deepseek_response<S>(
    upstream: S,
    thinking_expected: bool,
) -> Result<DeepSeekCollectedResponse, ProxyError>
where
    S: Stream<Item = Result<Bytes, reqwest::Error>> + Send,
{
    tokio::pin!(upstream);
    let mut decoder = DeepSeekSseDecoder::new(thinking_expected);
    let mut line_buffer = StrictLineBuffer::default();
    let mut retained = 0usize;
    let mut output = DeepSeekCollectedResponse::default();
    while let Some(item) = upstream.next().await {
        let bytes = item.map_err(|error| {
            ProxyError::bad_gateway(format!("DeepSeek upstream stream failed: {error}"))
        })?;
        retained = retained.saturating_add(bytes.len());
        if retained > MAX_STREAM_BYTES {
            return Err(ProxyError::bad_gateway("DeepSeek stream exceeds 64 MiB"));
        }
        for line in line_buffer.push(&bytes).map_err(ProxyError::bad_gateway)? {
            for delta in decoder
                .consume_line(&line)
                .map_err(ProxyError::bad_gateway)?
            {
                append_delta(&mut output, delta);
            }
        }
    }
    if let Some(line) = line_buffer.finish().map_err(ProxyError::bad_gateway)? {
        for delta in decoder
            .consume_line(&line)
            .map_err(ProxyError::bad_gateway)?
        {
            append_delta(&mut output, delta);
        }
    }
    let citations = decoder.finish().map_err(ProxyError::bad_gateway)?;
    if !citations.is_empty() {
        if !output.content.is_empty() {
            output.content.push_str("\n\n");
        }
        output.content.push_str(&citations);
    }
    Ok(output)
}

pub fn finalize_collected_response(
    mut collected: DeepSeekCollectedResponse,
    prepared: &PreparedDeepSeekRequest,
) -> Result<DeepSeekCollectedResponse, ProxyError> {
    if let Some(contract) = prepared.tool_contract.as_ref() {
        let (content, calls) = parse_tool_calls(&collected.content, contract)?;
        collected.content = content;
        collected.tool_calls = calls;
    }
    Ok(collected)
}

pub fn claude_message_json(
    collected: &DeepSeekCollectedResponse,
    model: &str,
    input_tokens: u32,
) -> Value {
    let mut content = Vec::new();
    if !collected.thinking.is_empty() {
        content.push(json!({"type":"thinking","thinking":collected.thinking}));
    }
    if !collected.content.is_empty() || collected.tool_calls.is_empty() {
        content.push(json!({"type":"text","text":collected.content}));
    }
    for call in &collected.tool_calls {
        content.push(json!({
            "type":"tool_use",
            "id":call.id,
            "name":call.name,
            "input":call.arguments
        }));
    }
    let output_tokens = estimate_tokens(&format!("{}{}", collected.thinking, collected.content));
    json!({
        "id": next_message_id(),
        "type": "message",
        "role": "assistant",
        "content": content,
        "model": model,
        "stop_reason": if collected.tool_calls.is_empty() {"end_turn"} else {"tool_use"},
        "stop_sequence": null,
        "usage": {"input_tokens": input_tokens, "output_tokens": output_tokens}
    })
}

pub fn collected_response_to_claude_sse(
    collected: &DeepSeekCollectedResponse,
    response_model: &str,
    input_tokens: u32,
) -> Vec<Bytes> {
    let mut output = vec![message_start(response_model, input_tokens)];
    let mut index = 0usize;
    if !collected.thinking.is_empty() {
        output.push(sse_event(
            "content_block_start",
            &json!({
                "type":"content_block_start","index":index,
                "content_block":{"type":"thinking","thinking":""}
            }),
        ));
        output.push(sse_event(
            "content_block_delta",
            &json!({
                "type":"content_block_delta","index":index,
                "delta":{"type":"thinking_delta","thinking":collected.thinking}
            }),
        ));
        output.push(content_block_stop(index));
        index += 1;
    }
    if !collected.content.is_empty() || collected.tool_calls.is_empty() {
        output.push(text_block_start(index));
        if !collected.content.is_empty() {
            output.push(text_delta(index, &collected.content));
        }
        output.push(content_block_stop(index));
        index += 1;
    }
    for call in &collected.tool_calls {
        output.push(sse_event(
            "content_block_start",
            &json!({
                "type":"content_block_start","index":index,
                "content_block":{"type":"tool_use","id":call.id,"name":call.name,"input":{}}
            }),
        ));
        output.push(sse_event(
            "content_block_delta",
            &json!({
                "type":"content_block_delta","index":index,
                "delta":{"type":"input_json_delta","partial_json":call.arguments.to_string()}
            }),
        ));
        output.push(content_block_stop(index));
        index += 1;
    }
    let output_tokens = estimate_tokens(&format!("{}{}", collected.thinking, collected.content));
    output.extend(terminal_events(
        if collected.tool_calls.is_empty() {
            "end_turn"
        } else {
            "tool_use"
        },
        output_tokens,
    ));
    output
}

pub fn deepseek_bytes_stream_to_claude_sse<S>(
    upstream: S,
    response_model: String,
    input_tokens: u32,
    thinking_expected: bool,
) -> impl Stream<Item = Result<Bytes, std::io::Error>> + Send
where
    S: Stream<Item = Result<Bytes, reqwest::Error>> + Send + 'static,
{
    stream! {
        yield Ok(message_start(&response_model, input_tokens));
        let mut decoder = DeepSeekSseDecoder::new(thinking_expected);
        let mut line_buffer = StrictLineBuffer::default();
        let mut retained = 0usize;
        let mut thinking_index = None;
        let mut content_index = None;
        let mut thinking_open = false;
        let mut content_open = false;
        let mut next_index = 0usize;
        let mut output_text = String::new();
        tokio::pin!(upstream);
        while let Some(item) = upstream.next().await {
            let bytes = match item {
                Ok(bytes) => bytes,
                Err(error) => {
                    yield Err(std::io::Error::other(format!("DeepSeek upstream stream failed: {error}")));
                    return;
                }
            };
            retained = retained.saturating_add(bytes.len());
            if retained > MAX_STREAM_BYTES {
                yield Err(std::io::Error::other("DeepSeek stream exceeds 64 MiB"));
                return;
            }
            let lines = match line_buffer.push(&bytes) {
                Ok(lines) => lines,
                Err(error) => {
                    yield Err(std::io::Error::other(error));
                    return;
                }
            };
            for line in lines {
                let deltas = match decoder.consume_line(&line) {
                    Ok(deltas) => deltas,
                    Err(error) => {
                        yield Err(std::io::Error::other(error));
                        return;
                    }
                };
                for delta in deltas {
                    match delta {
                        DeepSeekDelta::Thinking(text) => {
                            let was_none = thinking_index.is_none();
                            let index = *thinking_index.get_or_insert_with(|| {
                                let index = next_index;
                                next_index += 1;
                                index
                            });
                            if was_none {
                                yield Ok(sse_event("content_block_start", &json!({
                                    "type":"content_block_start","index":index,
                                    "content_block":{"type":"thinking","thinking":""}
                                })));
                                thinking_open = true;
                            }
                            output_text.push_str(&text);
                            yield Ok(sse_event("content_block_delta", &json!({
                                "type":"content_block_delta","index":index,
                                "delta":{"type":"thinking_delta","thinking":text}
                            })));
                        }
                        DeepSeekDelta::Content(text) => {
                            let was_none = content_index.is_none();
                            let index = *content_index.get_or_insert_with(|| {
                                let index = next_index;
                                next_index += 1;
                                index
                            });
                            if was_none {
                                if thinking_open {
                                    yield Ok(content_block_stop(thinking_index.expect("thinking index exists while open")));
                                    thinking_open = false;
                                }
                                yield Ok(text_block_start(index));
                                content_open = true;
                            }
                            output_text.push_str(&text);
                            yield Ok(text_delta(index, &text));
                        }
                    }
                }
            }
        }
        let tail = match line_buffer.finish() {
            Ok(tail) => tail,
            Err(error) => {
                yield Err(std::io::Error::other(error));
                return;
            }
        };
        if let Some(line) = tail {
            let deltas = match decoder.consume_line(&line) {
                Ok(deltas) => deltas,
                Err(error) => {
                    yield Err(std::io::Error::other(error));
                    return;
                }
            };
            for delta in deltas {
                match delta {
                    DeepSeekDelta::Thinking(text) => {
                        let was_none = thinking_index.is_none();
                        let index = *thinking_index.get_or_insert_with(|| {
                            let index = next_index;
                            next_index += 1;
                            index
                        });
                        if was_none {
                            yield Ok(sse_event("content_block_start", &json!({
                                "type":"content_block_start","index":index,
                                "content_block":{"type":"thinking","thinking":""}
                            })));
                            thinking_open = true;
                        }
                        output_text.push_str(&text);
                        yield Ok(sse_event("content_block_delta", &json!({
                            "type":"content_block_delta","index":index,
                            "delta":{"type":"thinking_delta","thinking":text}
                        })));
                    }
                    DeepSeekDelta::Content(text) => {
                        let was_none = content_index.is_none();
                        let index = *content_index.get_or_insert_with(|| {
                            let index = next_index;
                            next_index += 1;
                            index
                        });
                        if was_none {
                            if thinking_open {
                                yield Ok(content_block_stop(thinking_index.expect("thinking index exists while open")));
                                thinking_open = false;
                            }
                            yield Ok(text_block_start(index));
                            content_open = true;
                        }
                        output_text.push_str(&text);
                        yield Ok(text_delta(index, &text));
                    }
                }
            }
        }
        let citations = match decoder.finish() {
            Ok(citations) => citations,
            Err(error) => {
                yield Err(std::io::Error::other(error));
                return;
            }
        };
        if !citations.is_empty() {
            let was_none = content_index.is_none();
            let index = *content_index.get_or_insert_with(|| {
                let index = next_index;
                next_index += 1;
                index
            });
            if was_none {
                if thinking_open {
                    yield Ok(content_block_stop(thinking_index.expect("thinking index exists while open")));
                    thinking_open = false;
                }
                yield Ok(text_block_start(index));
                content_open = true;
            }
            let citations = if output_text.is_empty() { citations } else { format!("\n\n{citations}") };
            output_text.push_str(&citations);
            yield Ok(text_delta(index, &citations));
        }
        if thinking_open { yield Ok(content_block_stop(thinking_index.expect("thinking index exists while open"))); }
        if content_open { yield Ok(content_block_stop(content_index.expect("content index exists while open"))); }
        if thinking_index.is_none() && content_index.is_none() {
            yield Ok(text_block_start(0));
            yield Ok(content_block_stop(0));
        }
        for event in terminal_events("end_turn", estimate_tokens(&output_text)) {
            yield Ok(event);
        }
    }
}

fn prepare_tool_contract(
    tools: Option<&Value>,
) -> Result<(Option<String>, Option<DeepSeekToolContract>, bool), ProxyError> {
    let Some(tools) = tools.and_then(Value::as_array) else {
        return Ok((None, None, false));
    };
    if tools.len() > MAX_TOOL_CALLS {
        return Err(ProxyError::bad_request("DeepSeek tool count exceeds 64"));
    }
    let nonce = random_nonce();
    let mut definitions = Vec::new();
    let mut names = BTreeSet::new();
    let mut web_search = false;
    let mut schema_bytes = 0usize;
    for tool in tools {
        if tool
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(|kind| kind.starts_with("web_search"))
        {
            web_search = true;
            continue;
        }
        let Some(name) = tool.get("name").and_then(Value::as_str).map(str::trim) else {
            return Err(ProxyError::bad_request(
                "DeepSeek client tool is missing a name",
            ));
        };
        if name.is_empty() || name.len() > MAX_TOOL_NAME_BYTES || !names.insert(name.to_string()) {
            return Err(ProxyError::bad_request(
                "DeepSeek client tool name is invalid or duplicated",
            ));
        }
        let input_schema = tool
            .get("input_schema")
            .cloned()
            .unwrap_or_else(|| json!({"type":"object"}));
        if !input_schema.is_object() {
            return Err(ProxyError::bad_request(
                "DeepSeek tool input_schema must be an object",
            ));
        }
        let definition = json!({
            "name":name,
            "description":tool.get("description").and_then(Value::as_str).unwrap_or(""),
            "input_schema":input_schema
        });
        schema_bytes = schema_bytes.saturating_add(definition.to_string().len());
        if schema_bytes > MAX_TOOL_SCHEMA_BYTES {
            return Err(ProxyError::bad_request(
                "DeepSeek tool schemas exceed 512 KiB",
            ));
        }
        definitions.push(definition);
    }
    if definitions.is_empty() {
        return Ok((None, None, web_search));
    }
    let prompt = format!(
        "You may call only the tools below. To call a tool, output only one or more exact blocks with no markdown:\n<tool>{{\"name\":\"tool_name\",\"arguments\":{{}},\"_nonce\":\"{nonce}\"}}</tool>\nThe name must be listed, arguments must be a JSON object, and _nonce must match exactly.\nTools:\n{}",
        serde_json::to_string(&definitions)
            .map_err(|error| ProxyError::bad_gateway(error.to_string()))?
    );
    Ok((
        Some(prompt),
        Some(DeepSeekToolContract {
            nonce,
            allowed_names: names,
        }),
        web_search,
    ))
}

fn parse_tool_calls(
    text: &str,
    contract: &DeepSeekToolContract,
) -> Result<(String, Vec<DeepSeekToolCall>), ProxyError> {
    let mut cursor = 0usize;
    let mut cleaned = String::new();
    let mut calls = Vec::new();
    while let Some(relative) = text[cursor..].find("<tool>") {
        let start = cursor + relative;
        cleaned.push_str(&text[cursor..start]);
        let body_start = start + "<tool>".len();
        let Some(end_relative) = text[body_start..].find("</tool>") else {
            return Err(ProxyError::bad_gateway("DeepSeek tool block is not closed"));
        };
        let end = body_start + end_relative;
        let value: Value = serde_json::from_str(text[body_start..end].trim()).map_err(|error| {
            ProxyError::bad_gateway(format!("DeepSeek tool JSON is invalid: {error}"))
        })?;
        let object = value
            .as_object()
            .ok_or_else(|| ProxyError::bad_gateway("DeepSeek tool block must be a JSON object"))?;
        let name = object
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|name| contract.allowed_names.contains(*name))
            .ok_or_else(|| ProxyError::bad_gateway("DeepSeek returned an unknown tool name"))?;
        if object.get("_nonce").and_then(Value::as_str) != Some(contract.nonce.as_str()) {
            return Err(ProxyError::bad_gateway(
                "DeepSeek tool nonce does not match the request",
            ));
        }
        let arguments = object
            .get("arguments")
            .filter(|arguments| arguments.is_object())
            .cloned()
            .ok_or_else(|| ProxyError::bad_gateway("DeepSeek tool arguments must be an object"))?;
        calls.push(DeepSeekToolCall {
            id: next_tool_call_id(),
            name: name.to_string(),
            arguments,
        });
        if calls.len() > MAX_TOOL_CALLS {
            return Err(ProxyError::bad_gateway(
                "DeepSeek returned more than 64 tool calls",
            ));
        }
        cursor = end + "</tool>".len();
    }
    cleaned.push_str(&text[cursor..]);
    if cleaned.contains("<tool") || cleaned.contains("</tool") {
        return Err(ProxyError::bad_gateway(
            "DeepSeek returned a non-canonical tool wrapper",
        ));
    }
    Ok((cleaned.trim().to_string(), calls))
}

fn append_delta(output: &mut DeepSeekCollectedResponse, delta: DeepSeekDelta) {
    match delta {
        DeepSeekDelta::Thinking(text) => output.thinking.push_str(&text),
        DeepSeekDelta::Content(text) => output.content.push_str(&text),
    }
}

fn message_start(model: &str, input_tokens: u32) -> Bytes {
    sse_event(
        "message_start",
        &json!({
            "type":"message_start",
            "message":{
                "id":next_message_id(),"type":"message","role":"assistant","content":[],
                "model":model,"stop_reason":null,"stop_sequence":null,
                "usage":{"input_tokens":input_tokens,"output_tokens":0}
            }
        }),
    )
}

fn text_block_start(index: usize) -> Bytes {
    sse_event(
        "content_block_start",
        &json!({
            "type":"content_block_start","index":index,
            "content_block":{"type":"text","text":""}
        }),
    )
}

fn text_delta(index: usize, text: &str) -> Bytes {
    sse_event(
        "content_block_delta",
        &json!({
            "type":"content_block_delta","index":index,
            "delta":{"type":"text_delta","text":text}
        }),
    )
}

fn content_block_stop(index: usize) -> Bytes {
    sse_event(
        "content_block_stop",
        &json!({"type":"content_block_stop","index":index}),
    )
}

fn terminal_events(stop_reason: &str, output_tokens: u32) -> [Bytes; 2] {
    [
        sse_event(
            "message_delta",
            &json!({
                "type":"message_delta","delta":{"stop_reason":stop_reason,"stop_sequence":null},
                "usage":{"output_tokens":output_tokens}
            }),
        ),
        sse_event("message_stop", &json!({"type":"message_stop"})),
    ]
}

fn sse_event(event: &str, data: &Value) -> Bytes {
    Bytes::from(format!("event: {event}\ndata: {data}\n\n"))
}

fn next_message_id() -> String {
    let counter = MESSAGE_ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("msg_deepseek_{counter}")
}

fn next_tool_call_id() -> String {
    let counter = TOOL_CALL_ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("toolu_deepseek_{counter}")
}

fn random_nonce() -> String {
    let mut bytes = [0u8; 24];
    rand::thread_rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

enum ContentItem {
    Text(String),
    Thinking(String),
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    ToolResult {
        id: String,
        content: String,
    },
}

fn content_items(content: &Value) -> Vec<ContentItem> {
    match content {
        Value::String(text) => vec![ContentItem::Text(text.clone())],
        Value::Array(items) => items
            .iter()
            .filter_map(|item| match item.get("type").and_then(Value::as_str) {
                Some("text") => item
                    .get("text")
                    .and_then(Value::as_str)
                    .map(|text| ContentItem::Text(text.to_string())),
                Some("thinking") => item
                    .get("thinking")
                    .and_then(Value::as_str)
                    .map(|text| ContentItem::Thinking(text.to_string())),
                Some("tool_use") => Some(ContentItem::ToolUse {
                    id: item.get("id")?.as_str()?.to_string(),
                    name: item.get("name")?.as_str()?.to_string(),
                    input: item.get("input").filter(|input| input.is_object())?.clone(),
                }),
                Some("tool_result") => Some(ContentItem::ToolResult {
                    id: item.get("tool_use_id")?.as_str()?.to_string(),
                    content: text_from_content(item.get("content").unwrap_or(&Value::Null)),
                }),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn text_from_content(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(items) => items
            .iter()
            .filter_map(|item| match item.get("type").and_then(Value::as_str) {
                Some("text") => item.get("text").and_then(Value::as_str),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn values(value: &Value) -> Vec<&Value> {
    match value {
        Value::Array(values) => values.iter().collect(),
        Value::Object(_) => vec![value],
        _ => Vec::new(),
    }
}

fn string_payload(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Object(object) => object
            .get("content")
            .or_else(|| object.get("text"))
            .and_then(Value::as_str)
            .map(str::to_string),
        _ => None,
    }
}

fn should_ignore_path(path: &str) -> bool {
    path.contains("quasi_status")
        || path.contains("elapsed_secs")
        || path.contains("pending_fragment")
        || path.contains("conversation_mode")
        || (path.starts_with("response/fragments/") && path.ends_with("/status"))
}

fn bounded_string(value: Option<&Value>, max_chars: usize) -> String {
    value
        .and_then(Value::as_str)
        .unwrap_or("")
        .chars()
        .take(max_chars)
        .collect()
}

fn bounded_https_url(value: Option<&Value>) -> String {
    let Some(value) = value.and_then(Value::as_str).map(str::trim) else {
        return String::new();
    };
    if value.len() > 8 * 1024 {
        return String::new();
    }
    url::Url::parse(value)
        .ok()
        .filter(|url| url.scheme() == "https" && url.host_str().is_some())
        .map(|url| url.to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::stream;

    fn parse_claude_sse(text: &str) -> Vec<(String, Value)> {
        text.split("\n\n")
            .filter_map(|frame| {
                let mut event = None;
                let mut data = None;
                for line in frame.lines() {
                    if let Some(value) = line.strip_prefix("event: ") {
                        event = Some(value.to_string());
                    } else if let Some(value) = line.strip_prefix("data: ") {
                        data = serde_json::from_str(value).ok();
                    }
                }
                event.zip(data)
            })
            .collect()
    }

    #[test]
    fn model_resolution_is_reviewed_and_fail_closed() {
        assert_eq!(resolve_model("claude-opus-4-7").unwrap(), "deepseek-v4-pro");
        assert_eq!(
            resolve_model("claude-sonnet-4-7").unwrap(),
            "deepseek-v4-flash"
        );
        assert!(resolve_model("deepseek-future-unreviewed").is_err());
    }

    #[test]
    fn prompt_replays_tool_results_and_binds_tool_nonce() {
        let body = json!({
            "thinking":{"type":"enabled"},
            "tools":[{"name":"read_file","description":"read","input_schema":{"type":"object","properties":{"path":{"type":"string"}}}}],
            "messages":[
                {"role":"user","content":"inspect"},
                {"role":"assistant","content":[{"type":"tool_use","id":"call-a","name":"read_file","input":{"path":"a"}}]},
                {"role":"user","content":[{"type":"tool_result","tool_use_id":"call-a","content":"contents"}]}
            ]
        });
        let prepared = prepare_request(&body, "deepseek-v4-pro").unwrap();
        assert!(prepared.thinking_enabled);
        assert!(prepared.has_tools());
        assert!(prepared
            .prompt
            .contains("Tool result (read_file): contents"));
        assert!(prepared.prompt.contains("_nonce"));
    }

    #[tokio::test]
    async fn strict_stream_preserves_thinking_content_and_search_citations() {
        let fixture = concat!(
            "data: {\"p\":\"response/fragments\",\"v\":[{\"type\":\"THINK\",\"content\":\"why\"}]}\n",
            "data: {\"p\":\"response/fragments\",\"v\":[{\"type\":\"RESPONSE\",\"content\":\"answer\"}]}\n",
            "data: {\"p\":\"response/status\",\"v\":\"FINISHED\"}\n",
            "data: {\"p\":\"response/search_results\",\"v\":[{\"cite_index\":1,\"title\":\"Source\",\"url\":\"https://example.com/a\"}]}\n",
        );
        let result = collect_deepseek_response(
            stream::iter(vec![Ok::<_, reqwest::Error>(Bytes::from_static(
                fixture.as_bytes(),
            ))]),
            true,
        )
        .await
        .unwrap();
        assert_eq!(result.thinking, "why");
        assert!(result.content.starts_with("answer"));
        assert!(result
            .content
            .contains("[1]: [Source](https://example.com/a)"));
    }

    #[tokio::test]
    async fn strict_stream_rejects_malformed_truncated_duplicate_and_post_terminal_data() {
        for fixture in [
            "data: not-json\n",
            "data: {\"p\":\"response/content\",\"v\":\"partial\"}\n",
            concat!(
                "data: {\"p\":\"response/status\",\"v\":\"FINISHED\"}\n",
                "data: {\"p\":\"response/status\",\"v\":\"FINISHED\"}\n"
            ),
            concat!(
                "data: [DONE]\n",
                "data: {\"p\":\"response/content\",\"v\":\"late\"}\n"
            ),
        ] {
            let result = collect_deepseek_response(
                stream::iter(vec![Ok::<_, reqwest::Error>(Bytes::copy_from_slice(
                    fixture.as_bytes(),
                ))]),
                false,
            )
            .await;
            assert!(result.is_err(), "fixture unexpectedly passed: {fixture}");
        }
    }

    #[tokio::test]
    async fn claude_stream_closes_thinking_before_text_and_never_terminates_truncation() {
        let complete = concat!(
            "data: {\"p\":\"response/fragments\",\"v\":[{\"type\":\"THINK\",\"content\":\"why\"}]}\n",
            "data: {\"p\":\"response/fragments\",\"v\":[{\"type\":\"RESPONSE\",\"content\":\"answer\"}]}\n",
            "data: {\"p\":\"response/status\",\"v\":\"FINISHED\"}\n"
        );
        let output = deepseek_bytes_stream_to_claude_sse(
            stream::iter(vec![Ok::<_, reqwest::Error>(Bytes::from_static(
                complete.as_bytes(),
            ))]),
            "deepseek-v4-pro".to_string(),
            3,
            true,
        );
        tokio::pin!(output);
        let mut chunks = Vec::new();
        while let Some(chunk) = output.next().await {
            chunks.push(chunk.unwrap());
        }
        let text = String::from_utf8(chunks.concat()).unwrap();
        let events = parse_claude_sse(&text);
        let thinking_stop = events
            .iter()
            .position(|(event, data)| event == "content_block_stop" && data["index"] == 0)
            .unwrap();
        let text_start = events
            .iter()
            .position(|(event, data)| {
                event == "content_block_start"
                    && data["index"] == 1
                    && data["content_block"]["type"] == "text"
            })
            .unwrap();
        assert!(thinking_stop < text_start);
        assert!(events.iter().any(|(event, _)| event == "message_stop"));

        let truncated = "data: {\"p\":\"response/content\",\"v\":\"partial\"}\n";
        let output = deepseek_bytes_stream_to_claude_sse(
            stream::iter(vec![Ok::<_, reqwest::Error>(Bytes::from_static(
                truncated.as_bytes(),
            ))]),
            "deepseek-v4-flash".to_string(),
            1,
            false,
        );
        tokio::pin!(output);
        let mut saw_error = false;
        let mut merged = Vec::new();
        while let Some(chunk) = output.next().await {
            match chunk {
                Ok(chunk) => merged.extend_from_slice(&chunk),
                Err(_) => saw_error = true,
            }
        }
        assert!(saw_error);
        assert!(!String::from_utf8_lossy(&merged).contains("event: message_stop"));
    }

    #[test]
    fn canonical_tool_reply_requires_nonce_allowlist_and_object_arguments() {
        let contract = DeepSeekToolContract {
            nonce: "nonce-a".to_string(),
            allowed_names: BTreeSet::from(["read_file".to_string()]),
        };
        let (content, calls) = parse_tool_calls(
            "before<tool>{\"name\":\"read_file\",\"arguments\":{\"path\":\"a\"},\"_nonce\":\"nonce-a\"}</tool>after",
            &contract,
        )
        .unwrap();
        assert_eq!(content, "beforeafter");
        assert_eq!(calls[0].name, "read_file");
        assert!(parse_tool_calls(
            "<tool>{\"name\":\"read_file\",\"arguments\":{},\"_nonce\":\"wrong\"}</tool>",
            &contract
        )
        .is_err());
    }
}
