# Changelog

All notable changes to this project are documented here. The format
follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the
project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.4] — 2026-06-18

### Added

- **Lookup helpers for navigating the CPK → SPC → STX nesting from your own code:**
  `Cpk::file` / `file_mut`, `Spc::entry` / `entry_mut`, and `StxTable::entry` / `entry_mut` find
  a file, member, or string by name / id without a manual `.iter().find(...)`. The `Cpk::parse`
  and `Spc::parse` docs also gained usage examples.

### Changed

- **Glyph geometry must be named objects.** Glyph `position` / `size` / `kerning` in
  `drv3-translate/v1` patch JSON and `drv3-cli spft` JSON are now accepted only as named
  objects (`{ "x", "y" }` etc.); the previously-undocumented positional-array form (`[x, y]`)
  is rejected with a clear error. (The named-object form has been canonical since 0.1.1.)
- **`Cpk` borrows file bodies instead of copying them (zero-copy parse).** `Cpk` and `CpkFile`
  now carry a lifetime — `Cpk<'a>`, with `CpkFile::data: Cow<'a, [u8]>`. `Cpk::parse` borrows
  each body directly from the input buffer, so `drv3-cli cpk list` / `cpk extract` no longer
  copy gigabytes of bodies onto the heap; the translation engine replaces patched bodies with
  owned bytes (`Cow::Owned`), and a `Cpk` assembled from owned data is `Cpk<'static>`. Because
  bodies are borrowed, `drv3-translate-cli apply` keeps the input CPK memory-mapped through the
  output write. Breaking for direct `drv3-cpk` consumers that name `Cpk` / `CpkFile` in
  signatures; the common `Cpk::parse(&bytes)` + `file.data.len()` usage is source-compatible.
- **JSON DTOs consolidated into a new `drv3-dto` crate.** Both CLIs' serde DTOs — the dump/build
  exchange schema and the translation-patch schema — now live in one crate, with the glyph
  geometry types and the map-only deserializer defined once. `drv3_dat::ColumnType::{tag,
  from_tag}` are now public so the DAT JSON `type` mapping reuses the on-disk tag mapping rather
  than duplicating it. Internal restructure; no CLI behavior change.
- **Dropped the always-constant `header.name` field from the CPK extract `manifest.json`.** It
  was written but never read back (`Cpk::to_bytes` hardcodes the `@UTF` table name); older
  manifests that still carry the field load fine, since it is ignored.
- **Internal quality pass (no behavior change):** zero-copy SpFt index-table serialization,
  simplified SRD resource-string parsing, clearer internal names (`Cpk::resolve_align`,
  `read_packet`), workspace-wide per-site `#[expect(reason = …)]` lint discipline (dropping the
  `drv3-cpk` blanket `allow`), and assorted doc/test fixes.

### Removed

- **Trimmed unused public surface from `drv3-binio` and `drv3-translate`** (pre-1.0, no in-tree
  users): `Writer::reserve_u32_be` (byte-identical to `reserve_u32_le` — reserving placeholder
  bytes carries no endianness; the endianness lives in the matching `patch_*` call),
  `Reader::len` (duplicated `Reader::buffer().len()` and was inconsistent with
  `Reader::is_empty`), and the never-constructed `TranslateError::CpkFileNotFound` /
  `SpcMemberNotFound` variants (missing files/members are reported as `PatchReport` entries, not
  hard errors). `Patch`'s fields are now private — it is an opaque token produced by
  `Writer::reserve*` and consumed by `Writer::patch*`.

### Fixed

- **SPC-LZSS decompression now reports a malformed back-reference as
  `SpcError::BadBackreference`** instead of the misleading `UnexpectedEof`.

## [0.1.3] — 2026-06-17

Maintenance release — no changes to tool behavior or file formats. Toolchain and
dependency refresh plus a workspace-wide documentation and comment accuracy pass.

### Changed

- **Minimum supported Rust version is now 1.96** (raised from 1.85); the code adopts
  idioms it unlocks (let-chains, `Int::is_multiple_of`).
- **Refreshed dependencies** via `cargo update` — notably `bitflags` 2.13.0,
  `serde_json` 1.0.150, and `image` 0.25.10 (unblocked by the higher MSRV).
- **Dropped unused dependencies** — `thiserror` from `drv3-dat`, `drv3-spft`, `drv3-srd`,
  `drv3-stx`, `drv3-wrd`, and `bitflags` from `drv3-cpk`.

### Fixed

- **Documentation and comment accuracy, workspace-wide.** Audited and corrected
  `README.md`, `CONTRIBUTING.md`, `docs/json-schemas.md`, `docs/binary-formats.md`, and
  in-code `//!`/`///` comments against the implementation. Notable corrections: the
  translation binary is `drv3-translate-cli` (was written as `drv3-translate`); `drv3-cli
  srd` exposes only `inspect`; the `@UTF` cell-read pseudocode and the SPC/WRD header field
  offsets now match the code; documented the CPK manifest `files[].extra` field; the `LOC`
  opcode is `0x4B`; American-English prose throughout.

## [0.1.2] — 2026-06-16

### Changed

