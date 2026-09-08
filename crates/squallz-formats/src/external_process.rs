use std::io::{self, Read};
use std::process::{Child, ExitStatus, Output};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use squallz_format_api::{ControlToken, FormatError};

const POLL_INTERVAL: Duration = Duration::from_millis(25);

pub(crate) struct ControlledChild {
    child: Arc<Mutex<Child>>,
    stop: Arc<AtomicBool>,
    watcher: Option<JoinHandle<()>>,
    finished: bool,
}

impl ControlledChild {
    pub(crate) fn new(child: Child, control: &ControlToken) -> Self {
        let child = Arc::new(Mutex::new(child));
        let stop = Arc::new(AtomicBool::new(false));
        let watched_child = Arc::clone(&child);
        let watched_stop = Arc::clone(&stop);
        let watched_control = control.clone();
        let watcher = thread::spawn(move || {
            while !watched_stop.load(Ordering::Acquire) {
                if watched_control.is_cancelled() {
                    let mut child = lock_child(&watched_child);
                    let _ = child.kill();
                    return;
                }
                thread::park_timeout(POLL_INTERVAL);
            }
        });
        Self {
            child,
            stop,
            watcher: Some(watcher),
            finished: false,
        }
    }

    pub(crate) fn wait(&mut self) -> io::Result<ExitStatus> {
        let status = loop {
            let polled = {
                let mut child = lock_child(&self.child);
                child.try_wait()
            };
            match polled {
                Ok(Some(status)) => break Ok(status),
                Ok(None) => thread::sleep(POLL_INTERVAL),
                Err(error) => break Err(error),
            }
        };
        if status.is_ok() {
            self.finished = true;
        }
        self.stop_watcher();
        status
    }

    pub(crate) fn terminate(&mut self) {
        if !self.finished {
            let mut child = lock_child(&self.child);
            terminate_child(&mut child);
            self.finished = true;
        }
        self.stop_watcher();
    }

    fn stop_watcher(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(watcher) = self.watcher.take() {
            watcher.thread().unpark();
            let _ = watcher.join();
        }
    }
}

impl Drop for ControlledChild {
    fn drop(&mut self) {
        self.terminate();
    }
}

pub(crate) fn wait_with_output(
    mut child: Child,
    control: &ControlToken,
    backend: &'static str,
) -> Result<Output, FormatError> {
    let stdout = child.stdout.take().map(spawn_capture);
    let stderr = child.stderr.take().map(spawn_capture);
    let status = wait_for_exit(&mut child, control);
    let stdout = finish_capture(stdout, backend, "stdout");
    let stderr = finish_capture(stderr, backend, "stderr");
    let status = status?;
    let stdout = stdout?;
    let stderr = stderr?;
    control.checkpoint()?;
    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

fn wait_for_exit(child: &mut Child, control: &ControlToken) -> Result<ExitStatus, FormatError> {
    loop {
        if let Err(error) = control.checkpoint() {
            terminate_child(child);
            return Err(error);
        }
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) => thread::sleep(POLL_INTERVAL),
            Err(error) => {
                terminate_child(child);
                return Err(FormatError::from(error));
            }
        }
    }
}

fn terminate_child(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn lock_child(child: &Mutex<Child>) -> MutexGuard<'_, Child> {
    child
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn spawn_capture<R>(stream: R) -> JoinHandle<io::Result<Vec<u8>>>
where
    R: Read + Send + 'static,
{
    thread::spawn(move || {
        let mut stream = stream;
        let mut output = Vec::new();
        stream.read_to_end(&mut output)?;
        Ok(output)
    })
}

fn finish_capture(
    capture: Option<JoinHandle<io::Result<Vec<u8>>>>,
    backend: &'static str,
    stream: &'static str,
) -> Result<Vec<u8>, FormatError> {
    let Some(capture) = capture else {
        return Ok(Vec::new());
    };
    capture
        .join()
        .map_err(|_| {
            FormatError::Io(io::Error::other(format!(
                "{backend} {stream} reader stopped unexpectedly"
            )))
        })?
        .map_err(FormatError::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};
    use std::time::Instant;

    const WORKER_ENV: &str = "SQUALLZ_EXTERNAL_PROCESS_TEST_WORKER";

    #[test]
    fn cancellation_terminates_a_running_child() {
        let child = Command::new(std::env::current_exe().unwrap())
            .arg("--exact")
            .arg("external_process::tests::cancelled_child_worker")
            .arg("--nocapture")
            .env(WORKER_ENV, "1")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        assert!(child.stdout.is_some());
        assert!(child.stderr.is_some());

        let control = ControlToken::default();
        let cancelling_control = control.clone();
        let canceller = thread::spawn(move || {
            thread::sleep(Duration::from_millis(100));
            cancelling_control.cancel();
        });
        let started = Instant::now();
        let error = wait_with_output(child, &control, "test backend").unwrap_err();
        canceller.join().unwrap();

        assert!(matches!(error, FormatError::Cancelled));
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn cancellation_unblocks_a_child_stdout_read() {
        let mut child = Command::new(std::env::current_exe().unwrap())
            .arg("--exact")
            .arg("external_process::tests::cancelled_child_worker")
            .arg("--nocapture")
            .env(WORKER_ENV, "1")
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut stdout = child.stdout.take().unwrap();
        let control = ControlToken::default();
        let mut controlled_child = ControlledChild::new(child, &control);
        let cancelling_control = control.clone();
        let canceller = thread::spawn(move || {
            thread::sleep(Duration::from_millis(100));
            cancelling_control.cancel();
        });

        let started = Instant::now();
        let mut output = Vec::new();
        stdout.read_to_end(&mut output).unwrap();
        controlled_child.wait().unwrap();
        canceller.join().unwrap();

        assert!(control.is_cancelled());
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn cancelled_child_worker() {
        if std::env::var_os(WORKER_ENV).is_none() {
            return;
        }
        loop {
            thread::sleep(Duration::from_secs(1));
        }
    }
}
