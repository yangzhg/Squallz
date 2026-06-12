//! Reproducible GUI browse benchmark for large archive lists.
//!
//! This intentionally exercises `AppState::open_archive` and
//! `AppState::list_entries`, which are the backend pieces behind the GUI
//! virtual list. It does not claim visual regression coverage.

use std::env;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use squallz_gui::state::{AppState, DEFAULT_PAGE_SIZE};

const DEFAULT_ENTRIES: usize = 100_000;
const DEFAULT_MAX_FIRST_SCREEN_MS: u128 = 1_000;
const DEFAULT_MAX_PAGE_MS: u128 = 50;
const DEFAULT_MAX_FILTER_MS: u128 = 250;
const LARGE_DIR: &str = "files/";

#[derive(Debug)]
struct Args {
    entries: usize,
    archive: PathBuf,
    report: PathBuf,
    max_first_screen_ms: u128,
    max_page_ms: u128,
    max_filter_ms: u128,
}

#[derive(Debug)]
struct Timings {
    generate: Duration,
    open: Duration,
    root_page: Duration,
    first_page: Duration,
    middle_page: Duration,
    last_page: Duration,
    filter: Duration,
}

fn main() {
    if let Err(e) = run() {
        eprintln!("gui-browse-bench: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args = Args::parse()?;
    if args.entries == 0 {
        return Err("--entries must be greater than 0".to_owned());
    }
    if let Some(parent) = args.archive.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("create archive dir: {e}"))?;
    }
    if let Some(parent) = args.report.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("create report dir: {e}"))?;
    }

    let t0 = Instant::now();
    write_large_zip(&args.archive, args.entries)?;
    let generate = t0.elapsed();

    let state = AppState::new();
    let t0 = Instant::now();
    let info = state
        .open_archive(&args.archive, None, None)
        .map_err(|e| format!("open archive: {e}"))?;
    let open = t0.elapsed();

    let t0 = Instant::now();
    let root = state
        .list_entries(info.id, 0, DEFAULT_PAGE_SIZE, "", None)
        .map_err(|e| format!("list root page: {e}"))?;
    let root_page = t0.elapsed();

    let t0 = Instant::now();
    let first = state
        .list_entries(info.id, 0, DEFAULT_PAGE_SIZE, LARGE_DIR, None)
        .map_err(|e| format!("list first page: {e}"))?;
    let first_page = t0.elapsed();

    let middle_page_no = (args.entries / 2) / DEFAULT_PAGE_SIZE;
    let t0 = Instant::now();
    let middle = state
        .list_entries(info.id, middle_page_no, DEFAULT_PAGE_SIZE, LARGE_DIR, None)
        .map_err(|e| format!("list middle page: {e}"))?;
    let middle_page = t0.elapsed();

    let last_page_no = (args.entries - 1) / DEFAULT_PAGE_SIZE;
    let t0 = Instant::now();
    let last = state
        .list_entries(info.id, last_page_no, DEFAULT_PAGE_SIZE, LARGE_DIR, None)
        .map_err(|e| format!("list last page: {e}"))?;
    let last_page = t0.elapsed();

    let needle = format!("file_{:06}.txt", args.entries - 1);
    let t0 = Instant::now();
    let filtered = state
        .list_entries(info.id, 0, DEFAULT_PAGE_SIZE, LARGE_DIR, Some(&needle))
        .map_err(|e| format!("filter page: {e}"))?;
    let filter = t0.elapsed();

    if info.entry_count != args.entries {
        return Err(format!(
            "entry count mismatch: expected {}, got {}",
            args.entries, info.entry_count
        ));
    }
    if root.total != 1 || root.items.first().map(|e| e.path.as_str()) != Some(LARGE_DIR) {
        return Err(format!(
            "root page mismatch: total={}, first={:?}",
            root.total,
            root.items.first()
        ));
    }
    if first.total != args.entries || first.items.len() != DEFAULT_PAGE_SIZE.min(args.entries) {
        return Err(format!(
            "first page mismatch: total={}, len={}",
            first.total,
            first.items.len()
        ));
    }
    if middle.items.is_empty() || last.items.is_empty() || filtered.total != 1 {
        return Err("middle/last/filter page sanity check failed".to_owned());
    }

    let timings = Timings {
        generate,
        open,
        root_page,
        first_page,
        middle_page,
        last_page,
        filter,
    };
    let report = render_report(
        &args,
        &timings,
        args.archive.metadata().ok().map(|m| m.len()),
    );
    fs::write(&args.report, report).map_err(|e| format!("write report: {e}"))?;

    let failures = threshold_failures(&args, &timings);
    println!("archive={}", args.archive.display());
    println!("report={}", args.report.display());
    println!("entries={}", args.entries);
    println!("open_ms={}", ms(timings.open));
    println!("first_screen_ms={}", ms(timings.first_screen()));
    println!("first_page_ms={}", ms(timings.first_page));
    println!("filter_ms={}", ms(timings.filter));
    if !failures.is_empty() {
        return Err(format!("threshold failure: {}", failures.join("; ")));
    }
    Ok(())
}

