//! `sqz checksum`: compute local-file checksums.

use std::path::{Path, PathBuf};

use serde_json::json;
use squallz_core::{
    ChecksumAlgorithm, ChecksumReport, ChecksumVerificationItem, ChecksumVerificationReport,
};

use super::reports::print_pretty_json;
use crate::commands::{Ctx, ModernStatusField, ModernTableColumn, ModernTableRow};
use crate::errors::CliError;
use crate::progress::{fmt_bytes, CliProgress};
use crate::ui::Tone;

const EXIT_INTEGRITY: i32 = 3;

fn short_digest(value: &str) -> String {
    value.chars().take(16).collect()
}

fn optional_short_digest_or_dash(value: Option<&str>) -> String {
    match value {
        Some(digest) => short_digest(digest),
        None => "-".to_owned(),
    }
}

fn optional_problem_or_empty(value: Option<&str>) -> String {
    match value {
        Some(problem) => problem.to_owned(),
        None => String::new(),
    }
}

fn verification_row(item: &ChecksumVerificationItem) -> ModernTableRow {
    let row = vec![
        path_string(&item.path),
        if item.ok { "OK" } else { "FAILED" }.to_owned(),
        short_digest(&item.expected),
        optional_short_digest_or_dash(item.actual.as_deref()),
        optional_problem_or_empty(item.error.as_deref()),
    ];
    if item.ok {
        ModernTableRow::success(row)
    } else {
        ModernTableRow::danger(row)
    }
}

pub fn run(
    ctx: &Ctx,
    inputs: Vec<PathBuf>,
    algorithm: ChecksumAlgorithm,
    check: Option<PathBuf>,
    excludes: Vec<String>,
    as_json: bool,
) -> Result<(), CliError> {
    if let Some(manifest) = check {
        let progress = CliProgress::new_for_operation(
            ctx.quiet,
            ctx.verbose,
            as_json,
            ctx.output_style,
            ctx.color,
            ctx.accent,
            "checksum",
        );
        let report = ctx
            .engine
            .verify_checksum_manifest_with_progress(&manifest, algorithm, &progress, &ctx.ctl);
        progress.finish();
        let report = report?;
        if as_json {
            print_check_json(&report)?;
        } else if ctx.is_modern() {
            print_check_modern(ctx, &report);
        } else {
            print_check_classic(&report);
        }
        return if report.is_ok() {
            Ok(())
        } else {
            Err(CliError::Exit(EXIT_INTEGRITY))
        };
    }

    let progress = CliProgress::new_for_operation(
        ctx.quiet,
        ctx.verbose,
        as_json,
        ctx.output_style,
        ctx.color,
        ctx.accent,
        "checksum",
    );
    let report = ctx
        .engine
        .checksum_files_with_progress(&inputs, &excludes, algorithm, &progress, &ctx.ctl);
    progress.finish();
    let report = report?;

    if as_json {
        print_json(&report)?;
    } else if ctx.is_modern() {
        print_modern(ctx, &report, &excludes);
    } else {
        print_classic(&report);
    }
    Ok(())
}

fn print_json(report: &ChecksumReport) -> Result<(), CliError> {
    let value = json!({
        "ok": true,
        "operation": "checksum",
        "algorithm": report.algorithm.id(),
        "input_count": report.input_count,
        "entries_scanned": report.entries_scanned,
        "files_hashed": report.files_hashed,
        "bytes_hashed": report.bytes_hashed,
        "items": report.items.iter().map(|item| {
            json!({
                "path": path_string(&item.path),
                "size": item.size,
                "digest": item.digest,
            })
        }).collect::<Vec<_>>(),
    });
    print_pretty_json(&value)
}

fn print_check_json(report: &ChecksumVerificationReport) -> Result<(), CliError> {
    let value = json!({
        "ok": report.is_ok(),
        "operation": "checksum_check",
        "algorithm": report.algorithm.id(),
        "manifest": path_string(&report.manifest),
        "checked": report.checked,
        "passed": report.passed,
        "failed": report.failed,
        "bytes_hashed": report.bytes_hashed,
        "items": report.items.iter().map(|item| {
            json!({
                "path": path_string(&item.path),
                "expected": item.expected,
                "actual": item.actual,
                "ok": item.ok,
                "error": item.error,
            })
        }).collect::<Vec<_>>(),
    });
    print_pretty_json(&value)
}

fn print_classic(report: &ChecksumReport) {
    for item in &report.items {
        println!("{}  {}", item.digest, path_string(&item.path));
    }
}

fn print_check_classic(report: &ChecksumVerificationReport) {
    for item in &report.items {
        if item.ok {
            println!("{}: OK", path_string(&item.path));
        } else {
            println!("{}: FAILED", path_string(&item.path));
        }
    }
    if !report.is_ok() {
        eprintln!(
            "checksum: WARNING: {} of {} computed checksums did NOT match",
            report.failed, report.checked
        );
    }
}

