//! Danganronpa V3 `SpFt` font-block reader/writer.
//!
//! `SpFt` is a font-atlas metadata blob embedded inside an SRD `$RSI`
//! block's `ResourceData` section. It catalogues every glyph the font
//! defines:
//!
//! - **Bit-flag table**: one bit per codepoint indicating whether the font
//!   has a glyph for it. Indexed linearly, so codepoint *N* lives at bit
//!   `N % 8` of byte `N / 8`.
//! - **Sparse index table**: for every 32-codepoint window, the bbox-table
//!   index of the first present codepoint; subsequent set bits in the same
//!   window run sequentially from that base.
//! - **Bounding-box table**: per glyph, the atlas position (12-bit x and y),
//!   the cell size (8-bit width and height), and three signed kerning
//!   deltas (left, right, vertical).
//! - **Font name**: a UTF-16 LE null-terminated string.
//!
//! The atlas *pixel* data is not part of `SpFt`; it lives in the parent
//! `$RSI`'s `ExternalData` and is referenced by the same indices.

use drv3_binio::{BinError, BinResult, Reader, Writer};

const MAGIC_SPFT: &[u8; 4] = b"SpFt";
const HEADER_SIZE: u32 = 0x2C;
/// The bit-flag table is iterated up to (but not including) U+D800 — the
/// start of the UTF-16 surrogate range, where individual code units no
/// longer represent codepoints. Bits past this point in the table are
/// preserved verbatim on round-trip but never produce glyphs.
const READ_CODEPOINT_CAP: u32 = 0xD800;

/// Parsed `SpFt` font block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpFt {
    /// 4 bytes at header offset 0x04 — observed value is always `6`; preserved verbatim.
    pub unknown6: u32,
    /// Total number of codepoints addressed by the bit-flag table. DR V3 uses `0xFF5F` (65375).
    pub bit_flag_count: u32,
    /// 4 bytes at header offset 0x24 — preserved verbatim.
    pub scale_flag: u32,
    /// Font name, UTF-16 LE null-terminated in the file.
    pub font_name: String,
    /// Glyphs in codepoint order. Round-trip-safe writes require this ordering.
    pub glyphs: Vec<Glyph>,
}

/// A single atlas glyph.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Glyph {
    pub codepoint: u32,
    /// Atlas pixel position, top-left of bounding box. Each component is 12-bit (0..=4095).
    pub position: (u16, u16),
    /// Bounding-box width and height in pixels.
    pub size: (u8, u8),
    /// Kerning deltas `(left, right, vertical)` in pixels (signed).
    pub kerning: (i8, i8, i8),
}

impl SpFt {
    /// Parse a `SpFt` font block from a byte buffer.
    ///
    /// # Errors
    ///
    /// Returns an error if the magic is not `SpFt`, the bit-flag-table
    /// pointer is not at the expected fixed offset, a section offset is
    /// out of range, or the sparse-index lookup yields an out-of-bounds
    /// bbox-table index.
    pub fn parse(input: &[u8]) -> BinResult<Self> {
        let mut r = Reader::new(input);

        r.expect_magic(MAGIC_SPFT)?;
        let unknown6 = r.u32_le()?;
        let bit_flag_count = r.u32_le()?;
        let _font_name_length = r.u32_le()?; // recomputed on write
        let font_name_ptr = r.u32_le()? as usize;
        let glyph_count = r.u32_le()? as usize;
        let bbox_table_ptr = r.u32_le()? as usize;
        let bit_flags_ptr = r.u32_le()? as usize;
        let index_table_ptr = r.u32_le()? as usize;
        let scale_flag = r.u32_le()?;
        let _font_name_ptrs_ptr = r.u32_le()?;

        if bit_flags_ptr != HEADER_SIZE as usize {
            return Err(BinError::malformed(
                0x1C,
                format!("unexpected BitFlagsPtr {bit_flags_ptr:#x} (expected {HEADER_SIZE:#x})"),
            ));
        }

        // Walk the bit-flag table and collect every codepoint with its bit
        // set. Stop at U+D800 — the start of the UTF-16 surrogate range —
        // because higher bits in the table don't represent valid codepoints.
        let bit_flags_byte_count = bit_flag_count.div_ceil(8) as usize;
        r.seek(bit_flags_ptr)?;
        let bit_flags = r.bytes(bit_flags_byte_count)?.to_vec();
        let mut charset: Vec<u32> = Vec::with_capacity(glyph_count);
        'outer: for (byte_index, &b) in bit_flags.iter().enumerate() {
            if b == 0 {
                continue;
            }
            for bit in 0..8u32 {
                let codepoint = (byte_index as u32) * 8 + bit;
                if codepoint >= READ_CODEPOINT_CAP {
                    break 'outer;
                }
                if (b >> bit) & 1 == 1 {
                    charset.push(codepoint);
                }
            }
        }

