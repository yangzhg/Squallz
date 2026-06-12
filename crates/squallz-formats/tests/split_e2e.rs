//! End-to-end split-volume tests (`.001` byte-split semantics): create
//! through the engine, reopen through any volume, detect missing volumes.

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use common::{engine, TempDir};
use squallz_core::api::{
    ControlToken, CreateOptions, ExtractOptions, FormatError, NoProgress, OpenOptions,
};

/// Deterministic pseudo-random (incompressible-ish) payload.
fn payload(len: usize) -> Vec<u8> {
    let mut state = 0x1234_5678u32;
    (0..len)
        .map(|_| {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (state >> 24) as u8
        })
        .collect()
}

/// Creates `data.bin` (100 KB) under `dir` and returns its path.
fn sample_input(dir: &Path) -> PathBuf {
    sample_input_with_len(dir, 100 * 1024)
}

fn sample_input_with_len(dir: &Path, len: usize) -> PathBuf {
    let input = dir.join("data.bin");
    fs::write(&input, payload(len)).unwrap();
    input
}

fn split_archive(dir: &Path, volume_size: u64) -> PathBuf {
    let input = sample_input(dir);
    let dest = dir.join("out.zip");
    let opts = CreateOptions {
        split_size: Some(volume_size),
        ..CreateOptions::default()
    };
    engine()
        .create(&dest, &[input], &opts, &NoProgress, &ControlToken::new())
        .unwrap();
    dest
}

fn volume_paths(dir: &Path, base: &str) -> Vec<PathBuf> {
    let mut paths: Vec<_> = fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .and_then(|name| name.strip_prefix(base))
                .is_some_and(|suffix| suffix.chars().all(|ch| ch.is_ascii_digit()))
        })
        .collect();
    paths.sort();
    paths
}

fn sqz_recovery_volume_path(dir: &Path, index: usize) -> PathBuf {
    dir.join(format!("out.sqz.rev{index:03}"))
}

fn corrupt_sqzr_header(path: &Path) {
    let mut bytes = fs::read(path).unwrap();
    assert!(bytes.len() >= 64);
    assert_eq!(&bytes[0..4], b"SQZR");
    bytes[12] ^= 0x5A;
    fs::write(path, bytes).unwrap();
}

fn corrupt_sqzr_payload_byte(path: &Path, physical_offset: usize) {
    let mut bytes = fs::read(path).unwrap();
    let payload_offset = 64 + physical_offset;
    assert!(payload_offset < bytes.len());
    assert_eq!(&bytes[0..4], b"SQZR");
    bytes[payload_offset] ^= 0xA5;
    fs::write(path, bytes).unwrap();
}

fn assert_open_fails_with_corrupt_archive(path: &Path, expected: &str) {
    let err = engine().list(path, &OpenOptions::default()).unwrap_err();
    match err {
        FormatError::CorruptArchive(detail) => {
            assert!(
                detail.contains(expected),
                "expected {expected:?} in detail: {detail}"
            );
        }
        other => panic!("expected CorruptArchive, got {other:?}"),
    }
}

fn assert_sqzv_header(bytes: &[u8], index: u32, total: u32) -> (u64, u64) {
    assert!(bytes.len() >= 32);
    assert_eq!(&bytes[0..4], b"SQZV");
    assert_eq!(u32::from_le_bytes(bytes[4..8].try_into().unwrap()), index);
    assert_eq!(u32::from_le_bytes(bytes[8..12].try_into().unwrap()), total);
    let uuid = (
        u64::from_le_bytes(bytes[12..20].try_into().unwrap()),
        u64::from_le_bytes(bytes[20..28].try_into().unwrap()),
    );
    assert_eq!(
        u32::from_le_bytes(bytes[28..32].try_into().unwrap()),
        crc32c::crc32c(&bytes[..28])
    );
    uuid
}

fn assert_sqz_split_flag(bytes: &[u8]) {
    let sqz_start = 32;
    assert!(bytes.len() >= sqz_start + 64);
    assert_eq!(&bytes[sqz_start..sqz_start + 8], b"SQZARCH\x1A");
    let flags = u32::from_le_bytes(bytes[sqz_start + 12..sqz_start + 16].try_into().unwrap());
    assert_ne!(flags & (1 << 3), 0, "SQZ split flag must be set");
    assert_eq!(
        u32::from_le_bytes(bytes[sqz_start + 52..sqz_start + 56].try_into().unwrap()),
        crc32c::crc32c(&bytes[sqz_start..sqz_start + 52])
    );
}

