//! Streaming parser for 7-Zip's technical listing output.

mod rar_compatibility;

pub(crate) use rar_compatibility::RarCompatibility;

use std::collections::BTreeMap;
use std::io::{self, BufRead};

use squallz_format_api::{EntryMeta, EntryPath, EntryType, FormatError};

use rar_compatibility::RarFacts;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct SevenZipArchiveProperties {
    pub(crate) multivolume: Option<bool>,
    pub(crate) volume_index: Option<u64>,
    pub(crate) volume_count: Option<u64>,
}

pub(crate) struct ParsedListing {
    pub(crate) entries: Vec<EntryMeta>,
    // Volume-property validation is independent of entry listing.
    pub(crate) archive: Result<SevenZipArchiveProperties, FormatError>,
    pub(crate) compatibility: RarCompatibility,
}

pub(crate) fn read(mut reader: impl BufRead) -> io::Result<ParsedListing> {
    let mut entries = Vec::new();
    let mut block = BTreeMap::new();
    let mut archive = Ok(None);
    let mut facts = RarFacts::default();
    let mut buffer = Vec::new();
    loop {
        buffer.clear();
        if reader.read_until(b'\n', &mut buffer)? == 0 {
            break;
        }
        if buffer.last() == Some(&b'\n') {
            buffer.pop();
        }
        let line = String::from_utf8_lossy(&buffer);
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            finish_block(&mut entries, &mut block, &mut archive);
            facts.finish_block();
        } else {
            let field = line.split_once(" = ");
            facts.observe_line(line, field);
            if let Some((key, value)) = field {
                block.insert(key.trim().to_owned(), value.to_owned());
            }
        }
    }
    finish_block(&mut entries, &mut block, &mut archive);
    facts.finish_block();
    infer_directory_entries(&mut entries);
    Ok(ParsedListing {
        entries,
        archive: archive.map(Option::unwrap_or_default),
        compatibility: facts.finish(),
    })
}

fn finish_block(
    entries: &mut Vec<EntryMeta>,
    block: &mut BTreeMap<String, String>,
    archive: &mut Result<Option<SevenZipArchiveProperties>, FormatError>,
) {
    if block.contains_key("Type") && block.contains_key("Physical Size") {
        if let Ok(previous) = archive {
            let properties = if previous.is_some() {
                Err(FormatError::CorruptArchive(
                    "7-Zip reported more than one archive metadata block".into(),
                ))
            } else {
                archive_properties(block)
            };
            *archive = properties.map(Some);
        }
    } else {
        push_list_block(entries, block);
    }
    block.clear();
}

fn archive_properties(
    block: &BTreeMap<String, String>,
) -> Result<SevenZipArchiveProperties, FormatError> {
    Ok(SevenZipArchiveProperties {
        multivolume: parse_flag(block, "Multivolume")?,
        volume_index: parse_u64(block, "Volume Index")?,
        volume_count: parse_u64(block, "Volumes")?,
    })
}

fn parse_flag(block: &BTreeMap<String, String>, key: &str) -> Result<Option<bool>, FormatError> {
    match block.get(key).map(|value| value.trim()) {
        Some("+") => Ok(Some(true)),
        Some("-") => Ok(Some(false)),
        Some(_) => Err(FormatError::CorruptArchive(format!(
            "7-Zip reported an invalid {key} value"
        ))),
        None => Ok(None),
    }
}

fn parse_u64(block: &BTreeMap<String, String>, key: &str) -> Result<Option<u64>, FormatError> {
    block
        .get(key)
        .map(|value| {
            value.trim().parse::<u64>().map_err(|_| {
                FormatError::CorruptArchive(format!("7-Zip reported an invalid {key} value"))
            })
        })
        .transpose()
}

fn infer_directory_entries(entries: &mut [EntryMeta]) {
    let paths: Vec<String> = entries
        .iter()
        .map(|entry| entry.path.display.clone())
        .collect();
    for entry in entries {
        if matches!(entry.entry_type, EntryType::Dir) {
            continue;
        }
        let prefix = format!("{}/", entry.path.display.trim_end_matches('/'));
        if paths.iter().any(|path| path.starts_with(&prefix)) {
            entry.entry_type = EntryType::Dir;
            entry.size = 0;
            entry.compressed_size = None;
        }
    }
}

