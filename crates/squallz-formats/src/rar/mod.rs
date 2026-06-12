//! RAR/CBR read bridge.
//!
//! Squallz does not create RAR archives and does not link unrar code into
//! the binary. This bridge prefers the packageable `7zz`/`7z` external reader
//! and keeps `bsdtar`/libarchive as an explicit diagnostic fallback. Both
//! backends are used only for listing and per-entry stdout reads; extraction
//! still flows through the shared safe extraction engine.

use std::fs;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

use squallz_format_api::{
    ArchiveFormat, ArchiveReader, ArchiveWriter, CreateOptions, EntryMeta, EntryPath, EntryType,
    FormatCapabilities, FormatError, OpenOptions, ProgressSink, ReadSeek, TestReport, WriteSeek,
};

use crate::sevenzip_bridge;

const RAR4_MAGIC: &[u8] = b"Rar!\x1A\x07\x00";
const RAR5_MAGIC: &[u8] = b"Rar!\x1A\x07\x01\x00";

pub(crate) struct RarFormat;

impl ArchiveFormat for RarFormat {
    fn id(&self) -> &'static str {
        "rar"
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["rar", "cbr"]
    }

    fn capabilities(&self) -> FormatCapabilities {
        FormatCapabilities {
            can_create: false,
            can_extract: true,
            can_encrypt_data: false,
            can_encrypt_names: false,
            can_split: false,
            can_update: false,
            can_test: true,
        }
    }

    fn sniff(&self, head: &[u8], _tail: &[u8]) -> bool {
        head.starts_with(RAR4_MAGIC) || head.starts_with(RAR5_MAGIC)
    }

    fn open(
        &self,
        src: Box<dyn ReadSeek>,
        opts: &OpenOptions,
    ) -> Result<Box<dyn ArchiveReader>, FormatError> {
        if opts.password.is_some() {
            return Err(FormatError::Unsupported(
                "RAR password extraction through the external bridge is not supported yet".into(),
            ));
        }
        Ok(Box::new(RarArchiveReader::open(src)?))
    }

    fn create(
        &self,
        _dst: Box<dyn WriteSeek>,
        _opts: &CreateOptions,
    ) -> Result<Box<dyn ArchiveWriter>, FormatError> {
        Err(FormatError::Unsupported(
            "Squallz does not create RAR archives".into(),
        ))
    }
}

struct RarArchiveReader {
    temp: TempArchive,
    backend: RarBackend,
    entries: Vec<EntryMeta>,
}

impl RarArchiveReader {
    fn open(src: Box<dyn ReadSeek>) -> Result<Self, FormatError> {
        let temp = TempArchive::from_reader(src)?;
        let backend = RarBackend::select(temp.path());
        let entries = backend.list_entries(temp.path())?;
        if entries.is_empty() && temp.len()? > 0 {
            return Err(FormatError::CorruptArchive(format!(
                "{} listed no entries for a non-empty RAR archive",
                backend.name()
            )));
        }
        Ok(Self {
            temp,
            backend,
            entries,
        })
    }
}

impl ArchiveReader for RarArchiveReader {
    fn entries(&mut self) -> Box<dyn Iterator<Item = Result<EntryMeta, FormatError>> + '_> {
        Box::new(self.entries.clone().into_iter().map(Ok))
    }

    fn read_entry(&mut self, path: &EntryPath) -> Result<Box<dyn Read + '_>, FormatError> {
        self.backend.read_entry(self.temp.path(), path)
    }

    fn test(
        &mut self,
        progress: &dyn ProgressSink,
        ctl: &squallz_format_api::ControlToken,
    ) -> Result<TestReport, FormatError> {
        let mut report = TestReport::default();
        let entries = self.entries.clone();
        let total = entries.len() as u64;
        for meta in entries {
            ctl.checkpoint()?;
            if !matches!(meta.entry_type, EntryType::File) {
                continue;
            }
            match self.read_entry(&meta.path) {
                Ok(mut data) => {
                    let mut sink = io::sink();
                    if let Err(e) = io::copy(&mut data, &mut sink) {
                        report.problems.push(format!("{}: {e}", meta.path.display));
                    }
                }
                Err(e) => report.problems.push(format!("{}: {e}", meta.path.display)),
            }
            report.entries_tested += 1;
            progress.on_progress(report.entries_tested, total, &meta.path);
        }
        progress.on_progress(
            report.entries_tested,
            report.entries_tested,
            &EntryPath::from_utf8(""),
        );
        Ok(report)
    }
}

