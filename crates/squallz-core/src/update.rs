use std::io;
use std::time::SystemTime;

use crate::api::{
    ArchiveFormat, ArchiveWriter, ControlToken, CreateOptions, EntryMeta, EntryPath, EntryType,
    FormatError, PreparedUpdateAdditions, ProgressSink, UpdateOp,
};
use crate::compound::ProgressRead;
use crate::create::TrackedInputRead;
use crate::filesystem_identity::PathIdentity;
use crate::inputs::{collect_prepared_input_as, PreparedInputItem};
use crate::CreateDestinationGuard;
use crate::PathFilter;

mod transaction;

pub(crate) struct PreparedAdditions {
    entries: Vec<PreparedAddition>,
}

struct PreparedAddition {
    input: Option<PreparedInputItem>,
    meta: EntryMeta,
    consumed: bool,
}

pub(crate) fn prepare_additions(
    ops: &[UpdateOp],
    opts: &CreateOptions,
    progress: &dyn ProgressSink,
    ctl: &ControlToken,
) -> Result<PreparedAdditions, FormatError> {
    let excludes = PathFilter::new(&opts.excludes)?;
    let mut entries = Vec::new();
    let mut scanned = 0u64;
    for op in ops {
        ctl.checkpoint()?;
        match op {
            UpdateOp::Add { src, dest } => {
                let additions = collect_prepared_input_as(src, dest, &excludes, ctl, |path| {
                    scanned = scanned.saturating_add(1);
                    progress.on_scan_progress(scanned, path);
                })?;
                for input in additions {
                    entries.push(PreparedAddition::from_input(input));
                }
            }
            UpdateOp::AddDir { path } => {
                let path = path.display.trim_end_matches('/');
                if path.is_empty() {
                    return Err(FormatError::Other("directory path cannot be empty".into()));
                }
                if !excludes.matches(path) {
                    let addition = PreparedAddition {
                        input: None,
                        meta: EntryMeta {
                            path: EntryPath::from_utf8(format!("{path}/")),
                            entry_type: EntryType::Dir,
                            size: 0,
                            compressed_size: None,
                            modified: Some(SystemTime::now()),
                            unix_mode: Some(0o755),
                            crc32: None,
                            encrypted: false,
                        },
                        consumed: false,
                    };
                    scanned = scanned.saturating_add(1);
                    progress.on_scan_progress(scanned, &addition.meta.path);
                    entries.push(addition);
                    ctl.checkpoint()?;
                }
            }
            UpdateOp::Delete { .. } | UpdateOp::Rename { .. } => {}
        }
    }
    Ok(PreparedAdditions { entries })
}

pub(crate) fn run_update_rewrite(
    format: &dyn ArchiveFormat,
    target: &std::path::Path,
    ops: &[UpdateOp],
    opts: &CreateOptions,
    progress: &dyn ProgressSink,
    ctl: &ControlToken,
) -> Result<(), FormatError> {
    let mut additions = prepare_additions(ops, opts, progress, ctl)?;
    transaction::run(format, target, ops, &mut additions, opts, progress, ctl)
}

pub(crate) fn commit_created_archive(
    target: &std::path::Path,
    staged: &std::path::Path,
    staged_file: std::fs::File,
    staged_identity: PathIdentity,
    guard: CreateDestinationGuard,
    progress: &dyn ProgressSink,
    ctl: &ControlToken,
) -> Result<(), FormatError> {
    transaction::commit_created_archive(
        target,
        staged,
        staged_file,
        staged_identity,
        guard,
        progress,
        ctl,
    )
}

impl PreparedAddition {
    fn from_input(input: PreparedInputItem) -> Self {
        let item = input.item();
        let meta = EntryMeta {
            path: item.name.clone(),
            entry_type: item.entry_type.clone(),
            size: item.size,
            compressed_size: None,
            modified: item.modified,
            unix_mode: item.unix_mode,
            crc32: None,
            encrypted: false,
        };
        Self {
            input: Some(input),
            meta,
            consumed: false,
        }
    }

    fn add_to(
        &self,
        writer: &mut dyn ArchiveWriter,
        progress: &dyn ProgressSink,
        ctl: &ControlToken,
        completed_bytes: u64,
        total_bytes: u64,
    ) -> Result<(), FormatError> {
        let Some(input) = &self.input else {
            return writer.add_entry(&self.meta, None);
        };
        if matches!(self.meta.entry_type, EntryType::File) {
            let mut file = input.open_file()?;
            let data = ProgressRead::new(
                &mut file,
                progress,
                ctl,
                &self.meta.path,
                completed_bytes,
                total_bytes,
                self.meta.size,
            );
            let mut data = TrackedInputRead::new(data, false);
            writer
                .add_entry(&self.meta, Some(&mut data))
                .map_err(|error| {
                    if ctl.is_cancelled() {
                        FormatError::Cancelled
                    } else {
                        error
                    }
                })?;
            data.finish(None, self.meta.size)?;
            input.validate_after_read(&file)
        } else {
            input.validate_non_file()?;
            writer.add_entry(&self.meta, None)?;
            input.validate_non_file()
        }
    }
}

