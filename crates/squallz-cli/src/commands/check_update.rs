//! `sqz check-update`: read the stable release channel without downloading or
//! installing software.

use std::time::Duration;

use serde_json::{json, Value};
use squallz_core::api::{ControlToken, FormatError};
use squallz_update::{
    ReleasePackage, UpdateCheck, UpdateError, UpdateMetadataSource, UpdateStatus, UpdateTrust,
};

use super::reports::print_pretty_json;
use crate::commands::{Ctx, ModernStatusField};
use crate::errors::CliError;
use crate::progress::fmt_bytes;
use crate::ui::Tone;

const CANCEL_POLL_INTERVAL: Duration = Duration::from_millis(40);

pub fn run(ctx: &Ctx, json_output: bool) -> Result<(), CliError> {
    if !ctx.quiet && !json_output {
        ctx.eprint_notice(ctx.loc.t("cli.check_update.checking"));
    }

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| {
            UpdateError::unavailable(format!("cannot start the update-check runtime: {error}"))
        })?;
    let result = runtime.block_on(async {
        tokio::select! {
            biased;
            _ = wait_for_cancel(ctx.ctl.as_ref()) => Err(CliError::from(FormatError::Cancelled)),
            result = squallz_update::check_for_updates(
                env!("CARGO_PKG_VERSION"),
                ReleasePackage::CommandLine,
            ) => result.map_err(CliError::from),
        }
    })?;

    if json_output {
        print_pretty_json(&report_json(&result))?;
    } else if ctx.is_modern() {
        print_modern(ctx, &result);
    } else {
        print_classic(ctx, &result);
    }
    Ok(())
}

async fn wait_for_cancel(control: &ControlToken) {
    loop {
        if control.is_cancelled() {
            return;
        }
        tokio::time::sleep(CANCEL_POLL_INTERVAL).await;
    }
}

fn report_json(report: &UpdateCheck) -> Value {
    json!({
        "ok": true,
        "operation": "check_update",
        "status": report.status.as_str(),
        "current_version": report.current_version,
        "latest_version": report.latest_version,
        "release_name": report.release_name,
        "release_url": report.release_url,
        "published_at": report.published_at,
        "platform": report.platform,
        "architecture": report.architecture,
        "asset_name": report.asset_name,
        "download_url": report.download_url,
        "asset_size_bytes": report.asset_size_bytes,
        "asset_sha256": report.asset_sha256,
        "asset_trust": report.asset_trust.as_str(),
        "metadata_source": report.metadata_source.as_str(),
    })
}

fn print_classic(ctx: &Ctx, report: &UpdateCheck) {
    print_safe_line(ctx, status_summary(ctx, report), status_tone(report.status));
    print_details(ctx, report);
}

fn print_modern(ctx: &Ctx, report: &UpdateCheck) {
    ctx.print_modern_status_panel(
        &ctx.loc.t("cli.check_update.heading"),
        &status_label(ctx, report.status),
        status_tone(report.status),
        &status_summary(ctx, report),
        &[
            ModernStatusField::new(
                ctx.loc.t("cli.check_update.current_version"),
                report.current_version.clone(),
            ),
            ModernStatusField::new(
                ctx.loc.t("cli.check_update.latest_version"),
                report.latest_version.clone(),
            ),
            ModernStatusField::new(ctx.loc.t("cli.check_update.target"), target_label(report)),
        ],
    );
    print_details(ctx, report);
}

fn print_details(ctx: &Ctx, report: &UpdateCheck) {
    println!();
    print_safe_line(
        ctx,
        ctx.loc.t("cli.check_update.details_title"),
        Tone::Primary,
    );
    if report.status == UpdateStatus::UpdateAvailable {
        print_detail(
            ctx,
            "cli.check_update.package",
            report
                .asset_name
                .clone()
                .unwrap_or_else(|| ctx.loc.t("cli.check_update.package_unavailable")),
        );
        print_detail(
            ctx,
            "cli.check_update.package_size",
            report
                .asset_size_bytes
                .map(fmt_bytes)
                .unwrap_or_else(|| ctx.loc.t("common.unavailable")),
        );
        print_detail(
            ctx,
            "cli.check_update.trust",
            trust_label(ctx, report.asset_trust),
        );
        print_detail(
            ctx,
            "cli.check_update.sha256",
            report
                .asset_sha256
                .clone()
                .unwrap_or_else(|| ctx.loc.t("common.unavailable")),
        );
    }
    print_detail(
        ctx,
        "cli.check_update.metadata_source",
        metadata_source_label(ctx, report.metadata_source),
    );
    print_detail(
        ctx,
        "cli.check_update.published_at",
        if report.published_at.is_empty() {
            ctx.loc.t("common.unavailable")
        } else {
            report.published_at.clone()
        },
    );
    if report.status == UpdateStatus::UpdateAvailable {
        if let Some(url) = report.download_url.as_ref() {
            print_detail(ctx, "cli.check_update.download_url", url);
        }
    }
    print_detail(
        ctx,
        "cli.check_update.release_url",
        report.release_url.as_str(),
    );
    print_safe_line(
        ctx,
        ctx.loc.t("cli.check_update.no_install"),
        Tone::Secondary,
    );
}

