//! Danganronpa V3 SRD block container reader/writer.
//!
//! SRD is a typed-block tree used for textures, font atlases, vertex
//! buffers, and resource metadata. Each block has a fixed 16-byte header
//! followed by a `data` section and an optional `subdata` section
//! (recursive — subdata is itself a sequence of blocks):
//!
//! ```text
//! offset 0x00  4 bytes   magic — `$CFH` / `$TXR` / `$RSI` / `$CT0` / `$TRE`
//!                        / `$TXI` / `$VTX` / `$RSF` / etc.
//! offset 0x04  4 bytes   data_size u32 BE
//! offset 0x08  4 bytes   subdata_size u32 BE
//! offset 0x0C  4 bytes   unknown u32 BE — preserved verbatim
//! offset 0x10  …         data section (data_size bytes; payload is LE
//!                        despite the BE header sizes)
//! …            …         padding to 0x10 boundary
//! …            …         subdata section (subdata_size bytes) — zero or
//!                        more nested blocks
//! …            …         padding to 0x10 boundary
//! ```
//!
//! Large bulk data (texture pixels) can live in sidecar files alongside the
//! `.srd`: `.srdi` for inline/embedded pixel data, `.srdv` for streamed
//! video-memory blobs. `$RSI` blocks reference these via flag bits.
//!
//! ## Scope of v0.1
//!
//! This crate parses and re-emits the **`.srd` / `.stx`** file (the block
//! tree) with full round-trip fidelity:
//!
//! - The wrapper structure (16-byte big-endian header, 0x10 alignment between
//!   data and subdata sections, recursive subdata).
//! - `$CFH`, `$CT0`, `$TXR`, `$RSI` block payloads (typed).
//! - Any other block magic (`$TRE`, `$TXI`, `$VTX`, `$RSF`, or unknown) —
//!   stored as raw bytes and round-tripped verbatim.
//!
//! Sidecar files (`.srdi` / `.srdv`) are **not** parsed by this crate. The
//! `ResourceInfo` entries inside `$RSI` blocks are exposed verbatim
//! (`Vec<Vec<u32>>`); resolving the external blobs is a job for the CLI / font
//! patcher built on top.

// Several SRD field names (`unknown_10`, `unknown_1a`, `unknown_1d`,
// `unknown_4`) are deliberately named after the byte offset they sit at in
// the source block — renaming them would lose the on-disk correspondence
// that reviewers rely on when tracing bytes to fields. Clippy's
// `similar_names` flags the resulting cluster of `unknown_*` names; allow.
#![allow(clippy::similar_names)]

pub mod texture;

use bitflags::bitflags;

use drv3_binio::{BinError, BinResult, Reader, Writer};

pub const MAGIC_CFH: &[u8; 4] = b"$CFH";
pub const MAGIC_CT0: &[u8; 4] = b"$CT0";
pub const MAGIC_TXR: &[u8; 4] = b"$TXR";
pub const MAGIC_RSI: &[u8; 4] = b"$RSI";

/// Parsed SRD file (the `.srd` / `.stx` half — sidecars handled by callers).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Srd {
    pub blocks: Vec<Block>,
}

/// A single SRD block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    /// `$CFH` — container file header. No payload; `Unknown0C = 1`. Must be the first top-level block.
    Cfh,
    /// `$CT0` — terminator. No payload; `Unknown0C = 0`.
    Ct0,
    /// `$TXR` — texture metadata.
    Txr { txr: TxrData, children: Vec<Block> },
    /// `$RSI` — resource info.
    Rsi { rsi: RsiData, children: Vec<Block> },
    /// Any other 4-byte magic. Data and subdata are preserved verbatim;
    /// `unknown_0c` retains the header's third u32 BE field.
    Other {
        magic: [u8; 4],
        unknown_0c: u32,
        data: Vec<u8>,
        children: Vec<Block>,
    },
}

