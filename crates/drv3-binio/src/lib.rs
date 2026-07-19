//! Bounded binary I/O primitives shared by every `drv3-*` format crate.
//!
//! This crate is the only place where raw byte cursors, endian handling, and
//! string-encoding conventions are implemented. Format-specific crates build
//! their parsers on top of [`Reader`] and [`Writer`].
//!
//! Design choices:
//!
//! * **Endianness is always explicit.** There is no default; every reader and
//!   writer method names the byte order (`_le` / `_be`). The DR V3 formats
//!   mix little- and big-endian within a single file — CPK wrappers are LE
//!   but their `@UTF` payloads are BE; WRD headers are LE but opcode args
//!   are BE; SRD payloads are LE but per-block header sizes are BE. A single
//!   default endian would silently mis-read one half or the other.
//! * **Bounds-checked.** Every read returns a [`BinResult`] carrying the
//!   stream position on failure. Parsers must never panic on malformed input.
//! * **Zero-copy reads.** [`Reader::bytes`] returns `&'a [u8]` views into the
//!   underlying buffer, letting container parsers (CPK, SPC, SRD) hand off
//!   slices to leaf-format parsers without copying.
//! * **Back-patching.** [`Writer::reserve_u32_le`] returns a [`Patch`] handle
//!   so format writers can lay down placeholder offsets and fill them in once
//!   final positions are known.
//! * **64-bit host assumption.** On-disk offsets and sizes are 32- or 64-bit
//!   fields that the parsers narrow to `usize`. This is lossless only where
//!   `usize` is at least 64 bits wide, which the supported desktop targets
//!   (macOS and Windows on x86-64 / arm64) all are. Building for a 32-bit
//!   target could silently truncate a large offset; that is out of scope.

mod encoding;
mod error;
mod reader;
mod writer;

pub use encoding::utf16le_byte_len;
pub use error::{BinError, BinResult};
pub use reader::Reader;
pub use writer::{Patch, Writer};

use std::ops::{Add, Rem, Sub};

/// Round `value` up to the next multiple of `alignment`.
///
/// The doubled modulo yields `value` unchanged when it is already a multiple of
/// `alignment`. This is the single source of truth for the round-up arithmetic
/// every padding site in the workspace needs (the [`Reader`]/[`Writer`]
/// alignment helpers and the CPK/SPC layout code).
///
/// # Panics
///
/// Panics (division by zero) if `alignment` is `0`. Every call site passes a
/// non-zero literal.
#[must_use]
pub fn align_up<T>(value: T, alignment: T) -> T
where
    T: Copy + Add<Output = T> + Sub<Output = T> + Rem<Output = T>,
{
    value + (alignment - value % alignment) % alignment
}

#[cfg(test)]
mod tests {
    use super::align_up;

    #[test]
    fn align_up_rounds_to_the_next_multiple() {
        assert_eq!(align_up(0usize, 16), 0);
        assert_eq!(align_up(1usize, 16), 16);
        assert_eq!(align_up(16usize, 16), 16);
        assert_eq!(align_up(17usize, 16), 32);
        assert_eq!(align_up(31usize, 16), 32);
        // u64 (the CPK layout path uses 64-bit offsets).
        assert_eq!(align_up(0x1234u64, 0x10), 0x1240);
        assert_eq!(align_up(0x1240u64, 0x10), 0x1240);
    }
}
