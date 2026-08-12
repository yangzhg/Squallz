use std::io::{self, BufRead, IsTerminal, Write};
use std::path::Path;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

use squallz_core::api::{
    ConflictDecision, ConflictResolver, ControlToken, EntryMeta, FormatError, Password,
};
use squallz_i18n::Localizer;

use crate::output::safe_terminal_text;

const PASSWORD_PROMPTS: usize = 3;
const CANCEL_POLL_INTERVAL: Duration = Duration::from_millis(50);

pub(crate) fn stdin_is_tty() -> bool {
    io::stdin().is_terminal()
}

pub(crate) fn with_password_retry<T>(
    loc: &Localizer,
    ctl: &ControlToken,
    before_prompt: impl Fn(),
    mut operation: impl FnMut(Option<&Password>) -> Result<T, FormatError>,
) -> Result<T, FormatError> {
    let mut last_error = match operation(None) {
        Err(error @ (FormatError::PasswordRequired | FormatError::WrongPassword))
            if stdin_is_tty() =>
        {
            error
        }
        result => return result,
    };

    for attempt in 0..PASSWORD_PROMPTS {
        ctl.checkpoint()?;
        before_prompt();
        if attempt > 0 {
            let remaining = (PASSWORD_PROMPTS - attempt).to_string();
            write_stderr(&format!(
                "{}\n",
                loc.format("cli.password.retry", &[("remaining", &remaining)])
            ));
        }
        // Keep password input synchronous: rpassword's terminal echo guard
        // must be dropped before the process can observe cancellation.
        let password = match rpassword::prompt_password(loc.t("cli.password.prompt")) {
            Ok(password) => Password::new(password),
            Err(error) if ctl.is_cancelled() || error.kind() == io::ErrorKind::Interrupted => {
                return Err(FormatError::Cancelled)
            }
            Err(error) => return Err(error.into()),
        };
        ctl.checkpoint()?;
        match operation(Some(&password)) {
            Err(error @ (FormatError::PasswordRequired | FormatError::WrongPassword)) => {
                last_error = error;
            }
            result => return result,
        }
    }
    Err(last_error)
}

#[derive(Clone, Copy)]
enum RememberedDecision {
    Overwrite,
    Skip,
    Rename,
}

pub(crate) struct RuntimeConflictResolver {
    loc: Arc<Localizer>,
    ctl: ControlToken,
    remembered: Mutex<Option<RememberedDecision>>,
}

impl RuntimeConflictResolver {
    pub(crate) fn new(loc: Arc<Localizer>, ctl: ControlToken) -> Self {
        Self {
            loc,
            ctl,
            remembered: Mutex::new(None),
        }
    }

    fn read_line(&self) -> PromptLine {
        let (sender, receiver) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("squallz-sfx-conflict".to_owned())
            .spawn(move || {
                let mut line = String::new();
                let result = match io::stdin().lock().read_line(&mut line) {
                    Ok(0) | Err(_) => None,
                    Ok(_) => Some(line.trim().to_owned()),
                };
                let _ = sender.send(result);
            });
        if spawned.is_err() {
            return PromptLine::Closed;
        }
        loop {
            if self.ctl.is_cancelled() {
                return PromptLine::Cancelled;
            }
            match receiver.recv_timeout(CANCEL_POLL_INTERVAL) {
                Ok(Some(line)) => return PromptLine::Value(line),
                Ok(None) | Err(RecvTimeoutError::Disconnected) => return PromptLine::Closed,
                Err(RecvTimeoutError::Timeout) => {}
            }
        }
    }
}

enum PromptLine {
    Value(String),
    Closed,
    Cancelled,
}

impl ConflictResolver for RuntimeConflictResolver {
    fn resolve(&self, existing: &Path, _incoming: &EntryMeta) -> ConflictDecision {
        if let Some(decision) = *lock_unpoisoned(&self.remembered) {
            return remembered_decision(existing, decision);
        }

        loop {
            let path = safe_terminal_text(&existing.display().to_string());
            write_stderr(&self.loc.format("cli.conflict.prompt", &[("path", &path)]));
            let answer = match self.read_line() {
                PromptLine::Value(answer) => answer,
                PromptLine::Closed => return ConflictDecision::Skip,
                PromptLine::Cancelled => return ConflictDecision::Abort,
            };
            match answer.as_str() {
                "o" => return ConflictDecision::Overwrite,
                "O" => {
                    *lock_unpoisoned(&self.remembered) = Some(RememberedDecision::Overwrite);
                    return ConflictDecision::Overwrite;
                }
                "s" => return ConflictDecision::Skip,
                "S" => {
                    *lock_unpoisoned(&self.remembered) = Some(RememberedDecision::Skip);
                    return ConflictDecision::Skip;
                }
                "r" => {
                    write_stderr(&self.loc.t("cli.conflict.rename_prompt"));
                    let name = match self.read_line() {
                        PromptLine::Value(name) => name,
                        PromptLine::Closed => String::new(),
                        PromptLine::Cancelled => return ConflictDecision::Abort,
                    };
                    return ConflictDecision::Rename(if name.is_empty() {
                        auto_renamed_name(existing)
                    } else {
                        name
                    });
                }
                "R" => {
                    *lock_unpoisoned(&self.remembered) = Some(RememberedDecision::Rename);
                    return ConflictDecision::Rename(auto_renamed_name(existing));
                }
                "a" | "A" => return ConflictDecision::Abort,
                _ => write_stderr(&format!("{}\n", self.loc.t("cli.conflict.invalid_input"))),
            }
        }
    }
}

fn remembered_decision(existing: &Path, decision: RememberedDecision) -> ConflictDecision {
    match decision {
        RememberedDecision::Overwrite => ConflictDecision::Overwrite,
        RememberedDecision::Skip => ConflictDecision::Skip,
        RememberedDecision::Rename => ConflictDecision::Rename(auto_renamed_name(existing)),
    }
}

fn auto_renamed_name(existing: &Path) -> String {
    let stem = existing
        .file_stem()
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_default();
    let extension = existing
        .extension()
        .map(|value| value.to_string_lossy().into_owned());
    let parent = existing.parent().unwrap_or_else(|| Path::new(""));
    for number in 1u32..=u32::MAX {
        let name = rename_candidate(&stem, extension.as_deref(), number);
        if std::fs::symlink_metadata(parent.join(&name)).is_err() {
            return name;
        }
    }
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    match extension {
        Some(extension) => format!("{stem} ({stamp}).{extension}"),
        None => format!("{stem} ({stamp})"),
    }
}

fn rename_candidate(stem: &str, extension: Option<&str>, number: u32) -> String {
    match extension {
        Some(extension) => format!("{stem} ({number}).{extension}"),
        None => format!("{stem} ({number})"),
    }
}

fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn write_stderr(text: &str) {
    let stderr = io::stderr();
    let mut output = stderr.lock();
    let _ = output.write_all(text.as_bytes());
    let _ = output.flush();
}
