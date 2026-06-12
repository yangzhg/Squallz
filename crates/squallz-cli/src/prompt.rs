//! Interactive prompts: password input with retry and the overwrite
//! conflict resolver (`--overwrite ask`). All copy goes through the
//! language packs.

use std::io::{BufRead, IsTerminal, Write};
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use squallz_core::api::{ConflictDecision, ConflictResolver, EntryMeta, FormatError, Password};
use squallz_i18n::Localizer;

/// Prompted attempts after a missing/wrong password: 1 initial + 2 retries.
const PASSWORD_RETRIES: usize = 2;

/// Whether stdin is connected to a terminal (interactive session).
pub fn stdin_is_tty() -> bool {
    std::io::stdin().is_terminal()
}

/// Runs `op`, prompting for a password when it fails with
/// `PasswordRequired`/`WrongPassword`:
/// - an explicitly supplied `--password` is never retried (scripts must fail
///   fast with exit code 4);
/// - in a TTY the user is prompted up to 1 + [`PASSWORD_RETRIES`] times;
/// - without a TTY the error is returned directly (exit code 4).
pub fn with_password_retry<T>(
    loc: &Localizer,
    explicit: Option<&Password>,
    mut op: impl FnMut(Option<&Password>) -> Result<T, FormatError>,
) -> Result<T, FormatError> {
    let first = op(explicit);
    let mut last_err = match first {
        Err(e @ (FormatError::PasswordRequired | FormatError::WrongPassword))
            if explicit.is_none() && stdin_is_tty() =>
        {
            e
        }
        other => return other,
    };
    for attempt in 0..=PASSWORD_RETRIES {
        if attempt > 0 {
            let remaining = (PASSWORD_RETRIES - attempt + 1).to_string();
            eprintln!(
                "{}",
                loc.format("cli.password.retry", &[("remaining", &remaining)])
            );
        }
        let pw = rpassword::prompt_password(loc.t("cli.password.prompt"))
            .map(Password::new)
            .map_err(FormatError::Io)?;
        match op(Some(&pw)) {
            Err(e @ (FormatError::PasswordRequired | FormatError::WrongPassword)) => last_err = e,
            other => return other,
        }
    }
    Err(last_err)
}

fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// "Apply to all" memory of the interactive resolver.
#[derive(Clone, Copy)]
enum AllDecision {
    Overwrite,
    Skip,
    RenameAuto,
}

fn remembered_conflict_decision(existing: &Path, decision: AllDecision) -> ConflictDecision {
    match decision {
        AllDecision::Overwrite => ConflictDecision::Overwrite,
        AllDecision::Skip => ConflictDecision::Skip,
        AllDecision::RenameAuto => ConflictDecision::Rename(auto_renamed_name(existing)),
    }
}

/// Interactive conflict resolver for `--overwrite ask` in a TTY: per file,
/// o/s/r/a = overwrite / skip / rename / abort; an uppercase letter applies
/// the decision to all remaining conflicts.
pub struct CliConflictResolver {
    loc: Arc<Localizer>,
    all: Mutex<Option<AllDecision>>,
}

impl CliConflictResolver {
    /// Creates the resolver.
    pub fn new(loc: Arc<Localizer>) -> Self {
        Self {
            loc,
            all: Mutex::new(None),
        }
    }

    fn read_line(&self) -> Option<String> {
        let mut line = String::new();
        match std::io::stdin().lock().read_line(&mut line) {
            Ok(0) | Err(_) => None, // EOF / read failure: behave like "skip"
            Ok(_) => Some(line.trim().to_owned()),
        }
    }

    fn read_line_or_empty(&self) -> String {
        let Some(line) = self.read_line() else {
            return String::new();
        };
        line
    }
}

