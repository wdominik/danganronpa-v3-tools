//! Danganronpa V3 SPC archive reader/writer.
//!
//! SPC (`CPS.`) is a Spike Chunsoft inner archive that bundles together a
//! few related game-data files — typically the STX / DAT / WRD / SRD
//! quartet for one scene. The on-disk layout is:
//!
//! ```text
//! offset 0x00  4 bytes   magic "CPS."
//! offset 0x04  0x24 bytes unknown1 — preserved verbatim
//! offset 0x28  4 bytes   entry_count u32
//! offset 0x2C  4 bytes   unknown2 u32 — preserved verbatim
//! offset 0x30  0x10 bytes padding (zeros)
//! offset 0x40  …         "Root" marker, then each entry
//! ```
//!
//! Each entry has a small header (compression flag, current/original size,
//! name length), then the name (zero-padded to 16 bytes), then the data
//! (zero-padded to 16 bytes). Subfiles are either *stored* uncompressed or
//! compressed with the SPC-LZSS codec from [`drv3_compression::spc_lzss`].
//!
//! ## Round-trip discipline
//!
//! [`Spc::parse`] decompresses every entry into its raw form;
//! [`Spc::to_bytes`] re-compresses on write. This is **semantically**
//! round-trip-safe (`parse(write(x)) == x`), but the on-disk bytes of
//! compressed entries are not guaranteed to match the original because many
//! valid LZSS encodings exist for the same input. Synthetic SPCs built
//! through this crate do round-trip byte-equal because compression is
//! deterministic.
//!
//! The `$CMP`-wrapped variant (used by console versions of the game) is
//! out of scope for v0.1 — never observed in DR V3's PC archives.
//! [`Spc::parse`] returns [`SpcParseError::CmpVariant`] if it encounters one.

use drv3_binio::{BinError, BinResult, Reader, Writer};
use drv3_compression::spc_lzss;
use thiserror::Error;

const MAGIC_CPS: &[u8; 4] = b"CPS.";
const MAGIC_CMP: &[u8; 4] = b"$CMP";
const MAGIC_ROOT: &[u8; 4] = b"Root";

/// `compression_flag` value for entries stored verbatim (no compression).
pub const COMPRESSION_STORED: i16 = 1;
/// `compression_flag` value for entries compressed with the SPC-LZSS codec
/// (see [`drv3_compression::spc_lzss`]).
pub const COMPRESSION_LZSS: i16 = 2;

/// Errors produced by the SPC reader.
#[derive(Debug, Error)]
pub enum SpcParseError {
    #[error(transparent)]
    Bin(#[from] BinError),

    #[error(
        "$CMP-wrapped SPC variant is not supported in v0.1 (only the plain `CPS.` form, \
         which is what DR V3 ships)"
    )]
    CmpVariant,

    #[error("unknown compression flag {0} (expected 1 or 2)")]
    UnknownCompressionFlag(i16),

    #[error("LZSS decompression failed: {0}")]
    Lzss(#[from] spc_lzss::SpcError),
}

pub type SpcResult<T> = Result<T, SpcParseError>;

/// Parsed SPC archive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spc {
    /// 0x24 opaque bytes at header offset 0x04 — preserve verbatim.
    pub unknown1: [u8; 0x24],
    /// u32 at header offset 0x2C — preserve verbatim.
    pub unknown2: u32,
    pub entries: Vec<SpcEntry>,
}

/// A single SPC subfile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpcEntry {
    /// Subfile name as raw bytes (Shift-JIS on disk; in practice ASCII).
    pub name: Vec<u8>,
    /// `1` (stored) or `2` (LZSS-compressed). Preserved on round-trip so a
    /// stored→stored or compressed→compressed file's flag is unchanged.
    pub compression_flag: i16,
    /// 2 opaque bytes at entry offset 0x02 — preserve verbatim.
    pub unknown_flag: i16,
    /// Decompressed contents.
    pub data: Vec<u8>,
}

impl SpcEntry {
    /// Convenience: decode `name` as UTF-8 (which matches ASCII).
    pub fn name_as_str(&self) -> Option<&str> {
        std::str::from_utf8(&self.name).ok()
    }
}

/// Read a signed 32-bit size/length field, rejecting a negative value.
///
/// The on-disk fields are `i32`; a negative value is malformed and, if cast
/// straight to `usize`, would become a near-`usize::MAX` allocation request.
fn read_size(r: &mut Reader<'_>, field: &str) -> BinResult<usize> {
    let pos = r.position();
    let raw = r.i32_le()?;
    usize::try_from(raw).map_err(|_| BinError::malformed(pos, format!("negative {field}: {raw}")))
}

impl Spc {
    /// Find a subfile by name (UTF-8 / ASCII), or `None` if absent.
    pub fn entry(&self, name: &str) -> Option<&SpcEntry> {
        self.entries.iter().find(|e| e.name_as_str() == Some(name))
    }

