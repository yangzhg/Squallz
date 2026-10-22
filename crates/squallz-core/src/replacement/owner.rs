use std::fs;
use std::path::Path;

use crate::api::{FormatError, ProgressPhase};
use crate::filesystem_identity::{file_identity, path_identity};

use super::evidence::{invalid, EntryProof, ProofSlot, StatePoint, VerificationContext};
use super::model::*;
use super::moves::{self, Entry};
use super::scope::{identity_at, HeldDirectory, RecordPhase, Scope};

pub(crate) enum Recovery {
    Clear(Scope),
    Backup {
        scope: Scope,
        backup: std::path::PathBuf,
    },
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum CompletionUse {
    Installation,
    Reopening,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum EntryPosition {
    Missing,
    Previous,
    Replacement,
    ActiveWriter,
    Other,
}

/// Fresh and reopened owners execute one fixed sequence. Current positions
/// identify which recorded object occupies each path.
pub(crate) fn commit_resume<'a>(
    mut owner: Replacement<'a>,
    context: &VerificationContext<'_>,
) -> Result<Installed<'a>, Failure> {
    let work = (|| {
        moves::recover_aliases(&mut owner, context)?;
        owner.record.verify()?;
        moves::verify_bindings(&owner.bindings)?;
        let layout = &owner.bindings.layout;
        let scope = &owner.bindings.scope;
        let stage = layout.stage(scope);
        let protected = layout.protected(scope);
        let target = layout.target(scope);
        let previous = layout.previous(scope);
        let retired = layout.retired(scope);
        require_missing(&retired)?;
        let writer = position(&mut owner, &stage, true, context)?;
        let protected_state = position(&mut owner, &protected, false, context)?;
        let target_state = position(&mut owner, &target, false, context)?;
        let previous_state = position(&mut owner, &previous, false, context)?;
        let installed_archive = !owner.bindings.scope.is_sfx()
            && writer == EntryPosition::Missing
            && protected_state == EntryPosition::Missing
            && target_state == EntryPosition::Replacement
            && matches!(
                previous_state,
                EntryPosition::Previous | EntryPosition::Missing
            );
        if !installed_archive && !matches!(owner.bindings.holder, HolderProof::Held(_)) {
            return Err(invalid("installation requires its original held holder"));
        }
        match (writer, protected_state, target_state) {
            (EntryPosition::Replacement, EntryPosition::Replacement, _) => {
                moves::normalize_alias(&mut owner, AliasPair::WriterAndProtected, context)?
            }
            (EntryPosition::Replacement, EntryPosition::Missing, EntryPosition::Replacement) => {
                moves::normalize_alias(&mut owner, AliasPair::WriterAndInstalled, context)?
            }
            (EntryPosition::Replacement, EntryPosition::Missing, EntryPosition::Previous)
                if previous_state == EntryPosition::Missing =>
            {
                owner.bindings.verify_authorization(&target)?;
                moves::checked_move(
                    &mut owner,
                    Entry::Replacement,
                    &stage,
                    &protected,
                    StatePoint::BeforeMove,
                    context,
                )?;
            }
            (
                EntryPosition::Missing | EntryPosition::ActiveWriter,
                EntryPosition::Replacement,
                _,
            )
            | (
                EntryPosition::Missing | EntryPosition::ActiveWriter,
                EntryPosition::Missing,
                EntryPosition::Replacement,
            ) => {}
            _ => return Err(invalid("new-output protection state is inconsistent")),
        }
        let target_state = position(&mut owner, &target, false, context)?;
        let previous_state = position(&mut owner, &previous, false, context)?;
        match (target_state, previous_state) {
            (EntryPosition::Previous, EntryPosition::Previous) => {
                moves::require(
                    &mut owner.bindings,
                    Entry::Replacement,
                    &protected,
                    StatePoint::AfterRename,
                    false,
                    context,
                )?;
                moves::normalize_alias(&mut owner, AliasPair::TargetAndPrevious, context)?;
            }
            (EntryPosition::Previous, EntryPosition::Missing) => {
                owner.bindings.verify_authorization(&target)?;
                moves::require(
                    &mut owner.bindings,
                    Entry::Replacement,
                    &protected,
                    StatePoint::AfterRename,
                    false,
                    context,
                )?;
                moves::checked_move(
                    &mut owner,
                    Entry::Previous,
                    &target,
                    &previous,
                    StatePoint::BeforeMove,
                    context,
                )?;
            }
            (EntryPosition::Missing | EntryPosition::Replacement, EntryPosition::Previous) => {}
            (EntryPosition::Replacement, EntryPosition::Missing) if installed_archive => {}
            _ => {
                return Err(invalid(
                    "previous-output preservation state is inconsistent",
                ))
            }
        }
        let protected_state = position(&mut owner, &protected, false, context)?;
        let target_state = position(&mut owner, &target, false, context)?;
        match (protected_state, target_state) {
            (EntryPosition::Replacement, EntryPosition::Replacement) => {
                moves::normalize_alias(&mut owner, AliasPair::ProtectedAndInstalled, context)?
            }
            (EntryPosition::Replacement, EntryPosition::Missing) => {
                owner.bindings.verify_authorization(&previous)?;
                moves::checked_move(
                    &mut owner,
                    Entry::Replacement,
                    &protected,
                    &target,
                    StatePoint::AfterRename,
                    context,
                )?;
            }
            (EntryPosition::Missing, EntryPosition::Replacement) => {}
            _ => return Err(invalid("installation state is inconsistent")),
        }
        verify_installed(&mut owner, context, false)?;
        // Reopening at Installed still synchronizes both held parents.
        moves::sync_bindings(&owner.bindings)?;
        owner.record.verify()
    })();
    match work {
        Ok(()) => Ok(Installed { owner }),
        Err(error) => Err(failure(&owner, error)),
    }
}

