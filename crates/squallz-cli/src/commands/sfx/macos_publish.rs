use std::path::PathBuf;

use serde_json::json;
use squallz_publish::{publish_macos_sfx, MacosSfxPublishPhase};

use super::hex_digest;
use crate::args::resource_options;
use crate::commands::reports::print_pretty_json;
use crate::commands::Ctx;
use crate::errors::CliError;
use crate::progress::CliProgress;

pub(super) fn run(
    ctx: &Ctx,
    source: PathBuf,
    output: PathBuf,
    identity: String,
    notary_profile: String,
    memory_limit: Option<u64>,
    json_output: bool,
) -> Result<(), CliError> {
    let resources = resource_options(None, memory_limit);
    let preflight_progress = CliProgress::new_for_operation(
        ctx.quiet,
        ctx.verbose,
        json_output,
        ctx.output_style,
        ctx.color,
        ctx.accent,
        "test",
    );
    let final_progress = CliProgress::new_for_operation(
        ctx.quiet,
        ctx.verbose,
        json_output,
        ctx.output_style,
        ctx.color,
        ctx.accent,
        "test",
    );
    let mut phase = |next| {
        if next == MacosSfxPublishPhase::Sign {
            preflight_progress.finish();
        }
        if json_output {
            return;
        }
        let key = match next {
            MacosSfxPublishPhase::Verify => "cli.sfx.publish_macos.verifying",
            MacosSfxPublishPhase::Sign => "cli.sfx.publish_macos.signing",
            MacosSfxPublishPhase::Notarize => "cli.sfx.publish_macos.notarizing",
            MacosSfxPublishPhase::Finalize | MacosSfxPublishPhase::Commit => {
                "cli.sfx.publish_macos.finalizing"
            }
        };
        ctx.eprint_notice(ctx.loc.t(key));
    };
    let result = publish_macos_sfx(
        &ctx.ctl,
        &source,
        &output,
        &identity,
        &notary_profile,
        &resources,
        &preflight_progress,
        &final_progress,
        &mut phase,
    );
    preflight_progress.finish();
    final_progress.finish();
    let report = result?;

    if json_output {
        print_pretty_json(&json!({
            "ok": true,
            "operation": "sfx_publish_macos",
            "source": report.source,
            "path": report.output,
            "target": report.info.target.as_str(),
            "layout": report.info.layout.as_str(),
            "payload_bytes": report.info.payload_bytes,
            "total_bytes": report.info.total_bytes,
            "payload_sha256": report.info.payload_sha256.map(hex_digest),
            "signature": "developer_id",
            "team_id": report.team_id,
            "notarization": "Accepted",
            "submission_id": report.submission_id,
            "stapled": true,
            "codesign_verified": true,
            "gatekeeper_verified": true,
            "checksum_verified": true,
            "source_preserved": true,
            "requires_signing": false,
            "auto_run": false,
        }))?;
        return Ok(());
    }

    let output = report.output.display().to_string();
    let source = report.source.display().to_string();
    ctx.print_success(
        ctx.loc
            .format("cli.sfx.publish_macos.done", &[("path", &output)]),
    );
    ctx.eprint_notice(ctx.loc.format(
        "cli.sfx.publish_macos.accepted",
        &[
            ("submission", &report.submission_id),
            ("team", &report.team_id),
        ],
    ));
    ctx.eprint_notice(ctx.loc.format(
        "cli.sfx.publish_macos.source_preserved",
        &[("path", &source)],
    ));
    Ok(())
}
