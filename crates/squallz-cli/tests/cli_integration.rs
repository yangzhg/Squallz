//! End-to-end CLI integration tests, driving the real `sqz` binary through
//! `CARGO_BIN_EXE_sqz` (no extra harness dependency).
//!
//! Every invocation pins the language environment (`SQZ_LANG` removed,
//! `SQZ_LOCALES_DIR` pointed at a non-existent directory) so the assertions
//! are independent of the developer's machine.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const SQZ_RECOVERY_BLOCK: usize = 64 * 1024;
const RAR5_MAGIC: &[u8] = b"Rar!\x1A\x07\x01\x00";

/// A fresh `sqz` command with a deterministic i18n environment.
fn sqz() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_sqz"));
    cmd.env_remove("SQZ_LANG");
    cmd.env("SQZ_LOCALES_DIR", "/nonexistent/squallz-test-locales");
    cmd
}

fn run(cmd: &mut Command) -> Output {
    cmd.output().expect("failed to run sqz")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn stdout_json(out: &Output) -> serde_json::Value {
    serde_json::from_str(&stdout(out)).expect("valid JSON")
}

fn write_fake_executable(dir: &Path, name: &str) -> PathBuf {
    let path = if cfg!(windows) {
        dir.join(format!("{name}.exe"))
    } else {
        dir.join(name)
    };
    std::fs::write(&path, "#!/bin/sh\nexit 0\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).unwrap();
    }
    path
}

fn assert_no_i18n_keys(text: &str) {
    for token in ["cli.info.", "common.yes", "common.no"] {
        assert!(
            !text.contains(token),
            "human output leaked i18n key {token}: {text}"
        );
    }
}

fn assert_json_error(out: &Output, code: i32, kind: &str, message_part: &str) {
    assert_eq!(out.status.code(), Some(code), "stderr: {}", stderr(out));
    assert!(
        stderr(out).trim().is_empty(),
        "JSON error path must not also emit human stderr: {}",
        stderr(out)
    );
    let report = stdout_json(out);
    assert_eq!(report["ok"], false);
    assert_eq!(report["error"]["kind"], kind);
    assert_eq!(report["error"]["exit_code"], code);
    assert!(
        report["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains(message_part)),
        "stdout: {}",
        stdout(out)
    );
}

#[test]
fn cli_surface_contract_help_tokens_are_stable() {
    let root_help = run(sqz().arg("--help"));
    assert!(root_help.status.success(), "stderr: {}", stderr(&root_help));
    let help = stdout(&root_help);
    for token in [
        "compress",
        "pack",
        "estimate",
        "duplicates",
        "checksum",
        "extract",
        "list",
        "test",
        "convert",
        "nested",
        "export",
        "update",
        "protect",
        "verify",
        "repair",
        "batch",
        "doctor",
        "info",
        "--lang",
        "--quiet",
        "--verbose",
        "--style",
        "--color",
        "--accent",
        "--palette",
        "--theme",
        "--color-scheme",
        "--scheme",
    ] {
        assert!(help.contains(token), "root help missing {token}: {help}");
    }

    let surfaces: &[(&[&str], &[&str])] = &[
        (
            &["compress", "--help"],
            &[
                "--format",
                "--profile",
                "--split",
                "--threads",
                "--memory-limit",
                "--json",
                "--password",
                "--encrypt-names",
                "--exclude",
            ],
        ),
        (
            &["pack", "--help"],
            &[
                "--inner-format",
                "--profile",
                "--recovery",
                "--split",
                "--threads",
                "--memory-limit",
                "--json",
            ],
        ),
        (
            &["extract", "--help"],
            &[
                "--include",
                "--overwrite",
                "--encoding",
                "--symlinks",
                "--smart",
                "--best-effort",
                "--max-output-bytes",
                "--max-entries",
                "--max-compression-ratio",
                "--json",
            ],
        ),
        (
            &["duplicates", "--help"],
            &["--exclude", "--min-size", "--json"],
        ),
        (
            &["checksum", "--help"],
            &["--algorithm", "--check", "--exclude", "--json"],
        ),
        (&["list", "--help"], &["--json", "--tree"]),
        (
            &["nested", "list", "--help"],
            &[
                "--password",
                "--encoding",
                "--nested-password",
                "--nested-encoding",
                "--json",
                "--tree",
            ],
        ),
        (
            &["nested", "extract", "--help"],
            &[
                "--include",
                "--overwrite",
                "--encoding",
                "--symlinks",
                "--smart",
                "--best-effort",
                "--max-output-bytes",
                "--max-entries",
                "--max-compression-ratio",
                "--json",
            ],
        ),
        (
            &["convert", "--help"],
            &["--profile", "--password", "--encoding", "--json"],
        ),
        (&["export", "--help"], &["--profile", "--output", "--json"]),
        (
            &["update", "--help"],
            &[
                "--add",
                "--mkdir",
                "--delete",
                "--rename",
                "--move",
                "--profile",
                "--exclude",
                "--json",
            ],
        ),
        (
            &["protect", "--help"],
            &["--recovery", "--redundancy", "--tolerate-loss", "--json"],
        ),
        (
            &["verify", "--help"],
            &["--recovery", "--use-recovery", "--json"],
        ),
        (
            &["repair", "--help"],
            &[
                "--recovery",
                "--use-recovery",
                "--output",
                "--profile",
                "--json",
            ],
        ),
        (&["batch", "--help"], &["--keep-going", "--json", "script"]),
        (&["doctor", "--help"], &["--strict", "--json"]),
    ];

    for (args, tokens) in surfaces {
        let out = run(sqz().args(args.iter().copied()));
        assert!(out.status.success(), "{args:?} stderr: {}", stderr(&out));
        let help = stdout(&out);
        for token in *tokens {
            assert!(
                help.contains(token),
                "{args:?} help missing {token}: {help}"
            );
        }
    }
}

#[test]
fn localized_help_uses_requested_english_surface() {
    let root_help = run(sqz().args(["--lang", "en-US", "--help"]));
    assert!(root_help.status.success(), "stderr: {}", stderr(&root_help));
    let help = stdout(&root_help);
    assert!(
        help.contains("Squallz: cross-platform archive manager"),
        "stdout: {help}"
    );
    assert!(help.contains("Compress files or folders"), "stdout: {help}");
    assert!(
        help.contains("Human-readable output style"),
        "stdout: {help}"
    );
    assert!(
        help.contains("auto, always, rich, fancy, or never"),
        "stdout: {help}"
    );
    for palette in ["squallz", "brand", "icon", "surge", "glass", "teal", "mono"] {
        assert!(help.contains(palette), "stdout missing {palette}: {help}");
    }
    assert!(
        help.contains("--color-scheme") && help.contains("--scheme") && help.contains("--colors"),
        "stdout: {help}"
    );
    assert!(
        !help.contains("压缩文件/目录") && !help.contains("跨平台压缩解压工具"),
        "stdout: {help}"
    );

    let compress_help = run(sqz().args(["compress", "--lang", "en-US", "--help"]));
    assert!(
        compress_help.status.success(),
        "stderr: {}",
        stderr(&compress_help)
    );
    let help = stdout(&compress_help);
    assert!(
        help.contains("Output archive path. The format is detected from the file extension."),
        "stdout: {help}"
    );
    assert!(help.contains("--memory-limit"), "stdout: {help}");
    assert!(!help.contains("输入文件或目录"), "stdout: {help}");

    let nested_help = run(sqz()
        .env("SQZ_LANG", "en-US")
        .args(["nested", "extract", "--help"]));
    assert!(
        nested_help.status.success(),
        "stderr: {}",
        stderr(&nested_help)
    );
    let help = stdout(&nested_help);
    assert!(
        help.contains("Extract nested archive contents"),
        "stdout: {help}"
    );
    assert!(
        help.contains("Nested archive decryption password"),
        "stdout: {help}"
    );

    let zh_root_help = run(sqz().args(["--lang", "zh-CN", "--help"]));
    assert!(
        zh_root_help.status.success(),
        "stderr: {}",
        stderr(&zh_root_help)
    );
    let help = stdout(&zh_root_help);
    assert!(
        help.contains("跨平台压缩解压工具") && help.contains("压缩文件/目录"),
        "stdout: {help}"
    );
}

#[test]
fn output_style_modern_is_opt_in_and_keeps_json_stable() {
    let dir = temp_dir("output-style-modern");
    let root = sample_tree(&dir);
    let archive = dir.join("modern.zip");

    let out = run(sqz()
        .args(["--lang", "en-US", "--style", "modern", "compress"])
        .arg(&root)
        .arg("-o")
        .arg(&archive));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("Archive created"), "stdout: {text}");
    assert!(text.contains("Archive summary"), "stdout: {text}");
    assert!(text.contains("Create plan"), "stdout: {text}");
    assert!(text.contains("Create route"), "stdout: {text}");
    assert!(text.contains("Create details"), "stdout: {text}");
    assert!(text.contains("sqz test"), "stdout: {text}");
    assert!(text.contains("Create settings"), "stdout: {text}");
    assert!(text.contains("Source scan"), "stdout: {text}");
    assert!(text.contains("Write archive"), "stdout: {text}");
    assert!(text.contains("│ Status"), "stdout: {text}");
    assert!(text.contains("│ Format"), "stdout: {text}");
    assert!(text.contains("Level"), "stdout: {text}");
    assert!(text.contains("Volumes"), "stdout: {text}");
    assert!(text.contains("Output size"), "stdout: {text}");
    assert!(text.contains("│ Output"), "stdout: {text}");

    let sqz_pack = dir.join("modern-pack.sqz");
    let out = run(sqz()
        .args(["--lang", "en-US", "--style", "modern", "pack"])
        .arg(&root)
        .arg("-o")
        .arg(&sqz_pack)
        .args(["--inner-format", "tar", "--recovery", "12%"]));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("Archive created"), "stdout: {text}");
    assert!(text.contains("Archive summary"), "stdout: {text}");
    assert!(text.contains("Create plan"), "stdout: {text}");
    assert!(text.contains("Create route"), "stdout: {text}");
    assert!(text.contains("Create details"), "stdout: {text}");
    assert!(text.contains("sqz test"), "stdout: {text}");
    assert!(text.contains("Create settings"), "stdout: {text}");
    assert!(text.contains("SQZ container"), "stdout: {text}");
    assert!(text.contains("Inner archive"), "stdout: {text}");
    assert!(text.contains("Recovery redundancy"), "stdout: {text}");
    assert!(text.contains("tar"), "stdout: {text}");
    assert!(text.contains("12%"), "stdout: {text}");
    assert!(text.contains("┬"), "stdout: {text}");
    assert!(text.contains("┼"), "stdout: {text}");

    let out = run(sqz()
        .args(["--lang", "en-US", "--style", "modern", "extract"])
        .arg(&archive)
        .arg("-d")
        .arg(dir.join("extract-modern")));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("Extract complete"), "stdout: {text}");
    assert!(text.contains("Extraction summary"), "stdout: {text}");
    assert!(text.contains("Extraction plan"), "stdout: {text}");
    assert!(text.contains("Extraction route"), "stdout: {text}");
    assert!(text.contains("Extraction details"), "stdout: {text}");
    assert!(text.contains("Archive"), "stdout: {text}");
    assert!(text.contains("Destination"), "stdout: {text}");
    assert!(text.contains("Open archive"), "stdout: {text}");
    assert!(text.contains("Write files"), "stdout: {text}");
    assert!(text.contains("Extraction policy"), "stdout: {text}");
    assert!(text.contains("Selection"), "stdout: {text}");
    assert!(text.contains("Safety limits"), "stdout: {text}");
    assert!(text.contains("all entries"), "stdout: {text}");

    let out = run(sqz()
        .args([
            "--lang",
            "en-US",
            "--style",
            "modern",
            "extract",
            "--include",
            "does-not-exist-*",
        ])
        .arg(&archive)
        .arg("-d")
        .arg(dir.join("extract-modern-empty")));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("No matching entries"), "stdout: {text}");
    assert!(
        text.contains("Warning: no entries matched"),
        "stdout: {text}"
    );
    assert!(text.contains("Extraction policy"), "stdout: {text}");
    assert!(text.contains("Selection"), "stdout: {text}");
    assert!(text.contains("1 pattern"), "stdout: {text}");
    assert!(text.contains("strict"), "stdout: {text}");
    assert!(text.contains("┬"), "stdout: {text}");
    assert!(text.contains("┼"), "stdout: {text}");

    let out = run(sqz()
        .args(["--lang", "en-US", "--style", "modern", "list"])
        .arg(&archive));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("Archive contents"), "stdout: {text}");
    assert!(text.contains("Archive summary"), "stdout: {text}");
    assert!(text.contains("Entry mix"), "stdout: {text}");
    assert!(text.contains("│  Entries"), "stdout: {text}");
    assert!(text.contains("╭─ Entries"), "stdout: {text}");
    assert!(text.contains("│"), "stdout: {text}");
    assert!(text.contains("✓ "), "stdout: {text}");

    let out = run(sqz()
        .args(["--lang", "en-US", "--style", "modern", "test"])
        .arg(&archive));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("Integrity check passed"), "stdout: {text}");
    assert!(text.contains("│ Status"), "stdout: {text}");
    assert!(text.contains("Entries"), "stdout: {text}");
    assert!(text.contains("Problems"), "stdout: {text}");

    let converted = dir.join("modern-converted.tar");
    let out = run(sqz()
        .args(["--lang", "en-US", "--style", "modern", "convert"])
        .arg(&archive)
        .arg("-o")
        .arg(&converted));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("Conversion complete"), "stdout: {text}");
    assert!(text.contains("Conversion plan"), "stdout: {text}");
    assert!(text.contains("Output policy"), "stdout: {text}");
    assert!(text.contains("zip"), "stdout: {text}");
    assert!(text.contains("tar"), "stdout: {text}");
    assert!(text.contains("Destination encryption"), "stdout: {text}");
    assert!(text.contains("Encrypted filenames"), "stdout: {text}");
    assert!(text.contains("Output size"), "stdout: {text}");
    assert!(text.contains("┬"), "stdout: {text}");
    assert!(text.contains("┼"), "stdout: {text}");
    assert!(converted.is_file());

    let out = run(sqz()
        .args([
            "--lang",
            "en-US",
            "--style",
            "modern",
            "update",
            "--mkdir",
            "docs/",
            "--move",
            "project/sub/b.txt=docs/b.txt",
        ])
        .arg(&archive));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("Archive updated"), "stdout: {text}");
    assert!(text.contains("Update plan"), "stdout: {text}");
    assert!(text.contains("Write policy"), "stdout: {text}");
    assert!(text.contains("Create dirs"), "stdout: {text}");
    assert!(text.contains("Move entries"), "stdout: {text}");
    assert!(text.contains("Touched entries"), "stdout: {text}");
    assert!(text.contains("Encrypted filenames"), "stdout: {text}");
    assert!(text.contains("Exclude patterns"), "stdout: {text}");
    assert!(text.contains("┬"), "stdout: {text}");
    assert!(text.contains("┼"), "stdout: {text}");

    let sqz_archive = dir.join("modern-source.sqz");
    let out = run(sqz().arg("compress").arg(&root).arg("-o").arg(&sqz_archive));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let exported = dir.join("modern-exported.zip");
    let out = run(sqz()
        .args(["--lang", "en-US", "--style", "modern", "export"])
        .arg(&sqz_archive)
        .arg("-o")
        .arg(&exported));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("Export complete"), "stdout: {text}");
    assert!(text.contains("Export plan"), "stdout: {text}");
    assert!(text.contains("Output policy"), "stdout: {text}");
    assert!(text.contains("SQZ container"), "stdout: {text}");
    assert!(text.contains("Lock-in"), "stdout: {text}");
    assert!(text.contains("standard archive output"), "stdout: {text}");
    assert!(text.contains("Destination encryption"), "stdout: {text}");
    assert!(text.contains("Output size"), "stdout: {text}");
    assert!(text.contains("zip"), "stdout: {text}");
    assert!(text.contains("┬"), "stdout: {text}");
    assert!(text.contains("┼"), "stdout: {text}");
    assert!(exported.is_file());

    let repaired_sqz = dir.join("modern-repaired.sqz");
    let out = run(sqz()
        .args(["--lang", "en-US", "--style", "modern", "repair"])
        .arg(&sqz_archive)
        .arg("-o")
        .arg(&repaired_sqz));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("SQZ repair complete"), "stdout: {text}");
    assert!(text.contains("Repair report"), "stdout: {text}");
    assert!(text.contains("repair_sqz"), "stdout: {text}");
    assert!(text.contains("sqz-embedded-recovery"), "stdout: {text}");
    assert!(text.contains("In place"), "stdout: {text}");
    assert!(text.contains("false"), "stdout: {text}");
    assert!(text.contains("┬"), "stdout: {text}");
    assert!(text.contains("┼"), "stdout: {text}");
    assert!(repaired_sqz.is_file());

    let rebuilt_zip = dir.join("modern-rebuilt.zip");
    let out = run(sqz()
        .args(["--lang", "en-US", "--style", "modern", "repair"])
        .arg(&archive)
        .arg("-o")
        .arg(&rebuilt_zip));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("ZIP repair complete"), "stdout: {text}");
    assert!(text.contains("Repair report"), "stdout: {text}");
    assert!(text.contains("repair_zip"), "stdout: {text}");
    assert!(text.contains("zip-local-header-rebuild"), "stdout: {text}");
    assert!(text.contains("In place"), "stdout: {text}");
    assert!(text.contains("false"), "stdout: {text}");
    assert!(text.contains("┬"), "stdout: {text}");
    assert!(text.contains("┼"), "stdout: {text}");
    assert!(rebuilt_zip.is_file());

    let corrupt_archive = dir.join("modern-corrupt.zip");
    let corrupt_root = dir.join("modern-corrupt-src");
    std::fs::create_dir_all(&corrupt_root).unwrap();
    std::fs::write(corrupt_root.join("bad.txt"), b"visible corruption payload").unwrap();
    let out = run(sqz()
        .args(["--lang", "en-US", "compress", "--level", "0"])
        .arg(&corrupt_root)
        .arg("-o")
        .arg(&corrupt_archive));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    corrupt_stored_zip_payload(&corrupt_archive, b"visible corruption payload");
    let out = run(sqz()
        .args(["--lang", "en-US", "--style", "modern", "test"])
        .arg(&corrupt_archive));
    assert_eq!(out.status.code(), Some(3), "stderr: {}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("Integrity check failed"), "stdout: {text}");
    assert!(text.contains("Problem details"), "stdout: {text}");
    assert!(text.contains("bad.txt"), "stdout: {text}");
    assert!(text.contains("checksum"), "stdout: {text}");
    assert!(text.contains("┬"), "stdout: {text}");
    assert!(text.contains("┼"), "stdout: {text}");

    let planned = dir.join("planned.zip");
    let out = run(sqz()
        .args(["--lang", "en-US", "--style", "modern", "estimate"])
        .arg(&root)
        .arg("-o")
        .arg(&planned));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("Input estimate"), "stdout: {text}");
    assert!(text.contains("Input composition"), "stdout: {text}");
    assert!(text.contains("Output budget"), "stdout: {text}");
    assert!(text.contains("Disk preflight"), "stdout: {text}");
    assert!(text.contains("Input roots"), "stdout: {text}");
    assert!(text.contains("File payload"), "stdout: {text}");
    assert!(text.contains("Safety reserve"), "stdout: {text}");
    assert!(text.contains("Required output"), "stdout: {text}");
    assert!(text.contains("Count"), "stdout: {text}");
    assert!(text.contains("Size"), "stdout: {text}");
    assert!(text.contains("Path"), "stdout: {text}");
    assert!(text.contains("Available"), "stdout: {text}");
    assert!(text.contains("Required"), "stdout: {text}");
    assert!(text.contains("┬"), "stdout: {text}");
    assert!(text.contains("┼"), "stdout: {text}");

    let dup_root = dir.join("duplicates");
    std::fs::create_dir_all(dup_root.join("ignored")).unwrap();
    std::fs::write(dup_root.join("a.bin"), b"same duplicate payload").unwrap();
    std::fs::write(dup_root.join("b.bin"), b"same duplicate payload").unwrap();
    std::fs::write(dup_root.join("c.bin"), b"unique duplicate payload").unwrap();
    std::fs::write(dup_root.join("ignored/d.bin"), b"same duplicate payload").unwrap();

    let out = run(sqz()
        .args([
            "--lang",
            "en-US",
            "--style",
            "modern",
            "duplicates",
            "--exclude",
            "ignored",
        ])
        .arg(&dup_root));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("Duplicate scan"), "stdout: {text}");
    assert!(text.contains("Scan summary"), "stdout: {text}");
    assert!(text.contains("Duplicate groups"), "stdout: {text}");
    assert!(text.contains("Duplicate paths"), "stdout: {text}");
    assert!(text.contains("BLAKE3"), "stdout: {text}");
    assert!(text.contains("Reclaimable"), "stdout: {text}");
    assert!(text.contains("│"), "stdout: {text}");

    let out = run(sqz()
        .args(["--style", "modern", "duplicates"])
        .arg(&dup_root)
        .arg("--json"));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(
        !stdout(&out).contains('│'),
        "JSON stdout must not inherit modern tables: {}",
        stdout(&out)
    );
    let report = stdout_json(&out);
    assert_eq!(report["ok"], true);
    assert_eq!(report["operation"], "duplicates");
    assert_eq!(report["hash_algorithm"], "blake3");
    assert_eq!(report["duplicate_groups"], 1);
    assert_eq!(report["duplicate_files"], 3);
    assert!(report["reclaimable_bytes"].as_u64().unwrap() > 0);

    let checksum_root = dir.join("checksum");
    std::fs::create_dir_all(checksum_root.join("ignored")).unwrap();
    std::fs::write(checksum_root.join("a.txt"), b"abc").unwrap();
    std::fs::write(checksum_root.join("ignored/b.txt"), b"ignored").unwrap();

    let out = run(sqz()
        .args([
            "--lang",
            "en-US",
            "--style",
            "modern",
            "checksum",
            "--algorithm",
            "sha256",
            "--exclude",
            "ignored",
        ])
        .arg(&checksum_root));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("Checksum report"), "stdout: {text}");
    assert!(text.contains("Checksums"), "stdout: {text}");
    assert!(text.contains("sha256"), "stdout: {text}");
    assert!(
        text.contains("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"),
        "stdout: {text}"
    );
    assert!(text.contains("│"), "stdout: {text}");

    let out = run(sqz()
        .args(["--style", "modern", "checksum", "--algorithm", "crc32"])
        .arg(&checksum_root)
        .arg("--exclude")
        .arg("ignored")
        .arg("--json"));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(
        !stdout(&out).contains('│'),
        "JSON stdout must not inherit modern tables: {}",
        stdout(&out)
    );
    let report = stdout_json(&out);
    assert_eq!(report["ok"], true);
    assert_eq!(report["operation"], "checksum");
    assert_eq!(report["algorithm"], "crc32");
    assert_eq!(report["files_hashed"], 1);
    assert_eq!(report["items"][0]["digest"], "352441c2");

    let manifest = checksum_root.join("SHA256SUMS");
    std::fs::write(
        &manifest,
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad  a.txt\n",
    )
    .unwrap();
    let out = run(sqz()
        .args([
            "--lang", "en-US", "--style", "modern", "checksum", "--check",
        ])
        .arg(&manifest));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("Checksum verification"), "stdout: {text}");
    assert!(text.contains("Verification results"), "stdout: {text}");
    assert!(text.contains("1 passed"), "stdout: {text}");
    assert!(text.contains("OK"), "stdout: {text}");
    assert!(text.contains("│"), "stdout: {text}");

    std::fs::write(
        &manifest,
        concat!(
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad  a.txt\n",
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad  missing.txt\n",
        ),
    )
    .unwrap();
    let out = run(sqz()
        .args(["checksum", "--check"])
        .arg(&manifest)
        .arg("--json"));
    assert_eq!(out.status.code(), Some(3), "stderr: {}", stderr(&out));
    assert!(
        stderr(&out).trim().is_empty(),
        "JSON checksum failures must not emit human stderr: {}",
        stderr(&out)
    );
    let report = stdout_json(&out);
    assert_eq!(report["ok"], false);
    assert_eq!(report["operation"], "checksum_check");
    assert_eq!(report["checked"], 2);
    assert_eq!(report["passed"], 1);
    assert_eq!(report["failed"], 1);
    assert_eq!(report["items"][1]["ok"], false);

    let out = run(sqz().args(["--lang", "en-US", "--style", "modern", "info"]));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("Supported formats"), "stdout: {text}");
    assert!(text.contains("Engine"), "stdout: {text}");
    assert!(text.contains("Runtime inventory"), "stdout: {text}");
    assert!(text.contains("Command forms"), "stdout: {text}");
    assert!(
        text.contains("scorecard + decision tables"),
        "stdout: {text}"
    );
    assert!(
        text.contains("operation cockpit")
            && text.contains("snapshot dashboard")
            && text.contains("signal matrix")
            && text.contains("transfer matrix")
            && text.contains("action queue"),
        "stdout: {text}"
    );
    assert!(text.contains("Modern dashboard"), "stdout: {text}");
    assert!(text.contains("Support map"), "stdout: {text}");
    assert!(text.contains("Format coverage"), "stdout: {text}");
    assert!(text.contains("Capability lanes"), "stdout: {text}");
    assert!(text.contains("Action selector"), "stdout: {text}");
    assert!(text.contains("Modern surfaces"), "stdout: {text}");
    assert!(text.contains("Best form"), "stdout: {text}");
    assert!(text.contains("action queue"), "stdout: {text}");
    assert!(text.contains("Command cheatsheet"), "stdout: {text}");
    assert!(
        text.contains("phase rail") && text.contains("speed/ETA/current"),
        "stdout: {text}"
    );
    assert!(
        text.contains("next step") && text.contains("current object"),
        "stdout: {text}"
    );
    assert!(text.contains("Progress HUD"), "stdout: {text}");
    assert!(text.contains("snapshot dashboard table"), "stdout: {text}");
    assert!(text.contains("speed"), "stdout: {text}");
    assert!(text.contains("Modern style guide"), "stdout: {text}");
    assert!(text.contains("operation cockpit"), "stdout: {text}");
    assert!(text.contains("--color fancy"), "stdout: {text}");
    assert!(text.contains("--color rich"), "stdout: {text}");
    assert!(text.contains("Palette gallery"), "stdout: {text}");
    assert!(text.contains("--palette brand"), "stdout: {text}");
    assert!(text.contains("--palette cascade"), "stdout: {text}");
    assert!(text.contains("--palette daylight"), "stdout: {text}");
    assert!(text.contains("--palette foam"), "stdout: {text}");
    assert!(text.contains("--palette skyline"), "stdout: {text}");
    assert!(text.contains("--palette aero"), "stdout: {text}");
    assert!(text.contains("--palette crest"), "stdout: {text}");
    assert!(text.contains("--palette halo"), "stdout: {text}");
    assert!(text.contains("--palette tropic"), "stdout: {text}");
    assert!(text.contains("--palette kinetic"), "stdout: {text}");
    assert!(text.contains("--palette radiant"), "stdout: {text}");
    assert!(text.contains("--palette crystal"), "stdout: {text}");
    assert!(text.contains("--palette lumina"), "stdout: {text}");
    assert!(text.contains("--colors glass"), "stdout: {text}");
    assert!(text.contains("--colors icon"), "stdout: {text}");
    assert!(text.contains("Color scheme"), "stdout: {text}");
    assert!(text.contains("--color-scheme / --scheme"), "stdout: {text}");
    assert!(text.contains("--colors"), "stdout: {text}");
    assert!(text.contains("Unpack archives"), "stdout: {text}");
    assert!(
        text.contains("sqz extract archive -d out --smart"),
        "stdout: {text}"
    );
    assert!(text.contains("Ready now"), "stdout: {text}");
    assert!(text.contains("Needs tools"), "stdout: {text}");
    assert!(text.contains("Read"), "stdout: {text}");
    assert!(text.contains("Write"), "stdout: {text}");
    assert_no_i18n_keys(&text);

    let out = run(sqz().args([
        "--lang", "en-US", "--style", "modern", "--color", "never", "doctor",
    ]));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("Runtime doctor"), "stdout: {text}");
    assert!(text.contains("Runtime checks"), "stdout: {text}");
    assert!(text.contains("rar-product-boundary"), "stdout: {text}");
    assert!(
        text.contains("unpack-only through external 7zz/7z"),
        "stdout: {text}"
    );
    assert!(text.contains("diagnostic fallback"), "stdout: {text}");
    assert!(text.contains("RAR creation"), "stdout: {text}");
    assert!(text.contains("encrypted/full"), "stdout: {text}");
    assert!(text.contains("documented"), "stdout: {text}");
    assert!(text.contains("limitations"), "stdout: {text}");
    assert!(text.contains("┬"), "stdout: {text}");
    assert!(text.contains("┼"), "stdout: {text}");

    let out = run(sqz()
        .args(["--style", "modern", "list"])
        .arg(&archive)
        .arg("--json"));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(
        !stdout(&out).starts_with('✓'),
        "JSON stdout must not inherit modern decoration: {}",
        stdout(&out)
    );
    let entries = stdout_json(&out);
    assert!(entries
        .as_array()
        .is_some_and(|items| items.iter().any(|item| item["path"] == "project/a.txt")));

    let out = run(sqz()
        .args(["--lang", "en-US", "--style", "modern", "list"])
        .arg(dir.join("missing.zip")));
    assert_eq!(out.status.code(), Some(7), "stdout: {}", stdout(&out));
    assert!(
        stderr(&out).starts_with("✕ Error:"),
        "stderr: {}",
        stderr(&out)
    );

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn color_option_controls_modern_human_output_only() {
    let out = run(sqz().args([
        "--lang", "en-US", "--style", "modern", "--color", "always", "info",
    ]));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(
        stdout(&out).contains("\x1b["),
        "--color always should colorize modern human stdout: {}",
        stdout(&out)
    );

    let out = run(sqz().args([
        "--lang", "en-US", "--style", "modern", "--color", "never", "info",
    ]));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(
        !stdout(&out).contains("\x1b["),
        "--color never must suppress ANSI: {}",
        stdout(&out)
    );

    let out = run(sqz().args([
        "--lang", "en-US", "--style", "modern", "--color", "rich", "info",
    ]));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(
        stdout(&out).contains("\x1b["),
        "--color rich should force ANSI for modern demos and redirected previews: {}",
        stdout(&out)
    );

    let out = run(sqz().args([
        "--lang", "en-US", "--style", "modern", "--color", "fancy", "info",
    ]));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(
        stdout(&out).contains("\x1b["),
        "--color fancy should force ANSI for modern live-progress demos and redirected previews: {}",
        stdout(&out)
    );

    let out = run(sqz().args([
        "--lang", "en-US", "--style", "classic", "--color", "always", "info",
    ]));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(
        !stdout(&out).contains("\x1b["),
        "classic output stays conservative even when color is forced: {}",
        stdout(&out)
    );

    let out = run(sqz().args(["--style", "modern", "--color", "always", "info", "--json"]));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(
        !stdout(&out).contains("\x1b["),
        "JSON stdout must never contain ANSI: {}",
        stdout(&out)
    );
    assert!(stdout_json(&out).is_array());

    let out = run(sqz().args([
        "--lang", "en-US", "--style", "modern", "--color", "always", "--accent", "amber", "info",
    ]));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(
        stdout(&out).contains("\x1b[1;38;5;214m"),
        "--accent amber should use amber as the modern primary color: {}",
        stdout(&out)
    );

    let out = run(sqz().args([
        "--lang",
        "en-US",
        "--style",
        "modern",
        "--color",
        "always",
        "--palette",
        "icon",
        "info",
    ]));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let text = stdout(&out);
    assert!(
        text.contains("\x1b[1;38;2;45;212;191m") && text.contains("\x1b[38;2;14;165;233m"),
        "--palette icon should explicitly use the approved app icon teal-to-sky accents: {text}"
    );

    let out = run(sqz().args([
        "--lang",
        "en-US",
        "--style",
        "modern",
        "--color",
        "always",
        "--palette",
        "squallz",
        "info",
    ]));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(
        stdout(&out).contains("\x1b[1;38;2;45;212;191m"),
        "--palette squallz should use the exact app icon teal primary color: {}",
        stdout(&out)
    );

    let out = run(sqz().args([
        "--lang",
        "en-US",
        "--style",
        "modern",
        "--color",
        "always",
        "--palette",
        "brand",
        "info",
    ]));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let text = stdout(&out);
    assert!(
        text.contains("\x1b[1;38;2;45;212;191m") && text.contains("\x1b[38;2;14;165;233m"),
        "--palette brand should use the approved app icon teal-to-sky accents: {text}"
    );

    let out = run(sqz().args([
        "--lang",
        "en-US",
        "--style",
        "modern",
        "--color",
        "always",
        "--palette",
        "cascade",
        "info",
    ]));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let text = stdout(&out);
    assert!(
        text.contains("\x1b[1;38;2;45;212;191m") && text.contains("\x1b[38;2;125;211;252m"),
        "--palette cascade should keep the approved teal primary with brighter sky secondary accents: {text}"
    );

    let out = run(sqz().args([
        "--lang",
        "en-US",
        "--style",
        "modern",
        "--color",
        "always",
        "--palette",
        "daylight",
        "info",
    ]));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let text = stdout(&out);
    assert!(
        text.contains("\x1b[1;38;2;45;212;191m") && text.contains("\x1b[38;2;103;232;249m"),
        "--palette daylight should use approved teal primary with a bright sky secondary accent: {text}"
    );

    let out = run(sqz().args([
        "--lang",
        "en-US",
        "--style",
        "modern",
        "--color",
        "always",
        "--palette",
        "skyline",
        "info",
    ]));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let text = stdout(&out);
    assert!(
        text.contains("\x1b[1;38;2;14;165;233m") && text.contains("\x1b[38;2;45;212;191m"),
        "--palette skyline should invert the approved app icon colors for a brighter blue-led terminal look: {text}"
    );

    let out = run(sqz().args([
        "--lang",
        "en-US",
        "--style",
        "modern",
        "--color",
        "always",
        "--palette",
        "aero",
        "info",
    ]));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let text = stdout(&out);
    assert!(
        text.contains("\x1b[1;38;2;125;211;252m") && text.contains("\x1b[38;2;45;212;191m"),
        "--palette aero should use light sky primary and Squallz teal secondary accents: {text}"
    );

    let out = run(sqz().args([
        "--lang",
        "en-US",
        "--style",
        "modern",
        "--color",
        "always",
        "--palette",
        "crest",
        "info",
    ]));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let text = stdout(&out);
    assert!(
        text.contains("\x1b[1;38;2;56;189;248m") && text.contains("\x1b[38;2;94;234;212m"),
        "--palette crest should use bright sky primary and luminous aqua secondary accents: {text}"
    );

    let out = run(sqz().args([
        "--lang",
        "en-US",
        "--style",
        "modern",
        "--color",
        "always",
        "--palette",
        "halo",
        "info",
    ]));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let text = stdout(&out);
    assert!(
        text.contains("\x1b[1;38;2;94;234;212m") && text.contains("\x1b[38;2;56;189;248m"),
        "--palette halo should use luminous teal primary and bright sky secondary accents: {text}"
    );

    let out = run(sqz().args([
        "--lang",
        "en-US",
        "--style",
        "modern",
        "--color",
        "always",
        "--palette",
        "tropic",
        "info",
    ]));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let text = stdout(&out);
    assert!(
        text.contains("\x1b[1;38;2;45;212;191m") && text.contains("\x1b[38;2;34;211;238m"),
        "--palette tropic should use the approved teal primary and electric cyan secondary accents: {text}"
    );

    let out = run(sqz().args([
        "--lang",
        "en-US",
        "--style",
        "modern",
        "--color",
        "always",
        "--palette",
        "kinetic",
        "info",
    ]));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let text = stdout(&out);
    assert!(
        text.contains("\x1b[1;38;2;45;212;191m") && text.contains("\x1b[38;2;96;165;250m"),
        "--palette kinetic should use the approved teal primary and high-energy sky secondary accents: {text}"
    );

    let out = run(sqz().args([
        "--lang",
        "en-US",
        "--style",
        "modern",
        "--color",
        "always",
        "--palette",
        "radiant",
        "info",
    ]));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let text = stdout(&out);
    assert!(
        text.contains("\x1b[1;38;2;45;212;191m")
            && text.contains("\x1b[38;2;186;230;253m")
            && text.contains("--palette radiant"),
        "--palette radiant should use approved teal primary and bright sky-glass secondary accents: {text}"
    );

    let out = run(sqz().args([
        "--lang",
        "en-US",
        "--style",
        "modern",
        "--color",
        "always",
        "--palette",
        "surge",
        "info",
    ]));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let text = stdout(&out);
    assert!(
        text.contains("\x1b[1;38;2;45;212;191m")
            && text.contains("\x1b[38;2;56;189;248m")
            && text.contains("--palette surge"),
        "--palette surge should keep the approved teal primary with vivid sky-blue secondary accents: {text}"
    );

    let out = run(sqz().args([
        "--lang", "en-US", "--style", "modern", "--color", "always", "--colors", "glass", "info",
    ]));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let text = stdout(&out);
    assert!(
        text.contains("\x1b[1;38;2;103;232;249m") && text.contains("\x1b[38;2;45;212;191m"),
        "--colors glass should use bright cyan primary with Squallz teal secondary accents: {text}"
    );

    let out = run(sqz().args([
        "--lang", "en-US", "--style", "modern", "--color", "always", "--colors", "icon", "info",
    ]));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let text = stdout(&out);
    assert!(
        text.contains("\x1b[1;38;2;45;212;191m") && text.contains("\x1b[38;2;14;165;233m"),
        "--colors icon should behave as the explicit app icon palette alias: {text}"
    );

    let out = run(sqz().args([
        "--lang",
        "en-US",
        "--style",
        "modern",
        "--color",
        "always",
        "--palette",
        "nova",
        "info",
    ]));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let text = stdout(&out);
    assert!(
        text.contains("\x1b[1;38;2;34;211;238m") && text.contains("\x1b[38;2;250;204;21m"),
        "--palette nova should use bright cyan primary and sunlit gold secondary accents: {text}"
    );

    let out = run(sqz().args([
        "--lang",
        "en-US",
        "--style",
        "modern",
        "--color",
        "always",
        "--palette",
        "crystal",
        "info",
    ]));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let text = stdout(&out);
    assert!(
        text.contains("\x1b[1;38;2;94;234;212m") && text.contains("\x1b[38;2;125;211;252m"),
        "--palette crystal should use luminous aqua and clear sky truecolor accents: {text}"
    );

    let out = run(sqz().args([
        "--lang",
        "en-US",
        "--style",
        "modern",
        "--color",
        "always",
        "--palette",
        "lumina",
        "info",
    ]));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let text = stdout(&out);
    assert!(
        text.contains("\x1b[1;38;2;103;232;249m") && text.contains("\x1b[38;2;251;113;133m"),
        "--palette lumina should use bright cyan primary and coral secondary accents: {text}"
    );

    let out = run(sqz().args([
        "--lang",
        "en-US",
        "--style",
        "modern",
        "--color",
        "always",
        "--palette",
        "azure",
        "info",
    ]));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let text = stdout(&out);
    assert!(
        text.contains("\x1b[1;38;2;56;189;248m") && text.contains("\x1b[38;2;45;212;191m"),
        "--palette azure should use bright sky primary and Squallz teal secondary accents: {text}"
    );

    let out = run(sqz().args([
        "--lang",
        "en-US",
        "--style",
        "modern",
        "--color",
        "always",
        "--palette",
        "surf",
        "info",
    ]));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let text = stdout(&out);
    assert!(
        text.contains("\x1b[1;38;2;34;211;238m") && text.contains("\x1b[38;2;14;165;233m"),
        "--palette surf should use electric cyan primary and sky secondary accents: {text}"
    );

    let out = run(sqz().args([
        "--lang",
        "en-US",
        "--style",
        "modern",
        "--color",
        "always",
        "--palette",
        "signal",
        "info",
    ]));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let text = stdout(&out);
    assert!(
        text.contains("\x1b[1;38;2;94;234;212m") && text.contains("\x1b[38;2;56;189;248m"),
        "--palette signal should use bright teal primary and sky secondary accents: {text}"
    );

    let out = run(sqz().args([
        "--lang",
        "en-US",
        "--style",
        "modern",
        "--color",
        "always",
        "--palette",
        "tide",
        "info",
    ]));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let text = stdout(&out);
    assert!(
        text.contains("\x1b[1;38;2;103;232;249m") && text.contains("\x1b[38;2;56;189;248m"),
        "--palette tide should use light cyan primary and sky secondary accents: {text}"
    );

    let out = run(sqz().args([
        "--lang",
        "en-US",
        "--style",
        "modern",
        "--color",
        "always",
        "--palette",
        "neon",
        "info",
    ]));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let text = stdout(&out);
    assert!(
        text.contains("\x1b[1;38;2;34;211;238m") && text.contains("\x1b[38;2;244;114;182m"),
        "--palette neon should use cyan primary and pink secondary truecolor accents: {text}"
    );

    let out = run(sqz().args([
        "--lang",
        "en-US",
        "--style",
        "modern",
        "--color",
        "always",
        "--palette",
        "electric",
        "info",
    ]));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let text = stdout(&out);
    assert!(
        text.contains("\x1b[1;38;2;34;211;238m") && text.contains("\x1b[38;2;167;139;250m"),
        "--palette electric should use cyan primary and violet secondary truecolor accents: {text}"
    );

    let out = run(sqz().args([
        "--lang",
        "en-US",
        "--style",
        "modern",
        "--color",
        "always",
        "--palette",
        "ocean",
        "info",
    ]));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let text = stdout(&out);
    assert!(
        text.contains("\x1b[1;38;2;14;165;233m") && text.contains("\x1b[38;2;45;212;191m"),
        "--palette ocean should use sky primary and teal secondary truecolor accents: {text}"
    );

    let out = run(sqz().args([
        "--lang",
        "en-US",
        "--style",
        "modern",
        "--color",
        "always",
        "--palette",
        "jade",
        "info",
    ]));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let text = stdout(&out);
    assert!(
        text.contains("\x1b[1;38;2;52;211;153m") && text.contains("\x1b[38;2;45;212;191m"),
        "--palette jade should use green-cyan primary and Squallz teal secondary accents: {text}"
    );

    let out = run(sqz().args([
        "--lang",
        "en-US",
        "--style",
        "modern",
        "--color",
        "always",
        "--palette",
        "rose",
        "info",
    ]));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(
        stdout(&out).contains("\x1b[1;38;5;205m"),
        "--palette rose should use the rose primary color: {}",
        stdout(&out)
    );

    let out = run(sqz().args([
        "--lang",
        "en-US",
        "--style",
        "modern",
        "--color",
        "always",
        "--palette",
        "aqua",
        "info",
    ]));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(
        stdout(&out).contains("\x1b[1;38;5;51m"),
        "--palette aqua should use the bright aqua primary color: {}",
        stdout(&out)
    );

    let out = run(sqz().args([
        "--lang",
        "en-US",
        "--style",
        "modern",
        "--color",
        "always",
        "--palette",
        "glacier",
        "info",
    ]));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(
        stdout(&out).contains("\x1b[1;38;5;87m"),
        "--palette glacier should use a bright cyan/sky primary color: {}",
        stdout(&out)
    );

    let out = run(sqz().args([
        "--lang",
        "en-US",
        "--style",
        "modern",
        "--color",
        "always",
        "--palette",
        "aurora",
        "info",
    ]));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(
        stdout(&out).contains("\x1b[1;38;5;86m"),
        "--palette aurora should use a mint/cyan primary color: {}",
        stdout(&out)
    );

    let out = run(sqz().args([
        "--lang",
        "en-US",
        "--style",
        "modern",
        "--color",
        "always",
        "--palette",
        "prism",
        "info",
    ]));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let text = stdout(&out);
    assert!(
        text.contains("\x1b[1;38;5;51m") && text.contains("\x1b[38;5;213m"),
        "--palette prism should use cyan primary and magenta secondary accents: {text}"
    );

    let out = run(sqz().args([
        "--lang",
        "en-US",
        "--style",
        "modern",
        "--color",
        "always",
        "--palette",
        "lagoon",
        "info",
    ]));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let text = stdout(&out);
    assert!(
        text.contains("\x1b[1;38;2;45;212;191m") && text.contains("\x1b[38;2;56;189;248m"),
        "--palette lagoon should use vivid teal and sky truecolor accents: {text}"
    );

    let out = run(sqz().args([
        "--lang",
        "en-US",
        "--style",
        "modern",
        "--color",
        "always",
        "--palette",
        "mint",
        "info",
    ]));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let text = stdout(&out);
    assert!(
        text.contains("\x1b[1;38;2;45;212;191m") && text.contains("\x1b[38;2;125;211;252m"),
        "--palette mint should keep the Squallz teal base with a softer sky accent: {text}"
    );

    let out = run(sqz().args([
        "--lang",
        "en-US",
        "--style",
        "modern",
        "--color",
        "always",
        "--palette",
        "sunset",
        "info",
    ]));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let text = stdout(&out);
    assert!(
        text.contains("\x1b[1;38;2;251;146;60m") && text.contains("\x1b[38;2;244;114;182m"),
        "--palette sunset should use warm orange and rose truecolor accents: {text}"
    );

    let out = run(sqz().args([
        "--lang",
        "en-US",
        "--style",
        "modern",
        "--color",
        "always",
        "--palette",
        "citrus",
        "info",
    ]));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let text = stdout(&out);
    assert!(
        text.contains("\x1b[1;38;2;163;230;53m") && text.contains("\x1b[38;2;34;211;238m"),
        "--palette citrus should use fresh lime and cyan truecolor accents: {text}"
    );

    let out = run(sqz().args([
        "--lang",
        "en-US",
        "--style",
        "modern",
        "--color",
        "always",
        "--palette",
        "breeze",
        "info",
    ]));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let text = stdout(&out);
    assert!(
        text.contains("\x1b[1;38;2;20;184;166m") && text.contains("\x1b[38;2;56;189;248m"),
        "--palette breeze should use teal primary and sky secondary truecolor accents: {text}"
    );

    let out = run(sqz().args([
        "--lang",
        "en-US",
        "--style",
        "modern",
        "--color",
        "always",
        "--color-scheme",
        "breeze",
        "info",
    ]));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let text = stdout(&out);
    assert!(
        text.contains("\x1b[1;38;2;20;184;166m") && text.contains("\x1b[38;2;56;189;248m"),
        "--color-scheme should behave as a visible alias for modern palette selection: {text}"
    );

    let out = run(sqz().args([
        "--lang", "en-US", "--style", "modern", "--color", "always", "--scheme", "breeze", "info",
    ]));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let text = stdout(&out);
    assert!(
        text.contains("\x1b[1;38;2;20;184;166m") && text.contains("\x1b[38;2;56;189;248m"),
        "--scheme should behave as a visible alias for modern palette selection: {text}"
    );

    let out = run(sqz().args([
        "--lang", "en-US", "--style", "modern", "--color", "always", "--theme", "ocean", "info",
    ]));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let text = stdout(&out);
    assert!(
        text.contains("\x1b[1;38;2;14;165;233m") && text.contains("\x1b[38;2;45;212;191m"),
        "--theme should behave as a visible alias for modern palette selection: {text}"
    );

    let out = run(sqz().args([
        "--lang",
        "en-US",
        "--style",
        "modern",
        "--color",
        "always",
        "--palette",
        "vapor",
        "info",
    ]));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let text = stdout(&out);
    assert!(
        text.contains("\x1b[1;38;2;125;211;252m") && text.contains("\x1b[38;2;192;132;252m"),
        "--palette vapor should use luminous sky primary and soft violet secondary accents: {text}"
    );
}

