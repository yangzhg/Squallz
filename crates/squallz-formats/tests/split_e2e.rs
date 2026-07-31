//! End-to-end split-volume tests (`.001` byte-split semantics): create
//! through the engine, reopen through any volume, detect missing volumes.

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use common::{command_exists, engine, TempDir};
use squallz_core::api::{
    ControlToken, CreateOptions, EntryPath, ExtractOptions, FormatError, NoProgress, OpenOptions,
    ProgressPhase, ProgressSink, SplitOutputMode,
};

#[derive(Default)]
struct SplitProgress {
    phases: Mutex<Vec<(ProgressPhase, bool)>>,
    events: Mutex<Vec<(u64, u64, String)>>,
}

impl ProgressSink for SplitProgress {
    fn on_progress(&self, done: u64, total: u64, current: &EntryPath) {
        self.events
            .lock()
            .unwrap()
            .push((done, total, current.display.clone()));
    }

    fn on_phase(&self, phase: ProgressPhase, interruptible: bool) {
        self.phases.lock().unwrap().push((phase, interruptible));
    }
}

struct CancelDuringSplit {
    ctl: Arc<ControlToken>,
    splitting: AtomicBool,
}

impl ProgressSink for CancelDuringSplit {
    fn on_progress(&self, done: u64, _total: u64, _current: &EntryPath) {
        if done > 0 && self.splitting.load(Ordering::Relaxed) {
            self.ctl.cancel();
        }
    }

    fn on_phase(&self, phase: ProgressPhase, _interruptible: bool) {
        if phase == ProgressPhase::OutputSplit {
            self.splitting.store(true, Ordering::Relaxed);
        }
    }
}

/// Deterministic pseudo-random (incompressible-ish) payload.
fn payload(len: usize) -> Vec<u8> {
    let mut state = 0x1234_5678u32;
    (0..len)
        .map(|_| {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (state >> 24) as u8
        })
        .collect()
}

fn system_7z() -> Option<&'static str> {
    ["7zz", "7z"].into_iter().find(|tool| command_exists(tool))
}

fn system_wimlib() -> Option<&'static str> {
    command_exists("wimlib-imagex").then_some("wimlib-imagex")
}

/// Creates `data.bin` (100 KB) under `dir` and returns its path.
fn sample_input(dir: &Path) -> PathBuf {
    sample_input_with_len(dir, 100 * 1024)
}

fn sample_input_with_len(dir: &Path, len: usize) -> PathBuf {
    let input = dir.join("data.bin");
    fs::write(&input, payload(len)).unwrap();
    input
}

fn split_archive(dir: &Path, volume_size: u64) -> PathBuf {
    let input = sample_input(dir);
    let dest = dir.join("out.zip");
    let opts = CreateOptions {
        split_size: Some(volume_size),
        ..CreateOptions::default()
    };
    engine()
        .create(&dest, &[input], &opts, &NoProgress, &ControlToken::new())
        .unwrap();
    dest
}

fn output_bytes(paths: &[PathBuf]) -> u64 {
    paths
        .iter()
        .map(|path| fs::metadata(path).unwrap().len())
        .sum()
}

