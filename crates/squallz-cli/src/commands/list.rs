//! `sqz list`: list archive entries (human-readable table, tree or `--json`).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::UNIX_EPOCH;

use serde_json::{json, Value};
use squallz_core::api::{EntryMeta, EntryType, OpenOptions, Password};

use super::reports::print_pretty_json;
use crate::commands::{Ctx, ModernStatusField, ModernTableColumn, ModernTableRow};
use crate::errors::CliError;
use crate::progress::fmt_bytes;
use crate::prompt::with_password_retry;
use crate::ui::Tone;

pub fn run(
    ctx: &Ctx,
    archive: PathBuf,
    password: Option<String>,
    encoding: Option<String>,
    json: bool,
    tree: bool,
) -> Result<(), CliError> {
    let explicit = password.map(Password::new);
    let entries = with_password_retry(&ctx.loc, explicit.as_ref(), |pw| {
        ctx.engine.list(
            &archive,
            &OpenOptions {
                password: pw.cloned(),
                encoding_override: encoding.clone(),
            },
        )
    })?;

    if json {
        let value = Value::Array(entries.iter().map(entry_json).collect());
        print_pretty_json(&value)?;
        return Ok(());
    }

    if tree {
        print_tree(&entries, ctx.is_modern());
        let count = entries.len().to_string();
        let message = ctx.loc.format("cli.list.total", &[("count", &count)]);
        ctx.print_success(&message);
        return Ok(());
    }

    if ctx.is_modern() {
        print_modern_table(ctx, &entries);
    } else {
        println!(
            "{:>12}  {:>12}  {}",
            ctx.loc.t("common.size"),
            ctx.loc.t("common.compressed"),
            ctx.loc.t("common.name"),
        );
        for e in &entries {
            let compressed = raw_size_label(e.compressed_size);
            println!("{:>12}  {compressed:>12}  {}", e.size, e.path);
        }
    }
    let count = entries.len().to_string();
    let message = ctx.loc.format("cli.list.total", &[("count", &count)]);
    ctx.print_success(&message);
    Ok(())
}

pub(crate) fn print_modern_table(ctx: &Ctx, entries: &[EntryMeta]) {
    let summary = EntrySummary::from_entries(entries);
    ctx.print_modern_status_panel(
        &ctx.loc.t("cli.list.heading"),
        &ctx.loc.t("common.done"),
        Tone::Success,
        &format!(
            "{} · {} · {}",
            ctx.loc
                .format("cli.list.total", &[("count", &entries.len().to_string())]),
            fmt_bytes(summary.total_size),
            summary.packed_size_label()
        ),
        &[
            ModernStatusField::new(ctx.loc.t("common.entries"), entries.len().to_string()),
            ModernStatusField::new(ctx.loc.t("common.files"), summary.files.to_string()),
            ModernStatusField::new(
                ctx.loc.t("common.total_size"),
                fmt_bytes(summary.total_size),
            ),
            ModernStatusField::new(ctx.loc.t("common.packed_size"), summary.packed_size_label()),
        ],
    );
    ctx.print_modern_table(
        &ctx.loc.t("cli.list.summary_title"),
        &[
            ModernTableColumn::right(ctx.loc.t("common.entries"), 8),
            ModernTableColumn::right(ctx.loc.t("common.files"), 8),
            ModernTableColumn::right(ctx.loc.t("common.directories"), 11),
            ModernTableColumn::right(ctx.loc.t("common.symlinks"), 10),
            ModernTableColumn::right(ctx.loc.t("common.total_size"), 13),
            ModernTableColumn::right(ctx.loc.t("common.packed_size"), 13),
        ],
        &[ModernTableRow::new(vec![
            entries.len().to_string(),
            summary.files.to_string(),
            summary.directories.to_string(),
            summary.symlinks.to_string(),
            fmt_bytes(summary.total_size),
            summary.packed_size_label(),
        ])],
    );
    ctx.print_modern_table(
        &ctx.loc.t("cli.list.mix_title"),
        &[
            ModernTableColumn::new(ctx.loc.t("common.type"), 16),
            ModernTableColumn::right(ctx.loc.t("common.entries"), 10),
            ModernTableColumn::right(ctx.loc.t("common.size"), 14),
            ModernTableColumn::right(ctx.loc.t("common.compressed"), 14),
        ],
        &entry_mix_rows(ctx, &summary),
    );
    let rows = entries
        .iter()
        .map(|entry| {
            let compressed = formatted_size_label(entry.compressed_size);
            ModernTableRow::new(vec![
                fmt_bytes(entry.size),
                compressed,
                entry_type_label(ctx, &entry.entry_type),
                entry.path.display.clone(),
            ])
        })
        .collect::<Vec<_>>();
    ctx.print_modern_table(
        &ctx.loc.t("cli.list.table_title"),
        &[
            ModernTableColumn::right(ctx.loc.t("common.size"), 12),
            ModernTableColumn::right(ctx.loc.t("common.compressed"), 12),
            ModernTableColumn::new(ctx.loc.t("common.type"), 9),
            ModernTableColumn::new(ctx.loc.t("common.name"), 52),
        ],
        &rows,
    );
}