#[test]
fn cli_surface_contract_format_errors_use_json_envelope() {
    let dir = temp_dir("cli-surface-json-errors");
    let root = sample_tree(&dir);

    let created = dir.join("created.rar");
    let out = run(sqz()
        .args(["--lang", "en-US", "compress"])
        .arg(&root)
        .arg("-o")
        .arg(&created)
        .arg("--json"));
    assert_json_error(
        &out,
        2,
        "unsupported",
        "format rar does not support creation",
    );
    assert!(
        !created.exists(),
        "unsupported create must not leave output"
    );

    let missing = dir.join("missing.zip");
    let out = run(sqz()
        .args(["--lang", "en-US", "list"])
        .arg(&missing)
        .arg("--json"));
    assert_json_error(&out, 7, "io", "No such file");

    std::fs::remove_dir_all(&dir).unwrap();
}

/// Creates a unique scratch directory for one test.
fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("sqz-it-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Builds a small source tree: a.txt, sub/b.txt, .git/config, junk.tmp.
fn sample_tree(dir: &Path) -> PathBuf {
    let root = dir.join("project");
    std::fs::create_dir_all(root.join("sub")).unwrap();
    std::fs::create_dir_all(root.join(".git")).unwrap();
    std::fs::write(root.join("a.txt"), b"hello world").unwrap();
    std::fs::write(root.join("sub/b.txt"), b"nested content").unwrap();
    std::fs::write(root.join(".git/config"), b"[core]").unwrap();
    std::fs::write(root.join("junk.tmp"), b"scratch").unwrap();
    root
}

#[test]
fn batch_json_script_runs_core_archive_jobs() {
    let dir = temp_dir("batch-json");
    let root = sample_tree(&dir);
    let archive = dir.join("source.zip");
    let converted = dir.join("converted.7z");
    let extracted = dir.join("out");
    let script = dir.join("batch.json");
    run(sqz().arg("compress").arg(&root).arg("-o").arg(&archive));
    std::fs::create_dir_all(dir.join("dups")).unwrap();
    std::fs::write(dir.join("dups/one.bin"), b"same payload").unwrap();
    std::fs::write(dir.join("dups/two.bin"), b"same payload").unwrap();
    std::fs::write(
        dir.join("SHA256SUMS"),
        "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9  project/a.txt\n",
    )
    .unwrap();

    let manifest = serde_json::json!({
        "version": 1,
        "jobs": [
            { "kind": "estimate", "inputs": ["project"], "output": "planned.zip" },
            { "kind": "test", "archive": "source.zip" },
            { "kind": "extract", "archive": "source.zip", "dest": "out", "includes": ["project/a.txt"], "overwrite": "all" },
            { "kind": "convert", "src": "source.zip", "output": "converted.7z", "profile": "fast" },
            { "kind": "checksum", "inputs": ["project/a.txt"], "algorithm": "sha256" },
            { "kind": "checksum_check", "check": "SHA256SUMS", "algorithm": "sha256" },
            { "kind": "duplicates", "inputs": ["dups"], "min_size": 1, "fail_on_found": false }
        ]
    });
    std::fs::write(&script, serde_json::to_string_pretty(&manifest).unwrap()).unwrap();

    let out = run(sqz().arg("batch").arg(&script).arg("--json"));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let report = stdout_json(&out);
    assert_eq!(report["ok"], true);
    assert_eq!(report["operation"], "batch");
    assert_eq!(report["total"], 7);
    assert_eq!(report["failed"], 0);
    assert_eq!(report["jobs"][0]["operation"], "estimate");
    assert_eq!(report["jobs"][1]["operation"], "test");
    assert_eq!(report["jobs"][2]["operation"], "extract");
    assert_eq!(report["jobs"][3]["operation"], "convert");
    assert_eq!(report["jobs"][4]["operation"], "checksum");
    assert_eq!(report["jobs"][4]["result"]["algorithm"], "sha256");
    assert_eq!(report["jobs"][4]["result"]["files_hashed"], 1);
    assert_eq!(report["jobs"][5]["operation"], "checksum_check");
    assert_eq!(report["jobs"][5]["result"]["passed"], 1);
    assert_eq!(report["jobs"][6]["operation"], "duplicates");
    assert_eq!(report["jobs"][6]["result"]["duplicate_groups"], 1);
    assert_eq!(report["jobs"][6]["result"]["duplicate_files"], 2);
    assert!(
        report["jobs"][3]["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("converted")),
        "stdout: {}",
        stdout(&out)
    );
    assert!(converted.is_file());
    assert_eq!(
        std::fs::read_to_string(extracted.join("project/a.txt")).unwrap(),
        "hello world"
    );

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn batch_modern_human_output_uses_job_table() {
    let dir = temp_dir("batch-modern-human");
    let _root = sample_tree(&dir);
    let script = dir.join("batch.json");
    let manifest = serde_json::json!({
        "version": 1,
        "jobs": [
            { "id": "plan", "kind": "estimate", "inputs": ["project"], "output": "planned.zip" },
            { "id": "missing-test", "kind": "test", "archive": "missing.zip" }
        ]
    });
    std::fs::write(&script, serde_json::to_string_pretty(&manifest).unwrap()).unwrap();

    let out = run(sqz()
        .args(["--lang", "en-US", "--style", "modern", "--color", "never"])
        .arg("batch")
        .arg(&script)
        .arg("--keep-going"));
    assert_eq!(out.status.code(), Some(7), "stderr: {}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("Batch result"), "stdout: {text}");
    assert!(text.contains("Batch jobs"), "stdout: {text}");
    assert!(text.contains("Succeeded"), "stdout: {text}");
    assert!(text.contains("plan"), "stdout: {text}");
    assert!(text.contains("estimate"), "stdout: {text}");
    assert!(text.contains("missing-test"), "stdout: {text}");
    assert!(text.contains("failed"), "stdout: {text}");
    assert!(text.contains("I/O error"), "stdout: {text}");
    assert!(text.contains("┬"), "stdout: {text}");
    assert!(text.contains("┼"), "stdout: {text}");

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn batch_json_script_runs_workbench_archive_jobs() {
    let dir = temp_dir("batch-workbench");
    let root = sample_tree(&dir);
    let archive = dir.join("source.zip");
    let sqz_container = dir.join("container.sqz");
    let exported = dir.join("exported.zip");
    let rebuilt = dir.join("rebuilt.zip");
    let repaired_sqz = dir.join("repaired.sqz");
    let script = dir.join("batch.json");
    std::fs::write(dir.join("extra.txt"), b"extra payload").unwrap();

    let zip_out = run(sqz().arg("compress").arg(&root).arg("-o").arg(&archive));
    assert!(zip_out.status.success(), "stderr: {}", stderr(&zip_out));
    let sqz_out = run(sqz()
        .arg("pack")
        .arg(&root)
        .arg("-o")
        .arg(&sqz_container)
        .arg("--inner-format")
        .arg("zip"));
    assert!(sqz_out.status.success(), "stderr: {}", stderr(&sqz_out));

    let manifest = serde_json::json!({
        "version": 1,
        "jobs": [
            {
                "kind": "update",
                "archive": "source.zip",
                "add": ["extra.txt"],
                "mkdir": ["empty/"],
                "rename": [{ "from": "project/sub/b.txt", "to": "project/sub/renamed.txt" }],
                "profile": "fast"
            },
            { "kind": "export_sqz", "archive": "container.sqz", "output": "exported.zip" },
            { "kind": "repair_zip", "archive": "source.zip", "output": "rebuilt.zip" },
            { "kind": "repair_sqz", "archive": "container.sqz", "output": "repaired.sqz" }
        ]
    });
    std::fs::write(&script, serde_json::to_string_pretty(&manifest).unwrap()).unwrap();

    let out = run(sqz().arg("batch").arg(&script).arg("--json"));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let report = stdout_json(&out);
    assert_eq!(report["ok"], true);
    assert_eq!(report["total"], 4);
    assert_eq!(report["failed"], 0);
    assert_eq!(
        report["jobs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|job| job["operation"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["update", "export", "repair_zip", "repair_sqz"]
    );
    assert_eq!(report["jobs"][0]["result"]["operations"], 3);
    assert!(exported.is_file());
    assert!(rebuilt.is_file());
    assert!(repaired_sqz.is_file());

    let updated_list = stdout_json(&run(sqz().arg("list").arg(&archive).arg("--json")));
    let updated_paths = updated_list
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|entry| entry["path"].as_str())
        .collect::<Vec<_>>();
    assert!(
        updated_paths.contains(&"extra.txt"),
        "paths: {updated_paths:?}"
    );
    assert!(
        updated_paths.contains(&"project/sub/renamed.txt"),
        "paths: {updated_paths:?}"
    );
    assert!(!updated_paths.contains(&"project/sub/b.txt"));

    let exported_test = stdout_json(&run(sqz().arg("test").arg(&exported).arg("--json")));
    assert_eq!(exported_test["ok"], true);
    let rebuilt_test = stdout_json(&run(sqz().arg("test").arg(&rebuilt).arg("--json")));
    assert_eq!(rebuilt_test["ok"], true);
    let repaired_test = stdout_json(&run(sqz().arg("test").arg(&repaired_sqz).arg("--json")));
    assert_eq!(repaired_test["ok"], true);

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn batch_keep_going_reports_failures_without_stopping() {
    let dir = temp_dir("batch-keep-going");
    let root = sample_tree(&dir);
    let archive = dir.join("source.zip");
    let extracted = dir.join("out");
    let script = dir.join("batch.json");
    run(sqz().arg("compress").arg(&root).arg("-o").arg(&archive));
    std::fs::write(
        dir.join("SHA256SUMS.bad"),
        "0000000000000000000000000000000000000000000000000000000000000000  project/a.txt\n",
    )
    .unwrap();

    let manifest = serde_json::json!({
        "version": 1,
        "jobs": [
            { "kind": "checksum_check", "check": "SHA256SUMS.bad", "algorithm": "sha256" },
            { "kind": "test", "archive": "missing.zip" },
            { "kind": "extract", "archive": "source.zip", "dest": "out", "includes": ["project/sub/b.txt"], "overwrite": "all" }
        ]
    });
    std::fs::write(&script, serde_json::to_string_pretty(&manifest).unwrap()).unwrap();

    let out = run(sqz()
        .arg("batch")
        .arg(&script)
        .arg("--keep-going")
        .arg("--json"));
    assert_eq!(out.status.code(), Some(3), "stdout: {}", stdout(&out));
    assert!(
        stderr(&out).trim().is_empty(),
        "JSON batch failures must not emit human stderr: {}",
        stderr(&out)
    );
    let report = stdout_json(&out);
    assert_eq!(report["ok"], false);
    assert_eq!(report["total"], 3);
    assert_eq!(report["failed"], 2);
    assert_eq!(report["jobs"][0]["ok"], false);
    assert_eq!(report["jobs"][0]["operation"], "checksum_check");
    assert_eq!(report["jobs"][0]["error_kind"], "corrupt_archive");
    assert_eq!(report["jobs"][1]["ok"], false);
    assert_eq!(report["jobs"][1]["error_kind"], "io");
    assert_eq!(report["jobs"][2]["ok"], true);
    assert_eq!(
        std::fs::read_to_string(extracted.join("project/sub/b.txt")).unwrap(),
        "nested content"
    );

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn batch_extract_honors_shared_safety_limits() {
    let dir = temp_dir("batch-safety-limits");
    let root = sample_tree(&dir);
    let archive = dir.join("source.zip");
    let script = dir.join("batch.json");
    let created = run(sqz().arg("compress").arg(&root).arg("-o").arg(&archive));
    assert!(created.status.success(), "stderr: {}", stderr(&created));

    let manifest = serde_json::json!({
        "version": 1,
        "jobs": [
            {
                "kind": "extract",
                "archive": "source.zip",
                "dest": "limited-out",
                "overwrite": "all",
                "max_output_bytes": 1
            }
        ]
    });
    std::fs::write(&script, serde_json::to_string_pretty(&manifest).unwrap()).unwrap();

    let out = run(sqz().arg("batch").arg(&script).arg("--json"));
    assert_eq!(out.status.code(), Some(6), "stdout: {}", stdout(&out));
    assert!(
        stderr(&out).trim().is_empty(),
        "JSON batch failures must not emit human stderr: {}",
        stderr(&out)
    );
    let report = stdout_json(&out);
    assert_eq!(report["ok"], false);
    assert_eq!(report["total"], 1);
    assert_eq!(report["failed"], 1);
    assert_eq!(report["jobs"][0]["operation"], "extract");
    assert_eq!(report["jobs"][0]["error_kind"], "resource_limit_exceeded");
    assert_eq!(report["jobs"][0]["exit_code"], 6);

    std::fs::remove_dir_all(&dir).unwrap();
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

fn corrupt_sqz_file_header_crc(path: &Path) {
    let mut bytes = std::fs::read(path).unwrap();
    assert!(bytes.len() > 64);
    assert_eq!(&bytes[0..8], b"SQZARCH\x1A");
    bytes[16] ^= 0x55;
    std::fs::write(path, bytes).unwrap();
}

fn corrupt_sqz_file_header_uuid_with_valid_crc(path: &Path) {
    let mut bytes = std::fs::read(path).unwrap();
    assert!(bytes.len() > 64);
    assert_eq!(&bytes[0..8], b"SQZARCH\x1A");
    bytes[16] ^= 0x55;
    let crc = crc32c(&bytes[..52]);
    bytes[52..56].copy_from_slice(&crc.to_le_bytes());
    std::fs::write(path, bytes).unwrap();
}

fn corrupt_sqz_footer_index_length_with_valid_crc(path: &Path) {
    let mut bytes = std::fs::read(path).unwrap();
    assert!(bytes.len() > 64);
    let footer_start = bytes.len() - 64;
    assert_eq!(
        &bytes[footer_start + 56..footer_start + 64],
        b"\x1ASQZEND\n"
    );
    bytes[footer_start + 8..footer_start + 16].copy_from_slice(&u64::MAX.to_le_bytes());
    let crc = crc32c(&bytes[footer_start..footer_start + 48]);
    bytes[footer_start + 48..footer_start + 52].copy_from_slice(&crc.to_le_bytes());
    std::fs::write(path, bytes).unwrap();
}

fn corrupt_sqz_footer_magic(path: &Path) {
    let mut bytes = std::fs::read(path).unwrap();
    assert!(bytes.len() > 64);
    let footer_start = bytes.len() - 64;
    assert_eq!(
        &bytes[footer_start + 56..footer_start + 64],
        b"\x1ASQZEND\n"
    );
    bytes[footer_start + 63] ^= 0x5A;
    std::fs::write(path, bytes).unwrap();
}

fn corrupt_sqz_footer_crc_field(path: &Path) {
    let mut bytes = std::fs::read(path).unwrap();
    assert!(bytes.len() > 64);
    let footer_start = bytes.len() - 64;
    assert_eq!(
        &bytes[footer_start + 56..footer_start + 64],
        b"\x1ASQZEND\n"
    );
    bytes[footer_start] ^= 0x5A;
    std::fs::write(path, bytes).unwrap();
}

fn corrupt_sqz_recovery_protection_trailer(path: &Path) {
    let mut bytes = std::fs::read(path).unwrap();
    let trailer_pos = bytes
        .windows(b"RSPC".len())
        .rposition(|window| window == b"RSPC")
        .expect("recovery protection trailer found");
    bytes[trailer_pos + 44] ^= 0x55;
    std::fs::write(path, bytes).unwrap();
}

fn corrupt_sqz_recovery_primary_block(path: &Path) {
    let mut bytes = std::fs::read(path).unwrap();
    let recovery_pos = bytes
        .windows(b"RSEC".len())
        .position(|window| window == b"RSEC")
        .expect("primary recovery section found");
    bytes[recovery_pos] ^= 0x7F;
    std::fs::write(path, bytes).unwrap();
}

fn sqz_recovery_marker(block: usize) -> Vec<u8> {
    format!("SQZ-CLI-RECOVERY-BLOCK-{block:02}-unique-marker").into_bytes()
}

fn sqz_recovery_payload(blocks: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(blocks * SQZ_RECOVERY_BLOCK);
    for block_index in 0..blocks {
        let marker = sqz_recovery_marker(block_index);
        let mut block = vec![0u8; SQZ_RECOVERY_BLOCK];
        block[..marker.len()].copy_from_slice(&marker);
        for (offset, byte) in block.iter_mut().enumerate().skip(marker.len()) {
            *byte = ((block_index * 29 + offset * 19) % 251) as u8;
        }
        out.extend_from_slice(&block);
    }
    out
}

fn corrupt_sqz_marked_payload_blocks(path: &Path, blocks: &[usize]) {
    let mut bytes = std::fs::read(path).unwrap();
    for block in blocks {
        let marker = sqz_recovery_marker(*block);
        let pos = bytes
            .windows(marker.len())
            .position(|window| window == marker)
            .unwrap_or_else(|| panic!("payload marker not found for block {block}"));
        bytes[pos + marker.len() - 1] ^= 0x5A;
    }
    std::fs::write(path, bytes).unwrap();
}

fn corrupt_stored_zip_payload(path: &Path, needle: &[u8]) {
    let mut bytes = std::fs::read(path).unwrap();
    let pos = bytes
        .windows(needle.len())
        .position(|window| window == needle)
        .expect("stored zip payload bytes should be visible");
    bytes[pos] ^= 0xA5;
    std::fs::write(path, bytes).unwrap();
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

fn crc32c(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0x82F6_3B78 & mask);
        }
    }
    !crc
}

fn stored_zip_with_missing_central_directory(entries: &[(&[u8], &[u8])]) -> Vec<u8> {
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
    // The central directory is deliberately not appended. This sample proves
    // the CLI reaches the format-layer local-header fallback.
    out
}

fn stored_encrypted_flag_zip_without_central_directory(name: &[u8], data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let crc = crc32(data);
    let size = data.len() as u32;
    let name_len = name.len() as u16;

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
    out.extend_from_slice(name);
    out.extend_from_slice(data);
    out
}

fn stored_unsupported_method_zip_without_central_directory(
    name: &[u8],
    data: &[u8],
    method: u16,
) -> Vec<u8> {
    let mut out = Vec::new();
    let crc = crc32(data);
    let size = data.len() as u32;
    let name_len = name.len() as u16;

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
    out.extend_from_slice(name);
    out.extend_from_slice(data);
    out
}

fn stored_zip64_local_header_without_central_directory(name: &[u8], data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let crc = crc32(data);
    let size = data.len() as u64;
    let name_len = name.len() as u16;
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
    out.extend_from_slice(name);
    out.extend_from_slice(&0x0001u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(&size.to_le_bytes());
    out.extend_from_slice(&size.to_le_bytes());
    out.extend_from_slice(data);
    out
}

fn stored_zip64_data_descriptor_without_central_directory(name: &[u8], data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let crc = crc32(data);
    let size = data.len() as u64;
    let name_len = name.len() as u16;

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
    out.extend_from_slice(name);
    out.extend_from_slice(data);
    out.extend_from_slice(&[0x50, 0x4B, 0x07, 0x08]);
    out.extend_from_slice(&crc.to_le_bytes());
    out.extend_from_slice(&size.to_le_bytes());
    out.extend_from_slice(&size.to_le_bytes());
    out
}

#[test]
fn compress_list_test_extract_roundtrip_with_json() {
    let dir = temp_dir("roundtrip");
    let root = sample_tree(&dir);
    let archive = dir.join("out.zip");

    // compress
    let out = run(sqz()
        .args(["--lang", "en-US", "compress"])
        .arg(&root)
        .arg("-o")
        .arg(&archive)
        .args(["--format", "zip"])
        .arg("--json"));
    assert!(out.status.success(), "compress failed: {}", stderr(&out));
    let report: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    assert_eq!(report["ok"], true);
    assert_eq!(report["operation"], "compress");
    assert_eq!(report["output"], archive.display().to_string());
    assert_eq!(report["split"], false);
    assert_eq!(report["volumes"], 1);

    // list --json: parseable array with complete fields
    let out = run(sqz().arg("list").arg(&archive).arg("--json"));
    assert!(out.status.success());
    let entries: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    let entries = entries.as_array().expect("top-level array");
    assert!(!entries.is_empty());
    let file = entries
        .iter()
        .find(|e| e["path"] == "project/a.txt")
        .expect("a.txt listed");
    assert_eq!(file["type"], "file");
    assert_eq!(file["size"], 11);
    assert!(file["compressed_size"].is_u64());
    assert!(file["modified"].is_u64());
    assert!(file["crc32"].is_u64());
    assert_eq!(file["encrypted"], false);
    assert_eq!(file["encoding"], "utf-8");

    // list --tree: human-readable hierarchy without changing JSON contracts
    let out = run(sqz()
        .args(["--lang", "en-US", "list"])
        .arg(&archive)
        .arg("--tree"));
    assert!(out.status.success(), "list --tree failed: {}", stderr(&out));
    let tree = stdout(&out);
    assert!(tree.lines().next().is_some_and(|line| line == "."));
    assert!(tree.contains("project/"), "tree: {tree}");
    assert!(tree.contains("a.txt"), "tree: {tree}");
    assert!(tree.contains("sub/"), "tree: {tree}");
    assert!(tree.contains("b.txt"), "tree: {tree}");

    // test --json
    let out = run(sqz().arg("test").arg(&archive).arg("--json"));
    assert!(out.status.success());
    let report: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    assert_eq!(report["ok"], true);
    assert!(report["entries_tested"].as_u64().unwrap() >= 6);
    assert!(report["problems"].as_array().unwrap().is_empty());

    // extract and compare contents
    let dest = dir.join("extracted");
    let out = run(sqz()
        .args(["--lang", "en-US", "extract"])
        .arg(&archive)
        .arg("-d")
        .arg(&dest)
        .arg("--json"));
    assert!(out.status.success(), "extract failed: {}", stderr(&out));
    let report: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    assert_eq!(report["ok"], true);
    assert_eq!(report["operation"], "extract");
    assert_eq!(report["dest"], dest.display().to_string());
    assert_eq!(report["matched"], true);
    assert_eq!(report["best_effort"], false);
    assert_eq!(report["skipped"], 0);
    assert!(report["problems"].as_array().unwrap().is_empty());
    assert_eq!(
        std::fs::read(dest.join("project/a.txt")).unwrap(),
        b"hello world"
    );
    assert_eq!(
        std::fs::read(dest.join("project/sub/b.txt")).unwrap(),
        b"nested content"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn compress_format_must_match_output_extension() {
    let dir = temp_dir("compress-format");
    let root = sample_tree(&dir);
    let archive = dir.join("wrong.7z");

    let out = run(sqz()
        .arg("compress")
        .arg(&root)
        .arg("-o")
        .arg(&archive)
        .args(["--format", "zip"]));
    assert!(!out.status.success(), "format mismatch should fail");
    assert!(
        stderr(&out).contains("requested format 'zip' does not match output path"),
        "stderr: {}",
        stderr(&out)
    );
    assert!(!archive.exists(), "mismatched output should not be created");

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn nested_archive_list_and_extract_through_the_cli() {
    let dir = temp_dir("nested-cli");
    let inner_root = sample_tree(&dir);
    let inner = dir.join("inner.zip");
    let out = run(sqz().arg("compress").arg(&inner_root).arg("-o").arg(&inner));
    assert!(
        out.status.success(),
        "inner compress failed: {}",
        stderr(&out)
    );

    let outer_root = dir.join("outer");
    std::fs::create_dir_all(outer_root.join("bundles")).unwrap();
    std::fs::copy(&inner, outer_root.join("bundles/inner.zip")).unwrap();
    std::fs::write(outer_root.join("readme.txt"), b"outer").unwrap();
    let outer = dir.join("outer.zip");
    let out = run(sqz().arg("compress").arg(&outer_root).arg("-o").arg(&outer));
    assert!(
        out.status.success(),
        "outer compress failed: {}",
        stderr(&out)
    );

    let nested_entry = "outer/bundles/inner.zip";
    let out = run(sqz()
        .arg("nested")
        .arg("list")
        .arg(&outer)
        .arg(nested_entry)
        .arg("--json"));
    assert!(out.status.success(), "nested list failed: {}", stderr(&out));
    let entries: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    let paths: Vec<&str> = entries
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["path"].as_str().unwrap())
        .collect();
    assert!(paths.contains(&"project/sub/b.txt"));

    let out = run(sqz()
        .arg("nested")
        .arg("list")
        .arg(&outer)
        .arg(nested_entry)
        .arg("--tree"));
    assert!(
        out.status.success(),
        "nested list --tree failed: {}",
        stderr(&out)
    );
    assert!(
        stdout(&out).contains("project/"),
        "stdout: {}",
        stdout(&out)
    );
    assert!(stdout(&out).contains("b.txt"), "stdout: {}", stdout(&out));

    let out = run(sqz()
        .args(["--lang", "en-US", "--style", "modern", "--color", "never"])
        .arg("nested")
        .arg("list")
        .arg(&outer)
        .arg(nested_entry));
    assert!(
        out.status.success(),
        "nested modern list failed: {}",
        stderr(&out)
    );
    let text = stdout(&out);
    assert!(text.contains("Archive contents"), "stdout: {text}");
    assert!(text.contains("Archive summary"), "stdout: {text}");
    assert!(text.contains("Entry mix"), "stdout: {text}");
    assert!(text.contains("project/sub/b.txt"), "stdout: {text}");
    assert!(text.contains("┬"), "stdout: {text}");
    assert!(text.contains("┼"), "stdout: {text}");

    let dest = dir.join("nested-out");
    let out = run(sqz()
        .args(["--lang", "en-US", "nested", "extract"])
        .arg(&outer)
        .arg(nested_entry)
        .arg("-d")
        .arg(&dest)
        .arg("--include")
        .arg("project/sub/*")
        .arg("--smart")
        .arg("--json"));
    assert!(
        out.status.success(),
        "nested extract failed: {}",
        stderr(&out)
    );
    let report: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    assert_eq!(report["ok"], true);
    assert_eq!(report["operation"], "nested_extract");
    assert_eq!(report["dest"], dest.display().to_string());
    assert_eq!(report["matched"], true);
    assert_eq!(report["best_effort"], false);
    assert_eq!(report["skipped"], 0);
    assert!(report["problems"].as_array().unwrap().is_empty());
    assert_eq!(
        std::fs::read(dest.join("project/sub/b.txt")).unwrap(),
        b"nested content"
    );
    assert!(!dest.join("project/a.txt").exists());

    let empty_dest = dir.join("nested-empty-out");
    let out = run(sqz()
        .args(["--lang", "en-US", "nested", "extract"])
        .arg(&outer)
        .arg(nested_entry)
        .arg("-d")
        .arg(&empty_dest)
        .arg("--include")
        .arg("does-not-match/*")
        .arg("--json"));
    assert!(
        out.status.success(),
        "nested extract no-match failed: {}",
        stderr(&out)
    );
    let report: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    assert_eq!(report["ok"], true);
    assert_eq!(report["operation"], "nested_extract");
    assert_eq!(report["dest"], empty_dest.display().to_string());
    assert_eq!(report["matched"], false);
    assert_eq!(report["best_effort"], false);
    assert_eq!(report["skipped"], 0);
    assert!(report["problems"].as_array().unwrap().is_empty());
    assert!(!empty_dest.exists());

    let modern_dest = dir.join("nested-modern-out");
    let out = run(sqz()
        .args(["--lang", "en-US", "--style", "modern", "--color", "never"])
        .arg("nested")
        .arg("extract")
        .arg(&outer)
        .arg(nested_entry)
        .arg("-d")
        .arg(&modern_dest)
        .arg("--include")
        .arg("project/sub/*"));
    assert!(
        out.status.success(),
        "nested modern extract failed: {}",
        stderr(&out)
    );
    let text = stdout(&out);
    assert!(text.contains("Extract complete"), "stdout: {text}");
    assert!(text.contains("Status"), "stdout: {text}");
    assert!(text.contains("Mode"), "stdout: {text}");
    assert!(text.contains("Destination"), "stdout: {text}");
    assert!(text.contains("strict"), "stdout: {text}");
    assert!(text.contains("┬"), "stdout: {text}");
    assert!(text.contains("┼"), "stdout: {text}");
    assert_eq!(
        std::fs::read(modern_dest.join("project/sub/b.txt")).unwrap(),
        b"nested content"
    );
    assert!(!modern_dest.join("project/a.txt").exists());

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn compress_list_test_extract_sqz_roundtrip() {
    let dir = temp_dir("sqz-roundtrip");
    let root = sample_tree(&dir);
    let archive = dir.join("out.sqz");

    let out = run(sqz()
        .args(["--lang", "en-US", "compress"])
        .arg(&root)
        .arg("-o")
        .arg(&archive));
    assert!(out.status.success(), "compress failed: {}", stderr(&out));
    assert!(stdout(&out).contains("Created"), "stdout: {}", stdout(&out));

    let out = run(sqz().arg("list").arg(&archive).arg("--json"));
    assert!(out.status.success(), "list failed: {}", stderr(&out));
    let entries: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    let entries = entries.as_array().expect("top-level array");
    let file = entries
        .iter()
        .find(|e| e["path"] == "project/sub/b.txt")
        .expect("nested file listed");
    assert_eq!(file["type"], "file");
    assert_eq!(file["size"], 14);
    assert_eq!(file["compressed_size"], 14);
    assert_eq!(file["crc32"], serde_json::Value::Null);

    let out = run(sqz().arg("test").arg(&archive).arg("--json"));
    assert!(out.status.success(), "test failed: {}", stderr(&out));
    let report: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    assert_eq!(report["ok"], true);
    assert!(report["entries_tested"].as_u64().unwrap() >= 6);

    let dest = dir.join("extracted");
    let out = run(sqz()
        .args(["--lang", "en-US", "extract"])
        .arg(&archive)
        .arg("-d")
        .arg(&dest));
    assert!(out.status.success(), "extract failed: {}", stderr(&out));
    assert_eq!(
        std::fs::read(dest.join("project/sub/b.txt")).unwrap(),
        b"nested content"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn sqz_header_damage_recovers_through_cli() {
    let dir = temp_dir("sqz-header-cli");
    let root = sample_tree(&dir);
    let archive = dir.join("out.sqz");

    let out = run(sqz().arg("pack").arg(&root).arg("-o").arg(&archive));
    assert!(out.status.success(), "pack failed: {}", stderr(&out));
    corrupt_sqz_file_header_crc(&archive);

    let out = run(sqz().arg("list").arg(&archive).arg("--json"));
    assert!(out.status.success(), "list failed: {}", stderr(&out));
    let entries: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    assert!(entries
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry["path"] == "project/sub/b.txt"));

    let out = run(sqz().arg("test").arg(&archive).arg("--json"));
    assert!(out.status.success(), "test failed: {}", stderr(&out));
    let report: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    assert_eq!(report["ok"], true);

    let dest = dir.join("extracted");
    let out = run(sqz().arg("extract").arg(&archive).arg("-d").arg(&dest));
    assert!(out.status.success(), "extract failed: {}", stderr(&out));
    assert_eq!(
        std::fs::read(dest.join("project/sub/b.txt")).unwrap(),
        b"nested content"
    );

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn sqz_footer_header_bounds_damage_fails_through_cli() {
    let dir = temp_dir("sqz-footer-cli");
    let root = sample_tree(&dir);
    let archive = dir.join("out.sqz");

    let out = run(sqz().arg("pack").arg(&root).arg("-o").arg(&archive));
    assert!(out.status.success(), "pack failed: {}", stderr(&out));
    corrupt_sqz_footer_index_length_with_valid_crc(&archive);

    let out = run(sqz().arg("list").arg(&archive).arg("--json"));
    assert_json_error(&out, 3, "corrupt_archive", "footer index");

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn sqz_header_footer_uuid_mismatch_fails_through_cli() {
    let dir = temp_dir("sqz-header-footer-uuid-cli");
    let root = sample_tree(&dir);
    let archive = dir.join("out.sqz");

    let out = run(sqz().arg("pack").arg(&root).arg("-o").arg(&archive));
    assert!(out.status.success(), "pack failed: {}", stderr(&out));
    corrupt_sqz_file_header_uuid_with_valid_crc(&archive);

    for command in ["list", "test"] {
        let out = run(sqz().arg(command).arg(&archive).arg("--json"));
        assert_json_error(&out, 3, "corrupt_archive", "header/footer UUID mismatch");
    }

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn sqz_recovery_protection_trailer_damage_recovers_through_cli() {
    let dir = temp_dir("sqz-rspc-trailer-cli");
    let root = sample_tree(&dir);
    let archive = dir.join("out.sqz");

    let out = run(sqz().arg("pack").arg(&root).arg("-o").arg(&archive));
    assert!(out.status.success(), "pack failed: {}", stderr(&out));
    corrupt_sqz_recovery_protection_trailer(&archive);

    let out = run(sqz()
        .args(["--lang", "en-US", "list"])
        .arg(&archive)
        .arg("--json"));
    assert!(out.status.success(), "list failed: {}", stderr(&out));
    let entries: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    assert!(entries
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry["path"] == "project/sub/b.txt"));

    let out = run(sqz()
        .args(["--lang", "en-US", "test"])
        .arg(&archive)
        .arg("--json"));
    assert!(out.status.success(), "test failed: {}", stderr(&out));
    let report: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    assert_eq!(report["ok"], true);

    let dest = dir.join("extracted");
    let out = run(sqz().arg("extract").arg(&archive).arg("-d").arg(&dest));
    assert!(out.status.success(), "extract failed: {}", stderr(&out));
    assert_eq!(
        std::fs::read(dest.join("project/sub/b.txt")).unwrap(),
        b"nested content"
    );

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn sqz_recovery_protection_trailer_and_primary_damage_fails_through_cli() {
    let dir = temp_dir("sqz-rspc-trailer-primary-cli");
    let root = sample_tree(&dir);
    let archive = dir.join("out.sqz");

    let out = run(sqz().arg("pack").arg(&root).arg("-o").arg(&archive));
    assert!(out.status.success(), "pack failed: {}", stderr(&out));
    corrupt_sqz_recovery_primary_block(&archive);
    corrupt_sqz_recovery_protection_trailer(&archive);

    let out = run(sqz()
        .args(["--lang", "en-US", "list"])
        .arg(&archive)
        .arg("--json"));
    assert_json_error(&out, 3, "corrupt_archive", "recovery protection trailer");

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn sqz_footer_magic_damage_recovers_through_cli() {
    let dir = temp_dir("sqz-footer-recover-cli");
    let root = sample_tree(&dir);
    let archive = dir.join("out.sqz");

    let out = run(sqz().arg("pack").arg(&root).arg("-o").arg(&archive));
    assert!(out.status.success(), "pack failed: {}", stderr(&out));
    corrupt_sqz_footer_magic(&archive);

    let out = run(sqz().arg("list").arg(&archive).arg("--json"));
    assert!(out.status.success(), "list failed: {}", stderr(&out));
    let entries: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    assert!(entries
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry["path"] == "project/sub/b.txt"));

    let out = run(sqz().arg("test").arg(&archive).arg("--json"));
    assert!(out.status.success(), "test failed: {}", stderr(&out));
    let report: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    assert_eq!(report["ok"], true);

    let dest = dir.join("extracted");
    let out = run(sqz().arg("extract").arg(&archive).arg("-d").arg(&dest));
    assert!(out.status.success(), "extract failed: {}", stderr(&out));
    assert_eq!(
        std::fs::read(dest.join("project/sub/b.txt")).unwrap(),
        b"nested content"
    );

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn sqz_footer_crc_field_damage_recovers_through_cli() {
    let dir = temp_dir("sqz-footer-crc-field-cli");
    let root = sample_tree(&dir);
    let archive = dir.join("out.sqz");

    let out = run(sqz().arg("pack").arg(&root).arg("-o").arg(&archive));
    assert!(out.status.success(), "pack failed: {}", stderr(&out));
    corrupt_sqz_footer_crc_field(&archive);

    let out = run(sqz().arg("list").arg(&archive).arg("--json"));
    assert!(out.status.success(), "list failed: {}", stderr(&out));
    let entries: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    assert!(entries
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry["path"] == "project/sub/b.txt"));

    let out = run(sqz().arg("test").arg(&archive).arg("--json"));
    assert!(out.status.success(), "test failed: {}", stderr(&out));
    let report: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    assert_eq!(report["ok"], true);

    let dest = dir.join("extracted");
    let out = run(sqz().arg("extract").arg(&archive).arg("-d").arg(&dest));
    assert!(out.status.success(), "extract failed: {}", stderr(&out));
    assert_eq!(
        std::fs::read(dest.join("project/sub/b.txt")).unwrap(),
        b"nested content"
    );

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn pack_creates_sqz_container_as_a_first_class_cli_entry() {
    let dir = temp_dir("pack-sqz");
    let root = sample_tree(&dir);
    let archive = dir.join("packed.sqz");

    let out = run(sqz()
        .args(["--lang", "en-US", "pack"])
        .arg(&root)
        .arg("-o")
        .arg(&archive)
        .args([
            "--exclude",
            ".git",
            "--threads",
            "2",
            "--inner-format",
            "sqz",
            "--recovery",
            "10%",
        ])
        .arg("--json"));
    assert!(out.status.success(), "pack failed: {}", stderr(&out));
    let report: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    assert_eq!(report["ok"], true);
    assert_eq!(report["operation"], "pack_sqz");
    assert_eq!(report["output"], archive.display().to_string());
    assert_eq!(report["split"], false);
    assert_eq!(report["volumes"], 1);
    assert_eq!(report["inner_format"], "sqz");
    assert_eq!(report["recovery_percent"], 10);

    let out = run(sqz().arg("list").arg(&archive).arg("--json"));
    assert!(out.status.success(), "list failed: {}", stderr(&out));
    let entries: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    let paths: Vec<&str> = entries
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["path"].as_str().unwrap())
        .collect();
    assert!(paths.contains(&"project/a.txt"));
    assert!(paths.contains(&"project/sub/b.txt"));
    assert!(!paths.iter().any(|p| p.contains(".git")));

    let dest = dir.join("packed-files");
    let out = run(sqz().arg("extract").arg(&archive).arg("-d").arg(&dest));
    assert!(out.status.success(), "extract failed: {}", stderr(&out));
    assert_eq!(
        std::fs::read(dest.join("project/a.txt")).unwrap(),
        b"hello world"
    );

    let split_input = incompressible_file(&dir, "pack-json-data.bin");
    let split_archive = dir.join("packed-split.sqz");
    let out = run(sqz()
        .arg("pack")
        .arg(&split_input)
        .arg("-o")
        .arg(&split_archive)
        .args([
            "--inner-format",
            "sqz",
            "--recovery",
            "10%",
            "--split",
            "30k",
        ])
        .arg("--json"));
    assert!(out.status.success(), "split pack failed: {}", stderr(&out));
    let report: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    assert_eq!(report["ok"], true);
    assert_eq!(report["operation"], "pack_sqz");
    assert_eq!(
        report["output"],
        dir.join("packed-split.sqz.001").display().to_string()
    );
    assert_eq!(report["split"], true);
    assert!(report["volumes"].as_u64().unwrap() >= 2);
    assert_eq!(report["inner_format"], "sqz");
    assert_eq!(report["recovery_percent"], 10);

    let zip_profile_archive = dir.join("packed-zip-profile.sqz");
    let out = run(sqz()
        .arg("pack")
        .arg(&root)
        .arg("-o")
        .arg(&zip_profile_archive)
        .args([
            "--exclude",
            ".git",
            "--inner-format",
            "zip",
            "--recovery",
            "10%",
        ])
        .arg("--json"));
    assert!(
        out.status.success(),
        "zip profile pack failed: {}",
        stderr(&out)
    );
    let report: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    assert_eq!(report["ok"], true);
    assert_eq!(report["operation"], "pack_sqz");
    assert_eq!(report["output"], zip_profile_archive.display().to_string());
    assert_eq!(report["inner_format"], "zip");
    assert_eq!(report["recovery_percent"], 10);

    let out = run(sqz().arg("list").arg(&zip_profile_archive).arg("--json"));
    assert!(
        out.status.success(),
        "zip profile list failed: {}",
        stderr(&out)
    );
    let entries: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    let paths: Vec<&str> = entries
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["path"].as_str().unwrap())
        .collect();
    assert!(paths.contains(&"project/a.txt"));
    assert!(paths.contains(&"project/sub/b.txt"));
    assert!(!paths.contains(&"__sqz_inner.zip"));
    assert!(!paths.iter().any(|p| p.contains(".git")));

    let out = run(sqz().arg("test").arg(&zip_profile_archive).arg("--json"));
    assert!(
        out.status.success(),
        "zip profile test failed: {}",
        stderr(&out)
    );
    let report: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    assert_eq!(report["ok"], true);

    let dest = dir.join("zip-profile-files");
    let out = run(sqz()
        .arg("extract")
        .arg(&zip_profile_archive)
        .arg("-d")
        .arg(&dest));
    assert!(
        out.status.success(),
        "zip profile extract failed: {}",
        stderr(&out)
    );
    assert_eq!(
        std::fs::read(dest.join("project/sub/b.txt")).unwrap(),
        b"nested content"
    );

    let exported_zip_profile = dir.join("zip-profile-exported.zip");
    let out = run(sqz()
        .args(["--lang", "en-US", "export"])
        .arg(&zip_profile_archive)
        .arg("-o")
        .arg(&exported_zip_profile)
        .arg("--json"));
    assert!(
        out.status.success(),
        "zip profile export failed: {}",
        stderr(&out)
    );
    let report: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    assert_eq!(report["ok"], true);
    assert_eq!(report["operation"], "export_sqz");

    let out = run(sqz().arg("list").arg(&exported_zip_profile).arg("--json"));
    assert!(
        out.status.success(),
        "zip profile exported list failed: {}",
        stderr(&out)
    );
    let entries: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    let paths: Vec<&str> = entries
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["path"].as_str().unwrap())
        .collect();
    assert!(paths.contains(&"project/a.txt"));
    assert!(paths.contains(&"project/sub/b.txt"));
    if let Ok(out) = Command::new("unzip")
        .args(["-t", "-qq"])
        .arg(&exported_zip_profile)
        .output()
    {
        assert!(
            out.status.success(),
            "system unzip -t failed: {}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }

    assert_sqz_standard_inner_profile_roundtrip(
        &dir,
        &root,
        "tar",
        "__sqz_inner.tar",
        "tar-profile-exported.tar",
    );
    assert_sqz_standard_inner_profile_roundtrip(
        &dir,
        &root,
        "7z",
        "__sqz_inner.7z",
        "sevenz-profile-exported.7z",
    );
    assert_sqz_standard_inner_profile_roundtrip(
        &dir,
        &root,
        "zstd",
        "__sqz_inner.tar.zst",
        "zstd-profile-exported.tar.zst",
    );

    let out = run(sqz()
        .arg("pack")
        .arg(&root)
        .arg("-o")
        .arg(dir.join("not-sqz.zip")));
    assert_eq!(out.status.code(), Some(2), "stderr: {}", stderr(&out));
    assert!(
        stderr(&out).contains("pack output must end with .sqz"),
        "stderr: {}",
        stderr(&out)
    );

    {
        let profile = "raw";
        let out = run(sqz()
            .arg("pack")
            .arg(&root)
            .arg("-o")
            .arg(dir.join(format!("inner-{profile}.sqz")))
            .args(["--inner-format", profile]));
        assert_eq!(out.status.code(), Some(2), "stderr: {}", stderr(&out));
        assert!(
            stderr(&out).contains("currently supports only --inner-format sqz")
                && stderr(&out).contains("zip")
                && stderr(&out).contains("tar")
                && stderr(&out).contains("7z")
                && stderr(&out).contains("zstd")
                && stderr(&out).contains(profile),
            "stderr: {}",
            stderr(&out)
        );
    }

    std::fs::remove_dir_all(&dir).unwrap();
}

fn assert_sqz_standard_inner_profile_roundtrip(
    dir: &Path,
    root: &Path,
    profile: &str,
    payload_name: &str,
    exported_name: &str,
) {
    let archive = dir.join(format!("packed-{profile}-profile.sqz"));
    let out = run(sqz()
        .arg("pack")
        .arg(root)
        .arg("-o")
        .arg(&archive)
        .args([
            "--exclude",
            ".git",
            "--inner-format",
            profile,
            "--recovery",
            "10%",
        ])
        .arg("--json"));
    assert!(
        out.status.success(),
        "{profile} profile pack failed: {}",
        stderr(&out)
    );
    let report: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    assert_eq!(report["ok"], true);
    assert_eq!(report["operation"], "pack_sqz");
    assert_eq!(report["output"], archive.display().to_string());
    assert_eq!(report["split"], false);
    assert_eq!(report["volumes"], 1);
    assert_eq!(report["inner_format"], profile);
    assert_eq!(report["recovery_percent"], 10);

    let out = run(sqz().arg("list").arg(&archive).arg("--json"));
    assert!(
        out.status.success(),
        "{profile} profile list failed: {}",
        stderr(&out)
    );
    let entries: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    let paths: Vec<&str> = entries
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["path"].as_str().unwrap())
        .collect();
    assert!(
        paths.contains(&"project/a.txt"),
        "{profile} paths: {paths:?}"
    );
    assert!(
        paths.contains(&"project/sub/b.txt"),
        "{profile} paths: {paths:?}"
    );
    let file = entries
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["path"] == "project/a.txt")
        .unwrap_or_else(|| panic!("{profile} project/a.txt entry missing"));
    assert_eq!(file["type"], "file");
    assert_eq!(file["size"], 11);
    assert_eq!(file["encoding"], "utf-8");
    assert_eq!(file["encrypted"], false);
    assert!(
        !paths.contains(&payload_name),
        "{profile} payload wrapper leaked into public listing"
    );
    assert!(!paths.iter().any(|p| p.contains(".git")));

    let out = run(sqz().arg("test").arg(&archive).arg("--json"));
    assert!(
        out.status.success(),
        "{profile} profile test failed: {}",
        stderr(&out)
    );
    let report: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    assert_eq!(report["ok"], true);
    assert!(report["entries_tested"].as_u64().unwrap() > 0);
    assert!(report["problems"].as_array().unwrap().is_empty());
    assert_eq!(report["recovery"]["scheme"], "sqz-embedded-rs-gf8");
    assert_eq!(report["recovery"]["data_shards"], 8);
    assert_eq!(report["recovery"]["parity_shards"], 1);
    assert_eq!(report["recovery"]["damaged_blocks"], 0);
    assert_eq!(report["recovery"]["repaired_blocks"], 0);
    assert_eq!(report["recovery"]["unrepaired_blocks"], 0);
    assert_eq!(report["recovery"]["repair_possible"], true);

    let dest = dir.join(format!("{profile}-profile-files"));
    let out = run(sqz()
        .arg("extract")
        .arg(&archive)
        .arg("-d")
        .arg(&dest)
        .arg("--json"));
    assert!(
        out.status.success(),
        "{profile} profile extract failed: {}",
        stderr(&out)
    );
    let report: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    assert_eq!(report["ok"], true);
    assert_eq!(report["operation"], "extract");
    assert_eq!(report["dest"], dest.display().to_string());
    assert_eq!(report["matched"], true);
    assert_eq!(report["best_effort"], false);
    assert_eq!(report["skipped"], 0);
    assert!(report["problems"].as_array().unwrap().is_empty());
    assert_eq!(
        std::fs::read(dest.join("project/sub/b.txt")).unwrap(),
        b"nested content"
    );

    let exported = dir.join(exported_name);
    let out = run(sqz()
        .args(["--lang", "en-US", "export"])
        .arg(&archive)
        .arg("-o")
        .arg(&exported)
        .arg("--json"));
    assert!(
        out.status.success(),
        "{profile} profile export failed: {}",
        stderr(&out)
    );
    let report: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    assert_eq!(report["ok"], true);
    assert_eq!(report["operation"], "export_sqz");
    assert_eq!(report["archive"], archive.display().to_string());
    assert_eq!(report["output"], exported.display().to_string());

    let out = run(sqz().arg("list").arg(&exported).arg("--json"));
    assert!(
        out.status.success(),
        "{profile} profile exported list failed: {}",
        stderr(&out)
    );
    let entries: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    let paths: Vec<&str> = entries
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["path"].as_str().unwrap())
        .collect();
    assert!(
        paths.contains(&"project/a.txt"),
        "{profile} exported paths: {paths:?}"
    );
    assert!(
        paths.contains(&"project/sub/b.txt"),
        "{profile} exported paths: {paths:?}"
    );

    if exported_name.ends_with(".tar") {
        if let Ok(out) = Command::new("tar").arg("-tf").arg(&exported).output() {
            assert!(
                out.status.success(),
                "system tar -tf failed: {}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
        }
    }
}

#[test]
fn sqz_tar_inner_profile_uses_outer_recovery_before_inner_open() {
    let dir = temp_dir("sqz-tar-inner-recover-cli");
    let root = sample_tree(&dir);
    let archive = dir.join("recoverable-tar-inner.sqz");
    let out = run(sqz()
        .arg("pack")
        .arg(&root)
        .arg("-o")
        .arg(&archive)
        .args([
            "--exclude",
            ".git",
            "--inner-format",
            "tar",
            "--recovery",
            "25%",
        ])
        .arg("--json"));
    assert!(
        out.status.success(),
        "tar profile pack failed: {}",
        stderr(&out)
    );
    corrupt_sqz_payload_byte(&archive);

    let out = run(sqz().arg("test").arg(&archive).arg("--json"));
    assert!(
        out.status.success(),
        "test repaired tar inner failed: {}",
        stderr(&out)
    );
    let report: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    assert_eq!(report["ok"], true);
    assert!(report["entries_tested"].as_u64().unwrap() > 0);
    assert!(report["problems"].as_array().unwrap().is_empty());
    assert_eq!(report["recovery"]["scheme"], "sqz-embedded-rs-gf8");
    assert_eq!(report["recovery"]["damaged_blocks"], 1);
    assert_eq!(report["recovery"]["repaired_blocks"], 1);
    assert_eq!(report["recovery"]["unrepaired_blocks"], 0);
    assert_eq!(report["recovery"]["repair_possible"], true);

    let out = run(sqz().arg("list").arg(&archive).arg("--json"));
    assert!(
        out.status.success(),
        "list repaired tar inner failed: {}",
        stderr(&out)
    );
    let entries: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    let paths: Vec<&str> = entries
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["path"].as_str().unwrap())
        .collect();
    assert!(paths.contains(&"project/a.txt"), "paths: {paths:?}");
    assert!(paths.contains(&"project/sub/b.txt"), "paths: {paths:?}");
    assert!(!paths.contains(&"__sqz_inner.tar"));

    let dest = dir.join("recovered-tar-inner-files");
    let out = run(sqz()
        .arg("extract")
        .arg(&archive)
        .arg("-d")
        .arg(&dest)
        .arg("--json"));
    assert!(
        out.status.success(),
        "extract repaired tar inner failed: {}",
        stderr(&out)
    );
    let report: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    assert_eq!(report["ok"], true);
    assert_eq!(report["matched"], true);
    assert_eq!(report["skipped"], 0);
    assert!(report["problems"].as_array().unwrap().is_empty());
    assert_eq!(
        std::fs::read(dest.join("project/sub/b.txt")).unwrap(),
        b"nested content"
    );

    let exported = dir.join("recovered-tar-inner-export.tar");
    let out = run(sqz()
        .arg("export")
        .arg(&archive)
        .arg("-o")
        .arg(&exported)
        .arg("--json"));
    assert!(
        out.status.success(),
        "export repaired tar inner failed: {}",
        stderr(&out)
    );
    let report: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    assert_eq!(report["ok"], true);
    assert_eq!(report["operation"], "export_sqz");

    let out = run(sqz().arg("list").arg(&exported).arg("--json"));
    assert!(
        out.status.success(),
        "list repaired tar inner export failed: {}",
        stderr(&out)
    );
    let entries: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    assert!(entries
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry["path"] == "project/sub/b.txt"));

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn cli_memory_limit_is_enforced_for_stream_pumps() {
    let dir = temp_dir("memory-limit");
    let input = dir.join("payload.txt");
    std::fs::write(&input, vec![b'x'; 32 * 1024]).unwrap();
    let archive = dir.join("payload.txt.gz");

    let out = run(sqz()
        .arg("compress")
        .arg(&input)
        .arg("-o")
        .arg(&archive)
        .arg("--memory-limit")
        .arg("1k"));
    assert!(
        !out.status.success(),
        "compress should reject too-small memory limit"
    );
    assert!(
        stderr(&out).contains("memory limit"),
        "stderr: {}",
        stderr(&out)
    );

    let out = run(sqz()
        .arg("compress")
        .arg(&input)
        .arg("-o")
        .arg(&archive)
        .arg("--memory-limit")
        .arg("8k"));
    assert!(out.status.success(), "compress failed: {}", stderr(&out));

    let low_dest = dir.join("low-dest");
    let out = run(sqz()
        .arg("extract")
        .arg(&archive)
        .arg("-d")
        .arg(&low_dest)
        .arg("--memory-limit")
        .arg("1k"));
    assert!(
        !out.status.success(),
        "extract should reject too-small memory limit"
    );
    assert!(
        stderr(&out).contains("memory limit"),
        "stderr: {}",
        stderr(&out)
    );

    let dest = dir.join("dest");
    let out = run(sqz()
        .arg("extract")
        .arg(&archive)
        .arg("-d")
        .arg(&dest)
        .arg("--memory-limit")
        .arg("8k"));
    assert!(out.status.success(), "extract failed: {}", stderr(&out));
    assert_eq!(
        std::fs::read(dest.join("payload.txt")).unwrap(),
        vec![b'x'; 32 * 1024]
    );

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn compress_exclude_prunes_entries() {
    let dir = temp_dir("exclude");
    let root = sample_tree(&dir);
    let archive = dir.join("out.zip");

    let out = run(sqz()
        .arg("compress")
        .arg(&root)
        .arg("-o")
        .arg(&archive)
        .args(["--exclude", ".git", "--exclude", "*.tmp"]));
    assert!(out.status.success(), "compress failed: {}", stderr(&out));

    let out = run(sqz().arg("list").arg(&archive).arg("--json"));
    let entries: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    let paths: Vec<&str> = entries
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["path"].as_str().unwrap())
        .collect();
    assert!(paths.contains(&"project/a.txt"));
    assert!(paths.contains(&"project/sub/b.txt"));
    assert!(!paths.iter().any(|p| p.contains(".git")));
    assert!(!paths.iter().any(|p| p.ends_with(".tmp")));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn compress_profile_matches_gui_presets_and_allows_level_override() {
    let dir = temp_dir("compress-profile");
    let root = sample_tree(&dir);
    let archive = dir.join("maximum.zip");

    let out = run(sqz()
        .arg("compress")
        .arg(&root)
        .arg("-o")
        .arg(&archive)
        .args(["--profile", "maximum", "--json"]));
    assert!(out.status.success(), "compress failed: {}", stderr(&out));
    let report: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    assert_eq!(report["ok"], true);
    assert_eq!(report["operation"], "compress");
    assert_eq!(report["level"], 9);

    let out = run(sqz().arg("list").arg(&archive).arg("--json"));
    assert!(out.status.success(), "list failed: {}", stderr(&out));
    let entries: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert!(entries
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry["path"] == "project/sub/b.txt"));

    let override_archive = dir.join("override.zip");
    let out = run(sqz()
        .arg("compress")
        .arg(&root)
        .arg("-o")
        .arg(&override_archive)
        .args(["--profile", "maximum", "--level", "3", "--json"]));
    assert!(out.status.success(), "compress failed: {}", stderr(&out));
    let report: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    assert_eq!(report["level"], 3);

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn estimate_matches_gui_create_preflight_semantics() {
    let dir = temp_dir("estimate-cli");
    let root = sample_tree(&dir);
    let planned = dir.join("planned.zip");

    let out = run(sqz()
        .arg("estimate")
        .arg(&root)
        .args(["--exclude", ".git", "--exclude", "*.tmp"])
        .arg("-o")
        .arg(&planned)
        .arg("--json"));
    assert!(out.status.success(), "estimate failed: {}", stderr(&out));
    let report: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    assert_eq!(report["input_count"], 1);
    assert_eq!(report["entries"], 4);
    assert_eq!(report["files"], 2);
    assert_eq!(report["directories"], 2);
    assert_eq!(report["symlinks"], 0);
    assert_eq!(report["total_bytes"], 25);
    assert_eq!(
        report["output_budget_bytes"],
        25 + 1024 * 1024 + 4 * 1024 + 4096
    );
    assert_eq!(report["disk"]["path"], planned.display().to_string());
    assert_eq!(
        report["disk"]["required_bytes"],
        report["output_budget_bytes"]
    );
    assert!(report["disk"]["available_bytes"].as_u64().unwrap() > 0);
    assert_eq!(report["disk"]["ok"], true);

    let out = run(sqz()
        .args(["--lang", "en-US", "estimate"])
        .arg(&root)
        .args(["--exclude", ".git", "--exclude", "*.tmp"]));
    assert!(out.status.success(), "estimate failed: {}", stderr(&out));
    assert!(
        stdout(&out).contains("entries: 4"),
        "stdout: {}",
        stdout(&out)
    );

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn extract_include_selects_entries() {
    let dir = temp_dir("include");
    let root = sample_tree(&dir);
    let archive = dir.join("out.zip");
    run(sqz().arg("compress").arg(&root).arg("-o").arg(&archive));

    let dest = dir.join("partial");
    let out = run(sqz()
        .arg("extract")
        .arg(&archive)
        .arg("-d")
        .arg(&dest)
        .args(["--include", "project/sub/*"]));
    assert!(out.status.success(), "extract failed: {}", stderr(&out));
    assert!(dest.join("project/sub/b.txt").is_file());
    assert!(!dest.join("project/a.txt").exists());

    // No match: succeeds and reports the no-op in JSON for scripts.
    let dest2 = dir.join("none");
    let out = run(sqz()
        .args(["--lang", "en-US", "extract"])
        .arg(&archive)
        .arg("-d")
        .arg(&dest2)
        .args(["--include", "no/such/entry"])
        .arg("--json"));
    assert!(out.status.success());
    let report: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    assert_eq!(report["ok"], true);
    assert_eq!(report["operation"], "extract");
    assert_eq!(report["dest"], dest2.display().to_string());
    assert_eq!(report["matched"], false);
    assert_eq!(report["best_effort"], false);
    assert_eq!(report["skipped"], 0);
    assert!(report["problems"].as_array().unwrap().is_empty());
    assert!(!dest2.join("project").exists());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn extract_cli_safety_limits_are_enforced_by_core() {
    let dir = temp_dir("extract-limits");
    let root = sample_tree(&dir);
    let archive = dir.join("out.zip");
    let out = run(sqz().arg("compress").arg(&root).arg("-o").arg(&archive));
    assert!(out.status.success(), "compress failed: {}", stderr(&out));

    let out = run(sqz()
        .args(["--lang", "en-US", "extract"])
        .arg(&archive)
        .arg("-d")
        .arg(dir.join("too-many"))
        .args(["--max-entries", "1", "--threads", "2"]));
    assert_eq!(out.status.code(), Some(6), "stderr: {}", stderr(&out));
    assert!(
        stderr(&out).contains("Resource limit exceeded")
            || stderr(&out).contains("entry count exceeds limit"),
        "stderr: {}",
        stderr(&out)
    );

    let out = run(sqz()
        .arg("extract")
        .arg(&archive)
        .arg("-d")
        .arg(dir.join("bad-limit"))
        .args(["--max-output-bytes", "0"]));
    assert!(!out.status.success(), "zero size limit should be rejected");

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn extract_best_effort_skips_unreadable_entries() {
    let dir = temp_dir("best-effort-cli");
    let root = dir.join("src");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("good.txt"), b"good-data").unwrap();
    std::fs::write(root.join("bad.txt"), b"bad-data").unwrap();
    let archive = dir.join("out.zip");

    let out = run(sqz()
        .arg("compress")
        .arg(&root)
        .arg("-o")
        .arg(&archive)
        .args(["--level", "0"]));
    assert!(out.status.success(), "compress failed: {}", stderr(&out));
    corrupt_stored_zip_payload(&archive, b"bad-data");

    let dest = dir.join("readable");
    let out = run(sqz()
        .args(["--lang", "en-US", "extract"])
        .arg(&archive)
        .arg("-d")
        .arg(&dest)
        .arg("--best-effort"));
    assert!(out.status.success(), "extract failed: {}", stderr(&out));
    assert_eq!(
        std::fs::read(dest.join("src/good.txt")).unwrap(),
        b"good-data"
    );
    assert!(!dest.join("src/bad.txt").exists());
    assert!(
        stderr(&out).contains("Best-effort extract skipped 1"),
        "stderr: {}",
        stderr(&out)
    );

    let json_dest = dir.join("readable-json");
    let out = run(sqz()
        .args(["--lang", "en-US", "extract"])
        .arg(&archive)
        .arg("-d")
        .arg(&json_dest)
        .arg("--best-effort")
        .arg("--json"));
    assert!(
        out.status.success(),
        "json extract failed: {}",
        stderr(&out)
    );
    let report: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    assert_eq!(report["ok"], true);
    assert_eq!(report["operation"], "extract");
    assert_eq!(report["dest"], json_dest.display().to_string());
    assert_eq!(report["matched"], true);
    assert_eq!(report["best_effort"], true);
    assert_eq!(report["skipped"], 1);
    assert!(report["problems"][0].as_str().unwrap().contains("bad.txt"));
    assert_eq!(
        std::fs::read(json_dest.join("src/good.txt")).unwrap(),
        b"good-data"
    );
    assert!(!json_dest.join("src/bad.txt").exists());

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn zip_local_header_fallback_is_available_through_cli() {
    let dir = temp_dir("zip-local-header-cli");
    let archive = dir.join("missing-central.zip");
    std::fs::write(
        &archive,
        stored_zip_with_missing_central_directory(&[
            (b"good.txt", b"safe bytes"),
            (b"docs/readme.md", b"# recovered\n"),
        ]),
    )
    .unwrap();

    let out = run(sqz().arg("list").arg(&archive).arg("--json"));
    assert!(out.status.success(), "list failed: {}", stderr(&out));
    let entries: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    let paths: Vec<&str> = entries
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["path"].as_str().unwrap())
        .collect();
    assert_eq!(paths, vec!["good.txt", "docs/readme.md"]);

    let out = run(sqz().arg("test").arg(&archive).arg("--json"));
    assert!(out.status.success(), "test failed: {}", stderr(&out));
    let report: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    assert_eq!(report["ok"], true);
    assert_eq!(report["entries_tested"], 2);
    assert!(report["problems"].as_array().unwrap().is_empty());

    let dest = dir.join("out");
    let out = run(sqz()
        .args(["--lang", "en-US", "extract"])
        .arg(&archive)
        .arg("-d")
        .arg(&dest));
    assert!(out.status.success(), "extract failed: {}", stderr(&out));
    assert_eq!(std::fs::read(dest.join("good.txt")).unwrap(), b"safe bytes");
    assert_eq!(
        std::fs::read_to_string(dest.join("docs/readme.md")).unwrap(),
        "# recovered\n"
    );

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn repair_zip_rebuilds_missing_central_directory_through_cli() {
    let dir = temp_dir("zip-rebuild-cli");
    let archive = dir.join("missing-central.zip");
    let repaired = dir.join("repaired.zip");
    std::fs::write(
        &archive,
        stored_zip_with_missing_central_directory(&[
            (b"good.txt", b"safe bytes"),
            (b"docs/readme.md", b"# rebuilt\n"),
        ]),
    )
    .unwrap();
    let original = std::fs::read(&archive).unwrap();
    assert!(
        !original.windows(4).any(|window| window == b"PK\x01\x02"),
        "sample must not contain a central directory"
    );

    let out = run(sqz()
        .args(["--lang", "en-US", "repair"])
        .arg(&archive)
        .arg("-o")
        .arg(&repaired)
        .args(["--threads", "2", "--json"]));
    assert!(out.status.success(), "repair failed: {}", stderr(&out));
    let report: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    assert_eq!(report["ok"], true);
    assert_eq!(report["operation"], "repair_zip");
    assert_eq!(report["tool"], "zip-local-header-rebuild");
    assert_eq!(report["archive"], archive.display().to_string());
    assert_eq!(report["output"], repaired.display().to_string());
    assert_eq!(report["in_place"], false);
    assert_eq!(report["source"]["ok"], true);
    assert_eq!(report["source"]["entries_tested"], 2);

    let rebuilt = std::fs::read(&repaired).unwrap();
    assert!(
        rebuilt.windows(4).any(|window| window == b"PK\x01\x02"),
        "rebuilt ZIP must contain a central directory"
    );
    assert!(
        rebuilt.windows(4).any(|window| window == b"PK\x05\x06"),
        "rebuilt ZIP must contain an end-of-central-directory record"
    );

    let out = run(sqz().arg("test").arg(&repaired).arg("--json"));
    assert!(out.status.success(), "test failed: {}", stderr(&out));
    let report: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    assert_eq!(report["ok"], true);

    let dest = dir.join("out");
    let out = run(sqz().arg("extract").arg(&repaired).arg("-d").arg(&dest));
    assert!(out.status.success(), "extract failed: {}", stderr(&out));
    assert_eq!(std::fs::read(dest.join("good.txt")).unwrap(), b"safe bytes");
    assert_eq!(
        std::fs::read_to_string(dest.join("docs/readme.md")).unwrap(),
        "# rebuilt\n"
    );

    let out = run(sqz().args(["--lang", "en-US", "repair"]).arg(&archive));
    assert_eq!(out.status.code(), Some(2), "stderr: {}", stderr(&out));
    assert!(
        stderr(&out).contains("requires --output"),
        "stderr: {}",
        stderr(&out)
    );

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn repair_zip_rebuild_refuses_damaged_local_header_payloads() {
    let dir = temp_dir("zip-rebuild-damaged-cli");
    let archive = dir.join("damaged-missing-central.zip");
    let repaired = dir.join("must-not-exist.zip");
    std::fs::write(
        &archive,
        stored_zip_with_missing_central_directory(&[(b"bad.txt", b"visible payload")]),
    )
    .unwrap();
    corrupt_stored_zip_payload(&archive, b"visible payload");

    let out = run(sqz()
        .args(["--lang", "en-US", "repair"])
        .arg(&archive)
        .arg("-o")
        .arg(&repaired)
        .arg("--json"));
    assert_eq!(out.status.code(), Some(3), "stderr: {}", stderr(&out));
    let report: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    assert_eq!(report["ok"], false);
    assert_eq!(report["operation"], "repair_zip");
    assert_eq!(report["tool"], "zip-local-header-rebuild");
    assert_eq!(report["source"]["ok"], false);
    assert!(
        report["problems"]
            .as_array()
            .unwrap()
            .iter()
            .any(|problem| problem
                .as_str()
                .is_some_and(|text| text.contains("bad.txt"))),
        "report: {report}"
    );
    assert!(
        !repaired.exists(),
        "damaged payload must not produce a rebuilt archive"
    );

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn test_cli_accepts_entry_name_encoding_override() {
    const NAME_UTF8: &str = "压缩文件中文名称测试.txt";
    const NAME_GBK: &[u8] = &[
        0xD1, 0xB9, 0xCB, 0xF5, 0xCE, 0xC4, 0xBC, 0xFE, 0xD6, 0xD0, 0xCE, 0xC4, 0xC3, 0xFB, 0xB3,
        0xC6, 0xB2, 0xE2, 0xCA, 0xD4, 0x2E, 0x74, 0x78, 0x74,
    ];

    let dir = temp_dir("test-encoding-override");
    let archive = dir.join("gbk-damaged.zip");
    std::fs::write(
        &archive,
        stored_zip_with_missing_central_directory(&[(NAME_GBK, b"GBK named payload")]),
    )
    .unwrap();
    corrupt_stored_zip_payload(&archive, b"GBK named payload");

    let out = run(sqz()
        .arg("test")
        .arg(&archive)
        .args(["--encoding", "gbk", "--json"]));
    assert_eq!(out.status.code(), Some(3), "stderr: {}", stderr(&out));
    let report: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    assert_eq!(report["ok"], false);
    assert_eq!(report["entries_tested"], 1);
    let problems = report["problems"].as_array().unwrap();
    assert!(
        problems
            .iter()
            .any(|problem| problem.as_str().unwrap().contains(NAME_UTF8)),
        "problem paths should honor --encoding gbk: {problems:?}"
    );

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn zip64_local_header_fallback_is_available_through_cli() {
    let dir = temp_dir("zip64-local-header-cli");
    let archive = dir.join("zip64-local-only.zip");
    std::fs::write(
        &archive,
        stored_zip64_local_header_without_central_directory(
            b"large-marker.bin",
            b"zip64 local header payload",
        ),
    )
    .unwrap();

    let out = run(sqz().arg("list").arg(&archive).arg("--json"));
    assert!(out.status.success(), "list failed: {}", stderr(&out));
    let entries: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    let entries = entries.as_array().expect("top-level array");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["path"], "large-marker.bin");
    assert_eq!(entries[0]["size"], 26);
    assert_eq!(entries[0]["compressed_size"], 26);

    let out = run(sqz().arg("test").arg(&archive).arg("--json"));
    assert!(out.status.success(), "test failed: {}", stderr(&out));
    let report: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    assert_eq!(report["ok"], true);
    assert_eq!(report["entries_tested"], 1);
    assert!(report["problems"].as_array().unwrap().is_empty());

    let dest = dir.join("out");
    let out = run(sqz()
        .args(["--lang", "en-US", "extract"])
        .arg(&archive)
        .arg("-d")
        .arg(&dest));
    assert!(out.status.success(), "extract failed: {}", stderr(&out));
    assert_eq!(
        std::fs::read(dest.join("large-marker.bin")).unwrap(),
        b"zip64 local header payload"
    );

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn zip_local_header_encrypted_fallback_boundary_is_visible_through_cli() {
    let dir = temp_dir("zip-local-encrypted-cli");
    let archive = dir.join("encrypted-local-only.zip");
    std::fs::write(
        &archive,
        stored_encrypted_flag_zip_without_central_directory(
            b"secret.txt",
            b"plaintext sample is not exposed",
        ),
    )
    .unwrap();

    let out = run(sqz().arg("list").arg(&archive).arg("--json"));
    assert!(out.status.success(), "list failed: {}", stderr(&out));
    let entries: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    let entries = entries.as_array().expect("top-level array");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["path"], "secret.txt");
    assert_eq!(entries[0]["encrypted"], true);
    assert_eq!(entries[0]["size"], 31);

    let out = run(sqz()
        .args(["--lang", "en-US", "test"])
        .arg(&archive)
        .arg("--json"));
    assert_json_error(&out, 4, "password_required", "A password is required");

    let dest = dir.join("out");
    let out = run(sqz()
        .args(["--lang", "en-US", "extract"])
        .arg(&archive)
        .arg("-d")
        .arg(&dest));
    assert_eq!(out.status.code(), Some(4), "stderr: {}", stderr(&out));
    assert!(
        stderr(&out).contains("A password is required"),
        "stderr: {}",
        stderr(&out)
    );
    assert!(
        !dest.join("secret.txt").exists(),
        "encrypted fallback entry must not be written without a password"
    );

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn zip_local_header_unsupported_method_boundary_is_visible_through_cli() {
    let dir = temp_dir("zip-local-unsupported-method-cli");
    let archive = dir.join("unsupported-method-local-only.zip");
    std::fs::write(
        &archive,
        stored_unsupported_method_zip_without_central_directory(
            b"compressed.bin",
            b"opaque compressed payload",
            14,
        ),
    )
    .unwrap();

    let out = run(sqz().arg("list").arg(&archive).arg("--json"));
    assert!(out.status.success(), "list failed: {}", stderr(&out));
    let entries: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    let entries = entries.as_array().expect("top-level array");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["path"], "compressed.bin");
    assert_eq!(entries[0]["size"], 25);
    assert_eq!(entries[0]["compressed_size"], 25);

    let out = run(sqz().arg("test").arg(&archive).arg("--json"));
    assert_eq!(out.status.code(), Some(3), "stderr: {}", stderr(&out));
    let report: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    assert_eq!(report["ok"], false);
    assert_eq!(report["entries_tested"], 1);
    assert!(
        report["problems"]
            .as_array()
            .unwrap()
            .iter()
            .any(|problem| problem
                .as_str()
                .is_some_and(|text| text.contains("compression method 14"))),
        "report: {report}"
    );

    let dest = dir.join("out");
    let out = run(sqz()
        .args(["--lang", "en-US", "extract"])
        .arg(&archive)
        .arg("-d")
        .arg(&dest));
    assert_eq!(out.status.code(), Some(2), "stderr: {}", stderr(&out));
    assert!(
        stderr(&out).contains("compression method 14"),
        "stderr: {}",
        stderr(&out)
    );
    assert!(
        !dest.join("compressed.bin").exists(),
        "unsupported local-header entry must not be written"
    );

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn zip64_data_descriptor_fallback_is_available_through_cli() {
    let dir = temp_dir("zip64-descriptor-cli");
    let archive = dir.join("zip64-descriptor-only.zip");
    std::fs::write(
        &archive,
        stored_zip64_data_descriptor_without_central_directory(
            b"streamed64.txt",
            b"zip64 descriptor payload",
        ),
    )
    .unwrap();

    let out = run(sqz().arg("list").arg(&archive).arg("--json"));
    assert!(out.status.success(), "list failed: {}", stderr(&out));
    let entries: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    let entries = entries.as_array().expect("top-level array");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["path"], "streamed64.txt");
    assert_eq!(entries[0]["size"], 24);
    assert_eq!(entries[0]["compressed_size"], 24);

    let out = run(sqz().arg("test").arg(&archive).arg("--json"));
    assert!(out.status.success(), "test failed: {}", stderr(&out));
    let report: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    assert_eq!(report["ok"], true);
    assert_eq!(report["entries_tested"], 1);
    assert!(report["problems"].as_array().unwrap().is_empty());

    let dest = dir.join("out");
    let out = run(sqz()
        .args(["--lang", "en-US", "extract"])
        .arg(&archive)
        .arg("-d")
        .arg(&dest));
    assert!(out.status.success(), "extract failed: {}", stderr(&out));
    assert_eq!(
        std::fs::read(dest.join("streamed64.txt")).unwrap(),
        b"zip64 descriptor payload"
    );

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn exit_codes_match_the_documented_mapping() {
    let dir = temp_dir("exit-codes");

    // Missing input file → I/O error → 7.
    let out = run(sqz().arg("list").arg(dir.join("missing.zip")));
    assert_eq!(out.status.code(), Some(7), "stderr: {}", stderr(&out));

    // Unknown format → 2.
    let weird = dir.join("blob.weird");
    std::fs::write(&weird, b"this is not an archive, just bytes").unwrap();
    let out = run(sqz().arg("list").arg(&weird));
    assert_eq!(out.status.code(), Some(2), "stderr: {}", stderr(&out));

    // Corrupt archive → 3.
    let corrupt = dir.join("corrupt.zip");
    std::fs::write(&corrupt, b"PK\x03\x04 then pure garbage with no directory").unwrap();
    let out = run(sqz().arg("list").arg(&corrupt));
    assert_eq!(out.status.code(), Some(3), "stderr: {}", stderr(&out));

    // Wrong password (non-TTY, explicit --password) → 4, no retry prompt.
    let root = sample_tree(&dir);
    let archive = dir.join("secret.zip");
    let out = run(sqz()
        .arg("compress")
        .arg(&root)
        .arg("-o")
        .arg(&archive)
        .args(["--password", "right"]));
    assert!(out.status.success());
    let out = run(sqz()
        .arg("extract")
        .arg(&archive)
        .arg("-d")
        .arg(dir.join("x"))
        .args(["--password", "wrong"]));
    assert_eq!(out.status.code(), Some(4), "stderr: {}", stderr(&out));

    // Missing password (non-TTY) → 4 as well.
    let out = run(sqz()
        .arg("extract")
        .arg(&archive)
        .arg("-d")
        .arg(dir.join("y")));
    assert_eq!(out.status.code(), Some(4), "stderr: {}", stderr(&out));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn language_selection_and_fallback() {
    let dir = temp_dir("lang");
    let root = sample_tree(&dir);

    // --lang en-US vs zh-CN.
    let out = run(sqz()
        .args(["--lang", "en-US", "compress"])
        .arg(&root)
        .arg("-o")
        .arg(dir.join("a.zip")));
    assert!(stdout(&out).contains("Created"));
    let out = run(sqz()
        .args(["--lang", "zh-CN", "compress"])
        .arg(&root)
        .arg("-o")
        .arg(dir.join("b.zip")));
    assert!(stdout(&out).contains("已创建"));

    // SQZ_LANG environment variable.
    let out = run(sqz()
        .env("SQZ_LANG", "zh-CN")
        .arg("compress")
        .arg(&root)
        .arg("-o")
        .arg(dir.join("c.zip")));
    assert!(stdout(&out).contains("已创建"));

    // --lang wins over SQZ_LANG.
    let out = run(sqz()
        .env("SQZ_LANG", "zh-CN")
        .args(["--lang", "en-US", "compress"])
        .arg(&root)
        .arg("-o")
        .arg(dir.join("d.zip")));
    assert!(stdout(&out).contains("Created"));

    // Errors are localized too (variant → key mapping).
    let out = run(sqz()
        .args(["--lang", "zh-CN", "list"])
        .arg(dir.join("missing.zip")));
    assert!(stderr(&out).contains("错误："), "stderr: {}", stderr(&out));
    let out = run(sqz()
        .args(["--lang", "en-US", "list"])
        .arg(dir.join("missing.zip")));
    assert!(stderr(&out).contains("Error:"), "stderr: {}", stderr(&out));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn user_locale_packs_override_and_fall_back() {
    let dir = temp_dir("user-locales");
    let root = sample_tree(&dir);
    let locales = dir.join("locales");
    std::fs::create_dir_all(&locales).unwrap();
    // A new language with a partial pack: present keys are used, missing
    // keys fall back to en-US.
    std::fs::write(
        locales.join("xx-XX.json"),
        r#"{"cli.compress.done": "XX DONE {path}"}"#,
    )
    .unwrap();
    // Same-named keys override a built-in language.
    std::fs::write(
        locales.join("zh-CN.json"),
        r#"{"cli.compress.done": "搞定 {path}"}"#,
    )
    .unwrap();

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_sqz"));
    cmd.env_remove("SQZ_LANG");
    cmd.env("SQZ_LOCALES_DIR", &locales);
    let out = run(cmd
        .args(["--lang", "xx-XX", "compress"])
        .arg(&root)
        .arg("-o")
        .arg(dir.join("a.zip")));
    assert!(stdout(&out).contains("XX DONE"), "stdout: {}", stdout(&out));

    // Missing key in xx-XX falls back to the en-US text.
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_sqz"));
    cmd.env_remove("SQZ_LANG");
    cmd.env("SQZ_LOCALES_DIR", &locales);
    let out = run(cmd
        .args(["--lang", "xx-XX", "extract"])
        .arg(dir.join("a.zip"))
        .arg("-d")
        .arg(dir.join("x")));
    assert!(
        stdout(&out).contains("Extracted to"),
        "stdout: {}",
        stdout(&out)
    );

    // User override of a built-in language.
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_sqz"));
    cmd.env_remove("SQZ_LANG");
    cmd.env("SQZ_LOCALES_DIR", &locales);
    let out = run(cmd
        .args(["--lang", "zh-CN", "compress"])
        .arg(&root)
        .arg("-o")
        .arg(dir.join("b.zip")));
    assert!(stdout(&out).contains("搞定"), "stdout: {}", stdout(&out));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn overwrite_ask_degrades_to_skip_without_a_tty() {
    let dir = temp_dir("ask-degrade");
    let root = sample_tree(&dir);
    let archive = dir.join("out.zip");
    run(sqz().arg("compress").arg(&root).arg("-o").arg(&archive));

    let dest = dir.join("dest");
    // Pre-create a conflicting file with different content.
    std::fs::create_dir_all(dest.join("project")).unwrap();
    std::fs::write(dest.join("project/a.txt"), b"KEEP ME").unwrap();

    let out = run(sqz()
        .args(["--lang", "en-US", "extract"])
        .arg(&archive)
        .arg("-d")
        .arg(&dest)
        .args(["--overwrite", "ask"]));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    // Degradation warning on stderr (from the language pack).
    assert!(
        stderr(&out).contains("degraded to skip"),
        "stderr: {}",
        stderr(&out)
    );
    // The existing file was kept, the rest extracted normally.
    assert_eq!(
        std::fs::read(dest.join("project/a.txt")).unwrap(),
        b"KEEP ME"
    );
    assert_eq!(
        std::fs::read(dest.join("project/sub/b.txt")).unwrap(),
        b"nested content"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn info_json_reports_formats_and_capabilities() {
    let out = run(sqz().arg("info").arg("--json"));
    assert!(out.status.success());
    let formats: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    let zip = formats
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["id"] == "zip")
        .expect("zip format present");
    assert_eq!(zip["kind"], "archive");
    assert_eq!(zip["capabilities"]["can_create"], true);
    assert_eq!(zip["capabilities"]["can_encrypt_names"], false);
    assert!(zip["extensions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e == "zip"));
    assert_eq!(zip["level_mapping"]["cli_to_level"]["0"], "store");
    assert_eq!(zip["level_mapping"]["cli_to_level"]["5"], "normal");
    assert_eq!(zip["level_mapping"]["cli_to_level"]["9"], "ultra");
    assert_eq!(zip["level_mapping"]["backend"]["normal"], "deflate 6");
    assert_eq!(zip["implementation"]["status"], "built_in");
    assert_eq!(zip["implementation"]["bundled"], true);
    assert_eq!(
        zip["implementation"]["availability"]["read"]["available"],
        true
    );
    assert_eq!(
        zip["implementation"]["availability"]["read"]["source"],
        "built_in"
    );
    assert_eq!(
        zip["implementation"]["availability"]["write"]["available"],
        true
    );
    assert_eq!(
        zip["implementation"]["release_gate"],
        serde_json::Value::Null
    );
}

#[test]
fn info_json_reports_external_tool_availability() {
    let missing_7z = "/definitely/missing/squallz-test-7z";
    let missing_wimlib = "/definitely/missing/squallz-test-wimlib";
    let out = run(sqz()
        .env("SQUALLZ_7Z", missing_7z)
        .env("SQUALLZ_WIMLIB", missing_wimlib)
        .env_remove("SQUALLZ_BSDTAR")
        .arg("info")
        .arg("--json"));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let formats = stdout_json(&out);
    let formats = formats.as_array().unwrap();
    let find = |id: &str| {
        formats
            .iter()
            .find(|f| f["id"] == id)
            .unwrap_or_else(|| panic!("{id} missing from sqz info"))
    };

    let cab_read = &find("cab")["implementation"]["availability"]["read"];
    assert_eq!(cab_read["available"], false);
    assert_eq!(cab_read["configured"], true);
    assert_eq!(cab_read["source"], "env");
    assert_eq!(cab_read["env"], "SQUALLZ_7Z");
    assert_eq!(cab_read["selected"], missing_7z);
    assert_eq!(cab_read["path_exists"], false);

    let rar_read = &find("rar")["implementation"]["availability"]["read"];
    assert_eq!(rar_read["available"], false);
    assert_eq!(rar_read["source"], "env");
    assert_eq!(rar_read["selected"], missing_7z);
    assert_eq!(rar_read["path_exists"], false);
    let rar_policy = &find("rar")["implementation"]["policy"];
    assert_eq!(rar_policy["read_only"], true);
    assert_eq!(rar_policy["bundled"], false);
    assert_eq!(rar_policy["primary_env"], "SQUALLZ_7Z");
    assert_eq!(rar_policy["fallback_env"], "SQUALLZ_BSDTAR");
    assert_eq!(rar_policy["fallback_scope"], "diagnostic_or_rar5_v6");
    assert!(rar_policy["license_boundary"]
        .as_str()
        .is_some_and(|boundary| boundary.contains("does not link unrar code")));

    let wim_write = &find("wim")["implementation"]["availability"]["write"];
    assert_eq!(wim_write["available"], false);
    assert_eq!(wim_write["configured"], true);
    assert_eq!(wim_write["source"], "env");
    assert_eq!(wim_write["env"], "SQUALLZ_WIMLIB");
    assert_eq!(wim_write["selected"], missing_wimlib);
    assert_eq!(wim_write["path_exists"], false);

    let cab_write = &find("cab")["implementation"]["availability"]["write"];
    assert_eq!(cab_write["available"], false);
    assert_eq!(cab_write["source"], "unsupported");
}

#[test]
fn info_json_reports_available_external_tool_from_path() {
    let dir = temp_dir("info-tool-availability");
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let tool = if cfg!(windows) {
        bin.join("7zz.exe")
    } else {
        bin.join("7zz")
    };
    std::fs::write(&tool, "#!/bin/sh\nexit 0\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&tool).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&tool, perms).unwrap();
    }
    let old_path = std::env::var_os("PATH").unwrap_or_default();
    let path =
        std::env::join_paths(std::iter::once(bin.clone()).chain(std::env::split_paths(&old_path)))
            .unwrap();
    let selected = tool.to_string_lossy().into_owned();

    let out = run(sqz()
        .env_remove("SQUALLZ_7Z")
        .env_remove("SQUALLZ_BSDTAR")
        .env("PATH", path)
        .arg("info")
        .arg("--json"));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let formats = stdout_json(&out);
    let formats = formats.as_array().unwrap();
    let find = |id: &str| {
        formats
            .iter()
            .find(|f| f["id"] == id)
            .unwrap_or_else(|| panic!("{id} missing from sqz info"))
    };

    let cab_read = &find("cab")["implementation"]["availability"]["read"];
    assert_eq!(cab_read["available"], true);
    assert_eq!(cab_read["configured"], false);
    assert_eq!(cab_read["source"], "path");
    assert_eq!(cab_read["env"], "SQUALLZ_7Z");
    assert_eq!(cab_read["selected"].as_str(), Some(selected.as_str()));
    assert_eq!(cab_read["path_exists"], true);

    let rar_read = &find("rar")["implementation"]["availability"]["read"];
    assert_eq!(rar_read["available"], true);
    assert_eq!(rar_read["source"], "path");
    assert_eq!(rar_read["selected"].as_str(), Some(selected.as_str()));
    assert_eq!(rar_read["path_exists"], true);
    assert_eq!(
        find("rar")["implementation"]["policy"]["fallback_scope"],
        "diagnostic_or_rar5_v6"
    );

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn doctor_json_reports_runtime_and_recovery_boundaries() {
    let dir = temp_dir("doctor-runtime-ready");
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let sevenz = write_fake_executable(&bin, "7zz");
    let wimlib = write_fake_executable(&bin, "wimlib-imagex");
    let par2 = write_fake_executable(&bin, "par2");

    let out = run(sqz()
        .env("SQUALLZ_7Z", &sevenz)
        .env("SQUALLZ_WIMLIB", &wimlib)
        .env("SQUALLZ_PAR2", &par2)
        .arg("doctor")
        .arg("--json")
        .arg("--strict"));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let report = stdout_json(&out);
    assert_eq!(report["ok"], true);
    assert_eq!(report["operation"], "doctor");
    assert_eq!(report["strict"], true);
    assert_eq!(report["summary"]["formats"], 43);
    assert!(report["summary"]["ready"].as_u64().unwrap() >= 43);
    let checks = report["checks"].as_array().unwrap();
    let find = |id: &str| {
        checks
            .iter()
            .find(|check| check["id"] == id)
            .unwrap_or_else(|| panic!("{id} missing from doctor report: {report}"))
    };
    assert_eq!(find("7z-read-bridge")["status"], "pass");
    assert_eq!(find("wim-writer")["status"], "pass");
    assert_eq!(find("par2-create")["status"], "pass");
    assert_eq!(find("par2-verify-repair")["status"], "pass");
    assert_eq!(find("rar-product-boundary")["status"], "boundary");
    assert!(find("rar-product-boundary")["detail"]
        .as_str()
        .unwrap()
        .contains("outside release claims"));
    assert_eq!(find("par2-create")["availability"]["env"], "SQUALLZ_PAR2");

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn doctor_strict_json_exits_dependency_missing_when_runtime_tools_are_missing() {
    let missing = "/definitely/missing/squallz-doctor-tool";
    let out = run(sqz()
        .env("SQUALLZ_7Z", missing)
        .env("SQUALLZ_WIMLIB", missing)
        .env("SQUALLZ_PAR2", missing)
        .env_remove("SQUALLZ_BSDTAR")
        .arg("doctor")
        .arg("--json")
        .arg("--strict"));
    assert_eq!(out.status.code(), Some(8), "stderr: {}", stderr(&out));
    assert!(
        stderr(&out).trim().is_empty(),
        "doctor --json --strict must not print a second error envelope: {}",
        stderr(&out)
    );
    let report = stdout_json(&out);
    assert_eq!(report["ok"], false);
    let checks = report["checks"].as_array().unwrap();
    let find = |id: &str| {
        checks
            .iter()
            .find(|check| check["id"] == id)
            .unwrap_or_else(|| panic!("{id} missing from doctor report: {report}"))
    };
    assert_eq!(find("7z-read-bridge")["status"], "fail");
    assert_eq!(find("wim-writer")["status"], "fail");
    assert_eq!(find("par2-create")["status"], "fail");
    assert_eq!(find("par2-verify-repair")["status"], "pass");
    assert_eq!(
        find("par2-verify-repair")["availability"]["source"],
        "built_in_fallback"
    );
    assert_eq!(find("rar-product-boundary")["status"], "boundary");
}

#[test]
fn info_text_marks_builtin_and_external_implementations() {
    let out = run(sqz().args(["--lang", "en-US", "info"]));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("Summary"), "{text}");
    assert!(text.contains("Coverage"), "{text}");
    assert!(text.contains("Ready now"), "{text}");
    assert!(text.contains("Needs tools"), "{text}");
    assert!(text.contains("Pack / unpack"), "{text}");
    assert!(text.contains("Unpack only"), "{text}");
    assert!(text.contains("Stream codecs"), "{text}");
    assert!(text.contains("zip, tar, 7z, wim, sqz"), "{text}");
    assert!(text.contains("rar, apfs, ar, arj"), "{text}");
    assert!(text.contains("Engine"), "{text}");
    assert!(text.contains("Capabilities"), "{text}");
    assert!(!text.contains("✓"), "{text}");
    assert!(!text.contains("·"), "{text}");
    assert!(!text.contains("│"), "{text}");
    assert!(!text.contains("╭"), "{text}");
    assert!(!text.contains("◆"), "{text}");
    assert!(!text.contains("Implementation:"), "{text}");
    let find_line = |id: &str| {
        text.lines()
            .find(|line| line.split_whitespace().next() == Some(id))
            .unwrap_or_else(|| panic!("{id} info line missing: {text}"))
    };
    let zip_line = find_line("zip");
    let rar_line = find_line("rar");
    let wim_line = find_line("wim");
    assert!(zip_line.contains("built-in"), "{zip_line}");
    assert!(
        zip_line.contains("create extract test update split encrypt"),
        "{zip_line}"
    );
    assert!(!zip_line.contains("yes"), "{zip_line}");
    assert!(
        rar_line.contains("external: 7zz/7z; bsdtar diagnostic fallback"),
        "{rar_line}"
    );
    assert!(rar_line.contains("extract test"), "{rar_line}");
    assert!(
        wim_line.contains("external: 7zz read; wimlib write"),
        "{wim_line}"
    );
    assert_no_i18n_keys(&text);
}

#[test]
fn info_modern_groups_formats_and_uses_capability_matrix() {
    let out = run(sqz().args(["--lang", "en-US", "--style", "modern", "info"]));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let text = stdout(&out);
    assert!(
        text.contains("43 formats · 10 built in · 33 external bridges"),
        "{text}"
    );
    assert!(text.contains("Legend: C=create X=extract"), "{text}");
    assert!(text.contains("╭─ Built-in archives · 4"), "{text}");
    assert!(text.contains("╭─ External archive bridges · 33"), "{text}");
    assert!(text.contains("╭─ Stream compressors · 6"), "{text}");
    assert!(text.contains("├"), "{text}");
    assert!(text.contains("┬"), "{text}");
    assert!(text.contains("┼"), "{text}");
    assert!(text.contains("┴"), "{text}");
    assert!(text.contains("C X T U S E N"), "{text}");
    assert!(text.contains("Runtime inventory"), "{text}");
    assert!(text.contains("Command forms"), "{text}");
    assert!(text.contains("scorecard + decision tables"), "{text}");
    assert!(
        text.contains("operation cockpit")
            && text.contains("signal matrix")
            && text.contains("transfer matrix")
            && text.contains("action queue"),
        "{text}"
    );
    assert!(text.contains("Modern dashboard"), "{text}");
    assert!(text.contains("Support map"), "{text}");
    assert!(text.contains("Format coverage"), "{text}");
    assert!(text.contains("Capability lanes"), "{text}");
    assert!(text.contains("Action selector"), "{text}");
    assert!(text.contains("Modern surfaces"), "{text}");
    assert!(text.contains("Best form"), "{text}");
    assert!(text.contains("scorecard + support map"), "{text}");
    assert!(text.contains("action queue"), "{text}");
    assert!(
        text.contains("phase rail") && text.contains("speed/ETA/current"),
        "{text}"
    );
    assert!(text.contains("Modern output"), "{text}");
    assert!(text.contains("Modern style guide"), "{text}");
    assert!(text.contains("operation cockpit"), "{text}");
    assert!(text.contains("--color fancy"), "{text}");
    assert!(text.contains("--color rich"), "{text}");
    assert!(text.contains("Best for"), "{text}");
    assert!(text.contains("Signal"), "{text}");
    assert!(text.contains("Palette gallery"), "{text}");
    assert!(text.contains("Look"), "{text}");
    assert!(text.contains("Command"), "{text}");
    assert!(
        text.contains("next step") && text.contains("current object"),
        "{text}"
    );
    assert!(text.contains("speed"), "{text}");
    assert!(text.contains("--palette brand"), "{text}");
    assert!(text.contains("--palette cascade"), "{text}");
    assert!(text.contains("--palette daylight"), "{text}");
    assert!(text.contains("--palette foam"), "{text}");
    assert!(text.contains("--palette skyline"), "{text}");
    assert!(text.contains("--palette aero"), "{text}");
    assert!(text.contains("--palette crest"), "{text}");
    assert!(text.contains("--palette halo"), "{text}");
    assert!(text.contains("--palette tropic"), "{text}");
    assert!(text.contains("--palette kinetic"), "{text}");
    assert!(text.contains("--palette radiant"), "{text}");
    assert!(text.contains("--palette surge"), "{text}");
    assert!(text.contains("--colors icon"), "{text}");
    assert!(text.contains("--colors glass"), "{text}");
    assert!(text.contains("--palette nova"), "{text}");
    assert!(text.contains("--palette crystal"), "{text}");
    assert!(text.contains("--palette lumina"), "{text}");
    assert!(text.contains("Color mode"), "{text}");
    assert!(text.contains("Palette"), "{text}");
    assert!(text.contains("Color scheme"), "{text}");
    assert!(text.contains("--color-scheme / --scheme"), "{text}");
    assert!(text.contains("--colors"), "{text}");
    assert!(text.contains("Progress HUD"), "{text}");
    assert!(
        text.contains("operation cockpit")
            && text.contains("signal matrix")
            && text.contains("transfer matrix")
            && text.contains("action queue")
            && text.contains("speed"),
        "{text}"
    );
    assert!(text.contains("primary / secondary"), "{text}");
    assert!(text.contains("Lane"), "{text}");
    assert!(text.contains("Mode"), "{text}");
    assert!(text.contains("Ready"), "{text}");
    assert!(text.contains("Risk"), "{text}");
    assert!(text.contains("Examples"), "{text}");
    assert!(text.contains("Format coverage"), "{text}");
    assert!(text.contains("Pack / unpack"), "{text}");
    assert!(text.contains("zip, tar, 7z, wim, sqz"), "{text}");
    assert!(text.contains("apfs, ar, arj"), "{text}");
    assert!(text.contains("Archive pack/unpack"), "{text}");
    assert!(text.contains("Unpack only"), "{text}");
    assert!(text.contains("Recovery/repair"), "{text}");
    assert!(text.contains("built-in + PAR2 opt"), "{text}");
    assert!(text.contains("Command cheatsheet"), "{text}");
    assert!(text.contains("Create archives"), "{text}");
    assert!(text.contains("Unpack archives"), "{text}");
    assert!(text.contains("sqz compress <input> -o out.zip"), "{text}");
    assert!(text.contains("Hide names"), "{text}");
    assert!(text.contains("Read"), "{text}");
    assert!(text.contains("Write"), "{text}");
    assert!(text.contains("Engine"), "{text}");
    assert!(text.contains(".zip .jar .apk .cbz .ipa"), "{text}");
    assert!(text.contains("✓ ✓ ✓ ✓ ✓ ✓ ·"), "{text}");
    assert!(text.contains("· ✓ ✓ · · · ·"), "{text}");
    assert!(text.contains("ready(7z)"), "{text}");
    assert!(text.contains("unsupported"), "{text}");
    assert!(text.contains("external: 7zz/7z"), "{text}");
    assert!(text.contains("bsdtar diagnostic"), "{text}");
    assert!(text.contains("external: 7zz read; wimlib write"), "{text}");
    assert!(!text.contains("Implementation:"), "{text}");
    assert_no_i18n_keys(&text);
}

#[test]
fn info_lists_all_i3_formats_registry_driven() {
    // The CLI itself was not touched for I3: every new format must surface
    // through the registry alone.
    let out = run(sqz().arg("info").arg("--json"));
    assert!(out.status.success());
    let formats: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    let formats = formats.as_array().unwrap();
    let find = |id: &str| {
        formats
            .iter()
            .find(|f| f["id"] == id)
            .unwrap_or_else(|| panic!("{id} missing from sqz info"))
    };
    for id in ["zip", "tar", "7z", "rar"] {
        assert_eq!(find(id)["kind"], "archive");
    }
    for id in [
        "apfs", "ar", "arj", "cab", "chm", "cpio", "cramfs", "dmg", "ext", "fat", "gpt", "hfs",
        "ihex", "iso", "lzh", "lzma", "mbr", "msi", "nsis", "ntfs", "qcow2", "rpm", "squashfs",
        "udf", "uefi", "vdi", "vhd", "vhdx", "vmdk", "xar", "z",
    ] {
        let format = find(id);
        assert_eq!(format["kind"], "archive");
        assert_eq!(format["capabilities"]["can_create"], false);
        assert_eq!(format["capabilities"]["can_extract"], true);
        assert_eq!(format["capabilities"]["can_test"], true);
        assert_eq!(format["implementation"]["status"], "external_required");
        assert_eq!(format["implementation"]["bundled"], false);
        assert!(format["implementation"]["read"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tool| tool == "7zz"));
        assert_eq!(format["implementation"]["write"]["kind"], "unsupported");
    }
    let wim = find("wim");
    assert_eq!(wim["kind"], "archive");
    assert_eq!(wim["capabilities"]["can_create"], true);
    assert_eq!(wim["capabilities"]["can_extract"], true);
    assert_eq!(wim["capabilities"]["can_test"], true);
    assert_eq!(wim["implementation"]["status"], "external_required");
    assert_eq!(wim["implementation"]["write"]["env"], "SQUALLZ_WIMLIB");
    assert!(wim["implementation"]["write"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .any(|tool| tool == "wimlib-imagex"));
    assert!(wim["implementation"]["release_gate"]
        .as_str()
        .is_some_and(|gate| gate.contains("real WIM compatibility matrix")));
    for id in ["gzip", "bzip2", "xz", "zstd", "lz4", "brotli"] {
        assert_eq!(find(id)["kind"], "compressor");
    }
    let sevenz = find("7z");
    assert_eq!(sevenz["capabilities"]["can_create"], true);
    assert_eq!(sevenz["capabilities"]["can_encrypt_data"], true);
    assert_eq!(sevenz["capabilities"]["can_encrypt_names"], true);
    let tar = find("tar");
    assert_eq!(tar["capabilities"]["can_create"], true);
    assert_eq!(tar["capabilities"]["can_encrypt_data"], false);
    let rar = find("rar");
    assert_eq!(rar["capabilities"]["can_create"], false);
    assert_eq!(rar["capabilities"]["can_extract"], true);
    assert!(rar["level_mapping"].is_null());
    assert_eq!(rar["implementation"]["status"], "external_required");
    assert_eq!(rar["implementation"]["read"]["env"], "SQUALLZ_7Z");
    assert!(rar["implementation"]["read"]["fallback_tools"]
        .as_array()
        .unwrap()
        .iter()
        .any(|tool| tool == "bsdtar"));
    let rar_policy = &rar["implementation"]["policy"];
    assert_eq!(rar_policy["read_only"], true);
    assert_eq!(rar_policy["bundled"], false);
    assert!(rar_policy["primary_tools"]
        .as_array()
        .unwrap()
        .iter()
        .any(|tool| tool == "7zz"));
    assert!(rar_policy["fallback_tools"]
        .as_array()
        .unwrap()
        .iter()
        .any(|tool| tool == "bsdtar"));
    assert_eq!(rar_policy["fallback_scope"], "diagnostic_or_rar5_v6");
    assert!(rar_policy["release_claim"]
        .as_str()
        .is_some_and(|claim| claim.contains("read-only public-sample subset")));
    assert!(rar_policy["license_boundary"]
        .as_str()
        .is_some_and(|boundary| boundary.contains("unRAR restriction")));
    assert_eq!(rar["implementation"]["write"]["kind"], "unsupported");
    let rar_limitations = rar["implementation"]["limitations"]
        .as_array()
        .expect("RAR limitations are machine-readable");
    let has_rar_limit = |scope: &str, status: &str| {
        rar_limitations.iter().any(|item| {
            item["scope"] == scope
                && item["status"] == status
                && item["reason"]
                    .as_str()
                    .is_some_and(|reason| !reason.is_empty())
        })
    };
    assert!(has_rar_limit("create", "unsupported"));
    assert!(has_rar_limit("recovery_records", "unsupported"));
    assert!(has_rar_limit("encrypted", "not_release_claimed"));
    assert!(has_rar_limit("multi_volume", "not_release_claimed"));
    assert!(has_rar_limit("damaged_repair", "unsupported"));
    assert!(rar["extensions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e == "cbr"));
}

#[test]
fn rar_read_only_boundary_is_visible_through_cli() {
    let dir = temp_dir("rar-readonly-cli");
    let root = sample_tree(&dir);
    let input = dir.join("sample.rar");
    std::fs::write(&input, RAR5_MAGIC).unwrap();

    let out = run(sqz()
        .args(["--lang", "en-US", "list"])
        .arg(&input)
        .env("SQUALLZ_BSDTAR", "/definitely/missing/squallz-bsdtar"));
    assert_eq!(out.status.code(), Some(8), "stderr: {}", stderr(&out));
    assert!(
        stderr(&out).contains("Missing external dependency")
            && stderr(&out).contains("bsdtar with RAR/libarchive support"),
        "stderr: {}",
        stderr(&out)
    );

    let created = dir.join("created.rar");
    let out = run(sqz()
        .args(["--lang", "en-US", "compress"])
        .arg(&root)
        .arg("-o")
        .arg(&created));
    assert_eq!(out.status.code(), Some(2), "stderr: {}", stderr(&out));
    assert!(
        stderr(&out).contains("format rar does not support creation"),
        "stderr: {}",
        stderr(&out)
    );
    assert!(
        !created.exists(),
        "RAR create failure must not leave output"
    );

    std::fs::remove_dir_all(&dir).unwrap();
}

#[cfg(unix)]
#[test]
fn sevenzip_bridge_longtail_format_through_cli_when_tool_is_available() {
    use std::os::unix::fs::PermissionsExt;

    let dir = temp_dir("sevenzip-bridge-cli");
    let input = dir.join("sample.cab");
    let tool = dir.join("fake-7z.sh");
    let log = dir.join("fake-7z.log");
    std::fs::write(&input, b"MSCF fake cab").unwrap();

    let script = r#"#!/bin/sh
set -eu
printf '%s\n' "$*" >> "$SQUALLZ_FAKE_7Z_LOG"
if [ "$1" = "l" ] && [ "$2" = "-slt" ]; then
  cat <<'EOF'
Path = /tmp/squallz-fake-archive.cab
Type = cab
Physical Size = 1024
Size = 52
Packed Size = 21

Path = docs
Folder = +
Size = 0
Attributes = D

Path = hello.txt
Folder = -
Size = 26
Packed Size = 11
CRC = ABCD1234
Encrypted = -

Path = -dash.txt
Folder = -
Size = 26
Packed Size = 10
Encrypted = -

EOF
  exit 0
fi
if [ "$1" = "x" ] && [ "$2" = "-so" ]; then
  last=""
  prev=""
  for arg in "$@"; do
    prev="$last"
    last="$arg"
  done
  if [ "$last" = "-dash.txt" ] && [ "$prev" != "--" ]; then
    printf 'missing -- before dash entry\n' >&2
    exit 9
  fi
  case "$last" in
    hello.txt) printf 'hello from 7z cli bridge' ;;
    -dash.txt) printf 'dash entry from cli bridge' ;;
    *) printf 'unknown entry: %s\n' "$last" >&2; exit 3 ;;
  esac
  exit 0
fi
printf 'unexpected args\n' >&2
exit 2
"#;
    std::fs::write(&tool, script).unwrap();
    let mut perms = std::fs::metadata(&tool).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&tool, perms).unwrap();

    let bridge_env = |cmd: &mut Command| {
        cmd.env("SQUALLZ_7Z", &tool)
            .env("SQUALLZ_FAKE_7Z_LOG", &log);
    };

    let mut cmd = sqz();
    cmd.args(["--lang", "en-US", "list"])
        .arg(&input)
        .arg("--json");
    bridge_env(&mut cmd);
    let out = run(&mut cmd);
    assert!(out.status.success(), "list failed: {}", stderr(&out));
    let entries = stdout_json(&out);
    assert_eq!(entries.as_array().unwrap().len(), 3);
    assert!(
        !entries.as_array().unwrap().iter().any(|entry| entry["path"]
            .as_str()
            .is_some_and(|path| path.starts_with('/')))
    );
    assert!(entries.as_array().unwrap().iter().any(|entry| {
        entry["path"] == "hello.txt" && entry["crc32"] == serde_json::json!(0xABCD_1234u64)
    }));
    assert!(entries
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry["path"] == "-dash.txt"));

    let mut cmd = sqz();
    cmd.args(["--lang", "en-US", "test"])
        .arg(&input)
        .arg("--json");
    bridge_env(&mut cmd);
    let out = run(&mut cmd);
    assert!(out.status.success(), "test failed: {}", stderr(&out));
    let report = stdout_json(&out);
    assert_eq!(report["ok"], true);
    assert_eq!(report["entries_tested"], 2);

    let dest = dir.join("extracted");
    let mut cmd = sqz();
    cmd.args(["--lang", "en-US", "extract"])
        .arg(&input)
        .arg("-d")
        .arg(&dest);
    bridge_env(&mut cmd);
    let out = run(&mut cmd);
    assert!(out.status.success(), "extract failed: {}", stderr(&out));
    assert_eq!(
        std::fs::read(dest.join("hello.txt")).unwrap(),
        b"hello from 7z cli bridge"
    );
    assert_eq!(
        std::fs::read(dest.join("-dash.txt")).unwrap(),
        b"dash entry from cli bridge"
    );

    let log = std::fs::read_to_string(&log).unwrap();
    assert!(log.contains("l -slt"), "{log}");
    assert!(log.contains("x -so"), "{log}");
    assert!(log.contains("-- -dash.txt"), "{log}");

    std::fs::remove_dir_all(&dir).unwrap();
}

#[cfg(unix)]
#[test]
fn wim_create_and_read_through_external_bridges_when_tools_are_available() {
    use std::os::unix::fs::PermissionsExt;

    let dir = temp_dir("wim-create-cli");
    let root = sample_tree(&dir);
    let archive = dir.join("image.wim");
    let wimlib = dir.join("fake-wimlib.sh");
    let sevenz = dir.join("fake-7z.sh");
    let wimlib_log = dir.join("fake-wimlib.log");
    let sevenz_log = dir.join("fake-7z.log");

    let wimlib_script = r#"#!/bin/sh
set -eu
printf '%s\n' "$*" >> "$SQUALLZ_FAKE_WIMLIB_LOG"
if [ "$1" = "capture" ]; then
  src="$2"
  out="$3"
  [ -f "$src/project/a.txt" ]
  [ -f "$src/project/sub/b.txt" ]
  [ "$(cat "$src/project/a.txt")" = "hello world" ]
  [ "$(cat "$src/project/sub/b.txt")" = "nested content" ]
  printf 'MSWIM\000\000\000fake-cli-wim' > "$out"
  exit 0
fi
printf 'unexpected wimlib args\n' >&2
exit 2
"#;
    std::fs::write(&wimlib, wimlib_script).unwrap();
    let mut perms = std::fs::metadata(&wimlib).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&wimlib, perms).unwrap();

    let sevenz_script = r#"#!/bin/sh
set -eu
printf '%s\n' "$*" >> "$SQUALLZ_FAKE_7Z_LOG"
if [ "$1" = "l" ] && [ "$2" = "-slt" ]; then
  cat <<'EOF'
Path = project
Folder = +
Size = 0
Attributes = D

Path = project/a.txt
Folder = -
Size = 11
Packed Size = 11
Encrypted = -

Path = project/sub/b.txt
Folder = -
Size = 14
Packed Size = 14
Encrypted = -

EOF
  exit 0
fi
if [ "$1" = "x" ] && [ "$2" = "-so" ]; then
  last=""
  for arg in "$@"; do
    last="$arg"
  done
  case "$last" in
    project/a.txt) printf 'hello world' ;;
    project/sub/b.txt) printf 'nested content' ;;
    *) printf 'unknown entry: %s\n' "$last" >&2; exit 3 ;;
  esac
  exit 0
fi
printf 'unexpected 7z args\n' >&2
exit 2
"#;
    std::fs::write(&sevenz, sevenz_script).unwrap();
    let mut perms = std::fs::metadata(&sevenz).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&sevenz, perms).unwrap();

    let out = run(sqz()
        .args(["--lang", "en-US", "compress"])
        .arg(&root)
        .arg("-o")
        .arg(&archive)
        .arg("--json")
        .env("SQUALLZ_WIMLIB", &wimlib)
        .env("SQUALLZ_FAKE_WIMLIB_LOG", &wimlib_log));
    assert!(out.status.success(), "compress failed: {}", stderr(&out));
    let report = stdout_json(&out);
    assert_eq!(report["ok"], true);
    assert_eq!(report["operation"], "compress");
    assert_eq!(report["output"], archive.display().to_string());
    assert!(std::fs::read(&archive).unwrap().starts_with(b"MSWIM\0\0\0"));

    let out = run(sqz()
        .args(["--lang", "en-US", "list"])
        .arg(&archive)
        .arg("--json")
        .env("SQUALLZ_7Z", &sevenz)
        .env("SQUALLZ_FAKE_7Z_LOG", &sevenz_log));
    assert!(out.status.success(), "list failed: {}", stderr(&out));
    let entries = stdout_json(&out);
    assert!(entries
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry["path"] == "project/a.txt"));

    let out = run(sqz()
        .args(["--lang", "en-US", "test"])
        .arg(&archive)
        .arg("--json")
        .env("SQUALLZ_7Z", &sevenz)
        .env("SQUALLZ_FAKE_7Z_LOG", &sevenz_log));
    assert!(out.status.success(), "test failed: {}", stderr(&out));
    let report = stdout_json(&out);
    assert_eq!(report["ok"], true);
    assert_eq!(report["entries_tested"], 2);

    let dest = dir.join("out");
    let out = run(sqz()
        .args(["--lang", "en-US", "extract"])
        .arg(&archive)
        .arg("-d")
        .arg(&dest)
        .env("SQUALLZ_7Z", &sevenz)
        .env("SQUALLZ_FAKE_7Z_LOG", &sevenz_log));
    assert!(out.status.success(), "extract failed: {}", stderr(&out));
    assert_eq!(
        std::fs::read(dest.join("project/a.txt")).unwrap(),
        b"hello world"
    );
    assert_eq!(
        std::fs::read(dest.join("project/sub/b.txt")).unwrap(),
        b"nested content"
    );

    let wimlib_log = std::fs::read_to_string(&wimlib_log).unwrap();
    assert!(wimlib_log.contains("capture"), "{wimlib_log}");
    assert!(wimlib_log.contains("--compress=LZX"), "{wimlib_log}");
    let sevenz_log = std::fs::read_to_string(&sevenz_log).unwrap();
    assert!(sevenz_log.contains("l -slt"), "{sevenz_log}");
    assert!(sevenz_log.contains("x -so"), "{sevenz_log}");

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn wim_missing_writer_dependency_does_not_leave_output() {
    let dir = temp_dir("wim-missing-writer-cli");
    let root = sample_tree(&dir);
    let archive = dir.join("missing-writer.wim");
    let missing_tool = dir.join("missing-wimlib-imagex");

    let out = run(sqz()
        .args(["--lang", "en-US", "compress"])
        .arg(&root)
        .arg("-o")
        .arg(&archive)
        .arg("--json")
        .env("SQUALLZ_WIMLIB", &missing_tool));
    assert_json_error(&out, 8, "dependency_missing", "Missing external dependency");
    assert!(
        !archive.exists(),
        "failed WIM create must not leave an empty output"
    );

    std::fs::remove_dir_all(&dir).unwrap();
}

#[cfg(unix)]
#[test]
fn rar_bridge_list_test_extract_through_cli_when_tool_is_available() {
    use std::os::unix::fs::PermissionsExt;

    let dir = temp_dir("rar-bridge-cli");
    let input = dir.join("sample.rar");
    let tool = dir.join("fake-bsdtar.sh");
    let log = dir.join("fake-bsdtar.log");
    std::fs::write(&input, RAR5_MAGIC).unwrap();

    let script = r#"#!/bin/sh
set -eu
printf '%s\n' "$*" >> "$SQUALLZ_FAKE_BSDTAR_LOG"
if [ "$1" = "-tf" ]; then
  printf 'docs/\nhello.txt\n-dash.txt\n'
  exit 0
fi
if [ "$1" = "-tvf" ]; then
  printf 'drwxr-xr-x  0 0      0           0 Jan  1  2020 docs/\n'
  printf -- '-rw-r--r--  0 0      0          26 Jan  1  2020 hello.txt\n'
  printf -- '-rw-r--r--  0 0      0          26 Jan  1  2020 -dash.txt\n'
  exit 0
fi
if [ "$1" = "-xOf" ]; then
  last=""
  prev=""
  for arg in "$@"; do
    prev="$last"
    last="$arg"
  done
  if [ "$last" = "-dash.txt" ] && [ "$prev" != "--" ]; then
    printf 'missing -- before dash entry\n' >&2
    exit 9
  fi
  case "$last" in
    hello.txt) printf 'hello from cli rar bridge' ;;
    -dash.txt) printf 'dash entry from cli rar bridge' ;;
    *) printf 'unknown entry: %s\n' "$last" >&2; exit 3 ;;
  esac
  exit 0
fi
printf 'unexpected args\n' >&2
exit 2
"#;
    std::fs::write(&tool, script).unwrap();
    let mut perms = std::fs::metadata(&tool).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&tool, perms).unwrap();

    let out = run(sqz()
        .args(["--lang", "en-US", "list"])
        .arg(&input)
        .arg("--json")
        .env("SQUALLZ_BSDTAR", &tool)
        .env("SQUALLZ_FAKE_BSDTAR_LOG", &log));
    assert!(out.status.success(), "list failed: {}", stderr(&out));
    let entries: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    assert!(entries
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry["path"] == "hello.txt"));
    assert!(entries
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry["path"] == "-dash.txt"));

    let out = run(sqz()
        .args(["--lang", "en-US", "test"])
        .arg(&input)
        .arg("--json")
        .env("SQUALLZ_BSDTAR", &tool)
        .env("SQUALLZ_FAKE_BSDTAR_LOG", &log));
    assert!(out.status.success(), "test failed: {}", stderr(&out));
    let report: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    assert_eq!(report["ok"], true);
    assert_eq!(report["entries_tested"], 2);

    let dest = dir.join("extracted");
    let out = run(sqz()
        .args(["--lang", "en-US", "extract"])
        .arg(&input)
        .arg("-d")
        .arg(&dest)
        .env("SQUALLZ_BSDTAR", &tool)
        .env("SQUALLZ_FAKE_BSDTAR_LOG", &log));
    assert!(out.status.success(), "extract failed: {}", stderr(&out));
    assert_eq!(
        std::fs::read(dest.join("hello.txt")).unwrap(),
        b"hello from cli rar bridge"
    );
    assert_eq!(
        std::fs::read(dest.join("-dash.txt")).unwrap(),
        b"dash entry from cli rar bridge"
    );

    let log = std::fs::read_to_string(&log).unwrap();
    assert!(log.contains("-tf"), "{log}");
    assert!(log.contains("-xOf"), "{log}");
    assert!(log.contains("-- -dash.txt"), "{log}");

    std::fs::remove_dir_all(&dir).unwrap();
}

#[cfg(unix)]
#[test]
fn rar_bridge_prefers_7z_through_cli_when_tool_is_available() {
    use std::os::unix::fs::PermissionsExt;

    let dir = temp_dir("rar-bridge-7z-cli");
    let input = dir.join("sample.rar");
    let tool = dir.join("fake-7z.sh");
    let log = dir.join("fake-7z.log");
    std::fs::write(&input, RAR5_MAGIC).unwrap();

    let script = r#"#!/bin/sh
set -eu
printf '%s\n' "$*" >> "$SQUALLZ_FAKE_7Z_LOG"
if [ "$1" = "l" ] && [ "$2" = "-slt" ]; then
  cat <<'EOF'
Path = docs
Folder = +
Size = 0
Attributes = D

Path = hello.txt
Folder = -
Size = 24
Packed Size = 12
CRC = 1234ABCD
Encrypted = -

Path = -dash.txt
Folder = -
Size = 21
Packed Size = 9
Encrypted = -

EOF
  exit 0
fi
if [ "$1" = "x" ] && [ "$2" = "-so" ]; then
  last=""
  prev=""
  for arg in "$@"; do
    prev="$last"
    last="$arg"
  done
  if [ "$last" = "-dash.txt" ] && [ "$prev" != "--" ]; then
    printf 'missing -- before dash entry\n' >&2
    exit 9
  fi
  case "$last" in
    hello.txt) printf 'hello from cli rar via 7z' ;;
    -dash.txt) printf 'dash entry via 7z cli' ;;
    *) printf 'unknown entry: %s\n' "$last" >&2; exit 3 ;;
  esac
  exit 0
fi
printf 'unexpected args\n' >&2
exit 2
"#;
    std::fs::write(&tool, script).unwrap();
    let mut perms = std::fs::metadata(&tool).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&tool, perms).unwrap();

    let out = run(sqz()
        .args(["--lang", "en-US", "list"])
        .arg(&input)
        .arg("--json")
        .env_remove("SQUALLZ_BSDTAR")
        .env("SQUALLZ_7Z", &tool)
        .env("SQUALLZ_FAKE_7Z_LOG", &log));
    assert!(out.status.success(), "list failed: {}", stderr(&out));
    let entries = stdout_json(&out);
    assert!(entries
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry["path"] == "hello.txt" && entry["crc32"] == 0x1234ABCD));

    let out = run(sqz()
        .args(["--lang", "en-US", "test"])
        .arg(&input)
        .arg("--json")
        .env_remove("SQUALLZ_BSDTAR")
        .env("SQUALLZ_7Z", &tool)
        .env("SQUALLZ_FAKE_7Z_LOG", &log));
    assert!(out.status.success(), "test failed: {}", stderr(&out));
    let report = stdout_json(&out);
    assert_eq!(report["ok"], true);
    assert_eq!(report["entries_tested"], 2);

    let dest = dir.join("extracted");
    let out = run(sqz()
        .args(["--lang", "en-US", "extract"])
        .arg(&input)
        .arg("-d")
        .arg(&dest)
        .env_remove("SQUALLZ_BSDTAR")
        .env("SQUALLZ_7Z", &tool)
        .env("SQUALLZ_FAKE_7Z_LOG", &log));
    assert!(out.status.success(), "extract failed: {}", stderr(&out));
    assert_eq!(
        std::fs::read(dest.join("hello.txt")).unwrap(),
        b"hello from cli rar via 7z"
    );
    assert_eq!(
        std::fs::read(dest.join("-dash.txt")).unwrap(),
        b"dash entry via 7z cli"
    );

    let log = std::fs::read_to_string(&log).unwrap();
    assert!(log.contains("l -slt"), "{log}");
    assert!(log.contains("x -so"), "{log}");
    assert!(log.contains("-- -dash.txt"), "{log}");

    std::fs::remove_dir_all(&dir).unwrap();
}

#[cfg(unix)]
#[test]
fn rar_bridge_convert_to_zip_through_cli_when_tool_is_available() {
    use std::os::unix::fs::PermissionsExt;

    let dir = temp_dir("rar-bridge-convert-cli");
    let input = dir.join("sample.rar");
    let converted = dir.join("converted.zip");
    let tool = dir.join("fake-bsdtar.sh");
    std::fs::write(&input, RAR5_MAGIC).unwrap();

    let script = r#"#!/bin/sh
set -eu
if [ "$1" = "-tf" ]; then
  printf 'docs/\nhello.txt\n'
  exit 0
fi
if [ "$1" = "-tvf" ]; then
  printf 'drwxr-xr-x  0 0      0           0 Jan  1  2020 docs/\n'
  printf -- '-rw-r--r--  0 0      0          24 Jan  1  2020 hello.txt\n'
  exit 0
fi
if [ "$1" = "-xOf" ]; then
  last=""
  for arg in "$@"; do
    last="$arg"
  done
  case "$last" in
    hello.txt) printf 'hello from converted rar' ;;
    *) printf 'unknown entry: %s\n' "$last" >&2; exit 3 ;;
  esac
  exit 0
fi
printf 'unexpected args\n' >&2
exit 2
"#;
    std::fs::write(&tool, script).unwrap();
    let mut perms = std::fs::metadata(&tool).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&tool, perms).unwrap();

    let out = run(sqz()
        .args(["--lang", "en-US", "convert"])
        .arg(&input)
        .arg("-o")
        .arg(&converted)
        .env("SQUALLZ_BSDTAR", &tool));
    assert!(out.status.success(), "convert failed: {}", stderr(&out));
    assert!(converted.is_file(), "converted ZIP missing");

    let out = run(sqz().arg("list").arg(&converted).arg("--json"));
    assert!(out.status.success(), "list failed: {}", stderr(&out));
    let entries: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    assert!(entries
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry["path"] == "hello.txt"));

    let dest = dir.join("out");
    let out = run(sqz().arg("extract").arg(&converted).arg("-d").arg(&dest));
    assert!(out.status.success(), "extract failed: {}", stderr(&out));
    assert_eq!(
        std::fs::read(dest.join("hello.txt")).unwrap(),
        b"hello from converted rar"
    );

    std::fs::remove_dir_all(&dir).unwrap();
}

#[cfg(unix)]
#[test]
fn rar_bridge_password_like_entry_failure_is_reported_through_cli() {
    use std::os::unix::fs::PermissionsExt;

    let dir = temp_dir("rar-bridge-password-boundary-cli");
    let input = dir.join("protected.rar");
    let tool = dir.join("fake-bsdtar.sh");
    let dest = dir.join("readable");
    std::fs::write(&input, RAR5_MAGIC).unwrap();

    let script = r#"#!/bin/sh
set -eu
if [ "$1" = "-tf" ]; then
  printf 'public.txt\nsecret.txt\n'
  exit 0
fi
if [ "$1" = "-tvf" ]; then
  printf -- '-rw-r--r--  0 0      0          16 Jan  1  2020 public.txt\n'
  printf -- '-rw-r--r--  0 0      0           0 Jan  1  2020 secret.txt\n'
  exit 0
fi
if [ "$1" = "-xOf" ]; then
  last=""
  for arg in "$@"; do
    last="$arg"
  done
  case "$last" in
    public.txt) printf 'public rar bytes' ;;
    secret.txt) printf 'Passphrase required for this file\n' >&2; exit 6 ;;
    *) printf 'unknown entry: %s\n' "$last" >&2; exit 3 ;;
  esac
  exit 0
fi
printf 'unexpected args\n' >&2
exit 2
"#;
    std::fs::write(&tool, script).unwrap();
    let mut perms = std::fs::metadata(&tool).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&tool, perms).unwrap();

    let out = run(sqz()
        .args(["--lang", "en-US", "test"])
        .arg(&input)
        .arg("--json")
        .env("SQUALLZ_BSDTAR", &tool));
    assert_eq!(out.status.code(), Some(3), "stderr: {}", stderr(&out));
    let report: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    assert_eq!(report["ok"], false);
    assert_eq!(report["entries_tested"], 2);
    assert!(
        report["problems"]
            .as_array()
            .unwrap()
            .iter()
            .any(|problem| problem
                .as_str()
                .is_some_and(|text| text.contains("secret.txt"))),
        "report: {report}"
    );

    let out = run(sqz()
        .args(["--lang", "en-US", "extract"])
        .arg(&input)
        .arg("-d")
        .arg(&dest)
        .args(["--best-effort", "--json"])
        .env("SQUALLZ_BSDTAR", &tool));
    assert!(out.status.success(), "extract failed: {}", stderr(&out));
    let report: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    assert_eq!(report["ok"], true);
    assert_eq!(report["operation"], "extract");
    assert_eq!(report["best_effort"], true);
    assert_eq!(report["skipped"], 1);
    assert!(
        report["problems"]
            .as_array()
            .unwrap()
            .iter()
            .any(|problem| problem
                .as_str()
                .is_some_and(|text| text.contains("secret.txt"))),
        "report: {report}"
    );
    assert_eq!(
        std::fs::read(dest.join("public.txt")).unwrap(),
        b"public rar bytes"
    );
    assert!(
        !dest.join("secret.txt").exists(),
        "failed RAR entry must not leave a partial best-effort output"
    );

    std::fs::remove_dir_all(&dir).unwrap();
}

