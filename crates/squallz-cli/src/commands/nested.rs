//! `sqz nested`: operate on an archive entry that is itself an archive.

use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use squallz_core::api::{
    ConflictResolver, EntryPath, ExtractOptions, ExtractProblemReporter, ExtractReport,
    FormatError, OpenOptions, OverwritePolicy, Password, ProblemPreview,
};
use squallz_core::{ExtractPlan, PathFilter, SmartLayout};

use crate::args::{resource_options, safety_limits, NestedCmd, OverwriteArg, SymlinkArg};
use crate::commands::{
    extract::CliExtractProblemReporter,
    list::{entry_json, print_modern_table, print_tree},
    reports::{
        empty_extract_counts_json, extract_counts_json, extract_plan_json, print_pretty_json,
    },
    Ctx, ModernStatusField, ModernTableColumn, ModernTableRow,
};
use crate::errors::CliError;
use crate::progress::CliProgress;
use crate::prompt::{stdin_is_tty, with_password_retry, CliConflictResolver};
use crate::ui::Tone;

const FALLBACK_NESTED_BASENAME: &str = "nested-archive";
const MAX_NESTED_TEMP_ATTEMPTS: u64 = 64;
static NESTED_TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

struct NestedTempArchive {
    path: PathBuf,
}

struct NestedExtractRunOutcome {
    plan: ExtractPlan,
    report: Option<ExtractReport>,
}

