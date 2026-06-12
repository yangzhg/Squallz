//! Compound-format end-to-end tests: `.tar.gz`/`.tgz`/`.tar.bz2`/`.tar.xz`/
//! `.tar.zst` interop with the system bsdtar (both directions, no temp
//! files in our pipeline), plus the plain `.gz` single-entry virtual
//! archive. All fixtures are generated in code.

mod common;

use std::fs;
use std::path::Path;
use std::process::Command;

use common::{command_exists, engine, TempDir};
use squallz_core::api::{
    ControlToken, CreateOptions, EntryType, ExtractOptions, FormatError, NoProgress, OpenOptions,
};

/// Compound suffixes and the matching system-tar creation flag.
const COMBOS: [(&str, &str); 5] = [
    ("tar.gz", "-z"),
    ("tgz", "-z"),
    ("tar.bz2", "-j"),
    ("tar.xz", "-J"),
    ("tar.zst", "--zstd"),
];

fn build_tree(root: &Path) {
    fs::create_dir_all(root.join("sub/嵌套")).unwrap();
    fs::write(root.join("a.txt"), "hello compound world").unwrap();
    fs::write(root.join("sub/b.bin"), vec![7u8; 100_000]).unwrap();
    fs::write(root.join("sub/嵌套/中文.txt"), "中文内容").unwrap();
}

/// Compares the three fixture files between two extracted roots.
fn assert_tree_equal(a: &Path, b: &Path) {
    for rel in ["a.txt", "sub/b.bin", "sub/嵌套/中文.txt"] {
        assert_eq!(
            fs::read(a.join(rel)).unwrap(),
            fs::read(b.join(rel)).unwrap(),
            "mismatch at {rel}"
        );
    }
}

/// Whether the system tar can handle the given creation flag (bsdtar
/// builds may lack zstd support).
fn system_tar_supports(dir: &Path, flag: &str) -> bool {
    let probe_src = dir.join("probe");
    fs::create_dir_all(&probe_src).unwrap();
    fs::write(probe_src.join("x"), "x").unwrap();
    Command::new("tar")
        .arg("-c")
        .arg(flag)
        .arg("-f")
        .arg(dir.join("probe.out"))
        .arg("-C")
        .arg(dir)
        .arg("probe")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[test]
fn ours_to_system_tar_all_compound_suffixes() {
    if !command_exists("tar") {
        eprintln!("skipping: no system tar");
        return;
    }
    let engine = engine();
    let ctl = ControlToken::new();
    for (suffix, flag) in COMBOS {
        let dir = TempDir::new(&format!("compound-ours-{}", suffix.replace('.', "-")));
        if !system_tar_supports(dir.path(), flag) {
            eprintln!("skipping {suffix}: system tar lacks {flag}");
            continue;
        }
        let root = dir.path().join("tree");
        build_tree(&root);
        let archive = dir.path().join(format!("out.{suffix}"));
        engine
            .create(
                &archive,
                std::slice::from_ref(&root),
                &CreateOptions::default(),
                &NoProgress,
                &ctl,
            )
            .unwrap();

        // System tar must list and extract what we created.
        let list = Command::new("tar")
            .arg("-tf")
            .arg(&archive)
            .output()
            .unwrap();
        assert!(list.status.success(), "{suffix}: tar -tf failed");
        let listing = String::from_utf8_lossy(&list.stdout).into_owned();
        assert!(listing.contains("tree/a.txt"), "{suffix}: {listing}");

        let out = dir.path().join("sysout");
        fs::create_dir_all(&out).unwrap();
        let extract = Command::new("tar")
            .arg("-xf")
            .arg(&archive)
            .arg("-C")
            .arg(&out)
            .output()
            .unwrap();
        assert!(extract.status.success(), "{suffix}: tar -xf failed");
        assert_tree_equal(&out.join("tree"), &root);
    }
}

#[test]
fn system_tar_to_ours_all_compound_suffixes() {
    if !command_exists("tar") {
        eprintln!("skipping: no system tar");
        return;
    }
    let engine = engine();
    let ctl = ControlToken::new();
    for (suffix, flag) in COMBOS {
        let dir = TempDir::new(&format!("compound-sys-{}", suffix.replace('.', "-")));
        if !system_tar_supports(dir.path(), flag) {
            eprintln!("skipping {suffix}: system tar lacks {flag}");
            continue;
        }
        let root = dir.path().join("tree");
        build_tree(&root);
        let archive = dir.path().join(format!("sys.{suffix}"));
        let create = Command::new("tar")
            .arg("-c")
            .arg(flag)
            .arg("-f")
            .arg(&archive)
            .arg("-C")
            .arg(dir.path())
            .arg("tree")
            .output()
            .unwrap();
        assert!(create.status.success(), "{suffix}: tar -cf failed");

        // We must list, test and extract what system tar created.
        let entries = engine.list(&archive, &OpenOptions::default()).unwrap();
        assert!(
            entries.iter().any(|e| e.path.display.contains("a.txt")),
            "{suffix}: a.txt missing from listing"
        );
        let report = engine
            .test(&archive, &OpenOptions::default(), &NoProgress, &ctl)
            .unwrap();
        assert!(report.is_ok(), "{suffix}: {:?}", report.problems);
        let out = dir.path().join("ourout");
        engine
            .extract(
                &archive,
                &out,
                None,
                &OpenOptions::default(),
                &ExtractOptions::default(),
                &NoProgress,
                &ctl,
            )
            .unwrap();
        assert_tree_equal(&out.join("tree"), &root);
    }
}

#[test]
fn plain_gz_single_entry_virtual_archive() {
    let dir = TempDir::new("plain-gz");
    let src = dir.path().join("notes.txt");
    let content = "plain single-stream content 单流内容\n".repeat(1000);
    fs::write(&src, &content).unwrap();
    let engine = engine();
    let ctl = ControlToken::new();
    let archive = dir.path().join("notes.txt.gz");
    engine
        .create(
            &archive,
            std::slice::from_ref(&src),
            &CreateOptions::default(),
            &NoProgress,
            &ctl,
        )
        .unwrap();

    // list: one entry named without the .gz suffix, sized via gzip ISIZE.
    let entries = engine.list(&archive, &OpenOptions::default()).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].path.display, "notes.txt");
    assert!(matches!(entries[0].entry_type, EntryType::File));
    assert_eq!(entries[0].size, content.len() as u64);

    // test passes.
    let report = engine
        .test(&archive, &OpenOptions::default(), &NoProgress, &ctl)
        .unwrap();
    assert!(report.is_ok());

    // extract restores the payload.
    let out = dir.path().join("out");
    engine
        .extract(
            &archive,
            &out,
            None,
            &OpenOptions::default(),
            &ExtractOptions::default(),
            &NoProgress,
            &ctl,
        )
        .unwrap();
    assert_eq!(fs::read_to_string(out.join("notes.txt")).unwrap(), content);

    // System gzip reads our output, we read system gzip's output.
    if command_exists("gzip") {
        let check = Command::new("gzip")
            .arg("-t")
            .arg(&archive)
            .output()
            .unwrap();
        assert!(check.status.success(), "gzip -t rejected our file");

        let sys_src = dir.path().join("sys.txt");
        fs::write(&sys_src, "system gzip payload").unwrap();
        let gz = Command::new("gzip")
            .arg("sys.txt")
            .current_dir(dir.path())
            .output()
            .unwrap();
        assert!(gz.status.success());
        let sys_archive = dir.path().join("sys.txt.gz");
        let entries = engine.list(&sys_archive, &OpenOptions::default()).unwrap();
        assert_eq!(entries[0].path.display, "sys.txt");
        let out2 = dir.path().join("out2");
        engine
            .extract(
                &sys_archive,
                &out2,
                None,
                &OpenOptions::default(),
                &ExtractOptions::default(),
                &NoProgress,
                &ctl,
            )
            .unwrap();
        assert_eq!(
            fs::read_to_string(out2.join("sys.txt")).unwrap(),
            "system gzip payload"
        );
    } else {
        eprintln!("skipping gzip interop: no system gzip");
    }
}