#[cfg(unix)]
#[test]
fn rar_bridge_extract_rejects_path_traversal_through_cli() {
    use std::os::unix::fs::PermissionsExt;

    let dir = temp_dir("rar-bridge-traversal-cli");
    let input = dir.join("sample.rar");
    let tool = dir.join("fake-bsdtar.sh");
    let dest = dir.join("extract");
    let outside = dir.join("evil.txt");
    std::fs::write(&input, RAR5_MAGIC).unwrap();

    let script = r#"#!/bin/sh
set -eu
if [ "$1" = "-tf" ]; then
  printf '../evil.txt\n'
  exit 0
fi
if [ "$1" = "-tvf" ]; then
  printf -- '-rw-r--r--  0 0      0          12 Jan  1  2020 ../evil.txt\n'
  exit 0
fi
if [ "$1" = "-xOf" ]; then
  printf 'evil payload'
  exit 0
fi
printf 'unexpected args\n' >&2
exit 2
"#;
    std::fs::write(&tool, script).unwrap();
    let mut perms = std::fs::metadata(&tool).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&tool, perms).unwrap();

    let out = run(sqz()
        .args(["--lang", "en-US", "extract"])
        .arg(&input)
        .arg("-d")
        .arg(&dest)
        .env("SQUALLZ_BSDTAR", &tool));
    assert_eq!(out.status.code(), Some(6), "stderr: {}", stderr(&out));
    assert!(
        stderr(&out).contains("Path traversal") || stderr(&out).contains("unsafe path"),
        "stderr: {}",
        stderr(&out)
    );
    assert!(
        !outside.exists(),
        "RAR bridge path traversal must not write outside extraction root"
    );

    std::fs::remove_dir_all(&dir).unwrap();
}

