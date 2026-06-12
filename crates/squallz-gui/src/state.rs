//! Tauri-independent application state: the engine, the opened-archive
//! cache with per-directory pagination, and the session password cache.
//! Everything here is plain Rust so it can be unit-tested without a window.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};

use squallz_core::api::{
    split_volume_name, Detected, EntryMeta, EntryType, FormatError, OpenOptions, Password,
};
use squallz_core::{collect_volume_set, Engine, VolumeSet};

use crate::dto::{ArchiveInfo, EntryDto, Page};

/// Default page size of the entry list.
pub const DEFAULT_PAGE_SIZE: usize = 500;
const UNKNOWN_FORMAT_LABEL: &str = "unknown";

/// One row at a directory level: either a real entry index or a synthesized
/// intermediate directory.
#[derive(Debug, Clone)]
struct Row {
    /// Base name at this level
    name: String,
    /// Index into `CachedArchive::entries` (`None` = synthesized directory)
    entry: Option<usize>,
    /// Whether the row is a directory
    is_dir: bool,
}

/// A fully listed archive kept in memory for browsing.
pub struct CachedArchive {
    /// All entries in archive order
    pub entries: Vec<EntryMeta>,
    /// Directory level → sorted rows ("" = root, otherwise `a/b/`)
    levels: HashMap<String, Vec<Row>>,
}

/// Shared application state.
pub struct AppState {
    /// The engine (registry of all built-in formats)
    pub engine: Engine,
    archives: Mutex<HashMap<u64, CachedArchive>>,
    next_id: AtomicU64,
    /// Session password cache: archive path → password (zeroized on drop,
    /// cleared when the app exits.
    passwords: Mutex<HashMap<PathBuf, Password>>,
}

fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

impl AppState {
    /// Builds the state with the full built-in format registry.
    pub fn new() -> Self {
        Self {
            engine: Engine::new(squallz_formats::registry()),
            archives: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            passwords: Mutex::new(HashMap::new()),
        }
    }

    /// Opens an archive, lists all entries and caches them under a fresh id.
    pub fn open_archive(
        &self,
        path: &Path,
        password: Option<&str>,
        encoding: Option<&str>,
    ) -> Result<ArchiveInfo, FormatError> {
        let open_opts = OpenOptions {
            password: password
                .map(Password::new)
                .or_else(|| self.password_for(path)),
            encoding_override: encoding.map(str::to_owned),
        };
        let entries = self.engine.list(path, &open_opts)?;
        // Remember a freshly supplied, proven-good password for the session.
        if let Some(pw) = password {
            self.remember_password(path, pw);
        }

        let file_name = archive_file_name(path);
        let volumes = if split_volume_name(&file_name).is_some() {
            Some(collect_volume_set(path)?)
        } else {
            None
        };
        let display_name = archive_display_name(&file_name);
        let format = format_label_for_name(&self.engine, &display_name);
        let encoding = encoding_diagnostics(&entries, encoding);

        let levels = build_levels(&entries);
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let info = ArchiveInfo {
            id,
            path: path.to_string_lossy().into_owned(),
            name: display_name,
            format,
            entry_count: entries.len(),
            volumes: volumes.as_ref().map(volume_file_names),
            legacy_encoding_count: encoding.legacy_count,
            garbled_count: encoding.garbled_count,
            suggested_encoding: encoding.suggested,
            encoding_override: encoding.override_label,
        };
        lock_unpoisoned(&self.archives).insert(id, CachedArchive { entries, levels });
        Ok(info)
    }

    /// Drops a cached archive.
    pub fn close_archive(&self, id: u64) {
        lock_unpoisoned(&self.archives).remove(&id);
    }

    /// Pages one directory level of a cached archive. `dir_prefix` is ""
    /// for the root or `a/b/`; `filter` is a case-insensitive substring
    /// match on the base name.
    pub fn list_entries(
        &self,
        id: u64,
        page: usize,
        page_size: usize,
        dir_prefix: &str,
        filter: Option<&str>,
    ) -> Result<Page, FormatError> {
        let archives = lock_unpoisoned(&self.archives);
        let archive = archives
            .get(&id)
            .ok_or_else(|| FormatError::Other(format!("unknown archive handle {id}")))?;
        Ok(page_level(
            archive,
            page,
            page_size.max(1),
            dir_prefix,
            filter,
        ))
    }

