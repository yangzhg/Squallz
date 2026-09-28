//! Restore metadata through open handles, after content is complete.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io;
use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;

use crate::{
    ControlToken, EntryMeta, EntryPath, ExtractOptions, FormatError, PhysicalFileIdentity,
};

pub(super) fn restore_file(
    file: &File,
    meta: &EntryMeta,
    opts: &ExtractOptions,
) -> Result<(), FormatError> {
    restore_modified(file, meta.modified)?;
    restore_permissions(file, meta.unix_mode, opts.restore_permissions);
    Ok(())
}

fn restore_modified(file: &File, modified: Option<SystemTime>) -> io::Result<()> {
    if let Some(modified) = modified {
        file.set_times(fs::FileTimes::new().set_modified(modified))?;
    }
    Ok(())
}

pub(super) fn restore_symlink(
    staged: &Path,
    destination: &Path,
    modified: Option<SystemTime>,
) -> Result<(), FormatError> {
    let Some(modified) = modified else {
        return Ok(());
    };
    let file = open_symlink(staged)?;
    if !file.metadata()?.file_type().is_symlink() {
        return Err(FormatError::destination_changed(destination));
    }
    set_symlink_modified(&file, modified)?;
    Ok(())
}

#[cfg(target_vendor = "apple")]
fn open_symlink(path: &Path) -> io::Result<File> {
    use rustix::fs::{open, Mode, OFlags};
    // O_SYMLINK opens the link itself. O_NOFOLLOW would reject that open on
    // Apple platforms; NONBLOCK prevents a replaced FIFO from blocking us.
    Ok(File::from(open(
        path,
        OFlags::from_bits_retain(libc::O_SYMLINK as u32)
            | OFlags::RDONLY
            | OFlags::CLOEXEC
            | OFlags::NONBLOCK
            | OFlags::NOCTTY,
        Mode::empty(),
    )?))
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn open_symlink(path: &Path) -> io::Result<File> {
    use rustix::fs::{open, Mode, OFlags};
    Ok(File::from(open(
        path,
        OFlags::PATH | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )?))
}

#[cfg(windows)]
fn open_symlink(path: &Path) -> io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES,
        FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_WRITE_ATTRIBUTES,
    };
    fs::OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES | FILE_WRITE_ATTRIBUTES)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
}

#[cfg(not(any(
    target_vendor = "apple",
    target_os = "linux",
    target_os = "android",
    windows
)))]
fn open_symlink(_path: &Path) -> io::Result<File> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "safe symbolic-link metadata restoration is unavailable",
    ))
}

#[cfg(any(target_vendor = "apple", target_os = "linux", target_os = "android"))]
fn symlink_timestamps(modified: SystemTime) -> io::Result<rustix::fs::Timestamps> {
    use rustix::fs::{Timespec, Timestamps, UTIME_OMIT};
    let nanos = match modified.duration_since(std::time::UNIX_EPOCH) {
        Ok(duration) => duration.as_nanos() as i128,
        Err(error) => -(error.duration().as_nanos() as i128),
    };
    let seconds = nanos.div_euclid(1_000_000_000).try_into().map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "symbolic-link modification time is out of range",
        )
    })?;
    Ok(Timestamps {
        last_access: Timespec {
            tv_sec: 0,
            tv_nsec: UTIME_OMIT,
        },
        last_modification: Timespec {
            tv_sec: seconds,
            tv_nsec: nanos.rem_euclid(1_000_000_000) as _,
        },
    })
}

#[cfg(target_vendor = "apple")]
fn set_symlink_modified(file: &File, modified: SystemTime) -> io::Result<()> {
    rustix::fs::futimens(file, &symlink_timestamps(modified)?).map_err(Into::into)
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn set_symlink_modified(file: &File, modified: SystemTime) -> io::Result<()> {
    use rustix::fs::{utimensat, AtFlags};
    // An empty path binds the update to the opened link even if its name or
    // an ancestor is replaced. This operation requires Linux 5.8 or later.
    utimensat(
        file,
        "",
        &symlink_timestamps(modified)?,
        AtFlags::EMPTY_PATH | AtFlags::SYMLINK_NOFOLLOW,
    )
    .map_err(Into::into)
}

#[cfg(windows)]
fn set_symlink_modified(file: &File, modified: SystemTime) -> io::Result<()> {
    restore_modified(file, Some(modified))
}

#[cfg(not(any(
    target_vendor = "apple",
    target_os = "linux",
    target_os = "android",
    windows
)))]
fn set_symlink_modified(_file: &File, _modified: SystemTime) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "safe symbolic-link metadata restoration is unavailable",
    ))
}

