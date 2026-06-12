//! Command implementations. `main.rs` only assembles the context and
//! dispatches here.

mod batch;
mod checksum;
mod compress;
mod convert;
mod doctor;
mod duplicates;
mod estimate;
mod export;
mod extract;
mod info;
mod list;
mod nested;
mod recovery;
mod reports;
mod test;
mod update;

use std::io::IsTerminal;
use std::sync::Arc;

use squallz_core::api::ControlToken;
use squallz_core::Engine;
use squallz_i18n::Localizer;

use crate::args::{effective_compression_level, AccentArg, Cmd, ColorArg, OutputStyleArg};
use crate::errors::CliError;
use crate::ui::{self, Tone};

const MODERN_PANEL_INNER_WIDTH: usize = 96;

/// Shared command context.
pub struct Ctx {
    pub engine: Engine,
    pub loc: Arc<Localizer>,
    pub ctl: Arc<ControlToken>,
    pub quiet: bool,
    pub verbose: bool,
    pub output_style: OutputStyleArg,
    pub color: ColorArg,
    pub accent: AccentArg,
}

impl Ctx {
    pub fn is_modern(&self) -> bool {
        self.output_style.is_modern()
    }

    pub fn print_success(&self, message: impl AsRef<str>) {
        let message = message.as_ref();
        if self.is_modern() {
            println!(
                "{}",
                self.paint_stdout_tone(Tone::Success, &format!("✓ {message}"))
            );
        } else {
            println!("{message}");
        }
    }

    pub fn eprint_notice(&self, message: impl AsRef<str>) {
        let message = message.as_ref();
        if self.is_modern() {
            eprintln!(
                "{}",
                self.paint_stderr_tone(Tone::Secondary, &format!("• {message}"))
            );
        } else {
            eprintln!("{message}");
        }
    }

    pub fn eprint_problem(&self, message: impl AsRef<str>) {
        let message = message.as_ref();
        if self.is_modern() {
            eprintln!(
                "{}",
                self.paint_stderr_tone(Tone::Warning, &format!("! {message}"))
            );
        } else {
            eprintln!("{message}");
        }
    }

    pub fn paint_stdout_tone(&self, tone: Tone, text: &str) -> String {
        ui::paint_tone(self.stdout_color(), self.accent, tone, text)
    }

    pub fn paint_stderr_tone(&self, tone: Tone, text: &str) -> String {
        ui::paint_tone(self.stderr_color(), self.accent, tone, text)
    }

    pub fn print_modern_table(
        &self,
        title: &str,
        columns: &[ModernTableColumn],
        rows: &[ModernTableRow],
    ) {
        self.print_modern_table_with_note(title, None, columns, rows);
    }

    pub fn print_modern_table_with_note(
        &self,
        title: &str,
        note: Option<&str>,
        columns: &[ModernTableColumn],
        rows: &[ModernTableRow],
    ) {
        let widths: Vec<usize> = columns.iter().map(|column| column.width).collect();
        println!();
        println!(
            "{}",
            self.paint_stdout_tone(Tone::Primary, &ui::table_title_rule(title, &widths))
        );
        if let Some(note) = note.filter(|note| !note.trim().is_empty()) {
            let note_width = ui::table_width(&widths).saturating_sub(4);
            println!(
                "{}",
                self.paint_stdout_tone(Tone::Secondary, &ui::panel_content_line(note, note_width))
            );
        }
        println!(
            "{}",
            self.paint_stdout_tone(Tone::Primary, &ui::table_rule("├", "┬", "┤", &widths))
        );
        println!(
            "{}",
            self.paint_stdout_tone(
                Tone::Primary,
                &render_modern_table_line(columns, &header_cells(columns))
            )
        );
        println!(
            "{}",
            self.paint_stdout_tone(Tone::Primary, &ui::table_rule("├", "┼", "┤", &widths))
        );
        for row in rows {
            let line = render_modern_table_line(columns, &row.cells);
            println!("{}", self.paint_stdout_tone(row.tone, &line));
        }
        println!(
            "{}",
            self.paint_stdout_tone(Tone::Primary, &ui::table_rule("╰", "┴", "╯", &widths))
        );
    }

