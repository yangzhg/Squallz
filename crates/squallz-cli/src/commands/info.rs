//! `sqz info`: supported formats and their capabilities.

use std::path::Path;

use serde_json::{json, Value};
use squallz_core::api::{CompressionLevel, FormatInfo, FormatKind};
use squallz_formats::{is_sevenzip_bridge_format, wim_compression_name};

use super::reports::print_pretty_json;
use super::runtime::{
    is_external, Availability, FormatOverview, RuntimeFacts, RuntimeNeed, RAR_LIMITATIONS,
};
use crate::commands::{Ctx, ModernStatusField, ModernTableColumn, ModernTableRow};
use crate::errors::CliError;
use crate::ui::{self, Tone};

pub fn run(ctx: &Ctx, json: bool) -> Result<(), CliError> {
    let formats = ctx.engine.supported_formats();
    let runtime = RuntimeFacts::capture();

    if json {
        let array: Vec<Value> = formats
            .iter()
            .map(|f| {
                json!({
                    "id": f.id,
                    "kind": match f.kind {
                        FormatKind::Archive => "archive",
                        FormatKind::Compressor => "compressor",
                    },
                    "extensions": f.extensions,
                    "capabilities": capabilities_json(f),
                    "implementation": implementation_json(f.id, &runtime),
                    "level_mapping": level_mapping_json(f.id, f.capabilities.can_create),
                })
            })
            .collect();
        print_pretty_json(&Value::Array(array))?;
        return Ok(());
    }

    if formats.is_empty() {
        println!("{}", ctx.loc.t("cli.info.empty"));
        return Ok(());
    }
    let overview = FormatOverview::new(&formats, &runtime);
    if ctx.is_modern() {
        print_modern(ctx, &overview, &runtime);
    } else {
        print_classic(ctx, &overview);
    }
    Ok(())
}

fn print_classic(ctx: &Ctx, overview: &FormatOverview<'_>) {
    let formats = overview.formats;
    println!("{}", ctx.loc.t("cli.info.classic.summary_title"));
    print_classic_value(ctx, ctx.loc.t("common.formats"), formats.len());
    print_classic_value(
        ctx,
        ctx.loc.t("cli.info.inventory.built_in"),
        overview.built_in_archives.len(),
    );
    print_classic_value(
        ctx,
        ctx.loc.t("cli.info.inventory.external"),
        overview.external_archives.len(),
    );
    print_classic_value(
        ctx,
        ctx.loc.t("cli.info.inventory.compressors"),
        overview.stream_codecs.len(),
    );
    print_classic_value(ctx, ctx.loc.t("cli.info.inventory.ready"), overview.ready);
    print_classic_value(
        ctx,
        ctx.loc.t("cli.info.inventory.needs_tools"),
        overview.missing,
    );
    println!();
    println!("{}", ctx.loc.t("cli.info.classic.coverage_title"));
    print_classic_wrapped_value(
        ctx,
        ctx.loc.t("cli.info.coverage.pack_unpack"),
        &format_ids(&overview.pack_unpack),
    );
    print_classic_wrapped_value(
        ctx,
        ctx.loc.t("cli.info.coverage.unpack_only"),
        &format_ids(&overview.unpack_only_archives),
    );
    print_classic_wrapped_value(
        ctx,
        ctx.loc.t("cli.info.coverage.streams"),
        &format_ids(&overview.stream_codecs),
    );
    println!();
    let id_label = ctx.loc.t("common.id");
    let kind_header = ctx.loc.t("common.kind");
    let extensions_header = ctx.loc.t("cli.info.col.extensions");
    let capabilities_header = ctx.loc.t("cli.info.col.capabilities");
    let backend_header = ctx.loc.t("cli.info.col.engine");
    println!(
        "{}",
        classic_info_line(
            &id_label,
            &kind_header,
            &extensions_header,
            &capabilities_header,
            &backend_header,
        )
    );
    for format in formats {
        let kind = kind_label(ctx, format.kind);
        let extensions = format.extensions.join(",");
        let capabilities = classic_capabilities(ctx, format);
        let backend = backend_detail(ctx, format.id);
        println!(
            "{}",
            classic_info_line(format.id, &kind, &extensions, &capabilities, &backend)
        );
    }
}

fn print_classic_value(ctx: &Ctx, key: String, value: impl std::fmt::Display) {
    print_classic_wrapped_value(ctx, key, &value.to_string());
}