    /// Session password for a path, if one was proven good earlier.
    pub fn password_for(&self, path: &Path) -> Option<Password> {
        lock_unpoisoned(&self.passwords).get(path).cloned()
    }

    /// Verifies a password without adding another opened archive handle.
    pub fn verify_password(
        &self,
        path: &Path,
        password: &str,
        encoding: Option<&str>,
    ) -> Result<(), FormatError> {
        let open_opts = OpenOptions {
            password: Some(Password::new(password)),
            encoding_override: encoding.map(str::to_owned),
        };
        self.engine.list(path, &open_opts).map(|_| ())
    }

    /// Caches a working password for the session (zeroized on exit).
    pub fn remember_password(&self, path: &Path, password: &str) {
        lock_unpoisoned(&self.passwords).insert(path.to_path_buf(), Password::new(password));
    }

    /// Removes a session password, used when the user forgets a saved secret.
    pub fn forget_password(&self, path: &Path) {
        lock_unpoisoned(&self.passwords).remove(path);
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

fn archive_file_name(path: &Path) -> String {
    match path.file_name() {
        Some(name) => name.to_string_lossy().into_owned(),
        None => String::new(),
    }
}

fn archive_display_name(file_name: &str) -> String {
    match split_volume_name(file_name) {
        Some((base, _)) => base.to_owned(),
        None => file_name.to_owned(),
    }
}

fn format_label_for_name(engine: &Engine, display_name: &str) -> String {
    match engine.registry().detect_by_name(display_name) {
        Some(Detected::Archive(format)) => format.id().to_owned(),
        Some(Detected::Compressed {
            compressor,
            inner_archive,
        }) => match inner_archive {
            Some(inner) => format!("{}.{}", inner.id(), compressor.id()),
            None => compressor.id().to_owned(),
        },
        None => UNKNOWN_FORMAT_LABEL.to_owned(),
    }
}

fn volume_file_names(parts: &VolumeSet) -> Vec<String> {
    path_file_names(parts.iter())
}

fn path_file_names<'a>(paths: impl IntoIterator<Item = &'a PathBuf>) -> Vec<String> {
    paths
        .into_iter()
        .map(|path| archive_file_name(path))
        .collect()
}

struct EncodingDiagnostics {
    legacy_count: usize,
    garbled_count: usize,
    suggested: Option<String>,
    override_label: Option<String>,
}

fn encoding_diagnostics(
    entries: &[EntryMeta],
    override_label: Option<&str>,
) -> EncodingDiagnostics {
    let mut counts: HashMap<String, usize> = HashMap::new();
    let mut legacy_count = 0;
    let mut garbled_count = 0;
    for meta in entries {
        if meta.path.display.contains('\u{FFFD}') {
            garbled_count += 1;
        }
        if !meta.path.encoding.eq_ignore_ascii_case("utf-8") {
            legacy_count += 1;
            *counts.entry(meta.path.encoding.to_owned()).or_default() += 1;
        }
    }
    let suggested = counts
        .into_iter()
        .max_by_key(|(_, count)| *count)
        .map(|(encoding, _)| encoding);
    EncodingDiagnostics {
        legacy_count,
        garbled_count,
        suggested,
        override_label: override_label.map(str::to_owned),
    }
}

/// Normalizes an entry display path: `\` → `/`, no leading `/`, directories
/// (explicit or implied) end with `/`. Shared with the job layer so display
/// path selections match the rows shown in the list.
pub(crate) fn normalized_entry_path(meta: &EntryMeta) -> String {
    let mut p = meta.path.display.replace('\\', "/");
    while p.starts_with('/') {
        p.remove(0);
    }
    if matches!(meta.entry_type, EntryType::Dir) && !p.ends_with('/') {
        p.push('/');
    }
    p
}

/// Builds the per-directory row index: every entry is attached to its parent
/// level, intermediate directories without explicit entries are synthesized,
/// each level is sorted directories-first then case-insensitively by name.
fn build_levels(entries: &[EntryMeta]) -> HashMap<String, Vec<Row>> {
    let mut levels: HashMap<String, HashMap<String, Row>> = HashMap::new();
    let mut add = |parent: &str, row: Row| {
        let level = levels.entry(parent.to_owned()).or_default();
        match level.get_mut(&row.name) {
            // A real entry replaces a previously synthesized directory.
            Some(existing) => {
                if existing.entry.is_none() && row.entry.is_some() {
                    *existing = row;
                }
            }
            None => {
                level.insert(row.name.clone(), row);
            }
        }
    };

    for (idx, meta) in entries.iter().enumerate() {
        let path = normalized_entry_path(meta);
        let is_dir = path.ends_with('/');
        let trimmed = path.trim_end_matches('/');
        if trimmed.is_empty() {
            continue;
        }
        let segments: Vec<&str> = trimmed.split('/').collect();
        // Synthesize intermediate directories.
        let mut parent = String::new();
        for seg in &segments[..segments.len() - 1] {
            add(
                &parent.clone(),
                Row {
                    name: (*seg).to_owned(),
                    entry: None,
                    is_dir: true,
                },
            );
            parent.push_str(seg);
            parent.push('/');
        }
        let name = segments[segments.len() - 1].to_owned();
        add(
            &parent,
            Row {
                name,
                entry: Some(idx),
                is_dir,
            },
        );
    }

    levels
        .into_iter()
        .map(|(parent, rows)| {
            let mut rows: Vec<Row> = rows.into_values().collect();
            rows.sort_by(|a, b| {
                b.is_dir
                    .cmp(&a.is_dir)
                    .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            });
            (parent, rows)
        })
        .collect()
}

/// Slices one page out of a directory level.
fn page_level(
    archive: &CachedArchive,
    page: usize,
    page_size: usize,
    dir_prefix: &str,
    filter: Option<&str>,
) -> Page {
    let rows = match archive.levels.get(dir_prefix) {
        Some(rows) => rows.as_slice(),
        None => &[],
    };
    let filter_lc = filter
        .map(str::trim)
        .filter(|f| !f.is_empty())
        .map(str::to_lowercase);
    let filtered: Vec<&Row> = rows
        .iter()
        .filter(|r| match &filter_lc {
            Some(f) => r.name.to_lowercase().contains(f),
            None => true,
        })
        .collect();
    let total = filtered.len();
    let start = page.saturating_mul(page_size).min(total);
    let end = (start + page_size).min(total);
    let items = filtered[start..end]
        .iter()
        .map(|row| {
            let full = if row.is_dir {
                format!("{dir_prefix}{}/", row.name)
            } else {
                format!("{dir_prefix}{}", row.name)
            };
            match row.entry {
                Some(idx) => EntryDto::from_meta(&archive.entries[idx], full, row.name.clone()),
                None => EntryDto::synthesized_dir(full, row.name.clone()),
            }
        })
        .collect();
    Page { total, page, items }
}

#[cfg(test)]
mod tests {
    use super::*;
    use squallz_core::api::{
        CompressionLevel, ControlToken, CreateOptions, EntryPath, NoProgress, Password,
    };

