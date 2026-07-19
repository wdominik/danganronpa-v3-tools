//! JSON DTOs and conversions for the Danganronpa V3 `dump` / `build` /
//! extract-pack **exchange** schema.
//!
//! This crate owns the serde layer for the per-format sidecars (the modules
//! below) plus the CPK and SPC manifests. The format library crates stay
//! serde-free — they expose plain Rust types, and this crate maps them to and
//! from the human-editable JSON `drv3-cli` reads and writes. The
//! translation-**patch** schema lives in the separate `drv3-dto-patch` crate,
//! which reuses this crate's [`glyph`] geometry DTOs.

#![allow(clippy::wildcard_imports)]

pub mod glyph;

use serde::{Deserialize, Serialize};

/// JSON shape for an STX file.
pub mod stx {
    use super::*;

    /// Schema tag carried by every STX exchange JSON document.
    pub const STX_SCHEMA: &str = "drv3-stx/v1";

    /// JSON document for one STX string-table file.
    #[derive(Debug, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct StxJson {
        pub schema: String,
        pub tables: Vec<StxTableJson>,
    }

    /// One sub-table within an STX file.
    #[derive(Debug, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct StxTableJson {
        pub unknown: u32,
        pub entries: Vec<StxEntryJson>,
    }

    /// A single `(id, text)` string entry.
    #[derive(Debug, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct StxEntryJson {
        pub id: u32,
        pub text: String,
    }

    impl From<&drv3_stx::Stx> for StxJson {
        fn from(stx: &drv3_stx::Stx) -> Self {
            Self {
                schema: STX_SCHEMA.to_string(),
                tables: stx
                    .tables
                    .iter()
                    .map(|t| StxTableJson {
                        unknown: t.unknown,
                        entries: t
                            .entries
                            .iter()
                            .map(|e| StxEntryJson {
                                id: e.id,
                                text: e.text.clone(),
                            })
                            .collect(),
                    })
                    .collect(),
            }
        }
    }

    impl TryFrom<StxJson> for drv3_stx::Stx {
        type Error = anyhow::Error;
        fn try_from(j: StxJson) -> anyhow::Result<Self> {
            if j.schema != STX_SCHEMA {
                anyhow::bail!("unsupported schema {:?} (expected {STX_SCHEMA})", j.schema);
            }
            Ok(Self {
                tables: j
                    .tables
                    .into_iter()
                    .map(|t| drv3_stx::StxTable {
                        unknown: t.unknown,
                        entries: t
                            .entries
                            .into_iter()
                            .map(|e| drv3_stx::StxEntry {
                                id: e.id,
                                text: e.text,
                            })
                            .collect(),
                    })
                    .collect(),
            })
        }
    }
}

/// JSON shape for a DAT typed-table file.
pub mod dat {
    use super::*;
    use drv3_dat::{Cell, Column, ColumnType, Dat};

    /// Schema tag carried by every DAT exchange JSON document.
    pub const DAT_SCHEMA: &str = "drv3-dat/v1";

