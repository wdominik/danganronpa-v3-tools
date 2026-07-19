//! CPK archive reader/writer built on top of [`crate::utf::UtfTable`].

use std::borrow::Cow;

use drv3_binio::{BinError, BinResult, Reader};
use drv3_compression::crilayla;
use indexmap::IndexMap;
use thiserror::Error;

use crate::utf::{StorageFlag, UtfColumn, UtfRow, UtfTable, UtfType, UtfValue};

const MAGIC_CPK: &[u8; 4] = b"CPK ";
const MAGIC_TOC: &[u8; 4] = b"TOC ";

pub(crate) const PACKET_WRAPPER_SIZE: u64 = 0x10;
pub(crate) const DEFAULT_ALIGN: u16 = 0x800;

/// Errors produced by the CPK reader.
#[derive(Debug, Error)]
pub enum CpkParseError {
    #[error(transparent)]
    Bin(#[from] BinError),

    #[error("CRILAYLA-compressed CPK entry is not supported in v0.1: {0}")]
    Crilayla(#[from] crilayla::CrilaylaError),

    #[error("missing required column {0:?} in {1}")]
    MissingColumn(String, &'static str),

    #[error("TOC packet missing — DR V3 CPKs always have one")]
    MissingToc,
}

pub type CpkResult<T> = Result<T, CpkParseError>;

/// Parsed CPK archive.
///
/// File bodies are borrowed from the input buffer: [`Cpk::parse`] stores each
/// [`CpkFile::data`] as `Cow::Borrowed` into the parsed slice (zero-copy, so
/// `cpk list` / `extract` don't materialize gigabytes of bodies), while
/// mutators such as the translation engine replace individual bodies with
/// `Cow::Owned`. The lifetime `'a` is the input buffer's; a `Cpk` assembled
/// from owned data (a manifest, a test) is `Cpk<'static>`.
#[derive(Debug, Clone, PartialEq)]
pub struct Cpk<'a> {
    /// All columns from the single-row header `@UTF` table, preserved so a
    /// writer can re-emit verbatim metadata (timestamps, tool versions, etc.).
    pub header_row: UtfRow,
    pub header_columns: Vec<UtfColumn>,
    /// The TOC's `@UTF` column schema, preserved verbatim so a writer can
    /// re-emit the same column order, types, and storage flags. Without this,
    /// the writer would silently normalize the schema (e.g. force every
    /// column to `PerRow / u32`), which round-trips wrong for any CPK whose
    /// TOC carries a `Constant` column or a `u64 FileSize`.
    pub toc_columns: Vec<UtfColumn>,
    /// Files in TOC order.
    pub files: Vec<CpkFile<'a>>,
    /// Raw bytes of the ETOC packet (wrapper included), if present.
    pub etoc_packet: Option<Vec<u8>>,
    /// Raw bytes of the ITOC packet (wrapper included), if present.
    pub itoc_packet: Option<Vec<u8>>,
    /// Raw bytes of the GTOC packet (wrapper included), if present.
    pub gtoc_packet: Option<Vec<u8>>,
}

impl Cpk<'_> {
    /// Canonical DR V3 TOC schema: seven `PerRow` columns describing each
    /// file in the archive — `DirName`, `FileName`, `FileSize`,
    /// `ExtractSize`, `FileOffset`, `ID`, `UserString`. Useful when
    /// programmatically building a fresh CPK, and as the backward-
    /// compatibility fallback when reading a v1 manifest that doesn't carry
    /// the TOC schema explicitly.
    pub fn default_toc_columns() -> Vec<UtfColumn> {
        [
            ("DirName", UtfType::String),
            ("FileName", UtfType::String),
            ("FileSize", UtfType::U32),
            ("ExtractSize", UtfType::U32),
            ("FileOffset", UtfType::U64),
            ("ID", UtfType::U32),
            ("UserString", UtfType::String),
        ]
        .iter()
        .map(|&(name, ty)| UtfColumn {
            name: Some(name.to_string()),
            storage: StorageFlag::PerRow,
            ty,
            constant: None,
        })
        .collect()
    }
}

/// One file entry in the TOC.
#[derive(Debug, Clone, PartialEq)]
pub struct CpkFile<'a> {
    pub dir_name: String,
    pub file_name: String,
    pub id: u32,
    pub user_string: String,
    /// All other TOC columns preserved verbatim (per-row values keyed by column name).
    pub extra: IndexMap<String, UtfValue>,
    /// File body — `Cow::Borrowed` into the parsed buffer after [`Cpk::parse`],
    /// `Cow::Owned` when assembled from owned bytes or replaced by a mutator.
    pub data: Cow<'a, [u8]>,
}

