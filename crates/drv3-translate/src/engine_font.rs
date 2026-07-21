//! Font-group patching: SPFT metadata + atlas pixel writes.
//!
//! # Font container layout (DR V3)
//!
//! Each font in the game is a pair of co-located SPC members:
//!
//! - `<name>.stx` — despite the misleading extension, this is an SRD
//!   file (`$CFH` magic at offset 0). The SPFT (glyph metadata) sits at
//!   the start of `$RSI.resource_data` inside the top-level `$TXR`
//!   block's children.
//! - `<name>.srdv` — the atlas pixel sidecar. The game ships it as BC4
//!   (format `0x16`, swizzle `0x01`, atlas width 4096 for `v3_font00` and
//!   2048 for the other 24 fonts, heights between 100 and 469), decoded
//!   through [`drv3_srd::texture`]. When we patch a font we re-emit it as
//!   uncompressed ARGB8888 (format `0x01`) — see step 5 below.
//!
//! # Patch modes
//!
//! Every font group declares a [`FontPatchMode`]:
//!
//! - **Merge** — additive. The shipped glyph table and shipped atlas pixels
//!   survive; the group's glyphs are layered on top.
//! - **Replace** — wholesale recreation, for fonts whose original typeface
//!   couldn't be sourced. The shipped glyph table is discarded and the atlas
//!   starts as a zeroed buffer, so the group's glyphs are the entire font.
//!   Nothing of the shipped font survives, which is why the shipped sidecar is
//!   never decoded in this mode.
//!
//! # Pipeline (per font group)
//!
//! 1. Parse the `<name>.stx` SPC member bytes as [`Srd`].
//! 2. Walk `srd.blocks` for a `$RSI` whose `resource_data` starts with
//!    `SpFt` magic.
//! 3. Parse the SPFT. In replace mode, discard its glyph table first. Then
//!    apply metadata edits (`position`, `size`, `kerning`) and add glyphs for
//!    new codepoints — in replace mode every glyph is new by construction.
//! 4. Build the atlas coverage buffer at the target extent. Merge decodes the
//!    shipped atlas (BC4, or ARGB8888 on re-apply) to a full-resolution alpha8
//!    buffer and lifts it into the (possibly taller) extent with new rows
//!    zeroed; replace starts from all-zero at the declared extent. Then copy
//!    each glyph's coverage in at full 8-bit precision (no BC4 re-encoding —
//!    that would band the anti-aliased edges). Under merge, original glyphs
//!    come straight from the decode, untouched.
//! 5. Re-emit the whole atlas as uncompressed ARGB8888 into the `<name>.srdv`
//!    SPC member, and update the `$TXR` format (→ `0x01`) and display height,
//!    plus the `$RSI` `ResourceInfo` blob size. `$TXR.scanline` is left at the
//!    shipped BC4 block-row pitch `width*2` (the engine reads it as the upload
//!    row stride) — which is also why atlas width is locked to the shipped
//!    width in both modes. Every other container field is preserved verbatim.
//! 6. Re-serialize SPFT → put back into `rsi.resource_data` →
//!    re-serialize SRD → write back to the `.stx` SPC entry.

use std::collections::HashMap;

use drv3_spc::Spc;
use drv3_spft::{Glyph, SpFt};
use drv3_srd::texture::{blit_alpha8, decode_argb8888_mono, decode_bc4, encode_argb8888_mono};
use drv3_srd::{Block, ResourceLocationFlags, RsiData, Srd, TxrData};

use crate::error::TranslateError;
use crate::model::{FontFileGroup, FontGlyphPatch, FontPatchMode};
use crate::report::PatchReport;

/// BC4 pixel format tag in `$TXR.format` — the format the game ships font
/// atlases in. We decode it but never re-encode it; patched atlases are
/// re-emitted as [`TXR_FORMAT_ARGB8888`].
const TXR_FORMAT_BC4: u8 = 0x16;

/// Uncompressed 32-bit `ARGB8888` tag in `$TXR.format`. Patched font atlases
/// are re-emitted in this format so the anti-aliased coverage gradient is
/// stored at full 8-bit precision — BC4 block compression bands the soft
/// edges, ARGB8888 keeps them bit-for-bit. Mono replication (coverage → all
/// four channels) makes channel-order interpretation irrelevant.
const TXR_FORMAT_ARGB8888: u8 = 0x01;

const SPFT_MAGIC: &[u8; 4] = b"SpFt";

