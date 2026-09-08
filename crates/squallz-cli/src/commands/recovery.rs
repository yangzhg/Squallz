use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use squallz_core::api::{
    split_volume_name, CompressionLevel, CreateOptions, FormatError, NoProgress, OpenOptions,
};
use squallz_core::{is_plain_sqz_path, is_sqz_archive_path, is_zip_family_path};
use squallz_recovery::RecoveryReport;

use crate::args::resource_options;
use crate::commands::{
    reports::{
        print_pretty_json, print_test_problems, print_test_problems_with_structure,
        recovery_summary_json, test_report_json, test_report_json_with_structure,
    },
    Ctx, ModernStatusField, ModernTableColumn, ModernTableRow,
};
use crate::errors::CliError;
use crate::progress::CliProgress;
use crate::ui::Tone;

const EXIT_CORRUPT: i32 = 3;
const DEFAULT_REDUNDANCY_PERCENT: u8 = 10;

fn redundancy_or_default(redundancy: Option<u8>) -> u8 {
    match redundancy {
        Some(value) => value,
        None => DEFAULT_REDUNDANCY_PERCENT,
    }
}

fn repair_output_or_archive(output: Option<PathBuf>, archive: &Path) -> PathBuf {
    match output {
        Some(path) => path,
        None => archive.to_path_buf(),
    }
}

fn tolerated_loss_count(tolerate_loss: u32) -> usize {
    match usize::try_from(tolerate_loss) {
        Ok(count) => count,
        Err(_) => usize::MAX,
    }
}

fn status_code_or_unknown(status_code: Option<i32>) -> String {
    match status_code {
        Some(code) => code.to_string(),
        None => "unknown".to_owned(),
    }
}

fn status_code_or_dash(status_code: Option<i32>) -> String {
    match status_code {
        Some(code) => code.to_string(),
        None => "-".to_owned(),
    }
}

fn optional_number_or_dash<T: std::fmt::Display>(value: Option<T>) -> String {
    match value {
        Some(count) => count.to_string(),
        None => "-".to_owned(),
    }
}

pub fn protect(
    ctx: &Ctx,
    archive: PathBuf,
    redundancy: Option<u8>,
    tolerate_loss: Option<u32>,
    recovery: Option<PathBuf>,
    json: bool,
) -> Result<(), CliError> {
    let sources = ctx.engine.recovery_protect_sources(&archive)?;
    let redundancy = match tolerate_loss {
        Some(count) => redundancy_for_tolerated_volume_loss(&sources, count)?,
        None => redundancy_or_default(redundancy),
    };
    let progress = CliProgress::new_for_operation(
        ctx.quiet,
        ctx.verbose,
        json,
        ctx.output_style,
        ctx.color,
        ctx.accent,
        "protect",
    );
    let report = squallz_recovery::protect_files_controlled(
        &archive,
        redundancy,
        recovery.as_deref(),
        &sources,
        &progress,
        &ctx.ctl,
    );
    progress.finish();
    let report = report?;
    emit_report(ctx, &report, json, false)
}

pub fn verify(
    ctx: &Ctx,
    archive: PathBuf,
    _use_recovery: bool,
    recovery: Option<PathBuf>,
    json: bool,
) -> Result<(), CliError> {
    let progress = CliProgress::new_for_operation(
        ctx.quiet,
        ctx.verbose,
        json,
        ctx.output_style,
        ctx.color,
        ctx.accent,
        "verify",
    );
    let report =
        squallz_recovery::verify_controlled(&archive, recovery.as_deref(), &progress, &ctx.ctl);
    progress.finish();
    let report = report?;
    emit_report(ctx, &report, json, true)
}

