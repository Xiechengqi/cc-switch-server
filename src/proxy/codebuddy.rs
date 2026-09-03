use std::collections::BTreeMap;

use bytes::Bytes;
use rand::RngCore;
use serde_json::{json, Value};

use super::ProxyError;

const MAX_SSE_EVENT_BYTES: usize = 2 * 1024 * 1024;
const MAX_ERROR_MESSAGE_BYTES: usize = 2_048;
const MAX_TOOL_CALLS: usize = 128;
const MAX_TEXT_BYTES: usize = 32 * 1024 * 1024;
const MAX_AGGREGATE_BYTES: usize = 64 * 1024 * 1024;

pub fn random_codebuddy_request_id() -> String {
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeBuddyUpstreamError {
    pub status: u16,
    pub code: Option<i64>,
    pub message: String,
}

impl CodeBuddyUpstreamError {
    pub fn from_status_body(status: u16, body: &[u8]) -> Self {
        let value = serde_json::from_slice::<Value>(body).unwrap_or(Value::Null);
        let code = recursive_business_code(&value, 0);
        let message = value
            .get("msg")
            .or_else(|| value.get("message"))
            .or_else(|| value.pointer("/error/message"))
            .and_then(Value::as_str)
            .map(sanitize_error_message)
            .filter(|message| !message.is_empty())
            .unwrap_or_else(|| {
                let text = String::from_utf8_lossy(body);
                let lower = text.to_ascii_lowercase();
                if lower.contains("<html") || lower.contains("<!doctype") {
                    format!("CodeBuddy upstream returned HTTP {status} with an HTML error page")
                } else {
                    format!("CodeBuddy upstream returned HTTP {status}")
                }
            });
        Self {
            status,
            code,
            message,
        }
    }

    pub fn from_event(value: &Value) -> Option<Self> {
        let code = recursive_business_code(value, 0)?;
        if code == 0 || value.get("choices").is_some() {
            return None;
        }
        let message = value
            .get("msg")
            .or_else(|| value.get("message"))
            .or_else(|| value.pointer("/error/message"))
            .and_then(Value::as_str)
            .map(sanitize_error_message)
            .filter(|message| !message.is_empty())
            .unwrap_or_else(|| "CodeBuddy stream returned an upstream error".to_string());
        Some(Self {
            status: status_for_code(code).as_u16(),
            code: Some(code),
            message,
        })
    }

    pub fn downstream_status(&self) -> axum::http::StatusCode {
        self.code.map(status_for_code).unwrap_or_else(|| {
            axum::http::StatusCode::from_u16(self.status)
                .unwrap_or(axum::http::StatusCode::BAD_GATEWAY)
        })
    }

    pub fn is_authentication_failure(&self) -> bool {
        self.status == 401
    }

    pub fn is_rate_limited(&self) -> bool {
        self.downstream_status() == axum::http::StatusCode::TOO_MANY_REQUESTS
    }

    pub fn into_proxy_error(self) -> ProxyError {
        ProxyError {
            status: self.downstream_status(),
            message: match self.code {
                Some(code) => format!("CodeBuddy upstream error {code}: {}", self.message),
                None => self.message,
            },
        }
    }
}

fn status_for_code(code: i64) -> axum::http::StatusCode {
    match code {
        12_005 | 11_212 | 11_216 | 12_153 => axum::http::StatusCode::UNAUTHORIZED,
        11_102 | 11_133 => axum::http::StatusCode::FORBIDDEN,
        14_001..=14_018 | 6_001..=6_008 | 10_105 | 15_001 => {
            axum::http::StatusCode::TOO_MANY_REQUESTS
        }
        11_101 | 11_115 | 11_128 => axum::http::StatusCode::BAD_REQUEST,
        _ => axum::http::StatusCode::BAD_GATEWAY,
    }
}

#[derive(Debug)]
pub enum CodeBuddySseDecodeError {
    Upstream(CodeBuddyUpstreamError),
    Protocol(ProxyError),
}

impl CodeBuddySseDecodeError {
    pub fn into_proxy_error(self) -> ProxyError {
        match self {
            Self::Upstream(error) => error.into_proxy_error(),
            Self::Protocol(error) => error,
        }
    }
}

impl From<ProxyError> for CodeBuddySseDecodeError {
    fn from(error: ProxyError) -> Self {
        Self::Protocol(error)
    }
}

#[derive(Debug, Default)]
pub struct CodeBuddySseDecoder {
    buffer: Vec<u8>,
    saw_done: bool,
    complete: bool,
}

impl CodeBuddySseDecoder {
    /// True only after upstream EOF validated the single previously observed
    /// `[DONE]`. Merely receiving `[DONE]` does not commit a successful
    /// terminal because a later network chunk may still contain a duplicate
    /// terminal or business data.
    pub fn is_terminal(&self) -> bool {
        self.complete
    }

    pub fn push_classified(&mut self, chunk: Bytes) -> Result<Bytes, CodeBuddySseDecodeError> {
        if self.complete && !chunk.is_empty() {
            return Err(
                ProxyError::bad_gateway("CodeBuddy SSE emitted bytes after completion").into(),
            );
        }
        if chunk.is_empty() {
            return Ok(Bytes::new());
        }
        self.buffer.extend_from_slice(&chunk);
        self.drain(false)
    }

    pub fn finish_classified(&mut self) -> Result<Bytes, CodeBuddySseDecodeError> {
        if self.complete {
            return Err(
                ProxyError::bad_gateway("CodeBuddy SSE was finalized more than once").into(),
            );
        }
        let mut output = self.drain(true)?.to_vec();
        if !self.saw_done {
            return Err(
                ProxyError::bad_gateway("CodeBuddy SSE ended without exactly one [DONE]").into(),
            );
        }
        self.complete = true;
        output.extend_from_slice(b"data: [DONE]\n\n");
        Ok(Bytes::from(output))
    }

    fn drain(&mut self, finish: bool) -> Result<Bytes, CodeBuddySseDecodeError> {
        let mut output = Vec::new();
        while let Some((end, delimiter)) = next_event_boundary(&self.buffer) {
            if end > MAX_SSE_EVENT_BYTES {
                return Err(
                    ProxyError::bad_gateway("CodeBuddy SSE event exceeds the limit").into(),
                );
            }
            let event = self.buffer[..end].to_vec();
            self.buffer.drain(..end + delimiter);
            self.decode_event(&event, &mut output)?;
        }
        if self.buffer.len() > MAX_SSE_EVENT_BYTES {
            return Err(ProxyError::bad_gateway("CodeBuddy SSE event exceeds the limit").into());
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
    ) -> Result<(), CodeBuddySseDecodeError> {
        let text = std::str::from_utf8(event)
            .map_err(|_| ProxyError::bad_gateway("CodeBuddy SSE event is not UTF-8"))?;
        let mut data_lines = Vec::new();
        for line in text.split(['\n', '\r']) {
            let line = line.trim_end_matches('\r');
            if let Some(value) = line.strip_prefix("data:") {
                data_lines.push(value.trim_start());
            } else if !line.is_empty()
                && !line.starts_with(':')
                && !line.starts_with("event:")
                && !line.starts_with("id:")
                && !line.starts_with("retry:")
            {
                return Err(
                    ProxyError::bad_gateway("CodeBuddy SSE contains an unsupported field").into(),
                );
            }
        }
        if data_lines.is_empty() {
            return Ok(());
        }
        let data = data_lines.join("\n");
        if data == "[DONE]" {
            if self.saw_done {
                return Err(
                    ProxyError::bad_gateway("CodeBuddy SSE emitted duplicate [DONE]").into(),
                );
            }
            self.saw_done = true;
            return Ok(());
        }
        if self.saw_done {
            return Err(ProxyError::bad_gateway("CodeBuddy SSE emitted data after [DONE]").into());
        }
        let value = serde_json::from_str::<Value>(&data).map_err(|error| {
            ProxyError::bad_gateway(format!("invalid CodeBuddy SSE chunk: {error}"))
        })?;
        if let Some(error) = CodeBuddyUpstreamError::from_event(&value) {
            return Err(CodeBuddySseDecodeError::Upstream(error));
        }
        let object = value
            .as_object()
            .ok_or_else(|| ProxyError::bad_gateway("CodeBuddy SSE chunk must be an object"))?;
        if !object.contains_key("choices") && !object.contains_key("usage") {
            return Err(ProxyError::bad_gateway(
                "CodeBuddy SSE chunk has neither choices nor usage",
            )
            .into());
        }
        let canonical = serde_json::to_vec(&value).map_err(|error| {
            ProxyError::bad_gateway(format!("encode CodeBuddy SSE chunk: {error}"))
        })?;
        output.extend_from_slice(b"data: ");
        output.extend_from_slice(&canonical);
        output.extend_from_slice(b"\n\n");
        Ok(())
    }
}

#[derive(Debug, Default)]
pub struct CodeBuddyChatSseAggregator {
    buffer: Vec<u8>,
    aggregate_bytes: usize,
    id: Option<String>,
    created: Option<i64>,
    model: Option<String>,
    content: String,
    reasoning_content: String,
    tool_calls: BTreeMap<usize, AggregatedToolCall>,
    fallback_tool_order: Vec<usize>,
    finish_reason: Option<Value>,
    usage: Option<Value>,
    terminal: bool,
}

#[derive(Debug, Default)]
struct AggregatedToolCall {
    id: String,
    call_type: String,
    name: String,
    arguments: String,
}

impl CodeBuddyChatSseAggregator {
    pub fn push(&mut self, chunk: Bytes) -> Result<(), ProxyError> {
        if chunk.is_empty() {
            return Ok(());
        }
        self.aggregate_bytes = self.aggregate_bytes.saturating_add(chunk.len());
        if self.aggregate_bytes > MAX_AGGREGATE_BYTES {
            return Err(ProxyError::bad_gateway(
                "CodeBuddy Chat response exceeds the aggregate limit",
            ));
        }
        self.buffer.extend_from_slice(&chunk);
        self.drain(false)
    }

    pub fn finish(
        mut self,
        fallback_model: &str,
        now_unix_seconds: i64,
    ) -> Result<Value, ProxyError> {
        self.drain(true)?;
        if !self.terminal {
            return Err(ProxyError::bad_gateway(
                "CodeBuddy Chat SSE ended without exactly one [DONE]",
            ));
        }
        let mut message = serde_json::Map::new();
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
        if !self.tool_calls.is_empty() {
            let mut calls = Vec::with_capacity(self.tool_calls.len());
            for (index, call) in self.tool_calls {
                if call.id.trim().is_empty() || call.name.trim().is_empty() {
                    return Err(ProxyError::bad_gateway(format!(
                        "CodeBuddy Chat tool call {index} is missing id or function name"
                    )));
                }
                if serde_json::from_str::<Value>(&call.arguments).is_err() {
                    return Err(ProxyError::bad_gateway(format!(
                        "CodeBuddy Chat tool call {index} has invalid JSON arguments"
                    )));
                }
                calls.push(json!({
                    "id": call.id,
                    "type": if call.call_type.is_empty() { "function" } else { call.call_type.as_str() },
                    "function": {"name": call.name, "arguments": call.arguments}
                }));
            }
            message.insert("tool_calls".to_string(), Value::Array(calls));
        }
        let mut response = json!({
            "id": self.id.unwrap_or_else(|| format!("chatcmpl-codebuddy-{now_unix_seconds}")),
            "object": "chat.completion",
            "created": self.created.unwrap_or(now_unix_seconds),
            "model": self.model.filter(|value| !value.trim().is_empty()).unwrap_or_else(|| fallback_model.to_string()),
            "choices": [{
                "index": 0,
                "message": Value::Object(message),
                "finish_reason": self.finish_reason.unwrap_or_else(|| Value::String("stop".to_string()))
            }]
        });
        if let Some(usage) = self.usage {
            response["usage"] = usage;
        }
        Ok(response)
    }

    fn drain(&mut self, finish: bool) -> Result<(), ProxyError> {
        while let Some((end, delimiter)) = next_event_boundary(&self.buffer) {
            if end > MAX_SSE_EVENT_BYTES {
                return Err(ProxyError::bad_gateway(
                    "CodeBuddy Chat SSE event exceeds the limit",
                ));
            }
            let event = self.buffer[..end].to_vec();
            self.buffer.drain(..end + delimiter);
            self.consume_event(&event)?;
        }
        if self.buffer.len() > MAX_SSE_EVENT_BYTES {
            return Err(ProxyError::bad_gateway(
                "CodeBuddy Chat SSE event exceeds the limit",
            ));
        }
        if finish && !self.buffer.is_empty() {
            let event = std::mem::take(&mut self.buffer);
            self.consume_event(&event)?;
        }
        Ok(())
    }

    fn consume_event(&mut self, event: &[u8]) -> Result<(), ProxyError> {
        let text = std::str::from_utf8(event)
            .map_err(|_| ProxyError::bad_gateway("CodeBuddy Chat SSE event is not UTF-8"))?;
        let mut data = None;
        for line in text.lines() {
            let line = line.trim_end_matches('\r');
            if let Some(value) = line.strip_prefix("data:") {
                if data.is_some() {
                    return Err(ProxyError::bad_gateway(
                        "CodeBuddy Chat SSE event contains multiple data lines",
                    ));
                }
                data = Some(value.trim_start());
            } else if !line.is_empty() && !line.starts_with(':') && !line.starts_with("event:") {
                return Err(ProxyError::bad_gateway(
                    "CodeBuddy Chat SSE contains an unsupported field",
                ));
            }
        }
        let Some(data) = data else {
            return Ok(());
        };
        if data == "[DONE]" {
            if self.terminal {
                return Err(ProxyError::bad_gateway(
                    "CodeBuddy Chat SSE emitted duplicate [DONE]",
                ));
            }
            self.terminal = true;
            return Ok(());
        }
        if self.terminal {
            return Err(ProxyError::bad_gateway(
                "CodeBuddy Chat SSE emitted data after [DONE]",
            ));
        }
        let value = serde_json::from_str::<Value>(data).map_err(|error| {
            ProxyError::bad_gateway(format!("invalid CodeBuddy Chat SSE chunk: {error}"))
        })?;
        let object = value
            .as_object()
            .ok_or_else(|| ProxyError::bad_gateway("CodeBuddy Chat SSE chunk must be an object"))?;
        update_optional_string(&mut self.id, object.get("id"), "id")?;
        update_optional_string(&mut self.model, object.get("model"), "model")?;
        if let Some(created) = object.get("created") {
            let created = integer_value(created).ok_or_else(|| {
                ProxyError::bad_gateway("CodeBuddy Chat SSE created must be an integer")
            })?;
            if self.created.is_some_and(|current| current != created) {
                return Err(ProxyError::bad_gateway(
                    "CodeBuddy Chat SSE changed created across chunks",
                ));
            }
            self.created = Some(created);
        }
        if let Some(usage) = object.get("usage") {
            if !usage.is_object() {
                return Err(ProxyError::bad_gateway(
                    "CodeBuddy Chat SSE usage must be an object",
                ));
            }
            self.usage = Some(usage.clone());
        }
        let choices = object
            .get("choices")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                ProxyError::bad_gateway("CodeBuddy Chat SSE choices must be an array")
            })?;
        for choice in choices {
            let choice = choice.as_object().ok_or_else(|| {
                ProxyError::bad_gateway("CodeBuddy Chat SSE choice must be an object")
            })?;
            if choice.get("index").and_then(integer_value).unwrap_or(0) != 0 {
                return Err(ProxyError::bad_gateway(
                    "CodeBuddy Chat SSE returned an unsupported choice index",
                ));
            }
            if let Some(reason) = choice.get("finish_reason").filter(|value| !value.is_null()) {
                let reason = reason
                    .as_str()
                    .filter(|reason| !reason.trim().is_empty())
                    .ok_or_else(|| {
                        ProxyError::bad_gateway(
                            "CodeBuddy Chat SSE finish_reason must be a non-empty string or null",
                        )
                    })?;
                self.finish_reason = Some(Value::String(reason.to_string()));
            }
            let Some(delta) = choice.get("delta") else {
                continue;
            };
            let delta = delta.as_object().ok_or_else(|| {
                ProxyError::bad_gateway("CodeBuddy Chat SSE delta must be an object")
            })?;
            append_delta_text(&mut self.content, delta.get("content"), "content")?;
            append_delta_text(
                &mut self.reasoning_content,
                delta
                    .get("reasoning_content")
                    .or_else(|| delta.get("reasoning")),
                "reasoning_content",
            )?;
            if let Some(calls) = delta.get("tool_calls") {
                let calls = calls.as_array().ok_or_else(|| {
                    ProxyError::bad_gateway("CodeBuddy Chat SSE tool_calls must be an array")
                })?;
                for (position, call) in calls.iter().enumerate() {
                    let call = call.as_object().ok_or_else(|| {
                        ProxyError::bad_gateway("CodeBuddy Chat SSE tool call must be an object")
                    })?;
                    let explicit = call
                        .get("index")
                        .and_then(integer_value)
                        .and_then(|value| usize::try_from(value).ok());
                    let id = call
                        .get("id")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty());
                    let index = explicit
                        .or_else(|| {
                            id.and_then(|id| {
                                self.tool_calls
                                    .iter()
                                    .find_map(|(index, call)| (call.id == id).then_some(*index))
                            })
                        })
                        .or_else(|| self.fallback_tool_order.get(position).copied())
                        .unwrap_or_else(|| {
                            (0..MAX_TOOL_CALLS)
                                .find(|index| !self.tool_calls.contains_key(index))
                                .unwrap_or(MAX_TOOL_CALLS)
                        });
                    if index >= MAX_TOOL_CALLS {
                        return Err(ProxyError::bad_gateway(
                            "CodeBuddy Chat SSE tool call index exceeds the limit",
                        ));
                    }
                    if self.fallback_tool_order.len() <= position {
                        self.fallback_tool_order.resize(position + 1, index);
                    }
                    self.fallback_tool_order[position] = index;
                    let target = self.tool_calls.entry(index).or_default();
                    append_stable_field(&mut target.id, call.get("id"), "tool call id")?;
                    append_stable_field(&mut target.call_type, call.get("type"), "tool call type")?;
                    if let Some(function) = call.get("function") {
                        let function = function.as_object().ok_or_else(|| {
                            ProxyError::bad_gateway(
                                "CodeBuddy Chat SSE tool function must be an object",
                            )
                        })?;
                        append_fragment(&mut target.name, function.get("name"), "function name")?;
                        append_fragment(
                            &mut target.arguments,
                            function.get("arguments"),
                            "function arguments",
                        )?;
                    }
                }
            }
        }
        Ok(())
    }
}