    /// JSON document for a DAT typed table: a column schema plus tagged rows.
    #[derive(Debug, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct DatJson {
        pub schema: String,
        pub columns: Vec<ColumnJson>,
        pub rows: Vec<Vec<CellJson>>,
    }

    /// One column definition: name, type tag, and per-row element count.
    #[derive(Debug, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct ColumnJson {
        pub name: String,
        #[serde(rename = "type")]
        pub ty: String,
        pub count: u16,
    }

    /// A tagged cell value: the `type` tag selects the variant and `values`
    /// carries its array of elements.
    #[derive(Debug, Serialize, Deserialize)]
    #[serde(tag = "type", content = "values", rename_all = "snake_case")]
    pub enum CellJson {
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

    fn cell_to_json(c: &Cell) -> CellJson {
        match c {
            Cell::U8(v) => CellJson::U8(v.clone()),
            Cell::U16(v) => CellJson::U16(v.clone()),
            Cell::U32(v) => CellJson::U32(v.clone()),
            Cell::U64(v) => CellJson::U64(v.clone()),
            Cell::S8(v) => CellJson::S8(v.clone()),
            Cell::S16(v) => CellJson::S16(v.clone()),
            Cell::S32(v) => CellJson::S32(v.clone()),
            Cell::S64(v) => CellJson::S64(v.clone()),
            Cell::F32(v) => CellJson::F32(v.clone()),
            Cell::F64(v) => CellJson::F64(v.clone()),
            Cell::Ascii(v) => CellJson::Ascii(v.clone()),
            Cell::Label(v) => CellJson::Label(v.clone()),
            Cell::Refer(v) => CellJson::Refer(v.clone()),
            Cell::Utf16(v) => CellJson::Utf16(v.clone()),
        }
    }

    fn cell_from_json(j: CellJson) -> Cell {
        match j {
            CellJson::U8(v) => Cell::U8(v),
            CellJson::U16(v) => Cell::U16(v),
            CellJson::U32(v) => Cell::U32(v),
            CellJson::U64(v) => Cell::U64(v),
            CellJson::S8(v) => Cell::S8(v),
            CellJson::S16(v) => Cell::S16(v),
            CellJson::S32(v) => Cell::S32(v),
            CellJson::S64(v) => Cell::S64(v),
            CellJson::F32(v) => Cell::F32(v),
            CellJson::F64(v) => Cell::F64(v),
            CellJson::Ascii(v) => Cell::Ascii(v),
            CellJson::Label(v) => Cell::Label(v),
            CellJson::Refer(v) => Cell::Refer(v),
            CellJson::Utf16(v) => Cell::Utf16(v),
        }
    }

    impl From<&Dat> for DatJson {
        fn from(d: &Dat) -> Self {
            Self {
                schema: DAT_SCHEMA.to_string(),
                columns: d
                    .schema
                    .iter()
                    .map(|c| ColumnJson {
                        name: c.name.clone(),
                        ty: c.ty.tag().to_string(),
                        count: c.count,
                    })
                    .collect(),
                rows: d
                    .rows
                    .iter()
                    .map(|r| r.iter().map(cell_to_json).collect())
                    .collect(),
            }
        }
    }

    impl TryFrom<DatJson> for Dat {
        type Error = anyhow::Error;
        fn try_from(j: DatJson) -> anyhow::Result<Self> {
            if j.schema != DAT_SCHEMA {
                anyhow::bail!("unsupported schema {:?} (expected {DAT_SCHEMA})", j.schema);
            }
            let schema = j
                .columns
                .into_iter()
                .map(|c| {
                    let ty = ColumnType::from_tag(&c.ty)
                        .ok_or_else(|| anyhow::anyhow!("unknown column type {:?}", c.ty))?;
                    Ok::<_, anyhow::Error>(Column {
                        name: c.name,
                        ty,
                        count: c.count,
                    })
                })
                .collect::<anyhow::Result<Vec<_>>>()?;
            let rows = j
                .rows
                .into_iter()
                .map(|r| r.into_iter().map(cell_from_json).collect())
                .collect();
            Ok(Self { schema, rows })
        }
    }
}

/// JSON shape for a `SpFt` font-metadata block.
pub mod spft {
    use super::*;
    use drv3_spft::{Glyph, SpFt};

    /// Schema tag carried by every `SpFt` exchange JSON document.
    pub const SPFT_SCHEMA: &str = "drv3-spft/v1";

    /// JSON document for a `SpFt` font: header fields plus per-glyph metrics.
    #[derive(Debug, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct SpFtJson {
        pub schema: String,
        pub unknown6: u32,
        pub bit_flag_count: u32,
        pub scale_flag: u32,
        pub font_name: String,
        pub glyphs: Vec<GlyphJson>,
    }

    /// One glyph's codepoint plus its atlas position, size, and kerning.
    #[derive(Debug, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct GlyphJson {
        pub codepoint: u32,
        pub position: PositionJson,
        pub size: SizeJson,
        pub kerning: KerningJson,
    }

    use crate::glyph::{KerningJson, PositionJson, SizeJson};

    impl From<&SpFt> for SpFtJson {
        fn from(s: &SpFt) -> Self {
            Self {
                schema: SPFT_SCHEMA.to_string(),
                unknown6: s.unknown6,
                bit_flag_count: s.bit_flag_count,
                scale_flag: s.scale_flag,
                font_name: s.font_name.clone(),
                glyphs: s
                    .glyphs
                    .iter()
                    .map(|g| GlyphJson {
                        codepoint: g.codepoint,
                        position: PositionJson {
                            x: g.position.0,
                            y: g.position.1,
                        },
                        size: SizeJson {
                            width: g.size.0,
                            height: g.size.1,
                        },
                        kerning: KerningJson {
                            left: g.kerning.0,
                            right: g.kerning.1,
                            vertical: g.kerning.2,
                        },
                    })
                    .collect(),
            }
        }
    }

    impl TryFrom<SpFtJson> for SpFt {
        type Error = anyhow::Error;
        fn try_from(j: SpFtJson) -> anyhow::Result<Self> {
            if j.schema != SPFT_SCHEMA {
                anyhow::bail!("unsupported schema {:?} (expected {SPFT_SCHEMA})", j.schema);
            }
            Ok(Self {
                unknown6: j.unknown6,
                bit_flag_count: j.bit_flag_count,
                scale_flag: j.scale_flag,
                font_name: j.font_name,
                glyphs: j
                    .glyphs
                    .into_iter()
                    .map(|g| Glyph {
                        codepoint: g.codepoint,
                        position: (g.position.x, g.position.y),
                        size: (g.size.width, g.size.height),
                        kerning: (g.kerning.left, g.kerning.right, g.kerning.vertical),
                    })
                    .collect(),
            })
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn spft_json_object_shape_round_trips() {
            let spft = SpFt {
                unknown6: 6,
                bit_flag_count: 0x100,
                scale_flag: 20,
                font_name: "Test".into(),
                glyphs: vec![
                    Glyph {
                        codepoint: 65,
                        position: (10, 20),
                        size: (30, 40),
                        kerning: (1, -2, 5),
                    },
                    Glyph {
                        codepoint: 97,
                        position: (50, 60),
                        size: (12, 16),
                        kerning: (0, 0, 7),
                    },
                ],
            };
            // SpFt → JSON DTO → JSON text → JSON DTO → SpFt.
            let json = serde_json::to_string(&SpFtJson::from(&spft)).unwrap();
            // The new object field names are present in the serialized text.
            assert!(json.contains("\"x\":10"));
            assert!(json.contains("\"width\":30"));
            assert!(json.contains("\"vertical\":5"));
            let back: SpFt = serde_json::from_str::<SpFtJson>(&json)
                .unwrap()
                .try_into()
                .unwrap();
            assert_eq!(back, spft);
        }

        #[test]
        fn array_geometry_is_rejected() {
            // Glyph geometry must be a named object; the legacy positional-array
            // form (`[x, y]`) is no longer accepted.
            let json = r#"{
                "codepoint": 65,
                "position": [1, 2],
                "size": { "width": 3, "height": 4 },
                "kerning": { "left": 0, "right": 0, "vertical": 0 }
            }"#;
            assert!(serde_json::from_str::<GlyphJson>(json).is_err());
        }
    }
}

