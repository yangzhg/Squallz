#[cfg(not(unix))]
use std::fs::OpenOptions;
use std::fs::{self, File};
use std::io;
#[cfg(not(unix))]
use std::path::PathBuf;
use std::path::{Component, Path};

#[cfg(unix)]
use rustix::fs::{mkdirat, openat, symlinkat, Mode, OFlags};

pub(super) struct BundleTree {
    #[cfg(unix)]
    root: File,
    #[cfg(not(unix))]
    root: PathBuf,
}

impl BundleTree {
    pub(super) fn new(root: &File, path: &Path) -> io::Result<Self> {
        #[cfg(unix)]
        {
            let _ = path;
            Ok(Self {
                root: root.try_clone()?,
            })
        }
        #[cfg(not(unix))]
        {
            let _ = root;
            Ok(Self {
                root: path.to_path_buf(),
            })
        }
    }

    #[cfg(unix)]
    fn open_dir(&self, relative: &Path) -> io::Result<File> {
        let mut current = self.root.try_clone()?;
        for component in relative.components() {
            let Component::Normal(name) = component else {
                return Err(invalid_relative_path(relative));
            };
            let opened = openat(
                &current,
                name,
                OFlags::RDONLY
                    | OFlags::DIRECTORY
                    | OFlags::CLOEXEC
                    | OFlags::NOFOLLOW
                    | OFlags::NONBLOCK,
                Mode::empty(),
            )
            .map_err(io::Error::from)?;
            current = File::from(opened);
        }
        Ok(current)
    }

    #[cfg(unix)]
    fn open_parent<'a>(&self, relative: &'a Path) -> io::Result<(File, &'a std::ffi::OsStr)> {
        let name = relative
            .file_name()
            .ok_or_else(|| invalid_relative_path(relative))?;
        let parent = relative.parent().unwrap_or_else(|| Path::new(""));
        Ok((self.open_dir(parent)?, name))
    }

    #[cfg(unix)]
    pub(super) fn create_dir(&self, relative: &Path) -> io::Result<()> {
        let (parent, name) = self.open_parent(relative)?;
        mkdirat(&parent, name, Mode::from_raw_mode(0o700)).map_err(io::Error::from)
    }

    #[cfg(not(unix))]
    pub(super) fn create_dir(&self, relative: &Path) -> io::Result<()> {
        validate_relative_path(relative)?;
        fs::create_dir(self.root.join(relative))
    }

    #[cfg(unix)]
    pub(super) fn ensure_dir(&self, relative: &Path) -> io::Result<()> {
        let mut current = self.root.try_clone()?;
        for component in relative.components() {
            let Component::Normal(name) = component else {
                return Err(invalid_relative_path(relative));
            };
            match mkdirat(&current, name, Mode::from_raw_mode(0o755)) {
                Ok(()) => {}
                Err(error) if error == rustix::io::Errno::EXIST => {}
                Err(error) => return Err(error.into()),
            }
            let opened = openat(
                &current,
                name,
                OFlags::RDONLY
                    | OFlags::DIRECTORY
                    | OFlags::CLOEXEC
                    | OFlags::NOFOLLOW
                    | OFlags::NONBLOCK,
                Mode::empty(),
            )
            .map_err(io::Error::from)?;
            current = File::from(opened);
        }
        Ok(())
    }

    #[cfg(not(unix))]
    pub(super) fn ensure_dir(&self, relative: &Path) -> io::Result<()> {
        validate_relative_path(relative)?;
        fs::create_dir_all(self.root.join(relative))
    }

    #[cfg(unix)]
    pub(super) fn create_file(&self, relative: &Path) -> io::Result<File> {
        let (parent, name) = self.open_parent(relative)?;
        let opened = openat(
            &parent,
            name,
            OFlags::WRONLY
                | OFlags::CREATE
                | OFlags::EXCL
                | OFlags::CLOEXEC
                | OFlags::NOFOLLOW
                | OFlags::NONBLOCK,
            Mode::from_raw_mode(0o644),
        )
        .map_err(io::Error::from)?;
        Ok(File::from(opened))
    }

    #[cfg(not(unix))]
    pub(super) fn create_file(&self, relative: &Path) -> io::Result<File> {
        validate_relative_path(relative)?;
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(self.root.join(relative))
    }

    #[cfg(unix)]
    pub(super) fn rewrite_file(&self, relative: &Path) -> io::Result<File> {
        let (parent, name) = self.open_parent(relative)?;
        let opened = match openat(
            &parent,
            name,
            OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
            Mode::empty(),
        ) {
            Ok(opened) => opened,
            Err(error) if error == rustix::io::Errno::NOENT => {
                return self.create_file(relative);
            }
            Err(error) => return Err(error.into()),
        };
        let file = File::from(opened);
        if !file.metadata()?.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "SFX bundle output node is not a regular file",
            ));
        }
        file.set_len(0)?;
        Ok(file)
    }

    #[cfg(not(unix))]
    pub(super) fn rewrite_file(&self, relative: &Path) -> io::Result<File> {
        validate_relative_path(relative)?;
        let path = self.root.join(relative);
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
                OpenOptions::new().write(true).truncate(true).open(path)
            }
            Ok(_) => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "SFX bundle output node is not a regular file",
            )),
            Err(error) if error.kind() == io::ErrorKind::NotFound => self.create_file(relative),
            Err(error) => Err(error),
        }
    }

    #[cfg(unix)]
    pub(super) fn create_symlink(&self, target: &Path, relative: &Path) -> io::Result<()> {
        let (parent, name) = self.open_parent(relative)?;
        symlinkat(target, &parent, name).map_err(io::Error::from)
    }

    #[cfg(not(unix))]
    pub(super) fn create_symlink(&self, _target: &Path, _relative: &Path) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "macOS app templates with symlinks must be assembled on a Unix host",
        ))
    }

    #[cfg(unix)]
    pub(super) fn set_permissions(
        &self,
        relative: &Path,
        permissions: fs::Permissions,
        directory: bool,
    ) -> io::Result<()> {
        let file = if directory {
            self.open_dir(relative)?
        } else {
            let (parent, name) = self.open_parent(relative)?;
            let opened = openat(
                &parent,
                name,
                OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW | OFlags::NONBLOCK,
                Mode::empty(),
            )
            .map_err(io::Error::from)?;
            let file = File::from(opened);
            if !file.metadata()?.is_file() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "SFX bundle output node is not a regular file",
                ));
            }
            file
        };
        file.set_permissions(permissions)
    }

    #[cfg(not(unix))]
    pub(super) fn set_permissions(
        &self,
        relative: &Path,
        permissions: fs::Permissions,
        _directory: bool,
    ) -> io::Result<()> {
        validate_relative_path_or_root(relative)?;
        fs::set_permissions(self.root.join(relative), permissions)
    }

    #[cfg(unix)]
    pub(super) fn sync_dir(&self, relative: &Path) -> io::Result<()> {
        self.open_dir(relative)?.sync_all()
    }

    #[cfg(windows)]
    pub(super) fn sync_dir(&self, relative: &Path) -> io::Result<()> {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_BACKUP_SEMANTICS, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        };

        validate_relative_path_or_root(relative)?;
        OpenOptions::new()
            .read(true)
            .write(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(self.root.join(relative))?
            .sync_all()
    }

    #[cfg(not(any(unix, windows)))]
    pub(super) fn sync_dir(&self, relative: &Path) -> io::Result<()> {
        validate_relative_path_or_root(relative)?;
        File::open(self.root.join(relative))?.sync_all()
    }
}

