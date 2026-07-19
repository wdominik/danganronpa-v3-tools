//! `drv3-cli` — command-line tool for reading and writing Danganronpa V3 game data.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(
    name = "drv3-cli",
    about = "Read/write Danganronpa V3 game data files",
    version
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// CPK archive operations.
    #[command(subcommand)]
    Cpk(CpkCmd),
    /// SPC archive operations.
    #[command(subcommand)]
    Spc(SpcCmd),
    /// STX string-table operations.
    #[command(subcommand)]
    Stx(StxCmd),
    /// DAT typed-table operations.
    #[command(subcommand)]
    Dat(DatCmd),
    /// WRD script operations.
    #[command(subcommand)]
    Wrd(WrdCmd),
    /// SRD block-container operations.
    #[command(subcommand)]
    Srd(SrdCmd),
    /// `SpFt` font-block operations.
    #[command(subcommand)]
    Spft(SpftCmd),
    /// Parse a file, re-emit it, and confirm byte-for-byte equality.
    Roundtrip {
        /// File to verify. Format is inferred from the extension.
        #[arg(long = "in")]
        path: PathBuf,
    },
}

#[derive(Subcommand, Debug)]
enum CpkCmd {
    /// List files inside the archive.
    List {
        /// CPK archive to read.
        #[arg(long = "in")]
        archive: PathBuf,
    },
    /// Extract every file under the output directory.
    Extract {
        /// CPK archive to read.
        #[arg(long = "in")]
        archive: PathBuf,
        /// Output directory for the extracted tree.
        #[arg(long = "out")]
        out_dir: PathBuf,
    },
    /// Repack the directory layout produced by `extract` into a CPK.
    Pack {
        /// Directory produced by `cpk extract`.
        #[arg(long = "in")]
        in_dir: PathBuf,
        /// Output CPK path.
        #[arg(long = "out")]
        out: PathBuf,
    },
}

#[derive(Subcommand, Debug)]
enum SpcCmd {
    /// List members inside the SPC archive.
    List {
        /// SPC archive to read.
        #[arg(long = "in")]
        archive: PathBuf,
    },
    /// Extract every member under the output directory.
    Extract {
        /// SPC archive to read.
        #[arg(long = "in")]
        archive: PathBuf,
        /// Output directory for the extracted members.
        #[arg(long = "out")]
        out_dir: PathBuf,
    },
    /// Repack the directory layout produced by `extract` into an SPC.
    Pack {
        /// Directory produced by `spc extract`.
        #[arg(long = "in")]
        in_dir: PathBuf,
        /// Output SPC path.
        #[arg(long = "out")]
        out: PathBuf,
    },
}

#[derive(Subcommand, Debug)]
enum StxCmd {
    /// Dump an STX string table to JSON.
    Dump {
        /// STX file to read.
        #[arg(long = "in")]
        stx: PathBuf,
        /// Output JSON path.
        #[arg(long = "out")]
        out_json: PathBuf,
    },
    /// Build an STX file from JSON.
    Build {
        /// JSON file to read.
        #[arg(long = "in")]
        json: PathBuf,
        /// Output STX path.
        #[arg(long = "out")]
        out_stx: PathBuf,
    },
}

#[derive(Subcommand, Debug)]
enum DatCmd {
    /// Dump a DAT typed table to JSON.
    Dump {
        /// DAT file to read.
        #[arg(long = "in")]
        dat: PathBuf,
        /// Output JSON path.
        #[arg(long = "out")]
        out_json: PathBuf,
    },
    /// Build a DAT file from JSON.
    Build {
        /// JSON file to read.
        #[arg(long = "in")]
        json: PathBuf,
        /// Output DAT path.
        #[arg(long = "out")]
        out_dat: PathBuf,
    },
}

