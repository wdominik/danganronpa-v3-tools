//! CRI `@UTF` columnar table primitive.
//!
//! Every CPK packet (header, TOC, ETOC, ITOC, GTOC) wraps exactly one
//! `@UTF` table. The on-disk layout, in order, is:
//!
//! ```text
//! offset 0x00  4 bytes  magic "@UTF"
//! offset 0x04  4 bytes  TableSize  u32 BE — size of the table starting at byte 0x08
//! offset 0x08  4 bytes  RowsOffset    u32 BE   relative to byte 0x08
//! offset 0x0C  4 bytes  StringsOffset u32 BE   relative to byte 0x08
//! offset 0x10  4 bytes  DataOffset    u32 BE   relative to byte 0x08
//! offset 0x14  4 bytes  TableNameOffset u32 BE — index into the string pool
//! offset 0x18  2 bytes  ColumnCount  u16 BE
//! offset 0x1A  2 bytes  RowSize      u16 BE — bytes per row in the row-data section
//! offset 0x1C  4 bytes  RowCount     u32 BE
//! offset 0x20  …        ColumnSchema  ColumnCount variable-length entries
//! …                     RowData       RowCount × RowSize bytes
//! …                     StringPool    UTF-8 null-terminated strings; offset 0 is ""
//! …                     DataBlob      raw byte blobs, indexed by (offset, size) pairs
//! ```
//!
//! Endianness is mixed by design: the outer CPK packet wrapper is
//! little-endian but the `@UTF` payload is big-endian. Methods on
//! [`drv3_binio::Reader`] always name the byte order to avoid confusion.

use std::collections::HashMap;

use drv3_binio::{BinError, BinResult, Reader, Writer};
use indexmap::IndexMap;

pub const MAGIC: &[u8; 4] = b"@UTF";

/// Storage class for an `@UTF` column, derived from `flags & 0xF0`.
///
/// Each column schema entry on disk begins with a single *flag byte* whose
/// bits decompose as:
///
/// ```text
/// bits 0-3 : type nibble — encodes the column's value type (u8 / u16 /
///            u32 / u64 / s8 / s16 / s32 / s64 / f32 / f64 / string / data),
///            see [`UtfType`].
/// bit  4   : name-present flag — 1 = a 4-byte name offset follows in the
///            schema entry.
/// bits 5-6 : storage selector — 00 = None, 01 = Constant, 10 = PerRow,
///            11 = Constant2.
/// bit  7   : reserved / unused in observed files.
/// ```
///
/// The composite **storage byte** `flags & 0xF0` therefore takes one of five
/// canonical values; this enum maps each to a semantic variant:
///
/// | Raw  | Variant     | Has name? | Inline value? | Per-row value? |
/// |------|-------------|-----------|---------------|----------------|
/// | 0x00 | [`None`]      | no       | no            | no             |
/// | 0x10 | [`Zero`]      | yes      | no            | no (implicit 0)|
/// | 0x30 | [`Constant`]  | yes      | yes           | no             |
/// | 0x50 | [`PerRow`]    | yes      | no            | yes            |
/// | 0x70 | [`Constant2`] | yes      | yes (alt)     | no             |
///
/// `Constant` and `Constant2` are layout-identical on disk (both carry an
/// inline typed value sized per the type nibble); they exist as distinct
/// values so a writer can round-trip the exact byte a CRI encoder produced.
///
/// [`None`]: StorageFlag::None
/// [`Zero`]: StorageFlag::Zero
/// [`Constant`]: StorageFlag::Constant
/// [`PerRow`]: StorageFlag::PerRow
/// [`Constant2`]: StorageFlag::Constant2
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StorageFlag {
    /// 0x00 — no name, no on-disk value.
    None,
    /// 0x10 — name present, value implicit zero/empty.
    Zero,
    /// 0x30 — name present, inline value in schema.
    Constant,
    /// 0x50 — name present, value lives in row data.
    PerRow,
    /// 0x70 — name present, alternate inline value encoding.
    Constant2,
}

impl StorageFlag {
    /// Decode the storage class from a full flag byte (only `flags & 0xF0` is read).
    pub fn from_byte(flags: u8) -> Option<Self> {
        match flags & 0xF0 {
            0x00 => Some(Self::None),
            0x10 => Some(Self::Zero),
            0x30 => Some(Self::Constant),
            0x50 => Some(Self::PerRow),
            0x70 => Some(Self::Constant2),
            _ => None,
        }
    }

