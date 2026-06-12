//! Entry model: paths (raw bytes as the source of truth), types and metadata.

use std::fmt;
use std::time::SystemTime;

/// Path of an entry inside an archive: raw bytes are the source of truth,
/// the display name is decoded per encoding.
///
/// Entry names in legacy archives (CP936/Shift-JIS) are not valid UTF-8;
/// forcing them into a `String` would lose information, so `raw` always
/// keeps the original bytes.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EntryPath {
    /// Raw bytes (the actual representation inside the format)
    pub raw: Vec<u8>,
    /// Display path decoded with `encoding` (always `/`-separated)
    pub display: String,
    /// Label of the encoding used for decoding, e.g. `"utf-8"`, `"GBK"`,
    /// `"Shift_JIS"`
    pub encoding: &'static str,
}

impl EntryPath {
    /// Builds from a UTF-8 string (the common case when creating archives).
    pub fn from_utf8(s: impl Into<String>) -> Self {
        let display = s.into();
        Self {
            raw: display.clone().into_bytes(),
            display,
            encoding: "utf-8",
        }
    }

    /// Builds from raw bytes plus a decoded display name (used when reading
    /// legacy archives).
    pub fn from_raw(raw: Vec<u8>, display: String, encoding: &'static str) -> Self {
        Self {
            raw,
            display,
            encoding,
        }
    }
}

impl fmt::Display for EntryPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.display)
    }
}

/// Entry type. Link targets also keep their raw bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntryType {
    /// Regular file
    File,
    /// Directory
    Dir,
    /// Symbolic link
    Symlink {
        /// Link target (raw bytes)
        target: Vec<u8>,
    },
    /// Hard link
    Hardlink {
        /// Link target (raw bytes)
        target: Vec<u8>,
    },
    /// Anything else (device files etc.; skipped on extraction by default)
    Other,
}

/// Metadata of a single archive entry.
#[derive(Debug, Clone)]
pub struct EntryMeta {
    /// Entry path
    pub path: EntryPath,
    /// Entry type
    pub entry_type: EntryType,
    /// Uncompressed size in bytes
    pub size: u64,
    /// Compressed size (not provided by every format)
    pub compressed_size: Option<u64>,
    /// Modification time
    pub modified: Option<SystemTime>,
    /// Unix permission bits (e.g. 0o755; `None` for non-Unix origins)
    pub unix_mode: Option<u32>,
    /// CRC32 checksum (when the format provides one)
    pub crc32: Option<u32>,
    /// Whether the content is encrypted
    pub encrypted: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_path_display() {
        let p = EntryPath::from_utf8("目录/文件.txt");
        assert_eq!(p.to_string(), "目录/文件.txt");
        assert_eq!(p.encoding, "utf-8");
    }

    #[test]
    fn entry_path_raw_bytes_remain_the_source_of_truth() {
        let raw = vec![0xc4, 0xe3, b'/', 0xce, 0xc4, b'.', b't', b'x', b't'];
        let p = EntryPath::from_raw(raw.clone(), "你/文.txt".to_string(), "GBK");

        assert_eq!(p.raw, raw);
        assert_eq!(p.to_string(), "你/文.txt");
        assert_eq!(p.encoding, "GBK");
    }
}
