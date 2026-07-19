//! Danganronpa V3 DAT typed-binary-table reader/writer.
//!
//! DAT is a typed columnar table. The schema declares a sequence of named
//! columns, each with one of 14 cell types (signed/unsigned integers, two
//! float widths, plus four string variants — `ascii`, `label`, `refer` in
//! UTF-8 and `utf16` in UTF-16 LE). The on-disk layout is:
//!
//! ```text
//! offset 0x00  4 bytes   row_count u32 LE
//! offset 0x04  4 bytes   bytes_per_row u32 LE
//! offset 0x08  …         row data (row_count × bytes_per_row), columns laid out
//!                        in declaration order with each cell sized per its type
//! …            …         column-schema section: per column (name UTF-8 cstring,
//!                        column-type tag UTF-8 cstring, count u16 LE)
//! …            …         UTF-8 string pool, deduplicated, offset 0 is ""
//! …            …         UTF-16 LE string pool, deduplicated, offset 0 is ""
//! ```
//!
//! Cells of the string-type columns store a `u16 LE` index into the
//! corresponding pool (UTF-8 for `ascii`/`label`/`refer`, UTF-16 for
//! `utf16`). Index 0 is always the empty string.
//!
//! ## Round-trip
//!
//! [`Dat::to_bytes`] reproduces the original file *semantically* —
//! re-parsing the output yields an equal [`Dat`]. Byte-level round-trip
//! additionally requires the original file to have built its string pools
//! in the same row/column/value iteration order our writer uses; real DR V3
//! files do.
//!
//! Float columns use bitwise equality for [`PartialEq`] — `NaN`s round-trip
//! but compare unequal as in standard Rust.
//!
//! Fallible operations surface [`drv3_binio::BinResult`] directly; this crate
//! defines no error type of its own.

use std::collections::HashMap;

use drv3_binio::{BinError, BinResult, Reader, Writer};

const MAX_ROW_COUNT: u32 = 1_000_000;
const MAX_BYTES_PER_ROW: u32 = 100_000;
const MAX_COLUMN_COUNT: u32 = 256;

/// Parsed DAT file.
#[derive(Debug, Clone, PartialEq)]
pub struct Dat {
    pub schema: Vec<Column>,
    pub rows: Vec<Row>,
}

/// A single column descriptor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Column {
    pub name: String,
    pub ty: ColumnType,
    /// Number of values per row in this column (≥1; > 1 = fixed-size array).
    pub count: u16,
}

/// Type tag for a column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ColumnType {
    U8,
    U16,
    U32,
    U64,
    S8,
    S16,
    S32,
    S64,
    F32,
    F64,
    /// UTF-8 pool index, freeform string.
    Ascii,
    /// UTF-8 pool index, label/identifier semantics.
    Label,
    /// UTF-8 pool index, cross-reference semantics.
    Refer,
    /// UTF-16 pool index.
    Utf16,
}

impl ColumnType {
    /// Parse a column type from its on-disk UTF-8 tag (`"u8"`, `"ascii"`, …),
    /// or `None` if the tag is unrecognized. This is the canonical tag mapping;
    /// the CLI JSON `type` field reuses it so the two never drift.
    pub fn from_tag(tag: &str) -> Option<Self> {
        Some(match tag {
            "u8" => Self::U8,
            "u16" => Self::U16,
            "u32" => Self::U32,
            "u64" => Self::U64,
            "s8" => Self::S8,
            "s16" => Self::S16,
            "s32" => Self::S32,
            "s64" => Self::S64,
            "f32" => Self::F32,
            "f64" => Self::F64,
            "ascii" => Self::Ascii,
            "label" => Self::Label,
            "refer" => Self::Refer,
            "utf16" => Self::Utf16,
            _ => return None,
        })
    }

