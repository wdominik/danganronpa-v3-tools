//! Compression codecs used by Danganronpa V3.
//!
//! Two unrelated codecs:
//!
//! - [`spc_lzss`] — custom Spike Chunsoft byte-oriented LZSS, used inside SPC
//!   archives. Fully implemented; round-trip via [`spc_lzss::compress`] +
//!   [`spc_lzss::decompress`].
//! - [`crilayla`] — CRIWARE's per-entry CPK compression (an LZ77 variant
//!   with reverse-bit-order encoding). **Header recognition only** in v0.1.
//!   DR V3's CPKs do not apply CRILAYLA at the CPK layer — every shipped
//!   entry has `FileSize == ExtractSize`, so we recognize the magic to fail
//!   loudly but don't carry a real codec. Compressing/decompressing an
//!   actual CRILAYLA stream returns
//!   [`crilayla::CrilaylaError::NotImplemented`].

pub mod crilayla;
pub mod spc_lzss;
