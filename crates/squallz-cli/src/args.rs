//! clap argument definitions.
//!
//! i18n note: runtime output goes through the language packs. Clap help is
//! static by default, so `try_print_localized_help` rewrites the generated
//! command tree for English help before clap handles `--help`.

use std::ffi::OsString;
use std::path::PathBuf;

use clap::{error::ErrorKind, ArgGroup, Command, CommandFactory, Parser, Subcommand, ValueEnum};
use squallz_core::api::{OverwritePolicy, ResourceOptions, SafetyLimits, SymlinkPolicy};
use squallz_core::ChecksumAlgorithm;

pub fn try_print_localized_help<I>(args: I) -> Option<i32>
where
    I: IntoIterator<Item = OsString>,
{
    let args: Vec<OsString> = args.into_iter().collect();
    if !is_help_or_version_request(&args) || !help_lang_is_english(&args) {
        return None;
    }

    let mut cmd = localize_help_en(Cli::command());
    match cmd.try_get_matches_from_mut(args) {
        Ok(_) => None,
        Err(err) => {
            let code = err.exit_code();
            let kind = err.kind();
            let _ = err.print();
            if matches!(kind, ErrorKind::DisplayHelp | ErrorKind::DisplayVersion) {
                Some(0)
            } else {
                Some(code)
            }
        }
    }
}

fn is_help_or_version_request(args: &[OsString]) -> bool {
    args.iter().skip(1).any(|arg| {
        let value = arg.to_string_lossy();
        matches!(
            value.as_ref(),
            "--help" | "-h" | "help" | "--version" | "-V"
        )
    })
}

fn help_lang_is_english(args: &[OsString]) -> bool {
    let mut iter = args.iter().skip(1);
    while let Some(arg) = iter.next() {
        let value = arg.to_string_lossy();
        if value == "--lang" {
            let Some(next) = iter.next() else {
                return false;
            };
            return is_english_lang(&next.to_string_lossy());
        }
        if let Some(lang) = value.strip_prefix("--lang=") {
            return is_english_lang(lang);
        }
    }
    match std::env::var("SQZ_LANG") {
        Ok(lang) => is_english_lang(&lang),
        Err(_) => false,
    }
}

fn is_english_lang(lang: &str) -> bool {
    let normalized = lang.trim().to_ascii_lowercase();
    normalized == "en" || normalized.starts_with("en-") || normalized.starts_with("en_")
}

fn localize_help_en(cmd: Command) -> Command {
    let cmd = cmd
        .about("Squallz: cross-platform archive manager")
        .mut_arg("lang", |arg| {
            arg.help("Interface language, for example zh-CN or en-US. Defaults to SQZ_LANG or the system language.")
        })
        .mut_arg("quiet", |arg| arg.help("Quiet mode: do not print progress."))
        .mut_arg("verbose", |arg| arg.help("Verbose mode: print one line per entry."))
        .mut_arg("output_style", |arg| {
            arg.help("Human-readable output style. Defaults to classic; modern only changes non-JSON output.")
        })
        .mut_arg("color", |arg| {
            arg.help("When to colorize modern human output: auto, always, rich, fancy, or never.")
        })
        .mut_arg("accent", |arg| {
            arg.help("Theme palette for modern human output: squallz, brand, icon, cascade, daylight, foam, skyline, aero, crest, halo, tropic, kinetic, radiant, surge, glass, nova, crystal, lumina, azure, surf, signal, tide, breeze, neon, electric, vapor, ocean, jade, teal, aqua, glacier, aurora, prism, lagoon, mint, sunset, citrus, blue, violet, amber, rose, or mono. Also available as --palette, --theme, --color-scheme, --scheme, or --colors.")
        });

    cmd.mut_subcommand("compress", localize_compress_help_en)
        .mut_subcommand("pack", localize_pack_help_en)
        .mut_subcommand("estimate", localize_estimate_help_en)
        .mut_subcommand("duplicates", localize_duplicates_help_en)
        .mut_subcommand("checksum", localize_checksum_help_en)
        .mut_subcommand("extract", localize_extract_help_en)
        .mut_subcommand("list", localize_list_help_en)
        .mut_subcommand("test", localize_test_help_en)
        .mut_subcommand("convert", localize_convert_help_en)
        .mut_subcommand("nested", localize_nested_help_en)
        .mut_subcommand("export", localize_export_help_en)
        .mut_subcommand("update", localize_update_help_en)
        .mut_subcommand("protect", localize_protect_help_en)
        .mut_subcommand("verify", localize_verify_help_en)
        .mut_subcommand("repair", localize_repair_help_en)
        .mut_subcommand("batch", localize_batch_help_en)
        .mut_subcommand("doctor", localize_doctor_help_en)
        .mut_subcommand("info", |cmd| {
            cmd.about("List supported formats and capabilities")
                .mut_arg("json", json_help_en)
        })
}

fn localize_compress_help_en(cmd: Command) -> Command {
    cmd.about("Compress files or folders")
        .mut_arg("inputs", |arg| arg.help("Input files or folders."))
        .mut_arg("output", |arg| {
            arg.help("Output archive path. The format is detected from the file extension.")
        })
        .mut_arg("format", |arg| {
            arg.help("Explicit output format, such as zip, 7z, or tar.gz. Must match the output extension.")
        })
        .mut_arg("level", |arg| {
            arg.help("Compression level 0-9. 0 stores only; 9 is maximum. Overrides --profile.")
        })
        .mut_arg("profile", profile_help_en)
        .mut_arg("password", |arg| arg.help("Encryption password."))
        .mut_arg("encrypt_names", |arg| {
            arg.help("Encrypt entry names when the target format supports header encryption.")
        })
        .mut_arg("excludes", exclude_help_en)
        .mut_arg("split", split_help_en)
        .mut_arg("threads", threads_help_en)
        .mut_arg("memory_limit", memory_limit_help_en)
        .mut_arg("json", json_help_en)
}

fn localize_pack_help_en(cmd: Command) -> Command {
    cmd.about("Create a native Squallz .sqz self-recovery container")
        .mut_arg("inputs", |arg| arg.help("Input files or folders."))
        .mut_arg("output", |arg| arg.help("Output .sqz container path."))
        .mut_arg("level", |arg| {
            arg.help("Compression level 0-9. Overrides --profile; SQZ v1 transparent containers may ignore this value.")
        })
        .mut_arg("profile", profile_help_en)
        .mut_arg("inner_format", |arg| {
            arg.help("SQZ v1 inner payload profile: sqz, entry-set, zip, tar, 7z, or zstd.")
        })
        .mut_arg("recovery", |arg| {
            arg.help("SQZ payload recovery redundancy percentage, such as 10 or 10%.")
        })
        .mut_arg("excludes", exclude_help_en)
        .mut_arg("split", split_help_en)
        .mut_arg("threads", threads_help_en)
        .mut_arg("memory_limit", memory_limit_help_en)
        .mut_arg("json", json_help_en)
}

fn localize_estimate_help_en(cmd: Command) -> Command {
    cmd.about("Estimate input size and destination disk space before creating an archive")
        .mut_arg("inputs", |arg| arg.help("Input files or folders."))
        .mut_arg("excludes", exclude_help_en)
        .mut_arg("output", |arg| {
            arg.help(
                "Planned output archive path. When set, Squallz checks destination free space.",
            )
        })
        .mut_arg("json", json_help_en)
}

