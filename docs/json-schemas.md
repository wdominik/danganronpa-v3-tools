# JSON schemas

This document specifies every JSON format the tools in this workspace
read or write. There are three families:

1. **`drv3-cli` format sidecars** — round-trip a single binary format
   through a human-editable JSON file (STX, DAT, WRD, SpFt).
2. **`drv3-cli` container manifests** — preserve archive metadata that
   would otherwise be lost when extracting a CPK or SPC.
3. **`drv3-translate` patch documents** (`drv3-translate/v1`) — describe
   a translation: source/target language metadata plus per-file lists
   of STX text replacements and/or font-glyph edits.

The library crates are deliberately **serde-free** — they expose plain
Rust types and produce/consume raw bytes. The CLIs own every JSON
schema that ships here, so the on-disk JSON layouts can evolve without
churning library APIs.

SRD has **no JSON exchange** — it only supports raw byte extract /
pack. The CPK manifest's optional sidecar packets (`_etoc.bin`,
`_itoc.bin`, `_gtoc.bin`) are opaque binary blobs and likewise not
JSON.

## Schema-versioning policy

Every manifest carries a `version: 1` field; every translation patch
carries `schema: "drv3-translate/v1"`. The project is **pre-1.0**: the
schemas may change in place and the version stays at the current value.
From `1.0.0` onward, breaking schema changes will bump the version and
introduce a forward-compatible read path. Today, readers reject any
version other than the one above.

The authoritative source of truth for every field is the corresponding
DTO module:

- Manifests and format sidecars: [`crates/drv3-cli/src/dto.rs`](../crates/drv3-cli/src/dto.rs).
- Translation patches: [`crates/drv3-translate-cli/src/dto.rs`](../crates/drv3-translate-cli/src/dto.rs).

---

## drv3-cli format sidecars

### STX

```sh
drv3-cli stx dump  input.stx  out.json
drv3-cli stx build out.json  output.stx
```

```json
{
  "tables": [
    {
      "unknown": 0,
      "entries": [
        { "id": 0, "text": "Press any button" },
        { "id": 1, "text": "Continue" }
      ]
    }
  ]
}
```

`unknown` is a 4-byte field per sub-table whose meaning isn't pinned
down yet — preserve it verbatim. The writer deduplicates strings
within a table: two entries with identical `text` share one slot in
the string data.

### DAT

```sh
drv3-cli dat dump  input.dat  out.json
drv3-cli dat build out.json  output.dat
```

```json
{
  "schema": [
    { "name": "id",       "type": "u32",   "count": 1 },
    { "name": "label",    "type": "utf16", "count": 1 },
    { "name": "values",   "type": "f32",   "count": 4 }
  ],
  "rows": [
    [
      { "type": "u32",   "values": [42] },
      { "type": "utf16", "values": ["alpha"] },
      { "type": "f32",   "values": [1.0, 2.0, 3.0, 4.0] }
    ]
  ]
}
```

Cell types are tagged: every cell carries both its `type` and its
`values` array. Allowed types: `u8`, `u16`, `u32`, `u64`, `s8`,
`s16`, `s32`, `s64`, `f32`, `f64`, `ascii` (UTF-8 string),
`label` (UTF-8), `refer` (UTF-8), `utf16` (UTF-16 LE). The cell's
`values` length must equal its column's `count`.

### WRD

```sh
drv3-cli wrd dump  input.wrd  out.json
drv3-cli wrd build out.json  output.wrd
```

```json
{
  "unknown1": 0,
  "external_string_count": 12,
  "commands": [
    { "opcode": 76, "args": [0, 0] }
  ],
  "local_branches": [{ "id": 0, "offset": 16 }],
  "label_offsets": [0],
  "label_names": ["start"],
  "parameters": ["chap0_scene_a"],
  "internal_strings": null
}
```

Opcode bytes and offsets are plain decimal in the JSON (JSON has no
hex literal syntax). `opcode: 76` is `0x4C` — the `LOC` opcode in the
WRD spec.

The `dialogue` subcommand (`drv3-cli wrd dialogue input.wrd`)
prints the `(speaker, string_id)` pairs the bytecode references —
useful for cross-referencing STX strings with their on-screen
speaker.

### SpFt

```sh
drv3-cli spft dump  input.spft  out.json
drv3-cli spft build out.json  output.spft
```

```json
{
  "unknown6": 6,
  "bit_flag_count": 65375,
  "scale_flag": 1,
  "font_name": "FOT-NewRodin Pro DB",
  "glyphs": [
    {
      "codepoint": 65,
      "position": { "x": 128, "y": 0 },
      "size":     { "width": 12, "height": 16 },
      "kerning":  { "left": 0, "right": 0, "vertical": 0 }
    }
  ]
}
```

