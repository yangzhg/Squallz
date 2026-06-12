//! Persistent desktop operation audit log.
//!
//! The frontend keeps a rich local history for UX. This backend log is a
//! smaller, sanitized source of truth for completed GUI jobs: it records
//! operation kind, final state, timestamps, and path basenames, never
//! passwords or full user-selected path trees.

use std::collections::VecDeque;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use squallz_core::api::FormatError;

use crate::dto::JobSpec;

const DEFAULT_MAX_RECORDS: usize = 500;
const DEFAULT_EXPORT_FILE_NAME: &str = "operation-audit.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
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
    max_records: usize,
    records: Mutex<VecDeque<OperationAuditRecord>>,
}

fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn existing_records(path: Option<&Path>, max_records: usize) -> VecDeque<OperationAuditRecord> {
    match path {
        Some(path) => load_existing_records(path, max_records),
        None => VecDeque::new(),
    }
}

fn export_parent(path: &Path) -> &Path {
    match path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        Some(parent) => parent,
        None => Path::new("."),
    }
}

fn export_file_name(path: &Path) -> &str {
    match path.file_name().and_then(|name| name.to_str()) {
        Some(name) => name,
        None => DEFAULT_EXPORT_FILE_NAME,
    }
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

fn json_u64_or(value: &serde_json::Value, key: &str, fallback: u64) -> u64 {
    match value.get(key).and_then(|v| v.as_u64()) {
        Some(number) => number,
        None => fallback,
    }
}

fn json_str_or<'a>(value: &'a serde_json::Value, key: &str, fallback: &'a str) -> &'a str {
    match value.get(key).and_then(|v| v.as_str()) {
        Some(text) => text,
        None => fallback,
    }
}

impl OperationAudit {
    pub fn load() -> Self {
        let path = dirs::data_dir().map(|dir| dir.join("Squallz").join("operation-audit.jsonl"));
        Self::from_path(path, DEFAULT_MAX_RECORDS)
    }

    #[cfg(test)]
    pub fn memory() -> Self {
        Self {
            path: None,
            max_records: DEFAULT_MAX_RECORDS,
            records: Mutex::new(VecDeque::new()),
        }
    }

    #[cfg(test)]
    pub fn with_path(path: PathBuf, max_records: usize) -> Self {
        Self::from_path(Some(path), max_records)
    }

    fn from_path(path: Option<PathBuf>, max_records: usize) -> Self {
        let records = existing_records(path.as_deref(), max_records);
        Self {
            path,
            max_records,
            records: Mutex::new(records),
        }
    }

    pub fn append(&self, record: OperationAuditRecord) -> std::io::Result<()> {
        {
            let mut records = lock_unpoisoned(&self.records);
            records.push_back(record.clone());
            while records.len() > self.max_records {
                records.pop_front();
            }
        }

        if let Some(path) = &self.path {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            let mut file = OpenOptions::new().create(true).append(true).open(path)?;
            serde_json::to_writer(&mut file, &record)?;
            file.write_all(b"\n")?;
            file.flush()?;
        }
        Ok(())
    }

    pub fn recent(&self, limit: usize) -> Vec<OperationAuditRecord> {
        let limit = limit.clamp(1, self.max_records);
        lock_unpoisoned(&self.records)
            .iter()
            .rev()
            .take(limit)
            .cloned()
            .collect()
    }

    pub fn export_json(&self, path: &Path) -> Result<(), FormatError> {
        if path.is_dir() {
            return Err(FormatError::Unsupported(format!(
                "operation audit export target is a directory: {}",
                path.display()
            )));
        }
        let parent = export_parent(path);
        fs::create_dir_all(parent)?;
        let file_name = export_file_name(path);
        let tmp = parent.join(format!(".{file_name}.part-{}", std::process::id()));
        let write_result = (|| -> Result<(), FormatError> {
            let mut file = File::create(&tmp)?;
            let payload = serde_json::json!({
                "generatedAt": now_millis(),
                "records": self.recent(self.max_records),
            });
            serde_json::to_writer_pretty(&mut file, &payload).map_err(|e| {
                FormatError::Other(format!("cannot serialize operation audit: {e}"))
            })?;
            file.write_all(b"\n")?;
            file.sync_all()?;
            match fs::rename(&tmp, path) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    fs::remove_file(path)?;
                    fs::rename(&tmp, path)?;
                    Ok(())
                }
                Err(e) => Err(e.into()),
            }
        })();
        if write_result.is_err() {
            let _ = fs::remove_file(&tmp);
        }
        write_result
    }
}

pub fn now_millis() -> u64 {
    millis_since_epoch_or_zero(SystemTime::now())
}

