//! `sqz extract`: extract an archive (optionally a `--include` selection)
//! with interactive password and overwrite-conflict handling. `--smart`
//! inspects the layout first: a single-root archive extracts directly,
//! loose entries are wrapped in a folder named after the archive.

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::json;
use squallz_core::api::{
    BoundedProblemLog, ConflictResolver, EntryPath, ExtractOptions, ExtractProblemReporter,
    ExtractReport, FormatError, OpenOptions, OverwritePolicy, Password, ProblemPreview,
    SymlinkPolicy,
};
use squallz_core::{ExtractPlan, PathFilter, SmartLayout};
use squallz_i18n::{localize_error, Localizer};

use crate::args::{resource_options, safety_limits, OverwriteArg, SymlinkArg};
use crate::commands::{Ctx, ModernStatusField, ModernTableColumn, ModernTableRow};
use crate::errors::CliError;
use crate::progress::{fmt_bytes, CliProgress};
use crate::prompt::{stdin_is_tty, with_password_retry, CliConflictResolver};
use crate::ui::Tone;

use super::reports::{
    empty_extract_counts_json, extract_counts_json, extract_plan_json, print_pretty_json,
};

pub(crate) struct CliExtractProblemReporter {
    loc: Arc<Localizer>,
    problems: BoundedProblemLog,
}

impl CliExtractProblemReporter {
    pub(crate) fn new(loc: Arc<Localizer>) -> Self {
        Self {
            loc,
            problems: BoundedProblemLog::default(),
        }
    }

    pub(crate) fn summary(&self) -> ProblemPreview {
        self.problems.snapshot()
    }
}

impl ExtractProblemReporter for CliExtractProblemReporter {
    fn skipped_entry(&self, path: &EntryPath, error: &FormatError) {
        let message = self.loc.format(
            "cli.extract.skipped_entry",
            &[
                ("path", &path.display),
                ("message", &localize_error(&self.loc, error)),
            ],
        );
        self.problems.record(message);
    }
}

struct ExtractRunOutcome {
    plan: ExtractPlan,
    report: Option<ExtractReport>,
}

