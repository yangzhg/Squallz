//! Owns an archive's staging file from writer admission through publication.
//! Writers return content results; this owner measures, publishes and cleans up.

use crate::compound::SharedCompressSink;
use std::fs::File;
use std::path::Path;
use std::sync::Arc;

use crate::api::{
    split_volume_name, ArchiveFormat, ArchiveWriter, Compressor, ControlToken, CreateOptions,
    Detected, EntryMeta, FormatError, ProgressSink, SplitOutputMode,
};
use crate::{CreateArtifactKind, CreateCommitPolicy, CreateReport, Engine, ReservedTempFile};

pub(crate) enum ArchiveOutput<'a> {
    Published {
        destination: &'a Path,
        policy: CreateCommitPolicy,
    },
    CallerReserved {
        detect_name: &'a str,
        reserved: ReservedTempFile,
    },
}

pub(crate) struct ArchiveWriterTarget<'a> {
    detected: Detected,
    pub(crate) detect_name: &'a str,
    pub(crate) staging_path: &'a Path,
    pub(crate) file: File,
    pub(crate) options: &'a CreateOptions,
}

impl ArchiveOutput<'_> {
    pub(crate) fn path(&self) -> &Path {
        match self {
            Self::Published { destination, .. } => destination,
            Self::CallerReserved { reserved, .. } => &reserved.path,
        }
    }

    pub(crate) fn detect_name(&self) -> Result<&str, FormatError> {
        match self {
            Self::Published { destination, .. } => destination
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| FormatError::Unsupported("invalid output file name".into())),
            Self::CallerReserved { detect_name, .. } => Ok(detect_name),
        }
    }

    pub(crate) fn write<T>(
        self,
        engine: &Engine,
        options: &CreateOptions,
        progress: &dyn ProgressSink,
        ctl: &ControlToken,
        write: impl FnOnce(ArchiveWriterTarget<'_>) -> Result<T, FormatError>,
    ) -> Result<(T, CreateReport), FormatError> {
        if matches!(self, Self::CallerReserved { .. }) && options.split_size.is_some() {
            return Err(FormatError::Unsupported(
                "a caller-owned output reservation cannot be split".into(),
            ));
        }
        let name = self.detect_name()?;
        let name = if options.split_size.is_some() {
            split_volume_name(name).map_or(name, |(base, _)| base)
        } else {
            name
        };
        let detected = validate_create_target_name(engine, name, options)?;
        let name = name.to_owned();
        let output = self.path().to_owned();
        let base = if options.split_size.is_some() {
            output.with_file_name(&name)
        } else {
            output.clone()
        };
        if let Some(size) = options.split_size {
            if size < crate::volumes::MIN_SPLIT_SIZE {
                return Err(FormatError::Unsupported(format!(
                    "split size below the {}-byte minimum: {size}",
                    crate::volumes::MIN_SPLIT_SIZE
                )));
            }
            crate::volumes::validate_split_output_base(&base)?;
        }
        let (reserved, publication) = match self {
            Self::Published { policy, .. } => {
                let policy = if matches!(policy, CreateCommitPolicy::ReplaceExisting) {
                    bind_commit_policy(&output, options, policy)?
                } else {
                    policy
                };
                let purpose = if options.split_size.is_some() {
                    "split"
                } else {
                    "create"
                };
                (
                    crate::reserve_bound_sibling_temp_file(&base, purpose)?,
                    Some(policy),
                )
            }
            Self::CallerReserved { reserved, .. } => (reserved, None),
        };
        let write_options = CreateOptions {
            split_size: None,
            split_mode: SplitOutputMode::Generic,
            ..options.clone()
        };
        let context = "archive output staging";
        let file = reserved.clone_for_writer(context)?;
        let result = write(ArchiveWriterTarget {
            detected,
            detect_name: &name,
            staging_path: &reserved.path,
            file,
            options: &write_options,
        })
        .and_then(|value| {
            ctl.checkpoint()?;
            let bytes = reserved.written_size(|| {
                std::io::Error::other("archive output staging changed after writing")
            })?;
            Ok((value, bytes))
        });
        let (value, bytes) = result.map_err(|error| reserved.cleanup_error(error, context))?;
        let Some(policy) = publication else {
            return Ok((value, single_output_report(&output, bytes)));
        };
        let Some(size) = options.split_size else {
            match policy {
                CreateCommitPolicy::NoReplace => crate::publish_bound_file_no_replace(
                    &reserved.path,
                    &reserved.file,
                    reserved.identity,
                    &output,
                )
                .map_err(|error| reserved.cleanup_error(error, context))?,
                CreateCommitPolicy::ReplaceIfUnchanged(guard) => {
                    crate::update::commit_created_archive(&output, reserved, guard, progress, ctl)?;
                }
                CreateCommitPolicy::ReplaceExisting => {
                    return Err(reserved.cleanup_error(
                        FormatError::Other(
                            "archive replacement policy was not bound before writing".into(),
                        ),
                        context,
                    ));
                }
            }
            return Ok((value, single_output_report(&output, bytes)));
        };
        let split = (|| match options.split_mode {
            SplitOutputMode::Generic => {
                crate::volumes::split_into_volumes_with_commit_policy_and_source_identity(
                    &reserved.path,
                    reserved.file.try_clone()?,
                    reserved.identity,
                    &base,
                    size,
                    &options.resources,
                    progress,
                    ctl,
                    policy,
                )
            }
            SplitOutputMode::Native => {
                let Some(Detected::Archive(format)) = engine.registry().detect_by_name(&name)
                else {
                    return Err(FormatError::Unsupported(
                        "native volume creation requires a directly supported archive format"
                            .into(),
                    ));
                };
                crate::volumes::split_into_native_volumes_with_commit_policy_and_source_identity(
                    &reserved.path,
                    reserved.file.try_clone()?,
                    reserved.identity,
                    &base,
                    size,
                    &*format,
                    &options.resources,
                    progress,
                    ctl,
                    policy,
                )
            }
        })()
        .map_err(|error| reserved.cleanup_error(error, context))?;
        let primary_output = split
            .volumes
            .get(split.primary_volume_index)
            .cloned()
            .ok_or_else(|| {
                FormatError::Other("split creation returned no primary volume".into())
            })?;
        let split_volume_count = Some(split.volumes.len());
        let mut outputs = split.volumes;
        outputs.extend(split.sidecars);
        Ok((
            value,
            CreateReport {
                primary_output,
                outputs,
                preserved_outputs: split.preserved_outputs,
                total_output_bytes: split.total_output_bytes,
                split_volume_count,
            },
        ))
    }
}

