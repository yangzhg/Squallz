#![forbid(unsafe_code)]
//! Recovery data operations shared by the CLI and desktop app.
//!
//! I10 starts with a standard PAR2 sidecar bridge. It deliberately calls a
//! user-provided or PATH-resolved par2cmdline-compatible executable so
//! Squallz can interoperate before taking on a full PAR2 encoder/decoder.

use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use squallz_format_api::FormatError;

const TOOL_ENV: &str = "SQUALLZ_PAR2";
const TOOL_CANDIDATES: [&str; 3] = ["par2cmdline-turbo", "par2", "par2cmdline"];
const DEFAULT_TOOL_MISSING: &str = "par2cmdline-turbo/par2";
const RUST_PAR2_TOOL: &str = "rust-par2";

/// Machine-readable result for PAR2 operations.
#[derive(Debug, Clone, Serialize)]
pub struct RecoveryReport {
    pub ok: bool,
    pub operation: &'static str,
    pub archive: PathBuf,
    pub recovery: PathBuf,
    pub output: Option<PathBuf>,
    pub tool: PathBuf,
    pub redundancy_percent: Option<u8>,
    pub status_code: Option<i32>,
    pub metrics: Option<RecoveryMetrics>,
    pub stdout: String,
    pub stderr: String,
}

/// Structured recovery math when the backend exposes it directly.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RecoveryMetrics {
    pub all_correct: bool,
    pub repair_possible: bool,
    pub blocks_needed: u32,
    pub recovery_blocks_available: u32,
    pub blocks_repaired: Option<u32>,
    pub files_repaired: Option<usize>,
    pub no_damage: bool,
}

/// Builds external PAR2 data for one archive or split-set head.
pub fn protect(
    archive: &Path,
    redundancy: u8,
    recovery: Option<&Path>,
) -> Result<RecoveryReport, FormatError> {
    protect_files(archive, redundancy, recovery, &[archive.to_path_buf()])
}

/// Builds external PAR2 data for an explicit set of source files.
pub fn protect_files(
    archive: &Path,
    redundancy: u8,
    recovery: Option<&Path>,
    sources: &[PathBuf],
) -> Result<RecoveryReport, FormatError> {
    if sources.is_empty() {
        return Err(FormatError::Unsupported(
            "PAR2 protect requires at least one source file".into(),
        ));
    }
    for source in sources {
        ensure_file(source)?;
    }
    let recovery = recovery_path_or_default(archive, recovery);
    let tool = find_tool()?;
    let redundancy_arg = format!("-r{redundancy}");
    let mut args = vec![
        OsString::from("create"),
        OsString::from(redundancy_arg),
        recovery.as_os_str().to_owned(),
    ];
    args.extend(sources.iter().map(|source| source.as_os_str().to_owned()));
    let output = run_tool(&tool, &args)?;
    Ok(report(
        "protect",
        archive,
        &recovery,
        None,
        &tool,
        Some(redundancy),
        &output,
    ))
}

/// Verifies external PAR2 data for an archive.
pub fn verify(archive: &Path, recovery: Option<&Path>) -> Result<RecoveryReport, FormatError> {
    let recovery = recovery_path_or_default(archive, recovery);
    ensure_file(&recovery)?;
    match find_tool() {
        Ok(tool) => {
            let args = vec![OsString::from("verify"), recovery.as_os_str().to_owned()];
            let output = run_tool(&tool, &args)?;
            Ok(report(
                "verify", archive, &recovery, None, &tool, None, &output,
            ))
        }
        Err(e) if default_tool_missing(&e) => verify_with_rust_par2(archive, &recovery),
        Err(e) => Err(e),
    }
}

/// Repairs an archive with external PAR2 data.
pub fn repair(
    archive: &Path,
    output: Option<&Path>,
    recovery: Option<&Path>,
) -> Result<RecoveryReport, FormatError> {
    ensure_file(archive)?;
    let recovery = recovery_path_or_default(archive, recovery);
    ensure_file(&recovery)?;
    if let Some(output) = output {
        if output != archive {
            return repair_to_output(archive, output, &recovery);
        }
    }
    repair_in_place(archive, &recovery)
}

fn repair_in_place(archive: &Path, recovery: &Path) -> Result<RecoveryReport, FormatError> {
    match find_tool() {
        Ok(tool) => {
            let args = vec![OsString::from("repair"), recovery.as_os_str().to_owned()];
            let output = run_tool(&tool, &args)?;
            Ok(report(
                "repair", archive, recovery, None, &tool, None, &output,
            ))
        }
        Err(e) if default_tool_missing(&e) => repair_with_rust_par2(archive, recovery, recovery),
        Err(e) => Err(e),
    }
}

