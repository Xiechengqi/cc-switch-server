use serde_json::Value;

use super::adapters::UpstreamFormat;

const MAX_TERMINAL_EVENT_BYTES: usize = 8 * 1024 * 1024;
const MAX_OPENAI_RESPONSES_TERMINAL_EVENT_BYTES: usize = 128 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TerminalDetectorError {
    EventTooLarge,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum UpstreamTerminal {
    Completed,
    Incomplete,
    Failed,
}

#[derive(Debug)]
pub(super) struct UpstreamTerminalDetector {
    format: UpstreamFormat,
    pending: Vec<u8>,
    terminal: Option<UpstreamTerminal>,
    max_event_bytes: usize,
}

impl UpstreamTerminalDetector {
    pub(super) fn new(format: UpstreamFormat) -> Self {
        let max_event_bytes = match format {
            UpstreamFormat::OpenAiResponses => MAX_OPENAI_RESPONSES_TERMINAL_EVENT_BYTES,
            _ => MAX_TERMINAL_EVENT_BYTES,
        };
        Self {
            format,
            pending: Vec::new(),
            terminal: None,
            max_event_bytes,
        }
    }

    pub(super) fn max_event_bytes(&self) -> usize {
        self.max_event_bytes
    }

    pub(super) fn terminal(&self) -> Option<UpstreamTerminal> {
        self.terminal
    }

    pub(super) fn push(&mut self, chunk: &[u8]) -> Result<(), TerminalDetectorError> {
        if self.terminal.is_some() {
            return Ok(());
        }
        self.pending.extend_from_slice(chunk);
        while let Some((event_end, delimiter_len)) = next_event_boundary(&self.pending) {
            if event_end > self.max_event_bytes {
                return Err(TerminalDetectorError::EventTooLarge);
            }
            let event = self.pending[..event_end].to_vec();
            self.pending.drain(..event_end + delimiter_len);
            if let Some(terminal) = event_terminal(self.format, &event) {
                self.terminal = Some(terminal);
                self.pending.clear();
                return Ok(());
            }
        }
        if self.pending.len() > self.max_event_bytes {
            return Err(TerminalDetectorError::EventTooLarge);
        }
        Ok(())
    }
}

fn event_terminal(format: UpstreamFormat, event: &[u8]) -> Option<UpstreamTerminal> {
    let Ok(text) = std::str::from_utf8(event) else {
        return None;
    };
    let mut declared_event = None;
    let data = text
        .lines()
        .filter_map(|raw_line| {
            let line = raw_line.trim_end_matches('\r');
            if let Some(value) = sse_field(line, "event") {
                declared_event = Some(value.trim());
                None
            } else {
                sse_field(line, "data").map(str::trim_start)
            }
        })
        .collect::<Vec<_>>();
    let payload = if data.is_empty() {
        let text = text.trim();
        (text.starts_with('{') || text == "[DONE]").then_some(text.to_string())
    } else {
        Some(data.join("\n").trim().to_string())
    };
    if let Some(terminal) = declared_event.and_then(|event| declared_event_terminal(format, event))
    {
        return Some(terminal);
    }
    payload
        .as_deref()
        .and_then(|payload| payload_terminal(format, payload))
}

fn declared_event_terminal(format: UpstreamFormat, event: &str) -> Option<UpstreamTerminal> {
    match format {
        UpstreamFormat::OpenAiResponses => match event {
            "response.completed" => Some(UpstreamTerminal::Completed),
            "response.incomplete" => Some(UpstreamTerminal::Incomplete),
            "response.failed" | "response.cancelled" | "response.canceled" | "error" => {
                Some(UpstreamTerminal::Failed)
            }
            _ => None,
        },
        UpstreamFormat::OpenAiChat => (event == "error").then_some(UpstreamTerminal::Failed),
        UpstreamFormat::AnthropicMessages => match event {
            "message_stop" => Some(UpstreamTerminal::Completed),
            "error" => Some(UpstreamTerminal::Failed),
            _ => None,
        },
        UpstreamFormat::GeminiNative => (event == "error").then_some(UpstreamTerminal::Failed),
    }
}

fn payload_terminal(format: UpstreamFormat, payload: &str) -> Option<UpstreamTerminal> {
    if payload == "[DONE]" {
        return (format == UpstreamFormat::OpenAiChat).then_some(UpstreamTerminal::Completed);
    }
    let Ok(value) = serde_json::from_str::<Value>(payload) else {
        return None;
    };
    if value.get("error").is_some_and(|error| !error.is_null())
        || value.get("type").and_then(Value::as_str) == Some("error")
    {
        return Some(UpstreamTerminal::Failed);
    }
    match format {
        UpstreamFormat::OpenAiResponses => {
            let event_type = value.get("type").and_then(Value::as_str);
            let status = value
                .get("response")
                .and_then(|response| response.get("status"))
                .or_else(|| value.get("status"))
                .and_then(Value::as_str);
            match event_type.or(status) {
                Some("response.completed" | "completed") => Some(UpstreamTerminal::Completed),
                Some("response.incomplete" | "incomplete") => Some(UpstreamTerminal::Incomplete),
                Some(
                    "response.failed" | "response.cancelled" | "response.canceled" | "failed"
                    | "cancelled" | "canceled",
                ) => Some(UpstreamTerminal::Failed),
                _ => None,
            }
        }
        UpstreamFormat::OpenAiChat => None,
        UpstreamFormat::AnthropicMessages => match value.get("type").and_then(Value::as_str) {
            Some("message_stop") => Some(UpstreamTerminal::Completed),
            Some("error") => Some(UpstreamTerminal::Failed),
            _ => None,
        },
        UpstreamFormat::GeminiNative => {
            gemini_terminal(&value).then_some(UpstreamTerminal::Completed)
        }
    }
}

fn gemini_terminal(value: &Value) -> bool {
    let value = value.get("response").unwrap_or(value);
    value
        .pointer("/promptFeedback/blockReason")
        .or_else(|| value.pointer("/prompt_feedback/block_reason"))
        .and_then(Value::as_str)
        .is_some_and(|reason| !reason.trim().is_empty())
        || value
            .get("candidates")
            .and_then(Value::as_array)
            .is_some_and(|candidates| {
                !candidates.is_empty()
                    && candidates.iter().all(|candidate| {
                        candidate
                            .get("finishReason")
                            .or_else(|| candidate.get("finish_reason"))
                            .and_then(Value::as_str)
                            .is_some_and(|reason| !reason.trim().is_empty())
                    })
            })
}

fn sse_field<'a>(line: &'a str, field: &str) -> Option<&'a str> {
    let rest = line.strip_prefix(field)?.strip_prefix(':')?;
    Some(rest.strip_prefix(' ').unwrap_or(rest))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_terminals_across_chunk_boundaries() {
        let fixtures = [
            (
                UpstreamFormat::OpenAiResponses,
                "event: response.completed\r\ndata: {\"type\":\"response.completed\"}\r\n\r\n",
            ),
            (UpstreamFormat::OpenAiChat, "data: [DONE]\n\n"),
            (
                UpstreamFormat::AnthropicMessages,
                "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
            ),
            (
                UpstreamFormat::GeminiNative,
                "data: {\"candidates\":[{\"finishReason\":\"STOP\"}]}\n\n",
            ),
        ];
        for (format, fixture) in fixtures {
            for split in 1..fixture.len() {
                let mut detector = UpstreamTerminalDetector::new(format);
                detector.push(&fixture.as_bytes()[..split]).unwrap();
                assert_eq!(detector.terminal(), None, "format={format:?} split={split}");
                detector.push(&fixture.as_bytes()[split..]).unwrap();
                assert_eq!(
                    detector.terminal(),
                    Some(UpstreamTerminal::Completed),
                    "format={format:?} split={split}"
                );
            }
        }
    }

    #[test]
    fn ignores_keepalive_and_non_terminal_events() {
        let mut detector = UpstreamTerminalDetector::new(UpstreamFormat::OpenAiResponses);
        detector.push(b": keepalive\n\n").unwrap();
        detector
            .push(b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"hi\"}\n\n")
            .unwrap();
        detector
            .push(b"data: {\"type\":\"response.created\",\"error\":null}\n\n")
            .unwrap();
        assert_eq!(detector.terminal(), None);
    }

    #[test]
    fn responses_requires_response_terminal_and_rejects_bare_done_as_authority() {
        let mut detector = UpstreamTerminalDetector::new(UpstreamFormat::OpenAiResponses);
        detector.push(b"data: [DONE]\n\n").unwrap();
        assert_eq!(detector.terminal(), None);

        detector
            .push(b"event: response.completed\ndata: {\"type\":\"response.completed\"}\n\n")
            .unwrap();
        assert_eq!(detector.terminal(), Some(UpstreamTerminal::Completed));
    }

    #[test]
    fn detects_blocked_gemini_prompt() {
        let mut detector = UpstreamTerminalDetector::new(UpstreamFormat::GeminiNative);
        detector
            .push(b"data: {\"response\":{\"promptFeedback\":{\"blockReason\":\"SAFETY\"}}}\n\n")
            .unwrap();
        assert_eq!(detector.terminal(), Some(UpstreamTerminal::Completed));
    }

    #[test]
    fn protocol_event_capacities_preserve_large_responses_support() {
        assert_eq!(
            UpstreamTerminalDetector::new(UpstreamFormat::OpenAiResponses).max_event_bytes(),
            MAX_OPENAI_RESPONSES_TERMINAL_EVENT_BYTES
        );
        for format in [
            UpstreamFormat::OpenAiChat,
            UpstreamFormat::AnthropicMessages,
            UpstreamFormat::GeminiNative,
        ] {
            assert_eq!(
                UpstreamTerminalDetector::new(format).max_event_bytes(),
                MAX_TERMINAL_EVENT_BYTES,
                "format={format:?}"
            );
        }
    }
}