fn push_list_block(entries: &mut Vec<EntryMeta>, block: &BTreeMap<String, String>) {
    let Some(path) = block.get("Path") else {
        return;
    };
    let is_entry = block.contains_key("Folder")
        || block.contains_key("Size")
        || block.contains_key("Packed Size")
        || block.contains_key("Attributes")
        || block.contains_key("CRC")
        || block.contains_key("Encrypted")
        || block.contains_key("Type");
    if !is_entry || path.is_empty() || path == "." || path == "./" {
        return;
    }

    let folder = block.get("Folder").is_some_and(|value| value.trim() == "+")
        || block
            .get("Attributes")
            .is_some_and(|value| value.bytes().any(|byte| byte == b'D'))
        || block
            .get("Type")
            .is_some_and(|value| value.trim().eq_ignore_ascii_case("directory"));
    let size = if folder {
        0
    } else {
        block
            .get("Size")
            .and_then(|value| value.trim().parse::<u64>().ok())
            .map_or(0, |size| size)
    };
    entries.push(EntryMeta {
        path: EntryPath::from_utf8(path),
        entry_type: if folder {
            EntryType::Dir
        } else {
            EntryType::File
        },
        size,
        compressed_size: block
            .get("Packed Size")
            .and_then(|value| value.trim().parse::<u64>().ok()),
        modified: None,
        unix_mode: None,
        crc32: block
            .get("CRC")
            .and_then(|value| u32::from_str_radix(value.trim(), 16).ok()),
        encrypted: block
            .get("Encrypted")
            .is_some_and(|value| value.trim() == "+"),
    });
}

#[cfg(test)]
mod tests {
    use std::io::{BufReader, Cursor};

    use super::*;

    #[test]
    fn sevenzip_listing_skips_archive_metadata_block() {
        let stdout = br#"
Path = /tmp/squallz-7z-temp.wim
Type = wim
Physical Size = 1351
Size = 17
Packed Size = 17
Images = 1

Path = project
Folder = +
Attributes = D

Path = project/README.txt
Folder = -
Size = 10
Packed Size = 10
Attributes = N

"#;

        let crlf = String::from_utf8_lossy(stdout).replace('\n', "\r\n");
        for stdout in [&stdout[..], crlf.trim_end_matches(['\r', '\n']).as_bytes()] {
            let listing = read(stdout).unwrap();
            let entries = listing.entries;
            assert_eq!(entries.len(), 2);
            assert_eq!(entries[0].path.display, "project");
            assert_eq!(entries[1].path.display, "project/README.txt");
            assert_eq!(entries[1].size, 10);
            assert!(!entries
                .iter()
                .any(|entry| entry.path.display.starts_with('/')));
            assert_eq!(
                listing.archive.unwrap(),
                SevenZipArchiveProperties::default()
            );
        }
    }

    #[test]
    fn sevenzip_listing_reports_native_volume_properties() {
        let stdout = br#"
Path = /private/stage/archive.part001.rar
Type = Rar5
Physical Size = 2048
Total Physical Size = 4558
Multivolume = +
Volume Index = 0
Volumes = 3

----------
Path = private.txt
Folder = -
Size = 128
Packed Size = 96
Encrypted = +

"#;

        let listing = read(&stdout[..]).unwrap();
        let properties = listing.archive.unwrap();
        assert_eq!(properties.multivolume, Some(true));
        assert_eq!(properties.volume_index, Some(0));
        assert_eq!(properties.volume_count, Some(3));
        assert_eq!(listing.entries.len(), 1);

        for (metadata, expected) in [
            (
                "Type = Rar5\nPhysical Size = 1\nMultivolume = ?\nVolume Index = -1\nVolumes = invalid",
                "7-Zip reported an invalid Multivolume value",
            ),
            (
                "Type = Rar5\nPhysical Size = 1\nVolume Index = -1\nVolumes = invalid",
                "7-Zip reported an invalid Volume Index value",
            ),
            (
                "Type = Rar5\nPhysical Size = 1\nVolumes = invalid",
                "7-Zip reported an invalid Volumes value",
            ),
            (
                "Type = Rar5\nPhysical Size = 1\n\nType = Rar5\nPhysical Size = 1\nMultivolume = ?",
                "7-Zip reported more than one archive metadata block",
            ),
            (
                "Type = Rar5\nPhysical Size = 1\nMultivolume = ?\n\nType = Rar5\nPhysical Size = 1",
                "7-Zip reported an invalid Multivolume value",
            ),
        ] {
            let stdout = format!("{metadata}\n\nPath = retained.txt\nSize = 3");
            let listing = read(stdout.as_bytes()).unwrap();
            assert_eq!(listing.entries.len(), 1);
            assert_eq!(listing.entries[0].path.display, "retained.txt");
            assert_eq!(listing.entries[0].size, 3);
            assert!(matches!(
                listing.archive,
                Err(FormatError::CorruptArchive(detail)) if detail == expected
            ));
        }
    }

