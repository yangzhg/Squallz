//! `sqz batch`: run a JSON script of archive operations.
//!
//! The runner calls the shared engine directly instead of shelling out to
//! `sqz`, so batch automation stays on the same core path as the rest of the
//! CLI and GUI.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::{json, Value};
use squallz_core::api::{
    split_volume_name, CompressionLevel, CreateOptions, Detected, EntryPath, ExtractOptions,
    FormatError, NoProgress, OpenOptions, OverwritePolicy, Password, SplitOutputMode,
    SqzCreateOptions, SymlinkPolicy, TestSummary, UpdateOp,
};
use squallz_core::{ChecksumAlgorithm, CreateContentPolicy, PathFilter};

use crate::args::{resource_options, safety_limits};
use crate::commands::reports::{
    create_report_json, empty_extract_counts_json, extract_counts_json, extract_plan_json,
    print_preserved_output_warning, print_pretty_json, recovery_summary_json, test_report_json,
    test_report_json_with_structure,
};
use crate::errors::{error_kind, exit_code, localize_error, CliError};

use super::Ctx;

#[derive(Debug, Deserialize)]
struct BatchScript {
    #[serde(default)]
    base_dir: Option<PathBuf>,
    #[serde(default)]
    jobs: Vec<BatchJob>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum BatchSplitMode {
    Generic,
    Native,
}

impl From<BatchSplitMode> for SplitOutputMode {
    fn from(value: BatchSplitMode) -> Self {
        match value {
            BatchSplitMode::Generic => Self::Generic,
            BatchSplitMode::Native => Self::Native,
        }
    }
}

#[derive(Debug, Deserialize)]
struct BatchJob {
    #[serde(default)]
    id: Option<String>,
    #[serde(alias = "op", alias = "type", alias = "kind")]
    operation: String,
    #[serde(default)]
    archive: Option<PathBuf>,
    #[serde(default)]
    src: Option<PathBuf>,
    #[serde(default)]
    dest: Option<PathBuf>,
    #[serde(default)]
    inputs: Vec<PathBuf>,
    #[serde(default)]
    output: Option<PathBuf>,
    #[serde(default)]
    format: Option<String>,
    #[serde(default)]
    level: Option<u8>,
    #[serde(default)]
    profile: Option<String>,
    #[serde(default)]
    password: Option<String>,
    #[serde(default)]
    encoding: Option<String>,
    #[serde(default)]
    includes: Vec<String>,
    #[serde(default)]
    excludes: Vec<String>,
    #[serde(default)]
    content_policy: Option<CreateContentPolicy>,
    #[serde(default)]
    algorithm: Option<String>,
    #[serde(default, alias = "manifest")]
    check: Option<PathBuf>,
    #[serde(default, alias = "minimum_size")]
    min_size: Option<u64>,
    #[serde(default)]
    fail_on_found: bool,
    #[serde(default)]
    output_dir: Option<PathBuf>,
    #[serde(default)]
    threads: Option<usize>,
    #[serde(default)]
    memory_limit: Option<u64>,
    #[serde(default)]
    max_output_bytes: Option<u64>,
    #[serde(default)]
    max_entries: Option<u64>,
    #[serde(default)]
    max_compression_ratio: Option<u32>,
    #[serde(default)]
    smart: bool,
    #[serde(default)]
    best_effort: bool,
    #[serde(default)]
    overwrite: Option<String>,
    #[serde(default)]
    symlinks: Option<String>,
    #[serde(default)]
    split: Option<u64>,
    #[serde(default)]
    split_mode: Option<BatchSplitMode>,
    #[serde(default)]
    test_after_create: bool,
    #[serde(default)]
    encrypt_names: bool,
    #[serde(default)]
    out_password: Option<String>,
    #[serde(default)]
    inner_format: Option<String>,
    #[serde(default)]
    recovery: Option<BatchRecovery>,
    #[serde(default, alias = "recovery_file", alias = "par2")]
    recovery_path: Option<PathBuf>,
    #[serde(default)]
    redundancy: Option<u8>,
    #[serde(default)]
    tolerate_loss: Option<u32>,
    #[serde(default)]
    add: Vec<PathBuf>,
    #[serde(default)]
    mkdir: Vec<String>,
    #[serde(default)]
    delete: Vec<String>,
    #[serde(default)]
    rename: Vec<BatchMove>,
    #[serde(default, rename = "move", alias = "moves")]
    move_entries: Vec<BatchMove>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum BatchRecovery {
    Percent(u8),
    Text(String),
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum BatchMove {
    Object { from: String, to: String },
    Pair([String; 2]),
    Spec(String),
}

#[derive(Debug)]
struct BatchJobReport {
    id: String,
    kind: String,
    ok: bool,
    detail: String,
    result: Option<Value>,
    exit_code: i32,
    error_kind: Option<&'static str>,
}

struct JobSuccess {
    detail: String,
    result: Value,
}

pub fn run(
    ctx: &Ctx,
    script: PathBuf,
    keep_going: bool,
    json_output: bool,
) -> Result<(), CliError> {
    let script_text = fs::read_to_string(&script).map_err(FormatError::from)?;
    let parsed: BatchScript = serde_json::from_str(&script_text)
        .map_err(|e| FormatError::Unsupported(format!("batch script is not valid JSON: {e}")))?;
    if parsed.jobs.is_empty() {
        return Err(FormatError::Unsupported("batch script has no jobs".into()).into());
    }

    let script_dir = script_parent_or_current(&script);
    let base_dir = script_base_dir(script_dir, parsed.base_dir.as_deref());

    let mut reports = Vec::with_capacity(parsed.jobs.len());
    for (index, job) in parsed.jobs.iter().enumerate() {
        let id = job_id_or_default(job.id.as_deref(), index);
        let kind = normalize_operation(&job.operation);
        let report = match run_job(ctx, &base_dir, job, kind) {
            Ok(success) => BatchJobReport {
                id,
                kind: kind.to_owned(),
                ok: true,
                detail: success.detail,
                result: Some(success.result),
                exit_code: 0,
                error_kind: None,
            },
            Err(error) => BatchJobReport {
                id,
                kind: kind.to_owned(),
                ok: false,
                detail: localize_error(&ctx.loc, &error),
                result: None,
                exit_code: exit_code(&error),
                error_kind: Some(error_kind(&error)),
            },
        };
        let failed = !report.ok;
        reports.push(report);
        if failed && !keep_going {
            break;
        }
    }

    let failed = reports.iter().filter(|report| !report.ok).count();
    if json_output {
        print_json_report(&script, &base_dir, keep_going, &reports, failed)?;
    } else {
        print_human_report(ctx, &script, keep_going, &reports, failed);
    }

    if failed == 0 {
        Ok(())
    } else {
        let code = first_failed_exit_code(&reports);
        Err(CliError::Exit(code))
    }
}

fn script_parent_or_current(script: &Path) -> &Path {
    match script.parent().filter(|path| !path.as_os_str().is_empty()) {
        Some(parent) => parent,
        None => Path::new("."),
    }
}

fn script_base_dir(script_dir: &Path, base_dir: Option<&Path>) -> PathBuf {
    match base_dir {
        Some(path) => resolve_path(script_dir, path),
        None => script_dir.to_path_buf(),
    }
}

fn job_id_or_default(id: Option<&str>, index: usize) -> String {
    match id {
        Some(id) => id.to_owned(),
        None => format!("job-{}", index + 1),
    }
}

fn first_failed_exit_code(reports: &[BatchJobReport]) -> i32 {
    for report in reports {
        if !report.ok {
            return report.exit_code;
        }
    }
    1
}

fn run_job(
    ctx: &Ctx,
    base_dir: &Path,
    job: &BatchJob,
    operation: &str,
) -> Result<JobSuccess, FormatError> {
    match operation {
        "estimate" => run_estimate_job(ctx, base_dir, job),
        "test" => run_test_job(ctx, base_dir, job),
        "extract" => run_extract_job(ctx, base_dir, job),
        "compress" => run_compress_job(ctx, base_dir, job),
        "checksum" => run_checksum_job(ctx, base_dir, job),
        "checksum_check" => run_checksum_check_job(ctx, base_dir, job),
        "duplicates" => run_duplicates_job(ctx, base_dir, job),
        "convert" => run_convert_job(ctx, base_dir, job),
        "pack" => run_pack_job(ctx, base_dir, job),
        "export" => run_export_job(ctx, base_dir, job),
        "repair_sqz" => run_repair_sqz_job(ctx, base_dir, job),
        "repair_zip" => run_repair_zip_job(ctx, base_dir, job),
        "protect" => run_protect_job(ctx, base_dir, job),
        "verify_recovery" => run_verify_recovery_job(ctx, base_dir, job),
        "repair_recovery" => run_repair_recovery_job(ctx, base_dir, job),
        "update" => run_update_job(ctx, base_dir, job),
        other => Err(FormatError::Unsupported(format!(
            "unsupported batch operation: {other}"
        ))),
    }
}

fn run_estimate_job(ctx: &Ctx, base_dir: &Path, job: &BatchJob) -> Result<JobSuccess, FormatError> {
    let inputs = resolve_inputs(base_dir, &job.inputs)?;
    let excludes =
        crate::content_policy::resolve_create_excludes(job.content_policy, job.excludes.clone());
    let estimate = ctx.engine.estimate_create_inputs(&inputs, &excludes)?;
    let mut result = json!({
        "operation": "estimate",
        "input_count": estimate.input_count,
        "entries": estimate.entries,
        "files": estimate.files,
        "directories": estimate.directories,
        "symlinks": estimate.symlinks,
        "total_bytes": estimate.total_bytes,
        "output_budget_bytes": estimate.output_budget_bytes(),
    });
    if let Some(output) = job.output.as_deref() {
        result["output"] = json!(resolve_path(base_dir, output).display().to_string());
    }
    Ok(JobSuccess {
        detail: format!("estimated {} entries", estimate.entries),
        result,
    })
}

fn run_test_job(ctx: &Ctx, base_dir: &Path, job: &BatchJob) -> Result<JobSuccess, FormatError> {
    let archive = required_path(base_dir, job.archive.as_deref(), "archive")?;
    let report = ctx
        .engine
        .test_summary(&archive, &open_options(job), &NoProgress, &ctx.ctl)?;
    if report.is_ok() {
        Ok(JobSuccess {
            detail: format!(
                "{} entries tested in {}",
                report.entries_tested,
                archive.display()
            ),
            result: json!({
                "operation": "test",
                "ok": true,
                "archive": archive.display().to_string(),
                "entries_tested": report.entries_tested,
                "problems": [],
                "problems_total": 0,
                "problems_truncated": false,
            }),
        })
    } else {
        Err(test_report_error(report))
    }
}

fn run_extract_job(ctx: &Ctx, base_dir: &Path, job: &BatchJob) -> Result<JobSuccess, FormatError> {
    let archive = required_path(base_dir, job.archive.as_deref(), "archive")?;
    let dest = job_dest_or_base(base_dir, job.dest.as_deref());
    let open = open_options(job);
    let filter = PathFilter::new(&job.includes)?;
    let opts = ExtractOptions {
        overwrite: parse_overwrite(job.overwrite.as_deref())?,
        symlinks: parse_symlinks(job.symlinks.as_deref())?,
        limits: safety_limits(
            job.max_output_bytes,
            job.max_entries,
            job.max_compression_ratio,
        ),
        resources: resource_options(job.threads, job.memory_limit),
        best_effort: job.best_effort,
        ..ExtractOptions::default()
    };
    let (plan, report) = ctx.engine.plan_and_extract_with_report_controlled(
        &archive,
        &dest,
        &archive,
        job.smart,
        &open,
        &opts,
        &NoProgress,
        &ctx.ctl,
        |entries, control| filter.select_entries(entries, control),
        |_| Ok(()),
    )?;
    if !filter.is_empty() && plan.scope.entries == 0 {
        return Ok(JobSuccess {
            detail: format!("no entries matched in {}", archive.display()),
            result: json!({
                "operation": "extract",
                "archive": archive.display().to_string(),
                "dest": plan.requested_destination.display().to_string(),
                "matched": false,
                "best_effort": job.best_effort,
                "plan": extract_plan_json(&plan),
                "counts": empty_extract_counts_json(&plan.destination),
                "selected_entries": 0,
                "directories": 0,
                "output_bytes": 0,
            }),
        });
    }
    Ok(JobSuccess {
        detail: format!(
            "extracted {} to {}",
            archive.display(),
            report.destination.display()
        ),
        result: json!({
            "operation": "extract",
            "archive": archive.display().to_string(),
            "dest": report.destination.display().to_string(),
            "matched": true,
            "best_effort": job.best_effort,
            "plan": extract_plan_json(&plan),
            "counts": extract_counts_json(&report),
            "selected_entries": report.selected_entries,
            "directories": report.directories,
            "output_bytes": report.output_bytes,
        }),
    })
}

fn job_dest_or_base(base_dir: &Path, dest: Option<&Path>) -> PathBuf {
    match dest {
        Some(path) => resolve_path(base_dir, path),
        None => base_dir.to_path_buf(),
    }
}

fn run_compress_job(ctx: &Ctx, base_dir: &Path, job: &BatchJob) -> Result<JobSuccess, FormatError> {
    let inputs = resolve_inputs(base_dir, &job.inputs)?;
    let output = required_path(base_dir, job.output.as_deref(), "output")?;
    validate_requested_format(ctx, &output, job.format.as_deref())?;
    let level = job_level(job)?;
    let excludes =
        crate::content_policy::resolve_create_excludes(job.content_policy, job.excludes.clone());
    let opts = CreateOptions {
        level: CompressionLevel::from_numeric(level),
        password: job.password.clone().map(Password::new),
        encrypt_filenames: job.encrypt_names,
        split_size: job.split,
        split_mode: job
            .split_mode
            .map_or(SplitOutputMode::Generic, SplitOutputMode::from),
        excludes,
        resources: resource_options(job.threads, job.memory_limit),
        ..CreateOptions::default()
    };
    let kind = if job.split.is_some() {
        squallz_core::CreateArtifactKind::SplitArchive
    } else {
        squallz_core::CreateArtifactKind::Archive
    };
    let policy = super::create_commit_policy(&output, kind, true, &NoProgress, &ctx.ctl)?;
    let report = ctx.engine.create_with_report_policy(
        &output,
        &inputs,
        &opts,
        policy,
        &NoProgress,
        &ctx.ctl,
    )?;
    let entries_tested_after_create = if job.test_after_create {
        let test_report = ctx.engine.test_summary(
            &report.primary_output,
            &OpenOptions {
                password: opts.password.clone(),
                encoding_override: None,
            },
            &NoProgress,
            &ctx.ctl,
        )?;
        if !test_report.is_ok() {
            return Err(test_report_error(test_report));
        }
        Some(test_report.entries_tested)
    } else {
        None
    };
    let detail = format!("created {}", report.primary_output.display());
    let mut result = create_report_json(&report);
    result["output"] = json!(output.display().to_string());
    result["operation"] = json!("compress");
    result["level"] = json!(level);
    result["tested_after_create"] = json!(entries_tested_after_create.is_some());
    result["entries_tested_after_create"] = json!(entries_tested_after_create);
    Ok(JobSuccess { detail, result })
}

fn run_checksum_job(ctx: &Ctx, base_dir: &Path, job: &BatchJob) -> Result<JobSuccess, FormatError> {
    if job.check.is_some() {
        return run_checksum_check_job(ctx, base_dir, job);
    }
    let inputs = resolve_inputs(base_dir, &job.inputs)?;
    let algorithm = parse_checksum_algorithm(job.algorithm.as_deref())?;
    let report = ctx.engine.checksum_files_with_progress(
        &inputs,
        &job.excludes,
        algorithm,
        &NoProgress,
        &ctx.ctl,
    )?;
    Ok(JobSuccess {
        detail: format!(
            "hashed {} files with {}",
            report.files_hashed,
            report.algorithm.id()
        ),
        result: checksum_report_json(&report),
    })
}

fn run_checksum_check_job(
    ctx: &Ctx,
    base_dir: &Path,
    job: &BatchJob,
) -> Result<JobSuccess, FormatError> {
    let manifest = required_path(base_dir, job.check.as_deref(), "check")?;
    let algorithm = parse_checksum_algorithm(job.algorithm.as_deref())?;
    let report = ctx.engine.verify_checksum_manifest_with_progress(
        &manifest,
        algorithm,
        &NoProgress,
        &ctx.ctl,
    )?;
    if !report.is_ok() {
        return Err(FormatError::CorruptArchive(format!(
            "checksum verification failed: {} of {} entries did not match",
            report.failed, report.checked
        )));
    }
    Ok(JobSuccess {
        detail: format!(
            "verified {} checksums with {}",
            report.checked,
            report.algorithm.id()
        ),
        result: checksum_check_report_json(&report),
    })
}

fn run_duplicates_job(
    ctx: &Ctx,
    base_dir: &Path,
    job: &BatchJob,
) -> Result<JobSuccess, FormatError> {
    let inputs = resolve_inputs(base_dir, &job.inputs)?;
    let min_size = job_min_size(job);
    let report = ctx
        .engine
        .find_duplicate_files(&inputs, &job.excludes, min_size)?;
    if job.fail_on_found && !report.groups.is_empty() {
        return Err(FormatError::CorruptArchive(format!(
            "duplicate scan found {} duplicate groups",
            report.duplicate_groups()
        )));
    }
    Ok(JobSuccess {
        detail: format!(
            "found {} duplicate groups across {} files",
            report.duplicate_groups(),
            report.files_scanned
        ),
        result: duplicate_report_json(&report, min_size),
    })
}

fn job_min_size(job: &BatchJob) -> u64 {
    job.min_size.map_or(1, |size| size)
}

fn run_convert_job(ctx: &Ctx, base_dir: &Path, job: &BatchJob) -> Result<JobSuccess, FormatError> {
    let src = required_path(
        base_dir,
        job.src.as_deref().or(job.archive.as_deref()),
        "src",
    )?;
    let output = required_path(base_dir, job.output.as_deref(), "output")?;
    let level = job_level(job)?;
    let open = open_options(job);
    let create = CreateOptions {
        level: CompressionLevel::from_numeric(level),
        password: job
            .out_password
            .clone()
            .or_else(|| job.password.clone())
            .map(Password::new),
        encrypt_filenames: job.encrypt_names,
        split_size: job.split,
        split_mode: job
            .split_mode
            .map_or(SplitOutputMode::Generic, SplitOutputMode::from),
        resources: resource_options(job.threads, job.memory_limit),
        ..CreateOptions::default()
    };
    let allow_existing = matches!(
        parse_overwrite(job.overwrite.as_deref())?,
        OverwritePolicy::Overwrite
    );
    let kind = if job.split.is_some() {
        squallz_core::CreateArtifactKind::SplitArchive
    } else {
        squallz_core::CreateArtifactKind::Archive
    };
    let commit_policy =
        super::create_commit_policy(&output, kind, allow_existing, &NoProgress, &ctx.ctl)?;
    let report = ctx.engine.convert_with_report_policy(
        &src,
        &output,
        &open,
        &create,
        commit_policy,
        &NoProgress,
        &ctx.ctl,
    )?;
    let mut result = create_report_json(&report);
    result["operation"] = json!("convert");
    result["source"] = json!(src.display().to_string());
    result["output"] = json!(output.display().to_string());
    result["level"] = json!(level);
    Ok(JobSuccess {
        detail: format!("converted {} to {}", src.display(), output.display()),
        result,
    })
}

fn run_pack_job(ctx: &Ctx, base_dir: &Path, job: &BatchJob) -> Result<JobSuccess, FormatError> {
    let inputs = resolve_inputs(base_dir, &job.inputs)?;
    let output = required_path(base_dir, job.output.as_deref(), "output")?;
    let level = job_level(job)?;
    let inner_format = job_inner_format(job);
    let recovery_percent = job_recovery_percent(job, 25)?;
    let excludes =
        crate::content_policy::resolve_create_excludes(job.content_policy, job.excludes.clone());
    let opts = CreateOptions {
        level: CompressionLevel::from_numeric(level),
        split_size: job.split,
        split_mode: job
            .split_mode
            .map_or(SplitOutputMode::Generic, SplitOutputMode::from),
        excludes,
        resources: resource_options(job.threads, job.memory_limit),
        sqz: SqzCreateOptions {
            inner_format: inner_format.clone(),
            recovery_percent,
        },
        ..CreateOptions::default()
    };
    let kind = if job.split.is_some() {
        squallz_core::CreateArtifactKind::SplitArchive
    } else {
        squallz_core::CreateArtifactKind::Archive
    };
    let policy = super::create_commit_policy(&output, kind, true, &NoProgress, &ctx.ctl)?;
    let report = ctx.engine.create_with_report_policy(
        &output,
        &inputs,
        &opts,
        policy,
        &NoProgress,
        &ctx.ctl,
    )?;
    let detail = format!("packed {}", report.primary_output.display());
    let mut result = create_report_json(&report);
    result["output"] = json!(output.display().to_string());
    result["operation"] = json!("pack");
    result["level"] = json!(level);
    result["inner_format"] = json!(inner_format);
    result["recovery_percent"] = json!(recovery_percent);
    Ok(JobSuccess { detail, result })
}

fn job_inner_format(job: &BatchJob) -> String {
    match job.inner_format.as_deref() {
        Some(format) => format.to_owned(),
        None => "sqz".to_owned(),
    }
}

fn run_export_job(ctx: &Ctx, base_dir: &Path, job: &BatchJob) -> Result<JobSuccess, FormatError> {
    let archive = required_path(
        base_dir,
        job.archive.as_deref().or(job.src.as_deref()),
        "archive",
    )?;
    let output = required_path(
        base_dir,
        job.output.as_deref().or(job.dest.as_deref()),
        "output",
    )?;
    if !is_sqz_source_path(&archive) {
        return Err(FormatError::Unsupported(
            "batch export expects a .sqz source container".into(),
        ));
    }
    if is_sqz_source_path(&output) {
        return Err(FormatError::Unsupported(
            "batch export output must be a standard archive, not .sqz".into(),
        ));
    }
    let level = job_level(job)?;
    let create = CreateOptions {
        level: CompressionLevel::from_numeric(level),
        password: job.out_password.clone().map(Password::new),
        resources: resource_options(job.threads, job.memory_limit),
        ..CreateOptions::default()
    };
    let allow_existing = matches!(
        parse_overwrite(job.overwrite.as_deref())?,
        OverwritePolicy::Overwrite
    );
    let commit_policy = super::create_commit_policy(
        &output,
        squallz_core::CreateArtifactKind::Archive,
        allow_existing,
        &NoProgress,
        &ctx.ctl,
    )?;
    ctx.engine.convert_with_policy(
        &archive,
        &output,
        &OpenOptions::default(),
        &create,
        commit_policy,
        &NoProgress,
        &ctx.ctl,
    )?;
    Ok(JobSuccess {
        detail: format!("exported {} to {}", archive.display(), output.display()),
        result: json!({
            "operation": "export",
            "archive": archive.display().to_string(),
            "output": output.display().to_string(),
            "level": level,
        }),
    })
}

fn run_repair_sqz_job(
    ctx: &Ctx,
    base_dir: &Path,
    job: &BatchJob,
) -> Result<JobSuccess, FormatError> {
    let archive = required_path(
        base_dir,
        job.archive.as_deref().or(job.src.as_deref()),
        "archive",
    )?;
    let output = required_path(
        base_dir,
        job.output.as_deref().or(job.dest.as_deref()),
        "output",
    )?;
    if !is_sqz_source_path(&archive) {
        return Err(FormatError::Unsupported(
            "batch repair_sqz expects a .sqz source container".into(),
        ));
    }
    if !is_plain_sqz_path(&output) {
        return Err(FormatError::Unsupported(
            "batch repair_sqz output must be a .sqz container".into(),
        ));
    }
    let source_report =
        ctx.engine
            .test_summary(&archive, &OpenOptions::default(), &NoProgress, &ctx.ctl)?;
    if !source_report.is_ok() {
        return Err(test_report_error(source_report));
    }
    let level = job_level(job)?;
    let create = CreateOptions {
        level: CompressionLevel::from_numeric(level),
        resources: resource_options(job.threads, job.memory_limit),
        ..CreateOptions::default()
    };
    let in_place = ctx.engine.convert_with_atomic_replace(
        &archive,
        &output,
        &OpenOptions::default(),
        &create,
        &NoProgress,
        &ctx.ctl,
    )?;
    Ok(JobSuccess {
        detail: format!("repaired {} to {}", archive.display(), output.display()),
        result: json!({
            "operation": "repair_sqz",
            "archive": archive.display().to_string(),
            "output": output.display().to_string(),
            "tool": "sqz-embedded-recovery",
            "in_place": in_place,
            "source": test_report_json(&source_report),
            "recovery": source_report.recovery.as_ref().map(recovery_summary_json),
            "level": level,
        }),
    })
}

fn run_repair_zip_job(
    ctx: &Ctx,
    base_dir: &Path,
    job: &BatchJob,
) -> Result<JobSuccess, FormatError> {
    let archive = required_path(
        base_dir,
        job.archive.as_deref().or(job.src.as_deref()),
        "archive",
    )?;
    let output = required_path(
        base_dir,
        job.output.as_deref().or(job.dest.as_deref()),
        "output",
    )?;
    if !is_plain_zip_path(&archive) {
        return Err(FormatError::Unsupported(
            "batch repair_zip expects a ZIP-family source archive".into(),
        ));
    }
    if !is_plain_zip_path(&output) {
        return Err(FormatError::Unsupported(
            "batch repair_zip output must be a ZIP-family archive".into(),
        ));
    }
    let source_test = ctx.engine.test_summary_with_structure(
        &archive,
        &OpenOptions::default(),
        &NoProgress,
        &ctx.ctl,
    )?;
    if !source_test.payload_is_ok() {
        return Err(test_report_error(source_test.into_summary()));
    }
    let structure = source_test.structure;
    let source_report = source_test.into_summary();
    let level = job_level(job)?;
    let create = CreateOptions {
        level: CompressionLevel::from_numeric(level),
        resources: resource_options(job.threads, job.memory_limit),
        ..CreateOptions::default()
    };
    let in_place = ctx.engine.convert_with_atomic_replace(
        &archive,
        &output,
        &OpenOptions::default(),
        &create,
        &NoProgress,
        &ctx.ctl,
    )?;
    Ok(JobSuccess {
        detail: format!("rebuilt ZIP index into {}", output.display()),
        result: json!({
            "operation": "repair_zip",
            "archive": archive.display().to_string(),
            "output": output.display().to_string(),
            "tool": "zip-local-header-rebuild",
            "in_place": in_place,
            "source": test_report_json_with_structure(&source_report, structure),
            "level": level,
        }),
    })
}

fn run_protect_job(ctx: &Ctx, base_dir: &Path, job: &BatchJob) -> Result<JobSuccess, FormatError> {
    let archive = required_path(
        base_dir,
        job.archive.as_deref().or(job.src.as_deref()),
        "archive",
    )?;
    let sources = ctx.engine.recovery_protect_sources(&archive)?;
    let redundancy = match job.tolerate_loss {
        Some(count) => redundancy_for_tolerated_volume_loss(&sources, count)?,
        None => job_redundancy(job),
    };
    let recovery_path = job_recovery_path(base_dir, job)?;
    let report = squallz_recovery::protect_files_controlled(
        &archive,
        redundancy,
        recovery_path.as_deref(),
        &sources,
        &NoProgress,
        &ctx.ctl,
    )?;
    recovery_success(report, false)
}

fn job_redundancy(job: &BatchJob) -> u8 {
    job.redundancy.map_or(10, |redundancy| redundancy)
}

fn run_verify_recovery_job(
    ctx: &Ctx,
    base_dir: &Path,
    job: &BatchJob,
) -> Result<JobSuccess, FormatError> {
    let archive = required_path(
        base_dir,
        job.archive.as_deref().or(job.src.as_deref()),
        "archive",
    )?;
    let recovery_path = job_recovery_path(base_dir, job)?;
    let report = squallz_recovery::verify_controlled(
        &archive,
        recovery_path.as_deref(),
        &NoProgress,
        &ctx.ctl,
    )?;
    recovery_success(report, true)
}

fn run_repair_recovery_job(
    ctx: &Ctx,
    base_dir: &Path,
    job: &BatchJob,
) -> Result<JobSuccess, FormatError> {
    let archive = required_path(
        base_dir,
        job.archive.as_deref().or(job.src.as_deref()),
        "archive",
    )?;
    let output = job
        .output
        .as_deref()
        .or(job.dest.as_deref())
        .map(|path| resolve_path(base_dir, path));
    let output_dir = job
        .output_dir
        .as_deref()
        .map(|path| resolve_path(base_dir, path));
    if output.is_some() && output_dir.is_some() {
        return Err(FormatError::Unsupported(
            "batch repair_recovery accepts either output/dest or output_dir, not both".into(),
        ));
    }
    let recovery_path = job_recovery_path(base_dir, job)?;
    let report = match output_dir.as_deref() {
        Some(directory) => squallz_recovery::repair_to_directory_controlled(
            &archive,
            directory,
            recovery_path.as_deref(),
            &NoProgress,
            &ctx.ctl,
        )?,
        None => squallz_recovery::repair_controlled(
            &archive,
            output.as_deref(),
            recovery_path.as_deref(),
            &NoProgress,
            &ctx.ctl,
        )?,
    };
    recovery_success(report, true)
}

fn run_update_job(ctx: &Ctx, base_dir: &Path, job: &BatchJob) -> Result<JobSuccess, FormatError> {
    let archive = required_path(base_dir, job.archive.as_deref(), "archive")?;
    let mut ops = Vec::new();
    let add_inputs = if job.add.is_empty() {
        &job.inputs
    } else {
        &job.add
    };
    for src in add_inputs {
        let src = resolve_path(base_dir, src);
        let dest = path_file_name_string_or_empty(&src);
        ops.push(UpdateOp::Add {
            src,
            dest: EntryPath::from_utf8(dest),
        });
    }
    for path in &job.mkdir {
        ops.push(UpdateOp::AddDir {
            path: EntryPath::from_utf8(path.clone()),
        });
    }
    for pattern in &job.delete {
        ops.push(UpdateOp::Delete {
            pattern: pattern.clone(),
        });
    }
    for item in job.rename.iter().chain(job.move_entries.iter()) {
        let (from, to) = item.as_pair()?;
        ops.push(UpdateOp::Rename {
            from: EntryPath::from_utf8(from),
            to: EntryPath::from_utf8(to),
        });
    }
    if ops.is_empty() {
        return Err(FormatError::Unsupported(
            "batch update job has no operations".into(),
        ));
    }
    let operation_count = ops.len();
    let level = job_level(job)?;
    let excludes =
        crate::content_policy::resolve_create_excludes(job.content_policy, job.excludes.clone());
    let opts = CreateOptions {
        level: CompressionLevel::from_numeric(level),
        password: job.password.clone().map(Password::new),
        encrypt_filenames: job.encrypt_names,
        excludes,
        resources: resource_options(job.threads, job.memory_limit),
        ..CreateOptions::default()
    };
    ctx.engine
        .update(&archive, &ops, &opts, &NoProgress, &ctx.ctl)?;
    Ok(JobSuccess {
        detail: format!("updated {}", archive.display()),
        result: json!({
            "operation": "update",
            "archive": archive.display().to_string(),
            "operations": operation_count,
            "level": level,
        }),
    })
}

fn path_file_name_string_or_empty(path: &Path) -> String {
    match path.file_name() {
        Some(name) => name.to_string_lossy().into_owned(),
        None => String::new(),
    }
}

fn resolve_inputs(base_dir: &Path, inputs: &[PathBuf]) -> Result<Vec<PathBuf>, FormatError> {
    if inputs.is_empty() {
        return Err(FormatError::Unsupported(
            "batch job missing inputs".to_owned(),
        ));
    }
    Ok(inputs
        .iter()
        .map(|input| resolve_path(base_dir, input))
        .collect())
}

fn required_path(
    base_dir: &Path,
    value: Option<&Path>,
    field: &str,
) -> Result<PathBuf, FormatError> {
    value
        .map(|path| resolve_path(base_dir, path))
        .ok_or_else(|| FormatError::Unsupported(format!("batch job missing {field}")))
}

fn resolve_path(base_dir: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base_dir.join(path)
    }
}

fn open_options(job: &BatchJob) -> OpenOptions {
    OpenOptions {
        password: job.password.clone().map(Password::new),
        encoding_override: job.encoding.clone(),
    }
}

fn parse_overwrite(value: Option<&str>) -> Result<OverwritePolicy, FormatError> {
    let normalized = value.map_or("skip", |value| value).to_ascii_lowercase();
    match normalized.as_str() {
        "overwrite" | "replace" | "all" => Ok(OverwritePolicy::Overwrite),
        "skip" => Ok(OverwritePolicy::Skip),
        "rename" | "rename-both" | "keep-both" => Ok(OverwritePolicy::RenameBoth),
        // Batch scripts are non-interactive; `ask` degrades to the safe policy.
        "ask" => Ok(OverwritePolicy::Skip),
        other => Err(FormatError::Unsupported(format!(
            "unsupported batch overwrite policy: {other}"
        ))),
    }
}

fn parse_symlinks(value: Option<&str>) -> Result<SymlinkPolicy, FormatError> {
    let normalized = value.map_or("preserve", |value| value).to_ascii_lowercase();
    match normalized.as_str() {
        "preserve" => Ok(SymlinkPolicy::Preserve),
        "follow" => Ok(SymlinkPolicy::Follow),
        "skip" => Ok(SymlinkPolicy::Skip),
        other => Err(FormatError::Unsupported(format!(
            "unsupported batch symlink policy: {other}"
        ))),
    }
}

fn normalize_operation(operation: &str) -> &str {
    match operation {
        "create" => "compress",
        "pack_sqz" => "pack",
        "export_sqz" => "export",
        "check_checksum" => "checksum_check",
        "verify_checksum" => "checksum_check",
        "duplicate_scan" => "duplicates",
        "verify" => "verify_recovery",
        "repair_par2" => "repair_recovery",
        "verify_par2" => "verify_recovery",
        "protect_recovery" => "protect",
        other => other,
    }
}

fn parse_checksum_algorithm(value: Option<&str>) -> Result<ChecksumAlgorithm, FormatError> {
    let raw = value.map_or("sha256", |value| value);
    let normalized = raw.trim().to_ascii_lowercase().to_owned();
    if let Some(algorithm) = ChecksumAlgorithm::parse_alias(&normalized) {
        Ok(algorithm)
    } else {
        Err(FormatError::Unsupported(format!(
            "unsupported batch checksum algorithm: {normalized}"
        )))
    }
}

fn checksum_report_json(report: &squallz_core::ChecksumReport) -> Value {
    json!({
        "ok": true,
        "operation": "checksum",
        "algorithm": report.algorithm.id(),
        "input_count": report.input_count,
        "entries_scanned": report.entries_scanned,
        "files_hashed": report.files_hashed,
        "bytes_hashed": report.bytes_hashed,
        "items": report.items.iter().map(|item| {
            json!({
                "path": item.path.display().to_string(),
                "size": item.size,
                "digest": item.digest,
            })
        }).collect::<Vec<_>>(),
    })
}

fn checksum_check_report_json(report: &squallz_core::ChecksumVerificationReport) -> Value {
    json!({
        "ok": report.is_ok(),
        "operation": "checksum_check",
        "algorithm": report.algorithm.id(),
        "manifest": report.manifest.display().to_string(),
        "checked": report.checked,
        "passed": report.passed,
        "failed": report.failed,
        "bytes_hashed": report.bytes_hashed,
        "items": report.items.iter().map(|item| {
            json!({
                "path": item.path.display().to_string(),
                "expected": item.expected,
                "actual": item.actual,
                "ok": item.ok,
                "error": item.error,
            })
        }).collect::<Vec<_>>(),
    })
}

fn duplicate_report_json(report: &squallz_core::DuplicateScanReport, min_size: u64) -> Value {
    json!({
        "ok": true,
        "operation": "duplicates",
        "hash_algorithm": "blake3",
        "input_count": report.input_count,
        "entries_scanned": report.entries_scanned,
        "files_scanned": report.files_scanned,
        "bytes_scanned": report.bytes_scanned,
        "min_size": min_size,
        "candidate_files": report.candidate_files,
        "hashed_bytes": report.hashed_bytes,
        "duplicate_groups": report.duplicate_groups(),
        "duplicate_files": report.duplicate_files(),
        "reclaimable_bytes": report.reclaimable_bytes(),
        "groups": report.groups.iter().map(|group| {
            json!({
                "hash": group.hash,
                "hash_algorithm": "blake3",
                "size": group.size,
                "count": group.count(),
                "reclaimable_bytes": group.reclaimable_bytes(),
                "paths": group.paths.iter().map(|path| path.display().to_string()).collect::<Vec<_>>(),
            })
        }).collect::<Vec<_>>(),
    })
}

fn job_recovery_percent(job: &BatchJob, default: u8) -> Result<u8, FormatError> {
    match job.recovery.as_ref() {
        None => Ok(default),
        Some(BatchRecovery::Percent(value)) => Ok(*value),
        Some(BatchRecovery::Text(value)) => value
            .trim()
            .trim_end_matches('%')
            .parse::<u8>()
            .map_err(|_| {
                FormatError::Unsupported(format!(
                    "batch recovery percent must be 0-100, got {value}"
                ))
            }),
    }
}

fn job_recovery_path(base_dir: &Path, job: &BatchJob) -> Result<Option<PathBuf>, FormatError> {
    if let Some(path) = job.recovery_path.as_deref() {
        return Ok(Some(resolve_path(base_dir, path)));
    }
    match job.recovery.as_ref() {
        Some(BatchRecovery::Text(value)) if !looks_like_percent(value) => {
            Ok(Some(resolve_path(base_dir, Path::new(value))))
        }
        Some(BatchRecovery::Percent(_)) => Err(FormatError::Unsupported(
            "batch PAR2 jobs require recovery_path/recovery as a path, not a percent".into(),
        )),
        _ => Ok(None),
    }
}

fn looks_like_percent(value: &str) -> bool {
    let trimmed = value.trim().trim_end_matches('%');
    !trimmed.is_empty() && trimmed.bytes().all(|byte| byte.is_ascii_digit())
}

fn recovery_success(
    report: squallz_recovery::RecoveryReport,
    corrupt_on_failure: bool,
) -> Result<JobSuccess, FormatError> {
    let result = serde_json::to_value(&report)
        .map_err(|e| FormatError::Other(format!("cannot encode recovery report: {e}")))?;
    if !report.ok {
        return if corrupt_on_failure {
            Err(FormatError::CorruptArchive(if report.stderr.is_empty() {
                format!("PAR2 {} failed", report.operation)
            } else {
                report.stderr.clone()
            }))
        } else {
            Err(FormatError::Other(format!(
                "PAR2 {operation} failed with status {status}",
                operation = report.operation,
                status = status_code_label(report.status_code)
            )))
        };
    }
    Ok(JobSuccess {
        detail: format!(
            "{} {} using {}",
            report.operation,
            report.archive.display(),
            report.recovery.display()
        ),
        result,
    })
}

fn status_code_label(status_code: Option<i32>) -> String {
    match status_code {
        Some(code) => code.to_string(),
        None => "unknown".to_owned(),
    }
}

fn redundancy_for_tolerated_volume_loss(
    sources: &[PathBuf],
    tolerate_loss: u32,
) -> Result<u8, FormatError> {
    if sources.len() <= 1 {
        return Err(FormatError::Unsupported(
            "batch tolerate_loss requires a multi-file archive set".into(),
        ));
    }
    let count = tolerated_loss_count(tolerate_loss);
    if count > sources.len() {
        return Err(FormatError::Unsupported(format!(
            "batch tolerate_loss {tolerate_loss} exceeds volume count {}",
            sources.len()
        )));
    }
    let mut sizes = Vec::with_capacity(sources.len());
    for path in sources {
        sizes.push(fs::metadata(path).map_err(FormatError::from)?.len());
    }
    let total: u64 = sizes.iter().sum();
    if total == 0 {
        return Ok(100);
    }
    sizes.sort_unstable_by(|a, b| b.cmp(a));
    let needed: u64 = sizes.into_iter().take(count).sum();
    Ok(needed.saturating_mul(100).div_ceil(total).clamp(1, 100) as u8)
}

fn tolerated_loss_count(tolerate_loss: u32) -> usize {
    match usize::try_from(tolerate_loss) {
        Ok(count) => count,
        Err(_) => usize::MAX,
    }
}

fn job_level(job: &BatchJob) -> Result<u8, FormatError> {
    if let Some(level) = job.level {
        return if level <= 9 {
            Ok(level)
        } else {
            Err(FormatError::Unsupported(format!(
                "batch compression level must be 0-9, got {level}"
            )))
        };
    }
    match job_profile(job) {
        "fast" => Ok(2),
        "balanced" | "standard" => Ok(6),
        "maximum" | "max" => Ok(9),
        other => Err(FormatError::Unsupported(format!(
            "unsupported batch compression profile: {other}"
        ))),
    }
}

fn job_profile(job: &BatchJob) -> &str {
    match job.profile.as_deref() {
        Some(profile) => profile,
        None => "balanced",
    }
}

fn validate_requested_format(
    ctx: &Ctx,
    output: &Path,
    requested: Option<&str>,
) -> Result<(), FormatError> {
    let Some(requested) = requested else {
        return Ok(());
    };
    let output_name = output
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| FormatError::Unsupported("output path has no valid file name".into()))?;
    let output_key = detected_format_key(ctx, output_name).ok_or_else(|| {
        FormatError::Unsupported(format!(
            "output path does not identify a supported format: {}",
            output.display()
        ))
    })?;
    let requested_key = requested_format_key(ctx, requested).ok_or_else(|| {
        FormatError::Unsupported(format!("unsupported requested format: {requested}"))
    })?;
    if output_key != requested_key {
        return Err(FormatError::Unsupported(format!(
            "requested format '{requested}' does not match output path '{}'",
            output.display()
        )));
    }
    Ok(())
}

fn requested_format_key(ctx: &Ctx, requested_format: &str) -> Option<String> {
    let requested = requested_format
        .trim()
        .trim_start_matches('.')
        .to_ascii_lowercase();
    if requested.is_empty() {
        return None;
    }
    let direct_name = format!("archive.{requested}");
    if let Some(key) = detected_format_key(ctx, &direct_name) {
        return Some(key);
    }
    ctx.engine
        .supported_formats()
        .into_iter()
        .find(|format| format.id.eq_ignore_ascii_case(&requested))
        .and_then(|format| {
            format
                .extensions
                .first()
                .and_then(|ext| detected_format_key(ctx, &format!("archive.{ext}")))
        })
}

fn detected_format_key(ctx: &Ctx, name: &str) -> Option<String> {
    match ctx.engine.registry().detect_by_name(name)? {
        Detected::Archive(archive) => Some(format!("archive:{}", archive.id())),
        Detected::Compressed {
            compressor,
            inner_archive: Some(archive),
        } => Some(format!("compound:{}:{}", archive.id(), compressor.id())),
        Detected::Compressed {
            compressor,
            inner_archive: None,
        } => Some(format!("compressor:{}", compressor.id())),
    }
}

fn test_report_error(report: TestSummary) -> FormatError {
    let omitted = report.problems.omitted();
    let mut problems = report.problems.messages;
    if omitted > 0 {
        problems.push(format!("{omitted} additional integrity problem(s) omitted"));
    }
    let problems = problems.join("; ");
    FormatError::CorruptArchive(if problems.is_empty() {
        "batch test failed".to_owned()
    } else {
        problems
    })
}

impl BatchMove {
    fn as_pair(&self) -> Result<(String, String), FormatError> {
        match self {
            BatchMove::Object { from, to } => Ok((from.clone(), to.clone())),
            BatchMove::Pair([from, to]) => Ok((from.clone(), to.clone())),
            BatchMove::Spec(spec) => spec
                .split_once('=')
                .map(|(from, to)| (from.to_owned(), to.to_owned()))
                .ok_or_else(|| {
                    FormatError::Unsupported(format!(
                        "batch rename/move must be FROM=TO or {{from,to}}, got {spec}"
                    ))
                }),
        }
    }
}

fn is_sqz_source_path(path: &Path) -> bool {
    is_plain_sqz_path(path)
        || path
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| split_volume_name(name).map(|(base, _)| base.to_owned()))
            .is_some_and(|base| is_plain_sqz_path(Path::new(&base)))
}

