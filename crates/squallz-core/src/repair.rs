//! Source validation and atomic rewriting for embedded SQZ recovery and ZIP
//! local-header index rebuilding. Presentation stays with the caller.

use std::path::Path;

use crate::api::{
    ControlToken, CreateOptions, FormatError, OpenOptions, ProgressSink, SafetyLimits,
};
use crate::{
    is_plain_sqz_path, is_sqz_archive_path, is_zip_family_path, same_existing_path,
    ArchiveTestOutcome, CreateCommitPolicy, Engine,
};

/// The recovery mechanism requested for an archive rewrite.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveRepairKind {
    SqzEmbedded,
    ZipIndexRebuild,
}

/// Destination encoding and source-verification limits for one repair.
#[derive(Debug, Clone)]
pub struct ArchiveRepairOptions {
    pub kind: ArchiveRepairKind,
    pub create: CreateOptions,
    pub safety_limits: SafetyLimits,
}

/// Keeps the complete source diagnostics even when no rewrite is possible.
#[derive(Debug, Clone)]
pub enum ArchiveRepairOutcome {
    Repaired {
        source: ArchiveTestOutcome,
        in_place: bool,
    },
    SourceRejected(ArchiveTestOutcome),
}

impl Engine {
    /// Verifies a recoverable SQZ or ZIP source before rewriting it atomically.
    /// SQZ requires a complete successful integrity result; ZIP index rebuilding
    /// accepts intact payloads recovered from an incomplete central directory.
    /// Independent outputs are never replaced, while the same existing source
    /// may be repaired in place. Split sources require an unsplit destination.
    /// Separate progress sinks let callers keep source verification quiet.
    pub fn repair_archive(
        &self,
        src: &Path,
        dest: &Path,
        options: &ArchiveRepairOptions,
        source_progress: &dyn ProgressSink,
        rewrite_progress: &dyn ProgressSink,
        ctl: &ControlToken,
    ) -> Result<ArchiveRepairOutcome, FormatError> {
        match options.kind {
            ArchiveRepairKind::SqzEmbedded => {
                if !is_sqz_archive_path(src) {
                    return Err(FormatError::Unsupported(
                        "SQZ repair expects a .sqz source container".into(),
                    ));
                }
                if !is_plain_sqz_path(dest) {
                    return Err(FormatError::Unsupported(
                        "SQZ repair output must be a .sqz container".into(),
                    ));
                }
            }
            ArchiveRepairKind::ZipIndexRebuild => {
                if !is_zip_family_path(src) {
                    return Err(FormatError::Unsupported(
                        "ZIP index rebuild expects a ZIP-family source archive".into(),
                    ));
                }
                if !is_zip_family_path(dest) {
                    return Err(FormatError::Unsupported(
                        "ZIP rebuild output must be a ZIP-family archive (.zip/.jar/.apk/.cbz/.ipa)"
                            .into(),
                    ));
                }
            }
        }
        if options.create.split_size.is_some() {
            return Err(FormatError::Unsupported(
                "archive repair requires one complete archive output".into(),
            ));
        }

        let open = OpenOptions::default();
        let source = self.test_summary_with_structure(
            src,
            &open,
            &options.safety_limits,
            source_progress,
            ctl,
        )?;
        let can_rewrite = match options.kind {
            ArchiveRepairKind::SqzEmbedded => source.summary.is_ok(),
            ArchiveRepairKind::ZipIndexRebuild => source.payload_is_ok(),
        };
        if !can_rewrite {
            return Ok(ArchiveRepairOutcome::SourceRejected(source));
        }

        let in_place = same_existing_path(src, dest);
        let commit_policy = if in_place {
            CreateCommitPolicy::ReplaceExisting
        } else {
            CreateCommitPolicy::NoReplace
        };
        self.convert(
            src,
            dest,
            &open,
            &options.create,
            commit_policy,
            rewrite_progress,
            ctl,
        )?;
        Ok(ArchiveRepairOutcome::Repaired { source, in_place })
    }
}
