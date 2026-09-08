//! GUI settings persisted to `<config_dir>/Squallz/settings.json`
//! (macOS: `~/Library/Application Support/Squallz/settings.json`).

use std::ffi::OsString;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use crate::dto::SettingsDto;
use squallz_core::lock_unpoisoned;

static SETTINGS_TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Settings store: an in-memory copy guarded by a mutex, written through on
/// every change.
pub struct SettingsStore {
    path: Option<PathBuf>,
    current: Mutex<SettingsDto>,
}

fn read_settings(path: Option<&Path>) -> SettingsDto {
    let Some(path) = path else {
        return SettingsDto::default();
    };
    let Ok(json) = std::fs::read_to_string(path) else {
        return SettingsDto::default();
    };
    let Ok(settings) = serde_json::from_str(&json) else {
        return SettingsDto::default();
    };
    settings
}

impl SettingsStore {
    /// Loads the settings file (missing or invalid files yield defaults).
    pub fn load() -> Self {
        let path = dirs::config_dir().map(|d| d.join("Squallz").join("settings.json"));
        Self::load_from_path(path)
    }

    fn load_from_path(path: Option<PathBuf>) -> Self {
        let current = read_settings(path.as_deref());
        Self {
            path,
            current: Mutex::new(current),
        }
    }

    /// Current settings snapshot.
    pub fn get(&self) -> SettingsDto {
        lock_unpoisoned(&self.current).clone()
    }

    /// Persists a settings update before publishing it to the in-memory
    /// snapshot. Callers can therefore distinguish a saved preference from a
    /// preview that only exists in the frontend.
    pub fn update(&self, f: impl FnOnce(&mut SettingsDto)) -> io::Result<SettingsDto> {
        let mut current = lock_unpoisoned(&self.current);
        let mut next = current.clone();
        f(&mut next);
        if let Some(path) = &self.path {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let json = serde_json::to_string_pretty(&next)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
            write_settings_atomically(path, json.as_bytes())?;
        }
        *current = next.clone();
        Ok(next)
    }
}

fn write_settings_atomically(path: &Path, contents: &[u8]) -> io::Result<()> {
    let file_name = path.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "settings path must include a file name",
        )
    })?;
    let sequence = SETTINGS_TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let mut temp_name = OsString::from(".");
    temp_name.push(file_name);
    temp_name.push(format!(".tmp-{}-{sequence}", std::process::id()));
    let temp_path = path.with_file_name(temp_name);
    write_settings_with_temp_path(path, &temp_path, contents)
}

fn write_settings_with_temp_path(path: &Path, temp_path: &Path, contents: &[u8]) -> io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }

    let mut temp_file = options.open(temp_path)?;
    let write_result = temp_file
        .write_all(contents)
        .and_then(|()| temp_file.sync_all());
    drop(temp_file);

    if let Err(error) = write_result {
        let _ = std::fs::remove_file(temp_path);
        return Err(error);
    }
    if let Err(error) = replace_settings_file(temp_path, path) {
        let _ = std::fs::remove_file(temp_path);
        return Err(error);
    }
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn replace_settings_file(temp_path: &Path, path: &Path) -> io::Result<()> {
    std::fs::rename(temp_path, path)
}

