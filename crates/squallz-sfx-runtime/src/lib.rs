#![forbid(unsafe_code)]
//! Read-only runtime used by Squallz Windows and Linux self-extractors.

mod args;
mod output;
mod progress;
mod prompt;

use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use clap::error::ErrorKind;
use serde_json::Value;
use squallz_core::api::{
    ArchiveReader, ConflictResolver, ControlToken, EntryMeta, EntryType, ExtractOptions,
    FormatError, LimitsAccountant, OpenOptions, OverwritePolicy, Password, ProgressSink,
    SafetyLimits, SymlinkPolicy, TestSummary,
};
use squallz_core::{
    default_sfx_extract_destination, inspect_extract_space, verify_and_open_sfx_payload, Engine,
    SfxLayout, VerifiedSfxPayload,
};
use squallz_i18n::{localize_error, Localizer};

use args::{Mode, RuntimeArgs};
use output::{entry_json, error_json, exit_code, extract_json, safe_terminal_text, test_json};
use progress::RuntimeProgress;
use prompt::{stdin_is_tty, with_password_retry, RuntimeConflictResolver};

/// Returns whether `path` contains a Squallz single-file SFX footer.
pub fn probe(path: &Path) -> Result<bool, FormatError> {
    squallz_core::inspect_sfx(path)
        .map(|info| info.is_some_and(|info| info.layout == SfxLayout::SingleFile))
}

/// Runs the embedded archive payload. `args` excludes argv[0].
pub fn run(executable: &Path, args: &[OsString]) -> i32 {
    let explicit_language = args::explicit_language(args);
    let loc = Arc::new(Localizer::load(explicit_language.as_deref()));
    let parsed = match args::parse(args, &loc) {
        Ok(parsed) => parsed,
        Err(error) => return render_argument_error(error),
    };

    let ctl = ControlToken::new();
    let handler_ctl = Arc::clone(&ctl);
    let _ = ctrlc::set_handler(move || handler_ctl.cancel());
    let progress =
        RuntimeProgress::new(Arc::clone(&loc), parsed.quiet, parsed.verbose, parsed.json);
    let result = execute(executable, &parsed, &loc, &progress, &ctl);
    progress.finish();
    match result {
        Ok(code) => code,
        Err(error) => render_error(&error, &loc, parsed.json),
    }
}

/// Resolves the current executable and runs its embedded archive payload.
pub fn run_current(args: &[OsString]) -> i32 {
    match std::env::current_exe() {
        Ok(executable) => run(&executable, args),
        Err(error) => {
            let language = args::explicit_language(args);
            let loc = Localizer::load(language.as_deref());
            render_error(&FormatError::from(error), &loc, args::json_requested(args))
        }
    }
}

fn execute(
    executable: &Path,
    args: &RuntimeArgs,
    loc: &Arc<Localizer>,
    progress: &RuntimeProgress,
    ctl: &ControlToken,
) -> Result<i32, FormatError> {
    let resources = args.resources();
    let verified = verify_and_open_sfx_payload(executable, &resources, progress, ctl)?;
    progress.finish();
    let engine = Engine::new(squallz_formats::sfx_zip_registry());
    match args.mode {
        Mode::List => run_list(&engine, &verified, args, loc, progress, ctl),
        Mode::Test => run_test(&engine, &verified, args, loc, progress, ctl),
        Mode::Extract => run_extract(&engine, executable, &verified, args, loc, progress, ctl),
    }
}

fn run_list(
    engine: &Engine,
    verified: &VerifiedSfxPayload,
    args: &RuntimeArgs,
    loc: &Localizer,
    progress: &RuntimeProgress,
    ctl: &ControlToken,
) -> Result<i32, FormatError> {
    let entries = with_password_retry(
        loc,
        ctl,
        || progress.finish(),
        |password| {
            let mut reader = open_payload(engine, verified, password, ctl)?;
            collect_entries(&mut *reader, args.limits(), progress, ctl)
        },
    )?;
    progress.finish();
    if args.json {
        let value = Value::Array(entries.iter().map(entry_json).collect());
        output::write_json(&value)?;
    } else {
        print_list(loc, &entries)?;
    }
    Ok(0)
}

