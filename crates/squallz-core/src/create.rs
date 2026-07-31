//! Archive creation: format selection by destination name, the shared
//! destination-writer machinery (plain / compound / single-stream, reused
//! by format conversion) and `.001` split-volume output.

use std::cell::RefCell;
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::api::{
    split_volume_name, ArchiveFormat, ArchiveWriter, Compressor, ControlToken, CreateOptions,
    Detected, EntryMeta, EntryPath, EntryType, FormatCreateBudget, FormatError, ProgressSink,
    SplitOutputMode,
};
use crate::compound::{KnownTotal, ProgressRead, SharedCompressSink};
use crate::inputs::{
    collect_inputs_excluding_with_progress, collect_prepared_inputs_excluding_with_progress,
    deduplicate_prepared_input_roots, prepare_single_stream_input, InputItem, PreparedInputItem,
};
use crate::volumes::{self, MIN_SPLIT_SIZE};
use crate::{
    CreateArtifactKind, CreateCommitPolicy, CreateInputEstimate, CreateInputFingerprint,
    CreateInputManifestEntry, CreateInputModifiedTime, CreateInputSummary, CreatePlan,
    CreateReport, Engine, PathFilter, VerifiedCreateReport,
};

pub(crate) struct OutputArtifacts {
    primary_output: PathBuf,
    outputs: Vec<PathBuf>,
    preserved_outputs: Vec<PathBuf>,
    total_output_bytes: u64,
    split_volume_count: Option<usize>,
}

impl OutputArtifacts {
    pub(crate) fn into_report(self) -> CreateReport {
        CreateReport {
            primary_output: self.primary_output,
            outputs: self.outputs,
            preserved_outputs: self.preserved_outputs,
            total_output_bytes: self.total_output_bytes,
            split_volume_count: self.split_volume_count,
        }
    }
}

/// Destination writer over an archive output: plain (`x.zip`) or compound
/// (`x.tar.gz`, where finishing must also flush the compressor sink).
pub(crate) struct DestSink {
    writer: Box<dyn ArchiveWriter>,
    shared: Option<SharedCompressSink>,
}

impl DestSink {
    pub(crate) fn add_entry(
        &mut self,
        meta: &EntryMeta,
        data: Option<&mut dyn std::io::Read>,
    ) -> Result<(), FormatError> {
        self.writer.add_entry(meta, data)
    }

    pub(crate) fn finish(self) -> Result<(), FormatError> {
        self.writer.finish()?;
        match self.shared {
            Some(shared) => shared.finish(),
            None => Ok(()),
        }
    }
}

/// Resolved destination of a create/convert operation.
pub(crate) enum DestTarget {
    /// An archive container (possibly compound).
    Archive(DestSink),
    /// A bare single-stream compressor (`x.gz`): the caller must supply
    /// exactly one file's content.
    SingleStream {
        compressor: Arc<dyn Compressor>,
        output: File,
    },
}

pub(crate) fn open_dest_from_reserved_file(
    engine: &Engine,
    detect_name: &str,
    output: File,
    write_opts: &CreateOptions,
    validation_opts: &CreateOptions,
    ctl: &ControlToken,
) -> Result<DestTarget, FormatError> {
    open_dest_with(
        engine,
        detect_name,
        write_opts,
        validation_opts,
        ctl,
        || Ok(output),
    )
}

fn open_dest_with<F>(
    engine: &Engine,
    detect_name: &str,
    write_opts: &CreateOptions,
    validation_opts: &CreateOptions,
    ctl: &ControlToken,
    open_file: F,
) -> Result<DestTarget, FormatError>
where
    F: FnOnce() -> Result<File, io::Error>,
{
    let detected = engine
        .registry()
        .detect_by_name(detect_name)
        .ok_or_else(|| {
            FormatError::Unsupported(format!(
                "creating this format is not supported: {detect_name}"
            ))
        })?;
    validate_detected_create_target(&detected, detect_name, validation_opts)?;
    match detected {
        Detected::Archive(f) => {
            let file = open_file()?;
            let writer = f.create_with_control(Box::new(file), write_opts, ctl)?;
            Ok(DestTarget::Archive(DestSink {
                writer,
                shared: None,
            }))
        }
        Detected::Compressed {
            compressor,
            inner_archive: Some(archive),
        } => {
            let file = open_file()?;
            let sink = compressor.compress_writer(
                Box::new(file),
                write_opts.level,
                &write_opts.resources,
            )?;
            let shared = SharedCompressSink::new(sink);
            ctl.checkpoint()?;
            let writer = archive.create_stream(Box::new(shared.clone()), write_opts)?;
            ctl.checkpoint()?;
            Ok(DestTarget::Archive(DestSink {
                writer,
                shared: Some(shared),
            }))
        }
        Detected::Compressed {
            compressor,
            inner_archive: None,
        } => Ok(DestTarget::SingleStream {
            compressor,
            output: open_file()?,
        }),
    }
}

pub(crate) fn validate_create_target_name(
    engine: &Engine,
    detect_name: &str,
    opts: &CreateOptions,
) -> Result<(), FormatError> {
    let detected = engine
        .registry()
        .detect_by_name(detect_name)
        .ok_or_else(|| {
            FormatError::Unsupported(format!(
                "creating this format is not supported: {detect_name}"
            ))
        })?;
    validate_detected_create_target(&detected, detect_name, opts)
}

fn validate_detected_create_target(
    detected: &Detected,
    detect_name: &str,
    opts: &CreateOptions,
) -> Result<(), FormatError> {
    match detected {
        Detected::Archive(format)
        | Detected::Compressed {
            inner_archive: Some(format),
            ..
        } => check_can_create(format, detect_name, opts),
        Detected::Compressed {
            compressor,
            inner_archive: None,
        } if opts.password.is_some() => Err(FormatError::Unsupported(format!(
            "format {} does not support encryption",
            compressor.id()
        ))),
        Detected::Compressed {
            inner_archive: None,
            ..
        } => Ok(()),
    }?;
    if opts.split_mode != SplitOutputMode::Native {
        return Ok(());
    }
    let split_size = opts.split_size.ok_or_else(|| {
        FormatError::Unsupported("native volume mode requires a split size".into())
    })?;
    let Detected::Archive(format) = detected else {
        return Err(FormatError::Unsupported(
            "native volume creation requires a directly supported archive format".into(),
        ));
    };
    let limits = format.native_volume_limits().ok_or_else(|| {
        FormatError::Unsupported(format!(
            "format {} does not support native volume creation",
            format.id()
        ))
    })?;
    if !(limits.min_volume_size..=limits.max_volume_size).contains(&split_size) {
        return Err(FormatError::Unsupported(format!(
            "native {} volume size must be between {} and {} bytes",
            format.id(),
            limits.min_volume_size,
            limits.max_volume_size
        )));
    }
    format.native_volume_path(Path::new(detect_name), 0, true)?;
    Ok(())
}