fn assert_sqzr_header(
    bytes: &[u8],
    algorithm: u16,
    total: u32,
    uuid: (u64, u64),
    physical_volume_size: u64,
    tail_physical_len: u64,
) {
    assert!(bytes.len() >= 64);
    assert_eq!(&bytes[0..4], b"SQZR");
    assert_eq!(u16::from_le_bytes(bytes[4..6].try_into().unwrap()), 1);
    assert_eq!(
        u16::from_le_bytes(bytes[6..8].try_into().unwrap()),
        algorithm
    );
    assert_eq!(u32::from_le_bytes(bytes[8..12].try_into().unwrap()), total);
    assert_eq!(
        u64::from_le_bytes(bytes[12..20].try_into().unwrap()),
        uuid.0
    );
    assert_eq!(
        u64::from_le_bytes(bytes[20..28].try_into().unwrap()),
        uuid.1
    );
    assert_eq!(
        u64::from_le_bytes(bytes[28..36].try_into().unwrap()),
        physical_volume_size
    );
    assert_eq!(
        u64::from_le_bytes(bytes[36..44].try_into().unwrap()),
        tail_physical_len
    );
    assert_eq!(
        u64::from_le_bytes(bytes[44..52].try_into().unwrap()),
        physical_volume_size
    );
    assert_eq!(
        u32::from_le_bytes(bytes[52..56].try_into().unwrap()),
        crc32c::crc32c(&bytes[..52])
    );
    assert_eq!(bytes.len() as u64, 64 + physical_volume_size);
}

#[test]
fn split_create_produces_volumes_and_roundtrips() {
    let tmp = TempDir::new("split-roundtrip");
    let dest = split_archive(tmp.path(), 30 * 1024);

    // ~100 KB of incompressible data at 30 KB per volume → 4 volumes; the
    // unsplit file must not exist.
    assert!(!dest.exists());
    let volumes: Vec<PathBuf> = (1..=4)
        .map(|i| tmp.path().join(format!("out.zip.{i:03}")))
        .collect();
    for v in &volumes[..3] {
        assert!(v.is_file(), "{} missing", v.display());
        assert_eq!(fs::metadata(v).unwrap().len(), 30 * 1024);
    }
    assert!(volumes[3].is_file());
    assert!(!tmp.path().join("out.zip.005").exists());

    // list via the first volume.
    let entries = engine().list(&volumes[0], &OpenOptions::default()).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].path.display, "data.bin");

    // Opening a middle volume resolves the same set.
    let entries2 = engine().list(&volumes[2], &OpenOptions::default()).unwrap();
    assert_eq!(entries2.len(), 1);

    // extract via .001 and compare bytes.
    let out = tmp.path().join("extracted");
    engine()
        .extract(
            &volumes[0],
            &out,
            None,
            &OpenOptions::default(),
            &ExtractOptions::default(),
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap();
    assert_eq!(fs::read(out.join("data.bin")).unwrap(), payload(100 * 1024));

    // test passes too.
    let report = engine()
        .test(
            &volumes[0],
            &OpenOptions::default(),
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap();
    assert!(report.is_ok());
}

#[test]
fn missing_middle_volume_is_corrupt_with_detail() {
    let tmp = TempDir::new("split-missing");
    split_archive(tmp.path(), 30 * 1024);
    let missing = tmp.path().join("out.zip.002");
    fs::remove_file(&missing).unwrap();

    let err = engine()
        .list(&tmp.path().join("out.zip.001"), &OpenOptions::default())
        .unwrap_err();
    match err {
        FormatError::CorruptArchive(detail) => assert!(
            detail.contains("out.zip.002"),
            "detail must name the missing volume: {detail}"
        ),
        other => panic!("expected CorruptArchive, got {other:?}"),
    }
}

#[test]
fn split_works_for_compound_and_seven_z_formats() {
    let tmp = TempDir::new("split-formats");
    let input = sample_input(tmp.path());
    let ctl = ControlToken::new();
    for name in ["out.7z", "out.tar.gz"] {
        let dest = tmp.path().join(name);
        let opts = CreateOptions {
            split_size: Some(40 * 1024),
            ..CreateOptions::default()
        };
        engine()
            .create(
                &dest,
                std::slice::from_ref(&input),
                &opts,
                &NoProgress,
                &ctl,
            )
            .unwrap();
        let first = tmp.path().join(format!("{name}.001"));
        assert!(first.is_file(), "{name}: first volume missing");
        let out = tmp.path().join(format!("x-{name}"));
        engine()
            .extract(
                &first,
                &out,
                None,
                &OpenOptions::default(),
                &ExtractOptions::default(),
                &NoProgress,
                &ctl,
            )
            .unwrap();
        assert_eq!(
            fs::read(out.join("data.bin")).unwrap(),
            payload(100 * 1024),
            "{name}: extracted bytes differ"
        );
    }
}

#[test]
fn split_sqz_writes_sqzv_headers_and_roundtrips() {
    let tmp = TempDir::new("split-sqzv");
    let input = sample_input(tmp.path());
    let dest = tmp.path().join("out.sqz");
    let ctl = ControlToken::new();
    let opts = CreateOptions {
        split_size: Some(30 * 1024),
        ..CreateOptions::default()
    };
    engine()
        .create(&dest, &[input], &opts, &NoProgress, &ctl)
        .unwrap();

    assert!(!dest.exists());
    let volumes = volume_paths(tmp.path(), "out.sqz.");
    assert!(volumes.len() >= 4, "expected multiple SQZ volumes");
    let mut set_uuid = None;
    for (idx, volume) in volumes.iter().enumerate() {
        let bytes = fs::read(volume).unwrap();
        let uuid = assert_sqzv_header(&bytes, idx as u32 + 1, volumes.len() as u32);
        if idx == 0 {
            assert_sqz_split_flag(&bytes);
        }
        assert_ne!(uuid.1, 0, "SQZV uuid low word should come from SQZ header");
        if let Some(set_uuid) = set_uuid {
            assert_eq!(uuid, set_uuid, "all SQZV volumes share one container UUID");
        } else {
            set_uuid = Some(uuid);
        }
        if idx + 1 < volumes.len() {
            assert_eq!(bytes.len() as u64, 30 * 1024);
        }
    }
    let tail_mirror = sqz_recovery_volume_path(tmp.path(), volumes.len());
    let tail_bytes = fs::read(volumes.last().unwrap()).unwrap();
    let mirror_bytes = fs::read(&tail_mirror).unwrap();
    assert_eq!(
        mirror_bytes, tail_bytes,
        "tail recovery sidecar should be a validated mirror of the tail SQZV volume"
    );
    let parity = sqz_recovery_volume_path(tmp.path(), 1);
    let parity_bytes = fs::read(&parity).unwrap();
    assert_sqzr_header(
        &parity_bytes,
        1,
        volumes.len() as u32,
        set_uuid.expect("SQZV uuid captured"),
        30 * 1024,
        tail_bytes.len() as u64,
    );
    let weighted = sqz_recovery_volume_path(tmp.path(), 2);
    let weighted_bytes = fs::read(&weighted).unwrap();
    assert_sqzr_header(
        &weighted_bytes,
        2,
        volumes.len() as u32,
        set_uuid.expect("SQZV uuid captured"),
        30 * 1024,
        tail_bytes.len() as u64,
    );
    let quadratic = sqz_recovery_volume_path(tmp.path(), 3);
    let quadratic_bytes = fs::read(&quadratic).unwrap();
    assert_sqzr_header(
        &quadratic_bytes,
        3,
        volumes.len() as u32,
        set_uuid.expect("SQZV uuid captured"),
        30 * 1024,
        tail_bytes.len() as u64,
    );

    let entries = engine().list(&volumes[0], &OpenOptions::default()).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].path.display, "data.bin");

    let out = tmp.path().join("sqz-out");
    engine()
        .extract(
            &volumes[0],
            &out,
            None,
            &OpenOptions::default(),
            &ExtractOptions::default(),
            &NoProgress,
            &ctl,
        )
        .unwrap();
    assert_eq!(fs::read(out.join("data.bin")).unwrap(), payload(100 * 1024));
}

