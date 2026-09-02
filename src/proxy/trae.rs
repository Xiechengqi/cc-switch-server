use std::collections::BTreeMap;

use bytes::Bytes;
use rand::RngCore;
use serde_json::{json, Map, Value};

use super::trae_runtime::TraeModelCapability;
use super::ProxyError;
use crate::domain::trae::TRAE_FUNCTION;

const MAX_SSE_EVENT_BYTES: usize = 2 * 1024 * 1024;
const MAX_ERROR_MESSAGE_BYTES: usize = 2_048;
const MAX_TOOL_CALLS: usize = 128;
const MAX_TEXT_BYTES: usize = 32 * 1024 * 1024;
const MAX_AGGREGATE_BYTES: usize = 64 * 1024 * 1024;

pub fn random_trae_request_id() -> String {
    let mut bytes = [0_u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    format!(
        "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
        u32::from_be_bytes(bytes[0..4].try_into().expect("fixed UUID slice")),
        u16::from_be_bytes(bytes[4..6].try_into().expect("fixed UUID slice")),
        u16::from_be_bytes(bytes[6..8].try_into().expect("fixed UUID slice")),
        u16::from_be_bytes(bytes[8..10].try_into().expect("fixed UUID slice")),
        u64::from_be_bytes([
            0, 0, bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15]
        ])
    )
}

/// Rewrites the repository's canonical OpenAI Chat request into Trae Solo's
/// fixed `llm_utils_chat` contract. The live account catalog is authoritative
/// for the exact mixed-case model id and its reasoning/max-mode capabilities.
pub fn build_trae_payload(
    canonical: &Value,
    model_id: &str,
    capability: &TraeModelCapability,
) -> Result<Value, String> {
    let model_id = model_id.trim();
    if model_id.is_empty() || model_id != capability.id {
        return Err("Trae payload model must match the live catalog capability".to_string());
    }
    let mut payload = canonical
        .as_object()
        .cloned()
        .ok_or_else(|| "Trae canonical Chat request must be an object".to_string())?;

    reject_images(&payload)?;
    normalize_messages(&mut payload)?;
    normalize_tool_choice(&mut payload)?;
    normalize_tools(&mut payload)?;
    apply_reasoning_fields(&mut payload, capability)?;

    payload.remove("stream_options");
    payload.insert("model".to_string(), Value::String(model_id.to_string()));
    payload.insert(
        "config_name".to_string(),
        Value::String(model_id.to_string()),
    );
    payload.insert(
        "function".to_string(),
        Value::String(TRAE_FUNCTION.to_string()),
    );
    payload.insert("stream".to_string(), Value::Bool(true));
    Ok(Value::Object(payload))
}

fn reject_images(payload: &Map<String, Value>) -> Result<(), String> {
    let Some(messages) = payload.get("messages").and_then(Value::as_array) else {
        return Ok(());
    };
    for (message_index, message) in messages.iter().enumerate() {
        let Some(content) = message.get("content") else {
            continue;
        };
        let Value::Array(parts) = content else {
            continue;
        };
        for (part_index, part) in parts.iter().enumerate() {
            let Some(part) = part.as_object() else {
                continue;
            };
            let part_type = part
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase();
            if part_type.contains("image")
                || part.contains_key("image_url")
                || part.contains_key("inline_data")
                || part.contains_key("inlineData")
            {
                return Err(format!(
                    "Trae Solo does not support image input (message {message_index}, part {part_index})"
                ));
            }
        }
    }
    Ok(())
}

fn normalize_messages(payload: &mut Map<String, Value>) -> Result<(), String> {
    let messages = payload
        .get_mut("messages")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| "Trae canonical Chat request must contain messages array".to_string())?;
    for (index, message) in messages.iter_mut().enumerate() {
        let message = message
            .as_object_mut()
            .ok_or_else(|| format!("Trae message {index} must be an object"))?;
        let assistant = message
            .get("role")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| format!("Trae message {index} is missing role"))?
            .eq_ignore_ascii_case("assistant");

        if let Some(content) = message.get_mut("content") {
            match content {
                Value::String(text) => {
                    *content = json!([{"type": "text", "text": text}]);
                }
                Value::Array(parts) => {
                    for (part_index, part) in parts.iter().enumerate() {
                        let part = part.as_object().ok_or_else(|| {
                            format!(
                                "Trae message {index} content part {part_index} must be an object"
                            )
                        })?;
                        if part.get("type").and_then(Value::as_str) != Some("text")
                            || !part.get("text").is_some_and(Value::is_string)
                        {
                            return Err(format!(
                                "Trae message {index} content part {part_index} must be text"
                            ));
                        }
                    }
                }
                Value::Null => {}
                _ => return Err(format!("Trae message {index} content has an invalid shape")),
            }
        }

        if assistant {
            normalize_assistant_tool_calls(message, index)?;
        }
    }
    Ok(())
}

