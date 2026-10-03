//! ZIP update tests: add/delete/rename individually and combined, system
//! `unzip -t` interop, encrypted archives (raw copy without the password),
//! atomicity on failure.

mod common;

use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Barrier, Mutex};
use std::time::{Duration, Instant};

use common::{build_stored_zip, command_exists, engine, RawZipEntry, TempDir};
use squallz_core::api::{
    CompressionLevel, ControlToken, CreateOptions, EntryMeta, EntryPath, EntrySelection,
    FormatError, NoProgress, OpenOptions, Password, ProgressPhase, ProgressSink, UpdateOp,
    UpdateOptions,
};
use squallz_core::ArchiveUpdateGuard;

/// Builds a base archive with project/a.txt, project/sub/b.txt, project/c.log.
fn base_archive(dir: &Path, password: Option<&str>) -> PathBuf {
    let root = dir.join("project");
    fs::create_dir_all(root.join("sub")).unwrap();
    fs::write(root.join("a.txt"), b"alpha").unwrap();
    fs::write(root.join("sub/b.txt"), b"bravo").unwrap();
    fs::write(root.join("c.log"), b"log line").unwrap();
    let dest = dir.join("base.zip");
    let opts = CreateOptions {
        password: password.map(Password::new),
        ..CreateOptions::default()
    };
    engine()
        .create(&dest, &[root], &opts, &NoProgress, &ControlToken::new())
        .unwrap();
    dest
}

fn named_archive(dir: &Path, names: &[&str]) -> PathBuf {
    let archive = dir.join("named.zip");
    let mut writer = zip::ZipWriter::new(fs::File::create(&archive).unwrap());
    for name in names {
        let options = zip::write::SimpleFileOptions::default();
        if name.ends_with('/') {
            writer.add_directory(*name, options).unwrap();
        } else {
            writer.start_file(*name, options).unwrap();
            writer.write_all(name.as_bytes()).unwrap();
        }
    }
    writer.finish().unwrap();
    archive
}

#[derive(Debug, PartialEq, Eq)]
struct RawEntrySnapshot {
    name: Vec<u8>,
    payload: Vec<u8>,
    size: u64,
    crc32: u32,
    encrypted: bool,
    compression: zip::CompressionMethod,
    unix_mode: Option<u32>,
}

#[derive(Default)]
struct EntryTrace(Mutex<Vec<EntryPath>>);

impl ProgressSink for EntryTrace {
    fn on_progress(&self, _done: u64, _total: u64, current: &EntryPath) {
        if !current.raw.is_empty() {
            self.0.lock().unwrap().push(current.clone());
        }
    }
}

#[test]
fn update_encoding_drives_directory_selection_globs_conflicts_and_progress() {
    for (encoding, directory, file_name) in [
        (encoding_rs::GBK, "你好[1]", "目录.txt"),
        (encoding_rs::GBK, "目录[1]", "你.txt"),
        (encoding_rs::SHIFT_JIS, "日本語[1]", "表.txt"),
        (encoding_rs::BIG5, "目錄[1]", "書.txt"),
    ] {
        for explicit_directory in [false, true] {
            let tmp = TempDir::new("update-display-encoding");
            let archive = tmp.path().join("legacy.zip");
            let source = format!("{directory}/{file_name}");
            let mut names = vec![
                source.clone(),
                format!("{directory}/削除.log"),
                "保留.txt".into(),
            ];
            if explicit_directory {
                names.push(format!("{directory}/"));
            }
            let entries: Vec<_> = names
                .iter()
                .map(|name| {
                    let (raw, _, errors) = encoding.encode(name);
                    assert!(!errors, "{name}");
                    RawZipEntry {
                        name: raw.into_owned(),
                        data: if name.ends_with('/') {
                            Vec::new()
                        } else {
                            b"unchanged contents".to_vec()
                        },
                    }
                })
                .collect();
            let original = build_stored_zip(&entries);
            fs::write(&archive, &original).unwrap();
            let options = UpdateOptions {
                encoding_override: Some(encoding.name().into()),
                ..Default::default()
            };
            let error = run_update(
                &archive,
                &[UpdateOp::Rename {
                    from: EntrySelection::Display(source.clone()),
                    to: EntryPath::from_utf8("保留.txt"),
                }],
                &options,
            )
            .unwrap_err();
            assert_other_contains(error, "already exists");
            assert_eq!(fs::read(&archive).unwrap(), original);
            let mut expected = raw_entry_snapshots(&archive);
            expected.retain(|entry| entry.name != entries[1].name);
            expected[0].name = format!("整理/{file_name}").into_bytes();
            if explicit_directory {
                expected.last_mut().unwrap().name = "整理/".as_bytes().to_vec();
            }
            let progress = EntryTrace::default();
            engine()
                .update(
                    &archive,
                    &[
                        UpdateOp::Rename {
                            from: EntrySelection::Display(directory.into()),
                            to: EntryPath::from_utf8("整理/"),
                        },
                        UpdateOp::Delete {
                            pattern: "*除.log".into(),
                        },
                    ],
                    &options,
                    &progress,
                    &ControlToken::new(),
                )
                .unwrap();
            assert_eq!(raw_entry_snapshots(&archive), expected);
            let trace = progress.0.lock().unwrap();
            let current = trace
                .iter()
                .find(|path| path.raw == entries[0].name)
                .unwrap();
            assert_eq!(current.display, source);
            assert_eq!(current.encoding, encoding.name());
            assert_unzip_t(&archive);
            assert_no_update_temp(tmp.path());
        }
    }
}

#[test]
fn update_display_directory_deletion_preserves_backslash_path_boundaries() {
    for explicit_directory in [false, true] {
        let tmp = TempDir::new("update-backslash-directory");
        let archive = tmp.path().join("legacy.zip");
        let mut names = vec!["docs\\表.txt", "docs\\sub\\keep.txt", "docs-old\\表.txt"];
        if explicit_directory {
            names.push("docs\\");
        }
        let entries: Vec<_> = names
            .iter()
            .map(|name| RawZipEntry {
                name: encoding_rs::SHIFT_JIS.encode(name).0.into_owned(),
                data: if name.ends_with('\\') {
                    Vec::new()
                } else {
                    b"contents".to_vec()
                },
            })
            .collect();
        let original = build_stored_zip(&entries);
        fs::write(&archive, &original).unwrap();
        let options = UpdateOptions {
            encoding_override: Some("shift_jis".into()),
            ..Default::default()
        };
        // The second byte of the Japanese character is also 0x5c, but it
        // is not a path separator and must not produce another directory.
        assert_other_contains(
            run_update(
                &archive,
                &[UpdateOp::DeleteEntry {
                    path: EntrySelection::Display("docs/表/".into()),
                }],
                &options,
            )
            .unwrap_err(),
            "not found",
        );
        assert_eq!(fs::read(&archive).unwrap(), original);
        assert!(matches!(run_update(
        &archive,
        &[UpdateOp::Rename {
            from: EntrySelection::Display("docs/".into()),
            to: EntryPath::from_utf8("moved/"),
        }],
        &options,
    ), Err(FormatError::Unsupported(message)) if message.contains("separators")));
        assert_eq!(fs::read(&archive).unwrap(), original);
        let expected: Vec<_> = raw_entry_snapshots(&archive)
            .into_iter()
            .filter(|entry| entry.name == entries[2].name)
            .collect();
        assert_eq!(expected.len(), 1);
        run_update(
            &archive,
            &[UpdateOp::DeleteEntry {
                path: EntrySelection::Display("docs/".into()),
            }],
            &options,
        )
        .unwrap();
        assert_eq!(raw_entry_snapshots(&archive), expected);
        assert_no_update_temp(tmp.path());
    }
}

