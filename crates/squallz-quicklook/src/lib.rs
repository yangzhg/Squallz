//! Bounded, read-only archive summaries for constrained preview hosts.
//!
//! The macOS Finder extension links this crate as a static library. Archive
//! parsing stays in `squallz-formats`; this layer applies preview-specific I/O
//! budgets and renders self-contained HTML without spawning helpers.

use std::ffi::{c_char, CStr};
use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use squallz_format_api::{
    ArchiveReader, Detected, EntryMeta, EntryType, FormatError, OpenOptions, ReadSeek,
    StreamFactory,
};
use squallz_i18n::Localizer;

const SNIFF_HEAD_BYTES: usize = 512;
const SNIFF_TAIL_BYTES: usize = 64;
const DEFAULT_ENTRY_LIMIT: usize = 240;
const DEFAULT_INPUT_READ_BUDGET: u64 = 64 * 1024 * 1024;
const DEFAULT_DECODED_READ_BUDGET: u64 = 96 * 1024 * 1024;
const MAX_DISPLAY_CHARS: usize = 512;

#[derive(Clone, Copy, Debug)]
pub struct RenderOptions {
    pub entry_limit: usize,
    pub input_read_budget: u64,
    pub decoded_read_budget: u64,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            entry_limit: DEFAULT_ENTRY_LIMIT,
            input_read_budget: DEFAULT_INPUT_READ_BUDGET,
            decoded_read_budget: DEFAULT_DECODED_READ_BUDGET,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PreviewFailure {
    Password,
    Damaged,
    ReadLimit,
    Unsupported,
    Unavailable,
}

#[derive(Debug)]
struct PreviewEntry {
    name: String,
    kind: EntryKind,
    size: u64,
    packed: Option<u64>,
    encrypted: bool,
}

#[derive(Clone, Copy, Debug)]
enum EntryKind {
    File,
    Directory,
    Link,
    Other,
}

#[derive(Debug)]
struct ArchiveSnapshot {
    format: String,
    entries: Vec<PreviewEntry>,
    truncated: bool,
    partial: bool,
    total_size: u64,
    encrypted_entries: usize,
}

enum PreviewOpen {
    Archive {
        format: String,
        reader: Box<dyn ArchiveReader>,
    },
    CompressedStream {
        format: String,
        output_name: String,
        size_hint: Option<u64>,
    },
}

#[derive(Debug)]
struct ReadBudgetExceeded;

impl std::fmt::Display for ReadBudgetExceeded {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Quick Look read budget exceeded")
    }
}

impl std::error::Error for ReadBudgetExceeded {}

struct BudgetedReader<R> {
    inner: R,
    remaining: u64,
}

impl<R> BudgetedReader<R> {
    fn new(inner: R, budget: u64) -> Self {
        Self {
            inner,
            remaining: budget,
        }
    }
}

impl<R: Read> Read for BudgetedReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        if self.remaining == 0 {
            return Err(io::Error::other(ReadBudgetExceeded));
        }
        let allowed = buffer
            .len()
            .min(self.remaining.min(usize::MAX as u64) as usize);
        let read = self.inner.read(&mut buffer[..allowed])?;
        self.remaining = self.remaining.saturating_sub(read as u64);
        Ok(read)
    }
}

impl<R: Seek> Seek for BudgetedReader<R> {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.inner.seek(position)
    }
}

pub fn render_archive_preview(
    path: &Path,
    requested_language: Option<&str>,
    options: RenderOptions,
) -> Vec<u8> {
    let localizer = Localizer::with_user_dir(requested_language, None);
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| localizer.t("macos.quicklook.unnamed"));
    let archive_bytes = regular_file_size(path);

    match inspect_archive(path, options) {
        Ok(snapshot) => render_snapshot(&localizer, &file_name, archive_bytes, snapshot),
        Err(failure) => render_failure(
            &localizer,
            &file_name,
            archive_bytes,
            format_hint(path),
            failure,
        ),
    }
    .into_bytes()
}

fn regular_file_size(path: &Path) -> Option<u64> {
    fs::symlink_metadata(path)
        .ok()
        .filter(|metadata| metadata.is_file() && !metadata.file_type().is_symlink())
        .map(|metadata| metadata.len())
}