fn localize_duplicates_help_en(cmd: Command) -> Command {
    cmd.about("Find duplicate local files before archiving or cleanup")
        .mut_arg("inputs", |arg| arg.help("Input files or folders to scan."))
        .mut_arg("excludes", exclude_help_en)
        .mut_arg("min_size", |arg| {
            arg.help("Minimum file size to hash, such as 1, 4k, 10m, or 1g. Defaults to 1 byte.")
        })
        .mut_arg("json", json_help_en)
}

fn localize_checksum_help_en(cmd: Command) -> Command {
    cmd.about("Compute checksums for files or folders")
        .mut_arg("inputs", |arg| arg.help("Input files or folders to hash."))
        .mut_arg("algorithm", |arg| {
            arg.help("Checksum algorithm: sha256, blake3, sha512, sha384, sha224, sha1, md5, or crc32. Defaults to sha256.")
        })
        .mut_arg("check", |arg| {
            arg.help("Verify a checksum manifest in '<digest>  <path>' format. Relative paths resolve from the manifest directory.")
        })
        .mut_arg("excludes", exclude_help_en)
        .mut_arg("json", json_help_en)
}

fn localize_extract_help_en(cmd: Command) -> Command {
    cmd.about("Extract an archive")
        .mut_arg("archive", |arg| arg.help("Archive path."))
        .mut_arg("dest", |arg| arg.help("Destination directory. Defaults to the current directory."))
        .mut_arg("includes", |arg| arg.help("Only extract matching entries. Can be repeated."))
        .mut_arg("overwrite", |arg| arg.help("Overwrite policy."))
        .mut_arg("password", |arg| arg.help("Decryption password."))
        .mut_arg("encoding", |arg| arg.help("Entry-name encoding, such as gbk or shift_jis."))
        .mut_arg("symlinks", |arg| arg.help("Symbolic-link handling policy."))
        .mut_arg("smart", |arg| {
            arg.help("Smart extract: extract a single root directory directly, otherwise wrap loose entries in a same-name folder.")
        })
        .mut_arg("best_effort", |arg| {
            arg.help("Best-effort extract: skip readable-entry or CRC failures within recoverable boundaries.")
        })
        .mut_arg("threads", threads_help_en)
        .mut_arg("memory_limit", memory_limit_help_en)
        .mut_arg("max_output_bytes", |arg| arg.help("Maximum total extracted output size."))
        .mut_arg("max_entries", |arg| arg.help("Maximum number of extracted entries."))
        .mut_arg("max_compression_ratio", |arg| {
            arg.help("Maximum decompressed-to-compressed ratio for a single entry.")
        })
        .mut_arg("json", json_help_en)
}

fn localize_list_help_en(cmd: Command) -> Command {
    cmd.about("List archive contents")
        .mut_arg("archive", |arg| arg.help("Archive path."))
        .mut_arg("password", |arg| arg.help("Decryption password."))
        .mut_arg("encoding", |arg| arg.help("Entry-name encoding."))
        .mut_arg("json", json_help_en)
        .mut_arg("tree", |arg| {
            arg.help("Print a directory tree for human reading.")
        })
}

fn localize_test_help_en(cmd: Command) -> Command {
    cmd.about("Test archive integrity")
        .mut_arg("archive", |arg| arg.help("Archive path."))
        .mut_arg("password", |arg| arg.help("Decryption password."))
        .mut_arg("encoding", |arg| arg.help("Entry-name encoding."))
        .mut_arg("json", json_help_en)
}

fn localize_batch_help_en(cmd: Command) -> Command {
    cmd.about("Run a JSON batch script of archive operations")
        .mut_arg("script", |arg| {
            arg.help("Batch JSON script. Relative paths resolve from the script directory unless base_dir is set.")
        })
        .mut_arg("keep_going", |arg| {
            arg.help("Continue after a failed job and report all failures.")
        })
        .mut_arg("json", json_help_en)
}

fn localize_doctor_help_en(cmd: Command) -> Command {
    cmd.about("Diagnose runtime engines, external tools, and recovery readiness")
        .mut_arg("strict", |arg| {
            arg.help("Exit 8 when a runtime dependency required by release-claimed capabilities is missing.")
        })
        .mut_arg("json", json_help_en)
}

fn localize_convert_help_en(cmd: Command) -> Command {
    cmd.about("Convert an archive to another format without extracting it to disk")
        .mut_arg("src", |arg| arg.help("Source archive path."))
        .mut_arg("output", |arg| {
            arg.help("Output archive path. The format is detected from the file extension.")
        })
        .mut_arg("password", |arg| {
            arg.help("Source archive decryption password.")
        })
        .mut_arg("out_password", |arg| {
            arg.help("Output archive encryption password.")
        })
        .mut_arg("encrypt_names", |arg| {
            arg.help(
                "Encrypt output entry names when the target format supports header encryption.",
            )
        })
        .mut_arg("level", |arg| {
            arg.help("Output compression level 0-9. Overrides --profile.")
        })
        .mut_arg("profile", profile_help_en)
        .mut_arg("encoding", |arg| arg.help("Source entry-name encoding."))
        .mut_arg("threads", threads_help_en)
        .mut_arg("memory_limit", memory_limit_help_en)
        .mut_arg("json", json_help_en)
}

fn localize_nested_help_en(cmd: Command) -> Command {
    cmd.about("Operate on an archive entry that is itself an archive")
        .mut_subcommand("list", |cmd| {
            cmd.about("List nested archive contents")
                .mut_arg("archive", |arg| arg.help("Outer archive path."))
                .mut_arg("entry", |arg| arg.help("Nested archive entry path inside the outer archive."))
                .mut_arg("password", |arg| arg.help("Outer archive decryption password."))
                .mut_arg("encoding", |arg| arg.help("Outer archive entry-name encoding."))
                .mut_arg("nested_password", |arg| arg.help("Nested archive decryption password."))
                .mut_arg("nested_encoding", |arg| arg.help("Nested archive entry-name encoding."))
                .mut_arg("json", json_help_en)
                .mut_arg("tree", |arg| arg.help("Print a directory tree for human reading."))
        })
        .mut_subcommand("extract", |cmd| {
            cmd.about("Extract nested archive contents")
                .mut_arg("archive", |arg| arg.help("Outer archive path."))
                .mut_arg("entry", |arg| arg.help("Nested archive entry path inside the outer archive."))
                .mut_arg("dest", |arg| arg.help("Destination directory. Defaults to the current directory."))
                .mut_arg("includes", |arg| arg.help("Only extract matching nested entries. Can be repeated."))
                .mut_arg("overwrite", |arg| arg.help("Overwrite policy."))
                .mut_arg("password", |arg| arg.help("Outer archive decryption password."))
                .mut_arg("encoding", |arg| arg.help("Outer archive entry-name encoding."))
                .mut_arg("nested_password", |arg| arg.help("Nested archive decryption password."))
                .mut_arg("nested_encoding", |arg| arg.help("Nested archive entry-name encoding."))
                .mut_arg("symlinks", |arg| arg.help("Symbolic-link handling policy."))
                .mut_arg("smart", |arg| {
                    arg.help("Smart extract: extract a single root directory directly, otherwise wrap loose entries in a same-name folder.")
                })
                .mut_arg("best_effort", |arg| {
                    arg.help("Best-effort extract: skip readable-entry or CRC failures within recoverable boundaries.")
                })
                .mut_arg("threads", threads_help_en)
                .mut_arg("memory_limit", memory_limit_help_en)
                .mut_arg("max_output_bytes", |arg| arg.help("Maximum total extracted output size."))
                .mut_arg("max_entries", |arg| arg.help("Maximum number of extracted entries."))
                .mut_arg("max_compression_ratio", |arg| {
                    arg.help("Maximum decompressed-to-compressed ratio for a single entry.")
                })
                .mut_arg("json", json_help_en)
        })
}

