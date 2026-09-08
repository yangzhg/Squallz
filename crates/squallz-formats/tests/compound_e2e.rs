//! Compound-format end-to-end tests: `.tar.gz`/`.tgz`/`.tar.bz2`/`.tar.xz`/
//! `.tar.zst` interop with the system bsdtar (both directions, no temp
//! files in our pipeline), plus the plain `.gz` single-entry virtual
//! archive. All fixtures are generated in code.

mod common;

use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::Command;

use common::{command_exists, engine, TempDir};
use flate2::write::GzEncoder;
use flate2::Compression;
use squallz_core::api::{
    ControlToken, CreateOptions, EntryType, ExtractOptions, FormatError, NoProgress, OpenOptions,
    SafetyLimits,
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

fn write_single_byte_tar_gz(archive: &Path, trailing_zeros: usize) {
    let mut tar_bytes = Vec::new();
    {
        let mut builder = tar::Builder::new(&mut tar_bytes);
        let mut header = tar::Header::new_gnu();
        header.set_path("file.txt").unwrap();
        header.set_size(1);
        header.set_mode(0o644);
        header.set_cksum();
        builder.append(&header, &b"x"[..]).unwrap();
        builder.finish().unwrap();
    }
    tar_bytes.resize(tar_bytes.len() + trailing_zeros, 0);
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&tar_bytes).unwrap();
    fs::write(archive, encoder.finish().unwrap()).unwrap();
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
            .test_summary(&archive, &OpenOptions::default(), &NoProgress, &ctl)
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

#[cfg(unix)]
#[test]
fn opened_compound_reader_does_not_follow_a_replaced_source_path() {
    let dir = TempDir::new("compound-opened-source-binding");
    let original_root = dir.path().join("original/tree");
    let replacement_root = dir.path().join("replacement/tree");
    fs::create_dir_all(&original_root).unwrap();
    fs::create_dir_all(&replacement_root).unwrap();
    fs::write(original_root.join("payload.txt"), "original payload").unwrap();
    fs::write(replacement_root.join("payload.txt"), "replacement payload").unwrap();
    let archive = dir.path().join("source.tar.gz");
    let replacement = dir.path().join("replacement.tar.gz");
    let engine = engine();
    let ctl = ControlToken::new();
    engine
        .create(
            &archive,
            std::slice::from_ref(&original_root),
            &CreateOptions::default(),
            &NoProgress,
            &ctl,
        )
        .unwrap();
    engine
        .create(
            &replacement,
            std::slice::from_ref(&replacement_root),
            &CreateOptions::default(),
            &NoProgress,
            &ctl,
        )
        .unwrap();

    let mut reader = engine.open(&archive, &OpenOptions::default()).unwrap();
    fs::remove_file(&archive).unwrap();
    fs::rename(&replacement, &archive).unwrap();

    let entries = reader.entries().collect::<Result<Vec<_>, _>>().unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[1].path.display, "tree/payload.txt");
    assert!(reader.test_summary(&NoProgress, &ctl).unwrap().is_ok());
    let out = dir.path().join("out");
    reader
        .extract(&out, None, &ExtractOptions::default(), &NoProgress, &ctl)
        .unwrap();
    assert_eq!(
        fs::read_to_string(out.join("tree/payload.txt")).unwrap(),
        "original payload"
    );
}

#[test]
fn damaged_gzip_trailer_fails_tar_test_and_extract() {
    let dir = TempDir::new("compound-damaged-gzip-trailer");
    let root = dir.path().join("tree");
    build_tree(&root);
    let engine = engine();
    let ctl = ControlToken::new();
    let archive = dir.path().join("damaged.tar.gz");
    engine
        .create(
            &archive,
            &[root],
            &CreateOptions::default(),
            &NoProgress,
            &ctl,
        )
        .unwrap();

    let mut bytes = fs::read(&archive).unwrap();
    let trailer_crc = bytes.len().checked_sub(8).unwrap();
    bytes[trailer_crc] ^= 0x5a;
    fs::write(&archive, bytes).unwrap();

    let report = engine
        .test_summary(&archive, &OpenOptions::default(), &NoProgress, &ctl)
        .unwrap();
    assert!(
        !report.is_ok(),
        "testing tar.gz must validate the gzip trailer"
    );

    let err = engine
        .extract(
            &archive,
            &dir.path().join("out"),
            None,
            &OpenOptions::default(),
            &ExtractOptions::default(),
            &NoProgress,
            &ctl,
        )
        .unwrap_err();
    assert!(
        matches!(err, FormatError::Io(_) | FormatError::CorruptArchive(_)),
        "extracting tar.gz must validate the gzip trailer: {err}"
    );
}

#[test]
fn compound_trailer_drain_does_not_consume_extract_output_budget() {
    let dir = TempDir::new("compound-trailer-separate-output-budget");
    let archive = dir.path().join("single-byte.tar.gz");
    write_single_byte_tar_gz(&archive, 0);

    let opts = ExtractOptions {
        limits: SafetyLimits {
            max_output_bytes: 1,
            ..SafetyLimits::default()
        },
        ..ExtractOptions::default()
    };
    let out = dir.path().join("out");
    engine()
        .extract(
            &archive,
            &out,
            None,
            &OpenOptions::default(),
            &opts,
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap();
    assert_eq!(fs::read(out.join("file.txt")).unwrap(), b"x");
}

#[test]
fn compound_trailer_drain_has_a_fixed_safety_limit() {
    let dir = TempDir::new("compound-trailer-fixed-limit");
    let archive = dir.path().join("padded.tar.gz");
    write_single_byte_tar_gz(&archive, 17 * 1024 * 1024);

    let err = engine()
        .extract(
            &archive,
            &dir.path().join("out"),
            None,
            &OpenOptions::default(),
            &ExtractOptions::default(),
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap_err();
    assert!(matches!(err, FormatError::ResourceLimitExceeded(_)));
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
        .test_summary(&archive, &OpenOptions::default(), &NoProgress, &ctl)
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