pub fn summarize_job(spec: &JobSpec) -> OperationAuditSummary {
    match spec {
        JobSpec::Compress { inputs, dest, .. } => OperationAuditSummary {
            kind: "compress".into(),
            title: "Create archive".into(),
            detail: format!(
                "{} input{} -> {}",
                inputs.len(),
                plural(inputs.len()),
                base(dest)
            ),
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
                algorithm
            ),
        },
        JobSpec::ChecksumCheck {
            manifest,
            algorithm,
        } => OperationAuditSummary {
            kind: "checksum_check".into(),
            title: "Verify checksum manifest".into(),
            detail: format!("{} · {}", base(manifest), algorithm),
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

pub fn summarize_result(result: Option<&serde_json::Value>) -> Option<String> {
    let value = result?;
    if value.get("operation").and_then(|v| v.as_str()) == Some("batch_extract") {
        let archives = json_u64_or(value, "archives", 0);
        let extracted = json_u64_or(value, "extracted", 0);
        let failed = json_u64_or(value, "failed", 0);
        let skipped = json_u64_or(value, "skipped", 0);
        return Some(format!(
            "{extracted}/{archives} archive{} extracted, {failed} failed, {skipped} skipped",
            plural_u64(archives)
        ));
    }
    if let Some(skipped) = value.get("skipped").and_then(|v| v.as_u64()) {
        return Some(format!("skipped {skipped}"));
    }
    if let Some(entries) = value.get("entries").and_then(|v| v.as_u64()) {
        let problems = json_u64_or(value, "problems", 0);
        return Some(format!("tested {entries}, problems {problems}"));
    }
    if let Some(operations) = value.get("operations").and_then(|v| v.as_u64()) {
        return Some(format!(
            "{operations} update operation{}",
            plural_u64(operations)
        ));
    }
    if let Some(files) = value.get("files_hashed").and_then(|v| v.as_u64()) {
        let bytes = json_u64_or(value, "bytes_hashed", 0);
        let algorithm = json_str_or(value, "algorithm", "checksum");
        return Some(format!(
            "{files} file{} hashed, {bytes} bytes, {algorithm}",
            plural_u64(files)
        ));
    }
    if let Some(checked) = value.get("checked").and_then(|v| v.as_u64()) {
        let passed = json_u64_or(value, "passed", 0);
        let failed = json_u64_or(value, "failed", 0);
        return Some(format!(
            "{passed}/{checked} passed, {failed} failed{}",
            plural_u64(failed)
        ));
    }
    if let Some(groups) = value.get("duplicate_groups").and_then(|v| v.as_u64()) {
        let files = json_u64_or(value, "duplicate_files", 0);
        let reclaimable = json_u64_or(value, "reclaimable_bytes", 0);
        return Some(format!(
            "{groups} duplicate group{}, {files} file{}, {reclaimable} bytes reclaimable",
            plural_u64(groups),
            plural_u64(files)
        ));
    }
    if let Some(ok) = value.get("ok").and_then(|v| v.as_bool()) {
        return Some(if ok { "ok".into() } else { "not ok".into() });
    }
    None
}

fn load_existing_records(path: &Path, max_records: usize) -> VecDeque<OperationAuditRecord> {
    let Ok(file) = File::open(path) else {
        return VecDeque::new();
    };
    let mut records = VecDeque::new();
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(record) = serde_json::from_str::<OperationAuditRecord>(&line) {
            records.push_back(record);
            while records.len() > max_records {
                records.pop_front();
            }
        }
    }
    records
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

fn plural_u64(count: u64) -> &'static str {
    if count == 1 {
        ""
    } else {
        "s"
    }
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
    fn audit_persists_recent_records_and_exports_json() {
        let dir = temp_dir("persist");
        let path = dir.join("operation-audit.jsonl");
        let audit = OperationAudit::with_path(path.clone(), 2);
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

        let reloaded = OperationAudit::with_path(path, 2);
        let recent = reloaded.recent(10);
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].id, 3);
        assert_eq!(recent[1].id, 2);

        let exported = dir.join("audit-export.json");
        reloaded.export_json(&exported).unwrap();
        let written = std::fs::read_to_string(&exported).unwrap();
        assert!(written.contains("\"records\""));
        assert!(written.contains("\"Extract archive\""));
        assert!(!written.contains("first.zip"));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn audit_lock_recovers_after_poison() {
        let audit = std::sync::Arc::new(OperationAudit::memory());
        let poisoned_audit = std::sync::Arc::clone(&audit);
        assert!(std::thread::spawn(move || {
            let _guard = poisoned_audit.records.lock().unwrap();
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
        let recent = audit.recent(10);
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].id, 42);

        let dir = temp_dir("poison");
        let exported = dir.join("audit-export.json");
        audit.export_json(&exported).unwrap();
        let written = std::fs::read_to_string(&exported).unwrap();
        assert!(written.contains("\"Test archive\""));
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
            excludes: vec![],
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
            overwrite: "ask".into(),
            symlinks: "preserve".into(),
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

        let result = serde_json::json!({
            "operation": "batch_extract",
            "archives": 2,
            "extracted": 1,
            "failed": 1,
            "skipped": 3,
        });
        assert_eq!(
            summarize_result(Some(&result)).as_deref(),
            Some("1/2 archives extracted, 1 failed, 3 skipped")
        );
    }
}
