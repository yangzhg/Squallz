//! Terminal progress reporting (hand-written, stderr).
//!
//! Modes:
//! - bar: progress HUD (percentage / bytes / speed / current entry), redrawn
//!   at most every 100 ms — only when stderr is a TTY;
//! - verbose: one line per entry (`--verbose`, works without a TTY too);
//! - silent: `--quiet`, `--json`, or a non-TTY stderr.
//!
//! The bar is purely decorative (digits, punctuation and the entry name), so
//! it carries no language-pack copy.

use std::io::{IsTerminal, Write};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use squallz_core::api::{EntryPath, ProgressSink};

use crate::args::{AccentArg, ColorArg, OutputStyleArg};
use crate::ui::{self, Tone};

/// Minimum interval between two bar redraws.
const REDRAW_INTERVAL: Duration = Duration::from_millis(100);
/// Cells in the classic gauge.
const CLASSIC_BAR_CELLS: usize = 28;
/// Cells in the modern gauge. The modern HUD is a compact panel, so the bar can
/// be visibly richer without making the status chips unreadable.
const MODERN_BAR_CELLS: usize = 34;
const MODERN_MINI_BAR_CELLS: usize = 16;
/// Inner width of the modern progress panel.
const MODERN_HUD_INNER_WIDTH: usize = 112;
/// The live HUD embeds a compact table. These widths are chosen so the table
/// border exactly spans the panel width.
const MODERN_SNAPSHOT_WIDTHS: [usize; 4] = [15, 27, 31, 30];
const MODERN_ACTION_WIDTHS: [usize; 4] = [18, 35, 30, 20];
/// Maximum rendered line width for plain progress lines.
const LINE_WIDTH: usize = 148;

#[derive(PartialEq)]
enum Mode {
    Silent,
    Bar {
        style: OutputStyleArg,
        color: bool,
        accent: AccentArg,
        operation: String,
    },
    Verbose,
}

struct State {
    start: Instant,
    last_draw: Option<Instant>,
    last_entry: String,
    drawn: bool,
    drawn_lines: usize,
    frame: usize,
}

fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// stderr progress sink shared by every command.
pub struct CliProgress {
    mode: Mode,
    state: Mutex<State>,
}

impl CliProgress {
    /// Picks the mode from the flags and the TTY-ness of stderr, with a visible
    /// operation tag for modern progress lines.
    pub fn new_for_operation(
        quiet: bool,
        verbose: bool,
        json: bool,
        output_style: OutputStyleArg,
        color: ColorArg,
        accent: AccentArg,
        operation: impl Into<String>,
    ) -> Self {
        let mode = if json || quiet {
            Mode::Silent
        } else if verbose {
            Mode::Verbose
        } else if std::io::stderr().is_terminal() {
            Mode::Bar {
                style: output_style,
                color: output_style.is_modern() && color.enabled(true),
                accent,
                operation: operation.into(),
            }
        } else {
            // Non-TTY without --verbose: degrade to silent.
            Mode::Silent
        };
        Self {
            mode,
            state: Mutex::new(State {
                start: Instant::now(),
                last_draw: None,
                last_entry: String::new(),
                drawn: false,
                drawn_lines: 0,
                frame: 0,
            }),
        }
    }

    /// Clears the progress line; call before printing final results.
    pub fn finish(&self) {
        if !matches!(self.mode, Mode::Bar { .. }) {
            return;
        }
        let mut state = lock_unpoisoned(&self.state);
        if state.drawn {
            clear_progress_block(state.drawn_lines.max(1));
            let _ = std::io::stderr().flush();
            state.drawn = false;
            state.drawn_lines = 0;
        }
    }

    fn draw_bar(&self, done: u64, total: u64, current: &EntryPath) {
        let mut state = lock_unpoisoned(&self.state);
        let finished = total > 0 && done >= total;
        if let Some(last) = state.last_draw {
            if !finished && last.elapsed() < REDRAW_INTERVAL {
                return;
            }
        }
        state.last_draw = Some(Instant::now());
        state.drawn = true;
        let frame = state.frame;
        state.frame = state.frame.wrapping_add(1);

        let elapsed_duration = state.start.elapsed();
        let elapsed = elapsed_duration.as_secs_f64();
        let speed = if elapsed > 0.05 {
            done as f64 / elapsed
        } else {
            0.0
        };
        // total == 0 means "unknown total" (streaming sources such as
        // .tar.gz extraction): show processed bytes and speed without a
        // percentage gauge.
        let (style, color, accent, operation) = match &self.mode {
            Mode::Bar {
                style,
                color,
                accent,
                operation,
            } => (*style, *color, *accent, operation.as_str()),
            _ => return,
        };
        let snapshot = ProgressFrame {
            operation,
            done,
            total,
            current: &current.display,
            speed: speed as u64,
            elapsed_secs: elapsed_duration.as_secs(),
            frame,
        };
        let block = render_progress_line(style, color, accent, snapshot);
        let block = normalize_progress_block(&block, color);
        let line_count = block.lines().count().max(1);
        write_progress_block(&block, state.drawn, state.drawn_lines.max(1));
        state.drawn_lines = line_count;
        let _ = std::io::stderr().flush();
    }