#[test]
fn create_report_tracks_a_single_committed_output() {
    let tmp = TempDir::new("create-report-single");
    let input = sample_input_with_len(tmp.path(), 16 * 1024);
    let dest = tmp.path().join("out.zip");
    let engine = engine();
    let plan = engine
        .plan_create(
            &dest,
            std::slice::from_ref(&input),
            &CreateOptions::default(),
        )
        .unwrap();
    let report = engine
        .create_with_report(
            &dest,
            &[input],
            &CreateOptions::default(),
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap();

    assert_eq!(plan.primary_output, dest);
    assert_eq!(report.primary_output, dest);
    assert_eq!(report.outputs, vec![dest.clone()]);
    assert_eq!(report.split_volume_count, None);
    assert_eq!(
        report.total_output_bytes,
        fs::metadata(&dest).unwrap().len()
    );
    assert!(plan.final_output_budget_bytes >= report.total_output_bytes);
    assert!(plan.workspace_budget_bytes >= plan.final_output_budget_bytes);
}

#[test]
fn create_report_tracks_real_split_outputs_and_primary_volume() {
    let tmp = TempDir::new("create-report-split");
    let input = sample_input(tmp.path());
    let requested = tmp.path().join("out.zip.003");
    let opts = CreateOptions {
        split_size: Some(30 * 1024),
        ..CreateOptions::default()
    };
    let engine = engine();
    let progress = SplitProgress::default();
    let plan = engine
        .plan_create(&requested, std::slice::from_ref(&input), &opts)
        .unwrap();
    let report = engine
        .create_with_report(&requested, &[input], &opts, &progress, &ControlToken::new())
        .unwrap();

    let first = tmp.path().join("out.zip.001");
    assert_eq!(plan.primary_output, first);
    assert_eq!(report.primary_output, first);
    assert_eq!(report.outputs.first(), Some(&first));
    assert_eq!(report.split_volume_count, Some(report.outputs.len()));
    assert_eq!(report.total_output_bytes, output_bytes(&report.outputs));
    assert!(report.outputs.iter().all(|path| path.is_file()));
    assert!(!tmp.path().join("out.zip").exists());
    assert!(plan.final_output_budget_bytes >= report.total_output_bytes);
    assert!(plan.workspace_budget_bytes > plan.final_output_budget_bytes);

    let phases = progress.phases.lock().unwrap();
    assert!(phases.contains(&(ProgressPhase::OutputSplit, true)));
    let events = progress.events.lock().unwrap();
    let split_events = events
        .iter()
        .filter(|(_, _, current)| current.starts_with("out.zip."))
        .collect::<Vec<_>>();
    assert!(!split_events.is_empty());
    assert!(split_events
        .windows(2)
        .all(|events| events[0].0 <= events[1].0));
    assert_eq!(
        split_events.last().map(|event| (event.0, event.1)),
        Some((report.total_output_bytes, report.total_output_bytes))
    );
}

#[test]
fn cancelling_generic_split_does_not_publish_partial_volumes() {
    let tmp = TempDir::new("cancel-generic-split");
    let input = sample_input_with_len(tmp.path(), 512 * 1024);
    let dest = tmp.path().join("out.zip");
    let opts = CreateOptions {
        split_size: Some(64 * 1024),
        ..CreateOptions::default()
    };
    let ctl = ControlToken::new();
    let progress = CancelDuringSplit {
        ctl: ctl.clone(),
        splitting: AtomicBool::new(false),
    };

    let error = engine()
        .create(&dest, &[input], &opts, &progress, &ctl)
        .unwrap_err();

    assert!(matches!(error, FormatError::Cancelled));
    assert!(!dest.exists());
    assert!(!tmp.path().join("out.zip.001").exists());
    let mut remaining = fs::read_dir(tmp.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect::<Vec<_>>();
    remaining.sort();
    assert_eq!(remaining, vec![std::ffi::OsString::from("data.bin")]);
}

#[test]
fn native_zip_create_uses_pkware_names_and_primary_volume() {
    let tmp = TempDir::new("create-native-zip");
    let input = sample_input_with_len(tmp.path(), 180 * 1024);
    let dest = tmp.path().join("native.zip");
    let opts = CreateOptions {
        split_size: Some(64 * 1024),
        split_mode: SplitOutputMode::Native,
        ..CreateOptions::default()
    };
    let engine = engine();
    let plan = engine
        .plan_create(&dest, std::slice::from_ref(&input), &opts)
        .unwrap();
    let report = engine
        .create_with_report(&dest, &[input], &opts, &NoProgress, &ControlToken::new())
        .unwrap();

    assert_eq!(plan.primary_output, dest);
    assert_eq!(report.primary_output, dest);
    assert_eq!(report.outputs.last(), Some(&dest));
    assert_eq!(report.split_volume_count, Some(report.outputs.len()));
    assert!(report.outputs.len() >= 3);
    assert_eq!(report.outputs[0], tmp.path().join("native.z01"));
    assert_eq!(report.outputs[1], tmp.path().join("native.z02"));
    assert_eq!(&fs::read(&report.outputs[0]).unwrap()[..4], b"PK\x07\x08");
    assert_eq!(report.total_output_bytes, output_bytes(&report.outputs));
    assert!(plan.final_output_budget_bytes >= report.total_output_bytes);
    assert!(plan
        .split_volume_count_budget
        .is_some_and(|count| count as usize >= report.outputs.len()));

    let Some(tool) = system_7z() else {
        eprintln!("skipping external native ZIP check: no system 7zz/7z on PATH");
        return;
    };
    let check = Command::new(tool).arg("t").arg(&dest).output().unwrap();
    assert!(
        check.status.success(),
        "{tool} could not test Squallz native ZIP volumes: stdout={} stderr={}",
        String::from_utf8_lossy(&check.stdout),
        String::from_utf8_lossy(&check.stderr)
    );
}

#[test]
fn native_split_wim_create_uses_standard_names_and_primary_member() {
    let (Some(sevenz), Some(_wimlib)) = (system_7z(), system_wimlib()) else {
        eprintln!("skipping Split WIM create interop: 7zz/7z or wimlib-imagex is unavailable");
        return;
    };
    let tmp = TempDir::new("create-native-split-wim");
    let mut inputs = Vec::new();
    let mut expected = Vec::new();
    for index in 0..5u8 {
        let mut bytes = payload(1024 * 1024);
        bytes[0] ^= index;
        let path = tmp.path().join(format!("piece-{index}.bin"));
        fs::write(&path, &bytes).unwrap();
        inputs.push(path);
        expected.push(bytes);
    }
    let dest = tmp.path().join("install.swm");
    let opts = CreateOptions {
        level: squallz_core::api::CompressionLevel::Store,
        split_size: Some(2 * 1024 * 1024),
        split_mode: SplitOutputMode::Native,
        ..CreateOptions::default()
    };
    let engine = engine();
    let plan = engine
        .plan_create(&dest, &inputs, &opts)
        .unwrap_or_else(|error| panic!("Split WIM planning failed: {error}"));
    let progress = SplitProgress::default();
    let report = engine
        .create_with_report(&dest, &inputs, &opts, &progress, &ControlToken::new())
        .unwrap_or_else(|error| panic!("Split WIM creation failed: {error}"));

    assert_eq!(plan.primary_output, dest);
    assert_eq!(report.primary_output, dest);
    assert_eq!(report.outputs.first(), Some(&dest));
    assert_eq!(report.split_volume_count, Some(report.outputs.len()));
    assert!(report.outputs.len() >= 2);
    for (index, output) in report.outputs.iter().enumerate() {
        let expected_name = if index == 0 {
            "install.swm".to_owned()
        } else {
            format!("install{}.swm", index + 1)
        };
        assert_eq!(
            output.file_name().and_then(|name| name.to_str()),
            Some(expected_name.as_str())
        );
    }
    assert_eq!(report.total_output_bytes, output_bytes(&report.outputs));
    assert!(plan.final_output_budget_bytes >= report.total_output_bytes);
    assert!(plan
        .split_volume_count_budget
        .is_some_and(|count| count as usize >= report.outputs.len()));
    assert!(progress
        .phases
        .lock()
        .unwrap()
        .iter()
        .any(|(phase, _)| *phase == ProgressPhase::OutputSplit));
    let events = progress.events.lock().unwrap();
    let real_events = events
        .iter()
        .filter(|(_, total, _)| *total == report.total_output_bytes)
        .collect::<Vec<_>>();
    assert_eq!(
        real_events.last().map(|(done, total, _)| (*done, *total)),
        Some((report.total_output_bytes, report.total_output_bytes))
    );

    let selected = report
        .outputs
        .last()
        .unwrap_or_else(|| panic!("Split WIM report has no members"));
    let (_, entries, source_set) = engine
        .list_with_format_and_source_set(selected, &OpenOptions::default())
        .unwrap();
    assert!(entries
        .iter()
        .any(|entry| entry.path.display == "piece-3.bin"));
    let source_set = source_set.unwrap();
    assert_eq!(source_set.primary(), dest);
    assert_eq!(source_set.members(), report.outputs);

    let extracted = tmp.path().join("created-extracted");
    engine
        .extract(
            selected,
            &extracted,
            None,
            &OpenOptions::default(),
            &ExtractOptions::default(),
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap();
    assert_eq!(
        fs::read(extracted.join("piece-3.bin")).unwrap(),
        expected[3]
    );

    let direct_check = Command::new(sevenz).arg("t").arg(&dest).output().unwrap();
    assert!(
        direct_check.status.success(),
        "{sevenz} could not test Squallz Split WIM: stdout={} stderr={}",
        String::from_utf8_lossy(&direct_check.stdout),
        String::from_utf8_lossy(&direct_check.stderr)
    );
}

#[test]
fn native_split_wim_conversion_uses_the_first_swm_as_primary() {
    let (Some(_sevenz), Some(_wimlib)) = (system_7z(), system_wimlib()) else {
        eprintln!("skipping Split WIM conversion: 7zz/7z or wimlib-imagex is unavailable");
        return;
    };
    let tmp = TempDir::new("convert-native-split-wim");
    let inputs = (0..4u8)
        .map(|index| {
            let mut bytes = payload(1024 * 1024);
            bytes[0] ^= index;
            let path = tmp.path().join(format!("convert-{index}.bin"));
            fs::write(&path, bytes).unwrap();
            path
        })
        .collect::<Vec<_>>();
    let source = tmp.path().join("source.zip");
    let engine = engine();
    engine
        .create(
            &source,
            &inputs,
            &CreateOptions::default(),
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap();

    let destination = tmp.path().join("converted.swm");
    let options = CreateOptions {
        level: squallz_core::api::CompressionLevel::Store,
        split_size: Some(2 * 1024 * 1024),
        split_mode: SplitOutputMode::Native,
        ..CreateOptions::default()
    };
    let plan = engine
        .plan_convert(&source, &destination, &OpenOptions::default(), &options)
        .unwrap();
    let report = engine
        .convert_with_report(
            &source,
            &destination,
            &OpenOptions::default(),
            &options,
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap();

    assert_eq!(plan.primary_output, destination);
    assert_eq!(report.primary_output, destination);
    assert_eq!(report.outputs.first(), Some(&destination));
    assert!(report.outputs.len() >= 2);
    assert_eq!(report.split_volume_count, Some(report.outputs.len()));
    assert_eq!(report.total_output_bytes, output_bytes(&report.outputs));
    assert!(plan.final_output_budget_bytes >= report.total_output_bytes);
    assert!(plan
        .split_volume_count_budget
        .is_some_and(|count| count as usize >= report.outputs.len()));
    for (index, output) in report.outputs.iter().enumerate() {
        let expected = if index == 0 {
            "converted.swm".to_owned()
        } else {
            format!("converted{}.swm", index + 1)
        };
        assert_eq!(
            output.file_name().and_then(|name| name.to_str()),
            Some(expected.as_str())
        );
    }

    let selected = report
        .outputs
        .last()
        .unwrap_or_else(|| panic!("Split WIM conversion report has no members"));
    let (_, entries, source_set) = engine
        .list_with_format_and_source_set(selected, &OpenOptions::default())
        .unwrap();
    assert!(entries
        .iter()
        .any(|entry| entry.path.display == "convert-2.bin"));
    assert_eq!(
        source_set
            .unwrap_or_else(|| panic!("Split WIM conversion did not report its source set"))
            .primary(),
        destination
    );
}

#[test]
fn cancelling_native_split_wim_does_not_publish_partial_members() {
    let Some(_wimlib) = system_wimlib() else {
        eprintln!("skipping Split WIM cancellation: wimlib-imagex is unavailable");
        return;
    };
    let tmp = TempDir::new("cancel-native-split-wim");
    let input = sample_input_with_len(tmp.path(), 3 * 1024 * 1024);
    let dest = tmp.path().join("cancelled.swm");
    let opts = CreateOptions {
        level: squallz_core::api::CompressionLevel::Store,
        split_size: Some(1024 * 1024),
        split_mode: SplitOutputMode::Native,
        ..CreateOptions::default()
    };
    let ctl = ControlToken::new();
    let progress = CancelDuringSplit {
        ctl: ctl.clone(),
        splitting: AtomicBool::new(false),
    };

    let error = engine()
        .create(&dest, &[input], &opts, &progress, &ctl)
        .unwrap_err();

    assert!(matches!(error, FormatError::Cancelled));
    assert!(!dest.exists());
    assert!(!tmp.path().join("cancelled2.swm").exists());
    let mut remaining = fs::read_dir(tmp.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect::<Vec<_>>();
    remaining.sort();
    assert_eq!(remaining, vec![std::ffi::OsString::from("data.bin")]);
}

#[test]
fn native_split_wim_opens_from_any_member_and_reports_missing_parts() {
    let (Some(sevenz), Some(wimlib)) = (system_7z(), system_wimlib()) else {
        eprintln!("skipping Split WIM interop: 7zz/7z or wimlib-imagex is unavailable");
        return;
    };
    let tmp = TempDir::new("split-wim-interop");
    let source = tmp.path().join("source");
    fs::create_dir(&source).unwrap();
    let expected = payload(5 * 1024 * 1024);
    fs::write(source.join("payload.bin"), &expected).unwrap();
    fs::write(source.join("readme.txt"), b"native Split WIM\n").unwrap();
    let standalone = tmp.path().join("source.wim");
    let capture = Command::new(wimlib)
        .arg("capture")
        .arg(&source)
        .arg(&standalone)
        .arg("Squallz")
        .arg("--compress=none")
        .arg("--no-acls")
        .arg("--quiet")
        .output()
        .unwrap();
    assert!(
        capture.status.success(),
        "wimlib capture failed: stdout={} stderr={}",
        String::from_utf8_lossy(&capture.stdout),
        String::from_utf8_lossy(&capture.stderr)
    );
    let first = tmp.path().join("archive.swm");
    let split = Command::new(wimlib)
        .arg("split")
        .arg(&standalone)
        .arg(&first)
        .arg("1")
        .arg("--quiet")
        .output()
        .unwrap();
    assert!(
        split.status.success(),
        "wimlib split failed: stdout={} stderr={}",
        String::from_utf8_lossy(&split.stdout),
        String::from_utf8_lossy(&split.stderr)
    );
    let second = tmp.path().join("archive2.swm");
    let third = tmp.path().join("archive3.swm");
    assert!(second.is_file());
    assert!(third.is_file());

    let engine = engine();
    let (_, entries, source_set) = engine
        .list_with_format_and_source_set(&second, &OpenOptions::default())
        .unwrap();
    assert!(entries
        .iter()
        .any(|entry| entry.path.display == "payload.bin"));
    let source_set = source_set.unwrap();
    assert_eq!(source_set.primary(), first);
    assert_eq!(
        source_set.members(),
        [first.clone(), second.clone(), third.clone()]
    );

    let report = engine
        .test(
            &third,
            &OpenOptions::default(),
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap();
    assert!(report.is_ok());
    let extracted = tmp.path().join("extracted");
    engine
        .extract(
            &second,
            &extracted,
            None,
            &OpenOptions::default(),
            &ExtractOptions::default(),
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap();
    assert_eq!(fs::read(extracted.join("payload.bin")).unwrap(), expected);

    let saved_second = fs::read(&second).unwrap();
    fs::remove_file(&second).unwrap();
    let error = engine.list(&third, &OpenOptions::default()).unwrap_err();
    assert_eq!(error.missing_volume_path(), Some(second.as_path()));
    fs::write(second, saved_second).unwrap();

    let direct_check = Command::new(sevenz).arg("t").arg(&first).output().unwrap();
    assert!(
        direct_check.status.success(),
        "{sevenz} could not test the wimlib Split WIM: stdout={} stderr={}",
        String::from_utf8_lossy(&direct_check.stdout),
        String::from_utf8_lossy(&direct_check.stderr)
    );
}

#[test]
fn native_split_validation_rejects_unsupported_layouts_before_writing() {
    let tmp = TempDir::new("native-split-validation");
    let input = sample_input_with_len(tmp.path(), 8 * 1024);
    let engine = engine();

    let no_size = CreateOptions {
        split_mode: SplitOutputMode::Native,
        ..CreateOptions::default()
    };
    assert!(matches!(
        engine.plan_create(
            &tmp.path().join("no-size.zip"),
            std::slice::from_ref(&input),
            &no_size
        ),
        Err(FormatError::Unsupported(_))
    ));

    let unsupported_format = CreateOptions {
        split_size: Some(64 * 1024),
        split_mode: SplitOutputMode::Native,
        ..CreateOptions::default()
    };
    assert!(matches!(
        engine.plan_create(
            &tmp.path().join("unsupported.7z"),
            std::slice::from_ref(&input),
            &unsupported_format,
        ),
        Err(FormatError::Unsupported(_))
    ));

    let too_small = CreateOptions {
        split_size: Some(32 * 1024),
        split_mode: SplitOutputMode::Native,
        ..CreateOptions::default()
    };
    let output = tmp.path().join("too-small.zip");
    assert!(matches!(
        engine.plan_create(&output, &[input], &too_small),
        Err(FormatError::Unsupported(_))
    ));
    assert!(!output.exists());
}

#[test]
fn sqz_create_report_includes_recovery_sidecars_without_counting_them_as_volumes() {
    let tmp = TempDir::new("create-report-sqz");
    let input = sample_input(tmp.path());
    let dest = tmp.path().join("out.sqz");
    let stale_sidecar = tmp.path().join("out.sqz.rev999");
    fs::write(&stale_sidecar, b"stale recovery sidecar").unwrap();
    let opts = CreateOptions {
        split_size: Some(30 * 1024),
        ..CreateOptions::default()
    };
    let report = engine()
        .create_with_report(&dest, &[input], &opts, &NoProgress, &ControlToken::new())
        .unwrap();

    let volume_count = report.split_volume_count.unwrap();
    assert!(volume_count >= 4);
    assert!(report.outputs.len() > volume_count);
    assert!(report.outputs[..volume_count].iter().all(|path| path
        .file_name()
        .unwrap()
        .to_string_lossy()
        .contains(".sqz.0")));
    assert!(report.outputs[volume_count..].iter().all(|path| path
        .file_name()
        .unwrap()
        .to_string_lossy()
        .contains(".rev")));
    assert_eq!(report.total_output_bytes, output_bytes(&report.outputs));
    assert!(!stale_sidecar.exists());
}

fn volume_paths(dir: &Path, base: &str) -> Vec<PathBuf> {
    let mut paths: Vec<_> = fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .and_then(|name| name.strip_prefix(base))
                .is_some_and(|suffix| suffix.chars().all(|ch| ch.is_ascii_digit()))
        })
        .collect();
    paths.sort();
    paths
}

fn sqz_recovery_volume_path(dir: &Path, index: usize) -> PathBuf {
    dir.join(format!("out.sqz.rev{index:03}"))
}

fn corrupt_sqzr_header(path: &Path) {
    let mut bytes = fs::read(path).unwrap();
    assert!(bytes.len() >= 64);
    assert_eq!(&bytes[0..4], b"SQZR");
    bytes[12] ^= 0x5A;
    fs::write(path, bytes).unwrap();
}

fn corrupt_sqzr_payload_byte(path: &Path, physical_offset: usize) {
    let mut bytes = fs::read(path).unwrap();
    let payload_offset = 64 + physical_offset;
    assert!(payload_offset < bytes.len());
    assert_eq!(&bytes[0..4], b"SQZR");
    bytes[payload_offset] ^= 0xA5;
    fs::write(path, bytes).unwrap();
}

fn assert_open_fails_with_corrupt_archive(path: &Path, expected: &str) {
    let err = engine().list(path, &OpenOptions::default()).unwrap_err();
    match err {
        FormatError::CorruptArchive(detail) => {
            assert!(
                detail.contains(expected),
                "expected {expected:?} in detail: {detail}"
            );
        }
        other => panic!("expected CorruptArchive, got {other:?}"),
    }
}

fn assert_sqzv_header(bytes: &[u8], index: u32, total: u32) -> (u64, u64) {
    assert!(bytes.len() >= 32);
    assert_eq!(&bytes[0..4], b"SQZV");
    assert_eq!(u32::from_le_bytes(bytes[4..8].try_into().unwrap()), index);
    assert_eq!(u32::from_le_bytes(bytes[8..12].try_into().unwrap()), total);
    let uuid = (
        u64::from_le_bytes(bytes[12..20].try_into().unwrap()),
        u64::from_le_bytes(bytes[20..28].try_into().unwrap()),
    );
    assert_eq!(
        u32::from_le_bytes(bytes[28..32].try_into().unwrap()),
        crc32c::crc32c(&bytes[..28])
    );
    uuid
}

fn assert_sqz_split_flag(bytes: &[u8]) {
    let sqz_start = 32;
    assert!(bytes.len() >= sqz_start + 64);
    assert_eq!(&bytes[sqz_start..sqz_start + 8], b"SQZARCH\x1A");
    let flags = u32::from_le_bytes(bytes[sqz_start + 12..sqz_start + 16].try_into().unwrap());
    assert_ne!(flags & (1 << 3), 0, "SQZ split flag must be set");
    assert_eq!(
        u32::from_le_bytes(bytes[sqz_start + 52..sqz_start + 56].try_into().unwrap()),
        crc32c::crc32c(&bytes[sqz_start..sqz_start + 52])
    );
}

fn assert_sqzr_header(
    bytes: &[u8],
    algorithm: u16,
    total: u32,
    uuid: (u64, u64),
    physical_volume_size: u64,
    tail_physical_len: u64,
) {
    assert!(bytes.len() >= 64);
    assert_eq!(&bytes[0..4], b"SQZR");
    assert_eq!(u16::from_le_bytes(bytes[4..6].try_into().unwrap()), 1);
    assert_eq!(
        u16::from_le_bytes(bytes[6..8].try_into().unwrap()),
        algorithm
    );
    assert_eq!(u32::from_le_bytes(bytes[8..12].try_into().unwrap()), total);
    assert_eq!(
        u64::from_le_bytes(bytes[12..20].try_into().unwrap()),
        uuid.0
    );
    assert_eq!(
        u64::from_le_bytes(bytes[20..28].try_into().unwrap()),
        uuid.1
    );
    assert_eq!(
        u64::from_le_bytes(bytes[28..36].try_into().unwrap()),
        physical_volume_size
    );
    assert_eq!(
        u64::from_le_bytes(bytes[36..44].try_into().unwrap()),
        tail_physical_len
    );
    assert_eq!(
        u64::from_le_bytes(bytes[44..52].try_into().unwrap()),
        physical_volume_size
    );
    assert_eq!(
        u32::from_le_bytes(bytes[52..56].try_into().unwrap()),
        crc32c::crc32c(&bytes[..52])
    );
    assert_eq!(bytes.len() as u64, 64 + physical_volume_size);
}

#[test]
fn split_create_produces_volumes_and_roundtrips() {
    let tmp = TempDir::new("split-roundtrip");
    let dest = split_archive(tmp.path(), 30 * 1024);

    // ~100 KB of incompressible data at 30 KB per volume → 4 volumes; the
    // unsplit file must not exist.
    assert!(!dest.exists());
    let volumes: Vec<PathBuf> = (1..=4)
        .map(|i| tmp.path().join(format!("out.zip.{i:03}")))
        .collect();
    for v in &volumes[..3] {
        assert!(v.is_file(), "{} missing", v.display());
        assert_eq!(fs::metadata(v).unwrap().len(), 30 * 1024);
    }
    assert!(volumes[3].is_file());
    assert!(!tmp.path().join("out.zip.005").exists());

    // list via the first volume.
    let entries = engine().list(&volumes[0], &OpenOptions::default()).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].path.display, "data.bin");

    // Opening a middle volume resolves the same set.
    let entries2 = engine().list(&volumes[2], &OpenOptions::default()).unwrap();
    assert_eq!(entries2.len(), 1);

    // extract via .001 and compare bytes.
    let out = tmp.path().join("extracted");
    engine()
        .extract(
            &volumes[0],
            &out,
            None,
            &OpenOptions::default(),
            &ExtractOptions::default(),
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap();
    assert_eq!(fs::read(out.join("data.bin")).unwrap(), payload(100 * 1024));

    // test passes too.
    let report = engine()
        .test(
            &volumes[0],
            &OpenOptions::default(),
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap();
    assert!(report.is_ok());
}

#[test]
fn numbered_sevenz_volumes_interoperate_with_system_7zip() {
    let Some(tool) = system_7z() else {
        eprintln!("skipping: no system 7zz/7z on PATH (covered by self-read tests)");
        return;
    };
    let tmp = TempDir::new("split-7z-interop");
    let input = sample_input(tmp.path());
    let ctl = ControlToken::new();

    let ours = tmp.path().join("ours.7z");
    engine()
        .create(
            &ours,
            std::slice::from_ref(&input),
            &CreateOptions {
                split_size: Some(30 * 1024),
                ..CreateOptions::default()
            },
            &NoProgress,
            &ctl,
        )
        .unwrap();
    let ours_first = tmp.path().join("ours.7z.001");
    let check = Command::new(tool)
        .arg("t")
        .arg(&ours_first)
        .output()
        .unwrap();
    assert!(
        check.status.success(),
        "{tool} could not test Squallz volumes: stdout={} stderr={}",
        String::from_utf8_lossy(&check.stdout),
        String::from_utf8_lossy(&check.stderr)
    );

    let system_input = tmp.path().join("system-data.bin");
    fs::write(&system_input, payload(100 * 1024)).unwrap();
    let system_archive = tmp.path().join("system.7z");
    let create = Command::new(tool)
        .arg("a")
        .arg("-t7z")
        .arg("-mx=0")
        .arg("-v30k")
        .arg(&system_archive)
        .arg("system-data.bin")
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(
        create.status.success(),
        "{tool} could not create numbered volumes: stdout={} stderr={}",
        String::from_utf8_lossy(&create.stdout),
        String::from_utf8_lossy(&create.stderr)
    );

    let system_middle = tmp.path().join("system.7z.002");
    assert!(system_middle.is_file());
    let entries = engine()
        .list(&system_middle, &OpenOptions::default())
        .unwrap();
    assert!(entries
        .iter()
        .any(|entry| entry.path.display == "system-data.bin"));

    let out = tmp.path().join("system-extracted");
    engine()
        .extract(
            &system_middle,
            &out,
            None,
            &OpenOptions::default(),
            &ExtractOptions::default(),
            &NoProgress,
            &ctl,
        )
        .unwrap();
    assert_eq!(
        fs::read(out.join("system-data.bin")).unwrap(),
        payload(100 * 1024)
    );

    fs::remove_file(&system_middle).unwrap();
    let error = engine()
        .list(&tmp.path().join("system.7z.001"), &OpenOptions::default())
        .unwrap_err();
    match error {
        FormatError::CorruptArchive(detail) => assert!(
            detail.contains("system.7z.002"),
            "missing volume detail: {detail}"
        ),
        other => panic!("expected CorruptArchive, got {other:?}"),
    }
}

#[test]
fn split_rebuild_replaces_current_volumes_and_removes_unsplit_base() {
    let tmp = TempDir::new("split-rebuild");
    let input = sample_input(tmp.path());
    let dest = tmp.path().join("out.zip");
    fs::write(&dest, b"old unsplit archive").unwrap();
    fs::write(tmp.path().join("out.zip.001"), b"old first volume").unwrap();
    fs::write(tmp.path().join("out.zip.002"), b"old second volume").unwrap();
    fs::write(tmp.path().join("out.zip.999"), b"old stale volume").unwrap();

    engine()
        .create(
            &dest,
            &[input],
            &CreateOptions {
                split_size: Some(30 * 1024),
                ..CreateOptions::default()
            },
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap();

    let first = tmp.path().join("out.zip.001");
    assert!(!dest.exists());
    assert_ne!(fs::read(&first).unwrap(), b"old first volume");
    assert!(!tmp.path().join("out.zip.999").exists());
    assert_eq!(
        engine().list(&first, &OpenOptions::default()).unwrap()[0]
            .path
            .display,
        "data.bin"
    );
}

#[test]
fn split_create_rejects_a_directory_at_the_unsplit_base() {
    let tmp = TempDir::new("split-base-directory");
    let input = sample_input(tmp.path());
    let dest = tmp.path().join("out.zip");
    fs::create_dir(&dest).unwrap();

    let error = engine()
        .create(
            &dest,
            &[input],
            &CreateOptions {
                split_size: Some(30 * 1024),
                ..CreateOptions::default()
            },
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap_err();

    assert!(matches!(error, FormatError::Unsupported(_)));
    assert!(dest.is_dir());
    assert!(volume_paths(tmp.path(), "out.zip.").is_empty());
}

#[test]
fn split_create_rejects_an_abnormal_stale_volume_without_replacing_the_old_set() {
    let tmp = TempDir::new("split-stale-directory");
    let input = sample_input(tmp.path());
    let dest = tmp.path().join("out.zip");
    let first = tmp.path().join("out.zip.001");
    let stale = tmp.path().join("out.zip.999");
    fs::write(&first, b"old first volume").unwrap();
    fs::create_dir(&stale).unwrap();

    let error = engine()
        .create(
            &dest,
            &[input],
            &CreateOptions {
                split_size: Some(30 * 1024),
                ..CreateOptions::default()
            },
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap_err();

    assert!(matches!(error, FormatError::Unsupported(_)));
    assert_eq!(fs::read(&first).unwrap(), b"old first volume");
    assert!(stale.is_dir());
    assert!(!tmp.path().join("out.zip.002").exists());
}

#[cfg(windows)]
#[test]
fn split_rebuild_rolls_back_when_an_existing_volume_is_occupied() {
    use std::os::windows::fs::OpenOptionsExt;

    let tmp = TempDir::new("split-occupied-volume");
    let input = sample_input(tmp.path());
    let dest = tmp.path().join("out.zip");
    let first = tmp.path().join("out.zip.001");
    let second = tmp.path().join("out.zip.002");
    fs::write(&dest, b"old unsplit archive").unwrap();
    fs::write(&first, b"old first volume").unwrap();
    fs::write(&second, b"old second volume").unwrap();
    let occupied = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .share_mode(0)
        .open(&second)
        .unwrap();

    let error = engine()
        .create(
            &dest,
            &[input],
            &CreateOptions {
                split_size: Some(30 * 1024),
                ..CreateOptions::default()
            },
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap_err();

    drop(occupied);
    assert!(matches!(error, FormatError::Io(_)));
    assert_eq!(fs::read(&dest).unwrap(), b"old unsplit archive");
    assert_eq!(fs::read(&first).unwrap(), b"old first volume");
    assert_eq!(fs::read(&second).unwrap(), b"old second volume");
    assert!(volume_paths(tmp.path(), "out.zip.")
        .iter()
        .all(|path| path == &first || path == &second));
}

#[test]
fn missing_middle_volume_is_corrupt_with_detail() {
    let tmp = TempDir::new("split-missing");
    split_archive(tmp.path(), 30 * 1024);
    let missing = tmp.path().join("out.zip.002");
    fs::remove_file(&missing).unwrap();

    let err = engine()
        .list(&tmp.path().join("out.zip.001"), &OpenOptions::default())
        .unwrap_err();
    match err {
        FormatError::CorruptArchive(detail) => assert!(
            detail.contains("out.zip.002"),
            "detail must name the missing volume: {detail}"
        ),
        other => panic!("expected CorruptArchive, got {other:?}"),
    }
}

#[test]
fn split_works_for_compound_and_seven_z_formats() {
    let tmp = TempDir::new("split-formats");
    let input = sample_input(tmp.path());
    let ctl = ControlToken::new();
    for name in ["out.7z", "out.tar.gz"] {
        let dest = tmp.path().join(name);
        let opts = CreateOptions {
            split_size: Some(40 * 1024),
            ..CreateOptions::default()
        };
        engine()
            .create(
                &dest,
                std::slice::from_ref(&input),
                &opts,
                &NoProgress,
                &ctl,
            )
            .unwrap();
        let first = tmp.path().join(format!("{name}.001"));
        assert!(first.is_file(), "{name}: first volume missing");
        let out = tmp.path().join(format!("x-{name}"));
        engine()
            .extract(
                &first,
                &out,
                None,
                &OpenOptions::default(),
                &ExtractOptions::default(),
                &NoProgress,
                &ctl,
            )
            .unwrap();
        assert_eq!(
            fs::read(out.join("data.bin")).unwrap(),
            payload(100 * 1024),
            "{name}: extracted bytes differ"
        );
    }
}

#[test]
fn split_sqz_writes_sqzv_headers_and_roundtrips() {
    let tmp = TempDir::new("split-sqzv");
    let input = sample_input(tmp.path());
    let dest = tmp.path().join("out.sqz");
    let ctl = ControlToken::new();
    let opts = CreateOptions {
        split_size: Some(30 * 1024),
        ..CreateOptions::default()
    };
    engine()
        .create(&dest, &[input], &opts, &NoProgress, &ctl)
        .unwrap();

    assert!(!dest.exists());
    let volumes = volume_paths(tmp.path(), "out.sqz.");
    assert!(volumes.len() >= 4, "expected multiple SQZ volumes");
    let mut set_uuid = None;
    for (idx, volume) in volumes.iter().enumerate() {
        let bytes = fs::read(volume).unwrap();
        let uuid = assert_sqzv_header(&bytes, idx as u32 + 1, volumes.len() as u32);
        if idx == 0 {
            assert_sqz_split_flag(&bytes);
        }
        assert_ne!(uuid.1, 0, "SQZV uuid low word should come from SQZ header");
        if let Some(set_uuid) = set_uuid {
            assert_eq!(uuid, set_uuid, "all SQZV volumes share one container UUID");
        } else {
            set_uuid = Some(uuid);
        }
        if idx + 1 < volumes.len() {
            assert_eq!(bytes.len() as u64, 30 * 1024);
        }
    }
    let tail_mirror = sqz_recovery_volume_path(tmp.path(), volumes.len());
    let tail_bytes = fs::read(volumes.last().unwrap()).unwrap();
    let mirror_bytes = fs::read(&tail_mirror).unwrap();
    assert_eq!(
        mirror_bytes, tail_bytes,
        "tail recovery sidecar should be a validated mirror of the tail SQZV volume"
    );
    let parity = sqz_recovery_volume_path(tmp.path(), 1);
    let parity_bytes = fs::read(&parity).unwrap();
    assert_sqzr_header(
        &parity_bytes,
        1,
        volumes.len() as u32,
        set_uuid.expect("SQZV uuid captured"),
        30 * 1024,
        tail_bytes.len() as u64,
    );
    let weighted = sqz_recovery_volume_path(tmp.path(), 2);
    let weighted_bytes = fs::read(&weighted).unwrap();
    assert_sqzr_header(
        &weighted_bytes,
        2,
        volumes.len() as u32,
        set_uuid.expect("SQZV uuid captured"),
        30 * 1024,
        tail_bytes.len() as u64,
    );
    let quadratic = sqz_recovery_volume_path(tmp.path(), 3);
    let quadratic_bytes = fs::read(&quadratic).unwrap();
    assert_sqzr_header(
        &quadratic_bytes,
        3,
        volumes.len() as u32,
        set_uuid.expect("SQZV uuid captured"),
        30 * 1024,
        tail_bytes.len() as u64,
    );

    let entries = engine().list(&volumes[0], &OpenOptions::default()).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].path.display, "data.bin");

    let out = tmp.path().join("sqz-out");
    engine()
        .extract(
            &volumes[0],
            &out,
            None,
            &OpenOptions::default(),
            &ExtractOptions::default(),
            &NoProgress,
            &ctl,
        )
        .unwrap();
    assert_eq!(fs::read(out.join("data.bin")).unwrap(), payload(100 * 1024));
}

#[test]
fn split_sqz_excludes_but_preserves_fixed_parts_from_an_input_directory() {
    let tmp = TempDir::new("split-sqz-stale-parts");
    let source = tmp.path().join("source");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("data.bin"), payload(100 * 1024)).unwrap();
    let stale_volume_part = source.join("out.sqz.999.part");
    let stale_recovery_part = source.join("out.sqz.rev999.part");
    let private_volume_stage = source.join(".out.sqz.001.split-stage-123-4-0.tmp.out.sqz.001");
    let private_recovery_stage =
        source.join(".out.sqz.rev001.split-stage-123-4-0.tmp.out.sqz.rev001");
    let ordinary = source.join("out.sqz.rev000.part");
    fs::write(&stale_volume_part, b"stale volume staging").unwrap();
    fs::write(&stale_recovery_part, b"stale recovery staging").unwrap();
    fs::write(&private_volume_stage, b"private volume staging").unwrap();
    fs::write(&private_recovery_stage, b"private recovery staging").unwrap();
    fs::write(&ordinary, b"ordinary file").unwrap();

    let dest = source.join("out.sqz");
    engine()
        .create(
            &dest,
            std::slice::from_ref(&source),
            &CreateOptions {
                split_size: Some(30 * 1024),
                ..CreateOptions::default()
            },
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap();

    let names: Vec<_> = engine()
        .list(&source.join("out.sqz.001"), &OpenOptions::default())
        .unwrap()
        .into_iter()
        .map(|entry| entry.path.display)
        .collect();
    assert!(names.iter().any(|name| name == "source/data.bin"));
    assert!(names
        .iter()
        .any(|name| name == "source/out.sqz.rev000.part"));
    assert!(!names.iter().any(|name| name == "source/out.sqz.999.part"));
    assert!(!names
        .iter()
        .any(|name| name == "source/out.sqz.rev999.part"));
    assert!(!names.iter().any(|name| name.contains(".split-stage-")));
    assert_eq!(
        fs::read(stale_volume_part).unwrap(),
        b"stale volume staging"
    );
    assert_eq!(
        fs::read(stale_recovery_part).unwrap(),
        b"stale recovery staging"
    );
    assert_eq!(
        fs::read(private_volume_stage).unwrap(),
        b"private volume staging"
    );
    assert_eq!(
        fs::read(private_recovery_stage).unwrap(),
        b"private recovery staging"
    );
    assert!(ordinary.exists());
}

#[test]
fn corrupt_sqzv_header_is_reported() {
    let tmp = TempDir::new("split-sqzv-corrupt");
    let input = sample_input(tmp.path());
    let dest = tmp.path().join("out.sqz");
    let opts = CreateOptions {
        split_size: Some(40 * 1024),
        ..CreateOptions::default()
    };
    engine()
        .create(&dest, &[input], &opts, &NoProgress, &ControlToken::new())
        .unwrap();
    let first = tmp.path().join("out.sqz.001");
    let mut bytes = fs::read(&first).unwrap();
    bytes[12] ^= 0x7F;
    fs::write(&first, bytes).unwrap();

    let err = engine().list(&first, &OpenOptions::default()).unwrap_err();
    match err {
        FormatError::CorruptArchive(detail) => {
            assert!(detail.contains("SQZV"), "detail should name SQZV: {detail}");
        }
        other => panic!("expected CorruptArchive, got {other:?}"),
    }
}

#[test]
fn sqzv_uuid_mismatch_is_reported() {
    let tmp = TempDir::new("split-sqzv-uuid-mismatch");
    let input = sample_input(tmp.path());
    let dest = tmp.path().join("out.sqz");
    let opts = CreateOptions {
        split_size: Some(40 * 1024),
        ..CreateOptions::default()
    };
    engine()
        .create(&dest, &[input], &opts, &NoProgress, &ControlToken::new())
        .unwrap();

    let first = tmp.path().join("out.sqz.001");
    let second = tmp.path().join("out.sqz.002");
    let mut bytes = fs::read(&second).unwrap();
    bytes[20] ^= 0x5A;
    let crc = crc32c::crc32c(&bytes[..28]);
    bytes[28..32].copy_from_slice(&crc.to_le_bytes());
    fs::write(&second, bytes).unwrap();

    let err = engine().list(&first, &OpenOptions::default()).unwrap_err();
    match err {
        FormatError::CorruptArchive(detail) => {
            assert!(detail.contains("UUID"), "detail should name UUID: {detail}");
        }
        other => panic!("expected CorruptArchive, got {other:?}"),
    }
}

#[test]
fn missing_sqzv_payload_volume_recovers_when_within_rs_capacity() {
    let tmp = TempDir::new("split-sqzv-missing-repairable");
    let input = sample_input(tmp.path());
    let dest = tmp.path().join("out.sqz");
    let opts = CreateOptions {
        split_size: Some(30 * 1024),
        ..CreateOptions::default()
    };
    let ctl = ControlToken::new();
    engine()
        .create(&dest, &[input], &opts, &NoProgress, &ctl)
        .unwrap();

    let missing = tmp.path().join("out.sqz.002");
    fs::remove_file(&missing).unwrap();
    fs::remove_file(sqz_recovery_volume_path(tmp.path(), 1)).unwrap();

    let first = tmp.path().join("out.sqz.001");
    let entries = engine().list(&first, &OpenOptions::default()).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].path.display, "data.bin");

    let report = engine()
        .test(&first, &OpenOptions::default(), &NoProgress, &ctl)
        .unwrap();
    assert!(report.is_ok(), "problems: {:?}", report.problems);

    let out = tmp.path().join("sqzv-recovered");
    engine()
        .extract(
            &first,
            &out,
            None,
            &OpenOptions::default(),
            &ExtractOptions::default(),
            &NoProgress,
            &ctl,
        )
        .unwrap();
    assert_eq!(fs::read(out.join("data.bin")).unwrap(), payload(100 * 1024));
}

#[test]
fn missing_sqzv_payload_volume_recovers_from_rev_parity_when_rs_capacity_exceeded() {
    let tmp = TempDir::new("split-sqzv-missing-parity");
    let input_len = 700 * 1024;
    let input = sample_input_with_len(tmp.path(), input_len);
    let dest = tmp.path().join("out.sqz");
    let opts = CreateOptions {
        split_size: Some(180 * 1024),
        ..CreateOptions::default()
    };
    let ctl = ControlToken::new();
    engine()
        .create(&dest, &[input], &opts, &NoProgress, &ctl)
        .unwrap();

    assert!(sqz_recovery_volume_path(tmp.path(), 1).is_file());
    fs::remove_file(tmp.path().join("out.sqz.002")).unwrap();

    let first = tmp.path().join("out.sqz.001");
    let entries = engine().list(&first, &OpenOptions::default()).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].path.display, "data.bin");

    let report = engine()
        .test(&first, &OpenOptions::default(), &NoProgress, &ctl)
        .unwrap();
    assert!(report.is_ok(), "problems: {:?}", report.problems);

    let out = tmp.path().join("sqzv-parity-recovered");
    engine()
        .extract(
            &first,
            &out,
            None,
            &OpenOptions::default(),
            &ExtractOptions::default(),
            &NoProgress,
            &ctl,
        )
        .unwrap();
    assert_eq!(fs::read(out.join("data.bin")).unwrap(), payload(input_len));
}

#[test]
fn missing_two_sqzv_payload_volumes_recover_from_dual_rev_parity() {
    let tmp = TempDir::new("split-sqzv-two-missing-dual-parity");
    let input_len = 700 * 1024;
    let input = sample_input_with_len(tmp.path(), input_len);
    let dest = tmp.path().join("out.sqz");
    let opts = CreateOptions {
        split_size: Some(180 * 1024),
        ..CreateOptions::default()
    };
    let ctl = ControlToken::new();
    engine()
        .create(&dest, &[input], &opts, &NoProgress, &ctl)
        .unwrap();

    assert!(sqz_recovery_volume_path(tmp.path(), 1).is_file());
    assert!(sqz_recovery_volume_path(tmp.path(), 2).is_file());
    fs::remove_file(tmp.path().join("out.sqz.002")).unwrap();
    fs::remove_file(tmp.path().join("out.sqz.003")).unwrap();

    let first = tmp.path().join("out.sqz.001");
    let entries = engine().list(&first, &OpenOptions::default()).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].path.display, "data.bin");

    let report = engine()
        .test(&first, &OpenOptions::default(), &NoProgress, &ctl)
        .unwrap();
    assert!(report.is_ok(), "problems: {:?}", report.problems);

    let out = tmp.path().join("sqzv-two-missing-recovered");
    engine()
        .extract(
            &first,
            &out,
            None,
            &OpenOptions::default(),
            &ExtractOptions::default(),
            &NoProgress,
            &ctl,
        )
        .unwrap();
    assert_eq!(fs::read(out.join("data.bin")).unwrap(), payload(input_len));
}

#[test]
fn missing_sqzv_payload_volume_fails_with_damaged_rev001_header() {
    let tmp = TempDir::new("split-sqzv-damaged-rev001-header");
    let input = sample_input_with_len(tmp.path(), 700 * 1024);
    let dest = tmp.path().join("out.sqz");
    let opts = CreateOptions {
        split_size: Some(180 * 1024),
        ..CreateOptions::default()
    };
    engine()
        .create(&dest, &[input], &opts, &NoProgress, &ControlToken::new())
        .unwrap();

    fs::remove_file(tmp.path().join("out.sqz.002")).unwrap();
    corrupt_sqzr_header(&sqz_recovery_volume_path(tmp.path(), 1));

    assert_open_fails_with_corrupt_archive(
        &tmp.path().join("out.sqz.001"),
        "SQZ recovery volume header CRC-32C mismatch",
    );
}

#[test]
fn missing_sqzv_payload_volume_fails_with_damaged_rev001_payload() {
    let tmp = TempDir::new("split-sqzv-damaged-rev001-payload");
    let input = sample_input_with_len(tmp.path(), 700 * 1024);
    let dest = tmp.path().join("out.sqz");
    let opts = CreateOptions {
        split_size: Some(180 * 1024),
        ..CreateOptions::default()
    };
    engine()
        .create(&dest, &[input], &opts, &NoProgress, &ControlToken::new())
        .unwrap();

    fs::remove_file(tmp.path().join("out.sqz.002")).unwrap();
    corrupt_sqzr_payload_byte(&sqz_recovery_volume_path(tmp.path(), 1), 0);

    assert_open_fails_with_corrupt_archive(&tmp.path().join("out.sqz.001"), "SQZV");
}

#[test]
fn missing_two_sqzv_payload_volumes_fail_with_damaged_rev002_header() {
    let tmp = TempDir::new("split-sqzv-damaged-rev002-header");
    let input = sample_input_with_len(tmp.path(), 700 * 1024);
    let dest = tmp.path().join("out.sqz");
    let opts = CreateOptions {
        split_size: Some(180 * 1024),
        ..CreateOptions::default()
    };
    engine()
        .create(&dest, &[input], &opts, &NoProgress, &ControlToken::new())
        .unwrap();

    fs::remove_file(tmp.path().join("out.sqz.002")).unwrap();
    fs::remove_file(tmp.path().join("out.sqz.003")).unwrap();
    corrupt_sqzr_header(&sqz_recovery_volume_path(tmp.path(), 2));

    assert_open_fails_with_corrupt_archive(
        &tmp.path().join("out.sqz.001"),
        "SQZ recovery volume header CRC-32C mismatch",
    );
}

#[test]
fn missing_two_sqzv_payload_volumes_fail_with_damaged_rev002_payload() {
    let tmp = TempDir::new("split-sqzv-damaged-rev002-payload");
    let input = sample_input_with_len(tmp.path(), 700 * 1024);
    let dest = tmp.path().join("out.sqz");
    let opts = CreateOptions {
        split_size: Some(180 * 1024),
        ..CreateOptions::default()
    };
    engine()
        .create(&dest, &[input], &opts, &NoProgress, &ControlToken::new())
        .unwrap();

    fs::remove_file(tmp.path().join("out.sqz.002")).unwrap();
    fs::remove_file(tmp.path().join("out.sqz.003")).unwrap();
    corrupt_sqzr_payload_byte(&sqz_recovery_volume_path(tmp.path(), 2), 0);

    assert_open_fails_with_corrupt_archive(&tmp.path().join("out.sqz.001"), "SQZV");
}

#[test]
fn missing_three_sqzv_payload_volumes_fail_with_damaged_rev003_header() {
    let tmp = TempDir::new("split-sqzv-damaged-rev003-header");
    let input = sample_input_with_len(tmp.path(), 900 * 1024);
    let dest = tmp.path().join("out.sqz");
    let opts = CreateOptions {
        split_size: Some(180 * 1024),
        ..CreateOptions::default()
    };
    engine()
        .create(&dest, &[input], &opts, &NoProgress, &ControlToken::new())
        .unwrap();

    fs::remove_file(tmp.path().join("out.sqz.002")).unwrap();
    fs::remove_file(tmp.path().join("out.sqz.003")).unwrap();
    fs::remove_file(tmp.path().join("out.sqz.004")).unwrap();
    corrupt_sqzr_header(&sqz_recovery_volume_path(tmp.path(), 3));

    assert_open_fails_with_corrupt_archive(
        &tmp.path().join("out.sqz.001"),
        "SQZ recovery volume header CRC-32C mismatch",
    );
}

#[test]
fn missing_three_sqzv_payload_volumes_fail_with_damaged_rev003_payload() {
    let tmp = TempDir::new("split-sqzv-damaged-rev003-payload");
    let input = sample_input_with_len(tmp.path(), 900 * 1024);
    let dest = tmp.path().join("out.sqz");
    let opts = CreateOptions {
        split_size: Some(180 * 1024),
        ..CreateOptions::default()
    };
    engine()
        .create(&dest, &[input], &opts, &NoProgress, &ControlToken::new())
        .unwrap();

    fs::remove_file(tmp.path().join("out.sqz.002")).unwrap();
    fs::remove_file(tmp.path().join("out.sqz.003")).unwrap();
    fs::remove_file(tmp.path().join("out.sqz.004")).unwrap();
    corrupt_sqzr_payload_byte(&sqz_recovery_volume_path(tmp.path(), 3), 0);

    assert_open_fails_with_corrupt_archive(&tmp.path().join("out.sqz.001"), "SQZV");
}

#[test]
fn missing_two_sqzv_payload_volumes_fail_without_dual_rev_parity() {
    let tmp = TempDir::new("split-sqzv-two-missing-no-dual-parity");
    let input_len = 700 * 1024;
    let input = sample_input_with_len(tmp.path(), input_len);
    let dest = tmp.path().join("out.sqz");
    let opts = CreateOptions {
        split_size: Some(180 * 1024),
        ..CreateOptions::default()
    };
    let ctl = ControlToken::new();
    engine()
        .create(&dest, &[input], &opts, &NoProgress, &ctl)
        .unwrap();

    fs::remove_file(tmp.path().join("out.sqz.002")).unwrap();
    fs::remove_file(tmp.path().join("out.sqz.003")).unwrap();
    fs::remove_file(sqz_recovery_volume_path(tmp.path(), 2)).unwrap();

    let first = tmp.path().join("out.sqz.001");
    let report = engine()
        .test(&first, &OpenOptions::default(), &NoProgress, &ctl)
        .unwrap();
    assert!(
        !report.is_ok(),
        "two missing large volumes require the weighted parity sidecar"
    );

    let out = tmp.path().join("sqzv-two-missing-no-dual");
    let err = engine()
        .extract(
            &first,
            &out,
            None,
            &OpenOptions::default(),
            &ExtractOptions::default(),
            &NoProgress,
            &ctl,
        )
        .unwrap_err();
    match err {
        FormatError::CorruptArchive(detail) => {
            assert!(
                detail.contains("unrepaired") || detail.contains("block"),
                "{detail}"
            );
        }
        other => panic!("expected CorruptArchive, got {other:?}"),
    }
}

#[test]
fn missing_three_sqzv_payload_volumes_recover_from_triple_rev_parity() {
    let tmp = TempDir::new("split-sqzv-three-missing-triple-parity");
    let input_len = 1_000 * 1024;
    let input = sample_input_with_len(tmp.path(), input_len);
    let dest = tmp.path().join("out.sqz");
    let opts = CreateOptions {
        split_size: Some(180 * 1024),
        ..CreateOptions::default()
    };
    let ctl = ControlToken::new();
    engine()
        .create(&dest, &[input], &opts, &NoProgress, &ctl)
        .unwrap();

    assert!(sqz_recovery_volume_path(tmp.path(), 1).is_file());
    assert!(sqz_recovery_volume_path(tmp.path(), 2).is_file());
    assert!(sqz_recovery_volume_path(tmp.path(), 3).is_file());
    fs::remove_file(tmp.path().join("out.sqz.002")).unwrap();
    fs::remove_file(tmp.path().join("out.sqz.003")).unwrap();
    fs::remove_file(tmp.path().join("out.sqz.004")).unwrap();

    let first = tmp.path().join("out.sqz.001");
    let entries = engine().list(&first, &OpenOptions::default()).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].path.display, "data.bin");

    let report = engine()
        .test(&first, &OpenOptions::default(), &NoProgress, &ctl)
        .unwrap();
    assert!(report.is_ok(), "problems: {:?}", report.problems);

    let out = tmp.path().join("sqzv-three-missing-recovered");
    engine()
        .extract(
            &first,
            &out,
            None,
            &OpenOptions::default(),
            &ExtractOptions::default(),
            &NoProgress,
            &ctl,
        )
        .unwrap();
    assert_eq!(fs::read(out.join("data.bin")).unwrap(), payload(input_len));
}

