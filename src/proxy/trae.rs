use std::collections::{BTreeMap, BTreeSet};

mod contract;
mod controls;
mod media;
mod raw;
pub(super) use contract::TraeToolContract;
pub(super) use controls::TraeSourceControls;
pub(super) use raw::build_raw_payload;

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
    normalize_generation_controls(&mut payload, capability)?;
    TraeToolContract::from_payload(&Value::Object(payload.clone()))?;
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
    normalize_messages_with_images(payload, false)
}
fn normalize_messages_with_images(
    payload: &mut Map<String, Value>,
    images: bool,
) -> Result<(), String> {
    let messages = payload
        .get_mut("messages")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| "Trae canonical Chat request must contain messages array".to_string())?;
    if messages.is_empty() {
        return Err("Trae messages must not be empty".to_string());
    }
    let mut pending = BTreeMap::<String, String>::new();
    let mut seen = BTreeSet::new();
    for (index, message) in messages.iter_mut().enumerate() {
        let message = message
            .as_object_mut()
            .ok_or_else(|| format!("Trae message {index} must be an object"))?;
        let role = message
            .get("role")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("Trae message {index} is missing role"))?;
        let role = match role {
            "developer" => "system",
            "system" => "system",
            "user" => "user",
            "assistant" => "assistant",
            "tool" => "tool",
            _ => return Err(format!("Trae message {index} has an unsupported role")),
        }
        .to_string();
        message.insert("role".to_string(), json!(role));
        if role != "tool" && !pending.is_empty() {
            return Err(format!(
                "Trae message {index} precedes outstanding tool results"
            ));
        }
        if let Some(content) = message.get_mut("content") {
            match content {
                Value::String(text) => *content = json!([{"type":"text","text":text}]),
                Value::Array(parts) => {
                    for (part_index, part) in parts.iter().enumerate() {
                        if images && part.get("type").and_then(Value::as_str) == Some("image_url") {
                            let url = part
                                .pointer("/image_url/url")
                                .and_then(Value::as_str)
                                .ok_or("Trae image_url requires a URL")?;
                            media::validate_image_url(url)?;
                            continue;
                        }
                        if part.get("type").and_then(Value::as_str) != Some("text")
                            || !part.get("text").is_some_and(Value::is_string)
                        {
                            return Err(format!(
                                "Trae message {index} content part {part_index} must be text"
                            ));
                        }
                    }
                }
                Value::Null => *content = json!([]),
                _ => return Err(format!("Trae message {index} content has an invalid shape")),
            }
        } else {
            message.insert("content".to_string(), json!([]));
        }
        if role == "assistant" {
            normalize_assistant_tool_calls(message, index)?;
            if let Some(calls) = message.get("tool_calls").and_then(Value::as_array) {
                for call in calls {
                    let id = exact_tool_id(call.get("id"))?;
                    if !seen.insert(id.to_string()) {
                        return Err("Trae history contains a duplicate tool call id".to_string());
                    }
                    let name = call
                        .pointer("/function_call/name")
                        .and_then(Value::as_str)
                        .expect("normalized function name");
                    pending.insert(id.to_string(), name.to_string());
                }
            }
        } else if role == "tool" {
            let id = exact_tool_id(message.get("tool_call_id"))?;
            let name = pending
                .remove(id)
                .ok_or_else(|| "Trae tool result has no matching outstanding call".to_string())?;
            if message
                .get("name")
                .and_then(Value::as_str)
                .is_some_and(|current| current != name)
            {
                return Err("Trae tool result name conflicts with its call".to_string());
            }
            message.insert("name".to_string(), json!(name));
        }
    }
    if !pending.is_empty() {
        return Err("Trae history is missing tool results".to_string());
    }
    Ok(())
}