#[test]
fn update_display_selection_rejects_ambiguous_and_undecodable_paths_atomically() {
    let tmp = TempDir::new("update-display-ambiguity");
    let archive = tmp.path().join("legacy.zip");
    let original = build_stored_zip(&[
        RawZipEntry {
            name: b"folder/\xff.txt".to_vec(),
            data: b"first".to_vec(),
        },
        RawZipEntry {
            name: b"folder/\xfe.txt".to_vec(),
            data: b"second".to_vec(),
        },
    ]);
    fs::write(&archive, &original).unwrap();
    let entries = engine()
        .list(
            &archive,
            &OpenOptions {
                encoding_override: Some("gbk".into()),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(entries[0].path.display, entries[1].path.display);
    let options = UpdateOptions {
        encoding_override: Some("gbk".into()),
        ..Default::default()
    };
    for source in [entries[0].path.display.as_str(), "folder/"] {
        for operation in [
            UpdateOp::DeleteEntry {
                path: EntrySelection::Display(source.into()),
            },
            UpdateOp::Rename {
                from: EntrySelection::Display(source.into()),
                to: EntryPath::from_utf8("target"),
            },
        ] {
            assert_other_contains(
                run_update(&archive, &[operation], &options).unwrap_err(),
                "ambiguous",
            );
            assert_eq!(fs::read(&archive).unwrap(), original);
        }
    }
    let original = build_stored_zip(&[RawZipEntry {
        name: b"folder/\xff.txt".to_vec(),
        data: b"invalid name".to_vec(),
    }]);
    fs::write(&archive, &original).unwrap();
    assert!(matches!(run_update(&archive, &[UpdateOp::Rename {
        from: EntrySelection::Display("folder/".into()), to: EntryPath::from_utf8("target/"),
    }], &options), Err(FormatError::Unsupported(message)) if message.contains("encoding")));
    assert_eq!(fs::read(&archive).unwrap(), original);
    assert_no_update_temp(tmp.path());
}

fn raw_entry_snapshots(path: &Path) -> Vec<RawEntrySnapshot> {
    let mut headers = fs::File::open(path).unwrap();
    let mut archive = zip::ZipArchive::new(fs::File::open(path).unwrap()).unwrap();
    (0..archive.len())
        .map(|index| {
            let mut entry = archive.by_index_raw(index).unwrap();
            headers.seek(SeekFrom::Start(entry.header_start())).unwrap();
            let mut header = [0; 30];
            headers.read_exact(&mut header).unwrap();
            let flags = u16::from_le_bytes([header[6], header[7]]);
            let length = u16::from_le_bytes([header[26], header[27]]);
            let mut local_name = vec![0; usize::from(length)];
            headers.read_exact(&mut local_name).unwrap();
            assert_eq!(
                local_name,
                entry.name_raw(),
                "local and central names differ"
            );
            if std::str::from_utf8(entry.name_raw()).is_err() {
                assert_eq!(flags & 0x800, 0, "legacy name must not be marked UTF-8");
            }
            let mut snapshot = RawEntrySnapshot {
                name: entry.name_raw().to_vec(),
                payload: Vec::new(),
                size: entry.size(),
                crc32: entry.crc32(),
                encrypted: entry.encrypted(),
                compression: entry.compression(),
                unix_mode: entry.unix_mode(),
            };
            entry.read_to_end(&mut snapshot.payload).unwrap();
            snapshot
        })
        .collect()
}

#[test]
fn update_retains_zipcrypto_payload_and_password_check() {
    use zip::unstable::write::FileOptionsExt;

    let tmp = TempDir::new("update-zipcrypto-copy");
    let archive = tmp.path().join("encrypted.zip");
    let mut writer = zip::ZipWriter::new(fs::File::create(&archive).unwrap());
    let options = zip::write::SimpleFileOptions::default()
        .with_deprecated_encryption(b"test password")
        .unwrap();
    writer.start_file("keep.txt", options).unwrap();
    writer.write_all(b"encrypted contents").unwrap();
    writer.start_file("drop.txt", options).unwrap();
    writer.write_all(b"remove me").unwrap();
    writer.finish().unwrap();
    let mut before = raw_entry_snapshots(&archive);
    before.retain(|entry| entry.name == b"keep.txt");
    run_update(
        &archive,
        &[UpdateOp::DeleteEntry {
            path: EntrySelection::Raw(EntryPath::from_utf8("drop.txt")),
        }],
        &UpdateOptions::default(),
    )
    .unwrap();
    assert_eq!(raw_entry_snapshots(&archive), before);
    let mut reader = zip::ZipArchive::new(fs::File::open(&archive).unwrap()).unwrap();
    let mut contents = Vec::new();
    reader
        .by_index_decrypt(0, b"test password")
        .unwrap()
        .read_to_end(&mut contents)
        .unwrap();
    assert_eq!(contents, b"encrypted contents");
    let wrong_password = reader
        .by_index_decrypt(0, b"wrong password")
        .and_then(|mut file| {
            std::io::copy(&mut file, &mut std::io::sink()).map_err(zip::result::ZipError::Io)
        });
    assert!(wrong_password.is_err());
    if command_exists("unzip") {
        let output = Command::new("unzip")
            .args(["-P", "test password", "-t"])
            .arg(&archive)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
    }
}

#[test]
fn update_preserves_mixed_raw_names_without_lossy_identity_collisions() {
    let tmp = TempDir::new("update-mixed-names");
    let archive = tmp.path().join("mixed.zip");
    let names = [
        vec![0xff, b'.', b't', b'x', b't'],
        vec![0xfe, b'.', b't', b'x', b't'],
        encoding_rs::GBK
            .encode("压缩资料中文文件名称.txt")
            .0
            .into_owned(),
        encoding_rs::SHIFT_JIS
            .encode("日本語のファイル名.txt")
            .0
            .into_owned(),
        encoding_rs::BIG5
            .encode("傳統中文檔案名稱.txt")
            .0
            .into_owned(),
        "UTF-8资料.txt".as_bytes().to_vec(),
        b"drop.txt".to_vec(),
    ];
    let entries: Vec<_> = names
        .iter()
        .enumerate()
        .map(|(index, name)| RawZipEntry {
            name: name.clone(),
            data: format!("contents {index}").into_bytes(),
        })
        .collect();
    fs::write(&archive, build_stored_zip(&entries)).unwrap();
    let mut expected = raw_entry_snapshots(&archive);
    expected.retain(|entry| entry.name != b"drop.txt");
    expected[0].name = b"renamed.txt".to_vec();
    run_update(
        &archive,
        &[
            UpdateOp::DeleteEntry {
                path: EntrySelection::Raw(EntryPath::from_utf8("drop.txt")),
            },
            UpdateOp::Rename {
                from: EntrySelection::Raw(EntryPath::from_raw(
                    names[0].clone(),
                    "unreadable.txt".into(),
                    "utf-8",
                )),
                to: EntryPath::from_utf8("renamed.txt"),
            },
        ],
        &UpdateOptions::default(),
    )
    .unwrap();
    assert_eq!(raw_entry_snapshots(&archive), expected);
    assert_unzip_t(&archive);
}

#[test]
fn update_renames_legacy_subtrees_to_utf8_and_keeps_other_names() {
    let tmp = TempDir::new("update-legacy-subtree");
    let archive = tmp.path().join("legacy.zip");
    let names = [
        "压缩资料目录/",
        "压缩资料目录/中文文件名称.txt",
        "压缩资料目录/sub/子文件.txt",
        "保留原始名称.txt",
    ];
    let entries: Vec<_> = names
        .iter()
        .map(|name| RawZipEntry {
            name: encoding_rs::GBK.encode(name).0.into_owned(),
            data: if name.ends_with('/') {
                Vec::new()
            } else {
                b"contents".to_vec()
            },
        })
        .collect();
    fs::write(&archive, build_stored_zip(&entries)).unwrap();
    let mut expected = raw_entry_snapshots(&archive);
    for (entry, name) in expected.iter_mut().zip(names) {
        if let Some(suffix) = name.strip_prefix("压缩资料目录/") {
            entry.name = format!("新的目录/{suffix}").into_bytes();
        }
    }
    let from = EntryPath::from_raw(entries[0].name.clone(), names[0].into(), "GBK");
    let before = fs::read(&archive).unwrap();
    for target in ["压缩资料目录/", "压缩资料目录/inside/"] {
        assert!(run_update(
            &archive,
            &[UpdateOp::Rename {
                from: EntrySelection::Raw(from.clone()),
                to: EntryPath::from_utf8(target)
            }],
            &UpdateOptions::default()
        )
        .is_err());
        assert_eq!(fs::read(&archive).unwrap(), before);
    }
    run_update(
        &archive,
        &[UpdateOp::Rename {
            from: EntrySelection::Raw(from),
            to: EntryPath::from_utf8("新的目录/"),
        }],
        &UpdateOptions::default(),
    )
    .unwrap();
    assert_eq!(raw_entry_snapshots(&archive), expected);
    assert_unzip_t(&archive);
}

#[test]
fn update_rejects_legacy_display_name_collisions_and_undecodable_renames() {
    let tmp = TempDir::new("update-legacy-conflicts");
    let archive = tmp.path().join("legacy.zip");
    let displayed = "压缩文件中文名称测试.txt";
    let entries = [
        RawZipEntry {
            name: encoding_rs::GBK.encode(displayed).0.into_owned(),
            data: b"keep".to_vec(),
        },
        RawZipEntry {
            name: b"move.txt".to_vec(),
            data: b"move".to_vec(),
        },
    ];
    let before = build_stored_zip(&entries);
    fs::write(&archive, &before).unwrap();
    let error = run_update(
        &archive,
        &[UpdateOp::Rename {
            from: EntrySelection::Raw(EntryPath::from_utf8("move.txt")),
            to: EntryPath::from_utf8(displayed),
        }],
        &UpdateOptions::default(),
    )
    .unwrap_err();
    assert_other_contains(error, "already exists");
    assert_eq!(fs::read(&archive).unwrap(), before);
    let before = build_stored_zip(&[RawZipEntry {
        name: b"folder/\xff.txt".to_vec(),
        data: b"invalid name".to_vec(),
    }]);
    fs::write(&archive, &before).unwrap();
    let error = run_update(
        &archive,
        &[UpdateOp::Rename {
            from: EntrySelection::Raw(EntryPath::from_raw(
                b"folder/".to_vec(),
                "folder/".into(),
                "GBK",
            )),
            to: EntryPath::from_utf8("target/"),
        }],
        &UpdateOptions::default(),
    )
    .unwrap_err();
    assert!(matches!(error, FormatError::Unsupported(message) if message.contains("encoding")));
    assert_eq!(fs::read(&archive).unwrap(), before);
    assert_no_update_temp(tmp.path());
}

#[test]
fn raw_copy_touch_preserves_legacy_names_and_applies_metadata() {
    let tmp = TempDir::new("raw-copy-touch");
    let archive = tmp.path().join("source.zip");
    let output = tmp.path().join("touched.zip");
    let name = encoding_rs::GBK.encode("中文名称.txt").0.into_owned();
    fs::write(
        &archive,
        build_stored_zip(&[RawZipEntry {
            name: name.clone(),
            data: b"unchanged contents".to_vec(),
        }]),
    )
    .unwrap();
    let mut reader = zip::ZipArchive::new(fs::File::open(&archive).unwrap()).unwrap();
    let mut writer = zip::ZipWriter::new(fs::File::create(&output).unwrap());
    let modified = zip::DateTime::from_date_and_time(2026, 9, 28, 12, 30, 0).unwrap();
    writer
        .raw_copy_file_touch(reader.by_index_raw(0).unwrap(), modified, Some(0o600))
        .unwrap();
    writer.finish().unwrap();
    let mut reader = zip::ZipArchive::new(fs::File::open(&output).unwrap()).unwrap();
    let file = reader.by_index_raw(0).unwrap();
    assert_eq!(file.name_raw(), name);
    assert_eq!(file.unix_mode(), Some(0o100600));
    assert_eq!(file.last_modified(), Some(modified));
    let mut expected = raw_entry_snapshots(&archive);
    expected[0].unix_mode = Some(0o100600);
    assert_eq!(raw_entry_snapshots(&output), expected);
    assert_unzip_t(&output);
}

#[test]
fn update_preserves_encrypted_legacy_names_and_payloads() {
    use zip::unstable::write::FileOptionsExt;

    for (aes, large_file) in [(false, false), (true, false), (false, true), (true, true)] {
        let tmp = TempDir::new("update-encrypted-legacy");
        let archive = tmp.path().join("legacy.zip");
        let displayed = "压缩文件中文名称测试.txt";
        let raw_name = encoding_rs::GBK.encode(displayed).0.into_owned();
        let mut writer = zip::ZipWriter::new(fs::File::create(&archive).unwrap());
        let options = zip::write::SimpleFileOptions::default().large_file(large_file);
        let options = if aes {
            options.with_aes_encryption(zip::AesMode::Aes256, "test password")
        } else {
            options
                .with_deprecated_encryption(b"test password")
                .unwrap()
        };
        writer
            .start_file("x".repeat(raw_name.len()), options)
            .unwrap();
        writer.write_all(b"encrypted contents").unwrap();
        writer.start_file("drop.txt", options).unwrap();
        writer.write_all(b"remove me").unwrap();
        writer.finish().unwrap();
        let offsets = {
            let mut reader = zip::ZipArchive::new(fs::File::open(&archive).unwrap()).unwrap();
            let file = reader.by_index_raw(0).unwrap();
            [file.header_start() + 30, file.central_header_start() + 46]
        };
        let mut headers = fs::OpenOptions::new().write(true).open(&archive).unwrap();
        for offset in offsets {
            headers.seek(SeekFrom::Start(offset)).unwrap();
            headers.write_all(&raw_name).unwrap();
        }
        drop(headers);
        let mut expected = raw_entry_snapshots(&archive);
        expected.retain(|entry| entry.name == raw_name);
        run_update(
            &archive,
            &[UpdateOp::DeleteEntry {
                path: EntrySelection::Raw(EntryPath::from_utf8("drop.txt")),
            }],
            &UpdateOptions::default(),
        )
        .unwrap();
        assert_eq!(raw_entry_snapshots(&archive), expected);
        let options = OpenOptions {
            encoding_override: Some("gbk".into()),
            password: Some(Password::new("test password")),
        };
        let entries = engine().list(&archive, &options).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path.display, displayed);
        assert_eq!(entries[0].path.raw, raw_name);
        assert!(entries[0].encrypted);
        let mut reader = engine().open(&archive, &options).unwrap();
        let mut contents = Vec::new();
        reader
            .read_entry(&entries[0].path, &mut |entry| {
                entry.read_to_end(&mut contents)?;
                Ok(())
            })
            .unwrap();
        assert_eq!(contents, b"encrypted contents");
    }
}

#[test]
fn update_preserves_unix_types_and_permissions() {
    let tmp = TempDir::new("update-entry-attributes");
    let archive = tmp.path().join("attributes.zip");
    let mut writer = zip::ZipWriter::new(fs::File::create(&archive).unwrap());
    writer
        .add_directory(
            "bin/",
            zip::write::SimpleFileOptions::default().unix_permissions(0o750),
        )
        .unwrap();
    writer
        .start_file(
            "bin/run",
            zip::write::SimpleFileOptions::default().unix_permissions(0o755),
        )
        .unwrap();
    writer.write_all(b"program contents").unwrap();
    writer
        .add_symlink(
            "link",
            "bin/run",
            zip::write::SimpleFileOptions::default().unix_permissions(0o777),
        )
        .unwrap();
    writer.finish().unwrap();
    let mut expected = raw_entry_snapshots(&archive);
    expected[2].name = b"renamed-link".to_vec();
    run_update(
        &archive,
        &[UpdateOp::Rename {
            from: EntrySelection::Raw(EntryPath::from_utf8("link")),
            to: EntryPath::from_utf8("renamed-link"),
        }],
        &UpdateOptions::default(),
    )
    .unwrap();
    assert_eq!(raw_entry_snapshots(&archive), expected);
    let entries = engine().list(&archive, &OpenOptions::default()).unwrap();
    assert!(
        matches!(&entries.iter().find(|entry| entry.path.display == "renamed-link").unwrap().entry_type,
        squallz_core::api::EntryType::Symlink { target } if target == b"bin/run")
    );
    assert_unzip_t(&archive);
}

const LARGE_RAW_COPY_ENTRY: &str = "raw-copy/large.bin";

fn large_stored_archive(dir: &Path) -> PathBuf {
    let root = dir.join("raw-copy");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("large.bin"), vec![b'R'; 2 * 1024 * 1024]).unwrap();
    fs::write(root.join("remove.txt"), b"remove me").unwrap();
    let dest = dir.join("large.zip");
    let options = CreateOptions {
        level: CompressionLevel::Store,
        ..CreateOptions::default()
    };
    engine()
        .create(&dest, &[root], &options, &NoProgress, &ControlToken::new())
        .unwrap();
    dest
}

