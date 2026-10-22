//! clap argument definitions.
//!
//! Command descriptions and runtime output share the selected language pack.
//! Parsing resolves the resource keys in the derived command tree once.

use std::ffi::OsString;
use std::path::PathBuf;

use clap::{
    Arg, ArgAction, ArgGroup, Command, CommandFactory, FromArgMatches, Parser, Subcommand,
    ValueEnum,
};
use serde::Deserialize;
use squallz_core::api::{
    OverwritePolicy, ResourceOptions, SafetyLimits, SplitOutputMode, SqzInnerFormat, SymlinkPolicy,
};
use squallz_core::{ChecksumAlgorithm, CreateContentPolicy, PresetKind, SfxTarget};
use squallz_i18n::Localizer;

pub fn parse_cli<I, T>(args: I) -> Result<(Cli, Localizer), clap::Error>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString>,
{
    let args: Vec<OsString> = args.into_iter().map(Into::into).collect();
    let loc = Localizer::load(explicit_language(&args).as_deref());
    let mut cmd = localized_command(&loc);
    if loc.language() == "zh-CN" && is_check_update_help_request(&args) {
        cmd = customize_update_help(cmd, &loc);
    }
    let mut matches = cmd.try_get_matches_from_mut(args)?;
    Ok((Cli::from_arg_matches_mut(&mut matches)?, loc))
}

/// Builds the one command tree used by parsing and command-specific errors.
pub fn localized_command(loc: &Localizer) -> Command {
    fn localize(cmd: Command, loc: &Localizer) -> Command {
        let about = cmd.get_about().map(|about| loc.t(&about.to_string()));
        let cmd = match about {
            Some(about) => cmd.about(about),
            None => cmd,
        };
        cmd.mut_args(|arg| {
            let help = arg.get_help().map(|help| loc.t(&help.to_string()));
            match help {
                Some(help) => arg.help(help),
                None => arg,
            }
        })
        .mut_subcommands(|command| localize(command, loc))
    }
    localize(Cli::command(), loc)
}

fn is_check_update_help_request(args: &[OsString]) -> bool {
    let has_check_update = args
        .iter()
        .skip(1)
        .any(|arg| arg.to_string_lossy() == "check-update");
    let has_help = args.iter().skip(1).any(|arg| {
        let value = arg.to_string_lossy();
        matches!(value.as_ref(), "--help" | "-h" | "help")
    });
    has_check_update && has_help
}

fn explicit_language(args: &[OsString]) -> Option<String> {
    let mut requested = None;
    let mut iter = args.iter().skip(1);
    while let Some(arg) = iter.next() {
        let value = arg.to_string_lossy();
        if value == "--" {
            break;
        }
        if value == "--lang" {
            match iter.next() {
                Some(value) if value != "--" => {
                    requested = Some(value.to_string_lossy().into_owned());
                }
                _ => break,
            }
        } else if let Some(language) = value.strip_prefix("--lang=") {
            requested = Some(language.to_owned());
        }
    }
    requested
}

