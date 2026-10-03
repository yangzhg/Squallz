//! UTC metadata, DOS local time and real Info-ZIP interoperability.

mod common;

use std::fs;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use common::{command_exists, engine, TempDir};
use squallz_format_api::{
    ArchiveStructureStatus, ControlToken, CreateOptions, Detected, EntryMeta, EntryPath,
    EntrySelection, EntryType, ExtractOptions, NoProgress, OpenOptions, UpdateOp, UpdateOptions,
};
use zip::write::FullFileOptions;

fn instant(seconds: i64) -> SystemTime {
    if seconds < 0 {
        UNIX_EPOCH - Duration::from_secs(seconds.unsigned_abs())
    } else {
        UNIX_EPOCH + Duration::from_secs(seconds as u64)
    }
}

fn create_with_time(path: &Path, modified: SystemTime) {
    create_with_times(path, &[("report.txt", modified)]);
}

fn create_with_times(path: &Path, entries: &[(&str, SystemTime)]) {
    let Some(Detected::Archive(format)) = squallz_formats::registry().detect_by_name("test.zip")
    else {
        panic!("ZIP format missing");
    };
    let mut writer = format
        .create(
            Box::new(fs::File::create(path).unwrap()),
            &CreateOptions::default(),
        )
        .unwrap();
    for &(name, modified) in entries {
        let is_dir = name.ends_with('/');
        let mut data = Cursor::new(b"payload");
        writer
            .add_entry(
                &EntryMeta {
                    path: EntryPath::from_utf8(name),
                    entry_type: if is_dir {
                        EntryType::Dir
                    } else {
                        EntryType::File
                    },
                    size: if is_dir { 0 } else { 7 },
                    compressed_size: None,
                    modified: Some(modified),
                    unix_mode: None,
                    crc32: None,
                    encrypted: false,
                },
                if is_dir { None } else { Some(&mut data) },
            )
            .unwrap();
    }
    writer.finish().unwrap();
}

fn write_fixture(path: &Path, options: FullFileOptions<'static>) {
    let mut writer = zip::ZipWriter::new(fs::File::create(path).unwrap());
    writer.start_file("report.txt", options).unwrap();
    writer.write_all(b"payload").unwrap();
    writer.finish().unwrap();
}

fn read_time(path: &Path) -> Option<SystemTime> {
    let entries = engine().list(path, &OpenOptions::default()).unwrap();
    assert_eq!(entries.len(), 1);
    entries[0].modified
}

fn recovered_copy(path: &Path) -> PathBuf {
    let bytes = fs::read(path).unwrap();
    // These fixtures have tiny known payloads without ZIP signatures.
    let central = bytes.windows(4).position(|w| w == b"PK\x01\x02").unwrap();
    let recovered = path.with_extension("recovered.zip");
    fs::write(&recovered, &bytes[..central]).unwrap();
    let (_, structure) = engine()
        .list_with_structure(&recovered, &OpenOptions::default())
        .unwrap();
    assert_eq!(structure, ArchiveStructureStatus::ZipLocalHeadersRecovered);
    recovered
}

fn assert_time_with_recovery(path: &Path, expected: Option<SystemTime>) {
    assert_eq!(read_time(path), expected);
    assert_eq!(read_time(&recovered_copy(path)), expected);
}

#[test]
fn utc_metadata_survives_creation_and_raw_copy_rename() {
    let tmp = TempDir::new("timestamps-update");
    let archive = tmp.path().join("time.zip");
    // Pre-1970 and post-2107 instants cannot be represented by DOS. Subseconds
    // round down to NTFS's 100 ns precision, including before the Unix epoch.
    for (original, expected) in [
        (
            instant(-1) + Duration::from_nanos(123_456_789),
            instant(-1) + Duration::from_nanos(123_456_700),
        ),
        (
            instant(1_714_979_291) + Duration::from_nanos(123_456_789),
            instant(1_714_979_291) + Duration::from_nanos(123_456_700),
        ),
        (instant(4_354_819_200), instant(4_354_819_200)),
    ] {
        create_with_time(&archive, original);
        assert_time_with_recovery(&archive, Some(expected));
        engine()
            .update(
                &archive,
                &[UpdateOp::Rename {
                    from: EntrySelection::Raw(EntryPath::from_utf8("report.txt")),
                    to: EntryPath::from_utf8("renamed.txt"),
                }],
                &UpdateOptions::default(),
                &NoProgress,
                &ControlToken::new(),
            )
            .unwrap();
        assert_time_with_recovery(&archive, Some(expected));
    }
}

