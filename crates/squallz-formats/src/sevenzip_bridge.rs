//! Cross-platform 7-Zip/7zz read bridge for long-tail unpack-only formats.
//!
//! The bridge lists entries and streams individual files through stdout so
//! extraction still flows through Squallz's shared safe extraction engine.

mod wim_writer;

use std::collections::BTreeMap;
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

struct SevenZipSpec {
    id: &'static str,
    extensions: &'static [&'static str],
}

pub(crate) struct SevenZipBridgeFormat {
    spec: &'static SevenZipSpec,
}

const SPECS: &[SevenZipSpec] = &[
    SevenZipSpec {
        id: "wim",
        extensions: &["wim", "swm", "esd"],
    },
    SevenZipSpec {
        id: "apfs",
        extensions: &["apfs"],
    },
    SevenZipSpec {
        id: "ar",
        extensions: &["ar", "a", "deb", "lib"],
    },
    SevenZipSpec {
        id: "arj",
        extensions: &["arj"],
    },
    SevenZipSpec {
        id: "cab",
        extensions: &["cab"],
    },
    SevenZipSpec {
        id: "chm",
        extensions: &["chm", "chw", "chi", "chq"],
    },
    SevenZipSpec {
        id: "cpio",
        extensions: &["cpio"],
    },
    SevenZipSpec {
        id: "cramfs",
        extensions: &["cramfs"],
    },
    SevenZipSpec {
        id: "dmg",
        extensions: &["dmg"],
    },
    SevenZipSpec {
        id: "ext",
        extensions: &["ext", "ext2", "ext3", "ext4"],
    },
    SevenZipSpec {
        id: "fat",
        extensions: &["fat"],
    },
    SevenZipSpec {
        id: "gpt",
        extensions: &["gpt"],
    },
    SevenZipSpec {
        id: "hfs",
        extensions: &["hfs", "hfsx"],
    },
    SevenZipSpec {
        id: "ihex",
        extensions: &["ihex", "hex"],
    },
    SevenZipSpec {
        id: "iso",
        extensions: &["iso"],
    },
    SevenZipSpec {
        id: "lzh",
        extensions: &["lzh", "lha"],
    },
    SevenZipSpec {
        id: "lzma",
        extensions: &["lzma"],
    },
    SevenZipSpec {
        id: "mbr",
        extensions: &["mbr"],
    },
    SevenZipSpec {
        id: "msi",
        extensions: &["msi", "msp"],
    },
    SevenZipSpec {
        id: "nsis",
        extensions: &["nsis"],
    },
    SevenZipSpec {
        id: "ntfs",
        extensions: &["ntfs"],
    },
    SevenZipSpec {
        id: "qcow2",
        extensions: &["qcow", "qcow2", "qcow2c"],
    },
    SevenZipSpec {
        id: "rpm",
        extensions: &["rpm"],
    },
    SevenZipSpec {
        id: "squashfs",
        extensions: &["squashfs"],
    },
    SevenZipSpec {
        id: "udf",
        extensions: &["udf"],
    },
    SevenZipSpec {
        id: "uefi",
        extensions: &["scap", "uefif"],
    },
    SevenZipSpec {
        id: "vdi",
        extensions: &["vdi"],
    },
    SevenZipSpec {
        id: "vhd",
        extensions: &["vhd"],
    },
    SevenZipSpec {
        id: "vhdx",
        extensions: &["vhdx"],
    },
    SevenZipSpec {
        id: "vmdk",
        extensions: &["vmdk"],
    },
    SevenZipSpec {
        id: "xar",
        extensions: &["xar", "pkg"],
    },
    SevenZipSpec {
        id: "z",
        extensions: &["z", "taz"],
    },
];

pub(crate) fn formats() -> impl Iterator<Item = SevenZipBridgeFormat> {
    SPECS.iter().map(|spec| SevenZipBridgeFormat { spec })
}

