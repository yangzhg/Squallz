use std::io::{self, Read, Seek, SeekFrom};

use crate::api::{ControlToken, FormatError, ReadSeek};

const READ_CHUNK: usize = 64 * 1024;
const CANCELLED_IO_MESSAGE: &str = "archive operation cancelled";

pub(crate) struct ControlledReadSeek {
    inner: Box<dyn ReadSeek>,
    control: ControlToken,
}

impl ControlledReadSeek {
    pub(crate) fn boxed(inner: Box<dyn ReadSeek>, control: &ControlToken) -> Box<dyn ReadSeek> {
        Box::new(Self {
            inner,
            control: control.clone(),
        })
    }

    fn checkpoint(&self) -> io::Result<()> {
        self.control
            .checkpoint()
            .map_err(|_| io::Error::other(CANCELLED_IO_MESSAGE))
    }
}

impl Read for ControlledReadSeek {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        self.checkpoint()?;
        let read_len = buf.len().min(READ_CHUNK);
        self.inner.read(&mut buf[..read_len])
    }
}

impl Seek for ControlledReadSeek {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.checkpoint()?;
        self.inner.seek(pos)
    }
}

pub(crate) struct ControlledRead {
    inner: Box<dyn Read + Send>,
    control: ControlToken,
}

impl ControlledRead {
    pub(crate) fn boxed(
        inner: Box<dyn Read + Send>,
        control: &ControlToken,
    ) -> Box<dyn Read + Send> {
        Box::new(Self {
            inner,
            control: control.clone(),
        })
    }
}

impl Read for ControlledRead {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        self.control
            .checkpoint()
            .map_err(|_| io::Error::other(CANCELLED_IO_MESSAGE))?;
        let read_len = buf.len().min(READ_CHUNK);
        self.inner.read(&mut buf[..read_len])
    }
}

pub(crate) fn controlled_result<T>(
    control: &ControlToken,
    result: Result<T, FormatError>,
) -> Result<T, FormatError> {
    match result {
        Ok(value) => {
            control.checkpoint()?;
            Ok(value)
        }
        Err(_) if control.is_cancelled() => Err(FormatError::Cancelled),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    struct TrackingReader {
        inner: Cursor<Vec<u8>>,
        largest_request: Arc<AtomicUsize>,
    }

    impl Read for TrackingReader {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.largest_request.fetch_max(buf.len(), Ordering::SeqCst);
            self.inner.read(buf)
        }
    }

    impl Seek for TrackingReader {
        fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
            self.inner.seek(pos)
        }
    }

    #[test]
    fn controlled_seekable_reads_are_chunked_and_cancellable() {
        let largest_request = Arc::new(AtomicUsize::new(0));
        let source = TrackingReader {
            inner: Cursor::new(vec![0_u8; READ_CHUNK * 2]),
            largest_request: Arc::clone(&largest_request),
        };
        let control = ControlToken::default();
        let mut reader = ControlledReadSeek::boxed(Box::new(source), &control);
        let mut buffer = vec![0_u8; READ_CHUNK * 2];

        let read = reader.read(&mut buffer).unwrap();
        assert_eq!(read, READ_CHUNK);
        assert_eq!(largest_request.load(Ordering::SeqCst), READ_CHUNK);

        control.cancel();
        let error = reader.read(&mut buffer).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Other);
    }

    #[test]
    fn cancellation_wins_over_wrapped_format_errors() {
        let control = ControlToken::default();
        control.cancel();

        let result = controlled_result::<()>(&control, Err(FormatError::Other("late".into())));

        assert!(matches!(result, Err(FormatError::Cancelled)));
    }
}
