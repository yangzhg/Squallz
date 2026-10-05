use std::fs;

use crate::api::{EntryPath, FormatError, ProgressSink};
use crate::destination_guard::{path_state_digest, verify_path_state_digest};
use crate::filesystem_identity::{file_identity, path_identity, RegularFileState};

use super::evidence::{
    hash_payload, invalid, EntryProof, PathEvidence, PayloadEvidence, ProofSlot, RegularProof,
    VerificationContext,
};
use super::model::*;
use super::scope::{canonical_requested, HeldDirectory, Scope};

pub(crate) fn prepare(
    scope: Scope,
    previous: OwnedInput,
    replacement: OwnedInput,
    authorization: Authorization,
    context: &VerificationContext<'_>,
) -> Result<Prepared, Box<PrepareFailure>> {
    let mut holder = HolderReservation::Unreserved;
    let work = (|| {
        scope.verify_locked()?;
        if scope.existing_record()?.is_some() {
            return Err(invalid("replacement record already exists"));
        }
        if canonical_requested(previous.path())? != scope.target_path()
            || crate::parent_or_current(&canonical_requested(replacement.path())?)
                != scope.parent_path()
            || !scope.reserved_name(Name::from_path(replacement.path())?.os(), "stage")
            || previous.identity() == replacement.identity()
        {
            return Err(invalid(
                "replacement inputs are not the owned target and private sibling stage",
            ));
        }
        sync_new(&replacement)?;
        let ((old_evidence, old_proof), (new_evidence, new_proof)) = match (&previous, &replacement)
        {
            (OwnedInput::Archive { state: old, .. }, OwnedInput::Archive { state: new, .. }) => {
                let total = old.bytes().saturating_add(new.bytes());
                let bind_with_offset = |input: &OwnedInput, completed_before| {
                    let progress = ArchiveVerificationProgress {
                        sink: context.progress,
                        completed_before,
                        total,
                    };
                    bind(
                        input,
                        &VerificationContext {
                            progress: &progress,
                            ..*context
                        },
                    )
                };
                let previous = bind_with_offset(&previous, 0)?;
                let replacement = bind_with_offset(&replacement, old.bytes())?;
                context
                    .progress
                    .on_progress(total, total, context.public_label);
                (previous, replacement)
            }
            _ => (bind(&previous, context)?, bind(&replacement, context)?),
        };
        if let (
            BoundEvidence::File(previous) | BoundEvidence::Tree(previous),
            Authorization::Guarded { digest },
        ) = (&old_evidence, &authorization)
        {
            if previous.digest != *digest {
                return Err(FormatError::destination_changed(scope.requested_path()));
            }
        }
        let evidence = match (old_evidence, new_evidence, authorization) {
            (
                BoundEvidence::Payload(previous),
                BoundEvidence::Payload(replacement),
                authorization,
            ) if !scope.is_sfx() => PairEvidence::Archive {
                previous,
                replacement,
                authorization,
            },
            (BoundEvidence::File(previous), BoundEvidence::File(replacement), _)
                if scope.is_sfx() =>
            {
                PairEvidence::SfxFile {
                    previous,
                    replacement,
                }
            }
            (BoundEvidence::Tree(previous), BoundEvidence::Tree(replacement), _)
                if scope.is_sfx() =>
            {
                PairEvidence::SfxTree {
                    previous,
                    replacement,
                }
            }
            _ => {
                return Err(invalid(
                    "replacement requires one concrete matching evidence route",
                ))
            }
        };
        verify_authorization(&evidence, previous.path(), &scope.target_path())?;
        let path = reserve_holder(&scope)?;
        holder = HolderReservation::Unidentified { path: path.clone() };
        let identity = path_identity(&path)?;
        let held = HeldDirectory::open(path.clone(), identity)?;
        holder = HolderReservation::Held(held);
        scope.sync()?;
        let record = ReplacementRecord {
            version: 1,
            parent_identity: scope.parent.identity,
            requested: scope.requested.clone(),
            target: scope.target.clone(),
            stage: Name::from_path(replacement.path())?,
            holder: Name::from_path(&path)?,
            holder_identity: identity,
            evidence,
        };
        let layout = RecordLayout::validate(record, &scope)?;
        if !super::evidence::proof_is_current(&old_proof, previous.path())?
            || !super::evidence::proof_is_current(&new_proof, replacement.path())?
        {
            return Err(invalid("replacement input changed before record issuance"));
        }
        verify_authorization(
            &layout.record().evidence,
            previous.path(),
            &scope.target_path(),
        )?;
        Ok((
            layout,
            ProofPair {
                previous: ProofSlot::Verified(old_proof),
                replacement: ProofSlot::Verified(new_proof),
            },
        ))
    })();
    match work {
        Ok((layout, proofs)) => match holder {
            HolderReservation::Held(holder) => Ok(Prepared {
                bindings: Bindings {
                    layout,
                    scope,
                    holder: HolderProof::Held(holder),
                    proofs,
                },
            }),
            holder => Err(Box::new(PrepareFailure {
                scope,
                previous,
                replacement,
                holder,
                error: invalid("prepared holder was not retained"),
            })),
        },
        Err(error) => Err(Box::new(PrepareFailure {
            scope,
            previous,
            replacement,
            holder,
            error,
        })),
    }
}

/// Fresh archive verification reports both payloads as one continuous phase.
/// Individual recovery reads retain their original per-file progress contract.
struct ArchiveVerificationProgress<'a> {
    sink: &'a dyn ProgressSink,
    completed_before: u64,
    total: u64,
}

impl ProgressSink for ArchiveVerificationProgress<'_> {
    fn on_progress(&self, done: u64, _total: u64, current: &EntryPath) {
        self.sink.on_progress(
            self.completed_before.saturating_add(done),
            self.total,
            current,
        );
    }
}