    /// The on-disk UTF-8 tag for this column type (`"u8"`, `"ascii"`, …).
    pub fn tag(self) -> &'static str {
        match self {
            Self::U8 => "u8",
            Self::U16 => "u16",
            Self::U32 => "u32",
            Self::U64 => "u64",
            Self::S8 => "s8",
            Self::S16 => "s16",
            Self::S32 => "s32",
            Self::S64 => "s64",
            Self::F32 => "f32",
            Self::F64 => "f64",
            Self::Ascii => "ascii",
            Self::Label => "label",
            Self::Refer => "refer",
            Self::Utf16 => "utf16",
        }
    }

    /// Width in bytes of one value of this type as stored in row data.
    fn value_width(self) -> usize {
        match self {
            Self::U8 | Self::S8 => 1,
            Self::U16 | Self::S16 | Self::Ascii | Self::Label | Self::Refer | Self::Utf16 => 2,
            Self::U32 | Self::S32 | Self::F32 => 4,
            Self::U64 | Self::S64 | Self::F64 => 8,
        }
    }
}

pub type Row = Vec<Cell>;

/// A single cell — one variant per [`ColumnType`], holding `column.count` values.
#[derive(Debug, Clone, PartialEq)]
pub enum Cell {
    U8(Vec<u8>),
    U16(Vec<u16>),
    U32(Vec<u32>),
    U64(Vec<u64>),
    S8(Vec<i8>),
    S16(Vec<i16>),
    S32(Vec<i32>),
    S64(Vec<i64>),
    F32(Vec<f32>),
    F64(Vec<f64>),
    Ascii(Vec<String>),
    Label(Vec<String>),
    Refer(Vec<String>),
    Utf16(Vec<String>),
}

impl Cell {
    fn ty(&self) -> ColumnType {
        match self {
            Self::U8(_) => ColumnType::U8,
            Self::U16(_) => ColumnType::U16,
            Self::U32(_) => ColumnType::U32,
            Self::U64(_) => ColumnType::U64,
            Self::S8(_) => ColumnType::S8,
            Self::S16(_) => ColumnType::S16,
            Self::S32(_) => ColumnType::S32,
            Self::S64(_) => ColumnType::S64,
            Self::F32(_) => ColumnType::F32,
            Self::F64(_) => ColumnType::F64,
            Self::Ascii(_) => ColumnType::Ascii,
            Self::Label(_) => ColumnType::Label,
            Self::Refer(_) => ColumnType::Refer,
            Self::Utf16(_) => ColumnType::Utf16,
        }
    }

    fn len(&self) -> usize {
        match self {
            Self::U8(v) => v.len(),
            Self::U16(v) => v.len(),
            Self::U32(v) => v.len(),
            Self::U64(v) => v.len(),
            Self::S8(v) => v.len(),
            Self::S16(v) => v.len(),
            Self::S32(v) => v.len(),
            Self::S64(v) => v.len(),
            Self::F32(v) => v.len(),
            Self::F64(v) => v.len(),
            Self::Ascii(v) | Self::Label(v) | Self::Refer(v) | Self::Utf16(v) => v.len(),
        }
    }
}