#[test]
fn corrupt_sqzv_header_is_reported() {
    let tmp = TempDir::new("split-sqzv-corrupt");
    let input = sample_input(tmp.path());
    let dest = tmp.path().join("out.sqz");
    let opts = CreateOptions {
        split_size: Some(40 * 1024),
        ..CreateOptions::default()
    };
    engine()
        .create(&dest, &[input], &opts, &NoProgress, &ControlToken::new())
        .unwrap();
    let first = tmp.path().join("out.sqz.001");
    let mut bytes = fs::read(&first).unwrap();
    bytes[12] ^= 0x7F;
    fs::write(&first, bytes).unwrap();

    let err = engine().list(&first, &OpenOptions::default()).unwrap_err();
    match err {
        FormatError::CorruptArchive(detail) => {
            assert!(detail.contains("SQZV"), "detail should name SQZV: {detail}");
        }
        other => panic!("expected CorruptArchive, got {other:?}"),
    }
}

#[test]
fn sqzv_uuid_mismatch_is_reported() {
    let tmp = TempDir::new("split-sqzv-uuid-mismatch");
    let input = sample_input(tmp.path());
    let dest = tmp.path().join("out.sqz");
    let opts = CreateOptions {
        split_size: Some(40 * 1024),
        ..CreateOptions::default()
    };
    engine()
        .create(&dest, &[input], &opts, &NoProgress, &ControlToken::new())
        .unwrap();

    let first = tmp.path().join("out.sqz.001");
    let second = tmp.path().join("out.sqz.002");
    let mut bytes = fs::read(&second).unwrap();
    bytes[20] ^= 0x5A;
    let crc = crc32c::crc32c(&bytes[..28]);
    bytes[28..32].copy_from_slice(&crc.to_le_bytes());
    fs::write(&second, bytes).unwrap();

    let err = engine().list(&first, &OpenOptions::default()).unwrap_err();
    match err {
        FormatError::CorruptArchive(detail) => {
            assert!(detail.contains("UUID"), "detail should name UUID: {detail}");
        }
        other => panic!("expected CorruptArchive, got {other:?}"),
    }
}

