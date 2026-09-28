//! Atomic filesystem moves shared by extraction and archive publication.

use std::io;
use std::path::Path;

/// Moves an entry on the same filesystem without replacing its destination.
/// Symbolic links move as links; their targets are never followed. An existing
/// destination returns [`io::ErrorKind::AlreadyExists`] and leaves both entries
/// unchanged.
pub fn move_path_no_replace(src: &Path, dest: &Path) -> io::Result<()> {
    move_path_no_replace_impl(src, dest)
}

#[cfg(any(target_os = "android", target_os = "linux", target_vendor = "apple"))]
fn move_path_no_replace_impl(src: &Path, dest: &Path) -> io::Result<()> {
    use rustix::fs::{renameat_with, RenameFlags, CWD};
    renameat_with(CWD, src, CWD, dest, RenameFlags::NOREPLACE).map_err(Into::into)
}

#[cfg(windows)]
fn move_path_no_replace_impl(src: &Path, dest: &Path) -> io::Result<()> {
    use windows_sys::Win32::Foundation::{ERROR_ALREADY_EXISTS, ERROR_FILE_EXISTS};
    let error = match move_windows_path(src, dest, 0) {
        Ok(()) => return Ok(()),
        Err(error) => error,
    };
    match error.raw_os_error() {
        Some(code) if code == ERROR_ALREADY_EXISTS as i32 || code == ERROR_FILE_EXISTS as i32 => {
            Err(io::Error::new(io::ErrorKind::AlreadyExists, error))
        }
        _ => Err(error),
    }
}

#[cfg(not(any(
    target_os = "android",
    target_os = "linux",
    target_vendor = "apple",
    windows
)))]
fn move_path_no_replace_impl(_src: &Path, _dest: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "atomic no-replace rename is unavailable on this platform",
    ))
}

/// Atomically replaces a file with another entry on the same filesystem.
/// Windows retries transient sharing locks without falling back to a copy.
#[cfg(unix)]
pub fn atomic_replace_file(src: &Path, dest: &Path) -> io::Result<()> {
    std::fs::rename(src, dest)
}

/// Atomically replaces a file with another entry on the same filesystem.
/// Windows retries transient sharing locks without falling back to a copy.
#[cfg(windows)]
pub fn atomic_replace_file(src: &Path, dest: &Path) -> io::Result<()> {
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };
    move_windows_path(
        src,
        dest,
        MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
    )
}

/// Atomically replaces a file with another entry on the same filesystem.
#[cfg(not(any(unix, windows)))]
pub fn atomic_replace_file(_src: &Path, _dest: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "atomic file replacement is unavailable on this platform",
    ))
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn move_windows_path(src: &Path, dest: &Path, flags: u32) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::MoveFileExW;

    fn wide_path(path: &Path) -> io::Result<Vec<u16>> {
        let mut value: Vec<u16> = path.as_os_str().encode_wide().collect();
        if value.contains(&0) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "path contains a null character",
            ));
        }
        value.push(0);
        Ok(value)
    }

    let src = wide_path(src)?;
    let dest = wide_path(dest)?;
    // SAFETY: both buffers are valid null-terminated UTF-16 strings for this
    // synchronous call. Callers only use zero or replacement/write-through
    // flags; COPY_ALLOWED is omitted to prohibit non-atomic copy/delete.
    retry_windows_file_operation(|| unsafe { MoveFileExW(src.as_ptr(), dest.as_ptr(), flags) != 0 })
}