fn customize_update_help(cmd: Command, loc: &Localizer) -> Command {
    cmd.mut_arg("output_style", |arg| {
        arg.help(loc.t("cli.help.check_update.style"))
            .hide_possible_values(true)
            .hide_default_value(true)
    })
    .mut_arg("color", |arg| {
        arg.help(loc.t("cli.help.check_update.color"))
            .hide_possible_values(true)
            .hide_default_value(true)
    })
    .mut_arg("accent", |arg| {
        arg.help(loc.t("cli.help.check_update.palette"))
            .hide_possible_values(true)
            .hide_default_value(true)
    })
    .mut_subcommand("check-update", |cmd| {
        cmd.help_template(loc.t("cli.help.check_update.template"))
            .disable_help_flag(true)
            .arg(
                Arg::new("help")
                    .short('h')
                    .long("help")
                    .action(ArgAction::Help)
                    .help(loc.t("cli.help.check_update.help")),
            )
    })
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
#[command(name = "sqz", version, about = "cli.help.root.about")]
pub struct Cli {
    /// cli.help.root.lang
    #[arg(long, global = true)]
    pub lang: Option<String>,
    /// cli.help.root.quiet
    #[arg(short, long, global = true)]
    pub quiet: bool,
    /// cli.help.root.verbose
    #[arg(short, long, global = true, conflicts_with = "quiet")]
    pub verbose: bool,
    /// cli.help.root.output_style
    #[arg(long = "style", global = true, value_enum, default_value_t)]
    pub output_style: OutputStyleArg,
    /// cli.help.root.color
    #[arg(long, global = true, value_enum, default_value_t)]
    pub color: ColorArg,
    /// cli.help.root.accent
    #[arg(long = "palette", global = true, value_enum, default_value_t)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CreateProfileArg {
    /// Quick sharing, lower CPU pressure
    Fast,
    /// Good ratio without making the machine feel busy
    Balanced,
    /// Best ratio for long-term storage
    Maximum,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum CreateContentPolicyArg {
    /// Remove common macOS helper files while preserving ordinary hidden files
    CrossPlatformClean,
    /// Keep selected files unless an explicit --exclude rule removes them
    KeepAllFiles,
    /// Apply only explicit --exclude rules
    Custom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Default)]
pub enum SplitModeArg {
    /// Format-independent .001/.002 byte volumes
    #[default]
    Generic,
    /// Interoperable native ZIP .z01 … .zip or Split WIM .swm volumes
    Native,
}

impl From<SplitModeArg> for SplitOutputMode {
    fn from(value: SplitModeArg) -> Self {
        match value {
            SplitModeArg::Generic => Self::Generic,
            SplitModeArg::Native => Self::Native,
        }
    }
}

impl From<CreateContentPolicyArg> for CreateContentPolicy {
    fn from(value: CreateContentPolicyArg) -> Self {
        match value {
            CreateContentPolicyArg::CrossPlatformClean => Self::CrossPlatformClean,
            CreateContentPolicyArg::KeepAllFiles => Self::KeepAllFiles,
            CreateContentPolicyArg::Custom => Self::Custom,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum SfxTargetArg {
    /// Windows Portable Executable
    Windows,
    /// Linux ELF executable
    Linux,
    /// macOS app bundle target
    Macos,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum PresetKindArg {
    Create,
    Extract,
}

impl From<PresetKindArg> for PresetKind {
    fn from(value: PresetKindArg) -> Self {
        match value {
            PresetKindArg::Create => Self::Create,
            PresetKindArg::Extract => Self::Extract,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum PresetBindingArg {
    AppCreate,
    AppExtract,
    FileManagerCreate,
    FileManagerExtract,
}

impl From<SfxTargetArg> for SfxTarget {
    fn from(value: SfxTargetArg) -> Self {
        match value {
            SfxTargetArg::Windows => Self::Windows,
            SfxTargetArg::Linux => Self::Linux,
            SfxTargetArg::Macos => Self::Macos,
        }
    }
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
    Sha256,
    /// BLAKE3, fast modern content hashing
    Blake3,
    /// SHA-512, wider SHA-2 digest used by some manifests
    Sha512,
    /// SHA-384, used by some signed release manifests
    Sha384,
    /// SHA-224, used by some uncommon manifests
    Sha224,
    /// SHA-1, a weak algorithm for verification-only workflows
    Sha1,
    /// MD5, a weak algorithm for verification-only workflows
    Md5,
    /// ZIP-compatible CRC-32
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
    /// Never emit ANSI color
    Never,
}

impl ColorArg {
    pub fn enabled(self, stream_is_terminal: bool) -> bool {
        match self {
            Self::Always => true,
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
    /// Ocean palette with sky-blue primary and teal highlights
    Ocean,
    /// Jade palette with fresh green-cyan primary and Squallz teal highlights
    Jade,
    /// Warm sunset palette with orange and rose highlights
    Sunset,
    /// Violet palette
    Violet,
    /// Monochrome high-contrast palette
    Mono,
}

#[derive(clap::Args)]
pub struct CompressArgs {
    /// cli.help.shared.inputs
    #[arg(required = true)]
    pub inputs: Vec<PathBuf>,
    /// cli.help.compress.output
    #[arg(short, long)]
    pub output: PathBuf,
    /// cli.help.compress.format
    #[arg(long)]
    pub format: Option<String>,
    /// cli.help.compress.level
    #[arg(long, value_parser = parse_compression_level)]
    pub level: Option<u8>,
    /// cli.help.shared.profile
    #[arg(long, value_enum)]
    pub profile: Option<CreateProfileArg>,
    /// cli.help.compress.password
    #[arg(long)]
    pub password: Option<String>,
    /// cli.help.compress.encrypt_names
    #[arg(long)]
    pub encrypt_names: bool,
    /// cli.help.shared.excludes
    #[arg(long = "exclude", value_name = "GLOB")]
    pub excludes: Vec<String>,
    /// cli.help.shared.content_policy
    #[arg(long, value_enum)]
    pub content_policy: Option<CreateContentPolicyArg>,
    /// cli.help.shared.split
    #[arg(long, value_name = "SIZE", value_parser = parse_size)]
    pub split: Option<u64>,
    /// cli.help.shared.split_mode
    #[arg(long, value_enum, requires = "split")]
    pub split_mode: Option<SplitModeArg>,
    /// cli.help.shared.threads
    #[arg(long, value_name = "N", value_parser = parse_nonzero_usize)]
    pub threads: Option<usize>,
    /// cli.help.shared.memory_limit
    #[arg(long, value_name = "SIZE", value_parser = parse_nonzero_size)]
    pub memory_limit: Option<u64>,
    /// cli.help.compress.test_after_create
    #[arg(long)]
    pub test_after_create: bool,
    /// cli.help.shared.json
    #[arg(long)]
    pub json: bool,
}

#[derive(clap::Args)]
pub struct PackArgs {
    /// cli.help.shared.inputs
    #[arg(required = true)]
    pub inputs: Vec<PathBuf>,
    /// cli.help.pack.output
    #[arg(short, long)]
    pub output: PathBuf,
    /// cli.help.pack.level
    #[arg(long, value_parser = parse_compression_level)]
    pub level: Option<u8>,
    /// cli.help.shared.profile
    #[arg(long, value_enum)]
    pub profile: Option<CreateProfileArg>,
    /// cli.help.pack.inner_format
    #[arg(long, default_value_t = SqzInnerFormat::Sqz)]
    pub inner_format: SqzInnerFormat,
    /// cli.help.pack.recovery
    #[arg(long, default_value = "25%", value_parser = parse_percent)]
    pub recovery: u8,
    /// cli.help.shared.excludes
    #[arg(long = "exclude", value_name = "GLOB")]
    pub excludes: Vec<String>,
    /// cli.help.shared.content_policy
    #[arg(long, value_enum)]
    pub content_policy: Option<CreateContentPolicyArg>,
    /// cli.help.pack.split
    #[arg(long, value_name = "SIZE", value_parser = parse_size)]
    pub split: Option<u64>,
    /// cli.help.shared.threads
    #[arg(long, value_name = "N", value_parser = parse_nonzero_usize)]
    pub threads: Option<usize>,
    /// cli.help.shared.memory_limit
    #[arg(long, value_name = "SIZE", value_parser = parse_nonzero_size)]
    pub memory_limit: Option<u64>,
    /// cli.help.pack.test_after_create
    #[arg(long)]
    pub test_after_create: bool,
    /// cli.help.shared.json
    #[arg(long)]
    pub json: bool,
}

#[derive(Subcommand)]
pub enum Cmd {
    /// cli.help.file_manager.about
    #[command(hide = true)]
    FileManager {
        /// cli.help.file_manager.action
        action: squallz_core::ExternalTaskAction,
        /// cli.help.shared.inputs
        #[arg(required = true)]
        paths: Vec<PathBuf>,
        /// cli.help.shared.json
        #[arg(long)]
        json: bool,
    },
    /// cli.help.compress.about
    Compress(CompressArgs),
    /// cli.help.pack.about
    Pack(PackArgs),
    /// cli.help.sfx.about
    Sfx {
        #[command(subcommand)]
        cmd: SfxCmd,
    },
    /// cli.help.estimate.about
    Estimate {
        /// cli.help.shared.inputs
        #[arg(required = true)]
        inputs: Vec<PathBuf>,
        /// cli.help.shared.excludes
        #[arg(long = "exclude", value_name = "GLOB")]
        excludes: Vec<String>,
        /// cli.help.shared.content_policy
        #[arg(long, value_enum)]
        content_policy: Option<CreateContentPolicyArg>,
        /// cli.help.estimate.output
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// cli.help.shared.json
        #[arg(long)]
        json: bool,
    },
    /// cli.help.duplicates.about
    Duplicates {
        /// cli.help.duplicates.inputs
        #[arg(required = true)]
        inputs: Vec<PathBuf>,
        /// cli.help.shared.excludes
        #[arg(long = "exclude", value_name = "GLOB")]
        excludes: Vec<String>,
        /// cli.help.duplicates.min_size
        #[arg(long, value_name = "SIZE", default_value = "1", value_parser = parse_size)]
        min_size: u64,
        /// cli.help.shared.json
        #[arg(long)]
        json: bool,
    },
    /// cli.help.checksum.about
    Checksum {
        /// cli.help.checksum.inputs
        #[arg(required_unless_present = "check")]
        inputs: Vec<PathBuf>,
        /// cli.help.checksum.algorithm
        #[arg(short = 'a', long, value_enum, default_value_t)]
        algorithm: ChecksumAlgorithmArg,
        /// cli.help.checksum.check
        #[arg(long, value_name = "MANIFEST", conflicts_with = "inputs")]
        check: Option<PathBuf>,
        /// cli.help.shared.excludes
        #[arg(long = "exclude", value_name = "GLOB")]
        excludes: Vec<String>,
        /// cli.help.shared.json
        #[arg(long)]
        json: bool,
    },
    /// cli.help.extract.about
    Extract {
        /// cli.help.shared.archive
        archive: PathBuf,
        /// cli.help.shared.dest
        #[arg(short = 'd', long)]
        dest: Option<PathBuf>,
        /// cli.help.extract.includes
        #[arg(long = "include", value_name = "GLOB")]
        includes: Vec<String>,
        /// cli.help.shared.overwrite
        #[arg(long, value_enum, default_value_t)]
        overwrite: OverwriteArg,
        /// cli.help.shared.password
        #[arg(long)]
        password: Option<String>,
        /// cli.help.extract.encoding
        #[arg(long)]
        encoding: Option<String>,
        /// cli.help.shared.symlinks
        #[arg(long, value_enum, default_value_t)]
        symlinks: SymlinkArg,
        /// cli.help.shared.smart
        #[arg(long)]
        smart: bool,
        /// cli.help.shared.best_effort
        #[arg(long)]
        best_effort: bool,
        /// cli.help.shared.extract_threads
        #[arg(long, value_name = "N", value_parser = parse_nonzero_usize)]
        threads: Option<usize>,
        /// cli.help.shared.memory_limit
        #[arg(long, value_name = "SIZE", value_parser = parse_nonzero_size)]
        memory_limit: Option<u64>,
        /// cli.help.shared.max_output_bytes
        #[arg(long, value_name = "SIZE", value_parser = parse_nonzero_size)]
        max_output_bytes: Option<u64>,
        /// cli.help.shared.max_entries
        #[arg(long, value_name = "N", value_parser = parse_nonzero_u64)]
        max_entries: Option<u64>,
        /// cli.help.shared.max_compression_ratio
        #[arg(long, value_name = "N", value_parser = parse_nonzero_u32)]
        max_compression_ratio: Option<u32>,
        /// cli.help.shared.json
        #[arg(long)]
        json: bool,
    },
    /// cli.help.list.about
    List {
        /// cli.help.shared.archive
        archive: PathBuf,
        /// cli.help.shared.password
        #[arg(long)]
        password: Option<String>,
        /// cli.help.shared.encoding
        #[arg(long)]
        encoding: Option<String>,
        /// cli.help.list.search
        #[arg(long, value_name = "QUERY")]
        search: Option<String>,
        /// cli.help.shared.json
        #[arg(long)]
        json: bool,
        /// cli.help.shared.tree
        #[arg(long, conflicts_with = "json")]
        tree: bool,
    },
    /// cli.help.test.about
    Test {
        /// cli.help.shared.archive
        archive: PathBuf,
        /// cli.help.shared.password
        #[arg(long)]
        password: Option<String>,
        /// cli.help.shared.encoding
        #[arg(long)]
        encoding: Option<String>,
        /// cli.help.test.max_output_bytes
        #[arg(long, value_name = "SIZE", value_parser = parse_nonzero_size)]
        max_output_bytes: Option<u64>,
        /// cli.help.test.max_entries
        #[arg(long, value_name = "N", value_parser = parse_nonzero_u64)]
        max_entries: Option<u64>,
        /// cli.help.shared.max_compression_ratio
        #[arg(long, value_name = "N", value_parser = parse_nonzero_u32)]
        max_compression_ratio: Option<u32>,
        /// cli.help.shared.json
        #[arg(long)]
        json: bool,
    },
    /// cli.help.convert.about
    Convert {
        /// cli.help.convert.src
        src: PathBuf,
        /// cli.help.convert.output
        #[arg(short, long)]
        output: PathBuf,
        /// cli.help.convert.password
        #[arg(long)]
        password: Option<String>,
        /// cli.help.shared.out_password
        #[arg(long)]
        out_password: Option<String>,
        /// cli.help.convert.encrypt_names
        #[arg(long)]
        encrypt_names: bool,
        /// cli.help.shared.split
        #[arg(long, value_name = "SIZE", value_parser = parse_size)]
        split: Option<u64>,
        /// cli.help.shared.split_mode
        #[arg(long, value_enum, requires = "split")]
        split_mode: Option<SplitModeArg>,
        /// cli.help.shared.level
        #[arg(long, value_parser = parse_compression_level)]
        level: Option<u8>,
        /// cli.help.shared.profile
        #[arg(long, value_enum)]
        profile: Option<CreateProfileArg>,
        /// cli.help.convert.encoding
        #[arg(long)]
        encoding: Option<String>,
        /// cli.help.shared.threads
        #[arg(long, value_name = "N", value_parser = parse_nonzero_usize)]
        threads: Option<usize>,
        /// cli.help.shared.memory_limit
        #[arg(long, value_name = "SIZE", value_parser = parse_nonzero_size)]
        memory_limit: Option<u64>,
        /// cli.help.convert.force
        #[arg(long)]
        force: bool,
        /// cli.help.shared.json
        #[arg(long)]
        json: bool,
    },
    /// cli.help.nested.about
    Nested {
        #[command(subcommand)]
        cmd: NestedCmd,
    },
    /// cli.help.export.about
    Export {
        /// cli.help.export.archive
        archive: PathBuf,
        /// cli.help.export.output
        #[arg(short, long)]
        output: PathBuf,
        /// cli.help.shared.level
        #[arg(long, value_parser = parse_compression_level)]
        level: Option<u8>,
        /// cli.help.shared.profile
        #[arg(long, value_enum)]
        profile: Option<CreateProfileArg>,
        /// cli.help.shared.out_password
        #[arg(long)]
        out_password: Option<String>,
        /// cli.help.shared.threads
        #[arg(long, value_name = "N", value_parser = parse_nonzero_usize)]
        threads: Option<usize>,
        /// cli.help.shared.memory_limit
        #[arg(long, value_name = "SIZE", value_parser = parse_nonzero_size)]
        memory_limit: Option<u64>,
        /// cli.help.export.force
        #[arg(long)]
        force: bool,
        /// cli.help.shared.json
        #[arg(long)]
        json: bool,
    },
    /// cli.help.update.about
    #[command(group(
        ArgGroup::new("ops").required(true).multiple(true).args(["add", "delete", "delete_entries", "rename", "move_entries", "mkdir"])
    ))]
    Update {
        /// cli.help.shared.archive
        archive: PathBuf,
        /// cli.help.update.add
        #[arg(long = "add", value_name = "PATH")]
        add: Vec<PathBuf>,
        /// cli.help.update.mkdir
        #[arg(long = "mkdir", value_name = "ENTRY_PATH")]
        mkdir: Vec<String>,
        /// cli.help.update.delete
        #[arg(long = "delete", value_name = "GLOB")]
        delete: Vec<String>,
        /// cli.help.update.delete_entries
        #[arg(long = "delete-entry", value_name = "ENTRY_PATH")]
        delete_entries: Vec<String>,
        /// cli.help.update.encoding
        #[arg(long)]
        encoding: Option<String>,
        /// cli.help.update.rename
        #[arg(long = "rename", value_name = "FROM", num_args = 1, requires = "rename_to",
              action = ArgAction::Append, value_parser = clap::builder::NonEmptyStringValueParser::new())]
        rename: Vec<String>,
        /// cli.help.update.rename_to
        #[arg(long = "rename-to", value_name = "TO", num_args = 1, requires = "rename",
              action = ArgAction::Append, value_parser = clap::builder::NonEmptyStringValueParser::new())]
        rename_to: Vec<String>,
        /// cli.help.update.move_entries
        #[arg(long = "move", value_name = "FROM", num_args = 1, requires = "move_to",
              action = ArgAction::Append, value_parser = clap::builder::NonEmptyStringValueParser::new())]
        move_entries: Vec<String>,
        /// cli.help.update.move_to
        #[arg(long = "move-to", value_name = "TO", num_args = 1, requires = "move_entries",
              action = ArgAction::Append, value_parser = clap::builder::NonEmptyStringValueParser::new())]
        move_to: Vec<String>,
        /// cli.help.update.excludes
        #[arg(long = "exclude", value_name = "GLOB")]
        excludes: Vec<String>,
        /// cli.help.update.content_policy
        #[arg(long, value_enum)]
        content_policy: Option<CreateContentPolicyArg>,
        /// cli.help.update.password
        #[arg(long)]
        password: Option<String>,
        /// cli.help.update.encrypt_names
        #[arg(long)]
        encrypt_names: bool,
        /// cli.help.update.level
        #[arg(long, value_parser = parse_compression_level)]
        level: Option<u8>,
        /// cli.help.shared.profile
        #[arg(long, value_enum)]
        profile: Option<CreateProfileArg>,
        /// cli.help.shared.threads
        #[arg(long, value_name = "N", value_parser = parse_nonzero_usize)]
        threads: Option<usize>,
        /// cli.help.shared.memory_limit
        #[arg(long, value_name = "SIZE", value_parser = parse_nonzero_size)]
        memory_limit: Option<u64>,
        /// cli.help.shared.json
        #[arg(long)]
        json: bool,
    },
    /// cli.help.protect.about
    Protect {
        /// cli.help.protect.archive
        archive: PathBuf,
        /// cli.help.protect.redundancy
        #[arg(long, value_parser = parse_percent, conflicts_with = "tolerate_loss")]
        redundancy: Option<u8>,
        /// cli.help.protect.tolerate_loss
        #[arg(long, value_name = "Nvolumes", value_parser = parse_tolerate_loss)]
        tolerate_loss: Option<u32>,
        /// cli.help.protect.recovery
        #[arg(short, long)]
        recovery: Option<PathBuf>,
        /// cli.help.shared.json
        #[arg(long)]
        json: bool,
    },
    /// cli.help.verify.about
    Verify {
        /// cli.help.shared.original_archive
        archive: PathBuf,
        /// cli.help.verify.use_recovery
        #[arg(long)]
        use_recovery: bool,
        /// cli.help.shared.recovery
        #[arg(short, long)]
        recovery: Option<PathBuf>,
        /// cli.help.shared.json
        #[arg(long)]
        json: bool,
    },
    /// cli.help.repair.about
    Repair {
        /// cli.help.shared.original_archive
        archive: PathBuf,
        /// cli.help.repair.use_recovery
        #[arg(long)]
        use_recovery: bool,
        /// cli.help.repair.output
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// cli.help.repair.output_dir
        #[arg(
            long,
            value_name = "DIR",
            conflicts_with = "output",
            requires = "use_recovery"
        )]
        output_dir: Option<PathBuf>,
        /// cli.help.shared.recovery
        #[arg(short, long)]
        recovery: Option<PathBuf>,
        /// cli.help.repair.level
        #[arg(long, value_parser = parse_compression_level)]
        level: Option<u8>,
        /// cli.help.shared.profile
        #[arg(long, value_enum)]
        profile: Option<CreateProfileArg>,
        /// cli.help.repair.threads
        #[arg(long, value_name = "N", value_parser = parse_nonzero_usize)]
        threads: Option<usize>,
        /// cli.help.repair.memory_limit
        #[arg(long, value_name = "SIZE", value_parser = parse_nonzero_size)]
        memory_limit: Option<u64>,
        /// cli.help.shared.json
        #[arg(long)]
        json: bool,
    },
    /// cli.help.batch.about
    Batch {
        /// cli.help.batch.script
        script: PathBuf,
        /// cli.help.batch.keep_going
        #[arg(long)]
        keep_going: bool,
        /// cli.help.shared.json
        #[arg(long)]
        json: bool,
    },
    /// cli.help.check_update.about
    CheckUpdate {
        /// cli.help.shared.json
        #[arg(long)]
        json: bool,
    },
    /// cli.help.doctor.about
    Doctor {
        /// cli.help.doctor.strict
        #[arg(long)]
        strict: bool,
        /// cli.help.shared.json
        #[arg(long)]
        json: bool,
    },
    /// cli.help.preset.about
    Preset {
        #[command(subcommand)]
        cmd: PresetCmd,
    },
    /// cli.help.info.about
    Info {
        /// cli.help.shared.json
        #[arg(long)]
        json: bool,
    },
}

