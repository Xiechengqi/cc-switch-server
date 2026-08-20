use std::collections::BTreeMap;

use axum::http::StatusCode;
use serde_json::Value;

use crate::domain::usage::store::{
    usage_from_json_with_semantics, InputTokenSemantics, TokenUsage,
};

use super::ProxyError;

const DEFAULT_MAX_STREAM_EVENT_BYTES: usize = 128 * 1024 * 1024;

#[derive(Debug, Default)]
pub struct SseLineBuffer {
    buffer: String,
}

impl SseLineBuffer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push_chunk(&mut self, chunk: &[u8]) -> Vec<String> {
        self.buffer.push_str(&String::from_utf8_lossy(chunk));
        let mut lines = Vec::new();
        while let Some(pos) = self.buffer.find('\n') {
            let line = self.buffer[..pos].trim_end_matches('\r').to_string();
            self.buffer.drain(..=pos);
            if !line.is_empty() {
                lines.push(line);
            }
        }
        lines
    }

    pub fn finish(self) -> Option<String> {
        let tail = self.buffer.trim_end_matches('\r').trim().to_string();
        if tail.is_empty() {
            None
        } else {
            Some(tail)
        }
    }
}

#[derive(Debug)]
pub struct StreamUsageAccumulator {
    decoder: JsonStreamEventDecoder,
    usage: TokenUsage,
    input_semantics: InputTokenSemantics,
    parse_error: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct StreamUsageResult {
    pub usage: TokenUsage,
    pub parse_error: bool,
}

#[derive(Debug)]
pub struct ResponsesSseAggregation {
    pub response: Value,
    pub stream_status: &'static str,
}

#[derive(Debug)]
pub struct GeminiV1InternalSseAggregator {
    decoder: JsonStreamEventDecoder,
    last_response: Option<Value>,
    candidates: BTreeMap<i64, GeminiV1InternalCandidate>,
    retained_bytes: usize,
}

#[derive(Debug)]
struct GeminiV1InternalCandidate {
    latest: Value,
    parts: Vec<Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponsesSseAggregationErrorKind {
    ParseError,
    MissingTerminal,
    UpstreamFailure,
    Capacity,
}

#[derive(Debug)]
pub struct ResponsesSseAggregationError {
    pub kind: ResponsesSseAggregationErrorKind,
    error: ProxyError,
}

impl ResponsesSseAggregationError {
    fn new(kind: ResponsesSseAggregationErrorKind, error: ProxyError) -> Self {
        Self { kind, error }
    }

    pub fn into_proxy_error(self) -> ProxyError {
        self.error
    }

    #[cfg(test)]
    fn status(&self) -> StatusCode {
        self.error.status
    }
}

#[derive(Debug)]
pub struct ResponsesSseAggregator {
    decoder: JsonStreamEventDecoder,
    response: Option<Value>,
    response_bytes: usize,
    output_items: BTreeMap<u64, Value>,
    output_item_bytes: usize,
    next_output_index: u64,
    stream_status: Option<&'static str>,
    last_error: Option<Value>,
    max_retained_bytes: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JsonStreamMode {
    Unknown,
    Sse,
    Json,
}

#[derive(Debug)]
struct JsonStreamEvent {
    event: Option<String>,
    value: Value,
}

#[derive(Debug)]
struct JsonStreamEventDecoder {
    mode: JsonStreamMode,
    pending_line: Vec<u8>,
    event_name: Option<String>,
    event_data: Vec<u8>,
    json_data: Vec<u8>,
    max_event_bytes: usize,
}

impl JsonStreamEventDecoder {
    fn new(max_event_bytes: usize) -> Self {
        Self {
            mode: JsonStreamMode::Unknown,
            pending_line: Vec::new(),
            event_name: None,
            event_data: Vec::new(),
            json_data: Vec::new(),
            max_event_bytes: max_event_bytes.max(1),
        }
    }

    fn reset(&mut self) {
        let max_event_bytes = self.max_event_bytes;
        *self = Self::new(max_event_bytes);
    }

    fn push(&mut self, chunk: &[u8]) -> Result<Vec<JsonStreamEvent>, String> {
        let mut events = Vec::new();
        let mut offset = 0;
        while offset < chunk.len() {
            if let Some(relative_newline) = chunk[offset..].iter().position(|byte| *byte == b'\n') {
                let newline = offset + relative_newline;
                self.append_pending(&chunk[offset..newline])?;
                let mut line = std::mem::take(&mut self.pending_line);
                if line.last() == Some(&b'\r') {
                    line.pop();
                }
                self.process_line(&line, &mut events)?;
                offset = newline + 1;
            } else {
                self.append_pending(&chunk[offset..])?;
                break;
            }
        }
        Ok(events)
    }

    fn finish(&mut self) -> Result<Vec<JsonStreamEvent>, String> {
        let mut events = Vec::new();
        if !self.pending_line.is_empty() {
            let mut line = std::mem::take(&mut self.pending_line);
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            self.process_line(&line, &mut events)?;
        }
        if self.mode == JsonStreamMode::Sse {
            self.flush_sse_event(&mut events)?;
        } else if !trim_ascii_whitespace(&self.json_data).is_empty() {
            self.flush_json_document(&mut events, true)?;
        }
        Ok(events)
    }

    fn append_pending(&mut self, bytes: &[u8]) -> Result<(), String> {
        ensure_stream_event_bounded(self.pending_line.len(), bytes.len(), self.max_event_bytes)?;
        self.pending_line.extend_from_slice(bytes);
        Ok(())
    }

    fn process_line(
        &mut self,
        line: &[u8],
        events: &mut Vec<JsonStreamEvent>,
    ) -> Result<(), String> {
        let line = trim_ascii_start(line);
        let sse_field =
            line.starts_with(b"event:") || line.starts_with(b"data:") || line.starts_with(b":");
        if self.mode == JsonStreamMode::Unknown && sse_field {
            self.mode = JsonStreamMode::Sse;
        } else if self.mode == JsonStreamMode::Unknown && !line.is_empty() {
            self.mode = JsonStreamMode::Json;
        }

        match self.mode {
            JsonStreamMode::Unknown => Ok(()),
            JsonStreamMode::Sse => self.process_sse_line(line, events),
            JsonStreamMode::Json => self.process_json_line(line, events),
        }
    }

    fn process_sse_line(
        &mut self,
        line: &[u8],
        events: &mut Vec<JsonStreamEvent>,
    ) -> Result<(), String> {
        if line.is_empty() {
            return self.flush_sse_event(events);
        }
        if line.starts_with(b":") {
            return Ok(());
        }
        if let Some(value) = line.strip_prefix(b"event:") {
            if !self.event_data.is_empty() {
                self.flush_sse_event(events)?;
            }
            let value = trim_one_leading_space(value);
            let event = std::str::from_utf8(value)
                .map_err(|error| format!("stream event name is not UTF-8: {error}"))?;
            self.event_name = (!event.trim().is_empty()).then(|| event.trim().to_string());
            return Ok(());
        }
        if let Some(value) = line.strip_prefix(b"data:") {
            let value = trim_one_leading_space(value);
            let separator = usize::from(!self.event_data.is_empty());
            ensure_stream_event_bounded(
                self.event_data.len(),
                value.len().saturating_add(separator),
                self.max_event_bytes,
            )?;
            if separator == 1 {
                self.event_data.push(b'\n');
            }
            self.event_data.extend_from_slice(value);
        }
        Ok(())
    }