fn exact_tool_id(value: Option<&Value>) -> Result<&str, String> {
    value
        .and_then(Value::as_str)
        .filter(|id| {
            !id.is_empty()
                && id.trim() == *id
                && id.len() <= 512
                && !id.chars().any(char::is_control)
        })
        .ok_or_else(|| "Trae tool call id must be a nonempty exact identifier".to_string())
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
    if calls.len() > MAX_TOOL_CALLS {
        return Err("Trae history has too many tool calls".to_string());
    }
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
        call.insert("index".to_string(), json!(call_index));
        call.insert("type".to_string(), json!("function"));
        let function = call
            .get_mut("function_call")
            .and_then(Value::as_object_mut)
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
        let arguments = function
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| json!("{}"));
        let parsed = if let Some(text) = arguments.as_str() {
            serde_json::from_str::<Value>(text)
                .map_err(|_| "Trae historical tool arguments are not valid JSON".to_string())?
        } else {
            arguments
        };
        if !parsed.is_object() {
            return Err("Trae historical tool arguments must be an object".to_string());
        }
        function.insert(
            "arguments".to_string(),
            json!(serde_json::to_string(&parsed).map_err(|error| error.to_string())?),
        );
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
                _ => return Err("Trae tool_choice has an unsupported type".to_string()),
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
        let parameters = function
            .get("parameters")
            .cloned()
            .unwrap_or_else(|| json!({"type":"object","properties":{}}));
        let parameters = if let Some(text) = parameters.as_str() {
            serde_json::from_str::<Value>(text)
                .map_err(|_| format!("Trae tool {index} parameters string is not valid JSON"))?
        } else {
            parameters
        };
        if !parameters.is_object()
            || parameters
                .get("type")
                .and_then(Value::as_str)
                .is_some_and(|kind| kind != "object")
        {
            return Err(format!(
                "Trae tool {index} parameters must describe an object"
            ));
        }
        function.insert(
            "parameters".to_string(),
            json!(serde_json::to_string(&parameters).map_err(|error| error.to_string())?),
        );
    }
    Ok(())
}

fn normalize_generation_controls(
    payload: &mut Map<String, Value>,
    capability: &TraeModelCapability,
) -> Result<(), String> {
    normalize_generation_controls_with_limit(payload, capability.max_output_tokens)
}

fn normalize_generation_controls_with_limit(
    payload: &mut Map<String, Value>,
    max_output_tokens: Option<u64>,
) -> Result<(), String> {
    let mut maximum = None;
    for key in ["max_tokens", "max_completion_tokens", "max_output_tokens"] {
        if let Some(value) = payload.remove(key).filter(|value| !value.is_null()) {
            let value = value
                .as_u64()
                .filter(|value| *value > 0)
                .ok_or_else(|| format!("Trae {key} must be a positive integer"))?;
            if maximum.is_some_and(|current| current != value) {
                return Err("Trae output token limits conflict".to_string());
            }
            maximum = Some(value);
        }
    }
    if let Some(maximum) = maximum {
        if max_output_tokens.is_some_and(|limit| maximum > limit) {
            return Err("Trae output token limit exceeds the model capability".to_string());
        }
        payload.insert("max_tokens".to_string(), json!(maximum));
    }
    for (key, low, high) in [("temperature", 0.0, 2.0), ("top_p", 0.0, 1.0)] {
        if let Some(value) = payload.get(key).filter(|value| !value.is_null()) {
            let number = value
                .as_f64()
                .ok_or_else(|| format!("Trae {key} must be a number"))?;
            if !number.is_finite() || number < low || number > high {
                return Err(format!("Trae {key} is out of range"));
            }
        }
    }
    if let Some(stop) = payload.get_mut("stop").filter(|value| !value.is_null()) {
        if let Value::String(text) = stop {
            *stop = json!([text]);
        }
        let values = stop
            .as_array()
            .ok_or_else(|| "Trae stop must be text or a text array".to_string())?;
        if values.len() > 4 || values.iter().any(|value| !value.is_string()) {
            return Err("Trae stop must contain at most four strings".to_string());
        }
    }
    if payload
        .get("n")
        .is_some_and(|value| !value.is_null() && value.as_u64() != Some(1))
    {
        return Err("Trae supports one completion per request".to_string());
    }
    if payload.get("response_format").is_some_and(|value| {
        !value.is_null() && value.get("type").and_then(Value::as_str) != Some("text")
    }) {
        return Err(
            "Trae structured output constraints are not supported by the reviewed contract"
                .to_string(),
        );
    }
    // These controls have no reviewed Solo wire semantics. Never silently claim enforcement.
    if payload.get("seed").is_some_and(|value| !value.is_null()) {
        return Err("Trae does not support seed".to_string());
    }
    for key in [
        "logprobs",
        "top_logprobs",
        "logit_bias",
        "frequency_penalty",
        "presence_penalty",
        "top_k",
    ] {
        if payload
            .get(key)
            .is_some_and(|value| !value.is_null() && value != &json!(false) && value != &json!(0))
        {
            return Err(format!("Trae does not support {key}"));
        }
    }
    for key in [
        "n",
        "response_format",
        "metadata",
        "user",
        "service_tier",
        "store",
        "logprobs",
        "top_logprobs",
        "logit_bias",
        "seed",
        "frequency_penalty",
        "presence_penalty",
        "top_k",
    ] {
        payload.remove(key);
    }
    Ok(())
}

