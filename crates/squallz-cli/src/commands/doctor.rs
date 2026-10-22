//! `sqz doctor`: runtime readiness diagnostics for bundled and external
//! engines. This command does not execute archive operations; it explains
//! whether the current machine can use the advertised capabilities.

use serde_json::{json, Value};
use squallz_core::api::FormatInfo;

use crate::commands::reports::print_pretty_json;
use crate::commands::runtime::{
    is_external, Availability, FormatOverview, RuntimeFacts, RuntimeNeed, RAR_LIMITATIONS,
};
use crate::commands::{Ctx, ModernStatusField, ModernTableColumn, ModernTableRow};
use crate::errors::CliError;
use crate::ui::Tone;

pub fn run(ctx: &Ctx, strict: bool, json_output: bool) -> Result<(), CliError> {
    let formats = ctx.engine.supported_formats();
    let runtime = RuntimeFacts::capture();
    let report = DoctorReport::new(&formats, strict, &runtime);
    if json_output {
        let value = report.to_json();
        print_pretty_json(&value)?;
    } else if ctx.is_modern() {
        print_modern(ctx, &report);
    } else {
        print_classic(&report);
    }
    if !report.ok {
        return Err(CliError::Exit(8));
    }
    Ok(())
}

#[derive(Debug)]
struct DoctorReport<'a> {
    ok: bool,
    strict: bool,
    total_formats: usize,
    built_in_formats: usize,
    external_formats: usize,
    ready_formats: usize,
    missing_formats: usize,
    checks: Vec<DoctorCheck<'a>>,
}

impl<'a> DoctorReport<'a> {
    fn new(formats: &[FormatInfo], strict: bool, runtime: &'a RuntimeFacts) -> Self {
        let overview = FormatOverview::new(formats, runtime);
        let checks = vec![
            built_in_check(formats),
            sevenzip_check(formats, strict, runtime),
            wim_write_check(formats, strict, runtime),
            sqz_recovery_check(formats),
            par2_create_check(strict, runtime),
            par2_verify_repair_check(runtime),
            rar_boundary_check(formats, runtime),
        ];
        let ok = checks.iter().all(|check| check.status != CheckStatus::Fail);
        Self {
            ok,
            strict,
            total_formats: formats.len(),
            built_in_formats: overview.built_in,
            external_formats: overview.external,
            ready_formats: overview.ready,
            missing_formats: overview.missing,
            checks,
        }
    }

    fn to_json(&self) -> Value {
        json!({
            "ok": self.ok,
            "operation": "doctor",
            "strict": self.strict,
            "summary": {
                "formats": self.total_formats,
                "built_in": self.built_in_formats,
                "external": self.external_formats,
                "ready": self.ready_formats,
                "missing": self.missing_formats,
            },
            "checks": self.checks.iter().map(DoctorCheck::to_json).collect::<Vec<_>>(),
        })
    }
}

#[derive(Debug)]
struct DoctorCheck<'a> {
    id: &'static str,
    status: CheckStatus,
    scope: String,
    detail: String,
    strict_required: bool,
    formats: Vec<String>,
    availability: Availability<'a>,
}

impl DoctorCheck<'_> {
    fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "status": self.status.as_str(),
            "strict_required": self.strict_required,
            "scope": self.scope,
            "detail": self.detail,
            "formats": self.formats,
            "availability": self.availability.to_json(),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CheckStatus {
    Pass,
    Warn,
    Boundary,
    Fail,
}

impl CheckStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Warn => "warn",
            Self::Boundary => "boundary",
            Self::Fail => "fail",
        }
    }

    fn tone(self) -> Tone {
        match self {
            Self::Pass => Tone::Success,
            Self::Warn | Self::Boundary => Tone::Warning,
            Self::Fail => Tone::Danger,
        }
    }
}

fn built_in_check(formats: &[FormatInfo]) -> DoctorCheck<'static> {
    let built_in = formats
        .iter()
        .filter(|format| !is_external(format.id))
        .map(|format| format.id.to_owned())
        .collect::<Vec<_>>();
    DoctorCheck {
        id: "built-in-formats",
        status: CheckStatus::Pass,
        scope: "zip/tar/7z/sqz/stream codecs".to_owned(),
        detail: "built-in Rust engines are available without external tools".to_owned(),
        strict_required: true,
        formats: built_in,
        availability: Availability::BuiltIn(true),
    }
}