#[allow(clippy::too_many_arguments)] // direct image of the CLI surface
pub fn run(
    ctx: &Ctx,
    archive: PathBuf,
    dest: Option<PathBuf>,
    includes: Vec<String>,
    overwrite: OverwriteArg,
    password: Option<String>,
    encoding: Option<String>,
    symlinks: SymlinkArg,
    smart: bool,
    best_effort: bool,
    threads: Option<usize>,
    memory_limit: Option<u64>,
    max_output_bytes: Option<u64>,
    max_entries: Option<u64>,
    max_compression_ratio: Option<u32>,
    json_output: bool,
) -> Result<(), CliError> {
    let dest = extract_dest_or_current(dest);
    let filter = PathFilter::new(&includes)?;

    // `ask` needs an interactive stdin; otherwise degrade to skip + warning.
    let mut overwrite: OverwritePolicy = overwrite.into();
    let mut resolver: Option<Arc<dyn ConflictResolver>> = None;
    if overwrite == OverwritePolicy::Ask {
        if stdin_is_tty() {
            resolver = Some(Arc::new(CliConflictResolver::new(Arc::clone(&ctx.loc))));
        } else {
            overwrite = OverwritePolicy::Skip;
            ctx.eprint_notice(ctx.loc.t("cli.overwrite.non_tty_skip"));
        }
    }
    let problem_reporter =
        best_effort.then(|| Arc::new(CliExtractProblemReporter::new(Arc::clone(&ctx.loc))));
    let x_opts = ExtractOptions {
        overwrite,
        resolver,
        symlinks: symlinks.into(),
        limits: safety_limits(max_output_bytes, max_entries, max_compression_ratio),
        resources: resource_options(threads, memory_limit),
        best_effort,
        problem_reporter: problem_reporter
            .as_ref()
            .map(|reporter| Arc::clone(reporter) as Arc<dyn ExtractProblemReporter>),
        ..ExtractOptions::default()
    };

    let progress = CliProgress::new_for_operation(
        ctx.quiet,
        ctx.verbose,
        json_output,
        ctx.output_style,
        ctx.color,
        ctx.accent,
        "extract",
    );
    let explicit = password.map(Password::new);
    let result = with_password_retry(&ctx.loc, explicit.as_ref(), |pw| {
        let open = OpenOptions {
            password: pw.cloned(),
            encoding_override: encoding.clone(),
        };
        let (plan, report) = ctx.engine.plan_and_extract_with_report_controlled(
            &archive,
            &dest,
            &archive,
            smart,
            &open,
            &x_opts,
            &progress,
            &ctx.ctl,
            |entries, control| filter.select_entries(entries, control),
            |_| Ok(()),
        )?;
        let no_match = !filter.is_empty() && plan.scope.entries == 0;
        if smart {
            match plan.layout {
                SmartLayout::DirectExtract => {
                    ctx.eprint_notice(ctx.loc.t("cli.extract.smart_direct"));
                }
                SmartLayout::WrapInFolder => {
                    let folder = ctx.engine.archive_stem(&archive);
                    let message = ctx
                        .loc
                        .format("cli.extract.smart_wrap", &[("folder", &folder)]);
                    ctx.eprint_notice(&message);
                }
            }
        }
        Ok(ExtractRunOutcome {
            plan,
            report: (!no_match).then_some(report),
        })
    });
    progress.finish();
    let outcome = result?;
    let Some(report) = outcome.report else {
        let path = outcome.plan.requested_destination.display().to_string();
        if json_output {
            let value = json!({
                "ok": true,
                "operation": "extract",
                "dest": path,
                "matched": false,
                "best_effort": best_effort,
                "skipped": 0,
                "problems": [],
                "problems_total": 0,
                "problems_truncated": false,
                "plan": extract_plan_json(&outcome.plan),
                "counts": empty_extract_counts_json(&outcome.plan.destination),
                "selected_entries": 0,
                "directories": 0,
                "output_bytes": 0,
            });
            print_pretty_json(&value)?;
            return Ok(());
        }
        if ctx.is_modern() {
            print_extract_no_match(ctx, &path, includes.len(), smart, best_effort);
        } else {
            ctx.eprint_notice(ctx.loc.t("cli.extract.no_match"));
        }
        return Ok(());
    };
    let path = report.destination.display().to_string();
    let problems = match problem_reporter.as_ref() {
        Some(reporter) => reporter.summary(),
        None => ProblemPreview::default(),
    };
    if json_output {
        let problems_truncated = problems.is_truncated();
        let value = json!({
            "ok": true,
            "operation": "extract",
            "dest": path,
            "matched": true,
            "best_effort": best_effort,
            "skipped": problems.total,
            "problems": problems.messages,
            "problems_total": problems.total,
            "problems_truncated": problems_truncated,
            "plan": extract_plan_json(&outcome.plan),
            "counts": extract_counts_json(&report),
            "selected_entries": report.selected_entries,
            "directories": report.directories,
            "output_bytes": report.output_bytes,
        });
        print_pretty_json(&value)?;
        return Ok(());
    }
    if ctx.is_modern() {
        let mode = if best_effort {
            ctx.loc.t("common.best_effort")
        } else {
            ctx.loc.t("common.strict")
        };
        let tone = if report.skipped == 0 && report.failed == 0 {
            Tone::Success
        } else {
            Tone::Warning
        };
        let archive_label = archive.display().to_string();
        let result = ExtractResultView {
            archive: &archive_label,
            mode: &mode,
            path: &path,
            tone,
            opts: &x_opts,
            include_count: includes.len(),
            smart,
            encoding_selected: encoding.is_some(),
            plan: &outcome.plan,
            report: &report,
        };
        print_extract_result(ctx, &result);
    } else {
        let message = ctx.loc.format("cli.extract.done", &[("path", &path)]);
        ctx.print_success(&message);
    }
    if problem_reporter.is_some() && problems.total > 0 {
        let count = problems.total.to_string();
        let message = ctx
            .loc
            .format("cli.extract.best_effort_summary", &[("count", &count)]);
        ctx.eprint_notice(&message);
        if ctx.verbose {
            for problem in &problems.messages {
                ctx.eprint_problem(problem);
            }
            if problems.is_truncated() {
                let shown = problems.messages.len().to_string();
                let omitted = problems.omitted().to_string();
                let message = ctx.loc.format(
                    "cli.extract.best_effort_preview_truncated",
                    &[("shown", &shown), ("omitted", &omitted)],
                );
                ctx.eprint_notice(&message);
            }
        }
    }
    Ok(())
}