fn run_test(
    engine: &Engine,
    verified: &VerifiedSfxPayload,
    args: &RuntimeArgs,
    loc: &Localizer,
    progress: &RuntimeProgress,
    ctl: &ControlToken,
) -> Result<i32, FormatError> {
    let report = with_password_retry(
        loc,
        ctl,
        || progress.finish(),
        |password| {
            let mut reader = open_payload(engine, verified, password, ctl)?;
            reader.test_summary_with_limits(&args.limits(), progress, ctl)
        },
    )?;
    progress.finish();
    if args.json {
        output::write_json(&test_json(&report))?;
    } else {
        print_test(loc, &report)?;
    }
    Ok(if report.is_ok() { 0 } else { 3 })
}

#[allow(clippy::too_many_arguments)]
fn run_extract(
    engine: &Engine,
    executable: &Path,
    verified: &VerifiedSfxPayload,
    args: &RuntimeArgs,
    loc: &Arc<Localizer>,
    progress: &RuntimeProgress,
    ctl: &ControlToken,
) -> Result<i32, FormatError> {
    let destination = match &args.output {
        Some(path) => path.clone(),
        None => default_destination(executable, &std::env::current_dir()?),
    };
    let mut overwrite: OverwritePolicy = args.overwrite.into();
    let mut resolver: Option<Arc<dyn ConflictResolver>> = None;
    if overwrite == OverwritePolicy::Ask {
        if stdin_is_tty() {
            resolver = Some(Arc::new(RuntimeConflictResolver::new(
                Arc::clone(loc),
                ctl.clone(),
            )));
        } else {
            overwrite = OverwritePolicy::Skip;
            if !args.json {
                output::write_stderr(&format!("{}\n", loc.t("cli.overwrite.non_tty_skip")));
            }
        }
    }
    let extract_options = ExtractOptions {
        overwrite,
        resolver,
        symlinks: SymlinkPolicy::Skip,
        limits: args.limits(),
        resources: args.resources(),
        best_effort: false,
        ..ExtractOptions::default()
    };

    let (mut reader, entries) = with_password_retry(
        loc,
        ctl,
        || progress.finish(),
        |password| {
            let mut reader = open_payload(engine, verified, password, ctl)?;
            let entries = collect_entries(&mut *reader, args.limits(), progress, ctl)?;
            validate_encrypted_entries(&mut *reader, &entries, ctl)?;
            Ok((reader, entries))
        },
    )?;
    progress.finish();

    // The retry boundary ends before any destination path is created. Once
    // writing begins, an error is returned directly so a partial extraction
    // is never replayed over itself.
    let plan = engine.plan_extract_from_entries_with_control(
        &destination,
        executable,
        &entries,
        None,
        false,
        ctl,
    )?;
    if !inspect_extract_space(&plan)?.is_sufficient() {
        return Err(FormatError::DiskFull);
    }
    ctl.checkpoint()?;
    let report = extract_once(&mut *reader, &plan, &extract_options, progress, ctl)?;
    progress.finish();

    if args.json {
        output::write_json(&extract_json(&plan, &report))?;
    } else {
        let path = safe_terminal_text(&report.destination.display().to_string());
        let message = loc.format("cli.extract.done", &[("path", &path)]);
        output::write_stdout(&format!("{message}\n"))?;
    }
    Ok(0)
}

fn validate_encrypted_entries(
    reader: &mut dyn ArchiveReader,
    entries: &[EntryMeta],
    ctl: &ControlToken,
) -> Result<(), FormatError> {
    for entry in entries
        .iter()
        .filter(|entry| entry.encrypted && !matches!(entry.entry_type, EntryType::Dir))
    {
        ctl.checkpoint()?;
        let mut stream = reader.read_entry(&entry.path)?;
        let mut byte = [0u8; 1];
        let _ = stream.read(&mut byte)?;
    }
    Ok(())
}

