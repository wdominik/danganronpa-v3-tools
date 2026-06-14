//! Font-group patching: SPFT metadata + BC4 atlas pixel writes.
//!
//! # Font container layout (DR V3)
//!
//! Each font in the game is a pair of co-located SPC members:
//!
//! - `<name>.stx` — despite the misleading extension, this is an SRD
//!   file (`$CFH` magic at offset 0). The SPFT (glyph metadata) sits at
//!   the start of `$RSI.resource_data` inside the top-level `$TXR`
//!   block's children.
//! - `<name>.srdv` — the BC4 atlas pixel sidecar (format `0x16`, swizzle
//!   `0x01`, atlas width 4096 for `v3_font00` and 2048 for the other 24
//!   fonts, heights between 100 and 469). The BC4 block layout is
//!   block-row-major with `scanline` bytes per block-row, decoded /
//!   re-encoded through [`drv3_srd::texture`].
//!
//! # Pipeline (per font group)
//!
//! 1. Parse the `<name>.stx` SPC member bytes as [`Srd`].
//! 2. Walk `srd.blocks` for a `$RSI` whose `resource_data` starts with
//!    `SpFt` magic.
//! 3. Parse the SPFT, apply metadata edits (`position`, `size`,
//!    `kerning`), add glyphs for new codepoints.
//! 4. If the group requests a taller `atlas` than the game ships: grow
//!    the BC4 atlas in place — extend the `$TXR` height, re-allocate the
//!    `.srdv` buffer (old block-rows copied verbatim, new rows zeroed),
//!    and bump the `$RSI` `ResourceInfo` blob size. Width must stay
//!    constant, so `scanline` is unchanged and old rows map 1:1.
//! 5. If any glyph patch carries pixel data: locate the parallel
//!    `<name>.srdv` SPC member, validate each glyph against the (possibly
//!    grown) atlas extent, and call
//!    [`drv3_srd::texture::blit_alpha_into_bc4`] once per glyph — only the
//!    affected BC4 blocks get re-encoded, so untouched atlas regions stay
//!    byte-exact.
//! 6. Re-serialize SPFT → put back into `rsi.resource_data` →
//!    re-serialize SRD → write back to the `.stx` SPC entry.

use std::collections::HashMap;

use drv3_spc::Spc;
use drv3_spft::{Glyph, SpFt};
use drv3_srd::texture::blit_alpha_into_bc4;
use drv3_srd::{Block, ResourceLocationFlags, RsiData, Srd, TxrData};

use crate::error::TranslateError;
use crate::model::{FontFileGroup, FontGlyphPatch};
use crate::report::PatchReport;

/// BC4 pixel format tag in `$TXR.format`. The atlas-growth path only
/// supports this format (block-row-major, 4×4 blocks, 8 bytes/block).
const TXR_FORMAT_BC4: u8 = 0x16;

const SPFT_MAGIC: &[u8; 4] = b"SpFt";

pub(crate) fn patch_font_member(
    spc: &mut Spc,
    member_idx: usize,
    cpk_path: &str,
    spc_member: &str,
    group: &FontFileGroup,
    report: &mut PatchReport,
) -> Result<(), TranslateError> {
    // ---- SPFT side: parse the .stx (which is actually an SRD container) ----
    let mut srd = Srd::parse(&spc.entries[member_idx].data).map_err(|e| TranslateError::Srd {
        cpk_path: cpk_path.to_string(),
        spc_member: spc_member.to_string(),
        source: e,
    })?;

    let rsi_path = find_spft_rsi(&srd.blocks).ok_or_else(|| TranslateError::SpftNotFound {
        cpk_path: cpk_path.to_string(),
        spc_member: spc_member.to_string(),
    })?;

    let rsi_resource_data = std::mem::take(rsi_resource_data_mut(&mut srd.blocks, &rsi_path));

    let mut spft = SpFt::parse(&rsi_resource_data).map_err(|e| TranslateError::Spft {
        cpk_path: cpk_path.to_string(),
        spc_member: spc_member.to_string(),
        source: e,
    })?;

    if let Some(name) = &group.font_name {
        spft.font_name.clone_from(name);
    }

    apply_glyph_metadata(&mut spft, &group.glyphs, report);

    // Re-serialize SPFT and put back into the SRD before we move on
    // to the atlas (the .stx write happens after atlas writes complete).
    let new_resource_data = spft.to_bytes();
    *rsi_resource_data_mut(&mut srd.blocks, &rsi_path) = new_resource_data;

    // ---- Atlas side: grow the atlas (if requested) and blit glyphs
    // into the .srdv sidecar SPC member. ----
    if group.atlas.is_some() || group.glyphs.iter().any(|g| g.glyph_alpha8.is_some()) {
        patch_atlas(spc, &mut srd, &rsi_path, cpk_path, spc_member, group, report)?;
    }

    // Serialize the SRD wrapper and put it back into the .stx entry.
    spc.entries[member_idx].data = srd.to_bytes().map_err(|e| TranslateError::Srd {
        cpk_path: cpk_path.to_string(),
        spc_member: spc_member.to_string(),
        source: e,
    })?;
    Ok(())
}

