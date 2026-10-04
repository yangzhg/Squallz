//! End-to-end format conversion tests: zip→7z, 7z→zip, zip→tar.gz,
//! tar.gz→zip, password handling and unsupported-entry reporting.

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use common::{engine, read_archive_entries, TempDir};
use squallz_core::api::{
    ControlToken, CreateOptions, ExtractOptions, FormatError, NoProgress, OpenOptions, Password,
};
use squallz_core::{inspect_create_destination, CreateArtifactKind, CreateCommitPolicy};

/// Builds a small tree and packs it into `name` under `dir`, returning the
/// archive path.
fn make_archive(dir: &Path, name: &str) -> PathBuf {
    let root = dir.join("project");
    fs::create_dir_all(root.join("sub")).unwrap();
    fs::write(root.join("a.txt"), b"hello world").unwrap();
    fs::write(root.join("sub/b.txt"), vec![0xAB; 4096]).unwrap();
    let dest = dir.join(name);
    engine()
        .create(
            &dest,
            &[root],
            &CreateOptions::default(),
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap();
    dest
}

/// Converts `src` to `dest_name`, extracts the result and asserts the
/// content survived.
fn convert_and_check(dir: &Path, src: &Path, dest_name: &str) {
    let dest = dir.join(dest_name);
    let ctl = ControlToken::new();
    engine()
        .convert(
            src,
            &dest,
            &OpenOptions::default(),
            &CreateOptions::default(),
            &NoProgress,
            &ctl,
        )
        .unwrap();
    let out = dir.join(format!("x-{dest_name}"));
    engine()
        .extract(
            &dest,
            &out,
            None,
            &OpenOptions::default(),
            &ExtractOptions::default(),
            &NoProgress,
            &ctl,
        )
        .unwrap();
    assert_eq!(
        fs::read(out.join("project/a.txt")).unwrap(),
        b"hello world",
        "{dest_name}: a.txt differs"
    );
    assert_eq!(
        fs::read(out.join("project/sub/b.txt")).unwrap(),
        vec![0xAB; 4096],
        "{dest_name}: b.txt differs"
    );
}

fn make_hardlink_tar(path: &Path) {
    let file = fs::File::create(path).unwrap();
    let mut builder = tar::Builder::new(file);
    let data = b"original";
    let mut header = tar::Header::new_gnu();
    header.set_mode(0o644);
    header.set_size(data.len() as u64);
    header.set_entry_type(tar::EntryType::Regular);
    builder
        .append_data(&mut header, "original.txt", data.as_slice())
        .unwrap();

    let mut link = tar::Header::new_gnu();
    link.set_mode(0o644);
    link.set_size(0);
    link.set_entry_type(tar::EntryType::Link);
    builder
        .append_link(&mut link, "copy.txt", "original.txt")
        .unwrap();
    builder.finish().unwrap();
}

const SOLID_ENTRY_BYTES: usize = 256 * 1024;

fn make_solid_sevenz(path: &Path) {
    use sevenz_rust2::{
        ArchiveEntry, ArchiveWriter, EncoderConfiguration, EncoderMethod, SourceReader,
    };
    use std::io::{Cursor, Read};
    use std::time::{Duration, UNIX_EPOCH};

    let modified = UNIX_EPOCH + Duration::from_secs(1_714_979_291);
    let entry = |name: &str, mode: u32| {
        let mut entry = if mode & 0o170000 == 0o040000 {
            ArchiveEntry::new_directory(name)
        } else {
            ArchiveEntry::new_file(name)
        };
        entry.has_windows_attributes = true;
        entry.windows_attributes = 0x8000 | (mode << 16);
        entry.has_last_modified_date = true;
        entry.last_modified_date = sevenz_rust2::NtTime::try_from(modified).unwrap();
        entry
    };
    let mut writer = ArchiveWriter::new(fs::File::create(path).unwrap()).unwrap();
    writer.set_content_methods(vec![EncoderConfiguration::new(EncoderMethod::COPY)]);
    for name in ["tree", "tree/empty-dir"] {
        writer
            .push_archive_entry(entry(name, 0o040750), None::<Cursor<Vec<u8>>>)
            .unwrap();
    }
    let sources: Vec<Box<dyn Read>> = vec![
        Box::new(std::io::repeat(b'a').take(SOLID_ENTRY_BYTES as u64)),
        Box::new(Cursor::new(b"first.bin")),
        Box::new(std::io::repeat(b'b').take(SOLID_ENTRY_BYTES as u64)),
    ];
    writer
        .push_archive_entries(
            vec![
                entry("tree/first.bin", 0o100640),
                entry("tree/link", 0o120777),
                entry("tree/second.bin", 0o100600),
            ],
            sources.into_iter().map(SourceReader::new).collect(),
        )
        .unwrap();
    writer
        .push_archive_entry(entry("tree/empty-file", 0o100644), None::<Cursor<Vec<u8>>>)
        .unwrap();
    writer.finish().unwrap();
}

#[test]
fn solid_sevenz_conversion_preserves_contents_metadata_and_progress_including_split_outputs() {
    use squallz_core::api::{CompressionLevel, EntryPath, EntryType, ProgressPhase, ProgressSink};
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    };

    #[derive(Default)]
    struct ProgressRecorder {
        converting: AtomicBool,
        events: Mutex<Vec<(u64, u64)>>,
    }
    impl ProgressSink for ProgressRecorder {
        fn on_phase(&self, phase: ProgressPhase, _interruptible: bool) {
            self.converting
                .store(phase == ProgressPhase::ArchiveConvert, Ordering::Relaxed);
        }
        fn on_progress(&self, done: u64, total: u64, _current: &EntryPath) {
            if self.converting.load(Ordering::Relaxed) {
                self.events.lock().unwrap().push((done, total));
            }
        }
    }
    let tmp = TempDir::new("convert-solid-7z");
    let source = tmp.path().join("source.7z");
    make_solid_sevenz(&source);
    let original = read_archive_entries(&engine(), &source, &OpenOptions::default()).unwrap();
    assert_eq!(original.len(), 6);
    for split_size in [None, Some(128 * 1024)] {
        let destination = tmp.path().join(if split_size.is_some() {
            "split.zip"
        } else {
            "output.zip"
        });
        let progress = ProgressRecorder::default();
        let report = engine()
            .convert_with_report(
                &source,
                &destination,
                &OpenOptions::default(),
                &CreateOptions {
                    level: CompressionLevel::Store,
                    split_size,
                    ..CreateOptions::default()
                },
                &progress,
                &ControlToken::default(),
            )
            .unwrap();
        if split_size.is_some() {
            assert!(report.outputs.len() > 1);
            assert_eq!(report.split_volume_count, Some(report.outputs.len()));
            assert!(report
                .outputs
                .iter()
                .all(|path| fs::metadata(path).unwrap().len() <= 128 * 1024));
        }
        let entries =
            read_archive_entries(&engine(), &report.primary_output, &OpenOptions::default())
                .unwrap();
        assert_eq!(entries.len(), original.len());
        for before in &original {
            let name = before
                .path
                .normalized_display(matches!(before.entry_type, EntryType::Dir));
            let after = entries
                .iter()
                .find(|entry| entry.path.display == name)
                .unwrap();
            assert_eq!(after.entry_type, before.entry_type, "{name}");
            assert_eq!(after.modified, before.modified, "{name}");
            assert_eq!(
                after.unix_mode.map(|mode| mode & 0o7777),
                before.unix_mode.map(|mode| mode & 0o7777),
                "{name}"
            );
        }
        let out = tmp.path().join(if split_size.is_some() {
            "split-out"
        } else {
            "out"
        });
        engine()
            .extract(
                &report.primary_output,
                &out,
                None,
                &OpenOptions::default(),
                &ExtractOptions::default(),
                &NoProgress,
                &ControlToken::default(),
            )
            .unwrap();
        assert_eq!(
            fs::read(out.join("tree/first.bin")).unwrap(),
            vec![b'a'; SOLID_ENTRY_BYTES]
        );
        assert_eq!(
            fs::read(out.join("tree/second.bin")).unwrap(),
            vec![b'b'; SOLID_ENTRY_BYTES]
        );
        assert!(out.join("tree/empty-dir").is_dir());
        assert!(fs::read(out.join("tree/empty-file")).unwrap().is_empty());
        #[cfg(unix)]
        assert_eq!(
            fs::read_link(out.join("tree/link")).unwrap(),
            Path::new("first.bin")
        );
        let events = progress.events.lock().unwrap();
        let total = (2 * SOLID_ENTRY_BYTES) as u64;
        assert!(events.len() > 4);
        assert!(events
            .iter()
            .all(|(done, expected)| *expected == total && *done <= total));
        assert!(events.windows(2).all(|pair| pair[0].0 <= pair[1].0));
        assert!(events
            .iter()
            .any(|(done, _)| *done > SOLID_ENTRY_BYTES as u64 && *done < total));
        assert_eq!(events.last(), Some(&(total, total)));
    }
}

