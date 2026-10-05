//! Archive rewriting and guarded creation use the shared replacement owner.
use std::fs::{self, File, Permissions};
use std::io;
use std::path::{Path, PathBuf};

use crate::api::{
    ArchiveFormat, ControlToken, EntryPath, FormatError, PreparedUpdateAdditions, ProgressPhase,
    ProgressSink, UpdateOp, UpdateOptions,
};
use crate::destination_guard::{verify_destination_guard, verify_destination_guard_with_progress};
use crate::extract_guard::inspect_bound_archive_source_state;
use crate::filesystem_identity::{
    file_identity, open_regular_file_no_follow, path_identity, PathIdentity, RegularFileState,
};
use crate::replacement::{
    self, ActiveWriter, ArtifactRole, Authorization, Failure, Outcome, OwnedInput, PersistFailure,
    Recovery, Scope, VerificationContext, Visibility,
};
use crate::{
    lock_unpoisoned, ArchiveSourceState, ArchiveUpdateGuard, CreateArtifactKind,
    CreateDestinationGuard, ReservedTempFile,
};

struct Source {
    path: PathBuf,
    file: File,
    identity: PathIdentity,
    state: RegularFileState,
    permissions: Permissions,
}
impl Source {
    fn bind(path: PathBuf) -> Result<Self, FormatError> {
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(FormatError::Unsupported(
                "archive source is not a regular file".into(),
            ));
        }
        let file = open_regular_file_no_follow(&path)?;
        let identity = file_identity(&file)?;
        let source = Self {
            path,
            file,
            identity,
            state: RegularFileState::from_metadata(&metadata),
            permissions: metadata.permissions(),
        };
        source.verify()?;
        Ok(source)
    }
    fn verify(&self) -> Result<(), FormatError> {
        let metadata = fs::symlink_metadata(&self.path)?;
        if metadata.file_type().is_symlink()
            || !metadata.is_file()
            || file_identity(&self.file)? != self.identity
            || path_identity(&self.path)? != self.identity
            || !self.state.matches(&metadata)
            || !self.state.matches(&self.file.metadata()?)
        {
            return Err(FormatError::input_changed());
        }
        Ok(())
    }
    fn into_input(self) -> OwnedInput {
        OwnedInput::Archive {
            path: self.path,
            file: self.file,
            identity: self.identity,
            state: self.state,
        }
    }
}
#[derive(Clone, Copy)]
enum Purpose<'a> {
    Rewrite {
        guard: Option<&'a ArchiveUpdateGuard>,
    },
    GuardedCreate {
        guard: CreateDestinationGuard,
    },
}
struct Commit<'a> {
    requested: &'a Path,
    purpose: Purpose<'a>,
    progress: &'a dyn ProgressSink,
    control: &'a ControlToken,
}
impl Commit<'_> {
    fn phase(&self, update: ProgressPhase, output: ProgressPhase, cancellable: bool) {
        self.progress.on_phase(
            if matches!(self.purpose, Purpose::Rewrite { .. }) {
                update
            } else {
                output
            },
            cancellable,
        );
    }
    fn map_guard_error(&self, error: FormatError) -> FormatError {
        match self.purpose {
            Purpose::GuardedCreate { guard } => guarded_error(self.requested, guard, error),
            _ => error,
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn run(
    format: &dyn ArchiveFormat,
    requested: &Path,
    ops: &[UpdateOp],
    additions: &mut dyn PreparedUpdateAdditions,
    options: &UpdateOptions,
    source_guard: Option<&ArchiveUpdateGuard>,
    progress: &dyn ProgressSink,
    control: &ControlToken,
) -> Result<(), FormatError> {
    let commit = Commit {
        requested,
        purpose: Purpose::Rewrite {
            guard: source_guard,
        },
        progress,
        control,
    };
    control.checkpoint()?;
    let scope = recover_scope(Scope::archive(requested, control)?, &commit)?;
    scope.require_clear()?;
    control.checkpoint()?;
    let source = Source::bind(scope.target_path())?;
    if let Some(guard) = source_guard {
        if inspect_bound_archive_source_state(requested, &source.file, control)?
            != *lock_unpoisoned(&guard.state)
        {
            return Err(FormatError::input_changed());
        }
        source.verify().map_err(|_| FormatError::input_changed())?;
    }
    let mut addition_bytes = 0u64;
    for index in 0..additions.len() {
        let meta = additions.meta(index).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "prepared update entry index is out of range",
            )
        })?;
        addition_bytes = addition_bytes.saturating_add(meta.size);
    }
    let required = format.estimate_update_staging_bytes(
        source.state.bytes(),
        addition_bytes,
        &options.create,
    )?;
    if fs4::available_space(scope.parent_path())? < required {
        return Err(FormatError::DiskFull);
    }
    let stage = reserve_stage(&scope)?;
    let rewrite = (|| {
        progress.on_phase(ProgressPhase::UpdateRewrite, true);
        format.rewrite_update(
            Box::new(source.file.try_clone()?),
            Box::new(stage.file.try_clone()?),
            ops,
            additions,
            options,
            progress,
            control,
        )?;
        stage.file.set_permissions(source.permissions.clone())?;
        stage.file.sync_all()?;
        source.verify()
    })();
    if let Err(error) = rewrite {
        return Err(cleanup_stage(error, stage));
    }
    finish(scope, source, stage, Authorization::ContentBound, commit)
}