#[test]
fn missing_sqzv_payload_volume_recovers_when_within_rs_capacity() {
    let tmp = TempDir::new("split-sqzv-missing-repairable");
    let input = sample_input(tmp.path());
    let dest = tmp.path().join("out.sqz");
    let opts = CreateOptions {
        split_size: Some(30 * 1024),
        ..CreateOptions::default()
    };
    let ctl = ControlToken::new();
    engine()
        .create(&dest, &[input], &opts, &NoProgress, &ctl)
        .unwrap();

    let missing = tmp.path().join("out.sqz.002");
    fs::remove_file(&missing).unwrap();
    fs::remove_file(sqz_recovery_volume_path(tmp.path(), 1)).unwrap();

    let first = tmp.path().join("out.sqz.001");
    let entries = engine().list(&first, &OpenOptions::default()).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].path.display, "data.bin");

    let report = engine()
        .test(&first, &OpenOptions::default(), &NoProgress, &ctl)
        .unwrap();
    assert!(report.is_ok(), "problems: {:?}", report.problems);

    let out = tmp.path().join("sqzv-recovered");
    engine()
        .extract(
            &first,
            &out,
            None,
            &OpenOptions::default(),
            &ExtractOptions::default(),
            &NoProgress,
            &ctl,
        )
        .unwrap();
    assert_eq!(fs::read(out.join("data.bin")).unwrap(), payload(100 * 1024));
}

#[test]
fn missing_sqzv_payload_volume_recovers_from_rev_parity_when_rs_capacity_exceeded() {
    let tmp = TempDir::new("split-sqzv-missing-parity");
    let input_len = 700 * 1024;
    let input = sample_input_with_len(tmp.path(), input_len);
    let dest = tmp.path().join("out.sqz");
    let opts = CreateOptions {
        split_size: Some(180 * 1024),
        ..CreateOptions::default()
    };
    let ctl = ControlToken::new();
    engine()
        .create(&dest, &[input], &opts, &NoProgress, &ctl)
        .unwrap();

    assert!(sqz_recovery_volume_path(tmp.path(), 1).is_file());
    fs::remove_file(tmp.path().join("out.sqz.002")).unwrap();

    let first = tmp.path().join("out.sqz.001");
    let entries = engine().list(&first, &OpenOptions::default()).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].path.display, "data.bin");

    let report = engine()
        .test(&first, &OpenOptions::default(), &NoProgress, &ctl)
        .unwrap();
    assert!(report.is_ok(), "problems: {:?}", report.problems);

    let out = tmp.path().join("sqzv-parity-recovered");
    engine()
        .extract(
            &first,
            &out,
            None,
            &OpenOptions::default(),
            &ExtractOptions::default(),
            &NoProgress,
            &ctl,
        )
        .unwrap();
    assert_eq!(fs::read(out.join("data.bin")).unwrap(), payload(input_len));
}

#[test]
fn missing_two_sqzv_payload_volumes_recover_from_dual_rev_parity() {
    let tmp = TempDir::new("split-sqzv-two-missing-dual-parity");
    let input_len = 700 * 1024;
    let input = sample_input_with_len(tmp.path(), input_len);
    let dest = tmp.path().join("out.sqz");
    let opts = CreateOptions {
        split_size: Some(180 * 1024),
        ..CreateOptions::default()
    };
    let ctl = ControlToken::new();
    engine()
        .create(&dest, &[input], &opts, &NoProgress, &ctl)
        .unwrap();

    assert!(sqz_recovery_volume_path(tmp.path(), 1).is_file());
    assert!(sqz_recovery_volume_path(tmp.path(), 2).is_file());
    fs::remove_file(tmp.path().join("out.sqz.002")).unwrap();
    fs::remove_file(tmp.path().join("out.sqz.003")).unwrap();

    let first = tmp.path().join("out.sqz.001");
    let entries = engine().list(&first, &OpenOptions::default()).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].path.display, "data.bin");

    let report = engine()
        .test(&first, &OpenOptions::default(), &NoProgress, &ctl)
        .unwrap();
    assert!(report.is_ok(), "problems: {:?}", report.problems);

    let out = tmp.path().join("sqzv-two-missing-recovered");
    engine()
        .extract(
            &first,
            &out,
            None,
            &OpenOptions::default(),
            &ExtractOptions::default(),
            &NoProgress,
            &ctl,
        )
        .unwrap();
    assert_eq!(fs::read(out.join("data.bin")).unwrap(), payload(input_len));
}

