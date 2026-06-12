//! TAR read side. The `tar` crate consumes its reader while iterating, so
//! every pass (entries/read_entry/extract/test) rebuilds the archive: a
//! seekable source is rewound, a streamed source (`.tar.gz`) is re-created
//! through the engine-provided [`StreamFactory`].

use std::collections::HashSet;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::time::{Duration, SystemTime};

use squallz_format_api::{
    ArchiveReader, ControlToken, EntryMeta, EntryPath, EntryType, ExtractOptions, ExtractSink,
    FormatError, ProgressSink, ReadSeek, StreamFactory, TestReport,
};

/// Chunk size when draining entry data (test pass).
const READ_CHUNK: usize = 64 * 1024;

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
    fn total_file_bytes(&mut self) -> Result<u64, FormatError> {
        let mut total = 0u64;
        for meta in self.entries() {
            let meta = meta?;
            if matches!(meta.entry_type, EntryType::File) {
                total += meta.size;
            }
        }
        Ok(total)
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

impl ArchiveReader for TarArchiveReader {
    fn entries(&mut self) -> Box<dyn Iterator<Item = Result<EntryMeta, FormatError>> + '_> {
        let archive = match self.rebuild() {
            Ok(a) => a,
            Err(e) => return Box::new(std::iter::once(Err(e))),
        };
        match archive.entries() {
            Ok(entries) => Box::new(
                entries.map(|item| item.map_err(FormatError::from).and_then(|e| meta_of(&e))),
            ),
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
        let wanted: Option<HashSet<Vec<u8>>> =
            selection.map(|s| s.iter().map(|p| p.raw.clone()).collect());
        // Progress total: cheap metadata pre-pass for seekable sources;
        // streamed sources report an unknown total (0) instead of paying a
        // second full decompression.
        let total = if self.is_seekable() {
            self.total_file_bytes()?
        } else {
            0
        };
        let mut sink = ExtractSink::new(dest, opts, total)?;
        let archive = self.rebuild()?;
        for item in archive.entries()? {
            let mut entry = item?;
            let meta = meta_of(&entry)?;
            if let Some(w) = &wanted {
                if !w.contains(meta.path.raw.as_slice()) {
                    continue;
                }
            }
            match meta.entry_type {
                EntryType::File => {
                    if let Some(out_path) = sink.file_target(&meta, progress, ctl)? {
                        sink.write_file(&meta, &out_path, &mut entry, progress, ctl)?;
                    }
                }
                _ => sink.write_meta_entry(&meta, progress, ctl)?,
            }
        }
        sink.finish(progress);
        Ok(())
    }

    fn test(
        &mut self,
        progress: &dyn ProgressSink,
        ctl: &ControlToken,
    ) -> Result<TestReport, FormatError> {
        let mut report = TestReport::default();
        let archive = self.rebuild()?;
        let mut buf = vec![0u8; READ_CHUNK];
        let mut done = 0u64;
        for item in archive.entries()? {
            ctl.checkpoint()?;
            let mut entry = match item {
                Ok(entry) => entry,
                Err(e) => {
                    // A broken header desynchronizes the stream; record the
                    // problem and stop instead of misparsing what follows.
                    report.problems.push(e.to_string());
                    break;
                }
            };
            report.entries_tested += 1;
            let path = match meta_of(&entry) {
                Ok(meta) => meta.path,
                Err(e) => {
                    report.problems.push(e.to_string());
                    continue;
                }
            };
            // Draining the data validates entry framing and, for compressed
            // sources, the integrity of the underlying stream.
            loop {
                ctl.checkpoint()?;
                match entry.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        done += n as u64;
                        progress.on_progress(done, 0, &path);
                    }
                    Err(e) => {
                        report.problems.push(format!("{path}: {e}"));
                        break;
                    }
                }
            }
        }
        progress.on_progress(done, done, &EntryPath::from_utf8(""));
        Ok(report)
    }
}
