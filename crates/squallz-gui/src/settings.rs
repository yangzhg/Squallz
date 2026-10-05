//! GUI settings persisted to `<config_dir>/Squallz/settings.json`
//! (macOS: `~/Library/Application Support/Squallz/settings.json`).

use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use crate::dto::SettingsDto;
use squallz_core::{
    api::PhysicalFileIdentity, lock_unpoisoned, open_new_artifact, physical_file_identity,
    physical_path_identity, replace_file_atomically,
};

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
    let mut temporary = SettingsTempFile::create(temp_path)?;
    temporary.file.write_all(contents)?;
    temporary.file.sync_all()?;
    temporary.publish(path)
}

/// Holds the created file open through publication and only cleans up its own
/// directory entry. Identity checks reject an observed path substitution;
/// pathname replacement still assumes a trusted settings directory.
struct SettingsTempFile {
    path: PathBuf,
    file: File,
    identity: PhysicalFileIdentity,
    cleanup: bool,
}

impl SettingsTempFile {
    fn create(path: PathBuf) -> io::Result<Self> {
        let file = open_new_artifact(&path)?;
        // If identity capture fails, leave the unverified pathname untouched.
        let identity = physical_file_identity(&file)?;
        let temporary = Self {
            path,
            file,
            identity,
            cleanup: true,
        };
        temporary.verify_ownership()?;
        Ok(temporary)
    }

    fn verify_ownership(&self) -> io::Result<()> {
        if physical_file_identity(&self.file)? != self.identity
            || physical_path_identity(&self.path)? != self.identity
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "settings temporary file changed before publication",
            ));
        }
        Ok(())
    }

    fn publish(mut self, destination: &Path) -> io::Result<()> {
        self.verify_ownership()?;
        replace_file_atomically(&self.path, destination)?;
        self.cleanup = false;
        Ok(())
    }
}

impl Drop for SettingsTempFile {
    fn drop(&mut self) {
        if self.cleanup
            && physical_file_identity(&self.file).ok() == Some(self.identity)
            && physical_path_identity(&self.path).ok() == Some(self.identity)
        {
            let _ = fs::remove_file(&self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{write_settings_atomically, SettingsStore, SettingsTempFile};
    use std::io::Write;

    #[test]
    fn settings_store_persists_updates_and_reloads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
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

        let start = std::sync::Barrier::new(2);
        std::thread::scope(|scope| {
            let appearance = scope.spawn(|| {
                start.wait();
                store.update(|settings| settings.theme = Some("light".into()))
            });
            let limits = scope.spawn(|| {
                start.wait();
                store.update(|settings| settings.safety_max_entries = Some(23))
            });
            appearance.join().unwrap().unwrap();
            limits.join().unwrap().unwrap();
        });
        let current = store.get();
        assert_eq!(current.theme.as_deref(), Some("light"));
        assert_eq!(current.safety_max_entries, Some(23));
        assert_eq!(
            serde_json::to_value(SettingsStore::load_from_path(Some(path)).get()).unwrap(),
            serde_json::to_value(current).unwrap()
        );
    }

    #[test]
    fn settings_store_invalid_json_uses_defaults_then_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
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
    }

    #[test]
    fn settings_store_recovers_after_current_lock_poison() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
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
    }

    #[test]
    fn settings_store_reports_write_failure_without_publishing_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let parent = dir.path().join("blocked-parent");
        std::fs::write(&parent, b"not a directory").expect("blocked parent fixture");
        let store = SettingsStore::load_from_path(Some(parent.join("settings.json")));

        let result = store.update(|settings| settings.theme = Some("dark".into()));

        assert!(result.is_err());
        assert_eq!(store.get().theme, None);
        assert_eq!(std::fs::read(&parent).unwrap(), b"not a directory");

