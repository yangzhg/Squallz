//! File-system input planning and streaming for archive creation.
//! Output admission, writer selection and publication belong to archive_output.

use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::api::{
    split_volume_name, Compressor, ControlToken, CreateOptions, Detected, EntryMeta, EntryPath,
    EntryType, FormatCreateBudget, FormatError, ProgressSink, SplitOutputMode,
};
use crate::archive_output::{
    check_can_create, validate_create_target_name, ArchiveOutput, ArchiveWriterTarget, DestSink,
    DestTarget,
};
use crate::compound::{KnownTotal, ProgressRead};
use crate::inputs::{
    collect_inputs_excluding_with_progress, collect_prepared_inputs_excluding_with_progress,
    deduplicate_prepared_input_roots, prepare_single_stream_input, InputItem, PreparedInputItem,
};
use crate::volumes::{self, MIN_SPLIT_SIZE};
use crate::{
    CreateCommitPolicy, CreateInputEstimate, CreateInputManifestEntry, CreateInputModifiedTime,
    CreateInputSummary, CreatePlan, Engine, PathFilter, VerifiedCreateReport,
};

/// Publishes filesystem inputs with optional source-content verification.
/// Both public creation contracts use the same output admission and writer.
#[allow(clippy::too_many_arguments)] // published creation: capture mode is private to the engine facade
pub(crate) fn create_published(
    engine: &Engine,
    dest: &Path,
    inputs: &[PathBuf],
    opts: &CreateOptions,
    commit_policy: CreateCommitPolicy,
    progress: &dyn ProgressSink,
    ctl: &ControlToken,
    capture_input_manifest: bool,
) -> Result<VerifiedCreateReport, FormatError> {
    create_with_output(
        engine,
        ArchiveOutput::Published {
            destination: dest,
            policy: commit_policy,
        },
        inputs,
        &[],
        opts,
        progress,
        ctl,
        None,
        capture_input_manifest,
    )
}

/// Prepares one unsplit archive input manifest for a surrounding operation.
/// The output is borrowed here and consumed by [`create_prepared_with_output`]
/// so planning and writing use the same format and accepted entry set.
pub(crate) fn prepare_unsplit_create_with_reserved_outputs(
    engine: &Engine,
    output: &ArchiveOutput<'_>,
    inputs: &[PathBuf],
    reserved_outputs: &[&Path],
    opts: &CreateOptions,
    progress: impl FnMut(usize, &EntryPath),
    ctl: &ControlToken,
) -> Result<PreparedCreateInputs, FormatError> {
    if opts.split_size.is_some() {
        return Err(FormatError::Unsupported(
            "prepared creation requires one complete archive output".into(),
        ));
    }
    let detect_name = output.detect_name()?;
    let output_exclusions =
        CreateOutputExclusions::for_unsplit_estimate(output.path(), reserved_outputs);
    output_exclusions.reject_explicit_inputs(inputs, ctl)?;
    prepare_create_inputs(
        engine,
        detect_name,
        inputs,
        opts,
        &output_exclusions,
        progress,
        ctl,
    )
}

/// Writes a prepared manifest without walking its input roots again.
/// Caller-reserved outputs stay at their bound path for the surrounding
/// operation to consume; the caller retains its own handle until then.
#[allow(clippy::too_many_arguments)] // prepared create boundary: each argument has a distinct role
pub(crate) fn create_prepared_with_output(
    engine: &Engine,
    output: ArchiveOutput<'_>,
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
    create_with_output(
        engine,
        output,
        inputs,
        reserved_outputs,
        opts,
        progress,
        ctl,
        Some(prepared),
        capture_input_manifest,
    )
}

#[allow(clippy::too_many_arguments)] // internal create plumbing; each argument has a distinct role
fn create_with_output(
    engine: &Engine,
    output: ArchiveOutput<'_>,
    inputs: &[PathBuf],
    reserved_outputs: &[&Path],
    opts: &CreateOptions,
    progress: &dyn ProgressSink,
    ctl: &ControlToken,
    prepared: Option<PreparedCreateInputs>,
    capture_input_manifest: bool,
) -> Result<VerifiedCreateReport, FormatError> {
    let final_path = output.path().to_path_buf();
    let (manifest, create) = output.write(engine, opts, progress, ctl, |writer| {
        create_unsplit(
            engine,
            CreateTarget {
                final_path: &final_path,
                writer,
                reserved_outputs,
                capture_input_manifest,
            },
            inputs,
            prepared,
            opts,
            progress,
            ctl,
        )
    })?;
    Ok(VerifiedCreateReport { create, manifest })
}

struct CreateTarget<'a> {
    final_path: &'a Path,
    writer: ArchiveWriterTarget<'a>,
    reserved_outputs: &'a [&'a Path],
    capture_input_manifest: bool,
}

