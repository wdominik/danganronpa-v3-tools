//! Patch engine: apply a [`TranslationSet`] to one or more parsed CPKs.
//!
//! The engine groups entries by `(cpk, cpk_path, spc_member)` so each SPC
//! is parsed and serialized exactly once. Inside an STX, slot lookup goes
//! through a transient `HashMap<u32, usize>` keyed by [`StxEntry::id`].
//!
//! [`StxEntry::id`]: drv3_stx::StxEntry::id

use std::collections::{HashMap, HashSet};

use drv3_cpk::{Cpk, CpkFile};
use drv3_spc::{Spc, SpcEntry};
use drv3_stx::Stx;
use rayon::prelude::*;

use crate::error::TranslateError;
use crate::model::{FileFormat, StxFileGroup, TranslationFileGroup, TranslationSet};
use crate::options::{DriftPolicy, PatchOptions};
use crate::report::{DriftRecord, MissingRecord, PatchReport};

/// Apply `set` to one or more parsed CPKs in memory.
///
/// `cpks` is a slice of `(name, &mut Cpk)` pairs where `name` is the
/// filename the translation entries reference. Each file group is routed
/// to the CPK whose name matches its `cpk` field; entries that name a
/// CPK not in the slice produce [`TranslateError::CpkNotFound`] eagerly
/// (silent-skip would let half a translation set vanish unnoticed).
///
/// For the single-CPK case, pass a one-element slice:
///
/// ```no_run
/// # use drv3_cpk::Cpk;
/// # use drv3_translate::{PatchOptions, TranslationSet, apply};
/// # let mut cpk: Cpk = unimplemented!();
/// # let set: TranslationSet = unimplemented!();
/// let report = apply(
///     &mut [("partition_data_win_us.cpk", &mut cpk)],
///     &set,
///     &PatchOptions::default(),
/// )?;
/// # Ok::<(), drv3_translate::TranslateError>(())
/// ```
///
/// Per-CPK work runs sequentially in caller order; per-SPC parallelism
/// inside a CPK is controlled by [`PatchOptions::parallel`].
///
/// # Errors
///
/// Returns [`TranslateError`] for the first hard failure (parse error,
/// missing CPK, or drift under [`DriftPolicy::Error`]). Soft issues
/// (missing slots, drift under warn/skip policies) go into the
/// [`PatchReport`] instead.
pub fn apply(
    cpks: &mut [(&str, &mut Cpk)],
    set: &TranslationSet,
    opts: &PatchOptions,
) -> Result<PatchReport, TranslateError> {
    // Index file groups by CPK name so we visit each CPK at most once.
    let mut by_cpk: HashMap<&str, Vec<&TranslationFileGroup>> = HashMap::new();
    for group in &set.files {
        by_cpk.entry(group.cpk.as_str()).or_default().push(group);
    }

    // Surface unknown CPK names eagerly — failing fast beats silently
    // dropping half the translation set.
    let known: HashSet<&str> = cpks.iter().map(|(n, _)| *n).collect();
    for cpk_name in by_cpk.keys() {
        if !known.contains(cpk_name) {
            return Err(TranslateError::CpkNotFound((*cpk_name).to_string()));
        }
    }

    let mut report = PatchReport::default();
    // Re-borrow each CPK in input order so the report's drift/missing
    // sequences are reproducible regardless of HashMap iteration order.
    for (cpk_name, cpk) in cpks.iter_mut() {
        let Some(groups) = by_cpk.get(cpk_name) else {
            continue;
        };
        let sub = apply_to_cpk(cpk, cpk_name, groups, opts)?;
        report.extend(sub);
    }
    Ok(report)
}