#[test]
fn missing_three_sqzv_payload_volumes_fail_without_triple_rev_parity() {
    let tmp = TempDir::new("split-sqzv-three-missing-no-triple-parity");
    let input_len = 1_000 * 1024;
    let input = sample_input_with_len(tmp.path(), input_len);
    let dest = tmp.path().join("out.sqz");
    let opts = CreateOptions {
        split_size: Some(180 * 1024),
        ..CreateOptions::default()
    };
    let ctl = ControlToken::new();
    engine()
        .create(&dest, &[input], &opts, &NoProgress, &ctl)
        .unwrap();

    fs::remove_file(tmp.path().join("out.sqz.002")).unwrap();
    fs::remove_file(tmp.path().join("out.sqz.003")).unwrap();
    fs::remove_file(tmp.path().join("out.sqz.004")).unwrap();
    fs::remove_file(sqz_recovery_volume_path(tmp.path(), 3)).unwrap();

    let first = tmp.path().join("out.sqz.001");
    let report = engine()
        .test(&first, &OpenOptions::default(), &NoProgress, &ctl)
        .unwrap();
    assert!(
        !report.is_ok(),
        "three missing large volumes require the quadratic parity sidecar"
    );

    let out = tmp.path().join("sqzv-three-missing-no-triple");
    let err = engine()
        .extract(
            &first,
            &out,
            None,
            &OpenOptions::default(),
            &ExtractOptions::default(),
            &NoProgress,
            &ctl,
        )
        .unwrap_err();
    match err {
        FormatError::CorruptArchive(detail) => {
            assert!(
                detail.contains("unrepaired") || detail.contains("block"),
                "{detail}"
            );
        }
        other => panic!("expected CorruptArchive, got {other:?}"),
    }
}

