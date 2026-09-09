use axum::http::StatusCode;
use bytes::Bytes;
use serde_json::Value;

use super::ProxyError;

const MAX_OPENAI_CHAT_SSE_EVENT_BYTES: usize = 128 * 1024 * 1024;

#[derive(Debug)]
pub(crate) struct OpenAiChatStreamCanonicalizer {
    decoder: RawSseEventDecoder,
    fallback_created: i64,
    frozen_created: Option<i64>,
    path: &'static str,
}

impl OpenAiChatStreamCanonicalizer {
    pub(crate) fn new(path: &'static str) -> Self {
        Self::with_fallback_created(path, unix_timestamp_seconds())
    }

    fn with_fallback_created(path: &'static str, fallback_created: i64) -> Self {
        Self {
            decoder: RawSseEventDecoder::new(MAX_OPENAI_CHAT_SSE_EVENT_BYTES),
            fallback_created: fallback_created.max(1),
            frozen_created: None,
            path,
        }
    }

    pub(crate) fn push(&mut self, chunk: Bytes) -> Result<Bytes, ProxyError> {
        let events = self.decoder.push(&chunk).inspect_err(|_error| {
            crate::metrics::record_openai_chat_compat("invalid_chunk", self.path);
        })?;
        self.canonicalize_events(events)
    }

    pub(crate) fn finish(&mut self) -> Result<Bytes, ProxyError> {
        let events = self.decoder.finish().inspect_err(|_error| {
            crate::metrics::record_openai_chat_compat("invalid_chunk", self.path);
        })?;
        self.canonicalize_events(events)
    }

    fn canonicalize_events(&mut self, events: Vec<Vec<u8>>) -> Result<Bytes, ProxyError> {
        let mut output = Vec::new();
        for event in events {
            output.extend_from_slice(&self.canonicalize_event(event)?);
        }
        Ok(Bytes::from(output))
    }

    fn canonicalize_event(&mut self, event: Vec<u8>) -> Result<Vec<u8>, ProxyError> {
        let Some(payload) = sse_data_payload(&event) else {
            return Ok(event);
        };
        let payload = trim_ascii_whitespace(&payload);
        if payload.is_empty() || payload == b"[DONE]" || matches!(payload, b"ping" | b"keepalive") {
            return Ok(event);
        }
        let mut value = match serde_json::from_slice::<Value>(payload) {
            Ok(value) => value,
            Err(_) => {
                crate::metrics::record_openai_chat_compat("invalid_chunk", self.path);
                return Ok(event);
            }
        };
        if !looks_like_chat_stream_chunk(&value) {
            return Ok(event);
        }

        let field_present = value.get("created").is_some();
        let observed = value.get("created").and_then(valid_created);
        let created = match self.frozen_created {
            Some(created) => created,
            None => {
                let created = observed.unwrap_or(self.fallback_created);
                self.frozen_created = Some(created);
                created
            }
        };
        let action = if observed == Some(created) {
            "preserved"
        } else if field_present {
            "created_rewritten"
        } else {
            "created_injected"
        };
        crate::metrics::record_openai_chat_compat(action, self.path);
        if action == "preserved" {
            return Ok(event);
        }

        value["created"] = Value::from(created);
        let payload = serde_json::to_vec(&value).map_err(|error| {
            ProxyError::bad_gateway(format!(
                "encode normalized OpenAI Chat stream chunk: {error}"
            ))
        })?;
        Ok(rewrite_sse_data_payload(&event, &payload))
    }
}

