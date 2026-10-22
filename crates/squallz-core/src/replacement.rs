//! Durable Archive/SFX replacement with one installation and recovery owner.

mod evidence;
mod model;
mod moves;
mod owner;
mod prepare;
mod record;
mod scope;

#[cfg(test)]
mod archive_tests;
#[cfg(test)]
pub(crate) mod test_hooks;

#[cfg(windows)]
pub(crate) use evidence::hash_payload;
pub(crate) use evidence::{regular_is_current, RegularProof, VerificationContext};
pub(crate) use model::{
    ActiveWriter, ArtifactRole, Authorization, Failure, Outcome, OwnedInput, PersistFailure,
    Visibility,
};
#[cfg(test)]
pub(crate) use model::{Installed, Replacement};
pub(crate) use owner::{
    classify_sfx_artifact, commit_resume, complete, discard_prepare, discard_unpublished, recover,
    Recovery,
};
pub(crate) use prepare::{discard_archive_stage, prepare};
pub(crate) use record::persist;
pub(crate) use scope::{canonical_requested, RecordPhase, Scope};

pub(crate) fn move_path_no_replace(
    from: &std::path::Path,
    to: &std::path::Path,
) -> std::io::Result<()> {
    #[cfg(test)]
    test_hooks::emit(test_hooks::Event::BeforeMove {
        to: to.to_path_buf(),
    })?;
    #[cfg(test)]
    test_hooks::move_path(from, to)?;
    #[cfg(not(test))]
    crate::move_path_no_replace(from, to)?;
    #[cfg(test)]
    test_hooks::emit(test_hooks::Event::AfterMove {
        from: from.to_path_buf(),
        to: to.to_path_buf(),
    })?;
    Ok(())
}
