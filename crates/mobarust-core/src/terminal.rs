/// A UI-sized output chunk. Keeping this type separate makes IPC backpressure
/// policy explicit instead of coupling it to a renderer implementation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputChunk {
    pub bytes: Vec<u8>,
}

/// Batches PTY reads into bounded chunks. It never splits a UTF-8 sequence on
/// purpose (the renderer can decode lossily), and it never emits an unbounded
/// allocation for a noisy process.
#[derive(Debug, Clone)]
pub struct OutputBatcher {
    max_bytes: usize,
    pending: Vec<u8>,
}

impl OutputBatcher {
    pub fn new(max_bytes: usize) -> Self {
        assert!(max_bytes > 0, "output batch size must be positive");
        Self {
            max_bytes,
            pending: Vec::with_capacity(max_bytes),
        }
    }

    pub fn push(&mut self, bytes: &[u8]) -> Vec<OutputChunk> {
        let mut chunks = Vec::new();
        let mut remaining = bytes;

        while !remaining.is_empty() {
            let room = self.max_bytes - self.pending.len();
            let take = room.min(remaining.len());
            self.pending.extend_from_slice(&remaining[..take]);
            remaining = &remaining[take..];

            if self.pending.len() == self.max_bytes {
                chunks.push(self.take_pending());
            }
        }

        chunks
    }

    pub fn flush(&mut self) -> Option<OutputChunk> {
        (!self.pending.is_empty()).then(|| self.take_pending())
    }

    fn take_pending(&mut self) -> OutputChunk {
        OutputChunk {
            bytes: std::mem::replace(&mut self.pending, Vec::with_capacity(self.max_bytes)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batches_small_reads_until_flush() {
        let mut batcher = OutputBatcher::new(8);
        assert!(batcher.push(b"hello").is_empty());
        assert_eq!(batcher.flush().unwrap().bytes, b"hello");
    }

    #[test]
    fn splits_noisy_output_at_the_ipc_limit() {
        let mut batcher = OutputBatcher::new(4);
        let chunks = batcher.push(b"123456789");
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].bytes, b"1234");
        assert_eq!(chunks[1].bytes, b"5678");
        assert_eq!(batcher.flush().unwrap().bytes, b"9");
    }
}