impl Args {
    fn parse() -> Result<Self, String> {
        let mut entries = DEFAULT_ENTRIES;
        let mut archive: Option<PathBuf> = None;
        let mut report: Option<PathBuf> = None;
        let mut max_first_screen_ms = DEFAULT_MAX_FIRST_SCREEN_MS;
        let mut max_page_ms = DEFAULT_MAX_PAGE_MS;
        let mut max_filter_ms = DEFAULT_MAX_FILTER_MS;
        let mut it = env::args().skip(1);
        while let Some(arg) = it.next() {
            match arg.as_str() {
                "--entries" => {
                    let value = it
                        .next()
                        .ok_or_else(|| "--entries requires a value".to_owned())?;
                    entries = value
                        .parse()
                        .map_err(|_| format!("invalid --entries value: {value}"))?;
                }
                "--archive" => {
                    archive = Some(PathBuf::from(
                        it.next()
                            .ok_or_else(|| "--archive requires a value".to_owned())?,
                    ));
                }
                "--report" => {
                    report = Some(PathBuf::from(
                        it.next()
                            .ok_or_else(|| "--report requires a value".to_owned())?,
                    ));
                }
                "--max-first-screen-ms" => {
                    let value = it
                        .next()
                        .ok_or_else(|| "--max-first-screen-ms requires a value".to_owned())?;
                    max_first_screen_ms = parse_ms("--max-first-screen-ms", &value)?;
                }
                "--max-page-ms" => {
                    let value = it
                        .next()
                        .ok_or_else(|| "--max-page-ms requires a value".to_owned())?;
                    max_page_ms = parse_ms("--max-page-ms", &value)?;
                }
                "--max-filter-ms" => {
                    let value = it
                        .next()
                        .ok_or_else(|| "--max-filter-ms requires a value".to_owned())?;
                    max_filter_ms = parse_ms("--max-filter-ms", &value)?;
                }
                "-h" | "--help" => {
                    print_help();
                    std::process::exit(0);
                }
                _ => return Err(format!("unknown argument: {arg}")),
            }
        }

        let workspace = workspace_root();
        let archive = archive_path_or_default(archive, &workspace, entries);
        let report = report_path_or_default(report, &workspace);
        Ok(Self {
            entries,
            archive,
            report,
            max_first_screen_ms,
            max_page_ms,
            max_filter_ms,
        })
    }
}

impl Timings {
    fn first_screen(&self) -> Duration {
        self.open + self.root_page + self.first_page
    }
}

fn workspace_root() -> PathBuf {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    match manifest_dir.parent().and_then(Path::parent) {
        Some(root) => root.to_path_buf(),
        None => manifest_dir.to_path_buf(),
    }
}

fn archive_path_or_default(archive: Option<PathBuf>, workspace: &Path, entries: usize) -> PathBuf {
    match archive {
        Some(path) => path,
        None => workspace
            .join("target")
            .join("squallz-bench")
            .join(format!("gui-browse-{entries}.zip")),
    }
}

fn report_path_or_default(report: Option<PathBuf>, workspace: &Path) -> PathBuf {
    match report {
        Some(path) => path,
        None => workspace.join("benches").join("REPORT.md"),
    }
}

fn print_help() {
    println!(
        "Usage: cargo run -p squallz-gui --example gui_browse_bench -- [--entries N] [--archive PATH] [--report PATH] [--max-first-screen-ms N] [--max-page-ms N] [--max-filter-ms N]"
    );
}