/// Conversion binds before opening its source; creation binds before scanning.
pub(crate) fn bind_commit_policy(
    destination: &Path,
    options: &CreateOptions,
    policy: CreateCommitPolicy,
) -> Result<CreateCommitPolicy, FormatError> {
    let kind = if options.split_size.is_some() {
        CreateArtifactKind::SplitArchive
    } else {
        CreateArtifactKind::Archive
    };
    match policy {
        CreateCommitPolicy::NoReplace => {
            if crate::create_destination_has_conflict(destination, kind)? {
                return Err(crate::output_exists_error(destination));
            }
            Ok(policy)
        }
        CreateCommitPolicy::ReplaceIfUnchanged(_) => Ok(policy),
        CreateCommitPolicy::ReplaceExisting => {
            let inspection = crate::inspect_create_destination(destination, kind)?;
            match (inspection.conflict, inspection.guard) {
                (false, None) => Ok(CreateCommitPolicy::NoReplace),
                (true, Some(guard)) => Ok(CreateCommitPolicy::ReplaceIfUnchanged(guard)),
                _ => Err(FormatError::Other(
                    "archive destination inspection returned an inconsistent result".into(),
                )),
            }
        }
    }
}

fn single_output_report(path: &Path, bytes: u64) -> CreateReport {
    CreateReport {
        primary_output: path.to_owned(),
        outputs: vec![path.to_owned()],
        preserved_outputs: Vec::new(),
        total_output_bytes: bytes,
        split_volume_count: None,
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

impl ArchiveWriterTarget<'_> {
    pub(crate) fn open(self, ctl: &ControlToken) -> Result<DestTarget, FormatError> {
        match self.detected {
            Detected::Archive(f) => {
                let writer = f.create(Box::new(self.file), self.options, ctl)?;
                Ok(DestTarget::Archive(DestSink {
                    writer,
                    shared: None,
                }))
            }
            Detected::Compressed {
                compressor,
                inner_archive: Some(archive),
            } => {
                let sink = compressor.compress_writer(
                    Box::new(self.file),
                    self.options.level,
                    &self.options.resources,
                )?;
                let shared = SharedCompressSink::new(sink);
                ctl.checkpoint()?;
                let writer = archive.create_stream(Box::new(shared.clone()), self.options)?;
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
                output: self.file,
            }),
        }
    }
}

pub(crate) fn validate_create_target_name(
    engine: &Engine,
    detect_name: &str,
    opts: &CreateOptions,
) -> Result<Detected, FormatError> {
    let detected = engine
        .registry()
        .detect_by_name(detect_name)
        .ok_or_else(|| {
            FormatError::Unsupported(format!(
                "creating this format is not supported: {detect_name}"
            ))
        })?;
    validate_detected_create_target(&detected, detect_name, opts)?;
    Ok(detected)
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
pub(crate) fn check_can_create(
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
