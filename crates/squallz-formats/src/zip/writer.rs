//! ZIP write side: streaming creation with deflate levels, AES-256
//! encryption, ZIP64 large files, directories, symlinks and Unix
//! permissions.

use std::io::{self, Read, Seek, SeekFrom, Write};

use squallz_format_api::{
    ArchiveWriter, CompressionLevel, ControlToken, CreateOptions, EntryMeta, EntryType,
    FormatError, Password, WriteSeek,
};
use zip::write::SimpleFileOptions;
use zip::{AesMode, CompressionMethod, ZipWriter, ZIP64_BYTES_THR};

use super::datetime::to_zip_datetime;
use super::error::map_zip_error;

const WRITE_CHUNK: usize = 64 * 1024;
const CANCELLED_IO_MESSAGE: &str = "ZIP creation cancelled";

/// Write handle for a ZIP archive being created.
pub(super) struct ZipArchiveWriter {
    inner: ZipWriter<Box<dyn WriteSeek>>,
    level: CompressionLevel,
    password: Option<Password>,
    control: ControlToken,
}

impl ZipArchiveWriter {
    /// Raw-copies an entry (opened with `by_index_raw`) from another
    /// archive: compressed data and encryption are carried over verbatim,
    /// optionally under a new name. Used by the update path.
    pub(super) fn raw_copy<R: Read>(
        &mut self,
        file: zip::read::ZipFile<'_, R>,
        rename_to: Option<&str>,
    ) -> Result<(), FormatError> {
        let result = match rename_to {
            Some(name) => self.inner.raw_copy_file_rename(file, name),
            None => self.inner.raw_copy_file(file),
        };
        controlled_zip_result(result, &self.control)
    }

    pub(super) fn new(dst: Box<dyn WriteSeek>, opts: &CreateOptions) -> Self {
        Self::new_with_control(dst, opts, &ControlToken::default())
    }

    pub(super) fn new_with_control(
        dst: Box<dyn WriteSeek>,
        opts: &CreateOptions,
        control: &ControlToken,
    ) -> Self {
        Self {
            inner: ZipWriter::new(Box::new(ControlledWriteSeek::new(dst, control))),
            level: opts.level,
            password: opts.password.clone(),
            control: control.clone(),
        }
    }

    /// Base options shared by every entry kind.
    fn base_options(&self, meta: &EntryMeta) -> SimpleFileOptions {
        let (method, level) = zip_compression_method_and_level(self.level);
        let mut options = SimpleFileOptions::default()
            .compression_method(method)
            .compression_level(level)
            // ZIP64 for entries at or above the 4 GiB headroom threshold.
            .large_file(meta.size >= ZIP64_BYTES_THR);
        if let Some(mode) = meta.unix_mode {
            options = options.unix_permissions(zip_unix_permissions(mode));
        }
        if let Some(dt) = meta.modified.and_then(to_zip_datetime) {
            options = options.last_modified_time(dt);
        }
        options
    }

    fn copy_entry_data(&mut self, data: &mut dyn Read) -> Result<(), FormatError> {
        let mut buffer = vec![0u8; WRITE_CHUNK];
        loop {
            self.control.checkpoint()?;
            let read = data
                .read(&mut buffer)
                .map_err(|error| map_controlled_io_error(error, &self.control))?;
            if read == 0 {
                break;
            }
            self.inner
                .write_all(&buffer[..read])
                .map_err(|error| map_controlled_io_error(error, &self.control))?;
            self.control.checkpoint()?;
        }
        Ok(())
    }
}

fn zip_compression_method_and_level(level: CompressionLevel) -> (CompressionMethod, Option<i64>) {
    match level {
        CompressionLevel::Store => (CompressionMethod::Stored, None),
        // Deflate level mapping (documented in docs/level-mapping.md):
        // Fastest=1, Fast=3, Normal=6, Maximum=8, Ultra=9.
        CompressionLevel::Fastest => (CompressionMethod::Deflated, Some(1)),
        CompressionLevel::Fast => (CompressionMethod::Deflated, Some(3)),
        CompressionLevel::Normal => (CompressionMethod::Deflated, Some(6)),
        CompressionLevel::Maximum => (CompressionMethod::Deflated, Some(8)),
        CompressionLevel::Ultra => (CompressionMethod::Deflated, Some(9)),
    }
}

fn zip_unix_permissions(mode: u32) -> u32 {
    mode & 0o777
}

