//! `drv3` — command-line tool for reading and writing Danganronpa V3 game data.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(
    name = "drv3",
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
        path: PathBuf,
    },
}

#[derive(Subcommand, Debug)]
enum CpkCmd {
    /// List files inside the archive.
    List { archive: PathBuf },
    /// Extract every file under `out_dir`.
    Extract { archive: PathBuf, out_dir: PathBuf },
    /// Repack the directory layout produced by `extract` into a CPK.
    Pack { in_dir: PathBuf, out: PathBuf },
}

#[derive(Subcommand, Debug)]
enum SpcCmd {
    List { archive: PathBuf },
    Extract { archive: PathBuf, out_dir: PathBuf },
    Pack { in_dir: PathBuf, out: PathBuf },
}

#[derive(Subcommand, Debug)]
enum StxCmd {
    Dump { stx: PathBuf, out_json: PathBuf },
    Build { json: PathBuf, out_stx: PathBuf },
}

#[derive(Subcommand, Debug)]
enum DatCmd {
    Dump { dat: PathBuf, out_json: PathBuf },
    Build { json: PathBuf, out_dat: PathBuf },
}

#[derive(Subcommand, Debug)]
enum WrdCmd {
    Dump {
        wrd: PathBuf,
        out_json: PathBuf,
    },
    Build {
        json: PathBuf,
        out_wrd: PathBuf,
    },
    /// Print (speaker, `string_id`) pairs recovered from the byte-code stream.
    Dialogue {
        wrd: PathBuf,
    },
}

#[derive(Subcommand, Debug)]
enum SrdCmd {
    /// Print a tree of block types.
    Inspect { srd: PathBuf },
}

#[derive(Subcommand, Debug)]
enum SpftCmd {
    Dump { spft: PathBuf, out_json: PathBuf },
    Build { json: PathBuf, out_spft: PathBuf },
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
                let mut dest = out_dir.clone();
                if !file.dir_name.is_empty() {
                    for component in file.dir_name.split('/').filter(|c| !c.is_empty()) {
                        dest.push(component);
                    }
                }
                fs::create_dir_all(&dest)?;
                dest.push(&file.file_name);
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
            let read_sidecar = |name: &Option<String>, default: &str| -> Result<Option<Vec<u8>>> {
                if let Some(filename) = name {
                    let path = in_dir.join(filename);
                    Ok(Some(
                        fs::read(&path).with_context(|| format!("reading {}", path.display()))?,
                    ))
                } else if in_dir.join(default).is_file() {
                    // Sidecar exists on disk but manifest didn't reference it —
                    // ignore. Future-proofs against half-edited manifests.
                    Ok(None)
                } else {
                    Ok(None)
                }
            };
            let etoc = read_sidecar(&manifest.etoc_packet, ETOC_SIDECAR)?;
            let itoc = read_sidecar(&manifest.itoc_packet, ITOC_SIDECAR)?;
            let gtoc = read_sidecar(&manifest.gtoc_packet, GTOC_SIDECAR)?;

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
            let stx: drv3_stx::Stx = dto.into();
            fs::write(&out_stx, stx.to_bytes())?;
            Ok(())
        }
    }
}

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
            let wrd: drv3_wrd::Wrd = dto.into();
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
            let spft: drv3_spft::SpFt = dto.into();
            fs::write(&out_spft, spft.to_bytes())?;
            Ok(())
        }
    }
}

fn roundtrip(path: &Path) -> Result<()> {
    let bytes = read_file(path)?;
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    let (kind, written) = match ext.as_str() {
        "cpk" => {
            let parsed = drv3_cpk::Cpk::parse(&bytes)?;
            ("cpk", parsed.to_bytes()?)
        }
        "spc" => {
            let parsed = drv3_spc::Spc::parse(&bytes)?;
            ("spc", parsed.to_bytes()?)
        }
        "stx" => {
            // The `.stx` extension is overloaded in DR V3: dialogue STX
            // tables start with the magic `STXT`, while font-bearing SRDs
            // start with a `$XXX`-style block magic (typically `$CFH`).
            // Sniff the first four bytes to pick the right parser.
            if bytes.starts_with(b"STXT") {
                let parsed = drv3_stx::Stx::parse(&bytes)?;
                ("stx (dialogue)", parsed.to_bytes())
            } else {
                let parsed = drv3_srd::Srd::parse(&bytes)?;
                ("stx (srd)", parsed.to_bytes()?)
            }
        }
        "dat" => {
            let parsed = drv3_dat::Dat::parse(&bytes)?;
            ("dat", parsed.to_bytes()?)
        }
        "wrd" => {
            let parsed = drv3_wrd::Wrd::parse(&bytes)?;
            ("wrd", parsed.to_bytes()?)
        }
        "srd" => {
            let parsed = drv3_srd::Srd::parse(&bytes)?;
            ("srd", parsed.to_bytes()?)
        }
        other => bail!("unknown extension {other:?}"),
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