impl ArchiveFormat for SevenZipBridgeFormat {
    fn id(&self) -> &'static str {
        self.spec.id
    }

    fn extensions(&self) -> &'static [&'static str] {
        self.spec.extensions
    }

    fn capabilities(&self) -> FormatCapabilities {
        FormatCapabilities {
            can_create: self.spec.id == "wim",
            can_extract: true,
            can_encrypt_data: false,
            can_encrypt_names: false,
            can_split: false,
            can_update: false,
            can_test: true,
        }
    }

    fn sniff(&self, head: &[u8], _tail: &[u8]) -> bool {
        match self.spec.id {
            "wim" => head.starts_with(b"MSWIM\0\0\0"),
            "ar" => head.starts_with(b"!<arch>\n"),
            "cab" => head.starts_with(b"MSCF"),
            "rpm" => head.starts_with(&[0xED, 0xAB, 0xEE, 0xDB]),
            "xar" => head.starts_with(b"xar!"),
            _ => false,
        }
    }

    fn open(
        &self,
        src: Box<dyn ReadSeek>,
        opts: &OpenOptions,
    ) -> Result<Box<dyn ArchiveReader>, FormatError> {
        if opts.password.is_some() {
            return Err(FormatError::Unsupported(format!(
                "{} password extraction through the 7-Zip bridge is not supported yet",
                self.spec.id
            )));
        }
        Ok(Box::new(SevenZipArchiveReader::open(src, self.spec)?))
    }

    fn create(
        &self,
        dst: Box<dyn WriteSeek>,
        opts: &CreateOptions,
    ) -> Result<Box<dyn ArchiveWriter>, FormatError> {
        if self.spec.id == "wim" {
            return wim_writer::create(dst, opts);
        }
        Err(FormatError::Unsupported(format!(
            "format {} is currently read-only through the 7-Zip bridge",
            self.spec.id
        )))
    }
}

struct SevenZipArchiveReader {
    temp: TempArchive,
    tool: PathBuf,
    entries: Vec<EntryMeta>,
    backend_paths: BTreeMap<String, String>,
}

impl SevenZipArchiveReader {
    fn open(src: Box<dyn ReadSeek>, spec: &'static SevenZipSpec) -> Result<Self, FormatError> {
        let tool = sevenzip_tool()?;
        let temp = TempArchive::from_reader(src, spec.id)?;
        let raw_entries = list_entries(&tool, temp.path())?;
        let (entries, backend_paths) = normalize_entries(spec, raw_entries);
        if entries.is_empty() && temp.len()? > 0 {
            return Err(FormatError::CorruptArchive(format!(
                "7-Zip listed no entries for a non-empty {} archive",
                spec.id
            )));
        }
        Ok(Self {
            temp,
            tool,
            entries,
            backend_paths,
        })
    }
}

impl ArchiveReader for SevenZipArchiveReader {
    fn entries(&mut self) -> Box<dyn Iterator<Item = Result<EntryMeta, FormatError>> + '_> {
        Box::new(self.entries.clone().into_iter().map(Ok))
    }

    fn read_entry(&mut self, path: &EntryPath) -> Result<Box<dyn Read + '_>, FormatError> {
        let backend_path = backend_path_for(&self.backend_paths, path);
        let mut command = Command::new(&self.tool);
        command.arg("x").arg("-so").arg(self.temp.path());
        if !backend_path.is_empty() {
            command.arg("--").arg(backend_path);
        }
        let mut child = command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(map_tool_spawn_error)?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| FormatError::Other("7-Zip did not provide stdout".into()))?;
        Ok(Box::new(CommandStdoutReader {
            child,
            stdout,
            entry: path.display.clone(),
            finished: false,
        }))
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

fn normalize_entries(
    spec: &SevenZipSpec,
    mut entries: Vec<EntryMeta>,
) -> (Vec<EntryMeta>, BTreeMap<String, String>) {
    let mut backend_paths = BTreeMap::new();
    if !matches!(spec.id, "lzma" | "z") || entries.len() != 1 {
        return (entries, backend_paths);
    }

    let Some(entry) = entries.first_mut() else {
        return (entries, backend_paths);
    };
    if !Path::new(&entry.path.display).is_absolute() {
        return (entries, backend_paths);
    }
    let safe_name = "payload".to_owned();
    entry.path = EntryPath::from_utf8(&safe_name);
    backend_paths.insert(safe_name, String::new());
    (entries, backend_paths)
}

