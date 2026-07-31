//! Format registry and detection (verified name hint, then magic numbers and
//! extension fallback), including compound-format recognition (`.tar.gz` and
//! friends).

use std::sync::Arc;

use crate::options::FormatCapabilities;
use crate::traits::{ArchiveFormat, Compressor};

/// Format information (for `sqz info` / the GUI).
#[derive(Debug, Clone)]
pub struct FormatInfo {
    /// Format identifier
    pub id: &'static str,
    /// Category
    pub kind: FormatKind,
    /// Extensions
    pub extensions: Vec<&'static str>,
    /// Capabilities (always the default for compressors)
    pub capabilities: FormatCapabilities,
}

/// Format category.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormatKind {
    /// Archive container
    Archive,
    /// Single-stream compressor
    Compressor,
}

/// Detection result.
#[derive(Clone)]
pub enum Detected {
    /// Archive container
    Archive(Arc<dyn ArchiveFormat>),
    /// Single-stream compression (possibly wrapping an inner archive, e.g.
    /// `.tar.gz`)
    Compressed {
        /// Outer compressor
        compressor: Arc<dyn Compressor>,
        /// Inner archive (`x.tar.gz` → tar; plain `x.gz` → `None`)
        inner_archive: Option<Arc<dyn ArchiveFormat>>,
    },
}

/// Format registry. Adding a format = implement the trait + register it in
/// squallz-formats; core/cli/gui stay untouched.
#[derive(Default)]
pub struct FormatRegistry {
    archives: Vec<Arc<dyn ArchiveFormat>>,
    compressors: Vec<Arc<dyn Compressor>>,
    /// Extension aliases expanding to a canonical compound suffix,
    /// e.g. `("tgz", "tar.gz")`.
    aliases: Vec<(&'static str, &'static str)>,
}

/// Splits a `.001`-style split-volume file name into its base name and the
/// volume index (`"x.zip.001"` → `("x.zip", 1)`). The suffix must be at
/// least three all-digit characters (the 7-Zip byte-split convention).
pub fn split_volume_name(filename: &str) -> Option<(&str, u32)> {
    let (base, suffix) = filename.rsplit_once('.')?;
    if base.is_empty() || suffix.len() < 3 || !suffix.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let index = suffix.parse().ok()?;
    if index == 0 {
        return None;
    }
    Some((base, index))
}

/// Splits a native ZIP data-volume name into its base name and one-based disk
/// number (`"x.z01"` → `("x", 1)`). The final disk uses `.zip` and is
/// validated by the ZIP format implementation before any sibling is trusted.
pub fn split_zip_volume_name(filename: &str) -> Option<(&str, u64)> {
    let (base, suffix) = filename.rsplit_once('.')?;
    let bytes = suffix.as_bytes();
    if base.is_empty()
        || bytes.len() < 3
        || !bytes[0].eq_ignore_ascii_case(&b'z')
        || !bytes[1..].iter().all(u8::is_ascii_digit)
    {
        return None;
    }
    let number = suffix[1..].parse().ok()?;
    if number == 0 {
        return None;
    }
    Some((base, number))
}

impl FormatRegistry {
    /// Empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers an archive format.
    pub fn register_archive(&mut self, f: Arc<dyn ArchiveFormat>) {
        self.archives.push(f);
    }

    /// Registers a single-stream compressor.
    pub fn register_compressor(&mut self, c: Arc<dyn Compressor>) {
        self.compressors.push(c);
    }

    /// Registers an extension alias: a file name ending in `.{alias}` is
    /// detected as if it ended in `.{canonical}` (e.g. `tgz` → `tar.gz`).
    pub fn register_alias(&mut self, alias: &'static str, canonical: &'static str) {
        self.aliases.push((alias, canonical));
    }

    /// Expands a registered alias suffix into its canonical form.
    fn canonical_name(&self, lower: &str) -> String {
        for (alias, canonical) in &self.aliases {
            if let Some(stem) = lower.strip_suffix(&format!(".{alias}")) {
                return format!("{stem}.{canonical}");
            }
        }
        lower.to_string()
    }

    /// Detects by file name (extension), correctly handling double
    /// extensions such as `.tar.gz`, registered aliases (`.tgz`) and
    /// generic or native ZIP split-volume suffixes (`x.zip.001` and `x.z01`
    /// both detect as ZIP candidates).
    pub fn detect_by_name(&self, filename: &str) -> Option<Detected> {
        let split_zip_name = split_zip_volume_name(filename).map(|(base, _)| format!("{base}.zip"));
        let stripped = split_volume_name(filename).map_or(filename, |(base, _)| base);
        let detected_name = split_zip_name.as_deref().unwrap_or(stripped);
        let lower = self.canonical_name(&detected_name.to_lowercase());
        // Archive extensions take priority (.zip/.7z/.tar never double as
        // compressor extensions).
        for f in &self.archives {
            for ext in f.extensions() {
                if lower.ends_with(&format!(".{ext}")) {
                    return Some(Detected::Archive(Arc::clone(f)));
                }
            }
        }
        // Compressor extensions: strip the suffix and check whether the
        // inner part is an archive (x.tar.gz → gz + tar).
        for c in &self.compressors {
            for ext in c.extensions() {
                if let Some(stem) = lower.strip_suffix(&format!(".{ext}")) {
                    let inner = self
                        .archives
                        .iter()
                        .find(|a| {
                            a.extensions()
                                .iter()
                                .any(|ae| stem.ends_with(&format!(".{ae}")))
                        })
                        .map(Arc::clone);
                    return Some(Detected::Compressed {
                        compressor: Arc::clone(c),
                        inner_archive: inner,
                    });
                }
            }
        }
        None
    }