fn patch_atlas(
    spc: &mut Spc,
    srd: &mut Srd,
    rsi_path: &RsiPath,
    cpk_path: &str,
    spc_member: &str,
    group: &FontFileGroup,
    report: &mut PatchReport,
) -> Result<(), TranslateError> {
    let txr = find_txr(&srd.blocks).ok_or_else(|| TranslateError::AtlasGeometry {
        cpk_path: cpk_path.to_string(),
        spc_member: spc_member.to_string(),
        detail: "no $TXR block in SRD".into(),
    })?;
    let (cur_w, cur_h, fmt) = (txr.display_width, txr.display_height, txr.format);

    // Resolve the target atlas extent: honor a taller `atlas` request,
    // otherwise fall back to the shipped geometry. Validation of an
    // `atlas` block happens here even when no growth is needed, so a
    // bad width/format is reported up front.
    let (atlas_w, atlas_h) = match &group.atlas {
        Some(requested) => {
            if fmt != TXR_FORMAT_BC4 {
                return Err(TranslateError::AtlasUnsupportedFormat {
                    cpk_path: cpk_path.to_string(),
                    spc_member: spc_member.to_string(),
                    format: fmt,
                });
            }
            if requested.width != cur_w {
                return Err(TranslateError::AtlasWidthChange {
                    cpk_path: cpk_path.to_string(),
                    spc_member: spc_member.to_string(),
                    requested: requested.width,
                    current: cur_w,
                });
            }
            if requested.height < cur_h {
                return Err(TranslateError::AtlasShrink {
                    cpk_path: cpk_path.to_string(),
                    spc_member: spc_member.to_string(),
                    requested: requested.height,
                    current: cur_h,
                });
            }
            (cur_w, requested.height)
        }
        None => (cur_w, cur_h),
    };
    let grew = atlas_h > cur_h;
    let any_alpha = group.glyphs.iter().any(|g| g.glyph_alpha8.is_some());

    // Nothing to write into the sidecar: no growth and no pixels. (Reached
    // when an `atlas` block merely restates the shipped dimensions.)
    if !grew && !any_alpha {
        return Ok(());
    }

    let sidecar_name = sidecar_name_for(spc_member);
    let sidecar_idx = find_member_by_name(spc, &sidecar_name).ok_or_else(|| {
        TranslateError::AtlasSidecarMissing {
            cpk_path: cpk_path.to_string(),
            spc_member: spc_member.to_string(),
            sidecar_name: sidecar_name.clone(),
        }
    })?;

    if grew {
        grow_atlas(
            srd,
            rsi_path,
            &mut spc.entries[sidecar_idx].data,
            cur_w,
            cur_h,
            atlas_h,
            cpk_path,
            spc_member,
        )?;
        report.font_atlas_grows += 1;
    }

    // Validate every glyph's alpha-buffer geometry (against the possibly
    // grown extent) before mutating any bytes — fail-fast keeps the SPC
    // in a consistent state if one entry in a batch is malformed.
    for patch in &group.glyphs {
        validate_glyph_patch(patch, cpk_path, spc_member, atlas_w, atlas_h)?;
    }

    let sidecar = &mut spc.entries[sidecar_idx].data;
    for patch in &group.glyphs {
        let Some(alpha) = &patch.glyph_alpha8 else {
            continue;
        };
        let (w, h) = patch.size.expect("size validated");
        let pos = patch.position.unwrap_or((0, 0));
        blit_alpha_into_bc4(
            sidecar,
            usize::from(atlas_w),
            usize::from(atlas_h),
            usize::from(pos.0),
            usize::from(pos.1),
            usize::from(w),
            usize::from(h),
            alpha,
        );
        report.font_atlas_writes += 1;
    }
    Ok(())
}

/// BC4 byte count for a `width × height` atlas: `scanline × ceil(height /
/// 4)`, where `scanline = (width / 4) × 8` bytes per 4-px block-row.
fn bc4_byte_count(width: u16, height: u16) -> usize {
    let scanline = (usize::from(width) / 4) * 8;
    scanline * usize::from(height).div_ceil(4)
}

