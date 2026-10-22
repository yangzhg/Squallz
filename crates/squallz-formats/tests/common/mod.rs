//! Shared helpers for the ZIP integration tests. All fixtures are generated
//! in code; no binary files are checked into the repository.
#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};

use squallz_core::api::{ControlToken, EntryMeta, FormatError, OpenOptions, SafetyLimits};
use squallz_core::Engine;

/// Unique-per-call temp directory removed on drop.
pub struct TempDir(pub PathBuf);

impl TempDir {
    pub fn new(tag: &str) -> Self {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "squallz-zip-test-{tag}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Engine wired with the built-in registry.
pub fn engine() -> Engine {
    Engine::new(squallz_formats::registry())
}

/// Reads fixture entries through the full engine listing API with default limits.
pub fn read_archive_entries(
    engine: &Engine,
    path: &Path,
    options: &OpenOptions,
) -> Result<Vec<EntryMeta>, FormatError> {
    engine
        .list_archive(
            path,
            options,
            SafetyLimits::default().max_entries,
            &ControlToken::default(),
        )
        .map(|listing| listing.entries)
}

/// Returns true when `cmd` exists on PATH (used to skip interop tests on
/// machines without the system zip/unzip tools).
pub fn command_exists(cmd: &str) -> bool {
    Command::new("which")
        .arg(cmd)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

pub mod raw_zip;

/// One entry of a handcrafted ZIP: raw name bytes (deliberately not
/// validated — that is the point) and stored (uncompressed) content.
pub struct RawZipEntry {
    pub name: Vec<u8>,
    pub data: Vec<u8>,
}

/// Builds a minimal stored ZIP byte-by-byte, bypassing any writer-side name
/// sanitization. The general-purpose flags stay 0, so names are *not*
/// marked UTF-8 (required for the legacy-encoding tests).
pub fn build_stored_zip(entries: &[RawZipEntry]) -> Vec<u8> {
    let entries: Vec<_> = entries
        .iter()
        .map(|entry| (entry.name.as_slice(), entry.data.as_slice()))
        .collect();
    raw_zip::build_stored_zip(&entries)
}