/// Rejects formats that declare `can_create=false`.
fn check_can_create(
    format: &Arc<dyn ArchiveFormat>,
    detect_name: &str,
    opts: &CreateOptions,
) -> Result<(), FormatError> {
    if !format.capabilities().can_create {
        return Err(FormatError::Unsupported(format!(
            "format {} does not support creation",
            format.id()
        )));
    }
    format.validate_create_options(detect_name, opts)
}

/// Entry point for [`Engine::create`].
pub(crate) fn create(
    engine: &Engine,
    dest: &Path,
    inputs: &[PathBuf],
    opts: &CreateOptions,
    progress: &dyn ProgressSink,
    ctl: &ControlToken,
) -> Result<CreateReport, FormatError> {
    create_with_reserved_outputs(engine, dest, inputs, &[], opts, progress, ctl)
}

/// Entry point for [`Engine::create_with_report_no_replace`].
pub(crate) fn create_no_replace(
    engine: &Engine,
    dest: &Path,
    inputs: &[PathBuf],
    opts: &CreateOptions,
    progress: &dyn ProgressSink,
    ctl: &ControlToken,
) -> Result<CreateReport, FormatError> {
    create_with_reserved_outputs_and_policy(
        engine,
        dest,
        inputs,
        &[],
        opts,
        progress,
        ctl,
        None,
        CreateCommitPolicy::NoReplace,
        false,
        None,
        true,
    )
    .map(|report| report.create)
}

/// Entry point for the verified create APIs.
pub(crate) fn create_verified(
    engine: &Engine,
    dest: &Path,
    inputs: &[PathBuf],
    opts: &CreateOptions,
    progress: &dyn ProgressSink,
    ctl: &ControlToken,
    commit_policy: CreateCommitPolicy,
) -> Result<VerifiedCreateReport, FormatError> {
    create_with_reserved_outputs_and_policy(
        engine,
        dest,
        inputs,
        &[],
        opts,
        progress,
        ctl,
        None,
        commit_policy,
        true,
        None,
        true,
    )
}

/// Entry point for report creation with an explicit final publication policy.
pub(crate) fn create_report_with_policy(
    engine: &Engine,
    dest: &Path,
    inputs: &[PathBuf],
    opts: &CreateOptions,
    commit_policy: CreateCommitPolicy,
    progress: &dyn ProgressSink,
    ctl: &ControlToken,
) -> Result<CreateReport, FormatError> {
    create_with_reserved_outputs_and_policy(
        engine,
        dest,
        inputs,
        &[],
        opts,
        progress,
        ctl,
        None,
        commit_policy,
        false,
        None,
        true,
    )
    .map(|report| report.create)
}

/// Creates an archive while reserving outputs owned by a surrounding
/// operation, such as the final SFX artifact that will wrap this archive.
pub(crate) fn create_with_reserved_outputs(
    engine: &Engine,
    dest: &Path,
    inputs: &[PathBuf],
    reserved_outputs: &[&Path],
    opts: &CreateOptions,
    progress: &dyn ProgressSink,
    ctl: &ControlToken,
) -> Result<CreateReport, FormatError> {
    create_with_reserved_outputs_and_policy(
        engine,
        dest,
        inputs,
        reserved_outputs,
        opts,
        progress,
        ctl,
        None,
        CreateCommitPolicy::ReplaceExisting,
        false,
        None,
        true,
    )
    .map(|report| report.create)
}

/// Prepares one unsplit archive input manifest for a surrounding operation.
/// The returned value is consumed by [`create_prepared_with_reserved_outputs`]
/// so planning and writing use the same accepted entry set.
pub(crate) fn prepare_unsplit_create_with_reserved_outputs(
    engine: &Engine,
    dest: &Path,
    inputs: &[PathBuf],
    reserved_outputs: &[&Path],
    opts: &CreateOptions,
    mut progress: impl FnMut(usize, &str),
) -> Result<PreparedCreateInputs, FormatError> {
    if opts.split_size.is_some() {
        return Err(FormatError::Unsupported(
            "prepared creation requires one complete archive output".into(),
        ));
    }
    let detect_name = dest
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| FormatError::Unsupported("invalid output file name".into()))?;
    let output_exclusions = CreateOutputExclusions::for_unsplit_estimate(dest, reserved_outputs);
    output_exclusions.reject_explicit_inputs(inputs)?;
    prepare_create_inputs(
        engine,
        detect_name,
        inputs,
        opts,
        &output_exclusions,
        |count, path| progress(count, &path.display),
    )
}

/// Writes a manifest returned by
/// [`prepare_unsplit_create_with_reserved_outputs`] without walking its input
/// roots again.
#[allow(clippy::too_many_arguments)] // internal create plumbing; each role is distinct
#[cfg(test)]
pub(crate) fn create_prepared_with_reserved_outputs(
    engine: &Engine,
    dest: &Path,
    inputs: &[PathBuf],
    reserved_outputs: &[&Path],
    opts: &CreateOptions,
    progress: &dyn ProgressSink,
    ctl: &ControlToken,
    prepared: PreparedCreateInputs,
    capture_input_manifest: bool,
) -> Result<VerifiedCreateReport, FormatError> {
    if opts.split_size.is_some() {
        return Err(FormatError::Unsupported(
            "prepared creation requires one complete archive output".into(),
        ));
    }
    create_with_reserved_outputs_and_policy(
        engine,
        dest,
        inputs,
        reserved_outputs,
        opts,
        progress,
        ctl,
        Some(prepared),
        CreateCommitPolicy::ReplaceExisting,
        capture_input_manifest,
        None,
        true,
    )
}

/// Writes a prepared internal archive directly through a caller-owned
/// reservation. The caller retains another handle until the surrounding
/// artifact has consumed the archive.
#[allow(clippy::too_many_arguments)] // internal SFX boundary with distinct roles
pub(crate) fn create_prepared_into_reserved_output(
    engine: &Engine,
    dest: &Path,
    inputs: &[PathBuf],
    reserved_outputs: &[&Path],
    opts: &CreateOptions,
    progress: &dyn ProgressSink,
    ctl: &ControlToken,
    prepared: PreparedCreateInputs,
    capture_input_manifest: bool,
    reserved: crate::ReservedTempFile,
) -> Result<VerifiedCreateReport, FormatError> {
    if opts.split_size.is_some() {
        return Err(FormatError::Unsupported(
            "prepared creation requires one complete archive output".into(),
        ));
    }
    create_with_reserved_outputs_and_policy(
        engine,
        dest,
        inputs,
        reserved_outputs,
        opts,
        progress,
        ctl,
        Some(prepared),
        CreateCommitPolicy::ReplaceExisting,
        capture_input_manifest,
        Some(reserved),
        false,
    )
}

