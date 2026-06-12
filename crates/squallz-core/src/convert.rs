//! Format conversion: stream every entry of a source archive into a new
//! archive of the format chosen by the destination extension, reusing the
//! create-side format selection (compound pipelines included) without
//! extracting to disk.

use std::io::Read;
use std::path::Path;
use std::sync::Arc;

use crate::api::{
    ArchiveReader, Compressor, ControlToken, CreateOptions, EntryMeta, EntryPath, EntryType,
    FormatError, OpenOptions, ProgressSink,
};
use crate::compound::{KnownTotal, ProgressRead};
use crate::create::{open_dest, with_split_output, DestSink, DestTarget};
use crate::Engine;

/// Entry point for [`Engine::convert`]. Metadata is carried over as
/// faithfully as the destination format allows; entry types the destination
/// cannot store (e.g. symlinks in 7z) surface as
/// [`FormatError::Unsupported`] naming the offending entry.
pub(crate) fn convert(
    engine: &Engine,
    src: &Path,
    dest: &Path,
    open_opts: &OpenOptions,
    create_opts: &CreateOptions,
    progress: &dyn ProgressSink,
    ctl: &ControlToken,
) -> Result<(), FormatError> {
    let mut reader = engine.open(src, open_opts)?;
    let metas: Vec<EntryMeta> = reader.entries().collect::<Result<_, _>>()?;
    with_split_output(dest, create_opts, ctl, |detect_name, out_path, opts| {
        convert_unsplit(
            engine,
            &mut *reader,
            &metas,
            detect_name,
            out_path,
            opts,
            progress,
            ctl,
        )
    })
}

#[allow(clippy::too_many_arguments)] // internal plumbing with distinct roles
fn convert_unsplit(
    engine: &Engine,
    reader: &mut dyn ArchiveReader,
    metas: &[EntryMeta],
    detect_name: &str,
    out_path: &Path,
    opts: &CreateOptions,
    progress: &dyn ProgressSink,
    ctl: &ControlToken,
) -> Result<(), FormatError> {
    match open_dest(engine, detect_name, out_path, opts)? {
        DestTarget::Archive(sink) => copy_entries(reader, metas, sink, opts, progress, ctl),
        DestTarget::SingleStream(compressor) => {
            single_stream_convert(reader, metas, &compressor, out_path, opts, progress, ctl)
        }
    }
}

/// Streams every entry from the reader into the destination writer.
fn copy_entries(
    reader: &mut dyn ArchiveReader,
    metas: &[EntryMeta],
    mut sink: DestSink,
    opts: &CreateOptions,
    progress: &dyn ProgressSink,
    ctl: &ControlToken,
) -> Result<(), FormatError> {
    let total: u64 = metas
        .iter()
        .filter(|m| matches!(m.entry_type, EntryType::File))
        .map(|m| m.size)
        .sum();
    let mut done = 0u64;
    for meta in metas {
        ctl.checkpoint()?;
        progress.on_entry_progress(done, total, &meta.path, 0, meta.size);
        // The destination decides about encryption itself; compressed size
        // and CRC are recomputed by the destination writer.
        let out_meta = EntryMeta {
            compressed_size: None,
            crc32: None,
            encrypted: opts.password.is_some(),
            ..meta.clone()
        };
        match meta.entry_type {
            EntryType::File => {
                let data = reader.read_entry(&meta.path)?;
                let mut data =
                    ProgressRead::new(data, progress, ctl, &meta.path, done, total, meta.size);
                sink.add_entry(&out_meta, Some(&mut data)).map_err(|e| {
                    if ctl.is_cancelled() {
                        FormatError::Cancelled
                    } else {
                        e
                    }
                })?;
                done += meta.size;
            }
            _ => sink.add_entry(&out_meta, None)?,
        }
    }
    progress.on_progress(total, total, &EntryPath::from_utf8(""));
    sink.finish()
}

/// Conversion into a bare compressed stream (`x.gz`): the source must hold
/// exactly one file entry (directory markers are ignored).
fn single_stream_convert(
    reader: &mut dyn ArchiveReader,
    metas: &[EntryMeta],
    compressor: &Arc<dyn Compressor>,
    out_path: &Path,
    opts: &CreateOptions,
    progress: &dyn ProgressSink,
    ctl: &ControlToken,
) -> Result<(), FormatError> {
    let mut files = metas
        .iter()
        .filter(|m| !matches!(m.entry_type, EntryType::Dir));
    let (Some(meta), None) = (files.next(), files.next()) else {
        return Err(FormatError::Unsupported(format!(
            "format {} stores exactly one file",
            compressor.id()
        )));
    };
    if !matches!(meta.entry_type, EntryType::File) {
        return Err(FormatError::Unsupported(format!(
            "format {} cannot store entry type of '{}'",
            compressor.id(),
            meta.path
        )));
    }
    let mut data = reader.read_entry(&meta.path)?;
    let dst = std::fs::File::create(out_path)?;
    // Pump locally: the entry reader is not `Send`, so the trait's chunked
    // pump cannot be used here.
    let mut sink = compressor.compress_writer(Box::new(dst), opts.level, &opts.resources)?;
    let label = KnownTotal::new(progress, meta.size, meta.path.clone());
    let mut buf = vec![0u8; opts.resources.stream_buffer_size(64 * 1024)?];
    let mut done = 0u64;
    loop {
        ctl.checkpoint()?;
        let n = data.read(&mut buf)?;
        if n == 0 {
            break;
        }
        std::io::Write::write_all(&mut sink, &buf[..n])?;
        done += n as u64;
        label.on_progress(done, 0, &meta.path);
    }
    sink.finish()?;
    progress.on_progress(meta.size, meta.size, &EntryPath::from_utf8(""));
    Ok(())
}