#[test]
fn solid_sevenz_late_corruption_and_cancellation_preserve_the_existing_output() {
    use squallz_core::api::{EntryPath, ProgressSink};
    use std::io::{Seek, SeekFrom, Write};

    struct CancelOnSecondFile<'a>(&'a ControlToken);
    impl ProgressSink for CancelOnSecondFile<'_> {
        fn on_progress(&self, done: u64, _total: u64, _current: &EntryPath) {
            if done > SOLID_ENTRY_BYTES as u64 {
                self.0.cancel();
            }
        }
    }
    let tmp = TempDir::new("convert-solid-7z-failure");
    let source = tmp.path().join("source.7z");
    make_solid_sevenz(&source);
    let mut archive = fs::OpenOptions::new().write(true).open(&source).unwrap();
    archive
        .seek(SeekFrom::Start(
            (32 + 2 * SOLID_ENTRY_BYTES + b"first.bin".len() - 1) as u64,
        ))
        .unwrap();
    archive.write_all(b"c").unwrap();
    drop(archive);
    let destination = tmp.path().join("output.zip");
    for cancel in [true, false] {
        fs::write(&destination, b"original output").unwrap();
        let guard = inspect_create_destination(&destination, CreateArtifactKind::Archive)
            .unwrap()
            .guard
            .unwrap();
        let ctl = ControlToken::default();
        let cancelling = CancelOnSecondFile(&ctl);
        let progress: &dyn ProgressSink = if cancel { &cancelling } else { &NoProgress };
        let result = engine().convert_with_policy(
            &source,
            &destination,
            &OpenOptions::default(),
            &CreateOptions::default(),
            CreateCommitPolicy::ReplaceIfUnchanged(guard),
            progress,
            &ctl,
        );
        if cancel {
            assert!(matches!(result, Err(FormatError::Cancelled)), "{result:?}");
        } else {
            assert!(
                matches!(
                    result,
                    Err(FormatError::Io(_) | FormatError::CorruptArchive(_))
                ),
                "{result:?}"
            );
        }
        assert_eq!(fs::read(&destination).unwrap(), b"original output");
        assert_eq!(
            fs::read_dir(tmp.path()).unwrap().count(),
            2,
            "failed conversions must remove their staging artifacts"
        );
    }
}