#[derive(Subcommand, Debug)]
enum WrdCmd {
    /// Dump a WRD script (opcodes and strings) to JSON.
    Dump {
        /// WRD file to read.
        #[arg(long = "in")]
        wrd: PathBuf,
        /// Output JSON path.
        #[arg(long = "out")]
        out_json: PathBuf,
    },
    /// Build a WRD file from JSON.
    Build {
        /// JSON file to read.
        #[arg(long = "in")]
        json: PathBuf,
        /// Output WRD path.
        #[arg(long = "out")]
        out_wrd: PathBuf,
    },
    /// Print (speaker, `string_id`) pairs recovered from the byte-code stream.
    Dialogue {
        /// WRD file to read.
        #[arg(long = "in")]
        wrd: PathBuf,
    },
}

#[derive(Subcommand, Debug)]
enum SrdCmd {
    /// Print a tree of block types.
    Inspect {
        /// SRD file to read.
        #[arg(long = "in")]
        srd: PathBuf,
    },
}

#[derive(Subcommand, Debug)]
enum SpftCmd {
    /// Dump `SpFt` font metrics to JSON.
    Dump {
        /// `SpFt` file to read.
        #[arg(long = "in")]
        spft: PathBuf,
        /// Output JSON path.
        #[arg(long = "out")]
        out_json: PathBuf,
    },
    /// Build a `SpFt` file from JSON.
    Build {
        /// JSON file to read.
        #[arg(long = "in")]
        json: PathBuf,
        /// Output `SpFt` path.
        #[arg(long = "out")]
        out_spft: PathBuf,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Cpk(c) => cpk(c),
        Cmd::Spc(c) => spc(c),
        Cmd::Stx(c) => stx(c),
        Cmd::Dat(c) => dat(c),
        Cmd::Wrd(c) => wrd(c),
        Cmd::Srd(c) => srd(c),
        Cmd::Spft(c) => spft(c),
        Cmd::Roundtrip { path } => roundtrip(&path),
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "top-level CLI dispatcher for one subcommand family"
)]
/// Dispatch the `cpk` subcommand (list / extract / pack).
fn cpk(cmd: CpkCmd) -> Result<()> {
    use drv3_dto::cpk_manifest::{
        CpkManifestJson, ETOC_SIDECAR, GTOC_SIDECAR, ITOC_SIDECAR, MANIFEST_FILENAME,
    };

    match cmd {
        CpkCmd::List { archive } => {
            // Memory-map instead of `fs::read` so we don't pre-allocate 12 GB
            // of heap for the largest CPK. The kernel streams pages on demand.
            let mmap = mmap_file(&archive)?;
            let parsed = drv3_cpk::Cpk::parse(&mmap)?;
            for file in &parsed.files {
                println!(
                    "{}{}  ({} bytes, id={})",
                    if file.dir_name.is_empty() {
                        String::new()
                    } else {
                        format!("{}/", file.dir_name)
                    },
                    file.file_name,
                    file.data.len(),
                    file.id
                );
            }
            println!("({} files total)", parsed.files.len());
            Ok(())
        }
        CpkCmd::Extract { archive, out_dir } => {
            let mmap = mmap_file(&archive)?;
            let parsed = drv3_cpk::Cpk::parse(&mmap)?;
            fs::create_dir_all(&out_dir)?;

            // File bodies.
            for file in &parsed.files {
                let dest = safe_dest(&out_dir, &file.dir_name, &file.file_name)?;
                if let Some(parent) = dest.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::write(&dest, &file.data)
                    .with_context(|| format!("writing {}", dest.display()))?;
            }

            // Optional opaque packet sidecars.
            if let Some(p) = &parsed.etoc_packet {
                fs::write(out_dir.join(ETOC_SIDECAR), p)?;
            }
            if let Some(p) = &parsed.itoc_packet {
                fs::write(out_dir.join(ITOC_SIDECAR), p)?;
            }
            if let Some(p) = &parsed.gtoc_packet {
                fs::write(out_dir.join(GTOC_SIDECAR), p)?;
            }

            // Manifest.
            let manifest = drv3_dto::cpk_manifest::CpkManifestJson::from(&parsed);
            let manifest_path = out_dir.join(MANIFEST_FILENAME);
            fs::write(&manifest_path, serde_json::to_string_pretty(&manifest)?)
                .with_context(|| format!("writing {}", manifest_path.display()))?;

            eprintln!(
                "extracted {} files + manifest.json ({}{}{})",
                parsed.files.len(),
                if parsed.etoc_packet.is_some() {
                    "etoc "
                } else {
                    ""
                },
                if parsed.itoc_packet.is_some() {
                    "itoc "
                } else {
                    ""
                },
                if parsed.gtoc_packet.is_some() {
                    "gtoc "
                } else {
                    ""
                },
            );
            Ok(())
        }
        CpkCmd::Pack { in_dir, out } => {
            let manifest_path = in_dir.join(MANIFEST_FILENAME);
            let manifest_str = fs::read_to_string(&manifest_path).with_context(|| {
                format!(
                    "reading {} (extract first with `drv3-cli cpk extract`)",
                    manifest_path.display()
                )
            })?;
            let manifest: CpkManifestJson = serde_json::from_str(&manifest_str)
                .with_context(|| format!("parsing {}", manifest_path.display()))?;

            // Load every file body referenced by the manifest, in order.
            let mut file_bodies: Vec<(drv3_dto::cpk_manifest::CpkFileJson, Vec<u8>)> =
                Vec::with_capacity(manifest.files.len());
            for entry in &manifest.files {
                let body_path = in_dir.join(&entry.path);
                let data = fs::read(&body_path)
                    .with_context(|| format!("reading {}", body_path.display()))?;
                file_bodies.push((entry.clone(), data));
            }

            // Load optional packet sidecars. The manifest's *_packet field is
            // a filename; we resolve it relative to in_dir. (Always uses the
            // standard `_etoc.bin` / `_itoc.bin` / `_gtoc.bin` names today.)
            // A sidecar present on disk but not referenced by the manifest is
            // intentionally ignored (guards against half-edited manifests).
            let read_sidecar = |name: &Option<String>| -> Result<Option<Vec<u8>>> {
                let Some(filename) = name else {
                    return Ok(None);
                };
                let path = in_dir.join(filename);
                Ok(Some(
                    fs::read(&path).with_context(|| format!("reading {}", path.display()))?,
                ))
            };
            let etoc = read_sidecar(&manifest.etoc_packet)?;
            let itoc = read_sidecar(&manifest.itoc_packet)?;
            let gtoc = read_sidecar(&manifest.gtoc_packet)?;

            let cpk = manifest.into_cpk(file_bodies, etoc, itoc, gtoc)?;
            let bytes = cpk.to_bytes()?;
            fs::write(&out, &bytes).with_context(|| format!("writing {}", out.display()))?;
            eprintln!(
                "packed {} files -> {} ({} bytes)",
                cpk.files.len(),
                out.display(),
                bytes.len()
            );
            Ok(())
        }
    }
}

