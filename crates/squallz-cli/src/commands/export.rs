//! `sqz export`: convert a `.sqz` container back into a standard archive.
//! This is intentionally a named command instead of only `convert` so users
//! can see that SQZ is not a lock-in format.

use std::path::{Path, PathBuf};

use serde_json::json;
use squallz_core::api::{
    split_volume_name, CompressionLevel, CreateOptions, Detected, FormatError, OpenOptions,
    Password,
};

use super::reports::print_pretty_json;
use crate::args::resource_options;
use crate::commands::{Ctx, ModernStatusField, ModernTableColumn, ModernTableRow};
use crate::errors::CliError;
use crate::progress::{fmt_bytes, CliProgress};
use crate::ui::Tone;

#[allow(clippy::too_many_arguments)] // direct image of the CLI surface
pub fn run(
    ctx: &Ctx,
    archive: PathBuf,
    output: PathBuf,
    level: u8,
    out_password: Option<String>,
    threads: Option<usize>,
    memory_limit: Option<u64>,
    json_output: bool,
) -> Result<(), CliError> {
    if !is_sqz_path(&archive) {
        return Err(
            FormatError::Unsupported("export expects a .sqz source container".into()).into(),
        );
    }
    if is_sqz_path(&output) {
        return Err(FormatError::Unsupported(
            "export output must be a standard archive, not .sqz".into(),
        )
        .into());
    }
    let progress = CliProgress::new_for_operation(
        ctx.quiet,
        ctx.verbose,
        json_output,
        ctx.output_style,
        ctx.color,
        ctx.accent,
        "export",
    );
    let destination_encrypted = out_password.is_some();
    let create_opts = CreateOptions {
        level: CompressionLevel::from_numeric(level),
        password: out_password.map(Password::new),
        resources: resource_options(threads, memory_limit),
        ..CreateOptions::default()
    };
    ctx.engine.convert(
        &archive,
        &output,
        &OpenOptions::default(),
        &create_opts,
        &progress,
        &ctx.ctl,
    )?;
    progress.finish();
    if json_output {
        let value = json!({
            "ok": true,
            "operation": "export_sqz",
            "archive": archive.display().to_string(),
            "output": output.display().to_string(),
        });
        print_pretty_json(&value)?;
        return Ok(());
    }
    let path = output.display().to_string();
    if ctx.is_modern() {
        let target_format = detected_format_label(ctx, &output);
        let output_size = output_size_label(&output);
        ctx.print_modern_status_panel(
            &ctx.loc.t("cli.export.result_title"),
            &ctx.loc.t("common.done"),
            Tone::Success,
            &format!("sqz → {target_format} · {output_size} · {path}"),
            &[
                ModernStatusField::new(
                    ctx.loc.t("common.format"),
                    format!("sqz → {target_format}"),
                ),
                ModernStatusField::new(ctx.loc.t("common.output_size"), output_size.clone()),
                ModernStatusField::new(ctx.loc.t("common.source"), archive.display().to_string()),
                ModernStatusField::new(ctx.loc.t("common.output"), path.clone()),
            ],
        );
        ctx.print_modern_table(
            &ctx.loc.t("cli.export.plan_title"),
            &[
                ModernTableColumn::new(ctx.loc.t("common.operation"), 16),
                ModernTableColumn::new(ctx.loc.t("common.format"), 16),
                ModernTableColumn::new(ctx.loc.t("common.path"), 64),
            ],
            &[
                ModernTableRow::new(vec![
                    ctx.loc.t("common.source"),
                    ctx.loc.t("cli.export.sqz_container"),
                    archive.display().to_string(),
                ]),
                ModernTableRow::success(vec![ctx.loc.t("common.output"), target_format, path]),
            ],
        );
        ctx.print_modern_table(
            &ctx.loc.t("cli.export.policy_title"),
            &[
                ModernTableColumn::new(ctx.loc.t("common.setting"), 28),
                ModernTableColumn::new(ctx.loc.t("common.value"), 68),
            ],
            &[
                ModernTableRow::new(vec![
                    ctx.loc.t("cli.export.lock_in"),
                    ctx.loc.t("cli.export.lock_in.none"),
                ]),
                ModernTableRow::new(vec![ctx.loc.t("common.level"), level.to_string()]),
                ModernTableRow::new(vec![
                    ctx.loc.t("cli.export.destination_encryption"),
                    yes_no(ctx, destination_encrypted),
                ]),
                ModernTableRow::new(vec![
                    ctx.loc.t("common.threads"),
                    threads_label(ctx, threads),
                ]),
                ModernTableRow::new(vec![
                    ctx.loc.t("common.memory_limit"),
                    memory_limit_label(ctx, memory_limit),
                ]),
                ModernTableRow::new(vec![ctx.loc.t("common.output_size"), output_size]),
            ],
        );
    } else {
        let message = ctx.loc.format("cli.export.done", &[("path", &path)]);
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

fn is_sqz_path(path: &Path) -> bool {
    if is_plain_sqz_path(path) {
        return true;
    }
    match split_sqz_base_name(path) {
        Some(base) => is_plain_sqz_path(Path::new(&base)),
        None => false,
    }
}

fn split_sqz_base_name(path: &Path) -> Option<String> {
    let name = path.file_name().and_then(|name| name.to_str())?;
    split_volume_name(name).map(|(base, _)| base.to_owned())
}

fn is_plain_sqz_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("sqz"))
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
    path.file_name()
        .and_then(|name| name.to_str())
        .map(ToOwned::to_owned)
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
    fn sqz_path_accepts_plain_and_split_sqz_sources_only() {
        assert!(is_sqz_path(Path::new("archive.sqz")));
        assert!(is_sqz_path(Path::new("archive.sqz.001")));
        assert!(is_sqz_path(Path::new("ARCHIVE.SQZ")));
        assert!(!is_sqz_path(Path::new("archive.zip")));
        assert!(!is_sqz_path(Path::new("archive.zip.001")));
        assert!(!is_sqz_path(Path::new("/")));
    }
}
