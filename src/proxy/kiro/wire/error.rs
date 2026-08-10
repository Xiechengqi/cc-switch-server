use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WireError {
    BufferOverflow {
        size: usize,
        max: usize,
    },
    FrameTooSmall {
        size: usize,
    },
    FrameTooLarge {
        size: usize,
        max: usize,
    },
    InvalidFrameLayout {
        total_length: usize,
        headers_length: usize,
    },
    PreludeCrcMismatch {
        expected: u32,
        actual: u32,
    },
    MessageCrcMismatch {
        expected: u32,
        actual: u32,
    },
    InvalidHeaderName,
    InvalidHeaderType(u8),
    InvalidHeaderValue(&'static str),
    DuplicateHeader(String),
    UnexpectedEof {
        buffered: usize,
    },
    RecoveryLimitExceeded {
        errors: usize,
        skipped: usize,
    },
    InvalidMessageType(String),
    MissingEventType,
    InvalidEventPayload {
        event_type: String,
        message: String,
    },
    Upstream {
        message_type: String,
        code: Option<String>,
        message: String,
    },
}

impl WireError {
    pub(crate) fn code(&self) -> &'static str {
        match self {
            Self::BufferOverflow { .. } | Self::FrameTooLarge { .. } => "KIRO_EVENT_STREAM_LIMIT",
            Self::Upstream { .. } => "KIRO_UPSTREAM_STREAM_ERROR",
            _ => "KIRO_EVENT_STREAM_INVALID",
        }
    }
}

impl fmt::Display for WireError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BufferOverflow { size, max } => {
                write!(formatter, "event stream buffer is {size} bytes; limit is {max}")
            }
            Self::FrameTooSmall { size } => {
                write!(formatter, "event stream frame length {size} is below 16 bytes")
            }
            Self::FrameTooLarge { size, max } => {
                write!(formatter, "event stream frame length {size} exceeds {max}")
            }
            Self::InvalidFrameLayout {
                total_length,
                headers_length,
            } => write!(
                formatter,
                "event stream frame layout is invalid (total={total_length}, headers={headers_length})"
            ),
            Self::PreludeCrcMismatch { expected, actual } => write!(
                formatter,
                "event stream prelude CRC mismatch (expected {expected:#010x}, actual {actual:#010x})"
            ),
            Self::MessageCrcMismatch { expected, actual } => write!(
                formatter,
                "event stream message CRC mismatch (expected {expected:#010x}, actual {actual:#010x})"
            ),
            Self::InvalidHeaderName => formatter.write_str("event stream header name is invalid"),
            Self::InvalidHeaderType(value) => {
                write!(formatter, "event stream header type {value} is invalid")
            }
            Self::InvalidHeaderValue(kind) => {
                write!(formatter, "event stream {kind} header value is invalid")
            }
            Self::DuplicateHeader(name) => {
                write!(formatter, "event stream contains duplicate header {name}")
            }
            Self::UnexpectedEof { buffered } => write!(
                formatter,
                "event stream ended with {buffered} bytes of an incomplete frame"
            ),
            Self::RecoveryLimitExceeded { errors, skipped } => write!(
                formatter,
                "event stream recovery limit exceeded after {errors} errors and {skipped} skipped bytes"
            ),
            Self::InvalidMessageType(_) => {
                formatter.write_str("event stream message type is not recognized")
            }
            Self::MissingEventType => formatter.write_str("event stream message has no event type"),
            Self::InvalidEventPayload {
                event_type,
                message,
            } => write!(formatter, "invalid {event_type} payload: {message}"),
            Self::Upstream {
                message_type,
                code,
                message,
            } => {
                write!(formatter, "Kiro {message_type}")?;
                if let Some(code) = code {
                    write!(formatter, " ({code})")?;
                }
                if !message.is_empty() {
                    write!(formatter, ": {message}")?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for WireError {}
