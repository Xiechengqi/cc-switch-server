use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SseEvent {
    pub(crate) event: Option<String>,
    pub(crate) data: Vec<u8>,
    pub(crate) has_data: bool,
    pub(crate) id: Option<String>,
    pub(crate) retry: Option<String>,
    pub(crate) comment_lines: usize,
    pub(crate) unknown_fields: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SseDecodeError {
    InvalidUtf8(String),
    EventTooLarge { limit: usize },
}

impl fmt::Display for SseDecodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidUtf8(error) => write!(formatter, "SSE event is not UTF-8: {error}"),
            Self::EventTooLarge { limit } => {
                write!(formatter, "SSE event exceeded {limit} bytes")
            }
        }
    }
}

impl std::error::Error for SseDecodeError {}

#[derive(Debug, Default)]
struct PendingSseEvent {
    event: Option<String>,
    data: Vec<u8>,
    has_data: bool,
    id: Option<String>,
    retry: Option<String>,
    comment_lines: usize,
    unknown_fields: usize,
    saw_field: bool,
    wire_bytes: usize,
}

impl PendingSseEvent {
    fn into_event(self) -> Option<SseEvent> {
        self.saw_field.then_some(SseEvent {
            event: self.event,
            data: self.data,
            has_data: self.has_data,
            id: self.id,
            retry: self.retry,
            comment_lines: self.comment_lines,
            unknown_fields: self.unknown_fields,
        })
    }
}

/// Incremental, bounded Server-Sent Events decoder.
///
/// It accepts arbitrary byte chunking, LF, CRLF, and lone CR line endings. The
/// returned event follows the SSE data concatenation rule: multiple `data`
/// fields are joined with a single LF. UTF-8 is validated before any field is
/// interpreted.
#[derive(Debug)]
pub(crate) struct SseEventDecoder {
    pending: Vec<u8>,
    event: PendingSseEvent,
    max_pending_bytes: usize,
    max_event_bytes: usize,
}

impl SseEventDecoder {
    pub(crate) fn new(max_event_bytes: usize) -> Self {
        Self::with_limits(max_event_bytes, max_event_bytes)
    }

    pub(crate) fn with_limits(max_pending_bytes: usize, max_event_bytes: usize) -> Self {
        let max_pending_bytes = max_pending_bytes.max(1);
        Self {
            pending: Vec::new(),
            event: PendingSseEvent::default(),
            max_pending_bytes,
            max_event_bytes: max_event_bytes.max(max_pending_bytes),
        }
    }

    pub(crate) fn push(&mut self, chunk: &[u8]) -> Result<Vec<SseEvent>, SseDecodeError> {
        self.pending.extend_from_slice(chunk);
        self.drain(false)
    }

    pub(crate) fn finish(&mut self) -> Result<Vec<SseEvent>, SseDecodeError> {
        self.drain(true)
    }

    fn drain(&mut self, finish: bool) -> Result<Vec<SseEvent>, SseDecodeError> {
        let mut events = Vec::new();
        while let Some((line_end, delimiter_len)) = next_line_boundary(&self.pending, finish) {
            let line = self.pending[..line_end].to_vec();
            self.pending.drain(..line_end + delimiter_len);
            self.process_line(&line, delimiter_len, &mut events)?;
        }

        if finish && !self.pending.is_empty() {
            let line = std::mem::take(&mut self.pending);
            // Treat an EOF-terminated line as if it had the minimum one-byte
            // line ending so the complete-event bound is identical whether
            // or not the producer wrote a trailing newline.
            self.process_line(&line, 1, &mut events)?;
        }
        if finish {
            self.dispatch(&mut events);
        } else {
            self.ensure_pending_bounded()?;
        }
        Ok(events)
    }

    fn process_line(
        &mut self,
        line: &[u8],
        delimiter_len: usize,
        events: &mut Vec<SseEvent>,
    ) -> Result<(), SseDecodeError> {
        let line = std::str::from_utf8(line)
            .map_err(|error| SseDecodeError::InvalidUtf8(error.to_string()))?;
        if line.is_empty() {
            self.dispatch(events);
            return Ok(());
        }

        self.event.wire_bytes = self
            .event
            .wire_bytes
            .saturating_add(line.len())
            .saturating_add(delimiter_len);
        self.ensure_event_bounded()?;
        self.event.saw_field = true;

        if line.starts_with(':') {
            self.event.comment_lines = self.event.comment_lines.saturating_add(1);
            return Ok(());
        }

        let (field, value) = line
            .split_once(':')
            .map(|(field, value)| (field, value.strip_prefix(' ').unwrap_or(value)))
            .unwrap_or((line, ""));
        match field {
            "event" => {
                self.event.event = (!value.is_empty()).then(|| value.to_string());
            }
            "data" => {
                if self.event.has_data {
                    self.event.data.push(b'\n');
                }
                self.event.data.extend_from_slice(value.as_bytes());
                self.event.has_data = true;
                self.ensure_event_bounded()?;
            }
            "id" => {
                if !value.contains('\0') {
                    self.event.id = Some(value.to_string());
                }
            }
            "retry" => {
                self.event.retry = value
                    .bytes()
                    .all(|byte| byte.is_ascii_digit())
                    .then(|| value.to_string());
            }
            _ => {
                self.event.unknown_fields = self.event.unknown_fields.saturating_add(1);
            }
        }
        Ok(())
    }

