use std::fs::{self, File};
use std::io;
use std::path::Path;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PathIdentity {
    filesystem: u64,
    entry: u64,
}

impl PathIdentity {
    pub(crate) fn update_digest(self, hasher: &mut blake3::Hasher) {
        hasher.update(&self.filesystem.to_le_bytes());
        hasher.update(&self.entry.to_le_bytes());
    }

    pub(crate) fn components(self) -> (u64, u64) {
        (self.filesystem, self.entry)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RegularFileState {
    bytes: u64,
    #[serde(with = "optional_system_time")]
    modified: Option<SystemTime>,
    #[cfg(unix)]
    changed: (i64, i64),
    #[cfg(windows)]
    windows_times: (u64, u64),
}

mod optional_system_time {
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use serde::{de, Deserialize, Deserializer, Serialize, Serializer};

    #[derive(Serialize, Deserialize)]
    #[serde(rename_all = "snake_case", tag = "side")]
    enum WireTime {
        Before { seconds: u64, nanoseconds: u32 },
        After { seconds: u64, nanoseconds: u32 },
    }

    pub(super) fn serialize<S>(value: &Option<SystemTime>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let wire = value.map(|time| match time.duration_since(UNIX_EPOCH) {
            Ok(duration) => WireTime::After {
                seconds: duration.as_secs(),
                nanoseconds: duration.subsec_nanos(),
            },
            Err(error) => {
                let duration = error.duration();
                WireTime::Before {
                    seconds: duration.as_secs(),
                    nanoseconds: duration.subsec_nanos(),
                }
            }
        });
        wire.serialize(serializer)
    }

    pub(super) fn deserialize<'de, D>(deserializer: D) -> Result<Option<SystemTime>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = Option::<WireTime>::deserialize(deserializer)?;
        wire.map(|wire| {
            let (before, seconds, nanoseconds) = match wire {
                WireTime::Before {
                    seconds,
                    nanoseconds,
                } => (true, seconds, nanoseconds),
                WireTime::After {
                    seconds,
                    nanoseconds,
                } => (false, seconds, nanoseconds),
            };
            if nanoseconds >= 1_000_000_000 {
                return Err(de::Error::custom(
                    "file timestamp nanoseconds must be below one billion",
                ));
            }
            let duration = Duration::new(seconds, nanoseconds);
            if before {
                UNIX_EPOCH.checked_sub(duration)
            } else {
                UNIX_EPOCH.checked_add(duration)
            }
            .ok_or_else(|| de::Error::custom("file timestamp is outside the platform range"))
        })
        .transpose()
    }
}

impl RegularFileState {
    pub(crate) fn from_metadata(metadata: &fs::Metadata) -> Self {
        Self {
            bytes: metadata.len(),
            modified: metadata.modified().ok(),
            #[cfg(unix)]
            changed: unix_change_time(metadata),
            #[cfg(windows)]
            windows_times: windows_file_times(metadata),
        }
    }

    pub(crate) fn bytes(&self) -> u64 {
        self.bytes
    }

    pub(crate) fn modified(&self) -> Option<SystemTime> {
        self.modified
    }

    pub(crate) fn update_digest(&self, hasher: &mut blake3::Hasher) {
        hasher.update(&self.bytes.to_le_bytes());
        update_system_time_digest(hasher, self.modified);
        #[cfg(unix)]
        {
            hasher.update(&self.changed.0.to_le_bytes());
            hasher.update(&self.changed.1.to_le_bytes());
        }
        #[cfg(windows)]
        {
            hasher.update(&self.windows_times.0.to_le_bytes());
            hasher.update(&self.windows_times.1.to_le_bytes());
        }
    }

    pub(crate) fn matches(&self, metadata: &fs::Metadata) -> bool {
        metadata.is_file()
            && metadata.len() == self.bytes
            && metadata.modified().ok() == self.modified
            && {
                #[cfg(unix)]
                {
                    unix_change_time(metadata) == self.changed
                }
                #[cfg(not(unix))]
                {
                    #[cfg(windows)]
                    {
                        windows_file_times(metadata) == self.windows_times
                    }
                    #[cfg(not(windows))]
                    {
                        true
                    }
                }
            }
    }

    pub(crate) fn equivalent_after_rename(&self, other: &Self) -> bool {
        self.bytes == other.bytes && self.modified == other.modified && {
            #[cfg(windows)]
            {
                self.windows_times == other.windows_times
            }
            #[cfg(not(windows))]
            {
                true
            }
        }
    }
}

fn update_system_time_digest(hasher: &mut blake3::Hasher, value: Option<SystemTime>) {
    let Some(value) = value else {
        hasher.update(&[0]);
        return;
    };
    match value.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(duration) => {
            hasher.update(&[1]);
            hasher.update(&duration.as_secs().to_le_bytes());
            hasher.update(&duration.subsec_nanos().to_le_bytes());
        }
        Err(error) => {
            let duration = error.duration();
            hasher.update(&[2]);
            hasher.update(&duration.as_secs().to_le_bytes());
            hasher.update(&duration.subsec_nanos().to_le_bytes());
        }
    }
}

