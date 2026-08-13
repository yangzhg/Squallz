use std::io::{self, IsTerminal, Write};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use squallz_core::api::{EntryPath, ProgressSink};
use squallz_core::lock_unpoisoned;
use squallz_i18n::Localizer;

use crate::output::safe_terminal_text;

const UPDATE_INTERVAL: Duration = Duration::from_millis(100);

struct State {
    last_draw: Option<Instant>,
    last_path: String,
    line_visible: bool,
}

pub(crate) struct RuntimeProgress {
    loc: Arc<Localizer>,
    interactive: bool,
    verbose: bool,
    state: Mutex<State>,
}

impl RuntimeProgress {
    pub(crate) fn new(loc: Arc<Localizer>, quiet: bool, verbose: bool, json: bool) -> Self {
        Self {
            loc,
            interactive: !quiet && !json && io::stderr().is_terminal(),
            verbose: verbose && !quiet && !json,
            state: Mutex::new(State {
                last_draw: None,
                last_path: String::new(),
                line_visible: false,
            }),
        }
    }

    pub(crate) fn finish(&self) {
        if !self.interactive {
            return;
        }
        let mut state = lock_unpoisoned(&self.state);
        if state.line_visible {
            write_stderr("\r\x1b[2K");
            state.line_visible = false;
        }
    }

    fn render(&self, done: u64, total: u64, current: &EntryPath) {
        if !self.interactive && !self.verbose {
            return;
        }
        let path = safe_terminal_text(&current.display);
        let now = Instant::now();
        let mut state = lock_unpoisoned(&self.state);
        let changed = path != state.last_path;
        let complete = total > 0 && done >= total;
        let due = state
            .last_draw
            .is_none_or(|last| now.duration_since(last) >= UPDATE_INTERVAL);
        if !changed && !complete && !due {
            return;
        }
        if self.verbose && changed && !path.is_empty() {
            if self.interactive && state.line_visible {
                write_stderr("\r\x1b[2K");
            }
            write_stderr(&format!("{path}\n"));
            state.line_visible = false;
        }
        state.last_path = path.clone();
        if !self.interactive {
            return;
        }
        let done_text = format_bytes(done);
        let line = if total > 0 {
            let percent = done
                .saturating_mul(100)
                .checked_div(total)
                .unwrap_or(0)
                .min(100);
            let total_text = format_bytes(total);
            self.loc.format(
                "cli.sfx.runtime.progress",
                &[
                    ("percent", &percent.to_string()),
                    ("done", &done_text),
                    ("total", &total_text),
                    ("path", &path),
                ],
            )
        } else {
            self.loc.format(
                "cli.sfx.runtime.progress_unknown",
                &[("done", &done_text), ("path", &path)],
            )
        };
        write_stderr(&format!("\r\x1b[2K{line}"));
        state.last_draw = Some(now);
        state.line_visible = true;
    }
}

impl ProgressSink for RuntimeProgress {
    fn on_progress(&self, done: u64, total: u64, current: &EntryPath) {
        self.render(done, total, current);
    }

    fn on_entry_progress(
        &self,
        done: u64,
        total: u64,
        current: &EntryPath,
        _current_done: u64,
        _current_total: u64,
    ) {
        self.render(done, total, current);
    }

    fn on_scan_progress(&self, entries: u64, current: &EntryPath) {
        if !self.interactive && !self.verbose {
            return;
        }
        let path = safe_terminal_text(&current.display);
        let mut state = lock_unpoisoned(&self.state);
        if self.verbose && path != state.last_path && !path.is_empty() {
            if self.interactive && state.line_visible {
                write_stderr("\r\x1b[2K");
            }
            write_stderr(&format!("{path}\n"));
            state.line_visible = false;
        }
        state.last_path = path.clone();
        if !self.interactive {
            return;
        }
        let line = self.loc.format(
            "cli.sfx.runtime.scanning",
            &[("count", &entries.to_string()), ("path", &path)],
        );
        write_stderr(&format!("\r\x1b[2K{line}"));
        state.line_visible = true;
        state.last_draw = Some(Instant::now());
    }
}

fn write_stderr(text: &str) {
    let stderr = io::stderr();
    let mut output = stderr.lock();
    let _ = output.write_all(text.as_bytes());
    let _ = output.flush();
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0usize;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}