        // Build (codepoint → bbox-table index) using the sparse index table.
        // For each 32-codepoint window, the index-table entry is the bbox
        // index of the *first* present codepoint; subsequent codepoints in
        // the same window run sequentially.
        let mut glyph_index_for_cp: Vec<(u32, usize)> = Vec::with_capacity(charset.len());
        let mut window_seen: std::collections::HashMap<usize, u32> =
            std::collections::HashMap::new();
        for &cp in &charset {
            // Each index-table entry covers a 32-codepoint window (4 bytes
            // of bit-flag table = 32 bits = 32 codepoints). `cp / 8` gives
            // the byte index of `cp`'s bit; `& !0b11` rounds down to the
            // 4-byte (32-codepoint) window start.
            let char_offset = ((cp / 8) & !0b11) as usize;
            r.seek(index_table_ptr + char_offset)?;
            let base = r.u32_le()?;
            let seen = window_seen.entry(char_offset).or_insert(0);
            let idx = (base + *seen) as usize;
            *seen += 1;
            glyph_index_for_cp.push((cp, idx));
        }

        // BBox table — read all glyph_count entries, then assemble Glyphs.
        r.seek(bbox_table_ptr)?;
        let mut bbox: Vec<Glyph> = Vec::with_capacity(glyph_count);
        for _ in 0..glyph_count {
            let pa = r.u8()?;
            let pb = r.u8()?;
            let pc = r.u8()?;
            let position = abc_to_xy(pa, pb, pc);
            let width = r.u8()?;
            let height = r.u8()?;
            let kern_left = r.i8()?;
            let kern_right = r.i8()?;
            let kern_vertical = r.i8()?;
            bbox.push(Glyph {
                codepoint: 0,
                position,
                size: (width, height),
                kerning: (kern_left, kern_right, kern_vertical),
            });
        }

        let glyphs: Vec<Glyph> = glyph_index_for_cp
            .into_iter()
            .map(|(codepoint, idx)| Glyph {
                codepoint,
                ..bbox[idx]
            })
            .collect();

        // Font name.
        r.seek(font_name_ptr)?;
        let font_name = r.read_utf16le_cstring()?;