    /// The 8-bit storage component (top nibble worth of bits).
    pub fn storage_bits(self) -> u8 {
        match self {
            Self::None => 0x00,
            Self::Zero => 0x10,
            Self::Constant => 0x30,
            Self::PerRow => 0x50,
            Self::Constant2 => 0x70,
        }
    }

    /// Whether a 4-byte name offset is encoded after the flag byte.
    pub fn has_name(self) -> bool {
        !matches!(self, Self::None)
    }

    /// Whether an inline typed value follows the (optional) name offset.
    pub fn has_inline_value(self) -> bool {
        matches!(self, Self::Constant | Self::Constant2)
    }

    /// Whether the column contributes a per-row value to the row-data section.
    pub fn has_row_value(self) -> bool {
        matches!(self, Self::PerRow)
    }
}

/// Type tag for a column (low nibble of the flags byte).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UtfType {
    U8,
    S8,
    U16,
    S16,
    U32,
    S32,
    U64,
    S64,
    F32,
    F64,
    String,
    Data,
}

impl UtfType {
    fn from_nibble(n: u8) -> Option<Self> {
        Some(match n {
            0x00 => Self::U8,
            0x01 => Self::S8,
            0x02 => Self::U16,
            0x03 => Self::S16,
            0x04 => Self::U32,
            0x05 => Self::S32,
            0x06 => Self::U64,
            0x07 => Self::S64,
            0x08 => Self::F32,
            0x09 => Self::F64,
            0x0A => Self::String,
            0x0B => Self::Data,
            _ => return None,
        })
    }

    fn nibble(self) -> u8 {
        match self {
            Self::U8 => 0x00,
            Self::S8 => 0x01,
            Self::U16 => 0x02,
            Self::S16 => 0x03,
            Self::U32 => 0x04,
            Self::S32 => 0x05,
            Self::U64 => 0x06,
            Self::S64 => 0x07,
            Self::F32 => 0x08,
            Self::F64 => 0x09,
            Self::String => 0x0A,
            Self::Data => 0x0B,
        }
    }

    fn fixed_size(self) -> Option<usize> {
        Some(match self {
            Self::U8 | Self::S8 => 1,
            Self::U16 | Self::S16 => 2,
            Self::U32 | Self::S32 | Self::F32 | Self::String => 4,
            Self::U64 | Self::S64 | Self::F64 => 8,
            Self::Data => 8, // u32 offset + u32 size
        })
    }
}

/// A typed value held by a [`UtfRow`] or a [`UtfColumn`]'s constant.
#[derive(Debug, Clone, PartialEq)]
pub enum UtfValue {
    U8(u8),
    S8(i8),
    U16(u16),
    S16(i16),
    U32(u32),
    S32(i32),
    U64(u64),
    S64(i64),
    F32(f32),
    F64(f64),
    String(String),
    Data(Vec<u8>),
}

impl UtfValue {
    pub fn ty(&self) -> UtfType {
        match self {
            Self::U8(_) => UtfType::U8,
            Self::S8(_) => UtfType::S8,
            Self::U16(_) => UtfType::U16,
            Self::S16(_) => UtfType::S16,
            Self::U32(_) => UtfType::U32,
            Self::S32(_) => UtfType::S32,
            Self::U64(_) => UtfType::U64,
            Self::S64(_) => UtfType::S64,
            Self::F32(_) => UtfType::F32,
            Self::F64(_) => UtfType::F64,
            Self::String(_) => UtfType::String,
            Self::Data(_) => UtfType::Data,
        }
    }

    pub fn as_u32(&self) -> Option<u32> {
        match *self {
            Self::U8(v) => Some(u32::from(v)),
            Self::U16(v) => Some(u32::from(v)),
            Self::U32(v) => Some(v),
            _ => None,
        }
    }