fn normalize_assistant_tool_calls(
    message: &mut Map<String, Value>,
    message_index: usize,
) -> Result<(), String> {
    let Some(calls) = message.get_mut("tool_calls") else {
        return Ok(());
    };
    let calls = calls.as_array_mut().ok_or_else(|| {
        format!("Trae assistant message {message_index} tool_calls must be an array")
    })?;
    for (call_index, call) in calls.iter_mut().enumerate() {
        let call = call.as_object_mut().ok_or_else(|| {
            format!(
                "Trae assistant message {message_index} tool call {call_index} must be an object"
            )
        })?;
        if let Some(function) = call.remove("function") {
            if call.contains_key("function_call") {
                return Err(format!(
                    "Trae assistant message {message_index} tool call {call_index} has conflicting function fields"
                ));
            }
            call.insert("function_call".to_string(), function);
        }
        let function = call
            .get("function_call")
            .and_then(Value::as_object)
            .ok_or_else(|| {
                format!(
                    "Trae assistant message {message_index} tool call {call_index} is missing function"
                )
            })?;
        if function
            .get("name")
            .and_then(Value::as_str)
            .is_none_or(|name| name.trim().is_empty())
        {
            return Err(format!(
                "Trae assistant message {message_index} tool call {call_index} is missing function name"
            ));
        }
    }
    Ok(())
}

fn normalize_tool_choice(payload: &mut Map<String, Value>) -> Result<(), String> {
    let Some(choice) = payload.get("tool_choice").cloned() else {
        return Ok(());
    };
    let suppress = |payload: &mut Map<String, Value>| {
        payload.remove("tools");
        payload.remove("functions");
        payload.remove("tool_choice");
    };
    match choice {
        Value::String(value) => {
            let value = value.trim();
            match value.to_ascii_lowercase().as_str() {
                "none" => suppress(payload),
                reserved @ ("auto" | "required") => {
                    payload.insert(
                        "tool_choice".to_string(),
                        Value::String(reserved.to_string()),
                    );
                }
                _ if !value.is_empty() => {
                    // Function names are case-sensitive vendor identifiers.
                    // Only the three reserved control words are normalized.
                    payload.insert("tool_choice".to_string(), Value::String(value.to_string()));
                }
                _ => {
                    payload.remove("tool_choice");
                }
            }
        }
        Value::Object(choice) => {
            let kind = choice
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase();
            match kind.as_str() {
                "none" => suppress(payload),
                "auto" | "required" => {
                    payload.insert("tool_choice".to_string(), Value::String(kind));
                }
                "function" => {
                    let name = choice
                        .get("function")
                        .and_then(Value::as_object)
                        .and_then(|function| function.get("name"))
                        .or_else(|| choice.get("name"))
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .ok_or_else(|| {
                            "Trae function tool_choice is missing a function name".to_string()
                        })?;
                    payload.insert("tool_choice".to_string(), Value::String(name.to_string()));
                }
                _ => {
                    payload.remove("tool_choice");
                }
            }
        }
        Value::Null => {
            payload.remove("tool_choice");
        }
        _ => return Err("Trae tool_choice has an invalid shape".to_string()),
    }
    Ok(())
}

fn normalize_tools(payload: &mut Map<String, Value>) -> Result<(), String> {
    let Some(tools) = payload.get_mut("tools") else {
        return Ok(());
    };
    if tools.is_null() {
        payload.remove("tools");
        return Ok(());
    }
    let tools = tools
        .as_array_mut()
        .ok_or_else(|| "Trae tools must be an array".to_string())?;
    if tools.is_empty() {
        payload.remove("tools");
        payload.remove("tool_choice");
        return Ok(());
    }
    if tools.len() > MAX_TOOL_CALLS {
        return Err(format!("Trae tools exceed the {MAX_TOOL_CALLS} item limit"));
    }
    for (index, tool) in tools.iter_mut().enumerate() {
        let tool = tool
            .as_object_mut()
            .ok_or_else(|| format!("Trae tool {index} must be an object"))?;
        if tool.get("type").and_then(Value::as_str) != Some("function") {
            return Err(format!("Trae tool {index} must have type function"));
        }
        let function = tool
            .get_mut("function")
            .and_then(Value::as_object_mut)
            .ok_or_else(|| format!("Trae tool {index} is missing function"))?;
        if function
            .get("name")
            .and_then(Value::as_str)
            .is_none_or(|name| name.trim().is_empty())
        {
            return Err(format!("Trae tool {index} is missing function name"));
        }
        if let Some(parameters) = function.get_mut("parameters") {
            let encoded = if let Value::String(value) = parameters {
                serde_json::from_str::<Value>(value).map_err(|_| {
                    format!("Trae tool {index} parameters string is not valid JSON")
                })?;
                value.clone()
            } else {
                serde_json::to_string(&*parameters)
                    .map_err(|error| format!("encode Trae tool {index} parameters: {error}"))?
            };
            *parameters = Value::String(encoded);
        }
    }
    Ok(())
}

fn apply_reasoning_fields(
    payload: &mut Map<String, Value>,
    capability: &TraeModelCapability,
) -> Result<(), String> {
    let requested_level = requested_reasoning_level(payload)?;
    let max_requested = requested_max_mode(payload)?;
    for key in [
        "enable_thinking",
        "enable_reasoning",
        "is_reasoning",
        "reasoning_effort",
        "reasoning_budget_tokens",
        "thinking",
        "context_length",
        "max_input_tokens",
        "reasoning_effort_level",
        "is_max_mode",
    ] {
        payload.remove(key);
    }

    if max_requested && capability.max_mode {
        payload.insert("is_max_mode".to_string(), Value::from(1));
    }
    let Some(level) = requested_level else {
        return Ok(());
    };
    if level == "none" {
        return Ok(());
    }
    if !capability.supports_reasoning || capability.reasoning_efforts.is_empty() {
        return Err(format!(
            "Trae model {} does not advertise reasoning support",
            capability.id
        ));
    }
    let selected = capability
        .reasoning_efforts
        .iter()
        .find(|candidate| candidate.as_str() == level)
        .cloned()
        .or_else(|| capability.reasoning_default.clone())
        .or_else(|| capability.reasoning_efforts.first().cloned())
        .ok_or_else(|| {
            format!(
                "Trae model {} has an incomplete reasoning capability",
                capability.id
            )
        })?;
    payload.insert(
        "reasoning_effort_level".to_string(),
        Value::String(selected),
    );
    Ok(())
}