fn print_classic_wrapped_value(_ctx: &Ctx, key: String, value: &str) {
    const KEY_WIDTH: usize = 22;
    const VALUE_WIDTH: usize = 86;

    let key = format!("{key}:");
    let mut lines = ui::wrap_words(value, VALUE_WIDTH).into_iter();
    if let Some(first) = lines.next() {
        println!("  {} {first}", ui::pad_end(&key, KEY_WIDTH));
    } else {
        println!("  {} -", ui::pad_end(&key, KEY_WIDTH));
    }
    for line in lines {
        println!("  {} {line}", ui::pad_end("", KEY_WIDTH));
    }
}

fn classic_info_line(
    id: &str,
    kind: &str,
    extensions: &str,
    capabilities: &str,
    backend: &str,
) -> String {
    const WIDTHS: [usize; 4] = [9, 11, 28, 46];

    format!(
        "{} {} {} {} {}",
        ui::pad_end(id, WIDTHS[0]),
        ui::pad_end(kind, WIDTHS[1]),
        ui::pad_end(extensions, WIDTHS[2]),
        ui::pad_end(capabilities, WIDTHS[3]),
        backend
    )
}

fn print_modern(ctx: &Ctx, overview: &FormatOverview<'_>, runtime: &RuntimeFacts) {
    let formats = overview.formats;
    ctx.print_modern_status_panel(
        &ctx.loc.t("cli.info.heading"),
        &ctx.loc.t("cli.info.runtime.ready"),
        Tone::Success,
        &modern_summary(ctx, overview),
        &[
            ModernStatusField::new(ctx.loc.t("common.count"), formats.len().to_string()),
            ModernStatusField::new(
                ctx.loc.t("cli.info.inventory.built_in"),
                overview.built_in_archives.len().to_string(),
            ),
            ModernStatusField::new(
                ctx.loc.t("cli.info.inventory.external"),
                overview.external_archives.len().to_string(),
            ),
            ModernStatusField::new(
                ctx.loc.t("cli.info.inventory.compressors"),
                overview.stream_codecs.len().to_string(),
            ),
            ModernStatusField::new(
                ctx.loc.t("cli.info.inventory.ready"),
                overview.ready.to_string(),
            ),
            ModernStatusField::new(
                ctx.loc.t("cli.info.inventory.needs_tools"),
                overview.missing.to_string(),
            ),
        ],
    );
    ctx.print_modern_table(
        &ctx.loc.t("cli.info.support_map_title"),
        &[
            ModernTableColumn::new(ctx.loc.t("common.lane"), 20),
            ModernTableColumn::new(ctx.loc.t("common.mode"), 20),
            ModernTableColumn::right(ctx.loc.t("common.readiness"), 8),
            ModernTableColumn::new(ctx.loc.t("common.risk"), 26),
            ModernTableColumn::new(ctx.loc.t("common.examples"), 30),
        ],
        &modern_support_map_rows(ctx, overview, runtime),
    );
    ctx.print_modern_table(
        &ctx.loc.t("cli.info.capability_lanes_title"),
        &[
            ModernTableColumn::new(ctx.loc.t("common.workflow"), 18),
            ModernTableColumn::right(ctx.loc.t("common.formats"), 8),
            ModernTableColumn::right(ctx.loc.t("cli.info.inventory.ready"), 9),
            ModernTableColumn::right(ctx.loc.t("cli.info.inventory.needs_tools"), 11),
            ModernTableColumn::new(ctx.loc.t("common.examples"), 42),
        ],
        &modern_capability_lane_rows(ctx, formats, runtime),
    );
    ctx.print_modern_wrapped_table(
        &ctx.loc.t("cli.info.output_title"),
        &[
            ModernTableColumn::new(ctx.loc.t("common.setting"), 16),
            ModernTableColumn::new(ctx.loc.t("common.current"), 48),
            ModernTableColumn::new(ctx.loc.t("common.effect"), 38),
        ],
        &modern_output_rows(ctx),
    );
    ctx.print_modern_table(
        &ctx.loc.t("cli.info.cheatsheet_title"),
        &[
            ModernTableColumn::new(ctx.loc.t("common.workflow"), 18),
            ModernTableColumn::new(ctx.loc.t("common.command"), 38),
            ModernTableColumn::new(ctx.loc.t("common.use"), 38),
        ],
        &modern_cheatsheet_rows(ctx),
    );
    println!(
        "{}",
        ctx.paint_stdout_tone(Tone::Secondary, &ctx.loc.t("cli.info.legend"))
    );
    print_modern_group(
        ctx,
        &overview.built_in_archives,
        runtime,
        "cli.info.group.built_in_archives",
    );
    print_modern_group(
        ctx,
        &overview.external_archives,
        runtime,
        "cli.info.group.external_archives",
    );
    print_modern_group(
        ctx,
        &overview.stream_codecs,
        runtime,
        "cli.info.group.compressors",
    );
}