    fn print_verbose(&self, current: &EntryPath) {
        if current.display.is_empty() {
            return;
        }
        let mut state = lock_unpoisoned(&self.state);
        if state.last_entry != current.display {
            state.last_entry = current.display.clone();
            eprintln!("{}", current.display);
        }
    }
}

impl ProgressSink for CliProgress {
    fn on_progress(&self, done: u64, total: u64, current: &EntryPath) {
        match self.mode {
            Mode::Silent => {}
            Mode::Bar { .. } => self.draw_bar(done, total, current),
            Mode::Verbose => self.print_verbose(current),
        }
    }
}

#[derive(Clone, Copy)]
struct ProgressFrame<'a> {
    operation: &'a str,
    done: u64,
    total: u64,
    current: &'a str,
    speed: u64,
    elapsed_secs: u64,
    frame: usize,
}

fn render_progress_line(
    style: OutputStyleArg,
    color: bool,
    accent: AccentArg,
    snapshot: ProgressFrame<'_>,
) -> String {
    if style.is_modern() {
        return render_modern_progress_line(color, accent, snapshot);
    }
    if snapshot.total == 0 {
        format!(
            "[{}] {}  {}/s  {}",
            ".".repeat(CLASSIC_BAR_CELLS),
            fmt_bytes(snapshot.done),
            fmt_bytes(snapshot.speed),
            snapshot.current,
        )
    } else {
        let pct = percent(snapshot.done, snapshot.total);
        let filled = pct * CLASSIC_BAR_CELLS / 100;
        format!(
            "[{}{}] {:>3}%  {} / {}  {}/s  {}",
            "#".repeat(filled),
            "-".repeat(CLASSIC_BAR_CELLS - filled),
            pct,
            fmt_bytes(snapshot.done),
            fmt_bytes(snapshot.total),
            fmt_bytes(snapshot.speed),
            snapshot.current,
        )
    }
}