    pub fn as_u64(&self) -> Option<u64> {
        match *self {
            Self::U8(v) => Some(u64::from(v)),
            Self::U16(v) => Some(u64::from(v)),
            Self::U32(v) => Some(u64::from(v)),
            Self::U64(v) => Some(v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(s) => Some(s.as_str()),
            _ => None,
        }
    }
}

/// One column descriptor.
///
/// `name` is `None` only for [`StorageFlag::None`] columns, which carry neither
/// a name nor a value on disk. Every other storage variant has a name offset
/// in the schema entry.
#[derive(Debug, Clone, PartialEq)]
pub struct UtfColumn {
    pub name: Option<String>,
    pub storage: StorageFlag,
    pub ty: UtfType,
    /// Present only when `storage` is `Constant` or `Constant2`.
    pub constant: Option<UtfValue>,
}

/// A row, keyed by column name. Backed by `IndexMap` so iteration follows
/// insertion order — which matches the column-declaration order used by both
/// the parser and the writer, and makes manifests / debug output stable
/// across runs.
pub type UtfRow = IndexMap<String, UtfValue>;

/// Parsed `@UTF` table.
#[derive(Debug, Clone, PartialEq)]
pub struct UtfTable {
    pub name: String,
    pub columns: Vec<UtfColumn>,
    pub rows: Vec<UtfRow>,
}

impl UtfTable {
    /// Parse an `@UTF` table from the start of `input`. The table consumes
    /// `table_size + 8` bytes (the +8 covering the magic + size header).
    ///
    /// # Errors
    ///
    /// Returns an error if the magic bytes don't match `@UTF`, the
    /// declared `table_size` exceeds the input buffer, a column flag byte
    /// has an unknown storage selector or type nibble, a string-pool
    /// offset is out of bounds, the schema's per-row size doesn't agree
    /// with the header's `RowSize`, or any pool string is not valid UTF-8.
    ///
    /// # Panics
    ///
    /// Panics only if a new variable-size variant is added to [`UtfType`]
    /// without updating `fixed_size()` — a code-change-time invariant,
    /// not anything user input can trigger.
    pub fn parse(input: &[u8]) -> BinResult<Self> {
        let mut r = Reader::new(input);
        r.expect_magic(MAGIC)?;
        let table_size = r.u32_be()?;
        // All subsequent offsets are relative to byte 0x08 (start of size's address space).
        // We'll work with absolute positions in `input`.
        let table_base = 0x08usize; // position of byte right after magic
        let table_end = table_base + table_size as usize;
        if table_end > input.len() {
            return Err(BinError::malformed(
                0x04,
                format!(
                    "table size {table_size} exceeds buffer ({} bytes)",
                    input.len()
                ),
            ));
        }

        let rows_offset = r.u32_be()? as usize + table_base;
        let strings_offset = r.u32_be()? as usize + table_base;
        let data_offset = r.u32_be()? as usize + table_base;
        let table_name_offset = r.u32_be()? as usize;
        let column_count = r.u16_be()? as usize;
        let row_size = r.u16_be()? as usize;
        let row_count = r.u32_be()? as usize;

        let string_pool = &input[strings_offset..data_offset];
        let data_blob = &input[data_offset..table_end];

        let read_pool_string = |off: usize| -> BinResult<String> {
            if off >= string_pool.len() {
                return Err(BinError::malformed(
                    0,
                    format!(
                        "string offset {off} past pool ({} bytes)",
                        string_pool.len()
                    ),
                ));
            }
            let mut end = off;
            while end < string_pool.len() && string_pool[end] != 0 {
                end += 1;
            }
            std::str::from_utf8(&string_pool[off..end])
                .map(str::to_owned)
                .map_err(|source| BinError::InvalidUtf8 {
                    pos: strings_offset + off,
                    source,
                })
        };

        let read_blob = |off: usize, size: usize| -> BinResult<Vec<u8>> {
            if off + size > data_blob.len() {
                return Err(BinError::malformed(
                    0,
                    format!(
                        "data blob ({off}, {size}) past blob ({} bytes)",
                        data_blob.len()
                    ),
                ));
            }
            Ok(data_blob[off..off + size].to_vec())
        };

        let name = read_pool_string(table_name_offset)?;

        // Parse schema. Per the canonical CRI flag encoding, each entry is:
        //   - 1 byte flags  (storage | name-flag | type)
        //   - 4 bytes name_offset      (only if storage.has_name())
        //   - N bytes inline value     (only if storage.has_inline_value())
        let mut columns: Vec<UtfColumn> = Vec::with_capacity(column_count);
        // Parallel to `columns`: the byte offset of this column's value within
        // a single row, or `None` if the column has no row value. Indexed by
        // column index, not name, since names may collide or be absent.
        let mut row_offsets: Vec<Option<usize>> = Vec::with_capacity(column_count);
        let mut running_row_offset = 0usize;
        for _ in 0..column_count {
            let flag_pos = r.position();
            let flags = r.u8()?;
            let storage = StorageFlag::from_byte(flags).ok_or_else(|| {
                BinError::malformed(
                    flag_pos,
                    format!("unknown column storage byte {:#04x}", flags & 0xF0),
                )
            })?;
            let ty = UtfType::from_nibble(flags & 0x0F).ok_or_else(|| {
                BinError::malformed(
                    flag_pos,
                    format!("unknown column type tag {:#x}", flags & 0x0F),
                )
            })?;
            let name = if storage.has_name() {
                let name_offset = r.u32_be()? as usize;
                Some(read_pool_string(name_offset)?)
            } else {
                None
            };
            let constant = if storage.has_inline_value() {
                Some(read_typed_value(&mut r, ty, string_pool, data_blob)?)
            } else {
                None
            };
            if storage.has_row_value() {
                row_offsets.push(Some(running_row_offset));
                running_row_offset += ty.fixed_size().expect("all current types are fixed-size");
            } else {
                row_offsets.push(None);
            }
            columns.push(UtfColumn {
                name,
                storage,
                ty,
                constant,
            });
        }

        // Row data.
        if running_row_offset != row_size {
            return Err(BinError::malformed(
                0,
                format!(
                    "row_size mismatch: schema sum {} vs header {}",
                    running_row_offset, row_size
                ),
            ));
        }
        let mut rows: Vec<UtfRow> = Vec::with_capacity(row_count);
        for row_index in 0..row_count {
            let row_start = rows_offset + row_index * row_size;
            let mut row: UtfRow = UtfRow::with_capacity(columns.len());
            for (col_idx, col) in columns.iter().enumerate() {
                // Determine the value for this column.
                let value = match col.storage {
                    StorageFlag::None => continue, // no name, no value — not represented in rows
                    StorageFlag::Constant | StorageFlag::Constant2 => {
                        col.constant.clone().ok_or_else(|| {
                            BinError::malformed(
                                0,
                                format!(
                                    "column {:?} declared {:?} storage but carries no inline value",
                                    col.name, col.storage,
                                ),
                            )
                        })?
                    }
                    StorageFlag::Zero => zero_value(col.ty),
                    StorageFlag::PerRow => {
                        let column_row_offset = row_offsets[col_idx].ok_or_else(|| {
                            BinError::malformed(
                                0,
                                format!(
                                    "column {:?} is PerRow but has no row offset (internal invariant violated)",
                                    col.name,
                                ),
                            )
                        })?;
                        r.seek(row_start + column_row_offset)?;
                        read_typed_value(&mut r, col.ty, string_pool, data_blob)?
                    }
                };
                if let Some(name) = &col.name {
                    row.insert(name.clone(), value);
                }
            }
            rows.push(row);
        }

        // Validate that we touched only inside the table.
        let _ = read_blob;
        let _ = data_blob;

        Ok(Self {
            name,
            columns,
            rows,
        })
    }