fn extract_dest_or_current(dest: Option<PathBuf>) -> PathBuf {
    match dest {
        Some(dest) => dest,
        None => PathBuf::from("."),
    }
}

struct ExtractResultView<'a> {
    archive: &'a str,
    mode: &'a str,
    path: &'a str,
    tone: Tone,
    opts: &'a ExtractOptions,
    include_count: usize,
    smart: bool,
    encoding_selected: bool,
    plan: &'a ExtractPlan,
    report: &'a ExtractReport,
}

fn print_extract_result(ctx: &Ctx, result: &ExtractResultView<'_>) {
    let scope = extract_scope_label(ctx, result.plan);
    ctx.print_modern_status_panel(
        &ctx.loc.t("cli.extract.result_title"),
        &ctx.loc.t("common.done"),
        result.tone,
        &format!("{} · {}", result.mode, result.path),
        &[
            ModernStatusField::new(ctx.loc.t("common.mode"), result.mode.to_owned()),
            ModernStatusField::new(ctx.loc.t("common.scope"), scope),
            ModernStatusField::new(ctx.loc.t("common.target"), result.path.to_owned()),
            ModernStatusField::new(
                ctx.loc.t("common.created"),
                result.report.created.to_string(),
            ),
            ModernStatusField::new(
                ctx.loc.t("common.skipped"),
                result.report.skipped.to_string(),
            ),
            ModernStatusField::new(
                ctx.loc.t("common.replaced"),
                result.report.replaced.to_string(),
            ),
            ModernStatusField::new(
                ctx.loc.t("common.renamed"),
                result.report.renamed.to_string(),
            ),
            ModernStatusField::new(ctx.loc.t("common.failed"), result.report.failed.to_string()),
        ],
    );
    print_extract_plan(ctx, result);
    let result_row = vec![
        ctx.loc.t("common.done"),
        result.report.created.to_string(),
        result.report.skipped.to_string(),
        result.report.replaced.to_string(),
        result.report.renamed.to_string(),
        result.report.failed.to_string(),
        result.path.to_owned(),
    ];
    let status_row = if result.report.skipped == 0 && result.report.failed == 0 {
        ModernTableRow::success(result_row)
    } else {
        ModernTableRow::warning(result_row)
    };
    ctx.print_modern_table(
        &ctx.loc.t("cli.extract.summary_title"),
        &[
            ModernTableColumn::new(ctx.loc.t("common.status"), 10),
            ModernTableColumn::right(ctx.loc.t("common.created"), 9),
            ModernTableColumn::right(ctx.loc.t("common.skipped"), 9),
            ModernTableColumn::right(ctx.loc.t("common.replaced"), 9),
            ModernTableColumn::right(ctx.loc.t("common.renamed"), 9),
            ModernTableColumn::right(ctx.loc.t("common.failed"), 9),
            ModernTableColumn::new(ctx.loc.t("common.destination"), 42),
        ],
        &[status_row],
    );
    ctx.print_modern_wrapped_table(
        &ctx.loc.t("cli.extract.route_title"),
        &[
            ModernTableColumn::new(ctx.loc.t("common.lane"), 14),
            ModernTableColumn::new(ctx.loc.t("common.operation"), 14),
            ModernTableColumn::new(ctx.loc.t("common.value"), 48),
            ModernTableColumn::new(ctx.loc.t("common.detail"), 32),
        ],
        &[
            ModernTableRow::new(vec![
                ctx.loc.t("common.source"),
                ctx.loc.t("common.archive"),
                result.archive.to_owned(),
                result.mode.to_owned(),
            ]),
            ModernTableRow::new(vec![
                ctx.loc.t("common.selection"),
                ctx.loc.t("common.entries"),
                extract_scope_label(ctx, result.plan),
                if result.smart {
                    ctx.loc.t("common.smart_layout")
                } else {
                    selection_label(ctx, result.include_count)
                },
            ]),
            ModernTableRow::new(vec![
                ctx.loc.t("common.status"),
                ctx.loc.t("common.overwrite"),
                overwrite_policy_label(ctx, result.opts.overwrite),
                safety_limits_label(result.opts),
            ]),
            ModernTableRow::with_tone(
                vec![
                    ctx.loc.t("common.target"),
                    ctx.loc.t("common.done"),
                    extract_counts_label(ctx, result.report),
                    result.path.to_owned(),
                ],
                result.tone,
            ),
        ],
    );
    ctx.print_modern_table(
        &ctx.loc.t("cli.extract.policy_title"),
        &[
            ModernTableColumn::new(ctx.loc.t("common.setting"), 24),
            ModernTableColumn::new(ctx.loc.t("common.value"), 68),
        ],
        &[
            ModernTableRow::new(vec![
                ctx.loc.t("common.selection"),
                selection_label(ctx, result.include_count),
            ]),
            ModernTableRow::new(vec![
                ctx.loc.t("common.smart_layout"),
                if result.smart {
                    ctx.loc.t("common.yes")
                } else {
                    ctx.loc.t("common.no")
                },
            ]),
            ModernTableRow::new(vec![
                ctx.loc.t("common.overwrite"),
                overwrite_policy_label(ctx, result.opts.overwrite),
            ]),
            ModernTableRow::new(vec![
                ctx.loc.t("common.symlinks"),
                symlink_policy_label(ctx, result.opts.symlinks),
            ]),
            ModernTableRow::new(vec![
                ctx.loc.t("common.encoding"),
                if result.encoding_selected {
                    ctx.loc.t("common.yes")
                } else {
                    ctx.loc.t("common.auto")
                },
            ]),
            ModernTableRow::new(vec![
                ctx.loc.t("common.safety_limits"),
                safety_limits_label(result.opts),
            ]),
        ],
    );
    print_extract_details(ctx, result);
}

