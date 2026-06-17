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

/// Encode a single-channel alpha8 coverage buffer as uncompressed
/// `ARGB8888` (`$TXR` format `0x01`), replicating each coverage byte into all
/// four channels.
///
/// Output is row-major, 4 bytes per pixel, length `alpha8.len() * 4`. The
/// mono replication means the engine reads the same coverage value regardless
/// of how it interprets channel order, and — unlike BC4 — every 8-bit value
/// is preserved exactly (no block quantization). This is what keeps patched
/// glyph edges smooth.
///
/// # Panics
///
/// Never.
#[must_use]
pub fn encode_argb8888_mono(alpha8: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(alpha8.len() * 4);
    for &a in alpha8 {
        out.extend_from_slice(&[a, a, a, a]);
    }
    out
}

/// Decode an uncompressed `ARGB8888` atlas back to a single-channel alpha8
/// coverage buffer, taking one byte per pixel.
///
/// `bytes` must hold at least `width * height * 4` bytes (row-major, 4 bytes
/// per pixel). The first byte of each pixel is read as the coverage value;
/// for atlases written by [`encode_argb8888_mono`] all four channels are
/// equal, so the channel choice is immaterial. Lets us re-apply a patch to an
/// already-ARGB8888 atlas.
///
/// # Panics
///
/// Panics if `bytes.len() < width * height * 4`.
#[must_use]
pub fn decode_argb8888_mono(bytes: &[u8], width: usize, height: usize) -> Vec<u8> {
    let count = width * height;
    assert!(
        bytes.len() >= count * 4,
        "ARGB8888 decode: input has {} bytes, need {} for {}×{}",
        bytes.len(),
        count * 4,
        width,
        height,
    );
    (0..count).map(|i| bytes[i * 4]).collect()
}