impl PreparedUpdateAdditions for PreparedAdditions {
    fn len(&self) -> usize {
        self.entries.len()
    }

    fn meta(&self, index: usize) -> Option<&EntryMeta> {
        self.entries.get(index).map(|entry| &entry.meta)
    }

    fn add_entry(
        &mut self,
        index: usize,
        writer: &mut dyn ArchiveWriter,
        progress: &dyn ProgressSink,
        ctl: &ControlToken,
        completed_bytes: u64,
        total_bytes: u64,
    ) -> Result<(), FormatError> {
        let entry = self.entries.get_mut(index).ok_or_else(|| {
            FormatError::Io(io::Error::new(
                io::ErrorKind::InvalidInput,
                "prepared update entry index is out of range",
            ))
        })?;
        if entry.consumed {
            return Err(FormatError::Io(io::Error::new(
                io::ErrorKind::InvalidInput,
                "prepared update entry was already consumed",
            )));
        }
        entry.add_to(writer, progress, ctl, completed_bytes, total_bytes)?;
        entry.consumed = true;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::io::Read;
    use std::path::Path;
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::api::{
        ArchiveFormat, ArchiveReader, FormatCapabilities, FormatRegistry, NoProgress, OpenOptions,
        ReadSeek, WriteSeek,
    };
    use crate::Engine;

    struct ShortReadWriter;
    struct LegacyUpdateFormat;

    #[derive(Default)]
    struct ScanProgress {
        events: Mutex<Vec<(u64, String)>>,
        cancel_at: Option<u64>,
        ctl: Option<Arc<ControlToken>>,
    }

    impl ScanProgress {
        fn cancelling_at(entries: u64, ctl: Arc<ControlToken>) -> Self {
            Self {
                events: Mutex::default(),
                cancel_at: Some(entries),
                ctl: Some(ctl),
            }
        }

        fn events(&self) -> Vec<(u64, String)> {
            self.events.lock().unwrap().clone()
        }
    }

    impl ProgressSink for ScanProgress {
        fn on_progress(&self, _done: u64, _total: u64, _current: &EntryPath) {}

        fn on_scan_progress(&self, entries: u64, current: &EntryPath) {
            self.events
                .lock()
                .unwrap()
                .push((entries, current.display.clone()));
            if self.cancel_at == Some(entries) {
                if let Some(ctl) = &self.ctl {
                    ctl.cancel();
                }
            }
        }
    }

    impl ArchiveFormat for LegacyUpdateFormat {
        fn id(&self) -> &'static str {
            "legacy-update"
        }

        fn extensions(&self) -> &'static [&'static str] {
            &["legacy"]
        }

        fn capabilities(&self) -> FormatCapabilities {
            FormatCapabilities {
                can_update: true,
                ..FormatCapabilities::default()
            }
        }

        fn sniff(&self, _head: &[u8], _tail: &[u8]) -> bool {
            false
        }

        fn open(
            &self,
            _source: Box<dyn ReadSeek>,
            _options: &OpenOptions,
        ) -> Result<Box<dyn ArchiveReader>, FormatError> {
            Err(FormatError::Unsupported("legacy test open".into()))
        }

        fn create(
            &self,
            _output: Box<dyn WriteSeek>,
            _options: &CreateOptions,
        ) -> Result<Box<dyn ArchiveWriter>, FormatError> {
            Err(FormatError::Unsupported("legacy test create".into()))
        }

