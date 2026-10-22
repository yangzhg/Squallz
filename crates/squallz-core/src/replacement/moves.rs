use std::fs;
use std::path::Path;

use crate::api::FormatError;
use crate::filesystem_identity::{
    file_identity, open_regular_file_no_follow_for_cleanup, path_identity, PathIdentity,
};

use super::evidence::{
    invalid, observe, proof_identity_is_current, proof_is_current, regular_is_current, EntryProof,
    ExpectedEntry, Observation, ProofSlot, StatePoint, VerificationContext,
};
use super::model::*;
use super::scope::{identity_at, HeldDirectory};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Entry {
    Previous,
    Replacement,
}
impl Entry {
    pub(crate) fn expected(self, bindings: &Bindings) -> ExpectedEntry<'_> {
        match self {
            Self::Previous => bindings.layout.record().evidence.previous(),
            Self::Replacement => bindings.layout.record().evidence.replacement(),
        }
    }
    pub(crate) fn slot(self, bindings: &Bindings) -> &ProofSlot {
        match self {
            Self::Previous => &bindings.proofs.previous,
            Self::Replacement => &bindings.proofs.replacement,
        }
    }
    pub(crate) fn put(self, bindings: &mut Bindings, proof: EntryProof) {
        let slot = match self {
            Self::Previous => &mut bindings.proofs.previous,
            Self::Replacement => &mut bindings.proofs.replacement,
        };
        *slot = ProofSlot::Verified(proof);
    }
}

pub(crate) fn verify_bindings(bindings: &Bindings) -> Result<(), FormatError> {
    bindings.scope.verify_locked()?;
    verify_inventory(bindings)
}
pub(crate) fn verify_inventory(bindings: &Bindings) -> Result<(), FormatError> {
    bindings.scope.verify()?;
    if let HolderProof::Held(holder) = &bindings.holder {
        holder.verify()?;
        for entry in fs::read_dir(&holder.path)? {
            let name = entry?.file_name();
            let allowed = name == "previous"
                || name == "replacement"
                || (!bindings.scope.is_sfx() && name == "retired")
                || (matches!(
                    bindings.layout.record().evidence,
                    PairEvidence::SfxFile { .. }
                ) && name == "alias-replacement");
            if !allowed {
                return Err(invalid("replacement holder contains an unowned member"));
            }
        }
        holder.verify()?;
    }
    Ok(())
}
pub(crate) fn sync_bindings(bindings: &Bindings) -> Result<(), FormatError> {
    verify_bindings(bindings)?;
    if let HolderProof::Held(holder) = &bindings.holder {
        holder.sync()?;
    }
    bindings.scope.sync()?;
    verify_bindings(bindings)
}
pub(crate) fn require(
    bindings: &mut Bindings,
    entry: Entry,
    path: &Path,
    point: StatePoint,
    force_content: bool,
    context: &VerificationContext<'_>,
) -> Result<(), FormatError> {
    let observed = observe(
        path,
        entry.expected(bindings),
        point,
        entry.slot(bindings),
        force_content,
        context,
    )?;
    match observed {
        Observation::Expected(proof) => {
            entry.put(bindings, proof);
            Ok(())
        }
        _ => Err(invalid("replacement member is missing or changed")),
    }
}

