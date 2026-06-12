//! Symlink/hardlink target resolution inside an archive, used by the shared
//! extraction engine for [`crate::SymlinkPolicy::Follow`] and hardlink
//! entries.
//!
//! All resolution happens on normalized `/`-separated display paths and is
//! confined to the archive itself: targets that escape the archive root,
//! point to absolute paths, or form cycles resolve to `None` (the caller
//! skips such entries).

use std::collections::{HashMap, HashSet};

use crate::entry::{EntryMeta, EntryType};

/// Upper bound on symlink chain hops (mirrors typical kernel limits).
const MAX_LINK_DEPTH: usize = 40;

/// Normalizes an archive-internal path: drops empty and `.` components and
/// trailing slashes. Returns `None` when a `..` component escapes the root
/// or the path is absolute.
fn normalize(path: &str) -> Option<String> {
    if path.starts_with('/') {
        return None;
    }
    let unified = path.replace('\\', "/");
    let mut parts: Vec<&str> = Vec::new();
    for comp in unified.split('/') {
        match comp {
            "" | "." => continue,
            ".." => {
                parts.pop()?;
            }
            c => parts.push(c),
        }
    }
    Some(parts.join("/"))
}

/// Normalizes a hardlink target (raw bytes): hardlink targets name another
/// entry by its full archive path (tar semantics), so no joining happens.
pub(crate) fn normalize_archive_path(target: &[u8]) -> Option<String> {
    let target = String::from_utf8_lossy(target);
    if target.is_empty() {
        return None;
    }
    normalize(&target).filter(|p| !p.is_empty())
}

/// Resolves a *symlink* target (raw bytes) against the directory containing
/// the link (file-system semantics). Returns the normalized
/// archive-internal path of the target.
pub(crate) fn resolve_target_path(link_path: &str, target: &[u8]) -> Option<String> {
    let target = String::from_utf8_lossy(target);
    if target.is_empty() {
        return None;
    }
    let normalized_link = normalize(link_path)?;
    let parent = link_parent(&normalized_link);
    let joined = if target.starts_with('/') {
        return None; // absolute target: outside the archive by definition
    } else if parent.is_empty() {
        target.into_owned()
    } else {
        format!("{parent}/{target}")
    };
    normalize(&joined)
}

fn link_parent(normalized_link_path: &str) -> &str {
    let mut parent = "";
    if let Some((dir, _name)) = normalized_link_path.rsplit_once('/') {
        parent = dir;
    }
    parent
}

/// Index over archive entries for link resolution, keyed by normalized
/// display path.
pub(crate) struct LinkResolver<'a> {
    by_path: HashMap<String, &'a EntryMeta>,
}

impl<'a> LinkResolver<'a> {
    /// Builds the index from the full entry list of the archive.
    pub(crate) fn new(metas: &'a [EntryMeta]) -> Self {
        let mut by_path = HashMap::with_capacity(metas.len());
        for meta in metas {
            if let Some(key) = normalize(&meta.path.display) {
                if !key.is_empty() {
                    by_path.insert(key, meta);
                }
            }
        }
        Self { by_path }
    }

    /// Follows a link chain starting at `link` until it reaches a regular
    /// file entry inside the archive. Symlink targets resolve relative to
    /// the link's directory, hardlink targets are full archive paths.
    /// Returns `None` for targets that leave the archive, dangle, form a
    /// cycle, or end on a non-file entry.
    pub(crate) fn resolve_to_file(&self, link: &EntryMeta) -> Option<&'a EntryMeta> {
        let mut visited: HashSet<String> = HashSet::new();
        let mut current_path = normalize(&link.path.display)?;
        let mut current = link.entry_type.clone();
        for _ in 0..MAX_LINK_DEPTH {
            if !visited.insert(current_path.clone()) {
                return None; // cycle
            }
            let next = match &current {
                EntryType::Symlink { target } => resolve_target_path(&current_path, target)?,
                EntryType::Hardlink { target } => normalize_archive_path(target)?,
                _ => return None,
            };
            let meta = *self.by_path.get(&next)?;
            match &meta.entry_type {
                EntryType::File => return Some(meta),
                EntryType::Symlink { .. } | EntryType::Hardlink { .. } => {
                    current_path = next;
                    current = meta.entry_type.clone();
                }
                _ => return None, // directories etc. have no content copy
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entry::EntryPath;

    fn meta(name: &str, entry_type: EntryType) -> EntryMeta {
        EntryMeta {
            path: EntryPath::from_utf8(name),
            entry_type,
            size: 0,
            compressed_size: None,
            modified: None,
            unix_mode: None,
            crc32: None,
            encrypted: false,
        }
    }

    fn link(name: &str, target: &str) -> EntryMeta {
        meta(
            name,
            EntryType::Symlink {
                target: target.as_bytes().to_vec(),
            },
        )
    }

    #[test]
    fn resolves_relative_targets() {
        assert_eq!(link_parent("link"), "");
        assert_eq!(link_parent("dir/link"), "dir");
        assert_eq!(
            resolve_target_path("link", b"data.txt"),
            Some("data.txt".into())
        );
        assert_eq!(
            resolve_target_path("dir/link", b"../data.txt"),
            Some("data.txt".into())
        );
        assert_eq!(
            resolve_target_path("dir/link", b"sub/./x"),
            Some("dir/sub/x".into())
        );
        // Escaping the archive root or absolute targets resolve to None.
        assert_eq!(resolve_target_path("link", b"../evil"), None);
        assert_eq!(resolve_target_path("dir/link", b"/etc/passwd"), None);
    }

    #[test]
    fn resolver_follows_chains_and_detects_cycles() {
        let metas = vec![
            meta("data.txt", EntryType::File),
            link("a", "b"),
            link("b", "data.txt"),
            link("loop1", "loop2"),
            link("loop2", "loop1"),
            link("dangling", "missing.txt"),
        ];
        let resolver = LinkResolver::new(&metas);
        let resolved = resolver.resolve_to_file(&metas[1]).expect("a -> data.txt");
        assert_eq!(resolved.path.display, "data.txt");
        assert!(resolver.resolve_to_file(&metas[3]).is_none(), "cycle");
        assert!(resolver.resolve_to_file(&metas[5]).is_none(), "dangling");
    }
}
