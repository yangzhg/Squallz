//! Error presentation layer: FormatError variant → language-pack key
//! mapping and exit codes (documented in docs/exit-codes.md).

use squallz_core::api::FormatError;

pub use squallz_i18n::localize_error;

/// A command failure: either a structured engine error (localized at the
/// edge in `main`) or an exit code whose message has already been printed by
/// the command itself (e.g. a failed integrity test report).
pub enum CliError {
    /// Engine error, localized and printed by `main`.
    Format(FormatError),
    /// Message already emitted; only the exit code remains.
    Exit(i32),
}

impl From<FormatError> for CliError {
    fn from(e: FormatError) -> Self {
        Self::Format(e)
    }
}

/// FormatError → exit-code mapping (see docs/exit-codes.md).
pub fn exit_code(e: &FormatError) -> i32 {
    match e {
        FormatError::Unsupported(_) => 2,
        FormatError::CorruptArchive(_) => 3,
        FormatError::PasswordRequired | FormatError::WrongPassword => 4,
        FormatError::Cancelled => 5,
        FormatError::PathTraversal(_)
        | FormatError::SymlinkBreakout(_)
        | FormatError::ResourceLimitExceeded(_)
        | FormatError::UnsafeFileName(_) => 6,
        FormatError::Io(_) | FormatError::DiskFull => 7,
        FormatError::DependencyMissing(_) => 8,
        FormatError::Other(_) => 1,
    }
}

pub fn error_kind(e: &FormatError) -> &'static str {
    match e {
        FormatError::Unsupported(_) => "unsupported",
        FormatError::CorruptArchive(_) => "corrupt_archive",
        FormatError::PasswordRequired => "password_required",
        FormatError::WrongPassword => "wrong_password",
        FormatError::Cancelled => "cancelled",
        FormatError::PathTraversal(_) => "path_traversal",
        FormatError::SymlinkBreakout(_) => "symlink_breakout",
        FormatError::ResourceLimitExceeded(_) => "resource_limit_exceeded",
        FormatError::UnsafeFileName(_) => "unsafe_file_name",
        FormatError::Io(_) => "io",
        FormatError::DiskFull => "disk_full",
        FormatError::DependencyMissing(_) => "dependency_missing",
        FormatError::Other(_) => "other",
    }
}

// The FormatError → language-pack-key mapping lives in squallz-i18n
// (`squallz_i18n::error_message` / `localize_error`, re-exported above) so
// the GUI shares it instead of duplicating the match.

#[cfg(test)]
mod tests {
    use std::io;

    use super::*;

    fn error_cases() -> Vec<(FormatError, &'static str, i32)> {
        vec![
            (
                FormatError::Unsupported("feature".to_string()),
                "unsupported",
                2,
            ),
            (
                FormatError::CorruptArchive("bad central directory".to_string()),
                "corrupt_archive",
                3,
            ),
            (FormatError::PasswordRequired, "password_required", 4),
            (FormatError::WrongPassword, "wrong_password", 4),
            (FormatError::Cancelled, "cancelled", 5),
            (
                FormatError::PathTraversal("../secret".to_string()),
                "path_traversal",
                6,
            ),
            (
                FormatError::SymlinkBreakout("link/outside".to_string()),
                "symlink_breakout",
                6,
            ),
            (
                FormatError::ResourceLimitExceeded("max output".to_string()),
                "resource_limit_exceeded",
                6,
            ),
            (
                FormatError::UnsafeFileName("CON.txt".to_string()),
                "unsafe_file_name",
                6,
            ),
            (
                FormatError::Io(io::Error::new(io::ErrorKind::NotFound, "missing")),
                "io",
                7,
            ),
            (FormatError::DiskFull, "disk_full", 7),
            (
                FormatError::DependencyMissing("7zz".to_string()),
                "dependency_missing",
                8,
            ),
            (FormatError::Other("misc".to_string()), "other", 1),
        ]
    }

    #[test]
    fn exit_code_matches_documented_format_error_mapping() {
        for (error, _kind, code) in error_cases() {
            assert_eq!(exit_code(&error), code, "{error:?}");
        }
    }

    #[test]
    fn error_kind_matches_json_error_contract() {
        for (error, kind, _code) in error_cases() {
            assert_eq!(error_kind(&error), kind, "{error:?}");
        }
    }

    #[test]
    fn cli_error_from_format_preserves_structured_error() {
        let err = CliError::from(FormatError::DependencyMissing("par2".to_string()));
        match err {
            CliError::Format(FormatError::DependencyMissing(tool)) => assert_eq!(tool, "par2"),
            _ => panic!("expected dependency-missing format error"),
        }
    }
}
