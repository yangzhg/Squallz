//! `sqz info`: supported formats and their capabilities.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use squallz_core::api::{FormatInfo, FormatKind};

use super::reports::print_pretty_json;
use crate::commands::{Ctx, ModernStatusField, ModernTableColumn, ModernTableRow};
use crate::errors::CliError;
use crate::ui::{self, Tone};

pub fn run(ctx: &Ctx, json: bool) -> Result<(), CliError> {
    let formats = ctx.engine.supported_formats();

    if json {
        let array: Vec<Value> = formats
            .iter()
            .map(|f| {
                let caps = f.capabilities;
                json!({
                    "id": f.id,
                    "kind": match f.kind {
                        FormatKind::Archive => "archive",
                        FormatKind::Compressor => "compressor",
                    },
                    "extensions": f.extensions,
                    "capabilities": {
                        "can_create": caps.can_create,
                        "can_extract": caps.can_extract,
                        "can_encrypt_data": caps.can_encrypt_data,
                        "can_encrypt_names": caps.can_encrypt_names,
                        "can_split": caps.can_split,
                        "can_update": caps.can_update,
                        "can_test": caps.can_test,
                    },
                    "implementation": implementation_json(f.id),
                    "level_mapping": level_mapping_json(f.id, caps.can_create),
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
    if ctx.is_modern() {
        print_modern(ctx, &formats);
    } else {
        print_classic(ctx, &formats);
    }
    Ok(())
}

fn print_classic(ctx: &Ctx, formats: &[FormatInfo]) {
    let built_in_archives = formats
        .iter()
        .filter(|format| format.kind == FormatKind::Archive && !is_external(format.id))
        .count();
    let external_archives = formats
        .iter()
        .filter(|format| format.kind == FormatKind::Archive && is_external(format.id))
        .count();
    let compressors = formats
        .iter()
        .filter(|format| format.kind == FormatKind::Compressor)
        .count();
    let runtime_ready = formats
        .iter()
        .filter(|format| !format_has_missing_required_runtime(format))
        .count();
    let runtime_missing = formats.len().saturating_sub(runtime_ready);
    let pack_unpack = formats
        .iter()
        .filter(|format| format.capabilities.can_create && format.capabilities.can_extract)
        .collect::<Vec<_>>();
    let unpack_only = formats
        .iter()
        .filter(|format| {
            format.kind == FormatKind::Archive
                && !format.capabilities.can_create
                && format.capabilities.can_extract
        })
        .collect::<Vec<_>>();
    let stream_codecs = formats
        .iter()
        .filter(|format| format.kind == FormatKind::Compressor)
        .collect::<Vec<_>>();

    println!(
        "{}",
        label(ctx, "cli.info.classic.summary_title", "Summary")
    );
    print_classic_value(ctx, label(ctx, "common.formats", "Formats"), formats.len());
    print_classic_value(
        ctx,
        label(ctx, "cli.info.inventory.built_in", "Built-in archives"),
        built_in_archives,
    );
    print_classic_value(
        ctx,
        label(ctx, "cli.info.inventory.external", "External bridges"),
        external_archives,
    );
    print_classic_value(
        ctx,
        label(ctx, "cli.info.inventory.compressors", "Compressors"),
        compressors,
    );
    print_classic_value(
        ctx,
        label(ctx, "cli.info.inventory.ready", "Ready now"),
        runtime_ready,
    );
    print_classic_value(
        ctx,
        label(ctx, "cli.info.inventory.needs_tools", "Needs tools"),
        runtime_missing,
    );
    println!();
    println!(
        "{}",
        label(ctx, "cli.info.classic.coverage_title", "Coverage")
    );
    print_classic_wrapped_value(
        ctx,
        label(ctx, "cli.info.coverage.pack_unpack", "Pack / unpack"),
        &format_ids(&pack_unpack),
    );
    print_classic_wrapped_value(
        ctx,
        label(ctx, "cli.info.coverage.unpack_only", "Unpack only"),
        &format_ids(&unpack_only),
    );
    print_classic_wrapped_value(
        ctx,
        label(ctx, "cli.info.coverage.streams", "Stream codecs"),
        &format_ids(&stream_codecs),
    );
    println!();
    let id_label = label(ctx, "common.id", "ID");
    let kind_header = label(ctx, "common.kind", "Kind");
    let extensions_header = label(ctx, "cli.info.col.extensions", "Extensions");
    let capabilities_header = label(ctx, "cli.info.col.capabilities", "Capabilities");
    let backend_header = label(ctx, "cli.info.col.engine", "Engine");
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

fn print_modern(ctx: &Ctx, formats: &[FormatInfo]) {
    let built_in_archives = formats
        .iter()
        .filter(|format| format.kind == FormatKind::Archive && !is_external(format.id))
        .count();
    let external_archives = formats
        .iter()
        .filter(|format| format.kind == FormatKind::Archive && is_external(format.id))
        .count();
    let compressors = formats
        .iter()
        .filter(|format| format.kind == FormatKind::Compressor)
        .count();
    let runtime_ready = formats
        .iter()
        .filter(|format| !format_has_missing_required_runtime(format))
        .count();
    let runtime_missing = formats.len().saturating_sub(runtime_ready);
    ctx.print_modern_status_panel(
        &label(ctx, "cli.info.heading", "Supported formats"),
        &label(ctx, "cli.info.runtime.ready", "ready"),
        Tone::Success,
        &modern_summary(ctx, formats),
        &[
            ModernStatusField::new(
                label(ctx, "common.count", "Count"),
                formats.len().to_string(),
            ),
            ModernStatusField::new(
                label(ctx, "cli.info.inventory.built_in", "Built-in"),
                built_in_archives.to_string(),
            ),
            ModernStatusField::new(
                label(ctx, "cli.info.inventory.external", "External bridges"),
                external_archives.to_string(),
            ),
            ModernStatusField::new(
                label(ctx, "cli.info.inventory.compressors", "Compressors"),
                compressors.to_string(),
            ),
            ModernStatusField::new(
                label(ctx, "cli.info.inventory.ready", "Ready now"),
                runtime_ready.to_string(),
            ),
            ModernStatusField::new(
                label(ctx, "cli.info.inventory.needs_tools", "Needs tools"),
                runtime_missing.to_string(),
            ),
        ],
    );
    ctx.print_modern_wrapped_table(
        &label(ctx, "cli.info.command_forms_title", "Command forms"),
        &[
            ModernTableColumn::new(label(ctx, "common.command", "Command"), 20),
            ModernTableColumn::new(label(ctx, "common.form", "Form"), 34),
            ModernTableColumn::new(label(ctx, "common.detail", "Detail"), 42),
            ModernTableColumn::new(label(ctx, "common.best_for", "Best for"), 30),
        ],
        &modern_command_form_rows(),
    );
    ctx.print_modern_wrapped_table(
        &label(ctx, "cli.info.dashboard_title", "Modern dashboard"),
        &[
            ModernTableColumn::new(label(ctx, "common.signal", "Signal"), 18),
            ModernTableColumn::right(label(ctx, "common.count", "Count"), 8),
            ModernTableColumn::new(label(ctx, "common.form", "Form"), 26),
            ModernTableColumn::new(label(ctx, "common.detail", "Detail"), 44),
        ],
        &modern_dashboard_rows(
            ctx,
            formats,
            built_in_archives,
            external_archives,
            compressors,
            runtime_ready,
            runtime_missing,
        ),
    );
    ctx.print_modern_table(
        &label(ctx, "cli.info.support_map_title", "Support map"),
        &[
            ModernTableColumn::new(label(ctx, "common.lane", "Lane"), 20),
            ModernTableColumn::new(label(ctx, "common.mode", "Mode"), 20),
            ModernTableColumn::right(label(ctx, "common.readiness", "Ready"), 8),
            ModernTableColumn::new(label(ctx, "common.risk", "Risk"), 26),
            ModernTableColumn::new(label(ctx, "common.examples", "Examples"), 30),
        ],
        &modern_support_map_rows(ctx, formats),
    );
    ctx.print_modern_wrapped_table(
        &label(ctx, "cli.info.coverage_title", "Format coverage"),
        &[
            ModernTableColumn::new(label(ctx, "common.workflow", "Workflow"), 18),
            ModernTableColumn::right(label(ctx, "common.count", "Count"), 7),
            ModernTableColumn::new(label(ctx, "common.runtime", "Runtime"), 20),
            ModernTableColumn::new(label(ctx, "common.formats", "Formats"), 64),
        ],
        &modern_format_coverage_rows(ctx, formats),
    );
    ctx.print_modern_table(
        &label(ctx, "cli.info.capability_lanes_title", "Capability lanes"),
        &[
            ModernTableColumn::new(label(ctx, "common.workflow", "Workflow"), 18),
            ModernTableColumn::right(label(ctx, "common.formats", "Formats"), 8),
            ModernTableColumn::right(label(ctx, "cli.info.inventory.ready", "Ready now"), 9),
            ModernTableColumn::right(
                label(ctx, "cli.info.inventory.needs_tools", "Needs tools"),
                11,
            ),
            ModernTableColumn::new(label(ctx, "common.examples", "Examples"), 42),
        ],
        &modern_capability_lane_rows(ctx, formats),
    );
    ctx.print_modern_wrapped_table(
        &label(ctx, "cli.info.action_selector_title", "Action selector"),
        &[
            ModernTableColumn::new(label(ctx, "common.goal", "Goal"), 18),
            ModernTableColumn::new(label(ctx, "common.best_form", "Best form"), 34),
            ModernTableColumn::new(label(ctx, "common.command", "Command"), 34),
            ModernTableColumn::new(label(ctx, "common.reason", "Reason"), 34),
        ],
        &modern_action_selector_rows(ctx),
    );
    ctx.print_modern_wrapped_table(
        &label(ctx, "cli.info.surface_gallery_title", "Modern surfaces"),
        &[
            ModernTableColumn::new(label(ctx, "common.workflow", "Workflow"), 18),
            ModernTableColumn::new(label(ctx, "common.live_form", "Live form"), 34),
            ModernTableColumn::new(label(ctx, "common.result_form", "Result form"), 34),
            ModernTableColumn::new(label(ctx, "common.best_for", "Best for"), 34),
        ],
        &modern_surface_rows(ctx),
    );
    ctx.print_modern_wrapped_table(
        &label(ctx, "cli.info.output_title", "Modern output"),
        &[
            ModernTableColumn::new(label(ctx, "common.setting", "Setting"), 16),
            ModernTableColumn::new(label(ctx, "common.current", "Current"), 48),
            ModernTableColumn::new(label(ctx, "common.effect", "Effect"), 38),
        ],
        &modern_output_rows(ctx),
    );
    ctx.print_modern_wrapped_table(
        &label(ctx, "cli.info.style_guide_title", "Modern style guide"),
        &[
            ModernTableColumn::new(label(ctx, "common.scenario", "Scenario"), 20),
            ModernTableColumn::new(label(ctx, "common.best_form", "Best form"), 34),
            ModernTableColumn::new(label(ctx, "common.rendered_as", "Rendered as"), 42),
            ModernTableColumn::new(label(ctx, "common.example", "Example"), 38),
        ],
        &modern_style_guide_rows(ctx),
    );
    ctx.print_modern_wrapped_table(
        &label(ctx, "cli.info.palette_gallery_title", "Palette gallery"),
        &[
            ModernTableColumn::new(label(ctx, "common.palette", "Palette"), 16),
            ModernTableColumn::new(label(ctx, "common.command", "Command"), 24),
            ModernTableColumn::new(label(ctx, "common.look", "Look"), 34),
            ModernTableColumn::new(label(ctx, "common.best_for", "Best for"), 34),
        ],
        &modern_palette_rows(ctx),
    );
    ctx.print_modern_table(
        &label(ctx, "cli.info.inventory_title", "Runtime inventory"),
        &[
            ModernTableColumn::new(label(ctx, "common.scope", "Scope"), 24),
            ModernTableColumn::right(label(ctx, "common.count", "Count"), 8),
            ModernTableColumn::new(label(ctx, "common.runtime", "Runtime"), 58),
        ],
        &modern_inventory_rows(ctx, formats),
    );
    ctx.print_modern_table(
        &label(ctx, "cli.info.cheatsheet_title", "Command cheatsheet"),
        &[
            ModernTableColumn::new(label(ctx, "common.workflow", "Workflow"), 18),
            ModernTableColumn::new(label(ctx, "common.command", "Command"), 38),
            ModernTableColumn::new(label(ctx, "common.use", "Use"), 38),
        ],
        &modern_cheatsheet_rows(ctx),
    );
    println!(
        "{}",
        ctx.paint_stdout_tone(
            Tone::Secondary,
            &label(
                ctx,
                "cli.info.legend",
                "Legend: C=create X=extract T=test U=update S=split E=encrypt N=hide names; ✓ supported, · unavailable.",
            )
        )
    );
    print_modern_group(ctx, formats, "cli.info.group.built_in_archives", |format| {
        format.kind == FormatKind::Archive && !is_external(format.id)
    });
    print_modern_group(ctx, formats, "cli.info.group.external_archives", |format| {
        format.kind == FormatKind::Archive && is_external(format.id)
    });
    print_modern_group(ctx, formats, "cli.info.group.compressors", |format| {
        format.kind == FormatKind::Compressor
    });
}

fn print_modern_group(
    ctx: &Ctx,
    formats: &[FormatInfo],
    title_key: &str,
    include: impl Fn(&FormatInfo) -> bool,
) {
    let rows: Vec<&FormatInfo> = formats.iter().filter(|format| include(format)).collect();
    if rows.is_empty() {
        return;
    }
    let rows = rows
        .into_iter()
        .map(|format| {
            ModernTableRow::with_tone(
                vec![
                    format.id.to_owned(),
                    dotted_extensions(format),
                    capability_matrix(format),
                    runtime_operation_detail(ctx, format, "read"),
                    runtime_operation_detail(ctx, format, "write"),
                    backend_detail(ctx, format.id),
                ],
                format_runtime_tone(format),
            )
        })
        .collect::<Vec<_>>();
    ctx.print_modern_wrapped_table_with_note(
        &format!("{} · {}", group_label(ctx, title_key), rows.len()),
        Some(&modern_group_note(ctx, title_key)),
        &[
            ModernTableColumn::new(label(ctx, "common.id", "ID"), INFO_TABLE_WIDTHS[0]),
            ModernTableColumn::new(
                label(ctx, "cli.info.col.extensions", "Extensions"),
                INFO_TABLE_WIDTHS[1],
            ),
            ModernTableColumn::new(
                label(ctx, "cli.info.col.matrix", "C X T U S E N"),
                INFO_TABLE_WIDTHS[2],
            ),
            ModernTableColumn::new(
                label(ctx, "cli.info.col.read", "Read"),
                INFO_TABLE_WIDTHS[3],
            ),
            ModernTableColumn::new(
                label(ctx, "cli.info.col.write", "Write"),
                INFO_TABLE_WIDTHS[4],
            ),
            ModernTableColumn::new(
                label(ctx, "cli.info.col.engine", "Engine"),
                INFO_TABLE_WIDTHS[5],
            ),
        ],
        &rows,
    );
}

fn kind_label(ctx: &Ctx, kind: FormatKind) -> String {
    match kind {
        FormatKind::Archive => label(ctx, "common.format.archive", "archive"),
        FormatKind::Compressor => label(ctx, "common.format.compressor", "compressor"),
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
    let caps = format.capabilities;
    [
        caps.can_create,
        caps.can_extract,
        caps.can_test,
        caps.can_update,
        caps.can_split,
        caps.can_encrypt_data,
        caps.can_encrypt_names,
    ]
    .into_iter()
    .map(|supported| if supported { "✓" } else { "·" })
    .collect::<Vec<_>>()
    .join(" ")
}

fn implementation_status(ctx: &Ctx, format_id: &str) -> String {
    if is_external(format_id) {
        label(ctx, "cli.info.impl.external", "external")
    } else {
        label(ctx, "cli.info.impl.built_in", "built-in")
    }
}

fn backend_detail(ctx: &Ctx, format_id: &str) -> String {
    match format_id {
        "wim" => label(
            ctx,
            "cli.info.engine.wim",
            "external: 7zz read; wimlib write",
        ),
        "rar" => label(
            ctx,
            "cli.info.engine.rar",
            "external: 7zz/7z; bsdtar diagnostic fallback",
        ),
        id if long_tail_7z_bridge_format(id) => {
            label(ctx, "cli.info.engine.7z", "external: 7zz/7z")
        }
        _ => implementation_status(ctx, format_id),
    }
}

fn group_label(ctx: &Ctx, key: &str) -> String {
    match key {
        "cli.info.group.built_in_archives" => label(ctx, key, "Built-in archives"),
        "cli.info.group.external_archives" => label(ctx, key, "External archive bridges"),
        "cli.info.group.compressors" => label(ctx, key, "Stream compressors"),
        _ => label(ctx, key, key),
    }
}

fn modern_group_note(ctx: &Ctx, key: &str) -> String {
    match key {
        "cli.info.group.built_in_archives" => label(
            ctx,
            "cli.info.note.built_in_archives",
            "Ready now; no external tools required.",
        ),
        "cli.info.group.external_archives" => label(
            ctx,
            "cli.info.note.external_archives",
            "Runtime shows this machine's external tools; --json exposes exact paths.",
        ),
        "cli.info.group.compressors" => label(
            ctx,
            "cli.info.note.compressors",
            "Stream codecs are built in and available cross-platform.",
        ),
        _ => String::new(),
    }
}

fn modern_summary(ctx: &Ctx, formats: &[FormatInfo]) -> String {
    let built_in = formats
        .iter()
        .filter(|format| !is_external(format.id))
        .count()
        .to_string();
    let external = formats
        .iter()
        .filter(|format| is_external(format.id))
        .count()
        .to_string();
    let total = formats.len().to_string();
    ctx.loc.format(
        "cli.info.summary",
        &[
            ("total", &total),
            ("built_in", &built_in),
            ("external", &external),
        ],
    )
}

#[allow(clippy::too_many_arguments)]
fn modern_dashboard_rows(
    ctx: &Ctx,
    formats: &[FormatInfo],
    built_in_archives: usize,
    external_archives: usize,
    compressors: usize,
    runtime_ready: usize,
    runtime_missing: usize,
) -> Vec<ModernTableRow> {
    let pack_unpack = formats
        .iter()
        .filter(|format| format.capabilities.can_create && format.capabilities.can_extract)
        .count();
    let unpack_only = formats
        .iter()
        .filter(|format| !format.capabilities.can_create && format.capabilities.can_extract)
        .count();
    vec![
        ModernTableRow::success(vec![
            label(ctx, "cli.info.dashboard.ready", "Ready now"),
            format!("{runtime_ready}/{}", formats.len()),
            "scorecard + support map".to_owned(),
            format!(
                "{} built-in archives, {external_archives} external bridges, {compressors} stream codecs",
                built_in_archives
            ),
        ]),
        ModernTableRow::success(vec![
            label(ctx, "cli.info.dashboard.pack_unpack", "Pack / unpack"),
            pack_unpack.to_string(),
            "capability matrix".to_owned(),
            "create, extract, test, split, encrypt, and name-hiding lanes stay visible".to_owned(),
        ]),
        ModernTableRow::new(vec![
            label(ctx, "cli.info.dashboard.unpack_only", "Unpack only"),
            unpack_only.to_string(),
            "bridge table".to_owned(),
            "RAR/WIM and long-tail formats show runtime tool readiness instead of hidden notes"
                .to_owned(),
        ]),
        ModernTableRow::with_tone(
            vec![
                label(ctx, "cli.info.dashboard.live", "Live jobs"),
                if runtime_missing == 0 {
                    "clear".to_owned()
                } else {
                    format!("{runtime_missing} risks")
                },
                "operation cockpit + signal matrix + transfer matrix + action queue".to_owned(),
                "compress/extract HUD shows phase rail, progress, signal matrix, throughput, payload, guardrail, speed/ETA/current, elapsed time, next step, and action queue"
                    .to_owned(),
            ],
            if runtime_missing == 0 {
                Tone::Success
            } else {
                Tone::Warning
            },
        ),
    ]
}

fn modern_command_form_rows() -> Vec<ModernTableRow> {
    vec![
        ModernTableRow::success(vec![
            "info".to_owned(),
            "scorecard + decision tables".to_owned(),
            "format coverage, capability lanes, runtime readiness, and palette controls".to_owned(),
            "choosing the right archive route before doing work".to_owned(),
        ]),
        ModernTableRow::success(vec![
            "compress / pack".to_owned(),
            "result panel + create plan + detail tables".to_owned(),
            "output path, format, level/profile, volume mode, recovery mode, and verify-next cue"
                .to_owned(),
            "archive creation where the result must be inspectable immediately".to_owned(),
        ]),
        ModernTableRow::success(vec![
            "extract".to_owned(),
            "destination panel + policy tables".to_owned(),
            "archive, destination, selection, overwrite, symlink, smart layout, and safety limits"
                .to_owned(),
            "safe unpacking without hiding risk controls".to_owned(),
        ]),
        ModernTableRow::new(vec![
            "list / test".to_owned(),
            "summary card + focused table".to_owned(),
            "entry mix, content rows, integrity status, and problem details when present".to_owned(),
            "inspection and verification before extraction or release".to_owned(),
        ]),
        ModernTableRow::new(vec![
            "live TTY progress".to_owned(),
            "operation cockpit + signal matrix + transfer matrix + action queue".to_owned(),
            "phase rail, gauge, signal matrix, throughput, payload, guardrail, speed/ETA/current, elapsed time, next cue, finish cue, and current object"
                .to_owned(),
            "compress/extract jobs where progress must be readable at a glance".to_owned(),
        ]),
        ModernTableRow::new(vec![
            "--json / classic".to_owned(),
            "stable machine contract / conservative rows".to_owned(),
            "no ANSI, no box drawing, no redraw codes, and no modern-only reshaping".to_owned(),
            "scripts, CI, logs, and terminals that need predictable plain text".to_owned(),
        ]),
    ]
}

fn modern_action_selector_rows(ctx: &Ctx) -> Vec<ModernTableRow> {
    vec![
        ModernTableRow::success(vec![
            label(ctx, "cli.info.action.pack", "Pack files"),
            label(
                ctx,
                "cli.info.action.pack_form",
                "result panel + metrics table",
            ),
            "sqz compress input -o out.zip --style modern".to_owned(),
            label(
                ctx,
                "cli.info.action.pack_reason",
                "best when you need an immediate output path, size, volume count, and verify-next cue",
            ),
        ]),
        ModernTableRow::success(vec![
            label(ctx, "cli.info.action.unpack", "Unpack safely"),
            label(
                ctx,
                "cli.info.action.unpack_form",
                "policy table + destination route",
            ),
            "sqz extract archive -d out --smart --style modern".to_owned(),
            label(
                ctx,
                "cli.info.action.unpack_reason",
                "best when overwrite, symlink, smart-folder, and safety limits must stay visible",
            ),
        ]),
        ModernTableRow::new(vec![
            label(ctx, "cli.info.action.inspect", "Inspect support"),
            label(
                ctx,
                "cli.info.action.inspect_form",
                "support map + format coverage + runtime inventory",
            ),
            "sqz info --style modern".to_owned(),
            label(
                ctx,
                "cli.info.action.inspect_reason",
                "best when choosing a format or checking whether a bridge runtime is available",
            ),
        ]),
        ModernTableRow::success(vec![
            label(ctx, "cli.info.action.live", "Watch a live job"),
            label(
                ctx,
                "cli.info.action.live_form",
                "operation cockpit + signal matrix + transfer matrix + action queue",
            ),
            "interactive TTY stderr".to_owned(),
            label(
                ctx,
                "cli.info.action.live_reason",
                "best for compression/extraction where phase, ETA, elapsed time, current object, and next step matter",
            ),
        ]),
        ModernTableRow::new(vec![
            label(ctx, "cli.info.action.automation", "Automate"),
            label(
                ctx,
                "cli.info.action.automation_form",
                "classic rows or JSON without ANSI/redraw",
            ),
            "sqz <cmd> --json".to_owned(),
            label(
                ctx,
                "cli.info.action.automation_reason",
                "best for shell scripts, CI, and predictable parsing",
            ),
        ]),
    ]
}

fn modern_surface_rows(ctx: &Ctx) -> Vec<ModernTableRow> {
    vec![
        ModernTableRow::success(vec![
            label(ctx, "cli.info.surface.compress", "Compress / pack"),
            "TTY operation cockpit with phase rail, progress gauge, signal matrix, transfer board, transfer matrix, action queue, speed, ETA, elapsed time, and current object"
                .to_owned(),
            "status panel + route table + detail tables".to_owned(),
            "watching long archive creation without losing the next verification cue".to_owned(),
        ]),
        ModernTableRow::success(vec![
            label(ctx, "cli.info.surface.extract", "Extract"),
            "TTY operation cockpit with stream-aware gauge, signal matrix, transfer matrix, destination route cue, and action queue".to_owned(),
            "destination summary + policy table + safety limits".to_owned(),
            "unpacking where overwrite, symlink, smart folder, and skipped entries must stay visible"
                .to_owned(),
        ]),
        ModernTableRow::new(vec![
            label(ctx, "cli.info.surface.info", "Info"),
            "no live redraw".to_owned(),
            "support map + coverage tables + palette gallery".to_owned(),
            "choosing a format, runtime bridge, or modern terminal color before running work"
                .to_owned(),
        ]),
        ModernTableRow::new(vec![
            label(ctx, "cli.info.surface.doctor", "Doctor"),
            "no live redraw".to_owned(),
            "runtime checks table + limitation notes".to_owned(),
            "checking external tools, RAR boundaries, WIM tooling, and product support limits"
                .to_owned(),
        ]),
        ModernTableRow::new(vec![
            label(ctx, "cli.info.surface.automation", "Automation"),
            "classic stderr or quiet".to_owned(),
            "raw JSON without ANSI, boxes, or redraw control codes".to_owned(),
            "scripts, CI, and logs where parseability matters more than presentation".to_owned(),
        ]),
    ]
}

fn modern_output_rows(ctx: &Ctx) -> Vec<ModernTableRow> {
    let style = format!("{:?}", ctx.output_style).to_ascii_lowercase();
    let color = format!("{:?}", ctx.color).to_ascii_lowercase();
    let palette = format!("{:?}", ctx.accent).to_ascii_lowercase();
    vec![
        ModernTableRow::success(vec![
            label(ctx, "common.style", "Style"),
            style,
            "modern tables, routed panels, grouped summaries, and live HUDs".to_owned(),
        ]),
        ModernTableRow::new(vec![
            label(ctx, "cli.info.output.color_mode", "Color mode"),
            color,
            "auto follows TTY and NO_COLOR; rich/fancy force modern ANSI for screenshots, redirected demos, and live progress previews; always/never remain explicit".to_owned(),
        ]),
        ModernTableRow::success(vec![
            label(ctx, "common.palette", "Palette"),
            palette,
            "use --palette / --theme / --colors to switch modern terminal colors; surge keeps the approved teal primary with a vivid sky-blue HUD accent".to_owned(),
        ]),
        ModernTableRow::success(vec![
            label(ctx, "common.color_scheme", "Color scheme"),
            "--color-scheme / --scheme / --colors".to_owned(),
            "visible aliases for the same modern palette control; JSON and classic output remain unchanged".to_owned(),
        ]),
        ModernTableRow::success(vec![
            label(ctx, "cli.info.output.progress_hud", "Progress HUD"),
            "operation cockpit + snapshot dashboard + signal matrix + transfer matrix + action queue"
                .to_owned(),
            "compress, extract, pack, update, and repair show a table-form snapshot dashboard with progress, payload, current object, mini gauge, throughput, guardrail, speed/ETA/current, elapsed time, next step, and action queue when stderr is a TTY"
                .to_owned(),
        ]),
        ModernTableRow::new(vec![
            label(ctx, "common.preview", "Preview"),
            "primary / secondary".to_owned(),
            "success, warning, and error tones stay semantic across palettes".to_owned(),
        ]),
    ]
}

fn modern_style_guide_rows(ctx: &Ctx) -> Vec<ModernTableRow> {
    vec![
        ModernTableRow::success(vec![
            label(ctx, "cli.info.style.live", "Live archive work"),
            "operation cockpit".to_owned(),
            "progress gauge + snapshot dashboard table + phase rail + signal matrix + transfer board + transfer matrix + action queue"
                .to_owned(),
            "sqz compress input -o out.7z --style modern --color fancy".to_owned(),
        ]),
        ModernTableRow::success(vec![
            label(ctx, "cli.info.style.result", "Finished operation"),
            "status panel + result tables".to_owned(),
            "summary fields, route table, settings table, detail table, and verify-next cue"
                .to_owned(),
            "sqz extract archive -d out --style modern".to_owned(),
        ]),
        ModernTableRow::new(vec![
            label(ctx, "cli.info.style.discovery", "Capability discovery"),
            "scorecard + decision tables".to_owned(),
            "support map, format coverage, capability lanes, runtime inventory, and palette gallery"
                .to_owned(),
            "sqz info --style modern --color rich".to_owned(),
        ]),
        ModernTableRow::new(vec![
            label(ctx, "cli.info.style.audit", "Script or audit log"),
            "classic rows or JSON".to_owned(),
            "plain ASCII rows for humans, JSON for machines, no box drawing or redraw codes"
                .to_owned(),
            "sqz test archive --json".to_owned(),
        ]),
    ]
}

fn modern_palette_rows(ctx: &Ctx) -> Vec<ModernTableRow> {
    vec![
        ModernTableRow::success(vec![
            "brand".to_owned(),
            "--palette brand".to_owned(),
            "#2DD4BF primary + #0EA5E9 secondary".to_owned(),
            label(
                ctx,
                "cli.info.palette.best_brand",
                "short brand option for the approved app icon colors",
            ),
        ]),
        ModernTableRow::success(vec![
            "icon".to_owned(),
            "--colors icon".to_owned(),
            "#2DD4BF -> #0EA5E9 approved icon gradient".to_owned(),
            label(
                ctx,
                "cli.info.palette.best_icon",
                "explicitly use the selected app icon palette",
            ),
        ]),
        ModernTableRow::success(vec![
            "cascade".to_owned(),
            "--palette cascade".to_owned(),
            "#2DD4BF primary + #7DD3FC secondary".to_owned(),
            label(
                ctx,
                "cli.info.palette.best_cascade",
                "brighter modern tables while preserving the approved app icon teal",
            ),
        ]),
        ModernTableRow::success(vec![
            "daylight".to_owned(),
            "--palette daylight".to_owned(),
            "#2DD4BF primary + #67E8F9 secondary".to_owned(),
            label(
                ctx,
                "cli.info.palette.best_daylight",
                "bright teal/sky output for modern tables and live progress without dark chrome",
            ),
        ]),
        ModernTableRow::success(vec![
            "foam".to_owned(),
            "--palette foam".to_owned(),
            "#2DD4BF primary + #E0F2FE secondary".to_owned(),
            label(
                ctx,
                "cli.info.palette.best_foam",
                "soft app-icon colors with a bright ice-blue highlight and no dark chrome",
            ),
        ]),
        ModernTableRow::success(vec![
            "skyline".to_owned(),
            "--palette skyline".to_owned(),
            "#0EA5E9 primary + #2DD4BF secondary".to_owned(),
            label(
                ctx,
                "cli.info.palette.best_skyline",
                "brighter blue-led terminal output while keeping the approved icon colors",
            ),
        ]),
        ModernTableRow::success(vec![
            "aero".to_owned(),
            "--palette aero".to_owned(),
            "#7DD3FC primary + #2DD4BF secondary".to_owned(),
            label(
                ctx,
                "cli.info.palette.best_aero",
                "lighter sky-led modern output while staying in the approved icon palette",
            ),
        ]),
        ModernTableRow::success(vec![
            "crest".to_owned(),
            "--palette crest".to_owned(),
            "#38BDF8 primary + #5EEAD4 secondary".to_owned(),
            label(
                ctx,
                "cli.info.palette.best_crest",
                "brighter teal/sky command demos without dark chrome",
            ),
        ]),
        ModernTableRow::success(vec![
            "halo".to_owned(),
            "--palette halo".to_owned(),
            "#5EEAD4 primary + #38BDF8 secondary".to_owned(),
            label(
                ctx,
                "cli.info.palette.best_halo",
                "bright teal/sky output for richer modern progress and tables",
            ),
        ]),
        ModernTableRow::success(vec![
            "tropic".to_owned(),
            "--palette tropic".to_owned(),
            "#2DD4BF primary + #22D3EE secondary".to_owned(),
            label(
                ctx,
                "cli.info.palette.best_tropic",
                "approved teal with a brighter cyan accent for fancy transfer boards",
            ),
        ]),
        ModernTableRow::success(vec![
            "kinetic".to_owned(),
            "--palette kinetic".to_owned(),
            "#2DD4BF primary + #60A5FA secondary".to_owned(),
            label(
                ctx,
                "cli.info.palette.best_kinetic",
                "approved teal with a more energetic sky accent for live transfer matrices",
            ),
        ]),
        ModernTableRow::success(vec![
            "radiant".to_owned(),
            "--palette radiant".to_owned(),
            "#2DD4BF primary + #BAE6FD secondary".to_owned(),
            label(
                ctx,
                "cli.info.palette.best_radiant",
                "approved teal with a brighter sky-glass highlight for modern progress dashboards",
            ),
        ]),
        ModernTableRow::success(vec![
            "surge".to_owned(),
            "--palette surge".to_owned(),
            "#2DD4BF primary + #38BDF8 secondary".to_owned(),
            label(
                ctx,
                "cli.info.palette.best_surge",
                "approved teal with vivid sky-blue accents for live HUDs and dense tables",
            ),
        ]),
        ModernTableRow::success(vec![
            "squallz".to_owned(),
            "--palette squallz".to_owned(),
            "#2DD4BF primary + #0EA5E9 secondary".to_owned(),
            label(
                ctx,
                "cli.info.palette.best_default",
                "default brand look and app icon continuity",
            ),
        ]),
        ModernTableRow::success(vec![
            "glass".to_owned(),
            "--colors glass".to_owned(),
            "bright cyan primary + Squallz teal secondary".to_owned(),
            label(
                ctx,
                "cli.info.palette.best_fancy",
                "more luminous modern terminals without dark chrome",
            ),
        ]),
        ModernTableRow::success(vec![
            "nova".to_owned(),
            "--palette nova".to_owned(),
            "bright cyan primary + sunlit gold secondary".to_owned(),
            label(
                ctx,
                "cli.info.palette.best_vivid",
                "more vivid modern output without dark chrome",
            ),
        ]),
        ModernTableRow::success(vec![
            "crystal".to_owned(),
            "--palette crystal".to_owned(),
            "luminous aqua primary + clear sky secondary".to_owned(),
            label(
                ctx,
                "cli.info.palette.best_crystal",
                "fancy bright tables while staying in the icon color family",
            ),
        ]),
        ModernTableRow::success(vec![
            "lumina".to_owned(),
            "--palette lumina".to_owned(),
            "bright cyan primary + coral secondary".to_owned(),
            label(
                ctx,
                "cli.info.palette.best_lumina",
                "richer demos and screenshots with vivid modern contrast",
            ),
        ]),
        ModernTableRow::new(vec![
            "signal".to_owned(),
            "--theme signal".to_owned(),
            "high-signal teal + sky highlight".to_owned(),
            label(
                ctx,
                "cli.info.palette.best_dense",
                "dense tables and long diagnostics",
            ),
        ]),
        ModernTableRow::new(vec![
            "vapor".to_owned(),
            "--color-scheme vapor".to_owned(),
            "sky primary + soft violet secondary".to_owned(),
            label(
                ctx,
                "cli.info.palette.best_demo",
                "presentations and screenshots",
            ),
        ]),
        ModernTableRow::new(vec![
            "mono".to_owned(),
            "--palette mono".to_owned(),
            "monochrome high-contrast ANSI".to_owned(),
            label(
                ctx,
                "cli.info.palette.best_plain",
                "terminals that cannot render truecolor well",
            ),
        ]),
    ]
}

fn modern_inventory_rows(ctx: &Ctx, formats: &[FormatInfo]) -> Vec<ModernTableRow> {
    let built_in_archives = formats
        .iter()
        .filter(|format| format.kind == FormatKind::Archive && !is_external(format.id))
        .count();
    let external_archives = formats
        .iter()
        .filter(|format| format.kind == FormatKind::Archive && is_external(format.id))
        .count();
    let compressors = formats
        .iter()
        .filter(|format| format.kind == FormatKind::Compressor)
        .count();
    vec![
        ModernTableRow::new(vec![
            label(ctx, "cli.info.inventory.formats", "Formats"),
            formats.len().to_string(),
            modern_summary(ctx, formats),
        ]),
        ModernTableRow::success(vec![
            label(ctx, "cli.info.inventory.built_in", "Built-in"),
            built_in_archives.to_string(),
            label(
                ctx,
                "cli.info.note.built_in_archives",
                "Ready now; no external tools required.",
            ),
        ]),
        ModernTableRow::new(vec![
            label(ctx, "cli.info.inventory.external", "External bridges"),
            external_archives.to_string(),
            label(
                ctx,
                "cli.info.note.external_archives",
                "Runtime shows this machine's external tools; --json exposes exact paths.",
            ),
        ]),
        ModernTableRow::success(vec![
            label(ctx, "cli.info.inventory.compressors", "Compressors"),
            compressors.to_string(),
            label(
                ctx,
                "cli.info.note.compressors",
                "Stream codecs are built in and available cross-platform.",
            ),
        ]),
    ]
}

fn modern_support_map_rows(ctx: &Ctx, formats: &[FormatInfo]) -> Vec<ModernTableRow> {
    let archive_pack_unpack = formats
        .iter()
        .filter(|format| {
            format.kind == FormatKind::Archive
                && format.capabilities.can_create
                && format.capabilities.can_extract
        })
        .collect::<Vec<_>>();
    let stream_codecs = formats
        .iter()
        .filter(|format| format.kind == FormatKind::Compressor)
        .collect::<Vec<_>>();
    let unpack_only = formats
        .iter()
        .filter(|format| {
            format.kind == FormatKind::Archive
                && !format.capabilities.can_create
                && format.capabilities.can_extract
        })
        .collect::<Vec<_>>();
    let edit_update = formats
        .iter()
        .filter(|format| format.capabilities.can_update)
        .collect::<Vec<_>>();

    vec![
        support_map_row(
            label(ctx, "cli.info.support.archive_pack", "Archive pack/unpack"),
            label(
                ctx,
                "cli.info.support.mode.create_extract",
                "create + extract",
            ),
            &archive_pack_unpack,
            label(
                ctx,
                "cli.info.support.risk.native_external",
                "native + WIM external",
            ),
        ),
        support_map_row(
            label(ctx, "cli.info.support.stream_codecs", "Stream codecs"),
            label(
                ctx,
                "cli.info.support.mode.codec_streams",
                "single-file codecs",
            ),
            &stream_codecs,
            label(
                ctx,
                "cli.info.support.risk.built_in",
                "built-in cross-platform",
            ),
        ),
        support_map_row(
            label(ctx, "cli.info.support.unpack_only", "Unpack only"),
            label(ctx, "cli.info.support.mode.extract_test", "extract + test"),
            &unpack_only,
            label(
                ctx,
                "cli.info.support.risk.external_bridge",
                "external bridge tools",
            ),
        ),
        support_map_row(
            label(ctx, "cli.info.support.edit_update", "Edit/update"),
            label(ctx, "cli.info.support.mode.zip_entries", "zip entry edits"),
            &edit_update,
            label(ctx, "cli.info.support.risk.atomic_writer", "atomic writer"),
        ),
        ModernTableRow::success(vec![
            label(ctx, "cli.info.support.recovery", "Recovery/repair"),
            label(ctx, "cli.info.support.mode.recovery", "sqz/par2/zip repair"),
            label(ctx, "cli.info.support.ready.release", "3 paths"),
            label(ctx, "cli.info.support.risk.recovery", "built-in + PAR2 opt"),
            "sqz, zip rebuild, PAR2".to_owned(),
        ]),
    ]
}

fn support_map_row(
    lane: String,
    mode: String,
    formats: &[&FormatInfo],
    risk: String,
) -> ModernTableRow {
    let ready = formats
        .iter()
        .filter(|format| !format_has_missing_required_runtime(format))
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

fn modern_format_coverage_rows(ctx: &Ctx, formats: &[FormatInfo]) -> Vec<ModernTableRow> {
    let pack_unpack = formats
        .iter()
        .filter(|format| format.capabilities.can_create && format.capabilities.can_extract)
        .collect::<Vec<_>>();
    let unpack_only = formats
        .iter()
        .filter(|format| {
            format.kind == FormatKind::Archive
                && !format.capabilities.can_create
                && format.capabilities.can_extract
        })
        .collect::<Vec<_>>();
    let stream_codecs = formats
        .iter()
        .filter(|format| format.kind == FormatKind::Compressor)
        .collect::<Vec<_>>();
    vec![
        ModernTableRow::success(vec![
            label(ctx, "cli.info.coverage.pack_unpack", "Pack / unpack"),
            pack_unpack.len().to_string(),
            label(
                ctx,
                "cli.info.coverage.runtime.native_bridge",
                "built-in + bridge",
            ),
            format_ids(&pack_unpack),
        ]),
        ModernTableRow::warning(vec![
            label(ctx, "cli.info.coverage.unpack_only", "Unpack only"),
            unpack_only.len().to_string(),
            label(ctx, "cli.info.coverage.runtime.bridge", "external bridge"),
            format_ids(&unpack_only),
        ]),
        ModernTableRow::success(vec![
            label(ctx, "cli.info.coverage.streams", "Stream codecs"),
            stream_codecs.len().to_string(),
            label(ctx, "cli.info.coverage.runtime.built_in", "built-in"),
            format_ids(&stream_codecs),
        ]),
        ModernTableRow::success(vec![
            label(ctx, "cli.info.support.recovery", "Recovery/repair"),
            "3".to_owned(),
            label(
                ctx,
                "cli.info.coverage.runtime.mixed",
                "built-in + optional",
            ),
            "sqz, zip rebuild, PAR2".to_owned(),
        ]),
    ]
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

fn modern_capability_lane_rows(ctx: &Ctx, formats: &[FormatInfo]) -> Vec<ModernTableRow> {
    [
        CapabilityLane::Create,
        CapabilityLane::Extract,
        CapabilityLane::Test,
        CapabilityLane::Update,
        CapabilityLane::Split,
        CapabilityLane::Encrypt,
        CapabilityLane::EncryptNames,
    ]
    .into_iter()
    .map(|lane| {
        let supported: Vec<&FormatInfo> = formats
            .iter()
            .filter(|format| lane.supported(format))
            .collect();
        let ready = supported
            .iter()
            .filter(|format| format_runtime_ready_for_lane(format, lane))
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
            label(ctx, "cli.info.workflow.create", "Create archives"),
            "sqz compress <input> -o out.zip".to_owned(),
            label(
                ctx,
                "cli.info.cheatsheet.create",
                "Create ZIP, 7z, TAR, WIM, or SQZ output.",
            ),
        ]),
        ModernTableRow::success(vec![
            label(ctx, "cli.info.workflow.extract", "Unpack archives"),
            "sqz extract archive -d out --smart".to_owned(),
            label(
                ctx,
                "cli.info.cheatsheet.extract",
                "Unpack with overwrite and safety controls.",
            ),
        ]),
        ModernTableRow::new(vec![
            label(ctx, "cli.info.cheatsheet.inspect", "Inspect"),
            "sqz list archive --tree".to_owned(),
            label(
                ctx,
                "cli.info.cheatsheet.inspect_use",
                "Browse contents before extracting.",
            ),
        ]),
        ModernTableRow::new(vec![
            label(ctx, "cli.info.workflow.test", "Integrity test"),
            "sqz test archive --json".to_owned(),
            label(
                ctx,
                "cli.info.cheatsheet.test",
                "Machine-readable verification for scripts.",
            ),
        ]),
        ModernTableRow::new(vec![
            label(ctx, "cli.info.workflow.update", "Update entries"),
            "sqz update archive --add file".to_owned(),
            label(
                ctx,
                "cli.info.cheatsheet.update",
                "Add, delete, rename, or move entries atomically.",
            ),
        ]),
        ModernTableRow::new(vec![
            label(ctx, "cli.info.cheatsheet.recover", "Recovery"),
            "sqz pack <input> -o vault.sqz".to_owned(),
            label(
                ctx,
                "cli.info.cheatsheet.recover_use",
                "Use SQZ or PAR2 recovery flows for repairable archives.",
            ),
        ]),
    ]
}

fn format_runtime_ready_for_lane(format: &FormatInfo, lane: CapabilityLane) -> bool {
    if !lane.supported(format) {
        return false;
    }
    if !is_external(format.id) {
        return true;
    }
    let implementation = implementation_json(format.id);
    let operation = match lane.runtime_need() {
        RuntimeNeed::Read => "read",
        RuntimeNeed::Write => "write",
    };
    json_nested_bool_field(&implementation["availability"], operation, "available")
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
    let caps = format.capabilities;
    let mut values = Vec::new();
    if caps.can_create {
        values.push(label(ctx, "cli.info.cap.create", "create"));
    }
    if caps.can_extract {
        values.push(label(ctx, "cli.info.cap.extract", "extract"));
    }
    if caps.can_test {
        values.push(label(ctx, "cli.info.cap.test", "test"));
    }
    if caps.can_update {
        values.push(label(ctx, "cli.info.cap.update", "update"));
    }
    if caps.can_split {
        values.push(label(ctx, "cli.info.cap.split", "split"));
    }
    if caps.can_encrypt_data {
        values.push(label(ctx, "cli.info.cap.encrypt", "encrypt"));
    }
    if caps.can_encrypt_names {
        values.push(label(ctx, "cli.info.cap.encrypt_names", "hide-names"));
    }
    if values.is_empty() {
        label(ctx, "cli.info.cap.none", "none")
    } else {
        values.join(" ")
    }
}

fn runtime_operation_detail(ctx: &Ctx, format: &FormatInfo, operation: &str) -> String {
    if !is_external(format.id) {
        let supported = match operation {
            "read" => format.capabilities.can_extract || format.capabilities.can_test,
            "write" => format.capabilities.can_create,
            _ => false,
        };
        return if supported {
            label(ctx, "cli.info.runtime.ready", "ready")
        } else {
            label(ctx, "cli.info.runtime.unsupported", "unsupported")
        };
    }

    let implementation = implementation_json(format.id);
    availability_status_label(ctx, &implementation["availability"][operation])
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

#[derive(Clone, Copy)]
enum RuntimeNeed {
    Read,
    Write,
}

impl CapabilityLane {
    fn label(self, ctx: &Ctx) -> String {
        match self {
            Self::Create => label(ctx, "cli.info.workflow.create", "Create archives"),
            Self::Extract => label(ctx, "cli.info.workflow.extract", "Unpack archives"),
            Self::Test => label(ctx, "cli.info.workflow.test", "Integrity test"),
            Self::Update => label(ctx, "cli.info.workflow.update", "Update entries"),
            Self::Split => label(ctx, "cli.info.workflow.split", "Split volumes"),
            Self::Encrypt => label(ctx, "cli.info.workflow.encrypt", "Encrypt data"),
            Self::EncryptNames => label(ctx, "cli.info.workflow.encrypt_names", "Hide names"),
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

fn format_runtime_tone(format: &FormatInfo) -> Tone {
    if format_has_missing_required_runtime(format) {
        Tone::Warning
    } else if !is_external(format.id) {
        Tone::Success
    } else {
        Tone::Secondary
    }
}

fn format_has_missing_required_runtime(format: &FormatInfo) -> bool {
    if !is_external(format.id) {
        return false;
    }
    let implementation = implementation_json(format.id);
    let availability = &implementation["availability"];
    let read_required = format.capabilities.can_extract || format.capabilities.can_test;
    let write_required = format.capabilities.can_create;
    (read_required && !json_nested_bool_field(availability, "read", "available"))
        || (write_required && !json_nested_bool_field(availability, "write", "available"))
}

fn availability_status_label(ctx: &Ctx, availability: &Value) -> String {
    if availability["source"].as_str() == Some("unsupported") {
        return label(ctx, "cli.info.runtime.unsupported", "unsupported");
    }

    let hint = availability_tool_hint(availability);
    if json_bool_field(availability, "available") {
        let status = label(ctx, "cli.info.runtime.ready", "ready");
        if hint.is_empty() {
            status
        } else {
            format!("{status}({hint})")
        }
    } else {
        let status = label(ctx, "cli.info.runtime.missing", "missing");
        if hint.is_empty() {
            status
        } else {
            format!("{status}({hint})")
        }
    }
}

fn availability_tool_hint(availability: &Value) -> String {
    if let Some(selected) = availability["selected"].as_str().filter(|s| !s.is_empty()) {
        return normalize_tool_hint(path_file_name_or_self(selected));
    }
    match first_tool_name(availability) {
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

fn path_file_name_or_self(path: &str) -> &str {
    match Path::new(path).file_name().and_then(|name| name.to_str()) {
        Some(name) => name,
        None => path,
    }
}

fn first_tool_name(availability: &Value) -> Option<&str> {
    availability
        .get("tools")
        .and_then(Value::as_array)
        .and_then(|tools| tools.first())
        .and_then(Value::as_str)
}

pub(crate) fn implementation_json(format_id: &str) -> Value {
    match format_id {
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
                "read": sevenzip_availability(),
                "write": env_or_path_availability(Some("SQUALLZ_WIMLIB"), &["wimlib-imagex"]),
            },
            "platforms": ["macos", "windows", "linux"],
            "release_gate": "real WIM compatibility matrix plus wimlib/7z packaging and license review",
        }),
        "rar" => json!({
            "status": "external_required",
            "bundled": false,
            "read": {
                "kind": "external_tool",
                "tools": ["7zz", "7z", "7za"],
                "env": "SQUALLZ_7Z",
                "fallback_tools": ["bsdtar"],
                "fallback_env": "SQUALLZ_BSDTAR",
            },
            "write": {
                "kind": "unsupported",
                "reason": "RAR creation and RAR recovery records are outside the launch scope",
            },
            "policy": rar_policy_json(),
            "availability": {
                "read": rar_read_availability(),
                "write": unsupported_availability(),
            },
            "limitations": [
                {
                    "scope": "create",
                    "status": "unsupported",
                    "reason": "Squallz does not create RAR archives",
                },
                {
                    "scope": "recovery_records",
                    "status": "unsupported",
                    "reason": "RAR recovery records and RAR .rev files are outside the launch scope",
                },
                {
                    "scope": "encrypted",
                    "status": "not_release_claimed",
                    "reason": "encrypted RAR compatibility requires a licensed/full compatibility matrix",
                },
                {
                    "scope": "multi_volume",
                    "status": "not_release_claimed",
                    "reason": "the current stream bridge opens one source stream; adjacent-volume handling needs a path-aware engine readiness check",
                },
                {
                    "scope": "damaged_repair",
                    "status": "unsupported",
                    "reason": "damaged RAR can be detected or rejected, but RAR repair is not implemented",
                },
            ],
            "platforms": ["macos", "windows", "linux"],
            "release_gate": "licensed RAR compatibility matrix plus external tool packaging and license review",
        }),
        id if long_tail_7z_bridge_format(id) => json!({
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
                "read": sevenzip_availability(),
                "write": unsupported_availability(),
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
                "read": built_in_availability(),
                "write": built_in_availability(),
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
        "fallback_tools": ["bsdtar"],
        "fallback_env": "SQUALLZ_BSDTAR",
        "fallback_scope": "diagnostic_or_rar5_v6",
        "fallback_reason": "explicit SQUALLZ_BSDTAR override or RAR5 v6 method detection may select bsdtar; bsdtar is not a bundled product guarantee",
        "license_boundary": "Squallz does not link unrar code or create RAR archives; bundling 7zz/7z would require LGPL plus unRAR restriction notices, source/replacement path, and RAR creation must remain unsupported",
        "release_claim": "read-only public-sample subset; encrypted, full multi-volume, and damaged RAR repair are not release-claimed",
    })
}

fn built_in_availability() -> Value {
    json!({
        "available": true,
        "source": "built_in",
    })
}

fn unsupported_availability() -> Value {
    json!({
        "available": false,
        "source": "unsupported",
    })
}

fn sevenzip_availability() -> Value {
    env_or_path_availability(Some("SQUALLZ_7Z"), &["7zz", "7z", "7za"])
}

fn rar_read_availability() -> Value {
    if std::env::var_os("SQUALLZ_BSDTAR").is_some() {
        return bsdtar_availability();
    }
    let sevenzip = sevenzip_availability();
    if json_bool_field(&sevenzip, "configured") || json_bool_field(&sevenzip, "available") {
        return sevenzip;
    }
    bsdtar_availability()
}

fn bsdtar_availability() -> Value {
    if let Some(configured) = std::env::var_os("SQUALLZ_BSDTAR") {
        let configured = PathBuf::from(configured);
        let exists = command_path_is_executable(&configured);
        return json!({
            "available": exists,
            "source": "env",
            "env": "SQUALLZ_BSDTAR",
            "selected": configured.to_string_lossy(),
            "configured": true,
            "path_exists": exists,
            "tools": ["bsdtar"],
        });
    }
    let absolute = Path::new("/usr/bin/bsdtar");
    if command_is_executable(absolute) {
        return json!({
            "available": true,
            "source": "path",
            "selected": absolute.to_string_lossy(),
            "configured": false,
            "path_exists": true,
            "tools": ["bsdtar"],
        });
    }
    if let Some(path) = find_on_path("bsdtar") {
        return json!({
            "available": true,
            "source": "path",
            "selected": path.to_string_lossy(),
            "configured": false,
            "path_exists": true,
            "tools": ["bsdtar"],
        });
    }
    json!({
        "available": false,
        "source": null,
        "selected": null,
        "configured": false,
        "path_exists": false,
        "tools": ["bsdtar"],
    })
}

fn env_or_path_availability(env: Option<&str>, tools: &[&str]) -> Value {
    if let Some(env_name) = env {
        if let Some(configured) = std::env::var_os(env_name) {
            let configured = PathBuf::from(configured);
            let exists = command_path_is_executable(&configured);
            return json!({
                "available": exists,
                "source": "env",
                "env": env_name,
                "selected": configured.to_string_lossy(),
                "configured": true,
                "path_exists": exists,
                "tools": tools,
            });
        }
    }
    for tool in tools {
        if let Some(path) = find_on_path(tool) {
            return json!({
                "available": true,
                "source": "path",
                "env": env,
                "selected": path.to_string_lossy(),
                "configured": false,
                "path_exists": true,
                "tools": tools,
            });
        }
    }
    json!({
        "available": false,
        "source": null,
        "env": env,
        "selected": null,
        "configured": false,
        "path_exists": false,
        "tools": tools,
    })
}

fn command_path_is_executable(path: &Path) -> bool {
    if path.components().count() > 1 || path.is_absolute() {
        return command_is_executable(path);
    }
    find_on_path(&path.to_string_lossy()).is_some()
}

fn find_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
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

fn label(ctx: &Ctx, key: &str, fallback: &str) -> String {
    let translated = ctx.loc.t(key);
    if translated == key {
        fallback.to_owned()
    } else {
        translated
    }
}

fn is_external(format_id: &str) -> bool {
    format_id == "wim" || format_id == "rar" || long_tail_7z_bridge_format(format_id)
}

fn long_tail_7z_bridge_format(format_id: &str) -> bool {
    matches!(
        format_id,
        "apfs"
            | "ar"
            | "arj"
            | "cab"
            | "chm"
            | "cpio"
            | "cramfs"
            | "dmg"
            | "ext"
            | "fat"
            | "gpt"
            | "hfs"
            | "ihex"
            | "iso"
            | "lzh"
            | "lzma"
            | "mbr"
            | "msi"
            | "nsis"
            | "ntfs"
            | "qcow2"
            | "rpm"
            | "squashfs"
            | "udf"
            | "uefi"
            | "vdi"
            | "vhd"
            | "vhdx"
            | "vmdk"
            | "xar"
            | "z"
    )
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
        "sqz" => json!({
            "store": "transparent container",
            "fastest": "transparent container",
            "fast": "transparent container",
            "normal": "transparent container",
            "maximum": "transparent container",
            "ultra": "transparent container",
            "note": "SQZ v1 may ignore compression level for transparent container payloads",
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