fn print_modern(ctx: &Ctx, report: &ChecksumReport, excludes: &[String]) {
    let status = ctx.loc.t("common.done");
    let headline = ctx.loc.format(
        "cli.checksum.summary.modern",
        &[
            ("files", &report.files_hashed.to_string()),
            ("bytes", &fmt_bytes(report.bytes_hashed)),
            ("algorithm", report.algorithm.id()),
        ],
    );
    ctx.print_modern_status_panel(
        &ctx.loc.t("cli.checksum.heading"),
        &status,
        Tone::Success,
        &headline,
        &[
            ModernStatusField::new(ctx.loc.t("common.inputs"), report.input_count.to_string()),
            ModernStatusField::new(
                ctx.loc.t("common.entries"),
                report.entries_scanned.to_string(),
            ),
            ModernStatusField::new(ctx.loc.t("common.files"), report.files_hashed.to_string()),
            ModernStatusField::new(
                ctx.loc.t("common.total_size"),
                fmt_bytes(report.bytes_hashed),
            ),
            ModernStatusField::new(ctx.loc.t("cli.checksum.algorithm"), report.algorithm.id()),
            ModernStatusField::new(
                ctx.loc.t("cli.duplicates.excludes"),
                excludes.len().to_string(),
            ),
        ],
    );
    ctx.print_modern_table(
        &ctx.loc.t("cli.checksum.table_title"),
        &[
            ModernTableColumn::new(ctx.loc.t("common.path"), 38),
            ModernTableColumn::right(ctx.loc.t("common.size"), 10),
            ModernTableColumn::new(ctx.loc.t("cli.checksum.digest"), 64),
        ],
        &report
            .items
            .iter()
            .map(|item| {
                ModernTableRow::new(vec![
                    path_string(&item.path),
                    fmt_bytes(item.size),
                    item.digest.clone(),
                ])
            })
            .collect::<Vec<_>>(),
    );
}

fn print_check_modern(ctx: &Ctx, report: &ChecksumVerificationReport) {
    let tone = if report.is_ok() {
        Tone::Success
    } else {
        Tone::Danger
    };
    let status = if report.is_ok() {
        ctx.loc.t("common.done")
    } else {
        ctx.loc.t("common.failed")
    };
    let headline = ctx.loc.format(
        "cli.checksum.check_summary.modern",
        &[
            ("passed", &report.passed.to_string()),
            ("failed", &report.failed.to_string()),
            ("algorithm", report.algorithm.id()),
        ],
    );
    ctx.print_modern_status_panel(
        &ctx.loc.t("cli.checksum.check_heading"),
        &status,
        tone,
        &headline,
        &[
            ModernStatusField::new(ctx.loc.t("cli.checksum.algorithm"), report.algorithm.id()),
            ModernStatusField::new(ctx.loc.t("common.entries"), report.checked.to_string()),
            ModernStatusField::new(ctx.loc.t("cli.checksum.passed"), report.passed.to_string()),
            ModernStatusField::new(ctx.loc.t("common.failed"), report.failed.to_string()),
            ModernStatusField::new(
                ctx.loc.t("common.total_size"),
                fmt_bytes(report.bytes_hashed),
            ),
            ModernStatusField::new(ctx.loc.t("common.source"), path_string(&report.manifest)),
        ],
    );
    ctx.print_modern_table(
        &ctx.loc.t("cli.checksum.check_table_title"),
        &[
            ModernTableColumn::new(ctx.loc.t("common.path"), 38),
            ModernTableColumn::new(ctx.loc.t("common.status"), 10),
            ModernTableColumn::new(ctx.loc.t("cli.checksum.expected"), 18),
            ModernTableColumn::new(ctx.loc.t("cli.checksum.actual"), 18),
            ModernTableColumn::new(ctx.loc.t("common.problems"), 24),
        ],
        &report
            .items
            .iter()
            .map(verification_row)
            .collect::<Vec<_>>(),
    );
}

fn path_string(path: &Path) -> String {
    path.display().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verification_rows_use_success_and_danger_tones() {
        let ok = ChecksumVerificationItem {
            path: PathBuf::from("ok.bin"),
            expected: "0123456789abcdef0000".to_owned(),
            actual: Some("0123456789abcdef1111".to_owned()),
            ok: true,
            error: None,
        };
        let failed = ChecksumVerificationItem {
            path: PathBuf::from("missing.bin"),
            expected: "fedcba98765432100000".to_owned(),
            actual: None,
            ok: false,
            error: Some("missing file".to_owned()),
        };

        let ok_row = verification_row(&ok);
        assert_eq!(
            ok_row.cells,
            vec!["ok.bin", "OK", "0123456789abcdef", "0123456789abcdef", ""]
        );
        assert_eq!(ok_row.tone, Tone::Success);

        let failed_row = verification_row(&failed);
        assert_eq!(
            failed_row.cells,
            vec![
                "missing.bin",
                "FAILED",
                "fedcba9876543210",
                "-",
                "missing file"
            ]
        );
        assert_eq!(failed_row.tone, Tone::Danger);
    }
}
