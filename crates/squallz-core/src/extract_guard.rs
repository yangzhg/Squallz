use std::collections::HashSet;
use std::ffi::OsStr;
use std::fmt;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{de, Deserialize, Deserializer, Serialize, Serializer};

use crate::api::{ControlToken, EntryMeta, EntryPath, EntryType, FormatError};
use crate::filesystem_identity::{
    file_identity, open_regular_file_no_follow, path_identity, RegularFileState,
};

const TOKEN_PREFIX: &str = "sqeg1_";
const TOKEN_BYTES: usize = 32;
const CONTROL_CHECKPOINT_INTERVAL: usize = 256;

/// Stable state of the physical source members discovered for one archive.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ArchiveSourceState {
    digest: [u8; TOKEN_BYTES],
}

impl fmt::Debug for ArchiveSourceState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ArchiveSourceState([redacted])")
    }
}

/// Opaque binding between an extraction preflight and its observed input.
///
/// The guard covers the physical archive source set, the complete entry
/// metadata list and the exact selected scope. It deliberately does not read
/// every payload byte during preflight.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ExtractInputGuard {
    digest: [u8; TOKEN_BYTES],
}

impl fmt::Debug for ExtractInputGuard {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ExtractInputGuard([redacted])")
    }
}

impl Serialize for ExtractInputGuard {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&encode_guard(self.digest))
    }
}

impl<'de> Deserialize<'de> for ExtractInputGuard {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        decode_guard(&value).map_err(de::Error::custom)
    }
}

pub(crate) fn inspect_archive_source_state(
    members: &[PathBuf],
    control: &ControlToken,
) -> Result<ArchiveSourceState, FormatError> {
    if members.is_empty() {
        return Err(FormatError::CorruptArchive(
            "archive source set has no members".into(),
        ));
    }

    let mut hasher = blake3::Hasher::new();
    hasher.update(b"squallz-extract-source-state-v1\0");
    hasher.update(&(members.len() as u64).to_le_bytes());
    for member in members {
        control.checkpoint()?;
        update_os_str(&mut hasher, member.as_os_str());
        let file = open_regular_file_no_follow(member)?;
        let metadata = file.metadata()?;
        let identity = file_identity(&file)?;
        let state = RegularFileState::from_metadata(&metadata);
        identity.update_digest(&mut hasher);
        state.update_digest(&mut hasher);

        let current_metadata = fs::symlink_metadata(member)?;
        if path_identity(member)? != identity || !state.matches(&current_metadata) {
            return Err(FormatError::input_changed());
        }
    }
    control.checkpoint()?;
    Ok(ArchiveSourceState {
        digest: *hasher.finalize().as_bytes(),
    })
}

pub fn build_extract_input_guard(
    source: ArchiveSourceState,
    entries: &[EntryMeta],
    selection: Option<&[EntryPath]>,
    control: &ControlToken,
) -> Result<ExtractInputGuard, FormatError> {
    let wanted = selection.map(|paths| {
        paths
            .iter()
            .map(|path| path.raw.as_slice())
            .collect::<HashSet<_>>()
    });
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"squallz-extract-input-guard-v1\0");
    hasher.update(&source.digest);
    hasher.update(&[u8::from(selection.is_some())]);
    hasher.update(&(entries.len() as u64).to_le_bytes());

    for (index, entry) in entries.iter().enumerate() {
        if index % CONTROL_CHECKPOINT_INTERVAL == 0 {
            control.checkpoint()?;
        }
        update_bytes(&mut hasher, &entry.path.raw);
        update_bytes(&mut hasher, entry.path.display.as_bytes());
        update_bytes(&mut hasher, entry.path.encoding.as_bytes());
        update_entry_type(&mut hasher, &entry.entry_type);
        hasher.update(&entry.size.to_le_bytes());
        update_option_u64(&mut hasher, entry.compressed_size);
        update_system_time(&mut hasher, entry.modified);
        update_option_u32(&mut hasher, entry.unix_mode);
        update_option_u32(&mut hasher, entry.crc32);
        hasher.update(&[u8::from(entry.encrypted)]);
        let selected = wanted
            .as_ref()
            .is_none_or(|paths| paths.contains(entry.path.raw.as_slice()));
        hasher.update(&[u8::from(selected)]);
    }
    control.checkpoint()?;
    Ok(ExtractInputGuard {
        digest: *hasher.finalize().as_bytes(),
    })
}

fn update_entry_type(hasher: &mut blake3::Hasher, entry_type: &EntryType) {
    match entry_type {
        EntryType::File => {
            hasher.update(&[1]);
        }
        EntryType::Dir => {
            hasher.update(&[2]);
        }
        EntryType::Symlink { target } => {
            hasher.update(&[3]);
            update_bytes(hasher, target);
        }
        EntryType::Hardlink { target } => {
            hasher.update(&[4]);
            update_bytes(hasher, target);
        }
        EntryType::Other => {
            hasher.update(&[5]);
        }
    }
}