    /// Mutable counterpart to [`Spc::entry`] — for editing a member's bytes in place.
    pub fn entry_mut(&mut self, name: &str) -> Option<&mut SpcEntry> {
        self.entries
            .iter_mut()
            .find(|e| e.name_as_str() == Some(name))
    }

    /// Parse an SPC archive from a byte buffer.
    ///
    /// # Errors
    ///
    /// Returns an error if the magic is not `CPS.` (specifically, returns
    /// [`SpcParseError::CmpVariant`] for the unsupported `$CMP` form),
    /// the `Root` marker is missing, an entry's compression flag is
    /// neither stored nor LZSS, or LZSS decompression of any entry fails.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use drv3_spc::Spc;
    ///
    /// let bytes = std::fs::read("chap0_text_US.SPC")?;
    /// let spc = Spc::parse(&bytes)?;
    ///
    /// for entry in &spc.entries {
    ///     println!("{} ({} bytes)", entry.name_as_str().unwrap_or("?"), entry.data.len());
    /// }
    ///
    /// // A member's `data` is raw bytes — hand an `.stx` member to `drv3_stx::Stx::parse`.
    /// if let Some(member) = spc.entry("c00_001_018.stx") {
    ///     let _stx_bytes = &member.data;
    /// }
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn parse(input: &[u8]) -> SpcResult<Self> {
        let mut r = Reader::new(input);
        let magic: [u8; 4] = r.array()?;
        if &magic == MAGIC_CMP {
            return Err(SpcParseError::CmpVariant);
        }
        if &magic != MAGIC_CPS {
            return Err(BinError::BadMagic {
                pos: 0,
                expected: MAGIC_CPS.to_vec(),
                got: magic.to_vec(),
            }
            .into());
        }

        let unknown1: [u8; 0x24] = r.array()?;
        let file_count = r.u32_le()?;
        let unknown2 = r.u32_le()?;
        r.skip(0x10)?;

        r.expect_magic(MAGIC_ROOT)?;
        r.skip(0x0C)?;

        // Each entry header is at least 32 bytes on disk, so cap the hint; a
        // hostile `file_count` then fails on a bounded read, not a huge alloc.
        let mut entries: Vec<SpcEntry> =
            Vec::with_capacity(r.capacity_hint(file_count as usize, 32));
        for _ in 0..file_count {
            let compression_flag = r.i16_le()?;
            let unknown_flag = r.i16_le()?;
            // Sizes are stored signed; a negative value is malformed and, cast
            // straight to `usize`, would become a huge allocation request.
            let current_size = read_size(&mut r, "current_size")?;
            let original_size = read_size(&mut r, "original_size")?;
            let name_length = read_size(&mut r, "name_length")?;
            r.skip(0x10)?;

            let name = r.bytes(name_length)?.to_vec();
            let null = r.u8()?;
            if null != 0 {
                return Err(BinError::malformed(
                    r.position() - 1,
                    "missing null terminator after subfile name",
                )
                .into());
            }
            // Name padding pads `(NameLength + 1)` to a 16-byte boundary.
            let name_padding = drv3_binio::align_up(name_length + 1, 0x10) - (name_length + 1);
            r.skip(name_padding)?;

            let raw = r.bytes(current_size)?;
            let data = match compression_flag {
                COMPRESSION_STORED => {
                    if current_size != original_size {
                        return Err(BinError::malformed(
                            r.position(),
                            format!(
                                "stored entry size mismatch: current={current_size} original={original_size}"
                            ),
                        )
                        .into());
                    }
                    raw.to_vec()
                }
                COMPRESSION_LZSS => spc_lzss::decompress(raw, original_size)?,
                other => return Err(SpcParseError::UnknownCompressionFlag(other)),
            };
            // Data padding pads `current_size` to a 16-byte boundary.
            let data_padding = drv3_binio::align_up(current_size, 0x10) - current_size;
            r.skip(data_padding)?;

            entries.push(SpcEntry {
                name,
                compression_flag,
                unknown_flag,
                data,
            });
        }

        Ok(Self {
            unknown1,
            unknown2,
            entries,
        })
    }