#[test]
fn plain_compressor_rejects_multiple_inputs_and_directories() {
    let dir = TempDir::new("gz-multi");
    let a = dir.path().join("a.txt");
    let b = dir.path().join("b.txt");
    fs::write(&a, "a").unwrap();
    fs::write(&b, "b").unwrap();
    let engine = engine();
    let ctl = ControlToken::new();

    let err = engine
        .create(
            &dir.path().join("out.gz"),
            &[a, b],
            &CreateOptions::default(),
            &NoProgress,
            &ctl,
        )
        .unwrap_err();
    assert!(matches!(err, FormatError::Unsupported(_)));

    let sub = dir.path().join("sub");
    fs::create_dir_all(&sub).unwrap();
    let err = engine
        .create(
            &dir.path().join("dir.gz"),
            &[sub],
            &CreateOptions::default(),
            &NoProgress,
            &ctl,
        )
        .unwrap_err();
    assert!(matches!(err, FormatError::Unsupported(_)));
}

/// Selection-based extraction over a streamed compound source: only the
/// requested entry is written.
#[test]
fn compound_selective_extract() {
    let dir = TempDir::new("compound-select");
    let root = dir.path().join("tree");
    build_tree(&root);
    let engine = engine();
    let ctl = ControlToken::new();
    let archive = dir.path().join("sel.tar.gz");
    engine
        .create(
            &archive,
            &[root],
            &CreateOptions::default(),
            &NoProgress,
            &ctl,
        )
        .unwrap();
    let entries = engine.list(&archive, &OpenOptions::default()).unwrap();
    let pick: Vec<_> = entries
        .iter()
        .filter(|e| e.path.display == "tree/a.txt")
        .map(|e| e.path.clone())
        .collect();
    assert_eq!(pick.len(), 1);
    let out = dir.path().join("out");
    engine
        .extract(
            &archive,
            &out,
            Some(&pick),
            &OpenOptions::default(),
            &ExtractOptions::default(),
            &NoProgress,
            &ctl,
        )
        .unwrap();
    assert!(out.join("tree/a.txt").exists());
    assert!(!out.join("sub").exists() && !out.join("tree/sub").exists());
}
