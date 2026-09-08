//! Unified error model of the format layer.

use std::path::{Path, PathBuf};

const MISSING_VOLUME_PREFIX: &str = "missing volume: ";
const SPLIT_WIM_UNSUPPORTED_DETAIL: &str =
    "Split WIM archives are not supported yet; join the .swm parts into a complete .wim before opening, testing, or extracting";
const SPLIT_WIM_CREATE_UNSUPPORTED_DETAIL: &str =
    "creating .swm requires a split size and the native Split WIM layout";

#[derive(Debug, thiserror::Error)]
#[error("output already exists: {}", path.display())]
struct OutputExistsError {
    path: PathBuf,
}

#[derive(Debug, thiserror::Error)]
#[error("destination changed after overwrite confirmation: {}", path.display())]
struct DestinationChangedError {
    path: PathBuf,
}

#[derive(Debug, thiserror::Error)]
#[error("archive input changed after extraction preflight")]
struct InputChangedError;

/// Unified format-layer error. The CLI maps variants to exit codes, the GUI
/// to friendly messages.
///
/// i18n note: the `Display` text here is **log-only** English.
/// User-facing presentation layers (CLI/GUI) must map error variants to
/// language-pack keys and render the structured payload (paths, format ids)
/// themselves.
#[derive(Debug, thiserror::Error)]
pub enum FormatError {
    /// I/O error
    #[error("I/O error: {0}")]
    Io(#[source] std::io::Error),
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

impl FormatError {
    /// Creates the stable unsupported marker used when a multi-part `.swm`
    /// stream has no source path from which sibling volumes can be discovered.
    pub fn split_wim_unsupported() -> Self {
        Self::Unsupported(SPLIT_WIM_UNSUPPORTED_DETAIL.into())
    }

    /// Returns whether this is the stable source-less Split WIM marker.
    pub fn is_split_wim_unsupported(&self) -> bool {
        matches!(self, Self::Unsupported(detail) if detail == SPLIT_WIM_UNSUPPORTED_DETAIL)
    }

    /// Creates the stable validation marker for a requested `.swm` output
    /// without native Split WIM options.
    pub fn split_wim_creation_unsupported() -> Self {
        Self::Unsupported(SPLIT_WIM_CREATE_UNSUPPORTED_DETAIL.into())
    }

    /// Returns whether this is the stable `.swm` option-validation marker.
    pub fn is_split_wim_creation_unsupported(&self) -> bool {
        matches!(self, Self::Unsupported(detail) if detail == SPLIT_WIM_CREATE_UNSUPPORTED_DETAIL)
    }

    /// Creates a corrupt-archive error that identifies one required split
    /// volume. This is classified as `CorruptArchive` because the requested
    /// archive set is structurally incomplete.
    pub fn missing_volume(path: impl AsRef<Path>) -> Self {
        Self::CorruptArchive(format!(
            "{MISSING_VOLUME_PREFIX}{}",
            path.as_ref().display()
        ))
    }

    /// Returns the missing split-volume path carried by this error.
    pub fn missing_volume_path(&self) -> Option<&Path> {
        let Self::CorruptArchive(detail) = self else {
            return None;
        };
        detail
            .strip_prefix(MISSING_VOLUME_PREFIX)
            .filter(|path| !path.is_empty())
            .map(Path::new)
    }

    /// Creates a contextual I/O error for a destination that must not be
    /// replaced. Presentation layers can distinguish this from unrelated
    /// [`std::io::ErrorKind::AlreadyExists`] failures.
    pub fn output_exists(path: impl Into<PathBuf>) -> Self {
        Self::Io(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            OutputExistsError { path: path.into() },
        ))
    }

    /// Returns the occupied destination carried by an output-conflict error.
    pub fn output_exists_path(&self) -> Option<&Path> {
        let Self::Io(error) = self else {
            return None;
        };
        error
            .get_ref()
            .and_then(|source| source.downcast_ref::<OutputExistsError>())
            .map(|source| source.path.as_path())
    }

    /// Whether this error represents a deliberate no-replace output conflict.
    pub fn is_output_exists(&self) -> bool {
        self.output_exists_path().is_some()
    }

    /// Creates a contextual error for an overwrite authorization whose
    /// destination no longer matches the state the caller confirmed.
    pub fn destination_changed(path: impl Into<PathBuf>) -> Self {
        Self::Io(std::io::Error::other(DestinationChangedError {
            path: path.into(),
        }))
    }

    /// Returns the destination carried by a stale overwrite authorization.
    pub fn destination_changed_path(&self) -> Option<&Path> {
        let Self::Io(error) = self else {
            return None;
        };
        error
            .get_ref()
            .and_then(|source| source.downcast_ref::<DestinationChangedError>())
            .map(|source| source.path.as_path())
    }

    /// Whether this error represents a destination that changed after the
    /// caller confirmed replacement.
    pub fn is_destination_changed(&self) -> bool {
        self.destination_changed_path().is_some()
    }

    /// Creates a contextual error for an archive source or selected scope
    /// that no longer matches the extraction preflight.
    pub fn input_changed() -> Self {
        Self::Io(std::io::Error::other(InputChangedError))
    }

    /// Whether this error represents a stale extraction input preflight.
    pub fn is_input_changed(&self) -> bool {
        let Self::Io(error) = self else {
            return false;
        };
        error
            .get_ref()
            .is_some_and(|source| source.downcast_ref::<InputChangedError>().is_some())
    }
}

impl From<std::io::Error> for FormatError {
    fn from(error: std::io::Error) -> Self {
        if let Some(source) = error
            .get_ref()
            .and_then(|source| source.downcast_ref::<FormatError>())
        {
            match source {
                Self::Cancelled => return Self::Cancelled,
                Self::PasswordRequired => return Self::PasswordRequired,
                Self::WrongPassword => return Self::WrongPassword,
                _ => {}
            }
        }
        match error.kind() {
            std::io::ErrorKind::StorageFull | std::io::ErrorKind::QuotaExceeded => Self::DiskFull,
            _ => Self::Io(error),
        }
    }
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

