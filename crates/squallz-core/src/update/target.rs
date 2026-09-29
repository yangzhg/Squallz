use crate::api::{sanitize_entry_path, ControlToken, EntryPath, FormatError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveTargetInspection {
    pub exists: bool,
    pub blocked_parent: Option<String>,
}

/// Checks one proposed entry against a complete displayed-path index. The
/// lookup distinguishes files from slash-terminated (including implicit)
/// directories. Actual updates still validate the source and final namespace.
pub fn inspect_archive_target(
    target: &str,
    exists: impl Fn(&str) -> bool,
    ctl: &ControlToken,
) -> Result<ArchiveTargetInspection, FormatError> {
    ctl.checkpoint()?;
    validate_target(target)?;
    Ok(ArchiveTargetInspection {
        exists: occupied(target.trim_end_matches('/'), &exists),
        blocked_parent: blocked_parent(target, &exists, ctl)?,
    })
}

pub(super) fn occupied(key: &str, exists: &impl Fn(&str) -> bool) -> bool {
    exists(key) || exists(&format!("{key}/"))
}

pub(super) fn blocked_parent(
    target: &str,
    exists: &impl Fn(&str) -> bool,
    ctl: &ControlToken,
) -> Result<Option<String>, FormatError> {
    for (index, _) in target.trim_end_matches('/').match_indices('/') {
        ctl.checkpoint()?;
        let parent = &target[..index];
        if exists(parent) {
            return Ok(Some(parent.into()));
        }
    }
    Ok(None)
}

pub(super) fn validate_target(path: &str) -> Result<(), FormatError> {
    let relative = sanitize_entry_path(&EntryPath::from_utf8(path))?;
    let canonical = relative
        .components()
        .map(|part| part.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");
    if canonical != path.trim_end_matches('/') || canonical.is_empty() {
        return Err(FormatError::UnsafeFileName(path.into()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targets_respect_exact_names_directory_aliases_and_blocking_ancestors() {
        let names = [
            "docs/",
            "docs/File.txt",
            "docs/empty/",
            "implicit/",
            "implicit/child",
            "file",
        ];
        let inspect = |path| {
            inspect_archive_target(path, |name| names.contains(&name), &ControlToken::new())
                .unwrap()
        };
        for path in [
            "docs/File.txt",
            "docs/File.txt/",
            "docs/empty",
            "docs/empty/",
            "implicit",
            "implicit/",
        ] {
            let found = inspect(path);
            assert!(found.exists, "{path}");
            assert_eq!(found.blocked_parent, None, "{path}");
        }
        for path in ["docs/file.txt", "docs/new/", "missing/new/file.txt"] {
            assert_eq!(
                inspect(path),
                ArchiveTargetInspection {
                    exists: false,
                    blocked_parent: None
                }
            );
        }
        for path in ["file/child", "file/new/child/", "docs/File.txt/child/"] {
            let blocked = inspect(path);
            assert!(!blocked.exists);
            assert_eq!(
                blocked.blocked_parent.as_deref(),
                Some(if path.starts_with("file/") {
                    "file"
                } else {
                    "docs/File.txt"
                })
            );
        }
    }

    #[test]
    fn target_inspection_rejects_unsafe_noncanonical_paths_and_cancellation() {
        for path in [
            "",
            "/",
            "../file",
            "a/../file",
            "/absolute",
            "a\\file",
            "a//file",
            "a/./file",
        ] {
            assert!(
                inspect_archive_target(path, |_| false, &ControlToken::new()).is_err(),
                "{path}"
            );
        }
        let ctl = ControlToken::new();
        ctl.cancel();
        assert!(matches!(
            inspect_archive_target("file", |_| false, &ctl),
            Err(FormatError::Cancelled)
        ));
    }
}
