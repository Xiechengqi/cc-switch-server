use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

const MAX_EVENT_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AnthropicError {
    pub(super) error_type: String,
    pub(super) message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum AnthropicObservation {
    Lifecycle,
    Business,
    SuccessTerminal,
    Error(AnthropicError),
}

impl AnthropicObservation {
    pub(super) fn commits_downstream(&self) -> bool {
        true
    }

    pub(super) fn counts_as_business_output(&self) -> bool {
        matches!(self, Self::Business)
    }

    pub(super) fn metric_kind(&self) -> &'static str {
        match self {
            Self::Lifecycle => "lifecycle",
            Self::Business => "business",
            Self::SuccessTerminal => "success_terminal",
            Self::Error(_) => "provider_error",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum AnthropicJsonObservation {
    Success,
    Error(AnthropicError),
}

#[derive(Debug)]
pub(super) struct AnthropicProtocolError(String);

impl AnthropicProtocolError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }

    pub(super) fn metric_kind(&self) -> &'static str {
        let message = self.0.as_str();
        if message.contains("SSE") || message.contains("UTF-8") || message.contains("event:") {
            "framing"
        } else if message.contains("JSON") || message.contains("valid JSON") {
            "json"
        } else if message.contains("usage") || message.contains("tokens") {
            "usage"
        } else if message.contains("content block") || message.contains("content_block") {
            "content_block"
        } else if message.contains("message_start") || message.contains("message_stop") {
            "message_lifecycle"
        } else if message.contains("terminal") || message.contains("ended") {
            "terminal"
        } else {
            "protocol"
        }
    }
}

impl std::fmt::Display for AnthropicProtocolError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for AnthropicProtocolError {}

#[derive(Debug, Default)]
pub(super) struct AnthropicSseInspector {
    pending: Vec<u8>,
    saw_message_start: bool,
    saw_message_delta: bool,
    stop_reason: Option<String>,
    message_id: Option<String>,
    seen_block_indices: BTreeSet<u64>,
    open_blocks: BTreeMap<u64, AnthropicContentBlockState>,
    terminal: Option<AnthropicTerminal>,
    failed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AnthropicContentBlockKind {
    Text,
    Thinking,
    RedactedThinking,
    ToolUse,
    Unknown,
}

#[derive(Debug)]
struct AnthropicContentBlockState {
    kind: AnthropicContentBlockKind,
    partial_json: String,
    saw_partial_json: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum AnthropicTerminal {
    Success,
    Error(AnthropicError),
}

impl AnthropicTerminal {
    pub(super) fn stream_status(&self) -> &'static str {
        match self {
            Self::Success => "completed",
            Self::Error(_) => "provider_failed",
        }
    }
}

impl AnthropicSseInspector {
    pub(super) fn retained_bytes(&self) -> usize {
        let seen_blocks = self.seen_block_indices.len().saturating_mul(
            std::mem::size_of::<u64>().saturating_add(std::mem::size_of::<usize>() * 3),
        );
        let open_blocks = self.open_blocks.iter().fold(0usize, |bytes, (_, block)| {
            bytes
                .saturating_add(std::mem::size_of::<u64>())
                .saturating_add(std::mem::size_of::<AnthropicContentBlockState>())
                .saturating_add(std::mem::size_of::<usize>() * 3)
                .saturating_add(block.partial_json.capacity())
        });
        let terminal = match self.terminal.as_ref() {
            Some(AnthropicTerminal::Error(error)) => error
                .error_type
                .capacity()
                .saturating_add(error.message.as_ref().map_or(0, String::capacity)),
            _ => 0,
        };
        self.pending
            .capacity()
            .saturating_add(self.message_id.as_ref().map_or(0, String::capacity))
            .saturating_add(self.stop_reason.as_ref().map_or(0, String::capacity))
            .saturating_add(seen_blocks)
            .saturating_add(open_blocks)
            .saturating_add(terminal)
    }

    pub(super) fn push(
        &mut self,
        chunk: &[u8],
    ) -> Result<Vec<AnthropicObservation>, AnthropicProtocolError> {
        if self.failed {
            return Err(AnthropicProtocolError::new(
                "Anthropic SSE inspector has failed",
            ));
        }
        let result = self.push_inner(chunk);
        self.failed |= result.is_err();
        result
    }

    fn push_inner(
        &mut self,
        chunk: &[u8],
    ) -> Result<Vec<AnthropicObservation>, AnthropicProtocolError> {
        if self.terminal.is_some() && chunk.iter().any(|byte| !byte.is_ascii_whitespace()) {
            return Err(AnthropicProtocolError::new(
                "Anthropic stream contained data after its terminal event",
            ));
        }
        self.pending.extend_from_slice(chunk);
        let mut observations = Vec::new();
        while let Some((end, delimiter_len)) = next_event_boundary(&self.pending) {
            if end > MAX_EVENT_BYTES {
                return Err(AnthropicProtocolError::new(
                    "Anthropic SSE event exceeded the 8 MiB limit",
                ));
            }
            let event = self.pending[..end].to_vec();
            self.pending.drain(..end + delimiter_len);
            if let Some(observation) = self.inspect_event(&event)? {
                observations.push(observation);
            }
        }
        if self.pending.len() > MAX_EVENT_BYTES {
            return Err(AnthropicProtocolError::new(
                "Anthropic SSE event exceeded the 8 MiB limit",
            ));
        }
        if self.terminal.is_some() && self.pending.iter().any(|byte| !byte.is_ascii_whitespace()) {
            return Err(AnthropicProtocolError::new(
                "Anthropic stream contained data after its terminal event",
            ));
        }
        Ok(observations)
    }

    pub(super) fn finish(&mut self) -> Result<(), AnthropicProtocolError> {
        if self.failed {
            return Err(AnthropicProtocolError::new(
                "Anthropic SSE inspector has failed",
            ));
        }
        let result = self.finish_inner();
        self.failed |= result.is_err();
        result
    }