enum BoundEvidence {
    Payload(PayloadEvidence),
    File(PathEvidence),
    Tree(PathEvidence),
}
fn bind(
    input: &OwnedInput,
    context: &VerificationContext<'_>,
) -> Result<(BoundEvidence, EntryProof), FormatError> {
    let metadata = fs::symlink_metadata(input.path())?;
    let (held, expected_type) = match input {
        OwnedInput::Archive { file, state, .. } => {
            if !state.matches(&metadata) || !state.matches(&file.metadata()?) {
                return Err(invalid("saved archive input state changed"));
            }
            (file, metadata.is_file())
        }
        OwnedInput::SfxFile { file, .. } => (file, metadata.is_file()),
        OwnedInput::SfxTree { root, .. } => (root, metadata.is_dir()),
    };
    if metadata.file_type().is_symlink()
        || !expected_type
        || file_identity(held)? != input.identity()
        || path_identity(input.path())? != input.identity()
    {
        return Err(invalid(
            "writer/source handle is no longer bound to the owned input",
        ));
    }
    context.control.checkpoint()?;
    if matches!(input, OwnedInput::SfxTree { .. }) {
        let digest = path_state_digest(input.path())?
            .ok_or_else(|| invalid("SFX tree disappeared during binding"))?;
        let proof = EntryProof::Tree {
            root: held.try_clone()?,
            identity: input.identity(),
            digest,
        };
        if !super::evidence::proof_is_current(&proof, input.path())? {
            return Err(invalid("SFX tree changed during binding"));
        }
        return Ok((
            BoundEvidence::Tree(PathEvidence {
                identity: input.identity(),
                digest,
            }),
            proof,
        ));
    }
    let state = RegularFileState::from_metadata(&held.metadata()?);
    #[cfg(windows)]
    let change_time = crate::filesystem_identity::path_change_time(input.path())?;
    let digest = match input {
        OwnedInput::Archive { .. } => hash_payload(held, &state, context)?,
        OwnedInput::SfxFile { .. } => path_state_digest(input.path())?
            .ok_or_else(|| invalid("SFX file disappeared during binding"))?,
        OwnedInput::SfxTree { .. } => return Err(invalid("tree entered regular binding")),
    };
    let proof = RegularProof {
        file: held.try_clone()?,
        identity: input.identity(),
        state: state.clone(),
        #[cfg(windows)]
        change_time,
    };
    if !super::evidence::regular_is_current(&proof, input.path())? {
        return Err(invalid("input content changed during binding"));
    }
    let evidence = match input {
        OwnedInput::Archive { state, .. } => BoundEvidence::Payload(PayloadEvidence {
            identity: input.identity(),
            state: state.clone(),
            digest,
        }),
        _ => BoundEvidence::File(PathEvidence {
            identity: input.identity(),
            digest,
        }),
    };
    Ok((evidence, EntryProof::Regular(proof)))
}
fn sync_new(input: &OwnedInput) -> Result<(), FormatError> {
    #[cfg(windows)]
    if matches!(input, OwnedInput::SfxFile { .. }) {
        // The producer synced its writer before handing off the sealed read handle.
        return Ok(());
    }
    let file = match input {
        OwnedInput::Archive { file, .. } | OwnedInput::SfxFile { file, .. } => file,
        OwnedInput::SfxTree { root, .. } => root,
    };
    file.sync_all()?;
    Ok(())
}
pub(crate) fn verify_authorization(
    evidence: &PairEvidence,
    path: &std::path::Path,
    reported: &std::path::Path,
) -> Result<(), FormatError> {
    if let PairEvidence::Archive {
        authorization: Authorization::Guarded { digest },
        ..
    } = evidence
    {
        verify_path_state_digest(*digest, path, reported)?;
    }
    Ok(())
}
fn reserve_holder(scope: &Scope) -> Result<std::path::PathBuf, FormatError> {
    for _ in 0..1000 {
        let path = scope.reserve_name("holder");
        #[cfg(unix)]
        let mut builder = fs::DirBuilder::new();
        #[cfg(not(unix))]
        let builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        match builder.create(&path) {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Err(invalid("could not reserve private replacement holder"))
}

/// Used only on unpublished errors. Returns every path whose ownership could
/// not be discharged; the product still owns its stage cleanup contract.
pub(crate) fn abandon_holder(
    reservation: HolderReservation,
    scope: &Scope,
) -> Result<(), FormatError> {
    match reservation {
        HolderReservation::Unreserved => Ok(()),
        HolderReservation::Unidentified { .. } => {
            Err(invalid("unidentified holder must be retained"))
        }
        HolderReservation::Held(holder) => {
            let isolation = Name::from_path(&holder.path)?
                .suffixed(".empty-isolation")
                .join(scope.parent_path());
            super::moves::clear_empty_directory(holder, scope, &isolation)
        }
    }
}

pub(crate) fn discard_archive_stage(input: OwnedInput) -> Result<(), FormatError> {
    let OwnedInput::Archive {
        path,
        file,
        identity,
        state,
    } = input
    else {
        return Err(invalid("archive stage cleanup received an SFX input"));
    };
    let metadata = fs::symlink_metadata(&path)?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || file_identity(&file)? != identity
        || path_identity(&path)? != identity
        || !state.matches(&metadata)
        || !state.matches(&file.metadata()?)
    {
        return Err(invalid(
            "archive staging changed before cleanup and was retained",
        ));
    }
    super::moves::remove_readonly_file(&path, &file)?;
    crate::sync_directory(crate::parent_or_current(&path))?;
    Ok(())
}
