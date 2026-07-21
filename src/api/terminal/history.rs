//! Bounded ring buffer for PTY output replay.

#[derive(Debug, Clone)]
pub(crate) struct HistoryBuffer {
    chunks: Vec<Vec<u8>>,
    total: usize,
    capacity: usize,
}

impl HistoryBuffer {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            chunks: Vec::new(),
            total: 0,
            capacity: capacity.max(1),
        }
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.total
    }

    pub(crate) fn append(&mut self, data: &[u8]) {
        if data.is_empty() || self.capacity == 0 {
            return;
        }
        if data.len() >= self.capacity {
            self.chunks.clear();
            self.chunks
                .push(data[data.len() - self.capacity..].to_vec());
            self.total = self.capacity;
            return;
        }
        self.chunks.push(data.to_vec());
        self.total += data.len();
        while self.total > self.capacity {
            let Some(first) = self.chunks.first_mut() else {
                break;
            };
            let overflow = self.total - self.capacity;
            if overflow >= first.len() {
                self.total -= first.len();
                self.chunks.remove(0);
            } else {
                first.drain(..overflow);
                self.total -= overflow;
            }
        }
    }

    /// Snapshot for replay, chunked to avoid one giant allocation at the wire layer.
    pub(crate) fn snapshot_chunks(&self, max_chunk: usize) -> Vec<Vec<u8>> {
        let max_chunk = max_chunk.max(1);
        let mut out = Vec::new();
        let mut current = Vec::new();
        for chunk in &self.chunks {
            if current.len() + chunk.len() > max_chunk && !current.is_empty() {
                out.push(std::mem::take(&mut current));
            }
            if chunk.len() > max_chunk {
                if !current.is_empty() {
                    out.push(std::mem::take(&mut current));
                }
                for piece in chunk.chunks(max_chunk) {
                    out.push(piece.to_vec());
                }
            } else {
                current.extend_from_slice(chunk);
            }
        }
        if !current.is_empty() {
            out.push(current);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::HistoryBuffer;

    #[test]
    fn append_evicts_oldest_when_over_capacity() {
        let mut buf = HistoryBuffer::new(8);
        buf.append(b"abcdef");
        buf.append(b"ghij");
        assert!(buf.len() <= 8);
        let flat: Vec<u8> = buf.snapshot_chunks(64).into_iter().flatten().collect();
        assert_eq!(&flat, b"cdefghij");
    }

    #[test]
    fn oversized_append_keeps_tail_only() {
        let mut buf = HistoryBuffer::new(4);
        buf.append(b"0123456789");
        let flat: Vec<u8> = buf.snapshot_chunks(64).into_iter().flatten().collect();
        assert_eq!(&flat, b"6789");
    }

    #[test]
    fn snapshot_chunks_respects_max_chunk() {
        let mut buf = HistoryBuffer::new(64);
        buf.append(b"abcdefghij");
        let chunks = buf.snapshot_chunks(4);
        assert!(chunks.iter().all(|c| c.len() <= 4));
        let flat: Vec<u8> = chunks.into_iter().flatten().collect();
        assert_eq!(&flat, b"abcdefghij");
    }
}
