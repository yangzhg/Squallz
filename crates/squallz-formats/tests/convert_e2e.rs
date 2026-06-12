//! End-to-end format conversion tests: zip→7z, 7z→zip, zip→tar.gz,
//! tar.gz→zip, password handling and unsupported-entry reporting.

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use common::{engine, TempDir};
use squallz_core::api::{
    ControlToken, CreateOptions, ExtractOptions, FormatError, NoProgress, OpenOptions, Password,
};

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