enum RarBackend {
    SevenZip(PathBuf),
    Bsdtar(PathBuf),
}

impl RarBackend {
    fn select(archive: &Path) -> Self {
        if std::env::var_os("SQUALLZ_BSDTAR").is_some() {
            return Self::Bsdtar(bsdtar_tool());
        }
        if let Some(tool) = sevenzip_bridge::sevenzip_tool_if_configured_or_installed() {
            if rar5_v6_requires_bsdtar(&tool, archive) {
                if let Some(bsdtar) = bsdtar_tool_if_available() {
                    return Self::Bsdtar(bsdtar);
                }
            }
            return Self::SevenZip(tool);
        }
        Self::Bsdtar(bsdtar_tool())
    }

    fn name(&self) -> &'static str {
        match self {
            Self::SevenZip(_) => "7zz/7z",
            Self::Bsdtar(_) => "bsdtar",
        }
    }

    fn list_entries(&self, archive: &Path) -> Result<Vec<EntryMeta>, FormatError> {
        match self {
            Self::SevenZip(tool) => sevenzip_bridge::list_entries(tool, archive),
            Self::Bsdtar(tool) => list_bsdtar_entries(tool, archive),
        }
    }

    fn read_entry(&self, archive: &Path, path: &EntryPath) -> Result<Box<dyn Read>, FormatError> {
        match self {
            Self::SevenZip(tool) => sevenzip_bridge::read_entry_stdout(tool, archive, path),
            Self::Bsdtar(tool) => read_bsdtar_entry_stdout(tool, archive, path),
        }
    }
}

fn bsdtar_tool() -> PathBuf {
    if let Some(path) = std::env::var_os("SQUALLZ_BSDTAR") {
        return PathBuf::from(path);
    }
    if Path::new("/usr/bin/bsdtar").exists() {
        return PathBuf::from("/usr/bin/bsdtar");
    }
    PathBuf::from("bsdtar")
}

fn bsdtar_tool_if_available() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("SQUALLZ_BSDTAR") {
        return Some(PathBuf::from(path));
    }
    if Path::new("/usr/bin/bsdtar").exists() {
        return Some(PathBuf::from("/usr/bin/bsdtar"));
    }
    find_on_path("bsdtar")
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

fn rar5_v6_requires_bsdtar(tool: &Path, archive: &Path) -> bool {
    let output = Command::new(tool)
        .arg("l")
        .arg("-slt")
        .arg(archive)
        .stdin(Stdio::null())
        .output();
    let Ok(output) = output else {
        return false;
    };
    if !output.status.success() {
        return false;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    text.lines().any(|line| {
        line.split_once(" = ")
            .is_some_and(|(key, value)| key == "Method" && value.trim().starts_with("v6:"))
    })
}

fn list_bsdtar_entries(tool: &Path, archive: &Path) -> Result<Vec<EntryMeta>, FormatError> {
    let names = run_bsdtar_output(tool, archive, "-tf")?;
    let verbose = run_bsdtar_output(tool, archive, "-tvf").ok();
    let verbose_lines = split_verbose_output(verbose.as_ref());

    let mut entries = Vec::new();
    for (idx, raw) in names.stdout.split(|b| *b == b'\n').enumerate() {
        let raw = trim_cr(raw);
        if raw.is_empty() {
            continue;
        }
        let display = String::from_utf8_lossy(raw).into_owned();
        let detail = verbose_lines
            .get(idx)
            .and_then(|line| parse_verbose_entry(line));
        let entry_type = entry_type_from_detail_or_display(detail.as_ref(), &display);
        entries.push(EntryMeta {
            path: EntryPath::from_raw(raw.to_vec(), display.clone(), "utf-8"),
            entry_type,
            size: detail_size(detail.as_ref()),
            compressed_size: None,
            modified: None,
            unix_mode: detail.and_then(|detail| detail.unix_mode),
            crc32: None,
            encrypted: false,
        });
    }
    Ok(entries)
}

fn split_verbose_output(output: Option<&std::process::Output>) -> Vec<&[u8]> {
    match output {
        Some(output) => output.stdout.split(|b| *b == b'\n').collect(),
        None => Vec::new(),
    }
}

fn trim_cr(raw: &[u8]) -> &[u8] {
    match raw.strip_suffix(b"\r") {
        Some(stripped) => stripped,
        None => raw,
    }
}

fn entry_type_from_detail_or_display(detail: Option<&VerboseEntry>, display: &str) -> EntryType {
    match detail {
        Some(detail) => detail.entry_type.clone(),
        None if display.ends_with('/') => EntryType::Dir,
        None => EntryType::File,
    }
}

fn detail_size(detail: Option<&VerboseEntry>) -> u64 {
    match detail {
        Some(detail) => detail.size,
        None => 0,
    }
}

fn read_bsdtar_entry_stdout(
    tool: &Path,
    archive: &Path,
    path: &EntryPath,
) -> Result<Box<dyn Read>, FormatError> {
    let mut child = Command::new(tool)
        .arg("-xOf")
        .arg(archive)
        .arg("--")
        .arg(&path.display)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(map_tool_spawn_error)?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| FormatError::Other("bsdtar did not provide stdout".into()))?;
    Ok(Box::new(CommandStdoutReader {
        child,
        stdout,
        entry: path.display.clone(),
        finished: false,
    }))
}

