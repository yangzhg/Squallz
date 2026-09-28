//! Metadata must survive both the default and optimized extraction paths.

mod common;

use std::fs;
use std::io::Cursor;
use std::process::Command;
use std::time::{Duration, UNIX_EPOCH};

use common::{command_exists, engine, TempDir};
use squallz_format_api::{
    ControlToken, CreateOptions, Detected, EntryMeta, EntryPath, EntryType, ExtractOptions,
    NoProgress, OpenOptions,
};

#[test]
fn file_and_directory_times_survive_all_shared_extraction_paths() {
    let tmp = TempDir::new("extract-metadata");
    for extension in ["zip", "tar", "7z", "sqz"] {
        let archive = tmp.path().join(format!("archive.{extension}"));
        let Some(Detected::Archive(format)) =
            squallz_formats::registry().detect_by_name(&format!("archive.{extension}"))
        else {
            panic!("missing {extension} format");
        };
        let mut writer = format
            .create(
                Box::new(fs::File::create(&archive).unwrap()),
                &CreateOptions::default(),
            )
            .unwrap();
        let precision = if matches!(extension, "zip" | "7z") {
            123_456_700
        } else {
            0
        };
        let entries: Vec<_> = ["folder", "folder/child", "folder/child/report.txt"]
            .into_iter()
            .enumerate()
            .map(|(index, name)| EntryMeta {
                path: EntryPath::from_utf8(name),
                entry_type: if index == 2 {
                    EntryType::File
                } else {
                    EntryType::Dir
                },
                size: if index == 2 { 7 } else { 0 },
                compressed_size: None,
                modified: Some(UNIX_EPOCH + Duration::new(1_714_979_291 + index as u64, precision)),
                unix_mode: Some(if index == 2 { 0o644 } else { 0o755 }),
                crc32: None,
                encrypted: false,
            })
            .collect();
        for meta in &entries {
            let mut payload = Cursor::new(b"payload");
            let data = matches!(meta.entry_type, EntryType::File)
                .then_some(&mut payload as &mut dyn std::io::Read);
            writer.add_entry(meta, data).unwrap();
        }
        writer.finish().unwrap();

        for best_effort in [false, true] {
            let destination = tmp.path().join(format!("{extension}-{best_effort}"));
            let report = engine()
                .extract_with_report(
                    &archive,
                    &destination,
                    None,
                    &OpenOptions::default(),
                    &ExtractOptions {
                        best_effort,
                        ..ExtractOptions::default()
                    },
                    &NoProgress,
                    &ControlToken::default(),
                )
                .unwrap();
            assert_eq!(report.created, 1, "{extension}");
            assert_eq!(report.directories, 2, "{extension}");
            assert_eq!(report.failed, 0, "{extension}");
            for meta in &entries {
                assert_eq!(
                    fs::metadata(destination.join(&meta.path.display))
                        .unwrap()
                        .modified()
                        .unwrap(),
                    meta.modified.unwrap(),
                    "{extension}: {}",
                    meta.path
                );
            }
            assert_eq!(
                fs::read(destination.join("folder/child/report.txt")).unwrap(),
                b"payload"
            );
        }
    }
}

#[test]
fn system_tar_and_sevenz_archives_extract_with_original_times() {
    let tmp = TempDir::new("extract-external-metadata");
    let source = tmp.path().join("report.txt");
    let expected = UNIX_EPOCH + Duration::from_secs(1_714_979_291);
    fs::write(&source, b"payload").unwrap();
    fs::File::options()
        .write(true)
        .open(&source)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(expected))
        .unwrap();
    for (tool, extension) in [("tar", "tar"), ("7zz", "7z")] {
        if !command_exists(tool) {
            eprintln!("skipping extraction metadata interop: {tool} unavailable");
            continue;
        }
        let archive = tmp.path().join(format!("theirs.{extension}"));
        let mut command = Command::new(tool);
        command.arg(if tool == "tar" { "-cf" } else { "a" });
        let output = command
            .arg(&archive)
            .arg("report.txt")
            .current_dir(tmp.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{tool}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let destination = tmp.path().join(format!("out-{extension}"));
        engine()
            .extract(
                &archive,
                &destination,
                None,
                &OpenOptions::default(),
                &ExtractOptions::default(),
                &NoProgress,
                &ControlToken::default(),
            )
            .unwrap();
        let output = destination.join("report.txt");
        assert_eq!(fs::read(&output).unwrap(), b"payload");
        assert_eq!(
            fs::metadata(output).unwrap().modified().unwrap(),
            expected,
            "{tool}"
        );
    }
}
