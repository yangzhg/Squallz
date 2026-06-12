#![no_main]

use std::io::{Cursor, Read};

use libfuzzer_sys::fuzz_target;
use squallz_format_api::{ControlToken, Detected, EntryType, NoProgress, OpenOptions};

const MAX_INPUT_BYTES: usize = 2 * 1024 * 1024;
const MAX_ENTRIES_TO_LIST: usize = 32;
const MAX_ENTRIES_TO_READ: usize = 8;
const MAX_ENTRY_READ_BYTES: usize = 64 * 1024;
const MAX_TEST_DECLARED_BYTES: u64 = 256 * 1024;

fuzz_target!(|data: &[u8]| {
    fuzz_zip_reader(data);
});

fn fuzz_zip_reader(data: &[u8]) {
    if data.is_empty() || data.len() > MAX_INPUT_BYTES {
        return;
    }

    let registry = squallz_formats::registry();
    let head_len = data.len().min(512);
    let tail_len = data.len().min(64);
    let tail_start = data.len().saturating_sub(tail_len);
    let Some(Detected::Archive(format)) =
        registry.detect(Some("fuzz.zip"), &data[..head_len], &data[tail_start..])
    else {
        return;
    };
    if format.id() != "zip" {
        return;
    }

    let opts = OpenOptions::default();
    let cursor = Cursor::new(data.to_vec());
    let Ok(mut reader) = format.open(Box::new(cursor), &opts) else {
        return;
    };

    let entries = reader
        .entries()
        .take(MAX_ENTRIES_TO_LIST)
        .filter_map(Result::ok)
        .collect::<Vec<_>>();

    let declared_file_bytes = entries
        .iter()
        .filter(|entry| matches!(entry.entry_type, EntryType::File))
        .map(|entry| entry.size)
        .fold(0u64, u64::saturating_add);
    if declared_file_bytes <= MAX_TEST_DECLARED_BYTES {
        let progress = NoProgress;
        let control = ControlToken::default();
        let _ = reader.test(&progress, &control);
    }

    let mut read_buf = [0u8; 4096];
    for entry in entries
        .iter()
        .filter(|entry| matches!(entry.entry_type, EntryType::File))
        .take(MAX_ENTRIES_TO_READ)
    {
        let Ok(mut stream) = reader.read_entry(&entry.path) else {
            continue;
        };
        let mut remaining = MAX_ENTRY_READ_BYTES.min(entry.size as usize);
        while remaining > 0 {
            let want = remaining.min(read_buf.len());
            match stream.read(&mut read_buf[..want]) {
                Ok(0) => break,
                Ok(n) => remaining = remaining.saturating_sub(n),
                Err(_) => break,
            }
        }
    }
}
