use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use crate::api::FormatError;
use crate::filesystem_identity::{
    file_identity, open_regular_file_no_follow, path_identity, PathIdentity, RegularFileState,
};

use super::evidence::{invalid, regular_is_current, RegularProof};
use super::model::{
    ActiveWriter, ArtifactRole, Failure, PersistFailure, Prepared, Replacement, ReplacementRecord,
    Visibility,
};
use super::scope::{identity_at, open_new_artifact, RecordPhase, Scope};

enum ContentProof {
    PayloadBytes {
        state: RegularFileState,
        bytes: Vec<u8>,
    },
    BoundPathState {
        digest: [u8; 32],
    },
}
pub(crate) struct BoundRecord {
    pub path: PathBuf,
    pub file: File,
    pub identity: PathIdentity,
    proof: ContentProof,
    limit: usize,
}
pub(crate) struct RecordFailure {
    pub error: FormatError,
    pub paths: Vec<PathBuf>,
    pub published: bool,
}

pub(crate) fn read(
    path: &Path,
    scope: &Scope,
) -> Result<(BoundRecord, ReplacementRecord), FormatError> {
    scope.phase(path)?;
    let file = open_regular_file_no_follow(path)?;
    let identity = file_identity(&file)?;
    let (read_proof, bytes) = read_verified(&file, path, identity, scope.limit())?;
    let state = read_proof.state;
    let record = serde_json::from_slice(&bytes)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    let bound = BoundRecord::new(path.to_path_buf(), file, identity, state, bytes, scope);
    Ok((bound, record))
}

impl BoundRecord {
    fn new(
        path: PathBuf,
        file: File,
        identity: PathIdentity,
        state: RegularFileState,
        bytes: Vec<u8>,
        scope: &Scope,
    ) -> Self {
        let proof = if scope.is_sfx() {
            ContentProof::BoundPathState {
                digest: *blake3::hash(&bytes).as_bytes(),
            }
        } else {
            ContentProof::PayloadBytes { state, bytes }
        };
        Self {
            path,
            file,
            identity,
            proof,
            limit: scope.limit(),
        }
    }
    pub(crate) fn verify(&self) -> Result<(), FormatError> {
        self.verified_read(false).map(|_| ())
    }
    pub(crate) fn removal_proof(&self) -> Result<RegularProof, FormatError> {
        self.verified_read(false)
    }
    fn verified_read(&self, after_rename: bool) -> Result<RegularProof, FormatError> {
        require_identity(&self.file, &self.path, self.identity)?;
        if let ContentProof::PayloadBytes { state, .. } = &self.proof {
            if !after_rename
                && (!state.matches(&self.file.metadata()?)
                    || !state.matches(&fs::symlink_metadata(&self.path)?))
            {
                return Err(invalid(
                    "saved record state changed before content verification",
                ));
            }
        }
        let (proof, bytes) = read_verified(&self.file, &self.path, self.identity, self.limit)?;
        let matches = match &self.proof {
            ContentProof::PayloadBytes {
                bytes: expected, ..
            } => bytes == *expected,
            ContentProof::BoundPathState { digest } => *blake3::hash(&bytes).as_bytes() == *digest,
        };
        if !matches || !regular_is_current(&proof, &self.path)? {
            return Err(invalid("record identity or exact content changed"));
        }
        Ok(proof)
    }
    fn refresh(&mut self) -> Result<(), FormatError> {
        let proof = self.verified_read(true)?;
        if let ContentProof::PayloadBytes { state, .. } = &mut self.proof {
            *state = proof.state;
        }
        self.verify()
    }
    fn bind_partial_write(&mut self, planned: &[u8]) -> Result<(), FormatError> {
        let (proof, bytes) = read_verified(&self.file, &self.path, self.identity, self.limit)?;
        if !planned.starts_with(&bytes) {
            return Err(invalid("partial record is not the writer's byte prefix"));
        }
        let before = proof.state;
        match &mut self.proof {
            ContentProof::PayloadBytes {
                state,
                bytes: expected,
            } => {
                *state = before;
                *expected = bytes;
            }
            ContentProof::BoundPathState { digest } => *digest = *blake3::hash(&bytes).as_bytes(),
        }
        self.verify()
    }
}

