//! `sqz extract`: extract an archive (optionally a `--include` selection)
//! with interactive password and overwrite-conflict handling. `--smart`
//! inspects the layout first: a single-root archive extracts directly,
//! loose entries are wrapped in a folder named after the archive.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};

use serde_json::json;
use squallz_core::api::{
    ConflictResolver, EntryMeta, EntryPath, ExtractOptions, ExtractProblemReporter, FormatError,
    OpenOptions, OverwritePolicy, Password, SymlinkPolicy,
};
use squallz_core::{analyze_extract_layout, PathFilter, SmartLayout};
use squallz_i18n::{localize_error, Localizer};

use crate::args::{resource_options, safety_limits, OverwriteArg, SymlinkArg};
use crate::commands::{Ctx, ModernStatusField, ModernTableColumn, ModernTableRow};
use crate::errors::CliError;
use crate::progress::{fmt_bytes, CliProgress};
use crate::prompt::{stdin_is_tty, with_password_retry, CliConflictResolver};
use crate::ui::Tone;

use super::reports::print_pretty_json;

fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

pub(crate) struct CliExtractProblemReporter {
    loc: Arc<Localizer>,
    problems: Mutex<Vec<String>>,
}

impl CliExtractProblemReporter {
    pub(crate) fn new(loc: Arc<Localizer>) -> Self {
        Self {
            loc,
            problems: Mutex::new(Vec::new()),
        }
    }