fn print_extract_plan(ctx: &Ctx, result: &ExtractResultView<'_>) {
    ctx.print_modern_wrapped_table(
        &ctx.loc.t("cli.extract.plan_title"),
        &[
            ModernTableColumn::new(ctx.loc.t("common.stage"), 18),
            ModernTableColumn::new(ctx.loc.t("common.status"), 12),
            ModernTableColumn::new(ctx.loc.t("common.detail"), 45),
            ModernTableColumn::new(ctx.loc.t("common.destination"), 35),
        ],
        &[
            ModernTableRow::success(vec![
                ctx.loc.t("cli.extract.stage.open"),
                ctx.loc.t("common.done"),
                ctx.loc.t("cli.extract.detail.open"),
                result.archive.to_owned(),
            ]),
            ModernTableRow::success(vec![
                ctx.loc.t("cli.extract.stage.select"),
                ctx.loc.t("common.done"),
                extract_scope_label(ctx, result.plan),
                selection_label(ctx, result.include_count),
            ]),
            ModernTableRow::success(vec![
                ctx.loc.t("cli.extract.stage.policy"),
                ctx.loc.t("common.done"),
                ctx.loc.t("cli.extract.detail.policy"),
                format!(
                    "{} · {}",
                    overwrite_policy_label(ctx, result.opts.overwrite),
                    symlink_policy_label(ctx, result.opts.symlinks)
                ),
            ]),
            ModernTableRow::with_tone(
                vec![
                    ctx.loc.t("cli.extract.stage.write"),
                    ctx.loc.t("common.done"),
                    ctx.loc.t("cli.extract.detail.write"),
                    result.plan.destination.display().to_string(),
                ],
                result.tone,
            ),
        ],
    );
}