impl<'a> Cpk<'a> {
    /// Find a file by directory and filename, or `None` if absent.
    pub fn file(&self, dir_name: &str, file_name: &str) -> Option<&CpkFile<'a>> {
        self.files
            .iter()
            .find(|f| f.dir_name == dir_name && f.file_name == file_name)
    }

    /// Mutable counterpart to [`Cpk::file`] — for editing a file's bytes in place.
    pub fn file_mut(&mut self, dir_name: &str, file_name: &str) -> Option<&mut CpkFile<'a>> {
        self.files
            .iter_mut()
            .find(|f| f.dir_name == dir_name && f.file_name == file_name)
    }

    /// Parse a CPK archive from a byte buffer.
    ///
    /// # Errors
    ///
    /// Returns an error if the header or TOC packet wrappers are malformed,
    /// the `@UTF` tables fail to parse, the header table doesn't have
    /// exactly one row, a required column (`ContentOffset`, `TocOffset`,
    /// `FileName`, `FileSize`, `FileOffset`) is missing, any file body's
    /// declared offset extends past the buffer, or any entry is
    /// CRILAYLA-compressed (not supported in v0.1).
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use drv3_cpk::Cpk;
    ///
    /// let bytes = std::fs::read("partition_data_win_us.cpk")?;
    /// let mut cpk = Cpk::parse(&bytes)?;
    ///
    /// // List every file.
    /// for file in &cpk.files {
    ///     println!("{}/{} ({} bytes)", file.dir_name, file.file_name, file.data.len());
    /// }
    ///
    /// // Find a file and replace its bytes in place (`Vec<u8>` -> `Cow` via `.into()`).
    /// if let Some(file) = cpk.file_mut("", "some.dat") {
    ///     file.data = b"new contents".to_vec().into();
    /// }
    ///
    /// std::fs::write("patched.cpk", cpk.to_bytes()?)?;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[expect(
        clippy::too_many_lines,
        reason = "the header / TOC / offset-base / per-file passes read as one linear parse"
    )]
    pub fn parse(input: &'a [u8]) -> CpkResult<Self> {
        let mut r = Reader::new(input);
        let header_table = read_packet(&mut r, MAGIC_CPK)?;
        if header_table.rows.len() != 1 {
            return Err(BinError::malformed(
                0,
                format!(
                    "CPK header table must have exactly 1 row, has {}",
                    header_table.rows.len()
                ),
            )
            .into());
        }
        // The `.next()` is `Some` here because we just checked rows.len() == 1;
        // express it as an error path anyway to keep this function panic-free.
        let header_row = header_table.rows.into_iter().next().ok_or_else(|| {
            BinError::malformed(0, "CPK header row vector empty after length check")
        })?;
        let header_columns = header_table.columns;

        let content_offset = required_u64(&header_row, "ContentOffset", "CPK header")?;
        let toc_offset = required_u64(&header_row, "TocOffset", "CPK header")?;
        let etoc_offset = header_row
            .get("EtocOffset")
            .and_then(UtfValue::as_u64)
            .unwrap_or(0);
        let itoc_offset = header_row
            .get("ItocOffset")
            .and_then(UtfValue::as_u64)
            .unwrap_or(0);
        let gtoc_offset = header_row
            .get("GtocOffset")
            .and_then(UtfValue::as_u64)
            .unwrap_or(0);
        let etoc_size = header_row
            .get("EtocSize")
            .and_then(UtfValue::as_u64)
            .unwrap_or(0);
        let itoc_size = header_row
            .get("ItocSize")
            .and_then(UtfValue::as_u64)
            .unwrap_or(0);
        let gtoc_size = header_row
            .get("GtocSize")
            .and_then(UtfValue::as_u64)
            .unwrap_or(0);

        if toc_offset == 0 {
            return Err(CpkParseError::MissingToc);
        }

        // TOC packet.
        r.seek(toc_offset as usize)?;
        let toc_table = read_packet(&mut r, MAGIC_TOC)?;
        let toc_columns = toc_table.columns.clone();

        // Pass-through optional packets.
        let etoc_packet = if etoc_offset != 0 {
            Some(slice_packet(input, etoc_offset, etoc_size)?)
        } else {
            None
        };
        let itoc_packet = if itoc_offset != 0 {
            Some(slice_packet(input, itoc_offset, itoc_size)?)
        } else {
            None
        };
        let gtoc_packet = if gtoc_offset != 0 {
            Some(slice_packet(input, gtoc_offset, gtoc_size)?)
        } else {
            None
        };

        // Decide which base is added to each TOC row's FileOffset to get the
        // file's absolute position. Two conventions exist in the wild:
        //   • absolute = TocOffset + FileOffset  (DR V3 and most modern CPKs)
        //   • absolute = ContentOffset + FileOffset  (alternate base, seen on some archives)
        // We sniff with the first row — whichever base lands every file
        // inside [ContentOffset, file_end] is the right one. See
        // `pick_offset_base` below.
        let absolute_base = pick_offset_base(&toc_table, content_offset, toc_offset, input.len())?;

        let mut files: Vec<CpkFile> = Vec::with_capacity(toc_table.rows.len());
        for row in toc_table.rows {
            let dir_name = row
                .get("DirName")
                .and_then(UtfValue::as_str)
                .unwrap_or("")
                .to_string();
            let file_name = row
                .get("FileName")
                .and_then(UtfValue::as_str)
                .ok_or_else(|| CpkParseError::MissingColumn("FileName".into(), "TOC row"))?
                .to_string();
            let id = row.get("ID").and_then(UtfValue::as_u32).unwrap_or(0);
            let user_string = row
                .get("UserString")
                .and_then(UtfValue::as_str)
                .unwrap_or("")
                .to_string();
            // FileSize / ExtractSize are typically declared u32 in the TOC
            // schema, but a CPK can legally declare them u64 (DR V3's three
            // archives all use u32; we accept the wider variant for
            // compatibility). `as_u64` widens both widths to u64 cleanly.
            let file_size = row
                .get("FileSize")
                .and_then(UtfValue::as_u64)
                .ok_or_else(|| CpkParseError::MissingColumn("FileSize".into(), "TOC row"))?;
            let extract_size = row
                .get("ExtractSize")
                .and_then(UtfValue::as_u64)
                .unwrap_or(file_size);
            let rel_offset = row
                .get("FileOffset")
                .and_then(UtfValue::as_u64)
                .ok_or_else(|| CpkParseError::MissingColumn("FileOffset".into(), "TOC row"))?;
            let absolute = absolute_base + rel_offset;
            let abs = absolute as usize;
            let end = abs + file_size as usize;
            if end > input.len() {
                return Err(BinError::malformed(
                    abs,
                    format!(
                        "file body out of range (end {end}, file size {})",
                        input.len()
                    ),
                )
                .into());
            }
            let raw = &input[abs..end];
            let data = if file_size == extract_size {
                Cow::Borrowed(raw)
            } else if crilayla::is_crilayla(raw) {
                return Err(CpkParseError::Crilayla(
                    crilayla::CrilaylaError::NotImplemented,
                ));
            } else {
                return Err(BinError::malformed(
                    abs,
                    format!(
                        "file size ({file_size}) != extract size ({extract_size}) but data is not CRILAYLA"
                    ),
                )
                .into());
            };

            let mut extra: IndexMap<String, UtfValue> = IndexMap::new();
            for (k, v) in row {
                if !matches!(
                    k.as_str(),
                    "DirName"
                        | "FileName"
                        | "ID"
                        | "UserString"
                        | "FileSize"
                        | "ExtractSize"
                        | "FileOffset"
                ) {
                    extra.insert(k, v);
                }
            }

            files.push(CpkFile {
                dir_name,
                file_name,
                id,
                user_string,
                extra,
                data,
            });
        }

        Ok(Self {
            header_row,
            header_columns,
            toc_columns,
            files,
            etoc_packet,
            itoc_packet,
            gtoc_packet,
        })
    }

    /// Encode the CPK to a byte vector.
    ///
    /// The writer:
    /// - Resolves each row's `FileOffset` against `TocOffset` (the canonical
    ///   CRI convention used by every DR V3 archive). The reader auto-sniffs
    ///   the alternative `ContentOffset + FileOffset` convention.
    /// - Stores every file uncompressed (`FileSize == ExtractSize`).
    /// - Aligns the content blob to `Align` from the header (defaults to
    ///   `0x800` if absent).
    /// - Pads each file body to `Align` when `Sorted == 1`. The CRIWARE
    ///   loader issues sector-aligned DMA reads at each `FileOffset`; an
    ///   unaligned offset can cause the runtime to spin indefinitely.
    /// - Preserves header metadata fields verbatim except for the
    ///   layout-derived fields, which are recomputed: `ContentOffset`,
    ///   `ContentSize`, `TocOffset`, `TocSize`, `EtocOffset`/`EtocSize`,
    ///   `ItocOffset`/`ItocSize`, `GtocOffset`/`GtocSize`, `Files`.
    ///
    /// Implementation: a two-pass layout. The first pass builds the TOC and
    /// header with placeholder offsets just to learn their sizes; the second
    /// pass plans the byte layout, rebuilds both tables with the real offsets,
    /// and verifies neither changed size before assembling the final buffer.
    ///
    /// # Errors
    ///
    /// Returns an error if `Align` is not a positive power of two,
    /// `toc_columns` is empty, a file size exceeds the declared column
    /// width (`u32` overflow), or the second-pass TOC / header bytes
    /// don't match the size measured in the first pass (an internal
    /// inconsistency in the writer's planner).
    pub fn to_bytes(&self) -> CpkResult<Vec<u8>> {
        let align = self.resolve_align()?;
        let pad_files = sorted_flag(&self.header_row) != 0;

        // Pass 1: placeholder build, only to measure packet sizes. Two passes
        // are unavoidable, not wasteful: the file/table offsets depend on the
        // packet sizes, but the sizes are content-determined and independent of
        // the offsets, so the writer must measure sizes first, then plan offsets
        // and rebuild. The size-stability checks below guard that invariant —
        // do not fold this into a single pass.
        let toc_size_pass1 =
            self.build_toc_table()?.to_bytes()?.len() + PACKET_WRAPPER_SIZE as usize;
        let header_size_pass1 = self
            .build_header_table(HeaderLayout::default())?
            .to_bytes()?
            .len()
            + PACKET_WRAPPER_SIZE as usize;

        let layout = self.plan_layout(align, pad_files, toc_size_pass1, header_size_pass1);

        // Pass 2: real build, with size-stability checks against pass 1.
        let toc_table = self.build_toc_table_with_layout(&layout.file_offsets)?;
        let toc_bytes = toc_table.to_bytes()?;
        let toc_size_pass2 = toc_bytes.len() + PACKET_WRAPPER_SIZE as usize;
        if toc_size_pass2 != toc_size_pass1 {
            return Err(BinError::malformed(
                0,
                format!("TOC size changed between passes ({toc_size_pass1} -> {toc_size_pass2})"),
            )
            .into());
        }

        let header_table = self.build_header_table(layout.as_header_layout())?;
        let header_bytes = header_table.to_bytes()?;
        let header_size_pass2 = header_bytes.len() + PACKET_WRAPPER_SIZE as usize;
        if header_size_pass2 != header_size_pass1 {
            return Err(BinError::malformed(
                0,
                format!(
                    "Header size changed between passes ({header_size_pass1} -> {header_size_pass2})"
                ),
            )
            .into());
        }

        Ok(self.assemble_bytes(&layout, &header_bytes, &toc_bytes))
    }

    /// Read and validate the `Align` header field.
    fn resolve_align(&self) -> CpkResult<u64> {
        let align = self
            .header_row
            .get("Align")
            .and_then(|v| match v {
                UtfValue::U16(x) => Some(u64::from(*x)),
                UtfValue::U32(x) => Some(u64::from(*x)),
                _ => None,
            })
            .unwrap_or(u64::from(DEFAULT_ALIGN));
        if align == 0 || !align.is_power_of_two() {
            return Err(BinError::malformed(
                0,
                format!("Align {align} must be a positive power of two"),
            )
            .into());
        }
        Ok(align)
    }

    /// Plan every byte offset in the final layout given the pass-1 packet
    /// sizes. Pure function of `self`, `align`, and the two sizes — no I/O.
    ///
    /// Canonical CRI CPK layout (verified empirically against both shipped
    /// DR V3 CPKs):
    ///
    /// ```text
    /// [Header packet][pad to Align][TOC packet][pad 16][ITOC?][GTOC?][pad to Align][content blob with per-file Align pad][ETOC packet at file end]
    /// ```
    ///
    /// Two non-obvious invariants:
    /// - `TocOffset % Align == 0` — TOC starts on an Align sector so CRIWARE's
    ///   sector-aligned DMA reads land correctly.
    /// - `EtocOffset + EtocSize == FileSize` — ETOC terminates the file. The
    ///   loader uses this to compute the end of the content blob.
    fn plan_layout(
        &self,
        align: u64,
        pad_files: bool,
        toc_packet_size: usize,
        header_packet_size: usize,
    ) -> Layout {
        // Header → pad to Align → TOC → pad 16 → ITOC → GTOC → pad to Align
        //  → content blob (per-file Align pad) → ETOC at file end.
        let mut cursor: u64 = pad_to_u64(header_packet_size as u64, align);
        let toc_offset = cursor;
        cursor = pad_to_u64(cursor + toc_packet_size as u64, 16);

        let mut place_inner = |packet: &Option<Vec<u8>>| -> u64 {
            match packet {
                Some(p) => {
                    let off = cursor;
                    cursor = pad_to_u64(cursor + p.len() as u64, 16);
                    off
                }
                None => 0,
            }
        };
        // ITOC and GTOC (when present) live between TOC and the content blob
        // per CriPakTools convention. DR V3 doesn't ship either of these, so
        // this code path is exercised by synthetic tests only.
        let itoc_offset = place_inner(&self.itoc_packet);
        let gtoc_offset = place_inner(&self.gtoc_packet);

        let content_offset = pad_to_u64(cursor, align);
        cursor = content_offset;

        // File bodies. Each body is padded up to `Align` when Sorted == 1
        // (the layout invariant CRIWARE's loader expects: sector-aligned
        // FileOffsets so DMA reads succeed). FileOffset is emitted relative
        // to TocOffset, matching the canonical CRI convention.
        let mut file_offsets: Vec<u64> = Vec::with_capacity(self.files.len());
        for file in &self.files {
            file_offsets.push(cursor - toc_offset);
            cursor += file.data.len() as u64;
            if pad_files {
                cursor = pad_to_u64(cursor, align);
            }
        }
        let content_size = cursor - content_offset;

        // ETOC at the file END, immediately after the content blob's last
        // per-file pad. No further padding; `EtocOffset + EtocSize == FileSize`.
        let etoc_offset = if self.etoc_packet.is_some() {
            cursor
        } else {
            0
        };
        if let Some(p) = &self.etoc_packet {
            cursor += p.len() as u64;
        }

        Layout {
            align,
            pad_files,
            toc_offset,
            toc_packet_size: toc_packet_size as u64,
            etoc_offset,
            etoc_size: self.etoc_packet.as_ref().map_or(0, |p| p.len() as u64),
            itoc_offset,
            itoc_size: self.itoc_packet.as_ref().map_or(0, |p| p.len() as u64),
            gtoc_offset,
            gtoc_size: self.gtoc_packet.as_ref().map_or(0, |p| p.len() as u64),
            content_offset,
            content_size,
            file_offsets,
            total_size: cursor,
        }
    }

    /// Emit the final byte stream from a planned layout and the rebuilt packet
    /// payloads. The layout pre-computed every offset, so this function is
    /// effectively a sequence of `extend_from_slice` calls with sanity-check
    /// `debug_assert`s against the planned positions.
    ///
    /// Ordering matches the canonical CRI layout (see [`Cpk::plan_layout`]):
    /// header → TOC → ITOC? → GTOC? → content → ETOC (file end).
    fn assemble_bytes(&self, layout: &Layout, header_bytes: &[u8], toc_bytes: &[u8]) -> Vec<u8> {
        let mut out: Vec<u8> = Vec::with_capacity(layout.total_size as usize);

        // Header packet, padded up to Align so TOC starts on a sector boundary.
        write_packet(&mut out, MAGIC_CPK, header_bytes);
        pad_to(&mut out, layout.align as usize);
        debug_assert_eq!(out.len() as u64, layout.toc_offset);

        // TOC packet, padded to 16 before any inner optional packets.
        write_packet(&mut out, MAGIC_TOC, toc_bytes);
        pad_to(&mut out, 16);

        // ITOC and GTOC (when present) live between TOC and content.
        // ETOC does NOT — it goes at file end (below).
        for (packet, planned_offset) in [
            (&self.itoc_packet, layout.itoc_offset),
            (&self.gtoc_packet, layout.gtoc_offset),
        ] {
            if let Some(p) = packet {
                debug_assert_eq!(out.len() as u64, planned_offset);
                out.extend_from_slice(p);
                pad_to(&mut out, 16);
            }
        }

        // Content blob — start aligned to Align.
        pad_to(&mut out, layout.align as usize);
        debug_assert_eq!(out.len() as u64, layout.content_offset);

        for file in &self.files {
            out.extend_from_slice(&file.data);
            if layout.pad_files {
                pad_to(&mut out, layout.align as usize);
            }
        }

        // ETOC packet at file END (no trailing pad).
        // EtocOffset + EtocSize == FileSize after this.
        if let Some(p) = &self.etoc_packet {
            debug_assert_eq!(out.len() as u64, layout.etoc_offset);
            out.extend_from_slice(p);
        }

        debug_assert_eq!(out.len() as u64, layout.total_size);
        out
    }

    fn build_toc_table(&self) -> CpkResult<UtfTable> {
        let placeholders: Vec<u64> = vec![0; self.files.len()];
        self.build_toc_table_with_layout(&placeholders)
    }

    /// Build the TOC `@UTF` table using `self.toc_columns` as the authoritative
    /// schema. Standard columns (`DirName` / `FileName` / `FileSize` /
    /// `ExtractSize` / `FileOffset` / `ID` / `UserString`) draw their value
    /// from the dedicated `CpkFile` fields; anything else is looked up in
    /// `file.extra` by name.
    fn build_toc_table_with_layout(&self, file_offsets: &[u64]) -> CpkResult<UtfTable> {
        if self.toc_columns.is_empty() {
            return Err(BinError::malformed(
                0,
                "CPK has no TOC schema (toc_columns is empty); set Cpk::toc_columns or use \
                 Cpk::default_toc_columns()",
            )
            .into());
        }

        let mut rows: Vec<UtfRow> = Vec::with_capacity(self.files.len());
        for (idx, file) in self.files.iter().enumerate() {
            let mut row: UtfRow = UtfRow::new();
            for col in &self.toc_columns {
                if !col.storage.has_row_value() {
                    continue;
                }
                let name = col
                    .name
                    .as_deref()
                    .ok_or_else(|| BinError::malformed(0, "PerRow TOC column has no name"))?;
                let value = toc_row_value(name, col, file, file_offsets[idx])?;
                row.insert(name.to_string(), value);
            }
            rows.push(row);
        }

        Ok(UtfTable {
            name: "CpkTocInfo".into(),
            columns: self.toc_columns.clone(),
            rows,
        })
    }

    #[expect(
        clippy::unnecessary_wraps,
        reason = "mirrors the fallible build_toc_table_with_layout so the two-pass writer treats header and TOC uniformly"
    )]
    fn build_header_table(&self, layout: HeaderLayout) -> CpkResult<UtfTable> {
        // Start from preserved columns and substitute the layout-derived
        // fields. `Files` is recomputed from `self.files.len()` for safety.
        // Optional packet pointers (`Etoc*` / `Itoc*` / `Gtoc*`) are *always*
        // written — when the corresponding packet is absent we emit `0`, which
        // matches CRI's convention (a zero offset is the "absent" sentinel).
        let mut row = self.header_row.clone();
        row.insert("ContentOffset".into(), UtfValue::U64(layout.content_offset));
        row.insert("ContentSize".into(), UtfValue::U64(layout.content_size));
        row.insert("TocOffset".into(), UtfValue::U64(layout.toc_offset));
        row.insert("TocSize".into(), UtfValue::U64(layout.toc_size));
        row.insert("EtocOffset".into(), UtfValue::U64(layout.etoc_offset));
        row.insert("EtocSize".into(), UtfValue::U64(layout.etoc_size));
        row.insert("ItocOffset".into(), UtfValue::U64(layout.itoc_offset));
        row.insert("ItocSize".into(), UtfValue::U64(layout.itoc_size));
        row.insert("GtocOffset".into(), UtfValue::U64(layout.gtoc_offset));
        row.insert("GtocSize".into(), UtfValue::U64(layout.gtoc_size));
        row.insert("Files".into(), UtfValue::U32(self.files.len() as u32));
        Ok(UtfTable {
            name: "CpkHeader".into(),
            columns: self.header_columns.clone(),
            rows: vec![row],
        })
    }
}