impl Dat {
    /// Parse a DAT file from a byte buffer.
    ///
    /// Up-front sanity bounds are applied to the header counts so a file
    /// that happens to share the `.dat` extension but isn't a DR V3 DAT
    /// fails fast: `row_count` and `bytes_per_row` must each be non-zero
    /// and below `MAX_ROW_COUNT` / `MAX_BYTES_PER_ROW`; `column_count` is
    /// capped at `MAX_COLUMN_COUNT`. These thresholds are conservatively
    /// above anything observed in the game data.
    ///
    /// # Errors
    ///
    /// Returns an error if the header counts are zero or exceed their
    /// sanity bounds, the row data is shorter than the declared total,
    /// the schema names a column type the parser doesn't recognize, or
    /// any string-pool index points past the end of its pool.
    pub fn parse(input: &[u8]) -> BinResult<Self> {
        let mut r = Reader::new(input);
        let row_count = r.u32_le()?;
        let bytes_per_row = r.u32_le()?;
        let column_count = r.u32_le()?;

        if row_count == 0 || row_count > MAX_ROW_COUNT {
            return Err(BinError::malformed(
                0,
                format!("row_count {row_count} out of range"),
            ));
        }
        if bytes_per_row == 0 || bytes_per_row > MAX_BYTES_PER_ROW {
            return Err(BinError::malformed(
                4,
                format!("bytes_per_row {bytes_per_row} out of range"),
            ));
        }
        if column_count == 0 || column_count > MAX_COLUMN_COUNT {
            return Err(BinError::malformed(
                8,
                format!("column_count {column_count} out of range"),
            ));
        }

        // Schema.
        let mut schema: Vec<Column> = Vec::with_capacity(column_count as usize);
        for _ in 0..column_count {
            let name = r.read_utf8_cstring()?;
            let type_tag = r.read_utf8_cstring()?;
            let count = r.u16_le()?;
            let ty = ColumnType::from_tag(&type_tag).ok_or_else(|| {
                BinError::malformed(
                    r.position(),
                    format!("unknown column type tag {type_tag:?}"),
                )
            })?;
            schema.push(Column { name, ty, count });
        }

        // The declared row stride must equal the summed column widths;
        // otherwise the row reader (which advances per column) would drift out
        // of step with the pools located via `row_count * bytes_per_row`.
        let schema_width: usize = schema
            .iter()
            .map(|c| c.ty.value_width() * c.count as usize)
            .sum();
        if schema_width != bytes_per_row as usize {
            return Err(BinError::malformed(
                4,
                format!("bytes_per_row {bytes_per_row} disagrees with schema width {schema_width}"),
            ));
        }

        r.align_to(16)?;

        let row_data_pos = r.position();
        let row_data_end = row_data_pos
            .checked_add((row_count as usize) * (bytes_per_row as usize))
            .ok_or_else(|| BinError::malformed(row_data_pos, "row data overflow"))?;

        // Read string pools first — they live after row data.
        r.seek(row_data_end)?;
        let utf8_count = r.u16_le()? as usize;
        let utf16_count = r.u16_le()? as usize;
        let mut utf8_pool: Vec<String> = Vec::with_capacity(r.capacity_hint(utf8_count, 1));
        for _ in 0..utf8_count {
            utf8_pool.push(r.read_utf8_cstring()?);
        }
        r.align_to(2)?;
        let mut utf16_pool: Vec<String> = Vec::with_capacity(r.capacity_hint(utf16_count, 2));
        for _ in 0..utf16_count {
            utf16_pool.push(r.read_utf16le_cstring()?);
        }

        // Now read row data.
        r.seek(row_data_pos)?;
        let mut rows: Vec<Row> = Vec::with_capacity(row_count as usize);
        for _ in 0..row_count {
            let mut row: Row = Vec::with_capacity(schema.len());
            for col in &schema {
                let cell = read_cell(&mut r, col, &utf8_pool, &utf16_pool)?;
                row.push(cell);
            }
            rows.push(row);
        }

        Ok(Self { schema, rows })
    }