#[cfg(unix)]
fn restore_permissions(file: &File, mode: Option<u32>, enabled: bool) {
    use std::os::unix::fs::PermissionsExt;
    if let Some(mode) = mode.filter(|_| enabled) {
        // Preserve the existing best-effort permission policy, using the
        // bound file instead of following a potentially replaced path.
        let _ = file.set_permissions(fs::Permissions::from_mode(mode & 0o7777));
    }
}

#[cfg(not(unix))]
fn restore_permissions(_file: &File, _mode: Option<u32>, _enabled: bool) {}

struct DirectoryMetadata {
    relative: PathBuf,
    entry: EntryPath,
    modified: Option<SystemTime>,
    mode: Option<u32>,
}

pub(super) struct DeferredDirectories {
    root: File,
    root_path: PathBuf,
    // Filesystem aliases (including case variants) share one final update;
    // the last selected archive entry supplies the metadata and lookup path.
    pending: HashMap<PhysicalFileIdentity, DirectoryMetadata>,
}

impl DeferredDirectories {
    pub(super) fn new(root: &Path) -> io::Result<Self> {
        Ok(Self {
            root: open_root(root)?,
            root_path: root.to_path_buf(),
            pending: HashMap::new(),
        })
    }

    pub(super) fn record(&mut self, relative: &Path, meta: &EntryMeta) -> io::Result<()> {
        let directory = self.open_relative(relative, false)?;
        self.pending.insert(
            identity(&directory.file)?,
            DirectoryMetadata {
                relative: relative.to_path_buf(),
                entry: meta.path.clone(),
                modified: meta.modified,
                mode: meta.unix_mode,
            },
        );
        Ok(())
    }

    pub(super) fn finish(
        mut self,
        opts: &ExtractOptions,
        ctl: &ControlToken,
        mut report_current: impl FnMut(&EntryPath),
    ) -> Result<(), FormatError> {
        let mut pending: Vec<_> = self.pending.drain().collect();
        // Children first: restoring a parent's read-only mode must not make
        // later descendants inaccessible. Keep one root handle, not one per
        // archived directory, so large trees do not exhaust file descriptors.
        pending.sort_unstable_by(|(_, a), (_, b)| {
            b.relative
                .components()
                .count()
                .cmp(&a.relative.components().count())
                .then_with(|| a.relative.cmp(&b.relative))
        });
        for (expected_identity, metadata) in pending {
            ctl.checkpoint()?;
            report_current(&metadata.entry);
            ctl.checkpoint()?;
            let directory = self.open_relative(&metadata.relative, true)?;
            if identity(&directory.file)? != expected_identity {
                return Err(FormatError::destination_changed(
                    self.root_path.join(&metadata.relative),
                ));
            }
            restore_modified(&directory.file, metadata.modified)?;
            restore_permissions(&directory.file, metadata.mode, opts.restore_permissions);
        }
        Ok(())
    }

    #[cfg(unix)]
    fn open_relative(&self, relative: &Path, _write: bool) -> io::Result<DirectoryHandle> {
        use rustix::fs::{openat, Mode, OFlags};
        let mut directory = self.root.try_clone()?;
        for component in relative.components() {
            let Component::Normal(name) = component else {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "invalid extraction directory",
                ));
            };
            directory = File::from(openat(
                &directory,
                name,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )?);
        }
        Ok(DirectoryHandle { file: directory })
    }

    #[cfg(windows)]
    fn open_relative(&self, relative: &Path, write: bool) -> io::Result<DirectoryHandle> {
        let mut path = self.root_path.clone();
        let mut parents = vec![self.root.try_clone()?];
        let mut components = relative.components().peekable();
        while let Some(component) = components.next() {
            let Component::Normal(name) = component else {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "invalid extraction directory",
                ));
            };
            path.push(name);
            let directory = open_windows_directory(&path, write && components.peek().is_none())?;
            if components.peek().is_none() {
                return Ok(DirectoryHandle {
                    file: directory,
                    _parents: parents,
                });
            }
            // Deny delete sharing while walking: ancestors cannot be renamed
            // or replaced by junctions between validation and the final open.
            parents.push(directory);
        }
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "empty extraction directory",
        ))
    }

    #[cfg(not(any(unix, windows)))]
    fn open_relative(&self, _relative: &Path, _write: bool) -> io::Result<DirectoryHandle> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "safe directory metadata restoration is unavailable",
        ))
    }
}

struct DirectoryHandle {
    file: File,
    #[cfg(windows)]
    _parents: Vec<File>,
}

#[cfg(unix)]
fn open_root(path: &Path) -> io::Result<File> {
    use rustix::fs::{open, Mode, OFlags};
    Ok(File::from(open(
        path,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )?))
}

#[cfg(windows)]
fn open_root(path: &Path) -> io::Result<File> {
    open_windows_directory(path, false)
}

