//! Compound and single-stream format plumbing for the engine:
//! stream factories, the single-entry virtual archive over a plain
//! compressed file (`x.gz`), and the shared write-side sink that lets a
//! streaming archive writer (tar) feed a compressor without a temp file.

use std::io::{Read, Write};
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};

use crate::api::{
    ArchiveReader, CompressSink, Compressor, ControlToken, EntryMeta, EntryPath, EntryType,
    FormatError, ProgressSink, SafetyLimits, StreamFactory, TestReport,
};
use crate::Source;

/// Builds a factory that re-opens the source (single file or volume set)
/// and wraps it into the compressor's decoding reader; each call restarts
/// the decompressed stream.
pub(crate) fn decompress_factory(
    source: &Source,
    compressor: Arc<dyn Compressor>,
) -> StreamFactory {
    let source = source.clone();
    Box::new(move || {
        let stream = source.open_stream()?;
        compressor.decompress_reader(Box::new(stream))
    })
}

/// Single-entry virtual archive over a plain compressed file (`x.gz`):
/// the one entry is the decompressed payload, named after the file with the
/// compression extension stripped.
pub(crate) struct SingleFileArchiveReader {
    factory: StreamFactory,
    meta: EntryMeta,
}

impl SingleFileArchiveReader {
    /// `size_hint` comes from [`Compressor::uncompressed_size_hint`]
    /// (0 when unknown — header sizes are untrusted anyway).
    pub(crate) fn new(path: &Path, factory: StreamFactory, size_hint: u64) -> Self {
        let name = match path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .filter(|s| !s.is_empty())
        {
            Some(name) => name,
            None => "data".to_string(),
        };
        let fs_meta = std::fs::metadata(path).ok();
        let meta = EntryMeta {
            path: EntryPath::from_utf8(name),
            entry_type: EntryType::File,
            size: size_hint,
            compressed_size: fs_meta.as_ref().map(|m| m.len()),
            modified: fs_meta.and_then(|m| m.modified().ok()),
            unix_mode: None,
            crc32: None,
            encrypted: false,
        };
        Self { factory, meta }
    }
}

impl ArchiveReader for SingleFileArchiveReader {
    fn entries(&mut self) -> Box<dyn Iterator<Item = Result<EntryMeta, FormatError>> + '_> {
        Box::new(std::iter::once(Ok(self.meta.clone())))
    }

    fn read_entry(&mut self, path: &EntryPath) -> Result<Box<dyn Read + '_>, FormatError> {
        if path.raw != self.meta.path.raw {
            return Err(FormatError::Other(format!("entry not found: {path}")));
        }
        Ok((self.factory)()?)
    }

    fn test(
        &mut self,
        progress: &dyn ProgressSink,
        ctl: &ControlToken,
    ) -> Result<TestReport, FormatError> {
        // Reading to EOF drives the backend's integrity checks (gzip CRC32,
        // xz check, zstd frame checksums). Default limits guard against
        // bombs during the test itself.
        let mut report = TestReport {
            entries_tested: 1,
            problems: Vec::new(),
            recovery: None,
        };
        let mut accountant = crate::api::LimitsAccountant::new(SafetyLimits::default());
        let mut reader = (self.factory)()?;
        let mut buf = vec![0u8; 64 * 1024];
        let mut done = 0u64;
        loop {
            ctl.checkpoint()?;
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    accountant.add_output_bytes(n as u64)?;
                    done += n as u64;
                    progress.on_progress(done, 0, &self.meta.path);
                }
                Err(e) => {
                    report.problems.push(format!("{}: {e}", self.meta.path));
                    break;
                }
            }
        }
        progress.on_progress(done, done, &EntryPath::from_utf8(""));
        Ok(report)
    }
}

/// Cloneable [`Write`] handle over one compressor sink. The streaming
/// archive writer (tar) owns one clone while the engine keeps another to
/// finish the compressed stream after the archive writer is done.
#[derive(Clone)]
pub(crate) struct SharedCompressSink(Arc<Mutex<Box<dyn CompressSink>>>);

impl SharedCompressSink {
    pub(crate) fn new(sink: Box<dyn CompressSink>) -> Self {
        Self(Arc::new(Mutex::new(sink)))
    }

    /// Finishes the compressed stream (trailing format structures).
    pub(crate) fn finish(&self) -> Result<(), FormatError> {
        let mut sink = lock_compress_sink(&self.0);
        sink.finish()
    }
}