fn requested_reasoning_level(payload: &Map<String, Value>) -> Result<Option<String>, String> {
    if let Some(value) = payload.get("reasoning_effort") {
        let raw = match value {
            Value::String(value) => Some(value.as_str()),
            Value::Object(value) => ["effort", "level", "type"]
                .iter()
                .find_map(|key| value.get(*key).and_then(Value::as_str)),
            Value::Null => None,
            _ => return Err("Trae reasoning_effort has an invalid shape".to_string()),
        };
        if let Some(raw) = raw {
            return normalize_reasoning_level(raw)
                .map(Some)
                .ok_or_else(|| format!("unsupported Trae reasoning effort {raw:?}"));
        }
    }
    for key in ["enable_thinking", "enable_reasoning", "is_reasoning"] {
        if let Some(value) = payload.get(key) {
            let enabled = value
                .as_bool()
                .ok_or_else(|| format!("Trae {key} must be a boolean"))?;
            return Ok(Some(if enabled { "medium" } else { "none" }.to_string()));
        }
    }
    Ok(None)
}

fn requested_max_mode(payload: &Map<String, Value>) -> Result<bool, String> {
    let Some(value) = payload.get("is_max_mode") else {
        return Ok(false);
    };
    match value {
        Value::Bool(value) => Ok(*value),
        Value::Number(value) if value.as_i64() == Some(0) => Ok(false),
        Value::Number(value) if value.as_i64() == Some(1) => Ok(true),
        Value::Null => Ok(false),
        _ => Err("Trae is_max_mode must be boolean or 0/1".to_string()),
    }
}