`position` is `{ x, y }` in atlas pixels (12-bit each); `size` is
`{ width, height }` in pixels (8-bit each); `kerning` is
`{ left, right, vertical }` signed bytes (`left`/`right` horizontal side
bearings, `vertical` a vertical offset). The metadata round-trips
cleanly; pixel writes for new glyphs are driven by the translation
pipeline's font-group support (`drv3-translate apply`), not by the
standalone `spft build` subcommand.

---

## drv3-cli container manifests

### CPK manifest

`drv3-cli cpk extract` writes a `manifest.json` next to the file
bodies; `cpk pack` reads it back.

```json
{
  "version": 1,
  "header": {
    "name": "CpkHeader",
    "columns": [
      { "name": "UpdateDateTime", "storage": "PerRow", "type": "u64" },
      { "name": "Align",          "storage": "PerRow", "type": "u16" },
      { "name": "Tvers",          "storage": "Constant", "type": "string",
        "constant": { "string": "CPKMC2.18.04" } }
    ],
    "row": {
      "Align":          { "u16": 2048 },
      "Sorted":         { "u16": 1 },
      "UpdateDateTime": { "u64": 1 }
    }
  },
  "toc_columns": [
    { "name": "DirName",     "storage": "PerRow", "type": "string" },
    { "name": "FileName",    "storage": "PerRow", "type": "string" },
    { "name": "FileSize",    "storage": "PerRow", "type": "u32" },
    { "name": "ExtractSize", "storage": "PerRow", "type": "u32" },
    { "name": "FileOffset",  "storage": "PerRow", "type": "u64" },
    { "name": "ID",          "storage": "PerRow", "type": "u32" },
    { "name": "UserString",  "storage": "PerRow", "type": "string" }
  ],
  "files": [
    { "path": "boot/movie_logo.mp4", "id": 0, "user_string": "" },
    { "path": "boot/startup_US.spc", "id": 4, "user_string": "" }
  ],
  "etoc_packet": "_etoc.bin"
}
```

Notes:

- `storage` is one of `None` / `Zero` / `Constant` / `PerRow` /
  `Constant2`; `type` is one of `u8` / `u16` / `u32` / `u64` /
  `s8` / `s16` / `s32` / `s64` / `f32` / `f64` / `string` / `data`.
- Values are tagged objects so they round-trip through `UtfValue`
  without precision loss: `{"u64": 1}`, `{"string": "foo"}`,
  `{"data_hex": "deadbeef"}`.
- `etoc_packet` / `itoc_packet` / `gtoc_packet` reference the
  opaque sidecar files (`_etoc.bin` etc.) by filename. Omit (or
  set to `null`) when the source CPK has no such packet.
- Layout-derived fields in `header.row` (`ContentOffset`,
  `ContentSize`, `TocOffset`, `TocSize`, `EtocOffset`/`EtocSize`,
  `ItocOffset`/`ItocSize`, `GtocOffset`/`GtocSize`, `Files`) are
  always recomputed by the writer — their values in the manifest
  are documentary and ignored on pack.

### SPC manifest

`drv3-cli spc extract` writes a `manifest.json` alongside the
extracted entry bodies; `spc pack` reads it back. This preserves the
archive-level `unknown1` / `unknown2`, per-entry `compression_flag`
and `unknown_flag`, and the original on-disk entry order — metadata a
naïve "alphabetical sort + force-stored" packer would lose.

```json
{
  "version": 1,
  "unknown1": "cafebabe0000000000000000000000000000000000000000000000000000000000000000",
  "unknown2": 3735928559,
  "entries": [
    { "name": "c00_001_018.stx", "compression": "stored", "unknown_flag": 0 },
    { "name": "c00_001_018.wrd", "compression": "lzss",   "unknown_flag": 4 }
  ]
}
```

Notes:

- `unknown1` is exactly 36 bytes, hex-encoded (72 chars). Decoded
  to anything else is an error.
- `compression` is `"stored"` (uncompressed body on disk) or
  `"lzss"` (Spike-Chunsoft LZSS). The pack reproduces the
  original flag exactly; `lzss` entries are re-compressed by our
  encoder, which is non-deterministic — the compressed bytes need
  not match the original encoder byte-for-byte, but the decoded
  bytes are identical.
- `entries` is ordered. The pack writes entries in array order so
  any code path that reads the SPC by entry index still finds the
  right file.
