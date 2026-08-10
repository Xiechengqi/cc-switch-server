use super::error::WireError;
use super::header::{parse_headers, Headers};

pub(super) const PRELUDE_LENGTH: usize = 12;
pub(super) const MIN_FRAME_LENGTH: usize = PRELUDE_LENGTH + 4;
pub(super) const DEFAULT_MAX_FRAME_LENGTH: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Frame {
    pub(super) headers: Headers,
    pub(super) payload: Vec<u8>,
}

pub(super) enum ParseOutcome {
    Incomplete,
    Complete { frame: Frame, consumed: usize },
}

pub(super) fn parse_frame(
    buffer: &[u8],
    max_frame_length: usize,
) -> Result<ParseOutcome, WireError> {
    if buffer.len() < PRELUDE_LENGTH {
        return Ok(ParseOutcome::Incomplete);
    }
    let total_length = u32::from_be_bytes(buffer[0..4].try_into().expect("fixed slice")) as usize;
    let headers_length = u32::from_be_bytes(buffer[4..8].try_into().expect("fixed slice")) as usize;
    if total_length < MIN_FRAME_LENGTH {
        return Err(WireError::FrameTooSmall { size: total_length });
    }
    if total_length > max_frame_length {
        return Err(WireError::FrameTooLarge {
            size: total_length,
            max: max_frame_length,
        });
    }
    if headers_length > total_length - MIN_FRAME_LENGTH {
        return Err(WireError::InvalidFrameLayout {
            total_length,
            headers_length,
        });
    }
    let expected_prelude_crc = u32::from_be_bytes(buffer[8..12].try_into().expect("fixed slice"));
    let actual_prelude_crc = crc32(&buffer[..8]);
    if expected_prelude_crc != actual_prelude_crc {
        return Err(WireError::PreludeCrcMismatch {
            expected: expected_prelude_crc,
            actual: actual_prelude_crc,
        });
    }
    if buffer.len() < total_length {
        return Ok(ParseOutcome::Incomplete);
    }

    let expected_message_crc = u32::from_be_bytes(
        buffer[total_length - 4..total_length]
            .try_into()
            .expect("fixed slice"),
    );
    let actual_message_crc = crc32(&buffer[..total_length - 4]);
    if expected_message_crc != actual_message_crc {
        return Err(WireError::MessageCrcMismatch {
            expected: expected_message_crc,
            actual: actual_message_crc,
        });
    }
    let headers_end = PRELUDE_LENGTH + headers_length;
    let headers = parse_headers(&buffer[PRELUDE_LENGTH..headers_end])?;
    let payload = buffer[headers_end..total_length - 4].to_vec();
    Ok(ParseOutcome::Complete {
        frame: Frame { headers, payload },
        consumed: total_length,
    })
}

pub(crate) fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for byte in bytes {
        crc ^= *byte as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    !crc
}
