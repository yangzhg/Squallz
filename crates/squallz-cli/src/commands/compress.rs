//! `sqz compress`: create an archive from files/directories, optionally as
//! generic `.001` volumes or a supported format-native set.

use std::path::{Path, PathBuf};

use serde_json::json;
use squallz_core::api::{
    CompressionLevel, CreateOptions, Detected, FormatError, OpenOptions, Password, ProgressPhase,
    ProgressSink, SplitOutputMode, SqzCreateOptions,
};
use squallz_core::CreateArtifactKind;

use super::reports::{create_report_json, print_preserved_output_warning, print_pretty_json};
use crate::args::resource_options;
use crate::commands::{Ctx, ModernStatusField, ModernTableColumn, ModernTableRow};
use crate::errors::CliError;
use crate::progress::{fmt_bytes, CliProgress};
use crate::ui::Tone;

struct CreateJsonReport {
    operation: &'static str,
    inner_format: Option<String>,
    recovery_percent: Option<u8>,
}

impl CreateJsonReport {
    fn compress() -> Self {
        Self {
            operation: "compress",
            inner_format: None,
            recovery_percent: None,
        }
    }

    fn pack_sqz(inner_format: String, recovery_percent: u8) -> Self {
        Self {
            operation: "pack_sqz",
            inner_format: Some(inner_format),
            recovery_percent: Some(recovery_percent),
        }
    }
}

#[allow(clippy::too_many_arguments)] // direct image of the CLI surface
pub fn run(
    ctx: &Ctx,
    inputs: Vec<PathBuf>,
    output: PathBuf,
    format: Option<String>,
    level: u8,
    password: Option<String>,
    encrypt_names: bool,
    excludes: Vec<String>,
    split: Option<u64>,
    split_mode: SplitOutputMode,
    threads: Option<usize>,
    memory_limit: Option<u64>,
    test_after_create: bool,
    json_output: bool,
) -> Result<(), CliError> {
    run_create(
        ctx,
        inputs,
        output,
        format.as_deref(),
        SqzCreateOptions::default(),
        level,
        password,
        encrypt_names,
        excludes,
        split,
        split_mode,
        threads,
        memory_limit,
        test_after_create,
        json_output,
        CreateJsonReport::compress(),
    )
}

#[allow(clippy::too_many_arguments)] // direct image of the CLI surface
pub fn run_pack(
    ctx: &Ctx,
    inputs: Vec<PathBuf>,
    output: PathBuf,
    level: u8,
    inner_format: String,
    recovery: u8,
    excludes: Vec<String>,
    split: Option<u64>,
    threads: Option<usize>,
    memory_limit: Option<u64>,
    test_after_create: bool,
    json_output: bool,
) -> Result<(), CliError> {
    let is_sqz = output
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("sqz"));
    if !is_sqz {
        return Err(FormatError::Unsupported("pack output must end with .sqz".into()).into());
    }
    run_create(
        ctx,
        inputs,
        output,
        Some("sqz"),
        SqzCreateOptions {
            inner_format: inner_format.clone(),
            recovery_percent: recovery,
        },
        level,
        None,
        false,
        excludes,
        split,
        SplitOutputMode::Generic,
        threads,
        memory_limit,
        test_after_create,
        json_output,
        CreateJsonReport::pack_sqz(inner_format, recovery),
    )
}