/// JSON shape for a WRD byte-code script.
pub mod wrd {
    use super::*;
    use drv3_wrd::{Command, LocalBranch, Wrd};

    /// Schema tag carried by every WRD exchange JSON document.
    pub const WRD_SCHEMA: &str = "drv3-wrd/v1";

    /// JSON document for a WRD script: the opcode stream plus its label,
    /// parameter, and string tables.
    #[derive(Debug, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct WrdJson {
        pub schema: String,
        pub unknown1: u32,
        pub external_string_count: u16,
        pub commands: Vec<CommandJson>,
        pub local_branches: Vec<LocalBranchJson>,
        pub label_offsets: Vec<u16>,
        pub label_names: Vec<String>,
        pub parameters: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub internal_strings: Option<Vec<String>>,
    }

    /// One byte-code command: an opcode and its argument words.
    #[derive(Debug, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct CommandJson {
        pub opcode: u8,
        pub args: Vec<u16>,
    }

    /// One local branch target: a label id and its byte offset.
    #[derive(Debug, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct LocalBranchJson {
        pub id: u16,
        pub offset: u16,
    }

    impl From<&Wrd> for WrdJson {
        fn from(w: &Wrd) -> Self {
            Self {
                schema: WRD_SCHEMA.to_string(),
                unknown1: w.unknown1,
                external_string_count: w.external_string_count,
                commands: w
                    .commands
                    .iter()
                    .map(|c| CommandJson {
                        opcode: c.opcode,
                        args: c.args.clone(),
                    })
                    .collect(),
                local_branches: w
                    .local_branches
                    .iter()
                    .map(|b| LocalBranchJson {
                        id: b.id,
                        offset: b.offset,
                    })
                    .collect(),
                label_offsets: w.label_offsets.clone(),
                label_names: w.label_names.clone(),
                parameters: w.parameters.clone(),
                internal_strings: w.internal_strings.clone(),
            }
        }
    }

    impl TryFrom<WrdJson> for Wrd {
        type Error = anyhow::Error;
        fn try_from(j: WrdJson) -> anyhow::Result<Self> {
            if j.schema != WRD_SCHEMA {
                anyhow::bail!("unsupported schema {:?} (expected {WRD_SCHEMA})", j.schema);
            }
            Ok(Self {
                unknown1: j.unknown1,
                external_string_count: j.external_string_count,
                commands: j
                    .commands
                    .into_iter()
                    .map(|c| Command {
                        opcode: c.opcode,
                        args: c.args,
                    })
                    .collect(),
                local_branches: j
                    .local_branches
                    .into_iter()
                    .map(|b| LocalBranch {
                        id: b.id,
                        offset: b.offset,
                    })
                    .collect(),
                label_offsets: j.label_offsets,
                label_names: j.label_names,
                parameters: j.parameters,
                internal_strings: j.internal_strings,
            })
        }
    }
}

/// Serde-friendly mirror of `drv3_cpk::UtfValue` and the surrounding schema
/// types, used by the CPK manifest.
pub mod utf {
    use super::*;
    use drv3_cpk::{StorageFlag, UtfColumn, UtfType, UtfValue};

    /// Tagged enum: each variant maps to a single JSON key. Numeric values use
    /// JSON numbers (`serde_json` handles full u64 / i64); binary values are
    /// hex-encoded.
    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    #[serde(rename_all = "snake_case")]
    pub enum UtfValueJson {
        U8(u8),
        U16(u16),
        U32(u32),
        U64(u64),
        S8(i8),
        S16(i16),
        S32(i32),
        S64(i64),
        F32(f32),
        F64(f64),
        String(String),
        DataHex(String),
    }

