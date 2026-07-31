#![forbid(unsafe_code)]
//! Recovery data operations shared by the CLI and desktop app.
//!
//! Standard PAR2 sidecar support uses a Rust verifier/repairer with an
//! optional par2cmdline-compatible backend. PAR2 creation remains an external
//! tool boundary so Squallz writes interoperable recovery sets.

mod repair_workspace;

use std::collections::HashSet;
use std::env;
use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use squallz_format_api::{
    ControlToken, EntryPath, FormatError, NoProgress, ProgressPhase, ProgressSink,
};

use repair_workspace::{RepairWorkspace, RepairWorkspaceTarget, WorkspaceDebt};

const TOOL_ENV: &str = "SQUALLZ_PAR2";
const TOOL_CANDIDATES: [&str; 3] = ["par2cmdline-turbo", "par2", "par2cmdline"];
const DEFAULT_TOOL_MISSING: &str = "par2cmdline-turbo/par2";
const RUST_PAR2_TOOL: &str = "rust-par2";
const COPY_BUFFER_BYTES: usize = 256 * 1024;
const TOOL_POLL_INTERVAL: Duration = Duration::from_millis(25);
const TOOL_PROGRESS_RECORD_BYTES: usize = 1024;
const TOOL_PROGRESS_TOTAL: u64 = 1000;
const MAX_GENERATED_PAR2_FILES: usize = 32;

/// Machine-readable result for PAR2 operations.
#[derive(Debug, Clone, Serialize)]
pub struct RecoveryReport {
    pub ok: bool,
    pub operation: &'static str,
    pub archive: PathBuf,
    pub recovery: PathBuf,
    pub outputs: Vec<PathBuf>,
    pub output: Option<PathBuf>,
    pub tool: PathBuf,
    pub redundancy_percent: Option<u8>,
    pub source_file_count: usize,
    pub status_code: Option<i32>,
    pub metrics: Option<RecoveryMetrics>,
    pub stdout: String,
    pub stderr: String,
}

/// Structured recovery math when the backend exposes a consistent summary.
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

/// Exact paths retained when a PAR2 repair workspace could not be removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryCleanupDetails {
    /// Requested repaired-copy destination.
    pub target: PathBuf,
    /// Private workspace owned by this repair attempt, when its record was
    /// valid enough to bind an exact path.
    pub workspace: Option<PathBuf>,
    /// Target-bound persistent record required for automatic cleanup replay.
    pub journal: PathBuf,
    /// `true` only when the repaired copy was published and verified.
    pub output_ready: bool,
}

#[derive(Debug)]
struct RecoveryCleanupIoError {
    message: String,
    details: RecoveryCleanupDetails,
}

impl fmt::Display for RecoveryCleanupIoError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for RecoveryCleanupIoError {}

/// Returns the exact cleanup debt attached to a failed PAR2 repair.
pub fn recovery_cleanup_details(error: &FormatError) -> Option<RecoveryCleanupDetails> {
    let FormatError::Io(error) = error else {
        return None;
    };
    error
        .get_ref()
        .and_then(|source| source.downcast_ref::<RecoveryCleanupIoError>())
        .map(|source| source.details.clone())
}

/// Builds external PAR2 data for one archive or split-set head.
pub fn protect(
    archive: &Path,
    redundancy: u8,
    recovery: Option<&Path>,
) -> Result<RecoveryReport, FormatError> {
    let progress = NoProgress;
    let control = ControlToken::default();
    protect_controlled(archive, redundancy, recovery, &progress, &control)
}

/// Builds external PAR2 data while reporting its real backend stages.
pub fn protect_controlled(
    archive: &Path,
    redundancy: u8,
    recovery: Option<&Path>,
    progress: &dyn ProgressSink,
    control: &ControlToken,
) -> Result<RecoveryReport, FormatError> {
    protect_files_controlled(
        archive,
        redundancy,
        recovery,
        &[archive.to_path_buf()],
        progress,
        control,
    )
}

/// Builds external PAR2 data for an explicit set of source files.
pub fn protect_files(
    archive: &Path,
    redundancy: u8,
    recovery: Option<&Path>,
    sources: &[PathBuf],
) -> Result<RecoveryReport, FormatError> {
    let progress = NoProgress;
    let control = ControlToken::default();
    protect_files_controlled(archive, redundancy, recovery, sources, &progress, &control)
}

/// Builds external PAR2 data for an explicit source set with stage progress.
///
/// The backend writes into a private sibling directory. Its complete output
/// set is validated and hashed before a durable, no-replace publication makes
/// recovery volumes visible beside the requested index.
pub fn protect_files_controlled(
    archive: &Path,
    redundancy: u8,
    recovery: Option<&Path>,
    sources: &[PathBuf],
    progress: &dyn ProgressSink,
    control: &ControlToken,
) -> Result<RecoveryReport, FormatError> {
    control.checkpoint()?;
    if sources.is_empty() {
        return Err(FormatError::Unsupported(
            "PAR2 protect requires at least one source file".into(),
        ));
    }
    if !(1..=100).contains(&redundancy) {
        return Err(FormatError::Unsupported(
            "PAR2 redundancy must be a whole percentage from 1 to 100".into(),
        ));
    }
    for source in sources {
        ensure_file(source)?;
    }
    let recovery = recovery_path_or_default(archive, recovery);
    validate_protect_layout(&recovery, sources)?;
    squallz_core::recover_file_set_publication(&recovery)?;
    ensure_output_available(&recovery)?;
    let tool = find_tool()?;
    let work_dir = unique_protect_work_dir(&recovery)?;
    create_private_directory(&work_dir)?;
    let work_recovery = work_dir.join(recovery.file_name().ok_or_else(|| {
        FormatError::Unsupported(format!(
            "PAR2 recovery path must name a file: {}",
            recovery.display()
        ))
    })?);
    let current = recovery_progress_entry(archive);
    let result = (|| {
        let redundancy_arg = format!("-r{redundancy}");
        let source_base = fs::canonicalize(parent_dir(&recovery)).map_err(FormatError::from)?;
        let mut source_base_arg = OsString::from("-B");
        source_base_arg.push(source_base.as_os_str());
        let mut args = vec![
            OsString::from("create"),
            OsString::from(redundancy_arg),
            source_base_arg.clone(),
            work_recovery.as_os_str().to_owned(),
        ];
        args.extend(sources.iter().map(|source| source.as_os_str().to_owned()));
        control.checkpoint()?;
        progress.on_phase(ProgressPhase::RecoveryPrepare, true);
        progress.on_progress(0, 0, &current);
        let output = run_tool_controlled(
            &tool,
            &args,
            archive,
            progress,
            control,
            ProgressPhase::RecoveryPrepare,
            true,
        )?;
        let mut report = report(
            ReportScope::new("protect", archive, &recovery, sources.len()),
            None,
            &tool,
            Some(redundancy),
            &output,
        );
        if !report.ok {
            return Ok(report);
        }

        progress.on_phase(ProgressPhase::RecoveryVerify, true);
        progress.on_progress(0, 0, &current);
        let staged = validate_generated_par2_set(&work_dir, &work_recovery, &recovery, sources)?;
        let verify_args = vec![
            OsString::from("verify"),
            source_base_arg,
            work_recovery.as_os_str().to_owned(),
        ];
        let verification = run_tool_controlled(
            &tool,
            &verify_args,
            archive,
            progress,
            control,
            ProgressPhase::RecoveryVerify,
            true,
        )?;
        if !verification.status.success() {
            return Err(FormatError::CorruptArchive(format!(
                "PAR2 backend could not verify its generated recovery set (status {})",
                verification
                    .status
                    .code()
                    .map_or_else(|| "unknown".to_owned(), |code| code.to_string())
            )));
        }
        progress.on_phase(ProgressPhase::RecoveryVerify, true);
        let prepared = squallz_core::prepare_file_set_publication(
            &recovery, &work_dir, &staged, progress, control,
        )?;
        control.checkpoint()?;
        progress.on_phase(ProgressPhase::RecoveryFinalize, false);
        progress.on_progress(0, 0, &current);
        report.outputs = prepared.commit_no_replace()?;
        progress.on_progress(1, 1, &current);
        Ok(report)
    })();
    finish_protect_work_dir(result, &work_dir, &recovery)
}

/// Verifies external PAR2 data for an archive.
pub fn verify(archive: &Path, recovery: Option<&Path>) -> Result<RecoveryReport, FormatError> {
    let progress = NoProgress;
    let control = ControlToken::default();
    verify_controlled(archive, recovery, &progress, &control)
}

/// Verifies PAR2 data with real external-tool stages and safe cancellation.
pub fn verify_controlled(
    archive: &Path,
    recovery: Option<&Path>,
    progress: &dyn ProgressSink,
    control: &ControlToken,
) -> Result<RecoveryReport, FormatError> {
    control.checkpoint()?;
    let current = recovery_progress_entry(archive);
    progress.on_phase(ProgressPhase::RecoveryPrepare, true);
    progress.on_progress(0, 0, &current);
    ensure_file(archive)?;
    let recovery = recovery_path_or_default(archive, recovery);
    ensure_file(&recovery)?;
    let set = parse_par2_for_operation(archive, &recovery)?;
    control.checkpoint()?;
    match find_tool() {
        Ok(tool) => {
            let args = vec![OsString::from("verify"), recovery.as_os_str().to_owned()];
            progress.on_phase(ProgressPhase::RecoveryVerify, true);
            progress.on_progress(0, 0, &current);
            let output = run_tool_controlled(
                &tool,
                &args,
                archive,
                progress,
                control,
                ProgressPhase::RecoveryVerify,
                true,
            )?;
            Ok(external_recovery_report(
                ReportScope::new("verify", archive, &recovery, set.files.len()),
                None,
                &tool,
                None,
                &output,
            ))
        }
        Err(e) if default_tool_missing(&e) => {
            progress.on_phase(ProgressPhase::RecoveryVerify, false);
            progress.on_progress(0, 0, &current);
            let report = verify_with_rust_par2(archive, &recovery, &set)?;
            progress.on_progress(1, 1, &current);
            Ok(report)
        }
        Err(e) => Err(e),
    }
}

/// Repairs an archive with external PAR2 data.
pub fn repair(
    archive: &Path,
    output: Option<&Path>,
    recovery: Option<&Path>,
) -> Result<RecoveryReport, FormatError> {
    let progress = NoProgress;
    let control = ControlToken::default();
    repair_controlled(archive, output, recovery, &progress, &control)
}

