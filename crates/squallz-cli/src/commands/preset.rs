//! `sqz preset`: manage the same versioned preset document used by the
//! desktop app and file-manager integrations.

use std::fs::File;
use std::io::Read;
use std::path::Path;

use serde::Serialize;
use serde_json::{json, Value};
use squallz_core::api::FormatError;
use squallz_core::{
    NamedPreset, PresetBindings, PresetDocument, PresetError, PresetId, PresetKind, PresetLabel,
    PresetStore, BALANCED_CREATE_PRESET_ID, SMART_EXTRACT_PRESET_ID,
};

use super::reports::print_pretty_json;
use super::{Ctx, ModernTableColumn, ModernTableRow};
use crate::args::{PresetBindingArg, PresetCmd};
use crate::errors::CliError;
use crate::file_manager_presets::preset_store;

const MAX_PRESET_INPUT_BYTES: u64 = 1024 * 1024;

pub fn run(ctx: &Ctx, cmd: PresetCmd) -> Result<(), CliError> {
    match cmd {
        PresetCmd::List { kind, json } => list(ctx, kind.map(Into::into), json),
        PresetCmd::Show { id, json } => show(ctx, &id, json),
        PresetCmd::Clone {
            source_id,
            new_id,
            label,
            json,
        } => clone_preset(ctx, &source_id, new_id, label, json),
        PresetCmd::Update { id, file, json } => update(ctx, &id, &file, json),
        PresetCmd::Delete { id, json } => delete(ctx, &id, json),
        PresetCmd::Bind { slot, id, json } => bind(ctx, slot, &id, json),
        PresetCmd::Unbind { slot, json } => unbind(ctx, slot, json),
        PresetCmd::Path { json } => print_path(ctx, json),
    }
}

fn list(ctx: &Ctx, kind: Option<PresetKind>, json_output: bool) -> Result<(), CliError> {
    let (_, document) = load_document(ctx)?;
    let presets = document
        .presets
        .iter()
        .filter(|preset| kind.is_none_or(|kind| preset.kind() == kind))
        .collect::<Vec<_>>();

    if json_output {
        print_json(&json!({
            "schema_version": document.schema_version,
            "revision": document.revision,
            "presets": presets,
            "bindings": document.bindings,
        }))?;
        return Ok(());
    }

    if presets.is_empty() {
        println!("{}", ctx.loc.t("cli.presets.empty"));
        return Ok(());
    }

    let rows = presets
        .iter()
        .map(|preset| {
            ModernTableRow::new(vec![
                preset.id().as_str().to_owned(),
                preset_kind_label(ctx, preset.kind()),
                preset.label().as_str().to_owned(),
                preset_scope_label(ctx, preset),
                binding_summary(ctx, &document.bindings, preset.id()),
            ])
        })
        .collect::<Vec<_>>();
    if ctx.is_modern() {
        ctx.print_modern_table(
            &ctx.loc.t("cli.presets.heading"),
            &[
                ModernTableColumn::new(ctx.loc.t("common.id"), 28),
                ModernTableColumn::new(ctx.loc.t("common.kind"), 8),
                ModernTableColumn::new(ctx.loc.t("common.name"), 22),
                ModernTableColumn::new(ctx.loc.t("cli.presets.scope"), 10),
                ModernTableColumn::new(ctx.loc.t("cli.presets.bindings"), 20),
            ],
            &rows,
        );
    } else {
        println!(
            "{}\t{}\t{}\t{}\t{}",
            ctx.loc.t("common.id"),
            ctx.loc.t("common.kind"),
            ctx.loc.t("common.name"),
            ctx.loc.t("cli.presets.scope"),
            ctx.loc.t("cli.presets.bindings"),
        );
        for preset in presets {
            println!(
                "{}\t{}\t{}\t{}\t{}",
                preset.id().as_str(),
                preset_kind_label(ctx, preset.kind()),
                preset.label().as_str(),
                preset_scope_label(ctx, preset),
                binding_summary(ctx, &document.bindings, preset.id()),
            );
        }
    }
    Ok(())
}

fn show(ctx: &Ctx, id: &str, json_output: bool) -> Result<(), CliError> {
    let (_, document) = load_document(ctx)?;
    let id = parse_id(ctx, id)?;
    let preset = find_preset(ctx, &document, &id)?;
    if json_output {
        return print_serializable(preset);
    }

    println!("{}: {}", ctx.loc.t("common.id"), preset.id().as_str());
    println!("{}: {}", ctx.loc.t("common.name"), preset.label().as_str());
    println!(
        "{}: {}",
        ctx.loc.t("common.kind"),
        preset_kind_label(ctx, preset.kind())
    );
    println!(
        "{}: {}",
        ctx.loc.t("cli.presets.scope"),
        preset_scope_label(ctx, preset)
    );
    println!(
        "{}: {}",
        ctx.loc.t("cli.presets.bindings"),
        binding_summary(ctx, &document.bindings, preset.id())
    );
    println!();
    println!(
        "{}",
        serde_json::to_string_pretty(preset).map_err(json_error)?
    );
    Ok(())
}

