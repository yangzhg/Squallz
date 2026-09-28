use super::*;
use squallz_format_api::{CreateOptions, NoProgress, Password, SqzCreateOptions, SqzInnerFormat};
use std::io::{Cursor, Write};
use std::sync::atomic::{AtomicUsize, Ordering};

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
    let state = RecoveryState {
        payload_start: 3,
        payload_length: 10,
        block_size: 4,
        data_shards: 8,
        parity_shards: 2,
        block_hashes: vec![[0; 32]; 3],
        parity: vec![],
        index_hash: [0; 32],
        index_mirror: vec![],
        repaired_blocks: HashMap::from([(1, b"efgh".to_vec()), (2, b"ij".to_vec())]),
        unrepaired_blocks: HashSet::new(),
    };
    RecoveredReadSeek {
        inner: Box::new(Cursor::new(b"HDRabcdXXXXXXTAIL".to_vec())),
        recovery: Arc::new(state.into()),
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