/// `$TXR` payload (16 bytes, little-endian).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TxrData {
    pub unknown_10: i32,
    pub swizzle: u16,
    pub display_width: u16,
    pub display_height: u16,
    pub scanline: u16,
    pub format: u8,
    pub unknown_1d: u8,
    pub palette: u8,
    pub palette_id: u8,
}

bitflags! {
    /// High-bit flags carried in the first u32 of every ResourceInfo entry.
    /// The remaining 29 bits hold the byte offset into the chosen sidecar.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct ResourceLocationFlags: u32 {
        const SRDI = 0x2000_0000;
        const SRDV = 0x4000_0000;
    }
}

/// Mask isolating the 29-bit offset portion of `ResourceInfo[0]`.
pub const RESOURCE_OFFSET_MASK: u32 = 0x1FFF_FFFF;

/// `$RSI` payload — opaque metadata around an inline `ResourceData` blob and a
/// trailing string list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RsiData {
    pub unknown_10: u8,
    pub unknown_11: u8,
    pub unknown_12: u8,
    pub fallback_resource_info_count: u8,
    pub resource_info_count: i16,
    pub fallback_resource_info_size: i16,
    pub resource_info_size: i16,
    pub unknown_1a: i16,
    /// One entry per `ResourceInfo`. Each entry is exactly
    /// `resource_info_size / 4` u32 LE values, preserved verbatim.
    pub resource_info_list: Vec<Vec<u32>>,
    /// Inline payload between the resource-info list and the string list. For
    /// DR V3 font containers this holds an `SpFt` font block.
    pub resource_data: Vec<u8>,
    /// Resource strings. Stored as raw byte slices (Shift-JIS on disk; in
    /// practice the observed values are ASCII) — preserved verbatim so this
    /// crate need not depend on a Shift-JIS codec.
    pub resource_strings: Vec<Vec<u8>>,
}

impl Srd {
    /// Parse an SRD block tree from a byte buffer.
    ///
    /// # Errors
    ///
    /// Returns an error if a block header is malformed, a block's
    /// declared data or subdata size extends past the buffer, or a known
    /// block magic (`$CFH`, `$TXR`, `$RSI`, `$CT0`) has a payload that
    /// doesn't match the expected layout.
    pub fn parse(input: &[u8]) -> BinResult<Self> {
        let mut r = Reader::new(input);
        let blocks = parse_blocks(&mut r, input.len())?;
        Ok(Self { blocks })
    }

    /// Encode the SRD block tree to a byte vector.
    ///
    /// # Errors
    ///
    /// Returns an error if any block's serialized payload exceeds the
    /// u32 limits in its header, or if recursive subdata serialization
    /// fails for a nested block.
    pub fn to_bytes(&self) -> BinResult<Vec<u8>> {
        let mut w = Writer::new();
        write_blocks(&mut w, &self.blocks)?;
        Ok(w.into_inner())
    }
}

fn parse_blocks(r: &mut Reader<'_>, end: usize) -> BinResult<Vec<Block>> {
    let mut blocks: Vec<Block> = Vec::new();
    while r.position() < end {
        blocks.push(parse_block(r)?);
    }
    if r.position() != end {
        return Err(BinError::malformed(
            r.position(),
            format!(
                "block stream consumed past end (pos={} end={end})",
                r.position()
            ),
        ));
    }
    Ok(blocks)
}