    fn make_zip(dir: &Path, names: &[&str]) -> PathBuf {
        let src = dir.join("src");
        std::fs::create_dir_all(&src).unwrap();
        for name in names {
            let p = src.join(name);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, name.as_bytes()).unwrap();
        }
        let dest = dir.join("test.zip");
        let engine = Engine::new(squallz_formats::registry());
        engine
            .create(
                &dest,
                &[src],
                &CreateOptions {
                    level: CompressionLevel::Fastest,
                    ..CreateOptions::default()
                },
                &NoProgress,
                &ControlToken::new(),
            )
            .unwrap();
        dest
    }

    fn make_header_encrypted_7z(dir: &Path) -> PathBuf {
        let src = dir.join("secret-src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("secret.txt"), b"classified").unwrap();
        let dest = dir.join("secret.7z");
        let engine = Engine::new(squallz_formats::registry());
        engine
            .create(
                &dest,
                &[src],
                &CreateOptions {
                    level: CompressionLevel::Fastest,
                    password: Some(Password::new("secret")),
                    encrypt_filenames: true,
                    ..CreateOptions::default()
                },
                &NoProgress,
                &ControlToken::new(),
            )
            .unwrap();
        dest
    }

    fn crc32(data: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFFu32;
        for &b in data {
            crc ^= b as u32;
            for _ in 0..8 {
                let mask = (crc & 1).wrapping_neg();
                crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
            }
        }
        !crc
    }

    fn make_raw_name_zip(dir: &Path, raw_name: &[u8]) -> PathBuf {
        let data = b"legacy name";
        let crc = crc32(data);
        let size = data.len() as u32;
        let name_len = raw_name.len() as u16;
        let mut out = Vec::new();
        let offset = 0u32;

        out.extend_from_slice(&[0x50, 0x4B, 0x03, 0x04]);
        out.extend_from_slice(&20u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0x21u16.to_le_bytes());
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&name_len.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(raw_name);
        out.extend_from_slice(data);

        let central_offset = out.len() as u32;
        out.extend_from_slice(&[0x50, 0x4B, 0x01, 0x02]);
        out.extend_from_slice(&20u16.to_le_bytes());
        out.extend_from_slice(&20u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0x21u16.to_le_bytes());
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&name_len.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        out.extend_from_slice(raw_name);

        let central_size = out.len() as u32 - central_offset;
        out.extend_from_slice(&[0x50, 0x4B, 0x05, 0x06]);
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&central_size.to_le_bytes());
        out.extend_from_slice(&central_offset.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());

        let dest = dir.join("legacy-gbk.zip");
        std::fs::write(&dest, out).unwrap();
        dest
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("squallz-gui-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn meta(path: EntryPath) -> EntryMeta {
        EntryMeta {
            path,
            entry_type: EntryType::File,
            size: 0,
            compressed_size: None,
            modified: None,
            unix_mode: None,
            crc32: None,
            encrypted: false,
        }
    }

    #[test]
    fn encoding_diagnostics_reports_legacy_and_garbled_names() {
        let entries = vec![
            meta(EntryPath::from_utf8("plain.txt")),
            meta(EntryPath::from_raw(
                vec![0xc4, 0xe3],
                "你好.txt".to_owned(),
                "GBK",
            )),
            meta(EntryPath::from_raw(
                vec![0xce, 0xc4],
                "文件.txt".to_owned(),
                "GBK",
            )),
            meta(EntryPath::from_raw(
                vec![0xff],
                "bad\u{FFFD}.txt".to_owned(),
                "windows-1252",
            )),
        ];

        let diag = encoding_diagnostics(&entries, Some("gbk"));
        assert_eq!(diag.legacy_count, 3);
        assert_eq!(diag.garbled_count, 1);
        assert_eq!(diag.suggested.as_deref(), Some("GBK"));
        assert_eq!(diag.override_label.as_deref(), Some("gbk"));
    }

    #[test]
    fn open_archive_caches_and_reports_info() {
        let dir = temp_dir("open");
        let zip = make_zip(&dir, &["a.txt", "b/c.txt", "b/d/e.txt"]);
        let state = AppState::new();
        let info = state.open_archive(&zip, None, None).unwrap();
        assert_eq!(info.format, "zip");
        assert_eq!(info.name, "test.zip");
        assert!(info.volumes.is_none());
        assert!(info.entry_count >= 3, "files (and maybe dirs) listed");

        // Root level: the single "src" directory.
        let page = state.list_entries(info.id, 0, 500, "", None).unwrap();
        assert_eq!(page.total, 1);
        assert_eq!(page.items[0].path, "src/");
        assert_eq!(page.items[0].entry_type, "dir");

        // src/: directory "b" sorts before file "a.txt".
        let page = state.list_entries(info.id, 0, 500, "src/", None).unwrap();
        let names: Vec<&str> = page.items.iter().map(|e| e.display.as_str()).collect();
        assert_eq!(names, vec!["b", "a.txt"]);
        assert_eq!(page.items[0].path, "src/b/");
        assert_eq!(page.items[1].path, "src/a.txt");

        // Unknown handle is a structured error.
        assert!(state.list_entries(999, 0, 500, "", None).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn supported_formats_include_sqz_container() {
        let state = AppState::new();
        let formats = state.engine.supported_formats();
        let sqz = formats.iter().find(|f| f.id == "sqz").expect("sqz format");
        assert_eq!(sqz.extensions, vec!["sqz"]);
        assert!(sqz.capabilities.can_create);
        assert!(sqz.capabilities.can_extract);
        assert!(sqz.capabilities.can_test);
        assert!(sqz.capabilities.can_split);
        let rar = formats.iter().find(|f| f.id == "rar").expect("rar format");
        assert_eq!(rar.extensions, vec!["rar", "cbr"]);
        assert!(!rar.capabilities.can_create);
        assert!(rar.capabilities.can_extract);
        assert!(rar.capabilities.can_test);
    }

    #[test]
    fn open_archive_reports_legacy_encoding_diagnostics() {
        let dir = temp_dir("encoding-info");
        let zip = make_raw_name_zip(
            &dir,
            &[
                209, 185, 203, 245, 206, 196, 188, 254, 214, 208, 206, 196, 195, 251, 179, 198,
                178, 226, 202, 212, 46, 116, 120, 116,
            ],
        );
        let state = AppState::new();
        let info = state.open_archive(&zip, None, None).unwrap();
        assert_eq!(info.legacy_encoding_count, 1);
        assert_eq!(info.garbled_count, 0);
        assert_eq!(info.suggested_encoding.as_deref(), Some("GBK"));
        assert!(info.encoding_override.is_none());

        let info = state.open_archive(&zip, None, Some("gbk")).unwrap();
        assert_eq!(info.encoding_override.as_deref(), Some("gbk"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn open_archive_reuses_session_password_cache() {
        let dir = temp_dir("session-password");
        let archive = make_header_encrypted_7z(&dir);
        let state = AppState::new();

        let err = state.open_archive(&archive, None, None).unwrap_err();
        assert!(matches!(err, FormatError::PasswordRequired), "{err:?}");

        let info = state.open_archive(&archive, Some("secret"), None).unwrap();
        assert_eq!(info.format, "7z");
        assert_eq!(
            state.password_for(&archive).as_ref().map(Password::expose),
            Some("secret")
        );

        let reopened = state.open_archive(&archive, None, None).unwrap();
        assert_eq!(reopened.format, "7z");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn cache_locks_recover_after_poison() {
        let state = std::sync::Arc::new(AppState::new());

        let archive_state = std::sync::Arc::clone(&state);
        assert!(std::thread::spawn(move || {
            let _guard = archive_state.archives.lock().unwrap();
            panic!("poison archive cache");
        })
        .join()
        .is_err());
        assert!(state.list_entries(404, 0, 10, "", None).is_err());
        state.close_archive(404);

        let password_state = std::sync::Arc::clone(&state);
        assert!(std::thread::spawn(move || {
            let _guard = password_state.passwords.lock().unwrap();
            panic!("poison password cache");
        })
        .join()
        .is_err());
        let archive = PathBuf::from("/tmp/squallz-poison-password.7z");
        state.remember_password(&archive, "secret");
        assert_eq!(
            state.password_for(&archive).as_ref().map(Password::expose),
            Some("secret")
        );
        state.forget_password(&archive);
        assert!(state.password_for(&archive).is_none());
    }

    #[test]
    fn list_entries_paginates_and_filters() {
        let dir = temp_dir("paging");
        let names: Vec<String> = (0..23).map(|i| format!("f{i:02}.txt")).collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let zip = make_zip(&dir, &refs);
        let state = AppState::new();
        let info = state.open_archive(&zip, None, None).unwrap();

        // Page size 10 → pages of 10/10/3 under src/.
        let p0 = state.list_entries(info.id, 0, 10, "src/", None).unwrap();
        assert_eq!((p0.total, p0.items.len()), (23, 10));
        assert_eq!(p0.items[0].display, "f00.txt");
        let p2 = state.list_entries(info.id, 2, 10, "src/", None).unwrap();
        assert_eq!(p2.items.len(), 3);
        assert_eq!(p2.items[2].display, "f22.txt");
        // Out-of-range page is empty, not an error.
        let p9 = state.list_entries(info.id, 9, 10, "src/", None).unwrap();
        assert!(p9.items.is_empty());

        // Filter matches the base name, case-insensitively.
        let f = state
            .list_entries(info.id, 0, 10, "src/", Some("F1"))
            .unwrap();
        assert_eq!(f.total, 10); // f10..f19
        assert!(f.items.iter().all(|e| e.display.starts_with("f1")));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn levels_synthesize_intermediate_dirs() {
        // An archive listing only a deep file must still expose every level.
        let metas = vec![EntryMeta {
            path: squallz_core::api::EntryPath::from_utf8("a/b/c.txt"),
            entry_type: EntryType::File,
            size: 3,
            compressed_size: None,
            modified: None,
            unix_mode: None,
            crc32: None,
            encrypted: false,
        }];
        let levels = build_levels(&metas);
        assert_eq!(levels.get("").unwrap().len(), 1);
        assert!(levels.get("").unwrap()[0].is_dir);
        assert_eq!(levels.get("a/").unwrap()[0].name, "b");
        assert_eq!(levels.get("a/b/").unwrap()[0].name, "c.txt");
    }
}
