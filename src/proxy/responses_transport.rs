use std::fmt;

use bytes::Bytes;
use serde_json::Value;

use super::sse_transport::{SseDecodeError, SseEvent, SseEventDecoder};

const DEFAULT_MAX_PENDING_BYTES: usize = 2 * 1024 * 1024;
const DEFAULT_MAX_EVENT_BYTES: usize = 128 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResponsesLivenessKind {
    Comment,
    Control,
    TextPing,
    EventPing,
    JsonPing,
}

impl ResponsesLivenessKind {
    pub(crate) fn metric_kind(self) -> &'static str {
        match self {
            Self::Comment => "comment",
            Self::Control => "control",
            Self::TextPing => "text_ping",
            Self::EventPing => "event_ping",
            Self::JsonPing => "json_ping",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ResponsesTransportItem {
    Json {
        declared_event: Option<String>,
        value: Value,
    },
    Done,
    Liveness(ResponsesLivenessKind),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResponsesTransportError {
    kind: ResponsesTransportErrorKind,
    message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResponsesTransportErrorKind {
    Protocol,
    Capacity,
}

impl ResponsesTransportError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            kind: ResponsesTransportErrorKind::Protocol,
            message: message.into(),
        }
    }

    fn capacity(message: impl Into<String>) -> Self {
        Self {
            kind: ResponsesTransportErrorKind::Capacity,
            message: message.into(),
        }
    }

    pub(crate) fn is_capacity(&self) -> bool {
        self.kind == ResponsesTransportErrorKind::Capacity
    }
}

impl fmt::Display for ResponsesTransportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ResponsesTransportError {}

impl From<SseDecodeError> for ResponsesTransportError {
    fn from(error: SseDecodeError) -> Self {
        let message = format!("Responses SSE framing is invalid: {error}");
        match error {
            SseDecodeError::InvalidUtf8(_) => Self::new(message),
            SseDecodeError::EventTooLarge { .. } => Self::capacity(message),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResponsesTransportMode {
    Unknown,
    Sse,
    Json,
}

/// Decodes the transports observed on OpenAI Responses endpoints and applies a
/// strict liveness allowlist. It never treats arbitrary malformed data as a
/// keepalive.
#[derive(Debug)]
pub(crate) struct ResponsesTransportDecoder {
    mode: ResponsesTransportMode,
    undecided: Vec<u8>,
    sse: SseEventDecoder,
    json: Vec<u8>,
    max_pending_bytes: usize,
    max_event_bytes: usize,
}

impl Default for ResponsesTransportDecoder {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_PENDING_BYTES, DEFAULT_MAX_EVENT_BYTES)
    }
}

impl ResponsesTransportDecoder {
    pub(crate) fn new(max_pending_bytes: usize, max_event_bytes: usize) -> Self {
        let max_pending_bytes = max_pending_bytes.max(1);
        let max_event_bytes = max_event_bytes.max(max_pending_bytes);
        Self {
            mode: ResponsesTransportMode::Unknown,
            undecided: Vec::new(),
            sse: SseEventDecoder::with_limits(max_pending_bytes, max_event_bytes),
            json: Vec::new(),
            max_pending_bytes,
            max_event_bytes,
        }
    }

    pub(crate) fn is_sse(&self) -> bool {
        self.mode == ResponsesTransportMode::Sse
    }

    pub(crate) fn push(
        &mut self,
        chunk: &[u8],
    ) -> Result<Vec<ResponsesTransportItem>, ResponsesTransportError> {
        if chunk.is_empty() {
            return Ok(Vec::new());
        }
        if self.mode == ResponsesTransportMode::Unknown {
            self.undecided.extend_from_slice(chunk);
            self.detect_mode(false)?;
            if self.mode == ResponsesTransportMode::Unknown {
                self.ensure_undecided_bounded()?;
                return Ok(Vec::new());
            }
            let buffered = std::mem::take(&mut self.undecided);
            return self.push_decided(&buffered);
        }
        self.push_decided(chunk)
    }

