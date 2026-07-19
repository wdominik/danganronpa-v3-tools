//! Spike Chunsoft LZSS codec used inside SPC archives.
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

/// Number of bits in the match hash: one bucket per distinct 2-byte prefix.
const SPC_HASH_BITS: usize = 16;
/// Hash-table size (`1 << SPC_HASH_BITS` = 65536), one slot per 2-byte value.
const SPC_HASH_SIZE: usize = 1 << SPC_HASH_BITS;
/// "No position" sentinel for the hash-chain head/prev tables. A real input
/// position can never equal `usize::MAX` (it would exceed the address space).
const NO_POS: usize = usize::MAX;

/// Errors produced by the SPC LZSS codec.
#[derive(Debug, Error)]
pub enum SpcError {
    #[error("unexpected end of compressed stream at byte {pos}")]
    UnexpectedEof { pos: usize },

    #[error("back-reference out of range at byte {pos}")]
    BadBackreference { pos: usize },

    #[error("decompressed {got} bytes but expected {expected}")]
    SizeMismatch { got: usize, expected: usize },
}

/// Result alias for SPC-LZSS codec operations.
pub type SpcResult<T> = Result<T, SpcError>;

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
/// `expected_size` is the uncompressed length declared in the SPC entry
/// header. The function checks the decoded output against it on completion.
///
/// # Errors
///
/// Returns an error if the stream ends mid-backreference, a backreference
/// points before the start of the output (window underflow), or the
/// decompressed length doesn't match `expected_size` on completion.
pub fn decompress(input: &[u8], expected_size: usize) -> SpcResult<Vec<u8>> {
    // Cap the pre-allocation: each 2-byte back-reference emits at most
    // `SPC_SEQUENCE_MAX_SIZE` bytes, so the output can never exceed
    // `input.len() * SPC_SEQUENCE_MAX_SIZE`. A malformed `expected_size` then
    // can't force a huge allocation; the final size check still validates the
    // real length. Legitimate streams are well within this bound.
    let capacity = expected_size.min(input.len().saturating_mul(SPC_SEQUENCE_MAX_SIZE));
    let mut out: Vec<u8> = Vec::with_capacity(capacity);
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
                    return Err(SpcError::BadBackreference { pos });
                };
                if src >= out.len() {
                    return Err(SpcError::BadBackreference { pos });
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

/// Compress to the SPC-LZSS format using a hash-chain longest-match search.
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

    // Hash-chain match index. `head[hash2(i)]` is the most-recent position with
    // that 2-byte prefix (or `NO_POS`); `prev[p]` links to the next-older one.
    // Heap-allocated on purpose: `head` is 512 KiB on 64-bit, and this codec
    // runs on `drv3-translate`'s rayon worker threads (~2 MiB stacks), where a
    // stack array that size would risk overflow.
    let mut head = vec![NO_POS; SPC_HASH_SIZE];
    let mut prev = vec![NO_POS; input.len()];
    // High-water mark of positions already linked into the chains. Monotonic,
    // so every position is inserted exactly once (O(n) total).
    let mut next_insert = 0usize;

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

        // Link every position we've advanced past — including those covered by
        // a previous backreference — so the chain search sees the whole window,
        // exactly like the old full-window scan. Positions are linked in
        // increasing order, so every chain descends strictly (which bounds the
        // search walk). The final byte has no 2-byte prefix and is never linked.
        while next_insert < pos {
            if next_insert + 1 < input.len() {
                let h = hash2(input, next_insert);
                prev[next_insert] = head[h];
                head[h] = next_insert;
            }
            next_insert += 1;
        }

        let window_start = pos.saturating_sub(SPC_WINDOW_MAX_SIZE);
        let max_len = SPC_SEQUENCE_MAX_SIZE.min(input.len() - pos);
        let (best_len, best_offset) =
            find_longest_match(input, &head, &prev, window_start, pos, max_len);

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

/// Hash of the 2-byte sequence at `i`. The minimum match length is 2, so two
/// bytes fully determine a position's chain, and `(b0 << 8) | b1` is a
/// bijection on byte pairs (65536 values, no collisions) — "same hash" is
/// exactly "same first two bytes". Precondition: `i + 1 < input.len()`.
#[inline]
fn hash2(input: &[u8], i: usize) -> usize {
    (usize::from(input[i]) << 8) | usize::from(input[i + 1])
}

/// Length of the match between window position `off` and the current position
/// `pos`, capped at `max_len`. This is the original inner scan verbatim,
/// including the self-referential overlap branch: an LZSS match may extend at
/// or past `pos`, referencing bytes the decoder reproduces by periodic repeat.
#[inline]
fn extend_match(input: &[u8], off: usize, pos: usize, max_len: usize) -> usize {
    let mut len = 0usize;
    while len < max_len {
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
    len
}

/// Longest-match search over the hash chain for `input[pos..]`.
///
/// Walks the chain `head[hash2(pos)] -> prev[..]` from the most-recent position
/// backwards. Because every past position is linked (see [`compress`]) and the
/// hash is collision-free, the chain is *exactly* the set of positions whose
/// first two bytes equal `input[pos..pos + 2]` — every candidate that could
/// yield a match of length >= 2. The chain descends strictly, so the first
/// candidate older than `window_start` ends the walk. Walking the full chain
/// (no depth cap) keeps the returned length identical to a full-window scan
/// whenever that length is >= 2, so the compression ratio is unchanged.
fn find_longest_match(
    input: &[u8],
    head: &[usize],
    prev: &[usize],
    window_start: usize,
    pos: usize,
    max_len: usize,
) -> (usize, usize) {
    // Without two bytes at `pos` there is no hash and no >= 2 match.
    if max_len == 0 || pos + 1 >= input.len() {
        return (0, window_start);
    }

    let mut best_len = 0usize;
    let mut best_off = window_start; // ignored by the caller unless best_len >= 2

    let mut cand = head[hash2(input, pos)];
    while cand != NO_POS && cand >= window_start {
        let len = extend_match(input, cand, pos, max_len);
        if len > best_len {
            best_len = len;
            best_off = cand;
            if best_len == max_len {
                break;
            }
        }
        cand = prev[cand];
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

    #[test]
    fn absurd_expected_size_does_not_over_allocate() {
        // A hostile `expected_size` must not trigger a giant pre-allocation or
        // panic; the length check reports the mismatch instead.
        let compressed = compress(b"hello");
        let err = decompress(&compressed, usize::MAX).unwrap_err();
        assert!(matches!(err, SpcError::SizeMismatch { .. }));
    }

    #[test]
    fn truncated_backreference_errors_without_panic() {
        // A flag byte announces a back-reference (bit 7 clear → reversed bit 0
        // clear) but only one of its two bytes follows — must error, not panic.
        let err = decompress(&[0x00, 0x00], 100).unwrap_err();
        assert!(matches!(err, SpcError::UnexpectedEof { .. }));
    }

    #[test]
    fn backreference_before_output_start_errors() {
        // A back-reference at the very start points before the output; the
        // sliding window underflows and must error, not panic.
        let err = decompress(&[0x00, 0x00, 0x00], 100).unwrap_err();
        assert!(matches!(err, SpcError::BadBackreference { .. }));
    }

    #[test]
    fn long_single_chain_round_trips() {
        // A 2-byte-periodic buffer builds one very long hash chain, stressing
        // the full-chain walk and its window-boundary termination.
        let input = b"AB".repeat(2000);
        round_trip(&input);
    }

    #[test]
    fn near_window_boundary_repeat_round_trips() {
        // A repeat whose period sits just under the 1024-byte window forces
        // max-distance backreferences near window offset 0.
        let unit: Vec<u8> = (0..1000u32).map(|i| (i.wrapping_mul(31)) as u8).collect();
        let mut input = unit.clone();
        input.extend_from_slice(&unit);
        input.extend_from_slice(&unit);
        round_trip(&input);
    }

    #[test]
    fn hash_chain_finds_same_length_as_full_scan() {
        // Invariant that keeps the compression ratio unchanged: the hash-chain
        // matcher must return the SAME best length as a brute-force full-window
        // scan (offsets may differ on ties; lengths must not). Only matches
        // >= SPC_SEQUENCE_MIN_SIZE are encoded, so both are clamped to that gate
        // — below it a raw byte is emitted regardless of which offset won.
        fn naive_len(input: &[u8], window_start: usize, pos: usize, max_len: usize) -> usize {
            (window_start..pos)
                .map(|off| extend_match(input, off, pos, max_len))
                .max()
                .unwrap_or(0)
        }

        let mut inputs: Vec<Vec<u8>> = vec![b"AB".repeat(600), b"The quick brown fox. ".repeat(80)];
        // Low-entropy pseudo-random data (few distinct bytes → long chains).
        let mut lcg: u64 = 0x1234_5678_9ABC_DEF0;
        let mut noisy = Vec::with_capacity(3000);
        for _ in 0..3000 {
            lcg = lcg
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            noisy.push(((lcg >> 40) as u8) & 0x0F);
        }
        inputs.push(noisy);

        for input in &inputs {
            let mut head = vec![NO_POS; SPC_HASH_SIZE];
            let mut prev = vec![NO_POS; input.len()];
            let mut next_insert = 0usize;
            let mut pos = 0usize;
            while pos < input.len() {
                while next_insert < pos {
                    if next_insert + 1 < input.len() {
                        let h = hash2(input, next_insert);
                        prev[next_insert] = head[h];
                        head[h] = next_insert;
                    }
                    next_insert += 1;
                }
                let window_start = pos.saturating_sub(SPC_WINDOW_MAX_SIZE);
                let max_len = SPC_SEQUENCE_MAX_SIZE.min(input.len() - pos);
                let (chain_len, _) =
                    find_longest_match(input, &head, &prev, window_start, pos, max_len);
                let brute = naive_len(input, window_start, pos, max_len);
                let gate = |len: usize| if len >= SPC_SEQUENCE_MIN_SIZE { len } else { 0 };
                assert_eq!(
                    gate(chain_len),
                    gate(brute),
                    "length mismatch at pos {pos}: chain={chain_len}, brute={brute}"
                );
                pos += if chain_len >= SPC_SEQUENCE_MIN_SIZE {
                    chain_len
                } else {
                    1
                };
            }
        }
    }
}
