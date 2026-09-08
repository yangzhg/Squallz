//! ZIP format (backed by the `zip` crate): list/extract/create/test,
//! AES-256 read/write, ZipCrypto read-only, ZIP64, legacy entry-name
//! encodings.
//!
//! Extraction deliberately uses the default [`ArchiveReader::extract`]
//! implementation (the shared safe extraction engine in
//! squallz-format-api).

mod datetime;
mod encoding;
mod error;
mod reader;
mod split;
mod update;
#[cfg_attr(not(feature = "process-backend"), allow(dead_code))]
mod volume;
mod writer;

#[cfg(feature = "process-backend")]
use std::io::Read;
use std::path::{Path, PathBuf};

use squallz_format_api::{
    ArchiveFormat, ArchiveReader, ArchiveSourceSet, ArchiveWriter, ControlToken, CreateOptions,
    FormatCapabilities, FormatError, NativeVolumeLimits, NativeVolumeWriter, OpenOptions,
    PhysicalFileIdentity, PreparedUpdateAdditions, ProgressSink, ReadSeek, UpdateOp, WriteSeek,
};
#[cfg(feature = "process-backend")]
use squallz_format_api::{
    BoundedProblemLog, EntryMeta, EntryPath, EntryType, Password, TestSummary,
    TEST_PROBLEM_PREVIEW_LIMIT,
};

#[cfg(feature = "process-backend")]
use crate::sevenzip_bridge;
use volume::BoundZipSource;
#[cfg(feature = "process-backend")]
use volume::StagedSplitZipSet;

/// End-of-central-directory signature (`PK\x05\x06`).
const EOCD_MAGIC: [u8; 4] = [0x50, 0x4B, 0x05, 0x06];
/// Local-file-header signature (`PK\x03\x04`).
const LOCAL_MAGIC: [u8; 4] = [0x50, 0x4B, 0x03, 0x04];
/// Split-archive marker used at the start of the first native ZIP volume.
const SPLIT_MAGIC: [u8; 4] = [0x50, 0x4B, 0x07, 0x08];

/// The ZIP archive format.
pub(crate) struct ZipFormat;

/// Read-only ZIP surface used by the single-file SFX runtime.
///
/// Keeping this adapter separate from [`ZipFormat`] prevents a constrained
/// runtime registry from exposing archive creation, updates, or native volume
/// writing while reusing the same ZIP reader and extraction safety boundary.
pub(crate) struct SfxZipFormat;

fn sniff_zip(head: &[u8], tail: &[u8]) -> bool {
    // Plain ZIP starts with a local header; an empty ZIP starts with the
    // EOCD record directly.
    if head.starts_with(&LOCAL_MAGIC)
        || head.starts_with(&EOCD_MAGIC)
        || head.starts_with(&SPLIT_MAGIC)
    {
        return true;
    }
    // SFX archives start with an executable stub but still end with an EOCD
    // record, so also scan the tail window.
    tail.windows(EOCD_MAGIC.len())
        .any(|window| window == EOCD_MAGIC)
}