#[allow(clippy::too_many_arguments)] // internal create plumbing; each role is distinct
fn create_with_reserved_outputs_and_policy(
    engine: &Engine,
    dest: &Path,
    inputs: &[PathBuf],
    reserved_outputs: &[&Path],
    opts: &CreateOptions,
    progress: &dyn ProgressSink,
    ctl: &ControlToken,
    prepared: Option<PreparedCreateInputs>,
    commit_policy: CreateCommitPolicy,
    capture_input_manifest: bool,
    reserved_unsplit_staging: Option<crate::ReservedTempFile>,
    publish_unsplit_staging: bool,
) -> Result<VerifiedCreateReport, FormatError> {
    if reserved_unsplit_staging.is_some() && opts.split_size.is_some() {
        return Err(FormatError::Unsupported(
            "a caller-owned output reservation cannot be split".into(),
        ));
    }
    let name = dest
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| FormatError::Unsupported("invalid output file name".into()))?;
    let detect_name = if opts.split_size.is_some() {
        split_volume_name(name)
            .map(|(base, _index)| base)
            .unwrap_or(name)
    } else {
        name
    };
    validate_create_target_name(engine, detect_name, opts)?;

    let split_output = opts.split_size.is_some();
    let mut reserved_unsplit_staging = reserved_unsplit_staging;
    let (staged_archive, mut outputs) = with_split_output_policy(
        engine,
        dest,
        opts,
        progress,
        ctl,
        commit_policy,
        |detect_name, out_path, write_opts, reserved_staging| {
            create_unsplit(
                engine,
                detect_name,
                CreateTarget {
                    final_path: dest,
                    staging_path: out_path,
                    split: split_output,
                    reserved_outputs,
                    commit_policy,
                    capture_input_manifest,
                    reserved_staging: reserved_staging.or_else(|| reserved_unsplit_staging.take()),
                    publish_staging: publish_unsplit_staging,
                },
                inputs,
                prepared,
                write_opts,
                opts,
                progress,
                ctl,
            )
        },
    )?;
    if outputs.split_volume_count.is_none() {
        outputs.total_output_bytes = staged_archive.output_bytes;
    }
    Ok(VerifiedCreateReport {
        create: outputs.into_report(),
        inputs: staged_archive.inputs,
        manifest: staged_archive.manifest,
    })
}

/// Runs `write` against the final path directly, or — when
/// `opts.split_size` is set — against a temporary file that is then cut
/// into `.001`-style volumes and committed with the caller's destination
/// policy.
pub(crate) fn with_split_output_policy<T>(
    engine: &Engine,
    dest: &Path,
    opts: &CreateOptions,
    progress: &dyn ProgressSink,
    ctl: &ControlToken,
    commit_policy: CreateCommitPolicy,
    write: impl FnOnce(
        &str,
        &Path,
        &CreateOptions,
        Option<crate::ReservedTempFile>,
    ) -> Result<T, FormatError>,
) -> Result<(T, OutputArtifacts), FormatError> {
    let name = dest
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| FormatError::Unsupported("invalid output file name".into()))?;
    let Some(split) = opts.split_size else {
        let value = write(name, dest, opts, None)?;
        let output = dest.to_path_buf();
        return Ok((
            value,
            OutputArtifacts {
                primary_output: output.clone(),
                outputs: vec![output],
                preserved_outputs: Vec::new(),
                // The create path replaces this with its staging measurement;
                // conversion discards the artifact report.
                total_output_bytes: 0,
                split_volume_count: None,
            },
        ));
    };
    if split < MIN_SPLIT_SIZE {
        return Err(FormatError::Unsupported(format!(
            "split size below the {MIN_SPLIT_SIZE}-byte minimum: {split}"
        )));
    }
    // Accept an explicit first-volume name (`x.zip.001`) as the base too.
    let (base_name, base) = match split_volume_name(name) {
        Some((stripped, _)) => (stripped.to_string(), dest.with_file_name(stripped)),
        None => (name.to_string(), dest.to_path_buf()),
    };
    volumes::validate_split_output_base(&base)?;
    let reserved = crate::reserve_bound_sibling_temp_file(&base, "split")?;
    let tmp = reserved.path.clone();
    let tmp_identity = reserved.identity;
    let retained_tmp = reserved.file.try_clone()?;
    let inner_opts = CreateOptions {
        split_size: None,
        split_mode: SplitOutputMode::Generic,
        ..opts.clone()
    };
    let result = (|| {
        let value = write(&base_name, &tmp, &inner_opts, Some(reserved))?;
        let split_outputs = match opts.split_mode {
            SplitOutputMode::Generic => {
                volumes::split_into_volumes_with_commit_policy_and_source_identity(
                    &tmp,
                    retained_tmp.try_clone()?,
                    tmp_identity,
                    &base,
                    split,
                    &opts.resources,
                    progress,
                    ctl,
                    commit_policy,
                )?
            }
            SplitOutputMode::Native => {
                let detected = engine
                    .registry()
                    .detect_by_name(&base_name)
                    .ok_or_else(|| {
                        FormatError::Unsupported(format!(
                            "creating this format is not supported: {base_name}"
                        ))
                    })?;
                let Detected::Archive(format) = detected else {
                    return Err(FormatError::Unsupported(
                        "native volume creation requires a directly supported archive format"
                            .into(),
                    ));
                };
                volumes::split_into_native_volumes_with_commit_policy_and_source_identity(
                    &tmp,
                    retained_tmp.try_clone()?,
                    tmp_identity,
                    &base,
                    split,
                    &*format,
                    &opts.resources,
                    progress,
                    ctl,
                    commit_policy,
                )?
            }
        };
        let primary_output = split_outputs
            .volumes
            .get(split_outputs.primary_volume_index)
            .cloned()
            .ok_or_else(|| {
                FormatError::Other("split creation returned no primary volume".into())
            })?;
        let split_volume_count = split_outputs.volumes.len();
        let mut outputs = split_outputs.volumes;
        outputs.extend(split_outputs.sidecars);
        Ok((
            value,
            OutputArtifacts {
                primary_output,
                outputs,
                preserved_outputs: split_outputs.preserved_outputs,
                total_output_bytes: split_outputs.total_output_bytes,
                split_volume_count: Some(split_volume_count),
            },
        ))
    })();
    if result.is_err() {
        let _ = crate::remove_bound_temp_file(&tmp, &retained_tmp, tmp_identity);
    }
    result
}

struct CreateTarget<'a> {
    final_path: &'a Path,
    staging_path: &'a Path,
    split: bool,
    reserved_outputs: &'a [&'a Path],
    commit_policy: CreateCommitPolicy,
    capture_input_manifest: bool,
    reserved_staging: Option<crate::ReservedTempFile>,
    publish_staging: bool,
}

struct CreatedArchive {
    output_bytes: u64,
    inputs: Vec<CreateInputFingerprint>,
    manifest: Vec<CreateInputManifestEntry>,
}