#[test]
fn ntfs_precedes_unix_and_zero_ntfs_falls_back_to_signed_unix() {
    let tmp = TempDir::new("timestamps-precedence");
    let archive = tmp.path().join("time.zip");
    for ntfs_first in [false, true] {
        for ticks in [
            0u64,
            116_444_736_000_000_000 + 1_714_979_291 * 10_000_000 + 1_234_567,
        ] {
            let mut options = FullFileOptions::default();
            let mut ntfs = [0u8; 32];
            ntfs[4..6].copy_from_slice(&1u16.to_le_bytes());
            ntfs[6..8].copy_from_slice(&24u16.to_le_bytes());
            ntfs[8..16].copy_from_slice(&ticks.to_le_bytes());
            let unix = [1u8, 0xff, 0xff, 0xff, 0xff]; // -1 signed Unix second.
            if ntfs_first {
                options.add_extra_data(0x000a, ntfs, false).unwrap();
            }
            options.add_extra_data(0x5455, unix, false).unwrap();
            if !ntfs_first {
                options.add_extra_data(0x000a, ntfs, false).unwrap();
            }
            write_fixture(&archive, options);
            let expected = if ticks == 0 {
                instant(-1)
            } else {
                instant(1_714_979_291) + Duration::from_nanos(123_456_700)
            };
            assert_time_with_recovery(&archive, Some(expected));
        }
    }
}

#[cfg(unix)]
#[test]
fn dos_timestamps_use_historical_local_timezone() {
    // Isolate TZ in subprocesses; never mutate a multi-threaded test process's
    // environment or depend on the developer machine's configured time zone.
    let Ok(zone) = std::env::var("SQUALLZ_ZIP_TIME_TEST_ZONE") else {
        for zone in [
            "UTC",
            "Asia/Shanghai",
            "America/Los_Angeles",
            "Asia/Kathmandu",
        ] {
            let output = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "dos_timestamps_use_historical_local_timezone",
                    "--nocapture",
                ])
                .env("SQUALLZ_ZIP_TIME_TEST_ZONE", zone)
                .env("TZ", zone)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{zone}: {}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        return;
    };
    let offsets = match zone.as_str() {
        "UTC" => [0, 0],
        "Asia/Shanghai" => [8 * 3600, 8 * 3600],
        "America/Los_Angeles" => [-8 * 3600, -7 * 3600],
        "Asia/Kathmandu" => [5 * 3600 + 45 * 60; 2],
        _ => panic!("unexpected test zone"),
    };
    let tmp = TempDir::new("timestamps-dos");
    let archive = tmp.path().join("time.zip");
    for ((month, wall_seconds), offset) in [(1, 1_705_320_000), (7, 1_721_044_800)]
        .into_iter()
        .zip(offsets)
    {
        let dos = zip::DateTime::from_date_and_time(2024, month, 15, 12, 0, 0).unwrap();
        write_fixture(&archive, FullFileOptions::default().last_modified_time(dos));
        let expected = instant(wall_seconds - offset);
        assert_time_with_recovery(&archive, Some(expected));
        create_with_time(&archive, expected);
        let mut reader = zip::ZipArchive::new(fs::File::open(&archive).unwrap()).unwrap();
        assert_eq!(reader.by_index_raw(0).unwrap().last_modified(), Some(dos));
    }
    if zone == "America/Los_Angeles" {
        // Fall-back's repeated hour selects the first instant. Spring-forward's
        // missing hour stays unknown unless a UTC extra field is available.
        for (month, day, hour, expected) in
            [(11, 3, 1, Some(instant(1_730_626_200))), (3, 10, 2, None)]
        {
            let dos = zip::DateTime::from_date_and_time(2024, month, day, hour, 30, 0).unwrap();
            write_fixture(&archive, FullFileOptions::default().last_modified_time(dos));
            assert_time_with_recovery(&archive, expected);
        }
        create_with_time(&archive, instant(1_730_629_800)); // Second 01:30.
        assert_time_with_recovery(&archive, Some(instant(1_730_629_800)));
    }
}

