use std::path::Path;

use serde_json::{json, Value};
use squallz_core::api::{
    ArchiveStructureStatus, ExtractReport, FormatError, RecoverySummary, TestSummary,
};
use squallz_core::{CreateReport, ExtractPlan, SmartLayout};

use crate::errors::CliError;
use crate::ui::Tone;

use super::{Ctx, ModernStatusField};

pub(crate) fn print_pretty_json(value: &Value) -> Result<(), CliError> {
    let text = pretty_json_text(value)?;
    println!("{text}");
    Ok(())
}

pub(crate) fn create_report_json(report: &CreateReport) -> Value {
    let outputs = report
        .outputs
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>();
    let preserved_outputs = report
        .preserved_outputs
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>();
    json!({
        "output": report.primary_output.display().to_string(),
        "primary_output": report.primary_output.display().to_string(),
        "outputs": outputs,
        "preserved_outputs": preserved_outputs,
        "total_bytes": report.total_output_bytes,
        "split": report.split_volume_count.is_some(),
        "volumes": report.split_volume_count.unwrap_or(1),
    })
}

pub(crate) fn extract_plan_json(plan: &ExtractPlan) -> Value {
    json!({
        "requested_destination": plan.requested_destination.display().to_string(),
        "destination": plan.destination.display().to_string(),
        "layout": match plan.layout {
            SmartLayout::DirectExtract => "direct",
            SmartLayout::WrapInFolder => "wrap_in_folder",
        },
        "entries": plan.scope.entries,
        "files": plan.scope.files,
        "directories": plan.scope.directories,
        "symlinks": plan.scope.symlinks,
        "hardlinks": plan.scope.hardlinks,
        "other": plan.scope.other,
        "total_bytes": plan.scope.total_bytes,
        "estimated_conflicts": plan.estimated_conflicts,
    })
}

pub(crate) fn extract_counts_json(report: &ExtractReport) -> Value {
    json!({
        "destination": report.destination.display().to_string(),
        "selected_entries": report.selected_entries,
        "created": report.created,
        "directories": report.directories,
        "skipped": report.skipped,
        "replaced": report.replaced,
        "renamed": report.renamed,
        "failed": report.failed,
        "output_bytes": report.output_bytes,
    })
}

pub(crate) fn empty_extract_counts_json(destination: &Path) -> Value {
    json!({
        "destination": destination.display().to_string(),
        "selected_entries": 0,
        "created": 0,
        "directories": 0,
        "skipped": 0,
        "replaced": 0,
        "renamed": 0,
        "failed": 0,
        "output_bytes": 0,
    })
}

pub(crate) fn print_preserved_output_warning(ctx: &Ctx, paths: &[String]) {
    if paths.is_empty() {
        return;
    }

    let count = paths.len().to_string();
    let summary = ctx
        .loc
        .format("cli.create.preserved_summary", &[("count", &count)]);
    if ctx.is_modern() {
        ctx.print_modern_status_panel(
            &ctx.loc.t("cli.create.preserved_title"),
            &ctx.loc.t("cli.create.preserved_status"),
            Tone::Warning,
            &summary,
            &[ModernStatusField::new(
                ctx.loc.t("cli.create.preserved_count"),
                count,
            )],
        );
        println!();
        println!(
            "{}",
            ctx.paint_stdout_tone(Tone::Primary, &ctx.loc.t("cli.create.preserved_paths"))
        );
        for (index, path) in paths.iter().enumerate() {
            println!(
                "{}",
                ctx.paint_stdout_tone(Tone::Warning, &format!("  {}. {path}", index + 1))
            );
        }
        println!(
            "{}",
            ctx.paint_stdout_tone(
                Tone::Secondary,
                &ctx.loc.t("cli.create.preserved_next_step")
            )
        );
        return;
    }

    ctx.eprint_problem(&summary);
    for path in paths {
        ctx.eprint_problem(format!("  {path}"));
    }
    ctx.eprint_problem(ctx.loc.t("cli.create.preserved_next_step"));
}

fn pretty_json_text(value: &Value) -> Result<String, CliError> {
    serde_json::to_string_pretty(value)
        .map_err(|e| FormatError::Other(format!("cannot serialize CLI JSON report: {e}")).into())
}

pub(crate) fn test_report_json(report: &TestSummary) -> Value {
    json!({
        "ok": report.is_ok(),
        "entries_tested": report.entries_tested,
        "problems": &report.problems.messages,
        "problems_total": report.problems.total,
        "problems_truncated": report.problems.is_truncated(),
        "recovery": report.recovery.as_ref().map(recovery_summary_json),
    })
}

pub(crate) fn test_report_json_with_structure(
    report: &TestSummary,
    structure: ArchiveStructureStatus,
) -> Value {
    let mut value = test_report_json(report);
    if !structure.is_complete() {
        value["structure"] = json!(structure.id());
    }
    value
}

pub(crate) fn print_test_problems(ctx: &Ctx, report: &TestSummary) {
    print_test_problems_with_structure(ctx, report, ArchiveStructureStatus::Complete);
}

pub(crate) fn localized_test_problems(
    ctx: &Ctx,
    report: &TestSummary,
    structure: ArchiveStructureStatus,
) -> Vec<String> {
    report
        .problems
        .messages
        .iter()
        .enumerate()
        .map(|(index, problem)| {
            if index == 0 && structure == ArchiveStructureStatus::ZipLocalHeadersRecovered {
                ctx.loc.t("cli.test.zip_local_headers_recovered")
            } else {
                problem.clone()
            }
        })
        .collect()
}