#[test]
fn sevenz_early_end_preserves_existing_conversion_outputs() {
    use sevenz_rust2::{ArchiveEntry, ArchiveWriter, EncoderConfiguration, EncoderMethod};
    use std::io::{Read, Seek, SeekFrom, Write};

    let tmp = TempDir::new("convert-7z-early-end");
    let source = tmp.path().join("source.7z");
    let mut writer = ArchiveWriter::new(fs::File::create(&source).unwrap()).unwrap();
    writer.set_content_methods(vec![EncoderConfiguration::new(EncoderMethod::LZMA2)]);
    writer
        .push_archive_entry(
            ArchiveEntry::new_file("file.bin"),
            Some(std::io::repeat(b'x').take(4096)),
        )
        .unwrap();
    drop(writer.finish().unwrap());
    let mut archive = fs::OpenOptions::new().write(true).open(&source).unwrap();
    archive.seek(SeekFrom::Start(32)).unwrap();
    archive.write_all(&[0]).unwrap();
    drop(archive);
    for extension in ["zip", "tar"] {
        let destination = tmp.path().join(format!("output.{extension}"));
        fs::write(&destination, b"original output").unwrap();
        let guard = inspect_create_destination(&destination, CreateArtifactKind::Archive)
            .unwrap()
            .guard
            .unwrap();
        let error = engine()
            .convert_with_policy(
                &source,
                &destination,
                &OpenOptions::default(),
                &CreateOptions::default(),
                CreateCommitPolicy::ReplaceIfUnchanged(guard),
                &NoProgress,
                &ControlToken::default(),
            )
            .unwrap_err();
        assert!(
            matches!(error, FormatError::Io(ref error) if error.kind() == std::io::ErrorKind::UnexpectedEof)
        );
        assert_eq!(fs::read(&destination).unwrap(), b"original output");
        assert!(!fs::read_dir(tmp.path()).unwrap().any(|entry| entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains(".convert-")));
    }
}

