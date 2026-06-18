//! `drv3-translate` — apply translation JSON files to Danganronpa V3 CPKs.
//!
//! The library crate [`drv3_translate`] does the heavy lifting; this binary
//! handles I/O, argument parsing, JSON loading, and writing the patched
//! game data back to disk.

mod cmd_apply;
mod cmd_validate;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser, Debug)]
#[command(
    name = "drv3-translate",
    about = "Apply translation JSONs to Danganronpa V3 game data",
    version
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Apply one or more translation JSONs to the supplied CPKs and write
    /// patched output.
    Apply(ApplyArgs),
    /// Load the JSONs (and optionally compare against on-disk CPKs) but
    /// don't write any patched output. Useful as a pre-flight check.
    Validate(ValidateArgs),
}

#[derive(clap::Args, Debug)]
struct ApplyArgs {
    /// One or more translation JSON files.
    #[arg(long = "json", required = true)]
    json: Vec<PathBuf>,
    /// One or more source CPK files. Order is preserved in the report.
    #[arg(long = "cpk", required = true)]
    cpk: Vec<PathBuf>,
    /// Output directory. Repack mode writes `<out>/<cpk_name>` per CPK;
    /// extract mode merges every CPK into a single tree at
    /// `<out>/<dir>/<file>` (last `--cpk` wins on path collisions).
    #[arg(long = "out", required = true)]
    out: PathBuf,
    /// Output mode.
    #[arg(long = "mode", value_enum, default_value_t = OutputMode::Repack)]
    mode: OutputMode,
    /// What to do when the on-disk source string doesn't match the JSON `source`.
    #[arg(long = "on-drift", value_enum, default_value_t = DriftPolicyArg::Warn)]
    on_drift: DriftPolicyArg,
    /// Optional path for a JSON report (drift events, missing slots, counts).
    #[arg(long = "report")]
    report: Option<PathBuf>,
    /// Number of rayon worker threads. 0 = default (logical CPUs).
    #[arg(long = "threads", default_value_t = 0)]
    threads: usize,
}

#[derive(clap::Args, Debug)]
struct ValidateArgs {
    #[arg(long = "json", required = true)]
    json: Vec<PathBuf>,
    /// Optional CPKs to compare against — enables drift detection.
    #[arg(long = "cpk")]
    cpk: Vec<PathBuf>,
}

#[derive(Copy, Clone, Debug, ValueEnum)]
enum OutputMode {
    /// Repack each input CPK into a new CPK file at the output path.
    Repack,
    /// Extract each input CPK into a folder tree under the output path.
    Extract,
}

#[derive(Copy, Clone, Debug, ValueEnum)]
enum DriftPolicyArg {
    /// Log drift events and write the target anyway.
    Warn,
    /// Log drift events and skip those slots.
    Skip,
    /// Abort on the first drift event.
    Error,
}

impl From<DriftPolicyArg> for drv3_translate::DriftPolicy {
    fn from(value: DriftPolicyArg) -> Self {
        match value {
            DriftPolicyArg::Warn => Self::WarnAndApply,
            DriftPolicyArg::Skip => Self::Skip,
            DriftPolicyArg::Error => Self::Error,
        }
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match &cli.cmd {
        Cmd::Apply(args) => cmd_apply::run(args),
        Cmd::Validate(args) => cmd_validate::run(args),
    }
}

/// Memory-map a file for read-only access. Used for the largest inputs
/// (CPK archives up to 12 GB) so the OS pages them in on demand instead
/// of forcing a full read into a heap `Vec`.
///
/// `Mmap::map` is `unsafe` because the mapping shares the kernel's view
/// of the file: another process mutating the file invalidates the slice.
/// Since we open read-only and never modify the file ourselves, and the
/// caller drops the mapping promptly after parse, the contract is
/// satisfied.
///
/// # Errors
///
/// Returns an error if the file can't be opened or the mapping fails.
pub(crate) fn mmap_file(path: &Path) -> Result<memmap2::Mmap> {
    let file = std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    #[expect(
        unsafe_code,
        reason = "the crate's one unsafe site: memmap2::Mmap::map is unsafe — see the SAFETY note below"
    )]
    // SAFETY: file is opened read-only and used only for the duration of
    // this call's caller; no concurrent writer is expected on a game-data
    // CPK that we're patching from.
    let mmap = unsafe { memmap2::Mmap::map(&file) }
        .with_context(|| format!("mapping {}", path.display()))?;
    Ok(mmap)
}