/// Grow a font's BC4 atlas in height. Width (and therefore `scanline`)
/// stays constant, so the existing block-rows map 1:1 to the head of the
/// enlarged buffer and only the appended rows need zeroing. Updates the
/// `.srdv` buffer, the `$TXR` display height, and the `$RSI` `ResourceInfo`
/// blob size in one coordinated step.
#[expect(
    clippy::too_many_arguments,
    reason = "coordinated single-purpose grow: sidecar buffer + old/new geometry + diagnostics"
)]
fn grow_atlas(
    srd: &mut Srd,
    rsi_path: &RsiPath,
    sidecar: &mut Vec<u8>,
    cur_w: u16,
    cur_h: u16,
    new_h: u16,
    cpk_path: &str,
    spc_member: &str,
) -> Result<(), TranslateError> {
    let old_len = bc4_byte_count(cur_w, cur_h);
    let new_len = bc4_byte_count(cur_w, new_h);

    // The sidecar must currently match the shipped geometry exactly;
    // otherwise our 1:1 row mapping (and the size we write into the
    // `ResourceInfo`) would be wrong.
    if sidecar.len() != old_len {
        return Err(TranslateError::AtlasGeometry {
            cpk_path: cpk_path.to_string(),
            spc_member: spc_member.to_string(),
            detail: format!(
                ".srdv is {} bytes but $TXR {cur_w}×{cur_h} implies {old_len}",
                sidecar.len()
            ),
        });
    }

    // $RSI `ResourceInfo`: bump the .srdv blob's recorded size (Value[1]).
    // Done first because it's the only fallible step that can't be
    // pre-validated above — fail before touching the buffer or $TXR.
    let rsi = rsi_data_mut(&mut srd.blocks, rsi_path);
    update_srdv_resource_size(rsi, new_len, cpk_path, spc_member)?;

    // Extend the BC4 buffer; new block-rows decode to 0 (transparent).
    sidecar.resize(new_len, 0);

    // $TXR height. scanline is unchanged because width is constant.
    if let Some(txr) = txr_data_mut(&mut srd.blocks) {
        txr.display_height = new_h;
    }

    Ok(())
}

/// Set the byte size (`Value[1]`) of the `$RSI` `ResourceInfo` entry that
/// points into the `.srdv` sidecar. The entry is identified by the SRDV
/// flag in its first value; trailing values are opaque and untouched.
fn update_srdv_resource_size(
    rsi: &mut RsiData,
    new_len: usize,
    cpk_path: &str,
    spc_member: &str,
) -> Result<(), TranslateError> {
    let new_len_u32 = u32::try_from(new_len).map_err(|_| TranslateError::AtlasGeometry {
        cpk_path: cpk_path.to_string(),
        spc_member: spc_member.to_string(),
        detail: format!("grown atlas size {new_len} exceeds u32"),
    })?;
    for entry in &mut rsi.resource_info_list {
        let Some(&first) = entry.first() else {
            continue;
        };
        if first & ResourceLocationFlags::SRDV.bits() != 0 {
            if entry.len() < 2 {
                continue;
            }
            entry[1] = new_len_u32;
            return Ok(());
        }
    }
    Err(TranslateError::AtlasSrdvResourceInfoMissing {
        cpk_path: cpk_path.to_string(),
        spc_member: spc_member.to_string(),
    })
}

fn validate_glyph_patch(
    patch: &FontGlyphPatch,
    cpk_path: &str,
    spc_member: &str,
    atlas_w: u16,
    atlas_h: u16,
) -> Result<(), TranslateError> {
    let Some(alpha) = &patch.glyph_alpha8 else {
        return Ok(());
    };
    let (w, h) = patch.size.ok_or_else(|| TranslateError::AtlasAlphaSize {
        cpk_path: cpk_path.to_string(),
        spc_member: spc_member.to_string(),
        codepoint: patch.codepoint,
        size: (0, 0),
        expected: 0,
        actual: alpha.len(),
    })?;
    let expected = usize::from(w) * usize::from(h);
    if alpha.len() != expected {
        return Err(TranslateError::AtlasAlphaSize {
            cpk_path: cpk_path.to_string(),
            spc_member: spc_member.to_string(),
            codepoint: patch.codepoint,
            size: (w, h),
            expected,
            actual: alpha.len(),
        });
    }
    let pos = patch.position.unwrap_or((0, 0));
    if usize::from(pos.0) + usize::from(w) > usize::from(atlas_w)
        || usize::from(pos.1) + usize::from(h) > usize::from(atlas_h)
    {
        return Err(TranslateError::AtlasOverflow {
            cpk_path: cpk_path.to_string(),
            spc_member: spc_member.to_string(),
            codepoint: patch.codepoint,
            position: pos,
            size: (w, h),
            atlas: (atlas_w, atlas_h),
        });
    }
    Ok(())
}

