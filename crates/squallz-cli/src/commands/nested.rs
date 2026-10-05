//! `sqz nested`: operate on an archive entry that is itself an archive.

use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde_json::Value;
use squallz_core::api::{
    EntryMeta, EntryPath, ExtractReport, FormatError, OpenOptions, Password, ProgressPhase,
    ProgressSink, SafetyLimits,
};
use squallz_core::{PlaintextFile, PlaintextWorkspace};
use squallz_i18n::localize_error;

use crate::args::{resource_options, safety_limits, NestedCmd};
use crate::commands::{
    extract::{self, CliExtractRequest, CliExtractSource},
    list::{entry_json, print_modern_table, print_tree},
    reports::print_pretty_json,
    Ctx, ModernStatusField, ModernTableColumn, ModernTableRow,
};
use crate::errors::CliError;
use crate::progress::CliProgress;
use crate::prompt::with_password_retry;
use crate::ui::Tone;

const FALLBACK_NESTED_BASENAME: &str = "nested-archive";

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
        } => extract::run(
            ctx,
            CliExtractRequest {
                source: CliExtractSource::Nested {
                    archive,
                    entry,
                    password,
                    encoding,
                },
                dest,
                includes,
                overwrite,
                password: nested_password,
                encoding: nested_encoding,
                symlinks,
                smart,
                best_effort,
                resources: resource_options(threads, memory_limit),
                limits: safety_limits(max_output_bytes, max_entries, max_compression_ratio),
                json_output: json,
            },
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
    let progress = CliProgress::new_for_operation(ctx, json, "nested");
    let temp = extract_nested_archive_to_temp(
        ctx,
        &archive,
        &entry,
        OpenOptions {
            password: password.map(Password::new),
            encoding_override: encoding,
        },
        SafetyLimits::default(),
        &progress,
    )?;
    let explicit = nested_password.map(Password::new);
    let listing = with_password_retry(
        &ctx.loc,
        explicit.as_ref(),
        || {},
        |pw| {
            ctx.engine.list_archive(
                temp.path(),
                &OpenOptions {
                    password: pw.cloned(),
                    encoding_override: nested_encoding.clone(),
                },
                SafetyLimits::default().max_entries,
                &ctx.ctl,
            )
        },
    )
    .map_err(|error| public_nested_error(error, &temp, &safe_entry_basename(&entry)));
    close_nested_temp(ctx, temp, listing.is_err())?;
    let listing = listing?;
    let entries =
        crate::commands::list::filter_entries_for_search(listing.entries, search.as_deref());

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

pub(crate) fn print_extract_result(
    ctx: &Ctx,
    mode: &str,
    path: &str,
    tone: Tone,
    report: &ExtractReport,
) {
    ctx.print_modern_status_panel(
        &ctx.loc.t("cli.extract.result_title"),
        &ctx.loc.t("common.done"),
        tone,
        &format!("{mode} · {path}"),
        &[
            ModernStatusField::new(ctx.loc.t("common.mode"), mode.to_owned()),
            ModernStatusField::new(ctx.loc.t("common.skipped"), report.skipped.to_string()),
            ModernStatusField::new(ctx.loc.t("common.failed"), report.failed.to_string()),
        ],
    );
    let result_row = vec![
        ctx.loc.t("common.done"),
        mode.to_owned(),
        report.skipped.to_string(),
        report.failed.to_string(),
        path.to_owned(),
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
}

fn compressed_size_label(compressed_size: Option<u64>) -> String {
    match compressed_size {
        Some(size) => size.to_string(),
        None => "-".to_owned(),
    }
}

/// Requires a single matching metadata entry before copying. Readers exposing
/// repeated raw names cannot lend one occurrence's metadata limits to another
/// occurrence's byte stream.
fn find_nested_entry(
    outer: &mut dyn squallz_core::api::ArchiveReader,
    entry: &str,
    ctx: &Ctx,
) -> Result<EntryMeta, FormatError> {
    let requested = EntryPath::from_utf8(entry);
    let mut selected = None;
    for meta in outer.entries() {
        ctx.ctl.checkpoint()?;
        let meta = meta?;
        if meta.path.raw == requested.raw && selected.replace(meta).is_some() {
            return Err(FormatError::CorruptArchive(
                "nested archive entry has an ambiguous duplicate raw name".into(),
            ));
        }
    }
    ctx.ctl.checkpoint()?;
    selected.ok_or_else(|| FormatError::Other(format!("entry not found: {entry}")))
}

pub(crate) fn extract_nested_archive_to_temp(
    ctx: &Ctx,
    archive: &Path,
    entry: &str,
    options: OpenOptions,
    limits: SafetyLimits,
    progress: &CliProgress,
) -> Result<PlaintextFile, FormatError> {
    let result = with_password_retry(
        &ctx.loc,
        options.password.as_ref(),
        || progress.finish(),
        |password| {
            progress.on_phase(ProgressPhase::ArchiveOpen, true);
            let mut outer = ctx.engine.open(
                archive,
                &OpenOptions {
                    password: password.cloned(),
                    encoding_override: options.encoding_override.clone(),
                },
                &ctx.ctl,
            )?;
            let meta = find_nested_entry(outer.as_mut(), entry, ctx)?;
            let workspace =
                PlaintextWorkspace::create_in(&std::env::temp_dir()).map_err(|error| {
                    private_storage_error(ctx, error, "error.preview_workspace_unavailable")
                })?;
            let (temp, mut out) =
                workspace
                    .create_file(&safe_entry_basename(entry))
                    .map_err(|error| {
                        private_storage_error(ctx, error, "error.preview_workspace_unavailable")
                    })?;
            let copied = (|| -> Result<(), FormatError> {
                progress.on_phase(ProgressPhase::ExtractEntries, true);
                let read = outer.read_entry(&meta.path, &mut |reader| {
                    squallz_core::copy_archive_entry(
                        reader, &mut out, &meta, limits, progress, &ctx.ctl,
                    )?;
                    Ok(())
                });
                ctx.ctl.checkpoint()?;
                read?;
                out.flush()?;
                ctx.ctl.checkpoint()
            })();
            drop(out);
            if let Err(error) = copied {
                progress.finish();
                close_nested_temp(ctx, temp, true)?;
                return Err(error);
            }
            Ok(temp)
        },
    );
    progress.finish();
    result
}

fn private_storage_error(ctx: &Ctx, error: io::Error, message_key: &str) -> FormatError {
    FormatError::from(io::Error::new(error.kind(), ctx.loc.t(message_key)))
}

/// Cleanup failures keep the original operation's error kind when it already
/// failed. A successful operation must finish cleanup before printing success.
pub(crate) fn close_nested_temp(
    ctx: &Ctx,
    temp: PlaintextFile,
    operation_failed: bool,
) -> Result<(), FormatError> {
    let result = temp
        .close()
        .map_err(|error| private_storage_error(ctx, error, "cli.nested.cleanup_failed"));
    if operation_failed {
        if let Err(error) = result {
            ctx.eprint_problem(localize_error(&ctx.loc, &error));
        }
        Ok(())
    } else {
        result
    }
}

pub(crate) fn public_nested_error(
    error: FormatError,
    temp: &PlaintextFile,
    display: &str,
) -> FormatError {
    let error = error.with_public_path(&temp.path().to_string_lossy(), display);
    match temp.path().parent() {
        Some(parent) => error.with_public_path(&parent.to_string_lossy(), display),
        None => error,
    }
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

pub(crate) fn safe_entry_basename(entry_path: &str) -> String {
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
