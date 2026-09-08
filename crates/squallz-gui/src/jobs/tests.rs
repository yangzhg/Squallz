use super::progress::BATCH_PROGRESS_SCALE;
use super::snapshots::JobOrigin;
use super::test_support::{
    checksum_job, compress_file_job, compress_file_job_with_inner_format, deterministic_payload,
    temp_dir, FakeTrashAdapter, TestSink,
};
use super::*;
use crate::dto::{BatchExtractItem, JobSpec, PERFORMANCE_STREAM_BUFFER_MAX_BYTES};
use crate::events::{EV_ASK_CONFLICT, EV_ASK_PASSWORD, EV_PROGRESS};
use crate::preview_sessions::PreviewSessionManager;
use squallz_core::api::{CompressionLevel, CreateOptions, OpenOptions, Password};
use squallz_core::ChecksumAlgorithm;
use squallz_core::QueueWaitReason;
use squallz_core::{CreateArtifactKind, PostSuccessAction};
use std::fs;
use std::io::Write as _;
use std::path::Path;
use std::sync::Mutex as StdMutex;
use std::time::Instant;

#[cfg(unix)]
static EXTERNAL_TOOL_ENV_LOCK: StdMutex<()> = StdMutex::new(());

#[cfg(unix)]
struct EnvRestore {
    key: &'static str,
    old: Option<std::ffi::OsString>,
}

#[cfg(unix)]
impl EnvRestore {
    fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        let old = std::env::var_os(key);
        std::env::set_var(key, value);
        Self { key, old }
    }
}

#[cfg(unix)]
impl Drop for EnvRestore {
    fn drop(&mut self) {
        match &self.old {
            Some(value) => std::env::set_var(self.key, value),
            None => std::env::remove_var(self.key),
        }
    }
}

#[test]
fn scheduler_limits_and_cpu_reservations_are_conservative() {
    assert_eq!(resolved_parallel_job_limit(None, 1), 1);
    assert_eq!(resolved_parallel_job_limit(None, 4), 1);
    assert_eq!(resolved_parallel_job_limit(None, 8), 2);
    assert_eq!(resolved_parallel_job_limit(None, 16), 4);
    assert_eq!(resolved_parallel_job_limit(Some(0), 16), 1);
    assert_eq!(resolved_parallel_job_limit(Some(99), 16), 8);

    let state = AppState::new();
    let zip = JobSpec::RepairZip {
        src: "broken.zip".into(),
        dest: "repaired.zip".into(),
        level: 6,
    };
    let zstd = compress_file_job(Path::new("input.bin"), Path::new("archive.tar.zst"));
    let wim = compress_file_job(Path::new("input.bin"), Path::new("archive.wim"));
    let sqz_7z = compress_file_job_with_inner_format(
        Path::new("input.bin"),
        Path::new("archive.sqz"),
        Some(squallz_core::SqzInnerFormat::SevenZip),
    );
    let recovery = JobSpec::Protect {
        path: "archive.zip".into(),
        redundancy: 10,
        recovery: None,
    };
    let light = JobSpec::Checksum {
        inputs: vec!["input.bin".into()],
        excludes: Vec::new(),
        algorithm: ChecksumAlgorithm::Blake3,
    };
    let automatic = SettingsDto::default();
    let manual = SettingsDto {
        performance_threads: Some(3),
        ..SettingsDto::default()
    };

    assert_eq!(
        scheduler_resources(scheduler_cpu_profile(&state.engine, &zip), &automatic, 8),
        JobResources::new(1)
    );
    assert_eq!(
        scheduler_resources(scheduler_cpu_profile(&state.engine, &light), &automatic, 8),
        JobResources::new(1)
    );
    assert_eq!(
        scheduler_resources(scheduler_cpu_profile(&state.engine, &zstd), &automatic, 8),
        JobResources::new(8)
    );
    assert_eq!(
        scheduler_resources(scheduler_cpu_profile(&state.engine, &zstd), &manual, 8),
        JobResources::new(3)
    );
    assert_eq!(
        scheduler_resources(scheduler_cpu_profile(&state.engine, &wim), &automatic, 8),
        JobResources::new(8)
    );
    assert_eq!(
        scheduler_resources(scheduler_cpu_profile(&state.engine, &sqz_7z), &automatic, 8),
        JobResources::new(1)
    );
    assert_eq!(
        scheduler_resources(scheduler_cpu_profile(&state.engine, &recovery), &manual, 8),
        JobResources::new(8)
    );

    let execution = settings_for_job_execution(
        automatic.clone(),
        scheduler_cpu_profile(&state.engine, &zstd),
        JobResources::new(8),
    );
    assert_eq!(execution.resource_options().threads, Some(8));
    let serial_execution = settings_for_job_execution(
        manual.clone(),
        scheduler_cpu_profile(&state.engine, &zip),
        JobResources::new(1),
    );
    assert_eq!(serial_execution.resource_options().threads, Some(3));

    let custom_buffer = SettingsDto {
        performance_memory_limit_bytes: Some(512 * 1024 * 1024),
        ..SettingsDto::default()
    };
    assert_eq!(
        job_stream_buffer_limit_bytes(&zip, &custom_buffer),
        Some(PERFORMANCE_STREAM_BUFFER_MAX_BYTES)
    );
    assert_eq!(job_stream_buffer_limit_bytes(&light, &custom_buffer), None);
    assert_eq!(
        job_stream_buffer_limit_bytes(&zip, &SettingsDto::default()),
        None
    );
    assert!(job_supports_pause(&light));
    assert!(!job_supports_pause(&JobSpec::Protect {
        path: "archive.zip".into(),
        redundancy: 10,
        recovery: None,
    }));
    assert!(!job_supports_pause(&JobSpec::VerifyRecovery {
        path: "archive.zip".into(),
        recovery: None,
    }));
    assert!(!job_supports_pause(&JobSpec::RepairRecovery {
        path: "archive.zip".into(),
        output: None,
        output_directory: false,
        recovery: None,
    }));
}

#[test]
fn private_path_redaction_keeps_the_original_error_category() {
    let private = "/private/squallz-preview/inner.zip";
    let error = redact_format_error_path(
        FormatError::CorruptArchive(format!("invalid footer in {private}")),
        private,
        "inner.zip",
    );

    match error {
        FormatError::CorruptArchive(detail) => {
            assert_eq!(detail, "invalid footer in inner.zip");
        }
        other => panic!("expected corrupt archive, got {other:?}"),
    }

    let error = redact_format_error_path(
        FormatError::Io(io::Error::new(
            io::ErrorKind::StorageFull,
            format!("no space left while writing {private}"),
        )),
        private,
        "inner.zip",
    );
    assert!(matches!(error, FormatError::DiskFull));
}

#[test]
fn output_conflict_redaction_keeps_the_contextual_marker() {
    let private = "/private/squallz-preview/archive.repaired.zip";
    let error = redact_format_error_path(
        FormatError::output_exists(private),
        private,
        "archive.repaired.zip",
    );

    assert!(error.is_output_exists());
    assert_eq!(
        error.output_exists_path(),
        Some(Path::new("archive.repaired.zip"))
    );
    assert!(!error.to_string().contains("/private/squallz-preview"));
    assert_eq!(ErrorDto::from_engine(&error).key, "error.output_exists");
}