/// BC4 compresses the atlas in 4×4-pixel blocks, 8 bytes per block.
const BC4_BLOCK_DIM: usize = 4;
const BC4_BYTES_PER_BLOCK: usize = 8;
/// ARGB8888 stores four bytes (A, R, G, B) per pixel.
const ARGB_BYTES_PER_PIXEL: usize = 4;
/// `$RSI` `ResourceInfo` slot holding a resource's byte size (`Value[1]`).
const RSI_RESOURCE_SIZE_SLOT: usize = 1;

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

    // Replace mode: drop the shipped glyph table so the group's glyphs are the
    // complete font. Everything else about the SPFT survives — `unknown6`,
    // `scale_flag`, and `font_name` are container/engine fields, not content.
    // `bit_flag_count` also stays at the shipped 0xFF5F even when the
    // replacement set is smaller: `SpFt::to_bytes` only ever grows it (see
    // `drv3-spft/src/lib.rs`, `effective_bit_flag_count`), so the flag table
    // keeps its shipped size. That costs ~8 KB of mostly-zero bytes and is
    // self-consistent, since the reader only seeks entries whose bit is set.
    if let FontPatchMode::Replace { .. } = group.mode {
        report.font_glyphs_removed += spft.glyphs.len();
        spft.glyphs.clear();
    }

    // With the table cleared, every glyph below takes the "added" path.
    apply_glyph_metadata(&mut spft, &group.glyphs, report);

    // Re-serialize SPFT and put back into the SRD before we move on
    // to the atlas (the .stx write happens after atlas writes complete).
    let new_resource_data = spft.to_bytes().map_err(|e| TranslateError::Spft {
        cpk_path: cpk_path.to_string(),
        spc_member: spc_member.to_string(),
        source: e,
    })?;
    *rsi_resource_data_mut(&mut srd.blocks, &rsi_path) = new_resource_data;

    // ---- Atlas side: rebuild or grow the atlas and blit glyphs into the
    // .srdv sidecar SPC member. ----
    //
    // Replace mode always runs: its whole point is to discard the shipped
    // pixels, which requires rewriting the sidecar even if the group carried
    // no glyph images at all. Don't "simplify" this back to the merge-only
    // condition — a replace that skipped `patch_atlas` would leave shipped ink
    // sitting under a replaced glyph table, silently and with no error.
    let atlas_work = match group.mode {
        FontPatchMode::Replace { .. } => true,
        FontPatchMode::Merge { atlas } => {
            atlas.is_some() || group.glyphs.iter().any(|g| g.glyph_alpha8.is_some())
        }
    };
    if atlas_work {
        patch_atlas(
            spc, &mut srd, &rsi_path, cpk_path, spc_member, group, report,
        )?;
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

    let (atlas_w, atlas_h) = target_extent(group.mode, cur_w, cur_h, cpk_path, spc_member)?;
    let any_alpha = group.glyphs.iter().any(|g| g.glyph_alpha8.is_some());

    // Merge with nothing to write into the sidecar: no growth and no pixels.
    // (Reached when an `atlas` block merely restates the shipped dimensions.)
    // Deliberately merge-only — see the `atlas_work` comment in
    // `patch_font_member`.
    if matches!(group.mode, FontPatchMode::Merge { .. }) && atlas_h == cur_h && !any_alpha {
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

    // Validate every glyph's alpha-buffer geometry (against the possibly
    // grown extent) before touching any bytes — fail-fast keeps the SPC in a
    // consistent state if one entry in a batch is malformed.
    for patch in &group.glyphs {
        validate_glyph_patch(patch, cpk_path, spc_member, atlas_w, atlas_h)?;
    }

    // Build the starting coverage buffer at the target extent. We never
    // re-encode BC4 (it would band the anti-aliased edges); the whole atlas is
    // rebuilt here and re-emitted uncompressed below.
    let mut pixels = match group.mode {
        // Replace: start from all-zero (transparent). The shipped sidecar is
        // never decoded, so `AtlasUnsupportedFormat` can't fire on this path —
        // a font whose `$TXR.format` we can't read can still be rebuilt.
        FontPatchMode::Replace { .. } => {
            report.font_atlas_replaces += 1;
            vec![0u8; usize::from(atlas_w) * usize::from(atlas_h)]
        }
        // Merge: decode the shipped atlas, then lift it into the (possibly
        // taller) extent. Width is fixed, so rows map 1:1 and the appended
        // rows start zeroed.
        FontPatchMode::Merge { .. } => {
            let decoded = decode_atlas_to_alpha8(
                &spc.entries[sidecar_idx].data,
                cur_w,
                cur_h,
                fmt,
                cpk_path,
                spc_member,
            )?;
            if atlas_h > cur_h {
                let mut grown = vec![0u8; usize::from(atlas_w) * usize::from(atlas_h)];
                grown[..decoded.len()].copy_from_slice(&decoded);
                report.font_atlas_grows += 1;
                grown
            } else {
                decoded
            }
        }
    };

    // Blit each glyph's coverage in — a plain overwrite at full 8-bit precision.
    for patch in &group.glyphs {
        let Some(alpha) = &patch.glyph_alpha8 else {
            continue;
        };
        // invariant: size is `Some` — `validate_glyph_patch` rejects any glyph
        // that carries `glyph_alpha8` without a `size`.
        let (w, h) = patch.size.expect("size validated");
        let pos = patch.position.unwrap_or((0, 0));
        blit_alpha8(
            &mut pixels,
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

    // Re-emit the whole atlas as uncompressed ARGB8888: every coverage value
    // is preserved exactly, so patched glyph edges stay smooth.
    let argb = encode_argb8888_mono(&pixels);
    let new_len = argb.len();

    // $RSI: record the new .srdv blob size. Fallible — do every fallible step
    // before mutating the sidecar bytes or $TXR so a failure leaves the SPC
    // untouched.
    let rsi = rsi_data_mut(&mut srd.blocks, rsi_path);
    update_srdv_resource_size(rsi, new_len, cpk_path, spc_member)?;

    // Commit: swap in the new pixel buffer and switch $TXR to ARGB8888 with the
    // new height. `$TXR.scanline` is left unchanged: the engine reads it as the
    // texture's upload row stride and expects the shipped BC4 block-row pitch
    // `(width/4)*8 == width*2`, not the 32-bpp `width*4`. Atlas growth only
    // changes height, so the width-derived pitch stays valid. All other
    // $TXR/$RSI fields stay verbatim.
    spc.entries[sidecar_idx].data = argb;
    if let Some(txr) = txr_data_mut(&mut srd.blocks) {
        txr.format = TXR_FORMAT_ARGB8888;
        txr.display_height = atlas_h;
    }

    Ok(())
}

/// Resolve the atlas extent the group is asking for, validating the request
/// against the shipped geometry.
///
/// Both modes lock width to the shipped `$TXR.display_width`: `patch_atlas`
/// leaves `$TXR.scanline` at the shipped BC4 block-row pitch `width*2` (the
/// engine reads it as the upload row stride), so only a width-preserving
/// rewrite keeps it valid. They differ on height — merge may only grow (rows
/// that surviving glyphs reference must not be dropped), replace may pick any
/// nonzero height because nothing of the shipped atlas survives.
///
/// Runs before any allocation, so the ARGB size check here also bounds the
/// buffers `patch_atlas` is about to build from a producer-supplied `u16`.
fn target_extent(
    mode: FontPatchMode,
    cur_w: u16,
    cur_h: u16,
    cpk_path: &str,
    spc_member: &str,
) -> Result<(u16, u16), TranslateError> {
    let (atlas_w, atlas_h) = match mode {
        FontPatchMode::Merge { atlas: None } => (cur_w, cur_h),
        FontPatchMode::Merge {
            atlas: Some(requested),
        } => {
            check_atlas_width(requested.width, cur_w, cpk_path, spc_member)?;
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
        FontPatchMode::Replace { atlas } => {
            check_atlas_width(atlas.width, cur_w, cpk_path, spc_member)?;
            // Replace may shrink, but not to nothing: a zero-height atlas
            // would serialize as an empty sidecar and silently destroy the
            // font rather than rebuilding it.
            if atlas.height == 0 {
                return Err(TranslateError::AtlasGeometry {
                    cpk_path: cpk_path.to_string(),
                    spc_member: spc_member.to_string(),
                    detail: "replace mode requires a nonzero atlas height".into(),
                });
            }
            (cur_w, atlas.height)
        }
    };

    // The re-emitted ARGB8888 blob is 4 bytes per pixel and its size has to
    // fit the `$RSI` `ResourceInfo` u32 slot. Checking here — before
    // `patch_atlas` allocates — keeps a producer-supplied height from driving
    // a ~1 GB allocation that we would only reject afterwards.
    let argb_len = usize::from(atlas_w) * usize::from(atlas_h) * ARGB_BYTES_PER_PIXEL;
    if u32::try_from(argb_len).is_err() {
        return Err(TranslateError::AtlasGeometry {
            cpk_path: cpk_path.to_string(),
            spc_member: spc_member.to_string(),
            detail: format!("atlas {atlas_w}×{atlas_h} needs {argb_len} bytes, which exceeds u32"),
        });
    }

    Ok((atlas_w, atlas_h))
}

/// Atlas width is locked to the shipped width in both patch modes — see
/// [`target_extent`].
fn check_atlas_width(
    requested: u16,
    current: u16,
    cpk_path: &str,
    spc_member: &str,
) -> Result<(), TranslateError> {
    if requested != current {
        return Err(TranslateError::AtlasWidthChange {
            cpk_path: cpk_path.to_string(),
            spc_member: spc_member.to_string(),
            requested,
            current,
        });
    }
    Ok(())
}

/// BC4 byte count for a `width × height` atlas: `scanline × ceil(height /
/// 4)`, where `scanline = (width / 4) × 8` bytes per 4-px block-row.
fn bc4_byte_count(width: u16, height: u16) -> usize {
    let scanline = (usize::from(width) / BC4_BLOCK_DIM) * BC4_BYTES_PER_BLOCK;
    scanline * usize::from(height).div_ceil(BC4_BLOCK_DIM)
}

/// Decode the shipped atlas sidecar into a full-resolution single-channel
/// alpha8 coverage buffer (`width * height` bytes, row-major), dispatching on
/// the `$TXR` format. BC4 (the shipped format) and ARGB8888 (a previously
/// patched atlas — keeps re-apply idempotent) are supported; anything else is
/// rejected. The sidecar length is validated against the format + geometry.
///
/// Only reached in [`FontPatchMode::Merge`] — a replace discards the shipped
/// pixels, so it has nothing to decode.
fn decode_atlas_to_alpha8(
    sidecar: &[u8],
    width: u16,
    height: u16,
    fmt: u8,
    cpk_path: &str,
    spc_member: &str,
) -> Result<Vec<u8>, TranslateError> {
    let (wz, hz) = (usize::from(width), usize::from(height));
    let check = |need: usize| -> Result<(), TranslateError> {
        if sidecar.len() == need {
            Ok(())
        } else {
            Err(TranslateError::AtlasGeometry {
                cpk_path: cpk_path.to_string(),
                spc_member: spc_member.to_string(),
                detail: format!(
                    ".srdv is {} bytes but $TXR {width}×{height} format {fmt:#04x} implies {need}",
                    sidecar.len()
                ),
            })
        }
    };
    match fmt {
        TXR_FORMAT_BC4 => {
            check(bc4_byte_count(width, height))?;
            Ok(decode_bc4(sidecar, wz, hz))
        }
        TXR_FORMAT_ARGB8888 => {
            check(wz * hz * ARGB_BYTES_PER_PIXEL)?;
            Ok(decode_argb8888_mono(sidecar, wz, hz))
        }
        _ => Err(TranslateError::AtlasUnsupportedFormat {
            cpk_path: cpk_path.to_string(),
            spc_member: spc_member.to_string(),
            format: fmt,
        }),
    }
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
            if entry.len() <= RSI_RESOURCE_SIZE_SLOT {
                continue;
            }
            entry[RSI_RESOURCE_SIZE_SLOT] = new_len_u32;
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
                    if let Block::Rsi { rsi, .. } = child
                        && rsi.resource_data.starts_with(SPFT_MAGIC)
                    {
                        return Some(RsiPath::InTxr(i, j));
                    }
                }
            }
            _ => {}
        }
    }
    None
}