pub(super) fn commit_created_archive(
    requested: &Path,
    reserved: ReservedTempFile,
    guard: CreateDestinationGuard,
    progress: &dyn ProgressSink,
    control: &ControlToken,
) -> Result<(), FormatError> {
    if let Err(error) = validate_stage(&reserved)
        .and_then(|_| reserved.file.sync_all().map_err(FormatError::from))
        .and_then(|_| validate_stage(&reserved))
    {
        return Err(cleanup_stage(error, reserved));
    }
    let canonical = match replacement::canonical_requested(requested) {
        Ok(canonical) => canonical,
        Err(error) => {
            return Err(cleanup_stage(resolution_error(requested, error), reserved));
        }
    };
    let requested = canonical.as_path();
    let commit = Commit {
        requested,
        purpose: Purpose::GuardedCreate { guard },
        progress,
        control,
    };
    let admission = (|| {
        control.checkpoint()?;
        let scope = recover_scope(
            Scope::archive(requested, control)
                .map_err(|error| resolution_error(requested, error))?,
            &commit,
        )?;
        scope.require_clear()?;
        control.checkpoint()?;
        progress.on_phase(ProgressPhase::OutputVerify, true);
        let digest = verify_destination_guard_with_progress(
            requested,
            CreateArtifactKind::Archive,
            guard,
            progress,
            control,
        )?;
        let source = Source::bind(scope.target_path())
            .map_err(|error| guarded_bind_error(requested, guard, error))?;
        source
            .verify()
            .map_err(|error| guarded_error(requested, guard, error))?;
        Ok((scope, source, digest))
    })();
    let (scope, source, digest) = match admission {
        Ok(admission) => admission,
        Err(error) => return Err(cleanup_stage(error, reserved)),
    };
    let stage = adopt_stage(reserved, &scope)?;
    finish(
        scope,
        source,
        stage,
        Authorization::Guarded { digest },
        commit,
    )
}