/// All layout-derived header-row fields that depend on the final byte layout.
#[derive(Debug, Clone, Copy, Default)]
struct HeaderLayout {
    content_offset: u64,
    content_size: u64,
    toc_offset: u64,
    toc_size: u64,
    etoc_offset: u64,
    etoc_size: u64,
    itoc_offset: u64,
    itoc_size: u64,
    gtoc_offset: u64,
    gtoc_size: u64,
}

/// Full byte layout planned by [`Cpk::plan_layout`], consumed by
/// [`Cpk::assemble_bytes`] and [`Cpk::build_header_table`].
#[derive(Debug, Clone)]
struct Layout {
    align: u64,
    pad_files: bool,
    toc_offset: u64,
    toc_packet_size: u64,
    etoc_offset: u64,
    etoc_size: u64,
    itoc_offset: u64,
    itoc_size: u64,
    gtoc_offset: u64,
    gtoc_size: u64,
    content_offset: u64,
    content_size: u64,
    /// File offsets, each relative to `toc_offset`. The CRIWARE convention
    /// computes a file's absolute position as `toc_offset + file_offset`.
    file_offsets: Vec<u64>,
    total_size: u64,
}

impl Layout {
    fn as_header_layout(&self) -> HeaderLayout {
        HeaderLayout {
            content_offset: self.content_offset,
            content_size: self.content_size,
            toc_offset: self.toc_offset,
            toc_size: self.toc_packet_size,
            etoc_offset: self.etoc_offset,
            etoc_size: self.etoc_size,
            itoc_offset: self.itoc_offset,
            itoc_size: self.itoc_size,
            gtoc_offset: self.gtoc_offset,
            gtoc_size: self.gtoc_size,
        }
    }
}

