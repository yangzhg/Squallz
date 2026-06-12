//! Mapping from `zip::result::ZipError` to the unified [`FormatError`].

use squallz_format_api::FormatError;
use zip::result::ZipError;

/// Maps a `zip` crate error to the structured format-layer error.
///
/// Distinguishes "a password is needed" (the crate reports it as
/// `UnsupportedArchive(PASSWORD_REQUIRED)`) from "the password is wrong"
/// (`InvalidPassword`).
pub(super) fn map_zip_error(e: ZipError) -> FormatError {
    match e {
        ZipError::Io(io) => FormatError::Io(io),
        ZipError::InvalidArchive(msg) => FormatError::CorruptArchive(msg.into_owned()),
        ZipError::UnsupportedArchive(msg) if msg == ZipError::PASSWORD_REQUIRED => {
            FormatError::PasswordRequired
        }
        ZipError::UnsupportedArchive(msg) => FormatError::Unsupported(msg.to_string()),
        ZipError::FileNotFound => FormatError::Other("entry not found in archive".to_string()),
        ZipError::InvalidPassword => FormatError::WrongPassword,
        ZipError::CompressionMethodNotSupported(method) => {
            FormatError::Unsupported(format!("zip compression method {method} is not supported"))
        }
        // ZipError is #[non_exhaustive]; treat unknown variants as corruption.
        other => FormatError::CorruptArchive(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use std::io;

    use super::*;

    #[test]
    fn invalid_archive_and_io_keep_structured_error_kinds() {
        match map_zip_error(ZipError::InvalidArchive("bad central directory".into())) {
            FormatError::CorruptArchive(message) => {
                assert_eq!(message, "bad central directory");
            }
            other => panic!("expected CorruptArchive, got {other:?}"),
        }

        match map_zip_error(ZipError::Io(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "permission denied",
        ))) {
            FormatError::Io(err) => assert_eq!(err.kind(), io::ErrorKind::PermissionDenied),
            other => panic!("expected Io, got {other:?}"),
        }
    }

    #[test]
    fn password_errors_remain_distinct() {
        assert!(matches!(
            map_zip_error(ZipError::UnsupportedArchive(ZipError::PASSWORD_REQUIRED)),
            FormatError::PasswordRequired
        ));
        assert!(matches!(
            map_zip_error(ZipError::InvalidPassword),
            FormatError::WrongPassword
        ));
    }

    #[test]
    fn unsupported_and_missing_entry_errors_are_actionable() {
        match map_zip_error(ZipError::UnsupportedArchive("zip64 locator is unsupported")) {
            FormatError::Unsupported(message) => {
                assert_eq!(message, "zip64 locator is unsupported");
            }
            other => panic!("expected Unsupported, got {other:?}"),
        }

        match map_zip_error(ZipError::CompressionMethodNotSupported(99)) {
            FormatError::Unsupported(message) => {
                assert_eq!(message, "zip compression method 99 is not supported");
            }
            other => panic!("expected Unsupported, got {other:?}"),
        }

        match map_zip_error(ZipError::FileNotFound) {
            FormatError::Other(message) => assert_eq!(message, "entry not found in archive"),
            other => panic!("expected Other, got {other:?}"),
        }
    }
}