fn print_modern_group(ctx: &Ctx, formats: &[&FormatInfo], runtime: &RuntimeFacts, title_key: &str) {
    if formats.is_empty() {
        return;
    }
    let rows = formats
        .iter()
        .map(|format| {
            ModernTableRow::with_tone(
                vec![
                    format.id.to_owned(),
                    dotted_extensions(format),
                    capability_matrix(format),
                    runtime_operation_detail(ctx, format, RuntimeNeed::Read, runtime),
                    runtime_operation_detail(ctx, format, RuntimeNeed::Write, runtime),
                    backend_detail(ctx, format.id),
                ],
                format_runtime_tone(format, runtime),
            )
        })
        .collect::<Vec<_>>();
    ctx.print_modern_wrapped_table_with_note(
        &format!("{} · {}", ctx.loc.t(title_key), rows.len()),
        Some(&modern_group_note(ctx, title_key)),
        &[
            ModernTableColumn::new(ctx.loc.t("common.id"), INFO_TABLE_WIDTHS[0]),
            ModernTableColumn::new(ctx.loc.t("cli.info.col.extensions"), INFO_TABLE_WIDTHS[1]),
            ModernTableColumn::new(ctx.loc.t("cli.info.col.matrix"), INFO_TABLE_WIDTHS[2]),
            ModernTableColumn::new(ctx.loc.t("cli.info.col.read"), INFO_TABLE_WIDTHS[3]),
            ModernTableColumn::new(ctx.loc.t("cli.info.col.write"), INFO_TABLE_WIDTHS[4]),
            ModernTableColumn::new(ctx.loc.t("cli.info.col.engine"), INFO_TABLE_WIDTHS[5]),
        ],
        &rows,
    );
}

fn kind_label(ctx: &Ctx, kind: FormatKind) -> String {
    match kind {
        FormatKind::Archive => ctx.loc.t("common.format.archive"),
        FormatKind::Compressor => ctx.loc.t("common.format.compressor"),
    }
}