#[cfg(windows)]
fn open_windows_directory(path: &Path, write: bool) -> io::Result<File> {
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
        FILE_READ_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_WRITE_ATTRIBUTES,
    };
    let access = FILE_READ_ATTRIBUTES | if write { FILE_WRITE_ATTRIBUTES } else { 0 };
    let file = fs::OpenOptions::new()
        .access_mode(access)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_dir() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "extraction directory was replaced or is a reparse point",
        ));
    }
    Ok(file)
}

#[cfg(not(any(unix, windows)))]
fn open_root(_path: &Path) -> io::Result<File> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "safe directory metadata restoration is unavailable",
    ))
}

#[cfg(unix)]
fn identity(file: &File) -> io::Result<PhysicalFileIdentity> {
    use std::os::unix::fs::MetadataExt;
    let metadata = file.metadata()?;
    Ok(PhysicalFileIdentity::new(metadata.dev(), metadata.ino()))
}

#[cfg(windows)]
fn identity(file: &File) -> io::Result<PhysicalFileIdentity> {
    let information = winapi_util::file::information(file)?;
    let identity =
        PhysicalFileIdentity::new(information.volume_serial_number(), information.file_index());
    if identity.filesystem() == 0 && identity.file() == 0 {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "stable directory identity is unavailable",
        ));
    }
    Ok(identity)
}

#[cfg(not(any(unix, windows)))]
fn identity(_file: &File) -> io::Result<PhysicalFileIdentity> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "stable directory identity is unavailable",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, UNIX_EPOCH};

    #[cfg(any(target_vendor = "apple", target_os = "linux", target_os = "android"))]
    #[test]
    fn symlink_times_preserve_subseconds_on_both_sides_of_the_epoch() {
        for (time, seconds, nanos) in [
            (UNIX_EPOCH, 0, 0),
            (UNIX_EPOCH + Duration::new(1, 123_456_700), 1, 123_456_700),
            (UNIX_EPOCH - Duration::from_nanos(1), -1, 999_999_999),
            (UNIX_EPOCH - Duration::new(1, 123_456_700), -2, 876_543_300),
        ] {
            let times = symlink_timestamps(time).unwrap();
            assert_eq!(times.last_access.tv_nsec, rustix::fs::UTIME_OMIT);
            assert_eq!(times.last_modification.tv_sec, seconds);
            assert_eq!(times.last_modification.tv_nsec, nanos);
        }
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn symlink_metadata_rejects_a_regular_file_without_changing_it() {
        let root =
            std::env::temp_dir().join(format!("squallz-link-metadata-type-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let target = root.join("outside.txt");
        let path = root.join("staged-link");
        fs::write(&target, b"outside contents").unwrap();
        fs::hard_link(&target, &path).unwrap();
        let before = fs::metadata(&target).unwrap().modified().unwrap();
        let destination = root.join("output-link");
        let error = restore_symlink(
            &path,
            &destination,
            Some(UNIX_EPOCH + Duration::from_secs(100)),
        )
        .unwrap_err();
        assert!(error.is_destination_changed());
        assert_eq!(
            error.destination_changed_path(),
            Some(destination.as_path())
        );
        assert_eq!(fs::metadata(&target).unwrap().modified().unwrap(), before);
        assert_eq!(fs::read(&target).unwrap(), b"outside contents");
        restore_symlink(&root.join("missing"), &destination, None).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(any(target_vendor = "apple", target_os = "linux", target_os = "android"))]
    #[test]
    fn symlink_metadata_stays_bound_when_the_opened_path_is_replaced() {
        let root = std::env::temp_dir().join(format!(
            "squallz-link-metadata-bound-{}",
            std::process::id()
        ));
        fs::create_dir_all(&root).unwrap();
        let target = root.join("outside.txt");
        let path = root.join("staged-link");
        let retained = root.join("retained-link");
        fs::write(&target, b"outside contents").unwrap();
        let before = fs::metadata(&target).unwrap().modified().unwrap();
        std::os::unix::fs::symlink("outside.txt", &path).unwrap();
        let link = open_symlink(&path).unwrap();
        assert!(link.metadata().unwrap().file_type().is_symlink());
        fs::rename(&path, &retained).unwrap();
        fs::hard_link(&target, &path).unwrap();
        let expected = UNIX_EPOCH - Duration::new(1, 123_456_700);
        set_symlink_modified(&link, expected).unwrap();
        assert_eq!(
            fs::symlink_metadata(&retained).unwrap().modified().unwrap(),
            expected
        );
        assert_eq!(fs::metadata(&target).unwrap().modified().unwrap(), before);
        assert_eq!(fs::read(&path).unwrap(), b"outside contents");
        drop(link);
        fs::remove_dir_all(root).unwrap();
    }
}
