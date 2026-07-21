# Changelog

All notable changes to this project are documented here. The format
follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the
project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.3.0] — 2026-07-21

### Added

- **Font groups can now recreate a font wholesale.** The new required
  `mode` field on a font file group selects `"merge"` (the previous
  behavior: shipped glyph table and atlas pixels survive, listed glyphs are
  layered on top) or `"replace"` (both are discarded, so the listed glyphs
  are the complete font). `replace` exists for typefaces that couldn't be
  sourced or licensed and have to be re-rendered from a substitute.
  Because nothing of the shipped atlas survives, `replace` may also *shrink*
  the atlas height, and it never decodes the shipped `.srdv` — so a font
  whose `$TXR.format` is neither BC4 nor ARGB8888 can still be rebuilt
  (untested in-engine; the `$TXR.scanline` rule is derived from the shipped
  BC4 atlases).
- **Two report counters**: `font_glyphs_removed` (glyphs dropped by a
  `replace`) and `font_atlas_replaces` (atlases rebuilt from zero). A
  `replace` counts under `font_atlas_replaces`, never `font_atlas_grows`,
  even when its declared height exceeds the shipped one.

### Changed

Breaking, and taken without a compatibility shim since the schema is
pre-release:

- **`mode` is required on every font file group.** There is no default: the
  two modes differ in what they destroy, so guessing would be unsafe. A
  document written against 0.2.0 fails with `missing field \`mode\``.
- **`atlas.format` was removed**, one release after being added in 0.2.0.
  It was validated at parse time and then discarded — the engine reads the
  true format from `$TXR.format` and already rejects anything it can't
  decode — so it carried no information, and under `replace` (which decodes
  nothing) it would have been actively meaningless. `AtlasJson` keeps
  `deny_unknown_fields`, so a leftover `"format"` key is a hard error rather
  than silently ignored.
- **Under `replace`, every glyph must carry `position`, `size`, `kerning`,
  and `image_path`**, and the group must declare an `atlas` and list at
  least one glyph. There is no shipped glyph to inherit from, so a partial
  entry would silently land at `(0, 0)` with zero size or zero advance.
  Whitespace glyphs need a transparent PNG rather than an omitted
  `image_path`. All of a glyph's missing fields are reported at once.
- **`FontFileGroup.atlas` moved into the new `FontPatchMode` enum**
  (`Merge { atlas: Option<AtlasSpec> }` / `Replace { atlas: AtlasSpec }`),
  making "replace without a declared atlas" unrepresentable rather than a
  runtime check. Library callers constructing `FontFileGroup` directly are
  affected.
- **`AtlasWidthChange` and `AtlasShrink` messages were reworded.** Width is
  locked in both modes (not "only height growth is supported"), and the
  shrink error now points at `mode: "replace"` as the way to get a smaller
  atlas.
- **Atlas byte-size validation moved ahead of allocation.** A
  producer-supplied `u16` height could previously drive a ~1 GB allocation
  that was only rejected afterwards when the `$RSI` size slot overflowed
  `u32`.

## [0.2.0] — 2026-07-19

### Fixed

- **The real-data CPK round-trip test now spot-checks the layout.** Its
  "Alignment invariant" loop asserted nothing; it now verifies `ContentOffset`
  is `Align`-aligned and that the first file's body actually sits there (the
  exhaustive per-file offset check already lives in a unit test).
- **Hand-edited JSON now rejects unknown keys.** All exchange DTOs, both
  container manifests, and the translation doc / entry / atlas gained
  `#[serde(deny_unknown_fields)]`, so a mistyped key (`cpk_paht`, a stale `png`)
  is a hard error instead of being silently dropped. (The two internally-tagged
  file-group variants can't take the attribute, but their required fields still
  error on a typo.) `atlas.format` is now a parse-time-validated enum, and
  `WrdJson.internal_strings` / `UtfColumnJson.name` no longer emit `"…": null`.
- **`apply`'s drift/missing report is now fully reproducible.** Within a single
  CPK the work list was iterated in `HashMap` order, so the report's drift and
  missing sequences varied run to run; the list is now sorted by `cpk_path`
  before dispatch. Byte output is unaffected.
