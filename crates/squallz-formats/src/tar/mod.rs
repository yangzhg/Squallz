//! TAR format (backed by the pure-Rust `tar` crate): list/extract/create/
//! test, Unix permissions, symlinks and hardlink entries.
//!
//! tar is a forward-only stream format, so the reader supports both
//! seekable sources (plain `.tar`, rewound between passes) and restartable
//! streams (`.tar.gz` through [`ArchiveFormat::open_stream`] — the engine
//! re-creates the decompressed stream on demand, never touching a temp
//! file). Extraction is a single pass driven through the shared
//! [`squallz_format_api::ExtractSink`] safety engine.

mod reader;
mod writer;

use std::io::Write;

use squallz_format_api::{
    ArchiveFormat, ArchiveReader, ArchiveWriter, CreateOptions, FormatCapabilities, FormatError,
    OpenOptions, ReadSeek, StreamFactory, WriteSeek,
};

/// Offset of the `ustar` magic inside a tar header block.
const MAGIC_OFFSET: usize = 257;
/// POSIX magic is `ustar\0`, GNU magic is `ustar `; the shared prefix is
/// enough to tell tar apart.
const MAGIC: &[u8] = b"ustar";

/// The TAR archive format.
pub(crate) struct TarFormat;

impl TarFormat {
    /// tar has no encryption; reject creation with a password instead of
    /// silently writing plaintext.
    fn check_create_opts(&self, opts: &CreateOptions) -> Result<(), FormatError> {
        if opts.password.is_some() {
            return Err(FormatError::Unsupported(
                "format tar does not support encryption".into(),
            ));
        }
        Ok(())
    }
}

impl ArchiveFormat for TarFormat {
    fn id(&self) -> &'static str {
        "tar"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["tar"]
    }

    fn capabilities(&self) -> FormatCapabilities {
        FormatCapabilities {
            can_create: true,
            can_extract: true,
            can_encrypt_data: false,
            can_encrypt_names: false,
            can_split: true, // engine-side `.001` byte splitting
            can_update: false,
            can_test: true,
        }
    }

    fn sniff(&self, head: &[u8], _tail: &[u8]) -> bool {
        head.len() >= MAGIC_OFFSET + MAGIC.len()
            && &head[MAGIC_OFFSET..MAGIC_OFFSET + MAGIC.len()] == MAGIC
    }

    fn open(
        &self,
        src: Box<dyn ReadSeek>,
        _opts: &OpenOptions,
    ) -> Result<Box<dyn ArchiveReader>, FormatError> {
        Ok(Box::new(reader::TarArchiveReader::seekable(src)))
    }

    fn open_stream(
        &self,
        source: StreamFactory,
        _opts: &OpenOptions,
    ) -> Result<Box<dyn ArchiveReader>, FormatError> {
        Ok(Box::new(reader::TarArchiveReader::streaming(source)))
    }

    fn create(
        &self,
        dst: Box<dyn WriteSeek>,
        opts: &CreateOptions,
    ) -> Result<Box<dyn ArchiveWriter>, FormatError> {
        self.check_create_opts(opts)?;
        Ok(Box::new(writer::TarArchiveWriter::new(dst)))
    }

    fn create_stream(
        &self,
        dst: Box<dyn Write + Send>,
        opts: &CreateOptions,
    ) -> Result<Box<dyn ArchiveWriter>, FormatError> {
        self.check_create_opts(opts)?;
        Ok(Box::new(writer::TarArchiveWriter::new(dst)))
    }
}

#[cfg(test)]
mod tests {
    use squallz_format_api::Password;

    use super::*;

    #[test]
    fn tar_format_declares_plain_archive_capabilities() {
        let format = TarFormat;

        assert_eq!(format.id(), "tar");
        assert_eq!(format.extensions(), &["tar"]);

        let capabilities = format.capabilities();
        assert!(capabilities.can_create);
        assert!(capabilities.can_extract);
        assert!(!capabilities.can_encrypt_data);
        assert!(!capabilities.can_encrypt_names);
        assert!(capabilities.can_split);
        assert!(!capabilities.can_update);
        assert!(capabilities.can_test);
    }

    #[test]
    fn tar_sniffer_accepts_ustar_magic_at_header_offset_only() {
        let format = TarFormat;
        let mut head = vec![0_u8; MAGIC_OFFSET + MAGIC.len()];
        head[MAGIC_OFFSET..MAGIC_OFFSET + MAGIC.len()].copy_from_slice(MAGIC);

        assert!(format.sniff(&head, b"ignored tail"));
        assert!(!format.sniff(&head[..MAGIC_OFFSET + MAGIC.len() - 1], b""));

        let mut wrong_offset = head.clone();
        wrong_offset[MAGIC_OFFSET - 1] = b'u';
        wrong_offset[MAGIC_OFFSET..MAGIC_OFFSET + MAGIC.len()].fill(0);
        assert!(!format.sniff(&wrong_offset, b""));
    }

    #[test]
    fn tar_create_options_reject_password_without_silent_plaintext() {
        let format = TarFormat;
        let mut opts = CreateOptions::default();

        assert!(format.check_create_opts(&opts).is_ok());

        opts.password = Some(Password::new("secret"));
        match format.check_create_opts(&opts) {
            Err(FormatError::Unsupported(message)) => {
                assert_eq!(message, "format tar does not support encryption");
            }
            other => panic!("expected Unsupported, got {other:?}"),
        }
    }
}
