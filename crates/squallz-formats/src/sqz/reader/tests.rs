use super::super::RECOVERY_PROTECTION_TRAILER_LEN;
use super::*;
use squallz_format_api::{CreateOptions, NoProgress, Password, SqzCreateOptions, SqzInnerFormat};
use std::io::{Cursor, Write};
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Clone, Default)]
struct Output(Arc<Mutex<Cursor<Vec<u8>>>>);

impl Write for Output {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().write(bytes)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Seek for Output {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.0.lock().unwrap().seek(position)
    }
}

struct CountedSource {
    data: Cursor<Vec<u8>>,
    read_bytes: Arc<AtomicUsize>,
}

impl Read for CountedSource {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let count = self.data.read(buffer)?;
        self.read_bytes.fetch_add(count, Ordering::Relaxed);
        Ok(count)
    }
}

impl Seek for CountedSource {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.data.seek(position)
    }
}

fn sqz_bytes(meta: &EntryMeta, content: &[u8]) -> Vec<u8> {
    let output = Output::default();
    let mut writer = super::super::SqzFormat
        .create(Box::new(output.clone()), &CreateOptions::default())
        .unwrap();
    writer
        .add_entry(meta, Some(&mut Cursor::new(content)))
        .unwrap();
    writer.finish().unwrap();
    let bytes = output.0.lock().unwrap().get_ref().clone();
    bytes
}

fn file_record(data: &[u8]) -> SqzRecord {
    SqzRecord {
        meta: EntryMeta {
            path: EntryPath::from_utf8("file.bin"),
            entry_type: EntryType::File,
            size: data.len() as u64,
            compressed_size: Some(data.len() as u64),
            modified: None,
            unix_mode: None,
            crc32: None,
            encrypted: false,
        },
        data_offset: 0,
        data_size: data.len() as u64,
        hash: *blake3::hash(data).as_bytes(),
        crc32c: crc32c::crc32c(data),
    }
}

#[test]
fn integrity_testing_cancels_after_a_chunk_without_reading_the_complete_entry() {
    struct CancelAfterProgress(ControlToken);
    impl ProgressSink for CancelAfterProgress {
        fn on_progress(&self, done: u64, _total: u64, _current: &EntryPath) {
            if done > 0 {
                self.0.cancel();
            }
        }
    }
    let data = vec![b'x'; 8 * 1024 * 1024];
    let record = file_record(&data);
    let read_bytes = Arc::new(AtomicUsize::new(0));
    let mut reader = EntrySetSqzReader {
        src: Box::new(CountedSource {
            data: Cursor::new(data),
            read_bytes: read_bytes.clone(),
        }),
        records: vec![record],
        recovery: None,
    };
    let control = ControlToken::default();
    let result = reader.test_summary(&CancelAfterProgress(control.clone()), &control);
    assert!(matches!(result, Err(FormatError::Cancelled)), "{result:?}");
    assert!(
        read_bytes.load(Ordering::Relaxed) <= VERIFY_CHUNK,
        "integrity testing must report progress and check cancellation while reading"
    );
}

#[test]
fn streamed_integrity_checks_both_hashes_and_reports_truncated_data() {
    let data = vec![b'x'; VERIFY_CHUNK * 2 + 17];
    for corruption in ["none", "blake3", "crc32c", "truncated", "empty"] {
        let source = if corruption == "empty" {
            &[][..]
        } else {
            &data
        };
        let mut record = file_record(source);
        let mut source = source.to_vec();
        match corruption {
            "blake3" => record.hash[0] ^= 1,
            "crc32c" => record.crc32c ^= 1,
            "truncated" => {
                source.pop();
            }
            _ => {}
        }
        let mut reader = EntrySetSqzReader {
            src: Box::new(Cursor::new(source)),
            records: vec![record],
            recovery: None,
        };
        let result = reader
            .test_summary(&NoProgress, &ControlToken::new())
            .unwrap();
        assert_eq!(result.entries_tested, 1);
        assert_eq!(
            result.is_ok(),
            matches!(corruption, "none" | "empty"),
            "{corruption}: {result:?}"
        );
        if corruption == "truncated" {
            assert!(result.problems.messages[0].contains("truncated SQZ file data"));
        }
    }
}

#[test]
fn streamed_integrity_retries_interrupted_reads() {
    struct InterruptOnce {
        data: Cursor<Vec<u8>>,
        interrupted: bool,
    }
    impl Read for InterruptOnce {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            if !self.interrupted {
                self.interrupted = true;
                return Err(io::ErrorKind::Interrupted.into());
            }
            self.data.read(buffer)
        }
    }
    impl Seek for InterruptOnce {
        fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
            self.data.seek(position)
        }
    }
    let data = b"interrupted source";
    let mut reader = EntrySetSqzReader {
        src: Box::new(InterruptOnce {
            data: Cursor::new(data.to_vec()),
            interrupted: false,
        }),
        records: vec![file_record(data)],
        recovery: None,
    };
    assert!(reader
        .test_summary(&NoProgress, &ControlToken::new())
        .unwrap()
        .is_ok());
}