fn render_modern_progress_line(
    color: bool,
    accent: AccentArg,
    snapshot: ProgressFrame<'_>,
) -> String {
    let operation_raw = snapshot.operation.trim();
    let operation_label = operation_raw.to_ascii_uppercase();
    let operation_label = if operation_label.is_empty() {
        "WORK".to_owned()
    } else {
        operation_label
    };
    let pulse = if snapshot.total > 0 && snapshot.done >= snapshot.total {
        "◆"
    } else {
        ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧"][snapshot.frame % 8]
    };
    let state = if snapshot.total == 0 {
        "LIVE"
    } else if snapshot.done >= snapshot.total {
        "DONE"
    } else {
        "RUN"
    };
    let phase = modern_phase(operation_raw, snapshot.done, snapshot.total);
    let progress_tone = if snapshot.total > 0 && snapshot.done >= snapshot.total {
        Tone::Success
    } else {
        Tone::Primary
    };
    let top_right = if snapshot.total == 0 {
        "streaming".to_owned()
    } else {
        format!("{:>3}%", percent(snapshot.done, snapshot.total))
    };
    let top = modern_hud_top(
        &format!("{pulse} {operation_label} · {state} · operation cockpit · phase {phase}"),
        &top_right,
    );
    let status_eta = if snapshot.total == 0 {
        "ETA --".to_owned()
    } else {
        eta_label(snapshot.done, snapshot.total, snapshot.speed)
    };
    let phase_rail = modern_phase_rail(operation_raw, phase, snapshot.total);
    let elapsed = fmt_duration(snapshot.elapsed_secs);
    let status_line = modern_hud_content(&format!(
        "Phase {phase}   phase rail {phase_rail}   {status_eta}   elapsed {elapsed}   next {}",
        modern_next_phase(operation_raw, phase, snapshot.total),
    ));
    let snapshot_title = modern_metric_section(
        "Transfer board · Snapshot dashboard + Signal matrix + Transfer matrix",
    );
    let snapshot_header = modern_snapshot_header();
    let action_title = modern_metric_section("Action queue · route, cue, finish");
    let action_header = modern_action_header();
    let gauge = if snapshot.total == 0 {
        modern_hud_content(&format!(
            "▕{}▏  STREAM  processed {}  ·  adaptive read  ·  pulse {}",
            streaming_gauge(snapshot.frame),
            fmt_bytes(snapshot.done),
            modern_activity_spark(snapshot.frame),
        ))
    } else {
        let pct = percent(snapshot.done, snapshot.total);
        let filled = pct * MODERN_BAR_CELLS / 100;
        let gauge = format!(
            "{}{}",
            "▰".repeat(filled),
            "▱".repeat(MODERN_BAR_CELLS - filled)
        );
        modern_hud_content(&format!(
            "▕{gauge}▏  {pct:>3}%  {} / {}  ·  next {}  ·  pulse {}",
            fmt_bytes(snapshot.done),
            fmt_bytes(snapshot.total),
            modern_next_phase(operation_raw, phase, snapshot.total),
            modern_activity_spark(snapshot.frame),
        ))
    };
    let action_value = modern_action_value(
        operation_raw,
        phase,
        snapshot.done,
        snapshot.total,
        snapshot.speed,
        snapshot.current,
    );
    let snapshot_values = modern_snapshot_rows(snapshot, phase, &status_eta);

    let mut lines = vec![
        ui::paint_tone(color, accent, progress_tone, &top),
        ui::paint_tone(color, accent, Tone::Primary, &status_line),
        ui::paint_tone(color, accent, progress_tone, &gauge),
        ui::paint_tone(color, accent, Tone::Primary, &snapshot_title),
        ui::paint_tone(
            color,
            accent,
            Tone::Primary,
            &modern_metric_rule(&MODERN_SNAPSHOT_WIDTHS, "┬"),
        ),
        ui::paint_tone(color, accent, Tone::Primary, &snapshot_header),
        ui::paint_tone(
            color,
            accent,
            Tone::Primary,
            &modern_metric_rule(&MODERN_SNAPSHOT_WIDTHS, "┼"),
        ),
    ];
    for (idx, row) in snapshot_values.iter().enumerate() {
        let tone = if idx == 0 {
            progress_tone
        } else {
            Tone::Secondary
        };
        lines.push(ui::paint_tone(color, accent, tone, row));
    }
    lines.extend([
        ui::paint_tone(color, accent, Tone::Primary, &action_title),
        ui::paint_tone(
            color,
            accent,
            Tone::Primary,
            &modern_metric_rule(&MODERN_ACTION_WIDTHS, "┬"),
        ),
        ui::paint_tone(color, accent, Tone::Primary, &action_header),
        ui::paint_tone(
            color,
            accent,
            Tone::Primary,
            &modern_metric_rule(&MODERN_ACTION_WIDTHS, "┼"),
        ),
        ui::paint_tone(color, accent, progress_tone, &action_value),
        ui::paint_tone(color, accent, Tone::Primary, &modern_hud_bottom()),
    ]);
    lines.join("\n")
}

fn modern_snapshot_header() -> String {
    modern_metric_table_line(&[
        ("Metric", MODERN_SNAPSHOT_WIDTHS[0], ModernMetricAlign::Left),
        ("Value", MODERN_SNAPSHOT_WIDTHS[1], ModernMetricAlign::Left),
        ("Signal", MODERN_SNAPSHOT_WIDTHS[2], ModernMetricAlign::Left),
        ("Cue", MODERN_SNAPSHOT_WIDTHS[3], ModernMetricAlign::Left),
    ])
}

