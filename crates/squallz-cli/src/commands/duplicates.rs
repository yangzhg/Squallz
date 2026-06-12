//! `sqz duplicates`: local duplicate-file detection.

use std::path::{Path, PathBuf};

use serde_json::json;
use squallz_core::{DuplicateGroup, DuplicateScanReport};

use super::reports::print_pretty_json;
use crate::commands::{Ctx, ModernStatusField, ModernTableColumn, ModernTableRow};
use crate::errors::CliError;
use crate::progress::fmt_bytes;
use crate::ui::Tone;

const HASH_PREVIEW_CHARS: usize = 16;

pub fn run(
    ctx: &Ctx,
    inputs: Vec<PathBuf>,
    excludes: Vec<String>,
    min_size: u64,
    as_json: bool,
) -> Result<(), CliError> {
    let report = ctx
        .engine
        .find_duplicate_files(&inputs, &excludes, min_size)?;

    if as_json {
        print_json(&report, min_size)?;
    } else if ctx.is_modern() {
        print_modern(ctx, &report, min_size, &excludes);
    } else {
        print_classic(ctx, &report, min_size);
    }
    Ok(())
}

fn print_json(report: &DuplicateScanReport, min_size: u64) -> Result<(), CliError> {
    let groups = report
        .groups
        .iter()
        .map(|group| {
            json!({
                "hash": group.hash,
                "hash_algorithm": "blake3",
                "size": group.size,
                "count": group.count(),
                "reclaimable_bytes": group.reclaimable_bytes(),
                "paths": group.paths.iter().map(|path| path_string(path)).collect::<Vec<_>>(),
            })
        })
        .collect::<Vec<_>>();
    let value = json!({
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
        "groups": groups,
    });
    print_pretty_json(&value)
}

fn print_classic(ctx: &Ctx, report: &DuplicateScanReport, min_size: u64) {
    println!(
        "duplicate groups: {}; duplicate files: {}; reclaimable: {}; scanned files: {}; min size: {}",
        report.duplicate_groups(),
        report.duplicate_files(),
        report.reclaimable_bytes(),
        report.files_scanned,
        min_size,
    );
    if report.groups.is_empty() {
        ctx.print_success(ctx.loc.t("cli.duplicates.none"));
        return;
    }
    println!(
        "{:<6} {:>12} {:>12} {:<16} Example",
        "Count", "Size", "Reclaimable", "Hash"
    );
    for group in &report.groups {
        println!(
            "{:<6} {:>12} {:>12} {:<16} {}",
            group.count(),
            group.size,
            group.reclaimable_bytes(),
            hash_preview(&group.hash),
            group_example_path(group),
        );
        for path in &group.paths {
            println!("  {}", path_string(path));
        }
    }
}

