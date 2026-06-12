//! SQZ v1 tests: transparent container create/list/extract/test plus embedded
//! recovery behavior.

mod common;

use std::fs;
use std::path::Path;

use common::{engine, TempDir};
use squallz_core::api::{
    ControlToken, CreateOptions, EntryType, ExtractOptions, FormatError, NoProgress, OpenOptions,
    SqzCreateOptions,
};

const SQZ_RECOVERY_BLOCK: usize = 64 * 1024;

fn build_tree(root: &Path) {
    let project = root.join("project");
    fs::create_dir_all(project.join("deep/a/b")).unwrap();
    fs::create_dir_all(project.join("empty")).unwrap();
    fs::write(project.join("a.txt"), b"hello sqz").unwrap();
    fs::write(project.join("deep/a/b/data.bin"), vec![0x42; 8192]).unwrap();
    fs::write(project.join("中文.txt"), "中文内容").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink("a.txt", project.join("link.txt")).unwrap();
}

fn recovery_marker(block: usize) -> Vec<u8> {
    format!("SQZ-RECOVERY-BLOCK-{block:02}-unique-marker").into_bytes()
}

fn recovery_payload(blocks: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(blocks * SQZ_RECOVERY_BLOCK);
    for block_index in 0..blocks {
        let marker = recovery_marker(block_index);
        let mut block = vec![0u8; SQZ_RECOVERY_BLOCK];
        block[..marker.len()].copy_from_slice(&marker);
        for (offset, byte) in block.iter_mut().enumerate().skip(marker.len()) {
            *byte = ((block_index * 31 + offset * 17) % 251) as u8;
        }
        out.extend_from_slice(&block);
    }
    out
}

fn corrupt_payload_blocks(archive: &mut [u8], blocks: &[usize]) {
    for block in blocks {
        let marker = recovery_marker(*block);
        let pos = archive
            .windows(marker.len())
            .position(|w| w == marker)
            .unwrap_or_else(|| panic!("payload marker not found for block {block}"));
        archive[pos + marker.len() - 1] ^= 0x5A;
    }
}

fn rewrite_sqz_header_crc(archive: &mut [u8]) {
    let crc = crc32c::crc32c(&archive[..52]);
    archive[52..56].copy_from_slice(&crc.to_le_bytes());
}

fn rewrite_sqz_footer_crc(archive: &mut [u8]) {
    let footer_start = archive.len() - 64;
    let crc = crc32c::crc32c(&archive[footer_start..footer_start + 48]);
    archive[footer_start + 48..footer_start + 52].copy_from_slice(&crc.to_le_bytes());
}

fn corrupt_recovery_primary_blocks(archive: &mut [u8], blocks: &[usize]) {
    let recovery_pos = archive
        .windows(b"RSEC".len())
        .position(|w| w == b"RSEC")
        .expect("primary recovery section found");
    for block in blocks {
        let pos = recovery_pos + block * SQZ_RECOVERY_BLOCK;
        assert!(
            pos < archive.len(),
            "primary recovery section block {block} is inside archive"
        );
        archive[pos] ^= 0x7F;
    }
}

fn recovery_header_shards(archive: &[u8]) -> (u16, u16) {
    let recovery_pos = archive
        .windows(b"RSEC".len())
        .position(|w| w == b"RSEC")
        .expect("primary recovery section found");
    let data_shards = u16::from_le_bytes(
        archive[recovery_pos + 12..recovery_pos + 14]
            .try_into()
            .unwrap(),
    );
    let parity_shards = u16::from_le_bytes(
        archive[recovery_pos + 14..recovery_pos + 16]
            .try_into()
            .unwrap(),
    );
    (data_shards, parity_shards)
}

fn byte_pattern_positions(bytes: &[u8], pattern: &[u8]) -> Vec<usize> {
    bytes
        .windows(pattern.len())
        .enumerate()
        .filter_map(|(index, window)| (window == pattern).then_some(index))
        .collect()
}