pub(crate) fn recover(
    scope: Scope,
    active: ActiveWriter<'_>,
    context: &VerificationContext<'_>,
) -> Result<Recovery, Failure> {
    let path = match scope.existing_record() {
        Ok(Some(path)) => path,
        Ok(None) => return Ok(Recovery::Clear(scope)),
        Err(error) => return Err(scope_failure(&scope, error)),
    };
    let phase = match scope.phase(&path) {
        Ok(phase) => phase,
        Err(error) => return Err(scope_failure(&scope, error)),
    };
    let (record, body) = match super::record::read(&path, &scope) {
        Ok(record) => record,
        Err(error) => return Err(scope_failure(&scope, error)),
    };
    let layout = match RecordLayout::validate(body, &scope) {
        Ok(layout) => layout,
        Err(error) => return Err(scope_failure(&scope, error)),
    };
    let mut bindings = Bindings {
        scope,
        layout,
        holder: HolderProof::Missing,
        proofs: ProofPair {
            previous: ProofSlot::Unverified,
            replacement: ProofSlot::Unverified,
        },
    };
    bindings.holder = reopen_holder(&bindings.scope, &bindings.layout, phase)
        .map_err(|error| binding_failure(&bindings, error, Visibility::RecordMayBeDurable))?;
    let mut owner = Replacement {
        bindings,
        record,
        active,
        visibility: Visibility::RecordMayBeDurable,
    };
    if phase == RecordPhase::Pending {
        let journal = owner.bindings.scope.anchor(RecordPhase::Installing);
        if let Err(error) =
            super::record::move_record(&mut owner.record, &journal, &owner.bindings.scope)
        {
            return Err(record_failure(&owner, error));
        }
    }
    let outcome = match phase {
        RecordPhase::Pending | RecordPhase::Installing => {
            complete_inner(commit_resume(owner, context)?, context, false)?
        }
        RecordPhase::Completed => finish_completed(owner, context, CompletionUse::Reopening)?,
    };
    match outcome {
        Outcome::Archive { scope, .. } => Ok(Recovery::Clear(scope)),
        Outcome::Sfx { scope, backup } => Ok(Recovery::Backup { scope, backup }),
        Outcome::SfxCleared { scope } => Ok(Recovery::Clear(scope)),
    }
}