fn list_names(path: &Path, password: Option<&str>) -> Vec<String> {
    let opts = OpenOptions {
        password: password.map(Password::new),
        encoding_override: None,
    };
    let mut names: Vec<String> = engine()
        .list(path, &opts)
        .unwrap()
        .iter()
        .map(|e: &EntryMeta| e.path.display.clone())
        .collect();
    names.sort();
    names
}

fn run_update(path: &Path, ops: &[UpdateOp], opts: &UpdateOptions) -> Result<(), FormatError> {
    engine().update(path, ops, opts, &NoProgress, &ControlToken::new())
}

fn assert_other_contains(err: FormatError, needle: &str) {
    match err {
        FormatError::Other(msg) => assert!(msg.contains(needle), "{msg}"),
        other => panic!("expected FormatError::Other containing {needle}, got {other:?}"),
    }
}

/// `unzip -t` interop check (skipped when unzip is unavailable).
fn assert_unzip_t(path: &Path) {
    if !command_exists("unzip") {
        eprintln!("skipping unzip -t check: unzip not on PATH");
        return;
    }
    let out = Command::new("unzip").arg("-t").arg(path).output().unwrap();
    assert!(
        out.status.success(),
        "unzip -t failed:\n{}",
        String::from_utf8_lossy(&out.stdout)
    );
}

enum MutationPoint {
    RewriteStarted,
    RawCopyStarted,
    EntryRead(String),
}

struct OnceOnProgress<F> {
    point: MutationPoint,
    action: Mutex<Option<F>>,
}

impl<F> OnceOnProgress<F> {
    fn rewrite_started(action: F) -> Self {
        Self {
            point: MutationPoint::RewriteStarted,
            action: Mutex::new(Some(action)),
        }
    }

    fn raw_copy_started(action: F) -> Self {
        Self {
            point: MutationPoint::RawCopyStarted,
            action: Mutex::new(Some(action)),
        }
    }

    fn entry_read(path: &str, action: F) -> Self {
        Self {
            point: MutationPoint::EntryRead(path.to_owned()),
            action: Mutex::new(Some(action)),
        }
    }
}

impl<F: FnOnce() + Send> OnceOnProgress<F> {
    fn run(&self) {
        if let Some(action) = self.action.lock().unwrap().take() {
            action();
        }
    }
}

impl<F: FnOnce() + Send> ProgressSink for OnceOnProgress<F> {
    fn on_progress(&self, _done: u64, _total: u64, current: &EntryPath) {
        if matches!(self.point, MutationPoint::RewriteStarted)
            || (matches!(self.point, MutationPoint::RawCopyStarted) && !current.display.is_empty())
        {
            self.run();
        }
    }

    fn on_entry_progress(
        &self,
        _done: u64,
        _total: u64,
        current: &EntryPath,
        current_done: u64,
        _current_total: u64,
    ) {
        if current_done > 0
            && matches!(&self.point, MutationPoint::EntryRead(path) if path == &current.display)
        {
            self.run();
        }
    }
}

const PROCESS_WORKER_ROLE: &str = "SQUALLZ_ZIP_UPDATE_WORKER_ROLE";
const PROCESS_WORKER_ROOT: &str = "SQUALLZ_ZIP_UPDATE_WORKER_ROOT";
const PROCESS_WAIT_TIMEOUT: Duration = Duration::from_secs(30);
const LOCK_OBSERVATION_WINDOW: Duration = Duration::from_millis(750);
const MARKER_POLL_INTERVAL: Duration = Duration::from_millis(10);

struct WorkerProgress {
    archive: PathBuf,
    entered: PathBuf,
    release: Option<PathBuf>,
    required_entry: Option<&'static str>,
    fired: AtomicBool,
}

struct CancelOnScan {
    ctl: Arc<ControlToken>,
    scanned: AtomicU64,
    byte_progress: AtomicBool,
}

struct CancelDuringRawCopy {
    ctl: Arc<ControlToken>,
    fired: AtomicBool,
    observed: Mutex<Option<(u64, u64)>>,
}

struct PauseDuringRawCopy {
    ctl: Arc<ControlToken>,
    fired: AtomicBool,
    events: Arc<AtomicU64>,
    reached: mpsc::SyncSender<(u64, u64)>,
}

struct ResumeOnDrop(Arc<ControlToken>);

impl Drop for ResumeOnDrop {
    fn drop(&mut self) {
        self.0.resume();
    }
}

#[derive(Default)]
struct PhaseTrace {
    phases: Mutex<Vec<(ProgressPhase, bool)>>,
    byte_events: Mutex<Vec<(ProgressPhase, u64, u64)>>,
}

impl ProgressSink for PhaseTrace {
    fn on_progress(&self, done: u64, total: u64, _current: &EntryPath) {
        let phase = self.phases.lock().unwrap().last().map(|event| event.0);
        if let Some(phase) = phase {
            self.byte_events.lock().unwrap().push((phase, done, total));
        }
    }

    fn on_phase(&self, phase: ProgressPhase, interruptible: bool) {
        self.phases.lock().unwrap().push((phase, interruptible));
    }
}

impl ProgressSink for CancelOnScan {
    fn on_progress(&self, _done: u64, _total: u64, _current: &EntryPath) {
        self.byte_progress.store(true, Ordering::SeqCst);
    }

    fn on_scan_progress(&self, entries: u64, _current: &EntryPath) {
        self.scanned.store(entries, Ordering::SeqCst);
        if entries == 2 {
            self.ctl.cancel();
        }
    }
}

impl ProgressSink for CancelDuringRawCopy {
    fn on_progress(&self, _done: u64, _total: u64, _current: &EntryPath) {}

    fn on_entry_progress(
        &self,
        _done: u64,
        _total: u64,
        current: &EntryPath,
        current_done: u64,
        current_total: u64,
    ) {
        if current.display == LARGE_RAW_COPY_ENTRY
            && current_done > 0
            && current_done < current_total
            && !self.fired.swap(true, Ordering::SeqCst)
        {
            *self.observed.lock().unwrap() = Some((current_done, current_total));
            self.ctl.cancel();
        }
    }
}

impl ProgressSink for PauseDuringRawCopy {
    fn on_progress(&self, _done: u64, _total: u64, _current: &EntryPath) {}

    fn on_entry_progress(
        &self,
        _done: u64,
        _total: u64,
        current: &EntryPath,
        current_done: u64,
        current_total: u64,
    ) {
        if current.display == LARGE_RAW_COPY_ENTRY && current_done > 0 {
            self.events.fetch_add(1, Ordering::SeqCst);
            if current_done < current_total && !self.fired.swap(true, Ordering::SeqCst) {
                self.ctl.pause();
                self.reached.send((current_done, current_total)).unwrap();
            }
        }
    }
}

impl ProgressSink for WorkerProgress {
    fn on_progress(&self, _done: u64, _total: u64, _current: &EntryPath) {
        if self.fired.swap(true, Ordering::SeqCst) {
            return;
        }
        if let Some(required_entry) = self.required_entry {
            let names = list_names(&self.archive, None);
            assert!(
                names.iter().any(|name| name == required_entry),
                "second update did not start from the first update result: {names:?}"
            );
        }
        fs::write(&self.entered, b"entered").unwrap();
        if let Some(release) = &self.release {
            wait_for_marker(release, PROCESS_WAIT_TIMEOUT);
        }
    }
}

struct ChildGuard {
    child: Option<Child>,
    role: &'static str,
}