#[test]
fn sqz_file_header_crc_damage_falls_back_to_footer() {
    let tmp = TempDir::new("sqz-header-fallback");
    build_tree(tmp.path());
    let archive = tmp.path().join("out.sqz");
    let eng = engine();
    let ctl = ControlToken::new();
    eng.create(
        &archive,
        &[tmp.path().join("project")],
        &CreateOptions::default(),
        &NoProgress,
        &ctl,
    )
    .unwrap();

    let mut bytes = fs::read(&archive).unwrap();
    assert_eq!(&bytes[0..8], b"SQZARCH\x1A");
    bytes[16] ^= 0x55;
    let damaged = tmp.path().join("damaged-header.sqz");
    fs::write(&damaged, bytes).unwrap();

    let entries = eng.list(&damaged, &OpenOptions::default()).unwrap();
    assert!(entries.iter().any(|e| e.path.display == "project/a.txt"));
    let report = eng
        .test(&damaged, &OpenOptions::default(), &NoProgress, &ctl)
        .unwrap();
    assert!(report.is_ok(), "problems: {:?}", report.problems);

    let out = tmp.path().join("out");
    eng.extract(
        &damaged,
        &out,
        None,
        &OpenOptions::default(),
        &ExtractOptions::default(),
        &NoProgress,
        &ctl,
    )
    .unwrap();
    assert_eq!(fs::read(out.join("project/a.txt")).unwrap(), b"hello sqz");
}

#[test]
fn sqz_valid_header_footer_uuid_mismatch_fails() {
    let tmp = TempDir::new("sqz-header-uuid-mismatch");
    build_tree(tmp.path());
    let archive = tmp.path().join("out.sqz");
    let eng = engine();
    let ctl = ControlToken::new();
    eng.create(
        &archive,
        &[tmp.path().join("project")],
        &CreateOptions::default(),
        &NoProgress,
        &ctl,
    )
    .unwrap();

    let mut bytes = fs::read(&archive).unwrap();
    assert_eq!(&bytes[0..8], b"SQZARCH\x1A");
    bytes[16] ^= 0x55;
    rewrite_sqz_header_crc(&mut bytes);
    let damaged = tmp.path().join("valid-header-wrong-uuid.sqz");
    fs::write(&damaged, bytes).unwrap();

    let err = eng.list(&damaged, &OpenOptions::default()).unwrap_err();
    assert!(matches!(err, FormatError::CorruptArchive(_)), "{err:?}");
}

#[test]
fn sqz_footer_header_valid_crc_bad_index_bounds_fails() {
    let tmp = TempDir::new("sqz-footer-index-bounds");
    build_tree(tmp.path());
    let archive = tmp.path().join("out.sqz");
    let eng = engine();
    let ctl = ControlToken::new();
    eng.create(
        &archive,
        &[tmp.path().join("project")],
        &CreateOptions::default(),
        &NoProgress,
        &ctl,
    )
    .unwrap();

    let mut bytes = fs::read(&archive).unwrap();
    let footer_start = bytes.len() - 64;
    bytes[footer_start + 8..footer_start + 16].copy_from_slice(&u64::MAX.to_le_bytes());
    rewrite_sqz_footer_crc(&mut bytes);
    let damaged = tmp.path().join("valid-footer-bad-index.sqz");
    fs::write(&damaged, bytes).unwrap();

    let err = eng.list(&damaged, &OpenOptions::default()).unwrap_err();
    assert!(matches!(err, FormatError::CorruptArchive(_)), "{err:?}");
}

