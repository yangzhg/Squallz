//! SymlinkPolicy::Follow and hardlink extraction tests (the I4 closure of
//! the I1 leftovers): followed links materialize the target's content,
//! cycles/escaping targets are skipped, hardlinks restore as hard links.

#![cfg(unix)]

mod common;

use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use common::{engine, TempDir};
use squallz_core::api::{
    ControlToken, CreateOptions, ExtractOptions, NoProgress, OpenOptions, SymlinkPolicy,
};

fn extract_with(archive: &Path, dest: &Path, symlinks: SymlinkPolicy) {
    let opts = ExtractOptions {
        symlinks,
        ..ExtractOptions::default()
    };
    engine()
        .extract(
            archive,
            dest,
            None,
            &OpenOptions::default(),
            &opts,
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap();
}

/// tree/: data.txt, link.txt -> data.txt, chain.txt -> link.txt,
/// dangling -> missing, escape -> ../outside.
fn linked_tree(dir: &Path) -> PathBuf {
    let root = dir.join("tree");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("data.txt"), b"link target content").unwrap();
    std::os::unix::fs::symlink("data.txt", root.join("link.txt")).unwrap();
    std::os::unix::fs::symlink("link.txt", root.join("chain.txt")).unwrap();
    std::os::unix::fs::symlink("missing.txt", root.join("dangling")).unwrap();
    std::os::unix::fs::symlink("../../outside.txt", root.join("escape")).unwrap();
    root
}

#[test]
fn zip_follow_materializes_content_copies() {
    let tmp = TempDir::new("follow-zip");
    let root = linked_tree(tmp.path());
    let archive = tmp.path().join("links.zip");
    let ctl = ControlToken::new();
    engine()
        .create(
            &archive,
            &[root],
            &CreateOptions::default(),
            &NoProgress,
            &ctl,
        )
        .unwrap();

    let out = tmp.path().join("out");
    extract_with(&archive, &out, SymlinkPolicy::Follow);

    // Direct and chained links become regular files with the target bytes.
    for name in ["tree/link.txt", "tree/chain.txt"] {
        let path = out.join(name);
        let meta = fs::symlink_metadata(&path).unwrap();
        assert!(meta.is_file(), "{name} must be a regular file");
        assert_eq!(fs::read(&path).unwrap(), b"link target content");
    }
    // Dangling and escaping targets are skipped, not errors.
    assert!(fs::symlink_metadata(out.join("tree/dangling")).is_err());
    assert!(fs::symlink_metadata(out.join("tree/escape")).is_err());
}

#[test]
fn zip_follow_skips_cycles() {
    let tmp = TempDir::new("follow-cycle");
    let root = tmp.path().join("tree");
    fs::create_dir_all(&root).unwrap();
    std::os::unix::fs::symlink("b", root.join("a")).unwrap();
    std::os::unix::fs::symlink("a", root.join("b")).unwrap();
    let archive = tmp.path().join("cycle.zip");
    let ctl = ControlToken::new();
    engine()
        .create(
            &archive,
            &[root],
            &CreateOptions::default(),
            &NoProgress,
            &ctl,
        )
        .unwrap();
    let out = tmp.path().join("out");
    extract_with(&archive, &out, SymlinkPolicy::Follow);
    assert!(fs::symlink_metadata(out.join("tree/a")).is_err());
    assert!(fs::symlink_metadata(out.join("tree/b")).is_err());
}

#[test]
fn preserve_policy_still_creates_symlinks() {
    let tmp = TempDir::new("preserve");
    let root = linked_tree(tmp.path());
    let archive = tmp.path().join("links.zip");
    engine()
        .create(
            &archive,
            &[root],
            &CreateOptions::default(),
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap();
    let out = tmp.path().join("out");
    extract_with(&archive, &out, SymlinkPolicy::Preserve);
    let meta = fs::symlink_metadata(out.join("tree/link.txt")).unwrap();
    assert!(meta.file_type().is_symlink());
}

/// Handcrafts a tar containing tree/original.txt plus a *hardlink* entry
/// tree/alias.txt -> tree/original.txt (file-system walking cannot detect
/// hardlinks, so the fixture is built with the tar crate directly).
fn hardlink_tar(path: &Path, content: &[u8]) {
    let mut builder = tar::Builder::new(fs::File::create(path).unwrap());
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(tar::EntryType::Regular);
    header.set_mode(0o644);
    header.set_size(content.len() as u64);
    builder
        .append_data(&mut header, "tree/original.txt", content)
        .unwrap();
    let mut link = tar::Header::new_gnu();
    link.set_entry_type(tar::EntryType::Link);
    link.set_mode(0o644);
    link.set_size(0);
    builder
        .append_link(&mut link, "tree/alias.txt", "tree/original.txt")
        .unwrap();
    builder.finish().unwrap();
}

/// Hardlinks travel through tar (zip has no hardlink entries); the second
/// name must restore as a hard link to the first file.
#[test]
fn tar_hardlink_restores_as_hard_link() {
    let tmp = TempDir::new("hardlink-tar");
    let archive = tmp.path().join("hard.tar");
    hardlink_tar(&archive, b"shared inode content");

    let out = tmp.path().join("out");
    extract_with(&archive, &out, SymlinkPolicy::Preserve);
    let a = fs::metadata(out.join("tree/original.txt")).unwrap();
    let b = fs::metadata(out.join("tree/alias.txt")).unwrap();
    assert_eq!(
        fs::read(out.join("tree/alias.txt")).unwrap(),
        b"shared inode content"
    );
    assert_eq!(a.ino(), b.ino(), "must share one inode");
}

/// When the hardlink's target is excluded from the extraction, tar's
/// single-pass driver cannot link to anything on disk and skips the entry
/// (documented limitation; the two-pass engine falls back to a content
/// copy instead).
#[test]
fn tar_hardlink_with_excluded_target_is_skipped() {
    let tmp = TempDir::new("hardlink-fallback");
    let archive = tmp.path().join("hard.tar");
    hardlink_tar(&archive, b"fallback content");

    let entries = engine().list(&archive, &OpenOptions::default()).unwrap();
    let alias = entries
        .iter()
        .find(|e| e.path.display.ends_with("alias.txt"))
        .unwrap()
        .path
        .clone();
    let out = tmp.path().join("out");
    engine()
        .extract(
            &archive,
            &out,
            Some(&[alias]),
            &OpenOptions::default(),
            &ExtractOptions::default(),
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap();
    assert!(fs::symlink_metadata(out.join("tree/alias.txt")).is_err());
    assert!(fs::symlink_metadata(out.join("tree/original.txt")).is_err());
}