    fn flush_sse_event(&mut self, events: &mut Vec<JsonStreamEvent>) -> Result<(), String> {
        let event = self.event_name.take();
        let payload = std::mem::take(&mut self.event_data);
        let payload = trim_ascii_whitespace(&payload);
        if payload.is_empty() || payload == b"[DONE]" {
            return Ok(());
        }
        let value = serde_json::from_slice::<Value>(payload)
            .map_err(|error| format!("stream SSE data is not valid JSON: {error}"))?;
        events.push(JsonStreamEvent { event, value });
        Ok(())
    }

    fn process_json_line(
        &mut self,
        line: &[u8],
        events: &mut Vec<JsonStreamEvent>,
    ) -> Result<(), String> {
        if line.is_empty() && self.json_data.is_empty() {
            return Ok(());
        }
        let separator = usize::from(!self.json_data.is_empty());
        ensure_stream_event_bounded(
            self.json_data.len(),
            line.len().saturating_add(separator),
            self.max_event_bytes,
        )?;
        if separator == 1 {
            self.json_data.push(b'\n');
        }
        self.json_data.extend_from_slice(line);
        self.flush_json_document(events, false)
    }

    fn flush_json_document(
        &mut self,
        events: &mut Vec<JsonStreamEvent>,
        finish: bool,
    ) -> Result<(), String> {
        let payload = trim_ascii_whitespace(&self.json_data);
        if payload.is_empty() || payload == b"[DONE]" {
            self.json_data.clear();
            return Ok(());
        }
        match serde_json::from_slice::<Value>(payload) {
            Ok(value) => {
                self.json_data.clear();
                events.push(JsonStreamEvent { event: None, value });
                Ok(())
            }
            Err(error) if error.is_eof() && !finish => Ok(()),
            Err(error) => Err(format!("stream JSON data is not valid JSON: {error}")),
        }
    }
}

fn ensure_stream_event_bounded(
    current: usize,
    additional: usize,
    limit: usize,
) -> Result<(), String> {
    if additional <= limit.saturating_sub(current) {
        return Ok(());
    }
    Err(format!("stream event exceeded {limit} bytes"))
}

fn trim_ascii_start(mut value: &[u8]) -> &[u8] {
    while value.first().is_some_and(u8::is_ascii_whitespace) {
        value = &value[1..];
    }
    value
}

fn trim_ascii_whitespace(mut value: &[u8]) -> &[u8] {
    value = trim_ascii_start(value);
    while value.last().is_some_and(u8::is_ascii_whitespace) {
        value = &value[..value.len() - 1];
    }
    value
}

fn trim_one_leading_space(value: &[u8]) -> &[u8] {
    value.strip_prefix(b" ").unwrap_or(value)
}

impl Default for StreamUsageAccumulator {
    fn default() -> Self {
        Self::new(InputTokenSemantics::Auto)
    }
}

#[derive(Debug, Default)]
pub struct ClaudeSseErrorDetector {
    lines: SseLineBuffer,
    current_event: Option<String>,
    current_data: Vec<String>,
    non_error_event_ready: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeSseError {
    pub error_type: String,
    pub message: Option<String>,
}

impl ClaudeSseErrorDetector {
    pub fn push(&mut self, chunk: &[u8]) -> Option<ClaudeSseError> {
        for line in self.lines.push_chunk(chunk) {
            if let Some(error) = self.push_line(&line) {
                return Some(error);
            }
        }
        None
    }

    pub fn prelude_ready(&self) -> bool {
        self.non_error_event_ready
    }

    fn push_line(&mut self, line: &str) -> Option<ClaudeSseError> {
        if let Some(event) = line.strip_prefix("event:").map(str::trim) {
            self.flush_event();
            self.current_event = Some(event.to_string());
            return None;
        }
        if let Some(data) = line.strip_prefix("data:").map(str::trim) {
            self.current_data.push(data.to_string());
            if self.current_event.as_deref() != Some("error") {
                self.non_error_event_ready = true;
            }
            return self.flush_if_error_event();
        }
        None
    }

