use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek};

use squallz_format_api::{sanitize_entry_path, ControlToken, EntryPath, FormatError, UpdateOp};
use zip::ZipArchive;

use super::super::encoding::{decode_entry_name, resolve_fallback_encoding};
use super::selection::NameLookup;
use super::{addition_meta, map_controlled_zip_error, AdditionSet, Deletions};

pub(super) struct UpdatePlan {
    pub(super) deletes: Deletions,
    pub(super) renames: HashMap<Vec<u8>, String>,
    pub(super) encoding: Option<&'static encoding_rs::Encoding>,
}

struct ArchiveName {
    path: EntryPath,
    key: Vec<u8>,
    directory: bool,
    deleted: bool,
}

struct Rename {
    target: String,
    directory: bool,
    encoding: Option<&'static encoding_rs::Encoding>,
}

#[derive(Default)]
struct Namespace {
    files: HashSet<Vec<u8>>,
    directories: HashSet<Vec<u8>>,
    entries: HashSet<Vec<u8>>,
}

impl Namespace {
    fn insert(&mut self, key: &[u8], directory: bool) {
        self.entries.insert(key.to_owned());
        if directory {
            self.directories.insert(key.to_owned());
        } else {
            self.files.insert(key.to_owned());
        }
        self.directories.extend(parents(key).map(<[u8]>::to_vec));
    }

    fn contains(&self, key: &[u8]) -> bool {
        self.files.contains(key) || self.directories.contains(key)
    }

    fn blocks(&self, key: &[u8], directory: bool) -> bool {
        self.files.contains(key)
            || (!directory && self.directories.contains(key))
            || parents(key).any(|parent| self.files.contains(parent))
    }
}

fn parents(path: &[u8]) -> impl Iterator<Item = &[u8]> {
    path.iter()
        .enumerate()
        .filter_map(|(index, byte)| (*byte == b'/').then_some(&path[..index]))
}

