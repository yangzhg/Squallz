//! Literal entry selection for edits and glob filtering for compression
//! input pruning and selective extraction.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};

use crate::api::{ControlToken, EntryMeta, EntryPath, FormatError};

/// Resolves literal displayed paths to their original entry identities.
/// Directories retain `/` and include their descendants. Missing selections
/// and duplicate displayed names fail rather than selecting an uncertain
/// target. Callers supply exactly the names presented in their interface.
pub fn resolve_literal_selection<'a>(
    entries: impl IntoIterator<Item = (Cow<'a, str>, &'a EntryPath)>,
    selection: &[String],
    control: &ControlToken,
) -> Result<Vec<EntryPath>, FormatError> {
    control.checkpoint()?;
    let mut requested = HashMap::new();
    for path in selection {
        control.checkpoint()?;
        if path.is_empty() {
            return Err(FormatError::Other(
                "selected entry path cannot be empty".into(),
            ));
        }
        requested.insert(path.as_str(), false);
    }
    if requested.is_empty() {
        return Ok(Vec::new());
    }
    let mut selected = Vec::new();
    let mut seen = HashSet::new();
    for (display, path) in entries {
        control.checkpoint()?;
        let mut matched = false;
        let parents = display
            .match_indices('/')
            .map(|(index, _)| &display[..=index]);
        for candidate in std::iter::once(display.as_ref()).chain(parents) {
            if let Some(found) = requested.get_mut(candidate) {
                *found = true;
                matched = true;
            }
        }
        if matched {
            if !seen.insert(display.clone()) {
                return Err(FormatError::Other(format!(
                    "ambiguous selected entry in archive: {display}"
                )));
            }
            selected.push(path.clone());
        }
    }
    control.checkpoint()?;
    if let Some((path, _)) = requested.iter().find(|(_, found)| !**found) {
        return Err(FormatError::Other(format!(
            "selected entry not found in archive: {path}"
        )));
    }
    Ok(selected)
}

/// Compiled set of glob patterns matched against `/`-separated entry paths.
///
/// Each user pattern is expanded so the common intent "just works":
/// - `p` itself;
/// - `p/**` — everything below a matched directory;
/// - patterns without a `/` additionally match at any depth
///   (`**/p`, `**/p/**`), so `--exclude .git` prunes nested `.git`
///   directories and `--include *.txt` selects text files anywhere.
///
/// `*`/`?` never cross path separators (recursion is explicit via the
/// expanded variants).
#[derive(Debug, Default)]
pub struct PathFilter {
    set: Option<GlobSet>,
}

impl PathFilter {
    /// Compiles the patterns. An empty pattern list yields an empty filter
    /// that matches nothing.
    pub fn new(patterns: &[String]) -> Result<Self, FormatError> {
        if patterns.is_empty() {
            return Ok(Self::default());
        }
        let mut builder = GlobSetBuilder::new();
        let mut added = false;
        for pattern in patterns {
            for variant in variants(pattern) {
                let glob = GlobBuilder::new(&variant)
                    .literal_separator(true)
                    .build()
                    .map_err(|e| {
                        FormatError::Other(format!("invalid glob pattern '{pattern}': {e}"))
                    })?;
                builder.add(glob);
                added = true;
            }
        }
        if !added {
            return Ok(Self::default());
        }
        let set = builder
            .build()
            .map_err(|e| FormatError::Other(format!("invalid glob pattern set: {e}")))?;
        Ok(Self { set: Some(set) })
    }

    /// Whether the filter was built from an empty pattern list.
    pub fn is_empty(&self) -> bool {
        self.set.is_none()
    }

    /// Whether `path` (a `/`-separated entry path) matches any pattern.
    /// An empty filter matches nothing.
    pub fn matches(&self, path: &str) -> bool {
        self.set.as_ref().is_some_and(|s| s.is_match(path))
    }

    /// Selects matching archive entries while honoring an interactive
    /// operation's pause/cancellation token. An empty filter keeps the
    /// caller's `None` convention for whole-archive extraction.
    pub fn select_entries(
        &self,
        entries: &[EntryMeta],
        control: &ControlToken,
    ) -> Result<Option<Vec<EntryPath>>, FormatError> {
        if self.is_empty() {
            control.checkpoint()?;
            return Ok(None);
        }
        let mut selected = Vec::new();
        for entry in entries {
            control.checkpoint()?;
            if self.matches(&entry.path.display) {
                selected.push(entry.path.clone());
            }
        }
        control.checkpoint()?;
        Ok(Some(selected))
    }
}

