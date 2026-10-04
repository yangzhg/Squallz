use std::io::{Read, Write};

use crate::api::{
    ControlToken, EntryMeta, FormatError, LimitsAccountant, ProgressSink, SafetyLimits,
};
use crate::controlled_io::controlled_result;

const COPY_BUFFER_BYTES: usize = 256 * 1024;

/// Copies an already-opened archive entry with the operation's limits and
/// cancellation token. The caller retains the reader and destination owner;
/// this function neither opens another archive nor publishes an output.
pub fn copy_archive_entry(
    reader: &mut dyn Read,
    writer: &mut dyn Write,
    meta: &EntryMeta,
    limits: SafetyLimits,
    progress: &dyn ProgressSink,
    control: &ControlToken,
) -> Result<u64, FormatError> {
    control.checkpoint()?;
    let mut accountant = LimitsAccountant::new(limits);
    accountant.check_entry(meta)?;
    if meta.size > limits.max_output_bytes {
        return Err(FormatError::ResourceLimitExceeded(format!(
            "output bytes exceed limit of {}",
            limits.max_output_bytes
        )));
    }
    let result = (|| {
        let mut written = 0;
        let mut buffer = vec![0_u8; COPY_BUFFER_BYTES];
        progress.on_entry_progress(0, meta.size, &meta.path, 0, meta.size);
        loop {
            control.checkpoint()?;
            let read = reader.read(&mut buffer)?;
            if read == 0 {
                return Ok(written);
            }
            accountant.add_entry_output_bytes(meta, &mut written, read as u64)?;
            writer.write_all(&buffer[..read])?;
            let total = if written <= meta.size { meta.size } else { 0 };
            progress.on_entry_progress(written, total, &meta.path, written, total);
        }
    })();
    controlled_result(control, result)
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;
    use std::sync::Mutex;

    use super::*;
    use crate::api::{EntryPath, EntryType, NoProgress};

    #[derive(Default)]
    struct AggregateProgress(Mutex<Vec<(u64, u64)>>);

    impl ProgressSink for AggregateProgress {
        fn on_progress(&self, done: u64, total: u64, _current: &EntryPath) {
            self.0.lock().unwrap().push((done, total));
        }
    }

    #[test]
    fn aggregate_progress_reports_written_bytes_and_unknown_totals() {
        let length = 2 * COPY_BUFFER_BYTES as u64 + 7;
        for declared in [length, 0, COPY_BUFFER_BYTES as u64] {
            let meta = EntryMeta {
                path: EntryPath::from_utf8("inner.zip"),
                entry_type: EntryType::File,
                size: declared,
                compressed_size: None,
                modified: None,
                unix_mode: None,
                crc32: None,
                encrypted: false,
            };
            let progress = AggregateProgress::default();
            let mut output = Vec::new();
            assert_eq!(
                copy_archive_entry(
                    &mut Cursor::new(vec![0xA5; length as usize]),
                    &mut output,
                    &meta,
                    SafetyLimits::default(),
                    &progress,
                    &ControlToken::default(),
                )
                .unwrap(),
                length
            );
            assert_eq!(output, vec![0xA5; length as usize]);
            let samples = progress.0.lock().unwrap();
            assert_eq!(samples.first(), Some(&(0, declared)));
            assert_eq!(
                samples.last(),
                Some(&(length, if declared == length { length } else { 0 }))
            );
            assert!(samples.windows(2).all(|pair| pair[0].0 < pair[1].0));
            assert!(samples
                .iter()
                .filter(|(done, _)| *done > declared)
                .all(|(_, total)| *total == 0));
        }
    }

    #[test]
    fn copied_bytes_enforce_limits_when_declared_size_is_too_small() {
        let meta = EntryMeta {
            path: EntryPath::from_utf8("inner.zip"),
            entry_type: EntryType::File,
            size: 0,
            compressed_size: Some(1),
            modified: None,
            unix_mode: None,
            crc32: None,
            encrypted: false,
        };
        for (limits, accepted, detail) in [
            (
                SafetyLimits {
                    max_output_bytes: 2 * COPY_BUFFER_BYTES as u64,
                    ..SafetyLimits::default()
                },
                2 * COPY_BUFFER_BYTES,
                "output bytes",
            ),
            (
                SafetyLimits {
                    max_compression_ratio: 2,
                    ..SafetyLimits::default()
                },
                4 * COPY_BUFFER_BYTES,
                "observed compression ratio",
            ),
        ] {
            let mut reader = Cursor::new(vec![0xA5; 8 * COPY_BUFFER_BYTES]);
            let mut output = Vec::new();
            let error = copy_archive_entry(
                &mut reader,
                &mut output,
                &meta,
                limits,
                &NoProgress,
                &ControlToken::default(),
            )
            .unwrap_err();
            assert!(
                matches!(&error, FormatError::ResourceLimitExceeded(message) if message.contains(detail))
            );
            assert_eq!(output.len(), accepted);
            assert_eq!(reader.position(), (accepted + COPY_BUFFER_BYTES) as u64);
        }
    }
}