pub(crate) fn complete(
    installed: Installed<'_>,
    context: &VerificationContext<'_>,
) -> Result<Outcome, Failure> {
    complete_inner(installed, context, true)
}
fn complete_inner(
    installed: Installed<'_>,
    context: &VerificationContext<'_>,
    report_cleanup: bool,
) -> Result<Outcome, Failure> {
    let mut owner = installed.owner;
    if let Err(error) = verify_installed(&mut owner, context, false)
        .and_then(|_| moves::sync_bindings(&owner.bindings))
    {
        return Err(failure(&owner, error));
    }
    let completed = owner.bindings.scope.anchor(RecordPhase::Completed);
    if let Err(error) =
        super::record::move_record(&mut owner.record, &completed, &owner.bindings.scope)
    {
        return Err(record_failure(&owner, error));
    }
    if report_cleanup {
        if let PairEvidence::Archive { authorization, .. } =
            &owner.bindings.layout.record().evidence
        {
            context.progress.on_phase(
                match authorization {
                    Authorization::ContentBound => ProgressPhase::UpdateCleanup,
                    Authorization::Guarded { .. } => ProgressPhase::OutputCleanup,
                },
                false,
            );
        }
    }
    finish_completed(owner, context, CompletionUse::Installation)
}
fn finish_completed(
    mut owner: Replacement<'_>,
    context: &VerificationContext<'_>,
    completion: CompletionUse,
) -> Result<Outcome, Failure> {
    let work = (|| {
        owner.visibility = Visibility::OutputMayBeVisible;
        let scope = &owner.bindings.scope;
        let layout = &owner.bindings.layout;
        let target = layout.target(scope);
        let previous = layout.previous(scope);
        let retired = layout.retired(scope);
        let protected = layout.protected(scope);
        let stage = layout.stage(scope);
        owner.record.verify()?;
        moves::verify_bindings(&owner.bindings)?;
        absent_or_active(&owner, &stage)?;
        require_missing(&protected)?;
        moves::require(
            &mut owner.bindings,
            Entry::Replacement,
            &target,
            StatePoint::AfterRename,
            false,
            context,
        )?;
        if owner.bindings.scope.is_sfx() {
            require_missing(&retired)?;
            if identity_at(&previous)?.is_some() {
                moves::sync_bindings(&owner.bindings)?;
                owner.record.verify()?;
                moves::require(
                    &mut owner.bindings,
                    Entry::Previous,
                    &previous,
                    StatePoint::AfterRename,
                    true,
                    context,
                )?;
                return Ok(true);
            }
            if completion == CompletionUse::Installation {
                return Err(invalid("new SFX completion requires its preserved backup"));
            }
        } else {
            match (identity_at(&previous)?, identity_at(&retired)?) {
                (Some(_), None) => {
                    owner.bindings.verify_authorization(&previous)?;
                    moves::checked_move(
                        &mut owner,
                        Entry::Previous,
                        &previous,
                        &retired,
                        StatePoint::AfterRename,
                        context,
                    )?;
                }
                (None, Some(_)) => owner.bindings.verify_authorization(&retired)?,
                (None, None) => {}
                _ => {
                    return Err(invalid(
                        "completed archive has both previous and retired entries",
                    ))
                }
            }
            if identity_at(&retired)?.is_some() {
                moves::remove_retired(&mut owner, context)?;
            }
        }
        moves::require(
            &mut owner.bindings,
            Entry::Replacement,
            &target,
            StatePoint::AfterRename,
            false,
            context,
        )?;
        moves::sync_bindings(&owner.bindings)?;
        moves::remove_empty_holder(&mut owner)?;
        if let Err(error) = super::record::clear(&mut owner.record, &owner.bindings.scope) {
            return Err(error.error);
        }
        Ok(false)
    })();
    let backup_exists = match work {
        Ok(value) => value,
        Err(error) => return Err(failure(&owner, error)),
    };
    let Bindings { scope, layout, .. } = owner.bindings;
    if scope.is_sfx() {
        if !backup_exists {
            return Ok(Outcome::SfxCleared { scope });
        }
        return Ok(Outcome::Sfx {
            backup: layout.previous(&scope),
            scope,
        });
    }
    Ok(Outcome::Archive { scope })
}

