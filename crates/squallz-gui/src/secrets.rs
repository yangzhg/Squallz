//! Persistent secret storage boundary for GUI-only conveniences.
//! Passwords must never be persisted through SettingsStore or frontend
//! storage. Platform backends live behind this trait so platform-specific
//! secret stores can change without changing IPC.

use std::fmt;
use std::path::Path;
#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::process::{Command, Output};
use std::sync::Arc;
#[cfg(target_os = "linux")]
use std::{
    env,
    io::{Read, Write},
    os::unix::{fs::PermissionsExt, process::CommandExt},
    path::PathBuf,
    process::Stdio,
    time::{Duration, Instant},
};
#[cfg(target_os = "windows")]
use std::{ptr, slice};

use squallz_core::api::Password;

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows", test))]
const SERVICE: &str = "com.squallz.archive-password";
#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows", test))]
const DEFAULT_ARCHIVE_LABEL: &str = "Squallz archive password";

#[derive(Debug, Clone)]
pub struct SecretStoreError {
    detail: String,
}

impl SecretStoreError {
    fn new(detail: impl Into<String>) -> Self {
        Self {
            detail: detail.into(),
        }
    }
}

impl fmt::Display for SecretStoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.detail)
    }
}

pub trait SecretStore: Send + Sync {
    fn is_available(&self) -> bool;

    fn get_archive_password(&self, path: &Path) -> Result<Option<Password>, SecretStoreError>;

    fn set_archive_password(&self, path: &Path, password: &str) -> Result<(), SecretStoreError>;

    fn delete_archive_password(&self, path: &Path) -> Result<(), SecretStoreError>;

    fn has_archive_password(&self, path: &Path) -> Result<bool, SecretStoreError> {
        self.get_archive_password(path).map(|pw| pw.is_some())
    }
}

pub type SharedSecretStore = Arc<dyn SecretStore>;

pub fn system_secret_store() -> SharedSecretStore {
    #[cfg(target_os = "macos")]
    {
        Arc::new(MacOsKeychainSecretStore)
    }
    #[cfg(target_os = "linux")]
    {
        Arc::new(LinuxSecretServiceStore)
    }
    #[cfg(target_os = "windows")]
    {
        Arc::new(WindowsCredentialManagerSecretStore)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        Arc::new(UnavailableSecretStore)
    }
}

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows", test))]
fn archive_account(path: &Path) -> String {
    format!("archive:{}", path.to_string_lossy())
}

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows", test))]
fn archive_label(path: &Path) -> String {
    match path.file_name() {
        Some(name) => format!("{DEFAULT_ARCHIVE_LABEL}: {}", name.to_string_lossy()),
        None => DEFAULT_ARCHIVE_LABEL.to_owned(),
    }
}

#[cfg(target_os = "macos")]
struct MacOsKeychainSecretStore;

#[cfg(target_os = "macos")]
impl MacOsKeychainSecretStore {
    fn run_security(args: &[&str]) -> Result<Output, SecretStoreError> {
        Command::new("/usr/bin/security")
            .args(args)
            .output()
            .map_err(|e| SecretStoreError::new(format!("macOS Keychain command failed: {e}")))
    }

    fn output_error(action: &str, output: &Output) -> SecretStoreError {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        let detail = if stderr.is_empty() {
            format!(
                "macOS Keychain {action} failed with status {}",
                output.status
            )
        } else {
            format!("macOS Keychain {action} failed: {stderr}")
        };
        SecretStoreError::new(detail)
    }

    fn missing(output: &Output) -> bool {
        let stderr = String::from_utf8_lossy(&output.stderr).to_lowercase();
        stderr.contains("could not be found") || stderr.contains("not found")
    }
}

#[cfg(target_os = "macos")]
impl SecretStore for MacOsKeychainSecretStore {
    fn is_available(&self) -> bool {
        Path::new("/usr/bin/security").exists()
    }

    fn get_archive_password(&self, path: &Path) -> Result<Option<Password>, SecretStoreError> {
        if !self.is_available() {
            return Ok(None);
        }
        let account = archive_account(path);
        let output =
            Self::run_security(&["find-generic-password", "-s", SERVICE, "-a", &account, "-w"])?;
        if output.status.success() {
            let mut password = String::from_utf8_lossy(&output.stdout).into_owned();
            while password.ends_with('\n') || password.ends_with('\r') {
                password.pop();
            }
            Ok(Some(Password::new(password)))
        } else if Self::missing(&output) {
            Ok(None)
        } else {
            Err(Self::output_error("read", &output))
        }
    }

    fn set_archive_password(&self, path: &Path, password: &str) -> Result<(), SecretStoreError> {
        if !self.is_available() {
            return Err(SecretStoreError::new("macOS Keychain is not available"));
        }
        let account = archive_account(path);
        let label = archive_label(path);
        let output = Self::run_security(&[
            "add-generic-password",
            "-U",
            "-s",
            SERVICE,
            "-a",
            &account,
            "-l",
            &label,
            "-w",
            password,
        ])?;
        if output.status.success() {
            Ok(())
        } else {
            Err(Self::output_error("write", &output))
        }
    }