impl ChildGuard {
    fn assert_success(mut self, timeout: Duration) {
        let started = Instant::now();
        loop {
            let status = self.child.as_mut().unwrap().try_wait().unwrap();
            if let Some(status) = status {
                self.child = None;
                assert!(
                    status.success(),
                    "{} update worker failed: {status}",
                    self.role
                );
                return;
            }
            if started.elapsed() >= timeout {
                let child = self.child.as_mut().unwrap();
                let _ = child.kill();
                let _ = child.wait();
                self.child = None;
                panic!(
                    "{} update worker did not exit within {timeout:?}",
                    self.role
                );
            }
            std::thread::sleep(MARKER_POLL_INTERVAL);
        }
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn worker_marker(root: &Path, role: &str, state: &str) -> PathBuf {
    root.join(format!("{role}-{state}.marker"))
}

fn wait_for_marker(path: &Path, timeout: Duration) {
    let started = Instant::now();
    while !path.is_file() {
        assert!(
            started.elapsed() < timeout,
            "marker did not appear within {timeout:?}: {}",
            path.display()
        );
        std::thread::sleep(MARKER_POLL_INTERVAL);
    }
}

fn assert_marker_stays_absent(path: &Path, duration: Duration) {
    let started = Instant::now();
    while started.elapsed() < duration {
        assert!(
            !path.exists(),
            "second update entered rewrite while the first process held the target lock"
        );
        std::thread::sleep(MARKER_POLL_INTERVAL);
    }
    assert!(
        !path.exists(),
        "second update entered rewrite while the first process held the target lock"
    );
}

fn spawn_update_worker(root: &Path, role: &'static str) -> ChildGuard {
    let child = Command::new(std::env::current_exe().unwrap())
        .arg("zip_update_process_worker")
        .arg("--exact")
        .arg("--nocapture")
        .env(PROCESS_WORKER_ROLE, role)
        .env(PROCESS_WORKER_ROOT, root)
        .spawn()
        .unwrap();
    ChildGuard {
        child: Some(child),
        role,
    }
}

fn assert_update_source_change(error: FormatError, archive: &Path, original_archive: &[u8]) {
    assert!(matches!(
        error,
        FormatError::Io(ref error) if error.kind() == std::io::ErrorKind::InvalidData
    ));
    assert_eq!(fs::read(archive).unwrap(), original_archive);
    assert_no_update_temp(archive.parent().unwrap());
}

fn assert_no_update_temp(parent: &Path) {
    let artifacts = update_transaction_artifacts(parent);
    assert!(
        artifacts.is_empty(),
        "ZIP update transaction artifacts remain: {artifacts:?}"
    );
}

fn update_transaction_artifacts(parent: &Path) -> Vec<String> {
    let mut artifacts: Vec<String> = fs::read_dir(parent)
        .unwrap()
        .filter_map(|entry| {
            let name = entry.unwrap().file_name().to_string_lossy().into_owned();
            name.starts_with(".squallz-update-").then_some(name)
        })
        .collect();
    artifacts.sort();
    artifacts
}

#[test]
fn update_add_file_and_directory() {
    let tmp = TempDir::new("update-add");
    let archive = base_archive(tmp.path(), None);
    fs::write(tmp.path().join("new.txt"), b"newcomer").unwrap();
    let extra_dir = tmp.path().join("extra");
    fs::create_dir_all(&extra_dir).unwrap();
    fs::write(extra_dir.join("inner.txt"), b"inside").unwrap();

    let ops = vec![
        UpdateOp::Add {
            src: tmp.path().join("new.txt"),
            dest: EntryPath::from_utf8("new.txt"),
        },
        UpdateOp::Add {
            src: extra_dir,
            dest: EntryPath::from_utf8("extra"),
        },
    ];
    run_update(&archive, &ops, &UpdateOptions::default()).unwrap();

    let names = list_names(&archive, None);
    assert!(names.contains(&"new.txt".to_string()));
    assert!(names.contains(&"extra/inner.txt".to_string()));
    assert!(names.iter().any(|n| n.starts_with("project/a.txt")));
    assert_unzip_t(&archive);
}

#[test]
fn engine_update_reports_ordered_monotonic_phases() {
    let tmp = TempDir::new("update-progress-phases");
    let archive = base_archive(tmp.path(), None);
    let progress = PhaseTrace::default();

    engine()
        .update(
            &archive,
            &[UpdateOp::Rename {
                from: EntrySelection::Raw(EntryPath::from_utf8("project/a.txt")),
                to: EntryPath::from_utf8("project/renamed.txt"),
            }],
            &UpdateOptions::default(),
            &progress,
            &ControlToken::new(),
        )
        .unwrap();

    assert_eq!(
        *progress.phases.lock().unwrap(),
        vec![
            (ProgressPhase::UpdateRewrite, true),
            (ProgressPhase::UpdateVerify, true),
            (ProgressPhase::UpdateCommit, false),
            (ProgressPhase::UpdateCleanup, false),
        ]
    );
    let events = progress.byte_events.lock().unwrap();
    for phase in [ProgressPhase::UpdateRewrite, ProgressPhase::UpdateVerify] {
        let phase_events: Vec<(u64, u64)> = events
            .iter()
            .filter_map(|(event_phase, done, total)| {
                (*event_phase == phase).then_some((*done, *total))
            })
            .collect();
        assert!(!phase_events.is_empty());
        let total = phase_events[0].1;
        assert!(total > 0);
        assert!(phase_events.iter().all(|event| event.1 == total));
        assert!(phase_events.windows(2).all(|pair| pair[0].0 <= pair[1].0));
        assert_eq!(phase_events.last().map(|event| event.0), Some(total));
    }
    assert_no_update_temp(archive.parent().unwrap());
}

#[test]
fn rewrite_progress_excludes_deleted_entry_bytes() {
    let tmp = TempDir::new("update-delete-progress-total");
    let archive = base_archive(tmp.path(), None);
    let mut source = zip::ZipArchive::new(fs::File::open(&archive).unwrap()).unwrap();
    let mut expected_total = 0u64;
    for index in 0..source.len() {
        let entry = source.by_index_raw(index).unwrap();
        if entry.name().trim_end_matches('/') != "project/a.txt" {
            expected_total = expected_total.saturating_add(entry.compressed_size());
        }
    }
    drop(source);
    let progress = PhaseTrace::default();

    engine()
        .update(
            &archive,
            &[UpdateOp::DeleteEntry {
                path: EntrySelection::Raw(EntryPath::from_utf8("project/a.txt")),
            }],
            &UpdateOptions::default(),
            &progress,
            &ControlToken::new(),
        )
        .unwrap();

    let events = progress.byte_events.lock().unwrap();
    let rewrite_events: Vec<(u64, u64)> = events
        .iter()
        .filter_map(|(phase, done, total)| {
            (*phase == ProgressPhase::UpdateRewrite).then_some((*done, *total))
        })
        .collect();
    assert!(!rewrite_events.is_empty());
    assert!(rewrite_events
        .iter()
        .all(|(_, total)| *total == expected_total));
    assert_eq!(
        rewrite_events.last().map(|(done, _)| *done),
        Some(expected_total)
    );
    assert!(!list_names(&archive, None).contains(&"project/a.txt".to_owned()));
}

#[test]
fn update_rejects_target_rebind_during_rewrite_without_overwriting_competitor() {
    let tmp = TempDir::new("update-target-rebind");
    let archive = base_archive(tmp.path(), None);
    let held_original = tmp.path().join("held-original.zip");
    let competitor = tmp.path().join("competitor.zip");
    let original_archive = fs::read(&archive).unwrap();
    let competitor_contents = b"late competing target";
    fs::write(&competitor, competitor_contents).unwrap();
    let artifacts_before = update_transaction_artifacts(tmp.path());
    let archive_for_action = archive.clone();
    let held_for_action = held_original.clone();
    let competitor_for_action = competitor.clone();
    let rebound = Arc::new(AtomicBool::new(false));
    let rebound_for_action = Arc::clone(&rebound);
    let progress = OnceOnProgress::raw_copy_started(move || {
        fs::rename(&archive_for_action, &held_for_action).unwrap();
        fs::rename(&competitor_for_action, &archive_for_action).unwrap();
        rebound_for_action.store(true, Ordering::SeqCst);
    });

    let error = engine()
        .update(
            &archive,
            &[UpdateOp::Delete {
                pattern: "*.log".into(),
            }],
            &UpdateOptions::default(),
            &progress,
            &ControlToken::new(),
        )
        .unwrap_err();

    assert!(rebound.load(Ordering::SeqCst));
    assert!(matches!(error, FormatError::Io(_)), "{error:?}");
    assert_eq!(fs::read(&held_original).unwrap(), original_archive);
    assert_eq!(fs::read(&archive).unwrap(), competitor_contents);
    assert_eq!(update_transaction_artifacts(tmp.path()), artifacts_before);
}

#[cfg(unix)]
#[test]
fn update_preserves_archive_permissions() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = TempDir::new("update-permissions");
    let archive = base_archive(tmp.path(), None);
    fs::set_permissions(&archive, fs::Permissions::from_mode(0o640)).unwrap();

    run_update(
        &archive,
        &[UpdateOp::Delete {
            pattern: "*.log".into(),
        }],
        &UpdateOptions::default(),
    )
    .unwrap();

    assert_eq!(
        fs::metadata(&archive).unwrap().permissions().mode() & 0o777,
        0o640
    );
    assert_unzip_t(&archive);
}

#[test]
#[cfg(unix)]
fn archive_listing_rejects_parent_symlink_aba_even_when_path_stamps_match() {
    use std::os::unix::fs::symlink;

    use squallz_core::api::{
        ArchiveFormat, ArchiveReader, ArchiveWriter, Detected, FormatCapabilities, FormatRegistry,
        ReadSeek, WriteSeek,
    };

    struct RestoreParentAfterOpen {
        inner: Arc<dyn ArchiveFormat>,
        parent: PathBuf,
        original_directory: PathBuf,
        observed: Arc<Mutex<Vec<u8>>>,
    }

    impl ArchiveFormat for RestoreParentAfterOpen {
        fn id(&self) -> &'static str {
            "zip"
        }

        fn extensions(&self) -> &'static [&'static str] {
            &["zip"]
        }

        fn capabilities(&self) -> FormatCapabilities {
            self.inner.capabilities()
        }

        fn sniff(&self, head: &[u8], tail: &[u8]) -> bool {
            self.inner.sniff(head, tail)
        }

        fn open(
            &self,
            mut source: Box<dyn ReadSeek>,
            options: &OpenOptions,
        ) -> Result<Box<dyn ArchiveReader>, FormatError> {
            source.seek(SeekFrom::Start(0))?;
            source.read_to_end(&mut self.observed.lock().unwrap())?;
            source.seek(SeekFrom::Start(0))?;
            let reader = self.inner.open(source, options)?;
            fs::remove_file(&self.parent)?;
            symlink(&self.original_directory, &self.parent)?;
            Ok(reader)
        }

        fn create(
            &self,
            output: Box<dyn WriteSeek>,
            options: &CreateOptions,
        ) -> Result<Box<dyn ArchiveWriter>, FormatError> {
            self.inner.create(output, options)
        }
    }

    let tmp = TempDir::new("browse-parent-symlink-aba");
    let original_directory = tmp.path().join("original");
    let replacement_directory = tmp.path().join("replacement");
    fs::create_dir(&original_directory).unwrap();
    fs::create_dir(&replacement_directory).unwrap();
    let original = build_stored_zip(&[RawZipEntry {
        name: b"original.txt".to_vec(),
        data: b"original archive".to_vec(),
    }]);
    let replacement = build_stored_zip(&[RawZipEntry {
        name: b"replacement.txt".to_vec(),
        data: b"replacement archive".to_vec(),
    }]);
    fs::write(original_directory.join("archive.zip"), &original).unwrap();
    fs::write(replacement_directory.join("archive.zip"), &replacement).unwrap();
    let parent = tmp.path().join("current");
    symlink(&original_directory, &parent).unwrap();
    let path = parent.join("archive.zip");
    let before = engine()
        .inspect_archive_source_state(&path, &ControlToken::new())
        .unwrap();
    fs::remove_file(&parent).unwrap();
    symlink(&replacement_directory, &parent).unwrap();

    let Some(Detected::Archive(zip)) = squallz_formats::registry().detect_by_name("archive.zip")
    else {
        panic!("ZIP format is unavailable");
    };
    let observed = Arc::new(Mutex::new(Vec::new()));
    let mut registry = FormatRegistry::new();
    registry.register_archive(Arc::new(RestoreParentAfterOpen {
        inner: zip,
        parent,
        original_directory,
        observed: Arc::clone(&observed),
    }));
    let result = squallz_core::Engine::new(registry)
        .list_with_format_source_set_and_structure_with_entry_limit_and_control(
            &path,
            &OpenOptions::default(),
            100,
            &ControlToken::new(),
        );
    let after = engine()
        .inspect_archive_source_state(&path, &ControlToken::new())
        .unwrap();
    assert_eq!(
        before, after,
        "both independent path stamps observe the original archive"
    );
    assert_eq!(
        *observed.lock().unwrap(),
        replacement,
        "the actual reader consumed the replacement archive"
    );
    match result {
        Err(error) => assert!(error.is_input_changed(), "{error:?}"),
        Ok(listing) => panic!(
            "listing accepted the replacement archive behind matching path stamps: {:?}",
            listing
                .entries
                .iter()
                .map(|entry| &entry.path.display)
                .collect::<Vec<_>>()
        ),
    }
}

#[test]
fn guarded_update_rejects_a_replaced_browsed_archive_before_rewriting() {
    let tmp = TempDir::new("update-view-source-replaced");
    let archive = named_archive(tmp.path(), &["report.txt", "keep.txt"]);
    let guard = ArchiveUpdateGuard::new(
        engine()
            .inspect_archive_source_state(&archive, &ControlToken::new())
            .unwrap(),
    );
    let original = fs::read(&archive).unwrap();
    let held_original = tmp.path().join("held-original.zip");
    let replacement = tmp.path().join("replacement.zip");
    let replacement_bytes = build_stored_zip(&[
        RawZipEntry {
            name: b"report.txt".to_vec(),
            data: b"different report from a replacement archive".to_vec(),
        },
        RawZipEntry {
            name: b"replacement-only.txt".to_vec(),
            data: b"keep this archive intact".to_vec(),
        },
    ]);
    fs::write(&replacement, &replacement_bytes).unwrap();
    fs::rename(&archive, &held_original).unwrap();
    fs::rename(&replacement, &archive).unwrap();

    for operation in [
        UpdateOp::DeleteEntry {
            path: EntrySelection::Display("report.txt".into()),
        },
        UpdateOp::Rename {
            from: EntrySelection::Display("report.txt".into()),
            to: EntryPath::from_utf8("renamed.txt"),
        },
        UpdateOp::AddDir {
            path: EntryPath::from_utf8("new-folder/"),
        },
    ] {
        let error = engine()
            .update_guarded(
                &archive,
                &[operation],
                &UpdateOptions::default(),
                &guard,
                &NoProgress,
                &ControlToken::new(),
            )
            .unwrap_err();
        assert!(error.is_input_changed(), "{error:?}");
        assert_eq!(fs::read(&archive).unwrap(), replacement_bytes);
        assert_eq!(fs::read(&held_original).unwrap(), original);
        assert_no_update_temp(tmp.path());
    }
}

#[test]
fn guarded_concurrent_updates_advance_the_shared_browsed_source_binding() {
    let tmp = TempDir::new("update-view-source-concurrent");
    let actual_archive = base_archive(tmp.path(), None);
    let archive = tmp.path().join(".").join("base.zip");
    let guard = Arc::new(ArchiveUpdateGuard::new(
        engine()
            .inspect_archive_source_state(&archive, &ControlToken::new())
            .unwrap(),
    ));
    let first = tmp.path().join("first.txt");
    let second = tmp.path().join("second.txt");
    fs::write(&first, b"first").unwrap();
    fs::write(&second, b"second").unwrap();
    let barrier = Arc::new(Barrier::new(2));
    let workers = [(first, "first.txt"), (second, "second.txt")]
        .into_iter()
        .map(|(source, destination)| {
            let archive = archive.clone();
            let guard = Arc::clone(&guard);
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                engine().update_guarded(
                    &archive,
                    &[UpdateOp::Add {
                        src: source,
                        dest: EntryPath::from_utf8(destination),
                    }],
                    &UpdateOptions::default(),
                    &guard,
                    &NoProgress,
                    &ControlToken::new(),
                )
            })
        })
        .collect::<Vec<_>>();
    for worker in workers {
        worker.join().unwrap().unwrap();
    }
    let names = list_names(&actual_archive, None);
    assert!(names.contains(&"first.txt".into()));
    assert!(names.contains(&"second.txt".into()));
    assert_no_update_temp(tmp.path());
    assert_unzip_t(&actual_archive);
}