fn update_optional_string(
    target: &mut Option<String>,
    value: Option<&Value>,
    field: &str,
) -> Result<(), ProxyError> {
    let Some(value) = value else {
        return Ok(());
    };
    let value = value.as_str().ok_or_else(|| {
        ProxyError::bad_gateway(format!("CodeBuddy Chat SSE {field} must be a string"))
    })?;
    if target.as_deref().is_some_and(|current| current != value) {
        return Err(ProxyError::bad_gateway(format!(
            "CodeBuddy Chat SSE changed {field} across chunks"
        )));
    }
    *target = Some(value.to_string());
    Ok(())
}

fn append_delta_text(
    target: &mut String,
    value: Option<&Value>,
    field: &str,
) -> Result<(), ProxyError> {
    let Some(value) = value else {
        return Ok(());
    };
    if value.is_null() {
        return Ok(());
    }
    let value = value.as_str().ok_or_else(|| {
        ProxyError::bad_gateway(format!("CodeBuddy Chat SSE {field} must be a string"))
    })?;
    target.push_str(value);
    if target.len() > MAX_TEXT_BYTES {
        return Err(ProxyError::bad_gateway(format!(
            "CodeBuddy Chat SSE {field} exceeds the limit"
        )));
    }
    Ok(())
}

fn append_stable_field(
    target: &mut String,
    value: Option<&Value>,
    field: &str,
) -> Result<(), ProxyError> {
    let Some(value) = value else {
        return Ok(());
    };
    let value = value.as_str().ok_or_else(|| {
        ProxyError::bad_gateway(format!("CodeBuddy Chat SSE {field} must be a string"))
    })?;
    if !target.is_empty() && target != value {
        return Err(ProxyError::bad_gateway(format!(
            "CodeBuddy Chat SSE changed {field} across chunks"
        )));
    }
    target.clear();
    target.push_str(value);
    Ok(())
}

