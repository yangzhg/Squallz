use std::collections::HashMap;

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use squallz_format_api::{ControlToken, FormatError, UpdateOp};

/// Keeps explicit glob operations separate from literal entry identities.
pub(super) struct Deletions {
    patterns: Option<GlobSet>,
    entries: HashMap<Vec<u8>, bool>,
}

impl Deletions {
    pub(super) fn new(ops: &[UpdateOp], ctl: &ControlToken) -> Result<Self, FormatError> {
        let mut patterns = GlobSetBuilder::new();
        let mut has_patterns = false;
        let mut entries = HashMap::new();
        for op in ops {
            ctl.checkpoint()?;
            match op {
                UpdateOp::Delete { pattern } => {
                    // Match the engine's PathFilter: bare names match at any
                    // depth, and matched directories include their subtree.
                    let path = pattern.trim_end_matches('/');
                    let mut variants = vec![path.to_owned(), format!("{path}/**")];
                    if !path.contains('/') {
                        variants.push(format!("**/{path}"));
                        variants.push(format!("**/{path}/**"));
                    }
                    for variant in variants {
                        let glob = GlobBuilder::new(&variant)
                            .literal_separator(true)
                            .build()
                            .map_err(|error| {
                                FormatError::Other(format!(
                                    "invalid glob pattern '{pattern}': {error}"
                                ))
                            })?;
                        patterns.add(glob);
                    }
                    has_patterns = true;
                }
                UpdateOp::DeleteEntry { path } => {
                    if path.raw.is_empty() {
                        return Err(FormatError::Other(
                            "delete entry path cannot be empty".into(),
                        ));
                    }
                    entries.insert(path.raw.clone(), false);
                }
                _ => {}
            }
        }
        let patterns = if has_patterns {
            Some(patterns.build().map_err(|error| {
                FormatError::Other(format!("invalid glob pattern set: {error}"))
            })?)
        } else {
            None
        };
        Ok(Self { patterns, entries })
    }

    pub(super) fn matches(&self, raw: &[u8]) -> bool {
        (!self.entries.is_empty()
            && (self.entries.contains_key(raw)
                || directories(raw).any(|parent| self.entries.contains_key(parent))))
            || self.patterns.as_ref().is_some_and(|patterns| {
                patterns.is_match(String::from_utf8_lossy(raw).trim_end_matches('/'))
            })
    }

    pub(super) fn observe(&mut self, raw: &[u8]) {
        if self.entries.is_empty() {
            return;
        }
        for path in std::iter::once(raw).chain(directories(raw)) {
            if let Some(found) = self.entries.get_mut(path) {
                *found = true;
            }
        }
    }

    pub(super) fn validate(&self) -> Result<(), FormatError> {
        if let Some((path, _)) = self.entries.iter().find(|(_, found)| !**found) {
            return Err(FormatError::Other(format!(
                "delete entry not found in archive: {}",
                String::from_utf8_lossy(path)
            )));
        }
        Ok(())
    }
}

fn directories(raw: &[u8]) -> impl Iterator<Item = &[u8]> {
    raw.iter()
        .enumerate()
        .filter_map(|(index, byte)| (*byte == b'/').then_some(&raw[..=index]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use squallz_format_api::EntryPath;

    #[test]
    fn literal_deletions_use_original_bytes_not_lossy_display_names() {
        let raw = vec![0xff, b'/', b'[', b'1', b']', b'.', b't', b'x', b't'];
        let mut deletions = Deletions::new(
            &[UpdateOp::DeleteEntry {
                path: EntryPath::from_raw(raw.clone(), "replacement.txt".into(), "GBK"),
            }],
            &ControlToken::new(),
        )
        .unwrap();
        assert!(deletions.matches(&raw));
        assert!(!deletions.matches(b"replacement.txt"));
        let mut other = raw.clone();
        other[0] = 0xfe;
        assert!(!deletions.matches(&other));
        deletions.observe(&other);
        assert!(deletions.validate().is_err());
        deletions.observe(&raw);
        deletions.validate().unwrap();
    }
}