fn localize_export_help_en(cmd: Command) -> Command {
    cmd.about("Export a .sqz container to a standard archive")
        .mut_arg("archive", |arg| arg.help("Source .sqz container path."))
        .mut_arg("output", |arg| {
            arg.help(
                "Output standard archive path. The format is detected from the file extension.",
            )
        })
        .mut_arg("level", |arg| {
            arg.help("Output compression level 0-9. Overrides --profile.")
        })
        .mut_arg("profile", profile_help_en)
        .mut_arg("out_password", |arg| {
            arg.help("Output archive encryption password.")
        })
        .mut_arg("threads", threads_help_en)
        .mut_arg("memory_limit", memory_limit_help_en)
        .mut_arg("json", json_help_en)
}

fn localize_update_help_en(cmd: Command) -> Command {
    cmd.about("Update an existing archive: add, delete, rename, move, or create entries")
        .mut_arg("archive", |arg| arg.help("Archive path."))
        .mut_arg("add", |arg| {
            arg.help("Add a local file or folder. Can be repeated.")
        })
        .mut_arg("mkdir", |arg| {
            arg.help("Create an empty directory entry. Can be repeated.")
        })
        .mut_arg("delete", |arg| {
            arg.help("Delete entries matching a glob. Can be repeated.")
        })
        .mut_arg("rename", |arg| {
            arg.help("Rename an entry. Format: from=to. Can be repeated.")
        })
        .mut_arg("move_entries", |arg| {
            arg.help("Move an entry to a new archive path. Format: from=to. Can be repeated.")
        })
        .mut_arg("excludes", exclude_help_en)
        .mut_arg("password", |arg| {
            arg.help("Encryption password for added entries.")
        })
        .mut_arg("encrypt_names", |arg| {
            arg.help("Encrypt added entry names when the target format supports header encryption.")
        })
        .mut_arg("level", |arg| {
            arg.help("Compression level for added entries, 0-9. Overrides --profile.")
        })
        .mut_arg("profile", profile_help_en)
        .mut_arg("threads", threads_help_en)
        .mut_arg("memory_limit", memory_limit_help_en)
        .mut_arg("json", json_help_en)
}

fn localize_protect_help_en(cmd: Command) -> Command {
    cmd.about("Create external PAR2 recovery data for a standard archive")
        .mut_arg("archive", |arg| arg.help("Archive path to protect."))
        .mut_arg("redundancy", |arg| {
            arg.help("Recovery redundancy percentage, such as 10 or 10%. Conflicts with --tolerate-loss.")
        })
        .mut_arg("tolerate_loss", |arg| {
            arg.help("For a .001 split-volume set, create enough PAR2 redundancy to tolerate N missing largest volumes.")
        })
        .mut_arg("recovery", |arg| arg.help("Output .par2 index path. Defaults to <archive>.par2."))
        .mut_arg("json", json_help_en)
}

fn localize_verify_help_en(cmd: Command) -> Command {
    cmd.about("Verify external PAR2 recovery data")
        .mut_arg("archive", |arg| arg.help("Original archive path."))
        .mut_arg("use_recovery", |arg| {
            arg.help("Use external recovery data. Verify currently always uses PAR2.")
        })
        .mut_arg("recovery", |arg| {
            arg.help(".par2 index path. Defaults to <archive>.par2.")
        })
        .mut_arg("json", json_help_en)
}

fn localize_repair_help_en(cmd: Command) -> Command {
    cmd.about("Repair an archive with .sqz embedded recovery, ZIP local-header rebuild, or external PAR2 data")
        .mut_arg("archive", |arg| arg.help("Original archive path."))
        .mut_arg("use_recovery", |arg| arg.help("Force external PAR2 recovery data."))
        .mut_arg("output", |arg| {
            arg.help("Output path. Single-file .sqz repair can omit this for atomic in-place replacement; ZIP rebuild requires it.")
        })
        .mut_arg("recovery", |arg| arg.help(".par2 index path. Defaults to <archive>.par2."))
        .mut_arg("level", |arg| {
            arg.help("Compression level for SQZ rewrite output, 0-9. Overrides --profile; ignored by external PAR2 repair.")
        })
        .mut_arg("profile", profile_help_en)
        .mut_arg("threads", threads_help_en)
        .mut_arg("memory_limit", memory_limit_help_en)
        .mut_arg("json", json_help_en)
}

fn json_help_en(arg: clap::Arg) -> clap::Arg {
    arg.help("Print JSON output for machine use.")
}

fn exclude_help_en(arg: clap::Arg) -> clap::Arg {
    arg.help(
        "Exclude a glob pattern. Can be repeated, for example --exclude .git --exclude \"*.tmp\".",
    )
}

fn split_help_en(arg: clap::Arg) -> clap::Arg {
    arg.help("Split volume size, such as 500k, 100m, or 1g. Outputs .001/.002/... volumes.")
}

fn profile_help_en(arg: clap::Arg) -> clap::Arg {
    arg.help("Built-in create profile: fast=level 2, balanced=level 6, maximum=level 9. Explicit --level overrides it.")
}

fn threads_help_en(arg: clap::Arg) -> clap::Arg {
    arg.help("Worker thread count. Defaults to automatic.")
}

fn memory_limit_help_en(arg: clap::Arg) -> clap::Arg {
    arg.help(
        "Squallz stream-buffer memory limit, such as 256m or 1g. This is not a process RSS limit.",
    )
}

/// Parses a human-readable size: plain bytes or binary `k`/`m`/`g`
/// (optionally `kb`/`mb`/`gb`) suffixes, e.g. `500k`, `100m`, `1g`.
/// (clap parse errors stay English like the rest of clap's static output.)
pub fn parse_size(s: &str) -> Result<u64, String> {
    let lower = s.trim().to_lowercase();
    let mut digits_end = lower.len();
    for (index, c) in lower.char_indices() {
        if !c.is_ascii_digit() {
            digits_end = index;
            break;
        }
    }
    let (digits, suffix) = lower.split_at(digits_end);
    let multiplier: u64 = match suffix {
        "" | "b" => 1,
        "k" | "kb" => 1024,
        "m" | "mb" => 1024 * 1024,
        "g" | "gb" => 1024 * 1024 * 1024,
        other => return Err(format!("unknown size suffix '{other}'")),
    };
    let value: u64 = digits.parse().map_err(|_| format!("invalid size '{s}'"))?;
    value
        .checked_mul(multiplier)
        .ok_or_else(|| format!("size '{s}' overflows"))
}

/// Parses a positive size for safety/resource limits.
pub fn parse_nonzero_size(s: &str) -> Result<u64, String> {
    let value = parse_size(s)?;
    if value == 0 {
        Err(format!("size '{s}' must be greater than zero"))
    } else {
        Ok(value)
    }
}

/// Parses a positive `usize`.
pub fn parse_nonzero_usize(s: &str) -> Result<usize, String> {
    let value: usize = s.parse().map_err(|_| format!("invalid number '{s}'"))?;
    if value == 0 {
        Err(format!("value '{s}' must be greater than zero"))
    } else {
        Ok(value)
    }
}

