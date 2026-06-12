//! Brotli single-stream compressor (brotli crate, pure Rust).

use std::io::{Read, Write};

use squallz_format_api::{
    CompressSink, CompressionLevel, Compressor, FormatError, ResourceOptions,
};

use super::EncoderSink;

/// Internal encoder/decoder buffer size.
const BUFFER_SIZE: usize = 32 * 1024;
/// Window size (log2); 22 is the brotli default.
const LG_WINDOW: u32 = 22;

/// The brotli format.
pub(crate) struct Brotli;

/// Quality mapping 0–11 (docs/level-mapping.md). Quality 0 is the fastest
/// setting — brotli has no stored mode.
pub(super) fn quality(level: CompressionLevel) -> u32 {
    match level {
        CompressionLevel::Store => 0,
        CompressionLevel::Fastest => 1,
        CompressionLevel::Fast => 4,
        CompressionLevel::Normal => 6,
        CompressionLevel::Maximum => 9,
        CompressionLevel::Ultra => 11,
    }
}

impl Compressor for Brotli {
    fn id(&self) -> &'static str {
        "brotli"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["br"]
    }

    // No `sniff` override: the brotli stream format has no reliable magic
    // number, so detection stays extension-based (trait default `false`).

    fn compress_writer<'w>(
        &self,
        dst: Box<dyn Write + Send + 'w>,
        level: CompressionLevel,
        _res: &ResourceOptions,
    ) -> Result<Box<dyn CompressSink + 'w>, FormatError> {
        let encoder = brotli::CompressorWriter::new(dst, BUFFER_SIZE, quality(level), LG_WINDOW);
        Ok(EncoderSink::boxed(encoder, |e| {
            // `into_inner` emits the trailing FINISH block before handing
            // the destination back; flush it to surface late I/O errors.
            let mut inner = e.into_inner();
            inner.flush()
        }))
    }

    fn decompress_reader<'r>(
        &self,
        src: Box<dyn Read + Send + 'r>,
    ) -> Result<Box<dyn Read + Send + 'r>, FormatError> {
        Ok(Box::new(brotli::Decompressor::new(src, BUFFER_SIZE)))
    }
}