#[test]
fn zip_to_7z_and_back() {
    let tmp = TempDir::new("convert-zip-7z");
    let zip = make_archive(tmp.path(), "src.zip");
    convert_and_check(tmp.path(), &zip, "mid.7z");
    convert_and_check(tmp.path(), &tmp.path().join("mid.7z"), "back.zip");
}

#[test]
fn sevenz_conversion_cancellation_and_corruption_preserve_existing_outputs() {
    use squallz_core::api::{CompressionLevel, EntryPath, ProgressSink};
    use std::io::{Seek, SeekFrom, Write};

    struct CancelOnProgress<'a>(&'a ControlToken);
    impl ProgressSink for CancelOnProgress<'_> {
        fn on_progress(&self, done: u64, _total: u64, _current: &EntryPath) {
            if done > 0 {
                self.0.cancel();
            }
        }
    }
    let tmp = TempDir::new("convert-7z-stream-failure");
    let input = tmp.path().join("file.bin");
    let source = tmp.path().join("source.7z");
    let size = 1024 * 1024;
    fs::write(&input, vec![b'x'; size]).unwrap();
    engine()
        .create(
            &source,
            &[input],
            &CreateOptions {
                level: CompressionLevel::Store,
                ..CreateOptions::default()
            },
            &NoProgress,
            &ControlToken::default(),
        )
        .unwrap();
    let mut archive = fs::OpenOptions::new().write(true).open(&source).unwrap();
    archive.seek(SeekFrom::Start(32 + size as u64 - 1)).unwrap();
    archive.write_all(b"y").unwrap();
    drop(archive);

    for extension in ["zip", "gz"] {
        let destination = tmp.path().join(format!("output.{extension}"));
        for cancel in [true, false] {
            fs::write(&destination, b"original output").unwrap();
            let guard = inspect_create_destination(&destination, CreateArtifactKind::Archive)
                .unwrap()
                .guard
                .unwrap();
            let ctl = ControlToken::default();
            let cancelling = CancelOnProgress(&ctl);
            let progress: &dyn ProgressSink = if cancel { &cancelling } else { &NoProgress };
            let error = engine()
                .convert_with_policy(
                    &source,
                    &destination,
                    &OpenOptions::default(),
                    &CreateOptions::default(),
                    CreateCommitPolicy::ReplaceIfUnchanged(guard),
                    progress,
                    &ctl,
                )
                .unwrap_err();
            if cancel {
                assert!(
                    matches!(error, FormatError::Cancelled),
                    "{extension}: {error:?}"
                );
            } else {
                assert!(
                    matches!(error, FormatError::Io(_) | FormatError::CorruptArchive(_)),
                    "{extension}: {error:?}"
                );
            }
            assert_eq!(fs::read(&destination).unwrap(), b"original output");
            assert!(!fs::read_dir(tmp.path()).unwrap().any(|entry| entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains(".convert-")));
        }
    }
}

#[test]
fn zip_to_tar_gz_and_back() {
    let tmp = TempDir::new("convert-zip-targz");
    let zip = make_archive(tmp.path(), "src.zip");
    convert_and_check(tmp.path(), &zip, "mid.tar.gz");
    convert_and_check(tmp.path(), &tmp.path().join("mid.tar.gz"), "back.zip");
}

