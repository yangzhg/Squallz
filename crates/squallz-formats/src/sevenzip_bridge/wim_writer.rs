//! WIM write bridge through wimlib-imagex.

use std::fs;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

use squallz_format_api::{
    ArchiveWriter, CompressionLevel, CreateOptions, EntryMeta, EntryPath, EntryType, FormatError,
    WriteSeek,
};

pub(super) fn create(
    dst: Box<dyn WriteSeek>,
    opts: &CreateOptions,
) -> Result<Box<dyn ArchiveWriter>, FormatError> {
    if opts.password.is_some() || opts.encrypt_filenames {
        return Err(FormatError::Unsupported(
            "WIM creation does not support encryption".into(),
        ));
    }
    Ok(Box::new(WimArchiveWriter {
        dst,
        staging: TempWorkspace::new("wim-stage")?,
        output: TempPath::new("wim")?,
        compress: wim_compress_arg(opts.level),
    }))
}

struct WimArchiveWriter {
    dst: Box<dyn WriteSeek>,
    staging: TempWorkspace,
    output: TempPath,
    compress: &'static str,
}

impl ArchiveWriter for WimArchiveWriter {
    fn add_entry(
        &mut self,
        meta: &EntryMeta,
        data: Option<&mut dyn Read>,
    ) -> Result<(), FormatError> {
        let path = safe_stage_path(self.staging.path(), &meta.path)?;
        match &meta.entry_type {
            EntryType::Dir => fs::create_dir_all(&path)?,
            EntryType::File => {
                let data = data.ok_or_else(|| {
                    FormatError::Other(format!("file entry without data: {}", meta.path))
                })?;
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent)?;
                }
                let mut file = fs::File::create(&path)?;
                io::copy(data, &mut file)?;
            }
            EntryType::Symlink { .. } | EntryType::Hardlink { .. } | EntryType::Other => {
                return Err(FormatError::Unsupported(format!(
                    "WIM writer cannot store entry type of '{}'",
                    meta.path
                )));
            }
        }
        Ok(())
    }

    fn finish(mut self: Box<Self>) -> Result<(), FormatError> {
        run_wimlib_capture(self.staging.path(), self.output.path(), self.compress)?;
        let mut image = fs::File::open(self.output.path())?;
        self.dst.seek(SeekFrom::Start(0))?;
        io::copy(&mut image, &mut self.dst)?;
        self.dst.flush()?;
        Ok(())
    }
}

fn wim_compress_arg(level: CompressionLevel) -> &'static str {
    match level {
        CompressionLevel::Store => "--compress=none",
        CompressionLevel::Fastest | CompressionLevel::Fast => "--compress=XPRESS",
        CompressionLevel::Normal | CompressionLevel::Maximum => "--compress=LZX",
        CompressionLevel::Ultra => "--compress=LZMS",
    }
}

fn run_wimlib_capture(
    source: &Path,
    output: &Path,
    compress: &'static str,
) -> Result<(), FormatError> {
    let tool = wimlib_tool();
    let mut command = Command::new(&tool);
    command
        .arg("capture")
        .arg(source)
        .arg(output)
        .arg("Squallz")
        .arg(compress)
        .arg("--no-acls")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let proc = command.output().map_err(map_wimlib_spawn_error)?;
    if !proc.status.success() {
        return Err(map_wimlib_failure(&proc.stderr));
    }
    let len = fs::metadata(output)?.len();
    if len == 0 {
        return Err(FormatError::CorruptArchive(
            "wimlib-imagex created an empty WIM image".into(),
        ));
    }
    Ok(())
}

fn wimlib_tool() -> PathBuf {
    if let Some(path) = std::env::var_os("SQUALLZ_WIMLIB") {
        return PathBuf::from(path);
    }
    default_wimlib_tool()
}

fn default_wimlib_tool() -> PathBuf {
    match find_on_path("wimlib-imagex") {
        Some(path) => path,
        None => PathBuf::from("wimlib-imagex"),
    }
}