    /// Encode a DAT file to a byte vector.
    ///
    /// # Errors
    ///
    /// Returns an error if a cell's variant doesn't match its column's
    /// declared type, or any cell's value count differs from its column's
    /// `count` field.
    pub fn to_bytes(&self) -> BinResult<Vec<u8>> {
        // Validate schema/rows shape.
        for (row_idx, row) in self.rows.iter().enumerate() {
            if row.len() != self.schema.len() {
                return Err(BinError::malformed(
                    0,
                    format!(
                        "row {} has {} cells, expected {}",
                        row_idx,
                        row.len(),
                        self.schema.len()
                    ),
                ));
            }
            for (col_idx, (col, cell)) in self.schema.iter().zip(row.iter()).enumerate() {
                if cell.ty() != col.ty {
                    return Err(BinError::malformed(
                        0,
                        format!(
                            "row {row_idx} col {col_idx}: cell type {:?} does not match column type {:?}",
                            cell.ty(),
                            col.ty
                        ),
                    ));
                }
                if cell.len() != col.count as usize {
                    return Err(BinError::malformed(
                        0,
                        format!(
                            "row {row_idx} col {col_idx}: cell has {} values, column declares {}",
                            cell.len(),
                            col.count
                        ),
                    ));
                }
            }
        }

        let bytes_per_row: u32 = self
            .schema
            .iter()
            .map(|c| (c.ty.value_width() as u32) * u32::from(c.count))
            .sum();

        let mut w = Writer::new();
        w.write_u32_le(self.rows.len() as u32);
        w.write_u32_le(bytes_per_row);
        w.write_u32_le(self.schema.len() as u32);

        // Schema.
        for col in &self.schema {
            w.write_utf8_cstring(&col.name);
            w.write_utf8_cstring(col.ty.tag());
            w.write_u16_le(col.count);
        }
        w.pad_to(16, 0);

        // Row data + pool construction.
        let mut utf8_pool: Vec<String> = Vec::new();
        let mut utf8_idx: HashMap<String, u16> = HashMap::new();
        let mut utf16_pool: Vec<String> = Vec::new();
        let mut utf16_idx: HashMap<String, u16> = HashMap::new();

        for row in &self.rows {
            for cell in row {
                write_cell(
                    &mut w,
                    cell,
                    &mut utf8_pool,
                    &mut utf8_idx,
                    &mut utf16_pool,
                    &mut utf16_idx,
                )?;
            }
        }

        // String pools.
        w.write_u16_le(utf8_pool.len() as u16);
        w.write_u16_le(utf16_pool.len() as u16);
        for s in &utf8_pool {
            w.write_utf8_cstring(s);
        }
        w.pad_to(2, 0);
        for s in &utf16_pool {
            w.write_utf16le_cstring(s);
        }

        Ok(w.into_inner())
    }
}

fn read_cell(
    r: &mut Reader<'_>,
    col: &Column,
    utf8_pool: &[String],
    utf16_pool: &[String],
) -> BinResult<Cell> {
    let n = col.count as usize;
    Ok(match col.ty {
        ColumnType::U8 => Cell::U8((0..n).map(|_| r.u8()).collect::<BinResult<_>>()?),
        ColumnType::U16 => Cell::U16((0..n).map(|_| r.u16_le()).collect::<BinResult<_>>()?),
        ColumnType::U32 => Cell::U32((0..n).map(|_| r.u32_le()).collect::<BinResult<_>>()?),
        ColumnType::U64 => Cell::U64((0..n).map(|_| r.u64_le()).collect::<BinResult<_>>()?),
        ColumnType::S8 => Cell::S8((0..n).map(|_| r.i8()).collect::<BinResult<_>>()?),
        ColumnType::S16 => Cell::S16((0..n).map(|_| r.i16_le()).collect::<BinResult<_>>()?),
        ColumnType::S32 => Cell::S32((0..n).map(|_| r.i32_le()).collect::<BinResult<_>>()?),
        ColumnType::S64 => Cell::S64((0..n).map(|_| r.i64_le()).collect::<BinResult<_>>()?),
        ColumnType::F32 => Cell::F32((0..n).map(|_| r.f32_le()).collect::<BinResult<_>>()?),
        ColumnType::F64 => Cell::F64((0..n).map(|_| r.f64_le()).collect::<BinResult<_>>()?),
        ColumnType::Ascii | ColumnType::Label | ColumnType::Refer => {
            let mut strings = Vec::with_capacity(n);
            for _ in 0..n {
                let idx = r.u16_le()? as usize;
                let s = utf8_pool.get(idx).cloned().ok_or_else(|| {
                    BinError::malformed(r.position(), format!("utf8 pool index {idx} out of range"))
                })?;
                strings.push(s);
            }
            match col.ty {
                ColumnType::Ascii => Cell::Ascii(strings),
                ColumnType::Label => Cell::Label(strings),
                ColumnType::Refer => Cell::Refer(strings),
                _ => unreachable!(),
            }
        }
        ColumnType::Utf16 => {
            let mut strings = Vec::with_capacity(n);
            for _ in 0..n {
                let idx = r.u16_le()? as usize;
                let s = utf16_pool.get(idx).cloned().ok_or_else(|| {
                    BinError::malformed(
                        r.position(),
                        format!("utf16 pool index {idx} out of range"),
                    )
                })?;
                strings.push(s);
            }
            Cell::Utf16(strings)
        }
    })
}