impl ArchiveWriter for ZipArchiveWriter {
    fn add_entry(
        &mut self,
        meta: &EntryMeta,
        data: Option<&mut dyn Read>,
    ) -> Result<(), FormatError> {
        // Entries we create are always named in UTF-8 (raw == display).
        let name = meta.path.display.clone();
        let options = self.base_options(meta);
        match &meta.entry_type {
            EntryType::Dir => {
                // No point encrypting zero-byte directory markers; some
                // tools choke on encrypted directory entries.
                let result = self.inner.add_directory(name, options);
                controlled_zip_result(result, &self.control)
            }
            EntryType::Symlink { target } => {
                let target = String::from_utf8_lossy(target).into_owned();
                let result = match &self.password {
                    Some(pw) => self.inner.add_symlink(
                        name,
                        target,
                        options.with_aes_encryption(AesMode::Aes256, pw.expose()),
                    ),
                    None => self.inner.add_symlink(name, target, options),
                };
                controlled_zip_result(result, &self.control)
            }
            EntryType::File => {
                let result = match &self.password {
                    Some(pw) => self.inner.start_file(
                        name,
                        options.with_aes_encryption(AesMode::Aes256, pw.expose()),
                    ),
                    None => self.inner.start_file(name, options),
                };
                controlled_zip_result(result, &self.control)?;
                if let Some(data) = data {
                    self.copy_entry_data(data)?;
                }
                Ok(())
            }
            EntryType::Hardlink { .. } | EntryType::Other => Err(FormatError::Unsupported(
                format!("zip writer cannot store entry type of '{}'", meta.path),
            )),
        }
    }

    fn finish(self: Box<Self>) -> Result<(), FormatError> {
        let Self { inner, control, .. } = *self;
        control.checkpoint()?;
        inner
            .finish()
            .map(drop)
            .map_err(|error| map_controlled_zip_error(error, &control))?;
        control.checkpoint()
    }
}

struct ControlledWriteSeek {
    inner: Box<dyn WriteSeek>,
    control: ControlToken,
    discarding: bool,
    position: u64,
    end: u64,
}

impl ControlledWriteSeek {
    fn new(inner: Box<dyn WriteSeek>, control: &ControlToken) -> Self {
        Self {
            inner,
            control: control.clone(),
            discarding: false,
            position: 0,
            end: 0,
        }
    }

    fn should_discard(&mut self) -> io::Result<bool> {
        if self.discarding {
            return Ok(true);
        }
        match self.control.checkpoint() {
            Ok(()) => Ok(false),
            Err(FormatError::Cancelled) => {
                let position = self.inner.stream_position()?;
                let end = self.inner.seek(SeekFrom::End(0))?;
                self.inner.seek(SeekFrom::Start(position))?;
                self.position = position;
                self.end = end;
                self.discarding = true;
                Ok(true)
            }
            Err(error) => Err(io::Error::other(error)),
        }
    }

    fn discard_write(&mut self, len: usize) -> io::Result<usize> {
        let len = u64::try_from(len).map_err(|_| io::Error::other("ZIP write is too large"))?;
        self.position = self
            .position
            .checked_add(len)
            .ok_or_else(|| io::Error::other("ZIP output position overflow"))?;
        self.end = self.end.max(self.position);
        usize::try_from(len).map_err(|_| io::Error::other("ZIP write is too large"))
    }

    fn discard_seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        let next = match position {
            SeekFrom::Start(position) => i128::from(position),
            SeekFrom::End(offset) => i128::from(self.end) + i128::from(offset),
            SeekFrom::Current(offset) => i128::from(self.position) + i128::from(offset),
        };
        self.position = u64::try_from(next)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, CANCELLED_IO_MESSAGE))?;
        Ok(self.position)
    }
}

impl Write for ControlledWriteSeek {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        if self.should_discard()? {
            return self.discard_write(buf.len());
        }
        self.inner.write(&buf[..buf.len().min(WRITE_CHUNK)])
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.should_discard()? {
            return Ok(());
        }
        self.inner.flush()
    }
}

impl Seek for ControlledWriteSeek {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        if self.should_discard()? {
            return self.discard_seek(position);
        }
        self.inner.seek(position)
    }
}

fn controlled_zip_result<T>(
    result: Result<T, zip::result::ZipError>,
    control: &ControlToken,
) -> Result<T, FormatError> {
    let value = result.map_err(|error| map_controlled_zip_error(error, control))?;
    control.checkpoint()?;
    Ok(value)
}

fn map_controlled_zip_error(error: zip::result::ZipError, control: &ControlToken) -> FormatError {
    if control.is_cancelled() {
        FormatError::Cancelled
    } else {
        map_zip_error(error)
    }
}

