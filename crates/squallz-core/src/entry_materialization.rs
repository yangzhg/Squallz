//! Complete, bounded copies of one archive entry into caller-owned plaintext.

use std::fs::File;
use std::io::Write;
use std::path::Path;

use crate::api::{
    ArchiveReader, ControlToken, EntryMeta, EntryPath, FormatError, OpenOptions, ProgressPhase,
    ProgressSink, SafetyLimits,
};
use crate::{copy_archive_entry, Engine, PlaintextFile};

const MIN_FREE_BYTES: u64 = 64 * 1024 * 1024;

/// An entry's original identity or the normalized path shown in a browser.
/// Display lookups reject aliases instead of borrowing one entry's metadata
/// limits while reading another entry's data.
pub enum EntryReference<'a> {
    Raw(&'a EntryPath),
    Display(&'a str),
}

/// Retains the reader that supplied a uniquely selected entry's metadata.
/// Creating the plaintext output happens after this admission, allowing each
/// shell to report private-storage failures without hiding archive errors.
pub struct PreparedArchiveEntry {
    reader: Box<dyn ArchiveReader>,
    meta: EntryMeta,
    control: ControlToken,
}

impl Engine {
    pub fn prepare_entry_materialization(
        &self,
        archive: &Path,
        reference: EntryReference<'_>,
        options: &OpenOptions,
        control: &ControlToken,
    ) -> Result<PreparedArchiveEntry, FormatError> {
        let mut reader = self.open(archive, options, control)?;
        let normalized = match &reference {
            EntryReference::Display(path) => Some(normalized_display_path(path)),
            EntryReference::Raw(_) => None,
        };
        let mut selected = None;
        for meta in reader.entries() {
            control.checkpoint()?;
            let meta = meta?;
            let matches = match &reference {
                EntryReference::Raw(path) => meta.path.raw == path.raw,
                EntryReference::Display(_) => {
                    normalized.as_deref()
                        == Some(normalized_display_path(&meta.path.display).as_str())
                }
            };
            if matches && selected.replace(meta).is_some() {
                return Err(FormatError::CorruptArchive(match reference {
                    EntryReference::Raw(_) => {
                        "nested archive entry has an ambiguous duplicate raw name".into()
                    }
                    EntryReference::Display(_) => "nested archive entry name is ambiguous".into(),
                }));
            }
        }
        control.checkpoint()?;
        let meta = selected.ok_or_else(|| {
            FormatError::Other(match reference {
                EntryReference::Raw(path) => format!("entry not found: {path}"),
                EntryReference::Display(path) => format!("nested archive entry not found: {path}"),
            })
        })?;
        Ok(PreparedArchiveEntry {
            reader,
            meta,
            control: control.clone(),
        })
    }
}

impl PreparedArchiveEntry {
    /// Copies through the retained reader into the writer belonging to `output`.
    /// The caller closes the writer before dropping or explicitly closing the
    /// plaintext owner. Cancellation and copy errors never publish a result.
    pub fn copy_to(
        self,
        output: &PlaintextFile,
        writer: &mut File,
        mut limits: SafetyLimits,
        progress: &dyn ProgressSink,
    ) -> Result<u64, FormatError> {
        let Self {
            mut reader,
            meta,
            control,
        } = self;
        let control = &control;
        control.checkpoint()?;
        output.verify_writer(writer)?;
        let available = fs4::available_space(output.path())?;
        let headroom = (available / 20).max(MIN_FREE_BYTES).min(available);
        let writable = available.saturating_sub(headroom);
        if writable == 0 || meta.size > writable {
            return Err(FormatError::DiskFull);
        }
        limits.max_output_bytes = limits.max_output_bytes.min(writable);
        progress.on_phase(ProgressPhase::ExtractEntries, true);
        let mut written = 0;
        let result = reader.read_entry(&meta.path, &mut |reader| {
            written = copy_archive_entry(reader, writer, &meta, limits, progress, control)?;
            Ok(())
        });
        control.checkpoint()?;
        result?;
        writer.flush()?;
        control.checkpoint()?;
        output.verify_writer(writer)?;
        Ok(written)
    }
}

fn normalized_display_path(path: &str) -> String {
    path.replace('\\', "/").trim_start_matches('/').to_owned()
}