impl ArchiveFormat for ZipFormat {
    fn id(&self) -> &'static str {
        "zip"
    }

    fn extensions(&self) -> &'static [&'static str] {
        // JAR/APK/CBZ/IPA are ZIP container aliases.
        &["zip", "jar", "apk", "cbz", "ipa"]
    }

    fn capabilities(&self) -> FormatCapabilities {
        FormatCapabilities {
            can_create: true,
            can_extract: true,
            can_encrypt_data: true,
            can_encrypt_names: false, // the ZIP format cannot encrypt names
            can_split: true,          // engine-side `.001` byte splitting
            can_update: true,
            can_test: true,
        }
    }

    fn sniff(&self, head: &[u8], tail: &[u8]) -> bool {
        sniff_zip(head, tail)
    }

    fn open(
        &self,
        src: Box<dyn ReadSeek>,
        opts: &OpenOptions,
    ) -> Result<Box<dyn ArchiveReader>, FormatError> {
        reader::open(src, opts)
    }

    fn open_file(
        &self,
        source_path: &Path,
        source_identity: Option<PhysicalFileIdentity>,
        src: Box<dyn ReadSeek>,
        opts: &OpenOptions,
    ) -> Result<Box<dyn ArchiveReader>, FormatError> {
        self.open_file_with_control(
            source_path,
            source_identity,
            src,
            opts,
            &ControlToken::default(),
        )
    }

    fn open_file_with_control(
        &self,
        source_path: &Path,
        source_identity: Option<PhysicalFileIdentity>,
        src: Box<dyn ReadSeek>,
        opts: &OpenOptions,
        ctl: &ControlToken,
    ) -> Result<Box<dyn ArchiveReader>, FormatError> {
        ctl.checkpoint()?;
        match volume::bind_file_with_control(source_path, source_identity, src, ctl)? {
            BoundZipSource::Single(src) => reader::open(src, opts),
            BoundZipSource::Split(discovered, selected_src) => {
                #[cfg(feature = "process-backend")]
                {
                    let tool = sevenzip_bridge::sevenzip_tool_if_configured_or_installed()
                        .ok_or_else(|| {
                            FormatError::DependencyMissing(
                                "7zz/7z with native split ZIP support".into(),
                            )
                        })?;
                    let staged = StagedSplitZipSet::from_discovered_with_control(
                        discovered,
                        selected_src,
                        ctl,
                    )?;
                    Ok(Box::new(SplitZipArchiveReader::open(
                        staged,
                        tool,
                        opts.password.clone(),
                        ctl,
                    )?))
                }
                #[cfg(not(feature = "process-backend"))]
                {
                    let _ = (discovered, selected_src);
                    Err(FormatError::DependencyMissing(
                        "native split ZIP decoding is unavailable in this constrained build".into(),
                    ))
                }
            }
        }
    }

    fn probe_file_source_set(
        &self,
        source_path: &Path,
        source_identity: Option<PhysicalFileIdentity>,
        src: &mut dyn ReadSeek,
    ) -> Result<Option<ArchiveSourceSet>, FormatError> {
        volume::probe_bound_file(source_path, source_identity, src)
    }

    fn probe_file_source_set_with_control(
        &self,
        source_path: &Path,
        source_identity: Option<PhysicalFileIdentity>,
        src: &mut dyn ReadSeek,
        ctl: &ControlToken,
    ) -> Result<Option<ArchiveSourceSet>, FormatError> {
        volume::probe_bound_file_with_control(source_path, source_identity, src, ctl)
    }

    fn create(
        &self,
        dst: Box<dyn WriteSeek>,
        opts: &CreateOptions,
    ) -> Result<Box<dyn ArchiveWriter>, FormatError> {
        Ok(Box::new(writer::ZipArchiveWriter::new(dst, opts)))
    }

    fn create_with_control(
        &self,
        dst: Box<dyn WriteSeek>,
        opts: &CreateOptions,
        ctl: &ControlToken,
    ) -> Result<Box<dyn ArchiveWriter>, FormatError> {
        ctl.checkpoint()?;
        Ok(Box::new(writer::ZipArchiveWriter::new_with_control(
            dst, opts, ctl,
        )))
    }

    fn native_volume_limits(&self) -> Option<NativeVolumeLimits> {
        Some(NativeVolumeLimits {
            min_volume_size: 64 * 1024,
            max_volume_size: u32::MAX as u64,
            // Disk index 0xffff is reserved as the classic ZIP64 sentinel.
            max_volumes: u16::MAX as u32,
        })
    }

    fn native_volume_path(
        &self,
        destination: &Path,
        disk_index: u32,
        final_volume: bool,
    ) -> Result<PathBuf, FormatError> {
        split::volume_path(destination, disk_index, final_volume)
    }

    fn write_native_volumes(
        &self,
        source: &mut dyn ReadSeek,
        output: &mut dyn NativeVolumeWriter,
        progress: &dyn ProgressSink,
        ctl: &ControlToken,
    ) -> Result<(), FormatError> {
        split::write_native_volumes(source, output, progress, ctl)
    }

    fn supports_update_rewrite(&self) -> bool {
        true
    }

    fn estimate_update_staging_bytes(
        &self,
        source_bytes: u64,
        addition_bytes: u64,
        _opts: &CreateOptions,
    ) -> Result<u64, FormatError> {
        Ok(update::staging_bytes_estimate(source_bytes, addition_bytes))
    }

    fn rewrite_update(
        &self,
        source: Box<dyn ReadSeek>,
        output: Box<dyn WriteSeek>,
        ops: &[UpdateOp],
        additions: &mut dyn PreparedUpdateAdditions,
        opts: &CreateOptions,
        progress: &dyn ProgressSink,
        ctl: &ControlToken,
    ) -> Result<(), FormatError> {
        update::rewrite_archive(source, output, ops, additions, opts, progress, ctl)
    }
}

