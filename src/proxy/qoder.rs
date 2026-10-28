use std::collections::BTreeMap;

use crate::proxy::request_timing::TimingResult;
use bytes::Bytes;
use serde_json::{json, Value};

use super::request_memory::{
    retained_json_bytes, RequestMemoryBudget, RequestMemoryComponent, RequestMemoryReservation,
};
use super::ProxyError;

const MAX_QODER_SSE_EVENT_BYTES: usize = 2 * 1024 * 1024;
const MAX_QODER_ERROR_MESSAGE_BYTES: usize = 2048;
const MAX_QODER_CHAT_TOOL_CALLS: usize = 128;
const MAX_QODER_CHAT_TEXT_BYTES: usize = 32 * 1024 * 1024;
const MAX_QODER_CHAT_AGGREGATE_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QoderUpstreamError {
    pub status: u16,
    pub code: Option<String>,
    pub message: String,
    pub agent_limit_reset_at_ms: Option<i64>,
    pub retry_after_ms: Option<i64>,
}

impl QoderUpstreamError {
    pub fn from_status_body(status: u16, body: &[u8]) -> Self {
        let value = serde_json::from_slice::<Value>(body).unwrap_or(Value::Null);
        let encoded_message = qoder_error_field(&value, "message", 0)
            .and_then(Value::as_str)
            .filter(|message| message.len() <= 64 * 1024)
            .and_then(|message| serde_json::from_str::<Value>(message).ok());
        let code =
            recognized_qoder_error_code(qoder_error_field(&value, "code", 0).or_else(|| {
                encoded_message
                    .as_ref()
                    .and_then(|message| qoder_error_field(message, "code", 0))
            }));
        let message = qoder_error_field(&value, "message", 0)
            .and_then(Value::as_str)
            .map(sanitize_error_message)
            .filter(|message| !message.is_empty())
            .unwrap_or_else(|| format!("Qoder upstream returned HTTP {status}"));
        let agent_limit_reset_at_ms = qoder_error_field(&value, "agentLimitResetTime", 0)
            .or_else(|| {
                encoded_message
                    .as_ref()
                    .and_then(|message| qoder_error_field(message, "agentLimitResetTime", 0))
            })
            .and_then(integer_value)
            .filter(|value| *value > 0);
        let error = Self {
            status,
            code,
            message,
            agent_limit_reset_at_ms,
            retry_after_ms: None,
        };
        crate::metrics::record_qoder_error(if error.is_agent_limited() {
            "rate_limited"
        } else if error.is_entitlement_denied() {
            "permission"
        } else if error.is_authentication_failure() {
            "authentication"
        } else if status >= 500 {
            "temporary"
        } else {
            "upstream_protocol"
        });
        error
    }

    pub fn from_response(status: u16, headers: &axum::http::HeaderMap, body: &[u8]) -> Self {
        let mut error = Self::from_status_body(status, body);
        error.retry_after_ms = parse_qoder_retry_after_ms(headers);
        error
    }

    pub fn downstream_status(&self) -> axum::http::StatusCode {
        match self.code.as_deref() {
            Some("112") => axum::http::StatusCode::FORBIDDEN,
            Some("115") => axum::http::StatusCode::TOO_MANY_REQUESTS,
            _ if self.agent_limit_reset_at_ms.is_some() => {
                axum::http::StatusCode::TOO_MANY_REQUESTS
            }
            _ => axum::http::StatusCode::from_u16(self.status)
                .unwrap_or(axum::http::StatusCode::BAD_GATEWAY),
        }
    }

    pub fn is_authentication_failure(&self) -> bool {
        self.status == 401 && !matches!(self.code.as_deref(), Some("112" | "115"))
    }

    pub fn is_entitlement_denied(&self) -> bool {
        self.code.as_deref() == Some("112")
    }

    pub fn is_agent_limited(&self) -> bool {
        self.code.as_deref() == Some("115") || self.agent_limit_reset_at_ms.is_some()
    }

    pub fn into_proxy_error(self) -> ProxyError {
        let downstream_status = self.downstream_status();
        let rate_limited = self.is_agent_limited() || downstream_status.as_u16() == 429;
        let message = match self.code.as_deref() {
            Some(code) => format!("Qoder upstream error {code}: {}", self.message),
            None => self.message,
        };
        if rate_limited {
            let now_ms = crate::infra::time::now_ms().min(i64::MAX as u128) as i64;
            let retry_after_ms = self.retry_after_ms.or_else(|| {
                self.agent_limit_reset_at_ms
                    .map(|reset| reset.saturating_sub(now_ms))
            });
            let seconds = retry_after_ms
                .unwrap_or(1_000)
                .clamp(1_000, 24 * 60 * 60 * 1_000)
                .saturating_add(999) as u64
                / 1_000;
            return ProxyError::rate_limited(message, seconds.max(1));
        }
        ProxyError {
            status: downstream_status,
            message,
        }
    }
}

fn qoder_error_field<'a>(value: &'a Value, field: &str, depth: usize) -> Option<&'a Value> {
    if depth >= 8 {
        return None;
    }
    let object = value.as_object()?;
    if let Some(value) = object.get(field).filter(|value| !value.is_null()) {
        return Some(value);
    }
    for key in ["error", "data"] {
        if let Some(value) = object
            .get(key)
            .and_then(|value| qoder_error_field(value, field, depth + 1))
        {
            return Some(value);
        }
    }
    None
}

#[derive(Debug)]
pub enum QoderSseDecodeError {
    Upstream(QoderUpstreamError),
    Protocol(ProxyError),
}

impl QoderSseDecodeError {
    pub fn into_proxy_error(self) -> ProxyError {
        match self {
            Self::Upstream(error) => error.into_proxy_error(),
            Self::Protocol(error) => error,
        }
    }

    pub fn upstream(&self) -> Option<&QoderUpstreamError> {
        match self {
            Self::Upstream(error) => Some(error),
            Self::Protocol(_) => None,
        }
    }
}

impl From<ProxyError> for QoderSseDecodeError {
    fn from(error: ProxyError) -> Self {
        Self::Protocol(error)
    }
}

#[derive(Debug, Default)]
pub struct QoderSseDecoder {
    buffer: Vec<u8>,
    saw_terminal: bool,
    saw_meaningful: bool,
    saw_finish_reason: bool,
    saw_tool_calls: bool,
    complete: bool,
    failed: bool,
    request_memory: Option<RequestMemoryBudget>,
    buffer_memory: Option<RequestMemoryReservation>,
    canonical_chat_id: Option<String>,
    _canonical_chat_id_memory: Option<RequestMemoryReservation>,
}

impl QoderSseDecoder {
    pub(crate) fn set_canonical_chat_id(&mut self, id: String) -> Result<(), ProxyError> {
        let memory = self
            .request_memory
            .as_ref()
            .map(|budget| {
                budget.reserve(RequestMemoryComponent::StreamRetainedState, id.capacity())
            })
            .transpose()
            .map_err(|error| error.into_proxy_error())?;
        self.canonical_chat_id = Some(id);
        self._canonical_chat_id_memory = memory;
        Ok(())
    }
    pub(crate) fn with_request_memory(
        request_memory: Option<RequestMemoryBudget>,
    ) -> Result<Self, ProxyError> {
        let buffer_memory = request_memory
            .as_ref()
            .map(|budget| budget.reserve(RequestMemoryComponent::StreamRetainedState, 0))
            .transpose()
            .map_err(|error| error.into_proxy_error())?;
        Ok(Self {
            request_memory,
            buffer_memory,
            ..Self::default()
        })
    }

    pub(crate) fn retained_bytes(&self) -> usize {
        self.buffer.capacity()
    }

    /// True only after upstream EOF validates the single authoritative
    /// terminal. A finish reason or `[DONE]` alone is not enough because a
    /// later network chunk may still contain an error, duplicate terminal, or
    /// business data.
    pub fn is_terminal(&self) -> bool {
        self.complete && !self.failed
    }

    pub fn has_terminal_candidate(&self) -> bool {
        self.saw_terminal && !self.complete && !self.failed
    }

    pub fn push(&mut self, chunk: Bytes) -> Result<Bytes, ProxyError> {
        self.push_classified(chunk)
            .map_err(QoderSseDecodeError::into_proxy_error)
    }

    pub fn push_classified(&mut self, chunk: Bytes) -> Result<Bytes, QoderSseDecodeError> {
        if self.failed {
            return Err(QoderSseDecodeError::Protocol(ProxyError::bad_gateway(
                "Qoder SSE decoder has failed",
            )));
        }
        let result = self.push_classified_inner(chunk);
        self.failed |= result.is_err();
        if result.is_err() {
            if let Some(timing) = self
                .request_memory
                .as_ref()
                .and_then(RequestMemoryBudget::timing)
            {
                timing.observe_terminal(TimingResult::Failed);
            }
        }
        result
    }

    fn push_classified_inner(&mut self, chunk: Bytes) -> Result<Bytes, QoderSseDecodeError> {
        if self.complete && !chunk.is_empty() {
            return Err(QoderSseDecodeError::Protocol(ProxyError::bad_gateway(
                "Qoder SSE emitted bytes after completion",
            )));
        }
        if chunk.is_empty() {
            return Ok(Bytes::new());
        }
        let _transport_memory = self
            .request_memory
            .as_ref()
            .map(|budget| budget.reserve(RequestMemoryComponent::TransportPending, chunk.len()))
            .transpose()
            .map_err(|error| QoderSseDecodeError::Protocol(error.into_proxy_error()))?;
        self.reserve_buffer_append(chunk.len())?;
        self.buffer.extend_from_slice(&chunk);
        let output = self.drain(false)?;
        self.sync_buffer_memory()?;
        Ok(output)
    }

    pub fn finish(&mut self) -> Result<Bytes, ProxyError> {
        self.finish_classified()
            .map_err(QoderSseDecodeError::into_proxy_error)
    }

    pub fn finish_classified(&mut self) -> Result<Bytes, QoderSseDecodeError> {
        if self.failed {
            return Err(QoderSseDecodeError::Protocol(ProxyError::bad_gateway(
                "Qoder SSE decoder has failed",
            )));
        }
        let result = self.finish_classified_inner();
        self.failed |= result.is_err();
        if result.is_err() {
            if let Some(timing) = self
                .request_memory
                .as_ref()
                .and_then(RequestMemoryBudget::timing)
            {
                timing.observe_terminal(TimingResult::Failed);
            }
        }
        result
    }

    fn finish_classified_inner(&mut self) -> Result<Bytes, QoderSseDecodeError> {
        if self.complete {
            return Err(QoderSseDecodeError::Protocol(ProxyError::bad_gateway(
                "Qoder SSE was finalized more than once",
            )));
        }
        let canonical = self.drain(true)?;
        if !self.saw_terminal {
            crate::metrics::record_qoder_error("terminal_missing");
            return Err(QoderSseDecodeError::Protocol(ProxyError::bad_gateway(
                "Qoder SSE ended without an authoritative terminal event",
            )));
        }
        if !self.saw_meaningful || !self.saw_finish_reason {
            crate::metrics::record_qoder_error("upstream_empty");
            return Err(QoderSseDecodeError::Protocol(ProxyError::bad_gateway(
                "Qoder SSE ended without meaningful output and an authoritative finish reason",
            )));
        }
        self.complete = true;
        if let Some(timing) = self
            .request_memory
            .as_ref()
            .and_then(RequestMemoryBudget::timing)
        {
            timing.observe_terminal(TimingResult::Complete);
        }
        let output_capacity = projected_vec_capacity(
            0,
            0,
            canonical.len().saturating_add(b"data: [DONE]\n\n".len()),
        );
        let output_memory = self
            .request_memory
            .as_ref()
            .map(|budget| budget.reserve(RequestMemoryComponent::NormalizedEvent, output_capacity))
            .transpose()
            .map_err(|error| QoderSseDecodeError::Protocol(error.into_proxy_error()))?;
        let mut output = Vec::with_capacity(output_capacity);
        output.extend_from_slice(&canonical);
        output.extend_from_slice(b"data: [DONE]\n\n");
        self.sync_buffer_memory()?;
        let output = Bytes::from(output);
        Ok(match output_memory {
            Some(memory) => memory.retain_bytes(output),
            None => output,
        })
    }

    fn drain(&mut self, finish: bool) -> Result<Bytes, QoderSseDecodeError> {
        let mut output = Vec::new();
        let output_memory = self
            .request_memory
            .as_ref()
            .map(|budget| budget.reserve(RequestMemoryComponent::NormalizedEvent, 0))
            .transpose()
            .map_err(|error| QoderSseDecodeError::Protocol(error.into_proxy_error()))?;
        while let Some((end, delimiter)) = next_event_boundary(&self.buffer) {
            if end > MAX_QODER_SSE_EVENT_BYTES {
                return Err(QoderSseDecodeError::Protocol(ProxyError::bad_gateway(
                    "Qoder SSE event exceeds the limit",
                )));
            }
            let working_memory = self.reserve_event_working_set(end)?;
            let event = self.buffer[..end].to_vec();
            self.buffer.drain(..end + delimiter);
            self.decode_event(&event, &mut output, output_memory.as_ref())?;
            drop(working_memory);
        }
        if self.buffer.len() > MAX_QODER_SSE_EVENT_BYTES {
            return Err(QoderSseDecodeError::Protocol(ProxyError::bad_gateway(
                "Qoder SSE event exceeds the limit",
            )));
        }
        if finish && !self.buffer.is_empty() {
            let event_len = self.buffer.len();
            let working_memory = self.reserve_event_working_set(event_len)?;
            let event = self.buffer[..event_len].to_vec();
            self.buffer.clear();
            self.decode_event(&event, &mut output, output_memory.as_ref())?;
            drop(working_memory);
        }
        if let Some(memory) = output_memory.as_ref() {
            memory
                .resize(output.capacity())
                .map_err(|error| QoderSseDecodeError::Protocol(error.into_proxy_error()))?;
        }
        let output = Bytes::from(output);
        Ok(match output_memory {
            Some(memory) => memory.retain_bytes(output),
            None => output,
        })
    }