    impl From<&UtfValue> for UtfValueJson {
        fn from(v: &UtfValue) -> Self {
            match v {
                UtfValue::U8(x) => Self::U8(*x),
                UtfValue::U16(x) => Self::U16(*x),
                UtfValue::U32(x) => Self::U32(*x),
                UtfValue::U64(x) => Self::U64(*x),
                UtfValue::S8(x) => Self::S8(*x),
                UtfValue::S16(x) => Self::S16(*x),
                UtfValue::S32(x) => Self::S32(*x),
                UtfValue::S64(x) => Self::S64(*x),
                UtfValue::F32(x) => Self::F32(*x),
                UtfValue::F64(x) => Self::F64(*x),
                UtfValue::String(s) => Self::String(s.clone()),
                UtfValue::Data(bytes) => Self::DataHex(hex_encode(bytes)),
            }
        }
    }

    impl TryFrom<UtfValueJson> for UtfValue {
        type Error = anyhow::Error;
        fn try_from(j: UtfValueJson) -> anyhow::Result<Self> {
            Ok(match j {
                UtfValueJson::U8(x) => Self::U8(x),
                UtfValueJson::U16(x) => Self::U16(x),
                UtfValueJson::U32(x) => Self::U32(x),
                UtfValueJson::U64(x) => Self::U64(x),
                UtfValueJson::S8(x) => Self::S8(x),
                UtfValueJson::S16(x) => Self::S16(x),
                UtfValueJson::S32(x) => Self::S32(x),
                UtfValueJson::S64(x) => Self::S64(x),
                UtfValueJson::F32(x) => Self::F32(x),
                UtfValueJson::F64(x) => Self::F64(x),
                UtfValueJson::String(s) => Self::String(s),
                UtfValueJson::DataHex(s) => Self::Data(hex_decode(&s)?),
            })
        }
    }

    /// JSON tag for a column's storage class (mirrors `drv3_cpk::StorageFlag`).
    #[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
    #[serde(rename_all = "snake_case")]
    pub enum StorageFlagJson {
        None,
        Zero,
        Constant,
        PerRow,
        Constant2,
    }

    impl From<StorageFlag> for StorageFlagJson {
        fn from(s: StorageFlag) -> Self {
            match s {
                StorageFlag::None => Self::None,
                StorageFlag::Zero => Self::Zero,
                StorageFlag::Constant => Self::Constant,
                StorageFlag::PerRow => Self::PerRow,
                StorageFlag::Constant2 => Self::Constant2,
            }
        }
    }

    impl From<StorageFlagJson> for StorageFlag {
        fn from(s: StorageFlagJson) -> Self {
            match s {
                StorageFlagJson::None => Self::None,
                StorageFlagJson::Zero => Self::Zero,
                StorageFlagJson::Constant => Self::Constant,
                StorageFlagJson::PerRow => Self::PerRow,
                StorageFlagJson::Constant2 => Self::Constant2,
            }
        }
    }

    /// JSON tag for a column's value type (mirrors `drv3_cpk::UtfType`).
    #[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
    #[serde(rename_all = "snake_case")]
    pub enum UtfTypeJson {
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

    impl From<UtfType> for UtfTypeJson {
        fn from(t: UtfType) -> Self {
            match t {
                UtfType::U8 => Self::U8,
                UtfType::S8 => Self::S8,
                UtfType::U16 => Self::U16,
                UtfType::S16 => Self::S16,
                UtfType::U32 => Self::U32,
                UtfType::S32 => Self::S32,
                UtfType::U64 => Self::U64,
                UtfType::S64 => Self::S64,
                UtfType::F32 => Self::F32,
                UtfType::F64 => Self::F64,
                UtfType::String => Self::String,
                UtfType::Data => Self::Data,
            }
        }
    }

    impl From<UtfTypeJson> for UtfType {
        fn from(t: UtfTypeJson) -> Self {
            match t {
                UtfTypeJson::U8 => Self::U8,
                UtfTypeJson::S8 => Self::S8,
                UtfTypeJson::U16 => Self::U16,
                UtfTypeJson::S16 => Self::S16,
                UtfTypeJson::U32 => Self::U32,
                UtfTypeJson::S32 => Self::S32,
                UtfTypeJson::U64 => Self::U64,
                UtfTypeJson::S64 => Self::S64,
                UtfTypeJson::F32 => Self::F32,
                UtfTypeJson::F64 => Self::F64,
                UtfTypeJson::String => Self::String,
                UtfTypeJson::Data => Self::Data,
            }
        }
    }

    /// JSON shape for one `@UTF` column: optional name, storage class, type,
    /// and an optional inline constant value.
    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    #[serde(deny_unknown_fields)]
    pub struct UtfColumnJson {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub name: Option<String>,
        pub storage: StorageFlagJson,
        #[serde(rename = "type")]
        pub ty: UtfTypeJson,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub constant: Option<UtfValueJson>,
    }

    impl From<&UtfColumn> for UtfColumnJson {
        fn from(c: &UtfColumn) -> Self {
            Self {
                name: c.name.clone(),
                storage: c.storage.into(),
                ty: c.ty.into(),
                constant: c.constant.as_ref().map(Into::into),
            }
        }
    }

