//! `sqz estimate`: expose the same create-input preflight used by the GUI.

use std::path::{Path, PathBuf};

use serde_json::json;
use squallz_core::api::FormatError;
use squallz_core::CreateInputEstimate;

use super::reports::print_pretty_json;
use crate::commands::{Ctx, ModernStatusField, ModernTableColumn, ModernTableRow};
use crate::errors::CliError;
use crate::progress::fmt_bytes;
use crate::ui::Tone;

struct DiskEstimate {
    path: String,
    required_bytes: u64,
    available_bytes: u64,
    ok: bool,
}

pub fn run(
    ctx: &Ctx,
    inputs: Vec<PathBuf>,
    excludes: Vec<String>,
    output: Option<PathBuf>,
    as_json: bool,
) -> Result<(), CliError> {
    let estimate = ctx.engine.estimate_create_inputs(&inputs, &excludes)?;
    let disk = output
        .as_deref()
        .map(|path| disk_preflight(path, estimate.output_budget_bytes()))
        .transpose()?;

    if as_json {
        print_json(&estimate, disk.as_ref())?;
    } else {
        print_human(ctx, &estimate, disk.as_ref());
    }
    Ok(())
}

fn disk_preflight(path: &Path, required_bytes: u64) -> Result<DiskEstimate, FormatError> {
    let dir = output_preflight_dir(path);
    let available_bytes = fs4::available_space(dir)?;
    Ok(DiskEstimate {
        path: path.display().to_string(),
        required_bytes,
        available_bytes,
        ok: available_bytes >= required_bytes,
    })
}

fn output_preflight_dir(path: &Path) -> &Path {
    if path.is_dir() {
        return path;
    }
    match path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        Some(parent) => parent,
        None => Path::new("."),
    }
}

fn estimate_status_key(disk: Option<&DiskEstimate>) -> &'static str {
    match disk {
        Some(disk) if disk.ok => "cli.estimate.disk.ok",
        Some(_) => "cli.estimate.disk.blocked",
        None => "common.done",
    }
}

fn print_json(estimate: &CreateInputEstimate, disk: Option<&DiskEstimate>) -> Result<(), CliError> {
    let mut value = json!({
        "input_count": estimate.input_count,
        "entries": estimate.entries,
        "files": estimate.files,
        "directories": estimate.directories,
        "symlinks": estimate.symlinks,
        "total_bytes": estimate.total_bytes,
        "output_budget_bytes": estimate.output_budget_bytes(),
    });
    if let Some(disk) = disk {
        value["disk"] = json!({
            "path": disk.path,
            "required_bytes": disk.required_bytes,
            "available_bytes": disk.available_bytes,
            "ok": disk.ok,
        });
    }
    print_pretty_json(&value)
}

fn print_human(ctx: &Ctx, estimate: &CreateInputEstimate, disk: Option<&DiskEstimate>) {
    if ctx.is_modern() {
        print_modern(ctx, estimate, disk);
        return;
    }

    let input_count = estimate.input_count.to_string();
    let entries = estimate.entries.to_string();
    let files = estimate.files.to_string();
    let directories = estimate.directories.to_string();
    let symlinks = estimate.symlinks.to_string();
    let total_bytes = estimate.total_bytes.to_string();
    let message = ctx.loc.format(
        "cli.estimate.summary",
        &[
            ("inputs", &input_count),
            ("entries", &entries),
            ("files", &files),
            ("dirs", &directories),
            ("symlinks", &symlinks),
            ("bytes", &total_bytes),
        ],
    );
    ctx.print_success(&message);
    if let Some(disk) = disk {
        let available = disk.available_bytes.to_string();
        let required = disk.required_bytes.to_string();
        let status = if disk.ok {
            ctx.loc.t("cli.estimate.disk.ok")
        } else {
            ctx.loc.t("cli.estimate.disk.blocked")
        };
        let message = ctx.loc.format(
            "cli.estimate.disk",
            &[
                ("path", &disk.path),
                ("available", &available),
                ("required", &required),
                ("status", &status),
            ],
        );
        if disk.ok {
            ctx.print_success(&message);
        } else {
            ctx.eprint_problem(&message);
        }
    }
}