pub(crate) fn normalize_chat_completion_bytes(
    body: Bytes,
    path: &'static str,
) -> Result<Bytes, ProxyError> {
    let mut value = match serde_json::from_slice::<Value>(&body) {
        Ok(value) => value,
        Err(_) => {
            crate::metrics::record_openai_chat_compat("invalid_chunk", path);
            return Ok(body);
        }
    };
    if !looks_like_chat_completion(&value) {
        return Ok(body);
    }
    let field_present = value.get("created").is_some();
    if value.get("created").and_then(valid_created).is_some() {
        crate::metrics::record_openai_chat_compat("preserved", path);
        return Ok(body);
    }
    value["created"] = Value::from(unix_timestamp_seconds());
    crate::metrics::record_openai_chat_compat(
        if field_present {
            "created_rewritten"
        } else {
            "created_injected"
        },
        path,
    );
    serde_json::to_vec(&value)
        .map(Bytes::from)
        .map_err(|error| {
            ProxyError::bad_gateway(format!("encode normalized OpenAI Chat response: {error}"))
        })
}

pub(crate) fn valid_created(value: &Value) -> Option<i64> {
    value.as_i64().filter(|value| *value > 0)
}

pub(crate) fn preferred_created(input: &Value) -> i64 {
    input
        .get("created_at")
        .and_then(valid_created)
        .or_else(|| input.get("created").and_then(valid_created))
        .unwrap_or_else(unix_timestamp_seconds)
}

pub(crate) fn unix_timestamp_seconds() -> i64 {
    (crate::infra::time::now_ms() / 1_000)
        .min(i64::MAX as u128)
        .max(1) as i64
}

fn looks_like_chat_stream_chunk(value: &Value) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    if object.get("error").is_some_and(|error| !error.is_null()) {
        return false;
    }
    match object.get("object") {
        Some(Value::String(kind)) => kind == "chat.completion.chunk",
        Some(_) => false,
        None => object.get("choices").is_some_and(Value::is_array),
    }
}

fn looks_like_chat_completion(value: &Value) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    if object.get("error").is_some_and(|error| !error.is_null()) {
        return false;
    }
    match object.get("object") {
        Some(Value::String(kind)) => kind == "chat.completion",
        Some(_) => false,
        None => object.get("choices").is_some_and(Value::is_array),
    }
}

#[derive(Debug)]
struct RawSseEventDecoder {
    pending: Vec<u8>,
    line_start: usize,
    scan_at: usize,
    max_event_bytes: usize,
}

impl RawSseEventDecoder {
    fn new(max_event_bytes: usize) -> Self {
        Self {
            pending: Vec::new(),
            line_start: 0,
            scan_at: 0,
            max_event_bytes: max_event_bytes.max(1),
        }
    }

    fn push(&mut self, chunk: &[u8]) -> Result<Vec<Vec<u8>>, ProxyError> {
        let mut events = Vec::new();
        let mut offset = 0;
        while offset < chunk.len() {
            let room = self
                .max_event_bytes
                .saturating_add(1)
                .saturating_sub(self.pending.len());
            if room == 0 {
                return Err(openai_chat_event_too_large(self.max_event_bytes));
            }
            let end = offset.saturating_add(room).min(chunk.len());
            self.pending.extend_from_slice(&chunk[offset..end]);
            offset = end;
            events.extend(self.drain(false)?);
        }
        Ok(events)
    }

    fn finish(&mut self) -> Result<Vec<Vec<u8>>, ProxyError> {
        self.drain(true)
    }

    fn drain(&mut self, finish: bool) -> Result<Vec<Vec<u8>>, ProxyError> {
        let mut events = Vec::new();
        let mut consumed = 0;
        loop {
            let Some((line_end, delimiter_len)) =
                next_line_boundary(&self.pending, self.scan_at, finish)
            else {
                self.scan_at = self
                    .pending
                    .len()
                    .saturating_sub(usize::from(self.pending.last() == Some(&b'\r')));
                break;
            };
            let next_line = line_end.saturating_add(delimiter_len);
            if next_line.saturating_sub(consumed) > self.max_event_bytes {
                return Err(openai_chat_event_too_large(self.max_event_bytes));
            }
            if line_end == self.line_start {
                events.push(self.pending[consumed..next_line].to_vec());
                consumed = next_line;
                self.line_start = next_line;
                self.scan_at = next_line;
            } else {
                self.line_start = next_line;
                self.scan_at = next_line;
            }
        }
        if consumed > 0 {
            self.pending.drain(..consumed);
            self.line_start = self.line_start.saturating_sub(consumed);
            self.scan_at = self.scan_at.saturating_sub(consumed);
        }
        if self.pending.len() > self.max_event_bytes {
            return Err(openai_chat_event_too_large(self.max_event_bytes));
        }
        if finish && !self.pending.is_empty() {
            events.push(std::mem::take(&mut self.pending));
            self.line_start = 0;
            self.scan_at = 0;
        }
        Ok(events)
    }
}

