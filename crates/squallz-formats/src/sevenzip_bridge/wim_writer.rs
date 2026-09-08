//! WIM write bridge through wimlib-imagex.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{ChildStderr, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::SystemTime;

use squallz_format_api::{
    ArchiveWriter, CompressionLevel, ControlToken, CreateOptions, EntryMeta, EntryPath, EntryType,
    FormatCreateBudget, FormatError, NativeVolumeBudget, NativeVolumeLimits, NativeVolumeWriter,
    ProgressSink, ReadSeek, SplitOutputMode, WriteSeek,
};

use crate::external_process::ControlledChild;
use crate::stable_source::{self, SourceIdentity};

use super::{executable_in_dir, find_on_path, resolve_command_path, wim_volume};

const COPY_CHUNK: usize = 64 * 1024;
const MIB: u64 = 1024 * 1024;
const STDERR_LIMIT: usize = 64 * 1024;
const WIMLIB_ENV: &str = "SQUALLZ_WIMLIB";
const WIMLIB_TOOL: &str = "wimlib-imagex";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WimlibBackendSource {
    Application,
    Environment,
    Path,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WimlibBackendStatus {
    source: Option<WimlibBackendSource>,
    selected: Option<PathBuf>,
    executable: Option<PathBuf>,
    configured: bool,
}

impl WimlibBackendStatus {
    pub fn available(&self) -> bool {
        self.executable.is_some()
    }

    pub fn configured(&self) -> bool {
        self.configured
    }

    pub fn source(&self) -> Option<WimlibBackendSource> {
        self.source
    }

    pub fn selected(&self) -> Option<&Path> {
        self.selected.as_deref()
    }

    pub fn executable(&self) -> Option<&Path> {
        self.executable.as_deref()
    }
}

pub(super) fn create_budget(
    content_bytes: u64,
    archive_bytes: u64,
    opts: &CreateOptions,
) -> Result<FormatCreateBudget, FormatError> {
    validate_create_options(opts)?;
    // The staged tree and encoded image coexist until `finish` returns.
    // The shared archive bound already includes entry/path metadata and
    // encoder expansion, so reserve that complete bound for each copy.
    let archive_bytes = archive_bytes.max(content_bytes);
    let capture_temp_bytes = archive_bytes.checked_mul(2).ok_or_else(|| {
        FormatError::ResourceLimitExceeded("WIM temporary workspace overflow".into())
    })?;
    let system_temp_bytes = match (opts.split_size, opts.split_mode) {
        (Some(volume_size), SplitOutputMode::Native) => {
            let native = native_volume_budget(archive_bytes, 0, volume_size)?;
            capture_temp_bytes.max(archive_bytes.checked_add(native.output_bytes).ok_or_else(
                || {
                    FormatError::ResourceLimitExceeded(
                        "Split WIM temporary workspace overflow".into(),
                    )
                },
            )?)
        }
        _ => capture_temp_bytes,
    };
    Ok(FormatCreateBudget {
        output_bytes: archive_bytes,
        system_temp_bytes,
    })
}

pub(super) fn create(
    dst: Box<dyn WriteSeek>,
    opts: &CreateOptions,
) -> Result<Box<dyn ArchiveWriter>, FormatError> {
    create_with_control(dst, opts, &ControlToken::default())
}

pub(super) fn create_with_control(
    dst: Box<dyn WriteSeek>,
    opts: &CreateOptions,
    ctl: &ControlToken,
) -> Result<Box<dyn ArchiveWriter>, FormatError> {
    validate_create_options(opts)?;
    ctl.checkpoint()?;
    Ok(Box::new(WimArchiveWriter {
        dst,
        staging: TempWorkspace::new("wim-stage")?,
        output: TempPath::new("wim")?,
        compress: wim_compress_arg(opts.level),
        threads: opts.resources.threads.map(|threads| threads.max(1)),
        control: ctl.clone(),
    }))
}

pub(super) fn native_volume_limits() -> NativeVolumeLimits {
    NativeVolumeLimits {
        min_volume_size: 64 * 1024,
        max_volume_size: u64::MAX,
        max_volumes: u16::MAX.into(),
    }
}

pub(super) fn native_volume_primary_index(volume_count: u32) -> Result<u32, FormatError> {
    if volume_count == 0 {
        return Err(FormatError::Other(
            "Split WIM writer produced no output".into(),
        ));
    }
    Ok(0)
}

pub(super) fn native_volume_budget(
    archive_bytes: u64,
    _entry_count: u64,
    volume_size: u64,
) -> Result<NativeVolumeBudget, FormatError> {
    if volume_size == 0 {
        return Err(FormatError::Unsupported(
            "Split WIM part size must be greater than zero".into(),
        ));
    }
    let payload_volume_count = archive_bytes.div_ceil(volume_size).max(1);
    if payload_volume_count > u64::from(u16::MAX) {
        return Err(FormatError::ResourceLimitExceeded(
            "Split WIM would create more than 65,535 parts".into(),
        ));
    }
    // wimlib can repeat lookup, XML, integrity, and part metadata. Keep a
    // conservative bound instead of treating requested part size as a hard
    // physical limit; one indivisible resource can exceed that target.
    let output_bytes = archive_bytes
        .checked_mul(2)
        .and_then(|bytes| bytes.checked_add(payload_volume_count.saturating_mul(MIB)))
        .ok_or_else(|| {
            FormatError::ResourceLimitExceeded("Split WIM output budget overflow".into())
        })?;
    let volume_count = output_bytes.div_ceil(volume_size).max(1);
    if volume_count > u64::from(u16::MAX) {
        return Err(FormatError::ResourceLimitExceeded(
            "Split WIM budget would exceed 65,535 parts".into(),
        ));
    }
    Ok(NativeVolumeBudget {
        output_bytes,
        volume_count,
    })
}

pub(super) fn native_volume_path(
    destination: &Path,
    disk_index: u32,
) -> Result<PathBuf, FormatError> {
    if disk_index >= u32::from(u16::MAX) {
        return Err(FormatError::ResourceLimitExceeded(
            "Split WIM currently supports at most 65,535 parts".into(),
        ));
    }
    let extension = destination
        .extension()
        .and_then(|extension| extension.to_str())
        .filter(|extension| extension.eq_ignore_ascii_case("swm"))
        .ok_or_else(|| {
            FormatError::Unsupported("native Split WIM output must use a .swm name".into())
        })?;
    if disk_index == 0 {
        return Ok(destination.to_path_buf());
    }
    let stem = destination.file_stem().ok_or_else(|| {
        FormatError::Unsupported("native Split WIM output has no file name".into())
    })?;
    let mut name = OsString::from(stem);
    name.push((disk_index + 1).to_string());
    name.push(".");
    name.push(extension);
    Ok(destination.with_file_name(name))
}

pub(super) fn write_native_volumes(
    source: &mut dyn ReadSeek,
    output: &mut dyn NativeVolumeWriter,
    progress: &dyn ProgressSink,
    ctl: &ControlToken,
) -> Result<(), FormatError> {
    let current = EntryPath::from_utf8(String::new());
    progress.on_progress(0, 0, &current);
    ctl.checkpoint()?;
    let copy_buffer_size = output.stream_buffer_size(COPY_CHUNK)?;

    let workspace = stable_source::create_private_staging_dir("wim-native-split")?;
    let source_path = workspace.join("source.wim");
    let mut source_copy = stable_source::create_private_file(&source_path)?;
    source.seek(SeekFrom::Start(0))?;
    let mut buffer = vec![0u8; copy_buffer_size];
    loop {
        ctl.checkpoint()?;
        let read = source.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        source_copy.write_all(&buffer[..read])?;
    }
    source_copy.flush()?;
    source_copy.sync_all()?;

    let first_part = workspace.join("archive.swm");
    run_wimlib_split(&source_path, &first_part, output.volume_size(), ctl)?;
    stable_source::harden_private_staging_members(&workspace)?;
    let parts = wim_volume::validate_generated_set(&first_part)?;
    let total = parts.iter().try_fold(0u64, |total, part| {
        total.checked_add(part.len).ok_or_else(|| {
            FormatError::ResourceLimitExceeded("Split WIM output length overflow".into())
        })
    })?;
    progress.on_progress(0, total, &current);

    let mut written = 0u64;
    for part in &parts {
        ctl.checkpoint()?;
        output.begin_volume()?;
        let mut file =
            stable_source::open_regular_file_no_follow(&part.path, "generated WIM volume")?;
        if SourceIdentity::from_file(&file)? != part.identity {
            return Err(FormatError::Io(io::Error::other(
                "generated WIM volume changed before it was copied",
            )));
        }
        let mut remaining = part.len;
        while remaining > 0 {
            ctl.checkpoint()?;
            let want = buffer.len().min(remaining as usize);
            let read = file.read(&mut buffer[..want])?;
            if read == 0 {
                return Err(FormatError::Io(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "generated WIM volume shrank while it was copied",
                )));
            }
            output.write_current_volume(&buffer[..read])?;
            remaining -= read as u64;
            written = written.checked_add(read as u64).ok_or_else(|| {
                FormatError::ResourceLimitExceeded("Split WIM output progress overflow".into())
            })?;
            progress.on_progress(written, total, &current);
        }
        stable_source::verify_source_binding(&part.path, &part.identity, "generated WIM volume")?;
    }
    ctl.checkpoint()?;
    if written != total {
        return Err(FormatError::Other(
            "Split WIM output progress did not match the bytes written".into(),
        ));
    }
    Ok(())
}