fn sevenzip_check<'a>(
    formats: &[FormatInfo],
    strict: bool,
    runtime: &'a RuntimeFacts,
) -> DoctorCheck<'a> {
    let affected = formats
        .iter()
        .filter(|format| is_external(format.id))
        .map(|format| format.id.to_owned())
        .collect::<Vec<_>>();
    let availability = runtime.sevenzip();
    let available = availability.available();
    DoctorCheck {
        id: "7z-read-bridge",
        status: availability_status(available, strict),
        scope: "long-tail unpack/test bridge".to_owned(),
        detail: if available {
            availability_detail("7z bridge ready", availability)
        } else {
            "install 7zz/7z or set SQUALLZ_7Z for long-tail unpack-only formats".to_owned()
        },
        strict_required: true,
        formats: affected,
        availability,
    }
}

fn wim_write_check<'a>(
    formats: &[FormatInfo],
    strict: bool,
    runtime: &'a RuntimeFacts,
) -> DoctorCheck<'a> {
    let present = formats.iter().any(|format| format.id == "wim");
    let availability = runtime.availability("wim", RuntimeNeed::Write);
    let available = availability.available();
    DoctorCheck {
        id: "wim-writer",
        status: availability_status(available, strict),
        scope: "WIM create".to_owned(),
        detail: if available {
            availability_detail("wimlib-imagex writer ready", availability)
        } else {
            "install wimlib-imagex or set SQUALLZ_WIMLIB before creating WIM archives".to_owned()
        },
        strict_required: true,
        formats: if present {
            vec!["wim".to_owned()]
        } else {
            Vec::new()
        },
        availability,
    }
}

fn sqz_recovery_check(formats: &[FormatInfo]) -> DoctorCheck<'static> {
    let present = formats.iter().any(|format| format.id == "sqz");
    DoctorCheck {
        id: "sqz-embedded-recovery",
        status: if present {
            CheckStatus::Pass
        } else {
            CheckStatus::Fail
        },
        scope: ".sqz embedded recovery".to_owned(),
        detail: if present {
            "SQZ container read/write and embedded recovery are built in".to_owned()
        } else {
            "SQZ format is missing from the registry".to_owned()
        },
        strict_required: true,
        formats: if present {
            vec!["sqz".to_owned()]
        } else {
            Vec::new()
        },
        availability: Availability::BuiltIn(present),
    }
}

fn par2_create_check(strict: bool, runtime: &RuntimeFacts) -> DoctorCheck<'_> {
    let availability = runtime.par2();
    let available = availability.available();
    DoctorCheck {
        id: "par2-create",
        status: availability_status(available, strict),
        scope: "external PAR2 sidecar create".to_owned(),
        detail: if available {
            availability_detail("PAR2 create tool ready", availability)
        } else {
            "PAR2 create still needs par2cmdline-turbo, par2, par2cmdline, or SQUALLZ_PAR2"
                .to_owned()
        },
        strict_required: true,
        formats: vec!["par2".to_owned()],
        availability,
    }
}

fn par2_verify_repair_check(runtime: &RuntimeFacts) -> DoctorCheck<'_> {
    let availability = runtime.par2();
    let available = availability.available();
    DoctorCheck {
        id: "par2-verify-repair",
        status: CheckStatus::Pass,
        scope: "external PAR2 verify/repair".to_owned(),
        detail: if available {
            availability_detail("external PAR2 verify/repair ready", availability)
        } else {
            "rust-par2 fallback is built in for verify/repair; create still needs an external tool"
                .to_owned()
        },
        strict_required: false,
        formats: vec!["par2".to_owned()],
        availability: if available {
            availability
        } else {
            Availability::Par2Fallback
        },
    }
}