/// Path through `srd.blocks` to the SPFT-bearing `$RSI` block. Either
/// `Top(idx)` (the `$RSI` is a top-level block) or `InTxr(txr_idx,
/// child_idx)` (the `$RSI` is a child of the top-level `$TXR` at
/// `txr_idx`).
#[derive(Debug, Clone, Copy)]
enum RsiPath {
    Top(usize),
    InTxr(usize, usize),
}

fn find_spft_rsi(blocks: &[Block]) -> Option<RsiPath> {
    for (i, block) in blocks.iter().enumerate() {
        match block {
            Block::Rsi { rsi, .. } if rsi.resource_data.starts_with(SPFT_MAGIC) => {
                return Some(RsiPath::Top(i));
            }
            Block::Txr { children, .. } => {
                for (j, child) in children.iter().enumerate() {
                    if let Block::Rsi { rsi, .. } = child {
                        if rsi.resource_data.starts_with(SPFT_MAGIC) {
                            return Some(RsiPath::InTxr(i, j));
                        }
                    }
                }
            }
            _ => {}
        }
    }
    None
}

fn rsi_data_mut<'a>(blocks: &'a mut [Block], path: &RsiPath) -> &'a mut RsiData {
    match *path {
        RsiPath::Top(i) => {
            if let Block::Rsi { rsi, .. } = &mut blocks[i] {
                rsi
            } else {
                panic!("RsiPath::Top points at non-$RSI block")
            }
        }
        RsiPath::InTxr(i, j) => {
            if let Block::Txr { children, .. } = &mut blocks[i] {
                if let Block::Rsi { rsi, .. } = &mut children[j] {
                    rsi
                } else {
                    panic!("RsiPath::InTxr child is not $RSI")
                }
            } else {
                panic!("RsiPath::InTxr parent is not $TXR")
            }
        }
    }
}

fn rsi_resource_data_mut<'a>(blocks: &'a mut [Block], path: &RsiPath) -> &'a mut Vec<u8> {
    &mut rsi_data_mut(blocks, path).resource_data
}

/// First top-level `$TXR` block's metadata, if any. Font containers ship
/// exactly one texture block.
fn find_txr(blocks: &[Block]) -> Option<&TxrData> {
    blocks.iter().find_map(|block| match block {
        Block::Txr { txr, .. } => Some(txr),
        _ => None,
    })
}

/// Mutable counterpart to [`find_txr`].
fn txr_data_mut(blocks: &mut [Block]) -> Option<&mut TxrData> {
    blocks.iter_mut().find_map(|block| match block {
        Block::Txr { txr, .. } => Some(txr),
        _ => None,
    })
}

/// `v3_font00.stx` → `v3_font00.srdv`. If `name` doesn't end in `.stx`,
/// returns `name + ".srdv"` — robust against future producers that
/// might ship `.srd`-extension members directly.
fn sidecar_name_for(name: &str) -> String {
    if let Some(stem) = name.strip_suffix(".stx") {
        format!("{stem}.srdv")
    } else if let Some(stem) = name.strip_suffix(".srd") {
        format!("{stem}.srdv")
    } else {
        format!("{name}.srdv")
    }
}

fn find_member_by_name(spc: &Spc, name: &str) -> Option<usize> {
    spc.entries
        .iter()
        .position(|e| e.name_as_str() == Some(name))
}