/// Slice a complete packet (16-byte wrapper plus `@UTF` payload) at
/// `offset`, using the CPK header's `*Size` column for length.
///
/// **The CPK header's `TocSize` / `EtocSize` / `ItocSize` / `GtocSize`
/// columns store the *total* packet size — wrapper bytes already
/// included.** Verified empirically against both shipped DR V3 CPKs:
/// `partition_resident_win.cpk` has header `TocSize = 0x1640` while the
/// wrapper's own `PacketSize` field at `TocOffset` is `0x1630`, so the
/// difference is exactly the 16-byte wrapper. The reader and writer both
/// follow this convention.
fn slice_packet(input: &[u8], offset: u64, declared_size: u64) -> BinResult<Vec<u8>> {
    let start = offset as usize;
    let len = declared_size as usize;
    let end = start
        .checked_add(len)
        .ok_or_else(|| BinError::malformed(start, format!("packet at {start:#x} size overflow")))?;
    if end > input.len() {
        return Err(BinError::malformed(
            start,
            format!(
                "packet at {start:#x} extends past file (declared {len:#x}, file {:#x})",
                input.len()
            ),
        ));
    }
    Ok(input[start..end].to_vec())
}

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "magic is borrowed to match the &[u8; 4] MAGIC_* constants passed at call sites"
)]
fn read_packet(r: &mut Reader<'_>, expected_magic: &[u8; 4]) -> BinResult<UtfTable> {
    r.expect_magic(expected_magic)?;
    let _flag = r.u32_le()?;
    let packet_size = r.u64_le()?;
    let utf_start = r.position();
    let utf_end = utf_start
        .checked_add(packet_size as usize)
        .ok_or_else(|| BinError::malformed(utf_start, "packet size overflows buffer"))?;
    let table = UtfTable::parse(r.subslice(utf_start, utf_end)?)?;
    r.seek(utf_end)?;
    Ok(table)
}

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "magic is borrowed to match the &[u8; 4] MAGIC_* constants passed at call sites"
)]
fn write_packet(out: &mut Vec<u8>, magic: &[u8; 4], utf_bytes: &[u8]) {
    out.extend_from_slice(magic);
    out.extend_from_slice(&0xFFu32.to_le_bytes());
    out.extend_from_slice(&(utf_bytes.len() as u64).to_le_bytes());
    out.extend_from_slice(utf_bytes);
}

