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

use squallz_core::api::{EntryPath, ProgressPhase, ProgressSink};

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
    scanning: bool,
    phase: Option<ProgressPhase>,
    interruptible: bool,
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
                scanning: false,
                phase: None,
                interruptible: true,
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
        if state.scanning {
            state.start = Instant::now();
            state.last_draw = None;
            state.scanning = false;
        }
        let phase = state.phase;
        let interruptible = state.interruptible;
        let recovery_phase = is_recovery_progress_phase(phase);
        let (done, total) = if phase.is_some() && !interruptible && !recovery_phase {
            (0, 0)
        } else {
            (done, total)
        };
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
        let speed = if recovery_phase && total > 0 {
            0.0
        } else if elapsed > 0.05 {
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
            phase,
            interruptible,
        };
        let block = render_progress_line(style, color, accent, snapshot);
        let block = normalize_progress_block(&block, color);
        let line_count = block.lines().count().max(1);
        write_progress_block(&block, state.drawn, state.drawn_lines.max(1));
        state.drawn_lines = line_count;
        let _ = std::io::stderr().flush();
    }

    fn draw_scan(&self, entries: u64, current: &EntryPath) {
        let mut state = lock_unpoisoned(&self.state);
        state.scanning = true;
        state.phase = None;
        state.interruptible = true;
        if let Some(last) = state.last_draw {
            if last.elapsed() < REDRAW_INTERVAL {
                return;
            }
        }
        state.last_draw = Some(Instant::now());
        state.drawn = true;
        let frame = state.frame;
        state.frame = state.frame.wrapping_add(1);

        let (style, color, accent, operation) = match &self.mode {
            Mode::Bar {
                style,
                color,
                accent,
                operation,
            } => (*style, *color, *accent, operation.as_str()),
            _ => return,
        };
        let block = render_scan_progress_line(
            style,
            color,
            accent,
            operation,
            entries,
            &current.display,
            frame,
        );
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

    fn begin_phase(&self, phase: ProgressPhase, interruptible: bool) {
        {
            let mut state = lock_unpoisoned(&self.state);
            state.start = Instant::now();
            state.last_draw = None;
            state.scanning = false;
            state.phase = Some(phase);
            state.interruptible = interruptible;
        }
        match self.mode {
            Mode::Silent => {}
            Mode::Bar { .. } => self.draw_bar(0, 0, &EntryPath::from_utf8("")),
            Mode::Verbose => eprintln!("-- {} --", progress_phase_label(phase)),
        }
    }
}

impl ProgressSink for CliProgress {
    fn on_scan_progress(&self, entries: u64, current: &EntryPath) {
        match self.mode {
            Mode::Silent => {}
            Mode::Bar { .. } => self.draw_scan(entries, current),
            Mode::Verbose => self.print_verbose(current),
        }
    }

    fn on_progress(&self, done: u64, total: u64, current: &EntryPath) {
        match self.mode {
            Mode::Silent => {}
            Mode::Bar { .. } => self.draw_bar(done, total, current),
            Mode::Verbose => self.print_verbose(current),
        }
    }