fn parse_block(r: &mut Reader<'_>) -> BinResult<Block> {
    let magic: [u8; 4] = r.array()?;
    let data_length = r.u32_be()? as usize;
    let subdata_length = r.u32_be()? as usize;
    let unknown_0c = r.u32_be()?;

    let data = r.bytes(data_length)?.to_vec();
    r.align_to(0x10)?;
    let subdata_start = r.position();
    let subdata = r.bytes(subdata_length)?;
    let mut sub_reader = Reader::new(subdata);
    let children = parse_blocks(&mut sub_reader, subdata_length)?;
    r.seek(subdata_start + subdata_length)?;
    r.align_to(0x10)?;

    match (&magic, data_length, unknown_0c) {
        (b"$CFH", 0, 1) => {
            if !children.is_empty() {
                return Err(BinError::malformed(
                    r.position(),
                    "$CFH must not have child blocks",
                ));
            }
            Ok(Block::Cfh)
        }
        (b"$CT0", 0, 0) => {
            if !children.is_empty() {
                return Err(BinError::malformed(
                    r.position(),
                    "$CT0 must not have child blocks",
                ));
            }
            Ok(Block::Ct0)
        }
        (b"$TXR", _, 0) => {
            let txr = parse_txr(&data)?;
            Ok(Block::Txr { txr, children })
        }
        (b"$RSI", _, 0) => {
            let rsi = parse_rsi(&data)?;
            Ok(Block::Rsi { rsi, children })
        }
        _ => Ok(Block::Other {
            magic,
            unknown_0c,
            data,
            children,
        }),
    }
}

fn parse_txr(data: &[u8]) -> BinResult<TxrData> {
    let mut r = Reader::new(data);
    let unknown_10 = r.i32_le()?;
    let swizzle = r.u16_le()?;
    let display_width = r.u16_le()?;
    let display_height = r.u16_le()?;
    let scanline = r.u16_le()?;
    let format = r.u8()?;
    let unknown_1d = r.u8()?;
    let palette = r.u8()?;
    let palette_id = r.u8()?;
    Ok(TxrData {
        unknown_10,
        swizzle,
        display_width,
        display_height,
        scanline,
        format,
        unknown_1d,
        palette,
        palette_id,
    })
}

fn parse_rsi(data: &[u8]) -> BinResult<RsiData> {
    let mut r = Reader::new(data);
    let unknown_10 = r.u8()?;
    let unknown_11 = r.u8()?;
    let unknown_12 = r.u8()?;
    let fallback_resource_info_count = r.u8()?;
    let resource_info_count = r.i16_le()?;
    let fallback_resource_info_size = r.i16_le()?;
    let resource_info_size = r.i16_le()?;
    let unknown_1a = r.i16_le()?;
    let resource_string_list_offset = r.i32_le()? as usize;

    let effective_count = if resource_info_count != 0 {
        resource_info_count
    } else {
        i16::from(fallback_resource_info_count)
    };
    let effective_size = if resource_info_size != 0 {
        resource_info_size
    } else {
        fallback_resource_info_size
    };
    if effective_count < 0 || effective_size < 0 {
        return Err(BinError::malformed(
            0,
            format!(
                "negative resource_info dimensions: count={effective_count} size={effective_size}"
            ),
        ));
    }
    if effective_size % 4 != 0 {
        return Err(BinError::malformed(
            0,
            format!("resource_info_size {effective_size} is not a multiple of 4"),
        ));
    }
    let count = effective_count as usize;
    let size = effective_size as usize;
    let values_per_entry = size / 4;

    let mut resource_info_list: Vec<Vec<u32>> = Vec::with_capacity(count);
    for _ in 0..count {
        let mut entry = Vec::with_capacity(values_per_entry);
        for _ in 0..values_per_entry {
            entry.push(r.u32_le()?);
        }
        resource_info_list.push(entry);
    }

    let resource_data_start = r.position();
    if resource_string_list_offset < resource_data_start || resource_string_list_offset > data.len()
    {
        return Err(BinError::malformed(
            0x0C,
            format!(
                "resource_string_list_offset {resource_string_list_offset:#x} out of range \
                 (data start {resource_data_start:#x}, total {:#x})",
                data.len()
            ),
        ));
    }
    let resource_data = r
        .bytes(resource_string_list_offset - resource_data_start)?
        .to_vec();

    // Each string runs to its null terminator. The `while` bound stops after
    // the last terminator; a trailing empty string (two nulls in a row at the
    // end) is captured as a final empty entry, matching what `serialize_rsi`
    // writes back.
    let mut resource_strings: Vec<Vec<u8>> = Vec::new();
    while r.position() < data.len() {
        let start = r.position();
        let mut s = Vec::new();
        loop {
            if r.position() >= data.len() {
                return Err(BinError::malformed(start, "unterminated resource string"));
            }
            let b = r.u8()?;
            if b == 0 {
                break;
            }
            s.push(b);
        }
        resource_strings.push(s);
    }

    Ok(RsiData {
        unknown_10,
        unknown_11,
        unknown_12,
        fallback_resource_info_count,
        resource_info_count,
        fallback_resource_info_size,
        resource_info_size,
        unknown_1a,
        resource_info_list,
        resource_data,
        resource_strings,
    })
}

