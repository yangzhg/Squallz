//! Archive creation: format selection by destination name, the shared
//! destination-writer machinery (plain / compound / single-stream, reused
//! by format conversion) and `.001` split-volume output.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::api::{
    split_volume_name, ArchiveFormat, ArchiveWriter, Compressor, ControlToken, CreateOptions,
    Detected, EntryMeta, EntryPath, EntryType, FormatError, ProgressSink,
};
use crate::compound::{KnownTotal, ProgressRead, SharedCompressSink};
use crate::inputs::{collect_inputs, InputItem};
use crate::volumes::{self, MIN_SPLIT_SIZE};
use crate::{Engine, PathFilter};

/// Destination writer over an archive output: plain (`x.zip`) or compound
/// (`x.tar.gz`, where finishing must also flush the compressor sink).
pub(crate) struct DestSink {
    writer: Box<dyn ArchiveWriter>,
    shared: Option<SharedCompressSink>,
}

impl DestSink {
    pub(crate) fn add_entry(
        &mut self,
        meta: &EntryMeta,
        data: Option<&mut dyn std::io::Read>,
    ) -> Result<(), FormatError> {
        self.writer.add_entry(meta, data)
    }

    pub(crate) fn finish(self) -> Result<(), FormatError> {
        self.writer.finish()?;
        match self.shared {
            Some(shared) => shared.finish(),
            None => Ok(()),
        }
    }
}

/// Resolved destination of a create/convert operation.
pub(crate) enum DestTarget {
    /// An archive container (possibly compound).
    Archive(DestSink),
    /// A bare single-stream compressor (`x.gz`): the caller must supply
    /// exactly one file's content.
    SingleStream(Arc<dyn Compressor>),
}

/// Resolves `detect_name` to a format and opens a writer at `out_path`.
pub(crate) fn open_dest(
    engine: &Engine,
    detect_name: &str,
    out_path: &Path,
    opts: &CreateOptions,
) -> Result<DestTarget, FormatError> {
    match engine.registry().detect_by_name(detect_name) {
        Some(Detected::Archive(f)) => {
            check_can_create(&f)?;
            let file = File::create(out_path)?;
            let writer = f.create(Box::new(file), opts)?;
            Ok(DestTarget::Archive(DestSink {
                writer,
                shared: None,
            }))
        }
        Some(Detected::Compressed {
            compressor,
            inner_archive: Some(archive),
        }) => {
            check_can_create(&archive)?;
            let file = File::create(out_path)?;
            let sink = compressor.compress_writer(Box::new(file), opts.level, &opts.resources)?;
            let shared = SharedCompressSink::new(sink);
            let writer = archive.create_stream(Box::new(shared.clone()), opts)?;
            Ok(DestTarget::Archive(DestSink {
                writer,
                shared: Some(shared),
            }))
        }
        Some(Detected::Compressed {
            compressor,
            inner_archive: None,
        }) => {
            if opts.password.is_some() {
                return Err(FormatError::Unsupported(format!(
                    "format {} does not support encryption",
                    compressor.id()
                )));
            }
            Ok(DestTarget::SingleStream(compressor))
        }
        None => Err(FormatError::Unsupported(format!(
            "creating this format is not supported: {detect_name}"
        ))),
    }
}

/// Rejects formats that declare `can_create=false`.
fn check_can_create(format: &Arc<dyn ArchiveFormat>) -> Result<(), FormatError> {
    if !format.capabilities().can_create {
        return Err(FormatError::Unsupported(format!(
            "format {} does not support creation",
            format.id()
        )));
    }
    Ok(())
}

/// Entry point for [`Engine::create`].
pub(crate) fn create(
    engine: &Engine,
    dest: &Path,
    inputs: &[PathBuf],
    opts: &CreateOptions,
    progress: &dyn ProgressSink,
    ctl: &ControlToken,
) -> Result<(), FormatError> {
    with_split_output(dest, opts, ctl, |detect_name, out_path, opts| {
        create_unsplit(engine, detect_name, out_path, inputs, opts, progress, ctl)
    })
}