/// Dispatch the `spc` subcommand (list / extract / pack).
fn spc(cmd: SpcCmd) -> Result<()> {
    use drv3_dto::spc_manifest::{MANIFEST_FILENAME, SpcManifestJson};

    match cmd {
        SpcCmd::List { archive } => {
            let bytes = read_file(&archive)?;
            let parsed = drv3_spc::Spc::parse(&bytes)?;
            for entry in &parsed.entries {
                println!(
                    "{}  ({} bytes, flag={})",
                    entry.name_as_str().unwrap_or("<non-utf8>"),
                    entry.data.len(),
                    entry.compression_flag
                );
            }
            Ok(())
        }
        SpcCmd::Extract { archive, out_dir } => {
            let bytes = read_file(&archive)?;
            let parsed = drv3_spc::Spc::parse(&bytes)?;
            fs::create_dir_all(&out_dir)?;
            for entry in &parsed.entries {
                let name = entry.name_as_str().context("SPC entry name is not UTF-8")?;
                let dest = out_dir.join(name);
                fs::write(&dest, &entry.data)
                    .with_context(|| format!("writing {}", dest.display()))?;
            }

            let manifest = SpcManifestJson::from(&parsed);
            let manifest_path = out_dir.join(MANIFEST_FILENAME);
            fs::write(&manifest_path, serde_json::to_string_pretty(&manifest)?)
                .with_context(|| format!("writing {}", manifest_path.display()))?;

            eprintln!("extracted {} entries + manifest.json", parsed.entries.len());
            Ok(())
        }
        SpcCmd::Pack { in_dir, out } => {
            let manifest_path = in_dir.join(MANIFEST_FILENAME);
            let manifest_str = fs::read_to_string(&manifest_path).with_context(|| {
                format!(
                    "reading {} (extract first with `drv3-cli spc extract`)",
                    manifest_path.display(),
                )
            })?;
            let manifest: SpcManifestJson = serde_json::from_str(&manifest_str)
                .with_context(|| format!("parsing {}", manifest_path.display()))?;

            // Load every entry body in manifest order so the on-disk entry
            // ordering is preserved (the manifest captures the original
            // non-alphabetical order).
            let mut bodies: Vec<Vec<u8>> = Vec::with_capacity(manifest.entries.len());
            for entry in &manifest.entries {
                let body_path = in_dir.join(&entry.name);
                bodies.push(
                    fs::read(&body_path)
                        .with_context(|| format!("reading {}", body_path.display()))?,
                );
            }

            let spc = manifest.into_spc(bodies)?;
            fs::write(&out, spc.to_bytes()?)
                .with_context(|| format!("writing {}", out.display()))?;
            eprintln!("packed {} entries -> {}", spc.entries.len(), out.display());
            Ok(())
        }
    }
}

