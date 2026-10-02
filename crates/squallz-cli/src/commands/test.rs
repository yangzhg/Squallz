//! `sqz test`: archive integrity test (human-readable or `--json` report).

use std::path::PathBuf;

use squallz_core::api::{FormatError, OpenOptions, Password, SafetyLimits};

use crate::commands::{
    reports::{
        localized_test_problems, print_pretty_json, print_test_problems_with_structure,
        test_report_json_with_structure,
    },
    Ctx, ModernStatusField, ModernTableColumn, ModernTableRow,
};
use crate::errors::CliError;
use crate::progress::CliProgress;
use crate::prompt::with_password_retry;
use crate::ui::Tone;

/// Exit code for a failed integrity test (= CorruptArchive).
const EXIT_CORRUPT: i32 = 3;

pub fn run(
    ctx: &Ctx,
    archive: PathBuf,
    password: Option<String>,
    encoding: Option<String>,
    limits: SafetyLimits,
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
    let outcome = with_password_retry(&ctx.loc, explicit.as_ref(), |pw| {
        ctx.engine.test_summary_with_structure(
            &archive,
            &OpenOptions {
                password: pw.cloned(),
                encoding_override: encoding.clone(),
            },
            &limits,
            &progress,
            &ctx.ctl,
        )
    });
    progress.finish();
    if !json && matches!(&outcome, Err(FormatError::ResourceLimitExceeded(_))) {
        let limits_message = ctx.loc.format(
            "cli.test.safety_limits",
            &[
                ("bytes", &limits.max_output_bytes.to_string()),
                ("entries", &limits.max_entries.to_string()),
                ("ratio", &limits.max_compression_ratio.to_string()),
            ],
        );
        let next_step = ctx.loc.t("cli.test.safety_limit_next_step");
        for line in limits_message.lines().chain(next_step.lines()) {
            ctx.eprint_notice(line);
        }
    }
    let outcome = outcome?;
    let structure = outcome.structure;
    let report = outcome.into_summary();
    let exit_result = if report.is_ok() {
        Ok(())
    } else {
        Err(CliError::Exit(EXIT_CORRUPT))
    };

    if json {
        let value = test_report_json_with_structure(&report, structure);
        print_pretty_json(&value)?;
        return exit_result;
    }

    let entry_count = report.entries_tested.to_string();
    let problem_count = report.problems.total.to_string();
    let archive_name = archive.display().to_string();

    if exit_result.is_ok() {
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
    } else {
        print_test_problems_with_structure(ctx, &report, structure);
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
            let rows: Vec<_> = localized_test_problems(ctx, &report, structure)
                .into_iter()
                .enumerate()
                .map(|(idx, problem)| ModernTableRow::danger(vec![(idx + 1).to_string(), problem]))
                .collect();
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
    }
    exit_result
}
