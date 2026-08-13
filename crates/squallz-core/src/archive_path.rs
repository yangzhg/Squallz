use std::ffi::{OsStr, OsString};
use std::path::Path;

use squallz_format_api::{split_volume_name, FormatError};

/// Returns whether `path` names an unsplit SQZ container.
pub fn is_plain_sqz_path(path: &Path) -> bool {
    has_extension(path, "sqz")
}

/// Returns whether `path` names an SQZ container or one of its generic split
/// volumes.
pub fn is_sqz_archive_path(path: &Path) -> bool {
    is_plain_sqz_path(path)
        || path
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| split_volume_name(name).map(|(base, _)| base))
            .is_some_and(|base| is_plain_sqz_path(Path::new(base)))
}

/// Returns whether `path` uses one of the ZIP-family extensions handled by
/// the ZIP index repair workflow.
pub fn is_zip_family_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "zip" | "jar" | "apk" | "cbz" | "ipa"
            )
        })
}

fn has_extension(path: &Path, expected: &str) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case(expected))
}

pub(crate) fn is_canonical_process_sequence(value: &str) -> bool {
    let Some((process, sequence)) = value.split_once('-') else {
        return false;
    };
    !sequence.contains('-')
        && is_canonical_positive_u32(process)
        && is_canonical_positive_u64(sequence)
}

pub(crate) fn is_canonical_positive_u32(value: &str) -> bool {
    has_canonical_positive_integer_syntax(value)
        && value.parse::<u32>().is_ok_and(|value| value > 0)
}

pub(crate) fn is_canonical_positive_u64(value: &str) -> bool {
    has_canonical_positive_integer_syntax(value)
        && value.parse::<u64>().is_ok_and(|value| value > 0)
}

pub(crate) fn checked_path_component(
    name: Option<&OsStr>,
    role: &str,
) -> Result<OsString, FormatError> {
    let name = name.ok_or_else(|| FormatError::Unsupported(format!("{role} has no file name")))?;
    let path = Path::new(name);
    if path.file_name() != Some(path.as_os_str())
        || path
            .parent()
            .is_some_and(|parent| !parent.as_os_str().is_empty())
    {
        return Err(FormatError::Unsupported(format!(
            "{role} must be a single file name"
        )));
    }
    Ok(name.to_os_string())
}

fn has_canonical_positive_integer_syntax(value: &str) -> bool {
    let Some((&first, rest)) = value.as_bytes().split_first() else {
        return false;
    };
    (b'1'..=b'9').contains(&first) && rest.iter().all(u8::is_ascii_digit)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_path_classification_is_case_insensitive_and_split_aware() {
        assert!(is_plain_sqz_path(Path::new("archive.SQZ")));
        assert!(is_sqz_archive_path(Path::new("archive.sqz.001")));
        assert!(!is_plain_sqz_path(Path::new("archive.sqz.001")));
        assert!(is_zip_family_path(Path::new("comic.CBZ")));
        assert!(!is_zip_family_path(Path::new("archive.7z")));
    }

    #[test]
    fn process_sequence_requires_canonical_positive_integers() {
        assert!(is_canonical_process_sequence("1-1"));
        assert!(is_canonical_process_sequence(
            "4294967295-18446744073709551615"
        ));
        for value in ["0-1", "1-0", "01-1", "1-01", "1-1-1", "-1", "1-"] {
            assert!(!is_canonical_process_sequence(value), "matched {value}");
        }
    }
}