impl NestedTempArchive {
    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for NestedTempArchive {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

pub fn run(ctx: &Ctx, cmd: NestedCmd) -> Result<(), CliError> {
    match cmd {
        NestedCmd::List {
            archive,
            entry,
            password,
            encoding,
            nested_password,
            nested_encoding,
            search,
            json,
            tree,
        } => list_nested(
            ctx,
            archive,
            entry,
            password,
            encoding,
            nested_password,
            nested_encoding,
            search,
            json,
            tree,
        ),
        NestedCmd::Extract {
            archive,
            entry,
            dest,
            includes,
            overwrite,
            password,
            encoding,
            nested_password,
            nested_encoding,
            symlinks,
            smart,
            best_effort,
            threads,
            memory_limit,
            max_output_bytes,
            max_entries,
            max_compression_ratio,
            json,
        } => extract_nested(
            ctx,
            archive,
            entry,
            dest,
            includes,
            overwrite,
            password,
            encoding,
            nested_password,
            nested_encoding,
            symlinks,
            smart,
            best_effort,
            threads,
            memory_limit,
            max_output_bytes,
            max_entries,
            max_compression_ratio,
            json,
        ),
    }
}

#[allow(clippy::too_many_arguments)]
fn list_nested(
    ctx: &Ctx,
    archive: PathBuf,
    entry: String,
    password: Option<String>,
    encoding: Option<String>,
    nested_password: Option<String>,
    nested_encoding: Option<String>,
    search: Option<String>,
    json: bool,
    tree: bool,
) -> Result<(), CliError> {
    let temp = extract_nested_archive_to_temp(ctx, archive, &entry, password, encoding)?;
    let explicit = nested_password.map(Password::new);
    let entries = with_password_retry(&ctx.loc, explicit.as_ref(), |pw| {
        ctx.engine.list(
            temp.path(),
            &OpenOptions {
                password: pw.cloned(),
                encoding_override: nested_encoding.clone(),
            },
        )
    })?;
    let entries = crate::commands::list::filter_entries_for_search(entries, search.as_deref());

    if json {
        let array: Vec<Value> = entries.iter().map(entry_json).collect();
        print_pretty_json(&Value::Array(array))?;
        return Ok(());
    }

    if tree {
        print_tree(&entries, ctx.is_modern());
        let count = entries.len().to_string();
        let message = ctx.loc.format("cli.list.total", &[("count", &count)]);
        ctx.print_success(&message);
        return Ok(());
    }

    if ctx.is_modern() {
        print_modern_table(ctx, &entries);
    } else {
        println!(
            "{:>12}  {:>12}  {}",
            ctx.loc.t("common.size"),
            ctx.loc.t("common.compressed"),
            ctx.loc.t("common.name"),
        );
        for e in &entries {
            let compressed = compressed_size_label(e.compressed_size);
            println!("{:>12}  {compressed:>12}  {}", e.size, e.path);
        }
    }
    let count = entries.len().to_string();
    let message = ctx.loc.format("cli.list.total", &[("count", &count)]);
    ctx.print_success(&message);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn extract_nested(
    ctx: &Ctx,
    archive: PathBuf,
    entry: String,
    dest: Option<PathBuf>,
    includes: Vec<String>,
    overwrite: OverwriteArg,
    password: Option<String>,
    encoding: Option<String>,
    nested_password: Option<String>,
    nested_encoding: Option<String>,
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
    let temp = extract_nested_archive_to_temp(ctx, archive, &entry, password, encoding)?;
    let dest = extract_dest_or_current(dest);
    let archive_display_path = PathBuf::from(safe_entry_basename(&entry));
    let filter = PathFilter::new(&includes)?;

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
        "nested",
    );
    let explicit = nested_password.map(Password::new);
    let result = with_password_retry(&ctx.loc, explicit.as_ref(), |pw| {
        let open = OpenOptions {
            password: pw.cloned(),
            encoding_override: nested_encoding.clone(),
        };
        let (plan, report) = ctx.engine.plan_and_extract_with_report_controlled(
            temp.path(),
            &dest,
            &archive_display_path,
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
                    let folder = ctx.engine.archive_stem(&archive_display_path);
                    let message = ctx
                        .loc
                        .format("cli.extract.smart_wrap", &[("folder", &folder)]);
                    ctx.eprint_notice(&message);
                }
            }
        }
        Ok(NestedExtractRunOutcome {
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
                "operation": "nested_extract",
                "dest": path,
                "matched": false,
                "best_effort": best_effort,
                "problems": [],
                "problems_total": 0,
                "problems_truncated": false,
                "plan": extract_plan_json(&outcome.plan),
                "counts": empty_extract_counts_json(&outcome.plan.destination),
            });
            print_pretty_json(&value)?;
            return Ok(());
        }
        ctx.eprint_notice(ctx.loc.t("cli.extract.no_match"));
        return Ok(());
    };
    let path = report.destination.display().to_string();
    let problems = reported_extract_problems(problem_reporter.as_ref());
    if json_output {
        let problems_truncated = problems.is_truncated();
        let value = json!({
            "ok": true,
            "operation": "nested_extract",
            "dest": path,
            "matched": true,
            "best_effort": best_effort,
            "problems": problems.messages,
            "problems_total": problems.total,
            "problems_truncated": problems_truncated,
            "plan": extract_plan_json(&outcome.plan),
            "counts": extract_counts_json(&report),
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
        ctx.print_modern_status_panel(
            &ctx.loc.t("cli.extract.result_title"),
            &ctx.loc.t("common.done"),
            tone,
            &format!("{mode} · {path}"),
            &[
                ModernStatusField::new(ctx.loc.t("common.mode"), mode.clone()),
                ModernStatusField::new(ctx.loc.t("common.skipped"), report.skipped.to_string()),
                ModernStatusField::new(ctx.loc.t("common.failed"), report.failed.to_string()),
            ],
        );
        let result_row = vec![
            ctx.loc.t("common.done"),
            mode,
            report.skipped.to_string(),
            report.failed.to_string(),
            path.clone(),
        ];
        let result_row = if report.skipped == 0 && report.failed == 0 {
            ModernTableRow::success(result_row)
        } else {
            ModernTableRow::warning(result_row)
        };
        ctx.print_modern_table(
            &ctx.loc.t("cli.extract.result_title"),
            &[
                ModernTableColumn::new(ctx.loc.t("common.status"), 12),
                ModernTableColumn::new(ctx.loc.t("common.mode"), 12),
                ModernTableColumn::right(ctx.loc.t("common.skipped"), 8),
                ModernTableColumn::right(ctx.loc.t("common.failed"), 8),
                ModernTableColumn::new(ctx.loc.t("common.destination"), 50),
            ],
            &[result_row],
        );
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

fn compressed_size_label(compressed_size: Option<u64>) -> String {
    match compressed_size {
        Some(size) => size.to_string(),
        None => "-".to_owned(),
    }
}

fn extract_dest_or_current(dest: Option<PathBuf>) -> PathBuf {
    match dest {
        Some(dest) => dest,
        None => PathBuf::from("."),
    }
}

fn reported_extract_problems(
    problem_reporter: Option<&Arc<CliExtractProblemReporter>>,
) -> ProblemPreview {
    match problem_reporter {
        Some(reporter) => reporter.summary(),
        None => ProblemPreview::default(),
    }
}

fn extract_nested_archive_to_temp(
    ctx: &Ctx,
    archive: PathBuf,
    entry: &str,
    password: Option<String>,
    encoding: Option<String>,
) -> Result<NestedTempArchive, CliError> {
    let explicit = password.map(Password::new);
    let path = with_password_retry(&ctx.loc, explicit.as_ref(), |pw| {
        let open = OpenOptions {
            password: pw.cloned(),
            encoding_override: encoding.clone(),
        };
        let mut outer = ctx.engine.open(&archive, &open)?;
        let mut nested = outer.read_entry(&EntryPath::from_utf8(entry))?;
        let (path, mut out) = create_nested_temp_file(entry)?;
        match std::io::copy(&mut nested, &mut out) {
            Ok(_) => Ok(path),
            Err(e) => {
                let err = FormatError::from(e);
                let _ = fs::remove_file(&path);
                Err(err)
            }
        }
    })?;
    Ok(NestedTempArchive { path })
}

fn nanos_since_epoch_or_zero(now: SystemTime) -> u128 {
    match now.duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_nanos(),
        Err(_) => 0,
    }
}

fn nested_temp_path(entry_path: &str, nonce: u64, attempt: u64) -> PathBuf {
    let stamp = nanos_since_epoch_or_zero(SystemTime::now());
    std::env::temp_dir().join(format!(
        "squallz-cli-nested-{}-{stamp}-{nonce}-{attempt}-{}",
        std::process::id(),
        safe_entry_basename(entry_path)
    ))
}

fn create_nested_temp_file(entry_path: &str) -> Result<(PathBuf, File), FormatError> {
    let nonce = NESTED_TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    for attempt in 0..MAX_NESTED_TEMP_ATTEMPTS {
        let path = nested_temp_path(entry_path, nonce, attempt);
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(file) => return Ok((path, file)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.into()),
        }
    }
    Err(FormatError::Other(format!(
        "cannot create unique nested archive temp file for {}",
        safe_entry_basename(entry_path)
    )))
}

fn entry_basename_or_fallback(entry_path: &str) -> &str {
    match entry_path
        .rsplit(['/', '\\'])
        .next()
        .filter(|name| !name.is_empty())
    {
        Some(name) => name,
        None => FALLBACK_NESTED_BASENAME,
    }
}

fn safe_entry_basename(entry_path: &str) -> String {
    let basename = entry_basename_or_fallback(entry_path);
    let safe: String = basename
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect();
    if safe.is_empty() {
        FALLBACK_NESTED_BASENAME.into()
    } else {
        safe
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn remove_created_temp(path: PathBuf) {
        match std::fs::remove_file(path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => panic!("cannot remove nested temp file: {e}"),
        }
    }

    #[test]
    fn nested_temp_files_are_unique_for_same_entry() {
        let mut seen = HashSet::new();
        let mut opened = Vec::new();
        for _ in 0..128 {
            let (temp, file) = create_nested_temp_file("dir/inner.zip").unwrap();
            assert!(temp
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default()
                .starts_with("squallz-cli-nested-"));
            assert!(seen.insert(temp.clone()), "duplicate temp path: {temp:?}");
            opened.push((temp, file));
        }

        for (temp, file) in opened {
            drop(file);
            remove_created_temp(temp);
        }
    }

    #[test]
    fn nested_temp_file_sanitizes_entry_names() {
        let (temp, file) = create_nested_temp_file("../dir/inner archive?.zip").unwrap();
        let name = temp
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_owned();
        drop(file);
        remove_created_temp(temp);
        assert!(name.starts_with("squallz-cli-nested-"));
        assert!(name.ends_with("inner_archive_.zip"));
        assert!(!name.contains('/'));
        assert!(!name.contains('\\'));
    }
}