fn find_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
        #[cfg(windows)]
        {
            let candidate = dir.join(format!("{name}.exe"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

fn map_wimlib_spawn_error(e: io::Error) -> FormatError {
    if e.kind() == io::ErrorKind::NotFound {
        FormatError::DependencyMissing("wimlib-imagex WIM writer".into())
    } else {
        FormatError::Io(e)
    }
}

fn map_wimlib_failure(stderr: &[u8]) -> FormatError {
    let detail = String::from_utf8_lossy(stderr).trim().to_owned();
    let lower = detail.to_lowercase();
    if lower.contains("not found") || lower.contains("no such file") {
        FormatError::DependencyMissing("wimlib-imagex WIM writer".into())
    } else {
        FormatError::Other(if detail.is_empty() {
            "wimlib-imagex failed to create WIM image".into()
        } else {
            detail
        })
    }
}

fn safe_stage_path(root: &Path, path: &EntryPath) -> Result<PathBuf, FormatError> {
    let mut out = root.to_path_buf();
    let mut saw_component = false;
    for component in Path::new(&path.display).components() {
        match component {
            Component::Normal(part) => {
                out.push(part);
                saw_component = true;
            }
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(FormatError::PathTraversal(path.display.clone()))
            }
        }
    }
    if !saw_component {
        return Err(FormatError::UnsafeFileName(path.display.clone()));
    }
    Ok(out)
}

struct TempWorkspace {
    path: PathBuf,
}

impl TempWorkspace {
    fn new(tag: &str) -> Result<Self, FormatError> {
        let path = unique_temp_path(tag);
        fs::create_dir(&path)?;
        Ok(Self { path })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempWorkspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

struct TempPath {
    path: PathBuf,
}

impl TempPath {
    fn new(ext: &str) -> Result<Self, FormatError> {
        let path = unique_temp_path("wim-out").with_extension(ext);
        Ok(Self { path })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempPath {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn unique_temp_path(tag: &str) -> PathBuf {
    static NEXT_ID: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!(
        "squallz-{tag}-{}-{}-{}",
        std::process::id(),
        NEXT_ID.fetch_add(1, Ordering::Relaxed),
        system_time_nanos(SystemTime::now())
    ))
}

fn system_time_nanos(time: SystemTime) -> u128 {
    match time.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(duration) => duration.as_nanos(),
        Err(_) => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    struct EnvRestore {
        key: &'static str,
        old: Option<std::ffi::OsString>,
    }

    impl EnvRestore {
        fn new(key: &'static str) -> Self {
            Self {
                key,
                old: std::env::var_os(key),
            }
        }
    }

    impl Drop for EnvRestore {
        fn drop(&mut self) {
            match &self.old {
                Some(value) => std::env::set_var(self.key, value),
                None => std::env::remove_var(self.key),
            }
        }
    }

    #[test]
    fn wim_compress_args_match_creation_levels() {
        assert_eq!(wim_compress_arg(CompressionLevel::Store), "--compress=none");
        assert_eq!(
            wim_compress_arg(CompressionLevel::Fastest),
            "--compress=XPRESS"
        );
        assert_eq!(
            wim_compress_arg(CompressionLevel::Fast),
            "--compress=XPRESS"
        );
        assert_eq!(wim_compress_arg(CompressionLevel::Normal), "--compress=LZX");
        assert_eq!(
            wim_compress_arg(CompressionLevel::Maximum),
            "--compress=LZX"
        );
        assert_eq!(wim_compress_arg(CompressionLevel::Ultra), "--compress=LZMS");
    }

    #[test]
    fn wimlib_tool_prefers_env_then_command_name_fallback() {
        let _guard = env_lock();
        let _restore_wimlib = EnvRestore::new("SQUALLZ_WIMLIB");
        let _restore_path = EnvRestore::new("PATH");

        std::env::set_var("SQUALLZ_WIMLIB", "/tmp/custom-wimlib-imagex");
        assert_eq!(wimlib_tool(), PathBuf::from("/tmp/custom-wimlib-imagex"));

        std::env::remove_var("SQUALLZ_WIMLIB");
        std::env::set_var("PATH", "");
        assert_eq!(wimlib_tool(), PathBuf::from("wimlib-imagex"));
    }

    #[test]
    fn safe_stage_path_rejects_empty_absolute_and_parent_paths() {
        let root = Path::new("/tmp/squallz-wim-stage");

        assert!(safe_stage_path(root, &EntryPath::from_utf8("dir/file.txt"))
            .is_ok_and(|path| path.ends_with("dir/file.txt")));
        assert!(matches!(
            safe_stage_path(root, &EntryPath::from_utf8("")),
            Err(FormatError::UnsafeFileName(_))
        ));
        assert!(matches!(
            safe_stage_path(root, &EntryPath::from_utf8("../escape.txt")),
            Err(FormatError::PathTraversal(_))
        ));
        assert!(matches!(
            safe_stage_path(root, &EntryPath::from_utf8("/absolute.txt")),
            Err(FormatError::PathTraversal(_))
        ));
    }

    #[test]
    fn wimlib_failure_mapping_keeps_dependency_and_default_errors_actionable() {
        assert!(matches!(
            map_wimlib_failure(b"wimlib-imagex: not found"),
            FormatError::DependencyMissing(message) if message.contains("wimlib-imagex")
        ));

        let err = map_wimlib_failure(b"");
        assert!(
            matches!(err, FormatError::Other(ref message) if message.contains("failed to create WIM")),
            "expected default WIM failure, got {err:?}"
        );
    }

    #[test]
    fn system_time_before_epoch_uses_zero_timestamp_fallback() {
        let before_epoch = SystemTime::UNIX_EPOCH - std::time::Duration::from_nanos(1);
        assert_eq!(system_time_nanos(before_epoch), 0);
        assert_eq!(system_time_nanos(SystemTime::UNIX_EPOCH), 0);
    }
}