fn print_modern(ctx: &Ctx, report: &DuplicateScanReport, min_size: u64, excludes: &[String]) {
    let tone = if report.groups.is_empty() {
        Tone::Success
    } else {
        Tone::Warning
    };
    let status = if report.groups.is_empty() {
        ctx.loc.t("cli.duplicates.status.clean")
    } else {
        ctx.loc.t("cli.duplicates.status.found")
    };
    let headline = ctx.loc.format(
        "cli.duplicates.summary.modern",
        &[
            ("groups", &report.duplicate_groups().to_string()),
            ("files", &report.duplicate_files().to_string()),
            ("reclaimable", &fmt_bytes(report.reclaimable_bytes())),
        ],
    );
    ctx.print_modern_status_panel(
        &ctx.loc.t("cli.duplicates.heading"),
        &status,
        tone,
        &headline,
        &[
            ModernStatusField::new(ctx.loc.t("common.inputs"), report.input_count.to_string()),
            ModernStatusField::new(ctx.loc.t("common.files"), report.files_scanned.to_string()),
            ModernStatusField::new(
                ctx.loc.t("cli.duplicates.candidates"),
                report.candidate_files.to_string(),
            ),
            ModernStatusField::new(
                ctx.loc.t("common.total_size"),
                fmt_bytes(report.bytes_scanned),
            ),
            ModernStatusField::new(
                ctx.loc.t("cli.duplicates.hashed"),
                fmt_bytes(report.hashed_bytes),
            ),
            ModernStatusField::new(ctx.loc.t("cli.duplicates.min_size"), fmt_bytes(min_size)),
        ],
    );
    ctx.print_modern_table(
        &ctx.loc.t("cli.duplicates.scan_title"),
        &[
            ModernTableColumn::new(ctx.loc.t("common.metric"), 24),
            ModernTableColumn::right(ctx.loc.t("common.count"), 14),
            ModernTableColumn::right(ctx.loc.t("common.size"), 18),
            ModernTableColumn::new(ctx.loc.t("common.status"), 22),
        ],
        &[
            ModernTableRow::new(vec![
                ctx.loc.t("common.entries"),
                report.entries_scanned.to_string(),
                "-".to_owned(),
                ctx.loc.t("common.done"),
            ]),
            ModernTableRow::new(vec![
                ctx.loc.t("common.files"),
                report.files_scanned.to_string(),
                fmt_bytes(report.bytes_scanned),
                ctx.loc.t("common.done"),
            ]),
            ModernTableRow::new(vec![
                ctx.loc.t("cli.duplicates.candidates"),
                report.candidate_files.to_string(),
                fmt_bytes(report.hashed_bytes),
                "BLAKE3".to_owned(),
            ]),
            ModernTableRow::with_tone(
                vec![
                    ctx.loc.t("cli.duplicates.duplicates"),
                    report.duplicate_files().to_string(),
                    fmt_bytes(report.reclaimable_bytes()),
                    status.clone(),
                ],
                tone,
            ),
            ModernTableRow::new(vec![
                ctx.loc.t("cli.duplicates.excludes"),
                excludes.len().to_string(),
                "-".to_owned(),
                if excludes.is_empty() {
                    ctx.loc.t("common.no")
                } else {
                    ctx.loc.t("common.yes")
                },
            ]),
        ],
    );

    if report.groups.is_empty() {
        return;
    }

    ctx.print_modern_table(
        &ctx.loc.t("cli.duplicates.groups_title"),
        &[
            ModernTableColumn::right(ctx.loc.t("common.count"), 8),
            ModernTableColumn::right(ctx.loc.t("common.size"), 14),
            ModernTableColumn::right(ctx.loc.t("cli.duplicates.reclaimable"), 14),
            ModernTableColumn::new(ctx.loc.t("cli.duplicates.hash"), 18),
            ModernTableColumn::new(ctx.loc.t("cli.duplicates.example"), 42),
        ],
        &report
            .groups
            .iter()
            .map(group_row)
            .collect::<Vec<ModernTableRow>>(),
    );

    ctx.print_modern_table(
        &ctx.loc.t("cli.duplicates.paths_title"),
        &[
            ModernTableColumn::new(ctx.loc.t("cli.duplicates.group"), 8),
            ModernTableColumn::right(ctx.loc.t("common.size"), 14),
            ModernTableColumn::new(ctx.loc.t("common.path"), 72),
        ],
        &duplicate_path_rows(&report.groups),
    );
}

fn group_row(group: &DuplicateGroup) -> ModernTableRow {
    ModernTableRow::warning(vec![
        group.count().to_string(),
        fmt_bytes(group.size),
        fmt_bytes(group.reclaimable_bytes()),
        hash_preview(&group.hash),
        group_example_path(group),
    ])
}

fn duplicate_path_rows(groups: &[DuplicateGroup]) -> Vec<ModernTableRow> {
    let mut rows = Vec::new();
    for (idx, group) in groups.iter().enumerate() {
        for path in &group.paths {
            rows.push(ModernTableRow::new(vec![
                format!("#{}", idx + 1),
                fmt_bytes(group.size),
                path_string(path),
            ]));
        }
    }
    rows
}

fn group_example_path(group: &DuplicateGroup) -> String {
    group
        .paths
        .first()
        .map_or_else(String::new, |path| path_string(path))
}

fn hash_preview(hash: &str) -> String {
    hash.chars().take(HASH_PREVIEW_CHARS).collect()
}

fn path_string(path: &Path) -> String {
    path.display().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_preview_is_bounded_without_assuming_digest_length() {
        assert_eq!(hash_preview("abc123"), "abc123");
        assert_eq!(
            hash_preview("0123456789abcdef0123456789abcdef"),
            "0123456789abcdef"
        );
    }
}
