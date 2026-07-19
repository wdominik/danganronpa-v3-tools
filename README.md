# Danganronpa V3 Tools

A Rust toolkit — two CLI binaries plus twelve library crates — for reading,
writing, and patching the game data files shipped with
*Danganronpa V3: Killing Harmony*.

> **Unofficial fan project.** This repository has no affiliation with,
> endorsement from, or other relationship to Spike Chunsoft, NIS America,
> or anyone else involved in developing or publishing *Danganronpa V3*.
> Game data referenced or produced by these tools remains the property of
> its respective rights holders. You need a legitimate copy of the game
> to use any of this — none of it is distributed here.

---

## Status

**v0.1.** What works:

- **CPK archives** — extract, list, and pack. The repack matches the
  shipped CPK byte-for-byte for every load-bearing region; verified
  in-engine against the US Windows release.
- **STX string tables** — dump to JSON, edit, build back. Round-trip is
  byte-equal for every shipped STX.
- **SPC archives** — extract and pack. Each subfile decompresses
  cleanly; semantic round-trip only (compressed bytes need not match the
  original encoder).
- **DAT / WRD / SRD / SpFt** — parse and re-emit with full round-trip
  fidelity for every field reverse-engineered so far.
- **Translation pipeline** (`drv3-translate-cli apply | validate`) — JSON
  exchange format, drift detection, parallel patching across CPKs, and
  font-atlas pixel writing into the BC4-encoded `.srdv` sidecars.

This is a command-line toolkit by design — there is no GUI and none is
planned. Build one on top of these libraries if you want one; that's
exactly the surface they're shaped for.

---

## What's in this repo

### CLI

`drv3-cli` exposes one subcommand family per format:

```text
drv3-cli cpk      list | extract | pack
drv3-cli spc      list | extract | pack
drv3-cli stx      dump | build
drv3-cli dat      dump | build
drv3-cli wrd      dump | build | dialogue
drv3-cli srd      inspect
drv3-cli spft     dump | build
drv3-cli roundtrip
```

Inputs and outputs are named flags: every subcommand takes `--in
<input>`, and those that produce a file also take `--out <output>`
(e.g. `drv3-cli stx dump --in c00.stx --out c00.json`).

Every subcommand reads or writes the JSON exchange format documented
below, except `srd inspect` — that one prints a structural tree of the
SRD's blocks to stdout and exists as a quick orientation aid when
triaging unfamiliar SRD containers.

A second binary, `drv3-translate-cli`, drives the translation pipeline:

```text
drv3-translate-cli apply     --json … --cpk … --out …
drv3-translate-cli validate  --json … [--cpk …]
```

`apply` also accepts `--mode repack|extract`, `--on-drift`, `--report`,
and `--threads`; `validate` exits nonzero when it finds drift or missing
slots. See [`docs/json-schemas.md`](docs/json-schemas.md) for the full
option reference.

### Crates

Each format has its own crate so you can pull in only what you need:

| Crate | Purpose |
|---|---|
| `drv3-binio` | Bounded binary I/O primitives — endian-explicit `Reader` / `Writer`. Foundation, no DR V3-specific code. |
| `drv3-cli` | The primary command-line interface — dump / build / extract / pack, with its JSON in `drv3-dto`. |
| `drv3-compression` | SPC-LZSS codec; CRILAYLA header-recognition only. |
| `drv3-cpk` | CRIWARE CPK archive reader/writer + the `@UTF` columnar primitive. |
| `drv3-dat` | Typed columnar data tables with two string pools (UTF-8, UTF-16 LE). |
| `drv3-dto` | serde DTOs + conversions for the dump/build/extract-pack **exchange** schema. Keeps the format libraries serde-free. |
| `drv3-dto-patch` | serde DTOs for the **translation-patch** schema (`drv3-translate/v1`); depends on `drv3-dto` for shared glyph geometry. |
| `drv3-spc` | Spike Chunsoft SPC archive. |
| `drv3-spft` | `SpFt` font metadata block found inside SRD resources. |
| `drv3-srd` | SRD block container (textures, fonts, vertex buffers, resource info). |
| `drv3-stx` | STX string tables (the primary translation target). |
| `drv3-translate` | Translation patch engine — applies STX text and font glyph patches to parsed CPKs in memory. Serde-free; the JSON schema lives in `drv3-dto-patch`. |
| `drv3-translate-cli` | CLI front-end for the translation pipeline (`apply` / `validate`) — consumes patch JSON via `drv3-dto-patch`. |
| `drv3-wrd` | Byte-code script container paired with STX dialogue files. |