#[cfg(target_os = "windows")]
fn replace_settings_file(temp_path: &Path, path: &Path) -> io::Result<()> {
    use std::iter;
    use std::os::windows::ffi::OsStrExt;

    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };

    fn wide_path(path: &Path) -> io::Result<Vec<u16>> {
        let mut value: Vec<u16> = path.as_os_str().encode_wide().collect();
        if value.contains(&0) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "settings path contains a null character",
            ));
        }
        value.extend(iter::once(0));
        Ok(value)
    }

    let temp_path = wide_path(temp_path)?;
    let path = wide_path(path)?;
    // SAFETY: both pointers reference live, null-terminated UTF-16 buffers for
    // the duration of this synchronous Windows API call.
    let replaced = unsafe {
        MoveFileExW(
            temp_path.as_ptr(),
            path.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if replaced == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{write_settings_atomically, write_settings_with_temp_path, SettingsStore};

    fn temp_settings_path(name: &str) -> std::path::PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "squallz-settings-{name}-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        path
    }

    #[test]
    fn settings_store_persists_updates_and_reloads() {
        let path = temp_settings_path("persist");
        let store = SettingsStore::load_from_path(Some(path.clone()));

        let saved = store
            .update(|settings| {
                settings.theme = Some("dark".into());
                settings.language = Some("en-US".into());
                settings.ui_mode = Some("modern".into());
                settings.ui_density = Some("compact".into());
                settings.accent_palette = Some("custom".into());
                settings.custom_accent = Some("#D946EF".into());
                settings.accent_contrast_guard = Some(false);
                settings.default_extract_dir = Some("/tmp/Squallz Extracts".into());
                settings.default_create_dir = Some("/tmp/Squallz Archives".into());
                settings.check_updates_automatically = Some(false);
                settings.safety_max_output_bytes = Some(4096);
                settings.safety_max_entries = Some(17);
                settings.safety_max_compression_ratio = Some(9);
                settings.performance_threads = Some(8);
                settings.performance_memory_limit_bytes = Some(128 * 1024 * 1024);
                settings.performance_parallel_jobs = Some(3);
            })
            .expect("settings update should persist");

        assert_eq!(saved.theme.as_deref(), Some("dark"));
        assert_eq!(saved.ui_mode.as_deref(), Some("modern"));
        assert_eq!(saved.ui_density.as_deref(), Some("compact"));
        assert_eq!(saved.accent_palette.as_deref(), Some("custom"));
        assert_eq!(saved.custom_accent.as_deref(), Some("#D946EF"));
        assert_eq!(saved.accent_contrast_guard, Some(false));
        assert_eq!(
            saved.default_create_dir.as_deref(),
            Some("/tmp/Squallz Archives")
        );
        assert!(!saved.automatic_update_checks_enabled());
        assert_eq!(saved.safety_limits().max_output_bytes, 4096);
        assert_eq!(saved.safety_limits().max_entries, 17);
        assert_eq!(saved.safety_limits().max_compression_ratio, 9);
        assert_eq!(saved.resource_options().threads, Some(8));
        assert_eq!(
            saved.resource_options().memory_limit,
            Some(crate::dto::PERFORMANCE_STREAM_BUFFER_MAX_BYTES)
        );
        assert_eq!(saved.performance_parallel_jobs, Some(3));

        let disk = std::fs::read_to_string(&path).expect("settings should be written to disk");
        assert!(disk.contains("\"ui_mode\": \"modern\""), "{disk}");
        assert!(disk.contains("\"ui_density\": \"compact\""), "{disk}");
        assert!(disk.contains("\"accent_palette\": \"custom\""), "{disk}");
        assert!(disk.contains("\"custom_accent\": \"#D946EF\""), "{disk}");
        assert!(disk.contains("\"accent_contrast_guard\": false"), "{disk}");
        assert!(
            disk.contains("\"check_updates_automatically\": false"),
            "{disk}"
        );
        assert!(
            disk.contains("\"default_create_dir\": \"/tmp/Squallz Archives\""),
            "{disk}"
        );
        assert!(disk.contains("\"performance_threads\": 8"), "{disk}");
        assert!(disk.contains("\"performance_parallel_jobs\": 3"), "{disk}");

        let reloaded = SettingsStore::load_from_path(Some(path.clone())).get();
        assert_eq!(reloaded.theme.as_deref(), Some("dark"));
        assert_eq!(reloaded.language.as_deref(), Some("en-US"));
        assert_eq!(reloaded.ui_density.as_deref(), Some("compact"));
        assert_eq!(reloaded.accent_palette.as_deref(), Some("custom"));
        assert_eq!(reloaded.custom_accent.as_deref(), Some("#D946EF"));
        assert_eq!(reloaded.accent_contrast_guard, Some(false));
        assert_eq!(
            reloaded.default_create_dir.as_deref(),
            Some("/tmp/Squallz Archives")
        );
        assert!(!reloaded.automatic_update_checks_enabled());
        assert_eq!(reloaded.safety_limits().max_output_bytes, 4096);
        assert_eq!(reloaded.resource_options().threads, Some(8));
        assert_eq!(reloaded.performance_parallel_jobs, Some(3));

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn settings_store_invalid_json_uses_defaults_then_overwrites() {
        let path = temp_settings_path("invalid");
        std::fs::write(&path, "{not valid json").unwrap();

        let store = SettingsStore::load_from_path(Some(path.clone()));
        assert_eq!(store.get().ui_mode, None);
        assert_eq!(store.get().resource_options().threads, None);

        store
            .update(|settings| {
                settings.ui_mode = Some("classic".into());
                settings.performance_threads = Some(3);
            })
            .expect("invalid settings file should be replaced");

        let reloaded = SettingsStore::load_from_path(Some(path.clone())).get();
        assert_eq!(reloaded.ui_mode.as_deref(), Some("classic"));
        assert_eq!(reloaded.resource_options().threads, Some(3));

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn settings_store_recovers_after_current_lock_poison() {
        let path = temp_settings_path("poison");
        let store = SettingsStore::load_from_path(Some(path.clone()));

        let poison = std::panic::catch_unwind(|| {
            let mut current = store.current.lock().unwrap();
            current.theme = Some("light".into());
            current.performance_threads = Some(2);
            panic!("poison settings lock");
        });
        assert!(poison.is_err());

        let recovered = store.get();
        assert_eq!(recovered.theme.as_deref(), Some("light"));
        assert_eq!(recovered.resource_options().threads, Some(2));

        let saved = store
            .update(|settings| {
                settings.theme = Some("dark".into());
                settings.performance_threads = Some(4);
            })
            .expect("settings should persist after lock recovery");
        assert_eq!(saved.theme.as_deref(), Some("dark"));
        assert_eq!(saved.resource_options().threads, Some(4));

        let reloaded = SettingsStore::load_from_path(Some(path.clone())).get();
        assert_eq!(reloaded.theme.as_deref(), Some("dark"));
        assert_eq!(reloaded.resource_options().threads, Some(4));

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn settings_store_reports_write_failure_without_publishing_snapshot() {
        let parent = temp_settings_path("blocked-parent");
        std::fs::write(&parent, b"not a directory").expect("blocked parent fixture");
        let store = SettingsStore::load_from_path(Some(parent.join("settings.json")));

        let result = store.update(|settings| settings.theme = Some("dark".into()));

        assert!(result.is_err());
        assert_eq!(store.get().theme, None);
        let _ = std::fs::remove_file(parent);
    }

    #[test]
    fn atomic_settings_write_failure_preserves_existing_file() {
        let dir = temp_settings_path("atomic-preserve");
        std::fs::create_dir_all(&dir).expect("settings fixture directory");
        let path = dir.join("settings.json");
        let temp_path = dir.join("blocked-temp");
        let original = br#"{"theme":"light"}"#;
        std::fs::write(&path, original).expect("existing settings fixture");
        std::fs::create_dir(&temp_path).expect("blocked temp fixture");

        let result = write_settings_with_temp_path(&path, &temp_path, br#"{"theme":"dark"}"#);

        assert!(result.is_err());
        assert_eq!(
            std::fs::read(&path).expect("existing settings remain readable"),
            original
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn atomic_settings_write_replaces_existing_file() {
        let dir = temp_settings_path("atomic-replace");
        std::fs::create_dir_all(&dir).expect("settings fixture directory");
        let path = dir.join("settings.json");
        std::fs::write(&path, br#"{"theme":"light"}"#).expect("existing settings fixture");

        write_settings_atomically(&path, br#"{"theme":"dark"}"#)
            .expect("existing settings should be replaced atomically");

        assert_eq!(
            std::fs::read(&path).expect("replacement settings remain readable"),
            br#"{"theme":"dark"}"#
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
