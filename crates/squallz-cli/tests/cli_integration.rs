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

fn json_output_paths(report: &serde_json::Value) -> Vec<PathBuf> {
    report["outputs"]
        .as_array()
        .expect("output path array")
        .iter()
        .map(|path| PathBuf::from(path.as_str().expect("output path string")))
        .collect()
}

fn output_paths_bytes(paths: &[PathBuf]) -> u64 {
    paths
        .iter()
        .map(|path| std::fs::metadata(path).expect("output metadata").len())
        .sum()
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

fn write_sfx_pe_stub(dir: &Path) -> PathBuf {
    let path = dir.join("sfx-stub.exe");
    let mut bytes = vec![0u8; 512];
    bytes[..2].copy_from_slice(b"MZ");
    bytes[0x3c..0x40].copy_from_slice(&0x80u32.to_le_bytes());
    bytes[0x80..0x84].copy_from_slice(b"PE\0\0");
    let marker = squallz_core::SFX_CLI_STUB_MARKER;
    bytes[0x100..0x100 + marker.len()].copy_from_slice(&marker);
    std::fs::write(&path, bytes).unwrap();
    path
}

fn write_sfx_macos_app_stub(dir: &Path) -> PathBuf {
    let bundle = dir.join("Squallz.app");
    let executable = bundle.join("Contents/MacOS/squallz-gui");
    std::fs::create_dir_all(executable.parent().unwrap()).unwrap();
    std::fs::create_dir_all(bundle.join("Contents/Resources")).unwrap();
    let mut bytes = vec![0u8; 512];
    bytes[..4].copy_from_slice(&[0xcf, 0xfa, 0xed, 0xfe]);
    let marker = squallz_core::SFX_GUI_STUB_MARKER;
    bytes[0x100..0x100 + marker.len()].copy_from_slice(&marker);
    std::fs::write(executable, bytes).unwrap();
    std::fs::write(
        bundle.join("Contents/Info.plist"),
        r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>squallz-gui</string>
<key>LSMinimumSystemVersion</key><string>11.0</string>
</dict></plist>
"#,
    )
    .unwrap();
    bundle
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
fn sfx_command(path: &Path) -> Command {
    let mut cmd = Command::new(path);
    cmd.env_remove("SQZ_LANG");
    cmd.env("SQZ_LOCALES_DIR", "/nonexistent/squallz-test-locales");
    cmd
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
        "sfx",
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
        "check-update",
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
                "--content-policy",
            ],
        ),
        (
            &["pack", "--help"],
            &[
                "--inner-format",
                "--profile",
                "--recovery",
                "--exclude",
                "--content-policy",
                "--split",
                "--threads",
                "--memory-limit",
                "--json",
            ],
        ),
        (
            &["estimate", "--help"],
            &["--exclude", "--content-policy", "--output", "--json"],
        ),
        (
            &["sfx", "create", "--help"],
            &[
                "--output",
                "--target",
                "--stub",
                "--force",
                "--memory-limit",
                "--json",
            ],
        ),
        (&["sfx", "inspect", "--help"], &["--memory-limit", "--json"]),
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
        (&["list", "--help"], &["--search", "--json", "--tree"]),
        (
            &["nested", "list", "--help"],
            &[
                "--password",
                "--encoding",
                "--nested-password",
                "--nested-encoding",
                "--search",
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
            &["--profile", "--password", "--encoding", "--force", "--json"],
        ),
        (
            &["export", "--help"],
            &["--profile", "--output", "--force", "--json"],
        ),
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
                "--content-policy",
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
        (&["check-update", "--help"], &["--json"]),
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

    let update_help = run(sqz().args(["check-update", "--lang", "en-US", "--help"]));
    assert!(
        update_help.status.success(),
        "stderr: {}",
        stderr(&update_help)
    );
    let help = stdout(&update_help);
    assert!(
        help.contains("Check the stable Squallz release channel")
            && help.contains("does not download or install update packages"),
        "stdout: {help}"
    );
    assert!(!help.contains("检查 Squallz 稳定版"), "stdout: {help}");

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
    assert!(
        help.contains("Explicit --exclude rules are combined with the selected policy"),
        "stdout: {help}"
    );
    assert!(!help.contains("输入文件或目录"), "stdout: {help}");

    let list_help = run(sqz().args(["list", "--lang", "en-US", "--help"]));
    assert!(list_help.status.success(), "stderr: {}", stderr(&list_help));
    let help = stdout(&list_help);
    assert!(
        help.contains("Search literal text across complete entry paths, ignoring case."),
        "stdout: {help}"
    );
    assert!(!help.contains("按完整条目路径"), "stdout: {help}");

    let nested_list_help = run(sqz()
        .env("SQZ_LANG", "en-US")
        .args(["nested", "list", "--help"]));
    assert!(
        nested_list_help.status.success(),
        "stderr: {}",
        stderr(&nested_list_help)
    );
    let help = stdout(&nested_list_help);
    assert!(
        help.contains("Search literal text across complete nested entry paths, ignoring case."),
        "stdout: {help}"
    );
    assert!(!help.contains("按完整嵌套条目路径"), "stdout: {help}");

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

    let zh_update_help = run(sqz().args(["check-update", "--lang", "zh-CN", "--help"]));
    assert!(
        zh_update_help.status.success(),
        "stderr: {}",
        stderr(&zh_update_help)
    );
    let help = stdout(&zh_update_help);
    assert!(
        help.contains("检查 Squallz 稳定版软件更新") && help.contains("不会下载或安装"),
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
    assert!(text.contains("Scope"), "stdout: {text}");
    assert!(text.contains("Target"), "stdout: {text}");
    assert!(text.contains("Created"), "stdout: {text}");
    assert!(text.contains("Skipped"), "stdout: {text}");
    assert!(text.contains("Replaced"), "stdout: {text}");
    assert!(text.contains("Renamed"), "stdout: {text}");
    assert!(text.contains("failed"), "stdout: {text}");

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

fn content_policy_tree(dir: &Path, name: &str) -> PathBuf {
    let root = dir.join(name);
    std::fs::create_dir_all(root.join("__MACOSX")).unwrap();
    std::fs::write(root.join("keep.txt"), b"keep").unwrap();
    std::fs::write(root.join(".env"), b"MODE=test").unwrap();
    std::fs::write(root.join("skip.tmp"), b"scratch").unwrap();
    std::fs::write(root.join(".DS_Store"), b"finder metadata").unwrap();
    std::fs::write(root.join("._keep.txt"), b"appledouble metadata").unwrap();
    std::fs::write(root.join("__MACOSX/metadata"), b"metadata").unwrap();
    root
}

fn listed_paths(archive: &Path) -> Vec<String> {
    let out = run(sqz().arg("list").arg(archive).arg("--json"));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    stdout_json(&out)
        .as_array()
        .expect("entry array")
        .iter()
        .filter_map(|entry| entry["path"].as_str().map(str::to_owned))
        .collect()
}

fn assert_cross_platform_clean(paths: &[String], root_name: &str) {
    assert!(paths
        .iter()
        .any(|path| path == &format!("{root_name}/keep.txt")));
    assert!(paths
        .iter()
        .any(|path| path == &format!("{root_name}/.env")));
    assert!(!paths.iter().any(|path| path.ends_with("/.DS_Store")));
    assert!(!paths.iter().any(|path| {
        path.rsplit_once('/')
            .is_some_and(|(_, name)| name.starts_with("._"))
    }));
    assert!(!paths
        .iter()
        .any(|path| path.contains("/__MACOSX/") || path.ends_with("/__MACOSX")));
    assert!(!paths.iter().any(|path| path.ends_with(".tmp")));
}

#[cfg(target_os = "macos")]
fn preset_path_for_test_home(home: &Path) -> PathBuf {
    home.join("Library/Application Support/Squallz/presets.json")
}

#[cfg(target_os = "linux")]
fn preset_path_for_test_home(home: &Path) -> PathBuf {
    home.join(".config/Squallz/presets.json")
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn file_manager_create_fallback_uses_bound_shared_preset() {
    let dir = temp_dir("file-manager-create-preset");
    let home = dir.join("home");
    let root = sample_tree(&dir);
    std::fs::create_dir_all(root.join("__MACOSX")).unwrap();
    std::fs::write(root.join(".DS_Store"), b"finder metadata").unwrap();
    std::fs::write(root.join("._a.txt"), b"appledouble metadata").unwrap();
    std::fs::write(root.join("__MACOSX/metadata"), b"metadata").unwrap();
    std::fs::write(root.join(".env"), b"MODE=test").unwrap();
    let archive = dir.join("preset-output.7z");

    let mut document = squallz_core::PresetDocument::seeded();
    let mut options = document
        .presets
        .iter()
        .find_map(squallz_core::NamedPreset::create_options)
        .expect("built-in create preset")
        .clone();
    options.level = squallz_core::PresetCompressionLevel::new(8).expect("valid level");
    options.content_policy = squallz_core::CreateContentPolicy::CrossPlatformClean;
    options.excludes.clear();
    let id = squallz_core::PresetId::new("user.create.cli-fallback").expect("valid id");
    document.presets.push(squallz_core::NamedPreset::Create {
        id: id.clone(),
        label: squallz_core::PresetLabel::new("CLI fallback").expect("valid label"),
        built_in: false,
        options,
    });
    document.bindings.file_manager_create = Some(id);
    squallz_core::PresetStore::new(preset_path_for_test_home(&home))
        .compare_and_swap(0, document)
        .expect("write preset fixture");

    let out = run(sqz()
        .env("HOME", &home)
        .env_remove("XDG_CONFIG_HOME")
        .arg("compress")
        .arg(&root)
        .arg("-o")
        .arg(&archive)
        .arg("--file-manager-preset")
        .arg("--json"));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let report = stdout_json(&out);
    assert_eq!(report["level"], 8);
    assert_eq!(report["output"], archive.display().to_string());

    let listed = run(sqz().arg("list").arg(&archive).arg("--json"));
    assert!(listed.status.success(), "stderr: {}", stderr(&listed));
    let entries = stdout_json(&listed);
    let paths: Vec<&str> = entries
        .as_array()
        .expect("entry array")
        .iter()
        .filter_map(|entry| entry["path"].as_str())
        .collect();
    assert!(!paths.iter().any(|path| path.ends_with(".DS_Store")));
    assert!(!paths.iter().any(|path| path.ends_with("._a.txt")));
    assert!(!paths.iter().any(|path| path.contains("__MACOSX")));
    assert!(paths.iter().any(|path| path.ends_with(".env")));
    assert!(paths.iter().any(|path| path.ends_with("junk.tmp")));
    assert!(paths.iter().any(|path| path.contains(".git")));

    std::fs::remove_dir_all(dir).unwrap();
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn file_manager_fallback_rejects_a_malformed_shared_preset() {
    let dir = temp_dir("file-manager-malformed-preset");
    let home = dir.join("home");
    let input = dir.join("input.txt");
    let archive = dir.join("must-not-exist.7z");
    std::fs::write(&input, b"payload").unwrap();
    let preset_path = preset_path_for_test_home(&home);
    std::fs::create_dir_all(preset_path.parent().expect("preset parent")).unwrap();
    std::fs::write(&preset_path, b"{not json").unwrap();

    let out = run(sqz()
        .env("HOME", &home)
        .env_remove("XDG_CONFIG_HOME")
        .args(["--lang", "en-US", "compress"])
        .arg(&input)
        .arg("-o")
        .arg(&archive)
        .arg("--file-manager-preset"));
    assert_eq!(out.status.code(), Some(1), "stderr: {}", stderr(&out));
    assert!(
        stderr(&out).contains("cannot load the shared file-manager preset"),
        "stderr: {}",
        stderr(&out)
    );
    assert!(!archive.exists());

    std::fs::remove_dir_all(dir).unwrap();
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn preset_cli_round_trips_clone_update_bind_and_delete() {
    let dir = temp_dir("preset-cli-round-trip");
    let home = dir.join("home");
    let edit_file = dir.join("preset.json");

    let listed = run(sqz()
        .env("HOME", &home)
        .env_remove("XDG_CONFIG_HOME")
        .args(["--lang", "en-US", "preset", "list", "--json"]));
    assert!(listed.status.success(), "stderr: {}", stderr(&listed));
    let listed = stdout_json(&listed);
    assert_eq!(listed["schema_version"], 4);
    assert_eq!(listed["revision"], 0);
    assert_eq!(listed["presets"].as_array().map(Vec::len), Some(3));

    let cloned = run(sqz()
        .env("HOME", &home)
        .env_remove("XDG_CONFIG_HOME")
        .args([
            "--lang",
            "en-US",
            "preset",
            "clone",
            squallz_core::CROSS_PLATFORM_CREATE_PRESET_ID,
            "user.create.portable",
            "--label",
            "Portable",
            "--json",
        ]));
    assert!(cloned.status.success(), "stderr: {}", stderr(&cloned));
    let cloned = stdout_json(&cloned);
    assert_eq!(cloned["revision"], 1);
    assert_eq!(cloned["preset"]["id"], "user.create.portable");
    assert_eq!(cloned["preset"]["built_in"], false);

    let shown = run(sqz()
        .env("HOME", &home)
        .env_remove("XDG_CONFIG_HOME")
        .args([
            "--lang",
            "en-US",
            "preset",
            "show",
            "user.create.portable",
            "--json",
        ]));
    assert!(shown.status.success(), "stderr: {}", stderr(&shown));
    let mut edited = stdout_json(&shown);
    edited["label"] = serde_json::json!("Portable maximum");
    edited["options"]["level"] = serde_json::json!(9);
    std::fs::write(&edit_file, serde_json::to_vec_pretty(&edited).unwrap()).unwrap();

    let updated = run(sqz()
        .env("HOME", &home)
        .env_remove("XDG_CONFIG_HOME")
        .args([
            "--lang",
            "en-US",
            "preset",
            "update",
            "user.create.portable",
            "--file",
        ])
        .arg(&edit_file)
        .arg("--json"));
    assert!(updated.status.success(), "stderr: {}", stderr(&updated));
    let updated = stdout_json(&updated);
    assert_eq!(updated["revision"], 2);
    assert_eq!(updated["preset"]["label"], "Portable maximum");
    assert_eq!(updated["preset"]["options"]["level"], 9);

    let bound = run(sqz()
        .env("HOME", &home)
        .env_remove("XDG_CONFIG_HOME")
        .args([
            "--lang",
            "en-US",
            "preset",
            "bind",
            "app-create",
            "user.create.portable",
            "--json",
        ]));
    assert!(bound.status.success(), "stderr: {}", stderr(&bound));
    let bound = stdout_json(&bound);
    assert_eq!(bound["revision"], 3);
    assert_eq!(
        bound["bindings"]["app_default_create"],
        "user.create.portable"
    );

    let deleted = run(sqz()
        .env("HOME", &home)
        .env_remove("XDG_CONFIG_HOME")
        .args([
            "--lang",
            "en-US",
            "preset",
            "delete",
            "user.create.portable",
            "--json",
        ]));
    assert!(deleted.status.success(), "stderr: {}", stderr(&deleted));
    let deleted = stdout_json(&deleted);
    assert_eq!(deleted["revision"], 4);
    assert_eq!(
        deleted["bindings"]["app_default_create"],
        squallz_core::BALANCED_CREATE_PRESET_ID
    );

    let final_document = squallz_core::PresetStore::new(preset_path_for_test_home(&home))
        .load()
        .expect("preset document should remain readable");
    assert_eq!(final_document.revision, 4);
    assert!(final_document
        .presets
        .iter()
        .all(|preset| preset.id().as_str() != "user.create.portable"));

    std::fs::remove_dir_all(dir).unwrap();
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn preset_cli_rejects_unsafe_file_manager_binding_without_changing_document() {
    let dir = temp_dir("preset-cli-binding-validation");
    let home = dir.join("home");
    let edit_file = dir.join("prompt.json");

    let cloned = run(sqz()
        .env("HOME", &home)
        .env_remove("XDG_CONFIG_HOME")
        .args([
            "--lang",
            "en-US",
            "preset",
            "clone",
            squallz_core::CROSS_PLATFORM_CREATE_PRESET_ID,
            "user.create.prompt",
            "--label",
            "Prompt for password",
            "--json",
        ]));
    assert!(cloned.status.success(), "stderr: {}", stderr(&cloned));

    let shown = run(sqz()
        .env("HOME", &home)
        .env_remove("XDG_CONFIG_HOME")
        .args([
            "--lang",
            "en-US",
            "preset",
            "show",
            "user.create.prompt",
            "--json",
        ]));
    assert!(shown.status.success(), "stderr: {}", stderr(&shown));
    let mut edited = stdout_json(&shown);
    edited["options"]["credential"] = serde_json::json!({ "kind": "prompt" });
    std::fs::write(&edit_file, serde_json::to_vec_pretty(&edited).unwrap()).unwrap();
    let updated = run(sqz()
        .env("HOME", &home)
        .env_remove("XDG_CONFIG_HOME")
        .args([
            "--lang",
            "en-US",
            "preset",
            "update",
            "user.create.prompt",
            "--file",
        ])
        .arg(&edit_file)
        .arg("--json"));
    assert!(updated.status.success(), "stderr: {}", stderr(&updated));
    assert_eq!(stdout_json(&updated)["revision"], 2);

    let rejected = run(sqz()
        .env("HOME", &home)
        .env_remove("XDG_CONFIG_HOME")
        .args([
            "--lang",
            "en-US",
            "preset",
            "bind",
            "file-manager-create",
            "user.create.prompt",
            "--json",
        ]));
    assert_eq!(rejected.status.code(), Some(1));
    let rejected = stdout_json(&rejected);
    assert_eq!(rejected["ok"], false);
    assert!(rejected["error"]["message"]
        .as_str()
        .is_some_and(|message| message.contains("file-manager create preset")));

    let document = squallz_core::PresetStore::new(preset_path_for_test_home(&home))
        .load()
        .expect("rejected binding must preserve the document");
    assert_eq!(document.revision, 2);
    assert_eq!(
        document
            .bindings
            .file_manager_create
            .as_ref()
            .map(squallz_core::PresetId::as_str),
        Some(squallz_core::CROSS_PLATFORM_CREATE_PRESET_ID)
    );

    std::fs::remove_dir_all(dir).unwrap();
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
    assert_eq!(report["jobs"][2]["result"]["matched"], true);
    assert_eq!(
        report["jobs"][2]["result"]["plan"]["destination"],
        extracted.display().to_string()
    );
    assert_eq!(report["jobs"][2]["result"]["plan"]["entries"], 1);
    assert_eq!(
        report["jobs"][2]["result"]["counts"]["destination"],
        extracted.display().to_string()
    );
    assert_eq!(report["jobs"][2]["result"]["counts"]["created"], 1);
    assert_eq!(report["jobs"][2]["result"]["counts"]["skipped"], 0);
    assert_eq!(report["jobs"][2]["result"]["counts"]["replaced"], 0);
    assert_eq!(report["jobs"][2]["result"]["counts"]["renamed"], 0);
    assert_eq!(report["jobs"][2]["result"]["counts"]["failed"], 0);
    assert_eq!(
        report["results"][2]["result"]["counts"],
        report["jobs"][2]["result"]["counts"]
    );
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
fn batch_content_policy_covers_create_estimate_pack_and_update() {
    let dir = temp_dir("batch-content-policy");
    let _root = content_policy_tree(&dir, "batch-policy-input");
    let seed = dir.join("seed.txt");
    let updated = dir.join("updated.zip");
    std::fs::write(&seed, b"seed").unwrap();
    let out = run(sqz().arg("compress").arg(&seed).arg("-o").arg(&updated));
    assert!(out.status.success(), "stderr: {}", stderr(&out));

    let script = dir.join("content-policy.json");
    let manifest = serde_json::json!({
        "jobs": [
            {
                "id": "estimate-clean",
                "kind": "estimate",
                "inputs": ["batch-policy-input"],
                "content_policy": "cross_platform_clean",
                "excludes": ["*.tmp", ".DS_Store", "*.tmp"]
            },
            {
                "id": "compress-clean",
                "kind": "compress",
                "inputs": ["batch-policy-input"],
                "output": "clean.zip",
                "content_policy": "cross_platform_clean",
                "excludes": ["*.tmp", ".DS_Store", "*.tmp"]
            },
            {
                "id": "pack-clean",
                "kind": "pack",
                "inputs": ["batch-policy-input"],
                "output": "clean.sqz",
                "content_policy": "cross_platform_clean",
                "excludes": ["*.tmp", ".DS_Store", "*.tmp"]
            },
            {
                "id": "update-clean",
                "kind": "update",
                "archive": "updated.zip",
                "add": ["batch-policy-input"],
                "content_policy": "cross_platform_clean",
                "excludes": ["*.tmp", ".DS_Store", "*.tmp"]
            }
        ]
    });
    std::fs::write(&script, serde_json::to_string_pretty(&manifest).unwrap()).unwrap();

    let out = run(sqz().arg("batch").arg(&script).arg("--json"));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let report = stdout_json(&out);
    assert_eq!(report["ok"], true);
    assert_eq!(report["total"], 4);
    assert_eq!(report["jobs"][0]["result"]["entries"], 3);
    assert_eq!(report["jobs"][0]["result"]["files"], 2);

    for archive in [dir.join("clean.zip"), dir.join("clean.sqz"), updated] {
        assert_cross_platform_clean(&listed_paths(&archive), "batch-policy-input");
    }

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn batch_create_jobs_report_actual_split_output_families() {
    let dir = temp_dir("batch-create-report");
    incompressible_file(&dir, "batch-data.bin");
    let script = dir.join("batch.json");
    let manifest = serde_json::json!({
        "version": 1,
        "jobs": [
            {
                "id": "compress-split",
                "kind": "compress",
                "inputs": ["batch-data.bin"],
                "output": "batch.zip",
                "split": 30 * 1024,
                "profile": "fast"
            },
            {
                "id": "pack-split",
                "kind": "pack",
                "inputs": ["batch-data.bin"],
                "output": "batch.sqz",
                "split": 30 * 1024,
                "inner_format": "sqz",
                "recovery": 10
            },
            {
                "id": "compress-native-zip",
                "kind": "compress",
                "inputs": ["batch-data.bin"],
                "output": "batch-native.zip",
                "split": 64 * 1024,
                "split_mode": "native",
                "profile": "fast"
            }
        ]
    });
    std::fs::write(&script, serde_json::to_string_pretty(&manifest).unwrap()).unwrap();

    let out = run(sqz().arg("batch").arg(&script).arg("--json"));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let report = stdout_json(&out);
    assert_eq!(report["ok"], true);
    assert_eq!(report["total"], 3);
    assert_eq!(report["jobs"], report["results"]);

    let compress = &report["jobs"][0]["result"];
    assert_eq!(compress["operation"], "compress");
    assert_eq!(compress["split"], true);
    assert_eq!(
        compress["output"],
        dir.join("batch.zip").display().to_string()
    );
    assert_eq!(
        compress["primary_output"],
        dir.join("batch.zip.001").display().to_string()
    );
    let compress_volume_count = compress["volumes"].as_u64().unwrap() as usize;
    let compress_outputs = json_output_paths(compress);
    let expected_compress_outputs = (1..=compress_volume_count)
        .map(|index| dir.join(format!("batch.zip.{index:03}")))
        .collect::<Vec<_>>();
    assert_eq!(compress_outputs, expected_compress_outputs);
    assert_eq!(
        compress["total_bytes"],
        output_paths_bytes(&compress_outputs)
    );

    let pack = &report["jobs"][1]["result"];
    assert_eq!(pack["operation"], "pack");
    assert_eq!(pack["split"], true);
    assert_eq!(pack["output"], dir.join("batch.sqz").display().to_string());
    assert_eq!(
        pack["primary_output"],
        dir.join("batch.sqz.001").display().to_string()
    );
    let pack_volume_count = pack["volumes"].as_u64().unwrap() as usize;
    let pack_outputs = json_output_paths(pack);
    let expected_pack_volumes = (1..=pack_volume_count)
        .map(|index| dir.join(format!("batch.sqz.{index:03}")))
        .collect::<Vec<_>>();
    assert_eq!(
        &pack_outputs[..pack_volume_count],
        expected_pack_volumes.as_slice()
    );
    let mut expected_sidecars = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("batch.sqz.rev"))
        })
        .collect::<Vec<_>>();
    expected_sidecars.sort();
    assert!(!expected_sidecars.is_empty());
    assert_eq!(
        &pack_outputs[pack_volume_count..],
        expected_sidecars.as_slice()
    );
    assert_eq!(pack["total_bytes"], output_paths_bytes(&pack_outputs));

    let native = &report["jobs"][2]["result"];
    assert_eq!(native["operation"], "compress");
    assert_eq!(native["split"], true);
    assert_eq!(
        native["primary_output"],
        dir.join("batch-native.zip").display().to_string()
    );
    let native_outputs = json_output_paths(native);
    assert_eq!(native_outputs.first(), Some(&dir.join("batch-native.z01")));
    assert_eq!(native_outputs.last(), Some(&dir.join("batch-native.zip")));
    assert!(native_outputs.len() >= 2);
    assert!(native_outputs.iter().all(|path| path.is_file()));

    let human = run(sqz()
        .args(["--lang", "en-US", "--style", "modern", "--color", "never"])
        .arg("batch")
        .arg(&script));
    assert!(human.status.success(), "stderr: {}", stderr(&human));
    let human_text = stdout(&human);
    assert!(
        human_text.contains("Previous outputs need review"),
        "stdout: {human_text}"
    );
    assert!(
        human_text.contains("Test the new archive first"),
        "stdout: {human_text}"
    );
    let retained = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.to_string_lossy().contains(".split-backup-"))
        .collect::<Vec<_>>();
    assert!(!retained.is_empty());
    for path in retained {
        assert!(
            human_text.contains(&path.display().to_string()),
            "human batch output omitted preserved path {}: {human_text}",
            path.display()
        );
    }

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn batch_convert_can_publish_native_zip_volumes() {
    let dir = temp_dir("batch-convert-native-zip");
    let input = incompressible_file_with_len(&dir, "payload.bin", 180 * 1024);
    let source = dir.join("source.7z");
    let create = run(sqz().arg("compress").arg(&input).arg("-o").arg(&source));
    assert!(create.status.success(), "stderr: {}", stderr(&create));

    let script = dir.join("batch.json");
    let manifest = serde_json::json!({
        "version": 1,
        "jobs": [{
            "id": "convert-native",
            "kind": "convert",
            "src": "source.7z",
            "output": "converted.zip",
            "split": 64 * 1024,
            "split_mode": "native",
            "profile": "balanced"
        }]
    });
    std::fs::write(&script, serde_json::to_string_pretty(&manifest).unwrap()).unwrap();

    let out = run(sqz().arg("batch").arg(&script).arg("--json"));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let report = stdout_json(&out);
    let result = &report["jobs"][0]["result"];
    let destination = dir.join("converted.zip");
    let outputs = json_output_paths(result);
    assert_eq!(result["operation"], "convert");
    assert_eq!(result["split"], true);
    assert_eq!(result["primary_output"], destination.display().to_string());
    assert_eq!(outputs.first(), Some(&dir.join("converted.z01")));
    assert_eq!(outputs.last(), Some(&destination));
    assert!(outputs.len() >= 2);
    assert!(outputs.iter().all(|path| path.is_file()));

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

#[test]
fn batch_extract_no_match_preserves_an_invalid_destination() {
    let dir = temp_dir("batch-no-match-invalid-destination");
    let root = sample_tree(&dir);
    let archive = dir.join("source.zip");
    let occupied = dir.join("occupied-output");
    let script = dir.join("batch.json");
    let created = run(sqz().arg("compress").arg(&root).arg("-o").arg(&archive));
    assert!(created.status.success(), "stderr: {}", stderr(&created));
    std::fs::write(&occupied, b"keep").unwrap();
    let manifest = serde_json::json!({
        "version": 1,
        "jobs": [{
            "kind": "extract",
            "archive": "source.zip",
            "dest": "occupied-output",
            "includes": ["does/not/exist"]
        }]
    });
    std::fs::write(&script, serde_json::to_string_pretty(&manifest).unwrap()).unwrap();

    let out = run(sqz().arg("batch").arg(&script).arg("--json"));

    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let report = stdout_json(&out);
    assert_eq!(report["ok"], true);
    assert_eq!(report["jobs"][0]["result"]["matched"], false);
    assert_eq!(report["jobs"][0]["result"]["plan"]["entries"], 0);
    assert_eq!(std::fs::read(&occupied).unwrap(), b"keep");
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
        .arg("--test-after-create")
        .arg("--json"));
    assert!(out.status.success(), "compress failed: {}", stderr(&out));
    let report: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    assert_eq!(report["ok"], true);
    assert_eq!(report["operation"], "compress");
    assert_eq!(report["output"], archive.display().to_string());
    assert_eq!(report["split"], false);
    assert_eq!(report["volumes"], 1);
    assert_eq!(report["tested_after_create"], true);
    assert!(report["entries_tested_after_create"]
        .as_u64()
        .is_some_and(|count| count > 0));
    assert_eq!(
        report["outputs"],
        serde_json::json!([archive.display().to_string()])
    );
    assert_eq!(
        report["total_bytes"],
        std::fs::metadata(&archive).unwrap().len()
    );

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

    // list --search: literal, case-insensitive full-path filtering keeps the
    // existing JSON entry object unchanged.
    let expected_nested_file = entries
        .iter()
        .find(|e| e["path"] == "project/sub/b.txt")
        .cloned()
        .expect("b.txt listed");
    let out = run(sqz()
        .arg("list")
        .arg(&archive)
        .args(["--search", "SUB\\B.TXT", "--json"]));
    assert!(
        out.status.success(),
        "list --search failed: {}",
        stderr(&out)
    );
    let filtered: serde_json::Value =
        serde_json::from_str(&stdout(&out)).expect("valid filtered JSON");
    assert_eq!(filtered, serde_json::json!([expected_nested_file]));

    let out = run(sqz()
        .arg("list")
        .arg(&archive)
        .args(["--search", "  ", "--json"]));
    assert!(
        out.status.success(),
        "blank list --search failed: {}",
        stderr(&out)
    );
    let unfiltered: serde_json::Value =
        serde_json::from_str(&stdout(&out)).expect("valid unfiltered JSON");
    assert_eq!(unfiltered.as_array(), Some(entries));

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
    assert_eq!(report["plan"]["destination"], dest.display().to_string());
    assert_eq!(report["plan"]["layout"], "direct");
    assert!(report["plan"]["entries"].as_u64().unwrap() >= 6);
    assert_eq!(report["counts"]["destination"], dest.display().to_string());
    assert!(report["counts"]["created"].as_u64().unwrap() >= 4);
    assert_eq!(report["counts"]["skipped"], 0);
    assert_eq!(report["counts"]["replaced"], 0);
    assert_eq!(report["counts"]["renamed"], 0);
    assert_eq!(report["counts"]["failed"], 0);
    assert_eq!(report["selected_entries"], report["plan"]["entries"]);
    assert_eq!(
        report["counts"]["selected_entries"],
        report["selected_entries"]
    );
    assert_eq!(report["counts"]["directories"], report["directories"]);
    assert_eq!(report["counts"]["output_bytes"], report["output_bytes"]);
    assert!(report["output_bytes"].as_u64().unwrap() > 0);
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
    let loose_payload = dir.join("loose-payload.txt");
    std::fs::write(&loose_payload, b"loose nested content").unwrap();
    let logical_inner = dir.join("logical.zip");
    let out = run(sqz()
        .arg("compress")
        .arg(&loose_payload)
        .arg("-o")
        .arg(&logical_inner));
    assert!(
        out.status.success(),
        "loose inner compress failed: {}",
        stderr(&out)
    );

    let outer_root = dir.join("outer");
    std::fs::create_dir_all(outer_root.join("bundles")).unwrap();
    std::fs::copy(&inner, outer_root.join("bundles/inner.zip")).unwrap();
    std::fs::copy(&logical_inner, outer_root.join("bundles/logical.zip")).unwrap();
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

    let expected_nested_file = entries
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["path"] == "project/sub/b.txt")
        .cloned()
        .expect("nested b.txt listed");
    let out = run(sqz()
        .arg("nested")
        .arg("list")
        .arg(&outer)
        .arg(nested_entry)
        .args(["--search", "SUB\\B.TXT", "--json"]));
    assert!(
        out.status.success(),
        "nested list --search failed: {}",
        stderr(&out)
    );
    let filtered: serde_json::Value =
        serde_json::from_str(&stdout(&out)).expect("valid nested filtered JSON");
    assert_eq!(filtered, serde_json::json!([expected_nested_file]));

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
    assert_eq!(report["plan"]["requested_destination"], report["dest"]);
    assert_eq!(report["plan"]["destination"], report["dest"]);
    assert_eq!(report["plan"]["layout"], "direct");
    assert_eq!(report["plan"]["entries"], report["selected_entries"]);
    assert_eq!(report["counts"]["destination"], report["dest"]);
    assert_eq!(
        report["counts"]["selected_entries"],
        report["selected_entries"]
    );
    assert_eq!(report["counts"]["directories"], report["directories"]);
    assert_eq!(report["counts"]["skipped"], 0);
    assert_eq!(report["counts"]["replaced"], 0);
    assert_eq!(report["counts"]["renamed"], 0);
    assert_eq!(report["counts"]["failed"], 0);
    assert_eq!(report["counts"]["output_bytes"], report["output_bytes"]);
    assert_eq!(report["output_bytes"], 14);
    assert_eq!(
        std::fs::read(dest.join("project/sub/b.txt")).unwrap(),
        b"nested content"
    );
    assert!(!dest.join("project/a.txt").exists());

    let out = run(sqz()
        .args(["--lang", "en-US", "nested", "extract"])
        .arg(&outer)
        .arg(nested_entry)
        .arg("-d")
        .arg(&dest)
        .arg("--include")
        .arg("project/sub/*")
        .arg("--json"));
    assert!(
        out.status.success(),
        "nested conflict extract failed: {}",
        stderr(&out)
    );
    let conflict = stdout_json(&out);
    assert_eq!(
        conflict["skipped"], 0,
        "legacy skipped remains the best-effort problem count"
    );
    assert_eq!(conflict["counts"]["skipped"], 1);
    assert_eq!(conflict["counts"]["failed"], 0);

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
    assert_eq!(report["plan"]["requested_destination"], report["dest"]);
    assert_eq!(report["plan"]["destination"], report["dest"]);
    assert_eq!(report["plan"]["entries"], 0);
    assert_eq!(
        report["counts"],
        serde_json::json!({
            "destination": empty_dest.display().to_string(),
            "selected_entries": 0,
            "created": 0,
            "directories": 0,
            "skipped": 0,
            "replaced": 0,
            "renamed": 0,
            "failed": 0,
            "output_bytes": 0
        })
    );
    assert_eq!(report["selected_entries"], 0);
    assert_eq!(report["directories"], 0);
    assert_eq!(report["output_bytes"], 0);
    assert!(!empty_dest.exists());

    let smart_dest = dir.join("nested-smart-out");
    let out = run(sqz()
        .args(["--lang", "en-US", "nested", "extract"])
        .arg(&outer)
        .arg("outer/bundles/logical.zip")
        .arg("-d")
        .arg(&smart_dest)
        .args(["--smart", "--json"]));
    assert!(
        out.status.success(),
        "nested smart extract failed: {}",
        stderr(&out)
    );
    let report = stdout_json(&out);
    let planned_dest = smart_dest.join("logical");
    assert_eq!(report["dest"], planned_dest.display().to_string());
    assert_eq!(report["plan"]["layout"], "wrap_in_folder");
    assert_eq!(
        report["plan"]["requested_destination"],
        smart_dest.display().to_string()
    );
    assert_eq!(
        report["plan"]["destination"],
        planned_dest.display().to_string()
    );
    assert_eq!(
        report["counts"]["destination"],
        planned_dest.display().to_string()
    );
    assert_eq!(
        std::fs::read(planned_dest.join("loose-payload.txt")).unwrap(),
        b"loose nested content"
    );

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
fn compress_integrity_test_reuses_the_creation_password() {
    let dir = temp_dir("create-integrity-password");
    let root = sample_tree(&dir);
    let archive = dir.join("protected.7z");

    let out = run(sqz()
        .args(["--lang", "en-US", "compress"])
        .arg(&root)
        .arg("-o")
        .arg(&archive)
        .args([
            "--format",
            "7z",
            "--password",
            "test password",
            "--encrypt-names",
            "--test-after-create",
            "--json",
        ]));

    assert!(
        out.status.success(),
        "encrypted creation and integrity test failed: {}",
        stderr(&out)
    );
    let report = stdout_json(&out);
    assert_eq!(report["tested_after_create"], true);
    assert!(report["entries_tested_after_create"]
        .as_u64()
        .is_some_and(|count| count > 0));

    let out = run(sqz().arg("list").arg(&archive).arg("--json"));
    assert_eq!(out.status.code(), Some(4), "stdout: {}", stdout(&out));

    let out = run(sqz()
        .arg("list")
        .arg(&archive)
        .args(["--password", "test password", "--json"]));
    assert!(
        out.status.success(),
        "encrypted archive should reopen with its password: {}",
        stderr(&out)
    );

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
    assert_eq!(
        report["outputs"],
        serde_json::json!([archive.display().to_string()])
    );
    assert_eq!(
        report["total_bytes"],
        std::fs::metadata(&archive).unwrap().len()
    );

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
    let split_volume_count = report["volumes"].as_u64().unwrap() as usize;
    assert!(split_volume_count >= 2);
    assert_eq!(report["inner_format"], "sqz");
    assert_eq!(report["recovery_percent"], 10);
    let split_outputs = json_output_paths(&report);
    let expected_split_volumes = (1..=split_volume_count)
        .map(|index| dir.join(format!("packed-split.sqz.{index:03}")))
        .collect::<Vec<_>>();
    assert_eq!(
        &split_outputs[..split_volume_count],
        expected_split_volumes.as_slice()
    );
    assert!(split_outputs.len() > split_volume_count);
    assert!(split_outputs[split_volume_count..].iter().all(|path| path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with("packed-split.sqz.rev"))));
    assert_eq!(report["total_bytes"], output_paths_bytes(&split_outputs));

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
fn create_content_policy_is_opt_in_across_cli_create_paths() {
    let dir = temp_dir("content-policy-cli");
    let root = content_policy_tree(&dir, "policy-input");

    let legacy = dir.join("legacy.zip");
    let out = run(sqz()
        .arg("compress")
        .arg(&root)
        .arg("-o")
        .arg(&legacy)
        .args(["--exclude", "*.tmp"]));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let legacy_paths = listed_paths(&legacy);
    assert!(legacy_paths.iter().any(|path| path.ends_with("/.DS_Store")));
    assert!(legacy_paths
        .iter()
        .any(|path| path.ends_with("/._keep.txt")));
    assert!(legacy_paths.iter().any(|path| path.contains("/__MACOSX/")));
    assert!(!legacy_paths.iter().any(|path| path.ends_with(".tmp")));

    let clean = dir.join("clean.zip");
    let out = run(sqz()
        .arg("compress")
        .arg(&root)
        .arg("-o")
        .arg(&clean)
        .args(["--content-policy", "cross-platform-clean"])
        .args(["--exclude", "*.tmp", "--exclude", ".DS_Store"])
        .args(["--exclude", "*.tmp"]));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_cross_platform_clean(&listed_paths(&clean), "policy-input");

    let out = run(sqz()
        .arg("estimate")
        .arg(&root)
        .args(["--content-policy", "cross-platform-clean"])
        .args(["--exclude", "*.tmp"])
        .arg("--json"));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let estimate = stdout_json(&out);
    assert_eq!(estimate["entries"], 3);
    assert_eq!(estimate["files"], 2);
    assert_eq!(estimate["directories"], 1);

    let packed = dir.join("clean.sqz");
    let out = run(sqz()
        .arg("pack")
        .arg(&root)
        .arg("-o")
        .arg(&packed)
        .args(["--content-policy", "cross-platform-clean"])
        .args(["--exclude", "*.tmp"]));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_cross_platform_clean(&listed_paths(&packed), "policy-input");

    let seed = dir.join("seed.txt");
    let updated = dir.join("updated.zip");
    std::fs::write(&seed, b"seed").unwrap();
    let out = run(sqz().arg("compress").arg(&seed).arg("-o").arg(&updated));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let out = run(sqz()
        .arg("update")
        .arg(&updated)
        .arg("--add")
        .arg(&root)
        .args(["--content-policy", "cross-platform-clean"])
        .args(["--exclude", "*.tmp"]));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_cross_platform_clean(&listed_paths(&updated), "policy-input");

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
        25 + 1024 * 1024 + 4 * 1024 + 4096 + 2
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
    assert_eq!(report["plan"]["destination"], dest2.display().to_string());
    assert_eq!(report["plan"]["entries"], 0);
    assert_eq!(
        report["counts"],
        serde_json::json!({
            "destination": dest2.display().to_string(),
            "selected_entries": 0,
            "created": 0,
            "directories": 0,
            "skipped": 0,
            "replaced": 0,
            "renamed": 0,
            "failed": 0,
            "output_bytes": 0
        })
    );
    assert_eq!(report["selected_entries"], 0);
    assert_eq!(report["directories"], 0);
    assert_eq!(report["output_bytes"], 0);
    assert!(!dest2.join("project").exists());

    let occupied_dest = dir.join("occupied-no-match");
    std::fs::write(&occupied_dest, b"keep").unwrap();
    let out = run(sqz()
        .arg("extract")
        .arg(&archive)
        .arg("-d")
        .arg(&occupied_dest)
        .args(["--include", "no/such/entry", "--json"]));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let report = stdout_json(&out);
    assert_eq!(report["matched"], false);
    assert_eq!(report["plan"]["entries"], 0);
    assert_eq!(std::fs::read(&occupied_dest).unwrap(), b"keep");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn extract_smart_no_match_keeps_legacy_dest_and_reports_planned_target() {
    let dir = temp_dir("extract-smart-no-match");
    let input = dir.join("payload.txt");
    let archive = dir.join("loose.zip");
    std::fs::write(&input, b"payload").unwrap();
    let out = run(sqz().arg("compress").arg(&input).arg("-o").arg(&archive));
    assert!(out.status.success(), "compress failed: {}", stderr(&out));

    let dest = dir.join("output");
    let out = run(sqz()
        .args(["--lang", "en-US", "extract"])
        .arg(&archive)
        .arg("-d")
        .arg(&dest)
        .args(["--include", "does/not/exist", "--smart", "--json"]));
    assert!(out.status.success(), "extract failed: {}", stderr(&out));
    let report = stdout_json(&out);
    assert_eq!(report["matched"], false);
    assert_eq!(report["dest"], dest.display().to_string());
    assert_eq!(report["plan"]["layout"], "wrap_in_folder");
    assert_eq!(
        report["plan"]["destination"],
        dest.join("loose").display().to_string()
    );
    assert_eq!(
        report["counts"]["destination"],
        dest.join("loose").display().to_string()
    );
    assert_eq!(report["counts"]["selected_entries"], 0);
    assert_eq!(report["counts"]["created"], 0);
    assert_eq!(report["counts"]["directories"], 0);
    assert_eq!(report["counts"]["skipped"], 0);
    assert_eq!(report["counts"]["replaced"], 0);
    assert_eq!(report["counts"]["renamed"], 0);
    assert_eq!(report["counts"]["failed"], 0);
    assert_eq!(report["counts"]["output_bytes"], 0);
    assert!(!dest.exists());

    let occupied_dest = dir.join("occupied-output");
    std::fs::create_dir_all(&occupied_dest).unwrap();
    let occupied_wrapper = occupied_dest.join("loose");
    std::fs::write(&occupied_wrapper, b"keep").unwrap();
    let out = run(sqz()
        .arg("extract")
        .arg(&archive)
        .arg("-d")
        .arg(&occupied_dest)
        .args(["--include", "does/not/exist", "--smart", "--json"]));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let report = stdout_json(&out);
    assert_eq!(report["matched"], false);
    assert_eq!(
        report["plan"]["destination"],
        occupied_wrapper.display().to_string()
    );
    assert_eq!(std::fs::read(&occupied_wrapper).unwrap(), b"keep");

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
    assert_eq!(report["problems_total"], 1);
    assert_eq!(report["problems_truncated"], false);
    assert_eq!(
        report["counts"]["destination"],
        json_dest.display().to_string()
    );
    assert_eq!(report["counts"]["created"], 1);
    assert_eq!(report["counts"]["skipped"], 0);
    assert_eq!(report["counts"]["replaced"], 0);
    assert_eq!(report["counts"]["renamed"], 0);
    assert_eq!(report["counts"]["failed"], 1);
    assert_eq!(
        report["counts"]["selected_entries"],
        report["plan"]["entries"]
    );
    assert_eq!(report["counts"]["output_bytes"], 9);
    assert_eq!(
        std::fs::read(json_dest.join("src/good.txt")).unwrap(),
        b"good-data"
    );
    assert!(!json_dest.join("src/bad.txt").exists());

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn extract_best_effort_json_bounds_problem_preview_without_losing_total() {
    let dir = temp_dir("best-effort-bounded-problems");
    let root = dir.join("src");
    std::fs::create_dir_all(&root).unwrap();
    let mut payloads = Vec::new();
    for index in 0..25 {
        let payload = format!("squallz-damaged-payload-{index:02}").into_bytes();
        std::fs::write(root.join(format!("damaged-{index:02}.txt")), &payload).unwrap();
        payloads.push(payload);
    }
    let archive = dir.join("damaged.zip");
    let out = run(sqz()
        .arg("compress")
        .arg(&root)
        .arg("-o")
        .arg(&archive)
        .args(["--level", "0"]));
    assert!(out.status.success(), "compress failed: {}", stderr(&out));
    for payload in &payloads {
        corrupt_stored_zip_payload(&archive, payload);
    }

    let out = run(sqz()
        .args(["--lang", "en-US", "test"])
        .arg(&archive)
        .arg("--json"));
    assert_eq!(out.status.code(), Some(3), "stderr: {}", stderr(&out));
    let test_report: serde_json::Value =
        serde_json::from_str(&stdout(&out)).expect("valid test JSON");
    assert_eq!(test_report["ok"], false);
    assert_eq!(test_report["entries_tested"], 26);
    assert_eq!(test_report["problems_total"], 25);
    assert_eq!(test_report["problems_truncated"], true);
    assert_eq!(test_report["problems"].as_array().map(Vec::len), Some(20));

    let out = run(sqz()
        .args(["--lang", "en-US", "--verbose", "test"])
        .arg(&archive));
    assert_eq!(out.status.code(), Some(3), "stderr: {}", stderr(&out));
    assert!(
        stderr(&out).contains("Showing the first 20 problems; 5 more were omitted."),
        "stderr: {}",
        stderr(&out)
    );

    let destination = dir.join("recovered");
    let out = run(sqz()
        .args(["--lang", "en-US", "extract"])
        .arg(&archive)
        .arg("-d")
        .arg(&destination)
        .arg("--best-effort")
        .arg("--json"));
    assert!(
        out.status.success(),
        "best-effort extract failed: {}",
        stderr(&out)
    );
    let report: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    assert_eq!(report["skipped"], 25);
    assert_eq!(report["problems_total"], 25);
    assert_eq!(report["problems_truncated"], true);
    assert_eq!(report["problems"].as_array().map(Vec::len), Some(20));
    assert_eq!(report["counts"]["failed"], 25);
    assert_eq!(report["counts"]["created"], 0);

    let verbose_destination = dir.join("recovered-verbose");
    let out = run(sqz()
        .args(["--lang", "en-US", "--verbose", "extract"])
        .arg(&archive)
        .arg("-d")
        .arg(&verbose_destination)
        .arg("--best-effort"));
    assert!(
        out.status.success(),
        "verbose best-effort extract failed: {}",
        stderr(&out)
    );
    assert!(
        stderr(&out).contains("Best-effort extract skipped 25 unreadable item(s)"),
        "stderr: {}",
        stderr(&out)
    );
    assert!(
        stderr(&out).contains("Showing the first 20 problems; 5 more were omitted."),
        "stderr: {}",
        stderr(&out)
    );

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
fn extract_json_separates_conflict_outcomes_from_legacy_best_effort_skips() {
    let dir = temp_dir("extract-outcome-counts");
    let root = sample_tree(&dir);
    let archive = dir.join("out.zip");
    let out = run(sqz().arg("compress").arg(&root).arg("-o").arg(&archive));
    assert!(out.status.success(), "compress failed: {}", stderr(&out));

    let run_policy = |name: &str, policy: &str| {
        let dest = dir.join(name);
        std::fs::create_dir_all(dest.join("project")).unwrap();
        std::fs::write(dest.join("project/a.txt"), b"KEEP ME").unwrap();
        let out = run(sqz()
            .args(["--lang", "en-US", "extract"])
            .arg(&archive)
            .arg("-d")
            .arg(&dest)
            .args(["--overwrite", policy, "--json"]));
        assert!(
            out.status.success(),
            "{policy} extract failed: {}",
            stderr(&out)
        );
        (dest, stdout_json(&out))
    };

    let (skip_dest, skip) = run_policy("skip", "skip");
    assert_eq!(skip["skipped"], 0, "legacy skipped must stay unchanged");
    assert_eq!(
        skip["counts"]["destination"],
        skip_dest.display().to_string()
    );
    assert_eq!(skip["counts"]["skipped"], 1);
    assert_eq!(skip["counts"]["replaced"], 0);
    assert_eq!(skip["counts"]["renamed"], 0);
    assert_eq!(skip["counts"]["failed"], 0);
    assert_eq!(
        std::fs::read(skip_dest.join("project/a.txt")).unwrap(),
        b"KEEP ME"
    );

    let (replace_dest, replace) = run_policy("replace", "all");
    assert_eq!(replace["skipped"], 0);
    assert_eq!(replace["counts"]["skipped"], 0);
    assert_eq!(replace["counts"]["replaced"], 1);
    assert_eq!(replace["counts"]["renamed"], 0);
    assert_eq!(replace["counts"]["failed"], 0);
    assert_eq!(
        std::fs::read(replace_dest.join("project/a.txt")).unwrap(),
        b"hello world"
    );

    let (rename_dest, rename) = run_policy("rename", "rename");
    assert_eq!(rename["skipped"], 0);
    assert_eq!(rename["counts"]["skipped"], 0);
    assert_eq!(rename["counts"]["replaced"], 0);
    assert_eq!(rename["counts"]["renamed"], 1);
    assert_eq!(rename["counts"]["failed"], 0);
    assert_eq!(
        std::fs::read(rename_dest.join("project/a.txt")).unwrap(),
        b"KEEP ME"
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
        zip["implementation"]["optional_external"]["scope"],
        "native_split_read"
    );
    assert_eq!(
        zip["implementation"]["optional_external"]["env"],
        "SQUALLZ_7Z"
    );
    assert!(zip["implementation"]["optional_external"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .any(|tool| tool == "7zz"));
    let limitations = zip["implementation"]["limitations"].as_array().unwrap();
    let limitation = |scope: &str| {
        limitations
            .iter()
            .find(|limitation| limitation["scope"] == scope)
            .unwrap_or_else(|| panic!("{scope} ZIP limitation missing"))
    };
    assert_eq!(
        limitation("native_split_read")["status"],
        "external_required"
    );
    assert_eq!(limitation("native_split_create")["status"], "built_in");
    assert_eq!(
        limitation("native_split_encrypted")["status"],
        "external_required"
    );
    assert!(zip["implementation"]["release_gate"]
        .as_str()
        .is_some_and(|gate| gate.contains("three-platform filesystem")));
}

#[test]
fn info_json_reports_external_tool_availability() {
    let missing_7z = "/definitely/missing/squallz-test-7z";
    let missing_unrar = "/definitely/missing/squallz-test-unrar";
    let missing_wimlib = "/definitely/missing/squallz-test-wimlib";
    let out = run(sqz()
        .env("SQUALLZ_7Z", missing_7z)
        .env("SQUALLZ_UNRAR", missing_unrar)
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
    let zip_split_read = &find("zip")["implementation"]["optional_external"]["availability"];
    assert_eq!(zip_split_read["available"], false);
    assert_eq!(zip_split_read["configured"], true);
    assert_eq!(zip_split_read["source"], "env");
    assert_eq!(zip_split_read["selected"], missing_7z);
    assert_eq!(zip_split_read["path_exists"], false);
    let rar_policy = &find("rar")["implementation"]["policy"];
    assert_eq!(rar_policy["read_only"], true);
    assert_eq!(rar_policy["bundled"], false);
    assert_eq!(rar_policy["primary_env"], "SQUALLZ_7Z");
    assert_eq!(rar_policy["fallback_env"], "SQUALLZ_BSDTAR");
    assert_eq!(rar_policy["rar7_decoder_env"], "SQUALLZ_UNRAR");
    assert_eq!(
        rar_policy["fallback_scope"],
        "diagnostic_single_file_or_confirmed_unencrypted_rar7_v6"
    );
    assert!(rar_policy["license_boundary"]
        .as_str()
        .is_some_and(|boundary| boundary.contains("does not link or bundle unrar code")));
    let rar7_decoder = &find("rar")["implementation"]["availability"]["rar7_v6_decoder"];
    assert_eq!(rar7_decoder["available"], false);
    assert_eq!(rar7_decoder["configured"], true);
    assert_eq!(rar7_decoder["source"], "env");
    assert_eq!(rar7_decoder["selected"], missing_unrar);
    assert_eq!(rar7_decoder["path_exists"], false);

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
    let unrar = if cfg!(windows) {
        bin.join("unrar.exe")
    } else {
        bin.join("unrar")
    };
    std::fs::write(&tool, "#!/bin/sh\nexit 0\n").unwrap();
    std::fs::write(&unrar, "#!/bin/sh\nexit 0\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&tool).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&tool, perms).unwrap();
        let mut perms = std::fs::metadata(&unrar).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&unrar, perms).unwrap();
    }
    let old_path = std::env::var_os("PATH").unwrap_or_default();
    let path =
        std::env::join_paths(std::iter::once(bin.clone()).chain(std::env::split_paths(&old_path)))
            .unwrap();
    let selected = tool.to_string_lossy().into_owned();
    let selected_unrar = unrar.to_string_lossy().into_owned();

    let out = run(sqz()
        .env_remove("SQUALLZ_7Z")
        .env_remove("SQUALLZ_UNRAR")
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
        "diagnostic_single_file_or_confirmed_unencrypted_rar7_v6"
    );
    let rar7_decoder = &find("rar")["implementation"]["availability"]["rar7_v6_decoder"];
    assert_eq!(rar7_decoder["available"], true);
    assert_eq!(rar7_decoder["source"], "path");
    assert_eq!(
        rar7_decoder["selected"].as_str(),
        Some(selected_unrar.as_str())
    );
    assert_eq!(rar7_decoder["path_exists"], true);

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
        rar_line.contains("external: 7zz/7z; bsdtar and optional unrar fallback"),
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
    assert!(text.contains("optional unrar fallback"), "{text}");
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
    assert_eq!(wim["capabilities"]["can_split"], true);
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
        .is_some_and(|gate| gate.contains("three-platform")));
    let wim_limitations = wim["implementation"]["limitations"]
        .as_array()
        .unwrap_or_else(|| panic!("WIM limitations missing from sqz info"));
    assert!(wim_limitations.iter().any(|item| {
        item["scope"] == "native_split_create" && item["status"] == "external_required"
    }));
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
    assert!(rar["implementation"]["read"]["fallback_tools"]
        .as_array()
        .unwrap()
        .iter()
        .any(|tool| tool == "unrar"));
    assert_eq!(
        rar["implementation"]["read"]["rar7_decoder_env"],
        "SQUALLZ_UNRAR"
    );
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
    assert!(rar_policy["fallback_tools"]
        .as_array()
        .unwrap()
        .iter()
        .any(|tool| tool == "unrar"));
    assert_eq!(rar_policy["rar7_decoder_env"], "SQUALLZ_UNRAR");
    assert_eq!(
        rar_policy["fallback_scope"],
        "diagnostic_single_file_or_confirmed_unencrypted_rar7_v6"
    );
    assert_eq!(
        rar_policy["native_multi_volume"]["encrypted_read"],
        "stdin_only_password_bridge"
    );
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
    assert!(has_rar_limit(
        "encrypted",
        "implemented_not_release_claimed"
    ));
    assert!(has_rar_limit("multi_volume", "not_release_claimed"));
    assert!(has_rar_limit("rar7_v6", "implemented_not_release_claimed"));
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
  printf 'MSWIM\000\000\000\320\000\000\000\000\015\001\000\000\000\000\000\000\200\000\000\000\000\000\000\000\000\000\000\000\000\000\000\000\000\000\000\001\000\001\000' > "$out"
  dd if=/dev/zero bs=164 count=1 >> "$out" 2>/dev/null
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
    let json_outputs = json_output_paths(&report);
    let expected_outputs = (1..=4)
        .map(|index| dir.join(format!("out-json.zip.{index:03}")))
        .collect::<Vec<_>>();
    assert_eq!(json_outputs, expected_outputs);
    assert_eq!(report["total_bytes"], output_paths_bytes(&json_outputs));

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
fn compress_native_zip_split_reports_pkware_volume_family() {
    let dir = temp_dir("native-zip-split-cli");
    let input = incompressible_file_with_len(&dir, "data.bin", 180 * 1024);
    let archive = dir.join("native.zip");

    let out = run(sqz()
        .arg("compress")
        .arg(&input)
        .arg("-o")
        .arg(&archive)
        .args(["--split", "64k", "--split-mode", "native", "--json"]));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let report = stdout_json(&out);
    let outputs = json_output_paths(&report);
    assert_eq!(report["split"], true);
    assert_eq!(report["primary_output"], archive.display().to_string());
    assert_eq!(outputs.first(), Some(&dir.join("native.z01")));
    assert_eq!(outputs.last(), Some(&archive));
    assert!(outputs.len() >= 3);
    assert!(outputs.iter().all(|path| path.is_file()));
    assert!(!dir.join("native.zip.001").exists());

    let missing_size = run(sqz()
        .arg("compress")
        .arg(&input)
        .arg("-o")
        .arg(dir.join("invalid.zip"))
        .args(["--split-mode", "native"]));
    assert!(!missing_size.status.success());
    assert!(
        stderr(&missing_size).contains("--split"),
        "stderr: {}",
        stderr(&missing_size)
    );

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn compress_split_human_output_reports_preserved_previous_outputs() {
    let dir = temp_dir("split-cli-preserved-output");
    let input = incompressible_file(&dir, "data.bin");
    let archive = dir.join("out.zip");

    let first = run(sqz()
        .args(["--lang", "en-US", "--style", "classic"])
        .arg("compress")
        .arg(&input)
        .arg("-o")
        .arg(&archive)
        .args(["--split", "30k"]));
    assert!(first.status.success(), "stderr: {}", stderr(&first));

    let second = run(sqz()
        .args(["--lang", "en-US", "--style", "classic"])
        .arg("compress")
        .arg(&input)
        .arg("-o")
        .arg(&archive)
        .args(["--split", "30k"]));
    assert!(second.status.success(), "stderr: {}", stderr(&second));
    assert!(stdout(&second).contains("Created"));
    let warning = stderr(&second);
    assert!(
        warning.contains("Previous output paths kept")
            && warning.contains("Test the new archive first"),
        "stderr: {warning}"
    );
    let retained = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.to_string_lossy().contains(".split-backup-"))
        .collect::<Vec<_>>();
    assert!(!retained.is_empty());
    for path in retained {
        assert!(
            warning.contains(&path.display().to_string()),
            "classic output omitted preserved path {}: {warning}",
            path.display()
        );
    }

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
fn convert_can_publish_and_report_split_volumes() {
    let dir = temp_dir("convert-cli-split");
    let input = incompressible_file_with_len(&dir, "payload.bin", 400 * 1024);
    let source = dir.join("source.zip");
    let output = dir.join("converted.7z");
    run(sqz().arg("compress").arg(&input).arg("-o").arg(&source));

    let out = run(sqz()
        .args(["--lang", "en-US", "convert"])
        .arg(&source)
        .arg("-o")
        .arg(&output)
        .args(["--split", "100k", "--json"]));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let report: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    let primary = dir.join("converted.7z.001");
    let second = dir.join("converted.7z.002");
    assert_eq!(report["operation"], "convert");
    assert_eq!(report["split"], true);
    assert!(report["volumes"].as_u64().is_some_and(|count| count >= 2));
    assert_eq!(report["primary_output"], primary.display().to_string());
    assert!(report["outputs"]
        .as_array()
        .is_some_and(|outputs| outputs.len() >= 2));
    assert!(!output.exists());
    assert!(primary.is_file());
    assert!(second.is_file());

    let out = run(sqz().arg("list").arg(&primary).arg("--json"));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let entries: serde_json::Value = serde_json::from_str(&stdout(&out)).expect("valid JSON");
    assert!(entries
        .as_array()
        .is_some_and(|entries| entries.iter().any(|entry| entry["path"] == "payload.bin")));

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn convert_and_export_require_explicit_existing_output_authorization() {
    let dir = temp_dir("convert-export-output-policy");
    let root = sample_tree(&dir);
    let zip = dir.join("source.zip");
    let sqz_archive = dir.join("source.sqz");
    run(sqz().arg("compress").arg(&root).arg("-o").arg(&zip));
    run(sqz().arg("compress").arg(&root).arg("-o").arg(&sqz_archive));

    let converted = dir.join("converted.7z");
    std::fs::write(&converted, b"keep converted output").unwrap();
    let out = run(sqz()
        .args(["--lang", "en-US", "convert"])
        .arg(&zip)
        .arg("-o")
        .arg(&converted)
        .arg("--json"));
    assert_json_error(
        &out,
        7,
        "output_exists",
        "output location is already occupied",
    );
    assert_eq!(std::fs::read(&converted).unwrap(), b"keep converted output");

    let out = run(sqz()
        .arg("convert")
        .arg(&zip)
        .arg("-o")
        .arg(&converted)
        .args(["--force", "--json"]));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let out = run(sqz().arg("list").arg(&converted).arg("--json"));
    assert!(out.status.success(), "stderr: {}", stderr(&out));

    let exported = dir.join("exported.zip");
    std::fs::write(&exported, b"keep exported output").unwrap();
    let out = run(sqz()
        .args(["--lang", "en-US", "export"])
        .arg(&sqz_archive)
        .arg("-o")
        .arg(&exported)
        .arg("--json"));
    assert_json_error(
        &out,
        7,
        "output_exists",
        "output location is already occupied",
    );
    assert_eq!(std::fs::read(&exported).unwrap(), b"keep exported output");

    let out = run(sqz()
        .arg("export")
        .arg(&sqz_archive)
        .arg("-o")
        .arg(&exported)
        .args(["--force", "--json"]));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let out = run(sqz().arg("list").arg(&exported).arg("--json"));
    assert!(out.status.success(), "stderr: {}", stderr(&out));

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
    use base64::Engine as _;
    use std::os::unix::fs::PermissionsExt;

    let dir = temp_dir("recovery-cli");
    let archive = dir.join("protected.zip");
    let recovery = dir.join("protected.zip.par2");
    let recovery_fixture = dir.join("protected.zip.fixture.par2");
    let recovery_volume_fixture = dir.join("protected.zip.fixture.vol0+1.par2");
    let multi_first = dir.join("set.zip.001");
    let multi_second = dir.join("set.zip.002");
    let multi_recovery = dir.join("set.zip.par2");
    let multi_recovery_volume = dir.join("set.zip.vol0+4.par2");
    let tool = dir.join("fake-par2");
    let log = dir.join("fake-par2.log");
    std::fs::write(&archive, b"archive bytes").unwrap();
    std::fs::write(&multi_first, b"damaged").unwrap();
    let fixture = base64::engine::general_purpose::STANDARD
        .decode(include_str!("../../squallz-recovery/tests/fixtures/protected.zip.par2.b64").trim())
        .unwrap();
    std::fs::write(&recovery_fixture, fixture).unwrap();
    std::fs::write(
        &recovery_volume_fixture,
        base64::engine::general_purpose::STANDARD
            .decode(
                include_str!("../../squallz-recovery/tests/fixtures/protected.zip.vol0+1.par2.b64")
                    .trim(),
            )
            .unwrap(),
    )
    .unwrap();
    std::fs::write(
        &multi_recovery,
        base64::engine::general_purpose::STANDARD
            .decode(
                include_str!("../../squallz-recovery/tests/fixtures/multi-set.zip.par2.b64").trim(),
            )
            .unwrap(),
    )
    .unwrap();
    std::fs::write(
        &multi_recovery_volume,
        base64::engine::general_purpose::STANDARD
            .decode(
                include_str!("../../squallz-recovery/tests/fixtures/multi-set.zip.vol0+4.par2.b64")
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
    cp "$SQUALLZ_FAKE_PAR2_FIXTURE" "$4"
    cp "$SQUALLZ_FAKE_PAR2_VOLUME_FIXTURE" "${4%.par2}.vol0+1.par2"
    ;;
  verify|repair)
    recovery="$2"
    base=""
    case "$2" in
      -B*)
        base="${2#-B}"
        recovery="$3"
        ;;
    esac
    test -f "$recovery" || exit 2
    if [ "$1" = repair ]; then
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
      if [ -n "${SQUALLZ_FAKE_PAR2_COMPETITOR:-}" ]; then
        printf 'competing output\n' > "$SQUALLZ_FAKE_PAR2_COMPETITOR"
      fi
      if [ -n "${SQUALLZ_FAKE_PAR2_FAIL:-}" ]; then
        exit "$SQUALLZ_FAKE_PAR2_FAIL"
      fi
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
        .env("SQUALLZ_FAKE_PAR2_FIXTURE", &recovery_fixture)
        .env("SQUALLZ_FAKE_PAR2_VOLUME_FIXTURE", &recovery_volume_fixture)
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
    assert_eq!(report["outputs"].as_array().map(Vec::len), Some(2));
    let recovery_path = recovery.to_string_lossy().into_owned();
    assert_eq!(report["recovery"].as_str(), Some(recovery_path.as_str()));
    assert!(recovery.is_file());
    assert!(dir.join("protected.zip.vol0+1.par2").is_file());

    let modern_recovery = dir.join("modern.zip.par2");
    let out = run(sqz()
        .env("SQUALLZ_PAR2", &tool)
        .env("SQUALLZ_FAKE_PAR2_LOG", &log)
        .env("SQUALLZ_FAKE_PAR2_FIXTURE", &recovery_fixture)
        .env("SQUALLZ_FAKE_PAR2_VOLUME_FIXTURE", &recovery_volume_fixture)
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
            && text.contains("Files")
            && text.contains("protect")
            && text.contains("modern.zip.par2")
            && text.contains("modern.zip.vol0+1.par2")
            && text.contains("┬")
            && text.contains("┼"),
        "modern recovery output should show the complete physical output set: {text}"
    );
    assert!(modern_recovery.is_file());
    assert!(dir.join("modern.zip.vol0+1.par2").is_file());

    let classic_recovery = dir.join("classic.zip.par2");
    let out = run(sqz()
        .env("SQUALLZ_PAR2", &tool)
        .env("SQUALLZ_FAKE_PAR2_LOG", &log)
        .env("SQUALLZ_FAKE_PAR2_FIXTURE", &recovery_fixture)
        .env("SQUALLZ_FAKE_PAR2_VOLUME_FIXTURE", &recovery_volume_fixture)
        .args(["--lang", "en-US", "--style", "classic"])
        .arg("protect")
        .arg(&archive)
        .arg("--recovery")
        .arg(&classic_recovery)
        .args(["--redundancy", "12%"]));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let text = stdout(&out);
    assert!(
        text.contains("Files:")
            && text.contains(classic_recovery.to_string_lossy().as_ref())
            && text.contains("classic.zip.vol0+1.par2"),
        "classic recovery output should show the complete physical output set: {text}"
    );
    assert!(classic_recovery.is_file());
    assert!(dir.join("classic.zip.vol0+1.par2").is_file());

    let out = run(sqz()
        .env("SQUALLZ_PAR2", &tool)
        .env("SQUALLZ_FAKE_PAR2_LOG", &log)
        .env("SQUALLZ_FAKE_PAR2_FIXTURE", &recovery_fixture)
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
        .env("SQUALLZ_FAKE_PAR2_FIXTURE", &recovery_fixture)
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
        .env("SQUALLZ_FAKE_PAR2_FIXTURE", &recovery_fixture)
        .args(["--lang", "en-US"])
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

    let out = run(sqz()
        .env("SQUALLZ_PAR2", &tool)
        .env("SQUALLZ_FAKE_PAR2_LOG", &log)
        .env("SQUALLZ_FAKE_PAR2_FIXTURE", &recovery_fixture)
        .args(["--lang", "en-US", "--style", "modern", "--color", "never"])
        .arg("repair")
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
            && text.contains("repair")
            && text.contains("┬")
            && text.contains("┼"),
        "modern recovery repair output should use a status panel and table: {text}"
    );
    assert_eq!(std::fs::read(&archive).unwrap(), b"archive bytes");

    let copy_output = dir.join("restored.zip");
    let out = run(sqz()
        .env("SQUALLZ_PAR2", &tool)
        .env("SQUALLZ_FAKE_PAR2_LOG", &log)
        .env("SQUALLZ_FAKE_PAR2_FIXTURE", &recovery_fixture)
        .args(["--lang", "en-US"])
        .arg("repair")
        .arg(&archive)
        .arg("--use-recovery")
        .arg("--recovery")
        .arg(&recovery)
        .arg("--output")
        .arg(&copy_output)
        .arg("--json"));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let report: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(report["ok"], true);
    assert_eq!(report["operation"], "repair");
    let output_path = copy_output.to_string_lossy().into_owned();
    assert_eq!(report["output"].as_str(), Some(output_path.as_str()));
    assert_eq!(std::fs::read(&archive).unwrap(), b"archive bytes");
    assert_eq!(std::fs::read(&copy_output).unwrap(), b"archive bytes");

    let multi_output = dir.join("restored-set");
    let out = run(sqz()
        .env("SQUALLZ_PAR2", &tool)
        .env("SQUALLZ_FAKE_PAR2_LOG", &log)
        .arg("repair")
        .arg(&multi_first)
        .arg("--use-recovery")
        .arg("--recovery")
        .arg(&multi_recovery)
        .arg("--output-dir")
        .arg(&multi_output)
        .arg("--json"));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let report: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(report["ok"], true);
    assert_eq!(report["operation"], "repair");
    assert_eq!(report["source_file_count"], 2);
    assert_eq!(
        report["output"].as_str(),
        Some(multi_output.to_string_lossy().as_ref())
    );
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

    let batch_output = dir.join("batch-restored-set");
    let batch_script = dir.join("repair-set-batch.json");
    std::fs::write(
        &batch_script,
        serde_json::json!({
            "jobs": [{
                "id": "repair-set",
                "operation": "repair_recovery",
                "archive": multi_first,
                "recovery_path": multi_recovery,
                "output_dir": batch_output
            }]
        })
        .to_string(),
    )
    .unwrap();
    let out = run(sqz()
        .env("SQUALLZ_PAR2", &tool)
        .env("SQUALLZ_FAKE_PAR2_LOG", &log)
        .arg("batch")
        .arg(&batch_script)
        .arg("--json"));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let report: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(report["jobs"][0]["ok"], true);
    assert_eq!(report["jobs"][0]["result"]["source_file_count"], 2);
    assert_eq!(
        std::fs::read(batch_output.join("set.zip.001")).unwrap(),
        b"first-volume-original\n"
    );
    assert_eq!(
        std::fs::read(batch_output.join("set.zip.002")).unwrap(),
        b"second-volume-original\n"
    );

    let source_before_conflicts = std::fs::read(&archive).unwrap();
    std::fs::write(&copy_output, b"existing output\n").unwrap();
    let log_before_existing_conflict = std::fs::read_to_string(&log).unwrap();
    let out = run(sqz()
        .env("SQUALLZ_PAR2", &tool)
        .env("SQUALLZ_FAKE_PAR2_LOG", &log)
        .env("SQUALLZ_FAKE_PAR2_FIXTURE", &recovery_fixture)
        .args(["--lang", "en-US"])
        .arg("repair")
        .arg(&archive)
        .arg("--use-recovery")
        .arg("--recovery")
        .arg(&recovery)
        .arg("--output")
        .arg(&copy_output)
        .arg("--json"));
    assert_json_error(
        &out,
        7,
        "output_exists",
        "output location is already occupied",
    );
    assert_eq!(std::fs::read(&copy_output).unwrap(), b"existing output\n");
    assert_eq!(std::fs::read(&archive).unwrap(), source_before_conflicts);
    assert_eq!(
        std::fs::read_to_string(&log).unwrap(),
        log_before_existing_conflict,
        "an existing output should be rejected before running PAR2"
    );

    let late_output = dir.join("late-restored.zip");
    let out = run(sqz()
        .env("SQUALLZ_PAR2", &tool)
        .env("SQUALLZ_FAKE_PAR2_LOG", &log)
        .env("SQUALLZ_FAKE_PAR2_FIXTURE", &recovery_fixture)
        .env("SQUALLZ_FAKE_PAR2_COMPETITOR", &late_output)
        .args(["--lang", "en-US"])
        .arg("repair")
        .arg(&archive)
        .arg("--use-recovery")
        .arg("--recovery")
        .arg(&recovery)
        .arg("--output")
        .arg(&late_output)
        .arg("--json"));
    assert_json_error(
        &out,
        7,
        "output_exists",
        "output location is already occupied",
    );
    assert_eq!(std::fs::read(&late_output).unwrap(), b"competing output\n");
    assert_eq!(std::fs::read(&archive).unwrap(), source_before_conflicts);

    let failed_output = dir.join("failed-restored.zip");
    let out = run(sqz()
        .env("SQUALLZ_PAR2", &tool)
        .env("SQUALLZ_FAKE_PAR2_LOG", &log)
        .env("SQUALLZ_FAKE_PAR2_FIXTURE", &recovery_fixture)
        .env("SQUALLZ_FAKE_PAR2_FAIL", "9")
        .args(["--lang", "en-US"])
        .arg("repair")
        .arg(&archive)
        .arg("--use-recovery")
        .arg("--recovery")
        .arg(&recovery)
        .arg("--output")
        .arg(&failed_output)
        .arg("--json"));
    assert_eq!(out.status.code(), Some(3), "stdout: {}", stdout(&out));
    let failed_report: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(failed_report["ok"], false);
    assert_eq!(failed_report["output"], serde_json::Value::Null);
    assert_eq!(failed_report["status_code"], 9);
    assert!(!failed_output.exists());
    assert_eq!(std::fs::read(&archive).unwrap(), source_before_conflicts);
    assert!(std::fs::read_dir(&dir).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains(".sqz-par2-repair-")
    }));

    let log = std::fs::read_to_string(&log).unwrap();
    assert!(log.contains("create -r12"), "log: {log}");
    assert!(log.contains("verify"), "log: {log}");
    assert!(log.contains("repair"), "log: {log}");
    assert!(std::fs::read_dir(&dir).unwrap().all(|entry| {
        let name = entry.unwrap().file_name();
        let name = name.to_string_lossy();
        !name.contains(".sqz-par2-protect-") && !name.ends_with(".squallz-output-set.json")
    }));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[cfg(unix)]
#[test]
fn protect_rejects_existing_destinations_and_unexpected_backend_outputs() {
    use base64::Engine as _;
    use std::os::unix::fs::PermissionsExt;

    let dir = temp_dir("recovery-protect-boundaries");
    let archive = dir.join("protected.zip");
    let recovery = dir.join("protected.zip.par2");
    let volume = dir.join("protected.zip.vol0+1.par2");
    let fixture = dir.join("fixture.par2");
    let volume_fixture = dir.join("fixture.vol0+1.par2");
    let tool = dir.join("fake-par2");
    let log = dir.join("fake-par2.log");
    std::fs::write(&archive, b"archive bytes").unwrap();
    std::fs::write(
        &fixture,
        base64::engine::general_purpose::STANDARD
            .decode(
                include_str!("../../squallz-recovery/tests/fixtures/protected.zip.par2.b64").trim(),
            )
            .unwrap(),
    )
    .unwrap();
    std::fs::write(
        &volume_fixture,
        base64::engine::general_purpose::STANDARD
            .decode(
                include_str!("../../squallz-recovery/tests/fixtures/protected.zip.vol0+1.par2.b64")
                    .trim(),
            )
            .unwrap(),
    )
    .unwrap();
    std::fs::write(
        &tool,
        r#"#!/bin/sh
echo "$*" >> "$SQUALLZ_FAKE_PAR2_LOG"
cp "$SQUALLZ_FAKE_PAR2_FIXTURE" "$4"
cp "$SQUALLZ_FAKE_PAR2_VOLUME_FIXTURE" "${4%.par2}.vol0+1.par2"
printf 'foreign output\n' > "$(dirname "$4")/foreign.tmp"
"#,
    )
    .unwrap();
    let mut permissions = std::fs::metadata(&tool).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&tool, permissions).unwrap();

    std::fs::write(&recovery, b"existing recovery").unwrap();
    let out = run(sqz()
        .env("SQUALLZ_PAR2", &tool)
        .env("SQUALLZ_FAKE_PAR2_LOG", &log)
        .env("SQUALLZ_FAKE_PAR2_FIXTURE", &fixture)
        .env("SQUALLZ_FAKE_PAR2_VOLUME_FIXTURE", &volume_fixture)
        .arg("protect")
        .arg(&archive)
        .arg("--recovery")
        .arg(&recovery)
        .arg("--json"));
    assert!(!out.status.success(), "stdout: {}", stdout(&out));
    assert_eq!(std::fs::read(&recovery).unwrap(), b"existing recovery");
    assert!(
        !log.exists(),
        "the backend must not start for an occupied output"
    );

    std::fs::remove_file(&recovery).unwrap();
    let out = run(sqz()
        .env("SQUALLZ_PAR2", &tool)
        .env("SQUALLZ_FAKE_PAR2_LOG", &log)
        .env("SQUALLZ_FAKE_PAR2_FIXTURE", &fixture)
        .env("SQUALLZ_FAKE_PAR2_VOLUME_FIXTURE", &volume_fixture)
        .arg("protect")
        .arg(&archive)
        .arg("--recovery")
        .arg(&recovery)
        .arg("--json"));
    assert!(!out.status.success(), "stdout: {}", stdout(&out));
    assert!(!recovery.exists());
    assert!(!volume.exists());
    assert_eq!(std::fs::read(&archive).unwrap(), b"archive bytes");
    assert!(std::fs::read_dir(&dir).unwrap().all(|entry| {
        let name = entry.unwrap().file_name();
        let name = name.to_string_lossy();
        !name.contains(".sqz-par2-protect-") && !name.ends_with(".squallz-output-set.json")
    }));

    std::fs::remove_dir_all(dir).unwrap();
}

#[cfg(unix)]
#[test]
fn protect_tolerate_loss_maps_split_volumes_to_redundancy() {
    use base64::Engine as _;
    use std::os::unix::fs::PermissionsExt;

    let dir = temp_dir("recovery-tolerate-loss");
    let first = dir.join("set.zip.001");
    let second = dir.join("set.zip.002");
    std::fs::write(&first, b"first-volume-original\n").unwrap();
    std::fs::write(&second, b"second-volume-original\n").unwrap();
    let recovery = dir.join("set.zip.par2");
    let recovery_fixture = dir.join("set.zip.fixture.par2");
    let recovery_volume_fixture = dir.join("set.zip.fixture.vol0+4.par2");
    std::fs::write(
        &recovery_fixture,
        base64::engine::general_purpose::STANDARD
            .decode(
                include_str!("../../squallz-recovery/tests/fixtures/multi-set.zip.par2.b64").trim(),
            )
            .unwrap(),
    )
    .unwrap();
    std::fs::write(
        &recovery_volume_fixture,
        base64::engine::general_purpose::STANDARD
            .decode(
                include_str!("../../squallz-recovery/tests/fixtures/multi-set.zip.vol0+4.par2.b64")
                    .trim(),
            )
            .unwrap(),
    )
    .unwrap();
    let tool = dir.join("fake-par2");
    let log = dir.join("fake-par2.log");
    std::fs::write(
        &tool,
        r#"#!/bin/sh
echo "$*" >> "$SQUALLZ_FAKE_PAR2_LOG"
case "$1" in
  create)
    cp "$SQUALLZ_FAKE_PAR2_FIXTURE" "$4"
    cp "$SQUALLZ_FAKE_PAR2_VOLUME_FIXTURE" "${4%.par2}.vol0+4.par2"
    ;;
  verify)
    test -f "$3"
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
        .env("SQUALLZ_FAKE_PAR2_FIXTURE", &recovery_fixture)
        .env("SQUALLZ_FAKE_PAR2_VOLUME_FIXTURE", &recovery_volume_fixture)
        .arg("protect")
        .arg(&first)
        .arg("--tolerate-loss")
        .arg("1volume")
        .arg("--recovery")
        .arg(&recovery)
        .arg("--json"));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let report: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(report["ok"], true);
    assert_eq!(report["operation"], "protect");
    assert_eq!(report["redundancy_percent"], 52);
    assert!(recovery.is_file());

    let log_text = std::fs::read_to_string(&log).unwrap();
    assert!(log_text.contains("create -r52"), "log: {log_text}");
    assert!(
        log_text.contains(first.to_string_lossy().as_ref()),
        "log: {log_text}"
    );
    assert!(
        log_text.contains(second.to_string_lossy().as_ref()),
        "log: {log_text}"
    );
    let batch_recovery = dir.join("set-batch.par2");
    let batch_script = dir.join("protect-batch.json");
    std::fs::write(
        &batch_script,
        serde_json::json!({
            "jobs": [{
                "id": "protect-volume-set",
                "operation": "protect",
                "archive": second,
                "recovery_path": batch_recovery,
                "redundancy": 25
            }]
        })
        .to_string(),
    )
    .unwrap();
    let out = run(sqz()
        .env("SQUALLZ_PAR2", &tool)
        .env("SQUALLZ_FAKE_PAR2_LOG", &log)
        .env("SQUALLZ_FAKE_PAR2_FIXTURE", &recovery_fixture)
        .env("SQUALLZ_FAKE_PAR2_VOLUME_FIXTURE", &recovery_volume_fixture)
        .arg("batch")
        .arg(&batch_script)
        .arg("--json"));
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let report: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(report["jobs"][0]["ok"], true);
    assert_eq!(report["jobs"][0]["result"]["redundancy_percent"], 25);
    assert!(batch_recovery.is_file());
    let log_text = std::fs::read_to_string(&log).unwrap();
    assert!(log_text.contains("create -r25"), "log: {log_text}");
    assert!(
        log_text.contains(first.to_string_lossy().as_ref()),
        "log: {log_text}"
    );
    assert!(
        log_text.contains(second.to_string_lossy().as_ref()),
        "log: {log_text}"
    );
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
        "--tolerate-loss requires a multi-file archive set",
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

#[test]
fn sfx_create_inspect_open_and_tamper_detection_are_wired_through_cli() {
    let dir = temp_dir("sfx-cli");
    let input = dir.join("readme.txt");
    let archive = dir.join("payload.zip");
    let stub = write_sfx_pe_stub(&dir);
    let output = dir.join("package.exe");
    std::fs::write(&input, b"Squallz self extractor").unwrap();

    let compressed = run(sqz().arg("compress").arg(&input).arg("-o").arg(&archive));
    assert!(
        compressed.status.success(),
        "stderr: {}",
        stderr(&compressed)
    );

    let created = run(sqz()
        .args(["--lang", "en-US", "sfx", "create"])
        .arg(&archive)
        .arg("--target")
        .arg("windows")
        .arg("--stub")
        .arg(&stub)
        .arg("-o")
        .arg(&output)
        .arg("--json"));
    assert!(created.status.success(), "stderr: {}", stderr(&created));
    let report = stdout_json(&created);
    assert_eq!(report["operation"], "sfx_create");
    assert_eq!(report["target"], "windows");
    assert_eq!(report["requires_signing"], true);
    assert_eq!(report["preserved_outputs"], serde_json::json!([]));
    assert_eq!(report["auto_run"], false);

    let replaced = run(sqz()
        .args(["--lang", "en-US", "sfx", "create"])
        .arg(&archive)
        .arg("--target")
        .arg("windows")
        .arg("--stub")
        .arg(&stub)
        .arg("-o")
        .arg(&output)
        .arg("--force")
        .arg("--json"));
    assert!(replaced.status.success(), "stderr: {}", stderr(&replaced));
    let replacement_report = stdout_json(&replaced);
    let preserved = replacement_report["preserved_outputs"]
        .as_array()
        .expect("SFX replacement backup paths");
    assert_eq!(preserved.len(), 1);
    let preserved_path = PathBuf::from(preserved[0].as_str().expect("SFX backup path"));
    assert!(preserved_path.exists());
    std::fs::remove_file(&preserved_path).unwrap();

    let replaced_human = run(sqz()
        .args(["--lang", "en-US", "sfx", "create"])
        .arg(&archive)
        .arg("--target")
        .arg("windows")
        .arg("--stub")
        .arg(&stub)
        .arg("-o")
        .arg(&output)
        .arg("--force"));
    assert!(
        replaced_human.status.success(),
        "stderr: {}",
        stderr(&replaced_human)
    );
    let human_stderr = stderr(&replaced_human);
    assert!(human_stderr.contains("Previous output paths kept: 1"));
    assert!(human_stderr.contains("Test the new archive first"));
    assert!(human_stderr.contains(".squallz-sfx-"));
    assert!(human_stderr.contains("previous"));

    let inspected = run(sqz()
        .args(["--lang", "en-US", "sfx", "inspect"])
        .arg(&output)
        .arg("--json"));
    assert!(inspected.status.success(), "stderr: {}", stderr(&inspected));
    let report = stdout_json(&inspected);
    assert_eq!(report["operation"], "sfx_inspect");
    assert_eq!(report["checksum_verified"], true);

    let listed = run(sqz().arg("list").arg(&output).arg("--json"));
    assert!(listed.status.success(), "stderr: {}", stderr(&listed));
    let entries = stdout_json(&listed);
    assert_eq!(entries[0]["path"], "readme.txt");

    let dest = dir.join("extracted");
    let extracted = run(sqz()
        .arg("extract")
        .arg(&output)
        .arg("-d")
        .arg(&dest)
        .arg("--json"));
    assert!(extracted.status.success(), "stderr: {}", stderr(&extracted));
    assert_eq!(
        std::fs::read(dest.join("readme.txt")).unwrap(),
        b"Squallz self extractor"
    );

    let info = squallz_core::inspect_sfx(&output).unwrap().unwrap();
    let mut bytes = std::fs::read(&output).unwrap();
    bytes[(info.payload_offset + 8) as usize] ^= 0x5a;
    std::fs::write(&output, bytes).unwrap();
    let tampered = run(sqz()
        .args(["--lang", "en-US", "sfx", "inspect"])
        .arg(&output)
        .arg("--json"));
    assert_json_error(&tampered, 3, "corrupt_archive", "checksum mismatch");

    std::fs::remove_dir_all(dir).unwrap();
}

#[cfg(all(unix, not(target_os = "macos")))]
#[test]
fn sfx_create_json_handles_a_non_utf8_output_path_without_panicking() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let dir = temp_dir("sfx-cli-non-utf8-json");
    let input = dir.join("readme.txt");
    let archive = dir.join("payload.zip");
    let stub = write_sfx_pe_stub(&dir);
    let output = dir.join(OsString::from_vec(b"package-\xff.exe".to_vec()));
    std::fs::write(&input, b"non-UTF-8 output path").unwrap();

    let compressed = run(sqz().arg("compress").arg(&input).arg("-o").arg(&archive));
    assert!(
        compressed.status.success(),
        "stderr: {}",
        stderr(&compressed)
    );
    let created = run(sqz()
        .args(["--lang", "en-US", "sfx", "create"])
        .arg(&archive)
        .arg("--target")
        .arg("windows")
        .arg("--stub")
        .arg(&stub)
        .arg("-o")
        .arg(&output)
        .arg("--json"));

    assert!(created.status.success(), "stderr: {}", stderr(&created));
    assert!(output.is_file());
    let report = stdout_json(&created);
    assert_eq!(report["ok"], true);
    assert_eq!(report["operation"], "sfx_create");
    assert!(report["path"].as_str().is_some());

    std::fs::remove_dir_all(&dir).unwrap();
}

#[cfg(target_os = "macos")]
#[test]
fn sfx_create_json_reports_an_unsupported_non_utf8_output_path_without_panicking() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let dir = temp_dir("sfx-cli-non-utf8-json");
    let input = dir.join("readme.txt");
    let archive = dir.join("payload.zip");
    let stub = write_sfx_pe_stub(&dir);
    let output = dir.join(OsString::from_vec(b"package-\xff.exe".to_vec()));
    std::fs::write(&input, b"non-UTF-8 output path").unwrap();

    let compressed = run(sqz().arg("compress").arg(&input).arg("-o").arg(&archive));
    assert!(
        compressed.status.success(),
        "stderr: {}",
        stderr(&compressed)
    );
    let created = run(sqz()
        .args(["--lang", "en-US", "sfx", "create"])
        .arg(&archive)
        .arg("--target")
        .arg("windows")
        .arg("--stub")
        .arg(&stub)
        .arg("-o")
        .arg(&output)
        .arg("--json"));

    assert_eq!(created.status.code(), Some(7));
    assert!(stderr(&created).trim().is_empty());
    let report = stdout_json(&created);
    assert_eq!(report["ok"], false);
    assert_eq!(report["error"]["kind"], "io");
    assert_eq!(report["error"]["exit_code"], 7);
    assert!(!output.exists());

    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn macos_single_file_sfx_has_an_explicit_cli_boundary() {
    let dir = temp_dir("sfx-macos-cli");
    let input = dir.join("readme.txt");
    let archive = dir.join("payload.zip");
    let output = dir.join("package");
    std::fs::write(&input, b"payload").unwrap();
    let compressed = run(sqz().arg("compress").arg(&input).arg("-o").arg(&archive));
    assert!(compressed.status.success());

    let result = run(sqz()
        .args(["--lang", "en-US", "sfx", "create"])
        .arg(&archive)
        .arg("--target")
        .arg("macos")
        .arg("-o")
        .arg(&output)
        .arg("--json"));
    assert_json_error(&result, 2, "unsupported", "requires --stub Squallz.app");
    assert!(!output.exists());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn macos_sfx_app_bundle_create_and_inspect_are_wired_through_cli() {
    let dir = temp_dir("sfx-macos-app-cli");
    let input = dir.join("readme.txt");
    let archive = dir.join("payload.zip");
    let stub = write_sfx_macos_app_stub(&dir);
    let output = dir.join("Release.app");
    std::fs::write(&input, b"macOS bundle payload").unwrap();
    let compressed = run(sqz().arg("compress").arg(&input).arg("-o").arg(&archive));
    assert!(
        compressed.status.success(),
        "stderr: {}",
        stderr(&compressed)
    );

    let created = run(sqz()
        .args(["--lang", "en-US", "sfx", "create"])
        .arg(&archive)
        .args(["--target", "macos"])
        .arg("--stub")
        .arg(&stub)
        .arg("-o")
        .arg(&output)
        .arg("--json"));
    assert!(created.status.success(), "stderr: {}", stderr(&created));
    let report = stdout_json(&created);
    assert_eq!(report["target"], "macos");
    assert_eq!(report["layout"], "macos_app");
    assert_eq!(report["requires_signing"], true);
    assert!(report["payload_sha256"].as_str().is_some());

    let inspected = run(sqz()
        .args(["--lang", "en-US", "sfx", "inspect"])
        .arg(&output)
        .arg("--json"));
    assert!(inspected.status.success(), "stderr: {}", stderr(&inspected));
    let report = stdout_json(&inspected);
    assert_eq!(report["layout"], "macos_app");
    assert_eq!(report["checksum_verified"], true);
    std::fs::remove_dir_all(dir).unwrap();
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
#[test]
fn host_sfx_stub_executes_list_and_extract_runtime() {
    let dir = temp_dir("sfx-host-runtime");
    let input = dir.join("runtime.txt");
    let archive = dir.join("payload.zip");
    let target = if cfg!(target_os = "windows") {
        "windows"
    } else {
        "linux"
    };
    let output = if cfg!(target_os = "windows") {
        dir.join("runtime.exe")
    } else {
        dir.join("runtime.run")
    };
    std::fs::write(&input, b"runtime payload").unwrap();
    let compressed = run(sqz().arg("compress").arg(&input).arg("-o").arg(&archive));
    assert!(compressed.status.success());

    let created = run(sqz()
        .arg("sfx")
        .arg("create")
        .arg(&archive)
        .arg("--target")
        .arg(target)
        .arg("-o")
        .arg(&output)
        .arg("--json"));
    assert!(created.status.success(), "stderr: {}", stderr(&created));

    let listed = run(sfx_command(&output).args(["--list", "--json"]));
    assert!(listed.status.success(), "stderr: {}", stderr(&listed));
    let entries = stdout_json(&listed);
    assert_eq!(entries[0]["path"], "runtime.txt");

    let dest = dir.join("runtime-out");
    let extracted = run(sfx_command(&output).arg("-d").arg(&dest).arg("--json"));
    assert!(extracted.status.success(), "stderr: {}", stderr(&extracted));
    assert_eq!(
        std::fs::read(dest.join("runtime.txt")).unwrap(),
        b"runtime payload"
    );
    std::fs::remove_dir_all(dir).unwrap();
}
