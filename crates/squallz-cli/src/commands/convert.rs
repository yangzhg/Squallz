//! `sqz convert`: convert an archive into another format, entry by entry,
//! without extracting to disk. `--password` decrypts the source,
//! `--out-password` encrypts the destination.

use std::path::{Path, PathBuf};

use serde_json::json;
use squallz_core::api::{
    split_volume_name, CompressionLevel, CreateOptions, Detected, OpenOptions, Password,
};

use super::reports::print_pretty_json;
use crate::args::resource_options;
use crate::commands::{Ctx, ModernStatusField, ModernTableColumn, ModernTableRow};
use crate::errors::CliError;
use crate::progress::{fmt_bytes, CliProgress};
use crate::prompt::with_password_retry;
use crate::ui::Tone;

#[allow(clippy::too_many_arguments)] // direct image of the CLI surface
pub fn run(
    ctx: &Ctx,
    src: PathBuf,
    output: PathBuf,
    password: Option<String>,
    out_password: Option<String>,
    encrypt_names: bool,
    level: u8,
    encoding: Option<String>,
    threads: Option<usize>,
    memory_limit: Option<u64>,
    json_output: bool,
) -> Result<(), CliError> {
    let progress = CliProgress::new_for_operation(
        ctx.quiet,
        ctx.verbose,
        json_output,
        ctx.output_style,
        ctx.color,
        ctx.accent,
        "convert",
    );
    let destination_encrypted = out_password.is_some();
    let create_opts = CreateOptions {
        level: CompressionLevel::from_numeric(level),
        password: out_password.map(Password::new),
        encrypt_filenames: encrypt_names,
        resources: resource_options(threads, memory_limit),
        ..CreateOptions::default()
    };
    let explicit = password.map(Password::new);
    let result = with_password_retry(&ctx.loc, explicit.as_ref(), |pw| {
        let open = OpenOptions {
            password: pw.cloned(),
            encoding_override: encoding.clone(),
        };
        ctx.engine
            .convert(&src, &output, &open, &create_opts, &progress, &ctx.ctl)
    });
    progress.finish();
    result?;
    if json_output {
        let value = json!({
            "ok": true,
            "operation": "convert",
            "source": src.display().to_string(),
            "output": output.display().to_string(),
        });
        print_pretty_json(&value)?;
        return Ok(());
    }
    let path = output.display().to_string();
    if ctx.is_modern() {
        let source_format = detected_format_label(ctx, &src);
        let target_format = detected_format_label(ctx, &output);
        let output_size = output_size_label(&output);
        ctx.print_modern_status_panel(
            &ctx.loc.t("cli.convert.result_title"),
            &ctx.loc.t("common.done"),
            Tone::Success,
            &format!("{source_format} → {target_format} · {output_size} · {path}"),
            &[
                ModernStatusField::new(
                    ctx.loc.t("common.format"),
                    format!("{source_format} → {target_format}"),
                ),
                ModernStatusField::new(ctx.loc.t("common.output_size"), output_size.clone()),
                ModernStatusField::new(ctx.loc.t("common.source"), src.display().to_string()),
                ModernStatusField::new(ctx.loc.t("common.output"), path.clone()),
            ],
        );
        ctx.print_modern_table(
            &ctx.loc.t("cli.convert.plan_title"),
            &[
                ModernTableColumn::new(ctx.loc.t("common.operation"), 14),
                ModernTableColumn::new(ctx.loc.t("common.format"), 16),
                ModernTableColumn::new(ctx.loc.t("common.path"), 66),
            ],
            &[
                ModernTableRow::new(vec![
                    ctx.loc.t("common.source"),
                    source_format,
                    src.display().to_string(),
                ]),
                ModernTableRow::success(vec![ctx.loc.t("common.output"), target_format, path]),
            ],
        );
        ctx.print_modern_table(
            &ctx.loc.t("cli.convert.policy_title"),
            &[
                ModernTableColumn::new(ctx.loc.t("common.setting"), 28),
                ModernTableColumn::new(ctx.loc.t("common.value"), 68),
            ],
            &[
                ModernTableRow::new(vec![ctx.loc.t("common.level"), level.to_string()]),
                ModernTableRow::new(vec![
                    ctx.loc.t("cli.convert.destination_encryption"),
                    yes_no(ctx, destination_encrypted),
                ]),
                ModernTableRow::new(vec![
                    ctx.loc.t("cli.convert.encrypt_names"),
                    yes_no(ctx, encrypt_names),
                ]),
                ModernTableRow::new(vec![
                    ctx.loc.t("cli.convert.threads"),
                    threads_label(ctx, threads),
                ]),
                ModernTableRow::new(vec![
                    ctx.loc.t("cli.convert.memory_limit"),
                    memory_limit_label(ctx, memory_limit),
                ]),
                ModernTableRow::new(vec![ctx.loc.t("common.output_size"), output_size]),
            ],
        );
    } else {
        let message = ctx.loc.format("cli.convert.done", &[("path", &path)]);
        ctx.print_success(&message);
    }
    Ok(())
}

fn threads_label(ctx: &Ctx, threads: Option<usize>) -> String {
    threads.map_or_else(|| ctx.loc.t("common.auto"), |threads| threads.to_string())
}

fn memory_limit_label(ctx: &Ctx, memory_limit: Option<u64>) -> String {
    memory_limit.map_or_else(|| ctx.loc.t("common.auto"), fmt_bytes)
}

fn detected_format_label(ctx: &Ctx, path: &Path) -> String {
    match detected_format_name(ctx, path) {
        Some(name) => name,
        None => "-".to_owned(),
    }
}

fn detected_format_name(ctx: &Ctx, path: &Path) -> Option<String> {
    let name = detect_name_for_path(path)?;
    match ctx.engine.registry().detect_by_name(&name)? {
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

fn detect_name_for_path(path: &Path) -> Option<String> {
    let name = path.file_name().and_then(|name| name.to_str())?;
    match split_volume_name(name) {
        Some((base, _)) => Some(base.to_owned()),
        None => Some(name.to_owned()),
    }
}

fn output_size_label(path: &Path) -> String {
    match std::fs::metadata(path) {
        Ok(metadata) => fmt_bytes(metadata.len()),
        Err(_) => "-".to_owned(),
    }
}

fn yes_no(ctx: &Ctx, value: bool) -> String {
    if value {
        ctx.loc.t("common.yes")
    } else {
        ctx.loc.t("common.no")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_name_handles_split_and_missing_file_names() {
        assert_eq!(
            detect_name_for_path(Path::new("archive.7z.001")).as_deref(),
            Some("archive.7z")
        );
        assert_eq!(
            detect_name_for_path(Path::new("plain.zip")).as_deref(),
            Some("plain.zip")
        );
        assert_eq!(detect_name_for_path(Path::new("/")), None);
    }
}