    impl TryFrom<UtfColumnJson> for UtfColumn {
        type Error = anyhow::Error;
        fn try_from(j: UtfColumnJson) -> anyhow::Result<Self> {
            Ok(Self {
                name: j.name,
                storage: j.storage.into(),
                ty: j.ty.into(),
                constant: j.constant.map(UtfValue::try_from).transpose()?,
            })
        }
    }

    /// Lowercase hex without separators (`0xAB` → `"ab"`). Re-used by the
    /// SPC manifest's `unknown1` field encoder; visible to sibling modules
    /// under `super::utf`.
    pub(super) fn hex_encode(bytes: &[u8]) -> String {
        let mut s = String::with_capacity(bytes.len() * 2);
        for &b in bytes {
            use std::fmt::Write;
            let _ = write!(s, "{b:02x}");
        }
        s
    }

    /// Inverse of [`hex_encode`]. Visible to sibling modules under
    /// `super::utf`.
    ///
    /// # Errors
    ///
    /// Returns an error if the input has odd length or contains a byte
    /// pair that isn't valid hex.
    pub(super) fn hex_decode(s: &str) -> anyhow::Result<Vec<u8>> {
        let s = s.trim();
        if !s.len().is_multiple_of(2) {
            anyhow::bail!("hex string has odd length ({})", s.len());
        }
        let mut out = Vec::with_capacity(s.len() / 2);
        for i in (0..s.len()).step_by(2) {
            out.push(
                u8::from_str_radix(&s[i..i + 2], 16)
                    .map_err(|e| anyhow::anyhow!("invalid hex byte at offset {i}: {e}"))?,
            );
        }
        Ok(out)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn utf_value_round_trips_all_variants() {
            let cases = vec![
                UtfValue::U8(0xAB),
                UtfValue::U16(0xBEEF),
                UtfValue::U32(0xDEAD_BEEF),
                UtfValue::U64(0xCAFE_BABE_DEAD_BEEF),
                UtfValue::S8(-1),
                UtfValue::S16(-2),
                UtfValue::S32(-3),
                UtfValue::S64(i64::MIN),
                UtfValue::F32(std::f32::consts::PI),
                UtfValue::F64(std::f64::consts::E),
                UtfValue::String("hello, 日本語".into()),
                UtfValue::Data(vec![0, 1, 2, 0xFE, 0xFF]),
            ];
            for v in cases {
                let json = serde_json::to_string(&UtfValueJson::from(&v)).unwrap();
                let back: UtfValueJson = serde_json::from_str(&json).unwrap();
                let recovered = UtfValue::try_from(back).unwrap();
                assert_eq!(recovered, v, "round-trip mismatch for {v:?} via {json}");
            }
        }

        #[test]
        fn hex_encode_decode_round_trip() {
            let original = vec![0u8, 1, 2, 0xAB, 0xCD, 0xEF, 0xFF];
            let s = hex_encode(&original);
            assert_eq!(s, "000102abcdefff");
            assert_eq!(hex_decode(&s).unwrap(), original);
        }
    }
}

/// CPK extract/pack manifest: persists the full header + TOC metadata that
/// `Cpk::to_bytes` needs but file bodies alone can't carry.
pub mod cpk_manifest {
    use std::collections::BTreeMap;

    use super::utf::{UtfColumnJson, UtfValueJson};
    use super::*;
    use drv3_cpk::{Cpk, CpkFile, UtfColumn, UtfRow, UtfValue};

    pub const MANIFEST_FILENAME: &str = "manifest.json";
    pub const ETOC_SIDECAR: &str = "_etoc.bin";
    pub const ITOC_SIDECAR: &str = "_itoc.bin";
    pub const GTOC_SIDECAR: &str = "_gtoc.bin";

    /// Schema tag carried by every CPK manifest. The project is pre-1.0; the
    /// schema may change in place at `v1` — readers reject any other value.
    pub const CPK_MANIFEST_SCHEMA: &str = "drv3-cpk/v1";

    /// The full CPK manifest: schema tag, header `@UTF` table, TOC column
    /// schema, file list, and optional sidecar-packet filenames.
    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    #[serde(deny_unknown_fields)]
    pub struct CpkManifestJson {
        pub schema: String,
        pub header: HeaderJson,
        /// TOC `@UTF` column schema, preserved verbatim from extract so pack
        /// can re-emit the same column order, types, and storage flags.
        pub toc_columns: Vec<UtfColumnJson>,
        pub files: Vec<CpkFileJson>,
        /// Filename of the ETOC sidecar relative to the manifest, or `None` if absent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub etoc_packet: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub itoc_packet: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub gtoc_packet: Option<String>,
    }

    /// The CPK header `@UTF` table: its column schema plus the single row of
    /// `PerRow` values.
    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    #[serde(deny_unknown_fields)]
    pub struct HeaderJson {
        /// Column schema, in declaration order — preserves storage flags and types.
        pub columns: Vec<UtfColumnJson>,
        /// Per-row values for `PerRow` columns. Keys are column names; the
        /// JSON sorts them alphabetically (`BTreeMap`) so manifest diffs are
        /// stable across runs.
        pub row: BTreeMap<String, UtfValueJson>,
    }

