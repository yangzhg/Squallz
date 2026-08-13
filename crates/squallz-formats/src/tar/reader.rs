//! TAR read side. The `tar` crate consumes its reader while iterating, so
//! every pass (entries/read_entry/extract/test) rebuilds the archive: a
//! seekable source is rewound, a streamed source (`.tar.gz`) is re-created
//! through the engine-provided [`StreamFactory`].

use std::collections::HashSet;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::time::{Duration, SystemTime};

use squallz_format_api::{
    empty_extract_report, ArchiveReader, BoundedProblemLog, ControlToken, EntryMeta, EntryPath,
    EntryType, ExtractOptions, ExtractReport, ExtractSink, FormatError, LimitsAccountant,
    ProgressSink, ReadSeek, SafetyLimits, StreamFactory, TestSummary, TEST_PROBLEM_PREVIEW_LIMIT,
};

/// Chunk size when draining entry data and compound stream tails.
const READ_CHUNK: usize = 64 * 1024;
/// Maximum decompressed data accepted after TAR's logical end marker. This
/// budget is deliberately separate from extracted-file limits: the tail is
/// read only to finish the compound decoder's integrity checks.
const MAX_STREAM_END_BYTES: u64 = 16 * 1024 * 1024;

/// Unified tar input: both variants can produce a fresh stream positioned
/// at the start of the (decompressed) tar data.
enum TarInput {
    Seekable(Box<dyn ReadSeek>),
    Streamed(Box<dyn Read + Send>),
}

impl Read for TarInput {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            TarInput::Seekable(r) => r.read(buf),
            TarInput::Streamed(r) => r.read(buf),
        }
    }
}

/// Read handle over a tar archive (plain or inside a compressed stream).
pub(super) struct TarArchiveReader {
    /// Rebuilt at the start of every pass; `None` only transiently.
    archive: Option<tar::Archive<TarInput>>,
    /// Present for streamed sources; re-creates the decompressed stream.
    factory: Option<StreamFactory>,
}

impl TarArchiveReader {
    pub(super) fn seekable(src: Box<dyn ReadSeek>) -> Self {
        Self {
            archive: Some(tar::Archive::new(TarInput::Seekable(src))),
            factory: None,
        }
    }

    pub(super) fn streaming(factory: StreamFactory) -> Self {
        Self {
            archive: None,
            factory: Some(factory),
        }
    }

    /// Whether the source can be rewound cheaply (no re-decompression).
    fn is_seekable(&self) -> bool {
        self.factory.is_none()
    }

    /// Restarts the tar stream and returns a fresh archive over it.
    fn rebuild(&mut self) -> Result<&mut tar::Archive<TarInput>, FormatError> {
        let input = match (self.archive.take(), &self.factory) {
            (_, Some(factory)) => TarInput::Streamed(factory()?),
            (Some(archive), None) => match archive.into_inner() {
                TarInput::Seekable(mut src) => {
                    src.seek(SeekFrom::Start(0))?;
                    TarInput::Seekable(src)
                }
                // Unreachable by construction (no factory ⇒ seekable), but
                // degrade gracefully rather than panic.
                streamed @ TarInput::Streamed(_) => streamed,
            },
            (None, None) => {
                return Err(FormatError::Other(
                    "tar reader lost its source stream".into(),
                ))
            }
        };
        Ok(self.archive.insert(tar::Archive::new(input)))
    }

    /// Sums the file sizes for progress totals (cheap pre-pass for seekable
    /// sources only; a streamed source would pay a full re-decompression).
    fn total_file_bytes(&mut self, wanted: Option<&HashSet<Vec<u8>>>) -> Result<u64, FormatError> {
        let mut total = 0u64;
        for meta in self.entries() {
            let meta = meta?;
            if matches!(meta.entry_type, EntryType::File)
                && wanted.is_none_or(|paths| paths.contains(meta.path.raw.as_slice()))
            {
                total += meta.size;
            }
        }
        Ok(total)
    }