fn inspect_archive(path: &Path, options: RenderOptions) -> Result<ArchiveSnapshot, PreviewFailure> {
    let entry_limit = options.entry_limit.max(1);
    let opened = open_preview(path, options).map_err(classify_error)?;
    match opened {
        PreviewOpen::CompressedStream {
            format,
            output_name,
            size_hint,
        } => {
            let size = size_hint.unwrap_or(0);
            Ok(ArchiveSnapshot {
                format,
                entries: vec![PreviewEntry {
                    name: output_name,
                    kind: EntryKind::File,
                    size,
                    packed: regular_file_size(path),
                    encrypted: false,
                }],
                truncated: false,
                partial: false,
                total_size: size,
                encrypted_entries: 0,
            })
        }
        PreviewOpen::Archive { format, mut reader } => {
            let mut entries = Vec::with_capacity(entry_limit.min(256));
            let mut truncated = false;
            let mut partial = false;
            let mut iterator = reader.entries();
            while entries.len() < entry_limit {
                match iterator.next() {
                    Some(Ok(entry)) => entries.push(preview_entry(entry)),
                    Some(Err(error)) => {
                        if entries.is_empty() {
                            return Err(classify_error(error));
                        }
                        partial = true;
                        break;
                    }
                    None => break,
                }
            }
            if !partial && entries.len() == entry_limit {
                match iterator.next() {
                    Some(Ok(_)) => truncated = true,
                    Some(Err(_)) => partial = true,
                    None => {}
                }
            }
            let total_size = entries
                .iter()
                .fold(0u64, |total, entry| total.saturating_add(entry.size));
            let encrypted_entries = entries.iter().filter(|entry| entry.encrypted).count();
            Ok(ArchiveSnapshot {
                format,
                entries,
                truncated,
                partial,
                total_size,
                encrypted_entries,
            })
        }
    }
}

fn preview_entry(entry: EntryMeta) -> PreviewEntry {
    let kind = match entry.entry_type {
        EntryType::File => EntryKind::File,
        EntryType::Dir => EntryKind::Directory,
        EntryType::Symlink { .. } | EntryType::Hardlink { .. } => EntryKind::Link,
        EntryType::Other => EntryKind::Other,
    };
    PreviewEntry {
        name: bounded_display_text(&entry.path.display),
        kind,
        size: entry.size,
        packed: entry.compressed_size,
        encrypted: entry.encrypted,
    }
}

fn open_preview(path: &Path, options: RenderOptions) -> Result<PreviewOpen, FormatError> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(FormatError::Unsupported(
            "Quick Look source must be a regular non-symlink file".into(),
        ));
    }
    let mut file = File::open(path)?;
    if !file.metadata()?.is_file() {
        return Err(FormatError::Unsupported(
            "Quick Look source is not a regular file".into(),
        ));
    }
    let (head, tail) = sniff_windows(&mut file)?;
    let registry = squallz_formats::embedded_preview_registry();
    let name = path.file_name().and_then(|name| name.to_str());
    let detected = registry.detect(name, &head, &tail).ok_or_else(|| {
        FormatError::Unsupported("format has no in-process Quick Look reader".into())
    })?;

    match detected {
        Detected::Archive(format) => {
            file.seek(SeekFrom::Start(0))?;
            let format_id = format.id().to_owned();
            let source: Box<dyn ReadSeek> =
                Box::new(BudgetedReader::new(file, options.input_read_budget));
            let reader = format.open(source, &OpenOptions::default())?;
            Ok(PreviewOpen::Archive {
                format: format_id,
                reader,
            })
        }
        Detected::Compressed {
            compressor,
            inner_archive: Some(inner_archive),
        } => {
            let format = format!("{}.{}", inner_archive.id(), compressor.id());
            let source_path = path.to_path_buf();
            let input_budget = options.input_read_budget;
            let decoded_budget = options.decoded_read_budget;
            let factory: StreamFactory = Box::new(move || {
                let file = File::open(&source_path)?;
                let compressed = BudgetedReader::new(file, input_budget);
                let decoded = compressor.decompress_reader(Box::new(compressed))?;
                Ok(Box::new(BudgetedReader::new(decoded, decoded_budget)))
            });
            let reader = inner_archive.open_stream(factory, &OpenOptions::default())?;
            Ok(PreviewOpen::Archive { format, reader })
        }
        Detected::Compressed {
            compressor,
            inner_archive: None,
        } => {
            let size_hint = compressor.uncompressed_size_hint(&mut file);
            let output_name = stream_output_name(path, compressor.extensions());
            Ok(PreviewOpen::CompressedStream {
                format: compressor.id().to_owned(),
                output_name,
                size_hint,
            })
        }
    }
}

fn sniff_windows(file: &mut File) -> io::Result<(Vec<u8>, Vec<u8>)> {
    file.seek(SeekFrom::Start(0))?;
    let mut head = vec![0u8; SNIFF_HEAD_BYTES];
    let head_len = read_up_to(file, &mut head)?;
    head.truncate(head_len);

    let length = file.seek(SeekFrom::End(0))?;
    let tail_start = length.saturating_sub(SNIFF_TAIL_BYTES as u64);
    file.seek(SeekFrom::Start(tail_start))?;
    let mut tail = vec![0u8; SNIFF_TAIL_BYTES];
    let tail_len = read_up_to(file, &mut tail)?;
    tail.truncate(tail_len);
    file.seek(SeekFrom::Start(0))?;
    Ok((head, tail))
}

