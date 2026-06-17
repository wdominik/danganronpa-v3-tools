//! JSON DTO types for the CLI's `dump` / `build` subcommands.
//!
//! Libraries are deliberately serde-free; the CLI owns the JSON exchange
//! format and converts between DTOs and library types. This decouples the
//! on-disk JSON schema from internal library APIs and keeps `serde` /
//! `serde_json` out of the library dependency graph.

#![allow(clippy::wildcard_imports)]

use serde::{Deserialize, Serialize};

/// JSON shape for an STX file.
pub(crate) mod stx {
    use super::*;

    #[derive(Debug, Serialize, Deserialize)]
    pub struct StxJson {
        pub tables: Vec<StxTableJson>,
    }

    #[derive(Debug, Serialize, Deserialize)]
    pub struct StxTableJson {
        pub unknown: u32,
        pub entries: Vec<StxEntryJson>,
    }

    #[derive(Debug, Serialize, Deserialize)]
    pub struct StxEntryJson {
        pub id: u32,
        pub text: String,
    }

    impl From<&drv3_stx::Stx> for StxJson {
        fn from(stx: &drv3_stx::Stx) -> Self {
            Self {
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

    impl From<StxJson> for drv3_stx::Stx {
        fn from(j: StxJson) -> Self {
            Self {
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
            }
        }
    }
}

pub(crate) mod dat {
    use super::*;
    use drv3_dat::{Cell, Column, ColumnType, Dat};

    #[derive(Debug, Serialize, Deserialize)]
    pub struct DatJson {
        pub schema: Vec<ColumnJson>,
        pub rows: Vec<Vec<CellJson>>,
    }

    #[derive(Debug, Serialize, Deserialize)]
    pub struct ColumnJson {
        pub name: String,
        #[serde(rename = "type")]
        pub ty: String,
        pub count: u16,
    }

    #[derive(Debug, Serialize, Deserialize)]
    #[serde(tag = "type", content = "values", rename_all = "lowercase")]
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

    fn column_type_tag(ty: ColumnType) -> &'static str {
        match ty {
            ColumnType::U8 => "u8",
            ColumnType::U16 => "u16",
            ColumnType::U32 => "u32",
            ColumnType::U64 => "u64",
            ColumnType::S8 => "s8",
            ColumnType::S16 => "s16",
            ColumnType::S32 => "s32",
            ColumnType::S64 => "s64",
            ColumnType::F32 => "f32",
            ColumnType::F64 => "f64",
            ColumnType::Ascii => "ascii",
            ColumnType::Label => "label",
            ColumnType::Refer => "refer",
            ColumnType::Utf16 => "utf16",
        }
    }

    fn column_type_from_tag(tag: &str) -> Option<ColumnType> {
        Some(match tag {
            "u8" => ColumnType::U8,
            "u16" => ColumnType::U16,
            "u32" => ColumnType::U32,
            "u64" => ColumnType::U64,
            "s8" => ColumnType::S8,
            "s16" => ColumnType::S16,
            "s32" => ColumnType::S32,
            "s64" => ColumnType::S64,
            "f32" => ColumnType::F32,
            "f64" => ColumnType::F64,
            "ascii" => ColumnType::Ascii,
            "label" => ColumnType::Label,
            "refer" => ColumnType::Refer,
            "utf16" => ColumnType::Utf16,
            _ => return None,
        })
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
                schema: d
                    .schema
                    .iter()
                    .map(|c| ColumnJson {
                        name: c.name.clone(),
                        ty: column_type_tag(c.ty).to_string(),
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
            let schema = j
                .schema
                .into_iter()
                .map(|c| {
                    let ty = column_type_from_tag(&c.ty)
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

pub(crate) mod spft {
    use super::*;
    use drv3_spft::{Glyph, SpFt};

    #[derive(Debug, Serialize, Deserialize)]
    pub struct SpFtJson {
        pub unknown6: u32,
        pub bit_flag_count: u32,
        pub scale_flag: u32,
        pub font_name: String,
        pub glyphs: Vec<GlyphJson>,
    }

    #[derive(Debug, Serialize, Deserialize)]
    pub struct GlyphJson {
        pub codepoint: u32,
        pub position: PositionJson,
        pub size: SizeJson,
        pub kerning: KerningJson,
    }

    /// Top-left atlas coordinate of a glyph, in pixels (12-bit each).
    #[derive(Debug, Serialize, Deserialize)]
    pub struct PositionJson {
        pub x: u16,
        pub y: u16,
    }

    /// Glyph bounding-box dimensions, in pixels.
    #[derive(Debug, Serialize, Deserialize)]
    pub struct SizeJson {
        pub width: u8,
        pub height: u8,
    }

    /// Per-glyph spacing deltas, in signed pixels: `left`/`right` are the
    /// horizontal side bearings, `vertical` shifts the glyph up/down.
    #[derive(Debug, Serialize, Deserialize)]
    pub struct KerningJson {
        pub left: i8,
        pub right: i8,
        pub vertical: i8,
    }

    impl From<&SpFt> for SpFtJson {
        fn from(s: &SpFt) -> Self {
            Self {
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

    impl From<SpFtJson> for SpFt {
        fn from(j: SpFtJson) -> Self {
            Self {
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
            }
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
            let back: SpFt = serde_json::from_str::<SpFtJson>(&json).unwrap().into();
            assert_eq!(back, spft);
        }
    }
}

pub(crate) mod wrd {
    use super::*;
    use drv3_wrd::{Command, LocalBranch, Wrd};

    #[derive(Debug, Serialize, Deserialize)]
    pub struct WrdJson {
        pub unknown1: u32,
        pub external_string_count: u16,
        pub commands: Vec<CommandJson>,
        pub local_branches: Vec<LocalBranchJson>,
        pub label_offsets: Vec<u16>,
        pub label_names: Vec<String>,
        pub parameters: Vec<String>,
        pub internal_strings: Option<Vec<String>>,
    }

    #[derive(Debug, Serialize, Deserialize)]
    pub struct CommandJson {
        pub opcode: u8,
        pub args: Vec<u16>,
    }

    #[derive(Debug, Serialize, Deserialize)]
    pub struct LocalBranchJson {
        pub id: u16,
        pub offset: u16,
    }

    impl From<&Wrd> for WrdJson {
        fn from(w: &Wrd) -> Self {
            Self {
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

    impl From<WrdJson> for Wrd {
        fn from(j: WrdJson) -> Self {
            Self {
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
            }
        }
    }
}

/// Serde-friendly mirror of `drv3_cpk::UtfValue` and the surrounding schema
/// types, used by the CPK manifest.
pub(crate) mod utf {
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

    #[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
    #[serde(rename_all = "PascalCase")]
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

    #[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
    #[serde(rename_all = "lowercase")]
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

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    pub struct UtfColumnJson {
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
pub(crate) mod cpk_manifest {
    use std::collections::BTreeMap;

    use super::utf::{UtfColumnJson, UtfValueJson};
    use super::*;
    use drv3_cpk::{Cpk, CpkFile, UtfColumn, UtfRow, UtfValue};

    pub(crate) const MANIFEST_FILENAME: &str = "manifest.json";
    pub(crate) const ETOC_SIDECAR: &str = "_etoc.bin";
    pub(crate) const ITOC_SIDECAR: &str = "_itoc.bin";
    pub(crate) const GTOC_SIDECAR: &str = "_gtoc.bin";

    /// Current manifest schema version. The project is pre-1.0; the schema
    /// may change in place without bumping this number — readers reject any
    /// other value. Versioning starts on 1.0.0.
    pub(crate) const MANIFEST_VERSION: u32 = 1;

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    pub(crate) struct CpkManifestJson {
        pub version: u32,
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

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    pub(crate) struct HeaderJson {
        /// `@UTF` table name (always `"CpkHeader"` in observed files).
        pub name: String,
        /// Column schema, in declaration order — preserves storage flags and types.
        pub columns: Vec<UtfColumnJson>,
        /// Per-row values for `PerRow` columns. Keys are column names; the
        /// JSON sorts them alphabetically (`BTreeMap`) so manifest diffs are
        /// stable across runs.
        pub row: BTreeMap<String, UtfValueJson>,
    }

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    pub(crate) struct CpkFileJson {
        /// Forward-slash relative path inside the extract directory (`<dir_name>/<file_name>`).
        pub path: String,
        pub id: u32,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        pub user_string: String,
        /// Any other TOC columns beyond the standard set, preserved verbatim.
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        pub extra: BTreeMap<String, UtfValueJson>,
    }

    impl From<&Cpk> for CpkManifestJson {
        fn from(cpk: &Cpk) -> Self {
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
                version: MANIFEST_VERSION,
                header: HeaderJson {
                    name: "CpkHeader".to_string(),
                    columns,
                    row,
                },
                toc_columns,
                files,
                etoc_packet: cpk.etoc_packet.as_ref().map(|_| ETOC_SIDECAR.into()),
                itoc_packet: cpk.itoc_packet.as_ref().map(|_| ITOC_SIDECAR.into()),
                gtoc_packet: cpk.gtoc_packet.as_ref().map(|_| GTOC_SIDECAR.into()),
            }
        }
    }

    /// Realise a `Cpk` from the manifest + file bodies + optional packet bytes
    /// supplied by the caller. The caller is responsible for resolving paths.
    impl CpkManifestJson {
        pub(crate) fn into_cpk(
            self,
            file_bodies: Vec<(CpkFileJson, Vec<u8>)>,
            etoc: Option<Vec<u8>>,
            itoc: Option<Vec<u8>>,
            gtoc: Option<Vec<u8>>,
        ) -> anyhow::Result<Cpk> {
            if self.version != MANIFEST_VERSION {
                anyhow::bail!(
                    "manifest schema version {} is not supported (this build expects {})",
                    self.version,
                    MANIFEST_VERSION,
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
                    data,
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
pub(crate) mod spc_manifest {
    use super::*;
    use drv3_spc::{COMPRESSION_LZSS, COMPRESSION_STORED, Spc, SpcEntry};

    pub(crate) const MANIFEST_FILENAME: &str = "manifest.json";
    /// Pre-1.0 the schema may change in place without bumping this value.
    /// Versioning begins on 1.0.0.
    pub(crate) const MANIFEST_VERSION: u32 = 1;

    /// Size of the `unknown1` field in bytes — fixed by the SPC format.
    const UNKNOWN1_LEN: usize = 0x24;

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    pub(crate) struct SpcManifestJson {
        pub version: u32,
        /// Hex-encoded 36 bytes from SPC header offset `0x04`.
        pub unknown1: String,
        /// u32 at SPC header offset `0x2C`.
        pub unknown2: u32,
        /// Entries in **on-disk order**. Order is preserved so any index-
        /// based lookups the game's loader may do continue to match.
        pub entries: Vec<SpcEntryJson>,
    }

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    pub(crate) struct SpcEntryJson {
        /// Entry name, UTF-8. DR V3 ships ASCII-only names; non-UTF-8
        /// names are rejected at extract time.
        pub name: String,
        /// `"stored"` or `"lzss"` — readable rendering of the on-disk
        /// `compression_flag` (1 / 2 respectively).
        pub compression: SpcCompressionJson,
        /// Two opaque bytes at entry header offset `0x02`.
        pub unknown_flag: i16,
    }

    #[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
    #[serde(rename_all = "lowercase")]
    pub(crate) enum SpcCompressionJson {
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
                version: MANIFEST_VERSION,
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
        /// Returns an error if `version` is not [`MANIFEST_VERSION`],
        /// `unknown1` does not decode to exactly 36 bytes,
        /// `entries.len()` differs from `entry_bodies.len()`, or any
        /// entry's UTF-8 name fails the round-trip.
        pub(crate) fn into_spc(self, entry_bodies: Vec<Vec<u8>>) -> anyhow::Result<Spc> {
            if self.version != MANIFEST_VERSION {
                anyhow::bail!(
                    "manifest schema version {} is not supported (this build expects {})",
                    self.version,
                    MANIFEST_VERSION,
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
                version: MANIFEST_VERSION,
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
                version: MANIFEST_VERSION,
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
    }
}
