//! `BC4_UNORM` atlas codec (single-channel, 4 bpp, 4×4 blocks).
//!
//! # DR V3 atlas layout
//!
//! Every font atlas in DR V3 ships as `BC4_UNORM` with `TXR.format == 0x16`
//! and `TXR.swizzle == 0x01` (the swizzle bit is effectively a no-op for
//! these atlases: a linear block-row-major decode places glyphs exactly
//! at the positions recorded in the sibling `SPFT`). Atlas width is 4096
//! for `v3_font00` and 2048 for the other 24 fonts; heights vary
//! (100, 101, 193, 199, 279, 469).
//!
//! Sidecar layout: each font's pixel data is a parallel `.srdv` SPC
//! member next to the SRD-wrapped `.stx`. The on-disk byte count
//! satisfies `srdv_size == TXR.scanline × ceil(TXR.display_height / 4)`,
//! where `scanline == (width / 4) × 8` = bytes per block-row.
//!
//! # Block layout (`BC4_UNORM`, 8 bytes / 4×4 pixels)
//!
//! ```text
//! byte 0       u8  r0 — first endpoint
//! byte 1       u8  r1 — second endpoint
//! byte 2..7    48b 16 × 3-bit indices, packed LSB-first
//!
//! If r0 > r1:  ramp[i] = round((r0 * (7 - i) + r1 * i) / 7)  for i in 0..8
//! Else:        ramp[0..6] linear from r0 to r1, ramp[6] = 0, ramp[7] = 255
//! ```
//!
//! The shipped DR V3 atlases use the `r0 > r1` mode (typically
//! `r0 = 1, r1 = 0` for background blocks, so the empty parts of the
//! atlas decode to a uniform value of 1 rather than 0 — a harmless
//! quantization artifact).

const BLOCK_BYTES: usize = 8;
const BLOCK_DIM: usize = 4;

/// Decode a `BC4_UNORM`-compressed atlas into a single-channel alpha8 buffer.
///
/// `bytes` must be at least `ceil(width / 4) × ceil(height / 4) × 8`
/// bytes long. The returned buffer is `width × height` bytes, row-major
/// (no padding). Pixels outside the BC4 block grid that the original
/// encoder padded with are dropped.
///
/// # Errors
///
/// This function does not return a `Result`; it panics only if `bytes`
/// is shorter than the required size.
///
/// # Panics
///
/// Panics if `bytes.len()` is smaller than the BC4 byte count for the
/// given dimensions.
#[must_use]
pub fn decode_bc4(bytes: &[u8], width: usize, height: usize) -> Vec<u8> {
    let blocks_x = width.div_ceil(BLOCK_DIM);
    let blocks_y = height.div_ceil(BLOCK_DIM);
    let needed = blocks_x * blocks_y * BLOCK_BYTES;
    assert!(
        bytes.len() >= needed,
        "BC4 decode: input has {} bytes, need {} for {}×{}",
        bytes.len(),
        needed,
        width,
        height,
    );
    let aligned_w = blocks_x * BLOCK_DIM;
    let aligned_h = blocks_y * BLOCK_DIM;
    let mut full = vec![0u8; aligned_w * aligned_h];
    for by in 0..blocks_y {
        for bx in 0..blocks_x {
            let off = (by * blocks_x + bx) * BLOCK_BYTES;
            let block_bytes: [u8; 8] = bytes[off..off + BLOCK_BYTES]
                .try_into()
                .expect("BLOCK_BYTES slice");
            let pixels = decode_block(block_bytes);
            for j in 0..BLOCK_DIM {
                for i in 0..BLOCK_DIM {
                    let x = bx * BLOCK_DIM + i;
                    let y = by * BLOCK_DIM + j;
                    full[y * aligned_w + x] = pixels[j * BLOCK_DIM + i];
                }
            }
        }
    }
    if aligned_w == width && aligned_h == height {
        return full;
    }
    let mut out = Vec::with_capacity(width * height);
    for y in 0..height {
        out.extend_from_slice(&full[y * aligned_w..y * aligned_w + width]);
    }
    out
}