fn apply_to_cpk(
    cpk: &mut Cpk,
    cpk_name: &str,
    groups: &[&TranslationFileGroup],
    opts: &PatchOptions,
) -> Result<PatchReport, TranslateError> {
    // Build a (dir_name, file_name) → index map once per CPK so the
    // O(files × groups) linear scan collapses to O(files + groups).
    let mut file_index: HashMap<(&str, &str), usize> = HashMap::with_capacity(cpk.files.len());
    for (idx, file) in cpk.files.iter().enumerate() {
        file_index.insert((file.dir_name.as_str(), file.file_name.as_str()), idx);
    }

    // Group by cpk_path so each SPC is parsed/serialized once.
    let mut by_path: HashMap<&str, Vec<&TranslationFileGroup>> = HashMap::new();
    for g in groups {
        by_path.entry(g.cpk_path.as_str()).or_default().push(*g);
    }

    // Resolve which CPK file each cpk_path maps to (or record as missing),
    // then dispatch the actual SPC patching either sequentially or in
    // parallel via rayon.
    //
    // We resolve indices up front so the parallel path can split the
    // `cpk.files: Vec<CpkFile>` slice via disjoint mutable references.
    let mut work: Vec<(usize, &str, Vec<&TranslationFileGroup>)> = Vec::new();
    let mut missing = Vec::<MissingRecord>::new();
    for (cpk_path, groups) in by_path {
        let key = split_cpk_path(cpk_path);
        if let Some(&idx) = file_index.get(&key) {
            work.push((idx, cpk_path, groups));
        } else {
            for g in groups {
                missing.push(MissingRecord {
                    cpk: cpk_name.to_string(),
                    cpk_path: g.cpk_path.clone(),
                    spc_member: String::new(),
                    slot: None,
                });
            }
        }
    }

    // Build disjoint mutable handles into cpk.files so the parallel path
    // can safely process each entry in its own thread. The trick is to
    // split the underlying slice into single-element subslices indexed
    // by the work tuples we just resolved.
    let file_slots: Vec<&mut CpkFile> = collect_disjoint_mut(&mut cpk.files, &work);

    let sub_reports: Vec<Result<PatchReport, TranslateError>> = if opts.parallel {
        file_slots
            .into_par_iter()
            .zip(work.par_iter())
            .map(|(file_slot, (_, cpk_path, groups))| {
                patch_spc(file_slot, cpk_name, cpk_path, groups, opts)
            })
            .collect()
    } else {
        file_slots
            .into_iter()
            .zip(work.iter())
            .map(|(file_slot, (_, cpk_path, groups))| {
                patch_spc(file_slot, cpk_name, cpk_path, groups, opts)
            })
            .collect()
    };

    let mut report = PatchReport {
        missing,
        ..PatchReport::default()
    };
    for sub in sub_reports {
        report.extend(sub?);
    }
    Ok(report)
}

/// Hand out one `&mut T` per index in `work`, returned in the same order
/// as `work`. Indices must be unique and in-bounds — both are guaranteed
/// by the caller (each `work` index comes from a `HashMap<cpk_path, …>`
/// lookup into `cpk.files`).
fn collect_disjoint_mut<'a, T>(
    slice: &'a mut [T],
    work: &[(usize, &str, Vec<&TranslationFileGroup>)],
) -> Vec<&'a mut T> {
    // Map every requested slice index to the position it should occupy
    // in the output. One pass over the slice fills the output in place,
    // preserving the caller's order.
    let mut output_pos: HashMap<usize, usize> = HashMap::with_capacity(work.len());
    for (i, (slice_idx, _, _)) in work.iter().enumerate() {
        output_pos.insert(*slice_idx, i);
    }
    let mut out: Vec<Option<&mut T>> = (0..work.len()).map(|_| None).collect();
    for (i, item) in slice.iter_mut().enumerate() {
        if let Some(&pos) = output_pos.get(&i) {
            out[pos] = Some(item);
        }
    }
    out.into_iter()
        .map(|o| o.expect("every work entry resolved to a slice index"))
        .collect()
}

/// Patch a single SPC: parse, walk each member, mutate each STX, serialize.
fn patch_spc(
    cpk_file: &mut CpkFile,
    cpk_name: &str,
    cpk_path: &str,
    groups: &[&TranslationFileGroup],
    opts: &PatchOptions,
) -> Result<PatchReport, TranslateError> {
    let mut spc = Spc::parse(&cpk_file.data).map_err(|e| TranslateError::Spc {
        cpk_path: cpk_path.to_string(),
        source: e,
    })?;

    let mut member_index: HashMap<String, usize> = HashMap::with_capacity(spc.entries.len());
    for (idx, entry) in spc.entries.iter().enumerate() {
        if let Some(name) = entry.name_as_str() {
            member_index.insert(name.to_string(), idx);
        }
    }

    let mut report = PatchReport::default();
    for group in groups {
        let Some(&member_idx) = member_index.get(&group.spc_member) else {
            report.missing.push(MissingRecord {
                cpk: cpk_name.to_string(),
                cpk_path: cpk_path.to_string(),
                spc_member: group.spc_member.clone(),
                slot: None,
            });
            continue;
        };
        match &group.format {
            FileFormat::Stx(stx_group) => {
                patch_stx_member(
                    &mut spc.entries[member_idx],
                    cpk_name,
                    cpk_path,
                    &group.spc_member,
                    stx_group,
                    opts,
                    &mut report,
                )?;
            }
            FileFormat::Font(font_group) => {
                crate::engine_font::patch_font_member(
                    &mut spc,
                    member_idx,
                    cpk_path,
                    &group.spc_member,
                    font_group,
                    &mut report,
                )?;
            }
        }
    }

    cpk_file.data = spc.to_bytes().map_err(|e| TranslateError::Spc {
        cpk_path: cpk_path.to_string(),
        source: e,
    })?;
    Ok(report)
}