pub(crate) fn persist(prepared: Prepared) -> Result<Replacement<'static>, PersistFailure> {
    let scope = &prepared.bindings.scope;
    let bytes = match serde_json::to_vec(prepared.bindings.layout.record()) {
        Ok(bytes) if bytes.len() <= scope.limit() => bytes,
        Ok(_) => {
            return Err(PersistFailure::Unpublished {
                error: size_error(scope.limit()),
                prepared: Box::new(prepared),
            })
        }
        Err(error) => {
            return Err(PersistFailure::Unpublished {
                error: io::Error::new(io::ErrorKind::InvalidData, error).into(),
                prepared: Box::new(prepared),
            })
        }
    };
    if let Err(error) = scope.verify_locked().and_then(|_| {
        if scope.existing_record()?.is_some() {
            Err(invalid("durable record appeared before issuance"))
        } else {
            Ok(())
        }
    }) {
        return Err(PersistFailure::Unpublished {
            prepared: Box::new(prepared),
            error,
        });
    }
    let mut reservation = None;
    for _ in 0..1000 {
        let path = scope.reserve_name("journal");
        match open_new_artifact(&path) {
            Ok(file) => {
                reservation = Some((path, file));
                break;
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(PersistFailure::Unpublished {
                    prepared: Box::new(prepared),
                    error: error.into(),
                })
            }
        }
    }
    let Some((path, mut file)) = reservation else {
        return Err(PersistFailure::Unpublished {
            prepared: Box::new(prepared),
            error: invalid("could not reserve bounded replacement record"),
        });
    };
    let identity = match file_identity(&file) {
        Ok(identity) => identity,
        Err(error) => {
            return Err(PersistFailure::Retained(retained(
                &prepared,
                error.into(),
                [path],
            )))
        }
    };
    let write = file.write_all(&bytes).and_then(|_| file.sync_all());
    let state = match file.metadata() {
        Ok(metadata) => RegularFileState::from_metadata(&metadata),
        Err(error) => {
            return Err(PersistFailure::Retained(retained(
                &prepared,
                error.into(),
                [path],
            )))
        }
    };
    let planned = bytes.clone();
    let mut record = BoundRecord::new(path.clone(), file, identity, state, bytes, scope);
    let initial = match write {
        Ok(()) => record.verify(),
        Err(error) => {
            if let Err(binding) = record.bind_partial_write(&planned) {
                return Err(PersistFailure::Retained(retained(
                    &prepared,
                    invalid(&format!(
                        "{error}; partial record could not be bound: {binding}"
                    )),
                    [path],
                )));
            }
            Err(error.into())
        }
    };
    if let Err(error) = initial {
        return match discard_unpublished(&mut record, scope) {
            Ok(()) => Err(PersistFailure::Unpublished {
                prepared: Box::new(prepared),
                error,
            }),
            Err(cleanup) => {
                let failure = retained(
                    &prepared,
                    invalid(&format!(
                        "{error}; temporary record cleanup failed: {}",
                        cleanup.error
                    )),
                    cleanup.paths,
                );
                Err(PersistFailure::Retained(failure))
            }
        };
    }
    let first = if scope.is_sfx() {
        RecordPhase::Installing
    } else {
        RecordPhase::Pending
    };
    if let Err(error) = move_record(&mut record, &scope.anchor(first), scope) {
        if !error.published {
            return match discard_unpublished(&mut record, scope) {
                Ok(()) => Err(PersistFailure::Unpublished {
                    prepared: Box::new(prepared),
                    error: error.error,
                }),
                Err(cleanup) => Err(PersistFailure::Retained(retained(
                    &prepared,
                    invalid(&format!(
                        "{}; record cleanup failed: {}",
                        error.error, cleanup.error
                    )),
                    error.paths.into_iter().chain(cleanup.paths),
                ))),
            };
        }
        return Err(PersistFailure::Retained(retained(
            &prepared,
            error.error,
            error.paths,
        )));
    }
    if !scope.is_sfx() {
        if let Err(error) = move_record(&mut record, &scope.anchor(RecordPhase::Installing), scope)
        {
            return Err(PersistFailure::Retained(retained(
                &prepared,
                error.error,
                error.paths,
            )));
        }
    }
    Ok(Replacement {
        bindings: prepared.bindings,
        record,
        active: ActiveWriter::None,
        visibility: Visibility::RecordMayBeDurable,
    })
}