/// Copy a single-channel alpha8 glyph into a single-channel alpha8 atlas
/// buffer **in place**, row by row — a plain overwrite with no quantization
/// and no blending, so the glyph's coverage lands in the atlas bit-for-bit.
///
/// `dst` is the full atlas buffer (`dst_w * dst_h` bytes, row-major).
/// `(dst_x, dst_y)` is the glyph's top-left in atlas pixels, `(src_w, src_h)`
/// the glyph size, and `src` its `src_w * src_h` alpha8 pixels.
///
/// # Panics
///
/// - if `dst.len() != dst_w * dst_h`
/// - if `dst_x + src_w > dst_w` or `dst_y + src_h > dst_h`
/// - if `src.len() != src_w * src_h`
#[expect(
    clippy::too_many_arguments,
    reason = "8 inherently-positional values (atlas dims, dst pos, src dims, src buf)"
)]
pub fn blit_alpha8(
    dst: &mut [u8],
    dst_w: usize,
    dst_h: usize,
    dst_x: usize,
    dst_y: usize,
    src_w: usize,
    src_h: usize,
    src: &[u8],
) {
    assert_eq!(dst.len(), dst_w * dst_h, "atlas size mismatch");
    assert!(
        dst_x + src_w <= dst_w && dst_y + src_h <= dst_h,
        "glyph footprint exceeds atlas extents",
    );
    assert_eq!(src.len(), src_w * src_h, "src buffer size mismatch");

    for row in 0..src_h {
        let d0 = (dst_y + row) * dst_w + dst_x;
        let s0 = row * src_w;
        dst[d0..d0 + src_w].copy_from_slice(&src[s0..s0 + src_w]);
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

/// Build the 8-entry BC4 (`BC4_UNORM` / RGTC1-unsigned) decode palette for a
/// block with endpoints `r0`/`r1`.
///
/// This follows the **standard** index convention the game — and every other
/// BC4 codec — uses: code `0 → r0`, code `1 → r1`, and codes `2..=7` are
/// interpolated. With `r0 > r1` all six in-between stops are interpolated; with
/// `r0 <= r1` only four are, and codes `6`/`7` are the constants `0`/`255`.
/// Interpolation truncates (matching the shipped atlases byte-for-byte).
fn build_ramp(r0: u8, r1: u8) -> [u8; 8] {
    let r0u = u32::from(r0);
    let r1u = u32::from(r1);
    let mut r = [0u8; 8];
    r[0] = r0;
    r[1] = r1;
    if r0 > r1 {
        // 8-value block: codes 2..=7 interpolate between the endpoints.
        for (i, slot) in r.iter_mut().enumerate().skip(2) {
            let iu = i as u32;
            *slot = (((8 - iu) * r0u + (iu - 1) * r1u) / 7) as u8;
        }
    } else {
        // 6-value block: codes 2..=5 interpolate; codes 6/7 are 0 and 255.
        for (i, slot) in r.iter_mut().enumerate().take(6).skip(2) {
            let iu = i as u32;
            *slot = (((6 - iu) * r0u + (iu - 1) * r1u) / 5) as u8;
        }
        r[6] = 0;
        r[7] = 255;
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A uniform-`value` BC4 block: `r0 == r1 == value`, all indices 0, so it
    /// decodes back to `value` everywhere.
    fn uniform_block(value: u8) -> [u8; 8] {
        [value, value, 0, 0, 0, 0, 0, 0]
    }

    #[test]
    fn decode_bc4_lays_out_blocks_block_row_major() {
        // Two uniform blocks side by side → an 8×4 atlas, left half 200,
        // right half 0.
        let mut bc4 = Vec::new();
        bc4.extend_from_slice(&uniform_block(200));
        bc4.extend_from_slice(&uniform_block(0));
        let dec = decode_bc4(&bc4, 8, 4);
        assert_eq!(dec.len(), 8 * 4);
        for y in 0..4 {
            for x in 0..8 {
                let expected = if x < 4 { 200 } else { 0 };
                assert_eq!(dec[y * 8 + x], expected, "mismatch at ({x}, {y})");
            }
        }
    }

    #[test]
    fn decode_bc4_crops_partial_edge_blocks() {
        // 7×5 atlas pads to 8×8 on disk (2×2 blocks); decode returns 7×5.
        let bc4: Vec<u8> = std::iter::repeat_n(uniform_block(42), 4)
            .flatten()
            .collect();
        assert_eq!(bc4.len(), 2 * 2 * BLOCK_BYTES);
        let dec = decode_bc4(&bc4, 7, 5);
        assert_eq!(dec.len(), 7 * 5);
        assert!(dec.iter().all(|&v| v == 42));
    }

    #[test]
    fn decode_byte_layout_matches_shipped_block() {
        // The first block of v3_font00.srdv is `01 00 49 92 24 49 92 24`.
        // With r0=1 > r1=0 the standard palette is [1, 0, 0, 0, 0, 0, 0, 0]
        // (code 0 → r0 = 1, every other code → 0), so the tile is a quantized
        // "background ≈ 1" region: every pixel decodes to 0 or 1.
        let block = [0x01, 0x00, 0x49, 0x92, 0x24, 0x49, 0x92, 0x24];
        let pixels = decode_block(block);
        for p in &pixels {
            assert!(*p == 0 || *p == 1, "unexpected pixel value {p}");
        }
    }

    #[test]
    fn decode_block_uses_standard_bc4_index_convention() {
        // A real shipped block from v3_font03.srdv (`53 FF 49 92 04 C9 9C C0`):
        // r0=0x53=83 < r1=0xFF=255, the 6-value mode. Under the standard BC4
        // convention (code 1 → r1) the solid-glyph codes resolve to 255.
        let block = [0x53, 0xFF, 0x49, 0x92, 0x04, 0xC9, 0x9C, 0xC0];
        let pixels = decode_block(block);
        assert_eq!(
            pixels,
            [
                255, 255, 255, 255, 255, 255, 255, 83, 255, 255, 151, 0, 255, 255, 83, 0
            ],
        );
        // The standard palette for these endpoints: code 0 → r0, code 1 → r1.
        assert_eq!(build_ramp(83, 255), [83, 255, 117, 151, 186, 220, 0, 255]);
    }

    #[test]
    fn argb8888_mono_round_trips_every_value_exactly() {
        // The whole point of the uncompressed path: full 8-bit coverage
        // survives, including a smooth 0..255 gradient that BC4 would band.
        let alpha: Vec<u8> = (0..=255u8).collect();
        let enc = encode_argb8888_mono(&alpha);
        assert_eq!(enc.len(), alpha.len() * 4);
        for (i, &a) in alpha.iter().enumerate() {
            assert_eq!(&enc[i * 4..i * 4 + 4], &[a, a, a, a]);
        }
        let dec = decode_argb8888_mono(&enc, 16, 16);
        assert_eq!(dec, alpha, "ARGB8888 round-trip must be lossless");
    }

    #[test]
    fn blit_alpha8_copies_glyph_exactly_and_leaves_rest() {
        let mut atlas = vec![0u8; 8 * 8];
        let glyph = vec![10, 20, 30, 40, 50, 60]; // 3×2
        blit_alpha8(&mut atlas, 8, 8, 4, 5, 3, 2, &glyph);
        for y in 0..8 {
            for x in 0..8 {
                let in_glyph = (4..7).contains(&x) && (5..7).contains(&y);
                let expected = if in_glyph {
                    glyph[(y - 5) * 3 + (x - 4)]
                } else {
                    0
                };
                assert_eq!(atlas[y * 8 + x], expected, "mismatch at ({x}, {y})");
            }
        }
    }
}
