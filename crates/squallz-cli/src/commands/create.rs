//! Archive creation shared by ordinary commands and batch jobs.

use std::path::{Path, PathBuf};

use squallz_core::api::{
    CreateOptions, Detected, FormatError, NoProgress, OpenOptions, ProgressPhase, ProgressSink,
    SafetyLimits,
};
use squallz_core::{CreateArtifactKind, CreateReport};

use crate::progress::CliProgress;

use super::Ctx;

pub(super) struct CreatedArchive {
    pub(super) report: CreateReport,
    pub(super) entries_tested: Option<u64>,
}

#[derive(Clone, Copy)]
pub(super) enum CreateProgressMode {
    Ordinary {
        json_output: bool,
        operation: &'static str,
    },
    Batch,
}

impl CreateProgressMode {
    fn start(self, ctx: &Ctx) -> StageProgress {
        match self {
            Self::Ordinary {
                json_output,
                operation,
            } => {
                StageProgress::Ordinary(CliProgress::new_for_operation(ctx, json_output, operation))
            }
            Self::Batch => StageProgress::Batch,
        }
    }
}

enum StageProgress {
    Ordinary(CliProgress),
    Batch,
}

impl StageProgress {
    fn sink(&self) -> &dyn ProgressSink {
        match self {
            Self::Ordinary(progress) => progress,
            Self::Batch => &NoProgress,
        }
    }

    fn finish(self) {
        if let Self::Ordinary(progress) = self {
            progress.finish();
        }
    }
}

pub(super) struct CreationRequest<'a> {
    pub(super) output: &'a Path,
    pub(super) inputs: &'a [PathBuf],
    pub(super) options: &'a CreateOptions,
    pub(super) test_after_create: bool,
    pub(super) replace_existing: bool,
}

impl CreationRequest<'_> {
    pub(super) fn execute(
        self,
        ctx: &Ctx,
        progress_mode: CreateProgressMode,
    ) -> Result<CreatedArchive, FormatError> {
        let Self {
            output,
            inputs,
            options,
            test_after_create,
            replace_existing,
        } = self;
        let kind = if options.split_size.is_some() {
            CreateArtifactKind::SplitArchive
        } else {
            CreateArtifactKind::Archive
        };
        let inspection_progress = progress_mode.start(ctx);
        let policy = super::create_commit_policy(
            output,
            kind,
            replace_existing,
            inspection_progress.sink(),
            &ctx.ctl,
        );
        inspection_progress.finish();
        let policy = policy?;

        let write_progress = progress_mode.start(ctx);
        let result = ctx.engine.create(
            output,
            inputs,
            options,
            policy,
            write_progress.sink(),
            &ctx.ctl,
        );
        write_progress.finish();
        let report = result?;

        let test = if test_after_create {
            let test_progress = progress_mode.start(ctx);
            test_progress
                .sink()
                .on_phase(ProgressPhase::OutputVerify, true);
            let result = ctx.engine.test_summary(
                &report.primary_output,
                &OpenOptions {
                    password: options.password.clone(),
                    encoding_override: None,
                },
                &SafetyLimits::default(),
                test_progress.sink(),
                &ctx.ctl,
            );
            test_progress.finish();
            let result = result?;
            if !result.is_ok() {
                return Err(match progress_mode {
                    CreateProgressMode::Batch => super::reports::test_report_error(result),
                    CreateProgressMode::Ordinary { .. } => {
                        let preview = result.problems.messages.join("; ");
                        FormatError::CorruptArchive(if preview.is_empty() {
                            format!(
                                "created archive failed integrity testing with {} problem(s)",
                                result.problems.total
                            )
                        } else {
                            format!("created archive failed integrity testing: {preview}")
                        })
                    }
                });
            }
            Some(result.entries_tested)
        } else {
            None
        };
        Ok(CreatedArchive {
            report,
            entries_tested: test,
        })
    }
}

pub(super) fn validate_requested_format(
    ctx: &Ctx,
    output: &Path,
    requested: Option<&str>,
) -> Result<(), FormatError> {
    let Some(requested) = requested else {
        return Ok(());
    };
    let output_name = output
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| FormatError::Unsupported("output path has no valid file name".into()))?;
    let output_key = detected_format_key(ctx, output_name).ok_or_else(|| {
        FormatError::Unsupported(format!(
            "output path does not identify a supported format: {}",
            output.display()
        ))
    })?;
    let requested_key = requested_format_key(ctx, requested).ok_or_else(|| {
        FormatError::Unsupported(format!("unsupported requested format: {requested}"))
    })?;
    if output_key != requested_key {
        return Err(FormatError::Unsupported(format!(
            "requested format '{requested}' does not match output path '{}'",
            output.display()
        )));
    }
    Ok(())
}

fn requested_format_key(ctx: &Ctx, requested_format: &str) -> Option<String> {
    let requested = requested_format
        .trim()
        .trim_start_matches('.')
        .to_ascii_lowercase();
    if requested.is_empty() {
        return None;
    }
    let direct_name = format!("archive.{requested}");
    if let Some(key) = detected_format_key(ctx, &direct_name) {
        return Some(key);
    }
    ctx.engine
        .supported_formats()
        .into_iter()
        .find(|format| format.id.eq_ignore_ascii_case(&requested))
        .and_then(|format| {
            format
                .extensions
                .first()
                .and_then(|ext| detected_format_key(ctx, &format!("archive.{ext}")))
        })
}

fn detected_format_key(ctx: &Ctx, name: &str) -> Option<String> {
    match ctx.engine.registry().detect_by_name(name)? {
        Detected::Archive(archive) => Some(format!("archive:{}", archive.id())),
        Detected::Compressed {
            compressor,
            inner_archive: Some(archive),
        } => Some(format!("compound:{}:{}", archive.id(), compressor.id())),
        Detected::Compressed {
            compressor,
            inner_archive: None,
        } => Some(format!("compressor:{}", compressor.id())),
    }
}