fn write_cell(
    w: &mut Writer,
    cell: &Cell,
    utf8_pool: &mut Vec<String>,
    utf8_idx: &mut HashMap<String, u16>,
    utf16_pool: &mut Vec<String>,
    utf16_idx: &mut HashMap<String, u16>,
) -> BinResult<()> {
    match cell {
        Cell::U8(v) => v.iter().for_each(|&x| w.write_u8(x)),
        Cell::U16(v) => v.iter().for_each(|&x| w.write_u16_le(x)),
        Cell::U32(v) => v.iter().for_each(|&x| w.write_u32_le(x)),
        Cell::U64(v) => v.iter().for_each(|&x| w.write_u64_le(x)),
        Cell::S8(v) => v.iter().for_each(|&x| w.write_i8(x)),
        Cell::S16(v) => v.iter().for_each(|&x| w.write_i16_le(x)),
        Cell::S32(v) => v.iter().for_each(|&x| w.write_i32_le(x)),
        Cell::S64(v) => v.iter().for_each(|&x| w.write_i64_le(x)),
        Cell::F32(v) => v.iter().for_each(|&x| w.write_f32_le(x)),
        Cell::F64(v) => v.iter().for_each(|&x| w.write_f64_le(x)),
        Cell::Ascii(v) | Cell::Label(v) | Cell::Refer(v) => {
            for s in v {
                let idx = pool_intern(utf8_pool, utf8_idx, s)?;
                w.write_u16_le(idx);
            }
        }
        Cell::Utf16(v) => {
            for s in v {
                let idx = pool_intern(utf16_pool, utf16_idx, s)?;
                w.write_u16_le(idx);
            }
        }
    }
    Ok(())
}