fn run_bsdtar_output(
    tool: &Path,
    archive: &Path,
    flag: &str,
) -> Result<std::process::Output, FormatError> {
    let output = Command::new(tool)
        .arg(flag)
        .arg(archive)
        .stdin(Stdio::null())
        .output()
        .map_err(map_tool_spawn_error)?;
    if !output.status.success() {
        return Err(map_tool_failure(&output.stderr));
    }
    Ok(output)
}

struct VerboseEntry {
    entry_type: EntryType,
    size: u64,
    unix_mode: Option<u32>,
}

fn parse_verbose_entry(raw: &[u8]) -> Option<VerboseEntry> {
    let raw = trim_cr(raw);
    if raw.is_empty() {
        return None;
    }
    let line = String::from_utf8_lossy(raw);
    let mut parts = line.split_whitespace();
    let mode = parts.next()?;
    let _links = parts.next()?;
    let _owner = parts.next()?;
    let _group = parts.next()?;
    let size = parts.next()?.parse().ok()?;
    let _month = parts.next()?;
    let _day = parts.next()?;
    let _time_or_year = parts.next()?;
    let rest = parts.collect::<Vec<_>>().join(" ");
    if rest.is_empty() {
        return None;
    }
    let entry_type = match mode.as_bytes().first().copied()? {
        b'd' => EntryType::Dir,
        b'l' => {
            let target = symlink_target_from_verbose_rest(&rest);
            EntryType::Symlink {
                target: target.as_bytes().to_vec(),
            }
        }
        _ => EntryType::File,
    };
    Some(VerboseEntry {
        entry_type,
        size,
        unix_mode: unix_mode_from_verbose(mode),
    })
}

fn symlink_target_from_verbose_rest(rest: &str) -> &str {
    match rest.split_once(" -> ") {
        Some((_, target)) => target,
        None => "",
    }
}

fn unix_mode_from_verbose(mode: &str) -> Option<u32> {
    let bytes = mode.as_bytes();
    if bytes.len() < 10 {
        return None;
    }
    let kind = match bytes[0] {
        b'd' => 0o040000,
        b'l' => 0o120000,
        b'-' => 0o100000,
        _ => 0,
    };
    let mut perms = 0u32;
    for (idx, byte) in bytes[1..10].iter().enumerate() {
        let bit = match idx {
            0 => 0o400,
            1 => 0o200,
            2 => 0o100,
            3 => 0o040,
            4 => 0o020,
            5 => 0o010,
            6 => 0o004,
            7 => 0o002,
            8 => 0o001,
            _ => 0,
        };
        if *byte != b'-' {
            perms |= bit;
        }
    }
    Some(kind | perms)
}

fn map_tool_spawn_error(e: io::Error) -> FormatError {
    if e.kind() == io::ErrorKind::NotFound {
        FormatError::DependencyMissing("bsdtar with RAR/libarchive support".into())
    } else {
        FormatError::Io(e)
    }
}

