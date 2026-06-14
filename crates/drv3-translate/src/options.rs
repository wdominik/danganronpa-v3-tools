//! Patch-time options the caller can tune.

/// How the engine reacts when the on-disk source string for a slot doesn't
/// match the `source` recorded in the translation JSON.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DriftPolicy {
    /// Record the drift in the report and write the target anyway.
    #[default]
    WarnAndApply,
    /// Record the drift and skip the slot (leave the on-disk string untouched).
    Skip,
    /// Abort the entire patch run; surface the first drift as an error.
    Error,
}

/// Engine-level options.
#[derive(Debug, Clone, Default)]
pub struct PatchOptions {
    pub on_drift: DriftPolicy,
    /// When `true`, the engine parallelises patching across file groups
    /// via `rayon`. Defaults to `false` so deterministic ordering of
    /// report entries is the easy path; the CLI flips this on for real runs.
    pub parallel: bool,
}