fn finish(
    scope: Scope,
    source: Source,
    stage: ReservedTempFile,
    authorization: Authorization,
    commit: Commit<'_>,
) -> Result<(), FormatError> {
    if let Err(error) = commit
        .control
        .checkpoint()
        .and_then(|_| validate_stage(&stage))
    {
        return Err(cleanup_stage(error, stage));
    }
    commit.phase(
        ProgressPhase::UpdateVerify,
        ProgressPhase::OutputVerify,
        true,
    );
    let state = match stage.file.metadata() {
        Ok(metadata) => RegularFileState::from_metadata(&metadata),
        Err(error) => return Err(cleanup_stage(error.into(), stage)),
    };
    let replacement = OwnedInput::Archive {
        path: stage.path,
        file: stage.file,
        identity: stage.identity,
        state,
    };
    let label = public_label(commit.requested);
    let context = VerificationContext {
        public_label: &label,
        progress: commit.progress,
        control: commit.control,
    };
    let prepared = match replacement::prepare(
        scope,
        source.into_input(),
        replacement,
        authorization,
        &context,
    ) {
        Ok(prepared) => prepared,
        Err(mut attempt) => {
            attempt.error = commit.map_guard_error(attempt.error);
            return Err(archive_error(replacement::discard_prepare(attempt)));
        }
    };
    commit.phase(
        ProgressPhase::UpdateCommit,
        ProgressPhase::OutputCommit,
        false,
    );
    if let Err(error) = commit.control.checkpoint() {
        return Err(archive_error(replacement::discard_unpublished(
            prepared, error,
        )));
    }
    let owner = match replacement::persist(prepared) {
        Ok(owner) => owner,
        Err(PersistFailure::Unpublished { prepared, error }) => {
            return Err(archive_error(replacement::discard_unpublished(
                *prepared, error,
            )))
        }
        Err(PersistFailure::Retained(error)) => return Err(archive_error(error)),
    };
    let noncancelled = ControlToken::default();
    let context = VerificationContext {
        public_label: &label,
        progress: commit.progress,
        control: &noncancelled,
    };
    let installed = replacement::commit_resume(owner, &context).map_err(archive_error)?;
    let refreshed = match commit.purpose {
        Purpose::Rewrite { guard: Some(_) } => installed.regular_proof().and_then(|proof| {
            capture_source_state(commit.requested, &installed.target(), proof, &noncancelled)
        }),
        _ => None,
    };
    match replacement::complete(installed, &context).map_err(archive_error)? {
        Outcome::Archive { scope, .. } => {
            if let (Purpose::Rewrite { guard: Some(guard) }, Some(state)) =
                (commit.purpose, refreshed)
            {
                *lock_unpoisoned(&guard.state) = state;
            }
            drop(scope);
            Ok(())
        }
        _ => Err(
            io::Error::other("archive completion returned an inadmissible product result").into(),
        ),
    }
}
fn recover_scope(scope: Scope, commit: &Commit<'_>) -> Result<Scope, FormatError> {
    if scope.existing_record()?.is_some() {
        commit.phase(
            ProgressPhase::UpdateRecovery,
            ProgressPhase::OutputRecovery,
            false,
        );
    }
    let control = ControlToken::default();
    let label = public_label(commit.requested);
    let context = VerificationContext {
        public_label: &label,
        progress: commit.progress,
        control: &control,
    };
    match replacement::recover(scope, ActiveWriter::None, &context).map_err(archive_error)? {
        Recovery::Clear(scope) => Ok(scope),
        _ => Err(io::Error::other("SFX backup reached archive recovery").into()),
    }
}
fn capture_source_state(
    requested: &Path,
    target: &Path,
    proof: &replacement::RegularProof,
    control: &ControlToken,
) -> Option<ArchiveSourceState> {
    if !replacement::regular_is_current(proof, target).ok()? {
        return None;
    }
    let state = inspect_bound_archive_source_state(requested, &proof.file, control).ok()?;
    replacement::regular_is_current(proof, target)
        .ok()?
        .then_some(state)
}
fn public_label(target: &Path) -> EntryPath {
    EntryPath::from_utf8(
        target
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
    )
}