fn map_tool_failure(stderr: &[u8]) -> FormatError {
    let detail = String::from_utf8_lossy(stderr).trim().to_owned();
    let lower = detail.to_lowercase();
    if lower.contains("unsupported") || lower.contains("not supported") {
        FormatError::DependencyMissing("bsdtar with RAR/libarchive support".into())
    } else if lower.contains("password") {
        FormatError::PasswordRequired
    } else {
        FormatError::CorruptArchive(if detail.is_empty() {
            "bsdtar could not read RAR archive".into()
        } else {
            detail
        })
    }
}

struct CommandStdoutReader {
    child: Child,
    stdout: ChildStdout,
    entry: String,
    finished: bool,
}

impl Read for CommandStdoutReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.finished {
            return Ok(0);
        }
        let n = self.stdout.read(buf)?;
        if n > 0 {
            return Ok(n);
        }
        let status = self.child.wait()?;
        self.finished = true;
        if status.success() {
            Ok(0)
        } else {
            Err(io::Error::other(format!(
                "bsdtar failed while reading {}",
                self.entry
            )))
        }
    }
}

impl Drop for CommandStdoutReader {
    fn drop(&mut self) {
        if !self.finished {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

struct TempArchive {
    path: PathBuf,
}

impl TempArchive {
    fn from_reader(mut src: Box<dyn ReadSeek>) -> Result<Self, FormatError> {
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        src.seek(SeekFrom::Start(0))?;
        let path = std::env::temp_dir().join(format!(
            "squallz-rar-{}-{}-{}.rar",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed),
            system_time_nanos_since_epoch(SystemTime::now())
        ));
        let mut out = fs::File::create(&path)?;
        io::copy(&mut src, &mut out)?;
        Ok(Self { path })
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn len(&self) -> Result<u64, FormatError> {
        Ok(fs::metadata(&self.path)?.len())
    }
}

fn system_time_nanos_since_epoch(time: SystemTime) -> u128 {
    match time.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(duration) => duration.as_nanos(),
        Err(_) => 0,
    }
}

impl Drop for TempArchive {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        crate::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn temp_path(tag: &str, ext: &str) -> PathBuf {
        std::env::temp_dir().join(format!("squallz-rar-{tag}-{}-{ext}", std::process::id()))
    }

    #[test]
    fn rar_format_declares_read_only_capabilities_and_magic() {
        let format = RarFormat;
        assert_eq!(format.id(), "rar");
        assert_eq!(format.extensions(), ["rar", "cbr"]);
        let caps = format.capabilities();
        assert!(!caps.can_create);
        assert!(caps.can_extract);
        assert!(caps.can_test);
        assert!(format.sniff(RAR4_MAGIC, &[]));
        assert!(format.sniff(RAR5_MAGIC, &[]));
    }

    #[test]
    fn rar_verbose_parser_handles_cr_and_missing_symlink_target() {
        let symlink = parse_verbose_entry(
            b"lrwxrwxrwx  0 0      0           0 Jan  1  2020 link -> hello.txt\r",
        )
        .expect("symlink verbose entry");
        assert_eq!(symlink.size, 0);
        assert_eq!(symlink.unix_mode, Some(0o120777));
        assert!(matches!(
            symlink.entry_type,
            EntryType::Symlink { target } if target == b"hello.txt"
        ));

        let symlink_without_arrow =
            parse_verbose_entry(b"lrwxrwxrwx  0 0      0           0 Jan  1  2020 link")
                .expect("symlink without arrow still parses");
        assert!(matches!(
            symlink_without_arrow.entry_type,
            EntryType::Symlink { target } if target.is_empty()
        ));
    }

    #[test]
    fn rar_open_reports_missing_external_tool() {
        let _guard = env_lock();
        let old = std::env::var_os("SQUALLZ_BSDTAR");
        std::env::set_var("SQUALLZ_BSDTAR", "/definitely/missing/squallz-bsdtar");

        let path = temp_path("missing", "rar");
        fs::write(&path, RAR5_MAGIC).unwrap();
        let err = match RarFormat.open(
            Box::new(File::open(&path).unwrap()),
            &OpenOptions::default(),
        ) {
            Ok(_) => panic!("RAR open should fail when SQUALLZ_BSDTAR points to a missing tool"),
            Err(err) => err,
        };
        assert!(matches!(err, FormatError::DependencyMissing(_)), "{err:?}");

        let _ = fs::remove_file(path);
        match old {
            Some(value) => std::env::set_var("SQUALLZ_BSDTAR", value),
            None => std::env::remove_var("SQUALLZ_BSDTAR"),
        }
    }

    #[test]
    fn rar_create_is_unsupported() {
        let path = temp_path("create", "rar");
        let err = match RarFormat.create(
            Box::new(File::create(&path).unwrap()),
            &CreateOptions::default(),
        ) {
            Ok(_) => panic!("RAR creation should be unsupported"),
            Err(err) => err,
        };
        assert!(matches!(err, FormatError::Unsupported(_)), "{err:?}");
        let _ = fs::remove_file(path);
    }

    #[cfg(unix)]
    #[test]
    fn rar_bridge_rejects_empty_listing_from_nonempty_rar() {
        use std::os::unix::fs::PermissionsExt;

        struct EnvRestore {
            key: &'static str,
            old: Option<std::ffi::OsString>,
        }

        impl Drop for EnvRestore {
            fn drop(&mut self) {
                match &self.old {
                    Some(value) => std::env::set_var(self.key, value),
                    None => std::env::remove_var(self.key),
                }
            }
        }

        let _guard = env_lock();
        let old_tool = std::env::var_os("SQUALLZ_BSDTAR");
        let _restore_tool = EnvRestore {
            key: "SQUALLZ_BSDTAR",
            old: old_tool,
        };

        let script = temp_path("empty-bsdtar", "sh");
        let archive = temp_path("empty-listing", "rar");
        fs::write(
            &script,
            r#"#!/bin/sh
if [ "$1" = "-tf" ] || [ "$1" = "-tvf" ]; then
  exit 0
fi
exit 2
"#,
        )
        .unwrap();
        let mut perms = fs::metadata(&script).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script, perms).unwrap();
        fs::write(&archive, RAR5_MAGIC).unwrap();
        std::env::set_var("SQUALLZ_BSDTAR", &script);

        let err = match RarFormat.open(
            Box::new(File::open(&archive).unwrap()),
            &OpenOptions::default(),
        ) {
            Ok(_) => panic!("non-empty RAR with empty bridge listing must not open as healthy"),
            Err(err) => err,
        };
        assert!(matches!(err, FormatError::CorruptArchive(_)), "{err:?}");

        let _ = fs::remove_file(script);
        let _ = fs::remove_file(archive);
    }

    #[cfg(unix)]
    #[test]
    fn rar_bridge_prefers_7z_for_listing_testing_and_entry_streams() {
        use std::io::Read;
        use std::os::unix::fs::PermissionsExt;

        struct EnvRestore {
            key: &'static str,
            old: Option<std::ffi::OsString>,
        }

        impl Drop for EnvRestore {
            fn drop(&mut self) {
                match &self.old {
                    Some(value) => std::env::set_var(self.key, value),
                    None => std::env::remove_var(self.key),
                }
            }
        }

        let _guard = env_lock();
        let _restore_7z = EnvRestore {
            key: "SQUALLZ_7Z",
            old: std::env::var_os("SQUALLZ_7Z"),
        };
        let _restore_bsdtar = EnvRestore {
            key: "SQUALLZ_BSDTAR",
            old: std::env::var_os("SQUALLZ_BSDTAR"),
        };
        let _restore_log = EnvRestore {
            key: "SQUALLZ_FAKE_7Z_LOG",
            old: std::env::var_os("SQUALLZ_FAKE_7Z_LOG"),
        };

        let script = temp_path("fake-7z", "sh");
        let log = temp_path("fake-7z", "log");
        let archive = temp_path("fake-7z-archive", "rar");
        let script_body = r#"#!/bin/sh
set -eu
printf '%s\n' "$*" >> "$SQUALLZ_FAKE_7Z_LOG"
if [ "$1" = "l" ] && [ "$2" = "-slt" ]; then
  cat <<'EOF'
Path = docs
Folder = +
Size = 0
Attributes = D

Path = hello.txt
Folder = -
Size = 21
Packed Size = 12
CRC = 1234ABCD
Encrypted = -

Path = -dash.txt
Folder = -
Size = 18
Packed Size = 9
Encrypted = -

EOF
  exit 0
fi
if [ "$1" = "x" ] && [ "$2" = "-so" ]; then
  last=""
  prev=""
  for arg in "$@"; do
    prev="$last"
    last="$arg"
  done
  if [ "$last" = "-dash.txt" ] && [ "$prev" != "--" ]; then
    printf 'missing -- before dash entry\n' >&2
    exit 9
  fi
  case "$last" in
    hello.txt) printf 'hello from rar via 7z' ;;
    -dash.txt) printf 'dash entry content' ;;
    *) printf 'unknown entry: %s\n' "$last" >&2; exit 3 ;;
  esac
  exit 0