fn modern_snapshot_rows(snapshot: ProgressFrame<'_>, phase: &str, eta: &str) -> Vec<String> {
    let progress = if snapshot.total == 0 {
        "STREAM".to_owned()
    } else {
        format!("{:>3}% · {phase}", percent(snapshot.done, snapshot.total))
    };
    let payload = if snapshot.total == 0 {
        format!("processed {}", fmt_bytes(snapshot.done))
    } else {
        format!(
            "{} / {}",
            fmt_bytes(snapshot.done),
            fmt_bytes(snapshot.total)
        )
    };
    let eta = eta_without_prefix(eta);
    let speed_eta = if snapshot.total == 0 {
        format!("{}/s · adaptive read", fmt_bytes(snapshot.speed))
    } else {
        format!("{}/s · ETA {eta}", fmt_bytes(snapshot.speed))
    };
    let current = modern_current_label(snapshot.current, snapshot.done, snapshot.total);
    let progress_signal = modern_snapshot_signal(snapshot.done, snapshot.total, snapshot.frame);
    let payload_signal = format!(
        "{} · {}",
        modern_lane_label(snapshot.operation, snapshot.total),
        modern_guardrail_label(snapshot.operation)
    );
    let current_signal = modern_activity_spark(snapshot.frame);
    let speed_row_value = format!("{}/s", fmt_bytes(snapshot.speed));
    let speed_signal = if snapshot.total == 0 {
        "adaptive read".to_owned()
    } else {
        format!("ETA {eta}")
    };
    vec![
        modern_metric_table_line(&[
            (
                "Progress",
                MODERN_SNAPSHOT_WIDTHS[0],
                ModernMetricAlign::Left,
            ),
            (
                &progress,
                MODERN_SNAPSHOT_WIDTHS[1],
                ModernMetricAlign::Right,
            ),
            (
                &progress_signal,
                MODERN_SNAPSHOT_WIDTHS[2],
                ModernMetricAlign::Left,
            ),
            (
                &format!(
                    "next {}",
                    modern_next_phase(snapshot.operation, phase, snapshot.total)
                ),
                MODERN_SNAPSHOT_WIDTHS[3],
                ModernMetricAlign::Left,
            ),
        ]),
        modern_metric_table_line(&[
            (
                "Payload",
                MODERN_SNAPSHOT_WIDTHS[0],
                ModernMetricAlign::Left,
            ),
            (
                &payload,
                MODERN_SNAPSHOT_WIDTHS[1],
                ModernMetricAlign::Right,
            ),
            (
                &speed_eta,
                MODERN_SNAPSHOT_WIDTHS[2],
                ModernMetricAlign::Left,
            ),
            (
                modern_guardrail_label(snapshot.operation),
                MODERN_SNAPSHOT_WIDTHS[3],
                ModernMetricAlign::Left,
            ),
        ]),
        modern_metric_table_line(&[
            ("Speed", MODERN_SNAPSHOT_WIDTHS[0], ModernMetricAlign::Left),
            (
                &speed_row_value,
                MODERN_SNAPSHOT_WIDTHS[1],
                ModernMetricAlign::Right,
            ),
            (
                &speed_signal,
                MODERN_SNAPSHOT_WIDTHS[2],
                ModernMetricAlign::Left,
            ),
            (
                modern_operator_cue(snapshot.operation, phase, snapshot.total),
                MODERN_SNAPSHOT_WIDTHS[3],
                ModernMetricAlign::Left,
            ),
        ]),
        modern_metric_table_line(&[
            (
                "Current",
                MODERN_SNAPSHOT_WIDTHS[0],
                ModernMetricAlign::Left,
            ),
            (&current, MODERN_SNAPSHOT_WIDTHS[1], ModernMetricAlign::Left),
            (
                &payload_signal,
                MODERN_SNAPSHOT_WIDTHS[2],
                ModernMetricAlign::Left,
            ),
            (
                &format!(
                    "{} · {}",
                    modern_operator_cue(snapshot.operation, phase, snapshot.total),
                    current_signal
                ),
                MODERN_SNAPSHOT_WIDTHS[3],
                ModernMetricAlign::Left,
            ),
        ]),
    ]
}

fn modern_snapshot_signal(done: u64, total: u64, frame: usize) -> String {
    if total == 0 {
        return format!("{} · stream pulse", modern_stream_mini_gauge(frame));
    }
    let pct = percent(done, total);
    let filled = pct * MODERN_MINI_BAR_CELLS / 100;
    format!(
        "{}{} · {}",
        "▰".repeat(filled),
        "▱".repeat(MODERN_MINI_BAR_CELLS - filled),
        modern_activity_spark(frame)
    )
}

fn modern_stream_mini_gauge(frame: usize) -> String {
    let mut cells = vec!["·"; MODERN_MINI_BAR_CELLS];
    let head = frame % MODERN_MINI_BAR_CELLS;
    cells[head] = "◆";
    cells[(head + MODERN_MINI_BAR_CELLS - 1) % MODERN_MINI_BAR_CELLS] = "◇";
    cells[(head + 1) % MODERN_MINI_BAR_CELLS] = "◇";
    cells.join("")
}

fn modern_action_header() -> String {
    modern_metric_table_line(&[
        (
            "Route cue",
            MODERN_ACTION_WIDTHS[0],
            ModernMetricAlign::Left,
        ),
        (
            "Action cue",
            MODERN_ACTION_WIDTHS[1],
            ModernMetricAlign::Left,
        ),
        ("Finish", MODERN_ACTION_WIDTHS[2], ModernMetricAlign::Left),
        ("Display", MODERN_ACTION_WIDTHS[3], ModernMetricAlign::Left),
    ])
}

fn modern_action_value(
    operation: &str,
    phase: &str,
    done: u64,
    total: u64,
    speed: u64,
    current: &str,
) -> String {
    let route = format!(
        "now {phase} -> {}",
        modern_next_phase(operation, phase, total)
    );
    let finish = format!("finish {}", modern_finish_hint(operation));
    let display = if current.trim().is_empty() {
        format!("{}/s", fmt_bytes(speed))
    } else {
        format!("current {}", modern_current_label(current, done, total))
    };
    modern_metric_table_line(&[
        (&route, MODERN_ACTION_WIDTHS[0], ModernMetricAlign::Left),
        (
            modern_operator_cue(operation, phase, total),
            MODERN_ACTION_WIDTHS[1],
            ModernMetricAlign::Left,
        ),
        (&finish, MODERN_ACTION_WIDTHS[2], ModernMetricAlign::Left),
        (&display, MODERN_ACTION_WIDTHS[3], ModernMetricAlign::Left),
    ])
}

