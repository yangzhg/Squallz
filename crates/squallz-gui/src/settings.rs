//! GUI settings persisted to `<config_dir>/Squallz/settings.json`
//! (macOS: `~/Library/Application Support/Squallz/settings.json`).

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use crate::dto::SettingsDto;

/// Settings store: an in-memory copy guarded by a mutex, written through on
/// every change.
pub struct SettingsStore {
    path: Option<PathBuf>,
    current: Mutex<SettingsDto>,
}

fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
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

    /// Mutates and persists the settings. Write failures are logged, never
    /// surfaced (a read-only config dir must not break the session).
    pub fn update(&self, f: impl FnOnce(&mut SettingsDto)) -> SettingsDto {
        let mut current = lock_unpoisoned(&self.current);
        f(&mut current);
        if let Some(path) = &self.path {
            let write = || -> std::io::Result<()> {
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                let json = match serde_json::to_string_pretty(&*current) {
                    Ok(json) => json,
                    Err(error) => {
                        log::warn!("settings: cannot serialize {}: {error}", path.display());
                        return Ok(());
                    }
                };
                std::fs::write(path, json)
            };
            if let Err(e) = write() {
                log::warn!("settings: cannot persist {}: {e}", path.display());
            }
        }
        current.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::SettingsStore;

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

        let saved = store.update(|settings| {
            settings.theme = Some("dark".into());
            settings.language = Some("en-US".into());
            settings.ui_mode = Some("modern".into());
            settings.ui_density = Some("compact".into());
            settings.accent_palette = Some("custom".into());
            settings.custom_accent = Some("#D946EF".into());
            settings.accent_contrast_guard = Some(false);
            settings.safety_max_output_bytes = Some(4096);
            settings.safety_max_entries = Some(17);
            settings.safety_max_compression_ratio = Some(9);
            settings.performance_threads = Some(8);
            settings.performance_memory_limit_bytes = Some(128 * 1024 * 1024);
        });

        assert_eq!(saved.theme.as_deref(), Some("dark"));
        assert_eq!(saved.ui_mode.as_deref(), Some("modern"));
        assert_eq!(saved.ui_density.as_deref(), Some("compact"));
        assert_eq!(saved.accent_palette.as_deref(), Some("custom"));
        assert_eq!(saved.custom_accent.as_deref(), Some("#D946EF"));
        assert_eq!(saved.accent_contrast_guard, Some(false));
        assert_eq!(saved.safety_limits().max_output_bytes, 4096);
        assert_eq!(saved.safety_limits().max_entries, 17);
        assert_eq!(saved.safety_limits().max_compression_ratio, 9);
        assert_eq!(saved.resource_options().threads, Some(8));
        assert_eq!(
            saved.resource_options().memory_limit,
            Some(128 * 1024 * 1024)
        );

        let disk = std::fs::read_to_string(&path).expect("settings should be written to disk");
        assert!(disk.contains("\"ui_mode\": \"modern\""), "{disk}");
        assert!(disk.contains("\"ui_density\": \"compact\""), "{disk}");
        assert!(disk.contains("\"accent_palette\": \"custom\""), "{disk}");
        assert!(disk.contains("\"custom_accent\": \"#D946EF\""), "{disk}");
        assert!(disk.contains("\"accent_contrast_guard\": false"), "{disk}");
        assert!(disk.contains("\"performance_threads\": 8"), "{disk}");

        let reloaded = SettingsStore::load_from_path(Some(path.clone())).get();
        assert_eq!(reloaded.theme.as_deref(), Some("dark"));
        assert_eq!(reloaded.language.as_deref(), Some("en-US"));
        assert_eq!(reloaded.ui_density.as_deref(), Some("compact"));
        assert_eq!(reloaded.accent_palette.as_deref(), Some("custom"));
        assert_eq!(reloaded.custom_accent.as_deref(), Some("#D946EF"));
        assert_eq!(reloaded.accent_contrast_guard, Some(false));
        assert_eq!(reloaded.safety_limits().max_output_bytes, 4096);
        assert_eq!(reloaded.resource_options().threads, Some(8));

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn settings_store_invalid_json_uses_defaults_then_overwrites() {
        let path = temp_settings_path("invalid");
        std::fs::write(&path, "{not valid json").unwrap();

        let store = SettingsStore::load_from_path(Some(path.clone()));
        assert_eq!(store.get().ui_mode, None);
        assert_eq!(store.get().resource_options().threads, None);

        store.update(|settings| {
            settings.ui_mode = Some("classic".into());
            settings.performance_threads = Some(3);
        });

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

        let saved = store.update(|settings| {
            settings.theme = Some("dark".into());
            settings.performance_threads = Some(4);
        });
        assert_eq!(saved.theme.as_deref(), Some("dark"));
        assert_eq!(saved.resource_options().threads, Some(4));

        let reloaded = SettingsStore::load_from_path(Some(path.clone())).get();
        assert_eq!(reloaded.theme.as_deref(), Some("dark"));
        assert_eq!(reloaded.resource_options().threads, Some(4));

        let _ = std::fs::remove_file(path);
    }
}
