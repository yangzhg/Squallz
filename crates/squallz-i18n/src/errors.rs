//! Shared FormatError → language-pack key mapping (PLAN.md §5.5).
//!
//! Both presentation layers consume this: the CLI renders the key through a
//! [`crate::Localizer`] right away, the GUI ships the structured
//! `{key, params}` pair over IPC and lets the frontend render it.

use squallz_format_api::FormatError;

/// Structured, language-independent description of an engine error: an
/// `error.*` language-pack key plus its `{placeholder}` arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorMessage {
    /// Language-pack key (e.g. `"error.corrupt_archive"`)
    pub key: &'static str,
    /// Placeholder name → value pairs
    pub params: Vec<(&'static str, String)>,
}

/// Maps a [`FormatError`] variant to its language-pack key and placeholder
/// values. The variants' own `Display` text is log-only English and is never
/// shown directly.
pub fn error_message(e: &FormatError) -> ErrorMessage {
    let (key, params) = match e {
        FormatError::Io(err) => ("error.io", vec![("detail", err.to_string())]),
        FormatError::Unsupported(d) => ("error.unsupported", vec![("detail", d.clone())]),
        FormatError::CorruptArchive(d) => ("error.corrupt_archive", vec![("detail", d.clone())]),
        FormatError::PasswordRequired => ("error.password_required", vec![]),
        FormatError::WrongPassword => ("error.wrong_password", vec![]),
        FormatError::Cancelled => ("error.cancelled", vec![]),
        FormatError::PathTraversal(p) => ("error.path_traversal", vec![("path", p.clone())]),
        FormatError::SymlinkBreakout(p) => ("error.symlink_breakout", vec![("path", p.clone())]),
        FormatError::ResourceLimitExceeded(d) => {
            ("error.resource_limit", vec![("detail", d.clone())])
        }
        FormatError::UnsafeFileName(n) => ("error.unsafe_filename", vec![("name", n.clone())]),
        FormatError::DiskFull => ("error.disk_full", vec![]),
        FormatError::DependencyMissing(n) => {
            ("error.dependency_missing", vec![("name", n.clone())])
        }
        FormatError::Other(d) => ("error.other", vec![("detail", d.clone())]),
    };
    ErrorMessage { key, params }
}

/// Renders a [`FormatError`] through the given localizer (CLI convenience).
pub fn localize_error(loc: &crate::Localizer, e: &FormatError) -> String {
    let msg = error_message(e);
    let args: Vec<(&str, &str)> = msg.params.iter().map(|(k, v)| (*k, v.as_str())).collect();
    loc.format(msg.key, &args)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Localizer;

    #[test]
    fn every_variant_maps_to_a_known_key() {
        let errors = [
            FormatError::Io(std::io::Error::other("x")),
            FormatError::Unsupported("u".into()),
            FormatError::CorruptArchive("c".into()),
            FormatError::PasswordRequired,
            FormatError::WrongPassword,
            FormatError::Cancelled,
            FormatError::PathTraversal("p".into()),
            FormatError::SymlinkBreakout("s".into()),
            FormatError::ResourceLimitExceeded("r".into()),
            FormatError::UnsafeFileName("n".into()),
            FormatError::DiskFull,
            FormatError::DependencyMissing("d".into()),
            FormatError::Other("o".into()),
        ];
        let loc = Localizer::with_user_dir(Some("en-US"), None);
        for e in &errors {
            let msg = error_message(e);
            // Every mapped key must exist in the built-in packs: a rendered
            // message never equals the bare key.
            assert_ne!(localize_error(&loc, e), msg.key, "missing key {}", msg.key);
        }
    }

    #[test]
    fn params_are_substituted() {
        let loc = Localizer::with_user_dir(Some("zh-CN"), None);
        let rendered = localize_error(&loc, &FormatError::UnsafeFileName("CON".into()));
        assert_eq!(rendered, "不安全的文件名：CON");
    }
}
