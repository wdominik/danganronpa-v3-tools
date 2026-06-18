//! End-to-end test: synthesize a CPK containing an SPC containing two
//! STX files, apply a translation set, round-trip back through
//! Cpk/Spc/Stx parsers, and assert the patched slots match.
//!
//! This test stays in-memory — no fixture files — so it's safe to run on
//! every CI invocation. Real-data smoke tests against shipped CPKs live
//! behind `#[ignore]` in the CLI crate.

use drv3_cpk::utf::UtfRow;
use drv3_cpk::{Cpk, CpkFile};
use drv3_spc::{COMPRESSION_STORED, Spc, SpcEntry};
use drv3_stx::{Stx, StxEntry, StxTable};
use drv3_translate::{
    DriftPolicy, FileFormat, PatchOptions, StxEntryPatch, StxFileGroup, TranslationFileGroup,
    TranslationSet, apply,
};
use indexmap::IndexMap;

fn synth_stx(strings: &[(u32, &str)]) -> Vec<u8> {
    let stx = Stx {
        tables: vec![StxTable {
            unknown: 8,
            entries: strings
                .iter()
                .map(|&(id, text)| StxEntry {
                    id,
                    text: text.into(),
                })
                .collect(),
        }],
    };
    stx.to_bytes()
}

fn synth_spc(members: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let spc = Spc {
        unknown1: [0u8; 0x24],
        unknown2: 0,
        entries: members
            .iter()
            .map(|(name, data)| SpcEntry {
                name: name.as_bytes().to_vec(),
                compression_flag: COMPRESSION_STORED,
                unknown_flag: 0,
                data: data.clone(),
            })
            .collect(),
    };
    spc.to_bytes().unwrap()
}

fn synth_cpk(files: Vec<(&str, &str, Vec<u8>)>) -> Cpk<'static> {
    Cpk {
        header_row: UtfRow::default(),
        header_columns: Vec::new(),
        toc_columns: Cpk::default_toc_columns(),
        files: files
            .into_iter()
            .enumerate()
            .map(|(i, (dir, name, data))| CpkFile {
                dir_name: dir.into(),
                file_name: name.into(),
                id: i as u32,
                user_string: String::new(),
                extra: IndexMap::new(),
                data: data.into(),
            })
            .collect(),
        etoc_packet: None,
        itoc_packet: None,
        gtoc_packet: None,
    }
}

fn read_slot(cpk: &Cpk, cpk_path: &str, spc_member: &str, table: u32, id: u32) -> String {
    let (dir, name) = cpk_path.rsplit_once('/').unwrap_or(("", cpk_path));
    let file = cpk
        .files
        .iter()
        .find(|f| f.dir_name == dir && f.file_name == name)
        .expect("cpk file");
    let spc = Spc::parse(&file.data).unwrap();
    let entry = spc
        .entries
        .iter()
        .find(|e| e.name_as_str() == Some(spc_member))
        .expect("spc member");
    let stx = Stx::parse(&entry.data).unwrap();
    stx.tables[table as usize]
        .entries
        .iter()
        .find(|e| e.id == id)
        .expect("stx entry")
        .text
        .clone()
}