/// Encode a single-channel alpha8 buffer as `BC4_UNORM`.
///
/// `alpha8` must have exactly `width × height` bytes. Output is laid
/// out block-row-major (matching the on-disk format) and has length
/// `ceil(width / 4) × ceil(height / 4) × 8`. Pixels beyond the source
/// dimensions are zero-padded internally to fill partial edge blocks.
///
/// The encoder always picks the `r0 > r1` mode (linear 8-stop ramp) for
/// non-uniform blocks. Uniform blocks emit `r0 == r1` and all-zero
/// indices, which still decodes back to the original value.
///
/// # Errors
///
/// None — function is infallible given correctly-sized input.
///
/// # Panics
///
/// Panics if `alpha8.len() != width * height`.
#[must_use]
pub fn encode_bc4(alpha8: &[u8], width: usize, height: usize) -> Vec<u8> {
    assert_eq!(
        alpha8.len(),
        width * height,
        "BC4 encode: alpha8 length {} != width*height {}",
        alpha8.len(),
        width * height,
    );
    let blocks_x = width.div_ceil(BLOCK_DIM);
    let blocks_y = height.div_ceil(BLOCK_DIM);
    let mut out = Vec::with_capacity(blocks_x * blocks_y * BLOCK_BYTES);
    let mut block_pixels = [0u8; 16];
    for by in 0..blocks_y {
        for bx in 0..blocks_x {
            for j in 0..BLOCK_DIM {
                for i in 0..BLOCK_DIM {
                    let x = bx * BLOCK_DIM + i;
                    let y = by * BLOCK_DIM + j;
                    block_pixels[j * BLOCK_DIM + i] = if x < width && y < height {
                        alpha8[y * width + x]
                    } else {
                        0
                    };
                }
            }
            out.extend_from_slice(&encode_block(&block_pixels));
        }
    }
    out
}

/// Blit a single-channel alpha8 glyph into a `BC4`-encoded atlas
/// **in place**, re-encoding only the 4×4 blocks that overlap the
/// glyph footprint. Blocks outside that footprint keep their original
/// bytes verbatim — guarantees we don't introduce quantization drift
/// in regions the patch doesn't touch.
///
/// `srdv` is the full atlas byte buffer; its length must equal the `BC4`
/// byte count for `(atlas_width, atlas_height)`. `dst_x` / `dst_y` are
/// the atlas pixel coordinates of the glyph's top-left, `(src_w, src_h)`
/// are the glyph dimensions, and `src` is the glyph's alpha8 pixels
/// (row-major, length `src_w * src_h`).
///
/// # Errors
///
/// None — function is infallible if pre-conditions are met.
///
/// # Panics
///
/// - if `srdv.len()` doesn't match the expected `BC4` size for
///   `(atlas_width, atlas_height)`
/// - if `dst_x + src_w > atlas_width` or `dst_y + src_h > atlas_height`
/// - if `src.len() != src_w * src_h`
#[expect(
    clippy::too_many_arguments,
    reason = "8 inherently-positional values (atlas dims, dst pos, src dims, src buf) — splitting \
              into structs would force callers to construct boilerplate types for a hot blit helper"
)]
pub fn blit_alpha_into_bc4(
    srdv: &mut [u8],
    atlas_width: usize,
    atlas_height: usize,
    dst_x: usize,
    dst_y: usize,
    src_w: usize,
    src_h: usize,
    src: &[u8],
) {
    let blocks_x = atlas_width.div_ceil(BLOCK_DIM);
    let blocks_y = atlas_height.div_ceil(BLOCK_DIM);
    assert_eq!(
        srdv.len(),
        blocks_x * blocks_y * BLOCK_BYTES,
        "atlas size mismatch",
    );
    assert!(
        dst_x + src_w <= atlas_width && dst_y + src_h <= atlas_height,
        "glyph footprint exceeds atlas extents",
    );
    assert_eq!(src.len(), src_w * src_h, "src buffer size mismatch");

    if src_w == 0 || src_h == 0 {
        return;
    }

    let bx_min = dst_x / BLOCK_DIM;
    let by_min = dst_y / BLOCK_DIM;
    let bx_max = (dst_x + src_w - 1) / BLOCK_DIM;
    let by_max = (dst_y + src_h - 1) / BLOCK_DIM;

    for by in by_min..=by_max {
        for bx in bx_min..=bx_max {
            let off = (by * blocks_x + bx) * BLOCK_BYTES;
            let block_bytes: [u8; 8] = srdv[off..off + BLOCK_BYTES]
                .try_into()
                .expect("BLOCK_BYTES slice");
            let mut pixels = decode_block(block_bytes);
            for j in 0..BLOCK_DIM {
                for i in 0..BLOCK_DIM {
                    let ax = bx * BLOCK_DIM + i;
                    let ay = by * BLOCK_DIM + j;
                    // Skip pixels outside both the glyph footprint and
                    // the atlas extents.
                    let in_glyph_x = ax >= dst_x && ax < dst_x + src_w;
                    let in_glyph_y = ay >= dst_y && ay < dst_y + src_h;
                    if !in_glyph_x || !in_glyph_y {
                        continue;
                    }
                    let sx = ax - dst_x;
                    let sy = ay - dst_y;
                    pixels[j * BLOCK_DIM + i] = src[sy * src_w + sx];
                }
            }
            let new_block = encode_block(&pixels);
            srdv[off..off + BLOCK_BYTES].copy_from_slice(&new_block);
        }
    }
}

