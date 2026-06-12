//! ZIP format (backed by the `zip` crate): list/extract/create/test,
//! AES-256 read/write, ZipCrypto read-only, ZIP64, legacy entry-name
//! encodings.
//!
//! Extraction deliberately uses the default [`ArchiveReader::extract`]
//! implementation (the shared safe extraction engine in
//! squallz-format-api).

mod datetime;
mod encoding;
mod error;
mod reader;
mod update;
mod writer;

use std::path::Path;

use squallz_format_api::{
    ArchiveFormat, ArchiveReader, ArchiveWriter, ControlToken, CreateOptions, FormatCapabilities,
    FormatError, OpenOptions, ProgressSink, ReadSeek, UpdateOp, WriteSeek,
};

/// End-of-central-directory signature (`PK\x05\x06`).
const EOCD_MAGIC: [u8; 4] = [0x50, 0x4B, 0x05, 0x06];
/// Local-file-header signature (`PK\x03\x04`).
const LOCAL_MAGIC: [u8; 4] = [0x50, 0x4B, 0x03, 0x04];

/// The ZIP archive format.
pub(crate) struct ZipFormat;

impl ArchiveFormat for ZipFormat {
    fn id(&self) -> &'static str {
        "zip"
    }

    fn extensions(&self) -> &'static [&'static str] {
        // JAR/APK/CBZ/IPA are plain ZIP containers (PLAN.md §4 aliases).
        &["zip", "jar", "apk", "cbz", "ipa"]
    }

    fn capabilities(&self) -> FormatCapabilities {
        FormatCapabilities {
            can_create: true,
            can_extract: true,
            can_encrypt_data: true,
            can_encrypt_names: false, // the ZIP format cannot encrypt names
            can_split: true,          // engine-side `.001` byte splitting
            can_update: true,
            can_test: true,
        }
    }

    fn sniff(&self, head: &[u8], tail: &[u8]) -> bool {
        // Plain ZIP starts with a local header; an empty ZIP starts with the
        // EOCD record directly.
        if head.starts_with(&LOCAL_MAGIC) || head.starts_with(&EOCD_MAGIC) {
            return true;
        }
        // SFX archives start with an MZ executable stub but still end with
        // the EOCD record, so also scan the tail window.
        tail.windows(EOCD_MAGIC.len()).any(|w| w == EOCD_MAGIC)
    }

    fn open(
        &self,
        src: Box<dyn ReadSeek>,
        opts: &OpenOptions,
    ) -> Result<Box<dyn ArchiveReader>, FormatError> {
        reader::open(src, opts)
    }

    fn create(
        &self,
        dst: Box<dyn WriteSeek>,
        opts: &CreateOptions,
    ) -> Result<Box<dyn ArchiveWriter>, FormatError> {
        Ok(Box::new(writer::ZipArchiveWriter::new(dst, opts)))
    }

    fn update(
        &self,
        src: &Path,
        ops: &[UpdateOp],
        opts: &CreateOptions,
        progress: &dyn ProgressSink,
        ctl: &ControlToken,
    ) -> Result<(), FormatError> {
        update::update_archive(src, ops, opts, progress, ctl)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zip_format_declares_aliases_and_capabilities() {
        let format = ZipFormat;

        assert_eq!(format.id(), "zip");
        assert_eq!(format.extensions(), &["zip", "jar", "apk", "cbz", "ipa"]);

        let capabilities = format.capabilities();
        assert!(capabilities.can_create);
        assert!(capabilities.can_extract);
        assert!(capabilities.can_encrypt_data);
        assert!(!capabilities.can_encrypt_names);
        assert!(capabilities.can_split);
        assert!(capabilities.can_update);
        assert!(capabilities.can_test);
    }

    #[test]
    fn zip_sniffer_accepts_plain_empty_and_sfx_archives() {
        let format = ZipFormat;

        assert!(format.sniff(&LOCAL_MAGIC, &[]));
        assert!(format.sniff(&EOCD_MAGIC, &[]));

        let sfx_tail = b"stub bytes before PK\x05\x06 and after";
        assert!(format.sniff(b"MZ executable stub", sfx_tail));
    }

    #[test]
    fn zip_sniffer_rejects_non_zip_and_partial_signatures() {
        let format = ZipFormat;

        assert!(!format.sniff(b"not a zip", b"still not a zip"));
        assert!(!format.sniff(b"PK\x03", b"PK\x05"));
    }
}