    fn decode_event(
        &mut self,
        event: &[u8],
        output: &mut Vec<u8>,
        output_memory: Option<&RequestMemoryReservation>,
    ) -> Result<(), QoderSseDecodeError> {
        let text = std::str::from_utf8(event)
            .map_err(|_| ProxyError::bad_gateway("Qoder SSE event is not UTF-8"))?;
        let mut data_lines = Vec::new();
        for line in text.split(['\r', '\n']) {
            if let Some(data) = line.strip_prefix("data:") {
                data_lines.push(data.trim_start());
            } else if !line.is_empty() && !line.starts_with(':') {
                return Err(QoderSseDecodeError::Protocol(ProxyError::bad_gateway(
                    "Qoder SSE contains an unsupported field",
                )));
            }
        }
        if data_lines.is_empty() {
            return Ok(());
        }
        if data_lines.len() != 1 {
            return Err(QoderSseDecodeError::Protocol(ProxyError::bad_gateway(
                "Qoder SSE wrapper must contain exactly one data line",
            )));
        }
        let data = data_lines[0];
        if data == "[DONE]" {
            return self.mark_terminal();
        }
        if self.saw_terminal {
            crate::metrics::record_qoder_error("data_after_terminal");
            return Err(QoderSseDecodeError::Protocol(ProxyError::bad_gateway(
                "Qoder SSE emitted data after its terminal event",
            )));
        }
        let wrapper = serde_json::from_str::<Value>(data).map_err(|error| {
            ProxyError::bad_gateway(format!("invalid Qoder SSE wrapper: {error}"))
        })?;
        if !wrapper.is_object() {
            return Err(QoderSseDecodeError::Protocol(ProxyError::bad_gateway(
                "Qoder SSE wrapper must be an object",
            )));
        }
        let status = qoder_wrapper_status(&wrapper)?;
        let inner = match wrapper.get("body") {
            None => "",
            Some(Value::String(body)) => body.as_str(),
            Some(_) => {
                return Err(QoderSseDecodeError::Protocol(ProxyError::bad_gateway(
                    "Qoder SSE wrapper body must be a string",
                )))
            }
        };
        if status >= 400 {
            return Err(QoderSseDecodeError::Upstream(
                QoderUpstreamError::from_status_body(status, inner.as_bytes()),
            ));
        }
        if inner.is_empty() {
            return Ok(());
        }
        if inner == "[DONE]" {
            return self.mark_terminal();
        }
        let value = serde_json::from_str::<Value>(inner)
            .map_err(|error| ProxyError::bad_gateway(format!("invalid Qoder SSE body: {error}")))?;
        if value.get("error").is_some_and(|error| !error.is_null()) {
            return Err(QoderSseDecodeError::Upstream(
                QoderUpstreamError::from_status_body(502, inner.as_bytes()),
            ));
        }
        let mut projected = canonicalize_qoder_inner(&value)?;
        let is_message_stop = matches!(
            value
                .get("event")
                .or_else(|| value.get("type"))
                .and_then(Value::as_str),
            Some("message_stop")
        );
        if is_message_stop && self.saw_meaningful && !self.saw_finish_reason {
            let finish_reason = if self.saw_tool_calls {
                "tool_calls"
            } else {
                "stop"
            };
            projected.push(qoder_chat_chunk(
                &value,
                json!([{"index":0,"delta":{},"finish_reason":finish_reason}]),
                None,
            ));
        }
        for mut canonical in projected {
            let (meaningful, finish_reason, tool_calls) = qoder_canonical_semantics(&canonical);
            if !meaningful {
                continue;
            }
            self.saw_meaningful = true;
            self.saw_finish_reason |= finish_reason;
            self.saw_tool_calls |= tool_calls;
            if let Some(id) = self.canonical_chat_id.as_ref() {
                canonical["id"] = Value::String(id.clone());
            }
            if let Some(timing) = self
                .request_memory
                .as_ref()
                .and_then(RequestMemoryBudget::timing)
            {
                timing.observe_chat_value(&canonical);
            }
            let canonical = serde_json::to_vec(&canonical).map_err(|error| {
                ProxyError::bad_gateway(format!("encode Qoder SSE body: {error}"))
            })?;
            reserve_vec_append(
                output_memory,
                output,
                b"data: "
                    .len()
                    .saturating_add(canonical.len())
                    .saturating_add(b"\n\n".len()),
            )?;
            output.extend_from_slice(b"data: ");
            output.extend_from_slice(&canonical);
            output.extend_from_slice(b"\n\n");
        }
        if qoder_inner_is_terminal(&value) {
            self.mark_terminal()?;
        }
        Ok(())
    }

    fn reserve_buffer_append(&self, additional: usize) -> Result<(), QoderSseDecodeError> {
        if let Some(memory) = self.buffer_memory.as_ref() {
            memory
                .resize(projected_vec_capacity(
                    self.buffer.capacity(),
                    self.buffer.len(),
                    additional,
                ))
                .map_err(|error| QoderSseDecodeError::Protocol(error.into_proxy_error()))?;
        }
        Ok(())
    }

    fn sync_buffer_memory(&self) -> Result<(), QoderSseDecodeError> {
        if let Some(memory) = self.buffer_memory.as_ref() {
            memory
                .resize(self.retained_bytes())
                .map_err(|error| QoderSseDecodeError::Protocol(error.into_proxy_error()))?;
        }
        Ok(())
    }

    fn reserve_event_working_set(
        &self,
        event_bytes: usize,
    ) -> Result<Option<RequestMemoryReservation>, QoderSseDecodeError> {
        self.request_memory
            .as_ref()
            .map(|budget| {
                budget.reserve(
                    RequestMemoryComponent::NormalizedEvent,
                    event_bytes.saturating_mul(8),
                )
            })
            .transpose()
            .map_err(|error| QoderSseDecodeError::Protocol(error.into_proxy_error()))
    }

    fn mark_terminal(&mut self) -> Result<(), QoderSseDecodeError> {
        if self.saw_terminal {
            crate::metrics::record_qoder_error("terminal_duplicate");
            return Err(QoderSseDecodeError::Protocol(ProxyError::bad_gateway(
                "Qoder SSE emitted a second terminal event",
            )));
        }
        self.saw_terminal = true;
        Ok(())
    }
}

fn qoder_canonical_semantics(value: &Value) -> (bool, bool, bool) {
    let mut meaningful = value.get("usage").and_then(Value::as_object).is_some();
    let mut saw_finish = false;
    let mut saw_tools = false;
    if let Some(choices) = value.get("choices").and_then(Value::as_array) {
        for choice in choices {
            if choice
                .get("finish_reason")
                .and_then(Value::as_str)
                .is_some_and(|reason| !reason.trim().is_empty())
            {
                saw_finish = true;
                meaningful = true;
            }
            if let Some(delta) = choice.get("delta").and_then(Value::as_object) {
                meaningful |= [
                    "content",
                    "reasoning_content",
                    "reasoning_text",
                    "reasoning",
                ]
                .iter()
                .any(|name| {
                    delta
                        .get(*name)
                        .and_then(Value::as_str)
                        .is_some_and(|text| !text.is_empty())
                });
                if delta
                    .get("tool_calls")
                    .and_then(Value::as_array)
                    .is_some_and(|calls| !calls.is_empty())
                {
                    meaningful = true;
                    saw_tools = true;
                }
            }
        }
    }
    (meaningful, saw_finish, saw_tools)
}

fn projected_vec_capacity(current_capacity: usize, current_len: usize, additional: usize) -> usize {
    let required = current_len.saturating_add(additional);
    if required <= current_capacity {
        current_capacity
    } else {
        current_capacity.saturating_mul(2).max(required).max(8)
    }
}

fn reserve_vec_append(
    memory: Option<&RequestMemoryReservation>,
    output: &Vec<u8>,
    additional: usize,
) -> Result<(), QoderSseDecodeError> {
    if let Some(memory) = memory {
        memory
            .resize(projected_vec_capacity(
                output.capacity(),
                output.len(),
                additional,
            ))
            .map_err(|error| QoderSseDecodeError::Protocol(error.into_proxy_error()))?;
    }
    Ok(())
}

fn canonicalize_qoder_inner(value: &Value) -> Result<Vec<Value>, ProxyError> {
    if value.get("choices").is_some() {
        return Ok(vec![canonicalize_openai_chat_chunk(value)?]);
    }
    let event = value
        .get("event")
        .or_else(|| value.get("type"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let data = value
        .get("data")
        .or_else(|| value.get("delta"))
        .unwrap_or(&Value::Null);
    let chunk = match event {
        "content_block_delta" => {
            let kind = data.get("type").and_then(Value::as_str).unwrap_or_default();
            if kind == "input_json_delta" {
                let index = qoder_tool_call_index(value.get("index"), 0)?;
                let Some(call) = canonical_qoder_tool_call(data, index)? else {
                    return Ok(Vec::new());
                };
                crate::metrics::record_qoder_compatibility("tool_use_delta");
                return Ok(vec![qoder_chat_chunk(
                    value,
                    json!([{"index":0,"delta":{"tool_calls":[call]},"finish_reason":null}]),
                    None,
                )]);
            }
            let text = qoder_optional_semantic_text(data.get("text"), "text")?.unwrap_or_default();
            if text.is_empty() {
                return Ok(Vec::new());
            }
            crate::metrics::record_qoder_compatibility("content_block_delta");
            let delta = if kind == "thinking_delta" {
                json!({"reasoning_content": text})
            } else {
                json!({"content": text})
            };
            qoder_chat_chunk(
                value,
                json!([{"index":0,"delta":delta,"finish_reason":null}]),
                None,
            )
        }
        "content_block_start" | "tool_use_start" | "tool_use_delta" | "tool_call_delta" => {
            let index = qoder_tool_call_index(value.get("index"), 0)?;
            let tool = value
                .get("content_block")
                .or_else(|| value.get("data"))
                .or_else(|| value.get("delta"))
                .unwrap_or(&Value::Null);
            if event == "content_block_start"
                && tool.get("type").and_then(Value::as_str) != Some("tool_use")
            {
                return Ok(Vec::new());
            }
            let Some(call) = canonical_qoder_tool_call(tool, index)? else {
                return Ok(Vec::new());
            };
            crate::metrics::record_qoder_compatibility(if event == "tool_use_start" {
                "tool_use_start"
            } else if event == "content_block_start" {
                "content_block_start"
            } else {
                "tool_use_delta"
            });
            qoder_chat_chunk(
                value,
                json!([{"index":0,"delta":{"tool_calls":[call]},"finish_reason":null}]),
                None,
            )
        }
        "message_delta" => {
            let usage = canonical_qoder_usage(value.get("usage").or_else(|| data.get("usage")))?;
            let finish_reason = data
                .get("stop_reason")
                .and_then(Value::as_str)
                .and_then(qoder_finish_reason);
            if usage.is_none() && finish_reason.is_none() {
                return Ok(Vec::new());
            }
            crate::metrics::record_qoder_compatibility("message_delta_usage");
            let choices = finish_reason.map_or_else(
                || json!([]),
                |reason| json!([{"index":0,"delta":{},"finish_reason":reason}]),
            );
            qoder_chat_chunk(value, choices, usage)
        }
        "message_start" => {
            let usage = canonical_qoder_usage(
                value
                    .pointer("/message/usage")
                    .or_else(|| data.get("usage")),
            )?;
            if usage.is_none() {
                return Ok(Vec::new());
            }
            crate::metrics::record_qoder_compatibility("message_start_usage");
            qoder_chat_chunk(value, json!([]), usage)
        }
        "message_stop" => {
            crate::metrics::record_qoder_compatibility("message_stop");
            qoder_chat_chunk(value, json!([]), None)
        }
        "content_block_stop" | "ping" => {
            crate::metrics::record_qoder_compatibility("control_event_dropped");
            return Ok(Vec::new());
        }
        _ if value.get("usage").is_some() => {
            qoder_chat_chunk(value, json!([]), canonical_qoder_usage(value.get("usage"))?)
        }
        _ => value.clone(),
    };
    Ok(vec![chunk])
}

fn canonicalize_openai_chat_chunk(value: &Value) -> Result<Value, ProxyError> {
    let mut output = value
        .as_object()
        .cloned()
        .ok_or_else(|| ProxyError::bad_gateway("Qoder Chat chunk must be an object"))?;
    let choices = value
        .get("choices")
        .and_then(Value::as_array)
        .ok_or_else(|| ProxyError::bad_gateway("Qoder Chat choices must be an array"))?;
    if choices.len() > 1 {
        return Err(ProxyError::bad_gateway(
            "Qoder Chat returned multiple choices for one response",
        ));
    }
    let mut normalized = Vec::with_capacity(choices.len());
    for choice in choices {
        let object = choice
            .as_object()
            .ok_or_else(|| ProxyError::bad_gateway("Qoder Chat choice must be an object"))?;
        let index = qoder_choice_index(object)?;
        if let Some(reason) = object.get("finish_reason").filter(|value| !value.is_null()) {
            if reason
                .as_str()
                .is_none_or(|reason| reason.trim().is_empty())
            {
                return Err(ProxyError::bad_gateway(
                    "Qoder Chat finish_reason must be a non-empty string or null",
                ));
            }
        }
        let source = object
            .get("delta")
            .or_else(|| object.get("message"))
            .unwrap_or(&Value::Null);
        let delta = canonical_qoder_message_delta(source)?;
        let mut target = object.clone();
        target.insert("index".to_string(), Value::Number(index.into()));
        target.insert("delta".to_string(), delta);
        target.remove("message");
        normalized.push(Value::Object(target));
    }
    output.insert("choices".to_string(), Value::Array(normalized));
    if let Some(usage) = canonical_qoder_usage(value.get("usage"))? {
        output.insert("usage".to_string(), usage);
    }
    Ok(Value::Object(output))
}

fn canonical_qoder_message_delta(value: &Value) -> Result<Value, ProxyError> {
    if value.is_null() {
        return Ok(json!({}));
    }
    let object = value
        .as_object()
        .ok_or_else(|| ProxyError::bad_gateway("Qoder Chat delta/message must be an object"))?;
    let mut delta = object.clone();
    if let Some(content) = canonical_qoder_content(object.get("content"))? {
        delta.insert("content".to_string(), Value::String(content));
    }
    for key in ["reasoning_content", "reasoning_text", "reasoning"] {
        let _ = qoder_optional_semantic_text(object.get(key), key)?;
    }
    if let Some(reasoning) = qoder_optional_semantic_text(
        ["reasoning_content", "reasoning_text", "reasoning"]
            .into_iter()
            .find_map(|key| object.get(key).filter(|value| !value.is_null())),
        "reasoning",
    )? {
        delta.insert(
            "reasoning_content".to_string(),
            Value::String(reasoning.to_string()),
        );
    }
    if let Some(calls) = object.get("tool_calls") {
        let calls = calls
            .as_array()
            .ok_or_else(|| ProxyError::bad_gateway("Qoder Chat tool_calls must be an array"))?;
        let mut normalized = Vec::new();
        for (position, call) in calls.iter().enumerate() {
            let index = qoder_tool_call_index(call.get("index"), position)?;
            if let Some(call) = canonical_qoder_tool_call(call, index)? {
                normalized.push(call);
            }
        }
        delta.insert("tool_calls".to_string(), Value::Array(normalized));
    }
    Ok(Value::Object(delta))
}

fn canonical_qoder_content(value: Option<&Value>) -> Result<Option<String>, ProxyError> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(Value::Array(parts)) => {
            let mut text = Vec::new();
            for part in parts {
                let object = part.as_object().ok_or_else(|| {
                    ProxyError::bad_gateway("Qoder Chat content block must be an object")
                })?;
                if object.get("type").and_then(Value::as_str) == Some("text") {
                    if let Some(value) =
                        qoder_optional_semantic_text(object.get("text"), "content block text")?
                    {
                        text.push(value);
                    }
                }
            }
            crate::metrics::record_qoder_compatibility("message_content_blocks");
            Ok(Some(text.join("\n")))
        }
        Some(_) => Err(ProxyError::bad_gateway(
            "Qoder Chat content must be text, blocks, or null",
        )),
    }
}