    pub(crate) fn finish(
        &mut self,
    ) -> Result<Vec<ResponsesTransportItem>, ResponsesTransportError> {
        if self.mode == ResponsesTransportMode::Unknown {
            self.detect_mode(true)?;
            if self.mode == ResponsesTransportMode::Unknown {
                if self.undecided.iter().all(u8::is_ascii_whitespace) {
                    self.undecided.clear();
                    return Ok(Vec::new());
                }
                return Err(ResponsesTransportError::new(
                    "Responses stream ended with an unrecognized payload",
                ));
            }
            let buffered = std::mem::take(&mut self.undecided);
            let mut items = self.push_decided(&buffered)?;
            items.extend(self.finish_decided()?);
            return Ok(items);
        }
        self.finish_decided()
    }

    fn push_decided(
        &mut self,
        bytes: &[u8],
    ) -> Result<Vec<ResponsesTransportItem>, ResponsesTransportError> {
        match self.mode {
            ResponsesTransportMode::Sse => {
                let events = self.sse.push(bytes)?;
                classify_sse_events(events)
            }
            ResponsesTransportMode::Json => {
                self.json.extend_from_slice(bytes);
                self.drain_json(false)
            }
            ResponsesTransportMode::Unknown => unreachable!("transport mode was decided"),
        }
    }

    fn finish_decided(&mut self) -> Result<Vec<ResponsesTransportItem>, ResponsesTransportError> {
        match self.mode {
            ResponsesTransportMode::Sse => classify_sse_events(self.sse.finish()?),
            ResponsesTransportMode::Json => self.drain_json(true),
            ResponsesTransportMode::Unknown => Ok(Vec::new()),
        }
    }

    fn detect_mode(&mut self, finish: bool) -> Result<(), ResponsesTransportError> {
        let trimmed = trim_ascii_start(&self.undecided);
        if trimmed.is_empty() {
            return Ok(());
        }
        self.mode = match trimmed[0] {
            b'{' | b'[' => ResponsesTransportMode::Json,
            b':' => ResponsesTransportMode::Sse,
            _ if starts_sse_field(trimmed) => ResponsesTransportMode::Sse,
            _ if finish || trimmed.contains(&b'\n') || trimmed.contains(&b'\r') => {
                ResponsesTransportMode::Sse
            }
            _ => ResponsesTransportMode::Unknown,
        };
        Ok(())
    }

    fn drain_json(
        &mut self,
        finish: bool,
    ) -> Result<Vec<ResponsesTransportItem>, ResponsesTransportError> {
        let mut items = Vec::new();
        loop {
            let leading = self
                .json
                .iter()
                .position(|byte| !byte.is_ascii_whitespace())
                .unwrap_or(self.json.len());
            if leading > 0 {
                self.json.drain(..leading);
            }
            if self.json.is_empty() {
                break;
            }

            if let Some(done_len) = done_token_prefix_len(&self.json) {
                self.json.drain(..done_len);
                items.push(ResponsesTransportItem::Done);
                continue;
            }
            if !finish && b"[DONE]".starts_with(&self.json) {
                break;
            }

            let mut values = serde_json::Deserializer::from_slice(&self.json).into_iter::<Value>();
            match values.next() {
                Some(Ok(value)) => {
                    let consumed = values.byte_offset();
                    self.ensure_json_event_bounded(consumed)?;
                    if !value.is_object() {
                        return Err(ResponsesTransportError::new(
                            "Responses JSON payload must be an object",
                        ));
                    }
                    self.json.drain(..consumed);
                    if json_liveness(&value) {
                        items.push(ResponsesTransportItem::Liveness(
                            ResponsesLivenessKind::JsonPing,
                        ));
                    } else {
                        items.push(ResponsesTransportItem::Json {
                            declared_event: None,
                            value,
                        });
                    }
                }
                Some(Err(error)) if error.is_eof() && !finish => break,
                Some(Err(error)) => {
                    return Err(ResponsesTransportError::new(format!(
                        "Responses JSON data is not valid JSON: {error}"
                    )))
                }
                None => break,
            }
        }
        if !finish && self.json.len() > self.max_pending_bytes {
            return Err(ResponsesTransportError::capacity(format!(
                "Responses pending JSON event exceeded {} bytes",
                self.max_pending_bytes
            )));
        }
        Ok(items)
    }