fn normalize_reasoning_level(value: &str) -> Option<String> {
    let key = value.trim().to_ascii_lowercase().replace([' ', '_'], "-");
    let normalized = match key.as_str() {
        "none" | "off" | "disabled" => "none",
        "minimal" | "min" | "light" | "low" => "low",
        "medium" | "med" | "default" => "medium",
        "high" => "high",
        "xhigh" | "x-high" | "extra-high" | "extrahigh" | "max" => "xhigh",
        _ => return None,
    };
    Some(normalized.to_string())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraeUpstreamError {
    pub status: u16,
    pub code: Option<String>,
    pub message: String,
}

impl TraeUpstreamError {
    pub fn from_status_body(status: u16, body: &[u8]) -> Self {
        let value = serde_json::from_slice::<Value>(body).unwrap_or(Value::Null);
        let code = recursive_business_code(&value, 0);
        let message = extract_error_message(&value).unwrap_or_else(|| {
            let text = String::from_utf8_lossy(body);
            let lower = text.to_ascii_lowercase();
            if lower.contains("<html") || lower.contains("<!doctype") {
                format!("Trae upstream returned HTTP {status} with an HTML error page")
            } else {
                format!("Trae upstream returned HTTP {status}")
            }
        });
        Self {
            status,
            code,
            message,
        }
    }

    fn from_event(value: &Value) -> Self {
        let code = recursive_business_code(value, 0);
        let message = extract_error_message(value)
            .unwrap_or_else(|| "Trae stream returned an upstream error".to_string());
        let status = code
            .as_deref()
            .map(status_for_code)
            .unwrap_or(axum::http::StatusCode::BAD_GATEWAY)
            .as_u16();
        Self {
            status,
            code,
            message,
        }
    }

    pub fn downstream_status(&self) -> axum::http::StatusCode {
        self.code
            .as_deref()
            .map(status_for_code)
            .unwrap_or_else(|| {
                axum::http::StatusCode::from_u16(self.status)
                    .unwrap_or(axum::http::StatusCode::BAD_GATEWAY)
            })
    }

    pub fn is_authentication_failure(&self) -> bool {
        self.status == 401 || self.code.as_deref() == Some("1001")
    }

    pub fn into_proxy_error(self) -> ProxyError {
        ProxyError {
            status: self.downstream_status(),
            message: match self.code {
                Some(code) => format!("Trae upstream error {code}: {}", self.message),
                None => self.message,
            },
        }
    }
}

fn status_for_code(code: &str) -> axum::http::StatusCode {
    match code.trim() {
        "1001" => axum::http::StatusCode::UNAUTHORIZED,
        "1005" | "4008" | "4011" => axum::http::StatusCode::TOO_MANY_REQUESTS,
        "4001" => axum::http::StatusCode::BAD_REQUEST,
        _ => axum::http::StatusCode::BAD_GATEWAY,
    }
}

#[derive(Debug)]
pub enum TraeSseDecodeError {
    Upstream(TraeUpstreamError),
    Protocol(ProxyError),
}

impl TraeSseDecodeError {
    pub fn into_proxy_error(self) -> ProxyError {
        match self {
            Self::Upstream(error) => error.into_proxy_error(),
            Self::Protocol(error) => error,
        }
    }
}

impl From<ProxyError> for TraeSseDecodeError {
    fn from(error: ProxyError) -> Self {
        Self::Protocol(error)
    }
}

#[derive(Debug)]
pub struct TraeSseDecoder {
    buffer: Vec<u8>,
    aggregate_bytes: usize,
    id: String,
    created: i64,
    fallback_model: String,
    reported_model: Option<String>,
    content: String,
    reasoning_content: String,
    tool_calls: BTreeMap<usize, AggregatedToolCall>,
    finish_reason: String,
    usage: Option<Value>,
    saw_done: bool,
    complete: bool,
}

#[derive(Debug, Default)]
struct AggregatedToolCall {
    id: String,
    call_type: String,
    name: String,
    arguments: String,
}

impl TraeSseDecoder {
    pub fn new(fallback_model: impl Into<String>) -> Self {
        Self {
            buffer: Vec::new(),
            aggregate_bytes: 0,
            id: format!("chatcmpl-trae-{}", random_trae_request_id()),
            created: chrono::Utc::now().timestamp(),
            fallback_model: fallback_model.into(),
            reported_model: None,
            content: String::new(),
            reasoning_content: String::new(),
            tool_calls: BTreeMap::new(),
            finish_reason: "stop".to_string(),
            usage: None,
            saw_done: false,
            complete: false,
        }
    }

    /// True only after upstream EOF validated the single previously observed
    /// `done`. Merely receiving `done` does not commit a successful terminal.
    pub fn is_terminal(&self) -> bool {
        self.complete
    }

    pub fn push_classified(&mut self, chunk: Bytes) -> Result<Bytes, TraeSseDecodeError> {
        if self.complete && !chunk.is_empty() {
            return Err(ProxyError::bad_gateway("Trae SSE emitted bytes after completion").into());
        }
        if chunk.is_empty() {
            return Ok(Bytes::new());
        }
        self.aggregate_bytes = self.aggregate_bytes.saturating_add(chunk.len());
        if self.aggregate_bytes > MAX_AGGREGATE_BYTES {
            return Err(ProxyError::bad_gateway("Trae SSE response exceeds the limit").into());
        }
        self.buffer.extend_from_slice(&chunk);
        self.drain(false)
    }

    pub fn finish_classified(&mut self) -> Result<Bytes, TraeSseDecodeError> {
        if self.complete {
            return Err(ProxyError::bad_gateway("Trae SSE was finalized more than once").into());
        }
        let mut output = self.drain(true)?.to_vec();
        if !self.saw_done {
            return Err(
                ProxyError::bad_gateway("Trae SSE ended without exactly one done event").into(),
            );
        }
        self.validate_tool_calls()?;
        self.complete = true;
        append_sse_json(&mut output, &self.terminal_chunk())?;
        output.extend_from_slice(b"data: [DONE]\n\n");
        Ok(Bytes::from(output))
    }

    pub fn into_chat_completion(self) -> Result<Value, ProxyError> {
        if !self.complete {
            return Err(ProxyError::bad_gateway(
                "Trae Chat response was aggregated before a verified done event",
            ));
        }
        let final_tool_calls = self.final_tool_calls();
        let model = self.model().to_string();
        let mut message = Map::new();
        message.insert("role".to_string(), Value::String("assistant".to_string()));
        message.insert(
            "content".to_string(),
            if self.content.is_empty() && !self.tool_calls.is_empty() {
                Value::Null
            } else {
                Value::String(self.content)
            },
        );
        if !self.reasoning_content.is_empty() {
            message.insert(
                "reasoning_content".to_string(),
                Value::String(self.reasoning_content),
            );
        }
        if !final_tool_calls.is_empty() {
            message.insert("tool_calls".to_string(), Value::Array(final_tool_calls));
        }
        let mut response = json!({
            "id": self.id,
            "object": "chat.completion",
            "created": self.created,
            "model": model,
            "choices": [{
                "index": 0,
                "message": Value::Object(message),
                "finish_reason": self.finish_reason,
            }]
        });
        if let Some(usage) = self.usage {
            response["usage"] = usage;
        }
        Ok(response)
    }

    fn drain(&mut self, finish: bool) -> Result<Bytes, TraeSseDecodeError> {
        let mut output = Vec::new();
        while let Some((end, delimiter)) = next_event_boundary(&self.buffer) {
            if end > MAX_SSE_EVENT_BYTES {
                return Err(ProxyError::bad_gateway("Trae SSE event exceeds the limit").into());
            }
            let event = self.buffer[..end].to_vec();
            self.buffer.drain(..end + delimiter);
            self.decode_event(&event, &mut output)?;
        }
        if self.buffer.len() > MAX_SSE_EVENT_BYTES {
            return Err(ProxyError::bad_gateway("Trae SSE event exceeds the limit").into());
        }
        if finish && !self.buffer.is_empty() {
            let event = std::mem::take(&mut self.buffer);
            self.decode_event(&event, &mut output)?;
        }
        Ok(Bytes::from(output))
    }

    fn decode_event(
        &mut self,
        event: &[u8],
        output: &mut Vec<u8>,
    ) -> Result<(), TraeSseDecodeError> {
        let text = std::str::from_utf8(event)
            .map_err(|_| ProxyError::bad_gateway("Trae SSE event is not UTF-8"))?;
        let mut name = None;
        let mut data = None;
        for line in text.lines() {
            let line = line.trim_end_matches('\r');
            if line.is_empty() || line.starts_with(':') {
                continue;
            }
            if let Some(value) = line.strip_prefix("event:") {
                if name.is_some() {
                    return Err(ProxyError::bad_gateway(
                        "Trae SSE event contains multiple event fields",
                    )
                    .into());
                }
                name = Some(value.trim());
            } else if let Some(value) = line.strip_prefix("data:") {
                if data.is_some() {
                    return Err(ProxyError::bad_gateway(
                        "Trae SSE event contains multiple data fields",
                    )
                    .into());
                }
                data = Some(value.trim_start());
            } else {
                return Err(ProxyError::bad_gateway(
                    "Trae SSE event contains an unsupported field",
                )
                .into());
            }
        }
        let Some(name) = name.filter(|name| !name.is_empty()) else {
            if data.is_none() {
                return Ok(());
            }
            return Err(ProxyError::bad_gateway("Trae SSE data is missing an event name").into());
        };
        let data = data.ok_or_else(|| {
            ProxyError::bad_gateway(format!("Trae SSE {name} event is missing data"))
        })?;
        if self.saw_done {
            return Err(ProxyError::bad_gateway(if name == "done" {
                "Trae SSE emitted duplicate done events"
            } else {
                "Trae SSE emitted an event after done"
            })
            .into());
        }
        let value = serde_json::from_str::<Value>(data).map_err(|error| {
            ProxyError::bad_gateway(format!("invalid Trae SSE {name} event: {error}"))
        })?;
        match name {
            "metadata" => self.consume_metadata(&value),
            "output" => self.consume_output(&value, output),
            "token_usage" => self.consume_usage(&value),
            "done" => self.consume_done(&value, output),
            "error" => Err(TraeSseDecodeError::Upstream(TraeUpstreamError::from_event(
                &value,
            ))),
            _ => Err(ProxyError::bad_gateway(format!(
                "Trae SSE emitted unsupported event {name:?}"
            ))
            .into()),
        }
    }

    fn consume_metadata(&mut self, value: &Value) -> Result<(), TraeSseDecodeError> {
        let object = value
            .as_object()
            .ok_or_else(|| ProxyError::bad_gateway("Trae metadata must be an object"))?;
        if let Some(model) = object.get("model") {
            let model = model
                .as_str()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| ProxyError::bad_gateway("Trae metadata model must be a string"))?;
            if self
                .reported_model
                .as_deref()
                .is_some_and(|current| current != model)
            {
                return Err(ProxyError::bad_gateway(
                    "Trae metadata changed model during the stream",
                )
                .into());
            }
            self.reported_model = Some(model.to_string());
        }
        Ok(())
    }

    fn consume_output(
        &mut self,
        value: &Value,
        output: &mut Vec<u8>,
    ) -> Result<(), TraeSseDecodeError> {
        let object = value
            .as_object()
            .ok_or_else(|| ProxyError::bad_gateway("Trae output must be an object"))?;
        let mut delta = Map::new();
        if let Some(response) = object.get("response") {
            let response = response
                .as_str()
                .ok_or_else(|| ProxyError::bad_gateway("Trae output response must be a string"))?;
            self.content.push_str(response);
            ensure_text_limit(&self.content, "content")?;
            if !response.is_empty() {
                delta.insert("content".to_string(), Value::String(response.to_string()));
            }
        }
        if let Some(reasoning) = object.get("reasoning_content") {
            let reasoning = reasoning.as_str().ok_or_else(|| {
                ProxyError::bad_gateway("Trae output reasoning_content must be a string")
            })?;
            self.reasoning_content.push_str(reasoning);
            ensure_text_limit(&self.reasoning_content, "reasoning_content")?;
            if !reasoning.is_empty() {
                delta.insert(
                    "reasoning_content".to_string(),
                    Value::String(reasoning.to_string()),
                );
            }
        }
        if let Some(reason) = object.get("finish_reason").filter(|value| !value.is_null()) {
            self.finish_reason =
                required_nonempty_string(reason, "Trae finish_reason")?.to_string();
        }
        if let Some(calls) = object.get("tool_calls").filter(|value| !value.is_null()) {
            let canonical = self.merge_tool_calls(calls)?;
            if !canonical.is_empty() {
                delta.insert("tool_calls".to_string(), Value::Array(canonical));
            }
        }
        if !delta.is_empty() {
            append_sse_json(output, &self.chunk(Value::Object(delta), None))?;
        }
        Ok(())
    }

    fn consume_usage(&mut self, value: &Value) -> Result<(), TraeSseDecodeError> {
        if !value.is_object() {
            return Err(ProxyError::bad_gateway("Trae token_usage must be an object").into());
        }
        self.usage = Some(value.clone());
        Ok(())
    }

    fn consume_done(
        &mut self,
        value: &Value,
        output: &mut Vec<u8>,
    ) -> Result<(), TraeSseDecodeError> {
        let object = value
            .as_object()
            .ok_or_else(|| ProxyError::bad_gateway("Trae done must be an object"))?;
        if let Some(reason) = object.get("finish_reason").filter(|value| !value.is_null()) {
            self.finish_reason =
                required_nonempty_string(reason, "Trae done finish_reason")?.to_string();
        }
        let mut empty_argument_deltas = Vec::new();
        for (index, call) in &mut self.tool_calls {
            if call.arguments.is_empty() {
                call.arguments = "{}".to_string();
                empty_argument_deltas.push(json!({
                    "index": index,
                    "function": {"arguments": "{}"},
                }));
            }
        }
        if !empty_argument_deltas.is_empty() {
            append_sse_json(
                output,
                &self.chunk(json!({"tool_calls": empty_argument_deltas}), None),
            )?;
        }
        self.saw_done = true;
        Ok(())
    }

    fn merge_tool_calls(&mut self, value: &Value) -> Result<Vec<Value>, TraeSseDecodeError> {
        let items = match value {
            Value::Array(items) => items.clone(),
            Value::Object(_) => vec![value.clone()],
            _ => {
                return Err(ProxyError::bad_gateway(
                    "Trae output tool_calls must be an object or array",
                )
                .into())
            }
        };
        let mut canonical = Vec::with_capacity(items.len());
        for (position, item) in items.into_iter().enumerate() {
            let mut item = item
                .as_object()
                .cloned()
                .ok_or_else(|| ProxyError::bad_gateway("Trae tool call must be an object"))?;
            let index = item
                .get("index")
                .and_then(integer_value)
                .and_then(|value| usize::try_from(value).ok())
                .unwrap_or(position);
            if index >= MAX_TOOL_CALLS {
                return Err(
                    ProxyError::bad_gateway("Trae tool call index exceeds the limit").into(),
                );
            }
            item.insert("index".to_string(), Value::from(index));
            if !item.contains_key("type") {
                item.insert("type".to_string(), Value::String("function".to_string()));
            }
            let function = item
                .remove("function_call")
                .or_else(|| item.remove("function"))
                .ok_or_else(|| ProxyError::bad_gateway("Trae tool call is missing function"))?;
            let mut function = function.as_object().cloned().ok_or_else(|| {
                ProxyError::bad_gateway("Trae tool call function must be an object")
            })?;
            function.remove("namespace");
            function.remove("partial_arguments");
            if let Some(arguments) = function.get_mut("arguments") {
                if !arguments.is_string() {
                    *arguments =
                        Value::String(serde_json::to_string(arguments).map_err(|error| {
                            ProxyError::bad_gateway(format!(
                                "encode Trae tool call arguments: {error}"
                            ))
                        })?);
                }
            }

            let target = self.tool_calls.entry(index).or_default();
            merge_stable_string(&mut target.id, item.get("id"), "Trae tool call id")?;
            merge_stable_string(
                &mut target.call_type,
                item.get("type"),
                "Trae tool call type",
            )?;
            merge_stable_string(
                &mut target.name,
                function.get("name"),
                "Trae tool function name",
            )?;
            append_string_fragment(
                &mut target.arguments,
                function.get("arguments"),
                "Trae tool function arguments",
            )?;
            item.insert("function".to_string(), Value::Object(function));
            canonical.push(Value::Object(item));
        }
        Ok(canonical)
    }

    fn validate_tool_calls(&self) -> Result<(), TraeSseDecodeError> {
        for (index, call) in &self.tool_calls {
            if call.id.trim().is_empty() || call.name.trim().is_empty() {
                return Err(ProxyError::bad_gateway(format!(
                    "Trae tool call {index} is missing id or function name"
                ))
                .into());
            }
            if serde_json::from_str::<Value>(&call.arguments).is_err() {
                return Err(ProxyError::bad_gateway(format!(
                    "Trae tool call {index} has invalid JSON arguments"
                ))
                .into());
            }
        }
        Ok(())
    }

    fn final_tool_calls(&self) -> Vec<Value> {
        self.tool_calls
            .values()
            .map(|call| {
                json!({
                    "id": call.id,
                    "type": if call.call_type.is_empty() { "function" } else { call.call_type.as_str() },
                    "function": {
                        "name": call.name,
                        "arguments": call.arguments,
                    }
                })
            })
            .collect()
    }

    fn model(&self) -> &str {
        self.reported_model
            .as_deref()
            .unwrap_or(self.fallback_model.as_str())
    }

    fn chunk(&self, delta: Value, finish_reason: Option<&str>) -> Value {
        json!({
            "id": self.id,
            "object": "chat.completion.chunk",
            "created": self.created,
            "model": self.model(),
            "choices": [{
                "index": 0,
                "delta": delta,
                "finish_reason": finish_reason,
            }]
        })
    }

    fn terminal_chunk(&self) -> Value {
        let mut chunk = self.chunk(json!({}), Some(&self.finish_reason));
        if let Some(usage) = self.usage.as_ref() {
            chunk["usage"] = usage.clone();
        }
        chunk
    }
}