fn append_fragment(
    target: &mut String,
    value: Option<&Value>,
    field: &str,
) -> Result<(), ProxyError> {
    let Some(value) = value else {
        return Ok(());
    };
    if value.is_null() {
        return Ok(());
    }
    let value = value.as_str().ok_or_else(|| {
        ProxyError::bad_gateway(format!("CodeBuddy Chat SSE {field} must be a string"))
    })?;
    target.push_str(value);
    if target.len() > MAX_TEXT_BYTES {
        return Err(ProxyError::bad_gateway(format!(
            "CodeBuddy Chat SSE {field} exceeds the limit"
        )));
    }
    Ok(())
}

fn recursive_business_code(value: &Value, depth: usize) -> Option<i64> {
    if depth > 6 {
        return None;
    }
    let own = value.get("code").and_then(integer_value);
    if own.is_some_and(|code| code >= 1_000) {
        return own;
    }
    match value {
        Value::Object(object) => {
            for key in ["data", "error", "details", "message"] {
                let Some(child) = object.get(key) else {
                    continue;
                };
                if let Some(code) = recursive_business_code(child, depth + 1) {
                    return Some(code);
                }
                if let Some(text) = child.as_str() {
                    if let Ok(parsed) = serde_json::from_str::<Value>(text) {
                        if let Some(code) = recursive_business_code(&parsed, depth + 1) {
                            return Some(code);
                        }
                    }
                }
            }
            own.filter(|code| *code == 0)
        }
        Value::Array(values) => values
            .iter()
            .find_map(|value| recursive_business_code(value, depth + 1)),
        _ => own.filter(|code| *code == 0),
    }
}