fn update_bytes(hasher: &mut blake3::Hasher, value: &[u8]) {
    hasher.update(&(value.len() as u64).to_le_bytes());
    hasher.update(value);
}

fn update_option_u64(hasher: &mut blake3::Hasher, value: Option<u64>) {
    match value {
        Some(value) => {
            hasher.update(&[1]);
            hasher.update(&value.to_le_bytes());
        }
        None => {
            hasher.update(&[0]);
        }
    }
}

fn update_option_u32(hasher: &mut blake3::Hasher, value: Option<u32>) {
    match value {
        Some(value) => {
            hasher.update(&[1]);
            hasher.update(&value.to_le_bytes());
        }
        None => {
            hasher.update(&[0]);
        }
    }
}

fn update_system_time(hasher: &mut blake3::Hasher, value: Option<SystemTime>) {
    let Some(value) = value else {
        hasher.update(&[0]);
        return;
    };
    match value.duration_since(UNIX_EPOCH) {
        Ok(duration) => {
            hasher.update(&[1]);
            hasher.update(&duration.as_secs().to_le_bytes());
            hasher.update(&duration.subsec_nanos().to_le_bytes());
        }
        Err(error) => {
            let duration = error.duration();
            hasher.update(&[2]);
            hasher.update(&duration.as_secs().to_le_bytes());
            hasher.update(&duration.subsec_nanos().to_le_bytes());
        }
    }
}

#[cfg(unix)]
fn update_os_str(hasher: &mut blake3::Hasher, value: &OsStr) {
    use std::os::unix::ffi::OsStrExt;

    update_bytes(hasher, value.as_bytes());
}

#[cfg(windows)]
fn update_os_str(hasher: &mut blake3::Hasher, value: &OsStr) {
    use std::os::windows::ffi::OsStrExt;

    let units = value.encode_wide().collect::<Vec<_>>();
    hasher.update(&(units.len() as u64).to_le_bytes());
    for unit in units {
        hasher.update(&unit.to_le_bytes());
    }
}

#[cfg(not(any(unix, windows)))]
fn update_os_str(hasher: &mut blake3::Hasher, value: &OsStr) {
    update_bytes(hasher, value.to_string_lossy().as_bytes());
}

fn encode_guard(bytes: [u8; TOKEN_BYTES]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";

    let mut encoded = String::with_capacity(TOKEN_PREFIX.len() + TOKEN_BYTES * 2);
    encoded.push_str(TOKEN_PREFIX);
    for byte in bytes {
        encoded.push(char::from(HEX[(byte >> 4) as usize]));
        encoded.push(char::from(HEX[(byte & 0x0f) as usize]));
    }
    encoded
}

fn decode_guard(value: &str) -> Result<ExtractInputGuard, &'static str> {
    let Some(hex) = value.strip_prefix(TOKEN_PREFIX) else {
        return Err("unsupported extract input guard version");
    };
    if hex.len() != TOKEN_BYTES * 2 {
        return Err("invalid extract input guard length");
    }
    let mut digest = [0u8; TOKEN_BYTES];
    for (index, pair) in hex.as_bytes().as_chunks::<2>().0.iter().enumerate() {
        let high = decode_nibble(pair[0]).ok_or("invalid extract input guard encoding")?;
        let low = decode_nibble(pair[1]).ok_or("invalid extract input guard encoding")?;
        digest[index] = (high << 4) | low;
    }
    Ok(ExtractInputGuard { digest })
}

fn decode_nibble(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, size: u64) -> EntryMeta {
        EntryMeta {
            path: EntryPath::from_utf8(path),
            entry_type: EntryType::File,
            size,
            compressed_size: Some(size),
            modified: None,
            unix_mode: None,
            crc32: None,
            encrypted: false,
        }
    }

    #[test]
    fn input_guard_binds_metadata_and_selected_scope() {
        let source = ArchiveSourceState { digest: [7; 32] };
        let entries = vec![entry("one.txt", 1), entry("two.txt", 2)];
        let control = ControlToken::new();
        let all = build_extract_input_guard(source, &entries, None, &control).unwrap();
        let first = build_extract_input_guard(
            source,
            &entries,
            Some(std::slice::from_ref(&entries[0].path)),
            &control,
        )
        .unwrap();
        let mut changed_entries = entries.clone();
        changed_entries[0].size = 3;
        let changed = build_extract_input_guard(source, &changed_entries, None, &control).unwrap();

        assert_ne!(all, first);
        assert_ne!(all, changed);
    }

    #[test]
    fn input_guard_uses_a_versioned_redacted_wire_value() {
        let guard = ExtractInputGuard { digest: [0xab; 32] };
        let encoded = serde_json::to_string(&guard).unwrap();
        let decoded = serde_json::from_str::<ExtractInputGuard>(&encoded).unwrap();

        assert_eq!(decoded, guard);
        assert!(encoded.starts_with("\"sqeg1_"));
        assert_eq!(format!("{guard:?}"), "ExtractInputGuard([redacted])");
    }
}