#[test]
fn missing_four_sqzv_payload_volumes_still_fail_with_three_rev_parity() {
    let tmp = TempDir::new("split-sqzv-four-missing");
    let input_len = 1_200 * 1024;
    let input = sample_input_with_len(tmp.path(), input_len);
    let dest = tmp.path().join("out.sqz");
    let opts = CreateOptions {
        split_size: Some(180 * 1024),
        ..CreateOptions::default()
    };
    let ctl = ControlToken::new();
    engine()
        .create(&dest, &[input], &opts, &NoProgress, &ctl)
        .unwrap();

    assert!(sqz_recovery_volume_path(tmp.path(), 1).is_file());
    assert!(sqz_recovery_volume_path(tmp.path(), 2).is_file());
    assert!(sqz_recovery_volume_path(tmp.path(), 3).is_file());
    fs::remove_file(tmp.path().join("out.sqz.002")).unwrap();
    fs::remove_file(tmp.path().join("out.sqz.003")).unwrap();
    fs::remove_file(tmp.path().join("out.sqz.004")).unwrap();
    fs::remove_file(tmp.path().join("out.sqz.005")).unwrap();

    let first = tmp.path().join("out.sqz.001");
    let report = engine()
        .test(&first, &OpenOptions::default(), &NoProgress, &ctl)
        .unwrap();
    assert!(
        !report.is_ok(),
        "four missing large volumes must not be over-claimed as recoverable"
    );
}