    fn finish_inner(&mut self) -> Result<(), AnthropicProtocolError> {
        if self.pending.iter().any(|byte| !byte.is_ascii_whitespace()) {
            return Err(AnthropicProtocolError::new(
                "Anthropic stream ended with an incomplete SSE event",
            ));
        }
        self.pending.clear();
        if matches!(self.terminal.as_ref(), Some(AnthropicTerminal::Error(_))) {
            return Ok(());
        }
        if !self.saw_message_start {
            return Err(AnthropicProtocolError::new(
                "Anthropic stream ended before message_start",
            ));
        }
        if self.terminal.is_none() {
            return Err(AnthropicProtocolError::new(
                "Anthropic stream ended before message_stop",
            ));
        }
        Ok(())
    }

    pub(super) fn terminal(&self) -> Option<&AnthropicTerminal> {
        if self.failed {
            None
        } else {
            self.terminal.as_ref()
        }
    }

    pub(super) fn message_id(&self) -> Option<&str> {
        if self.failed {
            None
        } else {
            self.message_id.as_deref()
        }
    }

    fn inspect_event(
        &mut self,
        event: &[u8],
    ) -> Result<Option<AnthropicObservation>, AnthropicProtocolError> {
        let text = std::str::from_utf8(event).map_err(|error| {
            AnthropicProtocolError::new(format!("Anthropic SSE event is not valid UTF-8: {error}"))
        })?;
        let mut declared_event = None;
        let mut data = Vec::new();
        for line in text.split(['\r', '\n']) {
            if line.is_empty() || line.starts_with(':') {
                continue;
            }
            if let Some(value) = sse_field(line, "event") {
                declared_event = Some(value.to_string());
            } else if let Some(value) = sse_field(line, "data") {
                data.push(value);
            }
        }
        if data.is_empty() {
            return Ok(None);
        }
        let payload = data.join("\n");
        let value = serde_json::from_str::<Value>(&payload).map_err(|error| {
            AnthropicProtocolError::new(format!("Anthropic SSE data is not valid JSON: {error}"))
        })?;
        let payload_type = required_string(&value, "type", "Anthropic SSE payload")?;
        if declared_event
            .as_deref()
            .is_some_and(|event| event != payload_type)
        {
            return Err(AnthropicProtocolError::new(
                "Anthropic SSE event type does not match data.type",
            ));
        }
        if self.terminal.is_some() {
            return Err(AnthropicProtocolError::new(
                "Anthropic stream contained an event after its terminal event",
            ));
        }

        match payload_type {
            "error" => {
                let error = anthropic_error(&value);
                self.terminal = Some(AnthropicTerminal::Error(error.clone()));
                Ok(Some(AnthropicObservation::Error(error)))
            }
            "message_start" => {
                if self.saw_message_start {
                    return Err(AnthropicProtocolError::new(
                        "Anthropic stream contained duplicate message_start",
                    ));
                }
                validate_message(value.get("message").ok_or_else(|| {
                    AnthropicProtocolError::new("Anthropic message_start is missing message")
                })?)?;
                self.message_id = value
                    .pointer("/message/id")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                self.saw_message_start = true;
                Ok(Some(AnthropicObservation::Lifecycle))
            }
            "ping" => Ok(Some(AnthropicObservation::Lifecycle)),
            "content_block_start" => {
                self.require_message_start(payload_type)?;
                self.require_content_before_stop_reason()?;
                self.inspect_content_block_start(&value)?;
                Ok(Some(AnthropicObservation::Business))
            }
            "content_block_delta" => {
                self.require_message_start(payload_type)?;
                self.require_content_before_stop_reason()?;
                self.inspect_content_block_delta(&value)?;
                Ok(Some(AnthropicObservation::Business))
            }
            "content_block_stop" => {
                self.require_message_start(payload_type)?;
                self.require_content_before_stop_reason()?;
                self.inspect_content_block_stop(&value)?;
                Ok(Some(AnthropicObservation::Business))
            }
            "message_delta" => {
                self.require_message_start(payload_type)?;
                if let Some(reason) = validate_message_delta(&value)? {
                    if self
                        .stop_reason
                        .as_deref()
                        .is_some_and(|previous| previous != reason)
                    {
                        return Err(AnthropicProtocolError::new(
                            "Anthropic message_delta changed stop_reason",
                        ));
                    }
                    self.stop_reason = Some(reason.to_string());
                }
                self.saw_message_delta = true;
                Ok(Some(AnthropicObservation::Business))
            }
            "message_stop" => {
                self.require_message_start(payload_type)?;
                if !self.saw_message_delta {
                    return Err(AnthropicProtocolError::new(
                        "Anthropic message_stop arrived before message_delta",
                    ));
                }
                if !self.open_blocks.is_empty() {
                    return Err(AnthropicProtocolError::new(
                        "Anthropic message_stop arrived with open content blocks",
                    ));
                }
                self.terminal = Some(AnthropicTerminal::Success);
                Ok(Some(AnthropicObservation::SuccessTerminal))
            }
            _ => {
                self.require_message_start("unknown event")?;
                Ok(Some(AnthropicObservation::Business))
            }
        }
    }

    fn inspect_content_block_start(&mut self, value: &Value) -> Result<(), AnthropicProtocolError> {
        let index = required_index(value, "content_block_start")?;
        if !self.seen_block_indices.insert(index) {
            return Err(AnthropicProtocolError::new(format!(
                "Anthropic content block index {index} was started more than once"
            )));
        }
        let block = value.get("content_block").ok_or_else(|| {
            AnthropicProtocolError::new("Anthropic content_block_start is missing content_block")
        })?;
        let block_type = required_string(block, "type", "Anthropic content block")?;
        let kind = match block_type {
            "text" => AnthropicContentBlockKind::Text,
            "thinking" => AnthropicContentBlockKind::Thinking,
            "redacted_thinking" => {
                validate_redacted_thinking_block(block)?;
                AnthropicContentBlockKind::RedactedThinking
            }
            "tool_use" | "server_tool_use" => {
                required_string(block, "id", "Anthropic tool content block")?;
                required_string(block, "name", "Anthropic tool content block")?;
                if !block.get("input").is_some_and(Value::is_object) {
                    return Err(AnthropicProtocolError::new(
                        "Anthropic tool content block input must be an object",
                    ));
                }
                AnthropicContentBlockKind::ToolUse
            }
            _ => AnthropicContentBlockKind::Unknown,
        };
        self.open_blocks.insert(
            index,
            AnthropicContentBlockState {
                kind,
                partial_json: String::new(),
                saw_partial_json: false,
            },
        );
        Ok(())
    }

