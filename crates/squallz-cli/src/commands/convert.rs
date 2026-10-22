//! Conversion and SQZ export share a request, execution and result contract.
//! Input adapters retain their password defaults and public JSON envelopes.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use squallz_core::api::{
    CompressionLevel, CreateOptions, FormatError, OpenOptions, Password, ResourceOptions,
    SplitOutputMode,
};
use squallz_core::{is_sqz_archive_path, CreateArtifactKind, CreateReport};

use super::reports::{create_report_json, print_preserved_output_warning, print_pretty_json};
use crate::commands::{
    detected_format_label, memory_limit_label, threads_label, Ctx, ModernStatusField,
    ModernTableColumn, ModernTableRow,
};
use crate::errors::CliError;
use crate::progress::{fmt_bytes, CliProgress};
use crate::prompt::with_password_retry;
use crate::ui::Tone;

#[derive(Clone, Copy, PartialEq)]
pub(super) enum ConversionKind {
    Convert,
    Export,
}

impl ConversionKind {
    fn name(self) -> &'static str {
        match self {
            Self::Convert => "convert",
            Self::Export => "export",
        }
    }

    fn key(self, suffix: &str) -> String {
        format!("cli.{}.{suffix}", self.name())
    }

    pub(super) fn validate_paths(
        self,
        source: &Path,
        output: &Path,
        entry: ConversionEntry,
    ) -> Result<(), FormatError> {
        if self == Self::Export {
            let label = if entry.is_batch() {
                "batch export"
            } else {
                "export"
            };
            if !is_sqz_archive_path(source) {
                return Err(FormatError::Unsupported(format!(
                    "{label} expects a .sqz source container"
                )));
            }
            if is_sqz_archive_path(output) {
                return Err(FormatError::Unsupported(format!(
                    "{label} output must be a standard archive, not .sqz"
                )));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
pub(super) enum ConversionEntry {
    Command { json: bool },
    Batch,
}

impl ConversionEntry {
    fn is_batch(self) -> bool {
        matches!(self, Self::Batch)
    }
}

pub(super) struct ConversionRequest {
    pub kind: ConversionKind,
    pub source: PathBuf,
    pub output: PathBuf,
    pub open: OpenOptions,
    pub out_password: Option<String>,
    pub encrypt_names: bool,
    pub split: Option<u64>,
    pub split_mode: SplitOutputMode,
    pub level: u8,
    pub resources: ResourceOptions,
    pub force: bool,
}

pub(super) struct ConversionOutcome {
    report: CreateReport,
    pub result: Value,
    pub detail: String,
}

impl ConversionRequest {
    pub(super) fn execute(
        &self,
        ctx: &Ctx,
        entry: ConversionEntry,
    ) -> Result<ConversionOutcome, FormatError> {
        self.kind
            .validate_paths(&self.source, &self.output, entry)?;
        let json = match entry {
            ConversionEntry::Command { json } => json,
            ConversionEntry::Batch => true,
        };
        let progress = CliProgress::new_for_operation(ctx, json, self.kind.name());
        let result = (|| {
            let create = CreateOptions {
                level: CompressionLevel::from_numeric(self.level),
                password: self.out_password.clone().map(Password::new),
                encrypt_filenames: self.encrypt_names,
                split_size: self.split,
                split_mode: self.split_mode,
                resources: self.resources,
                ..CreateOptions::default()
            };
            let artifact = if self.split.is_some() {
                CreateArtifactKind::SplitArchive
            } else {
                CreateArtifactKind::Archive
            };
            // Bind overwrite authorization once, before any password prompts,
            // and reuse it on retries so a changed destination is rejected.
            let policy = super::create_commit_policy(
                &self.output,
                artifact,
                self.force,
                &progress,
                &ctx.ctl,
            )?;
            let convert = |password: Option<&Password>| {
                ctx.engine.convert(
                    &self.source,
                    &self.output,
                    &OpenOptions {
                        password: password.cloned(),
                        encoding_override: self.open.encoding_override.clone(),
                    },
                    &create,
                    policy,
                    &progress,
                    &ctx.ctl,
                )
            };
            if self.kind == ConversionKind::Convert && !entry.is_batch() {
                with_password_retry(
                    &ctx.loc,
                    self.open.password.as_ref(),
                    || progress.finish(),
                    convert,
                )
            } else {
                convert(self.open.password.as_ref())
            }
        })();
        progress.finish();
        let report = result?;
        Ok(ConversionOutcome {
            result: self.result_json(&report, entry),
            detail: self.detail(),
            report,
        })
    }

    fn result_json(&self, report: &CreateReport, entry: ConversionEntry) -> Value {
        let mut value = match self.kind {
            ConversionKind::Convert => {
                let mut value = create_report_json(report);
                value["operation"] = json!("convert");
                value["source"] = json!(self.source.display().to_string());
                value
            }
            ConversionKind::Export => json!({
                "operation": if entry.is_batch() { "export" } else { "export_sqz" },
                "archive": self.source.display().to_string(),
                "output": self.output.display().to_string(),
            }),
        };
        if entry.is_batch() {
            value["level"] = json!(self.level);
        } else {
            value["ok"] = json!(true);
        }
        value
    }

    fn detail(&self) -> String {
        let verb = match self.kind {
            ConversionKind::Convert => "converted",
            ConversionKind::Export => "exported",
        };
        format!(
            "{verb} {} to {}",
            self.source.display(),
            self.output.display()
        )
    }
}

pub(super) fn run(ctx: &Ctx, request: ConversionRequest, json: bool) -> Result<(), CliError> {
    let entry = ConversionEntry::Command { json };
    let outcome = request.execute(ctx, entry)?;
    if json {
        print_pretty_json(&outcome.result)?;
        return Ok(());
    }
    print_result(ctx, &request, &outcome.report);
    Ok(())
}

fn print_result(ctx: &Ctx, request: &ConversionRequest, report: &CreateReport) {
    let export = request.kind == ConversionKind::Export;
    let key = |suffix: &str| request.kind.key(suffix);
    let path = if export {
        &request.output
    } else {
        &report.primary_output
    };
    let path = path.display().to_string();
    if ctx.is_modern() {
        let source_format = if export {
            "sqz".to_owned()
        } else {
            detected_format_label(ctx, &request.source)
        };
        let target_format = detected_format_label(ctx, &request.output);
        let output_size = if export {
            std::fs::metadata(&request.output)
                .map_or_else(|_| "-".to_owned(), |metadata| fmt_bytes(metadata.len()))
        } else {
            fmt_bytes(report.total_output_bytes)
        };
        let volume_count = report.split_volume_count.unwrap_or(1).to_string();
        let formats = format!("{source_format} → {target_format}");
        let mut fields = vec![
            ModernStatusField::new(ctx.loc.t("common.format"), formats.clone()),
            ModernStatusField::new(ctx.loc.t("common.output_size"), output_size.clone()),
        ];
        if !export {
            fields.push(ModernStatusField::new(
                ctx.loc.t("common.volumes"),
                volume_count.clone(),
            ));
        }
        fields.extend([
            ModernStatusField::new(
                ctx.loc.t("common.source"),
                request.source.display().to_string(),
            ),
            ModernStatusField::new(ctx.loc.t("common.output"), path.clone()),
        ]);
        ctx.print_modern_status_panel(
            &ctx.loc.t(&key("result_title")),
            &ctx.loc.t("common.done"),
            Tone::Success,
            &format!("{formats} · {output_size} · {path}"),
            &fields,
        );
        let widths = if export { [16, 16, 64] } else { [14, 16, 66] };
        ctx.print_modern_table(
            &ctx.loc.t(&key("plan_title")),
            &[
                ModernTableColumn::new(ctx.loc.t("common.operation"), widths[0]),
                ModernTableColumn::new(ctx.loc.t("common.format"), widths[1]),
                ModernTableColumn::new(ctx.loc.t("common.path"), widths[2]),
            ],
            &[
                ModernTableRow::new(vec![
                    ctx.loc.t("common.source"),
                    if export {
                        ctx.loc.t("cli.export.sqz_container")
                    } else {
                        source_format
                    },
                    request.source.display().to_string(),
                ]),
                ModernTableRow::success(vec![
                    ctx.loc.t("common.output"),
                    target_format,
                    path.clone(),
                ]),
            ],
        );
        let policy = |label: &str, value| ModernTableRow::new(vec![ctx.loc.t(label), value]);
        let mut policies = Vec::new();
        if export {
            policies.push(policy(
                "cli.export.lock_in",
                ctx.loc.t("cli.export.lock_in.none"),
            ));
        }
        policies.extend([
            policy("common.level", request.level.to_string()),
            policy(
                &key("destination_encryption"),
                yes_no(ctx, request.out_password.is_some()),
            ),
        ]);
        if !export {
            policies.push(policy(
                "cli.convert.encrypt_names",
                yes_no(ctx, request.encrypt_names),
            ));
        }
        policies.extend([
            policy(
                if export {
                    "common.threads"
                } else {
                    "cli.convert.threads"
                },
                threads_label(ctx, request.resources.threads),
            ),
            policy(
                if export {
                    "common.memory_limit"
                } else {
                    "cli.convert.memory_limit"
                },
                memory_limit_label(ctx, request.resources.memory_limit),
            ),
        ]);
        if !export {
            policies.push(policy("common.volumes", volume_count));
        }
        policies.push(policy("common.output_size", output_size));
        ctx.print_modern_table(
            &ctx.loc.t(&key("policy_title")),
            &[
                ModernTableColumn::new(ctx.loc.t("common.setting"), 28),
                ModernTableColumn::new(ctx.loc.t("common.value"), 68),
            ],
            &policies,
        );
    } else {
        ctx.print_success(ctx.loc.format(&key("done"), &[("path", &path)]));
    }
    if !export {
        let preserved = report
            .preserved_outputs
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>();
        print_preserved_output_warning(ctx, &preserved);
    }
}

fn yes_no(ctx: &Ctx, value: bool) -> String {
    ctx.loc.t(if value { "common.yes" } else { "common.no" })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::detect_name_for_path;
    use std::path::Path;

    #[test]
    fn detect_name_handles_split_and_missing_file_names() {
        assert_eq!(
            detect_name_for_path(Path::new("archive.7z.001")).as_deref(),
            Some("archive.7z")
        );
        assert_eq!(
            detect_name_for_path(Path::new("plain.zip")).as_deref(),
            Some("plain.zip")
        );
        assert_eq!(detect_name_for_path(Path::new("/")), None);
    }

    #[test]
    fn sqz_path_accepts_plain_and_split_sqz_sources_only() {
        assert!(is_sqz_archive_path(Path::new("archive.sqz")));
        assert!(is_sqz_archive_path(Path::new("archive.sqz.001")));
        assert!(is_sqz_archive_path(Path::new("ARCHIVE.SQZ")));
        assert!(!is_sqz_archive_path(Path::new("archive.zip")));
        assert!(!is_sqz_archive_path(Path::new("archive.zip.001")));
        assert!(!is_sqz_archive_path(Path::new("/")));
    }
}