#[allow(clippy::too_many_arguments)]
fn run_create(
    ctx: &Ctx,
    inputs: Vec<PathBuf>,
    output: PathBuf,
    requested_format: Option<&str>,
    sqz: SqzCreateOptions,
    level: u8,
    password: Option<String>,
    encrypt_names: bool,
    excludes: Vec<String>,
    split: Option<u64>,
    split_mode: SplitOutputMode,
    threads: Option<usize>,
    memory_limit: Option<u64>,
    test_after_create: bool,
    json_output: bool,
    json_report: CreateJsonReport,
) -> Result<(), CliError> {
    validate_requested_format(ctx, &output, requested_format)?;
    let progress_operation = if json_report.operation == "pack_sqz" {
        "pack"
    } else {
        "compress"
    };
    let make_progress = || {
        CliProgress::new_for_operation(
            ctx.quiet,
            ctx.verbose,
            json_output,
            ctx.output_style,
            ctx.color,
            ctx.accent,
            progress_operation,
        )
    };
    let opts = CreateOptions {
        level: CompressionLevel::from_numeric(level),
        password: password.map(Password::new),
        encrypt_filenames: encrypt_names,
        excludes,
        split_size: split,
        split_mode,
        resources: resource_options(threads, memory_limit),
        sqz,
    };
    let kind = if split.is_some() {
        CreateArtifactKind::SplitArchive
    } else {
        CreateArtifactKind::Archive
    };
    let inspection_progress = make_progress();
    let policy =
        match super::create_commit_policy(&output, kind, true, &inspection_progress, &ctx.ctl) {
            Ok(policy) => policy,
            Err(error) => {
                inspection_progress.finish();
                return Err(error.into());
            }
        };
    inspection_progress.finish();
    let progress = make_progress();
    let result = ctx
        .engine
        .create_with_report_policy(&output, &inputs, &opts, policy, &progress, &ctx.ctl);
    progress.finish();
    let create_report = result?;
    let entries_tested_after_create = if test_after_create {
        let verify_progress = make_progress();
        verify_progress.on_phase(ProgressPhase::OutputVerify, true);
        let report = ctx.engine.test_summary(
            &create_report.primary_output,
            &OpenOptions {
                password: opts.password.clone(),
                encoding_override: None,
            },
            &verify_progress,
            &ctx.ctl,
        );
        verify_progress.finish();
        let report = report?;
        if !report.is_ok() {
            let preview = report.problems.messages.join("; ");
            let detail = if preview.is_empty() {
                format!(
                    "created archive failed integrity testing with {} problem(s)",
                    report.problems.total
                )
            } else {
                format!("created archive failed integrity testing: {preview}")
            };
            return Err(FormatError::CorruptArchive(detail).into());
        }
        Some(report.entries_tested)
    } else {
        None
    };
    let format_label = create_format_label(ctx, &output, requested_format);
    if json_output {
        let mut value = create_report_json(&create_report);
        value["ok"] = json!(true);
        value["operation"] = json!(json_report.operation);
        value["level"] = json!(level);
        value["tested_after_create"] = json!(entries_tested_after_create.is_some());
        value["entries_tested_after_create"] = json!(entries_tested_after_create);
        add_create_json_fields(&mut value, &json_report);
        print_pretty_json(&value)?;
        return Ok(());
    }
    let path = create_report.primary_output.display().to_string();
    let output_size = fmt_bytes(create_report.total_output_bytes);
    if let Some(volume_count) = create_report.split_volume_count {
        let count = volume_count.to_string();
        if ctx.is_modern() {
            let result = CreateResultView {
                title_key: "cli.compress.result_title_split",
                output: &path,
                volumes: &count,
                format: &format_label,
                level,
                output_size: &output_size,
                input_count: inputs.len(),
                report: &json_report,
            };
            print_create_result(ctx, &result);
        } else {
            let message = ctx.loc.format(
                "cli.compress.done_split",
                &[("path", &path), ("count", &count)],
            );
            ctx.print_success(&message);
        }
    } else {
        if ctx.is_modern() {
            let result = CreateResultView {
                title_key: "cli.compress.result_title",
                output: &path,
                volumes: "1",
                format: &format_label,
                level,
                output_size: &output_size,
                input_count: inputs.len(),
                report: &json_report,
            };
            print_create_result(ctx, &result);
        } else {
            let message = ctx.loc.format("cli.compress.done", &[("path", &path)]);
            ctx.print_success(&message);
        }
    }
    let preserved_outputs = create_report
        .preserved_outputs
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>();
    print_preserved_output_warning(ctx, &preserved_outputs);
    if let Some(entries) = entries_tested_after_create {
        ctx.eprint_notice(ctx.loc.format(
            "cli.compress.tested_after_create",
            &[("count", &entries.to_string())],
        ));
    }
    Ok(())
}

struct CreateResultView<'a> {
    title_key: &'static str,
    output: &'a str,
    volumes: &'a str,
    format: &'a str,
    level: u8,
    output_size: &'a str,
    input_count: usize,
    report: &'a CreateJsonReport,
}