    fn inspect_content_block_delta(&mut self, value: &Value) -> Result<(), AnthropicProtocolError> {
        let index = required_index(value, "content_block_delta")?;
        let block = self.open_blocks.get_mut(&index).ok_or_else(|| {
            AnthropicProtocolError::new(format!(
                "Anthropic content block delta references unopened index {index}"
            ))
        })?;
        let delta = value.get("delta").ok_or_else(|| {
            AnthropicProtocolError::new("Anthropic content_block_delta is missing delta")
        })?;
        let delta_type = required_string(delta, "type", "Anthropic content block delta")?;
        if block.kind == AnthropicContentBlockKind::Unknown {
            return Ok(());
        }
        if block.kind == AnthropicContentBlockKind::RedactedThinking {
            return Err(AnthropicProtocolError::new(
                "Anthropic redacted thinking content block cannot have deltas",
            ));
        }
        let compatible = match delta_type {
            "text_delta" => {
                matches!(block.kind, AnthropicContentBlockKind::Text)
                    && delta.get("text").is_some_and(Value::is_string)
            }
            "citations_delta" => {
                matches!(block.kind, AnthropicContentBlockKind::Text)
                    && delta.get("citation").is_some_and(Value::is_object)
            }
            "thinking_delta" => {
                matches!(block.kind, AnthropicContentBlockKind::Thinking)
                    && delta.get("thinking").is_some_and(Value::is_string)
            }
            "signature_delta" => {
                matches!(block.kind, AnthropicContentBlockKind::Thinking)
                    && delta.get("signature").is_some_and(Value::is_string)
            }
            "input_json_delta" => {
                if !matches!(block.kind, AnthropicContentBlockKind::ToolUse) {
                    false
                } else {
                    let partial_json = delta
                        .get("partial_json")
                        .and_then(Value::as_str)
                        .ok_or_else(|| {
                            AnthropicProtocolError::new(
                                "Anthropic input_json_delta is missing partial_json",
                            )
                        })?;
                    block.saw_partial_json = true;
                    block.partial_json.push_str(partial_json);
                    true
                }
            }
            _ => false,
        };
        if !compatible {
            let delta_type = match delta_type {
                "text_delta" | "citations_delta" | "thinking_delta" | "signature_delta"
                | "input_json_delta" => delta_type,
                _ => "unknown delta",
            };
            return Err(AnthropicProtocolError::new(format!(
                "Anthropic {delta_type} is incompatible with content block index {index}"
            )));
        }
        Ok(())
    }

    fn inspect_content_block_stop(&mut self, value: &Value) -> Result<(), AnthropicProtocolError> {
        let index = required_index(value, "content_block_stop")?;
        let block = self.open_blocks.remove(&index).ok_or_else(|| {
            AnthropicProtocolError::new(format!(
                "Anthropic content block stop references unopened index {index}"
            ))
        })?;
        if block.kind == AnthropicContentBlockKind::ToolUse && block.saw_partial_json {
            let parsed = serde_json::from_str::<Value>(&block.partial_json).map_err(|error| {
                AnthropicProtocolError::new(format!(
                    "Anthropic tool input JSON for content block {index} is incomplete: {error}"
                ))
            })?;
            if !parsed.is_object() {
                return Err(AnthropicProtocolError::new(format!(
                    "Anthropic tool input JSON for content block {index} must form an object"
                )));
            }
        }
        Ok(())
    }

    fn require_message_start(&self, event_type: &str) -> Result<(), AnthropicProtocolError> {
        if self.saw_message_start {
            Ok(())
        } else {
            Err(AnthropicProtocolError::new(format!(
                "Anthropic stream emitted {event_type} before message_start"
            )))
        }
    }

