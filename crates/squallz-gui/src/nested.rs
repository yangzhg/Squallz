//! Shared helpers for archive entries that themselves contain archives.

#[cfg(test)]
use std::fs;
use std::fs::File;
use std::io::{self, Read, Write};
use std::path::Path;

#[cfg(test)]
use squallz_core::api::EntryMeta;
use squallz_core::api::{
    ControlToken, EntryPath, FormatError, OpenOptions, Password, ProgressSink, SafetyLimits,
};

use crate::password_cache::PasswordAttempt;
use crate::state::AppState;
#[cfg(test)]
use squallz_core::copy_archive_entry;
use squallz_core::{EntryReference, PlaintextFile, PlaintextWorkspace};

const MAX_NESTED_EXTENSION_BYTES: usize = 16;
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
    workspace: &PlaintextWorkspace,
) -> Result<(PlaintextFile, File), FormatError> {
    let suffix = safe_entry_suffix(entry_path);
    Ok(workspace.create_file(&format!("nested-entry{suffix}"))?)
}

pub(crate) fn create_nested_job_workspace() -> Result<PlaintextWorkspace, FormatError> {
    Ok(PlaintextWorkspace::create_in(&std::env::temp_dir())?)
}

fn copy_with_limit<R: Read + ?Sized, W: Write>(
    reader: &mut R,
    writer: &mut W,
    max_bytes: u64,
) -> Result<u64, FormatError> {
    let written = {
        let mut limited = (&mut *reader).take(max_bytes);
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

pub(crate) struct PreparedEntry {
    pub size: u64,
    password: Option<(PasswordAttempt, Password)>,
}

impl PreparedEntry {
    pub(crate) fn remember_password(&self, state: &AppState, control: &ControlToken) {
        if let Some((attempt, password)) = &self.password {
            state.remember_password(attempt, password.expose(), control);
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn write_archive_entry_limited<W: Write>(
    state: &AppState,
    outer_path: &Path,
    entry_path: &str,
    password: Option<&str>,
    encoding: Option<&str>,
    writer: &mut W,
    max_bytes: u64,
    control: &ControlToken,
) -> Result<PreparedEntry, FormatError> {
    let attempt = password.and_then(|_| state.password_attempt(outer_path, control));
    let open_opts = OpenOptions {
        password: password
            .map(Password::new)
            .or_else(|| state.password_for(outer_path)),
        encoding_override: encoding.map(str::to_owned),
    };
    let mut written = 0;
    let verified = state.engine.read_entry_verifying_password(
        outer_path,
        &EntryPath::from_utf8(entry_path),
        &open_opts,
        control,
        &mut |entry| {
            written = copy_with_limit(entry, writer, max_bytes)?;
            Ok(())
        },
    )?;
    Ok(PreparedEntry {
        size: written,
        password: if verified {
            attempt.zip(open_opts.password)
        } else {
            None
        },
    })
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn extract_nested_archive_to_temp_limited(
    state: &AppState,
    outer_path: &Path,
    entry_path: &str,
    password: Option<&str>,
    encoding: Option<&str>,
    workspace: &PlaintextWorkspace,
    max_bytes: u64,
    control: &ControlToken,
) -> Result<(PlaintextFile, PreparedEntry), FormatError> {
    let (temp, mut writer) = create_nested_temp_file(entry_path, workspace)?;
    let prepared = write_archive_entry_limited(
        state,
        outer_path,
        entry_path,
        password,
        encoding,
        &mut writer,
        max_bytes,
        control,
    )?;
    writer.flush()?;
    drop(writer);
    Ok((temp, prepared))
}

/// Materializes one archive entry for a queued nested-extraction job. The
/// caller supplies the same safety limits, progress sink, and control token
/// used by the extraction that follows. The returned plaintext owner deletes the
/// private file on every normal, error, and cancellation path.
#[allow(clippy::too_many_arguments)]
pub(crate) fn extract_nested_archive_to_temp_for_job(
    state: &AppState,
    outer_path: &Path,
    entry_path: &str,
    password: Option<&Password>,
    encoding: Option<&str>,
    workspace: &PlaintextWorkspace,
    limits: SafetyLimits,
    progress: &dyn ProgressSink,
    ctl: &ControlToken,
) -> Result<PlaintextFile, FormatError> {
    let open_opts = OpenOptions {
        password: password.cloned(),
        encoding_override: encoding.map(str::to_owned),
    };
    let prepared = state.engine.prepare_entry_materialization(
        outer_path,
        EntryReference::Display(entry_path),
        &open_opts,
        ctl,
    )?;
    let (temp, mut writer) = create_nested_temp_file(entry_path, workspace)?;
    prepared.copy_to(&temp, &mut writer, limits, progress)?;
    drop(writer);
    Ok(temp)
}

#[cfg(test)]
mod tests {
    use super::*;
    use squallz_core::CreateCommitPolicy;
    use std::collections::HashSet;
    use std::sync::{Arc, Mutex};

    const NESTED_COPY_BUFFER_BYTES: usize = 256 * 1024;

    struct WriteAction<F: FnMut()> {
        action: F,
    }

    #[test]
    fn queued_nested_selection_rejects_duplicate_names_and_normalized_aliases() {
        use squallz_core::api::{CreateOptions, Detected, EntryType, NoProgress};

        let root = tempfile::tempdir().unwrap();
        let outer = root.path().join("outer.tar");
        let workspace = PlaintextWorkspace::create_in(root.path()).unwrap();
        let original_files = fs::read_dir(workspace.path()).unwrap().count();
        let Some(Detected::Archive(format)) =
            squallz_formats::registry().detect_by_name("outer.tar")
        else {
            panic!("TAR format must be registered");
        };
        let state = AppState::new();
        for last_name in ["folder/inner.zip", r"folder\inner.zip"] {
            let mut writer = format
                .create(
                    Box::new(fs::File::create(&outer).unwrap()),
                    &CreateOptions::default(),
                    &ControlToken::default(),
                )
                .unwrap();
            for (index, name) in ["folder/inner.zip", "unrelated.txt", last_name]
                .into_iter()
                .enumerate()
            {
                let meta = EntryMeta {
                    path: EntryPath::from_utf8(name),
                    entry_type: EntryType::File,
                    size: index as u64 + 1,
                    compressed_size: None,
                    modified: None,
                    unix_mode: None,
                    crc32: None,
                    encrypted: false,
                };
                let mut data = io::Cursor::new(vec![b'x'; meta.size as usize]);
                writer.add_entry(&meta, Some(&mut data)).unwrap();
            }
            writer.finish().unwrap();

            let result = extract_nested_archive_to_temp_for_job(
                &state,
                &outer,
                "/folder/inner.zip",
                None,
                None,
                &workspace,
                SafetyLimits::default(),
                &NoProgress,
                &ControlToken::default(),
            );
            assert!(matches!(result, Err(FormatError::CorruptArchive(_))));
            assert_eq!(
                fs::read_dir(workspace.path()).unwrap().count(),
                original_files
            );
            let raw = state.engine.prepare_entry_materialization(
                &outer,
                EntryReference::Raw(&EntryPath::from_utf8("folder/inner.zip")),
                &OpenOptions::default(),
                &ControlToken::default(),
            );
            #[cfg(unix)]
            assert_eq!(raw.is_ok(), last_name.contains('\\'));
            #[cfg(not(unix))]
            drop(raw);
        }
    }

    #[test]
    fn complete_materialization_retains_raw_identity_and_checks_its_output_owner() {
        use squallz_core::api::{CreateOptions, Detected, EntryType, NoProgress};

        let root = tempfile::tempdir().unwrap();
        let outer = root.path().join("outer.tar");
        let Some(Detected::Archive(format)) =
            squallz_formats::registry().detect_by_name("outer.tar")
        else {
            panic!("TAR format must be registered");
        };
        let control = ControlToken::default();
        let mut writer = format
            .create(
                Box::new(File::create(&outer).unwrap()),
                &CreateOptions::default(),
                &control,
            )
            .unwrap();
        let payload = b"the selected archive bytes";
        let meta = EntryMeta {
            path: EntryPath::from_utf8(r"folder\inner.bin"),
            entry_type: EntryType::File,
            size: payload.len() as u64,
            compressed_size: None,
            modified: None,
            unix_mode: None,
            crc32: None,
            encrypted: false,
        };
        writer
            .add_entry(&meta, Some(&mut io::Cursor::new(payload)))
            .unwrap();
        writer.finish().unwrap();
        let state = AppState::new();
        let workspace = PlaintextWorkspace::create_in(root.path()).unwrap();
        let (output, mut writer) = workspace.create_file("selected.bin").unwrap();
        let (other, mut other_writer) = workspace.create_file("other.bin").unwrap();
        let prepare = || {
            state
                .engine
                .prepare_entry_materialization(
                    &outer,
                    EntryReference::Display("/folder/inner.bin"),
                    &OpenOptions::default(),
                    &control,
                )
                .unwrap()
        };
        let mismatch = prepare().copy_to(
            &output,
            &mut other_writer,
            SafetyLimits::default(),
            &NoProgress,
        );
        assert!(matches!(mismatch, Err(FormatError::Io(_))));
        let limited = prepare().copy_to(
            &output,
            &mut writer,
            SafetyLimits {
                max_output_bytes: payload.len() as u64 - 1,
                ..SafetyLimits::default()
            },
            &NoProgress,
        );
        assert!(matches!(
            limited,
            Err(FormatError::ResourceLimitExceeded(_))
        ));
        assert!(fs::read(output.path()).unwrap().is_empty());
        assert!(fs::read(other.path()).unwrap().is_empty());
        assert_eq!(
            prepare()
                .copy_to(&output, &mut writer, SafetyLimits::default(), &NoProgress)
                .unwrap(),
            payload.len() as u64
        );
        drop((writer, other_writer, workspace));
        assert_eq!(fs::read(output.path()).unwrap(), payload);
        let output_root = output.root_path().to_path_buf();
        output.close().unwrap();
        assert!(!output_root.exists());
    }

    impl<F: FnMut()> Write for WriteAction<F> {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            (self.action)();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn preview_read_cancellation_and_forget_prevent_password_publication() {
        use squallz_core::api::{CreateOptions, NoProgress};
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("secret.txt");
        fs::write(&file, vec![b'x'; 128 * 1024]).unwrap();
        let path = dir.path().join("secret.zip");
        let state = AppState::new();
        state
            .engine
            .create(
                &path,
                &[file],
                &CreateOptions {
                    password: Some(Password::new("secret")),
                    ..CreateOptions::default()
                },
                CreateCommitPolicy::ReplaceExisting,
                &NoProgress,
                &ControlToken::default(),
            )
            .unwrap();
        for cancel in [false, true] {
            let control = ControlToken::default();
            let mut writer = WriteAction {
                action: || {
                    if cancel {
                        control.cancel();
                    } else {
                        state.forget_password(&path);
                    }
                },
            };
            let result = write_archive_entry_limited(
                &state,
                &path,
                "secret.txt",
                Some("secret"),
                None,
                &mut writer,
                256 * 1024,
                &control,
            );
            if cancel {
                assert!(matches!(result, Err(FormatError::Cancelled)));
            } else {
                let prepared = result.unwrap();
                assert_eq!(prepared.size, 128 * 1024);
                prepared.remember_password(&state, &control);
            }
            assert!(state.password_for(&path).is_none());
        }
    }

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
        let workspace = PlaintextWorkspace::create_in(root.path()).unwrap();
        let mut seen = HashSet::new();
        let mut opened = Vec::new();
        for _ in 0..128 {
            let (file, writer) = create_nested_temp_file("dir/inner.zip", &workspace).unwrap();
            drop(writer);
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
        let workspace = PlaintextWorkspace::create_in(root.path()).unwrap();
        let (file, writer) =
            create_nested_temp_file("../dir/inner archive?.zip", &workspace).unwrap();
        drop(writer);
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
        let (file, writer) = create_nested_temp_file("inner.zip", &workspace).unwrap();
        drop(writer);
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
    fn sevenz_preview_limit_and_nested_cancellation_stop_before_later_corruption() {
        use squallz_core::api::{CompressionLevel, CreateOptions, NoProgress};
        use std::io::{Seek, SeekFrom};

        let root = tempfile::tempdir().unwrap();
        let input = root.path().join("inner.bin");
        let outer = root.path().join("outer.7z");
        let size = 1024 * 1024;
        std::fs::write(&input, vec![b'x'; size]).unwrap();
        let state = AppState::new();
        state
            .engine
            .create(
                &outer,
                &[input],
                &CreateOptions {
                    level: CompressionLevel::Store,
                    ..CreateOptions::default()
                },
                CreateCommitPolicy::ReplaceExisting,
                &NoProgress,
                &ControlToken::default(),
            )
            .unwrap();
        // COPY payload begins after the fixed 7z signature header. Damage
        // beyond either consumer's stopping point must not be read eagerly.
        let mut archive = std::fs::OpenOptions::new()
            .write(true)
            .open(&outer)
            .unwrap();
        archive.seek(SeekFrom::Start(32 + size as u64 - 1)).unwrap();
        archive.write_all(b"y").unwrap();
        drop(archive);

        let mut output = Vec::new();
        let result = write_archive_entry_limited(
            &state,
            &outer,
            "inner.bin",
            None,
            None,
            &mut output,
            32,
            &ControlToken::default(),
        );
        assert!(
            matches!(result, Err(FormatError::ResourceLimitExceeded(_))),
            "{:?}",
            result.err()
        );
        assert_eq!(output, vec![b'x'; 32]);

        let workspace = create_nested_job_workspace().unwrap();
        let workspace_files = std::fs::read_dir(workspace.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<HashSet<_>>();
        let ctl = ControlToken::new();
        let progress = CancelAfterFirstChunk {
            ctl: Arc::clone(&ctl),
            samples: Mutex::new(Vec::new()),
        };
        let result = extract_nested_archive_to_temp_for_job(
            &state,
            &outer,
            "inner.bin",
            None,
            None,
            &workspace,
            SafetyLimits::default(),
            &progress,
            &ctl,
        );
        assert!(
            matches!(result, Err(FormatError::Cancelled)),
            "{:?}",
            result.err()
        );
        let remaining_files = std::fs::read_dir(workspace.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<HashSet<_>>();
        assert_eq!(remaining_files, workspace_files);
        let samples = progress.samples.lock().unwrap();
        assert_eq!(samples.len(), 2);
        assert_eq!(samples[0], (0, size as u64));
        assert!((1..=NESTED_COPY_BUFFER_BYTES as u64).contains(&samples[1].0));
        assert_eq!(samples[1].1, size as u64);
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

        let error = copy_archive_entry(
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
