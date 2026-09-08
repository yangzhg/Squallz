use std::path::{Path, PathBuf};
use std::sync::Mutex as StdMutex;

use squallz_core::api::{EntryPath, ProgressSink};
use squallz_core::{ChecksumAlgorithm, PostSuccessAction};

use super::source_cleanup::{TrashAdapter, TrashError};
use crate::dto::JobSpec;
use crate::events::EventSink;
use std::fs;

#[derive(Default, Clone)]
pub(super) struct FakeTrashAdapter {
    fail_names: Vec<String>,
    calls: std::sync::Arc<StdMutex<Vec<PathBuf>>>,
}

impl FakeTrashAdapter {
    pub(super) fn failing(names: &[&str]) -> Self {
        Self {
            fail_names: names.iter().map(|name| (*name).to_owned()).collect(),
            calls: std::sync::Arc::new(StdMutex::new(Vec::new())),
        }
    }

    pub(super) fn calls(&self) -> Vec<PathBuf> {
        self.calls.lock().unwrap().clone()
    }
}

impl TrashAdapter for FakeTrashAdapter {
    fn move_to_trash(&self, path: &Path) -> Result<(), TrashError> {
        self.calls.lock().unwrap().push(path.to_path_buf());
        let name = path
            .file_name()
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_default();
        if self.fail_names.contains(&name) {
            Err(TrashError)
        } else {
            let removed = match fs::symlink_metadata(path) {
                Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
                    fs::remove_dir_all(path)
                }
                Ok(_) => fs::remove_file(path),
                Err(error) => Err(error),
            };
            removed.map_err(|_| TrashError)
        }
    }
}

pub(super) fn deterministic_payload(len: usize) -> Vec<u8> {
    let mut state = 0x9e37_79b9_u32;
    let mut bytes = Vec::with_capacity(len);
    for _ in 0..len {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        bytes.push((state >> 24) as u8);
    }
    bytes
}

/// Buffering event sink for tests.
#[derive(Default)]
pub(super) struct TestSink {
    pub(super) events: StdMutex<Vec<(String, serde_json::Value)>>,
}

impl EventSink for TestSink {
    fn emit_json(&self, event: &str, payload: serde_json::Value) {
        self.events
            .lock()
            .unwrap()
            .push((event.to_owned(), payload));
    }
}

#[derive(Default)]
pub(super) struct RecordingProgressSink {
    pub(super) paths: StdMutex<Vec<String>>,
}

impl ProgressSink for RecordingProgressSink {
    fn on_progress(&self, _done: u64, _total: u64, current: &EntryPath) {
        self.paths.lock().unwrap().push(current.display.clone());
    }

    fn on_scan_progress(&self, _scanned_entries: u64, current: &EntryPath) {
        self.paths.lock().unwrap().push(current.display.clone());
    }
}

pub(super) fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("squallz-gui-jobs-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

pub(super) fn checksum_job(input: &Path) -> JobSpec {
    JobSpec::Checksum {
        inputs: vec![input.to_string_lossy().into_owned()],
        excludes: Vec::new(),
        algorithm: ChecksumAlgorithm::Sha256,
    }
}

pub(super) fn compress_file_job(input: &Path, output: &Path) -> JobSpec {
    compress_file_job_with_inner_format(input, output, None)
}

pub(super) fn compress_file_job_with_inner_format(
    input: &Path,
    output: &Path,
    sqz_inner_format: Option<squallz_core::SqzInnerFormat>,
) -> JobSpec {
    JobSpec::Compress {
        inputs: vec![input.to_string_lossy().into_owned()],
        dest: output.to_string_lossy().into_owned(),
        level: 5,
        password: None,
        encrypt_names: false,
        split_size: None,
        split_mode: squallz_core::api::SplitOutputMode::Generic,
        excludes: Vec::new(),
        content_policy: squallz_core::CreateContentPolicy::KeepAllFiles,
        sqz_inner_format,
        sfx_target: None,
        completion: squallz_core::CreateCompletionAction::None,
        post_success: PostSuccessAction::KeepSource,
        test_after_create: false,
        replace_existing: false,
        replacement_guard: None,
    }
}
