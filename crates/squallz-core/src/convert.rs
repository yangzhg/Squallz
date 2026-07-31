//! Format conversion: stream every entry of a source archive into a new
//! archive of the format chosen by the destination extension, reusing the
//! create-side format selection (compound pipelines included) without
//! extracting to disk.

use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::sync::Arc;

use crate::api::{
    ArchiveReader, Compressor, ControlToken, CreateOptions, Detected, EntryMeta, EntryPath,
    EntryType, FormatError, OpenOptions, ProgressSink,
};
use crate::compound::{KnownTotal, ProgressRead};
use crate::create::{
    create_plan_from_summary, ensure_create_space, open_dest_from_reserved_file,
    validate_create_target_name, with_split_output_policy, DestSink, DestTarget,
};
use crate::{
    create_input_summary, CreateArtifactKind, CreateCommitPolicy, CreateInputEstimate, CreatePlan,
    CreateReport, Engine,
};

/// Entry point for [`Engine::convert_with_report`]. Metadata is carried over as
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
    requested_commit_policy: Option<CreateCommitPolicy>,
    progress: &dyn ProgressSink,
    ctl: &ControlToken,
) -> Result<CreateReport, FormatError> {
    let detect_name = dest
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| FormatError::Unsupported("invalid output file name".into()))?;
    validate_create_target_name(engine, detect_name, create_opts)?;

    let artifact_kind = if create_opts.split_size.is_some() {
        CreateArtifactKind::SplitArchive
    } else {
        CreateArtifactKind::Archive
    };
    let commit_policy = match requested_commit_policy {
        Some(CreateCommitPolicy::NoReplace) => {
            if crate::create_destination_has_conflict(dest, artifact_kind)? {
                return Err(crate::output_exists_error(dest));
            }
            CreateCommitPolicy::NoReplace
        }
        Some(CreateCommitPolicy::ReplaceIfUnchanged(guard)) => {
            CreateCommitPolicy::ReplaceIfUnchanged(guard)
        }
        Some(CreateCommitPolicy::ReplaceExisting) | None => {
            let inspection = crate::inspect_create_destination(dest, artifact_kind)?;
            match (inspection.conflict, inspection.guard) {
                (false, None) => CreateCommitPolicy::NoReplace,
                (true, Some(guard)) => CreateCommitPolicy::ReplaceIfUnchanged(guard),
                _ => {
                    return Err(FormatError::Other(
                        "conversion destination inspection returned an inconsistent result".into(),
                    ));
                }
            }
        }
    };

    let mut reader = engine.open(src, open_opts)?;
    let metas = collect_entry_metadata(&mut *reader, ctl)?;
    let plan = plan_convert_from_entries(engine, dest, &metas, create_opts)?;
    ensure_create_space(dest, &plan)?;
    if create_opts.split_size.is_none() {
        let reserved = crate::reserve_bound_sibling_temp_file(dest, "convert")?;
        let staged = reserved.path.clone();
        let staged_identity = reserved.identity;
        let mut retained_file = Some(reserved.file.try_clone()?);
        let mut cleanup_staging = true;
        let result = convert_unsplit(
            engine,
            &mut *reader,
            &metas,
            detect_name,
            reserved.file,
            create_opts,
            create_opts,
            progress,
            ctl,
        );
        drop(reader);
        let result = result.and_then(|()| {
            let retained = retained_file.as_ref().ok_or_else(|| {
                FormatError::Other("conversion staging handle was transferred twice".into())
            })?;
            if crate::filesystem_identity::file_identity(retained)? != staged_identity
                || crate::filesystem_identity::path_identity(&staged)? != staged_identity
            {
                return Err(FormatError::Io(std::io::Error::other(
                    "conversion staging changed after writing",
                )));
            }
            let total_output_bytes = retained.metadata()?.len();
            match commit_policy {
                CreateCommitPolicy::NoReplace => {
                    crate::publish_bound_file_no_replace(&staged, retained, staged_identity, dest)?
                }
                CreateCommitPolicy::ReplaceIfUnchanged(guard) => {
                    cleanup_staging = false;
                    let retained = retained_file.take().ok_or_else(|| {
                        FormatError::Other("conversion staging handle was transferred twice".into())
                    })?;
                    crate::update::commit_created_archive(
                        dest,
                        &staged,
                        retained,
                        staged_identity,
                        guard,
                        progress,
                        ctl,
                    )?;
                }
                CreateCommitPolicy::ReplaceExisting => {
                    return Err(FormatError::Other(
                        "conversion replacement policy was not bound before writing".into(),
                    ));
                }
            }
            Ok(CreateReport {
                primary_output: dest.to_path_buf(),
                outputs: vec![dest.to_path_buf()],
                preserved_outputs: Vec::new(),
                total_output_bytes,
                split_volume_count: None,
            })
        });
        return match result {
            Ok(report) => Ok(report),
            Err(error) if !cleanup_staging => Err(error),
            Err(error) => {
                let retained = retained_file.as_ref().ok_or_else(|| {
                    FormatError::Other(
                        "conversion staging handle was unavailable for cleanup".into(),
                    )
                })?;
                match crate::remove_bound_temp_file(&staged, retained, staged_identity) {
                    Ok(()) => Err(error),
                    Err(cleanup) => Err(FormatError::Other(format!(
                        "{error}; conversion staging cleanup also failed: {cleanup}"
                    ))),
                }
            }
        };
    }

    with_split_output_policy(
        engine,
        dest,
        create_opts,
        progress,
        ctl,
        commit_policy,
        move |detect_name, _out_path, opts, reserved| {
            let reserved = reserved.ok_or_else(|| {
                FormatError::Other("split conversion lost its reserved output".into())
            })?;
            let result = convert_unsplit(
                engine,
                &mut *reader,
                &metas,
                detect_name,
                reserved.file,
                opts,
                create_opts,
                progress,
                ctl,
            );
            drop(reader);
            result
        },
    )
    .map(|(_value, outputs)| outputs.into_report())
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

#[allow(clippy::too_many_arguments)] // internal plumbing with distinct roles
fn convert_unsplit(
    engine: &Engine,
    reader: &mut dyn ArchiveReader,
    metas: &[EntryMeta],
    detect_name: &str,
    output: File,
    write_opts: &CreateOptions,
    validation_opts: &CreateOptions,
    progress: &dyn ProgressSink,
    ctl: &ControlToken,
) -> Result<(), FormatError> {
    match open_dest_from_reserved_file(
        engine,
        detect_name,
        output,
        write_opts,
        validation_opts,
        ctl,
    )? {
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
    for meta in metas {
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
                let data = reader.read_entry(&meta.path)?;
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
    }
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
    let mut data = reader.read_entry(&meta.path)?;
    // Pump locally: the entry reader is not `Send`, so the trait's chunked
    // pump cannot be used here.
    let mut sink = compressor.compress_writer(Box::new(dst), opts.level, &opts.resources)?;
    let label = KnownTotal::new(progress, meta.size, meta.path.clone());
    let mut buf = vec![0u8; opts.resources.stream_buffer_size(64 * 1024)?];
    let mut done = 0u64;
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
    sink.finish()?;
    progress.on_progress(meta.size, meta.size, &EntryPath::from_utf8(""));
    Ok(())
}
