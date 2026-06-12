//! `sqz doctor`: runtime readiness diagnostics for bundled and external
//! engines. This command does not execute archive operations; it explains
//! whether the current machine can use the advertised capabilities.

use std::env;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use squallz_core::api::FormatInfo;

use crate::commands::info::implementation_json;
use crate::commands::reports::print_pretty_json;
use crate::commands::{Ctx, ModernStatusField, ModernTableColumn, ModernTableRow};
use crate::errors::CliError;
use crate::ui::Tone;

const PAR2_ENV: &str = "SQUALLZ_PAR2";
const PAR2_TOOLS: [&str; 3] = ["par2cmdline-turbo", "par2", "par2cmdline"];

pub fn run(ctx: &Ctx, strict: bool, json_output: bool) -> Result<(), CliError> {
    let formats = ctx.engine.supported_formats();
    let report = DoctorReport::new(&formats, strict);
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
struct DoctorReport {
    ok: bool,
    strict: bool,
    total_formats: usize,
    built_in_formats: usize,
    external_formats: usize,
    ready_formats: usize,
    missing_formats: usize,
    checks: Vec<DoctorCheck>,
}

impl DoctorReport {
    fn new(formats: &[FormatInfo], strict: bool) -> Self {
        let built_in_formats = formats
            .iter()
            .filter(|format| !format_is_external(format.id))
            .count();
        let external_formats = formats.len().saturating_sub(built_in_formats);
        let ready_formats = formats
            .iter()
            .filter(|format| format_runtime_ready(format))
            .count();
        let missing_formats = formats.len().saturating_sub(ready_formats);
        let checks = vec![
            built_in_check(formats),
            sevenzip_check(formats, strict),
            wim_write_check(formats, strict),
            sqz_recovery_check(formats),
            par2_create_check(strict),
            par2_verify_repair_check(),
            rar_boundary_check(formats),
        ];
        let ok = checks.iter().all(|check| check.status != CheckStatus::Fail);
        Self {
            ok,
            strict,
            total_formats: formats.len(),
            built_in_formats,
            external_formats,
            ready_formats,
            missing_formats,
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
struct DoctorCheck {
    id: &'static str,
    status: CheckStatus,
    scope: String,
    detail: String,
    strict_required: bool,
    formats: Vec<String>,
    availability: Option<Value>,
}

impl DoctorCheck {
    fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "status": self.status.as_str(),
            "strict_required": self.strict_required,
            "scope": self.scope,
            "detail": self.detail,
            "formats": self.formats,
            "availability": self.availability,
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

fn built_in_check(formats: &[FormatInfo]) -> DoctorCheck {
    let built_in = formats
        .iter()
        .filter(|format| !format_is_external(format.id))
        .map(|format| format.id.to_owned())
        .collect::<Vec<_>>();
    DoctorCheck {
        id: "built-in-formats",
        status: CheckStatus::Pass,
        scope: "zip/tar/7z/sqz/stream codecs".to_owned(),
        detail: "built-in Rust engines are available without external tools".to_owned(),
        strict_required: true,
        formats: built_in,
        availability: Some(json!({"available": true, "source": "built_in"})),
    }
}

fn sevenzip_check(formats: &[FormatInfo], strict: bool) -> DoctorCheck {
    let affected = formats
        .iter()
        .filter(|format| {
            let implementation = implementation_json(format.id);
            implementation["read"]["kind"].as_str() == Some("external_tool")
                && implementation["read"]["tools"]
                    .as_array()
                    .is_some_and(|tools| tools.iter().any(|tool| tool == "7zz"))
        })
        .map(|format| format.id.to_owned())
        .collect::<Vec<_>>();
    let availability = implementation_json("cab")["availability"]["read"].clone();
    let available = json_bool_field(&availability, "available");
    DoctorCheck {
        id: "7z-read-bridge",
        status: availability_status(available, strict),
        scope: "long-tail unpack/test bridge".to_owned(),
        detail: if available {
            availability_detail("7z bridge ready", &availability)
        } else {
            "install 7zz/7z or set SQUALLZ_7Z for long-tail unpack-only formats".to_owned()
        },
        strict_required: true,
        formats: affected,
        availability: Some(availability),
    }
}

fn wim_write_check(formats: &[FormatInfo], strict: bool) -> DoctorCheck {
    let present = formats.iter().any(|format| format.id == "wim");
    let availability = implementation_json("wim")["availability"]["write"].clone();
    let available = json_bool_field(&availability, "available");
    DoctorCheck {
        id: "wim-writer",
        status: availability_status(available, strict),
        scope: "WIM create".to_owned(),
        detail: if available {
            availability_detail("wimlib-imagex writer ready", &availability)
        } else {
            "install wimlib-imagex or set SQUALLZ_WIMLIB before creating WIM archives".to_owned()
        },
        strict_required: true,
        formats: if present {
            vec!["wim".to_owned()]
        } else {
            Vec::new()
        },
        availability: Some(availability),
    }
}

fn sqz_recovery_check(formats: &[FormatInfo]) -> DoctorCheck {
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
        availability: Some(json!({"available": present, "source": "built_in"})),
    }
}

fn par2_create_check(strict: bool) -> DoctorCheck {
    let availability = par2_availability();
    let available = json_bool_field(&availability, "available");
    DoctorCheck {
        id: "par2-create",
        status: availability_status(available, strict),
        scope: "external PAR2 sidecar create".to_owned(),
        detail: if available {
            availability_detail("PAR2 create tool ready", &availability)
        } else {
            "PAR2 create still needs par2cmdline-turbo, par2, par2cmdline, or SQUALLZ_PAR2"
                .to_owned()
        },
        strict_required: true,
        formats: vec!["par2".to_owned()],
        availability: Some(availability),
    }
}

fn par2_verify_repair_check() -> DoctorCheck {
    let availability = par2_availability();
    let available = json_bool_field(&availability, "available");
    DoctorCheck {
        id: "par2-verify-repair",
        status: CheckStatus::Pass,
        scope: "external PAR2 verify/repair".to_owned(),
        detail: if available {
            availability_detail("external PAR2 verify/repair ready", &availability)
        } else {
            "rust-par2 fallback is built in for verify/repair; create still needs an external tool"
                .to_owned()
        },
        strict_required: false,
        formats: vec!["par2".to_owned()],
        availability: Some(if available {
            availability
        } else {
            json!({
                "available": true,
                "source": "built_in_fallback",
                "selected": "rust-par2",
                "create_available": false,
            })
        }),
    }
}

fn rar_boundary_check(formats: &[FormatInfo]) -> DoctorCheck {
    let present = formats.iter().any(|format| format.id == "rar");
    let implementation = implementation_json("rar");
    let limits = json_array_len(&implementation["limitations"]);
    DoctorCheck {
        id: "rar-product-boundary",
        status: CheckStatus::Boundary,
        scope: "RAR unpack-only".to_owned(),
        detail: format!(
            "RAR is unpack-only through external 7zz/7z with bsdtar as a diagnostic fallback; RAR creation, RAR recovery records, encrypted/full multi-volume compatibility, and damaged RAR repair remain outside release claims ({limits} documented limitations)"
        ),
        strict_required: false,
        formats: if present {
            vec!["rar".to_owned(), "cbr".to_owned()]
        } else {
            Vec::new()
        },
        availability: Some(implementation["availability"]["read"].clone()),
    }
}

fn availability_status(available: bool, strict: bool) -> CheckStatus {
    match (available, strict) {
        (true, _) => CheckStatus::Pass,
        (false, true) => CheckStatus::Fail,
        (false, false) => CheckStatus::Warn,
    }
}

fn availability_detail(ready: &str, availability: &Value) -> String {
    let selected = selected_label(availability);
    let source = source_label(availability);
    format!("{ready} via {source}: {selected}")
}

fn format_runtime_ready(format: &FormatInfo) -> bool {
    if !format_is_external(format.id) {
        return true;
    }
    let implementation = implementation_json(format.id);
    let availability = &implementation["availability"];
    let read_required = format.capabilities.can_extract || format.capabilities.can_test;
    let write_required = format.capabilities.can_create;
    (!read_required || json_nested_bool_field(availability, "read", "available"))
        && (!write_required || json_nested_bool_field(availability, "write", "available"))
}

fn format_is_external(format_id: &str) -> bool {
    implementation_json(format_id)["status"].as_str() == Some("external_required")
}

fn json_bool_field(value: &Value, field: &str) -> bool {
    value.get(field).and_then(Value::as_bool) == Some(true)
}

fn json_nested_bool_field(value: &Value, parent: &str, field: &str) -> bool {
    value
        .get(parent)
        .and_then(|parent| parent.get(field))
        .and_then(Value::as_bool)
        == Some(true)
}

fn json_array_len(value: &Value) -> usize {
    match value.as_array() {
        Some(items) => items.len(),
        None => 0,
    }
}

fn selected_label(availability: &Value) -> &str {
    match availability
        .get("selected")
        .and_then(Value::as_str)
        .filter(|selected| !selected.is_empty())
    {
        Some(selected) => selected,
        None => "built-in",
    }
}

fn source_label(availability: &Value) -> &str {
    match availability.get("source").and_then(Value::as_str) {
        Some(source) => source,
        None => "runtime",
    }
}

fn par2_availability() -> Value {
    if let Some(configured) = env::var_os(PAR2_ENV) {
        let configured = PathBuf::from(configured);
        let exists = command_path_is_executable(&configured);
        return json!({
            "available": exists,
            "source": "env",
            "env": PAR2_ENV,
            "selected": configured.to_string_lossy(),
            "configured": true,
            "path_exists": exists,
            "tools": PAR2_TOOLS,
        });
    }
    for tool in PAR2_TOOLS {
        if let Some(path) = find_on_path(tool) {
            return json!({
                "available": true,
                "source": "path",
                "env": PAR2_ENV,
                "selected": path.to_string_lossy(),
                "configured": false,
                "path_exists": true,
                "tools": PAR2_TOOLS,
            });
        }
    }
    json!({
        "available": false,
        "source": null,
        "env": PAR2_ENV,
        "selected": null,
        "configured": false,
        "path_exists": false,
        "tools": PAR2_TOOLS,
    })
}

fn command_path_is_executable(path: &Path) -> bool {
    if path.components().count() > 1 || path.is_absolute() {
        return command_is_executable(path);
    }
    find_on_path(&path.to_string_lossy()).is_some()
}

fn find_on_path(name: &str) -> Option<PathBuf> {
    let path = env::var_os("PATH")?;
    for dir in env::split_paths(&path) {
        let candidate = dir.join(name);
        if command_is_executable(&candidate) {
            return Some(candidate);
        }
        #[cfg(windows)]
        {
            let candidate = dir.join(format!("{name}.exe"));
            if command_is_executable(&candidate) {
                return Some(candidate);
            }
        }
    }
    None
}

fn command_is_executable(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        match path.metadata() {
            Ok(metadata) => metadata.permissions().mode() & 0o111 != 0,
            Err(_) => false,
        }
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn print_classic(report: &DoctorReport) {
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

fn print_modern(ctx: &Ctx, report: &DoctorReport) {
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