fn invalid_relative_path(path: &Path) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        format!("invalid SFX bundle relative path: {}", path.display()),
    )
}

#[cfg(not(unix))]
fn validate_relative_path(path: &Path) -> io::Result<()> {
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(invalid_relative_path(path));
    }
    Ok(())
}

#[cfg(not(unix))]
fn validate_relative_path_or_root(path: &Path) -> io::Result<()> {
    if path.as_os_str().is_empty() {
        return Ok(());
    }
    validate_relative_path(path)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::io::Write;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    fn test_dir(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "sqz-sfx-tree-{label}-{}-{}",
            std::process::id(),
            TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn writes_remain_bound_to_the_open_root_after_path_replacement() {
        let parent = test_dir("root-rebind");
        let root = parent.join("stage");
        let retained = parent.join("retained");
        fs::create_dir_all(&root).unwrap();
        let held = File::open(&root).unwrap();
        let tree = BundleTree::new(&held, &root).unwrap();

        fs::rename(&root, &retained).unwrap();
        fs::create_dir(&root).unwrap();
        fs::write(root.join("competitor"), b"keep").unwrap();

        tree.ensure_dir(Path::new("Contents/Resources")).unwrap();
        let mut output = tree
            .create_file(Path::new("Contents/Resources/payload"))
            .unwrap();
        output.write_all(b"payload").unwrap();
        output.sync_all().unwrap();

        assert_eq!(fs::read(root.join("competitor")).unwrap(), b"keep");
        assert!(!root.join("Contents").exists());
        assert_eq!(
            fs::read(retained.join("Contents/Resources/payload")).unwrap(),
            b"payload"
        );
        fs::remove_dir_all(parent).unwrap();
    }

    #[test]
    fn relative_writes_reject_a_symlinked_parent() {
        let parent = test_dir("parent-symlink");
        let root = parent.join("stage");
        let outside = parent.join("outside");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, root.join("Contents")).unwrap();
        let held = File::open(&root).unwrap();
        let tree = BundleTree::new(&held, &root).unwrap();

        let error = tree.create_file(Path::new("Contents/payload")).unwrap_err();

        assert!(error.raw_os_error().is_some());
        assert!(fs::read_dir(&outside).unwrap().next().is_none());
        fs::remove_dir_all(parent).unwrap();
    }
}