    /// One CPK file entry: its extract-relative path, id, user string, and any
    /// extra TOC columns preserved verbatim.
    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    #[serde(deny_unknown_fields)]
    pub struct CpkFileJson {
        /// Forward-slash relative path inside the extract directory (`<dir_name>/<file_name>`).
        pub path: String,
        pub id: u32,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        pub user_string: String,
        /// Any other TOC columns beyond the standard set, preserved verbatim.
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        pub extra: BTreeMap<String, UtfValueJson>,
    }

    impl From<&Cpk<'_>> for CpkManifestJson {
        fn from(cpk: &Cpk<'_>) -> Self {
            let columns: Vec<UtfColumnJson> =
                cpk.header_columns.iter().map(UtfColumnJson::from).collect();
            let toc_columns: Vec<UtfColumnJson> =
                cpk.toc_columns.iter().map(UtfColumnJson::from).collect();
            let mut row: BTreeMap<String, UtfValueJson> = BTreeMap::new();
            for (k, v) in &cpk.header_row {
                row.insert(k.clone(), UtfValueJson::from(v));
            }
            let files: Vec<CpkFileJson> = cpk
                .files
                .iter()
                .map(|f| CpkFileJson {
                    path: file_path(&f.dir_name, &f.file_name),
                    id: f.id,
                    user_string: f.user_string.clone(),
                    extra: f
                        .extra
                        .iter()
                        .map(|(k, v)| (k.clone(), UtfValueJson::from(v)))
                        .collect(),
                })
                .collect();
            Self {
                schema: CPK_MANIFEST_SCHEMA.to_string(),
                header: HeaderJson { columns, row },
                toc_columns,
                files,
                etoc_packet: cpk.etoc_packet.as_ref().map(|_| ETOC_SIDECAR.into()),
                itoc_packet: cpk.itoc_packet.as_ref().map(|_| ITOC_SIDECAR.into()),
                gtoc_packet: cpk.gtoc_packet.as_ref().map(|_| GTOC_SIDECAR.into()),
            }
        }
    }

    impl CpkManifestJson {
        /// Realize a [`Cpk`] from the manifest plus the file-body bytes and
        /// optional packet bytes supplied by the caller.
        ///
        /// # Errors
        ///
        /// Returns an error if the manifest `schema` tag is unsupported,
        /// `toc_columns` is empty, a header/TOC column or value fails to
        /// convert, or a file path is absolute or contains `..` / `.` segments.
        pub fn into_cpk(
            self,
            file_bodies: Vec<(CpkFileJson, Vec<u8>)>,
            etoc: Option<Vec<u8>>,
            itoc: Option<Vec<u8>>,
            gtoc: Option<Vec<u8>>,
        ) -> anyhow::Result<Cpk<'static>> {
            if self.schema != CPK_MANIFEST_SCHEMA {
                anyhow::bail!(
                    "manifest schema {:?} is not supported (this build expects {})",
                    self.schema,
                    CPK_MANIFEST_SCHEMA,
                );
            }
            let header_columns: Vec<UtfColumn> = self
                .header
                .columns
                .into_iter()
                .map(UtfColumn::try_from)
                .collect::<anyhow::Result<Vec<_>>>()?;
            let mut header_row: UtfRow = UtfRow::new();
            for (k, v) in self.header.row {
                header_row.insert(k, UtfValue::try_from(v)?);
            }

            if self.toc_columns.is_empty() {
                anyhow::bail!(
                    "manifest `toc_columns` is empty — re-extract the CPK to regenerate it"
                );
            }
            let toc_columns: Vec<UtfColumn> = self
                .toc_columns
                .into_iter()
                .map(UtfColumn::try_from)
                .collect::<anyhow::Result<Vec<_>>>()?;

            let mut files: Vec<CpkFile> = Vec::with_capacity(file_bodies.len());
            for (meta, data) in file_bodies {
                let (dir_name, file_name) = split_path(&meta.path)?;
                let mut extra: indexmap::IndexMap<String, UtfValue> = indexmap::IndexMap::new();
                for (k, v) in meta.extra {
                    extra.insert(k, UtfValue::try_from(v)?);
                }
                files.push(CpkFile {
                    dir_name,
                    file_name,
                    id: meta.id,
                    user_string: meta.user_string,
                    extra,
                    data: data.into(),
                });
            }

