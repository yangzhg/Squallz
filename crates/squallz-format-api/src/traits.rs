//! The two core abstractions: single-stream compressors and archive
//! containers, plus their reader/writer handles.

use std::io::{Read, Seek, Write};
use std::path::Path;

use crate::entry::{EntryMeta, EntryPath};
use crate::error::FormatError;
use crate::options::{
    CompressionLevel, CreateOptions, ExtractOptions, FormatCapabilities, OpenOptions,
    ResourceOptions, SafetyLimits, TestReport, UpdateOp,
};
use crate::progress::{ControlToken, ProgressSink};
use crate::safety::LimitsAccountant;

/// Chunk size of the streaming pumps; cancellation, guardrails and progress
/// are honoured at this granularity.
const STREAM_CHUNK: usize = 64 * 1024;

/// Readable, seekable input stream.
pub trait ReadSeek: Read + Seek + Send {}
impl<T: Read + Seek + Send> ReadSeek for T {}

/// Writable, seekable output stream.
pub trait WriteSeek: Write + Seek + Send {}
impl<T: Write + Seek + Send> WriteSeek for T {}

/// Write-side encoder handle produced by [`Compressor::compress_writer`].
/// `finish` flushes the trailing format structures (in place, so boxed
/// sinks can be finished through `dyn`).
pub trait CompressSink: Write + Send {
    /// Finishes the compressed stream. Must be called exactly once; the
    /// sink must not be written to afterwards.
    fn finish(&mut self) -> Result<(), FormatError>;
}

/// Factory producing fresh sequential streams over the same source, used by
/// [`ArchiveFormat::open_stream`]. Compound formats (`.tar.gz`) cannot seek
/// in the decompressed stream, but the engine can always restart it from the
/// underlying file; each call returns a new stream positioned at the start.
pub type StreamFactory = Box<dyn Fn() -> Result<Box<dyn Read + Send>, FormatError> + Send + Sync>;

/// Abstraction one: single-stream compressor (gzip/bzip2/xz/zstd/lz4/brotli).
/// Has no notion of a "file list"; combined with tar it forms compound
/// formats such as `.tar.gz`.
///
/// Implementations provide the two stream-wrapping constructors
/// ([`Compressor::compress_writer`] / [`Compressor::decompress_reader`]);
/// the chunked pumps ([`Compressor::compress`] / [`Compressor::decompress`])
/// are derived from them, so every backend gets cancellation, progress and
/// the decompression-bomb guardrail for free.
pub trait Compressor: Send + Sync {
    /// Format identifier, e.g. `"gzip"`
    fn id(&self) -> &'static str;
    /// Extensions (without the dot), e.g. `["gz"]`
    fn extensions(&self) -> &'static [&'static str];

    /// Magic-number sniffing on the first bytes of a file. Used as the last
    /// detection step for extensionless compressed streams; formats without
    /// a reliable magic (brotli) keep the default `false`.
    fn sniff(&self, _head: &[u8]) -> bool {
        false
    }

    /// Wraps `dst` into an encoding sink. The caller streams plain data into
    /// the sink and must call [`CompressSink::finish`] at the end.
    fn compress_writer<'w>(
        &self,
        dst: Box<dyn Write + Send + 'w>,
        level: CompressionLevel,
        res: &ResourceOptions,
    ) -> Result<Box<dyn CompressSink + 'w>, FormatError>;

    /// Wraps `src` into a decoding reader yielding plain data. Output-byte
    /// guardrails are enforced by whoever consumes the reader (the pump
    /// below or the shared extraction sink).
    fn decompress_reader<'r>(
        &self,
        src: Box<dyn Read + Send + 'r>,
    ) -> Result<Box<dyn Read + Send + 'r>, FormatError>;

    /// Best-effort uncompressed size of a compressed file (e.g. the gzip
    /// ISIZE trailer). Implementations must rewind `src` to the start.
    /// `None` when the format does not record it.
    fn uncompressed_size_hint(&self, _src: &mut dyn ReadSeek) -> Option<u64> {
        None
    }

    /// Compresses one stream in 64 KiB chunks. Progress is reported as
    /// `(consumed_input_bytes, 0)` — the total is unknown at this level and
    /// supplied by engine-side wrappers.
    fn compress(
        &self,
        src: &mut (dyn Read + Send),
        dst: &mut (dyn Write + Send),
        level: CompressionLevel,
        res: &ResourceOptions,
        progress: &dyn ProgressSink,
        ctl: &ControlToken,
    ) -> Result<(), FormatError> {
        let mut sink = self.compress_writer(Box::new(dst), level, res)?;
        let label = EntryPath::from_utf8("");
        let mut buf = vec![0u8; res.stream_buffer_size(STREAM_CHUNK)?];
        let mut done = 0u64;
        loop {
            ctl.checkpoint()?;
            let n = src.read(&mut buf)?;
            if n == 0 {
                break;
            }
            sink.write_all(&buf[..n])?;
            done += n as u64;
            progress.on_progress(done, 0, &label);
        }
        sink.finish()
    }

    /// Decompresses one stream in 64 KiB chunks, charging every output byte
    /// against the guardrails. Progress is `(produced_output_bytes, 0)`.
    fn decompress(
        &self,
        src: &mut (dyn Read + Send),
        dst: &mut (dyn Write + Send),
        limits: &SafetyLimits,
        progress: &dyn ProgressSink,
        ctl: &ControlToken,
    ) -> Result<(), FormatError> {
        let mut reader = self.decompress_reader(Box::new(src))?;
        let mut accountant = LimitsAccountant::new(*limits);
        let label = EntryPath::from_utf8("");
        let mut buf = vec![0u8; STREAM_CHUNK];
        let mut done = 0u64;
        loop {
            ctl.checkpoint()?;
            let n = reader.read(&mut buf)?;
            if n == 0 {
                return Ok(());
            }
            accountant.add_output_bytes(n as u64)?;
            dst.write_all(&buf[..n])?;
            done += n as u64;
            progress.on_progress(done, 0, &label);
        }
    }
}