#[test]
fn unsplit_conversion_replaces_without_hidden_backup_artifacts() {
    let tmp = TempDir::new("convert-atomic-replace");
    let source = make_archive(tmp.path(), "source.zip");
    let destination = tmp.path().join("converted.7z");
    fs::write(&destination, b"previous output").unwrap();

    let report = engine()
        .convert_with_report(
            &source,
            &destination,
            &OpenOptions::default(),
            &CreateOptions::default(),
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap();

    assert_eq!(report.primary_output, destination);
    assert_eq!(report.outputs, vec![destination.clone()]);
    assert!(report.preserved_outputs.is_empty());
    assert_eq!(report.split_volume_count, None);
    assert_eq!(
        report.total_output_bytes,
        fs::metadata(&destination).unwrap().len()
    );
    assert_eq!(
        read_archive_entries(&engine(), &destination, &OpenOptions::default())
            .unwrap()
            .iter()
            .filter(|entry| matches!(entry.entry_type, squallz_core::api::EntryType::File))
            .count(),
        2
    );
    assert!(!fs::read_dir(tmp.path()).unwrap().any(|entry| {
        let name = entry.unwrap().file_name();
        let name = name.to_string_lossy();
        name.contains("replace-backup") || name.contains(".convert-")
    }));
}

#[test]
fn conversion_policies_preserve_unapproved_changed_and_late_outputs() {
    use squallz_core::api::{EntryPath, ProgressSink};
    use squallz_core::{ArchiveRepairKind, ArchiveRepairOptions};
    use std::sync::atomic::{AtomicBool, Ordering};

    struct LateOutput<'a> {
        destination: &'a Path,
        written: AtomicBool,
    }
    impl ProgressSink for LateOutput<'_> {
        fn on_progress(&self, _done: u64, _total: u64, _current: &EntryPath) {
            if !self.written.swap(true, Ordering::Relaxed) {
                fs::write(self.destination, b"late output from another app").unwrap();
            }
        }
    }

    struct SourceProgress<'a> {
        visited: AtomicBool,
        cancel: Option<&'a ControlToken>,
    }
    impl ProgressSink for SourceProgress<'_> {
        fn on_progress(&self, _done: u64, _total: u64, _current: &EntryPath) {
            self.visited.store(true, Ordering::Relaxed);
            if let Some(ctl) = self.cancel {
                ctl.cancel();
            }
        }
    }

    let tmp = TempDir::new("convert-explicit-destination-policy");
    let source = make_archive(tmp.path(), "source.zip");
    let destination = tmp.path().join("converted.7z");
    fs::write(&destination, b"unapproved output").unwrap();
    let ctl = ControlToken::new();

    let error = engine()
        .convert_with_policy(
            &source,
            &destination,
            &OpenOptions::default(),
            &CreateOptions::default(),
            CreateCommitPolicy::NoReplace,
            &NoProgress,
            &ctl,
        )
        .unwrap_err();
    assert!(error.is_output_exists());
    assert_eq!(fs::read(&destination).unwrap(), b"unapproved output");

    let guard = inspect_create_destination(&destination, CreateArtifactKind::Archive)
        .unwrap()
        .guard
        .unwrap();
    fs::write(&destination, b"newer output from another app").unwrap();
    let error = engine()
        .convert_with_policy(
            &source,
            &destination,
            &OpenOptions::default(),
            &CreateOptions::default(),
            CreateCommitPolicy::ReplaceIfUnchanged(guard),
            &NoProgress,
            &ctl,
        )
        .unwrap_err();
    assert!(error.is_destination_changed());
    assert_eq!(
        fs::read(&destination).unwrap(),
        b"newer output from another app"
    );
    assert!(!fs::read_dir(tmp.path()).unwrap().any(|entry| {
        let name = entry.unwrap().file_name();
        let name = name.to_string_lossy();
        name.contains(".convert-")
            || name.contains("replace-backup")
            || name.starts_with(".squallz-update-")
    }));

    let current_guard = inspect_create_destination(&destination, CreateArtifactKind::Archive)
        .unwrap()
        .guard
        .unwrap();
    engine()
        .convert_with_policy(
            &source,
            &destination,
            &OpenOptions::default(),
            &CreateOptions::default(),
            CreateCommitPolicy::ReplaceIfUnchanged(current_guard),
            &NoProgress,
            &ctl,
        )
        .unwrap();
    assert_eq!(
        read_archive_entries(&engine(), &destination, &OpenOptions::default())
            .unwrap()
            .iter()
            .filter(|entry| matches!(entry.entry_type, squallz_core::api::EntryType::File))
            .count(),
        2
    );

    let source_before = fs::read(&source).unwrap();
    let repaired = tmp.path().join("repaired.zip");
    let late = LateOutput {
        destination: &repaired,
        written: AtomicBool::new(false),
    };
    let error = engine()
        .convert_with_atomic_replace(
            &source,
            &repaired,
            &OpenOptions::default(),
            &CreateOptions::default(),
            &late,
            &ctl,
        )
        .unwrap_err();
    assert!(late.written.load(Ordering::Relaxed));
    assert!(error.is_output_exists(), "{error:?}");
    assert_eq!(
        fs::read(&repaired).unwrap(),
        b"late output from another app"
    );
    assert_eq!(fs::read(&source).unwrap(), source_before);
    assert!(!fs::read_dir(tmp.path()).unwrap().any(|entry| entry
        .unwrap()
        .file_name()
        .to_string_lossy()
        .contains(".convert-")));

    fs::remove_file(&repaired).unwrap();
    assert!(!engine()
        .convert_with_atomic_replace(
            &source,
            &repaired,
            &OpenOptions::default(),
            &CreateOptions::default(),
            &NoProgress,
            &ctl,
        )
        .unwrap());
    assert_eq!(
        read_archive_entries(&engine(), &repaired, &OpenOptions::default())
            .unwrap()
            .into_iter()
            .map(|entry| entry.path.display)
            .collect::<Vec<_>>(),
        read_archive_entries(&engine(), &source, &OpenOptions::default())
            .unwrap()
            .into_iter()
            .map(|entry| entry.path.display)
            .collect::<Vec<_>>()
    );

    let options = ArchiveRepairOptions {
        kind: ArchiveRepairKind::ZipIndexRebuild,
        create: CreateOptions::default(),
        safety_limits: squallz_core::api::SafetyLimits::default(),
    };
    let repair_destination = tmp.path().join("repair-late-output.zip");
    let source_progress = SourceProgress {
        visited: AtomicBool::new(false),
        cancel: None,
    };
    let rewrite_progress = LateOutput {
        destination: &repair_destination,
        written: AtomicBool::new(false),
    };
    let error = engine()
        .repair_archive(
            &source,
            &repair_destination,
            &options,
            &source_progress,
            &rewrite_progress,
            &ctl,
        )
        .unwrap_err();
    assert!(source_progress.visited.load(Ordering::Relaxed));
    assert!(rewrite_progress.written.load(Ordering::Relaxed));
    assert!(error.is_output_exists(), "{error:?}");
    assert_eq!(
        fs::read(&repair_destination).unwrap(),
        b"late output from another app"
    );
    assert_eq!(fs::read(&source).unwrap(), source_before);

    let cancelled_destination = tmp.path().join("cancelled-repair.zip");
    let cancelled_ctl = ControlToken::new();
    let source_progress = SourceProgress {
        visited: AtomicBool::new(false),
        cancel: Some(&cancelled_ctl),
    };
    let rewrite_progress = LateOutput {
        destination: &cancelled_destination,
        written: AtomicBool::new(false),
    };
    let error = engine()
        .repair_archive(
            &source,
            &cancelled_destination,
            &options,
            &source_progress,
            &rewrite_progress,
            &cancelled_ctl,
        )
        .unwrap_err();
    assert!(matches!(error, FormatError::Cancelled), "{error:?}");
    assert!(source_progress.visited.load(Ordering::Relaxed));
    assert!(!rewrite_progress.written.load(Ordering::Relaxed));
    assert!(!cancelled_destination.exists());
    assert_eq!(fs::read(&source).unwrap(), source_before);
    assert!(!fs::read_dir(tmp.path()).unwrap().any(|entry| entry
        .unwrap()
        .file_name()
        .to_string_lossy()
        .contains(".convert-")));
}