- **CPK extract now rejects path-traversal and absolute entries.** Both
  `drv3-cli cpk extract` and `drv3-translate-cli`'s extract mode validated each
  archive-supplied path component (reject `..` / `.` / `\` directory segments
  and an empty, `..`, or separator-bearing `file_name`) before writing —
  matching the pack side's `split_path` guard. Previously a crafted CPK with
  `dir_name = "../../x"` could write outside the output directory, and an
  absolute `file_name` (`/etc/x`, `C:\x`) would make `PathBuf::push` replace the
  whole destination. (Medium; shipped game archives are unaffected.)
- **Font sidecar derivation is now case-insensitive.** `sidecar_name_for`
  matches the `.stx` / `.srd` suffix case-insensitively (preserving the stem's
  original casing) so an upper-cased member such as `V3_FONT00.STX` still
  resolves its `.srdv` sibling instead of silently deriving `…STX.srdv`.
- **Malformed-input hardening across the format parsers.** Several `parse`
  entry points could panic or attempt a huge allocation on crafted (non-shipped)
  input, contradicting `drv3-binio`'s "parsers must never panic on malformed
  input" contract. All now return a `BinError` instead:
  - `drv3-cpk` `@UTF` parsing no longer panics when the string-pool / data-blob
    section offsets are inverted or out of range; the bounds are checked via the
    new `Reader::subslice`.
  - `drv3-cpk` `read_packet` no longer panics on an oversized or overflowing
    `@UTF` packet size (checked add + `subslice` before the read).
  - `drv3-spft` now errors (as its docs already promised) instead of panicking
    when a sparse-index entry points past the bbox table.
  - `drv3-spc` rejects negative subfile sizes/name lengths (previously cast to a
    near-`usize::MAX` allocation request); `drv3-compression`'s SPC-LZSS
    decompressor caps its output pre-allocation so an absurd declared size can't
    force a giant allocation.
  - `drv3-dat` now rejects a `bytes_per_row` that disagrees with the summed
    column widths (previously a silent misparse of the string pools).
  - Untrusted element counts in `drv3-cpk`, `drv3-spc`, `drv3-stx`, `drv3-spft`,
    and `drv3-dat` no longer drive an unbounded `Vec::with_capacity`; capacity
    hints are clamped to what the remaining buffer can supply, and an `@UTF`
    table with zero columns is rejected.
- **`Reader::align_to`'s debug assertion no longer degenerates to a tautology.**
  `alignment.is_power_of_two() || alignment >= 1` was always true; it now
  asserts `alignment != 0`, and both `Reader::align_to` and `Writer::pad_to`
  document the non-zero precondition with a `# Panics` section. (Not reachable
  from parse paths — every call site passes a literal power of two.)
- **CLI `--help` / usage now shows the real binary names.** `drv3-cli` and
  `drv3-translate-cli` introduced themselves as `drv3` and `drv3-translate` in
  their `clap` usage strings; the command names now match the installed
  executables. (Completes the naming correction from 0.1.3, which fixed only
  the prose.)

### Added

- **`Reader::subslice` and `Reader::capacity_hint`** in `drv3-binio` — a
  bounds-checked sub-slice and a buffer-clamped capacity hint, used by the
  hardening fixes above.
- **Malformed-input tests across every format parser.** Truncated buffers,
  oversized counts, inverted offsets, negative sizes, and corrupt compressed
  streams are now exercised for the `drv3-cpk`, `drv3-spc`, `drv3-spft`,
  `drv3-stx`, `drv3-dat`, `drv3-srd`, `drv3-wrd`, and `spc_lzss::decompress`
  parsers — upholding the "parsers never panic on malformed input" contract
  across the workspace. Plus a `drv3-cli` unit test for the `roundtrip`
  extension classifier.

### Changed