    fn require_content_before_stop_reason(&self) -> Result<(), AnthropicProtocolError> {
        if self.stop_reason.is_some() {
            Err(AnthropicProtocolError::new(
                "Anthropic content block arrived after stop_reason",
            ))
        } else {
            Ok(())
        }
    }
}

fn required_index(value: &Value, event_type: &str) -> Result<u64, AnthropicProtocolError> {
    value.get("index").and_then(Value::as_u64).ok_or_else(|| {
        AnthropicProtocolError::new(format!(
            "Anthropic {event_type} is missing a non-negative integer index"
        ))
    })
}

fn validate_message_delta(value: &Value) -> Result<Option<&str>, AnthropicProtocolError> {
    let delta = value
        .get("delta")
        .filter(|delta| delta.is_object())
        .ok_or_else(|| {
            AnthropicProtocolError::new("Anthropic message_delta is missing delta object")
        })?;
    let stop_reason = validate_stop_fields(delta, "Anthropic message_delta")?;
    let usage = value
        .get("usage")
        .filter(|usage| usage.is_object())
        .ok_or_else(|| AnthropicProtocolError::new("Anthropic message_delta is missing usage"))?;
    if usage.get("output_tokens").and_then(Value::as_u64).is_none() {
        return Err(AnthropicProtocolError::new(
            "Anthropic message_delta usage is missing non-negative output_tokens",
        ));
    }
    for field in [
        "input_tokens",
        "cache_creation_input_tokens",
        "cache_read_input_tokens",
    ] {
        if usage.get(field).is_some() && usage.get(field).and_then(Value::as_u64).is_none() {
            return Err(AnthropicProtocolError::new(format!(
                "Anthropic message_delta usage contains invalid {field}"
            )));
        }
    }
    Ok(stop_reason)
}

pub(super) fn inspect_json_document(
    body: &[u8],
    count_tokens: bool,
) -> Result<AnthropicJsonObservation, AnthropicProtocolError> {
    let value = serde_json::from_slice::<Value>(body).map_err(|error| {
        AnthropicProtocolError::new(format!("Anthropic body is not valid JSON: {error}"))
    })?;
    if value.get("type").and_then(Value::as_str) == Some("error") {
        return Ok(AnthropicJsonObservation::Error(anthropic_error(&value)));
    }
    if count_tokens {
        let valid = value.get("input_tokens").and_then(Value::as_u64).is_some();
        if !valid {
            return Err(AnthropicProtocolError::new(
                "Anthropic count_tokens response is missing a non-negative input_tokens value",
            ));
        }
    } else {
        validate_message(&value)?;
    }
    Ok(AnthropicJsonObservation::Success)
}

fn validate_message(value: &Value) -> Result<(), AnthropicProtocolError> {
    if value.get("type").and_then(Value::as_str) != Some("message") {
        return Err(AnthropicProtocolError::new(
            "Anthropic response type must be message",
        ));
    }
    required_string(value, "id", "Anthropic message")?;
    required_string(value, "model", "Anthropic message")?;
    validate_stop_fields(value, "Anthropic message")?;
    if value.get("role").and_then(Value::as_str) != Some("assistant") {
        return Err(AnthropicProtocolError::new(
            "Anthropic message role must be assistant",
        ));
    }
    if !value.get("content").is_some_and(Value::is_array) {
        return Err(AnthropicProtocolError::new(
            "Anthropic message content must be an array",
        ));
    }
    if let Some(blocks) = value.get("content").and_then(Value::as_array) {
        for block in blocks {
            validate_message_content_block(block)?;
        }
    }
    let usage = value
        .get("usage")
        .filter(|usage| usage.is_object())
        .ok_or_else(|| AnthropicProtocolError::new("Anthropic message is missing usage"))?;
    for field in ["input_tokens", "output_tokens"] {
        if usage.get(field).and_then(Value::as_u64).is_none() {
            return Err(AnthropicProtocolError::new(format!(
                "Anthropic message usage is missing non-negative {field}"
            )));
        }
    }
    Ok(())
}

fn validate_message_content_block(block: &Value) -> Result<(), AnthropicProtocolError> {
    let block_type = required_string(block, "type", "Anthropic message content block")?;
    match block_type {
        "text" if !block.get("text").is_some_and(Value::is_string) => Err(
            AnthropicProtocolError::new("Anthropic text content block text must be a string"),
        ),
        "thinking" if !block.get("thinking").is_some_and(Value::is_string) => {
            Err(AnthropicProtocolError::new(
                "Anthropic thinking content block thinking must be a string",
            ))
        }
        "tool_use" | "server_tool_use" => {
            required_string(block, "id", "Anthropic tool content block")?;
            required_string(block, "name", "Anthropic tool content block")?;
            if !block.get("input").is_some_and(Value::is_object) {
                return Err(AnthropicProtocolError::new(
                    "Anthropic tool content block input must be an object",
                ));
            }
            Ok(())
        }
        "redacted_thinking" => validate_redacted_thinking_block(block),
        _ => Ok(()),
    }
}

fn validate_redacted_thinking_block(block: &Value) -> Result<(), AnthropicProtocolError> {
    required_string(block, "data", "Anthropic redacted thinking block")?;
    Ok(())
}

fn validate_stop_fields<'a>(
    value: &'a Value,
    context: &str,
) -> Result<Option<&'a str>, AnthropicProtocolError> {
    let stop_reason = match value.get("stop_reason") {
        None | Some(Value::Null) => None,
        Some(Value::String(reason)) if !reason.trim().is_empty() => Some(reason.as_str()),
        Some(_) => {
            return Err(AnthropicProtocolError::new(format!(
                "{context} has invalid stop_reason"
            )))
        }
    };
    if value
        .get("stop_sequence")
        .is_some_and(|sequence| !sequence.is_null() && !sequence.is_string())
    {
        return Err(AnthropicProtocolError::new(format!(
            "{context} has invalid stop_sequence"
        )));
    }
    Ok(stop_reason)
}

fn anthropic_error(value: &Value) -> AnthropicError {
    let error = value.get("error").unwrap_or(value);
    AnthropicError {
        error_type: error
            .get("type")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("api_error")
            .to_string(),
        message: error
            .get("message")
            .and_then(Value::as_str)
            .map(str::to_string),
    }
}

fn required_string<'a>(
    value: &'a Value,
    field: &str,
    context: &str,
) -> Result<&'a str, AnthropicProtocolError> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            AnthropicProtocolError::new(format!("{context} is missing non-empty {field}"))
        })
}

fn sse_field<'a>(line: &'a str, field: &str) -> Option<&'a str> {
    let value = line.strip_prefix(field)?.strip_prefix(':')?;
    Some(value.strip_prefix(' ').unwrap_or(value))
}

