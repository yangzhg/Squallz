//! Extraction safety primitives shared by every archive format:
//! path sanitization (Zip Slip), Windows portability checks and
//! decompression-bomb accounting (PLAN.md §2.3).

use std::path::{Component, Path, PathBuf};

use crate::entry::{EntryMeta, EntryPath};
use crate::error::FormatError;
use crate::options::SafetyLimits;

/// Entries smaller than this never trigger the compression-ratio check;
/// tiny files legitimately reach extreme ratios (e.g. a 4-byte file stored
/// in a 2-byte deflate stream) and would cause false positives.
const RATIO_CHECK_MIN_SIZE: u64 = 1024 * 1024; // 1 MiB

/// Sanitizes an archive entry path into a safe relative [`PathBuf`].
///
/// Rejected with [`FormatError::PathTraversal`]:
/// - absolute paths (`/etc/passwd`, `\\evil`),
/// - `..` components,
/// - Windows drive prefixes (`C:\...`, `C:foo`).
///
/// Both `/` and `\` are treated as separators (archives written on Windows
/// often contain backslashes). Empty and `.` components are dropped. An
/// entry that sanitizes to nothing yields [`FormatError::UnsafeFileName`].
pub fn sanitize_entry_path(path: &EntryPath) -> Result<PathBuf, FormatError> {
    let name = path.display.replace('\\', "/");
    if name.starts_with('/') {
        return Err(FormatError::PathTraversal(path.display.clone()));
    }
    let mut out = PathBuf::new();
    for comp in name.split('/') {
        match comp {
            "" | "." => continue,
            ".." => return Err(FormatError::PathTraversal(path.display.clone())),
            c => {
                // A drive prefix like `C:` smuggled into any component would
                // become absolute on Windows.
                if c.len() >= 2 && c.as_bytes()[1] == b':' && c.as_bytes()[0].is_ascii_alphabetic()
                {
                    return Err(FormatError::PathTraversal(path.display.clone()));
                }
                out.push(c);
            }
        }
    }
    // Double-check with std's component model: the result must be purely
    // relative and free of parent references.
    if out.as_os_str().is_empty() {
        return Err(FormatError::UnsafeFileName(path.display.clone()));
    }
    if out.components().any(|c| !matches!(c, Component::Normal(_))) {
        return Err(FormatError::PathTraversal(path.display.clone()));
    }
    Ok(out)
}