#[test]
fn concurrent_updates_are_serialized_against_the_latest_archive() {
    let tmp = TempDir::new("update-concurrent");
    let archive = base_archive(tmp.path(), None);
    let first_source = tmp.path().join("first.txt");
    let second_source = tmp.path().join("second.txt");
    fs::write(&first_source, b"first").unwrap();
    fs::write(&second_source, b"second").unwrap();
    let barrier = Arc::new(Barrier::new(2));

    let workers = [(first_source, "first.txt"), (second_source, "second.txt")]
        .into_iter()
        .map(|(source, destination)| {
            let archive = archive.clone();
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                engine().update(
                    &archive,
                    &[UpdateOp::Add {
                        src: source,
                        dest: EntryPath::from_utf8(destination),
                    }],
                    &UpdateOptions::default(),
                    &NoProgress,
                    &ControlToken::new(),
                )
            })
        })
        .collect::<Vec<_>>();

    for worker in workers {
        worker.join().unwrap().unwrap();
    }

    let names = list_names(&archive, None);
    assert!(names.contains(&"first.txt".to_owned()));
    assert!(names.contains(&"second.txt".to_owned()));
    assert_unzip_t(&archive);
}

#[test]
fn zip_update_process_worker() {
    let Some(role) = std::env::var_os(PROCESS_WORKER_ROLE) else {
        return;
    };
    let role = role.to_str().unwrap();
    let root = PathBuf::from(std::env::var_os(PROCESS_WORKER_ROOT).unwrap());
    let archive = root.join("base.zip");
    let (source, destination, release, required_entry) = match role {
        "first" => (
            root.join("first-source.txt"),
            "first.txt",
            Some(worker_marker(&root, role, "release")),
            None,
        ),
        "second" => (
            root.join("second-source.txt"),
            "second.txt",
            None,
            Some("first.txt"),
        ),
        _ => panic!("unknown ZIP update worker role: {role}"),
    };
    let progress = WorkerProgress {
        archive: archive.clone(),
        entered: worker_marker(&root, role, "entered"),
        release,
        required_entry,
        fired: AtomicBool::new(false),
    };

    fs::write(worker_marker(&root, role, "ready"), b"ready").unwrap();
    let mut operations = vec![UpdateOp::Add {
        src: source,
        dest: EntryPath::from_utf8(destination),
    }];
    if role == "second" {
        operations.push(UpdateOp::Rename {
            from: EntrySelection::Display("first.txt".into()),
            to: EntryPath::from_utf8("renamed-first.txt"),
        });
    }
    engine()
        .update(
            &archive,
            &operations,
            &UpdateOptions::default(),
            &progress,
            &ControlToken::new(),
        )
        .unwrap();
}

#[test]
fn cross_process_updates_wait_for_the_target_lock_and_use_the_latest_archive() {
    let tmp = TempDir::new("update-cross-process");
    let archive = base_archive(tmp.path(), None);
    fs::write(tmp.path().join("first-source.txt"), b"first").unwrap();
    fs::write(tmp.path().join("second-source.txt"), b"second").unwrap();

    let first = spawn_update_worker(tmp.path(), "first");
    wait_for_marker(
        &worker_marker(tmp.path(), "first", "entered"),
        PROCESS_WAIT_TIMEOUT,
    );

    let second = spawn_update_worker(tmp.path(), "second");
    wait_for_marker(
        &worker_marker(tmp.path(), "second", "ready"),
        PROCESS_WAIT_TIMEOUT,
    );
    let second_entered = worker_marker(tmp.path(), "second", "entered");
    assert_marker_stays_absent(&second_entered, LOCK_OBSERVATION_WINDOW);

    fs::write(worker_marker(tmp.path(), "first", "release"), b"release").unwrap();
    first.assert_success(PROCESS_WAIT_TIMEOUT);
    wait_for_marker(&second_entered, PROCESS_WAIT_TIMEOUT);
    second.assert_success(PROCESS_WAIT_TIMEOUT);

    let names = list_names(&archive, None);
    assert!(names.contains(&"renamed-first.txt".to_owned()), "{names:?}");
    assert!(!names.contains(&"first.txt".to_owned()), "{names:?}");
    assert!(names.contains(&"second.txt".to_owned()), "{names:?}");
    assert_no_update_temp(tmp.path());
    assert_unzip_t(&archive);
}

#[test]
fn update_add_rejects_same_length_source_replacement_and_preserves_archive() {
    let tmp = TempDir::new("update-add-source-replacement");
    let archive = base_archive(tmp.path(), None);
    let source = tmp.path().join("source.bin");
    let replacement = tmp.path().join("replacement.bin");
    fs::write(&source, [b'A'; 32]).unwrap();
    fs::write(&replacement, [b'B'; 32]).unwrap();
    let original_archive = fs::read(&archive).unwrap();
    let changed = Arc::new(AtomicBool::new(false));
    let changed_for_action = Arc::clone(&changed);
    let source_for_action = source.clone();
    let progress = OnceOnProgress::rewrite_started(move || {
        fs::remove_file(&source_for_action).unwrap();
        fs::rename(&replacement, &source_for_action).unwrap();
        changed_for_action.store(true, Ordering::SeqCst);
    });
    let ops = [UpdateOp::Add {
        src: source,
        dest: EntryPath::from_utf8("source.bin"),
    }];

    let error = engine()
        .update(
            &archive,
            &ops,
            &UpdateOptions::default(),
            &progress,
            &ControlToken::new(),
        )
        .unwrap_err();

    assert!(changed.load(Ordering::SeqCst));
    assert_update_source_change(error, &archive, &original_archive);
}

#[cfg(unix)]
#[test]
fn update_add_rejects_in_place_rewrite_with_restored_mtime() {
    use std::io::Write;
    use std::os::unix::fs::MetadataExt;
    use std::time::Duration;

    let tmp = TempDir::new("update-add-source-rewrite");
    let archive = base_archive(tmp.path(), None);
    let source = tmp.path().join("source.bin");
    fs::write(&source, [b'A'; 32]).unwrap();
    let original_archive = fs::read(&archive).unwrap();
    let original_metadata = fs::metadata(&source).unwrap();
    let original_modified = original_metadata.modified().unwrap();
    let original_changed = (original_metadata.ctime(), original_metadata.ctime_nsec());
    let source_for_action = source.clone();
    let progress = OnceOnProgress::rewrite_started(move || {
        let mut changed = original_changed;
        for _ in 0..100 {
            std::thread::sleep(Duration::from_millis(20));
            let mut file = fs::OpenOptions::new()
                .write(true)
                .truncate(true)
                .open(&source_for_action)
                .unwrap();
            file.write_all(&[b'B'; 32]).unwrap();
            file.set_times(std::fs::FileTimes::new().set_modified(original_modified))
                .unwrap();
            drop(file);
            let metadata = fs::metadata(&source_for_action).unwrap();
            changed = (metadata.ctime(), metadata.ctime_nsec());
            if changed != original_changed {
                break;
            }
        }
        assert_ne!(changed, original_changed);
        assert_eq!(
            fs::metadata(&source_for_action)
                .unwrap()
                .modified()
                .unwrap(),
            original_modified
        );
    });
    let ops = [UpdateOp::Add {
        src: source,
        dest: EntryPath::from_utf8("source.bin"),
    }];

    let error = engine()
        .update(
            &archive,
            &ops,
            &UpdateOptions::default(),
            &progress,
            &ControlToken::new(),
        )
        .unwrap_err();

    assert_update_source_change(error, &archive, &original_archive);
}

#[cfg(unix)]
#[test]
fn update_add_rechecks_source_path_after_streaming() {
    let tmp = TempDir::new("update-add-source-rebind");
    let archive = base_archive(tmp.path(), None);
    let source = tmp.path().join("stream.bin");
    let replacement = tmp.path().join("replacement.bin");
    fs::write(&source, vec![b'A'; 1024 * 1024]).unwrap();
    fs::write(&replacement, vec![b'B'; 1024 * 1024]).unwrap();
    let original_archive = fs::read(&archive).unwrap();
    let changed = Arc::new(AtomicBool::new(false));
    let changed_for_action = Arc::clone(&changed);
    let source_for_action = source.clone();
    let progress = OnceOnProgress::entry_read("stream.bin", move || {
        fs::remove_file(&source_for_action).unwrap();
        fs::rename(&replacement, &source_for_action).unwrap();
        changed_for_action.store(true, Ordering::SeqCst);
    });
    let ops = [UpdateOp::Add {
        src: source,
        dest: EntryPath::from_utf8("stream.bin"),
    }];
    let options = CreateOptions {
        level: CompressionLevel::Store,
        ..CreateOptions::default()
    };

    let error = engine()
        .update(
            &archive,
            &ops,
            &UpdateOptions {
                create: options.clone(),
                ..Default::default()
            },
            &progress,
            &ControlToken::new(),
        )
        .unwrap_err();

    assert!(changed.load(Ordering::SeqCst));
    assert_update_source_change(error, &archive, &original_archive);
}

#[test]
fn update_add_can_cancel_while_streaming_one_file() {
    let tmp = TempDir::new("update-add-cancel-stream");
    let archive = base_archive(tmp.path(), None);
    let source = tmp.path().join("stream.bin");
    fs::write(&source, vec![b'A'; 1024 * 1024]).unwrap();
    let original_archive = fs::read(&archive).unwrap();
    let control = ControlToken::new();
    let control_for_action = Arc::clone(&control);
    let progress = OnceOnProgress::entry_read("stream.bin", move || {
        control_for_action.cancel();
    });
    let ops = [UpdateOp::Add {
        src: source,
        dest: EntryPath::from_utf8("stream.bin"),
    }];
    let options = CreateOptions {
        level: CompressionLevel::Store,
        ..CreateOptions::default()
    };

    let error = engine()
        .update(
            &archive,
            &ops,
            &UpdateOptions {
                create: options.clone(),
                ..Default::default()
            },
            &progress,
            &control,
        )
        .unwrap_err();

    assert!(matches!(error, FormatError::Cancelled));
    assert_eq!(fs::read(&archive).unwrap(), original_archive);
    assert_no_update_temp(tmp.path());
}