#[test]
fn sqz_footer_magic_damage_recovers_from_recovery_scan() {
    let tmp = TempDir::new("sqz-footer-scan-recovery");
    build_tree(tmp.path());
    let archive = tmp.path().join("out.sqz");
    let eng = engine();
    let ctl = ControlToken::new();
    eng.create(
        &archive,
        &[tmp.path().join("project")],
        &CreateOptions::default(),
        &NoProgress,
        &ctl,
    )
    .unwrap();

    let mut bytes = fs::read(&archive).unwrap();
    let footer_start = bytes.len() - 64;
    assert_eq!(
        &bytes[footer_start + 56..footer_start + 64],
        b"\x1ASQZEND\n"
    );
    bytes[footer_start + 63] ^= 0x5A;
    let damaged = tmp.path().join("damaged-footer-magic.sqz");
    fs::write(&damaged, bytes).unwrap();

    let entries = eng.list(&damaged, &OpenOptions::default()).unwrap();
    assert!(entries.iter().any(|e| e.path.display == "project/a.txt"));
    let report = eng
        .test(&damaged, &OpenOptions::default(), &NoProgress, &ctl)
        .unwrap();
    assert!(report.is_ok(), "problems: {:?}", report.problems);

    let out = tmp.path().join("out");
    eng.extract(
        &damaged,
        &out,
        None,
        &OpenOptions::default(),
        &ExtractOptions::default(),
        &NoProgress,
        &ctl,
    )
    .unwrap();
    assert_eq!(fs::read(out.join("project/a.txt")).unwrap(), b"hello sqz");
}

#[test]
fn sqz_footer_crc_field_damage_recovers_from_recovery_scan() {
    let tmp = TempDir::new("sqz-footer-crc-field-recovery");
    build_tree(tmp.path());
    let archive = tmp.path().join("out.sqz");
    let eng = engine();
    let ctl = ControlToken::new();
    eng.create(
        &archive,
        &[tmp.path().join("project")],
        &CreateOptions::default(),
        &NoProgress,
        &ctl,
    )
    .unwrap();

    let mut bytes = fs::read(&archive).unwrap();
    let footer_start = bytes.len() - 64;
    assert_eq!(
        &bytes[footer_start + 56..footer_start + 64],
        b"\x1ASQZEND\n"
    );
    bytes[footer_start] ^= 0x5A;
    let damaged = tmp.path().join("damaged-footer-crc-field.sqz");
    fs::write(&damaged, bytes).unwrap();

    let entries = eng.list(&damaged, &OpenOptions::default()).unwrap();
    assert!(entries.iter().any(|e| e.path.display == "project/a.txt"));
    let report = eng
        .test(&damaged, &OpenOptions::default(), &NoProgress, &ctl)
        .unwrap();
    assert!(report.is_ok(), "problems: {:?}", report.problems);

    let out = tmp.path().join("out");
    eng.extract(
        &damaged,
        &out,
        None,
        &OpenOptions::default(),
        &ExtractOptions::default(),
        &NoProgress,
        &ctl,
    )
    .unwrap();
    assert_eq!(fs::read(out.join("project/a.txt")).unwrap(), b"hello sqz");
}

#[test]
fn sqz_recovery_protection_trailer_damage_uses_intact_primary() {
    let tmp = TempDir::new("sqz-rspc-trailer-damage");
    build_tree(tmp.path());
    let archive = tmp.path().join("out.sqz");
    let eng = engine();
    let ctl = ControlToken::new();
    eng.create(
        &archive,
        &[tmp.path().join("project")],
        &CreateOptions::default(),
        &NoProgress,
        &ctl,
    )
    .unwrap();

    let mut bytes = fs::read(&archive).unwrap();
    let trailer_pos = byte_pattern_positions(&bytes, b"RSPC")
        .pop()
        .expect("recovery protection trailer found");
    bytes[trailer_pos + 44] ^= 0x55;
    let damaged = tmp.path().join("damaged-rspc-trailer.sqz");
    fs::write(&damaged, bytes).unwrap();

    let entries = eng.list(&damaged, &OpenOptions::default()).unwrap();
    assert!(entries.iter().any(|e| e.path.display == "project/a.txt"));
    let report = eng
        .test(&damaged, &OpenOptions::default(), &NoProgress, &ctl)
        .unwrap();
    assert!(report.is_ok(), "problems: {:?}", report.problems);

    let out = tmp.path().join("out");
    eng.extract(
        &damaged,
        &out,
        None,
        &OpenOptions::default(),
        &ExtractOptions::default(),
        &NoProgress,
        &ctl,
    )
    .unwrap();
    assert_eq!(fs::read(out.join("project/a.txt")).unwrap(), b"hello sqz");
}