/// Single-output creation from file-system inputs.
#[allow(clippy::too_many_arguments)] // internal create plumbing; each path/options role is distinct
fn create_unsplit(
    engine: &Engine,
    target: CreateTarget<'_>,
    inputs: &[PathBuf],
    prepared: Option<PreparedCreateInputs>,
    plan_opts: &CreateOptions,
    progress: &dyn ProgressSink,
    ctl: &ControlToken,
) -> Result<Vec<CreateInputManifestEntry>, FormatError> {
    let detect_name = target.writer.detect_name;
    let staging_path = target.writer.staging_path;
    let write_opts = target.writer.options;
    let output_exclusions = CreateOutputExclusions::new(
        target.final_path,
        staging_path,
        plan_opts.split_size.is_some(),
        target.reserved_outputs,
    )?;
    output_exclusions.reject_explicit_inputs(inputs, ctl)?;
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
            |count, path| progress.on_scan_progress(crate::usize_to_u64(count), path),
            ctl,
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
    let captured_inputs = match (target.writer.open(ctl)?, prepared.items) {
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
        (DestTarget::SingleStream { compressor, output }, PreparedInputs::SingleStream(item)) => {
            create_single_stream(
                &compressor,
                output,
                &item,
                write_opts,
                progress,
                ctl,
                target.capture_input_manifest,
            )?
        }
        _ => {
            return Err(FormatError::Other(
                "create destination changed while preparing inputs".into(),
            ));
        }
    };
    Ok(captured_inputs.manifest)
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
    ctl: &ControlToken,
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
                ctl,
            )?;
            let summary = crate::summarize_create_input_manifest(inputs.len(), &items, ctl)?;
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
                ctl,
            )?;
            let summary = crate::summarize_create_input_manifest(inputs.len(), &items, ctl)?;
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
                    ctl.checkpoint()?;
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
                ctl.checkpoint()?;
                let label = EntryPath::from_utf8(
                    input
                        .file_name()
                        .map_or_else(String::new, |name| name.to_string_lossy().into_owned()),
                );
                let item =
                    prepare_single_stream_input(input, label.clone(), ctl).map_err(|error| {
                        match error {
                            FormatError::Unsupported(_) => FormatError::Unsupported(format!(
                                "format {} compresses a single regular file",
                                compressor.id()
                            )),
                            error => error,
                        }
                    })?;
                progress(index.saturating_add(1), &label);
                ctl.checkpoint()?;
                prepared_items.push(item);
            }
            let mut items = deduplicate_prepared_input_roots(prepared_items, ctl)?;
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
        split_output: bool,
        reserved_outputs: &[&Path],
    ) -> Result<Self, FormatError> {
        let base = split_output
            .then(|| split_output_base(final_dest))
            .transpose()?;
        let exact_paths: Vec<PathBuf> = [final_dest, staging]
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

    fn reject_explicit_inputs(
        &self,
        inputs: &[PathBuf],
        ctl: &ControlToken,
    ) -> Result<(), FormatError> {
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
            ctl.checkpoint()?;
        }
        ctl.checkpoint()?;
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

pub(crate) fn plan_create(
    engine: &Engine,
    dest: &Path,
    inputs: &[PathBuf],
    opts: &CreateOptions,
    mut progress: impl FnMut(usize, &str),
    ctl: &ControlToken,
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
    output_exclusions.reject_explicit_inputs(inputs, ctl)?;
    let prepared = prepare_create_inputs(
        engine,
        detect_name,
        inputs,
        opts,
        &output_exclusions,
        |count, path| progress(count, &path.display),
        ctl,
    )?;
    let plan = create_plan_from_summary(engine, detect_name, dest, prepared.summary, opts)?;
    ctl.checkpoint()?;
    Ok(plan)
}

pub(crate) fn prepare_sfx_input_summary_with_progress(
    engine: &Engine,
    inputs: &[PathBuf],
    dest: &Path,
    opts: &CreateOptions,
    mut progress: impl FnMut(usize, &str),
    ctl: &ControlToken,
) -> Result<CreateInputSummary, FormatError> {
    let output_exclusions = CreateOutputExclusions::for_unsplit_estimate(dest, &[]);
    output_exclusions.reject_explicit_inputs(inputs, ctl)?;
    let prepared = prepare_create_inputs(
        engine,
        "squallz-sfx-payload.zip",
        inputs,
        opts,
        &output_exclusions,
        |count, path| progress(count, &path.display),
        ctl,
    )?;
    Ok(prepared.summary)
}

pub(crate) fn collect_inputs_for_output_estimate(
    inputs: &[PathBuf],
    excludes: &PathFilter,
    final_dest: &Path,
    split_output: bool,
    mut progress: impl FnMut(usize, &EntryPath),
    ctl: &ControlToken,
) -> Result<Vec<InputItem>, FormatError> {
    let output_exclusions = CreateOutputExclusions::for_estimate(final_dest, split_output)?;
    output_exclusions.reject_explicit_inputs(inputs, ctl)?;
    collect_with_output_exclusions(inputs, excludes, &output_exclusions, &mut progress, ctl)
}

fn collect_with_output_exclusions(
    inputs: &[PathBuf],
    excludes: &PathFilter,
    output_exclusions: &CreateOutputExclusions,
    progress: &mut impl FnMut(usize, &EntryPath),
    ctl: &ControlToken,
) -> Result<Vec<InputItem>, FormatError> {
    collect_inputs_excluding_with_progress(
        inputs,
        excludes,
        |path| output_exclusions.matches(path),
        progress,
        ctl,
    )
}

fn collect_prepared_with_output_exclusions(
    inputs: &[PathBuf],
    excludes: &PathFilter,
    output_exclusions: &CreateOutputExclusions,
    progress: &mut impl FnMut(usize, &EntryPath),
    ctl: &ControlToken,
) -> Result<Vec<PreparedInputItem>, FormatError> {
    collect_prepared_inputs_excluding_with_progress(
        inputs,
        excludes,
        |path| output_exclusions.matches(path),
        progress,
        ctl,
    )
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
                let blake3 = data.finish(item.size)?;
                prepared.validate_after_read(&file)?;
                if let Some(source_path) = source_path {
                    captured
                        .manifest
                        .push(create_manifest_entry(source_path, &meta, blake3));
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

    pub(crate) fn finish(self, expected_size: u64) -> Result<Option<[u8; 32]>, FormatError> {
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
        Ok(Some(*hasher.finalize().as_bytes()))
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
    let blake3 = src.finish(item.size)?;
    prepared.validate_after_read(&source)?;
    let captured = if let Some(source_path) = source_path {
        CapturedCreateInputs {
            manifest: vec![create_manifest_entry(source_path, &entry_meta, blake3)],
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