fn dotted_extensions(format: &FormatInfo) -> String {
    format
        .extensions
        .iter()
        .map(|ext| format!(".{ext}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn capability_matrix(format: &FormatInfo) -> String {
    CapabilityLane::ALL
        .into_iter()
        .map(|lane| if lane.supported(format) { "✓" } else { "·" })
        .collect::<Vec<_>>()
        .join(" ")
}

fn implementation_status(ctx: &Ctx, format_id: &str) -> String {
    if is_external(format_id) {
        ctx.loc.t("cli.info.impl.external")
    } else {
        ctx.loc.t("cli.info.impl.built_in")
    }
}

fn backend_detail(ctx: &Ctx, format_id: &str) -> String {
    match format_id {
        "wim" => ctx.loc.t("cli.info.engine.wim"),
        "rar" => ctx.loc.t("cli.info.engine.rar"),
        id if is_sevenzip_bridge_format(id) => ctx.loc.t("cli.info.engine.7z"),
        _ => implementation_status(ctx, format_id),
    }
}

fn modern_group_note(ctx: &Ctx, key: &str) -> String {
    match key {
        "cli.info.group.built_in_archives" => ctx.loc.t("cli.info.note.built_in_archives"),
        "cli.info.group.external_archives" => ctx.loc.t("cli.info.note.external_archives"),
        "cli.info.group.compressors" => ctx.loc.t("cli.info.note.compressors"),
        _ => String::new(),
    }
}

fn modern_summary(ctx: &Ctx, overview: &FormatOverview<'_>) -> String {
    let built_in = overview.built_in.to_string();
    let external = overview.external.to_string();
    let total = overview.formats.len().to_string();
    ctx.loc.format(
        "cli.info.summary",
        &[
            ("total", &total),
            ("built_in", &built_in),
            ("external", &external),
        ],
    )
}

fn modern_output_rows(ctx: &Ctx) -> Vec<ModernTableRow> {
    let style = format!("{:?}", ctx.output_style).to_ascii_lowercase();
    let color = format!("{:?}", ctx.color).to_ascii_lowercase();
    let palette = format!("{:?}", ctx.accent).to_ascii_lowercase();
    vec![
        ModernTableRow::success(vec![
            ctx.loc.t("common.style"),
            format!("--style {style}"),
            ctx.loc.t("cli.info.output.style_description"),
        ]),
        ModernTableRow::new(vec![
            ctx.loc.t("cli.info.output.color_mode"),
            format!("--color {color}"),
            ctx.loc.t("cli.info.output.color_description"),
        ]),
        ModernTableRow::success(vec![
            ctx.loc.t("common.palette"),
            format!("--palette {palette}"),
            ctx.loc.t("cli.info.output.palette_description"),
        ]),
    ]
}

fn modern_support_map_rows(
    ctx: &Ctx,
    overview: &FormatOverview<'_>,
    runtime: &RuntimeFacts,
) -> Vec<ModernTableRow> {
    let archive_pack_unpack = overview
        .pack_unpack
        .iter()
        .copied()
        .filter(|format| format.kind == FormatKind::Archive)
        .collect::<Vec<_>>();
    let edit_update = overview
        .formats
        .iter()
        .filter(|format| format.capabilities.can_update)
        .collect::<Vec<_>>();

    vec![
        support_map_row(
            runtime,
            ctx.loc.t("cli.info.support.archive_pack"),
            ctx.loc.t("cli.info.support.mode.create_extract"),
            &archive_pack_unpack,
            ctx.loc.t("cli.info.support.risk.native_external"),
        ),
        support_map_row(
            runtime,
            ctx.loc.t("cli.info.support.stream_codecs"),
            ctx.loc.t("cli.info.support.mode.codec_streams"),
            &overview.stream_codecs,
            ctx.loc.t("cli.info.support.risk.built_in"),
        ),
        support_map_row(
            runtime,
            ctx.loc.t("cli.info.support.unpack_only"),
            ctx.loc.t("cli.info.support.mode.extract_test"),
            &overview.unpack_only_archives,
            ctx.loc.t("cli.info.support.risk.external_bridge"),
        ),
        support_map_row(
            runtime,
            ctx.loc.t("cli.info.support.edit_update"),
            ctx.loc.t("cli.info.support.mode.zip_entries"),
            &edit_update,
            ctx.loc.t("cli.info.support.risk.atomic_writer"),
        ),
        ModernTableRow::success(vec![
            ctx.loc.t("cli.info.support.recovery"),
            ctx.loc.t("cli.info.support.mode.recovery"),
            ctx.loc.t("cli.info.support.ready.release"),
            ctx.loc.t("cli.info.support.risk.recovery"),
            ctx.loc.t("cli.info.support.recovery_examples"),
        ]),
    ]
}

fn support_map_row(
    runtime: &RuntimeFacts,
    lane: String,
    mode: String,
    formats: &[&FormatInfo],
    risk: String,
) -> ModernTableRow {
    let ready = formats
        .iter()
        .filter(|format| runtime.format_ready(format))
        .count();
    let readiness = if formats.is_empty() {
        "-".to_owned()
    } else {
        format!("{ready}/{}", formats.len())
    };
    let cells = vec![lane, mode, readiness, risk, capability_examples(formats)];
    if ready == formats.len() {
        ModernTableRow::success(cells)
    } else {
        ModernTableRow::warning(cells)
    }
}

fn format_ids(formats: &[&FormatInfo]) -> String {
    if formats.is_empty() {
        return "-".to_owned();
    }
    formats
        .iter()
        .map(|format| format.id)
        .collect::<Vec<_>>()
        .join(", ")
}

fn modern_capability_lane_rows(
    ctx: &Ctx,
    formats: &[FormatInfo],
    runtime: &RuntimeFacts,
) -> Vec<ModernTableRow> {
    CapabilityLane::ALL
        .into_iter()
        .map(|lane| {
            let supported: Vec<&FormatInfo> = formats
                .iter()
                .filter(|format| lane.supported(format))
                .collect();
            let ready = supported
                .iter()
                .filter(|format| {
                    runtime
                        .availability(format.id, lane.runtime_need())
                        .available()
                })
                .count();
            let needs_tools = supported.len().saturating_sub(ready);
            let tone = if needs_tools == 0 {
                Tone::Success
            } else {
                Tone::Warning
            };
            ModernTableRow::with_tone(
                vec![
                    lane.label(ctx),
                    supported.len().to_string(),
                    ready.to_string(),
                    needs_tools.to_string(),
                    capability_examples(&supported),
                ],
                tone,
            )
        })
        .collect()
}

fn modern_cheatsheet_rows(ctx: &Ctx) -> Vec<ModernTableRow> {
    vec![
        ModernTableRow::success(vec![
            ctx.loc.t("cli.info.workflow.create"),
            ctx.loc.t("cli.info.cheatsheet.create_command"),
            ctx.loc.t("cli.info.cheatsheet.create"),
        ]),
        ModernTableRow::success(vec![
            ctx.loc.t("cli.info.workflow.extract"),
            ctx.loc.t("cli.info.cheatsheet.extract_command"),
            ctx.loc.t("cli.info.cheatsheet.extract"),
        ]),
        ModernTableRow::new(vec![
            ctx.loc.t("cli.info.cheatsheet.inspect"),
            ctx.loc.t("cli.info.cheatsheet.inspect_command"),
            ctx.loc.t("cli.info.cheatsheet.inspect_use"),
        ]),
        ModernTableRow::new(vec![
            ctx.loc.t("cli.info.workflow.test"),
            ctx.loc.t("cli.info.cheatsheet.test_command"),
            ctx.loc.t("cli.info.cheatsheet.test"),
        ]),
        ModernTableRow::new(vec![
            ctx.loc.t("cli.info.workflow.update"),
            ctx.loc.t("cli.info.cheatsheet.update_command"),
            ctx.loc.t("cli.info.cheatsheet.update"),
        ]),
        ModernTableRow::new(vec![
            ctx.loc.t("cli.info.cheatsheet.recover"),
            ctx.loc.t("cli.info.cheatsheet.recover_command"),
            ctx.loc.t("cli.info.cheatsheet.recover_use"),
        ]),
    ]
}

fn capability_examples(formats: &[&FormatInfo]) -> String {
    const MAX_EXAMPLES: usize = 7;
    if formats.is_empty() {
        return "-".to_owned();
    }
    let mut examples = formats
        .iter()
        .take(MAX_EXAMPLES)
        .map(|format| format.id.to_owned())
        .collect::<Vec<_>>();
    if formats.len() > MAX_EXAMPLES {
        examples.push(format!("+{}", formats.len() - MAX_EXAMPLES));
    }
    examples.join(", ")
}

fn classic_capabilities(ctx: &Ctx, format: &FormatInfo) -> String {
    let values = CapabilityLane::ALL
        .into_iter()
        .filter(|lane| lane.supported(format))
        .map(|lane| lane.classic_label(ctx))
        .collect::<Vec<_>>();
    if values.is_empty() {
        ctx.loc.t("cli.info.cap.none")
    } else {
        values.join(" ")
    }
}

fn runtime_operation_detail(
    ctx: &Ctx,
    format: &FormatInfo,
    need: RuntimeNeed,
    runtime: &RuntimeFacts,
) -> String {
    if !is_external(format.id) {
        let supported = match need {
            RuntimeNeed::Read => format.capabilities.can_extract || format.capabilities.can_test,
            RuntimeNeed::Write => format.capabilities.can_create,
        };
        return if supported {
            ctx.loc.t("cli.info.runtime.ready")
        } else {
            ctx.loc.t("cli.info.runtime.unsupported")
        };
    }

    availability_status_label(ctx, runtime.availability(format.id, need))
}

const INFO_TABLE_WIDTHS: [usize; 6] = [9, 26, 13, 16, 16, 36];

#[derive(Clone, Copy)]
enum CapabilityLane {
    Create,
    Extract,
    Test,
    Update,
    Split,
    Encrypt,
    EncryptNames,
}

impl CapabilityLane {
    const ALL: [Self; 7] = [
        Self::Create,
        Self::Extract,
        Self::Test,
        Self::Update,
        Self::Split,
        Self::Encrypt,
        Self::EncryptNames,
    ];

    fn json_key(self) -> &'static str {
        match self {
            Self::Create => "can_create",
            Self::Extract => "can_extract",
            Self::Test => "can_test",
            Self::Update => "can_update",
            Self::Split => "can_split",
            Self::Encrypt => "can_encrypt_data",
            Self::EncryptNames => "can_encrypt_names",
        }
    }

    fn classic_label(self, ctx: &Ctx) -> String {
        match self {
            Self::Create => ctx.loc.t("cli.info.cap.create"),
            Self::Extract => ctx.loc.t("cli.info.cap.extract"),
            Self::Test => ctx.loc.t("cli.info.cap.test"),
            Self::Update => ctx.loc.t("cli.info.cap.update"),
            Self::Split => ctx.loc.t("cli.info.cap.split"),
            Self::Encrypt => ctx.loc.t("cli.info.cap.encrypt"),
            Self::EncryptNames => ctx.loc.t("cli.info.cap.encrypt_names"),
        }
    }

    fn label(self, ctx: &Ctx) -> String {
        match self {
            Self::Create => ctx.loc.t("cli.info.workflow.create"),
            Self::Extract => ctx.loc.t("cli.info.workflow.extract"),
            Self::Test => ctx.loc.t("cli.info.workflow.test"),
            Self::Update => ctx.loc.t("cli.info.workflow.update"),
            Self::Split => ctx.loc.t("cli.info.workflow.split"),
            Self::Encrypt => ctx.loc.t("cli.info.workflow.encrypt"),
            Self::EncryptNames => ctx.loc.t("cli.info.workflow.encrypt_names"),
        }
    }

    fn runtime_need(self) -> RuntimeNeed {
        match self {
            Self::Extract | Self::Test => RuntimeNeed::Read,
            Self::Create | Self::Update | Self::Split | Self::Encrypt | Self::EncryptNames => {
                RuntimeNeed::Write
            }
        }
    }

    fn supported(self, format: &FormatInfo) -> bool {
        let caps = format.capabilities;
        match self {
            Self::Create => caps.can_create,
            Self::Extract => caps.can_extract,
            Self::Test => caps.can_test,
            Self::Update => caps.can_update,
            Self::Split => caps.can_split,
            Self::Encrypt => caps.can_encrypt_data,
            Self::EncryptNames => caps.can_encrypt_names,
        }
    }
}

fn capabilities_json(format: &FormatInfo) -> Value {
    CapabilityLane::ALL
        .into_iter()
        .map(|lane| (lane.json_key().to_owned(), json!(lane.supported(format))))
        .collect()
}

fn format_runtime_tone(format: &FormatInfo, runtime: &RuntimeFacts) -> Tone {
    if !runtime.format_ready(format) {
        Tone::Warning
    } else if !is_external(format.id) {
        Tone::Success
    } else {
        Tone::Secondary
    }
}

fn availability_status_label(ctx: &Ctx, availability: Availability<'_>) -> String {
    if matches!(availability, Availability::Unsupported) {
        return ctx.loc.t("cli.info.runtime.unsupported");
    }

    let hint = availability_tool_hint(availability);
    if availability.available() {
        let status = ctx.loc.t("cli.info.runtime.ready");
        if hint.is_empty() {
            status
        } else {
            format!("{status}({hint})")
        }
    } else {
        let status = ctx.loc.t("cli.info.runtime.missing");
        if hint.is_empty() {
            status
        } else {
            format!("{status}({hint})")
        }
    }
}

fn availability_tool_hint(availability: Availability<'_>) -> String {
    if let Some(selected) = availability
        .selected()
        .filter(|path| !path.as_os_str().is_empty())
    {
        return normalize_tool_hint(path_file_name_or_self(&selected.to_string_lossy()));
    }
    match availability.tools().first() {
        Some(tool) => normalize_tool_hint(tool),
        None => String::new(),
    }
}

fn normalize_tool_hint(tool: &str) -> String {
    match tool {
        "7zz" | "7z" | "7za" => "7z".to_owned(),
        "wimlib-imagex" => "wimlib".to_owned(),
        other => other.to_owned(),
    }
}

fn path_file_name_or_self(path: &str) -> &str {
    match Path::new(path).file_name().and_then(|name| name.to_str()) {
        Some(name) => name,
        None => path,
    }
}

fn implementation_json(format_id: &str, runtime: &RuntimeFacts) -> Value {
    let read = runtime.availability(format_id, RuntimeNeed::Read).to_json();
    let write = runtime
        .availability(format_id, RuntimeNeed::Write)
        .to_json();
    match format_id {
        "zip" => json!({
            "status": "built_in",
            "bundled": true,
            "read": {
                "kind": "rust",
            },
            "write": {
                "kind": "rust",
            },
            "optional_external": {
                "scope": "native_split_read",
                "tools": ["7zz", "7z", "7za"],
                "env": "SQUALLZ_7Z",
                "availability": runtime.sevenzip().to_json(),
            },
            "availability": {
                "read": read,
                "write": write,
            },
            "limitations": [
                {
                    "scope": "native_split_read",
                    "status": "external_required",
                    "reason": "native .z01/.zip sets are header-validated and staged privately, then read through 7zz/7z from any member",
                },
                {
                    "scope": "native_split_create",
                    "status": "built_in",
                    "reason": "ZIP creation can write PKWARE-compatible .z01/.z02/.../.zip sets without an external encoder",
                },
                {
                    "scope": "native_split_encrypted",
                    "status": "external_required",
                    "reason": "encrypted native split ZIP uses 7zz/7z with the password sent only through a short-lived stdin pipe, never through process arguments or environment variables",
                },
            ],
            "platforms": ["macos", "windows", "linux"],
            "release_gate": "native split ZIP creation requires a three-platform filesystem matrix; native split reading also requires a packaged-7z matrix",
        }),
        "wim" => json!({
            "status": "external_required",
            "bundled": false,
            "read": {
                "kind": "external_tool",
                "tools": ["7zz", "7z", "7za"],
            },
            "write": {
                "kind": "external_tool",
                "tools": ["wimlib-imagex"],
                "env": "SQUALLZ_WIMLIB",
            },
            "availability": {
                "read": read,
                "write": write,
            },
            "limitations": [
                {
                    "scope": "native_split_read",
                    "status": "external_required",
                    "reason": "native .swm sets are header-validated and staged privately, then read through 7zz/7z from any standard member",
                },
                {
                    "scope": "native_split_create",
                    "status": "external_required",
                    "reason": "Split WIM creation uses wimlib-imagex and publishes the standard first.swm, first2.swm, ... family transactionally",
                },
                {
                    "scope": "native_split_size",
                    "status": "format_limit",
                    "reason": "the requested part size is a target because one indivisible WIM resource can make a part larger",
                },
            ],
            "platforms": ["macos", "windows", "linux"],
            "release_gate": "broader WIM and Split WIM corpus plus three-platform wimlib/7z packaging and license review",
        }),
        "rar" => json!({
            "status": "external_required",
            "bundled": false,
            "read": {
                "kind": "external_tool",
                "tools": ["7zz", "7z", "7za"],
                "env": "SQUALLZ_7Z",
                "fallback_tools": ["bsdtar", "unrar"],
                "fallback_env": "SQUALLZ_BSDTAR",
                "rar7_decoder_env": "SQUALLZ_UNRAR",
            },
            "write": {
                "kind": "unsupported",
                "reason": "RAR creation and RAR recovery records are outside the launch scope",
            },
            "policy": rar_policy_json(),
            "availability": {
                "read": read,
                "rar7_v6_decoder": runtime.unrar().to_json(),
                "write": write,
            },
            "limitations": RAR_LIMITATIONS,
            "platforms": ["macos", "windows", "linux"],
            "release_gate": "licensed RAR compatibility matrix plus external tool packaging and license review",
        }),
        id if is_sevenzip_bridge_format(id) => json!({
            "status": "external_required",
            "bundled": false,
            "read": {
                "kind": "external_tool",
                "tools": ["7zz", "7z", "7za"],
                "env": "SQUALLZ_7Z",
            },
            "write": {
                "kind": "unsupported",
                "reason": "launch scope is unpack-only for this format",
            },
            "availability": {
                "read": read,
                "write": write,
            },
            "platforms": ["macos", "windows", "linux"],
            "release_gate": "real long-tail compatibility matrix plus 7z packaging and license review",
        }),
        _ => json!({
            "status": "built_in",
            "bundled": true,
            "read": {
                "kind": "rust",
            },
            "write": {
                "kind": "rust",
            },
            "availability": {
                "read": read,
                "write": write,
            },
            "platforms": ["macos", "windows", "linux"],
            "release_gate": null,
        }),
    }
}

fn rar_policy_json() -> Value {
    json!({
        "read_only": true,
        "bundled": false,
        "primary_tools": ["7zz", "7z", "7za"],
        "primary_env": "SQUALLZ_7Z",
        "fallback_tools": ["bsdtar", "unrar"],
        "fallback_env": "SQUALLZ_BSDTAR",
        "rar7_decoder_env": "SQUALLZ_UNRAR",
        "fallback_scopes": [
            "explicit_diagnostic",
            "validated_p7zip_16_02_rar5_single_file",
            "confirmed_unencrypted_rar7_v6_single_file",
            "single_file_no_password_7z_unavailable",
        ],
        "fallback_reason": "bsdtar remains explicit, can handle confirmed single-file RAR7 v6 input, and is selected for the legacy p7zip 16.02 RAR5 decoder gap only after exact regular-file path and size agreement; optional unrar only streams confirmed-unencrypted RAR7 v6 entries after 7zz/7z listing and volume validation; neither tool is bundled. When no executable 7zz/7z is available, single-file reads without a supplied password use bsdtar; native multi-volume and supplied-password reads still require 7zz/7z.",
        "native_multi_volume": {
            "read_only": true,
            "tools": ["7zz", "7z", "7za"],
            "optional_rar7_decoder": ["unrar"],
            "names": ["partN.rar", "partNNN.rar", ".rar/.r00-.r99"],
            "encrypted_read": "stdin_only_password_bridge",
            "status": "implemented_not_release_claimed",
        },
        "license_boundary": "Squallz does not link or bundle unrar code and does not create RAR archives; optional unrar is user-installed and read-only, while bundling 7zz/7z would require LGPL plus unRAR restriction notices, source/replacement path, and RAR creation must remain unsupported",
        "release_claim": "read-only public-sample subset; native multi-volume orchestration plus real macOS RAR5, legacy RAR4 old-style, and confirmed-unencrypted RAR7 v6 volume smokes are implemented, but broader historical RAR4 and encrypted/solid coverage, the full three-platform package matrix, and damaged RAR repair are not release-claimed",
    })
}

fn level_mapping_json(format_id: &str, can_create: bool) -> Value {
    if !can_create {
        return Value::Null;
    }
    json!({
        "cli_to_level": {
            "0": "store",
            "1": "fastest",
            "2": "fast",
            "3": "fast",
            "4": "normal",
            "5": "normal",
            "6": "normal",
            "7": "maximum",
            "8": "maximum",
            "9": "ultra",
        },
        "backend": backend_level_mapping(format_id),
    })
}

fn backend_level_mapping(format_id: &str) -> Value {
    match format_id {
        "zip" => json!({
            "store": "stored",
            "fastest": "deflate 1",
            "fast": "deflate 3",
            "normal": "deflate 6",
            "maximum": "deflate 8",
            "ultra": "deflate 9",
        }),
        "gzip" => json!({
            "store": "deflate 0",
            "fastest": "deflate 1",
            "fast": "deflate 3",
            "normal": "deflate 6",
            "maximum": "deflate 8",
            "ultra": "deflate 9",
        }),
        "bzip2" => json!({
            "store": "block 1",
            "fastest": "block 1",
            "fast": "block 3",
            "normal": "block 6",
            "maximum": "block 9",
            "ultra": "block 9",
        }),
        "xz" => json!({
            "store": "preset 0",
            "fastest": "preset 1",
            "fast": "preset 3",
            "normal": "preset 6",
            "maximum": "preset 8",
            "ultra": "preset 9",
        }),
        "zstd" => json!({
            "store": "level 1",
            "fastest": "level 1",
            "fast": "level 2",
            "normal": "level 3",
            "maximum": "level 12",
            "ultra": "level 19",
        }),
        "lz4" => json!({
            "store": "fast",
            "fastest": "fast",
            "fast": "fast",
            "normal": "fast",
            "maximum": "fast",
            "ultra": "fast",
        }),
        "brotli" => json!({
            "store": "quality 0",
            "fastest": "quality 1",
            "fast": "quality 4",
            "normal": "quality 6",
            "maximum": "quality 9",
            "ultra": "quality 11",
        }),
        "7z" => json!({
            "store": "copy",
            "fastest": "lzma2 preset 1",
            "fast": "lzma2 preset 3",
            "normal": "lzma2 preset 6",
            "maximum": "lzma2 preset 8",
            "ultra": "lzma2 preset 9",
        }),
        "wim" => json!({
            "store": wim_compression_name(CompressionLevel::Store),
            "fastest": wim_compression_name(CompressionLevel::Fastest),
            "fast": wim_compression_name(CompressionLevel::Fast),
            "normal": wim_compression_name(CompressionLevel::Normal),
            "maximum": wim_compression_name(CompressionLevel::Maximum),
            "ultra": wim_compression_name(CompressionLevel::Ultra),
        }),
        "sqz" => json!({
            "store": "depends on inner profile",
            "fastest": "depends on inner profile",
            "fast": "depends on inner profile",
            "normal": "depends on inner profile",
            "maximum": "depends on inner profile",
            "ultra": "depends on inner profile",
            "note": "SQZ and TAR inner profiles ignore compression level; ZIP, 7Z and Zstd inner profiles use their respective backend level mappings",
        }),
        _ => json!({
            "store": "no compression-level effect",
            "fastest": "no compression-level effect",
            "fast": "no compression-level effect",
            "normal": "no compression-level effect",
            "maximum": "no compression-level effect",
            "ultra": "no compression-level effect",
        }),
    }
}
