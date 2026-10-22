use std::fs::{self, File, Metadata};
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::api::{ControlToken, EntryPath, FormatError, ProgressSink};
use crate::destination_guard::path_state_digest;
use crate::filesystem_identity::{
    file_identity, open_regular_file_no_follow, path_identity, PathIdentity, RegularFileState,
};

pub(crate) type Digest = [u8; 32];

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PayloadEvidence {
    pub identity: PathIdentity,
    pub state: RegularFileState,
    pub digest: Digest,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PathEvidence {
    pub identity: PathIdentity,
    pub digest: Digest,
}

/// Expected identity and content evidence borrowed from the replacement record.
#[derive(Clone, Copy)]
pub(crate) enum ExpectedEntry<'a> {
    Payload(&'a PayloadEvidence),
    SfxFile(&'a PathEvidence),
    SfxTree(&'a PathEvidence),
}

impl ExpectedEntry<'_> {
    pub(crate) fn identity(self) -> PathIdentity {
        match self {
            Self::Payload(evidence) => evidence.identity,
            Self::SfxFile(evidence) | Self::SfxTree(evidence) => evidence.identity,
        }
    }

    fn state_matches(self, metadata: &Metadata, point: StatePoint) -> bool {
        match (self, point) {
            (Self::Payload(evidence), StatePoint::BeforeMove) => evidence.state.matches(metadata),
            (Self::Payload(evidence), StatePoint::AfterRename) => {
                metadata.is_file()
                    && evidence
                        .state
                        .equivalent_after_rename(&RegularFileState::from_metadata(metadata))
            }
            (Self::SfxFile(_), _) => metadata.is_file(),
            (Self::SfxTree(_), _) => metadata.is_dir(),
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) enum StatePoint {
    BeforeMove,
    AfterRename,
}

pub(crate) struct RegularProof {
    pub file: File,
    pub identity: PathIdentity,
    pub state: RegularFileState,
    #[cfg(windows)]
    pub change_time: i64,
}

pub(crate) enum EntryProof {
    Regular(RegularProof),
    Tree {
        root: File,
        identity: PathIdentity,
        digest: Digest,
    },
}

pub(crate) enum ProofSlot {
    Unverified,
    Verified(EntryProof),
}

pub(crate) enum Observation {
    Missing,
    Expected(EntryProof),
    Changed,
}

pub(crate) struct VerificationContext<'a> {
    pub public_label: &'a EntryPath,
    pub progress: &'a dyn ProgressSink,
    pub control: &'a ControlToken,
}

/// Only Archive's existing exact held-file byte cache is reusable. SFX always
/// verifies the full path state; a payload metadata cache is not its authority.
pub(crate) fn observe(
    path: &Path,
    expected: ExpectedEntry<'_>,
    point: StatePoint,
    retained: &ProofSlot,
    force_content: bool,
    context: &VerificationContext<'_>,
) -> Result<Observation, FormatError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Observation::Missing)
        }
        Err(error) => return Err(error.into()),
    };
    let identity = path_identity(path)?;
    if identity != expected.identity()
        || metadata.file_type().is_symlink()
        || !expected.state_matches(&metadata, point)
    {
        return Ok(Observation::Changed);
    }
    context.control.checkpoint()?;
    if let ExpectedEntry::SfxTree(evidence) = expected {
        let root = match retained {
            ProofSlot::Verified(EntryProof::Tree {
                root,
                identity: held,
                ..
            }) if *held == identity && file_identity(root)? == identity => root.try_clone()?,
            ProofSlot::Verified(_) => return Ok(Observation::Changed),
            _ => crate::open_directory_no_follow(path)?,
        };
        if file_identity(&root)? != identity
            || path_state_digest(path)? != Some(evidence.digest)
            || file_identity(&root)? != identity
            || path_identity(path)? != identity
        {
            return Ok(Observation::Changed);
        }
        return Ok(Observation::Expected(EntryProof::Tree {
            root,
            identity,
            digest: evidence.digest,
        }));
    }

    let file = match retained {
        ProofSlot::Verified(EntryProof::Regular(proof))
            if proof.identity == identity && file_identity(&proof.file)? == identity =>
        {
            if matches!(expected, ExpectedEntry::Payload(_))
                && !force_content
                && regular_is_current(proof, path)?
            {
                return Ok(Observation::Expected(EntryProof::Regular(RegularProof {
                    file: proof.file.try_clone()?,
                    identity,
                    state: proof.state.clone(),
                    #[cfg(windows)]
                    change_time: proof.change_time,
                })));
            }
            proof.file.try_clone()?
        }
        ProofSlot::Verified(_) => return Ok(Observation::Changed),
        _ => open_regular_file_no_follow(path)?,
    };
    let state = RegularFileState::from_metadata(&file.metadata()?);
    #[cfg(windows)]
    let change_time = crate::filesystem_identity::path_change_time(path)?;
    if file_identity(&file)? != identity
        || path_identity(path)? != identity
        || !expected.state_matches(&file.metadata()?, point)
        || !state.matches(&fs::symlink_metadata(path)?)
    {
        return Ok(Observation::Changed);
    }
    let content_matches = match expected {
        ExpectedEntry::Payload(evidence) => {
            hash_payload(&file, &state, context)? == evidence.digest
        }
        ExpectedEntry::SfxFile(evidence) => path_state_digest(path)? == Some(evidence.digest),
        ExpectedEntry::SfxTree(_) => {
            return Err(invalid("tree proof entered regular verification"))
        }
    };
    let proof = RegularProof {
        file,
        identity,
        state,
        #[cfg(windows)]
        change_time,
    };
    if !content_matches || !regular_is_current(&proof, path)? {
        return Ok(Observation::Changed);
    }
    Ok(Observation::Expected(EntryProof::Regular(proof)))
}

