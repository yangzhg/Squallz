//! Reader protocol shared by the external 7-Zip, RAR and native split ZIP bridges.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::PathBuf;

use squallz_format_api::{
    test_entry_data, ArchiveReader, ArchiveSourceSet, BoundedProblemLog, ControlToken, EntryMeta,
    EntryPath, EntryType, FormatError, LimitsAccountant, Password, ProgressSink, SafetyLimits,
    TestSummary, TEST_PROBLEM_PREVIEW_LIMIT,
};

use crate::rar::{RarBackend, StagedRarSet};
use crate::sevenzip_bridge::{self, StagedSplitWimSet, TempArchive};
use crate::zip::StagedSplitZipSet;

/// A bound source remains paired with the backend that accepted its listing.
pub(crate) enum ExternalArchiveSource {
    ToolArchive {
        archive: TempArchive,
        tool: PathBuf,
        backend_paths: BTreeMap<String, String>,
    },
    SplitWim {
        staged: StagedSplitWimSet,
        tool: PathBuf,
    },
    Rar {
        staged: StagedRarSet,
        backend: RarBackend,
    },
    SplitZip {
        staged: StagedSplitZipSet,
        tool: PathBuf,
    },
}

pub(crate) struct ExternalArchiveReader {
    source: ExternalArchiveSource,
    entries: Vec<EntryMeta>,
    password: Option<Password>,
    control: ControlToken,
}

impl ExternalArchiveReader {
    pub(crate) fn new(
        source: ExternalArchiveSource,
        entries: Vec<EntryMeta>,
        password: Option<Password>,
        control: &ControlToken,
    ) -> Self {
        Self {
            source,
            entries,
            password,
            control: control.clone(),
        }
    }

    fn entry_stream(
        &self,
        path: &EntryPath,
        control: &ControlToken,
    ) -> Result<Box<dyn Read>, FormatError> {
        let password = self.password.as_ref();
        sevenzip_bridge::require_password_for_entry(&self.entries, path, password)?;
        match &self.source {
            ExternalArchiveSource::ToolArchive {
                archive,
                tool,
                backend_paths,
            } => sevenzip_bridge::spawn_entry_reader(
                tool,
                archive.path(),
                sevenzip_bridge::backend_path_for(backend_paths, path),
                &path.display,
                password,
                control,
            ),
            ExternalArchiveSource::SplitWim { staged, tool } => {
                sevenzip_bridge::read_entry_stdout(tool, staged.path(), path, password, control)
            }
            ExternalArchiveSource::Rar { staged, backend } => {
                backend.read_entry(staged.path(), path, password, control)
            }
            ExternalArchiveSource::SplitZip { staged, tool } => {
                sevenzip_bridge::read_entry_stdout(tool, staged.path(), path, password, control)
                    .map_err(|error| staged.remap_external_error(error))
            }
        }
    }
}

impl ArchiveReader for ExternalArchiveReader {
    fn source_set(&self) -> Option<&ArchiveSourceSet> {
        match &self.source {
            ExternalArchiveSource::ToolArchive { .. } => None,
            ExternalArchiveSource::SplitWim { staged, .. } => Some(staged.source_set()),
            ExternalArchiveSource::Rar { staged, .. } => staged.source_set(),
            ExternalArchiveSource::SplitZip { staged, .. } => Some(staged.source_set()),
        }
    }

    fn verify_source_set(&self, control: &ControlToken) -> Result<(), FormatError> {
        match &self.source {
            ExternalArchiveSource::ToolArchive { .. } => control.checkpoint(),
            ExternalArchiveSource::SplitWim { staged, .. } => staged.verify_source_set(control),
            ExternalArchiveSource::Rar { staged, .. } => staged.verify_source_set(control),
            ExternalArchiveSource::SplitZip { staged, .. } => staged.verify_source_set(control),
        }
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

    fn read_entry(
        &mut self,
        path: &EntryPath,
        consume: &mut dyn FnMut(&mut dyn Read) -> Result<(), FormatError>,
    ) -> Result<(), FormatError> {
        consume(self.entry_stream(path, &self.control)?.as_mut())
    }

    fn test_summary(
        &mut self,
        limits: &SafetyLimits,
        progress: &dyn ProgressSink,
        control: &ControlToken,
    ) -> Result<TestSummary, FormatError> {
        let problems = BoundedProblemLog::new(TEST_PROBLEM_PREVIEW_LIMIT);
        let entries = self.entries.clone();
        let total = entries
            .iter()
            .filter(|entry| matches!(entry.entry_type, EntryType::File))
            .map(|entry| entry.size)
            .fold(0, u64::saturating_add);
        let mut entries_tested = 0u64;
        let mut accountant = LimitsAccountant::new(*limits);
        for meta in entries {
            control.checkpoint()?;
            accountant.check_entry(&meta)?;
            if !matches!(meta.entry_type, EntryType::File) {
                continue;
            }
            match self.entry_stream(&meta.path, control) {
                Ok(mut data) => {
                    if let Err(error) = test_entry_data(
                        data.as_mut(),
                        &meta,
                        &mut accountant,
                        total,
                        progress,
                        control,
                    ) {
                        let error = sevenzip_bridge::recoverable_test_error(error)?;
                        problems.record(format!("{}: {error}", meta.path.display));
                    }
                }
                Err(error) => {
                    let error = sevenzip_bridge::recoverable_test_error(error)?;
                    problems.record(format!("{}: {error}", meta.path.display));
                }
            }
            entries_tested += 1;
        }
        progress.on_progress(
            accountant.output_bytes(),
            accountant.output_bytes(),
            &EntryPath::from_utf8(""),
        );
        Ok(TestSummary {
            entries_tested,
            problems: problems.snapshot(),
            recovery: None,
        })
    }
}