fn read_up_to(reader: &mut dyn Read, buffer: &mut [u8]) -> io::Result<usize> {
    let mut filled = 0;
    while filled < buffer.len() {
        match reader.read(&mut buffer[filled..])? {
            0 => break,
            read => filled += read,
        }
    }
    Ok(filled)
}

fn stream_output_name(path: &Path, extensions: &[&str]) -> String {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let lower = name.to_lowercase();
    for extension in extensions {
        let suffix = format!(".{extension}");
        if lower.ends_with(&suffix) && name.len() > suffix.len() {
            return bounded_display_text(&name[..name.len() - suffix.len()]);
        }
    }
    bounded_display_text(&name)
}

fn classify_error(error: FormatError) -> PreviewFailure {
    match error {
        FormatError::PasswordRequired | FormatError::WrongPassword => PreviewFailure::Password,
        FormatError::CorruptArchive(_)
        | FormatError::PathTraversal(_)
        | FormatError::SymlinkBreakout(_)
        | FormatError::UnsafeFileName(_) => PreviewFailure::Damaged,
        FormatError::ResourceLimitExceeded(_) => PreviewFailure::ReadLimit,
        FormatError::Io(error) if is_read_budget_error(&error) => PreviewFailure::ReadLimit,
        FormatError::Unsupported(_) | FormatError::DependencyMissing(_) => {
            PreviewFailure::Unsupported
        }
        FormatError::Io(_)
        | FormatError::Cancelled
        | FormatError::DiskFull
        | FormatError::Other(_) => PreviewFailure::Unavailable,
    }
}

fn is_read_budget_error(error: &io::Error) -> bool {
    error
        .get_ref()
        .and_then(|source| source.downcast_ref::<ReadBudgetExceeded>())
        .is_some()
}

