//! `drv3-translate validate` — read-only pre-flight: schema, dedup, drift.

use std::collections::HashMap;
use std::process::ExitCode;

use anyhow::{Context, Result};
use drv3_cpk::Cpk;
use drv3_translate::{DriftPolicy, PatchOptions, apply};

use crate::ValidateArgs;
use drv3_dto_patch::{load_doc, merge_docs};

/// How many drift / missing records to list before summarizing the remainder.
const PREVIEW_LIMIT: usize = 10;

/// Run the `validate` pre-flight: load the JSONs, and (if `--cpk` is given)
/// dry-run the engine to surface drift and missing slots. Returns a nonzero
/// [`ExitCode`] when any drift or missing slot is found.
pub(crate) fn run(args: &ValidateArgs) -> Result<ExitCode> {
    eprintln!("loading {} JSON file(s)…", args.json.len());
    let mut docs = Vec::with_capacity(args.json.len());
    for path in &args.json {
        docs.push(load_doc(path)?);
    }
    let set = merge_docs(docs)?;
    eprintln!("schema OK, {} file group(s) parsed", set.files.len());

    summarize_parsed_set(&set);

    if args.cpk.is_empty() {
        eprintln!("(no --cpk supplied — skipping drift check)");
        return Ok(ExitCode::SUCCESS);
    }

    // Drift check by dry-running through the engine in Skip mode against
    // clones of the parsed CPKs. We don't write anything out, but the
    // engine's machinery surfaces every drift and missing slot.
    eprintln!("opening {} CPK(s) for drift check…", args.cpk.len());
    // Map every CPK up front and keep the mappings alive: parsed CPKs borrow
    // their file bodies (zero-copy) from these maps, so the maps must outlive
    // `owned`.
    let mut mappings: Vec<(String, memmap2::Mmap)> = Vec::with_capacity(args.cpk.len());
    for path in &args.cpk {
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .map(str::to_string)
            .ok_or_else(|| anyhow::anyhow!("CPK path {} has no filename", path.display()))?;
        mappings.push((name, crate::mmap_file(path)?));
    }
    let mut owned: Vec<(String, Cpk)> = Vec::with_capacity(mappings.len());
    for (name, mmap) in &mappings {
        let cpk = Cpk::parse(mmap).with_context(|| format!("parsing CPK {name}"))?;
        owned.push((name.clone(), cpk));
    }

    let mut view: Vec<(&str, &mut Cpk)> = owned.iter_mut().map(|(n, c)| (n.as_str(), c)).collect();
    let report = apply(
        &mut view,
        &set,
        &PatchOptions {
            on_drift: DriftPolicy::Skip,
            parallel: false,
        },
    )?;

    eprintln!(
        "STX: would apply {} target(s); drift events: {}; missing: {}",
        report.applied + report.skipped,
        report.drift.len(),
        report.missing.len(),
    );
    eprintln!(
        "Font: {} glyphs added, {} changed, {} removed, {} atlas writes, {} atlas replaces",
        report.font_glyphs_added,
        report.font_glyphs_changed,
        report.font_glyphs_removed,
        report.font_atlas_writes,
        report.font_atlas_replaces,
    );
    if !report.drift.is_empty() {
        for d in report.drift.iter().take(PREVIEW_LIMIT) {
            eprintln!(
                "  drift @ {}::{}::{} t={} i={}",
                d.cpk, d.cpk_path, d.spc_member, d.table, d.index
            );
        }
        if report.drift.len() > PREVIEW_LIMIT {
            eprintln!("  …and {} more", report.drift.len() - PREVIEW_LIMIT);
        }
    }
    if !report.missing.is_empty() {
        for m in report.missing.iter().take(PREVIEW_LIMIT) {
            eprintln!(
                "  missing @ {}::{}::{} slot={:?}",
                m.cpk, m.cpk_path, m.spc_member, m.slot
            );
        }
        if report.missing.len() > PREVIEW_LIMIT {
            eprintln!("  …and {} more", report.missing.len() - PREVIEW_LIMIT);
        }
    }

    // Fail as a pre-flight check: any drift or missing slot makes `validate`
    // exit nonzero so it's usable in scripts. Details were already printed
    // above; this is just the verdict + exit code.
    if report.drift.is_empty() && report.missing.is_empty() {
        Ok(ExitCode::SUCCESS)
    } else {
        eprintln!(
            "validation failed: {} drift event(s), {} missing slot(s)",
            report.drift.len(),
            report.missing.len(),
        );
        Ok(ExitCode::from(1))
    }
}

/// Print the per-CPK breakdown of a parsed translation set.
///
/// Font groups are split by patch mode: a `replace` discards the shipped
/// glyph table and atlas pixels wholesale, which a producer wants to see
/// *before* running `apply`, not after.
fn summarize_parsed_set(set: &drv3_translate::TranslationSet) {
    let mut stx_entry_count = 0usize;
    let mut font_glyph_count = 0usize;
    let mut replace_group_count = 0usize;
    // (stx groups, merge font groups, replace font groups) per CPK.
    let mut by_cpk: HashMap<&str, (usize, usize, usize)> = HashMap::new();
    for fg in &set.files {
        let bucket = by_cpk.entry(fg.cpk.as_str()).or_default();
        match &fg.format {
            drv3_translate::FileFormat::Stx(s) => {
                stx_entry_count += s.entries.len();
                bucket.0 += 1;
            }
            drv3_translate::FileFormat::Font(f) => {
                font_glyph_count += f.glyphs.len();
                match f.mode {
                    drv3_translate::FontPatchMode::Merge { .. } => bucket.1 += 1,
                    drv3_translate::FontPatchMode::Replace { .. } => {
                        bucket.2 += 1;
                        replace_group_count += 1;
                    }
                }
            }
            _ => {}
        }
    }
    eprintln!("{stx_entry_count} STX entries, {font_glyph_count} font glyphs across file groups");
    if replace_group_count > 0 {
        eprintln!(
            "  {replace_group_count} font group(s) run in replace mode \
             (shipped glyphs and atlas pixels discarded)"
        );
    }
    for (cpk, (stx, merge, replace)) in &by_cpk {
        eprintln!("  {cpk}: {stx} stx groups, {merge} font groups (merge), {replace} (replace)");
    }
}