fn apply_glyph_metadata(spft: &mut SpFt, patches: &[FontGlyphPatch], report: &mut PatchReport) {
    let mut idx: HashMap<u32, usize> = spft
        .glyphs
        .iter()
        .enumerate()
        .map(|(i, g)| (g.codepoint, i))
        .collect();

    for patch in patches {
        if let Some(i) = idx.get(&patch.codepoint).copied() {
            let existing = &mut spft.glyphs[i];
            let mut changed = false;
            if let Some((x, y)) = patch.position {
                if existing.position != (x, y) {
                    existing.position = (x, y);
                    changed = true;
                }
            }
            if let Some((w, h)) = patch.size {
                if existing.size != (w, h) {
                    existing.size = (w, h);
                    changed = true;
                }
            }
            if let Some((l, r, v)) = patch.kerning {
                if existing.kerning != (l, r, v) {
                    existing.kerning = (l, r, v);
                    changed = true;
                }
            }
            if changed {
                report.font_glyphs_changed += 1;
            }
        } else {
            let glyph = Glyph {
                codepoint: patch.codepoint,
                position: patch.position.unwrap_or((0, 0)),
                size: patch.size.unwrap_or((0, 0)),
                kerning: patch.kerning.unwrap_or((0, 0, 0)),
            };
            spft.glyphs.push(glyph);
            idx.insert(patch.codepoint, spft.glyphs.len() - 1);
            report.font_glyphs_added += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use drv3_spc::{COMPRESSION_STORED, SpcEntry};
    use drv3_spft::{Glyph, SpFt};
    use drv3_srd::texture::{decode_bc4, encode_bc4};
    use drv3_srd::{Block, RsiData, Srd, TxrData};

    fn build_spft_bytes() -> Vec<u8> {
        let spft = SpFt {
            unknown6: 6,
            bit_flag_count: 0xFF5F,
            scale_flag: 20,
            font_name: "Test".into(),
            glyphs: vec![
                Glyph {
                    codepoint: 32,
                    position: (0, 0),
                    size: (4, 8),
                    kerning: (0, 0, 0),
                },
                Glyph {
                    codepoint: 65,
                    position: (8, 0),
                    size: (6, 8),
                    kerning: (-1, 0, 0),
                },
            ],
        };
        spft.to_bytes()
    }

    /// Build a synthetic SRD containing $CFH, $TXR (32×16 atlas, format
    /// 0x16) with a child $RSI carrying our SPFT, terminator $CT0.
    fn build_srd_with_spft(spft_bytes: Vec<u8>) -> Vec<u8> {
        let rsi = RsiData {
            unknown_10: 0,
            unknown_11: 0,
            unknown_12: 0,
            fallback_resource_info_count: 0,
            resource_info_count: 0,
            fallback_resource_info_size: 0,
            resource_info_size: 0,
            unknown_1a: 0,
            resource_info_list: Vec::new(),
            resource_data: spft_bytes,
            resource_strings: Vec::new(),
        };
        let txr = TxrData {
            unknown_10: 0,
            swizzle: 1,
            display_width: 32,
            display_height: 16,
            scanline: (32 / 4) * 8,
            format: 0x16,
            unknown_1d: 0,
            palette: 0,
            palette_id: 0,
        };
        let srd = Srd {
            blocks: vec![
                Block::Cfh,
                Block::Txr {
                    txr,
                    children: vec![Block::Rsi {
                        rsi,
                        children: Vec::new(),
                    }],
                },
                Block::Ct0,
            ],
        };
        srd.to_bytes().unwrap()
    }

    fn make_spc_with_font(stx_bytes: Vec<u8>, srdv_bytes: Vec<u8>) -> Spc {
        Spc {
            unknown1: [0u8; 0x24],
            unknown2: 0,
            entries: vec![
                SpcEntry {
                    name: b"font.stx".to_vec(),
                    compression_flag: COMPRESSION_STORED,
                    unknown_flag: 0,
                    data: stx_bytes,
                },
                SpcEntry {
                    name: b"font.srdv".to_vec(),
                    compression_flag: COMPRESSION_STORED,
                    unknown_flag: 0,
                    data: srdv_bytes,
                },
            ],
        }
    }

    #[test]
    fn metadata_only_patch_changes_existing_glyph_and_reports_change() {
        let srd_bytes = build_srd_with_spft(build_spft_bytes());
        let srdv = encode_bc4(&vec![0u8; 32 * 16], 32, 16);
        let mut spc = make_spc_with_font(srd_bytes, srdv);
        let group = FontFileGroup {
            font_name: None,
            atlas: None,
            glyphs: vec![FontGlyphPatch {
                codepoint: 65,
                glyph_alpha8: None,
                position: Some((10, 5)),
                size: None,
                kerning: Some((-2, 1, 3)),
            }],
        };
        let mut report = PatchReport::default();
        patch_font_member(&mut spc, 0, "x.spc", "font.stx", &group, &mut report).unwrap();
        assert_eq!(report.font_glyphs_changed, 1);
        assert_eq!(report.font_atlas_writes, 0);

        let srd = Srd::parse(&spc.entries[0].data).unwrap();
        let rsi_data = if let Block::Txr { children, .. } = &srd.blocks[1] {
            if let Block::Rsi { rsi, .. } = &children[0] {
                rsi.resource_data.clone()
            } else {
                panic!("$RSI not child of $TXR");
            }
        } else {
            panic!("$TXR not at index 1");
        };
        let spft = SpFt::parse(&rsi_data).unwrap();
        let g = spft.glyphs.iter().find(|g| g.codepoint == 65).unwrap();
        assert_eq!(g.position, (10, 5));
        assert_eq!(g.kerning, (-2, 1, 3));
    }

    #[test]
    fn atlas_blit_writes_alpha_at_position_and_increments_counters() {
        let srd_bytes = build_srd_with_spft(build_spft_bytes());
        // Atlas is 32×16; encode all-zeros initially.
        let srdv = encode_bc4(&vec![0u8; 32 * 16], 32, 16);
        let mut spc = make_spc_with_font(srd_bytes, srdv);

        // Blit a 4×4 solid-255 glyph at (12, 4) — codepoint 0xE4 (ä).
        let group = FontFileGroup {
            font_name: None,
            atlas: None,
            glyphs: vec![FontGlyphPatch {
                codepoint: 0xE4,
                glyph_alpha8: Some(vec![255u8; 16]),
                position: Some((12, 4)),
                size: Some((4, 4)),
                kerning: Some((0, 0, 0)),
            }],
        };
        let mut report = PatchReport::default();
        patch_font_member(&mut spc, 0, "x.spc", "font.stx", &group, &mut report).unwrap();
        assert_eq!(report.font_glyphs_added, 1);
        assert_eq!(report.font_atlas_writes, 1);

        // Decode the sidecar and verify the pixels are where we put them.
        let decoded = decode_bc4(&spc.entries[1].data, 32, 16);
        for y in 4..8 {
            for x in 12..16 {
                assert_eq!(decoded[y * 32 + x], 255, "miss at ({x}, {y})");
            }
        }
        // Spot-check that an unrelated region is still 0.
        assert_eq!(decoded[0], 0);
        assert_eq!(decoded[31], 0);
    }

    #[test]
    fn atlas_overflow_is_caught() {
        let srd_bytes = build_srd_with_spft(build_spft_bytes());
        let srdv = encode_bc4(&vec![0u8; 32 * 16], 32, 16);
        let mut spc = make_spc_with_font(srd_bytes, srdv);

        let group = FontFileGroup {
            font_name: None,
            atlas: None,
            glyphs: vec![FontGlyphPatch {
                codepoint: 0xE4,
                glyph_alpha8: Some(vec![255u8; 16]),
                position: Some((30, 14)), // 30+4 > 32
                size: Some((4, 4)),
                kerning: None,
            }],
        };
        let mut report = PatchReport::default();
        let err =
            patch_font_member(&mut spc, 0, "x.spc", "font.stx", &group, &mut report).unwrap_err();
        assert!(matches!(err, TranslateError::AtlasOverflow { .. }));
    }

    #[test]
    fn alpha_size_mismatch_is_caught() {
        let srd_bytes = build_srd_with_spft(build_spft_bytes());
        let srdv = encode_bc4(&vec![0u8; 32 * 16], 32, 16);
        let mut spc = make_spc_with_font(srd_bytes, srdv);

        let group = FontFileGroup {
            font_name: None,
            atlas: None,
            glyphs: vec![FontGlyphPatch {
                codepoint: 0xE4,
                glyph_alpha8: Some(vec![255u8; 10]), // says 4×4 = 16 but provides 10
                position: Some((0, 0)),
                size: Some((4, 4)),
                kerning: None,
            }],
        };
        let mut report = PatchReport::default();
        let err =
            patch_font_member(&mut spc, 0, "x.spc", "font.stx", &group, &mut report).unwrap_err();
        assert!(matches!(err, TranslateError::AtlasAlphaSize { .. }));
    }

    #[test]
    fn missing_sidecar_is_caught() {
        let srd_bytes = build_srd_with_spft(build_spft_bytes());
        // Build an SPC WITHOUT the .srdv member.
        let mut spc = Spc {
            unknown1: [0u8; 0x24],
            unknown2: 0,
            entries: vec![SpcEntry {
                name: b"font.stx".to_vec(),
                compression_flag: COMPRESSION_STORED,
                unknown_flag: 0,
                data: srd_bytes,
            }],
        };
        let group = FontFileGroup {
            font_name: None,
            atlas: None,
            glyphs: vec![FontGlyphPatch {
                codepoint: 0xE4,
                glyph_alpha8: Some(vec![255u8; 16]),
                position: Some((0, 0)),
                size: Some((4, 4)),
                kerning: None,
            }],
        };
        let mut report = PatchReport::default();
        let err =
            patch_font_member(&mut spc, 0, "x.spc", "font.stx", &group, &mut report).unwrap_err();
        assert!(matches!(err, TranslateError::AtlasSidecarMissing { .. }));
    }

    #[test]
    fn sidecar_name_translation() {
        assert_eq!(sidecar_name_for("v3_font00.stx"), "v3_font00.srdv");
        assert_eq!(sidecar_name_for("font.srd"), "font.srdv");
        assert_eq!(sidecar_name_for("weird"), "weird.srdv");
    }

    // ---- Atlas-growth tests ----

    use crate::model::AtlasSpec;

    /// `.srdv` `ResourceInfo` flag marking the blob as living in the `.srdv`
    /// sidecar (mirrors `ResourceLocationFlags::SRDV`).
    const TEST_SRDV_FLAG: u32 = 0x4000_0000;

    /// Like [`build_srd_with_spft`] but the `$RSI` carries one SRDV
    /// `ResourceInfo` entry (8 × u32, `resource_info_size = 32`) whose
    /// `Value[1]` records the `.srdv` blob byte size — so atlas growth has
    /// an entry to update. `format` is the `$TXR` pixel format tag.
    fn build_srd_with_srdv_info(spft_bytes: Vec<u8>, w: u16, h: u16, srdv_len: u32, format: u8) -> Vec<u8> {
        let rsi = RsiData {
            unknown_10: 0,
            unknown_11: 0,
            unknown_12: 0,
            fallback_resource_info_count: 0,
            resource_info_count: 1,
            fallback_resource_info_size: 0,
            resource_info_size: 32,
            unknown_1a: 0,
            resource_info_list: vec![vec![TEST_SRDV_FLAG, srdv_len, 0x80, 0, 0, 0, 0, 0]],
            resource_data: spft_bytes,
            resource_strings: Vec::new(),
        };
        let txr = TxrData {
            unknown_10: 0,
            swizzle: 1,
            display_width: w,
            display_height: h,
            scanline: (w / 4) * 8,
            format,
            unknown_1d: 0,
            palette: 0,
            palette_id: 0,
        };
        let srd = Srd {
            blocks: vec![
                Block::Cfh,
                Block::Txr {
                    txr,
                    children: vec![Block::Rsi {
                        rsi,
                        children: Vec::new(),
                    }],
                },
                Block::Ct0,
            ],
        };
        srd.to_bytes().unwrap()
    }

    /// Read back the (TXR, first-RSI) pair from a serialized font `.stx`.
    fn parse_txr_and_rsi(stx: &[u8]) -> (TxrData, RsiData) {
        let srd = Srd::parse(stx).unwrap();
        let Block::Txr { txr, children } = &srd.blocks[1] else {
            panic!("$TXR not at index 1");
        };
        let Block::Rsi { rsi, .. } = &children[0] else {
            panic!("$RSI not child of $TXR");
        };
        (*txr, rsi.clone())
    }

    #[test]
    fn atlas_growth_updates_txr_srdv_and_resource_info() {
        // 32×16 atlas, scanline = (32/4)*8 = 64. old_len = 64*ceil(16/4) =
        // 256; grow to 32×32 → new_len = 64*ceil(32/4) = 512.
        let old_len = bc4_byte_count(32, 16) as u32;
        let srd_bytes = build_srd_with_srdv_info(build_spft_bytes(), 32, 16, old_len, 0x16);
        // Fill the original atlas with a recognizable value so we can
        // confirm the old block-rows survive the resize.
        let srdv = encode_bc4(&vec![7u8; 32 * 16], 32, 16);
        let mut spc = make_spc_with_font(srd_bytes, srdv);

        // New glyph in a row only the grown atlas has (y = 20).
        let group = FontFileGroup {
            font_name: None,
            atlas: Some(AtlasSpec {
                width: 32,
                height: 32,
            }),
            glyphs: vec![FontGlyphPatch {
                codepoint: 0xC4,
                glyph_alpha8: Some(vec![255u8; 16]),
                position: Some((12, 20)),
                size: Some((4, 4)),
                kerning: Some((0, 0, 0)),
            }],
        };
        let mut report = PatchReport::default();
        patch_font_member(&mut spc, 0, "x.spc", "font.stx", &group, &mut report).unwrap();

        assert_eq!(report.font_atlas_grows, 1);
        assert_eq!(report.font_atlas_writes, 1);
        assert_eq!(report.font_glyphs_added, 1);

        // .srdv re-allocated to the grown byte count.
        assert_eq!(spc.entries[1].data.len(), 512);

        let decoded = decode_bc4(&spc.entries[1].data, 32, 32);
        // New glyph landed in the grown region.
        for y in 20..24 {
            for x in 12..16 {
                assert_eq!(decoded[y * 32 + x], 255, "miss at ({x}, {y})");
            }
        }
        // Original block-rows survived 1:1 (value 7 from the fill).
        assert_eq!(decoded[0], 7);
        assert_eq!(decoded[15 * 32 + 31], 7);
        // Appended rows that no glyph touched are zero.
        assert_eq!(decoded[28 * 32], 0);

        // $TXR height grew, width unchanged; $RSI blob size bumped.
        let (txr, rsi) = parse_txr_and_rsi(&spc.entries[0].data);
        assert_eq!(txr.display_height, 32);
        assert_eq!(txr.display_width, 32);
        assert_eq!(rsi.resource_info_list[0][1], 512);
        // Opaque trailing value preserved.
        assert_eq!(rsi.resource_info_list[0][2], 0x80);
    }

    #[test]
    fn atlas_request_matching_shipped_dims_is_noop() {
        let old_len = bc4_byte_count(32, 16) as u32;
        let srd_bytes = build_srd_with_srdv_info(build_spft_bytes(), 32, 16, old_len, 0x16);
        let srdv = encode_bc4(&vec![0u8; 32 * 16], 32, 16);
        let mut spc = make_spc_with_font(srd_bytes, srdv);
        // Atlas restates current dims, no pixel glyphs → no growth, no writes.
        let group = FontFileGroup {
            font_name: None,
            atlas: Some(AtlasSpec {
                width: 32,
                height: 16,
            }),
            glyphs: vec![],
        };
        let mut report = PatchReport::default();
        patch_font_member(&mut spc, 0, "x.spc", "font.stx", &group, &mut report).unwrap();
        assert_eq!(report.font_atlas_grows, 0);
        assert_eq!(spc.entries[1].data.len(), 256);
    }

    #[test]
    fn atlas_growth_rejects_width_change() {
        let old_len = bc4_byte_count(32, 16) as u32;
        let srd_bytes = build_srd_with_srdv_info(build_spft_bytes(), 32, 16, old_len, 0x16);
        let srdv = encode_bc4(&vec![0u8; 32 * 16], 32, 16);
        let mut spc = make_spc_with_font(srd_bytes, srdv);
        let group = FontFileGroup {
            font_name: None,
            atlas: Some(AtlasSpec {
                width: 64,
                height: 32,
            }),
            glyphs: vec![],
        };
        let mut report = PatchReport::default();
        let err =
            patch_font_member(&mut spc, 0, "x.spc", "font.stx", &group, &mut report).unwrap_err();
        assert!(matches!(err, TranslateError::AtlasWidthChange { .. }));
    }

    #[test]
    fn atlas_growth_rejects_shrink() {
        let old_len = bc4_byte_count(32, 16) as u32;
        let srd_bytes = build_srd_with_srdv_info(build_spft_bytes(), 32, 16, old_len, 0x16);
        let srdv = encode_bc4(&vec![0u8; 32 * 16], 32, 16);
        let mut spc = make_spc_with_font(srd_bytes, srdv);
        let group = FontFileGroup {
            font_name: None,
            atlas: Some(AtlasSpec {
                width: 32,
                height: 8,
            }),
            glyphs: vec![],
        };
        let mut report = PatchReport::default();
        let err =
            patch_font_member(&mut spc, 0, "x.spc", "font.stx", &group, &mut report).unwrap_err();
        assert!(matches!(err, TranslateError::AtlasShrink { .. }));
    }

    #[test]
    fn atlas_growth_rejects_non_bc4() {
        let old_len = bc4_byte_count(32, 16) as u32;
        // format 0x01 is not BC4.
        let srd_bytes = build_srd_with_srdv_info(build_spft_bytes(), 32, 16, old_len, 0x01);
        let srdv = encode_bc4(&vec![0u8; 32 * 16], 32, 16);
        let mut spc = make_spc_with_font(srd_bytes, srdv);
        let group = FontFileGroup {
            font_name: None,
            atlas: Some(AtlasSpec {
                width: 32,
                height: 32,
            }),
            glyphs: vec![],
        };
        let mut report = PatchReport::default();
        let err =
            patch_font_member(&mut spc, 0, "x.spc", "font.stx", &group, &mut report).unwrap_err();
        assert!(matches!(err, TranslateError::AtlasUnsupportedFormat { .. }));
    }

    #[test]
    fn atlas_growth_without_srdv_resource_info_errors() {
        // build_srd_with_spft ships an empty resource_info_list.
        let srd_bytes = build_srd_with_spft(build_spft_bytes());
        let srdv = encode_bc4(&vec![0u8; 32 * 16], 32, 16);
        let mut spc = make_spc_with_font(srd_bytes, srdv);
        let group = FontFileGroup {
            font_name: None,
            atlas: Some(AtlasSpec {
                width: 32,
                height: 32,
            }),
            glyphs: vec![],
        };
        let mut report = PatchReport::default();
        let err =
            patch_font_member(&mut spc, 0, "x.spc", "font.stx", &group, &mut report).unwrap_err();
        assert!(matches!(
            err,
            TranslateError::AtlasSrdvResourceInfoMissing { .. }
        ));
    }
}
