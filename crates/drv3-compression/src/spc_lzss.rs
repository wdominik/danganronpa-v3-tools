//! Spike-Chunsoft LZSS codec used inside SPC archives.
//!
//! A byte-oriented sliding-window codec with these specifics:
//!
//! - **Sliding window**: 1024 bytes (the most recent decoded output).
//! - **Backreference**: encoded as a `u16 LE` packing
//!   `(length - 2) << 10 | offset`, so 6 bits of length (matches of 2..=65)
//!   and 10 bits of offset (0..=1023).
//! - **Source index** of a backref: `out.len() - 1024 + offset`. If the
//!   computed index points before the start of the stream the input is
//!   malformed.
//! - **Entries are grouped in blocks of 8**: each block is preceded by a
//!   single *flag byte* whose 8 bits classify the next 8 entries as either
//!   "raw byte" (1) or "backreference" (0).
//! - **The flag byte is stored with its bits reversed**: bit 0 of the
//!   on-disk byte corresponds to the 8th (last) entry of the block, bit 7 to
//!   the 1st. We reverse it at I/O time so the decoder can simply test
//!   `flag & 1` and shift right.
//!
//! These two writers/readers form a round-trip pair: a compressed stream
//! produced by [`compress`] decompresses back via [`decompress`] to the
//! original bytes. The compressed form is *not* guaranteed to match a
//! specific reference encoder byte-for-byte — many valid encodings exist
//! for the same input.

use thiserror::Error;

const SPC_WINDOW_MAX_SIZE: usize = 1024;
const SPC_SEQUENCE_MAX_SIZE: usize = 65;
const SPC_SEQUENCE_MIN_SIZE: usize = 2;

/// Errors produced by the SPC LZSS codec.
#[derive(Debug, Error)]
pub enum SpcError {
    #[error("unexpected end of compressed stream at byte {pos}")]
    UnexpectedEof { pos: usize },

    #[error("decompressed {got} bytes but expected {expected}")]
    SizeMismatch { got: usize, expected: usize },
}

/// Reverse the bit order within a single byte. The codec stores flag bytes
/// with bit 0 representing the *last* (8th) entry of each block and bit 7
/// the first; reversing puts them in natural MSB-first order so the decoder
/// can shift-and-test the low bit, and the encoder can OR-and-shift in the
/// straightforward direction.
#[inline]
fn reverse_bits_8(b: u8) -> u8 {
    b.reverse_bits()
}

/// Decompress an SPC-LZSS stream.
///
/// `expected_size` is the original uncompressed length (from
/// `SpcEntry.current_size` in the archive header). The function checks
/// this on completion.
///
/// # Errors
///
/// Returns an error if the stream ends mid-backreference, a backreference
/// points before the start of the output (window underflow), or the
/// decompressed length doesn't match `expected_size` on completion.
pub fn decompress(input: &[u8], expected_size: usize) -> Result<Vec<u8>, SpcError> {
    let mut out: Vec<u8> = Vec::with_capacity(expected_size);
    let mut pos = 0;
    let mut flag: u32 = 1; // sentinel

    while pos < input.len() {
        if flag == 1 {
            // Fetch new flag byte. The 0x100 sentinel detects when all 8
            // entries of a block have been consumed.
            flag = 0x100 | u32::from(reverse_bits_8(input[pos]));
            pos += 1;
            if pos >= input.len() {
                break;
            }
        }

        if flag & 1 == 1 {
            // Raw byte.
            out.push(input[pos]);
            pos += 1;
        } else {
            // Backreference: u16 LE packing `(length - 2) << 10 | offset`.
            // High 6 bits → length in 2..=65; low 10 bits → window offset
            // in 0..=1023.
            if pos + 2 > input.len() {
                return Err(SpcError::UnexpectedEof { pos });
            }
            let b = u16::from_le_bytes([input[pos], input[pos + 1]]);
            pos += 2;
            let count = ((b >> 10) as usize) + SPC_SEQUENCE_MIN_SIZE;
            let offset = (b & 0x3FF) as usize;

            for _ in 0..count {
                // Source index = `out.len() - 1024 + offset`. We compute it as
                // `(out.len() + offset) - 1024` to avoid an unsigned underflow
                // while `out.len() < 1024` (the common early-stream case).
                // If the result is negative the backref points before the
                // start of the stream — malformed input.
                let Some(src) = out
                    .len()
                    .checked_add(offset)
                    .and_then(|s| s.checked_sub(SPC_WINDOW_MAX_SIZE))
                else {
                    return Err(SpcError::UnexpectedEof { pos });
                };
                if src >= out.len() {
                    return Err(SpcError::UnexpectedEof { pos });
                }
                let byte = out[src];
                out.push(byte);
            }
        }

        flag >>= 1;
    }

    if out.len() != expected_size {
        return Err(SpcError::SizeMismatch {
            got: out.len(),
            expected: expected_size,
        });
    }
    Ok(out)
}