fn modern_operator_cue(operation: &str, phase: &str, total: u64) -> &'static str {
    if total == 0 {
        return match operation.trim().to_ascii_lowercase().as_str() {
            "extract" => "keep stream open until placement",
            "convert" | "export" => "preserve output handle",
            _ => "track streamed payload",
        };
    }
    if phase == "WRITE" || phase == "PLACE" || phase == "REPORT" || phase == "COMMIT" {
        return match operation.trim().to_ascii_lowercase().as_str() {
            "compress" | "pack" => "test output after write",
            "extract" => "review destination after place",
            "convert" | "export" => "inspect converted archive",
            "update" => "wait for atomic replace",
            "protect" => "verify recovery blocks",
            "repair" => "test repaired archive",
            "test" | "verify" => "read report table",
            _ => "confirm result table",
        };
    }
    match operation.trim().to_ascii_lowercase().as_str() {
        "compress" | "pack" => "feed archive writer",
        "extract" => "place files safely",
        "convert" | "export" => "stream entries to destination",
        "update" => "stage archive patch",
        "protect" => "build recovery parity",
        "repair" => "apply recovery blocks",
        "test" | "verify" => "validate payload checksums",
        _ => "keep job moving",
    }
}

fn modern_lane_label(operation: &str, total: u64) -> String {
    let streaming = total == 0;
    match operation.trim().to_ascii_lowercase().as_str() {
        "compress" | "pack" if streaming => "stream => archive".to_owned(),
        "compress" | "pack" => "source => archive".to_owned(),
        "extract" if streaming => "stream => dest".to_owned(),
        "extract" => "archive => dest".to_owned(),
        "test" | "verify" => "archive => report".to_owned(),
        "convert" | "export" => "archive => archive".to_owned(),
        "update" => "archive => patch".to_owned(),
        "protect" => "archive => parity".to_owned(),
        "repair" => "damage => repair".to_owned(),
        _ if streaming => "stream => output".to_owned(),
        _ => "input => output".to_owned(),
    }
}

fn modern_guardrail_label(operation: &str) -> &'static str {
    match operation.trim().to_ascii_lowercase().as_str() {
        "compress" | "pack" => "atomic output",
        "extract" => "safe extract",
        "test" | "verify" => "integrity read",
        "convert" | "export" => "format boundary",
        "update" => "atomic patch",
        "protect" => "parity plan",
        "repair" => "repair boundary",
        _ => "resource limits",
    }
}

#[derive(Clone, Copy)]
enum ModernMetricAlign {
    Left,
    Right,
}

fn modern_metric_rule(widths: &[usize], join: &str) -> String {
    let body = widths
        .iter()
        .map(|width| "─".repeat(width + 2))
        .collect::<Vec<_>>()
        .join(join);
    format!("├{body}┤")
}

fn modern_metric_section(title: &str) -> String {
    let title_budget = MODERN_HUD_INNER_WIDTH.saturating_sub(5);
    let title = ui::truncate_end(title, title_budget);
    let prefix = format!("├─ {title} ");
    let used = prefix.chars().count() + 1;
    let fill = "─".repeat((MODERN_HUD_INNER_WIDTH + 2).saturating_sub(used));
    format!("{prefix}{fill}┤")
}

fn modern_metric_table_line(cells: &[(&str, usize, ModernMetricAlign)]) -> String {
    let mut line = String::from("│");
    for (value, width, align) in cells {
        let width = *width;
        let value = ui::truncate_end(value, width);
        match *align {
            ModernMetricAlign::Left => {
                line.push_str(&format!(" {value:<width$} │"));
            }
            ModernMetricAlign::Right => {
                line.push_str(&format!(" {value:>width$} │"));
            }
        }
    }
    line
}

fn modern_current_label(current: &str, done: u64, total: u64) -> String {
    let current = current.trim();
    if current.is_empty() {
        if total > 0 && done >= total {
            return "finalizing".to_owned();
        }
        return "pending entry".to_owned();
    }
    truncate_middle(current, MODERN_HUD_INNER_WIDTH.saturating_sub(16))
}

fn modern_phase_rail(operation: &str, active_phase: &str, total: u64) -> String {
    let stages = modern_phase_stages(operation, total);
    stages
        .into_iter()
        .map(|stage| {
            if stage == active_phase {
                format!("● {stage}")
            } else {
                format!("○ {stage}")
            }
        })
        .collect::<Vec<_>>()
        .join(" ━━ ")
}

fn modern_next_phase(operation: &str, active_phase: &str, total: u64) -> &'static str {
    let stages = modern_phase_stages(operation, total);
    if let Some(next) = stages
        .iter()
        .position(|stage| *stage == active_phase)
        .and_then(|idx| stages.get(idx + 1))
        .copied()
    {
        next
    } else {
        "COMMIT"
    }
}