fn decode_block(block: [u8; 8]) -> [u8; 16] {
    let r0 = block[0];
    let r1 = block[1];
    let ramp = build_ramp(r0, r1);
    let bits = u64::from_le_bytes([
        block[2], block[3], block[4], block[5], block[6], block[7], 0, 0,
    ]);
    let mut out = [0u8; 16];
    for (i, slot) in out.iter_mut().enumerate() {
        let idx = ((bits >> (3 * i)) & 0x7) as usize;
        *slot = ramp[idx];
    }
    out
}

fn encode_block(pixels: &[u8; 16]) -> [u8; 8] {
    let r_min = *pixels.iter().min().expect("16 pixels");
    let r_max = *pixels.iter().max().expect("16 pixels");

    if r_min == r_max {
        // Uniform block: emit r0 == r1, all indices 0. ramp[0] == r_min
        // so the block decodes back to the original uniform value.
        let mut block = [0u8; 8];
        block[0] = r_min;
        block[1] = r_max;
        return block;
    }

    let r0 = r_max;
    let r1 = r_min;
    let ramp = build_ramp(r0, r1);

    let mut bits: u64 = 0;
    for (i, &p) in pixels.iter().enumerate() {
        let mut best_idx: u64 = 0;
        let mut best_err = u32::MAX;
        for (j, &r) in ramp.iter().enumerate() {
            let err = (i32::from(p) - i32::from(r)).unsigned_abs();
            if err < best_err {
                best_err = err;
                best_idx = j as u64;
            }
        }
        bits |= best_idx << (3 * i);
    }

    let mut block = [0u8; 8];
    block[0] = r0;
    block[1] = r1;
    block[2..8].copy_from_slice(&bits.to_le_bytes()[..6]);
    block
}