fn append_sse_json(output: &mut Vec<u8>, value: &Value) -> Result<(), ProxyError> {
    let encoded = serde_json::to_vec(value)
        .map_err(|error| ProxyError::bad_gateway(format!("encode Trae Chat SSE: {error}")))?;
    output.extend_from_slice(b"data: ");
    output.extend_from_slice(&encoded);
    output.extend_from_slice(b"\n\n");
    Ok(())
}

fn merge_stable_string(
    target: &mut String,
    value: Option<&Value>,
    field: &str,
) -> Result<(), TraeSseDecodeError> {
    let Some(value) = value else {
        return Ok(());
    };
    let value = value
        .as_str()
        .ok_or_else(|| ProxyError::bad_gateway(format!("{field} must be a string")))?;
    if !target.is_empty() && target != value {
        return Err(ProxyError::bad_gateway(format!("{field} changed during the stream")).into());
    }
    target.clear();
    target.push_str(value);
    Ok(())
}

fn append_string_fragment(
    target: &mut String,
    value: Option<&Value>,
    field: &str,
) -> Result<(), TraeSseDecodeError> {
    let Some(value) = value else {
        return Ok(());
    };
    let value = value
        .as_str()
        .ok_or_else(|| ProxyError::bad_gateway(format!("{field} must be a string")))?;
    target.push_str(value);
    ensure_text_limit(target, field)?;
    Ok(())
}