fn map_controlled_io_error(error: io::Error, control: &ControlToken) -> FormatError {
    if control.is_cancelled() {
        FormatError::Cancelled
    } else {
        error.into()
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;

    use squallz_format_api::{EntryPath, WriteSeek};

    use super::*;

    const LEVELS: [CompressionLevel; 6] = [
        CompressionLevel::Store,
        CompressionLevel::Fastest,
        CompressionLevel::Fast,
        CompressionLevel::Normal,
        CompressionLevel::Maximum,
        CompressionLevel::Ultra,
    ];

    fn meta(path: &str, entry_type: EntryType) -> EntryMeta {
        EntryMeta {
            path: EntryPath::from_utf8(path),
            entry_type,
            size: 0,
            compressed_size: None,
            modified: None,
            unix_mode: None,
            crc32: None,
            encrypted: false,
        }
    }

    fn memory_writer() -> ZipArchiveWriter {
        let dst: Box<dyn WriteSeek> = Box::new(Cursor::new(Vec::<u8>::new()));
        ZipArchiveWriter::new(dst, &CreateOptions::default())
    }

    struct CancelOnArmedWrite {
        inner: Cursor<Vec<u8>>,
        control: ControlToken,
        armed: Arc<AtomicBool>,
        armed_writes: Arc<AtomicUsize>,
        largest_write: Arc<AtomicUsize>,
    }

    impl Write for CancelOnArmedWrite {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.largest_write.fetch_max(buf.len(), Ordering::SeqCst);
            if self.armed.load(Ordering::SeqCst) {
                let index = self.armed_writes.fetch_add(1, Ordering::SeqCst);
                if index == 0 {
                    self.control.cancel();
                }
            }
            self.inner.write(buf)
        }

        fn flush(&mut self) -> io::Result<()> {
            self.inner.flush()
        }
    }

    impl Seek for CancelOnArmedWrite {
        fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
            self.inner.seek(position)
        }
    }

    #[test]
    fn zip_compression_levels_match_documented_deflate_mapping() {
        let actual = LEVELS.map(zip_compression_method_and_level);
        assert_eq!(
            actual,
            [
                (CompressionMethod::Stored, None),
                (CompressionMethod::Deflated, Some(1)),
                (CompressionMethod::Deflated, Some(3)),
                (CompressionMethod::Deflated, Some(6)),
                (CompressionMethod::Deflated, Some(8)),
                (CompressionMethod::Deflated, Some(9)),
            ]
        );
    }

    #[test]
    fn zip_unix_permissions_drop_file_type_bits() {
        assert_eq!(zip_unix_permissions(0o100755), 0o755);
        assert_eq!(zip_unix_permissions(0o120777), 0o777);
        assert_eq!(zip_unix_permissions(0o040700), 0o700);
    }

    #[test]
    fn unsupported_entry_types_report_the_entry_path() {
        let mut writer = memory_writer();
        let hardlink = meta(
            "links/hard",
            EntryType::Hardlink {
                target: b"target.txt".to_vec(),
            },
        );
        let err = writer
            .add_entry(&hardlink, None)
            .expect_err("hardlinks are not storable in ZIP writer");
        assert!(
            matches!(err, FormatError::Unsupported(ref message) if message.contains("links/hard")),
            "expected unsupported hardlink error with entry path, got {err:?}"
        );

        let mut writer = memory_writer();
        let other = meta("special/device", EntryType::Other);
        let err = writer
            .add_entry(&other, None)
            .expect_err("special entries are not storable in ZIP writer");
        assert!(
            matches!(err, FormatError::Unsupported(ref message) if message.contains("special/device")),
            "expected unsupported special-entry error with entry path, got {err:?}"
        );
    }

    #[test]
    fn final_directory_write_is_chunked_and_cancellable() {
        let control = ControlToken::default();
        let armed = Arc::new(AtomicBool::new(false));
        let armed_writes = Arc::new(AtomicUsize::new(0));
        let largest_write = Arc::new(AtomicUsize::new(0));
        let output = CancelOnArmedWrite {
            inner: Cursor::new(Vec::new()),
            control: control.clone(),
            armed: Arc::clone(&armed),
            armed_writes: Arc::clone(&armed_writes),
            largest_write: Arc::clone(&largest_write),
        };
        let mut writer = ZipArchiveWriter::new_with_control(
            Box::new(output),
            &CreateOptions::default(),
            &control,
        );
        for index in 0..50_000 {
            writer
                .add_entry(
                    &meta(&format!("directories/{index:04}/"), EntryType::Dir),
                    None,
                )
                .unwrap_or_else(|error| panic!("add ZIP directory {index}: {error}"));
        }

        armed.store(true, Ordering::SeqCst);
        let error = Box::new(writer)
            .finish()
            .expect_err("cancellation must interrupt ZIP central-directory output");

        assert!(matches!(error, FormatError::Cancelled));
        assert_eq!(
            armed_writes.load(Ordering::SeqCst),
            1,
            "no output write may reach the inner sink after cancellation"
        );
        assert!(
            largest_write.load(Ordering::SeqCst) <= WRITE_CHUNK,
            "ZIP output writes must stay within the control chunk"
        );
    }
}
