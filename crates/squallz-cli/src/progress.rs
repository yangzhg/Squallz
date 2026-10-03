//! stderr progress shared by CLI commands: bounded TTY redraws, verbose paths,
//! or silent output for JSON, quiet mode and redirected stderr.

use std::io::{self, IsTerminal, Write};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use squallz_core::api::{EntryPath, ProgressPhase, ProgressSink};
use squallz_core::lock_unpoisoned;
use squallz_i18n::Localizer;
use terminal_size::{terminal_size_of, Height, Width};

use crate::args::{AccentArg, OutputStyleArg};
use crate::commands::Ctx;
use crate::ui::{self, Tone};

const REDRAW_INTERVAL: Duration = Duration::from_millis(100);

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
    frame: usize,
    scanning: bool,
    phase: Option<ProgressPhase>,
    interruptible: bool,
}

impl Default for State {
    fn default() -> Self {
        Self {
            start: Instant::now(),
            last_draw: None,
            last_entry: String::new(),
            drawn: false,
            frame: 0,
            scanning: false,
            phase: None,
            interruptible: true,
        }
    }
}

/// stderr progress sink shared by every command.
pub struct CliProgress {
    mode: Mode,
    loc: Arc<Localizer>,
    state: Mutex<State>,
}

impl CliProgress {
    pub fn new_for_operation(ctx: &Ctx, json: bool, operation: impl Into<String>) -> Self {
        let mode = if json || ctx.quiet {
            Mode::Silent
        } else if ctx.verbose {
            Mode::Verbose
        } else if io::stderr().is_terminal() {
            Mode::Bar {
                style: ctx.output_style,
                color: ctx.output_style.is_modern() && ctx.color.enabled(true),
                accent: ctx.accent,
                operation: operation.into(),
            }
        } else {
            Mode::Silent
        };
        Self {
            mode,
            loc: Arc::clone(&ctx.loc),
            state: Mutex::new(State::default()),
        }
    }

    /// Remove live progress before results or a prompt use the terminal.
    pub fn finish(&self) {
        let mut state = lock_unpoisoned(&self.state);
        if state.drawn {
            let _ = clear_progress_block(&mut io::stderr().lock());
            state.drawn = false;
        }
    }

    fn display(&self) -> Option<ProgressDisplay<'_>> {
        match &self.mode {
            Mode::Bar {
                style,
                color,
                accent,
                operation,
            } => Some(ProgressDisplay {
                style: *style,
                color: *color,
                accent: *accent,
                operation,
                loc: &self.loc,
                size: terminal_size_of(io::stderr())
                    .map(|(Width(columns), Height(rows))| (columns, rows))
                    .unwrap_or((80, 24)),
            }),
            _ => None,
        }
    }

    fn draw_bar(&self, done: u64, total: u64, current: &EntryPath) {
        let Some(display) = self.display() else {
            return;
        };
        let mut state = lock_unpoisoned(&self.state);
        if state.scanning {
            state.start = Instant::now();
            state.last_draw = None;
            state.scanning = false;
        }
        let finished = total > 0 && done >= total;
        if state.last_draw.is_some_and(|last| {
            !finished
                && state.phase != Some(ProgressPhase::ArchiveOpen)
                && last.elapsed() < REDRAW_INTERVAL
        }) {
            return;
        }
        let elapsed = state.start.elapsed();
        let speed = if elapsed.as_secs_f64() > 0.05 {
            (done as f64 / elapsed.as_secs_f64()) as u64
        } else {
            0
        };
        let block = display.progress(ProgressFrame {
            done,
            total,
            current: &current.display,
            speed,
            elapsed_secs: elapsed.as_secs(),
            frame: state.frame,
            phase: state.phase,
            interruptible: state.interruptible,
        });
        Self::draw(&mut state, &block);
    }

    fn draw_scan(&self, entries: u64, current: &EntryPath) {
        let Some(display) = self.display() else {
            return;
        };
        let mut state = lock_unpoisoned(&self.state);
        state.scanning = true;
        state.phase = None;
        state.interruptible = true;
        if state
            .last_draw
            .is_some_and(|last| last.elapsed() < REDRAW_INTERVAL)
        {
            return;
        }
        let block = display.scan(entries, &current.display, state.frame);
        Self::draw(&mut state, &block);
    }

    fn draw(state: &mut State, block: &str) {
        let _ = write_progress_block(&mut io::stderr().lock(), block, state.drawn);
        state.drawn = true;
        state.last_draw = Some(Instant::now());
        state.frame = state.frame.wrapping_add(1);
    }

    fn print_verbose(&self, current: &EntryPath) {
        if current.display.is_empty() {
            return;
        }
        let mut state = lock_unpoisoned(&self.state);
        if state.last_entry != current.display {
            state.last_entry = current.display.clone();
            eprintln!("{}", ui::truncate_end(&current.display, usize::MAX));
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
            Mode::Verbose => eprintln!("-- {} --", phase_label(&self.loc, phase)),
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
    done: u64,
    total: u64,
    current: &'a str,
    speed: u64,
    elapsed_secs: u64,
    frame: usize,
    phase: Option<ProgressPhase>,
    interruptible: bool,
}

struct ProgressDisplay<'a> {
    style: OutputStyleArg,
    color: bool,
    accent: AccentArg,
    operation: &'a str,
    loc: &'a Localizer,
    size: (u16, u16),
}

