//! LZ4 frame-format single-stream compressor (lz4_flex, pure Rust).

use std::io::{self, Read, Write};

use lz4_flex::frame::{FrameDecoder, FrameEncoder};
use squallz_format_api::{
    CompressSink, CompressionLevel, Compressor, FormatError, ResourceOptions,
};

use super::EncoderSink;

/// The lz4 format.
pub(crate) struct Lz4;

impl Compressor for Lz4 {
    fn id(&self) -> &'static str {
        "lz4"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["lz4"]
    }

    fn sniff(&self, head: &[u8]) -> bool {
        head.starts_with(&[0x04, 0x22, 0x4D, 0x18])
    }

    fn compress_writer<'w>(
        &self,
        dst: Box<dyn Write + Send + 'w>,
        _level: CompressionLevel,
        _res: &ResourceOptions,
    ) -> Result<Box<dyn CompressSink + 'w>, FormatError> {
        // lz4_flex implements the fast mode only; levels map to the single
        // setting (docs/level-mapping.md).
        let encoder = FrameEncoder::new(dst);
        Ok(EncoderSink::boxed(encoder, |e| {
            e.finish().map(drop).map_err(io::Error::other)
        }))
    }

    fn decompress_reader<'r>(
        &self,
        src: Box<dyn Read + Send + 'r>,
    ) -> Result<Box<dyn Read + Send + 'r>, FormatError> {
        Ok(Box::new(FrameDecoder::new(src)))
    }
}
