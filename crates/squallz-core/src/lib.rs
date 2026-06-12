#![forbid(unsafe_code)]
//! squallz-core: the engine layer.
//!
//! Exposes a format-agnostic high-level API ([`Engine`]) to the CLI/GUI,
//! hiding the registry, compound-format and split-volume details. core
//! never depends on a concrete format implementation.

pub use squallz_format_api as api;

mod checksum;
mod compound;
mod convert;
mod create;
mod duplicates;
mod filter;
mod inputs;
mod layout;
mod queue;
mod volumes;

pub use checksum::{
    ChecksumAlgorithm, ChecksumItem, ChecksumReport, ChecksumVerificationItem,
    ChecksumVerificationReport,
};
pub use duplicates::{DuplicateGroup, DuplicateScanReport};
pub use filter::PathFilter;
pub use layout::{analyze_extract_layout, SmartLayout};
pub use queue::{Job, JobId, JobProgress, JobQueue, JobState};
pub use volumes::{collect_volume_set, VolumeSet};

use std::fs::{self, File};
use std::io::SeekFrom;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use api::{
    ArchiveReader, ControlToken, CreateOptions, EntryMeta, EntryPath, EntryType, ExtractOptions,
    FormatError, FormatInfo, FormatRegistry, OpenOptions, ProgressSink, ReadSeek, TestReport,
    UpdateOp,
};
use compound::{decompress_factory, SingleFileArchiveReader};
use volumes::MultiVolumeReader;

/// Engine: owns the registry and provides the high-level list/extract/
/// create/update/convert/test operations.
pub struct Engine {
    registry: FormatRegistry,
}

/// Preflight summary for local inputs before a create/update-add job starts.
///
/// This is intentionally an input-side estimate only: it never guesses the
/// compressed output size.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CreateInputEstimate {
    pub input_count: usize,
    pub entries: usize,
    pub files: usize,
    pub directories: usize,
    pub symlinks: usize,
    pub total_bytes: u64,
}

impl CreateInputEstimate {
    /// Conservative disk budget for create/update preflight.
    ///
    /// This is not a compressed output-size prediction. It reserves input
    /// bytes plus metadata/rewrite headroom so CLI and GUI use the same
    /// destination/temp-space guardrail before starting a write-heavy job.
    pub fn output_budget_bytes(self) -> u64 {
        const BASE_SLACK: u64 = 1024 * 1024;
        const ENTRY_SLACK: u64 = 1024;
        const INPUT_ROOT_SLACK: u64 = 4096;

        let entry_slack = (self.entries as u64).saturating_mul(ENTRY_SLACK);
        let root_slack = (self.input_count as u64).saturating_mul(INPUT_ROOT_SLACK);
        let metadata_slack = BASE_SLACK
            .saturating_add(entry_slack)
            .saturating_add(root_slack);
        self.total_bytes.saturating_add(metadata_slack)
    }
}

/// Physical source of an archive: a single file or a `.001` volume set.
#[derive(Clone)]
pub(crate) enum Source {
    Single(PathBuf),
    Volumes { base: PathBuf, parts: VolumeSet },
}

impl Source {
    /// Resolves a path, expanding split volumes (any `x.zip.NNN` opens the
    /// whole gap-checked set).
    fn resolve(path: &Path) -> Result<Self, FormatError> {
        match volumes::volume_base(path) {
            Some(base) => {
                let parts = collect_volume_set(path)?;
                Ok(Self::Volumes { base, parts })
            }
            None => Ok(Self::Single(path.to_path_buf())),
        }
    }

    /// Opens a fresh seekable stream over the source.
    pub(crate) fn open_stream(&self) -> Result<Box<dyn ReadSeek>, FormatError> {
        match self {
            Self::Single(path) => Ok(Box::new(File::open(path)?)),
            Self::Volumes { parts, .. } => Ok(Box::new(MultiVolumeReader::open(parts)?)),
        }
    }

    /// Path used for naming and format detection (volume sets detect under
    /// their base name, `x.zip.001` → `x.zip`).
    fn display_path(&self) -> &Path {
        match self {
            Self::Single(path) => path,
            Self::Volumes { base, .. } => base,
        }
    }
}

impl Engine {
    /// Builds an engine from the given registry (provided by
    /// squallz-formats).
    pub fn new(registry: FormatRegistry) -> Self {
        Self { registry }
    }

    /// Accesses the registry.
    pub fn registry(&self) -> &FormatRegistry {
        &self.registry
    }