fn print_create_result(ctx: &Ctx, result: &CreateResultView<'_>) {
    ctx.print_modern_status_panel(
        &ctx.loc.t(result.title_key),
        &ctx.loc.t("common.done"),
        Tone::Success,
        &format!(
            "{} · {} · {}",
            result.format, result.output_size, result.output
        ),
        &[
            ModernStatusField::new(ctx.loc.t("common.format"), result.format),
            ModernStatusField::new(ctx.loc.t("common.level"), result.level.to_string()),
            ModernStatusField::new(ctx.loc.t("common.volumes"), result.volumes),
            ModernStatusField::new(ctx.loc.t("common.output_size"), result.output_size),
        ],
    );
    print_create_plan(ctx, result);
    ctx.print_modern_table(
        &ctx.loc.t("cli.compress.summary_title"),
        &[
            ModernTableColumn::new(ctx.loc.t("common.status"), 12),
            ModernTableColumn::new(ctx.loc.t("common.format"), 12),
            ModernTableColumn::right(ctx.loc.t("common.volumes"), 8),
            ModernTableColumn::right(ctx.loc.t("common.output_size"), 14),
            ModernTableColumn::new(ctx.loc.t("common.output"), 50),
        ],
        &[ModernTableRow::success(vec![
            ctx.loc.t("common.done"),
            result.format.to_owned(),
            result.volumes.to_owned(),
            result.output_size.to_owned(),
            result.output.to_owned(),
        ])],
    );
    ctx.print_modern_wrapped_table(
        &ctx.loc.t("cli.compress.route_title"),
        &[
            ModernTableColumn::new(ctx.loc.t("common.lane"), 14),
            ModernTableColumn::new(ctx.loc.t("common.operation"), 14),
            ModernTableColumn::new(ctx.loc.t("common.value"), 18),
            ModernTableColumn::new(ctx.loc.t("common.detail"), 62),
        ],
        &[
            ModernTableRow::new(vec![
                ctx.loc.t("common.source"),
                ctx.loc.t("common.inputs"),
                result.input_count.to_string(),
                ctx.loc.t("common.readiness"),
            ]),
            ModernTableRow::new(vec![
                ctx.loc.t("common.format"),
                ctx.loc.t("common.level"),
                result.level.to_string(),
                result.format.to_owned(),
            ]),
            ModernTableRow::success(vec![
                ctx.loc.t("common.output"),
                ctx.loc.t("common.volumes"),
                result.volumes.to_owned(),
                format!("{} · {}", result.output_size, result.output),
            ]),
            ModernTableRow::new(vec![
                ctx.loc.t("common.status"),
                "sqz test".to_owned(),
                ctx.loc.t("common.recommended"),
                format!("sqz test {}", result.output),
            ]),
        ],
    );
    ctx.print_modern_table(
        &ctx.loc.t("cli.compress.settings_title"),
        &[
            ModernTableColumn::new(ctx.loc.t("common.setting"), 24),
            ModernTableColumn::new(ctx.loc.t("common.value"), 68),
        ],
        &[
            ModernTableRow::new(vec![
                ctx.loc.t("common.inputs"),
                result.input_count.to_string(),
            ]),
            ModernTableRow::new(vec![ctx.loc.t("common.level"), result.level.to_string()]),
            ModernTableRow::new(vec![ctx.loc.t("common.format"), result.format.to_owned()]),
            ModernTableRow::new(vec![ctx.loc.t("common.volumes"), result.volumes.to_owned()]),
        ],
    );
    print_create_details(ctx, result);
    if let Some(inner_format) = &result.report.inner_format {
        let recovery = recovery_percent_label(result.report.recovery_percent);
        ctx.print_modern_table(
            &ctx.loc.t("cli.pack.container_title"),
            &[
                ModernTableColumn::new(ctx.loc.t("common.setting"), 28),
                ModernTableColumn::new(ctx.loc.t("common.value"), 64),
            ],
            &[
                ModernTableRow::new(vec![ctx.loc.t("common.format"), "sqz".to_owned()]),
                ModernTableRow::new(vec![
                    ctx.loc.t("cli.pack.inner_format"),
                    inner_format.clone(),
                ]),
                ModernTableRow::new(vec![ctx.loc.t("cli.pack.recovery_redundancy"), recovery]),
            ],
        );
    }
}

fn print_create_plan(ctx: &Ctx, result: &CreateResultView<'_>) {
    ctx.print_modern_wrapped_table(
        &ctx.loc.t("cli.compress.plan_title"),
        &[
            ModernTableColumn::new(ctx.loc.t("common.stage"), 18),
            ModernTableColumn::new(ctx.loc.t("common.status"), 12),
            ModernTableColumn::new(ctx.loc.t("common.detail"), 44),
            ModernTableColumn::new(ctx.loc.t("common.output"), 36),
        ],
        &[
            ModernTableRow::success(vec![
                ctx.loc.t("cli.compress.stage.scan"),
                ctx.loc.t("common.done"),
                ctx.loc.t("cli.compress.detail.scan"),
                format!("{}: {}", ctx.loc.t("common.inputs"), result.input_count),
            ]),
            ModernTableRow::success(vec![
                ctx.loc.t("cli.compress.stage.encode"),
                ctx.loc.t("common.done"),
                ctx.loc.t("cli.compress.detail.encode"),
                format!(
                    "{} · {} {}",
                    result.format,
                    ctx.loc.t("common.level"),
                    result.level
                ),
            ]),
            ModernTableRow::success(vec![
                ctx.loc.t("cli.compress.stage.write"),
                ctx.loc.t("common.done"),
                ctx.loc.t("cli.compress.detail.write"),
                format!(
                    "{} · {}",
                    result.output_size,
                    create_volume_mode(ctx, result.volumes)
                ),
            ]),
            ModernTableRow::new(vec![
                ctx.loc.t("cli.compress.stage.verify"),
                ctx.loc.t("common.recommended"),
                ctx.loc.t("cli.compress.detail.verify"),
                format!("sqz test {}", result.output),
            ]),
        ],
    );
}