#[test]
fn sqz_recovery_protection_trailer_and_primary_damage_fails() {
    let tmp = TempDir::new("sqz-rspc-trailer-primary-damage");
    build_tree(tmp.path());
    let archive = tmp.path().join("out.sqz");
    let eng = engine();
    let ctl = ControlToken::new();
    eng.create(
        &archive,
        &[tmp.path().join("project")],
        &CreateOptions::default(),
        &NoProgress,
        &ctl,
    )
    .unwrap();

    let mut bytes = fs::read(&archive).unwrap();
    corrupt_recovery_primary_blocks(&mut bytes, &[0]);
    let trailer_pos = byte_pattern_positions(&bytes, b"RSPC")
        .pop()
        .expect("recovery protection trailer found");
    bytes[trailer_pos + 44] ^= 0x55;
    let damaged = tmp.path().join("damaged-rspc-trailer-primary.sqz");
    fs::write(&damaged, bytes).unwrap();

    let err = eng.list(&damaged, &OpenOptions::default()).unwrap_err();
    assert!(matches!(err, FormatError::CorruptArchive(_)), "{err:?}");
}

#[test]
fn sqz_recovery_protection_trailer_valid_crc_bad_version_fails() {
    let tmp = TempDir::new("sqz-rspc-trailer-version-damage");
    build_tree(tmp.path());
    let archive = tmp.path().join("out.sqz");
    let eng = engine();
    eng.create(
        &archive,
        &[tmp.path().join("project")],
        &CreateOptions::default(),
        &NoProgress,
        &ControlToken::new(),
    )
    .unwrap();

    let mut bytes = fs::read(&archive).unwrap();
    let trailer_pos = byte_pattern_positions(&bytes, b"RSPC")
        .pop()
        .expect("recovery protection trailer found");
    bytes[trailer_pos + 4] = 2;
    let crc = crc32c::crc32c(&bytes[trailer_pos..trailer_pos + 76]);
    bytes[trailer_pos + 76..trailer_pos + 80].copy_from_slice(&crc.to_le_bytes());
    let damaged = tmp.path().join("damaged-rspc-trailer-version.sqz");
    fs::write(&damaged, bytes).unwrap();

    let err = eng.list(&damaged, &OpenOptions::default()).unwrap_err();
    assert!(matches!(err, FormatError::Unsupported(_)), "{err:?}");
}

#[test]
fn sqz_roundtrip_create_list_extract_test() {
    let tmp = TempDir::new("sqz-roundtrip");
    build_tree(tmp.path());
    let archive = tmp.path().join("out.sqz");
    let eng = engine();
    let ctl = ControlToken::new();

    eng.create(
        &archive,
        &[tmp.path().join("project")],
        &CreateOptions::default(),
        &NoProgress,
        &ctl,
    )
    .unwrap();

    let entries = eng.list(&archive, &OpenOptions::default()).unwrap();
    assert!(entries.iter().any(|e| e.path.display == "project/a.txt"));
    assert!(entries
        .iter()
        .any(|e| e.path.display == "project/deep/a/b/data.bin"));
    assert!(entries.iter().any(|e| e.path.display == "project/中文.txt"));
    assert!(entries
        .iter()
        .any(|e| e.path.display == "project/empty" && matches!(e.entry_type, EntryType::Dir)));
    #[cfg(unix)]
    {
        let link = entries
            .iter()
            .find(|e| e.path.display == "project/link.txt")
            .expect("symlink listed");
        assert!(matches!(&link.entry_type, EntryType::Symlink { target } if target == b"a.txt"));
    }

    let report = eng
        .test(&archive, &OpenOptions::default(), &NoProgress, &ctl)
        .unwrap();
    assert!(report.is_ok(), "problems: {:?}", report.problems);
    assert_eq!(report.entries_tested, entries.len() as u64);

    let out = tmp.path().join("out");
    eng.extract(
        &archive,
        &out,
        None,
        &OpenOptions::default(),
        &ExtractOptions::default(),
        &NoProgress,
        &ctl,
    )
    .unwrap();
    assert_eq!(fs::read(out.join("project/a.txt")).unwrap(), b"hello sqz");
    assert_eq!(
        fs::read(out.join("project/deep/a/b/data.bin")).unwrap(),
        vec![0x42; 8192]
    );
    assert_eq!(
        fs::read_to_string(out.join("project/中文.txt")).unwrap(),
        "中文内容"
    );
    assert!(out.join("project/empty").is_dir());
    #[cfg(unix)]
    assert_eq!(
        fs::read_link(out.join("project/link.txt")).unwrap(),
        Path::new("a.txt")
    );
}

