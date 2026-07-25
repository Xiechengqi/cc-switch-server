use serde_json::Value;

const MAX_SEMANTIC_PENDING_BYTES: usize = 2 * 1024 * 1024;
const MAX_SEMANTIC_EVENT_BYTES: usize = 128 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FailureOrigin {
    Client,
    Provider,
}

impl FailureOrigin {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Client => "client",
            Self::Provider => "provider",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SemanticFailure {
    pub(super) origin: FailureOrigin,
    pub(super) code: String,
    pub(super) message: String,
}

impl SemanticFailure {
    pub(super) fn display_message(&self) -> String {
        format!("{}: {}", self.code, self.message)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum SemanticObservation {
    Lifecycle,
    Business,
    SuccessTerminal,
    IncompleteTerminal,
    Failure(SemanticFailure),
}

impl SemanticObservation {
    pub(super) fn metric_kind(&self) -> &'static str {
        match self {
            Self::Lifecycle => "lifecycle",
            Self::Business => "business",
            Self::SuccessTerminal => "success_terminal",
            Self::IncompleteTerminal => "incomplete_terminal",
            Self::Failure(failure) => match failure.origin {
                FailureOrigin::Client => "client_failure",
                FailureOrigin::Provider => "provider_failure",
            },
        }
    }

    pub(super) fn commits_downstream(&self) -> bool {
        !matches!(self, Self::Lifecycle)
    }

    pub(super) fn counts_as_business_output(&self) -> bool {
        matches!(self, Self::Business)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum SemanticTerminal {
    Success,
    Incomplete,
    Failure(SemanticFailure),
}

impl SemanticTerminal {
    pub(super) fn stream_status(&self) -> &'static str {
        match self {
            Self::Success => "completed",
            Self::Incomplete => "incomplete",
            Self::Failure(failure) => match failure.origin {
                FailureOrigin::Client => "client_error",
                FailureOrigin::Provider => "provider_failed",
            },
        }
    }
}

#[derive(Debug)]
pub(super) struct SemanticProtocolError {
    message: String,
}

impl SemanticProtocolError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for SemanticProtocolError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for SemanticProtocolError {}

pub(super) fn semantic_guard_enabled() -> bool {
    std::env::var("CC_SWITCH_PROXY_SEMANTIC_GUARD_ENABLED")
        .map(|value| {
            !matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "0" | "false" | "off" | "no" | "disabled"
            )
        })
        .unwrap_or(true)
}

pub(super) fn classify_json_document(
    body: &[u8],
) -> Result<SemanticObservation, SemanticProtocolError> {
    let value = serde_json::from_slice::<Value>(body).map_err(|error| {
        SemanticProtocolError::new(format!("Responses body is not valid JSON: {error}"))
    })?;
    Ok(classify_value(&value))
}

pub(super) fn classify_value(value: &Value) -> SemanticObservation {
    let event_type = value.get("type").and_then(Value::as_str).unwrap_or("");
    let response = value
        .get("response")
        .filter(|response| response.is_object())
        .unwrap_or(value);
    let status = response.get("status").and_then(Value::as_str);

    if matches!(status, Some("failed" | "cancelled"))
        || non_null_error(response).is_some()
        || event_type == "error"
        || event_type == "response.failed"
        || matches!(event_type, "response.cancelled" | "response.canceled")
    {
        return SemanticObservation::Failure(failure_from_value(value, response, status));
    }

    if event_type == "response.incomplete" || status == Some("incomplete") {
        return SemanticObservation::IncompleteTerminal;
    }
    if event_type == "response.completed" || status == Some("completed") {
        return SemanticObservation::SuccessTerminal;
    }
    if matches!(
        event_type,
        "response.created" | "response.in_progress" | "response.queued"
    ) || matches!(status, Some("queued" | "in_progress"))
    {
        return SemanticObservation::Lifecycle;
    }
    SemanticObservation::Business
}

fn failure_from_value(value: &Value, response: &Value, status: Option<&str>) -> SemanticFailure {
    let error = non_null_error(response)
        .or_else(|| non_null_error(value))
        .unwrap_or(response);
    let code = error
        .get("code")
        .and_then(Value::as_str)
        .or_else(|| error.get("type").and_then(Value::as_str))
        .or(status)
        .or_else(|| value.get("type").and_then(Value::as_str))
        .filter(|code| !code.trim().is_empty())
        .unwrap_or("upstream_error")
        .to_string();
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .or_else(|| error.as_str())
        .filter(|message| !message.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| match status {
            Some("cancelled") => "response generation was cancelled".to_string(),
            _ => "response generation failed".to_string(),
        });
    SemanticFailure {
        origin: classify_failure_origin(&code),
        code,
        message,
    }
}

fn non_null_error(value: &Value) -> Option<&Value> {
    value
        .get("error")
        .filter(|error| error_value_is_substantive(error))
}

pub(super) fn error_value_is_substantive(error: &Value) -> bool {
    match error {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::String(value) => !value.trim().is_empty(),
        Value::Array(value) => !value.is_empty(),
        Value::Object(value) => !value.is_empty(),
        Value::Number(_) => true,
    }
}

fn classify_failure_origin(code: &str) -> FailureOrigin {
    let code = code.trim().to_ascii_lowercase().replace('-', "_");
    if matches!(
        code.as_str(),
        "bad_request"
            | "bad_request_error"
            | "content_filter"
            | "content_filter_error"
            | "content_policy_violation"
            | "context_length_exceeded"
            | "cancelled"
            | "canceled"
            | "client_cancelled"
            | "invalid_argument"
            | "invalid_argument_error"
            | "invalid_request"
            | "invalid_request_error"
            | "missing_required_parameter"
            | "moderation_blocked"
            | "prompt_blocked"
            | "request_too_large"
            | "request_cancelled"
            | "safety_violation"
            | "unsupported_parameter"
            | "unprocessable_entity"
            | "validation_error"
            | "validation_failed"
    ) || code.starts_with("invalid_request_")
        || code.starts_with("invalid_argument_")
        || code.starts_with("validation_")
    {
        FailureOrigin::Client
    } else {
        FailureOrigin::Provider
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StreamEncoding {
    Unknown,
    Sse,
    Json,
}

#[derive(Debug)]
pub(super) struct ResponsesSseInspector {
    encoding: StreamEncoding,
    buffer: Vec<u8>,
    saw_business: bool,
    terminal: Option<SemanticTerminal>,
    done_seen: bool,
}

impl Default for ResponsesSseInspector {
    fn default() -> Self {
        Self {
            encoding: StreamEncoding::Unknown,
            buffer: Vec::new(),
            saw_business: false,
            terminal: None,
            done_seen: false,
        }
    }
}

impl ResponsesSseInspector {
    pub(super) fn push(
        &mut self,
        chunk: &[u8],
    ) -> Result<Vec<SemanticObservation>, SemanticProtocolError> {
        self.push_with_limits(chunk, MAX_SEMANTIC_PENDING_BYTES, MAX_SEMANTIC_EVENT_BYTES)
    }

    fn push_with_limits(
        &mut self,
        chunk: &[u8],
        max_pending_bytes: usize,
        max_event_bytes: usize,
    ) -> Result<Vec<SemanticObservation>, SemanticProtocolError> {
        if chunk.is_empty() {
            return Ok(Vec::new());
        }
        self.buffer.extend_from_slice(chunk);
        self.detect_encoding();
        match self.encoding {
            StreamEncoding::Unknown => self.ensure_bounded(max_pending_bytes).map(|()| Vec::new()),
            StreamEncoding::Json => self.drain_json(false, max_pending_bytes, max_event_bytes),
            StreamEncoding::Sse => self.drain_sse(false, max_pending_bytes, max_event_bytes),
        }
    }

    pub(super) fn finish(&mut self) -> Result<Vec<SemanticObservation>, SemanticProtocolError> {
        self.finish_with_limits(MAX_SEMANTIC_PENDING_BYTES, MAX_SEMANTIC_EVENT_BYTES)
    }

    fn finish_with_limits(
        &mut self,
        max_pending_bytes: usize,
        max_event_bytes: usize,
    ) -> Result<Vec<SemanticObservation>, SemanticProtocolError> {
        self.detect_encoding();
        let observations = match self.encoding {
            StreamEncoding::Unknown if self.buffer.iter().all(u8::is_ascii_whitespace) => {
                self.buffer.clear();
                Vec::new()
            }
            StreamEncoding::Unknown => {
                return Err(SemanticProtocolError::new(
                    "Responses stream ended with an unrecognized payload",
                ))
            }
            StreamEncoding::Json if self.buffer.is_empty() && self.terminal.is_some() => Vec::new(),
            StreamEncoding::Json => self.drain_json(true, max_pending_bytes, max_event_bytes)?,
            StreamEncoding::Sse => self.drain_sse(true, max_pending_bytes, max_event_bytes)?,
        };
        if self.terminal.is_none() {
            return Err(SemanticProtocolError::new(if self.done_seen {
                "Responses stream emitted [DONE] before a terminal response event"
            } else {
                "Responses stream ended before a terminal response event"
            }));
        }
        Ok(observations)
    }

    pub(super) fn saw_business(&self) -> bool {
        self.saw_business
    }

    pub(super) fn terminal(&self) -> Option<&SemanticTerminal> {
        self.terminal.as_ref()
    }

    fn detect_encoding(&mut self) {
        if self.encoding != StreamEncoding::Unknown {
            return;
        }
        let first = self
            .buffer
            .iter()
            .copied()
            .find(|byte| !byte.is_ascii_whitespace());
        self.encoding = match first {
            Some(b'{' | b'[') => StreamEncoding::Json,
            Some(_) if self.buffer.contains(&b'\n') => StreamEncoding::Sse,
            _ => StreamEncoding::Unknown,
        };
    }

    fn drain_json(
        &mut self,
        finish: bool,
        max_pending_bytes: usize,
        max_event_bytes: usize,
    ) -> Result<Vec<SemanticObservation>, SemanticProtocolError> {
        match serde_json::from_slice::<Value>(&self.buffer) {
            Ok(value) => {
                ensure_event_bounded(self.buffer.len(), max_event_bytes)?;
                self.buffer.clear();
                if self.terminal.is_some() {
                    return Ok(Vec::new());
                }
                let observation = classify_value(&value);
                self.record(&observation);
                Ok(vec![observation])
            }
            Err(_) => {
                let mut observations = Vec::new();
                let mut consumed = 0;
                while let Some(relative_end) = self.buffer[consumed..]
                    .iter()
                    .position(|byte| *byte == b'\n')
                {
                    let line_end = consumed + relative_end;
                    let line = self.buffer[consumed..line_end]
                        .strip_suffix(b"\r")
                        .unwrap_or(&self.buffer[consumed..line_end]);
                    let line = trim_ascii_whitespace(line);
                    if line.is_empty() {
                        consumed = line_end + 1;
                        continue;
                    }
                    if line == b"[DONE]" {
                        self.done_seen = true;
                        consumed = line_end + 1;
                        continue;
                    }
                    if self.terminal.is_some() {
                        consumed = line_end + 1;
                        continue;
                    }
                    match serde_json::from_slice::<Value>(line) {
                        Ok(value) => {
                            ensure_event_bounded(line.len(), max_event_bytes)?;
                            let observation = classify_value(&value);
                            self.record(&observation);
                            observations.push(observation);
                            consumed = line_end + 1;
                        }
                        Err(error) if error.is_eof() => break,
                        Err(error) => {
                            ensure_event_bounded(line.len(), max_pending_bytes)?;
                            return Err(SemanticProtocolError::new(format!(
                                "Responses JSON line is invalid: {error}"
                            )));
                        }
                    }
                }
                if consumed > 0 {
                    self.buffer.drain(..consumed);
                }
                if !finish {
                    self.ensure_bounded(max_pending_bytes)?;
                    return Ok(observations);
                }
                let remaining = trim_ascii_whitespace(&self.buffer);
                if remaining.is_empty() {
                    self.buffer.clear();
                    return Ok(observations);
                }
                if remaining == b"[DONE]" {
                    self.done_seen = true;
                    self.buffer.clear();
                    return Ok(observations);
                }
                if self.terminal.is_some() {
                    self.buffer.clear();
                    return Ok(observations);
                }
                let value = match serde_json::from_slice::<Value>(remaining) {
                    Ok(value) => value,
                    Err(error) => {
                        ensure_event_bounded(remaining.len(), max_pending_bytes)?;
                        return Err(SemanticProtocolError::new(format!(
                            "Responses JSON stream is invalid: {error}"
                        )));
                    }
                };
                ensure_event_bounded(remaining.len(), max_event_bytes)?;
                let observation = classify_value(&value);
                self.record(&observation);
                observations.push(observation);
                self.buffer.clear();
                Ok(observations)
            }
        }
    }

    fn drain_sse(
        &mut self,
        finish: bool,
        max_pending_bytes: usize,
        max_event_bytes: usize,
    ) -> Result<Vec<SemanticObservation>, SemanticProtocolError> {
        let mut observations = Vec::new();
        while let Some((event_end, delimiter_len)) = next_event_boundary(&self.buffer) {
            ensure_event_bounded(event_end, max_event_bytes)?;
            let event = self.buffer[..event_end].to_vec();
            self.buffer.drain(..event_end + delimiter_len);
            if let Some(observation) = self.parse_sse_event(&event)? {
                observations.push(observation);
            }
        }
        if finish && !self.buffer.is_empty() {
            ensure_event_bounded(self.buffer.len(), max_event_bytes)?;
            let event = std::mem::take(&mut self.buffer);
            if let Some(observation) = self.parse_sse_event(&event)? {
                observations.push(observation);
            }
        }
        self.ensure_bounded(max_pending_bytes)?;
        Ok(observations)
    }

    fn parse_sse_event(
        &mut self,
        event: &[u8],
    ) -> Result<Option<SemanticObservation>, SemanticProtocolError> {
        let event = std::str::from_utf8(event).map_err(|error| {
            SemanticProtocolError::new(format!("Responses SSE event is not UTF-8: {error}"))
        })?;
        let data = event
            .lines()
            .filter_map(|line| line.trim_end_matches('\r').strip_prefix("data:"))
            .map(str::trim_start)
            .collect::<Vec<_>>();
        let payload = if data.is_empty() {
            let event = event.trim();
            if event.is_empty() || event.starts_with(':') || event.starts_with("event:") {
                return Ok(None);
            }
            event.to_string()
        } else {
            data.join("\n").trim().to_string()
        };
        if payload.is_empty() {
            return Ok(None);
        }
        if payload == "[DONE]" {
            self.done_seen = true;
            return Ok(None);
        }
        if self.terminal.is_some() {
            return Ok(None);
        }
        let value = serde_json::from_str::<Value>(&payload).map_err(|error| {
            SemanticProtocolError::new(format!("Responses SSE data is not valid JSON: {error}"))
        })?;
        let observation = classify_value(&value);
        self.record(&observation);
        Ok(Some(observation))
    }

    fn record(&mut self, observation: &SemanticObservation) {
        match observation {
            SemanticObservation::Lifecycle => {}
            SemanticObservation::Business => self.saw_business = true,
            SemanticObservation::SuccessTerminal => {
                self.terminal.get_or_insert(SemanticTerminal::Success);
            }
            SemanticObservation::IncompleteTerminal => {
                self.terminal.get_or_insert(SemanticTerminal::Incomplete);
            }
            SemanticObservation::Failure(failure) => {
                self.terminal
                    .get_or_insert_with(|| SemanticTerminal::Failure(failure.clone()));
            }
        }
    }

    fn ensure_bounded(&self, max_event_bytes: usize) -> Result<(), SemanticProtocolError> {
        ensure_event_bounded(self.buffer.len(), max_event_bytes)
    }
}

fn ensure_event_bounded(
    event_bytes: usize,
    max_event_bytes: usize,
) -> Result<(), SemanticProtocolError> {
    if event_bytes <= max_event_bytes {
        return Ok(());
    }
    Err(SemanticProtocolError::new(format!(
        "Responses semantic event exceeded {max_event_bytes} bytes"
    )))
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

fn trim_ascii_whitespace(mut value: &[u8]) -> &[u8] {
    while value.first().is_some_and(u8::is_ascii_whitespace) {
        value = &value[1..];
    }
    while value.last().is_some_and(u8::is_ascii_whitespace) {
        value = &value[..value.len() - 1];
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn classifies_lifecycle_business_and_terminals() {
        assert_eq!(
            classify_value(&json!({"type": "response.created"})),
            SemanticObservation::Lifecycle
        );
        assert_eq!(
            classify_value(&json!({"type": "response.output_text.delta", "delta": "hi"})),
            SemanticObservation::Business
        );
        assert_eq!(
            classify_value(
                &json!({"type": "response.completed", "response": {"status": "completed"}})
            ),
            SemanticObservation::SuccessTerminal
        );
        assert_eq!(
            classify_value(&json!({"status": "incomplete", "output": []})),
            SemanticObservation::IncompleteTerminal
        );
    }

    #[test]
    fn separates_client_validation_from_provider_failure() {
        let client = classify_value(&json!({
            "type": "response.failed",
            "response": {"error": {"type": "invalid_request_error", "message": "bad tool"}}
        }));
        assert!(matches!(
            client,
            SemanticObservation::Failure(SemanticFailure {
                origin: FailureOrigin::Client,
                ..
            })
        ));

        let provider = classify_value(&json!({
            "status": "failed",
            "error": {"code": "server_error", "message": "busy"}
        }));
        assert!(matches!(
            provider,
            SemanticObservation::Failure(SemanticFailure {
                origin: FailureOrigin::Provider,
                ..
            })
        ));

        let cancelled = classify_value(&json!({
            "type": "response.cancelled",
            "response": {"status": "cancelled"}
        }));
        assert!(matches!(
            cancelled,
            SemanticObservation::Failure(SemanticFailure {
                origin: FailureOrigin::Client,
                ..
            })
        ));
    }

    #[test]
    fn empty_error_placeholders_do_not_override_response_status() {
        for error in [Value::Null, json!({}), json!([]), json!(""), json!(false)] {
            assert_eq!(
                classify_value(&json!({
                    "type": "response.completed",
                    "response": {"status": "completed", "error": error}
                })),
                SemanticObservation::SuccessTerminal
            );
        }
    }

    #[test]
    fn sse_inspector_handles_split_lifecycle_and_failure() {
        let mut inspector = ResponsesSseInspector::default();
        assert!(inspector
            .push(b"event: response.created\ndata: {\"type\":\"response.created\"}\n")
            .unwrap()
            .is_empty());
        let lifecycle = inspector.push(b"\n").unwrap();
        assert_eq!(lifecycle, vec![SemanticObservation::Lifecycle]);
        let failure = inspector
            .push(
                b"event: response.failed\ndata: {\"type\":\"response.failed\",\"response\":{\"error\":{\"type\":\"server_error\",\"message\":\"busy\"}}}\n\n",
            )
            .unwrap();
        assert!(matches!(
            failure.as_slice(),
            [SemanticObservation::Failure(SemanticFailure {
                origin: FailureOrigin::Provider,
                ..
            })]
        ));
        assert!(matches!(
            inspector.terminal(),
            Some(SemanticTerminal::Failure(_))
        ));
        inspector.finish().unwrap();
    }

    #[test]
    fn sse_inspector_requires_a_semantic_terminal() {
        let mut inspector = ResponsesSseInspector::default();
        let observations = inspector
            .push(b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"hi\"}\n\ndata: [DONE]\n\n")
            .unwrap();
        assert!(inspector.saw_business());
        assert_eq!(observations, vec![SemanticObservation::Business]);
        assert!(inspector
            .finish()
            .unwrap_err()
            .to_string()
            .contains("[DONE]"));
    }

    #[test]
    fn inspector_accepts_whole_json_stream_documents() {
        let mut inspector = ResponsesSseInspector::default();
        assert!(inspector.push(b"{\"status\":").unwrap().is_empty());
        assert_eq!(
            inspector.push(b"\"completed\",\"output\":[]}").unwrap(),
            vec![SemanticObservation::SuccessTerminal]
        );
        inspector.finish().unwrap();
    }

    #[test]
    fn inspector_accepts_line_delimited_json_events() {
        let mut inspector = ResponsesSseInspector::default();
        let observations = inspector
            .push(
                concat!(
                    "{\"type\":\"response.created\"}\n",
                    "{\"type\":\"response.output_text.delta\",\"delta\":\"hi\"}\n",
                    "{\"type\":\"response.completed\",\"response\":{\"status\":\"completed\"}}\n",
                    "[DONE]\n"
                )
                .as_bytes(),
            )
            .unwrap();
        assert_eq!(
            observations,
            vec![
                SemanticObservation::Lifecycle,
                SemanticObservation::Business,
                SemanticObservation::SuccessTerminal
            ]
        );
        inspector.finish().unwrap();
    }

    #[test]
    fn inspector_accepts_line_delimited_terminal_without_trailing_newline() {
        let mut inspector = ResponsesSseInspector::default();
        let observations = inspector
            .push(
                concat!(
                    "{\"type\":\"response.created\"}\n",
                    "{\"type\":\"response.completed\",\"response\":{\"status\":\"completed\"}}"
                )
                .as_bytes(),
            )
            .unwrap();
        assert_eq!(observations, vec![SemanticObservation::Lifecycle]);
        assert_eq!(
            inspector.finish().unwrap(),
            vec![SemanticObservation::SuccessTerminal]
        );
    }

    #[test]
    fn line_delimited_json_ignores_events_after_terminal() {
        let mut inspector = ResponsesSseInspector::default();
        let observations = inspector
            .push(
                concat!(
                    "{\"type\":\"response.completed\",\"response\":{\"status\":\"completed\"}}\n",
                    "{\"type\":\"response.failed\",\"response\":{\"status\":\"failed\",\"error\":{\"type\":\"server_error\"}}}\n"
                )
                .as_bytes(),
            )
            .unwrap();
        assert_eq!(observations, vec![SemanticObservation::SuccessTerminal]);
        assert_eq!(inspector.terminal(), Some(&SemanticTerminal::Success));
        inspector.finish().unwrap();
    }

    #[test]
    fn inspector_allows_complete_events_beyond_pending_bound() {
        let max_pending_bytes = 96;
        let max_event_bytes = 4 * 1024;

        let document = serde_json::to_vec(&json!({
            "status": "completed",
            "padding": "x".repeat(max_pending_bytes)
        }))
        .unwrap();
        assert_eq!(
            ResponsesSseInspector::default()
                .push_with_limits(&document, max_pending_bytes, max_event_bytes)
                .unwrap(),
            vec![SemanticObservation::SuccessTerminal]
        );

        let ndjson = format!(
            "{{\"type\":\"response.completed\",\"response\":{{\"status\":\"completed\",\"padding\":\"{}\"}}}}\n",
            "x".repeat(max_pending_bytes)
        );
        assert_eq!(
            ResponsesSseInspector::default()
                .push_with_limits(ndjson.as_bytes(), max_pending_bytes, max_event_bytes)
                .unwrap(),
            vec![SemanticObservation::SuccessTerminal]
        );

        let sse = format!(
            "data: {{\"type\":\"response.completed\",\"response\":{{\"status\":\"completed\",\"padding\":\"{}\"}}}}\n\n",
            "x".repeat(max_pending_bytes)
        );
        assert_eq!(
            ResponsesSseInspector::default()
                .push_with_limits(sse.as_bytes(), max_pending_bytes, max_event_bytes)
                .unwrap(),
            vec![SemanticObservation::SuccessTerminal]
        );
    }

    #[test]
    fn inspector_enforces_pending_and_complete_event_bounds() {
        let max_pending_bytes = 96;
        let max_event_bytes = 192;
        let pending = format!("{{\"status\":\"{}", "x".repeat(max_pending_bytes));
        assert!(ResponsesSseInspector::default()
            .push_with_limits(pending.as_bytes(), max_pending_bytes, max_event_bytes)
            .unwrap_err()
            .to_string()
            .contains("exceeded 96 bytes"));

        let complete = serde_json::to_vec(&json!({
            "status": "completed",
            "padding": "x".repeat(max_event_bytes)
        }))
        .unwrap();
        assert!(ResponsesSseInspector::default()
            .push_with_limits(&complete, max_pending_bytes, max_event_bytes)
            .unwrap_err()
            .to_string()
            .contains("exceeded 192 bytes"));
    }

    #[test]
    fn incomplete_is_a_valid_partial_terminal() {
        let mut inspector = ResponsesSseInspector::default();
        let observations = inspector
            .push(b"data: {\"type\":\"response.incomplete\",\"response\":{\"status\":\"incomplete\"}}\n\n")
            .unwrap();
        assert_eq!(observations, vec![SemanticObservation::IncompleteTerminal]);
        assert_eq!(inspector.terminal(), Some(&SemanticTerminal::Incomplete));
        inspector.finish().unwrap();
    }

    #[test]
    fn proxy_bridge_contract_fixture_distinguishes_failure_origins() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/proxy_bridge/failure_origin.json"
        ))
        .unwrap();
        assert_eq!(fixture["id"], "semantic-failure-origin");
        assert_eq!(fixture["category"], "failure_origin");

        for case in fixture["cases"].as_array().unwrap() {
            let observation = classify_value(&case["input"]);
            assert_eq!(
                observation.metric_kind(),
                case["expected"]["metricKind"].as_str().unwrap(),
                "fixture case {}",
                case["name"]
            );
            assert_eq!(
                observation.commits_downstream(),
                case["expected"]["commitsDownstream"].as_bool().unwrap()
            );
            let SemanticObservation::Failure(failure) = observation else {
                panic!("fixture case {} must classify as failure", case["name"]);
            };
            assert_eq!(failure.origin.as_str(), case["expected"]["origin"]);
            assert_eq!(failure.code, case["expected"]["code"]);
            assert_eq!(
                failure.origin == FailureOrigin::Provider,
                case["expected"]["eligibleForPreCommitFailover"]
                    .as_bool()
                    .unwrap()
            );
        }
    }

    #[test]
    fn proxy_bridge_contract_fixture_accepts_incomplete_terminals() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../tests/fixtures/proxy_bridge/incomplete_terminal.json"
        ))
        .unwrap();
        assert_eq!(fixture["id"], "incomplete-valid-terminal");
        assert_eq!(fixture["category"], "incomplete_terminal");

        for document in fixture["documents"].as_array().unwrap() {
            let observation = classify_value(document);
            assert_eq!(
                observation.metric_kind(),
                fixture["expected"]["metricKind"].as_str().unwrap()
            );
            assert_eq!(observation, SemanticObservation::IncompleteTerminal);
        }

        let mut inspector = ResponsesSseInspector::default();
        let observations = inspector
            .push(fixture["sse"].as_str().unwrap().as_bytes())
            .unwrap();
        assert!(observations.contains(&SemanticObservation::IncompleteTerminal));
        inspector.finish().unwrap();
        let terminal = inspector.terminal().unwrap();
        assert_eq!(
            terminal.stream_status(),
            fixture["expected"]["streamStatus"].as_str().unwrap()
        );
        assert_eq!(terminal, &SemanticTerminal::Incomplete);
    }
}
