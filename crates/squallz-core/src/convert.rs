//! Format conversion: stream every entry of a source archive into a new
//! archive of the format chosen by the destination extension, reusing the
//! create-side format selection (compound pipelines included) without
//! extracting to disk.

use std::fs::File;
use std::path::Path;
use std::sync::Arc;

use crate::api::{
    ArchiveReader, Compressor, ControlToken, CreateOptions, Detected, EntryMeta, EntryPath,
    EntryType, FormatError, OpenOptions, ProgressPhase, ProgressSink,
};
use crate::archive_output::{
    bind_commit_policy, validate_create_target_name, ArchiveOutput, ArchiveWriterTarget, DestSink,
    DestTarget,
};
use crate::compound::{KnownTotal, ProgressRead};
use crate::create::{create_plan_from_summary, ensure_create_space};
use crate::{
    create_input_summary, CreateCommitPolicy, CreateInputEstimate, CreatePlan, CreateReport, Engine,
};

/// Entry point for [`Engine::convert`]. Metadata is carried over as
/// faithfully as the destination format allows; entry types the destination
/// cannot store (e.g. symlinks in 7z) surface as
/// [`FormatError::Unsupported`] naming the offending entry.
#[allow(clippy::too_many_arguments)] // internal conversion boundary: each argument has a distinct role
pub(crate) fn convert(
    engine: &Engine,
    src: &Path,
    dest: &Path,
    open_opts: &OpenOptions,
    create_opts: &CreateOptions,
    requested_commit_policy: CreateCommitPolicy,
    progress: &dyn ProgressSink,
    ctl: &ControlToken,
) -> Result<CreateReport, FormatError> {
    let detect_name = dest
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| FormatError::Unsupported("invalid output file name".into()))?;
    validate_create_target_name(engine, detect_name, create_opts)?;

    let commit_policy = bind_commit_policy(dest, create_opts, requested_commit_policy)?;

    let mut reader = engine.open_for_operation(src, open_opts, progress, ctl)?;
    let metas = collect_entry_metadata(&mut *reader, ctl)?;
    let plan = plan_convert_from_entries(engine, dest, &metas, create_opts)?;
    ensure_create_space(dest, &plan)?;
    progress.on_phase(ProgressPhase::ArchiveConvert, true);
    ctl.checkpoint()?;
    ArchiveOutput::Published {
        destination: dest,
        policy: commit_policy,
    }
    .write(engine, create_opts, progress, ctl, move |writer| {
        let result = convert_unsplit(&mut *reader, &metas, writer, progress, ctl);
        // Release source handles before publishing over that same archive.
        drop(reader);
        result
    })
    .map(|(_, report)| report)
}

pub(crate) fn plan_convert_from_entries(
    engine: &Engine,
    dest: &Path,
    metas: &[EntryMeta],
    create_opts: &CreateOptions,
) -> Result<CreatePlan, FormatError> {
    let detect_name = dest
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| FormatError::Unsupported("invalid output file name".into()))?;
    validate_create_target_name(engine, detect_name, create_opts)?;
    validate_single_stream_layout(engine, detect_name, metas)?;
    create_plan_from_summary(
        engine,
        detect_name,
        dest,
        summarize_convert_entries(metas),
        create_opts,
    )
}

fn collect_entry_metadata(
    reader: &mut dyn ArchiveReader,
    ctl: &ControlToken,
) -> Result<Vec<EntryMeta>, FormatError> {
    let mut entries = Vec::new();
    for entry in reader.entries() {
        ctl.checkpoint()?;
        entries.push(entry?);
    }
    ctl.checkpoint()?;
    Ok(entries)
}

fn summarize_convert_entries(metas: &[EntryMeta]) -> crate::CreateInputSummary {
    let mut estimate = CreateInputEstimate {
        input_count: 1,
        ..CreateInputEstimate::default()
    };
    let mut archive_metadata_bytes = 0u64;
    for meta in metas {
        estimate.entries += 1;
        archive_metadata_bytes =
            archive_metadata_bytes.saturating_add(crate::usize_to_u64(meta.path.raw.len()));
        match &meta.entry_type {
            EntryType::File => {
                estimate.files += 1;
                estimate.total_bytes = estimate.total_bytes.saturating_add(meta.size);
            }
            EntryType::Dir => estimate.directories += 1,
            EntryType::Symlink { target } => {
                estimate.symlinks += 1;
                archive_metadata_bytes =
                    archive_metadata_bytes.saturating_add(crate::usize_to_u64(target.len()));
            }
            EntryType::Hardlink { target } => {
                archive_metadata_bytes =
                    archive_metadata_bytes.saturating_add(crate::usize_to_u64(target.len()));
            }
            EntryType::Other => {}
        }
    }
    create_input_summary(estimate, archive_metadata_bytes)
}