fn validate_create_options(opts: &CreateOptions) -> Result<(), FormatError> {
    if opts.password.is_some() || opts.encrypt_filenames {
        return Err(FormatError::Unsupported(
            "WIM creation does not support encryption".into(),
        ));
    }
    Ok(())
}

struct WimArchiveWriter {
    dst: Box<dyn WriteSeek>,
    staging: TempWorkspace,
    output: TempPath,
    compress: &'static str,
    threads: Option<usize>,
    control: ControlToken,
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
        run_wimlib_capture(
            self.staging.path(),
            self.output.path(),
            self.compress,
            self.threads,
            &self.control,
        )?;
        let mut image = fs::File::open(self.output.path())?;
        self.dst.seek(SeekFrom::Start(0))?;
        let mut buffer = vec![0u8; COPY_CHUNK];
        loop {
            self.control.checkpoint()?;
            let read = image.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            self.dst.write_all(&buffer[..read])?;
        }
        self.control.checkpoint()?;
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
    threads: Option<usize>,
    ctl: &ControlToken,
) -> Result<(), FormatError> {
    ctl.checkpoint()?;
    let tool = wimlib_tool()?;
    let mut command = wimlib_capture_command(&tool, source, output, compress, threads);
    let child = command.spawn().map_err(map_wimlib_spawn_error)?;
    let (status, stderr) = wait_wimlib_child(child, ctl)?;
    if !status.success() {
        return Err(redact_wimlib_paths(
            map_wimlib_failure(&stderr),
            &[source, output],
        ));
    }
    let len = fs::metadata(output)?.len();
    if len == 0 {
        return Err(FormatError::CorruptArchive(
            "wimlib-imagex created an empty WIM image".into(),
        ));
    }
    Ok(())
}