/// Default sidecar index path: `<archive-file-name>.par2` next to archive.
pub fn default_recovery_path(archive: &Path) -> PathBuf {
    let name = file_name_or_archive(archive);
    archive.with_file_name(format!("{name}.par2"))
}

fn recovery_path_or_default(archive: &Path, recovery: Option<&Path>) -> PathBuf {
    match recovery {
        Some(path) => path.to_path_buf(),
        None => default_recovery_path(archive),
    }
}

fn file_name_or_archive(path: &Path) -> String {
    match path.file_name() {
        Some(name) => name.to_string_lossy().into_owned(),
        None => "archive".to_owned(),
    }
}

fn repair_to_output(
    archive: &Path,
    output: &Path,
    recovery: &Path,
) -> Result<RecoveryReport, FormatError> {
    let work_dir = unique_work_dir(output)?;
    fs::create_dir(&work_dir).map_err(FormatError::Io)?;
    let result = (|| {
        let work_archive = copy_named(archive, &work_dir)?;
        let work_recovery = copy_recovery_set(recovery, &work_dir)?;
        let mut report = match find_tool() {
            Ok(tool) => {
                let args = vec![
                    OsString::from("repair"),
                    work_recovery.as_os_str().to_owned(),
                ];
                let tool_output = run_tool(&tool, &args)?;
                report(
                    "repair",
                    archive,
                    recovery,
                    Some(output),
                    &tool,
                    None,
                    &tool_output,
                )
            }
            Err(e) if default_tool_missing(&e) => {
                repair_with_rust_par2(archive, recovery, &work_recovery)?
            }
            Err(e) => return Err(e),
        };
        report.output = Some(output.to_path_buf());
        if report.ok {
            persist_repaired_output(&work_archive, output)?;
        }
        Ok(report)
    })();
    let _ = fs::remove_dir_all(&work_dir);
    result
}

fn unique_work_dir(output: &Path) -> Result<PathBuf, FormatError> {
    let parent = parent_dir(output);
    let name = output.file_name().ok_or_else(|| {
        FormatError::Unsupported(format!(
            "PAR2 repair output must name a file: {}",
            output.display()
        ))
    })?;
    Ok(parent.join(format!(
        ".{}.sqz-par2-repair-{}-{}.work",
        name.to_string_lossy(),
        std::process::id(),
        unique_nonce()
    )))
}

fn parent_dir(path: &Path) -> &Path {
    match path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        Some(parent) => parent,
        None => Path::new("."),
    }
}

fn unique_nonce() -> u128 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_nanos(),
        Err(_) => 0,
    }
}

fn copy_named(src: &Path, dest_dir: &Path) -> Result<PathBuf, FormatError> {
    let name = src.file_name().ok_or_else(|| {
        FormatError::Unsupported(format!("path must name a file: {}", src.display()))
    })?;
    let dest = dest_dir.join(name);
    fs::copy(src, &dest).map_err(FormatError::Io)?;
    Ok(dest)
}

fn copy_recovery_set(recovery: &Path, dest_dir: &Path) -> Result<PathBuf, FormatError> {
    let recovery_name = recovery.file_name().ok_or_else(|| {
        FormatError::Unsupported(format!(
            "PAR2 recovery path must name a file: {}",
            recovery.display()
        ))
    })?;
    let work_recovery = copy_named(recovery, dest_dir)?;
    let Some(stem) = recovery.file_stem().and_then(|stem| stem.to_str()) else {
        return Ok(work_recovery);
    };
    let prefix = format!("{stem}.vol");
    let dir = parent_dir(recovery);
    for entry in fs::read_dir(dir).map_err(FormatError::Io)? {
        let entry = entry.map_err(FormatError::Io)?;
        let name = entry.file_name();
        if name == recovery_name {
            continue;
        }
        let name_text = name.to_string_lossy();
        if name_text.starts_with(&prefix) && name_text.to_ascii_lowercase().ends_with(".par2") {
            fs::copy(entry.path(), dest_dir.join(&name)).map_err(FormatError::Io)?;
        }
    }
    Ok(work_recovery)
}

