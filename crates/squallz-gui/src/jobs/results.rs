//! The job result contract. A completed job has one report matching its kind.

use serde::Serialize;
use squallz_core::api::{ArchiveStructureStatus, ExtractReport, RecoverySummary};
use squallz_core::reports::{ChecksumCheckReport, CreateOutputReport, DuplicateReport, TestReport};
use squallz_core::{ChecksumReport, ExtractPlan};
use squallz_recovery::RecoveryReport;

use super::source_cleanup::SourceCleanupResult;
use crate::dto::ErrorDto;

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum JobResult {
    Compress(CreateResult),
    Convert(CreateResult),
    PublishMacosSfx(PublishResult),
    Extract(ExtractResult),
    ExtractNested(ExtractResult),
    BatchExtract(BatchExtractResult),
    Test(TestReport),
    ExportSqz(CreateOutputReport),
    RepairSqz {
        dest: String,
        in_place: bool,
        recovery: Option<RecoverySummary>,
    },
    RepairZip {
        dest: String,
        in_place: bool,
        source_entries: u64,
    },
    Protect(RecoveryReport),
    VerifyRecovery(RecoveryReport),
    RepairRecovery(RecoveryReport),
    Update {
        archive: String,
        operations: usize,
    },
    Checksum(ChecksumReport),
    ChecksumCheck(ChecksumCheckReport),
    DuplicateScan(DuplicateReport),
}

