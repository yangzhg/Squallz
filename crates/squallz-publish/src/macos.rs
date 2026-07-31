use std::collections::HashSet;
use std::ffi::{OsStr, OsString};
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::Duration;

use serde_json::Value;
use squallz_core::api::{ControlToken, FormatError, ProgressSink, ResourceOptions};
use squallz_core::{
    inspect_create_destination_with_progress, move_path_no_replace, open_directory_no_follow,
    open_regular_file_no_follow, physical_file_identity, physical_path_identity,
    verify_sfx_payload, CreateArtifactKind, CreateDestinationGuard, SfxInfo, SfxTarget,
};

use crate::{MacosSfxPublishPhase as PublishPhase, MacosSfxPublishReport as PublishReport};

const TOOL_OUTPUT_LIMIT: u64 = 1024 * 1024;
const NOTARY_TIMEOUT: &str = "2h";
const PROCESS_POLL_INTERVAL: Duration = Duration::from_millis(50);
const RESERVATION_ATTEMPTS: u64 = 1024;
const CODESIGN_PATH: &str = "/usr/bin/codesign";
const DITTO_PATH: &str = "/usr/bin/ditto";
const SECURITY_PATH: &str = "/usr/bin/security";
const SPCTL_PATH: &str = "/usr/sbin/spctl";
const XCRUN_PATH: &str = "/usr/bin/xcrun";
static NEXT_RESERVATION: AtomicU64 = AtomicU64::new(0);

pub(crate) fn signing_identities() -> Result<Vec<String>, FormatError> {
    let output = Command::new(SECURITY_PATH)
        .args(["find-identity", "-v", "-p", "codesigning"])
        .env("LC_ALL", "C")
        .env("LANG", "C")
        .stdin(Stdio::null())
        .output()
        .map_err(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                FormatError::DependencyMissing("security".into())
            } else {
                error.into()
            }
        })?;
    if output.stdout.len().saturating_add(output.stderr.len()) > TOOL_OUTPUT_LIMIT as usize {
        return Err(FormatError::ResourceLimitExceeded(
            "macOS signing identity output exceeded 1 MiB".into(),
        ));
    }
    if !output.status.success() {
        return Err(FormatError::Other(format!(
            "reading macOS signing identities failed (exit status {})",
            output
                .status
                .code()
                .map_or_else(|| "terminated".into(), |code| code.to_string())
        )));
    }
    parse_signing_identities(&output.stdout)
}

fn parse_signing_identities(output: &[u8]) -> Result<Vec<String>, FormatError> {
    const MAX_IDENTITIES: usize = 128;
    let output = String::from_utf8_lossy(output);
    let mut seen = HashSet::new();
    let mut identities = Vec::new();
    for line in output.lines() {
        let Some(start) = line.find('"') else {
            continue;
        };
        let Some(end) = line.rfind('"') else {
            continue;
        };
        if end <= start {
            continue;
        }
        let identity = &line[start + 1..end];
        if !identity.starts_with("Developer ID Application: ") {
            continue;
        }
        validate_label(identity, "Developer ID identity")?;
        if seen.insert(identity.to_owned()) {
            if identities.len() >= MAX_IDENTITIES {
                return Err(FormatError::ResourceLimitExceeded(format!(
                    "more than {MAX_IDENTITIES} Developer ID identities were returned"
                )));
            }
            identities.push(identity.to_owned());
        }
    }
    Ok(identities)
}

#[derive(Debug, PartialEq, Eq)]
struct SignatureEvidence {
    team_id: String,
}

#[derive(Debug, PartialEq, Eq)]
struct NotaryEvidence {
    submission_id: String,
}

#[derive(Debug)]
struct ToolOutput {
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

trait ToolRunner {
    fn run(
        &mut self,
        capture_dir: &Path,
        program: &str,
        args: &[OsString],
        ctl: &ControlToken,
    ) -> Result<ToolOutput, FormatError>;
}

struct SystemToolRunner {
    sequence: u64,
}

impl SystemToolRunner {
    fn new() -> Self {
        Self { sequence: 0 }
    }
}

impl ToolRunner for SystemToolRunner {
    fn run(
        &mut self,
        capture_dir: &Path,
        program: &str,
        args: &[OsString],
        ctl: &ControlToken,
    ) -> Result<ToolOutput, FormatError> {
        ctl.checkpoint()?;
        let sequence = self.sequence;
        self.sequence = self.sequence.saturating_add(1);
        let stdout_path = capture_dir.join(format!("tool-{sequence}.stdout"));
        let stderr_path = capture_dir.join(format!("tool-{sequence}.stderr"));
        let stdout_file = create_private_capture(&stdout_path)?;
        let stderr_file = match create_private_capture(&stderr_path) {
            Ok(file) => file,
            Err(error) => {
                let _ = fs::remove_file(&stdout_path);
                return Err(error);
            }
        };
        let spawned = Command::new(system_tool_path(program))
            .args(args)
            .env("LC_ALL", "C")
            .env("LANG", "C")
            .stdin(Stdio::null())
            .stdout(Stdio::from(stdout_file))
            .stderr(Stdio::from(stderr_file))
            .spawn();
        let mut child = match spawned {
            Ok(child) => child,
            Err(error) => {
                let _ = fs::remove_file(&stdout_path);
                let _ = fs::remove_file(&stderr_path);
                return Err(if error.kind() == io::ErrorKind::NotFound {
                    FormatError::DependencyMissing(program.to_owned())
                } else {
                    error.into()
                });
            }
        };

        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => {
                    if let Err(error) = ctl.checkpoint() {
                        let _ = child.kill();
                        let _ = child.wait();
                        let _ = fs::remove_file(&stdout_path);
                        let _ = fs::remove_file(&stderr_path);
                        return Err(error);
                    }
                    thread::sleep(PROCESS_POLL_INTERVAL);
                }
                Err(error) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = fs::remove_file(&stdout_path);
                    let _ = fs::remove_file(&stderr_path);
                    return Err(error.into());
                }
            }
        };

        let stdout = read_tool_output(&stdout_path);
        let stderr = read_tool_output(&stderr_path);
        let stdout_cleanup = fs::remove_file(&stdout_path);
        let stderr_cleanup = fs::remove_file(&stderr_path);
        let stdout = stdout?;
        let stderr = stderr?;
        stdout_cleanup?;
        stderr_cleanup?;
        Ok(ToolOutput {
            status,
            stdout,
            stderr,
        })
    }
}