#[test]
fn missing_sqzv_payload_volume_over_capacity_fails_without_rev_parity() {
    let tmp = TempDir::new("split-sqzv-missing-no-parity");
    let input_len = 700 * 1024;
    let input = sample_input_with_len(tmp.path(), input_len);
    let dest = tmp.path().join("out.sqz");
    let opts = CreateOptions {
        split_size: Some(180 * 1024),
        ..CreateOptions::default()
    };
    let ctl = ControlToken::new();
    engine()
        .create(&dest, &[input], &opts, &NoProgress, &ctl)
        .unwrap();

    fs::remove_file(tmp.path().join("out.sqz.002")).unwrap();
    fs::remove_file(sqz_recovery_volume_path(tmp.path(), 1)).unwrap();

    let first = tmp.path().join("out.sqz.001");
    let report = engine()
        .test(&first, &OpenOptions::default(), &NoProgress, &ctl)
        .unwrap();
    assert!(
        !report.is_ok(),
        "missing large volume must exceed embedded RS capacity"
    );

    let out = tmp.path().join("sqzv-no-parity");
    let err = engine()
        .extract(
            &first,
            &out,
            None,
            &OpenOptions::default(),
            &ExtractOptions::default(),
            &NoProgress,
            &ctl,
        )
        .unwrap_err();
    match err {
        FormatError::CorruptArchive(detail) => {
            assert!(
                detail.contains("unrepaired") || detail.contains("block"),
                "{detail}"
            );
        }
        other => panic!("expected CorruptArchive, got {other:?}"),
    }
}