#[test]
fn recovered_file_and_directory_times_survive_extraction_and_conversion() {
    let tmp = TempDir::new("timestamps-recovered");
    let file_time = instant(1_714_979_291) + Duration::from_nanos(123_456_700);
    let dir_time = instant(1_705_320_001);
    let archive = tmp.path().join("original.zip");
    create_with_times(
        &archive,
        &[
            ("documents/", dir_time),
            ("documents/report.txt", file_time),
        ],
    );
    let recovered = recovered_copy(&archive);
    let outcome = engine()
        .test_summary_with_structure(
            &recovered,
            &OpenOptions::default(),
            &squallz_format_api::SafetyLimits::default(),
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap();
    assert_eq!(
        outcome.structure,
        ArchiveStructureStatus::ZipLocalHeadersRecovered
    );
    assert_eq!(outcome.payload_problem_count(), 0);
    assert!(!outcome.summary.is_ok());

    let converted = tmp.path().join("converted.zip");
    engine()
        .convert(
            &recovered,
            &converted,
            &OpenOptions::default(),
            &CreateOptions::default(),
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap();
    for (path, structure, folder) in [
        (
            &recovered,
            ArchiveStructureStatus::ZipLocalHeadersRecovered,
            "recovered",
        ),
        (&converted, ArchiveStructureStatus::Complete, "converted"),
    ] {
        let (entries, actual_structure) = engine()
            .list_with_structure(path, &OpenOptions::default())
            .unwrap();
        assert_eq!(actual_structure, structure);
        assert_eq!(entries.len(), 2);
        for entry in entries {
            let expected = match entry.path.display.as_str() {
                "documents/" => dir_time,
                "documents/report.txt" => file_time,
                other => panic!("unexpected entry {other}"),
            };
            assert_eq!(entry.modified, Some(expected));
        }
        let dest = tmp.path().join(folder);
        engine()
            .extract(
                path,
                &dest,
                None,
                &OpenOptions::default(),
                &ExtractOptions::default(),
                &NoProgress,
                &ControlToken::new(),
            )
            .unwrap();
        assert_eq!(
            fs::read(dest.join("documents/report.txt")).unwrap(),
            b"payload"
        );
        for (name, expected) in [("documents", dir_time), ("documents/report.txt", file_time)] {
            assert_eq!(
                fs::metadata(dest.join(name)).unwrap().modified().unwrap(),
                expected
            );
        }
    }
}

#[test]
fn interop_infozip_and_unzip_preserve_modification_instant() {
    if !command_exists("zip") || !command_exists("unzip") {
        eprintln!("skipping timestamp interop: system zip/unzip unavailable");
        return;
    }
    let tmp = TempDir::new("timestamps-interop");
    let source = tmp.path().join("report.txt");
    let expected = instant(1_714_979_291); // Odd second needs an extra field.
    fs::write(&source, b"payload").unwrap();
    fs::File::options()
        .write(true)
        .open(&source)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(expected))
        .unwrap();
    let archive = tmp.path().join("theirs.zip");
    let output = Command::new("zip")
        .args(["-q"])
        .arg(&archive)
        .arg("report.txt")
        .current_dir(tmp.path())
        .env("TZ", "America/Los_Angeles")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let recovered = recovered_copy(&archive);
    for (path, folder) in [
        (&archive, "squallz-extracted"),
        (&recovered, "squallz-recovered"),
    ] {
        assert_eq!(read_time(path), Some(expected));
        let extracted = tmp.path().join(folder);
        engine()
            .extract(
                path,
                &extracted,
                None,
                &OpenOptions::default(),
                &ExtractOptions::default(),
                &NoProgress,
                &ControlToken::default(),
            )
            .unwrap();
        assert_eq!(fs::read(extracted.join("report.txt")).unwrap(), b"payload");
        assert_eq!(
            fs::metadata(extracted.join("report.txt"))
                .unwrap()
                .modified()
                .unwrap(),
            expected
        );
    }

    let ours = tmp.path().join("ours.zip");
    engine()
        .create(
            &ours,
            &[source],
            &CreateOptions::default(),
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap();
    let converted = tmp.path().join("converted.zip");
    engine()
        .convert(
            &recovered,
            &converted,
            &OpenOptions::default(),
            &CreateOptions::default(),
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap();
    for (path, folder) in [(&ours, "extracted"), (&converted, "converted-extracted")] {
        let dest = tmp.path().join(folder);
        let output = Command::new("unzip")
            .arg("-q")
            .arg(path)
            .arg("-d")
            .arg(&dest)
            .env("TZ", "UTC")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(fs::read(dest.join("report.txt")).unwrap(), b"payload");
        assert_eq!(
            fs::metadata(dest.join("report.txt"))
                .unwrap()
                .modified()
                .unwrap(),
            expected
        );
    }
}