#[test]
fn missing_sqzv_payload_volume_fails_with_damaged_rev001_header() {
    let tmp = TempDir::new("split-sqzv-damaged-rev001-header");
    let input = sample_input_with_len(tmp.path(), 700 * 1024);
    let dest = tmp.path().join("out.sqz");
    let opts = CreateOptions {
        split_size: Some(180 * 1024),
        ..CreateOptions::default()
    };
    engine()
        .create(&dest, &[input], &opts, &NoProgress, &ControlToken::new())
        .unwrap();

    fs::remove_file(tmp.path().join("out.sqz.002")).unwrap();
    corrupt_sqzr_header(&sqz_recovery_volume_path(tmp.path(), 1));

    assert_open_fails_with_corrupt_archive(
        &tmp.path().join("out.sqz.001"),
        "SQZ recovery volume header CRC-32C mismatch",
    );
}

#[test]
fn missing_sqzv_payload_volume_fails_with_damaged_rev001_payload() {
    let tmp = TempDir::new("split-sqzv-damaged-rev001-payload");
    let input = sample_input_with_len(tmp.path(), 700 * 1024);
    let dest = tmp.path().join("out.sqz");
    let opts = CreateOptions {
        split_size: Some(180 * 1024),
        ..CreateOptions::default()
    };
    engine()
        .create(&dest, &[input], &opts, &NoProgress, &ControlToken::new())
        .unwrap();

    fs::remove_file(tmp.path().join("out.sqz.002")).unwrap();
    corrupt_sqzr_payload_byte(&sqz_recovery_volume_path(tmp.path(), 1), 0);

    assert_open_fails_with_corrupt_archive(&tmp.path().join("out.sqz.001"), "SQZV");
}

#[test]
fn missing_two_sqzv_payload_volumes_fail_with_damaged_rev002_header() {
    let tmp = TempDir::new("split-sqzv-damaged-rev002-header");
    let input = sample_input_with_len(tmp.path(), 700 * 1024);
    let dest = tmp.path().join("out.sqz");
    let opts = CreateOptions {
        split_size: Some(180 * 1024),
        ..CreateOptions::default()
    };
    engine()
        .create(&dest, &[input], &opts, &NoProgress, &ControlToken::new())
        .unwrap();

    fs::remove_file(tmp.path().join("out.sqz.002")).unwrap();
    fs::remove_file(tmp.path().join("out.sqz.003")).unwrap();
    corrupt_sqzr_header(&sqz_recovery_volume_path(tmp.path(), 2));

    assert_open_fails_with_corrupt_archive(
        &tmp.path().join("out.sqz.001"),
        "SQZ recovery volume header CRC-32C mismatch",
    );
}

#[test]
fn missing_two_sqzv_payload_volumes_fail_with_damaged_rev002_payload() {
    let tmp = TempDir::new("split-sqzv-damaged-rev002-payload");
    let input = sample_input_with_len(tmp.path(), 700 * 1024);
    let dest = tmp.path().join("out.sqz");
    let opts = CreateOptions {
        split_size: Some(180 * 1024),
        ..CreateOptions::default()
    };
    engine()
        .create(&dest, &[input], &opts, &NoProgress, &ControlToken::new())
        .unwrap();

    fs::remove_file(tmp.path().join("out.sqz.002")).unwrap();
    fs::remove_file(tmp.path().join("out.sqz.003")).unwrap();
    corrupt_sqzr_payload_byte(&sqz_recovery_volume_path(tmp.path(), 2), 0);

    assert_open_fails_with_corrupt_archive(&tmp.path().join("out.sqz.001"), "SQZV");
}

#[test]
fn missing_three_sqzv_payload_volumes_fail_with_damaged_rev003_header() {
    let tmp = TempDir::new("split-sqzv-damaged-rev003-header");
    let input = sample_input_with_len(tmp.path(), 900 * 1024);
    let dest = tmp.path().join("out.sqz");
    let opts = CreateOptions {
        split_size: Some(180 * 1024),
        ..CreateOptions::default()
    };
    engine()
        .create(&dest, &[input], &opts, &NoProgress, &ControlToken::new())
        .unwrap();

    fs::remove_file(tmp.path().join("out.sqz.002")).unwrap();
    fs::remove_file(tmp.path().join("out.sqz.003")).unwrap();
    fs::remove_file(tmp.path().join("out.sqz.004")).unwrap();
    corrupt_sqzr_header(&sqz_recovery_volume_path(tmp.path(), 3));

    assert_open_fails_with_corrupt_archive(
        &tmp.path().join("out.sqz.001"),
        "SQZ recovery volume header CRC-32C mismatch",
    );
}