#[allow(clippy::too_many_arguments)] // direct image of the CLI surface
pub fn repair(
    ctx: &Ctx,
    archive: PathBuf,
    use_recovery: bool,
    output: Option<PathBuf>,
    output_dir: Option<PathBuf>,
    recovery: Option<PathBuf>,
    level: u8,
    threads: Option<usize>,
    memory_limit: Option<u64>,
    json: bool,
) -> Result<(), CliError> {
    if output_dir.is_some() && !use_recovery {
        return Err(FormatError::Unsupported(
            "--output-dir is only available with --use-recovery".into(),
        )
        .into());
    }
    if use_recovery {
        let progress = CliProgress::new_for_operation(
            ctx.quiet,
            ctx.verbose,
            json,
            ctx.output_style,
            ctx.color,
            ctx.accent,
            "repair",
        );
        let report = match output_dir.as_deref() {
            Some(directory) => squallz_recovery::repair_to_directory_controlled(
                &archive,
                directory,
                recovery.as_deref(),
                &progress,
                &ctx.ctl,
            ),
            None => squallz_recovery::repair_controlled(
                &archive,
                output.as_deref(),
                recovery.as_deref(),
                &progress,
                &ctx.ctl,
            ),
        };
        progress.finish();
        let report = report?;
        return emit_report(ctx, &report, json, true);
    }
    if is_sqz_archive_path(&archive) {
        return repair_sqz(
            ctx,
            archive,
            output,
            recovery,
            level,
            threads,
            memory_limit,
            json,
        );
    }
    if is_zip_family_path(&archive) {
        return repair_zip_rebuild(
            ctx,
            archive,
            output,
            recovery,
            level,
            threads,
            memory_limit,
            json,
        );
    }
    Err(FormatError::Unsupported(
        "repair without --use-recovery is supported only for .sqz embedded recovery or ZIP local-header rebuild".into(),
    )
    .into())
}

#[allow(clippy::too_many_arguments)] // direct image of the CLI surface
fn repair_sqz(
    ctx: &Ctx,
    archive: PathBuf,
    output: Option<PathBuf>,
    recovery: Option<PathBuf>,
    level: u8,
    threads: Option<usize>,
    memory_limit: Option<u64>,
    json: bool,
) -> Result<(), CliError> {
    if recovery.is_some() {
        return Err(FormatError::Unsupported(
            ".sqz repair uses embedded recovery; omit --recovery or pass --use-recovery for PAR2"
                .into(),
        )
        .into());
    }
    let in_place_requested = output.is_none();
    if in_place_requested && is_split_sqz_volume_path(&archive) {
        return Err(FormatError::Unsupported(
            ".sqz split-volume repair requires --output <path>".into(),
        )
        .into());
    }
    let output = repair_output_or_archive(output, &archive);
    if !is_plain_sqz_path(&output) {
        return Err(
            FormatError::Unsupported("SQZ repair output must be a .sqz container".into()).into(),
        );
    }

    let source_report =
        ctx.engine
            .test_summary(&archive, &OpenOptions::default(), &NoProgress, &ctx.ctl)?;
    if !source_report.is_ok() {
        if json {
            let archive_path = archive.display().to_string();
            let output_path = output.display().to_string();
            let value = json!({
                "ok": false,
                "operation": "repair_sqz",
                "archive": archive_path,
                "output": output_path,
                "tool": "sqz-embedded-recovery",
                "in_place": false,
                "source": test_report_json(&source_report),
                "recovery": source_report.recovery.as_ref().map(recovery_summary_json),
                "problems": &source_report.problems.messages,
                "problems_total": source_report.problems.total,
                "problems_truncated": source_report.problems.is_truncated(),
            });
            print_pretty_json(&value)?;
        } else {
            print_test_problems(ctx, &source_report);
            let count = source_report.problems.total.to_string();
            let message = ctx.loc.format("cli.test.failed", &[("count", &count)]);
            ctx.eprint_problem(&message);
        }
        return Err(CliError::Exit(EXIT_CORRUPT));
    }

    let progress = CliProgress::new_for_operation(
        ctx.quiet,
        ctx.verbose,
        json,
        ctx.output_style,
        ctx.color,
        ctx.accent,
        "repair",
    );
    let create = CreateOptions {
        level: CompressionLevel::from_numeric(level),
        resources: resource_options(threads, memory_limit),
        ..CreateOptions::default()
    };
    let in_place = ctx.engine.convert_with_atomic_replace(
        &archive,
        &output,
        &OpenOptions::default(),
        &create,
        &progress,
        &ctx.ctl,
    )?;
    progress.finish();
    if json {
        let archive_path = archive.display().to_string();
        let output_path = output.display().to_string();
        let value = json!({
                "ok": true,
                "operation": "repair_sqz",
                "archive": archive_path,
                "output": output_path,
                "tool": "sqz-embedded-recovery",
                "in_place": in_place,
                "source": test_report_json(&source_report),
                "recovery": source_report.recovery.as_ref().map(recovery_summary_json),
        });
        print_pretty_json(&value)?;
    } else if ctx.is_modern() {
        print_archive_repair_modern(
            ctx,
            &ctx.loc.t("cli.sqz.repair.result_title"),
            "repair_sqz",
            &archive,
            &output,
            "sqz-embedded-recovery",
            in_place,
        );
    } else {
        let path = output.display().to_string();
        let message = ctx.loc.format("cli.sqz.repair.done", &[("path", &path)]);
        ctx.print_success(&message);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)] // direct image of the CLI surface