/// Join a CPK-supplied `dir_name` / `file_name` under `root`, rejecting any
/// component that would escape it. A crafted archive can carry `dir_name =
/// "../../x"` or an absolute `file_name` (`/etc/x`, `C:\x`) — with `PathBuf`
/// the latter would *replace* the whole destination — so this mirrors the
/// pack-side `split_path` guard: reject `.` / `..` / `\` segments and a
/// `file_name` that is empty or carries a path separator.
fn safe_dest(root: &Path, dir_name: &str, file_name: &str) -> Result<PathBuf> {
    let mut dest = root.to_path_buf();
    for component in dir_name.split('/').filter(|c| !c.is_empty()) {
        if component == "." || component == ".." || component.contains('\\') {
            bail!("archive path {dir_name:?}/{file_name:?} has unsafe component {component:?}");
        }
        dest.push(component);
    }
    if file_name.is_empty()
        || file_name == "."
        || file_name == ".."
        || file_name.contains('/')
        || file_name.contains('\\')
    {
        bail!("archive file name {file_name:?} is not a safe single path component");
    }
    dest.push(file_name);
    Ok(dest)
}

/// Dispatch the `stx` subcommand (dump / build).
fn stx(cmd: StxCmd) -> Result<()> {
    match cmd {
        StxCmd::Dump { stx, out_json } => {
            let bytes = read_file(&stx)?;
            let parsed = drv3_stx::Stx::parse(&bytes)?;
            let json: drv3_dto::stx::StxJson = (&parsed).into();
            fs::write(&out_json, serde_json::to_string_pretty(&json)?)?;
            Ok(())
        }
        StxCmd::Build { json, out_stx } => {
            let raw = fs::read_to_string(&json)?;
            let dto: drv3_dto::stx::StxJson = serde_json::from_str(&raw)?;
            let stx: drv3_stx::Stx = dto.try_into()?;
            fs::write(&out_stx, stx.to_bytes()?)?;
            Ok(())
        }
    }
}