fn system_tool_path(program: &str) -> &str {
    match program {
        "codesign" => CODESIGN_PATH,
        "ditto" => DITTO_PATH,
        "spctl" => SPCTL_PATH,
        "xcrun" => XCRUN_PATH,
        _ => program,
    }
}

struct OwnedDirectory {
    path: PathBuf,
    handle: File,
    identity: squallz_core::api::PhysicalFileIdentity,
    active: bool,
}

impl OwnedDirectory {
    fn reserve(parent: &Path, kind: &str, suffix: &str) -> Result<Self, FormatError> {
        for _ in 0..RESERVATION_ATTEMPTS {
            let sequence = NEXT_RESERVATION.fetch_add(1, Ordering::Relaxed);
            let name = format!(
                ".squallz-sfx-{kind}-{}-{sequence}{suffix}",
                std::process::id()
            );
            let path = parent.join(name);
            let mut builder = DirBuilder::new();
            builder.mode(0o700);
            match builder.create(&path) {
                Ok(()) => {
                    let handle = open_directory_no_follow(&path)?;
                    let identity = physical_file_identity(&handle)?;
                    return Ok(Self {
                        path,
                        handle,
                        identity,
                        active: true,
                    });
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error.into()),
            }
        }
        Err(FormatError::Other(
            "could not reserve a private macOS SFX publishing workspace".into(),
        ))
    }

    fn ensure_bound(&self) -> Result<(), FormatError> {
        if !self.active
            || !self.handle.metadata()?.is_dir()
            || physical_file_identity(&self.handle)? != self.identity
            || physical_path_identity(&self.path)? != self.identity
        {
            return Err(FormatError::Other(format!(
                "private macOS SFX publishing workspace changed: {}",
                self.path.display()
            )));
        }
        Ok(())
    }

    fn cleanup(&mut self) -> Result<(), FormatError> {
        if !self.active {
            return Ok(());
        }
        self.ensure_bound()?;
        fs::remove_dir_all(&self.path)?;
        self.active = false;
        Ok(())
    }

    fn disarm(&mut self) {
        self.active = false;
    }
}

impl Drop for OwnedDirectory {
    fn drop(&mut self) {
        if self.active && self.ensure_bound().is_ok() && fs::remove_dir_all(&self.path).is_ok() {
            self.active = false;
        }
    }
}

struct PublishWorkspace {
    parent: PathBuf,
    parent_handle: File,
    parent_identity: squallz_core::api::PhysicalFileIdentity,
    stage: OwnedDirectory,
    captures: OwnedDirectory,
}

impl PublishWorkspace {
    fn reserve(output: &Path) -> Result<Self, FormatError> {
        let parent = output.parent().ok_or_else(|| {
            FormatError::Unsupported("macOS SFX output has no parent directory".into())
        })?;
        let parent_handle = open_directory_no_follow(parent)?;
        let parent_identity = physical_file_identity(&parent_handle)?;
        let stage = OwnedDirectory::reserve(parent, "publish", ".app")?;
        let captures = OwnedDirectory::reserve(parent, "notary", "")?;
        Ok(Self {
            parent: parent.to_path_buf(),
            parent_handle,
            parent_identity,
            stage,
            captures,
        })
    }

    fn ensure_bound(&self) -> Result<(), FormatError> {
        if !self.parent_handle.metadata()?.is_dir()
            || physical_file_identity(&self.parent_handle)? != self.parent_identity
            || physical_path_identity(&self.parent)? != self.parent_identity
        {
            return Err(FormatError::Other(
                "macOS SFX output directory changed during publishing".into(),
            ));
        }
        self.stage.ensure_bound()?;
        self.captures.ensure_bound()
    }