fn path_key(path: &[u8]) -> &[u8] {
    let end = path
        .iter()
        .rposition(|byte| *byte != b'/')
        .map_or(0, |index| index + 1);
    &path[..end]
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
    additions: &impl AdditionSet,
    encoding: Option<&str>,
    ctl: &ControlToken,
) -> Result<UpdatePlan, FormatError> {
    let mut raw_names = Vec::with_capacity(archive.len());
    for index in 0..archive.len() {
        ctl.checkpoint()?;
        let file = archive
            .by_index_raw(index)
            .map_err(|error| map_controlled_zip_error(error, ctl))?;
        raw_names.push((file.name_raw().to_vec(), file.is_dir()));
    }
    let fallback_encoding =
        resolve_fallback_encoding(raw_names.iter().map(|name| &name.0), encoding);
    let mut names = Vec::with_capacity(raw_names.len());
    for (raw, directory) in raw_names {
        ctl.checkpoint()?;
        names.push(ArchiveName {
            key: path_key(&raw).to_vec(),
            path: decode_entry_name(&raw, fallback_encoding),
            directory,
            deleted: false,
        });
    }
    let lookup = NameLookup::new(names.iter().map(|name| (&name.path, name.directory)), ctl)?;
    let mut deletes = Deletions::new(ops, &lookup, ctl)?;
    let mut original = Namespace::default();
    for name in &mut names {
        ctl.checkpoint()?;
        deletes.observe(&name.path.raw);
        name.deleted = deletes.matches(&name.path);
        original.insert(&name.key, name.directory);
    }
    deletes.validate()?;

    let mut requested = HashMap::<Vec<u8>, Rename>::new();
    let mut destinations = HashSet::new();
    for op in ops {
        ctl.checkpoint()?;
        let UpdateOp::Rename { from, to } = op else {
            continue;
        };
        let selected = lookup.resolve(from, true, ctl)?;
        let from = selected.path;
        let source = path_key(&from.raw);
        let directory = original.directories.contains(source);
        if (selected.directory || directory) && from.display.contains('\\') {
            return Err(FormatError::Unsupported(
                "directory renaming requires forward-slash path separators".into(),
            ));
        }
        if !original.contains(source) || (from.raw.ends_with(b"/") && !directory) {
            return Err(FormatError::Other(format!(
                "rename source not found in archive: {from}"
            )));
        }
        if directory && original.files.contains(source) && !selected.directory {
            return Err(FormatError::Other(format!(
                "ambiguous rename source: {from}"
            )));
        }
        let target = safe_target(&to.display)?;
        let source_display = from.normalized_display(directory);
        let source_display = source_display.trim_end_matches('/');
        if source == target.as_bytes()
            || source_display == target
            || (directory
                && (target.starts_with(&format!("{source_display}/"))
                    || target
                        .as_bytes()
                        .strip_prefix(source)
                        .is_some_and(|suffix| suffix.starts_with(b"/"))))
        {
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
            .insert(
                source.to_owned(),
                Rename {
                    target,
                    directory,
                    encoding: if from.encoding.eq_ignore_ascii_case("utf-8") {
                        fallback_encoding
                    } else {
                        encoding_rs::Encoding::for_label(from.encoding.as_bytes())
                            .or(fallback_encoding)
                    },
                },
            )
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
                "overlapping rename sources: {}",
                decode_entry_name(source, fallback_encoding).display
            )));
        }
    }

    let mut renames = HashMap::new();
    let mut retained = Namespace::default();
    for name in &names {
        ctl.checkpoint()?;
        if name.deleted {
            continue;
        }
        let source = requested
            .get_key_value(&name.key)
            .filter(|(_, item)| item.directory == name.directory)
            .or_else(|| {
                parents(&name.key).find_map(|parent| {
                    requested
                        .get_key_value(parent)
                        .filter(|(_, item)| item.directory)
                })
            });
        if let Some((source, request)) = source {
            let suffix = &name.key[source.len()..];
            let suffix = match std::str::from_utf8(suffix) {
                Ok(suffix) if name.path.encoding.eq_ignore_ascii_case("utf-8") => {
                    std::borrow::Cow::Borrowed(suffix)
                }
                _ => {
                    let encoding = request.encoding.ok_or_else(|| {
                        FormatError::Unsupported(
                            "renaming this directory requires an unambiguous entry-name encoding"
                                .into(),
                        )
                    })?;
                    let (suffix, errors) = encoding.decode_without_bom_handling(suffix);
                    if errors {
                        return Err(FormatError::Unsupported(
                            "renaming this directory requires an unambiguous entry-name encoding"
                                .into(),
                        ));
                    }
                    suffix
                }
            };
            let target = format!("{}{suffix}", request.target);
            let target = safe_target(&target)?;
            renames.insert(
                name.path.raw.clone(),
                format!("{target}{}", if name.directory { "/" } else { "" }),
            );
        } else {
            retained.insert(&name.key, name.directory);
            let display = &name.path.display;
            if display.as_bytes() != name.path.raw {
                retained.insert(path_key(display.as_bytes()), name.directory);
            }
            let normalized = name.path.normalized_display(name.directory);
            if normalized.as_ref() != display {
                retained.insert(path_key(normalized.as_bytes()), name.directory);
            }
        }
    }
    for request in requested.values().filter(|item| item.directory) {
        ctl.checkpoint()?;
        if retained.contains(request.target.as_bytes()) {
            return Err(FormatError::Other(format!(
                "update target already exists in archive: {}",
                request.target
            )));
        }
    }

    let mut produced = Namespace::default();
    for name in &names {
        ctl.checkpoint()?;
        if let Some(target) = renames.get(&name.path.raw) {
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
    Ok(UpdatePlan {
        deletes,
        renames,
        encoding: fallback_encoding,
    })
}

fn validate_target(
    target: &str,
    directory: bool,
    retained: &Namespace,
    produced: &mut Namespace,
) -> Result<(), FormatError> {
    let key = safe_target(target)?;
    if retained.contains(key.as_bytes()) || retained.blocks(key.as_bytes(), directory) {
        return Err(FormatError::Other(format!(
            "update target already exists in archive: {target}"
        )));
    }
    if produced.entries.contains(key.as_bytes()) || produced.blocks(key.as_bytes(), directory) {
        return Err(FormatError::Other(format!(
            "duplicate update target in archive: {target}"
        )));
    }
    produced.insert(key.as_bytes(), directory);
    Ok(())
}