fn format_hint(path: &Path) -> String {
    let lower = path
        .file_name()
        .map(|name| name.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let mappings = [
        (".tar.gz", "TAR.GZ"),
        (".tar.bz2", "TAR.BZ2"),
        (".tar.xz", "TAR.XZ"),
        (".tar.zst", "TAR.ZST"),
        (".tbz2", "TAR.BZ2"),
        (".tzst", "TAR.ZST"),
        (".tgz", "TAR.GZ"),
        (".txz", "TAR.XZ"),
        (".zip", "ZIP"),
        (".jar", "JAR"),
        (".apk", "APK"),
        (".ipa", "IPA"),
        (".cbz", "CBZ"),
        (".7z", "7Z"),
        (".rar", "RAR"),
        (".cbr", "CBR"),
        (".sqz", "SQZ"),
        (".tar", "TAR"),
        (".bz2", "BZIP2"),
        (".zst", "ZSTD"),
        (".lz4", "LZ4"),
        (".gz", "GZIP"),
        (".xz", "XZ"),
        (".br", "BROTLI"),
    ];
    mappings
        .iter()
        .find_map(|(suffix, label)| lower.ends_with(suffix).then_some((*label).to_owned()))
        .unwrap_or_else(|| "ARCHIVE".to_owned())
}

fn display_format(format: &str) -> String {
    match format {
        "tar.gzip" => "TAR.GZ".to_owned(),
        "tar.bzip2" => "TAR.BZ2".to_owned(),
        "tar.xz" => "TAR.XZ".to_owned(),
        "tar.zstd" => "TAR.ZST".to_owned(),
        "gzip" => "GZIP".to_owned(),
        "bzip2" => "BZIP2".to_owned(),
        "zstd" => "ZSTD".to_owned(),
        "brotli" => "BROTLI".to_owned(),
        other => other.to_uppercase(),
    }
}

fn render_snapshot(
    localizer: &Localizer,
    file_name: &str,
    archive_bytes: Option<u64>,
    snapshot: ArchiveSnapshot,
) -> String {
    let count = snapshot.entries.len().to_string();
    let item_value = if snapshot.truncated || snapshot.partial {
        localizer.format("macos.quicklook.items_more", &[("count", &count)])
    } else {
        count.clone()
    };
    let mut html = html_start(localizer, file_name, &display_format(&snapshot.format));
    push_stats(
        &mut html,
        localizer,
        &item_value,
        archive_bytes,
        snapshot.total_size,
        snapshot.truncated || snapshot.partial,
    );

    html.push_str("<section class=\"contents\"><div class=\"section-title\"><h2>");
    html.push_str(&escape_html(&localizer.t("macos.quicklook.contents")));
    if snapshot.encrypted_entries > 0 {
        let encrypted = snapshot.encrypted_entries.to_string();
        html.push_str("</h2><span class=\"badge\">");
        html.push_str(&escape_html(
            &localizer.format("macos.quicklook.encrypted_count", &[("count", &encrypted)]),
        ));
        html.push_str("</span>");
    } else {
        html.push_str("</h2>");
    }
    html.push_str("</div>");

    if snapshot.entries.is_empty() {
        html.push_str("<div class=\"empty\">");
        html.push_str(&escape_html(&localizer.t("macos.quicklook.empty")));
        html.push_str("</div>");
    } else {
        html.push_str("<div class=\"table-wrap\"><table><thead><tr><th scope=\"col\">");
        html.push_str(&escape_html(&localizer.t("macos.quicklook.name")));
        html.push_str("</th><th scope=\"col\">");
        html.push_str(&escape_html(&localizer.t("macos.quicklook.kind")));
        html.push_str("</th><th scope=\"col\" class=\"numeric\">");
        html.push_str(&escape_html(&localizer.t("macos.quicklook.size")));
        html.push_str("</th><th scope=\"col\" class=\"numeric\">");
        html.push_str(&escape_html(&localizer.t("macos.quicklook.packed")));
        html.push_str("</th></tr></thead><tbody>");
        for entry in &snapshot.entries {
            push_entry_row(&mut html, localizer, entry);
        }
        html.push_str("</tbody></table></div>");
    }

    if snapshot.partial {
        push_notice(
            &mut html,
            &localizer.t("macos.quicklook.partial"),
            "warning",
        );
    } else if snapshot.truncated {
        push_notice(
            &mut html,
            &localizer.format("macos.quicklook.truncated", &[("count", &count)]),
            "neutral",
        );
    }
    html.push_str("</section>");
    html_end(&mut html, localizer);
    html
}

fn render_failure(
    localizer: &Localizer,
    file_name: &str,
    archive_bytes: Option<u64>,
    format: String,
    failure: PreviewFailure,
) -> String {
    let message_key = match failure {
        PreviewFailure::Password => "macos.quicklook.password",
        PreviewFailure::Damaged => "macos.quicklook.damaged",
        PreviewFailure::ReadLimit => "macos.quicklook.read_limit",
        PreviewFailure::Unsupported => "macos.quicklook.unsupported",
        PreviewFailure::Unavailable => "macos.quicklook.unavailable",
    };
    let mut html = html_start(localizer, file_name, &format);
    push_basic_stats(&mut html, localizer, archive_bytes);
    html.push_str("<section class=\"failure\"><div class=\"failure-mark\">!</div><h2>");
    html.push_str(&escape_html(&localizer.t(message_key)));
    html.push_str("</h2><p>");
    html.push_str(&escape_html(
        &localizer.t("macos.quicklook.open_in_squallz"),
    ));
    html.push_str("</p></section>");
    html_end(&mut html, localizer);
    html
}

fn html_start(localizer: &Localizer, file_name: &str, format: &str) -> String {
    let mut html = String::with_capacity(24 * 1024);
    html.push_str("<!doctype html><html lang=\"");
    html.push_str(&escape_html(localizer.language()));
    html.push_str(
        "\"><head><meta charset=\"utf-8\"><meta name=\"color-scheme\" content=\"light dark\">",
    );
    html.push_str("<meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">");
    html.push_str("<title>");
    html.push_str(&escape_html(file_name));
    html.push_str("</title><style>");
    html.push_str(PREVIEW_CSS);
    html.push_str("</style></head><body><main><header class=\"hero\"><div class=\"archive-mark\" aria-hidden=\"true\"><i></i><i></i><i></i></div><div class=\"hero-copy\"><span class=\"eyebrow\">");
    html.push_str(&escape_html(
        &localizer.t("macos.quicklook.archive_preview"),
    ));
    html.push_str("</span><h1>");
    html.push_str(&escape_html(&bounded_display_text(file_name)));
    html.push_str("</h1><span class=\"format-pill\">");
    html.push_str(&escape_html(format));
    html.push_str("</span></div></header>");
    html
}

fn push_stats(
    html: &mut String,
    localizer: &Localizer,
    item_value: &str,
    archive_bytes: Option<u64>,
    content_bytes: u64,
    content_is_partial: bool,
) {
    html.push_str("<section class=\"stats\" aria-label=\"");
    html.push_str(&escape_html(&localizer.t("macos.quicklook.summary")));
    html.push_str("\">");
    push_stat(html, &localizer.t("macos.quicklook.items"), item_value);
    push_stat(
        html,
        &localizer.t("macos.quicklook.archive_size"),
        &archive_bytes
            .map(format_bytes)
            .unwrap_or_else(|| localizer.t("macos.quicklook.unknown")),
    );
    let content_value = if content_is_partial {
        let value = format_bytes(content_bytes);
        localizer.format("macos.quicklook.size_more", &[("size", &value)])
    } else {
        format_bytes(content_bytes)
    };
    push_stat(
        html,
        &localizer.t("macos.quicklook.content_size"),
        &content_value,
    );
    html.push_str("</section>");
}

fn push_basic_stats(html: &mut String, localizer: &Localizer, archive_bytes: Option<u64>) {
    html.push_str("<section class=\"stats compact\" aria-label=\"");
    html.push_str(&escape_html(&localizer.t("macos.quicklook.summary")));
    html.push_str("\">");
    push_stat(
        html,
        &localizer.t("macos.quicklook.archive_size"),
        &archive_bytes
            .map(format_bytes)
            .unwrap_or_else(|| localizer.t("macos.quicklook.unknown")),
    );
    html.push_str("</section>");
}

fn push_stat(html: &mut String, label: &str, value: &str) {
    html.push_str("<div class=\"stat\"><span>");
    html.push_str(&escape_html(label));
    html.push_str("</span><strong>");
    html.push_str(&escape_html(value));
    html.push_str("</strong></div>");
}

fn push_entry_row(html: &mut String, localizer: &Localizer, entry: &PreviewEntry) {
    let (kind_key, kind_class) = match entry.kind {
        EntryKind::File => ("macos.quicklook.type_file", "file"),
        EntryKind::Directory => ("macos.quicklook.type_folder", "folder"),
        EntryKind::Link => ("macos.quicklook.type_link", "link"),
        EntryKind::Other => ("macos.quicklook.type_other", "other"),
    };
    html.push_str("<tr><td><div class=\"entry\"><span class=\"entry-icon ");
    html.push_str(kind_class);
    html.push_str("\" aria-hidden=\"true\"></span><span>");
    html.push_str(&escape_html(&entry.name));
    if entry.encrypted {
        html.push_str("<span class=\"lock\" aria-label=\"");
        html.push_str(&escape_html(&localizer.t("macos.quicklook.encrypted")));
        html.push_str("\">◆</span>");
    }
    html.push_str("</span></div></td><td>");
    html.push_str(&escape_html(&localizer.t(kind_key)));
    html.push_str("</td><td class=\"numeric\">");
    html.push_str(&escape_html(&format_bytes(entry.size)));
    html.push_str("</td><td class=\"numeric\">");
    match entry.packed {
        Some(size) => html.push_str(&escape_html(&format_bytes(size))),
        None => html.push('—'),
    }
    html.push_str("</td></tr>");
}

fn push_notice(html: &mut String, message: &str, tone: &str) {
    html.push_str("<div class=\"notice ");
    html.push_str(tone);
    html.push_str("\"><span aria-hidden=\"true\">");
    html.push_str(if tone == "warning" { "!" } else { "…" });
    html.push_str("</span><p>");
    html.push_str(&escape_html(message));
    html.push_str("</p></div>");
}

fn html_end(html: &mut String, localizer: &Localizer) {
    html.push_str("<footer><span class=\"wordmark\">Squallz</span><span>");
    html.push_str(&escape_html(&localizer.t("macos.quicklook.finder_preview")));
    html.push_str("</span></footer></main></body></html>");
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    if bytes < 1000 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64;
    let mut unit = 0usize;
    while value >= 1000.0 && unit + 1 < UNITS.len() {
        value /= 1000.0;
        unit += 1;
    }
    if value >= 100.0 {
        format!("{value:.0} {}", UNITS[unit])
    } else if value >= 10.0 {
        format!("{value:.1} {}", UNITS[unit])
    } else {
        format!("{value:.2} {}", UNITS[unit])
    }
}

fn bounded_display_text(value: &str) -> String {
    let mut output = String::with_capacity(value.len().min(MAX_DISPLAY_CHARS * 2));
    for (count, character) in value.chars().enumerate() {
        if count == MAX_DISPLAY_CHARS {
            output.push('…');
            break;
        }
        if character.is_control() {
            output.push('�');
        } else {
            output.push(character);
        }
    }
    output
}

fn escape_html(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            _ => escaped.push(character),
        }
    }
    escaped
}

