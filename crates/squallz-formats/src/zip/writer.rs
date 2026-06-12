//! ZIP write side: streaming creation with deflate levels, AES-256
//! encryption, ZIP64 large files, directories, symlinks and Unix
//! permissions.

use std::io::Read;

use squallz_format_api::{
    ArchiveWriter, CompressionLevel, CreateOptions, EntryMeta, EntryType, FormatError, Password,
    WriteSeek,
};
use zip::write::SimpleFileOptions;
use zip::{AesMode, CompressionMethod, ZipWriter, ZIP64_BYTES_THR};

use super::datetime::to_zip_datetime;
use super::error::map_zip_error;

/// Write handle for a ZIP archive being created.
pub(super) struct ZipArchiveWriter {
    inner: ZipWriter<Box<dyn WriteSeek>>,
    level: CompressionLevel,
    password: Option<Password>,
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
        match rename_to {
            Some(name) => self.inner.raw_copy_file_rename(file, name),
            None => self.inner.raw_copy_file(file),
        }
        .map_err(map_zip_error)
    }

    pub(super) fn new(dst: Box<dyn WriteSeek>, opts: &CreateOptions) -> Self {
        Self {
            inner: ZipWriter::new(dst),
            level: opts.level,
            password: opts.password.clone(),
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
                self.inner
                    .add_directory(name, options)
                    .map_err(map_zip_error)
            }
            EntryType::Symlink { target } => {
                let target = String::from_utf8_lossy(target).into_owned();
                match &self.password {
                    Some(pw) => self
                        .inner
                        .add_symlink(
                            name,
                            target,
                            options.with_aes_encryption(AesMode::Aes256, pw.expose()),
                        )
                        .map_err(map_zip_error),
                    None => self
                        .inner
                        .add_symlink(name, target, options)
                        .map_err(map_zip_error),
                }
            }
            EntryType::File => {
                match &self.password {
                    Some(pw) => self
                        .inner
                        .start_file(
                            name,
                            options.with_aes_encryption(AesMode::Aes256, pw.expose()),
                        )
                        .map_err(map_zip_error)?,
                    None => self
                        .inner
                        .start_file(name, options)
                        .map_err(map_zip_error)?,
                }
                if let Some(data) = data {
                    std::io::copy(data, &mut self.inner)?;
                }
                Ok(())
            }
            EntryType::Hardlink { .. } | EntryType::Other => Err(FormatError::Unsupported(
                format!("zip writer cannot store entry type of '{}'", meta.path),
            )),
        }
    }

    fn finish(self: Box<Self>) -> Result<(), FormatError> {
        self.inner.finish().map(drop).map_err(map_zip_error)
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

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
}
