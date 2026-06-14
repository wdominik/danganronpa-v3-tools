//! Structured report describing the outcome of a patch run.

/// One drift event: the on-disk source string differed from the JSON
/// `source` for a given STX slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriftRecord {
    pub cpk: String,
    pub cpk_path: String,
    pub spc_member: String,
    pub table: u32,
    pub index: u32,
    pub on_disk_source: String,
    pub json_source: String,
    /// `true` if the engine still wrote the target (warn-and-apply policy).
    pub applied: bool,
}

/// One missing-target event: the translation pointed at a file or slot
/// that doesn't exist in the supplied game data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissingRecord {
    pub cpk: String,
    pub cpk_path: String,
    /// Empty for "the entire file is missing"; populated when only the slot is missing.
    pub spc_member: String,
    /// Populated when only a specific slot is missing; `None` means the
    /// SPC member or CPK file itself was absent.
    pub slot: Option<(u32, u32)>,
}

/// Aggregate report returned by [`crate::apply`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PatchReport {
    /// Number of slots whose `text` was changed.
    pub applied: usize,
    /// Number of slots that matched the JSON `target` already — written
    /// out anyway, so this is a subset of `applied` for accounting.
    pub already_translated: usize,
    /// Slots skipped because of [`crate::DriftPolicy::Skip`].
    pub skipped: usize,
    /// Drift events (warned, skipped, or — if you'd configured `Error` —
    /// the one that aborted the run).
    pub drift: Vec<DriftRecord>,
    /// Files / slots referenced by the JSON but absent from the game data.
    pub missing: Vec<MissingRecord>,
    /// Glyphs whose codepoint did not previously exist in the SPFT and
    /// were added by the patch (metadata only — see `font_atlas_writes`
    /// for the pixel-side count).
    pub font_glyphs_added: usize,
    /// Glyphs whose codepoint already existed in the SPFT and had at
    /// least one metadata field (`position` / `size` / `kerning`)
    /// changed by the patch.
    pub font_glyphs_changed: usize,
    /// Glyphs whose pixel data was blitted into the atlas (BC4-encoded
    /// `.srdv` sidecar). Metadata-only patches don't contribute here.
    pub font_atlas_writes: usize,
    /// Font atlases that were grown in height to fit a taller re-pack
    /// (`$TXR` height + `.srdv` buffer + `$RSI` `ResourceInfo` size all
    /// updated). One increment per font group whose atlas grew.
    pub font_atlas_grows: usize,
}

impl PatchReport {
    /// Merge another report into this one (used to combine per-CPK results).
    pub fn extend(&mut self, other: PatchReport) {
        self.applied += other.applied;
        self.already_translated += other.already_translated;
        self.skipped += other.skipped;
        self.drift.extend(other.drift);
        self.missing.extend(other.missing);
        self.font_glyphs_added += other.font_glyphs_added;
        self.font_glyphs_changed += other.font_glyphs_changed;
        self.font_atlas_writes += other.font_atlas_writes;
        self.font_atlas_grows += other.font_atlas_grows;
    }
}