fn pool_intern(pool: &mut Vec<String>, idx: &mut HashMap<String, u16>, s: &str) -> BinResult<u16> {
    if let Some(&i) = idx.get(s) {
        return Ok(i);
    }
    let new_idx = pool.len();
    if new_idx > u16::MAX as usize {
        return Err(BinError::malformed(0, "string pool exceeds u16 capacity"));
    }
    pool.push(s.to_string());
    let new_idx = new_idx as u16;
    idx.insert(s.to_string(), new_idx);
    Ok(new_idx)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Dat {
        Dat {
            schema: vec![
                Column {
                    name: "id".into(),
                    ty: ColumnType::U32,
                    count: 1,
                },
                Column {
                    name: "name".into(),
                    ty: ColumnType::Utf16,
                    count: 1,
                },
                Column {
                    name: "tags".into(),
                    ty: ColumnType::Utf16,
                    count: 3,
                },
                Column {
                    name: "key".into(),
                    ty: ColumnType::Label,
                    count: 1,
                },
                Column {
                    name: "weight".into(),
                    ty: ColumnType::F32,
                    count: 1,
                },
            ],
            rows: vec![
                vec![
                    Cell::U32(vec![1]),
                    Cell::Utf16(vec!["Alice".into()]),
                    Cell::Utf16(vec!["red".into(), "tall".into(), "kind".into()]),
                    Cell::Label(vec!["chr_alice".into()]),
                    Cell::F32(vec![1.25]),
                ],
                vec![
                    Cell::U32(vec![2]),
                    Cell::Utf16(vec!["Bob".into()]),
                    Cell::Utf16(vec!["blue".into(), "tall".into(), "red".into()]), // dups: "tall", "red"
                    Cell::Label(vec!["chr_bob".into()]),
                    Cell::F32(vec![1.50]),
                ],
            ],
        }
    }

    #[test]
    fn semantic_round_trip() {
        let dat = sample();
        let bytes = dat.to_bytes().unwrap();
        let parsed = Dat::parse(&bytes).unwrap();
        assert_eq!(parsed, dat);
    }

    #[test]
    fn byte_level_round_trip_is_stable() {
        let bytes1 = sample().to_bytes().unwrap();
        let parsed = Dat::parse(&bytes1).unwrap();
        let bytes2 = parsed.to_bytes().unwrap();
        assert_eq!(bytes1, bytes2);
    }

    #[test]
    fn string_pool_dedup() {
        // Build two rows sharing identical strings; confirm the file is
        // smaller than if pools were not deduplicated.
        let sample_dat = sample();
        let bytes_dedup = sample_dat.to_bytes().unwrap();

        // Construct an inflated reference where every cell uses a unique string.
        let mut inflated = sample_dat.clone();
        for (row_idx, row) in inflated.rows.iter_mut().enumerate() {
            for cell in row.iter_mut() {
                if let Cell::Utf16(strings) = cell {
                    use std::fmt::Write;
                    for s in strings.iter_mut() {
                        write!(s, "_r{row_idx}").unwrap();
                    }
                }
            }
        }
        let bytes_inflated = inflated.to_bytes().unwrap();
        assert!(bytes_dedup.len() < bytes_inflated.len());

        // And the deduped file still round-trips.
        assert_eq!(Dat::parse(&bytes_dedup).unwrap(), sample_dat);
    }

    #[test]
    fn rejects_implausible_header() {
        let mut bytes = sample().to_bytes().unwrap();
        // Zero out row_count.
        bytes[0..4].copy_from_slice(&0u32.to_le_bytes());
        let err = Dat::parse(&bytes).unwrap_err();
        assert!(matches!(err, BinError::Malformed { .. }));
    }

    #[test]
    fn schema_mismatch_errors() {
        let mut dat = sample();
        // Wrong cell type for column 0 (declared U32).
        dat.rows[0][0] = Cell::U8(vec![1]);
        let err = dat.to_bytes().unwrap_err();
        assert!(matches!(err, BinError::Malformed { .. }));
    }

    #[test]
    fn bytes_per_row_disagreeing_with_schema_errors() {
        let mut bytes = sample().to_bytes().unwrap();
        // bytes_per_row is the u32 LE at offset 0x04. Bumping it by one keeps
        // it in the plausible range but no longer matches the summed column
        // widths, which must be rejected rather than silently misparsed.
        let real = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
        bytes[4..8].copy_from_slice(&(real + 1).to_le_bytes());
        let err = Dat::parse(&bytes).unwrap_err();
        assert!(matches!(err, BinError::Malformed { .. }));
    }

    #[test]
    fn count_mismatch_errors() {
        let mut dat = sample();
        // tags column declares count=3, give 2.
        dat.rows[0][2] = Cell::Utf16(vec!["a".into(), "b".into()]);
        let err = dat.to_bytes().unwrap_err();
        assert!(matches!(err, BinError::Malformed { .. }));
    }

    #[test]
    fn handles_all_numeric_types() {
        let dat = Dat {
            schema: vec![
                Column {
                    name: "a".into(),
                    ty: ColumnType::U8,
                    count: 1,
                },
                Column {
                    name: "b".into(),
                    ty: ColumnType::U16,
                    count: 1,
                },
                Column {
                    name: "c".into(),
                    ty: ColumnType::U64,
                    count: 1,
                },
                Column {
                    name: "d".into(),
                    ty: ColumnType::S8,
                    count: 1,
                },
                Column {
                    name: "e".into(),
                    ty: ColumnType::S16,
                    count: 1,
                },
                Column {
                    name: "f".into(),
                    ty: ColumnType::S64,
                    count: 1,
                },
                Column {
                    name: "g".into(),
                    ty: ColumnType::F64,
                    count: 1,
                },
            ],
            rows: vec![vec![
                Cell::U8(vec![0xFF]),
                Cell::U16(vec![0xBEEF]),
                Cell::U64(vec![0xDEAD_BEEF_DEAD_BEEF]),
                Cell::S8(vec![-1]),
                Cell::S16(vec![-2]),
                Cell::S64(vec![-3]),
                Cell::F64(vec![std::f64::consts::PI]),
            ]],
        };
        let bytes = dat.to_bytes().unwrap();
        let parsed = Dat::parse(&bytes).unwrap();
        assert_eq!(parsed, dat);
    }
}