fi
printf 'unexpected args\n' >&2
exit 2
"#;
        fs::write(&script, script_body).unwrap();
        let mut perms = fs::metadata(&script).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script, perms).unwrap();
        let _ = fs::remove_file(&log);
        fs::write(&archive, RAR5_MAGIC).unwrap();

        std::env::set_var("SQUALLZ_7Z", &script);
        std::env::remove_var("SQUALLZ_BSDTAR");
        std::env::set_var("SQUALLZ_FAKE_7Z_LOG", &log);

        let mut reader = RarFormat
            .open(
                Box::new(File::open(&archive).unwrap()),
                &OpenOptions::default(),
            )
            .unwrap();
        let entries: Vec<_> = reader.entries().collect::<Result<_, _>>().unwrap();
        assert_eq!(entries.len(), 3);
        assert!(matches!(entries[0].entry_type, EntryType::Dir));
        assert_eq!(entries[1].path.display, "hello.txt");
        assert_eq!(entries[1].size, 21);
        assert_eq!(entries[1].crc32, Some(0x1234ABCD));
        assert_eq!(entries[2].path.display, "-dash.txt");

        let mut hello = String::new();
        reader
            .read_entry(&entries[1].path)
            .unwrap()
            .read_to_string(&mut hello)
            .unwrap();
        assert_eq!(hello, "hello from rar via 7z");

        let mut dash = String::new();
        reader
            .read_entry(&entries[2].path)
            .unwrap()
            .read_to_string(&mut dash)
            .unwrap();
        assert_eq!(dash, "dash entry content");

        let report = reader
            .test(
                &squallz_format_api::NoProgress,
                &squallz_format_api::ControlToken::new(),
            )
            .unwrap();
        assert_eq!(report.entries_tested, 2);
        assert!(report.problems.is_empty(), "{:?}", report.problems);

        let log = fs::read_to_string(&log).unwrap();
        assert!(log.contains("l -slt"), "{log}");
        assert!(log.contains("x -so"), "{log}");
        assert!(log.contains("-- -dash.txt"), "{log}");

        let _ = fs::remove_file(script);
        let _ = fs::remove_file(log);
        let _ = fs::remove_file(archive);
    }

    #[cfg(unix)]
    #[test]
    fn rar5_v6_method_detection_requests_bsdtar_fallback() {
        use std::os::unix::fs::PermissionsExt;

        let script = temp_path("fake-v6-7z", "sh");
        let archive = temp_path("fake-v6", "rar");
        fs::write(
            &script,
            r#"#!/bin/sh
if [ "$1" = "l" ] && [ "$2" = "-slt" ]; then
  cat <<'EOF'
Path = sample.rar
Type = Rar5
Method = v6:128K:m5

Path = hello.txt
Size = 5
Method = v6:m5:128K
EOF
  exit 0
fi
exit 2
"#,
        )
        .unwrap();
        let mut perms = fs::metadata(&script).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script, perms).unwrap();
        fs::write(&archive, RAR5_MAGIC).unwrap();

        assert!(rar5_v6_requires_bsdtar(&script, &archive));

        let _ = fs::remove_file(script);
        let _ = fs::remove_file(archive);
    }

    #[cfg(unix)]
    #[test]
    fn rar_bridge_uses_bsdtar_for_listing_testing_and_entry_streams() {
        use std::io::Read;
        use std::os::unix::fs::PermissionsExt;

        struct EnvRestore {
            key: &'static str,
            old: Option<std::ffi::OsString>,
        }

        impl Drop for EnvRestore {
            fn drop(&mut self) {
                match &self.old {
                    Some(value) => std::env::set_var(self.key, value),
                    None => std::env::remove_var(self.key),
                }
            }
        }

        let _guard = env_lock();
        let old_tool = std::env::var_os("SQUALLZ_BSDTAR");
        let old_log = std::env::var_os("SQUALLZ_FAKE_BSDTAR_LOG");
        let _restore_tool = EnvRestore {
            key: "SQUALLZ_BSDTAR",
            old: old_tool,
        };
        let _restore_log = EnvRestore {
            key: "SQUALLZ_FAKE_BSDTAR_LOG",
            old: old_log,
        };

        let script = temp_path("fake-bsdtar", "sh");
        let log = temp_path("fake-bsdtar", "log");
        let archive = temp_path("fake-archive", "rar");
        let script_body = r#"#!/bin/sh
set -eu
printf '%s\n' "$*" >> "$SQUALLZ_FAKE_BSDTAR_LOG"
if [ "$1" = "-tf" ]; then
  printf 'docs/\nhello.txt\nlink\n-dash.txt\n'
  exit 0
fi
if [ "$1" = "-tvf" ]; then
  printf 'drwxr-xr-x  0 0      0           0 Jan  1  2020 docs/\n'
  printf -- '-rw-r--r--  0 0      0          21 Jan  1  2020 hello.txt\n'
  printf 'lrwxrwxrwx  0 0      0           0 Jan  1  2020 link -> hello.txt\n'
  printf -- '-rw-r--r--  0 0      0          18 Jan  1  2020 -dash.txt\n'
  exit 0
fi
if [ "$1" = "-xOf" ]; then
  last=""
  prev=""
  for arg in "$@"; do
    prev="$last"
    last="$arg"
  done
  if [ "$last" = "-dash.txt" ] && [ "$prev" != "--" ]; then
    printf 'missing -- before dash entry\n' >&2
    exit 9
  fi
  case "$last" in
    hello.txt) printf 'hello from rar bridge' ;;
    -dash.txt) printf 'dash entry content' ;;
    *) printf 'unknown entry: %s\n' "$last" >&2; exit 3 ;;
  esac
  exit 0
