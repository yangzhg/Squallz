//! 7Z read side: entry listing, single-entry reads, single-pass extraction
//! and integrity testing. Solid blocks force sequential decoding, so
//! extraction and testing stream every entry exactly once through
//! `for_each_entries`; `read_entry` (preview path) decodes up to the
//! requested file.

use std::collections::HashSet;
use std::io::{Cursor, Read};
use std::path::Path;
use std::time::SystemTime;

use sevenz_rust2::ArchiveEntry;
use squallz_format_api::{
    ArchiveReader, ControlToken, EntryMeta, EntryPath, EntryType, ExtractOptions, ExtractSink,
    FormatError, OpenOptions, ProgressSink, ReadSeek, TestReport,
};

use super::{map_7z_error, FILE_ATTRIBUTE_UNIX_EXTENSION};

/// Chunk size when draining entry data (test pass).
const READ_CHUNK: usize = 64 * 1024;

/// Read handle over a 7z archive.
pub(super) struct SevenZArchiveReader {
    inner: sevenz_rust2::ArchiveReader<Box<dyn ReadSeek>>,
    /// Whether any block in the archive is AES-encrypted (the format flags
    /// encryption per block, not per file).
    encrypted: bool,
}

impl SevenZArchiveReader {
    pub(super) fn open(src: Box<dyn ReadSeek>, opts: &OpenOptions) -> Result<Self, FormatError> {
        let password = open_password(opts);
        // Opening a header-encrypted archive without a password surfaces
        // PasswordRequired here.
        let inner = sevenz_rust2::ArchiveReader::new(src, password).map_err(map_7z_error)?;
        let encrypted = inner.archive().blocks.iter().any(|block| {
            block
                .coders
                .iter()
                .any(|c| c.encoder_method_id() == sevenz_rust2::EncoderMethod::ID_AES256_SHA256)
        });
        Ok(Self { inner, encrypted })
    }
}

fn open_password(opts: &OpenOptions) -> sevenz_rust2::Password {
    match opts.password.as_ref() {
        Some(password) => sevenz_rust2::Password::from(password.expose()),
        None => sevenz_rust2::Password::empty(),
    }
}

/// Builds the [`EntryMeta`] of one 7z entry (names are UTF-8 strings in the
/// 7z model, decoded from UTF-16 by the backend).
fn meta_of(entry: &ArchiveEntry, encrypted: bool) -> EntryMeta {
    let entry_type = if entry.is_directory() {
        EntryType::Dir
    } else {
        EntryType::File
    };
    // p7zip stores Unix permissions in the high attribute bits.
    let attributes = entry.windows_attributes();
    let unix_mode = (entry.has_windows_attributes
        && attributes & FILE_ATTRIBUTE_UNIX_EXTENSION != 0)
        .then_some((attributes >> 16) & 0o7777);
    EntryMeta {
        path: EntryPath::from_utf8(entry.name()),
        entry_type,
        size: entry.size(),
        compressed_size: Some(entry.compressed_size),
        modified: entry
            .has_last_modified_date
            .then(|| SystemTime::from(entry.last_modified_date())),
        unix_mode,
        crc32: entry.has_crc.then_some(entry.crc as u32),
        encrypted: encrypted && entry.has_stream(),
    }
}

impl ArchiveReader for SevenZArchiveReader {
    fn entries(&mut self) -> Box<dyn Iterator<Item = Result<EntryMeta, FormatError>> + '_> {
        let encrypted = self.encrypted;
        Box::new(
            self.inner
                .archive()
                .files
                .iter()
                .map(move |e| Ok(meta_of(e, encrypted))),
        )
    }

    fn read_entry(&mut self, path: &EntryPath) -> Result<Box<dyn Read + '_>, FormatError> {
        // The backend's random-access read decodes the containing block up
        // to the requested file and returns it fully decoded (preview-sized
        // usage; extraction streams instead).
        let data = self.inner.read_file(&path.display).map_err(map_7z_error)?;
        Ok(Box::new(Cursor::new(data)))
    }

    /// Single-pass extraction through the shared safety engine, streaming
    /// every entry in block order (the only efficient order for solid
    /// archives).
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
        let encrypted = self.encrypted;
        let total: u64 = self
            .inner
            .archive()
            .files
            .iter()
            .filter(|e| {
                !e.is_directory()
                    && wanted
                        .as_ref()
                        .is_none_or(|w| w.contains(e.name().as_bytes()))
            })
            .map(|e| e.size())
            .sum();
        let mut sink = ExtractSink::new(dest, opts, total)?;
        let mut failure: Option<FormatError> = None;
        self.inner
            .for_each_entries(|entry, reader| {
                let meta = meta_of(entry, encrypted);
                if let Some(w) = &wanted {
                    if !w.contains(meta.path.raw.as_slice()) {
                        return Ok(true);
                    }
                }
                let result = match meta.entry_type {
                    EntryType::File => {
                        sink.file_target(&meta, progress, ctl)
                            .and_then(|t| match t {
                                Some(out_path) => {
                                    sink.write_file(&meta, &out_path, reader, progress, ctl)
                                }
                                None => Ok(()),
                            })
                    }
                    _ => sink.write_meta_entry(&meta, progress, ctl),
                };
                match result {
                    Ok(()) => Ok(true),
                    Err(e) => {
                        failure = Some(e);
                        Ok(false) // stop iterating; the real error is kept
                    }
                }
            })
            .map_err(map_7z_error)?;
        if let Some(e) = failure {
            return Err(e);
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
        let total: u64 = self.inner.archive().files.iter().map(|e| e.size()).sum();
        let encrypted = self.encrypted;
        let mut done = 0u64;
        let mut cancelled = false;
        self.inner
            .for_each_entries(|entry, reader| {
                if ctl.checkpoint().is_err() {
                    cancelled = true;
                    return Ok(false);
                }
                report.entries_tested += 1;
                let meta = meta_of(entry, encrypted);
                let mut hasher = crc32fast::Hasher::new();
                let mut buf = vec![0u8; READ_CHUNK];
                loop {
                    if ctl.checkpoint().is_err() {
                        cancelled = true;
                        return Ok(false);
                    }
                    match reader.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            hasher.update(&buf[..n]);
                            done += n as u64;
                            progress.on_progress(done, total, &meta.path);
                        }
                        Err(e) => {
                            report.problems.push(format!("{}: {e}", meta.path));
                            break;
                        }
                    }
                }
                if entry.has_crc && u64::from(hasher.finalize()) != entry.crc {
                    report.problems.push(format!("{}: CRC mismatch", meta.path));
                }
                Ok(true)
            })
            .map_err(|e| {
                // Reading garbage with a wrong password usually dies inside
                // the decoder; surface it as such.
                map_7z_error(e)
            })?;
        if cancelled {
            return Err(FormatError::Cancelled);
        }
        progress.on_progress(total, total, &EntryPath::from_utf8(""));
        Ok(report)
    }
}