/// Archive restores a failed content binding; SFX retains the forward position
/// for replay. A sync failure leaves the moved entry in place for both routes.
pub(crate) fn checked_move(
    owner: &mut Replacement<'_>,
    entry: Entry,
    source: &Path,
    destination: &Path,
    point: StatePoint,
    context: &VerificationContext<'_>,
) -> Result<(), FormatError> {
    owner.record.verify()?;
    verify_bindings(&owner.bindings)?;
    require(&mut owner.bindings, entry, source, point, false, context)?;
    if identity_at(destination)?.is_some() {
        return Err(invalid("no-replace move destination is occupied"));
    }
    let expected = entry.expected(&owner.bindings).identity();
    let ProofSlot::Verified(proof) = entry.slot(&owner.bindings) else {
        return Err(invalid("move has no owned source proof"));
    };
    if !proof_is_current(proof, source)? {
        return Err(invalid("source changed immediately before move"));
    }
    let moved = super::move_path_no_replace(source, destination);
    let after = identity_at(destination);
    if moved.is_ok()
        || after.is_err()
        || after
            .as_ref()
            .is_ok_and(|identity| *identity == Some(expected))
    {
        owner.visibility = owner.visibility.max(Visibility::OutputMayBeVisible);
    }
    let after = after?;
    if let Some(unexpected) = after.filter(|identity| *identity != expected) {
        if !owner.bindings.scope.is_sfx() {
            restore_known(owner, destination, source, unexpected)?;
        }
        return Err(invalid(
            "entry move selected a rebound source; observed competitor was retained or restored",
        ));
    }
    moved?;
    sync_bindings(&owner.bindings)?;
    let settled = (|| {
        if after != Some(expected) || identity_at(source)?.is_some() {
            return Err(invalid(
                "entry move left an unexpected source or destination",
            ));
        }
        let ProofSlot::Verified(proof) = entry.slot(&owner.bindings) else {
            return Err(invalid("moved source proof was lost"));
        };
        if !proof_identity_is_current(proof, destination, expected)? {
            return Err(invalid("original held source did not reach destination"));
        }
        require(
            &mut owner.bindings,
            entry,
            destination,
            StatePoint::AfterRename,
            false,
            context,
        )?;
        owner.record.verify()?;
        verify_bindings(&owner.bindings)
    })();
    if let Err(error) = settled {
        if let Some(observed) = after.filter(|_| !owner.bindings.scope.is_sfx()) {
            restore_known(owner, destination, source, observed)?;
        }
        return Err(error);
    }
    Ok(())
}

fn restore_known(
    owner: &Replacement<'_>,
    from: &Path,
    to: &Path,
    observed: PathIdentity,
) -> Result<(), FormatError> {
    verify_bindings(&owner.bindings)?;
    owner.record.verify()?;
    owner.bindings.scope.restore_known(from, to, observed)?;
    sync_bindings(&owner.bindings)
}