- **Trimmed dead public API.** CRILAYLA's parked codec
  (`read_header` / `compress` / `decompress` and its header types) and SRD's
  `RESOURCE_OFFSET_MASK` are now `pub(crate)`, and the unreferenced
  `RAW_HEADER_TRAILER_SIZE` was removed. `crilayla::is_crilayla` and
  `CrilaylaError` stay public. Breaking only for external consumers of the
  parked CRILAYLA symbols (none exist; pre-1.0).
- **Internal code-quality pass.** No behavior change: one
  `drv3-binio::align_up` helper now backs every padding site; the `@UTF`
  pool-string read is deduplicated; WRD's header back-patch goes through the
  `Writer` patch API instead of raw slice indexing; the BC4/ARGB and
  drift-preview magic numbers are named; and the engine's invariant-guarded
  `panic!`/`expect` sites carry `// invariant:` comments.
- **Split `drv3-dto`: the translation-patch DTOs moved to a new
  `drv3-dto-patch` crate.** `drv3-dto` now carries only the dump/build exchange
  schema; the `drv3-translate/v1` patch schema — together with its `image`
  (PNG decoding) and `drv3-translate` dependencies — lives in `drv3-dto-patch`,
  which depends on `drv3-dto` for the shared glyph-geometry DTOs. As a result
  `drv3-cli` no longer transitively compiles `drv3-translate`, `rayon`, or the
  `image` decoder. Breaking for code that named `drv3_dto::patch::*` (now
  `drv3_dto_patch::*`) — acceptable pre-1.0.
- **API-consistency pass across the format crates.**
  - `Stx::to_bytes` and `SpFt::to_bytes` now return `BinResult<Vec<u8>>`
    instead of `Vec<u8>`. The counts and offsets they narrow into 32-bit
    on-disk fields are range-checked with `u32::try_from` rather than a silent
    `as u32` truncation, matching the already-fallible `Dat` / `Wrd` / `Srd` /
    `UtfTable` writers. Breaking for callers of the infallible form (add `?` or
    `.unwrap()`) — acceptable pre-1.0.
  - `drv3-compression` gained `crilayla::CrilaylaResult` and
    `spc_lzss::SpcResult` aliases (parity with the `cpk` / `spc` crate
    `Result`s), and `crilayla::read_header` now returns a named
    `CrilaylaHeader { uncompressed_size, compressed_size }` instead of a bare
    `(u32, u32)` tuple.
  - Format magic constants (`@UTF`, `CPK `/`TOC `, `$CFH` / `$CT0` / `$TXR` /
    `$RSI`, `CRILAYLA`) are now private to their modules — none were referenced
    across a crate or module boundary.
  - Two `@UTF` parse errors (an out-of-range string-pool offset and a
    schema-vs-header `row_size` mismatch) now report the real stream position
    instead of a placeholder `0`; `BinError`'s type docs note that encode-time
    writer errors legitimately report `0`.
- **SPC-LZSS packing now uses a hash-chain match search.**
  `spc_lzss::compress` replaced its naive linear window rescan
  (`O(n × window × match-length)`) with a hash-chain longest-match search —
  roughly an order of magnitude faster on large SPC members, the hot path of
  `spc pack` and translation `apply`. The public `compress` signature is
  unchanged, output still round-trips, and the compression ratio is identical
  (the matcher walks the full in-window chain, so it finds the same longest
  matches). Compressed bytes may differ from before when several equal-length
  matches tie (the encoding was never guaranteed byte-identical).
- **The translation engine skips a redundant text copy.**
  `apply` no longer `clone_from`s an STX entry's target text when it already
  equals the requested translation; the reported counts and the output are
  unchanged.
- **`drv3-cli` now uses `--in` / `--out` named flags.** Every
  subcommand takes a named `--in <input>` and, where it writes a file,
  `--out <output>` instead of positional arguments — consistent with
  `drv3-translate-cli`. Breaking: e.g. `drv3-cli stx dump a.stx a.json` is now
  `drv3-cli stx dump --in a.stx --out a.json`.
- **`drv3-translate-cli validate` now signals failure via its exit code.** It
  exits nonzero when the pre-flight finds any drift event
  or missing slot (and on schema/parse errors), so it's usable as a scripted
  gate; a clean run still exits `0`. Breaking for scripts that ignored the
  previously always-zero exit code.