/// Compress to the SPC-LZSS format with a naive longest-match search.
///
/// The output is byte-for-byte compatible with [`decompress`] but is **not**
/// guaranteed to match a specific reference encoder's output — many valid
/// encodings exist for the same input. Round-trip via decompress is the
/// correctness guarantee.
pub fn compress(input: &[u8]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    let mut pos = 0;
    let mut flag: u8 = 0;
    let mut cur_bit = 0;
    let mut block_buf: Vec<u8> = Vec::with_capacity(16);

    loop {
        // Flush block when full or input exhausted.
        if cur_bit == 8 || pos >= input.len() {
            // First-iteration sentinel: only flush when block_buf is non-empty
            // (or after at least one bit was set).
            if cur_bit > 0 {
                out.push(reverse_bits_8(flag));
                out.extend_from_slice(&block_buf);
                flag = 0;
                cur_bit = 0;
                block_buf.clear();
            }
            if pos >= input.len() {
                break;
            }
        }

        let window_start = pos.saturating_sub(SPC_WINDOW_MAX_SIZE);
        let max_len = SPC_SEQUENCE_MAX_SIZE.min(input.len() - pos);
        let (best_len, best_offset) = find_longest_match(input, window_start, pos, max_len);

        if best_len >= SPC_SEQUENCE_MIN_SIZE {
            // Backreference. The on-disk offset is computed so the decoder's
            // `out.len() - SPC_WINDOW_MAX_SIZE + offset` yields `best_offset`.
            let window_pos =
                (SPC_WINDOW_MAX_SIZE - (pos - window_start)) + (best_offset - window_start);
            let encoded: u16 =
                (window_pos as u16) | (((best_len - SPC_SEQUENCE_MIN_SIZE) as u16) << 10);
            block_buf.extend_from_slice(&encoded.to_le_bytes());
            pos += best_len;
        } else {
            flag |= 1 << cur_bit;
            block_buf.push(input[pos]);
            pos += 1;
        }

        cur_bit += 1;
    }

    out
}

/// Naive longest-match search: scan the window for the longest run starting
/// at `pos` that already appears in `input[window_start..pos]`.
fn find_longest_match(
    input: &[u8],
    window_start: usize,
    pos: usize,
    max_len: usize,
) -> (usize, usize) {
    if max_len == 0 {
        return (0, window_start);
    }
    let mut best_len = 0usize;
    let mut best_off = window_start;
    for off in window_start..pos {
        let mut len = 0usize;
        while len < max_len {
            // LZSS allows the match to overlap the current position
            // (self-referential prefix repetition).
            let src = off + len;
            let src_byte = if src < pos {
                input[src]
            } else {
                input[pos + (len - (pos - off))]
            };
            if src_byte != input[pos + len] {
                break;
            }
            len += 1;
        }
        if len > best_len {
            best_len = len;
            best_off = off;
            if best_len == max_len {
                break;
            }
        }
    }
    (best_len, best_off)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(input: &[u8]) {
        let compressed = compress(input);
        let decompressed = decompress(&compressed, input.len()).expect("decompress");
        assert_eq!(
            decompressed,
            input,
            "round trip mismatch for {} bytes",
            input.len()
        );
    }

    #[test]
    fn empty_input_round_trips() {
        round_trip(&[]);
    }

    #[test]
    fn single_byte_round_trips() {
        round_trip(&[0x42]);
    }

    #[test]
    fn short_random_input_round_trips() {
        let input: Vec<u8> = (0..50).map(|i| (i * 7) as u8).collect();
        round_trip(&input);
    }

    #[test]
    fn highly_repetitive_input_compresses() {
        let input = vec![0xAA; 200];
        let compressed = compress(&input);
        assert!(
            compressed.len() < input.len(),
            "expected compression on repetitive data"
        );
        round_trip(&input);
    }

    #[test]
    fn long_pattern_with_backreferences() {
        let pattern = b"The quick brown fox jumps over the lazy dog. ";
        let mut input = Vec::new();
        for _ in 0..50 {
            input.extend_from_slice(pattern);
        }
        let compressed = compress(&input);
        assert!(compressed.len() < input.len() / 2);
        round_trip(&input);
    }

    #[test]
    fn varied_random_data_round_trips() {
        // Deterministic pseudo-random sequence (LCG).
        let mut state: u64 = 0xDEAD_BEEF;
        let mut input = Vec::with_capacity(4096);
        for _ in 0..4096 {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            input.push((state >> 32) as u8);
        }
        round_trip(&input);
    }

    #[test]
    fn reverse_bits_8_is_correct() {
        assert_eq!(reverse_bits_8(0b1011_0100), 0b0010_1101);
        assert_eq!(reverse_bits_8(0xFF), 0xFF);
        assert_eq!(reverse_bits_8(0x00), 0x00);
        assert_eq!(reverse_bits_8(0x01), 0x80);
    }

    #[test]
    fn boundary_window_sizes() {
        // Just under, at, and just over the 1024-byte window.
        for &n in &[1023usize, 1024, 1025, 2048] {
            let input: Vec<u8> = (0..n).map(|i| (i ^ (i >> 3)) as u8).collect();
            round_trip(&input);
        }
    }

    #[test]
    fn max_sequence_length() {
        // Build an input that forces backreferences of the maximum 65-byte length.
        let input = vec![b'A'; 70];
        round_trip(&input);
    }
}