fn next_line_boundary(buffer: &[u8], start: usize, finish: bool) -> Option<(usize, usize)> {
    let mut index = start;
    while index < buffer.len() {
        match buffer[index] {
            b'\n' => return Some((index, 1)),
            b'\r' => match buffer.get(index + 1) {
                Some(b'\n') => return Some((index, 2)),
                Some(_) => return Some((index, 1)),
                None if finish => return Some((index, 1)),
                None => return None,
            },
            _ => index += 1,
        }
    }
    None
}

fn openai_chat_event_too_large(limit: usize) -> ProxyError {
    ProxyError {
        status: StatusCode::PAYLOAD_TOO_LARGE,
        message: format!("OpenAI Chat SSE event exceeded {limit} bytes"),
    }
}

fn for_each_raw_line(value: &[u8], mut visit: impl FnMut(&[u8], &[u8])) {
    let mut start = 0;
    while start < value.len() {
        let Some((end, delimiter_len)) = next_line_boundary(value, start, true) else {
            visit(&value[start..], &[]);
            return;
        };
        visit(&value[start..end], &value[end..end + delimiter_len]);
        start = end + delimiter_len;
    }
}

fn sse_data_payload(event: &[u8]) -> Option<Vec<u8>> {
    let mut payload = Vec::new();
    let mut has_data = false;
    let mut valid_utf8 = true;
    for_each_raw_line(event, |line, _| {
        if line.is_empty() || line.starts_with(b":") {
            return;
        }
        let Ok(line) = std::str::from_utf8(line) else {
            valid_utf8 = false;
            return;
        };
        let (field, value) = line
            .split_once(':')
            .map(|(field, value)| (field, value.strip_prefix(' ').unwrap_or(value)))
            .unwrap_or((line, ""));
        if field == "data" {
            if has_data {
                payload.push(b'\n');
            }
            payload.extend_from_slice(value.as_bytes());
            has_data = true;
        }
    });
    (valid_utf8 && has_data).then_some(payload)
}

