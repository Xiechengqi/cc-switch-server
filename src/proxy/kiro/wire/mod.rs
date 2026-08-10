mod decoder;
mod error;
mod event;
mod frame;
mod header;

pub(crate) use decoder::EventStreamDecoder;
#[cfg(test)]
pub(crate) use decoder::RecoveryPolicy;
pub(crate) use error::WireError;
pub(crate) use event::{parse_event, Event};
#[cfg(test)]
pub(crate) use frame::crc32;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy::kiro::wire::frame::crc32;

    const WIRE_PROTOCOL_JSON: &str =
        include_str!("../../../../assets/contract/kiro-wire-protocol.json");

    fn string_header(name: &str, value: &str) -> Vec<u8> {
        let mut out = vec![name.len() as u8];
        out.extend_from_slice(name.as_bytes());
        out.push(7);
        out.extend_from_slice(&(value.len() as u16).to_be_bytes());
        out.extend_from_slice(value.as_bytes());
        out
    }

    fn frame(event_type: &str, payload: &[u8]) -> Vec<u8> {
        let headers = string_header(":event-type", event_type);
        frame_with_headers(headers, payload)
    }

    fn frame_with_message_type(event_type: &str, message_type: &str, payload: &[u8]) -> Vec<u8> {
        let mut headers = string_header(":event-type", event_type);
        headers.extend(string_header(":message-type", message_type));
        frame_with_headers(headers, payload)
    }

    fn frame_with_headers(headers: Vec<u8>, payload: &[u8]) -> Vec<u8> {
        let total = 12 + headers.len() + payload.len() + 4;
        let mut out = Vec::with_capacity(total);
        out.extend_from_slice(&(total as u32).to_be_bytes());
        out.extend_from_slice(&(headers.len() as u32).to_be_bytes());
        out.extend_from_slice(&crc32(&out).to_be_bytes());
        out.extend_from_slice(&headers);
        out.extend_from_slice(payload);
        out.extend_from_slice(&crc32(&out).to_be_bytes());
        out
    }

    #[test]
    fn decodes_every_possible_chunk_boundary() {
        let bytes = frame("assistantResponseEvent", br#"{"content":"hello"}"#);
        for split in 0..=bytes.len() {
            let mut decoder = EventStreamDecoder::strict();
            let mut events = decoder.feed(&bytes[..split]).unwrap();
            events.extend(decoder.feed(&bytes[split..]).unwrap());
            events.extend(decoder.finish().unwrap());
            assert_eq!(events.len(), 1, "split={split}");
            assert_eq!(
                parse_event(events.remove(0)).unwrap(),
                Event::AssistantResponse {
                    content: "hello".to_string()
                }
            );
        }
    }

    #[test]
    fn strict_decoder_rejects_crc_corruption_and_stays_failed() {
        let mut bytes = frame("assistantResponseEvent", br#"{"content":"hello"}"#);
        *bytes.last_mut().unwrap() ^= 0xff;
        let mut decoder = EventStreamDecoder::strict();
        assert!(matches!(
            decoder.feed(&bytes),
            Err(WireError::MessageCrcMismatch { .. })
        ));
        assert!(matches!(
            decoder.feed(&[]),
            Err(WireError::MessageCrcMismatch { .. })
        ));
    }

    #[test]
    fn finish_rejects_truncated_frame() {
        let bytes = frame("assistantResponseEvent", br#"{"content":"hello"}"#);
        let mut decoder = EventStreamDecoder::strict();
        assert!(decoder.feed(&bytes[..bytes.len() - 1]).unwrap().is_empty());
        assert!(matches!(
            decoder.finish(),
            Err(WireError::UnexpectedEof { .. })
        ));
    }

    #[test]
    fn unknown_message_type_is_rejected_without_echoing_its_value() {
        let bytes = frame_with_message_type(
            "assistantResponseEvent",
            "unexpected-control-message",
            br#"{"content":"hello"}"#,
        );
        let mut decoder = EventStreamDecoder::strict();
        let frame = decoder.feed(&bytes).unwrap().remove(0);
        let error = parse_event(frame).unwrap_err();
        assert!(matches!(
            error,
            WireError::InvalidMessageType(ref value) if value == "unexpected-control-message"
        ));
        assert!(!error.to_string().contains("unexpected-control-message"));
    }

    #[test]
    fn non_string_message_type_is_not_treated_as_missing() {
        let mut headers = string_header(":event-type", "assistantResponseEvent");
        headers.push(":message-type".len() as u8);
        headers.extend_from_slice(b":message-type");
        headers.push(0);
        let bytes = frame_with_headers(headers, br#"{"content":"hello"}"#);
        let mut decoder = EventStreamDecoder::strict();
        let frame = decoder.feed(&bytes).unwrap().remove(0);
        assert!(matches!(
            parse_event(frame),
            Err(WireError::InvalidHeaderValue("message type"))
        ));
    }

    #[test]
    fn bounded_recovery_can_resynchronize_without_unbounded_scanning() {
        let valid = frame("assistantResponseEvent", br#"{"content":"ok"}"#);
        let mut bytes = vec![0xff, 0xfe];
        bytes.extend_from_slice(&valid);
        let mut decoder = EventStreamDecoder::with_policy(RecoveryPolicy::bounded(32, 32));
        let frames = decoder.feed(&bytes).unwrap();
        assert_eq!(frames.len(), 1);

        let mut decoder = EventStreamDecoder::with_policy(RecoveryPolicy::bounded(1, 1));
        assert!(matches!(
            decoder.feed(&bytes),
            Err(WireError::RecoveryLimitExceeded { .. })
        ));
    }

    #[test]
    fn parses_all_aws_header_value_types() {
        let mut headers = Vec::new();
        for (name, kind, value) in [
            ("t", 0, vec![]),
            ("f", 1, vec![]),
            ("b", 2, vec![0xff]),
            ("s", 3, 1i16.to_be_bytes().to_vec()),
            ("i", 4, 2i32.to_be_bytes().to_vec()),
            ("l", 5, 3i64.to_be_bytes().to_vec()),
            ("a", 6, [2u16.to_be_bytes().as_slice(), &[1, 2]].concat()),
            ("x", 7, [2u16.to_be_bytes().as_slice(), b"ok"].concat()),
            ("d", 8, 4i64.to_be_bytes().to_vec()),
            ("u", 9, vec![5; 16]),
        ] {
            headers.push(name.len() as u8);
            headers.extend_from_slice(name.as_bytes());
            headers.push(kind);
            headers.extend_from_slice(&value);
        }
        let total = 12 + headers.len() + 4;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&(total as u32).to_be_bytes());
        bytes.extend_from_slice(&(headers.len() as u32).to_be_bytes());
        bytes.extend_from_slice(&crc32(&bytes).to_be_bytes());
        bytes.extend_from_slice(&headers);
        bytes.extend_from_slice(&crc32(&bytes).to_be_bytes());
        let mut decoder = EventStreamDecoder::strict();
        let frame = decoder.feed(&bytes).unwrap().remove(0);
        for name in ["t", "f", "b", "s", "i", "l", "a", "x", "d", "u"] {
            assert!(frame.headers.get(name).is_some());
        }
    }

    #[test]
    fn decoder_limits_and_error_codes_match_the_wire_protocol_fixture() {
        let fixture: serde_json::Value = serde_json::from_str(WIRE_PROTOCOL_JSON).unwrap();
        let stream = &fixture["eventStream"];
        assert_eq!(stream["decoderMode"], "strict_fail_closed");
        assert_eq!(stream["preludeCrcRequired"], true);
        assert_eq!(stream["messageCrcRequired"], true);
        assert_eq!(
            stream["messageTypePolicy"],
            "missing_or_event_plus_terminal_error_exception"
        );
        assert_eq!(stream["unexpectedEof"], "error");
        assert_eq!(stream["maxFrameBytes"], frame::DEFAULT_MAX_FRAME_LENGTH);
        assert_eq!(
            stream["maxBufferedBytes"],
            decoder::DEFAULT_MAX_BUFFER_LENGTH
        );
        assert_eq!(RecoveryPolicy::STRICT.max_errors, 0);
        assert_eq!(RecoveryPolicy::STRICT.max_skipped_bytes, 0);

        assert_eq!(
            fixture["errorCodes"]["invalidEventStream"],
            WireError::UnexpectedEof { buffered: 1 }.code()
        );
        assert_eq!(
            fixture["errorCodes"]["eventStreamLimit"],
            WireError::FrameTooLarge {
                size: frame::DEFAULT_MAX_FRAME_LENGTH + 1,
                max: frame::DEFAULT_MAX_FRAME_LENGTH,
            }
            .code()
        );
        assert_eq!(
            fixture["errorCodes"]["upstreamErrorFrame"],
            WireError::Upstream {
                message_type: "error".to_string(),
                code: Some("fixture".to_string()),
                message: "fixture".to_string(),
            }
            .code()
        );
    }
}