    /// TAR iteration stops at the archive's zero blocks, before a compound
    /// decoder necessarily reaches its trailer. Drain streamed inputs so the
    /// compressor can validate its final checksum and size fields.
    fn validate_stream_end(&mut self, ctl: &ControlToken) -> Result<(), FormatError> {
        if self.is_seekable() {
            return Ok(());
        }
        let Some(archive) = self.archive.take() else {
            return Err(FormatError::Other(
                "tar reader lost its source stream".into(),
            ));
        };
        let mut input = archive.into_inner();
        let result = match &mut input {
            TarInput::Streamed(reader) => {
                let mut buf = vec![0u8; READ_CHUNK];
                let mut drained = 0u64;
                loop {
                    if let Err(error) = ctl.checkpoint() {
                        break Err(error);
                    }
                    let remaining = MAX_STREAM_END_BYTES.saturating_sub(drained);
                    let read_len = (remaining.saturating_add(1) as usize).min(buf.len());
                    match reader.read(&mut buf[..read_len]) {
                        Ok(0) => break Ok(()),
                        Ok(n) => {
                            drained = drained.saturating_add(n as u64);
                            if drained > MAX_STREAM_END_BYTES {
                                break Err(FormatError::ResourceLimitExceeded(format!(
                                    "compound stream tail exceeds fixed safety limit of {MAX_STREAM_END_BYTES} bytes"
                                )));
                            }
                        }
                        Err(error) => break Err(error.into()),
                    }
                }
            }
            TarInput::Seekable(_) => Err(FormatError::Other(
                "tar streamed reader lost its decoder".into(),
            )),
        };
        self.archive = Some(tar::Archive::new(input));
        result
    }

    fn test_with_problem_recorder(
        &mut self,
        progress: &dyn ProgressSink,
        ctl: &ControlToken,
        mut record_problem: impl FnMut(String),
    ) -> Result<u64, FormatError> {
        let streamed = !self.is_seekable();
        let archive = self.rebuild()?;
        let mut buf = vec![0u8; READ_CHUNK];
        let mut done = 0u64;
        let mut entries_tested = 0u64;
        let mut accountant = LimitsAccountant::new(SafetyLimits::default());
        for item in archive.entries()? {
            ctl.checkpoint()?;
            let mut entry = match item {
                Ok(entry) => entry,
                Err(e) => {
                    // A broken header desynchronizes the stream, so later
                    // bytes cannot be treated as independent entries.
                    record_problem(e.to_string());
                    break;
                }
            };
            let meta = match meta_of(&entry) {
                Ok(meta) => meta,
                Err(e) => {
                    // Preserve the test report contract: an entry whose
                    // metadata is malformed was still encountered and tested.
                    entries_tested += 1;
                    record_problem(e.to_string());
                    continue;
                }
            };
            if is_tar_root_directory(&meta) {
                continue;
            }
            entries_tested += 1;
            let path = meta.path;
            // Draining validates entry framing and, for compound inputs, the
            // underlying stream's integrity checks.
            loop {
                ctl.checkpoint()?;
                match entry.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if streamed {
                            accountant.add_output_bytes(n as u64)?;
                        }
                        done += n as u64;
                        progress.on_progress(done, 0, &path);
                    }
                    Err(e) => {
                        record_problem(format!("{path}: {e}"));
                        break;
                    }
                }
            }
        }
        match self.validate_stream_end(ctl) {
            Ok(()) => {}
            Err(error @ (FormatError::Cancelled | FormatError::ResourceLimitExceeded(_))) => {
                return Err(error)
            }
            Err(error) => record_problem(error.to_string()),
        }
        progress.on_progress(done, done, &EntryPath::from_utf8(""));
        Ok(entries_tested)
    }
}

/// Builds the [`EntryMeta`] of one tar entry.
fn meta_of<R: Read>(entry: &tar::Entry<'_, R>) -> Result<EntryMeta, FormatError> {
    let raw = entry.path_bytes().into_owned();
    let display = String::from_utf8_lossy(&raw).into_owned();
    let header = entry.header();
    let link_target = |kind: &str| -> Result<Vec<u8>, FormatError> {
        let Some(target) = entry.link_name_bytes() else {
            return Err(FormatError::CorruptArchive(format!(
                "tar {kind} entry missing target: {display}"
            )));
        };
        Ok(target.into_owned())
    };
    let entry_type = match header.entry_type() {
        tar::EntryType::Directory => EntryType::Dir,
        tar::EntryType::Symlink => EntryType::Symlink {
            target: link_target("symlink")?,
        },
        tar::EntryType::Link => EntryType::Hardlink {
            target: link_target("hardlink")?,
        },
        tar::EntryType::Regular | tar::EntryType::Continuous | tar::EntryType::GNUSparse => {
            EntryType::File
        }
        _ => EntryType::Other,
    };
    Ok(EntryMeta {
        path: EntryPath::from_raw(raw, display, "utf-8"),
        entry_type,
        size: entry.size(),
        compressed_size: None,
        modified: header
            .mtime()
            .ok()
            .map(|secs| SystemTime::UNIX_EPOCH + Duration::from_secs(secs)),
        unix_mode: header.mode().ok(),
        crc32: None,
        encrypted: false,
    })
}