            Ok(Cpk {
                header_row,
                header_columns,
                toc_columns,
                files,
                etoc_packet: etoc,
                itoc_packet: itoc,
                gtoc_packet: gtoc,
            })
        }
    }

    fn file_path(dir: &str, name: &str) -> String {
        if dir.is_empty() {
            name.to_string()
        } else {
            format!("{dir}/{name}")
        }
    }

    /// Split `"dir/sub/name"` into `("dir/sub", "name")`. Bare `"name"` gives `("", "name")`.
    /// Rejects `..` segments and absolute paths to prevent extract escape.
    fn split_path(path: &str) -> anyhow::Result<(String, String)> {
        if path.starts_with('/') || path.contains('\\') {
            anyhow::bail!("manifest path {path:?} must be relative and use `/` separators");
        }
        for component in path.split('/') {
            if component == ".." || component == "." || component.is_empty() {
                anyhow::bail!("manifest path {path:?} contains invalid component {component:?}");
            }
        }
        match path.rsplit_once('/') {
            Some((dir, name)) => Ok((dir.to_string(), name.to_string())),
            None => Ok((String::new(), path.to_string())),
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn split_path_normal_cases() {
            assert_eq!(
                split_path("a.bin").unwrap(),
                (String::new(), "a.bin".into())
            );
            assert_eq!(
                split_path("dir/a.bin").unwrap(),
                ("dir".into(), "a.bin".into())
            );
            assert_eq!(
                split_path("a/b/c.bin").unwrap(),
                ("a/b".into(), "c.bin".into())
            );
        }

        #[test]
        fn split_path_rejects_escape() {
            assert!(split_path("../etc/passwd").is_err());
            assert!(split_path("/abs").is_err());
            assert!(split_path("a/../b").is_err());
            assert!(split_path("a\\b").is_err());
            assert!(split_path("a//b").is_err());
        }
    }
}

/// SPC archive extract/pack manifest: preserves the `unknown1` /
/// `unknown2` archive-level bytes, per-entry `compression_flag` and
/// `unknown_flag`, and the original entry order — all of which the
/// pre-manifest pack path was zeroing or normalizing away.
pub mod spc_manifest {
    use super::*;
    use drv3_spc::{COMPRESSION_LZSS, COMPRESSION_STORED, Spc, SpcEntry};

    pub const MANIFEST_FILENAME: &str = "manifest.json";
    /// Schema tag carried by every SPC manifest. Pre-1.0 the schema may change
    /// in place at `v1`; readers reject any other value.
    pub const SPC_MANIFEST_SCHEMA: &str = "drv3-spc/v1";

    /// Size of the `unknown1` field in bytes — fixed by the SPC format.
    const UNKNOWN1_LEN: usize = 0x24;

    /// The full SPC manifest: schema tag, archive-level `unknown1`/`unknown2`
    /// bytes, and the per-entry metadata in on-disk order.
    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    #[serde(deny_unknown_fields)]
    pub struct SpcManifestJson {
        pub schema: String,
        /// Hex-encoded 36 bytes from SPC header offset `0x04`.
        pub unknown1: String,
        /// u32 at SPC header offset `0x2C`.
        pub unknown2: u32,
        /// Entries in **on-disk order**. Order is preserved so any index-
        /// based lookups the game's loader may do continue to match.
        pub entries: Vec<SpcEntryJson>,
    }

    /// One SPC entry's metadata: name, compression method, and opaque flags.
    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    #[serde(deny_unknown_fields)]
    pub struct SpcEntryJson {
        /// Entry name, UTF-8. DR V3 ships ASCII-only names. `drv3-cli spc
        /// extract` rejects a non-UTF-8 name before writing the manifest;
        /// the `From<&Spc>` conversion substitutes an empty string.
        pub name: String,
        /// `"stored"` or `"lzss"` — readable rendering of the on-disk
        /// `compression_flag` (1 / 2 respectively).
        pub compression: SpcCompressionJson,
        /// Two opaque bytes at entry header offset `0x02`.
        pub unknown_flag: i16,
    }

    /// JSON tag for an entry's compression: `stored` or `lzss`.
    #[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
    #[serde(rename_all = "snake_case")]
    pub enum SpcCompressionJson {
        Stored,
        Lzss,
    }

    impl From<i16> for SpcCompressionJson {
        fn from(flag: i16) -> Self {
            if flag == COMPRESSION_LZSS {
                Self::Lzss
            } else {
                Self::Stored
            }
        }
    }

    impl SpcCompressionJson {
        fn to_flag(self) -> i16 {
            match self {
                Self::Stored => COMPRESSION_STORED,
                Self::Lzss => COMPRESSION_LZSS,
            }
        }
    }

    impl From<&Spc> for SpcManifestJson {
        fn from(spc: &Spc) -> Self {
            let entries: Vec<SpcEntryJson> = spc
                .entries
                .iter()
                .map(|e| SpcEntryJson {
                    name: std::str::from_utf8(&e.name).unwrap_or("").to_string(),
                    compression: SpcCompressionJson::from(e.compression_flag),
                    unknown_flag: e.unknown_flag,
                })
                .collect();
            Self {
                schema: SPC_MANIFEST_SCHEMA.to_string(),
                unknown1: super::utf::hex_encode(&spc.unknown1),
                unknown2: spc.unknown2,
                entries,
            }
        }
    }