#[test]
fn missing_three_sqzv_payload_volumes_fail_with_damaged_rev003_payload() {
    let tmp = TempDir::new("split-sqzv-damaged-rev003-payload");
    let input = sample_input_with_len(tmp.path(), 900 * 1024);
    let dest = tmp.path().join("out.sqz");
    let opts = CreateOptions {
        split_size: Some(180 * 1024),
        ..CreateOptions::default()
    };
    engine()
        .create(&dest, &[input], &opts, &NoProgress, &ControlToken::new())
        .unwrap();

    fs::remove_file(tmp.path().join("out.sqz.002")).unwrap();
    fs::remove_file(tmp.path().join("out.sqz.003")).unwrap();
    fs::remove_file(tmp.path().join("out.sqz.004")).unwrap();
    corrupt_sqzr_payload_byte(&sqz_recovery_volume_path(tmp.path(), 3), 0);

    assert_open_fails_with_corrupt_archive(&tmp.path().join("out.sqz.001"), "SQZV");
}

#[test]
fn missing_two_sqzv_payload_volumes_fail_without_dual_rev_parity() {
    let tmp = TempDir::new("split-sqzv-two-missing-no-dual-parity");
    let input_len = 700 * 1024;
    let input = sample_input_with_len(tmp.path(), input_len);
    let dest = tmp.path().join("out.sqz");
    let opts = CreateOptions {
        split_size: Some(180 * 1024),
        ..CreateOptions::default()
    };
    let ctl = ControlToken::new();
    engine()
        .create(&dest, &[input], &opts, &NoProgress, &ctl)
        .unwrap();

    fs::remove_file(tmp.path().join("out.sqz.002")).unwrap();
    fs::remove_file(tmp.path().join("out.sqz.003")).unwrap();
    fs::remove_file(sqz_recovery_volume_path(tmp.path(), 2)).unwrap();

    let first = tmp.path().join("out.sqz.001");
    let report = engine()
        .test(&first, &OpenOptions::default(), &NoProgress, &ctl)
        .unwrap();
    assert!(
        !report.is_ok(),
        "two missing large volumes require the weighted parity sidecar"
    );

    let out = tmp.path().join("sqzv-two-missing-no-dual");
    let err = engine()
        .extract(
            &first,
            &out,
            None,
            &OpenOptions::default(),
            &ExtractOptions::default(),
            &NoProgress,
            &ctl,
        )
        .unwrap_err();
    match err {
        FormatError::CorruptArchive(detail) => {
            assert!(
                detail.contains("unrepaired") || detail.contains("block"),
                "{detail}"
            );
        }
        other => panic!("expected CorruptArchive, got {other:?}"),
    }
}

#[test]
fn missing_three_sqzv_payload_volumes_recover_from_triple_rev_parity() {
    let tmp = TempDir::new("split-sqzv-three-missing-triple-parity");
    let input_len = 1_000 * 1024;
    let input = sample_input_with_len(tmp.path(), input_len);
    let dest = tmp.path().join("out.sqz");
    let opts = CreateOptions {
        split_size: Some(180 * 1024),
        ..CreateOptions::default()
    };
    let ctl = ControlToken::new();
    engine()
        .create(&dest, &[input], &opts, &NoProgress, &ctl)
        .unwrap();

    assert!(sqz_recovery_volume_path(tmp.path(), 1).is_file());
    assert!(sqz_recovery_volume_path(tmp.path(), 2).is_file());
    assert!(sqz_recovery_volume_path(tmp.path(), 3).is_file());
    fs::remove_file(tmp.path().join("out.sqz.002")).unwrap();
    fs::remove_file(tmp.path().join("out.sqz.003")).unwrap();
    fs::remove_file(tmp.path().join("out.sqz.004")).unwrap();

    let first = tmp.path().join("out.sqz.001");
    let entries = engine().list(&first, &OpenOptions::default()).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].path.display, "data.bin");

    let report = engine()
        .test(&first, &OpenOptions::default(), &NoProgress, &ctl)
        .unwrap();
    assert!(report.is_ok(), "problems: {:?}", report.problems);

    let out = tmp.path().join("sqzv-three-missing-recovered");
    engine()
        .extract(
            &first,
            &out,
            None,
            &OpenOptions::default(),
            &ExtractOptions::default(),
            &NoProgress,
            &ctl,
        )
        .unwrap();
    assert_eq!(fs::read(out.join("data.bin")).unwrap(), payload(input_len));
}