fn pad_to(out: &mut Vec<u8>, alignment: usize) {
    out.resize(drv3_binio::align_up(out.len(), alignment), 0);
}

fn pad_to_u64(value: u64, alignment: u64) -> u64 {
    drv3_binio::align_up(value, alignment)
}

fn required_u64(row: &UtfRow, name: &str, ctx: &'static str) -> CpkResult<u64> {
    row.get(name)
        .and_then(UtfValue::as_u64)
        .ok_or_else(|| CpkParseError::MissingColumn(name.to_string(), ctx))
}

/// Resolve the value for one `PerRow` TOC column.
///
/// Standard columns map to dedicated `CpkFile` fields; anything else falls
/// through to `file.extra`. Numeric standard columns honor the column's
/// declared type — most CPKs use `u32` for `FileSize` / `ExtractSize` / `ID`
/// and `u64` for `FileOffset`, but a CPK with `u64 FileSize` (rare but legal)
/// is supported transparently.
fn toc_row_value(
    name: &str,
    col: &UtfColumn,
    file: &CpkFile<'_>,
    file_offset: u64,
) -> CpkResult<UtfValue> {
    let size_value = |size: usize| -> CpkResult<UtfValue> {
        match col.ty {
            UtfType::U32 => Ok(UtfValue::U32(u32::try_from(size).map_err(|_| {
                BinError::malformed(
                    0,
                    format!("file {:?} size {size} exceeds u32 column", file.file_name),
                )
            })?)),
            UtfType::U64 => Ok(UtfValue::U64(size as u64)),
            other => Err(BinError::malformed(
                0,
                format!("{name:?} column has unsupported type {other:?}"),
            )
            .into()),
        }
    };
    let id_value = |id: u32| -> CpkResult<UtfValue> {
        match col.ty {
            UtfType::U32 => Ok(UtfValue::U32(id)),
            UtfType::U64 => Ok(UtfValue::U64(u64::from(id))),
            other => Err(BinError::malformed(
                0,
                format!("{name:?} column has unsupported type {other:?}"),
            )
            .into()),
        }
    };
    match name {
        "DirName" => Ok(UtfValue::String(file.dir_name.clone())),
        "FileName" => Ok(UtfValue::String(file.file_name.clone())),
        "UserString" => Ok(UtfValue::String(file.user_string.clone())),
        "FileSize" | "ExtractSize" => size_value(file.data.len()),
        "FileOffset" => match col.ty {
            UtfType::U64 => Ok(UtfValue::U64(file_offset)),
            UtfType::U32 => Ok(UtfValue::U32(u32::try_from(file_offset).map_err(|_| {
                BinError::malformed(0, format!("FileOffset {file_offset:#x} exceeds u32 column"))
            })?)),
            other => Err(BinError::malformed(
                0,
                format!("FileOffset column has unsupported type {other:?}"),
            )
            .into()),
        },
        "ID" => id_value(file.id),
        other => file.extra.get(other).cloned().ok_or_else(|| {
            BinError::malformed(
                0,
                format!(
                    "file {:?} missing PerRow column {other:?} declared in toc_columns",
                    file.file_name,
                ),
            )
            .into()
        }),
    }
}

/// Read the `Sorted` header field. Defaults to `1` (sorted, padded) — every
/// DR V3 CPK ships sorted, and CRI's own reader treats absence as "sorted".
fn sorted_flag(row: &UtfRow) -> u64 {
    row.get("Sorted")
        .and_then(|v| match v {
            UtfValue::U16(x) => Some(u64::from(*x)),
            UtfValue::U32(x) => Some(u64::from(*x)),
            _ => None,
        })
        .unwrap_or(1)
}

