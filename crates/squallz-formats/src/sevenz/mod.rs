//! 7Z format (backed by sevenz-rust2): list/extract/test with AES-256
//! decryption (including header-encrypted archives), creation with LZMA2
//! and optional AES-256 content + header (file name) encryption.

mod reader;
mod writer;

use squallz_format_api::{
    ArchiveFormat, ArchiveReader, ArchiveWriter, CreateOptions, FormatCapabilities, FormatError,
    OpenOptions, ReadSeek, WriteSeek,
};

/// 7z signature: `7z¼¯'\x1c`.
const MAGIC: [u8; 6] = [0x37, 0x7A, 0xBC, 0xAF, 0x27, 0x1C];

/// p7zip convention: this Windows attribute bit flags Unix metadata stored
/// in the high 16 bits of the attribute word.
const FILE_ATTRIBUTE_UNIX_EXTENSION: u32 = 0x8000;

/// The 7Z archive format.
pub(crate) struct SevenZFormat;

impl ArchiveFormat for SevenZFormat {
    fn id(&self) -> &'static str {
        "7z"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["7z"]
    }

    fn capabilities(&self) -> FormatCapabilities {
        FormatCapabilities {
            can_create: true,
            can_extract: true,
            can_encrypt_data: true,
            // 7z encrypts the header, hiding file names.
            can_encrypt_names: true,
            can_split: true, // engine-side `.001` byte splitting
            can_update: false,
            can_test: true,
        }
    }

    fn sniff(&self, head: &[u8], _tail: &[u8]) -> bool {
        head.starts_with(&MAGIC)
    }

    fn open(
        &self,
        src: Box<dyn ReadSeek>,
        opts: &OpenOptions,
    ) -> Result<Box<dyn ArchiveReader>, FormatError> {
        Ok(Box::new(reader::SevenZArchiveReader::open(src, opts)?))
    }

    fn create(
        &self,
        dst: Box<dyn WriteSeek>,
        opts: &CreateOptions,
    ) -> Result<Box<dyn ArchiveWriter>, FormatError> {
        Ok(Box::new(writer::SevenZArchiveWriter::new(dst, opts)?))
    }
}

/// Maps sevenz-rust2 errors onto the unified error model.
pub(super) fn map_7z_error(e: sevenz_rust2::Error) -> FormatError {
    use sevenz_rust2::Error as E;
    match e {
        E::PasswordRequired => FormatError::PasswordRequired,
        E::MaybeBadPassword(_) => FormatError::WrongPassword,
        E::Io(e, _) | E::FileOpen(e, _) => FormatError::Io(e),
        E::UnsupportedCompressionMethod(m) => {
            FormatError::Unsupported(format!("7z compression method: {m}"))
        }
        E::Unsupported(s) => FormatError::Unsupported(s.into_owned()),
        E::ChecksumVerificationFailed | E::NextHeaderCrcMismatch => {
            FormatError::CorruptArchive("7z checksum verification failed".into())
        }
        E::BadSignature(_) | E::UnsupportedVersion { .. } => {
            FormatError::CorruptArchive("not a valid 7z archive".into())
        }
        other => FormatError::CorruptArchive(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;
    use std::io;

    use super::*;

    #[test]
    fn sevenz_format_declares_encrypted_archive_capabilities() {
        let format = SevenZFormat;

        assert_eq!(format.id(), "7z");
        assert_eq!(format.extensions(), &["7z"]);

        let capabilities = format.capabilities();
        assert!(capabilities.can_create);
        assert!(capabilities.can_extract);
        assert!(capabilities.can_encrypt_data);
        assert!(capabilities.can_encrypt_names);
        assert!(capabilities.can_split);
        assert!(!capabilities.can_update);
        assert!(capabilities.can_test);
    }

    #[test]
    fn sevenz_sniffer_accepts_signature_at_archive_start_only() {
        let format = SevenZFormat;

        assert!(format.sniff(&MAGIC, b"ignored tail"));

        let mut with_more_header = MAGIC.to_vec();
        with_more_header.extend_from_slice(b"\0\x04rest");
        assert!(format.sniff(&with_more_header, b""));

        assert!(!format.sniff(&MAGIC[..MAGIC.len() - 1], b""));

        let mut wrong_magic = MAGIC;
        wrong_magic[0] = 0;
        assert!(!format.sniff(&wrong_magic, b""));
    }

    #[test]
    fn sevenz_error_mapping_keeps_password_and_corruption_distinct() {
        assert!(matches!(
            map_7z_error(sevenz_rust2::Error::PasswordRequired),
            FormatError::PasswordRequired
        ));

        let wrong_password = sevenz_rust2::Error::MaybeBadPassword(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "bad password",
        ));
        assert!(matches!(
            map_7z_error(wrong_password),
            FormatError::WrongPassword
        ));

        match map_7z_error(sevenz_rust2::Error::ChecksumVerificationFailed) {
            FormatError::CorruptArchive(message) => {
                assert_eq!(message, "7z checksum verification failed");
            }
            other => panic!("expected checksum corruption, got {other:?}"),
        }

        match map_7z_error(sevenz_rust2::Error::BadSignature(MAGIC)) {
            FormatError::CorruptArchive(message) => {
                assert_eq!(message, "not a valid 7z archive");
            }
            other => panic!("expected bad-signature corruption, got {other:?}"),
        }
    }

    #[test]
    fn sevenz_error_mapping_keeps_io_and_unsupported_actionable() {
        let io_error = sevenz_rust2::Error::Io(
            io::Error::new(io::ErrorKind::UnexpectedEof, "truncated"),
            Cow::Borrowed("header"),
        );
        match map_7z_error(io_error) {
            FormatError::Io(error) => assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof),
            other => panic!("expected io error, got {other:?}"),
        }

        match map_7z_error(sevenz_rust2::Error::UnsupportedCompressionMethod(
            "BCJ2".to_string(),
        )) {
            FormatError::Unsupported(message) => {
                assert_eq!(message, "7z compression method: BCJ2");
            }
            other => panic!("expected unsupported method, got {other:?}"),
        }

        match map_7z_error(sevenz_rust2::Error::Unsupported(Cow::Borrowed(
            "encrypted headers disabled",
        ))) {
            FormatError::Unsupported(message) => {
                assert_eq!(message, "encrypted headers disabled");
            }
            other => panic!("expected unsupported detail, got {other:?}"),
        }
    }
}