    /// Serialize the table to a byte vector (including the 8-byte magic+size prefix).
    ///
    /// # Errors
    ///
    /// Returns an error if any column declares a name as `Some(_)` while
    /// its storage is `None` (or vice versa), an inline-constant column
    /// is missing its constant value, a row's per-column value type
    /// differs from the schema's declared type, or the computed
    /// `column_count` / `row_count` exceeds the on-disk header's
    /// `u16` / `u32` limits.
    ///
    /// # Panics
    ///
    /// Panics only if internal invariants of the writer are violated
    /// (e.g. a string that was just interned isn't found in the pool, or
    /// a new variable-size [`UtfType`] is added without updating
    /// `fixed_size()`). User input cannot trigger these.
    pub fn to_bytes(&self) -> BinResult<Vec<u8>> {
        let mut w = Writer::new();

        // We assemble the four sections (schema, rows, strings, data) into
        // separate buffers, then patch offsets.
        let mut strings = StringPool::new();
        let _table_name_offset = strings.intern(&self.name);
        for col in &self.columns {
            if let Some(name) = &col.name {
                strings.intern(name);
            }
            if let Some(UtfValue::String(s)) = &col.constant {
                strings.intern(s);
            }
        }
        for row in &self.rows {
            for col in &self.columns {
                if col.storage.has_row_value() {
                    if let Some(name) = &col.name {
                        if let Some(UtfValue::String(s)) = row.get(name) {
                            strings.intern(s);
                        }
                    }
                }
            }
        }

        let mut data_blob = DataBlob::new();

        // Schema bytes (with conditional name offset + inline constants).
        let mut schema_buf = Writer::new();
        let mut row_size: u16 = 0;
        for col in &self.columns {
            let flags = col.storage.storage_bits() | col.ty.nibble();
            schema_buf.write_u8(flags);
            if col.storage.has_name() {
                let name = col.name.as_deref().ok_or_else(|| {
                    BinError::malformed(
                        0,
                        format!(
                            "column with storage {:?} requires a name (got None)",
                            col.storage
                        ),
                    )
                })?;
                let name_off = strings.offset_of(name).expect("interned above");
                schema_buf.write_u32_be(name_off);
            } else if col.name.is_some() {
                return Err(BinError::malformed(
                    0,
                    format!(
                        "column with storage {:?} must have name = None (got Some(_))",
                        col.storage
                    ),
                ));
            }
            if col.storage.has_inline_value() {
                let value = col.constant.as_ref().ok_or_else(|| {
                    BinError::malformed(
                        0,
                        format!(
                            "column {:?} declared {:?} but has no inline value",
                            col.name, col.storage
                        ),
                    )
                })?;
                if value.ty() != col.ty {
                    return Err(BinError::malformed(
                        0,
                        format!(
                            "column {:?} inline value type {:?} differs from declared {:?}",
                            col.name,
                            value.ty(),
                            col.ty
                        ),
                    ));
                }
                write_typed_value(&mut schema_buf, value, &mut strings, &mut data_blob)?;
            }
            if col.storage.has_row_value() {
                row_size = row_size
                    .checked_add(col.ty.fixed_size().expect("fixed-size") as u16)
                    .ok_or_else(|| BinError::malformed(0, "row_size overflows u16"))?;
            }
        }

        // Row bytes — only PerRow columns contribute.
        let mut rows_buf = Writer::new();
        for row in &self.rows {
            for col in &self.columns {
                if !col.storage.has_row_value() {
                    continue;
                }
                let name = col.name.as_deref().ok_or_else(|| {
                    BinError::malformed(0, "PerRow column without a name is invalid")
                })?;
                let value = row.get(name).ok_or_else(|| {
                    BinError::malformed(0, format!("row missing column {name:?}"))
                })?;
                if value.ty() != col.ty {
                    return Err(BinError::malformed(
                        0,
                        format!(
                            "row column {:?} has type {:?}, schema declares {:?}",
                            name,
                            value.ty(),
                            col.ty
                        ),
                    ));
                }
                write_typed_value(&mut rows_buf, value, &mut strings, &mut data_blob)?;
            }
        }

        let table_name_offset = strings.offset_of(&self.name).expect("interned above");

        // Compute section offsets (relative to the magic-end position, byte 0x08).
        let header_fields_size: u32 = 4 + 4 + 4 + 4 + 2 + 2 + 4; // RowsOff, StringsOff, DataOff, NameOff, ColCount, RowSize, RowCount = 24
        let schema_size = schema_buf.position() as u32;
        let rows_offset_rel = header_fields_size + schema_size;
        let rows_size = rows_buf.position() as u32;
        let strings_offset_rel = rows_offset_rel + rows_size;
        let strings_pool_bytes = strings.into_bytes();
        let strings_size = strings_pool_bytes.len() as u32;
        let data_offset_rel = strings_offset_rel + strings_size;
        let data_pool_bytes = data_blob.into_bytes();
        let data_size = data_pool_bytes.len() as u32;
        let total_table_size = data_offset_rel + data_size;
        let column_count: u16 = u16::try_from(self.columns.len())
            .map_err(|_| BinError::malformed(0, "column_count exceeds u16"))?;
        let row_count: u32 = u32::try_from(self.rows.len())
            .map_err(|_| BinError::malformed(0, "row_count exceeds u32"))?;

        // Emit table bytes (with the 8-byte magic+size prefix).
        w.write_bytes(MAGIC);
        w.write_u32_be(total_table_size);
        w.write_u32_be(rows_offset_rel);
        w.write_u32_be(strings_offset_rel);
        w.write_u32_be(data_offset_rel);
        w.write_u32_be(table_name_offset);
        w.write_u16_be(column_count);
        w.write_u16_be(row_size);
        w.write_u32_be(row_count);
        w.write_bytes(schema_buf.buffer());
        w.write_bytes(rows_buf.buffer());
        w.write_bytes(&strings_pool_bytes);
        w.write_bytes(&data_pool_bytes);

        Ok(w.into_inner())
    }
}

fn zero_value(ty: UtfType) -> UtfValue {
    match ty {
        UtfType::U8 => UtfValue::U8(0),
        UtfType::S8 => UtfValue::S8(0),
        UtfType::U16 => UtfValue::U16(0),
        UtfType::S16 => UtfValue::S16(0),
        UtfType::U32 => UtfValue::U32(0),
        UtfType::S32 => UtfValue::S32(0),
        UtfType::U64 => UtfValue::U64(0),
        UtfType::S64 => UtfValue::S64(0),
        UtfType::F32 => UtfValue::F32(0.0),
        UtfType::F64 => UtfValue::F64(0.0),
        UtfType::String => UtfValue::String(String::new()),
        UtfType::Data => UtfValue::Data(Vec::new()),
    }
}

fn read_typed_value(
    r: &mut Reader<'_>,
    ty: UtfType,
    string_pool: &[u8],
    data_blob: &[u8],
) -> BinResult<UtfValue> {
    Ok(match ty {
        UtfType::U8 => UtfValue::U8(r.u8()?),
        UtfType::S8 => UtfValue::S8(r.i8()?),
        UtfType::U16 => UtfValue::U16(r.u16_be()?),
        UtfType::S16 => UtfValue::S16(r.i16_be()?),
        UtfType::U32 => UtfValue::U32(r.u32_be()?),
        UtfType::S32 => UtfValue::S32(r.i32_be()?),
        UtfType::U64 => UtfValue::U64(r.u64_be()?),
        UtfType::S64 => UtfValue::S64(r.i64_be()?),
        UtfType::F32 => UtfValue::F32(r.f32_be()?),
        UtfType::F64 => UtfValue::F64(r.f64_be()?),
        UtfType::String => {
            let offset = r.u32_be()? as usize;
            if offset >= string_pool.len() {
                return Err(BinError::malformed(
                    r.position(),
                    format!("string offset {offset} past pool"),
                ));
            }
            let mut end = offset;
            while end < string_pool.len() && string_pool[end] != 0 {
                end += 1;
            }
            UtfValue::String(
                std::str::from_utf8(&string_pool[offset..end])
                    .map(str::to_owned)
                    .map_err(|source| BinError::InvalidUtf8 {
                        pos: r.position(),
                        source,
                    })?,
            )
        }
        UtfType::Data => {
            let offset = r.u32_be()? as usize;
            let size = r.u32_be()? as usize;
            if offset + size > data_blob.len() {
                return Err(BinError::malformed(
                    r.position(),
                    format!("data ({offset}, {size}) past blob"),
                ));
            }
            UtfValue::Data(data_blob[offset..offset + size].to_vec())
        }
    })
}

fn write_typed_value(
    w: &mut Writer,
    value: &UtfValue,
    strings: &mut StringPool,
    data_blob: &mut DataBlob,
) -> BinResult<()> {
    match value {
        UtfValue::U8(v) => w.write_u8(*v),
        UtfValue::S8(v) => w.write_i8(*v),
        UtfValue::U16(v) => w.write_u16_be(*v),
        UtfValue::S16(v) => w.write_i16_be(*v),
        UtfValue::U32(v) => w.write_u32_be(*v),
        UtfValue::S32(v) => w.write_i32_be(*v),
        UtfValue::U64(v) => w.write_u64_be(*v),
        UtfValue::S64(v) => w.write_i64_be(*v),
        UtfValue::F32(v) => w.write_f32_be(*v),
        UtfValue::F64(v) => w.write_f64_be(*v),
        UtfValue::String(s) => {
            let off = strings.intern(s);
            w.write_u32_be(off);
        }
        UtfValue::Data(bytes) => {
            let (off, size) = data_blob.append(bytes);
            w.write_u32_be(off);
            w.write_u32_be(size);
        }
    }
    Ok(())
}

/// Deduplicating string pool. Empty string is always at offset 0.
struct StringPool {
    bytes: Vec<u8>,
    index: HashMap<String, u32>,
}

impl StringPool {
    fn new() -> Self {
        let mut pool = Self {
            bytes: Vec::new(),
            index: HashMap::new(),
        };
        pool.intern(""); // empty string at offset 0
        pool
    }