#[cfg(unix)]
fn unix_change_time(metadata: &fs::Metadata) -> (i64, i64) {
    use std::os::unix::fs::MetadataExt;

    (metadata.ctime(), metadata.ctime_nsec())
}

#[cfg(windows)]
fn windows_file_times(metadata: &fs::Metadata) -> (u64, u64) {
    use std::os::windows::fs::MetadataExt;

    (metadata.creation_time(), metadata.last_write_time())
}

#[cfg(unix)]
pub(crate) fn path_identity(path: &Path) -> io::Result<PathIdentity> {
    use std::os::unix::fs::MetadataExt;

    let metadata = fs::symlink_metadata(path)?;
    Ok(PathIdentity {
        filesystem: metadata.dev(),
        entry: metadata.ino(),
    })
}

#[cfg(windows)]
pub(crate) fn path_identity(path: &Path) -> io::Result<PathIdentity> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE,
        FILE_SHARE_READ, FILE_SHARE_WRITE,
    };

    let file = fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    file_identity(&file)
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn path_identity(_path: &Path) -> io::Result<PathIdentity> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "stable file identity is unavailable on this platform",
    ))
}

#[cfg(unix)]
pub(crate) fn file_identity(file: &File) -> io::Result<PathIdentity> {
    use std::os::unix::fs::MetadataExt;

    let metadata = file.metadata()?;
    Ok(PathIdentity {
        filesystem: metadata.dev(),
        entry: metadata.ino(),
    })
}

#[cfg(windows)]
pub(crate) fn file_identity(file: &File) -> io::Result<PathIdentity> {
    let information = winapi_util::file::information(file)?;
    let identity = PathIdentity {
        filesystem: information.volume_serial_number(),
        entry: information.file_index(),
    };
    if identity.filesystem == 0 && identity.entry == 0 {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "the filesystem did not provide a stable file identity",
        ));
    }
    Ok(identity)
}

#[cfg(windows)]
pub(crate) fn path_change_time(path: &Path) -> io::Result<i64> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES,
        FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };

    let file = fs::OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    file_change_time(&file)
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn file_change_time(file: &File) -> io::Result<i64> {
    use std::mem::size_of;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::Storage::FileSystem::{
        FileBasicInfo, GetFileInformationByHandleEx, FILE_BASIC_INFO,
    };

    let mut information = FILE_BASIC_INFO::default();
    // The output points to a live FILE_BASIC_INFO for the duration of the call.
    let succeeded = unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle() as HANDLE,
            FileBasicInfo,
            (&raw mut information).cast(),
            size_of::<FILE_BASIC_INFO>() as u32,
        )
    };
    if succeeded == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(information.ChangeTime)
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn file_identity(_file: &File) -> io::Result<PathIdentity> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "stable file identity is unavailable on this platform",
    ))
}

#[cfg(unix)]
pub(crate) fn open_regular_file_no_follow(path: &Path) -> io::Result<File> {
    use rustix::fs::{open, Mode, OFlags};

    let file = open(
        path,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
        Mode::empty(),
    )?;
    Ok(File::from(file))
}

#[cfg(unix)]
pub(crate) fn open_regular_file_no_follow_read_write(path: &Path) -> io::Result<File> {
    use rustix::fs::{open, Mode, OFlags};

    let file = open(
        path,
        OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
        Mode::empty(),
    )?;
    Ok(File::from(file))
}

#[cfg(windows)]
pub(crate) fn open_regular_file_no_follow(path: &Path) -> io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE, FILE_SHARE_READ,
    };

    fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
}

#[cfg(windows)]
pub(crate) fn open_regular_file_no_follow_read_write(path: &Path) -> io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE, FILE_SHARE_READ,
    };

    fs::OpenOptions::new()
        .read(true)
        .write(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
}

#[cfg(windows)]
pub(crate) fn open_regular_file_no_follow_for_cleanup(path: &Path) -> io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ,
        FILE_WRITE_ATTRIBUTES,
    };

    fs::OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES | FILE_WRITE_ATTRIBUTES)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
}

#[cfg(not(windows))]
pub(crate) fn open_regular_file_no_follow_for_cleanup(path: &Path) -> io::Result<File> {
    open_regular_file_no_follow(path)
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn open_regular_file_no_follow(path: &Path) -> io::Result<File> {
    File::open(path)
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn open_regular_file_no_follow_read_write(path: &Path) -> io::Result<File> {
    fs::OpenOptions::new().read(true).write(true).open(path)
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, UNIX_EPOCH};

    use super::*;

    #[test]
    fn regular_file_state_round_trips_a_pre_epoch_modified_time() {
        let state = RegularFileState {
            bytes: 42,
            modified: Some(UNIX_EPOCH - Duration::from_secs(315_619_200)),
            #[cfg(unix)]
            changed: (7, 11),
            #[cfg(windows)]
            windows_times: (13, 17),
        };

        let encoded = serde_json::to_vec(&state).unwrap();
        let decoded: RegularFileState = serde_json::from_slice(&encoded).unwrap();

        assert_eq!(decoded, state);
    }
}