fn rar_boundary_check<'a>(formats: &[FormatInfo], runtime: &'a RuntimeFacts) -> DoctorCheck<'a> {
    let present = formats.iter().any(|format| format.id == "rar");
    let limits = RAR_LIMITATIONS.len();
    DoctorCheck {
        id: "rar-product-boundary",
        status: CheckStatus::Boundary,
        scope: "RAR unpack-only".to_owned(),
        detail: format!(
            "RAR is unpack-only through external 7zz/7z with bsdtar as a diagnostic fallback or validated single-file compatibility fallback; RAR creation, RAR recovery records, encrypted/full multi-volume compatibility, and damaged RAR repair remain outside release claims ({limits} documented limitations)"
        ),
        strict_required: false,
        formats: if present {
            vec!["rar".to_owned(), "cbr".to_owned()]
        } else {
            Vec::new()
        },
        availability: runtime.availability("rar", RuntimeNeed::Read),
    }
}

fn availability_status(available: bool, strict: bool) -> CheckStatus {
    match (available, strict) {
        (true, _) => CheckStatus::Pass,
        (false, true) => CheckStatus::Fail,
        (false, false) => CheckStatus::Warn,
    }
}

fn availability_detail(ready: &str, availability: Availability<'_>) -> String {
    let selected = availability
        .selected()
        .filter(|path| !path.as_os_str().is_empty())
        .map_or_else(|| "built-in".into(), std::path::Path::to_string_lossy);
    let source = availability.source().unwrap_or("runtime");
    format!("{ready} via {source}: {selected}")
}

fn print_classic(report: &DoctorReport<'_>) {
    println!("doctor: {}", if report.ok { "pass" } else { "fail" });
    println!(
        "formats: total={} built-in={} external={} ready={} missing={}",
        report.total_formats,
        report.built_in_formats,
        report.external_formats,
        report.ready_formats,
        report.missing_formats
    );
    println!("{:<26} {:<9} {:<30} detail", "check", "status", "scope");
    for check in &report.checks {
        println!(
            "{:<26} {:<9} {:<30} {}",
            check.id,
            check.status.as_str(),
            truncate(&check.scope, 30),
            check.detail
        );
    }
}

fn print_modern(ctx: &Ctx, report: &DoctorReport<'_>) {
    let pass = report
        .checks
        .iter()
        .filter(|check| check.status == CheckStatus::Pass)
        .count();
    let warn = report
        .checks
        .iter()
        .filter(|check| check.status == CheckStatus::Warn)
        .count();
    let boundary = report
        .checks
        .iter()
        .filter(|check| check.status == CheckStatus::Boundary)
        .count();
    let fail = report
        .checks
        .iter()
        .filter(|check| check.status == CheckStatus::Fail)
        .count();
    let tone = if fail > 0 {
        Tone::Danger
    } else if warn > 0 || boundary > 0 {
        Tone::Warning
    } else {
        Tone::Success
    };
    ctx.print_modern_status_panel(
        "Runtime doctor",
        if report.ok { "pass" } else { "fail" },
        tone,
        "Machine capability check for built-in formats, external bridges, and recovery tools",
        &[
            ModernStatusField::new("Formats", report.total_formats.to_string()),
            ModernStatusField::new("Ready", report.ready_formats.to_string()),
            ModernStatusField::new("Missing", report.missing_formats.to_string()),
            ModernStatusField::new("Pass", pass.to_string()),
            ModernStatusField::new("Warn", warn.to_string()),
            ModernStatusField::new("Boundary", boundary.to_string()),
        ],
    );
    ctx.print_modern_wrapped_table(
        "Runtime checks",
        &[
            ModernTableColumn::new("Check", 24),
            ModernTableColumn::new("Status", 9),
            ModernTableColumn::new("Scope", 22),
            ModernTableColumn::new("Detail", 50),
        ],
        &report
            .checks
            .iter()
            .map(|check| {
                ModernTableRow::with_tone(
                    vec![
                        check.id.to_owned(),
                        check.status.as_str().to_owned(),
                        check.scope.clone(),
                        check.detail.clone(),
                    ],
                    check.status.tone(),
                )
            })
            .collect::<Vec<_>>(),
    );
}

fn truncate(value: &str, width: usize) -> String {
    if value.chars().count() <= width {
        value.to_owned()
    } else {
        let mut out = value
            .chars()
            .take(width.saturating_sub(3))
            .collect::<String>();
        out.push_str("...");
        out
    }
}