/// Single-output creation from file-system inputs.
#[allow(clippy::too_many_arguments)] // internal create plumbing; each path/options role is distinct
fn create_unsplit(
    engine: &Engine,
    detect_name: &str,
    target: CreateTarget<'_>,
    inputs: &[PathBuf],
    prepared: Option<PreparedCreateInputs>,
    write_opts: &CreateOptions,
    plan_opts: &CreateOptions,
    progress: &dyn ProgressSink,
    ctl: &ControlToken,
) -> Result<CreatedArchive, FormatError> {
    let mut target = target;
    if !target.split
        && target.publish_staging
        && matches!(target.commit_policy, CreateCommitPolicy::ReplaceExisting)
    {
        let inspection =
            crate::inspect_create_destination(target.staging_path, CreateArtifactKind::Archive)?;
        target.commit_policy = match inspection.guard {
            Some(guard) => CreateCommitPolicy::ReplaceIfUnchanged(guard),
            None => CreateCommitPolicy::NoReplace,
        };
    }
    let reserved = match target.reserved_staging.take() {
        Some(reserved) => reserved,
        None => crate::reserve_bound_sibling_temp_file(target.staging_path, "create")?,
    };
    let tmp = reserved.path.clone();
    let tmp_identity = reserved.identity;
    let mut retained_file = Some(reserved.file.try_clone()?);
    let output_file = reserved.file;
    let mut cleanup_tmp = true;
    let result = (|| {
        let output_exclusions = CreateOutputExclusions::new(
            target.final_path,
            target.staging_path,
            &tmp,
            target.split,
            target.reserved_outputs,
        )?;
        output_exclusions.reject_explicit_inputs(inputs)?;
        let prepared = match prepared {
            Some(prepared) if prepared.detect_name == detect_name => prepared,
            Some(_) => {
                return Err(FormatError::Other(
                    "prepared create format does not match destination".into(),
                ));
            }
            None => prepare_create_inputs(
                engine,
                detect_name,
                inputs,
                plan_opts,
                &output_exclusions,
                |_count, _path| {},
            )?,
        };
        let plan = create_plan_from_summary(
            engine,
            detect_name,
            target.final_path,
            prepared.summary,
            plan_opts,
        )?;
        ensure_create_space(target.final_path, &plan)?;
        let captured_inputs = match (
            open_dest_from_reserved_file(
                engine,
                detect_name,
                output_file,
                write_opts,
                plan_opts,
                ctl,
            )?,
            prepared.items,
        ) {
            (DestTarget::Archive(mut sink), PreparedInputs::Archive(items)) => {
                let inputs = write_entries(
                    &mut sink,
                    &items,
                    write_opts,
                    progress,
                    ctl,
                    target.capture_input_manifest,
                )?;
                sink.finish()?;
                inputs
            }
            (
                DestTarget::SingleStream { compressor, output },
                PreparedInputs::SingleStream(item),
            ) => create_single_stream(
                &compressor,
                output,
                &item,
                write_opts,
                progress,
                ctl,
                target.capture_input_manifest,
            )?,
            _ => {
                return Err(FormatError::Other(
                    "create destination changed while preparing inputs".into(),
                ));
            }
        };
        let retained = retained_file.as_ref().ok_or_else(|| {
            FormatError::Other("created archive staging handle was transferred too early".into())
        })?;
        if crate::filesystem_identity::file_identity(retained)? != tmp_identity
            || crate::filesystem_identity::path_identity(&tmp)? != tmp_identity
        {
            return Err(FormatError::Io(io::Error::other(format!(
                "created archive staging changed before publication: {}",
                tmp.display()
            ))));
        }
        let output_bytes = retained.metadata()?.len();
        if !target.split && target.publish_staging {
            match target.commit_policy {
                CreateCommitPolicy::ReplaceExisting => {
                    return Err(FormatError::Other(
                        "archive replacement policy was not bound before writing".into(),
                    ));
                }
                CreateCommitPolicy::NoReplace => {
                    crate::publish_bound_file_no_replace(
                        &tmp,
                        retained,
                        tmp_identity,
                        target.staging_path,
                    )?;
                }
                CreateCommitPolicy::ReplaceIfUnchanged(guard) => {
                    cleanup_tmp = false;
                    let retained = retained_file.take().ok_or_else(|| {
                        FormatError::Other(
                            "created archive staging handle was transferred twice".into(),
                        )
                    })?;
                    crate::update::commit_created_archive(
                        target.staging_path,
                        &tmp,
                        retained,
                        tmp_identity,
                        guard,
                        progress,
                        ctl,
                    )?;
                }
            }
        }
        Ok(CreatedArchive {
            output_bytes,
            inputs: captured_inputs.fingerprints,
            manifest: captured_inputs.manifest,
        })
    })();
    match result {
        Ok(created) => Ok(created),
        Err(error) if !cleanup_tmp => Err(error),
        Err(error) => match retained_file.as_ref() {
            None => Err(error),
            Some(retained) => match crate::remove_bound_temp_file(&tmp, retained, tmp_identity) {
                Ok(()) => Err(error),
                Err(cleanup) => Err(FormatError::Other(format!(
                    "{error}; created archive staging cleanup also failed: {cleanup}"
                ))),
            },
        },
    }
}

enum PreparedInputs {
    Archive(Vec<PreparedInputItem>),
    SingleStream(Box<PreparedInputItem>),
}

pub(crate) struct PreparedCreateInputs {
    detect_name: String,
    summary: CreateInputSummary,
    items: PreparedInputs,
}

impl PreparedCreateInputs {
    pub(crate) fn summary(&self) -> CreateInputSummary {
        self.summary
    }
}