    fn flush_if_error_event(&mut self) -> Option<ClaudeSseError> {
        if self.current_event.as_deref() != Some("error") {
            return None;
        }
        let payload = self.current_data.join("\n");
        let value = serde_json::from_str::<serde_json::Value>(&payload).ok()?;
        let error_type = value
            .pointer("/error/type")
            .or_else(|| value.get("type"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        let message = value
            .pointer("/error/message")
            .or_else(|| value.get("message"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_string);
        self.flush_event();
        error_type.map(|error_type| ClaudeSseError {
            error_type,
            message,
        })
    }

    fn flush_event(&mut self) {
        self.current_event = None;
        self.current_data.clear();
    }
}

impl StreamUsageAccumulator {
    pub fn new(input_semantics: InputTokenSemantics) -> Self {
        Self::with_max_event_bytes(input_semantics, DEFAULT_MAX_STREAM_EVENT_BYTES)
    }

    fn with_max_event_bytes(input_semantics: InputTokenSemantics, max_event_bytes: usize) -> Self {
        Self {
            decoder: JsonStreamEventDecoder::new(max_event_bytes),
            usage: TokenUsage::default(),
            input_semantics,
            parse_error: false,
        }
    }

    pub fn push(&mut self, chunk: &[u8]) -> TokenUsage {
        match self.decoder.push(chunk) {
            Ok(events) => self.merge_events(events),
            Err(error) => {
                self.parse_error = true;
                self.decoder.reset();
                tracing::debug!(error, "stream usage event parse failed");
            }
        }
        self.usage
    }

    pub fn finish(self) -> TokenUsage {
        self.finish_with_status().usage
    }

    pub fn finish_with_status(mut self) -> StreamUsageResult {
        match self.decoder.finish() {
            Ok(events) => self.merge_events(events),
            Err(error) => {
                self.parse_error = true;
                tracing::debug!(error, "stream usage tail parse failed");
            }
        }
        StreamUsageResult {
            usage: self.usage,
            parse_error: self.parse_error,
        }
    }

    fn merge_events(&mut self, events: Vec<JsonStreamEvent>) {
        for event in events {
            merge_usage(
                &mut self.usage,
                usage_from_json_with_semantics(&event.value, self.input_semantics),
            );
        }
    }
}

impl ResponsesSseAggregator {
    pub fn new() -> Self {
        Self::with_limits(
            DEFAULT_MAX_STREAM_EVENT_BYTES,
            DEFAULT_MAX_STREAM_EVENT_BYTES,
        )
    }

    fn with_limits(max_event_bytes: usize, max_retained_bytes: usize) -> Self {
        Self {
            decoder: JsonStreamEventDecoder::new(max_event_bytes),
            response: None,
            response_bytes: 0,
            output_items: BTreeMap::new(),
            output_item_bytes: 0,
            next_output_index: 0,
            stream_status: None,
            last_error: None,
            max_retained_bytes: max_retained_bytes.max(1),
        }
    }

    pub fn push(&mut self, chunk: &[u8]) -> Result<(), ResponsesSseAggregationError> {
        let events = self.decoder.push(chunk).map_err(stream_aggregation_error)?;
        self.process_events(events)
    }

    pub fn is_terminal(&self) -> bool {
        self.stream_status.is_some()
    }

    pub fn finish(mut self) -> Result<ResponsesSseAggregation, ResponsesSseAggregationError> {
        if !self.is_terminal() {
            let events = self.decoder.finish().map_err(stream_aggregation_error)?;
            self.process_events(events)?;
        }
        if !self.is_terminal() {
            if let Some(error) = self.last_error.take() {
                return Err(stream_terminal_error(&error));
            }
        }
        let stream_status = self.stream_status.ok_or_else(|| {
            ResponsesSseAggregationError::new(
                ResponsesSseAggregationErrorKind::MissingTerminal,
                ProxyError::bad_gateway("OpenAI Responses stream ended before a terminal event"),
            )
        })?;
        let mut response = self.response.ok_or_else(|| {
            ResponsesSseAggregationError::new(
                ResponsesSseAggregationErrorKind::MissingTerminal,
                ProxyError::bad_gateway(
                    "OpenAI Responses stream did not include a response payload",
                ),
            )
        })?;
        if !self.output_items.is_empty() {
            let output = self.output_items.into_values().collect::<Vec<_>>();
            let object = response.as_object_mut().ok_or_else(|| {
                ResponsesSseAggregationError::new(
                    ResponsesSseAggregationErrorKind::ParseError,
                    ProxyError::bad_gateway("OpenAI Responses terminal payload is not an object"),
                )
            })?;
            object.insert("output".to_string(), Value::Array(output));
        }
        Ok(ResponsesSseAggregation {
            response,
            stream_status,
        })
    }

    fn process_events(
        &mut self,
        events: Vec<JsonStreamEvent>,
    ) -> Result<(), ResponsesSseAggregationError> {
        for event in events {
            if self.stream_status.is_some() {
                continue;
            }
            let event_type = event
                .event
                .as_deref()
                .or_else(|| event.value.get("type").and_then(Value::as_str))
                .unwrap_or_default();
            match event_type {
                "response.output_item.done" => self.retain_output_item(&event.value)?,
                "response.completed" => self.retain_terminal_response(event.value, "completed")?,
                "response.incomplete" => {
                    self.retain_terminal_response(event.value, "incomplete")?
                }
                "response.failed" | "response.cancelled" | "response.canceled" => {
                    return Err(stream_terminal_error(&event.value));
                }
                "error" => self.last_error = Some(event.value),
                _ => match event.value.get("status").and_then(Value::as_str) {
                    Some("completed") => self.retain_terminal_response(event.value, "completed")?,
                    Some("incomplete") => {
                        self.retain_terminal_response(event.value, "incomplete")?
                    }
                    Some("failed" | "cancelled" | "canceled") => {
                        return Err(stream_terminal_error(&event.value));
                    }
                    _ => {}
                },
            }
        }
        Ok(())
    }

    fn retain_output_item(&mut self, event: &Value) -> Result<(), ResponsesSseAggregationError> {
        let Some(item) = event.get("item").cloned() else {
            return Ok(());
        };
        let index = event
            .get("output_index")
            .and_then(Value::as_u64)
            .unwrap_or_else(|| {
                let index = self.next_output_index;
                self.next_output_index = self.next_output_index.saturating_add(1);
                index
            });
        let item_bytes = encoded_value_len(&item)?;
        let replaced_bytes = self
            .output_items
            .get(&index)
            .map(encoded_value_len)
            .transpose()?
            .unwrap_or(0);
        let next_output_bytes = self
            .output_item_bytes
            .saturating_sub(replaced_bytes)
            .saturating_add(item_bytes);
        ensure_aggregate_bounded(
            self.response_bytes,
            next_output_bytes,
            self.max_retained_bytes,
        )?;
        self.output_item_bytes = next_output_bytes;
        self.output_items.insert(index, item);
        Ok(())
    }

    fn retain_terminal_response(
        &mut self,
        event: Value,
        stream_status: &'static str,
    ) -> Result<(), ResponsesSseAggregationError> {
        let response = event.get("response").cloned().unwrap_or(event);
        if matches!(
            response.get("status").and_then(Value::as_str),
            Some("failed" | "cancelled" | "canceled")
        ) || response
            .get("error")
            .is_some_and(super::response_semantics::error_value_is_substantive)
        {
            return Err(stream_terminal_error(&response));
        }
        let response_bytes = encoded_value_len(&response)?;
        ensure_aggregate_bounded(
            response_bytes,
            self.output_item_bytes,
            self.max_retained_bytes,
        )?;
        self.response = Some(response);
        self.response_bytes = response_bytes;
        self.stream_status = Some(stream_status);
        Ok(())
    }
}

impl GeminiV1InternalSseAggregator {
    pub fn new() -> Self {
        Self {
            decoder: JsonStreamEventDecoder::new(DEFAULT_MAX_STREAM_EVENT_BYTES),
            last_response: None,
            candidates: BTreeMap::new(),
            retained_bytes: 0,
        }
    }

    pub fn push(&mut self, chunk: &[u8]) -> Result<(), ProxyError> {
        let events = self.decoder.push(chunk).map_err(|error| {
            ProxyError::bad_gateway(format!("Gemini v1internal stream decode failed: {error}"))
        })?;
        self.merge_events(events)
    }

    pub fn finish(mut self) -> Result<Value, ProxyError> {
        let events = self.decoder.finish().map_err(|error| {
            ProxyError::bad_gateway(format!("Gemini v1internal stream decode failed: {error}"))
        })?;
        self.merge_events(events)?;
        let mut response = self.last_response.ok_or_else(|| {
            ProxyError::bad_gateway("Gemini v1internal stream contained no JSON response")
        })?;
        if self.candidates.is_empty() {
            if response
                .pointer("/promptFeedback/blockReason")
                .or_else(|| response.pointer("/prompt_feedback/block_reason"))
                .and_then(Value::as_str)
                .map(str::trim)
                .is_some_and(|reason| !reason.is_empty())
            {
                return Ok(response);
            }
            return Err(ProxyError::bad_gateway(
                "Gemini v1internal stream ended without terminal candidates or blocked prompt feedback",
            ));
        }
        let Value::Object(response_object) = &mut response else {
            return Err(ProxyError::bad_gateway(
                "Gemini v1internal response must be a JSON object",
            ));
        };
        let candidates = self
            .candidates
            .into_values()
            .map(|mut candidate| {
                let candidate_object = candidate.latest.as_object_mut().ok_or_else(|| {
                    ProxyError::bad_gateway("Gemini v1internal candidate must be a JSON object")
                })?;
                if candidate_object
                    .get("finishReason")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .is_none_or(|reason| reason.is_empty())
                {
                    return Err(ProxyError::bad_gateway(
                        "Gemini v1internal stream ended before candidate finishReason",
                    ));
                }
                if !candidate.parts.is_empty() {
                    let content = candidate_object
                        .entry("content".to_string())
                        .or_insert_with(|| serde_json::json!({"role": "model"}));
                    let content_object = content.as_object_mut().ok_or_else(|| {
                        ProxyError::bad_gateway(
                            "Gemini v1internal candidate content must be a JSON object",
                        )
                    })?;
                    content_object.insert(
                        "parts".to_string(),
                        Value::Array(merge_gemini_v1internal_parts(candidate.parts)),
                    );
                }
                Ok(candidate.latest)
            })
            .collect::<Result<Vec<_>, ProxyError>>()?;
        response_object.insert("candidates".to_string(), Value::Array(candidates));
        Ok(response)
    }

    fn merge_events(&mut self, events: Vec<JsonStreamEvent>) -> Result<(), ProxyError> {
        for event in events {
            if let Some(error) = super::adapters::google_embedded_error(&event.value) {
                return Err(error);
            }
            let response = super::adapters::unwrap_gemini_v1internal_value(event.value);
            let response_object = response.as_object().ok_or_else(|| {
                ProxyError::bad_gateway("Gemini v1internal response must be a JSON object")
            })?;
            self.retained_bytes = self
                .retained_bytes
                .saturating_add(serde_json::to_vec(&response).map_or(0, |bytes| bytes.len()));
            if self.retained_bytes > DEFAULT_MAX_STREAM_EVENT_BYTES {
                return Err(ProxyError {
                    status: StatusCode::PAYLOAD_TOO_LARGE,
                    message: "Gemini v1internal aggregate exceeded 128 MiB".to_string(),
                });
            }
            if response
                .get("candidates")
                .is_some_and(|value| !value.is_array())
            {
                return Err(ProxyError::bad_gateway(
                    "Gemini v1internal candidates must be an array",
                ));
            }
            if let Some(candidates) = response_object.get("candidates").and_then(Value::as_array) {
                for (position, candidate) in candidates.iter().enumerate() {
                    if !candidate.is_object() {
                        return Err(ProxyError::bad_gateway(
                            "Gemini v1internal candidate must be a JSON object",
                        ));
                    }
                    let index = candidate
                        .get("index")
                        .and_then(Value::as_i64)
                        .unwrap_or(position as i64);
                    let entry =
                        self.candidates
                            .entry(index)
                            .or_insert_with(|| GeminiV1InternalCandidate {
                                latest: candidate.clone(),
                                parts: Vec::new(),
                            });
                    entry.latest = candidate.clone();
                    if let Some(parts) = candidate
                        .pointer("/content/parts")
                        .and_then(Value::as_array)
                    {
                        entry.parts.extend(parts.iter().cloned());
                    }
                }
            }
            let merged_response = self
                .last_response
                .get_or_insert_with(|| Value::Object(serde_json::Map::new()));
            let merged_object = merged_response.as_object_mut().ok_or_else(|| {
                ProxyError::bad_gateway("Gemini v1internal response must be a JSON object")
            })?;
            for (key, value) in response_object {
                if key != "candidates" {
                    merged_object.insert(key.clone(), value.clone());
                }
            }
        }
        Ok(())
    }
}

fn merge_gemini_v1internal_parts(parts: Vec<Value>) -> Vec<Value> {
    let mut merged: Vec<Value> = Vec::with_capacity(parts.len());
    for part in parts {
        let plain_text = part.as_object().and_then(|object| {
            (object.len() == 1)
                .then(|| object.get("text").and_then(Value::as_str))
                .flatten()
        });
        if let Some(text) = plain_text {
            if let Some(previous) = merged.last_mut().and_then(Value::as_object_mut) {
                if previous.len() == 1 {
                    if let Some(previous_text) =
                        previous.get_mut("text").and_then(|value| value.as_str())
                    {
                        let mut combined = String::with_capacity(previous_text.len() + text.len());
                        combined.push_str(previous_text);
                        combined.push_str(text);
                        previous.insert("text".to_string(), Value::String(combined));
                        continue;
                    }
                }
            }
        }
        merged.push(part);
    }
    merged
}

impl Default for ResponsesSseAggregator {
    fn default() -> Self {
        Self::new()
    }
}

fn encoded_value_len(value: &Value) -> Result<usize, ResponsesSseAggregationError> {
    serde_json::to_vec(value)
        .map(|encoded| encoded.len())
        .map_err(|error| {
            ResponsesSseAggregationError::new(
                ResponsesSseAggregationErrorKind::ParseError,
                ProxyError::bad_gateway(format!("encode aggregated response: {error}")),
            )
        })
}

fn ensure_aggregate_bounded(
    response_bytes: usize,
    output_bytes: usize,
    limit: usize,
) -> Result<(), ResponsesSseAggregationError> {
    if output_bytes <= limit.saturating_sub(response_bytes) {
        return Ok(());
    }
    Err(ResponsesSseAggregationError::new(
        ResponsesSseAggregationErrorKind::Capacity,
        ProxyError {
            status: StatusCode::PAYLOAD_TOO_LARGE,
            message: format!("aggregated OpenAI Responses payload exceeded {limit} bytes"),
        },
    ))
}

fn stream_aggregation_error(error: String) -> ResponsesSseAggregationError {
    ResponsesSseAggregationError::new(
        ResponsesSseAggregationErrorKind::ParseError,
        ProxyError::bad_gateway(format!("invalid OpenAI Responses stream: {error}")),
    )
}

fn stream_terminal_error(value: &Value) -> ResponsesSseAggregationError {
    let message = value
        .pointer("/response/error/message")
        .or_else(|| value.pointer("/error/message"))
        .or_else(|| value.get("message"))
        .and_then(Value::as_str)
        .filter(|message| !message.trim().is_empty())
        .unwrap_or("OpenAI Responses stream reported failure");
    ResponsesSseAggregationError::new(
        ResponsesSseAggregationErrorKind::UpstreamFailure,
        ProxyError::bad_gateway(message),
    )
}

fn merge_usage(target: &mut TokenUsage, next: TokenUsage) {
    let next_has_input = next.input_tokens.is_some()
        || next.cache_read_tokens.is_some()
        || next.cache_creation_tokens.is_some();
    let next_has_output = next.output_tokens.is_some();
    if next.raw_input_tokens.is_some() {
        target.raw_input_tokens = next.raw_input_tokens;
    }
    if next.input_tokens.is_some() {
        target.input_tokens = next.input_tokens;
    }
    if next.output_tokens.is_some() {
        target.output_tokens = next.output_tokens;
    }
    if next.cache_read_tokens.is_some() {
        target.cache_read_tokens = next.cache_read_tokens;
    }
    if next.cache_creation_tokens.is_some() {
        target.cache_creation_tokens = next.cache_creation_tokens;
    }
    if next.total_tokens.is_some()
        && (next_has_input || !next_has_output || target.total_tokens.is_none())
    {
        target.total_tokens = next.total_tokens;
    }
    if next_has_output
        && !next_has_input
        && (target.input_tokens.is_some() || target.output_tokens.is_some())
    {
        target.total_tokens = Some(
            target
                .raw_input_tokens
                .unwrap_or_else(|| {
                    target
                        .input_tokens
                        .unwrap_or(0)
                        .saturating_add(target.cache_read_tokens.unwrap_or(0))
                        .saturating_add(target.cache_creation_tokens.unwrap_or(0))
                })
                .saturating_add(target.output_tokens.unwrap_or(0)),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sse_line_buffer_splits_lines_across_chunks() {
        let mut buffer = SseLineBuffer::new();
        let first = buffer.push_chunk(b"data: {\"choices\":");
        assert!(first.is_empty());
        let second = buffer.push_chunk(b"[{\"delta\":{\"content\":\"hi\"}}]}\n");
        assert_eq!(second.len(), 1);
        assert!(second[0].starts_with("data:"));
    }

    #[test]
    fn sse_line_buffer_handles_crlf_line_endings() {
        let mut buffer = SseLineBuffer::new();
        let lines = buffer.push_chunk(b"event: ping\r\ndata: {}\r\n");
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], "event: ping");
        assert_eq!(lines[1], "data: {}");
    }

    #[test]
    fn sse_line_buffer_finish_returns_trailing_partial_line() {
        let mut buffer = SseLineBuffer::new();
        buffer.push_chunk(b"data: partial");
        assert_eq!(buffer.finish().as_deref(), Some("data: partial"));
    }

    #[test]
    fn sse_line_buffer_ignores_empty_tail() {
        let buffer = SseLineBuffer::new();
        assert!(buffer.finish().is_none());
    }

    #[test]
    fn sse_line_buffer_preserves_multiple_complete_lines_in_one_chunk() {
        let mut buffer = SseLineBuffer::new();
        let lines = buffer.push_chunk(b"line1\nline2\nline3\n");
        assert_eq!(lines, vec!["line1", "line2", "line3"]);
        assert!(buffer.finish().is_none());
    }

    #[test]
    fn sse_line_buffer_splits_mid_utf8_character_safely_via_lossy_decode() {
        let mut buffer = SseLineBuffer::new();
        let emoji = "data: 你好\n";
        let bytes = emoji.as_bytes();
        let split = bytes.len() - 2;
        buffer.push_chunk(&bytes[..split]);
        let lines = buffer.push_chunk(&bytes[split..]);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].starts_with("data:"));
    }

    #[test]
    fn claude_sse_error_detector_extracts_error_type_across_chunks() {
        let mut detector = ClaudeSseErrorDetector::default();
        assert!(detector.push(b"event: error\n").is_none());
        let error_type = detector
            .push(
                br#"data: {"error":{"type":"rate_limit_error","message":"slow down"}}
"#,
            )
            .unwrap();
        assert_eq!(error_type.error_type, "rate_limit_error");
        assert_eq!(error_type.message.as_deref(), Some("slow down"));
    }

    #[test]
    fn claude_sse_error_detector_ignores_non_error_events() {
        let mut detector = ClaudeSseErrorDetector::default();
        assert!(detector
            .push(
                br#"event: message_delta
data: {"type":"message_delta","delta":{"text":"hi"}}
"#
            )
            .is_none());
    }

    #[test]
    fn claude_sse_prelude_waits_for_complete_data_line_across_chunks() {
        let mut detector = ClaudeSseErrorDetector::default();
        assert!(detector
            .push(b"event: message_start\ndata: {\"type\":\"mess")
            .is_none());
        assert!(!detector.prelude_ready());
        assert!(detector.push(b"age_start\"}\n\n").is_none());
        assert!(detector.prelude_ready());
    }

    #[test]
    fn parses_openai_stream_usage_line() {
        let mut parser = StreamUsageAccumulator::default();
        parser.push(
            br#"data: {"choices":[],"usage":{"prompt_tokens":10,"completion_tokens":4,"total_tokens":14}}
"#,
        );
        let usage = parser.finish();

        assert_eq!(usage.input_tokens, Some(10));
        assert_eq!(usage.raw_input_tokens, Some(10));
        assert_eq!(usage.output_tokens, Some(4));
        assert_eq!(usage.total_tokens, Some(14));
    }

    #[test]
    fn parses_claude_message_start_usage() {
        let mut parser = StreamUsageAccumulator::default();
        parser.push(
            br#"event: message_start
data: {"type":"message_start","message":{"usage":{"input_tokens":11,"cache_read_input_tokens":5,"output_tokens":0}}}
"#,
        );
        let usage = parser.finish();

        assert_eq!(usage.input_tokens, Some(11));
        assert_eq!(usage.raw_input_tokens, Some(16));
        assert_eq!(usage.cache_read_tokens, Some(5));
        assert_eq!(usage.total_tokens, Some(16));
    }

    #[test]
    fn parses_codex_responses_completed_usage() {
        let mut parser = StreamUsageAccumulator::default();
        parser.push(
            br#"data: {"type":"response.completed","response":{"usage":{"input_tokens":21,"output_tokens":6,"input_tokens_details":{"cached_tokens":9}}}}
"#,
        );
        let usage = parser.finish();

        assert_eq!(usage.input_tokens, Some(12));
        assert_eq!(usage.output_tokens, Some(6));
        assert_eq!(usage.cache_read_tokens, Some(9));
    }

    #[test]
    fn parses_large_codex_usage_event_without_prefix_truncation() {
        let payload = serde_json::json!({
            "type": "response.completed",
            "response": {
                "status": "completed",
                "padding": "x".repeat(70 * 1024),
                "usage": {
                    "input_tokens": 101,
                    "output_tokens": 17,
                    "input_tokens_details": {"cached_tokens": 41}
                }
            }
        });
        let event = format!("event: response.completed\ndata: {payload}\n\n");
        let split = event.len() / 2;
        let mut parser = StreamUsageAccumulator::default();
        parser.push(&event.as_bytes()[..split]);
        parser.push(&event.as_bytes()[split..]);
        let result = parser.finish_with_status();

        assert!(!result.parse_error);
        assert_eq!(result.usage.input_tokens, Some(60));
        assert_eq!(result.usage.cache_read_tokens, Some(41));
        assert_eq!(result.usage.output_tokens, Some(17));
    }

    #[test]
    fn parses_crlf_multiline_data_and_unterminated_tail() {
        let mut parser = StreamUsageAccumulator::default();
        parser.push(
            b"event: response.completed\r\ndata: {\"type\":\"response.completed\",\r\ndata: \"response\":{\"usage\":{\"input_tokens\":12,\"output_tokens\":3}}}\r\n",
        );
        let result = parser.finish_with_status();

        assert!(!result.parse_error);
        assert_eq!(result.usage.input_tokens, Some(12));
        assert_eq!(result.usage.output_tokens, Some(3));
    }

    #[test]
    fn reports_oversized_usage_event_instead_of_fabricating_zero() {
        let mut parser =
            StreamUsageAccumulator::with_max_event_bytes(InputTokenSemantics::Inclusive, 64);
        parser.push(format!("data: {{\"padding\":\"{}", "x".repeat(80)).as_bytes());
        let result = parser.finish_with_status();

        assert!(result.parse_error);
        assert_eq!(result.usage.input_tokens, None);
        assert_eq!(result.usage.output_tokens, None);
    }

    #[test]
    fn explicit_zero_usage_is_observed_without_parse_error() {
        let mut parser = StreamUsageAccumulator::default();
        parser.push(
            b"data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":0,\"output_tokens\":0}}}\n\n",
        );
        let result = parser.finish_with_status();

        assert!(!result.parse_error);
        assert_eq!(result.usage.input_tokens, Some(0));
        assert_eq!(result.usage.output_tokens, Some(0));
    }

    #[test]
    fn aggregates_responses_sse_output_and_terminal_usage() {
        let stream = concat!(
            "event: response.output_item.done\r\n",
            "data: {\"type\":\"response.output_item.done\",\r\n",
            "data: \"output_index\":1,\"item\":{\"id\":\"second\",\"type\":\"message\"}}\r\n\r\n",
            "event: response.output_item.done\r\n",
            "data: {\"type\":\"response.output_item.done\",\"output_index\":0,\"item\":{\"id\":\"first\",\"type\":\"reasoning\"}}\r\n\r\n",
            "event: response.completed\r\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_1\",\"status\":\"completed\",\"output\":[],\"usage\":{\"input_tokens\":9,\"output_tokens\":2}}}\r\n"
        );
        let mut aggregator = ResponsesSseAggregator::new();
        for chunk in stream.as_bytes().chunks(17) {
            aggregator.push(chunk).unwrap();
        }
        let result = aggregator.finish().unwrap();

        assert_eq!(result.stream_status, "completed");
        assert_eq!(result.response["output"][0]["id"], "first");
        assert_eq!(result.response["output"][1]["id"], "second");
        assert_eq!(result.response["usage"]["input_tokens"], 9);
    }

    #[test]
    fn responses_aggregator_rejects_failure_and_retained_overflow() {
        let mut failed = ResponsesSseAggregator::new();
        failed
            .push(
                b"event: response.failed\ndata: {\"type\":\"response.failed\",\"response\":{\"error\":{\"message\":\"busy\"}}}\n\n",
            )
            .unwrap_err();

        let mut oversized = ResponsesSseAggregator::with_limits(1024, 96);
        let event = format!(
            "data: {{\"type\":\"response.output_item.done\",\"item\":{{\"padding\":\"{}\"}}}}\n\n",
            "x".repeat(96)
        );
        let error = oversized.push(event.as_bytes()).unwrap_err();
        assert_eq!(error.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[test]
    fn responses_aggregator_keeps_reading_after_error_frame() {
        let mut aggregator = ResponsesSseAggregator::new();
        aggregator
            .push(
                b"event: error\ndata: {\"type\":\"error\",\"error\":{\"code\":\"server_is_overloaded\",\"message\":\"overloaded\"}}\n\n",
            )
            .unwrap();
        assert!(!aggregator.is_terminal());
        aggregator
            .push(
                b"event: response.failed\ndata: {\"type\":\"response.failed\",\"response\":{\"error\":{\"message\":\"overloaded\"}}}\n\n",
            )
            .unwrap_err();
    }

    #[test]
    fn responses_aggregator_promotes_error_frame_at_eof() {
        let mut aggregator = ResponsesSseAggregator::new();
        aggregator
            .push(
                b"event: error\ndata: {\"type\":\"error\",\"error\":{\"code\":\"server_is_overloaded\",\"message\":\"overloaded\"}}\n\n",
            )
            .unwrap();
        let error = aggregator.finish().unwrap_err();
        assert_eq!(
            error.kind,
            ResponsesSseAggregationErrorKind::UpstreamFailure
        );
        assert_eq!(error.status(), StatusCode::BAD_GATEWAY);
        let message = error.into_proxy_error().to_string();
        assert!(message.contains("overloaded"), "{message}");
    }

    #[test]
    fn gemini_v1internal_aggregator_merges_wrapped_sse_across_chunks() {
        let stream = concat!(
            "data: {\"response\":{\"candidates\":[{\"index\":0,\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"Hello \"}]}}]}}\r\n\r\n",
            "data: {\"response\":{\"candidates\":[{\"index\":0,\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"world\"}]},\"finishReason\":\"STOP\"}],\"usageMetadata\":{\"promptTokenCount\":3,\"candidatesTokenCount\":2,\"totalTokenCount\":5}}}\r\n\r\n"
        );
        let mut aggregator = GeminiV1InternalSseAggregator::new();
        for chunk in stream.as_bytes().chunks(13) {
            aggregator.push(chunk).unwrap();
        }

        let response = aggregator.finish().unwrap();
        assert_eq!(
            response.pointer("/candidates/0/content/parts"),
            Some(&json!([{"text": "Hello world"}]))
        );
        assert_eq!(
            response.pointer("/candidates/0/finishReason"),
            Some(&json!("STOP"))
        );
        assert_eq!(
            response.pointer("/usageMetadata/promptTokenCount"),
            Some(&json!(3))
        );
        assert_eq!(
            response.pointer("/usageMetadata/candidatesTokenCount"),
            Some(&json!(2))
        );
        assert_eq!(
            response.pointer("/usageMetadata/totalTokenCount"),
            Some(&json!(5))
        );
        assert!(response.get("response").is_none());
    }

    #[test]
    fn gemini_v1internal_aggregator_rejects_top_level_and_wrapped_errors() {
        for event in [
            "data: {\"error\":{\"code\":429,\"status\":\"RESOURCE_EXHAUSTED\",\"message\":\"busy\"}}\n\n",
            "data: {\"response\":{\"error\":{\"code\":403,\"status\":\"PERMISSION_DENIED\",\"message\":\"denied\"}}}\n\n",
        ] {
            let mut aggregator = GeminiV1InternalSseAggregator::new();
            let error = aggregator.push(event.as_bytes()).unwrap_err();
            assert_eq!(error.status, StatusCode::BAD_GATEWAY);
            assert!(error.message.contains("embedded error"));
        }
    }

    #[test]
    fn gemini_v1internal_aggregator_requires_candidate_finish_reason() {
        let mut aggregator = GeminiV1InternalSseAggregator::new();
        aggregator
            .push(
                b"data: {\"response\":{\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"partial\"}]}}]}}\n\n",
            )
            .unwrap();

        let error = aggregator.finish().unwrap_err();
        assert_eq!(error.status, StatusCode::BAD_GATEWAY);
        assert!(error.message.contains("finishReason"));
    }

    #[test]
    fn gemini_v1internal_aggregator_accepts_explicit_prompt_block() {
        let mut aggregator = GeminiV1InternalSseAggregator::new();
        aggregator
            .push(
                b"data: {\"response\":{\"promptFeedback\":{\"blockReason\":\"SAFETY\"},\"usageMetadata\":{\"promptTokenCount\":3}}}\n\n",
            )
            .unwrap();

        let response = aggregator.finish().unwrap();
        assert_eq!(
            response.pointer("/promptFeedback/blockReason"),
            Some(&json!("SAFETY"))
        );
    }

    #[test]
    fn gemini_v1internal_aggregator_rejects_non_terminal_documents() {
        for event in [
            "data: {}\n\n",
            "data: {\"usageMetadata\":{\"totalTokenCount\":1}}\n\n",
            "data: 7\n\n",
        ] {
            let mut aggregator = GeminiV1InternalSseAggregator::new();
            let result = aggregator
                .push(event.as_bytes())
                .and_then(|_| aggregator.finish());
            let error = result.unwrap_err();
            assert_eq!(error.status, StatusCode::BAD_GATEWAY);
        }
    }

    #[test]
    fn responses_aggregator_finishes_at_terminal_and_ignores_empty_error() {
        let mut aggregator = ResponsesSseAggregator::new();
        aggregator
            .push(
                concat!(
                    "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp-terminal\",\"status\":\"completed\",\"error\":{},\"output\":[],\"usage\":{\"input_tokens\":1,\"output_tokens\":2}}}\n\n",
                    "data: {\"trailing\":"
                )
                .as_bytes(),
            )
            .unwrap();

        assert!(aggregator.is_terminal());
        let result = aggregator.finish().unwrap();
        assert_eq!(result.stream_status, "completed");
        assert_eq!(result.response["id"], "resp-terminal");
    }

    #[test]
    fn parses_gemini_stream_usage_metadata() {
        let mut parser = StreamUsageAccumulator::default();
        parser.push(
            br#"{"usageMetadata":{"promptTokenCount":7,"candidatesTokenCount":2,"totalTokenCount":9}}
"#,
        );
        let usage = parser.finish();

        assert_eq!(usage.input_tokens, Some(7));
        assert_eq!(usage.output_tokens, Some(2));
        assert_eq!(usage.total_tokens, Some(9));
    }

    #[test]
    fn stream_usage_keeps_latest_cumulative_gemini_metadata() {
        let mut parser = StreamUsageAccumulator::default();
        parser.push(
            br#"{"usageMetadata":{"promptTokenCount":7,"candidatesTokenCount":2,"totalTokenCount":9}}
{"usageMetadata":{"promptTokenCount":11,"candidatesTokenCount":5,"cachedContentTokenCount":3,"totalTokenCount":16}}
"#,
        );
        let usage = parser.finish();

        assert_eq!(usage.input_tokens, Some(8));
        assert_eq!(usage.output_tokens, Some(5));
        assert_eq!(usage.cache_read_tokens, Some(3));
        assert_eq!(usage.total_tokens, Some(16));
    }

    #[test]
    fn stream_usage_updates_from_claude_message_delta() {
        let mut parser = StreamUsageAccumulator::default();
        parser.push(
            br#"event: message_start
data: {"type":"message_start","message":{"usage":{"input_tokens":180,"cache_read_input_tokens":120,"output_tokens":0}}}
event: message_delta
data: {"type":"message_delta","usage":{"input_tokens":140,"output_tokens":8,"cache_read_input_tokens":90,"cache_creation_input_tokens":4}}
"#,
        );
        let usage = parser.finish();

        assert_eq!(usage.input_tokens, Some(140));
        assert_eq!(usage.output_tokens, Some(8));
        assert_eq!(usage.cache_read_tokens, Some(90));
        assert_eq!(usage.cache_creation_tokens, Some(4));
        assert_eq!(usage.raw_input_tokens, Some(234));
        assert_eq!(usage.total_tokens, Some(242));
    }

    #[test]
    fn output_only_delta_does_not_drop_existing_input_from_total() {
        let mut parser = StreamUsageAccumulator::default();
        parser.push(
            br#"event: message_start
data: {"type":"message_start","message":{"usage":{"input_tokens":11,"output_tokens":0}}}
event: message_delta
data: {"type":"message_delta","usage":{"output_tokens":8}}
"#,
        );
        let usage = parser.finish();

        assert_eq!(usage.input_tokens, Some(11));
        assert_eq!(usage.output_tokens, Some(8));
        assert_eq!(usage.total_tokens, Some(19));
    }

    fn assert_stream_usage(
        chunk: &[u8],
        input: Option<u64>,
        output: Option<u64>,
        cache_read: Option<u64>,
        cache_create: Option<u64>,
        total: Option<u64>,
    ) {
        let mut parser = StreamUsageAccumulator::default();
        parser.push(chunk);
        let usage = parser.finish();
        assert_eq!(usage.input_tokens, input);
        assert_eq!(usage.output_tokens, output);
        assert_eq!(usage.cache_read_tokens, cache_read);
        assert_eq!(usage.cache_creation_tokens, cache_create);
        assert_eq!(usage.total_tokens, total);
    }

    macro_rules! openai_usage_case {
        ($name:ident, $input:literal, $output:literal, $cache:literal) => {
            #[test]
            fn $name() {
                assert_stream_usage(
                    format!(
                        "data: {{\"choices\":[],\"usage\":{{\"prompt_tokens\":{},\"completion_tokens\":{},\"total_tokens\":{},\"prompt_tokens_details\":{{\"cached_tokens\":{}}}}}}}\n",
                        $input,
                        $output,
                        $input + $output,
                        $cache
                    )
                    .as_bytes(),
                    Some(($input as u64).saturating_sub($cache as u64)),
                    Some($output),
                    Some($cache),
                    None,
                    Some($input + $output),
                );
            }
        };
    }

    macro_rules! claude_usage_case {
        ($name:ident, $input:literal, $output:literal, $cache:literal, $write:literal) => {
            #[test]
            fn $name() {
                assert_stream_usage(
                    format!(
                        "event: message_delta\ndata: {{\"type\":\"message_delta\",\"usage\":{{\"input_tokens\":{},\"output_tokens\":{},\"cache_read_input_tokens\":{},\"cache_creation_input_tokens\":{}}}}}\n",
                        $input,
                        $output,
                        $cache,
                        $write
                    )
                    .as_bytes(),
                    Some($input),
                    Some($output),
                    Some($cache),
                    Some($write),
                    Some($input + $cache + $write + $output),
                );
            }
        };
    }

    macro_rules! codex_usage_case {
        ($name:ident, $input:literal, $output:literal, $cache:literal) => {
            #[test]
            fn $name() {
                assert_stream_usage(
                    format!(
                        "data: {{\"type\":\"response.completed\",\"response\":{{\"usage\":{{\"input_tokens\":{},\"output_tokens\":{},\"total_tokens\":{},\"input_tokens_details\":{{\"cached_tokens\":{}}}}}}}}}\n",
                        $input,
                        $output,
                        $input + $output,
                        $cache
                    )
                    .as_bytes(),
                    Some(($input as u64).saturating_sub($cache as u64)),
                    Some($output),
                    Some($cache),
                    None,
                    Some($input + $output),
                );
            }
        };
    }

    macro_rules! gemini_usage_case {
        ($name:ident, $input:literal, $output:literal, $cache:literal) => {
            #[test]
            fn $name() {
                assert_stream_usage(
                    format!(
                        "{{\"usageMetadata\":{{\"promptTokenCount\":{},\"candidatesTokenCount\":{},\"cachedContentTokenCount\":{},\"totalTokenCount\":{}}}}}\n",
                        $input,
                        $output,
                        $cache,
                        $input + $output
                    )
                    .as_bytes(),
                    Some(($input as u64).saturating_sub($cache as u64)),
                    Some($output),
                    Some($cache),
                    None,
                    Some($input + $output),
                );
            }
        };
    }

    openai_usage_case!(server_openai_include_usage_001, 1, 2, 0);
    openai_usage_case!(server_openai_include_usage_002, 3, 5, 1);
    openai_usage_case!(server_openai_include_usage_003, 8, 13, 2);
    openai_usage_case!(server_openai_include_usage_004, 21, 34, 3);
    openai_usage_case!(server_openai_include_usage_005, 55, 89, 5);
    openai_usage_case!(server_openai_include_usage_006, 144, 233, 8);
    openai_usage_case!(server_openai_include_usage_007, 377, 610, 13);
    openai_usage_case!(server_openai_include_usage_008, 987, 1597, 21);
    openai_usage_case!(server_openai_include_usage_009, 10, 1, 9);
    openai_usage_case!(server_openai_include_usage_010, 20, 2, 10);
    openai_usage_case!(server_openai_include_usage_011, 30, 3, 11);
    openai_usage_case!(server_openai_include_usage_012, 40, 4, 12);
    openai_usage_case!(server_openai_include_usage_013, 50, 5, 13);
    openai_usage_case!(server_openai_include_usage_014, 60, 6, 14);
    openai_usage_case!(server_openai_include_usage_015, 70, 7, 15);
    openai_usage_case!(server_openai_include_usage_016, 80, 8, 16);
    openai_usage_case!(server_openai_include_usage_017, 90, 9, 17);
    openai_usage_case!(server_openai_include_usage_018, 100, 10, 18);
    openai_usage_case!(server_openai_include_usage_019, 128, 16, 32);
    openai_usage_case!(server_openai_include_usage_020, 256, 32, 64);
    openai_usage_case!(server_openai_include_usage_021, 512, 64, 128);
    openai_usage_case!(server_openai_include_usage_022, 1024, 128, 256);
    openai_usage_case!(server_openai_include_usage_023, 2048, 256, 512);
    openai_usage_case!(server_openai_include_usage_024, 4096, 512, 1024);

    claude_usage_case!(server_claude_delta_usage_001, 1, 2, 0, 0);
    claude_usage_case!(server_claude_delta_usage_002, 3, 5, 1, 0);
    claude_usage_case!(server_claude_delta_usage_003, 8, 13, 2, 1);
    claude_usage_case!(server_claude_delta_usage_004, 21, 34, 3, 1);
    claude_usage_case!(server_claude_delta_usage_005, 55, 89, 5, 2);
    claude_usage_case!(server_claude_delta_usage_006, 144, 233, 8, 3);
    claude_usage_case!(server_claude_delta_usage_007, 377, 610, 13, 5);
    claude_usage_case!(server_claude_delta_usage_008, 987, 1597, 21, 8);
    claude_usage_case!(server_claude_delta_usage_009, 10, 1, 9, 1);
    claude_usage_case!(server_claude_delta_usage_010, 20, 2, 10, 2);
    claude_usage_case!(server_claude_delta_usage_011, 30, 3, 11, 3);
    claude_usage_case!(server_claude_delta_usage_012, 40, 4, 12, 4);
    claude_usage_case!(server_claude_delta_usage_013, 50, 5, 13, 5);
    claude_usage_case!(server_claude_delta_usage_014, 60, 6, 14, 6);
    claude_usage_case!(server_claude_delta_usage_015, 70, 7, 15, 7);
    claude_usage_case!(server_claude_delta_usage_016, 80, 8, 16, 8);
    claude_usage_case!(server_claude_delta_usage_017, 90, 9, 17, 9);
    claude_usage_case!(server_claude_delta_usage_018, 100, 10, 18, 10);
    claude_usage_case!(server_claude_delta_usage_019, 128, 16, 32, 4);
    claude_usage_case!(server_claude_delta_usage_020, 256, 32, 64, 8);
    claude_usage_case!(server_claude_delta_usage_021, 512, 64, 128, 16);
    claude_usage_case!(server_claude_delta_usage_022, 1024, 128, 256, 32);
    claude_usage_case!(server_claude_delta_usage_023, 2048, 256, 512, 64);
    claude_usage_case!(server_claude_delta_usage_024, 4096, 512, 1024, 128);

    codex_usage_case!(server_codex_response_completed_001, 1, 2, 0);
    codex_usage_case!(server_codex_response_completed_002, 3, 5, 1);
    codex_usage_case!(server_codex_response_completed_003, 8, 13, 2);
    codex_usage_case!(server_codex_response_completed_004, 21, 34, 3);
    codex_usage_case!(server_codex_response_completed_005, 55, 89, 5);
    codex_usage_case!(server_codex_response_completed_006, 144, 233, 8);
    codex_usage_case!(server_codex_response_completed_007, 377, 610, 13);
    codex_usage_case!(server_codex_response_completed_008, 987, 1597, 21);
    codex_usage_case!(server_codex_response_completed_009, 10, 1, 9);
    codex_usage_case!(server_codex_response_completed_010, 20, 2, 10);
    codex_usage_case!(server_codex_response_completed_011, 30, 3, 11);
    codex_usage_case!(server_codex_response_completed_012, 40, 4, 12);
    codex_usage_case!(server_codex_response_completed_013, 50, 5, 13);
    codex_usage_case!(server_codex_response_completed_014, 60, 6, 14);
    codex_usage_case!(server_codex_response_completed_015, 70, 7, 15);
    codex_usage_case!(server_codex_response_completed_016, 80, 8, 16);
    codex_usage_case!(server_codex_response_completed_017, 90, 9, 17);
    codex_usage_case!(server_codex_response_completed_018, 100, 10, 18);
    codex_usage_case!(server_codex_response_completed_019, 128, 16, 32);
    codex_usage_case!(server_codex_response_completed_020, 256, 32, 64);
    codex_usage_case!(server_codex_response_completed_021, 512, 64, 128);
    codex_usage_case!(server_codex_response_completed_022, 1024, 128, 256);
    codex_usage_case!(server_codex_response_completed_023, 2048, 256, 512);
    codex_usage_case!(server_codex_response_completed_024, 4096, 512, 1024);

    gemini_usage_case!(server_gemini_usage_metadata_001, 1, 2, 0);
    gemini_usage_case!(server_gemini_usage_metadata_002, 3, 5, 1);
    gemini_usage_case!(server_gemini_usage_metadata_003, 8, 13, 2);
    gemini_usage_case!(server_gemini_usage_metadata_004, 21, 34, 3);
    gemini_usage_case!(server_gemini_usage_metadata_005, 55, 89, 5);
    gemini_usage_case!(server_gemini_usage_metadata_006, 144, 233, 8);
    gemini_usage_case!(server_gemini_usage_metadata_007, 377, 610, 13);
    gemini_usage_case!(server_gemini_usage_metadata_008, 987, 1597, 21);
    gemini_usage_case!(server_gemini_usage_metadata_009, 10, 1, 9);
    gemini_usage_case!(server_gemini_usage_metadata_010, 20, 2, 10);
    gemini_usage_case!(server_gemini_usage_metadata_011, 30, 3, 11);
    gemini_usage_case!(server_gemini_usage_metadata_012, 40, 4, 12);
    gemini_usage_case!(server_gemini_usage_metadata_013, 50, 5, 13);
    gemini_usage_case!(server_gemini_usage_metadata_014, 60, 6, 14);
    gemini_usage_case!(server_gemini_usage_metadata_015, 70, 7, 15);
    gemini_usage_case!(server_gemini_usage_metadata_016, 80, 8, 16);
    gemini_usage_case!(server_gemini_usage_metadata_017, 90, 9, 17);
    gemini_usage_case!(server_gemini_usage_metadata_018, 100, 10, 18);
    gemini_usage_case!(server_gemini_usage_metadata_019, 128, 16, 32);
    gemini_usage_case!(server_gemini_usage_metadata_020, 256, 32, 64);
    gemini_usage_case!(server_gemini_usage_metadata_021, 512, 64, 128);
    gemini_usage_case!(server_gemini_usage_metadata_022, 1024, 128, 256);
    gemini_usage_case!(server_gemini_usage_metadata_023, 2048, 256, 512);
    gemini_usage_case!(server_gemini_usage_metadata_024, 4096, 512, 1024);
}
