//! `drv3-translate validate` — read-only pre-flight: schema, dedup, drift.

use std::collections::HashMap;

use anyhow::{Context, Result};
use drv3_cpk::Cpk;
use drv3_translate::{DriftPolicy, PatchOptions, apply};

use crate::ValidateArgs;
use crate::dto::{load_doc, merge_docs};

pub(crate) fn run(args: &ValidateArgs) -> Result<()> {
    eprintln!("loading {} JSON file(s)…", args.json.len());
    let mut docs = Vec::with_capacity(args.json.len());
    for path in &args.json {
        docs.push(load_doc(path)?);
    }
    let set = merge_docs(docs)?;
    eprintln!("schema OK, {} file group(s) parsed", set.files.len());

    let mut stx_entry_count = 0usize;
    let mut font_glyph_count = 0usize;
    let mut by_cpk: HashMap<&str, (usize, usize)> = HashMap::new(); // (stx, font) group counts
    for fg in &set.files {
        let bucket = by_cpk.entry(fg.cpk.as_str()).or_default();
        match &fg.format {
            drv3_translate::FileFormat::Stx(s) => {
                stx_entry_count += s.entries.len();
                bucket.0 += 1;
            }
            drv3_translate::FileFormat::Font(f) => {
                font_glyph_count += f.glyphs.len();
                bucket.1 += 1;
            }
            _ => {}
        }
    }
    eprintln!("{stx_entry_count} STX entries, {font_glyph_count} font glyphs across file groups");
    for (cpk, (stx, font)) in &by_cpk {
        eprintln!("  {cpk}: {stx} stx groups, {font} font groups");
    }

    if args.cpk.is_empty() {
        eprintln!("(no --cpk supplied — skipping drift check)");
        return Ok(());
    }

    // Drift check by dry-running through the engine in Skip mode against
    // clones of the parsed CPKs. We don't write anything out, but the
    // engine's machinery surfaces every drift and missing slot.
    eprintln!("opening {} CPK(s) for drift check…", args.cpk.len());
    let mut owned: Vec<(String, Cpk)> = Vec::with_capacity(args.cpk.len());
    for path in &args.cpk {
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .map(str::to_string)
            .ok_or_else(|| anyhow::anyhow!("CPK path {} has no filename", path.display()))?;
        let mmap = crate::mmap_file(path)?;
        let cpk = Cpk::parse(&mmap).with_context(|| format!("parsing CPK {name}"))?;
        owned.push((name, cpk));
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
        "Font: {} glyphs added, {} changed, {} atlas writes",
        report.font_glyphs_added, report.font_glyphs_changed, report.font_atlas_writes,
    );
    if !report.drift.is_empty() {
        for d in report.drift.iter().take(10) {
            eprintln!(
                "  drift @ {}::{}::{} t={} i={}",
                d.cpk, d.cpk_path, d.spc_member, d.table, d.index
            );
        }
        if report.drift.len() > 10 {
            eprintln!("  …and {} more", report.drift.len() - 10);
        }
    }
    if !report.missing.is_empty() {
        for m in report.missing.iter().take(10) {
            eprintln!(
                "  missing @ {}::{}::{} slot={:?}",
                m.cpk, m.cpk_path, m.spc_member, m.slot
            );
        }
        if report.missing.len() > 10 {
            eprintln!("  …and {} more", report.missing.len() - 10);
        }
    }
    Ok(())
}