fn print_create_details(ctx: &Ctx, result: &CreateResultView<'_>) {
    let recovery_mode = recovery_mode_label(result.report);
    ctx.print_modern_table(
        &ctx.loc.t("cli.compress.details_title"),
        &[
            ModernTableColumn::new(ctx.loc.t("common.metric"), 24),
            ModernTableColumn::new(ctx.loc.t("common.value"), 24),
            ModernTableColumn::new(ctx.loc.t("common.detail"), 48),
        ],
        &[
            ModernTableRow::new(vec![
                ctx.loc.t("common.inputs"),
                result.input_count.to_string(),
                ctx.loc.t("cli.compress.detail.scan"),
            ]),
            ModernTableRow::new(vec![
                ctx.loc.t("common.format"),
                result.format.to_owned(),
                format!("{} {}", ctx.loc.t("common.level"), result.level),
            ]),
            ModernTableRow::new(vec![
                ctx.loc.t("cli.compress.metric.volume_mode"),
                create_volume_mode(ctx, result.volumes),
                format!("{}: {}", ctx.loc.t("common.volumes"), result.volumes),
            ]),
            ModernTableRow::success(vec![
                ctx.loc.t("common.output_size"),
                result.output_size.to_owned(),
                result.output.to_owned(),
            ]),
            ModernTableRow::new(vec![
                ctx.loc.t("cli.compress.metric.recovery_mode"),
                recovery_mode,
                ctx.loc.t("common.recommended"),
            ]),
        ],
    );
}

fn create_volume_mode(ctx: &Ctx, volumes: &str) -> String {
    if volumes == "1" {
        ctx.loc.t("common.single_file")
    } else {
        ctx.loc.t("common.split_volumes")
    }
}

fn create_format_label(ctx: &Ctx, output: &Path, requested_format: Option<&str>) -> String {
    if let Some(detected) = output
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| detected_format_label(ctx, name))
    {
        return detected;
    }
    if let Some(format) = requested_format {
        return format.to_ascii_lowercase();
    }
    "-".to_owned()
}

fn detected_format_label(ctx: &Ctx, name: &str) -> Option<String> {
    match ctx.engine.registry().detect_by_name(name)? {
        Detected::Archive(archive) => Some(archive.id().to_owned()),
        Detected::Compressed {
            compressor,
            inner_archive: Some(archive),
        } => Some(format!("{}.{}", archive.id(), compressor.id())),
        Detected::Compressed {
            compressor,
            inner_archive: None,
        } => Some(compressor.id().to_owned()),
    }
}

fn recovery_percent_label(recovery_percent: Option<u8>) -> String {
    match recovery_percent {
        Some(percent) => format!("{percent}%"),
        None => "-".to_owned(),
    }
}

fn recovery_mode_label(report: &CreateJsonReport) -> String {
    match report.inner_format.as_ref() {
        Some(inner) => {
            let recovery = recovery_percent_label(report.recovery_percent);
            format!("sqz · inner {inner} · recovery {recovery}")
        }
        None => "-".to_owned(),
    }
}

fn add_create_json_fields(value: &mut serde_json::Value, report: &CreateJsonReport) {
    let Some(object) = value.as_object_mut() else {
        return;
    };
    if let Some(inner_format) = &report.inner_format {
        object.insert("inner_format".into(), json!(inner_format));
    }
    if let Some(recovery_percent) = report.recovery_percent {
        object.insert("recovery_percent".into(), json!(recovery_percent));
    }
}

fn validate_requested_format(
    ctx: &Ctx,
    output: &Path,
    requested_format: Option<&str>,
) -> Result<(), CliError> {
    let Some(requested_format) = requested_format else {
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
    let requested_key = requested_format_key(ctx, requested_format).ok_or_else(|| {
        FormatError::Unsupported(format!("unsupported requested format: {requested_format}"))
    })?;
    if output_key != requested_key {
        return Err(FormatError::Unsupported(format!(
            "requested format '{requested_format}' does not match output path '{}'",
            output.display()
        ))
        .into());
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
