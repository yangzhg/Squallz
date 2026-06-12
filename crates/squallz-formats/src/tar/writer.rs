//! TAR write side: streaming creation with Unix permissions, mtimes,
//! symlinks and hardlinks. Long names/link targets are handled by the `tar`
//! crate through GNU extension entries.

use std::io::{Read, Write};
use std::path::Path;
use std::time::SystemTime;

use squallz_format_api::{ArchiveWriter, EntryMeta, EntryType, FormatError};

/// Default permission bits when the input carries none.
const DEFAULT_FILE_MODE: u32 = 0o644;
const DEFAULT_DIR_MODE: u32 = 0o755;
const DEFAULT_LINK_MODE: u32 = 0o777;

/// Write handle for a tar archive being created. Generic over the sink so
/// the same writer serves seekable files (`x.tar`) and live compression
/// streams (`x.tar.gz`).
pub(super) struct TarArchiveWriter<W: Write + Send> {
    builder: tar::Builder<W>,
}

impl<W: Write + Send> TarArchiveWriter<W> {
    pub(super) fn new(dst: W) -> Self {
        Self {
            builder: tar::Builder::new(dst),
        }
    }

    /// Base header shared by every entry kind.
    fn header(meta: &EntryMeta, default_mode: u32) -> tar::Header {
        let mut header = tar::Header::new_gnu();
        header.set_mode(tar_unix_mode(meta.unix_mode, default_mode));
        header.set_mtime(tar_mtime(meta.modified));
        header.set_size(0);
        header
    }
}

fn tar_unix_mode(mode: Option<u32>, default_mode: u32) -> u32 {
    match mode {
        Some(mode) => mode & 0o7777,
        None => default_mode,
    }
}

fn tar_mtime(modified: Option<SystemTime>) -> u64 {
    match modified.and_then(|time| time.duration_since(SystemTime::UNIX_EPOCH).ok()) {
        Some(duration) => duration.as_secs(),
        None => 0,
    }
}

fn tar_link_target<'a>(meta: &EntryMeta, target: &'a [u8]) -> Result<&'a Path, FormatError> {
    std::str::from_utf8(target)
        .map(Path::new)
        .map_err(|_| FormatError::UnsafeFileName(meta.path.display.clone()))
}

impl<W: Write + Send> ArchiveWriter for TarArchiveWriter<W> {
    fn add_entry(
        &mut self,
        meta: &EntryMeta,
        data: Option<&mut dyn Read>,
    ) -> Result<(), FormatError> {
        // Entries we create are always named in UTF-8 (raw == display).
        let name = Path::new(&meta.path.display);
        match &meta.entry_type {
            EntryType::Dir => {
                let mut header = Self::header(meta, DEFAULT_DIR_MODE);
                header.set_entry_type(tar::EntryType::Directory);
                self.builder
                    .append_data(&mut header, name, std::io::empty())?;
            }
            EntryType::File => {
                let data = data.ok_or_else(|| {
                    FormatError::Other(format!("file entry without data: {}", meta.path))
                })?;
                let mut header = Self::header(meta, DEFAULT_FILE_MODE);
                header.set_entry_type(tar::EntryType::Regular);
                header.set_size(meta.size);
                self.builder.append_data(&mut header, name, data)?;
            }
            EntryType::Symlink { target } => {
                let mut header = Self::header(meta, DEFAULT_LINK_MODE);
                header.set_entry_type(tar::EntryType::Symlink);
                let target = tar_link_target(meta, target)?;
                self.builder.append_link(&mut header, name, target)?;
            }
            EntryType::Hardlink { target } => {
                let mut header = Self::header(meta, DEFAULT_LINK_MODE);
                header.set_entry_type(tar::EntryType::Link);
                let target = tar_link_target(meta, target)?;
                self.builder.append_link(&mut header, name, target)?;
            }
            EntryType::Other => {
                return Err(FormatError::Unsupported(format!(
                    "tar writer cannot store entry type of '{}'",
                    meta.path
                )))
            }
        }
        Ok(())
    }

    fn finish(self: Box<Self>) -> Result<(), FormatError> {
        // `into_inner` writes the two terminating zero blocks.
        let mut inner = self.builder.into_inner()?;
        inner.flush()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::time::Duration;

    use squallz_format_api::{EntryPath, WriteSeek};

    use super::*;

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

    fn memory_writer() -> TarArchiveWriter<Box<dyn WriteSeek>> {
        TarArchiveWriter::new(Box::new(io::Cursor::new(Vec::<u8>::new())))
    }

    #[test]
    fn tar_header_preserves_mode_and_normalizes_pre_epoch_mtime() -> io::Result<()> {
        let mut mode_meta = meta("bin/tool", EntryType::File);
        mode_meta.unix_mode = Some(0o100755);
        mode_meta.modified = Some(SystemTime::UNIX_EPOCH + Duration::from_secs(42));
        let header = TarArchiveWriter::<io::Sink>::header(&mode_meta, DEFAULT_FILE_MODE);
        assert_eq!(header.mode()?, 0o755);
        assert_eq!(header.mtime()?, 42);

        let mut default_meta = meta("old/file", EntryType::File);
        default_meta.modified = Some(SystemTime::UNIX_EPOCH - Duration::from_secs(1));
        let header = TarArchiveWriter::<io::Sink>::header(&default_meta, DEFAULT_FILE_MODE);
        assert_eq!(header.mode()?, DEFAULT_FILE_MODE);
        assert_eq!(header.mtime()?, 0);
        Ok(())
    }

    #[test]
    fn invalid_link_targets_report_the_entry_path() {
        let mut writer = memory_writer();
        let symlink = meta(
            "links/bad-symlink",
            EntryType::Symlink { target: vec![0xff] },
        );
        let err = writer
            .add_entry(&symlink, None)
            .expect_err("invalid symlink targets must be rejected");
        assert!(
            matches!(err, FormatError::UnsafeFileName(ref path) if path == "links/bad-symlink"),
            "expected unsafe filename with symlink path, got {err:?}"
        );

        let mut writer = memory_writer();
        let hardlink = meta(
            "links/bad-hardlink",
            EntryType::Hardlink { target: vec![0xff] },
        );
        let err = writer
            .add_entry(&hardlink, None)
            .expect_err("invalid hardlink targets must be rejected");
        assert!(
            matches!(err, FormatError::UnsafeFileName(ref path) if path == "links/bad-hardlink"),
            "expected unsafe filename with hardlink path, got {err:?}"
        );
    }

    #[test]
    fn missing_file_data_and_unsupported_entries_report_the_entry_path() {
        let mut writer = memory_writer();
        let file = meta("missing/data.bin", EntryType::File);
        let err = writer
            .add_entry(&file, None)
            .expect_err("file entries require data");
        assert!(
            matches!(err, FormatError::Other(ref message) if message.contains("missing/data.bin")),
            "expected missing data error with entry path, got {err:?}"
        );

        let mut writer = memory_writer();
        let other = meta("special/device", EntryType::Other);
        let err = writer
            .add_entry(&other, None)
            .expect_err("special entries are unsupported");
        assert!(
            matches!(err, FormatError::Unsupported(ref message) if message.contains("special/device")),
            "expected unsupported error with entry path, got {err:?}"
        );
    }
}
