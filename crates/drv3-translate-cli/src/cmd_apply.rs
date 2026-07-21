//! `drv3-translate apply` — load JSONs + CPKs, patch in memory, write output.

use std::collections::HashMap;
use std::fmt::Write;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use drv3_cpk::Cpk;
use drv3_translate::{PatchOptions, PatchReport, apply};
use serde::Serialize;

use crate::{ApplyArgs, OutputMode, mmap_file};
use drv3_dto_patch::{load_doc, merge_docs};

pub(crate) fn run(args: &ApplyArgs) -> Result<()> {
    if args.threads > 0 {
        rayon::ThreadPoolBuilder::new()
            .num_threads(args.threads)
            .build_global()
            .ok();
    }

    eprintln!("loading {} JSON file(s)…", args.json.len());
    let mut docs = Vec::with_capacity(args.json.len());
    for path in &args.json {
        docs.push(load_doc(path)?);
    }
    // merge_docs reads any PNGs referenced by font groups (relative to
    // each JSON's directory) so the engine never touches the filesystem.
    let set = merge_docs(docs)?;
    eprintln!("merged: {} file group(s) across the input", set.files.len());

    // Map each --cpk argument to its filename (the key translation
    // entries reference). Refuse early if any source path collides with
    // the requested output path.
    if args.cpk.iter().any(|cpk| paths_overlap(cpk, &args.out)) {
        return Err(anyhow!(
            "output path {} overlaps a source CPK; refusing to overwrite in place",
            args.out.display()
        ));
    }

    eprintln!("opening {} CPK(s)…", args.cpk.len());
    let mut mappings: Vec<(String, memmap2::Mmap)> = Vec::with_capacity(args.cpk.len());
    for path in &args.cpk {
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .map(str::to_string)
            .ok_or_else(|| anyhow!("CPK path {} has no filename", path.display()))?;
        let mmap = mmap_file(path)?;
        mappings.push((name, mmap));
    }

    eprintln!("parsing CPK headers…");
    let mut cpks: Vec<(String, Cpk)> = Vec::with_capacity(mappings.len());
    for (name, mmap) in &mappings {
        let cpk = Cpk::parse(mmap).with_context(|| format!("parsing CPK {name}"))?;
        cpks.push((name.clone(), cpk));
    }

    eprintln!("patching…");
    let opts = PatchOptions {
        on_drift: args.on_drift.into(),
        parallel: args.threads != 1,
    };
    let report = {
        // apply() wants &mut Cpk; build that view from the owned vec
        // without taking it apart.
        let mut view: Vec<(&str, &mut Cpk)> =
            cpks.iter_mut().map(|(n, c)| (n.as_str(), c)).collect();
        apply(&mut view, &set, &opts)?
    };

    // The input mmaps stay mapped through the write below: file bodies are
    // borrowed (zero-copy) from them, and repack/extract reads those bodies as
    // it serializes. They unmap when `run` returns.

    eprintln!("{}", summarize_report(&report));

    fs::create_dir_all(&args.out)
        .with_context(|| format!("creating output dir {}", args.out.display()))?;

    let extract_collisions = match args.mode {
        OutputMode::Repack => {
            write_repack(&cpks, &args.out)?;
            Vec::new()
        }
        OutputMode::Extract => write_extract(&cpks, &args.out)?,
    };

    if let Some(report_path) = &args.report {
        let report_json = ReportJson::build(&report, &extract_collisions);
        let json = serde_json::to_vec_pretty(&report_json)?;
        fs::write(report_path, json)
            .with_context(|| format!("writing report {}", report_path.display()))?;
        eprintln!("wrote report to {}", report_path.display());
    }

    Ok(())
}

/// Return true when `out` and `src` resolve to overlapping locations on
/// disk — used to refuse writes that would overwrite the input CPK.
///
/// `out` typically doesn't exist yet (we're about to create it), so we
/// canonicalize its nearest existing ancestor and re-attach the tail.
/// That makes the check robust against `./out`, `out/`, and other
/// equivalent spellings without depending on the user creating the
/// directory first.
fn paths_overlap(src: &Path, out: &Path) -> bool {
    let Ok(src_abs) = fs::canonicalize(src) else {
        return src == out;
    };
    let out_abs = canonicalize_into_missing(out);
    src_abs.starts_with(&out_abs) || out_abs.starts_with(&src_abs)
}

