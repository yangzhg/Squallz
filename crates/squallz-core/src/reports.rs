//! Typed reports shared by command-line and desktop adapters.

use crate::api::{ArchiveStructureStatus, RecoverySummary, TestSummary};
use crate::{ChecksumVerificationReport, CreateReport, DuplicateScanReport};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct CreateOutputReport {
    pub primary_output: String,
    pub outputs: Vec<String>,
    pub preserved_outputs: Vec<String>,
    pub total_bytes: u64,
    pub split: bool,
    pub volume_count: usize,
}

impl From<&CreateReport> for CreateOutputReport {
    fn from(report: &CreateReport) -> Self {
        Self {
            primary_output: report.primary_output.display().to_string(),
            outputs: report
                .outputs
                .iter()
                .map(|p| p.display().to_string())
                .collect(),
            preserved_outputs: report
                .preserved_outputs
                .iter()
                .map(|p| p.display().to_string())
                .collect(),
            total_bytes: report.total_output_bytes,
            split: report.split_volume_count.is_some(),
            volume_count: report.split_volume_count.unwrap_or(1),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct TestReport {
    pub ok: bool,
    pub entries_tested: u64,
    pub problems: Vec<String>,
    pub problems_total: u64,
    pub problems_truncated: bool,
    pub recovery: Option<RecoverySummary>,
    #[serde(skip_serializing_if = "ArchiveStructureStatus::is_complete")]
    pub structure: ArchiveStructureStatus,
}

impl TestReport {
    pub fn new(report: &TestSummary, structure: ArchiveStructureStatus) -> Self {
        Self {
            ok: report.is_ok(),
            entries_tested: report.entries_tested,
            problems: report.problems.messages.clone(),
            problems_total: report.problems.total,
            problems_truncated: report.problems.is_truncated(),
            recovery: report.recovery.clone(),
            structure,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ChecksumCheckReport {
    pub ok: bool,
    #[serde(flatten)]
    pub report: ChecksumVerificationReport,
}
impl From<&ChecksumVerificationReport> for ChecksumCheckReport {
    fn from(report: &ChecksumVerificationReport) -> Self {
        Self {
            ok: report.is_ok(),
            report: report.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct DuplicateGroupReport {
    pub hash: String,
    pub hash_algorithm: &'static str,
    pub size: u64,
    pub count: usize,
    pub reclaimable_bytes: u64,
    pub paths: Vec<String>,
}
#[derive(Debug, Clone, Serialize)]
pub struct DuplicateReport {
    pub hash_algorithm: &'static str,
    pub input_count: usize,
    pub entries_scanned: usize,
    pub files_scanned: usize,
    pub bytes_scanned: u64,
    pub min_size: u64,
    pub candidate_files: usize,
    pub hashed_bytes: u64,
    pub duplicate_groups: usize,
    pub duplicate_files: usize,
    pub reclaimable_bytes: u64,
    pub groups: Vec<DuplicateGroupReport>,
}
impl DuplicateReport {
    pub fn new(report: &DuplicateScanReport, min_size: u64) -> Self {
        Self {
            hash_algorithm: "blake3",
            input_count: report.input_count,
            entries_scanned: report.entries_scanned,
            files_scanned: report.files_scanned,
            bytes_scanned: report.bytes_scanned,
            min_size,
            candidate_files: report.candidate_files,
            hashed_bytes: report.hashed_bytes,
            duplicate_groups: report.duplicate_groups(),
            duplicate_files: report.duplicate_files(),
            reclaimable_bytes: report.reclaimable_bytes(),
            groups: report
                .groups
                .iter()
                .map(|group| DuplicateGroupReport {
                    hash: group.hash.clone(),
                    hash_algorithm: "blake3",
                    size: group.size,
                    count: group.count(),
                    reclaimable_bytes: group.reclaimable_bytes(),
                    paths: group
                        .paths
                        .iter()
                        .map(|p| p.display().to_string())
                        .collect(),
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ChecksumAlgorithm, ChecksumItem, ChecksumReport, ChecksumVerificationItem, DuplicateGroup,
    };
    use serde_json::json;

    #[test]
    fn checksum_json_preserves_algorithms_and_verification_failure_details() {
        for algorithm in [
            ChecksumAlgorithm::Blake3,
            ChecksumAlgorithm::Md5,
            ChecksumAlgorithm::Sha1,
            ChecksumAlgorithm::Sha224,
            ChecksumAlgorithm::Sha256,
            ChecksumAlgorithm::Sha384,
            ChecksumAlgorithm::Sha512,
            ChecksumAlgorithm::Crc32,
        ] {
            let report = ChecksumReport {
                algorithm,
                input_count: 2,
                entries_scanned: 4,
                files_hashed: 1,
                bytes_hashed: 3,
                items: vec![ChecksumItem {
                    path: "目录/空 格.txt".into(),
                    size: 3,
                    digest: "abc".into(),
                }],
            };
            assert_eq!(
                json!(report),
                json!({
                    "algorithm": algorithm.id(),
                    "input_count": 2, "entries_scanned": 4, "files_hashed": 1, "bytes_hashed": 3,
                    "items": [{"path": "目录/空 格.txt", "size": 3, "digest": "abc"}],
                })
            );
        }
        let report = ChecksumVerificationReport {
            algorithm: ChecksumAlgorithm::Sha256,
            manifest: "目录/SHA256SUMS".into(),
            checked: 1,
            passed: 0,
            failed: 1,
            bytes_hashed: 0,
            items: vec![ChecksumVerificationItem {
                path: "missing.txt".into(),
                expected: "abc".into(),
                actual: None,
                ok: false,
                error: Some("missing".into()),
            }],
        };
        assert_eq!(
            json!(ChecksumCheckReport::from(&report)),
            json!({
                "ok": false, "algorithm": "sha256",
                "manifest": "目录/SHA256SUMS", "checked": 1, "passed": 0, "failed": 1,
                "bytes_hashed": 0, "items": [{"path": "missing.txt", "expected": "abc",
                    "actual": null, "ok": false, "error": "missing"}],
            })
        );
    }

    #[test]
    fn duplicate_json_keeps_scan_totals_and_ordered_group_counts() {
        let report = DuplicateScanReport {
            input_count: 2,
            entries_scanned: 7,
            files_scanned: 6,
            bytes_scanned: 25,
            candidate_files: 5,
            hashed_bytes: 20,
            groups: vec![DuplicateGroup {
                hash: "abc".into(),
                size: 4,
                paths: vec!["a".into(), "b".into(), "c".into()],
            }],
        };
        assert_eq!(
            json!(DuplicateReport::new(&report, 4)),
            json!({
                "hash_algorithm": "blake3",
                "input_count": 2, "entries_scanned": 7, "files_scanned": 6, "bytes_scanned": 25,
                "min_size": 4, "candidate_files": 5, "hashed_bytes": 20, "duplicate_groups": 1,
                "duplicate_files": 3, "reclaimable_bytes": 8,
                "groups": [{"hash": "abc", "hash_algorithm": "blake3", "size": 4,
                    "count": 3, "reclaimable_bytes": 8, "paths": ["a", "b", "c"]}],
            })
        );
        assert_eq!(
            json!(DuplicateReport::new(&DuplicateScanReport::default(), 0))["groups"],
            json!([])
        );
    }

    #[cfg(unix)]
    #[test]
    fn reports_keep_lossy_paths_instead_of_rejecting_non_utf8_names() {
        use std::os::unix::ffi::OsStringExt;
        let path = std::path::PathBuf::from(std::ffi::OsString::from_vec(b"file-\xff".to_vec()));
        let report = ChecksumReport {
            algorithm: ChecksumAlgorithm::Blake3,
            input_count: 1,
            entries_scanned: 1,
            files_hashed: 1,
            bytes_hashed: 0,
            items: vec![ChecksumItem {
                path: path.clone(),
                size: 0,
                digest: "abc".into(),
            }],
        };
        assert_eq!(
            json!(report)["items"][0]["path"],
            path.to_string_lossy().as_ref()
        );
        let report = DuplicateScanReport {
            groups: vec![DuplicateGroup {
                hash: "abc".into(),
                size: 0,
                paths: vec![path.clone()],
            }],
            ..DuplicateScanReport::default()
        };
        assert_eq!(
            json!(DuplicateReport::new(&report, 0))["groups"][0]["paths"][0],
            path.to_string_lossy().as_ref()
        );
    }
}