    pub fn print_modern_wrapped_table(
        &self,
        title: &str,
        columns: &[ModernTableColumn],
        rows: &[ModernTableRow],
    ) {
        self.print_modern_wrapped_table_with_note(title, None, columns, rows);
    }

    pub fn print_modern_wrapped_table_with_note(
        &self,
        title: &str,
        note: Option<&str>,
        columns: &[ModernTableColumn],
        rows: &[ModernTableRow],
    ) {
        let widths: Vec<usize> = columns.iter().map(|column| column.width).collect();
        println!();
        println!(
            "{}",
            self.paint_stdout_tone(Tone::Primary, &ui::table_title_rule(title, &widths))
        );
        if let Some(note) = note.filter(|note| !note.trim().is_empty()) {
            let note_width = ui::table_width(&widths).saturating_sub(4);
            println!(
                "{}",
                self.paint_stdout_tone(Tone::Secondary, &ui::panel_content_line(note, note_width))
            );
        }
        println!(
            "{}",
            self.paint_stdout_tone(Tone::Primary, &ui::table_rule("├", "┬", "┤", &widths))
        );
        println!(
            "{}",
            self.paint_stdout_tone(
                Tone::Primary,
                &render_modern_table_line(columns, &header_cells(columns))
            )
        );
        println!(
            "{}",
            self.paint_stdout_tone(Tone::Primary, &ui::table_rule("├", "┼", "┤", &widths))
        );
        for row in rows {
            let wrapped = wrap_modern_row(columns, &row.cells);
            for idx in 0..wrapped.height {
                let cells = wrapped
                    .cells
                    .iter()
                    .map(|cell| wrapped_cell_line(cell, idx))
                    .collect::<Vec<_>>();
                println!(
                    "{}",
                    self.paint_stdout_tone(row.tone, &render_modern_table_line(columns, &cells))
                );
            }
        }
        println!(
            "{}",
            self.paint_stdout_tone(Tone::Primary, &ui::table_rule("╰", "┴", "╯", &widths))
        );
    }

    pub fn print_modern_status_panel(
        &self,
        title: &str,
        status: &str,
        tone: Tone,
        headline: &str,
        fields: &[ModernStatusField],
    ) {
        println!();
        println!(
            "{}",
            self.paint_stdout_tone(
                Tone::Primary,
                &ui::panel_title_rule(title, MODERN_PANEL_INNER_WIDTH)
            )
        );
        let status = format!("● {}", status.trim().to_ascii_uppercase());
        let headline_budget = MODERN_PANEL_INNER_WIDTH
            .saturating_sub(ui::display_width(&status))
            .saturating_sub(3);
        let headline = ui::truncate_end(headline, headline_budget);
        let lead = format!("{status} │ {headline}");
        println!(
            "{}",
            self.paint_stdout_tone(
                tone,
                &ui::panel_content_line(&lead, MODERN_PANEL_INNER_WIDTH)
            )
        );
        if !fields.is_empty() {
            println!(
                "{}",
                self.paint_stdout_tone(
                    Tone::Primary,
                    &ui::panel_separator(MODERN_PANEL_INNER_WIDTH)
                )
            );
            for chunk in fields.chunks(3) {
                let line = chunk
                    .iter()
                    .map(ModernStatusField::render)
                    .collect::<Vec<_>>()
                    .join("   ·   ");
                println!(
                    "{}",
                    self.paint_stdout_tone(
                        Tone::Secondary,
                        &ui::panel_content_line(&line, MODERN_PANEL_INNER_WIDTH)
                    )
                );
            }
        }
        println!(
            "{}",
            self.paint_stdout_tone(
                Tone::Primary,
                &ui::panel_bottom_rule(MODERN_PANEL_INNER_WIDTH)
            )
        );
    }

    pub fn stdout_color(&self) -> bool {
        self.is_modern() && self.color.enabled(std::io::stdout().is_terminal())
    }

    pub fn stderr_color(&self) -> bool {
        self.is_modern() && self.color.enabled(std::io::stderr().is_terminal())
    }
}