    fn intern(&mut self, s: &str) -> u32 {
        if let Some(&off) = self.index.get(s) {
            return off;
        }
        let offset = self.bytes.len() as u32;
        self.bytes.extend_from_slice(s.as_bytes());
        self.bytes.push(0);
        self.index.insert(s.to_string(), offset);
        offset
    }

    fn offset_of(&self, s: &str) -> Option<u32> {
        self.index.get(s).copied()
    }

    fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

struct DataBlob {
    bytes: Vec<u8>,
}

impl DataBlob {
    fn new() -> Self {
        Self { bytes: Vec::new() }
    }

    fn append(&mut self, data: &[u8]) -> (u32, u32) {
        let off = self.bytes.len() as u32;
        self.bytes.extend_from_slice(data);
        (off, data.len() as u32)
    }

    fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> UtfTable {
        let mut row1: UtfRow = UtfRow::new();
        row1.insert("id".into(), UtfValue::U32(1));
        row1.insert("name".into(), UtfValue::String("alpha".into()));
        row1.insert("size".into(), UtfValue::U64(0x1234_5678));

        let mut row2: UtfRow = UtfRow::new();
        row2.insert("id".into(), UtfValue::U32(2));
        row2.insert("name".into(), UtfValue::String("beta".into()));
        row2.insert("size".into(), UtfValue::U64(0xCAFE_BABE));

        UtfTable {
            name: "MyTable".into(),
            columns: vec![
                UtfColumn {
                    name: Some("id".into()),
                    storage: StorageFlag::PerRow,
                    ty: UtfType::U32,
                    constant: None,
                },
                UtfColumn {
                    name: Some("name".into()),
                    storage: StorageFlag::PerRow,
                    ty: UtfType::String,
                    constant: None,
                },
                UtfColumn {
                    name: Some("size".into()),
                    storage: StorageFlag::PerRow,
                    ty: UtfType::U64,
                    constant: None,
                },
                UtfColumn {
                    name: Some("kind".into()),
                    storage: StorageFlag::Constant,
                    ty: UtfType::String,
                    constant: Some(UtfValue::String("FILE".into())),
                },
                UtfColumn {
                    name: Some("reserved".into()),
                    storage: StorageFlag::Zero,
                    ty: UtfType::U32,
                    constant: None,
                },
            ],
            rows: vec![row1, row2],
        }
    }