/// Expands one user pattern into the glob variants described on
/// [`PathFilter`].
fn variants(pattern: &str) -> Vec<String> {
    let p = pattern.trim_end_matches('/');
    if p.is_empty() {
        return Vec::new();
    }
    let mut out = vec![p.to_owned(), format!("{p}/**")];
    if !p.contains('/') {
        out.push(format!("**/{p}"));
        out.push(format!("**/{p}/**"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn literal_selection(
        paths: &[EntryPath],
        requested: &[&str],
    ) -> Result<Vec<EntryPath>, FormatError> {
        resolve_literal_selection(
            paths
                .iter()
                .map(|path| (Cow::Borrowed(path.display.as_str()), path)),
            &requested
                .iter()
                .map(|path| (*path).to_owned())
                .collect::<Vec<_>>(),
            &ControlToken::new(),
        )
    }

    #[test]
    fn literal_selection_preserves_raw_names_and_exact_scope() {
        let legacy = EntryPath::from_raw(
            vec![0xc4, 0xe3, b'.', b't', b'x', b't'],
            "你.txt".into(),
            "GBK",
        );
        let paths = [
            EntryPath::from_utf8("notes.txt"),
            EntryPath::from_utf8("other/notes.txt"),
            EntryPath::from_utf8("logs[1]/sub/file.txt"),
            EntryPath::from_utf8("logs1/keep.txt"),
            legacy.clone(),
        ];
        assert_eq!(
            literal_selection(&paths, &["notes.txt", "logs[1]/", "logs[1]/sub/", "你.txt"])
                .unwrap(),
            [paths[0].clone(), paths[2].clone(), legacy]
        );
    }

    #[test]
    fn literal_selection_rejects_missing_empty_or_ambiguous_names() {
        let paths = [
            EntryPath::from_utf8("keep.txt"),
            EntryPath::from_utf8("docs/file.txt"),
        ];
        for requested in ["", "missing", "docs", "keep.txt/"] {
            assert!(
                literal_selection(&paths, &["keep.txt", requested]).is_err(),
                "{requested}"
            );
        }
        let ambiguous = [
            EntryPath::from_raw(vec![0xff], "same.txt".into(), "utf-8"),
            EntryPath::from_raw(vec![0xfe], "same.txt".into(), "utf-8"),
        ];
        assert!(
            matches!(literal_selection(&ambiguous, &["same.txt"]), Err(FormatError::Other(message)) if message.contains("ambiguous"))
        );
    }

    #[test]
    fn literal_selection_honors_cancellation_and_empty_selection() {
        let paths = [EntryPath::from_utf8("keep.txt")];
        assert!(literal_selection(&paths, &[]).unwrap().is_empty());
        let control = ControlToken::new();
        control.cancel();
        assert!(matches!(
            resolve_literal_selection(std::iter::empty(), &[], &control),
            Err(FormatError::Cancelled)
        ));
    }

    fn filter(patterns: &[&str]) -> PathFilter {
        let owned: Vec<String> = patterns.iter().map(|s| (*s).to_owned()).collect();
        PathFilter::new(&owned).unwrap()
    }

    #[test]
    fn empty_filter_matches_nothing() {
        let f = PathFilter::new(&[]).unwrap();
        assert!(f.is_empty());
        assert!(!f.matches("anything"));
    }

    #[test]
    fn empty_patterns_are_ignored() {
        let patterns = vec!["".to_owned(), "/".to_owned()];
        let f = PathFilter::new(&patterns).unwrap();
        assert!(f.is_empty());
        assert!(!f.matches("anything"));
        assert!(!f.matches("nested/file.txt"));
    }

    #[test]
    fn controlled_selection_stops_before_scanning_entries() {
        let f = filter(&["*.txt"]);
        let entries = vec![EntryMeta {
            path: EntryPath::from_utf8("file.txt"),
            entry_type: crate::api::EntryType::File,
            size: 1,
            compressed_size: None,
            modified: None,
            unix_mode: None,
            crc32: None,
            encrypted: false,
        }];
        let control = ControlToken::new();
        control.cancel();

        let error = f.select_entries(&entries, &control).unwrap_err();

        assert!(matches!(error, FormatError::Cancelled));
    }

    #[test]
    fn bare_name_matches_at_any_depth_and_prunes_subtree() {
        let f = filter(&[".git"]);
        assert!(f.matches(".git"));
        assert!(f.matches("project/.git"));
        assert!(f.matches("project/.git/config"));
        assert!(!f.matches("project/src/main.rs"));
        assert!(!f.matches("gitignore"));
    }

    #[test]
    fn star_patterns_match_anywhere_without_crossing_separators() {
        let f = filter(&["*.tmp"]);
        assert!(f.matches("a.tmp"));
        assert!(f.matches("deep/nested/b.tmp"));
        assert!(!f.matches("a.tmp.txt"));
    }

    #[test]
    fn slash_patterns_anchor_to_the_path_root() {
        let f = filter(&["docs/*"]);
        assert!(f.matches("docs/a.md"));
        assert!(f.matches("docs/sub/b.md")); // via docs/*/** subtree variant
        assert!(!f.matches("other/docs.md"));
        assert!(!f.matches("nested/docs/a.md")); // anchored: not at any depth
    }

    #[test]
    fn invalid_pattern_is_reported() {
        let err = PathFilter::new(&["[".to_owned()]).unwrap_err();
        assert!(matches!(err, FormatError::Other(_)));
    }
}
