//! End-to-end test for the `drv3-cli cpk extract` + `drv3-cli cpk pack` round-trip.
//!
//! Builds a small synthetic CPK in-memory, writes it to a tempfile, drives the
//! CLI binary to extract it into a temp directory, then packs the temp dir
//! back into a new CPK. The repacked CPK must parse cleanly and carry the
//! same files (paths, IDs, user strings, bodies) as the source.

use std::fs;
use std::process::Command;

use drv3_cpk::{Cpk, CpkFile, StorageFlag, UtfColumn, UtfRow, UtfType, UtfValue};
use indexmap::IndexMap;

fn drv3_binary() -> String {
    // Set by Cargo when running integration tests.
    env!("CARGO_BIN_EXE_drv3-cli").to_string()
}

#[expect(
    clippy::too_many_lines,
    reason = "one cohesive synthetic CPK fixture builder"
)]
fn synthetic_cpk() -> Cpk {
    let mut header_row = UtfRow::new();
    header_row.insert(
        "UpdateDateTime".into(),
        UtfValue::U64(0x1122_3344_5566_7788),
    );
    header_row.insert("ContentOffset".into(), UtfValue::U64(0));
    header_row.insert("ContentSize".into(), UtfValue::U64(0));
    header_row.insert("TocOffset".into(), UtfValue::U64(0));
    header_row.insert("TocSize".into(), UtfValue::U64(0));
    header_row.insert("EtocOffset".into(), UtfValue::U64(0));
    header_row.insert("EtocSize".into(), UtfValue::U64(0));
    header_row.insert("ItocOffset".into(), UtfValue::U64(0));
    header_row.insert("ItocSize".into(), UtfValue::U64(0));
    header_row.insert("GtocOffset".into(), UtfValue::U64(0));
    header_row.insert("GtocSize".into(), UtfValue::U64(0));
    header_row.insert("Files".into(), UtfValue::U32(0));
    header_row.insert("Align".into(), UtfValue::U16(0x800));
    header_row.insert("Sorted".into(), UtfValue::U16(1));
    header_row.insert("Version".into(), UtfValue::U16(7));
    header_row.insert("Revision".into(), UtfValue::U16(14));

    let header_columns = vec![
        UtfColumn {
            name: Some("UpdateDateTime".into()),
            storage: StorageFlag::PerRow,
            ty: UtfType::U64,
            constant: None,
        },
        UtfColumn {
            name: Some("ContentOffset".into()),
            storage: StorageFlag::PerRow,
            ty: UtfType::U64,
            constant: None,
        },
        UtfColumn {
            name: Some("ContentSize".into()),
            storage: StorageFlag::PerRow,
            ty: UtfType::U64,
            constant: None,
        },
        UtfColumn {
            name: Some("TocOffset".into()),
            storage: StorageFlag::PerRow,
            ty: UtfType::U64,
            constant: None,
        },
        UtfColumn {
            name: Some("TocSize".into()),
            storage: StorageFlag::PerRow,
            ty: UtfType::U64,
            constant: None,
        },
        UtfColumn {
            name: Some("EtocOffset".into()),
            storage: StorageFlag::PerRow,
            ty: UtfType::U64,
            constant: None,
        },
        UtfColumn {
            name: Some("EtocSize".into()),
            storage: StorageFlag::PerRow,
            ty: UtfType::U64,
            constant: None,
        },
        UtfColumn {
            name: Some("ItocOffset".into()),
            storage: StorageFlag::Zero,
            ty: UtfType::U64,
            constant: None,
        },
        UtfColumn {
            name: Some("ItocSize".into()),
            storage: StorageFlag::Zero,
            ty: UtfType::U64,
            constant: None,
        },
        UtfColumn {
            name: Some("GtocOffset".into()),
            storage: StorageFlag::Zero,
            ty: UtfType::U64,
            constant: None,
        },
        UtfColumn {
            name: Some("GtocSize".into()),
            storage: StorageFlag::Zero,
            ty: UtfType::U64,
            constant: None,
        },
        UtfColumn {
            name: Some("Files".into()),
            storage: StorageFlag::PerRow,
            ty: UtfType::U32,
            constant: None,
        },
        UtfColumn {
            name: Some("Align".into()),
            storage: StorageFlag::PerRow,
            ty: UtfType::U16,
            constant: None,
        },
        UtfColumn {
            name: Some("Sorted".into()),
            storage: StorageFlag::PerRow,
            ty: UtfType::U16,
            constant: None,
        },
        UtfColumn {
            name: Some("Version".into()),
            storage: StorageFlag::PerRow,
            ty: UtfType::U16,
            constant: None,
        },
        UtfColumn {
            name: Some("Revision".into()),
            storage: StorageFlag::PerRow,
            ty: UtfType::U16,
            constant: None,
        },
    ];

    Cpk {
        header_row,
        header_columns,
        toc_columns: Cpk::default_toc_columns(),
        files: vec![
            CpkFile {
                dir_name: String::new(),
                file_name: "root_file.bin".into(),
                id: 0,
                user_string: "root".into(),
                extra: IndexMap::new(),
                data: b"<<root contents 0>>".to_vec(),
            },
            CpkFile {
                dir_name: "boot".into(),
                file_name: "startup.spc".into(),
                id: 1,
                user_string: "boot/startup".into(),
                extra: IndexMap::new(),
                data: vec![0x42; 137],
            },
            CpkFile {
                dir_name: "game/sub".into(),
                file_name: "deep.dat".into(),
                id: 2,
                user_string: String::new(),
                extra: IndexMap::new(),
                data: (0..200).map(|i| (i ^ (i >> 3)) as u8).collect(),
            },
        ],
        etoc_packet: None,
        itoc_packet: None,
        gtoc_packet: None,
    }
}

