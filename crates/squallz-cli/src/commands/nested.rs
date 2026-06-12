//! `sqz nested`: operate on an archive entry that is itself an archive.

use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use squallz_core::api::{
    ConflictResolver, EntryMeta, EntryPath, ExtractOptions, ExtractProblemReporter, FormatError,
    OpenOptions, OverwritePolicy, Password,
};
use squallz_core::{analyze_extract_layout, PathFilter, SmartLayout};

use crate::args::{resource_options, safety_limits, NestedCmd, OverwriteArg, SymlinkArg};
use crate::commands::{
    extract::CliExtractProblemReporter,
    list::{entry_json, print_modern_table, print_tree},
    reports::print_pretty_json,
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
    let mut final_dest = dest.clone();
    let result = with_password_retry(&ctx.loc, explicit.as_ref(), |pw| {
        let open = OpenOptions {
            password: pw.cloned(),
            encoding_override: nested_encoding.clone(),
        };
        let entries = if smart || !filter.is_empty() {
            Some(ctx.engine.list(temp.path(), &open)?)
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
                    let folder = archive_stem_for_entry(ctx, &entry);
                    let message = ctx
                        .loc
                        .format("cli.extract.smart_wrap", &[("folder", &folder)]);
                    ctx.eprint_notice(&message);
                    final_dest = dest.join(folder);
                }
            }
        }
        ctx.engine.extract(
            temp.path(),
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
                "operation": "nested_extract",
                "dest": final_dest.display().to_string(),
                "matched": false,
                "best_effort": best_effort,
                "skipped": 0,
                "problems": [],
            });
            print_pretty_json(&value)?;
            return Ok(());
        }
        ctx.eprint_notice(ctx.loc.t("cli.extract.no_match"));
        return Ok(());
    }
    let path = final_dest.display().to_string();
    let problems = reported_extract_problems(problem_reporter.as_ref());
    if json_output {
        let value = json!({
            "ok": true,
            "operation": "nested_extract",
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
        ctx.print_modern_status_panel(
            &ctx.loc.t("cli.extract.result_title"),
            &ctx.loc.t("common.done"),
            tone,
            &format!("{mode} · {path}"),
            &[
                ModernStatusField::new(ctx.loc.t("common.mode"), mode.clone()),
                ModernStatusField::new(ctx.loc.t("common.skipped"), problems.len().to_string()),
            ],
        );
        ctx.print_modern_table(
            &ctx.loc.t("cli.extract.result_title"),
            &[
                ModernTableColumn::new(ctx.loc.t("common.status"), 12),
                ModernTableColumn::new(ctx.loc.t("common.mode"), 12),
                ModernTableColumn::right(ctx.loc.t("common.skipped"), 8),
                ModernTableColumn::new(ctx.loc.t("common.destination"), 58),
            ],
            &[ModernTableRow::success(vec![
                ctx.loc.t("common.done"),
                mode,
                problems.len().to_string(),
                path.clone(),
            ])],
        );
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

fn layout_entries(entries: Option<&[EntryMeta]>) -> &[EntryMeta] {
    match entries {
        Some(entries) => entries,
        None => &[],
    }
}

fn reported_extract_problems(
    problem_reporter: Option<&Arc<CliExtractProblemReporter>>,
) -> Vec<String> {
    match problem_reporter {
        Some(reporter) => reporter.problems(),
        None => Vec::new(),
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

fn archive_stem_for_entry(ctx: &Ctx, entry_path: &str) -> String {
    ctx.engine
        .archive_stem(Path::new(&safe_entry_basename(entry_path)))
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