impl ArchiveFormat for SfxZipFormat {
    fn id(&self) -> &'static str {
        "zip"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["zip"]
    }

    fn capabilities(&self) -> FormatCapabilities {
        FormatCapabilities {
            can_extract: true,
            can_encrypt_data: true,
            can_test: true,
            ..FormatCapabilities::default()
        }
    }

    fn sniff(&self, head: &[u8], tail: &[u8]) -> bool {
        sniff_zip(head, tail)
    }

    fn open(
        &self,
        src: Box<dyn ReadSeek>,
        opts: &OpenOptions,
    ) -> Result<Box<dyn ArchiveReader>, FormatError> {
        reader::open(src, opts)
    }

    fn create(
        &self,
        _dst: Box<dyn WriteSeek>,
        _opts: &CreateOptions,
    ) -> Result<Box<dyn ArchiveWriter>, FormatError> {
        Err(FormatError::Unsupported(
            "the SFX runtime ZIP registry is read-only".into(),
        ))
    }
}

#[cfg(feature = "process-backend")]
struct SplitZipArchiveReader {
    staged: StagedSplitZipSet,
    tool: PathBuf,
    entries: Vec<EntryMeta>,
    password: Option<Password>,
    control: ControlToken,
}

#[cfg(feature = "process-backend")]
impl SplitZipArchiveReader {
    fn open(
        staged: StagedSplitZipSet,
        tool: PathBuf,
        password: Option<Password>,
        ctl: &ControlToken,
    ) -> Result<Self, FormatError> {
        let entries = sevenzip_bridge::list_entries_with_control(
            &tool,
            staged.path(),
            password.as_ref(),
            ctl,
        )
        .map_err(|error| staged.remap_external_error(error))?;
        Ok(Self {
            staged,
            tool,
            entries,
            password,
            control: ctl.clone(),
        })
    }

    fn read_entry_with_control(
        &self,
        path: &EntryPath,
        control: &ControlToken,
    ) -> Result<Box<dyn Read>, FormatError> {
        sevenzip_bridge::require_password_for_entry(&self.entries, path, self.password.as_ref())?;
        sevenzip_bridge::read_entry_stdout(
            &self.tool,
            self.staged.path(),
            path,
            self.password.as_ref(),
            control,
        )
        .map_err(|error| self.staged.remap_external_error(error))
    }

