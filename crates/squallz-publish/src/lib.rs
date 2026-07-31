//! Platform publishing workflows shared by the desktop app and CLI.

use std::path::{Path, PathBuf};

use squallz_core::api::{ControlToken, FormatError, ProgressSink, ResourceOptions};
use squallz_core::SfxInfo;

#[cfg(target_os = "macos")]
mod macos;

/// User-visible stages of the macOS signing and notarization workflow.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MacosSfxPublishPhase {
    Verify,
    Sign,
    Notarize,
    Finalize,
    Commit,
}

/// Verified evidence returned after a macOS SFX is published.
#[derive(Debug)]
pub struct MacosSfxPublishReport {
    pub source: PathBuf,
    pub output: PathBuf,
    pub info: SfxInfo,
    pub team_id: String,
    pub submission_id: String,
}

/// Lists valid Developer ID Application identities visible to `codesign`.
pub fn macos_signing_identities() -> Result<Vec<String>, FormatError> {
    #[cfg(target_os = "macos")]
    {
        macos::signing_identities()
    }

    #[cfg(not(target_os = "macos"))]
    {
        Err(FormatError::Unsupported(
            "Developer ID identities are only available on macOS".into(),
        ))
    }
}

/// Publishes a separate Developer ID-signed and notarized macOS SFX app.
#[allow(clippy::too_many_arguments)]
pub fn publish_macos_sfx(
    ctl: &ControlToken,
    source: &Path,
    output: &Path,
    identity: &str,
    notary_profile: &str,
    resources: &ResourceOptions,
    preflight_progress: &dyn ProgressSink,
    final_progress: &dyn ProgressSink,
    phase: &mut dyn FnMut(MacosSfxPublishPhase),
) -> Result<MacosSfxPublishReport, FormatError> {
    #[cfg(target_os = "macos")]
    {
        macos::publish(
            ctl,
            source,
            output,
            identity,
            notary_profile,
            resources,
            preflight_progress,
            final_progress,
            phase,
        )
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = (
            ctl,
            source,
            output,
            identity,
            notary_profile,
            resources,
            preflight_progress,
            final_progress,
            phase,
        );
        Err(FormatError::Unsupported(
            "macOS SFX publishing must run on macOS with Xcode command-line tools".into(),
        ))
    }
}
