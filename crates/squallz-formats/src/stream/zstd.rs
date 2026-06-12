//! Zstandard single-stream compressor (zstd crate; the unsafe FFI stays
//! inside zstd-sys).

use std::io::{Read, Write};

use squallz_format_api::{
    CompressSink, CompressionLevel, Compressor, FormatError, ResourceOptions,
};

use super::EncoderSink;

/// The zstd format.
pub(crate) struct Zstd;

/// Level mapping 1–19 of the 1–22 range (docs/level-mapping.md); zstd has
/// no stored mode.
pub(super) fn zstd_level(level: CompressionLevel) -> i32 {
    match level {
        CompressionLevel::Store | CompressionLevel::Fastest => 1,
        CompressionLevel::Fast => 2,
        CompressionLevel::Normal => 3,
        CompressionLevel::Maximum => 12,
        CompressionLevel::Ultra => 19,
    }
}

impl Compressor for Zstd {
    fn id(&self) -> &'static str {
        "zstd"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["zst"]
    }

    fn sniff(&self, head: &[u8]) -> bool {
        head.starts_with(&[0x28, 0xB5, 0x2F, 0xFD])
    }

    fn compress_writer<'w>(
        &self,
        dst: Box<dyn Write + Send + 'w>,
        level: CompressionLevel,
        res: &ResourceOptions,
    ) -> Result<Box<dyn CompressSink + 'w>, FormatError> {
        let mut encoder = zstd::stream::write::Encoder::new(dst, zstd_level(level))?;
        if let Some(threads) = res.threads {
            encoder.multithread(threads.max(1).min(u32::MAX as usize) as u32)?;
        }
        Ok(EncoderSink::boxed(encoder, |e| e.finish().map(drop)))
    }

    fn decompress_reader<'r>(
        &self,
        src: Box<dyn Read + Send + 'r>,
    ) -> Result<Box<dyn Read + Send + 'r>, FormatError> {
        // The reader decodes concatenated frames until EOF by default.
        Ok(Box::new(zstd::stream::read::Decoder::new(src)?))
    }
}