fn qoder_optional_semantic_text<'a>(
    value: Option<&'a Value>,
    field: &str,
) -> Result<Option<&'a str>, ProxyError> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value)),
        Some(_) => Err(ProxyError::bad_gateway(format!(
            "Qoder Chat {field} must be a string or null"
        ))),
    }
}

fn canonical_qoder_tool_call(value: &Value, index: usize) -> Result<Option<Value>, ProxyError> {
    let object = value
        .as_object()
        .ok_or_else(|| ProxyError::bad_gateway("Qoder Chat tool call must be an object"))?;
    let function = object.get("function").and_then(Value::as_object);
    let id = qoder_consistent_tool_alias(
        [
            object.get("id"),
            object.get("tool_call_id"),
            object.get("call_id"),
        ],
        "tool call id",
        true,
    )?;
    let name = qoder_consistent_tool_alias(
        [
            function.and_then(|function| function.get("name")),
            object.get("name"),
            object.get("tool_name"),
        ],
        "tool name",
        false,
    )?;
    let argument_value = function
        .and_then(|function| function.get("arguments"))
        .or_else(|| object.get("arguments"))
        .or_else(|| object.get("input"))
        .or_else(|| object.get("parameters"))
        .or_else(|| object.get("partial_json"));
    let empty_tool_use_start = object.get("type").and_then(Value::as_str) == Some("tool_use")
        && argument_value
            .is_some_and(|value| value.as_object().is_some_and(|value| value.is_empty()));
    let arguments = if empty_tool_use_start {
        None
    } else {
        argument_value.map(qoder_argument_fragment).transpose()?
    };
    if id.is_none() && name.is_none() && arguments.as_deref().is_none_or(str::is_empty) {
        crate::metrics::record_qoder_compatibility("placeholder_tool_call_dropped");
        return Ok(None);
    }
    let call_type = object
        .get("type")
        .and_then(Value::as_str)
        .map(|value| match value {
            "" | "tool_use" | "tool_call" | "input_json_delta" => "function",
            value => value,
        })
        .unwrap_or("function");
    let mut call = json!({
        "index": index,
        "type": call_type,
        "function": {
            "name": name.unwrap_or_default(),
            "arguments": arguments.unwrap_or_default()
        }
    });
    if let Some(id) = id {
        call["id"] = Value::String(id.to_string());
    }
    Ok(Some(call))
}

fn qoder_argument_fragment(value: &Value) -> Result<String, ProxyError> {
    match value {
        Value::Null => Ok(String::new()),
        Value::String(value) => Ok(value.clone()),
        value => serde_json::to_string(value)
            .map_err(|error| ProxyError::bad_gateway(format!("encode Qoder tool input: {error}"))),
    }
}

fn qoder_finish_reason(value: &str) -> Option<&'static str> {
    match value.trim() {
        "end_turn" | "stop_sequence" | "stop" => Some("stop"),
        "tool_use" | "tool_calls" => Some("tool_calls"),
        "max_tokens" | "length" => Some("length"),
        _ => None,
    }
}

fn qoder_choice_index(object: &serde_json::Map<String, Value>) -> Result<i64, ProxyError> {
    let index = object
        .get("index")
        .map(integer_value)
        .unwrap_or(Some(0))
        .ok_or_else(|| ProxyError::bad_gateway("Qoder Chat choice index must be an integer"))?;
    if index != 0 {
        return Err(ProxyError::bad_gateway(
            "Qoder Chat returned an unsupported choice index",
        ));
    }
    Ok(index)
}

fn qoder_tool_call_index(value: Option<&Value>, default: usize) -> Result<usize, ProxyError> {
    let index = value.map_or(Ok(default), |value| {
        integer_value(value)
            .and_then(|index| usize::try_from(index).ok())
            .ok_or_else(|| {
                ProxyError::bad_gateway("Qoder Chat tool call index must be a non-negative integer")
            })
    })?;
    if index >= MAX_QODER_CHAT_TOOL_CALLS {
        return Err(ProxyError::bad_gateway(
            "Qoder Chat SSE tool call index exceeds the limit",
        ));
    }
    Ok(index)
}

fn qoder_consistent_tool_alias<'a>(
    values: impl IntoIterator<Item = Option<&'a Value>>,
    field: &str,
    preserve_space: bool,
) -> Result<Option<&'a str>, ProxyError> {
    let mut selected = None;
    for value in values.into_iter().flatten() {
        let raw = match value {
            Value::Null => continue,
            Value::String(raw) => raw.as_str(),
            _ => {
                return Err(ProxyError::bad_gateway(format!(
                    "Qoder Chat {field} must be a string or null"
                )))
            }
        };
        if raw.trim().is_empty() {
            continue;
        }
        let candidate = if preserve_space { raw } else { raw.trim() };
        if selected.is_some_and(|selected| selected != candidate) {
            return Err(ProxyError::bad_gateway(format!(
                "Qoder Chat {field} aliases disagree"
            )));
        }
        selected = Some(candidate);
    }
    Ok(selected)
}

fn canonical_qoder_usage(value: Option<&Value>) -> Result<Option<Value>, ProxyError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let object = value
        .as_object()
        .ok_or_else(|| ProxyError::bad_gateway("Qoder Chat usage must be an object"))?;
    let prompt = qoder_token_value(object, &["prompt_tokens", "input_tokens"])?.unwrap_or(0);
    let completion =
        qoder_token_value(object, &["completion_tokens", "output_tokens"])?.unwrap_or(0);
    let minimum_total = qoder_usage_sum(prompt, completion)?;
    let total = qoder_token_value(object, &["total_tokens"])?.unwrap_or(minimum_total);
    if total < minimum_total {
        return Err(ProxyError::bad_gateway(
            "Qoder Chat usage total is smaller than input plus output",
        ));
    }
    let mut usage = json!({
        "prompt_tokens": prompt,
        "completion_tokens": completion,
        "total_tokens": total
    });
    for key in ["prompt_tokens_details", "completion_tokens_details"] {
        if let Some(details) = object.get(key).filter(|value| value.is_object()) {
            usage[key] = details.clone();
        }
    }
    Ok(Some(usage))
}

fn qoder_usage_sum(prompt: u64, completion: u64) -> Result<u64, ProxyError> {
    prompt
        .checked_add(completion)
        .ok_or_else(|| ProxyError::bad_gateway("Qoder Chat usage token sum exceeds the limit"))
}