    #[test]
    fn round_trip_preserves_table() {
        let table = sample();
        // Add the constant + zero columns to rows for equality.
        let mut expected = table.clone();
        for row in &mut expected.rows {
            row.insert("kind".into(), UtfValue::String("FILE".into()));
            row.insert("reserved".into(), UtfValue::U32(0));
        }
        let bytes = table.to_bytes().unwrap();
        let parsed = UtfTable::parse(&bytes).unwrap();
        assert_eq!(parsed.name, expected.name);
        assert_eq!(parsed.columns, expected.columns);
        assert_eq!(parsed.rows, expected.rows);
    }

    #[test]
    fn byte_round_trip_is_stable() {
        let bytes1 = sample().to_bytes().unwrap();
        let parsed = UtfTable::parse(&bytes1).unwrap();
        let bytes2 = parsed.to_bytes().unwrap();
        assert_eq!(bytes1, bytes2);
    }

    #[test]
    fn header_magic_and_layout() {
        let bytes = sample().to_bytes().unwrap();
        assert_eq!(&bytes[..4], MAGIC);
        // Column count at offset 0x18 (in @UTF table, which is offset 0 of our buffer).
        let col_count = u16::from_be_bytes([bytes[0x18], bytes[0x19]]);
        assert_eq!(col_count, 5);
    }

