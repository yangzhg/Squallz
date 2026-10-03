//! Indexed block access keeps metadata reads out of unrelated solid blocks.

use std::collections::HashSet;
use std::io::{self, Read};

use sevenz_rust2::{Archive, ArchiveEntry, BlockDecoder, Error, Password};
use squallz_format_api::ReadSeek;

// The parallel LZMA2 decoder buffers complete reset intervals, which can span
// the whole solid block. The incremental decoder keeps reads and cancellation
// bounded without materializing the block before its first byte is consumed.
const DECODER_THREADS: u32 = 1;

struct EntrySizeReader<'r> {
    inner: &'r mut dyn Read,
    remaining: u64,
}

impl Read for EntrySizeReader<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        let read = self.inner.read(buffer)?;
        if read == 0 && self.remaining != 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "7z entry stream ended before its declared size",
            ));
        }
        self.remaining = self.remaining.checked_sub(read as u64).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "7z entry stream exceeded its declared size",
            )
        })?;
        Ok(read)
    }
}

pub(super) struct EntryStreams {
    source: Box<dyn ReadSeek>,
    archive: Archive,
    password: Password,
}

impl EntryStreams {
    pub(super) fn new(mut source: Box<dyn ReadSeek>, password: Password) -> Result<Self, Error> {
        let archive = Archive::read(&mut source, &password)?;
        Ok(Self {
            source,
            archive,
            password,
        })
    }

    pub(super) fn archive(&self) -> &Archive {
        &self.archive
    }

    pub(super) fn for_each_entries(
        &mut self,
        each: impl FnMut(&ArchiveEntry, &mut dyn Read) -> Result<bool, Error>,
    ) -> Result<(), Error> {
        self.visit(None, true, each)
    }

    pub(super) fn for_each_in_blocks(
        &mut self,
        blocks: Option<&HashSet<usize>>,
        each: impl FnMut(&ArchiveEntry, &mut dyn Read) -> Result<bool, Error>,
    ) -> Result<(), Error> {
        self.visit(blocks, false, each)
    }

    /// Entries in unused blocks still receive a metadata callback, with no
    /// decryption or decompression. Such callbacks must not request content.
    pub(super) fn for_each_entries_with_data_blocks(
        &mut self,
        blocks: &HashSet<usize>,
        each: impl FnMut(&ArchiveEntry, &mut dyn Read) -> Result<bool, Error>,
    ) -> Result<(), Error> {
        self.visit(Some(blocks), true, each)
    }

    fn visit(
        &mut self,
        blocks: Option<&HashSet<usize>>,
        include_metadata: bool,
        mut each: impl FnMut(&ArchiveEntry, &mut dyn Read) -> Result<bool, Error>,
    ) -> Result<(), Error> {
        for index in 0..self.archive.blocks.len() {
            if blocks.is_some_and(|blocks| !blocks.contains(&index)) {
                if include_metadata {
                    let start = self
                        .archive
                        .stream_map
                        .block_first_file_index
                        .get(index)
                        .ok_or_else(|| Error::Other("7z block is missing its first file".into()))?;
                    let entries =
                        self.archive.files.get(*start..).ok_or_else(|| {
                            Error::Other("7z block references missing files".into())
                        })?;
                    for (offset, entry) in entries.iter().enumerate() {
                        if self.archive.stream_map.file_block_index.get(start + offset)
                            != Some(&Some(index))
                        {
                            break;
                        }
                        if !each(entry, &mut std::io::empty())? {
                            break;
                        }
                    }
                }
                continue;
            }
            let complete = BlockDecoder::new(
                DECODER_THREADS,
                index,
                &self.archive,
                &self.password,
                &mut self.source,
            )
            .for_each_entries(&mut |entry, data| {
                let mut checked = EntrySizeReader {
                    inner: data,
                    remaining: entry.size(),
                };
                each(entry, &mut checked)
            })?;
            // A complete archive visit stops globally. Selected-block visits
            // use false to skip the remainder of only the current block.
            if !complete && blocks.is_none() && include_metadata {
                return Ok(());
            }
        }
        if include_metadata {
            for (index, entry) in self.archive.files.iter().enumerate() {
                if self.archive.stream_map.file_block_index.get(index) == Some(&None)
                    && !each(entry, &mut std::io::empty())?
                {
                    break;
                }
            }
        }
        Ok(())
    }
}
