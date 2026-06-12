//! Glob-based entry-path filtering, shared by `CreateOptions.excludes`
//! (compression input pruning) and `sqz extract --include` (selective
//! extraction).

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};

use crate::api::FormatError;

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
