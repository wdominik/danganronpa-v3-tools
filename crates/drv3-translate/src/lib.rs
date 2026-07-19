//! Translation patch engine for Danganronpa V3 game data.
//!
//! Consumes a [`TranslationSet`] (one or more file groups, each pointing at
//! an STX file inside an SPC inside a CPK) and applies it to one or more
//! parsed [`Cpk`](drv3_cpk::Cpk) archives in memory. The CLI front-end
//! ([`drv3-translate-cli`]) owns the on-disk JSON schema and feeds plain
//! Rust values into this crate; the engine itself is `serde`-free by
//! workspace convention — library crates expose plain Rust types and leave
//! JSON handling to the CLIs.
//!
//! # Patch pipeline
//!
//! ```text
//!  TranslationSet
//!       │
//!       │  (group by cpk → cpk_path → spc_member)
//!       ▼
//!  for each Cpk:
//!      for each cpk_path (file inside the CPK):
//!          Spc::parse(file.data)
//!          for each spc_member (file inside the SPC):
//!              Stx::parse(member.data)
//!              for each entry (table, index, source, target):
//!                  compare on-disk source vs. JSON source (drift policy)
//!                  set stx.tables[table].entries[where id == index].text = target
//!              member.data = Stx::to_bytes()?
//!          file.data = Spc::to_bytes()?
//! ```
//!
//! Patching mutates the parsed [`Cpk`](drv3_cpk::Cpk) value in place. The
//! caller is responsible for serializing back to disk via [`Cpk::to_bytes`]
//! or for re-extracting the file tree.
//!
//! # Drift policy
//!
//! The translation JSON carries the `source` string at export time. When the
//! game-data files drift (e.g., a patch update changed a line), the engine
//! consults [`PatchOptions::on_drift`] to decide whether to warn-and-apply,
//! skip, or abort. See [`DriftPolicy`].
//!
//! # Forward-compat
//!
//! [`FileFormat`] is `#[non_exhaustive]` so new file-format variants can
//! be added without breaking the JSON envelope or downstream callers.
//!
//! [`drv3-translate-cli`]: https://docs.rs/drv3-translate-cli
//! [`Cpk::to_bytes`]: drv3_cpk::Cpk::to_bytes

mod engine;
mod engine_font;
mod error;
mod model;
mod options;
mod report;

pub use engine::apply;
pub use error::TranslateError;
pub use model::{
    AtlasSpec, FileFormat, FontFileGroup, FontGlyphPatch, StxEntryPatch, StxFileGroup,
    TranslationFileGroup, TranslationSet,
};
pub use options::{DriftPolicy, PatchOptions};
pub use report::{DriftRecord, MissingRecord, PatchReport};