    #[test]
    fn storage_exhaustion_maps_to_disk_full() {
        for kind in [
            std::io::ErrorKind::StorageFull,
            std::io::ErrorKind::QuotaExceeded,
        ] {
            assert!(matches!(
                FormatError::from(std::io::Error::new(kind, "no space")),
                FormatError::DiskFull
            ));
        }
    }

    #[test]
    fn io_error_preserves_its_source() {
        let error = FormatError::from(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "missing input",
        ));
        let source = std::error::Error::source(&error)
            .and_then(|source| source.downcast_ref::<std::io::Error>());

        assert_eq!(
            source.map(std::io::Error::kind),
            Some(std::io::ErrorKind::NotFound)
        );
    }

    #[test]
    fn stream_control_and_password_failures_recover_their_structured_variant() {
        let cancelled = FormatError::from(std::io::Error::other(FormatError::Cancelled));
        let required = FormatError::from(std::io::Error::other(FormatError::PasswordRequired));
        let wrong = FormatError::from(std::io::Error::other(FormatError::WrongPassword));

        assert!(matches!(cancelled, FormatError::Cancelled));
        assert!(matches!(required, FormatError::PasswordRequired));
        assert!(matches!(wrong, FormatError::WrongPassword));
    }

    #[test]
    fn output_conflicts_are_distinct_from_unrelated_io_collisions() {
        let output = PathBuf::from("archive.repaired.zip");
        let conflict = FormatError::output_exists(output.clone());
        let unrelated = FormatError::Io(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "temporary allocation collision",
        ));

        assert!(matches!(
            &conflict,
            FormatError::Io(error) if error.kind() == std::io::ErrorKind::AlreadyExists
        ));
        assert_eq!(conflict.output_exists_path(), Some(output.as_path()));
        assert!(conflict.is_output_exists());
        assert!(!unrelated.is_output_exists());
    }

    #[test]
    fn destination_changes_are_structured_and_keep_the_confirmed_path() {
        let output = PathBuf::from("archive.zip");
        let changed = FormatError::destination_changed(output.clone());
        let unrelated = FormatError::Io(std::io::Error::other(
            "destination changed while opening a temporary file",
        ));

        assert_eq!(changed.destination_changed_path(), Some(output.as_path()));
        assert!(changed.is_destination_changed());
        assert!(!changed.is_output_exists());
        assert!(!unrelated.is_destination_changed());
    }

    #[test]
    fn extraction_input_changes_are_structured() {
        let changed = FormatError::input_changed();
        let unrelated = FormatError::Io(std::io::Error::other(
            "archive input changed while reading metadata",
        ));

        assert!(changed.is_input_changed());
        assert!(!changed.is_destination_changed());
        assert!(!changed.is_output_exists());
        assert!(!unrelated.is_input_changed());
    }

    #[test]
    fn missing_volumes_remain_corrupt_errors_with_a_borrowed_path() {
        let path = PathBuf::from("downloads/archive.7z.004");
        let missing = FormatError::missing_volume(&path);
        let unrelated =
            FormatError::CorruptArchive("archive mentions missing volume: archive.7z.004".into());

        assert!(matches!(&missing, FormatError::CorruptArchive(_)));
        assert_eq!(missing.missing_volume_path(), Some(path.as_path()));
        assert_eq!(unrelated.missing_volume_path(), None);
    }

    #[test]
    fn split_wim_marker_remains_an_unsupported_error() {
        let error = FormatError::split_wim_unsupported();

        assert!(matches!(&error, FormatError::Unsupported(_)));
        assert!(error.is_split_wim_unsupported());
        assert!(!FormatError::Unsupported("other".into()).is_split_wim_unsupported());
    }

    #[test]
    fn split_wim_creation_marker_remains_an_unsupported_error() {
        let error = FormatError::split_wim_creation_unsupported();

        assert!(matches!(&error, FormatError::Unsupported(_)));
        assert!(error.is_split_wim_creation_unsupported());
        assert!(!FormatError::Unsupported("other".into()).is_split_wim_creation_unsupported());
    }
}