- Entry bodies on disk are looked up by `name` in the manifest's
  order.

---

## drv3-translate patch schema (`drv3-translate/v1`)

A translation document describes the patches a `drv3-translate apply`
run should make to one or more CPKs. The CLI accepts multiple JSONs in
one invocation (`--json a.json --json b.json …`); they merge into a
single in-memory translation set before the engine runs.

### Top-level envelope

```json
{
  "schema": "drv3-translate/v1",
  "source_language": "en",
  "target_language": "de",
  "created_at": "2026-05-18T12:00:00Z",
  "title": "Chapter 0 — German",
  "files": [ /* file groups, see below */ ]
}
```

| Field | Type | Required | Notes |
|---|---|---|---|
| `schema` | `string` | yes | Must equal `"drv3-translate/v1"`. Any other value is a hard error. |
| `source_language` | `string` | no | Free-form. When multiple JSONs are loaded, the first non-empty value wins. |
| `target_language` | `string` | no | Same merge rule. |
| `created_at` | `string` | no | Accepted-and-ignored — audit trail for translators. |
| `title` | `string` | no | Accepted-and-ignored. |
| `files` | array of file groups | yes | See below. Order is preserved so duplicate-detection messages can point at the original position. |

### File-group dispatch

Each entry in `files` is a tagged object: the `format` field selects
the variant (`"stx"` or `"font"`). Unknown variants are rejected.

### STX file group

Replaces one or more text slots in a single STX file inside an SPC
inside a CPK.

```json
{
  "format": "stx",
  "cpk": "partition_data_win_us.cpk",
  "cpk_path": "wrd_script/003/chap0_text_US.SPC",
  "spc_member": "c00_001_018.stx",
  "entries": [
    {
      "table": 0,
      "index": 0,
      "source": "Press any button",
      "target": "Drücke eine Taste"
    }
  ]
}
```

| Field | Type | Required | Notes |
|---|---|---|---|
| `format` | `"stx"` | yes | Tag value. |
| `cpk` | `string` | yes | CPK filename. Matched against the `--cpk` argument by filename only — paths and parent directories don't participate. |
| `cpk_path` | `string` | yes | Forward-slash path inside the CPK. Split on the final `/` to locate the file. |
| `spc_member` | `string` | yes | Member filename inside the SPC. |
| `entries` | array | yes | One entry per text slot to replace. |

Each entry:

| Field | Type | Required | Notes |
|---|---|---|---|
| `table` | `u32` | yes | Table index inside the STX (`0` for almost every shipped file). |
| `index` | `u32` | yes | The `StxEntry::id` value — i.e., the numeric ID stored alongside the string in the STX, **not** the array position. |
| `source` | `string` | yes | Source string captured at export time. Compared against the on-disk text to detect drift (see below). |
| `target` | `string` | yes | Replacement string. |
| `context` | any JSON value | no | Accepted-and-ignored — opaque translator metadata. |

### Font file group

Edits glyph metadata (position, size, kerning) and optionally writes
new pixel data into a font's BC4 atlas.

```json
{
  "format": "font",
  "cpk": "partition_resident_win.cpk",
  "cpk_path": "font/v3_font00.spc",
  "spc_member": "v3_font00.stx",
  "font_name": "FOT-NewRodin Pro DB",
  "atlas": { "width": 4096, "height": 101, "format": "BC4" },
  "glyphs": [
    {
      "codepoint": 228,
      "char": "ä",
      "image_path": "./glyphs/v3_font00/U+00E4.png",
      "position": { "x": 1024, "y": 128 },
      "size": { "width": 14, "height": 18 },
      "kerning": { "left": 0, "right": 0, "vertical": 0 }
    }
  ]
}
```

| Field | Type | Required | Notes |
|---|---|---|---|
| `format` | `"font"` | yes | Tag value. |
| `cpk` | `string` | yes | Same as STX groups. |
| `cpk_path` | `string` | yes | Same as STX groups. The `.srdv` atlas sidecar is looked up by name next to the `spc_member`. |
| `spc_member` | `string` | yes | The `.stx`-extensioned SPC member that wraps the SRD container holding the SPFT block. |
| `font_name` | `string` | no | Replacement value for `SpFt.font_name`. Existing name preserved when omitted. |
| `atlas` | object | no | Target atlas geometry. When taller than the game's shipped atlas, the engine grows it (see [Atlas growth](#atlas-growth)). Omit to keep the shipped geometry. |
| `glyphs` | array | yes | One entry per glyph edit or addition. |

The `atlas` object:

| Field | Type | Required | Notes |
|---|---|---|---|
| `width` | `u16` | yes | Must equal the game's existing `$TXR` width — only height growth is supported. A mismatch is a hard error. |
| `height` | `u16` | yes | Target atlas height. May exceed the shipped height (grows the atlas); must not be smaller (shrinking is rejected). |
| `format` | `string` | yes | Format of the *existing* atlas: `"BC4"` (the shipped format) or `"ARGB8888"` (a previously patched atlas). Other values are rejected at load time. Patched atlases are always re-emitted uncompressed as ARGB8888 regardless of this value. |

#### Atlas growth

DR V3 ships each font atlas at a fixed size (`$TXR` width × height, BC4).
A producer that re-packs a font with extra glyphs (e.g. the full Latin
alphabet for a Western translation) often needs a **taller** atlas than
the game ships. Declaring a taller `atlas.height` makes the engine grow
the atlas before blitting:

- **Height only.** `atlas.width` must equal the shipped width, so the
  decoded rows map 1:1 into the head of the taller buffer. The appended
  rows start zeroed (transparent).
- **Re-emitted uncompressed.** Patched atlases are decoded to
  full-resolution coverage, the new glyphs are blitted in, and the whole
  atlas is written back as uncompressed **ARGB8888** — never re-encoded to
  BC4, whose block compression would band the anti-aliased edges. This
  happens whenever any glyph pixels are written, growth or not.
- **Updated in lock-step.** The `$TXR` format (→ ARGB8888) and display height,
  the `.srdv` buffer, and the `$RSI` `ResourceInfo` blob size (`Value[1]`) all
  change; every other `$TXR`/`$RSI` field is preserved verbatim. In particular
  **`$TXR.scanline` stays at the shipped BC4 block-row pitch `width*2`** — the
  engine reads it as the texture's upload row stride and expects that value.
- **Additive.** Glyphs not listed in the JSON keep their existing
  metadata *and* pixels. A producer that moves an existing glyph into the
  grown region must list it with an `image_path` so its pixels are
  re-blitted at the new position; stale pixels left at the old position
  are unreferenced and harmless.
- **Errors.** A different width (`AtlasWidthChange`), a smaller height
  (`AtlasShrink`), a `$TXR` whose format is neither BC4 nor ARGB8888
  (`AtlasUnsupportedFormat`), or a font whose `$RSI` has no `.srdv`
  `ResourceInfo` entry (`AtlasSrdvResourceInfoMissing`) all abort the run
  before any bytes are rewritten.

Each glyph:

| Field | Type | Required | Notes |
|---|---|---|---|
| `codepoint` | `u32` | yes | Unicode codepoint. Canonical key. |
| `char` | `string` | no | Human-readable mirror of `codepoint`. When present, validated to be a single character matching `codepoint`. |
| `image_path` | `string` | no | Path to the per-glyph bitmap image, **relative to this JSON file's directory**. Its alpha channel is decoded into single-channel alpha8 before the engine sees it. (Renamed from `png`.) |
| `position` | `{ x: u16, y: u16 }` | conditional | Top-left atlas coordinate in pixels. Required for new codepoints and when `image_path` is present. |
| `size` | `{ width: u8, height: u8 }` | conditional | Glyph dimensions in pixels. Must equal the image's pixel dimensions when both are present. |
| `kerning` | `{ left: i8, right: i8, vertical: i8 }` | no | Signed pixel deltas: `left`/`right` horizontal side bearings, `vertical` offset. |

Unknown keys in a glyph object are rejected (so a stale `png` field is a
hard error rather than a silently-dropped image reference).

### Drift policy

The engine compares the JSON's `source` field against the on-disk STX
text per slot before writing. The CLI's `--on-drift` flag maps to the
library's `DriftPolicy`:

| `--on-drift` value | Behavior on mismatch |
|---|---|
| `warn` *(default)* | Record the drift in the report and write `target` anyway. |
| `skip` | Record the drift and leave the on-disk text untouched. |
| `error` | Abort the run; surface the first drift as an error. |

### Validation rules

Loaded by [`merge_docs`](../crates/drv3-translate-cli/src/dto.rs):

- **Duplicate slot**: the same `(cpk, cpk_path, spc_member, table, index)` 5-tuple appearing more than once *across all loaded JSONs* is rejected. Guards against accidentally combining translations that disagree.
- **Duplicate codepoint**: the same `codepoint` appearing more than once within a single font group is rejected.
- **`char` vs `codepoint` disagreement**: when both are present, `char` must be a single character whose Unicode codepoint equals `codepoint`.
- **Image dimensions vs `size`**: when both are present, the decoded image's width and height must equal `size`.