impl Cmd {
    pub fn json_requested(&self) -> bool {
        match self {
            Cmd::FileManager { json, .. }
            | Cmd::Compress(CompressArgs { json, .. })
            | Cmd::Pack(PackArgs { json, .. })
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
            | Cmd::CheckUpdate { json }
            | Cmd::Doctor { json, .. }
            | Cmd::Info { json, .. } => *json,
            Cmd::Sfx { cmd } => cmd.json_requested(),
            Cmd::Nested { cmd } => cmd.json_requested(),
            Cmd::Preset { cmd } => cmd.json_requested(),
        }
    }
}

#[derive(Subcommand)]
pub enum PresetCmd {
    /// cli.help.preset.list.about
    List {
        /// cli.help.preset.list.kind
        #[arg(long, value_enum)]
        kind: Option<PresetKindArg>,
        /// cli.help.preset.list.json
        #[arg(long)]
        json: bool,
    },
    /// cli.help.preset.show.about
    Show {
        /// cli.help.shared.id
        id: String,
        /// cli.help.shared.json
        #[arg(long)]
        json: bool,
    },
    /// cli.help.preset.clone.about
    Clone {
        /// cli.help.preset.clone.source_id
        source_id: String,
        /// cli.help.preset.clone.new_id
        new_id: String,
        /// cli.help.preset.clone.label
        #[arg(long)]
        label: String,
        /// cli.help.shared.json
        #[arg(long)]
        json: bool,
    },
    /// cli.help.preset.update.about
    Update {
        /// cli.help.preset.update.id
        id: String,
        /// cli.help.preset.update.file
        #[arg(long)]
        file: PathBuf,
        /// cli.help.shared.json
        #[arg(long)]
        json: bool,
    },
    /// cli.help.preset.delete.about
    Delete {
        /// cli.help.preset.delete.id
        id: String,
        /// cli.help.shared.json
        #[arg(long)]
        json: bool,
    },
    /// cli.help.preset.bind.about
    Bind {
        /// cli.help.shared.slot
        #[arg(value_enum)]
        slot: PresetBindingArg,
        /// cli.help.shared.id
        id: String,
        /// cli.help.shared.json
        #[arg(long)]
        json: bool,
    },
    /// cli.help.preset.unbind.about
    Unbind {
        /// cli.help.shared.slot
        #[arg(value_enum)]
        slot: PresetBindingArg,
        /// cli.help.shared.json
        #[arg(long)]
        json: bool,
    },
    /// cli.help.preset.path.about
    Path {
        /// cli.help.shared.json
        #[arg(long)]
        json: bool,
    },
}