#[derive(Clone, Copy)]
pub enum ModernAlign {
    Left,
    Right,
}

pub struct ModernTableColumn {
    header: String,
    width: usize,
    align: ModernAlign,
}

impl ModernTableColumn {
    pub fn new(header: impl Into<String>, width: usize) -> Self {
        Self {
            header: header.into(),
            width,
            align: ModernAlign::Left,
        }
    }

    pub fn right(header: impl Into<String>, width: usize) -> Self {
        Self {
            header: header.into(),
            width,
            align: ModernAlign::Right,
        }
    }
}

pub struct ModernTableRow {
    cells: Vec<String>,
    tone: Tone,
}

impl ModernTableRow {
    pub fn new(cells: Vec<String>) -> Self {
        Self {
            cells,
            tone: Tone::Secondary,
        }
    }

    pub fn with_tone(cells: Vec<String>, tone: Tone) -> Self {
        Self { cells, tone }
    }

    pub fn success(cells: Vec<String>) -> Self {
        Self {
            cells,
            tone: Tone::Success,
        }
    }

    pub fn warning(cells: Vec<String>) -> Self {
        Self {
            cells,
            tone: Tone::Warning,
        }
    }

    pub fn danger(cells: Vec<String>) -> Self {
        Self {
            cells,
            tone: Tone::Danger,
        }
    }
}

pub struct ModernStatusField {
    label: String,
    value: String,
}

impl ModernStatusField {
    pub fn new(label: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            value: value.into(),
        }
    }

    fn render(&self) -> String {
        format!("{}: {}", self.label, self.value)
    }
}

fn header_cells(columns: &[ModernTableColumn]) -> Vec<String> {
    columns.iter().map(|column| column.header.clone()).collect()
}

struct WrappedModernRow {
    cells: Vec<Vec<String>>,
    height: usize,
}

fn wrap_modern_row(columns: &[ModernTableColumn], cells: &[String]) -> WrappedModernRow {
    let cells = columns
        .iter()
        .enumerate()
        .map(|(idx, column)| ui::wrap_words(cell_value(cells, idx), column.width))
        .collect::<Vec<_>>();
    let height = wrapped_height(&cells);
    WrappedModernRow { cells, height }
}

fn render_modern_table_line(columns: &[ModernTableColumn], cells: &[String]) -> String {
    let mut line = String::from("│");
    for (idx, column) in columns.iter().enumerate() {
        let value = cell_value(cells, idx);
        match column.align {
            ModernAlign::Left => {
                line.push_str(&format!(" {} │", ui::pad_end(value, column.width)));
            }
            ModernAlign::Right => {
                line.push_str(&format!(" {} │", ui::pad_start(value, column.width)));
            }
        }
    }
    line
}

fn cell_value(cells: &[String], idx: usize) -> &str {
    let Some(value) = cells.get(idx) else {
        return "";
    };
    value
}

fn wrapped_cell_line(cells: &[String], idx: usize) -> String {
    let Some(value) = cells.get(idx) else {
        return String::new();
    };
    value.clone()
}

fn wrapped_height(cells: &[Vec<String>]) -> usize {
    let mut height = 1;
    for cell in cells {
        height = height.max(cell.len());
    }
    height
}