fn repair_zip_rebuild(
    ctx: &Ctx,
    archive: PathBuf,
    output: Option<PathBuf>,
    recovery: Option<PathBuf>,
    level: u8,
    threads: Option<usize>,
    memory_limit: Option<u64>,
    json: bool,
) -> Result<(), CliError> {
    if recovery.is_some() {
        return Err(FormatError::Unsupported(
            "ZIP rebuild uses local headers; pass --use-recovery to repair with PAR2 data".into(),
        )
        .into());
    }
    let Some(output) = output else {
        return Err(
            FormatError::Unsupported("ZIP rebuild repair requires --output <path>".into()).into(),
        );
    };
    if !is_zip_family_path(&output) {
        return Err(FormatError::Unsupported(
            "ZIP rebuild output must be a ZIP-family archive (.zip/.jar/.apk/.cbz/.ipa)".into(),
        )
        .into());
    }

    let source_test = ctx.engine.test_summary_with_structure(
        &archive,
        &OpenOptions::default(),
        &NoProgress,
        &ctx.ctl,
    )?;
    if !source_test.payload_is_ok() {
        let source_report = &source_test.summary;
        if json {
            let value = json!({
                "ok": false,
                "operation": "repair_zip",
                "archive": archive.display().to_string(),
                "output": output.display().to_string(),
                "tool": "zip-local-header-rebuild",
                "in_place": false,
                "source": test_report_json_with_structure(source_report, source_test.structure),
                "problems": &source_report.problems.messages,
                "problems_total": source_report.problems.total,
                "problems_truncated": source_report.problems.is_truncated(),
            });
            print_pretty_json(&value)?;
        } else {
            print_test_problems_with_structure(ctx, source_report, source_test.structure);
            let count = source_report.problems.total.to_string();
            let message = ctx.loc.format("cli.test.failed", &[("count", &count)]);
            ctx.eprint_problem(&message);
        }
        return Err(CliError::Exit(EXIT_CORRUPT));
    }
    let structure = source_test.structure;
    let source_report = source_test.into_summary();

    let progress = CliProgress::new_for_operation(
        ctx.quiet,
        ctx.verbose,
        json,
        ctx.output_style,
        ctx.color,
        ctx.accent,
        "repair",
    );
    let create = CreateOptions {
        level: CompressionLevel::from_numeric(level),
        resources: resource_options(threads, memory_limit),
        ..CreateOptions::default()
    };
    let in_place = ctx.engine.convert_with_atomic_replace(
        &archive,
        &output,
        &OpenOptions::default(),
        &create,
        &progress,
        &ctx.ctl,
    )?;
    progress.finish();
    if json {
        let value = json!({
            "ok": true,
            "operation": "repair_zip",
            "archive": archive.display().to_string(),
            "output": output.display().to_string(),
            "tool": "zip-local-header-rebuild",
            "in_place": in_place,
            "source": test_report_json_with_structure(&source_report, structure),
        });
        print_pretty_json(&value)?;
    } else if ctx.is_modern() {
        print_archive_repair_modern(
            ctx,
            &ctx.loc.t("cli.zip.repair.result_title"),
            "repair_zip",
            &archive,
            &output,
            "zip-local-header-rebuild",
            in_place,
        );
    } else {
        let path = output.display().to_string();
        let message = ctx.loc.format("cli.zip.repair.done", &[("path", &path)]);
        ctx.print_success(&message);
    }
    Ok(())
}