#[cfg(test)]
pub(crate) fn finish_completed_for_test(
    owner: Replacement<'_>,
    context: &VerificationContext<'_>,
) -> Result<Outcome, Failure> {
    finish_completed(owner, context, CompletionUse::Reopening)
}

fn verify_installed(
    owner: &mut Replacement<'_>,
    context: &VerificationContext<'_>,
    allow_removed_previous: bool,
) -> Result<(), FormatError> {
    let layout = &owner.bindings.layout;
    let scope = &owner.bindings.scope;
    let target = layout.target(scope);
    let stage = layout.stage(scope);
    let protected = layout.protected(scope);
    let previous = layout.previous(scope);
    let retired = layout.retired(scope);
    absent_or_active(owner, &stage)?;
    require_missing(&protected)?;
    require_missing(&retired)?;
    moves::require(
        &mut owner.bindings,
        Entry::Replacement,
        &target,
        StatePoint::AfterRename,
        false,
        context,
    )?;
    if identity_at(&previous)?.is_some() {
        owner.bindings.verify_authorization(&previous)?;
        moves::require(
            &mut owner.bindings,
            Entry::Previous,
            &previous,
            StatePoint::AfterRename,
            false,
            context,
        )?;
    } else if owner.bindings.scope.is_sfx() && !allow_removed_previous {
        return Err(invalid(
            "SFX installation requires its previous-output backup",
        ));
    }
    moves::verify_bindings(&owner.bindings)
}
fn position(
    owner: &mut Replacement<'_>,
    path: &Path,
    is_stage: bool,
    context: &VerificationContext<'_>,
) -> Result<EntryPosition, FormatError> {
    let Some(identity) = identity_at(path)? else {
        return Ok(EntryPosition::Missing);
    };
    let evidence = &owner.bindings.layout.record().evidence;
    let (entry, position) = if identity == evidence.replacement().identity() {
        (Entry::Replacement, EntryPosition::Replacement)
    } else if identity == evidence.previous().identity() {
        (Entry::Previous, EntryPosition::Previous)
    } else if is_stage && active_matches(owner, path)? {
        return Ok(EntryPosition::ActiveWriter);
    } else {
        return Ok(EntryPosition::Other);
    };
    let target = owner.bindings.layout.target(&owner.bindings.scope);
    if path == target && entry == Entry::Replacement {
        owner.visibility = Visibility::OutputMayBeVisible;
    }
    let point = if is_stage || (path == target && entry == Entry::Previous) {
        StatePoint::BeforeMove
    } else {
        StatePoint::AfterRename
    };
    moves::require(&mut owner.bindings, entry, path, point, false, context)?;
    Ok(position)
}
pub(crate) fn absent_or_active(owner: &Replacement<'_>, path: &Path) -> Result<(), FormatError> {
    if identity_at(path)?.is_none()
        || (path == owner.bindings.layout.stage(&owner.bindings.scope)
            && active_matches(owner, path)?)
    {
        return Ok(());
    }
    Err(invalid("old writer path contains an unadmitted entry"))
}
fn active_matches(owner: &Replacement<'_>, path: &Path) -> Result<bool, FormatError> {
    if !owner.bindings.scope.is_sfx() {
        return Ok(false);
    }
    let ActiveWriter::With(input) = &owner.active else {
        return Ok(false);
    };
    let (held, regular) = match input {
        OwnedInput::SfxFile { file, .. } => (file, true),
        OwnedInput::SfxTree { root, .. } => (root, false),
        OwnedInput::Archive { .. } => return Ok(false),
    };
    let identity = input.identity();
    let metadata = fs::symlink_metadata(path)?;
    let evidence = &owner.bindings.layout.record().evidence;
    Ok(input.path() == path
        && identity != evidence.replacement().identity()
        && identity != evidence.previous().identity()
        && !metadata.file_type().is_symlink()
        && (if regular {
            metadata.is_file()
        } else {
            metadata.is_dir()
        })
        && file_identity(held)? == identity
        && path_identity(path)? == identity)
}
fn require_missing(path: &Path) -> Result<(), FormatError> {
    if identity_at(path)?.is_some() {
        return Err(invalid("transaction-owned path is unexpectedly occupied"));
    }
    Ok(())
}
fn reopen_holder(
    scope: &Scope,
    layout: &RecordLayout,
    phase: RecordPhase,
) -> Result<HolderProof, FormatError> {
    let path = layout.holder(scope);
    let expected = layout.record().holder_identity;
    match identity_at(&path)? {
        Some(identity) if identity == expected => {
            Ok(HolderProof::Held(HeldDirectory::open(path, expected)?))
        }
        Some(_) => Err(invalid("record holder identity changed")),
        None => {
            let isolation = layout.holder_isolation(scope);
            match identity_at(&isolation)? {
                Some(identity) if identity == expected && phase == RecordPhase::Completed => {
                    let held = HeldDirectory::open(isolation, expected)?;
                    if fs::read_dir(&held.path)?.next().is_some() {
                        return Err(invalid("isolated completed holder is not empty"));
                    }
                    Ok(HolderProof::Missing)
                }
                None => Ok(HolderProof::Missing),
                _ => Err(invalid("record holder isolation is not admitted")),
            }
        }
    }
}

