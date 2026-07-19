//! CRILAYLA codec (CRIWARE CPK per-entry compression).
//!
//! ## Status: not implemented in v0.1.
//!
//! CRILAYLA is an LZ77-style codec with a backward byte-and-bit-order
//! bitstream and variable-length (2/3/5/8-bit) match-length codes. CRIWARE
//! ships the algorithm; clean-room implementations exist in `CriFsV2Lib`
//! and `CriPakTools`.
//!
//! DR V3's CPKs do not apply CRILAYLA to individual TOC entries — every
//! shipped entry has `FileSize == ExtractSize`, so the recogniser only
//! exists to fail loudly if a CRILAYLA blob ever appears (which would
//! indicate a CPK from another title fed through this toolkit by mistake).
//!
//! For v0.1 we expose header recognition (so the CPK reader can detect
//! CRILAYLA blobs and bail with a clear error) but neither decompress nor
//! compress is implemented. Carrying the codec is parked until a real input
//! requires it.

use thiserror::Error;

const MAGIC: &[u8; 8] = b"CRILAYLA";
pub(crate) const HEADER_SIZE: usize = 0x10;

/// Errors produced by the CRILAYLA codec.
#[derive(Debug, Error)]
pub enum CrilaylaError {
    #[error(
        "CRILAYLA codec is not implemented in v0.1 (DR V3 does not use CRILAYLA at the CPK layer)"
    )]
    NotImplemented,

    #[error("input does not start with the CRILAYLA magic")]
    BadMagic,

    #[error("CRILAYLA header truncated (need at least {HEADER_SIZE} bytes, got {got})")]
    Truncated { got: usize },
}

/// Result alias for CRILAYLA codec operations.
#[allow(
    dead_code,
    reason = "parked CRILAYLA codec; kept and test-covered for un-parking"
)]
pub(crate) type CrilaylaResult<T> = Result<T, CrilaylaError>;

/// A CRILAYLA blob's header sizes.
#[allow(
    dead_code,
    reason = "parked CRILAYLA codec; kept and test-covered for un-parking"
)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CrilaylaHeader {
    /// Uncompressed (extracted) size in bytes, from header offset 0x08.
    pub(crate) uncompressed_size: u32,
    /// Compressed payload size in bytes, from header offset 0x0C.
    pub(crate) compressed_size: u32,
}

/// Inspect a CRILAYLA blob's header without decompressing.
///
/// Reads the uncompressed and compressed sizes at offsets 0x08 and 0x0C.
///
/// # Errors
///
/// Returns an error if `input` is shorter than the 16-byte header, or if
/// the first 8 bytes don't equal the `CRILAYLA` magic.
#[allow(
    dead_code,
    reason = "parked CRILAYLA codec; kept and test-covered for un-parking"
)]
pub(crate) fn read_header(input: &[u8]) -> CrilaylaResult<CrilaylaHeader> {
    if input.len() < HEADER_SIZE {
        return Err(CrilaylaError::Truncated { got: input.len() });
    }
    if &input[0..8] != MAGIC {
        return Err(CrilaylaError::BadMagic);
    }
    // The earlier `input.len() < HEADER_SIZE` check guarantees these
    // four-byte slices are in bounds; the array-pattern read is panic-free.
    let uncompressed_size =
        u32::from_le_bytes([input[0x08], input[0x09], input[0x0A], input[0x0B]]);
    let compressed_size = u32::from_le_bytes([input[0x0C], input[0x0D], input[0x0E], input[0x0F]]);
    Ok(CrilaylaHeader {
        uncompressed_size,
        compressed_size,
    })
}

/// Return whether the input begins with the CRILAYLA magic.
#[must_use]
pub fn is_crilayla(input: &[u8]) -> bool {
    input.len() >= 8 && &input[0..8] == MAGIC
}

/// Decompress a CRILAYLA stream. **Not implemented in v0.1.**
///
/// # Errors
///
/// Always returns [`CrilaylaError::NotImplemented`] in v0.1. The CPK
/// reader uses [`is_crilayla`] to refuse compressed entries up front.
#[allow(
    dead_code,
    reason = "parked CRILAYLA codec; kept and test-covered for un-parking"
)]
pub(crate) fn decompress(_input: &[u8]) -> CrilaylaResult<Vec<u8>> {
    Err(CrilaylaError::NotImplemented)
}

/// Compress to the CRILAYLA format. **Not implemented in v0.1.**
///
/// # Errors
///
/// Always returns [`CrilaylaError::NotImplemented`] in v0.1.
#[allow(
    dead_code,
    reason = "parked CRILAYLA codec; kept and test-covered for un-parking"
)]
pub(crate) fn compress(_input: &[u8]) -> CrilaylaResult<Vec<u8>> {
    Err(CrilaylaError::NotImplemented)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_magic() {
        assert!(is_crilayla(b"CRILAYLA\x00\x00\x00\x00\x00\x00\x00\x00"));
        assert!(!is_crilayla(b"NOPE"));
    }

    #[test]
    fn parses_header_sizes() {
        let mut buf = Vec::new();
        buf.extend_from_slice(MAGIC);
        buf.extend_from_slice(&1024u32.to_le_bytes());
        buf.extend_from_slice(&512u32.to_le_bytes());
        let header = read_header(&buf).unwrap();
        assert_eq!(header.uncompressed_size, 1024);
        assert_eq!(header.compressed_size, 512);
    }

    #[test]
    fn rejects_truncated_header() {
        let err = read_header(b"CRILAYL").unwrap_err();
        assert!(matches!(err, CrilaylaError::Truncated { .. }));
    }

    #[test]
    fn decompress_returns_not_implemented() {
        let err = decompress(&[]).unwrap_err();
        assert!(matches!(err, CrilaylaError::NotImplemented));
    }

    #[test]
    fn compress_returns_not_implemented() {
        let err = compress(&[]).unwrap_err();
        assert!(matches!(err, CrilaylaError::NotImplemented));
    }
}