fn wimlib_capture_command(
    tool: &Path,
    source: &Path,
    output: &Path,
    compress: &'static str,
    threads: Option<usize>,
) -> Command {
    let mut command = Command::new(tool);
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
    if let Some(threads) = threads {
        command.arg(format!("--threads={}", threads.max(1)));
    }
    command
}

fn run_wimlib_split(
    source: &Path,
    first_part: &Path,
    volume_size: u64,
    ctl: &ControlToken,
) -> Result<(), FormatError> {
    ctl.checkpoint()?;
    let tool = wimlib_tool()?;
    let child = Command::new(&tool)
        .arg("split")
        .arg(source)
        .arg(first_part)
        .arg(split_size_arg(volume_size))
        .arg("--check")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(map_wimlib_spawn_error)?;
    let (status, stderr) = wait_wimlib_child(child, ctl)?;
    if !status.success() {
        return Err(redact_wimlib_paths(
            map_wimlib_failure(&stderr),
            &[source, first_part],
        ));
    }
    ctl.checkpoint()
}

fn wait_wimlib_child(
    mut child: std::process::Child,
    ctl: &ControlToken,
) -> Result<(std::process::ExitStatus, Vec<u8>), FormatError> {
    let stderr = match child.stderr.take() {
        Some(stderr) => stderr,
        None => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(FormatError::Other(
                "wimlib-imagex did not provide an error stream".into(),
            ));
        }
    };
    let stderr_reader = read_bounded_stderr(stderr);
    let mut child = ControlledChild::new(child, ctl);
    let status = match child.wait() {
        Ok(status) => status,
        Err(error) => {
            child.terminate();
            let _ = join_stderr(stderr_reader);
            if ctl.is_cancelled() {
                return Err(FormatError::Cancelled);
            }
            return Err(error.into());
        }
    };
    let stderr = match join_stderr(stderr_reader) {
        Ok(stderr) => stderr,
        Err(_) if ctl.is_cancelled() => return Err(FormatError::Cancelled),
        Err(error) => return Err(error),
    };
    ctl.checkpoint()?;
    Ok((status, stderr))
}