pub(crate) fn binding_failure(
    bindings: &Bindings,
    error: FormatError,
    visibility: Visibility,
) -> Failure {
    let mut failure = scope_failure(&bindings.scope, error);
    failure.visibility = visibility;
    for (role, path) in bindings.layout.artifacts(&bindings.scope) {
        add_observable(&mut failure, role, path);
    }
    failure
}
fn failure(owner: &Replacement<'_>, error: FormatError) -> Failure {
    let mut failure = binding_failure(&owner.bindings, error, owner.visibility);
    add_observable(
        &mut failure,
        ArtifactRole::Record,
        owner.record.path.clone(),
    );
    match &owner.active {
        ActiveWriter::With(input) => {
            let path = input.path();
            if matches!(active_matches(owner, path), Ok(true)) {
                failure.artifacts.retain(|item| {
                    item.path != path || !matches!(item.role, ArtifactRole::WriterStage)
                });
            }
            add_observable(&mut failure, ArtifactRole::ActiveWriter, path.to_path_buf())
        }
        ActiveWriter::None => {}
    }
    failure
}
fn record_failure(owner: &Replacement<'_>, error: super::record::RecordFailure) -> Failure {
    let mut failure = failure(owner, error.error);
    for path in error.paths {
        add_observable(&mut failure, ArtifactRole::Record, path);
    }
    failure
}
pub(crate) fn scope_failure(scope: &Scope, error: FormatError) -> Failure {
    let mut failure = Failure {
        error,
        visibility: Visibility::RecordMayBeDurable,
        artifacts: Vec::new(),
    };
    failure.add(ArtifactRole::Target, scope.target_path());
    for phase in scope.phases() {
        add_observable(&mut failure, ArtifactRole::Record, scope.anchor(phase));
    }
    failure
}
fn add_observable(failure: &mut Failure, role: ArtifactRole, path: std::path::PathBuf) {
    if !matches!(identity_at(&path), Ok(None)) {
        failure.add(role, path);
    }
}

