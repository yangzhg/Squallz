use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::api::FormatError;
use crate::archive_path::checked_path_component;
use crate::filesystem_identity::{PathIdentity, RegularFileState};
use crate::stored_os_string::StoredOsString;

use super::evidence::{
    Digest, EntryProof, ExpectedEntry, PathEvidence, PayloadEvidence, ProofSlot,
};
use super::record::BoundRecord;
use super::scope::{HeldDirectory, Scope};

/// One lossless component is stored; serialized and runtime names do not coexist.
#[derive(Clone)]
pub(crate) struct Name(OsString);

impl Name {
    pub(crate) fn new(value: &OsStr) -> Result<Self, FormatError> {
        Ok(Self(checked_path_component(
            Some(value),
            "replacement component",
        )?))
    }
    pub(crate) fn from_path(path: &Path) -> Result<Self, FormatError> {
        Self::new(
            path.file_name()
                .ok_or_else(|| super::evidence::invalid("missing component"))?,
        )
    }
    pub(crate) fn os(&self) -> &OsStr {
        &self.0
    }
    pub(crate) fn join(&self, parent: &Path) -> PathBuf {
        parent.join(&self.0)
    }
    /// All callers append a fixed suffix without separators to a validated component.
    pub(crate) fn suffixed(&self, suffix: &'static str) -> Self {
        let mut name = self.0.clone();
        name.push(suffix);
        Self(name)
    }
}
impl Serialize for Name {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        StoredOsString::from_os_str(&self.0)
            .map_err(serde::ser::Error::custom)?
            .serialize(serializer)
    }
}
impl<'de> Deserialize<'de> for Name {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let stored = StoredOsString::deserialize(deserializer)?;
        let name = stored.to_os_string().map_err(serde::de::Error::custom)?;
        Self::new(&name).map_err(serde::de::Error::custom)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", deny_unknown_fields)]
pub(crate) enum Authorization {
    ContentBound,
    Guarded { digest: Digest },
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", deny_unknown_fields)]
pub(crate) enum PairEvidence {
    Archive {
        previous: PayloadEvidence,
        replacement: PayloadEvidence,
        authorization: Authorization,
    },
    SfxFile {
        previous: PathEvidence,
        replacement: PathEvidence,
    },
    SfxTree {
        previous: PathEvidence,
        replacement: PathEvidence,
    },
}
impl PairEvidence {
    pub(crate) fn previous(&self) -> ExpectedEntry<'_> {
        match self {
            Self::Archive { previous, .. } => ExpectedEntry::Payload(previous),
            Self::SfxFile { previous, .. } => ExpectedEntry::SfxFile(previous),
            Self::SfxTree { previous, .. } => ExpectedEntry::SfxTree(previous),
        }
    }
    pub(crate) fn replacement(&self) -> ExpectedEntry<'_> {
        match self {
            Self::Archive { replacement, .. } => ExpectedEntry::Payload(replacement),
            Self::SfxFile { replacement, .. } => ExpectedEntry::SfxFile(replacement),
            Self::SfxTree { replacement, .. } => ExpectedEntry::SfxTree(replacement),
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReplacementRecord {
    pub version: u32,
    pub parent_identity: PathIdentity,
    pub requested: Name,
    pub target: Name,
    pub stage: Name,
    pub holder: Name,
    pub holder_identity: PathIdentity,
    pub evidence: PairEvidence,
}

/// Validation is shared by fresh issuance and current-schema reopening.
pub(crate) struct RecordLayout {
    record: ReplacementRecord,
}
impl RecordLayout {
    pub(crate) fn validate(record: ReplacementRecord, scope: &Scope) -> Result<Self, FormatError> {
        scope.validate_record(&record)?;
        Ok(Self { record })
    }
    pub(crate) fn record(&self) -> &ReplacementRecord {
        &self.record
    }
    pub(crate) fn target(&self, scope: &Scope) -> PathBuf {
        self.record.target.join(scope.parent_path())
    }
    pub(crate) fn stage(&self, scope: &Scope) -> PathBuf {
        self.record.stage.join(scope.parent_path())
    }
    pub(crate) fn holder(&self, scope: &Scope) -> PathBuf {
        self.record.holder.join(scope.parent_path())
    }
    pub(crate) fn protected(&self, scope: &Scope) -> PathBuf {
        self.holder(scope).join("replacement")
    }
    pub(crate) fn previous(&self, scope: &Scope) -> PathBuf {
        self.holder(scope).join("previous")
    }
    pub(crate) fn retired(&self, scope: &Scope) -> PathBuf {
        self.holder(scope).join("retired")
    }
    pub(crate) fn holder_isolation(&self, scope: &Scope) -> PathBuf {
        self.record
            .holder
            .suffixed(".empty-isolation")
            .join(scope.parent_path())
    }
    pub(crate) fn alias(&self, scope: &Scope, pair: AliasPair) -> PathBuf {
        match pair {
            AliasPair::WriterAndProtected | AliasPair::WriterAndInstalled => self
                .record
                .stage
                .suffixed(".alias-isolation")
                .join(scope.parent_path()),
            AliasPair::TargetAndPrevious => self
                .record
                .holder
                .suffixed(".previous-alias")
                .join(scope.parent_path()),
            AliasPair::ProtectedAndInstalled => self.holder(scope).join("alias-replacement"),
        }
    }
    pub(crate) fn artifacts(&self, scope: &Scope) -> [(ArtifactRole, PathBuf); 10] {
        [
            (ArtifactRole::WriterStage, self.stage(scope)),
            (ArtifactRole::Protected, self.protected(scope)),
            (ArtifactRole::Target, self.target(scope)),
            (ArtifactRole::Previous, self.previous(scope)),
            (ArtifactRole::Retired, self.retired(scope)),
            (ArtifactRole::Holder, self.holder(scope)),
            (ArtifactRole::HolderIsolation, self.holder_isolation(scope)),
            (
                ArtifactRole::Alias,
                self.alias(scope, AliasPair::WriterAndProtected),
            ),
            (
                ArtifactRole::Alias,
                self.alias(scope, AliasPair::TargetAndPrevious),
            ),
            (
                ArtifactRole::Alias,
                self.alias(scope, AliasPair::ProtectedAndInstalled),
            ),
        ]
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum AliasPair {
    WriterAndProtected,
    WriterAndInstalled,
    TargetAndPrevious,
    ProtectedAndInstalled,
}

pub(crate) enum OwnedInput {
    Archive {
        path: PathBuf,
        file: File,
        identity: PathIdentity,
        state: RegularFileState,
    },
    /// A Windows staged replacement holds the original object after writer sync
    /// and a sealed readonly handoff; publication never flushes that read handle.
    SfxFile {
        path: PathBuf,
        file: File,
        identity: PathIdentity,
    },
    SfxTree {
        path: PathBuf,
        root: File,
        identity: PathIdentity,
    },
}
impl OwnedInput {
    pub(crate) fn path(&self) -> &Path {
        match self {
            Self::Archive { path, .. }
            | Self::SfxFile { path, .. }
            | Self::SfxTree { path, .. } => path,
        }
    }
    pub(crate) fn identity(&self) -> PathIdentity {
        match self {
            Self::Archive { identity, .. }
            | Self::SfxFile { identity, .. }
            | Self::SfxTree { identity, .. } => *identity,
        }
    }
}

pub(crate) struct ProofPair {
    pub previous: ProofSlot,
    pub replacement: ProofSlot,
}
pub(crate) enum HolderProof {
    Held(HeldDirectory),
    Missing,
}
pub(crate) struct Bindings {
    pub layout: RecordLayout,
    pub scope: Scope,
    pub holder: HolderProof,
    pub proofs: ProofPair,
}
impl Bindings {
    pub(crate) fn verify_authorization(&self, path: &Path) -> Result<(), FormatError> {
        super::prepare::verify_authorization(
            &self.layout.record().evidence,
            path,
            &self.layout.target(&self.scope),
        )
    }
}
pub(crate) struct Prepared {
    pub bindings: Bindings,
}
pub(crate) struct Replacement<'a> {
    pub bindings: Bindings,
    pub record: BoundRecord,
    pub active: ActiveWriter<'a>,
    pub visibility: Visibility,
}
pub(crate) struct Installed<'a> {
    pub owner: Replacement<'a>,
}
impl Installed<'_> {
    pub(crate) fn target(&self) -> PathBuf {
        self.owner
            .bindings
            .layout
            .target(&self.owner.bindings.scope)
    }
    pub(crate) fn regular_proof(&self) -> Option<&super::evidence::RegularProof> {
        match &self.owner.bindings.proofs.replacement {
            ProofSlot::Verified(EntryProof::Regular(proof)) => Some(proof),
            _ => None,
        }
    }
}

pub(crate) enum ActiveWriter<'a> {
    None,
    With(&'a OwnedInput),
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Visibility {
    Unpublished,
    RecordMayBeDurable,
    OutputMayBeVisible,
}
#[derive(Clone, Copy)]
pub(crate) enum ArtifactRole {
    Record,
    RecordTemporary,
    WriterStage,
    Protected,
    Target,
    Previous,
    Retired,
    Holder,
    HolderIsolation,
    Alias,
    ActiveWriter,
}
pub(crate) struct Artifact {
    pub role: ArtifactRole,
    pub path: PathBuf,
}
pub(crate) struct Failure {
    pub error: FormatError,
    pub visibility: Visibility,
    pub artifacts: Vec<Artifact>,
}
impl Failure {
    pub(crate) fn add(&mut self, role: ArtifactRole, path: PathBuf) {
        if !self.artifacts.iter().any(|item| item.path == path) {
            self.artifacts.push(Artifact { role, path });
        }
    }
}
pub(crate) enum PersistFailure {
    Unpublished {
        prepared: Box<Prepared>,
        error: FormatError,
    },
    Retained(Failure),
}
pub(crate) struct PrepareFailure {
    pub scope: Scope,
    pub previous: OwnedInput,
    pub replacement: OwnedInput,
    pub error: FormatError,
    pub holder: HolderReservation,
}
pub(crate) enum HolderReservation {
    Unreserved,
    Unidentified { path: PathBuf },
    Held(HeldDirectory),
}
pub(crate) enum Outcome {
    Archive { scope: Scope },
    Sfx { scope: Scope, backup: PathBuf },
    SfxCleared { scope: Scope },
}