    fn test_with_problem_recorder(
        &mut self,
        progress: &dyn ProgressSink,
        ctl: &ControlToken,
        mut record_problem: impl FnMut(String),
    ) -> Result<u64, FormatError> {
        let total = self
            .entries
            .iter()
            .filter(|entry| matches!(entry.entry_type, EntryType::File))
            .map(|entry| entry.size)
            .sum();
        let mut done = 0u64;
        let mut entries_tested = 0u64;
        for meta in self.entries.clone() {
            ctl.checkpoint()?;
            if !matches!(meta.entry_type, EntryType::File) {
                continue;
            }
            progress.on_progress(done, total, &meta.path);
            match self.read_entry_with_control(&meta.path, ctl) {
                Ok(mut data) => {
                    let mut buffer = [0u8; 64 * 1024];
                    loop {
                        ctl.checkpoint()?;
                        match data.read(&mut buffer) {
                            Ok(0) => break,
                            Ok(read) => {
                                done = done.saturating_add(read as u64);
                                progress.on_progress(done.min(total), total, &meta.path);
                            }
                            Err(error) => {
                                let error = sevenzip_bridge::recoverable_stream_error(error)?;
                                record_problem(format!("{}: {error}", meta.path.display));
                                break;
                            }
                        }
                    }
                }
                Err(error) => {
                    let error = sevenzip_bridge::recoverable_test_error(error)?;
                    record_problem(format!("{}: {error}", meta.path.display));
                }
            }
            entries_tested += 1;
        }
        progress.on_progress(done.min(total), total, &EntryPath::from_utf8(""));
        Ok(entries_tested)
    }
}

#[cfg(feature = "process-backend")]
impl ArchiveReader for SplitZipArchiveReader {
    fn source_set(&self) -> Option<&ArchiveSourceSet> {
        Some(self.staged.source_set())
    }

    fn verify_source_set(&self, ctl: &ControlToken) -> Result<(), FormatError> {
        self.staged.verify_source_set(ctl)
    }