fn ensure_text_limit(value: &str, field: &str) -> Result<(), TraeSseDecodeError> {
    if value.len() > MAX_TEXT_BYTES {
        return Err(ProxyError::bad_gateway(format!("Trae {field} exceeds the limit")).into());
    }
    Ok(())
}

fn required_nonempty_string<'a>(value: &'a Value, field: &str) -> Result<&'a str, ProxyError> {
    value
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ProxyError::bad_gateway(format!("{field} must be a non-empty string")))
}

fn recursive_business_code(value: &Value, depth: usize) -> Option<String> {
    if depth > 6 {
        return None;
    }
    if let Some(code) = value.get("code").and_then(json_scalar_string) {
        if !code.is_empty() && code != "0" {
            return Some(code);
        }
    }
    match value {
        Value::Object(object) => ["data", "error", "details", "message"]
            .iter()
            .filter_map(|key| object.get(*key))
            .find_map(|child| {
                recursive_business_code(child, depth + 1).or_else(|| {
                    child
                        .as_str()
                        .and_then(|text| serde_json::from_str::<Value>(text).ok())
                        .and_then(|parsed| recursive_business_code(&parsed, depth + 1))
                })
            }),
        Value::Array(values) => values
            .iter()
            .find_map(|value| recursive_business_code(value, depth + 1)),
        _ => None,
    }
}