Format-leaf crates (`drv3-stx`, `drv3-dat`, `drv3-wrd`, `drv3-srd`,
`drv3-spft`) depend only on `drv3-binio` (plus the small `bitflags` crate
for `drv3-srd`). Pulling in `drv3-stx` does not drag in CPK or compression
code. See
[`CONTRIBUTING.md`](CONTRIBUTING.md#1-project-shape) for the full
dependency graph.

---

## CLI quickstart

### Install

```sh
cargo install --path crates/drv3-cli
cargo install --path crates/drv3-translate-cli   # translation pipeline
# or, without installing:
cargo build --release -p drv3-cli
# binary lands at target/release/drv3-cli
```

### List what's inside a CPK

```sh
drv3-cli cpk list --in path/to/partition_data_win_us.cpk | head
# flash/adv/adv_6_kizuna_US.spc          (98368 bytes, id=0)
# flash/adv/adv_6_limit_ar_US.spc        (7936 bytes, id=1)
# wrd_script/003/chap0_text_US.SPC       (… bytes, id=…)
# …
```

### Extract → edit the first dialogue line → repack

A concrete worked example: `c00_001_018.stx` inside
`wrd_script/003/chap0_text_US.SPC` holds the first dialogue line the
player sees in *Chapter 0* — a good target for a visible end-to-end
test.

```sh
# 1. Extract the data CPK (writes file bodies + manifest.json).
drv3-cli cpk extract --in path/to/partition_data_win_us.cpk --out work/data_win_us

# 2. Open the chapter-0 dialogue SPC and dump its STX to JSON.
drv3-cli spc extract --in work/data_win_us/wrd_script/003/chap0_text_US.SPC --out work/chap0_text
drv3-cli stx dump    --in work/chap0_text/c00_001_018.stx                --out work/edit.json

# 3. Edit `work/edit.json` in your editor — change a `"text"` field.
#    The first dialogue line lives in tables[0].entries[0].

# 4. Build the JSON back into the STX, repack the SPC, repack the CPK.
drv3-cli stx build   --in work/edit.json     --out work/chap0_text/c00_001_018.stx
drv3-cli spc pack    --in work/chap0_text    --out work/data_win_us/wrd_script/003/chap0_text_US.SPC
drv3-cli cpk pack    --in work/data_win_us   --out work/patched-data-win-us.cpk
```

Drop the patched CPK into the game's `data/win/` directory (keep a
backup of the original!) and the edited line will show up in-game.

### Sanity-check a single file

```sh
drv3-cli roundtrip --in work/chap0_text/c00_001_018.stx
# stx (dialogue): round-trip byte-equal (… bytes)
```

`roundtrip` parses the file, re-emits it, and exits non-zero if the
bytes diverge.

---

## Library usage

The CLI is one consumer of the libraries; the same surface is available
to any Rust program. A few short examples:

### Walk a CPK from your own code

```toml
# Cargo.toml
[dependencies]
drv3-cpk = { path = "path/to/danganronpa-v3-tools/crates/drv3-cpk" }
```

```rust
use drv3_cpk::Cpk;

let bytes = std::fs::read("partition_resident_win.cpk")?;
let cpk = Cpk::parse(&bytes)?;

for file in &cpk.files {
    println!("{}/{}  ({} bytes, id={})",
        file.dir_name, file.file_name, file.data.len(), file.id);
}
# Ok::<(), Box<dyn std::error::Error>>(())
```

### Translate an STX file in place

```toml
[dependencies]
drv3-stx = { path = "path/to/danganronpa-v3-tools/crates/drv3-stx" }
```

```rust
use drv3_stx::Stx;

let mut stx = Stx::parse(&std::fs::read("dialogue.stx")?)?;
for entry in &mut stx.tables[0].entries {
    if entry.text == "Press any button" {
        entry.text = "Drücke eine Taste".into();
    }
}
std::fs::write("dialogue.stx", stx.to_bytes()?)?;
# Ok::<(), Box<dyn std::error::Error>>(())
```

### Find and edit a dialogue line inside a CPK

The formats nest CPK → SPC → STX. The lookup helpers (`Cpk::file`, `Spc::entry`,
`StxTable::entry`, plus their `*_mut` variants) make drilling in and editing a
single line concise:

```rust
use std::borrow::Cow;
use drv3_cpk::Cpk;
use drv3_spc::Spc;
use drv3_stx::Stx;

let bytes = std::fs::read("partition_data_win_us.cpk")?;
let mut cpk = Cpk::parse(&bytes)?;

if let Some(file) = cpk.file_mut("wrd_script/003", "chap0_text_US.SPC") {
    let mut spc = Spc::parse(&file.data)?;
    if let Some(member) = spc.entry_mut("c00_001_018.stx") {
        let mut stx = Stx::parse(&member.data)?;
        if let Some(entry) = stx.tables[0].entry_mut(0) {
            entry.text = "Drücke eine Taste".into();
        }
        member.data = stx.to_bytes()?;          // STX bytes back into the SPC member
    }
    file.data = Cow::Owned(spc.to_bytes()?);    // SPC bytes back into the CPK file
}

std::fs::write("patched.cpk", cpk.to_bytes()?)?;
# Ok::<(), Box<dyn std::error::Error>>(())
```