fn prepare_create_inputs(
    engine: &Engine,
    detect_name: &str,
    inputs: &[PathBuf],
    opts: &CreateOptions,
    output_exclusions: &CreateOutputExclusions,
    mut progress: impl FnMut(usize, &EntryPath),
) -> Result<PreparedCreateInputs, FormatError> {
    match engine.registry().detect_by_name(detect_name) {
        Some(Detected::Archive(format)) => {
            check_can_create(&format, detect_name, opts)?;
            let excludes = PathFilter::new(&opts.excludes)?;
            let items = collect_prepared_with_output_exclusions(
                inputs,
                &excludes,
                output_exclusions,
                &mut progress,
            )?;
            let summary = crate::summarize_create_input_manifest(inputs.len(), &items);
            Ok(PreparedCreateInputs {
                detect_name: detect_name.to_owned(),
                summary,
                items: PreparedInputs::Archive(items),
            })
        }
        Some(Detected::Compressed {
            compressor: _,
            inner_archive: Some(archive),
        }) => {
            check_can_create(&archive, detect_name, opts)?;
            let excludes = PathFilter::new(&opts.excludes)?;
            let items = collect_prepared_with_output_exclusions(
                inputs,
                &excludes,
                output_exclusions,
                &mut progress,
            )?;
            let summary = crate::summarize_create_input_manifest(inputs.len(), &items);
            Ok(PreparedCreateInputs {
                detect_name: detect_name.to_owned(),
                summary,
                items: PreparedInputs::Archive(items),
            })
        }
        Some(Detected::Compressed {
            compressor,
            inner_archive: None,
        }) => {
            if opts.password.is_some() {
                return Err(FormatError::Unsupported(format!(
                    "format {} does not support encryption",
                    compressor.id()
                )));
            }
            if inputs.len() > 1 {
                for input in inputs {
                    let metadata = std::fs::symlink_metadata(input)?;
                    if metadata.file_type().is_symlink() || !metadata.is_file() {
                        return Err(FormatError::Unsupported(format!(
                            "format {} compresses exactly one file",
                            compressor.id()
                        )));
                    }
                }
            }
            let mut prepared_items = Vec::with_capacity(inputs.len());
            for (index, input) in inputs.iter().enumerate() {
                let label = EntryPath::from_utf8(
                    input
                        .file_name()
                        .map_or_else(String::new, |name| name.to_string_lossy().into_owned()),
                );
                let item =
                    prepare_single_stream_input(input, label.clone()).map_err(
                        |error| match error {
                            FormatError::Unsupported(_) => FormatError::Unsupported(format!(
                                "format {} compresses a single regular file",
                                compressor.id()
                            )),
                            error => error,
                        },
                    )?;
                progress(index.saturating_add(1), &label);
                prepared_items.push(item);
            }
            let mut items = deduplicate_prepared_input_roots(prepared_items)?;
            if items.len() != 1 {
                return Err(FormatError::Unsupported(format!(
                    "format {} compresses exactly one file",
                    compressor.id()
                )));
            }
            let Some(item) = items.pop() else {
                return Err(FormatError::Other(
                    "single-stream input reconciliation lost its prepared file".into(),
                ));
            };
            let estimate = CreateInputEstimate {
                input_count: inputs.len(),
                entries: 1,
                files: 1,
                total_bytes: item.item().size,
                ..CreateInputEstimate::default()
            };
            Ok(PreparedCreateInputs {
                detect_name: detect_name.to_owned(),
                summary: CreateInputSummary {
                    estimate,
                    archive_budget_bytes: estimate.output_budget_bytes(),
                },
                items: PreparedInputs::SingleStream(Box::new(item)),
            })
        }
        None => Err(FormatError::Unsupported(format!(
            "creating this format is not supported: {detect_name}"
        ))),
    }
}

pub(crate) fn create_plan_from_summary(
    engine: &Engine,
    detect_name: &str,
    dest: &Path,
    summary: CreateInputSummary,
    opts: &CreateOptions,
) -> Result<CreatePlan, FormatError> {
    let format_budget = format_create_budget(engine, detect_name, summary, opts)?;
    let archive_budget = format_budget.output_bytes;
    let mut plan = if let Some(split_size) = opts.split_size {
        if split_size < MIN_SPLIT_SIZE {
            return Err(FormatError::Unsupported(format!(
                "split size below the {MIN_SPLIT_SIZE}-byte minimum: {split_size}"
            )));
        }
        let base = split_output_base(dest)?;
        volumes::validate_split_output_base(&base)?;
        let (split_budget, primary_output) = match opts.split_mode {
            SplitOutputMode::Generic => (
                volumes::split_output_budget(&base, archive_budget, split_size)?,
                volumes::first_volume_path(&base),
            ),
            SplitOutputMode::Native => {
                let detected = engine
                    .registry()
                    .detect_by_name(detect_name)
                    .ok_or_else(|| {
                        FormatError::Unsupported(format!(
                            "creating this format is not supported: {detect_name}"
                        ))
                    })?;
                let Detected::Archive(format) = detected else {
                    return Err(FormatError::Unsupported(
                        "native volume creation requires a directly supported archive format"
                            .into(),
                    ));
                };
                let budget = volumes::native_split_output_budget(
                    &*format,
                    archive_budget,
                    summary.estimate.entries as u64,
                    split_size,
                )?;
                let primary_index = format.native_volume_primary_index(1)?;
                let primary = format.native_volume_path(&base, primary_index, true)?;
                (budget, primary)
            }
        };
        CreatePlan {
            inputs: summary.estimate,
            primary_output,
            archive_output_budget_bytes: archive_budget,
            final_output_budget_bytes: split_budget.final_output_bytes,
            split_volume_count_budget: Some(split_budget.volume_count),
            workspace_budget_bytes: archive_budget
                .saturating_add(split_budget.additional_space_bytes),
            system_temp_budget_bytes: format_budget.system_temp_bytes,
        }
    } else {
        CreatePlan {
            inputs: summary.estimate,
            primary_output: dest.to_path_buf(),
            archive_output_budget_bytes: archive_budget,
            final_output_budget_bytes: archive_budget,
            split_volume_count_budget: None,
            workspace_budget_bytes: archive_budget,
            system_temp_budget_bytes: format_budget.system_temp_bytes,
        }
    };
    normalize_create_plan_space(dest, &mut plan);
    Ok(plan)
}

fn format_create_budget(
    engine: &Engine,
    detect_name: &str,
    summary: CreateInputSummary,
    opts: &CreateOptions,
) -> Result<FormatCreateBudget, FormatError> {
    let archive_bytes = summary.archive_budget_bytes;
    match engine.registry().detect_by_name(detect_name) {
        Some(Detected::Archive(format))
        | Some(Detected::Compressed {
            inner_archive: Some(format),
            ..
        }) => format.create_budget(summary.estimate.total_bytes, archive_bytes, opts),
        Some(Detected::Compressed {
            inner_archive: None,
            ..
        }) => Ok(FormatCreateBudget::direct(archive_bytes)),
        None => Err(FormatError::Unsupported(format!(
            "creating this format is not supported: {detect_name}"
        ))),
    }
}

pub(crate) fn ensure_create_space(dest: &Path, plan: &CreatePlan) -> Result<(), FormatError> {
    let parent = dest
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    ensure_available_space(parent, plan.workspace_budget_bytes)?;
    if plan.system_temp_budget_bytes > 0 {
        ensure_available_space(&std::env::temp_dir(), plan.system_temp_budget_bytes)?;
    }
    Ok(())
}

fn ensure_available_space(path: &Path, required_bytes: u64) -> Result<(), FormatError> {
    if fs4::available_space(path)? < required_bytes {
        return Err(FormatError::DiskFull);
    }
    Ok(())
}