    /// Opens an archive and returns a read handle. Split volume sets
    /// (`x.zip.001`) are reassembled transparently.
    pub fn open(
        &self,
        path: &Path,
        opts: &OpenOptions,
    ) -> Result<Box<dyn ArchiveReader>, FormatError> {
        let source = Source::resolve(path)?;
        let mut stream = source.open_stream()?;
        let (head, tail) = sniff_window(&mut *stream)?;
        let name = source
            .display_path()
            .file_name()
            .and_then(|s| s.to_str())
            .map(str::to_owned);
        match self.registry.detect(name.as_deref(), &head, &tail) {
            Some(api::Detected::Archive(f)) => f.open(stream, opts),
            Some(api::Detected::Compressed {
                compressor,
                inner_archive,
            }) => {
                let factory = decompress_factory(&source, Arc::clone(&compressor));
                match inner_archive {
                    // Compound (x.tar.gz): the inner archive reads the
                    // restartable decompressed stream — no temp file.
                    Some(archive) => archive.open_stream(factory, opts),
                    // Plain single stream (x.gz): single-entry virtual
                    // archive named after the file minus the extension.
                    None => {
                        let mut hint = 0;
                        if let Some(size) = compressor.uncompressed_size_hint(&mut *stream) {
                            hint = size;
                        }
                        Ok(Box::new(SingleFileArchiveReader::new(
                            source.display_path(),
                            factory,
                            hint,
                        )))
                    }
                }
            }
            None => Err(FormatError::Unsupported(format!(
                "unrecognized format: {}",
                path.display()
            ))),
        }
    }

    /// Lists entries.
    pub fn list(&self, path: &Path, opts: &OpenOptions) -> Result<Vec<EntryMeta>, FormatError> {
        let mut reader = self.open(path, opts)?;
        reader.entries().collect()
    }

    /// Extracts everything or a selection of entries.
    #[allow(clippy::too_many_arguments)] // engine facade: each argument has a distinct role
    pub fn extract(
        &self,
        path: &Path,
        dest: &Path,
        selection: Option<&[EntryPath]>,
        open_opts: &OpenOptions,
        extract_opts: &ExtractOptions,
        progress: &dyn ProgressSink,
        ctl: &ControlToken,
    ) -> Result<(), FormatError> {
        let mut reader = self.open(path, open_opts)?;
        reader.extract(dest, selection, extract_opts, progress, ctl)
    }

    /// Integrity test.
    pub fn test(
        &self,
        path: &Path,
        opts: &OpenOptions,
        progress: &dyn ProgressSink,
        ctl: &ControlToken,
    ) -> Result<TestReport, FormatError> {
        let mut reader = self.open(path, opts)?;
        reader.test(progress, ctl)
    }

    /// Creates an archive. The output format is chosen by the extension of
    /// `dest` (compound suffixes like `.tar.gz` / aliases like `.tgz`
    /// included); `opts.excludes` globs prune the inputs. With
    /// `opts.split_size`, the output is cut into `dest.001`, `dest.002`,
    /// ... byte-split volumes.
    pub fn create(
        &self,
        dest: &Path,
        inputs: &[PathBuf],
        opts: &CreateOptions,
        progress: &dyn ProgressSink,
        ctl: &ControlToken,
    ) -> Result<(), FormatError> {
        create::create(self, dest, inputs, opts, progress, ctl)
    }

    /// Walks local inputs with the same exclude semantics as archive creation
    /// and returns a non-compression estimate for the UI/preflight layer.
    pub fn estimate_create_inputs(
        &self,
        inputs: &[PathBuf],
        excludes: &[String],
    ) -> Result<CreateInputEstimate, FormatError> {
        self.estimate_create_inputs_with_progress(inputs, excludes, |_count, _path| {})
    }

    /// Same as [`Engine::estimate_create_inputs`], reporting each kept entry as
    /// it is discovered so the GUI can show large-directory preflight progress.
    pub fn estimate_create_inputs_with_progress(
        &self,
        inputs: &[PathBuf],
        excludes: &[String],
        mut progress: impl FnMut(usize, &str),
    ) -> Result<CreateInputEstimate, FormatError> {
        let filter = PathFilter::new(excludes)?;
        let items = inputs::collect_inputs_with_progress(inputs, &filter, |count, path| {
            progress(count, &path.display);
        })?;
        let mut estimate = CreateInputEstimate {
            input_count: inputs.len(),
            ..CreateInputEstimate::default()
        };
        for item in items {
            estimate.entries += 1;
            match item.entry_type {
                EntryType::File => {
                    estimate.files += 1;
                    estimate.total_bytes = estimate.total_bytes.saturating_add(item.size);
                }
                EntryType::Dir => estimate.directories += 1,
                EntryType::Symlink { .. } => estimate.symlinks += 1,
                _ => {}
            }
        }
        Ok(estimate)
    }