pub(super) fn next_event_boundary(bytes: &[u8]) -> Option<(usize, usize)> {
    let mut index = 0;
    while index < bytes.len() {
        let first_len = match bytes[index] {
            b'\r' if bytes.get(index + 1) == Some(&b'\n') => 2,
            b'\r' | b'\n' => 1,
            _ => {
                index += 1;
                continue;
            }
        };
        let next = index + first_len;
        let second_len = match bytes.get(next) {
            Some(b'\r') if bytes.get(next + 1) == Some(&b'\n') => 2,
            Some(b'\r' | b'\n') => 1,
            _ => {
                index = next;
                continue;
            }
        };
        return Some((index, first_len + second_len));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const START: &str = "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"claude-sonnet-4-6\",\"content\":[],\"usage\":{\"input_tokens\":1,\"output_tokens\":0}}}\n\n";

    #[test]
    fn inspector_handles_every_chunk_boundary_and_crlf() {
        let stream = format!(
            "{}event: message_delta\r\ndata: {{\"type\":\"message_delta\",\r\ndata: \"delta\":{{}},\"usage\":{{\"output_tokens\":1}}}}\r\n\r\nevent: message_stop\r\ndata: {{\"type\":\"message_stop\"}}\r\n\r\n",
            START.replace('\n', "\r\n")
        );
        for split in 0..=stream.len() {
            let mut inspector = AnthropicSseInspector::default();
            inspector.push(&stream.as_bytes()[..split]).unwrap();
            inspector.push(&stream.as_bytes()[split..]).unwrap();
            inspector.finish().unwrap();
            assert_eq!(inspector.terminal(), Some(&AnthropicTerminal::Success));
        }
    }

    #[test]
    fn inspector_accepts_cr_delimited_sse_fields_and_events() {
        let stream = START.replace('\n', "\r")
            + "event: message_delta\rdata: {\"type\":\"message_delta\",\"delta\":{},\"usage\":{\"output_tokens\":0}}\r\r"
            + "event: message_stop\rdata: {\"type\":\"message_stop\"}\r\r";
        for split in 0..=stream.len() {
            let mut inspector = AnthropicSseInspector::default();
            inspector.push(&stream.as_bytes()[..split]).unwrap();
            inspector.push(&stream.as_bytes()[split..]).unwrap();
            inspector.finish().unwrap();
            assert_eq!(inspector.terminal(), Some(&AnthropicTerminal::Success));
        }
    }

    #[test]
    fn inspector_accepts_mixed_sse_blank_line_endings() {
        let start = START.strip_suffix("\n\n").unwrap();
        let stream = format!(
            "{start}\n\r\nevent: message_delta\rdata: {{\"type\":\"message_delta\",\"delta\":{{}},\"usage\":{{\"output_tokens\":0}}}}\r\r\nevent: message_stop\rdata: {{\"type\":\"message_stop\"}}\r\r\n"
        );
        for split in 0..=stream.len() {
            let mut inspector = AnthropicSseInspector::default();
            inspector.push(&stream.as_bytes()[..split]).unwrap();
            inspector.push(&stream.as_bytes()[split..]).unwrap();
            inspector.finish().unwrap();
            assert_eq!(inspector.terminal(), Some(&AnthropicTerminal::Success));
        }
    }

    #[test]
    fn inspector_detects_data_error_without_event_error() {
        let mut inspector = AnthropicSseInspector::default();
        let observations = inspector
            .push(b"data: {\"type\":\"error\",\"error\":{\"type\":\"rate_limit_error\",\"message\":\"slow\"}}\n\n")
            .unwrap();
        assert!(matches!(
            &observations[0],
            AnthropicObservation::Error(error) if error.error_type == "rate_limit_error"
        ));
        inspector.finish().unwrap();
    }

    #[test]
    fn inspector_rejects_empty_truncated_and_unterminated_streams() {
        let mut empty = AnthropicSseInspector::default();
        assert!(empty
            .finish()
            .unwrap_err()
            .to_string()
            .contains("message_start"));

        let mut partial = AnthropicSseInspector::default();
        partial.push(b"event: message_start\ndata: {").unwrap();
        assert!(partial
            .finish()
            .unwrap_err()
            .to_string()
            .contains("incomplete"));

        let mut unterminated = AnthropicSseInspector::default();
        unterminated.push(START.as_bytes()).unwrap();
        assert!(unterminated
            .finish()
            .unwrap_err()
            .to_string()
            .contains("message_stop"));
    }

    #[test]
    fn inspector_rejects_oversized_unfinished_event_after_complete_event_in_same_chunk() {
        let mut inspector = AnthropicSseInspector::default();
        let mut chunk = START.as_bytes().to_vec();
        chunk.extend_from_slice(b"event: content_block_delta\ndata: ");
        chunk.extend(std::iter::repeat_n(b'x', MAX_EVENT_BYTES));
        let error = inspector.push(&chunk).unwrap_err();
        assert!(error.to_string().contains("8 MiB"), "{error}");
        assert!(inspector.terminal().is_none());
    }

    #[test]
    fn inspector_limits_each_event_not_the_total_network_chunk() {
        let mut comment = vec![b':'];
        comment.extend(std::iter::repeat_n(b'x', MAX_EVENT_BYTES / 2));
        comment.extend_from_slice(b"\n\n");
        let mut chunk = comment.clone();
        chunk.extend_from_slice(&comment);
        chunk.extend_from_slice(START.as_bytes());
        chunk.extend_from_slice(
            b"event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{},\"usage\":{\"output_tokens\":0}}\n\n",
        );
        chunk.extend_from_slice(b"event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n");
        let mut inspector = AnthropicSseInspector::default();
        inspector.push(&chunk).unwrap();
        inspector.finish().unwrap();
        assert_eq!(inspector.terminal(), Some(&AnthropicTerminal::Success));
    }

    #[test]
    fn inspector_rejects_message_stop_without_message_delta() {
        let mut inspector = AnthropicSseInspector::default();
        inspector.push(START.as_bytes()).unwrap();
        let error = inspector
            .push(b"event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n")
            .unwrap_err();
        assert!(error.to_string().contains("message_delta"), "{error}");
        assert!(inspector.terminal().is_none());
    }

    #[test]
    fn inspector_rejects_message_delta_without_delta_object() {
        for delta in [None, Some(Value::Null), Some(json!("")), Some(json!([]))] {
            let mut event = json!({
                "type": "message_delta",
                "usage": {"output_tokens": 1}
            });
            if let Some(delta) = delta {
                event["delta"] = delta;
            }
            let mut inspector = AnthropicSseInspector::default();
            inspector.push(START.as_bytes()).unwrap();
            let frame = format!("data: {event}\n\n");
            assert!(inspector.push(frame.as_bytes()).is_err(), "{event}");
            assert!(inspector.terminal().is_none());
        }

        let mut valid = AnthropicSseInspector::default();
        valid.push(START.as_bytes()).unwrap();
        valid
            .push(b"data: {\"type\":\"message_delta\",\"delta\":{},\"usage\":{\"output_tokens\":1}}\n\n")
            .unwrap();
        valid
            .push(b"data: {\"type\":\"message_stop\"}\n\n")
            .unwrap();
        valid.finish().unwrap();
    }

    #[test]
    fn inspector_rejects_invalid_stop_reason_and_content_after_stop_reason() {
        for reason in [Value::Null, json!(false), json!(1), json!(" ")] {
            let mut inspector = AnthropicSseInspector::default();
            inspector.push(START.as_bytes()).unwrap();
            let frame = format!(
                "data: {}\n\n",
                json!({"type":"message_delta","delta":{"stop_reason":reason},"usage":{"output_tokens":1}})
            );
            let result = inspector.push(frame.as_bytes());
            if reason.is_null() {
                assert!(result.is_ok(), "null remains a non-terminal hint");
            } else {
                assert!(result.is_err(), "invalid stop_reason {reason} must fail");
            }
        }

        let mut late = AnthropicSseInspector::default();
        late.push(START.as_bytes()).unwrap();
        late.push(b"data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":1}}\n\n")
            .unwrap();
        assert!(late
            .push(b"data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"late\"}}\n\n")
            .is_err());
        assert!(late.terminal().is_none());
        assert!(late.finish().is_err());

        let mut changed = AnthropicSseInspector::default();
        changed.push(START.as_bytes()).unwrap();
        changed
            .push(b"data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":1}}\n\n")
            .unwrap();
        assert!(changed
            .push(b"data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"},\"usage\":{\"output_tokens\":2}}\n\n")
            .is_err());

        let mut usage_only = AnthropicSseInspector::default();
        usage_only.push(START.as_bytes()).unwrap();
        usage_only
            .push(b"data: {\"type\":\"message_delta\",\"delta\":{},\"usage\":{\"output_tokens\":0}}\n\n")
            .unwrap();
        usage_only
            .push(b"data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"valid\"}}\n\n")
            .unwrap();
        usage_only
            .push(b"data: {\"type\":\"content_block_stop\",\"index\":0}\n\n")
            .unwrap();
        usage_only
            .push(b"data: {\"type\":\"message_stop\"}\n\n")
            .unwrap();
        usage_only.finish().unwrap();
    }

    #[test]
    fn json_message_rejects_invalid_stop_fields() {
        let mut message = json!({
            "id":"msg_1","type":"message","role":"assistant",
            "model":"claude-sonnet-4-6","content":[],
            "usage":{"input_tokens":1,"output_tokens":1},
            "stop_reason":"end_turn","stop_sequence":null
        });
        assert!(inspect_json_document(message.to_string().as_bytes(), false).is_ok());
        for invalid in [json!(false), json!(1), json!(" ")] {
            message["stop_reason"] = invalid;
            assert!(inspect_json_document(message.to_string().as_bytes(), false).is_err());
        }
        message["stop_reason"] = json!("end_turn");
        message["stop_sequence"] = json!({"unexpected":"value"});
        assert!(inspect_json_document(message.to_string().as_bytes(), false).is_err());
    }

    #[test]
    fn inspector_rejects_events_after_message_stop() {
        let mut inspector = AnthropicSseInspector::default();
        inspector.push(START.as_bytes()).unwrap();
        inspector
            .push(b"data: {\"type\":\"message_delta\",\"delta\":{},\"usage\":{\"output_tokens\":0}}\n\n")
            .unwrap();
        inspector
            .push(b"event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n")
            .unwrap();
        assert!(inspector
            .push(b"event: ping\ndata: {\"type\":\"ping\"}\n\n")
            .is_err());

        let mut same_chunk = AnthropicSseInspector::default();
        same_chunk.push(START.as_bytes()).unwrap();
        same_chunk
            .push(b"data: {\"type\":\"message_delta\",\"delta\":{},\"usage\":{\"output_tokens\":0}}\n\n")
            .unwrap();
        assert!(same_chunk
            .push(b"event: message_stop\ndata: {\"type\":\"message_stop\"}\n\ntrailing-data")
            .is_err());
    }

    #[test]
    fn inspector_cannot_restore_success_after_a_push_error() {
        let mut inspector = AnthropicSseInspector::default();
        inspector.push(START.as_bytes()).unwrap();
        inspector
            .push(b"data: {\"type\":\"message_delta\",\"delta\":{},\"usage\":{\"output_tokens\":0}}\n\n")
            .unwrap();
        inspector
            .push(b"data: {\"type\":\"message_stop\"}\n\n")
            .unwrap();
        let first = inspector
            .push(b"data: {\"type\":\"message_stop\"}\n\n")
            .unwrap_err();
        assert!(first.to_string().contains("terminal"));
        assert!(inspector.terminal().is_none());
        assert!(inspector.finish().is_err());

        let mut malformed = AnthropicSseInspector::default();
        malformed.push(START.as_bytes()).unwrap();
        assert!(malformed
            .push(b"data: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":0}}\n\n")
            .is_err());
        assert!(malformed
            .push(b"data: {\"type\":\"message_delta\",\"delta\":{},\"usage\":{\"output_tokens\":0}}\n\ndata: {\"type\":\"message_stop\"}\n\n")
            .is_err());
        assert!(malformed.finish().is_err());
        assert!(malformed.terminal().is_none());
    }

    #[test]
    fn inspector_cannot_repair_a_failed_finish_with_late_events() {
        let mut inspector = AnthropicSseInspector::default();
        inspector.push(START.as_bytes()).unwrap();
        assert!(inspector.finish().is_err());
        assert!(inspector
            .push(b"data: {\"type\":\"message_delta\",\"delta\":{},\"usage\":{\"output_tokens\":0}}\n\ndata: {\"type\":\"message_stop\"}\n\n")
            .is_err());
        assert!(inspector.finish().is_err());
        assert!(inspector.terminal().is_none());
    }

    #[test]
    fn inspector_protocol_errors_do_not_echo_untrusted_event_names() {
        const SECRET: &str = "Bearer claude-protocol-secret";
        let mut mismatched = AnthropicSseInspector::default();
        let frame = format!("event: {SECRET}\ndata: {{\"type\":\"message_start\"}}\n\n");
        let error = mismatched.push(frame.as_bytes()).unwrap_err();
        assert!(!error.to_string().contains(SECRET), "{error}");

        let mut unknown = AnthropicSseInspector::default();
        let frame = format!("data: {{\"type\":\"{SECRET}\"}}\n\n");
        let error = unknown.push(frame.as_bytes()).unwrap_err();
        assert!(!error.to_string().contains(SECRET), "{error}");

        let mut delta = AnthropicSseInspector::default();
        delta.push(START.as_bytes()).unwrap();
        delta
            .push(b"event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n")
            .unwrap();
        let frame = format!("event: content_block_delta\ndata: {{\"type\":\"content_block_delta\",\"index\":0,\"delta\":{{\"type\":\"{SECRET}\"}}}}\n\n");
        let error = delta.push(frame.as_bytes()).unwrap_err();
        assert!(!error.to_string().contains(SECRET), "{error}");
    }

    #[test]
    fn inspector_validates_content_block_lifecycle_and_tool_json() {
        let valid = concat!(
            "event: content_block_start\n",
            "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"tool_1\",\"name\":\"Read\",\"input\":{}}}\n\n",
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"path\\\":\\\"a.txt\\\"}\"}}\n\n",
            "event: content_block_stop\n",
            "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
            "event: message_delta\n",
            "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"},\"usage\":{\"output_tokens\":4}}\n\n",
            "event: message_stop\n",
            "data: {\"type\":\"message_stop\"}\n\n"
        );
        let mut inspector = AnthropicSseInspector::default();
        inspector.push(START.as_bytes()).unwrap();
        inspector.push(valid.as_bytes()).unwrap();
        inspector.finish().unwrap();
    }

    #[test]
    fn inspector_rejects_duplicate_indices_mismatched_deltas_and_open_blocks() {
        let start_text = b"data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n";
        let mut duplicate = AnthropicSseInspector::default();
        duplicate.push(START.as_bytes()).unwrap();
        duplicate.push(start_text).unwrap();
        assert!(duplicate.push(start_text).is_err());

        let mut mismatch = AnthropicSseInspector::default();
        mismatch.push(START.as_bytes()).unwrap();
        mismatch.push(start_text).unwrap();
        assert!(mismatch
            .push(b"data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{}\"}}\n\n")
            .is_err());

        let mut open = AnthropicSseInspector::default();
        open.push(START.as_bytes()).unwrap();
        open.push(start_text).unwrap();
        assert!(open.push(b"data: {\"type\":\"message_stop\"}\n\n").is_err());
    }

    #[test]
    fn inspector_rejects_known_content_deltas_without_typed_payloads() {
        for (block_type, delta) in [
            ("text", json!({"type": "text_delta"})),
            ("text", json!({"type": "text_delta", "text": 7})),
            ("text", json!({"type": "citations_delta"})),
            (
                "text",
                json!({"type": "citations_delta", "citation": "bad"}),
            ),
            ("thinking", json!({"type": "thinking_delta"})),
            (
                "thinking",
                json!({"type": "thinking_delta", "thinking": null}),
            ),
            ("thinking", json!({"type": "signature_delta"})),
            (
                "thinking",
                json!({"type": "signature_delta", "signature": []}),
            ),
        ] {
            let mut inspector = AnthropicSseInspector::default();
            inspector.push(START.as_bytes()).unwrap();
            let start = format!(
                "data: {}\n\n",
                json!({"type": "content_block_start", "index": 0,
                    "content_block": {"type": block_type}})
            );
            inspector.push(start.as_bytes()).unwrap();
            let frame = format!(
                "data: {}\n\n",
                json!({"type": "content_block_delta", "index": 0, "delta": delta})
            );
            assert!(inspector.push(frame.as_bytes()).is_err(), "{delta}");
        }
    }

    #[test]
    fn inspector_accepts_typed_content_delta_payloads_including_empty_strings() {
        for (block_type, delta) in [
            ("text", json!({"type": "text_delta", "text": ""})),
            ("text", json!({"type": "citations_delta", "citation": {}})),
            (
                "thinking",
                json!({"type": "thinking_delta", "thinking": ""}),
            ),
            (
                "thinking",
                json!({"type": "signature_delta", "signature": ""}),
            ),
        ] {
            let mut inspector = AnthropicSseInspector::default();
            inspector.push(START.as_bytes()).unwrap();
            for event in [
                json!({"type": "content_block_start", "index": 0,
                    "content_block": {"type": block_type}}),
                json!({"type": "content_block_delta", "index": 0, "delta": delta}),
                json!({"type": "content_block_stop", "index": 0}),
                json!({"type": "message_delta", "delta": {},
                    "usage": {"output_tokens": 1}}),
                json!({"type": "message_stop"}),
            ] {
                inspector
                    .push(format!("data: {event}\n\n").as_bytes())
                    .unwrap();
            }
            inspector.finish().unwrap();
        }
    }

    #[test]
    fn inspector_rejects_incomplete_tool_json_and_accepts_future_events() {
        let mut invalid_tool = AnthropicSseInspector::default();
        invalid_tool.push(START.as_bytes()).unwrap();
        invalid_tool
            .push(b"data: {\"type\":\"content_block_start\",\"index\":7,\"content_block\":{\"type\":\"tool_use\",\"id\":\"tool_7\",\"name\":\"Read\",\"input\":{}}}\n\n")
            .unwrap();
        invalid_tool
            .push(b"data: {\"type\":\"content_block_delta\",\"index\":7,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\"}}\n\n")
            .unwrap();
        assert!(invalid_tool
            .push(b"data: {\"type\":\"content_block_stop\",\"index\":7}\n\n")
            .is_err());

        let mut future = AnthropicSseInspector::default();
        future.push(START.as_bytes()).unwrap();
        future
            .push(b"event: message_metadata\ndata: {\"type\":\"message_metadata\",\"future\":true}\n\n")
            .unwrap();
        future
            .push(b"data: {\"type\":\"message_delta\",\"delta\":{},\"usage\":{\"output_tokens\":0}}\n\n")
            .unwrap();
        future
            .push(b"data: {\"type\":\"message_stop\"}\n\n")
            .unwrap();
        future.finish().unwrap();
    }

    #[test]
    fn inspector_allows_known_delta_names_on_future_content_block_types() {
        let mut inspector = AnthropicSseInspector::default();
        inspector.push(START.as_bytes()).unwrap();
        inspector
            .push(b"data: {\"type\":\"content_block_start\",\"index\":9,\"content_block\":{\"type\":\"future_rich_text\"}}\n\n")
            .unwrap();
        inspector
            .push(b"data: {\"type\":\"content_block_delta\",\"index\":9,\"delta\":{\"type\":\"text_delta\",\"text\":\"future\"}}\n\n")
            .unwrap();
        inspector
            .push(b"data: {\"type\":\"content_block_stop\",\"index\":9}\n\n")
            .unwrap();
        inspector
            .push(b"data: {\"type\":\"message_delta\",\"delta\":{},\"usage\":{\"output_tokens\":0}}\n\n")
            .unwrap();
        inspector
            .push(b"data: {\"type\":\"message_stop\"}\n\n")
            .unwrap();
        inspector.finish().unwrap();
    }

    #[test]
    fn redacted_thinking_requires_opaque_data_and_cannot_accept_thinking_deltas() {
        for block in [
            json!({"type": "redacted_thinking"}),
            json!({"type": "redacted_thinking", "data": null}),
            json!({"type": "redacted_thinking", "data": 42}),
            json!({"type": "redacted_thinking", "data": ""}),
        ] {
            let mut inspector = AnthropicSseInspector::default();
            inspector.push(START.as_bytes()).unwrap();
            let event = json!({"type": "content_block_start", "index": 0, "content_block": block});
            let error = inspector
                .push(format!("data: {event}\n\n").as_bytes())
                .unwrap_err();
            assert!(!error.to_string().contains("42"), "{error}");
        }

        let valid_start = json!({"type": "content_block_start", "index": 0,
            "content_block": {"type": "redacted_thinking", "data": "opaque-signature"}});
        let mut invalid_delta = AnthropicSseInspector::default();
        invalid_delta.push(START.as_bytes()).unwrap();
        invalid_delta
            .push(format!("data: {valid_start}\n\n").as_bytes())
            .unwrap();
        let delta = json!({"type": "content_block_delta", "index": 0,
            "delta": {"type": "thinking_delta", "thinking": "not opaque"}});
        assert!(invalid_delta
            .push(format!("data: {delta}\n\n").as_bytes())
            .is_err());

        let mut valid = AnthropicSseInspector::default();
        valid.push(START.as_bytes()).unwrap();
        for event in [
            valid_start,
            json!({"type": "content_block_stop", "index": 0}),
            json!({"type": "message_delta", "delta": {}, "usage": {"output_tokens": 1}}),
            json!({"type": "message_stop"}),
        ] {
            valid.push(format!("data: {event}\n\n").as_bytes()).unwrap();
        }
        valid.finish().unwrap();
        assert_eq!(valid.terminal(), Some(&AnthropicTerminal::Success));
    }

    #[test]
    fn json_redacted_thinking_requires_opaque_data() {
        let mut message = json!({"id": "msg_1", "type": "message", "role": "assistant",
            "model": "claude-sonnet-4-6", "content": [{"type": "redacted_thinking"}],
            "usage": {"input_tokens": 1, "output_tokens": 1}});
        assert!(inspect_json_document(message.to_string().as_bytes(), false).is_err());
        message["content"][0]["data"] = json!("opaque-signature");
        assert_eq!(
            inspect_json_document(message.to_string().as_bytes(), false).unwrap(),
            AnthropicJsonObservation::Success
        );
    }

    #[test]
    fn json_inspector_validates_messages_and_count_tokens() {
        let message = br#"{"id":"msg_1","type":"message","role":"assistant","model":"claude-sonnet-4-6","content":[],"usage":{"input_tokens":1,"output_tokens":2}}"#;
        assert_eq!(
            inspect_json_document(message, false).unwrap(),
            AnthropicJsonObservation::Success
        );
        assert_eq!(
            inspect_json_document(br#"{"input_tokens":7}"#, true).unwrap(),
            AnthropicJsonObservation::Success
        );
        assert!(inspect_json_document(br#"{}"#, false).is_err());
        assert!(inspect_json_document(br#"{}"#, true).is_err());
    }

    #[test]
    fn claude_success_requires_a_non_empty_reported_model() {
        for model in [None, Some(""), Some("   ")] {
            let mut message = serde_json::json!({
                "id": "msg_1", "type": "message", "role": "assistant",
                "content": [], "usage": {"input_tokens": 1, "output_tokens": 0}
            });
            if let Some(model) = model {
                message["model"] = Value::String(model.to_string());
            }
            let body = serde_json::to_vec(&message).unwrap();
            assert!(
                inspect_json_document(&body, false).is_err(),
                "model={model:?}"
            );

            let event = format!(
                "event: message_start\ndata: {}\n\n",
                serde_json::json!({"type": "message_start", "message": message})
            );
            let mut inspector = AnthropicSseInspector::default();
            assert!(inspector.push(event.as_bytes()).is_err(), "model={model:?}");
        }
    }

    #[test]
    fn nonstream_known_content_blocks_require_typed_payloads() {
        for block in [
            json!(42),
            json!({"type":"text","text":17}),
            json!({"type":"text"}),
            json!({"type":"thinking","thinking":{}}),
            json!({"type":"thinking"}),
            json!({"type":"tool_use","name":"lookup","input":{}}),
            json!({"type":"tool_use","id":"tool_1","name":"lookup","input":[]}),
            json!({"type":"server_tool_use","id":"tool_1","name":"search"}),
        ] {
            let message = json!({
                "id":"msg_1","type":"message","role":"assistant","model":"claude-sonnet-4-6",
                "content":[block],"stop_reason":"end_turn",
                "usage":{"input_tokens":1,"output_tokens":1}
            });
            assert!(
                inspect_json_document(message.to_string().as_bytes(), false).is_err(),
                "known content block with malformed fields must not become success: {message}"
            );
        }

        for block in [
            json!({"type":"text","text":""}),
            json!({"type":"thinking","thinking":""}),
            json!({"type":"tool_use","id":"tool_1","name":"lookup","input":{}}),
            json!({"type":"future_content","payload":{"version":1}}),
        ] {
            let message = json!({
                "id":"msg_1","type":"message","role":"assistant","model":"claude-sonnet-4-6",
                "content":[block],"stop_reason":"end_turn",
                "usage":{"input_tokens":1,"output_tokens":1}
            });
            assert!(inspect_json_document(message.to_string().as_bytes(), false).is_ok());
        }
    }
}