fn normalize_create_plan_space(dest: &Path, plan: &mut CreatePlan) {
    if plan.system_temp_budget_bytes == 0 {
        return;
    }
    let parent = dest
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let (workspace_bytes, system_temp_bytes) = normalized_create_space_budgets(
        plan.workspace_budget_bytes,
        plan.system_temp_budget_bytes,
        filesystem_relation(parent, &std::env::temp_dir()),
    );
    plan.workspace_budget_bytes = workspace_bytes;
    plan.system_temp_budget_bytes = system_temp_bytes;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FilesystemRelation {
    Same,
    Different,
    Unknown,
}

fn normalized_create_space_budgets(
    workspace_bytes: u64,
    system_temp_bytes: u64,
    relation: FilesystemRelation,
) -> (u64, u64) {
    match relation {
        FilesystemRelation::Same => (workspace_bytes.saturating_add(system_temp_bytes), 0),
        FilesystemRelation::Different => (workspace_bytes, system_temp_bytes),
        FilesystemRelation::Unknown => (
            workspace_bytes.saturating_add(system_temp_bytes),
            system_temp_bytes,
        ),
    }
}

fn filesystem_relation(left: &Path, right: &Path) -> FilesystemRelation {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        let (Ok(left), Ok(right)) = (std::fs::metadata(left), std::fs::metadata(right)) else {
            return FilesystemRelation::Unknown;
        };
        if left.dev() == right.dev() {
            FilesystemRelation::Same
        } else {
            FilesystemRelation::Different
        }
    }

    #[cfg(windows)]
    {
        match (
            windows_volume_identity(left),
            windows_volume_identity(right),
        ) {
            (Some(left), Some(right)) if left == right => FilesystemRelation::Same,
            (Some(_), Some(_)) => FilesystemRelation::Different,
            _ => FilesystemRelation::Unknown,
        }
    }

    #[cfg(not(any(unix, windows)))]
    {
        let _ = (left, right);
        FilesystemRelation::Unknown
    }
}

#[cfg(windows)]
fn windows_volume_identity(path: &Path) -> Option<u64> {
    let handle = winapi_util::Handle::from_path_any(path).ok()?;
    let serial = winapi_util::file::information(&handle)
        .ok()?
        .volume_serial_number();
    (serial != 0).then_some(serial)
}

struct CreateOutputExclusions {
    exact_paths: Vec<PathBuf>,
    reserved_directories: Vec<PathBuf>,
    split_base: Option<PathBuf>,
    sqz_recovery: bool,
}

impl CreateOutputExclusions {
    fn new(
        final_dest: &Path,
        staging: &Path,
        inner_temp: &Path,
        split_output: bool,
        reserved_outputs: &[&Path],
    ) -> Result<Self, FormatError> {
        let base = split_output
            .then(|| split_output_base(final_dest))
            .transpose()?;
        let exact_paths: Vec<PathBuf> = [final_dest, staging, inner_temp]
            .into_iter()
            .chain(base.as_deref())
            .chain(reserved_outputs.iter().copied())
            .map(Path::to_path_buf)
            .collect();
        let reserved_directories = reserved_outputs
            .iter()
            .copied()
            .filter(|path| {
                std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_dir())
            })
            .map(Path::to_path_buf)
            .collect();
        let sqz_recovery = base.as_deref().is_some_and(|base| {
            base.extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("sqz"))
        });
        Ok(Self {
            exact_paths,
            reserved_directories,
            split_base: base,
            sqz_recovery,
        })
    }

    fn for_estimate(final_dest: &Path, split_output: bool) -> Result<Self, FormatError> {
        let base = split_output
            .then(|| split_output_base(final_dest))
            .transpose()?;
        if let Some(base) = &base {
            volumes::validate_split_output_base(base)?;
        }
        let exact_paths: Vec<PathBuf> = [final_dest]
            .into_iter()
            .chain(base.as_deref())
            .map(Path::to_path_buf)
            .collect();
        let reserved_directories = std::fs::symlink_metadata(final_dest)
            .ok()
            .filter(|metadata| metadata.file_type().is_dir())
            .map(|_| vec![final_dest.to_path_buf()])
            .unwrap_or_default();
        let sqz_recovery = base.as_deref().is_some_and(|base| {
            base.extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("sqz"))
        });
        Ok(Self {
            exact_paths,
            reserved_directories,
            split_base: base,
            sqz_recovery,
        })
    }

    fn for_unsplit_estimate(final_dest: &Path, reserved_outputs: &[&Path]) -> Self {
        let exact_paths = std::iter::once(final_dest)
            .chain(reserved_outputs.iter().copied())
            .map(Path::to_path_buf)
            .collect();
        let reserved_directories = std::iter::once(final_dest)
            .chain(reserved_outputs.iter().copied())
            .filter(|path| {
                std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_dir())
            })
            .map(Path::to_path_buf)
            .collect();
        Self {
            exact_paths,
            reserved_directories,
            split_base: None,
            sqz_recovery: false,
        }
    }

    fn reject_explicit_inputs(&self, inputs: &[PathBuf]) -> Result<(), FormatError> {
        for input in inputs {
            if self.matches(input)?
                || self
                    .reserved_directories
                    .iter()
                    .any(|directory| path_is_within_directory(input, directory))
            {
                return Err(FormatError::Unsupported(format!(
                    "archive output cannot also be an input: {}",
                    input.display()
                )));
            }
        }
        Ok(())
    }

    fn matches(&self, path: &Path) -> Result<bool, FormatError> {
        if self
            .exact_paths
            .iter()
            .any(|candidate| crate::same_path_entry(candidate, path))
        {
            return Ok(true);
        }
        if crate::sfx::classify_sfx_transaction_artifact(path)? {
            return Ok(true);
        }
        Ok(self.split_base.as_deref().is_some_and(|base| {
            matches_split_volume(base, path)
                || (self.sqz_recovery && matches_sqz_recovery(base, path))
                || volumes::matches_split_staging_path(base, path, self.sqz_recovery)
                || volumes::matches_split_complete_staging_path(base, path)
                || volumes::matches_split_transaction_path(base, path, self.sqz_recovery)
                || volumes::matches_split_transaction_journal(base, path)
        }))
    }
}

pub(crate) fn plan_create_with_progress(
    engine: &Engine,
    dest: &Path,
    inputs: &[PathBuf],
    opts: &CreateOptions,
    mut progress: impl FnMut(usize, &str),
) -> Result<CreatePlan, FormatError> {
    let name = dest
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| FormatError::Unsupported("invalid output file name".into()))?;
    let detect_name = if opts.split_size.is_some() {
        split_volume_name(name)
            .map(|(base, _index)| base)
            .unwrap_or(name)
    } else {
        name
    };
    validate_create_target_name(engine, detect_name, opts)?;
    let output_exclusions = CreateOutputExclusions::for_estimate(dest, opts.split_size.is_some())?;
    output_exclusions.reject_explicit_inputs(inputs)?;
    let prepared = prepare_create_inputs(
        engine,
        detect_name,
        inputs,
        opts,
        &output_exclusions,
        |count, path| progress(count, &path.display),
    )?;
    create_plan_from_summary(engine, detect_name, dest, prepared.summary, opts)
}