/// Repairs PAR2 data with controlled private staging and truthful phases.
pub fn repair_controlled(
    archive: &Path,
    output: Option<&Path>,
    recovery: Option<&Path>,
    progress: &dyn ProgressSink,
    control: &ControlToken,
) -> Result<RecoveryReport, FormatError> {
    control.checkpoint()?;
    if let Some(output) = output {
        let same_path = output == archive
            || matches!(
                (fs::canonicalize(archive), fs::canonicalize(output)),
                (Ok(archive_path), Ok(output_path)) if archive_path == output_path
            );
        if same_path {
            return Err(FormatError::Unsupported(
                "PAR2 repair output must differ from the source archive".into(),
            ));
        }
    }
    let workspace_target = output
        .map(|output| prepare_repair_workspace_target(output, control))
        .transpose()?;
    ensure_file(archive)?;
    if let Some(output) = output {
        ensure_output_available(output)?;
    }
    let recovery = recovery_path_or_default(archive, recovery);
    ensure_file(&recovery)?;
    let set = parse_par2_for_operation(archive, &recovery)?;
    if output.is_some() {
        validate_single_file_output(archive, &set)?;
    }
    if let Some(output) = output {
        let workspace_target = workspace_target.ok_or_else(|| {
            FormatError::Other("PAR2 repair workspace target was not prepared".to_owned())
        })?;
        return repair_to_output_controlled(
            archive,
            output,
            &recovery,
            &set,
            workspace_target,
            progress,
            control,
        );
    }
    repair_in_place(archive, &recovery, &set, progress, control)
}

/// Repairs every file described by a PAR2 set into a new output directory.
///
/// Existing source files are copied into a private workspace and never
/// modified. A successful result publishes only the exact files described by
/// the PAR2 set, preserving their relative paths. The output directory must
/// not already exist.
pub fn repair_to_directory(
    archive: &Path,
    output: &Path,
    recovery: Option<&Path>,
) -> Result<RecoveryReport, FormatError> {
    let progress = NoProgress;
    let control = ControlToken::default();
    repair_to_directory_controlled(archive, output, recovery, &progress, &control)
}

/// Repairs every described file into a new directory with safe cancellation.
pub fn repair_to_directory_controlled(
    archive: &Path,
    output: &Path,
    recovery: Option<&Path>,
    progress: &dyn ProgressSink,
    control: &ControlToken,
) -> Result<RecoveryReport, FormatError> {
    control.checkpoint()?;
    if output == archive {
        return Err(FormatError::Unsupported(
            "PAR2 repair output must differ from the source archive".into(),
        ));
    }
    let workspace_target = prepare_repair_workspace_target(output, control)?;
    ensure_file(archive)?;
    ensure_output_available(output)?;
    let recovery = recovery_path_or_default(archive, recovery);
    ensure_file(&recovery)?;
    let set = parse_par2_for_operation(archive, &recovery)?;
    repair_to_safe_copy(
        archive,
        output,
        &recovery,
        &set,
        SafeCopyDestination::Directory,
        workspace_target,
        progress,
        control,
    )
}

fn repair_in_place(
    archive: &Path,
    recovery: &Path,
    set: &rust_par2::Par2FileSet,
    progress: &dyn ProgressSink,
    control: &ControlToken,
) -> Result<RecoveryReport, FormatError> {
    control.checkpoint()?;
    let current = recovery_progress_entry(archive);
    progress.on_phase(ProgressPhase::RecoveryProcess, false);
    progress.on_progress(0, 0, &current);
    match find_tool() {
        Ok(tool) => {
            let args = vec![OsString::from("repair"), recovery.as_os_str().to_owned()];
            let output = run_tool_controlled(
                &tool,
                &args,
                archive,
                progress,
                control,
                ProgressPhase::RecoveryProcess,
                false,
            )?;
            Ok(external_recovery_report(
                ReportScope::new("repair", archive, recovery, set.files.len()),
                None,
                &tool,
                None,
                &output,
            ))
        }
        Err(e) if default_tool_missing(&e) => {
            let report = repair_with_rust_par2(archive, recovery, recovery, set)?;
            progress.on_progress(1, 1, &current);
            Ok(report)
        }
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

fn recovery_progress_entry(path: &Path) -> EntryPath {
    EntryPath::from_utf8(file_name_or_archive(path))
}

#[cfg(test)]
fn repair_to_output(
    archive: &Path,
    output: &Path,
    recovery: &Path,
    set: &rust_par2::Par2FileSet,
) -> Result<RecoveryReport, FormatError> {
    let progress = NoProgress;
    let control = ControlToken::default();
    let workspace_target = prepare_repair_workspace_target(output, &control)?;
    repair_to_output_controlled(
        archive,
        output,
        recovery,
        set,
        workspace_target,
        &progress,
        &control,
    )
}

fn repair_to_output_controlled(
    archive: &Path,
    output: &Path,
    recovery: &Path,
    set: &rust_par2::Par2FileSet,
    workspace_target: RepairWorkspaceTarget,
    progress: &dyn ProgressSink,
    control: &ControlToken,
) -> Result<RecoveryReport, FormatError> {
    repair_to_safe_copy(
        archive,
        output,
        recovery,
        set,
        SafeCopyDestination::File,
        workspace_target,
        progress,
        control,
    )
}

#[derive(Clone, Copy)]
enum SafeCopyDestination {
    File,
    Directory,
}

#[allow(clippy::too_many_arguments)] // repair inputs plus the already-held destination transaction
fn repair_to_safe_copy(
    archive: &Path,
    output: &Path,
    recovery: &Path,
    set: &rust_par2::Par2FileSet,
    destination: SafeCopyDestination,
    workspace_target: RepairWorkspaceTarget,
    progress: &dyn ProgressSink,
    control: &ControlToken,
) -> Result<RecoveryReport, FormatError> {
    control.checkpoint()?;
    let workspace = workspace_target
        .begin()
        .map_err(|debt| recovery_workspace_error(output, debt, false, None))?;
    let work_dir = workspace.path().to_path_buf();
    let result = (|| {
        let data_dir = work_dir.join("data");
        let recovery_dir = work_dir.join("recovery");
        create_private_directory(&data_dir)?;
        create_private_directory(&recovery_dir)?;
        let mut copy_progress = RecoveryCopyProgress::new(progress, control);
        progress.on_phase(ProgressPhase::RecoveryPrepare, true);
        progress.on_progress(0, 0, &recovery_progress_entry(archive));
        let work_archive =
            copy_source_set_controlled(archive, recovery, &data_dir, set, &mut copy_progress)?;
        let (mut report, verify_external_output) = match find_tool() {
            Ok(tool) => {
                let work_recovery =
                    copy_recovery_set_controlled(recovery, &recovery_dir, &mut copy_progress)?;
                let mut base_arg = OsString::from("-B");
                base_arg.push(data_dir.as_os_str());
                let args = vec![
                    OsString::from("repair"),
                    base_arg,
                    work_recovery.as_os_str().to_owned(),
                ];
                progress.on_phase(ProgressPhase::RecoveryProcess, true);
                progress.on_progress(0, 0, &recovery_progress_entry(archive));
                let tool_output = run_tool_controlled(
                    &tool,
                    &args,
                    archive,
                    progress,
                    control,
                    ProgressPhase::RecoveryProcess,
                    true,
                )?;
                (
                    external_recovery_report(
                        ReportScope::new("repair", archive, recovery, set.files.len()),
                        None,
                        &tool,
                        None,
                        &tool_output,
                    ),
                    true,
                )
            }
            Err(e) if default_tool_missing(&e) => {
                let work_recovery =
                    copy_recovery_set_controlled(recovery, &data_dir, &mut copy_progress)?;
                progress.on_phase(ProgressPhase::RecoveryProcess, false);
                progress.on_progress(0, 0, &recovery_progress_entry(archive));
                let report = repair_with_rust_par2(archive, recovery, &work_recovery, set)?;
                progress.on_progress(1, 1, &recovery_progress_entry(archive));
                (report, false)
            }
            Err(e) => return Err(e),
        };
        if report.ok && verify_external_output {
            progress.on_phase(ProgressPhase::RecoveryVerify, false);
            progress.on_progress(0, 0, &recovery_progress_entry(archive));
            verify_external_repair(&mut report, set, &data_dir);
            progress.on_progress(1, 1, &recovery_progress_entry(archive));
        }
        if !report.ok {
            return Ok(report);
        }

        progress.on_phase(ProgressPhase::RecoveryFinalize, false);
        progress.on_progress(0, 0, &recovery_progress_entry(archive));
        match destination {
            SafeCopyDestination::File => persist_repaired_output(&work_archive, output)?,
            SafeCopyDestination::Directory => {
                let publish_dir = work_dir.join("publish");
                stage_repaired_directory(set, &data_dir, &publish_dir)?;
                squallz_core::publish_directory_no_replace(&publish_dir, output)?;
            }
        }
        progress.on_progress(1, 1, &recovery_progress_entry(archive));
        report.output = Some(output.to_path_buf());
        Ok(report)
    })();
    finish_repair_workspace(result, workspace, output)
}

fn create_private_directory(path: &Path) -> Result<(), FormatError> {
    #[cfg(unix)]
    let builder = {
        use std::os::unix::fs::DirBuilderExt;

        let mut builder = fs::DirBuilder::new();
        builder.mode(0o700);
        builder
    };
    #[cfg(not(unix))]
    let builder = fs::DirBuilder::new();
    builder.create(path).map_err(FormatError::from)
}

fn unique_protect_work_dir(recovery: &Path) -> Result<PathBuf, FormatError> {
    let parent = parent_dir(recovery);
    let name = recovery.file_name().ok_or_else(|| {
        FormatError::Unsupported(format!(
            "PAR2 recovery path must name a file: {}",
            recovery.display()
        ))
    })?;
    Ok(parent.join(format!(
        ".{}.sqz-par2-protect-{}-{}.work",
        name.to_string_lossy(),
        std::process::id(),
        unique_nonce()
    )))
}

fn finish_protect_work_dir(
    result: Result<RecoveryReport, FormatError>,
    work_dir: &Path,
    recovery: &Path,
) -> Result<RecoveryReport, FormatError> {
    if result.is_err() && squallz_core::file_set_publication_pending(recovery) {
        return result;
    }
    match fs::remove_dir_all(work_dir) {
        Ok(()) => result,
        Err(error) if error.kind() == io::ErrorKind::NotFound => result,
        Err(cleanup) => match result {
            Ok(_) => Err(FormatError::Other(format!(
                "PAR2 recovery data was created, but private staging cleanup failed: {cleanup}"
            ))),
            Err(error) => Err(FormatError::Other(format!(
                "{error}; private PAR2 staging cleanup also failed: {cleanup}"
            ))),
        },
    }
}

fn prepare_repair_workspace_target(
    output: &Path,
    control: &ControlToken,
) -> Result<RepairWorkspaceTarget, FormatError> {
    let target = RepairWorkspaceTarget::lock(output, control)?;
    target
        .recover_pending()
        .map_err(|debt| recovery_workspace_error(output, debt, false, None))?;
    Ok(target)
}

fn finish_repair_workspace(
    result: Result<RecoveryReport, FormatError>,
    workspace: RepairWorkspace,
    output: &Path,
) -> Result<RecoveryReport, FormatError> {
    match workspace.cleanup() {
        Ok(()) => result,
        Err(debt) => {
            let output_ready = result
                .as_ref()
                .is_ok_and(|report| report.ok && report.output.as_deref() == Some(output));
            let operation_error = result.as_ref().err();
            Err(recovery_workspace_error(
                output,
                debt,
                output_ready,
                operation_error,
            ))
        }
    }
}

fn recovery_workspace_error(
    output: &Path,
    debt: WorkspaceDebt,
    output_ready: bool,
    operation_error: Option<&FormatError>,
) -> FormatError {
    let operation = match (operation_error, output_ready) {
        (_, true) => format!(
            "PAR2 repair completed and the repaired copy is ready at {}, but private workspace \
             cleanup needs attention",
            output.display()
        ),
        (Some(error), false) => format!("{error}; private PAR2 repair cleanup needs attention"),
        (None, false) => {
            "PAR2 repair did not produce a verified repaired copy, and private workspace cleanup \
             needs attention"
                .to_owned()
        }
    };
    let workspace = debt
        .workspace
        .as_ref()
        .map(|path| format!("; exact private workspace: {}", path.display()))
        .unwrap_or_default();
    FormatError::Io(io::Error::other(RecoveryCleanupIoError {
        message: format!(
            "{operation}: {}; automatic recovery record: {}{workspace}; no unbound path was \
             removed",
            debt.reason,
            debt.journal.display()
        ),
        details: RecoveryCleanupDetails {
            target: output.to_path_buf(),
            workspace: debt.workspace,
            journal: debt.journal,
            output_ready,
        },
    }))
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

fn copy_named_controlled(
    src: &Path,
    dest_dir: &Path,
    copy_progress: &mut RecoveryCopyProgress<'_>,
) -> Result<PathBuf, FormatError> {
    let name = src.file_name().ok_or_else(|| {
        FormatError::Unsupported(format!("path must name a file: {}", src.display()))
    })?;
    let dest = dest_dir.join(name);
    copy_regular_file_controlled(src, &dest, "PAR2 recovery file", copy_progress)?;
    Ok(dest)
}

#[cfg(test)]
fn copy_source_set(
    archive: &Path,
    recovery: &Path,
    dest_dir: &Path,
    set: &rust_par2::Par2FileSet,
) -> Result<PathBuf, FormatError> {
    let progress = NoProgress;
    let control = ControlToken::default();
    let mut copy_progress = RecoveryCopyProgress::new(&progress, &control);
    copy_source_set_controlled(archive, recovery, dest_dir, set, &mut copy_progress)
}

fn copy_source_set_controlled(
    archive: &Path,
    recovery: &Path,
    dest_dir: &Path,
    set: &rust_par2::Par2FileSet,
    copy_progress: &mut RecoveryCopyProgress<'_>,
) -> Result<PathBuf, FormatError> {
    copy_progress.checkpoint()?;
    let source_dir = fs::canonicalize(parent_dir(recovery)).map_err(FormatError::from)?;
    let archive_member = archive_member_path(archive, &source_dir, set)?;
    let mut members: Vec<_> = set.files.values().collect();
    members.sort_by(|left, right| left.filename.cmp(&right.filename));
    for member in members {
        copy_progress.checkpoint()?;
        let relative = Path::new(&member.filename);
        if let Some(source) = source_member_path(&source_dir, relative)? {
            let dest = dest_dir.join(relative);
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent).map_err(FormatError::from)?;
            }
            copy_regular_file_controlled(&source, &dest, "PAR2 source file", copy_progress)?;
        }
    }
    copy_progress.checkpoint()?;
    Ok(dest_dir.join(archive_member))
}

fn archive_member_path(
    archive: &Path,
    source_dir: &Path,
    set: &rust_par2::Par2FileSet,
) -> Result<PathBuf, FormatError> {
    let archive_identity = fs::canonicalize(archive).map_err(FormatError::from)?;
    for file in set.files.values() {
        let relative = PathBuf::from(&file.filename);
        let Some(candidate) = source_member_path(source_dir, &relative)? else {
            continue;
        };
        if fs::canonicalize(candidate).map_err(FormatError::from)? == archive_identity {
            return Ok(relative);
        }
    }
    Err(FormatError::Unsupported(
        "the selected PAR2 data does not describe the selected archive".into(),
    ))
}

fn source_member_path(root: &Path, relative: &Path) -> Result<Option<PathBuf>, FormatError> {
    let mut current = root.to_path_buf();
    let mut components = relative.components().peekable();
    while let Some(component) = components.next() {
        let Component::Normal(name) = component else {
            return Err(FormatError::UnsafeFileName(
                "PAR2 data contains an unsafe file name".into(),
            ));
        };
        current.push(name);
        let metadata = match fs::symlink_metadata(&current) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(FormatError::from(error)),
        };
        if metadata.file_type().is_symlink() {
            return Err(FormatError::SymlinkBreakout(
                "PAR2 source path crosses a symbolic link".into(),
            ));
        }
        if components.peek().is_some() {
            if !metadata.file_type().is_dir() {
                return Err(FormatError::CorruptArchive(
                    "PAR2 source path has a non-directory parent".into(),
                ));
            }
        } else if metadata.file_type().is_file() {
            return Ok(Some(current));
        } else {
            return Err(FormatError::CorruptArchive(
                "PAR2 source is not a regular file".into(),
            ));
        }
    }
    Err(FormatError::UnsafeFileName(
        "PAR2 data contains an empty file name".into(),
    ))
}

