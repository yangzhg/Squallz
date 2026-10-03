//! Pipe ownership and shutdown for the 7-Zip read bridge.

use std::io::{self, Read, Write};
use std::process::{Child, ChildStdout, Command, ExitStatus, Stdio};
use std::thread::{self, JoinHandle};

use squallz_format_api::{ControlToken, FormatError, Password};

use super::diagnostics::{DiagnosticCapture, Diagnostics};
use crate::external_process::ControlledChild;

pub(super) struct SevenZipProcess {
    child: ControlledChild,
    pub(super) stdout: ChildStdout,
    diagnostics: Option<JoinHandle<io::Result<Diagnostics>>>,
    password_write: Option<JoinHandle<io::Result<()>>>,
    control: ControlToken,
}

pub(super) struct SevenZipExit {
    pub(super) status: ExitStatus,
    pub(super) diagnostics: Diagnostics,
    pub(super) password_write: io::Result<()>,
}

impl SevenZipProcess {
    pub(super) fn spawn(
        mut command: Command,
        password: Option<&Password>,
        control: &ControlToken,
    ) -> Result<Self, FormatError> {
        control.checkpoint()?;
        let stdin = password_stdio(password)?;
        let mut capture = DiagnosticCapture::for_stderr()?;
        let mut child = command
            .stdin(stdin)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(map_spawn_error)?;
        let streams = match (child.stdout.take(), child.stderr.take()) {
            (Some(stdout), Some(stderr)) => (stdout, stderr),
            _ => {
                terminate_child(&mut child);
                return Err(FormatError::Other(
                    "7-Zip did not provide its output streams".into(),
                ));
            }
        };
        let credentials = match password {
            Some(password) => match child.stdin.take() {
                Some(stdin) => Some((stdin, password.clone())),
                None => {
                    terminate_child(&mut child);
                    return Err(FormatError::Other(
                        "7-Zip did not provide a credential stream".into(),
                    ));
                }
            },
            None => None,
        };
        let child = ControlledChild::new(child, control);
        let (stdout, stderr) = streams;
        let diagnostics = Some(thread::spawn(move || {
            let mut stderr = stderr;
            let mut buffer = [0u8; 4096];
            loop {
                let read = match stderr.read(&mut buffer) {
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    result => result?,
                };
                if read == 0 {
                    return Ok(capture.finish());
                }
                capture.observe(&buffer[..read]);
            }
        }));
        let password_write = credentials.map(|(mut stdin, password)| {
            thread::spawn(move || {
                stdin.write_all(password.expose().as_bytes())?;
                stdin.write_all(b"\n")
            })
        });
        Ok(Self {
            child,
            stdout,
            diagnostics,
            password_write,
            control: control.clone(),
        })
    }

    pub(super) fn finish(&mut self) -> Result<SevenZipExit, FormatError> {
        let status = self.child.wait();
        if status.is_err() {
            self.child.terminate();
        }
        // Join both pipes before propagating errors. Cancellation closes a blocked
        // credential writer as well as the diagnostic reader.
        let diagnostics = match self.diagnostics.take() {
            Some(handle) => join_pipe(handle, "diagnostic reader"),
            None => Ok(Diagnostics::default()),
        };
        let password_write = match self.password_write.take() {
            Some(handle) => join_pipe(handle, "credential writer"),
            None => Ok(()),
        };
        self.control.checkpoint()?;
        Ok(SevenZipExit {
            status: status?,
            diagnostics: diagnostics?,
            password_write,
        })
    }

    pub(super) fn terminate(&mut self) {
        self.child.terminate();
    }
}

impl Drop for SevenZipProcess {
    fn drop(&mut self) {
        self.child.terminate();
        if let Some(handle) = self.diagnostics.take() {
            let _ = handle.join();
        }
        if let Some(handle) = self.password_write.take() {
            let _ = handle.join();
        }
    }
}

fn join_pipe<T>(handle: JoinHandle<io::Result<T>>, description: &str) -> io::Result<T> {
    handle
        .join()
        .map_err(|_| io::Error::other(format!("7-Zip {description} stopped unexpectedly")))?
}

fn password_stdio(password: Option<&Password>) -> Result<Stdio, FormatError> {
    if password.is_some_and(|password| password.expose().contains(['\r', '\n'])) {
        return Err(FormatError::Unsupported(
            "7-Zip bridge passwords cannot contain line breaks".into(),
        ));
    }
    Ok(if password.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    })
}

fn map_spawn_error(error: io::Error) -> FormatError {
    if error.kind() == io::ErrorKind::NotFound {
        FormatError::DependencyMissing("7zz/7z external format bridge".into())
    } else {
        FormatError::from(error)
    }
}

fn terminate_child(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}