#[test]
fn split_conversion_requires_and_returns_an_artifact_report() {
    let tmp = TempDir::new("convert-split-report");
    let source = make_archive(tmp.path(), "source.zip");
    let destination = tmp.path().join("converted.7z");
    let options = CreateOptions {
        split_size: Some(1024),
        ..CreateOptions::default()
    };
    let ctl = ControlToken::new();

    let error = engine()
        .convert(
            &source,
            &destination,
            &OpenOptions::default(),
            &options,
            &NoProgress,
            &ctl,
        )
        .unwrap_err();
    assert!(matches!(
        error,
        FormatError::Unsupported(ref detail) if detail.contains("convert_with_report")
    ));

    let error = engine()
        .convert_with_atomic_replace(
            &source,
            &destination,
            &OpenOptions::default(),
            &options,
            &NoProgress,
            &ctl,
        )
        .unwrap_err();
    assert!(matches!(
        error,
        FormatError::Unsupported(ref detail)
            if detail.contains("convert_with_atomic_replace") && detail.contains("split")
    ));
    assert!(!destination.exists());

    let first = engine()
        .convert_with_report(
            &source,
            &destination,
            &OpenOptions::default(),
            &options,
            &NoProgress,
            &ctl,
        )
        .unwrap();
    assert_eq!(first.split_volume_count, Some(first.outputs.len()));
    assert!(first.preserved_outputs.is_empty());

    let error = engine()
        .convert_with_report_policy(
            &source,
            &destination,
            &OpenOptions::default(),
            &options,
            CreateCommitPolicy::NoReplace,
            &NoProgress,
            &ctl,
        )
        .unwrap_err();
    assert!(error.is_output_exists());

    let second = engine()
        .convert_with_report(
            &source,
            &destination,
            &OpenOptions::default(),
            &options,
            &NoProgress,
            &ctl,
        )
        .unwrap();
    assert_eq!(second.preserved_outputs.len(), first.outputs.len());
    for path in second.outputs.iter().chain(second.preserved_outputs.iter()) {
        assert!(
            path.is_file(),
            "reported path is missing: {}",
            path.display()
        );
    }
}

