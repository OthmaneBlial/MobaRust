use thiserror::Error;

/// Maximum bytes accepted by one native terminal-write request. Larger
/// pastes must be split by the caller instead of occupying an unbounded
/// command-queue item.
pub const MAX_TERMINAL_INPUT_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, Copy, Error, PartialEq, Eq)]
pub enum TerminalInputError {
    #[error("terminal input exceeds the 1 MiB safety limit")]
    TooLarge,
}

pub fn validate_terminal_input(data: &[u8]) -> Result<(), TerminalInputError> {
    if data.len() > MAX_TERMINAL_INPUT_BYTES {
        return Err(TerminalInputError::TooLarge);
    }
    Ok(())
}

/// A UI-sized output chunk. Keeping this type separate makes IPC backpressure
/// policy explicit instead of coupling it to a renderer implementation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputChunk {
    pub bytes: Vec<u8>,
}

/// Batches PTY reads into bounded byte chunks. Chunks can split UTF-8
/// sequences; callers that emit text must decode across chunk boundaries.
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

/// Decodes a byte stream without replacing a UTF-8 sequence split across reads.
#[derive(Debug, Default)]
pub struct Utf8OutputDecoder {
    pending: Vec<u8>,
}

impl Utf8OutputDecoder {
    pub fn push(&mut self, bytes: &[u8]) -> String {
        if self.pending.is_empty()
            && let Ok(text) = std::str::from_utf8(bytes)
        {
            return text.to_owned();
        }

        let mut input = std::mem::take(&mut self.pending);
        input.extend_from_slice(bytes);
        let mut remaining = input.as_slice();
        let mut output = String::new();
        loop {
            match std::str::from_utf8(remaining) {
                Ok(text) => {
                    output.push_str(text);
                    break;
                }
                Err(error) => {
                    let (valid, invalid) = remaining.split_at(error.valid_up_to());
                    output.push_str(std::str::from_utf8(valid).expect("valid UTF-8 prefix"));
                    match error.error_len() {
                        Some(len) => {
                            output.push(char::REPLACEMENT_CHARACTER);
                            remaining = &invalid[len..];
                        }
                        None => {
                            self.pending.extend_from_slice(invalid);
                            break;
                        }
                    }
                }
            }
        }
        output
    }

    pub fn finish(&mut self) -> String {
        if self.pending.is_empty() {
            String::new()
        } else {
            self.pending.clear();
            char::REPLACEMENT_CHARACTER.to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

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

    #[test]
    fn utf8_decoder_preserves_sequences_split_by_bounded_chunks() {
        let mut batcher = OutputBatcher::new(4);
        let mut decoder = Utf8OutputDecoder::default();
        let chunks = batcher.push("abc€".as_bytes());
        assert_eq!(chunks.len(), 1);
        assert_eq!(decoder.push(&chunks[0].bytes), "abc");
        assert_eq!(decoder.push(&batcher.flush().unwrap().bytes), "€");
        assert!(decoder.finish().is_empty());
    }

    #[test]
    fn utf8_decoder_replaces_malformed_and_trailing_incomplete_sequences() {
        let mut decoder = Utf8OutputDecoder::default();
        assert_eq!(decoder.push(&[0xe2]), "");
        assert_eq!(decoder.push(&[b'(', 0xf0]), "�(");
        assert_eq!(decoder.finish(), "�");
        assert!(decoder.finish().is_empty());
    }

    proptest! {
        #[test]
        fn utf8_decoder_matches_whole_buffer_across_chunks(
            bytes in prop::collection::vec(any::<u8>(), 0..256),
            chunk_size in 1usize..64,
        ) {
            let mut decoder = Utf8OutputDecoder::default();
            let mut actual = String::new();
            for chunk in bytes.chunks(chunk_size) {
                actual.push_str(&decoder.push(chunk));
            }
            actual.push_str(&decoder.finish());
            prop_assert_eq!(actual, String::from_utf8_lossy(&bytes));
        }
    }

    #[test]
    fn terminal_input_is_bounded_before_queueing() {
        assert!(validate_terminal_input(&vec![b'x'; MAX_TERMINAL_INPUT_BYTES]).is_ok());
        assert_eq!(
            validate_terminal_input(&vec![b'x'; MAX_TERMINAL_INPUT_BYTES + 1]),
            Err(TerminalInputError::TooLarge)
        );
    }
}
