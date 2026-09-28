use std::collections::HashMap;

use encoding_rs::Encoding;
use squallz_format_api::{ControlToken, EntryPath, EntrySelection, FormatError};

struct Candidate {
    path: EntryPath,
    directory: bool,
    ambiguous: bool,
}

pub(super) struct SelectedName {
    pub(super) path: EntryPath,
    pub(super) directory: bool,
}

/// Maps both listed names and browser-normalized names to exact source bytes.
/// Implicit directories are derived from the same source snapshot.
#[derive(Default)]
pub(super) struct NameLookup {
    names: HashMap<String, Candidate>,
    ambiguous: Vec<String>,
}

impl NameLookup {
    pub(super) fn new<'a>(
        entries: impl IntoIterator<Item = (&'a EntryPath, bool)>,
        ctl: &ControlToken,
    ) -> Result<Self, FormatError> {
        ctl.checkpoint()?;
        let mut lookup = Self::default();
        for (path, directory) in entries {
            ctl.checkpoint()?;
            lookup.insert(path, directory);
            let encoding =
                Encoding::for_label(path.encoding.as_bytes()).unwrap_or(encoding_rs::UTF_8);
            for (index, byte) in path.raw.iter().enumerate() {
                ctl.checkpoint()?;
                if matches!(*byte, b'/' | b'\\') && index + 1 < path.raw.len() {
                    // A legacy-encoded prefix can also be valid UTF-8. Keep
                    // the encoding already chosen for the complete entry.
                    let raw = &path.raw[..=index];
                    let (display, _) = encoding.decode_without_bom_handling(raw);
                    if !display.ends_with(['/', '\\']) {
                        continue;
                    }
                    let parent =
                        EntryPath::from_raw(raw.to_vec(), display.into_owned(), path.encoding);
                    lookup.insert(&parent, true);
                }
            }
        }
        Ok(lookup)
    }

    fn insert(&mut self, path: &EntryPath, directory: bool) {
        let original = if directory && !path.display.ends_with(['/', '\\']) {
            format!("{}/", path.display)
        } else {
            path.display.clone()
        };
        for name in [original, path.normalized_display(directory).into_owned()] {
            match self.names.entry(name) {
                std::collections::hash_map::Entry::Occupied(mut entry) => {
                    let existing = entry.get();
                    if !existing.ambiguous
                        && (existing.path.raw != path.raw || existing.directory != directory)
                    {
                        self.ambiguous.push(entry.key().clone());
                        entry.get_mut().ambiguous = true;
                    }
                }
                std::collections::hash_map::Entry::Vacant(entry) => {
                    entry.insert(Candidate {
                        path: path.clone(),
                        directory,
                        ambiguous: false,
                    });
                }
            }
        }
    }

    pub(super) fn resolve(
        &self,
        selection: &EntrySelection,
        allow_directory_without_slash: bool,
        ctl: &ControlToken,
    ) -> Result<SelectedName, FormatError> {
        ctl.checkpoint()?;
        let display = match selection {
            EntrySelection::Raw(path) => {
                return Ok(SelectedName {
                    path: path.clone(),
                    directory: path.raw.ends_with(b"/"),
                });
            }
            EntrySelection::Display(display) => display,
        };
        if display.is_empty() {
            return Err(FormatError::Other(
                "selected entry path cannot be empty".into(),
            ));
        }
        let exact = self.names.get(display);
        let directory = (allow_directory_without_slash && !display.ends_with('/'))
            .then(|| self.names.get(&format!("{display}/")))
            .flatten();
        if exact
            .zip(directory)
            .is_some_and(|(left, right)| left.path.raw != right.path.raw)
        {
            return Err(ambiguous(display));
        }
        let candidate = exact.or(directory).ok_or_else(|| {
            FormatError::Other(format!("selected entry not found in archive: {display}"))
        })?;
        if candidate.ambiguous {
            return Err(ambiguous(display));
        }
        if candidate.directory {
            let prefix = candidate.path.normalized_display(true);
            for name in &self.ambiguous {
                ctl.checkpoint()?;
                if name.starts_with(prefix.as_ref()) {
                    return Err(ambiguous(name));
                }
            }
        }
        Ok(SelectedName {
            path: candidate.path.clone(),
            directory: candidate.directory,
        })
    }
}

fn ambiguous(path: &str) -> FormatError {
    FormatError::Other(format!("ambiguous selected entry in archive: {path}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lookup(paths: &[EntryPath]) -> NameLookup {
        NameLookup::new(
            paths.iter().map(|path| (path, path.raw.ends_with(b"/"))),
            &ControlToken::new(),
        )
        .unwrap()
    }

    fn select(lookup: &NameLookup, display: &str, rename: bool) -> Result<EntryPath, FormatError> {
        lookup
            .resolve(
                &EntrySelection::Display(display.into()),
                rename,
                &ControlToken::new(),
            )
            .map(|selected| selected.path)
    }

    #[test]
    fn display_selection_preserves_raw_identity_and_implicit_directories() {
        let legacy = EntryPath::from_raw(b"\xc4\xe3[1].txt".to_vec(), "你[1].txt".into(), "GBK");
        let paths = [
            legacy.clone(),
            EntryPath::from_utf8("docs/sub/file.txt"),
            EntryPath::from_utf8("other/docs/file.txt"),
        ];
        let names = lookup(&paths);
        assert_eq!(select(&names, "你[1].txt", false).unwrap(), legacy);
        assert_eq!(select(&names, "docs/", false).unwrap().raw, b"docs/");
        assert_eq!(select(&names, "docs", true).unwrap().raw, b"docs/");
        for missing in ["", "missing", "docs", "你[1].txt/"] {
            assert!(select(&names, missing, false).is_err(), "{missing}");
        }
    }

    #[test]
    fn display_selection_rejects_colliding_aliases_and_descendants() {
        let paths = [
            EntryPath::from_utf8("root/file.txt"),
            EntryPath::from_utf8("/root/file.txt"),
            EntryPath::from_utf8("root/"),
        ];
        let names = lookup(&paths);
        for selected in ["root/file.txt", "root/"] {
            assert!(matches!(select(&names, selected, false),
                Err(FormatError::Other(message)) if message.contains("ambiguous")));
        }
        assert_eq!(
            names
                .resolve(
                    &EntrySelection::Raw(paths[0].clone()),
                    false,
                    &ControlToken::new()
                )
                .unwrap()
                .path,
            paths[0]
        );
        let paths = [
            EntryPath::from_utf8("file"),
            EntryPath::from_utf8("file/child"),
        ];
        assert!(select(&lookup(&paths), "file", true).is_err());
    }

    #[test]
    fn display_selection_honors_cancellation_and_empty_archives() {
        let names = lookup(&[]);
        assert!(select(&names, "missing", false).is_err());
        let ctl = ControlToken::new();
        ctl.cancel();
        assert!(matches!(
            names.resolve(&EntrySelection::Display("file".into()), false, &ctl),
            Err(FormatError::Cancelled)
        ));
        assert!(matches!(
            NameLookup::new(std::iter::empty(), &ctl),
            Err(FormatError::Cancelled)
        ));
    }
}
