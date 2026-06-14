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

mod encoding;
mod error;
mod reader;
mod writer;

pub use encoding::utf16le_byte_len;
pub use error::{BinError, BinResult};
pub use reader::Reader;
pub use writer::{Patch, Writer};