    fn ensure_undecided_bounded(&self) -> Result<(), ResponsesTransportError> {
        if self.undecided.len() <= self.max_pending_bytes {
            Ok(())
        } else {
            Err(ResponsesTransportError::capacity(format!(
                "Responses undecided transport prefix exceeded {} bytes",
                self.max_pending_bytes
            )))
        }
    }

    fn ensure_json_event_bounded(&self, event_bytes: usize) -> Result<(), ResponsesTransportError> {
        if event_bytes <= self.max_event_bytes {
            Ok(())
        } else {
            Err(ResponsesTransportError::capacity(format!(
                "Responses JSON event exceeded {} bytes",
                self.max_event_bytes
            )))
        }
    }
}

fn classify_sse_events(
    events: Vec<SseEvent>,
) -> Result<Vec<ResponsesTransportItem>, ResponsesTransportError> {
    events.into_iter().map(classify_sse_event).collect()
}

fn classify_sse_event(event: SseEvent) -> Result<ResponsesTransportItem, ResponsesTransportError> {
    if event.unknown_fields > 0 {
        return Err(ResponsesTransportError::new(
            "Responses SSE event contained an unknown non-control field",
        ));
    }
    let declared_event = event
        .event
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty());
    let declared_liveness = declared_event.filter(|value| liveness_token(value));
    if !event.has_data {
        return Ok(ResponsesTransportItem::Liveness(
            if declared_liveness.is_some() {
                ResponsesLivenessKind::EventPing
            } else if event.comment_lines > 0
                && event.event.is_none()
                && event.id.is_none()
                && event.retry.is_none()
            {
                ResponsesLivenessKind::Comment
            } else {
                ResponsesLivenessKind::Control
            },
        ));
    }