    fn entries(&mut self) -> Box<dyn Iterator<Item = Result<EntryMeta, FormatError>> + '_> {
        Box::new(self.entries.clone().into_iter().map(Ok))
    }

    fn consume_entries(
        mut self: Box<Self>,
        visitor: &mut dyn FnMut(EntryMeta) -> Result<(), FormatError>,
    ) -> Result<(), FormatError> {
        for entry in std::mem::take(&mut self.entries) {
            visitor(entry)?;
        }
        Ok(())
    }

    fn read_entry(&mut self, path: &EntryPath) -> Result<Box<dyn Read + '_>, FormatError> {
        self.read_entry_with_control(path, &self.control)
    }

    fn test_summary(
        &mut self,
        progress: &dyn ProgressSink,
        ctl: &ControlToken,
    ) -> Result<TestSummary, FormatError> {
        let problems = BoundedProblemLog::new(TEST_PROBLEM_PREVIEW_LIMIT);
        let entries_tested =
            self.test_with_problem_recorder(progress, ctl, |problem| problems.record(problem))?;
        Ok(TestSummary {
            entries_tested,
            problems: problems.snapshot(),
            recovery: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Write as _};

    use squallz_format_api::{EntryMeta, EntryPath, EntryType, NoProgress};

    use super::*;

    #[test]
    fn zip_format_declares_aliases_and_capabilities() {
        let format = ZipFormat;

        assert_eq!(format.id(), "zip");
        assert_eq!(format.extensions(), &["zip", "jar", "apk", "cbz", "ipa"]);

        let capabilities = format.capabilities();
        assert!(format.supports_update_rewrite());
        assert!(capabilities.can_create);
        assert!(capabilities.can_extract);
        assert!(capabilities.can_encrypt_data);
        assert!(!capabilities.can_encrypt_names);
        assert!(capabilities.can_split);
        assert!(capabilities.can_update);
        assert!(capabilities.can_test);
    }

    #[test]
    fn sfx_zip_format_exposes_only_the_read_surface() {
        let format = SfxZipFormat;

        assert_eq!(format.id(), "zip");
        assert_eq!(format.extensions(), &["zip"]);

        let capabilities = format.capabilities();
        assert!(!capabilities.can_create);
        assert!(capabilities.can_extract);
        assert!(capabilities.can_encrypt_data);
        assert!(!capabilities.can_encrypt_names);
        assert!(!capabilities.can_split);
        assert!(!capabilities.can_update);
        assert!(capabilities.can_test);

        let error = match format.create(
            Box::new(Cursor::new(Vec::<u8>::new())),
            &CreateOptions::default(),
        ) {
            Ok(_) => panic!("SFX ZIP adapter must reject creation"),
            Err(error) => error,
        };
        assert!(matches!(error, FormatError::Unsupported(_)));
    }

    #[test]
    fn sfx_zip_format_reuses_the_shared_reader() {
        let mut archive = ::zip::ZipWriter::new(Cursor::new(Vec::new()));
        archive
            .start_file(
                "payload.txt",
                ::zip::write::SimpleFileOptions::default()
                    .compression_method(::zip::CompressionMethod::Deflated),
            )
            .expect("start SFX ZIP fixture entry");
        archive
            .write_all(b"SFX payload")
            .expect("write SFX ZIP fixture entry");
        let bytes = archive
            .finish()
            .expect("finish SFX ZIP fixture")
            .into_inner();

        let format = SfxZipFormat;
        let mut reader = format
            .open(Box::new(Cursor::new(bytes)), &OpenOptions::default())
            .expect("open SFX ZIP through shared reader");
        let entries = reader
            .entries()
            .collect::<Result<Vec<_>, _>>()
            .expect("list SFX ZIP entries");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path.display, "payload.txt");

        let mut contents = Vec::new();
        {
            let mut entry = reader
                .read_entry(&entries[0].path)
                .expect("open SFX ZIP payload entry");
            std::io::Read::read_to_end(&mut entry, &mut contents)
                .expect("read SFX ZIP payload entry");
        }
        assert_eq!(contents, b"SFX payload");

        let report = reader
            .test_summary(&NoProgress, &ControlToken::default())
            .expect("test SFX ZIP payload");
        assert!(report.is_ok());
        assert_eq!(report.entries_tested, 1);
    }

    #[test]
    fn controlled_create_retains_the_callers_token() {
        let format = ZipFormat;
        let control = ControlToken::default();
        let mut writer = format
            .create_with_control(
                Box::new(std::io::Cursor::new(Vec::<u8>::new())),
                &CreateOptions::default(),
                &control,
            )
            .unwrap_or_else(|error| panic!("create controlled ZIP writer: {error}"));
        control.cancel();
        let error = writer
            .add_entry(
                &EntryMeta {
                    path: EntryPath::from_utf8("cancelled/"),
                    entry_type: EntryType::Dir,
                    size: 0,
                    compressed_size: None,
                    modified: None,
                    unix_mode: None,
                    crc32: None,
                    encrypted: false,
                },
                None,
            )
            .expect_err("controlled ZIP writer must retain the caller's token");

        assert!(matches!(error, FormatError::Cancelled));
    }

    #[test]
    fn zip_sniffer_accepts_plain_empty_and_sfx_archives() {
        let format = ZipFormat;

        assert!(format.sniff(&LOCAL_MAGIC, &[]));
        assert!(format.sniff(&EOCD_MAGIC, &[]));
        assert!(format.sniff(&SPLIT_MAGIC, &[]));

        let sfx_tail = b"stub bytes before PK\x05\x06 and after";
        assert!(format.sniff(b"MZ executable stub", sfx_tail));
    }

    #[test]
    fn zip_sniffer_rejects_non_zip_and_partial_signatures() {
        let format = ZipFormat;

        assert!(!format.sniff(b"not a zip", b"still not a zip"));
        assert!(!format.sniff(b"PK\x03", b"PK\x05"));
    }

    #[cfg(feature = "process-backend")]
    #[test]
    fn native_split_encrypted_entries_require_a_password_at_the_read_boundary() {
        let entries = vec![EntryMeta {
            path: EntryPath::from_utf8("secret.txt"),
            entry_type: EntryType::File,
            size: 1,
            compressed_size: Some(1),
            modified: None,
            unix_mode: None,
            crc32: None,
            encrypted: true,
        }];
        let password = Password::new("fixture-password");

        assert!(matches!(
            sevenzip_bridge::require_password_for_entry(&entries, &entries[0].path, None),
            Err(FormatError::PasswordRequired)
        ));
        assert!(sevenzip_bridge::require_password_for_entry(
            &entries,
            &entries[0].path,
            Some(&password)
        )
        .is_ok());
    }
}