fn create_password_protected_zip(dir: &Path, state: &AppState) -> PathBuf {
    let src = dir.join("secret-src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("secret.txt"), b"window ownership fixture").unwrap();
    let archive = dir.join("secret.zip");
    state
        .engine
        .create(
            &archive,
            std::slice::from_ref(&src),
            &CreateOptions {
                password: Some(Password::new("secret")),
                ..CreateOptions::default()
            },
            &squallz_core::api::NoProgress,
            &ControlToken::new(),
        )
        .unwrap();
    archive
}

fn password_test_job(archive: &Path) -> JobSpec {
    JobSpec::Test {
        path: archive.to_string_lossy().into_owned(),
        encoding: None,
        password: None,
    }
}

#[cfg(target_os = "macos")]
fn write_macos_sfx_template(path: &Path) {
    let executable = path.join("Contents/MacOS/squallz-gui");
    std::fs::create_dir_all(executable.parent().unwrap()).unwrap();
    std::fs::create_dir_all(path.join("Contents/Resources")).unwrap();
    let mut bytes = vec![0u8; 512];
    bytes[..4].copy_from_slice(&[0xcf, 0xfa, 0xed, 0xfe]);
    bytes[0x80..0x80 + squallz_core::SFX_GUI_STUB_MARKER.len()]
        .copy_from_slice(&squallz_core::SFX_GUI_STUB_MARKER);
    std::fs::write(executable, bytes).unwrap();
    std::fs::write(
        path.join("Contents/Info.plist"),
        r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>squallz-gui</string>
<key>LSMinimumSystemVersion</key><string>11.0</string>
</dict></plist>
"#,
    )
    .unwrap();
}

#[cfg(target_os = "macos")]
fn write_host_sfx_template(path: &Path) {
    write_macos_sfx_template(path);
}

#[cfg(target_os = "windows")]
fn write_host_sfx_template(path: &Path) {
    let mut bytes = vec![0u8; 512];
    bytes[..2].copy_from_slice(b"MZ");
    bytes[0x3c..0x40].copy_from_slice(&0x80u32.to_le_bytes());
    bytes[0x80..0x84].copy_from_slice(b"PE\0\0");
    bytes[0x94..0x96].copy_from_slice(&240u16.to_le_bytes());
    bytes[0x98..0x9a].copy_from_slice(&0x20bu16.to_le_bytes());
    bytes[0x104..0x108].copy_from_slice(&16u32.to_le_bytes());
    bytes[0x190..0x190 + squallz_core::SFX_CLI_STUB_MARKER.len()]
        .copy_from_slice(&squallz_core::SFX_CLI_STUB_MARKER);
    std::fs::write(path, bytes).unwrap();
}

#[cfg(target_os = "linux")]
fn write_host_sfx_template(path: &Path) {
    let mut bytes = vec![0u8; 128];
    bytes[..4].copy_from_slice(b"\x7fELF");
    bytes[0x40..0x40 + squallz_core::SFX_CLI_STUB_MARKER.len()]
        .copy_from_slice(&squallz_core::SFX_CLI_STUB_MARKER);
    std::fs::write(path, bytes).unwrap();
}

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
#[test]
fn sfx_capability_distinguishes_missing_invalid_and_available_runtimes() {
    let missing = JobManager::with_audit_and_template(Arc::new(OperationAudit::memory()), None);
    let capability = missing.sfx_capability();
    assert!(!capability.available);
    assert_eq!(capability.status, "missing");

    let dir = temp_dir("sfx-capability-status");
    let invalid_path = dir.join("invalid-runtime");
    std::fs::write(&invalid_path, b"not an executable runtime").unwrap();
    let invalid =
        JobManager::with_test_sfx_template(Arc::new(OperationAudit::memory()), invalid_path);
    let capability = invalid.sfx_capability();
    assert!(!capability.available);
    assert_eq!(capability.status, "invalid");

    let valid_path = match SfxTarget::host() {
        SfxTarget::Macos => dir.join("Squallz.app"),
        SfxTarget::Windows => dir.join("sqz.exe"),
        SfxTarget::Linux => dir.join("sqz"),
    };
    write_host_sfx_template(&valid_path);
    let available =
        JobManager::with_test_sfx_template(Arc::new(OperationAudit::memory()), valid_path);
    let capability = available.sfx_capability();
    assert!(capability.available);
    assert_eq!(capability.status, "available");

    std::fs::remove_dir_all(dir).unwrap();
}

fn corrupt_sqz_payload_byte(path: &Path) {
    let mut bytes = std::fs::read(path).unwrap();
    assert!(bytes.len() > 64);
    assert_eq!(&bytes[0..8], b"SQZARCH\x1A");
    let descriptor_len = u64::from_le_bytes(bytes[40..48].try_into().unwrap()) as usize;
    let payload_start = 64 + descriptor_len;
    assert!(
        payload_start < bytes.len(),
        "payload starts outside archive"
    );
    bytes[payload_start] ^= 0xA5;
    std::fs::write(path, bytes).unwrap();
}

fn write_incompressible_file(path: &Path, len: usize) {
    let mut state = 0x9E37_79B9u32;
    let data: Vec<u8> = (0..len)
        .map(|_| {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (state >> 24) as u8
        })
        .collect();
    std::fs::write(path, data).unwrap();
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

#[cfg(unix)]
fn rar5_test_volume(index: u64, has_next: bool) -> Vec<u8> {
    fn encode_vint(mut value: u64) -> Vec<u8> {
        let mut bytes = Vec::new();
        loop {
            let mut byte = (value & 0x7f) as u8;
            value >>= 7;
            if value != 0 {
                byte |= 0x80;
            }
            bytes.push(byte);
            if value == 0 {
                return bytes;
            }
        }
    }

    fn block(header_type: u64, fields: &[u8]) -> Vec<u8> {
        let mut header = encode_vint(header_type);
        header.extend(encode_vint(0));
        header.extend_from_slice(fields);
        let size = encode_vint(header.len() as u64);
        let mut checksum_data = size.clone();
        checksum_data.extend_from_slice(&header);
        let mut block = crc32(&checksum_data).to_le_bytes().to_vec();
        block.extend(size);
        block.extend(header);
        block
    }

    let mut bytes = b"Rar!\x1A\x07\x01\x00".to_vec();
    let mut archive_flags = 0x0001;
    let mut main_fields = Vec::new();
    if index > 0 {
        archive_flags |= 0x0002;
    }
    main_fields.extend(encode_vint(archive_flags));
    if index > 0 {
        main_fields.extend(encode_vint(index));
    }
    bytes.extend(block(1, &main_fields));
    bytes.extend(block(5, &encode_vint(u64::from(has_next))));
    bytes
}

#[cfg(unix)]
fn split_zip_test_final_volume(final_disk: u16) -> Vec<u8> {
    let mut bytes = vec![0u8; 22];
    bytes[..4].copy_from_slice(b"PK\x05\x06");
    bytes[4..6].copy_from_slice(&final_disk.to_le_bytes());
    bytes[6..8].copy_from_slice(&final_disk.to_le_bytes());
    bytes
}

fn build_stored_zip(entries: &[(&[u8], &[u8])]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data) in entries {
        let offset = out.len() as u32;
        let crc = crc32(data);
        let size = data.len() as u32;
        let name_len = name.len() as u16;

        out.extend_from_slice(&[0x50, 0x4B, 0x03, 0x04]);
        out.extend_from_slice(&20u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0x21u16.to_le_bytes());
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&name_len.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(name);
        out.extend_from_slice(data);

        central.extend_from_slice(&[0x50, 0x4B, 0x01, 0x02]);
        central.extend_from_slice(&20u16.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0x21u16.to_le_bytes());
        central.extend_from_slice(&crc.to_le_bytes());
        central.extend_from_slice(&size.to_le_bytes());
        central.extend_from_slice(&size.to_le_bytes());
        central.extend_from_slice(&name_len.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u32.to_le_bytes());
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name);
    }
    let central_offset = out.len() as u32;
    let central_size = central.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(&[0x50, 0x4B, 0x05, 0x06]);
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    out.extend_from_slice(&central_size.to_le_bytes());
    out.extend_from_slice(&central_offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

fn states_of(events: &[(String, serde_json::Value)], id: u64) -> Vec<String> {
    events
        .iter()
        .filter(|(name, p)| name == EV_STATE && p["id"] == id)
        .map(|(_, p)| p["state"].as_str().unwrap().to_owned())
        .collect()
}

fn real_current_file_progress_events(
    events: &[(String, serde_json::Value)],
    id: u64,
) -> Vec<&serde_json::Value> {
    events
        .iter()
        .filter(|(name, payload)| {
            name == EV_PROGRESS
                && payload["id"] == id
                && payload["current_total"].as_u64().unwrap_or(0) > 0
        })
        .map(|(_, payload)| payload)
        .collect()
}

fn assert_real_current_file_progress(
    events: &[(String, serde_json::Value)],
    id: u64,
    operation: &str,
) {
    let progress = real_current_file_progress_events(events, id);
    assert!(
        !progress.is_empty(),
        "{operation} job should emit a real current-file progress event with current_total > 0"
    );
    assert!(
        progress.iter().any(|payload| {
            let done = payload["current_done"].as_u64().unwrap_or(0);
            let total = payload["current_total"].as_u64().unwrap_or(0);
            total > 0 && done <= total
        }),
        "{operation} job current-file progress should keep current_done bounded by current_total"
    );
}

fn wait_for_event(
    sink: &TestSink,
    timeout: std::time::Duration,
    predicate: impl Fn(&(String, serde_json::Value)) -> bool,
) {
    let started = Instant::now();
    while started.elapsed() < timeout {
        if sink.events.lock().unwrap().iter().any(&predicate) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    panic!("timed out waiting for event");
}

fn wait_for_state(sink: &TestSink, id: u64, state: &str, timeout: std::time::Duration) {
    wait_for_event(sink, timeout, |(name, payload)| {
        name == EV_STATE && payload["id"] == id && payload["state"] == state
    });
}

fn wait_for_password_prompt(sink: &TestSink, id: u64) {
    wait_for_event(
        sink,
        std::time::Duration::from_secs(2),
        |(name, payload)| name == EV_ASK_PASSWORD && payload["id"] == id,
    );
}

fn wait_for_password_prompt_count(sink: &TestSink, id: u64, expected: usize) {
    let started = Instant::now();
    while started.elapsed() < std::time::Duration::from_secs(2) {
        let count = sink
            .events
            .lock()
            .unwrap()
            .iter()
            .filter(|(name, payload)| name == EV_ASK_PASSWORD && payload["id"] == id)
            .count();
        if count >= expected {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    panic!("timed out waiting for password prompt {expected} for job {id}");
}

fn wait_for_snapshot_state(
    manager: &JobManager,
    id: u64,
    state: &str,
    timeout: std::time::Duration,
) {
    let started = Instant::now();
    while started.elapsed() < timeout {
        if manager
            .snapshot(id)
            .is_some_and(|snapshot| snapshot.state == state)
        {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    panic!("timed out waiting for job {id} snapshot state {state}");
}

fn done_result(events: &[(String, serde_json::Value)], id: u64) -> Option<serde_json::Value> {
    events
        .iter()
        .find(|(name, p)| name == EV_STATE && p["id"] == id && p["state"] == "done")
        .and_then(|(_, p)| p.get("result").cloned())
}

#[test]
fn main_window_can_reorder_visible_queued_jobs_without_cross_owner_access() {
    let manager = JobManager::new();
    let (gate_tx, gate_rx) = std::sync::mpsc::channel::<()>();
    let (started_tx, started_rx) = std::sync::mpsc::channel::<()>();
    manager.queue.submit(Box::new(move |_ctl, _progress| {
        started_tx.send(()).unwrap();
        gate_rx.recv().unwrap();
        Ok(())
    }));
    started_rx.recv().unwrap();

    let order = Arc::new(StdMutex::new(Vec::new()));
    let submit_marker = |marker| {
        let order = Arc::clone(&order);
        manager.queue.submit(Box::new(move |_ctl, _progress| {
            order.lock().unwrap().push(marker);
            Ok(())
        }))
    };
    let first_queue_id = submit_marker(1);
    let second_queue_id = submit_marker(2);
    let third_queue_id = submit_marker(3);
    let first_id = 101;
    let second_id = 102;
    let third_id = 103;
    let events = Arc::new(TestSink::default());
    {
        let mut snapshots = lock_unpoisoned(&manager.snapshots);
        snapshots.insert(
            first_id,
            Some("task-first".into()),
            checksum_job(Path::new("first-input")),
            "queued",
        );
        snapshots.insert(
            second_id,
            Some("task-second".into()),
            checksum_job(Path::new("second-input")),
            "queued",
        );
        snapshots.insert(
            third_id,
            Some("task-third".into()),
            checksum_job(Path::new("third-input")),
            "queued",
        );
    }
    {
        let mut registry = lock_unpoisoned(&manager.registry);
        registry.jobs.insert(
            first_id,
            ManagedJob {
                queue_id: first_queue_id,
                cancel_flag: Arc::new(AtomicBool::new(false)),
                owner_window: Some("task-first".into()),
                events: events.clone(),
                pausable: true,
            },
        );
        registry.jobs.insert(
            second_id,
            ManagedJob {
                queue_id: second_queue_id,
                cancel_flag: Arc::new(AtomicBool::new(false)),
                owner_window: Some("task-second".into()),
                events,
                pausable: true,
            },
        );
        registry.jobs.insert(
            third_id,
            ManagedJob {
                queue_id: third_queue_id,
                cancel_flag: Arc::new(AtomicBool::new(false)),
                owner_window: Some("task-third".into()),
                events: Arc::new(TestSink::default()),
                pausable: true,
            },
        );
    }
    manager.sync_queue_positions();
    assert_eq!(manager.snapshot(first_id).unwrap().queue_position, Some(1));
    assert_eq!(manager.snapshot(second_id).unwrap().queue_position, Some(2));
    assert_eq!(manager.snapshot(third_id).unwrap().queue_position, Some(3));
    assert_eq!(
        manager.snapshot(first_id).unwrap().queue_wait_reason,
        Some(QueueWaitReason::ParallelLimit)
    );
    assert_eq!(
        manager.snapshot(second_id).unwrap().queue_wait_reason,
        Some(QueueWaitReason::QueueOrder)
    );
    let first_snapshot = serde_json::to_value(manager.snapshot(first_id).unwrap()).unwrap();
    assert_eq!(first_snapshot["queue_wait_reason"], "parallel_limit");
    assert_eq!(first_snapshot["cpu_threads"], 1);

    assert!(manager
        .move_earlier_for_window("task-first", second_id)
        .is_err());
    assert!(manager
        .move_before_for_window("task-second", second_id, Some(first_id))
        .is_err());
    manager
        .move_before_for_window("main", third_id, Some(first_id))
        .unwrap();
    assert_eq!(manager.snapshot(third_id).unwrap().queue_position, Some(1));
    assert_eq!(manager.snapshot(first_id).unwrap().queue_position, Some(2));
    assert_eq!(manager.snapshot(second_id).unwrap().queue_position, Some(3));

    manager
        .move_before_for_window("main", first_id, None)
        .unwrap();
    assert_eq!(manager.snapshot(third_id).unwrap().queue_position, Some(1));
    assert_eq!(manager.snapshot(second_id).unwrap().queue_position, Some(2));
    assert_eq!(manager.snapshot(first_id).unwrap().queue_position, Some(3));

    manager.move_earlier_for_window("main", second_id).unwrap();
    assert_eq!(manager.snapshot(second_id).unwrap().queue_position, Some(1));
    assert_eq!(manager.snapshot(third_id).unwrap().queue_position, Some(2));
    assert_eq!(manager.snapshot(first_id).unwrap().queue_position, Some(3));
    assert!(manager.move_earlier_for_window("main", second_id).is_err());

    gate_tx.send(()).unwrap();
    manager.queue.wait_idle();
    assert_eq!(*order.lock().unwrap(), vec![2, 3, 1]);
}

#[test]
fn snapshot_controls_use_requester_scope_and_owner_event_sink() {
    let dir = temp_dir("snapshot-owner-controls");
    let state = Arc::new(AppState::new());
    let archive = create_password_protected_zip(&dir, &state);
    let manager = JobManager::new();
    let owner_sink = Arc::new(TestSink::default());
    let owner_events: Arc<dyn EventSink> = owner_sink.clone();
    let id = manager.submit_for_test_window(
        "task-owner".into(),
        Arc::clone(&state),
        owner_events,
        password_test_job(&archive),
        SettingsDto::default(),
    );

    wait_for_password_prompt(&owner_sink, id);
    let owner_snapshot = manager.snapshot_for_window("task-owner", id).unwrap();
    assert!(owner_snapshot.owned_by_requester);
    assert_eq!(owner_snapshot.interaction, Some(JobInteraction::Password));
    let main_snapshot = manager.snapshot_for_window("main", id).unwrap();
    assert!(!main_snapshot.owned_by_requester);
    assert_eq!(main_snapshot.origin, JobOrigin::FileManager);
    let denied = manager.snapshot_for_window("task-other", id).unwrap_err();
    assert_eq!(denied.key, "error.other");
    assert_eq!(
        manager.pause_for_window("task-other", id).unwrap_err().key,
        "error.other"
    );

    manager.pause_for_window("main", id).unwrap();
    manager.resume_for_window("main", id).unwrap();
    assert_eq!(
        manager
            .answer_password_for_window("main", id, None)
            .unwrap_err()
            .key,
        "error.other"
    );
    assert_eq!(
        manager
            .answer_conflict_for_window("task-owner", id, "skip".into(), false)
            .unwrap_err()
            .key,
        "error.other"
    );
    manager
        .answer_password_for_window("task-owner", id, None)
        .unwrap();
    wait_for_state(
        &owner_sink,
        id,
        "cancelled",
        std::time::Duration::from_secs(2),
    );
    assert_eq!(
        manager
            .answer_password_for_window("task-owner", id, None)
            .unwrap_err()
            .key,
        "error.other"
    );
    manager.wait_idle();

    let terminal = manager.snapshot_for_window("main", id).unwrap();
    assert_eq!(terminal.state, "cancelled");
    assert_eq!(terminal.interaction, None);
    let states = owner_sink.events.lock().unwrap().clone();
    assert_eq!(
        states_of(&states, id),
        vec!["queued", "running", "paused", "running", "cancelled"]
    );
    let versions = states
        .iter()
        .filter(|(name, payload)| name == EV_STATE && payload["id"] == id)
        .map(|(_, payload)| payload["version"].as_u64().unwrap())
        .collect::<Vec<_>>();
    assert!(versions.windows(2).all(|pair| pair[0] < pair[1]));
    assert_eq!(
        manager.cancel_for_window("main", u64::MAX).unwrap_err().key,
        "error.other"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn conflict_answers_require_the_owner_and_current_prompt_type() {
    let dir = temp_dir("snapshot-conflict-answer");
    let archive = dir.join("conflict.zip");
    std::fs::write(&archive, build_stored_zip(&[(b"same.txt", b"new bytes")])).unwrap();
    let output = dir.join("output");
    std::fs::create_dir_all(&output).unwrap();
    let existing = output.join("same.txt");
    std::fs::write(&existing, b"original bytes").unwrap();

    let manager = JobManager::new();
    let state = Arc::new(AppState::new());
    let owner_sink = Arc::new(TestSink::default());
    let owner_events: Arc<dyn EventSink> = owner_sink.clone();
    let id = manager.submit_for_test_window(
        "task-conflict-owner".into(),
        state,
        owner_events,
        JobSpec::Extract {
            path: archive.to_string_lossy().into_owned(),
            dest: output.to_string_lossy().into_owned(),
            expected_destination: None,
            expected_input_guard: None,
            selection: None,
            overwrite: squallz_core::api::OverwritePolicy::Ask,
            symlinks: squallz_core::api::SymlinkPolicy::Preserve,
            smart: false,
            encoding: None,
            password: None,
            verify_sfx: false,
            best_effort: false,
        },
        SettingsDto::default(),
    );

    wait_for_event(
        &owner_sink,
        std::time::Duration::from_secs(2),
        |(name, payload)| name == EV_ASK_CONFLICT && payload["id"] == id,
    );
    assert_eq!(
        manager
            .snapshot_for_window("task-conflict-owner", id)
            .unwrap()
            .interaction,
        Some(JobInteraction::Conflict)
    );
    assert_eq!(
        manager
            .answer_password_for_window("task-conflict-owner", id, None)
            .unwrap_err()
            .key,
        "error.other"
    );
    assert_eq!(
        manager
            .answer_conflict_for_window("main", id, "skip".into(), false)
            .unwrap_err()
            .key,
        "error.other"
    );
    manager
        .answer_conflict_for_window("task-conflict-owner", id, "skip".into(), false)
        .unwrap();
    wait_for_state(&owner_sink, id, "done", std::time::Duration::from_secs(2));
    assert_eq!(
        manager
            .answer_conflict_for_window("task-conflict-owner", id, "skip".into(), false)
            .unwrap_err()
            .key,
        "error.other"
    );
    manager.wait_idle();

    let terminal = manager.snapshot_for_window("main", id).unwrap();
    assert_eq!(terminal.interaction, None);
    assert_eq!(std::fs::read(existing).unwrap(), b"original bytes");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn queued_pause_does_not_claim_the_worker_until_resumed() {
    let dir = temp_dir("queued-pause-snapshot");
    let state = Arc::new(AppState::new());
    let archive = create_password_protected_zip(&dir, &state);
    let input = dir.join("queued.txt");
    std::fs::write(&input, b"queued pause").unwrap();
    let manager = JobManager::new();
    let sink = Arc::new(TestSink::default());
    let events: Arc<dyn EventSink> = sink.clone();

    let blocker_id = manager.submit(
        Arc::clone(&state),
        Arc::clone(&events),
        password_test_job(&archive),
        SettingsDto::default(),
    );
    wait_for_password_prompt(&sink, blocker_id);
    let queued_id = manager.submit(state, events, checksum_job(&input), SettingsDto::default());
    let queue_id = lock_unpoisoned(&manager.registry)
        .jobs
        .get(&queued_id)
        .unwrap()
        .queue_id;
    assert_eq!(manager.queue.state(queue_id), Some(JobState::Queued));
    assert_eq!(manager.snapshot(queued_id).unwrap().queue_position, Some(1));

    manager.pause_for_window("main", queued_id).unwrap();
    assert_eq!(manager.snapshot(queued_id).unwrap().state, "paused");
    assert_eq!(manager.snapshot(queued_id).unwrap().queue_position, None);
    manager.resume_for_window("main", queued_id).unwrap();
    assert_eq!(manager.snapshot(queued_id).unwrap().state, "queued");
    assert_eq!(manager.snapshot(queued_id).unwrap().queue_position, Some(1));
    assert_eq!(manager.queue.state(queue_id), Some(JobState::Queued));
    manager.pause_for_window("main", queued_id).unwrap();
    assert_eq!(manager.snapshot(queued_id).unwrap().state, "paused");
    manager.cancel_for_window("main", blocker_id).unwrap();
    wait_for_state(
        &sink,
        blocker_id,
        "cancelled",
        std::time::Duration::from_secs(2),
    );
    std::thread::sleep(std::time::Duration::from_millis(50));
    assert_eq!(manager.queue.state(queue_id), Some(JobState::Queued));
    assert_eq!(manager.snapshot(queued_id).unwrap().state, "paused");
    assert!(!states_of(&sink.events.lock().unwrap(), queued_id)
        .iter()
        .any(|state| state == "running"));

    manager.resume_for_window("main", queued_id).unwrap();
    wait_for_state(&sink, queued_id, "done", std::time::Duration::from_secs(2));
    manager.wait_idle();
    let observed = states_of(&sink.events.lock().unwrap(), queued_id);
    assert!(
        observed == vec!["queued", "paused", "queued", "paused", "running", "done"]
            || observed
                == vec!["queued", "paused", "queued", "paused", "queued", "running", "done"],
        "unexpected queued pause sequence: {observed:?}"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn core_terminal_failure_reconciles_a_nonterminal_snapshot() {
    let manager = JobManager::new();
    let sink = Arc::new(TestSink::default());
    let events: Arc<dyn EventSink> = sink.clone();
    let gui_id = 91;
    lock_unpoisoned(&manager.snapshots).insert(
        gui_id,
        Some("main".into()),
        checksum_job(Path::new("panic-input")),
        "running",
    );
    let queue_id = manager.queue.submit(Box::new(|_ctl, _progress| {
        panic!("job panic fixture");
    }));
    lock_unpoisoned(&manager.registry).jobs.insert(
        gui_id,
        ManagedJob {
            queue_id,
            cancel_flag: Arc::new(AtomicBool::new(false)),
            owner_window: Some("main".into()),
            events,
            pausable: true,
        },
    );

    manager.wait_idle();

    let snapshot = manager.snapshot_for_window("main", gui_id).unwrap();
    assert_eq!(snapshot.state, "failed");
    assert_eq!(snapshot.error.unwrap().detail, "job panicked");
    assert_eq!(
        states_of(&sink.events.lock().unwrap(), gui_id),
        vec!["failed"]
    );
    assert!(manager.queue.state(queue_id).is_none());
    assert!(!lock_unpoisoned(&manager.registry)
        .jobs
        .contains_key(&gui_id));
}

fn poison_lock<T>(mutex: &std::sync::Mutex<T>) {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _guard = mutex.lock().unwrap();
        panic!("poison lock for regression coverage");
    }));
    assert!(result.is_err());
}

#[test]
fn job_manager_registry_recovers_after_poison() {
    let dir = temp_dir("map-poison");
    let src = dir.join("data");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("hello.txt"), b"hello poison").unwrap();
    let zip = dir.join("poison.zip");

    let manager = JobManager::new();
    poison_lock(&manager.registry);

    let state = Arc::new(AppState::new());
    let sink = Arc::new(TestSink::default());
    let events: Arc<dyn EventSink> = sink.clone();
    let id = manager.submit(
        Arc::clone(&state),
        Arc::clone(&events),
        JobSpec::Compress {
            inputs: vec![src.to_string_lossy().into_owned()],
            dest: zip.to_string_lossy().into_owned(),
            level: 5,
            password: None,
            encrypt_names: false,
            split_size: None,
            split_mode: squallz_core::api::SplitOutputMode::Generic,
            excludes: vec![],
            content_policy: squallz_core::CreateContentPolicy::KeepAllFiles,
            sqz_inner_format: None,
            sfx_target: None,
            completion: squallz_core::CreateCompletionAction::None,
            post_success: PostSuccessAction::KeepSource,
            test_after_create: false,
            replace_existing: false,
            replacement_guard: None,
        },
        SettingsDto::default(),
    );
    manager.wait_idle();

    assert_eq!(manager.snapshot(id).unwrap().state, "done");
    assert!(manager.pause_for_window("main", id).is_err());
    assert!(manager.resume_for_window("main", id).is_err());
    assert!(manager.cancel_for_window("main", id).is_err());
    let events = sink.events.lock().unwrap();
    assert_eq!(states_of(&events, id), vec!["queued", "running", "done"]);

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn partial_source_cleanup_keeps_successful_archive_job_done() {
    let dir = temp_dir("source-cleanup-partial");
    let first = dir.join("first.txt");
    let second = dir.join("second.txt");
    let archive = dir.join("archive.zip");
    std::fs::write(&first, b"first").unwrap();
    std::fs::write(&second, b"second").unwrap();
    let fake = Arc::new(FakeTrashAdapter::failing(&["second.txt"]));
    let manager =
        JobManager::with_test_trash_adapter(Arc::new(OperationAudit::memory()), fake.clone());
    let state = Arc::new(AppState::new());
    let sink = Arc::new(TestSink::default());
    let events: Arc<dyn EventSink> = sink;

    let id = manager.submit(
        state,
        events,
        JobSpec::Compress {
            inputs: vec![
                first.to_string_lossy().into_owned(),
                second.to_string_lossy().into_owned(),
            ],
            dest: archive.to_string_lossy().into_owned(),
            level: 5,
            password: None,
            encrypt_names: false,
            split_size: None,
            split_mode: squallz_core::api::SplitOutputMode::Generic,
            excludes: vec![],
            content_policy: squallz_core::CreateContentPolicy::KeepAllFiles,
            sqz_inner_format: None,
            sfx_target: None,
            completion: squallz_core::CreateCompletionAction::None,
            post_success: PostSuccessAction::TrashSource,
            test_after_create: false,
            replace_existing: false,
            replacement_guard: None,
        },
        SettingsDto::default(),
    );
    manager.wait_idle();

    let snapshot = manager.snapshot(id).unwrap();
    assert_eq!(snapshot.state, "done");
    assert!(archive.exists());
    let result = snapshot.result.as_ref().unwrap();
    assert_eq!(result["tested_after_create"], true);
    assert_eq!(result["entries_tested_after_create"], 2);
    let cleanup = &result["source_cleanup"];
    assert_eq!(cleanup["status"], "partial");
    assert_eq!(cleanup["moved"], 1);
    assert_eq!(cleanup["kept"], 1);
    assert_eq!(cleanup["recovery_required"], 1);
    assert!(!cleanup.to_string().contains(dir.to_string_lossy().as_ref()));
    assert_eq!(fake.calls().len(), 2);
    assert!(second.exists());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn failed_source_cleanup_keeps_successful_archive_job_done() {
    let dir = temp_dir("source-cleanup-worker-failed");
    let source = dir.join("source.txt");
    let archive = dir.join("archive.zip");
    std::fs::write(&source, b"source").unwrap();
    let fake = Arc::new(FakeTrashAdapter::failing(&["source.txt"]));
    let manager =
        JobManager::with_test_trash_adapter(Arc::new(OperationAudit::memory()), fake.clone());
    let state = Arc::new(AppState::new());
    let sink = Arc::new(TestSink::default());
    let events: Arc<dyn EventSink> = sink;

    let id = manager.submit(
        state,
        events,
        JobSpec::Compress {
            inputs: vec![source.to_string_lossy().into_owned()],
            dest: archive.to_string_lossy().into_owned(),
            level: 5,
            password: None,
            encrypt_names: false,
            split_size: None,
            split_mode: squallz_core::api::SplitOutputMode::Generic,
            excludes: vec![],
            content_policy: squallz_core::CreateContentPolicy::KeepAllFiles,
            sqz_inner_format: None,
            sfx_target: None,
            completion: squallz_core::CreateCompletionAction::None,
            post_success: PostSuccessAction::TrashSource,
            test_after_create: false,
            replace_existing: false,
            replacement_guard: None,
        },
        SettingsDto::default(),
    );
    manager.wait_idle();

    let snapshot = manager.snapshot(id).unwrap();
    assert_eq!(snapshot.state, "done");
    assert!(archive.exists());
    let result = snapshot.result.as_ref().unwrap();
    assert_eq!(result["tested_after_create"], true);
    assert_eq!(result["entries_tested_after_create"], 1);
    let cleanup = &result["source_cleanup"];
    assert_eq!(cleanup["status"], "failed");
    assert_eq!(cleanup["moved"], 0);
    assert_eq!(cleanup["kept"], 1);
    assert_eq!(cleanup["recovery_required"], 1);
    assert!(!cleanup.to_string().contains(dir.to_string_lossy().as_ref()));
    assert_eq!(fake.calls().len(), 1);
    assert!(source.exists());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn create_worker_refuses_existing_destination_and_split_family() {
    let dir = temp_dir("create-destination-race");
    let source = dir.join("source.txt");
    let archive = dir.join("archive.zip");
    let split_archive = dir.join("split.zip");
    let late_volume = dir.join("split.zip.1000");
    std::fs::write(&source, b"source").unwrap();
    std::fs::write(&archive, b"do not replace").unwrap();
    std::fs::write(&late_volume, b"do not replace volume").unwrap();
    let manager = JobManager::new();
    let state = Arc::new(AppState::new());
    let sink = Arc::new(TestSink::default());
    let events: Arc<dyn EventSink> = sink;

    let existing_id = manager.submit(
        Arc::clone(&state),
        Arc::clone(&events),
        JobSpec::Compress {
            inputs: vec![source.to_string_lossy().into_owned()],
            dest: archive.to_string_lossy().into_owned(),
            level: 5,
            password: None,
            encrypt_names: false,
            split_size: None,
            split_mode: squallz_core::api::SplitOutputMode::Generic,
            excludes: vec![],
            content_policy: squallz_core::CreateContentPolicy::KeepAllFiles,
            sqz_inner_format: None,
            sfx_target: None,
            completion: squallz_core::CreateCompletionAction::None,
            post_success: PostSuccessAction::KeepSource,
            test_after_create: false,
            replace_existing: false,
            replacement_guard: None,
        },
        SettingsDto::default(),
    );
    let split_id = manager.submit(
        Arc::clone(&state),
        Arc::clone(&events),
        JobSpec::Compress {
            inputs: vec![source.to_string_lossy().into_owned()],
            dest: split_archive.to_string_lossy().into_owned(),
            level: 5,
            password: None,
            encrypt_names: false,
            split_size: Some(8 * 1024),
            split_mode: squallz_core::api::SplitOutputMode::Generic,
            excludes: vec![],
            content_policy: squallz_core::CreateContentPolicy::KeepAllFiles,
            sqz_inner_format: None,
            sfx_target: None,
            completion: squallz_core::CreateCompletionAction::None,
            post_success: PostSuccessAction::KeepSource,
            test_after_create: false,
            replace_existing: false,
            replacement_guard: None,
        },
        SettingsDto::default(),
    );
    manager.wait_idle();

    assert_eq!(manager.snapshot(existing_id).unwrap().state, "failed");
    assert_eq!(manager.snapshot(split_id).unwrap().state, "failed");
    assert_eq!(std::fs::read(&archive).unwrap(), b"do not replace");
    assert_eq!(
        std::fs::read(&late_volume).unwrap(),
        b"do not replace volume"
    );
    assert!(!split_archive.exists());

    let replacement_guard =
        squallz_core::inspect_create_destination(&archive, CreateArtifactKind::Archive)
            .unwrap()
            .guard
            .unwrap();
    let replace_id = manager.submit(
        state,
        events,
        JobSpec::Compress {
            inputs: vec![source.to_string_lossy().into_owned()],
            dest: archive.to_string_lossy().into_owned(),
            level: 5,
            password: None,
            encrypt_names: false,
            split_size: None,
            split_mode: squallz_core::api::SplitOutputMode::Generic,
            excludes: vec![],
            content_policy: squallz_core::CreateContentPolicy::KeepAllFiles,
            sqz_inner_format: None,
            sfx_target: None,
            completion: squallz_core::CreateCompletionAction::None,
            post_success: PostSuccessAction::KeepSource,
            test_after_create: false,
            replace_existing: true,
            replacement_guard: Some(replacement_guard),
        },
        SettingsDto::default(),
    );
    manager.wait_idle();

    assert_eq!(manager.snapshot(replace_id).unwrap().state, "done");
    assert_ne!(std::fs::read(&archive).unwrap(), b"do not replace");
    std::fs::remove_dir_all(dir).unwrap();
}

/// A compress job followed by an extract job runs end to end through
/// the queue, emitting queued → running → done with progress events.
#[test]
fn compress_then_extract_round_trip() {
    let dir = temp_dir("roundtrip");
    let src = dir.join("data");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("hello.txt"), b"hello squallz").unwrap();
    let zip = dir.join("out.zip");

    let manager = JobManager::new();
    let state = Arc::new(AppState::new());
    let sink = Arc::new(TestSink::default());
    let events: Arc<dyn EventSink> = sink.clone();

    let id1 = manager.submit(
        Arc::clone(&state),
        Arc::clone(&events),
        JobSpec::Compress {
            inputs: vec![src.to_string_lossy().into_owned()],
            dest: zip.to_string_lossy().into_owned(),
            level: 5,
            password: None,
            encrypt_names: false,
            split_size: None,
            split_mode: squallz_core::api::SplitOutputMode::Generic,
            excludes: vec![],
            content_policy: squallz_core::CreateContentPolicy::KeepAllFiles,
            sqz_inner_format: None,
            sfx_target: None,
            completion: squallz_core::CreateCompletionAction::None,
            post_success: PostSuccessAction::KeepSource,
            test_after_create: false,
            replace_existing: false,
            replacement_guard: None,
        },
        SettingsDto::default(),
    );
    let out = dir.join("out");
    let id2 = manager.submit(
        Arc::clone(&state),
        Arc::clone(&events),
        JobSpec::Extract {
            path: zip.to_string_lossy().into_owned(),
            dest: out.to_string_lossy().into_owned(),
            expected_destination: None,
            expected_input_guard: None,
            selection: None,
            overwrite: squallz_core::api::OverwritePolicy::Skip,
            symlinks: squallz_core::api::SymlinkPolicy::Preserve,
            smart: false,
            encoding: None,
            password: None,
            verify_sfx: false,
            best_effort: false,
        },
        SettingsDto::default(),
    );
    manager.wait_idle();

    assert!(out.join("data/hello.txt").exists());
    let create_snapshot = manager.snapshot(id1).unwrap();
    assert_eq!(create_snapshot.state, "done");
    let create_result = create_snapshot.result.as_ref().unwrap();
    assert_eq!(create_result["operation"], "create");
    assert_eq!(
        create_result["primary_output"].as_str(),
        Some(zip.to_string_lossy().as_ref())
    );
    assert_eq!(create_result["outputs"].as_array().map(Vec::len), Some(1));
    assert_eq!(create_result["volume_count"], 1);
    assert_eq!(create_result["split"], false);
    assert_eq!(create_result["source_cleanup"]["status"], "not_requested");
    assert_eq!(create_result["source_cleanup"]["moved"], 0);
    assert_eq!(create_result["source_cleanup"]["kept"], 1);
    assert_eq!(
        create_result["total_bytes"].as_u64(),
        Some(std::fs::metadata(&zip).unwrap().len())
    );
    let snapshot = manager.snapshot(id2).unwrap();
    assert_eq!(snapshot.state, "done");
    let extract_result = snapshot.result.as_ref().unwrap();
    assert_eq!(
        extract_result.get("dest").and_then(|dest| dest.as_str()),
        Some(out.to_string_lossy().as_ref())
    );
    assert_eq!(
        extract_result["plan"]["destination"],
        extract_result["dest"]
    );
    assert_eq!(extract_result["plan"]["layout"], "direct");
    assert_eq!(extract_result["plan"]["entries"], 2);
    assert_eq!(extract_result["counts"]["selected_entries"], 2);
    assert_eq!(extract_result["counts"]["created"], 1);
    assert_eq!(extract_result["counts"]["directories"], 1);
    assert_eq!(extract_result["counts"]["skipped"], 0);
    assert_eq!(extract_result["counts"]["replaced"], 0);
    assert_eq!(extract_result["counts"]["renamed"], 0);
    assert_eq!(extract_result["counts"]["failed"], 0);
    assert_eq!(extract_result["counts"]["output_bytes"], 13);
    let events = sink.events.lock().unwrap();
    assert_eq!(states_of(&events, id1), vec!["queued", "running", "done"]);
    assert_eq!(states_of(&events, id2), vec!["queued", "running", "done"]);
    assert_eq!(done_result(&events, id1).as_ref(), Some(create_result));
    assert_real_current_file_progress(&events, id1, "compress");
    assert_real_current_file_progress(&events, id2, "extract");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn extract_job_rejects_a_stale_expected_destination_before_writing() {
    let dir = temp_dir("extract-stale-destination");
    let source = dir.join("data");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::write(source.join("hello.txt"), b"hello squallz").unwrap();
    let archive = dir.join("archive.zip");
    let state = Arc::new(AppState::new());
    state
        .engine
        .create(
            &archive,
            std::slice::from_ref(&source),
            &CreateOptions::default(),
            &squallz_core::api::NoProgress,
            &ControlToken::new(),
        )
        .unwrap();
    let destination = dir.join("output");
    let stale_destination = dir.join("previous-preview");
    let manager = JobManager::new();
    let sink = Arc::new(TestSink::default());
    let events: Arc<dyn EventSink> = sink.clone();

    let id = manager.submit(
        state,
        events,
        JobSpec::Extract {
            path: archive.to_string_lossy().into_owned(),
            dest: destination.to_string_lossy().into_owned(),
            expected_destination: Some(stale_destination.to_string_lossy().into_owned()),
            expected_input_guard: None,
            selection: None,
            overwrite: squallz_core::api::OverwritePolicy::Skip,
            symlinks: squallz_core::api::SymlinkPolicy::Preserve,
            smart: false,
            encoding: None,
            password: None,
            verify_sfx: false,
            best_effort: false,
        },
        SettingsDto::default(),
    );
    manager.wait_idle();

    let events = sink.events.lock().unwrap();
    let failed = events
        .iter()
        .find(|(name, payload)| {
            name == EV_STATE && payload["id"] == id && payload["state"] == "failed"
        })
        .expect("stale extraction plan must fail");
    assert_eq!(
        failed.1["error"]["key"].as_str(),
        Some("error.destination_changed")
    );
    assert!(!destination.exists());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn extract_job_rejects_an_archive_replaced_after_preflight() {
    let dir = temp_dir("extract-stale-input");
    let original_source = dir.join("original");
    std::fs::create_dir_all(&original_source).unwrap();
    std::fs::write(original_source.join("hello.txt"), b"original payload").unwrap();
    let archive = dir.join("archive.zip");
    let state = Arc::new(AppState::new());
    let control = ControlToken::new();
    state
        .engine
        .create(
            &archive,
            std::slice::from_ref(&original_source),
            &CreateOptions::default(),
            &squallz_core::api::NoProgress,
            &control,
        )
        .unwrap();
    let destination = dir.join("output");
    let (_, _, input_guard) = state
        .engine
        .plan_extract_with_input_guard_controlled(
            &archive,
            &destination,
            &archive,
            false,
            &OpenOptions::default(),
            &control,
            |_, _| Ok(None),
        )
        .unwrap();

    let replacement_source = dir.join("replacement");
    std::fs::create_dir_all(&replacement_source).unwrap();
    std::fs::write(
        replacement_source.join("different.txt"),
        b"replacement payload",
    )
    .unwrap();
    let replacement_archive = dir.join("replacement.zip");
    state
        .engine
        .create(
            &replacement_archive,
            &[replacement_source],
            &CreateOptions::default(),
            &squallz_core::api::NoProgress,
            &control,
        )
        .unwrap();
    std::fs::remove_file(&archive).unwrap();
    std::fs::rename(&replacement_archive, &archive).unwrap();

    let manager = JobManager::new();
    let sink = Arc::new(TestSink::default());
    let events: Arc<dyn EventSink> = sink.clone();
    let id = manager.submit(
        state,
        events,
        JobSpec::Extract {
            path: archive.to_string_lossy().into_owned(),
            dest: destination.to_string_lossy().into_owned(),
            expected_destination: Some(destination.to_string_lossy().into_owned()),
            expected_input_guard: Some(input_guard),
            selection: None,
            overwrite: squallz_core::api::OverwritePolicy::Skip,
            symlinks: squallz_core::api::SymlinkPolicy::Preserve,
            smart: false,
            encoding: None,
            password: None,
            verify_sfx: false,
            best_effort: false,
        },
        SettingsDto::default(),
    );
    manager.wait_idle();

    let events = sink.events.lock().unwrap();
    let failed = events
        .iter()
        .find(|(name, payload)| {
            name == EV_STATE && payload["id"] == id && payload["state"] == "failed"
        })
        .expect("stale extraction input must fail");
    assert_eq!(
        failed.1["error"]["key"].as_str(),
        Some("error.input_changed")
    );
    assert!(!destination.exists());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn split_compress_job_reports_the_committed_output_set() {
    let dir = temp_dir("split-create-report");
    let src = dir.join("payload.bin");
    std::fs::write(&src, deterministic_payload(96 * 1024)).unwrap();
    let archive = dir.join("parts.zip");

    let manager = JobManager::new();
    let state = Arc::new(AppState::new());
    let sink = Arc::new(TestSink::default());
    let events: Arc<dyn EventSink> = sink.clone();
    let id = manager.submit(
        state,
        events,
        JobSpec::Compress {
            inputs: vec![src.to_string_lossy().into_owned()],
            dest: archive.to_string_lossy().into_owned(),
            level: 1,
            password: None,
            encrypt_names: false,
            split_size: Some(8 * 1024),
            split_mode: squallz_core::api::SplitOutputMode::Generic,
            excludes: vec![],
            content_policy: squallz_core::CreateContentPolicy::KeepAllFiles,
            sqz_inner_format: None,
            sfx_target: None,
            completion: squallz_core::CreateCompletionAction::None,
            post_success: PostSuccessAction::KeepSource,
            test_after_create: true,
            replace_existing: false,
            replacement_guard: None,
        },
        SettingsDto::default(),
    );
    manager.wait_idle();

    let snapshot = manager.snapshot(id).unwrap();
    assert_eq!(snapshot.state, "done");
    let result = snapshot.result.as_ref().unwrap();
    assert_eq!(result["operation"], "create");
    assert_eq!(result["split"], true);
    assert_eq!(result["tested_after_create"], true);
    assert!(result["entries_tested_after_create"]
        .as_u64()
        .is_some_and(|entries| entries > 0));
    let outputs = result["outputs"].as_array().unwrap();
    let volume_count = result["volume_count"].as_u64().unwrap() as usize;
    assert!(volume_count > 1);
    assert_eq!(outputs.len(), volume_count);
    assert_eq!(
        result["primary_output"].as_str(),
        outputs.first().and_then(|path| path.as_str())
    );
    assert!(result["primary_output"]
        .as_str()
        .is_some_and(|path| path.ends_with("parts.zip.001")));
    assert!(!archive.exists());

    let mut actual_total = 0_u64;
    for output in outputs {
        let path = output.as_str().unwrap();
        let metadata = std::fs::metadata(path).unwrap();
        assert!(metadata.is_file());
        actual_total = actual_total.saturating_add(metadata.len());
    }
    assert_eq!(result["total_bytes"].as_u64(), Some(actual_total));

    let events = sink.events.lock().unwrap();
    assert_eq!(done_result(&events, id).as_ref(), Some(result));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn compress_job_forwards_the_sqz_inner_format() {
    let spec = JobSpec::Compress {
        inputs: vec!["data".into()],
        dest: "profile.sqz".into(),
        level: 5,
        password: None,
        encrypt_names: false,
        split_size: None,
        split_mode: squallz_core::api::SplitOutputMode::Generic,
        excludes: vec![],
        content_policy: squallz_core::CreateContentPolicy::KeepAllFiles,
        sqz_inner_format: Some(squallz_core::SqzInnerFormat::Zip),
        sfx_target: None,
        completion: squallz_core::CreateCompletionAction::None,
        post_success: PostSuccessAction::KeepSource,
        test_after_create: false,
        replace_existing: false,
        replacement_guard: None,
    };

    let request = create_job_request(&spec, &SettingsDto::default()).unwrap();
    assert_eq!(
        request.options.sqz.inner_format,
        squallz_core::SqzInnerFormat::Zip
    );
}

#[cfg(target_os = "macos")]
#[test]
fn create_sfx_job_uses_the_shared_queue_and_core() {
    let dir = temp_dir("create-sfx");
    let src = dir.join("notes");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("hello.txt"), b"hello from the GUI job").unwrap();
    let template = dir.join("Squallz.app");
    write_macos_sfx_template(&template);
    let output = dir.join("Notes.app");
    let audit = Arc::new(OperationAudit::memory());
    let manager = JobManager::with_test_sfx_template(audit, template);
    let state = Arc::new(AppState::new());
    let sink = Arc::new(TestSink::default());
    let events: Arc<dyn EventSink> = sink.clone();

    let id = manager.submit(
        Arc::clone(&state),
        Arc::clone(&events),
        JobSpec::Compress {
            inputs: vec![src.to_string_lossy().into_owned()],
            dest: output.to_string_lossy().into_owned(),
            level: 5,
            password: None,
            encrypt_names: false,
            split_size: None,
            split_mode: squallz_core::api::SplitOutputMode::Generic,
            excludes: vec![],
            content_policy: squallz_core::CreateContentPolicy::KeepAllFiles,
            sqz_inner_format: None,
            sfx_target: Some(SfxTarget::Macos),
            completion: squallz_core::CreateCompletionAction::None,
            post_success: PostSuccessAction::KeepSource,
            test_after_create: true,
            replace_existing: false,
            replacement_guard: None,
        },
        SettingsDto::default(),
    );
    manager.wait_idle();

    let snapshot = manager.snapshot(id).unwrap();
    assert_eq!(snapshot.state, "done");
    assert_eq!(
        snapshot
            .result
            .as_ref()
            .and_then(|result| result.get("operation"))
            .and_then(|value| value.as_str()),
        Some("create_sfx")
    );
    let result = snapshot.result.as_ref().unwrap();
    assert_eq!(result["source_cleanup"]["status"], "not_requested");
    assert_eq!(
        result["primary_output"].as_str(),
        Some(output.to_string_lossy().as_ref())
    );
    assert_eq!(result["outputs"].as_array().map(Vec::len), Some(1));
    assert_eq!(result["volume_count"], 1);
    assert_eq!(result["split"], false);
    assert_eq!(result["tested_after_create"], true);
    assert!(result["entries_tested_after_create"]
        .as_u64()
        .is_some_and(|entries| entries > 0));
    assert!(result["total_bytes"]
        .as_u64()
        .is_some_and(|bytes| bytes > 0));
    assert_eq!(result["requires_signing"], true);
    assert!(output
        .join("Contents/Resources/squallz-sfx/payload.zip")
        .exists());
    let entries = state.engine.list(&output, &OpenOptions::default()).unwrap();
    assert_eq!(entries[0].path.display, "notes/");
    let events = sink.events.lock().unwrap();
    assert_eq!(states_of(&events, id), vec!["queued", "running", "done"]);
    assert_eq!(done_result(&events, id).as_ref(), Some(result));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn batch_extract_runs_multiple_archives_as_one_job() {
    let dir = temp_dir("batch-extract");
    let src_a = dir.join("alpha");
    let src_b = dir.join("bravo");
    std::fs::create_dir_all(&src_a).unwrap();
    std::fs::create_dir_all(&src_b).unwrap();
    std::fs::write(src_a.join("one.txt"), b"alpha one").unwrap();
    std::fs::write(src_b.join("two.txt"), b"bravo two").unwrap();
    let zip_a = dir.join("alpha.zip");
    let zip_b = dir.join("bravo.zip");
    let state = Arc::new(AppState::new());
    state
        .engine
        .create(
            &zip_a,
            std::slice::from_ref(&src_a),
            &CreateOptions {
                level: CompressionLevel::Fastest,
                ..CreateOptions::default()
            },
            &squallz_core::api::NoProgress,
            &ControlToken::new(),
        )
        .unwrap();
    state
        .engine
        .create(
            &zip_b,
            std::slice::from_ref(&src_b),
            &CreateOptions {
                level: CompressionLevel::Fastest,
                ..CreateOptions::default()
            },
            &squallz_core::api::NoProgress,
            &ControlToken::new(),
        )
        .unwrap();
    let mut zip_b_bytes = std::fs::read(&zip_b).unwrap();
    let central_start = zip_b_bytes
        .windows(4)
        .position(|window| window == b"PK\x01\x02")
        .expect("central directory exists in sample");
    zip_b_bytes.truncate(central_start);
    std::fs::write(&zip_b, zip_b_bytes).unwrap();

    let manager = JobManager::new();
    let sink = Arc::new(TestSink::default());
    let events: Arc<dyn EventSink> = sink.clone();
    let out_a = dir.join("out-a");
    let out_b = dir.join("out-b");
    let id = manager.submit(
        Arc::clone(&state),
        Arc::clone(&events),
        JobSpec::BatchExtract {
            items: vec![
                BatchExtractItem {
                    path: zip_a.to_string_lossy().into_owned(),
                    dest: out_a.to_string_lossy().into_owned(),
                    encoding: None,
                    password: None,
                    best_effort: false,
                },
                BatchExtractItem {
                    path: zip_b.to_string_lossy().into_owned(),
                    dest: out_b.to_string_lossy().into_owned(),
                    encoding: None,
                    password: None,
                    best_effort: false,
                },
            ],
            overwrite: squallz_core::api::OverwritePolicy::Skip,
            symlinks: squallz_core::api::SymlinkPolicy::Preserve,
            smart: false,
        },
        SettingsDto::default(),
    );
    manager.wait_idle();

    assert_eq!(
        std::fs::read(out_a.join("alpha/one.txt")).unwrap(),
        b"alpha one"
    );
    assert_eq!(
        std::fs::read(out_b.join("bravo/two.txt")).unwrap(),
        b"bravo two"
    );
    let events = sink.events.lock().unwrap().clone();
    assert_eq!(states_of(&events, id), vec!["queued", "running", "done"]);
    let result = done_result(&events, id).unwrap();
    assert_eq!(result["operation"], "batch_extract");
    assert_eq!(result["archives"], 2);
    assert_eq!(result["extracted"], 2);
    assert_eq!(result["failed"], 0);
    assert_eq!(result["outputs"].as_array().unwrap().len(), 2);
    assert_eq!(result["outputs"][0]["plan"]["layout"], "direct");
    assert_eq!(result["outputs"][0]["counts"]["created"], 1);
    assert_eq!(result["outputs"][0]["counts"]["failed"], 0);
    assert!(result["outputs"][0].get("structure").is_none());
    assert_eq!(
        result["outputs"][1]["structure"],
        "zip_local_headers_recovered"
    );
    assert_eq!(result["structure"], "zip_local_headers_recovered");
    assert_eq!(result["recovered_archives"], 1);
    assert!(events.iter().any(|(name, payload)| name == EV_PROGRESS
        && payload["id"] == id
        && payload["total"].as_u64().unwrap_or(0) == 2 * BATCH_PROGRESS_SCALE));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn extract_job_reports_recovered_zip_structure() {
    let dir = temp_dir("extract-recovered-zip-structure");
    let archive = dir.join("missing-central-directory.zip");
    let destination = dir.join("output");
    let mut bytes = build_stored_zip(&[(b"recoverable.txt", b"recoverable payload")]);
    let central_start = bytes
        .windows(4)
        .position(|window| window == b"PK\x01\x02")
        .expect("central directory exists in sample");
    bytes.truncate(central_start);
    std::fs::write(&archive, bytes).unwrap();

    let manager = JobManager::new();
    let state = Arc::new(AppState::new());
    let sink = Arc::new(TestSink::default());
    let events: Arc<dyn EventSink> = sink.clone();
    let id = manager.submit(
        Arc::clone(&state),
        Arc::clone(&events),
        JobSpec::Extract {
            path: archive.to_string_lossy().into_owned(),
            dest: destination.to_string_lossy().into_owned(),
            expected_destination: None,
            expected_input_guard: None,
            selection: None,
            overwrite: squallz_core::api::OverwritePolicy::Skip,
            symlinks: squallz_core::api::SymlinkPolicy::Preserve,
            smart: false,
            encoding: None,
            password: None,
            verify_sfx: false,
            best_effort: false,
        },
        SettingsDto::default(),
    );
    manager.wait_idle();

    assert_eq!(
        std::fs::read(destination.join("recoverable.txt")).unwrap(),
        b"recoverable payload"
    );
    let events = sink.events.lock().unwrap().clone();
    assert_eq!(states_of(&events, id), vec!["queued", "running", "done"]);
    let result = done_result(&events, id).unwrap();
    assert_eq!(result["structure"], "zip_local_headers_recovered");
    assert_eq!(result["counts"]["created"], 1);
    assert_eq!(result["counts"]["failed"], 0);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[cfg(unix)]
#[test]
fn native_rar_family_is_listed_and_extracted_once_when_all_volumes_are_selected() {
    use std::os::unix::fs::PermissionsExt;

    let _guard = EXTERNAL_TOOL_ENV_LOCK.lock().unwrap();
    let dir = temp_dir("batch-native-rar");
    let first = dir.join("sample.part1.rar");
    let second = dir.join("sample.part2.rar");
    let tool = dir.join("fake-7z.sh");
    let log = dir.join("fake-7z.log");
    std::fs::write(&first, rar5_test_volume(0, true)).unwrap();
    std::fs::write(&second, rar5_test_volume(1, false)).unwrap();
    std::fs::write(
        &tool,
        r#"#!/bin/sh
set -eu
printf '%s\n' "$*" >> "$SQUALLZ_FAKE_7Z_LOG"
case "$1" in
  l)
cat <<'EOF'
Path = hello.txt
Folder = -
Size = 5
Packed Size = 5
CRC = 3610A686
Encrypted = -

EOF
;;
  x)
printf 'hello'
;;
  *)
exit 64
;;
esac
"#,
    )
    .unwrap();
    let mut permissions = std::fs::metadata(&tool).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&tool, permissions).unwrap();
    let _tool_env = EnvRestore::set("SQUALLZ_7Z", &tool);
    let _log_env = EnvRestore::set("SQUALLZ_FAKE_7Z_LOG", &log);

    let state = Arc::new(AppState::new());
    let info = state.open_archive(&second, None, None).unwrap();
    assert_eq!(
        info.volumes,
        Some(vec![
            "sample.part1.rar".to_owned(),
            "sample.part2.rar".to_owned()
        ])
    );
    state.close_archive(info.id);
    std::fs::write(&log, b"").unwrap();

    let manager = JobManager::new();
    let sink = Arc::new(TestSink::default());
    let events: Arc<dyn EventSink> = sink.clone();
    let out_first = dir.join("out-first");
    let out_second = dir.join("out-second");
    let id = manager.submit(
        Arc::clone(&state),
        Arc::clone(&events),
        JobSpec::BatchExtract {
            items: vec![
                BatchExtractItem {
                    path: second.to_string_lossy().into_owned(),
                    dest: out_second.to_string_lossy().into_owned(),
                    encoding: None,
                    password: None,
                    best_effort: false,
                },
                BatchExtractItem {
                    path: first.to_string_lossy().into_owned(),
                    dest: out_first.to_string_lossy().into_owned(),
                    encoding: None,
                    password: None,
                    best_effort: false,
                },
            ],
            overwrite: squallz_core::api::OverwritePolicy::Skip,
            symlinks: squallz_core::api::SymlinkPolicy::Skip,
            smart: false,
        },
        SettingsDto::default(),
    );
    manager.wait_idle();

    assert_eq!(
        std::fs::read(out_first.join("hello.txt")).unwrap(),
        b"hello"
    );
    assert!(!out_second.exists());
    let recorded_events = sink.events.lock().unwrap().clone();
    let result = done_result(&recorded_events, id).unwrap();
    assert_eq!(result["archives"], 1);
    assert_eq!(result["selected_archives"], 2);
    assert_eq!(result["collapsed_volumes"], 1);
    assert_eq!(result["extracted"], 1);
    assert_eq!(result["failed"], 0);
    assert!(recorded_events.iter().any(|(name, payload)| {
        name == EV_PROGRESS
            && payload["id"] == id
            && payload["total"].as_u64() == Some(BATCH_PROGRESS_SCALE)
    }));
    let tool_log = std::fs::read_to_string(&log).unwrap();
    assert_eq!(
        tool_log
            .lines()
            .filter(|line| line.starts_with("l -slt"))
            .count(),
        1
    );
    assert_eq!(
        tool_log
            .lines()
            .filter(|line| line.starts_with("x -so"))
            .count(),
        1
    );

    std::fs::remove_dir_all(dir).unwrap();
}

#[cfg(unix)]
#[test]
fn native_split_zip_family_opens_from_middle_and_extracts_once_via_primary() {
    use std::os::unix::fs::PermissionsExt;

    let _guard = EXTERNAL_TOOL_ENV_LOCK.lock().unwrap();
    let dir = temp_dir("batch-native-split-zip");
    let first = dir.join("sample.z01");
    let second = dir.join("sample.z02");
    let primary = dir.join("sample.zip");
    let tool = dir.join("fake-7z.sh");
    let log = dir.join("fake-7z.log");
    std::fs::write(&first, b"first split ZIP data volume").unwrap();
    std::fs::write(&second, b"second split ZIP data volume").unwrap();
    std::fs::write(&primary, split_zip_test_final_volume(2)).unwrap();
    std::fs::write(
        &tool,
        r#"#!/bin/sh
set -eu
printf '%s\n' "$*" >> "$SQUALLZ_FAKE_7Z_LOG"
archive="$3"
stage="$(dirname "$archive")"
test "$(basename "$archive")" = "archive.zip"
test -f "$stage/archive.z01"
test -f "$stage/archive.z02"
test -f "$stage/archive.zip"
case "$1" in
  l)
cat <<'EOF'
Path = hello.txt
Folder = -
Size = 5
Packed Size = 5
CRC = 3610A686
Encrypted = -

EOF
;;
  x)
printf 'hello'
;;
  *)
exit 64
;;
esac
"#,
    )
    .unwrap();
    let mut permissions = std::fs::metadata(&tool).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&tool, permissions).unwrap();
    let _tool_env = EnvRestore::set("SQUALLZ_7Z", &tool);
    let _log_env = EnvRestore::set("SQUALLZ_FAKE_7Z_LOG", &log);

    let state = Arc::new(AppState::new());
    let info = state.open_archive(&second, None, None).unwrap();
    assert_eq!(
        info.volumes,
        Some(vec![
            "sample.z01".to_owned(),
            "sample.z02".to_owned(),
            "sample.zip".to_owned(),
        ])
    );
    state.close_archive(info.id);
    std::fs::write(&log, b"").unwrap();

    let manager = JobManager::new();
    let sink = Arc::new(TestSink::default());
    let events: Arc<dyn EventSink> = sink.clone();
    let out_first = dir.join("out-first");
    let out_second = dir.join("out-second");
    let out_primary = dir.join("out-primary");
    let id = manager.submit(
        Arc::clone(&state),
        Arc::clone(&events),
        JobSpec::BatchExtract {
            items: vec![
                BatchExtractItem {
                    path: second.to_string_lossy().into_owned(),
                    dest: out_second.to_string_lossy().into_owned(),
                    encoding: None,
                    password: None,
                    best_effort: false,
                },
                BatchExtractItem {
                    path: first.to_string_lossy().into_owned(),
                    dest: out_first.to_string_lossy().into_owned(),
                    encoding: None,
                    password: None,
                    best_effort: false,
                },
                BatchExtractItem {
                    path: primary.to_string_lossy().into_owned(),
                    dest: out_primary.to_string_lossy().into_owned(),
                    encoding: None,
                    password: None,
                    best_effort: false,
                },
            ],
            overwrite: squallz_core::api::OverwritePolicy::Skip,
            symlinks: squallz_core::api::SymlinkPolicy::Skip,
            smart: false,
        },
        SettingsDto::default(),
    );
    manager.wait_idle();

    assert_eq!(
        std::fs::read(out_primary.join("hello.txt")).unwrap(),
        b"hello"
    );
    assert!(!out_first.exists());
    assert!(!out_second.exists());
    let recorded_events = sink.events.lock().unwrap().clone();
    let result = done_result(&recorded_events, id).unwrap();
    assert_eq!(result["archives"], 1);
    assert_eq!(result["selected_archives"], 3);
    assert_eq!(result["collapsed_volumes"], 2);
    assert_eq!(result["extracted"], 1);
    assert_eq!(result["failed"], 0);
    let tool_log = std::fs::read_to_string(&log).unwrap();
    assert_eq!(
        tool_log
            .lines()
            .filter(|line| line.starts_with("l -slt"))
            .count(),
        1
    );
    assert_eq!(
        tool_log
            .lines()
            .filter(|line| line.starts_with("x -so"))
            .count(),
        1
    );

    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn cancel_password_prompt_reports_cancelled_without_poll_delay() {
    let dir = temp_dir("cancel-password-latency");
    let state = Arc::new(AppState::new());
    let archive = create_password_protected_zip(&dir, &state);

    let manager = JobManager::new();
    let sink = Arc::new(TestSink::default());
    let events: Arc<dyn EventSink> = sink.clone();
    let id = manager.submit(
        Arc::clone(&state),
        Arc::clone(&events),
        password_test_job(&archive),
        SettingsDto::default(),
    );

    wait_for_password_prompt(&sink, id);
    let cancel_start = Instant::now();
    manager.cancel_for_window("main", id).unwrap();
    wait_for_state(&sink, id, "cancelled", std::time::Duration::from_secs(2));
    let cancel_ms = cancel_start.elapsed().as_millis();
    println!("JOB_METRIC gui_cancel_prompt_to_cancelled_ms={cancel_ms}");
    assert!(
        cancel_ms <= 120,
        "password-prompt cancel took {cancel_ms}ms; expected sub-120ms state feedback"
    );

    manager.wait_idle();
    let events = sink.events.lock().unwrap();
    assert!(states_of(&events, id).contains(&"cancelled".to_owned()));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn releasing_window_cancels_password_wait_and_advances_other_owner() {
    let dir = temp_dir("release-password-owner");
    let state = Arc::new(AppState::new());
    let archive = create_password_protected_zip(&dir, &state);
    let next_input = dir.join("next.txt");
    std::fs::write(&next_input, b"next owner").unwrap();
    let manager = JobManager::new();
    let sink = Arc::new(TestSink::default());
    let events: Arc<dyn EventSink> = sink.clone();

    let waiting_id = manager.submit_for_test_window(
        "task-password-1".into(),
        Arc::clone(&state),
        Arc::clone(&events),
        password_test_job(&archive),
        SettingsDto::default(),
    );
    wait_for_password_prompt(&sink, waiting_id);

    let next_id = manager.submit_for_test_window(
        "main".into(),
        Arc::clone(&state),
        Arc::clone(&events),
        checksum_job(&next_input),
        SettingsDto::default(),
    );

    assert_eq!(manager.release_window("task-password-other"), 0);
    assert_eq!(manager.snapshot(waiting_id).unwrap().state, "running");
    manager.pause_for_window("main", waiting_id).unwrap();
    assert_eq!(manager.snapshot(waiting_id).unwrap().state, "paused");
    assert_eq!(manager.release_window("task-password-1"), 1);
    wait_for_snapshot_state(
        &manager,
        waiting_id,
        "cancelled",
        std::time::Duration::from_secs(2),
    );
    wait_for_state(&sink, next_id, "done", std::time::Duration::from_secs(2));
    manager.wait_idle();

    assert_eq!(manager.release_window("task-password-1"), 0);
    let recorded = sink.events.lock().unwrap().clone();
    assert_eq!(
        states_of(&recorded, waiting_id),
        vec!["queued", "running", "paused", "cancelled"]
    );
    assert_eq!(
        states_of(&recorded, next_id),
        vec!["queued", "running", "done"]
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn releasing_window_cancels_queued_jobs_and_rejects_late_submission() {
    let dir = temp_dir("release-queued-owner");
    let state = Arc::new(AppState::new());
    let archive = create_password_protected_zip(&dir, &state);
    let queued_input = dir.join("queued.txt");
    std::fs::write(&queued_input, b"must not be archived").unwrap();
    let queued_output = dir.join("cancelled.zip");
    let next_input = dir.join("next.txt");
    std::fs::write(&next_input, b"next in fifo").unwrap();
    let manager = JobManager::new();
    let sink = Arc::new(TestSink::default());
    let events: Arc<dyn EventSink> = sink.clone();

    let blocker_id = manager.submit_for_test_window(
        "main".into(),
        Arc::clone(&state),
        Arc::clone(&events),
        password_test_job(&archive),
        SettingsDto::default(),
    );
    wait_for_password_prompt(&sink, blocker_id);

    let queued_id = manager.submit_for_test_window(
        "task-queued-1".into(),
        Arc::clone(&state),
        Arc::clone(&events),
        compress_file_job(&queued_input, &queued_output),
        SettingsDto::default(),
    );
    let next_id = manager.submit_for_test_window(
        "main".into(),
        Arc::clone(&state),
        Arc::clone(&events),
        checksum_job(&next_input),
        SettingsDto::default(),
    );

    assert_eq!(manager.release_window("task-queued-1"), 1);
    assert_eq!(manager.snapshot(queued_id).unwrap().state, "cancelled");
    let late_id = manager.submit_for_test_window(
        "task-queued-1".into(),
        Arc::clone(&state),
        Arc::clone(&events),
        checksum_job(&queued_input),
        SettingsDto::default(),
    );
    assert_eq!(manager.snapshot(late_id).unwrap().state, "cancelled");

    manager.cancel_for_window("main", blocker_id).unwrap();
    wait_for_state(&sink, next_id, "done", std::time::Duration::from_secs(2));
    manager.wait_idle();

    assert!(!queued_output.exists());
    let recorded = sink.events.lock().unwrap().clone();
    assert!(!states_of(&recorded, queued_id)
        .iter()
        .any(|state| matches!(state.as_str(), "running" | "done" | "failed")));
    assert!(!states_of(&recorded, late_id)
        .iter()
        .any(|state| matches!(state.as_str(), "running" | "done" | "failed")));
    assert_eq!(
        states_of(&recorded, next_id),
        vec!["queued", "running", "done"]
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn releasing_window_serializes_with_an_in_flight_submission() {
    let dir = temp_dir("release-submit-race");
    let state = Arc::new(AppState::new());
    let archive = create_password_protected_zip(&dir, &state);
    let manager = Arc::new(JobManager::new());
    let sink = Arc::new(TestSink::default());
    let events: Arc<dyn EventSink> = sink.clone();
    let start = Arc::new(std::sync::Barrier::new(3));

    let submit_manager = Arc::clone(&manager);
    let submit_state = Arc::clone(&state);
    let submit_events = Arc::clone(&events);
    let submit_start = Arc::clone(&start);
    let submitter = std::thread::spawn(move || {
        submit_start.wait();
        submit_manager.submit_for_test_window(
            "task-race-1".into(),
            submit_state,
            submit_events,
            password_test_job(&archive),
            SettingsDto::default(),
        )
    });

    let release_manager = Arc::clone(&manager);
    let release_start = Arc::clone(&start);
    let releaser = std::thread::spawn(move || {
        release_start.wait();
        release_manager.release_window("task-race-1")
    });

    start.wait();
    let id = submitter.join().unwrap();
    let released = releaser.join().unwrap();
    assert!(released <= 1);
    wait_for_snapshot_state(&manager, id, "cancelled", std::time::Duration::from_secs(2));
    manager.wait_idle();

    let recorded = sink.events.lock().unwrap().clone();
    assert!(!states_of(&recorded, id)
        .iter()
        .any(|state| matches!(state.as_str(), "done" | "failed")));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn releasing_window_aborts_conflict_without_touching_existing_file() {
    let dir = temp_dir("release-conflict-owner");
    let archive = dir.join("conflict.zip");
    std::fs::write(&archive, build_stored_zip(&[(b"same.txt", b"new bytes")])).unwrap();
    let output = dir.join("output");
    std::fs::create_dir_all(&output).unwrap();
    let existing = output.join("same.txt");
    std::fs::write(&existing, b"original bytes").unwrap();
    let state = Arc::new(AppState::new());
    let manager = JobManager::new();
    let sink = Arc::new(TestSink::default());
    let events: Arc<dyn EventSink> = sink.clone();

    let conflict_id = manager.submit_for_test_window(
        "task-conflict-1".into(),
        Arc::clone(&state),
        Arc::clone(&events),
        JobSpec::Extract {
            path: archive.to_string_lossy().into_owned(),
            dest: output.to_string_lossy().into_owned(),
            expected_destination: None,
            expected_input_guard: None,
            selection: None,
            overwrite: squallz_core::api::OverwritePolicy::Ask,
            symlinks: squallz_core::api::SymlinkPolicy::Preserve,
            smart: false,
            encoding: None,
            password: None,
            verify_sfx: false,
            best_effort: false,
        },
        SettingsDto::default(),
    );
    wait_for_event(
        &sink,
        std::time::Duration::from_secs(2),
        |(name, payload)| name == EV_ASK_CONFLICT && payload["id"] == conflict_id,
    );

    let next_id = manager.submit_for_test_window(
        "main".into(),
        Arc::clone(&state),
        Arc::clone(&events),
        checksum_job(&existing),
        SettingsDto::default(),
    );
    assert_eq!(manager.release_window("task-conflict-1"), 1);
    wait_for_snapshot_state(
        &manager,
        conflict_id,
        "cancelled",
        std::time::Duration::from_secs(2),
    );
    wait_for_state(&sink, next_id, "done", std::time::Duration::from_secs(2));
    manager.wait_idle();

    assert_eq!(std::fs::read(&existing).unwrap(), b"original bytes");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn cancelling_all_releases_questions_and_queued_jobs_for_exit() {
    let dir = temp_dir("cancel-all-exit");
    let state = Arc::new(AppState::new());
    let archive = create_password_protected_zip(&dir, &state);
    let queued_input = dir.join("queued.txt");
    std::fs::write(&queued_input, b"queued on exit").unwrap();
    let queued_output = dir.join("queued.zip");
    let manager = JobManager::new();
    let sink = Arc::new(TestSink::default());
    let events: Arc<dyn EventSink> = sink.clone();

    let waiting_id = manager.submit_for_test_window(
        "task-exit-1".into(),
        Arc::clone(&state),
        Arc::clone(&events),
        password_test_job(&archive),
        SettingsDto::default(),
    );
    wait_for_password_prompt(&sink, waiting_id);
    let queued_id = manager.submit_for_test_window(
        "main".into(),
        Arc::clone(&state),
        Arc::clone(&events),
        compress_file_job(&queued_input, &queued_output),
        SettingsDto::default(),
    );

    assert_eq!(manager.cancel_all(), 2);
    assert_eq!(manager.cancel_all(), 0);
    let late_id = manager.submit_for_test_window(
        "main".into(),
        Arc::clone(&state),
        Arc::clone(&events),
        checksum_job(&queued_input),
        SettingsDto::default(),
    );
    assert_eq!(manager.snapshot(late_id).unwrap().state, "cancelled");
    manager.wait_idle();
    assert_eq!(manager.snapshot(waiting_id).unwrap().state, "cancelled");
    assert_eq!(manager.snapshot(queued_id).unwrap().state, "cancelled");
    assert!(!queued_output.exists());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn completed_jobs_are_written_to_backend_audit_log() {
    let dir = temp_dir("audit-log");
    let src = dir.join("secret-source");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("hello.txt"), b"hello audit").unwrap();
    let zip = dir.join("audited.zip");
    let audit_path = dir.join("audit").join("operation-audit.jsonl");
    let audit = Arc::new(OperationAudit::with_path(audit_path.clone(), 20));
    let manager = JobManager::with_audit(Arc::clone(&audit));
    let state = Arc::new(AppState::new());
    let sink = Arc::new(TestSink::default());
    let events: Arc<dyn EventSink> = sink.clone();

    let id = manager.submit(
        Arc::clone(&state),
        Arc::clone(&events),
        JobSpec::Compress {
            inputs: vec![src.to_string_lossy().into_owned()],
            dest: zip.to_string_lossy().into_owned(),
            level: 5,
            password: Some("audit-password-must-not-appear".into()),
            encrypt_names: false,
            split_size: None,
            split_mode: squallz_core::api::SplitOutputMode::Generic,
            excludes: vec![],
            content_policy: squallz_core::CreateContentPolicy::KeepAllFiles,
            sqz_inner_format: None,
            sfx_target: None,
            completion: squallz_core::CreateCompletionAction::None,
            post_success: PostSuccessAction::KeepSource,
            test_after_create: false,
            replace_existing: false,
            replacement_guard: None,
        },
        SettingsDto::default(),
    );
    manager.wait_idle();

    let recent = audit.recent(10);
    assert_eq!(recent.len(), 1);
    assert_eq!(recent[0].id, id);
    assert_eq!(recent[0].kind, "compress");
    assert_eq!(recent[0].state, "done");
    assert!(recent[0].detail.contains("audited.zip"));
    assert!(!recent[0].detail.contains("audit-password"));
    assert!(std::fs::read_to_string(audit_path)
        .unwrap()
        .contains("\"kind\":\"compress\""));

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn extract_nested_job_extracts_inner_archive() {
    let dir = temp_dir("extract-nested");
    let inner_src = dir.join("inner-src");
    std::fs::create_dir_all(&inner_src).unwrap();
    std::fs::write(inner_src.join("hello.txt"), b"hello nested job").unwrap();
    let inner_name = "inner-job-cleanup.zip";
    let inner = dir.join(inner_name);
    let outer = dir.join("outer.zip");
    let state = Arc::new(AppState::new());
    state
        .engine
        .create(
            &inner,
            std::slice::from_ref(&inner_src),
            &CreateOptions::default(),
            &squallz_core::api::NoProgress,
            &ControlToken::new(),
        )
        .unwrap();
    state
        .engine
        .create(
            &outer,
            std::slice::from_ref(&inner),
            &CreateOptions::default(),
            &squallz_core::api::NoProgress,
            &ControlToken::new(),
        )
        .unwrap();

    let manager = JobManager::new();
    let sink = Arc::new(TestSink::default());
    let events: Arc<dyn EventSink> = sink.clone();
    let out = dir.join("out");
    let id = manager.submit(
        Arc::clone(&state),
        Arc::clone(&events),
        JobSpec::ExtractNested {
            outer_path: outer.to_string_lossy().into_owned(),
            entry_path: inner_name.into(),
            dest: out.to_string_lossy().into_owned(),
            overwrite: squallz_core::api::OverwritePolicy::Skip,
            symlinks: squallz_core::api::SymlinkPolicy::Preserve,
            smart: true,
            encoding: None,
            password: None,
            best_effort: false,
        },
        SettingsDto::default(),
    );
    manager.wait_idle();

    assert_eq!(
        std::fs::read(out.join("inner-src/hello.txt")).unwrap(),
        b"hello nested job"
    );
    let events = sink.events.lock().unwrap();
    assert_eq!(states_of(&events, id), vec!["queued", "running", "done"]);
    assert_real_current_file_progress(&events, id, "extract nested");
    let expected_dest = out.to_string_lossy().into_owned();
    assert_eq!(
        done_result(&events, id)
            .and_then(|value| value["dest"].as_str().map(str::to_owned))
            .as_deref(),
        Some(expected_dest.as_str())
    );
    let result = done_result(&events, id).unwrap();
    assert_eq!(result["plan"]["destination"], expected_dest);
    assert_eq!(result["plan"]["layout"], "direct");
    assert_eq!(result["counts"]["selected_entries"], 2);
    assert_eq!(result["counts"]["created"], 1);
    assert_eq!(result["counts"]["directories"], 1);
    assert_eq!(result["counts"]["failed"], 0);

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn extract_nested_job_prompts_separately_for_outer_and_inner_passwords() {
    let dir = temp_dir("extract-nested-passwords");
    let inner_src = dir.join("inner-secret-src");
    std::fs::create_dir_all(&inner_src).unwrap();
    std::fs::write(inner_src.join("secret.txt"), b"two-stage password").unwrap();
    let inner_name = "inner-protected.zip";
    let inner = dir.join(inner_name);
    let outer_name = "outer-protected.zip";
    let outer = dir.join(outer_name);
    let state = Arc::new(AppState::new());
    state
        .engine
        .create(
            &inner,
            std::slice::from_ref(&inner_src),
            &CreateOptions {
                password: Some(Password::new("inner-secret")),
                ..CreateOptions::default()
            },
            &squallz_core::api::NoProgress,
            &ControlToken::new(),
        )
        .unwrap();
    state
        .engine
        .create(
            &outer,
            std::slice::from_ref(&inner),
            &CreateOptions {
                password: Some(Password::new("outer-secret")),
                ..CreateOptions::default()
            },
            &squallz_core::api::NoProgress,
            &ControlToken::new(),
        )
        .unwrap();

    let manager = JobManager::new();
    let sink = Arc::new(TestSink::default());
    let events: Arc<dyn EventSink> = sink.clone();
    let out = dir.join("out");
    let id = manager.submit_for_test_window(
        "main".into(),
        Arc::clone(&state),
        Arc::clone(&events),
        JobSpec::ExtractNested {
            outer_path: outer.to_string_lossy().into_owned(),
            entry_path: inner_name.into(),
            dest: out.to_string_lossy().into_owned(),
            overwrite: squallz_core::api::OverwritePolicy::Skip,
            symlinks: squallz_core::api::SymlinkPolicy::Preserve,
            smart: true,
            encoding: None,
            password: None,
            best_effort: false,
        },
        SettingsDto::default(),
    );

    wait_for_password_prompt_count(&sink, id, 1);
    {
        let recorded = sink.events.lock().unwrap();
        let first = recorded
            .iter()
            .find(|(name, payload)| name == EV_ASK_PASSWORD && payload["id"] == id)
            .unwrap();
        assert_eq!(first.1["name"], outer_name);
    }
    manager
        .answer_password_for_window("main", id, Some("outer-secret".into()))
        .unwrap();

    wait_for_password_prompt_count(&sink, id, 2);
    {
        let recorded = sink.events.lock().unwrap();
        let prompts = recorded
            .iter()
            .filter(|(name, payload)| name == EV_ASK_PASSWORD && payload["id"] == id)
            .collect::<Vec<_>>();
        assert_eq!(prompts[1].1["name"], inner_name);
    }
    manager
        .answer_password_for_window("main", id, Some("inner-secret".into()))
        .unwrap();
    manager.wait_idle();

    assert_eq!(
        std::fs::read(out.join("inner-secret-src/secret.txt")).unwrap(),
        b"two-stage password"
    );
    assert_eq!(state.cached_password_paths(), vec![outer.clone()]);
    let recorded = sink.events.lock().unwrap();
    assert_eq!(states_of(&recorded, id), vec!["queued", "running", "done"]);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn extract_nested_job_applies_safety_limit_before_writing_temp_archive() {
    let dir = temp_dir("extract-nested-limit");
    let inner_src = dir.join("inner-src");
    std::fs::create_dir_all(&inner_src).unwrap();
    std::fs::write(inner_src.join("payload.txt"), b"larger than one byte").unwrap();
    let inner_name = "inner-limited.zip";
    let inner = dir.join(inner_name);
    let outer = dir.join("outer.zip");
    let state = Arc::new(AppState::new());
    state
        .engine
        .create(
            &inner,
            std::slice::from_ref(&inner_src),
            &CreateOptions::default(),
            &squallz_core::api::NoProgress,
            &ControlToken::new(),
        )
        .unwrap();
    state
        .engine
        .create(
            &outer,
            std::slice::from_ref(&inner),
            &CreateOptions::default(),
            &squallz_core::api::NoProgress,
            &ControlToken::new(),
        )
        .unwrap();

    let manager = JobManager::new();
    let sink = Arc::new(TestSink::default());
    let events: Arc<dyn EventSink> = sink.clone();
    let out = dir.join("out");
    let id = manager.submit(
        Arc::clone(&state),
        Arc::clone(&events),
        JobSpec::ExtractNested {
            outer_path: outer.to_string_lossy().into_owned(),
            entry_path: inner_name.into(),
            dest: out.to_string_lossy().into_owned(),
            overwrite: squallz_core::api::OverwritePolicy::Skip,
            symlinks: squallz_core::api::SymlinkPolicy::Preserve,
            smart: true,
            encoding: None,
            password: None,
            best_effort: false,
        },
        SettingsDto {
            safety_max_output_bytes: Some(1),
            ..SettingsDto::default()
        },
    );
    manager.wait_idle();

    let recorded = sink.events.lock().unwrap();
    assert_eq!(
        states_of(&recorded, id),
        vec!["queued", "running", "failed"]
    );
    let failure = recorded
        .iter()
        .find(|(name, payload)| {
            name == EV_STATE && payload["id"] == id && payload["state"] == "failed"
        })
        .unwrap();
    assert_eq!(failure.1["error"]["key"], "error.resource_limit");
    assert!(!out.exists());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn queued_opaque_nested_source_stays_leased_and_never_reaches_public_state() {
    let dir = temp_dir("queued-nested-lease");
    let inner_src = dir.join("inner-src");
    std::fs::create_dir_all(&inner_src).unwrap();
    std::fs::write(inner_src.join("leased.txt"), b"leased source").unwrap();
    let inner_name = "inner-leased.zip";
    let inner = dir.join(inner_name);
    let outer = dir.join("outer-physical.zip");
    let state = Arc::new(AppState::new());
    state
        .engine
        .create(
            &inner,
            std::slice::from_ref(&inner_src),
            &CreateOptions::default(),
            &squallz_core::api::NoProgress,
            &ControlToken::new(),
        )
        .unwrap();
    state
        .engine
        .create(
            &outer,
            std::slice::from_ref(&inner),
            &CreateOptions::default(),
            &squallz_core::api::NoProgress,
            &ControlToken::new(),
        )
        .unwrap();

    let previews = PreviewSessionManager::new().unwrap();
    let preview_root = previews.root_path().unwrap().to_path_buf();
    let reservation = previews.reserve("lease-test").unwrap();
    let mut pending = tempfile::Builder::new()
        .prefix("owned-outer-")
        .suffix(".zip")
        .tempfile_in(reservation.workspace_path().unwrap())
        .unwrap();
    let mut source_file = fs::File::open(&outer).unwrap();
    let size = std::io::copy(&mut source_file, pending.as_file_mut()).unwrap();
    pending.as_file_mut().flush().unwrap();
    let physical_path = pending.path().to_path_buf();
    let display_path = dir
        .join("displayed-outer.zip")
        .to_string_lossy()
        .into_owned();
    let archive = state
        .open_archive_with_owned_temp_and_entry_limit(
            "lease-test",
            pending.into_temp_path(),
            reservation,
            size,
            display_path.clone(),
            "displayed-outer.zip".into(),
            squallz_core::api::SafetyLimits::default().max_entries,
        )
        .unwrap();

    let blocker = create_password_protected_zip(&dir.join("blocker"), &state);
    let manager = JobManager::new();
    let sink = Arc::new(TestSink::default());
    let events: Arc<dyn EventSink> = sink.clone();
    let out = dir.join("out");
    let nested_job = JobSpec::ExtractNested {
        outer_path: archive.source.clone(),
        entry_path: inner_name.into(),
        dest: out.to_string_lossy().into_owned(),
        overwrite: squallz_core::api::OverwritePolicy::Skip,
        symlinks: squallz_core::api::SymlinkPolicy::Preserve,
        smart: true,
        encoding: None,
        password: None,
        best_effort: false,
    };
    let next_id = manager.next_id.load(Ordering::Relaxed);
    let foreign_error = manager
        .submit_for_window(
            "other-window".into(),
            Arc::clone(&state),
            Arc::clone(&events),
            nested_job.clone(),
            SettingsDto::default(),
        )
        .unwrap_err();
    assert_eq!(foreign_error.to_string(), "archive is no longer available");
    assert_eq!(manager.next_id.load(Ordering::Relaxed), next_id);
    assert!(manager.audit.recent(10).is_empty());
    assert!(sink.events.lock().unwrap().is_empty());

    let blocker_id = manager.submit_for_test_window(
        "main".into(),
        Arc::clone(&state),
        Arc::clone(&events),
        password_test_job(&blocker),
        SettingsDto::default(),
    );
    wait_for_password_prompt(&sink, blocker_id);

    let id = manager.submit_for_test_window(
        "lease-test".into(),
        Arc::clone(&state),
        Arc::clone(&events),
        nested_job,
        SettingsDto::default(),
    );
    state.close_archive_for_window("lease-test", archive.id);

    assert_eq!(manager.snapshot(id).unwrap().state, "queued");
    assert!(physical_path.exists());
    manager.cancel_for_window("main", blocker_id).unwrap();
    wait_for_state(&sink, id, "done", std::time::Duration::from_secs(2));
    manager.wait_idle();

    assert_eq!(
        std::fs::read(out.join("inner-src/leased.txt")).unwrap(),
        b"leased source"
    );
    assert!(!physical_path.exists());
    let snapshot = serde_json::to_string(&manager.snapshot(id).unwrap()).unwrap();
    let events_json = serde_json::to_string(&*sink.events.lock().unwrap()).unwrap();
    let audit = manager
        .audit
        .recent(10)
        .into_iter()
        .find(|record| record.id == id)
        .unwrap();
    let audit_json = serde_json::to_string(&audit).unwrap();
    for private in [
        physical_path.to_string_lossy().into_owned(),
        preview_root.to_string_lossy().into_owned(),
        archive.source,
    ] {
        assert!(!snapshot.contains(&private), "snapshot exposed {private}");
        assert!(!events_json.contains(&private), "event exposed {private}");
        assert!(!audit_json.contains(&private), "audit exposed {private}");
    }
    assert!(snapshot.contains(&display_path));
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Convert jobs use the same GUI queue path as the dialog submits.
#[test]
fn convert_job_round_trip_through_queue() {
    let dir = temp_dir("convert");
    let src_dir = dir.join("data");
    std::fs::create_dir_all(&src_dir).unwrap();
    std::fs::write(src_dir.join("hello.txt"), b"hello from convert").unwrap();
    let zip = dir.join("source.zip");
    let sevenz = dir.join("converted.7z");

    AppState::new()
        .engine
        .create(
            &zip,
            &[src_dir],
            &CreateOptions {
                level: CompressionLevel::Fastest,
                ..CreateOptions::default()
            },
            &squallz_core::api::NoProgress,
            &ControlToken::new(),
        )
        .unwrap();

    let manager = JobManager::new();
    let state = Arc::new(AppState::new());
    let sink = Arc::new(TestSink::default());
    let events: Arc<dyn EventSink> = sink.clone();

    let id = manager.submit(
        Arc::clone(&state),
        Arc::clone(&events),
        JobSpec::Convert {
            src: zip.to_string_lossy().into_owned(),
            dest: sevenz.to_string_lossy().into_owned(),
            level: 6,
            src_encoding: None,
            src_password: None,
            dest_password: None,
            encrypt_names: false,
            split_size: None,
            split_mode: squallz_core::api::SplitOutputMode::Generic,
            replace_existing: false,
            replacement_guard: None,
        },
        SettingsDto::default(),
    );
    manager.wait_idle();

    assert!(sevenz.exists());
    let entries = AppState::new()
        .engine
        .list(&sevenz, &OpenOptions::default())
        .unwrap();
    assert!(entries
        .iter()
        .any(|entry| entry.path.display == "data/hello.txt"));
    let recorded_events = sink.events.lock().unwrap();
    assert_eq!(
        states_of(&recorded_events, id),
        vec!["queued", "running", "done"]
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn convert_job_reports_split_output_set() {
    let dir = temp_dir("convert-split");
    let input = dir.join("payload.txt");
    let source = dir.join("source.zip");
    let destination = dir.join("converted.7z");
    let primary = dir.join("converted.7z.001");
    let second = dir.join("converted.7z.002");
    write_incompressible_file(&input, 700 * 1024);

    let state = Arc::new(AppState::new());
    state
        .engine
        .create(
            &source,
            std::slice::from_ref(&input),
            &CreateOptions::default(),
            &squallz_core::api::NoProgress,
            &ControlToken::new(),
        )
        .unwrap();

    let manager = JobManager::new();
    let sink = Arc::new(TestSink::default());
    let events: Arc<dyn EventSink> = sink;
    let id = manager.submit(
        Arc::clone(&state),
        events,
        JobSpec::Convert {
            src: source.to_string_lossy().into_owned(),
            dest: destination.to_string_lossy().into_owned(),
            level: 6,
            src_encoding: None,
            src_password: None,
            dest_password: Some("destination secret".into()),
            encrypt_names: true,
            split_size: Some(256 * 1024),
            split_mode: squallz_core::api::SplitOutputMode::Generic,
            replace_existing: false,
            replacement_guard: None,
        },
        SettingsDto::default(),
    );
    manager.wait_idle();

    assert!(!destination.exists());
    assert!(primary.is_file());
    assert!(second.is_file());
    let snapshot = manager.snapshot(id).unwrap();
    assert_eq!(snapshot.state, "done");
    let result = snapshot.result.as_ref().unwrap();
    assert_eq!(result["operation"], "convert");
    assert_eq!(result["split"], true);
    assert!(result["volume_count"]
        .as_u64()
        .is_some_and(|count| count >= 2));
    assert_eq!(
        result["primary_output"].as_str(),
        Some(primary.to_string_lossy().as_ref())
    );
    assert!(result["outputs"]
        .as_array()
        .is_some_and(|outputs| outputs.len() >= 2));
    assert!(state
        .engine
        .list(&primary, &OpenOptions::default())
        .is_err());
    let entries = state
        .engine
        .list(
            &primary,
            &OpenOptions {
                password: Some(Password::new("destination secret")),
                ..OpenOptions::default()
            },
        )
        .unwrap();
    assert!(entries
        .iter()
        .any(|entry| entry.path.display == "payload.txt"));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn repair_zip_job_rebuilds_missing_central_directory() {
    let dir = temp_dir("repair-zip");
    let damaged = dir.join("missing-central.zip");
    let repaired = dir.join("rebuilt.zip");
    let mut bytes = build_stored_zip(&[(b"hello.txt", b"hello from zip repair")]);
    let central_start = bytes
        .windows(4)
        .position(|window| window == b"PK\x01\x02")
        .expect("central directory exists in sample");
    bytes.truncate(central_start);
    std::fs::write(&damaged, bytes).unwrap();

    let manager = JobManager::new();
    let state = Arc::new(AppState::new());
    let sink = Arc::new(TestSink::default());
    let events: Arc<dyn EventSink> = sink.clone();

    let id = manager.submit(
        Arc::clone(&state),
        Arc::clone(&events),
        JobSpec::RepairZip {
            src: damaged.to_string_lossy().into_owned(),
            dest: repaired.to_string_lossy().into_owned(),
            level: 5,
        },
        SettingsDto::default(),
    );
    manager.wait_idle();

    let rebuilt = std::fs::read(&repaired).unwrap();
    assert!(rebuilt.windows(4).any(|window| window == b"PK\x01\x02"));
    assert!(rebuilt.windows(4).any(|window| window == b"PK\x05\x06"));
    let out = dir.join("out");
    state
        .engine
        .extract(
            &repaired,
            &out,
            None,
            &OpenOptions::default(),
            &squallz_core::api::ExtractOptions::default(),
            &squallz_core::api::NoProgress,
            &ControlToken::new(),
        )
        .unwrap();
    assert_eq!(
        std::fs::read(out.join("hello.txt")).unwrap(),
        b"hello from zip repair"
    );
    let recorded_events = sink.events.lock().unwrap();
    assert_eq!(
        states_of(&recorded_events, id),
        vec!["queued", "running", "done"]
    );
    let result = done_result(&recorded_events, id).unwrap();
    assert_eq!(result["operation"].as_str(), Some("repair_zip"));
    assert_eq!(result["tool"].as_str(), Some("zip-local-header-rebuild"));
    assert_eq!(
        result["dest"].as_str(),
        Some(repaired.to_string_lossy().as_ref())
    );
    assert_eq!(result["source_entries"].as_u64(), Some(1));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn repair_zip_job_refuses_damaged_local_header_payloads() {
    let dir = temp_dir("repair-zip-damaged");
    let damaged = dir.join("damaged-missing-central.zip");
    let repaired = dir.join("must-not-exist.zip");
    let mut bytes = build_stored_zip(&[(b"bad.txt", b"visible payload")]);
    let central_start = bytes
        .windows(4)
        .position(|window| window == b"PK\x01\x02")
        .expect("central directory exists in sample");
    bytes.truncate(central_start);
    let payload_pos = bytes
        .windows(b"visible payload".len())
        .position(|window| window == b"visible payload")
        .expect("payload exists in sample");
    bytes[payload_pos] ^= 0xA5;
    std::fs::write(&damaged, bytes).unwrap();

    let manager = JobManager::new();
    let state = Arc::new(AppState::new());
    let sink = Arc::new(TestSink::default());
    let events: Arc<dyn EventSink> = sink.clone();

    let id = manager.submit(
        Arc::clone(&state),
        Arc::clone(&events),
        JobSpec::RepairZip {
            src: damaged.to_string_lossy().into_owned(),
            dest: repaired.to_string_lossy().into_owned(),
            level: 5,
        },
        SettingsDto::default(),
    );
    manager.wait_idle();

    assert!(!repaired.exists());
    let recorded_events = sink.events.lock().unwrap();
    assert_eq!(
        states_of(&recorded_events, id),
        vec!["queued", "running", "failed"]
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[cfg(unix)]
#[test]
fn convert_job_streams_rar_bridge_to_zip() {
    use std::os::unix::fs::PermissionsExt;

    let _guard = EXTERNAL_TOOL_ENV_LOCK.lock().unwrap();
    let dir = temp_dir("convert-rar");
    let rar = dir.join("source.rar");
    let zip = dir.join("converted.zip");
    let tool = dir.join("fake-bsdtar.sh");
    std::fs::write(&rar, b"Rar!\x1A\x07\x01\x00").unwrap();

    std::fs::write(
        &tool,
        r#"#!/bin/sh
set -eu
if [ "$1" = "-tf" ]; then
  printf 'docs/\nhello.txt\n'
  exit 0
fi
if [ "$1" = "-tvf" ]; then
  printf 'drwxr-xr-x  0 0      0           0 Jan  1  2020 docs/\n'
  printf -- '-rw-r--r--  0 0      0          26 Jan  1  2020 hello.txt\n'
  exit 0
fi
if [ "$1" = "-xOf" ]; then
  last=""
  for arg in "$@"; do
last="$arg"
  done
  case "$last" in
hello.txt) printf 'hello from gui rar convert' ;;
*) printf 'unknown entry: %s\n' "$last" >&2; exit 3 ;;
  esac
  exit 0
fi
printf 'unexpected args\n' >&2
exit 2
"#,
    )
    .unwrap();
    let mut perms = std::fs::metadata(&tool).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&tool, perms).unwrap();
    let _tool_env = EnvRestore::set("SQUALLZ_BSDTAR", &tool);

    let manager = JobManager::new();
    let state = Arc::new(AppState::new());
    let sink = Arc::new(TestSink::default());
    let events: Arc<dyn EventSink> = sink.clone();

    let id = manager.submit(
        Arc::clone(&state),
        Arc::clone(&events),
        JobSpec::Convert {
            src: rar.to_string_lossy().into_owned(),
            dest: zip.to_string_lossy().into_owned(),
            level: 6,
            src_encoding: None,
            src_password: None,
            dest_password: None,
            encrypt_names: false,
            split_size: None,
            split_mode: squallz_core::api::SplitOutputMode::Generic,
            replace_existing: false,
            replacement_guard: None,
        },
        SettingsDto::default(),
    );
    manager.wait_idle();

    assert!(zip.is_file(), "converted ZIP missing");
    let entries = state.engine.list(&zip, &OpenOptions::default()).unwrap();
    assert!(entries
        .iter()
        .any(|entry| entry.path.display == "hello.txt"));
    let out = dir.join("out");
    state
        .engine
        .extract(
            &zip,
            &out,
            None,
            &OpenOptions::default(),
            &squallz_core::api::ExtractOptions::default(),
            &squallz_core::api::NoProgress,
            &ControlToken::new(),
        )
        .unwrap();
    assert_eq!(
        std::fs::read(out.join("hello.txt")).unwrap(),
        b"hello from gui rar convert"
    );
    let events = sink.events.lock().unwrap();
    assert_eq!(states_of(&events, id), vec!["queued", "running", "done"]);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// SQZ export is a named GUI job so the desktop app exposes a clear
/// no-lock-in action instead of forcing users through generic conversion.
#[test]
fn export_sqz_job_round_trip_through_queue() {
    let dir = temp_dir("export-sqz");
    let src_dir = dir.join("data");
    std::fs::create_dir_all(&src_dir).unwrap();
    std::fs::write(src_dir.join("hello.txt"), b"hello from sqz export").unwrap();
    let sqz = dir.join("source.sqz");
    let zip = dir.join("exported.zip");

    AppState::new()
        .engine
        .create(
            &sqz,
            &[src_dir],
            &CreateOptions {
                level: CompressionLevel::Fastest,
                ..CreateOptions::default()
            },
            &squallz_core::api::NoProgress,
            &ControlToken::new(),
        )
        .unwrap();

    let manager = JobManager::new();
    let state = Arc::new(AppState::new());
    let sink = Arc::new(TestSink::default());
    let events: Arc<dyn EventSink> = sink.clone();

    let id = manager.submit(
        Arc::clone(&state),
        Arc::clone(&events),
        JobSpec::ExportSqz {
            src: sqz.to_string_lossy().into_owned(),
            dest: zip.to_string_lossy().into_owned(),
            level: 6,
            dest_password: None,
            replace_existing: false,
            replacement_guard: None,
        },
        SettingsDto::default(),
    );
    manager.wait_idle();

    assert!(zip.exists());
    let entries = AppState::new()
        .engine
        .list(&zip, &OpenOptions::default())
        .unwrap();
    assert!(entries
        .iter()
        .any(|entry| entry.path.display == "data/hello.txt"));
    let recorded_events = sink.events.lock().unwrap();
    assert_eq!(
        states_of(&recorded_events, id),
        vec!["queued", "running", "done"]
    );
    let result = done_result(&recorded_events, id).unwrap();
    assert_eq!(
        result["dest"].as_str(),
        Some(zip.to_string_lossy().as_ref())
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn conversion_jobs_without_replace_permission_preserve_existing_outputs() {
    let dir = temp_dir("conversion-output-policy");
    let input = dir.join("hello.txt");
    let zip = dir.join("source.zip");
    let sqz = dir.join("source.sqz");
    let converted = dir.join("converted.7z");
    let exported = dir.join("exported.zip");
    std::fs::write(&input, b"output policy").unwrap();

    let state = Arc::new(AppState::new());
    for archive in [&zip, &sqz] {
        state
            .engine
            .create(
                archive,
                std::slice::from_ref(&input),
                &CreateOptions::default(),
                &squallz_core::api::NoProgress,
                &ControlToken::new(),
            )
            .unwrap();
    }
    std::fs::write(&converted, b"keep converted output").unwrap();
    std::fs::write(&exported, b"keep exported output").unwrap();

    let manager = JobManager::new();
    let sink = Arc::new(TestSink::default());
    let events: Arc<dyn EventSink> = sink.clone();
    let convert_id = manager.submit(
        Arc::clone(&state),
        Arc::clone(&events),
        JobSpec::Convert {
            src: zip.to_string_lossy().into_owned(),
            dest: converted.to_string_lossy().into_owned(),
            level: 6,
            src_encoding: None,
            src_password: None,
            dest_password: None,
            encrypt_names: false,
            split_size: None,
            split_mode: squallz_core::api::SplitOutputMode::Generic,
            replace_existing: false,
            replacement_guard: None,
        },
        SettingsDto::default(),
    );
    let export_id = manager.submit(
        state,
        events,
        JobSpec::ExportSqz {
            src: sqz.to_string_lossy().into_owned(),
            dest: exported.to_string_lossy().into_owned(),
            level: 6,
            dest_password: None,
            replace_existing: false,
            replacement_guard: None,
        },
        SettingsDto::default(),
    );
    manager.wait_idle();

    assert_eq!(std::fs::read(&converted).unwrap(), b"keep converted output");
    assert_eq!(std::fs::read(&exported).unwrap(), b"keep exported output");
    assert_eq!(manager.snapshot(convert_id).unwrap().state, "failed");
    assert_eq!(manager.snapshot(export_id).unwrap().state, "failed");
    let recorded_events = sink.events.lock().unwrap();
    for id in [convert_id, export_id] {
        assert_eq!(
            states_of(&recorded_events, id),
            vec!["queued", "running", "failed"]
        );
        let failed = recorded_events
            .iter()
            .find(|(name, payload)| {
                name == EV_STATE && payload["id"] == id && payload["state"] == "failed"
            })
            .unwrap();
        assert_eq!(
            failed.1["error"]["key"].as_str(),
            Some("error.output_exists")
        );
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn export_sqz_job_keeps_output_changed_after_confirmation() {
    let dir = temp_dir("export-sqz-stale-output");
    let input = dir.join("hello.txt");
    std::fs::write(&input, b"hello from protected export").unwrap();
    let sqz = dir.join("source.sqz");
    let output = dir.join("exported.zip");
    let state = Arc::new(AppState::new());
    state
        .engine
        .create(
            &sqz,
            std::slice::from_ref(&input),
            &CreateOptions::default(),
            &squallz_core::api::NoProgress,
            &ControlToken::new(),
        )
        .unwrap();
    std::fs::write(&output, b"output presented for confirmation").unwrap();
    let guard = squallz_core::inspect_create_destination(&output, CreateArtifactKind::Archive)
        .unwrap()
        .guard
        .unwrap();
    std::fs::write(&output, b"newer output from another app").unwrap();

    let manager = JobManager::new();
    let sink = Arc::new(TestSink::default());
    let events: Arc<dyn EventSink> = sink.clone();
    let id = manager.submit(
        state,
        events,
        JobSpec::ExportSqz {
            src: sqz.to_string_lossy().into_owned(),
            dest: output.to_string_lossy().into_owned(),
            level: 6,
            dest_password: None,
            replace_existing: true,
            replacement_guard: Some(guard),
        },
        SettingsDto::default(),
    );
    manager.wait_idle();

    assert_eq!(
        std::fs::read(&output).unwrap(),
        b"newer output from another app"
    );
    assert!(!std::fs::read_dir(&dir).unwrap().any(|entry| {
        let name = entry.unwrap().file_name();
        let name = name.to_string_lossy();
        name.contains(".convert-")
            || name.contains("replace-backup")
            || name.starts_with(".squallz-update-")
    }));
    let recorded_events = sink.events.lock().unwrap();
    assert_eq!(
        states_of(&recorded_events, id),
        vec!["queued", "running", "failed"]
    );
    let failed = recorded_events
        .iter()
        .find(|(name, payload)| {
            name == EV_STATE && payload["id"] == id && payload["state"] == "failed"
        })
        .unwrap();
    assert_eq!(
        failed.1["error"]["key"].as_str(),
        Some("error.destination_changed")
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn split_sqz_source_jobs_accept_first_volume() {
    let dir = temp_dir("split-sqz-source");
    let input = dir.join("data.bin");
    write_incompressible_file(&input, 100 * 1024);
    let split_sqz = dir.join("source.sqz");
    AppState::new()
        .engine
        .create(
            &split_sqz,
            std::slice::from_ref(&input),
            &CreateOptions {
                level: CompressionLevel::Fastest,
                split_size: Some(30 * 1024),
                ..CreateOptions::default()
            },
            &squallz_core::api::NoProgress,
            &ControlToken::new(),
        )
        .unwrap();
    assert!(!split_sqz.exists());
    let first = dir.join("source.sqz.001");
    assert!(first.is_file());
    assert!(dir.join("source.sqz.002").is_file());

    let manager = JobManager::new();
    let state = Arc::new(AppState::new());
    let sink = Arc::new(TestSink::default());
    let events: Arc<dyn EventSink> = sink.clone();
    let zip = dir.join("exported.zip");
    let repaired = dir.join("repaired.sqz");

    let export_id = manager.submit(
        Arc::clone(&state),
        Arc::clone(&events),
        JobSpec::ExportSqz {
            src: first.to_string_lossy().into_owned(),
            dest: zip.to_string_lossy().into_owned(),
            level: 6,
            dest_password: None,
            replace_existing: false,
            replacement_guard: None,
        },
        SettingsDto::default(),
    );
    manager.wait_idle();

    std::fs::remove_file(dir.join("source.sqz.002")).unwrap();
    let repair_id = manager.submit(
        Arc::clone(&state),
        Arc::clone(&events),
        JobSpec::RepairSqz {
            src: first.to_string_lossy().into_owned(),
            dest: repaired.to_string_lossy().into_owned(),
            level: 6,
        },
        SettingsDto::default(),
    );
    manager.wait_idle();

    let engine = AppState::new().engine;
    let entries = engine.list(&zip, &OpenOptions::default()).unwrap();
    assert!(entries.iter().any(|entry| entry.path.display == "data.bin"));
    let report = engine
        .test_summary(
            &repaired,
            &OpenOptions::default(),
            &squallz_core::api::NoProgress,
            &ControlToken::new(),
        )
        .unwrap();
    assert!(report.is_ok(), "problems: {:?}", report.problems);

    let recorded_events = sink.events.lock().unwrap();
    assert_eq!(
        states_of(&recorded_events, export_id),
        vec!["queued", "running", "done"]
    );
    assert_eq!(
        states_of(&recorded_events, repair_id),
        vec!["queued", "running", "done"]
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn repair_sqz_job_rewrites_recovered_container() {
    let dir = temp_dir("repair-sqz");
    let src_dir = dir.join("data");
    std::fs::create_dir_all(&src_dir).unwrap();
    std::fs::write(src_dir.join("hello.txt"), b"hello from sqz repair").unwrap();
    let damaged = dir.join("damaged.sqz");
    let repaired = dir.join("repaired.sqz");

    AppState::new()
        .engine
        .create(
            &damaged,
            &[src_dir],
            &CreateOptions {
                level: CompressionLevel::Fastest,
                ..CreateOptions::default()
            },
            &squallz_core::api::NoProgress,
            &ControlToken::new(),
        )
        .unwrap();
    corrupt_sqz_payload_byte(&damaged);

    let manager = JobManager::new();
    let state = Arc::new(AppState::new());
    let sink = Arc::new(TestSink::default());
    let events: Arc<dyn EventSink> = sink.clone();

    let id = manager.submit(
        Arc::clone(&state),
        Arc::clone(&events),
        JobSpec::RepairSqz {
            src: damaged.to_string_lossy().into_owned(),
            dest: repaired.to_string_lossy().into_owned(),
            level: 6,
        },
        SettingsDto::default(),
    );
    manager.wait_idle();

    assert!(repaired.exists());
    let report = AppState::new()
        .engine
        .test_summary(
            &repaired,
            &OpenOptions::default(),
            &squallz_core::api::NoProgress,
            &ControlToken::new(),
        )
        .unwrap();
    assert!(report.is_ok(), "problems: {:?}", report.problems);
    let recorded_events = sink.events.lock().unwrap();
    assert_eq!(
        states_of(&recorded_events, id),
        vec!["queued", "running", "done"]
    );
    let result = done_result(&recorded_events, id).unwrap();
    assert_eq!(
        result["dest"].as_str(),
        Some(repaired.to_string_lossy().as_ref())
    );
    assert_eq!(result["in_place"].as_bool(), Some(false));
    assert_eq!(
        result["recovery"]["scheme"].as_str(),
        Some("sqz-embedded-rs-gf8")
    );
    assert_eq!(result["recovery"]["damaged_blocks"].as_u64(), Some(1));
    assert_eq!(result["recovery"]["repaired_blocks"].as_u64(), Some(1));
    assert_eq!(result["recovery"]["unrepaired_blocks"].as_u64(), Some(0));
    assert_eq!(result["recovery"]["repair_possible"].as_bool(), Some(true));
    drop(recorded_events);

    let in_place_id = manager.submit(
        Arc::clone(&state),
        Arc::clone(&events),
        JobSpec::RepairSqz {
            src: damaged.to_string_lossy().into_owned(),
            dest: damaged.to_string_lossy().into_owned(),
            level: 6,
        },
        SettingsDto::default(),
    );
    manager.wait_idle();

    let report = AppState::new()
        .engine
        .test_summary(
            &damaged,
            &OpenOptions::default(),
            &squallz_core::api::NoProgress,
            &ControlToken::new(),
        )
        .unwrap();
    assert!(report.is_ok(), "problems: {:?}", report.problems);
    let recorded_events = sink.events.lock().unwrap();
    assert_eq!(
        states_of(&recorded_events, in_place_id),
        vec!["queued", "running", "done"]
    );
    let result = done_result(&recorded_events, in_place_id).unwrap();
    assert_eq!(
        result["dest"].as_str(),
        Some(damaged.to_string_lossy().as_ref())
    );
    assert_eq!(result["in_place"].as_bool(), Some(true));
    assert_eq!(result["recovery"]["damaged_blocks"].as_u64(), Some(1));
    assert_eq!(result["recovery"]["repaired_blocks"].as_u64(), Some(1));
    assert_eq!(result["recovery"]["repair_possible"].as_bool(), Some(true));
    drop(recorded_events);

    let split_input = dir.join("large.bin");
    write_incompressible_file(&split_input, 100 * 1024);
    let split_base = dir.join("split-damaged.sqz");
    state
        .engine
        .create(
            &split_base,
            std::slice::from_ref(&split_input),
            &CreateOptions {
                level: CompressionLevel::Fastest,
                split_size: Some(30 * 1024),
                ..CreateOptions::default()
            },
            &squallz_core::api::NoProgress,
            &ControlToken::new(),
        )
        .unwrap();
    let split_first = dir.join("split-damaged.sqz.001");
    std::fs::remove_file(dir.join("split-damaged.sqz.002")).unwrap();
    let split_repaired = dir.join("split-repaired.sqz");

    let split_id = manager.submit(
        Arc::clone(&state),
        Arc::clone(&events),
        JobSpec::RepairSqz {
            src: split_first.to_string_lossy().into_owned(),
            dest: split_repaired.to_string_lossy().into_owned(),
            level: 6,
        },
        SettingsDto::default(),
    );
    manager.wait_idle();

    let report = AppState::new()
        .engine
        .test_summary(
            &split_repaired,
            &OpenOptions::default(),
            &squallz_core::api::NoProgress,
            &ControlToken::new(),
        )
        .unwrap();
    assert!(report.is_ok(), "problems: {:?}", report.problems);
    let recorded_events = sink.events.lock().unwrap();
    assert_eq!(
        states_of(&recorded_events, split_id),
        vec!["queued", "running", "done"]
    );
    let result = done_result(&recorded_events, split_id).unwrap();
    assert_eq!(
        result["dest"].as_str(),
        Some(split_repaired.to_string_lossy().as_ref())
    );
    assert_eq!(result["in_place"].as_bool(), Some(false));
    assert_eq!(
        result["recovery"]["scheme"].as_str(),
        Some("sqz-embedded-rs-gf8")
    );
    assert_eq!(result["recovery"]["unrepaired_blocks"].as_u64(), Some(0));
    assert_eq!(result["recovery"]["repair_possible"].as_bool(), Some(true));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn update_job_deletes_selected_entry() {
    let dir = temp_dir("update-delete");
    let src = dir.join("data");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("keep.txt"), b"keep").unwrap();
    std::fs::write(src.join("drop.txt"), b"drop").unwrap();
    let archive = dir.join("out.zip");
    let state = Arc::new(AppState::new());
    state
        .engine
        .create(
            &archive,
            std::slice::from_ref(&src),
            &CreateOptions::default(),
            &squallz_core::api::NoProgress,
            &ControlToken::new(),
        )
        .unwrap();

    let manager = JobManager::new();
    let sink = Arc::new(TestSink::default());
    let events: Arc<dyn EventSink> = sink.clone();
    let id = manager.submit(
        Arc::clone(&state),
        Arc::clone(&events),
        JobSpec::Update {
            path: archive.to_string_lossy().into_owned(),
            add: vec![],
            delete: vec!["data/drop.txt".into()],
            rename: vec![],
            mkdir: vec![],
            excludes: vec![],
            content_policy: squallz_core::CreateContentPolicy::KeepAllFiles,
            password: None,
            level: 5,
        },
        SettingsDto::default(),
    );
    manager.wait_idle();
    let events = sink.events.lock().unwrap().clone();
    assert_eq!(states_of(&events, id), vec!["queued", "running", "done"]);
    let entries = state
        .engine
        .list(&archive, &OpenOptions::default())
        .unwrap();
    assert!(entries.iter().any(|e| e.path.display == "data/keep.txt"));
    assert!(!entries.iter().any(|e| e.path.display == "data/drop.txt"));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn update_job_add_directory_applies_content_policy_and_explicit_excludes() {
    let dir = temp_dir("update-add-excludes");
    let seed = dir.join("seed");
    std::fs::create_dir_all(&seed).unwrap();
    std::fs::write(seed.join("base.txt"), b"base").unwrap();
    let archive = dir.join("out.zip");
    let state = Arc::new(AppState::new());
    state
        .engine
        .create(
            &archive,
            std::slice::from_ref(&seed),
            &CreateOptions::default(),
            &squallz_core::api::NoProgress,
            &ControlToken::new(),
        )
        .unwrap();

    let extra = dir.join("extra");
    std::fs::create_dir_all(extra.join("node_modules/pkg")).unwrap();
    std::fs::create_dir_all(extra.join("__MACOSX")).unwrap();
    std::fs::write(extra.join("keep.txt"), b"keep").unwrap();
    std::fs::write(extra.join(".env"), b"MODE=test").unwrap();
    std::fs::write(extra.join(".DS_Store"), b"finder metadata").unwrap();
    std::fs::write(extra.join("._keep.txt"), b"appledouble metadata").unwrap();
    std::fs::write(extra.join("__MACOSX/metadata"), b"metadata").unwrap();
    std::fs::write(extra.join("drop.tmp"), b"drop").unwrap();
    std::fs::write(extra.join("node_modules/pkg/index.js"), b"drop").unwrap();

    let manager = JobManager::new();
    let sink = Arc::new(TestSink::default());
    let events: Arc<dyn EventSink> = sink.clone();
    let id = manager.submit(
        Arc::clone(&state),
        Arc::clone(&events),
        JobSpec::Update {
            path: archive.to_string_lossy().into_owned(),
            add: vec![extra.to_string_lossy().into_owned()],
            delete: vec![],
            rename: vec![],
            mkdir: vec![],
            excludes: vec!["node_modules".into(), "*.tmp".into()],
            content_policy: squallz_core::CreateContentPolicy::CrossPlatformClean,
            password: None,
            level: 5,
        },
        SettingsDto::default(),
    );
    manager.wait_idle();
    let events = sink.events.lock().unwrap().clone();
    assert_eq!(states_of(&events, id), vec!["queued", "running", "done"]);
    let names: Vec<String> = state
        .engine
        .list(&archive, &OpenOptions::default())
        .unwrap()
        .into_iter()
        .map(|e| e.path.display)
        .collect();
    assert!(names.iter().any(|name| name == "extra/keep.txt"));
    assert!(names.iter().any(|name| name == "extra/.env"));
    assert!(!names.iter().any(|name| name.contains("node_modules")));
    assert!(!names.iter().any(|name| name.contains("__MACOSX")));
    assert!(!names.iter().any(|name| name.ends_with(".DS_Store")));
    assert!(!names.iter().any(|name| name.ends_with("._keep.txt")));
    assert!(!names.iter().any(|name| name.ends_with(".tmp")));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn duplicate_scan_job_reports_groups_without_modifying_files() {
    let dir = temp_dir("duplicate-scan-job");
    let root = dir.join("project");
    std::fs::create_dir_all(root.join("cache")).unwrap();
    std::fs::write(root.join("a.bin"), b"same bytes").unwrap();
    std::fs::write(root.join("b.bin"), b"same bytes").unwrap();
    std::fs::write(root.join("unique.bin"), b"unique bytes").unwrap();
    std::fs::write(root.join("cache").join("ignored.bin"), b"same bytes").unwrap();

    let manager = JobManager::new();
    let state = Arc::new(AppState::new());
    let sink = Arc::new(TestSink::default());
    let events: Arc<dyn EventSink> = sink.clone();
    let id = manager.submit(
        Arc::clone(&state),
        Arc::clone(&events),
        JobSpec::DuplicateScan {
            inputs: vec![root.to_string_lossy().into_owned()],
            excludes: vec!["cache".into()],
            min_size: 1,
        },
        SettingsDto::default(),
    );
    manager.wait_idle();

    assert_eq!(std::fs::read(root.join("a.bin")).unwrap(), b"same bytes");
    assert_eq!(std::fs::read(root.join("b.bin")).unwrap(), b"same bytes");
    let events = sink.events.lock().unwrap().clone();
    assert_eq!(states_of(&events, id), vec!["queued", "running", "done"]);
    let result = done_result(&events, id).expect("duplicate result");
    assert_eq!(result["operation"], "duplicates");
    assert_eq!(result["hash_algorithm"], "blake3");
    assert_eq!(result["duplicate_groups"].as_u64(), Some(1));
    assert_eq!(result["duplicate_files"].as_u64(), Some(2));
    assert_eq!(result["groups"][0]["count"].as_u64(), Some(2));
    assert_eq!(result["groups"][0]["paths"].as_array().unwrap().len(), 2);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn checksum_job_hashes_files_with_shared_excludes() {
    let dir = temp_dir("checksum-job");
    let root = dir.join("project");
    std::fs::create_dir_all(root.join("target")).unwrap();
    std::fs::write(root.join("keep.txt"), b"abc").unwrap();
    std::fs::write(root.join("target").join("ignored.txt"), b"ignore").unwrap();

    let manager = JobManager::new();
    let state = Arc::new(AppState::new());
    let sink = Arc::new(TestSink::default());
    let events: Arc<dyn EventSink> = sink.clone();
    let id = manager.submit(
        Arc::clone(&state),
        Arc::clone(&events),
        JobSpec::Checksum {
            inputs: vec![root.to_string_lossy().into_owned()],
            excludes: vec!["target".into()],
            algorithm: ChecksumAlgorithm::Sha256,
        },
        SettingsDto::default(),
    );
    manager.wait_idle();

    assert_eq!(std::fs::read(root.join("keep.txt")).unwrap(), b"abc");
    let events = sink.events.lock().unwrap().clone();
    assert_eq!(states_of(&events, id), vec!["queued", "running", "done"]);
    assert!(events.iter().any(|(name, payload)| name == EV_PROGRESS
        && payload["id"] == id
        && payload["done"] == 3
        && payload["total"] == 3
        && payload["current"]
            .as_str()
            .is_some_and(|current| current.ends_with("keep.txt"))));
    let result = done_result(&events, id).expect("checksum result");
    assert_eq!(result["operation"], "checksum");
    assert_eq!(result["algorithm"], "sha256");
    assert_eq!(result["files_hashed"].as_u64(), Some(1));
    assert_eq!(result["bytes_hashed"].as_u64(), Some(3));
    assert_eq!(
        result["items"][0]["digest"],
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert!(!result["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["path"]
            .as_str()
            .unwrap_or_default()
            .contains("ignored")));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn checksum_check_job_reports_manifest_mismatch() {
    let dir = temp_dir("checksum-check-job");
    std::fs::write(dir.join("good.txt"), b"abc").unwrap();
    std::fs::write(dir.join("bad.txt"), b"changed").unwrap();
    std::fs::write(
        dir.join("SHA256SUMS"),
        concat!(
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad  good.txt\n",
            "0000000000000000000000000000000000000000000000000000000000000000  bad.txt\n",
        ),
    )
    .unwrap();

    let manager = JobManager::new();
    let state = Arc::new(AppState::new());
    let sink = Arc::new(TestSink::default());
    let events: Arc<dyn EventSink> = sink.clone();
    let id = manager.submit(
        Arc::clone(&state),
        Arc::clone(&events),
        JobSpec::ChecksumCheck {
            manifest: dir.join("SHA256SUMS").to_string_lossy().into_owned(),
            algorithm: ChecksumAlgorithm::Sha256,
        },
        SettingsDto::default(),
    );
    manager.wait_idle();

    let events = sink.events.lock().unwrap().clone();
    assert_eq!(states_of(&events, id), vec!["queued", "running", "done"]);
    let result = done_result(&events, id).expect("checksum check result");
    assert_eq!(result["operation"], "checksum_check");
    assert_eq!(result["ok"].as_bool(), Some(false));
    assert_eq!(result["checked"].as_u64(), Some(2));
    assert_eq!(result["passed"].as_u64(), Some(1));
    assert_eq!(result["failed"].as_u64(), Some(1));
    assert_eq!(std::fs::read(dir.join("bad.txt")).unwrap(), b"changed");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn update_job_creates_empty_directory_entry() {
    let dir = temp_dir("update-mkdir");
    let seed = dir.join("seed");
    std::fs::create_dir_all(&seed).unwrap();
    std::fs::write(seed.join("base.txt"), b"base").unwrap();
    let archive = dir.join("out.zip");
    let state = Arc::new(AppState::new());
    state
        .engine
        .create(
            &archive,
            std::slice::from_ref(&seed),
            &CreateOptions::default(),
            &squallz_core::api::NoProgress,
            &ControlToken::new(),
        )
        .unwrap();

    let manager = JobManager::new();
    let sink = Arc::new(TestSink::default());
    let events: Arc<dyn EventSink> = sink.clone();
    let id = manager.submit(
        Arc::clone(&state),
        Arc::clone(&events),
        JobSpec::Update {
            path: archive.to_string_lossy().into_owned(),
            add: vec![],
            delete: vec![],
            rename: vec![],
            mkdir: vec!["new-folder".into()],
            excludes: vec![],
            content_policy: squallz_core::CreateContentPolicy::KeepAllFiles,
            password: None,
            level: 5,
        },
        SettingsDto::default(),
    );
    manager.wait_idle();
    let events = sink.events.lock().unwrap().clone();
    assert_eq!(states_of(&events, id), vec!["queued", "running", "done"]);
    let names: Vec<String> = state
        .engine
        .list(&archive, &OpenOptions::default())
        .unwrap()
        .into_iter()
        .map(|e| e.path.display)
        .collect();
    assert!(names.iter().any(|name| name == "new-folder/"));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn update_job_reports_target_conflict_as_failed() {
    let dir = temp_dir("update-conflict");
    let seed = dir.join("seed");
    std::fs::create_dir_all(&seed).unwrap();
    std::fs::write(seed.join("a.txt"), b"alpha").unwrap();
    std::fs::write(seed.join("b.txt"), b"bravo").unwrap();
    let archive = dir.join("out.zip");
    let state = Arc::new(AppState::new());
    state
        .engine
        .create(
            &archive,
            std::slice::from_ref(&seed),
            &CreateOptions::default(),
            &squallz_core::api::NoProgress,
            &ControlToken::new(),
        )
        .unwrap();
    let before = std::fs::read(&archive).unwrap();

    let manager = JobManager::new();
    let sink = Arc::new(TestSink::default());
    let events: Arc<dyn EventSink> = sink.clone();
    let id = manager.submit(
        Arc::clone(&state),
        Arc::clone(&events),
        JobSpec::Update {
            path: archive.to_string_lossy().into_owned(),
            add: vec![],
            delete: vec![],
            rename: vec![crate::dto::RenameSpec {
                from: "seed/a.txt".into(),
                to: "seed/b.txt".into(),
            }],
            mkdir: vec![],
            excludes: vec![],
            content_policy: squallz_core::CreateContentPolicy::KeepAllFiles,
            password: None,
            level: 5,
        },
        SettingsDto::default(),
    );
    manager.wait_idle();

    let events = sink.events.lock().unwrap().clone();
    assert_eq!(states_of(&events, id), vec!["queued", "running", "failed"]);
    let failed = events
        .iter()
        .find(|(name, p)| name == EV_STATE && p["id"] == id && p["state"] == "failed")
        .expect("failed state");
    assert_eq!(failed.1["error"]["key"].as_str(), Some("error.other"));
    assert!(
        failed.1["error"]["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("already exists")),
        "{:?}",
        failed.1
    );
    assert_eq!(std::fs::read(&archive).unwrap(), before);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn extract_job_uses_submitted_safety_limits() {
    let dir = temp_dir("limits");
    let src = dir.join("data");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("payload.txt"), b"payload over one byte").unwrap();
    let zip = dir.join("limited.zip");

    AppState::new()
        .engine
        .create(
            &zip,
            &[src],
            &CreateOptions {
                level: CompressionLevel::Fastest,
                ..CreateOptions::default()
            },
            &squallz_core::api::NoProgress,
            &ControlToken::new(),
        )
        .unwrap();

    let manager = JobManager::new();
    let state = Arc::new(AppState::new());
    let sink = Arc::new(TestSink::default());
    let events: Arc<dyn EventSink> = sink.clone();
    let out = dir.join("out");

    let id = manager.submit(
        Arc::clone(&state),
        Arc::clone(&events),
        JobSpec::Extract {
            path: zip.to_string_lossy().into_owned(),
            dest: out.to_string_lossy().into_owned(),
            expected_destination: None,
            expected_input_guard: None,
            selection: None,
            overwrite: squallz_core::api::OverwritePolicy::Skip,
            symlinks: squallz_core::api::SymlinkPolicy::Preserve,
            smart: false,
            encoding: None,
            password: None,
            verify_sfx: false,
            best_effort: false,
        },
        SettingsDto {
            safety_max_output_bytes: Some(1),
            ..SettingsDto::default()
        },
    );
    manager.wait_idle();

    let events = sink.events.lock().unwrap();
    let failed = events
        .iter()
        .find(|(name, p)| name == EV_STATE && p["id"] == id && p["state"] == "failed");
    assert_eq!(
        failed.and_then(|(_, p)| p["error"]["key"].as_str()),
        Some("error.resource_limit")
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn extract_job_best_effort_reports_skipped_entries() {
    let dir = temp_dir("best-effort");
    let archive = dir.join("damaged.zip");
    let good_name = b"good.txt";
    let good_data = b"safe bytes";
    let bad_name = b"bad.txt";
    let bad_data = b"broken bytes";
    let mut bytes = build_stored_zip(&[(good_name, good_data), (bad_name, bad_data)]);
    let bad_data_offset = 30 + good_name.len() + good_data.len() + 30 + bad_name.len();
    bytes[bad_data_offset] ^= 0xFF;
    std::fs::write(&archive, bytes).unwrap();

    let manager = JobManager::new();
    let state = Arc::new(AppState::new());
    let sink = Arc::new(TestSink::default());
    let events: Arc<dyn EventSink> = sink.clone();
    let out = dir.join("out");
    let id = manager.submit(
        Arc::clone(&state),
        Arc::clone(&events),
        JobSpec::Extract {
            path: archive.to_string_lossy().into_owned(),
            dest: out.to_string_lossy().into_owned(),
            expected_destination: None,
            expected_input_guard: None,
            selection: None,
            overwrite: squallz_core::api::OverwritePolicy::RenameBoth,
            symlinks: squallz_core::api::SymlinkPolicy::Preserve,
            smart: false,
            encoding: None,
            password: None,
            verify_sfx: false,
            best_effort: true,
        },
        SettingsDto::default(),
    );
    manager.wait_idle();

    assert_eq!(std::fs::read(out.join("good.txt")).unwrap(), good_data);
    assert!(!out.join("bad.txt").exists());
    let events = sink.events.lock().unwrap().clone();
    assert_eq!(states_of(&events, id), vec!["queued", "running", "done"]);
    let result = done_result(&events, id).unwrap();
    assert_eq!(result["best_effort"], true);
    assert_eq!(result["problems_total"], 1);
    assert_eq!(result["counts"]["selected_entries"], 2);
    assert_eq!(result["counts"]["created"], 1);
    assert_eq!(result["counts"]["failed"], 1);
    assert!(result["problems"][0].as_str().unwrap().contains("bad.txt"));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn test_job_keeps_exact_problem_total_with_bounded_messages() {
    let dir = temp_dir("test-bounded-problems");
    let archive = dir.join("damaged.zip");
    let names = (0..25)
        .map(|index| format!("damaged-{index:02}.txt").into_bytes())
        .collect::<Vec<_>>();
    let payloads = (0..25)
        .map(|index| format!("squallz-damaged-payload-{index:02}").into_bytes())
        .collect::<Vec<_>>();
    let entries = names
        .iter()
        .zip(&payloads)
        .map(|(name, payload)| (name.as_slice(), payload.as_slice()))
        .collect::<Vec<_>>();
    let mut bytes = build_stored_zip(&entries);
    for payload in &payloads {
        let offset = bytes
            .windows(payload.len())
            .position(|window| window == payload)
            .unwrap();
        bytes[offset] ^= 0xFF;
    }
    std::fs::write(&archive, bytes).unwrap();

    let manager = JobManager::new();
    let state = Arc::new(AppState::new());
    let sink = Arc::new(TestSink::default());
    let events: Arc<dyn EventSink> = sink.clone();
    let id = manager.submit(
        Arc::clone(&state),
        Arc::clone(&events),
        JobSpec::Test {
            path: archive.to_string_lossy().into_owned(),
            encoding: None,
            password: None,
        },
        SettingsDto::default(),
    );
    manager.wait_idle();

    let events = sink.events.lock().unwrap().clone();
    assert_eq!(states_of(&events, id), vec!["queued", "running", "done"]);
    let result = done_result(&events, id).unwrap();
    assert_eq!(result["ok"], false);
    assert_eq!(result["entries"], 25);
    assert_eq!(result["entries_tested"], 25);
    assert_eq!(result["problems_total"], 25);
    assert_eq!(result["problems_truncated"], true);
    assert_eq!(result["problems"].as_array().map(Vec::len), Some(20));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn recovered_zip_test_job_reports_typed_structure_status() {
    let dir = temp_dir("test-recovered-zip-structure");
    let archive = dir.join("missing-central-directory.zip");
    let mut bytes = build_stored_zip(&[(b"recoverable.txt", b"recoverable payload")]);
    let central_start = bytes
        .windows(4)
        .position(|window| window == b"PK\x01\x02")
        .expect("central directory exists in sample");
    bytes.truncate(central_start);
    std::fs::write(&archive, bytes).unwrap();

    let manager = JobManager::new();
    let state = Arc::new(AppState::new());
    let sink = Arc::new(TestSink::default());
    let events: Arc<dyn EventSink> = sink.clone();
    let id = manager.submit(
        Arc::clone(&state),
        Arc::clone(&events),
        JobSpec::Test {
            path: archive.to_string_lossy().into_owned(),
            encoding: None,
            password: None,
        },
        SettingsDto::default(),
    );
    manager.wait_idle();

    let events = sink.events.lock().unwrap().clone();
    assert_eq!(states_of(&events, id), vec!["queued", "running", "done"]);
    let result = done_result(&events, id).unwrap();
    assert_eq!(result["ok"], false);
    assert_eq!(result["entries_tested"], 1);
    assert_eq!(result["problems_total"], 1);
    assert_eq!(result["structure"], "zip_local_headers_recovered");
    assert_eq!(result["problems"].as_array().map(Vec::len), Some(1));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[cfg(unix)]
#[test]
fn recovery_job_bridges_to_external_par2_tool() {
    use base64::Engine as _;
    use std::os::unix::fs::PermissionsExt;

    let _guard = EXTERNAL_TOOL_ENV_LOCK.lock().unwrap();

    let dir = temp_dir("recovery");
    let archive = dir.join("protected.zip");
    let recovery = dir.join("protected.zip.par2");
    let recovery_fixture = dir.join("protected.zip.fixture.par2");
    let recovery_volume_fixture = dir.join("protected.zip.fixture.vol0+1.par2");
    let split_dir = dir.join("split");
    std::fs::create_dir(&split_dir).unwrap();
    let split_first = split_dir.join("set.zip.001");
    let split_second = split_dir.join("set.zip.002");
    let split_recovery = split_dir.join("set.zip.par2");
    let multi_first = dir.join("set.zip.001");
    let multi_second = dir.join("set.zip.002");
    let multi_recovery = dir.join("set.zip.par2");
    let multi_recovery_volume = dir.join("set.zip.vol0+4.par2");
    let multi_output = dir.join("repaired-set");
    let tool = dir.join("fake-par2");
    let log = dir.join("fake-par2.log");
    std::fs::write(&archive, b"archive bytes").unwrap();
    std::fs::write(&split_first, b"first-volume-original\n").unwrap();
    std::fs::write(&split_second, b"second-volume-original\n").unwrap();
    std::fs::write(&multi_first, b"damaged").unwrap();
    let fixture = base64::engine::general_purpose::STANDARD
        .decode(
            include_str!("../../../squallz-recovery/tests/fixtures/protected.zip.par2.b64").trim(),
        )
        .unwrap();
    std::fs::write(&recovery_fixture, fixture).unwrap();
    std::fs::write(
        &recovery_volume_fixture,
        base64::engine::general_purpose::STANDARD
            .decode(
                include_str!(
                    "../../../squallz-recovery/tests/fixtures/protected.zip.vol0+1.par2.b64"
                )
                .trim(),
            )
            .unwrap(),
    )
    .unwrap();
    std::fs::write(
        &multi_recovery,
        base64::engine::general_purpose::STANDARD
            .decode(
                include_str!("../../../squallz-recovery/tests/fixtures/multi-set.zip.par2.b64")
                    .trim(),
            )
            .unwrap(),
    )
    .unwrap();
    std::fs::write(
        &multi_recovery_volume,
        base64::engine::general_purpose::STANDARD
            .decode(
                include_str!(
                    "../../../squallz-recovery/tests/fixtures/multi-set.zip.vol0+4.par2.b64"
                )
                .trim(),
            )
            .unwrap(),
    )
    .unwrap();
    std::fs::write(
        &tool,
        r#"#!/bin/sh
echo "$*" >> "$SQUALLZ_FAKE_PAR2_LOG"
case "$1" in
  create)
printf 'Constructing: 25.0%%\rProcessing: 75.0%%\rWriting recovery packets\nDone\n'
if [ "$(basename "$5")" = "set.zip.001" ]; then
  cp "$SQUALLZ_FAKE_PAR2_MULTI_FIXTURE" "$4"
  cp "$SQUALLZ_FAKE_PAR2_MULTI_VOLUME_FIXTURE" "${4%.par2}.vol0+4.par2"
else
  cp "$SQUALLZ_FAKE_PAR2_FIXTURE" "$4"
  cp "$SQUALLZ_FAKE_PAR2_VOLUME_FIXTURE" "${4%.par2}.vol0+1.par2"
fi
;;
  verify|repair)
printf 'Loading: 20.0%%\r'
recovery="$2"
base=""
case "$2" in
  -B*)
    base="${2#-B}"
    recovery="$3"
    ;;
esac
test -f "$recovery" || exit 2
if [ "$1" = verify ]; then
  printf 'Verifying source files:\nScanning: 80.0%%\rDone\n'
fi
if [ "$1" = repair ]; then
  printf 'Processing: 60.0%%\r'
  if [ -n "$base" ] && [ "$(basename "${recovery%.par2}")" = "set.zip" ]; then
    printf 'first-volume-original\n' > "$base/set.zip.001"
    printf 'second-volume-original\n' > "$base/set.zip.002"
  elif [ -n "$base" ]; then
    target="$base/$(basename "${recovery%.par2}")"
    printf 'archive bytes' > "$target"
  else
    target="${recovery%.par2}"
    printf 'archive bytes' > "$target"
  fi
  printf 'Writing recovered data\nRepair complete.\n'
fi
;;
  *)
exit 64
;;
esac
"#,
    )
    .unwrap();
    let mut perms = std::fs::metadata(&tool).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&tool, perms).unwrap();
    let _tool_env = EnvRestore::set("SQUALLZ_PAR2", &tool);
    let _log_env = EnvRestore::set("SQUALLZ_FAKE_PAR2_LOG", &log);
    let _fixture_env = EnvRestore::set("SQUALLZ_FAKE_PAR2_FIXTURE", &recovery_fixture);
    let _volume_fixture_env =
        EnvRestore::set("SQUALLZ_FAKE_PAR2_VOLUME_FIXTURE", &recovery_volume_fixture);
    let _multi_fixture_env = EnvRestore::set("SQUALLZ_FAKE_PAR2_MULTI_FIXTURE", &multi_recovery);
    let _multi_volume_fixture_env = EnvRestore::set(
        "SQUALLZ_FAKE_PAR2_MULTI_VOLUME_FIXTURE",
        &multi_recovery_volume,
    );

    let manager = JobManager::new();
    let state = Arc::new(AppState::new());
    let sink = Arc::new(TestSink::default());
    let events: Arc<dyn EventSink> = sink.clone();

    let protect_id = manager.submit(
        Arc::clone(&state),
        Arc::clone(&events),
        JobSpec::Protect {
            path: archive.to_string_lossy().into_owned(),
            redundancy: 12,
            recovery: Some(recovery.to_string_lossy().into_owned()),
        },
        SettingsDto::default(),
    );
    let split_protect_id = manager.submit(
        Arc::clone(&state),
        Arc::clone(&events),
        JobSpec::Protect {
            path: split_second.to_string_lossy().into_owned(),
            redundancy: 18,
            recovery: Some(split_recovery.to_string_lossy().into_owned()),
        },
        SettingsDto::default(),
    );
    let verify_id = manager.submit(
        Arc::clone(&state),
        Arc::clone(&events),
        JobSpec::VerifyRecovery {
            path: archive.to_string_lossy().into_owned(),
            recovery: Some(recovery.to_string_lossy().into_owned()),
        },
        SettingsDto::default(),
    );
    let repair_id = manager.submit(
        Arc::clone(&state),
        Arc::clone(&events),
        JobSpec::RepairRecovery {
            path: archive.to_string_lossy().into_owned(),
            output: None,
            output_directory: false,
            recovery: Some(recovery.to_string_lossy().into_owned()),
        },
        SettingsDto::default(),
    );
    let copy_output = dir.join("restored.zip");
    let repair_copy_id = manager.submit(
        Arc::clone(&state),
        Arc::clone(&events),
        JobSpec::RepairRecovery {
            path: archive.to_string_lossy().into_owned(),
            output: Some(copy_output.to_string_lossy().into_owned()),
            output_directory: false,
            recovery: Some(recovery.to_string_lossy().into_owned()),
        },
        SettingsDto::default(),
    );
    let repair_set_id = manager.submit(
        Arc::clone(&state),
        Arc::clone(&events),
        JobSpec::RepairRecovery {
            path: multi_first.to_string_lossy().into_owned(),
            output: Some(multi_output.to_string_lossy().into_owned()),
            output_directory: true,
            recovery: Some(multi_recovery.to_string_lossy().into_owned()),
        },
        SettingsDto::default(),
    );
    manager.wait_idle();

    assert_eq!(std::fs::read(&copy_output).unwrap(), b"archive bytes");
    assert_eq!(
        std::fs::read(multi_output.join("set.zip.001")).unwrap(),
        b"first-volume-original\n"
    );
    assert_eq!(
        std::fs::read(multi_output.join("set.zip.002")).unwrap(),
        b"second-volume-original\n"
    );
    assert_eq!(std::fs::read(&multi_first).unwrap(), b"damaged");
    assert!(!multi_second.exists());
    assert!(std::fs::read_dir(&multi_output).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .to_ascii_lowercase()
            .ends_with(".par2")
    }));
    std::fs::write(&copy_output, b"existing repaired output\n").unwrap();
    let source_before_conflict = std::fs::read(&archive).unwrap();
    let conflict_id = manager.submit(
        Arc::clone(&state),
        Arc::clone(&events),
        JobSpec::RepairRecovery {
            path: archive.to_string_lossy().into_owned(),
            output: Some(copy_output.to_string_lossy().into_owned()),
            output_directory: false,
            recovery: Some(recovery.to_string_lossy().into_owned()),
        },
        SettingsDto::default(),
    );
    manager.wait_idle();

    let events = sink.events.lock().unwrap();
    assert_eq!(
        states_of(&events, protect_id),
        vec!["queued", "running", "done"]
    );
    assert_eq!(
        states_of(&events, split_protect_id),
        vec!["queued", "running", "done"]
    );
    assert_eq!(
        states_of(&events, verify_id),
        vec!["queued", "running", "done"]
    );
    assert_eq!(
        states_of(&events, repair_id),
        vec!["queued", "running", "done"]
    );
    assert_eq!(
        states_of(&events, repair_copy_id),
        vec!["queued", "running", "done"]
    );
    assert_eq!(
        states_of(&events, repair_set_id),
        vec!["queued", "running", "done"]
    );
    assert!(events.iter().any(|(name, payload)| {
        name == EV_PROGRESS
            && payload["id"] == protect_id
            && payload["phase"] == "recovery_process"
            && payload["interruptible"] == true
    }));
    assert!(events.iter().any(|(name, payload)| {
        name == EV_PROGRESS
            && payload["id"] == protect_id
            && payload["phase"] == "recovery_finalize"
            && payload["interruptible"] == false
    }));
    assert!(events.iter().any(|(name, payload)| {
        name == EV_PROGRESS
            && payload["id"] == verify_id
            && payload["phase"] == "recovery_verify"
            && payload["interruptible"] == true
    }));
    assert!(events.iter().any(|(name, payload)| {
        name == EV_PROGRESS
            && payload["id"] == repair_id
            && payload["phase"] == "recovery_process"
            && payload["interruptible"] == false
    }));
    assert!(events.iter().any(|(name, payload)| {
        name == EV_PROGRESS
            && payload["id"] == repair_copy_id
            && payload["phase"] == "recovery_prepare"
            && payload["interruptible"] == true
    }));
    assert!(events.iter().any(|(name, payload)| {
        name == EV_PROGRESS
            && payload["id"] == repair_copy_id
            && payload["phase"] == "recovery_verify"
            && payload["interruptible"] == false
    }));
    assert_eq!(
        states_of(&events, conflict_id),
        vec!["queued", "running", "failed"]
    );
    assert!(recovery.is_file());
    assert!(split_recovery.is_file());
    assert_eq!(
        done_result(&events, protect_id).and_then(|v| v["operation"].as_str().map(str::to_owned)),
        Some("protect".to_owned())
    );
    assert_eq!(
        done_result(&events, protect_id)
            .and_then(|value| value["outputs"].as_array().map(Vec::len)),
        Some(2)
    );
    let copy_result = done_result(&events, repair_copy_id).unwrap();
    assert_eq!(copy_result["operation"].as_str(), Some("repair"));
    assert_eq!(
        copy_result["output"].as_str(),
        Some(copy_output.to_string_lossy().as_ref())
    );
    let set_result = done_result(&events, repair_set_id).unwrap();
    assert_eq!(set_result["operation"].as_str(), Some("repair"));
    assert_eq!(set_result["source_file_count"].as_u64(), Some(2));
    assert_eq!(
        set_result["output"].as_str(),
        Some(multi_output.to_string_lossy().as_ref())
    );
    assert_eq!(std::fs::read(&archive).unwrap(), b"archive bytes");
    assert_eq!(std::fs::read(&archive).unwrap(), source_before_conflict);
    assert_eq!(
        std::fs::read(&copy_output).unwrap(),
        b"existing repaired output\n"
    );
    let conflict = events
        .iter()
        .find(|(name, payload)| {
            name == EV_STATE && payload["id"] == conflict_id && payload["state"] == "failed"
        })
        .expect("PAR2 output conflict must emit a failed state");
    assert_eq!(
        conflict.1["error"]["key"].as_str(),
        Some("error.output_exists")
    );
    assert!(std::fs::read_dir(&dir).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains(".sqz-par2-repair-")
    }));
    let log = std::fs::read_to_string(&log).unwrap();
    assert!(log.contains("create -r12"), "log: {log}");
    assert!(log.contains("create -r18"), "log: {log}");
    assert!(
        log.contains(split_first.to_string_lossy().as_ref()),
        "log: {log}"
    );
    assert!(
        log.contains(split_second.to_string_lossy().as_ref()),
        "log: {log}"
    );
    assert!(log.contains("verify"), "log: {log}");
    assert!(log.contains("repair"), "log: {log}");
    assert!(log.contains("repair -B"), "log: {log}");
    assert!(log.contains("set.zip.par2"), "log: {log}");

    std::fs::remove_dir_all(&dir).unwrap();
}