fn is_split_sqz_volume_path(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    split_volume_name(name).is_some_and(|(base, _)| {
        Path::new(base)
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("sqz"))
    })
}

fn redundancy_for_tolerated_volume_loss(
    sources: &[PathBuf],
    tolerate_loss: u32,
) -> Result<u8, CliError> {
    if sources.len() <= 1 {
        return Err(FormatError::Unsupported(
            "--tolerate-loss requires a multi-file archive set".into(),
        )
        .into());
    }
    let count = tolerated_loss_count(tolerate_loss);
    if count > sources.len() {
        return Err(FormatError::Unsupported(format!(
            "--tolerate-loss {tolerate_loss} exceeds volume count {}",
            sources.len()
        ))
        .into());
    }
    let mut sizes = Vec::with_capacity(sources.len());
    for path in sources {
        sizes.push(std::fs::metadata(path).map_err(FormatError::from)?.len());
    }
    let total: u64 = sizes.iter().sum();
    if total == 0 {
        return Ok(100);
    }
    sizes.sort_unstable_by(|a, b| b.cmp(a));
    let needed: u64 = sizes.into_iter().take(count).sum();
    let percent = needed.saturating_mul(100).div_ceil(total).clamp(1, 100);
    Ok(percent as u8)
}

fn emit_report(
    ctx: &Ctx,
    report: &RecoveryReport,
    json_output: bool,
    corrupt_on_failure: bool,
) -> Result<(), CliError> {
    if json_output {
        let value = recovery_report_value(report)?;
        print_pretty_json(&value)?;
    } else if report.ok {
        if ctx.is_modern() {
            print_recovery_report_modern(ctx, report);
            return Ok(());
        }
        let path = report.recovery.display().to_string();
        let key = match report.operation {
            "protect" => "cli.recovery.protect.done",
            "verify" => "cli.recovery.verify.ok",
            "repair" => "cli.recovery.repair.done",
            _ => "cli.recovery.done",
        };
        let message = ctx.loc.format(key, &[("path", &path)]);
        ctx.print_success(&message);
        if !report.outputs.is_empty() {
            println!("{}:", ctx.loc.t("common.files"));
            for output in &report.outputs {
                println!("  {}", output.display());
            }
        }
    } else if !report.stderr.is_empty() {
        ctx.eprint_problem(&report.stderr);
    }

    if report.ok {
        Ok(())
    } else if corrupt_on_failure {
        Err(CliError::Exit(EXIT_CORRUPT))
    } else {
        Err(squallz_core::api::FormatError::Other(format!(
            "PAR2 {operation} failed with status {status}",
            operation = report.operation,
            status = status_code_or_unknown(report.status_code)
        ))
        .into())
    }
}

fn recovery_report_value(report: &RecoveryReport) -> Result<Value, CliError> {
    serde_json::to_value(report)
        .map_err(|e| FormatError::Other(format!("cannot serialize CLI JSON report: {e}")).into())
}

fn print_archive_repair_modern(
    ctx: &Ctx,
    title: &str,
    operation: &str,
    archive: &Path,
    output: &Path,
    tool: &str,
    in_place: bool,
) {
    let output_path = output.display().to_string();
    ctx.print_modern_status_panel(
        title,
        &ctx.loc.t("common.done"),
        Tone::Success,
        &format!("{tool} · {output_path}"),
        &[
            ModernStatusField::new(ctx.loc.t("common.archive"), archive.display().to_string()),
            ModernStatusField::new(ctx.loc.t("common.output"), output_path.clone()),
            ModernStatusField::new(ctx.loc.t("common.tool"), tool),
        ],
    );
    ctx.print_modern_table(
        &ctx.loc.t("cli.repair.report_title"),
        &[
            ModernTableColumn::new(ctx.loc.t("common.operation"), 14),
            ModernTableColumn::new(ctx.loc.t("common.tool"), 26),
            ModernTableColumn::new(ctx.loc.t("common.in_place"), 10),
            ModernTableColumn::new(ctx.loc.t("common.archive"), 36),
            ModernTableColumn::new(ctx.loc.t("common.output"), 36),
        ],
        &[ModernTableRow::success(vec![
            operation.to_owned(),
            tool.to_owned(),
            in_place.to_string(),
            archive.display().to_string(),
            output_path,
        ])],
    );
}