### Glyph-image sidecar conventions

- Paths in the `image_path` field are resolved relative to **the JSON file's directory** (not the CWD of `drv3-translate apply`). Each JSON has its own base directory.
- RGBA images contribute via the alpha channel — the decoder reads the alpha plane straight through.
- The DR V3 atlas convention is "background = 0, ink opacity = 255", which matches what most font-rasterizer exports already produce.
- Writing glyph pixels re-emits the whole atlas in the parallel `.srdv` SPC member as uncompressed **ARGB8888**: the shipped BC4 atlas is decoded to coverage, the new glyphs are copied in at full 8-bit precision, and the result is written back with the coverage replicated into all four channels. This avoids BC4's block quantization, which bands anti-aliased glyph edges. Original glyphs are preserved exactly (decoded straight from the shipped atlas); only the `$TXR` format/height and `$RSI` size change — see [Atlas growth](#atlas-growth).

### Worked example: STX-only patch

```json
{
  "schema": "drv3-translate/v1",
  "source_language": "en",
  "target_language": "de",
  "title": "Chapter 0 — first line",
  "files": [
    {
      "format": "stx",
      "cpk": "partition_data_win_us.cpk",
      "cpk_path": "wrd_script/003/chap0_text_US.SPC",
      "spc_member": "c00_001_018.stx",
      "entries": [
        {
          "table": 0,
          "index": 0,
          "source": "Press any button",
          "target": "Drücke eine Taste"
        }
      ]
    }
  ]
}
```

Apply with:

```sh
drv3-translate apply \
  --json chap0.json \
  --cpk gamedata/partition_data_win_us.cpk \
  --out work/patched \
  --report work/report.json
```

### Worked example: font patch with one glyph

```json
{
  "schema": "drv3-translate/v1",
  "target_language": "de",
  "files": [
    {
      "format": "font",
      "cpk": "partition_resident_win.cpk",
      "cpk_path": "font/v3_font00.spc",
      "spc_member": "v3_font00.stx",
      "font_name": "FOT-NewRodin Pro DB",
      "glyphs": [
        {
          "codepoint": 228,
          "char": "ä",
          "image_path": "./glyphs/v3_font00/U+00E4.png",
          "position": { "x": 1024, "y": 128 },
          "size": { "width": 14, "height": 18 },
          "kerning": { "left": 0, "right": 0, "vertical": 0 }
        }
      ]
    }
  ]
}
```

The `image_path` is resolved relative to this JSON's location: if the JSON
is at `work/de.json`, the engine reads
`work/glyphs/v3_font00/U+00E4.png`.

### Report file (`--report`)

When `drv3-translate apply --report report.json` is given, the run
emits a structured outcome record:

```json
{
  "applied": 247,
  "already_translated": 3,
  "skipped": 0,
  "drift": [
    {
      "cpk": "partition_data_win_us.cpk",
      "cpk_path": "wrd_script/003/chap0_text_US.SPC",
      "spc_member": "c00_001_018.stx",
      "table": 0,
      "index": 5,
      "on_disk_source": "...",
      "json_source": "...",
      "applied": true
    }
  ],
  "missing": [],
  "extract_collisions": [],
  "font_glyphs_added": 12,
  "font_glyphs_changed": 4,
  "font_atlas_writes": 12,
  "font_atlas_grows": 1
}
```

| Field | Meaning |
|---|---|
| `applied` | Number of STX slots whose `text` was changed. |
| `already_translated` | Slots whose on-disk text already equaled `target` — written out anyway; counted as a subset of `applied`. |
| `skipped` | Slots skipped because of `--on-drift skip`. |
| `drift` | Per-slot drift events (`applied` is `true` for `warn`, `false` for `skip`). |
| `missing` | Patches that pointed at a file, SPC member, or STX slot absent from the supplied game data. |
| `extract_collisions` | In `--mode extract`, files overwritten because two input CPKs shipped the same path. Empty in `--mode repack`. |
| `font_glyphs_added` | Glyphs whose codepoint did not previously exist in the SPFT and were added. |
| `font_glyphs_changed` | Glyphs that already existed and had at least one metadata field (`position`, `size`, `kerning`) changed. |
| `font_atlas_writes` | Glyphs whose pixel data was blitted into the BC4 atlas. |
| `font_atlas_grows` | Font groups whose atlas was grown in height to fit a taller re-pack (see [Atlas growth](#atlas-growth)). |

The library-side report types are documented on `PatchReport` in
[`crates/drv3-translate/src/report.rs`](../crates/drv3-translate/src/report.rs).
