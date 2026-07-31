//! SFX v1 integration tests: ZIP payload assembly, transparent archive
//! access, footer verification and shared extraction safety.

mod common;

use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Barrier,
};

use common::{build_stored_zip, command_exists, engine, RawZipEntry, TempDir};
use squallz_core::api::{
    ControlToken, CreateOptions, EntryPath, ExtractOptions, FormatError, NoProgress, OpenOptions,
    Password, ProgressSink,
};
use squallz_core::{
    inspect_sfx, verify_sfx_payload, SfxBuildOptions, SfxLayout, SfxTarget, SFX_CLI_STUB_MARKER,
    SFX_GUI_STUB_MARKER,
};

fn write_pe_stub(path: &Path) {
    let mut bytes = vec![0u8; 512];
    bytes[..2].copy_from_slice(b"MZ");
    bytes[0x3c..0x40].copy_from_slice(&0x80u32.to_le_bytes());
    bytes[0x80..0x84].copy_from_slice(b"PE\0\0");
    bytes[0x94..0x96].copy_from_slice(&240u16.to_le_bytes());
    bytes[0x98..0x9a].copy_from_slice(&0x20bu16.to_le_bytes());
    bytes[0x104..0x108].copy_from_slice(&16u32.to_le_bytes());
    bytes[0x190..0x190 + SFX_CLI_STUB_MARKER.len()].copy_from_slice(&SFX_CLI_STUB_MARKER);
    fs::write(path, bytes).unwrap();
}

fn write_pe_stub_without_marker(path: &Path) {
    let mut bytes = vec![0u8; 512];
    bytes[..2].copy_from_slice(b"MZ");
    bytes[0x3c..0x40].copy_from_slice(&0x80u32.to_le_bytes());
    bytes[0x80..0x84].copy_from_slice(b"PE\0\0");
    bytes[0x94..0x96].copy_from_slice(&240u16.to_le_bytes());
    bytes[0x98..0x9a].copy_from_slice(&0x20bu16.to_le_bytes());
    bytes[0x104..0x108].copy_from_slice(&16u32.to_le_bytes());
    fs::write(path, bytes).unwrap();
}

fn append_fake_authenticode_certificate(path: &Path) {
    let mut bytes = fs::read(path).unwrap();
    let padding = (8 - (bytes.len() % 8)) % 8;
    bytes.resize(bytes.len() + padding, 0);
    let certificate_offset = bytes.len() as u32;
    let mut certificate = [0u8; 16];
    certificate[..4].copy_from_slice(&16u32.to_le_bytes());
    certificate[4..6].copy_from_slice(&0x0200u16.to_le_bytes());
    certificate[6..8].copy_from_slice(&0x0002u16.to_le_bytes());
    bytes.extend_from_slice(&certificate);
    let certificate_entry = 0x98 + 112 + 4 * 8;
    bytes[certificate_entry..certificate_entry + 4]
        .copy_from_slice(&certificate_offset.to_le_bytes());
    bytes[certificate_entry + 4..certificate_entry + 8]
        .copy_from_slice(&(certificate.len() as u32).to_le_bytes());
    fs::write(path, bytes).unwrap();
}

fn write_macho_stub(path: &Path) {
    let mut bytes = vec![0u8; 256];
    bytes[..4].copy_from_slice(&[0xcf, 0xfa, 0xed, 0xfe]);
    bytes[0x80..0x80 + SFX_CLI_STUB_MARKER.len()].copy_from_slice(&SFX_CLI_STUB_MARKER);
    fs::write(path, bytes).unwrap();
}

fn write_macos_app_template(path: &Path) {
    let executable = path.join("Contents/MacOS/squallz-gui");
    fs::create_dir_all(executable.parent().unwrap()).unwrap();
    fs::create_dir_all(path.join("Contents/Resources/en.lproj")).unwrap();
    let mut bytes = vec![0u8; 512];
    bytes[..4].copy_from_slice(&[0xcf, 0xfa, 0xed, 0xfe]);
    bytes[0x80..0x80 + SFX_GUI_STUB_MARKER.len()].copy_from_slice(&SFX_GUI_STUB_MARKER);
    fs::write(&executable, bytes).unwrap();
    fs::write(
        path.join("Contents/Info.plist"),
        r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>squallz-gui</string>
<key>LSMinimumSystemVersion</key><string>11.0</string>
</dict></plist>
"#,
    )
    .unwrap();
    fs::write(
        path.join("Contents/Resources/en.lproj/InfoPlist.strings"),
        "\"CFBundleName\" = \"Squallz\";\n",
    )
    .unwrap();
}

fn sample_zip(path: &Path) {
    fs::write(
        path,
        build_stored_zip(&[RawZipEntry {
            name: b"docs/readme.txt".to_vec(),
            data: b"Squallz SFX payload".to_vec(),
        }]),
    )
    .unwrap();
}

#[cfg(unix)]
fn allocated_tree_bytes(root: &Path) -> u64 {
    use std::os::unix::fs::MetadataExt;

    let mut total = 0u64;
    let mut pending = vec![root.to_path_buf()];
    while let Some(path) = pending.pop() {
        let metadata = fs::symlink_metadata(&path).unwrap();
        total = total.saturating_add(metadata.blocks().saturating_mul(512));
        if metadata.is_dir() && !metadata.file_type().is_symlink() {
            pending.extend(
                fs::read_dir(path)
                    .unwrap()
                    .map(|entry| entry.unwrap().path()),
            );
        }
    }
    total
}

struct CancelOnProgress {
    ctl: Arc<ControlToken>,
}

impl ProgressSink for CancelOnProgress {
    fn on_progress(&self, done: u64, _total: u64, _current: &EntryPath) {
        if done > 0 {
            self.ctl.cancel();
        }
    }
}

struct OnceOnProgress<F> {
    fired: AtomicBool,
    action: F,
}

impl<F> OnceOnProgress<F> {
    fn new(action: F) -> Self {
        Self {
            fired: AtomicBool::new(false),
            action,
        }
    }
}

impl<F> ProgressSink for OnceOnProgress<F>
where
    F: Fn() + Send + Sync,
{
    fn on_progress(&self, done: u64, _total: u64, _current: &EntryPath) {
        if done > 0 && !self.fired.swap(true, Ordering::AcqRel) {
            (self.action)();
        }
    }
}

struct OnceOnEntryProgress<F> {
    entry: String,
    fired: AtomicBool,
    action: F,
}

impl<F> OnceOnEntryProgress<F> {
    fn new(entry: &str, action: F) -> Self {
        Self {
            entry: entry.to_owned(),
            fired: AtomicBool::new(false),
            action,
        }
    }
}

impl<F> ProgressSink for OnceOnEntryProgress<F>
where
    F: Fn() + Send + Sync,
{
    fn on_progress(&self, _done: u64, _total: u64, _current: &EntryPath) {}

    fn on_entry_progress(
        &self,
        _done: u64,
        _total: u64,
        current: &EntryPath,
        current_done: u64,
        _current_total: u64,
    ) {
        if current_done > 0
            && current.display == self.entry
            && !self.fired.swap(true, Ordering::AcqRel)
        {
            (self.action)();
        }
    }
}

fn assert_no_private_sfx_staging(root: &Path) {
    assert!(fs::read_dir(root).unwrap().all(|entry| {
        let name = entry.unwrap().file_name();
        let name = name.to_string_lossy();
        !name.starts_with(".squallz-sfx-payload-") && !name.starts_with(".squallz-sfx-stage-")
    }));
}