/// Parses a positive `u64`.
pub fn parse_nonzero_u64(s: &str) -> Result<u64, String> {
    let value: u64 = s.parse().map_err(|_| format!("invalid number '{s}'"))?;
    if value == 0 {
        Err(format!("value '{s}' must be greater than zero"))
    } else {
        Ok(value)
    }
}

/// Parses a positive `u32`.
pub fn parse_nonzero_u32(s: &str) -> Result<u32, String> {
    let value: u32 = s.parse().map_err(|_| format!("invalid number '{s}'"))?;
    if value == 0 {
        Err(format!("value '{s}' must be greater than zero"))
    } else {
        Ok(value)
    }
}

/// Parses a compression level from 0 (store) through 9 (maximum).
pub fn parse_compression_level(s: &str) -> Result<u8, String> {
    let value: u8 = s
        .parse()
        .map_err(|_| format!("invalid compression level '{s}'"))?;
    if value <= 9 {
        Ok(value)
    } else {
        Err(format!("compression level '{s}' must be between 0 and 9"))
    }
}

/// Parses an `old=new` rename specification.
pub fn parse_rename(s: &str) -> Result<(String, String), String> {
    match s.split_once('=') {
        Some((from, to)) if !from.is_empty() && !to.is_empty() => {
            Ok((from.to_string(), to.to_string()))
        }
        _ => Err(format!("expected from=to, got '{s}'")),
    }
}

/// Parses a redundancy percentage such as `10` or `10%`.
pub fn parse_percent(s: &str) -> Result<u8, String> {
    let value = s.trim().trim_end_matches('%');
    let percent: u8 = value
        .parse()
        .map_err(|_| format!("invalid percentage '{s}'"))?;
    if (1..=100).contains(&percent) {
        Ok(percent)
    } else {
        Err(format!("percentage '{s}' must be between 1 and 100"))
    }
}

/// Parses a tolerated split-volume loss count such as `2`, `2volumes` or
/// `2vols`.
pub fn parse_tolerate_loss(s: &str) -> Result<u32, String> {
    let lower = s.trim().to_ascii_lowercase();
    let mut value = lower.as_str();
    for suffix in ["volumes", "volume", "vols", "vol"] {
        if let Some(stripped) = value.strip_suffix(suffix) {
            value = stripped;
            break;
        }
    }
    let value = value.trim();
    let count: u32 = value
        .parse()
        .map_err(|_| format!("invalid tolerated volume loss '{s}'"))?;
    if count == 0 {
        Err(format!(
            "tolerated volume loss '{s}' must be greater than zero"
        ))
    } else {
        Ok(count)
    }
}

/// Parses the SQZ v1 inner payload profile.
pub fn parse_sqz_inner_format(s: &str) -> Result<String, String> {
    let normalized = s.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "sqz" | "entry-set" | "entryset" | "entries" => Ok("sqz".to_string()),
        "zip" | "tar" | "7z" | "zstd" => Ok(normalized),
        "" => Err("inner format must not be empty".to_string()),
        "raw" => Err(format!(
            "SQZ v1 currently supports only --inner-format sqz (entry-set), zip, tar, 7z, and zstd; inner format '{normalized}' is planned but not implemented"
        )),
        other => Err(format!("unsupported SQZ inner format '{other}'")),
    }
}

/// Maps CLI resource flags onto the shared core resource options.
pub fn resource_options(threads: Option<usize>, memory_limit: Option<u64>) -> ResourceOptions {
    ResourceOptions {
        threads,
        memory_limit,
    }
}

/// Maps CLI safety flags onto the shared extraction guardrails.
pub fn safety_limits(
    max_output_bytes: Option<u64>,
    max_entries: Option<u64>,
    max_compression_ratio: Option<u32>,
) -> SafetyLimits {
    let mut limits = SafetyLimits::default();
    if let Some(value) = max_output_bytes {
        limits.max_output_bytes = value;
    }
    if let Some(value) = max_entries {
        limits.max_entries = value;
    }
    if let Some(value) = max_compression_ratio {
        limits.max_compression_ratio = value;
    }
    limits
}

#[derive(Parser)]
#[command(name = "sqz", version, about = "Squallz：跨平台压缩解压工具")]
pub struct Cli {
    /// 界面语言（如 zh-CN / en-US；默认 SQZ_LANG 环境变量或系统语言）
    #[arg(long, global = true)]
    pub lang: Option<String>,
    /// 静默模式：不输出进度
    #[arg(short, long, global = true)]
    pub quiet: bool,
    /// 详细模式：逐条目打印
    #[arg(short, long, global = true, conflicts_with = "quiet")]
    pub verbose: bool,
    /// 人类可读输出风格（默认 classic；modern 只影响非 JSON 输出）
    #[arg(long = "style", global = true, value_enum, default_value_t)]
    pub output_style: OutputStyleArg,
    /// 颜色输出策略（默认 auto；只影响人类可读输出）
    #[arg(long, global = true, value_enum, default_value_t)]
    pub color: ColorArg,
    /// modern 人类输出配色（默认 squallz；可用 --palette / --theme / --color-scheme / --colors；不影响 JSON / classic）
    #[arg(
        long,
        visible_aliases = ["palette", "theme", "color-scheme", "scheme", "colors"],
        global = true,
        value_enum,
        default_value_t
    )]
    pub accent: AccentArg,
    #[command(subcommand)]
    pub cmd: Cmd,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Default)]
pub enum OutputStyleArg {
    /// Traditional Linux-style output for scripts and logs
    #[default]
    Classic,
    /// Modern interactive output; does not change --json
    Modern,
}