fn clone_preset(
    ctx: &Ctx,
    source_id: &str,
    new_id: String,
    label: String,
    json_output: bool,
) -> Result<(), CliError> {
    let (store, mut document) = load_document(ctx)?;
    let source_id = parse_id(ctx, source_id)?;
    let new_id = parse_id(ctx, &new_id)?;
    let label = parse_label(ctx, label)?;
    if document.preset(&new_id).is_some() {
        return Err(message_error(
            ctx,
            "cli.presets.error_id_in_use",
            &[("id", new_id.as_str())],
        ));
    }
    let source = find_preset(ctx, &document, &source_id)?.clone();
    let cloned = match source {
        NamedPreset::Create { options, .. } => NamedPreset::Create {
            id: new_id,
            label,
            built_in: false,
            options,
        },
        NamedPreset::Extract { options, .. } => NamedPreset::Extract {
            id: new_id,
            label,
            built_in: false,
            options,
        },
    };
    let cloned_id = cloned.id().clone();
    document.presets.push(cloned);
    let document = save_document(ctx, &store, document)?;
    let preset = find_preset(ctx, &document, &cloned_id)?;
    print_mutation(
        ctx,
        json_output,
        "clone",
        &document,
        Some(preset),
        "cli.presets.cloned",
    )
}

fn update(ctx: &Ctx, id: &str, file: &Path, json_output: bool) -> Result<(), CliError> {
    let (store, mut document) = load_document(ctx)?;
    let id = parse_id(ctx, id)?;
    let index = preset_index(ctx, &document, &id)?;
    if document.presets[index].built_in() {
        return Err(built_in_error(ctx, id.as_str()));
    }

    let replacement = read_preset_file(ctx, file)?;
    if replacement.id() != &id {
        return Err(message_error(
            ctx,
            "cli.presets.error_id_mismatch",
            &[
                ("expected", id.as_str()),
                ("actual", replacement.id().as_str()),
            ],
        ));
    }
    if replacement.built_in() {
        return Err(message_error(
            ctx,
            "cli.presets.error_update_built_in_flag",
            &[],
        ));
    }
    if replacement.kind() != document.presets[index].kind() {
        return Err(message_error(
            ctx,
            "cli.presets.error_kind_changed",
            &[("id", id.as_str())],
        ));
    }
    document.presets[index] = replacement;
    let document = save_document(ctx, &store, document)?;
    let preset = find_preset(ctx, &document, &id)?;
    print_mutation(
        ctx,
        json_output,
        "update",
        &document,
        Some(preset),
        "cli.presets.updated",
    )
}

fn delete(ctx: &Ctx, id: &str, json_output: bool) -> Result<(), CliError> {
    let (store, mut document) = load_document(ctx)?;
    let id = parse_id(ctx, id)?;
    let index = preset_index(ctx, &document, &id)?;
    if document.presets[index].built_in() {
        return Err(built_in_error(ctx, id.as_str()));
    }
    let kind = document.presets[index].kind();
    let label = document.presets[index].label().as_str().to_owned();
    let fallback = fallback_id(ctx, &document, kind)?;
    document.presets.remove(index);
    replace_deleted_bindings(&mut document.bindings, &id, &fallback, kind);
    let document = save_document(ctx, &store, document)?;

    if json_output {
        return print_json(&json!({
            "ok": true,
            "operation": "preset.delete",
            "revision": document.revision,
            "deleted": id,
            "bindings": document.bindings,
        }));
    }
    ctx.print_success(ctx.loc.format("cli.presets.deleted", &[("name", &label)]));
    Ok(())
}

fn bind(ctx: &Ctx, slot: PresetBindingArg, id: &str, json_output: bool) -> Result<(), CliError> {
    let (store, mut document) = load_document(ctx)?;
    let id = parse_id(ctx, id)?;
    let preset = find_preset(ctx, &document, &id)?;
    if preset.kind() != binding_kind(slot) {
        return Err(message_error(
            ctx,
            "cli.presets.error_binding_kind",
            &[
                ("slot", &binding_slot_label(ctx, slot)),
                ("kind", &preset_kind_label(ctx, preset.kind())),
            ],
        ));
    }
    set_binding(&mut document.bindings, slot, Some(id));
    let document = save_document(ctx, &store, document)?;
    print_binding_result(ctx, json_output, "bind", slot, &document)
}