#[cfg(unix)]
fn private_sfx_stage(root: &Path) -> std::path::PathBuf {
    fs::read_dir(root)
        .unwrap()
        .map(|entry| entry.unwrap())
        .find(|entry| {
            entry.file_type().unwrap().is_dir()
                && entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".squallz-sfx-stage-")
        })
        .map(|entry| entry.path())
        .unwrap()
}

fn create_macos_sfx_from_input(
    template: &Path,
    input: &Path,
    output: &Path,
    overwrite: bool,
    progress: &dyn ProgressSink,
) -> Result<(), FormatError> {
    engine()
        .create_sfx_from_inputs(
            template,
            &[input.to_path_buf()],
            output,
            &CreateOptions::default(),
            &SfxBuildOptions {
                target: SfxTarget::Macos,
                overwrite,
                ..SfxBuildOptions::default()
            },
            progress,
            &ControlToken::new(),
        )
        .map(|_| ())
}

fn rejected_plan_before_input_scan(
    stub: &Path,
    input: &Path,
    output: &Path,
    target: SfxTarget,
) -> (FormatError, usize) {
    let mut scanned_entries = 0usize;
    let error = engine()
        .plan_sfx_from_inputs_with_progress(
            stub,
            &[input.to_path_buf()],
            output,
            &CreateOptions::default(),
            &SfxBuildOptions {
                target,
                ..SfxBuildOptions::default()
            },
            |_count, _path| scanned_entries += 1,
        )
        .unwrap_err();
    (error, scanned_entries)
}

#[test]
fn windows_sfx_roundtrips_through_shared_engine() {
    let temp = TempDir::new("sfx-roundtrip");
    let stub = temp.path().join("stub.exe");
    let archive = temp.path().join("payload.zip");
    let output = temp.path().join("package.exe");
    write_pe_stub(&stub);
    sample_zip(&archive);

    let report = engine()
        .create_sfx(
            &stub,
            &archive,
            &output,
            &SfxBuildOptions {
                target: SfxTarget::Windows,
                ..SfxBuildOptions::default()
            },
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap();
    assert_eq!(report.target, SfxTarget::Windows);
    assert_eq!(report.total_bytes, fs::metadata(&output).unwrap().len());
    assert!(report.requires_signing);

    let info = inspect_sfx(&output).unwrap().unwrap();
    assert_eq!(info.target, SfxTarget::Windows);
    assert_eq!(info.stub_bytes(), fs::metadata(&stub).unwrap().len());
    verify_sfx_payload(
        &output,
        &Default::default(),
        &NoProgress,
        &ControlToken::new(),
    )
    .unwrap();

    let entries = engine().list(&output, &OpenOptions::default()).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].path.display, "docs/readme.txt");

    let extracted = temp.path().join("extracted");
    engine()
        .extract(
            &output,
            &extracted,
            None,
            &OpenOptions::default(),
            &ExtractOptions::default(),
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap();
    assert_eq!(
        fs::read(extracted.join("docs/readme.txt")).unwrap(),
        b"Squallz SFX payload"
    );
}

