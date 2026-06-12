//! Unified error model of the format layer.

/// Unified format-layer error. The CLI maps variants to exit codes, the GUI
/// to friendly messages.
///
/// i18n note (PLAN.md §5.5): the `Display` text here is **log-only** English.
/// User-facing presentation layers (CLI/GUI) must map error variants to
/// language-pack keys and render the structured payload (paths, format ids)
/// themselves.
#[derive(Debug, thiserror::Error)]
pub enum FormatError {
    /// I/O error
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// Operation not supported by this format
    #[error("unsupported operation: {0}")]
    Unsupported(String),
    /// Corrupt archive
    #[error("corrupt archive: {0}")]
    CorruptArchive(String),
    /// A password is required
    #[error("password required")]
    PasswordRequired,
    /// Wrong password
    #[error("wrong password")]
    WrongPassword,
    /// Cancelled by the user
    #[error("operation cancelled")]
    Cancelled,
    /// Zip Slip path traversal
    #[error("path traversal entry detected (zip slip): {0}")]
    PathTraversal(String),
    /// Symlink breakout write
    #[error("symlink breakout write detected: {0}")]
    SymlinkBreakout(String),
    /// Decompression-bomb guardrail exceeded
    #[error("resource limit exceeded: {0}")]
    ResourceLimitExceeded(String),
    /// Unsafe file name (reserved name / illegal characters / ADS)
    #[error("unsafe file name: {0}")]
    UnsafeFileName(String),
    /// Disk full
    #[error("disk full")]
    DiskFull,
    /// Missing external dependency
    #[error("missing external dependency: {0}")]
    DependencyMissing(String),
    /// Anything else
    #[error("{0}")]
    Other(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_keeps_structured_variant_context() {
        assert_eq!(
            FormatError::PathTraversal("../secret.txt".to_string()).to_string(),
            "path traversal entry detected (zip slip): ../secret.txt"
        );
        assert_eq!(
            FormatError::DependencyMissing("7zz".to_string()).to_string(),
            "missing external dependency: 7zz"
        );
    }
}
