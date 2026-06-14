//! End-to-end test for `drv3-cli spc extract` + `spc pack` round-trip:
//! the SPC archive-level `unknown1` / `unknown2`, per-entry
//! `compression_flag` / `unknown_flag`, and on-disk entry order must all
//! survive a synthetic-build → extract → pack → re-parse cycle.

use std::fs;
use std::process::Command;

use drv3_spc::{COMPRESSION_LZSS, COMPRESSION_STORED, Spc, SpcEntry};

fn drv3_binary() -> String {
    env!("CARGO_BIN_EXE_drv3-cli").to_string()
}

fn synthetic_spc() -> Spc {
    Spc {
        // Recognisable byte pattern so a regression shows up cleanly in
        // hex dumps (and a 0x24-byte length confirms the `unknown1` slot).
        unknown1: {
            let mut a = [0u8; 0x24];
            a[0] = 0xCA;
            a[1] = 0xFE;
            a[2] = 0xBA;
            a[3] = 0xBE;
            a[0x23] = 0xFF;
            a
        },
        unknown2: 0xDEAD_BEEF,
        // Entries deliberately not in alphabetical order so the order-
        // preservation behavior is meaningful.
        entries: vec![
            SpcEntry {
                name: b"zeta.bin".to_vec(),
                compression_flag: COMPRESSION_STORED,
                unknown_flag: 7,
                data: b"first body, stored".to_vec(),
            },
            SpcEntry {
                name: b"alpha.bin".to_vec(),
                compression_flag: COMPRESSION_LZSS,
                unknown_flag: 0,
                data: vec![0x42; 256], // long enough that LZSS picks up repetition
            },
            SpcEntry {
                name: b"middle.bin".to_vec(),
                compression_flag: COMPRESSION_STORED,
                unknown_flag: -1,
                data: b"third body".to_vec(),
            },
        ],
    }
}

#[test]
fn extract_pack_round_trip_preserves_metadata() {
    let original = synthetic_spc();
    let bytes = original.to_bytes().expect("serialize synthetic SPC");

    let workdir = tempfile::tempdir().expect("tempdir");
    let src = workdir.path().join("source.spc");
    let extract_dir = workdir.path().join("extracted");
    let repacked = workdir.path().join("repacked.spc");
    fs::write(&src, &bytes).unwrap();

    let drv3 = drv3_binary();

    // 1. Extract via the CLI.
    let status = Command::new(&drv3)
        .args(["spc", "extract"])
        .arg(&src)
        .arg(&extract_dir)
        .status()
        .expect("run drv3-cli spc extract");
    assert!(status.success(), "extract failed");

    // 2. Manifest + every body must be on disk.
    assert!(extract_dir.join("manifest.json").is_file());
    for entry in &original.entries {
        let name = std::str::from_utf8(&entry.name).unwrap();
        assert!(extract_dir.join(name).is_file(), "missing extracted {name}");
    }

    // 3. Pack the extracted dir back.
    let status = Command::new(&drv3)
        .args(["spc", "pack"])
        .arg(&extract_dir)
        .arg(&repacked)
        .status()
        .expect("run drv3-cli spc pack");
    assert!(status.success(), "pack failed");

    // 4. Re-parse the repacked SPC and check that every field round-tripped.
    let repacked_bytes = fs::read(&repacked).expect("read repack");
    let reparsed = Spc::parse(&repacked_bytes).expect("repack parses");

    assert_eq!(reparsed.unknown1, original.unknown1, "unknown1 changed");
    assert_eq!(reparsed.unknown2, original.unknown2, "unknown2 changed");
    assert_eq!(
        reparsed.entries.len(),
        original.entries.len(),
        "entry count changed",
    );
    for (got, want) in reparsed.entries.iter().zip(original.entries.iter()) {
        assert_eq!(got.name, want.name, "entry name / order changed");
        assert_eq!(
            got.compression_flag,
            want.compression_flag,
            "compression_flag for {:?} changed",
            std::str::from_utf8(&want.name).unwrap_or("<non-utf8>"),
        );
        assert_eq!(
            got.unknown_flag,
            want.unknown_flag,
            "unknown_flag for {:?} changed",
            std::str::from_utf8(&want.name).unwrap_or("<non-utf8>"),
        );
        // Decompressed body must round-trip byte-equal (LZSS-encoded bytes
        // need not match the original, but `Spc::parse` decompresses on
        // read, so the post-parse view is comparable).
        assert_eq!(
            got.data,
            want.data,
            "body for {:?} differs",
            std::str::from_utf8(&want.name).unwrap_or("<non-utf8>"),
        );
    }
}

#[test]
fn manifest_json_shape_is_stable() {
    let original = synthetic_spc();
    let bytes = original.to_bytes().unwrap();

    let workdir = tempfile::tempdir().unwrap();
    let src = workdir.path().join("source.spc");
    let extract_dir = workdir.path().join("extracted");
    fs::write(&src, &bytes).unwrap();

    let status = Command::new(drv3_binary())
        .args(["spc", "extract"])
        .arg(&src)
        .arg(&extract_dir)
        .status()
        .unwrap();
    assert!(status.success());

    let manifest_str = fs::read_to_string(extract_dir.join("manifest.json")).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&manifest_str).unwrap();

    // Pre-1.0: schema lives at version 1.
    assert_eq!(parsed["version"], 1);
    // `unknown1` is hex-encoded 36 bytes = 72 hex chars.
    assert_eq!(parsed["unknown1"].as_str().unwrap().len(), 0x24 * 2);
    assert_eq!(parsed["entries"].as_array().unwrap().len(), 3);
    // First entry preserves the original (non-alphabetical) order.
    assert_eq!(parsed["entries"][0]["name"], "zeta.bin");
    // Compression flag is rendered as readable strings.
    assert_eq!(parsed["entries"][0]["compression"], "stored");
    assert_eq!(parsed["entries"][1]["compression"], "lzss");
}