pub(crate) fn print_test_problems_with_structure(
    ctx: &Ctx,
    report: &TestSummary,
    structure: ArchiveStructureStatus,
) {
    for problem in localized_test_problems(ctx, report, structure) {
        let message = ctx.loc.format("cli.test.problem", &[("detail", &problem)]);
        ctx.eprint_problem(&message);
    }
    if report.problems.is_truncated() {
        let shown = report.problems.messages.len().to_string();
        let omitted = report.problems.omitted().to_string();
        ctx.eprint_problem(ctx.loc.format(
            "cli.test.problem_preview_truncated",
            &[("shown", &shown), ("omitted", &omitted)],
        ));
    }
}

pub(crate) fn recovery_summary_json(summary: &RecoverySummary) -> Value {
    json!({
        "scheme": &summary.scheme,
        "block_size": summary.block_size,
        "total_blocks": summary.total_blocks,
        "data_shards": summary.data_shards,
        "parity_shards": summary.parity_shards,
        "recovery_blocks_available": summary.recovery_blocks_available,
        "damaged_blocks": summary.damaged_blocks,
        "repaired_blocks": summary.repaired_blocks,
        "unrepaired_blocks": summary.unrepaired_blocks,
        "repair_possible": summary.repair_possible,
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use squallz_core::api::ExtractReport;
    use squallz_core::{ExtractPlan, ExtractScope, SmartLayout};

    use super::*;

    #[test]
    fn complete_structure_keeps_the_legacy_test_json_shape() {
        let report = TestSummary {
            entries_tested: 2,
            ..TestSummary::default()
        };

        assert_eq!(
            test_report_json_with_structure(&report, ArchiveStructureStatus::Complete),
            test_report_json(&report)
        );
    }

    #[test]
    fn recovered_structure_is_reported_without_changing_existing_fields() {
        let report = TestSummary {
            entries_tested: 2,
            ..TestSummary::default()
        };
        let legacy = test_report_json(&report);
        let recovered = test_report_json_with_structure(
            &report,
            ArchiveStructureStatus::ZipLocalHeadersRecovered,
        );

        assert_eq!(recovered["structure"], "zip_local_headers_recovered");
        for key in [
            "ok",
            "entries_tested",
            "problems",
            "problems_total",
            "problems_truncated",
            "recovery",
        ] {
            assert_eq!(recovered[key], legacy[key], "field {key}");
        }
    }

    #[test]
    fn create_report_json_keeps_recovery_sidecars_out_of_volume_count() {
        let report = CreateReport {
            primary_output: PathBuf::from("backup.sqz.001"),
            outputs: vec![
                PathBuf::from("backup.sqz.001"),
                PathBuf::from("backup.sqz.002"),
                PathBuf::from("backup.sqz.rev001"),
                PathBuf::from("backup.sqz.rev002"),
            ],
            preserved_outputs: vec![PathBuf::from(
                ".backup.sqz.001.split-backup-123-0.tmp.backup.sqz.001",
            )],
            total_output_bytes: 4096,
            split_volume_count: Some(2),
        };

        let value = create_report_json(&report);

        assert_eq!(value["output"], "backup.sqz.001");
        assert_eq!(value["primary_output"], "backup.sqz.001");
        assert_eq!(
            value["outputs"],
            json!([
                "backup.sqz.001",
                "backup.sqz.002",
                "backup.sqz.rev001",
                "backup.sqz.rev002"
            ])
        );
        assert_eq!(value["total_bytes"], 4096);
        assert_eq!(
            value["preserved_outputs"],
            json!([".backup.sqz.001.split-backup-123-0.tmp.backup.sqz.001"])
        );
        assert_eq!(value["split"], true);
        assert_eq!(value["volumes"], 2);
    }

    #[test]
    fn extract_json_helpers_keep_plan_flat_and_counts_separate() {
        let plan = ExtractPlan {
            requested_destination: PathBuf::from("output"),
            destination: PathBuf::from("output/archive"),
            layout: SmartLayout::WrapInFolder,
            scope: ExtractScope {
                entries: 8,
                files: 3,
                directories: 1,
                symlinks: 1,
                hardlinks: 1,
                other: 2,
                total_bytes: 4096,
            },
            estimated_conflicts: 2,
        };
        let report = ExtractReport {
            destination: plan.destination.clone(),
            selected_entries: 8,
            created: 2,
            directories: 1,
            skipped: 1,
            replaced: 2,
            renamed: 1,
            failed: 2,
            output_bytes: 3072,
        };

        let plan_json = extract_plan_json(&plan);
        let counts_json = extract_counts_json(&report);
        let empty_counts_json = empty_extract_counts_json(&plan.destination);

        assert_eq!(plan_json["destination"], "output/archive");
        assert_eq!(plan_json["layout"], "wrap_in_folder");
        assert_eq!(plan_json["entries"], 8);
        assert_eq!(plan_json["estimated_conflicts"], 2);
        assert!(plan_json.get("scope").is_none());
        assert_eq!(counts_json["destination"], "output/archive");
        assert_eq!(counts_json["selected_entries"], 8);
        assert_eq!(counts_json["created"], 2);
        assert_eq!(counts_json["directories"], 1);
        assert_eq!(counts_json["skipped"], 1);
        assert_eq!(counts_json["replaced"], 2);
        assert_eq!(counts_json["renamed"], 1);
        assert_eq!(counts_json["failed"], 2);
        assert_eq!(counts_json["output_bytes"], 3072);
        assert_eq!(empty_counts_json["destination"], "output/archive");
        assert_eq!(empty_counts_json["failed"], 0);
    }
}
