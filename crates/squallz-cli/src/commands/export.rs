//! `sqz export`: convert a `.sqz` container back into a standard archive.
//! This is intentionally a named command instead of only `convert` so users
//! can see that SQZ is not a lock-in format.

use std::path::{Path, PathBuf};

use serde_json::json;
use squallz_core::api::{CompressionLevel, CreateOptions, FormatError, OpenOptions, Password};
use squallz_core::is_sqz_archive_path;

use super::reports::print_pretty_json;
use crate::args::resource_options;
use crate::commands::{
    detected_format_label, memory_limit_label, threads_label, Ctx, ModernStatusField,
    ModernTableColumn, ModernTableRow,
};
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
    force: bool,
    json_output: bool,
) -> Result<(), CliError> {
    if !is_sqz_archive_path(&archive) {
        return Err(
            FormatError::Unsupported("export expects a .sqz source container".into()).into(),
        );
    }
    if is_sqz_archive_path(&output) {
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
    let commit_policy = match super::create_commit_policy(
        &output,
        squallz_core::CreateArtifactKind::Archive,
        force,
        &progress,
        &ctx.ctl,
    ) {
        Ok(policy) => policy,
        Err(error) => {
            progress.finish();
            return Err(error.into());
        }
    };
    let result = ctx.engine.convert_with_policy(
        &archive,
        &output,
        &OpenOptions::default(),
        &create_opts,
        commit_policy,
        &progress,
        &ctx.ctl,
    );
    progress.finish();
    result?;
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
        assert!(is_sqz_archive_path(Path::new("archive.sqz")));
        assert!(is_sqz_archive_path(Path::new("archive.sqz.001")));
        assert!(is_sqz_archive_path(Path::new("ARCHIVE.SQZ")));
        assert!(!is_sqz_archive_path(Path::new("archive.zip")));
        assert!(!is_sqz_archive_path(Path::new("archive.zip.001")));
        assert!(!is_sqz_archive_path(Path::new("/")));
    }
}