struct RecoveryCopyProgress<'a> {
    progress: &'a dyn ProgressSink,
    control: &'a ControlToken,
    done: u64,
}

impl<'a> RecoveryCopyProgress<'a> {
    fn new(progress: &'a dyn ProgressSink, control: &'a ControlToken) -> Self {
        Self {
            progress,
            control,
            done: 0,
        }
    }

    fn checkpoint(&self) -> Result<(), FormatError> {
        self.control.checkpoint()
    }
}

fn copy_regular_file_controlled(
    src: &Path,
    dest: &Path,
    kind: &str,
    copy_progress: &mut RecoveryCopyProgress<'_>,
) -> Result<(), FormatError> {
    copy_progress.checkpoint()?;
    let metadata = fs::symlink_metadata(src).map_err(FormatError::from)?;
    if !metadata.file_type().is_file() {
        return Err(FormatError::SymlinkBreakout(format!(
            "{kind} must be a regular file"
        )));
    }
    let total = metadata.len();
    let current = recovery_progress_entry(src);
    let result = (|| {
        let mut input = fs::File::open(src).map_err(FormatError::from)?;
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut output = options.open(dest).map_err(FormatError::from)?;
        let mut buffer = vec![0; COPY_BUFFER_BYTES];
        let mut current_done = 0u64;
        loop {
            copy_progress.checkpoint()?;
            let read = input.read(&mut buffer).map_err(FormatError::from)?;
            if read == 0 {
                break;
            }
            output
                .write_all(&buffer[..read])
                .map_err(FormatError::from)?;
            let read = read as u64;
            current_done = current_done.saturating_add(read);
            copy_progress.done = copy_progress.done.saturating_add(read);
            copy_progress.progress.on_entry_progress(
                copy_progress.done,
                0,
                &current,
                current_done,
                total,
            );
        }
        if total == 0 {
            copy_progress
                .progress
                .on_entry_progress(copy_progress.done, 0, &current, 0, 0);
        }
        output.sync_all().map_err(FormatError::from)?;
        copy_progress.checkpoint()
    })();
    if result.is_err() {
        let _ = fs::remove_file(dest);
    }
    result
}

fn copy_recovery_set_controlled(
    recovery: &Path,
    dest_dir: &Path,
    copy_progress: &mut RecoveryCopyProgress<'_>,
) -> Result<PathBuf, FormatError> {
    copy_progress.checkpoint()?;
    let recovery_name = recovery.file_name().ok_or_else(|| {
        FormatError::Unsupported(format!(
            "PAR2 recovery path must name a file: {}",
            recovery.display()
        ))
    })?;
    let work_recovery = copy_named_controlled(recovery, dest_dir, copy_progress)?;
    let Some(stem) = recovery.file_stem().and_then(|stem| stem.to_str()) else {
        return Ok(work_recovery);
    };
    let prefix = format!("{stem}.vol");
    let dir = parent_dir(recovery);
    let mut volumes = Vec::new();
    for entry in fs::read_dir(dir).map_err(FormatError::from)? {
        copy_progress.checkpoint()?;
        let entry = entry.map_err(FormatError::from)?;
        let name = entry.file_name();
        if name == recovery_name {
            continue;
        }
        let name_text = name.to_string_lossy();
        if name_text.starts_with(&prefix) && name_text.to_ascii_lowercase().ends_with(".par2") {
            volumes.push((name, entry.path()));
        }
    }
    volumes.sort_by(|left, right| left.0.cmp(&right.0));
    for (name, path) in volumes {
        copy_regular_file_controlled(
            &path,
            &dest_dir.join(name),
            "PAR2 recovery volume",
            copy_progress,
        )?;
    }
    copy_progress.checkpoint()?;
    Ok(work_recovery)
}

fn verify_external_repair(
    report: &mut RecoveryReport,
    set: &rust_par2::Par2FileSet,
    data_dir: &Path,
) {
    let verification = rust_par2::verify(set, data_dir);
    if verification.all_correct() {
        return;
    }
    report.ok = false;
    report.metrics = Some(metrics_from_verify(&verification));
    let detail =
        "PAR2 backend reported success, but the repaired files did not pass checksum verification";
    report.stderr = if report.stderr.is_empty() {
        detail.to_owned()
    } else {
        format!("{}\n{detail}", report.stderr)
    };
}

fn stage_repaired_directory(
    set: &rust_par2::Par2FileSet,
    data_dir: &Path,
    publish_dir: &Path,
) -> Result<(), FormatError> {
    create_private_directory(publish_dir)?;
    let mut members: Vec<_> = set.files.values().collect();
    members.sort_by(|left, right| left.filename.cmp(&right.filename));
    for member in members {
        let relative = Path::new(&member.filename);
        validate_staged_member(data_dir, relative)?;
        let source = data_dir.join(relative);
        let destination = publish_dir.join(relative);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).map_err(FormatError::from)?;
        }
        squallz_core::move_path_no_replace(&source, &destination).map_err(FormatError::from)?;
    }
    Ok(())
}