- **`drv3_translate::apply` rejects duplicate CPK names.**
  Supplying the same CPK name twice now returns `TranslateError::DuplicateCpk`
  instead of silently applying the same file groups to each handle and
  double-counting the report.
- **Unified JSON schema versioning onto per-format `schema` tags.** Every
  JSON document now carries a `schema` string tag
  (`drv3-stx/v1`, `drv3-dat/v1`, `drv3-wrd/v1`, `drv3-spft/v1`, `drv3-cpk/v1`,
  `drv3-spc/v1`; the patch keeps `drv3-translate/v1`), validated on read. The
  container manifests drop their numeric `version` field; the exchange DTOs gain
  a tag (and their `From<…Json>` conversions became `TryFrom`); and DAT's column
  list moved from the now-reserved `schema` key to `columns`. JSON enum values
  are uniformly snake_case (`"storage": "PerRow"` → `"per_row"`). All breaking,
  acceptable pre-1.0.

### Documentation

- **Documentation and comment-style pass.** Added the missing
  `///` docs (the `write_int!` macro, small `drv3-binio` `Reader`/`Writer`
  methods, the `UtfValue` accessors, the `drv3-dto` exchange-schema structs and
  modules, and the `drv3-cli` dispatch functions); unified terminology
  ("Spike Chunsoft", "SPC archive", "byte-code"); sentence-cased the
  `docs/binary-formats.md` section headings and tagged its (and the README's)
  plain-text code fences; and fixed the `drv3-stx` doctest to use `?`, renamed
  the README "Crates" heading, and corrected the `--ignored real_data`
  invocation.
- **Rewrote the JSON schema-versioning policy and every example.**
  `docs/json-schemas.md` now documents the unified per-format `schema` tags and
  carries them in each example; storage values are shown snake_cased; DAT's
  example uses `columns`.
- Documented the WRD byte-code argument-scanning ambiguity (a `0x70` argument
  high byte is indistinguishable from the next opcode marker) as a known
  limitation in the `drv3-wrd` module header, and recorded `drv3-binio`'s
  64-bit-`usize` host assumption for the on-disk offset/size casts.
- **Doc/reality accuracy pass.** Removed references to a CI pipeline that does
  not exist (`README.md`, `CONTRIBUTING.md`, a test-module header); corrected
  the `SpcEntryJson.name`, `spc_lzss::decompress`, `merge_docs`, and `BinError`
  doc comments to match actual behavior; stopped calling the floating `stable`
  toolchain channel "pinned"; now ignore the whole `gamedata/` directory and
  dropped the stale `samples/` layout note; and de-linked the untagged `0.1.0`
  entry below.
- **CLI reference completeness.** Added one-line help to
  every `drv3-cli` subcommand (so `drv3-cli spc --help` etc. no longer print
  blank descriptions), and documented the `apply` `--mode` / `--threads` flags
  and `validate`'s exit behavior in `docs/json-schemas.md` and the README.

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

- **Documentation and comment accuracy, workspace-wide.** Reviewed and corrected
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

## 0.1.0 — 2026-05-18

First public release.

_Predates the public Git history — which begins at the initial commit tagged
`v0.1.1` — so there is no `v0.1.0` tag or release._

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

[Unreleased]: https://github.com/wdominik/danganronpa-v3-tools/compare/v0.3.0...HEAD
[0.3.0]: https://github.com/wdominik/danganronpa-v3-tools/releases/tag/v0.3.0
[0.2.0]: https://github.com/wdominik/danganronpa-v3-tools/releases/tag/v0.2.0
[0.1.4]: https://github.com/wdominik/danganronpa-v3-tools/releases/tag/v0.1.4
[0.1.3]: https://github.com/wdominik/danganronpa-v3-tools/releases/tag/v0.1.3
[0.1.2]: https://github.com/wdominik/danganronpa-v3-tools/releases/tag/v0.1.2
[0.1.1]: https://github.com/wdominik/danganronpa-v3-tools/releases/tag/v0.1.1