fn patch_stx_member(
    spc_entry: &mut SpcEntry,
    cpk_name: &str,
    cpk_path: &str,
    spc_member: &str,
    group: &StxFileGroup,
    opts: &PatchOptions,
    report: &mut PatchReport,
) -> Result<(), TranslateError> {
    let mut stx = Stx::parse(&spc_entry.data).map_err(|e| TranslateError::Stx {
        cpk_path: cpk_path.to_string(),
        spc_member: spc_member.to_string(),
        source: e,
    })?;

    // Build per-table id → entry-index maps lazily, only for tables we
    // actually touch. Most STX files use a single table, so this is
    // typically one tiny HashMap allocation.
    let mut id_indices: HashMap<u32, HashMap<u32, usize>> = HashMap::new();

    for patch in &group.entries {
        let table_idx = patch.table as usize;
        let Some(table) = stx.tables.get_mut(table_idx) else {
            report.missing.push(MissingRecord {
                cpk: cpk_name.to_string(),
                cpk_path: cpk_path.to_string(),
                spc_member: spc_member.to_string(),
                slot: Some((patch.table, patch.index)),
            });
            continue;
        };

        let id_map = id_indices.entry(patch.table).or_insert_with(|| {
            table
                .entries
                .iter()
                .enumerate()
                .map(|(i, e)| (e.id, i))
                .collect()
        });

        let Some(&entry_idx) = id_map.get(&patch.index) else {
            report.missing.push(MissingRecord {
                cpk: cpk_name.to_string(),
                cpk_path: cpk_path.to_string(),
                spc_member: spc_member.to_string(),
                slot: Some((patch.table, patch.index)),
            });
            continue;
        };

        let on_disk_source = &table.entries[entry_idx].text;
        if on_disk_source != &patch.source {
            match opts.on_drift {
                DriftPolicy::Error => {
                    return Err(TranslateError::Drift {
                        cpk_path: cpk_path.to_string(),
                        spc_member: spc_member.to_string(),
                        table: patch.table,
                        index: patch.index,
                        on_disk_source: on_disk_source.clone(),
                        json_source: patch.source.clone(),
                    });
                }
                DriftPolicy::Skip => {
                    report.drift.push(DriftRecord {
                        cpk: cpk_name.to_string(),
                        cpk_path: cpk_path.to_string(),
                        spc_member: spc_member.to_string(),
                        table: patch.table,
                        index: patch.index,
                        on_disk_source: on_disk_source.clone(),
                        json_source: patch.source.clone(),
                        applied: false,
                    });
                    report.skipped += 1;
                    continue;
                }
                DriftPolicy::WarnAndApply => {
                    report.drift.push(DriftRecord {
                        cpk: cpk_name.to_string(),
                        cpk_path: cpk_path.to_string(),
                        spc_member: spc_member.to_string(),
                        table: patch.table,
                        index: patch.index,
                        on_disk_source: on_disk_source.clone(),
                        json_source: patch.source.clone(),
                        applied: true,
                    });
                }
            }
        }

        if table.entries[entry_idx].text == patch.target {
            report.already_translated += 1;
        }
        table.entries[entry_idx].text.clone_from(&patch.target);
        report.applied += 1;
    }

    spc_entry.data = stx.to_bytes();
    Ok(())
}