#[test]
fn sqz_sniffs_without_extension_and_detects_corruption() {
    let tmp = TempDir::new("sqz-corrupt");
    build_tree(tmp.path());
    let archive = tmp.path().join("out.sqz");
    let eng = engine();
    let ctl = ControlToken::new();
    eng.create(
        &archive,
        &[tmp.path().join("project")],
        &CreateOptions::default(),
        &NoProgress,
        &ctl,
    )
    .unwrap();

    let anonymous = tmp.path().join("blob.bin");
    fs::copy(&archive, &anonymous).unwrap();
    assert!(eng
        .list(&anonymous, &OpenOptions::default())
        .unwrap()
        .iter()
        .any(|e| e.path.display == "project/a.txt"));

    let mut truncated = fs::read(&archive).unwrap();
    truncated.truncate(truncated.len() - 16);
    let truncated_path = tmp.path().join("truncated.sqz");
    fs::write(&truncated_path, truncated).unwrap();
    let err = eng
        .list(&truncated_path, &OpenOptions::default())
        .unwrap_err();
    assert!(matches!(err, FormatError::CorruptArchive(_)), "{err:?}");
}

#[test]
fn sqz_recovery_section_self_protection_repairs_primary_damage() {
    let tmp = TempDir::new("sqz-recovery-self-protect");
    build_tree(tmp.path());
    let archive = tmp.path().join("out.sqz");
    let eng = engine();
    let ctl = ControlToken::new();
    eng.create(
        &archive,
        &[tmp.path().join("project")],
        &CreateOptions::default(),
        &NoProgress,
        &ctl,
    )
    .unwrap();

    let mut bytes = fs::read(&archive).unwrap();
    assert!(
        !byte_pattern_positions(&bytes, b"RSPC").is_empty(),
        "protected recovery trailer should be present"
    );
    corrupt_recovery_primary_blocks(&mut bytes, &[0]);
    let damaged = tmp.path().join("damaged-recovery-primary.sqz");
    fs::write(&damaged, bytes).unwrap();

    let entries = eng.list(&damaged, &OpenOptions::default()).unwrap();
    assert!(entries.iter().any(|e| e.path.display == "project/a.txt"));
    let report = eng
        .test(&damaged, &OpenOptions::default(), &NoProgress, &ctl)
        .unwrap();
    assert!(report.is_ok(), "problems: {:?}", report.problems);

    let out = tmp.path().join("out");
    eng.extract(
        &damaged,
        &out,
        None,
        &OpenOptions::default(),
        &ExtractOptions::default(),
        &NoProgress,
        &ctl,
    )
    .unwrap();
    assert_eq!(fs::read(out.join("project/a.txt")).unwrap(), b"hello sqz");
}

#[test]
fn sqz_recovery_section_self_protection_reports_over_limit_damage() {
    let tmp = TempDir::new("sqz-recovery-self-protect-over-limit");
    let project = tmp.path().join("project");
    fs::create_dir_all(&project).unwrap();
    fs::write(project.join("large.bin"), recovery_payload(16)).unwrap();

    let archive = tmp.path().join("out.sqz");
    let eng = engine();
    let ctl = ControlToken::new();
    eng.create(
        &archive,
        &[project],
        &CreateOptions::default(),
        &NoProgress,
        &ctl,
    )
    .unwrap();

    let mut bytes = fs::read(&archive).unwrap();
    corrupt_recovery_primary_blocks(&mut bytes, &[0, 1, 2]);
    let damaged = tmp.path().join("damaged-recovery-primary-over-limit.sqz");
    fs::write(&damaged, bytes).unwrap();

    let err = eng.list(&damaged, &OpenOptions::default()).unwrap_err();
    assert!(matches!(err, FormatError::CorruptArchive(_)), "{err:?}");
}

