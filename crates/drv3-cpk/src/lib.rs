//! CRIWARE CPK archive reader/writer.
//!
//! A CPK file is a sequence of *packets*, each carrying one `@UTF` columnar
//! table. The first packet is the **CPK header** (one row of global
//! metadata: offsets and sizes of every other packet, alignment value, file
//! count); the **TOC** packet has one row per file (name, offset, size, id);
//! and three optional packets — **ITOC**, **GTOC**, **ETOC** — carry
//! supplementary indices / timestamps. After the on-disk packets comes the
//! **content blob** holding raw file bodies.
//!
//! DR V3 ships three CPKs (`partition_resident_win.cpk`,
//! `partition_data_win.cpk`, `partition_data_win_us.cpk`). All of them use:
//!
//! - **Uncompressed, unencrypted TOC.** No CRILAYLA / no obfuscation, so a
//!   plain `@UTF` parser is enough.
//! - **Per-file alignment to `Align`** (0x800 = 2 KiB), with `Sorted == 1`.
//! - **ETOC packet at file end**: `EtocOffset + EtocSize == FileSize`.
//! - **`TocOffset` on an `Align` sector**: `TocOffset == 0x800`.
//!
//! ## Scope of v0.1
//!
//! - Parse the CPK packet, the TOC packet, and every file entry.
//! - Re-emit a functional CPK with the same files. Round-trip is **semantic**
//!   (parse → write → parse yields the same `Cpk`); byte-equal output is
//!   not guaranteed because `@UTF` string-pool ordering and inter-file
//!   padding bytes are not load-bearing for the game runtime.
//! - CRILAYLA-compressed entries are detected and rejected — DR V3 doesn't
//!   use them at the CPK layer (see [`drv3_compression::crilayla`]).
//! - ETOC / ITOC / GTOC packets are passed through verbatim as opaque
//!   bytes; only their position in the layout matters to the game.

// CPK uses the same CRI-style field names across many columns
// (TocOffset/TocSize/EtocOffset/EtocSize/…), and the @UTF schema-builder code
// is inherently long because every column type/storage combination is
// enumerated explicitly.
#![allow(
    clippy::similar_names,
    clippy::too_many_lines,
    clippy::match_same_arms,
    clippy::trivially_copy_pass_by_ref,
    clippy::uninlined_format_args,
    clippy::map_unwrap_or,
    clippy::unnecessary_wraps,
    clippy::needless_pass_by_value
)]

pub mod archive;
pub mod utf;

pub use archive::{Cpk, CpkFile, CpkParseError, CpkResult};
pub use utf::{StorageFlag, UtfColumn, UtfRow, UtfTable, UtfType, UtfValue};