/// Dispatches one parsed subcommand.
pub fn dispatch(cmd: Cmd, ctx: &Ctx) -> Result<(), CliError> {
    match cmd {
        Cmd::Compress {
            inputs,
            output,
            format,
            level,
            profile,
            password,
            encrypt_names,
            excludes,
            split,
            threads,
            memory_limit,
            json,
        } => compress::run(
            ctx,
            inputs,
            output,
            format,
            effective_compression_level(level, profile),
            password,
            encrypt_names,
            excludes,
            split,
            threads,
            memory_limit,
            json,
        ),
        Cmd::Pack {
            inputs,
            output,
            level,
            profile,
            inner_format,
            recovery,
            excludes,
            split,
            threads,
            memory_limit,
            json,
        } => compress::run_pack(
            ctx,
            inputs,
            output,
            effective_compression_level(level, profile),
            inner_format,
            recovery,
            excludes,
            split,
            threads,
            memory_limit,
            json,
        ),
        Cmd::Estimate {
            inputs,
            excludes,
            output,
            json,
        } => estimate::run(ctx, inputs, excludes, output, json),
        Cmd::Duplicates {
            inputs,
            excludes,
            min_size,
            json,
        } => duplicates::run(ctx, inputs, excludes, min_size, json),
        Cmd::Checksum {
            inputs,
            algorithm,
            check,
            excludes,
            json,
        } => checksum::run(ctx, inputs, algorithm.into(), check, excludes, json),
        Cmd::Extract {
            archive,
            dest,
            includes,
            overwrite,
            password,
            encoding,
            symlinks,
            smart,
            best_effort,
            threads,
            memory_limit,
            max_output_bytes,
            max_entries,
            max_compression_ratio,
            json,
        } => extract::run(
            ctx,
            archive,
            dest,
            includes,
            overwrite,
            password,
            encoding,
            symlinks,
            smart,
            best_effort,
            threads,
            memory_limit,
            max_output_bytes,
            max_entries,
            max_compression_ratio,
            json,
        ),
        Cmd::Convert {
            src,
            output,
            password,
            out_password,
            encrypt_names,
            level,
            profile,
            encoding,
            threads,
            memory_limit,
            json,
        } => convert::run(
            ctx,
            src,
            output,
            password,
            out_password,
            encrypt_names,
            effective_compression_level(level, profile),
            encoding,
            threads,
            memory_limit,
            json,
        ),
        Cmd::Nested { cmd } => nested::run(ctx, cmd),
        Cmd::Export {
            archive,
            output,
            level,
            profile,
            out_password,
            threads,
            memory_limit,
            json,
        } => export::run(
            ctx,
            archive,
            output,
            effective_compression_level(level, profile),
            out_password,
            threads,
            memory_limit,
            json,
        ),
        Cmd::Update {
            archive,
            add,
            mkdir,
            delete,
            rename,
            move_entries,
            excludes,
            password,
            encrypt_names,
            level,
            profile,
            threads,
            memory_limit,
            json,
        } => update::run(
            ctx,
            archive,
            add,
            mkdir,
            delete,
            rename,
            move_entries,
            excludes,
            password,
            encrypt_names,
            effective_compression_level(level, profile),
            threads,
            memory_limit,
            json,
        ),
        Cmd::Protect {
            archive,
            redundancy,
            tolerate_loss,
            recovery,
            json,
        } => recovery::protect(ctx, archive, redundancy, tolerate_loss, recovery, json),
        Cmd::Verify {
            archive,
            use_recovery,
            recovery,
            json,
        } => recovery::verify(ctx, archive, use_recovery, recovery, json),
        Cmd::Repair {
            archive,
            use_recovery,
            output,
            recovery,
            level,
            profile,
            threads,
            memory_limit,
            json,
        } => recovery::repair(
            ctx,
            archive,
            use_recovery,
            output,
            recovery,
            effective_compression_level(level, profile),
            threads,
            memory_limit,
            json,
        ),
        Cmd::Batch {
            script,
            keep_going,
            json,
        } => batch::run(ctx, script, keep_going, json),
        Cmd::Doctor { strict, json } => doctor::run(ctx, strict, json),
        Cmd::List {
            archive,
            password,
            encoding,
            json,
            tree,
        } => list::run(ctx, archive, password, encoding, json, tree),
        Cmd::Test {
            archive,
            password,
            encoding,
            json,
        } => test::run(ctx, archive, password, encoding, json),
        Cmd::Info { json } => info::run(ctx, json),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modern_table_line_pads_missing_cells_to_column_width() {
        let columns = vec![
            ModernTableColumn::new("Name", 5),
            ModernTableColumn::right("Size", 4),
        ];
        let cells = vec!["zip".to_owned()];

        let line = render_modern_table_line(&columns, &cells);

        assert_eq!(line, "│ zip   │      │");
        assert_eq!(ui::display_width(&line), ui::table_width(&[5, 4]));
    }
}