    /// Finds duplicate local files with the same input walking and exclude
    /// semantics used by archive creation.
    pub fn find_duplicate_files(
        &self,
        inputs: &[PathBuf],
        excludes: &[String],
        min_size: u64,
    ) -> Result<DuplicateScanReport, FormatError> {
        duplicates::find_duplicates(inputs, excludes, min_size)
    }

    /// Computes checksums for local files and recursively scanned folders,
    /// using the same exclude semantics as archive creation.
    pub fn checksum_files(
        &self,
        inputs: &[PathBuf],
        excludes: &[String],
        algorithm: ChecksumAlgorithm,
    ) -> Result<ChecksumReport, FormatError> {
        checksum::checksum_files(inputs, excludes, algorithm)
    }

    /// Computes checksums with chunk-level progress and cancellation.
    pub fn checksum_files_with_progress(
        &self,
        inputs: &[PathBuf],
        excludes: &[String],
        algorithm: ChecksumAlgorithm,
        progress: &dyn ProgressSink,
        ctl: &ControlToken,
    ) -> Result<ChecksumReport, FormatError> {
        checksum::checksum_files_with_progress(inputs, excludes, algorithm, progress, ctl)
    }

    /// Verifies a `sha256sum`-style checksum manifest. Relative paths are
    /// resolved from the manifest file's parent directory.
    pub fn verify_checksum_manifest(
        &self,
        manifest: &Path,
        algorithm: ChecksumAlgorithm,
    ) -> Result<ChecksumVerificationReport, FormatError> {
        checksum::verify_checksum_manifest(manifest, algorithm)
    }

    /// Verifies a checksum manifest with chunk-level progress and cancellation.
    pub fn verify_checksum_manifest_with_progress(
        &self,
        manifest: &Path,
        algorithm: ChecksumAlgorithm,
        progress: &dyn ProgressSink,
        ctl: &ControlToken,
    ) -> Result<ChecksumVerificationReport, FormatError> {
        checksum::verify_checksum_manifest_with_progress(manifest, algorithm, progress, ctl)
    }

    /// Applies append/delete/rename operations to an existing archive
    /// (formats with `can_update`; the implementation contract is
    /// temp-file rewrite + atomic rename with a disk-space pre-check).
    pub fn update(
        &self,
        path: &Path,
        ops: &[UpdateOp],
        opts: &CreateOptions,
        progress: &dyn ProgressSink,
        ctl: &ControlToken,
    ) -> Result<(), FormatError> {
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| FormatError::Unsupported("invalid archive file name".into()))?;
        if api::split_volume_name(name).is_some() {
            return Err(FormatError::Unsupported(
                "updating split volume sets is not supported".into(),
            ));
        }
        match self.registry.detect_by_name(name) {
            Some(api::Detected::Archive(f)) => {
                if !f.capabilities().can_update {
                    return Err(FormatError::Unsupported(format!(
                        "format {} does not support updating",
                        f.id()
                    )));
                }
                f.update(path, ops, opts, progress, ctl)
            }
            _ => Err(FormatError::Unsupported(format!(
                "updating this format is not supported: {name}"
            ))),
        }
    }

    /// Converts an archive into another format, streaming entry by entry
    /// (no extraction to disk). `open_opts` applies to the source,
    /// `create_opts` (password, level, split) to the destination.
    #[allow(clippy::too_many_arguments)] // engine facade: distinct roles
    pub fn convert(
        &self,
        src: &Path,
        dest: &Path,
        open_opts: &OpenOptions,
        create_opts: &CreateOptions,
        progress: &dyn ProgressSink,
        ctl: &ControlToken,
    ) -> Result<(), FormatError> {
        convert::convert(self, src, dest, open_opts, create_opts, progress, ctl)
    }

    /// Converts an archive, but when `src` and `dest` name the same existing
    /// file, writes a same-directory temporary archive first and replaces the
    /// destination only after conversion succeeds.
    ///
    /// Returns `true` when the destination was replaced in place.
    #[allow(clippy::too_many_arguments)] // engine facade: distinct roles
    pub fn convert_with_atomic_replace(
        &self,
        src: &Path,
        dest: &Path,
        open_opts: &OpenOptions,
        create_opts: &CreateOptions,
        progress: &dyn ProgressSink,
        ctl: &ControlToken,
    ) -> Result<bool, FormatError> {
        if !same_existing_path(src, dest) {
            self.convert(src, dest, open_opts, create_opts, progress, ctl)?;
            return Ok(false);
        }
        if create_opts.split_size.is_some() {
            return Err(FormatError::Unsupported(
                "in-place conversion cannot produce split volumes".into(),
            ));
        }
        let tmp = sibling_temp_path(dest, "convert")?;
        let result = self
            .convert(src, &tmp, open_opts, create_opts, progress, ctl)
            .and_then(|()| replace_file(&tmp, dest));
        if result.is_err() {
            let _ = fs::remove_file(&tmp);
        }
        result.map(|()| true)
    }

    /// Folder-name stem of an archive path: the file name minus split
    /// suffix and recognized format extensions (`backup.tar.gz` →
    /// `backup`). Used by smart extraction to name the wrapping folder.
    pub fn archive_stem(&self, path: &Path) -> String {
        let name = match path.file_name() {
            Some(name) => name.to_string_lossy().into_owned(),
            None => String::new(),
        };
        let stem = self.registry.display_stem(&name);
        if stem.is_empty() {
            "extracted".to_string()
        } else {
            stem
        }
    }

    /// All supported formats (for `sqz info` / the GUI).
    pub fn supported_formats(&self) -> Vec<FormatInfo> {
        self.registry.formats()
    }
}