fn merge_qoder_usage(current: Option<Value>, incoming: &Value) -> Result<Value, ProxyError> {
    let Some(current) = current else {
        return Ok(incoming.clone());
    };
    let prompt = current
        .get("prompt_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0)
        .max(
            incoming
                .get("prompt_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
        );
    let completion = current
        .get("completion_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0)
        .max(
            incoming
                .get("completion_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
        );
    let total = current
        .get("total_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0)
        .max(
            incoming
                .get("total_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
        )
        .max(qoder_usage_sum(prompt, completion)?);
    let mut merged = incoming.as_object().cloned().unwrap_or_default();
    merged.insert("prompt_tokens".to_string(), Value::Number(prompt.into()));
    merged.insert(
        "completion_tokens".to_string(),
        Value::Number(completion.into()),
    );
    merged.insert("total_tokens".to_string(), Value::Number(total.into()));
    Ok(Value::Object(merged))
}

fn qoder_token_value(
    object: &serde_json::Map<String, Value>,
    keys: &[&str],
) -> Result<Option<u64>, ProxyError> {
    let mut selected = None;
    for key in keys {
        let Some(value) = object.get(*key) else {
            continue;
        };
        let parsed = match value {
            Value::Number(value) => value
                .as_u64()
                .or_else(|| value.as_f64().and_then(qoder_float_token)),
            Value::String(value) => value
                .trim()
                .parse::<u64>()
                .ok()
                .or_else(|| value.trim().parse::<f64>().ok().and_then(qoder_float_token)),
            _ => None,
        };
        let parsed = parsed.ok_or_else(|| {
            ProxyError::bad_gateway("Qoder Chat usage has an invalid token count")
        })?;
        if selected.is_none() || (selected == Some(0) && parsed > 0) {
            selected = Some(parsed);
        }
    }
    Ok(selected)
}

fn qoder_float_token(value: f64) -> Option<u64> {
    (value.is_finite() && (0.0..18_446_744_073_709_551_616.0).contains(&value))
        .then_some(value as u64)
}

fn qoder_chat_chunk(value: &Value, choices: Value, usage: Option<Value>) -> Value {
    let mut chunk = serde_json::Map::new();
    for key in ["id", "created", "model"] {
        if let Some(field) = value.get(key) {
            chunk.insert(key.to_string(), field.clone());
        }
    }
    chunk.insert(
        "object".to_string(),
        Value::String("chat.completion.chunk".to_string()),
    );
    chunk.insert("choices".to_string(), choices);
    if let Some(usage) = usage {
        chunk.insert("usage".to_string(), usage);
    }
    Value::Object(chunk)
}

#[derive(Debug, Default)]
pub struct QoderChatSseAggregator {
    buffer: Vec<u8>,
    aggregate_bytes: usize,
    id: Option<String>,
    created: Option<i64>,
    model: Option<String>,
    content: String,
    reasoning_content: String,
    tool_calls: BTreeMap<usize, QoderAggregatedToolCall>,
    finish_reason: Option<Value>,
    usage: Option<Value>,
    terminal: bool,
    failed: bool,
    request_memory: Option<RequestMemoryBudget>,
    retained_memory: Option<RequestMemoryReservation>,
}

#[derive(Debug, Default)]
struct QoderAggregatedToolCall {
    id: String,
    call_type: String,
    name: String,
    arguments: String,
}

#[derive(Debug)]
pub(crate) struct QoderAggregatedResponse {
    pub(crate) value: Value,
    pub(crate) memory: Option<RequestMemoryReservation>,
}

impl QoderChatSseAggregator {
    pub(crate) fn with_request_memory(
        request_memory: Option<RequestMemoryBudget>,
    ) -> Result<Self, ProxyError> {
        let retained_memory = request_memory
            .as_ref()
            .map(|budget| budget.reserve(RequestMemoryComponent::StreamRetainedState, 0))
            .transpose()
            .map_err(|error| error.into_proxy_error())?;
        Ok(Self {
            request_memory,
            retained_memory,
            ..Self::default()
        })
    }

    pub(crate) fn retained_bytes(&self) -> usize {
        let tool_calls = self.tool_calls.iter().fold(0_usize, |bytes, (_, call)| {
            bytes
                .saturating_add(std::mem::size_of::<(usize, QoderAggregatedToolCall)>())
                .saturating_add(std::mem::size_of::<usize>() * 3)
                .saturating_add(call.id.capacity())
                .saturating_add(call.call_type.capacity())
                .saturating_add(call.name.capacity())
                .saturating_add(call.arguments.capacity())
        });
        self.buffer
            .capacity()
            .saturating_add(self.id.as_ref().map_or(0, String::capacity))
            .saturating_add(self.model.as_ref().map_or(0, String::capacity))
            .saturating_add(self.content.capacity())
            .saturating_add(self.reasoning_content.capacity())
            .saturating_add(tool_calls)
            .saturating_add(self.finish_reason.as_ref().map_or(0, retained_json_bytes))
            .saturating_add(self.usage.as_ref().map_or(0, retained_json_bytes))
    }

    pub fn push(&mut self, chunk: Bytes) -> Result<(), ProxyError> {
        if self.failed {
            return Err(ProxyError::bad_gateway(
                "Qoder Chat SSE aggregator has failed",
            ));
        }
        let result = self.push_inner(chunk);
        self.failed |= result.is_err();
        result
    }

    fn push_inner(&mut self, chunk: Bytes) -> Result<(), ProxyError> {
        if chunk.is_empty() {
            return Ok(());
        }
        self.aggregate_bytes = self.aggregate_bytes.saturating_add(chunk.len());
        if self.aggregate_bytes > MAX_QODER_CHAT_AGGREGATE_BYTES {
            return Err(ProxyError::bad_gateway(
                "Qoder Chat response exceeds the aggregate limit",
            ));
        }
        self.reserve_buffer_append(chunk.len())?;
        self.buffer.extend_from_slice(&chunk);
        self.drain(false)?;
        self.sync_retained_memory()
    }

    pub fn finish(self, fallback_model: &str, now_unix_seconds: i64) -> Result<Value, ProxyError> {
        self.finish_with_memory(fallback_model, now_unix_seconds)
            .map(|response| response.value)
    }

    pub(crate) fn finish_with_memory(
        mut self,
        fallback_model: &str,
        now_unix_seconds: i64,
    ) -> Result<QoderAggregatedResponse, ProxyError> {
        if self.failed {
            return Err(ProxyError::bad_gateway(
                "Qoder Chat SSE aggregator has failed",
            ));
        }
        self.drain(true)?;
        if !self.terminal {
            return Err(ProxyError::bad_gateway(
                "Qoder Chat SSE ended without [DONE]",
            ));
        }
        if self.finish_reason.is_none()
            && self.content.is_empty()
            && self.reasoning_content.is_empty()
            && self.tool_calls.is_empty()
        {
            return Err(ProxyError::bad_gateway(
                "Qoder Chat SSE ended without meaningful output",
            ));
        }
        if self
            .content
            .len()
            .saturating_add(self.reasoning_content.len())
            > MAX_QODER_CHAT_TEXT_BYTES
        {
            return Err(ProxyError::bad_gateway(
                "Qoder Chat response text exceeds the limit",
            ));
        }

        if let Some(memory) = self.retained_memory.as_ref() {
            memory
                .resize(self.retained_bytes().saturating_mul(3))
                .map_err(|error| error.into_proxy_error())?;
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
                        "Qoder Chat tool call {index} is missing id or function name"
                    )));
                }
                if serde_json::from_str::<Value>(&call.arguments).is_err() {
                    return Err(ProxyError::bad_gateway(format!(
                        "Qoder Chat tool call {index} has invalid JSON arguments"
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
            "id": self.id.unwrap_or_else(|| format!("chatcmpl-qoder-{now_unix_seconds}")),
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
        if let Some(memory) = self.retained_memory.as_ref() {
            memory
                .resize(retained_json_bytes(&response))
                .map_err(|error| error.into_proxy_error())?;
        }
        Ok(QoderAggregatedResponse {
            value: response,
            memory: self.retained_memory.take(),
        })
    }

    fn drain(&mut self, finish: bool) -> Result<(), ProxyError> {
        while let Some((end, delimiter)) = next_event_boundary(&self.buffer) {
            if end > MAX_QODER_SSE_EVENT_BYTES {
                return Err(ProxyError::bad_gateway(
                    "Qoder Chat SSE event exceeds the limit",
                ));
            }
            let working_memory = self.reserve_event_working_set(end)?;
            let event = self.buffer[..end].to_vec();
            self.buffer.drain(..end + delimiter);
            self.consume_event(&event)?;
            drop(working_memory);
            self.sync_retained_memory()?;
        }
        if self.buffer.len() > MAX_QODER_SSE_EVENT_BYTES {
            return Err(ProxyError::bad_gateway(
                "Qoder Chat SSE event exceeds the limit",
            ));
        }
        if finish && !self.buffer.is_empty() {
            let event_len = self.buffer.len();
            let working_memory = self.reserve_event_working_set(event_len)?;
            let event = self.buffer[..event_len].to_vec();
            self.buffer.clear();
            self.consume_event(&event)?;
            drop(working_memory);
            self.sync_retained_memory()?;
        }
        Ok(())
    }

    fn consume_event(&mut self, event: &[u8]) -> Result<(), ProxyError> {
        let text = std::str::from_utf8(event)
            .map_err(|_| ProxyError::bad_gateway("Qoder Chat SSE event is not UTF-8"))?;
        let mut data = None;
        for line in text.split(['\r', '\n']) {
            if let Some(value) = line.strip_prefix("data:") {
                if data.is_some() {
                    return Err(ProxyError::bad_gateway(
                        "Qoder Chat SSE event contains multiple data lines",
                    ));
                }
                data = Some(value.trim_start());
            } else if !line.is_empty() && !line.starts_with(':') && !line.starts_with("event:") {
                return Err(ProxyError::bad_gateway(
                    "Qoder Chat SSE contains an unsupported field",
                ));
            }
        }
        let Some(data) = data else {
            return Ok(());
        };
        if data == "[DONE]" {
            if self.terminal {
                return Err(ProxyError::bad_gateway(
                    "Qoder Chat SSE emitted a second [DONE] event",
                ));
            }
            self.terminal = true;
            return Ok(());
        }
        if self.terminal {
            return Err(ProxyError::bad_gateway(
                "Qoder Chat SSE emitted data after [DONE]",
            ));
        }
        let value = serde_json::from_str::<Value>(data).map_err(|error| {
            ProxyError::bad_gateway(format!("invalid Qoder Chat SSE chunk: {error}"))
        })?;
        let object = value
            .as_object()
            .ok_or_else(|| ProxyError::bad_gateway("Qoder Chat SSE chunk must be an object"))?;
        update_optional_string(&mut self.id, object.get("id"), "id")?;
        update_optional_string(&mut self.model, object.get("model"), "model")?;
        if let Some(created) = object.get("created") {
            let created = integer_value(created).ok_or_else(|| {
                ProxyError::bad_gateway("Qoder Chat SSE created must be an integer")
            })?;
            if self.created.is_some_and(|current| current != created) {
                return Err(ProxyError::bad_gateway(
                    "Qoder Chat SSE changed created across chunks",
                ));
            }
            self.created = Some(created);
        }
        if let Some(usage) = object.get("usage") {
            let canonical = canonical_qoder_usage(Some(usage))?
                .ok_or_else(|| ProxyError::bad_gateway("Qoder Chat SSE usage is missing"))?;
            self.usage = Some(merge_qoder_usage(self.usage.take(), &canonical)?);
        }
        let choices = object
            .get("choices")
            .and_then(Value::as_array)
            .ok_or_else(|| ProxyError::bad_gateway("Qoder Chat SSE choices must be an array"))?;
        if choices.len() > 1 {
            return Err(ProxyError::bad_gateway(
                "Qoder Chat returned multiple choices for one response",
            ));
        }
        for choice in choices {
            let choice = choice.as_object().ok_or_else(|| {
                ProxyError::bad_gateway("Qoder Chat SSE choice must be an object")
            })?;
            qoder_choice_index(choice)?;
            if let Some(reason) = choice.get("finish_reason").filter(|value| !value.is_null()) {
                let reason = reason
                    .as_str()
                    .filter(|reason| !reason.trim().is_empty())
                    .ok_or_else(|| {
                        ProxyError::bad_gateway(
                            "Qoder Chat SSE finish_reason must be a non-empty string or null",
                        )
                    })?;
                self.finish_reason = Some(Value::String(reason.to_string()));
            }
            let Some(delta) = choice.get("delta") else {
                continue;
            };
            let delta = delta
                .as_object()
                .ok_or_else(|| ProxyError::bad_gateway("Qoder Chat SSE delta must be an object"))?;
            append_delta_text(&mut self.content, delta.get("content"), "content")?;
            let reasoning = ["reasoning_content", "reasoning_text", "reasoning"]
                .into_iter()
                .find_map(|key| delta.get(key).filter(|value| !value.is_null()));
            append_delta_text(&mut self.reasoning_content, reasoning, "reasoning_content")?;
            if let Some(calls) = delta.get("tool_calls") {
                let calls = calls.as_array().ok_or_else(|| {
                    ProxyError::bad_gateway("Qoder Chat SSE tool_calls must be an array")
                })?;
                for call in calls {
                    let call = call.as_object().ok_or_else(|| {
                        ProxyError::bad_gateway("Qoder Chat SSE tool call must be an object")
                    })?;
                    let index = qoder_tool_call_index(call.get("index"), 0)?;
                    let target = self.tool_calls.entry(index).or_default();
                    append_stable_field(&mut target.id, call.get("id"), "tool call id")?;
                    append_stable_field(&mut target.call_type, call.get("type"), "tool call type")?;
                    if let Some(function) = call.get("function") {
                        let function = function.as_object().ok_or_else(|| {
                            ProxyError::bad_gateway(
                                "Qoder Chat SSE tool function must be an object",
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

    fn reserve_buffer_append(&self, additional: usize) -> Result<(), ProxyError> {
        if let Some(memory) = self.retained_memory.as_ref() {
            let next_buffer_capacity =
                projected_vec_capacity(self.buffer.capacity(), self.buffer.len(), additional);
            memory
                .resize(
                    self.retained_bytes()
                        .saturating_sub(self.buffer.capacity())
                        .saturating_add(next_buffer_capacity),
                )
                .map_err(|error| error.into_proxy_error())?;
        }
        Ok(())
    }

    fn sync_retained_memory(&self) -> Result<(), ProxyError> {
        if let Some(memory) = self.retained_memory.as_ref() {
            memory
                .resize(self.retained_bytes())
                .map_err(|error| error.into_proxy_error())?;
        }
        Ok(())
    }

    fn reserve_event_working_set(
        &self,
        event_bytes: usize,
    ) -> Result<Option<RequestMemoryReservation>, ProxyError> {
        self.request_memory
            .as_ref()
            .map(|budget| {
                budget.reserve(
                    RequestMemoryComponent::NormalizedEvent,
                    event_bytes.saturating_mul(8),
                )
            })
            .transpose()
            .map_err(|error| error.into_proxy_error())
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
        ProxyError::bad_gateway(format!("Qoder Chat SSE {field} must be a string"))
    })?;
    if target.as_deref().is_some_and(|current| current != value) {
        return Err(ProxyError::bad_gateway(format!(
            "Qoder Chat SSE changed {field} across chunks"
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
        ProxyError::bad_gateway(format!("Qoder Chat SSE {field} must be a string"))
    })?;
    target.push_str(value);
    if target.len() > MAX_QODER_CHAT_TEXT_BYTES {
        return Err(ProxyError::bad_gateway(format!(
            "Qoder Chat SSE {field} exceeds the limit"
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
        ProxyError::bad_gateway(format!("Qoder Chat SSE {field} must be a string"))
    })?;
    if !target.is_empty() && target != value {
        return Err(ProxyError::bad_gateway(format!(
            "Qoder Chat SSE changed {field} across chunks"
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
    let value = value.as_str().ok_or_else(|| {
        ProxyError::bad_gateway(format!("Qoder Chat SSE {field} must be a string"))
    })?;
    target.push_str(value);
    if target.len() > MAX_QODER_CHAT_TEXT_BYTES {
        return Err(ProxyError::bad_gateway(format!(
            "Qoder Chat SSE {field} exceeds the limit"
        )));
    }
    Ok(())
}

fn qoder_inner_is_terminal(value: &Value) -> bool {
    if matches!(
        value
            .get("event")
            .or_else(|| value.get("type"))
            .and_then(Value::as_str),
        Some("message_stop")
    ) {
        return true;
    }
    value
        .get("choices")
        .and_then(Value::as_array)
        .is_some_and(|choices| {
            choices.iter().any(|choice| {
                choice.get("index").and_then(integer_value).unwrap_or(0) == 0
                    && choice
                        .get("finish_reason")
                        .and_then(Value::as_str)
                        .is_some_and(|reason| !reason.trim().is_empty())
            })
        })
}

fn next_event_boundary(buffer: &[u8]) -> Option<(usize, usize)> {
    let mut index = 0;
    while index < buffer.len() {
        let first = match buffer[index] {
            b'\r' if buffer.get(index + 1) == Some(&b'\n') => 2,
            b'\r' | b'\n' => 1,
            _ => {
                index += 1;
                continue;
            }
        };
        let next = index + first;
        let second = match buffer.get(next) {
            Some(b'\r') if buffer.get(next + 1) == Some(&b'\n') => 2,
            Some(b'\r' | b'\n') => 1,
            _ => {
                index = next;
                continue;
            }
        };
        return Some((index, first + second));
    }
    None
}

fn qoder_wrapper_status(wrapper: &Value) -> Result<u16, ProxyError> {
    let value_status = wrapper
        .get("statusCodeValue")
        .map(|value| {
            let parsed = match value {
                Value::String(text) => text.trim().parse::<u16>().ok(),
                _ => integer_value(value).and_then(|number| u16::try_from(number).ok()),
            };
            parsed
                .filter(|status| (100..=599).contains(status))
                .ok_or_else(|| ProxyError::bad_gateway("Qoder SSE wrapper has an invalid status"))
        })
        .transpose()?;
    let named_status = wrapper
        .get("statusCode")
        .map(|value| {
            value
                .as_str()
                .and_then(qoder_status_name)
                .filter(|status| (100..=599).contains(status))
                .ok_or_else(|| ProxyError::bad_gateway("Qoder SSE wrapper has an invalid status"))
        })
        .transpose()?;
    if value_status
        .zip(named_status)
        .is_some_and(|(left, right)| left != right)
    {
        return Err(ProxyError::bad_gateway(
            "Qoder SSE wrapper contains conflicting statuses",
        ));
    }
    Ok(value_status.or(named_status).unwrap_or(200))
}

fn recognized_qoder_error_code(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(value) => {
            let code = value.trim();
            matches!(code, "112" | "115").then(|| code.to_string())
        }
        Value::Number(value) => match value.as_u64() {
            Some(112) => Some("112".to_string()),
            Some(115) => Some("115".to_string()),
            _ => None,
        },
        _ => None,
    }
}

fn integer_value(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_str()?.trim().parse::<i64>().ok())
}

fn parse_qoder_retry_after_ms(headers: &axum::http::HeaderMap) -> Option<i64> {
    const MAX_RETRY_AFTER_MS: i64 = 24 * 60 * 60 * 1_000;
    if let Some(value) = headers
        .get("retry-after-ms")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<i64>().ok())
    {
        return Some(value.clamp(0, MAX_RETRY_AFTER_MS));
    }
    let value = headers
        .get(axum::http::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim();
    if let Ok(seconds) = value.parse::<i64>() {
        return Some(seconds.saturating_mul(1_000).clamp(0, MAX_RETRY_AFTER_MS));
    }
    let retry_at = httpdate::parse_http_date(value).ok()?;
    Some(
        retry_at
            .duration_since(std::time::SystemTime::now())
            .unwrap_or_default()
            .as_millis()
            .min(MAX_RETRY_AFTER_MS as u128) as i64,
    )
}

fn sanitize_error_message(value: &str) -> String {
    let mut nodes = 0usize;
    let structured = serde_json::from_str::<Value>(value).ok().map(|mut value| {
        redact_qoder_error_value(&mut value, 0, &mut nodes);
        match value {
            Value::String(value) => value,
            value => serde_json::to_string(&value).unwrap_or_default(),
        }
    });
    let message = structured.unwrap_or_else(|| value.to_string());
    crate::logging::redact_sensitive_text(&message)
        .chars()
        .filter(|character| !character.is_control() || matches!(character, '\n' | '\t'))
        .take(MAX_QODER_ERROR_MESSAGE_BYTES)
        .collect::<String>()
        .trim()
        .to_string()
}

fn redact_qoder_error_value(value: &mut Value, depth: usize, nodes: &mut usize) {
    const MAX_DEPTH: usize = 8;
    const MAX_NODES: usize = 256;
    *nodes = nodes.saturating_add(1);
    if depth >= MAX_DEPTH || *nodes > MAX_NODES {
        *value = Value::String("[REDACTED_TRUNCATED]".to_string());
        return;
    }
    match value {
        Value::Object(object) => {
            for (key, item) in object {
                let normalized = key
                    .chars()
                    .filter(|character| character.is_ascii_alphanumeric())
                    .flat_map(char::to_lowercase)
                    .collect::<String>();
                if matches!(
                    normalized.as_str(),
                    "authorization"
                        | "cookie"
                        | "setcookie"
                        | "cosykey"
                        | "securityoauthtoken"
                        | "token"
                        | "accesstoken"
                        | "devicetoken"
                        | "refreshtoken"
                        | "jobtoken"
                        | "personaltoken"
                        | "machinetoken"
                        | "uid"
                        | "aid"
                        | "orgid"
                        | "organizationid"
                ) {
                    *item = Value::String("[REDACTED]".to_string());
                } else {
                    redact_qoder_error_value(item, depth + 1, nodes);
                }
            }
        }
        Value::Array(values) => {
            for item in values {
                redact_qoder_error_value(item, depth + 1, nodes);
            }
        }
        Value::String(text) => {
            let trimmed = text.trim();
            if trimmed.starts_with('{') || trimmed.starts_with('[') {
                if let Ok(mut nested) = serde_json::from_str::<Value>(trimmed) {
                    redact_qoder_error_value(&mut nested, depth + 1, nodes);
                    *text = serde_json::to_string(&nested).unwrap_or_default();
                    return;
                }
            }
            *text = crate::logging::redact_sensitive_text(text);
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

fn qoder_status_name(value: &str) -> Option<u16> {
    value
        .trim()
        .parse::<u16>()
        .ok()
        .or_else(|| match value.trim().to_ascii_uppercase().as_str() {
            "OK" => Some(200),
            "BAD_REQUEST" => Some(400),
            "UNAUTHORIZED" => Some(401),
            "FORBIDDEN" => Some(403),
            "NOT_FOUND" => Some(404),
            "TOO_MANY_REQUESTS" => Some(429),
            "INTERNAL_SERVER_ERROR" => Some(500),
            "BAD_GATEWAY" => Some(502),
            "SERVICE_UNAVAILABLE" => Some(503),
            "GATEWAY_TIMEOUT" => Some(504),
            _ => None,
        })
}

pub fn qoder_error_json(error: &QoderUpstreamError) -> Bytes {
    Bytes::from(
        serde_json::to_vec(&json!({
            "error": {
                "type": if error.is_agent_limited() { "rate_limit_error" } else if error.is_entitlement_denied() { "permission_error" } else { "upstream_error" },
                "code": error.code,
                "message": error.message
            }
        }))
        .unwrap_or_default(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wrapped_qoder_event(status: u16, body: &str) -> Bytes {
        Bytes::from(format!(
            "data: {}\n\n",
            json!({"statusCodeValue": status, "body": body})
        ))
    }

    fn qoder_business_chunk(content: &str, finish_reason: Option<&str>) -> String {
        json!({
            "choices": [{
                "index": 0,
                "delta": {"content": content},
                "finish_reason": finish_reason,
            }]
        })
        .to_string()
    }

    #[test]
    fn frozen_qoder_cli_oracle_terminal_contract_drives_the_native_decoder() {
        let fixture: Value =
            serde_json::from_str(include_str!("../../assets/contract/qoder-cli-oracle.json"))
                .expect("Qoder CLI oracle fixture must be valid JSON");
        let contract = &fixture["terminalContract"];
        assert_eq!(
            contract["successRequires"],
            json!([
                "valid_inner_chunk",
                "authoritative_finish_reason",
                "upstream_eof",
                "exactly_one_downstream_done"
            ])
        );

        let mut decoder = QoderSseDecoder::default();
        let first = decoder
            .push(wrapped_qoder_event(
                200,
                &qoder_business_chunk("hello", None),
            ))
            .unwrap();
        assert!(!first.is_empty(), "valid_inner_chunk must be projected");
        let authoritative = decoder
            .push(wrapped_qoder_event(
                200,
                &qoder_business_chunk("", Some("stop")),
            ))
            .unwrap();
        assert!(
            !authoritative.is_empty(),
            "authoritative_finish_reason must remain visible downstream"
        );
        assert!(
            !decoder.is_terminal(),
            "terminal must wait for upstream EOF"
        );
        let eof = decoder.finish().unwrap();
        assert!(decoder.is_terminal());
        let downstream = String::from_utf8([first, authoritative, eof].concat()).unwrap();
        assert_eq!(downstream.matches("data: [DONE]").count(), 1);

        assert_eq!(
            contract["rejects"],
            json!([
                "missing_terminal",
                "malformed_json",
                "second_terminal",
                "business_data_after_terminal",
                "auth_error_after_commit"
            ])
        );
        for rejection in contract["rejects"].as_array().unwrap() {
            let rejection = rejection.as_str().unwrap();
            match rejection {
                "missing_terminal" => {
                    let mut decoder = QoderSseDecoder::default();
                    decoder
                        .push(wrapped_qoder_event(
                            200,
                            &qoder_business_chunk("partial", None),
                        ))
                        .unwrap();
                    assert!(decoder.finish().is_err());
                }
                "malformed_json" => {
                    let mut decoder = QoderSseDecoder::default();
                    assert!(decoder
                        .push(Bytes::from_static(b"data: {not-json}\n\n"))
                        .is_err());
                }
                "second_terminal" => {
                    let mut decoder = QoderSseDecoder::default();
                    decoder
                        .push(wrapped_qoder_event(
                            200,
                            &qoder_business_chunk("", Some("stop")),
                        ))
                        .unwrap();
                    assert!(decoder
                        .push(Bytes::from_static(b"data: [DONE]\n\n"))
                        .is_err());
                }
                "business_data_after_terminal" => {
                    let mut decoder = QoderSseDecoder::default();
                    decoder
                        .push(Bytes::from_static(b"data: [DONE]\n\n"))
                        .unwrap();
                    assert!(decoder
                        .push(wrapped_qoder_event(
                            200,
                            &qoder_business_chunk("late", None),
                        ))
                        .is_err());
                }
                "auth_error_after_commit" => {
                    let mut decoder = QoderSseDecoder::default();
                    let committed = decoder
                        .push(wrapped_qoder_event(
                            200,
                            &qoder_business_chunk("committed", None),
                        ))
                        .unwrap();
                    assert!(!committed.is_empty());
                    let error = decoder
                        .push_classified(wrapped_qoder_event(
                            401,
                            r#"{"message":"expired after commit"}"#,
                        ))
                        .unwrap_err();
                    assert!(error
                        .upstream()
                        .is_some_and(QoderUpstreamError::is_authentication_failure));
                }
                other => panic!("unexercised Qoder terminal rejection {other:?}"),
            }
        }
    }

    #[test]
    fn qoder_error_taxonomy_distinguishes_entitlement_limit_and_auth() {
        let entitlement = QoderUpstreamError::from_status_body(
            403,
            br#"{"code":112,"message":"model unavailable"}"#,
        );
        assert!(entitlement.is_entitlement_denied());
        assert!(!entitlement.is_authentication_failure());
        assert_eq!(
            entitlement.downstream_status(),
            axum::http::StatusCode::FORBIDDEN
        );

        let limit = QoderUpstreamError::from_status_body(
            400,
            br#"{"code":"115","agentLimitResetTime":1900000000000}"#,
        );
        assert!(limit.is_agent_limited());
        assert_eq!(
            limit.downstream_status(),
            axum::http::StatusCode::TOO_MANY_REQUESTS
        );

        let auth = QoderUpstreamError::from_status_body(401, br#"{"message":"login expired"}"#);
        assert!(auth.is_authentication_failure());
        let permission = QoderUpstreamError::from_status_body(403, br#"{"message":"denied"}"#);
        assert!(!permission.is_authentication_failure());
        assert_eq!(
            permission.downstream_status(),
            axum::http::StatusCode::FORBIDDEN
        );
    }

    #[test]
    fn qoder_null_outer_error_fields_do_not_hide_nested_limit_evidence() {
        let error = QoderUpstreamError::from_status_body(
            400,
            br#"{"code":null,"message":null,"agentLimitResetTime":null,"error":{"code":115,"message":"limited"},"data":{"agentLimitResetTime":1900000000000}}"#,
        );
        assert_eq!(error.code.as_deref(), Some("115"));
        assert_eq!(error.message, "limited");
        assert_eq!(error.agent_limit_reset_at_ms, Some(1_900_000_000_000));
        assert!(error.is_agent_limited());
        assert_eq!(
            error.downstream_status(),
            axum::http::StatusCode::TOO_MANY_REQUESTS
        );

        let mut decoder = QoderSseDecoder::default();
        let rejected = decoder
            .push_classified(wrapped_qoder_event(
                400,
                r#"{"code":null,"error":{"code":115,"message":"limited"}}"#,
            ))
            .unwrap_err();
        assert!(rejected
            .upstream()
            .is_some_and(QoderUpstreamError::is_agent_limited));

        let explicit_outer =
            QoderUpstreamError::from_status_body(403, br#"{"code":112,"error":{"code":115}}"#);
        assert!(explicit_outer.is_entitlement_denied());
        assert!(!explicit_outer.is_agent_limited());
        let unrecognized_outer = QoderUpstreamError::from_status_body(
            400,
            br#"{"code":"future-code","error":{"code":115}}"#,
        );
        assert!(!unrecognized_outer.is_agent_limited());
    }

    #[test]
    fn qoder_nested_error_payload_recovers_classification_without_overriding_outer_code() {
        let nested = QoderUpstreamError::from_status_body(
            400,
            br#"{"data":{"data":{"code":115,"message":"agent limit","agentLimitResetTime":1900000000000}}}"#,
        );
        assert_eq!(nested.code.as_deref(), Some("115"));
        assert_eq!(nested.message, "agent limit");
        assert_eq!(nested.agent_limit_reset_at_ms, Some(1_900_000_000_000));
        assert_eq!(nested.downstream_status().as_u16(), 429);

        let encoded = QoderUpstreamError::from_status_body(
            400,
            br#"{"message":"{\"code\":115,\"agentLimitResetTime\":1900000000000}"}"#,
        );
        assert_eq!(encoded.code.as_deref(), Some("115"));
        assert_eq!(encoded.agent_limit_reset_at_ms, Some(1_900_000_000_000));

        let outer = QoderUpstreamError::from_status_body(
            403,
            br#"{"code":112,"data":{"data":{"code":115}}}"#,
        );
        assert_eq!(outer.code.as_deref(), Some("112"));
        assert!(outer.is_entitlement_denied());
    }

    #[test]
    fn qoder_error_retry_after_and_recursive_redaction_are_bounded() {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert("retry-after-ms", "2500".parse().unwrap());
        let limited =
            QoderUpstreamError::from_response(429, &headers, br#"{"message":"slow down"}"#);
        assert_eq!(limited.retry_after_ms, Some(2_500));

        let secret = r#"{"message":"safe","securityOauthToken":"oauth-secret","nested":{"refresh_token":"refresh-secret","uid":"user-secret","body":"{\"cosy-key\":\"cosy-secret\",\"organizationId\":\"org-secret\"}"},"headers":{"Authorization":"Bearer bearer-secret","Set-Cookie":"session=cookie-secret"}}"#;
        let sanitized = sanitize_error_message(secret);
        for leaked in [
            "oauth-secret",
            "refresh-secret",
            "user-secret",
            "cosy-secret",
            "org-secret",
            "bearer-secret",
            "cookie-secret",
        ] {
            assert!(!sanitized.contains(leaked), "leaked {leaked}");
        }
        assert!(sanitized.contains("safe"));
        assert!(sanitized.contains("REDACTED"));
    }

    #[test]
    fn qoder_error_code_only_exposes_recognized_values() {
        for code in [
            json!("Authorization: Bearer leaked-secret"),
            json!("999999999999999999999999999999"),
            json!("123456"),
            json!(123456),
            json!(-112),
            json!(112.5),
            json!("0112"),
        ] {
            let body = serde_json::to_vec(&json!({"code": code, "message": "denied"})).unwrap();
            let error = QoderUpstreamError::from_status_body(403, &body);
            assert!(error.code.is_none());
            assert_eq!(error.clone().into_proxy_error().message, "denied");
            let downstream: Value = serde_json::from_slice(&qoder_error_json(&error)).unwrap();
            assert!(downstream["error"]["code"].is_null());
            assert_eq!(downstream["error"]["message"], "denied");
        }
        for code in [json!(112), json!("112"), json!(115), json!("115")] {
            let body = serde_json::to_vec(&json!({"code": code, "message": "denied"})).unwrap();
            let error = QoderUpstreamError::from_status_body(403, &body);
            let expected = if code == json!(112) || code == json!("112") {
                "112"
            } else {
                "115"
            };
            assert_eq!(error.code.as_deref(), Some(expected));
        }
    }

    #[test]
    fn qoder_sse_decoder_unwraps_chunks_and_commits_terminal_only_at_eof() {
        let mut decoder = QoderSseDecoder::default();
        let first = concat!(
            r#"data: {"statusCodeValue":200,"body":"{\"choices\":[{\"delta\":{\"content\":\"hello\"},\"finish_reason\":null}]}"}"#,
            "\n\n"
        )
        .as_bytes();
        let terminal = concat!(
            r#"data: {"statusCodeValue":200,"body":"{\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}"}"#,
            "\n\n"
        )
        .as_bytes();
        let split = first.len() / 2;
        assert!(decoder
            .push(Bytes::copy_from_slice(&first[..split]))
            .unwrap()
            .is_empty());
        let output = decoder
            .push(Bytes::from([&first[split..], terminal].concat()))
            .unwrap();
        let output = String::from_utf8(output.to_vec()).unwrap();
        assert!(output.contains("hello"));
        assert!(!output.contains("data: [DONE]"));
        assert!(!decoder.is_terminal());
        assert_eq!(
            decoder.finish().unwrap(),
            Bytes::from_static(b"data: [DONE]\n\n")
        );
        assert!(decoder.is_terminal());
    }

    #[test]
    fn qoder_sse_decoder_separates_mixed_sse_line_endings() {
        let first = wrapped_qoder_event(200, &qoder_business_chunk("hello", None));
        let terminal = wrapped_qoder_event(200, &qoder_business_chunk("", Some("stop")));
        let first = String::from_utf8(first.to_vec()).unwrap();
        let terminal = String::from_utf8(terminal.to_vec())
            .unwrap()
            .replace("\n", "\r\n");
        let stream = format!("{first}{terminal}");
        for split in 0..=stream.len() {
            let mut decoder = QoderSseDecoder::default();
            let head = decoder
                .push(Bytes::copy_from_slice(&stream.as_bytes()[..split]))
                .unwrap();
            let tail = decoder
                .push(Bytes::copy_from_slice(&stream.as_bytes()[split..]))
                .unwrap();
            let output = [head.as_ref(), tail.as_ref()].concat();
            assert!(String::from_utf8_lossy(&output).contains("hello"));
            assert_eq!(
                decoder.finish().unwrap(),
                Bytes::from_static(b"data: [DONE]\n\n")
            );
        }

        let mut cr_only = QoderSseDecoder::default();
        let stream = stream.replace("\r\n", "\n").replace('\n', "\r");
        assert!(cr_only.push(Bytes::from(stream)).is_ok());
        assert!(cr_only.finish().is_ok());
    }

    #[test]
    fn qoder_sse_decoder_fails_closed_on_error_truncation_and_missing_terminal() {
        let mut decoder = QoderSseDecoder::default();
        let error = concat!(
            r#"data: {"statusCodeValue":403,"body":"{\"code\":112,\"message\":\"not entitled\"}"}"#,
            "\n\n"
        );
        let parsed = decoder.push(Bytes::from(error)).unwrap_err();
        assert_eq!(parsed.status, axum::http::StatusCode::FORBIDDEN);
        assert!(parsed.message.contains("112"));

        let mut incomplete = QoderSseDecoder::default();
        incomplete
            .push(Bytes::from(concat!(
                r#"data: {"statusCodeValue":200,"body":"{\"choices\":[]}"}"#,
                "\n\n"
            )))
            .unwrap();
        assert!(incomplete.finish().is_err());
    }

    #[test]
    fn qoder_sse_decoder_rejects_business_data_after_terminal() {
        let mut decoder = QoderSseDecoder::default();
        decoder
            .push(Bytes::from_static(b"data: [DONE]\n\n"))
            .unwrap();
        assert!(decoder
            .push(Bytes::from_static(
                concat!(r#"data: {"statusCodeValue":200,"body":"{}"}"#, "\n\n").as_bytes(),
            ))
            .is_err());
    }

    #[test]
    fn qoder_sse_decoder_rejects_empty_wrappers_after_terminal() {
        for wrapper in [r#"{}"#, r#"{"statusCodeValue":200,"body":""}"#] {
            let mut decoder = QoderSseDecoder::default();
            decoder
                .push(Bytes::from_static(b"data: [DONE]\n\n"))
                .unwrap();
            let frame = format!("data: {wrapper}\n\n");
            assert!(decoder.push(Bytes::from(frame)).is_err());
        }
    }

    #[test]
    fn qoder_sse_decoder_rejects_malformed_wrapper_shape_before_done() {
        for wrapper in [
            json!(null),
            json!([]),
            json!("opaque"),
            json!({"body": null}),
            json!({"body": {}}),
            json!({"body": 1}),
        ] {
            let stream = format!("data: {wrapper}\n\ndata: [DONE]\n\n");
            let mut decoder = QoderSseDecoder::default();
            let error = decoder.push_classified(Bytes::from(stream)).unwrap_err();
            assert!(error.upstream().is_none(), "{wrapper}");
            assert!(!decoder.is_terminal(), "{wrapper}");
            assert!(decoder.finish().is_err(), "{wrapper}");
        }

        let mut empty_heartbeat = QoderSseDecoder::default();
        assert!(empty_heartbeat
            .push(Bytes::from_static(b"data: {}\n\ndata: [DONE]\n\n"))
            .unwrap()
            .is_empty());
        assert!(empty_heartbeat.finish().is_err());
    }

    #[test]
    fn qoder_heartbeat_only_stream_and_nonstream_cannot_fabricate_success() {
        for heartbeat in [
            b"data: [DONE]\n\n".as_slice(),
            b"data: {\"statusCodeValue\":200,\"body\":\"\"}\n\ndata: [DONE]\n\n".as_slice(),
            b"data: {\"statusCodeValue\":200,\"body\":\"{\\\"choices\\\":[{\\\"delta\\\":{},\\\"finish_reason\\\":null}]}\"}\n\ndata: [DONE]\n\n".as_slice(),
        ] {
            let mut decoder = QoderSseDecoder::default();
            assert!(decoder.push(Bytes::copy_from_slice(heartbeat)).unwrap().is_empty());
            assert!(decoder.finish().is_err());
            assert!(!decoder.is_terminal());
        }
        let mut no_finish = QoderSseDecoder::default();
        let content = wrapped_qoder_event(200, &qoder_business_chunk("visible", None));
        assert!(!no_finish.push(content).unwrap().is_empty());
        no_finish
            .push(Bytes::from_static(b"data: [DONE]\n\n"))
            .unwrap();
        assert!(no_finish.finish().is_err());

        let mut aggregator = QoderChatSseAggregator::default();
        aggregator
            .push(Bytes::from_static(b"data: [DONE]\n\n"))
            .unwrap();
        assert!(aggregator.finish("auto", 1).is_err());
    }

    #[test]
    fn qoder_sse_decoder_rejects_malformed_or_other_choice_terminals_before_forwarding() {
        for inner in [
            r#"{"choices":[{"index":0,"delta":{},"finish_reason":true}]}"#,
            r#"{"choices":[{"index":0,"delta":{},"finish_reason":"  "}]}"#,
            r#"{"choices":[{"index":1,"delta":{},"finish_reason":"stop"}]}"#,
            r#"{"choices":[{"index":"invalid","delta":{},"finish_reason":"stop"}]}"#,
        ] {
            let event = format!(
                "data: {{\"statusCodeValue\":200,\"body\":{}}}\n\n",
                serde_json::to_string(inner).unwrap()
            );
            let mut decoder = QoderSseDecoder::default();
            let error = decoder.push(Bytes::from(event)).unwrap_err();
            assert_eq!(error.status, axum::http::StatusCode::BAD_GATEWAY);
            assert!(!decoder.is_terminal());
            let mut aggregator = QoderChatSseAggregator::default();
            assert!(aggregator
                .push(Bytes::from(format!("data: {inner}\n\n")))
                .is_err());
        }
    }

    #[test]
    fn qoder_rejects_duplicate_choice_zero_within_one_chunk() {
        let inner = json!({
            "choices": [
                {"index": 0, "delta": {"content": "left"}, "finish_reason": null},
                {"index": 0, "delta": {"content": "right"}, "finish_reason": "stop"}
            ]
        });
        let mut decoder = QoderSseDecoder::default();
        let error = decoder
            .push(wrapped_qoder_event(200, &inner.to_string()))
            .expect_err("a duplicate choice must not be forwarded as two choice-zero deltas");
        assert_eq!(error.status, axum::http::StatusCode::BAD_GATEWAY);
        assert!(!decoder.is_terminal());

        let mut aggregator = QoderChatSseAggregator::default();
        let error = aggregator
            .push(Bytes::from(format!("data: {inner}\n\n")))
            .expect_err("a duplicate choice must not be concatenated into one answer");
        assert_eq!(error.status, axum::http::StatusCode::BAD_GATEWAY);
        assert!(aggregator.finish("model", 1).is_err());
    }

    #[test]
    fn qoder_decoder_rejects_explicit_non_string_semantic_text() {
        for inner in [
            json!({"choices":[{"index":0,"delta":{"reasoning_content":42},"finish_reason":"stop"}]}),
            json!({"choices":[{"index":0,"delta":{"reasoning_text":[]},"finish_reason":"stop"}]}),
            json!({"choices":[{"index":0,"delta":{"reasoning":{}},"finish_reason":"stop"}]}),
            json!({"choices":[{"index":0,"delta":{"reasoning_content":"kept","reasoning_text":42},"finish_reason":"stop"}]}),
            json!({"choices":[{"index":0,"delta":{"content":[{"type":"text","text":42}]},"finish_reason":"stop"}]}),
            json!({"event":"content_block_delta","data":{"type":"text_delta","text":{"lost":"text"}}}),
        ] {
            let mut decoder = QoderSseDecoder::default();
            assert!(
                decoder
                    .push(wrapped_qoder_event(200, &inner.to_string()))
                    .is_err(),
                "explicit non-string semantic text must not become a successful response: {inner}"
            );
            assert!(decoder.finish().is_err());
        }

        for inner in [
            json!({"choices":[{"index":0,"delta":{"reasoning_content":null},"finish_reason":"stop"}]}),
            json!({"choices":[{"index":0,"delta":{"reasoning_content":"kept"},"finish_reason":"stop"}]}),
            json!({"event":"content_block_delta","data":{"type":"text_delta","text":"kept"}}),
        ] {
            let mut decoder = QoderSseDecoder::default();
            assert!(decoder
                .push(wrapped_qoder_event(200, &inner.to_string()))
                .is_ok());
        }
    }

    #[test]
    fn qoder_null_reasoning_alias_does_not_hide_visible_reasoning() {
        let inner = json!({
            "choices": [{
                "index": 0,
                "delta": {"reasoning_content": null, "reasoning_text": "actual thought"},
                "finish_reason": null
            }]
        });
        let terminal = json!({
            "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]
        });
        let mut decoder = QoderSseDecoder::default();
        let mut aggregator = QoderChatSseAggregator::default();
        aggregator
            .push(
                decoder
                    .push(wrapped_qoder_event(200, &inner.to_string()))
                    .unwrap(),
            )
            .unwrap();
        aggregator
            .push(
                decoder
                    .push(wrapped_qoder_event(200, &terminal.to_string()))
                    .unwrap(),
            )
            .unwrap();
        aggregator.push(decoder.finish().unwrap()).unwrap();
        let response = aggregator.finish("model", 1).unwrap();
        assert_eq!(
            response["choices"][0]["message"]["reasoning_content"],
            "actual thought"
        );

        let mut direct = QoderChatSseAggregator::default();
        direct
            .push(Bytes::from(format!(
                "data: {inner}\n\ndata: {terminal}\n\ndata: [DONE]\n\n"
            )))
            .unwrap();
        let response = direct.finish("model", 1).unwrap();
        assert_eq!(
            response["choices"][0]["message"]["reasoning_content"],
            "actual thought"
        );
    }

    #[test]
    fn qoder_sse_decoder_preserves_structured_embedded_errors() {
        for (status, body, expected_status, expected_code) in [
            (401, r#"{"message":"expired"}"#, 401, None),
            (403, r#"{"code":112,"message":"denied"}"#, 403, Some("112")),
            (
                400,
                r#"{"code":115,"agentLimitResetTime":1900000000000}"#,
                429,
                Some("115"),
            ),
        ] {
            let event = format!(
                "data: {{\"statusCodeValue\":{status},\"body\":{}}}\n\n",
                serde_json::to_string(body).unwrap()
            );
            let mut decoder = QoderSseDecoder::default();
            let error = decoder.push_classified(Bytes::from(event)).unwrap_err();
            let upstream = error.upstream().expect("structured upstream error");
            assert_eq!(upstream.downstream_status().as_u16(), expected_status);
            assert_eq!(upstream.code.as_deref(), expected_code);
        }
    }

    #[test]
    fn qoder_sse_decoder_rejects_http_200_inner_error_before_success_terminal() {
        for error in [
            json!({"message": "quota exhausted", "code": 115}),
            json!({"message": "upstream failed"}),
            json!("upstream failed"),
            json!({}),
        ] {
            let inner = json!({"error": error}).to_string();
            let mut decoder = QoderSseDecoder::default();
            let failure = decoder
                .push_classified(wrapped_qoder_event(200, &inner))
                .expect_err("HTTP 200 inner error must not be dropped");
            assert!(failure.upstream().is_some(), "{inner}");
            assert!(!decoder.is_terminal());
            assert!(decoder.finish().is_err());
        }

        let mut decoder = QoderSseDecoder::default();
        let business = decoder
            .push(wrapped_qoder_event(
                200,
                &qoder_business_chunk("partial", None),
            ))
            .unwrap();
        assert!(!business.is_empty());
        let inner = json!({
            "error": {"code": 115, "message": "quota exhausted"},
            "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]
        });
        let failure = decoder
            .push_classified(wrapped_qoder_event(200, &inner.to_string()))
            .expect_err("error must prevail over a contradictory success terminal");
        let upstream = failure.upstream().expect("embedded upstream error");
        assert_eq!(upstream.downstream_status().as_u16(), 429);
        assert!(!decoder.is_terminal());
        assert!(decoder.finish().is_err());

        let mut decoder = QoderSseDecoder::default();
        assert!(decoder
            .push(wrapped_qoder_event(
                200,
                &json!({"error": null, "choices": [{"index": 0, "delta": {"content": "ok"}, "finish_reason": "stop"}]}).to_string(),
            ))
            .is_ok());
        assert!(decoder.finish().is_ok());
    }

    #[test]
    fn qoder_sse_decoder_rejects_explicit_invalid_or_conflicting_status() {
        for wrapper in [
            json!({"statusCodeValue": -1, "body": ""}),
            json!({"statusCodeValue": "401", "body": ""}),
            json!({"statusCodeValue": 0, "body": ""}),
            json!({"statusCodeValue": 700, "body": ""}),
            json!({"statusCodeValue": null, "body": ""}),
            json!({"statusCode": "NOT_A_STATUS", "body": ""}),
            json!({"statusCode": 401, "body": ""}),
            json!({"statusCodeValue": 200, "statusCode": "FORBIDDEN", "body": ""}),
            json!({"statusCodeValue": 200, "statusCode": null, "body": ""}),
        ] {
            let mut decoder = QoderSseDecoder::default();
            let frame = format!("data: {wrapper}\n\n");
            assert!(
                decoder.push(Bytes::from(frame)).is_err(),
                "explicit invalid status must not default to success: {wrapper}"
            );
        }
        for wrapper in [
            json!({"body": ""}),
            json!({"statusCodeValue": 200, "body": ""}),
            json!({"statusCodeValue": "200", "body": ""}),
            json!({"statusCode": "OK", "body": ""}),
            json!({"statusCodeValue": 200, "statusCode": "OK", "body": ""}),
        ] {
            let mut decoder = QoderSseDecoder::default();
            let frame = format!("data: {wrapper}\n\n");
            assert!(decoder.push(Bytes::from(frame)).is_ok(), "{wrapper}");
        }

        let mut decoder = QoderSseDecoder::default();
        let error = decoder
            .push_classified(Bytes::from_static(
                b"data: {\"statusCodeValue\":\"401\",\"body\":\"\"}\n\n",
            ))
            .unwrap_err();
        assert_eq!(error.upstream().unwrap().downstream_status().as_u16(), 401);

        let mut decoder = QoderSseDecoder::default();
        let conflict = decoder
            .push_classified(Bytes::from_static(
                b"data: {\"statusCodeValue\":200,\"statusCode\":\"FORBIDDEN\",\"body\":\"\"}\n\n",
            ))
            .unwrap_err();
        assert!(conflict.upstream().is_none());
    }

    #[test]
    fn qoder_chat_sse_aggregator_preserves_reasoning_tools_and_usage() {
        let chunks = concat!(
            "data: {\"id\":\"chat-q\",\"created\":7,\"model\":\"qmodel\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"reasoning_content\":\"plan \"},\"finish_reason\":null}]}\n\n",
            "data: {\"id\":\"chat-q\",\"created\":7,\"model\":\"qmodel\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hello\",\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"type\":\"function\",\"function\":{\"name\":\"look\",\"arguments\":\"{\\\"q\\\":\"}}]},\"finish_reason\":null}]}\n\n",
            "data: {\"id\":\"chat-q\",\"created\":7,\"model\":\"qmodel\",\"choices\":[{\"index\":0,\"delta\":{\"reasoning_content\":\"first\",\"tool_calls\":[{\"index\":0,\"function\":{\"name\":\"up\",\"arguments\":\"1}\"}}]},\"finish_reason\":\"tool_calls\"}],\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":4,\"total_tokens\":7}}\n\n",
            "data: [DONE]\n\n"
        );
        let mut aggregator = QoderChatSseAggregator::default();
        for part in chunks.as_bytes().chunks(37) {
            aggregator.push(Bytes::copy_from_slice(part)).unwrap();
        }
        let response = aggregator.finish("fallback", 9).unwrap();
        assert_eq!(response["id"], "chat-q");
        assert_eq!(response["choices"][0]["message"]["content"], "hello");
        assert_eq!(
            response["choices"][0]["message"]["reasoning_content"],
            "plan first"
        );
        assert_eq!(
            response["choices"][0]["message"]["tool_calls"][0]["function"]["name"],
            "lookup"
        );
        assert_eq!(
            response["choices"][0]["message"]["tool_calls"][0]["function"]["arguments"],
            "{\"q\":1}"
        );
        assert_eq!(response["usage"]["total_tokens"], 7);
        assert_eq!(response["choices"][0]["finish_reason"], "tool_calls");
    }

    #[test]
    fn qoder_tool_call_index_rejects_explicit_invalid_values() {
        for index in [json!(null), json!("invalid"), json!(-1), json!(128)] {
            let inner = json!({"choices":[{"index":0,"delta":{"tool_calls":[
                {"index":index,"id":"call_1","type":"function","function":{"name":"lookup","arguments":"{}"}}
            ]},"finish_reason":"tool_calls"}]});
            let mut decoder = QoderSseDecoder::default();
            let event = wrapped_qoder_event(200, &inner.to_string());
            assert!(decoder.push(event).is_err(), "decoder accepted {index}");
            assert!(!decoder.is_terminal());

            let mut aggregator = QoderChatSseAggregator::default();
            let stream = format!("data: {inner}\n\ndata: [DONE]\n\n");
            assert!(
                aggregator.push(Bytes::from(stream)).is_err(),
                "aggregator accepted {index}"
            );
        }

        let mut decoder = QoderSseDecoder::default();
        let envelope = json!({"event":"tool_use_start","index":"invalid","data":{
            "id":"call_1","name":"lookup","input":{}
        }});
        assert!(decoder
            .push(wrapped_qoder_event(200, &envelope.to_string()))
            .is_err());

        let valid = json!({"choices":[{"index":0,"delta":{"tool_calls":[
            {"index":"0","id":"call_1","type":"function","function":{"name":"lookup","arguments":"{}"}}
        ]},"finish_reason":"tool_calls"}]});
        let mut decoder = QoderSseDecoder::default();
        decoder
            .push(wrapped_qoder_event(200, &valid.to_string()))
            .unwrap();
        decoder.finish().unwrap();
    }

    #[test]
    fn qoder_tool_call_id_is_opaque_across_normalization_and_aggregation() {
        let padded = json!({"choices":[{"index":0,"delta":{"tool_calls":[
            {"index":0,"id":" call-a ","type":"function","function":{"name":"lookup","arguments":"{}"}}
        ]},"finish_reason":"tool_calls"}]});
        let mut decoder = QoderSseDecoder::default();
        let canonical = decoder
            .push(wrapped_qoder_event(200, &padded.to_string()))
            .unwrap();
        let mut aggregator = QoderChatSseAggregator::default();
        aggregator.push(canonical).unwrap();
        aggregator.push(decoder.finish().unwrap()).unwrap();
        let response = aggregator.finish("fallback", 9).unwrap();
        assert_eq!(
            response["choices"][0]["message"]["tool_calls"][0]["id"],
            " call-a "
        );

        let unpadded = json!({"choices":[{"index":0,"delta":{"tool_calls":[
            {"index":0,"id":"call-a","type":"function","function":{"name":"lookup"}}
        ]},"finish_reason":null}]});
        let mut padded_fragment = padded.clone();
        padded_fragment["choices"][0]["finish_reason"] = Value::Null;
        let mut decoder = QoderSseDecoder::default();
        let first = decoder
            .push(wrapped_qoder_event(200, &padded_fragment.to_string()))
            .unwrap();
        let second = decoder
            .push(wrapped_qoder_event(200, &unpadded.to_string()))
            .unwrap();
        let mut aggregator = QoderChatSseAggregator::default();
        aggregator.push(first).unwrap();
        assert!(aggregator.push(second).is_err());
    }

    #[test]
    fn qoder_tool_aliases_cannot_disagree_on_one_call() {
        for call in [
            json!({"id":"call-a","call_id":"call-b","name":"lookup","arguments":"{}"}),
            json!({"id":"call-a","function":{"name":"lookup","arguments":"{}"},"tool_name":"write"}),
            json!({"id":7,"call_id":"call-a","name":"lookup","arguments":"{}"}),
        ] {
            let inner = json!({"choices":[{"index":0,"delta":{"tool_calls":[call]},"finish_reason":"tool_calls"}]});
            let mut decoder = QoderSseDecoder::default();
            let error = decoder
                .push(wrapped_qoder_event(200, &inner.to_string()))
                .unwrap_err();
            assert_eq!(error.status, axum::http::StatusCode::BAD_GATEWAY);
            assert!(!decoder.is_terminal());
        }

        let consistent = json!({"choices":[{"index":0,"delta":{"tool_calls":[{
            "id":" call-a ","call_id":" call-a ",
            "name":"lookup","tool_name":"lookup",
            "function":{"name":"lookup","arguments":"{}"}
        }]},"finish_reason":"tool_calls"}]});
        let mut decoder = QoderSseDecoder::default();
        let canonical = decoder
            .push(wrapped_qoder_event(200, &consistent.to_string()))
            .unwrap();
        let mut aggregator = QoderChatSseAggregator::default();
        aggregator.push(canonical).unwrap();
        aggregator.push(decoder.finish().unwrap()).unwrap();
        let response = aggregator.finish("fallback", 9).unwrap();
        assert_eq!(
            response["choices"][0]["message"]["tool_calls"][0]["id"],
            " call-a "
        );
    }

    #[test]
    fn qoder_chat_sse_aggregator_rejects_duplicate_done() {
        let mut aggregator = QoderChatSseAggregator::default();
        let stream = concat!(
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hello\"},\"finish_reason\":\"stop\"}]}\n\n",
            "data: [DONE]\n\n",
            "data: [DONE]\n\n"
        );
        assert!(aggregator
            .push(Bytes::from_static(stream.as_bytes()))
            .is_err());
    }

    #[test]
    fn qoder_chat_sse_aggregator_separates_mixed_sse_line_endings() {
        let first = "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hello\"},\"finish_reason\":null}]}\n\n";
        let terminal = "data: [DONE]\r\n\r\n";
        let stream = format!("{first}{terminal}");
        for split in 0..=stream.len() {
            let mut aggregator = QoderChatSseAggregator::default();
            aggregator
                .push(Bytes::copy_from_slice(&stream.as_bytes()[..split]))
                .unwrap();
            aggregator
                .push(Bytes::copy_from_slice(&stream.as_bytes()[split..]))
                .unwrap();
            let response = aggregator.finish("model", 1).unwrap();
            assert_eq!(response["choices"][0]["message"]["content"], "hello");
        }
        let mut cr_only = QoderChatSseAggregator::default();
        cr_only
            .push(Bytes::from(
                stream.replace("\r\n", "\n").replace('\n', "\r"),
            ))
            .unwrap();
        assert_eq!(
            cr_only.finish("model", 1).unwrap()["choices"][0]["message"]["content"],
            "hello"
        );
    }

    #[test]
    fn qoder_chat_sse_aggregator_rejects_incomplete_or_invalid_tool_calls() {
        let mut incomplete = QoderChatSseAggregator::default();
        incomplete
            .push(Bytes::from_static(
                b"data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"x\"}}]}\n\n",
            ))
            .unwrap();
        assert!(incomplete.finish("model", 1).is_err());

        let mut invalid = QoderChatSseAggregator::default();
        invalid
            .push(Bytes::from_static(
                b"data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call\",\"function\":{\"name\":\"tool\",\"arguments\":\"{\"}}]},\"finish_reason\":\"tool_calls\"}]}\n\ndata: [DONE]\n\n",
            ))
            .unwrap();
        assert!(invalid.finish("model", 1).is_err());
    }

    #[test]
    fn qoder_decoder_normalizes_anthropic_style_envelopes_without_weakening_eof() {
        let events = [
            json!({"event":"content_block_delta","data":{"type":"thinking_delta","text":"plan "}}),
            json!({"event":"content_block_delta","data":{"type":"text_delta","text":"hello"}}),
            json!({"event":"tool_use_start","index":0,"data":{"id":"call_1","name":"lookup","type":"tool_use"}}),
            json!({"event":"tool_use_delta","index":0,"data":{"arguments":{"q":"x"}}}),
            json!({"event":"message_delta","data":{"usage":{"input_tokens":"3","output_tokens":4.9}}}),
            json!({"event":"message_stop"}),
        ];
        let mut decoder = QoderSseDecoder::default();
        let mut aggregator = QoderChatSseAggregator::default();
        for event in events {
            let output = decoder
                .push(wrapped_qoder_event(200, &event.to_string()))
                .unwrap();
            aggregator.push(output).unwrap();
        }
        assert!(!decoder.is_terminal());
        aggregator.push(decoder.finish().unwrap()).unwrap();
        let response = aggregator.finish("fallback", 9).unwrap();
        assert_eq!(
            response["choices"][0]["message"]["reasoning_content"],
            "plan "
        );
        assert_eq!(response["choices"][0]["message"]["content"], "hello");
        assert_eq!(
            response["choices"][0]["message"]["tool_calls"][0]["id"],
            "call_1"
        );
        assert_eq!(
            response["choices"][0]["message"]["tool_calls"][0]["function"]["arguments"],
            "{\"q\":\"x\"}"
        );
        assert_eq!(response["usage"]["prompt_tokens"], 3);
        assert_eq!(response["usage"]["completion_tokens"], 4);
        assert_eq!(response["usage"]["total_tokens"], 7);
        assert_eq!(response["choices"][0]["finish_reason"], "tool_calls");

        let standard = [
            json!({"type":"message_start","message":{"usage":{"input_tokens":3,"output_tokens":0}}}),
            json!({"type":"ping"}),
            json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"call_2","name":"lookup","input":{}}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"q\":\"y\"}"}}),
            json!({"type":"content_block_stop","index":0}),
            json!({"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"world"}}),
            json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":2}}),
            json!({"type":"message_stop"}),
        ];
        let mut decoder = QoderSseDecoder::default();
        let mut aggregator = QoderChatSseAggregator::default();
        for event in standard {
            aggregator
                .push(
                    decoder
                        .push(wrapped_qoder_event(200, &event.to_string()))
                        .unwrap(),
                )
                .unwrap();
        }
        aggregator.push(decoder.finish().unwrap()).unwrap();
        let response = aggregator.finish("fallback", 9).unwrap();
        assert_eq!(response["choices"][0]["message"]["content"], "world");
        assert_eq!(
            response["choices"][0]["message"]["tool_calls"][0]["id"],
            "call_2"
        );
        assert_eq!(
            response["choices"][0]["message"]["tool_calls"][0]["function"]["arguments"],
            "{\"q\":\"y\"}"
        );
        assert_eq!(response["choices"][0]["finish_reason"], "tool_calls");
        assert_eq!(response["usage"]["prompt_tokens"], 3);
        assert_eq!(response["usage"]["total_tokens"], 5);
    }

    #[test]
    fn qoder_decoder_rejects_explicit_invalid_usage_instead_of_reporting_zero() {
        for usage in [
            json!({"input_tokens": -1, "output_tokens": 2}),
            json!({"prompt_tokens": {"unexpected": 1}, "completion_tokens": 2}),
            json!({"prompt_tokens": 3, "input_tokens": -1, "completion_tokens": 2}),
            json!({"prompt_tokens": 0, "input_tokens": -1, "completion_tokens": 2}),
            json!({"prompt_tokens": 3, "completion_tokens": 2, "total_tokens": "NaN"}),
            json!({"input_tokens": "1e100", "output_tokens": 2}),
        ] {
            let inner = json!({
                "choices": [{"index": 0, "delta": {"content": "hello"}, "finish_reason": "stop"}],
                "usage": usage,
            });
            let mut decoder = QoderSseDecoder::default();
            assert!(decoder
                .push(wrapped_qoder_event(200, &inner.to_string()))
                .is_err());
        }

        let mut decoder = QoderSseDecoder::default();
        assert!(decoder
            .push(wrapped_qoder_event(
                200,
                &json!({"event":"message_delta","data":{"usage":{"input_tokens":-1}}}).to_string(),
            ))
            .is_err());

        let mut aggregator = QoderChatSseAggregator::default();
        assert!(aggregator
            .push(Bytes::from_static(
                b"data: {\"choices\":[],\"usage\":{\"prompt_tokens\":-1}}\n\n",
            ))
            .is_err());
    }

    #[test]
    fn qoder_usage_fills_zero_primary_counts_from_nonzero_aliases() {
        let cases = [
            vec![json!({
                "choices": [{"index": 0, "delta": {"content": "hello"}, "finish_reason": "stop"}],
                "usage": {
                    "prompt_tokens": 0,
                    "input_tokens": 5,
                    "completion_tokens": 0,
                    "output_tokens": 2,
                    "total_tokens": 7
                }
            })],
            vec![
                json!({"event": "content_block_delta", "data": {"text": "hello"}}),
                json!({
                    "event": "message_delta",
                    "data": {
                        "stop_reason": "end_turn",
                        "usage": {
                            "prompt_tokens": 0,
                            "input_tokens": 5,
                            "completion_tokens": 0,
                            "output_tokens": 2,
                            "total_tokens": 7
                        }
                    }
                }),
                json!({"event": "message_stop"}),
            ],
        ];
        for events in cases {
            let mut decoder = QoderSseDecoder::default();
            let mut aggregator = QoderChatSseAggregator::default();
            for event in events {
                aggregator
                    .push(
                        decoder
                            .push(wrapped_qoder_event(200, &event.to_string()))
                            .unwrap(),
                    )
                    .unwrap();
            }
            aggregator.push(decoder.finish().unwrap()).unwrap();
            let response = aggregator.finish("fallback", 9).unwrap();
            assert_eq!(response["usage"]["prompt_tokens"], 5);
            assert_eq!(response["usage"]["completion_tokens"], 2);
            assert_eq!(response["usage"]["total_tokens"], 7);
        }

        let primary = canonical_qoder_usage(Some(&json!({
            "prompt_tokens": 4,
            "input_tokens": 5,
            "completion_tokens": 2,
            "output_tokens": 3,
            "total_tokens": 6,
        })))
        .unwrap()
        .unwrap();
        assert_eq!(primary["prompt_tokens"], 4);
        assert_eq!(primary["completion_tokens"], 2);

        let zero = canonical_qoder_usage(Some(&json!({
            "prompt_tokens": 0,
            "input_tokens": 0,
            "completion_tokens": 0,
            "output_tokens": 0,
            "total_tokens": 0,
        })))
        .unwrap()
        .unwrap();
        assert_eq!(zero["total_tokens"], 0);
    }

    #[test]
    fn qoder_usage_rejects_contradictory_or_overflowed_totals() {
        for usage in [
            json!({"prompt_tokens": 4, "completion_tokens": 3, "total_tokens": 2}),
            json!({"prompt_tokens": u64::MAX, "completion_tokens": 1}),
        ] {
            let inner = json!({
                "choices": [{"index": 0, "delta": {"content": "hello"}, "finish_reason": "stop"}],
                "usage": usage,
            });
            let mut decoder = QoderSseDecoder::default();
            assert!(decoder
                .push(wrapped_qoder_event(200, &inner.to_string()))
                .is_err());
            assert!(!decoder.is_terminal());

            let mut aggregator = QoderChatSseAggregator::default();
            let stream = format!("data: {inner}\n\ndata: [DONE]\n\n");
            assert!(aggregator.push(Bytes::from(stream)).is_err());
            assert!(aggregator.finish("fallback", 9).is_err());
        }

        assert!(merge_qoder_usage(
            Some(
                json!({"prompt_tokens": u64::MAX, "completion_tokens": 0, "total_tokens": u64::MAX})
            ),
            &json!({"prompt_tokens": 0, "completion_tokens": 1, "total_tokens": 1}),
        )
        .is_err());

        assert_eq!(
            canonical_qoder_usage(Some(&json!({
                "prompt_tokens": 4,
                "completion_tokens": 3,
                "total_tokens": 9,
            })))
            .unwrap()
            .unwrap()["total_tokens"],
            9,
        );
    }

    #[test]
    fn qoder_decoder_normalizes_final_messages_aliases_and_placeholders() {
        let inner = json!({
            "id":"chat-final",
            "choices":[{
                "index":"0",
                "message":{
                    "content":[{"type":"text","text":"a"},{"type":"text","text":"b"}],
                    "reasoning_text":"reason",
                    "tool_calls":[
                        {"tool_call_id":"call-a","tool_name":"run","input":{"x":1}},
                        {"type":"function"}
                    ]
                },
                "finish_reason":"tool_calls"
            }],
            "usage":{"input_tokens":"2.0","output_tokens":"3","total_tokens":"5"}
        });
        let mut decoder = QoderSseDecoder::default();
        let canonical = decoder
            .push(wrapped_qoder_event(200, &inner.to_string()))
            .unwrap();
        let text = String::from_utf8(canonical.to_vec()).unwrap();
        assert!(
            text.contains("\\\"content\\\":\\\"a\\\\nb\\\"")
                || text.contains("\"content\":\"a\\nb\"")
        );
        let mut aggregator = QoderChatSseAggregator::default();
        aggregator.push(Bytes::from(text)).unwrap();
        aggregator.push(decoder.finish().unwrap()).unwrap();
        let response = aggregator.finish("fallback", 9).unwrap();
        assert_eq!(response["choices"][0]["message"]["content"], "a\nb");
        assert_eq!(
            response["choices"][0]["message"]["reasoning_content"],
            "reason"
        );
        assert_eq!(
            response["choices"][0]["message"]["tool_calls"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(response["usage"]["total_tokens"], 5);
    }

    #[test]
    fn qoder_decoder_reservations_follow_canonical_bytes_and_release_on_drop() {
        let budget = RequestMemoryBudget::new(2 * 1024 * 1024);
        let mut decoder = QoderSseDecoder::with_request_memory(Some(budget.clone())).unwrap();
        let canonical = decoder
            .push(wrapped_qoder_event(
                200,
                &qoder_business_chunk("bounded", Some("stop")),
            ))
            .unwrap();
        assert!(!canonical.is_empty());
        assert!(budget.snapshot().used_bytes > 0);

        drop(decoder);
        assert!(
            budget.snapshot().used_bytes > 0,
            "canonical Bytes must retain its owner reservation"
        );
        drop(canonical);
        assert_eq!(budget.snapshot().used_bytes, 0);
    }

    #[test]
    fn qoder_aggregator_reservation_follows_finished_value_and_releases_on_drop() {
        let budget = RequestMemoryBudget::new(2 * 1024 * 1024);
        let mut aggregator =
            QoderChatSseAggregator::with_request_memory(Some(budget.clone())).unwrap();
        aggregator
            .push(Bytes::from_static(
                b"data: {\"id\":\"chat-memory\",\"model\":\"auto\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"bounded\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n",
            ))
            .unwrap();
        let response = aggregator.finish_with_memory("auto", 1).unwrap();
        assert_eq!(
            response.value["choices"][0]["message"]["content"],
            "bounded"
        );
        assert!(response.memory.is_some());
        assert!(budget.snapshot().used_bytes > 0);

        drop(response);
        assert_eq!(budget.snapshot().used_bytes, 0);
    }

    #[test]
    fn qoder_decoder_capacity_exhaustion_is_sticky_and_content_free() {
        let budget = RequestMemoryBudget::new(64);
        let mut decoder = QoderSseDecoder::with_request_memory(Some(budget.clone())).unwrap();
        let error = decoder.push(Bytes::from(vec![b'x'; 65])).unwrap_err();
        assert!(error.is_request_memory_exhausted());
        assert_eq!(error.error_code(), "cc_switch_request_memory_exhausted");
        assert!(budget.is_exhausted());
    }

    #[test]
    fn qoder_decoder_rejected_data_after_terminal_cannot_be_finished_as_success() {
        let mut decoder = QoderSseDecoder::default();
        decoder
            .push(Bytes::from_static(b"data: [DONE]\n\n"))
            .unwrap();
        assert!(decoder.push(Bytes::from_static(b"data: {}\n\n")).is_err());
        assert!(decoder.push(Bytes::new()).is_err());
        assert!(decoder.finish().is_err());
        assert!(!decoder.is_terminal());
    }

    #[test]
    fn qoder_aggregator_rejected_data_after_done_cannot_be_finished_as_success() {
        let mut aggregator = QoderChatSseAggregator::default();
        aggregator
            .push(Bytes::from_static(b"data: [DONE]\n\n"))
            .unwrap();
        assert!(aggregator
            .push(Bytes::from_static(b"data: {}\n\n"))
            .is_err());
        assert!(aggregator.push(Bytes::new()).is_err());
        assert!(aggregator.finish("auto", 1).is_err());
    }
}