#[test]
fn sqz_embedded_recovery_repairs_single_payload_block() {
    let tmp = TempDir::new("sqz-recovery-repair");
    let project = tmp.path().join("project");
    fs::create_dir_all(&project).unwrap();
    let original = recovery_payload(8);
    fs::write(project.join("large.bin"), &original).unwrap();

    let archive = tmp.path().join("repairable.sqz");
    let eng = engine();
    let ctl = ControlToken::new();
    eng.create(
        &archive,
        std::slice::from_ref(&project),
        &CreateOptions::default(),
        &NoProgress,
        &ctl,
    )
    .unwrap();

    let mut bytes = fs::read(&archive).unwrap();
    corrupt_payload_blocks(&mut bytes, &[3]);
    let corrupt = tmp.path().join("repairable-corrupt.sqz");
    fs::write(&corrupt, bytes).unwrap();

    let report = eng
        .test(&corrupt, &OpenOptions::default(), &NoProgress, &ctl)
        .unwrap();
    assert!(report.is_ok(), "problems: {:?}", report.problems);
    let recovery = report.recovery.as_ref().expect("SQZ recovery summary");
    assert_eq!(recovery.scheme.as_str(), "sqz-embedded-rs-gf8");
    assert_eq!(recovery.damaged_blocks, 1);
    assert_eq!(recovery.repaired_blocks, 1);
    assert_eq!(recovery.unrepaired_blocks, 0);
    assert!(recovery.repair_possible);

    let out = tmp.path().join("out");
    eng.extract(
        &corrupt,
        &out,
        None,
        &OpenOptions::default(),
        &ExtractOptions::default(),
        &NoProgress,
        &ctl,
    )
    .unwrap();
    assert_eq!(fs::read(out.join("project/large.bin")).unwrap(), original);
}

#[test]
fn sqz_custom_recovery_percent_controls_payload_parity_shards() {
    let tmp = TempDir::new("sqz-recovery-custom-percent");
    let project = tmp.path().join("project");
    fs::create_dir_all(&project).unwrap();
    fs::write(project.join("large.bin"), recovery_payload(8)).unwrap();

    let archive = tmp.path().join("custom-recovery.sqz");
    let eng = engine();
    let ctl = ControlToken::new();
    eng.create(
        &archive,
        &[project],
        &CreateOptions {
            sqz: SqzCreateOptions {
                recovery_percent: 10,
                ..SqzCreateOptions::default()
            },
            ..CreateOptions::default()
        },
        &NoProgress,
        &ctl,
    )
    .unwrap();

    let mut bytes = fs::read(&archive).unwrap();
    assert_eq!(recovery_header_shards(&bytes), (8, 1));
    corrupt_payload_blocks(&mut bytes, &[0, 1]);
    let corrupt = tmp.path().join("custom-recovery-corrupt.sqz");
    fs::write(&corrupt, bytes).unwrap();

    let report = eng
        .test(&corrupt, &OpenOptions::default(), &NoProgress, &ctl)
        .unwrap();
    assert!(!report.is_ok());
    let recovery = report.recovery.as_ref().expect("SQZ recovery summary");
    assert_eq!(recovery.scheme.as_str(), "sqz-embedded-rs-gf8");
    assert_eq!(recovery.damaged_blocks, 2);
    assert_eq!(recovery.repaired_blocks, 0);
    assert_eq!(recovery.unrepaired_blocks, 2);
    assert!(!recovery.repair_possible);
    assert!(report
        .problems
        .iter()
        .any(|problem| problem.contains("unrepaired SQZ recovery block damage")));
}