    fn dispatch(&mut self, events: &mut Vec<SseEvent>) {
        if let Some(event) = std::mem::take(&mut self.event).into_event() {
            events.push(event);
        }
    }

    fn ensure_pending_bounded(&self) -> Result<(), SseDecodeError> {
        let pending_bytes = self.event.wire_bytes.saturating_add(self.pending.len());
        if pending_bytes <= self.max_pending_bytes {
            Ok(())
        } else {
            Err(SseDecodeError::EventTooLarge {
                limit: self.max_pending_bytes,
            })
        }
    }

    fn ensure_event_bounded(&self) -> Result<(), SseDecodeError> {
        if self.event.wire_bytes <= self.max_event_bytes
            && self.event.data.len() <= self.max_event_bytes
        {
            Ok(())
        } else {
            Err(SseDecodeError::EventTooLarge {
                limit: self.max_event_bytes,
            })
        }
    }
}

fn next_line_boundary(buffer: &[u8], finish: bool) -> Option<(usize, usize)> {
    for (index, byte) in buffer.iter().copied().enumerate() {
        match byte {
            b'\n' => return Some((index, 1)),
            b'\r' => match buffer.get(index + 1) {
                Some(b'\n') => return Some((index, 2)),
                Some(_) => return Some((index, 1)),
                None if finish => return Some((index, 1)),
                None => return None,
            },
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode_in_splits(input: &[u8]) -> Vec<SseEvent> {
        let expected = {
            let mut decoder = SseEventDecoder::new(1024);
            let mut events = decoder.push(input).unwrap();
            events.extend(decoder.finish().unwrap());
            events
        };
        for split in 0..=input.len() {
            let mut decoder = SseEventDecoder::new(1024);
            let mut events = decoder.push(&input[..split]).unwrap();
            events.extend(decoder.push(&input[split..]).unwrap());
            events.extend(decoder.finish().unwrap());
            assert_eq!(events, expected, "split={split}");
        }
        expected
    }

    #[test]
    fn decodes_fields_multiline_data_and_arbitrary_splits() {
        let events = decode_in_splits(
            b": hello\r\nevent: response.output_text.delta\r\nid: 7\r\nretry: 1000\r\ndata: {\r\ndata: \"ok\":true}\r\n\r\n",
        );
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0],
            SseEvent {
                event: Some("response.output_text.delta".to_string()),
                data: b"{\n\"ok\":true}".to_vec(),
                has_data: true,
                id: Some("7".to_string()),
                retry: Some("1000".to_string()),
                comment_lines: 1,
                unknown_fields: 0,
            }
        );
    }

    #[test]
    fn accepts_lf_crlf_lone_cr_and_eof_tail() {
        for input in [
            b"data: one\n\n".as_slice(),
            b"data: one\r\n\r\n".as_slice(),
            b"data: one\r\r".as_slice(),
            b"data: one".as_slice(),
        ] {
            let events = decode_in_splits(input);
            assert_eq!(events.len(), 1, "input={input:?}");
            assert_eq!(events[0].data, b"one");
        }
    }

    #[test]
    fn preserves_empty_data_and_control_only_events() {
        let events = decode_in_splits(b"data:\n\nid: 1\nretry: nope\n\n");
        assert_eq!(events.len(), 2);
        assert!(events[0].has_data);
        assert!(events[0].data.is_empty());
        assert!(!events[1].has_data);
        assert_eq!(events[1].id.as_deref(), Some("1"));
        assert_eq!(events[1].retry, None);
    }

    #[test]
    fn rejects_invalid_utf8_and_oversized_events() {
        let mut invalid = SseEventDecoder::new(64);
        assert!(matches!(
            invalid.push(b"data: \xff\n\n"),
            Err(SseDecodeError::InvalidUtf8(_))
        ));

        let mut oversized = SseEventDecoder::new(8);
        assert_eq!(
            oversized.push(b"data: 123456789\n\n").unwrap_err(),
            SseDecodeError::EventTooLarge { limit: 8 }
        );

        let mut pending = SseEventDecoder::new(8);
        assert_eq!(
            pending.push(b"data: 123").unwrap_err(),
            SseDecodeError::EventTooLarge { limit: 8 }
        );

        let mut split_limits = SseEventDecoder::with_limits(16, 64);
        assert_eq!(
            split_limits.push(b"data: 12345678901").unwrap_err(),
            SseDecodeError::EventTooLarge { limit: 16 }
        );

        let mut complete_beyond_pending = SseEventDecoder::with_limits(16, 64);
        let event = complete_beyond_pending
            .push(b"data: 12345678901\n\n")
            .unwrap();
        assert_eq!(event[0].data, b"12345678901");
    }
}