#[cfg(windows)]
fn retry_windows_file_operation(mut operation: impl FnMut() -> bool) -> io::Result<()> {
    use std::time::{Duration, Instant};
    use windows_sys::Win32::Foundation::{ERROR_LOCK_VIOLATION, ERROR_SHARING_VIOLATION};

    const RETRY_WINDOW: Duration = Duration::from_secs(2);
    const RETRY_DELAY: Duration = Duration::from_millis(50);
    let deadline = Instant::now() + RETRY_WINDOW;
    loop {
        if operation() {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        if !matches!(error.raw_os_error(), Some(code) if code == ERROR_SHARING_VIOLATION as i32 || code == ERROR_LOCK_VIOLATION as i32)
            || Instant::now() >= deadline
        {
            return Err(error);
        }
        std::thread::sleep(RETRY_DELAY);
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let path =
            std::env::temp_dir().join(format!("squallz-file-ops-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[cfg(windows)]
    #[test]
    #[allow(unsafe_code)]
    fn windows_file_operation_retries_only_transient_share_locks() {
        use std::cell::Cell;

        use windows_sys::Win32::Foundation::{
            SetLastError, ERROR_ACCESS_DENIED, ERROR_SHARING_VIOLATION,
        };

        let attempts = Cell::new(0u32);
        retry_windows_file_operation(|| {
            let attempt = attempts.get() + 1;
            attempts.set(attempt);
            if attempt < 3 {
                // SAFETY: the test controls this thread and reads the error
                // immediately through retry_windows_file_operation.
                unsafe { SetLastError(ERROR_SHARING_VIOLATION) };
                false
            } else {
                true
            }
        })
        .unwrap();
        assert_eq!(attempts.get(), 3);

        let attempts = Cell::new(0u32);
        let error = retry_windows_file_operation(|| {
            attempts.set(attempts.get() + 1);
            // SAFETY: the test controls this thread and reads the error
            // immediately through retry_windows_file_operation.
            unsafe { SetLastError(ERROR_ACCESS_DENIED) };
            false
        })
        .unwrap_err();
        assert_eq!(attempts.get(), 1);
        assert_eq!(error.raw_os_error(), Some(ERROR_ACCESS_DENIED as i32));
    }

    #[cfg(windows)]
    #[test]
    fn windows_no_replace_move_preserves_existing_destination() {
        let dir = temp_dir("windows-rename-no-replace");
        let staged = dir.join("archive.tmp");
        let dest = dir.join("archive.zip");
        std::fs::write(&staged, b"new payload").unwrap();
        std::fs::write(&dest, b"existing payload").unwrap();

        let error = move_path_no_replace(&staged, &dest).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(&dest).unwrap(), b"existing payload");
        assert_eq!(std::fs::read(&staged).unwrap(), b"new payload");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn no_replace_move_supports_directories_and_preserves_conflicts() {
        let dir = temp_dir("directory-move-no-replace");
        let staged = dir.join("staged");
        let dest = dir.join("destination");
        std::fs::create_dir_all(&staged).unwrap();
        std::fs::write(staged.join("source.txt"), b"source directory").unwrap();
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(dest.join("existing.txt"), b"existing directory").unwrap();

        let error = move_path_no_replace(&staged, &dest).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(
            std::fs::read(staged.join("source.txt")).unwrap(),
            b"source directory"
        );
        assert_eq!(
            std::fs::read(dest.join("existing.txt")).unwrap(),
            b"existing directory"
        );

        std::fs::remove_dir_all(&dest).unwrap();
        move_path_no_replace(&staged, &dest).unwrap();
        assert!(!staged.exists());
        assert_eq!(
            std::fs::read(dest.join("source.txt")).unwrap(),
            b"source directory"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn no_replace_move_moves_a_symbolic_link_without_following_it() {
        use std::os::unix::fs::symlink;

        let dir = temp_dir("symlink-move-no-replace");
        let staged = dir.join("staged-link");
        let dest = dir.join("destination-link");
        symlink("source-target", &staged).unwrap();
        symlink("existing-target", &dest).unwrap();

        let error = move_path_no_replace(&staged, &dest).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(
            std::fs::read_link(&staged).unwrap(),
            Path::new("source-target")
        );
        assert_eq!(
            std::fs::read_link(&dest).unwrap(),
            Path::new("existing-target")
        );

        std::fs::remove_file(&dest).unwrap();
        move_path_no_replace(&staged, &dest).unwrap();
        assert!(matches!(
            std::fs::symlink_metadata(&staged),
            Err(ref error) if error.kind() == io::ErrorKind::NotFound
        ));
        assert_eq!(
            std::fs::read_link(&dest).unwrap(),
            Path::new("source-target")
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