#[test]
fn apply_to_multiple_cpks_keeps_unrelated_files_intact() {
    // Two SPCs, three STX members, spread across two CPKs.
    let stx_a = synth_stx(&[(0, "Hello"), (1, "Goodbye"), (5, "Other")]);
    let stx_b = synth_stx(&[(0, "Apple"), (1, "Banana")]);
    let stx_c = synth_stx(&[(0, "Untouched1"), (1, "Untouched2")]);

    let spc1 = synth_spc(&[("a.stx", stx_a.clone()), ("b.stx", stx_b.clone())]);
    let spc2 = synth_spc(&[("c.stx", stx_c.clone())]);

    let mut cpk_data = synth_cpk(vec![("dir1", "scene.spc", spc1)]);
    let mut cpk_resident = synth_cpk(vec![("sys", "resident.spc", spc2)]);

    let set = TranslationSet {
        source_language: "en".into(),
        target_language: "de".into(),
        files: vec![
            TranslationFileGroup {
                cpk: "data.cpk".into(),
                cpk_path: "dir1/scene.spc".into(),
                spc_member: "a.stx".into(),
                format: FileFormat::Stx(StxFileGroup {
                    entries: vec![
                        StxEntryPatch {
                            table: 0,
                            index: 0,
                            source: "Hello".into(),
                            target: "Hallo".into(),
                        },
                        StxEntryPatch {
                            table: 0,
                            index: 5,
                            source: "Other".into(),
                            target: "Andere".into(),
                        },
                    ],
                }),
            },
            TranslationFileGroup {
                cpk: "data.cpk".into(),
                cpk_path: "dir1/scene.spc".into(),
                spc_member: "b.stx".into(),
                format: FileFormat::Stx(StxFileGroup {
                    entries: vec![StxEntryPatch {
                        table: 0,
                        index: 1,
                        source: "Banana".into(),
                        target: "Banane".into(),
                    }],
                }),
            },
            TranslationFileGroup {
                cpk: "resident.cpk".into(),
                cpk_path: "sys/resident.spc".into(),
                spc_member: "c.stx".into(),
                format: FileFormat::Stx(StxFileGroup {
                    entries: vec![StxEntryPatch {
                        table: 0,
                        index: 0,
                        source: "Untouched1".into(),
                        target: "Berührt1".into(),
                    }],
                }),
            },
        ],
    };

    let mut cpks: Vec<(&str, &mut Cpk)> = vec![
        ("data.cpk", &mut cpk_data),
        ("resident.cpk", &mut cpk_resident),
    ];
    let report = apply(&mut cpks, &set, &PatchOptions::default()).unwrap();
    assert_eq!(report.applied, 4);
    assert!(report.drift.is_empty());
    assert!(report.missing.is_empty());

    // Patched slots.
    assert_eq!(
        read_slot(&cpk_data, "dir1/scene.spc", "a.stx", 0, 0),
        "Hallo"
    );
    assert_eq!(
        read_slot(&cpk_data, "dir1/scene.spc", "a.stx", 0, 5),
        "Andere"
    );
    assert_eq!(
        read_slot(&cpk_data, "dir1/scene.spc", "b.stx", 0, 1),
        "Banane"
    );
    assert_eq!(
        read_slot(&cpk_resident, "sys/resident.spc", "c.stx", 0, 0),
        "Berührt1"
    );

    // Untouched slots in the same files.
    assert_eq!(
        read_slot(&cpk_data, "dir1/scene.spc", "a.stx", 0, 1),
        "Goodbye"
    );
    assert_eq!(
        read_slot(&cpk_data, "dir1/scene.spc", "b.stx", 0, 0),
        "Apple"
    );
    assert_eq!(
        read_slot(&cpk_resident, "sys/resident.spc", "c.stx", 0, 1),
        "Untouched2"
    );
}

#[test]
fn apply_errors_when_set_references_unsupplied_cpk() {
    let mut cpk = synth_cpk(vec![]);
    let set = TranslationSet {
        source_language: "en".into(),
        target_language: "de".into(),
        files: vec![TranslationFileGroup {
            cpk: "missing.cpk".into(),
            cpk_path: "x/y.spc".into(),
            spc_member: "z.stx".into(),
            format: FileFormat::Stx(StxFileGroup { entries: vec![] }),
        }],
    };
    let mut cpks: Vec<(&str, &mut Cpk)> = vec![("present.cpk", &mut cpk)];
    let err = apply(&mut cpks, &set, &PatchOptions::default()).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("missing.cpk"), "unexpected error: {msg}");
}

#[test]
fn parallel_dispatch_produces_same_results_as_sequential() {
    // Build a CPK with several SPCs so the rayon path actually has work to split.
    let stxs: Vec<Vec<u8>> = (0..6)
        .map(|i| synth_stx(&[(0, "before"), (1, &format!("x{i}"))]))
        .collect();
    let spcs: Vec<Vec<u8>> = stxs
        .iter()
        .enumerate()
        .map(|(i, s)| synth_spc(&[(&format!("m{i}.stx"), s.clone())]))
        .collect();
    // Own the formatted scene names so `files` can borrow them — no leaking.
    let names: Vec<String> = (0..6).map(|i| format!("scene{i}.spc")).collect();
    let files: Vec<(&str, &str, Vec<u8>)> = spcs
        .iter()
        .zip(&names)
        .map(|(s, name)| ("dir", name.as_str(), s.clone()))
        .collect();

    let make_set = || TranslationSet {
        source_language: "en".into(),
        target_language: "de".into(),
        files: (0..6)
            .map(|i| TranslationFileGroup {
                cpk: "data.cpk".into(),
                cpk_path: format!("dir/scene{i}.spc"),
                spc_member: format!("m{i}.stx"),
                format: FileFormat::Stx(StxFileGroup {
                    entries: vec![StxEntryPatch {
                        table: 0,
                        index: 0,
                        source: "before".into(),
                        target: format!("after{i}"),
                    }],
                }),
            })
            .collect(),
    };

    let mut seq = synth_cpk(files.clone());
    let mut par = synth_cpk(files);
    let _ = apply(
        &mut [("data.cpk", &mut seq)],
        &make_set(),
        &PatchOptions {
            on_drift: DriftPolicy::WarnAndApply,
            parallel: false,
        },
    )
    .unwrap();
    let _ = apply(
        &mut [("data.cpk", &mut par)],
        &make_set(),
        &PatchOptions {
            on_drift: DriftPolicy::WarnAndApply,
            parallel: true,
        },
    )
    .unwrap();
    // CPK data blobs must match byte-for-byte regardless of dispatch mode.
    assert_eq!(seq.files.len(), par.files.len());
    for (a, b) in seq.files.iter().zip(par.files.iter()) {
        assert_eq!(a.file_name, b.file_name);
        assert_eq!(a.data, b.data, "mismatch in {}", a.file_name);
    }
}