pub(crate) fn prepare_sfx_input_summary_with_progress(
    engine: &Engine,
    inputs: &[PathBuf],
    dest: &Path,
    opts: &CreateOptions,
    mut progress: impl FnMut(usize, &str),
) -> Result<CreateInputSummary, FormatError> {
    let output_exclusions = CreateOutputExclusions::for_unsplit_estimate(dest, &[]);
    output_exclusions.reject_explicit_inputs(inputs)?;
    let prepared = prepare_create_inputs(
        engine,
        "squallz-sfx-payload.zip",
        inputs,
        opts,
        &output_exclusions,
        |count, path| progress(count, &path.display),
    )?;
    Ok(prepared.summary)
}

pub(crate) fn collect_inputs_for_output_estimate(
    inputs: &[PathBuf],
    excludes: &PathFilter,
    final_dest: &Path,
    split_output: bool,
    mut progress: impl FnMut(usize, &EntryPath),
) -> Result<Vec<InputItem>, FormatError> {
    let output_exclusions = CreateOutputExclusions::for_estimate(final_dest, split_output)?;
    output_exclusions.reject_explicit_inputs(inputs)?;
    collect_with_output_exclusions(inputs, excludes, &output_exclusions, &mut progress)
}

fn collect_with_output_exclusions(
    inputs: &[PathBuf],
    excludes: &PathFilter,
    output_exclusions: &CreateOutputExclusions,
    progress: &mut impl FnMut(usize, &EntryPath),
) -> Result<Vec<InputItem>, FormatError> {
    let exclusion_error = RefCell::new(None);
    let items = collect_inputs_excluding_with_progress(
        inputs,
        excludes,
        |path| match output_exclusions.matches(path) {
            Ok(excluded) => excluded,
            Err(error) => {
                let mut first = exclusion_error.borrow_mut();
                if first.is_none() {
                    *first = Some(error);
                }
                true
            }
        },
        progress,
    );
    match (exclusion_error.into_inner(), items) {
        (Some(error), _) => Err(error),
        (None, items) => items,
    }
}

fn collect_prepared_with_output_exclusions(
    inputs: &[PathBuf],
    excludes: &PathFilter,
    output_exclusions: &CreateOutputExclusions,
    progress: &mut impl FnMut(usize, &EntryPath),
) -> Result<Vec<PreparedInputItem>, FormatError> {
    let exclusion_error = RefCell::new(None);
    let items = collect_prepared_inputs_excluding_with_progress(
        inputs,
        excludes,
        |path| match output_exclusions.matches(path) {
            Ok(excluded) => excluded,
            Err(error) => {
                let mut first = exclusion_error.borrow_mut();
                if first.is_none() {
                    *first = Some(error);
                }
                true
            }
        },
        progress,
    );
    match (exclusion_error.into_inner(), items) {
        (Some(error), _) => Err(error),
        (None, items) => items,
    }
}

fn matches_split_volume(base: &Path, path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let (volume_name, part) = match name.rsplit_once('.') {
        Some((stem, suffix)) if suffix.eq_ignore_ascii_case("part") => (stem, true),
        _ => (name, false),
    };
    if split_volume_name(volume_name).is_none() {
        return false;
    }
    let Some(suffix) = volume_name.rsplit_once('.').map(|(_, suffix)| suffix) else {
        return false;
    };
    let Some(base_name) = base.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let part = if part { ".part" } else { "" };
    crate::same_path_entry(
        &base.with_file_name(format!("{base_name}.{suffix}{part}")),
        path,
    )
}

fn matches_sqz_recovery(base: &Path, path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let (recovery_name, part) = match name.rsplit_once('.') {
        Some((stem, suffix)) if suffix.eq_ignore_ascii_case("part") => (stem, true),
        _ => (name, false),
    };
    let Some((suffix, _index)) = volumes::sqz_recovery_suffix(recovery_name) else {
        return false;
    };
    let Some(index) = suffix.get(4..) else {
        return false;
    };
    let Some(base_name) = base.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let part = if part { ".part" } else { "" };
    crate::same_path_entry(
        &base.with_file_name(format!("{base_name}.rev{index}{part}")),
        path,
    )
}

fn path_is_within_directory(path: &Path, directory: &Path) -> bool {
    let Some(parent) = path.parent() else {
        return false;
    };
    matches!(
        (std::fs::canonicalize(parent), std::fs::canonicalize(directory)),
        (Ok(parent), Ok(directory)) if parent.starts_with(&directory)
    )
}

fn split_output_base(dest: &Path) -> Result<PathBuf, FormatError> {
    let name = dest
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| FormatError::Unsupported("invalid output file name".into()))?;
    Ok(split_volume_name(name)
        .map(|(base, _index)| dest.with_file_name(base))
        .unwrap_or_else(|| dest.to_path_buf()))
}

/// Streams the collected input items into an archive writer, with
/// byte-granular progress on file contents and chunk-boundary cancellation.
#[derive(Default)]
struct CapturedCreateInputs {
    fingerprints: Vec<CreateInputFingerprint>,
    manifest: Vec<CreateInputManifestEntry>,
}

fn write_entries(
    sink: &mut DestSink,
    items: &[PreparedInputItem],
    opts: &CreateOptions,
    progress: &dyn ProgressSink,
    ctl: &ControlToken,
    capture_input_manifest: bool,
) -> Result<CapturedCreateInputs, FormatError> {
    let total: u64 = items.iter().map(|item| item.item().size).sum();
    let mut done = 0u64;
    let mut captured = CapturedCreateInputs::default();
    for prepared in items {
        let item = prepared.item();
        ctl.checkpoint()?;
        progress.on_entry_progress(done, total, &item.name, 0, item.size);
        let meta = EntryMeta {
            path: item.name.clone(),
            entry_type: item.entry_type.clone(),
            size: item.size,
            compressed_size: None,
            modified: item.modified,
            unix_mode: item.unix_mode,
            crc32: None,
            encrypted: opts.password.is_some(),
        };
        let source_path = capture_input_manifest.then(|| prepared.source_path().to_path_buf());
        match item.entry_type {
            EntryType::File => {
                let mut file = prepared.open_file()?;
                let data =
                    ProgressRead::new(&mut file, progress, ctl, &item.name, done, total, item.size);
                let mut data = TrackedInputRead::new(data, capture_input_manifest);
                add_entry_data(sink, &meta, &mut data, ctl)?;
                let fingerprint = data.finish(source_path, item.size)?;
                prepared.validate_after_read(&file)?;
                if let Some(fingerprint) = fingerprint {
                    captured.manifest.push(create_manifest_entry(
                        fingerprint.path.clone(),
                        &meta,
                        Some(fingerprint.blake3),
                    ));
                    captured.fingerprints.push(fingerprint);
                }
            }
            _ => {
                prepared.validate_non_file()?;
                sink.add_entry(&meta, None)?;
                prepared.validate_non_file()?;
                if let Some(source_path) = source_path {
                    captured
                        .manifest
                        .push(create_manifest_entry(source_path, &meta, None));
                }
            }
        }
        done += item.size;
    }
    progress.on_progress(total, total, &EntryPath::from_utf8(""));
    Ok(captured)
}