- **Patched font atlases are re-emitted as uncompressed ARGB8888.** BC4 block
  compression represents only ~8 coverage levels per 4×4 block, which bands the
  soft anti-aliased edges of newly added glyphs (e.g. German `ß`/`ä`/`ö`). The
  font patch path now decodes the shipped BC4 atlas, blits new glyphs at full
  8-bit precision, and re-emits the whole atlas uncompressed as ARGB8888 (`$TXR`
  format `0x01`, coverage replicated into all four channels), so the gradient
  survives bit-for-bit. The `$TXR` format and display height change, plus the
  `$RSI` resource size; `$TXR.scanline` stays at the shipped BC4 block-row pitch
  `width*2` (the engine's upload row stride), and every other `$TXR`/`$RSI` field
  is preserved verbatim. The `atlas.format` JSON field now also accepts
  `"ARGB8888"` (the source-format hint) so re-applying a patch is idempotent.
- **`drv3-cli srd inspect`** now also prints `$TXR` `scanline`/`swizzle`/`palette`
  and the `$RSI` `resource_info_list` values.

### Fixed

- **BC4 (`BC4_UNORM`) decoder used a non-standard index convention.** `build_ramp`
  built a linear ramp indexed directly by the 3-bit code, so it mis-decoded the
  game's shipped atlases — reused glyphs decoded at ~56% coverage (`255 → 143`)
  with a `1` floor instead of `0`, rendering faded next to full-strength new
  glyphs. `build_ramp` now follows the standard BC4 (RGTC1-unsigned) palette
  (code `0 → r0`, `1 → r1`, `2..=7` interpolated), so `decode_bc4` reproduces the
  shipped atlases exactly.

### Removed

- **BC4 encoder.** `encode_bc4` and the in-place `blit_alpha_into_bc4` glyph blit
  are removed — the font patch path always re-emits ARGB8888, so nothing encodes
  BC4 any more. `decode_bc4` (reading the shipped atlases) is unchanged.

## [0.1.1] — 2026-06-14

### Changed

- **Breaking — font glyph JSON schema.** Glyph geometry is now expressed as
  named objects instead of positional arrays, and the glyph image field is
  renamed for clarity:
  - `position: [x, y]` → `position: { "x", "y" }`
  - `size: [w, h]` → `size: { "width", "height" }`
  - `kerning: [l, r, v]` → `kerning: { "left", "right", "vertical" }`
  - `png` → `image_path`
  Applies to both the `drv3-translate/v1` patch documents (`fonts.json`) and
  the standalone `drv3-cli spft dump | build` JSON. Per the pre-1.0 policy
  the `drv3-translate/v1` schema string is unchanged. Glyph objects now
  reject unknown keys, so a stale `png` field fails loudly. (Positional
  arrays still deserialize as an undocumented migration convenience; the
  named-object form is canonical.)

### Added

- **Font-atlas height growth** in the translation pipeline. A font file
  group may now declare a taller `atlas` (`{width, height, format}`); the
  engine extends the BC4 atlas in height (width fixed), copying existing
  block-rows verbatim and updating the `$TXR` height, the `.srdv` buffer,
  and the `$RSI` `ResourceInfo` blob size in lock-step. Lets producers
  re-pack fonts with extra glyphs (e.g. the full Latin alphabet) that no
  longer fit the shipped atlas. New report counter `font_atlas_grows`.

## [0.1.0] — 2026-05-18

First public release.

### Added

- **CPK archive** read and write (`drv3-cpk`, `drv3-cli cpk list | extract | pack`).
  Byte-for-byte repack against every load-bearing region of the shipped
  DR V3 CPKs; verified in-engine on the US Windows release.
- **SPC inner archive** read and write (`drv3-spc`, `drv3-cli spc list | extract | pack`).
  Semantic round-trip with metadata preservation via `manifest.json`.
- **STX string-table** read and write (`drv3-stx`, `drv3-cli stx dump | build`).
  Byte-equal round-trip for every shipped STX.
- **DAT typed table** read and write (`drv3-dat`, `drv3-cli dat dump | build`).
- **WRD bytecode script** read and write (`drv3-wrd`, `drv3-cli wrd dump | build | dialogue`).
- **SRD block container** read and write (`drv3-srd`, `drv3-cli srd inspect`),
  including the `BC4_UNORM` atlas codec.
- **SpFt font metadata** read and write (`drv3-spft`, `drv3-cli spft dump | build`).
- **Foundation crates** `drv3-binio` (bounded binary I/O primitives) and
  `drv3-compression` (SPC-LZSS codec, CRILAYLA header recognition).
- **Translation pipeline** (`drv3-translate`, `drv3-translate-cli apply | validate`):
  JSON exchange format, drift detection (warn / skip / error policies),
  parallel patching across CPKs, font-atlas pixel writing into the
  BC4-encoded `.srdv` sidecars.
- **`drv3-cli roundtrip`** sanity-check subcommand: parse a file,
  re-emit it, exit non-zero if the bytes diverge.

[Unreleased]: https://github.com/wdominik/danganronpa-v3-tools/compare/v0.1.4...HEAD
[0.1.4]: https://github.com/wdominik/danganronpa-v3-tools/releases/tag/v0.1.4
[0.1.3]: https://github.com/wdominik/danganronpa-v3-tools/releases/tag/v0.1.3
[0.1.2]: https://github.com/wdominik/danganronpa-v3-tools/releases/tag/v0.1.2
[0.1.1]: https://github.com/wdominik/danganronpa-v3-tools/releases/tag/v0.1.1
[0.1.0]: https://github.com/wdominik/danganronpa-v3-tools/releases/tag/v0.1.0