impl OutputStyleArg {
    pub fn is_modern(self) -> bool {
        matches!(self, Self::Modern)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum CreateProfileArg {
    /// Quick sharing, lower CPU pressure
    Fast,
    /// Good ratio without making the machine feel busy
    Balanced,
    /// Best ratio for long-term storage
    Maximum,
}

impl CreateProfileArg {
    pub fn level(self) -> u8 {
        match self {
            Self::Fast => 2,
            Self::Balanced => 6,
            Self::Maximum => 9,
        }
    }
}

pub fn effective_compression_level(level: Option<u8>, profile: Option<CreateProfileArg>) -> u8 {
    if let Some(level) = level {
        return level;
    }
    if let Some(profile) = profile {
        return profile.level();
    }
    5
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Default)]
pub enum ChecksumAlgorithmArg {
    /// SHA-256, conventional for release artifacts and scripts
    #[default]
    #[value(alias = "sha-256")]
    Sha256,
    /// BLAKE3, fast modern content hashing
    #[value(alias = "b3")]
    Blake3,
    /// SHA-512, wider SHA-2 digest used by some manifests
    #[value(alias = "sha-512")]
    Sha512,
    /// SHA-384, SHA-2 compatibility for signed release manifests
    #[value(alias = "sha-384")]
    Sha384,
    /// SHA-224, SHA-2 compatibility for uncommon manifests
    #[value(alias = "sha-224")]
    Sha224,
    /// SHA-1, legacy manifest compatibility
    #[value(alias = "sha-1")]
    Sha1,
    /// MD5, legacy manifest compatibility
    #[value(alias = "md-5")]
    Md5,
    /// ZIP-compatible CRC-32
    #[value(alias = "crc-32")]
    Crc32,
}

impl From<ChecksumAlgorithmArg> for ChecksumAlgorithm {
    fn from(value: ChecksumAlgorithmArg) -> Self {
        match value {
            ChecksumAlgorithmArg::Sha256 => Self::Sha256,
            ChecksumAlgorithmArg::Blake3 => Self::Blake3,
            ChecksumAlgorithmArg::Sha512 => Self::Sha512,
            ChecksumAlgorithmArg::Sha384 => Self::Sha384,
            ChecksumAlgorithmArg::Sha224 => Self::Sha224,
            ChecksumAlgorithmArg::Sha1 => Self::Sha1,
            ChecksumAlgorithmArg::Md5 => Self::Md5,
            ChecksumAlgorithmArg::Crc32 => Self::Crc32,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Default)]
pub enum ColorArg {
    /// Color only when writing to an interactive terminal
    #[default]
    Auto,
    /// Always emit ANSI color in human output
    Always,
    /// Force rich ANSI color for modern screenshots, demos, and redirected previews
    Rich,
    /// Force richer ANSI color for modern live progress, screenshots, and redirected demos
    Fancy,
    /// Never emit ANSI color
    Never,
}

impl ColorArg {
    pub fn enabled(self, stream_is_terminal: bool) -> bool {
        match self {
            Self::Always => true,
            Self::Rich => true,
            Self::Fancy => true,
            Self::Never => false,
            Self::Auto => stream_is_terminal && std::env::var_os("NO_COLOR").is_none(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Default)]
pub enum AccentArg {
    /// Squallz brand palette matching the app icon teal-to-sky gradient
    #[default]
    Squallz,
    /// Short brand alias for the approved teal-to-sky app icon palette
    Brand,
    /// Explicit app-icon palette using the approved teal-to-sky SVG gradient
    Icon,
    /// Cascade palette with approved teal primary and brighter sky highlights
    Cascade,
    /// Bright daylight palette using approved teal with a clean sky highlight
    Daylight,
    /// Soft foam palette using approved teal with an ice-blue highlight
    Foam,
    /// Skyline palette with sky primary and the approved Squallz teal highlight
    Skyline,
    /// Airy sky-led palette with the approved Squallz teal highlight
    Aero,
    /// Crest palette with bright sky primary and luminous aqua secondary accents
    Crest,
    /// Halo palette with luminous teal primary and bright sky secondary accents
    Halo,
    /// Tropic palette with the approved teal primary and electric cyan secondary accents
    Tropic,
    /// Kinetic palette with the approved teal primary and high-energy sky secondary accents
    Kinetic,
    /// Radiant palette with approved teal primary and bright sky-glass highlights
    Radiant,
    /// Surge palette with approved teal primary and vivid sky-blue highlights for live HUDs
    Surge,
    /// Glassy teal-to-sky palette with brighter readable highlights
    Glass,
    /// Nova palette with bright cyan primary and sunlit gold highlights
    Nova,
    /// Crystal palette with luminous aqua primary and clear sky highlights
    Crystal,
    /// Lumina palette with bright cyan primary and coral highlights for vivid modern dashboards
    Lumina,
    /// Bright azure palette with sky primary and Squallz teal highlights
    Azure,
    /// Surf palette with electric cyan primary and sky-blue highlights
    Surf,
    /// High-signal teal and sky palette with a brighter primary accent
    Signal,
    /// Bright tide palette with light cyan primary and sky highlights
    Tide,
    /// Breeze palette with teal primary and sky highlights for calm high-contrast terminals
    Breeze,
    /// Neon palette with cyan primary and pink highlights for high-contrast modern terminals
    Neon,
    /// Electric cyan and violet palette for high-energy modern terminals
    Electric,
    /// Vapor palette with luminous sky primary and soft violet highlights
    Vapor,
    /// Ocean palette with sky-blue primary and teal highlights
    Ocean,
    /// Jade palette with fresh green-cyan primary and Squallz teal highlights
    Jade,
    /// Teal/cyan palette
    Teal,
    /// Aqua palette with brighter cyan highlights
    Aqua,
    /// Glacier palette with bright cyan and sky highlights
    Glacier,
    /// Aurora palette with mint and cyan highlights
    Aurora,
    /// Prism palette with cyan and magenta highlights
    Prism,
    /// Lagoon palette with vivid teal and sky tones
    Lagoon,
    /// Mint palette with the Squallz teal base and a softer sky highlight
    Mint,
    /// Warm sunset palette with orange and rose highlights
    Sunset,
    /// Fresh citrus palette with lime and cyan highlights
    Citrus,
    /// Crisp blue palette
    Blue,
    /// Violet palette
    Violet,
    /// Warm amber palette
    Amber,
    /// Bright rose palette
    Rose,
    /// Monochrome high-contrast palette
    Mono,
}

#[derive(Subcommand)]
pub enum Cmd {
    /// 压缩文件/目录
    Compress {
        /// 输入文件或目录
        #[arg(required = true)]
        inputs: Vec<PathBuf>,
        /// 输出压缩包路径（格式由扩展名决定）
        #[arg(short, long)]
        output: PathBuf,
        /// 显式声明输出格式（如 zip / 7z / tar.gz；必须与 -o 扩展名匹配）
        #[arg(long)]
        format: Option<String>,
        /// 压缩级别 0-9（0=仅存储，9=极限；会覆盖 --profile）
        #[arg(long, value_parser = parse_compression_level)]
        level: Option<u8>,
        /// 内置创建预设（fast=2，balanced=6，maximum=9）
        #[arg(long, value_enum)]
        profile: Option<CreateProfileArg>,
        /// 加密密码
        #[arg(long)]
        password: Option<String>,
        /// 加密条目名（仅支持 7z 等具备 header encryption 的格式）
        #[arg(long)]
        encrypt_names: bool,
        /// 排除 glob 模式（可多次，如 --exclude .git --exclude "*.tmp"）
        #[arg(long = "exclude", value_name = "GLOB")]
        excludes: Vec<String>,
        /// 分卷大小（如 500k / 100m / 1g），产物为 .001/.002/... 分卷
        #[arg(long, value_name = "SIZE", value_parser = parse_size)]
        split: Option<u64>,
        /// 压缩 worker 线程数（默认自动）
        #[arg(long, value_name = "N", value_parser = parse_nonzero_usize)]
        threads: Option<usize>,
        /// Squallz 流式缓冲内存上限（如 256m / 1g；不是进程 RSS 上限）
        #[arg(long, value_name = "SIZE", value_parser = parse_nonzero_size)]
        memory_limit: Option<u64>,
        /// 以 JSON 输出（机器可读）
        #[arg(long)]
        json: bool,
    },
    /// 创建 Squallz 原生 .sqz 自恢复容器
    Pack {
        /// 输入文件或目录
        #[arg(required = true)]
        inputs: Vec<PathBuf>,
        /// 输出 .sqz 容器路径
        #[arg(short, long)]
        output: PathBuf,
        /// 压缩级别 0-9（会覆盖 --profile；当前 SQZ v1 透明容器可能忽略该值）
        #[arg(long, value_parser = parse_compression_level)]
        level: Option<u8>,
        /// 内置创建预设（fast=2，balanced=6，maximum=9）
        #[arg(long, value_enum)]
        profile: Option<CreateProfileArg>,
        /// SQZ v1 内部 payload profile（支持 sqz / entry-set / zip / tar / 7z / zstd）
        #[arg(long, default_value = "sqz", value_parser = parse_sqz_inner_format)]
        inner_format: String,
        /// SQZ payload 恢复冗余比例（如 10 或 10%；默认 25% 兼容原 8+2）
        #[arg(long, default_value = "25%", value_parser = parse_percent)]
        recovery: u8,
        /// 排除 glob 模式（可多次，如 --exclude .git --exclude "*.tmp"）
        #[arg(long = "exclude", value_name = "GLOB")]
        excludes: Vec<String>,
        /// 分卷大小（如 500k / 100m / 1g），产物为 .001/.002/... 分卷
        #[arg(long, value_name = "SIZE", value_parser = parse_size)]
        split: Option<u64>,
        /// 压缩 worker 线程数（默认自动）
        #[arg(long, value_name = "N", value_parser = parse_nonzero_usize)]
        threads: Option<usize>,
        /// Squallz 流式缓冲内存上限（如 256m / 1g；不是进程 RSS 上限）
        #[arg(long, value_name = "SIZE", value_parser = parse_nonzero_size)]
        memory_limit: Option<u64>,
        /// 以 JSON 输出（机器可读）
        #[arg(long)]
        json: bool,
    },
    /// 估算创建压缩包前的输入规模与目标磁盘空间
    Estimate {
        /// 输入文件或目录
        #[arg(required = true)]
        inputs: Vec<PathBuf>,
        /// 排除 glob 模式（可多次，如 --exclude .git --exclude "*.tmp"）
        #[arg(long = "exclude", value_name = "GLOB")]
        excludes: Vec<String>,
        /// 计划输出压缩包路径；提供后会检查目标卷可用空间
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// 以 JSON 输出（机器可读）
        #[arg(long)]
        json: bool,
    },
    /// 查找本地重复文件
    Duplicates {
        /// 输入文件或目录
        #[arg(required = true)]
        inputs: Vec<PathBuf>,
        /// 排除 glob 模式（可多次，如 --exclude .git --exclude "*.tmp"）
        #[arg(long = "exclude", value_name = "GLOB")]
        excludes: Vec<String>,
        /// 最小参与哈希的文件大小（默认 1 字节；可用 4k / 10m / 1g）
        #[arg(long, value_name = "SIZE", default_value = "1", value_parser = parse_size)]
        min_size: u64,
        /// 以 JSON 输出（机器可读）
        #[arg(long)]
        json: bool,
    },
    /// 计算本地文件校验和
    Checksum {
        /// 输入文件或目录
        #[arg(required_unless_present = "check")]
        inputs: Vec<PathBuf>,
        /// 校验算法（默认 sha256）
        #[arg(short = 'a', long, value_enum, default_value_t)]
        algorithm: ChecksumAlgorithmArg,
        /// 校验 `<digest>  <path>` 格式的 manifest（相对路径按 manifest 所在目录解析）
        #[arg(long, value_name = "MANIFEST", conflicts_with = "inputs")]
        check: Option<PathBuf>,
        /// 排除 glob 模式（可多次，如 --exclude .git --exclude "*.tmp"）
        #[arg(long = "exclude", value_name = "GLOB")]
        excludes: Vec<String>,
        /// 以 JSON 输出（机器可读）
        #[arg(long)]
        json: bool,
    },
    /// 解压压缩包
    Extract {
        /// 压缩包路径
        archive: PathBuf,
        /// 目标目录（默认当前目录）
        #[arg(short = 'd', long)]
        dest: Option<PathBuf>,
        /// 仅解压匹配的条目（glob，可多次，如 --include "docs/*"）
        #[arg(long = "include", value_name = "GLOB")]
        includes: Vec<String>,
        /// 覆盖策略
        #[arg(long, value_enum, default_value_t)]
        overwrite: OverwriteArg,
        /// 解密密码
        #[arg(long)]
        password: Option<String>,
        /// 条目名编码（如 gbk、shift_jis；默认自动检测）
        #[arg(long)]
        encoding: Option<String>,
        /// 符号链接策略
        #[arg(long, value_enum, default_value_t)]
        symlinks: SymlinkArg,
        /// 智能解压：单根目录直接解压，散文件自动装入同名子目录
        #[arg(long)]
        smart: bool,
        /// 尽力提取可读条目：跳过可恢复范围内的单条目读/CRC 错误
        #[arg(long)]
        best_effort: bool,
        /// 解压 worker 线程数（默认自动）
        #[arg(long, value_name = "N", value_parser = parse_nonzero_usize)]
        threads: Option<usize>,
        /// Squallz 流式缓冲内存上限（如 256m / 1g；不是进程 RSS 上限）
        #[arg(long, value_name = "SIZE", value_parser = parse_nonzero_size)]
        memory_limit: Option<u64>,
        /// 最大总输出字节数（支持 1g / 500m 等后缀）
        #[arg(long, value_name = "SIZE", value_parser = parse_nonzero_size)]
        max_output_bytes: Option<u64>,
        /// 最大解压条目数
        #[arg(long, value_name = "N", value_parser = parse_nonzero_u64)]
        max_entries: Option<u64>,
        /// 单条目最大解压/压缩比
        #[arg(long, value_name = "N", value_parser = parse_nonzero_u32)]
        max_compression_ratio: Option<u32>,
        /// 以 JSON 输出（机器可读）
        #[arg(long)]
        json: bool,
    },
    /// 列出压缩包内容
    List {
        /// 压缩包路径
        archive: PathBuf,
        /// 解密密码
        #[arg(long)]
        password: Option<String>,
        /// 条目名编码
        #[arg(long)]
        encoding: Option<String>,
        /// 以 JSON 输出（机器可读）
        #[arg(long)]
        json: bool,
        /// 以目录树输出（面向人工阅读）
        #[arg(long, conflicts_with = "json")]
        tree: bool,
    },
    /// 测试压缩包完整性
    Test {
        /// 压缩包路径
        archive: PathBuf,
        /// 解密密码
        #[arg(long)]
        password: Option<String>,
        /// 条目名编码
        #[arg(long)]
        encoding: Option<String>,
        /// 以 JSON 输出（机器可读）
        #[arg(long)]
        json: bool,
    },
    /// 转换压缩包格式（如 zip → 7z，流式逐条目，不解压到磁盘）
    Convert {
        /// 源压缩包路径
        src: PathBuf,
        /// 目标压缩包路径（格式由扩展名决定）
        #[arg(short, long)]
        output: PathBuf,
        /// 源压缩包解密密码
        #[arg(long)]
        password: Option<String>,
        /// 目标压缩包加密密码
        #[arg(long)]
        out_password: Option<String>,
        /// 加密目标条目名（仅支持 7z 等具备 header encryption 的格式）
        #[arg(long)]
        encrypt_names: bool,
        /// 目标压缩级别 0-9（会覆盖 --profile）
        #[arg(long, value_parser = parse_compression_level)]
        level: Option<u8>,
        /// 内置创建预设（fast=2，balanced=6，maximum=9）
        #[arg(long, value_enum)]
        profile: Option<CreateProfileArg>,
        /// 源条目名编码（如 gbk；默认自动检测）
        #[arg(long)]
        encoding: Option<String>,
        /// 压缩 worker 线程数（默认自动）
        #[arg(long, value_name = "N", value_parser = parse_nonzero_usize)]
        threads: Option<usize>,
        /// Squallz 流式缓冲内存上限（如 256m / 1g；不是进程 RSS 上限）
        #[arg(long, value_name = "SIZE", value_parser = parse_nonzero_size)]
        memory_limit: Option<u64>,
        /// 以 JSON 输出（机器可读）
        #[arg(long)]
        json: bool,
    },
    /// 操作压缩包内部的嵌套压缩包条目
    Nested {
        #[command(subcommand)]
        cmd: NestedCmd,
    },
    /// 导出 .sqz 容器为标准压缩包
    Export {
        /// 源 .sqz 容器路径
        archive: PathBuf,
        /// 输出标准压缩包路径（格式由扩展名决定）
        #[arg(short, long)]
        output: PathBuf,
        /// 目标压缩级别 0-9（会覆盖 --profile）
        #[arg(long, value_parser = parse_compression_level)]
        level: Option<u8>,
        /// 内置创建预设（fast=2，balanced=6，maximum=9）
        #[arg(long, value_enum)]
        profile: Option<CreateProfileArg>,
        /// 目标压缩包加密密码
        #[arg(long)]
        out_password: Option<String>,
        /// 压缩 worker 线程数（默认自动）
        #[arg(long, value_name = "N", value_parser = parse_nonzero_usize)]
        threads: Option<usize>,
        /// Squallz 流式缓冲内存上限（如 256m / 1g；不是进程 RSS 上限）
        #[arg(long, value_name = "SIZE", value_parser = parse_nonzero_size)]
        memory_limit: Option<u64>,
        /// 以 JSON 输出（机器可读）
        #[arg(long)]
        json: bool,
    },
    /// 修改已有压缩包：追加 / 删除 / 重命名 / 移动条目
    #[command(group(
        ArgGroup::new("ops").required(true).multiple(true).args(["add", "delete", "rename", "move_entries", "mkdir"])
    ))]
    Update {
        /// 压缩包路径
        archive: PathBuf,
        /// 追加本地文件或目录（可多次）
        #[arg(long = "add", value_name = "PATH")]
        add: Vec<PathBuf>,
        /// 创建空目录条目（可多次，如 --mkdir docs/reports/）
        #[arg(long = "mkdir", value_name = "ENTRY_PATH")]
        mkdir: Vec<String>,
        /// 按 glob 删除条目（可多次，如 --delete "*.log"）
        #[arg(long = "delete", value_name = "GLOB")]
        delete: Vec<String>,
        /// 重命名条目（可多次，格式 from=to）
        #[arg(long = "rename", value_name = "FROM=TO", value_parser = parse_rename)]
        rename: Vec<(String, String)>,
        /// 移动条目到新的压缩包内路径（可多次，格式 from=to）
        #[arg(long = "move", value_name = "FROM=TO", value_parser = parse_rename)]
        move_entries: Vec<(String, String)>,
        /// 追加本地目录时排除 glob 模式（可多次）
        #[arg(long = "exclude", value_name = "GLOB")]
        excludes: Vec<String>,
        /// 新增条目的加密密码
        #[arg(long)]
        password: Option<String>,
        /// 加密新增条目名（仅支持 7z 等具备 header encryption 的格式）
        #[arg(long)]
        encrypt_names: bool,
        /// 新增条目的压缩级别 0-9（会覆盖 --profile）
        #[arg(long, value_parser = parse_compression_level)]
        level: Option<u8>,
        /// 内置创建预设（fast=2，balanced=6，maximum=9）
        #[arg(long, value_enum)]
        profile: Option<CreateProfileArg>,
        /// 压缩 worker 线程数（默认自动）
        #[arg(long, value_name = "N", value_parser = parse_nonzero_usize)]
        threads: Option<usize>,
        /// Squallz 流式缓冲内存上限（如 256m / 1g；不是进程 RSS 上限）
        #[arg(long, value_name = "SIZE", value_parser = parse_nonzero_size)]
        memory_limit: Option<u64>,
        /// 以 JSON 输出（机器可读）
        #[arg(long)]
        json: bool,
    },
    /// 为标准压缩包生成外置 PAR2 恢复数据
    Protect {
        /// 要保护的压缩包路径
        archive: PathBuf,
        /// 恢复冗余百分比，如 10 或 10%（默认 10%；与 --tolerate-loss 互斥）
        #[arg(long, value_parser = parse_percent, conflicts_with = "tolerate_loss")]
        redundancy: Option<u8>,
        /// 为 .001 分卷集生成足以容忍 N 个最大卷丢失的 PAR2 冗余
        #[arg(long, value_name = "Nvolumes", value_parser = parse_tolerate_loss)]
        tolerate_loss: Option<u32>,
        /// 输出 .par2 索引路径（默认 <archive>.par2）
        #[arg(short, long)]
        recovery: Option<PathBuf>,
        /// 以 JSON 输出（机器可读）
        #[arg(long)]
        json: bool,
    },
    /// 校验外置 PAR2 恢复数据
    Verify {
        /// 原始压缩包路径
        archive: PathBuf,
        /// 使用外置恢复数据（当前 verify 命令固定走 PAR2）
        #[arg(long)]
        use_recovery: bool,
        /// .par2 索引路径（默认 <archive>.par2）
        #[arg(short, long)]
        recovery: Option<PathBuf>,
        /// 以 JSON 输出（机器可读）
        #[arg(long)]
        json: bool,
    },
    /// 修复压缩包：.sqz 用内嵌恢复；ZIP 可重建 central directory；--use-recovery 使用外置 PAR2
    Repair {
        /// 原始压缩包路径
        archive: PathBuf,
        /// 强制使用外置 PAR2 恢复数据
        #[arg(long)]
        use_recovery: bool,
        /// 输出路径（单文件 .sqz 省略时原地安全替换；ZIP rebuild 必填；外置 PAR2 会先修复隔离副本再写到该路径）
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// .par2 索引路径（默认 <archive>.par2）
        #[arg(short, long)]
        recovery: Option<PathBuf>,
        /// SQZ 重写输出的压缩级别 0-9（会覆盖 --profile；外置 PAR2 修复忽略）
        #[arg(long, value_parser = parse_compression_level)]
        level: Option<u8>,
        /// 内置创建预设（fast=2，balanced=6，maximum=9）
        #[arg(long, value_enum)]
        profile: Option<CreateProfileArg>,
        /// SQZ 重写输出的压缩 worker 线程数（默认自动；外置 PAR2 修复忽略）
        #[arg(long, value_name = "N", value_parser = parse_nonzero_usize)]
        threads: Option<usize>,
        /// SQZ 重写输出的 Squallz 流式缓冲内存上限（外置 PAR2 修复忽略）
        #[arg(long, value_name = "SIZE", value_parser = parse_nonzero_size)]
        memory_limit: Option<u64>,
        /// 以 JSON 输出（机器可读）
        #[arg(long)]
        json: bool,
    },
    /// 运行 JSON 批处理脚本（自动化 / CI）
    Batch {
        /// 批处理 JSON 脚本路径；相对路径默认按脚本所在目录解析
        script: PathBuf,
        /// 某个 job 失败后继续执行后续 job，并汇总所有失败
        #[arg(long)]
        keep_going: bool,
        /// 以 JSON 输出（机器可读）
        #[arg(long)]
        json: bool,
    },
    /// 诊断当前机器的引擎、外部工具与恢复能力
    Doctor {
        /// 发布/CI 严格模式：声明能力所需运行时缺失时退出 8
        #[arg(long)]
        strict: bool,
        /// 以 JSON 输出（机器可读）
        #[arg(long)]
        json: bool,
    },
    /// 列出支持的格式与能力
    Info {
        /// 以 JSON 输出（机器可读）
        #[arg(long)]
        json: bool,
    },
}

impl Cmd {
    pub fn json_requested(&self) -> bool {
        match self {
            Cmd::Compress { json, .. }
            | Cmd::Pack { json, .. }
            | Cmd::Estimate { json, .. }
            | Cmd::Duplicates { json, .. }
            | Cmd::Checksum { json, .. }
            | Cmd::Extract { json, .. }
            | Cmd::List { json, .. }
            | Cmd::Test { json, .. }
            | Cmd::Convert { json, .. }
            | Cmd::Export { json, .. }
            | Cmd::Update { json, .. }
            | Cmd::Protect { json, .. }
            | Cmd::Verify { json, .. }
            | Cmd::Repair { json, .. }
            | Cmd::Batch { json, .. }
            | Cmd::Doctor { json, .. }
            | Cmd::Info { json, .. } => *json,
            Cmd::Nested { cmd } => cmd.json_requested(),
        }
    }
}

#[derive(Subcommand)]
pub enum NestedCmd {
    /// 列出嵌套压缩包内容
    List {
        /// 外层压缩包路径
        archive: PathBuf,
        /// 外层压缩包内的嵌套压缩包条目路径
        entry: String,
        /// 外层压缩包解密密码
        #[arg(long)]
        password: Option<String>,
        /// 外层条目名编码
        #[arg(long)]
        encoding: Option<String>,
        /// 嵌套压缩包解密密码
        #[arg(long)]
        nested_password: Option<String>,
        /// 嵌套条目名编码
        #[arg(long)]
        nested_encoding: Option<String>,
        /// 以 JSON 输出（机器可读）
        #[arg(long)]
        json: bool,
        /// 以目录树输出（面向人工阅读）
        #[arg(long, conflicts_with = "json")]
        tree: bool,
    },
    /// 解压嵌套压缩包内容
    Extract {
        /// 外层压缩包路径
        archive: PathBuf,
        /// 外层压缩包内的嵌套压缩包条目路径
        entry: String,
        /// 目标目录（默认当前目录）
        #[arg(short = 'd', long)]
        dest: Option<PathBuf>,
        /// 仅解压匹配的嵌套条目（glob，可多次）
        #[arg(long = "include", value_name = "GLOB")]
        includes: Vec<String>,
        /// 覆盖策略
        #[arg(long, value_enum, default_value_t)]
        overwrite: OverwriteArg,
        /// 外层压缩包解密密码
        #[arg(long)]
        password: Option<String>,
        /// 外层条目名编码
        #[arg(long)]
        encoding: Option<String>,
        /// 嵌套压缩包解密密码
        #[arg(long)]
        nested_password: Option<String>,
        /// 嵌套条目名编码
        #[arg(long)]
        nested_encoding: Option<String>,
        /// 符号链接策略
        #[arg(long, value_enum, default_value_t)]
        symlinks: SymlinkArg,
        /// 智能解压：单根目录直接解压，散文件自动装入同名子目录
        #[arg(long)]
        smart: bool,
        /// 尽力提取可读条目：跳过可恢复范围内的单条目读/CRC 错误
        #[arg(long)]
        best_effort: bool,
        /// 解压 worker 线程数（默认自动）
        #[arg(long, value_name = "N", value_parser = parse_nonzero_usize)]
        threads: Option<usize>,
        /// Squallz 流式缓冲内存上限（如 256m / 1g；不是进程 RSS 上限）
        #[arg(long, value_name = "SIZE", value_parser = parse_nonzero_size)]
        memory_limit: Option<u64>,
        /// 最大总输出字节数（支持 1g / 500m 等后缀）
        #[arg(long, value_name = "SIZE", value_parser = parse_nonzero_size)]
        max_output_bytes: Option<u64>,
        /// 最大解压条目数
        #[arg(long, value_name = "N", value_parser = parse_nonzero_u64)]
        max_entries: Option<u64>,
        /// 单条目最大解压/压缩比
        #[arg(long, value_name = "N", value_parser = parse_nonzero_u32)]
        max_compression_ratio: Option<u32>,
        /// 以 JSON 输出（机器可读）
        #[arg(long)]
        json: bool,
    },
}

impl NestedCmd {
    fn json_requested(&self) -> bool {
        match self {
            NestedCmd::List { json, .. } | NestedCmd::Extract { json, .. } => *json,
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum, Default)]
pub enum OverwriteArg {
    /// Skip existing files
    #[default]
    Skip,
    /// Overwrite existing files
    All,
    /// Keep both files by renaming
    Rename,
    /// Ask for each conflict; non-TTY sessions degrade to skip
    Ask,
}

impl From<OverwriteArg> for OverwritePolicy {
    fn from(v: OverwriteArg) -> Self {
        match v {
            OverwriteArg::Skip => OverwritePolicy::Skip,
            OverwriteArg::All => OverwritePolicy::Overwrite,
            OverwriteArg::Rename => OverwritePolicy::RenameBoth,
            OverwriteArg::Ask => OverwritePolicy::Ask,
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum, Default)]
pub enum SymlinkArg {
    /// Restore symbolic links
    #[default]
    Preserve,
    /// Follow links and extract target contents when safe
    Follow,
    /// Skip link entries
    Skip,
}

impl From<SymlinkArg> for SymlinkPolicy {
    fn from(v: SymlinkArg) -> Self {
        match v {
            SymlinkArg::Preserve => SymlinkPolicy::Preserve,
            SymlinkArg::Follow => SymlinkPolicy::Follow,
            SymlinkArg::Skip => SymlinkPolicy::Skip,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os_args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    #[test]
    fn help_language_requires_explicit_english_value() {
        assert!(help_lang_is_english(&os_args(&[
            "sqz", "--lang", "en-US", "--help"
        ])));
        assert!(help_lang_is_english(&os_args(&[
            "sqz",
            "--lang=en",
            "list",
            "--help"
        ])));
        assert!(!help_lang_is_english(&os_args(&[
            "sqz", "--lang", "zh-CN", "--help"
        ])));
        assert!(!help_lang_is_english(&os_args(&["sqz", "--lang"])));
    }

    #[test]
    fn parse_size_accepts_plain_digits_and_binary_suffixes() {
        assert_eq!(parse_size("0"), Ok(0));
        assert_eq!(parse_size("42"), Ok(42));
        assert_eq!(parse_size("4k"), Ok(4 * 1024));
        assert_eq!(parse_size("10MB"), Ok(10 * 1024 * 1024));
        assert!(parse_size("mb").is_err());
    }

    #[test]
    fn tolerate_loss_suffixes_resolve_to_positive_counts() {
        assert_eq!(parse_tolerate_loss("2"), Ok(2));
        assert_eq!(parse_tolerate_loss("2volumes"), Ok(2));
        assert_eq!(parse_tolerate_loss("3vols"), Ok(3));
        assert_eq!(parse_tolerate_loss("4 vol"), Ok(4));
        assert!(parse_tolerate_loss("0vols").is_err());
    }

    #[test]
    fn safety_limits_merge_overrides_with_defaults() {
        let defaults = SafetyLimits::default();
        let limits = safety_limits(Some(123), None, Some(7));

        assert_eq!(limits.max_output_bytes, 123);
        assert_eq!(limits.max_entries, defaults.max_entries);
        assert_eq!(limits.max_compression_ratio, 7);
    }

    #[test]
    fn compression_level_prefers_explicit_level_then_profile_then_default() {
        assert_eq!(
            effective_compression_level(Some(3), Some(CreateProfileArg::Maximum)),
            3
        );
        assert_eq!(
            effective_compression_level(None, Some(CreateProfileArg::Balanced)),
            6
        );
        assert_eq!(effective_compression_level(None, None), 5);
    }
}
