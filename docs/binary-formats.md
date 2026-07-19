# Danganronpa V3 — Binary File Formats Reference

A self-contained specification of the binary file formats used by Danganronpa V3:
Killing Harmony. Written so a developer with binary parsing experience and no prior
exposure to this game, CRIWARE tooling, or the modding community can implement
readers and writers in any language without consulting source code.

This document covers **what the bytes mean**. It does not cover tooling, library
choices, translation workflows, or end-to-end patching pipelines — those topics
belong in separate documents.

---

## Table of contents

1. [Overview](#1-overview)
2. [Common primitives & conventions](#2-common-primitives--conventions)
3. [CPK — outer archive](#3-cpk--outer-archive)
4. [SPC — inner archive container](#4-spc--inner-archive-container)
5. [STX — string table](#5-stx--string-table)
6. [DAT — typed binary tables](#6-dat--typed-binary-tables)
7. [WRD — script files](#7-wrd--script-files)
8. [SRD — block container](#8-srd--block-container)
9. [SpFt FontBlock](#9-spft-fontblock)
10. [GPU texture formats](#10-gpu-texture-formats)
11. [Cross-format notes](#11-cross-format-notes)
12. [Out of scope](#12-out-of-scope)
13. [References](#13-references)

---

## 1. Overview

The game ships its data inside a small number of large `.cpk` archive files. To
reach an individual translatable string you must traverse a nest of containers:

```text
.cpk  (CRIWARE archive — outer container)
 └─ .spc  (custom archive — inner container, usually compressed)
     ├─ .stx   (string tables — dialogue and menu text)
     ├─ .dat   (typed tables — UI labels, item data, etc.)
     ├─ .wrd   (byte-code scripts — drive dialogue scenes, reference STX strings)
     └─ .stx + .srdv [+ .srdi]   (SRD block containers — font atlases, textures)
                                  ^ same .stx extension as string tables, different format
```

| Format | Role | Reader needed? | Writer needed? |
|--------|------|----------------|----------------|
| CPK    | Top-level archive containing all game assets | Yes | Optional — alternative is to ship loose files alongside the original |
| SPC    | Inner archive bundling related game-data files | Yes | Yes — STX/DAT/SRD changes must be repacked into the parent SPC |
| STX    | String table — primary translation target | Yes | Yes |
| DAT    | Typed binary table — secondary translation target | Yes | Yes |
| WRD    | Byte-code script paired with STX; provides speaker / choice metadata for dialogue | Yes | No — never modified by translation work |
| SRD    | Block container for textures and font atlases | Yes | Yes — needed if patching the font (adding new glyphs for non-Japanese characters) |
| SpFt   | Font metadata blob embedded inside an SRD `$RSI` block | Yes | Yes — patching a font means rewriting this blob and the texture atlas it references |

One other format appears inside the archives but is out of scope (see
[§12](#12-out-of-scope)):

- **`$CMP` wrapper** — an SRD-compressed variant of SPC; treat as opaque and skip.

> **Note on the `.stx` extension.** Two different file formats use the `.stx`
> extension: dialogue **String Tables** (§5) inside dialogue SPCs, and **SRD
> block containers** (§8) inside font/texture SPCs. They share nothing but the
> extension. Distinguish by reading the first 4 bytes: `STXT` → dialogue STX,
> `$CFH` → SRD container. Always check the magic, never decide by file name.

### Reading style of this document

- All byte offsets and sizes are given in hexadecimal (`0x10` = 16 bytes).
- Integer widths are specified as `u8`, `u16`, `u32`, `s32`, etc.
- Endianness is called out per field. Unless stated otherwise, multi-byte
  integers are **little-endian**.
- "Pseudocode" uses C-like syntax: `read_u32_le()`, `seek(pos)`, `read_bytes(n)`.
- Each format section ends with a "Reference implementations" pointer naming
  one or more publicly available projects that implement the format. See
  also §13.

---

## 2. Common primitives & conventions

### 2.1 Endianness map

| Format | Default | Exceptions |
|--------|---------|------------|
| CPK    | Big-endian inside `@UTF` tables (table header, row-data values for u16/u32/u64/float); little-endian for the outer packet wrappers and content offsets | none |
| SPC    | Little-endian | none |
| STX    | Little-endian | none |
| DAT    | Little-endian | none |
| WRD    | Little-endian for the file header (counts, pointers); **big-endian for opcode argument values** in the byte-code stream | see [§7.3](#73-byte-code-stream) |
| SRD    | Little-endian for block *data payloads*; **big-endian for the per-block header lengths** (`DataLength`, `SubdataLength`, the trailing u32) | see [§8.3](#83-block-wrapper) |
| SpFt   | Little-endian | none |

### 2.2 Character encodings

| Encoding | Where used |
|----------|------------|
| ASCII    | Magic numbers and signatures (`STXT`, `JPLL`, `CPS.`, `@UTF`, `CRILAYLA`, `Root`, `TOC `, `ETOC`, `ITOC`, `GTOC`, `$CFH`, `$TXR`, `$RSI`, `$TRE`, `$TXI`, `$VTX`, `$RSF`, `$CT0`, `SpFt`), DAT column type names |
| UTF-16 LE | STX string data; DAT `utf16` columns; WRD internal dialogue strings; SpFt font names |
| Shift-JIS | SPC subfile names; WRD label and parameter strings; SRD `$RSI` resource string list |
| UTF-8     | DAT column names; DAT `ascii` / `label` / `refer` columns |

A Shift-JIS decoder is required to enumerate the contents of an SPC archive. On
platforms where Shift-JIS is not built in, a third-party decoder must be linked.

### 2.3 Null-terminator conventions

- **UTF-16 strings** end with a 2-byte `0x00 0x00` terminator. String byte
  lengths do **not** include the terminator.
- **Shift-JIS / ASCII names** end with a 1-byte `0x00` terminator. Stored
  `NameLength` fields exclude the terminator; the file layout reserves
  `NameLength + 1` bytes for the name plus terminator.
- **UTF-8 strings** in DAT column names end with `0x00`.

### 2.4 Alignment

Several alignment boundaries appear repeatedly:

- **16-byte (0x10)** — used inside SPC subfile entries (around names and data
  blocks), inside CPK UTF tables, and **between every SRD block and its
  subdata** (each block's data and subdata payloads are individually padded
  to 0x10).
- **2048-byte (0x800)** — typical alignment for the content blob inside a CPK
  (`Align` field in the CPK header is authoritative).
- **2-byte (0x02)** — DAT string pool boundary between UTF-8 and UTF-16 strings.

Padding bytes are always `0x00`.

When a section says "pad to N", compute:

```text
pad_size = (N - (current_length % N)) % N
```

### 2.5 Round-trip preservation

Every format in this document contains "unknown" fields whose meaning is not
fully documented. **A correct writer must round-trip these bytes verbatim from
the source file.** Specifically:

- `Unknown1` (0x24 bytes) and `Unknown2` (4 bytes) in the SPC header.
- The per-table `Unknown` field (4 bytes) in STX.
- `UnknownFlag` (2 bytes) on each SPC subfile.
- `unknown1` (4 bytes) in the WRD header.
- The 16-byte (`0x10`) padding region at SPC offset `0x30`.
- All `UnknownNN` fields in SRD `$TXR` (`Unknown10`, `Unknown1D`) and `$RSI`
  (`Unknown10`, `Unknown11`, `Unknown12`, `Unknown1A`) blocks.
- `Unknown6` (4 bytes) and `ScaleFlag` (4 bytes) in the SpFt FontBlock header.

A round-trip test (read → write → byte-compare to original) on every untouched
file in a CPK is the recommended way to validate a new implementation.

---

## 3. CPK — outer archive

CRIWARE Middleware archive format. Used across many Japanese games; not
Danganronpa-specific.

### 3.1 Provenance

The CPK format is the file-system container for CRIWARE's CRI File System
(CRI FS) middleware. The authoritative reference implementations are:

- **CriFsV2Lib** (read-only): https://github.com/Sewer56/CriFsV2Lib
- **CriPakTools** (read + write): https://github.com/esperknight/CriPakTools
- **ConnorKrammer/cpk-tools**: https://github.com/ConnorKrammer/cpk-tools

The details below are derived from those public references. They should be
cross-checked against a working CPK before being trusted in a new
implementation, since the format is large and has corners that depend on
the producing tool.

### 3.2 Top-level layout

A CPK file is a sequence of **packets**, each of which wraps a `@UTF` table
(§3.4). At minimum:

```text
+---------------------------+ offset 0
| CPK packet                |  header — single-row @UTF table with global metadata
+---------------------------+
| padding (0x00 bytes)      |  pads `TocOffset` up to the `Align` boundary
+---------------------------+
| TOC packet  (optional*)   |  table of contents — one row per file
+---------------------------+
| ITOC packet (optional)    |  index TOC (accelerator for ID-keyed lookups)
+---------------------------+
| GTOC packet (optional)    |  group TOC (file groupings)
+---------------------------+
| padding (0x00 bytes)      |  pads `ContentOffset` up to the `Align` boundary
+---------------------------+
| content blob              |  raw file data; each file padded to `Align` if `Sorted == 1`
+---------------------------+
| ETOC packet (optional)    |  extended TOC at FILE END — `EtocOffset + EtocSize == FileSize`
+---------------------------+
```

\* TOC is present in every CPK observed in Danganronpa V3. ETOC/ITOC/GTOC may
or may not be present; their offsets in the header packet are `0` when absent.

**Two non-obvious invariants** (verified empirically against both shipped
DR V3 CPKs — `partition_resident_win.cpk` and `partition_data_win_us.cpk`):

- **`TocOffset` is aligned to `Align`** (not just to 16). For both shipped
  CPKs, `TocOffset == 0x800`. CRIWARE's loader issues sector-aligned DMA
  reads at `TocOffset`; an unaligned value can cause the in-game loader to
  spin indefinitely.
- **ETOC lives at the FILE END**: `EtocOffset + EtocSize == FileSize`. The
  loader uses this invariant to bound the content blob (`content_end =
  EtocOffset`). Placing ETOC anywhere else (e.g. between TOC and content)
  causes the runtime to compute a nonsensical content extent.

ITOC and GTOC, when present, live between the TOC packet and the content
blob's leading pad — empirical data is unavailable for DR V3 (neither
shipped CPK uses them); convention is taken from CriPakTools / CriFsV2Lib.

### 3.3 Packet wrapper

Every packet begins with a 16-byte little-endian header:

| Offset | Size | Field      | Notes |
|--------|------|------------|-------|
| 0x00   | 4    | Magic      | `CPK `, `TOC `, `ETOC`, `ITOC`, `GTOC` (each 4 ASCII bytes; the CPK and TOC magics are space-padded to 4 bytes) |
| 0x04   | 4    | Flag       | u32 LE — usage varies; typically `0xFF` or `0x00` |
| 0x08   | 8    | PacketSize | u64 LE — size of the @UTF payload that follows (not including this 16-byte wrapper) |
| 0x10   | …    | @UTF table | The actual payload, see §3.4 |

The packet header bytes (offsets 0x00–0x0F) are little-endian. The `@UTF`
payload that follows is **big-endian**.

After the @UTF payload, packets may be padded with `0x00` bytes up to the next
alignment boundary before the next packet begins.

### 3.4 @UTF table format

CRI's columnar table primitive. Every CPK packet (header, TOC, ETOC, ITOC,
GTOC) contains exactly one @UTF table.

| Offset | Size | Field          | Notes |
|--------|------|----------------|-------|
| 0x00   | 4    | Magic          | `@UTF` (ASCII) |
| 0x04   | 4    | TableSize      | u32 **big-endian** — size of the table, not including these 8 bytes (magic+size) |
| 0x08   | 4    | RowsOffset     | u32 BE — offset from end of the size field (i.e. from byte 0x08) to the row-data section |
| 0x0C   | 4    | StringsOffset  | u32 BE — offset to the string pool |
| 0x10   | 4    | DataOffset     | u32 BE — offset to the binary blob pool |
| 0x14   | 4    | TableNameOff   | u32 BE — offset within the string pool of the table's own name |
| 0x18   | 2    | ColumnCount    | u16 BE |
| 0x1A   | 2    | RowSize        | u16 BE — size of a single row in bytes |
| 0x1C   | 4    | RowCount       | u32 BE |
| 0x20   | …    | ColumnSchema   | ColumnCount entries (see §3.4.1) |
| …      | …    | RowData        | RowCount × RowSize bytes (see §3.4.2) |
| …      | …    | StringPool     | UTF-8 null-terminated strings, indexed by offset |
| …      | …    | DataBlob       | raw binary blobs, indexed by (offset, size) pairs |

All offsets in the @UTF table are relative to **the start of the @UTF magic**
(i.e. offset 0x08 in the packet wrapper from §3.3).

#### 3.4.1 Column schema

Each column descriptor is variable-length; the on-disk layout is:

| Field      | Size | Notes |
|------------|------|-------|
| Flags      | 1 byte | bit layout below |
| NameOffset | 4 bytes (u32 BE) | offset into the string pool — **only present if the name flag is set** |
| Constant   | variable | only present if the storage selector is `Constant` or `Constant2` |

The Flags byte splits as follows. It is **not** a clean high/low nibble — the
storage selector lives in bits 5-6, with bit 4 acting as an independent
"name-present" flag:

```text
bits 0-3 : type nibble (UtfType)
bit  4   : name-present flag    — 1 = a 4-byte name offset follows
bits 5-6 : storage selector     — 00=None, 01=Constant, 10=PerRow, 11=Constant2
bit  7   : reserved / unused in observed files
```

The composite "storage byte" `Flags & 0xF0` takes one of these canonical
values:

| Storage byte | Name | Layout |
|--------------|------|--------|
| `0x00`       | `None`      | flag byte only — no name, no value |
| `0x10`       | `Zero`      | flag + name offset; value is implicit `0` / empty |
| `0x30`       | `Constant`  | flag + name offset + inline value |
| `0x50`       | `PerRow`    | flag + name offset; value lives in the row-data section |
| `0x70`       | `Constant2` | flag + name offset + inline value (alternate constant) |

`Constant` and `Constant2` are layout-identical (both carry an inline value
sized per the type nibble) but the byte distinguishes them on disk and must be
preserved verbatim on round-trip.

> **Historical note.** Earlier revisions of this document described the high
> nibble alone as the storage selector (`0x1 = Constant`, `0x3 = Per-row`,
> `0x5 = Zero`). That mapping is **incorrect** and contradicts every observed
> CRI CPK. In `partition_resident_win.cpk`'s header table each schema entry is
> exactly 5 bytes (flag + name offset, no inline constants) while the flags
> alternate between `0x16` (Zero u64 for `FileSize`) and `0x56` (PerRow u64 for
> `ContentOffset`). The 5-byte uniform layout only fits the canonical encoding
> above.

The type nibble (`Flags & 0x0F`):

- `0x00` — u8
- `0x01` — s8
- `0x02` — u16
- `0x03` — s16
- `0x04` — u32
- `0x05` — s32
- `0x06` — u64
- `0x07` — s64
- `0x08` — f32
- `0x09` — f64
- `0x0A` — string (stored as a u32 offset into the string pool)
- `0x0B` — data (stored as a u32 offset + u32 size pointing into the data blob)

#### 3.4.2 Row data

For each row (RowCount of them):

```text
for each column whose storage flag is Per-row:
    read a value of the column's type, big-endian
```

Rows are tightly packed; total row size equals the sum of per-row column sizes
and matches the `RowSize` header field.

Multi-byte integers and floats inside row data are big-endian. Strings are u32
offsets into the string pool; data blobs are (u32 offset, u32 size) pairs into
the data blob.

#### 3.4.3 Reading a single cell

```text
function get(row_index, column_name):
    col = schema[column_name]
    storage = col.flags & 0xF0          # storage selector, per §3.4.1
    if storage == 0x00:                 # None — no value on disk
        return null
    if storage == 0x10:                 # Zero — value is an implicit 0 / empty
        return 0
    if storage == 0x30 or storage == 0x70:   # Constant / Constant2 — inline value
        return col.constant
    # storage == 0x50 (PerRow) — value lives in the row-data section
    seek(rows_offset + row_index * row_size + col.row_offset)
    raw = read typed value
    if col.type == STRING:
        return read_cstring(strings_offset + raw)
    if col.type == DATA:
        return read_bytes(data_offset + raw.offset, raw.size)
    return raw
```

### 3.5 CPK header packet (the first @UTF table)

The CPK header packet's @UTF table has exactly **one row**. Common columns
include (names are the literal string-pool values):

| Column         | Type   | Notes |
|----------------|--------|-------|
| `UpdateDateTime` | u64  | timestamp |
| `FileSize`     | u64    | total file size |
| `ContentOffset` | u64   | absolute file offset of the content blob |
| `ContentSize`  | u64    | size of the content blob |
| `TocOffset`    | u64    | absolute file offset of the TOC packet (`0` if absent) |
| `TocSize`      | u64    | size of the TOC packet @UTF payload |
| `EtocOffset`   | u64    | absolute file offset of ETOC packet (`0` if absent) |
| `EtocSize`     | u64    | … |
| `ItocOffset`   | u64    | absolute file offset of ITOC packet (`0` if absent) |
| `ItocSize`     | u64    | … |
| `GtocOffset`   | u64    | absolute file offset of GTOC packet (`0` if absent) |
| `GtocSize`     | u64    | … |
| `Files`        | u32    | total number of entries across the TOC |
| `Groups`       | u32    | number of file groups |
| `Attrs`        | u32    | attribute flag mask |
| `Align`        | u16    | alignment of content blob (typically `0x800` = 2048) |
| `Sorted`       | u16    | `1` if file list is sorted |
| `Version`      | u16    | format version |
| `Revision`     | u16    | format revision |
| `Tvers`        | string | tool version that wrote the file |

Not every column is required for reading; only the offset/size pointers and
`Align` matter.

### 3.6 TOC packet (the file index)

The TOC packet's @UTF table has one row per file in the archive. Common
columns:

| Column        | Type   | Notes |
|---------------|--------|-------|
| `DirName`     | string | directory path; `""` for root |
| `FileName`    | string | base file name |
| `FileSize`    | u32    | size as stored in the CPK (compressed size if CRILAYLA-compressed) |
| `ExtractSize` | u32    | size after decompression (equal to `FileSize` if uncompressed) |
| `FileOffset` | u64 | offset of the file's bytes relative to either the start of the file or `ContentOffset`, depending on the header (see §3.7) |
| `ID`          | u32    | file ID — used by some games for asset lookup |
| `UserString`  | string | optional user-defined tag |

### 3.7 Resolving file offsets

`FileOffset` in a TOC row is **relative**, and two conventions exist in the wild:

- `absolute = TocOffset + FileOffset` — canonical CRIWARE behavior; what DR V3
  and every modern CPK uses.
- `absolute = ContentOffset + FileOffset` — alternate base seen on some archives.

Rather than guess, **sniff both bases and pick the one that resolves cleanly**. A
candidate base is valid only if *every* row's span `[abs, abs + FileSize)` lies
inside `[ContentOffset, file_length]` — file bodies always live in the content
blob, never inside the packet headers. As a sharper tie-breaker, the first row's
`abs` should equal `ContentOffset` exactly. Among bases that satisfy this, prefer
`TocOffset`. (ITOC rows, when used, are always `ContentOffset`-relative.)

### 3.8 Compressed file entries (CRILAYLA)

A TOC row whose `FileSize != ExtractSize` indicates the file data is
CRILAYLA-compressed. The compressed data begins with the magic:

```text
Offset  Size  Field
0x00    8     "CRILAYLA"  (ASCII)
0x08    4     UncompressedSize  (u32 LE)
0x0C    4     CompressedSize    (u32 LE)
0x10    …     CompressedBitstream
…       0x100 RawHeaderTrailer  (last 256 bytes of the original uncompressed data, stored uncompressed)
```

The compressed bitstream is read **backwards from its end** (toward the
beginning of the file) — the algorithm reverses both byte and bit order during
both encoding and decoding. The output buffer is also filled backwards. After
the bitstream completes, the final 256 bytes of the original data (kept raw at
the tail of the file) are placed at the start of the decompressed output.

Bitstream format (decoded back-to-front):

- 1 bit flag.
  - `0` — emit the next 8 bits as a literal byte.
  - `1` — backreference:
    - 13 bits: offset within already-decompressed output (relative to current
      write position).
    - Variable-length length code (decoded with successive 2-, 3-, 5-, then
      8-bit chunks; each non-maximum chunk terminates the length). Base lengths
      add to the chunk values per the standard CRILAYLA scheme.

A full algorithm description is available in the public references in §3.1; a
clean-room implementation should validate against a CPK that contains
CRILAYLA-compressed entries.

> **Note for Danganronpa V3 specifically.** The CPK files shipped by DR V3 do
> not typically apply CRILAYLA to individual TOC entries — compression is
> handled one level deeper, inside SPC archives (§4.5). A reader for DR V3's
> CPKs can ignore CRILAYLA in practice, but a general-purpose CPK reader must
> implement it.

### 3.9 Encryption

Some CPK files are wrapped in a CRIWARE-supplied XOR obfuscation layer at the
packet level. **Danganronpa V3's CPKs are not encrypted.** No XOR step is
required. If a tool needs to detect encryption, the symptom is the @UTF magic
not appearing 16 bytes into a packet.

### 3.10 Read flow (file enumeration + extraction)

```text
open file
read CPK packet wrapper at offset 0:
    assert magic == "CPK "
    read header @UTF table
    header_row = utf.get_row(0)

content_offset = header_row["ContentOffset"]
toc_offset     = header_row["TocOffset"]
align          = header_row["Align"]   # usually 0x800

seek(toc_offset)
read TOC packet wrapper:
    assert magic == "TOC "
    read TOC @UTF table

for each row in toc_utf:
    dir_name     = row["DirName"]
    file_name    = row["FileName"]
    file_size    = row["FileSize"]
    extract_size = row["ExtractSize"]
    rel_offset   = row["FileOffset"]

    # Resolve absolute offset using the base chosen per §3.7
    abs_offset = offset_base + rel_offset

    seek(abs_offset)
    bytes = read_bytes(file_size)

    if file_size != extract_size:
        bytes = crilayla_decompress(bytes)   # §3.8

    yield (dir_name, file_name, bytes)
```

### 3.11 Write flow

Writing a CPK from scratch requires:

1. Decide the set of files to include and their order.
2. Compute the layout (see §3.2 for the canonical order):
   - `TocOffset` = size of the header packet, rounded up to **`Align`** (not 16).
   - `ContentOffset` = end of the last pre-content packet, rounded up to `Align`.
3. Construct the TOC `@UTF` table: one row per file, with columns from §3.6.
   File offsets are relative to `TocOffset` (per §3.7). Sizes are post-CRILAYLA
   if compression is applied.
4. Construct the header `@UTF` table: one row with the global metadata,
   including the final `TocOffset`, `TocSize`, `ContentOffset`, `ContentSize`,
   `EtocOffset`, `EtocSize`, `Files`, etc.
5. Emit in this exact order:
   - Header packet (wrapper + `@UTF`), pad up to **`Align`** (puts `TocOffset`
     on an Align-aligned sector boundary).
   - TOC packet (wrapper + `@UTF`), pad to 16 bytes.
   - ITOC packet (verbatim), if present, pad to 16 bytes.
   - GTOC packet (verbatim), if present, pad to 16 bytes.
   - Padding to `Align` (puts `ContentOffset` on an Align sector).
   - File content blob — each file body padded to `Align` if `Sorted == 1`.
   - **ETOC packet at file END (no trailing pad)**, if present. After this,
     `EtocOffset + EtocSize == FileSize` must hold.

Padding bytes are `0x00`. (CRI's reference encoder sometimes leaves
uninitialized memory in padding regions; the loader ignores anything past a
file body's declared size, so the bytes don't matter functionally.)

When emitting a @UTF table:

- The string pool is built by deduplicating every string referenced by the
  schema and rows; the empty string `""` should be the first entry at offset 0.
- The data blob is built by concatenating every (offset, size) blob referenced
  by data columns.
- Row data is written in column-declaration order; only **Per-row** columns
  occupy space.

`drv3-cpk` contains a full in-tree CPK writer (`Cpk::to_bytes`, driven by
`drv3-cli cpk pack`); its repack is byte-for-byte faithful to the shipped DR V3
CPKs for every load-bearing region. The references in §3.1 are additional
clean-room implementations.

### 3.12 Reference implementations

- **CriFsV2Lib** (https://github.com/Sewer56/CriFsV2Lib) — tracks the CPK
  read path closely. Recommended starting point for a clean-room reader.
- **CriPakTools** (https://github.com/esperknight/CriPakTools) — read +
  write. Longest-standing reference for the CPK writer.

---

## 4. SPC — inner archive container

A custom archive format used by Spike Chunsoft for Danganronpa games. Bundles
together related game-data files (typically a chapter's STX/DAT/WRD) into a
single asset that the CPK indexes by name.

### 4.1 Magic and variants

The first 4 bytes determine the variant:

- `CPS.` (0x43 0x50 0x53 0x2E) — **standard SPC**. The format documented below.
- `$CMP` (0x24 0x43 0x4D 0x50) — **SRD-compressed wrapper**. The actual SPC
  bytes are wrapped in a separate compression layer used by the console
  versions of the game. This document treats `$CMP` as opaque; readers should
  detect it and skip the file. See [§12](#12-out-of-scope).

Any other magic indicates the file is not an SPC.

### 4.2 Header (0x40 bytes)

All little-endian.

| Offset | Size | Field            | Notes |
|--------|------|------------------|-------|
| 0x00   | 4    | Magic            | `CPS.` |
| 0x04   | 0x24 | Unknown1         | 36 opaque bytes — preserve verbatim on round-trip |
| 0x28   | 4    | FileCount        | u32 — number of subfiles |
| 0x2C   | 4    | Unknown2         | u32 — preserve verbatim |
| 0x30   | 0x10 | Padding          | 16 bytes of `0x00` |
| 0x40   | 4    | TableMagic       | `Root` (ASCII) — start of the subfile table |
| 0x44   | 0x0C | Padding          | 12 bytes of `0x00` |
| 0x50   | …    | SubfileEntries   | `FileCount` consecutive entries, see §4.3 |

The `Root` literal at offset `0x40` marks the start of the subfile table. A
reader should validate this magic before iterating subfiles; a writer must
emit it verbatim.

### 4.3 Subfile entry layout

Each subfile entry is variable-length but laid out predictably:

| Offset (relative) | Size | Field            | Notes |
|-------------------|------|------------------|-------|
| 0x00              | 2    | CompressionFlag  | s16 LE — `1` = stored uncompressed, `2` = compressed (see §4.5) |
| 0x02              | 2    | UnknownFlag      | s16 LE — preserve verbatim |
| 0x04              | 4    | CurrentSize      | s32 LE — on-disk size (compressed size if compressed) |
| 0x08              | 4    | OriginalSize     | s32 LE — size after decompression (equal to `CurrentSize` if stored) |
| 0x0C              | 4    | NameLength       | s32 LE — length of name in bytes (Shift-JIS, excludes null terminator) |
| 0x10              | 0x10 | Padding          | 16 bytes of `0x00` |
| 0x20              | N    | Name             | `NameLength` bytes, Shift-JIS encoded |
| 0x20+N            | 1    | Null terminator  | `0x00` |
| 0x20+N+1          | P_n  | Name padding     | `P_n` zero bytes — see formula below |
| 0x20+N+1+P_n      | C    | Data             | `CurrentSize` bytes |
| …                 | P_d  | Data padding     | `P_d` zero bytes — see formula below |

Name padding:

```text
P_n = (0x10 - (NameLength + 1) % 0x10) % 0x10
```

Data padding:

```text
P_d = (0x10 - CurrentSize % 0x10) % 0x10
```

Both paddings align the next subfile entry to a 16-byte boundary.

### 4.4 Read flow

```text
open file
assert read_bytes(4) == "CPS."

unknown1   = read_bytes(0x24)
file_count = read_s32_le()
unknown2   = read_s32_le()
skip(0x10)

assert read_bytes(4) == "Root"
skip(0x0C)

for i in 0 .. file_count - 1:
    compression_flag = read_s16_le()
    unknown_flag     = read_s16_le()
    current_size     = read_s32_le()
    original_size    = read_s32_le()
    name_length      = read_s32_le()
    skip(0x10)

    name_padding = (0x10 - (name_length + 1) % 0x10) % 0x10
    name_bytes   = read_bytes(name_length)
    skip(1)                 # null terminator
    skip(name_padding)
    name = decode_shift_jis(name_bytes)

    data_padding = (0x10 - current_size % 0x10) % 0x10
    data = read_bytes(current_size)
    skip(data_padding)

    if compression_flag == 2:
        data = spc_decompress(data, original_size)   # §4.5

    yield (name, data)
```

### 4.5 SPC compression algorithm

SPC uses a custom byte-oriented LZSS-style sliding-window codec — **not**
standard Deflate, and unrelated to CRILAYLA in §3.8.

Constants:

| Name                    | Value | Notes |
|-------------------------|-------|-------|
| `SPC_WINDOW_MAX_SIZE`   | 1024  | Sliding-window size in bytes |
| `SPC_SEQUENCE_MAX_SIZE` | 65    | Maximum backreference length |
| `SPC_SEQUENCE_MIN_SIZE` | 2     | Minimum backreference length (a backreference encodes its length as `count − 2`) |

Each flag byte governs the next **8 entries** — one bit per entry — so a block is
one flag byte followed by 8 entries of 1–2 bytes each.

**Stream layout.** The compressed bytes are a sequence of *blocks*. Each block
is:

```text
flag_byte (1 byte)
8 entries (1–2 bytes each)
```

The flag byte's 8 bits, **after bit-reversal**, control the 8 entries from
LSB to MSB:

- bit = `1` — entry is a single raw byte.
- bit = `0` — entry is a 2-byte backreference (little-endian u16):
  - Low 10 bits — offset within the sliding window (`offset`, range `0..1023`).
  - High 6 bits — length minus 2 (`count - 2`, range `0..63`, so emit lengths
    of `2..65`).

The final block may have fewer than 8 entries if the stream ends mid-block; the
flag bits beyond the valid count are ignored.

**Reverse-bit helper.** The flag byte is stored bit-reversed. Any standard
8-bit reversal works (the "reverse byte with 64-bit multiply" trick from
Sean Eron Anderson's *Bit Twiddling Hacks* is a common branch-free choice).

#### 4.5.1 Decompression

```text
function spc_decompress(compressed_bytes, original_size):
    out = empty buffer
    pos = 0
    flag = 1     # sentinel: triggers fetch on first iteration

    while pos < len(compressed_bytes):
        if flag == 1:
            flag = 0x100 | reverse_bits_8(compressed_bytes[pos])
            pos += 1
            if pos >= len(compressed_bytes): break

        if (flag & 1) == 1:
            # raw byte
            out.append(compressed_bytes[pos])
            pos += 1
        else:
            # backreference
            b = read_u16_le(compressed_bytes[pos..pos+2])
            pos += 2
            count  = (b >> 10) + 2
            offset = b & 0x3FF   # SPC_WINDOW_MAX_SIZE - 1

            for j in 0 .. count - 1:
                src_index = len(out) - SPC_WINDOW_MAX_SIZE + offset
                out.append(out[src_index])

        flag >>= 1

    assert len(out) == original_size
    return out
```

Notes:

- `reverse_bits_8` reverses a single byte (`0b10110100` → `0b00101101`).
- The "sentinel" trick — initializing `flag = 1` and OR'ing the fetched byte
  with `0x100` — uses the `0x100` bit to detect when all 8 entries of a block
  have been consumed (and time to fetch a new flag byte).
- Backreference source index can land in the not-yet-written tail of the
  output buffer when `offset > len(out) - SPC_WINDOW_MAX_SIZE + something`;
  this is intentional and produces repeat-of-prefix output. (The standard
  LZSS trick.)

#### 4.5.2 Compression

The encoder must be byte-for-byte compatible with the decoder above. Strategy:

```text
function spc_compress(raw_bytes):
    out = empty buffer
    pos = 0
    flag = 0
    cur_bit = 0
    block_buf = empty buffer

    while pos < len(raw_bytes):
        if cur_bit == 8 or pos >= len(raw_bytes):
            out.append(reverse_bits_8(flag))
            out.extend(block_buf)
            flag = 0
            cur_bit = 0
            block_buf = empty buffer
            if pos >= len(raw_bytes): break

        # Search the previous up-to-1024 bytes for the longest sequence
        # starting at `pos` that already appears in the window.
        window_start = max(0, pos - SPC_WINDOW_MAX_SIZE)
        best_len, best_offset = find_longest_match(
            raw_bytes,
            window_start,
            pos,
            max_len = min(SPC_SEQUENCE_MAX_SIZE, len(raw_bytes) - pos))

        if best_len >= 2:
            # backreference
            window_pos = (SPC_WINDOW_MAX_SIZE - (pos - window_start)) + (best_offset - window_start)
            encoded = window_pos | ((best_len - 2) << 10)
            block_buf.extend(le_u16(encoded))
            pos += best_len
        else:
            # raw byte
            flag |= (1 << cur_bit)
            block_buf.append(raw_bytes[pos])
            pos += 1

        cur_bit += 1

    # Final block already flushed by the loop above when pos == len(raw_bytes).
    return out
```

The `find_longest_match` implementation determines compression ratio versus
encoder speed. A naive `LastIndexOf` of the prefix sequence works (slow but
maximal); hashed lookup tables can match standard LZSS speed.

### 4.6 Write flow

```text
open file for writing
write "CPS."
write unknown1 (0x24 bytes, preserved from source)
write file_count (s32 LE)
write unknown2 (s32 LE, preserved from source)
write 0x10 zero bytes
write "Root"
write 0x0C zero bytes

for each subfile:
    if subfile is marked stored:
        compression_flag = 1
        current_size = original_size = len(data)
        data_out = data
    else:
        data_out = spc_compress(data)
        compression_flag = 2
        current_size = len(data_out)
        original_size = len(data)

    write compression_flag (s16 LE)
    write unknown_flag (s16 LE, preserved from source)
    write current_size (s32 LE)
    write original_size (s32 LE)
    write name_length (s32 LE)
    write 0x10 zero bytes

    write_shift_jis(name)
    write 0x00 (null terminator)
    write name_padding zero bytes

    write data_out
    write data_padding zero bytes
```

Both `name_padding` and `data_padding` are computed per the formulas in §4.3.

### 4.7 Reference implementations

- **Harmony-Tools** (https://github.com/redssu/Harmony-Tools) — the SPC
  driver covers header parsing, the subfile-entry layout, and the
  sliding-window compression / decompression codec described in §4.5.
  A peek-mode reader that skips data payloads (useful for cheaply
  enumerating subfile names) is a straightforward derivative of that
  driver.

---

## 5. STX — string table

The primary translation target. Contains one or more tables, each mapping a
numeric string ID to a UTF-16 LE string.

### 5.1 Header

| Offset | Size | Field        | Notes |
|--------|------|--------------|-------|
| 0x00   | 4    | Magic        | `STXT` (ASCII) |
| 0x04   | 4    | Language     | `JPLL` (ASCII) — language tag; only `JPLL` observed |
| 0x08   | 4    | TableCount   | u32 LE — almost always `1`, but the format permits N |
| 0x0C   | 4    | TableOffset  | u32 LE — absolute file offset of the first table's ID/offset array |

### 5.2 Table info

Starting at offset `0x10`, one 16-byte info block per table:

| Offset (relative) | Size | Field       | Notes |
|-------------------|------|-------------|-------|
| 0x00              | 4    | Unknown     | u32 LE — preserve verbatim on round-trip |
| 0x04              | 4    | StringCount | u32 LE — number of (id, offset) entries in this table's array |
| 0x08              | 8    | Padding     | 8 zero bytes (alignment to 16 bytes) |

Total table-info section size: `0x10 * TableCount` bytes. For the typical case
(`TableCount == 1`), the section spans `0x10..0x20`.

### 5.3 String index array

At absolute offset `TableOffset`, the per-table ID/offset arrays appear
contiguously:

```text
for each table t in 0..TableCount-1:
    for each entry e in 0..t.StringCount-1:
        StringId     (u32 LE)
        StringOffset (u32 LE)   # absolute file offset of the UTF-16 string
```

So if `TableCount == 1` and `StringCount == 200`, the array occupies
`200 * 8 = 1600` bytes immediately after the table info section.

`StringOffset` is **absolute** — measured from the start of the file, not from
any other anchor.

### 5.4 String data

At each `StringOffset`:

- UTF-16 LE code units.
- Terminated by a `0x00 0x00` (2-byte) null.

The byte length of a string is therefore `2 * (number_of_code_units) + 2`
(including the terminator).

### 5.5 Deduplication rule

**Critical for round-trip correctness.** When two `StringId`s have identical
string content, the original game files store the string data only once and
point both entries at the same `StringOffset`. Any writer that does not
deduplicate will produce files that differ byte-for-byte from the original (and
may be larger, which can break tooling expecting a specific size).

A reader sees deduplication as multiple IDs pointing at the same offset; this
is legal and must be tolerated.

### 5.6 Read flow

```text
open file
assert read_bytes(4) == "STXT"
assert read_bytes(4) == "JPLL"
table_count  = read_u32_le()
table_offset = read_u32_le()

# Table info section
tables = []
seek(0x10)
for t in 0 .. table_count - 1:
    unknown      = read_u32_le()
    string_count = read_u32_le()
    skip(8)
    tables.append({ unknown, string_count, entries: {} })

# Index array
seek(table_offset)
for t in tables:
    for s in 0 .. t.string_count - 1:
        string_id     = read_u32_le()
        string_offset = read_u32_le()

        if string_id already in t.entries:
            # duplicate ID (rare; legal if offset matches)
            assert t.entries[string_id].offset == string_offset
            continue

        return_pos = current_position()
        seek(string_offset)
        text = read_utf16_le_null_terminated()
        seek(return_pos)

        t.entries[string_id] = { offset: string_offset, text: text }
```

### 5.7 Write flow

A two-pass approach is required because the index array must be written before
the string data (so offsets can be patched in), but offsets are only known
after the strings are laid out.

```text
open file for writing
write "STXTJPLL"
write table_count (u32 LE)
write 0_u32                          # placeholder for table_offset; patched later

# Table info section
for each table t:
    write t.unknown (u32 LE)
    write t.string_count (u32 LE)
    write 8 zero bytes

# Patch table_offset to point here
table_offset = current_position()
seek(0x0C)
write table_offset (u32 LE)
seek(table_offset)

# Reserve space for the index array
total_entries = sum(t.string_count for t in tables)
write (8 * total_entries) zero bytes

# Write strings, patch the index array as we go
info_pair_pos = table_offset
for each table t:
    written = {}                      # dedup map: text -> offset
    for each (id, element) in t.entries:
        if element.text in written:
            string_pos = written[element.text]
            # do not re-write the bytes
        else:
            string_pos = current_position()
            write_utf16_le(element.text)
            write_u16(0)              # 2-byte null terminator
            written[element.text] = string_pos

        latest = current_position()
        seek(info_pair_pos)
        write element.id (u32 LE)
        write string_pos (u32 LE)
        seek(latest)
        info_pair_pos += 8
```

### 5.8 Embedded markup

STX string data is not raw plain text. Strings frequently contain inline tags
that must be preserved verbatim by any patcher. Examples:

- `<CLT=cltSYSTEM>` — color/style tag (system text color)
- `<CLT=cltIMPORTANT>` — color/style tag (important text color)
- `<CLT>` — generic close-color
- Other angle-bracket-and-equals tags follow the same shape

Tags are stored as literal UTF-16 sequences (the `<`, `>`, and `=` characters
are encoded as ordinary BMP code units `0x003C`, `0x003E`, `0x003D`). A writer
that re-encodes strings should ensure the resulting UTF-16 reproduces the same
byte sequence — there is no escape mechanism.

### 5.9 Reference implementations

- **Harmony-Tools** (https://github.com/redssu/Harmony-Tools) — full STX
  reader and writer, including the deduplication-on-write logic described
  in §5.5 and §5.7. The dedup loop is the load-bearing detail; a writer
  that omits it will not round-trip.

---

## 6. DAT — typed binary tables

A row-and-column binary table. Many files inside the CPK that carry the `.dat`
extension use this format for UI labels, item descriptions, character data,
etc. Strings are stored in two deduplicated pools (UTF-8 and UTF-16) and
referenced by index from row data.

### 6.1 Header (12 bytes)

| Offset | Size | Field       | Notes |
|--------|------|-------------|-------|
| 0x00   | 4    | RowCount    | u32 LE |
| 0x04   | 4    | BytesPerRow | u32 LE — sum of all per-column sizes × counts |
| 0x08   | 4    | ColumnCount | u32 LE |

**Sanity check.** Many files in the CPK use `.dat` as a generic extension and
do not follow this format. Before attempting to parse:

- `RowCount > 0` and `RowCount < 1_000_000`
- `BytesPerRow > 0` and `BytesPerRow < 100_000`
- `ColumnCount > 0` and `ColumnCount <= 256`

Reject anything outside those bounds.

### 6.2 Column schema

Immediately after the header, `ColumnCount` column descriptors appear back to
back. Each descriptor:

| Field      | Type            | Notes |
|------------|-----------------|-------|
| Name       | UTF-8 cstring   | null-terminated; column name |
| TypeTag    | ASCII cstring   | null-terminated; one of the type tags below |
| Count      | u16 LE          | number of values per row in this column (always ≥1; > 1 means a fixed-size array) |

### 6.3 Column types

| TypeTag | Width per value | Encoding in row data | Resolved string lookup |
|---------|-----------------|----------------------|------------------------|
| `u8`    | 1 byte          | u8                   | — |
| `u16`   | 2 bytes         | u16 LE               | — |
| `u32`   | 4 bytes         | u32 LE               | — |
| `u64`   | 8 bytes         | u64 LE               | — |
| `s8`    | 1 byte          | s8                   | — |
| `s16`   | 2 bytes         | s16 LE               | — |
| `s32`   | 4 bytes         | s32 LE               | — |
| `s64`   | 8 bytes         | s64 LE               | — |
| `f32`   | 4 bytes         | IEEE-754 single, LE  | — |
| `f64`   | 8 bytes         | IEEE-754 double, LE  | — |
| `ascii` | 2 bytes         | u16 LE — index into the **UTF-8 string pool** | yes |
| `label` | 2 bytes         | u16 LE — index into the **UTF-8 string pool** | yes |
| `refer` | 2 bytes         | u16 LE — index into the **UTF-8 string pool** | yes |
| `utf16` | 2 bytes         | u16 LE — index into the **UTF-16 string pool** | yes |

`ascii`, `label`, and `refer` differ in **semantics**, not encoding — all three
index the UTF-8 pool. `utf16` indexes the separate UTF-16 pool.

> The `ascii` name is historical: the pool is UTF-8, but in practice most
> values fit in ASCII. The DAT format itself does not enforce ASCII-ness.

### 6.4 Row data section

After the column schema, the file is padded with `0x00` to the next 16-byte
boundary, then row data begins.

Row data is `RowCount * BytesPerRow` bytes — tightly packed, no per-row
padding. For each row, for each column, write `Count` values back-to-back in
the column's encoding.

### 6.5 String pool

Immediately after the row data:

| Offset (relative) | Size | Field       | Notes |
|-------------------|------|-------------|-------|
| 0x00              | 2    | Utf8Count   | u16 LE — number of strings in the UTF-8 pool |
| 0x02              | 2    | Utf16Count  | u16 LE — number of strings in the UTF-16 pool |
| 0x04              | …    | Utf8Pool    | `Utf8Count` UTF-8 null-terminated strings, packed |
| …                 | P    | Padding     | pad to next 2-byte boundary |
| …                 | …    | Utf16Pool   | `Utf16Count` UTF-16 LE null-terminated strings (2-byte terminator), packed |

The first entry of each pool (index 0) is conventionally the empty string
`""`.

### 6.6 Multi-value cells

When `Count > 1` for a string column, the row stores `Count` separate u16
indices, each resolving to its own string. Implementations that present DAT
cells as application-level strings concatenate them with the separator `|`
(U+007C) — but **that separator is not stored on disk**, it is a presentation
convention. Disk storage is always `Count` separate u16 indices.

### 6.7 Read flow

```text
open file
row_count    = read_u32_le()
bytes_per_row = read_u32_le()
column_count = read_u32_le()

columns = []
for c in 0 .. column_count - 1:
    name = read_utf8_cstring()
    type = read_ascii_cstring()
    count = read_u16_le()
    columns.append({ name, type, count })

align_to(16)

# String pools live after the row data — peek there first
row_data_pos = current_position()
seek(row_data_pos + bytes_per_row * row_count)

utf8_count  = read_u16_le()
utf16_count = read_u16_le()
utf8_pool  = [read_utf8_cstring() for _ in range(utf8_count)]
align_to(2)
utf16_pool = [read_utf16_le_cstring() for _ in range(utf16_count)]

# Now read row data
seek(row_data_pos)
rows = []
for r in 0 .. row_count - 1:
    row = []
    for col in columns:
        values = []
        for i in 0 .. col.count - 1:
            raw = read_value_for_type(col.type)
            if col.type in ("ascii", "label", "refer"):
                values.append(utf8_pool[raw])
            elif col.type == "utf16":
                values.append(utf16_pool[raw])
            else:
                values.append(raw)
        row.append(values)
    rows.append(row)
```

### 6.8 Write flow

```text
open file for writing
write row_count (u32 LE)

# Compute bytes_per_row from the schema
bytes_per_row = sum(size_of(col.type) * col.count for col in columns)
write bytes_per_row (u32 LE)
write column_count (u32 LE)

# Column schema
for col in columns:
    write_utf8_cstring(col.name)
    write_ascii_cstring(col.type)
    write_u16_le(col.count)
write padding to next 16-byte boundary

# Build string pools as rows are written (deduplicated)
utf8_pool  = []
utf16_pool = []

for r in rows:
    for (col, values) in zip(columns, r):
        for v in values:
            if col.type in ("ascii", "label", "refer"):
                if v not in utf8_pool:
                    utf8_pool.append(v)
                write_u16_le(utf8_pool.index(v))
            elif col.type == "utf16":
                if v not in utf16_pool:
                    utf16_pool.append(v)
                write_u16_le(utf16_pool.index(v))
            else:
                write_value_for_type(col.type, v)

# String pools
write_u16_le(len(utf8_pool))
write_u16_le(len(utf16_pool))
for s in utf8_pool:
    write_utf8_cstring(s)
write padding to next 2-byte boundary
for s in utf16_pool:
    write_utf16_le_cstring(s)
```

### 6.9 Reference implementations

- **Harmony-Tools** (https://github.com/redssu/Harmony-Tools) — full DAT
  reader and writer covering all column types and the two string-pool
  layout.

---

## 7. WRD — script files

A byte-code script that drives Danganronpa's dialogue scenes. Pairs with a
same-named STX file: opcodes such as `LOC` reference STX string IDs, and
opcodes such as `CHN` provide the speaker for each line. WRD files are not
modified during translation but must be readable to recover speaker / choice
metadata for an STX line.

### 7.1 Header (0x20 bytes)

All little-endian.

| Offset | Size | Field             | Notes |
|--------|------|-------------------|-------|
| 0x00   | 2    | StringCount       | u16 LE — number of dialogue strings (internal *or* external, see §7.5) |
| 0x02   | 2    | LabelCount        | u16 LE |
| 0x04   | 2    | ParameterCount    | u16 LE |
| 0x06   | 2    | LocalBranchCount  | u16 LE |
| 0x08   | 4    | Unknown1          | u32 — preserve verbatim |
| 0x0C   | 4    | LocalBranchDataPtr | u32 LE — absolute file offset of the local-branch data block |
| 0x10   | 4    | LabelOffsetsPtr   | u32 LE — absolute file offset of the per-label byte-code offset array |
| 0x14   | 4    | LabelNamesPtr     | u32 LE — absolute file offset of the label-name list |
| 0x18   | 4    | ParametersPtr     | u32 LE — absolute file offset of the parameter list |
| 0x1C   | 4    | StringsPtr        | u32 LE — absolute file offset of the internal-strings list, or `0` if external |

The byte-code stream starts at `0x20` and runs up to (but not including)
`LocalBranchDataPtr`.

### 7.2 Section layout

```text
+------------------+ 0x00
| header (0x20)    |
+------------------+ 0x20
| byte-code stream | up to LocalBranchDataPtr
+------------------+
| local branches   | LocalBranchCount * 4 bytes (u16 ID, u16 offset, LE)
+------------------+
| label offsets    | LabelCount * u16 LE — byte-code offset of each LAB
+------------------+
| label names      | LabelCount entries; each: u8 length, ASCII bytes, 0x00 |
+------------------+
| parameters       | ParameterCount entries; same encoding as label names    |
+------------------+
| internal strings | (optional — see §7.5) StringCount UTF-16 LE null-terminated
+------------------+
```

### 7.3 Byte-code stream

The byte-code stream consists of commands. Each command:

| Field   | Size | Notes |
|---------|------|-------|
| Marker  | 1 byte | always `0x70` |
| Opcode  | 1 byte | index into the opcode table (§7.4) |
| Args    | 2 bytes × N | each argument is a **big-endian** u16 — *unlike the rest of the file* |

The argument count `N` per opcode is fixed by an argument-type table (§7.4) but
some opcodes use a variable trailing list. Readers detect the end of an
argument list by reading until the next `0x70` byte appears or the byte-code
stream's end is reached (the next byte must be unread/rewound when this
happens).

### 7.4 Opcode table

There are 76 opcodes, indexed by the second byte of each command (`0..75`).
The complete list with one-line annotations is enumerated in the
Harmony-Tools `WrdCommandHelper` table (see §7.9 for the reference). The
opcodes whose meaning matters for translation work are:

| Opcode index | Mnemonic | Purpose | Argument types |
|--------------|----------|---------|----------------|
| 0x0A         | `CHK`    | Branch / choice metadata | one parameter |
| 0x14         | `LAB`    | Mark a label | one label index |
| 0x1D         | `CHN`    | Set the currently-speaking character | one parameter |
| 0x22         | `CHR`    | Character parameters (sometimes synthesised into a CHN by readers) | two parameters |
| 0x4B         | `LOC`    | Display a string from the paired STX | one dialogue-string index (= STX `StringId`) |

#### 7.4.1 Argument types

For each opcode, an N-element list specifies the type of each of its first N
arguments. Arguments past N use the last type (effectively making the list
repeat the final element for trailing args). The four argument types are:

| Code | Meaning                                                              |
|------|----------------------------------------------------------------------|
| 0    | Plaintext parameter — u16 index into the parameter list (§7.6.2) |
| 1    | Raw number — emit the u16 verbatim |
| 2    | Dialogue string — u16 index referring to an STX `StringId` (the `LOC` opcode uses this) |
| 3    | Label — u16 index into the label-names list (§7.6.1) |

The argument-type lists for all 76 opcodes are enumerated alongside the
opcode-name table in the same Harmony-Tools `WrdCommandHelper` reference.

### 7.5 Internal vs. external strings

The `StringsPtr` header field is `0` if the WRD's dialogue strings live
entirely in the paired STX. When non-zero, it points at an internal
UTF-16 LE null-terminated string list inside the WRD itself.

Logic for distinguishing the cases:

```text
if StringsPtr != 0:
    seek(StringsPtr)
    for i in 0..StringCount-1:
        strings.append(read_utf16_le_null_terminated())
    uses_external_strings = false
else:
    uses_external_strings = (StringCount > 0)
```

For Danganronpa V3 dialogue files, external strings (i.e. strings in the
paired STX) are the norm.

### 7.6 Auxiliary tables

#### 7.6.1 Label-names list (at `LabelNamesPtr`)

`LabelCount` entries. Each entry:

| Field   | Size           | Notes |
|---------|----------------|-------|
| Length  | 1 byte         | length of the name in bytes |
| Name    | `Length` bytes | ASCII (or Shift-JIS — practice has been ASCII-only in observed files) |
| Null    | 1 byte         | `0x00` terminator |

#### 7.6.2 Parameter list (at `ParametersPtr`)

`ParameterCount` entries, same encoding as label names.

#### 7.6.3 Label-offsets array (at `LabelOffsetsPtr`)

`LabelCount` entries; each entry is a u16 LE — byte-code offset (within the
byte-code stream, relative to its start at `0x20`) where the corresponding
`LAB` opcode was emitted.

#### 7.6.4 Local-branch data (at `LocalBranchDataPtr`)

`LocalBranchCount` entries; each entry is:

| Field   | Size | Notes |
|---------|------|-------|
| ID      | u16 LE | branch ID |
| Offset  | u16 LE | byte-code offset of the corresponding `LBN` |

### 7.7 Read flow

```text
open file
string_count, label_count, parameter_count, local_branch_count = read four u16 LE
unknown1 = read_u32_le()
local_branch_data_ptr, label_offsets_ptr, label_names_ptr, parameters_ptr, strings_ptr
    = read five u32 LE

seek(label_names_ptr)
label_names = [read_pascal_string() for _ in label_count]

seek(parameters_ptr)
parameters = [read_pascal_string() for _ in parameter_count]

internal_strings = []
if strings_ptr != 0:
    seek(strings_ptr)
    internal_strings = [read_utf16_le_null_terminated() for _ in string_count]

# Decode byte code
seek(0x20)
commands = []
while current_position() + 1 < local_branch_data_ptr:
    b = read_u8()
    if b != 0x70:
        continue
    opcode = read_u8()
    arg_types = ARG_TYPE_TABLE[opcode]
    args = []
    arg_num = 0
    while current_position() + 1 < local_branch_data_ptr:
        peek = read_u8()
        if peek == 0x70:
            seek(current_position() - 1)
            break
        b2 = read_u8()
        arg = (peek << 8) | b2          # big-endian u16
        t = arg_types[arg_num % len(arg_types)]
        match t:
            case 0: args.append(parameters[arg] if arg < len(parameters) else "")
            case 1: args.append(str(arg))
            case 2: args.append(str(arg))
            case 3: args.append(label_names[arg] if arg < len(label_names) else "")
        arg_num += 1
    commands.append({ opcode: OPCODE_NAMES[opcode], args: args })
```

`read_pascal_string()` reads `u8 length`, `length` bytes, then a `0x00` byte.

### 7.8 Write flow

WRD files are not modified by translation work; this document does not
include a writer specification. A working writer exists in the
Harmony-Tools project (see §7.9) for consultation if a writer is ever
required.

### 7.9 Reference implementations

- **Harmony-Tools** (https://github.com/redssu/Harmony-Tools) — full WRD
  reader and writer, plus the canonical 76-entry opcode-name table and
  per-opcode argument-type list referenced by §7.4.

---

## 8. SRD — block container

A typed-block container used for textures, font atlases, vertex buffers, and
related resource metadata. Spike Chunsoft format, shared across Danganronpa
games. Files that hold an SRD payload may use a variety of extensions: in
DR V3, font containers use **`.stx`** (despite the name collision with
dialogue STX — see §1).

### 8.1 Triple-file convention

A single SRD "package" may be split across up to three sidecar files:

| Extension | Purpose |
|-----------|---------|
| `.srd` (or `.stx` for fonts) | The block tree itself — sequence of typed blocks |
| `.srdi` | Bulk external data referenced by blocks (image / texture bytes). May be absent. |
| `.srdv` | Bulk external data referenced by blocks (vertex buffers and, in DR V3 fonts, texture pixel bytes). May be absent. |

A reader needs to open the `.srd` (or `.stx`) **plus whichever sidecars are
referenced** by blocks inside it. The `$RSI` block's `ResourceInfo` entries
(§8.5) carry a flag that determines which sidecar each external blob lives
in:

- Top bits `0x20000000` of the first ResourceInfo value → blob is in
  `.srdi`.
- Top bits `0x40000000` of the first ResourceInfo value → blob is in
  `.srdv`.
- The remaining 29 bits of that value are the absolute byte offset within
  the sidecar.

DR V3 font packages omit `.srdi` and store the texture atlas entirely inside
`.srdv`.

A writer must create the sidecar files lazily (only if blocks write to them)
and delete them if they end up empty — the game crashes if it encounters a
zero-byte `.srdi`.

### 8.2 File layout

An SRD file is a flat sequence of blocks. There is no file header — the
first 4 bytes are already a block's `BlockType` magic. Read blocks until
end-of-file.

Each block may have **child blocks** (subdata). The standard hierarchy for a
texture-bearing SRD is:

```text
$CFH                         (always first; carries no payload)
$TXR                         (texture metadata)
 ├─ $RSI                     (resource info — external data refs + ResourceData)
 └─ $CT0                     (terminator child)
$CT0                         (terminator at the top level)
```

For DR V3 font containers, the in-memory shape is exactly: `[$CFH, $TXR, $CT0]`
at the top level, with `$TXR.Children = [$RSI, $CT0]` and
`$RSI.ResourceData` carrying an SpFt FontBlock (§9).

### 8.3 Block wrapper

Every block begins with a 16-byte header:

| Offset | Size | Field         | Notes |
|--------|------|---------------|-------|
| 0x00   | 4    | BlockType     | ASCII; one of `$CFH`, `$TXR`, `$RSI`, `$TRE`, `$TXI`, `$VTX`, `$RSF`, `$CT0`, or any other unknown 4-byte magic |
| 0x04   | 4    | DataLength    | u32 **big-endian** — size in bytes of the block's data payload |
| 0x08   | 4    | SubdataLength | u32 **big-endian** — size in bytes of the child-blocks payload |
| 0x0C   | 4    | Unknown0C     | u32 **big-endian** — `1` for `$CFH`, `0` for every other block type. Preserve verbatim. |

The three length/unknown u32s are big-endian. **Block data payloads
themselves are little-endian** (see §8.6). This split-endianness is unusual;
implementers commonly get it wrong on the first pass.

After the 16-byte header:

```text
data        : DataLength bytes      (block-type-specific payload)
padding     : pad to next 0x10 boundary, zero bytes
subdata     : SubdataLength bytes   (a recursive sequence of child blocks)
padding     : pad to next 0x10 boundary, zero bytes
```

`subdata` itself is parsed exactly like the top-level stream: read child
blocks until you have consumed `SubdataLength` bytes.

### 8.4 Read flow

```text
function read_blocks(stream, end_position):
    blocks = []
    while stream.position < end_position:
        block_type      = read_bytes(4) as ASCII
        data_length     = read_u32_be()
        subdata_length  = read_u32_be()
        unknown_0c      = read_u32_be()

        data = read_bytes(data_length)
        align_to(0x10)

        subdata_bytes = read_bytes(subdata_length)
        children = read_blocks(memory_stream(subdata_bytes), subdata_length)
        align_to(0x10)

        block = build_block(block_type, data, children, unknown_0c)
        blocks.append(block)
    return blocks

function read_srd(srd_path, srdi_path, srdv_path):
    open srd_path
    return read_blocks(stream, file_length(srd_path))

# External data (for $RSI blocks) is loaded on demand from srdi_path / srdv_path
# based on the ResourceInfo flags (§8.5).
```

### 8.5 Block types

The seven named block types are listed below. Carry-no-data blocks (`$CFH`,
`$CT0`) emit a `DataLength` of 0 — they exist only as markers.

#### 8.5.1 `$CFH` — container file header

- `DataLength` = 0.
- `Unknown0C` = 1 (the only block type that uses this slot).
- Must be the first block in an SRD top-level stream.
- No payload.

#### 8.5.2 `$CT0` — Terminator

- `DataLength` = 0.
- `Unknown0C` = 0.
- No payload.
- Appears at the end of the top-level block list, and at the end of `$TXR`
  children.

#### 8.5.3 `$TXR` — texture metadata

Data payload (all little-endian):

| Offset | Size | Field         | Notes |
|--------|------|---------------|-------|
| 0x00   | 4    | Unknown10     | s32 LE — preserve verbatim |
| 0x04   | 2    | Swizzle       | u16 LE — `1` = unswizzled (the only value DR V3 fonts use) |
| 0x06   | 2    | DisplayWidth  | u16 LE — texture width in pixels |
| 0x08   | 2    | DisplayHeight | u16 LE — texture height in pixels |
| 0x0A   | 2    | Scanline      | u16 LE — row pitch hint |
| 0x0C   | 1    | Format        | u8 — `TextureFormat` enum (see §10) |
| 0x0D   | 1    | Unknown1D     | u8 — preserve verbatim (sometimes referred to as a "resources count") |
| 0x0E   | 1    | Palette       | u8 — `1` = palette-indexed; DR V3 fonts do not use this |
| 0x0F   | 1    | PaletteId     | u8 — palette resource id |

Total data size: 16 bytes (0x10).

The actual pixel data is **not** stored in the `$TXR` payload — it lives in
the child `$RSI` block's external data (§8.5.4) and is referenced from the
sidecar `.srdi` or `.srdv` file.

#### 8.5.4 `$RSI` — resource info

Data payload (mixed widths, little-endian throughout):

| Offset | Size | Field                       | Notes |
|--------|------|-----------------------------|-------|
| 0x00   | 1    | Unknown10                   | u8 — preserve verbatim |
| 0x01   | 1    | Unknown11                   | u8 — preserve verbatim |
| 0x02   | 1    | Unknown12                   | u8 — preserve verbatim |
| 0x03   | 1    | FallbackResourceInfoCount   | u8 — used when `ResourceInfoCount == 0` |
| 0x04   | 2    | ResourceInfoCount           | s16 LE — number of ResourceInfo entries that follow (typically `1` for textures) |
| 0x06   | 2    | FallbackResourceInfoSize    | s16 LE — bytes per ResourceInfo entry when `ResourceInfoSize == 0` |
| 0x08   | 2    | ResourceInfoSize            | s16 LE — bytes per ResourceInfo entry; typically `32` (i.e. 8 × u32) |
| 0x0A   | 2    | Unknown1A                   | s16 LE — preserve verbatim |
| 0x0C   | 4    | ResourceStringListOffset    | s32 LE — byte offset (relative to the start of this $RSI data payload) where the resource-string list begins |
| 0x10   | …    | ResourceInfoList            | `ResourceInfoCount` (or `FallbackResourceInfoCount`) entries, each `ResourceInfoSize / 4` u32 LE values — see below |
| …      | …    | ResourceData                | bytes from end-of-info-list up to `ResourceStringListOffset` — an inline payload (for fonts: an SpFt blob — see §9) |
| …      | …    | ResourceStringList          | a sequence of null-terminated Shift-JIS strings, packed up to the end of the block data |

##### ResourceInfo entry

Each ResourceInfo is a fixed-size array of u32 LE values. The first two
values are well-defined; the remaining values are opaque metadata that
must be preserved verbatim:

| Value index | Meaning |
|-------------|---------|
| 0 | High 3 bits = sidecar flag (`0x20000000` = `.srdi`, `0x40000000` = `.srdv`). Low 29 bits = byte offset within that sidecar. Mask with `0x1FFFFFFF` to extract offset, mask with `~0x1FFFFFFF` to extract flag. |
| 1 | Size in bytes of the external blob (read from the sidecar at the computed offset). |
| 2 … N−1 | Opaque — preserve verbatim. For DR V3 font textures, observed values include `0x00000080`, `0`, `0x00000E93`, `0x00000030`, `0x00000E4C`, `0x0000FFFF`. |

Total entry size = `ResourceInfoSize` bytes = (`ResourceInfoSize / 4`) × u32.

##### External data resolution

For each ResourceInfo in the list, after parsing the `$RSI` block payload,
open the appropriate sidecar (`.srdi` or `.srdv`), seek to the masked
offset, and read `Values[1]` bytes. The resulting blob is the actual
texture / vertex data.

When **writing** an `$RSI` block, the writer must (re-)append each
ExternalData blob to the sidecar file, record the new offset in
`Values[0]` with the same flag bits, and update `Values[1]` with the new
size. Sidecar entries are padded to a 16-byte boundary after each write.

#### 8.5.5 Other block types

These block types appear elsewhere in DR V3 SRD files but are **not**
required by font work. A complete reader should round-trip them verbatim
(block magic + data payload + child blocks); their full payload layout is
described by the Harmony-Tools SRD driver (see §8.8).

| Magic | Purpose |
|-------|---------|
| `$TRE` | Scene-graph tree node |
| `$TXI` | Texture instance |
| `$VTX` | Vertex buffer |
| `$RSF` | Resource folder |
| (any other 4-byte magic) | Unknown block — store the raw data and round-trip verbatim |

### 8.6 Write flow

```text
function write_blocks(stream, blocks, srdi_path, srdv_path):
    for block in blocks:
        write_bytes(block.block_type)            # 4 ASCII bytes

        data    = serialize_block_data(block, srdi_path, srdv_path)
        subdata = serialize_blocks_to_buffer(block.children, srdi_path, srdv_path)

        write_u32_be(len(data))
        write_u32_be(len(subdata))
        write_u32_be(1 if block.is_cfh else 0)

        write_bytes(data)
        pad_to(0x10)
        write_bytes(subdata)
        pad_to(0x10)
```

The `$RSI` writer is responsible for writing its `ExternalData` blobs into
the `.srdi` / `.srdv` sidecars and patching the ResourceInfo entries with
the new offsets (§8.5.4). Other block writers operate purely on the
in-block data.

### 8.7 Round-trip preservation

A correct writer must preserve verbatim:

- All `Unknown0C` values across all blocks.
- `$TXR.Unknown10`, `$TXR.Unknown1D`.
- `$RSI.Unknown10`, `$RSI.Unknown11`, `$RSI.Unknown12`, `$RSI.Unknown1A`.
- ResourceInfo `Values[2 .. N−1]` (the trailing opaque metadata).
- The 4-byte magic of any block type the writer doesn't understand,
  alongside its raw data and subdata.

### 8.8 Reference implementations

- **Harmony-Tools** (https://github.com/redssu/Harmony-Tools) — full SRD
  block container reader and writer, covering the `0x10`-aligned wrapper,
  every block type listed in §8.5, and the `0x20000000` / `0x40000000`
  sidecar flag scheme used by `$RSI` external data resolution.

---

## 9. SpFt FontBlock

A Spike Chunsoft–specific font metadata blob embedded inside a font
SRD's `$RSI.ResourceData` (§8.5.4). Pairs with the texture atlas stored in
the same `$RSI`'s ExternalData: the FontBlock says **which glyphs exist,
where they live in the atlas, and how to space them**; the atlas holds the
actual pixels.

### 9.1 Magic and header

The FontBlock is little-endian. The header is 44 bytes (0x2C):

| Offset | Size | Field           | Notes |
|--------|------|-----------------|-------|
| 0x00   | 4    | Magic           | `SpFt` (ASCII) |
| 0x04   | 4    | Unknown6        | u32 LE — always `6` in observed files; preserve verbatim |
| 0x08   | 4    | BitFlagCount    | u32 LE — total number of codepoints addressed by the bit-flag table (DR V3 uses `65375` = 0xFF5F) |
| 0x0C   | 4    | FontNameLength  | u32 LE — length of the font name in UTF-16 characters, excluding the terminator |
| 0x10   | 4    | FontNamePtr     | u32 LE — offset (within the FontBlock) of the UTF-16 LE font name |
| 0x14   | 4    | GlyphCount      | u32 LE — number of entries in the BBox table |
| 0x18   | 4    | BBoxTablePtr    | u32 LE — offset of the per-glyph BBox table |
| 0x1C   | 4    | BitFlagsPtr     | u32 LE — offset of the bit-flag table (always `0x2C` in observed files) |
| 0x20   | 4    | IndexTablePtr   | u32 LE — offset of the index table |
| 0x24   | 4    | ScaleFlag       | u32 LE — preserve verbatim |
| 0x28   | 4    | FontNamePtrsPtr | u32 LE — offset of a 4 × u32 LE "font name pointers" array (each entry equals `FontNamePtr`) |

### 9.2 Bit-flag table

Starts at `BitFlagsPtr` (= `0x2C`). Size in bytes:

```text
BitFlagsByteCount = ceil(BitFlagCount / 8)
```

One bit per Unicode codepoint, packed with bit 0 (LSB) of each byte
representing the first codepoint of that byte's 8-codepoint window:

```text
bit_index_within_byte = codepoint & 0b111
byte_within_table     = codepoint >> 3
present  = (table[byte_within_table] >> bit_index_within_byte) & 1 == 1
```

A set bit means "this codepoint is in the font" — i.e. has an entry in the
index table and BBox table.

### 9.3 Index table

Starts at `IndexTablePtr` (= `BitFlagsPtr + BitFlagsByteCount`).

The index table is **sparse**: it has one u32 LE entry per 32-codepoint
window of the bit-flag table. For a codepoint `c`, compute:

```text
char_offset = (c / 8) & ~3        # round down to a multiple of 4 bytes
glyph_index = read_u32_le(index_table + char_offset)
```

`glyph_index` is the **base** index into the BBox table for the first
codepoint whose bit-flag byte fell inside this 32-codepoint window. To
disambiguate when multiple codepoints in the same window are present,
increment `glyph_index` by 1 for each earlier present codepoint in the
same window. (Equivalent algorithm: walk the codepoints in ascending
order; assign each present codepoint a running index; record the running
index in `index_table[char_offset]` for the first codepoint of each new
window.)

### 9.4 BBox table

Starts at `BBoxTablePtr`. Contains `GlyphCount` entries, each 8 bytes:

| Offset | Size | Field    | Notes |
|--------|------|----------|-------|
| 0      | 3    | Position | packed `(x, y)` — see §9.5 |
| 3      | 2    | Size     | `(width, height)` as two u8 pixel counts |
| 5      | 3    | Kerning  | three sbytes: `(left, right, vertical)` |

The position fields are atlas pixel coordinates (top-left of the glyph's
bounding box). Size is the bounding-box width and height in pixels.
Kerning values are signed pixel deltas applied by the renderer.

### 9.5 `xy2abc` / `abc2xy` position packing

Two 12-bit unsigned integers `(x, y)` are packed into 3 bytes `(a, b, c)`:

```text
# Pack (x, y) → (a, b, c)
a = x & 0xFF                              # low 8 bits of x
b = ((y & 0xF) << 4) | ((x >> 8) & 0xF)   # top 4 of x in low nibble, low 4 of y in high nibble
c = (y >> 4) & 0xFF                       # top 8 bits of y

# Unpack (a, b, c) → (x, y)
x = ((b & 0xF) << 8) | a                  # low 4 of b shifts up to form top 4 of x
y = ((b >> 4) & 0xF) | (c << 4)           # high 4 of b shifts up to form low 4 of y
```

Each coordinate has a 12-bit range (0 … 4095). The byte `b` carries the
top 4 bits of `x` in its low nibble and the low 4 bits of `y` in its
high nibble.

### 9.6 Font name

At `FontNamePtr` (always preceded by a 4 × u32 LE array of font-name
pointers at `FontNamePtrsPtr`, each entry equal to `FontNamePtr`).

- Encoding: UTF-16 LE.
- Terminated by a 2-byte `0x00 0x00`.

`FontNameLength` counts UTF-16 code units, not bytes.

### 9.7 Read flow

```text
open font_block_bytes
assert read_bytes(4) == "SpFt"
unknown6, bit_flag_count, font_name_length, font_name_ptr,
    glyph_count, bbox_table_ptr, bit_flags_ptr, index_table_ptr,
    scale_flag, font_name_ptrs_ptr
        = read ten u32 LE

# Bit flags → codepoint list
bit_flags_byte_count = ceil(bit_flag_count / 8)
seek(bit_flags_ptr)
bit_flags = read_bytes(bit_flags_byte_count)
charset = []
for byte_index in 0 .. bit_flags_byte_count - 1:
    b = bit_flags[byte_index]
    for bit in 0 .. 7:
        codepoint = byte_index * 8 + bit
        if codepoint >= 55296: break              # DR V3 cuts off at U+D800
        if (b >> bit) & 1:
            charset.append(codepoint)

# Reconstruct glyph index for each codepoint
glyphs = {}
running_offset_in_window = {}
for charset_position, c in enumerate(charset):
    char_offset = (c // 8) & ~3
    seek(index_table_ptr + char_offset)
    base = read_u32_le()
    seen = running_offset_in_window.get(char_offset, 0)
    running_offset_in_window[char_offset] = seen + 1
    glyph_index = base + seen
    glyphs[c] = { kerning_index: glyph_index }

# BBox entries
seek(bbox_table_ptr)
kerning_list = []
for _ in 0 .. glyph_count - 1:
    pos_bytes = read_bytes(3)
    size      = read_bytes(2)
    kerning   = read_bytes(3)  # interpret as 3 sbytes
    x, y      = abc2xy(pos_bytes[0], pos_bytes[1], pos_bytes[2])
    kerning_list.append({ position: (x, y), size: size, kerning: kerning })

for c, g in glyphs.items():
    g.position = kerning_list[g.kerning_index].position
    g.size     = kerning_list[g.kerning_index].size
    g.kerning  = kerning_list[g.kerning_index].kerning

# Font name
seek(font_name_ptr)
font_name = read_utf16_le_null_terminated()
```

### 9.8 Write flow

```text
open font_block_buffer for writing

# Reserve header space
seek(0x2C)                                       # BitFlagsPtr is fixed

# Sort glyphs by codepoint so collisions resolve in favor of the lowest codepoint
sorted_glyphs = glyphs sorted by codepoint
bit_flags_byte_count = ceil(bit_flag_count / 8)

# Write zero-filled bit-flag region
write zero_bytes(bit_flags_byte_count)

# Index table immediately follows
index_table_ptr = 0x2C + bit_flags_byte_count
already_written_index_windows = set()

# As we lay out glyphs, also set their bit-flag bit and (lazily) write index entries
for i, glyph in enumerate(sorted_glyphs):
    glyph.index = i

    cp = glyph.codepoint
    byte_offset_in_flags = (cp >> 3) + 0x2C
    bit_offset           = cp & 0b111

    seek(byte_offset_in_flags)
    b = read_u8()
    b |= (1 << bit_offset)
    seek(byte_offset_in_flags)
    write_u8(b)

    char_offset = (cp // 8) & ~3
    if char_offset not in already_written_index_windows:
        seek(index_table_ptr + char_offset)
        write_u32_le(i)
        already_written_index_windows.add(char_offset)

    bbox_table_ptr = max(bbox_table_ptr, index_table_ptr + char_offset)

bbox_table_ptr += 4                              # leave room for the last index entry

# BBox entries
seek(bbox_table_ptr)
for glyph in sorted_glyphs:
    (a, b, c) = xy2abc(glyph.position.x, glyph.position.y)
    write_bytes([a, b, c])
    write_u8(glyph.size.width)
    write_u8(glyph.size.height)
    write_sbyte(glyph.kerning.left)
    write_sbyte(glyph.kerning.right)
    write_sbyte(glyph.kerning.vertical)

# Font name pointers and font name
font_name_ptrs_ptr = current_position()
font_name_ptr      = font_name_ptrs_ptr + 0x10
for _ in 0 .. 3:
    write_u32_le(font_name_ptr)
write_utf16_le(font_name)
write_u16_le(0)                                  # null terminator

# Header
seek(0)
write_bytes("SpFt")
write_u32_le(unknown6)
write_u32_le(bit_flag_count)
write_u32_le(font_name_length)
write_u32_le(font_name_ptr)
write_u32_le(len(sorted_glyphs))                 # GlyphCount
write_u32_le(bbox_table_ptr)
write_u32_le(0x2C)                               # BitFlagsPtr
write_u32_le(index_table_ptr)
write_u32_le(scale_flag)
write_u32_le(font_name_ptrs_ptr)
```

### 9.9 Round-trip preservation

- `Unknown6` (4 bytes) — always `6`, but treat as opaque.
- `ScaleFlag` (4 bytes).
- The `FontNamePtrsPtr` 4 × u32 LE pointer array — every entry equals
  `FontNamePtr` in observed files.

### 9.10 Reference implementations

- **Harmony-Tools** (https://github.com/redssu/Harmony-Tools) — full
  read/write of the SpFt FontBlock, including the bit-flag table, the
  sparse index-table writer, and the `xy2abc` / `abc2xy` position
  packing. The format was originally reverse-engineered by Paks and
  implemented by redssu.

---

## 10. GPU texture formats

The `$TXR.Format` field (§8.5.3) is a single byte selecting one of the
following pixel formats. A reader that opens a font texture must decode
the appropriate format before reading or modifying glyph pixels; a writer
must re-encode to the same format the original used (or to `ARGB8888` if
the texture is being rewritten as raw).

### 10.1 Format enum

| Enum value | Hex | Name        | Bits/pixel | Block size | Description |
|------------|-----|-------------|-----------:|-----------:|-------------|
| 0x00       | —   | Unknown     | —          | —          | Placeholder; never observed in valid files |
| 0x01       | 1   | ARGB8888    | 32         | 1×1        | Raw 4-channel; alpha first in source memory |
| 0x02       | 2   | BGR565      | 16         | 1×1        | Raw 3-channel |
| 0x05       | 5   | BGRA4444    | 16         | 1×1        | Raw 4-channel, 4-bits per channel |
| 0x0F       | 15  | DXT1RGB     | 4          | 4×4        | Block-compressed RGB (S3TC) |
| 0x11       | 17  | DXT5        | 8          | 4×4        | Block-compressed RGBA (S3TC) |
| 0x14       | 20  | BC5 / RGTC2 | 8          | 4×4        | Block-compressed 2-channel; both channels are 8-bit |
| 0x16       | 22  | BC4 / RGTC1 | 4          | 4×4        | Block-compressed 1-channel |
| 0x1A       | 26  | Indexed8    | 8          | 1×1        | Palette-indexed (DR V3 fonts do not use this) |
| 0x1C       | 28  | BPTC        | 8          | 4×4        | Block-compressed high-quality (BC6H/BC7) |

The hex values are those observed in the `TextureFormat` enum used by
reference SRD implementations (see §10.5).

### 10.2 DR V3 font conventions

In observed DR V3 font textures:

- `Swizzle` is always `1` (unswizzled).
- `Palette` is always `0` (no palette).
- `Format` is one of `ARGB8888`, `BC4`, or `BC5`. Other regions /
  platforms of the same game may use `DXT5` or `BPTC` for the same font
  slot.

Font atlases are conceptually monochrome — the glyph silhouette is
stored in a single intensity channel, replicated where necessary into
RGB so the engine's renderer reads the same luminance regardless of how
it interprets the channel order. When decoding a `BC4` font, the single
decoded channel carries the intensity; when decoding `BC5`, channel 0
carries the intensity and channel 1 may be unused.

### 10.3 Decoding

This document does **not** include byte-level specifications for the
block-compressed formats. They are industry-standard GPU formats with
authoritative specifications elsewhere:

- **S3TC / DXT1 / DXT5** — Microsoft documentation, "Block Compression
  (BC1/BC3) for DirectX".
- **RGTC1 / RGTC2 (BC4 / BC5)** — Microsoft documentation, "Block
  Compression (BC4/BC5) for DirectX".
- **BPTC (BC6H / BC7)** — Microsoft documentation, "Block Compression
  (BC6H/BC7) for DirectX".

Use a known, well-tested reference implementation; reverse-engineering
these from a single existing reader will only reproduce one of many
edge-case behaviors and is not recommended.

`ARGB8888`, `BGR565`, and `BGRA4444` are raw / linear and need no
external reference — just walk the pixel buffer row-by-row.

### 10.4 Texture data layout in `.srdv`

When `Swizzle == 1` (the only value DR V3 fonts use), pixel data is
linear top-to-bottom, left-to-right. For block-compressed formats,
"rows" are rows of 4×4 pixel blocks; total byte size is:

```text
blocks_w = ceil(DisplayWidth  / 4)
blocks_h = ceil(DisplayHeight / 4)
bytes    = blocks_w * blocks_h * bytes_per_block
```

where `bytes_per_block` is 8 for DXT1/BC4 and 16 for DXT5/BC5/BPTC. For
raw formats, `bytes = DisplayWidth * DisplayHeight * (bits_per_pixel / 8)`.

The offset of this pixel buffer in `.srdv` (or `.srdi`) is encoded in
the parent `$RSI` block's `ResourceInfoList[0].Values[0]` (low 29 bits),
and its size in `Values[1]`.

### 10.5 Reference implementations

- **Harmony-Tools** (https://github.com/redssu/Harmony-Tools) — the
  `TextureFormat` enum and the SRD-to-image decode path that selects an
  appropriate GPU texture decoder for each format. The format → decoder
  mapping there is a useful reference for which formats are observed in
  practice on DR V3 font textures.

---

## 11. Cross-format notes

### 11.1 STX ↔ WRD pairing

Dialogue lines are split between the STX (text) and WRD (script flow). To
recover the speaker of an STX string `S` with ID `id`:

1. Load the paired WRD (same base filename, `.wrd` extension).
2. Scan WRD commands forward to find the `LOC` whose argument equals `id`.
3. Scan backwards from that `LOC` until you encounter either:
   - a `CHN` opcode — its argument is the speaker character ID, or
   - a `CHK` opcode (encountered first) — the line is a choice/branch, not
     a speech bubble.

The detailed pairing algorithm — including `WAK wkHeroNo = X` → synthetic
`CHN` rewriting and `CHR` → `CHN` mapping — is layered *on top of* the
byte-level WRD format and is documented here only for completeness; a
strict format reader needs only the WRD spec in §7.

### 11.2 Speaker character IDs

WRD `CHN` arguments are parameter strings like `C000_`, `C001_`, etc. — a
trailing-underscore convention for Danganronpa V3 character codes. The
mapping from character ID to human-readable name (e.g. `C001_` → "Kaede
Akamatsu") is a Danganronpa-specific lookup table, not a property of the
WRD format itself. A canonical table is maintained by the Danganronpa
modding community. Enumerating it is outside the scope of this document.

### 11.3 File-path conventions inside the CPK

The DR V3 CPKs contain (among others):

- `wrd_script/<chapter>/*.SPC` — chapter dialogue archives (each holds many
  `.stx` + `.wrd` pairs).
- `wrd_data_us/`, `wrd_data_jp/`, etc. — localization-specific WRD/STX
  bundles for non-dialogue text.
- `dat/` — top-level `.dat` files for engine tables (some are translatable,
  many are not).

These conventions affect which files a translator needs to reach but are not
part of the binary file format definitions.

---

## 12. Out of scope

The following formats and topics are intentionally excluded from this
document:

- **`$CMP` / SRD-compressed SPC wrapper** — A compression layer used by the
  console versions; detect by the `$CMP` magic at offset 0 and skip.
  Decompression requires a separate SRD-specific codec not documented here.
- **Audio formats** — `.HCA`, `.ADX`, etc. Not translatable.
- **Non-font image formats** — `.PNG`, `.DDS`, and SRD textures that do not
  carry font data. The SRD container itself is documented (§8) but image
  semantics for non-font textures are out of scope.
- **GPU block-compression codecs** — DXT1, DXT5, BC4, BC5, BPTC. These are
  industry-standard formats; §10 points at authoritative external specs.
- **TTF / OTF source typefaces** — Well-documented industry standards; not
  Danganronpa-specific.
- **Font patcher workflow** — TTF rasterization, atlas placement / packing,
  umlaut compositing, kerning derivation, all live in a separate document.
- **Translation workflows** — Translation memory formats, AI translation
  pipelines, export/import to translator-friendly formats.
- **Patching strategy** — Loose-file overlay vs. CPK repack; mod loaders;
  game-specific install layouts.
- **Tooling and library choices** — Which language, which crates/packages,
  which libraries to wrap. Language-agnostic by design.

Each of those belongs in a separate document.

---

## 13. References

- **Harmony-Tools** (https://github.com/redssu/Harmony-Tools) — the
  Danganronpa-specific format readers and writers (STX, SPC, WRD, SRD,
  SpFt FontBlock, the `TextureFormat` enum, and the `xy2abc` / `abc2xy`
  position packing).
- **CriFsV2Lib** (https://github.com/Sewer56/CriFsV2Lib) — read-only CPK
  reader; the closest match to the read path described in §3.
- **CriPakTools** (https://github.com/esperknight/CriPakTools) — CPK
  reader and writer; longest-standing reference for the writer side.
- **ConnorKrammer/cpk-tools** (https://github.com/ConnorKrammer/cpk-tools)
  — additional CPK toolset; useful for cross-checking @UTF table edge
  cases.
- **Valkyria Chronicles file-format references**
  (https://gist.github.com/unknownbrackets/78c4631a4091044d381432ffb7f1bae4)
  — the most accessible public description of CRILAYLA (§3.8). Note: the
  DR V3 SPC codec (§4.5) is **different** from CRILAYLA but is sometimes
  confused with it in third-party documentation.
- **Microsoft "Block Compression in Direct3D 10"**
  (https://learn.microsoft.com/en-us/windows/win32/direct3d10/d3d10-graphics-programming-guide-resources-block-compression)
  — authoritative spec for BC1 / BC3 (DXT1 / DXT5), BC4, BC5, BC6H, BC7,
  referenced by §10.
- **Khronos KTX / DDS pixel-format references** — supplementary sources
  for cross-checking GPU texture decoding when the Microsoft
  documentation is ambiguous.