fn recovered_source() -> RecoveredReadSeek {
    let state = RecoveredPayload {
        summary: RecoverySummary {
            scheme: "sqz-embedded-rs-gf8".into(),
            block_size: 4,
            total_blocks: 3,
            data_shards: 8,
            parity_shards: 2,
            recovery_blocks_available: 2,
            damaged_blocks: 2,
            repaired_blocks: 2,
            unrepaired_blocks: 0,
            repair_possible: true,
        },
        payload_start: 3,
        payload_length: 10,
        block_size: 4,
        repaired_blocks: HashMap::from([(1, b"efgh".to_vec()), (2, b"ij".to_vec())]),
        unrepaired_blocks: HashSet::new(),
    };
    RecoveredReadSeek {
        inner: Box::new(Cursor::new(b"HDRabcdXXXXXXTAIL".to_vec())),
        recovery: Arc::new(state),
        len: 17,
        pos: 0,
    }
}

#[test]
fn recovered_reads_preserve_boundaries_and_random_access() {
    let expected = b"HDRabcdefghijTAIL";
    let mut reader = recovered_source();
    let mut all = Vec::new();
    reader.read_to_end(&mut all).unwrap();
    assert_eq!(&all, expected);
    for start in (0..expected.len()).rev() {
        reader.seek(SeekFrom::Start(start as u64)).unwrap();
        assert_eq!(reader.read(&mut []).unwrap(), 0);
        let mut suffix = Vec::new();
        reader.read_to_end(&mut suffix).unwrap();
        assert_eq!(&suffix, &expected[start..]);
    }
    reader.seek(SeekFrom::End(-6)).unwrap();
    let mut tail = [0; 6];
    reader.read_exact(&mut tail).unwrap();
    assert_eq!(&tail, b"ijTAIL");
    reader.seek(SeekFrom::Current(-10)).unwrap();
    let mut repaired = [0; 6];
    reader.read_exact(&mut repaired).unwrap();
    assert_eq!(&repaired, b"efghij");
    assert!(reader.seek(SeekFrom::Start(u64::MAX)).is_ok());
    assert_eq!(
        reader.seek(SeekFrom::Current(1)).unwrap_err().kind(),
        io::ErrorKind::InvalidInput
    );
    assert_eq!(reader.read(&mut tail).unwrap(), 0);
    assert_eq!(
        reader.seek(SeekFrom::End(-18)).unwrap_err().kind(),
        io::ErrorKind::InvalidInput
    );
}

#[test]
fn recovered_entry_prefix_reads_only_requested_bytes_and_remains_reusable() {
    let mut source = recovered_source();
    let read_bytes = Arc::new(AtomicUsize::new(0));
    source.inner = Box::new(CountedSource {
        data: Cursor::new(b"HDRabcdXXXXXXTAIL".to_vec()),
        read_bytes: Arc::clone(&read_bytes),
    });
    let mut record = file_record(b"abcdefghij");
    record.data_offset = 3;
    let path = record.meta.path.clone();
    let mut reader = EntrySetSqzReader {
        recovery: Some(Arc::clone(&source.recovery)),
        src: Box::new(source),
        records: vec![record],
    };
    reader
        .read_entry(&path, &mut |data| {
            let mut prefix = [0; 2];
            data.read_exact(&mut prefix)?;
            assert_eq!(&prefix, b"ab");
            Ok(())
        })
        .unwrap();
    assert_eq!(read_bytes.load(Ordering::Relaxed), 2);
    reader
        .read_entry(&path, &mut |data| {
            let mut all = Vec::new();
            data.read_to_end(&mut all)?;
            assert_eq!(&all, b"abcdefghij");
            Ok(())
        })
        .unwrap();
    assert!(reader
        .test_summary(&NoProgress, &ControlToken::new())
        .unwrap()
        .is_ok());
}