#[derive(Debug, Clone, Serialize)]
pub struct CreateResult {
    #[serde(flatten)]
    pub output: CreateOutputReport,
    pub source_cleanup: SourceCleanupResult,
    pub entries_tested_after_create: Option<u64>,
    pub sfx: Option<SfxDetails>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SfxDetails {
    pub target: &'static str,
    pub layout: &'static str,
    pub payload_bytes: u64,
    pub requires_signing: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct PublishResult {
    pub source: String,
    pub primary_output: String,
    pub outputs: Vec<String>,
    pub target: &'static str,
    pub layout: &'static str,
    pub payload_bytes: u64,
    pub total_bytes: u64,
    pub team_id: String,
    pub submission_id: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExtractResult {
    pub dest: String,
    pub best_effort: bool,
    pub problems: Vec<String>,
    pub problems_total: u64,
    pub problems_truncated: bool,
    pub plan: ExtractPlan,
    pub counts: ExtractReport,
    pub structure: ArchiveStructureStatus,
}

#[derive(Debug, Clone, Serialize)]
pub struct BatchExtractOutput {
    pub archive: String,
    #[serde(flatten)]
    pub result: ExtractResult,
}

#[derive(Debug, Clone, Serialize)]
pub struct BatchExtractFailure {
    pub archive: String,
    pub error: ErrorDto,
}

#[derive(Debug, Clone, Serialize)]
pub struct BatchExtractResult {
    pub archives: usize,
    pub selected_archives: usize,
    pub collapsed_volumes: usize,
    pub extracted: usize,
    pub failed: usize,
    pub recovered_archives: usize,
    pub outputs: Vec<BatchExtractOutput>,
    pub failures: Vec<BatchExtractFailure>,
}

impl JobResult {
    /// Replace private source leases before publishing any result or audit entry.
    pub(super) fn redact_sources(&mut self, replacements: &[(String, String)]) {
        if replacements.is_empty() {
            return;
        }
        let text = |value: &mut String| *value = super::redact_source_text(value, replacements);
        let texts = |values: &mut Vec<String>| values.iter_mut().for_each(&text);
        let path = |value: &mut std::path::PathBuf| {
            *value = super::redact_source_text(&value.to_string_lossy(), replacements).into();
        };
        let output = |report: &mut CreateOutputReport| {
            text(&mut report.primary_output);
            texts(&mut report.outputs);
            texts(&mut report.preserved_outputs);
        };
        let extract = |report: &mut ExtractResult| {
            text(&mut report.dest);
            path(&mut report.plan.requested_destination);
            path(&mut report.plan.destination);
            path(&mut report.counts.destination);
            texts(&mut report.problems);
        };
        match self {
            Self::Compress(report) | Self::Convert(report) => output(&mut report.output),
            Self::ExportSqz(report) => output(report),
            Self::PublishMacosSfx(report) => {
                text(&mut report.source);
                text(&mut report.primary_output);
                texts(&mut report.outputs);
            }
            Self::Extract(report) | Self::ExtractNested(report) => extract(report),
            Self::BatchExtract(report) => {
                for item in &mut report.outputs {
                    text(&mut item.archive);
                    extract(&mut item.result);
                }
                for item in &mut report.failures {
                    text(&mut item.archive);
                    super::redact_source_error(&mut item.error, replacements);
                }
            }
            Self::Test(report) => texts(&mut report.problems),
            Self::RepairSqz { dest, .. } | Self::RepairZip { dest, .. } => text(dest),
            Self::Update { archive, .. } => text(archive),
            Self::Checksum(report) => {
                for item in &mut report.items {
                    path(&mut item.path);
                }
            }
            Self::ChecksumCheck(report) => {
                path(&mut report.report.manifest);
                for item in &mut report.report.items {
                    path(&mut item.path);
                    if let Some(error) = &mut item.error {
                        text(error);
                    }
                }
            }
            Self::DuplicateScan(report) => {
                for group in &mut report.groups {
                    texts(&mut group.paths);
                }
            }
            Self::Protect(report) | Self::VerifyRecovery(report) | Self::RepairRecovery(report) => {
                path(&mut report.archive);
                path(&mut report.recovery);
                path(&mut report.tool);
                if let Some(value) = &mut report.output {
                    path(value);
                }
                report.outputs.iter_mut().for_each(path);
                text(&mut report.stdout);
                text(&mut report.stderr);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{create_report, extract_report, recovery_report};
    use super::*;
    use squallz_core::api::TestSummary;
    use squallz_core::{
        ChecksumAlgorithm, ChecksumItem, ChecksumReport, ChecksumVerificationItem,
        ChecksumVerificationReport, DuplicateGroup, DuplicateScanReport,
    };
    use std::path::Path;

    #[test]
    fn result_contract_covers_all_kinds_and_redacts_private_sources() {
        let private = "/private/source-lease/archive.zip";
        let public = "outer.zip → archive.zip";
        let source = Path::new(private);
        let mut extracted = extract_report(source);
        extracted.problems = vec![format!("{private}: damaged entry")];
        extracted.problems_total = 1;
        extracted.counts.failed = 1;
        extracted.counts.skipped = 2;
        let mut test = TestReport::new(&TestSummary::default(), ArchiveStructureStatus::Complete);
        test.ok = false;
        test.problems = extracted.problems.clone();
        test.problems_total = 1;
        let mut recovered = recovery_report(source, Some(source.into()), false);
        recovered.recovery = source.into();
        recovered.outputs = vec![source.into()];
        recovered.stderr = format!("{private}: damage found");
        let mut protected = recovered.clone();
        protected.operation = "protect";
        let mut verified = recovered.clone();
        verified.operation = "verify";
        let mut reports = vec![
            JobResult::Compress(create_report(source, false, 64)),
            JobResult::Convert(create_report(source, false, 64)),
            JobResult::PublishMacosSfx(PublishResult {
                source: private.into(),
                primary_output: private.into(),
                outputs: vec![private.into()],
                target: "macos",
                layout: "macos_app",
                payload_bytes: 32,
                total_bytes: 64,
                team_id: "TEAM".into(),
                submission_id: "accepted-submission".into(),
            }),
            JobResult::Extract(extracted.clone()),
            JobResult::ExtractNested(extracted.clone()),
            JobResult::BatchExtract(BatchExtractResult {
                archives: 2,
                selected_archives: 2,
                collapsed_volumes: 0,
                extracted: 1,
                failed: 1,
                recovered_archives: 0,
                outputs: vec![BatchExtractOutput {
                    archive: private.into(),
                    result: extracted,
                }],
                failures: vec![BatchExtractFailure {
                    archive: private.into(),
                    error: ErrorDto::other(format!("{private}: unreadable")),
                }],
            }),
            JobResult::Test(test),
            JobResult::ExportSqz(create_report(source, false, 64).output),
            JobResult::RepairSqz {
                dest: private.into(),
                in_place: false,
                recovery: None,
            },
            JobResult::RepairZip {
                dest: private.into(),
                in_place: false,
                source_entries: 1,
            },
            JobResult::Protect(protected),
            JobResult::VerifyRecovery(verified),
            JobResult::RepairRecovery(recovered),
            JobResult::Update {
                archive: private.into(),
                operations: 1,
            },
            JobResult::Checksum(ChecksumReport {
                algorithm: ChecksumAlgorithm::Sha256,
                input_count: 1,
                entries_scanned: 1,
                files_hashed: 1,
                bytes_hashed: 64,
                items: vec![ChecksumItem {
                    path: source.into(),
                    size: 64,
                    digest: "ab".into(),
                }],
            }),
            JobResult::ChecksumCheck(ChecksumCheckReport::from(&ChecksumVerificationReport {
                algorithm: ChecksumAlgorithm::Sha256,
                manifest: source.into(),
                checked: 1,
                passed: 0,
                failed: 1,
                bytes_hashed: 0,
                items: vec![ChecksumVerificationItem {
                    path: source.into(),
                    expected: "ab".into(),
                    actual: None,
                    ok: false,
                    error: Some(format!("{private}: missing")),
                }],
            })),
            JobResult::DuplicateScan(DuplicateReport::new(
                &DuplicateScanReport {
                    groups: vec![DuplicateGroup {
                        hash: "ab".into(),
                        size: 64,
                        paths: vec![source.into(), source.into()],
                    }],
                    ..Default::default()
                },
                0,
            )),
        ];
        for report in &mut reports {
            report.redact_sources(&[(private.into(), public.into())]);
        }
        let wire = serde_json::to_value(reports).unwrap();
        let kinds = wire
            .as_array()
            .unwrap()
            .iter()
            .map(|report| report["kind"].as_str().unwrap())
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(kinds.len(), 17);
        let json = serde_json::to_string(&wire).unwrap();
        assert!(!json.contains(private));
        assert!(json.contains(public));
        assert!(wire
            .as_array()
            .unwrap()
            .iter()
            .all(|report| report.get("entries").is_none()));
        // --nocapture exposes the exact Rust wire values for the frontend contract check.
        println!("SQUALLZ_RESULT_CONTRACT={json}");
    }
}
