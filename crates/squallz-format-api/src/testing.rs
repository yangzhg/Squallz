//! Shared byte accounting for entry streams whose decoders verify at EOF.

use std::io::{self, Read};

use crate::{ControlToken, EntryMeta, FormatError, LimitsAccountant, ProgressSink};

/// Drains one integrity-test stream with cancellation, observed-byte limits,
/// and byte progress. The caller registers the entry before opening its data.
pub fn test_entry_data(
    data: &mut dyn Read,
    meta: &EntryMeta,
    accountant: &mut LimitsAccountant,
    total: u64,
    progress: &dyn ProgressSink,
    control: &ControlToken,
) -> Result<(), FormatError> {
    let mut buffer = [0; 64 * 1024];
    let mut entry_output_bytes = 0;
    progress.on_progress(accountant.output_bytes(), total, &meta.path);
    loop {
        control.checkpoint()?;
        let read = match data.read(&mut buffer) {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            result => result?,
        };
        if read == 0 {
            return Ok(());
        }
        accountant.add_entry_output_bytes(meta, &mut entry_output_bytes, read as u64)?;
        progress.on_progress(accountant.output_bytes(), total, &meta.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EntryPath, EntryType, NoProgress, SafetyLimits};

    struct CountingReader {
        data: io::Take<io::Repeat>,
        read_bytes: u64,
    }

    impl CountingReader {
        fn new(bytes: u64) -> Self {
            Self {
                data: io::repeat(0).take(bytes),
                read_bytes: 0,
            }
        }
    }

    impl Read for CountingReader {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            let count = self.data.read(buffer)?;
            self.read_bytes += count as u64;
            Ok(count)
        }
    }

    fn meta(compressed_size: Option<u64>) -> EntryMeta {
        EntryMeta {
            path: EntryPath::from_utf8("payload.bin"),
            entry_type: EntryType::File,
            size: 0,
            compressed_size,
            modified: None,
            unix_mode: None,
            crc32: None,
            encrypted: false,
        }
    }

    #[test]
    fn test_stream_stops_on_observed_bytes_and_ratio_with_understated_metadata() {
        for (limits, compressed, expected_read_bytes) in [
            (
                SafetyLimits {
                    max_output_bytes: 70_000,
                    ..SafetyLimits::default()
                },
                None,
                2 * 64 * 1024,
            ),
            (
                SafetyLimits {
                    max_compression_ratio: 4,
                    ..SafetyLimits::default()
                },
                Some(32),
                17 * 64 * 1024,
            ),
        ] {
            let mut data = CountingReader::new(8 * 1024 * 1024);
            let mut accountant = LimitsAccountant::new(limits);
            let meta = meta(compressed);
            accountant.check_entry(&meta).unwrap();
            let result = test_entry_data(
                &mut data,
                &meta,
                &mut accountant,
                0,
                &NoProgress,
                &ControlToken::default(),
            );

            assert!(matches!(result, Err(FormatError::ResourceLimitExceeded(_))));
            assert_eq!(data.read_bytes, expected_read_bytes);
            assert_eq!(accountant.output_bytes(), expected_read_bytes);
        }
    }

    struct CancelAfterOutput(ControlToken);

    impl ProgressSink for CancelAfterOutput {
        fn on_progress(&self, done: u64, _total: u64, _path: &EntryPath) {
            if done > 0 {
                self.0.cancel();
            }
        }
    }

    #[test]
    fn test_stream_preserves_cancellation_and_password_errors() {
        let control = ControlToken::default();
        let mut data = CountingReader::new(8 * 1024 * 1024);
        let mut accountant = LimitsAccountant::new(SafetyLimits::default());
        let meta = meta(None);
        accountant.check_entry(&meta).unwrap();
        let result = test_entry_data(
            &mut data,
            &meta,
            &mut accountant,
            0,
            &CancelAfterOutput(control.clone()),
            &control,
        );
        assert!(matches!(result, Err(FormatError::Cancelled)));
        assert_eq!(data.read_bytes, 64 * 1024);

        struct PasswordFailure;
        impl Read for PasswordFailure {
            fn read(&mut self, _buffer: &mut [u8]) -> io::Result<usize> {
                Err(io::Error::other(FormatError::WrongPassword))
            }
        }
        let result = test_entry_data(
            &mut PasswordFailure,
            &meta,
            &mut accountant,
            0,
            &NoProgress,
            &ControlToken::default(),
        );
        assert!(matches!(result, Err(FormatError::WrongPassword)));
    }
}
