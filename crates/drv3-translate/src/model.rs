//! Plain-data input model for the patch engine.
//!
//! These types mirror the on-disk JSON schema (`drv3-translate/v1`) but
//! stay `serde`-free; the CLI front-end converts its serde DTOs into these
//! values before calling [`crate::apply`].

/// One top-level translation set, typically the union of every JSON file
/// the user passes on the command line.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TranslationSet {
    pub source_language: String,
    pub target_language: String,
    /// File groups in the order the caller provided them. Order is
    /// preserved so duplicate-detection messages can point at the original
    /// position.
    pub files: Vec<TranslationFileGroup>,
}

/// Per-format payload of a file group. v1 carries `Stx` and `Font`;
/// future formats become new variants without breaking the outer envelope.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum FileFormat {
    Stx(StxFileGroup),
    Font(FontFileGroup),
}

/// One file group: a target STX inside an SPC inside a CPK, together with
/// the entries to patch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranslationFileGroup {
    /// CPK filename (e.g. `"partition_data_win_us.cpk"`). Matched against
    /// the input CPK's filename in [`crate::apply`].
    pub cpk: String,
    /// Path inside the CPK (e.g.
    /// `"wrd_script/003/chap0_text_US.SPC"`). Split on the final `/`
    /// when locating the [`CpkFile`](drv3_cpk::CpkFile).
    pub cpk_path: String,
    /// Member filename inside the SPC (e.g. `"c00_002_018.stx"`).
    pub spc_member: String,
    pub format: FileFormat,
}

/// STX-format payload: the list of `(table, index, source, target)` rows
/// to apply to one STX file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StxFileGroup {
    pub entries: Vec<StxEntryPatch>,
}

/// One translation row.
///
/// `index` is the value of [`StxEntry::id`](drv3_stx::StxEntry::id) — i.e.
/// the numeric ID stored alongside the string in the STX file, **not** the
/// array position. Today the two coincide in shipped files because IDs are
/// dense from 0, but the engine resolves by ID so any future remapping
/// remains correct.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StxEntryPatch {
    pub table: u32,
    pub index: u32,
    /// Source string captured by the translator's exporter; the engine
    /// compares this to the on-disk STX text to detect drift.
    pub source: String,
    /// Replacement string written into the STX slot.
    pub target: String,
}

/// Font-format payload: the list of glyph edits to apply to one
/// SPFT-bearing SRD container.
///
/// In DR V3 each font is a pair of co-located SPC members: `<name>.stx`
/// (an SRD container; the `.stx` extension is a misnomer — bytes start
/// with `$CFH` SRD magic, and the SPFT metadata lives at the start of
/// the `$RSI` block's `resource_data` field) and `<name>.srdv` (the BC4
/// atlas pixel sidecar). The engine targets the `.stx` member; atlas
/// pixel writes are a separate phase that updates the `.srdv` sibling.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FontFileGroup {
    /// Replacement font name (e.g. `"FOT-HummingStd-D.otf"`). When
    /// `Some`, overwrites `SpFt.font_name`; when `None`, the existing
    /// name is preserved.
    pub font_name: Option<String>,
    /// Target atlas geometry. When `Some` and its `height` exceeds the
    /// game's existing `$TXR` height, the engine grows the BC4 atlas
    /// (height-only; width must stay constant) before blitting glyphs —
    /// this lets a producer that re-packed a font into a taller atlas
    /// place glyphs in rows beyond the original extent. When `None`, the
    /// existing atlas geometry is used verbatim (legacy additive blit).
    pub atlas: Option<AtlasSpec>,
    pub glyphs: Vec<FontGlyphPatch>,
}

/// Desired atlas geometry for a font group. Mirrors the JSON `atlas`
/// object. The pixel format is validated to be BC4 by the CLI front-end
/// before this value is built, so it isn't carried here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AtlasSpec {
    /// Atlas width in pixels. Must equal the game's existing `$TXR`
    /// width — only height growth is supported.
    pub width: u16,
    /// Atlas height in pixels. May exceed the existing height to grow
    /// the atlas; must not be smaller (no shrinking).
    pub height: u16,
}

/// One glyph edit. All shape fields are optional so the producer can
/// emit metadata-only updates (kerning fix, reposition, etc.) without
/// having to repeat unchanged values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FontGlyphPatch {
    /// Unicode codepoint. Canonical key; matches `SpFt.glyphs[i].codepoint`.
    pub codepoint: u32,
    /// Atlas pixel bytes for this glyph, if the producer is changing
    /// the rasterized image. **Single-channel alpha8**, row-major,
    /// length must equal `size.0 * size.1`. The CLI decodes the PNG
    /// and extracts the alpha channel before handing the bytes in
    /// here; library callers can hand any single-channel buffer.
    /// `None` ⇒ atlas pixel data is left untouched for this glyph.
    pub glyph_alpha8: Option<Vec<u8>>,
    /// Top-left atlas coordinate `(x, y)`. `Some` overrides the
    /// existing `SpFt.glyphs[i].position`; required for new codepoints
    /// and required when `glyph_alpha8` is present.
    pub position: Option<(u16, u16)>,
    /// Glyph bounding-box `(width, height)`. Must equal the PNG's
    /// pixel dimensions when `glyph_alpha8` is present.
    pub size: Option<(u8, u8)>,
    /// `(left, right, vertical)` kerning deltas in pixels.
    pub kerning: Option<(i8, i8, i8)>,
}