fn next_event_boundary(buffer: &[u8]) -> Option<(usize, usize)> {
    let lf = buffer.windows(2).position(|window| window == b"\n\n");
    let crlf = buffer.windows(4).position(|window| window == b"\r\n\r\n");
    let cr = buffer.windows(2).position(|window| window == b"\r\r");
    [(lf, 2), (crlf, 4), (cr, 2)]
        .into_iter()
        .filter_map(|(position, delimiter)| position.map(|position| (position, delimiter)))
        .min_by_key(|(position, _)| *position)
}

fn integer_value(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_u64().and_then(|value| i64::try_from(value).ok()))
        .or_else(|| value.as_str()?.trim().parse::<i64>().ok())
}

fn sanitize_error_message(value: &str) -> String {
    value.trim().chars().take(MAX_ERROR_MESSAGE_BYTES).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(value: Value) -> Bytes {
        Bytes::from(format!("data: {value}\n\n"))
    }

    #[test]
    fn decoder_requires_one_done_and_rejects_post_terminal_data() {
        let mut decoder = CodeBuddySseDecoder::default();
        let output = decoder
            .push_classified(chunk(json!({
                "id":"c1","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"content":"ok"},"finish_reason":null}]
            })))
            .unwrap();
        assert!(String::from_utf8_lossy(&output).contains("\"content\":\"ok\""));
        assert!(decoder.finish_classified().is_err());

        let mut decoder = CodeBuddySseDecoder::default();
        let output = decoder
            .push_classified(Bytes::from_static(b"data: [DONE]\n\n"))
            .unwrap();
        assert!(output.is_empty(), "[DONE] must not commit before EOF");
        assert!(!decoder.is_terminal());
        assert_eq!(
            decoder.finish_classified().unwrap(),
            Bytes::from_static(b"data: [DONE]\n\n")
        );
        assert!(decoder.is_terminal());
        assert!(decoder.finish_classified().is_err());
        assert!(decoder
            .push_classified(Bytes::from_static(b"data: [DONE]\n\n"))
            .is_err());

        let mut decoder = CodeBuddySseDecoder::default();
        decoder
            .push_classified(Bytes::from_static(b"data: [DONE]\n\n"))
            .unwrap();
        assert!(decoder
            .push_classified(chunk(json!({"choices":[]})))
            .is_err());

        let mut decoder = CodeBuddySseDecoder::default();
        decoder
            .push_classified(Bytes::from_static(b"data: [DONE]\n\n"))
            .unwrap();
        assert!(decoder
            .push_classified(Bytes::from_static(b"data: [DONE]\n\n"))
            .is_err());
    }

    #[test]
    fn decoder_accepts_standard_multiline_data_single_cr_and_liveness_fields() {
        let mut decoder = CodeBuddySseDecoder::default();
        let output = decoder
            .push_classified(Bytes::from_static(
                b": keepalive\rid: 1\revent: message\rdata: {\rdata: \"choices\":[]}\r\rdata: [DONE]\r\r",
            ))
            .unwrap();
        assert!(String::from_utf8_lossy(&output).contains("\"choices\":[]"));
        assert_eq!(
            decoder.finish_classified().unwrap(),
            Bytes::from_static(b"data: [DONE]\n\n")
        );
    }

    #[test]
    fn decoder_classifies_html_401_and_nested_business_error() {
        let error = CodeBuddyUpstreamError::from_status_body(
            401,
            b"<!doctype html><html>unauthorized</html>",
        );
        assert!(error.is_authentication_failure());
        assert!(!error.message.contains("unauthorized</html>"));

        let mut decoder = CodeBuddySseDecoder::default();
        let error = decoder
            .push_classified(chunk(json!({
                "code":0,"data":{"error":{"code":14003,"message":"too many"}}
            })))
            .unwrap_err();
        let CodeBuddySseDecodeError::Upstream(error) = error else {
            panic!("business error must remain classified")
        };
        assert_eq!(error.code, Some(14_003));
        assert!(error.is_rate_limited());
    }

    #[test]
    fn aggregator_preserves_reasoning_usage_and_missing_tool_index() {
        let mut aggregator = CodeBuddyChatSseAggregator::default();
        aggregator
            .push(chunk(json!({
                "id":"c1","created":1,"model":"default-model","choices":[{"index":0,"delta":{
                    "content":"","reasoning_content":"think","tool_calls":[{"id":"call-1","type":"function","function":{"name":"shell","arguments":"{\"cmd\":"}}]
                },"finish_reason":null}]
            })))
            .unwrap();
        aggregator
            .push(chunk(json!({
                "id":"c1","created":1,"model":"default-model","choices":[{"index":0,"delta":{
                    "content":"","reasoning_content":"","tool_calls":[{"index":0,"function":{"arguments":"\"pwd\"}"}}]
                },"finish_reason":"tool_calls"}]
            })))
            .unwrap();
        aggregator
            .push(chunk(json!({
                "id":"c1","created":1,"model":"default-model","choices":[],"usage":{
                    "prompt_tokens":10,"completion_tokens":3,"total_tokens":13,"credit":0.02,
                    "completion_thinking_tokens":2,"cached_tokens":1,
                    "cache_read_input_tokens":1,"cache_creation_input_tokens":0,
                    "prompt_cache_hit_tokens":1,"prompt_cache_miss_tokens":9,
                    "prompt_cache_write_tokens":0,"completion_tokens_details":{},"prompt_tokens_details":{}
                }
            })))
            .unwrap();
        aggregator
            .push(Bytes::from_static(b"data: [DONE]\n\n"))
            .unwrap();
        let response = aggregator.finish("fallback", 2).unwrap();
        assert_eq!(
            response["choices"][0]["message"]["reasoning_content"],
            "think"
        );
        assert_eq!(
            response["choices"][0]["message"]["tool_calls"][0]["function"]["arguments"],
            "{\"cmd\":\"pwd\"}"
        );
        assert_eq!(response["usage"]["completion_thinking_tokens"], 2);
        assert_eq!(response["usage"]["credit"], 0.02);
    }

    #[test]
    fn aggregator_rejects_duplicate_done_and_invalid_tool_json() {
        let mut aggregator = CodeBuddyChatSseAggregator::default();
        aggregator
            .push(Bytes::from_static(b"data: [DONE]\n\n"))
            .unwrap();
        assert!(aggregator
            .push(Bytes::from_static(b"data: [DONE]\n\n"))
            .is_err());

        let mut aggregator = CodeBuddyChatSseAggregator::default();
        aggregator
            .push(chunk(json!({"choices":[{"index":0,"delta":{"tool_calls":[{
                "id":"call-1","function":{"name":"shell","arguments":"{"}
            }]},"finish_reason":"tool_calls"}]})))
            .unwrap();
        aggregator
            .push(Bytes::from_static(b"data: [DONE]\n\n"))
            .unwrap();
        assert!(aggregator.finish("model", 1).is_err());
    }
}
