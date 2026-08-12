use std::io::{self, Write};
use std::time::UNIX_EPOCH;

use serde_json::{json, Value};
use squallz_core::api::{EntryMeta, EntryType, ExtractReport, FormatError, TestSummary};
use squallz_core::{ExtractPlan, SmartLayout};
use squallz_i18n::{localize_error, Localizer};

pub(crate) fn write_stdout(text: &str) -> Result<(), FormatError> {
    let result = {
        let stdout = io::stdout();
        let mut output = stdout.lock();
        output.write_all(text.as_bytes())
    };
    match result {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        Err(error) => Err(error.into()),
    }
}

pub(crate) fn write_stderr(text: &str) {
    let stderr = io::stderr();
    let mut output = stderr.lock();
    let _ = output.write_all(text.as_bytes());
}

pub(crate) fn safe_terminal_text(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_control() {
                '\u{fffd}'
            } else {
                character
            }
        })
        .collect()
}

pub(crate) fn write_json(value: &Value) -> Result<(), FormatError> {
    let mut text = serde_json::to_string_pretty(value)
        .map_err(|error| FormatError::Other(format!("cannot serialize JSON output: {error}")))?;
    text.push('\n');
    write_stdout(&text)
}

pub(crate) fn error_json(error: &FormatError, loc: &Localizer) -> Value {
    let code = exit_code(error);
    json!({
        "ok": false,
        "error": {
            "kind": error_kind(error),
            "message": localize_error(loc, error),
            "exit_code": code,
        }
    })
}

pub(crate) fn entry_json(entry: &EntryMeta) -> Value {
    let (entry_type, link_target) = match &entry.entry_type {
        EntryType::File => ("file", None),
        EntryType::Dir => ("dir", None),
        EntryType::Symlink { target } => (
            "symlink",
            Some(String::from_utf8_lossy(target).into_owned()),
        ),
        EntryType::Hardlink { target } => (
            "hardlink",
            Some(String::from_utf8_lossy(target).into_owned()),
        ),
        EntryType::Other => ("other", None),
    };
    json!({
        "path": entry.path.display,
        "encoding": entry.path.encoding,
        "type": entry_type,
        "link_target": link_target,
        "size": entry.size,
        "compressed_size": entry.compressed_size,
        "modified": entry
            .modified
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|duration| duration.as_secs()),
        "unix_mode": entry.unix_mode,
        "crc32": entry.crc32,
        "encrypted": entry.encrypted,
    })
}

pub(crate) fn test_json(report: &TestSummary) -> Value {
    json!({
        "ok": report.is_ok(),
        "entries_tested": report.entries_tested,
        "problems": &report.problems.messages,
        "problems_total": report.problems.total,
        "problems_truncated": report.problems.is_truncated(),
        "recovery": null,
    })
}

pub(crate) fn extract_json(plan: &ExtractPlan, report: &ExtractReport) -> Value {
    json!({
        "ok": true,
        "operation": "extract",
        "dest": report.destination.display().to_string(),
        "matched": true,
        "best_effort": false,
        "skipped": 0,
        "problems": [],
        "problems_total": 0,
        "problems_truncated": false,
        "plan": {
            "requested_destination": plan.requested_destination.display().to_string(),
            "destination": plan.destination.display().to_string(),
            "layout": match plan.layout {
                SmartLayout::DirectExtract => "direct",
                SmartLayout::WrapInFolder => "wrap_in_folder",
            },
            "entries": plan.scope.entries,
            "files": plan.scope.files,
            "directories": plan.scope.directories,
            "symlinks": plan.scope.symlinks,
            "hardlinks": plan.scope.hardlinks,
            "other": plan.scope.other,
            "total_bytes": plan.scope.total_bytes,
            "estimated_conflicts": plan.estimated_conflicts,
        },
        "counts": {
            "destination": report.destination.display().to_string(),
            "selected_entries": report.selected_entries,
            "created": report.created,
            "directories": report.directories,
            "skipped": report.skipped,
            "replaced": report.replaced,
            "renamed": report.renamed,
            "failed": report.failed,
            "output_bytes": report.output_bytes,
        },
        "selected_entries": report.selected_entries,
        "directories": report.directories,
        "output_bytes": report.output_bytes,
    })
}

pub(crate) fn exit_code(error: &FormatError) -> i32 {
    match error {
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

pub(crate) fn error_kind(error: &FormatError) -> &'static str {
    match error {
        FormatError::Unsupported(_) => "unsupported",
        FormatError::CorruptArchive(_) => "corrupt_archive",
        FormatError::PasswordRequired => "password_required",
        FormatError::WrongPassword => "wrong_password",
        FormatError::Cancelled => "cancelled",
        FormatError::PathTraversal(_) => "path_traversal",
        FormatError::SymlinkBreakout(_) => "symlink_breakout",
        FormatError::ResourceLimitExceeded(_) => "resource_limit_exceeded",
        FormatError::UnsafeFileName(_) => "unsafe_file_name",
        FormatError::Io(_) if error.is_destination_changed() => "destination_changed",
        FormatError::Io(_) if error.is_output_exists() => "output_exists",
        FormatError::Io(_) => "io",
        FormatError::DiskFull => "disk_full",
        FormatError::DependencyMissing(_) => "dependency_missing",
        FormatError::Other(_) => "other",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_exit_mapping_covers_every_format_error_family() {
        let cases = [
            (FormatError::Unsupported(String::new()), 2),
            (FormatError::CorruptArchive(String::new()), 3),
            (FormatError::PasswordRequired, 4),
            (FormatError::WrongPassword, 4),
            (FormatError::Cancelled, 5),
            (FormatError::PathTraversal(String::new()), 6),
            (FormatError::SymlinkBreakout(String::new()), 6),
            (FormatError::ResourceLimitExceeded(String::new()), 6),
            (FormatError::UnsafeFileName(String::new()), 6),
            (FormatError::Io(io::Error::other("failed")), 7),
            (FormatError::DiskFull, 7),
            (FormatError::DependencyMissing(String::new()), 8),
            (FormatError::Other(String::new()), 1),
        ];
        for (error, expected) in cases {
            assert_eq!(exit_code(&error), expected);
        }
    }

    #[test]
    fn json_error_keeps_the_full_cli_shape() {
        let loc = Localizer::with_user_dir(Some("en-US"), None);
        let value = error_json(&FormatError::PasswordRequired, &loc);

        assert_eq!(value["ok"], false);
        assert_eq!(value["error"]["kind"], "password_required");
        assert_eq!(value["error"]["exit_code"], 4);
        assert!(value["error"]["message"].as_str().is_some());
    }
}