fn persist_repaired_output(repaired_archive: &Path, output: &Path) -> Result<(), FormatError> {
    let parent = parent_dir(output);
    let name = output.file_name().ok_or_else(|| {
        FormatError::Unsupported(format!(
            "PAR2 repair output must name a file: {}",
            output.display()
        ))
    })?;
    let part = parent.join(format!(
        ".{}.sqz-par2-repair-{}-{}.part",
        name.to_string_lossy(),
        std::process::id(),
        unique_nonce()
    ));
    fs::rename(repaired_archive, &part).map_err(FormatError::Io)?;
    match fs::rename(&part, output) {
        Ok(()) => Ok(()),
        Err(_) if output.exists() => {
            fs::remove_file(output).map_err(FormatError::Io)?;
            fs::rename(&part, output).map_err(FormatError::Io)?;
            Ok(())
        }
        Err(err) => Err(FormatError::Io(err)),
    }
}

fn report(
    operation: &'static str,
    archive: &Path,
    recovery: &Path,
    output_path: Option<&Path>,
    tool: &Path,
    redundancy: Option<u8>,
    output: &Output,
) -> RecoveryReport {
    RecoveryReport {
        ok: output.status.success(),
        operation,
        archive: archive.to_path_buf(),
        recovery: recovery.to_path_buf(),
        output: output_path.map(Path::to_path_buf),
        tool: tool.to_path_buf(),
        redundancy_percent: redundancy,
        status_code: output.status.code(),
        metrics: None,
        stdout: String::from_utf8_lossy(&output.stdout).trim().to_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
    }
}

fn rust_report(
    operation: &'static str,
    archive: &Path,
    recovery: &Path,
    ok: bool,
    metrics: RecoveryMetrics,
    stdout: String,
    stderr: String,
) -> RecoveryReport {
    RecoveryReport {
        ok,
        operation,
        archive: archive.to_path_buf(),
        recovery: recovery.to_path_buf(),
        output: None,
        tool: PathBuf::from(RUST_PAR2_TOOL),
        redundancy_percent: None,
        status_code: None,
        metrics: Some(metrics),
        stdout,
        stderr,
    }
}

fn verify_with_rust_par2(archive: &Path, recovery: &Path) -> Result<RecoveryReport, FormatError> {
    let set = parse_par2(recovery)?;
    let dir = parent_dir(recovery);
    let result = rust_par2::verify(&set, dir);
    let ok = result.all_correct();
    let stdout = format_verify_result(&result);
    let metrics = metrics_from_verify(&result);
    let stderr = if ok {
        String::new()
    } else {
        "PAR2 verify found damaged or missing files".to_owned()
    };
    Ok(rust_report(
        "verify", archive, recovery, ok, metrics, stdout, stderr,
    ))
}

fn repair_with_rust_par2(
    archive: &Path,
    report_recovery: &Path,
    work_recovery: &Path,
) -> Result<RecoveryReport, FormatError> {
    let set = parse_par2(work_recovery)?;
    let dir = parent_dir(work_recovery);
    let verify = rust_par2::verify(&set, dir);
    if verify.all_correct() {
        return Ok(rust_report(
            "repair",
            archive,
            report_recovery,
            true,
            repair_metrics(&verify, None, None, true),
            format!("{}\nno_damage=true", format_verify_result(&verify)),
            String::new(),
        ));
    }

    match rust_par2::repair_from_verify(&set, dir, &verify) {
        Ok(result) if result.success => Ok(rust_report(
            "repair",
            archive,
            report_recovery,
            true,
            repair_metrics(
                &verify,
                Some(result.blocks_repaired),
                Some(result.files_repaired),
                false,
            ),
            format!(
                "{}\nblocks_repaired={}\nfiles_repaired={}",
                format_verify_result(&verify),
                result.blocks_repaired,
                result.files_repaired
            ),
            String::new(),
        )),
        Ok(result) => Ok(rust_report(
            "repair",
            archive,
            report_recovery,
            false,
            repair_metrics(
                &verify,
                Some(result.blocks_repaired),
                Some(result.files_repaired),
                false,
            ),
            format_verify_result(&verify),
            result.message,
        )),
        Err(rust_par2::RepairError::NoDamage) => Ok(rust_report(
            "repair",
            archive,
            report_recovery,
            true,
            repair_metrics(&verify, None, None, true),
            format!("{}\nno_damage=true", format_verify_result(&verify)),
            String::new(),
        )),
        Err(err) => Ok(rust_report(
            "repair",
            archive,
            report_recovery,
            false,
            repair_metrics(&verify, None, None, false),
            format_verify_result(&verify),
            err.to_string(),
        )),
    }
}

fn metrics_from_verify(result: &rust_par2::VerifyResult) -> RecoveryMetrics {
    RecoveryMetrics {
        all_correct: result.all_correct(),
        repair_possible: result.repair_possible,
        blocks_needed: result.blocks_needed(),
        recovery_blocks_available: result.recovery_blocks_available,
        blocks_repaired: None,
        files_repaired: None,
        no_damage: result.all_correct(),
    }
}