    #[test]
    fn sevenzip_listing_keeps_xar_typed_entries() {
        let stdout = br#"
Path = /tmp/squallz-7z-temp.xar
Type = Xar
Physical Size = 979
Method = SHA1

Path = hello.txt
Size = 12
Packed Size = 20
Mode = -rw-r--r--
Type = file

Path = dir
Size =
Packed Size =
Mode = drwxr-xr-x
Type = directory

Path = dir/nested.txt
Size = 13
Packed Size = 21
Mode = -rw-r--r--
Type = file

"#;

        let entries = read(&stdout[..]).unwrap().entries;
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].path.display, "hello.txt");
        assert_eq!(entries[0].size, 12);
        assert!(matches!(entries[0].entry_type, EntryType::File));
        assert_eq!(entries[1].path.display, "dir");
        assert!(matches!(entries[1].entry_type, EntryType::Dir));
        assert_eq!(entries[2].path.display, "dir/nested.txt");
        assert_eq!(entries[2].size, 13);

        let path = format!("{}中文 = payload.txt", "segment/".repeat(1024));
        let mut stdout = format!(
            "Path = ignored.txt\n Path = {path}\nSize = -1\nSize = 27\n \nPacked Size = invalid\nCRC = deadBEEF\nEncrypted = unknown\nMode = drwxr-xr-x\r\n\r\nPath = bad"
        )
        .into_bytes();
        stdout.push(0xff);
        stdout.extend_from_slice(b".txt\r\nSize = invalid\r\nPacked Size = -1\r\nCRC = invalid\r");
        let listing = read(BufReader::with_capacity(1, Cursor::new(stdout))).unwrap();
        assert_eq!(listing.entries.len(), 2);
        let entry = &listing.entries[0];
        assert_eq!(entry.path.display, path);
        assert_eq!(entry.path.raw, path.as_bytes());
        assert_eq!(entry.path.encoding, "utf-8");
        assert_eq!(entry.size, 27);
        assert_eq!(entry.compressed_size, None);
        assert_eq!(entry.crc32, Some(0xdead_beef));
        assert!(!entry.encrypted);
        assert!(matches!(entry.entry_type, EntryType::File));
        let entry = &listing.entries[1];
        assert_eq!(entry.path.display, "bad\u{fffd}.txt");
        assert_eq!(entry.path.raw, entry.path.display.as_bytes());
        assert_eq!(entry.size, 0);
        assert_eq!(entry.compressed_size, None);
        assert_eq!(entry.crc32, None);
    }

    #[test]
    fn sevenzip_listing_skips_cpio_root_dot_entry() {
        let stdout = br#"
Path = .
Folder = +
Size = 0
Packed Size = 0

Path = ./sub
Folder = +
Size = 0
Packed Size = 0

Path = ./sub/data.txt
Folder = -
Size = 15
Packed Size = 16

Path = ./README.txt
Folder = -
Size = 14
Packed Size = 16

"#;

        let entries = read(&stdout[..]).unwrap().entries;
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].path.display, "./sub");
        assert_eq!(entries[1].path.display, "./sub/data.txt");
        assert_eq!(entries[2].path.display, "./README.txt");
        assert!(!entries.iter().any(|entry| entry.path.display == "."));

        let entries = read(
            &b"Path = ./\nFolder = +\n\nPath = only-path.txt\n\nPath = \nSize = 4\n\nPath = ./kept.txt\nSize = 2"[..],
        )
        .unwrap()
        .entries;
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path.display, "./kept.txt");
    }

    #[test]
    fn sevenzip_listing_infers_directory_prefix_entries() {
        let stdout = br#"
Path = sub
Folder = -
Size = 0

Path = README.txt
Folder = -
Size = 15
Packed Size = 4096

Path = sub/data.txt
Folder = -
Size = 16
Packed Size = 4096

"#;

        let entries = read(&stdout[..]).unwrap().entries;
        assert_eq!(entries.len(), 3);
        assert!(matches!(entries[0].entry_type, EntryType::Dir));
        assert_eq!(entries[0].path.display, "sub");
        assert_eq!(entries[0].size, 0);
        assert_eq!(entries[0].compressed_size, None);
        assert!(matches!(entries[1].entry_type, EntryType::File));
        assert!(matches!(entries[2].entry_type, EntryType::File));

        let entries = read(
            &b"Path = typed\nFolder = -\nType = DiReCtOrY\nSize = 7\nPacked Size = 9\n\nPath = attributed\nFolder = -\nAttributes = ND\nSize = 7\nPacked Size = 9\n\nPath = inferred/\nFolder = -\nSize = 7\nPacked Size = 9\nCRC = 12ab\nEncrypted = +\n\nPath = duplicate.txt\nSize = 1\n\nPath = duplicate.txt\nSize = 2"[..],
        )
        .unwrap()
        .entries;
        assert_eq!(entries.len(), 5);
        for entry in &entries[..3] {
            assert!(matches!(entry.entry_type, EntryType::Dir));
            assert_eq!(entry.size, 0);
        }
        assert_eq!(entries[0].compressed_size, Some(9));
        assert_eq!(entries[1].compressed_size, Some(9));
        assert_eq!(entries[2].path.display, "inferred/");
        assert_eq!(entries[2].compressed_size, None);
        assert_eq!(entries[2].crc32, Some(0x12ab));
        assert!(entries[2].encrypted);
        assert_eq!(entries[3].path.display, "duplicate.txt");
        assert_eq!(entries[3].size, 1);
        assert_eq!(entries[4].path.display, "duplicate.txt");
        assert_eq!(entries[4].size, 2);
    }
}
