//! Shared helpers for archive entries that themselves contain archives.

use std::fs;
use std::io::{self, Read, Write};
use std::path::Path;

use squallz_core::api::{
    ControlToken, EntryMeta, EntryPath, FormatError, LimitsAccountant, OpenOptions, Password,
    ProgressSink, SafetyLimits,
};
use tempfile::{Builder, NamedTempFile, TempPath};

use crate::preview_workspace::PreviewWorkspace;
use crate::state::AppState;

const MAX_NESTED_EXTENSION_BYTES: usize = 16;
const NESTED_COPY_BUFFER_BYTES: usize = 256 * 1024;
const NESTED_TEMP_MIN_FREE_BYTES: u64 = 64 * 1024 * 1024;
pub(crate) const PREVIEW_ENTRY_TOO_LARGE_DETAIL: &str =
    "preview entry exceeds the temporary-file limit";

fn safe_entry_suffix(entry_path: &str) -> String {
    let basename = entry_path.rsplit(['/', '\\']).next().unwrap_or_default();
    match Path::new(basename)
        .extension()
        .and_then(|extension| extension.to_str())
    {
        Some(extension)
            if !extension.is_empty()
                && extension.len() <= MAX_NESTED_EXTENSION_BYTES
                && extension.chars().all(|ch| ch.is_ascii_alphanumeric()) =>
        {
            format!(".{}", extension.to_ascii_lowercase())
        }
        _ => String::new(),
    }
}

fn create_nested_temp_file(
    entry_path: &str,
    workspace: &Path,
) -> Result<NamedTempFile, FormatError> {
    let suffix = safe_entry_suffix(entry_path);
    let file = Builder::new()
        .prefix("nested-")
        .suffix(&suffix)
        .tempfile_in(workspace)?;
    set_private_file_permissions(file.as_file())?;
    Ok(file)
}