fn print_detail(ctx: &Ctx, key: &str, value: impl AsRef<str>) {
    print_safe_line(
        ctx,
        format!("{}: {}", ctx.loc.t(key), value.as_ref()),
        Tone::Secondary,
    );
}

fn print_safe_line(ctx: &Ctx, line: impl AsRef<str>, tone: Tone) {
    println!("{}", ctx.paint_stdout_tone(tone, line.as_ref()));
}

fn status_summary(ctx: &Ctx, report: &UpdateCheck) -> String {
    match report.status {
        UpdateStatus::UpdateAvailable => ctx.loc.format(
            "cli.check_update.summary.available",
            &[
                ("version", &report.latest_version),
                ("current", &report.current_version),
            ],
        ),
        UpdateStatus::UpToDate => ctx.loc.format(
            "cli.check_update.summary.current",
            &[("version", &report.current_version)],
        ),
        UpdateStatus::Ahead => ctx.loc.format(
            "cli.check_update.summary.ahead",
            &[
                ("current", &report.current_version),
                ("version", &report.latest_version),
            ],
        ),
    }
}

fn status_label(ctx: &Ctx, status: UpdateStatus) -> String {
    let key = match status {
        UpdateStatus::UpdateAvailable => "cli.check_update.status.available",
        UpdateStatus::UpToDate => "cli.check_update.status.current",
        UpdateStatus::Ahead => "cli.check_update.status.ahead",
    };
    ctx.loc.t(key)
}

fn status_tone(status: UpdateStatus) -> Tone {
    match status {
        UpdateStatus::UpToDate => Tone::Success,
        UpdateStatus::UpdateAvailable | UpdateStatus::Ahead => Tone::Warning,
    }
}

fn trust_label(ctx: &Ctx, trust: UpdateTrust) -> String {
    let key = match trust {
        UpdateTrust::DeveloperIdNotarized => "cli.check_update.trust.notarized",
        UpdateTrust::UnsignedPreview => "cli.check_update.trust.unsigned",
        UpdateTrust::Unavailable => "cli.check_update.trust.unavailable",
    };
    ctx.loc.t(key)
}

fn metadata_source_label(ctx: &Ctx, source: UpdateMetadataSource) -> String {
    let key = match source {
        UpdateMetadataSource::GithubApi => "cli.check_update.source.github_api",
        UpdateMetadataSource::LatestReleaseRedirect => {
            "cli.check_update.source.latest_release_redirect"
        }
        UpdateMetadataSource::LatestReleaseManifest => {
            "cli.check_update.source.latest_release_manifest"
        }
    };
    ctx.loc.t(key)
}

fn target_label(report: &UpdateCheck) -> String {
    format!("{}/{}", report.platform, report.architecture)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn update_check() -> UpdateCheck {
        UpdateCheck {
            status: UpdateStatus::UpdateAvailable,
            current_version: "0.1.0".to_owned(),
            latest_version: "0.2.0".to_owned(),
            release_name: "Squallz 0.2.0".to_owned(),
            release_url: "https://github.com/yangzhg/Squallz/releases/tag/v0.2.0".to_owned(),
            published_at: "2026-08-12T08:00:00Z".to_owned(),
            platform: "macos".to_owned(),
            architecture: "arm64".to_owned(),
            asset_name: None,
            download_url: None,
            asset_size_bytes: None,
            asset_sha256: None,
            asset_trust: UpdateTrust::Unavailable,
            metadata_source: UpdateMetadataSource::LatestReleaseRedirect,
        }
    }

    #[test]
    fn json_contract_uses_cli_snake_case_and_keeps_missing_package_fields() {
        let value = report_json(&update_check());

        assert_eq!(value["ok"], true);
        assert_eq!(value["operation"], "check_update");
        assert_eq!(value["status"], "update_available");
        assert_eq!(value["current_version"], "0.1.0");
        assert_eq!(value["metadata_source"], "latest_release_redirect");
        assert!(value["asset_name"].is_null());
        assert!(value["download_url"].is_null());
        assert!(value["asset_size_bytes"].is_null());
        assert!(value["asset_sha256"].is_null());
        assert!(value.get("currentVersion").is_none());
    }

    #[test]
    fn update_status_tones_do_not_present_ahead_as_success() {
        assert_eq!(status_tone(UpdateStatus::UpToDate), Tone::Success);
        assert_eq!(status_tone(UpdateStatus::UpdateAvailable), Tone::Warning);
        assert_eq!(status_tone(UpdateStatus::Ahead), Tone::Warning);
    }
}