fn validate_staged_member(root: &Path, relative: &Path) -> Result<(), FormatError> {
    let mut current = root.to_path_buf();
    for component in relative.components() {
        let Component::Normal(name) = component else {
            return Err(FormatError::UnsafeFileName(
                "PAR2 data contains an unsafe file name".into(),
            ));
        };
        current.push(name);
        let metadata = fs::symlink_metadata(&current).map_err(FormatError::from)?;
        if metadata.file_type().is_symlink() {
            return Err(FormatError::SymlinkBreakout(
                "PAR2 repair produced a symbolic-link target".into(),
            ));
        }
    }
    let metadata = fs::symlink_metadata(&current).map_err(FormatError::from)?;
    if metadata.file_type().is_file() {
        Ok(())
    } else {
        Err(FormatError::CorruptArchive(
            "PAR2 repair did not produce every described regular file".into(),
        ))
    }
}

fn persist_repaired_output(repaired_archive: &Path, output: &Path) -> Result<(), FormatError> {
    squallz_core::publish_file_no_replace(repaired_archive, output)
}

#[derive(Clone, Copy)]
struct ReportScope<'a> {
    operation: &'static str,
    archive: &'a Path,
    recovery: &'a Path,
    source_file_count: usize,
}

impl<'a> ReportScope<'a> {
    fn new(
        operation: &'static str,
        archive: &'a Path,
        recovery: &'a Path,
        source_file_count: usize,
    ) -> Self {
        Self {
            operation,
            archive,
            recovery,
            source_file_count,
        }
    }
}

fn report(
    scope: ReportScope<'_>,
    output_path: Option<&Path>,
    tool: &Path,
    redundancy: Option<u8>,
    output: &Output,
) -> RecoveryReport {
    RecoveryReport {
        ok: output.status.success(),
        operation: scope.operation,
        archive: scope.archive.to_path_buf(),
        recovery: scope.recovery.to_path_buf(),
        outputs: Vec::new(),
        output: output_path.map(Path::to_path_buf),
        tool: tool.to_path_buf(),
        redundancy_percent: redundancy,
        source_file_count: scope.source_file_count,
        status_code: output.status.code(),
        metrics: None,
        stdout: String::from_utf8_lossy(&output.stdout).trim().to_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
    }
}

fn external_recovery_report(
    scope: ReportScope<'_>,
    output_path: Option<&Path>,
    tool: &Path,
    redundancy: Option<u8>,
    output: &Output,
) -> RecoveryReport {
    let mut report = report(scope, output_path, tool, redundancy, output);
    report.metrics = parse_par2cmdline_metrics(&report.stdout);
    report
}

fn parse_par2cmdline_metrics(stdout: &str) -> Option<RecoveryMetrics> {
    let mut all_correct = false;
    let mut repair_required = false;
    let mut repair_possible = None;
    let mut data_blocks = None;
    let mut reported_recovery_blocks = None;
    let mut reported_missing_recovery_blocks = None;
    let mut loaded_recovery_blocks = None;

    for line in stdout.split(['\r', '\n']).map(str::trim) {
        match line {
            "All files are correct, repair is not required." => all_correct = true,
            "Repair is required." => repair_required = true,
            "Repair is possible." => record_consistent(&mut repair_possible, true)?,
            "Repair is not possible." => record_consistent(&mut repair_possible, false)?,
            _ => {
                if let Some(counts) = parse_available_data_blocks(line) {
                    record_consistent(&mut data_blocks, counts)?;
                }
                if let Some(count) = parse_loaded_recovery_blocks(line) {
                    loaded_recovery_blocks =
                        Some(loaded_recovery_blocks.unwrap_or(0u32).checked_add(count)?);
                }
                if let Some(count) =
                    parse_summary_count(line, "You have ", " recovery blocks available.").or_else(
                        || parse_summary_count(line, "You have ", " recovery block available."),
                    )
                {
                    record_consistent(&mut reported_recovery_blocks, count)?;
                }
                if let Some(count) = parse_summary_count(
                    line,
                    "You need ",
                    " more recovery blocks to be able to repair.",
                )
                .or_else(|| {
                    parse_summary_count(
                        line,
                        "You need ",
                        " more recovery block to be able to repair.",
                    )
                }) {
                    record_consistent(&mut reported_missing_recovery_blocks, count)?;
                }
            }
        }
    }

    if all_correct {
        if repair_required
            || repair_possible.is_some()
            || data_blocks.is_some()
            || reported_recovery_blocks.is_some()
            || reported_missing_recovery_blocks.is_some()
        {
            return None;
        }
        let recovery_blocks_available = loaded_recovery_blocks?;
        return Some(RecoveryMetrics {
            all_correct: true,
            repair_possible: true,
            blocks_needed: 0,
            recovery_blocks_available,
            blocks_repaired: None,
            files_repaired: None,
            no_damage: true,
        });
    }

    if !repair_required {
        return None;
    }
    let repair_possible = repair_possible?;
    let (available_data_blocks, total_data_blocks) = data_blocks?;
    let blocks_needed = total_data_blocks.checked_sub(available_data_blocks)?;
    if blocks_needed == 0 {
        return None;
    }
    let recovery_blocks_available = match (reported_recovery_blocks, repair_possible) {
        (Some(available), _) => available,
        (None, false) => blocks_needed.checked_sub(reported_missing_recovery_blocks?)?,
        (None, true) => return None,
    };
    if loaded_recovery_blocks.is_some_and(|loaded| loaded != recovery_blocks_available) {
        return None;
    }
    if repair_possible != (recovery_blocks_available >= blocks_needed) {
        return None;
    }
    if repair_possible {
        if reported_missing_recovery_blocks.is_some() {
            return None;
        }
    } else {
        let missing = blocks_needed.checked_sub(recovery_blocks_available)?;
        if reported_missing_recovery_blocks != Some(missing) {
            return None;
        }
    }

    Some(RecoveryMetrics {
        all_correct: false,
        repair_possible,
        blocks_needed,
        recovery_blocks_available,
        blocks_repaired: None,
        files_repaired: None,
        no_damage: false,
    })
}

fn record_consistent<T: Copy + Eq>(slot: &mut Option<T>, value: T) -> Option<()> {
    match slot {
        Some(existing) if *existing != value => None,
        Some(_) => Some(()),
        None => {
            *slot = Some(value);
            Some(())
        }
    }
}

fn parse_available_data_blocks(line: &str) -> Option<(u32, u32)> {
    let counts = line
        .strip_prefix("You have ")?
        .strip_suffix(" data blocks available.")?;
    let (available, total) = counts.split_once(" out of ")?;
    Some((available.parse().ok()?, total.parse().ok()?))
}

fn parse_loaded_recovery_blocks(line: &str) -> Option<u32> {
    let (_, recovery) = line.split_once(" including ")?;
    recovery.strip_suffix(" recovery blocks")?.parse().ok()
}

fn parse_summary_count(line: &str, prefix: &str, suffix: &str) -> Option<u32> {
    line.strip_prefix(prefix)?
        .strip_suffix(suffix)?
        .parse()
        .ok()
}

fn rust_report(
    scope: ReportScope<'_>,
    ok: bool,
    metrics: RecoveryMetrics,
    stdout: String,
    stderr: String,
) -> RecoveryReport {
    RecoveryReport {
        ok,
        operation: scope.operation,
        archive: scope.archive.to_path_buf(),
        recovery: scope.recovery.to_path_buf(),
        outputs: Vec::new(),
        output: None,
        tool: PathBuf::from(RUST_PAR2_TOOL),
        redundancy_percent: None,
        source_file_count: scope.source_file_count,
        status_code: None,
        metrics: Some(metrics),
        stdout,
        stderr,
    }
}

fn verify_with_rust_par2(
    archive: &Path,
    recovery: &Path,
    set: &rust_par2::Par2FileSet,
) -> Result<RecoveryReport, FormatError> {
    let dir = parent_dir(recovery);
    let result = rust_par2::verify(set, dir);
    let ok = result.all_correct();
    let stdout = format_verify_result(&result);
    let metrics = metrics_from_verify(&result);
    let stderr = if ok {
        String::new()
    } else {
        "PAR2 verify found damaged or missing files".to_owned()
    };
    Ok(rust_report(
        ReportScope::new("verify", archive, recovery, set.files.len()),
        ok,
        metrics,
        stdout,
        stderr,
    ))
}