#[cfg(unix)]
fn set_private_file_permissions(file: &fs::File) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    file.set_permissions(fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn set_private_file_permissions(_file: &fs::File) -> io::Result<()> {
    Ok(())
}

pub(crate) fn create_nested_job_workspace() -> Result<PreviewWorkspace, FormatError> {
    Ok(PreviewWorkspace::create_in(&std::env::temp_dir())?)
}

fn copy_with_limit<R: Read, W: Write>(
    reader: &mut R,
    writer: &mut W,
    max_bytes: u64,
) -> Result<u64, FormatError> {
    let written = {
        let mut limited = reader.by_ref().take(max_bytes);
        io::copy(&mut limited, writer)?
    };
    let mut probe = [0_u8; 1];
    if written == max_bytes && reader.read(&mut probe)? > 0 {
        return Err(FormatError::ResourceLimitExceeded(
            PREVIEW_ENTRY_TOO_LARGE_DETAIL.to_owned(),
        ));
    }
    Ok(written)
}

pub(crate) fn write_archive_entry_limited<W: Write>(
    state: &AppState,
    outer_path: &Path,
    entry_path: &str,
    password: Option<&str>,
    encoding: Option<&str>,
    writer: &mut W,
    max_bytes: u64,
) -> Result<u64, FormatError> {
    let open_opts = OpenOptions {
        password: password
            .map(Password::new)
            .or_else(|| state.password_for(outer_path)),
        encoding_override: encoding.map(str::to_owned),
    };
    let mut outer = state.engine.open(outer_path, &open_opts)?;
    let mut entry = outer.read_entry(&EntryPath::from_utf8(entry_path))?;
    copy_with_limit(&mut entry, writer, max_bytes)
}

pub(crate) fn extract_nested_archive_to_temp_limited(
    state: &AppState,
    outer_path: &Path,
    entry_path: &str,
    password: Option<&str>,
    encoding: Option<&str>,
    workspace: &Path,
    max_bytes: u64,
) -> Result<(TempPath, u64), FormatError> {
    let mut temp = create_nested_temp_file(entry_path, workspace)?;
    let size = write_archive_entry_limited(
        state,
        outer_path,
        entry_path,
        password,
        encoding,
        temp.as_file_mut(),
        max_bytes,
    )?;
    temp.as_file_mut().flush()?;
    Ok((temp.into_temp_path(), size))
}

fn normalized_entry_name(path: &str) -> String {
    path.replace('\\', "/").trim_start_matches('/').to_owned()
}

fn find_nested_entry(
    outer: &mut dyn squallz_core::api::ArchiveReader,
    entry_path: &str,
    ctl: &ControlToken,
) -> Result<EntryMeta, FormatError> {
    let requested = normalized_entry_name(entry_path);
    for entry in outer.entries() {
        ctl.checkpoint()?;
        let entry = entry?;
        if normalized_entry_name(&entry.path.display) == requested {
            return Ok(entry);
        }
    }
    Err(FormatError::Other(format!(
        "nested archive entry not found: {entry_path}"
    )))
}

fn copy_nested_entry_with_limits<R: Read, W: Write>(
    reader: &mut R,
    writer: &mut W,
    meta: &EntryMeta,
    limits: SafetyLimits,
    progress: &dyn ProgressSink,
    ctl: &ControlToken,
) -> Result<u64, FormatError> {
    let mut accountant = LimitsAccountant::new(limits);
    accountant.check_entry(meta)?;
    let mut written = 0_u64;
    let mut buffer = vec![0_u8; NESTED_COPY_BUFFER_BYTES];
    progress.on_entry_progress(0, 0, &meta.path, 0, meta.size);
    loop {
        ctl.checkpoint()?;
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        accountant.add_output_bytes(read as u64)?;
        writer.write_all(&buffer[..read])?;
        written = written.saturating_add(read as u64);
        progress.on_entry_progress(0, 0, &meta.path, written, meta.size.max(written));
    }
    Ok(written)
}

fn nested_temp_limits(
    workspace: &Path,
    mut limits: SafetyLimits,
) -> Result<(SafetyLimits, u64), FormatError> {
    let available = fs4::available_space(workspace)?;
    let headroom = (available / 20)
        .max(NESTED_TEMP_MIN_FREE_BYTES)
        .min(available);
    let writable = available.saturating_sub(headroom);
    if writable == 0 {
        return Err(FormatError::DiskFull);
    }
    limits.max_output_bytes = limits.max_output_bytes.min(writable);
    Ok((limits, writable))
}

/// Materializes one archive entry for a queued nested-extraction job. The
/// caller supplies the same safety limits, progress sink, and control token
/// used by the extraction that follows. The returned TempPath deletes the
/// private file on every normal, error, and cancellation path.
#[allow(clippy::too_many_arguments)]
pub(crate) fn extract_nested_archive_to_temp_for_job(
    state: &AppState,
    outer_path: &Path,
    entry_path: &str,
    password: Option<&Password>,
    encoding: Option<&str>,
    workspace: &Path,
    limits: SafetyLimits,
    progress: &dyn ProgressSink,
    ctl: &ControlToken,
) -> Result<TempPath, FormatError> {
    let open_opts = OpenOptions {
        password: password.cloned(),
        encoding_override: encoding.map(str::to_owned),
    };
    let mut outer = state.engine.open(outer_path, &open_opts)?;
    let meta = find_nested_entry(outer.as_mut(), entry_path, ctl)?;
    let (limits, writable) = nested_temp_limits(workspace, limits)?;
    if meta.size > limits.max_output_bytes {
        return Err(if meta.size > writable {
            FormatError::DiskFull
        } else {
            FormatError::ResourceLimitExceeded(format!(
                "output bytes exceed limit of {}",
                limits.max_output_bytes
            ))
        });
    }
    let mut entry = outer.read_entry(&meta.path)?;
    let mut temp = create_nested_temp_file(entry_path, workspace)?;
    copy_nested_entry_with_limits(&mut entry, temp.as_file_mut(), &meta, limits, progress, ctl)?;
    ctl.checkpoint()?;
    temp.as_file_mut().flush()?;
    Ok(temp.into_temp_path())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::sync::{Arc, Mutex};

    struct CancelAfterFirstChunk {
        ctl: Arc<ControlToken>,
        samples: Mutex<Vec<(u64, u64)>>,
    }

    impl ProgressSink for CancelAfterFirstChunk {
        fn on_progress(&self, done: u64, total: u64, current: &EntryPath) {
            self.on_entry_progress(done, total, current, 0, 0);
        }

        fn on_entry_progress(
            &self,
            _done: u64,
            _total: u64,
            _current: &EntryPath,
            current_done: u64,
            current_total: u64,
        ) {
            self.samples
                .lock()
                .unwrap()
                .push((current_done, current_total));
            if current_done > 0 {
                self.ctl.cancel();
            }
        }
    }

    #[test]
    fn nested_temp_files_are_unique_for_same_entry() {
        let root = tempfile::tempdir().expect("temporary root should be created");
        let mut seen = HashSet::new();
        let mut opened = Vec::new();
        for _ in 0..128 {
            let file = create_nested_temp_file("dir/inner.zip", root.path()).unwrap();
            assert!(file
                .path()
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default()
                .starts_with("nested-"));
            assert!(
                seen.insert(file.path().to_path_buf()),
                "duplicate temp path: {:?}",
                file.path()
            );
            opened.push(file);
        }

        drop(opened);
    }

    #[test]
    fn nested_temp_file_hides_entry_names_and_keeps_safe_extension() {
        let root = tempfile::tempdir().expect("temporary root should be created");
        let file = create_nested_temp_file("../dir/inner archive?.zip", root.path()).unwrap();
        let name = file
            .path()
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_owned();
        drop(file);
        assert!(name.starts_with("nested-"));
        assert!(name.ends_with(".zip"));
        assert!(!name.contains("inner"));
        assert!(!name.contains('/'));
        assert!(!name.contains('\\'));
    }

    #[test]
    fn nested_job_workspace_and_file_are_removed_by_raii() {
        let workspace = create_nested_job_workspace().unwrap();
        let workspace_path = workspace.path().to_path_buf();
        let file = create_nested_temp_file("inner.zip", workspace.path()).unwrap();
        let file_path = file.path().to_path_buf();

        assert!(workspace_path.exists());
        assert!(file_path.exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&workspace_path).unwrap().permissions().mode() & 0o777,
                0o700
            );
            assert_eq!(
                fs::metadata(&file_path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        drop(file);
        assert!(!file_path.exists());
        drop(workspace);
        assert!(!workspace_path.exists());
    }

    #[test]
    fn bounded_copy_accepts_exact_limit_and_rejects_one_extra_byte() {
        let mut exact = io::Cursor::new(b"1234");
        let mut exact_output = Vec::new();
        assert_eq!(
            copy_with_limit(&mut exact, &mut exact_output, 4).unwrap(),
            4
        );
        assert_eq!(exact_output, b"1234");

        let mut oversized = io::Cursor::new(b"12345");
        let mut oversized_output = Vec::new();
        assert!(matches!(
            copy_with_limit(&mut oversized, &mut oversized_output, 4),
            Err(FormatError::ResourceLimitExceeded(_))
        ));
        assert_eq!(oversized_output, b"1234");
    }

    #[test]
    fn queued_nested_copy_reports_current_file_progress_and_cancels_by_chunk() {
        let ctl = ControlToken::new();
        let progress = CancelAfterFirstChunk {
            ctl: Arc::clone(&ctl),
            samples: Mutex::new(Vec::new()),
        };
        let size = NESTED_COPY_BUFFER_BYTES * 2;
        let meta = EntryMeta {
            path: EntryPath::from_utf8("inner.zip"),
            entry_type: squallz_core::api::EntryType::File,
            size: size as u64,
            compressed_size: Some(size as u64),
            modified: None,
            unix_mode: None,
            crc32: None,
            encrypted: false,
        };
        let mut reader = io::Cursor::new(vec![0xA5; size]);
        let mut output = Vec::new();

        let error = copy_nested_entry_with_limits(
            &mut reader,
            &mut output,
            &meta,
            SafetyLimits::default(),
            &progress,
            &ctl,
        )
        .unwrap_err();

        assert!(matches!(error, FormatError::Cancelled));
        assert_eq!(output.len(), NESTED_COPY_BUFFER_BYTES);
        assert_eq!(
            progress.samples.lock().unwrap().as_slice(),
            &[
                (0, size as u64),
                (NESTED_COPY_BUFFER_BYTES as u64, size as u64)
            ]
        );
    }
}