    /// Combined detection: a name hint is accepted first only when its
    /// selected format also recognizes the bytes. This preserves compound
    /// streams such as `.tar.gz` without trusting a misleading suffix. Other
    /// magic matches follow, then the name remains the fallback for formats
    /// whose signatures are not available in the sniff window.
    pub fn detect(&self, name_hint: Option<&str>, head: &[u8], tail: &[u8]) -> Option<Detected> {
        let named = name_hint.and_then(|name| self.detect_by_name(name));
        let named_matches = named.as_ref().is_some_and(|detected| match detected {
            Detected::Archive(format) => format.sniff(head, tail),
            Detected::Compressed { compressor, .. } => compressor.sniff(head),
        });
        if named_matches {
            return named;
        }
        if let Some(f) = self.archives.iter().find(|f| f.sniff(head, tail)) {
            return Some(Detected::Archive(Arc::clone(f)));
        }
        if let Some(compressor) = self.compressors.iter().find(|c| c.sniff(head)) {
            return Some(Detected::Compressed {
                compressor: Arc::clone(compressor),
                inner_archive: None,
            });
        }
        named
    }

    /// Strips the recognized format suffix from a file name: split-volume
    /// suffix first, then alias/compound/archive/compressor extensions
    /// (`x.zip.001` → `x`, `x.z01` → `x`, `backup.tar.gz` → `backup`,
    /// `notes.tgz` → `notes`). Used to derive a folder name for smart
    /// extraction.
    pub fn display_stem(&self, filename: &str) -> String {
        if let Some((base, _)) = split_zip_volume_name(filename) {
            return base.to_owned();
        }
        let name = split_volume_name(filename).map_or(filename, |(base, _)| base);
        let lower = name.to_lowercase();
        // Aliases collapse a whole compound suffix at once (`.tgz`).
        for (alias, _) in &self.aliases {
            if let Some(stem) = lower.strip_suffix(&format!(".{alias}")) {
                return name[..stem.len()].to_string();
            }
        }
        for f in &self.archives {
            for ext in f.extensions() {
                if let Some(stem) = lower.strip_suffix(&format!(".{ext}")) {
                    return name[..stem.len()].to_string();
                }
            }
        }
        for c in &self.compressors {
            for ext in c.extensions() {
                if let Some(stem) = lower.strip_suffix(&format!(".{ext}")) {
                    // Compound names lose the inner archive extension too
                    // (`backup.tar.gz` → `backup`).
                    let inner = &lower[..stem.len()];
                    for a in &self.archives {
                        for ae in a.extensions() {
                            if let Some(s2) = inner.strip_suffix(&format!(".{ae}")) {
                                return name[..s2.len()].to_string();
                            }
                        }
                    }
                    return name[..stem.len()].to_string();
                }
            }
        }
        name.to_string()
    }

    /// Information about every registered format.
    pub fn formats(&self) -> Vec<FormatInfo> {
        let mut out: Vec<FormatInfo> = self
            .archives
            .iter()
            .map(|f| FormatInfo {
                id: f.id(),
                kind: FormatKind::Archive,
                extensions: f.extensions().to_vec(),
                capabilities: f.capabilities(),
            })
            .collect();
        out.extend(self.compressors.iter().map(|c| FormatInfo {
            id: c.id(),
            kind: FormatKind::Compressor,
            extensions: c.extensions().to_vec(),
            // Single-file compress/decompress/test through the engine's
            // virtual single-entry archive, byte-split volumes through the
            // engine splitter; the rest never applies.
            capabilities: FormatCapabilities {
                can_create: true,
                can_extract: true,
                can_test: true,
                can_split: true,
                ..FormatCapabilities::default()
            },
        }));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_volume_name_accepts_positive_numeric_suffixes() {
        assert_eq!(
            split_volume_name("archive.zip.001"),
            Some(("archive.zip", 1))
        );
        assert_eq!(
            split_volume_name("archive.tar.gz.010"),
            Some(("archive.tar.gz", 10))
        );
        assert_eq!(
            split_volume_name("archive.zip.0001"),
            Some(("archive.zip", 1))
        );
    }

    #[test]
    fn split_volume_name_rejects_non_volume_suffixes() {
        assert_eq!(split_volume_name("archive.zip.000"), None);
        assert_eq!(split_volume_name("archive.zip.00a"), None);
        assert_eq!(split_volume_name("archive.zip.01"), None);
        assert_eq!(split_volume_name(".001"), None);
    }

    #[test]
    fn split_zip_volume_name_accepts_native_data_volumes() {
        assert_eq!(split_zip_volume_name("archive.z01"), Some(("archive", 1)));
        assert_eq!(
            split_zip_volume_name("archive.tar.Z100"),
            Some(("archive.tar", 100))
        );
    }

    #[test]
    fn split_zip_volume_name_rejects_non_native_names() {
        assert_eq!(split_zip_volume_name("archive.z00"), None);
        assert_eq!(split_zip_volume_name("archive.z1"), None);
        assert_eq!(split_zip_volume_name("archive.za1"), None);
        assert_eq!(split_zip_volume_name(".z01"), None);
    }
}