/// Resolve an [`RsiPath`] to its `$RSI` data. The path was produced by an
/// earlier block-walk that already confirmed the block types, so the `panic!`
/// arms below are unreachable-bug guards, not input validation.
fn rsi_data_mut<'a>(blocks: &'a mut [Block], path: &RsiPath) -> &'a mut RsiData {
    match *path {
        RsiPath::Top(i) => {
            if let Block::Rsi { rsi, .. } = &mut blocks[i] {
                rsi
            } else {
                // invariant: RsiPath::Top only ever indexes an $RSI block.
                panic!("RsiPath::Top points at non-$RSI block")
            }
        }
        RsiPath::InTxr(i, j) => {
            if let Block::Txr { children, .. } = &mut blocks[i] {
                if let Block::Rsi { rsi, .. } = &mut children[j] {
                    rsi
                } else {
                    // invariant: RsiPath::InTxr's child index only ever points at $RSI.
                    panic!("RsiPath::InTxr child is not $RSI")
                }
            } else {
                // invariant: RsiPath::InTxr's parent index only ever points at $TXR.
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

/// `v3_font00.stx` → `v3_font00.srdv`. The `.stx` / `.srd` suffix is matched
/// case-insensitively (so `.STX` maps too) while the stem keeps its original
/// casing. If `name` has neither suffix it becomes `name + ".srdv"` — robust
/// against future producers that ship `.srd`-extension members directly.
fn sidecar_name_for(name: &str) -> String {
    // `.stx` and `.srd` are both 4 ASCII bytes; compare the tail on raw bytes
    // so the check is case-insensitive and panic-free on non-ASCII names. When
    // it matches, `name.len() - 4` is an ASCII (char) boundary, so slicing the
    // stem is safe and preserves its original casing.
    let tail = name.as_bytes();
    let has_font_suffix = tail.len() >= 4
        && (tail[tail.len() - 4..].eq_ignore_ascii_case(b".stx")
            || tail[tail.len() - 4..].eq_ignore_ascii_case(b".srd"));
    if has_font_suffix {
        format!("{}.srdv", &name[..name.len() - 4])
    } else {
        format!("{name}.srdv")
    }
}

// Member names are matched exactly (case-sensitive): the patch JSON's
// `spc_member` and the sidecar name derived by `sidecar_name_for` must equal
// the on-disk entry name byte-for-byte. DR V3 ships lowercase names, and
// `sidecar_name_for` preserves the member's casing, so the derived `.srdv`
// sibling lines up with what the SPC actually carries.
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
            if let Some((x, y)) = patch.position
                && existing.position != (x, y)
            {
                existing.position = (x, y);
                changed = true;
            }
            if let Some((w, h)) = patch.size
                && existing.size != (w, h)
            {
                existing.size = (w, h);
                changed = true;
            }
            if let Some((l, r, v)) = patch.kerning
                && existing.kerning != (l, r, v)
            {
                existing.kerning = (l, r, v);
                changed = true;
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
    use crate::model::AtlasSpec;
    use drv3_spc::{COMPRESSION_STORED, SpcEntry};
    use drv3_spft::{Glyph, SpFt};
    use drv3_srd::texture::{decode_argb8888_mono, encode_argb8888_mono};
    use drv3_srd::{Block, RsiData, Srd, TxrData};

    /// A `width × height` BC4 atlas filled with a single coverage `value`:
    /// every 4×4 block is `[value, value, 0, 0, 0, 0, 0, 0]` (`r0 == r1`,
    /// all indices 0), which `decode_bc4` reads back as a uniform `value`.
    /// Stands in for a shipped source atlas in the patch tests.
    fn uniform_bc4(value: u8, width: usize, height: usize) -> Vec<u8> {
        let blocks = width.div_ceil(4) * height.div_ceil(4);
        std::iter::repeat_n([value, value, 0, 0, 0, 0, 0, 0], blocks)
            .flatten()
            .collect()
    }

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
        spft.to_bytes().unwrap()
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

    fn atlas(width: u16, height: u16) -> AtlasSpec {
        AtlasSpec { width, height }
    }

    /// A merge-mode group with no `font_name` override — what almost every
    /// test here wants.
    fn merge_group(atlas: Option<AtlasSpec>, glyphs: Vec<FontGlyphPatch>) -> FontFileGroup {
        FontFileGroup {
            mode: FontPatchMode::Merge { atlas },
            font_name: None,
            glyphs,
        }
    }

    /// A replace-mode group. The atlas is mandatory here, not `Option`:
    /// replace rebuilds from nothing and has no shipped extent to inherit.
    fn replace_group(atlas: AtlasSpec, glyphs: Vec<FontGlyphPatch>) -> FontFileGroup {
        FontFileGroup {
            mode: FontPatchMode::Replace { atlas },
            font_name: None,
            glyphs,
        }
    }

    #[test]
    fn metadata_only_patch_changes_existing_glyph_and_reports_change() {
        let srd_bytes = build_srd_with_spft(build_spft_bytes());
        let srdv = uniform_bc4(0, 32, 16);
        let mut spc = make_spc_with_font(srd_bytes, srdv);
        let group = merge_group(
            None,
            vec![FontGlyphPatch {
                codepoint: 65,
                glyph_alpha8: None,
                position: Some((10, 5)),
                size: None,
                kerning: Some((-2, 1, 3)),
            }],
        );
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
    fn atlas_blit_writes_alpha_at_position_and_emits_argb8888() {
        // Atlas is 32×16 BC4; a glyph-only patch re-emits it as ARGB8888, so
        // the `.srdv` size changes and the $RSI SRDV entry must be present.
        let old_len = bc4_byte_count(32, 16) as u32;
        let srd_bytes = build_srd_with_srdv_info(build_spft_bytes(), 32, 16, old_len, 0x16);
        let srdv = uniform_bc4(0, 32, 16);
        let mut spc = make_spc_with_font(srd_bytes, srdv);

        // Blit a 4×4 solid-255 glyph at (12, 4) — codepoint 0xE4 (ä).
        let group = merge_group(
            None,
            vec![FontGlyphPatch {
                codepoint: 0xE4,
                glyph_alpha8: Some(vec![255u8; 16]),
                position: Some((12, 4)),
                size: Some((4, 4)),
                kerning: Some((0, 0, 0)),
            }],
        );
        let mut report = PatchReport::default();
        patch_font_member(&mut spc, 0, "x.spc", "font.stx", &group, &mut report).unwrap();
        assert_eq!(report.font_glyphs_added, 1);
        assert_eq!(report.font_atlas_writes, 1);

        // Sidecar is now uncompressed ARGB8888: 32×16×4 bytes.
        assert_eq!(spc.entries[1].data.len(), 32 * 16 * 4);
        let decoded = decode_argb8888_mono(&spc.entries[1].data, 32, 16);
        for y in 4..8 {
            for x in 12..16 {
                assert_eq!(decoded[y * 32 + x], 255, "miss at ({x}, {y})");
            }
        }
        assert_eq!(decoded[0], 0);
        assert_eq!(decoded[31], 0);

        // $TXR switched to ARGB8888; scanline left at the shipped BC4 pitch w*2.
        let (txr, rsi) = parse_txr_and_rsi(&spc.entries[0].data);
        assert_eq!(txr.format, 0x01);
        assert_eq!(txr.scanline, (32 / 4) * 8); // preserved BC4 pitch = width * 2
        assert_eq!(txr.display_width, 32);
        assert_eq!(txr.display_height, 16);
        assert_eq!(rsi.resource_info_list[0][1], 32 * 16 * 4);
    }

    #[test]
    fn atlas_overflow_is_caught() {
        let srd_bytes = build_srd_with_spft(build_spft_bytes());
        let srdv = uniform_bc4(0, 32, 16);
        let mut spc = make_spc_with_font(srd_bytes, srdv);

        let group = merge_group(
            None,
            vec![FontGlyphPatch {
                codepoint: 0xE4,
                glyph_alpha8: Some(vec![255u8; 16]),
                position: Some((30, 14)), // 30+4 > 32
                size: Some((4, 4)),
                kerning: None,
            }],
        );
        let mut report = PatchReport::default();
        let err =
            patch_font_member(&mut spc, 0, "x.spc", "font.stx", &group, &mut report).unwrap_err();
        assert!(matches!(err, TranslateError::AtlasOverflow { .. }));
    }

    #[test]
    fn alpha_size_mismatch_is_caught() {
        let srd_bytes = build_srd_with_spft(build_spft_bytes());
        let srdv = uniform_bc4(0, 32, 16);
        let mut spc = make_spc_with_font(srd_bytes, srdv);

        let group = merge_group(
            None,
            vec![FontGlyphPatch {
                codepoint: 0xE4,
                glyph_alpha8: Some(vec![255u8; 10]), // says 4×4 = 16 but provides 10
                position: Some((0, 0)),
                size: Some((4, 4)),
                kerning: None,
            }],
        );
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
        let group = merge_group(
            None,
            vec![FontGlyphPatch {
                codepoint: 0xE4,
                glyph_alpha8: Some(vec![255u8; 16]),
                position: Some((0, 0)),
                size: Some((4, 4)),
                kerning: None,
            }],
        );
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
        // Suffix match is case-insensitive; the stem keeps its casing.
        assert_eq!(sidecar_name_for("V3_FONT00.STX"), "V3_FONT00.srdv");
        assert_eq!(sidecar_name_for("Font.SRD"), "Font.srdv");
    }

    // ---- Atlas-growth and atlas-replace tests ----

    /// `.srdv` `ResourceInfo` flag marking the blob as living in the `.srdv`
    /// sidecar (mirrors `ResourceLocationFlags::SRDV`).
    const TEST_SRDV_FLAG: u32 = 0x4000_0000;

    /// Like [`build_srd_with_spft`] but the `$RSI` carries one SRDV
    /// `ResourceInfo` entry (8 × u32, `resource_info_size = 32`) whose
    /// `Value[1]` records the `.srdv` blob byte size — so atlas growth has
    /// an entry to update. `format` is the `$TXR` pixel format tag.
    fn build_srd_with_srdv_info(
        spft_bytes: Vec<u8>,
        w: u16,
        h: u16,
        srdv_len: u32,
        format: u8,
    ) -> Vec<u8> {
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
        let srdv = uniform_bc4(7, 32, 16);
        let mut spc = make_spc_with_font(srd_bytes, srdv);

        // New glyph in a row only the grown atlas has (y = 20).
        let group = merge_group(
            Some(atlas(32, 32)),
            vec![FontGlyphPatch {
                codepoint: 0xC4,
                glyph_alpha8: Some(vec![255u8; 16]),
                position: Some((12, 20)),
                size: Some((4, 4)),
                kerning: Some((0, 0, 0)),
            }],
        );
        let mut report = PatchReport::default();
        patch_font_member(&mut spc, 0, "x.spc", "font.stx", &group, &mut report).unwrap();

        assert_eq!(report.font_atlas_grows, 1);
        assert_eq!(report.font_atlas_writes, 1);
        assert_eq!(report.font_glyphs_added, 1);

        // .srdv re-allocated to the grown ARGB8888 byte count.
        assert_eq!(spc.entries[1].data.len(), 32 * 32 * 4);

        let decoded = decode_argb8888_mono(&spc.entries[1].data, 32, 32);
        // New glyph landed in the grown region.
        for y in 20..24 {
            for x in 12..16 {
                assert_eq!(decoded[y * 32 + x], 255, "miss at ({x}, {y})");
            }
        }
        // Original rows survived 1:1 (value 7 from the fill).
        assert_eq!(decoded[0], 7);
        assert_eq!(decoded[15 * 32 + 31], 7);
        // Appended rows that no glyph touched are zero.
        assert_eq!(decoded[28 * 32], 0);

        // $TXR switched to ARGB8888, height grew, width unchanged; scanline is
        // left at the shipped BC4 pitch w*2 (width-derived, unaffected by the
        // height growth); $RSI blob size bumped.
        let (txr, rsi) = parse_txr_and_rsi(&spc.entries[0].data);
        assert_eq!(txr.format, 0x01);
        assert_eq!(txr.scanline, (32 / 4) * 8); // preserved BC4 pitch = width * 2
        assert_eq!(txr.display_height, 32);
        assert_eq!(txr.display_width, 32);
        assert_eq!(rsi.resource_info_list[0][1], 32 * 32 * 4);
        // Opaque trailing value preserved.
        assert_eq!(rsi.resource_info_list[0][2], 0x80);
    }

    #[test]
    fn atlas_request_matching_shipped_dims_is_noop() {
        let old_len = bc4_byte_count(32, 16) as u32;
        let srd_bytes = build_srd_with_srdv_info(build_spft_bytes(), 32, 16, old_len, 0x16);
        let srdv = uniform_bc4(0, 32, 16);
        let mut spc = make_spc_with_font(srd_bytes, srdv);
        // Atlas restates current dims, no pixel glyphs → no growth, no writes.
        let group = merge_group(Some(atlas(32, 16)), vec![]);
        let mut report = PatchReport::default();
        patch_font_member(&mut spc, 0, "x.spc", "font.stx", &group, &mut report).unwrap();
        assert_eq!(report.font_atlas_grows, 0);
        assert_eq!(spc.entries[1].data.len(), 256);
    }

    #[test]
    fn atlas_growth_rejects_width_change() {
        let old_len = bc4_byte_count(32, 16) as u32;
        let srd_bytes = build_srd_with_srdv_info(build_spft_bytes(), 32, 16, old_len, 0x16);
        let srdv = uniform_bc4(0, 32, 16);
        let mut spc = make_spc_with_font(srd_bytes, srdv);
        let group = merge_group(Some(atlas(64, 32)), vec![]);
        let mut report = PatchReport::default();
        let err =
            patch_font_member(&mut spc, 0, "x.spc", "font.stx", &group, &mut report).unwrap_err();
        assert!(matches!(err, TranslateError::AtlasWidthChange { .. }));
    }

    #[test]
    fn atlas_growth_rejects_shrink() {
        let old_len = bc4_byte_count(32, 16) as u32;
        let srd_bytes = build_srd_with_srdv_info(build_spft_bytes(), 32, 16, old_len, 0x16);
        let srdv = uniform_bc4(0, 32, 16);
        let mut spc = make_spc_with_font(srd_bytes, srdv);
        let group = merge_group(Some(atlas(32, 8)), vec![]);
        let mut report = PatchReport::default();
        let err =
            patch_font_member(&mut spc, 0, "x.spc", "font.stx", &group, &mut report).unwrap_err();
        assert!(matches!(err, TranslateError::AtlasShrink { .. }));
    }

    #[test]
    fn atlas_rejects_unsupported_format() {
        let old_len = bc4_byte_count(32, 16) as u32;
        // 0x11 (DXT5) is neither the shipped BC4 nor our ARGB8888 output.
        let srd_bytes = build_srd_with_srdv_info(build_spft_bytes(), 32, 16, old_len, 0x11);
        let srdv = uniform_bc4(0, 32, 16);
        let mut spc = make_spc_with_font(srd_bytes, srdv);
        let group = merge_group(Some(atlas(32, 32)), vec![]);
        let mut report = PatchReport::default();
        let err =
            patch_font_member(&mut spc, 0, "x.spc", "font.stx", &group, &mut report).unwrap_err();
        assert!(matches!(err, TranslateError::AtlasUnsupportedFormat { .. }));
    }

    #[test]
    fn atlas_reapply_to_argb8888_is_accepted_and_lossless() {
        // Re-applying to an already-patched (ARGB8888) atlas must work, and a
        // smooth gradient glyph must survive bit-for-bit.
        let argb_len = 32 * 16 * 4;
        let srd_bytes = build_srd_with_srdv_info(build_spft_bytes(), 32, 16, argb_len as u32, 0x01);
        let srdv = encode_argb8888_mono(&vec![0u8; 32 * 16]);
        let mut spc = make_spc_with_font(srd_bytes, srdv);

        let gradient: Vec<u8> = (0..16).map(|i| (i * 17) as u8).collect();
        let group = merge_group(
            None,
            vec![FontGlyphPatch {
                codepoint: 0xF6,
                glyph_alpha8: Some(gradient.clone()),
                position: Some((0, 0)),
                size: Some((4, 4)),
                kerning: Some((0, 0, 0)),
            }],
        );
        let mut report = PatchReport::default();
        patch_font_member(&mut spc, 0, "x.spc", "font.stx", &group, &mut report).unwrap();

        let decoded = decode_argb8888_mono(&spc.entries[1].data, 32, 16);
        for row in 0..4 {
            for col in 0..4 {
                assert_eq!(
                    decoded[row * 32 + col],
                    gradient[row * 4 + col],
                    "gradient pixel ({col}, {row}) not preserved"
                );
            }
        }
        let (txr, _) = parse_txr_and_rsi(&spc.entries[0].data);
        assert_eq!(txr.format, 0x01);
    }

    #[test]
    fn atlas_growth_without_srdv_resource_info_errors() {
        // build_srd_with_spft ships an empty resource_info_list.
        let srd_bytes = build_srd_with_spft(build_spft_bytes());
        let srdv = uniform_bc4(0, 32, 16);
        let mut spc = make_spc_with_font(srd_bytes, srdv);
        let group = merge_group(Some(atlas(32, 32)), vec![]);
        let mut report = PatchReport::default();
        let err =
            patch_font_member(&mut spc, 0, "x.spc", "font.stx", &group, &mut report).unwrap_err();
        assert!(matches!(
            err,
            TranslateError::AtlasSrdvResourceInfoMissing { .. }
        ));
    }

    // ---- Replace-mode tests ----

    /// A 4×4 fully-opaque glyph at `pos` — the standard replace-mode payload
    /// for these tests.
    fn solid_glyph(codepoint: u32, pos: (u16, u16)) -> FontGlyphPatch {
        FontGlyphPatch {
            codepoint,
            glyph_alpha8: Some(vec![255u8; 16]),
            position: Some(pos),
            size: Some((4, 4)),
            kerning: Some((0, 0, 0)),
        }
    }

    #[test]
    fn replace_drops_shipped_glyphs() {
        let old_len = bc4_byte_count(32, 16) as u32;
        let srd_bytes = build_srd_with_srdv_info(build_spft_bytes(), 32, 16, old_len, 0x16);
        let mut spc = make_spc_with_font(srd_bytes, uniform_bc4(0, 32, 16));

        // The fixture ships codepoints 32 and 65; replace lists only 0xC4, so
        // a surviving shipped glyph would be visible in the table.
        let group = replace_group(atlas(32, 16), vec![solid_glyph(0xC4, (12, 4))]);
        let mut report = PatchReport::default();
        patch_font_member(&mut spc, 0, "x.spc", "font.stx", &group, &mut report).unwrap();

        assert_eq!(report.font_glyphs_removed, 2);
        assert_eq!(report.font_glyphs_added, 1);
        assert_eq!(report.font_glyphs_changed, 0);

        let (_, rsi) = parse_txr_and_rsi(&spc.entries[0].data);
        let spft = SpFt::parse(&rsi.resource_data).unwrap();
        assert_eq!(spft.glyphs.len(), 1);
        assert_eq!(spft.glyphs[0].codepoint, 0xC4);
    }

    #[test]
    fn replace_zeroes_shipped_atlas_ink() {
        let old_len = bc4_byte_count(32, 16) as u32;
        let srd_bytes = build_srd_with_srdv_info(build_spft_bytes(), 32, 16, old_len, 0x16);
        // Shipped atlas is uniformly inked, so any surviving pixel is obvious.
        let mut spc = make_spc_with_font(srd_bytes, uniform_bc4(7, 32, 16));

        // Height matches the shipped extent on purpose: this is the case the
        // merge-only early return would have swallowed.
        let group = replace_group(atlas(32, 16), vec![solid_glyph(0xC4, (0, 0))]);
        let mut report = PatchReport::default();
        patch_font_member(&mut spc, 0, "x.spc", "font.stx", &group, &mut report).unwrap();

        assert_eq!(report.font_atlas_replaces, 1);
        assert_eq!(report.font_atlas_grows, 0);

        let decoded = decode_argb8888_mono(&spc.entries[1].data, 32, 16);
        assert_eq!(decoded[0], 255, "replacement glyph missing");
        assert_eq!(
            decoded[15 * 32 + 31],
            0,
            "shipped ink survived a replace at the far corner"
        );
    }

    #[test]
    fn replace_without_glyph_pixels_still_zeroes_atlas() {
        let old_len = bc4_byte_count(32, 16) as u32;
        let srd_bytes = build_srd_with_srdv_info(build_spft_bytes(), 32, 16, old_len, 0x16);
        let mut spc = make_spc_with_font(srd_bytes, uniform_bc4(7, 32, 16));

        // Metadata-only glyph at the shipped height: no pixels and no growth,
        // which is exactly the shape the merge-mode early return short-circuits.
        // A replace must still rewrite the sidecar, or shipped ink would sit
        // under a replaced glyph table with no error to show for it.
        let group = replace_group(
            atlas(32, 16),
            vec![FontGlyphPatch {
                codepoint: 0xC4,
                glyph_alpha8: None,
                position: Some((0, 0)),
                size: Some((4, 4)),
                kerning: Some((0, 0, 0)),
            }],
        );
        let mut report = PatchReport::default();
        patch_font_member(&mut spc, 0, "x.spc", "font.stx", &group, &mut report).unwrap();

        assert_eq!(report.font_atlas_replaces, 1);
        assert_eq!(report.font_atlas_writes, 0);
        let decoded = decode_argb8888_mono(&spc.entries[1].data, 32, 16);
        assert!(
            decoded.iter().all(|&p| p == 0),
            "shipped ink survived a pixel-less replace"
        );
    }

    #[test]
    fn replace_permits_atlas_shrink() {
        let old_len = bc4_byte_count(32, 16) as u32;
        let srd_bytes = build_srd_with_srdv_info(build_spft_bytes(), 32, 16, old_len, 0x16);
        let mut spc = make_spc_with_font(srd_bytes, uniform_bc4(7, 32, 16));

        let group = replace_group(atlas(32, 8), vec![solid_glyph(0xC4, (0, 0))]);
        let mut report = PatchReport::default();
        patch_font_member(&mut spc, 0, "x.spc", "font.stx", &group, &mut report).unwrap();

        assert_eq!(spc.entries[1].data.len(), 32 * 8 * 4);
        let (txr, rsi) = parse_txr_and_rsi(&spc.entries[0].data);
        assert_eq!(txr.display_height, 8);
        assert_eq!(txr.display_width, 32);
        assert_eq!(txr.format, 0x01);
        // Width-derived, so shrinking the height leaves the pitch valid.
        assert_eq!(txr.scanline, (32 / 4) * 8);
        assert_eq!(rsi.resource_info_list[0][1], 32 * 8 * 4);
        assert_eq!(report.font_atlas_replaces, 1);
        assert_eq!(report.font_atlas_grows, 0);
    }

    #[test]
    fn replace_rejects_width_change() {
        let old_len = bc4_byte_count(32, 16) as u32;
        let srd_bytes = build_srd_with_srdv_info(build_spft_bytes(), 32, 16, old_len, 0x16);
        let mut spc = make_spc_with_font(srd_bytes, uniform_bc4(0, 32, 16));

        let group = replace_group(atlas(64, 16), vec![solid_glyph(0xC4, (0, 0))]);
        let mut report = PatchReport::default();
        let err =
            patch_font_member(&mut spc, 0, "x.spc", "font.stx", &group, &mut report).unwrap_err();
        assert!(matches!(err, TranslateError::AtlasWidthChange { .. }));
    }

    #[test]
    fn replace_rejects_zero_atlas_height() {
        let old_len = bc4_byte_count(32, 16) as u32;
        let srd_bytes = build_srd_with_srdv_info(build_spft_bytes(), 32, 16, old_len, 0x16);
        let mut spc = make_spc_with_font(srd_bytes, uniform_bc4(0, 32, 16));

        let group = replace_group(atlas(32, 0), vec![solid_glyph(0xC4, (0, 0))]);
        let mut report = PatchReport::default();
        let err =
            patch_font_member(&mut spc, 0, "x.spc", "font.stx", &group, &mut report).unwrap_err();
        assert!(matches!(err, TranslateError::AtlasGeometry { .. }));
    }

    #[test]
    fn replace_succeeds_against_unsupported_shipped_format() {
        // Format 0x11 is one `decode_atlas_to_alpha8` rejects — but replace
        // never decodes, so a font we can't read can still be rebuilt.
        let srd_bytes = build_srd_with_srdv_info(build_spft_bytes(), 32, 16, 256, 0x11);
        let mut spc = make_spc_with_font(srd_bytes, vec![0u8; 256]);

        let group = replace_group(atlas(32, 16), vec![solid_glyph(0xC4, (0, 0))]);
        let mut report = PatchReport::default();
        patch_font_member(&mut spc, 0, "x.spc", "font.stx", &group, &mut report).unwrap();

        let (txr, _) = parse_txr_and_rsi(&spc.entries[0].data);
        assert_eq!(txr.format, 0x01);
        let decoded = decode_argb8888_mono(&spc.entries[1].data, 32, 16);
        assert_eq!(decoded[0], 255);
    }

    #[test]
    fn replace_preserves_spft_header_and_font_name() {
        let old_len = bc4_byte_count(32, 16) as u32;
        let srd_bytes = build_srd_with_srdv_info(build_spft_bytes(), 32, 16, old_len, 0x16);
        let mut spc = make_spc_with_font(srd_bytes, uniform_bc4(0, 32, 16));

        // Names none of the header fields — they're container/engine state, not
        // content, so a replace must leave them alone.
        let group = replace_group(atlas(32, 16), vec![solid_glyph(0xC4, (0, 0))]);
        let mut report = PatchReport::default();
        patch_font_member(&mut spc, 0, "x.spc", "font.stx", &group, &mut report).unwrap();

        let (_, rsi) = parse_txr_and_rsi(&spc.entries[0].data);
        let spft = SpFt::parse(&rsi.resource_data).unwrap();
        assert_eq!(spft.unknown6, 6);
        assert_eq!(spft.bit_flag_count, 0xFF5F);
        assert_eq!(spft.scale_flag, 20);
        assert_eq!(spft.font_name, "Test");
    }

    #[test]
    fn replace_taller_atlas_counts_as_replace_not_grow() {
        let old_len = bc4_byte_count(32, 16) as u32;
        let srd_bytes = build_srd_with_srdv_info(build_spft_bytes(), 32, 16, old_len, 0x16);
        let mut spc = make_spc_with_font(srd_bytes, uniform_bc4(0, 32, 16));

        let group = replace_group(atlas(32, 32), vec![solid_glyph(0xC4, (0, 20))]);
        let mut report = PatchReport::default();
        patch_font_member(&mut spc, 0, "x.spc", "font.stx", &group, &mut report).unwrap();

        // Taller than shipped, but rebuilt rather than grown.
        assert_eq!(report.font_atlas_grows, 0);
        assert_eq!(report.font_atlas_replaces, 1);
        assert_eq!(spc.entries[1].data.len(), 32 * 32 * 4);
    }

    #[test]
    fn replace_glyph_outside_shrunk_atlas_is_caught() {
        let old_len = bc4_byte_count(32, 16) as u32;
        let srd_bytes = build_srd_with_srdv_info(build_spft_bytes(), 32, 16, old_len, 0x16);
        let mut spc = make_spc_with_font(srd_bytes, uniform_bc4(0, 32, 16));

        // y=6 + height 4 fits the shipped 16-row atlas but not the declared
        // 8-row one, so this only fails if validation uses the *new* extent.
        let group = replace_group(atlas(32, 8), vec![solid_glyph(0xC4, (0, 6))]);
        let mut report = PatchReport::default();
        let err =
            patch_font_member(&mut spc, 0, "x.spc", "font.stx", &group, &mut report).unwrap_err();
        assert!(matches!(err, TranslateError::AtlasOverflow { .. }));
    }
}