fn read_bounded_stderr(mut stderr: ChildStderr) -> thread::JoinHandle<Result<Vec<u8>, io::Error>> {
    thread::spawn(move || {
        let mut bytes = Vec::new();
        let mut buffer = [0u8; 4096];
        loop {
            let read = stderr.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            let remaining = STDERR_LIMIT.saturating_sub(bytes.len());
            bytes.extend_from_slice(&buffer[..read.min(remaining)]);
        }
        Ok(bytes)
    })
}

fn join_stderr(
    reader: thread::JoinHandle<Result<Vec<u8>, io::Error>>,
) -> Result<Vec<u8>, FormatError> {
    match reader.join() {
        Ok(result) => result.map_err(FormatError::from),
        Err(_) => Err(FormatError::Other(
            "wimlib-imagex error reader stopped unexpectedly".into(),
        )),
    }
}

fn split_size_arg(bytes: u64) -> String {
    let whole = bytes / MIB;
    let remainder = bytes % MIB;
    if remainder == 0 {
        return whole.to_string();
    }
    // 10^20 / 2^20 = 5^20, so this renders the byte value as an exact,
    // terminating number of mebibytes without floating-point rounding.
    const FIVE_TO_TWENTY: u128 = 95_367_431_640_625;
    let fraction = u128::from(remainder) * FIVE_TO_TWENTY;
    let digits = format!("{fraction:020}");
    format!("{whole}.{}", digits.trim_end_matches('0'))
}

fn redact_wimlib_paths(error: FormatError, paths: &[&Path]) -> FormatError {
    let redact = |mut detail: String| {
        for path in paths {
            detail = detail.replace(path.to_string_lossy().as_ref(), "[private WIM staging]");
        }
        detail
    };
    match error {
        FormatError::Other(detail) => FormatError::Other(redact(detail)),
        FormatError::DependencyMissing(detail) => FormatError::DependencyMissing(redact(detail)),
        other => other,
    }
}

pub fn wimlib_backend_status() -> WimlibBackendStatus {
    let configured = std::env::var_os(WIMLIB_ENV);
    let application_dir = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf));
    let search_path = std::env::var_os("PATH");
    let fallback_directories = wimlib_fallback_directories();
    detect_wimlib_backend(
        configured.as_deref(),
        application_dir.as_deref(),
        search_path.as_deref(),
        &fallback_directories,
    )
}