fn is_plain_sqz_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("sqz"))
}

fn is_plain_zip_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            matches!(
                ext.to_ascii_lowercase().as_str(),
                "zip" | "jar" | "apk" | "cbz" | "ipa"
            )
        })
}

fn print_json_report(
    script: &Path,
    base_dir: &Path,
    keep_going: bool,
    reports: &[BatchJobReport],
    failed: usize,
) -> Result<(), CliError> {
    let jobs = json_job_reports(reports);
    let value = json!({
        "ok": failed == 0,
        "operation": "batch",
        "script": script.display().to_string(),
        "base_dir": base_dir.display().to_string(),
        "keep_going": keep_going,
        "total": reports.len(),
        "failed": failed,
        "jobs": jobs,
        "results": jobs,
    });
    print_pretty_json(&value)
}

fn json_job_reports(reports: &[BatchJobReport]) -> Vec<Value> {
    reports
        .iter()
        .map(|report| {
            if report.ok {
                json!({
                    "id": report.id,
                    "kind": report.kind,
                    "operation": report.kind,
                    "ok": true,
                    "detail": report.detail,
                    "exit_code": 0,
                    "result": report.result,
                })
            } else {
                json!({
                    "id": report.id,
                    "kind": report.kind,
                    "operation": report.kind,
                    "ok": false,
                    "detail": report.detail,
                    "exit_code": report.exit_code,
                    "error_kind": report.error_kind,
                    "error": {
                        "kind": report.error_kind,
                        "message": report.detail,
                        "exit_code": report.exit_code,
                    },
                })
            }
        })
        .collect()
}

