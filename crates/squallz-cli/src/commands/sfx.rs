//! SFX creation, inspection and embedded-runtime dispatch.

use std::path::{Path, PathBuf};

#[cfg(target_os = "macos")]
mod macos_publish;

use serde_json::json;
use squallz_core::api::FormatError;
use squallz_core::{
    inspect_sfx, verify_sfx_payload, CreateArtifactKind, SfxBuildOptions, SfxTarget,
    SfxTarget::Macos,
};

use crate::args::{resource_options, SfxCmd, SfxRuntimeCli, SymlinkArg};
use crate::commands::reports::{print_preserved_output_warning, print_pretty_json};
use crate::commands::Ctx;
use crate::errors::CliError;
use crate::progress::{fmt_bytes, CliProgress};

pub fn run(ctx: &Ctx, cmd: SfxCmd) -> Result<(), CliError> {
    match cmd {
        SfxCmd::Create {
            archive,
            output,
            target,
            stub,
            force,
            memory_limit,
            json,
        } => create(
            ctx,
            archive,
            output,
            target.into(),
            stub,
            force,
            memory_limit,
            json,
        ),
        SfxCmd::Inspect {
            file,
            memory_limit,
            json,
        } => inspect(ctx, file, memory_limit, json),
        SfxCmd::PublishMacos {
            source,
            output,
            identity,
            notary_profile,
            memory_limit,
            json,
        } => publish_macos(
            ctx,
            source,
            output,
            identity,
            notary_profile,
            memory_limit,
            json,
        ),
    }
}

#[allow(clippy::too_many_arguments)]
fn publish_macos(
    ctx: &Ctx,
    source: PathBuf,
    output: PathBuf,
    identity: String,
    notary_profile: String,
    memory_limit: Option<u64>,
    json_output: bool,
) -> Result<(), CliError> {
    #[cfg(target_os = "macos")]
    {
        macos_publish::run(
            ctx,
            source,
            output,
            identity,
            notary_profile,
            memory_limit,
            json_output,
        )
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = (
            ctx,
            source,
            output,
            identity,
            notary_profile,
            memory_limit,
            json_output,
        );
        Err(FormatError::Unsupported(
            "macOS SFX publishing must run on macOS with Xcode command-line tools".into(),
        )
        .into())
    }
}

#[allow(clippy::too_many_arguments)]
fn create(
    ctx: &Ctx,
    archive: PathBuf,
    output: PathBuf,
    target: SfxTarget,
    stub: Option<PathBuf>,
    force: bool,
    memory_limit: Option<u64>,
    json_output: bool,
) -> Result<(), CliError> {
    let stub = resolve_stub(target, stub)?;
    let resources = resource_options(None, memory_limit);
    let make_progress = || {
        CliProgress::new_for_operation(
            ctx.quiet,
            ctx.verbose,
            json_output,
            ctx.output_style,
            ctx.color,
            ctx.accent,
            "compress",
        )
    };
    let inspection_progress = make_progress();
    let policy = match super::create_commit_policy(
        &output,
        if target == SfxTarget::Macos {
            CreateArtifactKind::SfxMacosApp
        } else {
            CreateArtifactKind::SfxSingleFile
        },
        force,
        &inspection_progress,
        &ctx.ctl,
    ) {
        Ok(policy) => policy,
        Err(error) => {
            inspection_progress.finish();
            return Err(error.into());
        }
    };
    inspection_progress.finish();
    let progress = make_progress();
    let report = ctx.engine.create_sfx_with_policy(
        &stub,
        &archive,
        &output,
        &SfxBuildOptions {
            target,
            overwrite: force,
            resources,
        },
        policy,
        &progress,
        &ctx.ctl,
    );
    progress.finish();
    let report = report?;

    if json_output {
        let path = report.path.display().to_string();
        let preserved_outputs = report
            .preserved_outputs
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>();
        print_pretty_json(&json!({
            "ok": true,
            "operation": "sfx_create",
            "path": path,
            "target": report.target.as_str(),
            "layout": report.layout.as_str(),
            "stub_bytes": report.stub_bytes,
            "payload_bytes": report.payload_bytes,
            "total_bytes": report.total_bytes,
            "payload_crc32": format!("{:08x}", report.payload_crc32),
            "payload_sha256": report.payload_sha256.map(hex_digest),
            "requires_signing": report.requires_signing,
            "preserved_outputs": preserved_outputs,
            "auto_run": false,
        }))?;
        return Ok(());
    }

    let path = report.path.display().to_string();
    let size = fmt_bytes(report.total_bytes);
    ctx.print_success(
        ctx.loc
            .format("cli.sfx.created", &[("path", &path), ("size", &size)]),
    );
    ctx.eprint_notice(ctx.loc.t("cli.sfx.sign_after_build"));
    ctx.eprint_notice(ctx.loc.t("cli.sfx.no_auto_run"));
    let preserved_outputs = report
        .preserved_outputs
        .iter()
        .map(|preserved| preserved.display().to_string())
        .collect::<Vec<_>>();
    print_preserved_output_warning(ctx, &preserved_outputs);
    Ok(())
}

