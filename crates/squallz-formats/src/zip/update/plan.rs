use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek};

use globset::GlobSet;
use squallz_format_api::{sanitize_entry_path, ControlToken, EntryPath, FormatError, UpdateOp};
use zip::ZipArchive;

use super::{addition_meta, map_controlled_zip_error, AdditionSet};

struct ArchiveName {
    raw: String,
    key: String,
    directory: bool,
}

struct Rename {
    target: String,
    directory: bool,
}

#[derive(Default)]
struct Namespace {
    files: HashSet<String>,
    directories: HashSet<String>,
    entries: HashSet<String>,
}

impl Namespace {
    fn insert(&mut self, key: &str, directory: bool) {
        self.entries.insert(key.to_owned());
        if directory {
            self.directories.insert(key.to_owned());
        } else {
            self.files.insert(key.to_owned());
        }
        self.directories.extend(parents(key).map(str::to_owned));
    }

    fn contains(&self, key: &str) -> bool {
        self.files.contains(key) || self.directories.contains(key)
    }

    fn blocks(&self, key: &str, directory: bool) -> bool {
        self.files.contains(key)
            || (!directory && self.directories.contains(key))
            || parents(key).any(|parent| self.files.contains(parent))
    }
}

fn parents(path: &str) -> impl Iterator<Item = &str> {
    path.match_indices('/').map(|(index, _)| &path[..index])
}

fn safe_target(path: &str) -> Result<String, FormatError> {
    let path = EntryPath::from_utf8(path);
    let relative = sanitize_entry_path(&path)?;
    Ok(relative
        .components()
        .map(|part| part.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/"))
}

/// Resolves directory operations against the original archive, then validates
/// the complete resulting namespace before any entry is written.
pub(super) fn prepare<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    ops: &[UpdateOp],
    deletes: &Option<GlobSet>,
    additions: &impl AdditionSet,
    ctl: &ControlToken,
) -> Result<HashMap<String, String>, FormatError> {
    let mut names = Vec::with_capacity(archive.len());
    let mut original = Namespace::default();
    for index in 0..archive.len() {
        ctl.checkpoint()?;
        let file = archive
            .by_index_raw(index)
            .map_err(|error| map_controlled_zip_error(error, ctl))?;
        let raw = String::from_utf8_lossy(file.name_raw()).into_owned();
        let key = raw.trim_end_matches('/').to_owned();
        let directory = file.is_dir();
        original.insert(&key, directory);
        names.push(ArchiveName {
            raw,
            key,
            directory,
        });
    }

    let mut requested = HashMap::<String, Rename>::new();
    let mut destinations = HashSet::new();
    for op in ops {
        ctl.checkpoint()?;
        let UpdateOp::Rename { from, to } = op else {
            continue;
        };
        let source = from.display.trim_end_matches('/');
        let directory = original.directories.contains(source);
        if !original.contains(source) || (from.display.ends_with('/') && !directory) {
            return Err(FormatError::Other(format!(
                "rename source not found in archive: {from}"
            )));
        }
        if directory && original.files.contains(source) {
            return Err(FormatError::Other(format!(
                "ambiguous rename source: {from}"
            )));
        }
        let target = safe_target(&to.display)?;
        if source == target || (directory && target.starts_with(&format!("{source}/"))) {
            return Err(FormatError::Other(format!(
                "rename target must be outside the source: {from} -> {to}"
            )));
        }
        if !directory && to.display.ends_with('/') {
            return Err(FormatError::Other(format!(
                "file rename target is a directory: {to}"
            )));
        }
        if !destinations.insert(target.clone()) {
            return Err(FormatError::Other(format!(
                "duplicate update target in archive: {to}"
            )));
        }
        if requested
            .insert(source.to_owned(), Rename { target, directory })
            .is_some()
        {
            return Err(FormatError::Other(format!(
                "duplicate rename source: {from}"
            )));
        }
    }
    for source in requested.keys() {
        ctl.checkpoint()?;
        if parents(source).any(|parent| requested.get(parent).is_some_and(|item| item.directory)) {
            return Err(FormatError::Other(format!(
                "overlapping rename sources: {source}"
            )));
        }
    }

    let mut renames = HashMap::new();
    let mut retained = Namespace::default();
    for name in &names {
        ctl.checkpoint()?;
        if deletes.as_ref().is_some_and(|set| set.is_match(&name.key)) {
            continue;
        }
        let source = requested.get_key_value(&name.key).or_else(|| {
            parents(&name.key).find_map(|parent| {
                requested
                    .get_key_value(parent)
                    .filter(|(_, item)| item.directory)
            })
        });
        if let Some((source, request)) = source {
            let suffix = &name.key[source.len()..];
            let target = format!("{}{suffix}", request.target);
            let target = safe_target(&target)?;
            renames.insert(
                name.raw.clone(),
                format!("{target}{}", if name.directory { "/" } else { "" }),
            );
        } else {
            retained.insert(&name.key, name.directory);
        }
    }
    for request in requested.values().filter(|item| item.directory) {
        ctl.checkpoint()?;
        if retained.contains(&request.target) {
            return Err(FormatError::Other(format!(
                "update target already exists in archive: {}",
                request.target
            )));
        }
    }

    let mut produced = Namespace::default();
    for name in &names {
        ctl.checkpoint()?;
        if let Some(target) = renames.get(&name.raw) {
            validate_target(target, name.directory, &retained, &mut produced)?;
        }
    }
    for index in 0..additions.len() {
        ctl.checkpoint()?;
        let meta = addition_meta(additions, index)?;
        validate_target(
            &meta.path.display,
            matches!(meta.entry_type, squallz_format_api::EntryType::Dir),
            &retained,
            &mut produced,
        )?;
    }
    ctl.checkpoint()?;
    Ok(renames)
}

fn validate_target(
    target: &str,
    directory: bool,
    retained: &Namespace,
    produced: &mut Namespace,
) -> Result<(), FormatError> {
    let key = safe_target(target)?;
    if retained.contains(&key) || retained.blocks(&key, directory) {
        return Err(FormatError::Other(format!(
            "update target already exists in archive: {target}"
        )));
    }
    if produced.entries.contains(&key) || produced.blocks(&key, directory) {
        return Err(FormatError::Other(format!(
            "duplicate update target in archive: {target}"
        )));
    }
    produced.insert(&key, directory);
    Ok(())
}