/// Checks a single path component for Windows portability hazards:
/// reserved device names (`CON`, `NUL`, `COM1`...), illegal characters
/// (`< > : " | ? *`, control chars, which also covers NTFS ADS via `:`),
/// and trailing dots/spaces.
///
/// Not enforced on macOS/Linux extraction; the Windows extraction path
/// enables it (escaping/renaming strategy lands together with the Windows
/// port).
pub fn check_windows_portability(component: &str) -> Result<(), FormatError> {
    const RESERVED: &[&str] = &[
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    if component.is_empty() {
        return Err(FormatError::UnsafeFileName(component.to_string()));
    }
    // Reserved names apply to the stem (`CON.txt` is also reserved).
    let stem = windows_reserved_stem(component);
    if RESERVED.iter().any(|r| stem.eq_ignore_ascii_case(r)) {
        return Err(FormatError::UnsafeFileName(component.to_string()));
    }
    if component
        .chars()
        .any(|ch| matches!(ch, '<' | '>' | ':' | '"' | '|' | '?' | '*') || (ch as u32) < 0x20)
    {
        return Err(FormatError::UnsafeFileName(component.to_string()));
    }
    if component.ends_with('.') || component.ends_with(' ') {
        return Err(FormatError::UnsafeFileName(component.to_string()));
    }
    Ok(())
}

fn windows_reserved_stem(component: &str) -> &str {
    let mut stem = component;
    if let Some((before_dot, _after_dot)) = component.split_once('.') {
        stem = before_dot;
    }
    stem
}

/// Tracks cumulative output bytes and entry counts against
/// [`SafetyLimits`] during one extraction run.
#[derive(Debug)]
pub struct LimitsAccountant {
    limits: SafetyLimits,
    entries: u64,
    output_bytes: u64,
}

impl LimitsAccountant {
    /// Starts a fresh accounting run.
    pub fn new(limits: SafetyLimits) -> Self {
        Self {
            limits,
            entries: 0,
            output_bytes: 0,
        }
    }

    /// Registers one entry: bumps the entry counter and runs the per-entry
    /// compression-ratio check (only when the compressed size is known and
    /// the entry is larger than 1 MiB, to avoid small-file false positives).
    pub fn check_entry(&mut self, meta: &EntryMeta) -> Result<(), FormatError> {
        self.entries += 1;
        if self.entries > self.limits.max_entries {
            return Err(FormatError::ResourceLimitExceeded(format!(
                "entry count exceeds limit of {}",
                self.limits.max_entries
            )));
        }
        if meta.size > RATIO_CHECK_MIN_SIZE {
            if let Some(compressed) = meta.compressed_size {
                let ratio = meta.size / compressed.max(1);
                if ratio > u64::from(self.limits.max_compression_ratio) {
                    return Err(FormatError::ResourceLimitExceeded(format!(
                        "entry '{}' compression ratio {} exceeds limit of {}",
                        meta.path, ratio, self.limits.max_compression_ratio
                    )));
                }
            }
        }
        Ok(())
    }

    /// Registers actually written output bytes; fails once the cumulative
    /// total crosses `max_output_bytes`.
    pub fn add_output_bytes(&mut self, n: u64) -> Result<(), FormatError> {
        self.output_bytes = self.output_bytes.saturating_add(n);
        if self.output_bytes > self.limits.max_output_bytes {
            return Err(FormatError::ResourceLimitExceeded(format!(
                "output bytes exceed limit of {}",
                self.limits.max_output_bytes
            )));
        }
        Ok(())
    }

    /// Cumulative output bytes written so far.
    pub fn output_bytes(&self) -> u64 {
        self.output_bytes
    }
}

/// Returns `true` when `path` (relative, sanitized) has any ancestor
/// directory listed in `symlinks` — i.e. writing through it would traverse a
/// symlink created earlier in this run.
pub(crate) fn crosses_created_symlink(
    path: &Path,
    symlinks: &std::collections::HashSet<PathBuf>,
) -> bool {
    let mut prefix = PathBuf::new();
    for comp in path.components() {
        prefix.push(comp);
        // The full path itself is allowed to *be* the symlink (overwrite
        // handling decides); only strict ancestors are traversal.
        if prefix != path && symlinks.contains(&prefix) {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entry::EntryType;

    fn meta(name: &str, size: u64, compressed: Option<u64>) -> EntryMeta {
        EntryMeta {
            path: EntryPath::from_utf8(name),
            entry_type: EntryType::File,
            size,
            compressed_size: compressed,
            modified: None,
            unix_mode: None,
            crc32: None,
            encrypted: false,
        }
    }

    #[test]
    fn sanitize_accepts_normal_paths() {
        let p = sanitize_entry_path(&EntryPath::from_utf8("a/b/中文.txt")).unwrap();
        assert_eq!(p, PathBuf::from("a/b/中文.txt"));
        // Redundant separators and dots collapse.
        let p = sanitize_entry_path(&EntryPath::from_utf8("a//./b.txt")).unwrap();
        assert_eq!(p, PathBuf::from("a/b.txt"));
    }

    #[test]
    fn sanitize_rejects_traversal_and_absolute() {
        for bad in [
            "../evil.txt",
            "a/../../evil",
            "/etc/passwd",
            "\\evil",
            "C:\\evil",
            "c:evil",
        ] {
            let err = sanitize_entry_path(&EntryPath::from_utf8(bad)).unwrap_err();
            assert!(
                matches!(err, FormatError::PathTraversal(_)),
                "{bad} should be PathTraversal, got {err:?}"
            );
        }
        let err = sanitize_entry_path(&EntryPath::from_utf8("././")).unwrap_err();
        assert!(matches!(err, FormatError::UnsafeFileName(_)));
    }

    #[test]
    fn windows_portability_checks() {
        assert!(check_windows_portability("normal.txt").is_ok());
        for bad in [
            "CON", "con.txt", "NUL", "lpt1.log", "a:b", "x?y", "dot.", "space ",
        ] {
            assert!(
                check_windows_portability(bad).is_err(),
                "{bad} should be rejected"
            );
        }
    }

    #[test]
    fn windows_reserved_stem_uses_text_before_first_dot() {
        assert_eq!(windows_reserved_stem("CON.txt"), "CON");
        assert_eq!(windows_reserved_stem("COM9.backup.zip"), "COM9");
        assert_eq!(windows_reserved_stem("normal"), "normal");
        assert_eq!(windows_reserved_stem(".profile"), "");

        assert!(check_windows_portability("COM9.backup.zip").is_err());
        assert!(check_windows_portability(".profile").is_ok());
    }

    #[test]
    fn accountant_entry_and_output_limits() {
        let mut acc = LimitsAccountant::new(SafetyLimits {
            max_output_bytes: 100,
            max_entries: 2,
            max_compression_ratio: 10,
        });
        assert!(acc.check_entry(&meta("a", 1, Some(1))).is_ok());
        assert!(acc.check_entry(&meta("b", 1, Some(1))).is_ok());
        assert!(matches!(
            acc.check_entry(&meta("c", 1, Some(1))),
            Err(FormatError::ResourceLimitExceeded(_))
        ));
        assert!(acc.add_output_bytes(100).is_ok());
        assert!(matches!(
            acc.add_output_bytes(1),
            Err(FormatError::ResourceLimitExceeded(_))
        ));
    }

    #[test]
    fn accountant_ratio_check_skips_small_files() {
        let mut acc = LimitsAccountant::new(SafetyLimits {
            max_output_bytes: u64::MAX,
            max_entries: u64::MAX,
            max_compression_ratio: 100,
        });
        // Small file with an extreme ratio: allowed.
        assert!(acc.check_entry(&meta("small", 4096, Some(2))).is_ok());
        // Large file with an extreme ratio: rejected.
        assert!(matches!(
            acc.check_entry(&meta("bomb", 10 * 1024 * 1024 * 1024, Some(1024))),
            Err(FormatError::ResourceLimitExceeded(_))
        ));
    }
}