fn backend_path_for<'a>(
    backend_paths: &'a BTreeMap<String, String>,
    path: &'a EntryPath,
) -> &'a str {
    match backend_paths.get(&path.display) {
        Some(backend_path) => backend_path.as_str(),
        None => path.display.as_str(),
    }
}

pub(crate) fn sevenzip_tool_if_configured_or_installed() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("SQUALLZ_7Z") {
        return Some(PathBuf::from(path));
    }
    for candidate in ["7zz", "7z", "7za"] {
        if let Some(path) = find_on_path(candidate) {
            return Some(path);
        }
    }
    None
}

fn sevenzip_tool() -> Result<PathBuf, FormatError> {
    if let Some(path) = sevenzip_tool_if_configured_or_installed() {
        return Ok(path);
    }
    Ok(PathBuf::from("7zz"))
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

pub(crate) fn list_entries(tool: &Path, archive: &Path) -> Result<Vec<EntryMeta>, FormatError> {
    let output = run_7z_output(tool, archive, &["l", "-slt"])?;
    parse_7z_list(&output.stdout)
}

pub(crate) fn read_entry_stdout(
    tool: &Path,
    archive: &Path,
    path: &EntryPath,
) -> Result<Box<dyn Read>, FormatError> {
    let mut child = Command::new(tool)
        .arg("x")
        .arg("-so")
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
        .ok_or_else(|| FormatError::Other("7-Zip did not provide stdout".into()))?;
    Ok(Box::new(CommandStdoutReader {
        child,
        stdout,
        entry: path.display.clone(),
        finished: false,
    }))
}

fn parse_7z_list(stdout: &[u8]) -> Result<Vec<EntryMeta>, FormatError> {
    let text = String::from_utf8_lossy(stdout);
    let mut entries = Vec::new();
    let mut block = BTreeMap::<String, String>::new();
    for line in text.lines().chain(std::iter::once("")) {
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            push_list_block(&mut entries, &mut block);
            continue;
        }
        if let Some((key, value)) = line.split_once(" = ") {
            block.insert(key.trim().to_owned(), value.to_owned());
        }
    }
    infer_directory_entries(&mut entries);
    Ok(entries)
}

fn infer_directory_entries(entries: &mut [EntryMeta]) {
    let paths: Vec<String> = entries
        .iter()
        .map(|entry| entry.path.display.clone())
        .collect();
    for entry in entries {
        if matches!(entry.entry_type, EntryType::Dir) {
            continue;
        }
        let prefix = format!("{}/", entry.path.display.trim_end_matches('/'));
        if paths.iter().any(|path| path.starts_with(&prefix)) {
            entry.entry_type = EntryType::Dir;
            entry.size = 0;
            entry.compressed_size = None;
        }
    }
}

fn push_list_block(entries: &mut Vec<EntryMeta>, block: &mut BTreeMap<String, String>) {
    let Some(path) = block.get("Path").cloned() else {
        block.clear();
        return;
    };
    if block.contains_key("Type") && block.contains_key("Physical Size") {
        block.clear();
        return;
    }
    let is_entry = block.contains_key("Folder")
        || block.contains_key("Size")
        || block.contains_key("Packed Size")
        || block.contains_key("Attributes")
        || block.contains_key("CRC")
        || block.contains_key("Encrypted")
        || block.contains_key("Type");
    if !is_entry || path.is_empty() || path == "." || path == "./" {
        block.clear();
        return;
    }

    let attrs = block_text(block, "Attributes");
    let folder = block.get("Folder").is_some_and(|value| value.trim() == "+")
        || attrs.bytes().any(|b| b == b'D')
        || block
            .get("Type")
            .is_some_and(|value| value.trim().eq_ignore_ascii_case("directory"));
    let entry_type = if folder {
        EntryType::Dir
    } else {
        EntryType::File
    };
    let size = list_entry_size(block, folder);
    let compressed_size = block
        .get("Packed Size")
        .and_then(|value| value.trim().parse::<u64>().ok());
    let crc32 = block
        .get("CRC")
        .and_then(|value| u32::from_str_radix(value.trim(), 16).ok());
    let encrypted = block
        .get("Encrypted")
        .is_some_and(|value| value.trim() == "+");
    entries.push(EntryMeta {
        path: EntryPath::from_utf8(&path),
        entry_type,
        size,
        compressed_size,
        modified: None,
        unix_mode: None,
        crc32,
        encrypted,
    });
    block.clear();
}