#[test]
fn standard_unzip_can_extract_the_embedded_zip_payload() {
    if !command_exists("unzip") {
        return;
    }
    let temp = TempDir::new("sfx-unzip-interop");
    let stub = temp.path().join("stub.exe");
    let archive = temp.path().join("payload.zip");
    let output = temp.path().join("package.exe");
    write_pe_stub(&stub);
    sample_zip(&archive);
    engine()
        .create_sfx(
            &stub,
            &archive,
            &output,
            &SfxBuildOptions {
                target: SfxTarget::Windows,
                ..SfxBuildOptions::default()
            },
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap();

    let extracted = temp.path().join("unzip-extracted");
    let result = Command::new("unzip")
        .arg("-q")
        .arg(&output)
        .arg("-d")
        .arg(&extracted)
        .output()
        .unwrap();
    assert!(
        matches!(result.status.code(), Some(0) | Some(1)),
        "unzip rejected SFX; stdout: {}; stderr: {}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        fs::read(extracted.join("docs/readme.txt")).unwrap(),
        b"Squallz SFX payload"
    );
}

#[test]
fn sfx_checksum_detects_payload_tampering_before_extract() {
    let temp = TempDir::new("sfx-tamper");
    let stub = temp.path().join("stub.exe");
    let archive = temp.path().join("payload.zip");
    let output = temp.path().join("package.exe");
    write_pe_stub(&stub);
    sample_zip(&archive);
    engine()
        .create_sfx(
            &stub,
            &archive,
            &output,
            &SfxBuildOptions {
                target: SfxTarget::Windows,
                ..SfxBuildOptions::default()
            },
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap();

    let info = inspect_sfx(&output).unwrap().unwrap();
    let mut bytes = fs::read(&output).unwrap();
    bytes[(info.payload_offset + 8) as usize] ^= 0x5a;
    fs::write(&output, bytes).unwrap();

    let err = verify_sfx_payload(
        &output,
        &Default::default(),
        &NoProgress,
        &ControlToken::new(),
    )
    .unwrap_err();
    assert!(matches!(err, FormatError::CorruptArchive(_)));
}

#[test]
fn authenticode_certificate_table_can_follow_the_sfx_footer() {
    let temp = TempDir::new("sfx-authenticode-layout");
    let stub = temp.path().join("stub.exe");
    let archive = temp.path().join("payload.zip");
    let output = temp.path().join("package.exe");
    write_pe_stub(&stub);
    sample_zip(&archive);
    engine()
        .create_sfx(
            &stub,
            &archive,
            &output,
            &SfxBuildOptions {
                target: SfxTarget::Windows,
                ..SfxBuildOptions::default()
            },
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap();

    append_fake_authenticode_certificate(&output);
    let info = inspect_sfx(&output).unwrap().unwrap();
    assert_eq!(info.target, SfxTarget::Windows);
    assert_eq!(info.total_bytes, fs::metadata(&output).unwrap().len());
    verify_sfx_payload(
        &output,
        &Default::default(),
        &NoProgress,
        &ControlToken::new(),
    )
    .unwrap();
    let entries = engine().list(&output, &OpenOptions::default()).unwrap();
    assert_eq!(entries[0].path.display, "docs/readme.txt");

    let mut bytes = fs::read(&output).unwrap();
    bytes.push(0x5a);
    fs::write(&output, bytes).unwrap();
    let err = inspect_sfx(&output).unwrap_err();
    assert!(matches!(err, FormatError::CorruptArchive(_)));
}

#[test]
fn sfx_extraction_keeps_shared_path_traversal_guard() {
    let temp = TempDir::new("sfx-traversal");
    let stub = temp.path().join("stub.exe");
    let archive = temp.path().join("payload.zip");
    let output = temp.path().join("package.exe");
    write_pe_stub(&stub);
    fs::write(
        &archive,
        build_stored_zip(&[RawZipEntry {
            name: b"../outside.txt".to_vec(),
            data: b"blocked".to_vec(),
        }]),
    )
    .unwrap();
    engine()
        .create_sfx(
            &stub,
            &archive,
            &output,
            &SfxBuildOptions {
                target: SfxTarget::Windows,
                ..SfxBuildOptions::default()
            },
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap();

    let dest = temp.path().join("dest");
    let err = engine()
        .extract(
            &output,
            &dest,
            None,
            &OpenOptions::default(),
            &ExtractOptions::default(),
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap_err();
    assert!(matches!(err, FormatError::PathTraversal(_)));
    assert!(!temp.path().join("outside.txt").exists());
}

#[test]
fn encrypted_zip_payload_keeps_password_boundary() {
    let temp = TempDir::new("sfx-encrypted");
    let stub = temp.path().join("stub.exe");
    let input = temp.path().join("secret.txt");
    let archive = temp.path().join("payload.zip");
    let output = temp.path().join("package.exe");
    write_pe_stub(&stub);
    fs::write(&input, b"private").unwrap();
    engine()
        .create(
            &archive,
            &[input],
            &CreateOptions {
                password: Some(Password::new("correct horse")),
                ..CreateOptions::default()
            },
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap();
    engine()
        .create_sfx(
            &stub,
            &archive,
            &output,
            &SfxBuildOptions {
                target: SfxTarget::Windows,
                ..SfxBuildOptions::default()
            },
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap();

    let err = engine()
        .extract(
            &output,
            &temp.path().join("without-password"),
            None,
            &OpenOptions::default(),
            &ExtractOptions::default(),
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap_err();
    assert!(matches!(err, FormatError::PasswordRequired));

    engine()
        .extract(
            &output,
            &temp.path().join("with-password"),
            None,
            &OpenOptions {
                password: Some(Password::new("correct horse")),
                ..OpenOptions::default()
            },
            &ExtractOptions::default(),
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap();
}

#[test]
fn macos_app_sfx_roundtrips_through_shared_engine() {
    let temp = TempDir::new("sfx-macos-bundle");
    let stub = temp.path().join("Squallz.app");
    let archive = temp.path().join("payload.zip");
    let output = temp.path().join("Photos.app");
    write_macos_app_template(&stub);
    sample_zip(&archive);

    let report = engine()
        .create_sfx(
            &stub,
            &archive,
            &output,
            &SfxBuildOptions {
                target: SfxTarget::Macos,
                ..SfxBuildOptions::default()
            },
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap();
    assert_eq!(report.layout, SfxLayout::MacosApp);
    assert_eq!(report.target, SfxTarget::Macos);
    assert!(report.payload_sha256.is_some());
    assert!(report.requires_signing);

    let info = inspect_sfx(&output).unwrap().unwrap();
    assert_eq!(info.layout, SfxLayout::MacosApp);
    assert_eq!(info.target, SfxTarget::Macos);
    verify_sfx_payload(
        &output,
        &Default::default(),
        &NoProgress,
        &ControlToken::new(),
    )
    .unwrap();
    let entries = engine().list(&output, &OpenOptions::default()).unwrap();
    assert_eq!(entries[0].path.display, "docs/readme.txt");
    let plist = fs::read_to_string(output.join("Contents/Info.plist")).unwrap();
    assert!(plist.contains("<string>Photos</string>"));
    assert!(plist.contains("<key>LSMinimumSystemVersion</key><string>11.0</string>"));
    assert!(!output.join("Contents/_CodeSignature").exists());
}

#[test]
fn macos_app_sfx_requires_a_string_minimum_system_version() {
    let temp = TempDir::new("sfx-macos-minimum-version");
    let stub = temp.path().join("Squallz.app");
    let archive = temp.path().join("payload.zip");
    let output = temp.path().join("Package.app");
    write_macos_app_template(&stub);
    sample_zip(&archive);
    let plist_path = stub.join("Contents/Info.plist");
    let plist = fs::read_to_string(&plist_path).unwrap();
    let wrong_type = plist.replace(
        "<key>LSMinimumSystemVersion</key><string>11.0</string>",
        "<key>LSMinimumSystemVersion</key><true/><key>Fallback</key><string>11.0</string>",
    );
    fs::write(&plist_path, wrong_type).unwrap();

    let err = engine()
        .create_sfx(
            &stub,
            &archive,
            &output,
            &SfxBuildOptions {
                target: SfxTarget::Macos,
                ..SfxBuildOptions::default()
            },
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap_err();
    assert!(matches!(err, FormatError::Unsupported(_)));
    assert!(err
        .to_string()
        .contains("LSMinimumSystemVersion is not a string"));
    assert!(!output.exists());

    fs::write(
        &plist_path,
        plist.replace("<key>LSMinimumSystemVersion</key><string>11.0</string>", ""),
    )
    .unwrap();
    let missing_output = temp.path().join("Missing.app");
    let err = engine()
        .create_sfx(
            &stub,
            &archive,
            &missing_output,
            &SfxBuildOptions {
                target: SfxTarget::Macos,
                ..SfxBuildOptions::default()
            },
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap_err();
    assert!(matches!(err, FormatError::Unsupported(_)));
    assert!(err
        .to_string()
        .contains("Info.plist has no LSMinimumSystemVersion"));
    assert!(!missing_output.exists());
}

#[test]
fn macos_sfx_can_create_its_zip_payload_from_inputs() {
    let temp = TempDir::new("sfx-macos-inputs");
    let stub = temp.path().join("Squallz.app");
    let input = temp.path().join("readme.txt");
    let output = temp.path().join("Notes.app");
    write_macos_app_template(&stub);
    fs::write(&input, b"created through the shared input path").unwrap();

    let engine = engine();
    let sfx_options = SfxBuildOptions {
        target: SfxTarget::Macos,
        ..SfxBuildOptions::default()
    };
    let plan = engine
        .plan_sfx_from_inputs(
            &stub,
            std::slice::from_ref(&input),
            &output,
            &CreateOptions::default(),
            &sfx_options,
        )
        .unwrap();
    let report = engine
        .create_sfx_from_inputs(
            &stub,
            &[input],
            &output,
            &CreateOptions::default(),
            &sfx_options,
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap();
    assert_eq!(report.layout, SfxLayout::MacosApp);
    assert_eq!(plan.primary_output, output);
    assert!(plan.final_output_budget_bytes >= report.total_bytes);
    assert!(plan.workspace_budget_bytes > plan.final_output_budget_bytes);
    let entries = engine.list(&output, &OpenOptions::default()).unwrap();
    assert_eq!(entries[0].path.display, "readme.txt");
    assert!(fs::read_dir(temp.path()).unwrap().all(|entry| !entry
        .unwrap()
        .file_name()
        .to_string_lossy()
        .contains("sfx-payload")));

    let split_output = temp.path().join("Split.app");
    let err = engine
        .create_sfx_from_inputs(
            &stub,
            &[temp.path().join("readme.txt")],
            &split_output,
            &CreateOptions {
                split_size: Some(1024 * 1024),
                ..CreateOptions::default()
            },
            &SfxBuildOptions {
                target: SfxTarget::Macos,
                ..SfxBuildOptions::default()
            },
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap_err();
    assert!(matches!(err, FormatError::Unsupported(_)));
    assert!(!split_output.exists());
}

#[test]
fn macos_sfx_ignores_template_entries_added_after_preparation() {
    let temp = TempDir::new("sfx-macos-prepared-template-members");
    let template = temp.path().join("Squallz.app");
    let input = temp.path().join("input.bin");
    let output = temp.path().join("Package.app");
    let theme = template.join("Contents/Resources/theme");
    write_macos_app_template(&template);
    fs::create_dir(&theme).unwrap();
    fs::write(theme.join("early.dat"), b"early").unwrap();
    fs::write(&input, vec![0x5a; 128 * 1024]).unwrap();

    let late = theme.join("late.dat");
    let progress = OnceOnProgress::new({
        let late = late.clone();
        move || fs::write(&late, b"late").unwrap()
    });
    create_macos_sfx_from_input(&template, &input, &output, false, &progress).unwrap();

    assert!(progress.fired.load(Ordering::Acquire));
    assert_eq!(
        fs::read(output.join("Contents/Resources/theme/early.dat")).unwrap(),
        b"early"
    );
    assert!(!output.join("Contents/Resources/theme/late.dat").exists());
    assert_no_private_sfx_staging(temp.path());
}

#[test]
fn macos_sfx_rejects_a_prepared_template_file_replacement() {
    let temp = TempDir::new("sfx-macos-prepared-template-replacement");
    let template = temp.path().join("Squallz.app");
    let input = temp.path().join("input.bin");
    let output = temp.path().join("Package.app");
    let theme = template.join("Contents/Resources/theme.dat");
    let replacement = temp.path().join("replacement.dat");
    write_macos_app_template(&template);
    fs::write(&theme, b"original").unwrap();
    fs::write(&replacement, b"replaced").unwrap();
    fs::write(&input, vec![0x33; 128 * 1024]).unwrap();
    fs::create_dir(&output).unwrap();
    fs::write(output.join("previous"), b"keep").unwrap();

    let progress = OnceOnProgress::new({
        let theme = theme.clone();
        let replacement = replacement.clone();
        move || {
            fs::remove_file(&theme).unwrap();
            fs::rename(&replacement, &theme).unwrap();
        }
    });
    let error =
        create_macos_sfx_from_input(&template, &input, &output, true, &progress).unwrap_err();

    assert!(progress.fired.load(Ordering::Acquire));
    assert!(matches!(error, FormatError::CorruptArchive(_)));
    assert!(error.to_string().contains("app template changed"));
    assert_eq!(fs::read(output.join("previous")).unwrap(), b"keep");
    assert_no_private_sfx_staging(temp.path());
}

#[test]
fn macos_sfx_rejects_a_template_file_rebound_after_open() {
    let temp = TempDir::new("sfx-macos-open-template-rebind");
    let template = temp.path().join("Squallz.app");
    let archive = temp.path().join("payload.zip");
    let output = temp.path().join("Package.app");
    let theme = template.join("Contents/Resources/theme.dat");
    let retained = temp.path().join("original-theme.dat");
    let replacement = temp.path().join("replacement-theme.dat");
    write_macos_app_template(&template);
    sample_zip(&archive);
    fs::write(&theme, vec![0x31; 128 * 1024]).unwrap();
    fs::write(&replacement, vec![0x72; 128 * 1024]).unwrap();

    let progress = OnceOnEntryProgress::new("Contents/Resources/theme.dat", {
        let theme = theme.clone();
        let retained = retained.clone();
        let replacement = replacement.clone();
        move || {
            fs::rename(&theme, &retained).unwrap();
            fs::rename(&replacement, &theme).unwrap();
        }
    });
    let error = engine()
        .create_sfx(
            &template,
            &archive,
            &output,
            &SfxBuildOptions {
                target: SfxTarget::Macos,
                ..SfxBuildOptions::default()
            },
            &progress,
            &ControlToken::new(),
        )
        .unwrap_err();

    assert!(progress.fired.load(Ordering::Acquire));
    assert!(matches!(error, FormatError::CorruptArchive(_)));
    assert!(error.to_string().contains("app template changed"));
    assert!(!output.exists());
    assert_no_private_sfx_staging(temp.path());
}

#[cfg(unix)]
#[test]
fn macos_sfx_staging_root_replacement_never_mutates_the_competing_path() {
    let temp = TempDir::new("sfx-macos-stage-root-rebind");
    let template = temp.path().join("Squallz.app");
    let archive = temp.path().join("payload.zip");
    let output = temp.path().join("Package.app");
    let theme = template.join("Contents/Resources/theme.dat");
    let retained = temp.path().join("retained-stage");
    write_macos_app_template(&template);
    sample_zip(&archive);
    fs::write(&theme, vec![0x35; 128 * 1024]).unwrap();

    let progress = OnceOnEntryProgress::new("Contents/Resources/theme.dat", {
        let root = temp.path().to_path_buf();
        let retained = retained.clone();
        move || {
            let staged = private_sfx_stage(&root);
            fs::rename(&staged, &retained).unwrap();
            fs::create_dir(&staged).unwrap();
            fs::write(staged.join("competitor"), b"keep").unwrap();
        }
    });
    let error = engine()
        .create_sfx(
            &template,
            &archive,
            &output,
            &SfxBuildOptions {
                target: SfxTarget::Macos,
                ..SfxBuildOptions::default()
            },
            &progress,
            &ControlToken::new(),
        )
        .unwrap_err();

    assert!(progress.fired.load(Ordering::Acquire));
    assert!(error.to_string().contains("staging changed"));
    let competing = private_sfx_stage(temp.path());
    assert_eq!(fs::read(competing.join("competitor")).unwrap(), b"keep");
    assert_eq!(fs::read_dir(&competing).unwrap().count(), 1);
    assert!(retained
        .join("Contents/Resources/squallz-sfx/payload.zip")
        .is_file());
    assert!(!output.exists());

    fs::remove_dir_all(competing).unwrap();
    fs::remove_dir_all(retained).unwrap();
    assert_no_private_sfx_staging(temp.path());
}

#[test]
fn macos_sfx_rejects_a_prepared_template_file_removed_before_assembly() {
    let temp = TempDir::new("sfx-macos-prepared-template-removal");
    let template = temp.path().join("Squallz.app");
    let input = temp.path().join("input.bin");
    let output = temp.path().join("Package.app");
    let theme = template.join("Contents/Resources/theme.dat");
    write_macos_app_template(&template);
    fs::write(&theme, b"theme").unwrap();
    fs::write(&input, vec![0x44; 128 * 1024]).unwrap();

    let progress = OnceOnProgress::new({
        let theme = theme.clone();
        move || fs::remove_file(&theme).unwrap()
    });
    let error =
        create_macos_sfx_from_input(&template, &input, &output, false, &progress).unwrap_err();

    assert!(progress.fired.load(Ordering::Acquire));
    assert!(matches!(error, FormatError::CorruptArchive(_)));
    assert!(error.to_string().contains("app template changed"));
    assert!(!output.exists());
    assert_no_private_sfx_staging(temp.path());
}

#[test]
fn macos_sfx_rejects_a_prepared_template_directory_replacement() {
    let temp = TempDir::new("sfx-macos-prepared-template-directory");
    let template = temp.path().join("Squallz.app");
    let input = temp.path().join("input.bin");
    let output = temp.path().join("Package.app");
    let theme = template.join("Contents/Resources/theme");
    write_macos_app_template(&template);
    fs::create_dir(&theme).unwrap();
    fs::write(&input, vec![0x22; 128 * 1024]).unwrap();

    let progress = OnceOnProgress::new({
        let theme = theme.clone();
        move || {
            fs::remove_dir(&theme).unwrap();
            fs::create_dir(&theme).unwrap();
        }
    });
    let error =
        create_macos_sfx_from_input(&template, &input, &output, false, &progress).unwrap_err();

    assert!(progress.fired.load(Ordering::Acquire));
    assert!(matches!(error, FormatError::CorruptArchive(_)));
    assert!(error.to_string().contains("app template changed"));
    assert!(!output.exists());
    assert_no_private_sfx_staging(temp.path());
}

#[cfg(unix)]
#[test]
fn macos_sfx_rejects_a_prepared_template_file_changed_to_a_symlink() {
    let temp = TempDir::new("sfx-macos-prepared-template-symlink");
    let template = temp.path().join("Squallz.app");
    let input = temp.path().join("input.bin");
    let output = temp.path().join("Package.app");
    let theme = template.join("Contents/Resources/theme.dat");
    let outside = temp.path().join("outside.dat");
    write_macos_app_template(&template);
    fs::write(&theme, b"inside").unwrap();
    fs::write(&outside, b"outside").unwrap();
    fs::write(&input, vec![0x77; 128 * 1024]).unwrap();

    let progress = OnceOnProgress::new({
        let theme = theme.clone();
        let outside = outside.clone();
        move || {
            fs::remove_file(&theme).unwrap();
            std::os::unix::fs::symlink(&outside, &theme).unwrap();
        }
    });
    let error =
        create_macos_sfx_from_input(&template, &input, &output, false, &progress).unwrap_err();

    assert!(progress.fired.load(Ordering::Acquire));
    assert!(matches!(error, FormatError::CorruptArchive(_)));
    assert!(error.to_string().contains("app template changed"));
    assert!(!output.exists());
    assert_no_private_sfx_staging(temp.path());
}

#[cfg(unix)]
#[test]
fn macos_sfx_rejects_a_prepared_template_symlink_target_change() {
    let temp = TempDir::new("sfx-macos-prepared-template-link-target");
    let template = temp.path().join("Squallz.app");
    let input = temp.path().join("input.bin");
    let output = temp.path().join("Package.app");
    let resources = template.join("Contents/Resources");
    let current = resources.join("current-theme");
    write_macos_app_template(&template);
    fs::write(resources.join("light.dat"), b"light").unwrap();
    fs::write(resources.join("dark.dat"), b"dark").unwrap();
    std::os::unix::fs::symlink("light.dat", &current).unwrap();
    fs::write(&input, vec![0x66; 128 * 1024]).unwrap();

    let progress = OnceOnProgress::new({
        let current = current.clone();
        move || {
            fs::remove_file(&current).unwrap();
            std::os::unix::fs::symlink("dark.dat", &current).unwrap();
        }
    });
    let error =
        create_macos_sfx_from_input(&template, &input, &output, false, &progress).unwrap_err();

    assert!(progress.fired.load(Ordering::Acquire));
    assert!(matches!(error, FormatError::CorruptArchive(_)));
    assert!(error.to_string().contains("app template changed"));
    assert!(!output.exists());
    assert_no_private_sfx_staging(temp.path());
}

#[cfg(unix)]
#[test]
fn macos_sfx_plan_budgets_custom_template_nodes_and_symlinks() {
    let temp = TempDir::new("sfx-macos-custom-template-budget");
    let stub = temp.path().join("Squallz.app");
    let input = temp.path().join("readme.txt");
    let output = temp.path().join("Package.app");
    write_macos_app_template(&stub);
    fs::write(&input, b"custom template budget").unwrap();

    let engine = engine();
    let options = SfxBuildOptions {
        target: SfxTarget::Macos,
        ..SfxBuildOptions::default()
    };
    let baseline = engine
        .plan_sfx_from_inputs(
            &stub,
            std::slice::from_ref(&input),
            &output,
            &CreateOptions::default(),
            &options,
        )
        .unwrap();

    let custom = stub
        .join("Contents/Resources")
        .join(format!("theme-{}", "x".repeat(180)));
    fs::create_dir_all(custom.join("empty/nested")).unwrap();
    fs::write(custom.join("palette.dat"), b"midnight").unwrap();
    std::os::unix::fs::symlink("palette.dat", custom.join("current")).unwrap();

    let plan = engine
        .plan_sfx_from_inputs(
            &stub,
            std::slice::from_ref(&input),
            &output,
            &CreateOptions::default(),
            &options,
        )
        .unwrap();
    assert!(plan.final_output_budget_bytes > baseline.final_output_budget_bytes);

    let report = engine
        .create_sfx_from_inputs(
            &stub,
            &[input],
            &output,
            &CreateOptions::default(),
            &options,
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap();
    let allocated = allocated_tree_bytes(&output);
    assert!(
        plan.final_output_budget_bytes >= allocated,
        "plan {} must cover {} physically allocated bytes",
        plan.final_output_budget_bytes,
        allocated
    );
    assert!(plan.final_output_budget_bytes > report.total_bytes);
    let copied_link = output
        .join("Contents/Resources")
        .join(format!("theme-{}", "x".repeat(180)))
        .join("current");
    assert_eq!(
        fs::read_link(copied_link).unwrap(),
        Path::new("palette.dat")
    );
}

#[test]
fn macos_sfx_rebuild_prunes_the_existing_output_bundle_from_its_payload() {
    let temp = TempDir::new("sfx-macos-rebuild-input-dir");
    let stub = temp.path().join("Squallz.app");
    let source = temp.path().join("source");
    let output = source.join("Notes.app");
    write_macos_app_template(&stub);
    fs::create_dir_all(output.join("Contents/Resources")).unwrap();
    fs::write(source.join("keep.txt"), b"keep me").unwrap();
    fs::write(
        output.join("Contents/Resources/old-output.bin"),
        vec![0x5a; 32 * 1024],
    )
    .unwrap();

    engine()
        .create_sfx_from_inputs(
            &stub,
            std::slice::from_ref(&source),
            &output,
            &CreateOptions::default(),
            &SfxBuildOptions {
                target: SfxTarget::Macos,
                overwrite: true,
                ..SfxBuildOptions::default()
            },
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap();

    let names: Vec<_> = engine()
        .list(&output, &OpenOptions::default())
        .unwrap()
        .into_iter()
        .map(|entry| entry.path.display)
        .collect();
    assert!(names.iter().any(|name| name == "source/keep.txt"));
    assert!(names
        .iter()
        .all(|name| !name.starts_with("source/Notes.app")));
    assert!(names.iter().all(|name| !name.contains(".squallz-sfx-")));
}

#[test]
fn single_file_sfx_rebuild_prunes_the_existing_output_from_its_payload() {
    let temp = TempDir::new("sfx-windows-rebuild-input-dir");
    let stub = temp.path().join("stub.exe");
    let source = temp.path().join("source");
    let output = source.join("Package.exe");
    write_pe_stub(&stub);
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("keep.txt"), b"keep me").unwrap();
    fs::write(&output, vec![0xa5; 32 * 1024]).unwrap();

    let engine = engine();
    let sfx_options = SfxBuildOptions {
        target: SfxTarget::Windows,
        overwrite: true,
        ..SfxBuildOptions::default()
    };
    let plan = engine
        .plan_sfx_from_inputs(
            &stub,
            std::slice::from_ref(&source),
            &output,
            &CreateOptions::default(),
            &sfx_options,
        )
        .unwrap();
    let report = engine
        .create_sfx_from_inputs(
            &stub,
            std::slice::from_ref(&source),
            &output,
            &CreateOptions::default(),
            &sfx_options,
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap();
    assert_eq!(plan.primary_output, output);
    assert!(plan.final_output_budget_bytes >= report.total_bytes);
    assert!(plan.workspace_budget_bytes > plan.final_output_budget_bytes);

    let names: Vec<_> = engine
        .list(&output, &OpenOptions::default())
        .unwrap()
        .into_iter()
        .map(|entry| entry.path.display)
        .collect();
    assert!(names.iter().any(|name| name == "source/keep.txt"));
    assert!(names.iter().all(|name| name != "source/Package.exe"));
    assert!(names.iter().all(|name| !name.contains(".squallz-sfx-")));
}

#[test]
fn sfx_rejects_an_explicit_output_input_without_overwriting_it() {
    let temp = TempDir::new("sfx-output-as-input");
    let stub = temp.path().join("stub.exe");
    let output = temp.path().join("Package.exe");
    let original = b"existing output must survive";
    write_pe_stub(&stub);
    fs::write(&output, original).unwrap();

    let error = engine()
        .create_sfx_from_inputs(
            &stub,
            std::slice::from_ref(&output),
            &output,
            &CreateOptions::default(),
            &SfxBuildOptions {
                target: SfxTarget::Windows,
                overwrite: true,
                ..SfxBuildOptions::default()
            },
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap_err();

    assert!(matches!(error, FormatError::Unsupported(_)));
    assert!(error.to_string().contains("cannot also be an input"));
    assert_eq!(fs::read(&output).unwrap(), original);
}

#[test]
fn macos_sfx_rejects_an_explicit_input_inside_the_output_bundle() {
    let temp = TempDir::new("sfx-output-child-as-input");
    let stub = temp.path().join("Squallz.app");
    let output = temp.path().join("Notes.app");
    let input = output.join("Contents/Resources/source.txt");
    let original = b"this source must not be deleted with the old bundle";
    write_macos_app_template(&stub);
    fs::create_dir_all(input.parent().unwrap()).unwrap();
    fs::write(&input, original).unwrap();

    let error = engine()
        .create_sfx_from_inputs(
            &stub,
            std::slice::from_ref(&input),
            &output,
            &CreateOptions::default(),
            &SfxBuildOptions {
                target: SfxTarget::Macos,
                overwrite: true,
                ..SfxBuildOptions::default()
            },
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap_err();

    assert!(matches!(error, FormatError::Unsupported(_)));
    assert!(error.to_string().contains("cannot also be an input"));
    assert_eq!(fs::read(&input).unwrap(), original);
}

#[test]
fn macos_bundle_detects_payload_tampering_and_strips_source_signature() {
    let temp = TempDir::new("sfx-macos-boundaries");
    let stub = temp.path().join("Squallz.app");
    let archive = temp.path().join("payload.zip");
    let output = temp.path().join("Package.app");
    write_macos_app_template(&stub);
    sample_zip(&archive);
    engine()
        .create_sfx(
            &stub,
            &archive,
            &output,
            &SfxBuildOptions {
                target: SfxTarget::Macos,
                ..SfxBuildOptions::default()
            },
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap();
    let payload = output.join("Contents/Resources/squallz-sfx/payload.zip");
    let mut bytes = fs::read(&payload).unwrap();
    bytes[8] ^= 0x5a;
    fs::write(&payload, bytes).unwrap();
    let err = verify_sfx_payload(
        &output,
        &Default::default(),
        &NoProgress,
        &ControlToken::new(),
    )
    .unwrap_err();
    assert!(matches!(err, FormatError::CorruptArchive(_)));

    let manifest = output.join("Contents/Resources/squallz-sfx/manifest.v1");
    let mut manifest_bytes = fs::read(&manifest).unwrap();
    manifest_bytes[0] ^= 0x5a;
    fs::write(&manifest, manifest_bytes).unwrap();
    let err = inspect_sfx(&output).unwrap_err();
    assert!(matches!(err, FormatError::CorruptArchive(_)));

    let cancelled = temp.path().join("Cancelled.app");
    let ctl = ControlToken::new();
    ctl.cancel();
    let err = engine()
        .create_sfx(
            &stub,
            &archive,
            &cancelled,
            &SfxBuildOptions {
                target: SfxTarget::Macos,
                ..SfxBuildOptions::default()
            },
            &NoProgress,
            &ctl,
        )
        .unwrap_err();
    assert!(matches!(err, FormatError::Cancelled));
    assert!(!cancelled.exists());

    let signature = stub.join("Contents/_CodeSignature/CodeResources");
    fs::create_dir_all(signature.parent().unwrap()).unwrap();
    fs::write(&signature, b"stale outer signature").unwrap();
    let rebuilt = temp.path().join("Rebuilt.app");
    engine()
        .create_sfx(
            &stub,
            &archive,
            &rebuilt,
            &SfxBuildOptions {
                target: SfxTarget::Macos,
                ..SfxBuildOptions::default()
            },
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap();
    assert!(!rebuilt.join("Contents/_CodeSignature").exists());
    verify_sfx_payload(
        &rebuilt,
        &Default::default(),
        &NoProgress,
        &ControlToken::new(),
    )
    .unwrap();
}

#[cfg(unix)]
#[test]
fn macos_bundle_template_rejects_symlinks_that_resolve_outside() {
    let temp = TempDir::new("sfx-macos-symlink-template");
    let stub = temp.path().join("Squallz.app");
    let archive = temp.path().join("payload.zip");
    let output = temp.path().join("Package.app");
    let outside = temp.path().join("outside.txt");
    write_macos_app_template(&stub);
    sample_zip(&archive);
    fs::write(&outside, b"outside").unwrap();
    std::os::unix::fs::symlink(&outside, stub.join("Contents/Resources/outside-link")).unwrap();

    let err = engine()
        .create_sfx(
            &stub,
            &archive,
            &output,
            &SfxBuildOptions {
                target: SfxTarget::Macos,
                ..SfxBuildOptions::default()
            },
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap_err();
    assert!(matches!(err, FormatError::Unsupported(_)));
    assert!(!output.exists());
}

#[cfg(unix)]
#[test]
fn macos_bundle_template_rejects_symlinked_generated_path_ancestors() {
    let temp = TempDir::new("sfx-macos-symlinked-resources");
    let template = temp.path().join("Squallz.app");
    let archive = temp.path().join("payload.zip");
    let output = temp.path().join("Package.app");
    write_macos_app_template(&template);
    sample_zip(&archive);
    let resources = template.join("Contents/Resources");
    let actual_resources = template.join("Contents/SharedResources");
    fs::rename(&resources, &actual_resources).unwrap();
    std::os::unix::fs::symlink("SharedResources", &resources).unwrap();

    let error = engine()
        .create_sfx(
            &template,
            &archive,
            &output,
            &SfxBuildOptions {
                target: SfxTarget::Macos,
                ..SfxBuildOptions::default()
            },
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap_err();

    assert!(matches!(error, FormatError::Unsupported(_)));
    assert!(error.to_string().contains("non-symlink directory"));
    assert!(!output.exists());
}

#[cfg(unix)]
#[test]
fn macos_bundle_template_rejects_special_files_during_plan_and_create() {
    use std::os::unix::net::UnixListener;

    let short_root = Path::new("/tmp").join(format!("sqz-sfx-special-{}", std::process::id()));
    let _ = fs::remove_dir_all(&short_root);
    fs::create_dir(&short_root).unwrap();
    let temp = TempDir(short_root);
    let stub = temp.path().join("Squallz.app");
    let input = temp.path().join("readme.txt");
    let archive = temp.path().join("payload.zip");
    let output = temp.path().join("Package.app");
    write_macos_app_template(&stub);
    fs::write(&input, b"source").unwrap();
    sample_zip(&archive);
    let socket = stub.join("Contents/Resources/runtime.sock");
    let _listener = UnixListener::bind(&socket).unwrap();
    let options = SfxBuildOptions {
        target: SfxTarget::Macos,
        ..SfxBuildOptions::default()
    };

    let error = engine()
        .plan_sfx_from_inputs(
            &stub,
            &[input],
            &output,
            &CreateOptions::default(),
            &options,
        )
        .unwrap_err();
    assert!(matches!(error, FormatError::Unsupported(_)));
    assert!(error.to_string().contains("unsupported app template entry"));

    let error = engine()
        .create_sfx(
            &stub,
            &archive,
            &output,
            &options,
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap_err();
    assert!(matches!(error, FormatError::Unsupported(_)));
    assert!(error.to_string().contains("unsupported app template entry"));
    assert!(!output.exists());
}

#[test]
fn macos_single_file_sfx_is_rejected() {
    let temp = TempDir::new("sfx-macos-single-file");
    let stub = temp.path().join("stub");
    let archive = temp.path().join("payload.zip");
    let output = temp.path().join("package.app");
    write_macho_stub(&stub);
    sample_zip(&archive);
    let err = engine()
        .create_sfx(
            &stub,
            &archive,
            &output,
            &SfxBuildOptions {
                target: SfxTarget::Macos,
                ..SfxBuildOptions::default()
            },
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap_err();
    assert!(matches!(err, FormatError::Unsupported(_)));
    assert!(!output.exists());
}

#[test]
fn sfx_build_requires_a_matching_squallz_stub() {
    let temp = TempDir::new("sfx-stub-boundary");
    let stub = temp.path().join("generic.exe");
    let archive = temp.path().join("payload.zip");
    let output = temp.path().join("package.exe");
    write_pe_stub_without_marker(&stub);
    sample_zip(&archive);

    let err = engine()
        .create_sfx(
            &stub,
            &archive,
            &output,
            &SfxBuildOptions {
                target: SfxTarget::Windows,
                ..SfxBuildOptions::default()
            },
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap_err();
    assert!(matches!(err, FormatError::Unsupported(_)));
    assert!(!output.exists());

    write_pe_stub(&stub);
    append_fake_authenticode_certificate(&stub);
    let err = engine()
        .create_sfx(
            &stub,
            &archive,
            &output,
            &SfxBuildOptions {
                target: SfxTarget::Windows,
                ..SfxBuildOptions::default()
            },
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap_err();
    assert!(matches!(err, FormatError::Unsupported(_)));
    assert!(!output.exists());

    write_pe_stub(&stub);
    let err = engine()
        .create_sfx(
            &stub,
            &archive,
            &output,
            &SfxBuildOptions {
                target: SfxTarget::Linux,
                ..SfxBuildOptions::default()
            },
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap_err();
    assert!(matches!(err, FormatError::Unsupported(_)));
    assert!(!output.exists());
}

#[test]
fn sfx_plan_rejects_a_missing_cli_marker_before_scanning_inputs() {
    let temp = TempDir::new("sfx-plan-missing-cli-marker");
    let stub = temp.path().join("generic.exe");
    let input = temp.path().join("input.txt");
    let output = temp.path().join("package.exe");
    write_pe_stub_without_marker(&stub);
    fs::write(&input, b"source").unwrap();

    let (error, scanned_entries) =
        rejected_plan_before_input_scan(&stub, &input, &output, SfxTarget::Windows);

    assert!(matches!(error, FormatError::Unsupported(_)));
    assert!(error.to_string().contains("not a Squallz SFX-capable"));
    assert_eq!(scanned_entries, 0);
}

#[test]
fn sfx_plan_rejects_a_wrong_target_before_scanning_inputs() {
    let temp = TempDir::new("sfx-plan-wrong-target");
    let stub = temp.path().join("stub.exe");
    let input = temp.path().join("input.txt");
    let output = temp.path().join("package.run");
    write_pe_stub(&stub);
    fs::write(&input, b"source").unwrap();

    let (error, scanned_entries) =
        rejected_plan_before_input_scan(&stub, &input, &output, SfxTarget::Linux);

    assert!(matches!(error, FormatError::Unsupported(_)));
    assert!(error
        .to_string()
        .contains("does not match requested target"));
    assert_eq!(scanned_entries, 0);
}

#[test]
fn sfx_plan_rejects_a_signed_windows_stub_before_scanning_inputs() {
    let temp = TempDir::new("sfx-plan-signed-windows-stub");
    let stub = temp.path().join("signed.exe");
    let input = temp.path().join("input.txt");
    let output = temp.path().join("package.exe");
    write_pe_stub(&stub);
    append_fake_authenticode_certificate(&stub);
    fs::write(&input, b"source").unwrap();

    let (error, scanned_entries) =
        rejected_plan_before_input_scan(&stub, &input, &output, SfxTarget::Windows);

    assert!(matches!(error, FormatError::Unsupported(_)));
    assert!(error.to_string().contains("unsigned Squallz Windows stub"));
    assert_eq!(scanned_entries, 0);
}

#[test]
fn sfx_plan_rejects_an_existing_sfx_before_scanning_inputs() {
    let temp = TempDir::new("sfx-plan-existing-artifact");
    let stub = temp.path().join("stub.exe");
    let archive = temp.path().join("payload.zip");
    let existing = temp.path().join("existing.exe");
    let input = temp.path().join("input.txt");
    let output = temp.path().join("package.exe");
    write_pe_stub(&stub);
    sample_zip(&archive);
    fs::write(&input, b"source").unwrap();
    engine()
        .create_sfx(
            &stub,
            &archive,
            &existing,
            &SfxBuildOptions {
                target: SfxTarget::Windows,
                ..SfxBuildOptions::default()
            },
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap();

    let (error, scanned_entries) =
        rejected_plan_before_input_scan(&existing, &input, &output, SfxTarget::Windows);

    assert!(matches!(error, FormatError::Unsupported(_)));
    assert!(error.to_string().contains("cannot be reused as a stub"));
    assert_eq!(scanned_entries, 0);
}

#[test]
fn sfx_plan_rejects_a_missing_macos_gui_marker_before_scanning_inputs() {
    let temp = TempDir::new("sfx-plan-missing-macos-gui-marker");
    let template = temp.path().join("Squallz.app");
    let input = temp.path().join("input.txt");
    let output = temp.path().join("Package.app");
    write_macos_app_template(&template);
    let executable = template.join("Contents/MacOS/squallz-gui");
    let mut bytes = fs::read(&executable).unwrap();
    bytes[0x80..0x80 + SFX_GUI_STUB_MARKER.len()].fill(0);
    fs::write(executable, bytes).unwrap();
    fs::write(&input, b"source").unwrap();

    let (error, scanned_entries) =
        rejected_plan_before_input_scan(&template, &input, &output, SfxTarget::Macos);

    assert!(matches!(error, FormatError::Unsupported(_)));
    assert!(error.to_string().contains("GUI SFX-capable Mach-O"));
    assert_eq!(scanned_entries, 0);
}

#[test]
fn input_sfx_creation_validates_the_runtime_before_reading_sources() {
    let temp = TempDir::new("sfx-create-invalid-runtime-first");
    let stub = temp.path().join("generic.exe");
    let missing_input = temp.path().join("missing-input.txt");
    let output = temp.path().join("package.exe");
    write_pe_stub_without_marker(&stub);

    let error = engine()
        .create_sfx_from_inputs(
            &stub,
            &[missing_input],
            &output,
            &CreateOptions::default(),
            &SfxBuildOptions {
                target: SfxTarget::Windows,
                ..SfxBuildOptions::default()
            },
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap_err();

    assert!(matches!(error, FormatError::Unsupported(_)));
    assert!(error.to_string().contains("not a Squallz SFX-capable"));
    assert!(!output.exists());
    assert_no_private_sfx_staging(temp.path());
}

#[test]
fn single_file_sfx_rejects_runtime_path_replacement_during_copy() {
    let temp = TempDir::new("sfx-runtime-replaced-during-copy");
    let stub = temp.path().join("stub.exe");
    let original = temp.path().join("original-stub.exe");
    let archive = temp.path().join("payload.zip");
    let output = temp.path().join("package.exe");
    write_pe_stub(&stub);
    sample_zip(&archive);

    let progress = OnceOnEntryProgress::new("stub", {
        let stub = stub.clone();
        let original = original.clone();
        move || {
            fs::rename(&stub, &original).unwrap();
            write_pe_stub_without_marker(&stub);
        }
    });
    let error = engine()
        .create_sfx(
            &stub,
            &archive,
            &output,
            &SfxBuildOptions {
                target: SfxTarget::Windows,
                ..SfxBuildOptions::default()
            },
            &progress,
            &ControlToken::new(),
        )
        .unwrap_err();

    assert!(error.to_string().contains("SFX runtime changed"));
    assert!(!output.exists());
    assert_no_private_sfx_staging(temp.path());
}

#[cfg(unix)]
#[test]
fn single_file_sfx_rejects_runtime_rebound_to_a_symlink_during_copy() {
    let temp = TempDir::new("sfx-runtime-symlink-during-copy");
    let stub = temp.path().join("stub.exe");
    let original = temp.path().join("original-stub.exe");
    let outside = temp.path().join("outside.exe");
    let archive = temp.path().join("payload.zip");
    let output = temp.path().join("package.exe");
    write_pe_stub(&stub);
    write_pe_stub_without_marker(&outside);
    sample_zip(&archive);

    let progress = OnceOnEntryProgress::new("stub", {
        let stub = stub.clone();
        let original = original.clone();
        let outside = outside.clone();
        move || {
            fs::rename(&stub, &original).unwrap();
            std::os::unix::fs::symlink(&outside, &stub).unwrap();
        }
    });
    let error = engine()
        .create_sfx(
            &stub,
            &archive,
            &output,
            &SfxBuildOptions {
                target: SfxTarget::Windows,
                ..SfxBuildOptions::default()
            },
            &progress,
            &ControlToken::new(),
        )
        .unwrap_err();

    assert!(error.to_string().contains("SFX runtime changed"));
    assert!(!output.exists());
    assert_no_private_sfx_staging(temp.path());
}

#[test]
fn cancelled_sfx_build_leaves_no_output() {
    let temp = TempDir::new("sfx-cancel");
    let stub = temp.path().join("stub.exe");
    let archive = temp.path().join("payload.zip");
    let output = temp.path().join("package.exe");
    write_pe_stub(&stub);
    sample_zip(&archive);
    let ctl = ControlToken::new();
    ctl.cancel();

    let err = engine()
        .create_sfx(
            &stub,
            &archive,
            &output,
            &SfxBuildOptions {
                target: SfxTarget::Windows,
                ..SfxBuildOptions::default()
            },
            &NoProgress,
            &ctl,
        )
        .unwrap_err();
    assert!(matches!(err, FormatError::Cancelled));
    assert!(!output.exists());
    assert!(
        fs::read_dir(temp.path())
            .unwrap()
            .flatten()
            .all(|entry| !entry.file_name().to_string_lossy().contains(".sfx-")),
        "cancelled build must remove sibling temporary output"
    );
}

#[test]
fn cancelled_sfx_overwrite_preserves_the_previous_output() {
    let temp = TempDir::new("sfx-cancel-overwrite");
    let archive = temp.path().join("payload.zip");
    sample_zip(&archive);

    let stub = temp.path().join("stub.exe");
    let output = temp.path().join("package.exe");
    write_pe_stub(&stub);
    fs::write(&output, b"previous single-file output").unwrap();
    let ctl = ControlToken::new();
    let progress = CancelOnProgress { ctl: ctl.clone() };
    let error = engine()
        .create_sfx(
            &stub,
            &archive,
            &output,
            &SfxBuildOptions {
                target: SfxTarget::Windows,
                overwrite: true,
                ..SfxBuildOptions::default()
            },
            &progress,
            &ctl,
        )
        .unwrap_err();
    assert!(matches!(error, FormatError::Cancelled));
    assert_eq!(fs::read(&output).unwrap(), b"previous single-file output");

    let template = temp.path().join("Squallz.app");
    let bundle = temp.path().join("Package.app");
    write_macos_app_template(&template);
    fs::create_dir(&bundle).unwrap();
    fs::write(bundle.join("previous"), b"previous bundle output").unwrap();
    let ctl = ControlToken::new();
    let progress = CancelOnProgress { ctl: ctl.clone() };
    let error = engine()
        .create_sfx(
            &template,
            &archive,
            &bundle,
            &SfxBuildOptions {
                target: SfxTarget::Macos,
                overwrite: true,
                ..SfxBuildOptions::default()
            },
            &progress,
            &ctl,
        )
        .unwrap_err();
    assert!(matches!(error, FormatError::Cancelled));
    assert_eq!(
        fs::read(bundle.join("previous")).unwrap(),
        b"previous bundle output"
    );
}

#[test]
fn concurrent_input_builds_publish_once_and_clean_their_private_staging() {
    let temp = TempDir::new("sfx-concurrent-input-builds");
    let stub = temp.path().join("stub.exe");
    let input = temp.path().join("input.bin");
    let output = temp.path().join("package.exe");
    write_pe_stub(&stub);
    fs::write(&input, vec![0x5a; 1024 * 1024]).unwrap();
    let barrier = Arc::new(Barrier::new(2));

    let handles: Vec<_> = (0..2)
        .map(|_| {
            let barrier = barrier.clone();
            let stub = stub.clone();
            let input = input.clone();
            let output = output.clone();
            std::thread::spawn(move || {
                barrier.wait();
                engine().create_sfx_from_inputs(
                    &stub,
                    &[input],
                    &output,
                    &CreateOptions::default(),
                    &SfxBuildOptions {
                        target: SfxTarget::Windows,
                        ..SfxBuildOptions::default()
                    },
                    &NoProgress,
                    &ControlToken::new(),
                )
            })
        })
        .collect();
    let results: Vec<_> = handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .collect();

    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, Err(FormatError::Unsupported(_))))
            .count(),
        1
    );
    verify_sfx_payload(
        &output,
        &Default::default(),
        &NoProgress,
        &ControlToken::new(),
    )
    .unwrap();
    assert!(fs::read_dir(temp.path()).unwrap().all(|entry| {
        let name = entry.unwrap().file_name().to_string_lossy().into_owned();
        !name.contains("sfx-payload") && !name.contains(".sfx-")
    }));
}

#[test]
fn split_volume_cannot_be_used_as_sfx_payload() {
    let temp = TempDir::new("sfx-split-payload");
    let stub = temp.path().join("stub.exe");
    let archive = temp.path().join("payload.zip.001");
    let output = temp.path().join("package.exe");
    write_pe_stub(&stub);
    sample_zip(&archive);

    let err = engine()
        .create_sfx(
            &stub,
            &archive,
            &output,
            &SfxBuildOptions {
                target: SfxTarget::Windows,
                ..SfxBuildOptions::default()
            },
            &NoProgress,
            &ControlToken::new(),
        )
        .unwrap_err();
    assert!(matches!(err, FormatError::Unsupported(_)));
    assert!(!output.exists());
}
