//! Shared helpers for archive entries that themselves contain archives.

use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use squallz_core::api::{EntryPath, FormatError, OpenOptions, Password};

use crate::state::AppState;

const MAX_NESTED_TEMP_ATTEMPTS: u64 = 64;
const FALLBACK_NESTED_BASENAME: &str = "nested-archive";
static NESTED_TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

fn entry_basename_or_fallback(entry_path: &str) -> &str {
    match entry_path
        .rsplit(['/', '\\'])
        .next()
        .filter(|name| !name.is_empty())
    {
        Some(name) => name,
        None => FALLBACK_NESTED_BASENAME,
    }
}

fn safe_entry_basename(entry_path: &str) -> String {
    let basename = entry_basename_or_fallback(entry_path);
    let safe: String = basename
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect();
    if safe.is_empty() {
        FALLBACK_NESTED_BASENAME.into()
    } else {
        safe
    }
}

fn nanos_since_epoch_or_zero(now: SystemTime) -> u128 {
    match now.duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_nanos(),
        Err(_) => 0,
    }
}

fn nested_temp_path(entry_path: &str, nonce: u64, attempt: u64) -> PathBuf {
    let stamp = nanos_since_epoch_or_zero(SystemTime::now());
    std::env::temp_dir().join(format!(
        "squallz-nested-{}-{stamp}-{nonce}-{attempt}-{}",
        std::process::id(),
        safe_entry_basename(entry_path)
    ))
}

fn create_nested_temp_file(entry_path: &str) -> Result<(PathBuf, File), FormatError> {
    let nonce = NESTED_TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    for attempt in 0..MAX_NESTED_TEMP_ATTEMPTS {
        let path = nested_temp_path(entry_path, nonce, attempt);
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(file) => return Ok((path, file)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.into()),
        }
    }
    Err(FormatError::Other(format!(
        "cannot create unique nested archive temp file for {}",
        safe_entry_basename(entry_path)
    )))
}

pub(crate) fn extract_nested_archive_to_temp(
    state: &AppState,
    outer_path: &Path,
    entry_path: &str,
    password: Option<&str>,
    encoding: Option<&str>,
) -> Result<PathBuf, FormatError> {
    let open_opts = OpenOptions {
        password: password
            .map(Password::new)
            .or_else(|| state.password_for(outer_path)),
        encoding_override: encoding.map(str::to_owned),
    };
    let mut outer = state.engine.open(outer_path, &open_opts)?;
    let mut entry = outer.read_entry(&EntryPath::from_utf8(entry_path))?;
    let (temp, mut out) = create_nested_temp_file(entry_path)?;
    match std::io::copy(&mut entry, &mut out) {
        Ok(_) => Ok(temp),
        Err(e) => {
            let err = FormatError::from(e);
            let _ = fs::remove_file(&temp);
            Err(err)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn remove_created_temp(path: PathBuf) {
        match std::fs::remove_file(path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => panic!("cannot remove nested temp file: {e}"),
        }
    }

    #[test]
    fn nested_temp_files_are_unique_for_same_entry() {
        let mut seen = HashSet::new();
        let mut opened = Vec::new();
        for _ in 0..128 {
            let (temp, file) = create_nested_temp_file("dir/inner.zip").unwrap();
            assert!(temp
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default()
                .starts_with("squallz-nested-"));
            assert!(seen.insert(temp.clone()), "duplicate temp path: {temp:?}");
            opened.push((temp, file));
        }

        for (temp, file) in opened {
            drop(file);
            remove_created_temp(temp);
        }
    }

    #[test]
    fn nested_temp_file_sanitizes_entry_names() {
        let (temp, file) = create_nested_temp_file("../dir/inner archive?.zip").unwrap();
        let name = temp
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_owned();
        drop(file);
        remove_created_temp(temp);
        assert!(name.starts_with("squallz-nested-"));
        assert!(name.ends_with("inner_archive_.zip"));
        assert!(!name.contains('/'));
        assert!(!name.contains('\\'));
    }
}
