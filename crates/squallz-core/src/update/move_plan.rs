use std::collections::{HashMap, HashSet};

use super::target::{blocked_parent, occupied, validate_target};
use crate::api::{ControlToken, FormatError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveMoveConflict {
    ExistingTarget,
    DuplicateTarget,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveMoveItem {
    pub from: String,
    pub to: String,
    pub conflict: Option<ArchiveMoveConflict>,
    pub keep_both_to: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ArchiveMovePlan {
    pub items: Vec<ArchiveMoveItem>,
    pub missing_sources: Vec<String>,
    pub blocked_parent: Option<String>,
}

/// Plans moves against a complete, immutable displayed-path index. `exists`
/// distinguishes files from slash-terminated directories, including implicit
/// directories. The format writer still validates the namespace at execution.
pub fn plan_archive_moves(
    paths: &[String],
    target_dir: &str,
    exists: impl Fn(&str) -> bool,
    ctl: &ControlToken,
) -> Result<ArchiveMovePlan, FormatError> {
    if !target_dir.is_empty() {
        validate_target(target_dir)?;
        if !target_dir.ends_with('/') {
            return Err(FormatError::UnsafeFileName(target_dir.into()));
        }
    }
    let mut plan = ArchiveMovePlan::default();
    let selected: HashSet<&str> = paths.iter().map(String::as_str).collect();
    let mut seen = HashSet::new();
    for path in paths {
        ctl.checkpoint()?;
        if !seen.insert(path)
            || path
                .match_indices('/')
                .any(|(index, _)| index + 1 < path.len() && selected.contains(&path[..=index]))
        {
            continue;
        }
        if !exists(path) {
            plan.missing_sources.push(path.clone());
            continue;
        }
        let directory = path.ends_with('/');
        let clean = path.trim_end_matches('/');
        let name = clean.rsplit('/').next().unwrap_or(clean);
        let to = format!("{target_dir}{name}{}", if directory { "/" } else { "" });
        validate_target(&to)?;
        if to == *path || (directory && target_dir.starts_with(path)) {
            return Err(FormatError::Other(
                "move target must be outside the source".into(),
            ));
        }
        plan.items.push(ArchiveMoveItem {
            from: path.clone(),
            to,
            conflict: None,
            keep_both_to: None,
        });
    }
    plan.blocked_parent = blocked_parent(target_dir, &exists, ctl)?;
    let directory_key = target_dir.trim_end_matches('/');
    if plan.blocked_parent.is_none() && !directory_key.is_empty() && exists(directory_key) {
        plan.blocked_parent = Some(directory_key.into());
    }
    if !plan.missing_sources.is_empty() || plan.blocked_parent.is_some() {
        plan.items.clear();
        return Ok(plan);
    }
    let mut counts = HashMap::<String, usize>::new();
    for item in &plan.items {
        *counts
            .entry(item.to.trim_end_matches('/').into())
            .or_default() += 1;
    }
    let mut reserved: HashSet<String> = counts.keys().cloned().collect();
    let mut next_copy = HashMap::<String, usize>::new();
    for item in &mut plan.items {
        ctl.checkpoint()?;
        let key = item.to.trim_end_matches('/');
        item.conflict = if occupied(key, &exists) {
            Some(ArchiveMoveConflict::ExistingTarget)
        } else if counts.get(key).copied().unwrap_or(0) > 1 {
            Some(ArchiveMoveConflict::DuplicateTarget)
        } else {
            None
        };
        if item.conflict.is_none() {
            continue;
        }
        let directory = item.to.ends_with('/');
        let base_start = key.rfind('/').map_or(0, |index| index + 1);
        let extension = if directory {
            None
        } else {
            key[base_start..]
                .rfind('.')
                .filter(|index| *index > 0)
                .map(|index| base_start + index)
        };
        let (stem, ext) = extension.map_or((key, ""), |index| key.split_at(index));
        let copy = next_copy.entry(key.into()).or_insert(1);
        loop {
            ctl.checkpoint()?;
            let suffix = if *copy == 1 {
                " copy".into()
            } else {
                format!(" copy {copy}")
            };
            let candidate = format!("{stem}{suffix}{ext}");
            *copy = copy.checked_add(1).ok_or_else(|| {
                FormatError::ResourceLimitExceeded("move copy names exhausted".into())
            })?;
            if !reserved.contains(&candidate) && !occupied(&candidate, &exists) {
                reserved.insert(candidate.clone());
                item.keep_both_to =
                    Some(format!("{candidate}{}", if directory { "/" } else { "" }));
                break;
            }
        }
    }
    Ok(plan)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(paths: &[&str], target: &str, existing: &[&str]) -> ArchiveMovePlan {
        let names: HashSet<_> = paths.iter().chain(existing).copied().collect();
        plan_archive_moves(
            &paths.iter().map(|path| (*path).into()).collect::<Vec<_>>(),
            target,
            |path| names.contains(path),
            &ControlToken::new(),
        )
        .unwrap()
    }

    #[test]
    fn keeps_directory_roots_and_reserves_file_directory_aliases() {
        let result = plan(
            &["src/folder/", "src/folder/a.txt", "src/file", "other/file/"],
            "out/",
            &["out/folder/", "out/folder copy", "out/file copy/"],
        );
        assert_eq!(result.items.len(), 3);
        assert_eq!(
            result.items[0].keep_both_to.as_deref(),
            Some("out/folder copy 2/")
        );
        assert_eq!(
            result.items[1].conflict,
            Some(ArchiveMoveConflict::DuplicateTarget)
        );
        assert_eq!(
            result.items[1].keep_both_to.as_deref(),
            Some("out/file copy 2")
        );
        assert_eq!(
            result.items[2].keep_both_to.as_deref(),
            Some("out/file copy 3/")
        );
    }

    #[test]
    fn large_moves_have_unique_stable_names_without_repeated_scans() {
        let mut paths: Vec<String> = (0..5000).map(|i| format!("day-{i}/report.txt")).collect();
        paths.push("other/report copy 1000.txt".into());
        let names: HashSet<_> = paths
            .iter()
            .map(String::as_str)
            .chain(["out/report copy.txt", "out/report copy 2.txt/"])
            .collect();
        let run = || {
            plan_archive_moves(
                &paths,
                "out/",
                |path| names.contains(path),
                &ControlToken::new(),
            )
            .unwrap()
        };
        let result = run();
        assert_eq!(result, run());
        assert_eq!(
            result.items[0].keep_both_to.as_deref(),
            Some("out/report copy 3.txt")
        );
        let targets: HashSet<_> = result
            .items
            .iter()
            .map(|item| item.keep_both_to.as_ref().unwrap_or(&item.to))
            .collect();
        assert_eq!(targets.len(), paths.len());
        assert!(targets.iter().all(|path| !names.contains(path.as_str())));
        assert_eq!(result.items.last().unwrap().to, "out/report copy 1000.txt");
    }

    #[test]
    fn missing_sources_and_non_directory_ancestors_produce_no_moves() {
        let missing =
            plan_archive_moves(&["gone".into()], "out/", |_| false, &ControlToken::new()).unwrap();
        assert_eq!(missing.missing_sources, ["gone"]);
        assert!(missing.items.is_empty());
        let blocked = plan(&["src/a"], "out/file/new/", &["out/file"]);
        assert_eq!(blocked.blocked_parent.as_deref(), Some("out/file"));
        assert!(blocked.items.is_empty());
    }

    #[test]
    fn validates_root_moves_names_subtrees_and_cancellation() {
        let root = plan(
            &["docs/.env", "docs/文档.tar.gz", "docs/folder.ext/"],
            "",
            &[".env", "文档.tar.gz", "folder.ext/"],
        );
        assert_eq!(
            root.items
                .iter()
                .map(|item| item.keep_both_to.as_deref())
                .collect::<Vec<_>>(),
            [
                Some(".env copy"),
                Some("文档.tar copy.gz"),
                Some("folder.ext copy/")
            ]
        );
        for target in ["../", "src/", "src/child/", "wrong\\separator/"] {
            assert!(
                plan_archive_moves(&["src/".into()], target, |_| true, &ControlToken::new())
                    .is_err(),
                "{target}"
            );
        }
        let ctl = ControlToken::new();
        ctl.cancel();
        assert!(matches!(
            plan_archive_moves(&["src/a".into()], "out/", |_| true, &ctl),
            Err(FormatError::Cancelled)
        ));
    }
}