/// Abstraction two: archive container (zip/tar/7z/rar/iso/...).
pub trait ArchiveFormat: Send + Sync {
    /// Format identifier, e.g. `"zip"`
    fn id(&self) -> &'static str;
    /// Extensions (without the dot) including aliases, e.g.
    /// `["zip", "jar", "apk", "cbz"]`
    fn extensions(&self) -> &'static [&'static str];
    /// Capability declaration
    fn capabilities(&self) -> FormatCapabilities;
    /// Magic-number sniffing. `head` holds up to 512 bytes from the start of
    /// the file (the tar `ustar` magic lives at offset 257), `tail` up to
    /// 64 bytes from the end (ZIP keeps its central directory there and SFX
    /// archives start with MZ, hence both windows).
    fn sniff(&self, head: &[u8], tail: &[u8]) -> bool;
    /// Opens for reading.
    fn open(
        &self,
        src: Box<dyn ReadSeek>,
        opts: &OpenOptions,
    ) -> Result<Box<dyn ArchiveReader>, FormatError>;
    /// Opens for reading from a restartable sequential stream (no `Seek`).
    /// This is how compound formats (`.tar.gz`) are read without a temp
    /// file: the engine hands a factory that re-creates the decompressed
    /// stream on demand. Formats that require random access keep the
    /// default `Unsupported`.
    fn open_stream(
        &self,
        _source: StreamFactory,
        _opts: &OpenOptions,
    ) -> Result<Box<dyn ArchiveReader>, FormatError> {
        Err(FormatError::Unsupported(format!(
            "format {} cannot read from a non-seekable stream",
            self.id()
        )))
    }
    /// Creates for writing (returns `Unsupported` when `can_create=false`).
    fn create(
        &self,
        dst: Box<dyn WriteSeek>,
        opts: &CreateOptions,
    ) -> Result<Box<dyn ArchiveWriter>, FormatError>;
    /// Creates for writing into a forward-only sink (no `Seek`). This is how
    /// compound formats (`.tar.gz`) are written without a temp file: the
    /// destination is a live compression stream. Formats that must seek
    /// while writing keep the default `Unsupported`.
    fn create_stream(
        &self,
        _dst: Box<dyn Write + Send>,
        _opts: &CreateOptions,
    ) -> Result<Box<dyn ArchiveWriter>, FormatError> {
        Err(FormatError::Unsupported(format!(
            "format {} cannot write to a non-seekable stream",
            self.id()
        )))
    }
    /// Append/delete/rename (returns `Unsupported` when `can_update=false`).
    /// Implementation contract: write to a temporary file + atomic rename,
    /// pre-check disk space.
    fn update(
        &self,
        _src: &Path,
        _ops: &[UpdateOp],
        _opts: &CreateOptions,
        _progress: &dyn ProgressSink,
        _ctl: &ControlToken,
    ) -> Result<(), FormatError> {
        Err(FormatError::Unsupported(format!(
            "format {} cannot update existing archives",
            self.id()
        )))
    }
}