fn unbind(ctx: &Ctx, slot: PresetBindingArg, json_output: bool) -> Result<(), CliError> {
    let (store, mut document) = load_document(ctx)?;
    set_binding(&mut document.bindings, slot, None);
    let document = save_document(ctx, &store, document)?;
    print_binding_result(ctx, json_output, "unbind", slot, &document)
}

fn print_path(_ctx: &Ctx, json_output: bool) -> Result<(), CliError> {
    let store = preset_store()?;
    let path = store.path().display().to_string();
    if json_output {
        print_json(&json!({ "path": path }))
    } else {
        println!("{path}");
        Ok(())
    }
}

fn load_document(ctx: &Ctx) -> Result<(PresetStore, PresetDocument), CliError> {
    let store = preset_store()?;
    let document = store
        .load()
        .map_err(|error| preset_error(ctx, "cli.presets.error_load", error))?;
    Ok((store, document))
}

fn save_document(
    ctx: &Ctx,
    store: &PresetStore,
    document: PresetDocument,
) -> Result<PresetDocument, CliError> {
    store
        .compare_and_swap(document.revision, document)
        .map_err(|error| preset_error(ctx, "cli.presets.error_save", error))
}

fn read_preset_file(ctx: &Ctx, path: &Path) -> Result<NamedPreset, CliError> {
    let mut file = File::open(path).map_err(FormatError::from)?;
    let mut bytes = Vec::new();
    file.by_ref()
        .take(MAX_PRESET_INPUT_BYTES.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(FormatError::from)?;
    if bytes.len() as u64 > MAX_PRESET_INPUT_BYTES {
        return Err(message_error(
            ctx,
            "cli.presets.error_file_too_large",
            &[("limit", "1 MiB")],
        ));
    }
    serde_json::from_slice(&bytes).map_err(|error| {
        message_error(
            ctx,
            "cli.presets.error_file_invalid",
            &[("detail", &error.to_string())],
        )
    })
}

fn parse_id(ctx: &Ctx, value: &str) -> Result<PresetId, CliError> {
    PresetId::new(value).map_err(|error| {
        message_error(
            ctx,
            "cli.presets.error_invalid_id",
            &[("detail", &error.to_string())],
        )
    })
}

fn parse_label(ctx: &Ctx, value: String) -> Result<PresetLabel, CliError> {
    PresetLabel::new(value).map_err(|error| {
        message_error(
            ctx,
            "cli.presets.error_invalid_label",
            &[("detail", &error.to_string())],
        )
    })
}

fn find_preset<'a>(
    ctx: &Ctx,
    document: &'a PresetDocument,
    id: &PresetId,
) -> Result<&'a NamedPreset, CliError> {
    document
        .preset(id)
        .ok_or_else(|| message_error(ctx, "cli.presets.error_not_found", &[("id", id.as_str())]))
}

fn preset_index(ctx: &Ctx, document: &PresetDocument, id: &PresetId) -> Result<usize, CliError> {
    document
        .presets
        .iter()
        .position(|preset| preset.id() == id)
        .ok_or_else(|| message_error(ctx, "cli.presets.error_not_found", &[("id", id.as_str())]))
}

fn fallback_id(
    ctx: &Ctx,
    document: &PresetDocument,
    kind: PresetKind,
) -> Result<PresetId, CliError> {
    let id = match kind {
        PresetKind::Create => BALANCED_CREATE_PRESET_ID,
        PresetKind::Extract => SMART_EXTRACT_PRESET_ID,
    };
    let id = parse_id(ctx, id)?;
    find_preset(ctx, document, &id)?;
    Ok(id)
}

fn replace_deleted_bindings(
    bindings: &mut PresetBindings,
    deleted: &PresetId,
    fallback: &PresetId,
    kind: PresetKind,
) {
    let slots = match kind {
        PresetKind::Create => [
            &mut bindings.app_default_create,
            &mut bindings.file_manager_create,
        ],
        PresetKind::Extract => [
            &mut bindings.app_default_extract,
            &mut bindings.file_manager_extract,
        ],
    };
    for slot in slots {
        if slot.as_ref() == Some(deleted) {
            *slot = Some(fallback.clone());
        }
    }
}

fn set_binding(bindings: &mut PresetBindings, slot: PresetBindingArg, id: Option<PresetId>) {
    match slot {
        PresetBindingArg::AppCreate => bindings.app_default_create = id,
        PresetBindingArg::AppExtract => bindings.app_default_extract = id,
        PresetBindingArg::FileManagerCreate => bindings.file_manager_create = id,
        PresetBindingArg::FileManagerExtract => bindings.file_manager_extract = id,
    }
}

fn binding_kind(slot: PresetBindingArg) -> PresetKind {
    match slot {
        PresetBindingArg::AppCreate | PresetBindingArg::FileManagerCreate => PresetKind::Create,
        PresetBindingArg::AppExtract | PresetBindingArg::FileManagerExtract => PresetKind::Extract,
    }
}