#[test]
fn recovered_reads_reject_missing_or_truncated_repaired_blocks() {
    for unrepaired in [true, false] {
        let mut reader = recovered_source();
        let recovery = Arc::get_mut(&mut reader.recovery).unwrap();
        if unrepaired {
            recovery.unrepaired_blocks.insert(1);
        } else {
            recovery.repaired_blocks.insert(1, vec![0]);
        }
        reader.seek(SeekFrom::Start(7)).unwrap();
        assert_eq!(
            reader.read(&mut [0; 4]).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }
}

#[test]
fn recovered_encrypted_inner_archives_remain_readable() {
    let content = vec![b'x'; VERIFY_CHUNK + 17];
    let meta = file_record(&content).meta;
    for inner_format in [SqzInnerFormat::Zip, SqzInnerFormat::SevenZip] {
        let password = Password::new("encrypted inner payload test");
        let opts = CreateOptions {
            password: Some(password.clone()),
            sqz: SqzCreateOptions {
                inner_format,
                ..SqzCreateOptions::default()
            },
            ..CreateOptions::default()
        };
        assert!(matches!(
            super::super::SqzFormat.create(Box::new(Output::default()), &opts),
            Err(FormatError::Unsupported(_))
        ));
        // Build a reader compatibility fixture below the public creation boundary.
        let output = Output::default();
        let mut writer = super::super::writer::create(Box::new(output.clone()), &opts).unwrap();
        writer
            .add_entry(&meta, Some(&mut Cursor::new(&content)))
            .unwrap();
        writer.finish().unwrap();
        let mut bytes = output.0.lock().unwrap().get_ref().clone();
        let descriptor_len = u64::from_le_bytes(bytes[40..48].try_into().unwrap()) as usize;
        bytes[HEADER_LEN + descriptor_len] ^= 0xA5;
        let mut reader = SqzArchiveReader::open(
            Box::new(Cursor::new(bytes)),
            &OpenOptions {
                password: Some(password),
                ..OpenOptions::default()
            },
        )
        .unwrap();
        let entries = reader.entries().collect::<Result<Vec<_>, _>>().unwrap();
        assert_eq!(entries.len(), 1);
        assert!(entries[0].encrypted, "{inner_format}");
        for _ in 0..2 {
            let mut restored = Vec::new();
            reader
                .read_entry(&entries[0].path, &mut |data| {
                    data.read_to_end(&mut restored)?;
                    Ok(())
                })
                .unwrap();
            assert_eq!(restored, content, "{inner_format}");
        }
        let report = reader
            .test_summary(&NoProgress, &ControlToken::new())
            .unwrap();
        assert!(report.is_ok(), "{inner_format}: {report:?}");
        assert_eq!(report.recovery.unwrap().repaired_blocks, 1);
    }
}

#[test]
fn recovery_loading_uses_bounded_reads_for_healthy_and_damaged_archives() {
    struct BoundedSource {
        data: Cursor<Vec<u8>>,
        max_read: Arc<AtomicUsize>,
    }
    impl Read for BoundedSource {
        fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
            self.max_read.fetch_max(bytes.len(), Ordering::Relaxed);
            self.data.read(bytes)
        }
    }
    impl Seek for BoundedSource {
        fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
            self.data.seek(pos)
        }
    }
    let content = vec![b'x'; 8 * 1024 * 1024 + 17];
    let original = sqz_bytes(&file_record(&content).meta, &content);
    let footer = original.len() - FOOTER_LEN;
    let recovery =
        u64::from_le_bytes(original[footer + 16..footer + 24].try_into().unwrap()) as usize;
    let recovery_len =
        u64::from_le_bytes(original[footer + 24..footer + 32].try_into().unwrap()) as usize;
    let payload = HEADER_LEN + u64::from_le_bytes(original[40..48].try_into().unwrap()) as usize;
    for damage in ["none", "payload", "recovery", "trailer", "footer"] {
        let mut bytes = original.clone();
        match damage {
            "payload" => bytes[payload + 13] ^= 1,
            "recovery" => bytes[recovery] ^= 1,
            "trailer" => bytes[recovery + recovery_len - 1] ^= 1,
            "footer" => bytes[footer + 48] ^= 1,
            _ => {}
        }
        let max_read = Arc::new(AtomicUsize::new(0));
        let mut reader = SqzArchiveReader::open(
            Box::new(BoundedSource {
                data: Cursor::new(bytes),
                max_read: Arc::clone(&max_read),
            }),
            &OpenOptions::default(),
        )
        .unwrap();
        let report = reader
            .test_summary(&NoProgress, &ControlToken::new())
            .unwrap();
        assert!(report.is_ok(), "{damage}: {report:?}");
        assert_eq!(
            report.recovery.unwrap().repaired_blocks,
            u64::from(damage == "payload")
        );
        assert!(
            max_read.load(Ordering::Relaxed) <= VERIFY_CHUNK + RECOVERY_PROTECTION_TRAILER_LEN,
            "{damage}: recovery loading requested {} bytes in one read",
            max_read.load(Ordering::Relaxed)
        );
    }
}