impl ConflictResolver for CliConflictResolver {
    fn resolve(&self, existing: &Path, _incoming: &EntryMeta) -> ConflictDecision {
        if let Some(decision) = *lock_unpoisoned(&self.all) {
            return remembered_conflict_decision(existing, decision);
        }
        loop {
            let path = existing.display().to_string();
            eprint!(
                "{}",
                self.loc.format("cli.conflict.prompt", &[("path", &path)])
            );
            let _ = std::io::stderr().flush();
            let Some(answer) = self.read_line() else {
                return ConflictDecision::Skip;
            };
            match answer.as_str() {
                "o" => return ConflictDecision::Overwrite,
                "O" => {
                    *lock_unpoisoned(&self.all) = Some(AllDecision::Overwrite);
                    return ConflictDecision::Overwrite;
                }
                "s" => return ConflictDecision::Skip,
                "S" => {
                    *lock_unpoisoned(&self.all) = Some(AllDecision::Skip);
                    return ConflictDecision::Skip;
                }
                "r" => {
                    eprint!("{}", self.loc.t("cli.conflict.rename_prompt"));
                    let _ = std::io::stderr().flush();
                    let name = self.read_line_or_empty();
                    let name = if name.is_empty() {
                        auto_renamed_name(existing)
                    } else {
                        name
                    };
                    return ConflictDecision::Rename(name);
                }
                "R" => {
                    *lock_unpoisoned(&self.all) = Some(AllDecision::RenameAuto);
                    return ConflictDecision::Rename(auto_renamed_name(existing));
                }
                "a" | "A" => return ConflictDecision::Abort,
                _ => eprintln!("{}", self.loc.t("cli.conflict.invalid_input")),
            }
        }
    }
}

/// Picks the first free `name (n).ext` sibling file name (mirrors the
/// RenameBoth policy of the extraction engine).
fn auto_renamed_name(existing: &Path) -> String {
    let stem = file_stem_or_empty(existing);
    let ext = existing
        .extension()
        .map(|e| e.to_string_lossy().into_owned());
    let parent = parent_or_empty(existing);
    for n in 1u32..=u32::MAX {
        let name = match &ext {
            Some(ext) => format!("{stem} ({n}).{ext}"),
            None => format!("{stem} ({n})"),
        };
        if std::fs::symlink_metadata(parent.join(&name)).is_err() {
            return name;
        }
    }
    exhausted_auto_rename_fallback(&stem, ext.as_deref())
}

fn file_stem_or_empty(existing: &Path) -> String {
    let Some(stem) = existing.file_stem() else {
        return String::new();
    };
    stem.to_string_lossy().into_owned()
}

fn parent_or_empty(existing: &Path) -> &Path {
    if let Some(parent) = existing.parent() {
        parent
    } else {
        Path::new("")
    }
}

fn exhausted_auto_rename_fallback(stem: &str, ext: Option<&str>) -> String {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    match ext {
        Some(ext) => format!("{stem} ({stamp}).{ext}"),
        None => format!("{stem} ({stamp})"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::panic::{catch_unwind, AssertUnwindSafe};

    use squallz_core::api::{EntryPath, EntryType};

    fn localizer() -> Arc<Localizer> {
        Arc::new(Localizer::with_user_dir(Some("en-US"), None))
    }

    fn incoming_meta() -> EntryMeta {
        EntryMeta {
            path: EntryPath::from_utf8("incoming.txt"),
            entry_type: EntryType::File,
            size: 0,
            compressed_size: None,
            modified: None,
            unix_mode: None,
            crc32: None,
            encrypted: false,
        }
    }

    #[test]
    fn conflict_resolver_recovers_after_apply_all_lock_poison() {
        let resolver = CliConflictResolver::new(localizer());
        let poisoned = catch_unwind(AssertUnwindSafe(|| {
            let mut all = resolver.all.lock().expect("poison test lock");
            *all = Some(AllDecision::Skip);
            panic!("poison prompt apply-all lock");
        }));
        assert!(poisoned.is_err());

        let decision = resolver.resolve(Path::new("existing.txt"), &incoming_meta());
        assert!(matches!(decision, ConflictDecision::Skip));
    }

    #[test]
    fn auto_rename_uses_first_free_sibling_name() {
        let name = auto_renamed_name(Path::new("sqz-prompt-redline-unique.txt"));
        assert_eq!(name, "sqz-prompt-redline-unique (1).txt");
    }
}