        Ok(Self {
            unknown6,
            bit_flag_count,
            scale_flag,
            font_name,
            glyphs,
        })
    }

    /// Encode a `SpFt` font block to a byte vector.
    ///
    /// Glyphs are written in ascending codepoint order. The function reorders
    /// `self.glyphs` only locally; the original struct is not mutated.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut glyphs = self.glyphs.clone();
        glyphs.sort_by_key(|g| g.codepoint);

        // Layout planning.
        //
        // The bit-flag table is iterated up to `bit_flag_count` by readers,
        // so it must extend past the highest glyph codepoint — otherwise a
        // glyph added beyond the shipped count would have nowhere to set
        // its bit (panicking the write) and would be invisible to the game
        // (its bit lies past where the reader scans). Grow the effective
        // count to cover the highest codepoint; existing fonts keep their
        // shipped count (German codepoints sit below the DR V3 `0xFF5F`).
        let effective_bit_flag_count = match glyphs.last() {
            Some(highest) => self.bit_flag_count.max(highest.codepoint + 1),
            None => self.bit_flag_count,
        };
        let bit_flags_byte_count = effective_bit_flag_count.div_ceil(8) as usize;

        let max_char_offset = glyphs
            .iter()
            .map(|g| ((g.codepoint / 8) & !0b11) as usize)
            .max()
            .unwrap_or(0);
        let index_table_size = max_char_offset + 4;

        let bit_flags_ptr = HEADER_SIZE as usize;
        let index_table_ptr = bit_flags_ptr + bit_flags_byte_count;
        let bbox_table_ptr = index_table_ptr + index_table_size;
        let bbox_table_size = glyphs.len() * 8;
        let font_name_ptrs_ptr = bbox_table_ptr + bbox_table_size;
        let font_name_ptr = font_name_ptrs_ptr + 0x10;

        let font_name_byte_len = drv3_binio::utf16le_byte_len(&self.font_name) + 2; // + terminator
        let total_size = font_name_ptr + font_name_byte_len;

        let mut w = Writer::with_capacity(total_size);

        // Header (44 bytes, all u32 LE after the magic).
        w.write_bytes(MAGIC_SPFT);
        w.write_u32_le(self.unknown6);
        w.write_u32_le(effective_bit_flag_count);
        w.write_u32_le(self.font_name.encode_utf16().count() as u32);
        w.write_u32_le(font_name_ptr as u32);
        w.write_u32_le(glyphs.len() as u32);
        w.write_u32_le(bbox_table_ptr as u32);
        w.write_u32_le(bit_flags_ptr as u32);
        w.write_u32_le(index_table_ptr as u32);
        w.write_u32_le(self.scale_flag);
        w.write_u32_le(font_name_ptrs_ptr as u32);
        debug_assert_eq!(w.position(), HEADER_SIZE as usize);

        // Bit-flag table — build in a buffer, then write.
        let mut bit_flags = vec![0u8; bit_flags_byte_count];
        for glyph in &glyphs {
            let byte_index = (glyph.codepoint >> 3) as usize;
            let bit = glyph.codepoint & 0b111;
            bit_flags[byte_index] |= 1 << bit;
        }
        w.write_bytes(&bit_flags);
        debug_assert_eq!(w.position(), index_table_ptr);

        // Index table — zero-fill, then patch in one entry per glyph window.
        let index_start = w.position();
        w.write_fill(index_table_size, 0);
        let mut written_windows: std::collections::HashSet<usize> =
            std::collections::HashSet::new();
        let mut buf = w.into_inner();
        for (idx, glyph) in glyphs.iter().enumerate() {
            let char_offset = ((glyph.codepoint / 8) & !0b11) as usize;
            if written_windows.insert(char_offset) {
                let target = index_start + char_offset;
                buf[target..target + 4].copy_from_slice(&(idx as u32).to_le_bytes());
            }
        }
        let mut w = Writer::with_capacity(total_size);
        w.write_bytes(&buf);
        drop(buf);
        debug_assert_eq!(w.position(), bbox_table_ptr);

        // BBox table.
        for glyph in &glyphs {
            let (pa, pb, pc) = xy_to_abc(glyph.position.0, glyph.position.1);
            w.write_u8(pa);
            w.write_u8(pb);
            w.write_u8(pc);
            w.write_u8(glyph.size.0);
            w.write_u8(glyph.size.1);
            w.write_i8(glyph.kerning.0);
            w.write_i8(glyph.kerning.1);
            w.write_i8(glyph.kerning.2);
        }
        debug_assert_eq!(w.position(), font_name_ptrs_ptr);

        // Font-name pointers (4 × u32 LE, all equal to font_name_ptr).
        for _ in 0..4 {
            w.write_u32_le(font_name_ptr as u32);
        }
        debug_assert_eq!(w.position(), font_name_ptr);

        // Font name.
        w.write_utf16le_cstring(&self.font_name);

        w.into_inner()
    }
}

/// Pack two 12-bit unsigned coordinates `(x, y)` into three bytes.
///
/// The on-disk layout interleaves x and y so their high nibbles share the
/// middle byte:
///
/// ```text
/// byte a (low 8 bits): x[0..8]
/// byte b (high nibble = y[0..4], low nibble = x[8..12])
/// byte c (low 8 bits): y[4..12]
/// ```
///
/// Together a/b/c encode 12 + 12 = 24 bits in 3 bytes with no wasted bits.
#[must_use]
#[allow(clippy::many_single_char_names)]
pub fn xy_to_abc(x: u16, y: u16) -> (u8, u8, u8) {
    let a = (x & 0xFF) as u8;
    let b = (((y & 0xF) << 4) | ((x >> 8) & 0xF)) as u8;
    let c = ((y >> 4) & 0xFF) as u8;
    (a, b, c)
}

