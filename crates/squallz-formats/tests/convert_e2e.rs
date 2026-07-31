//! End-to-end format conversion tests: zip→7z, 7z→zip, zip→tar.gz,
//! tar.gz→zip, password handling and unsupported-entry reporting.

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use common::{engine, TempDir};
use squallz_core::api::{
    ControlToken, CreateOptions, ExtractOptions, FormatError, NoProgress, OpenOptions, Password,
};
use squallz_core::{inspect_create_destination, CreateArtifactKind, CreateCommitPolicy};

/// Builds a small tree and packs it into `name` under `dir`, returning the
/// archive path.
fn make_archive(dir: &Path, name: &str) -> PathBuf {
    let root = dir.join("project");
    fs::create_dir_all(root.join("sub")).unwrap();
    fs::write(root.join("a.txt"), b"hello world").unwrap();
    fs::write(root.join("sub/b.txt"), vec![0xAB; 4096]).unwrap();
    let dest = dir.join(name);
    engine()
        .create(
            &dest,
            &[root],
            &CreateOptions::default(),
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap();
    dest
}

/// Converts `src` to `dest_name`, extracts the result and asserts the
/// content survived.
fn convert_and_check(dir: &Path, src: &Path, dest_name: &str) {
    let dest = dir.join(dest_name);
    let ctl = ControlToken::new();
    engine()
        .convert(
            src,
            &dest,
            &OpenOptions::default(),
            &CreateOptions::default(),
            &NoProgress,
            &ctl,
        )
        .unwrap();
    let out = dir.join(format!("x-{dest_name}"));
    engine()
        .extract(
            &dest,
            &out,
            None,
            &OpenOptions::default(),
            &ExtractOptions::default(),
            &NoProgress,
            &ctl,
        )
        .unwrap();
    assert_eq!(
        fs::read(out.join("project/a.txt")).unwrap(),
        b"hello world",
        "{dest_name}: a.txt differs"
    );
    assert_eq!(
        fs::read(out.join("project/sub/b.txt")).unwrap(),
        vec![0xAB; 4096],
        "{dest_name}: b.txt differs"
    );
}

fn make_hardlink_tar(path: &Path) {
    let file = fs::File::create(path).unwrap();
    let mut builder = tar::Builder::new(file);
    let data = b"original";
    let mut header = tar::Header::new_gnu();
    header.set_mode(0o644);
    header.set_size(data.len() as u64);
    header.set_entry_type(tar::EntryType::Regular);
    builder
        .append_data(&mut header, "original.txt", data.as_slice())
        .unwrap();

    let mut link = tar::Header::new_gnu();
    link.set_mode(0o644);
    link.set_size(0);
    link.set_entry_type(tar::EntryType::Link);
    builder
        .append_link(&mut link, "copy.txt", "original.txt")
        .unwrap();
    builder.finish().unwrap();
}

#[test]
fn zip_to_7z_and_back() {
    let tmp = TempDir::new("convert-zip-7z");
    let zip = make_archive(tmp.path(), "src.zip");
    convert_and_check(tmp.path(), &zip, "mid.7z");
    convert_and_check(tmp.path(), &tmp.path().join("mid.7z"), "back.zip");
}

#[test]
fn zip_to_tar_gz_and_back() {
    let tmp = TempDir::new("convert-zip-targz");
    let zip = make_archive(tmp.path(), "src.zip");
    convert_and_check(tmp.path(), &zip, "mid.tar.gz");
    convert_and_check(tmp.path(), &tmp.path().join("mid.tar.gz"), "back.zip");
}

#[test]
fn unsplit_conversion_replaces_without_hidden_backup_artifacts() {
    let tmp = TempDir::new("convert-atomic-replace");
    let source = make_archive(tmp.path(), "source.zip");
    let destination = tmp.path().join("converted.7z");
    fs::write(&destination, b"previous output").unwrap();

    let report = engine()
        .convert_with_report(
            &source,
            &destination,
            &OpenOptions::default(),
            &CreateOptions::default(),
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap();

    assert_eq!(report.primary_output, destination);
    assert_eq!(report.outputs, vec![destination.clone()]);
    assert!(report.preserved_outputs.is_empty());
    assert_eq!(report.split_volume_count, None);
    assert_eq!(
        report.total_output_bytes,
        fs::metadata(&destination).unwrap().len()
    );
    assert_eq!(
        engine()
            .list(&destination, &OpenOptions::default())
            .unwrap()
            .iter()
            .filter(|entry| matches!(entry.entry_type, squallz_core::api::EntryType::File))
            .count(),
        2
    );
    assert!(!fs::read_dir(tmp.path()).unwrap().any(|entry| {
        let name = entry.unwrap().file_name();
        let name = name.to_string_lossy();
        name.contains("replace-backup") || name.contains(".convert-")
    }));
}

#[test]
fn explicit_conversion_policy_refuses_unapproved_or_changed_outputs() {
    let tmp = TempDir::new("convert-explicit-destination-policy");
    let source = make_archive(tmp.path(), "source.zip");
    let destination = tmp.path().join("converted.7z");
    fs::write(&destination, b"unapproved output").unwrap();
    let ctl = ControlToken::new();

    let error = engine()
        .convert_with_policy(
            &source,
            &destination,
            &OpenOptions::default(),
            &CreateOptions::default(),
            CreateCommitPolicy::NoReplace,
            &NoProgress,
            &ctl,
        )
        .unwrap_err();
    assert!(error.is_output_exists());
    assert_eq!(fs::read(&destination).unwrap(), b"unapproved output");

    let guard = inspect_create_destination(&destination, CreateArtifactKind::Archive)
        .unwrap()
        .guard
        .unwrap();
    fs::write(&destination, b"newer output from another app").unwrap();
    let error = engine()
        .convert_with_policy(
            &source,
            &destination,
            &OpenOptions::default(),
            &CreateOptions::default(),
            CreateCommitPolicy::ReplaceIfUnchanged(guard),
            &NoProgress,
            &ctl,
        )
        .unwrap_err();
    assert!(error.is_destination_changed());
    assert_eq!(
        fs::read(&destination).unwrap(),
        b"newer output from another app"
    );
    assert!(!fs::read_dir(tmp.path()).unwrap().any(|entry| {
        let name = entry.unwrap().file_name();
        let name = name.to_string_lossy();
        name.contains(".convert-")
            || name.contains("replace-backup")
            || name.starts_with(".squallz-update-")
    }));

    let current_guard = inspect_create_destination(&destination, CreateArtifactKind::Archive)
        .unwrap()
        .guard
        .unwrap();
    engine()
        .convert_with_policy(
            &source,
            &destination,
            &OpenOptions::default(),
            &CreateOptions::default(),
            CreateCommitPolicy::ReplaceIfUnchanged(current_guard),
            &NoProgress,
            &ctl,
        )
        .unwrap();
    assert_eq!(
        engine()
            .list(&destination, &OpenOptions::default())
            .unwrap()
            .iter()
            .filter(|entry| matches!(entry.entry_type, squallz_core::api::EntryType::File))
            .count(),
        2
    );
}

#[test]
fn split_conversion_requires_and_returns_an_artifact_report() {
    let tmp = TempDir::new("convert-split-report");
    let source = make_archive(tmp.path(), "source.zip");
    let destination = tmp.path().join("converted.7z");
    let options = CreateOptions {
        split_size: Some(1024),
        ..CreateOptions::default()
    };
    let ctl = ControlToken::new();

    let error = engine()
        .convert(
            &source,
            &destination,
            &OpenOptions::default(),
            &options,
            &NoProgress,
            &ctl,
        )
        .unwrap_err();
    assert!(matches!(
        error,
        FormatError::Unsupported(ref detail) if detail.contains("convert_with_report")
    ));

    let error = engine()
        .convert_with_atomic_replace(
            &source,
            &destination,
            &OpenOptions::default(),
            &options,
            &NoProgress,
            &ctl,
        )
        .unwrap_err();
    assert!(matches!(
        error,
        FormatError::Unsupported(ref detail)
            if detail.contains("convert_with_atomic_replace") && detail.contains("split")
    ));
    assert!(!destination.exists());

    let first = engine()
        .convert_with_report(
            &source,
            &destination,
            &OpenOptions::default(),
            &options,
            &NoProgress,
            &ctl,
        )
        .unwrap();
    assert_eq!(first.split_volume_count, Some(first.outputs.len()));
    assert!(first.preserved_outputs.is_empty());

    let error = engine()
        .convert_with_report_policy(
            &source,
            &destination,
            &OpenOptions::default(),
            &options,
            CreateCommitPolicy::NoReplace,
            &NoProgress,
            &ctl,
        )
        .unwrap_err();
    assert!(error.is_output_exists());

    let second = engine()
        .convert_with_report(
            &source,
            &destination,
            &OpenOptions::default(),
            &options,
            &NoProgress,
            &ctl,
        )
        .unwrap();
    assert_eq!(second.preserved_outputs.len(), first.outputs.len());
    for path in second.outputs.iter().chain(second.preserved_outputs.iter()) {
        assert!(
            path.is_file(),
            "reported path is missing: {}",
            path.display()
        );
    }
}

#[test]
fn conversion_plan_reuses_the_real_split_output_layout() {
    let tmp = TempDir::new("convert-plan-split");
    let source = make_archive(tmp.path(), "source.zip");
    let destination = tmp.path().join("converted.7z");
    let options = CreateOptions {
        split_size: Some(1024),
        ..CreateOptions::default()
    };
    let engine = engine();

    let plan = engine
        .plan_convert(&source, &destination, &OpenOptions::default(), &options)
        .unwrap();
    assert_eq!(plan.inputs.input_count, 1);
    assert_eq!(plan.inputs.files, 2);
    assert_eq!(plan.inputs.directories, 2);
    assert_eq!(plan.inputs.total_bytes, 4107);
    assert_eq!(plan.primary_output, tmp.path().join("converted.7z.001"));
    assert!(plan
        .split_volume_count_budget
        .is_some_and(|count| count > 1));
    assert!(plan.workspace_budget_bytes >= plan.final_output_budget_bytes);

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
    assert!(report.total_output_bytes <= plan.final_output_budget_bytes);
    assert!(plan
        .split_volume_count_budget
        .is_some_and(|budget| budget as usize >= report.split_volume_count.unwrap()));
}

#[test]
fn conversion_plan_rejects_invalid_single_stream_layout() {
    let tmp = TempDir::new("convert-plan-stream-layout");
    let source = make_archive(tmp.path(), "source.zip");
    let destination = tmp.path().join("converted.gz");

    let error = engine()
        .plan_convert(
            &source,
            &destination,
            &OpenOptions::default(),
            &CreateOptions::default(),
        )
        .unwrap_err();

    assert!(matches!(
        error,
        FormatError::Unsupported(ref detail)
            if detail.contains("gzip") && detail.contains("exactly one file")
    ));
    assert!(!destination.exists());
}

#[test]
fn conversion_plan_rejects_an_invalid_target_before_opening_the_source() {
    let tmp = TempDir::new("convert-plan-target-first");
    let source = tmp.path().join("missing-source.zip");
    let destination = tmp.path().join("converted.swm");

    let error = engine()
        .plan_convert(
            &source,
            &destination,
            &OpenOptions::default(),
            &CreateOptions::default(),
        )
        .unwrap_err();

    assert!(error.is_split_wim_creation_unsupported());
    assert_eq!(fs::read_dir(tmp.path()).unwrap().count(), 0);
}

#[test]
fn swm_destination_without_native_options_is_rejected_before_source_open_or_staging() {
    let tmp = TempDir::new("convert-split-wim-preflight");
    let source = tmp.path().join("missing-source.zip");
    let destination = tmp.path().join("image.swm");

    let error = engine()
        .convert_with_report(
            &source,
            &destination,
            &OpenOptions::default(),
            &CreateOptions::default(),
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap_err();

    assert!(error.is_split_wim_creation_unsupported());
    assert!(!destination.exists());
    assert_eq!(fs::read_dir(tmp.path()).unwrap().count(), 0);
}

#[test]
fn encrypted_source_to_encrypted_destination() {
    let tmp = TempDir::new("convert-encrypted");
    let root = tmp.path().join("data");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("secret.txt"), b"classified").unwrap();
    let src = tmp.path().join("src.zip");
    let ctl = ControlToken::new();
    let src_opts = CreateOptions {
        password: Some(Password::new("in-pass")),
        ..CreateOptions::default()
    };
    engine()
        .create(&src, &[root], &src_opts, &NoProgress, &ctl)
        .unwrap();

    // Wrong/missing source password fails.
    let err = engine()
        .convert(
            &src,
            &tmp.path().join("fail.7z"),
            &OpenOptions::default(),
            &CreateOptions::default(),
            &NoProgress,
            &ctl,
        )
        .unwrap_err();
    assert!(matches!(
        err,
        FormatError::PasswordRequired | FormatError::WrongPassword
    ));

    // Correct source password, new destination password.
    let dest = tmp.path().join("out.7z");
    let open = OpenOptions {
        password: Some(Password::new("in-pass")),
        encoding_override: None,
    };
    let create = CreateOptions {
        password: Some(Password::new("out-pass")),
        ..CreateOptions::default()
    };
    engine()
        .convert(&src, &dest, &open, &create, &NoProgress, &ctl)
        .unwrap();
    let out = tmp.path().join("extracted");
    let dest_open = OpenOptions {
        password: Some(Password::new("out-pass")),
        encoding_override: None,
    };
    engine()
        .extract(
            &dest,
            &out,
            None,
            &dest_open,
            &ExtractOptions::default(),
            &NoProgress,
            &ctl,
        )
        .unwrap();
    assert_eq!(
        fs::read(out.join("data/secret.txt")).unwrap(),
        b"classified"
    );
}

#[cfg(unix)]
#[test]
fn symlink_to_7z_reports_unsupported_with_entry() {
    let tmp = TempDir::new("convert-symlink");
    let root = tmp.path().join("tree");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("real.txt"), b"data").unwrap();
    std::os::unix::fs::symlink("real.txt", root.join("link.txt")).unwrap();
    let src = tmp.path().join("src.zip");
    let ctl = ControlToken::new();
    engine()
        .create(&src, &[root], &CreateOptions::default(), &NoProgress, &ctl)
        .unwrap();
    let dest = tmp.path().join("out.7z");
    fs::write(&dest, b"previous output").unwrap();
    let err = engine()
        .convert(
            &src,
            &dest,
            &OpenOptions::default(),
            &CreateOptions::default(),
            &NoProgress,
            &ctl,
        )
        .unwrap_err();
    match err {
        FormatError::Unsupported(detail) => {
            assert!(
                detail.contains("symbolic link"),
                "detail must name the entry type: {detail}"
            );
            assert!(
                detail.contains("tree/link.txt"),
                "detail must name the entry: {detail}"
            );
            assert!(
                detail.contains("real.txt"),
                "detail must name the link target: {detail}"
            );
            assert!(
                detail.contains("tar or zip"),
                "detail must suggest a preserving format: {detail}"
            );
        }
        other => panic!("expected Unsupported, got {other:?}"),
    }
    assert_eq!(fs::read(dest).unwrap(), b"previous output");
}

#[test]
fn hardlink_to_7z_reports_unsupported_with_entry_and_target() {
    let tmp = TempDir::new("convert-hardlink");
    let src = tmp.path().join("links.tar");
    make_hardlink_tar(&src);
    let ctl = ControlToken::new();
    let err = engine()
        .convert(
            &src,
            &tmp.path().join("out.7z"),
            &OpenOptions::default(),
            &CreateOptions::default(),
            &NoProgress,
            &ctl,
        )
        .unwrap_err();
    match err {
        FormatError::Unsupported(detail) => {
            assert!(
                detail.contains("hard link"),
                "detail must name the entry type: {detail}"
            );
            assert!(
                detail.contains("copy.txt"),
                "detail must name the entry: {detail}"
            );
            assert!(
                detail.contains("original.txt"),
                "detail must name the hardlink target: {detail}"
            );
            assert!(
                detail.contains("tar"),
                "detail must suggest a preserving format: {detail}"
            );
        }
        other => panic!("expected Unsupported, got {other:?}"),
    }
}

#[test]
fn single_file_zip_converts_to_plain_gz() {
    let tmp = TempDir::new("convert-gz");
    let root = tmp.path().join("one");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("only.txt"), b"single file payload").unwrap();
    let src = tmp.path().join("src.zip");
    let ctl = ControlToken::new();
    engine()
        .create(&src, &[root], &CreateOptions::default(), &NoProgress, &ctl)
        .unwrap();
    let dest = tmp.path().join("only.txt.gz");
    engine()
        .convert(
            &src,
            &dest,
            &OpenOptions::default(),
            &CreateOptions::default(),
            &NoProgress,
            &ctl,
        )
        .unwrap();
    // The virtual single-entry view of the .gz must decompress to the
    // original content.
    let out = tmp.path().join("x-gz");
    engine()
        .extract(
            &dest,
            &out,
            None,
            &OpenOptions::default(),
            &ExtractOptions::default(),
            &NoProgress,
            &ctl,
        )
        .unwrap();
    assert_eq!(
        fs::read(out.join("only.txt")).unwrap(),
        b"single file payload"
    );
}