#[cfg(target_os = "macos")]
fn wimlib_fallback_directories() -> Vec<PathBuf> {
    ["/opt/homebrew/bin", "/usr/local/bin"]
        .into_iter()
        .map(PathBuf::from)
        .collect()
}

#[cfg(not(target_os = "macos"))]
fn wimlib_fallback_directories() -> Vec<PathBuf> {
    Vec::new()
}

fn detect_wimlib_backend(
    configured: Option<&OsStr>,
    application_dir: Option<&Path>,
    search_path: Option<&OsStr>,
    fallback_directories: &[PathBuf],
) -> WimlibBackendStatus {
    if let Some(configured) = configured {
        let selected = PathBuf::from(configured);
        let executable = resolve_command_path(&selected, search_path);
        return WimlibBackendStatus {
            source: Some(WimlibBackendSource::Environment),
            selected: Some(selected),
            executable,
            configured: true,
        };
    }

    if let Some(application_dir) = application_dir {
        if let Some(executable) = executable_in_dir(application_dir, OsStr::new(WIMLIB_TOOL)) {
            return WimlibBackendStatus {
                source: Some(WimlibBackendSource::Application),
                selected: Some(executable.clone()),
                executable: Some(executable),
                configured: false,
            };
        }
    }

    if let Some(executable) = find_on_path(OsStr::new(WIMLIB_TOOL), search_path) {
        return WimlibBackendStatus {
            source: Some(WimlibBackendSource::Path),
            selected: Some(executable.clone()),
            executable: Some(executable),
            configured: false,
        };
    }

    for directory in fallback_directories {
        if let Some(executable) = executable_in_dir(directory, OsStr::new(WIMLIB_TOOL)) {
            return WimlibBackendStatus {
                source: Some(WimlibBackendSource::Path),
                selected: Some(executable.clone()),
                executable: Some(executable),
                configured: false,
            };
        }
    }

    WimlibBackendStatus {
        source: None,
        selected: None,
        executable: None,
        configured: false,
    }
}

fn wimlib_tool() -> Result<PathBuf, FormatError> {
    let status = wimlib_backend_status();
    wimlib_backend_executable(&status)
}

fn wimlib_backend_executable(status: &WimlibBackendStatus) -> Result<PathBuf, FormatError> {
    status
        .executable()
        .map(Path::to_path_buf)
        .ok_or_else(|| FormatError::DependencyMissing("wimlib-imagex WIM writer".into()))
}

