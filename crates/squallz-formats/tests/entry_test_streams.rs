//! Exercise entry-test I/O policy through each native reader adapter.

mod common;

use std::io::{self, Cursor, Read, Seek, SeekFrom};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use common::raw_zip::{build_stored_zip, stored_zip_with_missing_central_directory};
use squallz_format_api::{
    ControlToken, Detected, EntryPath, FormatError, OpenOptions, ProgressSink, SafetyLimits,
};

#[derive(Clone, Copy)]
enum Failure {
    Interrupted,
    Cancelled,
    Damaged,
}

struct FaultReader {
    input: Cursor<Vec<u8>>,
    payload_offset: u64,
    armed: Arc<AtomicBool>,
    failure: Failure,
}

impl Read for FaultReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if self.input.position() == self.payload_offset && self.armed.swap(false, Ordering::SeqCst)
        {
            return Err(match self.failure {
                Failure::Interrupted => io::Error::from(io::ErrorKind::Interrupted),
                Failure::Cancelled => io::Error::other(FormatError::Cancelled),
                Failure::Damaged => io::Error::other("fixture read failure"),
            });
        }
        self.input.read(buffer)
    }
}

impl Seek for FaultReader {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.input.seek(position)
    }
}

#[derive(Default)]
struct Trace(Mutex<Vec<(u64, u64, String)>>);

impl ProgressSink for Trace {
    fn on_progress(&self, done: u64, total: u64, current: &EntryPath) {
        self.0
            .lock()
            .unwrap()
            .push((done, total, current.display.clone()));
    }
}

fn fixture(kind: &str) -> (Vec<u8>, u64) {
    let entries: &[(&[u8], &[u8])] = &[(b"payload.bin", b"payload bytes"), (b"empty", b"")];
    match kind {
        "zip" => (build_stored_zip(entries), 30 + entries[0].0.len() as u64),
        "local-zip" => (
            stored_zip_with_missing_central_directory(entries),
            30 + entries[0].0.len() as u64,
        ),
        "tar" => {
            let mut builder = tar::Builder::new(Vec::new());
            for (name, data) in entries {
                let mut header = tar::Header::new_gnu();
                header.set_path(std::str::from_utf8(name).unwrap()).unwrap();
                header.set_size(data.len() as u64);
                header.set_mode(0o644);
                header.set_cksum();
                builder.append(&header, *data).unwrap();
            }
            (builder.into_inner().unwrap(), 512)
        }
        _ => unreachable!(),
    }
}

#[test]
fn native_entry_tests_retry_interrupted_reads_and_report_empty_entries() {
    for kind in ["zip", "local-zip", "tar"] {
        let (bytes, payload_offset) = fixture(kind);
        let armed = Arc::new(AtomicBool::new(false));
        let Detected::Archive(format) = squallz_formats::registry()
            .detect_by_name(if kind == "tar" { "a.tar" } else { "a.zip" })
            .unwrap()
        else {
            panic!("expected archive")
        };
        let control = ControlToken::default();
        let mut reader = format
            .open(
                Box::new(FaultReader {
                    input: Cursor::new(bytes),
                    payload_offset,
                    armed: armed.clone(),
                    failure: Failure::Interrupted,
                }),
                &OpenOptions::default(),
                &control,
            )
            .unwrap();
        armed.store(true, Ordering::SeqCst);
        let trace = Trace::default();
        let report = reader
            .test_summary(&SafetyLimits::default(), &trace, &control)
            .unwrap();
        assert!(
            !armed.load(Ordering::SeqCst),
            "{kind}: fault was not exercised"
        );
        assert!(report.is_ok(), "{kind}: {report:?}");
        assert_eq!(report.entries_tested, 2, "{kind}");
        let total = if kind == "tar" { 0 } else { 13 };
        assert_eq!(
            *trace.0.lock().unwrap(),
            vec![
                (0, total, "payload.bin".into()),
                (13, total, "payload.bin".into()),
                (13, total, "empty".into()),
                (13, 13, String::new()),
            ],
            "{kind}"
        );
    }
}

#[test]
fn native_entry_tests_propagate_stream_cancellation_and_preserve_damage_details() {
    for kind in ["zip", "local-zip", "tar"] {
        for failure in [Failure::Cancelled, Failure::Damaged] {
            let (bytes, payload_offset) = fixture(kind);
            let armed = Arc::new(AtomicBool::new(false));
            let Detected::Archive(format) = squallz_formats::registry()
                .detect_by_name(if kind == "tar" { "a.tar" } else { "a.zip" })
                .unwrap()
            else {
                panic!("expected archive")
            };
            let control = ControlToken::default();
            let mut reader = format
                .open(
                    Box::new(FaultReader {
                        input: Cursor::new(bytes),
                        payload_offset,
                        armed: armed.clone(),
                        failure,
                    }),
                    &OpenOptions::default(),
                    &control,
                )
                .unwrap();
            armed.store(true, Ordering::SeqCst);
            let result = reader.test_summary(&SafetyLimits::default(), &Trace::default(), &control);
            assert!(
                !armed.load(Ordering::SeqCst),
                "{kind}: fault was not exercised"
            );
            match failure {
                Failure::Cancelled => assert!(
                    matches!(result, Err(FormatError::Cancelled)),
                    "{kind}: {result:?}"
                ),
                Failure::Damaged => assert_eq!(
                    result.unwrap().problems.messages,
                    vec!["payload.bin: fixture read failure"],
                    "{kind}"
                ),
                Failure::Interrupted => unreachable!(),
            }
        }
    }
}