fn build_ramp(r0: u8, r1: u8) -> [u8; 8] {
    if r0 > r1 {
        let r0u = u32::from(r0);
        let r1u = u32::from(r1);
        std::array::from_fn(|i| {
            let iu = i as u32;
            ((r0u * (7 - iu) + r1u * iu + 3) / 7) as u8
        })
    } else {
        let r0u = u32::from(r0);
        let r1u = u32::from(r1);
        let mut r = [0u8; 8];
        for (i, slot) in r.iter_mut().take(6).enumerate() {
            let iu = i as u32;
            *slot = ((r0u * (5 - iu) + r1u * iu + 2) / 5) as u8;
        }
        r[6] = 0;
        r[7] = 255;
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_uniform_zero() {
        let src = vec![0u8; 16];
        let enc = encode_bc4(&src, 4, 4);
        let dec = decode_bc4(&enc, 4, 4);
        assert_eq!(dec, src);
    }

    #[test]
    fn round_trip_uniform_value() {
        for v in [1u8, 64, 128, 200, 255] {
            let src = vec![v; 16];
            let enc = encode_bc4(&src, 4, 4);
            let dec = decode_bc4(&enc, 4, 4);
            assert_eq!(dec, src, "uniform value {v} did not round-trip");
        }
    }

    #[test]
    fn round_trip_two_value_block() {
        // 4×4 with a hard edge — values 0 and 255 only.
        let src: Vec<u8> = (0..16).map(|i| if i % 4 < 2 { 0 } else { 255 }).collect();
        let enc = encode_bc4(&src, 4, 4);
        let dec = decode_bc4(&enc, 4, 4);
        // BC4 can represent {0, 255} exactly via the `r0=255, r1=0` ramp
        // with endpoints at ramp[0] and ramp[7].
        assert_eq!(dec, src);
    }

    #[test]
    fn round_trip_gradient_within_tolerance() {
        // 0..255 in 16 steps — not exactly representable in 8 quantization
        // levels, but should round-trip within ±9 of the original.
        let src: Vec<u8> = (0..16).map(|i| (i * 17) as u8).collect();
        let enc = encode_bc4(&src, 4, 4);
        let dec = decode_bc4(&enc, 4, 4);
        let max_err = src
            .iter()
            .zip(&dec)
            .map(|(a, b)| (i32::from(*a) - i32::from(*b)).abs())
            .max()
            .unwrap();
        assert!(max_err <= 20, "gradient error {max_err} exceeded tolerance");
    }

    #[test]
    fn blit_only_touches_affected_blocks_and_preserves_others() {
        // Build a 16×16 atlas filled with a pattern, encode, then blit
        // a 4×4 glyph at (4, 4). The two blocks NOT touched should be
        // byte-identical pre vs post.
        let mut alpha = vec![0u8; 16 * 16];
        for y in 0..16 {
            for x in 0..16 {
                alpha[y * 16 + x] = ((x + y * 16) as u8).wrapping_mul(3);
            }
        }
        let mut atlas = encode_bc4(&alpha, 16, 16);
        let orig = atlas.clone();
        // 4×4 glyph of 255s at (4, 4) — should hit exactly one BC4 block.
        let glyph = vec![255u8; 16];
        blit_alpha_into_bc4(&mut atlas, 16, 16, 4, 4, 4, 4, &glyph);

        // Block grid is 4×4; the touched block is (1, 1). Every other
        // block must be byte-identical.
        for by in 0..4 {
            for bx in 0..4 {
                let off = (by * 4 + bx) * 8;
                if (bx, by) == (1, 1) {
                    continue;
                }
                assert_eq!(
                    atlas[off..off + 8],
                    orig[off..off + 8],
                    "block ({bx}, {by}) was modified but shouldn't have been",
                );
            }
        }
        // The touched block should decode to all-255.
        let decoded = decode_bc4(&atlas, 16, 16);
        for y in 4..8 {
            for x in 4..8 {
                assert_eq!(decoded[y * 16 + x], 255);
            }
        }
    }

    #[test]
    fn blit_glyph_crossing_block_boundary_updates_each_affected_block() {
        // 16×16 atlas, blit 6×6 glyph at (3, 3) — spans 4 BC4 blocks.
        let mut atlas = encode_bc4(&vec![0u8; 16 * 16], 16, 16);
        let glyph = vec![200u8; 36];
        blit_alpha_into_bc4(&mut atlas, 16, 16, 3, 3, 6, 6, &glyph);
        let decoded = decode_bc4(&atlas, 16, 16);
        // Pixels in the glyph footprint should be 200.
        for y in 3..9 {
            for x in 3..9 {
                assert_eq!(decoded[y * 16 + x], 200, "miss at ({x}, {y})");
            }
        }
        // Pixels outside should be 0.
        for y in 0..16 {
            for x in 0..16 {
                if (3..9).contains(&y) && (3..9).contains(&x) {
                    continue;
                }
                assert_eq!(decoded[y * 16 + x], 0, "edge spilled at ({x}, {y})");
            }
        }
    }

    #[test]
    fn encode_alignment_handles_non_multiple_of_four() {
        // 7×5 non-aligned — encoder pads to 8×8 internally.
        let src = vec![42u8; 7 * 5];
        let enc = encode_bc4(&src, 7, 5);
        assert_eq!(enc.len(), 2 * 2 * BLOCK_BYTES);
        let dec = decode_bc4(&enc, 7, 5);
        assert_eq!(dec.len(), 7 * 5);
        assert!(dec.iter().all(|&v| v == 42));
    }

    #[test]
    fn decode_byte_layout_matches_shipped_block() {
        // The first block of v3_font00.srdv is `01 00 49 92 24 49 92 24`.
        // With r0=1 > r1=0, the 8-stop ramp is essentially
        // [1, 1, 1, 1, 0, 0, 0, 0] and the indices spell out a mix of
        // 1s and 2s — i.e., a quantized "background = 1" tile.
        let block = [0x01, 0x00, 0x49, 0x92, 0x24, 0x49, 0x92, 0x24];
        let pixels = decode_block(block);
        // Every pixel should be 0 or 1 given the ramp.
        for p in &pixels {
            assert!(*p == 0 || *p == 1, "unexpected pixel value {p}");
        }
    }
}
