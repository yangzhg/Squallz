//! BZIP2 single-stream compressor (bzip2 crate, pure-Rust libbz2-rs
//! backend).

use std::io::{Read, Write};

use bzip2::read::MultiBzDecoder;
use bzip2::write::BzEncoder;
use bzip2::Compression;
use squallz_format_api::{
    CompressSink, CompressionLevel, Compressor, FormatError, ResourceOptions,
};

use super::EncoderSink;

/// The bzip2 format.
pub(crate) struct Bzip2;

/// Block-size mapping 1–9 (docs/level-mapping.md). bzip2 has no stored
/// mode, so `Store` falls back to the fastest setting.
pub(super) fn block_level(level: CompressionLevel) -> u32 {
    match level {
        CompressionLevel::Store | CompressionLevel::Fastest => 1,
        CompressionLevel::Fast => 3,
        CompressionLevel::Normal => 6,
        CompressionLevel::Maximum | CompressionLevel::Ultra => 9,
    }
}

impl Compressor for Bzip2 {
    fn id(&self) -> &'static str {
        "bzip2"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["bz2"]
    }

    fn sniff(&self, head: &[u8]) -> bool {
        // `BZh` followed by the block-size digit 1-9.
        head.len() >= 4 && head.starts_with(b"BZh") && (b'1'..=b'9').contains(&head[3])
    }

    fn compress_writer<'w>(
        &self,
        dst: Box<dyn Write + Send + 'w>,
        level: CompressionLevel,
        _res: &ResourceOptions,
    ) -> Result<Box<dyn CompressSink + 'w>, FormatError> {
        let encoder = BzEncoder::new(dst, Compression::new(block_level(level)));
        Ok(EncoderSink::boxed(encoder, |e| e.finish().map(drop)))
    }

    fn decompress_reader<'r>(
        &self,
        src: Box<dyn Read + Send + 'r>,
    ) -> Result<Box<dyn Read + Send + 'r>, FormatError> {
        Ok(Box::new(MultiBzDecoder::new(src)))
    }
}
