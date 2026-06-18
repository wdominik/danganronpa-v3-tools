//! Danganronpa V3 STX string-table reader/writer.
//!
//! STX is the primary translation target: a flat mapping from numeric IDs
//! to UTF-16 LE strings, optionally split across multiple sub-tables. The
//! on-disk layout is:
//!
//! ```text
//! offset 0x00  4 bytes   magic "STXT"
//! offset 0x04  4 bytes   secondary magic "JPLL" (language tag)
//! offset 0x08  4 bytes   table_count u32 LE
//! offset 0x0C  4 bytes   table_offset u32 LE — start of the per-table index arrays
//! offset 0x10  …         table-info: one 16-byte record per table (unknown u32,
//!                        string_count u32, 8 bytes padding)
//! offset table_offset
//!              …         index array: per table, `string_count` × (id u32, string_offset u32)
//! …            …         string data: UTF-16 LE, null-terminated, deduplicated
//! ```
//!
//! ## Round-trip
//!
//! [`Stx::to_bytes`] is byte-exact with original game files **provided
//! strings are deduplicated on write**: two entries with identical `text`
//! share a single offset slot in the string data. Entries are laid out in
//! the order they appear in [`StxTable::entries`].
//!
//! ## Example
//!
//! ```no_run
//! use drv3_stx::Stx;
//! let bytes = std::fs::read("dialogue.stx").unwrap();
//! let mut stx = Stx::parse(&bytes).unwrap();
//! for entry in &stx.tables[0].entries {
//!     println!("{}: {}", entry.id, entry.text);
//! }
//! // Edit one line by its id, then write the file back.
//! if let Some(entry) = stx.tables[0].entry_mut(1) {
//!     entry.text = "Neuer Text".to_string();
//! }
//! std::fs::write("dialogue.stx", stx.to_bytes()).unwrap();
//! ```

use std::collections::HashMap;

use drv3_binio::{BinResult, Reader, Writer, utf16le_byte_len};

const MAGIC_STXT: &[u8; 4] = b"STXT";
const MAGIC_JPLL: &[u8; 4] = b"JPLL";

/// Parsed STX file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stx {
    pub tables: Vec<StxTable>,
}

/// One table within an STX file.
///
/// `unknown` is a 4-byte field whose semantic meaning is not yet pinned
/// down by reverse-engineering; the parser captures it verbatim and the
/// writer emits it back unchanged so round-trip stays byte-equal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StxTable {
    pub unknown: u32,
    pub entries: Vec<StxEntry>,
}

/// A single (id, text) entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StxEntry {
    pub id: u32,
    pub text: String,
}

impl StxTable {
    /// Find an entry by its [`StxEntry::id`], or `None` if absent.
    pub fn entry(&self, id: u32) -> Option<&StxEntry> {
        self.entries.iter().find(|e| e.id == id)
    }

    /// Mutable counterpart to [`StxTable::entry`] — for editing the text in place.
    pub fn entry_mut(&mut self, id: u32) -> Option<&mut StxEntry> {
        self.entries.iter_mut().find(|e| e.id == id)
    }
}

impl Stx {
    /// Parse an STX file from a byte buffer.
    ///
    /// # Errors
    ///
    /// Returns an error if either magic word is wrong (must be `STXT`
    /// then `JPLL`), the header counts overflow the buffer, or any
    /// string-data offset points past the end of the buffer / yields
    /// invalid UTF-16.
    pub fn parse(input: &[u8]) -> BinResult<Self> {
        let mut r = Reader::new(input);

        r.expect_magic(MAGIC_STXT)?;
        r.expect_magic(MAGIC_JPLL)?;
        let table_count = r.u32_le()? as usize;
        let table_offset = r.u32_le()? as usize;

        // Table-info section, starting at offset 0x10. Record each table's
        // declared string count explicitly — recovering it later from
        // `Vec::capacity()` would lean on an allocation hint that is only
        // guaranteed to be >= the requested value, not exactly equal.
        let mut tables: Vec<StxTable> = Vec::with_capacity(table_count);
        let mut counts: Vec<usize> = Vec::with_capacity(table_count);
        for _ in 0..table_count {
            let unknown = r.u32_le()?;
            let string_count = r.u32_le()? as usize;
            r.skip(8)?; // padding
            counts.push(string_count);
            tables.push(StxTable {
                unknown,
                entries: Vec::with_capacity(string_count),
            });
        }

        // Index array — one big concatenated array, table 0 first.
        r.seek(table_offset)?;
        for (table_idx, &string_count) in counts.iter().enumerate() {
            for _ in 0..string_count {
                let id = r.u32_le()?;
                let string_offset = r.u32_le()? as usize;
                let text = r.with_seek(string_offset, Reader::read_utf16le_cstring)?;
                tables[table_idx].entries.push(StxEntry { id, text });
            }
        }

        Ok(Self { tables })
    }