#[test]
fn guarded_update_cancellation_keeps_the_binding_for_an_explicit_retry() {
    let tmp = TempDir::new("update-view-source-cancel");
    let archive = large_stored_archive(tmp.path());
    let original = fs::read(&archive).unwrap();
    let guard = ArchiveUpdateGuard::new(
        engine()
            .inspect_archive_source_state(&archive, &ControlToken::new())
            .unwrap(),
    );
    let control = ControlToken::new();
    let progress = CancelDuringRawCopy {
        ctl: Arc::clone(&control),
        fired: AtomicBool::new(false),
        observed: Mutex::new(None),
    };
    let ops = [UpdateOp::DeleteEntry {
        path: EntrySelection::Display("raw-copy/remove.txt".into()),
    }];
    let error = engine()
        .update_guarded(
            &archive,
            &ops,
            &UpdateOptions::default(),
            &guard,
            &progress,
            &control,
        )
        .unwrap_err();
    assert!(matches!(error, FormatError::Cancelled), "{error:?}");
    assert!(progress.fired.load(Ordering::SeqCst));
    assert_eq!(fs::read(&archive).unwrap(), original);
    assert_no_update_temp(tmp.path());

    engine()
        .update_guarded(
            &archive,
            &ops,
            &UpdateOptions::default(),
            &guard,
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap();
    assert!(!list_names(&archive, None).contains(&"raw-copy/remove.txt".into()));
    assert_no_update_temp(tmp.path());
    assert_unzip_t(&archive);
}

#[test]
fn update_can_cancel_during_unchanged_entry_raw_copy() {
    let tmp = TempDir::new("update-cancel-raw-copy");
    let archive = large_stored_archive(tmp.path());
    let original_archive = fs::read(&archive).unwrap();
    let control = ControlToken::new();
    let progress = CancelDuringRawCopy {
        ctl: Arc::clone(&control),
        fired: AtomicBool::new(false),
        observed: Mutex::new(None),
    };

    let error = engine()
        .update(
            &archive,
            &[UpdateOp::Delete {
                pattern: "remove.txt".into(),
            }],
            &UpdateOptions::default(),
            &progress,
            &control,
        )
        .unwrap_err();

    assert!(matches!(error, FormatError::Cancelled));
    assert!(progress.fired.load(Ordering::SeqCst));
    let (current_done, current_total) = progress.observed.lock().unwrap().unwrap();
    assert!(current_done > 0);
    assert!(current_done < current_total);
    assert_eq!(fs::read(&archive).unwrap(), original_archive);
    assert_no_update_temp(tmp.path());
}

#[test]
fn update_can_pause_and_resume_during_unchanged_entry_raw_copy() {
    let tmp = TempDir::new("update-pause-raw-copy");
    let archive = large_stored_archive(tmp.path());
    let original_archive = fs::read(&archive).unwrap();
    let control = ControlToken::new();
    let control_for_worker = Arc::clone(&control);
    let archive_for_worker = archive.clone();
    let events = Arc::new(AtomicU64::new(0));
    let events_for_worker = Arc::clone(&events);
    let (reached_tx, reached_rx) = mpsc::sync_channel(1);
    let (done_tx, done_rx) = mpsc::sync_channel(1);
    let worker = std::thread::spawn(move || {
        let progress = PauseDuringRawCopy {
            ctl: Arc::clone(&control_for_worker),
            fired: AtomicBool::new(false),
            events: events_for_worker,
            reached: reached_tx,
        };
        let result = engine().update(
            &archive_for_worker,
            &[UpdateOp::Delete {
                pattern: "remove.txt".into(),
            }],
            &UpdateOptions::default(),
            &progress,
            &control_for_worker,
        );
        done_tx.send(result).unwrap();
    });
    let resume_on_drop = ResumeOnDrop(Arc::clone(&control));

    let (current_done, current_total) = reached_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    let paused = control.is_paused();
    let paused_result = done_rx.recv_timeout(Duration::from_millis(250));
    let still_running = matches!(&paused_result, Err(mpsc::RecvTimeoutError::Timeout));
    let events_while_paused = events.load(Ordering::SeqCst);
    let target_unchanged = fs::read(&archive).unwrap() == original_archive;
    control.resume();
    drop(resume_on_drop);
    let result = match paused_result {
        Ok(result) => result,
        Err(mpsc::RecvTimeoutError::Timeout) => {
            done_rx.recv_timeout(Duration::from_secs(30)).unwrap()
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            panic!("ZIP update worker disconnected while paused")
        }
    };
    worker.join().unwrap();

    assert!(current_done > 0);
    assert!(current_done < current_total);
    assert!(paused);
    assert!(still_running);
    assert_eq!(events_while_paused, 1);
    assert!(events.load(Ordering::SeqCst) > events_while_paused);
    assert!(target_unchanged);
    result.unwrap();
    let names = list_names(&archive, None);
    assert!(names.contains(&LARGE_RAW_COPY_ENTRY.to_owned()));
    assert!(!names.iter().any(|name| name.ends_with("remove.txt")));
    assert_no_update_temp(tmp.path());
    assert_unzip_t(&archive);
}

#[test]
fn update_add_can_cancel_while_scanning_a_directory() {
    let tmp = TempDir::new("update-add-cancel-scan");
    let archive = base_archive(tmp.path(), None);
    let source = tmp.path().join("incoming");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("a.txt"), b"a").unwrap();
    let original_archive = fs::read(&archive).unwrap();
    let ctl = ControlToken::new();
    let progress = CancelOnScan {
        ctl: Arc::clone(&ctl),
        scanned: AtomicU64::new(0),
        byte_progress: AtomicBool::new(false),
    };
    let ops = [UpdateOp::Add {
        src: source,
        dest: EntryPath::from_utf8("incoming"),
    }];

    let error = engine()
        .update(&archive, &ops, &UpdateOptions::default(), &progress, &ctl)
        .unwrap_err();

    assert!(matches!(error, FormatError::Cancelled));
    assert_eq!(progress.scanned.load(Ordering::SeqCst), 2);
    assert!(!progress.byte_progress.load(Ordering::SeqCst));
    assert_eq!(fs::read(&archive).unwrap(), original_archive);
    assert_no_update_temp(tmp.path());
}

#[cfg(unix)]
#[test]
fn update_add_rejects_symbolic_link_target_change() {
    use std::os::unix::fs::symlink;

    let tmp = TempDir::new("update-add-link-target");
    let archive = base_archive(tmp.path(), None);
    let first = tmp.path().join("first.txt");
    let second = tmp.path().join("second.txt");
    let source = tmp.path().join("source-link");
    fs::write(&first, b"first").unwrap();
    fs::write(&second, b"second").unwrap();
    symlink(&first, &source).unwrap();
    let original_archive = fs::read(&archive).unwrap();
    let source_for_action = source.clone();
    let progress = OnceOnProgress::rewrite_started(move || {
        fs::remove_file(&source_for_action).unwrap();
        symlink(&second, &source_for_action).unwrap();
    });
    let ops = [UpdateOp::Add {
        src: source,
        dest: EntryPath::from_utf8("source-link"),
    }];

    let error = engine()
        .update(
            &archive,
            &ops,
            &UpdateOptions::default(),
            &progress,
            &ControlToken::new(),
        )
        .unwrap_err();

    assert_update_source_change(error, &archive, &original_archive);
}

#[test]
fn update_add_ignores_directory_members_created_after_preparation() {
    let tmp = TempDir::new("update-add-late-member");
    let archive = base_archive(tmp.path(), None);
    let source = tmp.path().join("extra");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("ready.txt"), b"ready").unwrap();
    let source_for_action = source.clone();
    let progress = OnceOnProgress::rewrite_started(move || {
        fs::write(source_for_action.join("late.txt"), b"late").unwrap();
    });
    let ops = [UpdateOp::Add {
        src: source.clone(),
        dest: EntryPath::from_utf8("extra"),
    }];

    engine()
        .update(
            &archive,
            &ops,
            &UpdateOptions::default(),
            &progress,
            &ControlToken::new(),
        )
        .unwrap();

    assert!(source.join("late.txt").is_file());
    let names = list_names(&archive, None);
    assert!(names.contains(&"extra/ready.txt".to_owned()), "{names:?}");
    assert!(!names.contains(&"extra/late.txt".to_owned()), "{names:?}");
    assert_unzip_t(&archive);
}

#[test]
fn update_add_empty_directory_entry() {
    let tmp = TempDir::new("update-add-dir");
    let archive = base_archive(tmp.path(), None);
    let ops = vec![UpdateOp::AddDir {
        path: EntryPath::from_utf8("empty-folder"),
    }];

    run_update(&archive, &ops, &UpdateOptions::default()).unwrap();

    let names = list_names(&archive, None);
    assert!(names.contains(&"empty-folder/".to_string()), "{names:?}");
    assert_unzip_t(&archive);
}

#[test]
fn update_add_directory_applies_create_excludes() {
    let tmp = TempDir::new("update-add-excludes");
    let archive = base_archive(tmp.path(), None);
    let extra_dir = tmp.path().join("extra");
    fs::create_dir_all(extra_dir.join("node_modules/pkg")).unwrap();
    fs::create_dir_all(extra_dir.join(".git")).unwrap();
    fs::write(extra_dir.join("keep.txt"), b"keep").unwrap();
    fs::write(extra_dir.join("drop.tmp"), b"drop").unwrap();
    fs::write(extra_dir.join("node_modules/pkg/index.js"), b"drop").unwrap();
    fs::write(extra_dir.join(".git/config"), b"drop").unwrap();

    let ops = vec![UpdateOp::Add {
        src: extra_dir,
        dest: EntryPath::from_utf8("extra"),
    }];
    let opts = CreateOptions {
        excludes: vec!["node_modules".into(), ".git".into(), "*.tmp".into()],
        ..CreateOptions::default()
    };
    run_update(
        &archive,
        &ops,
        &UpdateOptions {
            create: opts.clone(),
            ..Default::default()
        },
    )
    .unwrap();

    let names = list_names(&archive, None);
    assert!(names.contains(&"extra/keep.txt".to_string()));
    assert!(
        !names.iter().any(|n| n.contains("node_modules")),
        "{names:?}"
    );
    assert!(!names.iter().any(|n| n.contains(".git")), "{names:?}");
    assert!(!names.iter().any(|n| n.ends_with(".tmp")), "{names:?}");
    assert_unzip_t(&archive);
}