fi
printf 'unexpected args\n' >&2
exit 2
"#;
        fs::write(&script, script_body).unwrap();
        let mut perms = fs::metadata(&script).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script, perms).unwrap();
        let _ = fs::remove_file(&log);
        fs::write(&archive, RAR5_MAGIC).unwrap();

        std::env::set_var("SQUALLZ_BSDTAR", &script);
        std::env::set_var("SQUALLZ_FAKE_BSDTAR_LOG", &log);

        let mut reader = RarFormat
            .open(
                Box::new(File::open(&archive).unwrap()),
                &OpenOptions::default(),
            )
            .unwrap();
        let entries: Vec<_> = reader.entries().collect::<Result<_, _>>().unwrap();
        assert_eq!(entries.len(), 4);
        assert!(matches!(entries[0].entry_type, EntryType::Dir));
        assert_eq!(entries[1].path.display, "hello.txt");
        assert_eq!(entries[1].size, 21);
        assert_eq!(entries[1].unix_mode, Some(0o100644));
        assert_eq!(entries[2].path.display, "link");
        assert!(matches!(
            &entries[2].entry_type,
            EntryType::Symlink { target } if target == b"hello.txt"
        ));
        assert_eq!(entries[3].path.display, "-dash.txt");

        let mut hello = String::new();
        reader
            .read_entry(&entries[1].path)
            .unwrap()
            .read_to_string(&mut hello)
            .unwrap();
        assert_eq!(hello, "hello from rar bridge");

        let mut dash = String::new();
        reader
            .read_entry(&entries[3].path)
            .unwrap()
            .read_to_string(&mut dash)
            .unwrap();
        assert_eq!(dash, "dash entry content");

        let report = reader
            .test(
                &squallz_format_api::NoProgress,
                &squallz_format_api::ControlToken::new(),
            )
            .unwrap();
        assert_eq!(report.entries_tested, 2);
        assert!(report.problems.is_empty(), "{:?}", report.problems);

        let log = fs::read_to_string(&log).unwrap();
        assert!(log.contains("-tf"));
        assert!(log.contains("-xOf"));
        assert!(log.contains("-- -dash.txt"), "{log}");

        let _ = fs::remove_file(script);
        let _ = fs::remove_file(log);
        let _ = fs::remove_file(archive);
    }
}