    /// Encode an STX file to a byte vector.
    ///
    /// Deduplicates string data per table: every entry whose `text` matches
    /// another entry in the same table points to the same string-data
    /// offset. This matches what the original game files do — a writer that
    /// stores duplicates separately will produce a larger, non-byte-equal
    /// output, but the game itself reads it back identically.
    ///
    /// # Panics
    ///
    /// Panics in the unreachable case where the writer's internal patch
    /// slots become inconsistent (each `# expect()` documents the
    /// invariant being asserted). User input cannot trigger these — they
    /// guard against bugs in the writer itself.
    pub fn to_bytes(&self) -> Vec<u8> {
        // Pre-size by what we can compute up front: header + table-info +
        // index-array + worst-case strings.
        let total_entries: usize = self.tables.iter().map(|t| t.entries.len()).sum();
        let strings_estimate: usize = self
            .tables
            .iter()
            .flat_map(|t| t.entries.iter())
            .map(|e| utf16le_byte_len(&e.text) + 2)
            .sum();
        let mut w = Writer::with_capacity(
            0x10 + 0x10 * self.tables.len() + 8 * total_entries + strings_estimate,
        );

        // Header.
        w.write_bytes(MAGIC_STXT);
        w.write_bytes(MAGIC_JPLL);
        w.write_u32_le(self.tables.len() as u32);
        let table_offset_patch = w.reserve_u32_le();

        // Table-info section.
        for table in &self.tables {
            w.write_u32_le(table.unknown);
            w.write_u32_le(table.entries.len() as u32);
            w.write_fill(8, 0);
        }

        // Index array — one slot per entry; will be patched as strings are
        // written. Slots are laid out in table order.
        let table_offset = w.position() as u32;
        w.patch_u32_le(table_offset_patch, table_offset)
            .expect("placeholder size matches u32");
        let mut slot_patches: Vec<(u32, drv3_binio::Patch)> = Vec::with_capacity(total_entries);
        for table in &self.tables {
            for entry in &table.entries {
                w.write_u32_le(entry.id);
                let offset_patch = w.reserve_u32_le();
                slot_patches.push((entry.id, offset_patch));
            }
        }

        // String data — deduplicated per-table.
        let mut slot_iter = slot_patches.into_iter();
        for table in &self.tables {
            let mut written: HashMap<&str, u32> = HashMap::with_capacity(table.entries.len());
            for entry in &table.entries {
                let offset = if let Some(&existing) = written.get(entry.text.as_str()) {
                    existing
                } else {
                    let pos = w.position() as u32;
                    w.write_utf16le_cstring(&entry.text);
                    written.insert(entry.text.as_str(), pos);
                    pos
                };
                let (_id, patch) = slot_iter.next().expect("one slot per entry");
                w.patch_u32_le(patch, offset)
                    .expect("placeholder size matches u32");
            }
        }
        debug_assert!(slot_iter.next().is_none(), "slot count matches entry count");

        w.into_inner()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Stx {
        Stx {
            tables: vec![StxTable {
                unknown: 0xdead_beef,
                entries: vec![
                    StxEntry {
                        id: 0,
                        text: "Hello, world!".into(),
                    },
                    StxEntry {
                        id: 1,
                        text: "Hello, world!".into(),
                    }, // dup text
                    StxEntry {
                        id: 2,
                        text: "Goodbye.".into(),
                    },
                    StxEntry {
                        id: 99,
                        text: "<CLT=cltSYSTEM>system<CLT>".into(),
                    },
                ],
            }],
        }
    }

    #[test]
    fn entry_lookup_by_id() {
        let stx = sample();
        let table = &stx.tables[0];
        assert_eq!(table.entry(2).unwrap().text, "Goodbye.");
        assert!(table.entry(12_345).is_none());
    }

    #[test]
    fn round_trip_preserves_bytes() {
        let stx = sample();
        let bytes = stx.to_bytes();
        let parsed = Stx::parse(&bytes).expect("parses back");
        assert_eq!(parsed, stx);
        // Re-encode and confirm byte-stable.
        assert_eq!(parsed.to_bytes(), bytes);
    }

    #[test]
    fn dedup_shares_offset_for_identical_text() {
        let stx = sample();
        let bytes = stx.to_bytes();

        // Walk the index array directly and confirm entry 0 and entry 1
        // point to the same offset.
        let mut r = Reader::new(&bytes);
        r.skip(0x0c).unwrap();
        let table_offset = r.u32_le().unwrap() as usize;
        r.seek(table_offset).unwrap();
        let (_id0, off0) = (r.u32_le().unwrap(), r.u32_le().unwrap());
        let (_id1, off1) = (r.u32_le().unwrap(), r.u32_le().unwrap());
        assert_eq!(off0, off1, "deduplicated entries must share offset");
    }

    #[test]
    fn header_magic_and_counts_correct() {
        let stx = sample();
        let bytes = stx.to_bytes();
        assert_eq!(&bytes[..4], b"STXT");
        assert_eq!(&bytes[4..8], b"JPLL");
        assert_eq!(&bytes[8..12], &1u32.to_le_bytes()); // table count
    }

    #[test]
    fn rejects_bad_magic() {
        let mut bytes = sample().to_bytes();
        bytes[0] = b'X';
        let err = Stx::parse(&bytes).unwrap_err();
        assert!(matches!(err, drv3_binio::BinError::BadMagic { .. }));
    }

    #[test]
    fn handles_empty_table() {
        let stx = Stx {
            tables: vec![StxTable {
                unknown: 0,
                entries: vec![],
            }],
        };
        let bytes = stx.to_bytes();
        let parsed = Stx::parse(&bytes).unwrap();
        assert_eq!(parsed, stx);
    }

    #[test]
    fn unicode_strings_with_surrogate_pairs() {
        let stx = Stx {
            tables: vec![StxTable {
                unknown: 0,
                entries: vec![
                    StxEntry {
                        id: 0,
                        text: "日本語テスト".into(),
                    },
                    StxEntry {
                        id: 1,
                        text: "emoji: 😀🎮".into(),
                    },
                ],
            }],
        };
        let bytes = stx.to_bytes();
        let parsed = Stx::parse(&bytes).unwrap();
        assert_eq!(parsed, stx);
    }
}