fn apply_reasoning_fields(
    payload: &mut Map<String, Value>,
    capability: &TraeModelCapability,
) -> Result<(), String> {
    let requested_level = requested_reasoning_level(payload)?;
    let max_requested = requested_max_mode(payload)?;
    if max_requested && !capability.max_mode {
        return Err("Trae model does not support SOLO max mode".to_string());
    }
    for key in [
        "enable_thinking",
        "enable_reasoning",
        "is_reasoning",
        "reasoning_effort",
        "reasoning",
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
        .ok_or_else(|| {
            format!(
                "Trae model {} does not support the requested reasoning level",
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
    let mut selected = None;
    for key in ["reasoning_effort", "reasoning", "reasoning_effort_level"] {
        let Some(value) = payload.get(key).filter(|value| !value.is_null()) else {
            continue;
        };
        let raw = value
            .as_str()
            .or_else(|| value.get("effort").and_then(Value::as_str))
            .or_else(|| value.get("level").and_then(Value::as_str))
            .ok_or_else(|| format!("Trae {key} has an invalid shape"))?;
        let level = normalize_reasoning_level(raw)
            .ok_or_else(|| "unsupported Trae reasoning effort".to_string())?;
        if selected.as_ref().is_some_and(|current| current != &level) {
            return Err("Trae reasoning controls conflict".to_string());
        }
        selected = Some(level);
    }
    for key in ["enable_thinking", "enable_reasoning", "is_reasoning"] {
        if let Some(value) = payload.get(key) {
            let enabled = value
                .as_bool()
                .ok_or_else(|| format!("Trae {key} must be a boolean"))?;
            let level = if enabled { "medium" } else { "none" }.to_string();
            if selected
                .as_ref()
                .is_some_and(|current| (current == "none") == enabled)
            {
                return Err("Trae reasoning controls conflict".to_string());
            }
            if selected.is_none() {
                selected = Some(level);
            }
        }
    }
    Ok(selected)
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
        "4001" | "4023" => axum::http::StatusCode::BAD_REQUEST,
        "4015" | "4019" => axum::http::StatusCode::FORBIDDEN,
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
    wire: super::sse_transport::SseEventDecoder,
    contract: Option<TraeToolContract>,
    raw: bool,
    markers: Option<raw::ToolMarkers>,
    aggregate_bytes: usize,
    memory: Option<super::request_memory::RequestMemoryBudget>,
    resident: Option<super::request_memory::RequestMemoryReservation>,
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
            wire: super::sse_transport::SseEventDecoder::new(MAX_SSE_EVENT_BYTES),
            contract: None,
            raw: false,
            markers: None,
            aggregate_bytes: 0,
            memory: None,
            resident: None,
            id: format!("chatcmpl-trae-{}", random_trae_request_id()),
            created: super::openai_chat_compat::unix_timestamp_seconds(),
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

    pub(super) fn with_contract(model: impl Into<String>, contract: TraeToolContract) -> Self {
        let mut decoder = Self::new(model);
        decoder.contract = Some(contract);
        decoder
    }

    pub(super) fn with_raw(model: &str, contract: TraeToolContract, simulation: bool) -> Self {
        let mut decoder = Self::with_contract(model, contract);
        decoder.raw = true;
        decoder.markers = simulation.then(raw::ToolMarkers::default);
        decoder
    }

    pub(super) fn attach_memory(
        &mut self,
        memory: Option<super::request_memory::RequestMemoryBudget>,
    ) -> Result<(), ProxyError> {
        self.resident = memory
            .as_ref()
            .map(|budget| {
                budget.reserve(
                    super::request_memory::RequestMemoryComponent::StreamRetainedState,
                    self.retained_bytes(),
                )
            })
            .transpose()
            .map_err(|error| error.into_proxy_error())?;
        self.memory = memory;
        Ok(())
    }

    fn working_memory(
        &self,
        incoming: usize,
    ) -> Result<Option<super::request_memory::RequestMemoryReservation>, ProxyError> {
        self.memory
            .as_ref()
            .map(|budget| {
                budget.reserve(
                    super::request_memory::RequestMemoryComponent::NormalizedEvent,
                    self.wire
                        .retained_bytes()
                        .saturating_add(
                            self.markers
                                .as_ref()
                                .map_or(0, raw::ToolMarkers::retained_bytes),
                        )
                        .saturating_add(
                            self.tool_calls
                                .values()
                                .map(|call| call.arguments.capacity())
                                .sum::<usize>(),
                        )
                        .saturating_add(incoming)
                        .saturating_mul(64)
                        .saturating_add(self.retained_bytes().saturating_mul(4))
                        .saturating_add(8192),
                )
            })
            .transpose()
            .map_err(|error| error.into_proxy_error())
    }

    fn retain_output(&self, output: Vec<u8>) -> Result<Bytes, ProxyError> {
        if let Some(resident) = &self.resident {
            resident
                .resize(self.retained_bytes())
                .map_err(|error| error.into_proxy_error())?;
        }
        let bytes = Bytes::from(output);
        self.memory.as_ref().map_or(Ok(bytes.clone()), |budget| {
            budget
                .retain_bytes(
                    super::request_memory::RequestMemoryComponent::NormalizedEvent,
                    bytes,
                )
                .map_err(|error| error.into_proxy_error())
        })
    }

    pub(super) fn snapshot_completion(&self) -> Result<Value, ProxyError> {
        if !self.complete {
            return Err(ProxyError::bad_gateway("Trae completion is not terminal"));
        }
        let mut message = json!({"role":"assistant", "content": self.content});
        if !self.reasoning_content.is_empty() {
            message["reasoning_content"] = json!(self.reasoning_content);
        }
        if !self.tool_calls.is_empty() {
            message["tool_calls"] = json!(self.final_tool_calls());
        }
        let mut result = json!({"id":self.id,"object":"chat.completion","created":self.created,"model":self.model(),"choices":[{"index":0,"message":message,"finish_reason":self.finish_reason}]});
        if let Some(usage) = &self.usage {
            result["usage"] = usage.clone();
        }
        Ok(result)
    }

    pub(super) fn retained_bytes(&self) -> usize {
        self.wire
            .retained_bytes()
            .saturating_add(
                self.markers
                    .as_ref()
                    .map_or(0, raw::ToolMarkers::retained_bytes),
            )
            .saturating_add(
                self.contract
                    .as_ref()
                    .map_or(0, TraeToolContract::retained_bytes),
            )
            .saturating_add(self.content.capacity())
            .saturating_add(self.reasoning_content.capacity())
            .saturating_add(
                self.tool_calls
                    .values()
                    .map(|call| {
                        call.id.capacity()
                            + call.name.capacity()
                            + call.arguments.capacity()
                            + call.call_type.capacity()
                            + 128
                    })
                    .sum::<usize>(),
            )
            .saturating_add(
                self.usage
                    .as_ref()
                    .map_or(0, super::request_memory::retained_json_bytes),
            )
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
        let _working = self.working_memory(chunk.len())?;
        let events = self
            .wire
            .push(&chunk)
            .map_err(|error| ProxyError::bad_gateway(format!("invalid Trae SSE: {error}")))?;
        let mut output = Vec::new();
        for event in events {
            self.decode_event(event, &mut output)?;
        }
        self.retain_output(output).map_err(Into::into)
    }

    pub fn finish_classified(&mut self) -> Result<Bytes, TraeSseDecodeError> {
        if self.complete {
            return Err(ProxyError::bad_gateway("Trae SSE was finalized more than once").into());
        }
        let _working = self.working_memory(0)?;
        let events = self
            .wire
            .finish()
            .map_err(|error| ProxyError::bad_gateway(format!("invalid Trae SSE: {error}")))?;
        let mut output = Vec::new();
        for event in events {
            self.decode_event(event, &mut output)?;
        }
        if !self.saw_done {
            return Err(
                ProxyError::bad_gateway("Trae SSE ended without exactly one done event").into(),
            );
        }
        self.validate_tool_calls()?;
        self.complete = true;
        append_sse_json(&mut output, &self.terminal_chunk())?;
        output.extend_from_slice(b"data: [DONE]\n\n");
        self.retain_output(output).map_err(Into::into)
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

    fn decode_event(
        &mut self,
        event: super::sse_transport::SseEvent,
        output: &mut Vec<u8>,
    ) -> Result<(), TraeSseDecodeError> {
        if !event.has_data {
            return Ok(());
        }
        let name = event
            .event
            .as_deref()
            .filter(|name| !name.is_empty())
            .or(self.raw.then_some("message"))
            .ok_or_else(|| ProxyError::bad_gateway("Trae SSE data is missing an event name"))?;
        if self.saw_done {
            return Err(ProxyError::bad_gateway(if name == "done" {
                "Trae SSE emitted duplicate done events"
            } else {
                "Trae SSE emitted an event after done"
            })
            .into());
        }
        let data = std::str::from_utf8(&event.data)
            .map_err(|_| ProxyError::bad_gateway("Trae SSE event is not UTF-8"))?;
        if self.raw && data.trim() == "[DONE]" {
            return self.consume_done(&json!({}), output);
        }
        let value = serde_json::from_str::<Value>(data).map_err(|error| {
            ProxyError::bad_gateway(format!("invalid Trae SSE {name} event: {error}"))
        })?;
        if name != "error" && recursive_business_code(&value, 0).is_some() {
            return Err(TraeSseDecodeError::Upstream(TraeUpstreamError::from_event(
                &value,
            )));
        }
        match name {
            "progress_notice" => {
                let notice = value
                    .as_str()
                    .filter(|notice| {
                        notice.starts_with("Processing_") && notice.len() <= MAX_ERROR_MESSAGE_BYTES
                    })
                    .ok_or_else(|| ProxyError::bad_gateway("invalid Trae progress notice"))?;
                let _ = notice;
                Ok(())
            }
            "metadata" => self.consume_metadata(&value),
            "output" | "message" if self.raw => {
                if value
                    .get("tool_calls")
                    .and_then(Value::as_array)
                    .is_some_and(|calls| !calls.is_empty())
                {
                    return Err(ProxyError::bad_gateway(
                        "Trae raw_chat native tools have no reviewed contract",
                    )
                    .into());
                }
                if let Some(text) = value.get("response").and_then(Value::as_str) {
                    if let Some(markers) = &mut self.markers {
                        let deltas = markers.feed(text, false)?;
                        for delta in deltas {
                            self.consume_output(&delta, output)?;
                        }
                        let mut rest = value.clone();
                        rest.as_object_mut().unwrap().remove("response");
                        return self.consume_output(&rest, output);
                    }
                }
                self.consume_output(&value, output)
            }
            "output" => self.consume_output(&value, output),
            "usage" if self.raw => self.consume_usage(&value),
            "notify_usage" if self.raw && value.is_object() => Ok(()),
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
        let object = value
            .as_object()
            .ok_or_else(|| ProxyError::bad_gateway("Trae token_usage must be an object"))?;
        let mut usage = self.usage.clone().unwrap_or_else(|| json!({}));
        for key in [
            "prompt_tokens",
            "completion_tokens",
            "total_tokens",
            "cache_read_input_tokens",
            "cache_creation_input_tokens",
            "reasoning_tokens",
        ] {
            if let Some(value) = object.get(key) {
                let number = value
                    .as_u64()
                    .filter(|number| *number <= i64::MAX as u64)
                    .ok_or_else(|| {
                        ProxyError::bad_gateway(format!(
                            "Trae usage {key} must be a nonnegative integer"
                        ))
                    })?;
                usage[key] = json!(number);
            }
        }
        for (detail, key) in [
            ("prompt_tokens_details", "cached_tokens"),
            ("completion_tokens_details", "reasoning_tokens"),
        ] {
            if let Some(value) = object.get(detail) {
                let value = value.as_object().ok_or_else(|| {
                    ProxyError::bad_gateway("Trae usage details must be an object")
                })?;
                if let Some(number) = value.get(key) {
                    let number = number
                        .as_u64()
                        .filter(|number| *number <= i64::MAX as u64)
                        .ok_or_else(|| {
                            ProxyError::bad_gateway(
                                "Trae usage detail must be a nonnegative integer",
                            )
                        })?;
                    usage[detail] = json!({key:number});
                }
            }
        }
        let prompt = usage["prompt_tokens"].as_u64().unwrap_or(0);
        let completion = usage["completion_tokens"].as_u64().unwrap_or(0);
        let sum = prompt
            .checked_add(completion)
            .filter(|sum| *sum <= i64::MAX as u64)
            .ok_or_else(|| ProxyError::bad_gateway("Trae usage total overflows"))?;
        usage["prompt_tokens"] = json!(prompt);
        usage["completion_tokens"] = json!(completion);
        if !object.contains_key("total_tokens") {
            usage["total_tokens"] = json!(sum);
        }
        let cached = usage["cache_read_input_tokens"]
            .as_u64()
            .or_else(|| {
                usage
                    .pointer("/prompt_tokens_details/cached_tokens")
                    .and_then(Value::as_u64)
            })
            .unwrap_or(0);
        let reasoning = usage["reasoning_tokens"]
            .as_u64()
            .or_else(|| {
                usage
                    .pointer("/completion_tokens_details/reasoning_tokens")
                    .and_then(Value::as_u64)
            })
            .unwrap_or(0);
        if cached > prompt || reasoning > completion {
            return Err(ProxyError::bad_gateway(
                "Trae usage detail exceeds its inclusive token count",
            )
            .into());
        }
        usage["prompt_tokens_details"] = json!({"cached_tokens":cached});
        usage["completion_tokens_details"] = json!({"reasoning_tokens":reasoning});
        self.usage = Some(usage);
        Ok(())
    }

    fn consume_done(
        &mut self,
        value: &Value,
        output: &mut Vec<u8>,
    ) -> Result<(), TraeSseDecodeError> {
        if self.saw_done {
            return Err(ProxyError::bad_gateway("Trae emitted duplicate done").into());
        }
        if let Some(markers) = &mut self.markers {
            let deltas = markers.feed("", true)?;
            for delta in deltas {
                self.consume_output(&delta, output)?;
            }
        }
        let object = value
            .as_object()
            .ok_or_else(|| ProxyError::bad_gateway("Trae done must be an object"))?;
        if let Some(reason) = object.get("finish_reason").filter(|value| !value.is_null()) {
            self.finish_reason =
                required_nonempty_string(reason, "Trae done finish_reason")?.to_string();
        }
        if !self.tool_calls.is_empty()
            && !matches!(self.finish_reason.as_str(), "stop" | "tool_calls")
        {
            return Err(ProxyError::bad_gateway(
                "Trae tool calls ended with an incomplete finish reason",
            )
            .into());
        }
        let mut empty_argument_deltas = Vec::new();
        for (index, call) in &mut self.tool_calls {
            if call.arguments.is_empty() {
                if !self
                    .contract
                    .as_ref()
                    .is_some_and(|contract| contract.allows_empty_arguments(&call.name))
                {
                    return Err(
                        ProxyError::bad_gateway("Trae tool call is missing arguments").into(),
                    );
                }
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
        self.validate_tool_calls()?;
        if !self.tool_calls.is_empty() {
            self.finish_reason = "tool_calls".to_string();
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
            for field in ["id", "type"] {
                if item.get(field).and_then(Value::as_str) == Some("") {
                    item.remove(field);
                }
            }
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
            if function.get("name").and_then(Value::as_str) == Some("") {
                function.remove("name");
            }
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
        let mut ids = BTreeSet::new();
        for call in self.tool_calls.values() {
            if exact_tool_id(Some(&json!(call.id))).is_err()
                || call.name.trim().is_empty()
                || call.call_type != "function"
            {
                return Err(
                    ProxyError::bad_gateway("Trae tool call has an invalid identity").into(),
                );
            }
            if !ids.insert(&call.id) {
                return Err(ProxyError::bad_gateway("Trae tool calls reuse an id").into());
            }
            let arguments = serde_json::from_str::<Value>(&call.arguments).map_err(|_| {
                ProxyError::bad_gateway("Trae tool call has invalid JSON arguments")
            })?;
            if !arguments.is_object() {
                return Err(
                    ProxyError::bad_gateway("Trae tool arguments must be a JSON object").into(),
                );
            }
            if let Some(contract) = &self.contract {
                contract
                    .validate_call(&call.name, &arguments)
                    .map_err(ProxyError::bad_gateway)?;
            }
        }
        if let Some(contract) = &self.contract {
            contract
                .validate_count(self.tool_calls.len())
                .map_err(ProxyError::bad_gateway)?;
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
    if let Some(code) = value
        .get("code")
        .or_else(|| value.get("Code"))
        .and_then(json_scalar_string)
    {
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

fn integer_value(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_u64().and_then(|value| i64::try_from(value).ok()))
}

#[cfg(test)]
mod tests {
    use super::*;
    include!("trae/enhancement_tests.rs");

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
                }, {"role":"tool","tool_call_id":"call-1","content":"sunny"}],
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
    fn payload_rejects_images_and_unsupported_explicit_max_mode() {
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
        let error = build_trae_payload(
            &json!({
                "messages": [{"role": "user", "content": "hello"}],
                "is_max_mode": 1
            }),
            "DeepSeek-V4-Pro-Official",
            &caps,
        )
        .unwrap_err();
        assert!(error.contains("max mode"));
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
        let tail_text = String::from_utf8(tail.to_vec()).unwrap();
        assert!(tail_text.ends_with("data: [DONE]\n\n"));
        assert!(decoder.is_terminal());
        let created_values = format!("{first_text}{tail_text}")
            .lines()
            .filter_map(|line| line.strip_prefix("data: "))
            .filter(|payload| *payload != "[DONE]")
            .map(|payload| {
                serde_json::from_str::<Value>(payload).unwrap()["created"]
                    .as_i64()
                    .unwrap()
            })
            .collect::<Vec<_>>();
        assert!(!created_values.is_empty());
        assert!(created_values[0] > 0);
        assert!(created_values
            .iter()
            .all(|created| *created == created_values[0]));
        let response = decoder.into_chat_completion().unwrap();
        assert_eq!(response["created"].as_i64(), Some(created_values[0]));
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
        let contract = TraeToolContract::from_payload(&json!({"tools":[{"function":{"name":"NoArgs","parameters":{"type":"object","properties":{}}}}]})).unwrap();
        let mut decoder = TraeSseDecoder::with_contract("glm-5.2", contract);
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