fn reserve_stage(scope: &Scope) -> Result<ReservedTempFile, FormatError> {
    for _ in 0..1000 {
        let path = scope.reserve_name("stage");
        match replacement::open_new_artifact(&path) {
            Ok(file) => {
                let identity = file_identity(&file).map_err(|error| unbound_stage(&path, error))?;
                let stage = ReservedTempFile {
                    path,
                    file,
                    identity,
                };
                validate_stage(&stage)?;
                return Ok(stage);
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Err(io::Error::other("could not reserve update writer stage").into())
}
fn adopt_stage(
    mut stage: ReservedTempFile,
    scope: &Scope,
) -> Result<ReservedTempFile, FormatError> {
    let admission = (|| {
        let parent = fs::canonicalize(crate::parent_or_current(&stage.path))?;
        if parent != scope.parent_path() {
            return Err(FormatError::Unsupported(
                "created archive staging must be next to its destination".into(),
            ));
        }
        validate_stage(&stage)
    })();
    if let Err(error) = admission {
        return Err(cleanup_stage(error, stage));
    }
    for _ in 0..1000 {
        let adopted = scope.reserve_name("stage");
        match crate::move_path_no_replace(&stage.path, &adopted) {
            Ok(()) => {
                stage.path = adopted;
                if let Err(error) = scope.sync().and_then(|_| validate_stage(&stage)) {
                    return Err(cleanup_stage(error, stage));
                }
                return Ok(stage);
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(cleanup_stage(error.into(), stage)),
        }
    }
    Err(cleanup_stage(
        io::Error::other("could not adopt created archive writer").into(),
        stage,
    ))
}
fn validate_stage(stage: &ReservedTempFile) -> Result<(), FormatError> {
    let metadata = fs::symlink_metadata(&stage.path)?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || !stage.file.metadata()?.is_file()
        || file_identity(&stage.file)? != stage.identity
        || path_identity(&stage.path)? != stage.identity
    {
        return Err(io::Error::other(
            "archive writer staging binding changed; competing path was retained",
        )
        .into());
    }
    Ok(())
}
fn cleanup_stage(original: FormatError, stage: ReservedTempFile) -> FormatError {
    let metadata = match stage.file.metadata() {
        Ok(metadata) => metadata,
        Err(error) => {
            return io::Error::other(format!("{original}; staged file was retained: {error}"))
                .into()
        }
    };
    let input = OwnedInput::Archive {
        path: stage.path,
        file: stage.file,
        identity: stage.identity,
        state: RegularFileState::from_metadata(&metadata),
    };
    match replacement::discard_archive_stage(input) {
        Ok(()) => original,
        Err(error) => io::Error::other(format!(
            "{original}; archive writer cleanup failed: {error}"
        ))
        .into(),
    }
}
fn unbound_stage(path: &Path, error: io::Error) -> FormatError {
    io::Error::new(
        error.kind(),
        format!(
            "{error}; created archive ownership was not verified; retained {}",
            path.display()
        ),
    )
    .into()
}
fn resolution_error(destination: &Path, error: FormatError) -> FormatError {
    match &error {
        FormatError::Unsupported(_) => FormatError::destination_changed(destination.to_path_buf()),
        FormatError::Io(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::PermissionDenied
            ) =>
        {
            FormatError::destination_changed(destination.to_path_buf())
        }
        _ => error,
    }
}
fn guarded_bind_error(
    destination: &Path,
    guard: CreateDestinationGuard,
    error: FormatError,
) -> FormatError {
    if matches!(&error, FormatError::Unsupported(_))
        || matches!(&error, FormatError::Io(error) if error.kind() == io::ErrorKind::NotFound)
    {
        FormatError::destination_changed(destination.to_path_buf())
    } else {
        guarded_error(destination, guard, error)
    }
}
fn guarded_error(
    destination: &Path,
    guard: CreateDestinationGuard,
    error: FormatError,
) -> FormatError {
    if matches!(error, FormatError::Cancelled) {
        return error;
    }
    match verify_destination_guard(destination, CreateArtifactKind::Archive, guard) {
        Ok(_) => error,
        Err(changed) => changed,
    }
}
fn archive_error(failure: Failure) -> FormatError {
    if failure.visibility == Visibility::Unpublished {
        return failure.error;
    }
    let paths = failure
        .artifacts
        .iter()
        .filter(|entry| !matches!(entry.role, ArtifactRole::Target))
        .map(|entry| entry.path.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    io::Error::other(format!(
        "archive replacement requires recovery: {}; retained {paths}",
        failure.error
    ))
    .into()
}

#[cfg(test)]
pub(crate) fn archive_failure_for_test(failure: Failure) -> FormatError {
    archive_error(failure)
}

#[cfg(test)]
pub(crate) fn recover_scope_for_test(
    scope: Scope,
    progress: &dyn ProgressSink,
    control: &ControlToken,
) -> Result<Scope, FormatError> {
    let requested = scope.requested_path();
    recover_scope(
        scope,
        &Commit {
            requested: &requested,
            purpose: Purpose::Rewrite { guard: None },
            progress,
            control,
        },
    )
}

#[cfg(test)]
mod tests;
