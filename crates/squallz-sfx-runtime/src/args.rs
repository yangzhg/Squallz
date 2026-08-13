use std::ffi::OsString;
use std::path::PathBuf;

use clap::builder::PossibleValuesParser;
use clap::{value_parser, Arg, ArgAction, ArgGroup, ColorChoice, Command};
use squallz_core::api::{OverwritePolicy, ResourceOptions, SafetyLimits};
use squallz_i18n::Localizer;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    Extract,
    List,
    Test,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OverwriteArg {
    Skip,
    All,
    Rename,
    Ask,
}

impl OverwriteArg {
    fn from_value(value: &str) -> Self {
        match value {
            "all" => Self::All,
            "rename" => Self::Rename,
            "ask" => Self::Ask,
            _ => Self::Skip,
        }
    }
}

impl From<OverwriteArg> for OverwritePolicy {
    fn from(value: OverwriteArg) -> Self {
        match value {
            OverwriteArg::Skip => Self::Skip,
            OverwriteArg::All => Self::Overwrite,
            OverwriteArg::Rename => Self::RenameBoth,
            OverwriteArg::Ask => Self::Ask,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RuntimeArgs {
    pub(crate) quiet: bool,
    pub(crate) verbose: bool,
    pub(crate) output: Option<PathBuf>,
    pub(crate) mode: Mode,
    pub(crate) overwrite: OverwriteArg,
    pub(crate) threads: Option<usize>,
    pub(crate) memory_limit: Option<u64>,
    pub(crate) max_output_bytes: Option<u64>,
    pub(crate) max_entries: Option<u64>,
    pub(crate) max_compression_ratio: Option<u32>,
    pub(crate) json: bool,
}

impl RuntimeArgs {
    pub(crate) fn resources(&self) -> ResourceOptions {
        ResourceOptions {
            threads: self.threads,
            memory_limit: self.memory_limit,
        }
    }

    pub(crate) fn limits(&self) -> SafetyLimits {
        let defaults = SafetyLimits::default();
        SafetyLimits {
            max_output_bytes: self.max_output_bytes.unwrap_or(defaults.max_output_bytes),
            max_entries: self.max_entries.unwrap_or(defaults.max_entries),
            max_compression_ratio: self
                .max_compression_ratio
                .unwrap_or(defaults.max_compression_ratio),
        }
    }
}

pub(crate) fn explicit_language(args: &[OsString]) -> Option<String> {
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        let value = arg.to_string_lossy();
        if value == "--lang" {
            return iter
                .next()
                .map(|language| language.to_string_lossy().into_owned());
        }
        if let Some(language) = value.strip_prefix("--lang=") {
            return Some(language.to_owned());
        }
    }
    None
}

pub(crate) fn json_requested(args: &[OsString]) -> bool {
    args.iter().any(|arg| arg == "--json")
}

pub(crate) fn parse(args: &[OsString], loc: &Localizer) -> Result<RuntimeArgs, clap::Error> {
    let mut argv = Vec::with_capacity(args.len().saturating_add(1));
    argv.push(OsString::from("sqz-sfx"));
    argv.extend_from_slice(args);
    let matches = command(loc).try_get_matches_from(argv)?;

    let mode = if matches.get_flag("list") {
        Mode::List
    } else if matches.get_flag("test") {
        Mode::Test
    } else {
        Mode::Extract
    };
    let overwrite = matches
        .get_one::<String>("overwrite")
        .map(|value| OverwriteArg::from_value(value))
        .unwrap_or(OverwriteArg::Skip);

    Ok(RuntimeArgs {
        quiet: matches.get_flag("quiet"),
        verbose: matches.get_flag("verbose"),
        output: matches.get_one::<PathBuf>("output").cloned(),
        mode,
        overwrite,
        threads: matches.get_one::<usize>("threads").copied(),
        memory_limit: matches.get_one::<u64>("memory_limit").copied(),
        max_output_bytes: matches.get_one::<u64>("max_output_bytes").copied(),
        max_entries: matches.get_one::<u64>("max_entries").copied(),
        max_compression_ratio: matches.get_one::<u32>("max_compression_ratio").copied(),
        json: matches.get_flag("json"),
    })
}

fn command(loc: &Localizer) -> Command {
    let chinese = loc.language().to_ascii_lowercase().starts_with("zh");
    let help_template = if chinese {
        "{about-with-newline}\n用法: {usage}\n\n选项:\n{options}"
    } else {
        "{about-with-newline}\nUsage: {usage}\n\nOptions:\n{options}"
    };
    let positive_integer = loc.t("cli.sfx.runtime.error.positive_integer");
    let byte_size = loc.t("cli.sfx.runtime.error.byte_size");
    let byte_size_too_large = loc.t("cli.sfx.runtime.error.byte_size_too_large");
    let threads_error = positive_integer.clone();
    let entries_error = positive_integer.clone();
    let ratio_error = positive_integer;
    let memory_size_error = byte_size.clone();
    let output_size_error = byte_size;
    let memory_too_large = byte_size_too_large.clone();
    let output_too_large = byte_size_too_large;
    Command::new("sqz-sfx")
        .version(env!("CARGO_PKG_VERSION"))
        .about(loc.t("cli.sfx.runtime.about"))
        .help_template(help_template)
        .color(ColorChoice::Never)
        .disable_help_subcommand(true)
        .disable_help_flag(true)
        .disable_version_flag(true)
        .group(ArgGroup::new("mode").multiple(false).args(["list", "test"]))
        .arg(
            Arg::new("lang")
                .long("lang")
                .value_name("LANG")
                .help(loc.t("cli.sfx.runtime.help.lang")),
        )
        .arg(
            Arg::new("quiet")
                .short('q')
                .long("quiet")
                .action(ArgAction::SetTrue)
                .help(loc.t("cli.sfx.runtime.help.quiet")),
        )
        .arg(
            Arg::new("verbose")
                .short('v')
                .long("verbose")
                .action(ArgAction::SetTrue)
                .conflicts_with("quiet")
                .help(loc.t("cli.sfx.runtime.help.verbose")),
        )
        .arg(
            Arg::new("output")
                .short('d')
                .long("output")
                .value_name("DIRECTORY")
                .value_parser(value_parser!(PathBuf))
                .conflicts_with_all(["list", "test"])
                .help(loc.t("cli.sfx.runtime.help.output")),
        )
        .arg(
            Arg::new("list")
                .long("list")
                .action(ArgAction::SetTrue)
                .help(loc.t("cli.sfx.runtime.help.list")),
        )
        .arg(
            Arg::new("test")
                .long("test")
                .action(ArgAction::SetTrue)
                .help(loc.t("cli.sfx.runtime.help.test")),
        )
        .arg(
            Arg::new("overwrite")
                .long("overwrite")
                .value_name("POLICY")
                .default_value("skip")
                .value_parser(PossibleValuesParser::new(["skip", "all", "rename", "ask"]))
                .help(loc.t("cli.sfx.runtime.help.overwrite")),
        )
        .arg(
            Arg::new("threads")
                .long("threads")
                .value_name("N")
                .value_parser(clap::builder::ValueParser::new(move |value: &str| {
                    parse_nonzero_usize(value, &threads_error)
                }))
                .help(loc.t("cli.sfx.runtime.help.threads")),
        )
        .arg(
            Arg::new("memory_limit")
                .long("memory-limit")
                .value_name("SIZE")
                .value_parser(clap::builder::ValueParser::new(move |value: &str| {
                    parse_nonzero_size(value, &memory_size_error, &memory_too_large)
                }))
                .help(loc.t("cli.sfx.runtime.help.memory_limit")),
        )
        .arg(
            Arg::new("max_output_bytes")
                .long("max-output-bytes")
                .value_name("SIZE")
                .value_parser(clap::builder::ValueParser::new(move |value: &str| {
                    parse_nonzero_size(value, &output_size_error, &output_too_large)
                }))
                .help(loc.t("cli.sfx.runtime.help.max_output_bytes")),
        )
        .arg(
            Arg::new("max_entries")
                .long("max-entries")
                .value_name("N")
                .value_parser(clap::builder::ValueParser::new(move |value: &str| {
                    parse_nonzero_u64(value, &entries_error)
                }))
                .help(loc.t("cli.sfx.runtime.help.max_entries")),
        )
        .arg(
            Arg::new("max_compression_ratio")
                .long("max-compression-ratio")
                .value_name("N")
                .value_parser(clap::builder::ValueParser::new(move |value: &str| {
                    parse_nonzero_u32(value, &ratio_error)
                }))
                .help(loc.t("cli.sfx.runtime.help.max_compression_ratio")),
        )
        .arg(
            Arg::new("json")
                .long("json")
                .action(ArgAction::SetTrue)
                .help(loc.t("cli.sfx.runtime.help.json")),
        )
        .arg(
            Arg::new("help")
                .short('h')
                .long("help")
                .action(ArgAction::Help)
                .help(loc.t("cli.sfx.runtime.help.help")),
        )
        .arg(
            Arg::new("version")
                .short('V')
                .long("version")
                .action(ArgAction::Version)
                .help(loc.t("cli.sfx.runtime.help.version")),
        )
        .arg(
            Arg::new("style")
                .long("style")
                .value_name("STYLE")
                .hide(true),
        )
        .arg(
            Arg::new("color")
                .long("color")
                .value_name("WHEN")
                .hide(true),
        )
        .arg(
            Arg::new("accent")
                .long("accent")
                .value_name("PALETTE")
                .hide(true),
        )
}

fn parse_nonzero_usize(value: &str, message: &str) -> Result<usize, String> {
    value
        .parse::<usize>()
        .map_err(|_| message.to_owned())
        .and_then(|number| {
            if number == 0 {
                Err(message.to_owned())
            } else {
                Ok(number)
            }
        })
}

fn parse_nonzero_u64(value: &str, message: &str) -> Result<u64, String> {
    value
        .parse::<u64>()
        .map_err(|_| message.to_owned())
        .and_then(|number| {
            if number == 0 {
                Err(message.to_owned())
            } else {
                Ok(number)
            }
        })
}

fn parse_nonzero_u32(value: &str, message: &str) -> Result<u32, String> {
    value
        .parse::<u32>()
        .map_err(|_| message.to_owned())
        .and_then(|number| {
            if number == 0 {
                Err(message.to_owned())
            } else {
                Ok(number)
            }
        })
}

fn parse_nonzero_size(value: &str, message: &str, too_large: &str) -> Result<u64, String> {
    let normalized = value.trim().to_ascii_lowercase();
    let (digits, multiplier) = if let Some(digits) = normalized.strip_suffix("kb") {
        (digits, 1024u64)
    } else if let Some(digits) = normalized.strip_suffix('k') {
        (digits, 1024u64)
    } else if let Some(digits) = normalized.strip_suffix("mb") {
        (digits, 1024u64.pow(2))
    } else if let Some(digits) = normalized.strip_suffix('m') {
        (digits, 1024u64.pow(2))
    } else if let Some(digits) = normalized.strip_suffix("gb") {
        (digits, 1024u64.pow(3))
    } else if let Some(digits) = normalized.strip_suffix('g') {
        (digits, 1024u64.pow(3))
    } else {
        (normalized.as_str(), 1)
    };
    let number = digits
        .trim()
        .parse::<u64>()
        .map_err(|_| message.to_owned())?;
    if number == 0 {
        return Err(message.to_owned());
    }
    number
        .checked_mul(multiplier)
        .ok_or_else(|| too_large.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loc() -> Localizer {
        Localizer::with_user_dir(Some("en-US"), None)
    }

    fn os_args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    #[test]
    fn parses_modes_limits_and_presentation_flags() {
        let args = os_args(&[
            "--list",
            "--json",
            "--threads",
            "2",
            "--memory-limit",
            "64M",
            "--max-output-bytes",
            "2G",
            "--max-entries",
            "42",
            "--max-compression-ratio",
            "99",
            "--style",
            "modern",
            "--color",
            "never",
            "--accent",
            "ocean",
        ]);
        let parsed = parse(&args, &loc()).expect("valid SFX arguments");

        assert_eq!(parsed.mode, Mode::List);
        assert!(parsed.json);
        assert_eq!(parsed.threads, Some(2));
        assert_eq!(parsed.memory_limit, Some(64 * 1024 * 1024));
        assert_eq!(parsed.max_output_bytes, Some(2 * 1024 * 1024 * 1024));
        assert_eq!(parsed.max_entries, Some(42));
        assert_eq!(parsed.max_compression_ratio, Some(99));
    }

    #[test]
    fn defaults_to_extract_and_safe_overwrite() {
        let parsed = parse(&[], &loc()).expect("default SFX arguments");

        assert_eq!(parsed.mode, Mode::Extract);
        assert_eq!(parsed.overwrite, OverwriteArg::Skip);
        assert!(!parsed.json);
    }

    #[test]
    fn rejects_conflicting_modes_and_output_for_read_only_modes() {
        assert!(parse(&os_args(&["--list", "--test"]), &loc()).is_err());
        assert!(parse(&os_args(&["--list", "-d", "output"]), &loc()).is_err());
    }

    #[test]
    fn explicit_language_accepts_both_clap_forms() {
        assert_eq!(
            explicit_language(&os_args(&["--lang", "zh-CN", "--list"])),
            Some("zh-CN".to_owned())
        );
        assert_eq!(
            explicit_language(&os_args(&["--lang=en-US", "--test"])),
            Some("en-US".to_owned())
        );
    }

    #[test]
    fn chinese_help_has_no_english_heading_or_bare_keys() {
        let loc = Localizer::with_user_dir(Some("zh-CN"), None);
        let error = command(&loc)
            .try_get_matches_from(["sqz-sfx", "--help"])
            .expect_err("help stops argument parsing");
        let help = error.to_string();

        assert!(help.contains("用法: sqz-sfx"));
        assert!(help.contains("选项:"));
        assert!(!help.contains("Options:"));
        assert!(!help.contains("cli.sfx.runtime"));
    }
}