fn path_from_ffi(bytes: &[u8]) -> Option<PathBuf> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        Some(PathBuf::from(std::ffi::OsStr::from_bytes(bytes)))
    }
    #[cfg(not(unix))]
    {
        std::str::from_utf8(bytes).ok().map(PathBuf::from)
    }
}

/// Renders a Quick Look document and transfers ownership of the returned byte
/// buffer to the caller.
///
/// # Safety
///
/// The caller must pass a valid NUL-terminated file-system path and a valid
/// writable `output_len`, then release a non-null result with
/// [`squallz_quicklook_free`].
#[no_mangle]
pub unsafe extern "C" fn squallz_quicklook_render(
    path: *const c_char,
    language: *const c_char,
    output_len: *mut usize,
) -> *mut u8 {
    if output_len.is_null() {
        return std::ptr::null_mut();
    }
    // SAFETY: `output_len` is required by the function contract to be writable.
    unsafe {
        output_len.write(0);
    }
    if path.is_null() {
        return std::ptr::null_mut();
    }

    let rendered = std::panic::catch_unwind(|| {
        // SAFETY: `path` is required by the function contract to point to a
        // NUL-terminated byte sequence for the duration of this call.
        let path_bytes = unsafe { CStr::from_ptr(path) }.to_bytes();
        let path = path_from_ffi(path_bytes)?;
        let language = if language.is_null() {
            None
        } else {
            // SAFETY: a non-null `language` follows the same C string contract.
            unsafe { CStr::from_ptr(language) }.to_str().ok()
        };
        Some(render_archive_preview(
            &path,
            language,
            RenderOptions::default(),
        ))
    });

    let bytes = match rendered {
        Ok(Some(bytes)) if !bytes.is_empty() => bytes,
        _ => return std::ptr::null_mut(),
    };
    let boxed = bytes.into_boxed_slice();
    let length = boxed.len();
    let pointer = Box::into_raw(boxed) as *mut u8;
    // SAFETY: `output_len` was validated above and remains owned by the caller.
    unsafe {
        output_len.write(length);
    }
    pointer
}

