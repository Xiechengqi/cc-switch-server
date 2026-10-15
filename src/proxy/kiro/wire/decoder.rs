use bytes::{Buf, BytesMut};

use super::error::WireError;
use super::frame::{parse_frame, Frame, ParseOutcome, DEFAULT_MAX_FRAME_LENGTH};

pub(super) const DEFAULT_MAX_BUFFER_LENGTH: usize = 16 * 1024 * 1024 + 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RecoveryPolicy {
    pub(crate) max_errors: usize,
    pub(crate) max_skipped_bytes: usize,
}

impl RecoveryPolicy {
    pub(crate) const STRICT: Self = Self {
        max_errors: 0,
        max_skipped_bytes: 0,
    };

    #[cfg(test)]
    pub(crate) const fn bounded(max_errors: usize, max_skipped_bytes: usize) -> Self {
        Self {
            max_errors,
            max_skipped_bytes,
        }
    }
}

#[derive(Debug)]
pub(crate) struct EventStreamDecoder {
    buffer: BytesMut,
    max_buffer_length: usize,
    max_frame_length: usize,
    recovery: RecoveryPolicy,
    errors: usize,
    skipped: usize,
    failed: Option<WireError>,
}

impl Default for EventStreamDecoder {
    fn default() -> Self {
        Self::strict()
    }
}

impl EventStreamDecoder {
    pub(crate) fn strict() -> Self {
        Self::with_policy(RecoveryPolicy::STRICT)
    }

    pub(crate) fn with_policy(recovery: RecoveryPolicy) -> Self {
        Self {
            buffer: BytesMut::with_capacity(8192),
            max_buffer_length: DEFAULT_MAX_BUFFER_LENGTH,
            max_frame_length: DEFAULT_MAX_FRAME_LENGTH,
            recovery,
            errors: 0,
            skipped: 0,
            failed: None,
        }
    }

    pub(crate) fn feed(&mut self, bytes: &[u8]) -> Result<Vec<Frame>, WireError> {
        if let Some(error) = self.failed.clone() {
            return Err(error);
        }
        let mut frames = self.drain()?;
        let mut remaining = bytes;
        while !remaining.is_empty() {
            let room = self.max_buffer_length.saturating_sub(self.buffer.len());
            if room == 0 {
                return self.fail(WireError::BufferOverflow {
                    size: self.buffer.len().saturating_add(remaining.len()),
                    max: self.max_buffer_length,
                });
            }
            let take = remaining.len().min(room);
            self.buffer.extend_from_slice(&remaining[..take]);
            remaining = &remaining[take..];
            frames.extend(self.drain()?);
        }
        Ok(frames)
    }

    pub(crate) fn finish(&mut self) -> Result<Vec<Frame>, WireError> {
        if let Some(error) = self.failed.clone() {
            return Err(error);
        }
        let frames = self.drain()?;
        if self.buffer.is_empty() {
            Ok(frames)
        } else {
            self.fail(WireError::UnexpectedEof {
                buffered: self.buffer.len(),
            })
        }
    }

    pub(crate) fn retained_bytes(&self) -> usize {
        self.buffer.capacity()
    }

    fn drain(&mut self) -> Result<Vec<Frame>, WireError> {
        let mut frames = Vec::new();
        loop {
            match parse_frame(&self.buffer, self.max_frame_length) {
                Ok(ParseOutcome::Incomplete) => return Ok(frames),
                Ok(ParseOutcome::Complete { frame, consumed }) => {
                    self.buffer.advance(consumed);
                    self.errors = 0;
                    frames.push(frame);
                }
                Err(error) => {
                    if self.recovery == RecoveryPolicy::STRICT {
                        return self.fail(error);
                    }
                    self.errors = self.errors.saturating_add(1);
                    if self.errors > self.recovery.max_errors
                        || self.skipped >= self.recovery.max_skipped_bytes
                    {
                        return self.fail(WireError::RecoveryLimitExceeded {
                            errors: self.errors,
                            skipped: self.skipped,
                        });
                    }
                    self.buffer.advance(1);
                    self.skipped = self.skipped.saturating_add(1);
                }
            }
        }
    }

    fn fail<T>(&mut self, error: WireError) -> Result<T, WireError> {
        self.failed = Some(error.clone());
        Err(error)
    }
}

#[cfg(test)]
mod tests {
    use super::super::frame::crc32;
    use super::*;

    fn empty_frame() -> Vec<u8> {
        let mut frame = Vec::new();
        frame.extend_from_slice(&16_u32.to_be_bytes());
        frame.extend_from_slice(&0_u32.to_be_bytes());
        frame.extend_from_slice(&crc32(&frame).to_be_bytes());
        frame.extend_from_slice(&crc32(&frame).to_be_bytes());
        frame
    }

    #[test]
    fn multiple_valid_frames_in_one_chunk_do_not_exhaust_pending_buffer() {
        let frame = empty_frame();
        for prefix_len in [0, 8, frame.len() - 1] {
            let mut decoder = EventStreamDecoder::strict();
            decoder.max_buffer_length = frame.len() * 2;
            decoder.max_frame_length = frame.len();
            assert!(decoder.feed(&frame[..prefix_len]).unwrap().is_empty());

            let mut chunk = frame[prefix_len..].to_vec();
            chunk.extend_from_slice(&frame);
            chunk.extend_from_slice(&frame);
            assert_eq!(
                decoder.feed(&chunk).unwrap().len(),
                3,
                "prefix={prefix_len}"
            );
            assert!(decoder.finish().unwrap().is_empty());
        }
    }

    #[test]
    fn finish_preserves_a_prior_buffer_limit_failure() {
        let mut decoder = EventStreamDecoder::strict();
        decoder.max_buffer_length = 4;
        assert!(matches!(
            decoder.feed(b"12345"),
            Err(WireError::BufferOverflow { .. })
        ));
        assert!(matches!(
            decoder.finish(),
            Err(WireError::BufferOverflow { .. })
        ));
    }
}