fn map_wimlib_spawn_error(e: io::Error) -> FormatError {
    if e.kind() == io::ErrorKind::NotFound {
        FormatError::DependencyMissing("wimlib-imagex WIM writer".into())
    } else {
        FormatError::from(e)
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

    fn write_test_executable(dir: &Path) -> PathBuf {
        fs::create_dir_all(dir).unwrap();
        let path = dir.join(if cfg!(windows) {
            format!("{WIMLIB_TOOL}.exe")
        } else {
            WIMLIB_TOOL.to_owned()
        });
        fs::write(&path, b"test executable").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let mut permissions = fs::metadata(&path).unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&path, permissions).unwrap();
        }
        path
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
    fn wim_capture_command_honors_configured_threads() {
        let command = wimlib_capture_command(
            Path::new("wimlib-imagex"),
            Path::new("source"),
            Path::new("archive.wim"),
            "--compress=LZX",
            Some(6),
        );
        let args = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();

        assert_eq!(
            args,
            [
                "capture",
                "source",
                "archive.wim",
                "Squallz",
                "--compress=LZX",
                "--no-acls",
                "--threads=6",
            ]
        );
    }

    #[test]
    fn wim_budget_covers_staging_and_temporary_image_headroom() {
        let content_bytes = 8 * 1024 * 1024;
        let archive_bytes = content_bytes + 3 * 1024 * 1024;
        let budget = create_budget(content_bytes, archive_bytes, &CreateOptions::default())
            .unwrap_or_else(|error| panic!("WIM budget failed: {error}"));

        assert_eq!(budget.output_bytes, archive_bytes);
        assert_eq!(budget.system_temp_bytes, archive_bytes * 2);
    }

    #[test]
    fn split_wim_budget_includes_private_split_workspace() {
        let archive_bytes = 8 * MIB;
        let opts = CreateOptions {
            split_size: Some(3 * MIB),
            split_mode: SplitOutputMode::Native,
            ..CreateOptions::default()
        };
        let native = native_volume_budget(archive_bytes, 4, 3 * MIB)
            .unwrap_or_else(|error| panic!("Split WIM budget failed: {error}"));
        let budget = create_budget(archive_bytes, archive_bytes, &opts)
            .unwrap_or_else(|error| panic!("WIM budget failed: {error}"));

        assert_eq!(native.volume_count, 7);
        assert!(native.output_bytes > archive_bytes);
        assert_eq!(
            budget.system_temp_bytes,
            archive_bytes + native.output_bytes
        );
    }

    #[test]
    fn split_wim_paths_use_standard_first_and_numbered_names() {
        let base = Path::new("/tmp/install.swm");

        assert_eq!(native_volume_path(base, 0).unwrap(), base);
        assert_eq!(
            native_volume_path(base, 1).unwrap(),
            PathBuf::from("/tmp/install2.swm")
        );
        assert_eq!(
            native_volume_path(base, 41).unwrap(),
            PathBuf::from("/tmp/install42.swm")
        );
        assert!(matches!(
            native_volume_path(Path::new("/tmp/install.wim"), 0),
            Err(FormatError::Unsupported(_))
        ));
    }

    #[test]
    fn split_wim_size_argument_preserves_exact_byte_targets() {
        assert_eq!(split_size_arg(MIB), "1");
        assert_eq!(split_size_arg(64 * 1024), "0.0625");
        assert_eq!(split_size_arg(MIB + 1), "1.00000095367431640625");
        assert_eq!(split_size_arg(4 * 1024 * MIB), "4096");
    }

    #[test]
    fn wim_budget_and_writer_reject_the_same_encryption_options() {
        let invalid_options = [
            CreateOptions {
                password: Some(squallz_format_api::Password::new("secret")),
                ..CreateOptions::default()
            },
            CreateOptions {
                encrypt_filenames: true,
                ..CreateOptions::default()
            },
        ];

        for opts in invalid_options {
            assert!(matches!(
                create_budget(1024, 2048, &opts),
                Err(FormatError::Unsupported(_))
            ));
            assert!(matches!(
                create(Box::new(io::Cursor::new(Vec::new())), &opts),
                Err(FormatError::Unsupported(_))
            ));
        }
    }

    #[test]
    fn wim_budget_rejects_temporary_workspace_overflow() {
        assert!(matches!(
            create_budget(u64::MAX, u64::MAX, &CreateOptions::default()),
            Err(FormatError::ResourceLimitExceeded(_))
        ));
    }

    #[test]
    fn wimlib_backend_status_distinguishes_configuration_application_and_path() {
        let root = unique_temp_path("wimlib-backend-status");
        let application_dir = root.join("application");
        let path_dir = root.join("path");
        let fallback_dir = root.join("fallback");
        let application_tool = write_test_executable(&application_dir);
        let path_tool = write_test_executable(&path_dir);
        let fallback_tool = write_test_executable(&fallback_dir);
        let fallback_directories = vec![fallback_dir];
        let search_path = std::env::join_paths([path_dir]).unwrap();

        let missing_override = root.join("missing-override");
        let configured = detect_wimlib_backend(
            Some(missing_override.as_os_str()),
            Some(&application_dir),
            Some(search_path.as_os_str()),
            &fallback_directories,
        );
        assert!(!configured.available());
        assert!(configured.configured());
        assert_eq!(configured.source(), Some(WimlibBackendSource::Environment));
        assert_eq!(configured.selected(), Some(missing_override.as_path()));
        assert_eq!(configured.executable(), None);
        assert!(matches!(
            wimlib_backend_executable(&configured),
            Err(FormatError::DependencyMissing(_))
        ));

        let application = detect_wimlib_backend(
            None,
            Some(&application_dir),
            Some(search_path.as_os_str()),
            &fallback_directories,
        );
        assert!(application.available());
        assert!(!application.configured());
        assert_eq!(application.source(), Some(WimlibBackendSource::Application));
        assert_eq!(application.selected(), Some(application_tool.as_path()));
        assert_eq!(application.executable(), Some(application_tool.as_path()));
        assert_eq!(
            wimlib_backend_executable(&application).unwrap(),
            application_tool
        );

        let path = detect_wimlib_backend(
            None,
            None,
            Some(search_path.as_os_str()),
            &fallback_directories,
        );
        assert!(path.available());
        assert!(!path.configured());
        assert_eq!(path.source(), Some(WimlibBackendSource::Path));
        assert_eq!(path.selected(), Some(path_tool.as_path()));
        assert_eq!(path.executable(), Some(path_tool.as_path()));
        assert_eq!(wimlib_backend_executable(&path).unwrap(), path_tool);

        let fallback = detect_wimlib_backend(None, None, None, &fallback_directories);
        assert!(fallback.available());
        assert!(!fallback.configured());
        assert_eq!(fallback.source(), Some(WimlibBackendSource::Path));
        assert_eq!(fallback.selected(), Some(fallback_tool.as_path()));
        assert_eq!(fallback.executable(), Some(fallback_tool.as_path()));
        assert_eq!(wimlib_backend_executable(&fallback).unwrap(), fallback_tool);

        let missing = detect_wimlib_backend(None, None, None, &[]);
        assert!(!missing.available());
        assert!(!missing.configured());
        assert_eq!(missing.source(), None);
        assert_eq!(missing.selected(), None);
        assert_eq!(missing.executable(), None);

        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn wim_capture_cancellation_terminates_the_external_tool() {
        use std::os::unix::fs::PermissionsExt;

        let _guard = env_lock();
        let _restore_wimlib = EnvRestore::new("SQUALLZ_WIMLIB");
        let tool = TempPath::new("sh")
            .unwrap_or_else(|error| panic!("create fake WIM tool path: {error}"));
        fs::write(tool.path(), b"#!/bin/sh\nexec /bin/sleep 30\n")
            .unwrap_or_else(|error| panic!("write fake WIM tool: {error}"));
        let mut permissions = fs::metadata(tool.path())
            .unwrap_or_else(|error| panic!("read fake WIM tool permissions: {error}"))
            .permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(tool.path(), permissions)
            .unwrap_or_else(|error| panic!("make fake WIM tool executable: {error}"));
        std::env::set_var("SQUALLZ_WIMLIB", tool.path());

        let control = ControlToken::default();
        let writer = create_with_control(
            Box::new(io::Cursor::new(Vec::<u8>::new())),
            &CreateOptions::default(),
            &control,
        )
        .unwrap_or_else(|error| panic!("create controlled WIM writer: {error}"));
        let cancellation = control.clone();
        let canceller = thread::spawn(move || {
            thread::sleep(std::time::Duration::from_millis(100));
            cancellation.cancel();
        });
        let started = std::time::Instant::now();
        let error = writer
            .finish()
            .expect_err("cancelling WIM capture must stop the writer");
        canceller
            .join()
            .unwrap_or_else(|_| panic!("WIM cancellation thread panicked"));

        assert!(
            matches!(error, FormatError::Cancelled),
            "expected WIM cancellation, got {error:?}"
        );
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "WIM capture cancellation took {:?}",
            started.elapsed()
        );
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