#[test]
fn missing_sqzv_tail_volume_recovers_from_rev_sidecar() {
    let tmp = TempDir::new("split-sqzv-missing-tail-sidecar");
    let input = sample_input(tmp.path());
    let dest = tmp.path().join("out.sqz");
    let opts = CreateOptions {
        split_size: Some(30 * 1024),
        ..CreateOptions::default()
    };
    let ctl = ControlToken::new();
    engine()
        .create(&dest, &[input], &opts, &NoProgress, &ctl)
        .unwrap();

    let volumes = volume_paths(tmp.path(), "out.sqz.");
    let tail = volumes.last().expect("tail volume").clone();
    let tail_mirror = sqz_recovery_volume_path(tmp.path(), volumes.len());
    assert!(tail_mirror.is_file(), "{} missing", tail_mirror.display());
    assert!(sqz_recovery_volume_path(tmp.path(), 1).is_file());
    fs::remove_file(&tail).unwrap();

    let first = tmp.path().join("out.sqz.001");
    let entries = engine().list(&first, &OpenOptions::default()).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].path.display, "data.bin");

    let report = engine()
        .test(&first, &OpenOptions::default(), &NoProgress, &ctl)
        .unwrap();
    assert!(report.is_ok(), "problems: {:?}", report.problems);

    let out = tmp.path().join("sqzv-tail-recovered");
    engine()
        .extract(
            &first,
            &out,
            None,
            &OpenOptions::default(),
            &ExtractOptions::default(),
            &NoProgress,
            &ctl,
        )
        .unwrap();
    assert_eq!(fs::read(out.join("data.bin")).unwrap(), payload(100 * 1024));
}