    pub(crate) fn problems(&self) -> Vec<String> {
        lock_unpoisoned(&self.problems).clone()
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
        lock_unpoisoned(&self.problems).push(message);
    }
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
    // The final destination (smart mode may add a wrapping folder); set
    // inside the closure, reported after it.
    let mut final_dest = dest.clone();
    // Returns false when --include patterns matched no entry.
    let result = with_password_retry(&ctx.loc, explicit.as_ref(), |pw| {
        let open = OpenOptions {
            password: pw.cloned(),
            encoding_override: encoding.clone(),
        };
        // --smart and --include both need the entry list up front.
        let entries = if smart || !filter.is_empty() {
            Some(ctx.engine.list(&archive, &open)?)
        } else {
            None
        };
        let selection: Option<Vec<EntryPath>> = if filter.is_empty() {
            None
        } else {
            entries.as_ref().map(|entries| {
                entries
                    .iter()
                    .filter(|e| filter.matches(&e.path.display))
                    .map(|e| e.path.clone())
                    .collect()
            })
        };
        if let Some(sel) = &selection {
            if sel.is_empty() {
                return Ok(false);
            }
        }
        final_dest = dest.clone();
        if smart {
            match analyze_extract_layout(layout_entries(entries.as_deref())) {
                SmartLayout::DirectExtract => {
                    ctx.eprint_notice(ctx.loc.t("cli.extract.smart_direct"));
                }
                SmartLayout::WrapInFolder => {
                    let folder = ctx.engine.archive_stem(&archive);
                    let message = ctx
                        .loc
                        .format("cli.extract.smart_wrap", &[("folder", &folder)]);
                    ctx.eprint_notice(&message);
                    final_dest = dest.join(folder);
                }
            }
        }
        ctx.engine.extract(
            &archive,
            &final_dest,
            selection.as_deref(),
            &open,
            &x_opts,
            &progress,
            &ctx.ctl,
        )?;
        Ok(true)
    });
    progress.finish();
    if !result? {
        if json_output {
            let value = json!({
                "ok": true,
                "operation": "extract",
                "dest": final_dest.display().to_string(),
                "matched": false,
                "best_effort": best_effort,
                "skipped": 0,
                "problems": [],
            });
            print_pretty_json(&value)?;
            return Ok(());
        }
        if ctx.is_modern() {
            print_extract_no_match(
                ctx,
                &final_dest.display().to_string(),
                includes.len(),
                smart,
                best_effort,
            );
        } else {
            ctx.eprint_notice(ctx.loc.t("cli.extract.no_match"));
        }
        return Ok(());
    }
    let path = final_dest.display().to_string();
    let problems = match problem_reporter.as_ref() {
        Some(reporter) => reporter.problems(),
        None => Vec::new(),
    };
    if json_output {
        let value = json!({
            "ok": true,
            "operation": "extract",
            "dest": path,
            "matched": true,
            "best_effort": best_effort,
            "skipped": problems.len(),
            "problems": problems,
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
        let tone = if problems.is_empty() {
            Tone::Success
        } else {
            Tone::Warning
        };
        let archive_label = archive.display().to_string();
        let result = ExtractResultView {
            archive: &archive_label,
            mode: &mode,
            path: &path,
            skipped: problems.len(),
            tone,
            opts: &x_opts,
            include_count: includes.len(),
            smart,
            encoding_selected: encoding.is_some(),
        };
        print_extract_result(ctx, &result);
    } else {
        let message = ctx.loc.format("cli.extract.done", &[("path", &path)]);
        ctx.print_success(&message);
    }
    if problem_reporter.is_some() && !problems.is_empty() {
        let count = problems.len().to_string();
        let message = ctx
            .loc
            .format("cli.extract.best_effort_summary", &[("count", &count)]);
        ctx.eprint_notice(&message);
        if ctx.verbose {
            for problem in problems {
                ctx.eprint_problem(&problem);
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

fn layout_entries(entries: Option<&[EntryMeta]>) -> &[EntryMeta] {
    match entries {
        Some(entries) => entries,
        None => &[],
    }
}

struct ExtractResultView<'a> {
    archive: &'a str,
    mode: &'a str,
    path: &'a str,
    skipped: usize,
    tone: Tone,
    opts: &'a ExtractOptions,
    include_count: usize,
    smart: bool,
    encoding_selected: bool,
}

fn print_extract_result(ctx: &Ctx, result: &ExtractResultView<'_>) {
    ctx.print_modern_status_panel(
        &ctx.loc.t("cli.extract.result_title"),
        &ctx.loc.t("common.done"),
        result.tone,
        &format!("{} · {}", result.mode, result.path),
        &[
            ModernStatusField::new(ctx.loc.t("common.mode"), result.mode.to_owned()),
            ModernStatusField::new(ctx.loc.t("common.skipped"), result.skipped.to_string()),
            ModernStatusField::new(ctx.loc.t("common.destination"), result.path.to_owned()),
        ],
    );
    print_extract_plan(ctx, result);
    let result_row = vec![
        ctx.loc.t("common.done"),
        result.mode.to_owned(),
        result.skipped.to_string(),
        result.path.to_owned(),
    ];
    let status_row = if result.skipped == 0 {
        ModernTableRow::success(result_row)
    } else {
        ModernTableRow::warning(result_row)
    };
    ctx.print_modern_table(
        &ctx.loc.t("cli.extract.summary_title"),
        &[
            ModernTableColumn::new(ctx.loc.t("common.status"), 12),
            ModernTableColumn::new(ctx.loc.t("common.mode"), 14),
            ModernTableColumn::right(ctx.loc.t("common.skipped"), 8),
            ModernTableColumn::new(ctx.loc.t("common.destination"), 58),
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
                selection_label(ctx, result.include_count),
                if result.smart {
                    ctx.loc.t("common.smart_layout")
                } else {
                    result.mode.to_owned()
                },
            ]),
            ModernTableRow::new(vec![
                ctx.loc.t("common.status"),
                ctx.loc.t("common.overwrite"),
                overwrite_policy_label(ctx, result.opts.overwrite),
                safety_limits_label(result.opts),
            ]),
            ModernTableRow::success(vec![
                ctx.loc.t("common.destination"),
                ctx.loc.t("common.skipped"),
                result.skipped.to_string(),
                result.path.to_owned(),
            ]),
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
                ctx.loc.t("cli.extract.detail.select"),
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
                    format!(
                        "{} · {} · {}",
                        result.path,
                        result.mode,
                        if result.smart {
                            ctx.loc.t("common.smart_layout")
                        } else {
                            format!("{} {}", ctx.loc.t("common.skipped"), result.skipped)
                        }
                    ),
                ],
                if result.skipped == 0 {
                    Tone::Success
                } else {
                    Tone::Warning
                },
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
                ctx.loc.t("common.selection"),
                selection_label(ctx, result.include_count),
                ctx.loc.t("cli.extract.detail.select"),
            ]),
            ModernTableRow::new(vec![
                ctx.loc.t("common.smart_layout"),
                if result.smart {
                    ctx.loc.t("common.yes")
                } else {
                    ctx.loc.t("common.no")
                },
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
                    ctx.loc.t("common.skipped"),
                    result.skipped.to_string(),
                    ctx.loc.t("common.problems"),
                ],
                if result.skipped == 0 {
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