fn rewrite_sse_data_payload(event: &[u8], payload: &[u8]) -> Vec<u8> {
    let mut output = Vec::with_capacity(event.len().saturating_add(payload.len()));
    let mut replaced = false;
    for_each_raw_line(event, |line, delimiter| {
        let is_data = std::str::from_utf8(line).ok().is_some_and(|line| {
            line.split_once(':')
                .map(|(field, _)| field == "data")
                .unwrap_or(line == "data")
        });
        if is_data {
            if !replaced {
                output.extend_from_slice(b"data: ");
                output.extend_from_slice(payload);
                output.extend_from_slice(delimiter);
                replaced = true;
            }
            return;
        }
        output.extend_from_slice(line);
        output.extend_from_slice(delimiter);
    });
    output
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
    use serde::Deserialize;

    use super::*;

    #[derive(Debug, Deserialize)]
    struct StrictChatChunk {
        object: String,
        created: i64,
        choices: Vec<Value>,
    }

    fn normalize(input: &[u8], created: i64) -> Vec<u8> {
        let mut canonicalizer =
            OpenAiChatStreamCanonicalizer::with_fallback_created("test", created);
        let mut output = canonicalizer
            .push(Bytes::copy_from_slice(input))
            .unwrap()
            .to_vec();
        output.extend_from_slice(&canonicalizer.finish().unwrap());
        output
    }

    fn parsed_chunks(stream: &[u8]) -> Vec<StrictChatChunk> {
        let mut chunks = Vec::new();
        for event in RawSseEventDecoder::new(1024 * 1024).push(stream).unwrap() {
            let Some(payload) = sse_data_payload(&event) else {
                continue;
            };
            if trim_ascii_whitespace(&payload) == b"[DONE]" {
                continue;
            }
            if let Ok(chunk) = serde_json::from_slice::<StrictChatChunk>(&payload) {
                chunks.push(chunk);
            }
        }
        chunks
    }

    #[test]
    fn canonicalizer_fallback_is_stable_across_all_chunk_kinds() {
        let input = concat!(
            ": keepalive\n\n",
            "data: {\"object\":\"chat.completion.chunk\",\"choices\":[{\"delta\":{\"role\":\"assistant\"}}]}\n\n",
            "data: {\"object\":\"chat.completion.chunk\",\"created\":null,\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\n",
            "data: {\"object\":\"chat.completion.chunk\",\"created\":99,\"choices\":[{\"delta\":{\"reasoning_content\":\"plan\"}}]}\n\n",
            "data: {\"object\":\"chat.completion.chunk\",\"choices\":[{\"delta\":{\"tool_calls\":[]}}]}\n\n",
            "data: {\"object\":\"chat.completion.chunk\",\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
            "data: {\"object\":\"chat.completion.chunk\",\"choices\":[],\"usage\":{\"total_tokens\":2}}\n\n",
            "data: [DONE]\n\n"
        );
        let output = normalize(input.as_bytes(), 42);
        let chunks = parsed_chunks(&output);
        assert_eq!(chunks.len(), 6);
        assert!(chunks.iter().all(|chunk| chunk.created == 42));
        assert!(chunks
            .iter()
            .all(|chunk| chunk.object == "chat.completion.chunk"));
        assert!(chunks.iter().any(|chunk| chunk.choices.is_empty()));
        assert!(output.ends_with(b"data: [DONE]\n\n"));
    }

    #[test]
    fn canonicalizer_freezes_first_valid_created() {
        let input = concat!(
            "data: {\"object\":\"chat.completion.chunk\",\"created\":7,\"choices\":[]}\n\n",
            "data: {\"object\":\"chat.completion.chunk\",\"created\":8,\"choices\":[]}\n\n"
        );
        let output = normalize(input.as_bytes(), 42);
        let chunks = parsed_chunks(&output);
        assert_eq!(
            chunks.iter().map(|chunk| chunk.created).collect::<Vec<_>>(),
            vec![7, 7]
        );
    }

    #[test]
    fn canonicalizer_handles_every_split_and_line_ending() {
        for input in [
            b"event: message\ndata: {\"object\":\"chat.completion.chunk\",\"choices\":[]}\n\n".as_slice(),
            b"event: message\r\ndata: {\"object\":\"chat.completion.chunk\",\"choices\":[]}\r\n\r\n".as_slice(),
            b"event: message\rdata: {\"object\":\"chat.completion.chunk\",\"choices\":[]}\r\r".as_slice(),
        ] {
            let expected = normalize(input, 73);
            for split in 0..=input.len() {
                let mut canonicalizer =
                    OpenAiChatStreamCanonicalizer::with_fallback_created("test", 73);
                let mut actual = canonicalizer
                    .push(Bytes::copy_from_slice(&input[..split]))
                    .unwrap()
                    .to_vec();
                actual.extend_from_slice(
                    &canonicalizer
                        .push(Bytes::copy_from_slice(&input[split..]))
                        .unwrap(),
                );
                actual.extend_from_slice(&canonicalizer.finish().unwrap());
                assert_eq!(actual, expected, "split={split}, input={input:?}");
            }
        }
    }

    #[test]
    fn canonicalizer_preserves_untouched_events_byte_for_byte() {
        let input = b": comment\r\nid: 7\r\nretry: 1000\r\nevent: chunk\r\ndata: {\"object\":\"chat.completion.chunk\",\"created\":17,\"choices\":[]}\r\n\r\ndata: [DONE]\r\n\r\n";
        assert_eq!(normalize(input, 99), input);
    }

    #[test]
    fn canonicalizer_rewrites_multiline_data_and_preserves_control_fields() {
        let input = b"event: chunk\nid: abc\ndata: {\"object\":\"chat.completion.chunk\",\ndata: \"choices\":[]}\n\n";
        let output = normalize(input, 31);
        let text = std::str::from_utf8(&output).unwrap();
        assert!(text.starts_with("event: chunk\nid: abc\ndata: "));
        assert_eq!(text.matches("data:").count(), 1);
        assert_eq!(parsed_chunks(&output)[0].created, 31);
    }

    #[test]
    fn canonicalizer_leaves_errors_and_invalid_json_unchanged() {
        for input in [
            b"data: {\"error\":{\"message\":\"bad\"}}\n\n".as_slice(),
            b"data: {not-json}\n\n".as_slice(),
            b"data: \xff\n\n".as_slice(),
        ] {
            assert_eq!(normalize(input, 5), input);
        }
    }

    #[test]
    fn null_error_does_not_hide_a_success_chat_envelope() {
        let stream = normalize(
            br#"data: {"object":"chat.completion.chunk","error":null,"choices":[]}

"#,
            19,
        );
        assert_eq!(parsed_chunks(&stream)[0].created, 19);

        let body = Bytes::from_static(br#"{"object":"chat.completion","error":null,"choices":[]}"#);
        let normalized = normalize_chat_completion_bytes(body, "test").unwrap();
        assert!(
            serde_json::from_slice::<Value>(&normalized).unwrap()["created"]
                .as_i64()
                .is_some_and(|created| created > 0)
        );
    }

    #[test]
    fn normalizes_nonstream_chat_created_without_touching_errors() {
        for invalid in [
            "",
            ",\"created\":null",
            ",\"created\":0",
            ",\"created\":-1",
            ",\"created\":1.5",
            ",\"created\":\"7\"",
        ] {
            let body = Bytes::from(format!(
                "{{\"id\":\"c\",\"object\":\"chat.completion\",\"choices\":[]{invalid}}}"
            ));
            let normalized = normalize_chat_completion_bytes(body, "test").unwrap();
            let value: Value = serde_json::from_slice(&normalized).unwrap();
            assert!(
                value.get("created").and_then(valid_created).is_some(),
                "invalid={invalid}"
            );
        }

        let valid = Bytes::from_static(
            br#"{"id":"c","object":"chat.completion","created":7,"choices":[]}"#,
        );
        assert_eq!(
            normalize_chat_completion_bytes(valid.clone(), "test").unwrap(),
            valid
        );

        let error = Bytes::from_static(br#"{"error":{"message":"bad"}}"#);
        assert_eq!(
            normalize_chat_completion_bytes(error.clone(), "test").unwrap(),
            error
        );
    }

    #[test]
    fn invalid_created_values_are_rejected() {
        for value in [
            json_value("null"),
            json_value("0"),
            json_value("-1"),
            json_value("1.5"),
            json_value("\"7\""),
        ] {
            assert_eq!(valid_created(&value), None, "value={value}");
        }
        assert_eq!(valid_created(&Value::from(7)), Some(7));
    }

    #[test]
    fn raw_decoder_applies_the_limit_per_event_not_per_network_chunk() {
        let event = b"data: 1\n\n";
        let input = [event.as_slice(), event.as_slice(), event.as_slice()].concat();
        let mut decoder = RawSseEventDecoder::new(event.len());
        assert_eq!(decoder.push(&input).unwrap().len(), 3);

        let mut decoder = RawSseEventDecoder::new(event.len());
        assert!(decoder.push(b"data: too-large").is_err());
    }

    fn json_value(value: &str) -> Value {
        serde_json::from_str(value).unwrap()
    }
}