impl PresetCmd {
    fn json_requested(&self) -> bool {
        match self {
            Self::List { json, .. }
            | Self::Show { json, .. }
            | Self::Clone { json, .. }
            | Self::Update { json, .. }
            | Self::Delete { json, .. }
            | Self::Bind { json, .. }
            | Self::Unbind { json, .. }
            | Self::Path { json } => *json,
        }
    }
}

#[derive(Subcommand)]
pub enum SfxCmd {
    /// cli.help.sfx.create.about
    Create {
        /// cli.help.sfx.create.archive
        archive: PathBuf,
        /// cli.help.sfx.create.output
        #[arg(short, long)]
        output: PathBuf,
        /// cli.help.sfx.create.target
        #[arg(long, value_enum)]
        target: SfxTargetArg,
        /// cli.help.sfx.create.stub
        #[arg(long)]
        stub: Option<PathBuf>,
        /// cli.help.sfx.create.force
        #[arg(long)]
        force: bool,
        /// cli.help.sfx.create.memory_limit
        #[arg(long, value_name = "SIZE", value_parser = parse_nonzero_size)]
        memory_limit: Option<u64>,
        /// cli.help.shared.json
        #[arg(long)]
        json: bool,
    },
    /// cli.help.sfx.inspect.about
    Inspect {
        /// cli.help.sfx.inspect.file
        file: PathBuf,
        /// cli.help.sfx.verification_memory_limit
        #[arg(long, value_name = "SIZE", value_parser = parse_nonzero_size)]
        memory_limit: Option<u64>,
        /// cli.help.shared.json
        #[arg(long)]
        json: bool,
    },
    /// cli.help.sfx.publish_macos.about
    PublishMacos {
        /// cli.help.sfx.publish_macos.source
        source: PathBuf,
        /// cli.help.sfx.publish_macos.output
        #[arg(short, long)]
        output: PathBuf,
        /// cli.help.sfx.publish_macos.identity
        #[arg(long, value_name = "IDENTITY")]
        identity: String,
        /// cli.help.sfx.publish_macos.notary_profile
        #[arg(long, value_name = "PROFILE")]
        notary_profile: String,
        /// cli.help.sfx.verification_memory_limit
        #[arg(long, value_name = "SIZE", value_parser = parse_nonzero_size)]
        memory_limit: Option<u64>,
        /// cli.help.shared.json
        #[arg(long)]
        json: bool,
    },
}