/// Releases a buffer returned by [`squallz_quicklook_render`].
///
/// # Safety
///
/// `pointer` and `length` must be the unchanged values returned by the render
/// call. Passing a null pointer is a no-op.
#[no_mangle]
pub unsafe extern "C" fn squallz_quicklook_free(pointer: *mut u8, length: usize) {
    if pointer.is_null() {
        return;
    }
    let slice = std::ptr::slice_from_raw_parts_mut(pointer, length);
    // SAFETY: the function contract requires the exact pointer and length
    // produced by `Box<[u8]>::into_raw` in `squallz_quicklook_render`.
    unsafe {
        drop(Box::from_raw(slice));
    }
}

const PREVIEW_CSS: &str = r#"
:root {
  color-scheme: light dark;
  --bg: #f5f6fa;
  --surface: rgba(255, 255, 255, .82);
  --surface-strong: #fff;
  --text: #172033;
  --muted: #697286;
  --line: rgba(23, 32, 51, .09);
  --accent: #635bff;
  --accent-soft: rgba(99, 91, 255, .12);
  --teal: #11a89d;
  --warning: #c97718;
  --shadow: 0 20px 54px rgba(25, 31, 50, .12);
}
* { box-sizing: border-box; }
html, body { min-height: 100%; margin: 0; }
body {
  background:
    radial-gradient(circle at 84% 4%, rgba(99, 91, 255, .16), transparent 34%),
    radial-gradient(circle at 8% 18%, rgba(17, 168, 157, .10), transparent 30%),
    var(--bg);
  color: var(--text);
  font: 14px/1.45 -apple-system, BlinkMacSystemFont, "SF Pro Text", "Helvetica Neue", sans-serif;
  padding: 28px;
}
main {
  max-width: 980px;
  margin: 0 auto;
  overflow: hidden;
  border: 1px solid var(--line);
  border-radius: 24px;
  background: var(--surface);
  box-shadow: var(--shadow);
  backdrop-filter: blur(28px) saturate(1.15);
}
.hero {
  display: flex;
  align-items: center;
  gap: 20px;
  padding: 27px 30px 23px;
  border-bottom: 1px solid var(--line);
}
.archive-mark {
  position: relative;
  flex: 0 0 58px;
  width: 58px;
  height: 66px;
  border-radius: 17px;
  background: linear-gradient(145deg, var(--accent), #827cff 52%, var(--teal));
  box-shadow: 0 12px 28px rgba(99, 91, 255, .25);
}
.archive-mark::before {
  content: "";
  position: absolute;
  inset: 9px;
  border: 1px solid rgba(255,255,255,.38);
  border-radius: 11px;
}
.archive-mark i {
  position: absolute;
  left: 24px;
  width: 10px;
  height: 7px;
  border: 2px solid #fff;
  border-radius: 3px;
}
.archive-mark i:nth-child(1) { top: 14px; }
.archive-mark i:nth-child(2) { top: 29px; }
.archive-mark i:nth-child(3) { top: 44px; }
.hero-copy { min-width: 0; }
.eyebrow {
  display: block;
  color: var(--muted);
  font-size: 11px;
  font-weight: 700;
  letter-spacing: .12em;
  text-transform: uppercase;
}
h1 {
  max-width: 760px;
  margin: 3px 0 8px;
  overflow: hidden;
  font-size: 26px;
  line-height: 1.16;
  letter-spacing: -.025em;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.format-pill, .badge {
  display: inline-flex;
  align-items: center;
  min-height: 24px;
  padding: 3px 9px;
  border: 1px solid rgba(99, 91, 255, .18);
  border-radius: 999px;
  background: var(--accent-soft);
  color: var(--accent);
  font-size: 11px;
  font-weight: 750;
  letter-spacing: .04em;
}
.stats {
  display: grid;
  grid-template-columns: repeat(3, minmax(0, 1fr));
  gap: 1px;
  margin: 0;
  border-bottom: 1px solid var(--line);
  background: var(--line);
}
.stats.compact { grid-template-columns: minmax(0, 1fr); }
.stat {
  min-width: 0;
  padding: 17px 30px;
  background: var(--surface-strong);
}
.stat span {
  display: block;
  color: var(--muted);
  font-size: 11px;
  font-weight: 650;
}
.stat strong {
  display: block;
  margin-top: 3px;
  overflow: hidden;
  font-size: 17px;
  letter-spacing: -.01em;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.contents { padding: 23px 24px 24px; }
.section-title {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 16px;
  margin: 0 6px 12px;
}
h2 { margin: 0; font-size: 15px; letter-spacing: -.01em; }
.table-wrap {
  overflow: hidden;
  border: 1px solid var(--line);
  border-radius: 15px;
  background: var(--surface-strong);
}
table { width: 100%; border-collapse: collapse; table-layout: fixed; }
th {
  padding: 10px 14px;
  border-bottom: 1px solid var(--line);
  color: var(--muted);
  font-size: 10px;
  font-weight: 700;
  letter-spacing: .07em;
  text-align: left;
  text-transform: uppercase;
}
th:first-child { width: 58%; }
th:nth-child(2) { width: 15%; }
th:nth-child(3), th:nth-child(4) { width: 13.5%; }
td {
  padding: 9px 14px;
  border-bottom: 1px solid var(--line);
  color: var(--muted);
  font-size: 12px;
  vertical-align: middle;
}
tr:last-child td { border-bottom: 0; }
tbody tr:nth-child(even) { background: rgba(99, 91, 255, .018); }
.numeric {
  font-variant-numeric: tabular-nums;
  text-align: right;
  white-space: nowrap;
}
.entry {
  display: flex;
  align-items: center;
  min-width: 0;
  gap: 9px;
  color: var(--text);
}
.entry > span:last-child {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.entry-icon {
  position: relative;
  flex: 0 0 16px;
  width: 16px;
  height: 18px;
  border: 1.5px solid currentColor;
  border-radius: 4px;
  color: var(--accent);
  opacity: .86;
}
.entry-icon.folder {
  height: 13px;
  border-radius: 3px;
  color: var(--teal);
}
.entry-icon.folder::before {
  content: "";
  position: absolute;
  left: -1.5px;
  top: -5px;
  width: 8px;
  height: 5px;
  border: 1.5px solid currentColor;
  border-bottom: 0;
  border-radius: 3px 3px 0 0;
}
.entry-icon.link { border-radius: 999px; color: var(--warning); }
.entry-icon.other { border-style: dashed; color: var(--muted); }
.lock {
  margin-left: 7px;
  color: var(--warning);
  font-size: 8px;
  vertical-align: 1px;
}
.notice {
  display: flex;
  align-items: center;
  gap: 9px;
  margin-top: 12px;
  padding: 11px 13px;
  border: 1px solid var(--line);
  border-radius: 12px;
  background: rgba(105, 114, 134, .06);
  color: var(--muted);
}
.notice.warning {
  border-color: rgba(201, 119, 24, .20);
  background: rgba(201, 119, 24, .08);
  color: var(--warning);
}
.notice > span {
  display: grid;
  flex: 0 0 22px;
  width: 22px;
  height: 22px;
  place-items: center;
  border-radius: 7px;
  background: currentColor;
  color: var(--surface-strong);
  font-weight: 800;
}
.notice p { margin: 0; color: var(--text); font-size: 12px; }
.empty {
  padding: 54px 24px;
  border: 1px dashed var(--line);
  border-radius: 15px;
  color: var(--muted);
  text-align: center;
}
.failure {
  display: grid;
  justify-items: center;
  padding: 58px 32px 64px;
  text-align: center;
}
.failure-mark {
  display: grid;
  width: 46px;
  height: 46px;
  margin-bottom: 15px;
  place-items: center;
  border-radius: 15px;
  background: var(--accent-soft);
  color: var(--accent);
  font-size: 20px;
  font-weight: 800;
}
.failure h2 { font-size: 18px; }
.failure p { max-width: 520px; margin: 8px 0 0; color: var(--muted); }
footer {
  display: flex;
  justify-content: space-between;
  gap: 16px;
  padding: 14px 30px 17px;
  border-top: 1px solid var(--line);
  color: var(--muted);
  font-size: 10px;
}
.wordmark { color: var(--text); font-weight: 800; letter-spacing: .02em; }
@media (prefers-color-scheme: dark) {
  :root {
    --bg: #11141d;
    --surface: rgba(25, 29, 41, .86);
    --surface-strong: #1b202c;
    --text: #f2f4fa;
    --muted: #9da6ba;
    --line: rgba(255, 255, 255, .085);
    --accent: #aaa5ff;
    --accent-soft: rgba(132, 124, 255, .16);
    --teal: #51d6c9;
    --warning: #f0ad58;
    --shadow: 0 22px 60px rgba(0, 0, 0, .34);
  }
}
@media (max-width: 680px) {
  body { padding: 14px; }
  main { border-radius: 19px; }
  .hero { padding: 21px 20px 18px; }
  .archive-mark { flex-basis: 48px; width: 48px; height: 56px; border-radius: 14px; }
  .archive-mark i { left: 19px; }
  .archive-mark i:nth-child(1) { top: 10px; }
  .archive-mark i:nth-child(2) { top: 24px; }
  .archive-mark i:nth-child(3) { top: 38px; }
  h1 { font-size: 21px; }
  .stats { grid-template-columns: 1fr; }
  .stat { padding: 12px 20px; }
  .contents { padding: 18px 14px; }
  th:nth-child(2), td:nth-child(2), th:nth-child(4), td:nth-child(4) { display: none; }
  th:first-child { width: 72%; }
  th:nth-child(3) { width: 28%; }
  footer { padding: 12px 18px 14px; }
}
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    fn write_zip(path: &Path, names: &[&str]) {
        let file = File::create(path).unwrap();
        let mut archive = zip::ZipWriter::new(file);
        for name in names {
            archive
                .start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            archive.write_all(b"preview payload").unwrap();
        }
        archive.finish().unwrap();
    }

    #[test]
    fn renderer_escapes_names_and_never_emits_active_content() {
        let localizer = Localizer::with_user_dir(Some("en-US"), None);
        let snapshot = ArchiveSnapshot {
            format: "zip".into(),
            entries: vec![PreviewEntry {
                name: "<script>alert('x')</script>.txt".into(),
                kind: EntryKind::File,
                size: 10,
                packed: Some(8),
                encrypted: false,
            }],
            truncated: false,
            partial: false,
            total_size: 10,
            encrypted_entries: 0,
        };

        let html = render_snapshot(&localizer, "bad<&>.zip", Some(42), snapshot);

        assert!(html.contains("bad&lt;&amp;&gt;.zip"));
        assert!(html.contains("&lt;script&gt;alert(&#39;x&#39;)&lt;/script&gt;.txt"));
        assert!(!html.contains("<script"));
        assert!(!html.contains("javascript:"));
    }

    #[test]
    fn detailed_preview_is_bounded_and_localized() {
        let directory = tempfile::tempdir().unwrap();
        let archive = directory.path().join("样例.zip");
        write_zip(&archive, &["one.txt", "two.txt", "three.txt"]);

        let html = String::from_utf8(render_archive_preview(
            &archive,
            Some("zh-CN"),
            RenderOptions {
                entry_limit: 2,
                ..RenderOptions::default()
            },
        ))
        .unwrap();

        assert!(html.contains("one.txt"));
        assert!(html.contains("two.txt"));
        assert!(!html.contains("three.txt"));
        assert!(html.contains("仅显示前 2 项"));
        assert!(html.contains("压缩包预览"));
    }

    #[test]
    fn read_budget_failure_stays_friendly_and_does_not_leak_internal_errors() {
        let directory = tempfile::tempdir().unwrap();
        let archive = directory.path().join("budget.zip");
        write_zip(&archive, &["one.txt", "two.txt"]);

        let html = String::from_utf8(render_archive_preview(
            &archive,
            Some("en-US"),
            RenderOptions {
                input_read_budget: 1,
                ..RenderOptions::default()
            },
        ))
        .unwrap();

        assert!(html.contains("safety budget"));
        assert!(!html.contains("Quick Look read budget exceeded"));
        assert!(!html.contains(&archive.to_string_lossy().to_string()));
    }

    #[test]
    fn long_and_control_character_names_are_bounded() {
        let value = format!("{}\nsecret", "a".repeat(MAX_DISPLAY_CHARS + 20));
        let bounded = bounded_display_text(&value);

        assert_eq!(bounded.chars().count(), MAX_DISPLAY_CHARS + 1);
        assert!(bounded.ends_with('…'));
        assert!(!bounded.contains('\n'));
    }

    #[test]
    fn ffi_buffer_can_be_released_with_the_reported_length() {
        let directory = tempfile::tempdir().unwrap();
        let archive = directory.path().join("ffi.zip");
        write_zip(&archive, &["entry.txt"]);
        let path = std::ffi::CString::new(archive.as_os_str().as_encoded_bytes()).unwrap();
        let language = std::ffi::CString::new("en-US").unwrap();
        let mut length = 0usize;

        // SAFETY: the test supplies valid C strings and releases the exact
        // pointer-length pair returned by the FFI boundary.
        let pointer =
            unsafe { squallz_quicklook_render(path.as_ptr(), language.as_ptr(), &mut length) };
        assert!(!pointer.is_null());
        assert!(length > 100);
        // SAFETY: the pointer and length are unchanged from the render call.
        unsafe {
            squallz_quicklook_free(pointer, length);
        }
    }
}