fn write_blocks(w: &mut Writer, blocks: &[Block]) -> BinResult<()> {
    for block in blocks {
        write_block(w, block)?;
    }
    Ok(())
}

fn write_block(w: &mut Writer, block: &Block) -> BinResult<()> {
    let (magic, unknown_0c, data, children) = match block {
        Block::Cfh => (*MAGIC_CFH, 1u32, Vec::new(), &[][..]),
        Block::Ct0 => (*MAGIC_CT0, 0u32, Vec::new(), &[][..]),
        Block::Txr { txr, children } => (*MAGIC_TXR, 0, serialize_txr(txr), children.as_slice()),
        Block::Rsi { rsi, children } => (*MAGIC_RSI, 0, serialize_rsi(rsi)?, children.as_slice()),
        Block::Other {
            magic,
            unknown_0c,
            data,
            children,
        } => (*magic, *unknown_0c, data.clone(), children.as_slice()),
    };

    // Serialize subdata to a buffer so we know its length up front.
    let mut sub_writer = Writer::new();
    write_blocks(&mut sub_writer, children)?;
    let subdata = sub_writer.into_inner();

    w.write_bytes(&magic);
    w.write_u32_be(data.len() as u32);
    w.write_u32_be(subdata.len() as u32);
    w.write_u32_be(unknown_0c);

    w.write_bytes(&data);
    w.pad_to(0x10, 0);
    w.write_bytes(&subdata);
    w.pad_to(0x10, 0);
    Ok(())
}

fn serialize_txr(txr: &TxrData) -> Vec<u8> {
    let mut w = Writer::with_capacity(0x10);
    w.write_i32_le(txr.unknown_10);
    w.write_u16_le(txr.swizzle);
    w.write_u16_le(txr.display_width);
    w.write_u16_le(txr.display_height);
    w.write_u16_le(txr.scanline);
    w.write_u8(txr.format);
    w.write_u8(txr.unknown_1d);
    w.write_u8(txr.palette);
    w.write_u8(txr.palette_id);
    w.into_inner()
}