#[test]
fn extract_pack_round_trip_via_cli() {
    let original = synthetic_cpk();
    let original_bytes = original.to_bytes().expect("synthetic CPK serializes");

    let workdir = tempfile::tempdir().expect("tempdir");
    let src_cpk = workdir.path().join("source.cpk");
    let extract_dir = workdir.path().join("extracted");
    let repacked_cpk = workdir.path().join("repacked.cpk");
    fs::write(&src_cpk, &original_bytes).unwrap();

    let drv3 = drv3_binary();

    // 1. Extract via the CLI.
    let extract_status = Command::new(&drv3)
        .args(["cpk", "extract"])
        .arg(&src_cpk)
        .arg(&extract_dir)
        .status()
        .expect("run drv3-cli cpk extract");
    assert!(extract_status.success(), "extract failed");

    // 2. Manifest + every file body must be on disk.
    assert!(extract_dir.join("manifest.json").is_file());
    assert!(extract_dir.join("root_file.bin").is_file());
    assert!(extract_dir.join("boot/startup.spc").is_file());
    assert!(extract_dir.join("game/sub/deep.dat").is_file());

    // 3. Pack the extracted dir back.
    let pack_status = Command::new(&drv3)
        .args(["cpk", "pack"])
        .arg(&extract_dir)
        .arg(&repacked_cpk)
        .status()
        .expect("run drv3-cli cpk pack");
    assert!(pack_status.success(), "pack failed");

    // 4. Re-parse the repacked CPK and check semantic equality.
    let repacked_bytes = fs::read(&repacked_cpk).expect("read repacked");
    let reparsed = Cpk::parse(&repacked_bytes).expect("repacked CPK parses");

    assert_eq!(reparsed.files.len(), original.files.len());
    for (got, want) in reparsed.files.iter().zip(original.files.iter()) {
        assert_eq!(got.dir_name, want.dir_name);
        assert_eq!(got.file_name, want.file_name);
        assert_eq!(got.id, want.id);
        assert_eq!(got.user_string, want.user_string);
        assert_eq!(
            got.data, want.data,
            "file body mismatch for {}",
            want.file_name
        );
    }

    // 5. The preserved header fields the writer doesn't touch must survive.
    for key in &["UpdateDateTime", "Align", "Sorted", "Version", "Revision"] {
        assert_eq!(
            reparsed.header_row.get(*key),
            original.header_row.get(*key),
            "header field {key} not preserved"
        );
    }
}

#[test]
fn manifest_json_is_valid_and_versioned() {
    let original = synthetic_cpk();
    let original_bytes = original.to_bytes().unwrap();

    let workdir = tempfile::tempdir().unwrap();
    let src_cpk = workdir.path().join("source.cpk");
    let extract_dir = workdir.path().join("extracted");
    fs::write(&src_cpk, &original_bytes).unwrap();

    let status = Command::new(drv3_binary())
        .args(["cpk", "extract"])
        .arg(&src_cpk)
        .arg(&extract_dir)
        .status()
        .unwrap();
    assert!(status.success());

    let manifest_str = fs::read_to_string(extract_dir.join("manifest.json")).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&manifest_str).unwrap();
    assert_eq!(parsed["version"], 1);
    assert_eq!(parsed["files"].as_array().unwrap().len(), 3);
    assert_eq!(parsed["header"]["name"], "CpkHeader");
    // The first column carries a real schema entry.
    assert!(parsed["header"]["columns"][0]["name"].is_string());
    assert!(parsed["header"]["columns"][0]["storage"].is_string());
    assert!(parsed["header"]["columns"][0]["type"].is_string());

    // toc_columns must be present and structurally valid.
    let toc_cols = parsed["toc_columns"]
        .as_array()
        .expect("manifest must serialize toc_columns as an array");
    assert!(!toc_cols.is_empty(), "toc_columns must not be empty");
    assert!(toc_cols[0]["name"].is_string());
    assert!(toc_cols[0]["storage"].is_string());
    assert!(toc_cols[0]["type"].is_string());
}