impl SfxCmd {
    fn json_requested(&self) -> bool {
        match self {
            Self::Create { json, .. }
            | Self::Inspect { json, .. }
            | Self::PublishMacos { json, .. } => *json,
        }
    }
}

#[derive(Subcommand)]
pub enum NestedCmd {
    /// cli.help.nested.list.about
    List {
        /// cli.help.nested.outer_archive
        archive: PathBuf,
        /// cli.help.shared.entry
        entry: String,
        /// cli.help.nested.outer_password
        #[arg(long)]
        password: Option<String>,
        /// cli.help.nested.outer_encoding
        #[arg(long)]
        encoding: Option<String>,
        /// cli.help.shared.nested_password
        #[arg(long)]
        nested_password: Option<String>,
        /// cli.help.shared.nested_encoding
        #[arg(long)]
        nested_encoding: Option<String>,
        /// cli.help.nested.list.search
        #[arg(long, value_name = "QUERY")]
        search: Option<String>,
        /// cli.help.shared.json
        #[arg(long)]
        json: bool,
        /// cli.help.shared.tree
        #[arg(long, conflicts_with = "json")]
        tree: bool,
    },
    /// cli.help.nested.extract.about
    Extract {
        /// cli.help.nested.outer_archive
        archive: PathBuf,
        /// cli.help.shared.entry
        entry: String,
        /// cli.help.shared.dest
        #[arg(short = 'd', long)]
        dest: Option<PathBuf>,
        /// cli.help.nested.extract.includes
        #[arg(long = "include", value_name = "GLOB")]
        includes: Vec<String>,
        /// cli.help.shared.overwrite
        #[arg(long, value_enum, default_value_t)]
        overwrite: OverwriteArg,
        /// cli.help.nested.outer_password
        #[arg(long)]
        password: Option<String>,
        /// cli.help.nested.outer_encoding
        #[arg(long)]
        encoding: Option<String>,
        /// cli.help.shared.nested_password
        #[arg(long)]
        nested_password: Option<String>,
        /// cli.help.shared.nested_encoding
        #[arg(long)]
        nested_encoding: Option<String>,
        /// cli.help.shared.symlinks
        #[arg(long, value_enum, default_value_t)]
        symlinks: SymlinkArg,
        /// cli.help.shared.smart
        #[arg(long)]
        smart: bool,
        /// cli.help.shared.best_effort
        #[arg(long)]
        best_effort: bool,
        /// cli.help.shared.extract_threads
        #[arg(long, value_name = "N", value_parser = parse_nonzero_usize)]
        threads: Option<usize>,
        /// cli.help.shared.memory_limit
        #[arg(long, value_name = "SIZE", value_parser = parse_nonzero_size)]
        memory_limit: Option<u64>,
        /// cli.help.shared.max_output_bytes
        #[arg(long, value_name = "SIZE", value_parser = parse_nonzero_size)]
        max_output_bytes: Option<u64>,
        /// cli.help.shared.max_entries
        #[arg(long, value_name = "N", value_parser = parse_nonzero_u64)]
        max_entries: Option<u64>,
        /// cli.help.shared.max_compression_ratio
        #[arg(long, value_name = "N", value_parser = parse_nonzero_u32)]
        max_compression_ratio: Option<u32>,
        /// cli.help.shared.json
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
    use std::path::Path;

    use clap::error::ErrorKind;

    use super::*;

    fn os_args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    #[test]
    fn explicit_help_language_accepts_both_clap_forms() {
        assert_eq!(
            explicit_language(&os_args(&["sqz", "--lang", "en-US", "--help"])),
            Some("en-US".to_owned())
        );
        assert_eq!(
            explicit_language(&os_args(&["sqz", "--lang=en", "list", "--help"])),
            Some("en".to_owned())
        );
        assert_eq!(
            explicit_language(&os_args(&["sqz", "--lang", "zh-CN", "--help"])),
            Some("zh-CN".to_owned())
        );
        assert_eq!(explicit_language(&os_args(&["sqz", "--lang"])), None);
    }

    #[test]
    fn check_update_chinese_help_has_localized_structure() {
        let loc = Localizer::with_user_dir(Some("zh-CN"), None);
        let mut cmd = customize_update_help(localized_command(&loc), &loc);
        let error = cmd
            .try_get_matches_from_mut(["sqz", "--lang", "zh-CN", "check-update", "--help"])
            .expect_err("help should stop argument parsing");
        assert_eq!(error.kind(), ErrorKind::DisplayHelp);

        let help = error.to_string();
        assert!(help.contains("用法: sqz check-update"));
        assert!(help.contains("选项:"));
        assert!(help.contains("显示帮助"));
        assert!(!help.contains("Usage:"));
        assert!(!help.contains("Options:"));
        assert!(!help.contains("Possible values:"));
        assert!(!help.contains("Traditional Linux-style output"));
        assert!(!help.contains("[default:"));
        assert!(!help.contains("[aliases:"));
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

    #[test]
    fn content_policy_cli_values_use_kebab_case() {
        assert_eq!(
            CreateContentPolicyArg::from_str("cross-platform-clean", false),
            Ok(CreateContentPolicyArg::CrossPlatformClean)
        );
        assert_eq!(
            CreateContentPolicyArg::from_str("keep-all-files", false),
            Ok(CreateContentPolicyArg::KeepAllFiles)
        );
        assert_eq!(
            CreateContentPolicyArg::from_str("custom", false),
            Ok(CreateContentPolicyArg::Custom)
        );
        assert!(CreateContentPolicyArg::from_str("cross_platform_clean", false).is_err());
    }

    #[test]
    fn file_manager_actions_accept_raw_paths_and_json() {
        let (parsed, _) = parse_cli([
            "sqz",
            "file-manager",
            "compress-to-7z",
            "--json",
            "--",
            "-input.txt",
        ])
        .expect("file-manager action should parse");
        assert!(parsed.cmd.json_requested());
        assert!(
            matches!(parsed.cmd, Cmd::FileManager { action: squallz_core::ExternalTaskAction::CompressTo7z, paths, .. } if paths == [PathBuf::from("-input.txt")])
        );
        assert!(parse_cli(["sqz", "file-manager", "invalid", "input.txt"]).is_err());
        assert!(parse_cli([
            "sqz",
            "compress",
            "input.txt",
            "-o",
            "out.7z",
            "--file-manager-preset"
        ])
        .is_err());
    }

    #[test]
    fn preset_management_commands_keep_json_error_mode() {
        let (parsed, _) = parse_cli([
            "sqz",
            "preset",
            "bind",
            "file-manager-create",
            "user.create.portable",
            "--json",
        ])
        .expect("preset binding command should parse");
        assert!(parsed.cmd.json_requested());
        assert!(matches!(
            parsed.cmd,
            Cmd::Preset {
                cmd: PresetCmd::Bind {
                    slot: PresetBindingArg::FileManagerCreate,
                    ..
                }
            }
        ));

        assert!(parse_cli(["sqz", "preset", "update", "user.create.portable"]).is_err());
    }

    #[test]
    fn create_commands_accept_integrity_testing_after_commit() {
        let (compress, _) = parse_cli([
            "sqz",
            "compress",
            "input.txt",
            "-o",
            "output.7z",
            "--test-after-create",
        ])
        .expect("compress integrity flag should parse");
        assert!(matches!(
            compress.cmd,
            Cmd::Compress(CompressArgs {
                test_after_create: true,
                ..
            })
        ));

        let (pack, _) = parse_cli([
            "sqz",
            "pack",
            "input.txt",
            "-o",
            "output.sqz",
            "--test-after-create",
        ])
        .expect("pack integrity flag should parse");
        assert!(matches!(
            pack.cmd,
            Cmd::Pack(PackArgs {
                test_after_create: true,
                ..
            })
        ));
    }

    #[test]
    fn software_update_check_is_distinct_from_archive_update() {
        let (parsed, _) = parse_cli(["sqz", "check-update", "--json"])
            .expect("software update check should parse");
        assert!(parsed.cmd.json_requested());
        assert!(matches!(parsed.cmd, Cmd::CheckUpdate { json: true }));

        assert!(parse_cli(["sqz", "check-update", "archive.zip"]).is_err());
        assert!(parse_cli(["sqz", "update", "archive.zip"]).is_err());
    }

    #[test]
    fn macos_sfx_publish_requires_explicit_identity_profile_and_output() {
        let (parsed, _) = parse_cli([
            "sqz",
            "sfx",
            "publish-macos",
            "Unsigned.app",
            "-o",
            "Published.app",
            "--identity",
            "Developer ID Application: Example (TEAM123456)",
            "--notary-profile",
            "squallz-release",
            "--json",
        ])
        .expect("complete macOS SFX publishing arguments should parse");
        assert!(parsed.cmd.json_requested());
        assert!(matches!(
            parsed.cmd,
            Cmd::Sfx {
                cmd: SfxCmd::PublishMacos {
                    source,
                    output,
                    identity,
                    notary_profile,
                    json: true,
                    ..
                }
            } if source.as_path() == Path::new("Unsigned.app")
                && output.as_path() == Path::new("Published.app")
                && identity == "Developer ID Application: Example (TEAM123456)"
                && notary_profile == "squallz-release"
        ));

        assert!(parse_cli([
            "sqz",
            "sfx",
            "publish-macos",
            "Unsigned.app",
            "-o",
            "Published.app",
            "--identity",
            "Developer ID Application: Example (TEAM123456)",
        ])
        .is_err());
    }

    #[test]
    fn par2_directory_repair_requires_an_explicit_non_file_destination() {
        let (parsed, _) = parse_cli([
            "sqz",
            "repair",
            "archive.zip.001",
            "--use-recovery",
            "--output-dir",
            "repaired-set",
        ])
        .expect("PAR2 directory output should parse");
        assert!(matches!(
            parsed.cmd,
            Cmd::Repair {
                use_recovery: true,
                output: None,
                output_dir: Some(ref path),
                ..
            } if path == &PathBuf::from("repaired-set")
        ));

        assert!(parse_cli([
            "sqz",
            "repair",
            "archive.zip.001",
            "--output-dir",
            "repaired-set",
        ])
        .is_err());
        assert!(parse_cli([
            "sqz",
            "repair",
            "archive.zip.001",
            "--use-recovery",
            "--output",
            "repaired.zip.001",
            "--output-dir",
            "repaired-set",
        ])
        .is_err());
    }
}
