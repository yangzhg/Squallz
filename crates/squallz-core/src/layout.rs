//! Smart-extraction layout analysis: decide whether an archive already has
//! a single root directory (extract directly) or holds loose entries
//! (wrap them in a folder named after the archive).

use crate::api::{EntryMeta, EntryType};

/// Verdict of [`analyze_extract_layout`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SmartLayout {
    /// Every entry lives under one root directory: extract as-is.
    DirectExtract,
    /// Loose files at the archive root: wrap them in a folder named after
    /// the archive (the caller derives the name).
    WrapInFolder,
}

/// Analyzes the entry list: [`SmartLayout::DirectExtract`] when all entries
/// share the same first path component *and* that component is a directory
/// (an explicit directory entry, or implicit because every entry is nested
/// below it); [`SmartLayout::WrapInFolder`] otherwise.
pub fn analyze_extract_layout(entries: &[EntryMeta]) -> SmartLayout {
    let mut root: Option<String> = None;
    let mut root_is_dir = false;
    for meta in entries {
        let display = meta.path.display.replace('\\', "/");
        let mut comps = display.split('/').filter(|c| !c.is_empty() && *c != ".");
        let Some(first) = comps.next() else {
            continue; // degenerate entry name; ignore for the verdict
        };
        match &root {
            None => root = Some(first.to_string()),
            Some(r) if r != first => return SmartLayout::WrapInFolder,
            Some(_) => {}
        }
        if comps.next().is_none() {
            // Single-component entry: only a directory keeps the verdict.
            if matches!(meta.entry_type, EntryType::Dir) {
                root_is_dir = true;
            } else {
                return SmartLayout::WrapInFolder;
            }
        } else {
            root_is_dir = true; // implicit: something is nested below it
        }
    }
    if root.is_some() && root_is_dir {
        SmartLayout::DirectExtract
    } else if root.is_some() {
        SmartLayout::WrapInFolder
    } else {
        // Empty archive: nothing to wrap.
        SmartLayout::DirectExtract
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{EntryPath, EntryType};

    fn meta(name: &str, dir: bool) -> EntryMeta {
        EntryMeta {
            path: EntryPath::from_utf8(name),
            entry_type: if dir { EntryType::Dir } else { EntryType::File },
            size: 0,
            compressed_size: None,
            modified: None,
            unix_mode: None,
            crc32: None,
            encrypted: false,
        }
    }

    #[test]
    fn single_root_directory_extracts_directly() {
        // Explicit directory entry.
        let entries = vec![
            meta("project/", true),
            meta("project/a.txt", false),
            meta("project/sub/b.txt", false),
        ];
        assert_eq!(analyze_extract_layout(&entries), SmartLayout::DirectExtract);
        // Implicit root (no explicit dir entry).
        let entries = vec![meta("root/a.txt", false), meta("root/b/c.txt", false)];
        assert_eq!(analyze_extract_layout(&entries), SmartLayout::DirectExtract);
    }

    #[test]
    fn windows_separators_are_normalized_for_layout() {
        let entries = vec![meta("root\\a.txt", false), meta("root\\sub\\b.txt", false)];
        assert_eq!(analyze_extract_layout(&entries), SmartLayout::DirectExtract);

        let entries = vec![meta("root\\a.txt", false), meta("other\\b.txt", false)];
        assert_eq!(analyze_extract_layout(&entries), SmartLayout::WrapInFolder);
    }

    #[test]
    fn loose_entries_wrap_in_folder() {
        // Multiple roots.
        let entries = vec![meta("a.txt", false), meta("b.txt", false)];
        assert_eq!(analyze_extract_layout(&entries), SmartLayout::WrapInFolder);
        // Single loose file (not a directory).
        let entries = vec![meta("readme.md", false)];
        assert_eq!(analyze_extract_layout(&entries), SmartLayout::WrapInFolder);
        // A root dir plus a stray top-level file.
        let entries = vec![
            meta("root/", true),
            meta("root/a", false),
            meta("x.txt", false),
        ];
        assert_eq!(analyze_extract_layout(&entries), SmartLayout::WrapInFolder);
    }

    #[test]
    fn empty_archive_extracts_directly() {
        assert_eq!(analyze_extract_layout(&[]), SmartLayout::DirectExtract);
    }
}
