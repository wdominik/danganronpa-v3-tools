# Changelog

All notable changes to this project are documented here. The format
follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the
project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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
- **Translation pipeline** (`drv3-translate`, `drv3-translate apply | validate`):
  JSON exchange format, drift detection (warn / skip / error policies),
  parallel patching across CPKs, font-atlas pixel writing into the
  BC4-encoded `.srdv` sidecars.
- **`drv3-cli roundtrip`** sanity-check subcommand: parse a file,
  re-emit it, exit non-zero if the bytes diverge.

[0.1.1]: https://github.com/wdominik/danganronpa-v3-tools/releases/tag/v0.1.1
[0.1.0]: https://github.com/wdominik/danganronpa-v3-tools/releases/tag/v0.1.0