fn print_human_report(
    ctx: &Ctx,
    script: &Path,
    keep_going: bool,
    reports: &[BatchJobReport],
    failed: usize,
) {
    let preserved_outputs = batch_preserved_output_paths(reports);
    if ctx.is_modern() {
        let succeeded = reports.len().saturating_sub(failed);
        let tone = if failed > 0 {
            crate::ui::Tone::Danger
        } else if preserved_outputs.is_empty() {
            crate::ui::Tone::Success
        } else {
            crate::ui::Tone::Warning
        };
        let status = if failed > 0 {
            "failed".to_owned()
        } else if preserved_outputs.is_empty() {
            "done".to_owned()
        } else {
            ctx.loc.t("cli.create.preserved_status")
        };
        ctx.print_modern_status_panel(
            "Batch result",
            &status,
            tone,
            &format!("{} jobs from {}", reports.len(), script.display()),
            &[
                super::ModernStatusField::new("Jobs", reports.len().to_string()),
                super::ModernStatusField::new("Succeeded", succeeded.to_string()),
                super::ModernStatusField::new("Failed", failed.to_string()),
                super::ModernStatusField::new("Keep going", keep_going.to_string()),
            ],
        );
        let rows = reports
            .iter()
            .map(|report| {
                let cells = vec![
                    report.id.clone(),
                    report.kind.clone(),
                    if report.ok { "ok" } else { "failed" }.to_owned(),
                    report.exit_code.to_string(),
                    report.detail.clone(),
                ];
                if report.ok {
                    super::ModernTableRow::success(cells)
                } else {
                    super::ModernTableRow::danger(cells)
                }
            })
            .collect::<Vec<_>>();
        ctx.print_modern_wrapped_table(
            "Batch jobs",
            &[
                super::ModernTableColumn::new("Job", 18),
                super::ModernTableColumn::new("Operation", 16),
                super::ModernTableColumn::new("Status", 8),
                super::ModernTableColumn::right("Exit", 6),
                super::ModernTableColumn::new("Detail", 48),
            ],
            &rows,
        );
        print_preserved_output_warning(ctx, &preserved_outputs);
        return;
    }
    for report in reports {
        if report.ok {
            ctx.print_success(format!("{}: {}", report.id, report.detail));
        } else {
            ctx.eprint_problem(format!("{}: {}", report.id, report.detail));
        }
    }
    print_preserved_output_warning(ctx, &preserved_outputs);
}

fn batch_preserved_output_paths(reports: &[BatchJobReport]) -> Vec<String> {
    reports
        .iter()
        .filter(|report| report.ok)
        .filter_map(|report| report.result.as_ref())
        .filter_map(|result| result.get("preserved_outputs"))
        .filter_map(Value::as_array)
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}