fn extract_once(
    reader: &mut dyn ArchiveReader,
    plan: &squallz_core::ExtractPlan,
    options: &ExtractOptions,
    progress: &dyn ProgressSink,
    ctl: &ControlToken,
) -> Result<squallz_core::api::ExtractReport, FormatError> {
    reader.extract_with_report(&plan.destination, None, options, progress, ctl)
}

fn open_payload(
    engine: &Engine,
    verified: &VerifiedSfxPayload,
    password: Option<&Password>,
    ctl: &ControlToken,
) -> Result<Box<dyn ArchiveReader>, FormatError> {
    engine.open_verified_sfx_with_control(
        verified,
        &OpenOptions {
            password: password.cloned(),
            encoding_override: None,
        },
        ctl,
    )
}

fn collect_entries(
    reader: &mut dyn ArchiveReader,
    limits: SafetyLimits,
    progress: &dyn ProgressSink,
    ctl: &ControlToken,
) -> Result<Vec<EntryMeta>, FormatError> {
    let mut entries = Vec::new();
    let mut accountant = LimitsAccountant::new(limits);
    for item in reader.entries() {
        ctl.checkpoint()?;
        let entry = item?;
        accountant.check_entry(&entry)?;
        accountant.add_output_bytes(entry.size)?;
        progress.on_scan_progress(entries.len().saturating_add(1) as u64, &entry.path);
        entries.push(entry);
    }
    ctl.checkpoint()?;
    Ok(entries)
}

fn print_list(loc: &Localizer, entries: &[EntryMeta]) -> Result<(), FormatError> {
    let mut text = format!(
        "{:>12}  {:>12}  {}\n",
        loc.t("common.size"),
        loc.t("common.compressed"),
        loc.t("common.name")
    );
    for entry in entries {
        let compressed = entry
            .compressed_size
            .map_or_else(|| "-".to_owned(), |size| size.to_string());
        text.push_str(&format!(
            "{:>12}  {compressed:>12}  {}\n",
            entry.size,
            safe_terminal_text(&entry.path.display)
        ));
    }
    let count = entries.len().to_string();
    text.push_str(&loc.format("cli.list.total", &[("count", &count)]));
    text.push('\n');
    output::write_stdout(&text)
}

fn print_test(loc: &Localizer, report: &TestSummary) -> Result<(), FormatError> {
    let count = report.entries_tested.to_string();
    if report.is_ok() {
        let message = loc.format("cli.test.ok", &[("count", &count)]);
        output::write_stdout(&format!("{message}\n"))?;
        return Ok(());
    }
    for problem in &report.problems.messages {
        let detail = safe_terminal_text(problem);
        output::write_stderr(&format!(
            "{}\n",
            loc.format("cli.test.problem", &[("detail", &detail)])
        ));
    }
    if report.problems.is_truncated() {
        let shown = report.problems.messages.len().to_string();
        let omitted = report.problems.omitted().to_string();
        output::write_stderr(&format!(
            "{}\n",
            loc.format(
                "cli.test.problem_preview_truncated",
                &[("shown", &shown), ("omitted", &omitted)]
            )
        ));
    }
    let problems = report.problems.total.to_string();
    output::write_stderr(&format!(
        "{}\n",
        loc.format("cli.test.failed", &[("count", &problems)])
    ));
    Ok(())
}

fn default_destination(executable: &Path, current_dir: &Path) -> PathBuf {
    default_sfx_extract_destination(current_dir, executable)
}

fn render_argument_error(error: clap::Error) -> i32 {
    let kind = error.kind();
    let code = if matches!(kind, ErrorKind::DisplayHelp | ErrorKind::DisplayVersion) {
        0
    } else {
        error.exit_code()
    };
    let text = error.to_string();
    if matches!(kind, ErrorKind::DisplayHelp | ErrorKind::DisplayVersion) {
        if output::write_stdout(&text).is_err() {
            return 7;
        }
    } else {
        output::write_stderr(&safe_terminal_text(&text));
    }
    code
}