fn inspect(
    ctx: &Ctx,
    file: PathBuf,
    memory_limit: Option<u64>,
    json_output: bool,
) -> Result<(), CliError> {
    let resources = resource_options(None, memory_limit);
    let progress = CliProgress::new_for_operation(
        ctx.quiet,
        ctx.verbose,
        json_output,
        ctx.output_style,
        ctx.color,
        ctx.accent,
        "test",
    );
    let info = verify_sfx_payload(&file, &resources, &progress, &ctx.ctl);
    progress.finish();
    let info = info?;

    if json_output {
        print_pretty_json(&json!({
            "ok": true,
            "operation": "sfx_inspect",
            "path": file,
            "target": info.target.as_str(),
            "layout": info.layout.as_str(),
            "stub_bytes": info.stub_bytes(),
            "payload_bytes": info.payload_bytes,
            "total_bytes": info.total_bytes,
            "payload_crc32": format!("{:08x}", info.payload_crc32),
            "payload_sha256": info.payload_sha256.map(hex_digest),
            "checksum_verified": true,
            "auto_run": false,
        }))?;
        return Ok(());
    }

    let path = file.display().to_string();
    let target = info.target.as_str();
    let size = fmt_bytes(info.payload_bytes);
    ctx.print_success(ctx.loc.format(
        "cli.sfx.verified",
        &[("path", &path), ("target", target), ("size", &size)],
    ));
    Ok(())
}

fn resolve_stub(target: SfxTarget, stub: Option<PathBuf>) -> Result<PathBuf, FormatError> {
    if let Some(stub) = stub {
        return Ok(stub);
    }
    if target == Macos {
        return current_macos_app_template()?.ok_or_else(|| {
            FormatError::Unsupported(
                "macOS SFX creation requires --stub Squallz.app when sqz is not running from a Squallz app bundle"
                    .into(),
            )
        });
    }
    if target != SfxTarget::host() {
        return Err(FormatError::Unsupported(format!(
            "target {} requires --stub built for that platform",
            target.as_str()
        )));
    }
    std::env::current_exe().map_err(FormatError::from)
}

fn current_macos_app_template() -> Result<Option<PathBuf>, FormatError> {
    let executable = std::env::current_exe()?;
    Ok(executable
        .ancestors()
        .find(|path| {
            path.extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("app"))
                && path.join("Contents/Info.plist").is_file()
        })
        .map(Path::to_path_buf))
}

fn hex_digest(bytes: [u8; 32]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub fn run_embedded(ctx: &Ctx, executable: PathBuf, cli: SfxRuntimeCli) -> Result<(), CliError> {
    let resources = resource_options(cli.threads, cli.memory_limit);
    let progress = CliProgress::new_for_operation(
        ctx.quiet,
        ctx.verbose,
        cli.json,
        ctx.output_style,
        ctx.color,
        ctx.accent,
        "test",
    );
    let verification = verify_sfx_payload(&executable, &resources, &progress, &ctx.ctl);
    progress.finish();
    verification?;

    if cli.list {
        return super::list::run(ctx, executable, None, None, None, cli.json, false);
    }
    if cli.test {
        return super::test::run(ctx, executable, None, None, cli.json);
    }
    let output = match cli.output {
        Some(path) => path,
        None => default_extract_dest(&executable)?,
    };
    super::extract::run(
        ctx,
        executable,
        Some(output),
        Vec::new(),
        cli.overwrite,
        None,
        None,
        SymlinkArg::Skip,
        true,
        false,
        cli.threads,
        cli.memory_limit,
        cli.max_output_bytes,
        cli.max_entries,
        cli.max_compression_ratio,
        cli.json,
    )
}

fn default_extract_dest(executable: &Path) -> Result<PathBuf, FormatError> {
    let stem = executable
        .file_stem()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or("extracted");
    Ok(std::env::current_dir()?.join(stem))
}

pub fn embedded_probe(path: &Path) -> Result<bool, FormatError> {
    Ok(inspect_sfx(path)?.is_some())
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;

    #[test]
    fn default_destination_uses_executable_stem() {
        let dest = default_extract_dest(Path::new("/tmp/Release.exe")).unwrap();
        assert_eq!(dest.file_name().unwrap(), "Release");
    }

    #[test]
    fn cross_target_without_stub_is_rejected() {
        let target = match SfxTarget::host() {
            SfxTarget::Windows => SfxTarget::Linux,
            SfxTarget::Linux | SfxTarget::Macos => SfxTarget::Windows,
        };
        let err = resolve_stub(target, None).unwrap_err();
        assert!(matches!(err, FormatError::Unsupported(_)));
    }

    #[test]
    fn macos_default_stub_requires_an_app_context() {
        let err = resolve_stub(SfxTarget::Macos, None).unwrap_err();
        assert!(matches!(err, FormatError::Unsupported(_)));
    }

    #[test]
    fn overwrite_arg_is_part_of_runtime_contract() {
        let parsed = SfxRuntimeCli::try_parse_from(["sfx", "--overwrite", "rename"]).unwrap();
        assert!(matches!(
            parsed.overwrite,
            crate::args::OverwriteArg::Rename
        ));
    }
}