    let payload = trim_ascii_whitespace(&event.data);
    if payload.is_empty() {
        return Ok(ResponsesTransportItem::Liveness(
            if declared_liveness.is_some() {
                ResponsesLivenessKind::EventPing
            } else {
                ResponsesLivenessKind::Control
            },
        ));
    }
    if payload == b"[DONE]" {
        if declared_event.is_some() {
            return Err(ResponsesTransportError::new(
                "Responses SSE [DONE] sentinel must not declare an event",
            ));
        }
        return Ok(ResponsesTransportItem::Done);
    }
    if matches!(payload, b"ping" | b"keepalive") {
        if declared_event.is_none() || declared_liveness == std::str::from_utf8(payload).ok() {
            return Ok(ResponsesTransportItem::Liveness(
                if declared_event.is_some() {
                    ResponsesLivenessKind::EventPing
                } else {
                    ResponsesLivenessKind::TextPing
                },
            ));
        }
        return Err(ResponsesTransportError::new(
            "Responses SSE liveness payload conflicted with its declared event",
        ));
    }
    let value = serde_json::from_slice::<Value>(payload).map_err(|error| {
        ResponsesTransportError::new(format!("Responses SSE data is not valid JSON: {error}"))
    })?;
    if !value.is_object() {
        return Err(ResponsesTransportError::new(
            "Responses SSE data must be a JSON object",
        ));
    }
    let payload_event = value
        .get("type")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if let (Some(declared_event), Some(payload_event)) = (declared_event, payload_event) {
        if declared_event != payload_event {
            return Err(ResponsesTransportError::new(
                "Responses SSE declared event conflicted with its JSON type",
            ));
        }
    }
    if json_liveness(&value) {
        return Ok(ResponsesTransportItem::Liveness(
            if declared_event.is_some() {
                ResponsesLivenessKind::EventPing
            } else {
                ResponsesLivenessKind::JsonPing
            },
        ));
    }
    if declared_liveness.is_some() {
        return Err(ResponsesTransportError::new(
            "Responses SSE liveness event contained a non-liveness payload",
        ));
    }
    Ok(ResponsesTransportItem::Json {
        declared_event: event.event,
        value,
    })
}

pub(crate) fn encode_canonical_json_sse(declared_event: Option<&str>, value: &Value) -> Bytes {
    let event = value
        .get("type")
        .and_then(Value::as_str)
        .or(declared_event)
        .map(str::trim)
        .filter(|event| valid_event_name(event));
    let payload = serde_json::to_string(value).expect("serializing a JSON Value cannot fail");
    let mut output = String::with_capacity(payload.len().saturating_add(48));
    if let Some(event) = event {
        output.push_str("event: ");
        output.push_str(event);
        output.push('\n');
    }
    output.push_str("data: ");
    output.push_str(&payload);
    output.push_str("\n\n");
    Bytes::from(output)
}

pub(crate) fn encode_canonical_done_sse() -> Bytes {
    Bytes::from_static(b"data: [DONE]\n\n")
}

fn starts_sse_field(value: &[u8]) -> bool {
    [b"data".as_slice(), b"event", b"id", b"retry"]
        .into_iter()
        .any(|field| {
            value.starts_with(field)
                && value
                    .get(field.len())
                    .is_none_or(|byte| matches!(byte, b':' | b'\r' | b'\n'))
        })
}

fn done_token_prefix_len(value: &[u8]) -> Option<usize> {
    const DONE: &[u8] = b"[DONE]";
    if !value.starts_with(DONE) {
        return None;
    }
    value
        .get(DONE.len())
        .is_none_or(u8::is_ascii_whitespace)
        .then_some(DONE.len())
}

fn json_liveness(value: &Value) -> bool {
    value.as_object().is_some_and(|object| {
        object.len() == 1
            && object
                .get("type")
                .and_then(Value::as_str)
                .is_some_and(liveness_token)
    })
}

fn liveness_token(value: &str) -> bool {
    matches!(value, "ping" | "keepalive")
}

fn valid_event_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b':'))
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn decode_all_splits(input: &[u8]) -> Vec<ResponsesTransportItem> {
        let expected = {
            let mut decoder = ResponsesTransportDecoder::default();
            let mut items = decoder.push(input).unwrap();
            items.extend(decoder.finish().unwrap());
            items
        };
        for split in 0..=input.len() {
            let mut decoder = ResponsesTransportDecoder::default();
            let mut items = decoder.push(&input[..split]).unwrap();
            items.extend(decoder.push(&input[split..]).unwrap());
            items.extend(decoder.finish().unwrap());
            assert_eq!(items, expected, "split={split}");
        }
        expected
    }

    #[test]
    fn filters_only_allowlisted_liveness_and_preserves_json() {
        let items = decode_all_splits(
            concat!(
                ": keepalive\r\n\r\n",
                "id: 7\r\nretry: 1000\r\n\r\n",
                "data: ping\r\n\r\n",
                "event: keepalive\r\ndata: keepalive\r\n\r\n",
                "data: {\"type\":\"ping\"}\r\n\r\n",
                "event: ping\r\ndata: {\"type\":\"ping\"}\r\n\r\n",
                "event: response.output_text.delta\r\n",
                "data: {\"type\":\"response.output_text.delta\",\r\n",
                "data: \"delta\":\"ok\"}\r\n\r\n"
            )
            .as_bytes(),
        );
        assert_eq!(
            items,
            vec![
                ResponsesTransportItem::Liveness(ResponsesLivenessKind::Comment),
                ResponsesTransportItem::Liveness(ResponsesLivenessKind::Control),
                ResponsesTransportItem::Liveness(ResponsesLivenessKind::TextPing),
                ResponsesTransportItem::Liveness(ResponsesLivenessKind::EventPing),
                ResponsesTransportItem::Liveness(ResponsesLivenessKind::JsonPing),
                ResponsesTransportItem::Liveness(ResponsesLivenessKind::EventPing),
                ResponsesTransportItem::Json {
                    declared_event: Some("response.output_text.delta".to_string()),
                    value: json!({"type":"response.output_text.delta","delta":"ok"}),
                },
            ]
        );
    }

    #[test]
    fn accepts_json_documents_ndjson_and_done() {
        let items = decode_all_splits(
            b"{\"type\":\"response.created\"}\n{\"type\":\"response.completed\"}\n[DONE]",
        );
        assert_eq!(items.len(), 3);
        assert!(matches!(items[0], ResponsesTransportItem::Json { .. }));
        assert!(matches!(items[1], ResponsesTransportItem::Json { .. }));
        assert_eq!(items[2], ResponsesTransportItem::Done);
    }

    #[test]
    fn rejects_html_plaintext_and_non_allowlisted_payloads() {
        for input in [
            b"<!doctype html>\n\n".as_slice(),
            b"data: pong\n\n".as_slice(),
            b"event: ping\ndata: {\"type\":\"response.created\"}\n\n".as_slice(),
            b"event: ping\ndata: {\"type\":\"keepalive\"}\n\n".as_slice(),
            b"event: response.output_text.delta\ndata: {\"type\":\"ping\"}\n\n".as_slice(),
            b"event: response.completed\ndata: {\"type\":\"response.failed\",\"response\":{\"status\":\"failed\"}}\n\n".as_slice(),
            b"event: response.completed\ndata: [DONE]\n\n".as_slice(),
            b"data: definitely-not-json\n\n".as_slice(),
            b"data: []\n\n".as_slice(),
            b"\"valid JSON, but not a Responses object\"\n".as_slice(),
        ] {
            let mut decoder = ResponsesTransportDecoder::default();
            let result = decoder.push(input).and_then(|mut items| {
                decoder.finish().map(|tail| {
                    items.extend(tail);
                    items
                })
            });
            assert!(result.is_err(), "input={input:?}");
        }

        let mut decoder = ResponsesTransportDecoder::default();
        let items = decoder
            .push(b"data: {\"type\":\"ping\",\"response\":{\"status\":\"completed\"}}\n\n")
            .unwrap();
        assert!(matches!(
            items.as_slice(),
            [ResponsesTransportItem::Json { .. }]
        ));
    }

    #[test]
    fn canonical_encoder_emits_one_valid_json_data_line() {
        let encoded = encode_canonical_json_sse(
            Some("ignored"),
            &json!({"type":"response.completed","response":{"status":"completed"}}),
        );
        let text = std::str::from_utf8(&encoded).unwrap();
        assert!(text.starts_with("event: response.completed\ndata: "));
        assert!(text.ends_with("\n\n"));
        assert_eq!(text.matches("data: ").count(), 1);
    }

    #[test]
    fn enforces_pending_and_complete_json_bounds() {
        let mut pending = ResponsesTransportDecoder::new(8, 64);
        assert!(pending.push(b"{\"value\":").is_err());

        let mut complete = ResponsesTransportDecoder::new(8, 16);
        assert!(complete.push(b"{\"value\":\"0123456789\"}").is_err());

        let mut pending_sse = ResponsesTransportDecoder::new(16, 64);
        let error = pending_sse.push(b"data: 12345678901").unwrap_err();
        assert!(error.is_capacity());

        let mut complete_sse = ResponsesTransportDecoder::new(16, 64);
        let items = complete_sse
            .push(b"data: {\"type\":\"response.completed\"}\n\n")
            .unwrap();
        assert!(matches!(
            items.as_slice(),
            [ResponsesTransportItem::Json { .. }]
        ));
    }
}