#[test]
fn missing_three_sqzv_payload_volumes_fail_without_triple_rev_parity() {
    let tmp = TempDir::new("split-sqzv-three-missing-no-triple-parity");
    let input_len = 1_000 * 1024;
    let input = sample_input_with_len(tmp.path(), input_len);
    let dest = tmp.path().join("out.sqz");
    let opts = CreateOptions {
        split_size: Some(180 * 1024),
        ..CreateOptions::default()
    };
    let ctl = ControlToken::new();
    engine()
        .create(&dest, &[input], &opts, &NoProgress, &ctl)
        .unwrap();

    fs::remove_file(tmp.path().join("out.sqz.002")).unwrap();
    fs::remove_file(tmp.path().join("out.sqz.003")).unwrap();
    fs::remove_file(tmp.path().join("out.sqz.004")).unwrap();
    fs::remove_file(sqz_recovery_volume_path(tmp.path(), 3)).unwrap();

    let first = tmp.path().join("out.sqz.001");
    let report = engine()
        .test(&first, &OpenOptions::default(), &NoProgress, &ctl)
        .unwrap();
    assert!(
        !report.is_ok(),
        "three missing large volumes require the quadratic parity sidecar"
    );

    let out = tmp.path().join("sqzv-three-missing-no-triple");
    let err = engine()
        .extract(
            &first,
            &out,
            None,
            &OpenOptions::default(),
            &ExtractOptions::default(),
            &NoProgress,
            &ctl,
        )
        .unwrap_err();
    match err {
        FormatError::CorruptArchive(detail) => {
            assert!(
                detail.contains("unrepaired") || detail.contains("block"),
                "{detail}"
            );
        }
        other => panic!("expected CorruptArchive, got {other:?}"),
    }
}

#[test]
fn missing_four_sqzv_payload_volumes_still_fail_with_three_rev_parity() {
    let tmp = TempDir::new("split-sqzv-four-missing");
    let input_len = 1_200 * 1024;
    let input = sample_input_with_len(tmp.path(), input_len);
    let dest = tmp.path().join("out.sqz");
    let opts = CreateOptions {
        split_size: Some(180 * 1024),
        ..CreateOptions::default()
    };
    let ctl = ControlToken::new();
    engine()
        .create(&dest, &[input], &opts, &NoProgress, &ctl)
        .unwrap();

    assert!(sqz_recovery_volume_path(tmp.path(), 1).is_file());
    assert!(sqz_recovery_volume_path(tmp.path(), 2).is_file());
    assert!(sqz_recovery_volume_path(tmp.path(), 3).is_file());
    fs::remove_file(tmp.path().join("out.sqz.002")).unwrap();
    fs::remove_file(tmp.path().join("out.sqz.003")).unwrap();
    fs::remove_file(tmp.path().join("out.sqz.004")).unwrap();
    fs::remove_file(tmp.path().join("out.sqz.005")).unwrap();

    let first = tmp.path().join("out.sqz.001");
    let report = engine()
        .test(&first, &OpenOptions::default(), &NoProgress, &ctl)
        .unwrap();
    assert!(
        !report.is_ok(),
        "four missing large volumes must not be over-claimed as recoverable"
    );
}

#[test]
fn missing_sqzv_payload_volume_over_capacity_fails_without_rev_parity() {
    let tmp = TempDir::new("split-sqzv-missing-no-parity");
    let input_len = 700 * 1024;
    let input = sample_input_with_len(tmp.path(), input_len);
    let dest = tmp.path().join("out.sqz");
    let opts = CreateOptions {
        split_size: Some(180 * 1024),
        ..CreateOptions::default()
    };
    let ctl = ControlToken::new();
    engine()
        .create(&dest, &[input], &opts, &NoProgress, &ctl)
        .unwrap();

    fs::remove_file(tmp.path().join("out.sqz.002")).unwrap();
    fs::remove_file(sqz_recovery_volume_path(tmp.path(), 1)).unwrap();

    let first = tmp.path().join("out.sqz.001");
    let report = engine()
        .test(&first, &OpenOptions::default(), &NoProgress, &ctl)
        .unwrap();
    assert!(
        !report.is_ok(),
        "missing large volume must exceed embedded RS capacity"
    );

    let out = tmp.path().join("sqzv-no-parity");
    let err = engine()
        .extract(
            &first,
            &out,
            None,
            &OpenOptions::default(),
            &ExtractOptions::default(),
            &NoProgress,
            &ctl,
        )
        .unwrap_err();
    match err {
        FormatError::CorruptArchive(detail) => {
            assert!(
                detail.contains("unrepaired") || detail.contains("block"),
                "{detail}"
            );
        }
        other => panic!("expected CorruptArchive, got {other:?}"),
    }
}