/// Dispatch the `dat` subcommand (dump / build).
fn dat(cmd: DatCmd) -> Result<()> {
    match cmd {
        DatCmd::Dump { dat, out_json } => {
            let bytes = read_file(&dat)?;
            let parsed = drv3_dat::Dat::parse(&bytes)?;
            let json: drv3_dto::dat::DatJson = (&parsed).into();
            fs::write(&out_json, serde_json::to_string_pretty(&json)?)?;
            Ok(())
        }
        DatCmd::Build { json, out_dat } => {
            let raw = fs::read_to_string(&json)?;
            let dto: drv3_dto::dat::DatJson = serde_json::from_str(&raw)?;
            let dat: drv3_dat::Dat = dto.try_into()?;
            fs::write(&out_dat, dat.to_bytes()?)?;
            Ok(())
        }
    }
}

/// Dispatch the `wrd` subcommand (dump / build / dialogue).
fn wrd(cmd: WrdCmd) -> Result<()> {
    match cmd {
        WrdCmd::Dump { wrd, out_json } => {
            let bytes = read_file(&wrd)?;
            let parsed = drv3_wrd::Wrd::parse(&bytes)?;
            let json: drv3_dto::wrd::WrdJson = (&parsed).into();
            fs::write(&out_json, serde_json::to_string_pretty(&json)?)?;
            Ok(())
        }
        WrdCmd::Build { json, out_wrd } => {
            let raw = fs::read_to_string(&json)?;
            let dto: drv3_dto::wrd::WrdJson = serde_json::from_str(&raw)?;
            let wrd: drv3_wrd::Wrd = dto.try_into()?;
            fs::write(&out_wrd, wrd.to_bytes()?)?;
            Ok(())
        }
        WrdCmd::Dialogue { wrd } => {
            let bytes = read_file(&wrd)?;
            let parsed = drv3_wrd::Wrd::parse(&bytes)?;
            for line in parsed.iter_dialogue_lines() {
                let speaker = line
                    .speaker_param
                    .and_then(|idx| parsed.parameters.get(idx as usize))
                    .map_or("<unknown>", String::as_str);
                println!("{:>6}  [{}]", line.string_id, speaker);
            }
            Ok(())
        }
    }
}

/// Dispatch the `srd` subcommand (inspect).
fn srd(cmd: SrdCmd) -> Result<()> {
    match cmd {
        SrdCmd::Inspect { srd } => {
            let bytes = read_file(&srd)?;
            let parsed = drv3_srd::Srd::parse(&bytes)?;
            for block in &parsed.blocks {
                print_block(block, 0);
            }
            Ok(())
        }
    }
}

/// Recursively print an SRD block tree, one indented line per block.
fn print_block(block: &drv3_srd::Block, depth: usize) {
    let indent = "  ".repeat(depth);
    match block {
        drv3_srd::Block::Cfh => println!("{indent}$CFH"),
        drv3_srd::Block::Ct0 => println!("{indent}$CT0"),
        drv3_srd::Block::Txr { txr, children } => {
            println!(
                "{indent}$TXR  {}x{} format={:#04x} scanline={} swizzle={} palette={}",
                txr.display_width,
                txr.display_height,
                txr.format,
                txr.scanline,
                txr.swizzle,
                txr.palette,
            );
            for c in children {
                print_block(c, depth + 1);
            }
        }
        drv3_srd::Block::Rsi { rsi, children } => {
            println!(
                "{indent}$RSI  resource_info_count={} resource_info_size={} resource_data={} bytes",
                rsi.resource_info_count,
                rsi.resource_info_size,
                rsi.resource_data.len()
            );
            for (i, entry) in rsi.resource_info_list.iter().enumerate() {
                let values: Vec<String> = entry.iter().map(|v| format!("{v:#010x}")).collect();
                println!("{indent}  resource_info[{i}] = [{}]", values.join(", "));
            }
            for c in children {
                print_block(c, depth + 1);
            }
        }
        drv3_srd::Block::Other {
            magic,
            data,
            children,
            ..
        } => {
            let m = std::str::from_utf8(magic).unwrap_or("????");
            println!("{indent}{m}  ({} data bytes)", data.len());
            for c in children {
                print_block(c, depth + 1);
            }
        }
    }
}