fn print_extract_details(ctx: &Ctx, result: &ExtractResultView<'_>) {
    ctx.print_modern_table(
        &ctx.loc.t("cli.extract.details_title"),
        &[
            ModernTableColumn::new(ctx.loc.t("common.metric"), 24),
            ModernTableColumn::new(ctx.loc.t("common.value"), 24),
            ModernTableColumn::new(ctx.loc.t("common.detail"), 48),
        ],
        &[
            ModernTableRow::new(vec![
                ctx.loc.t("common.scope"),
                extract_scope_label(ctx, result.plan),
                ctx.loc.t("cli.extract.detail.select"),
            ]),
            ModernTableRow::new(vec![
                ctx.loc.t("common.target"),
                result.plan.destination.display().to_string(),
                result.plan.requested_destination.display().to_string(),
            ]),
            ModernTableRow::new(vec![
                ctx.loc.t("common.size"),
                fmt_bytes(result.report.output_bytes),
                result.path.to_owned(),
            ]),
            ModernTableRow::new(vec![
                ctx.loc.t("common.overwrite"),
                overwrite_policy_label(ctx, result.opts.overwrite),
                ctx.loc.t("cli.extract.detail.policy"),
            ]),
            ModernTableRow::new(vec![
                ctx.loc.t("common.symlinks"),
                symlink_policy_label(ctx, result.opts.symlinks),
                ctx.loc.t("common.safety_limits"),
            ]),
            ModernTableRow::new(vec![
                ctx.loc.t("common.encoding"),
                if result.encoding_selected {
                    ctx.loc.t("common.yes")
                } else {
                    ctx.loc.t("common.auto")
                },
                safety_limits_label(result.opts),
            ]),
            ModernTableRow::with_tone(
                vec![
                    ctx.loc.t("common.created"),
                    result.report.created.to_string(),
                    ctx.loc.t("cli.extract.detail.write"),
                ],
                Tone::Success,
            ),
            ModernTableRow::with_tone(
                vec![
                    ctx.loc.t("common.skipped"),
                    result.report.skipped.to_string(),
                    overwrite_policy_label(ctx, result.opts.overwrite),
                ],
                if result.report.skipped == 0 {
                    Tone::Success
                } else {
                    Tone::Warning
                },
            ),
            ModernTableRow::with_tone(
                vec![
                    ctx.loc.t("common.replaced"),
                    result.report.replaced.to_string(),
                    ctx.loc.t("common.policy.overwrite"),
                ],
                Tone::Success,
            ),
            ModernTableRow::with_tone(
                vec![
                    ctx.loc.t("common.renamed"),
                    result.report.renamed.to_string(),
                    ctx.loc.t("common.policy.rename_both"),
                ],
                Tone::Success,
            ),
            ModernTableRow::with_tone(
                vec![
                    ctx.loc.t("common.failed"),
                    result.report.failed.to_string(),
                    ctx.loc.t("common.problems"),
                ],
                if result.report.failed == 0 {
                    Tone::Success
                } else {
                    Tone::Warning
                },
            ),
        ],
    );
}