/// Unpack three bytes into two 12-bit unsigned coordinates. Inverse of
/// [`xy_to_abc`]; see that function for the bit layout.
#[must_use]
#[allow(clippy::many_single_char_names)]
pub fn abc_to_xy(a: u8, b: u8, c: u8) -> (u16, u16) {
    let x = u16::from(a) | ((u16::from(b) & 0xF) << 8);
    let y = ((u16::from(b) >> 4) & 0xF) | (u16::from(c) << 4);
    (x, y)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xy_packing_round_trip() {
        for &(x, y) in &[
            (0u16, 0u16),
            (1, 2),
            (4095, 4095),
            (1234, 567),
            (0xABC, 0x123),
        ] {
            let (a, b, c) = xy_to_abc(x, y);
            assert_eq!(abc_to_xy(a, b, c), (x, y), "round trip ({x},{y})");
        }
    }

    fn sample() -> SpFt {
        SpFt {
            unknown6: 6,
            bit_flag_count: 0x100, // small for tests (256 bits, 32 bytes)
            scale_flag: 0x4011_4011,
            font_name: "TestFont-Regular".to_string(),
            glyphs: vec![
                Glyph {
                    codepoint: 0x41,
                    position: (10, 20),
                    size: (8, 12),
                    kerning: (-1, 0, 1),
                },
                Glyph {
                    codepoint: 0x42,
                    position: (18, 20),
                    size: (8, 12),
                    kerning: (0, 0, 1),
                },
                // Same window as 0x42 (window contains codepoints 0x40..0x60).
                Glyph {
                    codepoint: 0x43,
                    position: (26, 20),
                    size: (8, 12),
                    kerning: (0, -1, 1),
                },
                // Different window (codepoints 0x60..0x80).
                Glyph {
                    codepoint: 0x61,
                    position: (10, 32),
                    size: (8, 12),
                    kerning: (0, 0, 0),
                },
            ],
        }
    }

    #[test]
    fn round_trip_preserves_bytes() {
        let spft = sample();
        let bytes = spft.to_bytes();
        let parsed = SpFt::parse(&bytes).expect("parses back");
        assert_eq!(parsed, spft);
        assert_eq!(parsed.to_bytes(), bytes, "second write must equal first");
    }

    #[test]
    fn header_contains_correct_pointers() {
        let bytes = sample().to_bytes();
        // BitFlagsPtr at 0x1C must be 0x2C.
        let bit_flags_ptr = u32::from_le_bytes(bytes[0x1C..0x20].try_into().unwrap());
        assert_eq!(bit_flags_ptr, 0x2C);
    }

    #[test]
    fn glyphs_are_sorted_on_write() {
        let mut spft = sample();
        spft.glyphs.reverse(); // hand-scrambled
        let bytes = spft.to_bytes();
        let parsed = SpFt::parse(&bytes).unwrap();
        let codes: Vec<_> = parsed.glyphs.iter().map(|g| g.codepoint).collect();
        assert_eq!(codes, vec![0x41, 0x42, 0x43, 0x61]);
    }

    #[test]
    fn rejects_bad_magic() {
        let mut bytes = sample().to_bytes();
        bytes[0] = b'X';
        let err = SpFt::parse(&bytes).unwrap_err();
        assert!(matches!(err, BinError::BadMagic { .. }));
    }

    #[test]
    fn supports_dr_v3_full_bit_flag_count() {
        let spft = SpFt {
            unknown6: 6,
            bit_flag_count: 0xFF5F,
            scale_flag: 0,
            font_name: "F".into(),
            glyphs: vec![
                Glyph {
                    codepoint: 0x21,
                    position: (0, 0),
                    size: (4, 8),
                    kerning: (0, 0, 0),
                },
                Glyph {
                    codepoint: 0x7F,
                    position: (4, 0),
                    size: (4, 8),
                    kerning: (0, 0, 0),
                },
            ],
        };
        let bytes = spft.to_bytes();
        let parsed = SpFt::parse(&bytes).unwrap();
        assert_eq!(parsed, spft);
    }

    #[test]
    fn writing_glyph_beyond_bit_flag_count_grows_table() {
        // A font that ships with a small bit-flag table (128 bits = 16
        // bytes) gains a high-codepoint glyph (ü, U+00FC → byte index 31).
        // The writer must grow the table and the recorded count rather
        // than index out of bounds, and the glyph must round-trip.
        let spft = SpFt {
            unknown6: 6,
            bit_flag_count: 128,
            scale_flag: 0,
            font_name: "F".into(),
            glyphs: vec![
                Glyph {
                    codepoint: 0x41,
                    position: (0, 0),
                    size: (4, 8),
                    kerning: (0, 0, 0),
                },
                Glyph {
                    codepoint: 0xFC,
                    position: (8, 0),
                    size: (4, 8),
                    kerning: (0, 0, 0),
                },
            ],
        };
        let bytes = spft.to_bytes();
        let parsed = SpFt::parse(&bytes).unwrap();
        // Both glyphs survive; the table grew to cover U+00FC.
        assert!(parsed.glyphs.iter().any(|g| g.codepoint == 0x41));
        assert!(parsed.glyphs.iter().any(|g| g.codepoint == 0xFC));
        assert!(parsed.bit_flag_count >= 0xFD);
        // Idempotent on a second write.
        assert_eq!(parsed.to_bytes(), bytes);
    }

    #[test]
    fn small_bit_flag_count_preserved_when_no_high_glyphs() {
        // Growth only kicks in for glyphs past the shipped count — a font
        // with only low codepoints keeps its original bit_flag_count.
        let spft = SpFt {
            unknown6: 6,
            bit_flag_count: 128,
            scale_flag: 0,
            font_name: "F".into(),
            glyphs: vec![Glyph {
                codepoint: 0x41,
                position: (0, 0),
                size: (4, 8),
                kerning: (0, 0, 0),
            }],
        };
        let parsed = SpFt::parse(&spft.to_bytes()).unwrap();
        assert_eq!(parsed.bit_flag_count, 128);
    }
}