/// Dispatch the `spft` subcommand (dump / build).
fn spft(cmd: SpftCmd) -> Result<()> {
    match cmd {
        SpftCmd::Dump { spft, out_json } => {
            let bytes = read_file(&spft)?;
            let parsed = drv3_spft::SpFt::parse(&bytes)?;
            let json: drv3_dto::spft::SpFtJson = (&parsed).into();
            fs::write(&out_json, serde_json::to_string_pretty(&json)?)?;
            Ok(())
        }
        SpftCmd::Build { json, out_spft } => {
            let raw = fs::read_to_string(&json)?;
            let dto: drv3_dto::spft::SpFtJson = serde_json::from_str(&raw)?;
            let spft: drv3_spft::SpFt = dto.try_into()?;
            fs::write(&out_spft, spft.to_bytes()?)?;
            Ok(())
        }
    }
}

/// Which format `roundtrip` should parse a file as, inferred from its
/// extension (and, for the overloaded `.stx`, a leading-magic sniff).
#[derive(Debug, PartialEq, Eq)]
enum RoundtripKind {
    Cpk,
    Spc,
    StxDialogue,
    StxSrd,
    Dat,
    Wrd,
    Srd,
}

/// Classify a file for `roundtrip` from its lowercased extension and leading
/// bytes; `None` means the extension isn't recognized. The `.stx` extension is
/// overloaded in DR V3: dialogue STX tables start with the magic `STXT`, while
/// font-bearing SRDs start with a `$XXX`-style block magic (typically `$CFH`).
fn classify_roundtrip(ext: &str, head: &[u8]) -> Option<RoundtripKind> {
    Some(match ext {
        "cpk" => RoundtripKind::Cpk,
        "spc" => RoundtripKind::Spc,
        "stx" if head.starts_with(b"STXT") => RoundtripKind::StxDialogue,
        "stx" => RoundtripKind::StxSrd,
        "dat" => RoundtripKind::Dat,
        "wrd" => RoundtripKind::Wrd,
        "srd" => RoundtripKind::Srd,
        _ => return None,
    })
}

/// Parse a file (format inferred from its extension), re-emit it, and report
/// whether it round-trips byte-for-byte.
fn roundtrip(path: &Path) -> Result<()> {
    let bytes = read_file(path)?;
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let Some(format) = classify_roundtrip(&ext, &bytes) else {
        bail!("unknown extension {ext:?}");
    };

    let (kind, written) = match format {
        RoundtripKind::Cpk => ("cpk", drv3_cpk::Cpk::parse(&bytes)?.to_bytes()?),
        RoundtripKind::Spc => ("spc", drv3_spc::Spc::parse(&bytes)?.to_bytes()?),
        RoundtripKind::StxDialogue => ("stx (dialogue)", drv3_stx::Stx::parse(&bytes)?.to_bytes()?),
        RoundtripKind::StxSrd => ("stx (srd)", drv3_srd::Srd::parse(&bytes)?.to_bytes()?),
        RoundtripKind::Dat => ("dat", drv3_dat::Dat::parse(&bytes)?.to_bytes()?),
        RoundtripKind::Wrd => ("wrd", drv3_wrd::Wrd::parse(&bytes)?.to_bytes()?),
        RoundtripKind::Srd => ("srd", drv3_srd::Srd::parse(&bytes)?.to_bytes()?),
    };

    if written == bytes {
        eprintln!("{}: round-trip byte-equal ({} bytes)", kind, bytes.len());
        Ok(())
    } else {
        let first_diff = bytes.iter().zip(written.iter()).position(|(a, b)| a != b);
        bail!(
            "{}: bytes differ (input {} bytes, output {} bytes, first diff at {:?})",
            kind,
            bytes.len(),
            written.len(),
            first_diff
        );
    }
}