fn modern_finish_hint(operation: &str) -> &'static str {
    match operation.trim().to_ascii_lowercase().as_str() {
        "compress" | "pack" => "run sqz test",
        "extract" => "review destination",
        "test" | "verify" => "read report",
        "convert" | "export" => "inspect output",
        "update" => "atomic archive ready",
        "protect" => "verify recovery",
        "repair" => "test repaired output",
        _ => "result table",
    }
}

fn modern_phase_stages(operation: &str, total: u64) -> [&'static str; 3] {
    if total == 0 {
        return match operation.trim().to_ascii_lowercase().as_str() {
            "extract" => ["OPEN", "STREAM", "PLACE"],
            "convert" | "export" => ["READ", "STREAM", "WRITE"],
            _ => ["SCAN", "STREAM", "WRITE"],
        };
    }
    match operation.trim().to_ascii_lowercase().as_str() {
        "compress" | "pack" => ["SCAN", "PACK", "WRITE"],
        "extract" => ["OPEN", "UNPACK", "PLACE"],
        "test" | "verify" => ["OPEN", "VERIFY", "REPORT"],
        "convert" | "export" => ["READ", "TRANSCODE", "WRITE"],
        "update" => ["OPEN", "PATCH", "WRITE"],
        "protect" => ["SCAN", "PARITY", "WRITE"],
        "repair" => ["SCAN", "REPAIR", "WRITE"],
        _ => ["PREP", "WORK", "COMMIT"],
    }
}

fn modern_hud_top(left: &str, right: &str) -> String {
    let left_budget = MODERN_HUD_INNER_WIDTH
        .saturating_sub(right.chars().count())
        .saturating_sub(5);
    let left = ui::truncate_end(left, left_budget);
    let prefix = format!("╭─ {left} ");
    let used = prefix.chars().count() + right.chars().count() + 3;
    let fill = "─".repeat(MODERN_HUD_INNER_WIDTH.saturating_sub(used));
    format!("{prefix}{fill} {right} ─╮")
}

fn modern_hud_content(content: &str) -> String {
    let content = ui::truncate_end(content, MODERN_HUD_INNER_WIDTH);
    let padding = " ".repeat(MODERN_HUD_INNER_WIDTH.saturating_sub(content.chars().count()));
    format!("│ {content}{padding} │")
}

fn modern_hud_bottom() -> String {
    format!("╰{}╯", "─".repeat(MODERN_HUD_INNER_WIDTH + 2))
}

fn modern_phase(operation: &str, done: u64, total: u64) -> &'static str {
    if total == 0 {
        return "STREAM";
    }
    let pct = percent(done, total);
    match operation.trim().to_ascii_lowercase().as_str() {
        "compress" | "pack" => {
            if pct < 8 {
                "SCAN"
            } else if pct < 95 {
                "PACK"
            } else {
                "WRITE"
            }
        }
        "extract" => {
            if pct < 8 {
                "OPEN"
            } else if pct < 95 {
                "UNPACK"
            } else {
                "PLACE"
            }
        }
        "test" => "VERIFY",
        "protect" => {
            if pct < 12 {
                "SCAN"
            } else if pct < 95 {
                "PARITY"
            } else {
                "WRITE"
            }
        }
        "verify" => "VERIFY",
        "repair" => {
            if pct < 12 {
                "SCAN"
            } else if pct < 95 {
                "REPAIR"
            } else {
                "WRITE"
            }
        }
        "convert" | "export" => "TRANSCODE",
        "update" => "PATCH",
        _ => "WORK",
    }
}

fn streaming_gauge(frame: usize) -> String {
    let mut cells = vec!["·"; MODERN_BAR_CELLS];
    let head = frame % MODERN_BAR_CELLS;
    cells[head] = "◆";
    cells[(head + MODERN_BAR_CELLS - 1) % MODERN_BAR_CELLS] = "◇";
    cells[(head + 1) % MODERN_BAR_CELLS] = "◇";
    cells.join("")
}