/// Read handle of an opened archive.
pub trait ArchiveReader: Send {
    /// Streams entry metadata (huge archives are never loaded wholesale).
    fn entries(&mut self) -> Box<dyn Iterator<Item = Result<EntryMeta, FormatError>> + '_>;

    /// Extracts all entries (or a selection) into `dest`.
    ///
    /// The default implementation is the shared safe extraction engine
    /// ([`crate::extract_entries`]): Zip-Slip rejection, decompression-bomb
    /// guardrails, symlink-breakout protection, overwrite/symlink policies
    /// and permission restore. Formats may override it for performance, but
    /// any override must uphold the same safety guarantees.
    fn extract(
        &mut self,
        dest: &Path,
        selection: Option<&[EntryPath]>,
        opts: &ExtractOptions,
        progress: &dyn ProgressSink,
        ctl: &ControlToken,
    ) -> Result<(), FormatError> {
        crate::extract::extract_entries(self, dest, selection, opts, progress, ctl)
    }

    /// Streams a single entry (GUI preview, nested archives, format
    /// conversion).
    fn read_entry(&mut self, path: &EntryPath) -> Result<Box<dyn Read + '_>, FormatError>;

    /// Integrity test.
    fn test(
        &mut self,
        progress: &dyn ProgressSink,
        ctl: &ControlToken,
    ) -> Result<TestReport, FormatError>;
}

/// Write handle of an archive being created.
pub trait ArchiveWriter: Send {
    /// Writes one entry; pass `None` for data-less entries
    /// (directories/symlinks).
    fn add_entry(
        &mut self,
        meta: &EntryMeta,
        data: Option<&mut dyn Read>,
    ) -> Result<(), FormatError>;
    /// Finishes writing (flushes trailing structures such as the central
    /// directory).
    fn finish(self: Box<Self>) -> Result<(), FormatError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    struct DummyArchiveFormat;

    impl ArchiveFormat for DummyArchiveFormat {
        fn id(&self) -> &'static str {
            "dummy"
        }

        fn extensions(&self) -> &'static [&'static str] {
            &["dummy"]
        }

        fn capabilities(&self) -> FormatCapabilities {
            FormatCapabilities::default()
        }

        fn sniff(&self, _head: &[u8], _tail: &[u8]) -> bool {
            false
        }

        fn open(
            &self,
            _src: Box<dyn ReadSeek>,
            _opts: &OpenOptions,
        ) -> Result<Box<dyn ArchiveReader>, FormatError> {
            Err(FormatError::Unsupported("dummy open".to_string()))
        }

        fn create(
            &self,
            _dst: Box<dyn WriteSeek>,
            _opts: &CreateOptions,
        ) -> Result<Box<dyn ArchiveWriter>, FormatError> {
            Err(FormatError::Unsupported("dummy create".to_string()))
        }
    }

    fn unsupported_message(result: Result<(), FormatError>) -> String {
        match result {
            Err(FormatError::Unsupported(message)) => message,
            other => panic!("expected unsupported error, got {other:?}"),
        }
    }

    #[test]
    fn archive_format_default_stream_and_update_errors_name_the_format() {
        let format = DummyArchiveFormat;

        let open_message = match format.open_stream(
            Box::new(|| Ok(Box::new(std::io::empty()) as Box<dyn Read + Send>)),
            &OpenOptions::default(),
        ) {
            Err(FormatError::Unsupported(message)) => message,
            _ => panic!("expected unsupported open_stream error"),
        };
        assert!(open_message.contains("dummy"));
        assert!(open_message.contains("non-seekable stream"));

        let create_message =
            match format.create_stream(Box::new(Vec::<u8>::new()), &CreateOptions::default()) {
                Err(FormatError::Unsupported(message)) => message,
                _ => panic!("expected unsupported create_stream error"),
            };
        assert!(create_message.contains("dummy"));
        assert!(create_message.contains("non-seekable stream"));

        let update_message = unsupported_message(format.update(
            Path::new("archive.dummy"),
            &[],
            &CreateOptions::default(),
            &crate::NoProgress,
            &ControlToken::default(),
        ));
        assert!(update_message.contains("dummy"));
        assert!(update_message.contains("update existing archives"));
    }
}
