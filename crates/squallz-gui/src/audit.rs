//! Persistent desktop operation audit log.
//!
//! The frontend keeps a rich local history for UX. This backend log is a
//! smaller, sanitized source of truth for completed GUI jobs: it records
//! operation kind, final state, timestamps, and path basenames, never
//! passwords or full user-selected path trees.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

use crate::dto::JobSpec;
use squallz_core::lock_unpoisoned;

#[derive(Debug, Serialize)]
pub struct OperationAuditRecord {
    pub id: u64,
    pub time: u64,
    pub kind: String,
    pub state: String,
    pub title: String,
    pub detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result_summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_key: Option<String>,
}

#[derive(Debug, Clone)]
pub struct OperationAuditSummary {
    pub kind: String,
    pub title: String,
    pub detail: String,
}

pub struct OperationAudit {
    path: Option<PathBuf>,
    write_lock: Mutex<()>,
}

fn millis_since_epoch_or_zero(time: SystemTime) -> u64 {
    match time.duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_millis() as u64,
        Err(_) => 0,
    }
}

fn selected_entries_label(selection: &Option<Vec<String>>) -> String {
    match selection {
        Some(items) => format!("{} selected", items.len()),
        None => "all entries".into(),
    }
}

fn repair_recovery_detail(path: &str, output: &Option<String>) -> String {
    match output {
        Some(output) => format!("{} -> {}", base(path), base(output)),
        None => base(path),
    }
}

impl OperationAudit {
    pub fn persistent() -> Self {
        let path = dirs::data_dir().map(|dir| dir.join("Squallz").join("operation-audit.jsonl"));
        Self::from_path(path)
    }

    #[cfg(test)]
    pub fn disabled() -> Self {
        Self::from_path(None)
    }

    #[cfg(test)]
    pub fn with_path(path: PathBuf) -> Self {
        Self::from_path(Some(path))
    }

    fn from_path(path: Option<PathBuf>) -> Self {
        Self {
            path,
            write_lock: Mutex::new(()),
        }
    }

    pub fn append(&self, record: OperationAuditRecord) -> std::io::Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        // A record spans multiple writes; hold the lock through its newline
        // and flush so concurrent job completions cannot interleave JSON.
        let _guard = lock_unpoisoned(&self.write_lock);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut file = OpenOptions::new().create(true).append(true).open(path)?;
        serde_json::to_writer(&mut file, &record)?;
        file.write_all(b"\n")?;
        file.flush()
    }
}

pub fn now_millis() -> u64 {
    millis_since_epoch_or_zero(SystemTime::now())
}

