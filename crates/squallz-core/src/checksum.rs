//! Local-file checksum calculation shared by CLI and GUI surfaces.

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use sha2::Digest;

use crate::api::{ControlToken, EntryPath, EntryType, FormatError, NoProgress, ProgressSink};
use crate::{inputs, PathFilter};

const HASH_BUFFER_SIZE: usize = 128 * 1024;
const CHECKSUM_PROGRESS_STEP_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChecksumAlgorithm {
    Blake3,
    Md5,
    Sha1,
    Sha224,
    Sha256,
    Sha384,
    Sha512,
    Crc32,
}

impl ChecksumAlgorithm {
    pub fn parse_alias(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "blake3" | "b3" => Some(Self::Blake3),
            "md5" | "md-5" => Some(Self::Md5),
            "sha1" | "sha-1" => Some(Self::Sha1),
            "sha224" | "sha-224" => Some(Self::Sha224),
            "sha256" | "sha-256" => Some(Self::Sha256),
            "sha384" | "sha-384" => Some(Self::Sha384),
            "sha512" | "sha-512" => Some(Self::Sha512),
            "crc32" | "crc-32" => Some(Self::Crc32),
            _ => None,
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Self::Blake3 => "blake3",
            Self::Md5 => "md5",
            Self::Sha1 => "sha1",
            Self::Sha224 => "sha224",
            Self::Sha256 => "sha256",
            Self::Sha384 => "sha384",
            Self::Sha512 => "sha512",
            Self::Crc32 => "crc32",
        }
    }

    fn digest_len(self) -> usize {
        match self {
            Self::Blake3 | Self::Sha256 => 64,
            Self::Md5 => 32,
            Self::Sha1 => 40,
            Self::Sha224 => 56,
            Self::Sha384 => 96,
            Self::Sha512 => 128,
            Self::Crc32 => 8,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChecksumItem {
    pub path: PathBuf,
    pub size: u64,
    pub digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChecksumReport {
    pub algorithm: ChecksumAlgorithm,
    pub input_count: usize,
    pub entries_scanned: usize,
    pub files_hashed: usize,
    pub bytes_hashed: u64,
    pub items: Vec<ChecksumItem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChecksumVerificationItem {
    pub path: PathBuf,
    pub expected: String,
    pub actual: Option<String>,
    pub ok: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChecksumVerificationReport {
    pub algorithm: ChecksumAlgorithm,
    pub manifest: PathBuf,
    pub checked: usize,
    pub passed: usize,
    pub failed: usize,
    pub bytes_hashed: u64,
    pub items: Vec<ChecksumVerificationItem>,
}

impl ChecksumVerificationReport {
    pub fn is_ok(&self) -> bool {
        self.failed == 0
    }
}

pub(crate) fn checksum_files(
    inputs: &[PathBuf],
    excludes: &[String],
    algorithm: ChecksumAlgorithm,
) -> Result<ChecksumReport, FormatError> {
    let progress = NoProgress;
    let ctl = ControlToken::new();
    checksum_files_with_progress(inputs, excludes, algorithm, &progress, &ctl)
}

pub(crate) fn checksum_files_with_progress(
    inputs: &[PathBuf],
    excludes: &[String],
    algorithm: ChecksumAlgorithm,
    progress: &dyn ProgressSink,
    ctl: &ControlToken,
) -> Result<ChecksumReport, FormatError> {
    ctl.checkpoint()?;
    let filter = PathFilter::new(excludes)?;
    let items = inputs::collect_inputs(inputs, &filter)?;
    let entries_scanned = items.len();
    ctl.checkpoint()?;

    let total_bytes = items
        .iter()
        .filter(|item| item.entry_type == EntryType::File)
        .fold(0u64, |sum, item| sum.saturating_add(item.size));
    let mut report = ChecksumReport {
        algorithm,
        input_count: inputs.len(),
        entries_scanned,
        files_hashed: 0,
        bytes_hashed: 0,
        items: Vec::new(),
    };
    if total_bytes == 0 {
        progress.on_progress(0, 0, &EntryPath::from_utf8("No files to checksum"));
    }

    let mut progress_state = ChecksumProgressState::new(progress, total_bytes);
    for item in items {
        if item.entry_type != EntryType::File {
            continue;
        }
        if report.files_hashed == 0 {
            progress_state.emit_current(&item.name, true);
        }
        ctl.checkpoint()?;
        let hashed = checksum_file(&item.src, algorithm, &item.name, &mut progress_state, ctl)?;
        report.files_hashed += 1;
        report.bytes_hashed = report.bytes_hashed.saturating_add(hashed.bytes_hashed);
        report.items.push(ChecksumItem {
            path: item.src,
            size: item.size,
            digest: hashed.digest,
        });
    }
    Ok(report)
}

pub(crate) fn verify_checksum_manifest(
    manifest: &Path,
    algorithm: ChecksumAlgorithm,
) -> Result<ChecksumVerificationReport, FormatError> {
    let progress = NoProgress;
    let ctl = ControlToken::new();
    verify_checksum_manifest_with_progress(manifest, algorithm, &progress, &ctl)
}

pub(crate) fn verify_checksum_manifest_with_progress(
    manifest: &Path,
    algorithm: ChecksumAlgorithm,
    progress: &dyn ProgressSink,
    ctl: &ControlToken,
) -> Result<ChecksumVerificationReport, FormatError> {
    ctl.checkpoint()?;
    let text = std::fs::read_to_string(manifest)?;
    let base_dir = match manifest
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
    {
        Some(path) => path,
        None => Path::new("."),
    };
    let expected = parse_manifest(&text, base_dir, algorithm)?;
    ctl.checkpoint()?;

    let total_bytes = expected
        .iter()
        .filter_map(|item| std::fs::metadata(&item.path).ok())
        .filter(|meta| meta.is_file())
        .fold(0u64, |sum, meta| sum.saturating_add(meta.len()));
    let mut report = ChecksumVerificationReport {
        algorithm,
        manifest: manifest.to_path_buf(),
        checked: 0,
        passed: 0,
        failed: 0,
        bytes_hashed: 0,
        items: Vec::new(),
    };
    if total_bytes == 0 {
        progress.on_progress(0, 0, &EntryPath::from_utf8("No files to verify"));
    }

    let mut progress_state = ChecksumProgressState::new(progress, total_bytes);
    for item in expected {
        ctl.checkpoint()?;
        report.checked += 1;
        match std::fs::metadata(&item.path) {
            Ok(meta) if meta.is_file() => {
                let current = EntryPath::from_utf8(item.path.to_string_lossy().into_owned());
                if report.passed == 0 && report.failed == 0 {
                    progress_state.emit_current(&current, true);
                }
                let hashed =
                    checksum_file(&item.path, algorithm, &current, &mut progress_state, ctl)?;
                report.bytes_hashed = report.bytes_hashed.saturating_add(hashed.bytes_hashed);
                let actual = hashed.digest;
                let ok = actual.eq_ignore_ascii_case(&item.expected);
                if ok {
                    report.passed += 1;
                } else {
                    report.failed += 1;
                }
                report.items.push(ChecksumVerificationItem {
                    path: item.path,
                    expected: item.expected,
                    actual: Some(actual),
                    ok,
                    error: None,
                });
            }
            Ok(_) => {
                report.failed += 1;
                report.items.push(ChecksumVerificationItem {
                    path: item.path,
                    expected: item.expected,
                    actual: None,
                    ok: false,
                    error: Some("not a regular file".to_owned()),
                });
            }
            Err(e) => {
                report.failed += 1;
                report.items.push(ChecksumVerificationItem {
                    path: item.path,
                    expected: item.expected,
                    actual: None,
                    ok: false,
                    error: Some(e.to_string()),
                });
            }
        }
    }
    Ok(report)
}

struct HashedFile {
    digest: String,
    bytes_hashed: u64,
}

struct ChecksumProgressState<'a> {
    sink: &'a dyn ProgressSink,
    total: u64,
    done: u64,
    next_emit_at: u64,
}

impl<'a> ChecksumProgressState<'a> {
    fn new(sink: &'a dyn ProgressSink, total: u64) -> Self {
        Self {
            sink,
            total,
            done: 0,
            next_emit_at: CHECKSUM_PROGRESS_STEP_BYTES,
        }
    }

    fn add_bytes(&mut self, bytes: u64, current: &EntryPath) {
        self.done = self.done.saturating_add(bytes);
        self.emit_current(current, false);
    }

    fn emit_current(&mut self, current: &EntryPath, force: bool) {
        if !force && self.done < self.next_emit_at {
            return;
        }
        let displayed_done = if self.total == 0 {
            self.done
        } else {
            self.done.min(self.total)
        };
        self.sink.on_progress(displayed_done, self.total, current);
        self.next_emit_at = self.done.saturating_add(CHECKSUM_PROGRESS_STEP_BYTES);
    }
}

struct ExpectedChecksum {
    path: PathBuf,
    expected: String,
}

fn parse_manifest(
    text: &str,
    base_dir: &Path,
    algorithm: ChecksumAlgorithm,
) -> Result<Vec<ExpectedChecksum>, FormatError> {
    let mut out = Vec::new();
    for (idx, raw_line) in text.lines().enumerate() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (digest, path) = line.split_once(char::is_whitespace).ok_or_else(|| {
            FormatError::Other(format!(
                "invalid checksum manifest line {}: expected '<digest>  <path>'",
                idx + 1
            ))
        })?;
        let digest = digest.trim().to_ascii_lowercase();
        if digest.len() != algorithm.digest_len()
            || !digest.chars().all(|ch| ch.is_ascii_hexdigit())
        {
            return Err(FormatError::Other(format!(
                "invalid {} digest on manifest line {}",
                algorithm.id(),
                idx + 1
            )));
        }
        let path = path.trim_start_matches(|ch: char| ch.is_whitespace() || ch == '*');
        if path.is_empty() {
            return Err(FormatError::Other(format!(
                "invalid checksum manifest line {}: missing path",
                idx + 1
            )));
        }
        let path = PathBuf::from(path);
        let path = if path.is_absolute() {
            path
        } else {
            base_dir.join(path)
        };
        out.push(ExpectedChecksum {
            path,
            expected: digest,
        });
    }
    if out.is_empty() {
        return Err(FormatError::Other(
            "checksum manifest did not contain any entries".into(),
        ));
    }
    Ok(out)
}

fn checksum_file(
    path: &Path,
    algorithm: ChecksumAlgorithm,
    current: &EntryPath,
    progress: &mut ChecksumProgressState<'_>,
    ctl: &ControlToken,
) -> Result<HashedFile, FormatError> {
    ctl.checkpoint()?;
    let mut file = File::open(path)?;
    let mut state = ChecksumState::new(algorithm);
    let mut bytes_hashed = 0u64;
    let mut buf = [0u8; HASH_BUFFER_SIZE];
    loop {
        ctl.checkpoint()?;
        let read = file.read(&mut buf)?;
        if read == 0 {
            break;
        }
        state.update(&buf[..read]);
        let read = read as u64;
        bytes_hashed = bytes_hashed.saturating_add(read);
        progress.add_bytes(read, current);
        ctl.checkpoint()?;
    }
    progress.emit_current(current, true);
    ctl.checkpoint()?;
    Ok(HashedFile {
        digest: state.finalize(),
        bytes_hashed,
    })
}

enum ChecksumState {
    Blake3(Box<blake3::Hasher>),
    Md5(md5::Md5),
    Sha1(sha1::Sha1),
    Sha224(sha2::Sha224),
    Sha256(sha2::Sha256),
    Sha384(sha2::Sha384),
    Sha512(sha2::Sha512),
    Crc32(crc32fast::Hasher),
}

impl ChecksumState {
    fn new(algorithm: ChecksumAlgorithm) -> Self {
        match algorithm {
            ChecksumAlgorithm::Blake3 => Self::Blake3(Box::new(blake3::Hasher::new())),
            ChecksumAlgorithm::Md5 => Self::Md5(md5::Md5::new()),
            ChecksumAlgorithm::Sha1 => Self::Sha1(sha1::Sha1::new()),
            ChecksumAlgorithm::Sha224 => Self::Sha224(sha2::Sha224::new()),
            ChecksumAlgorithm::Sha256 => Self::Sha256(sha2::Sha256::new()),
            ChecksumAlgorithm::Sha384 => Self::Sha384(sha2::Sha384::new()),
            ChecksumAlgorithm::Sha512 => Self::Sha512(sha2::Sha512::new()),
            ChecksumAlgorithm::Crc32 => Self::Crc32(crc32fast::Hasher::new()),
        }
    }

    fn update(&mut self, bytes: &[u8]) {
        match self {
            Self::Blake3(hasher) => {
                hasher.update(bytes);
            }
            Self::Md5(hasher) => {
                hasher.update(bytes);
            }
            Self::Sha1(hasher) => {
                hasher.update(bytes);
            }
            Self::Sha224(hasher) => {
                hasher.update(bytes);
            }
            Self::Sha256(hasher) => {
                hasher.update(bytes);
            }
            Self::Sha384(hasher) => {
                hasher.update(bytes);
            }
            Self::Sha512(hasher) => {
                hasher.update(bytes);
            }
            Self::Crc32(hasher) => {
                hasher.update(bytes);
            }
        }
    }

    fn finalize(self) -> String {
        match self {
            Self::Blake3(hasher) => hasher.finalize().to_hex().to_string(),
            Self::Md5(hasher) => format!("{:x}", hasher.finalize()),
            Self::Sha1(hasher) => format!("{:x}", hasher.finalize()),
            Self::Sha224(hasher) => format!("{:x}", hasher.finalize()),
            Self::Sha256(hasher) => format!("{:x}", hasher.finalize()),
            Self::Sha384(hasher) => format!("{:x}", hasher.finalize()),
            Self::Sha512(hasher) => format!("{:x}", hasher.finalize()),
            Self::Crc32(hasher) => format!("{:08x}", hasher.finalize()),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::{
        checksum_file, checksum_files_with_progress, verify_checksum_manifest, ChecksumAlgorithm,
        ChecksumProgressState, CHECKSUM_PROGRESS_STEP_BYTES, HASH_BUFFER_SIZE,
    };
    use crate::api::{ControlToken, EntryPath, FormatError, NoProgress, ProgressSink};

    fn temp_file(name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let path =
            std::env::temp_dir().join(format!("squallz-checksum-{}-{name}", std::process::id()));
        std::fs::write(&path, bytes).expect("write checksum fixture");
        path
    }

    fn digest_file(path: &std::path::Path, algorithm: ChecksumAlgorithm) -> String {
        let progress = NoProgress;
        let ctl = ControlToken::new();
        let mut progress_state =
            ChecksumProgressState::new(&progress, std::fs::metadata(path).unwrap().len());
        checksum_file(
            path,
            algorithm,
            &EntryPath::from_utf8(path.display().to_string()),
            &mut progress_state,
            &ctl,
        )
        .unwrap()
        .digest
    }

    #[derive(Default)]
    struct RecordingProgress {
        events: Mutex<Vec<(u64, u64, String)>>,
    }

    impl ProgressSink for RecordingProgress {
        fn on_progress(&self, done: u64, total: u64, current: &EntryPath) {
            self.events
                .lock()
                .unwrap()
                .push((done, total, current.display.clone()));
        }
    }

    struct CancelAfterFirstChunk {
        ctl: Arc<ControlToken>,
        events: Mutex<Vec<u64>>,
    }

    impl ProgressSink for CancelAfterFirstChunk {
        fn on_progress(&self, done: u64, _total: u64, _current: &EntryPath) {
            self.events.lock().unwrap().push(done);
            if done > 0 {
                self.ctl.cancel();
            }
        }
    }

    #[test]
    fn checksum_algorithms_match_known_vectors() {
        let path = temp_file("abc.txt", b"abc");

        let cases = [
            (
                ChecksumAlgorithm::Md5,
                "900150983cd24fb0d6963f7d28e17f72",
            ),
            (
                ChecksumAlgorithm::Sha1,
                "a9993e364706816aba3e25717850c26c9cd0d89d",
            ),
            (
                ChecksumAlgorithm::Sha224,
                "23097d223405d8228642a477bda255b32aadbce4bda0b3f7e36c9da7",
            ),
            (
                ChecksumAlgorithm::Sha256,
                "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            ),
            (
                ChecksumAlgorithm::Sha384,
                "cb00753f45a35e8bb5a03d699ac65007272c32ab0eded1631a8b605a43ff5bed8086072ba1e7cc2358baeca134c825a7",
            ),
            (
                ChecksumAlgorithm::Sha512,
                "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f",
            ),
            (
                ChecksumAlgorithm::Blake3,
                "6437b3ac38465133ffb63b75273a8db548c558465d79db03fd359c6cd5bd9d85",
            ),
            (ChecksumAlgorithm::Crc32, "352441c2"),
        ];

        for (algorithm, expected) in cases {
            assert_eq!(
                digest_file(&path, algorithm),
                expected,
                "{}",
                algorithm.id()
            );
        }

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn checksum_files_reports_byte_progress_to_completion() {
        let path = temp_file("progress.bin", b"progress");
        let progress = RecordingProgress::default();
        let ctl = ControlToken::new();

        let report = checksum_files_with_progress(
            std::slice::from_ref(&path),
            &[],
            ChecksumAlgorithm::Sha256,
            &progress,
            &ctl,
        )
        .unwrap();

        assert_eq!(report.files_hashed, 1);
        assert_eq!(report.bytes_hashed, 8);
        let events = progress.events.lock().unwrap();
        assert!(events.iter().any(|(_, total, _)| *total == 8));
        assert_eq!(
            events.last().map(|(done, total, _)| (*done, *total)),
            Some((8, 8))
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn checksum_files_can_cancel_during_large_file_hashing() {
        let bytes = vec![0x5a; (CHECKSUM_PROGRESS_STEP_BYTES as usize) + HASH_BUFFER_SIZE];
        let path = temp_file("cancel.bin", &bytes);
        let ctl = ControlToken::new();
        let progress = CancelAfterFirstChunk {
            ctl: Arc::clone(&ctl),
            events: Mutex::new(Vec::new()),
        };

        let err = checksum_files_with_progress(
            std::slice::from_ref(&path),
            &[],
            ChecksumAlgorithm::Sha256,
            &progress,
            &ctl,
        )
        .unwrap_err();

        assert!(matches!(err, FormatError::Cancelled));
        assert!(progress
            .events
            .lock()
            .unwrap()
            .iter()
            .any(|done| *done > 0 && *done < bytes.len() as u64));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn verify_checksum_manifest_resolves_parentless_manifest_from_current_dir() {
        let tag = format!("{}-parentless", std::process::id());
        let payload = std::path::PathBuf::from(format!("squallz-checksum-payload-{tag}.txt"));
        let manifest = std::path::PathBuf::from(format!("squallz-checksum-manifest-{tag}.txt"));
        std::fs::write(&payload, b"abc").unwrap();
        let digest = digest_file(&payload, ChecksumAlgorithm::Sha256);
        let payload_name = payload.file_name().unwrap().to_string_lossy();
        std::fs::write(&manifest, format!("{digest}  {payload_name}\n")).unwrap();

        let report = verify_checksum_manifest(&manifest, ChecksumAlgorithm::Sha256).unwrap();
        assert!(report.is_ok());
        assert_eq!(report.checked, 1);
        assert_eq!(report.passed, 1);
        assert_eq!(report.failed, 0);

        std::fs::remove_file(&manifest).unwrap();
        std::fs::remove_file(&payload).unwrap();
    }

    #[test]
    fn checksum_algorithm_aliases_cover_cli_and_gui_values() {
        let cases = [
            ("md5", ChecksumAlgorithm::Md5),
            ("md-5", ChecksumAlgorithm::Md5),
            ("sha1", ChecksumAlgorithm::Sha1),
            ("sha-1", ChecksumAlgorithm::Sha1),
            ("sha224", ChecksumAlgorithm::Sha224),
            ("sha-224", ChecksumAlgorithm::Sha224),
            ("sha256", ChecksumAlgorithm::Sha256),
            ("sha-256", ChecksumAlgorithm::Sha256),
            ("sha384", ChecksumAlgorithm::Sha384),
            ("sha-384", ChecksumAlgorithm::Sha384),
            ("sha512", ChecksumAlgorithm::Sha512),
            ("sha-512", ChecksumAlgorithm::Sha512),
            ("blake3", ChecksumAlgorithm::Blake3),
            ("b3", ChecksumAlgorithm::Blake3),
            ("crc32", ChecksumAlgorithm::Crc32),
            ("crc-32", ChecksumAlgorithm::Crc32),
        ];

        for (alias, algorithm) in cases {
            assert_eq!(ChecksumAlgorithm::parse_alias(alias), Some(algorithm));
        }
        assert_eq!(ChecksumAlgorithm::parse_alias("sha3-256"), None);
    }
}