/// Canonicalize `path` even if its tail doesn't exist yet by walking up
/// to the first ancestor that does, canonicalizing that, and re-joining
/// the unresolved tail. Falls back to `path` verbatim if no ancestor
/// resolves (extremely unusual; happens on truly absent roots).
fn canonicalize_into_missing(path: &Path) -> PathBuf {
    let mut existing: Option<PathBuf> = None;
    let mut tail: Vec<&std::ffi::OsStr> = Vec::new();
    for ancestor in path.ancestors() {
        if let Ok(canon) = fs::canonicalize(ancestor) {
            existing = Some(canon);
            break;
        }
        if let Some(name) = ancestor.file_name() {
            tail.push(name);
        }
    }
    let mut base = existing.unwrap_or_else(|| path.to_path_buf());
    for name in tail.iter().rev() {
        base.push(name);
    }
    base
}

/// Repack each patched CPK back into a new `.cpk` file at `<out>/<name>`.
fn write_repack(cpks: &[(String, Cpk)], out: &Path) -> Result<()> {
    for (name, cpk) in cpks {
        let dest = out.join(name);
        eprintln!("serializing {} → {}", name, dest.display());
        let bytes = cpk
            .to_bytes()
            .with_context(|| format!("serializing CPK {name}"))?;
        fs::write(&dest, &bytes).with_context(|| format!("writing {}", dest.display()))?;
    }
    Ok(())
}

/// One file that was overwritten because two input CPKs shipped the same
/// CPK-relative path. Last-CPK-wins is the resolved policy; the earlier
/// CPK's bytes are gone but recorded here so the user can review the run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ExtractCollision {
    /// CPK-relative path of the colliding file (forward-slash, what the
    /// game's runtime would see — independent of host path separator).
    pub(crate) path: String,
    /// The CPK whose bytes ended up on disk for this path.
    pub(crate) kept_from_cpk: String,
    /// The CPK whose bytes were overwritten.
    pub(crate) replaced_from_cpk: String,
}

/// Write every input CPK's contents into one merged tree under `out`,
/// with last-CPK-in-argv winning when two CPKs ship the same path. Each
/// collision is emitted to stderr and returned for the report file.
fn write_extract(cpks: &[(String, Cpk)], out: &Path) -> Result<Vec<ExtractCollision>> {
    // `written` maps the final on-disk path to the CPK that put it there;
    // a second hit at the same path is a collision, and the later write
    // overwrites the earlier one (per policy).
    let mut written: HashMap<PathBuf, String> = HashMap::new();
    let mut collisions: Vec<ExtractCollision> = Vec::new();

    for (name, cpk) in cpks {
        eprintln!("extracting {} → {}", name, out.display());
        for file in &cpk.files {
            let dest = extract_dest(out, &file.dir_name, &file.file_name)?;
            let rel = cpk_relative_path(&file.dir_name, &file.file_name);
            if let Some(prev) = written.get(&dest) {
                eprintln!("WARN: {rel} overwritten by {name} (was from {prev})");
                collisions.push(ExtractCollision {
                    path: rel,
                    kept_from_cpk: name.clone(),
                    replaced_from_cpk: prev.clone(),
                });
            }
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&dest, &file.data).with_context(|| format!("writing {}", dest.display()))?;
            written.insert(dest, name.clone());
        }
    }
    Ok(collisions)
}

/// Compute the on-disk destination of a CPK file under `out`. Split out
/// so we can unit-test path assembly without touching the filesystem.
///
/// Rejects CPK-supplied components that would escape `out`: a crafted archive
/// can carry `dir_name = "../../x"` or an absolute `file_name` (`/etc/x`,
/// `C:\x`) — with `PathBuf` the latter *replaces* the whole destination — so
/// this mirrors the pack-side `split_path` guard (reject `.` / `..` / `\`
/// segments and a separator-bearing or empty `file_name`).
fn extract_dest(out: &Path, dir_name: &str, file_name: &str) -> Result<PathBuf> {
    let mut dest = out.to_path_buf();
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

/// Forward-slash CPK-relative path. Stable across host OSes (so the
/// report file is portable and matches what the game runtime expects).
fn cpk_relative_path(dir_name: &str, file_name: &str) -> String {
    if dir_name.is_empty() {
        file_name.to_string()
    } else {
        format!("{dir_name}/{file_name}")
    }
}

/// Render a one-line-per-CPK human summary of a completed [`PatchReport`].
fn summarize_report(report: &PatchReport) -> String {
    let mut by_cpk: HashMap<&str, usize> = HashMap::new();
    for d in &report.drift {
        *by_cpk.entry(d.cpk.as_str()).or_default() += 1;
    }
    let mut s = format!(
        "STX: applied {} (already-translated {}), skipped {}, drift {}, missing {}\n\
         Font: glyphs added {}, glyphs changed {}, glyphs removed {}, \
         atlas writes {}, atlas grows {}, atlas replaces {}",
        report.applied,
        report.already_translated,
        report.skipped,
        report.drift.len(),
        report.missing.len(),
        report.font_glyphs_added,
        report.font_glyphs_changed,
        report.font_glyphs_removed,
        report.font_atlas_writes,
        report.font_atlas_grows,
        report.font_atlas_replaces,
    );
    if !by_cpk.is_empty() {
        s.push_str("  (drift by CPK:");
        for (cpk, n) in &by_cpk {
            let _ = write!(s, " {cpk}={n}");
        }
        s.push(')');
    }
    s
}

#[derive(Serialize)]
struct ReportJson<'a> {
    applied: usize,
    already_translated: usize,
    skipped: usize,
    drift: Vec<DriftJson<'a>>,
    missing: Vec<MissingJson<'a>>,
    /// Files overwritten in extract mode when two input CPKs shipped the
    /// same path. Empty in repack mode.
    extract_collisions: &'a [ExtractCollision],
    font_glyphs_added: usize,
    font_glyphs_changed: usize,
    font_glyphs_removed: usize,
    font_atlas_writes: usize,
    font_atlas_grows: usize,
    font_atlas_replaces: usize,
}