fn print_extract_no_match(
    ctx: &Ctx,
    dest: &str,
    include_count: usize,
    smart: bool,
    best_effort: bool,
) {
    ctx.print_modern_status_panel(
        &ctx.loc.t("cli.extract.no_match_title"),
        &ctx.loc.t("common.skipped"),
        Tone::Warning,
        &ctx.loc.t("cli.extract.no_match"),
        &[
            ModernStatusField::new(
                ctx.loc.t("common.selection"),
                selection_label(ctx, include_count),
            ),
            ModernStatusField::new(ctx.loc.t("common.destination"), dest.to_owned()),
            ModernStatusField::new(
                ctx.loc.t("common.mode"),
                if best_effort {
                    ctx.loc.t("common.best_effort")
                } else {
                    ctx.loc.t("common.strict")
                },
            ),
        ],
    );
    ctx.print_modern_table(
        &ctx.loc.t("cli.extract.policy_title"),
        &[
            ModernTableColumn::new(ctx.loc.t("common.setting"), 24),
            ModernTableColumn::new(ctx.loc.t("common.value"), 68),
        ],
        &[
            ModernTableRow::warning(vec![
                ctx.loc.t("common.status"),
                ctx.loc.t("cli.extract.no_match"),
            ]),
            ModernTableRow::new(vec![
                ctx.loc.t("common.selection"),
                selection_label(ctx, include_count),
            ]),
            ModernTableRow::new(vec![ctx.loc.t("common.destination"), dest.to_owned()]),
            ModernTableRow::new(vec![
                ctx.loc.t("common.smart_layout"),
                if smart {
                    ctx.loc.t("common.yes")
                } else {
                    ctx.loc.t("common.no")
                },
            ]),
            ModernTableRow::new(vec![
                ctx.loc.t("common.mode"),
                if best_effort {
                    ctx.loc.t("common.best_effort")
                } else {
                    ctx.loc.t("common.strict")
                },
            ]),
        ],
    );
}

fn extract_scope_label(ctx: &Ctx, plan: &ExtractPlan) -> String {
    format!(
        "{} {} · {} {} · {} {} · {}",
        plan.scope.entries,
        ctx.loc.t("common.entries"),
        plan.scope.files,
        ctx.loc.t("common.files"),
        plan.scope.directories,
        ctx.loc.t("common.directories"),
        fmt_bytes(plan.scope.total_bytes),
    )
}

fn extract_counts_label(ctx: &Ctx, report: &ExtractReport) -> String {
    format!(
        "{} {} · {} {} · {} {} · {} {} · {} {}",
        ctx.loc.t("common.created"),
        report.created,
        ctx.loc.t("common.skipped"),
        report.skipped,
        ctx.loc.t("common.replaced"),
        report.replaced,
        ctx.loc.t("common.renamed"),
        report.renamed,
        ctx.loc.t("common.failed"),
        report.failed,
    )
}

fn selection_label(ctx: &Ctx, include_count: usize) -> String {
    if include_count == 0 {
        return ctx.loc.t("common.all_entries");
    }
    ctx.loc
        .format("common.patterns", &[("count", &include_count.to_string())])
}

fn overwrite_policy_label(ctx: &Ctx, policy: OverwritePolicy) -> String {
    let key = match policy {
        OverwritePolicy::Overwrite => "common.policy.overwrite",
        OverwritePolicy::Skip => "common.policy.skip",
        OverwritePolicy::RenameBoth => "common.policy.rename_both",
        OverwritePolicy::Ask => "common.policy.ask",
    };
    ctx.loc.t(key)
}

fn symlink_policy_label(ctx: &Ctx, policy: SymlinkPolicy) -> String {
    let key = match policy {
        SymlinkPolicy::Preserve => "common.symlink.preserve",
        SymlinkPolicy::Follow => "common.symlink.follow",
        SymlinkPolicy::Skip => "common.symlink.skip",
    };
    ctx.loc.t(key)
}

fn safety_limits_label(opts: &ExtractOptions) -> String {
    format!(
        "{} entries · {} · {}x",
        opts.limits.max_entries,
        fmt_bytes(opts.limits.max_output_bytes),
        opts.limits.max_compression_ratio,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_problem_reporter_keeps_an_exact_count_and_bounded_preview() {
        let reporter =
            CliExtractProblemReporter::new(Arc::new(Localizer::with_user_dir(Some("en-US"), None)));
        let path = EntryPath::from_utf8("damaged/item.bin");

        for index in 0..25 {
            reporter.skipped_entry(
                &path,
                &FormatError::CorruptArchive(format!("checksum mismatch {index}")),
            );
        }

        let summary = reporter.summary();
        assert_eq!(summary.total, 25);
        assert_eq!(summary.messages.len(), 20);
        assert!(summary.messages[0].contains("damaged/item.bin"));
        assert!(summary.is_truncated());
        assert_eq!(summary.omitted(), 5);
    }
}