pub(crate) fn discard_unpublished(prepared: Prepared, original: FormatError) -> Failure {
    let mut failure = binding_failure(&prepared.bindings, original, Visibility::Unpublished);
    let Bindings {
        scope,
        holder,
        layout,
        proofs,
    } = prepared.bindings;
    let holder = match holder {
        HolderProof::Held(holder) => HolderReservation::Held(holder),
        HolderProof::Missing => HolderReservation::Unreserved,
    };
    discard_holder(&scope, holder, &mut failure);
    if !scope.is_sfx() && failure.visibility == Visibility::Unpublished {
        if let ProofSlot::Verified(EntryProof::Regular(proof)) = proofs.replacement {
            let input = OwnedInput::Archive {
                path: layout.stage(&scope),
                file: proof.file,
                identity: proof.identity,
                state: proof.state,
            };
            discharged_stage(&mut failure, super::prepare::discard_archive_stage(input));
        }
    }
    failure
}
pub(crate) fn discard_prepare(attempt: Box<PrepareFailure>) -> Failure {
    let PrepareFailure {
        scope,
        previous,
        replacement,
        error,
        holder,
    } = *attempt;
    let mut failure = Failure {
        error,
        visibility: Visibility::Unpublished,
        artifacts: Vec::new(),
    };
    failure.add(ArtifactRole::Target, previous.path().to_path_buf());
    failure.add(ArtifactRole::WriterStage, replacement.path().to_path_buf());
    discard_holder(&scope, holder, &mut failure);
    if !scope.is_sfx() && failure.visibility == Visibility::Unpublished {
        discharged_stage(
            &mut failure,
            super::prepare::discard_archive_stage(replacement),
        );
    }
    failure
}
fn discard_holder(scope: &Scope, holder: HolderReservation, failure: &mut Failure) {
    let path = match &holder {
        HolderReservation::Unreserved => None,
        HolderReservation::Unidentified { path } => Some(path.clone()),
        HolderReservation::Held(holder) => Some(holder.path.clone()),
    };
    match super::prepare::abandon_holder(holder, scope) {
        Ok(()) => failure.artifacts.retain(|item| {
            !matches!(
                item.role,
                ArtifactRole::Holder | ArtifactRole::HolderIsolation
            )
        }),
        Err(error) => {
            failure.error = invalid(&format!(
                "{}; unused holder cleanup failed: {error}",
                failure.error
            ));
            failure.visibility = Visibility::RecordMayBeDurable;
            if let Some(path) = path {
                if let Ok(name) = Name::from_path(&path) {
                    add_observable(
                        failure,
                        ArtifactRole::HolderIsolation,
                        name.suffixed(".empty-isolation").join(scope.parent_path()),
                    );
                }
                failure.add(ArtifactRole::Holder, path);
            }
        }
    }
}
fn discharged_stage(failure: &mut Failure, cleanup: Result<(), FormatError>) {
    match cleanup {
        Ok(()) => failure
            .artifacts
            .retain(|item| !matches!(item.role, ArtifactRole::WriterStage)),
        Err(error) => {
            failure.error = invalid(&format!(
                "{}; archive staging cleanup failed: {error}",
                failure.error
            ));
            failure.visibility = Visibility::RecordMayBeDurable;
        }
    }
}