#[test]
fn missing_sqzv_payload_and_tail_recover_from_parity_plus_tail_mirror() {
    let tmp = TempDir::new("split-sqzv-missing-payload-tail");
    let input = sample_input(tmp.path());
    let dest = tmp.path().join("out.sqz");
    let opts = CreateOptions {
        split_size: Some(30 * 1024),
        ..CreateOptions::default()
    };
    let ctl = ControlToken::new();
    engine()
        .create(&dest, &[input], &opts, &NoProgress, &ctl)
        .unwrap();

    let volumes = volume_paths(tmp.path(), "out.sqz.");
    assert!(volumes.len() >= 4, "volumes: {volumes:?}");
    let tail = volumes.last().expect("tail volume").clone();
    let tail_mirror = sqz_recovery_volume_path(tmp.path(), volumes.len());
    assert!(tail_mirror.is_file(), "{} missing", tail_mirror.display());
    assert!(sqz_recovery_volume_path(tmp.path(), 1).is_file());

    fs::remove_file(tmp.path().join("out.sqz.002")).unwrap();
    fs::remove_file(&tail).unwrap();

    let first = tmp.path().join("out.sqz.001");
    let entries = engine().list(&first, &OpenOptions::default()).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].path.display, "data.bin");

    let report = engine()
        .test(&first, &OpenOptions::default(), &NoProgress, &ctl)
        .unwrap();
    assert!(report.is_ok(), "problems: {:?}", report.problems);

    let out = tmp.path().join("sqzv-payload-tail-recovered");
    engine()
        .extract(
            &first,
            &out,
            None,
            &OpenOptions::default(),
            &ExtractOptions::default(),
            &NoProgress,
            &ctl,
        )
        .unwrap();
    assert_eq!(fs::read(out.join("data.bin")).unwrap(), payload(100 * 1024));
}

