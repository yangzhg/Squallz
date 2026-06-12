//! 7Z write side: LZMA2 compression, optional AES-256 content encryption
//! and header (file name) encryption. Symlinks/hardlinks are not stored yet
//! (planned with the I4 update work).

use std::io::Read;

use sevenz_rust2::encoder_options::{AesEncoderOptions, Lzma2Options};
use sevenz_rust2::{ArchiveEntry, EncoderConfiguration, EncoderMethod};
use squallz_format_api::{
    ArchiveWriter, CompressionLevel, CreateOptions, EntryMeta, EntryType, FormatError, WriteSeek,
};

use super::{map_7z_error, FILE_ATTRIBUTE_UNIX_EXTENSION};

/// Write handle for a 7z archive being created.
pub(super) struct SevenZArchiveWriter {
    inner: sevenz_rust2::ArchiveWriter<Box<dyn WriteSeek>>,
}

/// LZMA2 preset mapping 0–9 (docs/level-mapping.md); `Store` uses the COPY
/// method instead.
fn lzma2_preset(level: CompressionLevel) -> u32 {
    match level {
        CompressionLevel::Store | CompressionLevel::Fastest => 1,
        CompressionLevel::Fast => 3,
        CompressionLevel::Normal => 6,
        CompressionLevel::Maximum => 8,
        CompressionLevel::Ultra => 9,
    }
}

impl SevenZArchiveWriter {
    pub(super) fn new(dst: Box<dyn WriteSeek>, opts: &CreateOptions) -> Result<Self, FormatError> {
        let mut inner = sevenz_rust2::ArchiveWriter::new(dst).map_err(map_7z_error)?;
        let mut methods: Vec<EncoderConfiguration> = Vec::new();
        if let Some(password) = &opts.password {
            methods.push(AesEncoderOptions::new(password.expose().into()).into());
        }
        let compression = match opts.level {
            CompressionLevel::Store => EncoderConfiguration::new(EncoderMethod::COPY),
            level => Lzma2Options::from_level(lzma2_preset(level)).into(),
        };
        methods.push(compression);
        inner.set_content_methods(methods);
        // Header (file name) encryption only on request; it takes effect
        // only when an AES method is configured.
        inner.set_encrypt_header(opts.password.is_some() && opts.encrypt_filenames);
        Ok(Self { inner })
    }
}

fn unsupported_link(
    kind: &str,
    meta: &EntryMeta,
    target: &[u8],
    suggested_format: &str,
) -> FormatError {
    let target = String::from_utf8_lossy(target);
    FormatError::Unsupported(format!(
        "7z writer cannot store {kind} '{}' -> '{}'; choose {suggested_format} to preserve links",
        meta.path, target
    ))
}

fn unsupported_other(meta: &EntryMeta) -> FormatError {
    FormatError::Unsupported(format!(
        "7z writer cannot store special filesystem entry '{}'; choose tar for special entries",
        meta.path
    ))
}

impl ArchiveWriter for SevenZArchiveWriter {
    fn add_entry(
        &mut self,
        meta: &EntryMeta,
        data: Option<&mut dyn Read>,
    ) -> Result<(), FormatError> {
        let mut entry = match &meta.entry_type {
            EntryType::Dir => ArchiveEntry::new_directory(&meta.path.display),
            EntryType::File => ArchiveEntry::new_file(&meta.path.display),
            EntryType::Symlink { target } => {
                return Err(unsupported_link(
                    "symbolic link",
                    meta,
                    target,
                    "tar or zip",
                ))
            }
            EntryType::Hardlink { target } => {
                return Err(unsupported_link("hard link", meta, target, "tar"))
            }
            EntryType::Other => return Err(unsupported_other(meta)),
        };
        if let Some(modified) = meta.modified {
            if let Ok(date) = sevenz_rust2::NtTime::try_from(modified) {
                entry.last_modified_date = date;
                entry.has_last_modified_date = true;
            }
        }
        if let Some(mode) = meta.unix_mode {
            // p7zip convention: Unix mode in the high attribute bits.
            entry.has_windows_attributes = true;
            entry.windows_attributes = FILE_ATTRIBUTE_UNIX_EXTENSION | ((mode & 0o7777) << 16);
        }
        match &meta.entry_type {
            EntryType::File => {
                let data = data.ok_or_else(|| {
                    FormatError::Other(format!("file entry without data: {}", meta.path))
                })?;
                self.inner
                    .push_archive_entry(entry, Some(data))
                    .map_err(map_7z_error)?;
            }
            _ => {
                self.inner
                    .push_archive_entry(entry, None::<&mut dyn Read>)
                    .map_err(map_7z_error)?;
            }
        }
        Ok(())
    }

    fn finish(self: Box<Self>) -> Result<(), FormatError> {
        self.inner.finish().map(drop)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use squallz_format_api::EntryPath;

    use super::*;

    fn test_entry(path: &str, entry_type: EntryType) -> EntryMeta {
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

    fn memory_writer() -> SevenZArchiveWriter {
        SevenZArchiveWriter::new(Box::new(Cursor::new(Vec::new())), &CreateOptions::default())
            .expect("in-memory 7z writer should initialize")
    }

    #[test]
    fn lzma2_presets_match_documented_levels() {
        assert_eq!(lzma2_preset(CompressionLevel::Store), 1);
        assert_eq!(lzma2_preset(CompressionLevel::Fastest), 1);
        assert_eq!(lzma2_preset(CompressionLevel::Fast), 3);
        assert_eq!(lzma2_preset(CompressionLevel::Normal), 6);
        assert_eq!(lzma2_preset(CompressionLevel::Maximum), 8);
        assert_eq!(lzma2_preset(CompressionLevel::Ultra), 9);
    }

    #[test]
    fn links_and_special_entries_report_actionable_unsupported_errors() {
        let mut writer = memory_writer();
        let symlink = test_entry(
            "link",
            EntryType::Symlink {
                target: b"target.txt".to_vec(),
            },
        );
        let err = writer.add_entry(&symlink, None).unwrap_err();
        match err {
            FormatError::Unsupported(message) => {
                assert!(message.contains("symbolic link"));
                assert!(message.contains("link"));
                assert!(message.contains("target.txt"));
                assert!(message.contains("tar or zip"));
            }
            other => panic!("unexpected error: {other:?}"),
        }

        let mut writer = memory_writer();
        let hardlink = test_entry(
            "hard",
            EntryType::Hardlink {
                target: b"source.bin".to_vec(),
            },
        );
        let err = writer.add_entry(&hardlink, None).unwrap_err();
        match err {
            FormatError::Unsupported(message) => {
                assert!(message.contains("hard link"));
                assert!(message.contains("source.bin"));
                assert!(message.contains("tar"));
            }
            other => panic!("unexpected error: {other:?}"),
        }

        let err = unsupported_other(&test_entry("device", EntryType::Other));
        match err {
            FormatError::Unsupported(message) => {
                assert!(message.contains("special filesystem entry"));
                assert!(message.contains("device"));
                assert!(message.contains("tar"));
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn file_entry_without_data_is_reported_before_finish() {
        let mut writer = memory_writer();
        let file = test_entry("payload.bin", EntryType::File);
        let err = writer.add_entry(&file, None).unwrap_err();
        match err {
            FormatError::Other(message) => {
                assert!(message.contains("file entry without data"));
                assert!(message.contains("payload.bin"));
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }
}