/// A read-only artifact classifier uses the same header validator and full
/// evidence. It never resumes, acknowledges or removes a record.
pub(crate) fn classify_sfx_artifact(
    candidate: &Path,
    context: &VerificationContext<'_>,
) -> Result<Option<bool>, Failure> {
    let parent = if candidate
        .parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_prefix(".squallz-sfx-holder-"))
        .is_some_and(crate::archive_path::is_canonical_process_sequence)
    {
        candidate
            .parent()
            .and_then(Path::parent)
            .unwrap_or_else(|| Path::new("."))
    } else {
        crate::parent_or_current(candidate)
    };
    let component = Name::from_path(candidate).map_err(|error| Failure {
        error,
        visibility: Visibility::RecordMayBeDurable,
        artifacts: vec![Artifact {
            role: ArtifactRole::Target,
            path: candidate.to_path_buf(),
        }],
    })?;
    let mut scope = Scope::inspect_sfx(&component.join(parent)).map_err(|error| Failure {
        error,
        visibility: Visibility::RecordMayBeDurable,
        artifacts: vec![Artifact {
            role: ArtifactRole::Target,
            path: candidate.to_path_buf(),
        }],
    })?;
    let path = match scope
        .existing_record()
        .map_err(|error| scope_failure(&scope, error))?
    {
        Some(path) => path,
        None if scope.reserved_artifact(candidate) => {
            return Err(scope_failure(
                &scope,
                invalid("reserved SFX artifact has no current durable owner"),
            ))
        }
        None => return Ok(None),
    };
    let (record, body) =
        super::record::read(&path, &scope).map_err(|error| scope_failure(&scope, error))?;
    scope
        .inspection_target(&body)
        .map_err(|error| scope_failure(&scope, error))?;
    let layout =
        RecordLayout::validate(body, &scope).map_err(|error| scope_failure(&scope, error))?;
    let phase = scope
        .phase(&path)
        .map_err(|error| scope_failure(&scope, error))?;
    let holder =
        reopen_holder(&scope, &layout, phase).map_err(|error| scope_failure(&scope, error))?;
    let mut bindings = Bindings {
        scope,
        layout,
        holder,
        proofs: ProofPair {
            previous: ProofSlot::Unverified,
            replacement: ProofSlot::Unverified,
        },
    };
    let work = (|| {
        record.verify()?;
        moves::verify_inventory(&bindings)?;
        let target = bindings.layout.target(&bindings.scope);
        if phase == RecordPhase::Completed {
            moves::require(
                &mut bindings,
                Entry::Replacement,
                &target,
                StatePoint::AfterRename,
                true,
                context,
            )?;
            let previous = bindings.layout.previous(&bindings.scope);
            if identity_at(&previous)?.is_some() {
                moves::require(
                    &mut bindings,
                    Entry::Previous,
                    &previous,
                    StatePoint::AfterRename,
                    true,
                    context,
                )?;
            }
            if let HolderProof::Held(holder) = &bindings.holder {
                for entry in fs::read_dir(&holder.path)? {
                    if entry?.file_name() != "previous" {
                        return Err(invalid("completed SFX holder contains an unowned member"));
                    }
                }
            }
        } else {
            let mut reached = [false; 2];
            for (entry, path, point) in [
                (
                    Entry::Replacement,
                    bindings.layout.stage(&bindings.scope),
                    StatePoint::BeforeMove,
                ),
                (
                    Entry::Replacement,
                    bindings.layout.protected(&bindings.scope),
                    StatePoint::AfterRename,
                ),
                (Entry::Replacement, target.clone(), StatePoint::AfterRename),
                (Entry::Previous, target, StatePoint::BeforeMove),
                (
                    Entry::Previous,
                    bindings.layout.previous(&bindings.scope),
                    StatePoint::AfterRename,
                ),
            ] {
                if identity_at(&path)? == Some(entry.expected(&bindings).identity()) {
                    moves::require(&mut bindings, entry, &path, point, true, context)?;
                    reached[usize::from(entry == Entry::Replacement)] = true;
                } else if identity_at(&path)?.is_some()
                    && path != bindings.layout.target(&bindings.scope)
                {
                    return Err(invalid(
                        "record-owned SFX artifact has an unexpected identity",
                    ));
                }
            }
            if reached != [true, true] {
                return Err(invalid("both SFX identities must remain reachable"));
            }
        }
        record.verify()?;
        Ok(())
    })();
    work.map_err(|error| binding_failure(&bindings, error, Visibility::RecordMayBeDurable))?;
    let candidate = super::scope::canonical_requested(candidate)
        .map_err(|error| binding_failure(&bindings, error, Visibility::RecordMayBeDurable))?;
    let owned = candidate == record.path
        || bindings
            .layout
            .artifacts(&bindings.scope)
            .into_iter()
            .any(|(_, path)| path == candidate);
    Ok(Some(owned && !matches!(identity_at(&candidate), Ok(None))))
}