        fn update(
            &self,
            _source: &Path,
            _operations: &[UpdateOp],
            _options: &CreateOptions,
            _progress: &dyn ProgressSink,
            _control: &ControlToken,
        ) -> Result<(), FormatError> {
            Ok(())
        }
    }

    impl ArchiveWriter for ShortReadWriter {
        fn add_entry(
            &mut self,
            _meta: &EntryMeta,
            data: Option<&mut dyn Read>,
        ) -> Result<(), FormatError> {
            if let Some(data) = data {
                let mut byte = [0u8; 1];
                let _ = data.read(&mut byte)?;
            }
            Ok(())
        }

        fn finish(self: Box<Self>) -> Result<(), FormatError> {
            Ok(())
        }
    }

    #[test]
    fn prepared_update_rejects_a_writer_that_underreads_input() {
        let dir =
            std::env::temp_dir().join(format!("squallz-update-underread-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let source = dir.join("source.bin");
        std::fs::write(&source, b"source payload").unwrap();
        let operations = [UpdateOp::Add {
            src: source,
            dest: EntryPath::from_utf8("source.bin"),
        }];
        let mut additions = prepare_additions(
            &operations,
            &CreateOptions::default(),
            &NoProgress,
            &ControlToken::default(),
        )
        .unwrap();
        let mut writer = ShortReadWriter;

        let error = additions
            .add_entry(
                0,
                &mut writer,
                &NoProgress,
                &ControlToken::default(),
                0,
                b"source payload".len() as u64,
            )
            .unwrap_err();

        assert!(matches!(
            error,
            FormatError::Io(ref error) if error.kind() == io::ErrorKind::UnexpectedEof
        ));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn prepared_update_filters_an_excluded_add_before_reading_the_source() {
        let operations = [UpdateOp::Add {
            src: Path::new("ignored.tmp").to_path_buf(),
            dest: EntryPath::from_utf8("ignored.tmp"),
        }];
        let options = CreateOptions {
            excludes: vec!["*.tmp".to_owned()],
            ..CreateOptions::default()
        };

        let progress = ScanProgress::default();
        let additions =
            prepare_additions(&operations, &options, &progress, &ControlToken::default()).unwrap();

        assert!(additions.is_empty());
        assert!(progress.events().is_empty());
    }

    #[test]
    fn prepared_update_reports_kept_entries_across_operations() {
        let dir = std::env::temp_dir().join(format!(
            "squallz-update-scan-progress-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let source = dir.join("source");
        std::fs::create_dir_all(source.join(".cache")).unwrap();
        std::fs::create_dir_all(source.join("sub")).unwrap();
        std::fs::write(source.join(".cache/hidden.txt"), b"hidden").unwrap();
        std::fs::write(source.join("a.txt"), b"a").unwrap();
        std::fs::write(source.join("scratch.tmp"), b"skip").unwrap();
        std::fs::write(source.join("sub/b.txt"), b"b").unwrap();
        let tail = dir.join("tail.bin");
        std::fs::write(&tail, b"tail").unwrap();
        let operations = [
            UpdateOp::Add {
                src: source,
                dest: EntryPath::from_utf8("bundle"),
            },
            UpdateOp::Add {
                src: tail,
                dest: EntryPath::from_utf8("tail.bin"),
            },
            UpdateOp::AddDir {
                path: EntryPath::from_utf8("empty"),
            },
        ];
        let options = CreateOptions {
            excludes: vec![".cache".to_owned(), "*.tmp".to_owned()],
            ..CreateOptions::default()
        };
        let progress = ScanProgress::default();

        let additions =
            prepare_additions(&operations, &options, &progress, &ControlToken::default()).unwrap();

        assert_eq!(additions.len(), 6);
        assert_eq!(
            progress.events(),
            vec![
                (1, "bundle".to_owned()),
                (2, "bundle/a.txt".to_owned()),
                (3, "bundle/sub".to_owned()),
                (4, "bundle/sub/b.txt".to_owned()),
                (5, "tail.bin".to_owned()),
                (6, "empty/".to_owned()),
            ]
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn prepared_update_scan_cancels_after_the_reported_entry() {
        let dir =
            std::env::temp_dir().join(format!("squallz-update-scan-cancel-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let source = dir.join("source.bin");
        std::fs::write(&source, b"source").unwrap();
        let operations = [UpdateOp::Add {
            src: source,
            dest: EntryPath::from_utf8("source.bin"),
        }];
        let ctl = ControlToken::new();
        let progress = ScanProgress::cancelling_at(1, Arc::clone(&ctl));

        let error = match prepare_additions(&operations, &CreateOptions::default(), &progress, &ctl)
        {
            Ok(_) => panic!("scan cancellation must stop input preparation"),
            Err(error) => error,
        };

        assert!(matches!(error, FormatError::Cancelled));
        assert_eq!(progress.events(), vec![(1, "source.bin".to_owned())]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn legacy_update_formats_do_not_trigger_a_core_input_scan() {
        let mut registry = FormatRegistry::new();
        registry.register_archive(Arc::new(LegacyUpdateFormat));
        let engine = Engine::new(registry);
        let operations = [UpdateOp::Add {
            src: Path::new("missing-source.bin").to_path_buf(),
            dest: EntryPath::from_utf8("missing-source.bin"),
        }];
        let progress = ScanProgress::default();

        engine
            .update(
                Path::new("archive.legacy"),
                &operations,
                &CreateOptions::default(),
                &progress,
                &ControlToken::default(),
            )
            .unwrap();
        assert!(progress.events().is_empty());
    }
}
