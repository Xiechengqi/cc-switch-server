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
    terminal: Option<AnthropicTerminal>,
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
    pub(super) fn push(
        &mut self,
        chunk: &[u8],
    ) -> Result<Vec<AnthropicObservation>, AnthropicProtocolError> {
        if self.terminal.is_some() && chunk.iter().any(|byte| !byte.is_ascii_whitespace()) {
            return Err(AnthropicProtocolError::new(
                "Anthropic stream contained data after its terminal event",
            ));
        }
        self.pending.extend_from_slice(chunk);
        if self.pending.len() > MAX_EVENT_BYTES && next_event_boundary(&self.pending).is_none() {
            return Err(AnthropicProtocolError::new(
                "Anthropic SSE event exceeded the 8 MiB limit",
            ));
        }

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
        if self.terminal.is_some() && self.pending.iter().any(|byte| !byte.is_ascii_whitespace()) {
            return Err(AnthropicProtocolError::new(
                "Anthropic stream contained data after its terminal event",
            ));
        }
        Ok(observations)
    }

    pub(super) fn finish(&mut self) -> Result<(), AnthropicProtocolError> {
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
        self.terminal.as_ref()
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
        for raw_line in text.split('\n') {
            let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
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
            return Err(AnthropicProtocolError::new(format!(
                "Anthropic SSE event type mismatch: event={:?}, data.type={payload_type}",
                declared_event.as_deref().unwrap_or_default()
            )));
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
                self.saw_message_start = true;
                Ok(Some(AnthropicObservation::Lifecycle))
            }
            "ping" => Ok(Some(AnthropicObservation::Lifecycle)),
            "content_block_start"
            | "content_block_delta"
            | "content_block_stop"
            | "message_delta" => {
                self.require_message_start(payload_type)?;
                Ok(Some(AnthropicObservation::Business))
            }
            "message_stop" => {
                self.require_message_start(payload_type)?;
                self.terminal = Some(AnthropicTerminal::Success);
                Ok(Some(AnthropicObservation::SuccessTerminal))
            }
            other => Err(AnthropicProtocolError::new(format!(
                "Anthropic stream contained unknown event type: {other}"
            ))),
        }
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

fn next_event_boundary(bytes: &[u8]) -> Option<(usize, usize)> {
    let lf = bytes
        .windows(2)
        .position(|window| window == b"\n\n")
        .map(|end| (end, 2));
    let crlf = bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|end| (end, 4));
    let cr = bytes
        .windows(2)
        .position(|window| window == b"\r\r")
        .map(|end| (end, 2));
    [lf, crlf, cr]
        .into_iter()
        .flatten()
        .min_by_key(|(end, _)| *end)
}

#[cfg(test)]
mod tests {
    use super::*;

    const START: &str = "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[],\"usage\":{\"input_tokens\":1,\"output_tokens\":0}}}\n\n";

    #[test]
    fn inspector_handles_every_chunk_boundary_and_crlf() {
        let stream = format!(
            "{}event: message_delta\r\ndata: {{\"type\":\"message_delta\",\r\ndata: \"delta\":{{}}}}\r\n\r\nevent: message_stop\r\ndata: {{\"type\":\"message_stop\"}}\r\n\r\n",
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
    fn inspector_rejects_events_after_message_stop() {
        let mut inspector = AnthropicSseInspector::default();
        inspector.push(START.as_bytes()).unwrap();
        inspector
            .push(b"event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n")
            .unwrap();
        assert!(inspector
            .push(b"event: ping\ndata: {\"type\":\"ping\"}\n\n")
            .is_err());

        let mut same_chunk = AnthropicSseInspector::default();
        same_chunk.push(START.as_bytes()).unwrap();
        assert!(same_chunk
            .push(b"event: message_stop\ndata: {\"type\":\"message_stop\"}\n\ntrailing-data")
            .is_err());
    }

    #[test]
    fn json_inspector_validates_messages_and_count_tokens() {
        let message = br#"{"id":"msg_1","type":"message","role":"assistant","content":[],"usage":{"input_tokens":1,"output_tokens":2}}"#;
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
}