fn json_scalar_string(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value.trim().to_string()),
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}

fn extract_error_message(value: &Value) -> Option<String> {
    ["/message", "/msg", "/error/message", "/data/message"]
        .iter()
        .find_map(|pointer| value.pointer(pointer).and_then(Value::as_str))
        .map(sanitize_error_message)
        .filter(|message| !message.is_empty())
}

fn sanitize_error_message(value: &str) -> String {
    let mut output = value
        .chars()
        .filter(|character| !character.is_control() || *character == ' ')
        .take(MAX_ERROR_MESSAGE_BYTES)
        .collect::<String>();
    if output.is_empty() {
        output = "Trae upstream error".to_string();
    }
    output
}

fn next_event_boundary(buffer: &[u8]) -> Option<(usize, usize)> {
    let lf = buffer.windows(2).position(|window| window == b"\n\n");
    let crlf = buffer.windows(4).position(|window| window == b"\r\n\r\n");
    match (lf, crlf) {
        (Some(lf), Some(crlf)) if lf <= crlf => Some((lf, 2)),
        (Some(_), Some(crlf)) => Some((crlf, 4)),
        (Some(lf), None) => Some((lf, 2)),
        (None, Some(crlf)) => Some((crlf, 4)),
        (None, None) => None,
    }
}

