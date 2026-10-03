//! RAR fallback listing from bsdtar's ordered name and verbose passes.

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Command, Stdio};

use squallz_format_api::{ControlToken, EntryMeta, EntryPath, EntryType, FormatError};

use crate::external_process::{StdoutExit, StdoutProcess};

use super::map_tool_spawn_error;

#[derive(Default)]
struct NamesListing {
    entries: Vec<EntryMeta>,
    ordinals: Vec<usize>,
}

pub(super) fn list_entries(
    tool: &Path,
    archive: &Path,
    control: &ControlToken,
) -> Result<Vec<EntryMeta>, FormatError> {
    let mut names = NamesListing::default();
    let mut process = spawn_listing(tool, archive, "-tf", control)?;
    let read = read_names(BufReader::new(&mut process), &mut names);
    ensure_success(process.finish(read)?)?;

    match read_verbose_details(tool, archive, &names.ordinals, control) {
        Ok(details) => {
            for (entry, detail) in names.entries.iter_mut().zip(details) {
                if let Some(detail) = detail {
                    entry.entry_type = detail.entry_type;
                    entry.size = detail.size;
                    entry.unix_mode = detail.unix_mode;
                }
            }
        }
        Err(FormatError::Cancelled) => return Err(FormatError::Cancelled),
        Err(_) => {}
    }
    Ok(names.entries)
}

fn read_names(mut reader: impl BufRead, listing: &mut NamesListing) -> Result<(), FormatError> {
    let mut line = Vec::new();
    let mut ordinal = 0;
    loop {
        line.clear();
        if reader.read_until(b'\n', &mut line)? == 0 {
            return Ok(());
        }
        if line.last() == Some(&b'\n') {
            line.pop();
        }
        let raw = trim_cr(&line);
        if !raw.is_empty() {
            let display = String::from_utf8_lossy(raw).into_owned();
            let entry_type = if display.ends_with('/') {
                EntryType::Dir
            } else {
                EntryType::File
            };
            listing.entries.push(EntryMeta {
                path: EntryPath::from_raw(raw.to_vec(), display, "utf-8"),
                entry_type,
                size: 0,
                compressed_size: None,
                modified: None,
                unix_mode: None,
                crc32: None,
                encrypted: false,
            });
            // Empty lines still occupy a verbose row in the second pass.
            listing.ordinals.push(ordinal);
        }
        ordinal += 1;
    }
}

fn read_verbose_details(
    tool: &Path,
    archive: &Path,
    ordinals: &[usize],
    control: &ControlToken,
) -> Result<Vec<Option<VerboseEntry>>, FormatError> {
    let mut details = std::iter::repeat_with(|| None)
        .take(ordinals.len())
        .collect::<Vec<_>>();
    let mut process = spawn_listing(tool, archive, "-tvf", control)?;
    let read = read_details(BufReader::new(&mut process), ordinals, &mut details);
    ensure_success(process.finish(read)?)?;
    Ok(details)
}

fn read_details(
    mut reader: impl BufRead,
    ordinals: &[usize],
    details: &mut [Option<VerboseEntry>],
) -> Result<(), FormatError> {
    let mut line = Vec::new();
    let mut ordinal = 0;
    let mut index = 0;
    loop {
        line.clear();
        if reader.read_until(b'\n', &mut line)? == 0 {
            return Ok(());
        }
        if line.last() == Some(&b'\n') {
            line.pop();
        }
        if ordinals.get(index) == Some(&ordinal) {
            details[index] = parse_verbose_entry(&line);
            index += 1;
        }
        // Continue draining even after every requested row has been seen.
        ordinal += 1;
    }
}

fn spawn_listing(
    tool: &Path,
    archive: &Path,
    flag: &str,
    control: &ControlToken,
) -> Result<StdoutProcess, FormatError> {
    control.checkpoint()?;
    let child = Command::new(tool)
        .arg(flag)
        .arg(archive)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| map_tool_spawn_error(error, "bsdtar with RAR/libarchive support"))?;
    StdoutProcess::new(child, control, "bsdtar")
}