pub fn summarize_job(spec: &JobSpec) -> OperationAuditSummary {
    match spec {
        JobSpec::Compress {
            inputs,
            dest,
            sfx_target,
            ..
        } => {
            let sfx = sfx_target.map(|target| target.as_str());
            OperationAuditSummary {
                kind: if sfx.is_some() {
                    "create_sfx"
                } else {
                    "compress"
                }
                .into(),
                title: if sfx.is_some() {
                    "Create self-extractor".into()
                } else {
                    "Create archive".into()
                },
                detail: format!(
                    "{} input{} -> {}{}",
                    inputs.len(),
                    plural(inputs.len()),
                    base(dest),
                    sfx.map(|target| format!(" · {target}")).unwrap_or_default()
                ),
            }
        }
        JobSpec::PublishMacosSfx { source, output, .. } => OperationAuditSummary {
            kind: "publish_macos_sfx".into(),
            title: "Publish macOS self-extractor".into(),
            detail: format!("{} -> {}", base(source), base(output)),
        },
        JobSpec::Extract {
            path,
            dest,
            selection,
            best_effort,
            ..
        } => {
            let selection = selected_entries_label(selection);
            OperationAuditSummary {
                kind: "extract".into(),
                title: if *best_effort {
                    "Extract readable files".into()
                } else {
                    "Extract archive".into()
                },
                detail: format!("{} -> {} · {selection}", base(path), base(dest)),
            }
        }
        JobSpec::BatchExtract { items, .. } => OperationAuditSummary {
            kind: "batch_extract".into(),
            title: "Batch extract archives".into(),
            detail: format!(
                "{} archive{} -> {}",
                items.len(),
                plural(items.len()),
                batch_dest_label(items)
            ),
        },
        JobSpec::ExtractNested {
            outer_path,
            entry_path,
            dest,
            ..
        } => OperationAuditSummary {
            kind: "extract_nested".into(),
            title: "Extract nested archive".into(),
            detail: format!(
                "{}:{} -> {}",
                base(outer_path),
                entry_leaf(entry_path),
                base(dest)
            ),
        },
        JobSpec::Test { path, .. } => OperationAuditSummary {
            kind: "test".into(),
            title: "Test archive".into(),
            detail: base(path),
        },
        JobSpec::Convert { src, dest, .. } => OperationAuditSummary {
            kind: "convert".into(),
            title: "Convert archive".into(),
            detail: format!("{} -> {}", base(src), base(dest)),
        },
        JobSpec::ExportSqz { src, dest, .. } => OperationAuditSummary {
            kind: "export_sqz".into(),
            title: "Export SQZ".into(),
            detail: format!("{} -> {}", base(src), base(dest)),
        },
        JobSpec::RepairSqz { src, dest, .. } => OperationAuditSummary {
            kind: "repair_sqz".into(),
            title: "Repair SQZ".into(),
            detail: format!("{} -> {}", base(src), base(dest)),
        },
        JobSpec::RepairZip { src, dest, .. } => OperationAuditSummary {
            kind: "repair_zip".into(),
            title: "Repair ZIP index".into(),
            detail: format!("{} -> {}", base(src), base(dest)),
        },
        JobSpec::Protect {
            path, redundancy, ..
        } => OperationAuditSummary {
            kind: "protect".into(),
            title: "Protect archive".into(),
            detail: format!("{} · {}% recovery", base(path), redundancy),
        },
        JobSpec::VerifyRecovery { path, .. } => OperationAuditSummary {
            kind: "verify_recovery".into(),
            title: "Verify recovery".into(),
            detail: base(path),
        },
        JobSpec::RepairRecovery { path, output, .. } => OperationAuditSummary {
            kind: "repair_recovery".into(),
            title: "Repair with recovery".into(),
            detail: repair_recovery_detail(path, output),
        },
        JobSpec::Update {
            path,
            add,
            delete,
            rename,
            mkdir,
            ..
        } => {
            let ops = add.len() + delete.len() + rename.len() + mkdir.len();
            OperationAuditSummary {
                kind: "update".into(),
                title: "Update archive".into(),
                detail: format!("{} · {} operation{}", base(path), ops, plural(ops)),
            }
        }
        JobSpec::Checksum {
            inputs, algorithm, ..
        } => OperationAuditSummary {
            kind: "checksum".into(),
            title: "Compute checksums".into(),
            detail: format!(
                "{} input{} · {}",
                inputs.len(),
                plural(inputs.len()),
                algorithm.id()
            ),
        },
        JobSpec::ChecksumCheck {
            manifest,
            algorithm,
        } => OperationAuditSummary {
            kind: "checksum_check".into(),
            title: "Verify checksum manifest".into(),
            detail: format!("{} · {}", base(manifest), algorithm.id()),
        },
        JobSpec::DuplicateScan {
            inputs, min_size, ..
        } => OperationAuditSummary {
            kind: "duplicate_scan".into(),
            title: "Find duplicate files".into(),
            detail: format!(
                "{} input{} · min {} bytes",
                inputs.len(),
                plural(inputs.len()),
                min_size
            ),
        },
    }
}

