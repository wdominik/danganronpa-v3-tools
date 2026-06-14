//! Real-data regression tests using captured bytes from
//! `gamedata/partition_resident_win.cpk`.
//!
//! These pin the canonical CRI `@UTF` storage-flag encoding — any
//! regression that re-introduces the wrong `0x1 = Constant` mapping
//! fails here without needing the 2.3 GB game file on disk.

use drv3_cpk::utf::{StorageFlag, UtfTable, UtfType};

/// 848 bytes of `@UTF` table extracted from `partition_resident_win.cpk` at
/// CPK offset 0x10 (immediately after the 16-byte `CPK ` packet wrapper).
/// Captured 2026-05-15 from the US Windows shipped CPK.
const CPK_HEADER_UTF: &[u8] = include_bytes!("fixtures/cpk_header_utf.bin");

#[test]
fn cpk_header_table_parses() {
    let table = UtfTable::parse(CPK_HEADER_UTF).expect("CPK header @UTF must parse");

    assert_eq!(table.name, "CpkHeader");
    assert_eq!(table.columns.len(), 44);
    assert_eq!(table.rows.len(), 1);
}

#[test]
fn cpk_header_columns_match_canonical_encoding() {
    // The CPK header schema as observed in DR V3's resident partition. Storage
    // flags are the *canonical* CRI values.
    let expected: &[(&str, StorageFlag, UtfType)] = &[
        ("UpdateDateTime", StorageFlag::PerRow, UtfType::U64),
        ("FileSize", StorageFlag::Zero, UtfType::U64),
        ("ContentOffset", StorageFlag::PerRow, UtfType::U64),
        ("ContentSize", StorageFlag::PerRow, UtfType::U64),
        ("TocOffset", StorageFlag::PerRow, UtfType::U64),
        ("TocSize", StorageFlag::PerRow, UtfType::U64),
        ("TocCrc", StorageFlag::Zero, UtfType::U32),
        ("HtocOffset", StorageFlag::Zero, UtfType::U64),
        ("HtocSize", StorageFlag::Zero, UtfType::U64),
        ("EtocOffset", StorageFlag::PerRow, UtfType::U64),
        ("EtocSize", StorageFlag::PerRow, UtfType::U64),
        ("ItocOffset", StorageFlag::Zero, UtfType::U64),
        ("ItocSize", StorageFlag::Zero, UtfType::U64),
        ("ItocCrc", StorageFlag::Zero, UtfType::U32),
    ];

    let table = UtfTable::parse(CPK_HEADER_UTF).unwrap();
    for (idx, (name, storage, ty)) in expected.iter().enumerate() {
        let col = &table.columns[idx];
        assert_eq!(col.name.as_deref(), Some(*name), "column {idx} name");
        assert_eq!(col.storage, *storage, "column {idx} ({name}) storage");
        assert_eq!(col.ty, *ty, "column {idx} ({name}) type");
    }
}

#[test]
fn cpk_header_row_carries_real_layout_fields() {
    let table = UtfTable::parse(CPK_HEADER_UTF).unwrap();
    let row = &table.rows[0];

    // ContentOffset and TocOffset must be reasonable absolute byte positions.
    // ContentOffset is observed to be 0x80000000 = 2 GiB in this CPK (the
    // content blob is past the first 2 GiB section — large but valid for a
    // 2.3 GB archive). TocOffset is the second packet position.
    let content_offset = row
        .get("ContentOffset")
        .and_then(drv3_cpk::UtfValue::as_u64)
        .expect("ContentOffset must be present");
    let toc_offset = row
        .get("TocOffset")
        .and_then(drv3_cpk::UtfValue::as_u64)
        .expect("TocOffset must be present");
    assert!(content_offset > 0, "ContentOffset must be non-zero");
    assert!(toc_offset > 0, "TocOffset must be non-zero");
    assert!(
        toc_offset < content_offset || content_offset == 0x2000,
        "TocOffset {toc_offset:#x} should precede ContentOffset {content_offset:#x}"
    );
}
