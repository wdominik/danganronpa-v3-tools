//! Engine error type.

use drv3_binio::BinError;
use drv3_cpk::CpkParseError;
use drv3_spc::SpcParseError;
use thiserror::Error;

/// Errors raised by the patch engine.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum TranslateError {
    /// Top-level CPK parse failed.
    #[error("CPK parse failed: {0}")]
    Cpk(#[from] CpkParseError),

    /// Inner SPC parse / serialize failed.
    #[error("SPC parse failed at {cpk_path}: {source}")]
    Spc {
        cpk_path: String,
        #[source]
        source: SpcParseError,
    },

    /// Inner STX parse failed.
    #[error("STX parse failed at {cpk_path}::{spc_member}: {source}")]
    Stx {
        cpk_path: String,
        spc_member: String,
        #[source]
        source: BinError,
    },

    /// Translation references a CPK that wasn't supplied to [`crate::apply`].
    #[error("no input CPK named {0} was supplied")]
    CpkNotFound(String),

    /// Drift detected and [`crate::DriftPolicy::Error`] is set.
    #[error(
        "source-string drift at {cpk_path}::{spc_member} table {table} index {index}: \
         on disk {on_disk_source:?}, JSON expects {json_source:?}"
    )]
    Drift {
        cpk_path: String,
        spc_member: String,
        table: u32,
        index: u32,
        on_disk_source: String,
        json_source: String,
    },

    /// SRD-container parse / serialize failure for a font SPC member.
    #[error("SRD parse failed at {cpk_path}::{spc_member}: {source}")]
    Srd {
        cpk_path: String,
        spc_member: String,
        #[source]
        source: BinError,
    },

    /// SPFT (font metadata) parse / serialize failure.
    #[error("SPFT parse failed at {cpk_path}::{spc_member}: {source}")]
    Spft {
        cpk_path: String,
        spc_member: String,
        #[source]
        source: BinError,
    },

    /// The `.stx` SPC member that should hold an SRD-wrapped SPFT did
    /// not contain a `$RSI` block whose `resource_data` starts with
    /// `SpFt` magic. Likely means the file group's `spc_member` points
    /// at the wrong SPC entry, or the producer's `font_name` doesn't
    /// match any font in the container.
    #[error("no SPFT-bearing $RSI block found in {cpk_path}::{spc_member}")]
    SpftNotFound {
        cpk_path: String,
        spc_member: String,
    },

    /// A font group asked us to patch atlas pixels (`glyph_alpha8`
    /// present) but the SPC doesn't carry the expected `<name>.srdv`
    /// sidecar entry next to the `.stx`.
    #[error("missing atlas sidecar {sidecar_name} in {cpk_path} (expected next to {spc_member})")]
    AtlasSidecarMissing {
        cpk_path: String,
        spc_member: String,
        sidecar_name: String,
    },

    /// The SRD's `$TXR` block exposes atlas dimensions that the BC4
    /// byte-count invariant doesn't satisfy (or the block was missing).
    /// Should not happen on shipped data — all 25 fonts pass the
    /// invariant — but guards against corrupt input.
    #[error("invalid atlas geometry in {cpk_path}::{spc_member}: {detail}")]
    AtlasGeometry {
        cpk_path: String,
        spc_member: String,
        detail: String,
    },

    /// A glyph's `position + size` falls outside the atlas extents
    /// declared by the `$TXR` block. Producer needs to relocate it.
    #[error(
        "glyph U+{codepoint:04X} in {cpk_path}::{spc_member} doesn't fit: \
         position={position:?}, size={size:?}, atlas={atlas:?}"
    )]
    AtlasOverflow {
        cpk_path: String,
        spc_member: String,
        codepoint: u32,
        position: (u16, u16),
        size: (u8, u8),
        atlas: (u16, u16),
    },

    /// `glyph_alpha8` buffer length doesn't equal `size.0 * size.1`.
    #[error(
        "glyph U+{codepoint:04X} in {cpk_path}::{spc_member}: alpha8 buffer is {actual} bytes \
         but size {size:?} requires {expected}"
    )]
    AtlasAlphaSize {
        cpk_path: String,
        spc_member: String,
        codepoint: u32,
        size: (u8, u8),
        expected: usize,
        actual: usize,
    },

    /// The JSON `atlas` requested a width different from the game's
    /// existing `$TXR` width. Only height growth is supported (width
    /// changes would force a full re-encode of every BC4 block-row).
    #[error(
        "atlas width mismatch in {cpk_path}::{spc_member}: JSON requests width {requested} \
         but game atlas is {current} (only height growth is supported)"
    )]
    AtlasWidthChange {
        cpk_path: String,
        spc_member: String,
        requested: u16,
        current: u16,
    },

    /// The JSON `atlas` requested a height smaller than the game's
    /// existing `$TXR` height. Shrinking would drop atlas rows that
    /// existing glyphs may reference.
    #[error(
        "atlas shrink in {cpk_path}::{spc_member}: JSON requests height {requested} \
         but game atlas is {current} (cannot shrink an atlas)"
    )]
    AtlasShrink {
        cpk_path: String,
        spc_member: String,
        requested: u16,
        current: u16,
    },

    /// A font atlas patch targeted a `$TXR` whose pixel format we can't
    /// decode. Only the shipped BC4 (`0x16`) and ARGB8888 (`0x01`, our own
    /// re-emitted output) are supported.
    #[error(
        "atlas patch in {cpk_path}::{spc_member} unsupported for texture format {format:#04x} \
         (only BC4 0x16 and ARGB8888 0x01 are supported)"
    )]
    AtlasUnsupportedFormat {
        cpk_path: String,
        spc_member: String,
        format: u8,
    },

    /// Atlas growth was requested but the SPFT-bearing `$RSI` block has
    /// no `.srdv` `ResourceInfo` entry whose blob size we can update.
    #[error("no .srdv ResourceInfo entry to resize in {cpk_path}::{spc_member}")]
    AtlasSrdvResourceInfoMissing {
        cpk_path: String,
        spc_member: String,
    },
}