/// Split `"foo/bar/baz.spc"` into `("foo/bar", "baz.spc")`. A path with
/// no `/` returns `("", path)` to match `CpkFile { dir_name: "", file_name: ... }`.
fn split_cpk_path(path: &str) -> (&str, &str) {
    match path.rfind('/') {
        Some(i) => (&path[..i], &path[i + 1..]),
        None => ("", path),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{StxEntryPatch, StxFileGroup};
    use drv3_stx::{Stx, StxEntry, StxTable};

    fn synth_stx() -> Vec<u8> {
        let stx = Stx {
            tables: vec![StxTable {
                unknown: 0x08,
                entries: vec![
                    StxEntry {
                        id: 0,
                        text: "Hello".into(),
                    },
                    StxEntry {
                        id: 1,
                        text: "World".into(),
                    },
                    StxEntry {
                        id: 7,
                        text: "Sparse ID".into(),
                    },
                ],
            }],
        };
        stx.to_bytes()
    }

    fn make_patch(table: u32, index: u32, src: &str, tgt: &str) -> StxEntryPatch {
        StxEntryPatch {
            table,
            index,
            source: src.into(),
            target: tgt.into(),
        }
    }

    #[test]
    fn split_cpk_path_handles_paths_with_and_without_slashes() {
        assert_eq!(split_cpk_path("a/b/c.spc"), ("a/b", "c.spc"));
        assert_eq!(split_cpk_path("c.spc"), ("", "c.spc"));
        assert_eq!(split_cpk_path("a/c.spc"), ("a", "c.spc"));
    }

    #[test]
    fn patch_stx_member_replaces_text_by_id_not_position() {
        let mut entry = SpcEntry {
            name: b"x.stx".to_vec(),
            compression_flag: drv3_spc::COMPRESSION_STORED,
            unknown_flag: 0,
            data: synth_stx(),
        };
        let group = StxFileGroup {
            entries: vec![
                make_patch(0, 7, "Sparse ID", "Replaced"),
                make_patch(0, 1, "World", "Welt"),
            ],
        };
        let mut report = PatchReport::default();
        patch_stx_member(
            &mut entry,
            "cpk",
            "path",
            "x.stx",
            &group,
            &PatchOptions::default(),
            &mut report,
        )
        .unwrap();

        let stx = Stx::parse(&entry.data).unwrap();
        let entries = &stx.tables[0].entries;
        assert_eq!(entries[0].text, "Hello"); // unchanged
        assert_eq!(entries[1].text, "Welt"); // replaced by id=1
        assert_eq!(entries[2].text, "Replaced"); // replaced by id=7 even though it's the 3rd slot
        assert_eq!(report.applied, 2);
        assert!(report.drift.is_empty());
        assert!(report.missing.is_empty());
    }

    #[test]
    fn drift_warn_records_and_applies() {
        let mut entry = SpcEntry {
            name: b"x.stx".to_vec(),
            compression_flag: drv3_spc::COMPRESSION_STORED,
            unknown_flag: 0,
            data: synth_stx(),
        };
        let group = StxFileGroup {
            entries: vec![make_patch(0, 0, "Different source", "Hallo")],
        };
        let mut report = PatchReport::default();
        patch_stx_member(
            &mut entry,
            "cpk",
            "path",
            "x.stx",
            &group,
            &PatchOptions {
                on_drift: DriftPolicy::WarnAndApply,
                ..PatchOptions::default()
            },
            &mut report,
        )
        .unwrap();

        let stx = Stx::parse(&entry.data).unwrap();
        assert_eq!(stx.tables[0].entries[0].text, "Hallo");
        assert_eq!(report.applied, 1);
        assert_eq!(report.drift.len(), 1);
        assert!(report.drift[0].applied);
        assert_eq!(report.skipped, 0);
    }

    #[test]
    fn drift_skip_records_and_skips() {
        let mut entry = SpcEntry {
            name: b"x.stx".to_vec(),
            compression_flag: drv3_spc::COMPRESSION_STORED,
            unknown_flag: 0,
            data: synth_stx(),
        };
        let group = StxFileGroup {
            entries: vec![make_patch(0, 0, "Different source", "Hallo")],
        };
        let mut report = PatchReport::default();
        patch_stx_member(
            &mut entry,
            "cpk",
            "path",
            "x.stx",
            &group,
            &PatchOptions {
                on_drift: DriftPolicy::Skip,
                ..PatchOptions::default()
            },
            &mut report,
        )
        .unwrap();

        let stx = Stx::parse(&entry.data).unwrap();
        assert_eq!(stx.tables[0].entries[0].text, "Hello");
        assert_eq!(report.applied, 0);
        assert_eq!(report.drift.len(), 1);
        assert!(!report.drift[0].applied);
        assert_eq!(report.skipped, 1);
    }

    #[test]
    fn drift_error_aborts() {
        let mut entry = SpcEntry {
            name: b"x.stx".to_vec(),
            compression_flag: drv3_spc::COMPRESSION_STORED,
            unknown_flag: 0,
            data: synth_stx(),
        };
        let group = StxFileGroup {
            entries: vec![make_patch(0, 0, "Different source", "Hallo")],
        };
        let mut report = PatchReport::default();
        let err = patch_stx_member(
            &mut entry,
            "cpk",
            "path",
            "x.stx",
            &group,
            &PatchOptions {
                on_drift: DriftPolicy::Error,
                ..PatchOptions::default()
            },
            &mut report,
        )
        .unwrap_err();
        assert!(matches!(err, TranslateError::Drift { .. }));
    }

    #[test]
    fn missing_slot_recorded_when_id_absent() {
        let mut entry = SpcEntry {
            name: b"x.stx".to_vec(),
            compression_flag: drv3_spc::COMPRESSION_STORED,
            unknown_flag: 0,
            data: synth_stx(),
        };
        let group = StxFileGroup {
            entries: vec![make_patch(0, 999, "doesn't matter", "n/a")],
        };
        let mut report = PatchReport::default();
        patch_stx_member(
            &mut entry,
            "cpk",
            "path",
            "x.stx",
            &group,
            &PatchOptions::default(),
            &mut report,
        )
        .unwrap();
        assert_eq!(report.applied, 0);
        assert_eq!(report.missing.len(), 1);
        assert_eq!(report.missing[0].slot, Some((0, 999)));
    }
}