fn serialize_rsi(rsi: &RsiData) -> BinResult<Vec<u8>> {
    if rsi.resource_info_size != 0 && rsi.resource_info_size % 4 != 0 {
        return Err(BinError::malformed(
            0,
            format!(
                "resource_info_size {} is not a multiple of 4",
                rsi.resource_info_size
            ),
        ));
    }

    let mut w = Writer::new();
    w.write_u8(rsi.unknown_10);
    w.write_u8(rsi.unknown_11);
    w.write_u8(rsi.unknown_12);
    w.write_u8(rsi.fallback_resource_info_count);
    w.write_i16_le(rsi.resource_info_count);
    w.write_i16_le(rsi.fallback_resource_info_size);
    w.write_i16_le(rsi.resource_info_size);
    w.write_i16_le(rsi.unknown_1a);

    // Reserve resource_string_list_offset (4 bytes signed); patch later.
    let offset_patch = w.reserve(4);

    for entry in &rsi.resource_info_list {
        for &value in entry {
            w.write_u32_le(value);
        }
    }
    w.write_bytes(&rsi.resource_data);

    let resource_string_list_offset = i32::try_from(w.position())
        .map_err(|_| BinError::malformed(0, "resource string list offset exceeds i32"))?;
    w.patch_slice(offset_patch, &resource_string_list_offset.to_le_bytes())?;

    for s in &rsi.resource_strings {
        w.write_bytes(s);
        w.write_u8(0);
    }

    Ok(w.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_txr() -> TxrData {
        TxrData {
            unknown_10: 0x1234_5678,
            swizzle: 1,
            display_width: 512,
            display_height: 256,
            scanline: 512,
            format: 0x01,
            unknown_1d: 0,
            palette: 0,
            palette_id: 0,
        }
    }

    fn make_rsi() -> RsiData {
        RsiData {
            unknown_10: 1,
            unknown_11: 2,
            unknown_12: 3,
            fallback_resource_info_count: 0,
            resource_info_count: 1,
            fallback_resource_info_size: 0,
            resource_info_size: 32,
            unknown_1a: 0,
            resource_info_list: vec![vec![
                0x4000_0000,
                0x0000_2000,
                0x80,
                0,
                0x0E93,
                0x30,
                0x0E4C,
                0xFFFF,
            ]],
            resource_data: b"SpFt-placeholder".to_vec(),
            resource_strings: vec![b"font_atlas".to_vec(), b"".to_vec()],
        }
    }

    fn sample() -> Srd {
        Srd {
            blocks: vec![
                Block::Cfh,
                Block::Txr {
                    txr: make_txr(),
                    children: vec![
                        Block::Rsi {
                            rsi: make_rsi(),
                            children: vec![],
                        },
                        Block::Ct0,
                    ],
                },
                Block::Other {
                    magic: *b"$TRE",
                    unknown_0c: 0,
                    data: vec![1, 2, 3, 4, 5],
                    children: vec![],
                },
                Block::Ct0,
            ],
        }
    }

    #[test]
    fn round_trip_preserves_bytes() {
        let srd = sample();
        let bytes = srd.to_bytes().unwrap();
        let parsed = Srd::parse(&bytes).unwrap();
        assert_eq!(parsed, srd);
        assert_eq!(parsed.to_bytes().unwrap(), bytes);
    }

    #[test]
    fn cfh_must_be_marked() {
        let bytes = sample().to_bytes().unwrap();
        // $CFH magic at offset 0, unknown_0c at offset 0x0C must be 1 (big-endian).
        assert_eq!(&bytes[0..4], MAGIC_CFH);
        assert_eq!(&bytes[0x0C..0x10], &1u32.to_be_bytes());
    }

    #[test]
    fn unknown_block_preserved_verbatim() {
        let srd = Srd {
            blocks: vec![
                Block::Cfh,
                Block::Other {
                    magic: *b"$ZZZ",
                    unknown_0c: 0xAA55_BB66,
                    data: (0..37).collect::<Vec<u8>>(),
                    children: vec![],
                },
                Block::Ct0,
            ],
        };
        let bytes = srd.to_bytes().unwrap();
        let parsed = Srd::parse(&bytes).unwrap();
        assert_eq!(parsed, srd);
    }

    #[test]
    fn nested_subdata_round_trips() {
        let srd = Srd {
            blocks: vec![
                Block::Cfh,
                Block::Other {
                    magic: *b"$AAA",
                    unknown_0c: 0,
                    data: b"top".to_vec(),
                    children: vec![Block::Other {
                        magic: *b"$BBB",
                        unknown_0c: 0,
                        data: b"middle".to_vec(),
                        children: vec![Block::Other {
                            magic: *b"$CCC",
                            unknown_0c: 0,
                            data: b"leaf".to_vec(),
                            children: vec![],
                        }],
                    }],
                },
                Block::Ct0,
            ],
        };
        let bytes = srd.to_bytes().unwrap();
        let parsed = Srd::parse(&bytes).unwrap();
        assert_eq!(parsed, srd);
    }

    #[test]
    fn resource_location_flags_parse() {
        let flags = ResourceLocationFlags::SRDV;
        let value: u32 = flags.bits() | 0x1234;
        assert_eq!(
            value & !RESOURCE_OFFSET_MASK,
            ResourceLocationFlags::SRDV.bits()
        );
        assert_eq!(value & RESOURCE_OFFSET_MASK, 0x1234);
    }
}