#[test]
fn conversion_plan_reuses_the_real_split_output_layout() {
    let tmp = TempDir::new("convert-plan-split");
    let source = make_archive(tmp.path(), "source.zip");
    let destination = tmp.path().join("converted.7z");
    let options = CreateOptions {
        split_size: Some(1024),
        ..CreateOptions::default()
    };
    let engine = engine();

    let plan = engine
        .plan_convert(&source, &destination, &OpenOptions::default(), &options)
        .unwrap();
    assert_eq!(plan.inputs.input_count, 1);
    assert_eq!(plan.inputs.files, 2);
    assert_eq!(plan.inputs.directories, 2);
    assert_eq!(plan.inputs.total_bytes, 4107);
    assert_eq!(plan.primary_output, tmp.path().join("converted.7z.001"));
    assert!(plan
        .split_volume_count_budget
        .is_some_and(|count| count > 1));
    assert!(plan.workspace_budget_bytes >= plan.final_output_budget_bytes);

    let report = engine
        .convert_with_report(
            &source,
            &destination,
            &OpenOptions::default(),
            &options,
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap();
    assert!(report.total_output_bytes <= plan.final_output_budget_bytes);
    assert!(plan
        .split_volume_count_budget
        .is_some_and(|budget| budget as usize >= report.split_volume_count.unwrap()));
}

#[test]
fn conversion_plan_rejects_invalid_single_stream_layout() {
    let tmp = TempDir::new("convert-plan-stream-layout");
    let source = make_archive(tmp.path(), "source.zip");
    let destination = tmp.path().join("converted.gz");

    let error = engine()
        .plan_convert(
            &source,
            &destination,
            &OpenOptions::default(),
            &CreateOptions::default(),
        )
        .unwrap_err();

    assert!(matches!(
        error,
        FormatError::Unsupported(ref detail)
            if detail.contains("gzip") && detail.contains("exactly one file")
    ));
    assert!(!destination.exists());
}

#[test]
fn conversion_plan_rejects_an_invalid_target_before_opening_the_source() {
    let tmp = TempDir::new("convert-plan-target-first");
    let source = tmp.path().join("missing-source.zip");
    let destination = tmp.path().join("converted.swm");

    let error = engine()
        .plan_convert(
            &source,
            &destination,
            &OpenOptions::default(),
            &CreateOptions::default(),
        )
        .unwrap_err();

    assert!(error.is_split_wim_creation_unsupported());
    assert_eq!(fs::read_dir(tmp.path()).unwrap().count(), 0);
}

#[test]
fn swm_destination_without_native_options_is_rejected_before_source_open_or_staging() {
    let tmp = TempDir::new("convert-split-wim-preflight");
    let source = tmp.path().join("missing-source.zip");
    let destination = tmp.path().join("image.swm");

    let error = engine()
        .convert_with_report(
            &source,
            &destination,
            &OpenOptions::default(),
            &CreateOptions::default(),
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap_err();

    assert!(error.is_split_wim_creation_unsupported());
    assert!(!destination.exists());
    assert_eq!(fs::read_dir(tmp.path()).unwrap().count(), 0);
}

#[test]
fn encrypted_source_to_encrypted_destination() {
    let tmp = TempDir::new("convert-encrypted");
    let root = tmp.path().join("data");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("secret.txt"), b"classified").unwrap();
    let src = tmp.path().join("src.zip");
    let ctl = ControlToken::new();
    let src_opts = CreateOptions {
        password: Some(Password::new("in-pass")),
        ..CreateOptions::default()
    };
    engine()
        .create(&src, &[root], &src_opts, &NoProgress, &ctl)
        .unwrap();

    // Wrong/missing source password fails.
    let err = engine()
        .convert(
            &src,
            &tmp.path().join("fail.7z"),
            &OpenOptions::default(),
            &CreateOptions::default(),
            &NoProgress,
            &ctl,
        )
        .unwrap_err();
    assert!(matches!(
        err,
        FormatError::PasswordRequired | FormatError::WrongPassword
    ));

    // Correct source password, new destination password.
    let dest = tmp.path().join("out.7z");
    let open = OpenOptions {
        password: Some(Password::new("in-pass")),
        encoding_override: None,
    };
    let create = CreateOptions {
        password: Some(Password::new("out-pass")),
        ..CreateOptions::default()
    };
    engine()
        .convert(&src, &dest, &open, &create, &NoProgress, &ctl)
        .unwrap();
    let out = tmp.path().join("extracted");
    let dest_open = OpenOptions {
        password: Some(Password::new("out-pass")),
        encoding_override: None,
    };
    engine()
        .extract(
            &dest,
            &out,
            None,
            &dest_open,
            &ExtractOptions::default(),
            &NoProgress,
            &ctl,
        )
        .unwrap();
    assert_eq!(
        fs::read(out.join("data/secret.txt")).unwrap(),
        b"classified"
    );
}

#[cfg(unix)]
#[test]
fn symlink_to_7z_reports_unsupported_with_entry() {
    let tmp = TempDir::new("convert-symlink");
    let root = tmp.path().join("tree");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("real.txt"), b"data").unwrap();
    std::os::unix::fs::symlink("real.txt", root.join("link.txt")).unwrap();
    let src = tmp.path().join("src.zip");
    let ctl = ControlToken::new();
    engine()
        .create(&src, &[root], &CreateOptions::default(), &NoProgress, &ctl)
        .unwrap();
    let dest = tmp.path().join("out.7z");
    fs::write(&dest, b"previous output").unwrap();
    let err = engine()
        .convert(
            &src,
            &dest,
            &OpenOptions::default(),
            &CreateOptions::default(),
            &NoProgress,
            &ctl,
        )
        .unwrap_err();
    match err {
        FormatError::Unsupported(detail) => {
            assert!(
                detail.contains("symbolic link"),
                "detail must name the entry type: {detail}"
            );
            assert!(
                detail.contains("tree/link.txt"),
                "detail must name the entry: {detail}"
            );
            assert!(
                detail.contains("real.txt"),
                "detail must name the link target: {detail}"
            );
            assert!(
                detail.contains("tar or zip"),
                "detail must suggest a preserving format: {detail}"
            );
        }
        other => panic!("expected Unsupported, got {other:?}"),
    }
    assert_eq!(fs::read(dest).unwrap(), b"previous output");
}

