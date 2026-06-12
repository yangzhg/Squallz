//! SQZ v1 container.
//!
//! S1 established a real transparent archive container with a fixed header,
//! footer index, BLAKE3-256 entry hashes and CRC-32C quick checks. S2 adds an
//! embedded Reed-Solomon recovery section over fixed-size payload blocks so
//! small payload damage can be repaired before entries are exposed.

use squallz_format_api::{
    ArchiveFormat, ArchiveReader, ArchiveWriter, CreateOptions, FormatCapabilities, FormatError,
    OpenOptions, ReadSeek, WriteSeek,
};

mod reader;
mod writer;

const MAGIC: &[u8; 8] = b"SQZARCH\x1A";
const TAIL_MAGIC: &[u8; 8] = b"\x1ASQZEND\n";
const INDEX_MAGIC: &[u8; 4] = b"FIDX";
const RECOVERY_MAGIC: &[u8; 4] = b"RSEC";
const RECOVERY_PROTECTION_MAGIC: &[u8; 4] = b"RSPC";
const HEADER_LEN: usize = 64;
const FOOTER_LEN: usize = 64;
const RECOVERY_PROTECTION_TRAILER_LEN: usize = 80;
const VERSION_MAJOR: u16 = 1;
const VERSION_MINOR: u16 = 0;
const BLOCK_SIZE: u32 = 64 * 1024;
const RECOVERY_VERSION: u16 = 1;
const RECOVERY_PROTECTION_VERSION: u16 = 1;
const RECOVERY_ALGO_RS_GF8: u16 = 1;
const RECOVERY_DATA_SHARDS: u16 = 8;
const RECOVERY_PARITY_SHARDS: u16 = 2;
const HEADER_FLAG_RECOVERY: u32 = 1 << 2;
const KIND_FILE: u8 = 0;
const KIND_DIR: u8 = 1;
const KIND_SYMLINK: u8 = 2;
const KIND_HARDLINK: u8 = 3;
const KIND_OTHER: u8 = 4;

/// Squallz native container format.
pub(crate) struct SqzFormat;

impl SqzFormat {
    fn check_create_opts(&self, opts: &CreateOptions) -> Result<(), FormatError> {
        if opts.password.is_some() || opts.encrypt_filenames {
            return Err(FormatError::Unsupported(
                "format sqz does not support encryption in v1 S1".into(),
            ));
        }
        if !matches!(
            opts.sqz.inner_format.as_str(),
            "sqz" | "zip" | "tar" | "7z" | "zstd"
        ) {
            return Err(FormatError::Unsupported(
                "SQZ v1 currently supports only inner-format sqz (entry-set), zip, tar, 7z, and zstd"
                    .into(),
            ));
        }
        Ok(())
    }
}

impl ArchiveFormat for SqzFormat {
    fn id(&self) -> &'static str {
        "sqz"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["sqz"]
    }

    fn capabilities(&self) -> FormatCapabilities {
        FormatCapabilities {
            can_create: true,
            can_extract: true,
            can_encrypt_data: false,
            can_encrypt_names: false,
            can_split: true,
            can_update: false,
            can_test: true,
        }
    }

    fn sniff(&self, head: &[u8], tail: &[u8]) -> bool {
        head.starts_with(MAGIC) || tail.ends_with(TAIL_MAGIC)
    }

    fn open(
        &self,
        src: Box<dyn ReadSeek>,
        opts: &OpenOptions,
    ) -> Result<Box<dyn ArchiveReader>, FormatError> {
        Ok(Box::new(reader::SqzArchiveReader::open(src, opts)?))
    }

    fn create(
        &self,
        dst: Box<dyn WriteSeek>,
        opts: &CreateOptions,
    ) -> Result<Box<dyn ArchiveWriter>, FormatError> {
        self.check_create_opts(opts)?;
        writer::create(dst, opts)
    }
}

fn crc32c_bytes(bytes: &[u8]) -> u32 {
    crc32c::crc32c(bytes)
}

fn empty_hash() -> [u8; 32] {
    *blake3::hash(&[]).as_bytes()
}

fn encoding_label(label: &str) -> &'static str {
    match label {
        "utf-8" | "UTF-8" => "utf-8",
        "GBK" | "gbk" | "cp936" | "CP936" => "GBK",
        "Shift_JIS" | "shift_jis" | "shift-jis" | "SHIFT_JIS" => "Shift_JIS",
        _ => "utf-8",
    }
}

fn put_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_u64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn read_exact<'a>(buf: &mut &'a [u8], len: usize) -> Result<&'a [u8], FormatError> {
    if buf.len() < len {
        return Err(FormatError::CorruptArchive(
            "truncated sqz footer index".into(),
        ));
    }
    let (head, tail) = buf.split_at(len);
    *buf = tail;
    Ok(head)
}

fn fixed_array<const N: usize>(
    bytes: &[u8],
    context: &'static str,
) -> Result<[u8; N], FormatError> {
    if bytes.len() != N {
        return Err(FormatError::CorruptArchive(format!(
            "{context} has invalid fixed width"
        )));
    }
    let mut out = [0u8; N];
    out.copy_from_slice(bytes);
    Ok(out)
}

fn read_array<const N: usize>(buf: &mut &[u8]) -> Result<[u8; N], FormatError> {
    fixed_array(read_exact(buf, N)?, "truncated sqz fixed-width field")
}

fn read_u8(buf: &mut &[u8]) -> Result<u8, FormatError> {
    Ok(read_exact(buf, 1)?[0])
}

fn read_u16(buf: &mut &[u8]) -> Result<u16, FormatError> {
    Ok(u16::from_le_bytes(read_array::<2>(buf)?))
}

fn read_u32(buf: &mut &[u8]) -> Result<u32, FormatError> {
    Ok(u32::from_le_bytes(read_array::<4>(buf)?))
}

fn read_u64(buf: &mut &[u8]) -> Result<u64, FormatError> {
    Ok(u64::from_le_bytes(read_array::<8>(buf)?))
}
