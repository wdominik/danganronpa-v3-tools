# Contributing

Coding conventions and comment style for the `danganronpa-v3-tools` Rust
workspace. The guidance below is opinionated where Rust itself isn't. We run a
stricter-than-default lint set — the full clippy `pedantic` group plus several
`rustc` lints, with a few documented carve-outs (see §11). Everywhere the
workspace lints don't override, we follow the official [Rust API
guidelines](https://rust-lang.github.io/api-guidelines/) and
[rustfmt](https://github.com/rust-lang/rustfmt) / [clippy](https://github.com/rust-lang/rust-clippy)
defaults.

If a convention here conflicts with a newer official Rust recommendation,
the official recommendation wins — please open a PR that updates this file.

---

## 1. Project shape

The repository is a Cargo workspace with twelve crates under `crates/`:

```
drv3-binio  ──┬── drv3-cpk
              ├── drv3-compression ── drv3-spc
              ├── drv3-stx
              ├── drv3-dat
              ├── drv3-wrd
              ├── drv3-srd
              ├── drv3-spft
              ├── drv3-translate ── drv3-translate-cli
              └── drv3-cli ── (depends on every format crate above)
```

- **Format crates** (`drv3-cpk`, `drv3-spc`, `drv3-stx`, `drv3-dat`,
  `drv3-wrd`, `drv3-srd`, `drv3-spft`) parse and write one file format
  each. They share no format knowledge — each owns its container types.
- **Foundation crates** (`drv3-binio`, `drv3-compression`) provide
  primitives reused by the format crates.
- **`drv3-translate`** is a library: a serde-free patch engine that
  consumes the format crates (`drv3-cpk`, `drv3-spc`, `drv3-stx`,
  `drv3-srd`, `drv3-spft`) and applies translation patches to parsed
  CPKs in memory.
- **`drv3-cli`** and **`drv3-translate-cli`** are the two binaries.
  `drv3-cli` owns the dump/build JSON-exchange DTOs; `drv3-translate-cli`
  owns the patch-JSON schema. Library crates stay serde-free.

Each crate's `lib.rs` opens with a module-level `//!` doc comment that
describes the format's on-disk layout. **Read that header before touching
the crate** — it is the single source of truth for what the bytes mean.

The companion document `docs/binary-formats.md` is a longer human-readable
reverse-engineering reference for the on-disk DR V3 byte layouts.
`docs/json-schemas.md` specifies the JSON sidecar and translation-patch
schemas the CLIs emit. Code comments must remain **self-contained**
and never reference either document directly (see §8).

---

## 2. Toolchain & build

- **Rust stable**, pinned in `rust-toolchain.toml`. The MSRV
  (`rust-version` in the workspace `Cargo.toml`) tracks the latest stable
  release — currently 1.96 — so the modern idioms in §12 are always
  available; bumping it requires a workspace-wide compile check.
- **Edition 2024** across every crate.
- **`rustfmt` defaults** — no custom `rustfmt.toml`. Run
  `cargo fmt --all` before pushing.
- **`clippy` with workspace lints** — see §11. The full pre-merge gate is:

  ```sh
  cargo fmt --all --check
  cargo clippy --workspace --all-targets -- -D warnings
  cargo test --workspace
  cargo doc --workspace --no-deps
  ```

  All four must pass. These same commands run on every push and PR via
  [`.github/workflows/ci.yml`](.github/workflows/ci.yml); CI green is
  the canonical merge signal.

---

## 3. Module & file organization

- **One concern per module.** A module exists because it groups related
  types and functions, not because of a directory naming convention.
- **Re-export the public surface from `lib.rs`** so call sites can write
  `drv3_cpk::Cpk` rather than `drv3_cpk::archive::Cpk`. Use
  `pub use module::Item;` deliberately — the re-export *is* the API.
- **Inline unit tests** at the bottom of each source file, gated by
  `#[cfg(test)] mod tests { use super::*; … }`. They can reach private
  items.
- **Integration tests** live under `crates/<crate>/tests/*.rs` and may only
  use the crate's public API. Reserve them for end-to-end scenarios.
- **`unreachable_pub`** is set to `warn`; any `pub` item not actually
  reachable from outside the crate trips it. Use `pub(crate)` instead.

---

## 4. Naming

Standard Rust naming applies:

| Item | Style | Examples |
|---|---|---|
| Crates | `kebab-case` | `drv3-cpk`, `drv3-binio` |
| Modules | `snake_case` | `archive`, `utf`, `cpk_manifest` |
| Types, traits, enum variants | `UpperCamelCase` | `Cpk`, `UtfTable`, `StorageFlag::PerRow` |
| Functions, methods, fields | `snake_case` | `parse`, `to_bytes`, `header_columns` |
| Constants, statics | `SCREAMING_SNAKE_CASE` | `PACKET_WRAPPER_SIZE`, `DEFAULT_ALIGN` |
| Type parameters | `T`, `U`, or descriptive `UpperCamelCase` | `T`, `R: Read` |
| Lifetimes | short lowercase, `'a` first | `'a`, `'mmap` |

### Format-derived names

Field and type names that map directly to bytes in a CRI / DR V3 file are
**load-bearing** and must match the on-disk identifier — even when this
produces clusters like `unknown_10`, `unknown_1a`, `unknown_1d`. Renaming
them loses the on-disk correspondence that reviewers rely on when tracing
bytes to fields.

The `clippy::module_name_repetitions` lint is allowed off across the
workspace because `drv3_stx::StxTable` reads better at call sites than
`drv3_stx::Table` (the type often travels into binaries that already
import multiple `Table` analogues).

### Abbreviations

Avoid abbreviations in identifiers (`description`, not `desc`). Format
magic words (`CPK`, `STX`, `RSI`, `SpFt`) are kept verbatim because they
are proper nouns in the file format.

---

## 5. Visibility

- Default to `pub(crate)`; promote to `pub` only when an item is part of
  the published API surface of the crate.
- `pub(super)`, `pub(in path)` are fine — prefer them over `pub` when the
  visibility is bounded.
- **Don't leak internal types** from public function signatures. If a
  return type pulls a private type into the public API, either make that
  type `pub` deliberately or wrap it.
- Re-exports in `lib.rs` define the API. Anything not re-exported is
  internal even if it's `pub`.

---

## 6. Imports

Group imports in three blocks separated by a blank line — `std`, then
external crates, then `crate`/local:

```rust
use std::collections::HashMap;
use std::fs;

use anyhow::{Context, Result};
use clap::Parser;
use thiserror::Error;

use crate::dto::cpk_manifest::CpkManifestJson;
use crate::utf::{StorageFlag, UtfTable};
```

- **Bring types into scope** rather than fully qualifying. Write
  `Vec<u8>`, not `std::vec::Vec<u8>`.
- **No glob imports** (`use foo::*;`) outside test modules. In test
  modules, `use super::*;` is the standard convention and is encouraged.
- Order imports alphabetically within each block — stable rustfmt does this
  automatically (`reorder_imports`, on by default).
- The three-block grouping itself is a **manual convention**: rustfmt's
  default `group_imports = "Preserve"` sorts *within* blocks but never
  creates or reorders them, so keep the `std` / external / `crate` split by
  hand.

---

## 7. Errors

- **Libraries with failure modes beyond raw I/O** (`drv3-cpk`, `drv3-spc`,
  `drv3-compression`, `drv3-translate`) use
  [`thiserror`](https://docs.rs/thiserror) to derive a per-crate error
  enum. Each enum wraps `drv3_binio::BinError` via `#[from]` where the
  underlying I/O error is the failure cause.

  ```rust
  #[derive(Debug, Error)]
  pub enum CpkParseError {
      #[error(transparent)]
      Bin(#[from] BinError),

      #[error("missing required column {0:?} in {1}")]
      MissingColumn(String, &'static str),
  }
  ```

  The simpler format crates (`drv3-dat`, `drv3-spft`, `drv3-srd`,
  `drv3-stx`, `drv3-wrd`) introduce no error states of their own — they
  propagate `drv3_binio::{BinError, BinResult}` directly and don't depend
  on `thiserror`.

- **Binaries** (`drv3-cli`) use `anyhow::Result` at the handler boundary.
  Library errors propagate naturally via `?`; use `.with_context(|| …)`
  to add path or operation context.
- **Never `panic!` / `unwrap` / `expect` on user input.** Use `Result`.
  Internal-invariant unwraps (where the panic is genuinely unreachable in
  practice) are acceptable but must carry an `.expect("…")` message that
  explains why the invariant holds.
- **`debug_assert_eq!`** is the right tool for self-consistency checks
  inside multi-pass writers (the assertion fires only in debug builds; in
  release builds the writer trusts its own planner).
- **Error enums likely to grow** get `#[non_exhaustive]` so adding a
  variant is not a breaking change.

---

## 8. Comments

The central rule: **comments are self-contained**.

- **No external doc references.** Comments must not say "see the format
  docs", "per §3.7", "per the spec", or similar — keep every comment
  self-contained. The rationale lives in §16.
- **WHY, not WHAT.** Identifiers describe what the code does. Comments
  describe the things a reader can't see:
  - Format quirks (`flag bytes are stored with bits reversed because…`)
  - Round-trip invariants (`preserved verbatim so byte-equal round-trip
    works`)
  - Hidden constraints (`CRIWARE loader DMAs from this offset, so it must
    be Align-aligned`)
  - Workarounds (`compute as (a + b) - c to avoid unsigned underflow when…`)
- **Don't restate code.** If removing a comment wouldn't confuse a
  reader who knows Rust, the comment shouldn't exist.
- **Style**:
  - `///` for items that appear in the public API or in `cargo doc`.
  - `//` for impl-internal and inline comments.
  - **`///` doc comments are complete sentences ending in a period**, per
    std / rustdoc convention. Terse inline `//` notes can stay fragmentary.
    (Existing comments predate this — match the standard in new and edited
    code rather than mass-rewriting.)
  - **Imperative present tense** for action descriptions ("Parse the
    header", "Reverse the bit order"). **Descriptive form** for return
    values ("Returns the header layout").
  - Comments wrap with the surrounding line width (~100 columns).

### Bit-layout diagrams

Non-obvious bit manipulations get an inline diagram in a ` ```text ` code
fence. The diagram lives next to the code so a reader doesn't have to look
elsewhere:

````
/// Pack two 12-bit unsigned coordinates `(x, y)` into three bytes.
///
/// The on-disk layout interleaves x and y so their high nibbles share the
/// middle byte:
///
/// ```text
/// byte a (low 8 bits): x[0..8]
/// byte b (high nibble = y[0..4], low nibble = x[8..12])
/// byte c (low 8 bits): y[4..12]
/// ```
///
/// Together a/b/c encode 12 + 12 = 24 bits in 3 bytes with no wasted bits.
pub fn xy_to_abc(x: u16, y: u16) -> (u8, u8, u8) { … }
````

### On-disk layout headers

Every format-crate's `lib.rs` opens with a module-level summary that
sketches the file's byte layout in the same fenced-text style. Example
template:

````
//! Danganronpa V3 STX string-table reader/writer.
//!
//! STX is the primary translation target …
//!
//! ```text
//! offset 0x00  4 bytes   magic "STXT"
//! offset 0x04  4 bytes   secondary magic "JPLL"
//! offset 0x08  4 bytes   table_count u32 LE
//! offset 0x0C  4 bytes   table_offset u32 LE
//! offset 0x10  …         table-info entries (16 bytes each)
//! …                      index array, string data
//! ```
````

This is the only consistent way we've found to convey on-disk structure
without forcing a contributor to open the docs.

---

## 9. Doc-comments

Every `pub` function and type gets a `///` doc comment. The compiler
enforces a subset of this — `missing_errors_doc` and `missing_panics_doc`
(via clippy `pedantic`) require `# Errors` / `# Panics` on the relevant
`pub fn`s. Blanket `missing_docs` is deliberately *not* enabled: the format
crates expose many `pub` struct fields and enum variants (`unknown_2c`,
`StorageFlag::PerRow`) whose meaning lives in the crate's on-disk layout
header (§8), and a per-field `///` would be the noise §8 forbids.

### Required structure

1. **First line: a one-sentence summary** ending in a period.
2. **Blank line**, then optional explanatory paragraphs.
3. Sections in this order when present: `# Errors`, `# Panics`, `# Safety`,
   `# Examples`.

### `# Errors` (required on every `pub fn` returning `Result`)

The workspace lint `missing_errors_doc` is set to `warn`, so clippy fails
without one. Describe **when** the function errors, in plain prose. Don't
describe the error type — its variants document themselves.

```rust
/// Parse a CPK archive from a byte buffer.
///
/// # Errors
///
/// Returns an error if the header packet is malformed, the TOC schema
/// declares an unknown column type, the @UTF string pool contains
/// invalid UTF-8, or any file body's declared offset extends past the
/// end of the buffer.
pub fn parse(input: &[u8]) -> CpkResult<Self> { … }
```

### `# Panics` (required on every `pub fn` that can panic on its arguments)

Enforced by `missing_panics_doc`. If a function only panics on unreachable
internal invariants (e.g. an `expect` whose message documents the
invariant), it doesn't need a `# Panics` section — the panic is a bug, not
behavior.

### `# Safety` (required on every `unsafe fn`)

Document the contract the caller must uphold. The two existing examples are
`drv3-cli::mmap_file` and `drv3-translate-cli::mmap_file`, which share the
same pattern:

```rust
// SAFETY: file is opened read-only and used only for the duration of this
// call's caller; no concurrent writer is expected on a game-data CPK.
let mmap = unsafe { memmap2::Mmap::map(&file) }?;
```

### `# Examples` (encouraged for non-trivial public APIs)

Format-crate `parse` / `to_bytes` functions are good candidates. Examples
should be runnable doctests where possible:

```rust
/// ```no_run
/// use drv3_stx::Stx;
/// let bytes = std::fs::read("dialogue.stx")?;
/// let stx = Stx::parse(&bytes)?;
/// for entry in &stx.tables[0].entries {
///     println!("{}: {}", entry.id, entry.text);
/// }
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
```

### Intra-doc links

Reference other items with `` [`Name`] `` syntax, not markdown URLs:

```rust
/// See [`Cpk::default_toc_columns`] for the canonical schema.
```

### Error-variant docs

Every variant of every public error enum gets a one-line `///`
description, explaining the condition that produces it.

---

## 10. Tests

- **Unit tests** at the bottom of source files, in `#[cfg(test)] mod tests`.
- **Integration tests** in `crates/<crate>/tests/*.rs`.
- **Real-game-data tests** gated behind `#[ignore = "requires
  gamedata/…; run with --ignored"]`. They run via
  `cargo test -p <crate> -- --ignored`.

### Test naming

Test functions read as full sentences in `snake_case`:

```rust
#[test]
fn etoc_lives_at_file_end_when_present() { … }

#[test]
fn unsorted_cpks_are_tightly_packed() { … }

#[test]
fn flag_0x16_is_zero_not_constant() { … }
```

A failing test name should already tell the reader what behavior is
broken.

### One behavior per test

Don't combine assertions that exercise unrelated logic. If a test
combines a setup invariant with an output assertion, that's one
behavior and is fine.

### `Result`-returning tests

For tests that exercise `?` heavily, prefer the signature
`#[test] fn name() -> Result<(), Box<dyn Error>>` so `.unwrap()` calls
disappear.

### `assert_eq!` over `assert!`

```rust
assert_eq!(parsed.files.len(), 1597);          // ✅ shows both sides on failure
assert!(parsed.files.len() == 1597);           // ❌ shows only "false"
```

---

## 11. Linting

The workspace `Cargo.toml` declares lints once for every crate:

```toml
[workspace.lints.rust]
unsafe_code = "deny"
missing_debug_implementations = "warn"
unreachable_pub = "warn"

[workspace.lints.clippy]
pedantic = { level = "warn", priority = -1 }
module_name_repetitions = "allow"
must_use_candidate = "allow"
cast_possible_truncation = "allow"
cast_sign_loss = "allow"
cast_lossless = "allow"
```

### Active lints worth knowing

- **`unsafe_code = "deny"`** — every unsafe block requires a per-site
  `#[allow(unsafe_code)]` with a `// SAFETY: …` comment. Workspace-wide
  there are exactly two such sites (the mmap calls in `drv3-cli` and
  `drv3-translate-cli`).
- **`pedantic = warn`** — the full pedantic group is enabled. New
  contributors should expect clippy to flag style issues that the default
  group misses.
- **`missing_errors_doc = warn`** (via pedantic) — every public
  `Result`-returning function needs a `# Errors` section. See §9.
- **`missing_panics_doc = warn`** (via pedantic) — every public function
  that can panic on its arguments needs a `# Panics` section.
- **`missing_debug_implementations = warn`** — every public type
  implements `Debug`. Format crates derive it on their model types.
- **`unreachable_pub = warn`** — any `pub` not actually exported is
  flagged; use `pub(crate)` instead.

### Why the five carve-outs

| Lint | Why allowed off |
|---|---|
| `module_name_repetitions` | Format-derived type names like `StxTable` / `CpkFile` repeat the crate name on purpose — readability at call sites wins over deduplication. |
| `must_use_candidate` | Pedantic wants `#[must_use]` on every pure value-returning function. For dozens of tiny accessors (`UtfValue::ty()`, `has_name()`) the annotation is rote noise. |
| `cast_possible_truncation`, `cast_sign_loss`, `cast_lossless` | The codebase has ~160 numeric casts driven by format specifications (`u32 ↔ usize` for offsets, `u64 ↔ u32` for sizes). Most are correct by construction, not by accident. A future audit may tighten this; for now the cost outweighs the safety win. |

**New casts** should not lean on the workspace allow. Because these lints
are `allow`ed workspace-wide, the compiler won't flag a new unannotated
cast — so this is a **reviewer-enforced convention, not a hard gate**: any
*new* cast gets a per-site `#[expect(clippy::cast_possible_truncation,
reason = "…")]` (or the appropriate sibling lint) that names the invariant
making the cast safe. Reviewers surface unannotated casts by re-enabling the
lints:

```sh
cargo clippy --workspace --all-targets -- -D warnings \
    -W clippy::cast_possible_truncation \
    -W clippy::cast_sign_loss \
    -W clippy::cast_lossless
```

### `#[expect]` over `#[allow]`

For per-site lint suppressions where the reason is worth recording,
prefer `#[expect(lint, reason = "…")]` over `#[allow(lint)]`. `expect`
fires if the lint *stops* being relevant, catching annotations that have
outlived their purpose.

---

## 12. Newer Rust idioms to prefer

Edition 2024 and recent stdlib additions provide ergonomic alternatives
to older patterns. Use the modern form unless there's a reason not to. All
of these are within the workspace MSRV (§2); when you reach for a
newer-than-MSRV feature, bump `rust-version` first:

| Modern | Old |
|---|---|
| `let Some(x) = option else { return Err(…) }` | `let x = if let Some(x) = option { x } else { … }` |
| `if let Some(x) = a && let Some(y) = b { … }` (chains) | nested `if let` |
| `option.is_some_and(\|x\| x > 0)` | `option.map(\|x\| x > 0).unwrap_or(false)` |
| `option.is_none_or(\|x\| x > 0)` | `option.map(\|x\| x > 0).unwrap_or(true)` |
| `result.inspect(\|x\| …)` | `result.map(\|x\| { …; x })` |
| `std::sync::OnceLock`, `LazyLock` | `once_cell::sync::OnceCell` |
| Range patterns `0..=255` | `matches!(x, 0..=255)` (and `if x >= 0 && x <= 255`) |
| `&str` parameter | `&String` parameter |
| `impl AsRef<Path>` for paths | `&Path` directly |
| `&[T]` parameter | `&Vec<T>` parameter |
| `Vec::extract_if` | manual `retain` + collect |

Avoid `unwrap()` in non-test code; use `.expect("…")` with an explanatory
message when a panic is genuinely unreachable.

---

## 13. Unsafe code

- **Default**: workspace-wide `unsafe_code = "deny"`. No unsafe in
  libraries.
- **Exception**: two `#[allow(unsafe_code)]` sites, in
  `drv3-cli/src/main.rs::mmap_file` and
  `drv3-translate-cli/src/main.rs::mmap_file`, both for `memmap2::Mmap::map`,
  each with a `// SAFETY: …` comment explaining the contract.
- **New unsafe** anywhere else requires PR discussion. Document the
  invariant the caller must uphold using a `# Safety` section on `unsafe
  fn`, and a `// SAFETY: …` comment on each `unsafe { … }` block
  explaining why the contract holds at that call site.

---

## 14. Performance & memory

- **`memmap2`** for files ≥ ~64 MB. The CPK list/extract paths in the
  CLI already use it; the kernel pages bytes in on demand instead of
  forcing a 12 GB heap allocation for the largest single shipped archive.
- **Preallocate `Vec` capacity** when the final size is known up front
  (`Vec::with_capacity(n)`). The format writers do this for header,
  schema, row-data, string pool, data blob buffers.
- **No allocations in hot decompression loops** beyond the output
  buffer. The SPC-LZSS decoder reuses a single output `Vec<u8>`
  pre-sized to the expected output length.
- **Benchmarks are deferred.** Profile with `samply` or
  `cargo flamegraph` when a specific path needs attention. The
  architecture doesn't preclude benchmarks; they just aren't part of
  v0.1.

---

## 15. Round-trip discipline (format crates)

Every format crate must support **at least semantic round-trip**:
`parse(write(x)) == x`. Where possible, support **byte-for-byte
round-trip**: `write(parse(bytes)) == bytes`.

### Required practices

- **Preserve every "unknown" field.** If the format has a byte the
  reverse-engineering hasn't pinned down, give it a named field
  (`unknown_4`, `unknown_2c`) on the model and round-trip it verbatim.
  Future-you will thank present-you.
- **Document what's recomputed** vs. preserved. The CPK writer recomputes
  layout-derived fields (offsets, sizes, file count) and preserves
  everything else.
- **Real-data round-trip tests** where possible. Gate them behind
  `#[ignore]` so they don't run by default (the test suite must work
  without the shipped game files on disk), but make them easy to opt into:

  ```rust
  #[test]
  #[ignore = "requires gamedata/partition_resident_win.cpk; run with --ignored"]
  fn partition_resident_round_trips() { … }
  ```

- **Synthetic round-trip tests** in unit-test modules. These exercise the
  code path in CI without needing the full ~23 GB of game archives on disk.

### Bit-equal vs. semantic

A few formats can only be semantically round-tripped:

- **CPK content blob**: file bodies and the @UTF tables round-trip
  bit-equally; padding bytes between files are not — the original
  encoder sometimes leaves uninitialized memory there, our writer
  zero-pads. The game's runtime ignores any byte past a file body's
  declared `FileSize`.
- **SPC LZSS-compressed entries**: many valid encodings exist for the
  same input; we don't guarantee re-emitting the original encoder's
  exact bytes. Round-trip is `decompress(compress(x)) == x`.

When a format can only be semantically round-tripped, **say so in the
crate's `lib.rs` module header** — and explain why.

---

## 16. Code / docs separation

The repository carries two reference documents:

- `docs/binary-formats.md` — human-readable reverse-engineering
  reference for the DR V3 on-disk byte layouts.
- `docs/json-schemas.md` — specification for the JSON sidecars and
  translation-patch documents the CLIs emit.

The Rust code carries its own documentation in `///` comments.
Neither side is the canonical source of truth for the other — they
are two views of the same understanding.

- **Code comments never reference the docs.** No "see
  `docs/binary-formats.md`", no "see `docs/json-schemas.md`", no
  `§X.Y`. If a fact lives in the docs and the code depends on it, the
  fact must also live in the code's comments in plain prose.
- **Docs may reference the code** for implementation specifics, since
  the docs are written for humans browsing the project. The JSON
  schemas in `docs/json-schemas.md` point at
  `crates/drv3-cli/src/dto.rs` and `crates/drv3-translate-cli/src/dto.rs`
  as their authoritative sources; the binary-format reference points
  at the per-crate `lib.rs` module headers.

Both can evolve independently: the docs as our understanding of the
formats deepens, the code as we fix bugs or add features.

---

## 17. Language

All prose in this repository — `README.md`, `CONTRIBUTING.md`, the
files under `docs/`, every `///` doc-comment, every `//` inline
comment — uses **American English** spellings:

- *-ize*, not *-ise*: `serialize`, `normalize`, `organize`,
  `recognize`, `analyze`.
- *-or*, not *-our*: `color`, `behavior`, `favor`, `honor`.
- *-er*, not *-re*: `center`, `fiber`.
- *-se*, not *-ce* for the noun forms of `defense`, `license`.
- Single-l past tense: `canceled`, `labeled`, `modeling`.

Domain terms borrowed from the game stay verbatim. The most common
one is **"dialogue"** — kept everywhere it refers to in-game spoken
or written exchanges (the game's translation target). American
English uses "dialogue" for that sense; "dialog" is reserved for UI
dialog boxes, which this project doesn't have.

Quoted excerpts from external sources keep their original spelling.

---

## Quick checklist before opening a PR

```
[ ] cargo fmt --all --check
[ ] cargo clippy --workspace --all-targets -- -D warnings
[ ] cargo test --workspace
[ ] cargo doc --workspace --no-deps
[ ] no docs/ references or §X.Y citations in code comments
[ ] American English in new prose (see §17)
[ ] in-game text uses "dialogue", UI boxes use "dialog" (see §17)
[ ] new pub Result-returning fns have # Errors sections
[ ] new pub panicking fns have # Panics sections
[ ] no new unsafe code (or, if needed, # Safety + // SAFETY explained)
[ ] CHANGELOG or PR description notes user-visible changes
```