fn entry_mix_rows(ctx: &Ctx, summary: &EntrySummary) -> Vec<ModernTableRow> {
    let mut rows = Vec::new();
    if summary.files > 0 {
        rows.push(ModernTableRow::success(vec![
            ctx.loc.t("cli.list.type.file"),
            summary.files.to_string(),
            fmt_bytes(summary.file_size),
            summary.file_packed_label(),
        ]));
    }
    if summary.directories > 0 {
        rows.push(ModernTableRow::new(vec![
            ctx.loc.t("cli.list.type.dir"),
            summary.directories.to_string(),
            "-".to_owned(),
            "-".to_owned(),
        ]));
    }
    if summary.symlinks > 0 {
        rows.push(ModernTableRow::new(vec![
            ctx.loc.t("cli.list.type.symlink"),
            summary.symlinks.to_string(),
            "-".to_owned(),
            "-".to_owned(),
        ]));
    }
    if summary.other > 0 {
        rows.push(ModernTableRow::warning(vec![
            ctx.loc.t("cli.list.type.other"),
            summary.other.to_string(),
            "-".to_owned(),
            "-".to_owned(),
        ]));
    }
    if rows.is_empty() {
        rows.push(ModernTableRow::new(vec![
            ctx.loc.t("cli.list.type.other"),
            "0".to_owned(),
            "-".to_owned(),
            "-".to_owned(),
        ]));
    }
    rows
}

fn missing_size_label() -> String {
    "-".to_owned()
}

fn raw_size_label(size: Option<u64>) -> String {
    match size {
        Some(size) => size.to_string(),
        None => missing_size_label(),
    }
}

fn formatted_size_label(size: Option<u64>) -> String {
    match size {
        Some(size) => fmt_bytes(size),
        None => missing_size_label(),
    }
}

struct EntrySummary {
    files: usize,
    directories: usize,
    symlinks: usize,
    other: usize,
    total_size: u64,
    file_size: u64,
    packed_size: Option<u64>,
    file_packed_size: Option<u64>,
}

impl EntrySummary {
    fn from_entries(entries: &[EntryMeta]) -> Self {
        let mut summary = Self {
            files: 0,
            directories: 0,
            symlinks: 0,
            other: 0,
            total_size: 0,
            file_size: 0,
            packed_size: Some(0),
            file_packed_size: Some(0),
        };
        for entry in entries {
            match &entry.entry_type {
                EntryType::File => {
                    summary.files += 1;
                    summary.file_size = summary.file_size.saturating_add(entry.size);
                    match (summary.file_packed_size, entry.compressed_size) {
                        (Some(total), Some(size)) => {
                            summary.file_packed_size = Some(total.saturating_add(size));
                        }
                        _ => summary.file_packed_size = None,
                    }
                }
                EntryType::Dir => summary.directories += 1,
                EntryType::Symlink { .. } | EntryType::Hardlink { .. } => summary.symlinks += 1,
                EntryType::Other => summary.other += 1,
            }
            summary.total_size = summary.total_size.saturating_add(entry.size);
            match (summary.packed_size, entry.compressed_size) {
                (Some(total), Some(size)) => summary.packed_size = Some(total.saturating_add(size)),
                _ => summary.packed_size = None,
            }
        }
        summary
    }

    fn packed_size_label(&self) -> String {
        formatted_size_label(self.packed_size)
    }

    fn file_packed_label(&self) -> String {
        formatted_size_label(self.file_packed_size)
    }
}

fn entry_type_label(ctx: &Ctx, entry_type: &EntryType) -> String {
    let key = match entry_type {
        EntryType::File => "cli.list.type.file",
        EntryType::Dir => "cli.list.type.dir",
        EntryType::Symlink { .. } => "cli.list.type.symlink",
        EntryType::Hardlink { .. } => "cli.list.type.hardlink",
        EntryType::Other => "cli.list.type.other",
    };
    ctx.loc.t(key)
}

#[derive(Default)]
struct TreeNode {
    is_dir: bool,
    children: BTreeMap<String, TreeNode>,
}

impl TreeNode {
    fn insert(&mut self, components: &[&str], is_dir: bool) {
        if components.is_empty() {
            return;
        }
        let mut current = self;
        for (idx, component) in components.iter().enumerate() {
            let last = idx + 1 == components.len();
            current = current.children.entry((*component).to_owned()).or_default();
            if !last || is_dir {
                current.is_dir = true;
            }
        }
    }
}