pub(crate) fn regular_is_current(proof: &RegularProof, path: &Path) -> Result<bool, FormatError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    #[cfg(windows)]
    let change_time_matches =
        crate::filesystem_identity::path_change_time(path)? == proof.change_time;
    #[cfg(not(windows))]
    let change_time_matches = true;
    Ok(!metadata.file_type().is_symlink()
        && metadata.is_file()
        && file_identity(&proof.file)? == proof.identity
        && path_identity(path)? == proof.identity
        && proof.state.matches(&proof.file.metadata()?)
        && proof.state.matches(&metadata)
        && change_time_matches)
}

pub(crate) fn proof_identity_is_current(
    proof: &EntryProof,
    path: &Path,
    expected: PathIdentity,
) -> Result<bool, FormatError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    let (file, identity, shape) = match proof {
        EntryProof::Regular(regular) => (&regular.file, regular.identity, metadata.is_file()),
        EntryProof::Tree { root, identity, .. } => (root, *identity, metadata.is_dir()),
    };
    Ok(!metadata.file_type().is_symlink()
        && shape
        && identity == expected
        && file_identity(file)? == expected
        && path_identity(path)? == expected)
}

pub(crate) fn proof_is_current(proof: &EntryProof, path: &Path) -> Result<bool, FormatError> {
    match proof {
        EntryProof::Regular(proof) => regular_is_current(proof, path),
        EntryProof::Tree {
            identity, digest, ..
        } => Ok(proof_identity_is_current(proof, path, *identity)?
            && path_state_digest(path)? == Some(*digest)
            && proof_identity_is_current(proof, path, *identity)?),
    }
}

pub(crate) fn hash_payload(
    file: &File,
    state: &RegularFileState,
    context: &VerificationContext<'_>,
) -> Result<Digest, FormatError> {
    let mut reader = file.try_clone()?;
    reader.seek(SeekFrom::Start(0))?;
    let mut buffer = vec![0; 256 * 1024];
    let mut hasher = blake3::Hasher::new();
    let mut bytes = 0u64;
    context
        .progress
        .on_progress(0, state.bytes(), context.public_label);
    loop {
        context.control.checkpoint()?;
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        bytes = bytes.saturating_add(read as u64);
        hasher.update(&buffer[..read]);
        context
            .progress
            .on_progress(bytes, state.bytes(), context.public_label);
    }
    if bytes != state.bytes() || !state.matches(&file.metadata()?) {
        return Err(invalid("replacement payload changed during verification"));
    }
    Ok(*hasher.finalize().as_bytes())
}

pub(crate) fn invalid(reason: &str) -> FormatError {
    FormatError::Io(std::io::Error::other(reason.to_owned()))
}
