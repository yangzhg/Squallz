//! Single-stream compressors (gzip/bzip2/xz/zstd/lz4/brotli).
//!
//! Each backend only provides the two stream-wrapping constructors of the
//! [`squallz_format_api::Compressor`] trait; the chunked pumps with
//! cancellation, progress and the decompression-bomb guardrail are derived
//! in the trait's default methods. Level mappings are documented in
//! docs/level-mapping.md.

mod brotli;
mod bzip2;
mod gzip;
mod lz4;
mod xz;
mod zstd;

pub(crate) use self::brotli::Brotli;
pub(crate) use self::bzip2::Bzip2;
pub(crate) use self::gzip::Gzip;
pub(crate) use self::lz4::Lz4;
pub(crate) use self::xz::Xz;
pub(crate) use self::zstd::Zstd;

use std::io::{self, Write};

use squallz_format_api::{CompressSink, FormatError};

/// Generic [`CompressSink`] adapter: owns the backend encoder and finishes
/// it through a consuming function (all backends finish by value; brotli
/// finishes on drop, wrapped the same way).
pub(crate) struct EncoderSink<E: Write + Send> {
    encoder: Option<E>,
    finish_fn: fn(E) -> io::Result<()>,
}

impl<E: Write + Send> EncoderSink<E> {
    /// Boxes an encoder together with its consuming finish function.
    pub(crate) fn boxed<'w>(
        encoder: E,
        finish_fn: fn(E) -> io::Result<()>,
    ) -> Box<dyn CompressSink + 'w>
    where
        E: 'w,
    {
        Box::new(Self {
            encoder: Some(encoder),
            finish_fn,
        })
    }

    fn encoder(&mut self) -> io::Result<&mut E> {
        self.encoder
            .as_mut()
            .ok_or_else(|| io::Error::other("compressed stream already finished"))
    }
}

impl<E: Write + Send> Write for EncoderSink<E> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.encoder()?.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.encoder()?.flush()
    }
}

impl<E: Write + Send> CompressSink for EncoderSink<E> {
    fn finish(&mut self) -> Result<(), FormatError> {
        let encoder = self
            .encoder
            .take()
            .ok_or_else(|| FormatError::Other("compressed stream already finished".into()))?;
        (self.finish_fn)(encoder).map_err(FormatError::Io)
    }
}

#[cfg(test)]
mod tests {
    use std::io;

    use squallz_format_api::{CompressionLevel, Compressor};

    use super::*;

    const LEVELS: [CompressionLevel; 6] = [
        CompressionLevel::Store,
        CompressionLevel::Fastest,
        CompressionLevel::Fast,
        CompressionLevel::Normal,
        CompressionLevel::Maximum,
        CompressionLevel::Ultra,
    ];

    #[test]
    fn level_mappings_match_documented_stream_presets() {
        assert_eq!(
            LEVELS.map(gzip::deflate_level),
            [0, 1, 3, 6, 8, 9],
            "gzip deflate levels must match docs/level-mapping.md"
        );
        assert_eq!(
            LEVELS.map(bzip2::block_level),
            [1, 1, 3, 6, 9, 9],
            "bzip2 block levels must match docs/level-mapping.md"
        );
        assert_eq!(
            LEVELS.map(xz::preset),
            [0, 1, 3, 6, 8, 9],
            "xz presets must match docs/level-mapping.md"
        );
        assert_eq!(
            LEVELS.map(zstd::zstd_level),
            [1, 1, 2, 3, 12, 19],
            "zstd levels must match docs/level-mapping.md"
        );
        assert_eq!(
            LEVELS.map(brotli::quality),
            [0, 1, 4, 6, 9, 11],
            "brotli qualities must match docs/level-mapping.md"
        );
    }

    #[test]
    fn stream_sniffers_accept_only_declared_magic_boundaries() {
        assert!(Gzip.sniff(&[0x1F, 0x8B, 0x08, 0x00]));
        assert!(!Gzip.sniff(&[0x1F]));
        assert!(!Gzip.sniff(&[0x1F, 0x00]));

        assert!(Bzip2.sniff(b"BZh9payload"));
        assert!(!Bzip2.sniff(b"BZh0payload"));
        assert!(!Bzip2.sniff(b"BZh"));

        assert!(Xz.sniff(&[0xFD, 0x37, 0x7A, 0x58, 0x5A, 0x00, 0x00]));
        assert!(!Xz.sniff(&[0xFD, 0x37, 0x7A, 0x58, 0x5A]));

        assert!(Zstd.sniff(&[0x28, 0xB5, 0x2F, 0xFD, 0x00]));
        assert!(!Zstd.sniff(&[0x28, 0xB5, 0x2F]));

        assert!(Lz4.sniff(&[0x04, 0x22, 0x4D, 0x18, 0x40]));
        assert!(!Lz4.sniff(&[0x04, 0x22, 0x4D]));

        assert!(!Brotli.sniff(b"brotli has no stable magic"));
    }

    #[test]
    fn encoder_sink_finish_is_single_use_and_reports_late_io_errors() {
        let mut sink = EncoderSink::boxed(Vec::<u8>::new(), |_encoder| Ok(()));
        sink.finish().expect("first finish succeeds");
        let err = sink.finish().expect_err("second finish is rejected");
        assert!(
            matches!(err, FormatError::Other(ref message) if message.contains("already finished")),
            "expected single-use finish error, got {err:?}"
        );

        let mut sink = EncoderSink::boxed(Vec::<u8>::new(), |_encoder| {
            Err(io::Error::other("late encoder failure"))
        });
        let err = sink.finish().expect_err("late encoder error is surfaced");
        assert!(
            matches!(err, FormatError::Io(ref error) if error.to_string().contains("late encoder failure")),
            "expected late io error, got {err:?}"
        );
    }
}
