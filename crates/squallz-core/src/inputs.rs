//! Compression input collection: walks the input paths into an entry
//! manifest, applying `CreateOptions.excludes` glob pruning.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::api::{EntryPath, EntryType, FormatError};
use crate::filter::PathFilter;

/// One item of the compression input manifest.
pub(crate) struct InputItem {
    pub src: PathBuf,
    pub name: EntryPath,
    pub entry_type: EntryType,
    pub size: u64,
    pub unix_mode: Option<u32>,
    pub modified: Option<SystemTime>,
}

/// Walks the input paths and produces the entry manifest. Entry names are
/// relative to each input's parent directory (the top-level folder name is
/// kept); symbolic links are not followed. Entries matching `excludes` are
/// pruned (a matched directory is skipped with its whole subtree).
pub(crate) fn collect_inputs(
    inputs: &[PathBuf],
    excludes: &PathFilter,
) -> Result<Vec<InputItem>, FormatError> {
    collect_inputs_with_progress(inputs, excludes, |_count, _path| {})
}

pub(crate) fn collect_inputs_with_progress(
    inputs: &[PathBuf],
    excludes: &PathFilter,
    mut progress: impl FnMut(usize, &EntryPath),
) -> Result<Vec<InputItem>, FormatError> {
    let mut out = Vec::new();
    for input in inputs {
        let base = input.parent().map_or_else(PathBuf::new, Path::to_path_buf);
        walk(input, &base, excludes, &mut out, &mut progress)?;
    }
    Ok(out)
}

fn walk(
    path: &Path,
    base: &Path,
    excludes: &PathFilter,
    out: &mut Vec<InputItem>,
    progress: &mut impl FnMut(usize, &EntryPath),
) -> Result<(), FormatError> {
    let meta = std::fs::symlink_metadata(path)?;
    let rel = match path.strip_prefix(base) {
        Ok(rel) => rel,
        Err(_) => path,
    };
    let name = EntryPath::from_utf8(
        rel.to_string_lossy()
            .replace(std::path::MAIN_SEPARATOR, "/"),
    );
    if excludes.matches(&name.display) {
        // Pruned: a matched directory is skipped together with its subtree.
        return Ok(());
    }
    let unix_mode = unix_mode_of(&meta);
    let modified = meta.modified().ok();

    if meta.file_type().is_symlink() {
        let target = std::fs::read_link(path)?;
        push_input(
            out,
            InputItem {
                src: path.to_path_buf(),
                name,
                entry_type: EntryType::Symlink {
                    target: target.to_string_lossy().into_owned().into_bytes(),
                },
                size: 0,
                unix_mode,
                modified,
            },
            progress,
        );
    } else if meta.is_dir() {
        push_input(
            out,
            InputItem {
                src: path.to_path_buf(),
                name,
                entry_type: EntryType::Dir,
                size: 0,
                unix_mode,
                modified,
            },
            progress,
        );
        let mut children: Vec<PathBuf> = std::fs::read_dir(path)?
            .map(|e| e.map(|e| e.path()))
            .collect::<Result<_, _>>()?;
        children.sort();
        for child in children {
            walk(&child, base, excludes, out, progress)?;
        }
    } else {
        push_input(
            out,
            InputItem {
                src: path.to_path_buf(),
                name,
                entry_type: EntryType::File,
                size: meta.len(),
                unix_mode,
                modified,
            },
            progress,
        );
    }
    Ok(())
}

fn push_input(
    out: &mut Vec<InputItem>,
    item: InputItem,
    progress: &mut impl FnMut(usize, &EntryPath),
) {
    out.push(item);
    let count = out.len();
    if let Some(item) = out.last() {
        progress(count, &item.name);
    }
}

#[cfg(unix)]
fn unix_mode_of(meta: &std::fs::Metadata) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    Some(meta.permissions().mode())
}

#[cfg(not(unix))]
fn unix_mode_of(_meta: &std::fs::Metadata) -> Option<u32> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("squallz-core-inputs-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn names(items: &[InputItem]) -> Vec<String> {
        items.iter().map(|i| i.name.display.clone()).collect()
    }

    #[test]
    fn collect_inputs_walks_tree_with_top_folder_name() {
        let dir = temp_dir("walk");
        let root = dir.join("project");
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("a.txt"), b"hello").unwrap();
        std::fs::write(root.join("sub/b.txt"), b"world!").unwrap();

        let items = collect_inputs(std::slice::from_ref(&root), &PathFilter::default()).unwrap();
        assert_eq!(
            names(&items),
            vec![
                "project",
                "project/a.txt",
                "project/sub",
                "project/sub/b.txt"
            ]
        );
        let total: u64 = items.iter().map(|i| i.size).sum();
        assert_eq!(total, 11);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn collect_inputs_applies_exclude_globs() {
        let dir = temp_dir("excludes");
        let root = dir.join("project");
        std::fs::create_dir_all(root.join(".git/objects")).unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join(".git/config"), b"git").unwrap();
        std::fs::write(root.join(".git/objects/x"), b"obj").unwrap();
        std::fs::write(root.join("src/main.rs"), b"fn main() {}").unwrap();
        std::fs::write(root.join("scratch.tmp"), b"tmp").unwrap();

        let excludes = PathFilter::new(&[".git".to_owned(), "*.tmp".to_owned()]).unwrap();
        let items = collect_inputs(std::slice::from_ref(&root), &excludes).unwrap();
        assert_eq!(
            names(&items),
            vec!["project", "project/src", "project/src/main.rs"]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn collect_inputs_reports_progress_for_kept_items() {
        let dir = temp_dir("progress");
        let root = dir.join("project");
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join(".git/config"), b"git").unwrap();
        std::fs::write(root.join("src/main.rs"), b"fn main() {}").unwrap();
        std::fs::write(root.join("scratch.tmp"), b"tmp").unwrap();

        let excludes = PathFilter::new(&[".git".to_owned(), "*.tmp".to_owned()]).unwrap();
        let mut progress = Vec::new();
        let items =
            collect_inputs_with_progress(std::slice::from_ref(&root), &excludes, |count, path| {
                progress.push((count, path.display.clone()));
            })
            .unwrap();

        assert_eq!(
            names(&items),
            vec!["project", "project/src", "project/src/main.rs"]
        );
        assert_eq!(
            progress,
            vec![
                (1, "project".to_owned()),
                (2, "project/src".to_owned()),
                (3, "project/src/main.rs".to_owned())
            ]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