fn repair_with_rust_par2(
    archive: &Path,
    report_recovery: &Path,
    work_recovery: &Path,
    set: &rust_par2::Par2FileSet,
) -> Result<RecoveryReport, FormatError> {
    let dir = parent_dir(work_recovery);
    let verify = rust_par2::verify(set, dir);
    if verify.all_correct() {
        return Ok(rust_report(
            ReportScope::new("repair", archive, report_recovery, set.files.len()),
            true,
            repair_metrics(&verify, None, None, true),
            format!("{}\nno_damage=true", format_verify_result(&verify)),
            String::new(),
        ));
    }

    match rust_par2::repair_from_verify(set, dir, &verify) {
        Ok(result) if result.success => Ok(rust_report(
            ReportScope::new("repair", archive, report_recovery, set.files.len()),
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
            ReportScope::new("repair", archive, report_recovery, set.files.len()),
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
            ReportScope::new("repair", archive, report_recovery, set.files.len()),
            true,
            repair_metrics(&verify, None, None, true),
            format!("{}\nno_damage=true", format_verify_result(&verify)),
            String::new(),
        )),
        Err(err) => Ok(rust_report(
            ReportScope::new("repair", archive, report_recovery, set.files.len()),
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

fn parse_par2_for_operation(
    archive: &Path,
    recovery: &Path,
) -> Result<rust_par2::Par2FileSet, FormatError> {
    let set = rust_par2::parse(recovery)
        .map_err(|error| FormatError::CorruptArchive(format!("cannot parse PAR2 data: {error}")))?;
    validate_par2_file_names(&set)?;
    validate_archive_membership(archive, recovery, &set)?;
    validate_par2_targets(&set, parent_dir(recovery))?;
    Ok(set)
}

fn validate_par2_file_names(set: &rust_par2::Par2FileSet) -> Result<(), FormatError> {
    if set.files.is_empty() {
        return Err(FormatError::CorruptArchive(
            "PAR2 data does not describe any files".into(),
        ));
    }
    let mut names = HashSet::with_capacity(set.files.len());
    for file in set.files.values() {
        let path = Path::new(&file.filename);
        let mut components = path.components();
        let has_component = components.next().is_some();
        let is_relative_path = has_component
            && path
                .components()
                .all(|component| matches!(component, Component::Normal(name) if !name.is_empty()));
        let has_foreign_separator = cfg!(not(windows)) && file.filename.contains('\\');
        if !is_relative_path || has_foreign_separator {
            return Err(FormatError::UnsafeFileName(
                "PAR2 data contains an unsafe file name".into(),
            ));
        }
        if !names.insert(path.to_path_buf()) {
            return Err(FormatError::CorruptArchive(
                "PAR2 data describes the same file name more than once".into(),
            ));
        }
    }
    for name in &names {
        let mut ancestor = name.parent();
        while let Some(path) = ancestor.filter(|path| !path.as_os_str().is_empty()) {
            if names.contains(path) {
                return Err(FormatError::CorruptArchive(
                    "PAR2 data contains conflicting file and directory paths".into(),
                ));
            }
            ancestor = path.parent();
        }
    }
    Ok(())
}

fn validate_archive_membership(
    archive: &Path,
    recovery: &Path,
    set: &rust_par2::Par2FileSet,
) -> Result<(), FormatError> {
    let archive_identity = fs::canonicalize(archive).ok();
    let recovery_dir = fs::canonicalize(parent_dir(recovery)).map_err(FormatError::from)?;
    let matches = archive_identity.as_ref().is_some_and(|identity| {
        set.files.values().any(|file| {
            fs::canonicalize(recovery_dir.join(&file.filename))
                .is_ok_and(|candidate| candidate == *identity)
        })
    });
    if matches {
        return Ok(());
    }
    Err(FormatError::Unsupported(
        "the selected PAR2 data does not describe the selected archive".into(),
    ))
}

fn validate_par2_targets(set: &rust_par2::Par2FileSet, dir: &Path) -> Result<(), FormatError> {
    let root = fs::canonicalize(dir).map_err(FormatError::from)?;
    for file in set.files.values() {
        let mut target = root.clone();
        for component in Path::new(&file.filename).components() {
            let Component::Normal(name) = component else {
                return Err(FormatError::UnsafeFileName(
                    "PAR2 data contains an unsafe file name".into(),
                ));
            };
            target.push(name);
            match fs::symlink_metadata(&target) {
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    return Err(FormatError::SymlinkBreakout(
                        "PAR2 data cannot read or repair symbolic-link targets".into(),
                    ));
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
                Err(error) => return Err(FormatError::from(error)),
            }
        }
    }
    Ok(())
}

fn validate_protect_layout(recovery: &Path, sources: &[PathBuf]) -> Result<(), FormatError> {
    let recovery_dir = fs::canonicalize(parent_dir(recovery)).map_err(FormatError::from)?;
    for source in sources {
        if fs::symlink_metadata(source)
            .map_err(FormatError::from)?
            .file_type()
            .is_symlink()
        {
            return Err(FormatError::SymlinkBreakout(
                "PAR2 data cannot protect symbolic-link sources".into(),
            ));
        }
        let source = fs::canonicalize(source).map_err(FormatError::from)?;
        let relative = source.strip_prefix(&recovery_dir).map_err(|_| {
            FormatError::Unsupported(
                "PAR2 recovery data must be stored in the source folder or an ancestor".into(),
            )
        })?;
        if relative.as_os_str().is_empty() {
            return Err(FormatError::Unsupported(
                "PAR2 protection requires a source file".into(),
            ));
        }
    }
    Ok(())
}

fn validate_generated_par2_set(
    work_dir: &Path,
    work_recovery: &Path,
    recovery: &Path,
    sources: &[PathBuf],
) -> Result<Vec<PathBuf>, FormatError> {
    let recovery_name = recovery.file_name().ok_or_else(|| {
        FormatError::Unsupported(format!(
            "PAR2 recovery path must name a file: {}",
            recovery.display()
        ))
    })?;
    let recovery_name_text = recovery_name.to_str().ok_or_else(|| {
        FormatError::Unsupported("PAR2 recovery file name must be valid Unicode".into())
    })?;
    let recovery_stem = strip_par2_extension(recovery_name_text).ok_or_else(|| {
        FormatError::Unsupported("PAR2 recovery file name must end with .par2".into())
    })?;

    let mut generated = Vec::new();
    let mut volume_count = 0usize;
    for entry in fs::read_dir(work_dir).map_err(FormatError::from)? {
        let entry = entry.map_err(FormatError::from)?;
        if generated.len() >= MAX_GENERATED_PAR2_FILES {
            return Err(FormatError::ResourceLimitExceeded(format!(
                "PAR2 backend produced more than {MAX_GENERATED_PAR2_FILES} files"
            )));
        }
        let metadata = fs::symlink_metadata(entry.path()).map_err(FormatError::from)?;
        if !metadata.file_type().is_file() {
            return Err(FormatError::CorruptArchive(
                "PAR2 backend produced a non-regular output".into(),
            ));
        }
        let name = entry.file_name();
        if name != recovery_name {
            let name = name.to_str().ok_or_else(|| {
                FormatError::CorruptArchive(
                    "PAR2 backend produced a non-Unicode output name".into(),
                )
            })?;
            if !is_par2_volume_name(recovery_stem, name) {
                return Err(FormatError::CorruptArchive(
                    "PAR2 backend produced an unexpected output file".into(),
                ));
            }
            volume_count += 1;
        }
        generated.push(entry.path());
    }
    if !generated.iter().any(|path| path == work_recovery) {
        return Err(FormatError::CorruptArchive(
            "PAR2 backend did not produce the requested index file".into(),
        ));
    }
    if volume_count == 0 {
        return Err(FormatError::CorruptArchive(
            "PAR2 backend did not produce any recovery volume".into(),
        ));
    }

    let index_set = rust_par2::parse(work_recovery)
        .map_err(|error| FormatError::CorruptArchive(format!("cannot parse PAR2 data: {error}")))?;
    validate_par2_file_names(&index_set)?;
    validate_generated_source_set(&index_set, recovery, sources)?;
    let mut has_recovery_blocks = false;
    for path in &generated {
        let set = rust_par2::parse(path).map_err(|error| {
            FormatError::CorruptArchive(format!("cannot parse PAR2 output: {error}"))
        })?;
        validate_par2_file_names(&set)?;
        if set.recovery_set_id != index_set.recovery_set_id {
            return Err(FormatError::CorruptArchive(
                "PAR2 backend produced files from different recovery sets".into(),
            ));
        }
        if path != work_recovery && set.recovery_block_count > 0 {
            has_recovery_blocks = true;
        }
    }
    if !has_recovery_blocks {
        return Err(FormatError::CorruptArchive(
            "PAR2 backend did not produce usable recovery blocks".into(),
        ));
    }
    generated.sort();
    Ok(generated)
}

fn strip_par2_extension(name: &str) -> Option<&str> {
    let split = name.len().checked_sub(5)?;
    name.get(split..)
        .is_some_and(|suffix| suffix.eq_ignore_ascii_case(".par2"))
        .then(|| &name[..split])
}

fn is_par2_volume_name(stem: &str, candidate: &str) -> bool {
    let prefix = format!("{stem}.vol");
    let Some(remainder) = candidate.strip_prefix(&prefix) else {
        return false;
    };
    let Some(number) = strip_par2_extension(remainder) else {
        return false;
    };
    let Some((first, count)) = number.split_once('+') else {
        return false;
    };
    !first.is_empty()
        && !count.is_empty()
        && first.bytes().all(|byte| byte.is_ascii_digit())
        && count.bytes().all(|byte| byte.is_ascii_digit())
}

fn validate_generated_source_set(
    set: &rust_par2::Par2FileSet,
    recovery: &Path,
    sources: &[PathBuf],
) -> Result<(), FormatError> {
    if set.files.len() != sources.len() {
        return Err(FormatError::CorruptArchive(
            "PAR2 backend protected a different number of source files".into(),
        ));
    }
    let recovery_dir = fs::canonicalize(parent_dir(recovery)).map_err(FormatError::from)?;
    let expected = sources
        .iter()
        .map(|source| fs::canonicalize(source).map_err(FormatError::from))
        .collect::<Result<HashSet<_>, FormatError>>()?;
    let actual = set
        .files
        .values()
        .map(|file| fs::canonicalize(recovery_dir.join(&file.filename)).map_err(FormatError::from))
        .collect::<Result<HashSet<_>, FormatError>>()?;
    if actual != expected {
        return Err(FormatError::CorruptArchive(
            "PAR2 backend protected a different source set".into(),
        ));
    }
    validate_par2_targets(set, &recovery_dir)
}

fn validate_single_file_output(
    archive: &Path,
    set: &rust_par2::Par2FileSet,
) -> Result<(), FormatError> {
    if set.files.len() != 1 {
        return Err(FormatError::Unsupported(
            "PAR2 safe-copy repair requires a single-file recovery set".into(),
        ));
    }
    let name = file_name_or_archive(archive);
    let has_split_suffix = name.rsplit_once('.').is_some_and(|(_, suffix)| {
        suffix.len() >= 3 && suffix.chars().all(|ch| ch.is_ascii_digit())
    });
    if has_split_suffix {
        return Err(FormatError::Unsupported(
            "PAR2 safe-copy repair does not support split-volume sources".into(),
        ));
    }
    Ok(())
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

fn run_tool_controlled(
    tool: &Path,
    args: &[OsString],
    archive: &Path,
    progress: &dyn ProgressSink,
    control: &ControlToken,
    initial_phase: ProgressPhase,
    cancellable: bool,
) -> Result<Output, FormatError> {
    control.checkpoint()?;
    let mut child = Command::new(tool)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(FormatError::from)?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| FormatError::Io(io::Error::other("PAR2 backend stdout was not captured")))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| FormatError::Io(io::Error::other("PAR2 backend stderr was not captured")))?;
    let current = recovery_progress_entry(archive);

    thread::scope(|scope| {
        let stdout_reader = scope.spawn(move || {
            capture_tool_stdout(stdout, progress, current, initial_phase, cancellable)
        });
        let stderr_reader = scope.spawn(move || capture_tool_stream(stderr));
        let status = loop {
            if cancellable && control.is_cancelled() {
                let _ = child.kill();
                let _ = child.wait();
                break Err(FormatError::Cancelled);
            }
            match child.try_wait() {
                Ok(Some(status)) => break Ok(status),
                Ok(None) => thread::sleep(TOOL_POLL_INTERVAL),
                Err(error) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break Err(FormatError::from(error));
                }
            }
        };
        let stdout = stdout_reader.join();
        let stderr = stderr_reader.join();
        let status = status?;
        let stdout = stdout
            .map_err(|_| {
                FormatError::Io(io::Error::other(
                    "PAR2 backend stdout reader terminated unexpectedly",
                ))
            })?
            .map_err(FormatError::from)?;
        let stderr = stderr
            .map_err(|_| {
                FormatError::Io(io::Error::other(
                    "PAR2 backend stderr reader terminated unexpectedly",
                ))
            })?
            .map_err(FormatError::from)?;
        Ok(Output {
            status,
            stdout,
            stderr,
        })
    })
}

fn capture_tool_stdout(
    mut stream: impl Read,
    progress: &dyn ProgressSink,
    current: EntryPath,
    initial_phase: ProgressPhase,
    interruptible: bool,
) -> io::Result<Vec<u8>> {
    let mut output = Vec::new();
    let mut buffer = [0u8; 4096];
    let mut parser = ToolProgressParser::new(progress, current, interruptible, Some(initial_phase));
    loop {
        let read = stream.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        output.extend_from_slice(&buffer[..read]);
        parser.push(&buffer[..read]);
    }
    parser.finish();
    Ok(output)
}

fn capture_tool_stream(mut stream: impl Read) -> io::Result<Vec<u8>> {
    let mut output = Vec::new();
    stream.read_to_end(&mut output)?;
    Ok(output)
}

struct ToolProgressParser<'a> {
    progress: &'a dyn ProgressSink,
    current: EntryPath,
    interruptible: bool,
    phase: Option<ProgressPhase>,
    record: Vec<u8>,
}

impl<'a> ToolProgressParser<'a> {
    fn new(
        progress: &'a dyn ProgressSink,
        current: EntryPath,
        interruptible: bool,
        phase: Option<ProgressPhase>,
    ) -> Self {
        Self {
            progress,
            current,
            interruptible,
            phase,
            record: Vec::with_capacity(128),
        }
    }

    fn push(&mut self, bytes: &[u8]) {
        for byte in bytes {
            if matches!(byte, b'\r' | b'\n') {
                self.flush_record();
            } else if self.record.len() < TOOL_PROGRESS_RECORD_BYTES {
                self.record.push(*byte);
            }
        }
    }

    fn finish(&mut self) {
        self.flush_record();
    }

    fn flush_record(&mut self) {
        if self.record.is_empty() {
            return;
        }
        let record = String::from_utf8_lossy(&self.record).trim().to_owned();
        if !record.is_empty() {
            self.report_record(&record);
        }
        self.record.clear();
    }

    fn report_record(&mut self, record: &str) {
        let terminal = matches!(record, "Done" | "Done." | "Repair complete.");
        let phase = if terminal {
            Some(ProgressPhase::RecoveryFinalize)
        } else if record.starts_with("Loading:") || record.starts_with("Constructing:") {
            Some(ProgressPhase::RecoveryPrepare)
        } else if record.starts_with("Verifying source files:")
            || record.starts_with("Verifying repaired files:")
            || record.starts_with("Scanning:")
        {
            Some(ProgressPhase::RecoveryVerify)
        } else if record.starts_with("Processing:") || record.starts_with("Repairing:") {
            Some(ProgressPhase::RecoveryProcess)
        } else if record.starts_with("Writing recovery packets")
            || record.starts_with("Writing verification packets")
            || record.starts_with("Writing recovered data")
        {
            Some(ProgressPhase::RecoveryFinalize)
        } else {
            None
        };
        let Some(phase) = phase else {
            return;
        };
        if self
            .phase
            .is_some_and(|current| recovery_phase_rank(phase) < recovery_phase_rank(current))
        {
            return;
        }
        let phase_changed = self.phase != Some(phase);
        if phase_changed {
            self.phase = Some(phase);
            self.progress.on_phase(phase, self.interruptible);
        }
        if terminal {
            self.progress
                .on_progress(TOOL_PROGRESS_TOTAL, TOOL_PROGRESS_TOTAL, &self.current);
        } else if let Some(done) = parse_percent_tenths(record) {
            self.progress
                .on_progress(done, TOOL_PROGRESS_TOTAL, &self.current);
        } else if phase_changed {
            self.progress.on_progress(0, 0, &self.current);
        }
    }
}

fn recovery_phase_rank(phase: ProgressPhase) -> u8 {
    match phase {
        ProgressPhase::RecoveryPrepare => 0,
        ProgressPhase::RecoveryVerify => 1,
        ProgressPhase::RecoveryProcess => 2,
        ProgressPhase::RecoveryFinalize => 3,
        _ => 0,
    }
}

fn parse_percent_tenths(record: &str) -> Option<u64> {
    let prefix = record.get(..record.find('%')?)?;
    let token = prefix
        .rsplit(|character: char| !(character.is_ascii_digit() || character == '.'))
        .find(|part| !part.is_empty())?;
    let (whole, fraction) = match token.split_once('.') {
        Some((whole, fraction)) => (whole, fraction),
        None => (token, ""),
    };
    let whole = whole.parse::<u64>().ok()?;
    let fraction = fraction
        .as_bytes()
        .first()
        .filter(|byte| byte.is_ascii_digit())
        .map_or(0, |byte| u64::from(*byte - b'0'));
    whole
        .checked_mul(10)
        .and_then(|value| value.checked_add(fraction))
        .map(|value| value.min(TOOL_PROGRESS_TOTAL))
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

fn ensure_output_available(path: &Path) -> Result<(), FormatError> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(FormatError::from(error)),
        Ok(_) => Err(FormatError::output_exists(path)),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};
    #[cfg(unix)]
    use std::time::Instant;

    use super::*;
    use base64::Engine as _;

    static TOOL_ENV_LOCK: Mutex<()> = Mutex::new(());

    #[derive(Default)]
    struct RecordedProgress {
        phases: Mutex<Vec<(ProgressPhase, bool)>>,
        values: Mutex<Vec<(u64, u64, String)>>,
    }

    impl RecordedProgress {
        #[cfg(unix)]
        fn has_phase(&self, phase: ProgressPhase) -> bool {
            self.phases
                .lock()
                .expect("recorded phase lock")
                .iter()
                .any(|(recorded, _)| *recorded == phase)
        }
    }

    impl ProgressSink for RecordedProgress {
        fn on_progress(&self, done: u64, total: u64, current: &EntryPath) {
            self.values.lock().expect("recorded progress lock").push((
                done,
                total,
                current.display.clone(),
            ));
        }

        fn on_phase(&self, phase: ProgressPhase, interruptible: bool) {
            self.phases
                .lock()
                .expect("recorded phase lock")
                .push((phase, interruptible));
        }
    }

    struct CancelAfterProgress {
        control: Arc<ControlToken>,
    }

    impl ProgressSink for CancelAfterProgress {
        fn on_progress(&self, _done: u64, _total: u64, _current: &EntryPath) {
            self.control.cancel();
        }
    }

    struct EnvRestore {
        key: &'static str,
        old: Option<OsString>,
    }

    impl EnvRestore {
        fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
            let old = env::var_os(key);
            env::set_var(key, value);
            Self { key, old }
        }

        fn remove(key: &'static str) -> Self {
            let old = env::var_os(key);
            env::remove_var(key);
            Self { key, old }
        }
    }

    impl Drop for EnvRestore {
        fn drop(&mut self) {
            match &self.old {
                Some(value) => env::set_var(self.key, value),
                None => env::remove_var(self.key),
            }
        }
    }

    fn par2_set_with_filename(filename: &str) -> rust_par2::Par2FileSet {
        let id = [1; 16];
        let file = rust_par2::Par2File {
            file_id: id,
            hash: [0; 16],
            hash_16k: [0; 16],
            size: 0,
            filename: filename.to_owned(),
            slices: Vec::new(),
        };
        rust_par2::Par2FileSet {
            recovery_set_id: [0; 16],
            slice_size: 4,
            files: HashMap::from([(id, file)]),
            recovery_block_count: 0,
            creator: None,
        }
    }

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
    fn external_tool_progress_parser_reports_real_stage_percentages() {
        let progress = RecordedProgress::default();
        let mut parser =
            ToolProgressParser::new(&progress, EntryPath::from_utf8("archive.zip"), true, None);
        parser.push(b"Load");
        parser.push(b"ing: 12.3%\rVerifying source files:\nLoading: 14.0%\rScann");
        parser.push(b"ing: 50.0%\rProcessing: 99.9%\rWriting recovered data\rRepair complete.\n");
        parser.finish();

        assert_eq!(
            *progress.phases.lock().expect("recorded phase lock"),
            vec![
                (ProgressPhase::RecoveryPrepare, true),
                (ProgressPhase::RecoveryVerify, true),
                (ProgressPhase::RecoveryProcess, true),
                (ProgressPhase::RecoveryFinalize, true),
            ]
        );
        assert_eq!(
            *progress.values.lock().expect("recorded progress lock"),
            vec![
                (123, 1000, "archive.zip".to_owned()),
                (0, 0, "archive.zip".to_owned()),
                (500, 1000, "archive.zip".to_owned()),
                (999, 1000, "archive.zip".to_owned()),
                (0, 0, "archive.zip".to_owned()),
                (1000, 1000, "archive.zip".to_owned()),
            ]
        );
    }

    #[test]
    fn external_tool_progress_parser_does_not_regress_the_operation_phase() {
        let progress = RecordedProgress::default();
        let mut parser = ToolProgressParser::new(
            &progress,
            EntryPath::from_utf8("archive.zip"),
            true,
            Some(ProgressPhase::RecoveryVerify),
        );
        parser.push(b"Loading: 14.0%\rVerifying source files:\rScanning: 37.5%\r");
        parser.finish();

        assert!(progress
            .phases
            .lock()
            .expect("recorded phase lock")
            .is_empty());
        assert_eq!(
            *progress.values.lock().expect("recorded progress lock"),
            vec![(375, 1000, "archive.zip".to_owned())]
        );
    }

    #[test]
    fn recovery_copy_cancellation_removes_the_partial_private_file(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let root = std::env::temp_dir().join(format!(
            "squallz-par2-copy-cancel-{}-{}",
            std::process::id(),
            unique_nonce()
        ));
        fs::create_dir_all(&root)?;
        let source = root.join("archive.zip");
        let destination = root.join("archive.work.zip");
        fs::write(&source, vec![0x5a; COPY_BUFFER_BYTES * 3])?;
        let control = ControlToken::new();
        let progress = CancelAfterProgress {
            control: Arc::clone(&control),
        };
        let mut copy_progress = RecoveryCopyProgress::new(&progress, &control);

        let result = copy_regular_file_controlled(
            &source,
            &destination,
            "PAR2 source file",
            &mut copy_progress,
        );

        assert!(matches!(result, Err(FormatError::Cancelled)));
        assert!(!destination.exists());
        assert_eq!(fs::metadata(&source)?.len(), (COPY_BUFFER_BYTES * 3) as u64);
        fs::remove_dir_all(root)?;
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn external_tool_cancellation_terminates_the_child_process(
    ) -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::PermissionsExt;

        let _guard = TOOL_ENV_LOCK.lock()?;
        let root = std::env::temp_dir().join(format!(
            "squallz-par2-tool-cancel-{}-{}",
            std::process::id(),
            unique_nonce()
        ));
        fs::create_dir_all(&root)?;
        let tool = root.join("par2-fixture");
        fs::write(
            &tool,
            b"#!/bin/sh\nprintf 'Processing: 1.0%%\\r'\nexec sleep 30\n",
        )?;
        fs::set_permissions(&tool, fs::Permissions::from_mode(0o755))?;
        let progress = Arc::new(RecordedProgress::default());
        let control = ControlToken::new();
        let worker_progress = Arc::clone(&progress);
        let worker_control = Arc::clone(&control);
        let worker_tool = tool.clone();
        let worker = thread::spawn(move || {
            run_tool_controlled(
                &worker_tool,
                &[],
                Path::new("archive.zip"),
                worker_progress.as_ref(),
                worker_control.as_ref(),
                ProgressPhase::RecoveryVerify,
                true,
            )
        });
        let wait_started = Instant::now();
        while !progress.has_phase(ProgressPhase::RecoveryProcess)
            && wait_started.elapsed() < Duration::from_secs(2)
        {
            thread::sleep(Duration::from_millis(10));
        }
        assert!(progress.has_phase(ProgressPhase::RecoveryProcess));

        let cancel_started = Instant::now();
        control.cancel();
        let result = worker.join().expect("PAR2 worker thread");

        assert!(matches!(result, Err(FormatError::Cancelled)));
        assert!(cancel_started.elapsed() < Duration::from_secs(5));
        fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn protect_rejects_redundancy_outside_the_supported_range() {
        for redundancy in [0, 101] {
            assert!(matches!(
                protect_files(
                    Path::new("archive.zip"),
                    redundancy,
                    None,
                    &[PathBuf::from("archive.zip")],
                ),
                Err(FormatError::Unsupported(message))
                    if message == "PAR2 redundancy must be a whole percentage from 1 to 100"
            ));
        }
    }

    #[test]
    fn explicit_repair_output_rejects_the_source_path() {
        let archive = Path::new("archive.zip");
        assert!(matches!(
            repair(archive, Some(archive), None),
            Err(FormatError::Unsupported(message))
                if message == "PAR2 repair output must differ from the source archive"
        ));
    }

    #[test]
    fn explicit_repair_output_rejects_an_existing_entry() -> Result<(), Box<dyn std::error::Error>>
    {
        let root = std::env::temp_dir().join(format!(
            "squallz-par2-existing-output-{}-{}",
            std::process::id(),
            unique_nonce()
        ));
        fs::create_dir_all(&root)?;
        let archive = root.join("archive.zip");
        let output = root.join("archive.repaired.zip");
        fs::write(&archive, b"source archive")?;
        fs::write(&output, b"existing output")?;

        let error = repair(&archive, Some(&output), None)
            .expect_err("existing output must be rejected before repair starts");
        assert!(error.is_output_exists());
        assert_eq!(error.output_exists_path(), Some(output.as_path()));
        assert_eq!(fs::read(&archive)?, b"source archive");
        assert_eq!(fs::read(&output)?, b"existing output");
        assert!(fs::read_dir(&root)?.all(|entry| {
            entry
                .map(|entry| {
                    !entry
                        .file_name()
                        .to_string_lossy()
                        .contains(".sqz-par2-repair-")
                })
                .unwrap_or(false)
        }));
        fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn directory_repair_restores_a_complete_multi_file_set_without_touching_sources(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let _guard = TOOL_ENV_LOCK.lock()?;
        let root = std::env::temp_dir().join(format!(
            "squallz-par2-directory-repair-{}-{}",
            std::process::id(),
            unique_nonce()
        ));
        let empty_path = root.join("empty-path");
        fs::create_dir_all(&empty_path)?;
        let _configured_tool = EnvRestore::remove(TOOL_ENV);
        let _path = EnvRestore::set("PATH", &empty_path);

        let first = root.join("set.zip.001");
        let second = root.join("set.zip.002");
        let recovery = root.join("set.zip.par2");
        let recovery_volume = root.join("set.zip.vol0+4.par2");
        let output = root.join("repaired-set");
        let damaged_first = b"damaged";
        fs::write(&first, damaged_first)?;
        fs::write(
            &recovery,
            base64::engine::general_purpose::STANDARD
                .decode(include_str!("../tests/fixtures/multi-set.zip.par2.b64").trim())?,
        )?;
        fs::write(
            &recovery_volume,
            base64::engine::general_purpose::STANDARD
                .decode(include_str!("../tests/fixtures/multi-set.zip.vol0+4.par2.b64").trim())?,
        )?;

        let report = repair_to_directory(&first, &output, Some(&recovery))?;
        assert!(report.ok, "{report:?}");
        assert_eq!(report.output.as_deref(), Some(output.as_path()));
        assert_eq!(report.source_file_count, 2);
        assert_eq!(report.tool, PathBuf::from(RUST_PAR2_TOOL));
        assert_eq!(
            fs::read(output.join("set.zip.001"))?,
            b"first-volume-original\n"
        );
        assert_eq!(
            fs::read(output.join("set.zip.002"))?,
            b"second-volume-original\n"
        );
        assert_eq!(fs::read(&first)?, damaged_first);
        assert!(!second.exists());
        assert!(fs::read_dir(&output)?.all(|entry| {
            entry
                .map(|entry| {
                    !entry
                        .file_name()
                        .to_string_lossy()
                        .to_ascii_lowercase()
                        .ends_with(".par2")
                })
                .unwrap_or(false)
        }));
        assert!(fs::read_dir(&root)?.all(|entry| {
            entry
                .map(|entry| {
                    !entry
                        .file_name()
                        .to_string_lossy()
                        .contains(".sqz-par2-repair-")
                })
                .unwrap_or(false)
        }));

        fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn directory_repair_rejects_an_existing_output_directory(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let root = std::env::temp_dir().join(format!(
            "squallz-par2-existing-directory-{}-{}",
            std::process::id(),
            unique_nonce()
        ));
        let archive = root.join("archive.zip");
        let output = root.join("repaired");
        fs::create_dir_all(&output)?;
        fs::write(&archive, b"source archive")?;

        let error = repair_to_directory(&archive, &output, None)
            .expect_err("an existing output directory must be preserved");
        assert!(error.is_output_exists());
        assert_eq!(error.output_exists_path(), Some(output.as_path()));
        assert_eq!(fs::read(&archive)?, b"source archive");
        assert!(output.is_dir());

        fs::remove_dir_all(root)?;
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn source_set_copy_rejects_a_symlinked_member_parent() -> Result<(), Box<dyn std::error::Error>>
    {
        use std::os::unix::fs::symlink;

        let root = std::env::temp_dir().join(format!(
            "squallz-par2-source-symlink-{}-{}",
            std::process::id(),
            unique_nonce()
        ));
        let source_dir = root.join("source");
        let outside_dir = root.join("outside");
        let work_dir = root.join("work");
        fs::create_dir_all(&source_dir)?;
        fs::create_dir_all(&outside_dir)?;
        fs::create_dir_all(&work_dir)?;
        let archive = source_dir.join("archive.zip");
        fs::write(&archive, b"archive")?;
        fs::write(outside_dir.join("secret.bin"), b"outside")?;
        symlink(&outside_dir, source_dir.join("linked"))?;

        let mut set = par2_set_with_filename("archive.zip");
        let second_id = [2; 16];
        set.files.insert(
            second_id,
            rust_par2::Par2File {
                file_id: second_id,
                hash: [0; 16],
                hash_16k: [0; 16],
                size: 7,
                filename: "linked/secret.bin".to_owned(),
                slices: Vec::new(),
            },
        );

        let error = copy_source_set(
            &archive,
            &source_dir.join("archive.zip.par2"),
            &work_dir,
            &set,
        )
        .expect_err("a PAR2 source member must not cross a symbolic-link parent");

        assert!(matches!(error, FormatError::SymlinkBreakout(_)));
        assert!(!work_dir.join("linked/secret.bin").exists());
        assert_eq!(fs::read(outside_dir.join("secret.bin"))?, b"outside");
        fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn safe_copy_staging_failure_removes_the_private_work_directory(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let root = std::env::temp_dir().join(format!(
            "squallz-par2-staging-failure-{}-{}",
            std::process::id(),
            unique_nonce()
        ));
        fs::create_dir_all(&root)?;
        let missing_archive = root.join("missing.zip");
        let recovery = root.join("missing.zip.par2");
        let output = root.join("missing.repaired.zip");
        fs::write(&recovery, b"unused recovery data")?;

        let error = repair_to_output(
            &missing_archive,
            &output,
            &recovery,
            &par2_set_with_filename("missing.zip"),
        )
        .expect_err("copying a missing archive must fail");
        assert!(matches!(
            error,
            FormatError::Io(ref error) if error.kind() == io::ErrorKind::NotFound
        ));
        assert!(!output.exists());
        assert!(fs::read_dir(&root)?.all(|entry| {
            entry
                .map(|entry| {
                    !entry
                        .file_name()
                        .to_string_lossy()
                        .contains(".sqz-par2-repair-")
                })
                .unwrap_or(false)
        }));
        fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn repair_cleanup_failure_reports_a_ready_output_and_exact_workspace() {
        let output = PathBuf::from("/tmp/archive.repaired.zip");
        let work_dir = PathBuf::from("/tmp/.squallz-par2-repair-a1-7-11.work");
        let journal = PathBuf::from("/tmp/.squallz-par2-repair-a1.json");
        let error = recovery_workspace_error(
            &output,
            WorkspaceDebt {
                journal: journal.clone(),
                workspace: Some(work_dir.clone()),
                reason: "injected cleanup denial".to_owned(),
            },
            true,
            None,
        );
        let details = recovery_cleanup_details(&error).expect("structured cleanup details");

        assert_eq!(details.target, output);
        assert_eq!(details.workspace, Some(work_dir));
        assert_eq!(details.journal, journal);
        assert!(details.output_ready);
        assert!(error.to_string().contains("repaired copy is ready"));
        assert!(error.to_string().contains("automatic recovery record"));
    }

    #[test]
    fn repair_cleanup_failure_preserves_an_unconfirmed_operation_in_the_detail() {
        let output = PathBuf::from("/tmp/archive.repaired.zip");
        let work_dir = PathBuf::from("/tmp/.squallz-par2-repair-a1-7-12.work");
        let journal = PathBuf::from("/tmp/.squallz-par2-repair-a1.json");
        let original = FormatError::Unsupported("injected repair failure".into());

        let error = recovery_workspace_error(
            &output,
            WorkspaceDebt {
                journal: journal.clone(),
                workspace: Some(work_dir.clone()),
                reason: "injected cleanup denial".to_owned(),
            },
            false,
            Some(&original),
        );
        let details = recovery_cleanup_details(&error).expect("structured cleanup details");

        assert_eq!(details.target, output);
        assert_eq!(details.workspace, Some(work_dir));
        assert_eq!(details.journal, journal);
        assert!(!details.output_ready);
        assert!(error.to_string().contains("injected repair failure"));
        assert!(error.to_string().contains("injected cleanup denial"));
    }

    #[test]
    fn damaged_repair_record_reports_no_unbound_workspace() {
        let output = PathBuf::from("/tmp/archive.repaired.zip");
        let journal = PathBuf::from("/tmp/.squallz-par2-repair-a1.json");
        let error = recovery_workspace_error(
            &output,
            WorkspaceDebt {
                journal: journal.clone(),
                workspace: None,
                reason: "injected damaged record".to_owned(),
            },
            false,
            None,
        );
        let details = recovery_cleanup_details(&error).expect("structured cleanup details");

        assert_eq!(details.workspace, None);
        assert_eq!(details.journal, journal);
        assert!(error.to_string().contains("no unbound path was removed"));
    }

    #[test]
    fn safe_copy_publish_preserves_a_late_output_conflict() -> Result<(), Box<dyn std::error::Error>>
    {
        let root = std::env::temp_dir().join(format!(
            "squallz-par2-late-output-{}-{}",
            std::process::id(),
            unique_nonce()
        ));
        fs::create_dir_all(&root)?;
        let work = root.join("work/nested");
        fs::create_dir_all(&work)?;
        let staged = work.join("archive.work.zip");
        let output = root.join("archive.repaired.zip");
        fs::write(&staged, b"repaired output")?;
        fs::write(&output, b"late output")?;

        let error = persist_repaired_output(&staged, &output)
            .expect_err("a late output conflict must not be replaced");
        assert!(error.is_output_exists());
        assert_eq!(error.output_exists_path(), Some(output.as_path()));
        assert_eq!(fs::read(&output)?, b"late output");
        assert_eq!(fs::read(&staged)?, b"repaired output");
        fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn safe_copy_publish_moves_the_repaired_output() -> Result<(), Box<dyn std::error::Error>> {
        let root = std::env::temp_dir().join(format!(
            "squallz-par2-publish-output-{}-{}",
            std::process::id(),
            unique_nonce()
        ));
        fs::create_dir_all(&root)?;
        let work = root.join("work/nested");
        fs::create_dir_all(&work)?;
        let staged = work.join("archive.work.zip");
        let output = root.join("archive.repaired.zip");
        fs::write(&staged, b"repaired output")?;

        persist_repaired_output(&staged, &output)?;
        assert_eq!(fs::read(&output)?, b"repaired output");
        assert!(!staged.exists());
        fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn par2_file_names_are_confined_to_the_sidecar_directory() {
        assert!(validate_par2_file_names(&par2_set_with_filename("archive.zip")).is_ok());
        assert!(validate_par2_file_names(&par2_set_with_filename("nested/archive.zip")).is_ok());

        for filename in [
            "",
            ".",
            "..",
            "../outside.zip",
            "/tmp/outside.zip",
            "nested/../outside.zip",
            "..\\outside.zip",
            "C:\\outside.zip",
        ] {
            assert!(matches!(
                validate_par2_file_names(&par2_set_with_filename(filename)),
                Err(FormatError::UnsafeFileName(message))
                    if message == "PAR2 data contains an unsafe file name"
            ));
        }
    }

    #[test]
    fn par2_file_names_reject_duplicate_and_conflicting_member_trees() {
        let mut duplicate = par2_set_with_filename("archive.zip");
        let second_id = [2; 16];
        duplicate.files.insert(
            second_id,
            rust_par2::Par2File {
                file_id: second_id,
                hash: [0; 16],
                hash_16k: [0; 16],
                size: 0,
                filename: "archive.zip".to_owned(),
                slices: Vec::new(),
            },
        );
        assert!(matches!(
            validate_par2_file_names(&duplicate),
            Err(FormatError::CorruptArchive(message))
                if message == "PAR2 data describes the same file name more than once"
        ));

        let mut conflict = par2_set_with_filename("nested");
        conflict.files.insert(
            second_id,
            rust_par2::Par2File {
                file_id: second_id,
                hash: [0; 16],
                hash_16k: [0; 16],
                size: 0,
                filename: "nested/archive.zip".to_owned(),
                slices: Vec::new(),
            },
        );
        assert!(matches!(
            validate_par2_file_names(&conflict),
            Err(FormatError::CorruptArchive(message))
                if message == "PAR2 data contains conflicting file and directory paths"
        ));
    }

    #[test]
    fn par2_archive_must_belong_to_the_selected_set() {
        let set = par2_set_with_filename("other.zip");
        assert!(matches!(
            validate_archive_membership(
                Path::new("archive.zip"),
                Path::new("archive.zip.par2"),
                &set,
            ),
            Err(FormatError::Unsupported(message))
                if message == "the selected PAR2 data does not describe the selected archive"
        ));
    }

    #[test]
    fn single_file_safe_copy_rejects_split_and_multi_file_sets() {
        let split = par2_set_with_filename("archive.zip.001");
        assert!(matches!(
            validate_single_file_output(Path::new("archive.zip.001"), &split),
            Err(FormatError::Unsupported(message))
                if message == "PAR2 safe-copy repair does not support split-volume sources"
        ));

        let mut multiple = par2_set_with_filename("archive.zip.001");
        let second_id = [2; 16];
        multiple.files.insert(
            second_id,
            rust_par2::Par2File {
                file_id: second_id,
                hash: [0; 16],
                hash_16k: [0; 16],
                size: 0,
                filename: "archive.zip.002".to_owned(),
                slices: Vec::new(),
            },
        );
        assert!(matches!(
            validate_single_file_output(Path::new("archive.zip.001"), &multiple),
            Err(FormatError::Unsupported(message))
                if message == "PAR2 safe-copy repair requires a single-file recovery set"
        ));
    }

    #[test]
    fn safe_copy_preserves_the_par2_member_path() -> Result<(), Box<dyn std::error::Error>> {
        let root = std::env::temp_dir().join(format!(
            "squallz-par2-copy-{}-{}",
            std::process::id(),
            unique_nonce()
        ));
        let work = root.join("work");
        fs::create_dir_all(&work)?;
        fs::create_dir_all(root.join("nested"))?;
        let source = root.join("nested/archive.zip");
        fs::write(&source, b"archive")?;

        let copied = copy_source_set(
            &source,
            &root.join("archive.par2"),
            &work,
            &par2_set_with_filename("nested/archive.zip"),
        )?;
        assert_eq!(copied, work.join("nested/archive.zip"));
        assert_eq!(fs::read(copied)?, b"archive");
        fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn protect_layout_allows_nested_sources_without_parent_traversal(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let root = std::env::temp_dir().join(format!(
            "squallz-par2-layout-{}-{}",
            std::process::id(),
            unique_nonce()
        ));
        let nested = root.join("nested");
        let sibling = root.with_extension("sibling");
        fs::create_dir_all(&nested)?;
        fs::create_dir_all(&sibling)?;
        let nested_source = nested.join("archive.zip");
        let sibling_source = sibling.join("archive.zip");
        fs::write(&nested_source, b"nested")?;
        fs::write(&sibling_source, b"sibling")?;

        assert!(validate_protect_layout(&root.join("archive.par2"), &[nested_source]).is_ok());
        assert!(matches!(
            validate_protect_layout(&root.join("archive.par2"), &[sibling_source]),
            Err(FormatError::Unsupported(message))
                if message == "PAR2 recovery data must be stored in the source folder or an ancestor"
        ));
        fs::remove_dir_all(root)?;
        fs::remove_dir_all(sibling)?;
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn par2_targets_reject_symbolic_links() -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::symlink;

        let root = std::env::temp_dir().join(format!(
            "squallz-par2-symlink-{}-{}",
            std::process::id(),
            unique_nonce()
        ));
        let sidecar_dir = root.join("sidecar");
        fs::create_dir_all(&sidecar_dir)?;
        let outside = root.join("outside.zip");
        fs::write(&outside, b"unchanged")?;
        symlink(&outside, sidecar_dir.join("archive.zip"))?;

        let result = validate_par2_targets(&par2_set_with_filename("archive.zip"), &sidecar_dir);
        assert!(matches!(
            result,
            Err(FormatError::SymlinkBreakout(message))
                if message == "PAR2 data cannot read or repair symbolic-link targets"
        ));
        assert_eq!(fs::read(&outside)?, b"unchanged");
        fs::remove_dir_all(root)?;
        Ok(())
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

    #[test]
    fn par2cmdline_metrics_report_no_damage() {
        let stdout = "\
Loaded 4 new packets including 4 recovery blocks
Loaded 8 new packets including 8 recovery blocks
Loading: 99.9%\rAll files are correct, repair is not required.
";

        assert_eq!(
            parse_par2cmdline_metrics(stdout),
            Some(RecoveryMetrics {
                all_correct: true,
                repair_possible: true,
                blocks_needed: 0,
                recovery_blocks_available: 12,
                blocks_repaired: None,
                files_repaired: None,
                no_damage: true,
            })
        );
    }

    #[test]
    fn par2cmdline_metrics_report_repair_capacity() {
        let repairable = "\
Repair is required.
1 file(s) exist but are damaged.
You have 1961 out of 1967 data blocks available.
You have 197 recovery blocks available.
Repair is possible.
6 recovery blocks will be used to repair.
";
        assert_eq!(
            parse_par2cmdline_metrics(repairable),
            Some(RecoveryMetrics {
                all_correct: false,
                repair_possible: true,
                blocks_needed: 6,
                recovery_blocks_available: 197,
                blocks_repaired: None,
                files_repaired: None,
                no_damage: false,
            })
        );

        let over_capacity = "\
Repair is required.
You have 1961 out of 1967 data blocks available.
Repair is not possible.
You need 6 more recovery blocks to be able to repair.
";
        assert_eq!(
            parse_par2cmdline_metrics(over_capacity),
            Some(RecoveryMetrics {
                all_correct: false,
                repair_possible: false,
                blocks_needed: 6,
                recovery_blocks_available: 0,
                blocks_repaired: None,
                files_repaired: None,
                no_damage: false,
            })
        );
    }

    #[test]
    fn par2cmdline_metrics_reject_incomplete_or_conflicting_summaries() {
        assert_eq!(
            parse_par2cmdline_metrics("Repair is required.\nRepair is possible.\n"),
            None
        );
        assert_eq!(
            parse_par2cmdline_metrics(
                "\
Repair is required.
You have 3 out of 8 data blocks available.
Loaded 5 new packets including 5 recovery blocks
You have 4 recovery blocks available.
Repair is possible.
"
            ),
            None
        );
        assert_eq!(
            parse_par2cmdline_metrics(
                "\
Repair is required.
You have 3 out of 8 data blocks available.
Loaded 1 new packets including 1 recovery blocks
Repair is not possible.
You need 2 more recovery blocks to be able to repair.
"
            ),
            None
        );
        assert_eq!(
            parse_par2cmdline_metrics(
                "\
All files are correct, repair is not required.
Repair is required.
"
            ),
            None
        );
        assert_eq!(
            parse_par2cmdline_metrics(
                "\
Repair is required.
You have 3 out of 8 data blocks available.
You have 4 out of 8 data blocks available.
You have 5 recovery blocks available.
Repair is possible.
"
            ),
            None
        );
        assert_eq!(
            parse_par2cmdline_metrics(
                "\
Repair is required.
You have 3 out of 8 data blocks available.
You have 5 recovery blocks available.
Repair is possible.
Repair is not possible.
"
            ),
            None
        );
    }
}