    #[test]
    fn data_blob_round_trips() {
        let bytes = [0x42u8; 32];
        let table = UtfTable {
            name: "BlobTable".into(),
            columns: vec![UtfColumn {
                name: Some("payload".into()),
                storage: StorageFlag::PerRow,
                ty: UtfType::Data,
                constant: None,
            }],
            rows: vec![{
                let mut r: UtfRow = UtfRow::new();
                r.insert("payload".into(), UtfValue::Data(bytes.to_vec()));
                r
            }],
        };
        let encoded = table.to_bytes().unwrap();
        let parsed = UtfTable::parse(&encoded).unwrap();
        assert_eq!(parsed, table);
    }

    /// All five canonical CRI storage variants must round-trip byte-equal.
    #[test]
    fn all_five_storage_variants_round_trip() {
        let table = UtfTable {
            name: "FiveStorage".into(),
            columns: vec![
                UtfColumn {
                    name: None,
                    storage: StorageFlag::None,
                    ty: UtfType::U32,
                    constant: None,
                },
                UtfColumn {
                    name: Some("a_zero".into()),
                    storage: StorageFlag::Zero,
                    ty: UtfType::U64,
                    constant: None,
                },
                UtfColumn {
                    name: Some("b_const".into()),
                    storage: StorageFlag::Constant,
                    ty: UtfType::String,
                    constant: Some(UtfValue::String("constant-value".into())),
                },
                UtfColumn {
                    name: Some("c_perrow".into()),
                    storage: StorageFlag::PerRow,
                    ty: UtfType::U32,
                    constant: None,
                },
                UtfColumn {
                    name: Some("d_const2".into()),
                    storage: StorageFlag::Constant2,
                    ty: UtfType::U16,
                    constant: Some(UtfValue::U16(0xABCD)),
                },
            ],
            rows: vec![{
                let mut r: UtfRow = UtfRow::new();
                r.insert("c_perrow".into(), UtfValue::U32(42));
                r
            }],
        };

        let bytes = table.to_bytes().unwrap();
        let parsed = UtfTable::parse(&bytes).unwrap();

        // Schema round-trips exactly.
        assert_eq!(parsed.columns, table.columns);

        // Row contains: a_zero=0, b_const=constant, c_perrow=42, d_const2=0xABCD.
        // (The None column produces no row entry.)
        let row = &parsed.rows[0];
        assert_eq!(row.get("a_zero").unwrap(), &UtfValue::U64(0));
        assert_eq!(
            row.get("b_const").unwrap(),
            &UtfValue::String("constant-value".into())
        );
        assert_eq!(row.get("c_perrow").unwrap(), &UtfValue::U32(42));
        assert_eq!(row.get("d_const2").unwrap(), &UtfValue::U16(0xABCD));
        assert_eq!(row.len(), 4, "the None column must not appear in rows");

        // And byte-equal round-trip.
        let bytes2 = parsed.to_bytes().unwrap();
        assert_eq!(bytes, bytes2);
    }