fn parse_ms(flag: &str, value: &str) -> Result<u128, String> {
    let parsed: u128 = value
        .parse()
        .map_err(|_| format!("invalid {flag} value: {value}"))?;
    if parsed == 0 {
        Err(format!("{flag} must be greater than 0"))
    } else {
        Ok(parsed)
    }
}

fn write_large_zip(path: &Path, entries: usize) -> Result<(), String> {
    let file = File::create(path).map_err(|e| format!("create archive: {e}"))?;
    let mut out = BufWriter::new(file);
    let mut central = Vec::with_capacity(entries.saturating_mul(80));
    let mut offset = 0u64;
    for i in 0..entries {
        let name = format!("{LARGE_DIR}file_{i:06}.txt");
        let name_bytes = name.as_bytes();
        if name_bytes.len() > u16::MAX as usize {
            return Err("entry name too long".to_owned());
        }
        if offset > u32::MAX as u64 {
            return Err("benchmark local-header offset exceeded ZIP32 range".to_owned());
        }
        let local_offset = offset as u32;
        write_local_header(&mut out, name_bytes)?;
        offset += 30 + name_bytes.len() as u64;
        write_central_header(&mut central, name_bytes, local_offset)?;
    }

    let central_offset = offset;
    out.write_all(&central)
        .map_err(|e| format!("write central directory: {e}"))?;
    let central_size = central.len() as u64;
    offset += central_size;

    if entries > u16::MAX as usize {
        write_zip64_eocd(&mut out, entries, central_size, central_offset, offset)?;
    }
    write_eocd(&mut out, entries, central_size, central_offset)?;
    out.flush().map_err(|e| format!("flush archive: {e}"))
}

fn write_local_header(out: &mut dyn Write, name: &[u8]) -> Result<(), String> {
    write_u32(out, 0x0403_4B50)?;
    write_u16(out, 20)?;
    write_u16(out, 1 << 11)?;
    write_u16(out, 0)?;
    write_u16(out, 0)?;
    write_u16(out, 0)?;
    write_u32(out, 0)?;
    write_u32(out, 0)?;
    write_u32(out, 0)?;
    write_u16(out, name.len() as u16)?;
    write_u16(out, 0)?;
    out.write_all(name)
        .map_err(|e| format!("write local name: {e}"))
}

fn write_central_header(out: &mut Vec<u8>, name: &[u8], local_offset: u32) -> Result<(), String> {
    write_u32(out, 0x0201_4B50)?;
    write_u16(out, 20)?;
    write_u16(out, 20)?;
    write_u16(out, 1 << 11)?;
    write_u16(out, 0)?;
    write_u16(out, 0)?;
    write_u16(out, 0)?;
    write_u32(out, 0)?;
    write_u32(out, 0)?;
    write_u32(out, 0)?;
    write_u16(out, name.len() as u16)?;
    write_u16(out, 0)?;
    write_u16(out, 0)?;
    write_u16(out, 0)?;
    write_u16(out, 0)?;
    write_u32(out, 0)?;
    write_u32(out, local_offset)?;
    out.write_all(name)
        .map_err(|e| format!("write central name: {e}"))
}

fn write_zip64_eocd(
    out: &mut dyn Write,
    entries: usize,
    central_size: u64,
    central_offset: u64,
    zip64_offset: u64,
) -> Result<(), String> {
    write_u32(out, 0x0606_4B50)?;
    write_u64(out, 44)?;
    write_u16(out, 45)?;
    write_u16(out, 45)?;
    write_u32(out, 0)?;
    write_u32(out, 0)?;
    write_u64(out, entries as u64)?;
    write_u64(out, entries as u64)?;
    write_u64(out, central_size)?;
    write_u64(out, central_offset)?;

    write_u32(out, 0x0706_4B50)?;
    write_u32(out, 0)?;
    write_u64(out, zip64_offset)?;
    write_u32(out, 1)
}

fn write_eocd(
    out: &mut dyn Write,
    entries: usize,
    central_size: u64,
    central_offset: u64,
) -> Result<(), String> {
    write_u32(out, 0x0605_4B50)?;
    write_u16(out, 0)?;
    write_u16(out, 0)?;
    write_u16(out, entries.min(u16::MAX as usize) as u16)?;
    write_u16(out, entries.min(u16::MAX as usize) as u16)?;
    write_u32(out, central_size.min(u32::MAX as u64) as u32)?;
    write_u32(out, central_offset.min(u32::MAX as u64) as u32)?;
    write_u16(out, 0)
}