        let path = dir.path().join("settings.json");
        let store = SettingsStore::load_from_path(Some(path.clone()));
        let saved = store
            .update(|settings| {
                settings.theme = Some("light".into());
                settings.performance_threads = Some(2);
            })
            .unwrap();
        let original = std::fs::read(&path).unwrap();
        let backup = dir.path().join("saved.json");
        std::fs::rename(&path, &backup).unwrap();
        std::fs::create_dir(&path).unwrap();
        let sentinel = path.join("keep");
        std::fs::write(&sentinel, b"keep directory contents").unwrap();

        let result = store.update(|settings| {
            settings.theme = Some("dark".into());
            settings.performance_threads = Some(4);
        });
        assert!(result.is_err(), "replacing a nonempty directory must fail");
        assert_eq!(
            serde_json::to_value(store.get()).unwrap(),
            serde_json::to_value(saved).unwrap()
        );
        assert_eq!(std::fs::read(&backup).unwrap(), original);
        assert_eq!(
            std::fs::read(&sentinel).unwrap(),
            b"keep directory contents"
        );
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 3);
    }

    #[test]
    fn atomic_settings_write_failure_preserves_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let temp_path = dir.path().join("settings.tmp");
        let original = br#"{"theme":"light"}"#;
        std::fs::write(&path, original).expect("existing settings fixture");
        std::fs::create_dir(&temp_path).expect("blocked temp fixture");

        assert!(SettingsTempFile::create(temp_path.clone()).is_err());
        assert!(temp_path.is_dir());
        std::fs::remove_dir(&temp_path).unwrap();
        std::fs::write(&temp_path, b"foreign candidate").unwrap();
        assert!(SettingsTempFile::create(temp_path.clone()).is_err());
        assert_eq!(std::fs::read(&temp_path).unwrap(), b"foreign candidate");
        std::fs::remove_file(&temp_path).unwrap();

        let mut temporary = SettingsTempFile::create(temp_path.clone()).unwrap();
        temporary.file.write_all(b"owned contents").unwrap();
        temporary.file.sync_all().unwrap();
        let retained = dir.path().join("retained.tmp");
        std::fs::rename(&temp_path, &retained).unwrap();
        std::fs::write(&temp_path, b"foreign replacement").unwrap();
        assert!(temporary.publish(&path).is_err());
        assert_eq!(std::fs::read(&temp_path).unwrap(), b"foreign replacement");
        assert_eq!(std::fs::read(&retained).unwrap(), b"owned contents");

        #[cfg(unix)]
        {
            let link_path = dir.path().join("link.tmp");
            let temporary = SettingsTempFile::create(link_path.clone()).unwrap();
            let retained_link = dir.path().join("retained-link.tmp");
            std::fs::rename(&link_path, &retained_link).unwrap();
            std::os::unix::fs::symlink(&temp_path, &link_path).unwrap();
            assert!(temporary.publish(&path).is_err());
            assert!(std::fs::symlink_metadata(&link_path)
                .unwrap()
                .file_type()
                .is_symlink());
            assert_eq!(std::fs::read(&temp_path).unwrap(), b"foreign replacement");
            assert_eq!(std::fs::read(&retained_link).unwrap(), b"");
        }
        assert_eq!(
            std::fs::read(&path).expect("existing settings remain readable"),
            original
        );
    }

    #[test]
    fn atomic_settings_write_replaces_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, br#"{"theme":"light"}"#).expect("existing settings fixture");

        write_settings_atomically(&path, br#"{"theme":"dark"}"#)
            .expect("existing settings should be replaced atomically");

        assert_eq!(
            std::fs::read(&path).expect("replacement settings remain readable"),
            br#"{"theme":"dark"}"#
        );
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );

            let referent = dir.path().join("referent.json");
            std::fs::rename(&path, &referent).unwrap();
            std::os::unix::fs::symlink(&referent, &path).unwrap();
            write_settings_atomically(&path, br#"{"theme":"light"}"#).unwrap();
            assert!(std::fs::symlink_metadata(&path).unwrap().is_file());
            assert_eq!(std::fs::read(&path).unwrap(), br#"{"theme":"light"}"#);
            assert_eq!(std::fs::read(&referent).unwrap(), br#"{"theme":"dark"}"#);
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
            assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
        }
    }
}