    fn on_phase(&self, phase: ProgressPhase, interruptible: bool) {
        self.begin_phase(phase, interruptible);
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
    phase: Option<ProgressPhase>,
    interruptible: bool,
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
    if is_recovery_progress_phase(snapshot.phase) && snapshot.total > 0 {
        let pct = percent(snapshot.done, snapshot.total);
        let filled = pct * CLASSIC_BAR_CELLS / 100;
        return format!(
            "[{}{}] {} {:>3}%  {}",
            "#".repeat(filled),
            "-".repeat(CLASSIC_BAR_CELLS - filled),
            snapshot
                .phase
                .map(progress_phase_label)
                .unwrap_or("RECOVERY"),
            pct,
            snapshot.current,
        );
    }
    if snapshot.phase == Some(ProgressPhase::RecoveryPrepare)
        && snapshot.total == 0
        && snapshot.done > 0
    {
        return format!(
            "[{}] PREPARE  {}  {}/s  {}",
            ".".repeat(CLASSIC_BAR_CELLS),
            fmt_bytes(snapshot.done),
            fmt_bytes(snapshot.speed),
            snapshot.current,
        );
    }
    if snapshot.total == 0 {
        if let Some(phase) = snapshot.phase {
            return format!(
                "[{}] {} active  {}",
                ".".repeat(CLASSIC_BAR_CELLS),
                progress_phase_label(phase),
                snapshot.current,
            );
        }
        format!(
            "[{}] {}  {}/s  {}",
            ".".repeat(CLASSIC_BAR_CELLS),
            fmt_bytes(snapshot.done),
            fmt_bytes(snapshot.speed),
            snapshot.current,
        )
    } else {
        let phase = snapshot
            .phase
            .map(|phase| format!("{}  ", progress_phase_label(phase)))
            .unwrap_or_default();
        let pct = percent(snapshot.done, snapshot.total);
        let filled = pct * CLASSIC_BAR_CELLS / 100;
        format!(
            "[{}{}] {phase}{:>3}%  {} / {}  {}/s  {}",
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

fn render_scan_progress_line(
    style: OutputStyleArg,
    color: bool,
    accent: AccentArg,
    operation: &str,
    entries: u64,
    current: &str,
    frame: usize,
) -> String {
    if !style.is_modern() {
        return format!(
            "[{}] SCAN #{entries}  {current}",
            ".".repeat(CLASSIC_BAR_CELLS),
        );
    }

    let operation = operation.trim().to_ascii_uppercase();
    let operation = if operation.is_empty() {
        "WORK".to_owned()
    } else {
        operation
    };
    let pulse = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧"][frame % 8];
    let top = modern_hud_top(
        &format!("{pulse} {operation} · SCAN"),
        &format!("#{entries}"),
    );
    let gauge = modern_hud_content(&format!(
        "▕{}▏  SCAN #{entries}  ·  {}",
        streaming_gauge(frame),
        modern_activity_spark(frame),
    ));
    let current = truncate_middle(current, MODERN_HUD_INNER_WIDTH.saturating_sub(6));
    let current = modern_hud_content(&format!("SCAN  {current}"));
    [
        ui::paint_tone(color, accent, Tone::Primary, &top),
        ui::paint_tone(color, accent, Tone::Primary, &gauge),
        ui::paint_tone(color, accent, Tone::Secondary, &current),
        ui::paint_tone(color, accent, Tone::Primary, &modern_hud_bottom()),
    ]
    .join("\n")
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
    let phase_is_explicit = snapshot.phase.is_some();
    let recovery_percentage = is_recovery_progress_phase(snapshot.phase) && snapshot.total > 0;
    let pulse = if !phase_is_explicit && snapshot.total > 0 && snapshot.done >= snapshot.total {
        "◆"
    } else {
        ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧"][snapshot.frame % 8]
    };
    let recovery_streaming = snapshot.phase == Some(ProgressPhase::RecoveryPrepare)
        && snapshot.total == 0
        && snapshot.done > 0;
    let phase_without_total = phase_is_explicit && snapshot.total == 0 && !recovery_streaming;
    let state = if phase_is_explicit && !snapshot.interruptible {
        "SAFE"
    } else if phase_is_explicit {
        "RUN"
    } else if snapshot.total == 0 {
        "LIVE"
    } else if !phase_is_explicit && snapshot.done >= snapshot.total {
        "DONE"
    } else {
        "RUN"
    };
    let phase = snapshot
        .phase
        .map(progress_phase_label)
        .unwrap_or_else(|| modern_phase(operation_raw, snapshot.done, snapshot.total));
    let progress_tone =
        if !phase_is_explicit && snapshot.total > 0 && snapshot.done >= snapshot.total {
            Tone::Success
        } else {
            Tone::Primary
        };
    let top_right = if phase_without_total {
        "phase active".to_owned()
    } else if snapshot.total == 0 {
        "streaming".to_owned()
    } else {
        format!("{:>3}%", percent(snapshot.done, snapshot.total))
    };
    let top = modern_hud_top(
        &format!("{pulse} {operation_label} · {state} · operation cockpit · phase {phase}"),
        &top_right,
    );
    let status_eta = if snapshot.total == 0 || recovery_percentage {
        "ETA --".to_owned()
    } else {
        eta_label(snapshot.done, snapshot.total, snapshot.speed)
    };
    let phase_rail = modern_phase_rail(operation_raw, phase, snapshot.total, snapshot.phase);
    let elapsed = fmt_duration(snapshot.elapsed_secs);
    let status_line = modern_hud_content(&format!(
        "Phase {phase}   phase rail {phase_rail}   {status_eta}   elapsed {elapsed}   next {}",
        modern_next_phase(operation_raw, phase, snapshot.total, snapshot.phase),
    ));
    let snapshot_title = modern_metric_section(
        "Transfer board · Snapshot dashboard + Signal matrix + Transfer matrix",
    );
    let snapshot_header = modern_snapshot_header();
    let action_title = modern_metric_section("Action queue · route, cue, finish");
    let action_header = modern_action_header();
    let gauge = if phase_without_total {
        modern_hud_content(&format!(
            "▕{}▏  PHASE {phase}  ·  byte total unavailable  ·  pulse {}",
            streaming_gauge(snapshot.frame),
            modern_activity_spark(snapshot.frame),
        ))
    } else if snapshot.total == 0 {
        modern_hud_content(&format!(
            "▕{}▏  STREAM  processed {}  ·  adaptive read  ·  pulse {}",
            streaming_gauge(snapshot.frame),
            fmt_bytes(snapshot.done),
            modern_activity_spark(snapshot.frame),
        ))
    } else if recovery_percentage {
        let pct = percent(snapshot.done, snapshot.total);
        let filled = pct * MODERN_BAR_CELLS / 100;
        let gauge = format!(
            "{}{}",
            "▰".repeat(filled),
            "▱".repeat(MODERN_BAR_CELLS - filled)
        );
        modern_hud_content(&format!(
            "▕{gauge}▏  {pct:>3}%  phase progress  ·  next {}  ·  pulse {}",
            modern_next_phase(operation_raw, phase, snapshot.total, snapshot.phase),
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
            modern_next_phase(operation_raw, phase, snapshot.total, snapshot.phase),
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
        snapshot.phase,
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
    let recovery_streaming = snapshot.phase == Some(ProgressPhase::RecoveryPrepare)
        && snapshot.total == 0
        && snapshot.done > 0;
    let phase_without_total =
        snapshot.phase.is_some() && snapshot.total == 0 && !recovery_streaming;
    let recovery_percentage = is_recovery_progress_phase(snapshot.phase) && snapshot.total > 0;
    let progress = if phase_without_total {
        format!("{phase} · ACTIVE")
    } else if snapshot.total == 0 {
        "STREAM".to_owned()
    } else {
        format!("{:>3}% · {phase}", percent(snapshot.done, snapshot.total))
    };
    let payload = if phase_without_total {
        "byte total unavailable".to_owned()
    } else if recovery_percentage {
        "backend stage progress".to_owned()
    } else if snapshot.total == 0 {
        format!("processed {}", fmt_bytes(snapshot.done))
    } else {
        format!(
            "{} / {}",
            fmt_bytes(snapshot.done),
            fmt_bytes(snapshot.total)
        )
    };
    let eta = eta_without_prefix(eta);
    let speed_eta = if phase_without_total {
        "phase-local metrics pending".to_owned()
    } else if recovery_percentage {
        "stage-local percentage".to_owned()
    } else if snapshot.total == 0 {
        format!("{}/s · adaptive read", fmt_bytes(snapshot.speed))
    } else {
        format!("{}/s · ETA {eta}", fmt_bytes(snapshot.speed))
    };
    let current = modern_current_label(snapshot.current, snapshot.done, snapshot.total);
    let progress_signal = if phase_without_total {
        format!("{} · phase pulse", modern_stream_mini_gauge(snapshot.frame))
    } else {
        modern_snapshot_signal(snapshot.done, snapshot.total, snapshot.frame)
    };
    let payload_signal = if phase_without_total || recovery_percentage {
        modern_explicit_phase_signal(snapshot.phase).to_owned()
    } else {
        format!(
            "{} · {}",
            modern_lane_label(snapshot.operation, snapshot.total),
            modern_guardrail_label(snapshot.operation)
        )
    };
    let current_signal = modern_activity_spark(snapshot.frame);
    let speed_row_value = if phase_without_total || recovery_percentage {
        "--".to_owned()
    } else {
        format!("{}/s", fmt_bytes(snapshot.speed))
    };
    let speed_signal = if phase_without_total {
        "not reported".to_owned()
    } else if recovery_percentage {
        "backend reported".to_owned()
    } else if snapshot.total == 0 {
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
                    modern_next_phase(snapshot.operation, phase, snapshot.total, snapshot.phase,)
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
                modern_operator_cue(snapshot.operation, phase, snapshot.total, snapshot.phase),
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
                    modern_operator_cue(snapshot.operation, phase, snapshot.total, snapshot.phase,),
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
    explicit_phase: Option<ProgressPhase>,
) -> String {
    let route = format!(
        "now {phase} -> {}",
        modern_next_phase(operation, phase, total, explicit_phase)
    );
    let finish = format!("finish {}", modern_finish_hint(operation));
    let display = if is_recovery_progress_phase(explicit_phase) && total > 0 {
        format!("stage {:>3}%", percent(done, total))
    } else if explicit_phase.is_some()
        && total == 0
        && !(explicit_phase == Some(ProgressPhase::RecoveryPrepare) && done > 0)
    {
        "phase active".to_owned()
    } else if current.trim().is_empty() {
        format!("{}/s", fmt_bytes(speed))
    } else {
        format!("current {}", modern_current_label(current, done, total))
    };
    modern_metric_table_line(&[
        (&route, MODERN_ACTION_WIDTHS[0], ModernMetricAlign::Left),
        (
            modern_operator_cue(operation, phase, total, explicit_phase),
            MODERN_ACTION_WIDTHS[1],
            ModernMetricAlign::Left,
        ),
        (&finish, MODERN_ACTION_WIDTHS[2], ModernMetricAlign::Left),
        (&display, MODERN_ACTION_WIDTHS[3], ModernMetricAlign::Left),
    ])
}

fn modern_operator_cue(
    operation: &str,
    phase: &str,
    total: u64,
    explicit_phase: Option<ProgressPhase>,
) -> &'static str {
    if is_recovery_progress_phase(explicit_phase) {
        return match explicit_phase {
            Some(ProgressPhase::RecoveryPrepare) => "prepare recovery inputs",
            Some(ProgressPhase::RecoveryVerify) => "check protected data",
            Some(ProgressPhase::RecoveryProcess) => "process recovery blocks",
            Some(ProgressPhase::RecoveryFinalize) => "finish recovery result",
            _ => "follow recovery stage",
        };
    }
    if explicit_phase == Some(ProgressPhase::OutputSplit) {
        return "write physical volume set";
    }
    if matches!(
        explicit_phase,
        Some(
            ProgressPhase::OutputRecovery
                | ProgressPhase::OutputCommit
                | ProgressPhase::OutputCleanup
        )
    ) {
        return "let durable publish finish";
    }
    if matches!(phase, "RECOVER" | "COMMIT" | "CLEANUP") {
        return "let durable update finish";
    }
    if matches!(phase, "REWRITE" | "VERIFY") && total == 0 {
        return "await phase byte total";
    }
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

fn modern_explicit_phase_signal(phase: Option<ProgressPhase>) -> &'static str {
    match phase {
        Some(
            ProgressPhase::RecoveryPrepare
            | ProgressPhase::RecoveryVerify
            | ProgressPhase::RecoveryProcess
            | ProgressPhase::RecoveryFinalize,
        ) => "recovery engine · stage progress",
        Some(ProgressPhase::OutputSplit) => "volume output · byte progress",
        Some(
            ProgressPhase::OutputRecovery
            | ProgressPhase::OutputVerify
            | ProgressPhase::OutputCommit
            | ProgressPhase::OutputCleanup,
        ) => "durable publish · integrity guard",
        Some(
            ProgressPhase::UpdateRecovery
            | ProgressPhase::UpdateRewrite
            | ProgressPhase::UpdateVerify
            | ProgressPhase::UpdateCommit
            | ProgressPhase::UpdateCleanup,
        ) => "durable update · integrity guard",
        _ => "durable work · integrity guard",
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

fn modern_phase_rail(
    operation: &str,
    active_phase: &str,
    total: u64,
    explicit_phase: Option<ProgressPhase>,
) -> String {
    let stages: Vec<_> = match explicit_phase {
        Some(phase) => explicit_phase_stages(phase).to_vec(),
        None => modern_phase_stages(operation, total).into_iter().collect(),
    };
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

fn modern_next_phase(
    operation: &str,
    active_phase: &str,
    total: u64,
    explicit_phase: Option<ProgressPhase>,
) -> &'static str {
    let stages: Vec<_> = match explicit_phase {
        Some(phase) => explicit_phase_stages(phase).to_vec(),
        None => modern_phase_stages(operation, total).into_iter().collect(),
    };
    if let Some(next) = stages
        .iter()
        .position(|stage| *stage == active_phase)
        .and_then(|idx| stages.get(idx + 1))
        .copied()
    {
        next
    } else {
        if explicit_phase.is_some() {
            "RESULT"
        } else {
            "COMMIT"
        }
    }
}

fn explicit_phase_stages(phase: ProgressPhase) -> &'static [&'static str] {
    const RECOVERY_STAGES: &[&str] = &["PREPARE", "VERIFY", "PROCESS", "FINALIZE"];
    const SPLIT_OUTPUT_STAGES: &[&str] = &["SPLIT", "PUBLISH", "CLEANUP"];
    const OUTPUT_STAGES: &[&str] = &["RECOVER", "VERIFY", "PUBLISH", "CLEANUP"];
    const UPDATE_STAGES: &[&str] = &["RECOVER", "REWRITE", "VERIFY", "COMMIT", "CLEANUP"];
    const FALLBACK_STAGES: &[&str] = &["WORK"];

    match phase {
        ProgressPhase::RecoveryPrepare
        | ProgressPhase::RecoveryVerify
        | ProgressPhase::RecoveryProcess
        | ProgressPhase::RecoveryFinalize => RECOVERY_STAGES,
        ProgressPhase::OutputSplit => SPLIT_OUTPUT_STAGES,
        ProgressPhase::OutputRecovery
        | ProgressPhase::OutputVerify
        | ProgressPhase::OutputCommit
        | ProgressPhase::OutputCleanup => OUTPUT_STAGES,
        ProgressPhase::UpdateRecovery
        | ProgressPhase::UpdateRewrite
        | ProgressPhase::UpdateVerify
        | ProgressPhase::UpdateCommit
        | ProgressPhase::UpdateCleanup => UPDATE_STAGES,
        _ => FALLBACK_STAGES,
    }
}

fn progress_phase_label(phase: ProgressPhase) -> &'static str {
    match phase {
        ProgressPhase::RecoveryPrepare => "PREPARE",
        ProgressPhase::RecoveryVerify => "VERIFY",
        ProgressPhase::RecoveryProcess => "PROCESS",
        ProgressPhase::RecoveryFinalize => "FINALIZE",
        ProgressPhase::OutputSplit => "SPLIT",
        ProgressPhase::OutputRecovery => "RECOVER",
        ProgressPhase::OutputVerify => "VERIFY",
        ProgressPhase::OutputCommit => "PUBLISH",
        ProgressPhase::OutputCleanup => "CLEANUP",
        ProgressPhase::UpdateRecovery => "RECOVER",
        ProgressPhase::UpdateRewrite => "REWRITE",
        ProgressPhase::UpdateVerify => "VERIFY",
        ProgressPhase::UpdateCommit => "COMMIT",
        ProgressPhase::UpdateCleanup => "CLEANUP",
        _ => "WORK",
    }
}

fn is_recovery_progress_phase(phase: Option<ProgressPhase>) -> bool {
    matches!(
        phase,
        Some(
            ProgressPhase::RecoveryPrepare
                | ProgressPhase::RecoveryVerify
                | ProgressPhase::RecoveryProcess
                | ProgressPhase::RecoveryFinalize
        )
    )
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
            phase: None,
            interruptible: true,
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
                scanning: false,
                phase: None,
                interruptible: true,
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
    fn update_phases_do_not_report_the_task_done_early() {
        let mut verify = progress_frame("update", 100, 100, "archive.zip", 64, 1, 0);
        verify.phase = Some(ProgressPhase::UpdateVerify);
        let verify_line =
            render_progress_line(OutputStyleArg::Modern, false, AccentArg::Lagoon, verify);
        assert!(verify_line.contains("UPDATE · RUN"));
        assert!(verify_line.contains("Phase VERIFY"));
        assert!(verify_line.contains("REWRITE"));
        assert!(verify_line.contains("CLEANUP"));
        assert!(!verify_line.contains("UPDATE · DONE"));

        let mut commit = progress_frame("update", 0, 0, "archive.zip", 0, 0, 1);
        commit.phase = Some(ProgressPhase::UpdateCommit);
        commit.interruptible = false;
        let commit_line =
            render_progress_line(OutputStyleArg::Modern, false, AccentArg::Lagoon, commit);
        assert!(commit_line.contains("UPDATE · SAFE"));
        assert!(commit_line.contains("Phase COMMIT"));
        assert!(commit_line.contains("byte total unavailable"));
        assert!(!commit_line.contains("100%"));
        assert!(!commit_line.contains("adaptive read"));
        assert!(!commit_line.contains("processed 0 B"));

        let classic_commit =
            render_progress_line(OutputStyleArg::Classic, false, AccentArg::Lagoon, commit);
        assert!(classic_commit.contains("COMMIT active"));
        assert!(!classic_commit.contains("/s"));

        let mut publish = progress_frame("compress", 0, 0, "archive.zip", 0, 0, 2);
        publish.phase = Some(ProgressPhase::OutputCommit);
        publish.interruptible = false;
        let publish_line =
            render_progress_line(OutputStyleArg::Modern, false, AccentArg::Lagoon, publish);
        assert!(publish_line.contains("Phase PUBLISH"));
        assert!(publish_line.contains("○ RECOVER ━━ ○ VERIFY ━━ ● PUBLISH ━━ ○ CLEANUP"));
        assert!(publish_line.contains("let durable publish finish"));
        assert!(!publish_line.to_ascii_lowercase().contains("update"));

        let mut split = progress_frame(
            "compress",
            64 * 1024,
            256 * 1024,
            "archive.zip.001",
            64 * 1024,
            1,
            3,
        );
        split.phase = Some(ProgressPhase::OutputSplit);
        let split_line =
            render_progress_line(OutputStyleArg::Modern, false, AccentArg::Lagoon, split);
        assert!(split_line.contains("Phase SPLIT"));
        assert!(split_line.contains("● SPLIT ━━ ○ PUBLISH ━━ ○ CLEANUP"));
        assert!(!split_line.contains("RECOVER"));
        assert_eq!(
            modern_explicit_phase_signal(Some(ProgressPhase::OutputSplit)),
            "volume output · byte progress"
        );
        assert_eq!(
            modern_operator_cue(
                "compress",
                "SPLIT",
                256 * 1024,
                Some(ProgressPhase::OutputSplit)
            ),
            "write physical volume set"
        );
    }

    #[test]
    fn recovery_stage_percentages_are_not_presented_as_bytes() {
        let mut recovery = progress_frame("repair", 380, 1000, "archive.zip", 512, 1, 0);
        recovery.phase = Some(ProgressPhase::RecoveryProcess);
        recovery.interruptible = false;

        let classic =
            render_progress_line(OutputStyleArg::Classic, false, AccentArg::Lagoon, recovery);
        assert!(classic.contains("PROCESS"));
        assert!(classic.contains("38%"));
        assert!(!classic.contains("380 B"));
        assert!(!classic.contains("1000 B"));
        assert!(!classic.contains("/s"));

        let modern =
            render_progress_line(OutputStyleArg::Modern, false, AccentArg::Lagoon, recovery);
        assert!(modern.contains("Phase PROCESS"));
        assert!(modern.contains("phase progress"));
        assert!(modern.contains("backend stage progress"));
        assert!(modern.contains("stage-local percentage"));
        assert!(!modern.contains("380 B"));
        assert!(!modern.contains("1000 B"));
        assert!(!modern.contains("/s"));
    }

    #[test]
    fn recovery_prepare_copy_keeps_real_streamed_byte_feedback() {
        let mut recovery = progress_frame("repair", 768 * 1024, 0, "archive.zip", 128 * 1024, 2, 1);
        recovery.phase = Some(ProgressPhase::RecoveryPrepare);

        let classic =
            render_progress_line(OutputStyleArg::Classic, false, AccentArg::Lagoon, recovery);
        assert!(classic.contains("PREPARE"));
        assert!(classic.contains("768.0 KiB"));
        assert!(classic.contains("128.0 KiB/s"));

        let modern =
            render_progress_line(OutputStyleArg::Modern, false, AccentArg::Lagoon, recovery);
        assert!(modern.contains("Phase PREPARE"));
        assert!(modern.contains("processed 768.0 KiB"));
        assert!(modern.contains("128.0 KiB/s"));
    }

    #[test]
    fn classic_scan_progress_reports_entries_without_byte_metrics() {
        let line = render_scan_progress_line(
            OutputStyleArg::Classic,
            true,
            AccentArg::Teal,
            "update",
            42,
            "folder/item.txt",
            0,
        );

        assert!(line.starts_with("[............................]"));
        assert!(line.contains("SCAN #42"));
        assert!(line.contains("folder/item.txt"));
        assert!(!line.contains(" B"));
        assert!(!line.contains("iB"));
        assert!(!line.contains("/s"));
        assert!(!line.contains('%'));
        assert!(!line.contains("\x1b["));
    }

    #[test]
    fn modern_scan_progress_reports_entries_without_byte_metrics() {
        let line = render_scan_progress_line(
            OutputStyleArg::Modern,
            false,
            AccentArg::Lagoon,
            "update",
            42,
            "folder/item.txt",
            3,
        );

        assert!(line.contains("UPDATE · SCAN"));
        assert!(line.contains("SCAN #42"));
        assert!(line.contains("folder/item.txt"));
        assert!(line.contains('⠸'));
        assert!(line.contains('◆'));
        assert!(!line.contains(" B"));
        assert!(!line.contains("iB"));
        assert!(!line.contains("/s"));
        assert!(!line.contains('%'));
        assert!(!line.contains("\x1b["));
        assert_eq!(line.lines().count(), 4);
    }

    #[test]
    fn verbose_scan_progress_keeps_path_only_behavior() {
        let progress = progress_for_test(Mode::Verbose);
        let current = EntryPath::from_utf8("folder/item.txt");

        progress.on_scan_progress(42, &current);

        let state = lock_unpoisoned(&progress.state);
        assert_eq!(state.last_entry, "folder/item.txt");
        assert!(!state.scanning);
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