pub(crate) fn normalize_alias(
    owner: &mut Replacement<'_>,
    pair: AliasPair,
    context: &VerificationContext<'_>,
) -> Result<(), FormatError> {
    if !matches!(
        owner.bindings.layout.record().evidence,
        PairEvidence::SfxFile { .. }
    ) {
        return Err(invalid(
            "only regular SFX entries permit duplicate hard-link paths",
        ));
    }
    let (entry, source, survivor) = alias_paths(&owner.bindings, pair);
    let isolation = owner.bindings.layout.alias(&owner.bindings.scope, pair);
    require(
        &mut owner.bindings,
        entry,
        &survivor,
        StatePoint::AfterRename,
        true,
        context,
    )?;
    checked_move(
        owner,
        entry,
        &source,
        &isolation,
        StatePoint::AfterRename,
        context,
    )?;
    remove_alias(owner, entry, &source, &survivor, &isolation, context)
}
pub(crate) fn recover_aliases(
    owner: &mut Replacement<'_>,
    context: &VerificationContext<'_>,
) -> Result<(), FormatError> {
    for pair in [
        AliasPair::WriterAndProtected,
        AliasPair::WriterAndInstalled,
        AliasPair::TargetAndPrevious,
        AliasPair::ProtectedAndInstalled,
    ] {
        let isolation = owner.bindings.layout.alias(&owner.bindings.scope, pair);
        let Some(identity) = identity_at(&isolation)? else {
            continue;
        };
        let (entry, source, survivor) = alias_paths(&owner.bindings, pair);
        if !matches!(
            owner.bindings.layout.record().evidence,
            PairEvidence::SfxFile { .. }
        ) || identity != entry.expected(&owner.bindings).identity()
        {
            return Err(invalid("alias isolation has an unowned type or identity"));
        }
        // The writer alias has one fixed isolation and two legitimate survivors.
        if pair == AliasPair::WriterAndProtected && identity_at(&survivor)? != Some(identity) {
            continue;
        }
        super::owner::absent_or_active(owner, &source)?;
        remove_alias(owner, entry, &source, &survivor, &isolation, context)?;
    }
    Ok(())
}
fn alias_paths(
    bindings: &Bindings,
    pair: AliasPair,
) -> (Entry, std::path::PathBuf, std::path::PathBuf) {
    let layout = &bindings.layout;
    let scope = &bindings.scope;
    match pair {
        AliasPair::WriterAndProtected => (
            Entry::Replacement,
            layout.stage(scope),
            layout.protected(scope),
        ),
        AliasPair::WriterAndInstalled => (
            Entry::Replacement,
            layout.stage(scope),
            layout.target(scope),
        ),
        AliasPair::TargetAndPrevious => (
            Entry::Previous,
            layout.target(scope),
            layout.previous(scope),
        ),
        AliasPair::ProtectedAndInstalled => (
            Entry::Replacement,
            layout.protected(scope),
            layout.target(scope),
        ),
    }
}
fn remove_alias(
    owner: &mut Replacement<'_>,
    entry: Entry,
    source: &Path,
    survivor: &Path,
    isolation: &Path,
    context: &VerificationContext<'_>,
) -> Result<(), FormatError> {
    owner.record.verify()?;
    verify_bindings(&owner.bindings)?;
    require(
        &mut owner.bindings,
        entry,
        isolation,
        StatePoint::AfterRename,
        true,
        context,
    )?;
    let survivor_proof = match observe(
        survivor,
        entry.expected(&owner.bindings),
        StatePoint::AfterRename,
        entry.slot(&owner.bindings),
        true,
        context,
    )? {
        Observation::Expected(proof) => proof,
        _ => {
            return Err(invalid(
                "hard-link survivor changed before duplicate removal",
            ))
        }
    };
    let ProofSlot::Verified(isolated) = entry.slot(&owner.bindings) else {
        return Err(invalid("alias has no verified isolation"));
    };
    if !proof_is_current(&survivor_proof, survivor)? || !proof_is_current(isolated, isolation)? {
        return Err(invalid(
            "hard-link isolation or survivor changed immediately before removal",
        ));
    }
    if let Err(error) = fs::remove_file(isolation) {
        let identity = entry.expected(&owner.bindings).identity();
        restore_known(owner, isolation, source, identity)?;
        return Err(error.into());
    }
    entry.put(&mut owner.bindings, survivor_proof);
    sync_bindings(&owner.bindings)?;
    require(
        &mut owner.bindings,
        entry,
        survivor,
        StatePoint::AfterRename,
        true,
        context,
    )
}

pub(crate) fn remove_retired(
    owner: &mut Replacement<'_>,
    context: &VerificationContext<'_>,
) -> Result<(), FormatError> {
    let path = owner.bindings.layout.retired(&owner.bindings.scope);
    let target = owner.bindings.layout.target(&owner.bindings.scope);
    owner.record.verify()?;
    verify_bindings(&owner.bindings)?;
    require(
        &mut owner.bindings,
        Entry::Previous,
        &path,
        StatePoint::AfterRename,
        false,
        context,
    )?;
    require(
        &mut owner.bindings,
        Entry::Replacement,
        &target,
        StatePoint::AfterRename,
        false,
        context,
    )?;
    owner.bindings.verify_authorization(&path)?;
    let old = std::mem::replace(&mut owner.bindings.proofs.previous, ProofSlot::Unverified);
    let ProofSlot::Verified(EntryProof::Regular(proof)) = old else {
        return Err(invalid("archive retirement requires a regular proof"));
    };
    if !regular_is_current(&proof, &path)? {
        return Err(invalid(
            "retired file changed before cleanup handle opening",
        ));
    }
    let state = proof.state;
    let identity = proof.identity;
    #[cfg(windows)]
    let change_time = proof.change_time;
    drop(proof.file);
    // All old read handles are closed before acquiring Windows cleanup access.
    let cleanup = open_regular_file_no_follow_for_cleanup(&path)?;
    let metadata = fs::symlink_metadata(&path)?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || file_identity(&cleanup)? != identity
        || path_identity(&path)? != identity
        || !state.matches(&cleanup.metadata()?)
        || !state.matches(&metadata)
    {
        return Err(invalid(
            "retired cleanup handle does not bind the verified bytes",
        ));
    }
    #[cfg(windows)]
    if crate::filesystem_identity::path_change_time(&path)? != change_time {
        return Err(invalid(
            "retired file changed while acquiring cleanup access",
        ));
    }
    let ProofSlot::Verified(installed) = &owner.bindings.proofs.replacement else {
        return Err(invalid("installed proof disappeared"));
    };
    if !proof_is_current(installed, &target)? {
        return Err(invalid("installed output changed before old-file deletion"));
    }
    remove_readonly_file(&path, &cleanup)?;
    sync_bindings(&owner.bindings)
}
pub(crate) fn remove_readonly_file(path: &Path, file: &std::fs::File) -> Result<(), FormatError> {
    #[cfg(windows)]
    {
        let permissions = file.metadata()?.permissions();
        if permissions.readonly() {
            let mut writable = permissions.clone();
            writable.set_readonly(false);
            file.set_permissions(writable)?;
            if let Err(error) = fs::remove_file(path) {
                file.set_permissions(permissions)?;
                return Err(error.into());
            }
            return Ok(());
        }
    }
    #[cfg(not(windows))]
    let _ = file;
    fs::remove_file(path)?;
    Ok(())
}