fn create_manifest_entry(
    source_path: PathBuf,
    meta: &EntryMeta,
    blake3: Option<[u8; 32]>,
) -> CreateInputManifestEntry {
    CreateInputManifestEntry {
        source_path,
        archive_path: meta.path.clone(),
        entry_type: meta.entry_type.clone(),
        size: meta.size,
        modified: meta.modified.map(CreateInputModifiedTime::from),
        unix_mode: meta.unix_mode,
        blake3,
    }
}

fn add_entry_data(
    sink: &mut DestSink,
    meta: &EntryMeta,
    data: &mut dyn Read,
    ctl: &ControlToken,
) -> Result<(), FormatError> {
    // Cancellation inside the copy surfaces as an I/O error; restore the
    // precise variant here.
    sink.add_entry(meta, Some(data)).map_err(|error| {
        if ctl.is_cancelled() {
            FormatError::Cancelled
        } else {
            error
        }
    })
}

pub(crate) struct TrackedInputRead<R> {
    inner: R,
    hasher: Option<blake3::Hasher>,
    bytes: u64,
}

impl<R> TrackedInputRead<R> {
    pub(crate) fn new(inner: R, capture_fingerprint: bool) -> Self {
        Self {
            inner,
            hasher: capture_fingerprint.then(blake3::Hasher::new),
            bytes: 0,
        }
    }

    pub(crate) fn finish(
        self,
        path: Option<PathBuf>,
        expected_size: u64,
    ) -> Result<Option<CreateInputFingerprint>, FormatError> {
        if self.bytes != expected_size {
            return Err(FormatError::Io(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                format!(
                    "input changed while it was being archived: expected {expected_size} bytes, read {}",
                    self.bytes
                ),
            )));
        }
        let Some(hasher) = self.hasher else {
            return Ok(None);
        };
        let path = path
            .ok_or_else(|| FormatError::Other("verified create lost a source identity".into()))?;
        Ok(Some(CreateInputFingerprint {
            path,
            size: self.bytes,
            blake3: *hasher.finalize().as_bytes(),
        }))
    }
}

impl<R: Read> Read for TrackedInputRead<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let read = self.inner.read(buffer)?;
        if read > 0 {
            if let Some(hasher) = &mut self.hasher {
                hasher.update(&buffer[..read]);
            }
            self.bytes = self.bytes.saturating_add(read as u64);
        }
        Ok(read)
    }
}

/// Single-stream creation (`x.gz`): compresses exactly one input file.
fn create_single_stream(
    compressor: &Arc<dyn Compressor>,
    mut dst: File,
    prepared: &PreparedInputItem,
    opts: &CreateOptions,
    progress: &dyn ProgressSink,
    ctl: &ControlToken,
    capture_input_manifest: bool,
) -> Result<CapturedCreateInputs, FormatError> {
    let item = prepared.item();
    if !matches!(item.entry_type, EntryType::File) {
        return Err(FormatError::Other(
            "prepared single-stream input is not a regular file".into(),
        ));
    }
    let mut source = prepared.open_file()?;
    let entry_meta = EntryMeta {
        path: item.name.clone(),
        entry_type: EntryType::File,
        size: item.size,
        compressed_size: None,
        modified: item.modified,
        unix_mode: item.unix_mode,
        crc32: None,
        encrypted: false,
    };
    let sink = KnownTotal::new(progress, item.size, item.name.clone());
    let source_path = capture_input_manifest.then(|| prepared.source_path().to_path_buf());
    let mut src = TrackedInputRead::new(&mut source, capture_input_manifest);
    compressor.compress(&mut src, &mut dst, opts.level, &opts.resources, &sink, ctl)?;
    let fingerprint = src.finish(source_path, item.size)?;
    prepared.validate_after_read(&source)?;
    let captured = if let Some(fingerprint) = fingerprint {
        CapturedCreateInputs {
            manifest: vec![create_manifest_entry(
                fingerprint.path.clone(),
                &entry_meta,
                Some(fingerprint.blake3),
            )],
            fingerprints: vec![fingerprint],
        }
    } else {
        CapturedCreateInputs::default()
    };
    progress.on_progress(item.size, item.size, &EntryPath::from_utf8(""));
    Ok(captured)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_plan_with_temp_budget() -> CreatePlan {
        CreatePlan {
            inputs: CreateInputEstimate::default(),
            primary_output: PathBuf::from("archive.sqz"),
            archive_output_budget_bytes: 100,
            final_output_budget_bytes: 100,
            split_volume_count_budget: None,
            workspace_budget_bytes: 150,
            system_temp_budget_bytes: 75,
        }
    }

    #[test]
    fn create_plan_folds_system_temp_into_a_shared_destination_filesystem() {
        let mut plan = create_plan_with_temp_budget();
        let destination = std::env::temp_dir().join("squallz-plan-space-test.sqz");

        normalize_create_plan_space(&destination, &mut plan);

        assert_eq!(plan.workspace_budget_bytes, 225);
        assert_eq!(plan.system_temp_budget_bytes, 0);
    }

    #[test]
    fn same_filesystem_budget_is_folded_once() {
        assert_eq!(
            normalized_create_space_budgets(150, 75, FilesystemRelation::Same),
            (225, 0)
        );
    }

    #[test]
    fn different_filesystem_budget_keeps_independent_gates() {
        assert_eq!(
            normalized_create_space_budgets(150, 75, FilesystemRelation::Different),
            (150, 75)
        );
    }

    #[test]
    fn unknown_filesystem_budget_gates_both_possible_topologies() {
        assert_eq!(
            normalized_create_space_budgets(150, 75, FilesystemRelation::Unknown),
            (225, 75)
        );
    }

    #[test]
    fn temporary_output_reservation_preserves_an_existing_candidate() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let seed = std::env::temp_dir().join(format!(
            "squallz-create-reservation-{}-{}.zip",
            std::process::id(),
            nonce
        ));
        let first_reserved = crate::reserve_bound_sibling_temp_file(&seed, "split").unwrap();
        let first = first_reserved.path;
        drop(first_reserved.file);
        std::fs::write(&first, b"existing candidate").unwrap();

        let second_reserved = crate::reserve_bound_sibling_temp_file(&seed, "split").unwrap();
        let second = second_reserved.path;
        drop(second_reserved.file);

        assert_ne!(first, second);
        assert_eq!(std::fs::read(&first).unwrap(), b"existing candidate");
        assert!(second.is_file());
        std::fs::remove_file(first).unwrap();
        std::fs::remove_file(second).unwrap();
    }
}