    /// Encode an SPC archive to a byte vector.
    ///
    /// # Errors
    ///
    /// Returns an error if an entry declares the `LZSS` compression flag
    /// but its body fails to compress (currently can't happen — the
    /// compressor is infallible — but the signature reserves the option
    /// for future fallible codecs).
    pub fn to_bytes(&self) -> SpcResult<Vec<u8>> {
        let mut w = Writer::new();
        w.write_bytes(MAGIC_CPS);
        w.write_bytes(&self.unknown1);
        w.write_u32_le(self.entries.len() as u32);
        w.write_u32_le(self.unknown2);
        w.write_fill(0x10, 0);
        w.write_bytes(MAGIC_ROOT);
        w.write_fill(0x0C, 0);

        for entry in &self.entries {
            let encoded = match entry.compression_flag {
                COMPRESSION_STORED => entry.data.clone(),
                COMPRESSION_LZSS => spc_lzss::compress(&entry.data),
                other => return Err(SpcParseError::UnknownCompressionFlag(other)),
            };
            let current_size = i32::try_from(encoded.len())
                .map_err(|_| BinError::malformed(0, "encoded size exceeds i32"))?;
            let original_size = i32::try_from(entry.data.len())
                .map_err(|_| BinError::malformed(0, "entry data size exceeds i32"))?;
            let name_length = i32::try_from(entry.name.len())
                .map_err(|_| BinError::malformed(0, "name length exceeds i32"))?;

            w.write_i16_le(entry.compression_flag);
            w.write_i16_le(entry.unknown_flag);
            w.write_i32_le(current_size);
            w.write_i32_le(original_size);
            w.write_i32_le(name_length);
            w.write_fill(0x10, 0);

            w.write_bytes(&entry.name);
            w.write_u8(0);
            let name_len = entry.name.len() + 1;
            let name_padding = drv3_binio::align_up(name_len, 0x10) - name_len;
            w.write_fill(name_padding, 0);

            w.write_bytes(&encoded);
            let data_padding = drv3_binio::align_up(encoded.len(), 0x10) - encoded.len();
            w.write_fill(data_padding, 0);
        }

        Ok(w.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Spc {
        Spc {
            unknown1: [0xAA; 0x24],
            unknown2: 0xCAFE_BABE,
            entries: vec![
                SpcEntry {
                    name: b"chapter1.stx".to_vec(),
                    compression_flag: COMPRESSION_STORED,
                    unknown_flag: 0,
                    data: b"<STX raw bytes ...>".to_vec(),
                },
                SpcEntry {
                    name: b"chapter1.wrd".to_vec(),
                    compression_flag: COMPRESSION_LZSS,
                    unknown_flag: 1,
                    data: b"AAAAAAAAAAAAAAAAAAAA".repeat(20),
                },
                SpcEntry {
                    name: b"chapter1.dat".to_vec(),
                    compression_flag: COMPRESSION_LZSS,
                    unknown_flag: 0,
                    data: vec![],
                },
            ],
        }
    }

    #[test]
    fn entry_lookup_by_name() {
        let spc = sample();
        assert_eq!(
            spc.entry("chapter1.wrd").unwrap().compression_flag,
            COMPRESSION_LZSS
        );
        assert!(spc.entry("missing.stx").is_none());
    }

    #[test]
    fn round_trip_preserves_contents() {
        let spc = sample();
        let bytes = spc.to_bytes().unwrap();
        let parsed = Spc::parse(&bytes).unwrap();
        assert_eq!(parsed, spc);
    }

    #[test]
    fn round_trip_byte_equal_for_synthetic_archives() {
        // Because our compression is deterministic, a synthetic SPC round-trips byte-for-byte.
        let bytes1 = sample().to_bytes().unwrap();
        let parsed = Spc::parse(&bytes1).unwrap();
        let bytes2 = parsed.to_bytes().unwrap();
        assert_eq!(bytes1, bytes2);
    }

    #[test]
    fn header_layout_correct() {
        let bytes = sample().to_bytes().unwrap();
        assert_eq!(&bytes[..4], MAGIC_CPS);
        assert_eq!(&bytes[0x28..0x2C], &3u32.to_le_bytes());
        assert_eq!(&bytes[0x40..0x44], MAGIC_ROOT);
    }

    #[test]
    fn cmp_variant_is_rejected() {
        let bytes = b"$CMP\x00\x00\x00\x00".to_vec();
        let err = Spc::parse(&bytes).unwrap_err();
        assert!(matches!(err, SpcParseError::CmpVariant));
    }

    #[test]
    fn unknown_compression_flag_errors() {
        let mut spc = sample();
        spc.entries[0].compression_flag = 99;
        let err = spc.to_bytes().unwrap_err();
        assert!(matches!(err, SpcParseError::UnknownCompressionFlag(99)));
    }

    #[test]
    fn negative_entry_size_errors_without_panic() {
        let mut bytes = sample().to_bytes().unwrap();
        // The first entry header begins at 0x50; `original_size` is the i32 LE
        // at +8 (0x58). A negative value cast to `usize` would previously
        // become a near-`usize::MAX` allocation request.
        bytes[0x58..0x5C].copy_from_slice(&(-1i32).to_le_bytes());
        let err = Spc::parse(&bytes).unwrap_err();
        assert!(matches!(
            err,
            SpcParseError::Bin(BinError::Malformed { .. })
        ));
    }

    #[test]
    fn name_padding_aligns_to_16_bytes() {
        let bytes = sample().to_bytes().unwrap();
        // Entry table starts at 0x50 in the file. Each entry header is 0x20 bytes,
        // then name + null + padding takes us to a 16-byte boundary before data.
        // We verify the global file size is a multiple of 16.
        assert_eq!(bytes.len() % 0x10, 0, "file must end on 0x10 boundary");
    }
}