pub(crate) fn remove_empty_holder(owner: &mut Replacement<'_>) -> Result<(), FormatError> {
    let holder_path = owner.bindings.layout.holder(&owner.bindings.scope);
    let isolation = owner
        .bindings
        .layout
        .holder_isolation(&owner.bindings.scope);
    let expected = owner.bindings.layout.record().holder_identity;
    let original = identity_at(&holder_path)?;
    let isolated = identity_at(&isolation)?;
    let path = match (original, isolated) {
        (None, None) => {
            owner.bindings.holder = HolderProof::Missing;
            return Ok(());
        }
        (Some(identity), None) if identity == expected => holder_path.clone(),
        (None, Some(identity)) if identity == expected => isolation.clone(),
        _ => {
            return Err(invalid(
                "empty holder isolation has unexpected paths or identity",
            ))
        }
    };
    owner.record.verify()?;
    owner.bindings.scope.verify_locked()?;
    let holder = match std::mem::replace(&mut owner.bindings.holder, HolderProof::Missing) {
        HolderProof::Held(holder) if holder.path == path => holder,
        _ => HeldDirectory::open(path, expected)?,
    };
    clear_empty_directory(holder, &owner.bindings.scope, &isolation)
}

pub(crate) fn clear_empty_directory(
    holder: HeldDirectory,
    scope: &super::scope::Scope,
    isolation: &Path,
) -> Result<(), FormatError> {
    scope.verify_locked()?;
    holder.sync()?;
    require_empty(&holder)?;
    if holder.path != isolation {
        if identity_at(isolation)?.is_some() {
            return Err(invalid("empty holder isolation is occupied"));
        }
        let moved = super::move_path_no_replace(&holder.path, isolation);
        let after = identity_at(isolation)?;
        if let Some(unexpected) = after.filter(|identity| *identity != holder.identity) {
            scope.restore_known(isolation, &holder.path, unexpected)?;
            return Err(invalid(
                "empty-holder move selected a changed source; competitor was preserved",
            ));
        }
        moved?;
        scope.sync()?;
    }
    let relocated = HeldDirectory {
        path: isolation.to_path_buf(),
        file: holder.file,
        identity: holder.identity,
    };
    require_empty(&relocated)?;
    relocated.sync()?;
    let expected = relocated.identity;
    drop(relocated);
    if identity_at(isolation)? != Some(expected) || fs::read_dir(isolation)?.next().is_some() {
        return Err(invalid("empty holder changed immediately before removal"));
    }
    fs::remove_dir(isolation)?;
    scope.sync()
}
fn require_empty(holder: &HeldDirectory) -> Result<(), FormatError> {
    holder.verify()?;
    if fs::read_dir(&holder.path)?.next().is_some() {
        return Err(invalid("completed holder is not empty"));
    }
    holder.verify()
}