fn write_u16(out: &mut dyn Write, value: u16) -> Result<(), String> {
    out.write_all(&value.to_le_bytes())
        .map_err(|e| format!("write u16: {e}"))
}

fn write_u32(out: &mut dyn Write, value: u32) -> Result<(), String> {
    out.write_all(&value.to_le_bytes())
        .map_err(|e| format!("write u32: {e}"))
}

fn write_u64(out: &mut dyn Write, value: u64) -> Result<(), String> {
    out.write_all(&value.to_le_bytes())
        .map_err(|e| format!("write u64: {e}"))
}

fn render_report(args: &Args, timings: &Timings, archive_bytes: Option<u64>) -> String {
    let first_screen_ms = ms(timings.first_screen());
    let max_page_ms = max_page_ms(timings);
    let filter_ms = ms(timings.filter);
    let status = if threshold_failures(args, timings).is_empty() {
        "pass"
    } else {
        "needs optimization"
    };
    format!(
        r#"# Squallz Performance Report

## GUI 100k Browse Smoke
- Generated at unix seconds: {}
- Platform: {}
- Entries: {}
- Archive: `{}`
- Archive bytes: {}
- Page size: {}
- First-screen target: <= {} ms
- Single-page target: <= {} ms
- Filter target: <= {} ms
- Threshold status: {}

| Step | Time (ms) |
| ---- | ---------: |
| Generate fixture | {} |
| Open + index archive | {} |
| Root page | {} |
| First `files/` page | {} |
| Middle `files/` page | {} |
| Last `files/` page | {} |
| Filter exact final file | {} |
| First screen total | {} |
| Max single page | {} |

Notes:
- This benchmark exercises the GUI backend browse path (`AppState::open_archive` + `list_entries`) with a generated ZIP64 archive containing empty files under `files/`.
- It fails with a non-zero exit code when thresholds are exceeded.
- It does not verify rendered WebView pixels or user interaction; visual/window smoke remains separate evidence.
"#,
        unix_seconds(),
        env::consts::OS,
        args.entries,
        args.archive.display(),
        archive_bytes_label(archive_bytes),
        DEFAULT_PAGE_SIZE,
        args.max_first_screen_ms,
        args.max_page_ms,
        args.max_filter_ms,
        status,
        ms(timings.generate),
        ms(timings.open),
        ms(timings.root_page),
        ms(timings.first_page),
        ms(timings.middle_page),
        ms(timings.last_page),
        filter_ms,
        first_screen_ms,
        max_page_ms,
    )
}

fn threshold_failures(args: &Args, timings: &Timings) -> Vec<String> {
    let mut failures = Vec::new();
    let first_screen_ms = ms(timings.first_screen());
    if first_screen_ms > args.max_first_screen_ms {
        failures.push(format!(
            "first screen {first_screen_ms} ms > {} ms",
            args.max_first_screen_ms
        ));
    }
    let max_page_ms = max_page_ms(timings);
    if max_page_ms > args.max_page_ms {
        failures.push(format!("page {max_page_ms} ms > {} ms", args.max_page_ms));
    }
    let filter_ms = ms(timings.filter);
    if filter_ms > args.max_filter_ms {
        failures.push(format!("filter {filter_ms} ms > {} ms", args.max_filter_ms));
    }
    failures
}

fn max_page_ms(timings: &Timings) -> u128 {
    let mut max_ms = 0;
    for duration in [
        timings.root_page,
        timings.first_page,
        timings.middle_page,
        timings.last_page,
    ] {
        max_ms = max_ms.max(ms(duration));
    }
    max_ms
}

fn unix_seconds() -> u64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_secs(),
        Err(_) => 0,
    }
}

fn archive_bytes_label(archive_bytes: Option<u64>) -> String {
    match archive_bytes {
        Some(bytes) => bytes.to_string(),
        None => "unknown".to_owned(),
    }
}

fn ms(duration: Duration) -> u128 {
    duration.as_millis()
}