fn integer_value(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_u64().and_then(|value| i64::try_from(value).ok()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn capability() -> TraeModelCapability {
        TraeModelCapability {
            id: "DeepSeek-V4-Pro-Official".to_string(),
            display_name: "DeepSeek V4".to_string(),
            context_window: Some(200_000),
            context_window_max: Some(1_000_000),
            max_output_tokens: Some(16_384),
            prompt_max_tokens: None,
            supports_tools: true,
            supports_reasoning: true,
            reasoning_efforts: vec![
                "low".to_string(),
                "medium".to_string(),
                "high".to_string(),
                "xhigh".to_string(),
            ],
            reasoning_default: Some("medium".to_string()),
            reasoning_type: Some("enabled".to_string()),
            max_mode: true,
        }
    }

    #[test]
    fn payload_applies_solo_tools_reasoning_and_explicit_max_mode() {
        let payload = build_trae_payload(
            &json!({
                "model": "alias",
                "messages": [{
                    "role": "assistant",
                    "content": "calling",
                    "tool_calls": [{
                        "id": "call-1",
                        "type": "function",
                        "function": {"name": "weather", "arguments": "{}"}
                    }]
                }],
                "tools": [{
                    "type": "function",
                    "function": {
                        "name": "weather",
                        "parameters": {"type": "object", "properties": {}}
                    }
                }],
                "reasoning_effort": "extra_high",
                "is_max_mode": true,
                "stream": false,
                "stream_options": {"include_usage": true}
            }),
            "DeepSeek-V4-Pro-Official",
            &capability(),
        )
        .unwrap();
        assert_eq!(payload["function"], TRAE_FUNCTION);
        assert_eq!(payload["stream"], true);
        assert_eq!(payload["config_name"], "DeepSeek-V4-Pro-Official");
        assert_eq!(payload["reasoning_effort_level"], "xhigh");
        assert_eq!(payload["is_max_mode"], 1);
        assert!(payload.get("stream_options").is_none());
        assert!(payload
            .pointer("/messages/0/tool_calls/0/function")
            .is_none());
        assert_eq!(
            payload
                .pointer("/messages/0/tool_calls/0/function_call/name")
                .and_then(Value::as_str),
            Some("weather")
        );
        assert!(payload
            .pointer("/tools/0/function/parameters")
            .is_some_and(Value::is_string));
    }

    #[test]
    fn payload_preserves_case_sensitive_function_tool_choices() {
        for choice in [
            json!("CaseSensitiveTool"),
            json!({
                "type": "function",
                "function": {"name": "CaseSensitiveTool"}
            }),
        ] {
            let payload = build_trae_payload(
                &json!({
                    "messages": [{"role": "user", "content": "hello"}],
                    "tools": [{
                        "type": "function",
                        "function": {
                            "name": "CaseSensitiveTool",
                            "parameters": {"type": "object", "properties": {}}
                        }
                    }],
                    "tool_choice": choice,
                }),
                "DeepSeek-V4-Pro-Official",
                &capability(),
            )
            .unwrap();
            assert_eq!(payload["tool_choice"], "CaseSensitiveTool");
        }

        let reserved = build_trae_payload(
            &json!({
                "messages": [{"role": "user", "content": "hello"}],
                "tools": [{
                    "type": "function",
                    "function": {"name": "CaseSensitiveTool", "parameters": {}}
                }],
                "tool_choice": "AuTo",
            }),
            "DeepSeek-V4-Pro-Official",
            &capability(),
        )
        .unwrap();
        assert_eq!(reserved["tool_choice"], "auto");
    }

    #[test]
    fn payload_rejects_images_and_omits_unsupported_explicit_max_mode() {
        let error = build_trae_payload(
            &json!({
                "messages": [{"role": "user", "content": [{
                    "type": "image_url", "image_url": {"url": "data:image/png;base64,AA=="}
                }]}]
            }),
            "DeepSeek-V4-Pro-Official",
            &capability(),
        )
        .unwrap_err();
        assert!(error.contains("does not support image"));

        let mut caps = capability();
        caps.max_mode = false;
        let payload = build_trae_payload(
            &json!({
                "messages": [{"role": "user", "content": "hello"}],
                "is_max_mode": 1
            }),
            "DeepSeek-V4-Pro-Official",
            &caps,
        )
        .unwrap();
        assert!(payload.get("is_max_mode").is_none());
    }

    #[test]
    fn solo_done_is_not_emitted_until_eof_is_verified() {
        let mut decoder = TraeSseDecoder::new("glm-5.2");
        let first = decoder
            .push_classified(Bytes::from_static(
                b"event: metadata\ndata: {\"model\":\"glm-5.2\"}\n\nevent: output\ndata: {\"response\":\"hello\",\"reasoning_content\":\"think\"}\n\nevent: token_usage\ndata: {\"prompt_tokens\":2,\"completion_tokens\":1,\"total_tokens\":3}\n\nevent: done\ndata: {\"finish_reason\":\"stop\"}\n\n",
            ))
            .unwrap();
        let first_text = String::from_utf8(first.to_vec()).unwrap();
        assert!(first_text.contains("hello"));
        assert!(!first_text.contains("[DONE]"));
        assert!(!decoder.is_terminal());

        let tail = decoder.finish_classified().unwrap();
        assert!(String::from_utf8(tail.to_vec())
            .unwrap()
            .ends_with("data: [DONE]\n\n"));
        assert!(decoder.is_terminal());
        let response = decoder.into_chat_completion().unwrap();
        assert_eq!(
            response.pointer("/choices/0/message/content"),
            Some(&json!("hello"))
        );
        assert_eq!(
            response.pointer("/choices/0/message/reasoning_content"),
            Some(&json!("think"))
        );
        assert_eq!(response.pointer("/usage/total_tokens"), Some(&json!(3)));
    }

    #[test]
    fn solo_tool_call_without_arguments_emits_and_aggregates_empty_object() {
        let mut decoder = TraeSseDecoder::new("glm-5.2");
        let first = decoder
            .push_classified(Bytes::from_static(concat!(
                "event: output\n",
                "data: {\"tool_calls\":[{\"index\":0,\"id\":\"call-empty\",\"function_call\":{\"name\":\"NoArgs\"}}]}\n\n",
                "event: done\n",
                "data: {\"finish_reason\":\"tool_calls\"}\n\n"
            ).as_bytes()))
            .unwrap();
        let first = String::from_utf8(first.to_vec()).unwrap();
        assert!(first.contains("\"arguments\":\"{}\""));

        decoder.finish_classified().unwrap();
        let response = decoder.into_chat_completion().unwrap();
        assert_eq!(
            response.pointer("/choices/0/message/tool_calls/0/function/arguments"),
            Some(&json!("{}"))
        );
    }

    #[test]
    fn eof_duplicate_done_and_event_after_done_fail_closed() {
        let mut truncated = TraeSseDecoder::new("glm-5.2");
        truncated
            .push_classified(Bytes::from_static(
                b"event: output\ndata: {\"response\":\"partial\"}\n\n",
            ))
            .unwrap();
        assert!(truncated.finish_classified().is_err());

        for suffix in [
            "event: done\ndata: {}\n\n",
            "event: output\ndata: {\"response\":\"late\"}\n\n",
        ] {
            let mut decoder = TraeSseDecoder::new("glm-5.2");
            let wire = format!("event: done\ndata: {{}}\n\n{suffix}");
            assert!(decoder.push_classified(Bytes::from(wire)).is_err());
        }
    }

    #[test]
    fn solo_error_codes_are_classified() {
        let mut decoder = TraeSseDecoder::new("glm-5.2");
        let error = decoder
            .push_classified(Bytes::from_static(
                b"event: error\ndata: {\"code\":1001,\"message\":\"expired\"}\n\n",
            ))
            .unwrap_err();
        let TraeSseDecodeError::Upstream(error) = error else {
            panic!("expected classified upstream error");
        };
        assert!(error.is_authentication_failure());
        assert_eq!(
            error.downstream_status(),
            axum::http::StatusCode::UNAUTHORIZED
        );
        let forbidden = TraeUpstreamError::from_status_body(403, br#"{"message":"denied"}"#);
        assert!(!forbidden.is_authentication_failure());
    }
}