fn validate_single_stream_layout(
    engine: &Engine,
    detect_name: &str,
    metas: &[EntryMeta],
) -> Result<(), FormatError> {
    let Some(Detected::Compressed {
        compressor,
        inner_archive: None,
    }) = engine.registry().detect_by_name(detect_name)
    else {
        return Ok(());
    };
    let mut files = metas
        .iter()
        .filter(|meta| !matches!(meta.entry_type, EntryType::Dir));
    let (Some(meta), None) = (files.next(), files.next()) else {
        return Err(FormatError::Unsupported(format!(
            "format {} stores exactly one file",
            compressor.id()
        )));
    };
    if !matches!(meta.entry_type, EntryType::File) {
        return Err(FormatError::Unsupported(format!(
            "format {} cannot store entry type of '{}'",
            compressor.id(),
            meta.path
        )));
    }
    Ok(())
}

fn convert_unsplit(
    reader: &mut dyn ArchiveReader,
    metas: &[EntryMeta],
    writer: ArchiveWriterTarget<'_>,
    progress: &dyn ProgressSink,
    ctl: &ControlToken,
) -> Result<(), FormatError> {
    let write_opts = writer.options;
    match writer.open(ctl)? {
        DestTarget::Archive(sink) => copy_entries(reader, metas, sink, write_opts, progress, ctl),
        DestTarget::SingleStream { compressor, output } => single_stream_convert(
            reader,
            metas,
            &compressor,
            output,
            write_opts,
            progress,
            ctl,
        ),
    }
}

/// Streams every entry from the reader into the destination writer.
fn copy_entries(
    reader: &mut dyn ArchiveReader,
    metas: &[EntryMeta],
    mut sink: DestSink,
    opts: &CreateOptions,
    progress: &dyn ProgressSink,
    ctl: &ControlToken,
) -> Result<(), FormatError> {
    let total: u64 = metas
        .iter()
        .filter(|m| matches!(m.entry_type, EntryType::File))
        .map(|m| m.size)
        .sum();
    let mut done = 0u64;
    reader.read_entries(
        metas,
        &mut |meta, data| {
            ctl.checkpoint()?;
            progress.on_entry_progress(done, total, &meta.path, 0, meta.size);
            // The destination decides about encryption itself; compressed size
            // and CRC are recomputed by the destination writer.
            let out_meta = EntryMeta {
                compressed_size: None,
                crc32: None,
                encrypted: opts.password.is_some(),
                ..meta.clone()
            };
            match meta.entry_type {
                EntryType::File => {
                    let data = data.ok_or_else(|| {
                        FormatError::CorruptArchive(format!(
                            "file entry without data: {}",
                            meta.path
                        ))
                    })?;
                    let mut data =
                        ProgressRead::new(data, progress, ctl, &meta.path, done, total, meta.size);
                    sink.add_entry(&out_meta, Some(&mut data)).map_err(|e| {
                        if ctl.is_cancelled() {
                            FormatError::Cancelled
                        } else {
                            e
                        }
                    })?;
                    done += meta.size;
                }
                _ => sink.add_entry(&out_meta, None)?,
            }
            Ok(())
        },
        ctl,
    )?;
    ctl.checkpoint()?;
    progress.on_progress(total, total, &EntryPath::from_utf8(""));
    sink.finish()
}

/// Conversion into a bare compressed stream (`x.gz`): the source must hold
/// exactly one file entry (directory markers are ignored).
fn single_stream_convert(
    reader: &mut dyn ArchiveReader,
    metas: &[EntryMeta],
    compressor: &Arc<dyn Compressor>,
    dst: File,
    opts: &CreateOptions,
    progress: &dyn ProgressSink,
    ctl: &ControlToken,
) -> Result<(), FormatError> {
    let mut files = metas
        .iter()
        .filter(|m| !matches!(m.entry_type, EntryType::Dir));
    let (Some(meta), None) = (files.next(), files.next()) else {
        return Err(FormatError::Unsupported(format!(
            "format {} stores exactly one file",
            compressor.id()
        )));
    };
    if !matches!(meta.entry_type, EntryType::File) {
        return Err(FormatError::Unsupported(format!(
            "format {} cannot store entry type of '{}'",
            compressor.id(),
            meta.path
        )));
    }
    // Pump locally: the entry reader is not `Send`, so the trait's chunked
    // pump cannot be used here.
    let mut sink = compressor.compress_writer(Box::new(dst), opts.level, &opts.resources)?;
    let label = KnownTotal::new(progress, meta.size, meta.path.clone());
    let mut buf = vec![0u8; opts.resources.stream_buffer_size(64 * 1024)?];
    let mut done = 0u64;
    reader.read_entry(&meta.path, &mut |data| {
        loop {
            ctl.checkpoint()?;
            let n = data.read(&mut buf)?;
            if n == 0 {
                break;
            }
            std::io::Write::write_all(&mut sink, &buf[..n])?;
            done += n as u64;
            label.on_progress(done, 0, &meta.path);
        }
        Ok(())
    })?;
    sink.finish()?;
    progress.on_progress(meta.size, meta.size, &EntryPath::from_utf8(""));
    Ok(())
}