fn ensure_success(output: StdoutExit) -> Result<(), FormatError> {
    if output.status.success() {
        Ok(())
    } else {
        Err(map_tool_failure(&output.stderr))
    }
}

fn trim_cr(raw: &[u8]) -> &[u8] {
    match raw.strip_suffix(b"\r") {
        Some(stripped) => stripped,
        None => raw,
    }
}

struct VerboseEntry {
    entry_type: EntryType,
    size: u64,
    unix_mode: Option<u32>,
}

fn parse_verbose_entry(raw: &[u8]) -> Option<VerboseEntry> {
    let raw = trim_cr(raw);
    if raw.is_empty() {
        return None;
    }
    let line = String::from_utf8_lossy(raw);
    let mut parts = line.split_whitespace();
    let mode = parts.next()?;
    let _links = parts.next()?;
    let _owner = parts.next()?;
    let _group = parts.next()?;
    let size = parts.next()?.parse().ok()?;
    let _month = parts.next()?;
    let _day = parts.next()?;
    let _time_or_year = parts.next()?;
    let rest = parts.collect::<Vec<_>>().join(" ");
    if rest.is_empty() {
        return None;
    }
    let entry_type = match mode.as_bytes().first().copied()? {
        b'd' => EntryType::Dir,
        b'l' => {
            let target = symlink_target_from_verbose_rest(&rest);
            EntryType::Symlink {
                target: target.as_bytes().to_vec(),
            }
        }
        _ => EntryType::File,
    };
    Some(VerboseEntry {
        entry_type,
        size,
        unix_mode: unix_mode_from_verbose(mode),
    })
}

fn symlink_target_from_verbose_rest(rest: &str) -> &str {
    match rest.split_once(" -> ") {
        Some((_, target)) => target,
        None => "",
    }
}

fn unix_mode_from_verbose(mode: &str) -> Option<u32> {
    let bytes = mode.as_bytes();
    if bytes.len() < 10 {
        return None;
    }
    let kind = match bytes[0] {
        b'd' => 0o040000,
        b'l' => 0o120000,
        b'-' => 0o100000,
        _ => 0,
    };
    let mut perms = 0u32;
    for (idx, byte) in bytes[1..10].iter().enumerate() {
        let bit = match idx {
            0 => 0o400,
            1 => 0o200,
            2 => 0o100,
            3 => 0o040,
            4 => 0o020,
            5 => 0o010,
            6 => 0o004,
            7 => 0o002,
            8 => 0o001,
            _ => 0,
        };
        if *byte != b'-' {
            perms |= bit;
        }
    }
    Some(kind | perms)
}

fn map_tool_failure(stderr: &[u8]) -> FormatError {
    let detail = String::from_utf8_lossy(stderr).trim().to_owned();
    let lower = detail.to_lowercase();
    if lower.contains("unsupported") || lower.contains("not supported") {
        FormatError::DependencyMissing("bsdtar with RAR/libarchive support".into())
    } else if lower.contains("password") {
        FormatError::PasswordRequired
    } else {
        FormatError::CorruptArchive(if detail.is_empty() {
            "bsdtar could not read RAR archive".into()
        } else {
            detail
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rar_verbose_parser_handles_cr_and_missing_symlink_target() {
        let symlink = parse_verbose_entry(
            b"lrwxrwxrwx  0 0      0           0 Jan  1  2020 link -> hello.txt\r",
        )
        .expect("symlink verbose entry");
        assert_eq!(symlink.size, 0);
        assert_eq!(symlink.unix_mode, Some(0o120777));
        assert!(matches!(
            symlink.entry_type,
            EntryType::Symlink { target } if target == b"hello.txt"
        ));

        let symlink_without_arrow =
            parse_verbose_entry(b"lrwxrwxrwx  0 0      0           0 Jan  1  2020 link")
                .expect("symlink without arrow still parses");
        assert!(matches!(
            symlink_without_arrow.entry_type,
            EntryType::Symlink { target } if target.is_empty()
        ));
    }
}