fn print_recovery_report_modern(ctx: &Ctx, report: &RecoveryReport) {
    let recovery_path = report.recovery.display().to_string();
    let mut fields = vec![
        ModernStatusField::new(ctx.loc.t("common.operation"), report.operation),
        ModernStatusField::new(
            ctx.loc.t("common.archive"),
            report.archive.display().to_string(),
        ),
        ModernStatusField::new(ctx.loc.t("common.recovery"), recovery_path.clone()),
    ];
    if !report.outputs.is_empty() {
        fields.push(ModernStatusField::new(
            ctx.loc.t("common.files"),
            report.outputs.len().to_string(),
        ));
    }
    ctx.print_modern_status_panel(
        &ctx.loc.t("cli.recovery.result_title"),
        &ctx.loc.t("common.done"),
        Tone::Success,
        &format!("{} · {}", report.operation, recovery_path),
        &fields,
    );
    ctx.print_modern_table(
        &ctx.loc.t("cli.recovery.report_title"),
        &[
            ModernTableColumn::new(ctx.loc.t("common.operation"), 12),
            ModernTableColumn::new(ctx.loc.t("common.archive"), 34),
            ModernTableColumn::new(ctx.loc.t("common.recovery"), 34),
            ModernTableColumn::new(ctx.loc.t("common.tool"), 22),
            ModernTableColumn::right(ctx.loc.t("common.status"), 8),
        ],
        &[ModernTableRow::success(vec![
            report.operation.to_owned(),
            report.archive.display().to_string(),
            recovery_path,
            report.tool.display().to_string(),
            status_code_or_dash(report.status_code),
        ])],
    );
    if !report.outputs.is_empty() {
        let rows = report
            .outputs
            .iter()
            .map(|output| {
                let name = match output.file_name() {
                    Some(name) => name.to_string_lossy().into_owned(),
                    None => output.display().to_string(),
                };
                ModernTableRow::success(vec![name])
            })
            .collect::<Vec<_>>();
        ctx.print_modern_wrapped_table(
            &ctx.loc.t("common.files"),
            &[ModernTableColumn::new(ctx.loc.t("common.name"), 100)],
            &rows,
        );
    }
    if let Some(metrics) = &report.metrics {
        ctx.print_modern_table(
            &ctx.loc.t("cli.recovery.metrics_title"),
            &[
                ModernTableColumn::new(ctx.loc.t("common.metric"), 28),
                ModernTableColumn::new(ctx.loc.t("common.value"), 18),
                ModernTableColumn::new(ctx.loc.t("common.detail"), 44),
            ],
            &[
                ModernTableRow::new(vec![
                    "repair_possible".to_owned(),
                    metrics.repair_possible.to_string(),
                    ctx.loc.t("cli.recovery.metric.repair_possible.detail"),
                ]),
                ModernTableRow::new(vec![
                    "blocks_needed".to_owned(),
                    metrics.blocks_needed.to_string(),
                    ctx.loc.t("cli.recovery.metric.blocks_needed.detail"),
                ]),
                ModernTableRow::new(vec![
                    "recovery_blocks_available".to_owned(),
                    metrics.recovery_blocks_available.to_string(),
                    ctx.loc.t("cli.recovery.metric.blocks_available.detail"),
                ]),
                ModernTableRow::new(vec![
                    "blocks_repaired".to_owned(),
                    optional_number_or_dash(metrics.blocks_repaired),
                    ctx.loc.t("cli.recovery.metric.blocks_repaired.detail"),
                ]),
                ModernTableRow::new(vec![
                    "files_repaired".to_owned(),
                    optional_number_or_dash(metrics.files_repaired),
                    ctx.loc.t("cli.recovery.metric.files_repaired.detail"),
                ]),
            ],
        );
    }
}