/// Writes ~100 KB of incompressible data for the split tests.
fn incompressible_file(dir: &Path, name: &str) -> PathBuf {
    incompressible_file_with_len(dir, name, 100 * 1024)
}

fn incompressible_file_with_len(dir: &Path, name: &str, len: usize) -> PathBuf {
    let mut state = 0x9E37_79B9u32;
    let data: Vec<u8> = (0..len)
        .map(|_| {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (state >> 24) as u8
        })
        .collect();
    let path = dir.join(name);
    std::fs::write(&path, data).unwrap();
    path
}

fn numbered_volume_paths(dir: &Path, prefix: &str) -> Vec<PathBuf> {
    let mut paths: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .and_then(|name| name.strip_prefix(prefix))
                .is_some_and(|suffix| suffix.chars().all(|ch| ch.is_ascii_digit()))
        })
        .collect();
    paths.sort();
    paths
}

#[test]
fn compress_split_produces_volumes_and_reads_back_transparently() {
    let dir = temp_dir("split-cli");
    let input = incompressible_file(&dir, "data.bin");
    let archive = dir.join("out.zip");

    // zh-CN split message with the volume count.
    let out = run(sqz()
        .args(["--lang", "zh-CN", "compress"])
        .arg(&input)
        .arg("-o")
        .arg(&archive)
        .args(["--split", "30k"]));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(
        stdout(&out).contains("已创建") && stdout(&out).contains("共 4 卷"),
        "stdout: {}",
        stdout(&out)
    );
    assert!(!archive.exists(), "unsplit output must not remain");
    for i in 1..=4 {
        assert!(dir.join(format!("out.zip.{i:03}")).is_file(), "volume {i}");
    }

    let json_archive = dir.join("out-json.zip");
    let out = run(sqz()
        .arg("compress")
        .arg(&input)
        .arg("-o")
        .arg(&json_archive)
        .args(["--split", "30k"])
        .arg("--json"));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let report: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    assert_eq!(report["ok"], true);
    assert_eq!(report["operation"], "compress");
    assert_eq!(
        report["output"],
        dir.join("out-json.zip.001").display().to_string()
    );
    assert_eq!(report["split"], true);
    assert_eq!(report["volumes"], 4);

    // list/test/extract operate on the first volume transparently.
    let first = dir.join("out.zip.001");
    let out = run(sqz().arg("list").arg(&first).arg("--json"));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let out = run(sqz().arg("test").arg(&first).arg("--json"));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let dest = dir.join("restored");
    let out = run(sqz().arg("extract").arg(&first).arg("-d").arg(&dest));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(
        std::fs::read(dest.join("data.bin")).unwrap(),
        std::fs::read(&input).unwrap()
    );

    // Removing a middle volume reports a corrupt archive (exit code 3)
    // naming the missing volume.
    std::fs::remove_file(dir.join("out.zip.002")).unwrap();
    let out = run(sqz().args(["--lang", "en-US", "list"]).arg(&first));
    assert_eq!(out.status.code(), Some(3), "stderr: {}", stderr(&out));
    assert!(
        stderr(&out).contains("out.zip.002"),
        "stderr: {}",
        stderr(&out)
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn sqz_split_missing_volume_recovers_through_cli() {
    let dir = temp_dir("sqz-split-recover-cli");
    let input = incompressible_file(&dir, "data.bin");
    let archive = dir.join("out.sqz");

    let out = run(sqz()
        .args(["--lang", "en-US", "pack"])
        .arg(&input)
        .arg("-o")
        .arg(&archive)
        .args([
            "--inner-format",
            "sqz",
            "--recovery",
            "10%",
            "--split",
            "30k",
        ]));
    assert!(out.status.success(), "pack failed: {}", stderr(&out));
    assert!(stdout(&out).contains("Created"), "stdout: {}", stdout(&out));
    assert!(!archive.exists(), "unsplit output must not remain");
    assert!(dir.join("out.sqz.001").is_file());
    assert!(dir.join("out.sqz.002").is_file());
    assert!(dir.join("out.sqz.rev001").is_file());

    std::fs::remove_file(dir.join("out.sqz.002")).unwrap();
    let first = dir.join("out.sqz.001");

    let out = run(sqz().arg("list").arg(&first).arg("--json"));
    assert!(out.status.success(), "list failed: {}", stderr(&out));
    let entries: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    let paths: Vec<&str> = entries
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["path"].as_str().unwrap())
        .collect();
    assert!(paths.contains(&"data.bin"), "paths: {paths:?}");

    let out = run(sqz().arg("test").arg(&first).arg("--json"));
    assert!(out.status.success(), "test failed: {}", stderr(&out));
    let report: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    assert_eq!(report["ok"], true);
    assert_eq!(report["problems"].as_array().unwrap().len(), 0);

    let dest = dir.join("restored");
    let out = run(sqz().arg("extract").arg(&first).arg("-d").arg(&dest));
    assert!(out.status.success(), "extract failed: {}", stderr(&out));
    assert_eq!(
        std::fs::read(dest.join("data.bin")).unwrap(),
        std::fs::read(&input).unwrap()
    );

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn sqz_split_missing_tail_volume_recovers_through_cli() {
    let dir = temp_dir("sqz-split-tail-recover-cli");
    let input = incompressible_file(&dir, "data.bin");
    let archive = dir.join("out.sqz");

    let out = run(sqz()
        .args(["--lang", "en-US", "pack"])
        .arg(&input)
        .arg("-o")
        .arg(&archive)
        .args([
            "--inner-format",
            "sqz",
            "--recovery",
            "10%",
            "--split",
            "30k",
        ]));
    assert!(out.status.success(), "pack failed: {}", stderr(&out));
    let volumes = numbered_volume_paths(&dir, "out.sqz.");
    assert!(volumes.len() >= 3, "volumes: {volumes:?}");
    let tail = volumes.last().unwrap().clone();
    let tail_index = volumes.len();
    let tail_mirror = dir.join(format!("out.sqz.rev{tail_index:03}"));
    assert!(tail_mirror.is_file(), "missing {}", tail_mirror.display());
    assert!(dir.join("out.sqz.rev001").is_file());

    std::fs::remove_file(tail).unwrap();
    std::fs::remove_file(dir.join("out.sqz.rev001")).unwrap();
    let first = dir.join("out.sqz.001");

    let out = run(sqz().arg("list").arg(&first).arg("--json"));
    assert!(out.status.success(), "list failed: {}", stderr(&out));
    let entries: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    assert_eq!(entries.as_array().unwrap()[0]["path"], "data.bin");

    let out = run(sqz().arg("test").arg(&first).arg("--json"));
    assert!(out.status.success(), "test failed: {}", stderr(&out));
    let report: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    assert_eq!(report["ok"], true);
    assert_eq!(report["problems"].as_array().unwrap().len(), 0);

    let dest = dir.join("tail-restored");
    let out = run(sqz().arg("extract").arg(&first).arg("-d").arg(&dest));
    assert!(out.status.success(), "extract failed: {}", stderr(&out));
    assert_eq!(
        std::fs::read(dest.join("data.bin")).unwrap(),
        std::fs::read(&input).unwrap()
    );

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn sqz_split_missing_payload_and_tail_recovers_through_cli() {
    let dir = temp_dir("sqz-split-payload-tail-recover-cli");
    let input = incompressible_file(&dir, "data.bin");
    let archive = dir.join("out.sqz");

    let out = run(sqz()
        .args(["--lang", "en-US", "pack"])
        .arg(&input)
        .arg("-o")
        .arg(&archive)
        .args([
            "--inner-format",
            "sqz",
            "--recovery",
            "10%",
            "--split",
            "30k",
        ]));
    assert!(out.status.success(), "pack failed: {}", stderr(&out));
    let volumes = numbered_volume_paths(&dir, "out.sqz.");
    assert!(volumes.len() >= 4, "volumes: {volumes:?}");
    let tail = volumes.last().unwrap().clone();
    let tail_index = volumes.len();
    let tail_mirror = dir.join(format!("out.sqz.rev{tail_index:03}"));
    assert!(tail_mirror.is_file(), "missing {}", tail_mirror.display());
    assert!(dir.join("out.sqz.rev001").is_file());

    std::fs::remove_file(dir.join("out.sqz.002")).unwrap();
    std::fs::remove_file(tail).unwrap();
    let first = dir.join("out.sqz.001");

    let out = run(sqz().arg("list").arg(&first).arg("--json"));
    assert!(out.status.success(), "list failed: {}", stderr(&out));
    let entries: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    assert_eq!(entries.as_array().unwrap()[0]["path"], "data.bin");

    let out = run(sqz().arg("test").arg(&first).arg("--json"));
    assert!(out.status.success(), "test failed: {}", stderr(&out));
    let report: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    assert_eq!(report["ok"], true);
    assert_eq!(report["problems"].as_array().unwrap().len(), 0);

    let dest = dir.join("payload-tail-restored");
    let out = run(sqz().arg("extract").arg(&first).arg("-d").arg(&dest));
    assert!(out.status.success(), "extract failed: {}", stderr(&out));
    assert_eq!(
        std::fs::read(dest.join("data.bin")).unwrap(),
        std::fs::read(&input).unwrap()
    );

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn sqz_split_two_missing_volumes_recover_through_cli() {
    let dir = temp_dir("sqz-split-two-missing-recover-cli");
    let input = incompressible_file(&dir, "data.bin");
    let archive = dir.join("out.sqz");

    let out = run(sqz().arg("pack").arg(&input).arg("-o").arg(&archive).args([
        "--inner-format",
        "sqz",
        "--recovery",
        "10%",
        "--split",
        "30k",
    ]));
    assert!(out.status.success(), "pack failed: {}", stderr(&out));
    assert!(dir.join("out.sqz.rev001").is_file());
    assert!(dir.join("out.sqz.rev002").is_file());
    assert!(dir.join("out.sqz.002").is_file());
    assert!(dir.join("out.sqz.003").is_file());

    std::fs::remove_file(dir.join("out.sqz.002")).unwrap();
    std::fs::remove_file(dir.join("out.sqz.003")).unwrap();
    let first = dir.join("out.sqz.001");

    let out = run(sqz().arg("list").arg(&first).arg("--json"));
    assert!(out.status.success(), "list failed: {}", stderr(&out));
    let entries: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    assert_eq!(entries.as_array().unwrap()[0]["path"], "data.bin");

    let out = run(sqz().arg("test").arg(&first).arg("--json"));
    assert!(out.status.success(), "test failed: {}", stderr(&out));
    let report: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    assert_eq!(report["ok"], true);
    assert_eq!(report["problems"].as_array().unwrap().len(), 0);

    let dest = dir.join("two-missing-restored");
    let out = run(sqz().arg("extract").arg(&first).arg("-d").arg(&dest));
    assert!(out.status.success(), "extract failed: {}", stderr(&out));
    assert_eq!(
        std::fs::read(dest.join("data.bin")).unwrap(),
        std::fs::read(&input).unwrap()
    );

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn sqz_split_three_missing_volumes_recover_through_cli() {
    let dir = temp_dir("sqz-split-three-missing-recover-cli");
    let input = incompressible_file_with_len(&dir, "data.bin", 900 * 1024);
    let archive = dir.join("out.sqz");

    let out = run(sqz().arg("pack").arg(&input).arg("-o").arg(&archive).args([
        "--inner-format",
        "sqz",
        "--recovery",
        "10%",
        "--split",
        "180k",
    ]));
    assert!(out.status.success(), "pack failed: {}", stderr(&out));
    assert!(dir.join("out.sqz.rev001").is_file());
    assert!(dir.join("out.sqz.rev002").is_file());
    assert!(dir.join("out.sqz.rev003").is_file());
    assert!(dir.join("out.sqz.002").is_file());
    assert!(dir.join("out.sqz.003").is_file());
    assert!(dir.join("out.sqz.004").is_file());

    std::fs::remove_file(dir.join("out.sqz.002")).unwrap();
    std::fs::remove_file(dir.join("out.sqz.003")).unwrap();
    std::fs::remove_file(dir.join("out.sqz.004")).unwrap();
    let first = dir.join("out.sqz.001");

    let out = run(sqz().arg("list").arg(&first).arg("--json"));
    assert!(out.status.success(), "list failed: {}", stderr(&out));
    let entries: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    assert_eq!(entries.as_array().unwrap()[0]["path"], "data.bin");

    let out = run(sqz().arg("test").arg(&first).arg("--json"));
    assert!(out.status.success(), "test failed: {}", stderr(&out));
    let report: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    assert_eq!(report["ok"], true);
    assert_eq!(report["problems"].as_array().unwrap().len(), 0);

    let dest = dir.join("three-missing-restored");
    let out = run(sqz().arg("extract").arg(&first).arg("-d").arg(&dest));
    assert!(out.status.success(), "extract failed: {}", stderr(&out));
    assert_eq!(
        std::fs::read(dest.join("data.bin")).unwrap(),
        std::fs::read(&input).unwrap()
    );

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn sqz_split_three_missing_volumes_fail_without_triple_parity_through_cli() {
    let dir = temp_dir("sqz-split-three-missing-no-triple-cli");
    let input = incompressible_file_with_len(&dir, "data.bin", 900 * 1024);
    let archive = dir.join("out.sqz");

    let out = run(sqz().arg("pack").arg(&input).arg("-o").arg(&archive).args([
        "--inner-format",
        "sqz",
        "--recovery",
        "10%",
        "--split",
        "180k",
    ]));
    assert!(out.status.success(), "pack failed: {}", stderr(&out));
    assert!(dir.join("out.sqz.rev001").is_file());
    assert!(dir.join("out.sqz.rev002").is_file());
    assert!(dir.join("out.sqz.rev003").is_file());
    assert!(dir.join("out.sqz.002").is_file());
    assert!(dir.join("out.sqz.003").is_file());
    assert!(dir.join("out.sqz.004").is_file());

    std::fs::remove_file(dir.join("out.sqz.002")).unwrap();
    std::fs::remove_file(dir.join("out.sqz.003")).unwrap();
    std::fs::remove_file(dir.join("out.sqz.004")).unwrap();
    std::fs::remove_file(dir.join("out.sqz.rev003")).unwrap();
    let first = dir.join("out.sqz.001");

    let out = run(sqz().arg("test").arg(&first).arg("--json"));
    assert_eq!(out.status.code(), Some(3), "stderr: {}", stderr(&out));
    let report: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    assert_eq!(report["ok"], false);
    assert!(report["problems"]
        .as_array()
        .unwrap()
        .iter()
        .any(|problem| problem
            .as_str()
            .is_some_and(|text| text.contains("unrepaired SQZ recovery block damage"))));

    let out = run(sqz()
        .args(["--lang", "en-US", "extract"])
        .arg(&first)
        .arg("-d")
        .arg(dir.join("three-missing-out")));
    assert_eq!(out.status.code(), Some(3), "stderr: {}", stderr(&out));
    assert!(
        stderr(&out).contains("unrepaired") || stderr(&out).contains("Corrupt archive"),
        "stderr: {}",
        stderr(&out)
    );
    assert!(
        !stdout(&out).contains("Extracted to"),
        "stdout: {}",
        stdout(&out)
    );

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn sqz_split_four_missing_volumes_fail_through_cli() {
    let dir = temp_dir("sqz-split-four-missing-cli");
    let input = incompressible_file_with_len(&dir, "data.bin", 1_200 * 1024);
    let archive = dir.join("out.sqz");

    let out = run(sqz().arg("pack").arg(&input).arg("-o").arg(&archive).args([
        "--inner-format",
        "sqz",
        "--recovery",
        "10%",
        "--split",
        "180k",
    ]));
    assert!(out.status.success(), "pack failed: {}", stderr(&out));
    assert!(dir.join("out.sqz.rev001").is_file());
    assert!(dir.join("out.sqz.rev002").is_file());
    assert!(dir.join("out.sqz.rev003").is_file());
    for index in 2..=5 {
        assert!(dir.join(format!("out.sqz.{index:03}")).is_file());
        std::fs::remove_file(dir.join(format!("out.sqz.{index:03}"))).unwrap();
    }
    let first = dir.join("out.sqz.001");

    let out = run(sqz().arg("test").arg(&first).arg("--json"));
    assert_eq!(out.status.code(), Some(3), "stderr: {}", stderr(&out));
    let report: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    assert_eq!(report["ok"], false);
    assert!(
        report["problems"]
            .as_array()
            .unwrap()
            .iter()
            .any(|problem| problem
                .as_str()
                .is_some_and(|text| text.contains("unrepaired SQZ recovery block damage"))),
        "report: {report}"
    );

    let out = run(sqz()
        .args(["--lang", "en-US", "extract"])
        .arg(&first)
        .arg("-d")
        .arg(dir.join("four-missing-out")));
    assert_eq!(out.status.code(), Some(3), "stderr: {}", stderr(&out));
    assert!(
        stderr(&out).contains("unrepaired") || stderr(&out).contains("Corrupt archive"),
        "stderr: {}",
        stderr(&out)
    );

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn sqz_split_two_missing_volumes_fail_without_dual_parity_through_cli() {
    let dir = temp_dir("sqz-split-two-missing-no-dual-cli");
    let input = incompressible_file(&dir, "data.bin");
    let archive = dir.join("out.sqz");

    let out = run(sqz().arg("pack").arg(&input).arg("-o").arg(&archive).args([
        "--inner-format",
        "sqz",
        "--recovery",
        "10%",
        "--split",
        "30k",
    ]));
    assert!(out.status.success(), "pack failed: {}", stderr(&out));
    assert!(dir.join("out.sqz.rev001").is_file());
    assert!(dir.join("out.sqz.rev002").is_file());
    assert!(dir.join("out.sqz.002").is_file());
    assert!(dir.join("out.sqz.003").is_file());

    std::fs::remove_file(dir.join("out.sqz.002")).unwrap();
    std::fs::remove_file(dir.join("out.sqz.003")).unwrap();
    std::fs::remove_file(dir.join("out.sqz.rev002")).unwrap();
    let first = dir.join("out.sqz.001");

    let out = run(sqz().arg("test").arg(&first).arg("--json"));
    assert_eq!(out.status.code(), Some(3), "stderr: {}", stderr(&out));
    let report: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    assert_eq!(report["ok"], false);
    assert!(report["problems"]
        .as_array()
        .unwrap()
        .iter()
        .any(|problem| problem
            .as_str()
            .is_some_and(|text| text.contains("unrepaired SQZ recovery block damage"))));

    let out = run(sqz()
        .args(["--lang", "en-US", "extract"])
        .arg(&first)
        .arg("-d")
        .arg(dir.join("two-missing-out")));
    assert_eq!(out.status.code(), Some(3), "stderr: {}", stderr(&out));
    assert!(
        stderr(&out).contains("unrepaired") || stderr(&out).contains("Corrupt archive"),
        "stderr: {}",
        stderr(&out)
    );
    assert!(
        !stdout(&out).contains("Extracted to"),
        "stdout: {}",
        stdout(&out)
    );

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn sqz_over_limit_payload_damage_fails_through_cli() {
    let dir = temp_dir("sqz-over-limit-cli");
    let input = dir.join("large.bin");
    std::fs::write(&input, sqz_recovery_payload(8)).unwrap();
    let archive = dir.join("damaged.sqz");

    let out = run(sqz().arg("pack").arg(&input).arg("-o").arg(&archive).args([
        "--inner-format",
        "sqz",
        "--recovery",
        "25%",
    ]));
    assert!(out.status.success(), "pack failed: {}", stderr(&out));

    corrupt_sqz_marked_payload_blocks(&archive, &[0, 1, 2]);

    let out = run(sqz().arg("test").arg(&archive).arg("--json"));
    assert_eq!(out.status.code(), Some(3), "stderr: {}", stderr(&out));
    let report: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    assert_eq!(report["ok"], false);
    assert!(report["entries_tested"].as_u64().unwrap() >= 1);
    assert_eq!(report["recovery"]["scheme"], "sqz-embedded-rs-gf8");
    assert_eq!(report["recovery"]["damaged_blocks"], 3);
    assert_eq!(report["recovery"]["repaired_blocks"], 0);
    assert_eq!(report["recovery"]["unrepaired_blocks"], 3);
    assert_eq!(report["recovery"]["repair_possible"], false);
    assert_eq!(report["recovery"]["parity_shards"], 2);
    assert!(
        report["recovery"]["recovery_blocks_available"]
            .as_u64()
            .unwrap()
            >= 2
    );
    assert!(
        report["problems"]
            .as_array()
            .unwrap()
            .iter()
            .any(|problem| problem
                .as_str()
                .is_some_and(|text| text.contains("unrepaired SQZ recovery block damage"))),
        "report: {report}"
    );

    let out = run(sqz()
        .args(["--lang", "en-US", "extract"])
        .arg(&archive)
        .arg("-d")
        .arg(dir.join("strict-out")));
    assert_eq!(out.status.code(), Some(3), "stderr: {}", stderr(&out));
    assert!(
        stderr(&out).contains("SQZ recovery")
            || stderr(&out).contains("Corrupt archive")
            || stderr(&out).contains("corrupt"),
        "stderr: {}",
        stderr(&out)
    );
    assert!(
        !stdout(&out).contains("Extracted to"),
        "stdout: {}",
        stdout(&out)
    );

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn extract_smart_wraps_loose_entries_and_keeps_single_root() {
    let dir = temp_dir("smart-cli");

    // Loose files → wrapped into a folder named after the archive.
    let loose = dir.join("loose");
    std::fs::create_dir_all(&loose).unwrap();
    std::fs::write(loose.join("a.txt"), b"a").unwrap();
    std::fs::write(loose.join("b.txt"), b"b").unwrap();
    let archive = dir.join("bundle.zip");
    run(sqz()
        .arg("compress")
        .arg(loose.join("a.txt"))
        .arg(loose.join("b.txt"))
        .arg("-o")
        .arg(&archive));
    let dest = dir.join("d1");
    let out = run(sqz()
        .args(["--lang", "zh-CN", "extract"])
        .arg(&archive)
        .arg("-d")
        .arg(&dest)
        .arg("--smart"));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(
        stderr(&out).contains("智能解压"),
        "stderr: {}",
        stderr(&out)
    );
    assert!(dest.join("bundle/a.txt").is_file());
    assert!(dest.join("bundle/b.txt").is_file());

    // Single root directory → extracted directly (no extra folder), with
    // the English notice.
    let root = sample_tree(&dir);
    let archive2 = dir.join("rooted.zip");
    run(sqz().arg("compress").arg(&root).arg("-o").arg(&archive2));
    let dest2 = dir.join("d2");
    let out = run(sqz()
        .args(["--lang", "en-US", "extract"])
        .arg(&archive2)
        .arg("-d")
        .arg(&dest2)
        .arg("--smart"));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(
        stderr(&out).contains("Smart extract"),
        "stderr: {}",
        stderr(&out)
    );
    assert!(dest2.join("project/a.txt").is_file());
    assert!(!dest2.join("rooted").exists());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn convert_zip_to_7z_to_zip_roundtrip() {
    let dir = temp_dir("convert-cli");
    let root = sample_tree(&dir);
    let zip = dir.join("src.zip");
    run(sqz().arg("compress").arg(&root).arg("-o").arg(&zip));

    // zip → 7z (zh-CN message).
    let sevenz = dir.join("mid.7z");
    let out = run(sqz()
        .args(["--lang", "zh-CN", "convert"])
        .arg(&zip)
        .arg("-o")
        .arg(&sevenz)
        .args(["--threads", "2"]));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(
        stdout(&out).contains("已转换为"),
        "stdout: {}",
        stdout(&out)
    );

    // 7z → zip (en-US message).
    let back = dir.join("back.zip");
    let out = run(sqz()
        .args(["--lang", "en-US", "convert"])
        .arg(&sevenz)
        .arg("-o")
        .arg(&back)
        .arg("--json"));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let report: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    assert_eq!(report["ok"], true);
    assert_eq!(report["operation"], "convert");
    assert_eq!(report["source"], sevenz.display().to_string());
    assert_eq!(report["output"], back.display().to_string());

    // Round-tripped archive extracts to identical content.
    let dest = dir.join("restored");
    run(sqz().arg("extract").arg(&back).arg("-d").arg(&dest));
    assert_eq!(
        std::fs::read(dest.join("project/a.txt")).unwrap(),
        b"hello world"
    );
    assert_eq!(
        std::fs::read(dest.join("project/sub/b.txt")).unwrap(),
        b"nested content"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn cli_encrypt_names_hides_7z_header_until_password_is_supplied() {
    let dir = temp_dir("cli-7z-header");
    let root = sample_tree(&dir);
    let zip = dir.join("src.zip");
    let out = run(sqz().arg("compress").arg(&root).arg("-o").arg(&zip));
    assert!(
        out.status.success(),
        "zip compress failed: {}",
        stderr(&out)
    );

    let hidden = dir.join("hidden.7z");
    let out = run(sqz().arg("convert").arg(&zip).arg("-o").arg(&hidden).args([
        "--out-password",
        "hidden names",
        "--encrypt-names",
        "--threads",
        "2",
    ]));
    assert!(out.status.success(), "convert failed: {}", stderr(&out));

    let out = run(sqz().arg("list").arg(&hidden));
    assert_eq!(out.status.code(), Some(4), "stderr: {}", stderr(&out));

    let out = run(sqz()
        .arg("list")
        .arg(&hidden)
        .args(["--password", "hidden names", "--json"]));
    assert!(
        out.status.success(),
        "list with password failed: {}",
        stderr(&out)
    );
    let entries: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    assert!(entries
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["path"] == "project/a.txt"));

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn export_sqz_to_standard_zip_roundtrip() {
    let dir = temp_dir("export-sqz");
    let root = sample_tree(&dir);
    let sqz_archive = dir.join("source.sqz");
    let out = run(sqz().arg("compress").arg(&root).arg("-o").arg(&sqz_archive));
    assert!(out.status.success(), "compress failed: {}", stderr(&out));

    let exported = dir.join("exported.zip");
    let out = run(sqz()
        .args(["--lang", "en-US", "export"])
        .arg(&sqz_archive)
        .arg("-o")
        .arg(&exported)
        .arg("--json"));
    assert!(out.status.success(), "export failed: {}", stderr(&out));
    let report: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    assert_eq!(report["ok"], true);
    assert_eq!(report["operation"], "export_sqz");
    assert_eq!(report["archive"], sqz_archive.display().to_string());
    assert_eq!(report["output"], exported.display().to_string());

    let out = run(sqz().arg("list").arg(&exported).arg("--json"));
    assert!(out.status.success(), "list failed: {}", stderr(&out));
    let entries: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    let paths: Vec<&str> = entries
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["path"].as_str().unwrap())
        .collect();
    assert!(paths.contains(&"project/a.txt"));
    assert!(paths.contains(&"project/sub/b.txt"));

    let dest = dir.join("exported-files");
    let out = run(sqz().arg("extract").arg(&exported).arg("-d").arg(&dest));
    assert!(out.status.success(), "extract failed: {}", stderr(&out));
    assert_eq!(
        std::fs::read(dest.join("project/a.txt")).unwrap(),
        b"hello world"
    );
    assert_eq!(
        std::fs::read(dest.join("project/sub/b.txt")).unwrap(),
        b"nested content"
    );

    if let Ok(out) = Command::new("unzip")
        .args(["-t", "-qq"])
        .arg(&exported)
        .output()
    {
        assert!(
            out.status.success(),
            "system unzip -t failed: {}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }

    let zip_source = dir.join("not-sqz.zip");
    let out = run(sqz().arg("compress").arg(&root).arg("-o").arg(&zip_source));
    assert!(
        out.status.success(),
        "compress zip failed: {}",
        stderr(&out)
    );
    let out = run(sqz()
        .arg("export")
        .arg(&zip_source)
        .arg("-o")
        .arg(dir.join("wrong.zip")));
    assert_eq!(out.status.code(), Some(2), "stderr: {}", stderr(&out));
    assert!(
        stderr(&out).contains("export expects a .sqz source container"),
        "stderr: {}",
        stderr(&out)
    );

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn export_and_repair_accept_split_sqz_first_volume_source() {
    let dir = temp_dir("split-sqz-export-repair");
    let input = incompressible_file(&dir, "data.bin");
    let archive = dir.join("source.sqz");
    let out = run(sqz().arg("pack").arg(&input).arg("-o").arg(&archive).args([
        "--inner-format",
        "sqz",
        "--recovery",
        "10%",
        "--split",
        "30k",
    ]));
    assert!(out.status.success(), "pack failed: {}", stderr(&out));
    assert!(!archive.exists(), "unsplit output must not remain");
    assert!(dir.join("source.sqz.001").is_file());
    assert!(dir.join("source.sqz.002").is_file());
    assert!(dir.join("source.sqz.rev001").is_file());
    std::fs::remove_file(dir.join("source.sqz.002")).unwrap();
    let first = dir.join("source.sqz.001");

    let exported = dir.join("exported.zip");
    let out = run(sqz().arg("export").arg(&first).arg("-o").arg(&exported));
    assert!(out.status.success(), "export failed: {}", stderr(&out));
    let dest = dir.join("exported-files");
    let out = run(sqz().arg("extract").arg(&exported).arg("-d").arg(&dest));
    assert!(out.status.success(), "extract failed: {}", stderr(&out));
    assert_eq!(
        std::fs::read(dest.join("data.bin")).unwrap(),
        std::fs::read(&input).unwrap()
    );

    let out = run(sqz().arg("repair").arg(&first).arg("--json"));
    assert_json_error(
        &out,
        2,
        "unsupported",
        ".sqz split-volume repair requires --output",
    );

    let repaired = dir.join("repaired.sqz");
    let out = run(sqz()
        .arg("repair")
        .arg(&first)
        .arg("-o")
        .arg(&repaired)
        .arg("--json"));
    assert!(out.status.success(), "repair failed: {}", stderr(&out));
    let report: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    assert_eq!(report["ok"], true);
    assert_eq!(report["operation"], "repair_sqz");
    assert_eq!(report["in_place"], false);
    assert_eq!(report["recovery"]["scheme"], "sqz-embedded-rs-gf8");
    assert_eq!(report["recovery"]["repair_possible"], true);
    assert_eq!(report["recovery"]["unrepaired_blocks"], 0);
    let out = run(sqz().arg("test").arg(&repaired).arg("--json"));
    assert!(
        out.status.success(),
        "test repaired failed: {}",
        stderr(&out)
    );
    let repaired_dest = dir.join("repaired-files");
    let out = run(sqz()
        .arg("extract")
        .arg(&repaired)
        .arg("-d")
        .arg(&repaired_dest));
    assert!(
        out.status.success(),
        "extract repaired failed: {}",
        stderr(&out)
    );
    assert_eq!(
        std::fs::read(repaired_dest.join("data.bin")).unwrap(),
        std::fs::read(&input).unwrap()
    );

    let bad_output = dir.join("bad.sqz.001");
    let out = run(sqz().arg("repair").arg(&first).arg("-o").arg(&bad_output));
    assert_eq!(out.status.code(), Some(2), "stderr: {}", stderr(&out));
    assert!(
        stderr(&out).contains("SQZ repair output must be a .sqz container"),
        "stderr: {}",
        stderr(&out)
    );
    assert!(
        !bad_output.exists(),
        "rejected split output should not be created"
    );

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn repair_sqz_rewrites_recovered_container() {
    let dir = temp_dir("repair-sqz");
    let root = sample_tree(&dir);
    let damaged = dir.join("damaged.sqz");
    let out = run(sqz().arg("compress").arg(&root).arg("-o").arg(&damaged));
    assert!(out.status.success(), "compress failed: {}", stderr(&out));

    corrupt_sqz_payload_byte(&damaged);

    let out = run(sqz().arg("test").arg(&damaged).arg("--json"));
    assert!(
        out.status.success(),
        "test damaged failed: {}",
        stderr(&out)
    );
    let report: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    assert_eq!(report["ok"], true);
    assert_eq!(report["recovery"]["scheme"], "sqz-embedded-rs-gf8");
    assert_eq!(report["recovery"]["damaged_blocks"], 1);
    assert_eq!(report["recovery"]["repaired_blocks"], 1);
    assert_eq!(report["recovery"]["unrepaired_blocks"], 0);
    assert_eq!(report["recovery"]["repair_possible"], true);

    let repaired = dir.join("repaired.sqz");
    let out = run(sqz()
        .arg("repair")
        .arg(&damaged)
        .arg("-o")
        .arg(&repaired)
        .arg("--json"));
    assert!(out.status.success(), "repair failed: {}", stderr(&out));
    let report: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    assert_eq!(report["ok"], true);
    assert_eq!(report["operation"], "repair_sqz");
    assert_eq!(report["tool"], "sqz-embedded-recovery");
    assert_eq!(report["in_place"], false);
    assert_eq!(report["recovery"]["scheme"], "sqz-embedded-rs-gf8");
    assert_eq!(report["recovery"]["damaged_blocks"], 1);
    assert_eq!(report["recovery"]["repaired_blocks"], 1);
    assert_eq!(report["recovery"]["unrepaired_blocks"], 0);
    assert_eq!(report["recovery"]["repair_possible"], true);
    assert_eq!(report["source"]["recovery"], report["recovery"]);
    assert!(repaired.is_file(), "repaired output missing");

    let out = run(sqz().arg("test").arg(&repaired).arg("--json"));
    assert!(
        out.status.success(),
        "test repaired failed: {}",
        stderr(&out)
    );
    let report: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    assert_eq!(report["ok"], true);
    assert_eq!(report["recovery"]["damaged_blocks"], 0);
    assert_eq!(report["recovery"]["repaired_blocks"], 0);
    assert_eq!(report["recovery"]["unrepaired_blocks"], 0);

    let dest = dir.join("repaired-files");
    let out = run(sqz().arg("extract").arg(&repaired).arg("-d").arg(&dest));
    assert!(
        out.status.success(),
        "extract repaired failed: {}",
        stderr(&out)
    );
    assert_eq!(
        std::fs::read(dest.join("project/a.txt")).unwrap(),
        b"hello world"
    );
    assert_eq!(
        std::fs::read(dest.join("project/sub/b.txt")).unwrap(),
        b"nested content"
    );

    let out = run(sqz().arg("repair").arg(&damaged).arg("--json"));
    assert!(
        out.status.success(),
        "in-place repair failed: {}",
        stderr(&out)
    );
    let report: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    assert_eq!(report["ok"], true);
    assert_eq!(report["operation"], "repair_sqz");
    assert_eq!(report["output"], damaged.display().to_string());
    assert_eq!(report["in_place"], true);
    assert_eq!(report["source"]["ok"], true);
    assert_eq!(report["source"]["recovery"], report["recovery"]);
    assert_eq!(report["recovery"]["damaged_blocks"], 1);
    assert_eq!(report["recovery"]["repaired_blocks"], 1);
    assert_eq!(report["recovery"]["unrepaired_blocks"], 0);
    assert_eq!(report["recovery"]["repair_possible"], true);

    let out = run(sqz().arg("test").arg(&damaged).arg("--json"));
    assert!(
        out.status.success(),
        "test in-place repaired failed: {}",
        stderr(&out)
    );
    let report: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    assert_eq!(report["ok"], true);
    assert_eq!(report["recovery"]["damaged_blocks"], 0);
    assert_eq!(report["recovery"]["repaired_blocks"], 0);
    assert_eq!(report["recovery"]["unrepaired_blocks"], 0);

    let out = run(sqz()
        .arg("repair")
        .arg(&damaged)
        .arg("--recovery")
        .arg(dir.join("wrong.par2")));
    assert_eq!(out.status.code(), Some(2), "stderr: {}", stderr(&out));
    assert!(
        stderr(&out).contains(".sqz repair uses embedded recovery"),
        "stderr: {}",
        stderr(&out)
    );

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn update_add_delete_rename_through_the_cli() {
    let dir = temp_dir("update-cli");
    let root = sample_tree(&dir);
    let archive = dir.join("out.zip");
    run(sqz().arg("compress").arg(&root).arg("-o").arg(&archive));
    std::fs::write(dir.join("extra.txt"), b"appended").unwrap();
    let add_dir = dir.join("append-dir");
    std::fs::create_dir_all(add_dir.join("node_modules")).unwrap();
    std::fs::write(add_dir.join("keep.txt"), b"keep").unwrap();
    std::fs::write(add_dir.join("node_modules/skip.js"), b"skip").unwrap();
    std::fs::write(add_dir.join("skip.tmp"), b"skip").unwrap();

    let out = run(sqz()
        .args(["--lang", "zh-CN", "update"])
        .arg(&archive)
        .arg("--add")
        .arg(dir.join("extra.txt"))
        .arg("--add")
        .arg(&add_dir)
        .args(["--mkdir", "empty/reports/"])
        .args(["--delete", "*.tmp"])
        .args(["--exclude", "node_modules", "--exclude", "*.tmp"])
        .args(["--rename", "project/a.txt=project/renamed.txt"])
        .args(["--move", "project/sub/b.txt=moved/b.txt"])
        .arg("--json"));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let report: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    assert_eq!(report["ok"], true);
    assert_eq!(report["operation"], "update");
    assert_eq!(report["archive"], archive.display().to_string());
    assert_eq!(report["operations"], 6);

    let out = run(sqz().arg("list").arg(&archive).arg("--json"));
    let entries: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    let paths: Vec<&str> = entries
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["path"].as_str().unwrap())
        .collect();
    assert!(paths.contains(&"extra.txt"));
    assert!(paths.contains(&"moved/b.txt"));
    assert!(!paths.contains(&"project/sub/b.txt"));
    assert!(paths.contains(&"append-dir/keep.txt"));
    assert!(paths.contains(&"empty/reports/"));
    assert!(paths.contains(&"project/renamed.txt"));
    assert!(!paths.contains(&"project/a.txt"));
    assert!(!paths.iter().any(|p| p.ends_with(".tmp")));
    assert!(!paths.iter().any(|p| p.contains("node_modules")));

    // English message variant.
    let out = run(sqz()
        .args(["--lang", "en-US", "update"])
        .arg(&archive)
        .args(["--delete", "*.log"]));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(stdout(&out).contains("Updated"), "stdout: {}", stdout(&out));

    // No operation flags at all → clap usage error.
    let out = run(sqz().arg("update").arg(&archive));
    assert!(!out.status.success());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[cfg(unix)]
#[test]
fn recovery_commands_bridge_to_external_par2_tool() {
    use std::os::unix::fs::PermissionsExt;

    let dir = temp_dir("recovery-cli");
    let archive = dir.join("protected.zip");
    let recovery = dir.join("protected.zip.par2");
    let tool = dir.join("fake-par2");
    let log = dir.join("fake-par2.log");
    std::fs::write(&archive, b"archive bytes").unwrap();
    std::fs::write(
        &tool,
        r#"#!/bin/sh
echo "$*" >> "$SQUALLZ_FAKE_PAR2_LOG"
case "$1" in
  create)
    printf 'fake recovery data\n' > "$3"
    ;;
  verify|repair)
    test -f "$2" || exit 2
    if [ "$1" = repair ]; then
      target="${2%.par2}"
      printf 'repaired bytes\n' > "$target"
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

    let out = run(sqz()
        .env("SQUALLZ_PAR2", &tool)
        .env("SQUALLZ_FAKE_PAR2_LOG", &log)
        .arg("protect")
        .arg(&archive)
        .arg("--recovery")
        .arg(&recovery)
        .args(["--redundancy", "12%", "--json"]));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let report: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(report["ok"], true);
    assert_eq!(report["operation"], "protect");
    assert_eq!(report["redundancy_percent"], 12);
    let recovery_path = recovery.to_string_lossy().into_owned();
    assert_eq!(report["recovery"].as_str(), Some(recovery_path.as_str()));
    assert!(recovery.is_file());

    let modern_recovery = dir.join("modern.zip.par2");
    let out = run(sqz()
        .env("SQUALLZ_PAR2", &tool)
        .env("SQUALLZ_FAKE_PAR2_LOG", &log)
        .args(["--lang", "en-US", "--style", "modern", "--color", "never"])
        .arg("protect")
        .arg(&archive)
        .arg("--recovery")
        .arg(&modern_recovery)
        .args(["--redundancy", "12%"]));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let text = stdout(&out);
    assert!(
        text.contains("Recovery result")
            && text.contains("Recovery report")
            && text.contains("Operation")
            && text.contains("Tool")
            && text.contains("protect")
            && text.contains("┬")
            && text.contains("┼"),
        "modern recovery output should use a status panel and table: {text}"
    );
    assert!(modern_recovery.is_file());

    let out = run(sqz()
        .env("SQUALLZ_PAR2", &tool)
        .env("SQUALLZ_FAKE_PAR2_LOG", &log)
        .arg("verify")
        .arg(&archive)
        .arg("--use-recovery")
        .arg("--recovery")
        .arg(&recovery)
        .arg("--json"));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let report: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(report["ok"], true);
    assert_eq!(report["operation"], "verify");

    let out = run(sqz()
        .env("SQUALLZ_PAR2", &tool)
        .env("SQUALLZ_FAKE_PAR2_LOG", &log)
        .args(["--lang", "en-US", "--style", "modern", "--color", "never"])
        .arg("verify")
        .arg(&archive)
        .arg("--use-recovery")
        .arg("--recovery")
        .arg(&recovery));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let text = stdout(&out);
    assert!(
        text.contains("Recovery result")
            && text.contains("Recovery report")
            && text.contains("Operation")
            && text.contains("Tool")
            && text.contains("Status")
            && text.contains("verify")
            && text.contains("┬")
            && text.contains("┼"),
        "modern recovery verify output should use a status panel and table: {text}"
    );

    let out = run(sqz()
        .env("SQUALLZ_PAR2", &tool)
        .env("SQUALLZ_FAKE_PAR2_LOG", &log)
        .arg("repair")
        .arg(&archive)
        .arg("--use-recovery")
        .arg("--recovery")
        .arg(&recovery)
        .arg("--json"));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let report: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(report["ok"], true);
    assert_eq!(report["operation"], "repair");

    let modern_repair_archive = dir.join("modern-repair.zip");
    let modern_repair_recovery = dir.join("modern-repair.zip.par2");
    std::fs::write(&modern_repair_archive, b"damaged bytes").unwrap();
    std::fs::write(&modern_repair_recovery, b"fake recovery data").unwrap();
    let out = run(sqz()
        .env("SQUALLZ_PAR2", &tool)
        .env("SQUALLZ_FAKE_PAR2_LOG", &log)
        .args(["--lang", "en-US", "--style", "modern", "--color", "never"])
        .arg("repair")
        .arg(&modern_repair_archive)
        .arg("--use-recovery")
        .arg("--recovery")
        .arg(&modern_repair_recovery));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let text = stdout(&out);
    assert!(
        text.contains("Recovery result")
            && text.contains("Recovery report")
            && text.contains("Operation")
            && text.contains("Tool")
            && text.contains("Status")
            && text.contains("repair")
            && text.contains("┬")
            && text.contains("┼"),
        "modern recovery repair output should use a status panel and table: {text}"
    );
    assert_eq!(
        std::fs::read(&modern_repair_archive).unwrap(),
        b"repaired bytes\n"
    );

    let copy_archive = dir.join("damaged.zip");
    let copy_recovery = dir.join("damaged.zip.par2");
    let copy_output = dir.join("restored.zip");
    std::fs::write(&copy_archive, b"damaged bytes").unwrap();
    std::fs::write(&copy_recovery, b"fake recovery data").unwrap();
    let out = run(sqz()
        .env("SQUALLZ_PAR2", &tool)
        .env("SQUALLZ_FAKE_PAR2_LOG", &log)
        .arg("repair")
        .arg(&copy_archive)
        .arg("--use-recovery")
        .arg("--recovery")
        .arg(&copy_recovery)
        .arg("--output")
        .arg(&copy_output)
        .arg("--json"));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let report: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(report["ok"], true);
    assert_eq!(report["operation"], "repair");
    let output_path = copy_output.to_string_lossy().into_owned();
    assert_eq!(report["output"].as_str(), Some(output_path.as_str()));
    assert_eq!(std::fs::read(&copy_archive).unwrap(), b"damaged bytes");
    assert_eq!(std::fs::read(&copy_output).unwrap(), b"repaired bytes\n");

    let log = std::fs::read_to_string(&log).unwrap();
    assert!(log.contains("create -r12"), "log: {log}");
    assert!(log.contains("verify"), "log: {log}");
    assert!(log.contains("repair"), "log: {log}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[cfg(unix)]
#[test]
fn protect_tolerate_loss_maps_split_volumes_to_redundancy() {
    use std::os::unix::fs::PermissionsExt;

    let dir = temp_dir("recovery-tolerate-loss");
    let first = dir.join("split.zip.001");
    let second = dir.join("split.zip.002");
    let third = dir.join("split.zip.003");
    std::fs::write(&first, vec![b'a'; 100]).unwrap();
    std::fs::write(&second, vec![b'b'; 100]).unwrap();
    std::fs::write(&third, vec![b'c'; 100]).unwrap();
    let recovery = dir.join("split.zip.par2");
    let tool = dir.join("fake-par2");
    let log = dir.join("fake-par2.log");
    std::fs::write(
        &tool,
        r#"#!/bin/sh
echo "$*" >> "$SQUALLZ_FAKE_PAR2_LOG"
case "$1" in
  create)
    printf 'fake recovery data\n' > "$3"
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

    let out = run(sqz()
        .env("SQUALLZ_PAR2", &tool)
        .env("SQUALLZ_FAKE_PAR2_LOG", &log)
        .arg("protect")
        .arg(&first)
        .arg("--tolerate-loss")
        .arg("2volumes")
        .arg("--recovery")
        .arg(&recovery)
        .arg("--json"));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let report: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(report["ok"], true);
    assert_eq!(report["operation"], "protect");
    assert_eq!(report["redundancy_percent"], 67);
    assert!(recovery.is_file());

    let log = std::fs::read_to_string(&log).unwrap();
    assert!(log.contains("create -r67"), "log: {log}");
    assert!(log.contains(first.to_string_lossy().as_ref()), "log: {log}");
    assert!(
        log.contains(second.to_string_lossy().as_ref()),
        "log: {log}"
    );
    assert!(log.contains(third.to_string_lossy().as_ref()), "log: {log}");

    let single = dir.join("single.zip");
    std::fs::write(&single, b"single").unwrap();
    let out = run(sqz()
        .env("SQUALLZ_PAR2", &tool)
        .arg("protect")
        .arg(&single)
        .arg("--tolerate-loss")
        .arg("1")
        .arg("--json"));
    assert_json_error(
        &out,
        2,
        "unsupported",
        "--tolerate-loss requires a .001 split volume set",
    );

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn recovery_reports_missing_par2_dependency() {
    let dir = temp_dir("recovery-missing");
    let archive = dir.join("protected.zip");
    std::fs::write(&archive, b"archive bytes").unwrap();
    let missing_tool = dir.join("missing-par2");

    let out = run(sqz()
        .env("SQUALLZ_PAR2", &missing_tool)
        .args(["--lang", "en-US", "protect"])
        .arg(&archive)
        .arg("--json"));
    assert_json_error(&out, 8, "dependency_missing", "Missing external dependency");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn extract_symlink_follow_materializes_content() {
    #[cfg(unix)]
    {
        let dir = temp_dir("follow-cli");
        let root = dir.join("tree");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("data.txt"), b"the real bytes").unwrap();
        std::os::unix::fs::symlink("data.txt", root.join("link.txt")).unwrap();
        let archive = dir.join("links.zip");
        run(sqz().arg("compress").arg(&root).arg("-o").arg(&archive));

        let dest = dir.join("out");
        let out = run(sqz()
            .arg("extract")
            .arg(&archive)
            .arg("-d")
            .arg(&dest)
            .args(["--symlinks", "follow"]));
        assert!(out.status.success(), "stderr: {}", stderr(&out));
        let link = dest.join("tree/link.txt");
        let meta = std::fs::symlink_metadata(&link).unwrap();
        assert!(meta.is_file(), "followed link must be a regular file");
        assert_eq!(std::fs::read(&link).unwrap(), b"the real bytes");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