Full per-crate API documentation: `cargo doc --workspace --no-deps --open`.

---

## JSON exchange formats

Several tools emit human-editable JSON sidecars:

- `drv3-cli {stx,dat,wrd,spft} dump | build` — round-trip a single
  file format through a JSON sidecar.
- `drv3-cli {cpk,spc} extract | pack` — alongside the file bodies,
  write/read a `manifest.json` that carries every byte of metadata
  the writer needs.
- `drv3-translate-cli apply | validate` — consume one or more
  `drv3-translate/v1` translation patch JSONs.

Full schemas with worked examples live in
[`docs/json-schemas.md`](docs/json-schemas.md). SRD has no JSON
exchange and no extract/pack — `srd inspect` only prints a structural
block tree.

---

## Building from source

- **Rust stable**, edition 2024. The toolchain channel is declared in
  [`rust-toolchain.toml`](rust-toolchain.toml) (it tracks the current stable
  release); the MSRV is the `rust-version` in `Cargo.toml` (currently 1.96).
- Build everything:

  ```sh
  cargo build --workspace --release
  ```

- Run the formatter and linter before pushing:

  ```sh
  cargo fmt --all
  cargo clippy --workspace --all-targets -- -D warnings
  ```

---

## Tests

```sh
# Workspace tests — full unit + integration suite, no external data needed.
cargo test --workspace

# Real-game-data integration test — gated behind #[ignore].
# Place a shipped CPK at gamedata/partition_resident_win.cpk first.
cargo test -p drv3-cpk -- --ignored real_data
```

---

## Project layout

```text
.
├── crates/                    every Rust crate lives here
│   ├── drv3-binio/            foundation
│   ├── drv3-cli/              CLI binary (drv3-cli)
│   ├── drv3-compression/      CRILAYLA + SPC-LZSS
│   ├── drv3-cpk/              CPK archive + @UTF table
│   ├── drv3-dat/              DAT typed tables
│   ├── drv3-dto/              JSON DTOs for the dump/build exchange schema
│   ├── drv3-dto-patch/        JSON DTOs for the translation-patch schema
│   ├── drv3-spc/              SPC archive
│   ├── drv3-spft/             SpFt font metadata
│   ├── drv3-srd/              SRD block container
│   ├── drv3-stx/              STX string tables
│   ├── drv3-translate/        translation patch engine
│   ├── drv3-translate-cli/    CLI binary, front-end for drv3-translate
│   └── drv3-wrd/              WRD byte-code scripts
├── docs/
│   ├── binary-formats.md      reverse-engineering reference for DR V3 on-disk bytes
│   └── json-schemas.md        JSON sidecar + translation-patch schemas
├── gamedata/                  gitignored: your shipped CPKs
├── CONTRIBUTING.md            coding conventions and comment style
└── README.md                  this file
```

---

## Known limitations

- **CRILAYLA decompression is intentionally not implemented.** None of
  *Danganronpa V3*'s CPKs apply CRILAYLA to TOC entries; the parser
  detects compressed entries and refuses them with a clear error.
  Implementing the codec would add complexity for zero behavioral
  change on the data this toolkit targets.
- **SPC re-pack preserves the original metadata via `manifest.json`**
  (entry order, compression flags, archive-level `unknown1` /
  `unknown2`, per-entry `unknown_flag`). The LZSS *encoder* is
  non-deterministic — many valid encodings exist for the same
  uncompressed input — so byte-equal compressed output is not
  guaranteed. The game's decoder reads the same bytes back either way.
- **Font-atlas pixel writing happens through `drv3-translate-cli`.**
  Standalone `spft build` only edits SPFT metadata; the atlas pixels
  live in the parallel `.srdv` SPC member and are rewritten by
  `drv3-translate-cli apply` when a font group carries glyph PNGs. The
  BC4 re-encoder reproduces the `r0 > r1` ramp mode used by every
  shipped DR V3 atlas; the rare `r0 < r1` mode is decoded but
  re-encoded as the equivalent 8-stop linear ramp, which can shift a
  handful of pixels by a small amount on round-trip.
- **Only the US Windows release is verified in-engine.** The toolkit
  works on the data from any platform CPK in principle, but JP /
  PS-Vita / PS4 releases may differ in details we haven't seen.

---

## Contributing

Coding conventions, doc-comment requirements, lint policy, and the
pre-PR checklist live in [`CONTRIBUTING.md`](CONTRIBUTING.md). The
reverse-engineering reference for the DR V3 binary formats lives in
[`docs/binary-formats.md`](docs/binary-formats.md), and the JSON
schemas the tools emit are specified in
[`docs/json-schemas.md`](docs/json-schemas.md).

---

## Acknowledgments and license

Parts of this codebase — implementation, tests, and documentation —
were written with the help of large language models (Anthropic's
Claude). All design decisions and the final form of every file remain
the responsibility of the human maintainers.

Licensed under the **MIT License** — see [`LICENSE`](LICENSE) for the
full text.