fn block_text<'a>(block: &'a BTreeMap<String, String>, key: &str) -> &'a str {
    match block.get(key) {
        Some(value) => value.as_str(),
        None => "",
    }
}

fn list_entry_size(block: &BTreeMap<String, String>, folder: bool) -> u64 {
    if folder {
        return 0;
    }
    if let Some(value) = block.get("Size") {
        if let Ok(size) = value.trim().parse::<u64>() {
            return size;
        }
    }
    0
}

fn run_7z_output(
    tool: &Path,
    archive: &Path,
    args: &[&str],
) -> Result<std::process::Output, FormatError> {
    let output = Command::new(tool)
        .args(args)
        .arg(archive)
        .stdin(Stdio::null())
        .output()
        .map_err(map_tool_spawn_error)?;
    if !output.status.success() {
        return Err(map_tool_failure(&output.stderr));
    }
    Ok(output)
}

fn map_tool_spawn_error(e: io::Error) -> FormatError {
    if e.kind() == io::ErrorKind::NotFound {
        FormatError::DependencyMissing("7zz/7z external format bridge".into())
    } else {
        FormatError::Io(e)
    }
}

fn map_tool_failure(stderr: &[u8]) -> FormatError {
    let detail = String::from_utf8_lossy(stderr).trim().to_owned();
    let lower = detail.to_lowercase();
    if lower.contains("unsupported") || lower.contains("not implemented") {
        FormatError::DependencyMissing("7zz/7z external format bridge".into())
    } else if lower.contains("password") {
        FormatError::PasswordRequired
    } else {
        FormatError::CorruptArchive(if detail.is_empty() {
            "7-Zip could not read archive".into()
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
                "7-Zip failed while reading {}",
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
    fn from_reader(mut src: Box<dyn ReadSeek>, tag: &str) -> Result<Self, FormatError> {
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        src.seek(SeekFrom::Start(0))?;
        let path = std::env::temp_dir().join(format!(
            "squallz-7z-{}-{}-{}.{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed),
            system_time_nanos(SystemTime::now()),
            tag
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

fn system_time_nanos(time: SystemTime) -> u128 {
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
        std::env::temp_dir().join(format!("squallz-7z-{tag}-{}.{ext}", std::process::id()))
    }

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

    #[test]
    fn sevenzip_bridge_declares_read_only_capabilities() {
        let format = SevenZipBridgeFormat { spec: &SPECS[4] };
        assert_eq!(format.id(), "cab");
        assert_eq!(format.extensions(), ["cab"]);
        let caps = format.capabilities();
        assert!(!caps.can_create);
        assert!(caps.can_extract);
        assert!(caps.can_test);
        let wim = SevenZipBridgeFormat { spec: &SPECS[0] };
        assert!(wim.capabilities().can_create);
        assert!(wim.sniff(b"MSWIM\0\0\0more", &[]));
        assert!(SevenZipBridgeFormat { spec: &SPECS[2] }.sniff(b"!<arch>\n", &[]));
    }

    #[test]
    fn backend_path_falls_back_to_display_path() {
        let path = EntryPath::from_utf8("hello.txt");
        let backend_paths = BTreeMap::new();
        assert_eq!(backend_path_for(&backend_paths, &path), "hello.txt");

        let mut backend_paths = BTreeMap::new();
        backend_paths.insert("hello.txt".to_owned(), "raw/backend/path.txt".to_owned());
        assert_eq!(
            backend_path_for(&backend_paths, &path),
            "raw/backend/path.txt"
        );
    }

    #[test]
    fn sevenzip_listing_skips_archive_metadata_block() {
        let stdout = br#"
Path = /tmp/squallz-7z-temp.wim
Type = wim
Physical Size = 1351
Size = 17
Packed Size = 17
Images = 1

Path = project
Folder = +
Attributes = D

Path = project/README.txt
Folder = -
Size = 10
Packed Size = 10
Attributes = N

"#;

        let entries = parse_7z_list(stdout).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].path.display, "project");
        assert_eq!(entries[1].path.display, "project/README.txt");
        assert_eq!(entries[1].size, 10);
        assert!(!entries
            .iter()
            .any(|entry| entry.path.display.starts_with('/')));
    }

    #[test]
    fn sevenzip_listing_keeps_xar_typed_entries() {
        let stdout = br#"
Path = /tmp/squallz-7z-temp.xar
Type = Xar
Physical Size = 979
Method = SHA1

Path = hello.txt
Size = 12
Packed Size = 20
Mode = -rw-r--r--
Type = file

Path = dir
Size =
Packed Size =
Mode = drwxr-xr-x
Type = directory

Path = dir/nested.txt
Size = 13
Packed Size = 21
Mode = -rw-r--r--
Type = file

"#;

        let entries = parse_7z_list(stdout).unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].path.display, "hello.txt");
        assert_eq!(entries[0].size, 12);
        assert!(matches!(entries[0].entry_type, EntryType::File));
        assert_eq!(entries[1].path.display, "dir");
        assert!(matches!(entries[1].entry_type, EntryType::Dir));
        assert_eq!(entries[2].path.display, "dir/nested.txt");
        assert_eq!(entries[2].size, 13);
    }

    #[test]
    fn sevenzip_listing_skips_cpio_root_dot_entry() {
        let stdout = br#"
Path = .
Folder = +
Size = 0
Packed Size = 0

Path = ./sub
Folder = +
Size = 0
Packed Size = 0

Path = ./sub/data.txt
Folder = -
Size = 15
Packed Size = 16

Path = ./README.txt
Folder = -
Size = 14
Packed Size = 16

"#;

        let entries = parse_7z_list(stdout).unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].path.display, "./sub");
        assert_eq!(entries[1].path.display, "./sub/data.txt");
        assert_eq!(entries[2].path.display, "./README.txt");
        assert!(!entries.iter().any(|entry| entry.path.display == "."));
    }

    #[test]
    fn sevenzip_listing_infers_directory_prefix_entries() {
        let stdout = br#"
Path = sub
Folder = -
Size = 0

Path = README.txt
Folder = -
Size = 15
Packed Size = 4096

Path = sub/data.txt
Folder = -
Size = 16
Packed Size = 4096

"#;

        let entries = parse_7z_list(stdout).unwrap();
        assert_eq!(entries.len(), 3);
        assert!(matches!(entries[0].entry_type, EntryType::Dir));
        assert_eq!(entries[0].path.display, "sub");
        assert_eq!(entries[0].size, 0);
        assert_eq!(entries[0].compressed_size, None);
        assert!(matches!(entries[1].entry_type, EntryType::File));
        assert!(matches!(entries[2].entry_type, EntryType::File));
    }

    #[test]
    fn sevenzip_stream_listing_normalizes_temp_absolute_path() {
        let raw = vec![EntryMeta {
            path: EntryPath::from_utf8("/tmp/squallz-7z-temp.lzma"),
            entry_type: EntryType::File,
            size: 0,
            compressed_size: Some(32),
            modified: None,
            unix_mode: None,
            crc32: None,
            encrypted: false,
        }];
        let spec = SPECS.iter().find(|spec| spec.id == "lzma").unwrap();
        let (entries, backend_paths) = normalize_entries(spec, raw);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path.display, "payload");
        assert_eq!(backend_paths.get("payload").map(String::as_str), Some(""));
    }

    #[cfg(unix)]
    #[test]
    fn sevenzip_stream_bridge_reads_without_entry_argument() {
        use std::io::Read;
        use std::os::unix::fs::PermissionsExt;

        let _guard = env_lock();
        let _restore_tool = EnvRestore {
            key: "SQUALLZ_7Z",
            old: std::env::var_os("SQUALLZ_7Z"),
        };
        let _restore_log = EnvRestore {
            key: "SQUALLZ_FAKE_7Z_LOG",
            old: std::env::var_os("SQUALLZ_FAKE_7Z_LOG"),
        };

        let script = temp_path("fake-stream-7z", "sh");
        let log = temp_path("fake-stream-7z", "log");
        let archive = temp_path("fake-stream", "lzma");
        let script_body = r#"#!/bin/sh
set -eu
printf '%s\n' "$*" >> "$SQUALLZ_FAKE_7Z_LOG"
if [ "$1" = "l" ]; then
  cat <<'EOF'
Path = /tmp/squallz-7z-temp.lzma
Type = lzma
Method = LZMA:23

----------
Size =
Packed Size =
Method = LZMA:23

EOF
  exit 0
fi
if [ "$1" = "x" ] && [ "$2" = "-so" ]; then
  if [ "$#" -ne 3 ]; then
    printf 'stream extraction must not pass an entry path\n' >&2
    exit 9
  fi
  printf 'stream payload'
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
        fs::write(&archive, b"fake lzma").unwrap();

        std::env::set_var("SQUALLZ_7Z", &script);
        std::env::set_var("SQUALLZ_FAKE_7Z_LOG", &log);

        let spec = SPECS.iter().find(|spec| spec.id == "lzma").unwrap();
        let mut reader = SevenZipBridgeFormat { spec }
            .open(
                Box::new(File::open(&archive).unwrap()),
                &OpenOptions::default(),
            )
            .unwrap();
        let entries: Vec<_> = reader.entries().collect::<Result<_, _>>().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path.display, "payload");

        let mut payload = String::new();
        reader
            .read_entry(&entries[0].path)
            .unwrap()
            .read_to_string(&mut payload)
            .unwrap();
        assert_eq!(payload, "stream payload");

        let log = fs::read_to_string(&log).unwrap();
        assert!(log.lines().any(|line| line.starts_with("x -so ")));
        assert!(!log.contains(" -- "), "{log}");

        let _ = fs::remove_file(script);
        let _ = fs::remove_file(log);
        let _ = fs::remove_file(archive);
    }

    #[cfg(unix)]
    #[test]
    fn sevenzip_bridge_uses_tool_for_listing_testing_and_entry_streams() {
        use std::io::Read;
        use std::os::unix::fs::PermissionsExt;

        let _guard = env_lock();
        let _restore_tool = EnvRestore {
            key: "SQUALLZ_7Z",
            old: std::env::var_os("SQUALLZ_7Z"),
        };
        let _restore_log = EnvRestore {
            key: "SQUALLZ_FAKE_7Z_LOG",
            old: std::env::var_os("SQUALLZ_FAKE_7Z_LOG"),
        };

        let script = temp_path("fake-7z", "sh");
        let log = temp_path("fake-7z", "log");
        let archive = temp_path("fake-archive", "cab");
        let script_body = r#"#!/bin/sh
set -eu
printf '%s\n' "$*" >> "$SQUALLZ_FAKE_7Z_LOG"
if [ "$1" = "l" ]; then
  cat <<'EOF'
Path = docs
Folder = +
Size = 0
Attributes = D

Path = hello.txt
Folder = -
Size = 28
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
    hello.txt) printf 'hello from 7z bridge payload' ;;
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
        fs::write(&archive, b"MSCF fake cab").unwrap();

        std::env::set_var("SQUALLZ_7Z", &script);
        std::env::set_var("SQUALLZ_FAKE_7Z_LOG", &log);

        let mut reader = SevenZipBridgeFormat { spec: &SPECS[4] }
            .open(
                Box::new(File::open(&archive).unwrap()),
                &OpenOptions::default(),
            )
            .unwrap();
        let entries: Vec<_> = reader.entries().collect::<Result<_, _>>().unwrap();
        assert_eq!(entries.len(), 3);
        assert!(matches!(entries[0].entry_type, EntryType::Dir));
        assert_eq!(entries[1].path.display, "hello.txt");
        assert_eq!(entries[1].size, 28);
        assert_eq!(entries[1].compressed_size, Some(12));
        assert_eq!(entries[1].crc32, Some(0x1234_ABCD));
        assert_eq!(entries[2].path.display, "-dash.txt");

        let mut hello = String::new();
        reader
            .read_entry(&entries[1].path)
            .unwrap()
            .read_to_string(&mut hello)
            .unwrap();
        assert_eq!(hello, "hello from 7z bridge payload");

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
        assert!(log.contains("l -slt"));
        assert!(log.contains("x -so"));
        assert!(log.contains("-- -dash.txt"), "{log}");

        let _ = fs::remove_file(script);
        let _ = fs::remove_file(log);
        let _ = fs::remove_file(archive);
    }

    #[cfg(unix)]
    #[test]
    fn wim_bridge_creates_through_wimlib_writer() {
        use std::os::unix::fs::PermissionsExt;

        let _guard = env_lock();
        let _restore_wimlib = EnvRestore {
            key: "SQUALLZ_WIMLIB",
            old: std::env::var_os("SQUALLZ_WIMLIB"),
        };
        let _restore_log = EnvRestore {
            key: "SQUALLZ_FAKE_WIMLIB_LOG",
            old: std::env::var_os("SQUALLZ_FAKE_WIMLIB_LOG"),
        };

        let script = temp_path("fake-wimlib", "sh");
        let log = temp_path("fake-wimlib", "log");
        let archive = temp_path("created-wim", "wim");
        let script_body = r#"#!/bin/sh
set -eu
printf '%s\n' "$*" >> "$SQUALLZ_FAKE_WIMLIB_LOG"
if [ "$1" = "capture" ]; then
  src="$2"
  out="$3"
  [ -d "$src/project/sub" ]
  [ "$(cat "$src/project/a.txt")" = "hello wim" ]
  [ "$(cat "$src/project/sub/b.txt")" = "nested wim" ]
  printf 'MSWIM\000\000\000fake-wim' > "$out"
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
        let _ = fs::remove_file(&archive);
        std::env::set_var("SQUALLZ_WIMLIB", &script);
        std::env::set_var("SQUALLZ_FAKE_WIMLIB_LOG", &log);

        let format = SevenZipBridgeFormat { spec: &SPECS[0] };
        let mut writer = format
            .create(
                Box::new(File::create(&archive).unwrap()),
                &CreateOptions::default(),
            )
            .unwrap();
        writer
            .add_entry(
                &EntryMeta {
                    path: EntryPath::from_utf8("project"),
                    entry_type: EntryType::Dir,
                    size: 0,
                    compressed_size: None,
                    modified: None,
                    unix_mode: None,
                    crc32: None,
                    encrypted: false,
                },
                None,
            )
            .unwrap();
        let mut a = io::Cursor::new(b"hello wim".to_vec());
        writer
            .add_entry(
                &EntryMeta {
                    path: EntryPath::from_utf8("project/a.txt"),
                    entry_type: EntryType::File,
                    size: 9,
                    compressed_size: None,
                    modified: None,
                    unix_mode: None,
                    crc32: None,
                    encrypted: false,
                },
                Some(&mut a),
            )
            .unwrap();
        let mut b = io::Cursor::new(b"nested wim".to_vec());
        writer
            .add_entry(
                &EntryMeta {
                    path: EntryPath::from_utf8("project/sub/b.txt"),
                    entry_type: EntryType::File,
                    size: 10,
                    compressed_size: None,
                    modified: None,
                    unix_mode: None,
                    crc32: None,
                    encrypted: false,
                },
                Some(&mut b),
            )
            .unwrap();
        writer.finish().unwrap();

        assert!(fs::read(&archive).unwrap().starts_with(b"MSWIM\0\0\0"));
        let log = fs::read_to_string(&log).unwrap();
        assert!(log.contains("capture"), "{log}");
        assert!(log.contains("--compress=LZX"), "{log}");

        let _ = fs::remove_file(script);
        let _ = fs::remove_file(log);
        let _ = fs::remove_file(archive);
    }
}