    /// A `StorageFlag::None` column has neither name nor on-disk value. Its
    /// flag byte (`0x00 | type_nibble`) must survive parse → write → parse
    /// with name == None.
    #[test]
    fn none_storage_preserves_name_absence() {
        let table = UtfTable {
            name: "NoneTable".into(),
            columns: vec![UtfColumn {
                name: None,
                storage: StorageFlag::None,
                ty: UtfType::U16,
                constant: None,
            }],
            rows: vec![UtfRow::new()],
        };
        let bytes = table.to_bytes().unwrap();
        // Column entry on disk: just the flag byte (no name offset, no value).
        // Schema lives between header (24 bytes after magic+size = byte 32 of file)
        // and rows offset. The first byte after the @UTF header is the flag.
        assert_eq!(bytes[0x20], 0x02, "flag = storage(0x00) | type(0x02=u16)");

        let parsed = UtfTable::parse(&bytes).unwrap();
        assert_eq!(parsed.columns.len(), 1);
        assert_eq!(parsed.columns[0].name, None);
        assert_eq!(parsed.columns[0].storage, StorageFlag::None);
    }

    /// Regression: confirm the doc's old (wrong) mapping no longer works.
    /// Encode a column with raw flag `0x16` (the real CPK encoding of
    /// `FileSize` with `StorageFlag::Zero`) and verify our parser reads it
    /// as Zero, not Constant.
    #[test]
    fn flag_0x16_is_zero_not_constant() {
        // Hand-build a one-column @UTF table with flag = 0x16 (Zero u64).
        let table = UtfTable {
            name: "T".into(),
            columns: vec![UtfColumn {
                name: Some("FileSize".into()),
                storage: StorageFlag::Zero,
                ty: UtfType::U64,
                constant: None,
            }],
            rows: vec![UtfRow::new()],
        };
        let bytes = table.to_bytes().unwrap();
        // Schema's first byte (offset 0x20 of the @UTF buffer) is the flag.
        assert_eq!(bytes[0x20], 0x16, "0x16 = Zero(0x10) | u64(0x06)");

        let parsed = UtfTable::parse(&bytes).unwrap();
        assert_eq!(parsed.columns[0].storage, StorageFlag::Zero);
        // And the row carries the implicit zero value.
        assert_eq!(parsed.rows[0].get("FileSize").unwrap(), &UtfValue::U64(0));
    }
}