/// Read a whole file into memory, attaching the path to any I/O error.
fn read_file(path: &Path) -> Result<Vec<u8>> {
    fs::read(path).with_context(|| format!("reading {}", path.display()))
}

/// Memory-map a file for read-only access. Used for the largest inputs (CPK
/// archives up to 12 GB) so the OS pages them in on demand instead of forcing
/// a full read into a heap `Vec`.
///
/// `Mmap::map` is `unsafe` because mapped memory shares the kernel's view of
/// the file: another process mutating the file invalidates the slice. Since
/// we open read-only and never modify the file ourselves, and the caller
/// drops the mapping promptly after parse, the contract is satisfied.
fn mmap_file(path: &Path) -> Result<memmap2::Mmap> {
    let file = std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    #[expect(
        unsafe_code,
        reason = "the crate's one unsafe site: memmap2::Mmap::map is unsafe — see the SAFETY note below"
    )]
    // SAFETY: file is opened read-only and used only for the duration of this
    // call's caller; no concurrent writer is expected on a game-data CPK.
    let mmap = unsafe { memmap2::Mmap::map(&file) }
        .with_context(|| format!("mapping {}", path.display()))?;
    Ok(mmap)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_dest_joins_valid_paths() {
        let root = Path::new("/tmp/out");
        assert_eq!(
            safe_dest(root, "wrd_script/003", "foo.spc").unwrap(),
            PathBuf::from("/tmp/out/wrd_script/003/foo.spc"),
        );
        // Root-level file, and empty/double-slash segments are dropped.
        assert_eq!(
            safe_dest(root, "", "manifest.bin").unwrap(),
            PathBuf::from("/tmp/out/manifest.bin"),
        );
        assert_eq!(
            safe_dest(root, "a//b/", "c.dat").unwrap(),
            PathBuf::from("/tmp/out/a/b/c.dat"),
        );
    }

    #[test]
    fn safe_dest_rejects_escapes() {
        let root = Path::new("/tmp/out");
        // `..` in the directory would climb out of the root.
        assert!(safe_dest(root, "../../etc", "passwd").is_err());
        assert!(safe_dest(root, "a/../b", "c").is_err());
        // Absolute / separator-bearing file names would replace the whole dest.
        assert!(safe_dest(root, "", "/etc/passwd").is_err());
        assert!(safe_dest(root, "", "C:\\windows\\x").is_err());
        assert!(safe_dest(root, "", "sub/evil").is_err());
        assert!(safe_dest(root, "", "..").is_err());
        // Backslash directory segment.
        assert!(safe_dest(root, "a\\b", "c").is_err());
    }

    #[test]
    fn classify_roundtrip_maps_extensions() {
        assert_eq!(classify_roundtrip("cpk", &[]), Some(RoundtripKind::Cpk));
        assert_eq!(classify_roundtrip("spc", &[]), Some(RoundtripKind::Spc));
        assert_eq!(classify_roundtrip("dat", &[]), Some(RoundtripKind::Dat));
        assert_eq!(classify_roundtrip("wrd", &[]), Some(RoundtripKind::Wrd));
        assert_eq!(classify_roundtrip("srd", &[]), Some(RoundtripKind::Srd));
    }

    #[test]
    fn classify_roundtrip_sniffs_overloaded_stx() {
        // `.stx` is a dialogue STX when it starts with `STXT`, else a font SRD.
        assert_eq!(
            classify_roundtrip("stx", b"STXT\x00\x00"),
            Some(RoundtripKind::StxDialogue),
        );
        assert_eq!(
            classify_roundtrip("stx", b"$CFH"),
            Some(RoundtripKind::StxSrd)
        );
        // An empty head defaults to the SRD branch.
        assert_eq!(classify_roundtrip("stx", &[]), Some(RoundtripKind::StxSrd));
    }

    #[test]
    fn classify_roundtrip_rejects_unknown_extension() {
        assert_eq!(classify_roundtrip("txt", &[]), None);
        assert_eq!(classify_roundtrip("", &[]), None);
    }
}