#[test]
fn hardlink_to_7z_reports_unsupported_with_entry_and_target() {
    let tmp = TempDir::new("convert-hardlink");
    let src = tmp.path().join("links.tar");
    make_hardlink_tar(&src);
    let ctl = ControlToken::new();
    let err = engine()
        .convert(
            &src,
            &tmp.path().join("out.7z"),
            &OpenOptions::default(),
            &CreateOptions::default(),
            &NoProgress,
            &ctl,
        )
        .unwrap_err();
    match err {
        FormatError::Unsupported(detail) => {
            assert!(
                detail.contains("hard link"),
                "detail must name the entry type: {detail}"
            );
            assert!(
                detail.contains("copy.txt"),
                "detail must name the entry: {detail}"
            );
            assert!(
                detail.contains("original.txt"),
                "detail must name the hardlink target: {detail}"
            );
            assert!(
                detail.contains("tar"),
                "detail must suggest a preserving format: {detail}"
            );
        }
        other => panic!("expected Unsupported, got {other:?}"),
    }
}

#[test]
fn single_file_zip_converts_to_plain_gz() {
    let tmp = TempDir::new("convert-gz");
    let root = tmp.path().join("one");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("only.txt"), b"single file payload").unwrap();
    let src = tmp.path().join("src.zip");
    let ctl = ControlToken::new();
    engine()
        .create(&src, &[root], &CreateOptions::default(), &NoProgress, &ctl)
        .unwrap();
    let dest = tmp.path().join("only.txt.gz");
    engine()
        .convert(
            &src,
            &dest,
            &OpenOptions::default(),
            &CreateOptions::default(),
            &NoProgress,
            &ctl,
        )
        .unwrap();
    // The virtual single-entry view of the .gz must decompress to the
    // original content.
    let out = tmp.path().join("x-gz");
    engine()
        .extract(
            &dest,
            &out,
            None,
            &OpenOptions::default(),
            &ExtractOptions::default(),
            &NoProgress,
            &ctl,
        )
        .unwrap();
    assert_eq!(
        fs::read(out.join("only.txt")).unwrap(),
        b"single file payload"
    );
}