pub fn summarize_result(result: Option<&crate::jobs::JobResult>) -> Option<String> {
    use crate::jobs::JobResult;
    Some(match result? {
        JobResult::Compress(report) | JobResult::Convert(report) => {
            let total_bytes = report.output.total_bytes;
            let summary = if let Some(sfx) = &report.sfx {
                let signing = if sfx.requires_signing {
                    ", signing required"
                } else {
                    ""
                };
                format!("created self-extractor, {total_bytes} bytes{signing}")
            } else if report.output.split {
                let volumes = report.output.volume_count;
                format!(
                    "created {volumes} volume{}, {total_bytes} bytes",
                    plural(volumes)
                )
            } else {
                format!("created archive, {total_bytes} bytes")
            };
            append_source_cleanup_summary(summary, &report.source_cleanup)
        }
        JobResult::PublishMacosSfx(_) => "signed, notarized, stapled, and verified".into(),
        JobResult::BatchExtract(report) => {
            let (extracted, archives, failed) = (report.extracted, report.archives, report.failed);
            let skipped: u64 = report
                .outputs
                .iter()
                .map(|output| output.result.counts.skipped)
                .sum();
            format!(
                "{extracted}/{archives} archive{} extracted, {failed} failed, {skipped} skipped",
                plural(archives)
            )
        }
        JobResult::Extract(report) | JobResult::ExtractNested(report) => format!(
            "extracted {}, {} failed, {} skipped",
            report.counts.created, report.counts.failed, report.counts.skipped
        ),
        JobResult::Test(report) => format!(
            "tested {}, problems {}",
            report.entries_tested, report.problems_total
        ),
        JobResult::Update { operations, .. } => {
            format!("{operations} update operation{}", plural(*operations))
        }
        JobResult::Checksum(report) => format!(
            "{} file{} hashed, {} bytes, {}",
            report.files_hashed,
            plural(report.files_hashed),
            report.bytes_hashed,
            report.algorithm.id()
        ),
        JobResult::ChecksumCheck(report) => format!(
            "{}/{} passed, {} failed{}",
            report.report.passed,
            report.report.checked,
            report.report.failed,
            plural(report.report.failed)
        ),
        JobResult::DuplicateScan(report) => format!(
            "{} duplicate group{}, {} file{}, {} bytes reclaimable",
            report.duplicate_groups,
            plural(report.duplicate_groups),
            report.duplicate_files,
            plural(report.duplicate_files),
            report.reclaimable_bytes
        ),
        JobResult::Protect(report)
        | JobResult::VerifyRecovery(report)
        | JobResult::RepairRecovery(report) => {
            if report.ok {
                "ok".into()
            } else {
                "not ok".into()
            }
        }
        JobResult::ExportSqz(_) | JobResult::RepairSqz { .. } | JobResult::RepairZip { .. } => {
            return None
        }
    })
}

fn append_source_cleanup_summary(
    mut summary: String,
    cleanup: &crate::jobs::SourceCleanupResult,
) -> String {
    let status = cleanup.status.as_str();
    if status != "not_requested" {
        summary.push_str(&format!(
            ", source cleanup {status}, {} moved, {} not moved, {} recovery required",
            cleanup.moved, cleanup.kept, cleanup.recovery_required
        ));
    }
    summary
}

fn base(path: &str) -> String {
    let path = Path::new(path);
    match path
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
    {
        Some(name) => shorten(name),
        None => shorten(path.to_string_lossy().as_ref()),
    }
}

fn entry_leaf(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    let leaf = match trimmed.rsplit('/').next().filter(|name| !name.is_empty()) {
        Some(name) => name,
        None => path,
    };
    shorten(leaf)
}

fn batch_dest_label(items: &[crate::dto::BatchExtractItem]) -> String {
    let first = match items.first() {
        Some(item) => base(&item.dest),
        None => "no destination".into(),
    };
    if items.len() <= 1 {
        return first;
    }
    format!("{first} + {} more", items.len().saturating_sub(1))
}

fn shorten(value: &str) -> String {
    const MAX: usize = 96;
    let mut chars = value.chars();
    let prefix: String = chars.by_ref().take(MAX).collect();
    if chars.next().is_some() {
        format!("{prefix}...")
    } else {
        prefix
    }
}

fn plural(count: usize) -> &'static str {
    if count == 1 {
        ""
    } else {
        "s"
    }
}