fn modern_activity_spark(frame: usize) -> String {
    const SPARK: [&str; 8] = ["▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];
    (0..8)
        .map(|idx| SPARK[(frame + idx) % SPARK.len()])
        .collect::<Vec<_>>()
        .join("")
}

fn normalize_progress_block(block: &str, color: bool) -> String {
    if color {
        return block.to_owned();
    }
    block
        .lines()
        .map(|line| line.chars().take(LINE_WIDTH).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

fn write_progress_block(block: &str, had_previous: bool, previous_lines: usize) {
    if had_previous && previous_lines > 1 {
        eprint!("\x1b[{}F", previous_lines - 1);
    } else if had_previous {
        eprint!("\r");
    }
    for (idx, line) in block.lines().enumerate() {
        if idx > 0 {
            eprintln!();
        }
        eprint!("\r\x1b[2K{line}");
    }
}

fn clear_progress_block(lines: usize) {
    eprint!("\r\x1b[2K");
    for _ in 1..lines {
        eprint!("\x1b[1F\r\x1b[2K");
    }
    eprint!("\r");
}

fn eta_label(done: u64, total: u64, speed: u64) -> String {
    if speed == 0 || done >= total {
        return "ETA --".to_owned();
    }
    let remaining = total.saturating_sub(done);
    let seconds = remaining.div_ceil(speed);
    format!("ETA {}", fmt_duration(seconds))
}

fn fmt_duration(seconds: u64) -> String {
    if seconds < 60 {
        return format!("{seconds}s");
    }
    let minutes = seconds / 60;
    let seconds = seconds % 60;
    if minutes < 60 {
        return format!("{minutes}m{seconds:02}s");
    }
    let hours = minutes / 60;
    let minutes = minutes % 60;
    format!("{hours}h{minutes:02}m")
}

fn percent(done: u64, total: u64) -> usize {
    if total == 0 {
        return 100;
    }
    ((done.min(total) as u128 * 100) / total as u128) as usize
}

fn eta_without_prefix(eta: &str) -> &str {
    if let Some(stripped) = eta.strip_prefix("ETA ") {
        stripped
    } else {
        eta
    }
}

fn truncate_middle(value: &str, max_chars: usize) -> String {
    let chars: Vec<char> = value.chars().collect();
    if chars.len() <= max_chars {
        return value.to_owned();
    }
    if max_chars <= 1 {
        return "…".to_owned();
    }
    let head = (max_chars - 1) / 2;
    let tail = max_chars - 1 - head;
    let mut out = chars[..head].iter().collect::<String>();
    out.push('…');
    out.push_str(&chars[chars.len() - tail..].iter().collect::<String>());
    out
}

/// Human-readable byte count (binary units).
pub fn fmt_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = n as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{n} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn progress_frame<'a>(
        operation: &'a str,
        done: u64,
        total: u64,
        current: &'a str,
        speed: u64,
        elapsed_secs: u64,
        frame: usize,
    ) -> ProgressFrame<'a> {
        ProgressFrame {
            operation,
            done,
            total,
            current,
            speed,
            elapsed_secs,
            frame,
        }
    }

    #[test]
    fn bytes_formatting() {
        assert_eq!(fmt_bytes(0), "0 B");
        assert_eq!(fmt_bytes(512), "512 B");
        assert_eq!(fmt_bytes(2048), "2.0 KiB");
        assert_eq!(fmt_bytes(5 * 1024 * 1024 + 256 * 1024), "5.2 MiB");
    }

    fn progress_for_test(mode: Mode) -> CliProgress {
        CliProgress {
            mode,
            state: Mutex::new(State {
                start: Instant::now(),
                last_draw: None,
                last_entry: String::new(),
                drawn: false,
                drawn_lines: 0,
                frame: 0,
            }),
        }
    }

    #[test]
    fn cli_progress_recovers_after_state_lock_poison() {
        let progress = progress_for_test(Mode::Verbose);

        let poison = std::panic::catch_unwind(|| {
            let mut state = progress.state.lock().unwrap();
            state.last_entry = "before.txt".to_owned();
            panic!("poison progress state");
        });
        assert!(poison.is_err());

        progress.print_verbose(&EntryPath::from_utf8("after.txt"));

        let state = lock_unpoisoned(&progress.state);
        assert_eq!(state.last_entry, "after.txt");
    }

    #[test]
    fn modern_progress_line_is_rich_and_colorable() {
        let line = render_progress_line(
            OutputStyleArg::Modern,
            true,
            AccentArg::Teal,
            progress_frame(
                "compress",
                512 * 1024,
                1024 * 1024,
                "very/long/path/to/a/deeply/nested/archive-entry-with-a-long-name-and-many-extra-segments/2026/release/candidate/assets/large-design-export-final-final.txt",
                256 * 1024,
                1,
                0,
            ),
        );
        assert!(line.contains("COMPRESS"));
        assert!(line.contains("RUN"));
        assert!(line.contains("operation cockpit"));
        assert!(line.contains("Phase PACK"));
        assert!(line.contains("phase rail"));
        assert!(line.contains("finish run sqz test"));
        assert!(line.contains("Snapshot dashboard"));
        assert!(line.contains("Metric"));
        assert!(line.contains("Value"));
        assert!(line.contains("Progress"));
        assert!(line.contains("Payload"));
        assert!(line.contains("Speed"));
        assert!(line.contains("Current"));
        assert!(line.contains("Signal matrix"));
        assert!(line.contains("Signal"));
        assert!(line.contains("Cue"));
        assert!(line.contains("Transfer board"));
        assert!(line.contains("Transfer matrix"));
        assert!(line.contains("Action queue"));
        assert!(line.contains("Route cue"));
        assert!(line.contains("Action cue"));
        assert!(line.contains("feed archive writer"));
        assert!(line.contains("ETA"));
        assert!(line.contains("Current"));
        assert!(line.contains("source => archive"));
        assert!(line.contains("atomic output"));
        assert!(line.contains("PACK"));
        assert!(line.contains("○ SCAN ━━ ● PACK ━━ ○ WRITE"));
        assert!(line.contains('⠋'));
        assert!(line.contains('▰'));
        assert!(line.contains('▱'));
        assert!(line.contains('▕'));
        assert!(line.contains('▏'));
        assert!(line.contains("pulse"));
        assert!(line.contains("next WRITE"));
        assert!(line.contains("now PACK -> WRITE"));
        assert!(line.contains("elapsed 1s"));
        assert!(line.contains('╭'));
        assert!(line.contains('╰'));
        assert!(line.contains('┬'));
        assert!(line.contains('┼'));
        assert!(line.contains("512.0 KiB"));
        assert!(line.contains("50%"));
        assert!(line.contains("2s"));
        assert!(line.contains("ETA 2s"));
        assert!(line.contains("\x1b["));
        assert!(line.contains('…'));
        assert!(!line.contains("Scene dashboard"));
        assert!(!line.contains("Task board"));
        assert!(!line.contains("Workload board"));
        assert!(!line.contains("Focus"));
        assert!(!line.contains("Rhythm"));
        assert_eq!(line.lines().count(), 17);
    }

    #[test]
    fn modern_streaming_progress_uses_live_hud() {
        let line = render_progress_line(
            OutputStyleArg::Modern,
            false,
            AccentArg::Lagoon,
            progress_frame(
                "extract",
                768 * 1024,
                0,
                "streaming.tar.gz",
                128 * 1024,
                2,
                3,
            ),
        );
        assert!(line.contains("EXTRACT"));
        assert!(line.contains("LIVE"));
        assert!(line.contains("operation cockpit"));
        assert!(line.contains("Phase STREAM"));
        assert!(line.contains("phase rail"));
        assert!(line.contains("STREAM"));
        assert!(line.contains("finish review destination"));
        assert!(line.contains("Snapshot dashboard"));
        assert!(line.contains("Metric"));
        assert!(line.contains("Value"));
        assert!(line.contains("Progress"));
        assert!(line.contains("Payload"));
        assert!(line.contains("Speed"));
        assert!(line.contains("Current"));
        assert!(line.contains("Signal matrix"));
        assert!(line.contains("Signal"));
        assert!(line.contains("Cue"));
        assert!(line.contains("Transfer board"));
        assert!(line.contains("Transfer matrix"));
        assert!(line.contains("Action queue"));
        assert!(line.contains("Route cue"));
        assert!(line.contains("Action cue"));
        assert!(line.contains("stream => dest"));
        assert!(line.contains("safe extract"));
        assert!(line.contains("keep stream open until placement"));
        assert!(line.contains("○ OPEN ━━ ● STREAM ━━ ○ PLACE"));
        assert!(line.contains("adaptive read"));
        assert!(line.contains("processed 768.0 KiB"));
        assert!(line.contains("Payload"));
        assert!(line.contains("Speed"));
        assert!(line.contains("Current"));
        assert!(line.contains("128.0 KiB/s"));
        assert!(line.contains("ETA --"));
        assert!(line.contains("LIVE"));
        assert!(line.contains("streaming"));
        assert!(line.contains("STREAM "));
        assert!(line.contains("◇◆◇"));
        assert!(line.contains("pulse"));
        assert!(line.contains("adaptive read"));
        assert!(line.contains("processed 768.0 KiB"));
        assert!(line.contains("next PLACE"));
        assert!(line.contains('┬'));
        assert!(line.contains('┼'));
        assert!(line.contains("elapsed 2s"));
        assert_eq!(line.lines().count(), 17);
        assert!(!line.contains("\x1b["));
        assert!(!line.contains("Scene dashboard"));
        assert!(!line.contains("Task board"));
        assert!(!line.contains("Workload board"));
    }

    #[test]
    fn classic_progress_line_stays_plain_ascii() {
        let line = render_progress_line(
            OutputStyleArg::Classic,
            true,
            AccentArg::Teal,
            progress_frame(
                "compress",
                512 * 1024,
                1024 * 1024,
                "entry.txt",
                256 * 1024,
                1,
                0,
            ),
        );
        assert!(line.starts_with("[##############--------------]"));
        assert!(!line.contains("\x1b["));
        assert!(!line.contains('▰'));
    }
}