/// Pick the base offset for resolving TOC `FileOffset` to absolute file
/// positions. Two conventions exist in CPK files in the wild:
///
/// - `absolute = TocOffset + FileOffset` — canonical CRIWARE behavior;
///   what DR V3 and every modern CPK uses.
/// - `absolute = ContentOffset + FileOffset` — alternate base, seen on
///   some archives.
///
/// We sniff both bases so the reader works on either. For a candidate base
/// to be valid:
/// 1. Every row's `(abs, abs + file_size)` must fall inside `[ContentOffset, file_len]`
///    — files always live in the content blob, never inside packet headers.
/// 2. The first row's `abs` should be exactly `ContentOffset` (the start of the
///    content blob), as a sharper check than just `>= ContentOffset`.
///
/// Among bases that satisfy both, we prefer `TocOffset` (canonical).
fn pick_offset_base(
    toc_table: &UtfTable,
    content_offset: u64,
    toc_offset: u64,
    file_len: usize,
) -> CpkResult<u64> {
    let candidate_score = |base: u64| -> Option<u8> {
        let mut score: u8 = 0;
        for (row_idx, row) in toc_table.rows.iter().enumerate() {
            let rel = row
                .get("FileOffset")
                .and_then(UtfValue::as_u64)
                .unwrap_or(0);
            let size = row.get("FileSize").and_then(UtfValue::as_u64).unwrap_or(0);
            let abs = base.saturating_add(rel);
            let end = abs.saturating_add(size);
            if abs < content_offset || (end as usize) > file_len {
                return None;
            }
            if row_idx == 0 && abs == content_offset {
                score = 1;
            }
        }
        Some(score)
    };

    let mut best: Option<(u64, u8)> = None;
    // TocOffset first so it wins ties.
    for &base in &[toc_offset, content_offset] {
        if let Some(score) = candidate_score(base)
            && best.is_none_or(|(_, bs)| score > bs)
        {
            best = Some((base, score));
        }
    }

    best.map(|(b, _)| b).ok_or_else(|| {
        BinError::malformed(
            0,
            format!(
                "no offset base ({toc_offset:#x} or {content_offset:#x}) keeps all TOC rows \
                 inside the content blob ({content_offset:#x}..{file_len:#x})",
            ),
        )
        .into()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[expect(
        clippy::too_many_lines,
        reason = "test fixture enumerating a full 17-column CPK header schema"
    )]
    fn sample_cpk() -> Cpk<'static> {
        let mut header_row = UtfRow::new();
        // Seed required header columns. Pre-populate the four "layout-derived"
        // fields with placeholder zeros; the writer overrides them.
        header_row.insert(
            "UpdateDateTime".into(),
            UtfValue::U64(0x1122_3344_5566_7788),
        );
        header_row.insert("ContentOffset".into(), UtfValue::U64(0));
        header_row.insert("ContentSize".into(), UtfValue::U64(0));
        header_row.insert("TocOffset".into(), UtfValue::U64(0));
        header_row.insert("TocSize".into(), UtfValue::U64(0));
        header_row.insert("EtocOffset".into(), UtfValue::U64(0));
        header_row.insert("EtocSize".into(), UtfValue::U64(0));
        header_row.insert("ItocOffset".into(), UtfValue::U64(0));
        header_row.insert("ItocSize".into(), UtfValue::U64(0));
        header_row.insert("GtocOffset".into(), UtfValue::U64(0));
        header_row.insert("GtocSize".into(), UtfValue::U64(0));
        header_row.insert("Files".into(), UtfValue::U32(0));
        header_row.insert("Align".into(), UtfValue::U16(0x800));
        header_row.insert("Sorted".into(), UtfValue::U16(1));
        header_row.insert("Version".into(), UtfValue::U16(7));
        header_row.insert("Revision".into(), UtfValue::U16(14));
        header_row.insert("Tvers".into(), UtfValue::String("drv3-cpk 0.1".into()));

        // Mirror a real CPK header: PerRow for active offsets, Zero for the
        // optional packet pointers that aren't used (so the writer exercises
        // the Zero-storage path).
        let header_columns = vec![
            UtfColumn {
                name: Some("UpdateDateTime".into()),
                storage: StorageFlag::PerRow,
                ty: UtfType::U64,
                constant: None,
            },
            UtfColumn {
                name: Some("ContentOffset".into()),
                storage: StorageFlag::PerRow,
                ty: UtfType::U64,
                constant: None,
            },
            UtfColumn {
                name: Some("ContentSize".into()),
                storage: StorageFlag::PerRow,
                ty: UtfType::U64,
                constant: None,
            },
            UtfColumn {
                name: Some("TocOffset".into()),
                storage: StorageFlag::PerRow,
                ty: UtfType::U64,
                constant: None,
            },
            UtfColumn {
                name: Some("TocSize".into()),
                storage: StorageFlag::PerRow,
                ty: UtfType::U64,
                constant: None,
            },
            UtfColumn {
                name: Some("EtocOffset".into()),
                storage: StorageFlag::Zero,
                ty: UtfType::U64,
                constant: None,
            },
            UtfColumn {
                name: Some("EtocSize".into()),
                storage: StorageFlag::Zero,
                ty: UtfType::U64,
                constant: None,
            },
            UtfColumn {
                name: Some("ItocOffset".into()),
                storage: StorageFlag::Zero,
                ty: UtfType::U64,
                constant: None,
            },
            UtfColumn {
                name: Some("ItocSize".into()),
                storage: StorageFlag::Zero,
                ty: UtfType::U64,
                constant: None,
            },
            UtfColumn {
                name: Some("GtocOffset".into()),
                storage: StorageFlag::Zero,
                ty: UtfType::U64,
                constant: None,
            },
            UtfColumn {
                name: Some("GtocSize".into()),
                storage: StorageFlag::Zero,
                ty: UtfType::U64,
                constant: None,
            },
            UtfColumn {
                name: Some("Files".into()),
                storage: StorageFlag::PerRow,
                ty: UtfType::U32,
                constant: None,
            },
            UtfColumn {
                name: Some("Align".into()),
                storage: StorageFlag::PerRow,
                ty: UtfType::U16,
                constant: None,
            },
            UtfColumn {
                name: Some("Sorted".into()),
                storage: StorageFlag::PerRow,
                ty: UtfType::U16,
                constant: None,
            },
            UtfColumn {
                name: Some("Version".into()),
                storage: StorageFlag::PerRow,
                ty: UtfType::U16,
                constant: None,
            },
            UtfColumn {
                name: Some("Revision".into()),
                storage: StorageFlag::PerRow,
                ty: UtfType::U16,
                constant: None,
            },
            UtfColumn {
                name: Some("Tvers".into()),
                storage: StorageFlag::Constant,
                ty: UtfType::String,
                constant: Some(UtfValue::String("drv3-cpk 0.1.1".into())),
            },
        ];

        Cpk {
            header_row,
            header_columns,
            toc_columns: Cpk::default_toc_columns(),
            files: vec![
                CpkFile {
                    dir_name: String::new(),
                    file_name: "file1.stx".into(),
                    id: 1,
                    user_string: String::new(),
                    extra: IndexMap::new(),
                    data: b"<<file1 contents>>".to_vec().into(),
                },
                CpkFile {
                    dir_name: "sub".into(),
                    file_name: "file2.wrd".into(),
                    id: 2,
                    user_string: "tag".into(),
                    extra: IndexMap::new(),
                    data: vec![0x42; 137].into(),
                },
            ],
            etoc_packet: None,
            itoc_packet: None,
            gtoc_packet: None,
        }
    }

    #[test]
    fn file_lookup_by_dir_and_name() {
        let cpk = sample_cpk();
        assert_eq!(cpk.file("sub", "file2.wrd").unwrap().id, 2);
        // Wrong directory must not match.
        assert!(cpk.file("", "file2.wrd").is_none());
        assert!(cpk.file("nope", "x").is_none());
    }

    #[test]
    fn round_trip_preserves_files() {
        let cpk = sample_cpk();
        let bytes = cpk.to_bytes().unwrap();
        let parsed = Cpk::parse(&bytes).unwrap();
        assert_eq!(parsed.files.len(), cpk.files.len());
        for (a, b) in parsed.files.iter().zip(cpk.files.iter()) {
            assert_eq!(a.dir_name, b.dir_name);
            assert_eq!(a.file_name, b.file_name);
            assert_eq!(a.id, b.id);
            assert_eq!(a.user_string, b.user_string);
            assert_eq!(a.data, b.data);
        }
    }

    #[test]
    fn content_blob_is_aligned() {
        let bytes = sample_cpk().to_bytes().unwrap();
        let parsed = Cpk::parse(&bytes).unwrap();
        let content_offset = parsed
            .header_row
            .get("ContentOffset")
            .unwrap()
            .as_u64()
            .unwrap();
        assert_eq!(content_offset % 0x800, 0);
    }

    #[test]
    fn second_write_is_byte_stable() {
        let bytes1 = sample_cpk().to_bytes().unwrap();
        let parsed = Cpk::parse(&bytes1).unwrap();
        let bytes2 = parsed.to_bytes().unwrap();
        assert_eq!(bytes1, bytes2);
    }

    /// When `Sorted == 1`, every file body must start on an `Align`
    /// boundary so CRIWARE's sector-aligned async reads land correctly. An
    /// unaligned `FileOffset` reliably hangs the in-game loader (verified
    /// against `partition_data_win_us.cpk`).
    #[test]
    fn each_file_body_is_align_padded_when_sorted() {
        let cpk = {
            let mut c = sample_cpk();
            // Replace the two stock files with three of intentionally awkward
            // sizes — none divisible by 0x800.
            c.files = vec![
                CpkFile {
                    dir_name: String::new(),
                    file_name: "a.bin".into(),
                    id: 0,
                    user_string: String::new(),
                    extra: IndexMap::new(),
                    data: vec![0xAA; 137].into(),
                },
                CpkFile {
                    dir_name: "sub".into(),
                    file_name: "b.bin".into(),
                    id: 1,
                    user_string: String::new(),
                    extra: IndexMap::new(),
                    data: vec![0xBB; 5_000].into(),
                },
                CpkFile {
                    dir_name: String::new(),
                    file_name: "c.bin".into(),
                    id: 2,
                    user_string: String::new(),
                    extra: IndexMap::new(),
                    data: vec![0xCC; 1].into(),
                },
            ];
            c
        };

        let bytes = cpk.to_bytes().unwrap();
        let parsed = Cpk::parse(&bytes).unwrap();

        let align = parsed
            .header_row
            .get("Align")
            .and_then(UtfValue::as_u64)
            .unwrap();
        let content_offset = parsed
            .header_row
            .get("ContentOffset")
            .and_then(UtfValue::as_u64)
            .unwrap();
        let toc_offset = parsed
            .header_row
            .get("TocOffset")
            .and_then(UtfValue::as_u64)
            .unwrap();

        // The serialized form must store per-file padding on disk.
        let body_bytes: u64 = cpk.files.iter().map(|f| f.data.len() as u64).sum();
        let content_size = parsed
            .header_row
            .get("ContentSize")
            .and_then(UtfValue::as_u64)
            .unwrap();
        assert!(
            content_size > body_bytes,
            "ContentSize {content_size} must exceed sum of file bodies {body_bytes} (padding present)",
        );

        // Round-trip parsing must show every absolute file offset aligned.
        for (parsed_file, original) in parsed.files.iter().zip(cpk.files.iter()) {
            // Body bytes survive byte-equal — the parser slices [abs, abs+size).
            assert_eq!(parsed_file.data, original.data);
        }

        // Re-derive each absolute file offset from the raw TOC bytes so the
        // assertion isn't a tautology over the path we just used to parse.
        let toc_table = {
            let mut r = drv3_binio::Reader::new(&bytes);
            r.seek(toc_offset as usize).unwrap();
            super::read_packet(&mut r, super::MAGIC_TOC).unwrap()
        };
        for row in &toc_table.rows {
            let rel = row.get("FileOffset").and_then(UtfValue::as_u64).unwrap();
            let absolute = toc_offset + rel;
            assert!(
                absolute >= content_offset,
                "file body must live inside the content blob",
            );
            assert_eq!(
                absolute % align,
                0,
                "FileOffset {absolute:#x} (rel {rel:#x}) is not aligned to {align:#x}",
            );
        }
    }

    /// `Sorted == 0` opt-out: bodies pack tightly with no padding between.
    /// This is what CRI itself does for unsorted CPKs.
    #[test]
    fn unsorted_cpks_are_tightly_packed() {
        let mut cpk = sample_cpk();
        cpk.header_row.insert("Sorted".into(), UtfValue::U16(0));
        cpk.files = vec![
            CpkFile {
                dir_name: String::new(),
                file_name: "a.bin".into(),
                id: 0,
                user_string: String::new(),
                extra: IndexMap::new(),
                data: vec![0xAA; 100].into(),
            },
            CpkFile {
                dir_name: String::new(),
                file_name: "b.bin".into(),
                id: 1,
                user_string: String::new(),
                extra: IndexMap::new(),
                data: vec![0xBB; 200].into(),
            },
        ];

        let bytes = cpk.to_bytes().unwrap();
        let parsed = Cpk::parse(&bytes).unwrap();

        let toc_offset = parsed
            .header_row
            .get("TocOffset")
            .and_then(UtfValue::as_u64)
            .unwrap();
        let toc_table = {
            let mut r = drv3_binio::Reader::new(&bytes);
            r.seek(toc_offset as usize).unwrap();
            super::read_packet(&mut r, super::MAGIC_TOC).unwrap()
        };
        let off0 = toc_table.rows[0]
            .get("FileOffset")
            .and_then(UtfValue::as_u64)
            .unwrap();
        let off1 = toc_table.rows[1]
            .get("FileOffset")
            .and_then(UtfValue::as_u64)
            .unwrap();
        assert_eq!(
            off1 - off0,
            100,
            "Sorted==0 must produce tightly-packed bodies; got gap {} (expected 100)",
            off1 - off0,
        );
    }

    /// When ETOC is present, it must terminate the file:
    /// `EtocOffset + EtocSize == file_size`. The CRIWARE loader uses this
    /// invariant to compute the end of the content blob; placing ETOC
    /// anywhere else (e.g. immediately after TOC) causes the loader to spin
    /// indefinitely on a nonsensical content-extent calculation. Verified
    /// against both shipped DR V3 CPKs.
    #[test]
    fn etoc_lives_at_file_end_when_present() {
        let mut cpk = sample_cpk();
        // sample_cpk() declares EtocOffset/EtocSize as Zero-storage (modeled
        // after a CPK with no ETOC). To emit a non-zero offset/size, upgrade
        // those columns to PerRow first — real shipped CPKs declare them
        // PerRow whenever ETOC is present.
        for col in &mut cpk.header_columns {
            if let Some(name) = &col.name
                && (name == "EtocOffset" || name == "EtocSize")
            {
                col.storage = StorageFlag::PerRow;
            }
        }
        cpk.etoc_packet = Some(
            b"\x45\x54\x4F\x43\xFF\x00\x00\x00\x10\x00\x00\x00\x00\x00\x00\x00FAKE-ETOC-PAYLOAD"
                .to_vec(),
        );

        let bytes = cpk.to_bytes().expect("write");
        let parsed = Cpk::parse(&bytes).expect("parse");

        let etoc_offset = parsed
            .header_row
            .get("EtocOffset")
            .and_then(UtfValue::as_u64)
            .expect("EtocOffset present");
        let etoc_size = parsed
            .header_row
            .get("EtocSize")
            .and_then(UtfValue::as_u64)
            .expect("EtocSize present");

        assert!(
            etoc_offset > 0,
            "EtocOffset must be non-zero when ETOC present"
        );
        assert_eq!(
            etoc_offset + etoc_size,
            bytes.len() as u64,
            "EtocOffset ({etoc_offset:#x}) + EtocSize ({etoc_size:#x}) must equal file size ({:#x})",
            bytes.len(),
        );

        // Verify the ETOC packet's actual bytes are at the planned offset
        // (not just that the header claims they are).
        let etoc_in_file = &bytes[etoc_offset as usize..];
        let etoc_original = cpk.etoc_packet.as_ref().unwrap();
        assert_eq!(
            &etoc_in_file[..etoc_original.len()],
            etoc_original.as_slice(),
            "ETOC bytes at planned offset must match the source ETOC packet",
        );
    }

    /// `TocOffset` must be aligned to `Align` so CRIWARE's sector-aligned
    /// DMA reads of the TOC land on a sector boundary. The shipped DR V3
    /// CPKs both have `TocOffset == 0x800 == Align`.
    #[test]
    fn toc_offset_is_align_aligned() {
        let bytes = sample_cpk().to_bytes().expect("write");
        let parsed = Cpk::parse(&bytes).expect("parse");

        let align = parsed
            .header_row
            .get("Align")
            .and_then(UtfValue::as_u64)
            .expect("Align present");
        let toc_offset = parsed
            .header_row
            .get("TocOffset")
            .and_then(UtfValue::as_u64)
            .expect("TocOffset present");

        assert_eq!(
            toc_offset % align,
            0,
            "TocOffset ({toc_offset:#x}) must be aligned to Align ({align:#x})",
        );
        // Sanity: for the sample CPK, Align is 0x800; pin the expected value
        // so a future Align-default change can't silently break this.
        assert_eq!(align, 0x800);
        assert_eq!(toc_offset, 0x800);
    }

    /// Non-default TOC schemas must survive round-trip: column order, types
    /// (e.g. `u64 FileSize`), and extra columns. `toc_columns` is a
    /// first-class `Cpk` field so the writer preserves non-default schemas
    /// verbatim instead of silently normalizing them.
    #[test]
    fn non_default_toc_schema_round_trips() {
        let mut cpk = sample_cpk();

        // Replace the default schema with one that:
        // - reorders columns (FileName before DirName),
        // - declares FileSize as u64 (not u32),
        // - adds an extra `CRC` u32 column,
        // - omits UserString (legal — the column is optional in CRI's encoding).
        cpk.toc_columns = vec![
            UtfColumn {
                name: Some("FileName".into()),
                storage: StorageFlag::PerRow,
                ty: UtfType::String,
                constant: None,
            },
            UtfColumn {
                name: Some("DirName".into()),
                storage: StorageFlag::PerRow,
                ty: UtfType::String,
                constant: None,
            },
            UtfColumn {
                name: Some("FileSize".into()),
                storage: StorageFlag::PerRow,
                ty: UtfType::U64,
                constant: None,
            },
            UtfColumn {
                name: Some("ExtractSize".into()),
                storage: StorageFlag::PerRow,
                ty: UtfType::U64,
                constant: None,
            },
            UtfColumn {
                name: Some("FileOffset".into()),
                storage: StorageFlag::PerRow,
                ty: UtfType::U64,
                constant: None,
            },
            UtfColumn {
                name: Some("ID".into()),
                storage: StorageFlag::PerRow,
                ty: UtfType::U32,
                constant: None,
            },
            UtfColumn {
                name: Some("CRC".into()),
                storage: StorageFlag::PerRow,
                ty: UtfType::U32,
                constant: None,
            },
        ];

        // Seed each file's `extra` with the CRC value so the writer can find it.
        for (i, file) in cpk.files.iter_mut().enumerate() {
            file.extra
                .insert("CRC".into(), UtfValue::U32(0xCAFE_0000 + i as u32));
        }

        let bytes = cpk.to_bytes().expect("writes with non-default schema");
        let parsed = Cpk::parse(&bytes).expect("re-parses");

        // Schema preserved exactly.
        assert_eq!(parsed.toc_columns, cpk.toc_columns);

        // File bodies preserved.
        for (got, want) in parsed.files.iter().zip(cpk.files.iter()) {
            assert_eq!(got.file_name, want.file_name);
            assert_eq!(got.dir_name, want.dir_name);
            assert_eq!(got.data, want.data);
            assert_eq!(got.id, want.id);
            assert_eq!(
                got.extra.get("CRC"),
                want.extra.get("CRC"),
                "CRC extra column for {:?}",
                want.file_name,
            );
        }

        // FileSize was u64 in the schema; ensure it round-tripped as u64
        // (the parser stores it in `extra` since FileSize lives in dedicated
        // `CpkFile.data.len()`, but the schema column type is what matters).
        let file_size_col = parsed
            .toc_columns
            .iter()
            .find(|c| c.name.as_deref() == Some("FileSize"))
            .expect("FileSize column survives");
        assert_eq!(file_size_col.ty, UtfType::U64);
    }

    /// A CRILAYLA-compressed entry must be rejected. The writer derives both
    /// `FileSize` and `ExtractSize` from `data.len()`, so it never emits
    /// `FileSize != ExtractSize` itself — such an entry only arrives from a
    /// foreign packer. Forge one: rewrite the TOC so row 0's `ExtractSize`
    /// differs from its `FileSize`, then stamp `CRILAYLA` magic over its body.
    #[test]
    fn rejects_crilayla_entries() {
        let mut bytes = sample_cpk().to_bytes().unwrap();
        let parsed = Cpk::parse(&bytes).unwrap();
        let content_offset = parsed
            .header_row
            .get("ContentOffset")
            .and_then(UtfValue::as_u64)
            .unwrap() as usize;
        let toc_offset = parsed
            .header_row
            .get("TocOffset")
            .and_then(UtfValue::as_u64)
            .unwrap() as usize;

        forge_extract_size_mismatch(&mut bytes, toc_offset);
        bytes[content_offset..content_offset + 8].copy_from_slice(b"CRILAYLA");

        let err = Cpk::parse(&bytes).unwrap_err();
        assert!(matches!(err, CpkParseError::Crilayla(_)));
    }

    /// The same `FileSize != ExtractSize` divergence with an ordinary
    /// (non-CRILAYLA) body is a plain malformed error, not a CRILAYLA reject —
    /// the two branches are distinct.
    #[test]
    fn size_mismatch_without_crilayla_magic_is_malformed() {
        let mut bytes = sample_cpk().to_bytes().unwrap();
        let toc_offset = Cpk::parse(&bytes)
            .unwrap()
            .header_row
            .get("TocOffset")
            .and_then(UtfValue::as_u64)
            .unwrap() as usize;

        forge_extract_size_mismatch(&mut bytes, toc_offset);

        let err = Cpk::parse(&bytes).unwrap_err();
        assert!(matches!(
            err,
            CpkParseError::Bin(BinError::Malformed { .. })
        ));
    }

    /// Rewrite the TOC packet in `bytes` so the first file row's `ExtractSize`
    /// no longer equals its `FileSize`. Only a fixed-width cell value changes,
    /// so the re-serialized TOC is the same length and the splice stays aligned.
    fn forge_extract_size_mismatch(bytes: &mut [u8], toc_offset: usize) {
        let new_toc = {
            let mut r = Reader::new(bytes);
            r.seek(toc_offset).unwrap();
            let mut toc = read_packet(&mut r, MAGIC_TOC).unwrap();
            toc.rows[0].insert("ExtractSize".into(), UtfValue::U32(0xDEAD));
            toc.to_bytes().unwrap()
        };
        let toc_payload = toc_offset + PACKET_WRAPPER_SIZE as usize;
        bytes[toc_payload..toc_payload + new_toc.len()].copy_from_slice(&new_toc);
    }
}