    fn delete_archive_password(&self, path: &Path) -> Result<(), SecretStoreError> {
        if !self.is_available() {
            return Ok(());
        }
        let account = archive_account(path);
        let output =
            Self::run_security(&["delete-generic-password", "-s", SERVICE, "-a", &account])?;
        if output.status.success() || Self::missing(&output) {
            Ok(())
        } else {
            Err(Self::output_error("delete", &output))
        }
    }
}

#[cfg(target_os = "linux")]
struct LinuxSecretServiceStore;

#[cfg(target_os = "linux")]
impl LinuxSecretServiceStore {
    const COMMAND_TIMEOUT: Duration = Duration::from_secs(5);
    const COMMAND_OUTPUT_LIMIT: usize = 1024 * 1024;

    fn is_executable(path: &Path) -> bool {
        path.metadata()
            .map(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }

    fn find_on_path(name: &str) -> Option<PathBuf> {
        let path = env::var_os("PATH")?;
        env::split_paths(&path)
            .map(|dir| dir.join(name))
            .find(|candidate| Self::is_executable(candidate))
    }

    fn secret_tool() -> Option<PathBuf> {
        if let Some(path) = env::var_os("SQUALLZ_SECRET_TOOL") {
            let path = PathBuf::from(path);
            return Self::is_executable(&path).then_some(path);
        }
        Self::find_on_path("secret-tool")
    }

    fn gdbus() -> Option<PathBuf> {
        Self::find_on_path("gdbus")
    }

    fn drain_nonblocking_output(
        reader: &mut impl Read,
        retained: &mut Vec<u8>,
        overflow: &mut bool,
    ) -> std::io::Result<(bool, bool)> {
        let mut buffer = [0_u8; 16 * 1024];
        match reader.read(&mut buffer) {
            Ok(0) => Ok((false, true)),
            Ok(read) => {
                let retain = read.min(Self::COMMAND_OUTPUT_LIMIT.saturating_sub(retained.len()));
                retained.extend_from_slice(&buffer[..retain]);
                *overflow |= retain != read;
                Ok((true, false))
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => Ok((false, false)),
            Err(error) => Err(error),
        }
    }

    fn set_nonblocking(
        descriptor: &impl std::os::fd::AsFd,
        stream: &str,
    ) -> Result<(), SecretStoreError> {
        let flags = rustix::fs::fcntl_getfl(descriptor).map_err(|error| {
            SecretStoreError::new(format!(
                "Linux Secret Service {stream} flags could not be read: {error}"
            ))
        })?;
        rustix::fs::fcntl_setfl(descriptor, flags | rustix::fs::OFlags::NONBLOCK).map_err(|error| {
            SecretStoreError::new(format!(
                "Linux Secret Service {stream} could not be made non-blocking: {error}"
            ))
        })
    }

    fn terminate_command_group(
        child: &mut std::process::Child,
        process_group: rustix::process::Pid,
    ) {
        let _ = rustix::process::kill_process_group(process_group, rustix::process::Signal::KILL);
        let _ = child.kill();
        let _ = child.wait();
    }

    fn run_command(
        tool: &Path,
        args: &[&str],
        stdin: Option<&str>,
        name: &str,
        timeout: Duration,
    ) -> Result<Output, SecretStoreError> {
        let started = Instant::now();
        let mut command = Command::new(tool);
        command
            .args(args)
            .stdin(if stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command.process_group(0);
        let mut child = command.spawn().map_err(|error| {
            SecretStoreError::new(format!(
                "Linux Secret Service {name} command could not start: {error}"
            ))
        })?;
        let process_group = rustix::process::Pid::from_child(&child);
        let mut input = if stdin.is_some() {
            match child.stdin.take() {
                Some(input) => Some(input),
                None => {
                    Self::terminate_command_group(&mut child, process_group);
                    return Err(SecretStoreError::new(
                        "Linux Secret Service input is unavailable",
                    ));
                }
            }
        } else {
            None
        };
        let mut stdout = match child.stdout.take() {
            Some(stdout) => stdout,
            None => {
                Self::terminate_command_group(&mut child, process_group);
                return Err(SecretStoreError::new(
                    "Linux Secret Service output is unavailable",
                ));
            }
        };
        let mut stderr = match child.stderr.take() {
            Some(stderr) => stderr,
            None => {
                Self::terminate_command_group(&mut child, process_group);
                return Err(SecretStoreError::new(
                    "Linux Secret Service diagnostics are unavailable",
                ));
            }
        };

        if let Some(input) = &input {
            if let Err(error) = Self::set_nonblocking(input, "input") {
                Self::terminate_command_group(&mut child, process_group);
                return Err(error);
            }
        }
        if let Err(error) = Self::set_nonblocking(&stdout, "output") {
            Self::terminate_command_group(&mut child, process_group);
            return Err(error);
        }
        if let Err(error) = Self::set_nonblocking(&stderr, "diagnostics") {
            Self::terminate_command_group(&mut child, process_group);
            return Err(error);
        }

        let secret = stdin.map(str::as_bytes).unwrap_or_default();
        let mut secret_offset = 0usize;
        let mut stdout_bytes = Vec::new();
        let mut stderr_bytes = Vec::new();
        let mut stdout_overflow = false;
        let mut stderr_overflow = false;
        let mut stdout_open = true;
        let mut stderr_open = true;
        let mut status = None;

        loop {
            let mut progressed = false;
            if status.is_none() {
                match child.try_wait() {
                    Ok(Some(exit_status)) => {
                        status = Some(exit_status);
                        progressed = true;
                    }
                    Ok(None) => {}
                    Err(error) => {
                        Self::terminate_command_group(&mut child, process_group);
                        return Err(SecretStoreError::new(format!(
                            "Linux Secret Service {name} command failed: {error}"
                        )));
                    }
                }
            }

            if let Some(writer) = input.as_mut() {
                if secret_offset == secret.len() {
                    input = None;
                    progressed = true;
                } else {
                    match writer.write(&secret[secret_offset..]) {
                        Ok(0) => {
                            Self::terminate_command_group(&mut child, process_group);
                            return Err(SecretStoreError::new(
                                "Linux Secret Service input closed before the password was written",
                            ));
                        }
                        Ok(written) => {
                            secret_offset += written;
                            progressed = true;
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                        Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => {
                            Self::terminate_command_group(&mut child, process_group);
                            return Err(SecretStoreError::new(
                                "Linux Secret Service input closed before the password was written",
                            ));
                        }
                        Err(error) => {
                            Self::terminate_command_group(&mut child, process_group);
                            return Err(SecretStoreError::new(format!(
                                "Linux Secret Service input failed: {error}"
                            )));
                        }
                    }
                }
            }

            if stdout_open {
                match Self::drain_nonblocking_output(
                    &mut stdout,
                    &mut stdout_bytes,
                    &mut stdout_overflow,
                ) {
                    Ok((read_progress, closed)) => {
                        progressed |= read_progress;
                        stdout_open = !closed;
                    }
                    Err(error) => {
                        Self::terminate_command_group(&mut child, process_group);
                        return Err(SecretStoreError::new(format!(
                            "Linux Secret Service output stream failed: {error}"
                        )));
                    }
                }
            }
            if stderr_open {
                match Self::drain_nonblocking_output(
                    &mut stderr,
                    &mut stderr_bytes,
                    &mut stderr_overflow,
                ) {
                    Ok((read_progress, closed)) => {
                        progressed |= read_progress;
                        stderr_open = !closed;
                    }
                    Err(error) => {
                        Self::terminate_command_group(&mut child, process_group);
                        return Err(SecretStoreError::new(format!(
                            "Linux Secret Service diagnostic stream failed: {error}"
                        )));
                    }
                }
            }

            if let Some(exit_status) = status {
                if input.is_none() && !stdout_open && !stderr_open {
                    if stdout_overflow || stderr_overflow {
                        return Err(SecretStoreError::new(format!(
                            "Linux Secret Service {name} command returned too much output"
                        )));
                    }
                    return Ok(Output {
                        status: exit_status,
                        stdout: stdout_bytes,
                        stderr: stderr_bytes,
                    });
                }
            }

            if started.elapsed() >= timeout {
                Self::terminate_command_group(&mut child, process_group);
                return Err(SecretStoreError::new(format!(
                    "Linux Secret Service did not respond within {} seconds. Start or unlock the desktop password store, then try again.",
                    timeout.as_secs()
                )));
            }
            if !progressed {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }

    fn run_secret_tool(args: &[&str], stdin: Option<&str>) -> Result<Output, SecretStoreError> {
        let tool = Self::secret_tool()
            .ok_or_else(|| {
                SecretStoreError::new(
                    "Linux secret-tool is not installed or is not executable. Install the desktop Secret Service tools, then try again.",
                )
            })?;
        Self::run_command(&tool, args, stdin, "secret-tool", Self::COMMAND_TIMEOUT)
    }

    fn default_collection_path(stdout: &[u8]) -> Option<&str> {
        let stdout = std::str::from_utf8(stdout).ok()?;
        let marker = "objectpath '";
        let start = stdout.find(marker)? + marker.len();
        let remainder = &stdout[start..];
        let end = remainder.find('\'')?;
        Some(&remainder[..end])
    }

    fn no_default_collection_error() -> SecretStoreError {
        SecretStoreError::new(
            "Linux Secret Service has no usable default password collection. Create or unlock a default keyring in the desktop password manager, then try again.",
        )
    }

    fn service_unavailable_error() -> SecretStoreError {
        SecretStoreError::new(
            "Linux Secret Service is not available in this desktop session. Start the desktop password-store service, then try again.",
        )
    }

    fn check_default_collection_with(
        gdbus: &Path,
        timeout: Duration,
    ) -> Result<(), SecretStoreError> {
        let output = Self::run_command(
            gdbus,
            &[
                "call",
                "--session",
                "--dest",
                "org.freedesktop.secrets",
                "--object-path",
                "/org/freedesktop/secrets",
                "--method",
                "org.freedesktop.Secret.Service.ReadAlias",
                "default",
            ],
            None,
            "status-check",
            timeout,
        )?;
        if !output.status.success() {
            return Err(Self::service_unavailable_error());
        }
        match Self::default_collection_path(&output.stdout) {
            Some(path) if path != "/" => Ok(()),
            Some(_) => Err(Self::no_default_collection_error()),
            None => Err(SecretStoreError::new(
                "Linux Secret Service returned an invalid default password collection. Restart the desktop password-store service, then try again.",
            )),
        }
    }

    fn ensure_available() -> Result<(), SecretStoreError> {
        if Self::secret_tool().is_none() {
            return Err(SecretStoreError::new(
                "Linux secret-tool is not installed or is not executable. Install the desktop Secret Service tools, then try again.",
            ));
        }
        let gdbus = Self::gdbus().ok_or_else(|| {
            SecretStoreError::new(
                "Linux Secret Service status checking requires the gdbus utility. Install the desktop GLib tools, then try again.",
            )
        })?;
        Self::check_default_collection_with(&gdbus, Self::COMMAND_TIMEOUT)
    }

    fn output_error(action: &str, output: &Output) -> SecretStoreError {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        let normalized = stderr.to_lowercase();
        let detail = if normalized.contains("object does not exist at path")
            || normalized.contains("no such object")
        {
            return Self::no_default_collection_error();
        } else if normalized.contains("serviceunknown")
            || normalized.contains("namehasnoowner")
            || normalized.contains("cannot autolaunch")
            || normalized.contains("cannot connect")
            || normalized.contains("was not provided by any .service")
        {
            return Self::service_unavailable_error();
        } else if normalized.contains("locked") || normalized.contains("prompt dismissed") {
            format!(
                "Linux Secret Service {action} failed because the desktop password store is locked. Unlock it, then try again."
            )
        } else {
            format!(
                "Linux Secret Service {action} failed with status {}",
                output.status
            )
        };
        SecretStoreError::new(detail)
    }

    fn missing(output: &Output) -> bool {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stderr = stderr.trim().to_lowercase();
        output.status.code() == Some(1)
            && (stderr.is_empty() || stderr.contains("couldn't find matching"))
    }
}

#[cfg(any(target_os = "linux", test))]
fn linux_secret_tool_attributes(path: &Path) -> [String; 4] {
    [
        "service".to_owned(),
        SERVICE.to_owned(),
        "account".to_owned(),
        archive_account(path),
    ]
}

#[cfg(target_os = "linux")]
impl SecretStore for LinuxSecretServiceStore {
    fn is_available(&self) -> bool {
        Self::ensure_available().is_ok()
    }

    fn get_archive_password(&self, path: &Path) -> Result<Option<Password>, SecretStoreError> {
        if Self::ensure_available().is_err() {
            return Ok(None);
        }
        let attrs = linux_secret_tool_attributes(path);
        let args: Vec<&str> = std::iter::once("lookup")
            .chain(attrs.iter().map(String::as_str))
            .collect();
        let output = Self::run_secret_tool(&args, None)?;
        if output.status.success() {
            let mut password = String::from_utf8_lossy(&output.stdout).into_owned();
            while password.ends_with('\n') || password.ends_with('\r') {
                password.pop();
            }
            Ok(Some(Password::new(password)))
        } else if Self::missing(&output) {
            Ok(None)
        } else {
            Err(Self::output_error("read", &output))
        }
    }

    fn set_archive_password(&self, path: &Path, password: &str) -> Result<(), SecretStoreError> {
        Self::ensure_available()?;
        let label = archive_label(path);
        let attrs = linux_secret_tool_attributes(path);
        let args: Vec<&str> = ["store", "--label", label.as_str()]
            .into_iter()
            .chain(attrs.iter().map(String::as_str))
            .collect();
        let output = Self::run_secret_tool(&args, Some(password))?;
        if output.status.success() {
            Ok(())
        } else {
            Err(Self::output_error("write", &output))
        }
    }

    fn delete_archive_password(&self, path: &Path) -> Result<(), SecretStoreError> {
        Self::ensure_available()?;
        let attrs = linux_secret_tool_attributes(path);
        let args: Vec<&str> = std::iter::once("clear")
            .chain(attrs.iter().map(String::as_str))
            .collect();
        let output = Self::run_secret_tool(&args, None)?;
        if output.status.success() || Self::missing(&output) {
            Ok(())
        } else {
            Err(Self::output_error("delete", &output))
        }
    }
}

#[cfg(target_os = "windows")]
struct WindowsCredentialManagerSecretStore;

#[cfg(target_os = "windows")]
impl WindowsCredentialManagerSecretStore {
    fn credential_target_name(path: &Path) -> String {
        windows_credential_target_name(path)
    }

    fn to_wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn last_error() -> u32 {
        // SAFETY: GetLastError has no preconditions and only reads the thread-local
        // Win32 error set by the immediately preceding Credential Manager call.
        unsafe { windows_sys::Win32::Foundation::GetLastError() }
    }

    fn missing_error(code: u32) -> bool {
        matches!(
            code,
            windows_sys::Win32::Foundation::ERROR_NOT_FOUND
                | windows_sys::Win32::Foundation::ERROR_NO_SUCH_LOGON_SESSION
        )
    }

    fn output_error(action: &str, code: u32) -> SecretStoreError {
        SecretStoreError::new(format!(
            "Windows Credential Manager {action} failed with Win32 error {code}"
        ))
    }
}

#[cfg(target_os = "windows")]
struct WindowsCredentialHandle(*mut windows_sys::Win32::Security::Credentials::CREDENTIALW);

#[cfg(target_os = "windows")]
impl Drop for WindowsCredentialHandle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: CredReadW returns this pointer on success and documents that
            // callers release it with CredFree exactly once.
            unsafe { windows_sys::Win32::Security::Credentials::CredFree(self.0.cast()) };
        }
    }
}

#[cfg(any(target_os = "windows", test))]
fn windows_credential_target_name(path: &Path) -> String {
    format!("{SERVICE}:{}", archive_account(path))
}

#[cfg(target_os = "windows")]
impl SecretStore for WindowsCredentialManagerSecretStore {
    fn is_available(&self) -> bool {
        true
    }

    fn get_archive_password(&self, path: &Path) -> Result<Option<Password>, SecretStoreError> {
        use windows_sys::Win32::Security::Credentials::{
            CredReadW, CREDENTIALW, CRED_TYPE_GENERIC,
        };

        let target = Self::credential_target_name(path);
        let target = Self::to_wide(&target);
        let mut raw: *mut CREDENTIALW = ptr::null_mut();
        // SAFETY: target is a NUL-terminated UTF-16 string and raw is a valid
        // out-pointer. On success, raw is owned by Windows and freed by the guard.
        let ok = unsafe { CredReadW(target.as_ptr(), CRED_TYPE_GENERIC, 0, &mut raw) };
        if ok == 0 {
            let code = Self::last_error();
            return if Self::missing_error(code) {
                Ok(None)
            } else {
                Err(Self::output_error("read", code))
            };
        }
        let handle = WindowsCredentialHandle(raw);
        // SAFETY: CredReadW succeeded, so handle.0 points to a valid CREDENTIALW
        // until the guard drops.
        let credential = unsafe { &*handle.0 };
        let bytes = if credential.CredentialBlobSize == 0 || credential.CredentialBlob.is_null() {
            &[][..]
        } else {
            // SAFETY: Windows returns CredentialBlob with CredentialBlobSize bytes
            // valid for the lifetime of the credential handle.
            unsafe {
                slice::from_raw_parts(
                    credential.CredentialBlob,
                    credential.CredentialBlobSize as usize,
                )
            }
        };
        let password = String::from_utf8(bytes.to_vec()).map_err(|e| {
            SecretStoreError::new(format!(
                "Windows Credential Manager read returned non-UTF-8 password bytes: {e}"
            ))
        })?;
        Ok(Some(Password::new(password)))
    }

    fn set_archive_password(&self, path: &Path, password: &str) -> Result<(), SecretStoreError> {
        use windows_sys::Win32::Security::Credentials::{
            CredWriteW, CREDENTIALW, CRED_MAX_CREDENTIAL_BLOB_SIZE, CRED_PERSIST_LOCAL_MACHINE,
            CRED_TYPE_GENERIC,
        };

        let bytes = password.as_bytes();
        if bytes.len() > CRED_MAX_CREDENTIAL_BLOB_SIZE as usize {
            return Err(SecretStoreError::new(format!(
                "Windows Credential Manager password is too large: {} bytes",
                bytes.len()
            )));
        }

        let target = Self::credential_target_name(path);
        let mut target = Self::to_wide(&target);
        let label = archive_label(path);
        let mut comment = Self::to_wide(&label);
        let mut user_name = Self::to_wide("Squallz");
        let mut credential = CREDENTIALW {
            Type: CRED_TYPE_GENERIC,
            TargetName: target.as_mut_ptr(),
            Comment: comment.as_mut_ptr(),
            CredentialBlobSize: bytes.len() as u32,
            CredentialBlob: if bytes.is_empty() {
                ptr::null_mut()
            } else {
                bytes.as_ptr() as *mut u8
            },
            Persist: CRED_PERSIST_LOCAL_MACHINE,
            UserName: user_name.as_mut_ptr(),
            ..Default::default()
        };

        // SAFETY: credential points at NUL-terminated UTF-16 fields and a valid
        // password byte slice for the duration of the call. CredWriteW copies it.
        let ok = unsafe { CredWriteW(&mut credential, 0) };
        if ok == 0 {
            Err(Self::output_error("write", Self::last_error()))
        } else {
            Ok(())
        }
    }

    fn delete_archive_password(&self, path: &Path) -> Result<(), SecretStoreError> {
        use windows_sys::Win32::Security::Credentials::{CredDeleteW, CRED_TYPE_GENERIC};

        let target = Self::credential_target_name(path);
        let target = Self::to_wide(&target);
        // SAFETY: target is a NUL-terminated UTF-16 string and flags=0 follows
        // the Credential Manager contract for generic credentials.
        let ok = unsafe { CredDeleteW(target.as_ptr(), CRED_TYPE_GENERIC, 0) };
        if ok != 0 {
            return Ok(());
        }
        let code = Self::last_error();
        if Self::missing_error(code) {
            Ok(())
        } else {
            Err(Self::output_error("delete", code))
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
struct UnavailableSecretStore;

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
impl SecretStore for UnavailableSecretStore {
    fn is_available(&self) -> bool {
        false
    }

    fn get_archive_password(&self, _path: &Path) -> Result<Option<Password>, SecretStoreError> {
        Ok(None)
    }

    fn set_archive_password(&self, _path: &Path, _password: &str) -> Result<(), SecretStoreError> {
        Err(SecretStoreError::new(
            "persistent secret storage is not available on this platform",
        ))
    }

    fn delete_archive_password(&self, _path: &Path) -> Result<(), SecretStoreError> {
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::collections::HashMap;
    #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
    use std::env;
    #[cfg(target_os = "linux")]
    use std::fs;
    #[cfg(target_os = "linux")]
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::sync::{Mutex, MutexGuard};
    #[cfg(target_os = "linux")]
    use std::time::{Duration, Instant};

    #[cfg(target_os = "linux")]
    use super::LinuxSecretServiceStore;
    #[cfg(target_os = "macos")]
    use super::MacOsKeychainSecretStore;
    #[cfg(target_os = "windows")]
    use super::WindowsCredentialManagerSecretStore;
    use super::{
        archive_account, archive_label, linux_secret_tool_attributes,
        windows_credential_target_name, Password, SecretStore, SecretStoreError,
    };

    pub(crate) struct MemorySecretStore {
        passwords: Mutex<HashMap<PathBuf, String>>,
    }

    pub(crate) struct ReadFailingSecretStore;

    impl SecretStore for ReadFailingSecretStore {
        fn is_available(&self) -> bool {
            true
        }

        fn get_archive_password(&self, _path: &Path) -> Result<Option<Password>, SecretStoreError> {
            Err(SecretStoreError::new("secret store is locked"))
        }

        fn set_archive_password(
            &self,
            _path: &Path,
            _password: &str,
        ) -> Result<(), SecretStoreError> {
            Err(SecretStoreError::new("secret store is locked"))
        }

        fn delete_archive_password(&self, _path: &Path) -> Result<(), SecretStoreError> {
            Err(SecretStoreError::new("secret store is locked"))
        }
    }

    fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
        match mutex.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    impl MemorySecretStore {
        pub(crate) fn new() -> Self {
            Self {
                passwords: Mutex::new(HashMap::new()),
            }
        }

        pub(crate) fn insert(&self, path: PathBuf, password: &str) {
            lock_unpoisoned(&self.passwords).insert(path, password.to_owned());
        }
    }

    impl SecretStore for MemorySecretStore {
        fn is_available(&self) -> bool {
            true
        }

        fn get_archive_password(&self, path: &Path) -> Result<Option<Password>, SecretStoreError> {
            Ok(lock_unpoisoned(&self.passwords)
                .get(path)
                .map(|pw| Password::new(pw.clone())))
        }

        fn set_archive_password(
            &self,
            path: &Path,
            password: &str,
        ) -> Result<(), SecretStoreError> {
            self.insert(path.to_path_buf(), password);
            Ok(())
        }

        fn delete_archive_password(&self, path: &Path) -> Result<(), SecretStoreError> {
            lock_unpoisoned(&self.passwords).remove(path);
            Ok(())
        }
    }

    fn poison_lock<T>(mutex: &Mutex<T>) {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = mutex.lock().unwrap();
            panic!("poison lock for regression coverage");
        }));
        assert!(result.is_err());
    }

    #[test]
    fn archive_account_is_stable_and_namespaced() {
        let account = archive_account(Path::new("/tmp/demo.7z"));
        assert_eq!(account, "archive:/tmp/demo.7z");
    }

    #[test]
    fn archive_label_uses_file_name_without_exposing_password() {
        let label = archive_label(Path::new("/tmp/demo.7z"));
        assert_eq!(label, "Squallz archive password: demo.7z");
        assert!(!label.contains("secret"));
    }

    #[test]
    fn linux_secret_tool_attributes_are_namespaced() {
        let attrs = linux_secret_tool_attributes(Path::new("/tmp/demo.7z"));
        assert_eq!(
            attrs,
            [
                "service".to_owned(),
                "com.squallz.archive-password".to_owned(),
                "account".to_owned(),
                "archive:/tmp/demo.7z".to_owned(),
            ],
        );
    }

    #[cfg(target_os = "linux")]
    fn linux_test_script(body: &str) -> (tempfile::TempDir, PathBuf) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("command");
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&path, permissions).unwrap();
        (directory, path)
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_secret_service_requires_a_default_collection() {
        let (_directory, missing) = linux_test_script("printf \"(objectpath '/',)\\n\"");
        let error = LinuxSecretServiceStore::check_default_collection_with(
            &missing,
            Duration::from_secs(1),
        )
        .unwrap_err();
        assert!(error.to_string().contains("default password collection"));

        let (_directory, available) = linux_test_script(
            "printf \"(objectpath '/org/freedesktop/secrets/collection/login',)\\n\"",
        );
        LinuxSecretServiceStore::check_default_collection_with(&available, Duration::from_secs(1))
            .unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_secret_service_command_has_a_hard_timeout() {
        let (_directory, command) = linux_test_script("exec sleep 30");
        let started = Instant::now();
        let error = LinuxSecretServiceStore::run_command(
            &command,
            &[],
            None,
            "test",
            Duration::from_millis(50),
        )
        .unwrap_err();

        assert!(error.to_string().contains("did not respond"));
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_secret_service_timeout_covers_blocked_secret_input() {
        let (_directory, command) = linux_test_script("exec sleep 1");
        let secret = format!("sensitive-marker{}", "x".repeat(4 * 1024 * 1024));
        let started = Instant::now();
        let error = LinuxSecretServiceStore::run_command(
            &command,
            &[],
            Some(&secret),
            "test",
            Duration::from_millis(50),
        )
        .unwrap_err();

        assert!(error.to_string().contains("did not respond"));
        assert!(!error.to_string().contains("sensitive-marker"));
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_secret_service_timeout_covers_descendants_holding_pipes() {
        let (_directory, command) = linux_test_script("sleep 30 & exit 0");
        let started = Instant::now();
        let error = LinuxSecretServiceStore::run_command(
            &command,
            &[],
            None,
            "test",
            Duration::from_millis(50),
        )
        .unwrap_err();

        assert!(error.to_string().contains("did not respond"));
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_secret_service_rejects_success_without_consuming_secret_input() {
        let (_directory, command) = linux_test_script("exit 0");
        let secret = format!("sensitive-marker{}", "x".repeat(4 * 1024 * 1024));
        let started = Instant::now();
        let error = LinuxSecretServiceStore::run_command(
            &command,
            &[],
            Some(&secret),
            "test",
            Duration::from_secs(1),
        )
        .unwrap_err();

        assert!(error.to_string().contains("input closed"));
        assert!(!error.to_string().contains("sensitive-marker"));
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_secret_service_drains_large_command_output() {
        let (_directory, command) =
            linux_test_script("head -c 262144 /dev/zero; printf diagnostics >&2");
        let started = Instant::now();
        let output = LinuxSecretServiceStore::run_command(
            &command,
            &[],
            None,
            "test",
            Duration::from_secs(1),
        )
        .unwrap();

        assert!(output.status.success());
        assert_eq!(output.stdout.len(), 262_144);
        assert_eq!(output.stderr, b"diagnostics");
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_secret_service_timeout_is_fair_under_continuous_output() {
        let (_directory, command) = linux_test_script("while :; do printf 0123456789abcdef; done");
        let started = Instant::now();
        let error = LinuxSecretServiceStore::run_command(
            &command,
            &[],
            None,
            "test",
            Duration::from_millis(50),
        )
        .unwrap_err();

        assert!(error.to_string().contains("did not respond"));
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_secret_service_executable_check_rejects_plain_files() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("secret-tool");
        fs::write(&path, "not executable").unwrap();

        assert!(!LinuxSecretServiceStore::is_executable(&path));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_secret_service_missing_requires_the_expected_exit_status() {
        let (_directory, missing) = linux_test_script("exit 1");
        let output = LinuxSecretServiceStore::run_command(
            &missing,
            &[],
            None,
            "test",
            Duration::from_secs(1),
        )
        .unwrap();
        assert!(LinuxSecretServiceStore::missing(&output));

        let (_directory, failed) = linux_test_script("exit 2");
        let output = LinuxSecretServiceStore::run_command(
            &failed,
            &[],
            None,
            "test",
            Duration::from_secs(1),
        )
        .unwrap();
        assert!(!LinuxSecretServiceStore::missing(&output));
    }

    #[test]
    fn windows_credential_target_name_is_namespaced() {
        let target = windows_credential_target_name(Path::new("C:\\tmp\\demo.7z"));
        assert_eq!(
            target,
            "com.squallz.archive-password:archive:C:\\tmp\\demo.7z"
        );
        assert!(!target.contains("secret"));
    }

    #[test]
    fn memory_secret_store_recovers_after_poison() {
        let store = MemorySecretStore::new();
        let path = PathBuf::from("/tmp/poisoned-memory-store.7z");
        poison_lock(&store.passwords);

        store.set_archive_password(&path, "secret").unwrap();
        assert!(store.has_archive_password(&path).unwrap());
        let saved = store.get_archive_password(&path).unwrap();
        assert_eq!(saved.as_ref().map(Password::expose), Some("secret"));

        store.delete_archive_password(&path).unwrap();
        assert!(!store.has_archive_password(&path).unwrap());
    }

    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "touches the user's macOS Keychain; run scripts/macos_keychain_smoke.sh"]
    fn macos_keychain_write_read_delete_validation() {
        if env::var("SQUALLZ_KEYCHAIN_VALIDATION").ok().as_deref() != Some("1") {
            eprintln!("set SQUALLZ_KEYCHAIN_VALIDATION=1 or use scripts/macos_keychain_smoke.sh");
            return;
        }

        let path = env::var_os("SQUALLZ_KEYCHAIN_VALIDATION_ARCHIVE")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/tmp/squallz-keychain-validation.7z"));
        let password = env::var("SQUALLZ_KEYCHAIN_VALIDATION_PASSWORD")
            .unwrap_or_else(|_| "squallz-keychain-validation-secret".to_owned());
        let store = MacOsKeychainSecretStore;

        assert!(store.is_available());
        store.delete_archive_password(&path).unwrap();
        assert!(!store.has_archive_password(&path).unwrap());

        store.set_archive_password(&path, &password).unwrap();
        assert!(store.has_archive_password(&path).unwrap());
        let saved = store
            .get_archive_password(&path)
            .unwrap()
            .expect("saved password should be readable");
        assert!(saved.expose() == password, "saved password mismatch");

        store.delete_archive_password(&path).unwrap();
        assert!(!store.has_archive_password(&path).unwrap());
    }

    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "touches the user's Linux Secret Service; run scripts/linux_secret_service_smoke.sh"]
    fn linux_secret_service_write_read_delete_validation() {
        if env::var("SQUALLZ_SECRET_SERVICE_VALIDATION")
            .ok()
            .as_deref()
            != Some("1")
        {
            eprintln!(
                "set SQUALLZ_SECRET_SERVICE_VALIDATION=1 or use scripts/linux_secret_service_smoke.sh"
            );
            return;
        }

        let path = env::var_os("SQUALLZ_SECRET_SERVICE_VALIDATION_ARCHIVE")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/tmp/squallz-secret-service-validation.7z"));
        let password = env::var("SQUALLZ_SECRET_SERVICE_VALIDATION_PASSWORD")
            .unwrap_or_else(|_| "squallz-secret-service-validation-secret".to_owned());
        let store = LinuxSecretServiceStore;

        assert!(store.is_available());
        store.delete_archive_password(&path).unwrap();
        assert!(!store.has_archive_password(&path).unwrap());

        store.set_archive_password(&path, &password).unwrap();
        assert!(store.has_archive_password(&path).unwrap());
        let saved = store
            .get_archive_password(&path)
            .unwrap()
            .expect("saved password should be readable");
        assert_eq!(saved.expose(), password);

        store.delete_archive_password(&path).unwrap();
        assert!(!store.has_archive_password(&path).unwrap());
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "touches the user's Windows Credential Manager; run scripts/windows_credential_manager_smoke.ps1"]
    fn windows_credential_manager_write_read_delete_validation() {
        if env::var("SQUALLZ_CREDENTIAL_VALIDATION").ok().as_deref() != Some("1") {
            eprintln!(
                "set SQUALLZ_CREDENTIAL_VALIDATION=1 or use scripts/windows_credential_manager_smoke.ps1"
            );
            return;
        }

        let path = env::var_os("SQUALLZ_CREDENTIAL_VALIDATION_ARCHIVE")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("C:\\Temp\\squallz-credential-validation.7z"));
        let password = env::var("SQUALLZ_CREDENTIAL_VALIDATION_PASSWORD")
            .unwrap_or_else(|_| "squallz-credential-validation-secret".to_owned());
        let store = WindowsCredentialManagerSecretStore;

        assert!(store.is_available());
        store.delete_archive_password(&path).unwrap();
        assert!(!store.has_archive_password(&path).unwrap());

        store.set_archive_password(&path, &password).unwrap();
        assert!(store.has_archive_password(&path).unwrap());
        let saved = store
            .get_archive_password(&path)
            .unwrap()
            .expect("saved password should be readable");
        assert_eq!(saved.expose(), password);

        store.delete_archive_password(&path).unwrap();
        assert!(!store.has_archive_password(&path).unwrap());
    }
}