pub(crate) fn print_tree(entries: &[EntryMeta], modern: bool) {
    let mut root = TreeNode::default();
    for entry in entries {
        let components: Vec<&str> = entry
            .path
            .display
            .split('/')
            .filter(|component| !component.is_empty())
            .collect();
        root.insert(&components, matches!(entry.entry_type, EntryType::Dir));
    }

    println!(".");
    render_tree_children(&root, "", modern, &mut |line| println!("{line}"));
}

fn render_tree_children(node: &TreeNode, prefix: &str, modern: bool, emit: &mut dyn FnMut(String)) {
    let total = node.children.len();
    for (idx, (name, child)) in node.children.iter().enumerate() {
        let last = idx + 1 == total;
        let connector = match (modern, last) {
            (true, true) => "└── ",
            (true, false) => "├── ",
            (false, true) => "`-- ",
            (false, false) => "|-- ",
        };
        let suffix = if child.is_dir || !child.children.is_empty() {
            "/"
        } else {
            ""
        };
        emit(format!("{prefix}{connector}{name}{suffix}"));
        let next_prefix = match (modern, last) {
            (_, true) => format!("{prefix}    "),
            (true, false) => format!("{prefix}│   "),
            (false, false) => format!("{prefix}|   "),
        };
        render_tree_children(child, &next_prefix, modern, emit);
    }
}

/// Machine-readable projection of one entry.
pub(crate) fn entry_json(e: &EntryMeta) -> Value {
    let (entry_type, link_target) = match &e.entry_type {
        EntryType::File => ("file", None),
        EntryType::Dir => ("dir", None),
        EntryType::Symlink { target } => (
            "symlink",
            Some(String::from_utf8_lossy(target).into_owned()),
        ),
        EntryType::Hardlink { target } => (
            "hardlink",
            Some(String::from_utf8_lossy(target).into_owned()),
        ),
        EntryType::Other => ("other", None),
    };
    json!({
        "path": e.path.display,
        "encoding": e.path.encoding,
        "type": entry_type,
        "link_target": link_target,
        "size": e.size,
        "compressed_size": e.compressed_size,
        "modified": e
            .modified
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs()),
        "unix_mode": e.unix_mode,
        "crc32": e.crc32,
        "encrypted": e.encrypted,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use squallz_core::api::EntryPath;
    use std::time::{Duration, SystemTime};

    fn entry(
        path: &str,
        entry_type: EntryType,
        size: u64,
        compressed_size: Option<u64>,
    ) -> EntryMeta {
        EntryMeta {
            path: EntryPath::from_utf8(path),
            entry_type,
            size,
            compressed_size,
            modified: None,
            unix_mode: None,
            crc32: None,
            encrypted: false,
        }
    }

    #[test]
    fn entry_summary_drops_packed_totals_when_any_compressed_size_is_missing() {
        let entries = vec![
            entry("a.txt", EntryType::File, 100, Some(60)),
            entry("b.txt", EntryType::File, 40, None),
            entry("dir", EntryType::Dir, 0, None),
        ];

        let summary = EntrySummary::from_entries(&entries);

        assert_eq!(summary.files, 2);
        assert_eq!(summary.directories, 1);
        assert_eq!(summary.total_size, 140);
        assert_eq!(summary.packed_size_label(), "-");
        assert_eq!(summary.file_packed_label(), "-");
    }

    #[test]
    fn entry_summary_keeps_formatted_packed_totals_when_all_sizes_are_known() {
        let entries = vec![
            entry("a.txt", EntryType::File, 1024, Some(512)),
            entry("b.txt", EntryType::File, 2048, Some(1024)),
        ];

        let summary = EntrySummary::from_entries(&entries);

        assert_eq!(summary.files, 2);
        assert_eq!(summary.total_size, 3072);
        assert_eq!(summary.packed_size_label(), "1.5 KiB");
        assert_eq!(summary.file_packed_label(), "1.5 KiB");
    }

    #[test]
    fn entry_json_maps_links_and_pre_epoch_modified_time_without_panicking() {
        let mut link = entry(
            "link",
            EntryType::Symlink {
                target: b"target.txt".to_vec(),
            },
            0,
            None,
        );
        link.modified = Some(SystemTime::UNIX_EPOCH - Duration::from_secs(1));
        link.unix_mode = Some(0o120777);
        link.crc32 = Some(0x1234_ABCD);
        link.encrypted = true;

        let value = entry_json(&link);

        assert_eq!(value["path"], "link");
        assert_eq!(value["type"], "symlink");
        assert_eq!(value["link_target"], "target.txt");
        assert!(value["modified"].is_null());
        assert_eq!(value["unix_mode"], 0o120777);
        assert_eq!(value["crc32"], 0x1234_ABCDu32);
        assert_eq!(value["encrypted"], true);
    }
}
