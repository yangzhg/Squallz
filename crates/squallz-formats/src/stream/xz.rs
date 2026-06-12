//! XZ single-stream compressor (liblzma crate — the maintained successor of
//! the abandoned xz2; the unsafe FFI stays inside liblzma-sys).

use std::io::{Read, Write};

use liblzma::read::XzDecoder;
use liblzma::write::XzEncoder;
use squallz_format_api::{
    CompressSink, CompressionLevel, Compressor, FormatError, ResourceOptions,
};

use super::EncoderSink;

/// The xz format.
pub(crate) struct Xz;

/// Preset mapping 0–9 (docs/level-mapping.md). Preset 0 is xz's lightest
/// compression — the format has no stored mode.
pub(super) fn preset(level: CompressionLevel) -> u32 {
    match level {
        CompressionLevel::Store => 0,
        CompressionLevel::Fastest => 1,
        CompressionLevel::Fast => 3,
        CompressionLevel::Normal => 6,
        CompressionLevel::Maximum => 8,
        CompressionLevel::Ultra => 9,
    }
}

impl Compressor for Xz {
    fn id(&self) -> &'static str {
        "xz"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["xz"]
    }

    fn sniff(&self, head: &[u8]) -> bool {
        head.starts_with(&[0xFD, 0x37, 0x7A, 0x58, 0x5A, 0x00])
    }

    fn compress_writer<'w>(
        &self,
        dst: Box<dyn Write + Send + 'w>,
        level: CompressionLevel,
        _res: &ResourceOptions,
    ) -> Result<Box<dyn CompressSink + 'w>, FormatError> {
        let encoder = XzEncoder::new(dst, preset(level));
        Ok(EncoderSink::boxed(encoder, |e| e.finish().map(drop)))
    }

    fn decompress_reader<'r>(
        &self,
        src: Box<dyn Read + Send + 'r>,
    ) -> Result<Box<dyn Read + Send + 'r>, FormatError> {
        // Multi-stream decoder: `xz` supports concatenated streams and
        // stream padding.
        Ok(Box::new(XzDecoder::new_multi_decoder(src)))
    }
}