#[cfg(test)]
pub(crate) fn read_audit_records(path: &Path) -> Vec<serde_json::Value> {
    fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("squallz-gui-audit-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn audit_persists_records_across_writers() {
        let dir = temp_dir("persist");
        let path = dir.join("operation-audit.jsonl");
        let audit = OperationAudit::with_path(path.clone());
        audit
            .append(OperationAuditRecord {
                id: 1,
                time: 10,
                kind: "compress".into(),
                state: "done".into(),
                title: "Create archive".into(),
                detail: "one input -> first.zip".into(),
                result_summary: None,
                error_key: None,
            })
            .unwrap();
        audit
            .append(OperationAuditRecord {
                id: 2,
                time: 20,
                kind: "test".into(),
                state: "failed".into(),
                title: "Test archive".into(),
                detail: "bad.zip".into(),
                result_summary: None,
                error_key: Some("error.corrupt_archive".into()),
            })
            .unwrap();
        drop(audit);
        let audit = OperationAudit::with_path(path.clone());
        audit
            .append(OperationAuditRecord {
                id: 3,
                time: 30,
                kind: "extract".into(),
                state: "done".into(),
                title: "Extract archive".into(),
                detail: "ok.zip -> out".into(),
                result_summary: Some("skipped 0".into()),
                error_key: None,
            })
            .unwrap();

        let records = read_audit_records(&path);
        assert_eq!(records.len(), 3);
        assert_eq!(records[0]["id"], 1);
        assert_eq!(records[0]["detail"], "one input -> first.zip");
        assert_eq!(records[1]["id"], 2);
        assert_eq!(records[1]["state"], "failed");
        assert_eq!(records[1]["error_key"], "error.corrupt_archive");
        assert_eq!(records[2]["id"], 3);
        assert_eq!(records[2]["result_summary"], "skipped 0");
        assert!(records[2].get("error_key").is_none());

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn audit_concurrent_appends_remain_complete_json_lines() {
        let dir = temp_dir("concurrent");
        let path = dir.join("operation-audit.jsonl");
        let audit = std::sync::Arc::new(OperationAudit::with_path(path.clone()));
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(4));
        let detail = "concurrent\"record\n".repeat(1_024);
        let writers: Vec<_> = (0..4)
            .map(|worker| {
                let audit = std::sync::Arc::clone(&audit);
                let barrier = std::sync::Arc::clone(&barrier);
                let detail = detail.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    for index in 0..16 {
                        audit
                            .append(OperationAuditRecord {
                                id: worker * 16 + index,
                                time: 100,
                                kind: "test".into(),
                                state: "done".into(),
                                title: "Test archive".into(),
                                detail: detail.clone(),
                                result_summary: None,
                                error_key: None,
                            })
                            .unwrap();
                    }
                })
            })
            .collect();
        for writer in writers {
            writer.join().unwrap();
        }
        let records = read_audit_records(&path);
        assert_eq!(records.len(), 64);
        assert!(records.iter().all(|record| record["detail"] == detail));
        let mut ids: Vec<_> = records
            .iter()
            .map(|record| record["id"].as_u64().unwrap())
            .collect();
        ids.sort_unstable();
        assert_eq!(ids, (0..64).collect::<Vec<_>>());

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn audit_lock_recovers_after_poison() {
        let dir = temp_dir("poison");
        let path = dir.join("operation-audit.jsonl");
        let audit = std::sync::Arc::new(OperationAudit::with_path(path.clone()));
        let poisoned_audit = std::sync::Arc::clone(&audit);
        assert!(std::thread::spawn(move || {
            let _guard = poisoned_audit.write_lock.lock().unwrap();
            panic!("poison operation audit");
        })
        .join()
        .is_err());

        audit
            .append(OperationAuditRecord {
                id: 42,
                time: 100,
                kind: "test".into(),
                state: "done".into(),
                title: "Test archive".into(),
                detail: "archive.zip".into(),
                result_summary: Some("ok".into()),
                error_key: None,
            })
            .unwrap();
        let records = read_audit_records(&path);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0]["id"], 42);
        assert_eq!(records[0]["title"], "Test archive");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn job_summary_omits_passwords_and_full_paths() {
        let spec = JobSpec::Compress {
            inputs: vec!["/Users/example/Secret Folder".into()],
            dest: "/Users/example/Documents/private-backup.zip".into(),
            level: 5,
            password: Some("do-not-log".into()),
            encrypt_names: true,
            split_size: None,
            split_mode: squallz_core::api::SplitOutputMode::Generic,
            excludes: vec![],
            content_policy: squallz_core::CreateContentPolicy::KeepAllFiles,
            sqz_inner_format: None,
            sfx_target: None,
            completion: squallz_core::CreateCompletionAction::None,
            post_success: squallz_core::PostSuccessAction::KeepSource,
            test_after_create: false,
            replace_existing: false,
            replacement_guard: None,
        };
        let summary = summarize_job(&spec);
        assert_eq!(summary.kind, "compress");
        assert!(summary.detail.contains("private-backup.zip"));
        assert!(!summary.detail.contains("do-not-log"));
        assert!(!summary.detail.contains("/Users/example"));
    }

    #[test]
    fn batch_extract_summary_omits_passwords_and_full_paths() {
        let spec = JobSpec::BatchExtract {
            items: vec![
                crate::dto::BatchExtractItem {
                    path: "/Users/example/Downloads/private-client.zip".into(),
                    dest: "/Users/example/Downloads/private-client".into(),
                    encoding: None,
                    password: Some("do-not-log".into()),
                    best_effort: false,
                },
                crate::dto::BatchExtractItem {
                    path: "/Users/example/Downloads/secret-photos.7z".into(),
                    dest: "/Users/example/Downloads/secret-photos".into(),
                    encoding: Some("utf-8".into()),
                    password: Some("also-secret".into()),
                    best_effort: true,
                },
            ],
            overwrite: squallz_core::api::OverwritePolicy::Ask,
            symlinks: squallz_core::api::SymlinkPolicy::Preserve,
            smart: true,
        };
        let summary = summarize_job(&spec);
        assert_eq!(summary.kind, "batch_extract");
        assert_eq!(summary.title, "Batch extract archives");
        assert!(summary.detail.contains("2 archives"));
        assert!(summary.detail.contains("private-client"));
        assert!(!summary.detail.contains("do-not-log"));
        assert!(!summary.detail.contains("also-secret"));
        assert!(!summary.detail.contains("/Users/example"));

        use crate::jobs::{
            results::{BatchExtractOutput, BatchExtractResult},
            JobResult,
        };
        let mut output = crate::jobs::test_support::extract_report(Path::new("private-client"));
        output.counts.skipped = 3;
        let result = JobResult::BatchExtract(BatchExtractResult {
            archives: 2,
            selected_archives: 2,
            collapsed_volumes: 0,
            extracted: 1,
            failed: 1,
            recovered_archives: 0,
            outputs: vec![BatchExtractOutput {
                archive: "private-client.zip".into(),
                result: output,
            }],
            failures: Vec::new(),
        });
        assert_eq!(
            summarize_result(Some(&result)).as_deref(),
            Some("1/2 archives extracted, 1 failed, 3 skipped")
        );
    }

    #[test]
    fn create_result_summary_uses_counts_without_persisting_output_paths() {
        use crate::jobs::{results::SfxDetails, test_support::create_report, JobResult};
        let mut report = create_report(
            Path::new("/Users/example/Documents/private-backup.zip.001"),
            true,
            8192,
        );
        report.output.volume_count = 2;
        report
            .output
            .outputs
            .push("/Users/example/Documents/private-backup.zip.002".into());
        report.source_cleanup = crate::jobs::test_support::partial_cleanup();
        let result = JobResult::Compress(report);
        let summary = summarize_result(Some(&result)).expect("create result summary");
        assert_eq!(
            summary,
            "created 2 volumes, 8192 bytes, source cleanup partial, 1 moved, 1 not moved, 0 recovery required"
        );
        assert!(!summary.contains("private-backup"));
        assert!(!summary.contains("/Users/example"));

        let unsplit = JobResult::Compress(create_report(
            Path::new("/Users/example/Documents/private-backup.zip"),
            false,
            4096,
        ));
        assert_eq!(
            summarize_result(Some(&unsplit)).as_deref(),
            Some("created archive, 4096 bytes")
        );

        let mut report = create_report(
            Path::new("/Users/example/Documents/private-backup.app"),
            false,
            16384,
        );
        report.sfx = Some(SfxDetails {
            target: "macos",
            layout: "macos_app",
            payload_bytes: 1024,
            requires_signing: true,
        });
        let sfx = JobResult::Compress(report);
        assert_eq!(
            summarize_result(Some(&sfx)).as_deref(),
            Some("created self-extractor, 16384 bytes, signing required")
        );
    }

    #[test]
    fn test_summary_uses_the_exact_problem_total_instead_of_the_preview_length() {
        let report = squallz_core::reports::TestReport {
            ok: false,
            entries_tested: 25,
            problems: vec!["bad entry".into()],
            problems_total: 25,
            problems_truncated: true,
            recovery: None,
            structure: squallz_core::api::ArchiveStructureStatus::Complete,
        };
        assert_eq!(
            summarize_result(Some(&crate::jobs::JobResult::Test(report))).as_deref(),
            Some("tested 25, problems 25")
        );
    }
}