#[test]
fn sqz_embedded_recovery_reports_over_limit_payload_damage() {
    let tmp = TempDir::new("sqz-recovery-over-limit");
    let project = tmp.path().join("project");
    fs::create_dir_all(&project).unwrap();
    fs::write(project.join("large.bin"), recovery_payload(8)).unwrap();

    let archive = tmp.path().join("over-limit.sqz");
    let eng = engine();
    let ctl = ControlToken::new();
    eng.create(
        &archive,
        &[project],
        &CreateOptions::default(),
        &NoProgress,
        &ctl,
    )
    .unwrap();

    let mut bytes = fs::read(&archive).unwrap();
    corrupt_payload_blocks(&mut bytes, &[0, 1, 2]);
    let corrupt = tmp.path().join("over-limit-corrupt.sqz");
    fs::write(&corrupt, bytes).unwrap();

    let report = eng
        .test(&corrupt, &OpenOptions::default(), &NoProgress, &ctl)
        .unwrap();
    assert!(!report.is_ok());
    let recovery = report.recovery.as_ref().expect("SQZ recovery summary");
    assert_eq!(recovery.scheme.as_str(), "sqz-embedded-rs-gf8");
    assert_eq!(recovery.damaged_blocks, 3);
    assert_eq!(recovery.repaired_blocks, 0);
    assert_eq!(recovery.unrepaired_blocks, 3);
    assert!(!recovery.repair_possible);
    assert!(report
        .problems
        .iter()
        .any(|p| p.contains("unrepaired SQZ recovery block damage")));

    let err = eng
        .extract(
            &corrupt,
            &tmp.path().join("strict-out"),
            None,
            &OpenOptions::default(),
            &ExtractOptions::default(),
            &NoProgress,
            &ctl,
        )
        .unwrap_err();
    assert!(matches!(err, FormatError::CorruptArchive(_)), "{err:?}");
}

#[test]
fn sqz_index_mirror_recovers_when_footer_index_is_damaged() {
    let tmp = TempDir::new("sqz-index-mirror");
    build_tree(tmp.path());
    let archive = tmp.path().join("out.sqz");
    let eng = engine();
    let ctl = ControlToken::new();
    eng.create(
        &archive,
        &[tmp.path().join("project")],
        &CreateOptions::default(),
        &NoProgress,
        &ctl,
    )
    .unwrap();

    let mut bytes = fs::read(&archive).unwrap();
    let fidx_positions = byte_pattern_positions(&bytes, b"FIDX");
    assert!(
        fidx_positions.len() >= 2,
        "recovery index mirror and primary footer index should both be present"
    );
    let primary_index = *fidx_positions.last().unwrap();
    bytes[primary_index] ^= 0x55;
    let damaged = tmp.path().join("damaged-index.sqz");
    fs::write(&damaged, bytes).unwrap();

    let entries = eng.list(&damaged, &OpenOptions::default()).unwrap();
    assert!(entries.iter().any(|e| e.path.display == "project/a.txt"));

    let out = tmp.path().join("out");
    eng.extract(
        &damaged,
        &out,
        None,
        &OpenOptions::default(),
        &ExtractOptions::default(),
        &NoProgress,
        &ctl,
    )
    .unwrap();
    assert_eq!(fs::read(out.join("project/a.txt")).unwrap(), b"hello sqz");
}

#[test]
fn sqz_index_corruption_fails_when_primary_mirror_and_protection_are_damaged() {
    let tmp = TempDir::new("sqz-index-both-damaged");
    build_tree(tmp.path());
    let archive = tmp.path().join("out.sqz");
    let eng = engine();
    let ctl = ControlToken::new();
    eng.create(
        &archive,
        &[tmp.path().join("project")],
        &CreateOptions::default(),
        &NoProgress,
        &ctl,
    )
    .unwrap();

    let mut bytes = fs::read(&archive).unwrap();
    let fidx_positions = byte_pattern_positions(&bytes, b"FIDX");
    assert!(fidx_positions.len() >= 2);
    for pos in fidx_positions {
        bytes[pos] ^= 0x55;
    }
    let protection_pos = byte_pattern_positions(&bytes, b"RSPC")
        .pop()
        .expect("recovery protection trailer found");
    bytes[protection_pos] ^= 0x55;
    let damaged = tmp.path().join("damaged-index-and-mirror.sqz");
    fs::write(&damaged, bytes).unwrap();

    let err = eng.list(&damaged, &OpenOptions::default()).unwrap_err();
    assert!(matches!(err, FormatError::CorruptArchive(_)), "{err:?}");
}
