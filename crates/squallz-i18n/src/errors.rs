//! Shared `FormatError` to language-pack key mapping.
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
    if e.is_split_wim_creation_unsupported() {
        return ErrorMessage {
            key: "error.unsupported_split_wim_create",
            params: vec![],
        };
    }
    if e.is_split_wim_unsupported() {
        return ErrorMessage {
            key: "error.unsupported_split_wim",
            params: vec![],
        };
    }
    if let Some(path) = e.missing_volume_path() {
        let name = match path.file_name() {
            Some(name) => name.to_string_lossy().into_owned(),
            None => path.to_string_lossy().into_owned(),
        };
        return ErrorMessage {
            key: "gui.error.corrupt.volume_missing",
            params: vec![("name", name)],
        };
    }
    let (key, params) = match e {
        FormatError::Io(_) if e.is_input_changed() => ("error.input_changed", vec![]),
        FormatError::Io(_) if e.is_destination_changed() => ("error.destination_changed", vec![]),
        FormatError::Io(_) if e.is_output_exists() => ("error.output_exists", vec![]),
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
            FormatError::output_exists("archive.repaired.zip"),
            FormatError::destination_changed("archive.zip"),
            FormatError::input_changed(),
            FormatError::Io(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "temporary allocation collision",
            )),
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

    #[test]
    fn tagged_output_conflict_uses_a_specific_error_key() {
        let error = FormatError::output_exists("private/output.zip");

        let message = error_message(&error);

        assert_eq!(message.key, "error.output_exists");
        assert!(message.params.is_empty());
    }

    #[test]
    fn unrelated_already_exists_error_stays_generic_io() {
        let error = FormatError::Io(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "temporary allocation collision",
        ));

        assert_eq!(error_message(&error).key, "error.io");
    }

    #[test]
    fn stale_replacement_authorization_uses_a_specific_error_key() {
        let error = FormatError::destination_changed("private/archive.zip");

        let message = error_message(&error);

        assert_eq!(message.key, "error.destination_changed");
        assert!(message.params.is_empty());
    }

    #[test]
    fn stale_extraction_input_uses_a_specific_error_key() {
        let message = error_message(&FormatError::input_changed());

        assert_eq!(message.key, "error.input_changed");
        assert!(message.params.is_empty());
    }

    #[test]
    fn missing_volume_uses_the_specific_message_without_exposing_its_directory() {
        let error = FormatError::missing_volume("/private/downloads/archive.7z.004");

        let message = error_message(&error);

        assert_eq!(message.key, "gui.error.corrupt.volume_missing");
        assert_eq!(message.params, vec![("name", "archive.7z.004".to_owned())]);
        let zh = Localizer::with_user_dir(Some("zh-CN"), None);
        assert_eq!(
            localize_error(&zh, &error),
            "缺少分卷 archive.7z.004，请将所有分卷放在同一文件夹"
        );
    }

    #[test]
    fn split_wim_stream_without_a_source_folder_explains_the_disk_path() {
        let error = FormatError::split_wim_unsupported();
        let message = error_message(&error);

        assert_eq!(message.key, "error.unsupported_split_wim");
        assert!(message.params.is_empty());
        let zh = Localizer::with_user_dir(Some("zh-CN"), None);
        assert_eq!(
            localize_error(&zh, &error),
            "这个 Split WIM 流没有可定位其他分卷的来源文件夹。请从磁盘打开任一 .swm 卷，并把全部分卷放在一起。"
        );
    }

    #[test]
    fn split_wim_creation_explains_the_required_native_options() {
        let error = FormatError::split_wim_creation_unsupported();
        let message = error_message(&error);

        assert_eq!(message.key, "error.unsupported_split_wim_create");
        assert!(message.params.is_empty());
        let zh = Localizer::with_user_dir(Some("zh-CN"), None);
        assert_eq!(
            localize_error(&zh, &error),
            "创建 .swm 需要设置分卷大小，并选择“原生 Split WIM”布局。"
        );
    }
}
