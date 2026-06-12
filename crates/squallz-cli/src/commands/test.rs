//! `sqz test`: archive integrity test (human-readable or `--json` report).

use std::path::{Path, PathBuf};

use squallz_core::api::{OpenOptions, Password, TestReport};

use crate::commands::{
    reports::{print_pretty_json, test_report_json},
    Ctx, ModernStatusField, ModernTableColumn, ModernTableRow,
};
use crate::errors::CliError;
use crate::progress::CliProgress;
use crate::prompt::with_password_retry;
use crate::ui::Tone;

/// Exit code for a failed integrity test (= CorruptArchive).
const EXIT_CORRUPT: i32 = 3;

fn entry_count_label(report: &TestReport) -> String {
    report.entries_tested.to_string()
}

fn problem_count_label(report: &TestReport) -> String {
    report.problems.len().to_string()
}

fn archive_label(path: &Path) -> String {
    path.display().to_string()
}

fn test_exit_result(report: &TestReport) -> Result<(), CliError> {
    if report.is_ok() {
        Ok(())
    } else {
        Err(CliError::Exit(EXIT_CORRUPT))
    }
}

fn problem_rows(report: &TestReport) -> Vec<ModernTableRow> {
    report
        .problems
        .iter()
        .enumerate()
        .map(|(idx, problem)| ModernTableRow::danger(vec![(idx + 1).to_string(), problem.clone()]))
        .collect()
}

pub fn run(
    ctx: &Ctx,
    archive: PathBuf,
    password: Option<String>,
    encoding: Option<String>,
    json: bool,
) -> Result<(), CliError> {
    let progress = CliProgress::new_for_operation(
        ctx.quiet,
        ctx.verbose,
        json,
        ctx.output_style,
        ctx.color,
        ctx.accent,
        "test",
    );
    let explicit = password.map(Password::new);
    let report = with_password_retry(&ctx.loc, explicit.as_ref(), |pw| {
        ctx.engine.test(
            &archive,
            &OpenOptions {
                password: pw.cloned(),
                encoding_override: encoding.clone(),
            },
            &progress,
            &ctx.ctl,
        )
    });
    progress.finish();
    let report = report?;

    if json {
        let value = test_report_json(&report);
        print_pretty_json(&value)?;
        return test_exit_result(&report);
    }

    let entry_count = entry_count_label(&report);
    let problem_count = problem_count_label(&report);
    let archive_name = archive_label(&archive);

    if report.is_ok() {
        let message = ctx.loc.format("cli.test.ok", &[("count", &entry_count)]);
        if ctx.is_modern() {
            ctx.print_modern_status_panel(
                &ctx.loc.t("cli.test.result_title"),
                &ctx.loc.t("common.done"),
                Tone::Success,
                &message,
                &[
                    ModernStatusField::new(ctx.loc.t("common.entries"), entry_count.clone()),
                    ModernStatusField::new(ctx.loc.t("common.problems"), "0"),
                ],
            );
            ctx.print_modern_table(
                &ctx.loc.t("cli.test.result_title"),
                &[
                    ModernTableColumn::new(ctx.loc.t("common.status"), 24),
                    ModernTableColumn::right(ctx.loc.t("common.entries"), 10),
                    ModernTableColumn::right(ctx.loc.t("common.problems"), 10),
                    ModernTableColumn::new(ctx.loc.t("common.archive"), 50),
                ],
                &[ModernTableRow::success(vec![
                    message,
                    entry_count,
                    "0".to_owned(),
                    archive_name,
                ])],
            );
        } else {
            ctx.print_success(&message);
        }
        Ok(())
    } else {
        for problem in &report.problems {
            let message = ctx.loc.format("cli.test.problem", &[("detail", problem)]);
            ctx.eprint_problem(&message);
        }
        let message = ctx
            .loc
            .format("cli.test.failed", &[("count", &problem_count)]);
        if ctx.is_modern() {
            ctx.print_modern_status_panel(
                &ctx.loc.t("cli.test.failed_title"),
                &ctx.loc.t("common.failed"),
                Tone::Danger,
                &format!("{message} · {archive_name}"),
                &[
                    ModernStatusField::new(ctx.loc.t("common.entries"), entry_count.clone()),
                    ModernStatusField::new(ctx.loc.t("common.problems"), problem_count.clone()),
                ],
            );
            ctx.print_modern_table(
                &ctx.loc.t("cli.test.failed_title"),
                &[
                    ModernTableColumn::new(ctx.loc.t("common.status"), 24),
                    ModernTableColumn::right(ctx.loc.t("common.entries"), 10),
                    ModernTableColumn::right(ctx.loc.t("common.problems"), 10),
                    ModernTableColumn::new(ctx.loc.t("common.archive"), 50),
                ],
                &[ModernTableRow::danger(vec![
                    message,
                    entry_count,
                    problem_count,
                    archive_name,
                ])],
            );
            let rows = problem_rows(&report);
            ctx.print_modern_wrapped_table(
                &ctx.loc.t("cli.test.problems_title"),
                &[
                    ModernTableColumn::right(ctx.loc.t("common.id"), 4),
                    ModernTableColumn::new(ctx.loc.t("common.detail"), 92),
                ],
                &rows,
            );
        } else {
            ctx.eprint_problem(&message);
        }
        Err(CliError::Exit(EXIT_CORRUPT))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(entries_tested: u64, problems: &[&str]) -> TestReport {
        TestReport {
            entries_tested,
            problems: problems
                .iter()
                .map(|problem| (*problem).to_owned())
                .collect(),
            recovery: None,
        }
    }

    #[test]
    fn test_exit_result_maps_integrity_failure_to_corrupt_exit() {
        assert!(test_exit_result(&report(2, &[])).is_ok());

        match test_exit_result(&report(2, &["checksum mismatch"])) {
            Err(CliError::Exit(code)) => assert_eq!(code, EXIT_CORRUPT),
            _ => panic!("expected corrupt exit"),
        }
    }

    #[test]
    fn problem_rows_are_one_based_and_danger_toned() {
        let rows = problem_rows(&report(3, &["bad crc", "missing tail"]));

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].cells, vec!["1", "bad crc"]);
        assert_eq!(rows[0].tone, Tone::Danger);
        assert_eq!(rows[1].cells, vec!["2", "missing tail"]);
        assert_eq!(rows[1].tone, Tone::Danger);
    }
}
