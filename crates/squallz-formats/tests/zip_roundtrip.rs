//! ZIP end-to-end round-trip and interoperability tests
//! (create → list → extract → test, system zip/unzip interop, corrupt
//! inputs, ZIP64).

mod common;

use std::fs;
use std::io::Read;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use common::{build_stored_zip, command_exists, crc32, engine, RawZipEntry, TempDir};
use squallz_format_api::{
    CompressionLevel, ControlToken, CreateOptions, Detected, EntryMeta, EntryPath, EntryType,
    ExtractOptions, ExtractProblemReporter, FormatError, NoProgress, OpenOptions, OverwritePolicy,
    ProgressPhase, ProgressSink,
};

#[derive(Default)]
struct SkippedCollector {
    items: Mutex<Vec<String>>,
}

impl ExtractProblemReporter for SkippedCollector {
    fn skipped_entry(&self, path: &EntryPath, error: &FormatError) {
        self.items
            .lock()
            .unwrap()
            .push(format!("{}: {error}", path.display));
    }
}

struct CancelOnUnexpectedEntry<'a> {
    ctl: Arc<ControlToken>,
    allowed: &'a [&'a str],
    monitor_input_progress: AtomicBool,
    saw_unexpected: AtomicBool,
}

impl ProgressSink for CancelOnUnexpectedEntry<'_> {
    fn on_progress(&self, _done: u64, _total: u64, current: &EntryPath) {
        if self.monitor_input_progress.load(Ordering::Relaxed)
            && !current.display.is_empty()
            && !self.allowed.contains(&current.display.as_str())
        {
            self.saw_unexpected.store(true, Ordering::Relaxed);
            self.ctl.cancel();
        }
    }

    fn on_phase(&self, phase: ProgressPhase, _interruptible: bool) {
        if matches!(
            phase,
            ProgressPhase::OutputSplit | ProgressPhase::OutputVerify
        ) {
            self.monitor_input_progress.store(false, Ordering::Relaxed);
        }
    }
}

fn build_stored_zip_with_data_descriptor(entries: &[RawZipEntry], signed: bool) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for entry in entries {
        let offset = out.len() as u32;
        let crc = crc32(&entry.data);
        let size = entry.data.len() as u32;
        let name_len = entry.name.len() as u16;

        out.extend_from_slice(&[0x50, 0x4B, 0x03, 0x04]);
        out.extend_from_slice(&20u16.to_le_bytes());
        out.extend_from_slice(&0x08u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0x21u16.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&name_len.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&entry.name);
        out.extend_from_slice(&entry.data);
        if signed {
            out.extend_from_slice(&[0x50, 0x4B, 0x07, 0x08]);
        }
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());

        central.extend_from_slice(&[0x50, 0x4B, 0x01, 0x02]);
        central.extend_from_slice(&20u16.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes());
        central.extend_from_slice(&0x08u16.to_le_bytes());
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
        central.extend_from_slice(&entry.name);
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

fn build_stored_zip64_with_data_descriptor(entries: &[RawZipEntry], signed: bool) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for entry in entries {
        let offset = out.len() as u32;
        let crc = crc32(&entry.data);
        let size = entry.data.len() as u64;
        let name_len = entry.name.len() as u16;

        out.extend_from_slice(&[0x50, 0x4B, 0x03, 0x04]);
        out.extend_from_slice(&45u16.to_le_bytes());
        out.extend_from_slice(&0x08u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0x21u16.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&u32::MAX.to_le_bytes());
        out.extend_from_slice(&u32::MAX.to_le_bytes());
        out.extend_from_slice(&name_len.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&entry.name);
        out.extend_from_slice(&entry.data);
        if signed {
            out.extend_from_slice(&[0x50, 0x4B, 0x07, 0x08]);
        }
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());

        central.extend_from_slice(&[0x50, 0x4B, 0x01, 0x02]);
        central.extend_from_slice(&45u16.to_le_bytes());
        central.extend_from_slice(&45u16.to_le_bytes());
        central.extend_from_slice(&0x08u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0x21u16.to_le_bytes());
        central.extend_from_slice(&crc.to_le_bytes());
        central.extend_from_slice(&u32::MAX.to_le_bytes());
        central.extend_from_slice(&u32::MAX.to_le_bytes());
        central.extend_from_slice(&name_len.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u32.to_le_bytes());
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(&entry.name);
        central.extend_from_slice(&0x0001u16.to_le_bytes());
        central.extend_from_slice(&16u16.to_le_bytes());
        central.extend_from_slice(&size.to_le_bytes());
        central.extend_from_slice(&size.to_le_bytes());
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

