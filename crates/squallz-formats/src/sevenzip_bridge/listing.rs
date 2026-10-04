//! Streaming parser for 7-Zip's technical listing output.

mod rar_compatibility;

pub(crate) use rar_compatibility::RarCompatibility;

use std::collections::BTreeMap;
use std::io::{self, BufRead};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use chrono::NaiveDateTime;

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
    let mut is_zip = false;
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
            finish_block(&mut entries, &mut block, &mut archive, &mut is_zip);
            facts.finish_block();
        } else {
            let field = line.split_once(" = ");
            facts.observe_line(line, field);
            if let Some((key, value)) = field {
                block.insert(key.trim().to_owned(), value.to_owned());
            }
        }
    }
    finish_block(&mut entries, &mut block, &mut archive, &mut is_zip);
    facts.finish_block();
    if archive.is_err() {
        for entry in &mut entries {
            entry.modified = None;
        }
    }
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
    is_zip: &mut bool,
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
            *is_zip =
                properties.is_ok() && block.get("Type").is_some_and(|value| value.trim() == "zip");
            *archive = properties.map(Some);
        } else {
            *is_zip = false;
        }
    } else {
        push_list_block(entries, block, *is_zip);
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
    let mut path_indices: Vec<usize> = (0..entries.len()).collect();
    path_indices.sort_unstable_by(|&left, &right| {
        entries[left].path.display.cmp(&entries[right].path.display)
    });
    // Only metadata changes below, so the sorted path index stays valid.
    let mut prefix = String::new();
    for index in 0..entries.len() {
        if matches!(entries[index].entry_type, EntryType::Dir) {
            continue;
        }
        prefix.clear();
        prefix.push_str(entries[index].path.display.trim_end_matches('/'));
        prefix.push('/');
        let position = path_indices
            .partition_point(|&index| entries[index].path.display.as_str() < prefix.as_str());
        if path_indices
            .get(position)
            .is_some_and(|&index| entries[index].path.display.starts_with(&prefix))
        {
            entries[index].entry_type = EntryType::Dir;
            entries[index].size = 0;
            entries[index].compressed_size = None;
        }
    }
}

fn push_list_block(entries: &mut Vec<EntryMeta>, block: &BTreeMap<String, String>, is_zip: bool) {
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
        // ZIP NTFS timestamps carry a fraction. DOS and Unix seconds cannot
        // be distinguished here; keep both unknown rather than trusting
        // 7-Zip's current DST bias for a historical DOS timestamp.
        modified: block
            .get("Modified")
            .filter(|value| is_zip && value.contains('.'))
            .and_then(|value| parse_modified(value)),
        unix_mode: None,
        crc32: block
            .get("CRC")
            .and_then(|value| u32::from_str_radix(value.trim(), 16).ok()),
        encrypted: block
            .get("Encrypted")
            .is_some_and(|value| value.trim() == "+"),
    });
}

fn parse_modified(value: &str) -> Option<SystemTime> {
    // The explicit UTC marker avoids interpreting an unmarked wall time in
    // the reader's local time zone.
    let value = value.trim().strip_suffix('Z')?;
    let (seconds, fraction) = match value.split_once('.') {
        Some((seconds, fraction)) => (seconds, Some(fraction)),
        None => (value, None),
    };
    if seconds.len() != 19
        || !seconds
            .bytes()
            .enumerate()
            .all(|(index, byte)| match index {
                4 | 7 => byte == b'-',
                10 => byte == b' ',
                13 | 16 => byte == b':',
                _ => byte.is_ascii_digit(),
            })
        || seconds.get(17..19)?.parse::<u8>().ok()? >= 60
    {
        return None;
    }
    let nanos = match fraction {
        Some(fraction)
            if (1..=9).contains(&fraction.len())
                && fraction.bytes().all(|byte| byte.is_ascii_digit()) =>
        {
            fraction.parse::<u32>().ok()? * 10u32.pow(9 - fraction.len() as u32)
        }
        Some(_) => return None,
        None => 0,
    };
    let seconds = NaiveDateTime::parse_from_str(seconds, "%Y-%m-%d %H:%M:%S")
        .ok()?
        .and_utc()
        .timestamp();
    let base = if seconds < 0 {
        UNIX_EPOCH.checked_sub(Duration::from_secs(seconds.unsigned_abs()))?
    } else {
        UNIX_EPOCH.checked_add(Duration::from_secs(seconds as u64))?
    };
    base.checked_add(Duration::from_nanos(u64::from(nanos)))
}