fn repair_metrics(
    verify: &rust_par2::VerifyResult,
    blocks_repaired: Option<u32>,
    files_repaired: Option<usize>,
    no_damage: bool,
) -> RecoveryMetrics {
    RecoveryMetrics {
        blocks_repaired,
        files_repaired,
        no_damage,
        ..metrics_from_verify(verify)
    }
}

fn parse_par2(recovery: &Path) -> Result<rust_par2::Par2FileSet, FormatError> {
    rust_par2::parse(recovery)
        .map_err(|e| FormatError::CorruptArchive(format!("cannot parse PAR2 data: {e}")))
}

fn format_verify_result(result: &rust_par2::VerifyResult) -> String {
    format!(
        "all_correct={}\nrepair_possible={}\nblocks_needed={}\navailable={}",
        result.all_correct(),
        result.repair_possible,
        result.blocks_needed(),
        result.recovery_blocks_available
    )
}

fn default_tool_missing(error: &FormatError) -> bool {
    matches!(error, FormatError::DependencyMissing(name) if name == DEFAULT_TOOL_MISSING)
}

fn run_tool(tool: &Path, args: &[OsString]) -> Result<Output, FormatError> {
    Command::new(tool)
        .args(args)
        .output()
        .map_err(FormatError::Io)
}

fn find_tool() -> Result<PathBuf, FormatError> {
    if let Ok(value) = env::var(TOOL_ENV) {
        let path = PathBuf::from(value);
        if path.is_file() {
            return Ok(path);
        }
        return Err(FormatError::DependencyMissing(format!(
            "{TOOL_ENV} ({})",
            path.display()
        )));
    }

    let Some(paths) = env::var_os("PATH") else {
        return Err(FormatError::DependencyMissing(DEFAULT_TOOL_MISSING.into()));
    };
    for dir in env::split_paths(&paths) {
        for name in TOOL_CANDIDATES {
            let path = dir.join(name);
            if path.is_file() {
                return Ok(path);
            }
            #[cfg(windows)]
            {
                let path = dir.join(format!("{name}.exe"));
                if path.is_file() {
                    return Ok(path);
                }
            }
        }
    }
    Err(FormatError::DependencyMissing(DEFAULT_TOOL_MISSING.into()))
}

fn ensure_file(path: &Path) -> Result<(), FormatError> {
    if path.is_file() {
        Ok(())
    } else {
        Err(FormatError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("missing file: {}", path.display()),
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_path_keeps_full_archive_name() {
        assert_eq!(
            default_recovery_path(Path::new("/tmp/data.7z")),
            PathBuf::from("/tmp/data.7z.par2")
        );
        assert_eq!(
            default_recovery_path(Path::new("/tmp/data.7z.001")),
            PathBuf::from("/tmp/data.7z.001.par2")
        );
    }

    #[test]
    fn rust_verify_metrics_are_structured() {
        let result = rust_par2::VerifyResult {
            intact: Vec::new(),
            damaged: vec![rust_par2::DamagedFile {
                filename: "damaged.bin".to_owned(),
                size: 4096,
                damaged_block_count: 2,
                total_block_count: 4,
                damaged_block_indices: vec![1, 3],
            }],
            missing: vec![rust_par2::MissingFile {
                filename: "missing.bin".to_owned(),
                expected_size: 2048,
                block_count: 1,
            }],
            recovery_blocks_available: 4,
            repair_possible: true,
        };

        assert_eq!(
            metrics_from_verify(&result),
            RecoveryMetrics {
                all_correct: false,
                repair_possible: true,
                blocks_needed: 3,
                recovery_blocks_available: 4,
                blocks_repaired: None,
                files_repaired: None,
                no_damage: false,
            }
        );
    }

    #[test]
    fn rust_repair_metrics_include_repair_counts() {
        let result = rust_par2::VerifyResult {
            intact: Vec::new(),
            damaged: Vec::new(),
            missing: Vec::new(),
            recovery_blocks_available: 1,
            repair_possible: true,
        };

        assert_eq!(
            repair_metrics(&result, Some(1), Some(1), false),
            RecoveryMetrics {
                all_correct: true,
                repair_possible: true,
                blocks_needed: 0,
                recovery_blocks_available: 1,
                blocks_repaired: Some(1),
                files_repaired: Some(1),
                no_damage: false,
            }
        );
    }
}
