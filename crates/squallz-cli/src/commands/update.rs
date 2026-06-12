//! `sqz update`: append/delete/rename entries of an existing archive
//! (temp-file rewrite + atomic replacement happens in the format layer).

use std::path::{Path, PathBuf};

use serde_json::json;
use squallz_core::api::{CompressionLevel, CreateOptions, EntryPath, Password, UpdateOp};

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
    add: Vec<PathBuf>,
    mkdir: Vec<String>,
    delete: Vec<String>,
    rename: Vec<(String, String)>,
    move_entries: Vec<(String, String)>,
    excludes: Vec<String>,
    password: Option<String>,
    encrypt_names: bool,
    level: u8,
    threads: Option<usize>,
    memory_limit: Option<u64>,
    json_output: bool,
) -> Result<(), CliError> {
    let add_count = add.len();
    let mkdir_count = mkdir.len();
    let delete_count = delete.len();
    let rename_count = rename.len();
    let move_count = move_entries.len();
    let exclude_count = excludes.len();
    let mut ops: Vec<UpdateOp> = Vec::new();
    for path in add {
        let dest = add_dest_from_path(&path);
        ops.push(UpdateOp::Add {
            src: path,
            dest: EntryPath::from_utf8(dest),
        });
    }
    for path in mkdir {
        ops.push(UpdateOp::AddDir {
            path: EntryPath::from_utf8(path),
        });
    }
    for pattern in delete {
        ops.push(UpdateOp::Delete { pattern });
    }
    for (from, to) in rename {
        ops.push(UpdateOp::Rename {
            from: EntryPath::from_utf8(from),
            to: EntryPath::from_utf8(to),
        });
    }
    for (from, to) in move_entries {
        ops.push(UpdateOp::Rename {
            from: EntryPath::from_utf8(from),
            to: EntryPath::from_utf8(to),
        });
    }

    let opts = CreateOptions {
        level: CompressionLevel::from_numeric(level),
        password: password.map(Password::new),
        encrypt_filenames: encrypt_names,
        excludes,
        resources: resource_options(threads, memory_limit),
        ..CreateOptions::default()
    };
    let operation_count = ops.len();
    let progress = CliProgress::new_for_operation(
        ctx.quiet,
        ctx.verbose,
        json_output,
        ctx.output_style,
        ctx.color,
        ctx.accent,
        "update",
    );
    let result = ctx
        .engine
        .update(&archive, &ops, &opts, &progress, &ctx.ctl);
    progress.finish();
    result?;
    if json_output {
        let value = json!({
            "ok": true,
            "operation": "update",
            "archive": archive.display().to_string(),
            "operations": operation_count,
        });
        print_pretty_json(&value)?;
        return Ok(());
    }
    let path = archive.display().to_string();
    if ctx.is_modern() {
        ctx.print_modern_status_panel(
            &ctx.loc.t("cli.update.result_title"),
            &ctx.loc.t("common.done"),
            Tone::Success,
            &format!(
                "{operation_count} {} · {path}",
                ctx.loc.t("common.operations"),
            ),
            &[
                ModernStatusField::new(ctx.loc.t("common.operations"), operation_count.to_string()),
                ModernStatusField::new(
                    ctx.loc.t("cli.update.touched_entries"),
                    touched_count(
                        add_count,
                        mkdir_count,
                        delete_count,
                        rename_count,
                        move_count,
                    ),
                ),
                ModernStatusField::new(ctx.loc.t("common.archive"), path.clone()),
            ],
        );
        ctx.print_modern_table(
            &ctx.loc.t("cli.update.plan_title"),
            &[
                ModernTableColumn::new(ctx.loc.t("common.operation"), 22),
                ModernTableColumn::right(ctx.loc.t("common.count"), 8),
                ModernTableColumn::new(ctx.loc.t("common.detail"), 58),
            ],
            &[
                ModernTableRow::new(vec![
                    ctx.loc.t("cli.update.add_files"),
                    add_count.to_string(),
                    ctx.loc.t("cli.update.detail.add_files"),
                ]),
                ModernTableRow::new(vec![
                    ctx.loc.t("cli.update.create_dirs"),
                    mkdir_count.to_string(),
                    ctx.loc.t("cli.update.detail.create_dirs"),
                ]),
                ModernTableRow::new(vec![
                    ctx.loc.t("cli.update.delete_patterns"),
                    delete_count.to_string(),
                    ctx.loc.t("cli.update.detail.delete_patterns"),
                ]),
                ModernTableRow::new(vec![
                    ctx.loc.t("cli.update.rename_entries"),
                    rename_count.to_string(),
                    ctx.loc.t("cli.update.detail.rename_entries"),
                ]),
                ModernTableRow::new(vec![
                    ctx.loc.t("cli.update.move_entries"),
                    move_count.to_string(),
                    ctx.loc.t("cli.update.detail.move_entries"),
                ]),
                ModernTableRow::success(vec![
                    ctx.loc.t("common.operations"),
                    operation_count.to_string(),
                    ctx.loc.t("cli.update.detail.total"),
                ]),
            ],
        );
        ctx.print_modern_table(
            &ctx.loc.t("cli.update.policy_title"),
            &[
                ModernTableColumn::new(ctx.loc.t("common.setting"), 28),
                ModernTableColumn::new(ctx.loc.t("common.value"), 68),
            ],
            &[
                ModernTableRow::new(vec![ctx.loc.t("common.archive"), path]),
                ModernTableRow::new(vec![ctx.loc.t("common.level"), level.to_string()]),
                ModernTableRow::new(vec![
                    ctx.loc.t("cli.update.encrypt_names"),
                    yes_no(ctx, encrypt_names),
                ]),
                ModernTableRow::new(vec![
                    ctx.loc.t("common.exclude_patterns"),
                    exclude_count.to_string(),
                ]),
                ModernTableRow::new(vec![
                    ctx.loc.t("common.threads"),
                    threads_label(threads, &ctx.loc.t("common.auto")),
                ]),
                ModernTableRow::new(vec![
                    ctx.loc.t("common.memory_limit"),
                    memory_limit_label(memory_limit, &ctx.loc.t("common.auto")),
                ]),
            ],
        );
    } else {
        let message = ctx.loc.format("cli.update.done", &[("path", &path)]);
        ctx.print_success(&message);
    }
    Ok(())
}

fn add_dest_from_path(path: &Path) -> String {
    match path.file_name() {
        Some(name) => name.to_string_lossy().into_owned(),
        None => String::new(),
    }
}

fn threads_label(threads: Option<usize>, auto: &str) -> String {
    match threads {
        Some(threads) => threads.to_string(),
        None => auto.to_owned(),
    }
}

fn memory_limit_label(memory_limit: Option<u64>, auto: &str) -> String {
    match memory_limit {
        Some(memory_limit) => fmt_bytes(memory_limit),
        None => auto.to_owned(),
    }
}

fn touched_count(
    add_count: usize,
    mkdir_count: usize,
    delete_count: usize,
    rename_count: usize,
    move_count: usize,
) -> String {
    add_count
        .saturating_add(mkdir_count)
        .saturating_add(delete_count)
        .saturating_add(rename_count)
        .saturating_add(move_count)
        .to_string()
}

fn yes_no(ctx: &Ctx, value: bool) -> String {
    if value {
        ctx.loc.t("common.yes")
    } else {
        ctx.loc.t("common.no")
    }
}