/// A successful rename updates the visible location before any fallible sync.
/// Restore admits the first post-move unexpected identity, never a later path occupant.
pub(crate) fn move_record(
    record: &mut BoundRecord,
    destination: &Path,
    scope: &Scope,
) -> Result<(), RecordFailure> {
    let source = record.path.clone();
    let mut published = false;
    let operation = (|| {
        scope.verify_locked()?;
        record.verify()?;
        if identity_at(destination)?.is_some() {
            return Err(invalid("record destination is occupied"));
        }
        let moved = super::move_path_no_replace(&source, destination);
        let observed = identity_at(destination);
        published = moved.is_ok()
            || observed.as_ref().is_err()
            || observed
                .as_ref()
                .is_ok_and(|identity| *identity == Some(record.identity));
        if moved.is_err()
            && observed
                .as_ref()
                .is_ok_and(|value| *value == Some(record.identity))
        {
            record.path = destination.to_path_buf();
        }
        if moved.is_ok() {
            record.path = destination.to_path_buf();
        }
        let first_destination = match observed {
            Ok(Some(identity)) if identity == record.identity => Some(identity),
            Ok(Some(unexpected)) => {
                if !scope.is_sfx() {
                    scope.restore_known(destination, &source, unexpected)?;
                }
                return Err(invalid("record move selected an unexpected source"));
            }
            Ok(None) => None,
            Err(error) => return Err(error),
        };
        moved?;
        scope.sync()?;
        let settled = (|| {
            if first_destination.is_none() || identity_at(&source)?.is_some() {
                return Err(invalid("record move left inconsistent paths"));
            }
            record.refresh()?;
            scope.verify_locked()
        })();
        if let Err(error) = settled {
            if let Some(observed) = first_destination.filter(|_| !scope.is_sfx()) {
                scope.restore_known(destination, &source, observed)?;
                if identity_at(&source)? == Some(record.identity) {
                    record.path = source.clone();
                }
            }
            return Err(error);
        }
        Ok(())
    })();
    operation.map_err(|error| RecordFailure {
        error,
        paths: vec![source, destination.to_path_buf()],
        published,
    })
}
pub(crate) fn clear(record: &mut BoundRecord, scope: &Scope) -> Result<(), RecordFailure> {
    if scope.phase(&record.path).ok() != Some(RecordPhase::Completed) {
        return Err(RecordFailure {
            error: invalid("only a completed record may be cleared"),
            paths: vec![record.path.clone()],
            published: true,
        });
    }
    unlink_verified(record, scope).map_err(|error| RecordFailure {
        error,
        paths: vec![record.path.clone()],
        published: true,
    })
}
fn discard_unpublished(record: &mut BoundRecord, scope: &Scope) -> Result<(), RecordFailure> {
    let mut name = record.path.as_os_str().to_os_string();
    name.push(".discard");
    let isolation = PathBuf::from(name);
    move_record(record, &isolation, scope)?;
    unlink_verified(record, scope).map_err(|error| RecordFailure {
        error,
        paths: vec![isolation],
        published: true,
    })
}
fn unlink_verified(record: &BoundRecord, scope: &Scope) -> Result<(), FormatError> {
    scope.verify_locked()?;
    let proof = record.removal_proof()?;
    // The proof's state came from the verified bounded read, not a later sample.
    if !regular_is_current(&proof, &record.path)? {
        return Err(invalid("record changed immediately before removal"));
    }
    fs::remove_file(&record.path)?;
    scope.sync()
}
fn retained(
    prepared: &Prepared,
    error: FormatError,
    paths: impl IntoIterator<Item = PathBuf>,
) -> Failure {
    let mut failure =
        super::owner::binding_failure(&prepared.bindings, error, Visibility::RecordMayBeDurable);
    for path in paths {
        failure.add(ArtifactRole::RecordTemporary, path);
    }
    failure
}
fn read_bounded(file: &File, limit: usize) -> Result<Vec<u8>, FormatError> {
    let mut reader = file.try_clone()?;
    reader.seek(SeekFrom::Start(0))?;
    let mut bytes = Vec::new();
    Read::by_ref(&mut reader)
        .take((limit + 1) as u64)
        .read_to_end(&mut bytes)?;
    Ok(bytes)
}
fn require_identity(file: &File, path: &Path, identity: PathIdentity) -> Result<(), FormatError> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || file_identity(file)? != identity
        || path_identity(path)? != identity
    {
        return Err(invalid("record path and held file are not bound"));
    }
    Ok(())
}
fn size_error(limit: usize) -> FormatError {
    let detail = format!("replacement record exceeds {limit} bytes");
    if limit == 16 * 1024 {
        FormatError::ResourceLimitExceeded(detail)
    } else {
        invalid(&detail)
    }
}
fn read_verified(
    file: &File,
    path: &Path,
    identity: PathIdentity,
    limit: usize,
) -> Result<(RegularProof, Vec<u8>), FormatError> {
    require_identity(file, path, identity)?;
    let state = RegularFileState::from_metadata(&file.metadata()?);
    #[cfg(windows)]
    let change_time = crate::filesystem_identity::path_change_time(path)?;
    let bytes = read_bounded(file, limit)?;
    if bytes.len() > limit {
        return Err(size_error(limit));
    }
    let proof = RegularProof {
        file: file.try_clone()?,
        identity,
        state,
        #[cfg(windows)]
        change_time,
    };
    if !regular_is_current(&proof, path)? {
        return Err(invalid("record changed during bounded content read"));
    }
    Ok((proof, bytes))
}