#[derive(Serialize)]
struct DriftJson<'a> {
    cpk: &'a str,
    cpk_path: &'a str,
    spc_member: &'a str,
    table: u32,
    index: u32,
    on_disk_source: &'a str,
    json_source: &'a str,
    applied: bool,
}

#[derive(Serialize)]
struct MissingJson<'a> {
    cpk: &'a str,
    cpk_path: &'a str,
    spc_member: &'a str,
    slot: Option<(u32, u32)>,
}

impl<'a> ReportJson<'a> {
    fn build(r: &'a PatchReport, extract_collisions: &'a [ExtractCollision]) -> Self {
        Self {
            applied: r.applied,
            already_translated: r.already_translated,
            skipped: r.skipped,
            drift: r
                .drift
                .iter()
                .map(|d| DriftJson {
                    cpk: &d.cpk,
                    cpk_path: &d.cpk_path,
                    spc_member: &d.spc_member,
                    table: d.table,
                    index: d.index,
                    on_disk_source: &d.on_disk_source,
                    json_source: &d.json_source,
                    applied: d.applied,
                })
                .collect(),
            missing: r
                .missing
                .iter()
                .map(|m| MissingJson {
                    cpk: &m.cpk,
                    cpk_path: &m.cpk_path,
                    spc_member: &m.spc_member,
                    slot: m.slot,
                })
                .collect(),
            extract_collisions,
            font_glyphs_added: r.font_glyphs_added,
            font_glyphs_changed: r.font_glyphs_changed,
            font_glyphs_removed: r.font_glyphs_removed,
            font_atlas_writes: r.font_atlas_writes,
            font_atlas_grows: r.font_atlas_grows,
            font_atlas_replaces: r.font_atlas_replaces,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real-data smoke test for font-atlas growth. Runs the full
    /// JSON-load → `apply` path against shipped CPKs **in memory** (no
    /// multi-GB output write) and checks the report. Ignored by default;
    /// point it at game data with:
    ///
    /// ```sh
    /// DRV3_SMOKE_JSON=…/fonts.json \
    /// DRV3_SMOKE_CPK_DATA=…/partition_data_win_us.cpk \
    /// DRV3_SMOKE_CPK_RESIDENT=…/partition_resident_win.cpk \
    ///   cargo test -p drv3-translate-cli -- --ignored font_atlas_growth
    /// ```
    #[test]
    #[ignore = "real-data smoke; set DRV3_SMOKE_JSON / DRV3_SMOKE_CPK_DATA / DRV3_SMOKE_CPK_RESIDENT"]
    fn font_atlas_growth_real_data_smoke() {
        let (Ok(json), Ok(data_cpk), Ok(resident_cpk)) = (
            std::env::var("DRV3_SMOKE_JSON"),
            std::env::var("DRV3_SMOKE_CPK_DATA"),
            std::env::var("DRV3_SMOKE_CPK_RESIDENT"),
        ) else {
            eprintln!(
                "skipping: set DRV3_SMOKE_JSON / DRV3_SMOKE_CPK_DATA / DRV3_SMOKE_CPK_RESIDENT"
            );
            return;
        };

        let doc = load_doc(Path::new(&json)).unwrap();
        let set = merge_docs(vec![doc]).unwrap();

        // Memory-map and parse the CPKs (reads only — apply mutates in
        // memory and we never serialize the multi-GB result back out).
        let mappings: Vec<(String, memmap2::Mmap)> = [data_cpk, resident_cpk]
            .into_iter()
            .map(|p| {
                let path = PathBuf::from(&p);
                let name = path.file_name().unwrap().to_str().unwrap().to_string();
                (name, mmap_file(&path).unwrap())
            })
            .collect();
        let mut cpks: Vec<(String, Cpk)> = mappings
            .iter()
            .map(|(n, m)| (n.clone(), Cpk::parse(m).unwrap()))
            .collect();

        let report = {
            let mut view: Vec<(&str, &mut Cpk)> =
                cpks.iter_mut().map(|(n, c)| (n.as_str(), c)).collect();
            apply(&mut view, &set, &PatchOptions::default()).unwrap()
        };

        eprintln!(
            "smoke report: atlas_grows={} atlas_replaces={} atlas_writes={} \
             glyphs_added={} glyphs_changed={} glyphs_removed={} missing={}",
            report.font_atlas_grows,
            report.font_atlas_replaces,
            report.font_atlas_writes,
            report.font_glyphs_added,
            report.font_glyphs_changed,
            report.font_glyphs_removed,
            report.missing.len(),
        );
        assert!(
            report.font_atlas_grows > 0,
            "expected at least one font atlas to grow"
        );
        assert!(report.font_atlas_writes > 0, "expected atlas pixel writes");
        assert!(
            report.missing.is_empty(),
            "unexpected missing targets: {:?}",
            report.missing
        );
    }

    #[test]
    fn extract_dest_flattens_no_per_cpk_layer() {
        let out = Path::new("/tmp/out");
        // Typical case: directory + filename.
        assert_eq!(
            extract_dest(out, "wrd_script/003", "foo.spc").unwrap(),
            PathBuf::from("/tmp/out/wrd_script/003/foo.spc"),
        );
        // Root-level file (no directory).
        assert_eq!(
            extract_dest(out, "", "manifest.bin").unwrap(),
            PathBuf::from("/tmp/out/manifest.bin"),
        );
        // Filters out leading/trailing/double slashes in dir_name.
        assert_eq!(
            extract_dest(out, "/a//b/", "c.dat").unwrap(),
            PathBuf::from("/tmp/out/a/b/c.dat"),
        );
    }

    #[test]
    fn extract_dest_rejects_escapes() {
        let out = Path::new("/tmp/out");
        assert!(extract_dest(out, "../../etc", "passwd").is_err());
        assert!(extract_dest(out, "a/../b", "c").is_err());
        assert!(extract_dest(out, "", "/etc/passwd").is_err());
        assert!(extract_dest(out, "", "C:\\windows\\x").is_err());
        assert!(extract_dest(out, "", "sub/evil").is_err());
        assert!(extract_dest(out, "a\\b", "c").is_err());
    }

    #[test]
    fn cpk_relative_path_uses_forward_slashes_regardless_of_host() {
        assert_eq!(cpk_relative_path("a/b", "c.spc"), "a/b/c.spc");
        assert_eq!(cpk_relative_path("", "c.spc"), "c.spc");
    }

    #[test]
    fn write_extract_records_last_wins_collision() {
        // Two synthetic CPKs sharing a path; second wins and the first
        // shows up as `replaced_from_cpk` in the recorded collision.
        use drv3_cpk::CpkFile;
        use drv3_cpk::utf::UtfRow;
        use indexmap::IndexMap;

        let tmp = tempfile::tempdir().unwrap();

        let mk_cpk = |bytes: &[u8]| Cpk {
            header_row: UtfRow::default(),
            header_columns: Vec::new(),
            toc_columns: Cpk::default_toc_columns(),
            files: vec![CpkFile {
                dir_name: "dir".into(),
                file_name: "shared.bin".into(),
                id: 0,
                user_string: String::new(),
                extra: IndexMap::new(),
                data: bytes.to_vec().into(),
            }],
            etoc_packet: None,
            itoc_packet: None,
            gtoc_packet: None,
        };

        let cpks = vec![
            ("first.cpk".to_string(), mk_cpk(b"FIRST")),
            ("second.cpk".to_string(), mk_cpk(b"SECOND")),
        ];

        let collisions = write_extract(&cpks, tmp.path()).unwrap();
        assert_eq!(collisions.len(), 1);
        assert_eq!(collisions[0].path, "dir/shared.bin");
        assert_eq!(collisions[0].kept_from_cpk, "second.cpk");
        assert_eq!(collisions[0].replaced_from_cpk, "first.cpk");

        // On-disk bytes are from the second CPK (last-wins).
        let on_disk = fs::read(tmp.path().join("dir").join("shared.bin")).unwrap();
        assert_eq!(on_disk, b"SECOND");
    }
}
