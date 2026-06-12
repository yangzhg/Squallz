//! GZIP single-stream compressor (flate2, pure-Rust miniz_oxide backend).

use std::io::{Read, SeekFrom, Write};

use flate2::read::MultiGzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use squallz_format_api::{
    CompressSink, CompressionLevel, Compressor, FormatError, ReadSeek, ResourceOptions,
};

use super::EncoderSink;

/// Minimum size of a non-empty gzip member (10-byte header + 8-byte trailer).
const MIN_GZIP_LEN: u64 = 18;

/// The gzip format.
pub(crate) struct Gzip;

/// Deflate level mapping (docs/level-mapping.md). Level 0 stores members
/// uncompressed but keeps the gzip framing.
pub(super) fn deflate_level(level: CompressionLevel) -> u32 {
    match level {
        CompressionLevel::Store => 0,
        CompressionLevel::Fastest => 1,
        CompressionLevel::Fast => 3,
        CompressionLevel::Normal => 6,
        CompressionLevel::Maximum => 8,
        CompressionLevel::Ultra => 9,
    }
}

impl Compressor for Gzip {
    fn id(&self) -> &'static str {
        "gzip"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["gz"]
    }

    fn sniff(&self, head: &[u8]) -> bool {
        head.starts_with(&[0x1F, 0x8B])
    }

    fn compress_writer<'w>(
        &self,
        dst: Box<dyn Write + Send + 'w>,
        level: CompressionLevel,
        _res: &ResourceOptions,
    ) -> Result<Box<dyn CompressSink + 'w>, FormatError> {
        let encoder = GzEncoder::new(dst, Compression::new(deflate_level(level)));
        Ok(EncoderSink::boxed(encoder, |e| e.finish().map(drop)))
    }

    fn decompress_reader<'r>(
        &self,
        src: Box<dyn Read + Send + 'r>,
    ) -> Result<Box<dyn Read + Send + 'r>, FormatError> {
        // Multi-member decoder: `gzip` happily concatenates members.
        Ok(Box::new(MultiGzDecoder::new(src)))
    }

    /// Reads the ISIZE trailer (uncompressed size mod 2^32). Only a hint:
    /// multi-member files and >4 GiB payloads report it short, and a hostile
    /// file can claim anything — consumers must keep their guardrails on.
    fn uncompressed_size_hint(&self, src: &mut dyn ReadSeek) -> Option<u64> {
        let hint = read_isize(src);
        let _ = src.seek(SeekFrom::Start(0));
        hint
    }
}

fn read_isize(src: &mut dyn ReadSeek) -> Option<u64> {
    let len = src.seek(SeekFrom::End(0)).ok()?;
    if len < MIN_GZIP_LEN {
        return None;
    }
    src.seek(SeekFrom::End(-4)).ok()?;
    let mut buf = [0u8; 4];
    src.read_exact(&mut buf).ok()?;
    Some(u64::from(u32::from_le_bytes(buf)))
}
