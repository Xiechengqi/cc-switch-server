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
        let size = self.buffer.len().saturating_add(bytes.len());
        if size > self.max_buffer_length {
            return self.fail(WireError::BufferOverflow {
                size,
                max: self.max_buffer_length,
            });
        }
        self.buffer.extend_from_slice(bytes);
        self.drain()
    }

    pub(crate) fn finish(&mut self) -> Result<Vec<Frame>, WireError> {
        let frames = self.drain()?;
        if self.buffer.is_empty() {
            Ok(frames)
        } else {
            self.fail(WireError::UnexpectedEof {
                buffered: self.buffer.len(),
            })
        }
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