    impl SpcManifestJson {
        /// Realise an [`Spc`] from the manifest plus the file-body bytes
        /// supplied by the caller, in the same order as `self.entries`.
        ///
        /// # Errors
        ///
        /// Returns an error if the `schema` tag is not [`SPC_MANIFEST_SCHEMA`],
        /// `unknown1` does not decode to exactly 36 bytes,
        /// `entries.len()` differs from `entry_bodies.len()`, or any
        /// entry's UTF-8 name fails the round-trip.
        pub fn into_spc(self, entry_bodies: Vec<Vec<u8>>) -> anyhow::Result<Spc> {
            if self.schema != SPC_MANIFEST_SCHEMA {
                anyhow::bail!(
                    "manifest schema {:?} is not supported (this build expects {})",
                    self.schema,
                    SPC_MANIFEST_SCHEMA,
                );
            }
            if self.entries.len() != entry_bodies.len() {
                anyhow::bail!(
                    "manifest declares {} entries but {} bodies were supplied",
                    self.entries.len(),
                    entry_bodies.len(),
                );
            }

            let unknown1_vec = super::utf::hex_decode(&self.unknown1)?;
            if unknown1_vec.len() != UNKNOWN1_LEN {
                anyhow::bail!(
                    "`unknown1` decodes to {} bytes; expected {UNKNOWN1_LEN}",
                    unknown1_vec.len(),
                );
            }
            let mut unknown1 = [0u8; UNKNOWN1_LEN];
            unknown1.copy_from_slice(&unknown1_vec);

            let entries: Vec<SpcEntry> = self
                .entries
                .into_iter()
                .zip(entry_bodies)
                .map(|(meta, data)| SpcEntry {
                    name: meta.name.into_bytes(),
                    compression_flag: meta.compression.to_flag(),
                    unknown_flag: meta.unknown_flag,
                    data,
                })
                .collect();

            Ok(Spc {
                unknown1,
                unknown2: self.unknown2,
                entries,
            })
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn manifest_round_trips_metadata() {
            let spc = Spc {
                unknown1: {
                    let mut a = [0u8; UNKNOWN1_LEN];
                    a[0] = 0xAA;
                    a[UNKNOWN1_LEN - 1] = 0xBB;
                    a
                },
                unknown2: 0xCAFE_BABE,
                entries: vec![
                    SpcEntry {
                        name: b"zeta.dat".to_vec(),
                        compression_flag: COMPRESSION_LZSS,
                        unknown_flag: 7,
                        data: b"first body".to_vec(),
                    },
                    SpcEntry {
                        name: b"alpha.dat".to_vec(),
                        compression_flag: COMPRESSION_STORED,
                        unknown_flag: 0,
                        data: b"second body".to_vec(),
                    },
                ],
            };

            let manifest = SpcManifestJson::from(&spc);
            // Order is preserved: zeta first, alpha second — not alphabetical.
            assert_eq!(manifest.entries[0].name, "zeta.dat");
            assert_eq!(manifest.entries[1].name, "alpha.dat");
            assert_eq!(manifest.entries[0].compression, SpcCompressionJson::Lzss);
            assert_eq!(manifest.entries[1].compression, SpcCompressionJson::Stored);

            let bodies: Vec<Vec<u8>> = spc.entries.iter().map(|e| e.data.clone()).collect();
            let recovered = manifest.into_spc(bodies).unwrap();
            assert_eq!(recovered, spc);
        }

        #[test]
        fn manifest_rejects_wrong_unknown1_length() {
            let mut m = SpcManifestJson {
                schema: SPC_MANIFEST_SCHEMA.to_string(),
                unknown1: "00".to_string(), // only 1 byte
                unknown2: 0,
                entries: vec![],
            };
            assert!(m.clone().into_spc(vec![]).is_err());

            m.unknown1 = "00".repeat(UNKNOWN1_LEN);
            assert!(m.into_spc(vec![]).is_ok());
        }

        #[test]
        fn manifest_rejects_body_count_mismatch() {
            let m = SpcManifestJson {
                schema: SPC_MANIFEST_SCHEMA.to_string(),
                unknown1: "00".repeat(UNKNOWN1_LEN),
                unknown2: 0,
                entries: vec![SpcEntryJson {
                    name: "x".into(),
                    compression: SpcCompressionJson::Stored,
                    unknown_flag: 0,
                }],
            };
            assert!(m.into_spc(vec![]).is_err());
        }

        #[test]
        fn manifest_rejects_unknown_key() {
            // deny_unknown_fields: a stray key is a hard error, not silently dropped.
            let json = r#"{
                "schema": "drv3-spc/v1",
                "unknown1": "00",
                "unknown2": 0,
                "entries": [],
                "typo_field": 1
            }"#;
            assert!(serde_json::from_str::<SpcManifestJson>(json).is_err());
        }

        #[test]
        fn manifest_rejects_wrong_schema_tag() {
            let m = SpcManifestJson {
                schema: "drv3-spc/v2".to_string(),
                unknown1: "00".repeat(UNKNOWN1_LEN),
                unknown2: 0,
                entries: vec![],
            };
            assert!(m.into_spc(vec![]).is_err());
        }
    }
}