fn print_modern(ctx: &Ctx, estimate: &CreateInputEstimate, disk: Option<&DiskEstimate>) {
    let summary = ctx.loc.format(
        "cli.estimate.summary.modern",
        &[
            ("inputs", &estimate.input_count.to_string()),
            ("entries", &estimate.entries.to_string()),
            ("files", &estimate.files.to_string()),
            ("dirs", &estimate.directories.to_string()),
            ("symlinks", &estimate.symlinks.to_string()),
            ("bytes", &fmt_bytes(estimate.total_bytes)),
        ],
    );
    let tone = if disk.is_some_and(|disk| !disk.ok) {
        Tone::Warning
    } else {
        Tone::Success
    };
    let status = ctx.loc.t(estimate_status_key(disk));
    ctx.print_modern_status_panel(
        &ctx.loc.t("cli.estimate.heading"),
        &status,
        tone,
        &summary,
        &[
            ModernStatusField::new(ctx.loc.t("common.inputs"), estimate.input_count.to_string()),
            ModernStatusField::new(ctx.loc.t("common.entries"), estimate.entries.to_string()),
            ModernStatusField::new(
                ctx.loc.t("common.total_size"),
                fmt_bytes(estimate.total_bytes),
            ),
            ModernStatusField::new(
                ctx.loc.t("cli.estimate.output_budget_title"),
                fmt_bytes(estimate.output_budget_bytes()),
            ),
        ],
    );
    ctx.print_modern_table(
        &ctx.loc.t("cli.estimate.composition_title"),
        &[
            ModernTableColumn::new(ctx.loc.t("common.lane"), 20),
            ModernTableColumn::right(ctx.loc.t("common.count"), 10),
            ModernTableColumn::right(ctx.loc.t("common.size"), 14),
            ModernTableColumn::new(ctx.loc.t("common.detail"), 40),
        ],
        &[
            ModernTableRow::new(vec![
                ctx.loc.t("cli.estimate.row.input_roots"),
                estimate.input_count.to_string(),
                "-".to_owned(),
                ctx.loc.t("cli.estimate.detail.input_roots"),
            ]),
            ModernTableRow::new(vec![
                ctx.loc.t("cli.estimate.row.file_payload"),
                estimate.files.to_string(),
                fmt_bytes(estimate.total_bytes),
                ctx.loc.t("cli.estimate.detail.file_payload"),
            ]),
            ModernTableRow::new(vec![
                ctx.loc.t("cli.estimate.row.directories"),
                estimate.directories.to_string(),
                "-".to_owned(),
                ctx.loc.t("cli.estimate.detail.directories"),
            ]),
            ModernTableRow::new(vec![
                ctx.loc.t("cli.estimate.row.symlinks"),
                estimate.symlinks.to_string(),
                "-".to_owned(),
                ctx.loc.t("cli.estimate.detail.symlinks"),
            ]),
            ModernTableRow::new(vec![
                ctx.loc.t("cli.estimate.row.entries"),
                estimate.entries.to_string(),
                fmt_bytes(estimate.total_bytes),
                ctx.loc.t("cli.estimate.detail.entries"),
            ]),
        ],
    );
    let reserve = estimate
        .output_budget_bytes()
        .saturating_sub(estimate.total_bytes);
    ctx.print_modern_table(
        &ctx.loc.t("cli.estimate.output_budget_title"),
        &[
            ModernTableColumn::new(ctx.loc.t("common.lane"), 22),
            ModernTableColumn::right(ctx.loc.t("common.size"), 14),
            ModernTableColumn::new(ctx.loc.t("common.detail"), 58),
        ],
        &[
            ModernTableRow::new(vec![
                ctx.loc.t("cli.estimate.row.source_payload"),
                fmt_bytes(estimate.total_bytes),
                ctx.loc.t("cli.estimate.detail.source_payload"),
            ]),
            ModernTableRow::new(vec![
                ctx.loc.t("cli.estimate.row.safety_reserve"),
                fmt_bytes(reserve),
                ctx.loc.t("cli.estimate.detail.safety_reserve"),
            ]),
            ModernTableRow::success(vec![
                ctx.loc.t("cli.estimate.row.required_output"),
                fmt_bytes(estimate.output_budget_bytes()),
                ctx.loc.t("cli.estimate.detail.required_output"),
            ]),
        ],
    );
    if let Some(disk) = disk {
        let row = if disk.ok {
            ModernTableRow::success(vec![
                disk.path.clone(),
                fmt_bytes(disk.available_bytes),
                fmt_bytes(disk.required_bytes),
                ctx.loc.t("cli.estimate.disk.ok"),
            ])
        } else {
            ModernTableRow::warning(vec![
                disk.path.clone(),
                fmt_bytes(disk.available_bytes),
                fmt_bytes(disk.required_bytes),
                ctx.loc.t("cli.estimate.disk.blocked"),
            ])
        };
        ctx.print_modern_table(
            &ctx.loc.t("cli.estimate.disk_title"),
            &[
                ModernTableColumn::new(ctx.loc.t("common.path"), 46),
                ModernTableColumn::right(ctx.loc.t("common.available"), 14),
                ModernTableColumn::right(ctx.loc.t("common.required"), 14),
                ModernTableColumn::new(ctx.loc.t("common.status"), 10),
            ],
            &[row],
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn disk_estimate(ok: bool) -> DiskEstimate {
        DiskEstimate {
            path: "planned.zip".to_owned(),
            required_bytes: 10,
            available_bytes: if ok { 20 } else { 5 },
            ok,
        }
    }

    #[test]
    fn output_preflight_dir_uses_parent_or_current_dir_for_file_outputs() {
        assert_eq!(
            output_preflight_dir(Path::new("planned.zip")),
            Path::new(".")
        );
        assert_eq!(
            output_preflight_dir(Path::new("nested/planned.zip")),
            Path::new("nested")
        );
    }

    #[test]
    fn output_preflight_dir_keeps_existing_directory_outputs() {
        let dir = std::env::temp_dir();
        assert_eq!(output_preflight_dir(&dir), dir.as_path());
    }

    #[test]
    fn estimate_status_key_tracks_disk_state_and_absent_preflight() {
        let ok = disk_estimate(true);
        let blocked = disk_estimate(false);

        assert_eq!(estimate_status_key(Some(&ok)), "cli.estimate.disk.ok");
        assert_eq!(
            estimate_status_key(Some(&blocked)),
            "cli.estimate.disk.blocked"
        );
        assert_eq!(estimate_status_key(None), "common.done");
    }
}