fn build_stored_zip64_local_header_without_central_directory(entry: &RawZipEntry) -> Vec<u8> {
    let mut out = Vec::new();
    let crc = crc32(&entry.data);
    let size = entry.data.len() as u64;
    let name_len = entry.name.len() as u16;
    let zip64_extra_len = 4 + 16;

    out.extend_from_slice(&[0x50, 0x4B, 0x03, 0x04]);
    out.extend_from_slice(&45u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0x21u16.to_le_bytes());
    out.extend_from_slice(&crc.to_le_bytes());
    out.extend_from_slice(&u32::MAX.to_le_bytes());
    out.extend_from_slice(&u32::MAX.to_le_bytes());
    out.extend_from_slice(&name_len.to_le_bytes());
    out.extend_from_slice(&(zip64_extra_len as u16).to_le_bytes());
    out.extend_from_slice(&entry.name);
    out.extend_from_slice(&0x0001u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(&size.to_le_bytes());
    out.extend_from_slice(&size.to_le_bytes());
    out.extend_from_slice(&entry.data);
    out
}

fn build_encrypted_flag_stored_zip_without_central_directory(entry: &RawZipEntry) -> Vec<u8> {
    let mut out = Vec::new();
    let crc = crc32(&entry.data);
    let size = entry.data.len() as u32;
    let name_len = entry.name.len() as u16;

    out.extend_from_slice(&[0x50, 0x4B, 0x03, 0x04]);
    out.extend_from_slice(&20u16.to_le_bytes());
    out.extend_from_slice(&0x01u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0x21u16.to_le_bytes());
    out.extend_from_slice(&crc.to_le_bytes());
    out.extend_from_slice(&size.to_le_bytes());
    out.extend_from_slice(&size.to_le_bytes());
    out.extend_from_slice(&name_len.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&entry.name);
    out.extend_from_slice(&entry.data);
    out
}

fn build_unsupported_method_zip_without_central_directory(
    entry: &RawZipEntry,
    method: u16,
) -> Vec<u8> {
    let mut out = Vec::new();
    let crc = crc32(&entry.data);
    let size = entry.data.len() as u32;
    let name_len = entry.name.len() as u16;

    out.extend_from_slice(&[0x50, 0x4B, 0x03, 0x04]);
    out.extend_from_slice(&20u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&method.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0x21u16.to_le_bytes());
    out.extend_from_slice(&crc.to_le_bytes());
    out.extend_from_slice(&size.to_le_bytes());
    out.extend_from_slice(&size.to_le_bytes());
    out.extend_from_slice(&name_len.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&entry.name);
    out.extend_from_slice(&entry.data);
    out
}

/// Builds a fixture tree exercising Chinese names, deep nesting, an empty
/// directory, executable permissions and a symlink.
fn build_fixture_tree(root: &std::path::Path) {
    let project = root.join("project");
    fs::create_dir_all(project.join("deep/a/b/c/d")).unwrap();
    fs::create_dir_all(project.join("empty_dir")).unwrap();
    fs::create_dir_all(project.join("中文目录")).unwrap();
    fs::write(project.join("a.txt"), b"hello world").unwrap();
    fs::write(project.join("deep/a/b/c/d/file.bin"), vec![0xAB; 4096]).unwrap();
    fs::write(project.join("中文目录/中文文件.txt"), "你好，世界").unwrap();
    fs::write(project.join("script.sh"), b"#!/bin/sh\necho hi\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(project.join("script.sh"), fs::Permissions::from_mode(0o755)).unwrap();
        std::os::unix::fs::symlink("a.txt", project.join("link")).unwrap();
    }
}

#[test]
fn create_excludes_existing_output_and_inner_temp_from_multiple_inputs() {
    let tmp = TempDir::new("create-output-exclusion");
    let project = tmp.path().join("project");
    let nested = project.join("nested");
    fs::create_dir_all(&nested).unwrap();
    fs::write(project.join("keep.txt"), b"keep me").unwrap();
    fs::write(project.join("archive.zip.notes"), b"ordinary file").unwrap();
    let outside = tmp.path().join("outside.txt");
    fs::write(&outside, b"second input").unwrap();

    let archive = nested.join("..").join("archive.zip");
    fs::write(&archive, b"stale archive").unwrap();
    let ctl = ControlToken::new();
    let progress = CancelOnUnexpectedEntry {
        ctl: Arc::clone(&ctl),
        allowed: &[
            "project",
            "project/archive.zip.notes",
            "project/keep.txt",
            "project/nested",
            "outside.txt",
        ],
        monitor_input_progress: AtomicBool::new(true),
        saw_unexpected: AtomicBool::new(false),
    };
    engine()
        .create(
            &archive,
            &[project, outside],
            &CreateOptions::default(),
            &progress,
            &ctl,
        )
        .unwrap();

    assert!(!progress.saw_unexpected.load(Ordering::Relaxed));
    let entries = engine().list(&archive, &OpenOptions::default()).unwrap();
    let names: Vec<_> = entries
        .iter()
        .map(|entry| entry.path.display.as_str())
        .collect();
    assert!(names.contains(&"project/keep.txt"), "names: {names:?}");
    assert!(
        names.contains(&"project/archive.zip.notes"),
        "similarly named ordinary file was excluded: {names:?}"
    );
    assert!(names.contains(&"outside.txt"), "names: {names:?}");
    assert!(!names.contains(&"project/archive.zip"), "names: {names:?}");
    assert!(
        !names
            .iter()
            .any(|name| name.contains(".create-") && name.contains(".tmp.")),
        "names: {names:?}"
    );
}

#[test]
fn create_rejects_an_explicit_output_input_without_overwriting_it() {
    let tmp = TempDir::new("create-output-as-input");
    let archive = tmp.path().join("archive.zip");
    let original = b"this file must survive a rejected create";
    fs::write(&archive, original).unwrap();

    let error = engine()
        .create(
            &archive,
            std::slice::from_ref(&archive),
            &CreateOptions::default(),
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap_err();

    assert!(matches!(error, FormatError::Unsupported(_)));
    assert!(error.to_string().contains("cannot also be an input"));
    assert_eq!(fs::read(&archive).unwrap(), original);
}

#[cfg(unix)]
#[test]
fn create_rejects_a_symlink_output_without_touching_its_target() {
    let tmp = TempDir::new("create-output-symlink-leaf");
    let project = tmp.path().join("project");
    let outside = tmp.path().join("outside");
    fs::create_dir_all(&project).unwrap();
    fs::create_dir_all(&outside).unwrap();
    let input = outside.join("source.bin");
    let original = b"source behind the output symlink";
    fs::write(&input, original).unwrap();
    let archive = project.join("archive.zip");
    std::os::unix::fs::symlink(&input, &archive).unwrap();

    let error = engine()
        .create(
            &archive,
            std::slice::from_ref(&input),
            &CreateOptions::default(),
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap_err();

    assert!(matches!(error, FormatError::Unsupported(_)));
    assert_eq!(fs::read(&input).unwrap(), original);
    assert!(fs::symlink_metadata(&archive)
        .unwrap()
        .file_type()
        .is_symlink());
}

#[cfg(unix)]
#[test]
fn create_keeps_a_differently_named_hardlink_as_an_input() {
    let tmp = TempDir::new("create-output-hardlink-leaf");
    let input = tmp.path().join("source.bin");
    let archive = tmp.path().join("archive.zip");
    let original = b"source shared with the old output directory entry";
    fs::write(&input, original).unwrap();
    fs::hard_link(&input, &archive).unwrap();

    engine()
        .create(
            &archive,
            std::slice::from_ref(&input),
            &CreateOptions::default(),
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap();

    assert_eq!(fs::read(&input).unwrap(), original);
    let names: Vec<_> = engine()
        .list(&archive, &OpenOptions::default())
        .unwrap()
        .into_iter()
        .map(|entry| entry.path.display)
        .collect();
    assert!(names.iter().any(|name| name == "source.bin"));
}

#[cfg(unix)]
#[test]
fn create_excludes_output_temp_through_symlinked_parent() {
    let tmp = TempDir::new("create-output-symlink-parent");
    let project = tmp.path().join("project");
    fs::create_dir_all(&project).unwrap();
    fs::write(project.join("keep.txt"), b"keep me").unwrap();
    let alias = tmp.path().join("project-alias");
    std::os::unix::fs::symlink(&project, &alias).unwrap();
    let archive = alias.join("archive.zip");
    fs::write(project.join("archive.zip"), b"stale archive").unwrap();

    let ctl = ControlToken::new();
    let progress = CancelOnUnexpectedEntry {
        ctl: Arc::clone(&ctl),
        allowed: &["project", "project/keep.txt"],
        monitor_input_progress: AtomicBool::new(true),
        saw_unexpected: AtomicBool::new(false),
    };
    engine()
        .create(
            &archive,
            std::slice::from_ref(&project),
            &CreateOptions::default(),
            &progress,
            &ctl,
        )
        .unwrap();

    assert!(!progress.saw_unexpected.load(Ordering::Relaxed));
    let names: Vec<_> = engine()
        .list(&archive, &OpenOptions::default())
        .unwrap()
        .into_iter()
        .map(|entry| entry.path.display)
        .collect();
    assert!(names.iter().any(|name| name == "project/keep.txt"));
    assert!(!names.iter().any(|name| name == "project/archive.zip"));
}

#[test]
fn split_create_excludes_old_target_volumes_without_prefix_overreach() {
    let tmp = TempDir::new("create-split-output-exclusion");
    let project = tmp.path().join("project");
    fs::create_dir_all(&project).unwrap();
    let keep = project.join("keep.bin");
    let mut payload = Vec::with_capacity(32 * 1024);
    let mut state = 0x1234_5678u32;
    for _ in 0..32 * 1024 {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        payload.push((state >> 24) as u8);
    }
    fs::write(&keep, payload).unwrap();
    let old_first = project.join("bundle.zip.001");
    let old_second = project.join("bundle.zip.002");
    let stale_part = project.join("bundle.zip.999.part");
    let ordinary = project.join("bundle.zip.001.notes");
    let pid = std::process::id();
    let legacy_staging_name = format!(".bundle.zip.sqz-split-{pid}.tmp");
    let legacy_staging_entry = format!("project/{legacy_staging_name}");
    let legacy_staging_path = project.join(&legacy_staging_name);
    let outer_collision_name = format!(".bundle.zip.split-{pid}-0.tmp.bundle.zip");
    let outer_collision_entry = format!("project/{outer_collision_name}");
    let outer_collision_path = project.join(&outer_collision_name);
    let selected_outer_name = format!(".bundle.zip.split-{pid}-1.tmp.bundle.zip");
    let selected_outer_entry = format!("project/{selected_outer_name}");
    let inner_collision_name =
        format!(".{selected_outer_name}.create-{pid}-0.tmp.{selected_outer_name}");
    let inner_collision_entry = format!("project/{inner_collision_name}");
    let inner_collision_path = project.join(&inner_collision_name);
    let selected_inner_name =
        format!(".{selected_outer_name}.create-{pid}-1.tmp.{selected_outer_name}");
    let selected_inner_entry = format!("project/{selected_inner_name}");
    fs::write(&old_first, b"old first volume").unwrap();
    fs::write(&old_second, b"old second volume").unwrap();
    fs::write(&stale_part, b"stale internal split staging").unwrap();
    fs::write(&ordinary, b"ordinary file").unwrap();
    fs::write(&legacy_staging_path, b"legacy staging-name collision").unwrap();
    fs::write(
        &outer_collision_path,
        b"pre-existing complete staging archive",
    )
    .unwrap();
    fs::write(&inner_collision_path, b"pre-existing inner create temp").unwrap();

    let archive = project.join("bundle.zip");
    let opts = CreateOptions {
        split_size: Some(4 * 1024),
        ..CreateOptions::default()
    };
    let engine = engine();
    let plan = engine
        .plan_create(&archive, std::slice::from_ref(&project), &opts)
        .unwrap();
    let ctl = ControlToken::new();
    let allowed = [
        "project",
        "project/bundle.zip.001.notes",
        "project/keep.bin",
        legacy_staging_entry.as_str(),
        inner_collision_entry.as_str(),
    ];
    let progress = CancelOnUnexpectedEntry {
        ctl: Arc::clone(&ctl),
        allowed: &allowed,
        monitor_input_progress: AtomicBool::new(true),
        saw_unexpected: AtomicBool::new(false),
    };
    engine
        .create(
            &archive,
            std::slice::from_ref(&project),
            &opts,
            &progress,
            &ctl,
        )
        .unwrap();

    assert!(!progress.saw_unexpected.load(Ordering::Relaxed));
    let names: Vec<_> = engine
        .list(&project.join("bundle.zip.001"), &OpenOptions::default())
        .unwrap()
        .into_iter()
        .map(|entry| entry.path.display)
        .collect();
    assert!(names.iter().any(|name| name == "project/keep.bin"));
    assert!(names
        .iter()
        .any(|name| name == "project/bundle.zip.001.notes"));
    assert!(!names.iter().any(|name| name == "project/bundle.zip.001"));
    assert!(!names.iter().any(|name| name == "project/bundle.zip.002"));
    assert!(!names
        .iter()
        .any(|name| name == "project/bundle.zip.999.part"));
    assert!(names.iter().any(|name| name == &legacy_staging_entry));
    assert!(!names.iter().any(|name| name == &outer_collision_entry));
    assert!(names.iter().any(|name| name == &inner_collision_entry));
    assert!(!names.iter().any(|name| name == &selected_outer_entry));
    assert!(!names.iter().any(|name| name == &selected_inner_entry));
    assert_eq!(plan.inputs.entries, names.len());
    assert_eq!(
        fs::read(&legacy_staging_path).unwrap(),
        b"legacy staging-name collision"
    );
    assert_eq!(
        fs::read(&outer_collision_path).unwrap(),
        b"pre-existing complete staging archive"
    );
    assert_eq!(
        fs::read(&inner_collision_path).unwrap(),
        b"pre-existing inner create temp"
    );
    assert_eq!(
        fs::read(&stale_part).unwrap(),
        b"stale internal split staging"
    );
}

#[test]
fn split_create_excludes_case_variant_volumes_on_case_insensitive_filesystems() {
    let tmp = TempDir::new("create-split-case-exclusion");
    let project = tmp.path().join("project");
    fs::create_dir_all(&project).unwrap();
    let mut payload = Vec::with_capacity(24 * 1024);
    let mut state = 0x89ab_cdefu32;
    for _ in 0..24 * 1024 {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        payload.push((state >> 24) as u8);
    }
    fs::write(project.join("keep.bin"), payload).unwrap();
    let old_volume = project.join("BUNDLE.ZIP.999");
    let ordinary = project.join("BUNDLE.ZIP.999.notes");
    fs::write(&old_volume, b"old volume").unwrap();
    fs::write(&ordinary, b"ordinary file").unwrap();

    let archive = project.join("bundle.zip");
    if fs::metadata(project.join("bundle.zip.999")).is_err() {
        return;
    }

    engine()
        .create(
            &archive,
            std::slice::from_ref(&project),
            &CreateOptions {
                split_size: Some(4 * 1024),
                ..CreateOptions::default()
            },
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap();

    let names: Vec<_> = engine()
        .list(&project.join("bundle.zip.001"), &OpenOptions::default())
        .unwrap()
        .into_iter()
        .map(|entry| entry.path.display)
        .collect();
    assert!(!names.iter().any(|name| name == "project/BUNDLE.ZIP.999"));
    assert!(names
        .iter()
        .any(|name| name == "project/BUNDLE.ZIP.999.notes"));
    assert!(!old_volume.exists());
}

#[test]
fn roundtrip_create_list_extract_test() {
    let tmp = TempDir::new("roundtrip");
    build_fixture_tree(tmp.path());
    let archive = tmp.path().join("out.zip");
    let eng = engine();
    let ctl = ControlToken::new();

    eng.create(
        &archive,
        &[tmp.path().join("project")],
        &CreateOptions::default(),
        &NoProgress,
        &ctl,
    )
    .unwrap();

    // List: every fixture path is present with correct metadata.
    let entries = eng.list(&archive, &OpenOptions::default()).unwrap();
    let names: Vec<&str> = entries.iter().map(|e| e.path.display.as_str()).collect();
    let has = |n: &str| {
        names
            .iter()
            .any(|x| *x == n || x.trim_end_matches('/') == n)
    };
    assert!(has("project/a.txt"), "names: {names:?}");
    assert!(has("project/deep/a/b/c/d/file.bin"));
    assert!(has("project/中文目录/中文文件.txt"));
    assert!(has("project/empty_dir"));
    assert!(has("project/script.sh"));
    let a = entries
        .iter()
        .find(|e| e.path.display == "project/a.txt")
        .unwrap();
    assert_eq!(a.size, 11);
    assert!(matches!(a.entry_type, EntryType::File));
    assert!(!a.encrypted);
    #[cfg(unix)]
    {
        let link = entries
            .iter()
            .find(|e| e.path.display == "project/link")
            .unwrap();
        assert!(
            matches!(&link.entry_type, EntryType::Symlink { target } if target == b"a.txt"),
            "symlink meta: {:?}",
            link.entry_type
        );
    }

    // Extract and compare.
    let dest = tmp.path().join("extracted");
    eng.extract(
        &archive,
        &dest,
        None,
        &OpenOptions::default(),
        &ExtractOptions::default(),
        &NoProgress,
        &ctl,
    )
    .unwrap();
    assert_eq!(
        fs::read(dest.join("project/a.txt")).unwrap(),
        b"hello world"
    );
    assert_eq!(
        fs::read(dest.join("project/deep/a/b/c/d/file.bin")).unwrap(),
        vec![0xAB; 4096]
    );
    assert_eq!(
        fs::read_to_string(dest.join("project/中文目录/中文文件.txt")).unwrap(),
        "你好，世界"
    );
    assert!(dest.join("project/empty_dir").is_dir());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(dest.join("project/script.sh"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o755, "permissions restored");
        let target = fs::read_link(dest.join("project/link")).unwrap();
        assert_eq!(target, std::path::PathBuf::from("a.txt"));
    }

    // Integrity test passes for every entry.
    let report = eng
        .test(&archive, &OpenOptions::default(), &NoProgress, &ctl)
        .unwrap();
    assert!(report.is_ok(), "problems: {:?}", report.problems);
    assert_eq!(report.entries_tested, entries.len() as u64);
}

#[test]
fn interop_our_zip_passes_system_unzip() {
    if !command_exists("unzip") {
        eprintln!("skipped: system unzip not found");
        return;
    }
    let tmp = TempDir::new("interop-out");
    build_fixture_tree(tmp.path());
    let archive = tmp.path().join("ours.zip");
    let eng = engine();
    eng.create(
        &archive,
        &[tmp.path().join("project")],
        &CreateOptions::default(),
        &NoProgress,
        &ControlToken::new(),
    )
    .unwrap();
    let out = Command::new("unzip")
        .arg("-t")
        .arg(&archive)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "unzip -t failed: {}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn interop_system_zip_is_readable() {
    if !command_exists("zip") {
        eprintln!("skipped: system zip not found");
        return;
    }
    let tmp = TempDir::new("interop-in");
    let src = tmp.path().join("data");
    fs::create_dir_all(src.join("sub")).unwrap();
    fs::write(src.join("one.txt"), b"first file").unwrap();
    fs::write(src.join("sub/two.txt"), b"second file").unwrap();
    let archive = tmp.path().join("system.zip");
    let status = Command::new("zip")
        .arg("-r")
        .arg(&archive)
        .arg("data")
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(status.status.success());

    let eng = engine();
    let entries = eng.list(&archive, &OpenOptions::default()).unwrap();
    assert!(entries.iter().any(|e| e.path.display == "data/one.txt"));
    let dest = tmp.path().join("dest");
    eng.extract(
        &archive,
        &dest,
        None,
        &OpenOptions::default(),
        &ExtractOptions::default(),
        &NoProgress,
        &ControlToken::new(),
    )
    .unwrap();
    assert_eq!(fs::read(dest.join("data/one.txt")).unwrap(), b"first file");
    assert_eq!(
        fs::read(dest.join("data/sub/two.txt")).unwrap(),
        b"second file"
    );
}

#[test]
fn interop_infozip_native_split_opens_from_any_volume() {
    if !command_exists("zip") || !command_exists("7zz") {
        eprintln!("skipped: Info-ZIP zip or 7zz not found");
        return;
    }
    let tmp = TempDir::new("native-split");
    let mut payload = vec![0u8; 200 * 1024];
    let mut state = 0x4d59_5df4_d0f3_3173u64;
    for byte in &mut payload {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        *byte = state as u8;
    }
    fs::write(tmp.path().join("payload.bin"), &payload).unwrap();
    let output = Command::new("zip")
        .args(["-0", "-q", "-s", "64k", "native.zip", "payload.bin"])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "Info-ZIP split creation failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let first = tmp.path().join("native.z01");
    let second = tmp.path().join("native.z02");
    let final_path = tmp.path().join("native.zip");
    assert!(first.is_file());
    assert!(second.is_file());
    assert!(final_path.is_file());

    let eng = engine();
    let (format, entries, source_set) = eng
        .list_with_format_and_source_set(&second, &OpenOptions::default())
        .unwrap();
    assert_eq!(format, "zip");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].path.display, "payload.bin");
    let source_set = source_set.unwrap();
    assert_eq!(source_set.primary(), final_path);
    assert_eq!(source_set.members().first(), Some(&first));
    assert_eq!(source_set.members().last(), Some(&final_path));

    let report = eng
        .test(
            &first,
            &OpenOptions::default(),
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap();
    assert_eq!(report.entries_tested, 1);
    assert!(report.problems.is_empty());

    let dest = tmp.path().join("dest");
    eng.extract(
        &second,
        &dest,
        None,
        &OpenOptions::default(),
        &ExtractOptions::default(),
        &NoProgress,
        &ControlToken::new(),
    )
    .unwrap();
    assert_eq!(fs::read(dest.join("payload.bin")).unwrap(), payload);

    let hidden = tmp.path().join("native.z02.missing");
    fs::rename(&second, &hidden).unwrap();
    let error = eng.list(&first, &OpenOptions::default()).unwrap_err();
    assert_eq!(error.missing_volume_path(), Some(second.as_path()));
    fs::rename(hidden, second).unwrap();
}

#[test]
fn empty_zip_lists_zero_entries() {
    let tmp = TempDir::new("empty");
    let archive = tmp.path().join("empty.zip");
    let eng = engine();
    eng.create(
        &archive,
        &[],
        &CreateOptions::default(),
        &NoProgress,
        &ControlToken::new(),
    )
    .unwrap();
    let entries = eng.list(&archive, &OpenOptions::default()).unwrap();
    assert!(entries.is_empty());
}

#[test]
fn garbage_and_truncated_zip_report_corrupt() {
    let tmp = TempDir::new("corrupt");
    let eng = engine();

    // A zero-byte and a garbage file with .zip extension.
    let garbage = tmp.path().join("garbage.zip");
    fs::write(&garbage, b"this is definitely not a zip file").unwrap();
    let err = eng.list(&garbage, &OpenOptions::default()).unwrap_err();
    assert!(matches!(err, FormatError::CorruptArchive(_)), "{err:?}");

    // A real zip truncated inside the first file payload remains corrupt:
    // local-header recovery only accepts entries whose compressed payload is
    // fully present.
    let src = tmp.path().join("data.txt");
    fs::write(&src, vec![b'x'; 8192]).unwrap();
    let archive = tmp.path().join("ok.zip");
    eng.create(
        &archive,
        &[src],
        &CreateOptions::default(),
        &NoProgress,
        &ControlToken::new(),
    )
    .unwrap();
    let bytes = fs::read(&archive).unwrap();
    let name_len = u16::from_le_bytes([bytes[26], bytes[27]]) as usize;
    let extra_len = u16::from_le_bytes([bytes[28], bytes[29]]) as usize;
    let compressed_size = u32::from_le_bytes([bytes[18], bytes[19], bytes[20], bytes[21]]) as usize;
    let data_offset = 30 + name_len + extra_len;
    assert!(compressed_size > 1);
    let truncated = tmp.path().join("truncated.zip");
    fs::write(
        &truncated,
        &bytes[..data_offset + (compressed_size / 2).max(1)],
    )
    .unwrap();
    let err = eng.list(&truncated, &OpenOptions::default()).unwrap_err();
    assert!(matches!(err, FormatError::CorruptArchive(_)), "{err:?}");
}

#[test]
fn malformed_zip64_unsigned_descriptor_does_not_overflow() {
    let tmp = TempDir::new("zip64-descriptor-overflow");
    let archive = tmp.path().join("descriptor-overflow.zip");
    let bytes = [
        80, 75, 3, 4, 59, 145, 62, 75, 0, 0, 187, 96, 157, 81, 75, 3, 4, 0, 0, 252, 255, 255, 255,
        255, 255, 255, 1, 0, 0, 0, 0, 0, 0, 34, 220, 16, 16, 0, 0, 0, 47, 33, 220, 46, 0, 37, 220,
        220, 5, 255, 5, 220, 220, 3, 255, 5, 65, 69, 0, 0, 0, 0, 0, 0, 0, 252, 255, 255, 255, 255,
        255, 255, 1, 0, 0, 0, 0, 0, 0, 34, 220, 16, 16, 0, 0, 0, 47, 33, 220, 46, 0, 37, 220, 220,
        5, 255, 5, 220, 220, 3, 255, 5, 65, 69, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 15, 0, 0, 4, 0, 0,
        0, 0, 29, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 193, 69, 255, 9,
        0, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
        255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
        255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
        255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
        255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 3, 0,
    ];
    fs::write(&archive, bytes).unwrap();

    let err = engine()
        .list(&archive, &OpenOptions::default())
        .unwrap_err();
    assert!(
        matches!(
            err,
            FormatError::CorruptArchive(_) | FormatError::Io(_) | FormatError::Other(_)
        ),
        "{err:?}"
    );
}

#[test]
fn best_effort_extract_skips_crc_damaged_entry() {
    let tmp = TempDir::new("best-effort");
    let eng = engine();
    let good_name = b"good.txt";
    let good_data = b"safe bytes";
    let bad_name = b"bad.txt";
    let bad_data = b"broken bytes";
    let mut bytes = build_stored_zip(&[
        RawZipEntry {
            name: good_name.to_vec(),
            data: good_data.to_vec(),
        },
        RawZipEntry {
            name: bad_name.to_vec(),
            data: bad_data.to_vec(),
        },
    ]);
    let bad_data_offset = 30 + good_name.len() + good_data.len() + 30 + bad_name.len();
    bytes[bad_data_offset] ^= 0xFF;
    let archive = tmp.path().join("damaged.zip");
    fs::write(&archive, bytes).unwrap();

    let strict_dest = tmp.path().join("strict");
    fs::create_dir_all(&strict_dest).unwrap();
    fs::write(strict_dest.join("bad.txt"), b"existing bytes").unwrap();
    let strict_err = eng
        .extract(
            &archive,
            &strict_dest,
            None,
            &OpenOptions::default(),
            &ExtractOptions {
                overwrite: OverwritePolicy::Overwrite,
                ..ExtractOptions::default()
            },
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap_err();
    assert!(
        matches!(
            strict_err,
            FormatError::Io(_) | FormatError::CorruptArchive(_) | FormatError::Other(_)
        ),
        "{strict_err:?}"
    );
    assert_eq!(
        fs::read(strict_dest.join("bad.txt")).unwrap(),
        b"existing bytes"
    );
    assert!(
        fs::read_dir(&strict_dest).unwrap().all(|entry| {
            entry
                .ok()
                .and_then(|entry| entry.file_name().into_string().ok())
                .is_none_or(|name| !name.starts_with(".squallz-extract-"))
        }),
        "strict extraction left a staging file behind"
    );

    let collector = Arc::new(SkippedCollector::default());
    let best_dest = tmp.path().join("best");
    eng.extract(
        &archive,
        &best_dest,
        None,
        &OpenOptions::default(),
        &ExtractOptions {
            best_effort: true,
            problem_reporter: Some(collector.clone()),
            ..ExtractOptions::default()
        },
        &NoProgress,
        &ControlToken::new(),
    )
    .unwrap();

    assert_eq!(fs::read(best_dest.join("good.txt")).unwrap(), good_data);
    assert!(!best_dest.join("bad.txt").exists());
    let skipped = collector.items.lock().unwrap();
    assert_eq!(skipped.len(), 1, "{skipped:?}");
    assert!(skipped[0].contains("bad.txt"), "{skipped:?}");
}

#[test]
fn local_header_fallback_extracts_when_central_directory_is_missing() {
    let tmp = TempDir::new("local-header-fallback");
    let mut bytes = build_stored_zip(&[
        RawZipEntry {
            name: b"good.txt".to_vec(),
            data: b"safe bytes".to_vec(),
        },
        RawZipEntry {
            name: b"docs/readme.md".to_vec(),
            data: b"# recovered\n".to_vec(),
        },
    ]);
    let central_start = bytes
        .windows(4)
        .position(|w| w == [0x50, 0x4B, 0x01, 0x02])
        .expect("central directory signature");
    bytes.truncate(central_start);
    let archive = tmp.path().join("truncated-central.zip");
    fs::write(&archive, bytes).unwrap();

    let eng = engine();
    let entries = eng.list(&archive, &OpenOptions::default()).unwrap();
    let names: Vec<String> = entries
        .iter()
        .map(|entry| entry.path.display.clone())
        .collect();
    assert_eq!(names, vec!["good.txt", "docs/readme.md"]);

    let dest = tmp.path().join("out");
    eng.extract(
        &archive,
        &dest,
        None,
        &OpenOptions::default(),
        &ExtractOptions::default(),
        &NoProgress,
        &ControlToken::new(),
    )
    .unwrap();
    assert_eq!(fs::read(dest.join("good.txt")).unwrap(), b"safe bytes");
    assert_eq!(
        fs::read_to_string(dest.join("docs/readme.md")).unwrap(),
        "# recovered\n"
    );

    let report = eng
        .test(
            &archive,
            &OpenOptions::default(),
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap();
    assert!(report.is_ok(), "{:?}", report.problems);
    assert_eq!(report.entries_tested, 2);
}

#[test]
fn local_header_fallback_extracts_zip64_local_sizes() {
    let tmp = TempDir::new("local-header-zip64");
    let archive = tmp.path().join("zip64-local-only.zip");
    fs::write(
        &archive,
        build_stored_zip64_local_header_without_central_directory(&RawZipEntry {
            name: b"large-marker.bin".to_vec(),
            data: b"zip64 local header payload".to_vec(),
        }),
    )
    .unwrap();

    let eng = engine();
    let entries = eng.list(&archive, &OpenOptions::default()).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].path.display, "large-marker.bin");
    assert_eq!(entries[0].size, 26);
    assert_eq!(entries[0].compressed_size, Some(26));

    let report = eng
        .test(
            &archive,
            &OpenOptions::default(),
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap();
    assert!(report.is_ok(), "{:?}", report.problems);
    assert_eq!(report.entries_tested, 1);

    let dest = tmp.path().join("out");
    eng.extract(
        &archive,
        &dest,
        None,
        &OpenOptions::default(),
        &ExtractOptions::default(),
        &NoProgress,
        &ControlToken::new(),
    )
    .unwrap();
    assert_eq!(
        fs::read(dest.join("large-marker.bin")).unwrap(),
        b"zip64 local header payload"
    );
}

#[test]
fn local_header_fallback_lists_encrypted_entries_but_requires_password_to_read() {
    let tmp = TempDir::new("local-header-encrypted");
    let archive = tmp.path().join("encrypted-local-only.zip");
    fs::write(
        &archive,
        build_encrypted_flag_stored_zip_without_central_directory(&RawZipEntry {
            name: b"secret.txt".to_vec(),
            data: b"plaintext fixture is not exposed".to_vec(),
        }),
    )
    .unwrap();

    let eng = engine();
    let entries = eng.list(&archive, &OpenOptions::default()).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].path.display, "secret.txt");
    assert!(entries[0].encrypted);
    assert_eq!(entries[0].size, 32);

    let err = eng
        .test(
            &archive,
            &OpenOptions::default(),
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap_err();
    assert!(matches!(err, FormatError::PasswordRequired), "{err:?}");

    let err = eng
        .extract(
            &archive,
            &tmp.path().join("out"),
            None,
            &OpenOptions::default(),
            &ExtractOptions::default(),
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap_err();
    assert!(matches!(err, FormatError::PasswordRequired), "{err:?}");
    assert!(!tmp.path().join("out/secret.txt").exists());
}

#[test]
fn local_header_fallback_lists_unsupported_methods_but_refuses_to_read() {
    let tmp = TempDir::new("local-header-unsupported-method");
    let archive = tmp.path().join("unsupported-method-local-only.zip");
    fs::write(
        &archive,
        build_unsupported_method_zip_without_central_directory(
            &RawZipEntry {
                name: b"compressed.bin".to_vec(),
                data: b"opaque compressed payload".to_vec(),
            },
            14,
        ),
    )
    .unwrap();

    let eng = engine();
    let entries = eng.list(&archive, &OpenOptions::default()).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].path.display, "compressed.bin");
    assert_eq!(entries[0].size, 25);
    assert_eq!(entries[0].compressed_size, Some(25));

    let report = eng
        .test(
            &archive,
            &OpenOptions::default(),
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap();
    assert!(!report.is_ok(), "{report:?}");
    assert_eq!(report.entries_tested, 1);
    assert!(
        report
            .problems
            .iter()
            .any(|problem| problem.contains("compression method 14")),
        "{:?}",
        report.problems
    );

    let err = eng
        .extract(
            &archive,
            &tmp.path().join("strict"),
            None,
            &OpenOptions::default(),
            &ExtractOptions::default(),
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap_err();
    match err {
        FormatError::Unsupported(detail) => {
            assert!(detail.contains("compression method 14"), "{detail}");
        }
        other => panic!("expected Unsupported, got {other:?}"),
    }
}

#[test]
fn local_header_fallback_extracts_signed_zip64_data_descriptor_entries() {
    let tmp = TempDir::new("local-header-zip64-descriptor");
    let mut bytes = build_stored_zip64_with_data_descriptor(
        &[
            RawZipEntry {
                name: b"streamed64.txt".to_vec(),
                data: b"zip64 descriptor payload".to_vec(),
            },
            RawZipEntry {
                name: b"docs/zip64.txt".to_vec(),
                data: b"second zip64 descriptor".to_vec(),
            },
        ],
        true,
    );
    let central_start = bytes
        .windows(4)
        .position(|w| w == [0x50, 0x4B, 0x01, 0x02])
        .expect("central directory signature");
    bytes.truncate(central_start);
    let archive = tmp.path().join("truncated-zip64-descriptor.zip");
    fs::write(&archive, bytes).unwrap();

    let eng = engine();
    let entries = eng.list(&archive, &OpenOptions::default()).unwrap();
    let names: Vec<String> = entries
        .iter()
        .map(|entry| entry.path.display.clone())
        .collect();
    assert_eq!(names, vec!["streamed64.txt", "docs/zip64.txt"]);
    assert_eq!(entries[0].size, 24);
    assert_eq!(entries[0].compressed_size, Some(24));

    let report = eng
        .test(
            &archive,
            &OpenOptions::default(),
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap();
    assert!(report.is_ok(), "{:?}", report.problems);
    assert_eq!(report.entries_tested, 2);

    let dest = tmp.path().join("out");
    eng.extract(
        &archive,
        &dest,
        None,
        &OpenOptions::default(),
        &ExtractOptions::default(),
        &NoProgress,
        &ControlToken::new(),
    )
    .unwrap();
    assert_eq!(
        fs::read_to_string(dest.join("streamed64.txt")).unwrap(),
        "zip64 descriptor payload"
    );
    assert_eq!(
        fs::read_to_string(dest.join("docs/zip64.txt")).unwrap(),
        "second zip64 descriptor"
    );
}

#[test]
fn local_header_fallback_extracts_unsigned_zip64_data_descriptor_entries() {
    let tmp = TempDir::new("local-header-zip64-unsigned-descriptor");
    let mut bytes = build_stored_zip64_with_data_descriptor(
        &[
            RawZipEntry {
                name: b"first64.txt".to_vec(),
                data: b"first zip64 unsigned descriptor".to_vec(),
            },
            RawZipEntry {
                name: b"last64.txt".to_vec(),
                data: b"last zip64 unsigned descriptor".to_vec(),
            },
        ],
        false,
    );
    let central_start = bytes
        .windows(4)
        .position(|w| w == [0x50, 0x4B, 0x01, 0x02])
        .expect("central directory signature");
    bytes.truncate(central_start);
    let archive = tmp.path().join("truncated-zip64-unsigned-descriptor.zip");
    fs::write(&archive, bytes).unwrap();

    let eng = engine();
    let entries = eng.list(&archive, &OpenOptions::default()).unwrap();
    let names: Vec<String> = entries
        .iter()
        .map(|entry| entry.path.display.clone())
        .collect();
    assert_eq!(names, vec!["first64.txt", "last64.txt"]);
    assert_eq!(entries[0].size, 31);
    assert_eq!(entries[0].compressed_size, Some(31));

    let report = eng
        .test(
            &archive,
            &OpenOptions::default(),
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap();
    assert!(report.is_ok(), "{:?}", report.problems);
    assert_eq!(report.entries_tested, 2);

    let dest = tmp.path().join("out");
    eng.extract(
        &archive,
        &dest,
        None,
        &OpenOptions::default(),
        &ExtractOptions::default(),
        &NoProgress,
        &ControlToken::new(),
    )
    .unwrap();
    assert_eq!(
        fs::read_to_string(dest.join("first64.txt")).unwrap(),
        "first zip64 unsigned descriptor"
    );
    assert_eq!(
        fs::read_to_string(dest.join("last64.txt")).unwrap(),
        "last zip64 unsigned descriptor"
    );
}

#[test]
fn local_header_fallback_extracts_signed_data_descriptor_entries() {
    let tmp = TempDir::new("local-header-descriptor");
    let mut bytes = build_stored_zip_with_data_descriptor(
        &[
            RawZipEntry {
                name: b"streamed.txt".to_vec(),
                data: b"descriptor payload".to_vec(),
            },
            RawZipEntry {
                name: b"docs/notes.txt".to_vec(),
                data: b"notes via descriptor".to_vec(),
            },
        ],
        true,
    );
    let central_start = bytes
        .windows(4)
        .position(|w| w == [0x50, 0x4B, 0x01, 0x02])
        .expect("central directory signature");
    bytes.truncate(central_start);
    let archive = tmp.path().join("truncated-descriptor.zip");
    fs::write(&archive, bytes).unwrap();

    let eng = engine();
    let entries = eng.list(&archive, &OpenOptions::default()).unwrap();
    let names: Vec<String> = entries
        .iter()
        .map(|entry| entry.path.display.clone())
        .collect();
    assert_eq!(names, vec!["streamed.txt", "docs/notes.txt"]);

    let dest = tmp.path().join("out");
    eng.extract(
        &archive,
        &dest,
        None,
        &OpenOptions::default(),
        &ExtractOptions::default(),
        &NoProgress,
        &ControlToken::new(),
    )
    .unwrap();
    assert_eq!(
        fs::read_to_string(dest.join("streamed.txt")).unwrap(),
        "descriptor payload"
    );
    assert_eq!(
        fs::read_to_string(dest.join("docs/notes.txt")).unwrap(),
        "notes via descriptor"
    );

    let report = eng
        .test(
            &archive,
            &OpenOptions::default(),
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap();
    assert!(report.is_ok(), "{:?}", report.problems);
    assert_eq!(report.entries_tested, 2);
}

#[test]
fn local_header_fallback_extracts_unsigned_data_descriptor_entries() {
    let tmp = TempDir::new("local-header-unsigned-descriptor");
    let mut bytes = build_stored_zip_with_data_descriptor(
        &[
            RawZipEntry {
                name: b"first.txt".to_vec(),
                data: b"first unsigned descriptor".to_vec(),
            },
            RawZipEntry {
                name: b"last.txt".to_vec(),
                data: b"last unsigned descriptor".to_vec(),
            },
        ],
        false,
    );
    let central_start = bytes
        .windows(4)
        .position(|w| w == [0x50, 0x4B, 0x01, 0x02])
        .expect("central directory signature");
    bytes.truncate(central_start);
    let archive = tmp.path().join("truncated-unsigned-descriptor.zip");
    fs::write(&archive, bytes).unwrap();

    let eng = engine();
    let entries = eng.list(&archive, &OpenOptions::default()).unwrap();
    let names: Vec<String> = entries
        .iter()
        .map(|entry| entry.path.display.clone())
        .collect();
    assert_eq!(names, vec!["first.txt", "last.txt"]);

    let dest = tmp.path().join("out");
    eng.extract(
        &archive,
        &dest,
        None,
        &OpenOptions::default(),
        &ExtractOptions::default(),
        &NoProgress,
        &ControlToken::new(),
    )
    .unwrap();
    assert_eq!(
        fs::read_to_string(dest.join("first.txt")).unwrap(),
        "first unsigned descriptor"
    );
    assert_eq!(
        fs::read_to_string(dest.join("last.txt")).unwrap(),
        "last unsigned descriptor"
    );

    let report = eng
        .test(
            &archive,
            &OpenOptions::default(),
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap();
    assert!(report.is_ok(), "{:?}", report.problems);
    assert_eq!(report.entries_tested, 2);
}

#[test]
fn local_header_fallback_reports_crc_mismatch() {
    let tmp = TempDir::new("local-header-crc");
    let name = b"bad.txt";
    let mut bytes = build_stored_zip(&[RawZipEntry {
        name: name.to_vec(),
        data: b"original bytes".to_vec(),
    }]);
    let central_start = bytes
        .windows(4)
        .position(|w| w == [0x50, 0x4B, 0x01, 0x02])
        .expect("central directory signature");
    let data_offset = 30 + name.len();
    bytes[data_offset] ^= 0xFF;
    bytes.truncate(central_start);
    let archive = tmp.path().join("truncated-central-crc.zip");
    fs::write(&archive, bytes).unwrap();

    let eng = engine();
    let entries = eng.list(&archive, &OpenOptions::default()).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].path.display, "bad.txt");

    let report = eng
        .test(
            &archive,
            &OpenOptions::default(),
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap();
    assert!(!report.is_ok(), "{report:?}");
    assert_eq!(report.entries_tested, 1);
    assert!(
        report
            .problems
            .iter()
            .any(|problem| problem.contains("CRC mismatch")),
        "{:?}",
        report.problems
    );

    let strict_err = eng
        .extract(
            &archive,
            &tmp.path().join("strict"),
            None,
            &OpenOptions::default(),
            &ExtractOptions::default(),
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap_err();
    assert!(matches!(strict_err, FormatError::Io(_)), "{strict_err:?}");

    let collector = Arc::new(SkippedCollector::default());
    let best_dest = tmp.path().join("best");
    eng.extract(
        &archive,
        &best_dest,
        None,
        &OpenOptions::default(),
        &ExtractOptions {
            best_effort: true,
            problem_reporter: Some(collector.clone()),
            ..ExtractOptions::default()
        },
        &NoProgress,
        &ControlToken::new(),
    )
    .unwrap();
    assert!(!best_dest.join("bad.txt").exists());
    let skipped = collector.items.lock().unwrap();
    assert_eq!(skipped.len(), 1, "{skipped:?}");
    assert!(skipped[0].contains("CRC mismatch"), "{skipped:?}");
}

/// ZIP64 large-file path: stream 5 GiB of zeros in Store mode and read it
/// back. Run explicitly: `scripts/zip64_large_smoke.sh`.
#[test]
#[ignore]
fn zip64_store_5gib_roundtrip() {
    const SIZE: u64 = 5 * 1024 * 1024 * 1024;
    let tmp = TempDir::new("zip64");
    let archive = tmp.path().join("big.zip");
    let registry = squallz_formats::registry();
    let Some(Detected::Archive(format)) = registry.detect_by_name("big.zip") else {
        panic!("zip format not registered");
    };

    // Stream-write 5 GiB of zeros without materializing them in memory.
    let dst = fs::File::create(&archive).unwrap();
    let mut writer = format
        .create(
            Box::new(dst),
            &CreateOptions {
                level: CompressionLevel::Store,
                ..CreateOptions::default()
            },
        )
        .unwrap();
    let meta = EntryMeta {
        path: EntryPath::from_utf8("zeros.bin"),
        entry_type: EntryType::File,
        size: SIZE,
        compressed_size: None,
        modified: None,
        unix_mode: Some(0o644),
        crc32: None,
        encrypted: false,
    };
    let mut zeros = std::io::repeat(0).take(SIZE);
    writer.add_entry(&meta, Some(&mut zeros)).unwrap();
    writer.finish().unwrap();
    assert!(fs::metadata(&archive).unwrap().len() > SIZE);

    // Read back: metadata sees the true size, streaming returns every byte.
    let src = fs::File::open(&archive).unwrap();
    let mut reader = format.open(Box::new(src), &OpenOptions::default()).unwrap();
    let entries: Vec<EntryMeta> = reader.entries().collect::<Result<_, _>>().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].size, SIZE);
    let mut stream = reader.read_entry(&entries[0].path).unwrap();
    let mut remaining = 0u64;
    let mut buf = vec![0u8; 1024 * 1024];
    loop {
        let n = stream.read(&mut buf).unwrap();
        if n == 0 {
            break;
        }
        assert!(buf[..n].iter().all(|&b| b == 0));
        remaining += n as u64;
    }
    assert_eq!(remaining, SIZE);
}