fn binding_summary(ctx: &Ctx, bindings: &PresetBindings, id: &PresetId) -> String {
    let mut slots = Vec::new();
    for slot in [
        PresetBindingArg::AppCreate,
        PresetBindingArg::AppExtract,
        PresetBindingArg::FileManagerCreate,
        PresetBindingArg::FileManagerExtract,
    ] {
        let bound = match slot {
            PresetBindingArg::AppCreate => bindings.app_default_create.as_ref(),
            PresetBindingArg::AppExtract => bindings.app_default_extract.as_ref(),
            PresetBindingArg::FileManagerCreate => bindings.file_manager_create.as_ref(),
            PresetBindingArg::FileManagerExtract => bindings.file_manager_extract.as_ref(),
        };
        if bound == Some(id) {
            slots.push(binding_slot_label(ctx, slot));
        }
    }
    if slots.is_empty() {
        "-".to_owned()
    } else {
        slots.join(", ")
    }
}

fn preset_kind_label(ctx: &Ctx, kind: PresetKind) -> String {
    match kind {
        PresetKind::Create => ctx.loc.t("cli.presets.kind.create"),
        PresetKind::Extract => ctx.loc.t("cli.presets.kind.extract"),
    }
}

fn preset_scope_label(ctx: &Ctx, preset: &NamedPreset) -> String {
    if preset.built_in() {
        ctx.loc.t("cli.presets.scope.built_in")
    } else {
        ctx.loc.t("cli.presets.scope.editable")
    }
}

fn binding_slot_label(ctx: &Ctx, slot: PresetBindingArg) -> String {
    let key = match slot {
        PresetBindingArg::AppCreate => "cli.presets.binding.app_create",
        PresetBindingArg::AppExtract => "cli.presets.binding.app_extract",
        PresetBindingArg::FileManagerCreate => "cli.presets.binding.file_manager_create",
        PresetBindingArg::FileManagerExtract => "cli.presets.binding.file_manager_extract",
    };
    ctx.loc.t(key)
}

fn print_mutation(
    ctx: &Ctx,
    json_output: bool,
    operation: &str,
    document: &PresetDocument,
    preset: Option<&NamedPreset>,
    message_key: &str,
) -> Result<(), CliError> {
    if json_output {
        return print_json(&json!({
            "ok": true,
            "operation": format!("preset.{operation}"),
            "revision": document.revision,
            "preset": preset,
            "bindings": &document.bindings,
        }));
    }
    let name = preset
        .map(|preset| preset.label().as_str())
        .unwrap_or_default();
    ctx.print_success(ctx.loc.format(message_key, &[("name", name)]));
    Ok(())
}

fn print_binding_result(
    ctx: &Ctx,
    json_output: bool,
    operation: &str,
    slot: PresetBindingArg,
    document: &PresetDocument,
) -> Result<(), CliError> {
    if json_output {
        return print_json(&json!({
            "ok": true,
            "operation": format!("preset.{operation}"),
            "revision": document.revision,
            "slot": binding_slot_id(slot),
            "bindings": &document.bindings,
        }));
    }
    let key = if operation == "bind" {
        "cli.presets.bound"
    } else {
        "cli.presets.unbound"
    };
    ctx.print_success(
        ctx.loc
            .format(key, &[("slot", &binding_slot_label(ctx, slot))]),
    );
    Ok(())
}

fn binding_slot_id(slot: PresetBindingArg) -> &'static str {
    match slot {
        PresetBindingArg::AppCreate => "app_create",
        PresetBindingArg::AppExtract => "app_extract",
        PresetBindingArg::FileManagerCreate => "file_manager_create",
        PresetBindingArg::FileManagerExtract => "file_manager_extract",
    }
}

fn print_serializable(value: &impl Serialize) -> Result<(), CliError> {
    print_json(&serde_json::to_value(value).map_err(json_error)?)
}

fn print_json(value: &Value) -> Result<(), CliError> {
    print_pretty_json(value)
}

fn json_error(error: serde_json::Error) -> CliError {
    FormatError::Other(format!("preset JSON serialization failed: {error}")).into()
}

fn preset_error(ctx: &Ctx, key: &str, error: PresetError) -> CliError {
    match error {
        PresetError::Io(error) => FormatError::from(error).into(),
        error => message_error(ctx, key, &[("detail", &error.to_string())]),
    }
}

fn built_in_error(ctx: &Ctx, id: &str) -> CliError {
    message_error(ctx, "cli.presets.error_built_in_read_only", &[("id", id)])
}

fn message_error(ctx: &Ctx, key: &str, values: &[(&str, &str)]) -> CliError {
    FormatError::Other(ctx.loc.format(key, values)).into()
}