fn same_existing_path(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

fn sibling_temp_path(dest: &Path, purpose: &str) -> Result<PathBuf, FormatError> {
    let parent = match dest
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        Some(parent) => parent,
        None => Path::new("."),
    };
    let name = dest
        .file_name()
        .map(|name| name.to_string_lossy())
        .ok_or_else(|| FormatError::Unsupported("destination path has no file name".into()))?;
    for attempt in 0..1000u32 {
        let candidate = parent.join(format!(
            ".{name}.{purpose}-{}-{attempt}.tmp.{name}",
            std::process::id()
        ));
        if fs::symlink_metadata(&candidate).is_err() {
            return Ok(candidate);
        }
    }
    Err(FormatError::Unsupported(format!(
        "could not allocate a temporary path next to {}",
        dest.display()
    )))
}

fn replace_file(tmp: &Path, dest: &Path) -> Result<(), FormatError> {
    match fs::rename(tmp, dest) {
        Ok(()) => Ok(()),
        Err(_) if dest.exists() => {
            fs::remove_file(dest)?;
            fs::rename(tmp, dest)?;
            Ok(())
        }
        Err(e) => Err(e.into()),
    }
}

/// Reads up to 512 bytes from the head (the tar `ustar` magic sits at offset
/// 257) and 64 bytes from the tail of the stream for magic-number sniffing,
/// rewinding to the start afterwards.
fn sniff_window(stream: &mut dyn ReadSeek) -> Result<(Vec<u8>, Vec<u8>), FormatError> {
    let len = stream.seek(SeekFrom::End(0))?;
    let head_len = len.min(512) as usize;
    let mut head = vec![0u8; head_len];
    stream.seek(SeekFrom::Start(0))?;
    stream.read_exact(&mut head)?;
    let tail_len = len.min(64);
    let mut tail = vec![0u8; tail_len as usize];
    stream.seek(SeekFrom::End(-(tail_len as i64)))?;
    stream.read_exact(&mut tail)?;
    stream.seek(SeekFrom::Start(0))?;
    Ok((head, tail))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("squallz-core-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn unknown_format_is_rejected() {
        let dir = temp_dir("unknown");
        let f = dir.join("blob.unknown");
        std::fs::write(&f, b"not an archive at all").unwrap();
        let engine = Engine::new(FormatRegistry::new());
        let err = engine.list(&f, &OpenOptions::default()).unwrap_err();
        assert!(matches!(err, FormatError::Unsupported(_)));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn estimate_create_inputs_counts_and_applies_excludes() {
        let dir = temp_dir("estimate");
        let root = dir.join("project");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("node_modules/pkg")).unwrap();
        std::fs::write(root.join("src/main.rs"), b"fn main() {}").unwrap();
        std::fs::write(root.join("notes.tmp"), b"skip").unwrap();
        std::fs::write(root.join("node_modules/pkg/index.js"), b"skip").unwrap();

        let engine = Engine::new(FormatRegistry::new());
        let estimate = engine
            .estimate_create_inputs(
                std::slice::from_ref(&root),
                &["node_modules".to_owned(), "*.tmp".to_owned()],
            )
            .unwrap();
        assert_eq!(estimate.input_count, 1);
        assert_eq!(estimate.files, 1);
        assert_eq!(estimate.directories, 2);
        assert_eq!(estimate.entries, 3);
        assert_eq!(estimate.total_bytes, b"fn main() {}".len() as u64);
        assert_eq!(
            estimate.output_budget_bytes(),
            b"fn main() {}".len() as u64 + 1024 * 1024 + 3 * 1024 + 4096
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn estimate_create_inputs_reports_scan_progress() {
        let dir = temp_dir("estimate-progress");
        let root = dir.join("project");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/main.rs"), b"fn main() {}").unwrap();
        std::fs::write(root.join("notes.tmp"), b"skip").unwrap();

        let engine = Engine::new(FormatRegistry::new());
        let mut progress = Vec::new();
        let estimate = engine
            .estimate_create_inputs_with_progress(
                std::slice::from_ref(&root),
                &["*.tmp".to_owned()],
                |count, path| progress.push((count, path.to_owned())),
            )
            .unwrap();

        assert_eq!(estimate.entries, 3);
        assert_eq!(
            progress,
            vec![
                (1, "project".to_owned()),
                (2, "project/src".to_owned()),
                (3, "project/src/main.rs".to_owned())
            ]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn find_duplicate_files_groups_by_size_and_hash_with_excludes() {
        let dir = temp_dir("duplicates");
        let root = dir.join("project");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("cache")).unwrap();
        std::fs::write(root.join("src/a.bin"), b"same payload").unwrap();
        std::fs::write(root.join("src/b.bin"), b"same payload").unwrap();
        std::fs::write(root.join("src/c.bin"), b"same length!").unwrap();
        std::fs::write(root.join("cache/d.bin"), b"same payload").unwrap();

        let engine = Engine::new(FormatRegistry::new());
        let report = engine
            .find_duplicate_files(std::slice::from_ref(&root), &["cache".to_owned()], 1)
            .unwrap();

        assert_eq!(report.input_count, 1);
        assert_eq!(report.files_scanned, 3);
        assert_eq!(report.duplicate_groups(), 1);
        assert_eq!(report.duplicate_files(), 2);
        assert_eq!(report.reclaimable_bytes(), b"same payload".len() as u64);
        assert_eq!(report.groups[0].paths.len(), 2);
        assert!(report.groups[0]
            .paths
            .iter()
            .any(|path| path.ends_with("src/a.bin")));
        assert!(report.groups[0]
            .paths
            .iter()
            .any(|path| path.ends_with("src/b.bin")));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn checksum_files_hashes_files_with_shared_excludes() {
        let dir = temp_dir("checksum");
        let root = dir.join("project");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("target")).unwrap();
        std::fs::write(root.join("src/a.txt"), b"abc").unwrap();
        std::fs::write(root.join("target/ignored.txt"), b"ignore").unwrap();

        let engine = Engine::new(FormatRegistry::new());
        let report = engine
            .checksum_files(
                std::slice::from_ref(&root),
                &["target".to_owned()],
                ChecksumAlgorithm::Sha256,
            )
            .unwrap();

        assert_eq!(report.algorithm, ChecksumAlgorithm::Sha256);
        assert_eq!(report.input_count, 1);
        assert_eq!(report.files_hashed, 1);
        assert_eq!(report.bytes_hashed, 3);
        assert_eq!(
            report.items[0].digest,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert!(report.items[0].path.ends_with("src/a.txt"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn verify_checksum_manifest_reports_matches_and_mismatches() {
        let dir = temp_dir("checksum-verify");
        std::fs::write(dir.join("good.txt"), b"abc").unwrap();
        std::fs::write(dir.join("bad.txt"), b"changed").unwrap();
        std::fs::write(
            dir.join("SHA256SUMS"),
            concat!(
                "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad  good.txt\n",
                "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad  bad.txt\n",
                "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad  missing.txt\n",
            ),
        )
        .unwrap();

        let engine = Engine::new(FormatRegistry::new());
        let report = engine
            .verify_checksum_manifest(&dir.join("SHA256SUMS"), ChecksumAlgorithm::Sha256)
            .unwrap();

        assert!(!report.is_ok());
        assert_eq!(report.checked, 3);
        assert_eq!(report.passed, 1);
        assert_eq!(report.failed, 2);
        assert_eq!(
            report.items[0].actual.as_deref(),
            Some("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad")
        );
        assert!(report.items[1].actual.is_some());
        assert!(report.items[2].error.is_some());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