impl Write for SharedCompressSink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let mut sink = lock_compress_sink(&self.0);
        sink.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        let mut sink = lock_compress_sink(&self.0);
        sink.flush()
    }
}

fn lock_compress_sink(
    inner: &Mutex<Box<dyn CompressSink>>,
) -> MutexGuard<'_, Box<dyn CompressSink>> {
    match inner.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// Progress adapter supplying a known total to sinks fed by sources that
/// report `(done, 0)` (the compressor pumps).
pub(crate) struct KnownTotal<'a> {
    inner: &'a dyn ProgressSink,
    total: u64,
    label: EntryPath,
}

impl<'a> KnownTotal<'a> {
    pub(crate) fn new(inner: &'a dyn ProgressSink, total: u64, label: EntryPath) -> Self {
        Self {
            inner,
            total,
            label,
        }
    }
}

impl ProgressSink for KnownTotal<'_> {
    fn on_progress(&self, done: u64, _total: u64, _current: &EntryPath) {
        let done = done.min(self.total);
        self.inner
            .on_entry_progress(done, self.total, &self.label, done, self.total);
    }
}

/// Reader adapter that reports byte-granular progress and honours
/// cancellation while an archive writer copies it (smooth progress on large
/// files; used for both file inputs and entry-to-entry conversion).
pub(crate) struct ProgressRead<'a, R: Read> {
    inner: R,
    progress: &'a dyn ProgressSink,
    ctl: &'a ControlToken,
    name: &'a EntryPath,
    base: u64,
    total: u64,
    current_total: u64,
    read: u64,
}

impl<'a, R: Read> ProgressRead<'a, R> {
    pub(crate) fn new(
        inner: R,
        progress: &'a dyn ProgressSink,
        ctl: &'a ControlToken,
        name: &'a EntryPath,
        base: u64,
        total: u64,
        current_total: u64,
    ) -> Self {
        Self {
            inner,
            progress,
            ctl,
            name,
            base,
            total,
            current_total,
            read: 0,
        }
    }
}

impl<R: Read> Read for ProgressRead<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        // A cancelled token surfaces as an I/O error here; the engine maps
        // it back to FormatError::Cancelled at the call site.
        self.ctl.checkpoint().map_err(std::io::Error::other)?;
        let n = self.inner.read(buf)?;
        self.read += n as u64;
        self.progress.on_entry_progress(
            self.base + self.read,
            self.total,
            self.name,
            self.read.min(self.current_total),
            self.current_total,
        );
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::sync::atomic::{AtomicBool, Ordering};

    fn stream_factory(bytes: &'static [u8]) -> StreamFactory {
        Box::new(move || Ok(Box::new(Cursor::new(bytes)) as Box<dyn Read + Send>))
    }

    #[test]
    fn single_file_virtual_archive_uses_data_fallback_for_missing_stem() {
        let mut reader = SingleFileArchiveReader::new(Path::new("/"), stream_factory(b"abc"), 3);
        let meta = reader
            .entries()
            .next()
            .expect("single virtual entry")
            .expect("entry meta");
        assert_eq!(meta.path.display, "data");

        let mut entry = reader.read_entry(&meta.path).unwrap();
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"abc");
    }

    struct RecordingSink {
        bytes: Arc<Mutex<Vec<u8>>>,
        finished: Arc<AtomicBool>,
    }

    impl Write for RecordingSink {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.bytes.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl CompressSink for RecordingSink {
        fn finish(&mut self) -> Result<(), FormatError> {
            self.finished.store(true, Ordering::Relaxed);
            Ok(())
        }
    }

    #[test]
    fn shared_compress_sink_recovers_after_poisoned_lock() {
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let finished = Arc::new(AtomicBool::new(false));
        let shared = SharedCompressSink::new(Box::new(RecordingSink {
            bytes: Arc::clone(&bytes),
            finished: Arc::clone(&finished),
        }));
        let poison_target = shared.clone();
        let join = std::thread::spawn(move || {
            let _guard = poison_target.0.lock().unwrap();
            panic!("poison shared compress sink");
        })
        .join();
        assert!(join.is_err());

        let mut writer = shared.clone();
        writer.write_all(b"abc").unwrap();
        shared.finish().unwrap();

        assert_eq!(*bytes.lock().unwrap(), b"abc");
        assert!(finished.load(Ordering::Relaxed));
    }
}