    fn cleanup(&mut self) -> Result<(), FormatError> {
        let capture_result = self.captures.cleanup();
        let stage_result = self.stage.cleanup();
        match (capture_result, stage_result) {
            (Ok(()), Ok(())) => self.parent_handle.sync_all().map_err(Into::into),
            (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
            (Err(first), Err(second)) => Err(FormatError::Other(format!(
                "{first}; another publishing workspace also needs inspection: {second}"
            ))),
        }
    }

    fn publish_stage(&mut self, output: &Path) -> Result<(), FormatError> {
        self.ensure_bound()?;
        self.captures.cleanup()?;
        self.parent_handle.sync_all()?;
        move_path_no_replace(&self.stage.path, output).map_err(|error| {
            if error.kind() == io::ErrorKind::AlreadyExists {
                FormatError::output_exists(output)
            } else {
                FormatError::from(error)
            }
        })?;
        self.stage.disarm();
        if physical_path_identity(output)? != self.stage.identity
            || physical_file_identity(&self.stage.handle)? != self.stage.identity
        {
            return Err(FormatError::Other(format!(
                "published macOS SFX no longer matches the verified staging app: {}",
                output.display()
            )));
        }
        self.parent_handle.sync_all()?;
        Ok(())
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn publish(
    ctl: &ControlToken,
    source: &Path,
    output: &Path,
    identity: &str,
    notary_profile: &str,
    resources: &ResourceOptions,
    preflight_progress: &dyn ProgressSink,
    final_progress: &dyn ProgressSink,
    phase: &mut dyn FnMut(PublishPhase),
) -> Result<PublishReport, FormatError> {
    let mut runner = SystemToolRunner::new();
    publish_with(
        ctl,
        source,
        output,
        identity,
        notary_profile,
        resources,
        preflight_progress,
        final_progress,
        &mut runner,
        phase,
    )
}

#[allow(clippy::too_many_arguments)]
fn publish_with<R: ToolRunner>(
    ctl: &ControlToken,
    requested_source: &Path,
    requested_output: &Path,
    identity: &str,
    notary_profile: &str,
    resources: &ResourceOptions,
    preflight_progress: &dyn ProgressSink,
    final_progress: &dyn ProgressSink,
    runner: &mut R,
    phase: &mut dyn FnMut(PublishPhase),
) -> Result<PublishReport, FormatError> {
    validate_label(identity, "Developer ID identity")?;
    validate_label(notary_profile, "notarytool Keychain profile")?;
    let source = canonical_source(requested_source)?;
    let output = canonical_output(requested_output)?;
    validate_paths(&source, &output)?;
    ctl.checkpoint()?;

    phase(PublishPhase::Verify);
    let source_info = verify_sfx_payload(&source, resources, preflight_progress, ctl)?;
    if source_info.target != SfxTarget::Macos {
        return Err(FormatError::Unsupported(
            "macOS SFX publishing requires a macOS .app self-extractor".into(),
        ));
    }
    let source_guard = inspect_source_guard(&source, preflight_progress, ctl)?;
    let mut workspace = PublishWorkspace::reserve(&output)?;
    let result = publish_in_workspace(
        ctl,
        &source,
        &output,
        identity,
        notary_profile,
        resources,
        preflight_progress,
        final_progress,
        source_guard,
        source_info,
        runner,
        phase,
        &mut workspace,
    );
    match result {
        Ok(report) => Ok(report),
        Err(error) => match workspace.cleanup() {
            Ok(()) => Err(error),
            Err(cleanup) => Err(FormatError::Other(format!(
                "{error}; private publishing cleanup also failed: {cleanup}"
            ))),
        },
    }
}

#[allow(clippy::too_many_arguments)]
fn publish_in_workspace<R: ToolRunner>(
    ctl: &ControlToken,
    source: &Path,
    output: &Path,
    identity: &str,
    notary_profile: &str,
    resources: &ResourceOptions,
    preflight_progress: &dyn ProgressSink,
    final_progress: &dyn ProgressSink,
    source_guard: CreateDestinationGuard,
    source_info: SfxInfo,
    runner: &mut R,
    phase: &mut dyn FnMut(PublishPhase),
    workspace: &mut PublishWorkspace,
) -> Result<PublishReport, FormatError> {
    run_success(
        workspace,
        runner,
        "ditto",
        &[path_arg(source), path_arg(&workspace.stage.path)],
        ctl,
        "copying the macOS SFX app",
    )?;
    if inspect_source_guard(source, preflight_progress, ctl)? != source_guard {
        return Err(FormatError::Other(
            "source macOS SFX changed while its private publishing copy was created".into(),
        ));
    }
    let staged_info =
        verify_sfx_payload(&workspace.stage.path, resources, preflight_progress, ctl)?;
    if staged_info.target != SfxTarget::Macos
        || staged_info.payload_bytes != source_info.payload_bytes
        || staged_info.payload_sha256 != source_info.payload_sha256
    {
        return Err(FormatError::CorruptArchive(
            "private macOS SFX publishing copy does not match the source payload".into(),
        ));
    }

    phase(PublishPhase::Sign);
    let sidecar = workspace.stage.path.join("Contents/MacOS/sqz");
    match fs::symlink_metadata(&sidecar) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
            sign_path(workspace, runner, identity, &sidecar, ctl)?;
        }
        Ok(_) => {
            return Err(FormatError::Unsupported(
                "macOS SFX CLI sidecar must be a regular non-symlink file".into(),
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    sign_path(workspace, runner, identity, &workspace.stage.path, ctl)?;
    verify_codesign(workspace, runner, ctl)?;
    let signature = inspect_signature(workspace, runner, ctl)?;

    let submission_zip = workspace.captures.path.join("submission.zip");
    let zip_output = run_bound_tool(
        workspace,
        runner,
        "ditto",
        &[
            OsString::from("-c"),
            OsString::from("-k"),
            OsString::from("--keepParent"),
            path_arg(&workspace.stage.path),
            path_arg(&submission_zip),
        ],
        ctl,
        "packaging the notarization submission",
    )?;
    require_success(zip_output, "packaging the notarization submission")?;
    require_regular_file(&submission_zip, "notarization submission ZIP")?;

    phase(PublishPhase::Notarize);
    let notary_output = run_bound_tool(
        workspace,
        runner,
        "xcrun",
        &[
            OsString::from("notarytool"),
            OsString::from("submit"),
            path_arg(&submission_zip),
            OsString::from("--keychain-profile"),
            OsString::from(notary_profile),
            OsString::from("--wait"),
            OsString::from("--timeout"),
            OsString::from(NOTARY_TIMEOUT),
            OsString::from("--output-format"),
            OsString::from("json"),
        ],
        ctl,
        "submitting the macOS SFX for notarization",
    )?;
    let notary = parse_notary_evidence(&notary_output)?;
    let notary_log = workspace.captures.path.join("notary-log.json");
    run_success(
        workspace,
        runner,
        "xcrun",
        &[
            OsString::from("notarytool"),
            OsString::from("log"),
            OsString::from(&notary.submission_id),
            OsString::from("--keychain-profile"),
            OsString::from(notary_profile),
            path_arg(&notary_log),
        ],
        ctl,
        "retrieving the Apple notarization log",
    )?;
    require_regular_file(&notary_log, "Apple notarization log")?;
    verify_notary_log(&notary_log, &notary.submission_id)?;

    phase(PublishPhase::Finalize);
    run_success(
        workspace,
        runner,
        "xcrun",
        &[
            OsString::from("stapler"),
            OsString::from("staple"),
            OsString::from("-v"),
            path_arg(&workspace.stage.path),
        ],
        ctl,
        "stapling the notarization ticket",
    )?;
    run_success(
        workspace,
        runner,
        "xcrun",
        &[
            OsString::from("stapler"),
            OsString::from("validate"),
            OsString::from("-v"),
            path_arg(&workspace.stage.path),
        ],
        ctl,
        "validating the notarization ticket",
    )?;
    sync_publish_tree(workspace)?;
    verify_codesign(workspace, runner, ctl)?;
    run_success(
        workspace,
        runner,
        "spctl",
        &[
            OsString::from("--assess"),
            OsString::from("--type"),
            OsString::from("execute"),
            OsString::from("--verbose=4"),
            path_arg(&workspace.stage.path),
        ],
        ctl,
        "checking the macOS SFX with Gatekeeper",
    )?;
    let final_info = verify_sfx_payload(&workspace.stage.path, resources, final_progress, ctl)?;
    if final_info.payload_bytes != source_info.payload_bytes
        || final_info.payload_sha256 != source_info.payload_sha256
    {
        return Err(FormatError::CorruptArchive(
            "signed macOS SFX payload changed during publishing".into(),
        ));
    }
    ctl.checkpoint()?;
    phase(PublishPhase::Commit);
    fs::remove_file(&submission_zip)?;
    workspace.ensure_bound()?;
    workspace.publish_stage(output)?;

    Ok(PublishReport {
        source: source.to_path_buf(),
        output: output.to_path_buf(),
        info: final_info,
        team_id: signature.team_id,
        submission_id: notary.submission_id,
    })
}

fn validate_label(value: &str, label: &str) -> Result<(), FormatError> {
    let trimmed = value.trim();
    if trimmed.is_empty()
        || trimmed.len() > 512
        || trimmed.chars().any(char::is_control)
        || trimmed != value
    {
        return Err(FormatError::Unsupported(format!(
            "{label} must be a non-empty single-line value without surrounding whitespace"
        )));
    }
    Ok(())
}

fn canonical_source(source: &Path) -> Result<PathBuf, FormatError> {
    let metadata = fs::symlink_metadata(source)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(FormatError::Unsupported(
            "macOS SFX source must be a non-symlink .app directory".into(),
        ));
    }
    fs::canonicalize(source).map_err(Into::into)
}

fn canonical_output(output: &Path) -> Result<PathBuf, FormatError> {
    let name = output
        .file_name()
        .ok_or_else(|| FormatError::Unsupported("macOS SFX output must have a file name".into()))?;
    let parent = output
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    Ok(fs::canonicalize(parent)?.join(name))
}

fn validate_paths(source: &Path, output: &Path) -> Result<(), FormatError> {
    if !has_app_extension(source) || !has_app_extension(output) {
        return Err(FormatError::Unsupported(
            "macOS SFX source and output must use the .app extension".into(),
        ));
    }
    if source == output {
        return Err(FormatError::Unsupported(
            "macOS SFX publishing requires a separate output path".into(),
        ));
    }
    match fs::symlink_metadata(output) {
        Ok(_) => Err(FormatError::output_exists(output)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn has_app_extension(path: &Path) -> bool {
    path.extension()
        .and_then(OsStr::to_str)
        .is_some_and(|extension| extension.eq_ignore_ascii_case("app"))
}

fn inspect_source_guard(
    source: &Path,
    progress: &dyn ProgressSink,
    ctl: &ControlToken,
) -> Result<CreateDestinationGuard, FormatError> {
    let state = inspect_create_destination_with_progress(
        source,
        CreateArtifactKind::SfxMacosApp,
        progress,
        ctl,
    )?;
    match (state.conflict, state.guard) {
        (true, Some(guard)) => Ok(guard),
        _ => Err(FormatError::Other(
            "macOS SFX source inspection returned no stable bundle state".into(),
        )),
    }
}

fn sign_path<R: ToolRunner>(
    workspace: &PublishWorkspace,
    runner: &mut R,
    identity: &str,
    path: &Path,
    ctl: &ControlToken,
) -> Result<(), FormatError> {
    run_success(
        workspace,
        runner,
        "codesign",
        &[
            OsString::from("--force"),
            OsString::from("--options"),
            OsString::from("runtime"),
            OsString::from("--timestamp"),
            OsString::from("--sign"),
            OsString::from(identity),
            path_arg(path),
        ],
        ctl,
        "signing the macOS SFX",
    )
}

fn verify_codesign<R: ToolRunner>(
    workspace: &PublishWorkspace,
    runner: &mut R,
    ctl: &ControlToken,
) -> Result<(), FormatError> {
    run_success(
        workspace,
        runner,
        "codesign",
        &[
            OsString::from("--verify"),
            OsString::from("--deep"),
            OsString::from("--strict"),
            OsString::from("--verbose=2"),
            path_arg(&workspace.stage.path),
        ],
        ctl,
        "verifying the macOS SFX code signature",
    )
}

fn inspect_signature<R: ToolRunner>(
    workspace: &PublishWorkspace,
    runner: &mut R,
    ctl: &ControlToken,
) -> Result<SignatureEvidence, FormatError> {
    let output = run_bound_tool(
        workspace,
        runner,
        "codesign",
        &[
            OsString::from("-d"),
            OsString::from("--verbose=4"),
            path_arg(&workspace.stage.path),
        ],
        ctl,
        "reading the macOS SFX signature",
    )?;
    require_success(output, "reading the macOS SFX signature")
        .and_then(|output| parse_signature_evidence(&output.stdout, &output.stderr))
}

fn parse_signature_evidence(
    stdout: &[u8],
    stderr: &[u8],
) -> Result<SignatureEvidence, FormatError> {
    let stdout = String::from_utf8_lossy(stdout);
    let stderr = String::from_utf8_lossy(stderr);
    let lines = stdout.lines().chain(stderr.lines());
    let mut developer_id = false;
    let mut team_id = None;
    let mut timestamp = false;
    let mut runtime = false;
    for line in lines {
        let line = line.trim();
        developer_id |= line.starts_with("Authority=Developer ID Application:");
        if let Some(value) = line.strip_prefix("TeamIdentifier=") {
            team_id = Some(value.trim().to_owned());
        }
        timestamp |= line.starts_with("Timestamp=") && line.len() > "Timestamp=".len();
        runtime |= line.contains("(runtime)") || line.starts_with("Runtime Version=");
    }
    let team_id = team_id.filter(|value| {
        !value.is_empty()
            && value.len() <= 64
            && value
                .chars()
                .all(|character| character.is_ascii_alphanumeric())
    });
    let Some(team_id) = team_id else {
        return Err(FormatError::Unsupported(
            "macOS SFX signature is not a timestamped hardened-runtime Developer ID Application signature"
                .into(),
        ));
    };
    if !developer_id || !timestamp || !runtime {
        return Err(FormatError::Unsupported(
            "macOS SFX signature is not a timestamped hardened-runtime Developer ID Application signature"
                .into(),
        ));
    }
    Ok(SignatureEvidence { team_id })
}

fn parse_notary_evidence(output: &ToolOutput) -> Result<NotaryEvidence, FormatError> {
    let value = serde_json::from_slice::<Value>(&output.stdout).map_err(|_| {
        FormatError::Other(
            "notarytool did not return a valid machine-readable submission result".into(),
        )
    })?;
    let submission_id = value
        .get("id")
        .and_then(Value::as_str)
        .filter(|value| valid_submission_id(value))
        .map(str::to_owned);
    let status = value
        .get("status")
        .and_then(Value::as_str)
        .map(normalized_notary_status)
        .unwrap_or("Unknown");
    if !output.status.success() || status != "Accepted" {
        let id = submission_id.as_deref().unwrap_or("unavailable");
        return Err(FormatError::Other(format!(
            "Apple notarization did not accept the macOS SFX (submission {id}, status {status}); the source remains unchanged and no output was published"
        )));
    }
    let submission_id = submission_id.ok_or_else(|| {
        FormatError::Other("accepted notarization result has no valid submission ID".into())
    })?;
    Ok(NotaryEvidence { submission_id })
}

fn verify_notary_log(path: &Path, submission_id: &str) -> Result<(), FormatError> {
    let bytes = read_tool_output(path)?;
    let value = serde_json::from_slice::<Value>(&bytes)
        .map_err(|_| FormatError::Other("Apple notarization log is not valid JSON".into()))?;
    let log_id = value
        .get("jobId")
        .or_else(|| value.get("id"))
        .and_then(Value::as_str);
    let status = value
        .get("status")
        .and_then(Value::as_str)
        .map(normalized_notary_status)
        .unwrap_or("Unknown");
    if log_id != Some(submission_id) || status != "Accepted" {
        return Err(FormatError::Other(
            "Apple notarization log does not confirm the accepted submission".into(),
        ));
    }
    Ok(())
}

fn normalized_notary_status(status: &str) -> &'static str {
    match status {
        "Accepted" => "Accepted",
        "Invalid" => "Invalid",
        "Rejected" => "Rejected",
        "In Progress" => "In Progress",
        _ => "Unknown",
    }
}

fn valid_submission_id(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 36
        && [8, 13, 18, 23]
            .into_iter()
            .all(|index| bytes[index] == b'-')
        && bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| [8, 13, 18, 23].contains(&index) || byte.is_ascii_hexdigit())
}

fn run_success<R: ToolRunner>(
    workspace: &PublishWorkspace,
    runner: &mut R,
    program: &str,
    args: &[OsString],
    ctl: &ControlToken,
    step: &str,
) -> Result<(), FormatError> {
    let output = run_bound_tool(workspace, runner, program, args, ctl, step)?;
    require_success(output, step).map(|_| ())
}

fn run_bound_tool<R: ToolRunner>(
    workspace: &PublishWorkspace,
    runner: &mut R,
    program: &str,
    args: &[OsString],
    ctl: &ControlToken,
    _step: &str,
) -> Result<ToolOutput, FormatError> {
    workspace.ensure_bound()?;
    let output = runner.run(&workspace.captures.path, program, args, ctl)?;
    workspace.ensure_bound()?;
    Ok(output)
}

fn require_success(output: ToolOutput, step: &str) -> Result<ToolOutput, FormatError> {
    if output.status.success() {
        return Ok(output);
    }
    let status = output
        .status
        .code()
        .map(|code| code.to_string())
        .unwrap_or_else(|| "terminated".into());
    Err(FormatError::Other(format!(
        "{step} failed (exit status {status})"
    )))
}

fn require_regular_file(path: &Path, label: &str) -> Result<(), FormatError> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.is_file() && !metadata.file_type().is_symlink() {
        Ok(())
    } else {
        Err(FormatError::Other(format!("{label} is not a regular file")))
    }
}

fn sync_publish_tree(workspace: &PublishWorkspace) -> Result<(), FormatError> {
    const MAX_ENTRIES: usize = 200_000;
    const MAX_DEPTH: usize = 64;

    workspace.ensure_bound()?;
    let mut pending = vec![(workspace.stage.path.clone(), 0usize)];
    let mut directories = Vec::new();
    let mut entries = 0usize;
    while let Some((directory, depth)) = pending.pop() {
        if depth > MAX_DEPTH {
            return Err(FormatError::ResourceLimitExceeded(format!(
                "macOS SFX publishing copy exceeds the {MAX_DEPTH}-level directory limit"
            )));
        }
        let handle = open_directory_no_follow(&directory)?;
        let identity = physical_file_identity(&handle)?;
        for entry in fs::read_dir(&directory)? {
            let entry = entry?;
            entries = entries.checked_add(1).ok_or_else(|| {
                FormatError::ResourceLimitExceeded(
                    "macOS SFX publishing entry count overflow".into(),
                )
            })?;
            if entries > MAX_ENTRIES {
                return Err(FormatError::ResourceLimitExceeded(format!(
                    "macOS SFX publishing copy contains more than {MAX_ENTRIES} entries"
                )));
            }
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path)?;
            let file_type = metadata.file_type();
            if file_type.is_dir() {
                pending.push((path, depth.saturating_add(1)));
            } else if file_type.is_file() {
                open_regular_file_no_follow(&path)?.sync_all()?;
            } else if !file_type.is_symlink() {
                return Err(FormatError::Unsupported(
                    "macOS SFX publishing copy contains a special file".into(),
                ));
            }
        }
        if physical_path_identity(&directory)? != identity
            || physical_file_identity(&handle)? != identity
        {
            return Err(FormatError::Other(
                "macOS SFX publishing directory changed while it was synchronized".into(),
            ));
        }
        directories.push(handle);
    }
    for directory in directories.into_iter().rev() {
        directory.sync_all()?;
    }
    workspace.ensure_bound()
}

fn create_private_capture(path: &Path) -> Result<File, FormatError> {
    use std::os::unix::fs::OpenOptionsExt;

    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(Into::into)
}

fn read_tool_output(path: &Path) -> Result<Vec<u8>, FormatError> {
    let metadata = fs::metadata(path)?;
    if metadata.len() > TOOL_OUTPUT_LIMIT {
        return Err(FormatError::ResourceLimitExceeded(
            "macOS publishing tool output exceeded 1 MiB".into(),
        ));
    }
    fs::read(path).map_err(Into::into)
}

fn path_arg(path: &Path) -> OsString {
    path.as_os_str().to_owned()
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use serde_json::json;
    use squallz_core::api::{NoProgress, ResourceOptions};
    use squallz_core::{Engine, SfxBuildOptions, SFX_CLI_STUB_MARKER, SFX_GUI_STUB_MARKER};

    use super::*;

    fn success_status() -> ExitStatus {
        use std::os::unix::process::ExitStatusExt;

        ExitStatus::from_raw(0)
    }

    fn failure_status() -> ExitStatus {
        use std::os::unix::process::ExitStatusExt;

        ExitStatus::from_raw(1 << 8)
    }

    struct FakeRunner {
        calls: Vec<Vec<String>>,
        notary_status: &'static str,
        mutate_source_after_copy: Option<PathBuf>,
        create_late_output: Option<PathBuf>,
    }

    impl FakeRunner {
        fn accepted() -> Self {
            Self {
                calls: Vec::new(),
                notary_status: "Accepted",
                mutate_source_after_copy: None,
                create_late_output: None,
            }
        }
    }

    impl ToolRunner for FakeRunner {
        fn run(
            &mut self,
            _capture_dir: &Path,
            program: &str,
            args: &[OsString],
            _ctl: &ControlToken,
        ) -> Result<ToolOutput, FormatError> {
            let mut call = vec![program.to_owned()];
            call.extend(
                args.iter()
                    .map(|argument| argument.to_string_lossy().into_owned()),
            );
            self.calls.push(call);

            if program == "ditto" && args.len() == 2 {
                copy_test_tree(Path::new(&args[0]), Path::new(&args[1]))?;
                if let Some(source) = self.mutate_source_after_copy.take() {
                    fs::write(source.join("Contents/Resources/late.txt"), b"changed")?;
                }
            } else if program == "ditto" && args.first() == Some(&OsString::from("-c")) {
                let path = args.last().ok_or_else(|| {
                    FormatError::Other("fake ditto archive has no destination".into())
                })?;
                fs::write(Path::new(path), b"fake notarization zip")?;
            } else if program == "xcrun"
                && args.first() == Some(&OsString::from("notarytool"))
                && args.get(1) == Some(&OsString::from("log"))
            {
                let path = args.last().ok_or_else(|| {
                    FormatError::Other("fake notary log has no destination".into())
                })?;
                fs::write(
                    Path::new(path),
                    br#"{"jobId":"2efe2717-52ef-43a5-96dc-0797e4ca1041","status":"Accepted","issues":[]}"#,
                )?;
            } else if program == "spctl" {
                if let Some(path) = self.create_late_output.take() {
                    fs::create_dir(&path)?;
                    fs::write(path.join("competing.txt"), b"keep me")?;
                }
            }

            let (status, stdout, stderr) = if program == "codesign"
                && args.first() == Some(&OsString::from("-d"))
            {
                (
                    success_status(),
                    Vec::new(),
                    b"Authority=Developer ID Application: Squallz Test (TEAM123456)\nTeamIdentifier=TEAM123456\nTimestamp=Jul 27, 2026\nflags=0x10000(runtime)\n".to_vec(),
                )
            } else if program == "xcrun"
                && args.first() == Some(&OsString::from("notarytool"))
                && args.get(1) == Some(&OsString::from("submit"))
            {
                let status = if self.notary_status == "Accepted" {
                    success_status()
                } else {
                    failure_status()
                };
                let stdout = serde_json::to_vec(&json!({
                    "id": "2efe2717-52ef-43a5-96dc-0797e4ca1041",
                    "status": self.notary_status,
                }))
                .map_err(|error| FormatError::Other(error.to_string()))?;
                (status, stdout, Vec::new())
            } else {
                (success_status(), Vec::new(), Vec::new())
            };
            Ok(ToolOutput {
                status,
                stdout,
                stderr,
            })
        }
    }

    fn temp_dir(label: &str) -> PathBuf {
        let sequence = NEXT_RESERVATION.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "squallz-cli-{label}-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).unwrap();
        path
    }

    fn write_fake_macho(path: &Path, marker: &[u8]) {
        let mut bytes = vec![0u8; 512];
        bytes[..4].copy_from_slice(&[0xcf, 0xfa, 0xed, 0xfe]);
        bytes[0x100..0x100 + marker.len()].copy_from_slice(marker);
        fs::write(path, bytes).unwrap();
    }

    fn create_test_sfx(root: &Path) -> PathBuf {
        let template = root.join("Squallz.app");
        let macos = template.join("Contents/MacOS");
        fs::create_dir_all(&macos).unwrap();
        fs::create_dir_all(template.join("Contents/Resources")).unwrap();
        write_fake_macho(&macos.join("squallz-gui"), &SFX_GUI_STUB_MARKER);
        write_fake_macho(&macos.join("sqz"), &SFX_CLI_STUB_MARKER);
        fs::write(
            template.join("Contents/Info.plist"),
            r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>squallz-gui</string>
<key>LSMinimumSystemVersion</key><string>11.0</string>
</dict></plist>
"#,
        )
        .unwrap();
        let payload = root.join("payload.zip");
        fs::write(&payload, b"PK\x05\x06\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0").unwrap();
        let source = root.join("Unsigned.app");
        Engine::new(squallz_formats::registry())
            .create_sfx(
                &template,
                &payload,
                &source,
                &SfxBuildOptions {
                    target: SfxTarget::Macos,
                    overwrite: false,
                    resources: ResourceOptions::default(),
                },
                &NoProgress,
                &ControlToken::default(),
            )
            .unwrap();
        source
    }

    fn copy_test_tree(source: &Path, destination: &Path) -> Result<(), FormatError> {
        let mut pending = VecDeque::from([(source.to_path_buf(), destination.to_path_buf())]);
        while let Some((from, to)) = pending.pop_front() {
            for entry in fs::read_dir(from)? {
                let entry = entry?;
                let source_path = entry.path();
                let target_path = to.join(entry.file_name());
                let metadata = fs::symlink_metadata(&source_path)?;
                if metadata.is_dir() {
                    fs::create_dir(&target_path)?;
                    pending.push_back((source_path, target_path));
                } else if metadata.is_file() {
                    fs::copy(source_path, target_path)?;
                } else {
                    return Err(FormatError::Unsupported(
                        "test copy only accepts files and directories".into(),
                    ));
                }
            }
        }
        Ok(())
    }

    fn publish_for_test(
        root: &Path,
        runner: &mut FakeRunner,
    ) -> Result<PublishReport, FormatError> {
        let source = root.join("Unsigned.app");
        let output = root.join("Published.app");
        publish_with(
            &ControlToken::default(),
            &source,
            &output,
            "Developer ID Application: Squallz Test (TEAM123456)",
            "squallz-test",
            &ResourceOptions::default(),
            &NoProgress,
            &NoProgress,
            runner,
            &mut |_| {},
        )
    }

    fn assert_no_private_workspaces(root: &Path) {
        let leftovers = fs::read_dir(root)
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with(".squallz-sfx-"))
            .collect::<Vec<_>>();
        assert!(leftovers.is_empty(), "leftovers: {leftovers:?}");
    }

    #[test]
    fn signature_evidence_requires_developer_id_timestamp_runtime_and_team() {
        let evidence = parse_signature_evidence(
            b"",
            b"Authority=Developer ID Application: Example (TEAM123456)\nTeamIdentifier=TEAM123456\nTimestamp=Jul 27, 2026\nflags=0x10000(runtime)\n",
        )
        .unwrap();
        assert_eq!(evidence.team_id, "TEAM123456");

        let error = parse_signature_evidence(
            b"",
            b"Signature=adhoc\nTeamIdentifier=not set\nflags=0x2(adhoc)\n",
        )
        .unwrap_err();
        assert!(matches!(error, FormatError::Unsupported(_)));
    }

    #[test]
    fn signing_identity_parser_keeps_only_unique_developer_id_applications() {
        let identities = parse_signing_identities(
            br#"
  1) ABCDEF0123456789 "Developer ID Application: Squallz Project (TEAM123456)"
  2) 0123456789ABCDEF "Apple Development: Local Developer (TEAM123456)"
  3) ABCDEF0123456789 "Developer ID Application: Squallz Project (TEAM123456)"
     2 valid identities found
"#,
        )
        .unwrap();
        assert_eq!(
            identities,
            ["Developer ID Application: Squallz Project (TEAM123456)"]
        );
    }

    #[test]
    fn publishing_uses_fixed_apple_tool_paths() {
        assert_eq!(system_tool_path("codesign"), "/usr/bin/codesign");
        assert_eq!(system_tool_path("ditto"), "/usr/bin/ditto");
        assert_eq!(system_tool_path("spctl"), "/usr/sbin/spctl");
        assert_eq!(system_tool_path("xcrun"), "/usr/bin/xcrun");
    }

    #[test]
    fn notary_evidence_requires_an_accepted_result_and_valid_id() {
        let accepted = ToolOutput {
            status: success_status(),
            stdout: br#"{"id":"2efe2717-52ef-43a5-96dc-0797e4ca1041","status":"Accepted"}"#
                .to_vec(),
            stderr: Vec::new(),
        };
        assert_eq!(
            parse_notary_evidence(&accepted).unwrap().submission_id,
            "2efe2717-52ef-43a5-96dc-0797e4ca1041"
        );

        let rejected = ToolOutput {
            status: failure_status(),
            stdout: br#"{"id":"2efe2717-52ef-43a5-96dc-0797e4ca1041","status":"Invalid"}"#.to_vec(),
            stderr: Vec::new(),
        };
        assert!(parse_notary_evidence(&rejected)
            .unwrap_err()
            .to_string()
            .contains("status Invalid"));
    }

    #[test]
    fn publishing_preserves_source_and_commits_only_after_all_checks() {
        let root = temp_dir("sfx-publish-success");
        let source = create_test_sfx(&root);
        let source_guard =
            inspect_source_guard(&source, &NoProgress, &ControlToken::default()).unwrap();
        let mut runner = FakeRunner::accepted();

        let report = publish_for_test(&root, &mut runner).unwrap();

        assert!(report.output.is_dir());
        assert!(source.is_dir());
        assert_eq!(
            inspect_source_guard(&source, &NoProgress, &ControlToken::default()).unwrap(),
            source_guard
        );
        assert_eq!(report.team_id, "TEAM123456");
        assert_eq!(report.submission_id, "2efe2717-52ef-43a5-96dc-0797e4ca1041");
        assert_no_private_workspaces(&root);
        let programs = runner
            .calls
            .iter()
            .map(|call| call[0].as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            programs,
            [
                "ditto", "codesign", "codesign", "codesign", "codesign", "ditto", "xcrun", "xcrun",
                "xcrun", "xcrun", "codesign", "spctl",
            ]
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejected_notarization_leaves_no_output_or_private_copy() {
        let root = temp_dir("sfx-publish-rejected");
        let source = create_test_sfx(&root);
        let source_guard =
            inspect_source_guard(&source, &NoProgress, &ControlToken::default()).unwrap();
        let mut runner = FakeRunner {
            notary_status: "Invalid",
            ..FakeRunner::accepted()
        };

        let error = publish_for_test(&root, &mut runner).unwrap_err();

        assert!(error.to_string().contains("status Invalid"));
        assert!(!root.join("Published.app").exists());
        assert_eq!(
            inspect_source_guard(&source, &NoProgress, &ControlToken::default()).unwrap(),
            source_guard
        );
        assert_no_private_workspaces(&root);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn source_change_during_copy_is_rejected_before_signing() {
        let root = temp_dir("sfx-publish-source-change");
        let source = create_test_sfx(&root);
        let mut runner = FakeRunner {
            mutate_source_after_copy: Some(source.clone()),
            ..FakeRunner::accepted()
        };

        let error = publish_for_test(&root, &mut runner).unwrap_err();

        assert!(error.to_string().contains("source macOS SFX changed"));
        assert!(!root.join("Published.app").exists());
        assert_eq!(runner.calls.len(), 1);
        assert_no_private_workspaces(&root);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn late_output_conflict_is_preserved_and_staging_is_cleaned() {
        let root = temp_dir("sfx-publish-late-output");
        create_test_sfx(&root);
        let output = root.join("Published.app");
        let mut runner = FakeRunner {
            create_late_output: Some(output.clone()),
            ..FakeRunner::accepted()
        };

        let error = publish_for_test(&root, &mut runner).unwrap_err();

        assert!(error.is_output_exists());
        assert_eq!(fs::read(output.join("competing.txt")).unwrap(), b"keep me");
        assert_no_private_workspaces(&root);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn system_tools_reject_an_unknown_identity_without_publishing() {
        let root = temp_dir("sfx-publish-system-rejection");
        create_test_sfx(&root);
        let output = root.join("Published.app");
        let mut runner = SystemToolRunner::new();

        let error = publish_with(
            &ControlToken::default(),
            &root.join("Unsigned.app"),
            &output,
            "Squallz deliberately missing Developer ID identity",
            "unused-because-signing-must-fail-first",
            &ResourceOptions::default(),
            &NoProgress,
            &NoProgress,
            &mut runner,
            &mut |_| {},
        )
        .unwrap_err();

        assert!(
            error.to_string().contains("signing the macOS SFX failed"),
            "{error}"
        );
        assert!(!output.exists());
        assert_no_private_workspaces(&root);
        fs::remove_dir_all(root).unwrap();
    }
}