#[cfg(test)]
mod tests {
    use std::io::{BufReader, Cursor};
    use std::time::{Duration, UNIX_EPOCH};

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
Modified = 2000-01-01 00:00:00Z

Path = project
Folder = +
Attributes = D
Modified = 2023-11-14 22:13:20Z

Path = project/README.txt
Folder = -
Size = 10
Packed Size = 10
Attributes = N
Modified = 2023-11-14 22:13:21.1234567Z

"#;

        let crlf = String::from_utf8_lossy(stdout).replace('\n', "\r\n");
        for stdout in [&stdout[..], crlf.trim_end_matches(['\r', '\n']).as_bytes()] {
            let listing = read(stdout).unwrap();
            let entries = listing.entries;
            assert_eq!(entries.len(), 2);
            assert_eq!(entries[0].path.display, "project");
            assert_eq!(entries[1].path.display, "project/README.txt");
            assert_eq!(entries[1].size, 10);
            assert_eq!(entries[0].modified, None);
            assert_eq!(entries[1].modified, None);
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
    fn sevenzip_listing_preserves_fractional_and_historical_utc_times() {
        for (value, expected) in [
            ("1970-01-01 00:00:00Z", UNIX_EPOCH),
            (
                "1969-12-31 23:59:59.123456789Z",
                UNIX_EPOCH - Duration::from_nanos(876_543_211),
            ),
            (
                "2024-02-29 23:59:58Z",
                UNIX_EPOCH + Duration::from_secs(1_709_251_198),
            ),
            (
                "2038-01-19 03:14:08Z",
                UNIX_EPOCH + Duration::from_secs(2_147_483_648),
            ),
        ] {
            assert_eq!(parse_modified(value), Some(expected), "{value}");
        }
        for (fraction, nanos) in [
            ("1", 100_000_000),
            ("12", 120_000_000),
            ("123", 123_000_000),
            ("1234", 123_400_000),
            ("12345", 123_450_000),
            ("123456", 123_456_000),
            ("1234567", 123_456_700),
            ("12345678", 123_456_780),
            ("123456789", 123_456_789),
        ] {
            let stdout = format!(
                "Type = zip\r\nPhysical Size = 1\r\n\r\nPath = retained.txt\r\nSize = 1\r\nModified = 1970-01-01 00:00:01.{fraction}Z"
            );
            let entries = read(BufReader::with_capacity(1, Cursor::new(stdout)))
                .unwrap()
                .entries;
            assert_eq!(entries.len(), 1);
            assert_eq!(
                entries[0].modified,
                Some(UNIX_EPOCH + Duration::new(1, nanos))
            );
        }
    }

    #[test]
    fn sevenzip_listing_does_not_infer_utc_metadata_from_other_time_sources() {
        for (archive, entry, modified) in [
            (
                "Type = zip\nPhysical Size = 1\n\n",
                "",
                "2023-11-14 22:13:20Z",
            ),
            (
                "Type = zip\nPhysical Size = 1\n\n",
                "",
                "2023-11-14 22:13:21Z",
            ),
            (
                "Type = zip\nPhysical Size = 1\n\n",
                "",
                "2023-11-15 06:13:20.1234567",
            ),
            (
                "Type = wim\nPhysical Size = 1\n\n",
                "",
                "2023-11-14 22:13:20.1234567Z",
            ),
            (
                "Type = Rar5\nPhysical Size = 1\n\n",
                "",
                "2023-11-14 22:13:20.1234567Z",
            ),
            (
                "Type = Xar\nPhysical Size = 1\n\n",
                "",
                "2023-11-14 22:13:20.1234567Z",
            ),
            ("", "", "2023-11-14 22:13:20.1234567Z"),
            ("", "Type = zip\n", "2023-11-14 22:13:20.1234567Z"),
            ("Type = zip\n\n", "", "2023-11-14 22:13:20.1234567Z"),
        ] {
            let stdout =
                format!("{archive}Path = retained.txt\nSize = 3\n{entry}Modified = {modified}\n");
            let entries = read(stdout.as_bytes()).unwrap().entries;
            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0].modified, None, "{stdout}");
        }
        let entries = read(
            &b"Type = zip\nPhysical Size = 1\n\nPath = folder\nFolder = +\nModified = 2023-11-14 22:13:20.0000000Z"[..],
        )
        .unwrap()
        .entries;
        assert_eq!(entries[0].entry_type, EntryType::Dir);
        assert_eq!(
            entries[0].modified,
            Some(UNIX_EPOCH + Duration::from_secs(1_700_000_000))
        );
        let listing = read(
            &b"Type = zip\nPhysical Size = 1\n\nPath = retained.txt\nSize = 3\nModified = 2023-11-14 22:13:20.1234567Z\n\nType = zip\nPhysical Size = 1"[..],
        )
        .unwrap();
        assert_eq!(listing.entries.len(), 1);
        assert_eq!(listing.entries[0].path.display, "retained.txt");
        assert_eq!(listing.entries[0].size, 3);
        assert_eq!(listing.entries[0].modified, None);
        assert!(matches!(
            listing.archive,
            Err(FormatError::CorruptArchive(detail))
                if detail == "7-Zip reported more than one archive metadata block"
        ));
    }

    #[test]
    fn sevenzip_listing_keeps_missing_invalid_and_local_times_unknown() {
        for value in [
            None,
            Some(""),
            Some("2023-11-15 06:13:20"),
            Some("2023-11-15 06:13:20.1234567"),
            Some("2024-02-30 00:00:00Z"),
            Some("2024-13-01 00:00:00Z"),
            Some("2024-01-01 24:00:00Z"),
            Some("2024-01-01 00:60:00Z"),
            Some("2016-12-31 23:59:60Z"),
            Some("1970-01-01 00:00:00.Z"),
            Some("1970-01-01 00:00:00.1234567890Z"),
            Some("1970-01-01 00:00:00+00:00"),
            Some("1970-01-01 00:00:00z"),
            Some("1970-01-01 00:00:00Z trailing"),
            Some("invalid"),
        ] {
            let mut stdout = String::from(
                "Type = zip\nPhysical Size = 1\n\nPath = retained.txt\nSize = 3\nEncrypted = +\n",
            );
            if let Some(value) = value {
                assert_eq!(parse_modified(value), None, "{value}");
                stdout.push_str(&format!("Modified = {value}\n"));
            }
            let entries = read(stdout.as_bytes()).unwrap().entries;
            assert_eq!(entries.len(), 1, "{value:?}");
            assert_eq!(entries[0].path.display, "retained.txt");
            assert_eq!(entries[0].size, 3);
            assert!(entries[0].encrypted);
            assert_eq!(entries[0].modified, None, "{value:?}");
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

        let entries = read(
            concat!(
                "Path = self///\nSize = 7\nPacked Size = 9\n\n",
                "Path = self-other\nSize = 3\n\n",
                "Path = literal\\name\nSize = 5\nPacked Size = 4\n\n",
                "Path = literal/name/child\nSize = 6",
            )
            .as_bytes(),
        )
        .unwrap()
        .entries;
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.path.display.as_str())
                .collect::<Vec<_>>(),
            [
                "self///",
                "self-other",
                "literal\\name",
                "literal/name/child"
            ]
        );
        assert!(matches!(entries[0].entry_type, EntryType::Dir));
        assert_eq!(entries[0].size, 0);
        assert_eq!(entries[0].compressed_size, None);
        for entry in &entries[1..] {
            assert!(matches!(entry.entry_type, EntryType::File));
        }
        assert_eq!(entries[1].size, 3);
        assert_eq!(entries[2].size, 5);
        assert_eq!(entries[2].compressed_size, Some(4));
        assert_eq!(entries[3].size, 6);
    }
}