impl ProgressDisplay<'_> {
    fn progress(&self, frame: ProgressFrame<'_>) -> String {
        let recovery = is_recovery_progress_phase(frame.phase);
        let pending = matches!(
            frame.phase,
            Some(ProgressPhase::ArchiveOpen | ProgressPhase::ExtractMetadata)
        ) || (frame.phase.is_some() && !frame.interruptible && !recovery)
            || (recovery
                && frame.total == 0
                && (frame.phase != Some(ProgressPhase::RecoveryPrepare) || frame.done == 0));
        let phase = frame
            .phase
            .map(|phase| phase_label(self.loc, phase))
            .unwrap_or_else(|| self.loc.t("gui.task.state.running"));
        let progress = if pending {
            String::new()
        } else if recovery && frame.total > 0 {
            format!("{}%", percent(frame.done, frame.total))
        } else {
            let done = fmt_bytes(frame.done);
            let total = fmt_bytes(frame.total);
            let speed = format!("{}/s", fmt_bytes(frame.speed));
            if frame.total > 0 {
                self.loc.format(
                    "gui.task.progress_known",
                    &[
                        ("percent", &percent(frame.done, frame.total).to_string()),
                        ("done", &done),
                        ("total", &total),
                        ("speed", &speed),
                    ],
                )
            } else {
                self.loc.format(
                    "gui.task.progress_unknown",
                    &[("done", &done), ("speed", &speed)],
                )
            }
        };
        let eta = if !pending
            && !recovery
            && frame.total > 0
            && frame.speed > 0
            && frame.done < frame.total
        {
            fmt_duration(frame.total.saturating_sub(frame.done).div_ceil(frame.speed))
        } else {
            "--".to_owned()
        };
        let timing = self.loc.format(
            "cli.progress.timing",
            &[
                ("elapsed", &fmt_duration(frame.elapsed_secs)),
                ("eta", &eta),
            ],
        );
        let heading = format!("{} · {phase}", self.operation);
        self.render(
            &heading,
            &progress,
            Some(&timing),
            frame.current,
            frame.frame,
        )
    }

    fn scan(&self, entries: u64, current: &str, frame: usize) -> String {
        let heading = format!("{} · {}", self.operation, self.loc.t("gui.task.scan_badge"));
        let metrics = self
            .loc
            .format("gui.task.progress_scan", &[("count", &entries.to_string())]);
        self.render(&heading, &metrics, None, current, frame)
    }

    fn render(
        &self,
        heading: &str,
        progress: &str,
        timing: Option<&str>,
        current: &str,
        frame: usize,
    ) -> String {
        // Leave one column unused so writing the final cell cannot cause wrapping.
        let width = usize::from(self.size.0).saturating_sub(1);
        let height = usize::from(self.size.1).saturating_sub(1).clamp(1, 3);
        let compact = if progress.is_empty() {
            heading.to_owned()
        } else {
            format!("{progress} · {heading}")
        };
        let plain = if !self.style.is_modern() || height == 1 {
            let current = ui::truncate_end(current, width / 3);
            let line = if current.is_empty() {
                compact
            } else {
                format!(
                    "{} · {current}",
                    ui::truncate_end(
                        &compact,
                        width.saturating_sub(ui::display_width(&current) + 3)
                    )
                )
            };
            vec![(Tone::Primary, line)]
        } else if height == 2 {
            vec![
                (Tone::Primary, compact),
                (Tone::Secondary, current.to_owned()),
            ]
        } else {
            let pulse = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧"][frame % 8];
            let metrics = [Some(progress).filter(|value| !value.is_empty()), timing]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(" · ");
            vec![
                (Tone::Primary, format!("{pulse} {heading}")),
                (Tone::Secondary, metrics),
                (Tone::Primary, current.to_owned()),
            ]
        };
        plain
            .into_iter()
            .filter(|(_, line)| !line.is_empty())
            .map(|(tone, line)| {
                ui::paint_tone(
                    self.style.is_modern() && self.color,
                    self.accent,
                    tone,
                    &ui::truncate_end(&line, width),
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn phase_label(loc: &Localizer, phase: ProgressPhase) -> String {
    let key = match phase {
        ProgressPhase::ArchiveOpen => "archive_open",
        ProgressPhase::ArchiveTest => "archive_test",
        ProgressPhase::ArchiveConvert => "archive_convert",
        ProgressPhase::ExtractEntries => "extract_entries",
        ProgressPhase::ExtractMetadata => "extract_metadata",
        ProgressPhase::RecoveryPrepare => "recovery_prepare",
        ProgressPhase::RecoveryVerify => "recovery_verify",
        ProgressPhase::RecoveryProcess => "recovery_process",
        ProgressPhase::RecoveryFinalize => "recovery_finalize",
        ProgressPhase::OutputSplit => "output_split",
        ProgressPhase::OutputRecovery => "output_recovery",
        ProgressPhase::OutputVerify => "output_verify",
        ProgressPhase::OutputCommit => "output_commit",
        ProgressPhase::OutputCleanup => "output_cleanup",
        ProgressPhase::UpdateRecovery => "update_recovery",
        ProgressPhase::UpdateRewrite => "update_rewrite",
        ProgressPhase::UpdateVerify => "update_verify",
        ProgressPhase::UpdateCommit => "update_commit",
        ProgressPhase::UpdateCleanup => "update_cleanup",
        ProgressPhase::SfxPublishVerify => "sfx_publish_verify",
        ProgressPhase::SfxPublishSign => "sfx_publish_sign",
        ProgressPhase::SfxPublishNotarize => "sfx_publish_notarize",
        ProgressPhase::SfxPublishFinalize => "sfx_publish_finalize",
        _ => return loc.t("gui.task.state.running"),
    };
    loc.t(&format!("gui.task.phase.{key}"))
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

fn write_progress_block(
    output: &mut impl Write,
    block: &str,
    had_previous: bool,
) -> io::Result<()> {
    if had_previous {
        output.write_all(b"\r\x1b[0J")?;
    }
    let mut lines = 0;
    for line in block.lines() {
        if lines > 0 {
            output.write_all(b"\n\r")?;
        }
        output.write_all(line.as_bytes())?;
        lines += 1;
    }
    // Keep the cursor at the first line, including between redraws and on resize.
    // Erasing from this anchor also clears old rows after terminal reflow.
    output.write_all(b"\r")?;
    if lines > 1 {
        write!(output, "\x1b[{}F", lines - 1)?;
    }
    output.flush()
}

fn clear_progress_block(output: &mut impl Write) -> io::Result<()> {
    output.write_all(b"\r\x1b[0J")?;
    output.flush()
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
    format!("{}h{:02}m", minutes / 60, minutes % 60)
}

fn percent(done: u64, total: u64) -> usize {
    if total == 0 {
        return 0;
    }
    ((done.min(total) as u128 * 100) / total as u128) as usize
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

    fn display(loc: &Localizer, style: OutputStyleArg) -> ProgressDisplay<'_> {
        ProgressDisplay {
            style,
            color: false,
            accent: AccentArg::Ocean,
            operation: "test",
            loc,
            size: (120, 24),
        }
    }

    fn frame(phase: Option<ProgressPhase>) -> ProgressFrame<'static> {
        ProgressFrame {
            done: 512 * 1024,
            total: 1024 * 1024,
            current: "reports/年度报告.txt",
            speed: 256 * 1024,
            elapsed_secs: 2,
            frame: 0,
            phase,
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

    #[test]
    fn cli_progress_recovers_after_state_lock_poison() {
        let progress = CliProgress {
            mode: Mode::Verbose,
            loc: Arc::new(Localizer::with_user_dir(Some("en-US"), None)),
            state: Mutex::new(State::default()),
        };
        let poison = std::panic::catch_unwind(|| {
            let mut state = progress.state.lock().unwrap();
            state.last_entry = "before.txt".to_owned();
            panic!("poison progress state");
        });
        assert!(poison.is_err());
        progress.print_verbose(&EntryPath::from_utf8("after.txt"));
        assert_eq!(lock_unpoisoned(&progress.state).last_entry, "after.txt");
    }

    #[test]
    fn progress_preserves_measured_metrics_and_unknown_totals() {
        let loc = Localizer::with_user_dir(Some("en-US"), None);
        for style in [OutputStyleArg::Classic, OutputStyleArg::Modern] {
            let display = display(&loc, style);
            let known = display.progress(frame(Some(ProgressPhase::ExtractEntries)));
            assert!(known.contains("50%"));
            assert!(known.contains("512.0 KiB / 1.0 MiB"));
            assert!(known.contains("256.0 KiB/s"));
            if style.is_modern() {
                assert!(known.contains("ETA 2s"));
            }
            let unknown = display.progress(ProgressFrame {
                total: 0,
                ..frame(Some(ProgressPhase::ExtractEntries))
            });
            assert!(unknown.contains("512.0 KiB processed"));
            assert!(unknown.contains("256.0 KiB/s"));
            assert!(!unknown.contains('%'));
            assert!(!unknown.contains("ETA 2s"));
        }
    }

    #[test]
    fn pending_phases_hide_stale_transfer_metrics() {
        let loc = Localizer::with_user_dir(Some("en-US"), None);
        for phase in [
            ProgressPhase::ArchiveOpen,
            ProgressPhase::ExtractMetadata,
            ProgressPhase::UpdateCommit,
            ProgressPhase::OutputCommit,
        ] {
            for style in [OutputStyleArg::Classic, OutputStyleArg::Modern] {
                let display = display(&loc, style);
                let line = display.progress(ProgressFrame {
                    interruptible: false,
                    done: 1024 * 1024,
                    ..frame(Some(phase))
                });
                assert!(line.contains(&phase_label(&loc, phase)));
                assert!(!line.contains('%'));
                assert!(!line.contains("KiB"));
                assert!(!line.contains("MiB"));
                assert!(!line.contains("/s"));
                assert!(!line.contains("Complete"));
            }
        }
        let line = display(&loc, OutputStyleArg::Modern).progress(ProgressFrame {
            done: 1024 * 1024,
            ..frame(Some(ProgressPhase::UpdateVerify))
        });
        assert!(line.contains("100%"));
        assert!(!line.contains("Complete"));
        let resumed = display(&loc, OutputStyleArg::Modern)
            .progress(frame(Some(ProgressPhase::ExtractEntries)));
        assert!(resumed.contains("50%"));
        assert!(resumed.contains("256.0 KiB/s"));
    }

    #[test]
    fn recovery_percentages_are_not_bytes_but_preparation_can_stream() {
        let loc = Localizer::with_user_dir(Some("en-US"), None);
        for style in [OutputStyleArg::Classic, OutputStyleArg::Modern] {
            let display = display(&loc, style);
            let line = display.progress(ProgressFrame {
                done: 380,
                total: 1000,
                interruptible: false,
                ..frame(Some(ProgressPhase::RecoveryProcess))
            });
            assert!(line.contains("38%"));
            assert!(!line.contains("380 B"));
            assert!(!line.contains("1000 B"));
            assert!(!line.contains("/s"));
            let line = display.progress(ProgressFrame {
                total: 0,
                ..frame(Some(ProgressPhase::RecoveryPrepare))
            });
            assert!(line.contains("512.0 KiB"));
            assert!(line.contains("256.0 KiB/s"));
        }
    }

    #[test]
    fn progress_fits_terminal_cells_and_sanitizes_paths_before_color() {
        let loc = Localizer::with_user_dir(Some("zh-CN"), None);
        let long_path = "年度报告/".repeat(25) + "\x1b[2J\nreport.txt";
        for columns in [1, 20, 40, 80, 120] {
            for rows in [1, 3, 24] {
                for style in [OutputStyleArg::Classic, OutputStyleArg::Modern] {
                    let mut display = display(&loc, style);
                    display.size = (columns, rows);
                    for block in [
                        display.progress(ProgressFrame {
                            current: &long_path,
                            ..frame(Some(ProgressPhase::ArchiveTest))
                        }),
                        display.scan(42, &long_path, 0),
                    ] {
                        assert!(
                            block.lines().count()
                                <= usize::from(rows).saturating_sub(1).clamp(1, 3)
                        );
                        for line in block.lines() {
                            assert!(ui::display_width(line) < usize::from(columns));
                            assert!(!line.contains('\x1b'));
                        }
                    }
                    display.color = true;
                    let colored = display.progress(ProgressFrame {
                        current: &long_path,
                        ..frame(Some(ProgressPhase::ArchiveTest))
                    });
                    assert_eq!(colored.contains("\x1b["), style.is_modern());
                    assert!(!colored.contains("\x1b[2J"));
                }
            }
        }
        let scan = display(&loc, OutputStyleArg::Modern).scan(42, "资料.txt", 0);
        assert!(scan.contains("42"));
        assert!(scan.contains("资料.txt"));
        assert!(!scan.contains('%'));
        assert!(!scan.contains("/s"));
        assert!(!scan.contains(" B"));
        for style in [OutputStyleArg::Classic, OutputStyleArg::Modern] {
            let mut compact = display(&loc, style);
            compact.size = (80, 1);
            let line = compact.progress(frame(Some(ProgressPhase::ArchiveTest)));
            assert!(line.contains("512.0 KiB"));
            assert!(line.contains("reports/年度报告.txt"));
        }
    }

    #[test]
    fn redraw_and_finish_keep_the_cursor_at_the_start_of_owned_output() {
        let mut output = Vec::new();
        write_progress_block(&mut output, "status\nmetrics\nfile", false).unwrap();
        assert!(output.ends_with(b"\r\x1b[2F"));
        let offset = output.len();
        write_progress_block(&mut output, "short", true).unwrap();
        assert_eq!(&output[offset..], b"\r\x1b[0Jshort\r");
        let offset = output.len();
        clear_progress_block(&mut output).unwrap();
        assert_eq!(&output[offset..], b"\r\x1b[0J");
    }
}