#[test]
fn missing_sqzv_tail_volume_recovers_from_rev_sidecar() {
    let tmp = TempDir::new("split-sqzv-missing-tail-sidecar");
    let input = sample_input(tmp.path());
    let dest = tmp.path().join("out.sqz");
    let opts = CreateOptions {
        split_size: Some(30 * 1024),
        ..CreateOptions::default()
    };
    let ctl = ControlToken::new();
    engine()
        .create(&dest, &[input], &opts, &NoProgress, &ctl)
        .unwrap();

    let volumes = volume_paths(tmp.path(), "out.sqz.");
    let tail = volumes.last().expect("tail volume").clone();
    let tail_mirror = sqz_recovery_volume_path(tmp.path(), volumes.len());
    assert!(tail_mirror.is_file(), "{} missing", tail_mirror.display());
    assert!(sqz_recovery_volume_path(tmp.path(), 1).is_file());
    fs::remove_file(&tail).unwrap();

    let first = tmp.path().join("out.sqz.001");
    let entries = engine().list(&first, &OpenOptions::default()).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].path.display, "data.bin");

    let report = engine()
        .test(&first, &OpenOptions::default(), &NoProgress, &ctl)
        .unwrap();
    assert!(report.is_ok(), "problems: {:?}", report.problems);

    let out = tmp.path().join("sqzv-tail-recovered");
    engine()
        .extract(
            &first,
            &out,
            None,
            &OpenOptions::default(),
            &ExtractOptions::default(),
            &NoProgress,
            &ctl,
        )
        .unwrap();
    assert_eq!(fs::read(out.join("data.bin")).unwrap(), payload(100 * 1024));
}

#[test]
fn missing_sqzv_payload_and_tail_recover_from_parity_plus_tail_mirror() {
    let tmp = TempDir::new("split-sqzv-missing-payload-tail");
    let input = sample_input(tmp.path());
    let dest = tmp.path().join("out.sqz");
    let opts = CreateOptions {
        split_size: Some(30 * 1024),
        ..CreateOptions::default()
    };
    let ctl = ControlToken::new();
    engine()
        .create(&dest, &[input], &opts, &NoProgress, &ctl)
        .unwrap();

    let volumes = volume_paths(tmp.path(), "out.sqz.");
    assert!(volumes.len() >= 4, "volumes: {volumes:?}");
    let tail = volumes.last().expect("tail volume").clone();
    let tail_mirror = sqz_recovery_volume_path(tmp.path(), volumes.len());
    assert!(tail_mirror.is_file(), "{} missing", tail_mirror.display());
    assert!(sqz_recovery_volume_path(tmp.path(), 1).is_file());

    fs::remove_file(tmp.path().join("out.sqz.002")).unwrap();
    fs::remove_file(&tail).unwrap();

    let first = tmp.path().join("out.sqz.001");
    let entries = engine().list(&first, &OpenOptions::default()).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].path.display, "data.bin");

    let report = engine()
        .test(&first, &OpenOptions::default(), &NoProgress, &ctl)
        .unwrap();
    assert!(report.is_ok(), "problems: {:?}", report.problems);

    let out = tmp.path().join("sqzv-payload-tail-recovered");
    engine()
        .extract(
            &first,
            &out,
            None,
            &OpenOptions::default(),
            &ExtractOptions::default(),
            &NoProgress,
            &ctl,
        )
        .unwrap();
    assert_eq!(fs::read(out.join("data.bin")).unwrap(), payload(100 * 1024));
}

#[test]
fn missing_sqzv_tail_volume_is_still_unrecoverable() {
    let tmp = TempDir::new("split-sqzv-missing-tail");
    let input = sample_input(tmp.path());
    let dest = tmp.path().join("out.sqz");
    let opts = CreateOptions {
        split_size: Some(30 * 1024),
        ..CreateOptions::default()
    };
    engine()
        .create(&dest, &[input], &opts, &NoProgress, &ControlToken::new())
        .unwrap();

    let volumes = volume_paths(tmp.path(), "out.sqz.");
    let tail = volumes.last().expect("tail volume").clone();
    let tail_mirror = sqz_recovery_volume_path(tmp.path(), volumes.len());
    let parity = sqz_recovery_volume_path(tmp.path(), 1);
    fs::remove_file(&tail).unwrap();
    fs::remove_file(&tail_mirror).unwrap();
    fs::remove_file(&parity).unwrap();

    let err = engine()
        .list(&tmp.path().join("out.sqz.001"), &OpenOptions::default())
        .unwrap_err();
    match err {
        FormatError::CorruptArchive(detail) => {
            assert!(detail.contains("tail volume"), "detail: {detail}");
        }
        other => panic!("expected CorruptArchive, got {other:?}"),
    }
}

#[test]
fn tiny_split_size_is_rejected() {
    let tmp = TempDir::new("split-tiny");
    let input = sample_input(tmp.path());
    let opts = CreateOptions {
        split_size: Some(64),
        ..CreateOptions::default()
    };
    let err = engine()
        .create(
            &tmp.path().join("out.zip"),
            &[input],
            &opts,
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap_err();
    assert!(matches!(err, FormatError::Unsupported(_)));
}
