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
use std::{env, io::Write, path::PathBuf, process::Stdio};
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
    fn secret_tool() -> Option<PathBuf> {
        if let Some(path) = env::var_os("SQUALLZ_SECRET_TOOL") {
            let path = PathBuf::from(path);
            return path.is_file().then_some(path);
        }
        let path = env::var_os("PATH")?;
        env::split_paths(&path)
            .map(|dir| dir.join("secret-tool"))
            .find(|candidate| candidate.is_file())
    }

    fn run_secret_tool(args: &[&str], stdin: Option<&str>) -> Result<Output, SecretStoreError> {
        let tool = Self::secret_tool()
            .ok_or_else(|| SecretStoreError::new("Linux secret-tool is not available"))?;
        let mut command = Command::new(tool);
        command
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(secret) = stdin {
            let mut child = command
                .stdin(Stdio::piped())
                .spawn()
                .map_err(|e| SecretStoreError::new(format!("secret-tool failed: {e}")))?;
            let mut input = child
                .stdin
                .take()
                .ok_or_else(|| SecretStoreError::new("secret-tool stdin is unavailable"))?;
            input
                .write_all(secret.as_bytes())
                .map_err(|e| SecretStoreError::new(format!("secret-tool stdin failed: {e}")))?;
            drop(input);
            child
                .wait_with_output()
                .map_err(|e| SecretStoreError::new(format!("secret-tool failed: {e}")))
        } else {
            command
                .output()
                .map_err(|e| SecretStoreError::new(format!("secret-tool failed: {e}")))
        }
    }

    fn output_error(action: &str, output: &Output) -> SecretStoreError {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        let detail = if stderr.is_empty() {
            format!(
                "Linux Secret Service {action} failed with status {}",
                output.status
            )
        } else {
            format!("Linux Secret Service {action} failed: {stderr}")
        };
        SecretStoreError::new(detail)
    }

    fn missing(output: &Output) -> bool {
        let stderr = String::from_utf8_lossy(&output.stderr).to_lowercase();
        stderr.is_empty()
            || stderr.contains("not found")
            || stderr.contains("no such")
            || stderr.contains("couldn't find")
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
        Self::secret_tool().is_some()
    }

    fn get_archive_password(&self, path: &Path) -> Result<Option<Password>, SecretStoreError> {
        if !self.is_available() {
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
        if !self.is_available() {
            return Err(SecretStoreError::new(
                "Linux Secret Service is not available",
            ));
        }
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
        if !self.is_available() {
            return Ok(());
        }
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
    use std::path::{Path, PathBuf};
    use std::sync::{Mutex, MutexGuard};

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