fn render_error(error: &FormatError, loc: &Localizer, json: bool) -> i32 {
    let code = exit_code(error);
    if json && output::write_json(&error_json(error, loc)).is_ok() {
        return code;
    }
    let message = safe_terminal_text(&localize_error(loc, error));
    let line = loc.format("cli.error_prefix", &[("message", &message)]);
    output::write_stderr(&format!("{line}\n"));
    code
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use squallz_core::api::{EntryPath, NoProgress, TestSummary};

    use super::*;

    struct FailingExtractReader {
        calls: Arc<AtomicUsize>,
    }

    impl ArchiveReader for FailingExtractReader {
        fn entries(&mut self) -> Box<dyn Iterator<Item = Result<EntryMeta, FormatError>> + '_> {
            Box::new(std::iter::empty())
        }

        fn read_entry(&mut self, _path: &EntryPath) -> Result<Box<dyn Read + '_>, FormatError> {
            Ok(Box::new(Cursor::new(Vec::<u8>::new())))
        }

        fn extract_with_report(
            &mut self,
            _dest: &Path,
            _selection: Option<&[EntryPath]>,
            _options: &ExtractOptions,
            _progress: &dyn ProgressSink,
            _ctl: &ControlToken,
        ) -> Result<squallz_core::api::ExtractReport, FormatError> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            Err(FormatError::WrongPassword)
        }

        fn test_summary(
            &mut self,
            _progress: &dyn ProgressSink,
            _ctl: &ControlToken,
        ) -> Result<TestSummary, FormatError> {
            Ok(TestSummary::default())
        }
    }

    #[test]
    fn default_destination_adds_exactly_one_named_layer() {
        let destination = default_destination(Path::new("/opt/Release.exe"), Path::new("/work"));

        assert_eq!(destination, PathBuf::from("/work/Release"));
        assert_ne!(destination, PathBuf::from("/work/Release/Release"));
    }

    #[test]
    fn default_destination_uses_safe_fallback_for_invalid_stems() {
        for executable in ["...exe", "CON.exe", "extensionless"] {
            assert_eq!(
                default_destination(Path::new(executable), Path::new("/work")),
                PathBuf::from("/work/extracted")
            );
        }
    }

    #[test]
    fn terminal_text_removes_control_sequences() {
        assert_eq!(safe_terminal_text("safe\u{1b}[31m"), "safe�[31m");
    }

    #[test]
    fn extract_json_reports_direct_layout() {
        use squallz_core::api::ExtractReport;
        use squallz_core::{ExtractPlan, ExtractScope, SmartLayout};

        let plan = ExtractPlan {
            requested_destination: PathBuf::from("out"),
            destination: PathBuf::from("out"),
            layout: SmartLayout::DirectExtract,
            scope: ExtractScope::default(),
            estimated_conflicts: 0,
        };
        let report = ExtractReport {
            destination: PathBuf::from("out"),
            ..ExtractReport::default()
        };
        let value = extract_json(&plan, &report);

        assert_eq!(value["plan"]["layout"], "direct");
        assert_eq!(value["dest"], "out");
        assert_eq!(value["ok"], true);
    }

    #[test]
    fn write_phase_password_failure_is_not_retried() {
        use squallz_core::{ExtractPlan, ExtractScope, SmartLayout};

        let calls = Arc::new(AtomicUsize::new(0));
        let mut reader = FailingExtractReader {
            calls: Arc::clone(&calls),
        };
        let plan = ExtractPlan {
            requested_destination: PathBuf::from("out"),
            destination: PathBuf::from("out"),
            layout: SmartLayout::DirectExtract,
            scope: ExtractScope::default(),
            estimated_conflicts: 0,
        };

        let result = extract_once(
            &mut reader,
            &plan,
            &ExtractOptions::default(),
            &NoProgress,
            &ControlToken::default(),
        );

        assert!(matches!(result, Err(FormatError::WrongPassword)));
        assert_eq!(calls.load(Ordering::Relaxed), 1);
    }
}