#[test]
fn update_delete_resolves_legacy_names_to_original_entry_bytes() {
    let tmp = TempDir::new("update-delete-legacy");
    let archive = tmp.path().join("legacy.zip");
    let (name, _, errors) = encoding_rs::GBK.encode("压缩文件中文名称测试[1].txt");
    assert!(!errors);
    fs::write(
        &archive,
        build_stored_zip(&[
            RawZipEntry {
                name: name.into_owned(),
                data: b"delete me".to_vec(),
            },
            RawZipEntry {
                name: b"keep.txt".to_vec(),
                data: b"keep me".to_vec(),
            },
        ]),
    )
    .unwrap();
    let ops = [UpdateOp::DeleteEntry {
        path: EntrySelection::Display("压缩文件中文名称测试[1].txt".into()),
    }];
    run_update(
        &archive,
        &ops,
        &UpdateOptions {
            encoding_override: Some("gbk".into()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(list_names(&archive, None), ["keep.txt"]);
    assert_unzip_t(&archive);
}

#[test]
fn update_preserves_unflagged_utf8_names() {
    let tmp = TempDir::new("update-unflagged-utf8");
    let archive = tmp.path().join("unflagged.zip");
    let retained = "资料/保留.txt";
    fs::write(
        &archive,
        build_stored_zip(&[
            RawZipEntry {
                name: retained.as_bytes().to_vec(),
                data: b"keep me".to_vec(),
            },
            RawZipEntry {
                name: b"drop.txt".to_vec(),
                data: b"drop me".to_vec(),
            },
        ]),
    )
    .unwrap();
    run_update(
        &archive,
        &[UpdateOp::DeleteEntry {
            path: EntrySelection::Raw(EntryPath::from_utf8("drop.txt")),
        }],
        &UpdateOptions::default(),
    )
    .unwrap();
    assert_eq!(list_names(&archive, None), [retained]);
    let mut reader = zip::ZipArchive::new(fs::File::open(&archive).unwrap()).unwrap();
    assert_eq!(
        reader.by_index_raw(0).unwrap().name_raw(),
        retained.as_bytes()
    );
    assert_unzip_t(&archive);
}

#[test]
fn update_preserves_retained_legacy_names() {
    let tmp = TempDir::new("update-retained-legacy");
    let archive = tmp.path().join("legacy.zip");
    let (name, _, errors) = encoding_rs::GBK.encode("压缩文件中文名称测试.txt");
    assert!(!errors);
    let name = name.into_owned();
    let before = build_stored_zip(&[
        RawZipEntry {
            name: name.clone(),
            data: b"keep me".to_vec(),
        },
        RawZipEntry {
            name: b"drop.txt".to_vec(),
            data: b"drop me".to_vec(),
        },
    ]);
    fs::write(&archive, &before).unwrap();
    run_update(
        &archive,
        &[UpdateOp::DeleteEntry {
            path: EntrySelection::Raw(EntryPath::from_utf8("drop.txt")),
        }],
        &UpdateOptions::default(),
    )
    .unwrap();
    let mut reader = zip::ZipArchive::new(fs::File::open(&archive).unwrap()).unwrap();
    assert_eq!(reader.len(), 1);
    assert_eq!(reader.by_index_raw(0).unwrap().name_raw(), name);
    let entries = engine()
        .list(
            &archive,
            &OpenOptions {
                encoding_override: Some("gbk".into()),
                ..OpenOptions::default()
            },
        )
        .unwrap();
    assert_eq!(entries[0].path.display, "压缩文件中文名称测试.txt");
    assert_eq!(entries[0].path.raw, name);
    assert_unzip_t(&archive);
    assert_no_update_temp(tmp.path());
}

#[test]
fn update_delete_literal_paths_preserve_metacharacters_and_archive_depth() {
    let tmp = TempDir::new("update-delete-literal");
    let selected = [
        "notes.txt",
        "a[1].txt",
        "x?.txt",
        "x*.txt",
        "one{a,b}.txt",
        "资料/说明.txt",
    ];
    let retained = [
        "nested/notes.txt",
        "a1.txt",
        "xa.txt",
        "onea.txt",
        "keep.txt",
    ];
    let archive = named_archive(
        tmp.path(),
        &[selected.as_slice(), retained.as_slice()].concat(),
    );
    let operations: Vec<_> = selected
        .into_iter()
        .map(|path| UpdateOp::DeleteEntry {
            path: EntrySelection::Raw(EntryPath::from_utf8(path)),
        })
        .collect();
    run_update(&archive, &operations, &UpdateOptions::default()).unwrap();
    let mut expected = retained.to_vec();
    expected.sort();
    assert_eq!(list_names(&archive, None), expected);
    assert_unzip_t(&archive);
    assert_no_update_temp(tmp.path());
}

#[test]
fn update_delete_literal_directory_removes_only_its_complete_subtree() {
    for explicit in [false, true] {
        let tmp = TempDir::new("update-delete-directory");
        let mut names = vec![
            "logs/a.txt",
            "logs/sub/b.txt",
            "other/logs/keep.txt",
            "logstash/keep.txt",
        ];
        if explicit {
            names.extend(["logs/", "logs/sub/"]);
        }
        let archive = named_archive(tmp.path(), &names);
        run_update(
            &archive,
            &[UpdateOp::DeleteEntry {
                path: EntrySelection::Raw(EntryPath::from_utf8("logs/")),
            }],
            &UpdateOptions::default(),
        )
        .unwrap();
        assert_eq!(
            list_names(&archive, None),
            ["logstash/keep.txt", "other/logs/keep.txt"]
        );
        assert_unzip_t(&archive);
    }
}

#[test]
fn update_delete_literal_file_and_directory_are_distinct() {
    for (selected, expected) in [
        ("docs", vec!["docs/", "docs/readme.txt", "keep.txt"]),
        ("docs/", vec!["docs", "keep.txt"]),
    ] {
        let tmp = TempDir::new("update-delete-file-directory");
        let archive = named_archive(
            tmp.path(),
            &["docs", "docs/", "docs/readme.txt", "keep.txt"],
        );
        run_update(
            &archive,
            &[UpdateOp::DeleteEntry {
                path: EntrySelection::Raw(EntryPath::from_utf8(selected)),
            }],
            &UpdateOptions::default(),
        )
        .unwrap();
        assert_eq!(list_names(&archive, None), expected);
        assert_unzip_t(&archive);
    }
}

#[test]
fn update_delete_missing_or_empty_literal_path_is_atomic() {
    let tmp = TempDir::new("update-delete-missing");
    let archive = base_archive(tmp.path(), None);
    let before = fs::read(&archive).unwrap();
    for missing in ["", "missing.txt", "project/sub", "project/a.txt/"] {
        let result = run_update(
            &archive,
            &[
                UpdateOp::Delete {
                    pattern: "*.log".into(),
                },
                UpdateOp::DeleteEntry {
                    path: EntrySelection::Raw(EntryPath::from_utf8("project/a.txt")),
                },
                UpdateOp::DeleteEntry {
                    path: EntrySelection::Raw(EntryPath::from_utf8(missing)),
                },
            ],
            &UpdateOptions::default(),
        );
        assert_other_contains(
            result.unwrap_err(),
            if missing.is_empty() {
                "cannot be empty"
            } else {
                "not found"
            },
        );
        assert_eq!(fs::read(&archive).unwrap(), before, "{missing}");
        assert_no_update_temp(tmp.path());
    }
}

#[test]
fn update_delete_literal_and_glob_operations_can_be_combined_with_replacement() {
    let tmp = TempDir::new("update-delete-combined");
    let archive = base_archive(tmp.path(), None);
    let replacement = tmp.path().join("replacement.txt");
    fs::write(&replacement, b"replacement").unwrap();
    run_update(
        &archive,
        &[
            UpdateOp::DeleteEntry {
                path: EntrySelection::Raw(EntryPath::from_utf8("project/a.txt")),
            },
            UpdateOp::Delete {
                pattern: "*.log".into(),
            },
            UpdateOp::Add {
                src: replacement,
                dest: EntryPath::from_utf8("project/a.txt"),
            },
        ],
        &UpdateOptions::default(),
    )
    .unwrap();
    assert!(!list_names(&archive, None)
        .iter()
        .any(|name| name.ends_with(".log")));
    let mut archive_reader = zip::ZipArchive::new(fs::File::open(&archive).unwrap()).unwrap();
    let mut data = String::new();
    std::io::Read::read_to_string(
        &mut archive_reader.by_name("project/a.txt").unwrap(),
        &mut data,
    )
    .unwrap();
    assert_eq!(data, "replacement");
    assert_unzip_t(&archive);
}

#[test]
fn update_delete_literal_keeps_encrypted_payloads_without_a_password() {
    let tmp = TempDir::new("update-delete-encrypted");
    let archive = base_archive(tmp.path(), Some("deletion-test-password"));
    run_update(
        &archive,
        &[UpdateOp::DeleteEntry {
            path: EntrySelection::Raw(EntryPath::from_utf8("project/sub/")),
        }],
        &UpdateOptions::default(),
    )
    .unwrap();
    let options = OpenOptions {
        password: Some(Password::new("deletion-test-password")),
        encoding_override: None,
    };
    let entries = engine().list(&archive, &options).unwrap();
    assert!(!entries
        .iter()
        .any(|entry| entry.path.display.starts_with("project/sub/")));
    assert!(entries
        .iter()
        .filter(|entry| !entry.path.display.ends_with('/'))
        .all(|entry| entry.encrypted));
    let mut reader = engine().open(&archive, &options).unwrap();
    let mut data = Vec::new();
    reader
        .read_entry(&EntryPath::from_utf8("project/a.txt"), &mut |entry| {
            entry.read_to_end(&mut data)?;
            Ok(())
        })
        .unwrap();
    assert_eq!(data, b"alpha");
}

#[test]
fn update_delete_by_glob() {
    let tmp = TempDir::new("update-delete");
    let archive = base_archive(tmp.path(), None);
    let ops = vec![UpdateOp::Delete {
        pattern: "*.log".into(),
    }];
    run_update(&archive, &ops, &UpdateOptions::default()).unwrap();
    let names = list_names(&archive, None);
    assert!(!names.iter().any(|n| n.ends_with(".log")), "{names:?}");
    assert!(names.iter().any(|n| n.contains("a.txt")));

    // Deleting a directory name prunes its subtree.
    let ops = vec![UpdateOp::Delete {
        pattern: "project/sub".into(),
    }];
    run_update(&archive, &ops, &UpdateOptions::default()).unwrap();
    let names = list_names(&archive, None);
    assert!(!names.iter().any(|n| n.contains("sub")), "{names:?}");
    assert_unzip_t(&archive);
}

#[test]
fn update_rename_entry() {
    let tmp = TempDir::new("update-rename");
    let archive = base_archive(tmp.path(), None);
    let ops = vec![UpdateOp::Rename {
        from: EntrySelection::Raw(EntryPath::from_utf8("project/a.txt")),
        to: EntryPath::from_utf8("project/renamed.txt"),
    }];
    run_update(&archive, &ops, &UpdateOptions::default()).unwrap();
    let names = list_names(&archive, None);
    assert!(names.contains(&"project/renamed.txt".to_string()));
    assert!(!names.contains(&"project/a.txt".to_string()));
    assert_unzip_t(&archive);

    // The renamed entry's content is intact.
    let opts = OpenOptions::default();
    let mut reader = engine().open(&archive, &opts).unwrap();
    let mut data = Vec::new();
    reader
        .read_entry(&EntryPath::from_utf8("project/renamed.txt"), &mut |entry| {
            entry.read_to_end(&mut data)?;
            Ok(())
        })
        .unwrap();
    assert_eq!(data, b"alpha");

    // Renaming a missing entry fails and leaves the archive intact.
    let before = fs::read(&archive).unwrap();
    let ops = vec![UpdateOp::Rename {
        from: EntrySelection::Raw(EntryPath::from_utf8("missing.txt")),
        to: EntryPath::from_utf8("whatever.txt"),
    }];
    let err = run_update(&archive, &ops, &UpdateOptions::default()).unwrap_err();
    assert!(matches!(err, FormatError::Other(_)));
    assert_eq!(
        fs::read(&archive).unwrap(),
        before,
        "archive must be untouched"
    );
}

#[test]
fn update_rename_directory_moves_its_complete_subtree() {
    for source in ["project/sub", "project/sub/"] {
        let tmp = TempDir::new("update-rename-directory");
        let archive = base_archive(tmp.path(), None);
        run_update(
            &archive,
            &[UpdateOp::Rename {
                from: EntrySelection::Raw(EntryPath::from_utf8(source)),
                to: EntryPath::from_utf8("整理/资料/"),
            }],
            &UpdateOptions::default(),
        )
        .unwrap();
        let names = list_names(&archive, None);
        assert!(names.contains(&"整理/资料/b.txt".into()), "{names:?}");
        assert!(names.contains(&"整理/资料/".into()), "{names:?}");
        assert!(!names.iter().any(|name| name.starts_with("project/sub")));
        assert!(names.contains(&"project/a.txt".into()));
        let mut reader = engine().open(&archive, &OpenOptions::default()).unwrap();
        let mut data = Vec::new();
        reader
            .read_entry(&EntryPath::from_utf8("整理/资料/b.txt"), &mut |entry| {
                entry.read_to_end(&mut data)?;
                Ok(())
            })
            .unwrap();
        assert_eq!(data, b"bravo");
        assert_unzip_t(&archive);
    }

    for names in [
        &[
            "a",
            "a/",
            "a/child.txt",
            "a/nested/deep.txt",
            "a-old/keep.txt",
        ][..],
        &[
            "a/",
            "a/child.txt",
            "a/nested/deep.txt",
            "a",
            "a-old/keep.txt",
        ][..],
        &["a", "a/child.txt", "a/nested/deep.txt", "a-old/keep.txt"][..],
    ] {
        let tmp = TempDir::new("update-rename-file-directory");
        let archive = named_archive(tmp.path(), names);
        let mut expected = raw_entry_snapshots(&archive);
        for entry in &mut expected {
            if let Some(suffix) = entry.name.strip_prefix(b"a/") {
                entry.name = [b"moved/".as_slice(), suffix].concat();
            }
        }
        run_update(
            &archive,
            &[UpdateOp::Rename {
                from: EntrySelection::Display("a/".into()),
                to: EntryPath::from_utf8("moved/"),
            }],
            &UpdateOptions::default(),
        )
        .unwrap();
        assert_eq!(raw_entry_snapshots(&archive), expected, "{names:?}");
        let mut reader = zip::ZipArchive::new(fs::File::open(&archive).unwrap()).unwrap();
        for (name, original) in [
            ("a", "a"),
            ("moved/child.txt", "a/child.txt"),
            ("moved/nested/deep.txt", "a/nested/deep.txt"),
            ("a-old/keep.txt", "a-old/keep.txt"),
        ] {
            let mut payload = Vec::new();
            reader
                .by_name(name)
                .unwrap()
                .read_to_end(&mut payload)
                .unwrap();
            assert_eq!(payload, original.as_bytes(), "{name}");
        }
        assert_unzip_t(&archive);
    }
}

#[test]
fn update_rename_implicit_directory_preserves_unrelated_prefixes() {
    use std::io::Write;
    let tmp = TempDir::new("update-rename-implicit-directory");
    let archive = tmp.path().join("implicit.zip");
    let mut writer = zip::ZipWriter::new(fs::File::create(&archive).unwrap());
    for name in ["docs/a.txt", "docs/nested/b.txt", "docs-old/keep.txt"] {
        writer
            .start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(name.as_bytes()).unwrap();
    }
    writer.finish().unwrap();
    run_update(
        &archive,
        &[UpdateOp::Rename {
            from: EntrySelection::Raw(EntryPath::from_utf8("docs/")),
            to: EntryPath::from_utf8("archive/notes/"),
        }],
        &UpdateOptions::default(),
    )
    .unwrap();
    assert_eq!(
        list_names(&archive, None),
        [
            "archive/notes/a.txt",
            "archive/notes/nested/b.txt",
            "docs-old/keep.txt"
        ]
    );
    assert_unzip_t(&archive);
}

#[test]
fn update_directory_rename_rejects_unsafe_or_ambiguous_plans_atomically() {
    let tmp = TempDir::new("update-directory-conflicts");
    let archive = base_archive(tmp.path(), None);
    let before = fs::read(&archive).unwrap();
    let cases = [
        vec![("project/sub/", "project/sub/child/")],
        vec![("project/sub/", "project/")],
        vec![("project/sub/", "project/a.txt/child/")],
        vec![("project/sub/", "../outside/")],
        vec![("project/sub/", "/outside/")],
        vec![("project/sub/", "C:\\outside\\")],
        vec![("project/sub/", "")],
        vec![("project/sub/", "project/sub/")],
        vec![("project/a.txt", "directory/")],
        vec![("project/sub/", "one/"), ("project/sub/", "two/")],
        vec![("project/", "one/"), ("project/sub/", "two/")],
        vec![("project/a.txt", "same.txt"), ("project/c.log", "same.txt")],
    ];
    for case in cases {
        let ops: Vec<_> = case
            .iter()
            .map(|(from, to)| UpdateOp::Rename {
                from: EntrySelection::Raw(EntryPath::from_utf8(*from)),
                to: EntryPath::from_utf8(*to),
            })
            .collect();
        assert!(
            run_update(&archive, &ops, &UpdateOptions::default()).is_err(),
            "{case:?}"
        );
        assert_eq!(fs::read(&archive).unwrap(), before, "{case:?}");
        assert_no_update_temp(tmp.path());
    }

    for names in [
        &["a", "a/", "a/child.txt"][..],
        &["a/", "a/child.txt", "a"][..],
        &["a", "a/child.txt"][..],
    ] {
        let tmp = TempDir::new("update-rename-ambiguous-source");
        let archive = named_archive(tmp.path(), names);
        let before = fs::read(&archive).unwrap();
        for from in [
            EntrySelection::Display("a".into()),
            EntrySelection::Raw(EntryPath::from_utf8("a")),
        ] {
            assert_other_contains(
                run_update(
                    &archive,
                    &[UpdateOp::Rename {
                        from,
                        to: EntryPath::from_utf8("moved/"),
                    }],
                    &UpdateOptions::default(),
                )
                .unwrap_err(),
                "ambiguous",
            );
            assert_eq!(fs::read(&archive).unwrap(), before, "{names:?}");
            assert_no_update_temp(tmp.path());
        }
    }
}

#[test]
fn update_implicit_directory_targets_do_not_merge_unrelated_trees() {
    use std::io::Write;
    let tmp = TempDir::new("update-implicit-conflicts");
    let archive = tmp.path().join("implicit.zip");
    let mut writer = zip::ZipWriter::new(fs::File::create(&archive).unwrap());
    for name in ["docs/a.txt", "other/b.txt"] {
        writer
            .start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(b"content").unwrap();
    }
    writer.finish().unwrap();
    let before = fs::read(&archive).unwrap();
    for mappings in [
        vec![("docs/", "other/")],
        vec![("docs/", "target/"), ("other/", "target/")],
    ] {
        let ops: Vec<_> = mappings
            .iter()
            .map(|(from, to)| UpdateOp::Rename {
                from: EntrySelection::Raw(EntryPath::from_utf8(*from)),
                to: EntryPath::from_utf8(*to),
            })
            .collect();
        assert!(run_update(&archive, &ops, &UpdateOptions::default()).is_err());
        assert_eq!(fs::read(&archive).unwrap(), before);
    }
}

#[test]
fn update_encrypted_directory_rename_preserves_payloads_without_a_password() {
    let tmp = TempDir::new("update-encrypted-directory");
    let archive = base_archive(tmp.path(), Some("directory-test-password"));
    run_update(
        &archive,
        &[UpdateOp::Rename {
            from: EntrySelection::Raw(EntryPath::from_utf8("project/")),
            to: EntryPath::from_utf8("资料/"),
        }],
        &UpdateOptions::default(),
    )
    .unwrap();
    let options = OpenOptions {
        password: Some(Password::new("directory-test-password")),
        encoding_override: None,
    };
    let entries = engine().list(&archive, &options).unwrap();
    assert!(entries
        .iter()
        .filter(|entry| entry.size > 0)
        .all(|entry| entry.encrypted));
    let mut reader = engine().open(&archive, &options).unwrap();
    for (name, expected) in [
        ("资料/a.txt", b"alpha".as_slice()),
        ("资料/sub/b.txt", b"bravo".as_slice()),
    ] {
        let mut data = Vec::new();
        reader
            .read_entry(&EntryPath::from_utf8(name), &mut |entry| {
                entry.read_to_end(&mut data)?;
                Ok(())
            })
            .unwrap();
        assert_eq!(data, expected);
    }
    assert!(!list_names(&archive, Some("directory-test-password"))
        .iter()
        .any(|name| name.starts_with("project/")));
}

#[test]
fn update_rejects_target_conflicts_without_explicit_delete() {
    let tmp = TempDir::new("update-conflicts");
    let archive = base_archive(tmp.path(), None);
    fs::write(tmp.path().join("new.txt"), b"replacement").unwrap();

    let before = fs::read(&archive).unwrap();
    let err = run_update(
        &archive,
        &[UpdateOp::Rename {
            from: EntrySelection::Raw(EntryPath::from_utf8("project/a.txt")),
            to: EntryPath::from_utf8("project/sub/b.txt"),
        }],
        &UpdateOptions::default(),
    )
    .unwrap_err();
    assert_other_contains(err, "already exists");
    assert_eq!(fs::read(&archive).unwrap(), before);

    let err = run_update(
        &archive,
        &[UpdateOp::Add {
            src: tmp.path().join("new.txt"),
            dest: EntryPath::from_utf8("project/a.txt"),
        }],
        &UpdateOptions::default(),
    )
    .unwrap_err();
    assert_other_contains(err, "already exists");
    assert_eq!(fs::read(&archive).unwrap(), before);

    let err = run_update(
        &archive,
        &[UpdateOp::AddDir {
            path: EntryPath::from_utf8("project"),
        }],
        &UpdateOptions::default(),
    )
    .unwrap_err();
    assert_other_contains(err, "already exists");
    assert_eq!(fs::read(&archive).unwrap(), before);

    let err = run_update(
        &archive,
        &[
            UpdateOp::Rename {
                from: EntrySelection::Raw(EntryPath::from_utf8("project/a.txt")),
                to: EntryPath::from_utf8("dup.txt"),
            },
            UpdateOp::Rename {
                from: EntrySelection::Raw(EntryPath::from_utf8("project/sub/b.txt")),
                to: EntryPath::from_utf8("dup.txt"),
            },
        ],
        &UpdateOptions::default(),
    )
    .unwrap_err();
    assert_other_contains(err, "duplicate update target");
    assert_eq!(fs::read(&archive).unwrap(), before);

    run_update(
        &archive,
        &[
            UpdateOp::Delete {
                pattern: "project/a.txt".into(),
            },
            UpdateOp::Add {
                src: tmp.path().join("new.txt"),
                dest: EntryPath::from_utf8("project/a.txt"),
            },
        ],
        &UpdateOptions::default(),
    )
    .unwrap();

    let mut reader = engine().open(&archive, &OpenOptions::default()).unwrap();
    let mut data = Vec::new();
    reader
        .read_entry(&EntryPath::from_utf8("project/a.txt"), &mut |entry| {
            entry.read_to_end(&mut data)?;
            Ok(())
        })
        .unwrap();
    assert_eq!(data, b"replacement");
    assert_unzip_t(&archive);
}

#[test]
fn update_combined_add_delete_rename() {
    for comment in [
        Vec::new(),
        "归档说明：保留原始字节。".as_bytes().to_vec(),
        b"\xff\xfe\0\x80".to_vec(),
        vec![b'x'; usize::from(u16::MAX)],
    ] {
        let tmp = TempDir::new("update-combo");
        let archive = base_archive(tmp.path(), None);
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&archive)
            .unwrap();
        let mut writer = zip::ZipWriter::new_append(file).unwrap();
        writer
            .set_raw_comment(comment.clone().into_boxed_slice())
            .unwrap();
        writer.finish().unwrap();
        assert_eq!(
            zip::ZipArchive::new(fs::File::open(&archive).unwrap())
                .unwrap()
                .comment(),
            comment
        );
        let external_comment = command_exists("unzip").then(|| {
            let output = Command::new("unzip")
                .arg("-z")
                .arg(&archive)
                .output()
                .unwrap();
            assert!(output.status.success());
            output.stdout
        });
        fs::write(tmp.path().join("fresh.txt"), b"fresh").unwrap();
        let ops = vec![
            UpdateOp::Add {
                src: tmp.path().join("fresh.txt"),
                dest: EntryPath::from_utf8("fresh.txt"),
            },
            UpdateOp::AddDir {
                path: EntryPath::from_utf8("fresh-folder/"),
            },
            UpdateOp::Delete {
                pattern: "*.log".into(),
            },
            UpdateOp::Rename {
                from: EntrySelection::Raw(EntryPath::from_utf8("project/sub/b.txt")),
                to: EntryPath::from_utf8("project/sub/beta.txt"),
            },
        ];
        run_update(&archive, &ops, &UpdateOptions::default()).unwrap();
        let names = list_names(&archive, None);
        assert!(names.contains(&"fresh.txt".to_string()));
        assert!(names.contains(&"fresh-folder/".to_string()));
        assert!(names.contains(&"project/sub/beta.txt".to_string()));
        assert!(!names.iter().any(|n| n.ends_with(".log")));
        assert!(!names.contains(&"project/sub/b.txt".to_string()));
        let mut updated = zip::ZipArchive::new(fs::File::open(&archive).unwrap()).unwrap();
        assert_eq!(
            updated.comment(),
            comment,
            "ZIP update changed archive comment bytes"
        );
        for (path, expected) in [
            ("project/a.txt", b"alpha".as_slice()),
            ("project/sub/beta.txt", b"bravo".as_slice()),
            ("fresh.txt", b"fresh".as_slice()),
        ] {
            let mut payload = Vec::new();
            updated
                .by_name(path)
                .unwrap()
                .read_to_end(&mut payload)
                .unwrap();
            assert_eq!(payload, expected);
            if external_comment.is_some() {
                let output = Command::new("unzip")
                    .arg("-p")
                    .arg(&archive)
                    .arg(path)
                    .output()
                    .unwrap();
                assert!(output.status.success());
                assert_eq!(output.stdout, expected);
            }
        }
        assert_unzip_t(&archive);
        if let Some(expected) = external_comment {
            let output = Command::new("unzip")
                .arg("-z")
                .arg(&archive)
                .output()
                .unwrap();
            assert!(output.status.success());
            assert_eq!(
                output.stdout, expected,
                "unzip displayed a changed archive comment"
            );
        }
    }
}

#[test]
fn update_encrypted_archive_without_password_keeps_encryption() {
    let tmp = TempDir::new("update-encrypted");
    let archive = base_archive(tmp.path(), Some("secret"));
    fs::write(tmp.path().join("plain.txt"), b"added later").unwrap();
    // No password supplied: old entries are raw-copied still encrypted.
    let ops = vec![UpdateOp::Add {
        src: tmp.path().join("plain.txt"),
        dest: EntryPath::from_utf8("plain.txt"),
    }];
    run_update(&archive, &ops, &UpdateOptions::default()).unwrap();

    let opts = OpenOptions::default();
    let entries = engine().list(&archive, &opts).unwrap();
    let old = entries
        .iter()
        .find(|e| e.path.display == "project/a.txt")
        .unwrap();
    assert!(old.encrypted, "raw-copied entry must stay encrypted");
    let new = entries
        .iter()
        .find(|e| e.path.display == "plain.txt")
        .unwrap();
    assert!(!new.encrypted);

    // Old content still decrypts with the original password.
    let open = OpenOptions {
        password: Some(Password::new("secret")),
        encoding_override: None,
    };
    let mut reader = engine().open(&archive, &open).unwrap();
    let mut data = Vec::new();
    reader
        .read_entry(&EntryPath::from_utf8("project/a.txt"), &mut |entry| {
            entry.read_to_end(&mut data)?;
            Ok(())
        })
        .unwrap();
    assert_eq!(data, b"alpha");
}

#[test]
fn update_unsupported_format_is_rejected() {
    let tmp = TempDir::new("update-unsupported");
    let root = tmp.path().join("d");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("f.txt"), b"x").unwrap();
    let dest = tmp.path().join("a.tar");
    engine()
        .create(
            &dest,
            &[root],
            &CreateOptions::default(),
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap();
    let ops = vec![UpdateOp::Delete {
        pattern: "f.txt".into(),
    }];
    let err = run_update(&dest, &ops, &UpdateOptions::default()).unwrap_err();
    assert!(matches!(err, FormatError::Unsupported(_)));
}