fn is_tar_root_directory(meta: &EntryMeta) -> bool {
    matches!(&meta.entry_type, EntryType::Dir)
        && (meta.path.raw.as_slice() == b"." || meta.path.raw.as_slice() == b"./")
}

impl ArchiveReader for TarArchiveReader {
    fn entries(&mut self) -> Box<dyn Iterator<Item = Result<EntryMeta, FormatError>> + '_> {
        let archive = match self.rebuild() {
            Ok(a) => a,
            Err(e) => return Box::new(std::iter::once(Err(e))),
        };
        match archive.entries() {
            Ok(entries) => Box::new(entries.filter_map(|item| {
                match item
                    .map_err(FormatError::from)
                    .and_then(|entry| meta_of(&entry))
                {
                    Ok(meta) if is_tar_root_directory(&meta) => None,
                    result => Some(result),
                }
            })),
            Err(e) => Box::new(std::iter::once(Err(e.into()))),
        }
    }

    fn read_entry(&mut self, path: &EntryPath) -> Result<Box<dyn Read + '_>, FormatError> {
        let archive = self.rebuild()?;
        for item in archive.entries()? {
            let entry = item?;
            if entry.path_bytes().as_ref() == path.raw.as_slice() {
                return Ok(Box::new(entry));
            }
        }
        Err(FormatError::Other(format!("entry not found: {path}")))
    }

    /// Single-pass extraction through the shared safety engine. The default
    /// entries+read_entry flow would restart the stream once per file
    /// (quadratic for `.tar.gz`); here every entry is streamed in archive
    /// order instead.
    fn extract(
        &mut self,
        dest: &Path,
        selection: Option<&[EntryPath]>,
        opts: &ExtractOptions,
        progress: &dyn ProgressSink,
        ctl: &ControlToken,
    ) -> Result<(), FormatError> {
        self.extract_with_report(dest, selection, opts, progress, ctl)
            .map(drop)
    }

    fn extract_with_report(
        &mut self,
        dest: &Path,
        selection: Option<&[EntryPath]>,
        opts: &ExtractOptions,
        progress: &dyn ProgressSink,
        ctl: &ControlToken,
    ) -> Result<ExtractReport, FormatError> {
        if selection.is_some_and(<[EntryPath]>::is_empty) {
            return Ok(empty_extract_report(dest, progress));
        }
        let wanted: Option<HashSet<Vec<u8>>> =
            selection.map(|s| s.iter().map(|p| p.raw.clone()).collect());
        // Progress total: cheap metadata pre-pass for seekable sources;
        // streamed sources report an unknown total (0) instead of paying a
        // second full decompression.
        let total = if self.is_seekable() {
            self.total_file_bytes(wanted.as_ref())?
        } else {
            0
        };
        let mut sink = ExtractSink::new(dest, opts, total)?;
        let archive = self.rebuild()?;
        for item in archive.entries()? {
            let mut entry = item?;
            let meta = meta_of(&entry)?;
            if is_tar_root_directory(&meta) {
                continue;
            }
            if let Some(w) = &wanted {
                if !w.contains(meta.path.raw.as_slice()) {
                    continue;
                }
            }
            match meta.entry_type {
                EntryType::File => {
                    if let Some(out_path) = sink.file_target(&meta, progress, ctl)? {
                        if opts.best_effort {
                            sink.write_file_best_effort(
                                &meta, &out_path, &mut entry, progress, ctl,
                            )?;
                        } else {
                            sink.write_file(&meta, &out_path, &mut entry, progress, ctl)?;
                        }
                    }
                }
                _ => sink.write_meta_entry(&meta, progress, ctl)?,
            }
        }
        self.validate_stream_end(ctl)?;
        Ok(sink.finish_with_report(progress))
    }

    fn test_summary(
        &mut self,
        progress: &dyn ProgressSink,
        ctl: &ControlToken,
    ) -> Result<TestSummary, FormatError> {
        let problems = BoundedProblemLog::new(TEST_PROBLEM_PREVIEW_LIMIT);
        let entries_tested =
            self.test_with_problem_recorder(progress, ctl, |problem| problems.record(problem))?;
        Ok(TestSummary {
            entries_tested,
            problems: problems.snapshot(),
            recovery: None,
        })
    }
}