#[test]
fn recovery_loading_preserves_cancellation_and_source_errors() {
    struct FaultingSource {
        data: Cursor<Vec<u8>>,
        range: std::ops::Range<u64>,
        cancel: bool,
    }
    impl Read for FaultingSource {
        fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
            if self.range.contains(&self.data.position()) {
                return Err(if self.cancel {
                    io::Error::other(FormatError::Cancelled)
                } else {
                    io::ErrorKind::PermissionDenied.into()
                });
            }
            self.data.read(bytes)
        }
    }
    impl Seek for FaultingSource {
        fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
            self.data.seek(pos)
        }
    }
    let content = vec![b'x'; 1024 * 1024];
    let original = sqz_bytes(&file_record(&content).meta, &content);
    let footer = original.len() - FOOTER_LEN;
    let recovery = u64::from_le_bytes(original[footer + 16..footer + 24].try_into().unwrap());
    let recovery_len = u64::from_le_bytes(original[footer + 24..footer + 32].try_into().unwrap());
    let payload = HEADER_LEN as u64 + u64::from_le_bytes(original[40..48].try_into().unwrap());
    for phase in ["payload", "trailer", "footer", "footer_read"] {
        for cancel in [false, true] {
            let mut bytes = original.clone();
            let range = match phase {
                "footer_read" => footer as u64..bytes.len() as u64,
                "trailer" => {
                    bytes[(recovery + recovery_len - 1) as usize] ^= 1;
                    recovery..recovery + 84
                }
                "footer" => {
                    bytes[footer + 48] ^= 1;
                    recovery..recovery + recovery_len
                }
                _ => payload..recovery,
            };
            let result = SqzArchiveReader::open(
                Box::new(FaultingSource {
                    data: Cursor::new(bytes),
                    range,
                    cancel,
                }),
                &OpenOptions::default(),
            );
            let error = result.err().expect("source failure must stop recovery");
            if cancel {
                assert!(
                    matches!(error, FormatError::Cancelled),
                    "{phase}: {error:?}"
                );
            } else {
                assert!(
                    matches!(error, FormatError::Io(ref io) if io.kind() == io::ErrorKind::PermissionDenied),
                    "{phase}: {error:?}"
                );
            }
        }
    }
}

#[test]
fn footer_recovery_skips_false_candidates_and_finds_a_trailer_across_chunks() {
    let content = b"scan boundary";
    let mut meta = file_record(content).meta;
    let small = sqz_bytes(&meta, content);
    let footer = small.len() - FOOTER_LEN;
    let index_len = u64::from_le_bytes(small[footer + 8..footer + 16].try_into().unwrap()) as usize;
    let target = VERIFY_CHUNK + 2 - FOOTER_LEN - RECOVERY_PROTECTION_TRAILER_LEN;
    meta.path = EntryPath::from_utf8(format!(
        "RSPC{}",
        "x".repeat(meta.path.raw.len() + (target - index_len) / 2 - 4)
    ));
    let mut bytes = sqz_bytes(&meta, content);
    let footer = bytes.len() - FOOTER_LEN;
    let index_len = u64::from_le_bytes(bytes[footer + 8..footer + 16].try_into().unwrap()) as usize;
    let distance = FOOTER_LEN + index_len + RECOVERY_PROTECTION_TRAILER_LEN;
    assert!((1..=3).contains(&(distance - VERIFY_CHUNK)));
    bytes[footer + 48] ^= 1;
    let mut reader =
        SqzArchiveReader::open(Box::new(Cursor::new(bytes)), &OpenOptions::default()).unwrap();
    assert!(reader
        .test_summary(&NoProgress, &ControlToken::new())
        .unwrap()
        .is_ok());
    let entries = reader.entries().collect::<Result<Vec<_>, _>>().unwrap();
    assert_eq!(entries[0].path.raw, meta.path.raw);
}

#[test]
fn recovery_loading_rejects_metadata_changed_after_verification() {
    struct ChangingSource {
        data: Cursor<Vec<u8>>,
        recovery_start: u64,
        primary_reads: usize,
    }
    impl Read for ChangingSource {
        fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
            if self.data.position() == self.recovery_start {
                self.primary_reads += 1;
                if self.primary_reads == 2 {
                    self.data.get_mut()[self.recovery_start as usize + 84] ^= 1;
                }
            }
            self.data.read(bytes)
        }
    }
    impl Seek for ChangingSource {
        fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
            self.data.seek(pos)
        }
    }
    let content = vec![b'x'; VERIFY_CHUNK];
    let bytes = sqz_bytes(&file_record(&content).meta, &content);
    let footer = bytes.len() - FOOTER_LEN;
    let recovery_start = u64::from_le_bytes(bytes[footer + 16..footer + 24].try_into().unwrap());
    let source = ChangingSource {
        data: Cursor::new(bytes),
        recovery_start,
        primary_reads: 0,
    };
    let error = SqzArchiveReader::open(Box::new(source), &OpenOptions::default())
        .err()
        .expect("modified recovery metadata must fail");
    assert!(
        matches!(error, FormatError::CorruptArchive(ref message) if message.contains("changed while opening archive")),
        "{error:?}"
    );
}