/// Runs `write` against the final path directly, or — when
/// `opts.split_size` is set — against a temporary file that is then cut
/// into `.001`-style volumes (disk-space pre-check + atomic finish inside
/// the splitter). Shared by create and convert.
pub(crate) fn with_split_output(
    dest: &Path,
    opts: &CreateOptions,
    ctl: &ControlToken,
    write: impl FnOnce(&str, &Path, &CreateOptions) -> Result<(), FormatError>,
) -> Result<(), FormatError> {
    let name = dest
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| FormatError::Unsupported("invalid output file name".into()))?;
    let Some(split) = opts.split_size else {
        return write(name, dest, opts);
    };
    if split < MIN_SPLIT_SIZE {
        return Err(FormatError::Unsupported(format!(
            "split size below the {MIN_SPLIT_SIZE}-byte minimum: {split}"
        )));
    }
    // Accept an explicit first-volume name (`x.zip.001`) as the base too.
    let (base_name, base) = match split_volume_name(name) {
        Some((stripped, _)) => (stripped.to_string(), dest.with_file_name(stripped)),
        None => (name.to_string(), dest.to_path_buf()),
    };
    let tmp = dest.with_file_name(format!(".{base_name}.sqz-split-{}.tmp", std::process::id()));
    let inner_opts = CreateOptions {
        split_size: None,
        ..opts.clone()
    };
    let result = write(&base_name, &tmp, &inner_opts)
        .and_then(|()| volumes::split_into_volumes(&tmp, &base, split, ctl).map(drop));
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// Single-output creation from file-system inputs.
fn create_unsplit(
    engine: &Engine,
    detect_name: &str,
    out_path: &Path,
    inputs: &[PathBuf],
    opts: &CreateOptions,
    progress: &dyn ProgressSink,
    ctl: &ControlToken,
) -> Result<(), FormatError> {
    let tmp = crate::sibling_temp_path(out_path, "create")?;
    let result = (|| match open_dest(engine, detect_name, &tmp, opts)? {
        DestTarget::Archive(mut sink) => {
            let excludes = PathFilter::new(&opts.excludes)?;
            let items = collect_inputs(inputs, &excludes)?;
            write_entries(&mut sink, &items, opts, progress, ctl)?;
            sink.finish()
        }
        DestTarget::SingleStream(compressor) => {
            create_single_stream(&compressor, &tmp, inputs, opts, progress, ctl)
        }
    })()
    .and_then(|()| crate::replace_file(&tmp, out_path));
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// Streams the collected input items into an archive writer, with
/// byte-granular progress on file contents and chunk-boundary cancellation.
fn write_entries(
    sink: &mut DestSink,
    items: &[InputItem],
    opts: &CreateOptions,
    progress: &dyn ProgressSink,
    ctl: &ControlToken,
) -> Result<(), FormatError> {
    let total: u64 = items.iter().map(|i| i.size).sum();
    let mut done = 0u64;
    for item in items {
        ctl.checkpoint()?;
        progress.on_entry_progress(done, total, &item.name, 0, item.size);
        let meta = EntryMeta {
            path: item.name.clone(),
            entry_type: item.entry_type.clone(),
            size: item.size,
            compressed_size: None,
            modified: item.modified,
            unix_mode: item.unix_mode,
            crc32: None,
            encrypted: opts.password.is_some(),
        };
        match item.entry_type {
            EntryType::File => {
                let f = File::open(&item.src)?;
                let mut data =
                    ProgressRead::new(f, progress, ctl, &item.name, done, total, item.size);
                // Cancellation inside the copy surfaces as an I/O error;
                // restore the precise variant here.
                sink.add_entry(&meta, Some(&mut data)).map_err(|e| {
                    if ctl.is_cancelled() {
                        FormatError::Cancelled
                    } else {
                        e
                    }
                })?;
            }
            _ => sink.add_entry(&meta, None)?,
        }
        done += item.size;
    }
    progress.on_progress(total, total, &EntryPath::from_utf8(""));
    Ok(())
}

/// Single-stream creation (`x.gz`): compresses exactly one input file.
fn create_single_stream(
    compressor: &Arc<dyn Compressor>,
    dest: &Path,
    inputs: &[PathBuf],
    opts: &CreateOptions,
    progress: &dyn ProgressSink,
    ctl: &ControlToken,
) -> Result<(), FormatError> {
    let [input] = inputs else {
        return Err(FormatError::Unsupported(format!(
            "format {} compresses exactly one file",
            compressor.id()
        )));
    };
    let meta = std::fs::metadata(input)?;
    if !meta.is_file() {
        return Err(FormatError::Unsupported(format!(
            "format {} compresses a single regular file",
            compressor.id()
        )));
    }
    let mut src = File::open(input)?;
    let mut dst = File::create(dest)?;
    let label = EntryPath::from_utf8(
        input
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned()),
    );
    let sink = KnownTotal::new(progress, meta.len(), label);
    compressor.compress(&mut src, &mut dst, opts.level, &opts.resources, &sink, ctl)?;
    progress.on_progress(meta.len(), meta.len(), &EntryPath::from_utf8(""));
    Ok(())
}