#[test]
fn missing_sqzv_tail_volume_is_still_unrecoverable() {
    let tmp = TempDir::new("split-sqzv-missing-tail");
    let input = sample_input(tmp.path());
    let dest = tmp.path().join("out.sqz");
    let opts = CreateOptions {
        split_size: Some(30 * 1024),
        ..CreateOptions::default()
    };
    engine()
        .create(&dest, &[input], &opts, &NoProgress, &ControlToken::new())
        .unwrap();

    let volumes = volume_paths(tmp.path(), "out.sqz.");
    let tail = volumes.last().expect("tail volume").clone();
    let tail_mirror = sqz_recovery_volume_path(tmp.path(), volumes.len());
    let parity = sqz_recovery_volume_path(tmp.path(), 1);
    fs::remove_file(&tail).unwrap();
    fs::remove_file(&tail_mirror).unwrap();
    fs::remove_file(&parity).unwrap();

    let err = engine()
        .list(&tmp.path().join("out.sqz.001"), &OpenOptions::default())
        .unwrap_err();
    match err {
        FormatError::CorruptArchive(detail) => {
            assert!(detail.contains("tail volume"), "detail: {detail}");
        }
        other => panic!("expected CorruptArchive, got {other:?}"),
    }
}

#[test]
fn tiny_split_size_is_rejected() {
    let tmp = TempDir::new("split-tiny");
    let input = sample_input(tmp.path());
    let opts = CreateOptions {
        split_size: Some(64),
        ..CreateOptions::default()
    };
    let err = engine()
        .create(
            &tmp.path().join("out.zip"),
            &[input],
            &opts,
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap_err();
    assert!(matches!(err, FormatError::Unsupported(_)));
}
