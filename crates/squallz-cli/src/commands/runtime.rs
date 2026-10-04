//! Runtime facts captured once for an info or doctor command.

use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::{json, Value};
use squallz_core::api::FormatInfo;
use squallz_formats::{
    sevenzip_backend_status, wimlib_backend_status, SevenZipBackendSource, SevenZipBackendStatus,
    WimlibBackendSource, WimlibBackendStatus,
};

#[derive(Clone, Copy)]
pub(super) enum RuntimeNeed {
    Read,
    Write,
}

pub(super) struct RuntimeFacts {
    sevenzip: SevenZipBackendStatus,
    wimlib: WimlibBackendStatus,
    bsdtar: CommandSelection,
    unrar: CommandSelection,
    par2: CommandSelection,
}

impl RuntimeFacts {
    pub(super) fn capture() -> Self {
        Self {
            sevenzip: sevenzip_backend_status(),
            wimlib: wimlib_backend_status(),
            bsdtar: CommandSelection::detect(
                "SQUALLZ_BSDTAR",
                &["bsdtar"],
                Some("/usr/bin/bsdtar"),
            ),
            unrar: CommandSelection::detect("SQUALLZ_UNRAR", &["unrar"], None),
            par2: CommandSelection::detect("SQUALLZ_PAR2", PAR2_TOOLS, None),
        }
    }

    pub(super) fn sevenzip(&self) -> Availability<'_> {
        Availability::Tool {
            available: self.sevenzip.available(),
            source: self.sevenzip.source().map(|source| match source {
                SevenZipBackendSource::Application => "application",
                SevenZipBackendSource::Environment => "env",
                SevenZipBackendSource::Path => "path",
            }),
            env: Some("SQUALLZ_7Z"),
            selected: self.sevenzip.selected(),
            configured: self.sevenzip.configured(),
            tools: &["7zz", "7z", "7za"],
        }
    }

    pub(super) fn availability(&self, format_id: &str, need: RuntimeNeed) -> Availability<'_> {
        match (format_id, need) {
            ("wim", RuntimeNeed::Write) => Availability::Tool {
                available: self.wimlib.available(),
                source: self.wimlib.source().map(|source| match source {
                    WimlibBackendSource::Application => "application",
                    WimlibBackendSource::Environment => "env",
                    WimlibBackendSource::Path => "path",
                }),
                env: Some("SQUALLZ_WIMLIB"),
                selected: self.wimlib.selected(),
                configured: self.wimlib.configured(),
                tools: &["wimlib-imagex"],
            },
            ("rar", RuntimeNeed::Read) => {
                if self.bsdtar.configured() {
                    self.bsdtar
                        .availability(Some("SQUALLZ_BSDTAR"), &["bsdtar"])
                } else if self.sevenzip.configured() || self.sevenzip.available() {
                    self.sevenzip()
                } else {
                    self.bsdtar.availability(None, &["bsdtar"])
                }
            }
            (id, RuntimeNeed::Read) if is_external(id) => self.sevenzip(),
            (id, RuntimeNeed::Write) if is_external(id) => Availability::Unsupported,
            _ => Availability::BuiltIn(true),
        }
    }

    pub(super) fn unrar(&self) -> Availability<'_> {
        self.unrar.availability(Some("SQUALLZ_UNRAR"), &["unrar"])
    }

    pub(super) fn par2(&self) -> Availability<'_> {
        self.par2.availability(Some("SQUALLZ_PAR2"), PAR2_TOOLS)
    }

    pub(super) fn format_ready(&self, format: &FormatInfo) -> bool {
        let caps = format.capabilities;
        let read_required = caps.can_extract || caps.can_test;
        let read_ready =
            !read_required || self.availability(format.id, RuntimeNeed::Read).available();
        let write_ready =
            !caps.can_create || self.availability(format.id, RuntimeNeed::Write).available();
        read_ready && write_ready
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) enum Availability<'a> {
    BuiltIn(bool),
    Unsupported,
    Tool {
        available: bool,
        source: Option<&'static str>,
        env: Option<&'static str>,
        selected: Option<&'a Path>,
        configured: bool,
        tools: &'static [&'static str],
    },
    Par2Fallback,
}

impl<'a> Availability<'a> {
    pub(super) fn available(self) -> bool {
        match self {
            Self::BuiltIn(available) | Self::Tool { available, .. } => available,
            Self::Unsupported => false,
            Self::Par2Fallback => true,
        }
    }

    pub(super) fn source(self) -> Option<&'static str> {
        match self {
            Self::BuiltIn(_) => Some("built_in"),
            Self::Unsupported => Some("unsupported"),
            Self::Tool { source, .. } => source,
            Self::Par2Fallback => Some("built_in_fallback"),
        }
    }

    pub(super) fn selected(self) -> Option<&'a Path> {
        match self {
            Self::Tool { selected, .. } => selected,
            Self::Par2Fallback => Some(Path::new("rust-par2")),
            _ => None,
        }
    }

    pub(super) fn tools(self) -> &'static [&'static str] {
        match self {
            Self::Tool { tools, .. } => tools,
            _ => &[],
        }
    }

    pub(super) fn to_json(self) -> Value {
        match self {
            Self::BuiltIn(_) | Self::Unsupported => json!({
                "available": self.available(),
                "source": self.source(),
            }),
            Self::Par2Fallback => json!({
                "available": true,
                "source": "built_in_fallback",
                "selected": "rust-par2",
                "create_available": false,
            }),
            Self::Tool {
                available,
                source,
                env,
                selected,
                configured,
                tools,
            } => {
                let mut value = json!({
                    "available": available,
                    "source": source,
                    "selected": selected.map(Path::to_string_lossy),
                    "configured": configured,
                    "path_exists": available,
                    "tools": tools,
                });
                if let Some(env) = env {
                    value["env"] = json!(env);
                }
                value
            }
        }
    }
}

enum CommandSelection {
    Configured { selected: PathBuf, available: bool },
    Path(PathBuf),
    Missing,
}

impl CommandSelection {
    fn detect(env: &str, tools: &[&str], preferred: Option<&str>) -> Self {
        if let Some(configured) = std::env::var_os(env) {
            let selected = PathBuf::from(configured);
            let available = if selected.components().count() > 1 || selected.is_absolute() {
                command_is_executable(&selected)
            } else {
                find_on_path(&selected.to_string_lossy()).is_some()
            };
            return Self::Configured {
                selected,
                available,
            };
        }
        if let Some(path) = preferred
            .map(PathBuf::from)
            .filter(|path| command_is_executable(path))
        {
            return Self::Path(path);
        }
        match tools.iter().find_map(|tool| find_on_path(tool)) {
            Some(path) => Self::Path(path),
            None => Self::Missing,
        }
    }

    fn configured(&self) -> bool {
        matches!(self, Self::Configured { .. })
    }

    fn availability(
        &self,
        env: Option<&'static str>,
        tools: &'static [&'static str],
    ) -> Availability<'_> {
        let (available, source, selected) = match self {
            Self::Configured {
                selected,
                available,
            } => (*available, Some("env"), Some(selected.as_path())),
            Self::Path(path) => (true, Some("path"), Some(path.as_path())),
            Self::Missing => (false, None, None),
        };
        Availability::Tool {
            available,
            source,
            env,
            selected,
            configured: self.configured(),
            tools,
        }
    }
}

fn find_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(name);
        if command_is_executable(&candidate) {
            return Some(candidate);
        }
        #[cfg(windows)]
        {
            let candidate = dir.join(format!("{name}.exe"));
            if command_is_executable(&candidate) {
                return Some(candidate);
            }
        }
    }
    None
}

fn command_is_executable(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        match path.metadata() {
            Ok(metadata) => metadata.permissions().mode() & 0o111 != 0,
            Err(_) => false,
        }
    }
    #[cfg(not(unix))]
    {
        true
    }
}

const PAR2_TOOLS: &[&str] = &["par2cmdline-turbo", "par2", "par2cmdline"];

pub(super) fn is_external(format_id: &str) -> bool {
    format_id == "wim" || format_id == "rar" || long_tail_7z_bridge_format(format_id)
}

pub(super) fn long_tail_7z_bridge_format(format_id: &str) -> bool {
    matches!(
        format_id,
        "apfs"
            | "ar"
            | "arj"
            | "cab"
            | "chm"
            | "cpio"
            | "cramfs"
            | "dmg"
            | "ext"
            | "fat"
            | "gpt"
            | "hfs"
            | "ihex"
            | "iso"
            | "lzh"
            | "lzma"
            | "mbr"
            | "msi"
            | "nsis"
            | "ntfs"
            | "qcow2"
            | "rpm"
            | "squashfs"
            | "udf"
            | "uefi"
            | "vdi"
            | "vhd"
            | "vhdx"
            | "vmdk"
            | "xar"
            | "z"
    )
}

#[derive(Serialize)]
pub(super) struct FormatLimitation {
    scope: &'static str,
    status: &'static str,
    reason: &'static str,
}

pub(super) const RAR_LIMITATIONS: &[FormatLimitation] = &[
    FormatLimitation {
        scope: "create",
        status: "unsupported",
        reason: "Squallz does not create RAR archives",
    },
    FormatLimitation {
        scope: "recovery_records",
        status: "unsupported",
        reason: "RAR recovery records and RAR .rev files are outside the launch scope",
    },
    FormatLimitation {
        scope: "encrypted",
        status: "implemented_not_release_claimed",
        reason: "encrypted RAR reading uses 7zz/7z with a stdin-only password bridge; a licensed full corpus and three-platform package matrix are still required for a release claim",
    },
    FormatLimitation {
        scope: "multi_volume",
        status: "not_release_claimed",
        reason: "native partN.rar and legacy rar/r00 read orchestration is implemented with private first-volume staging, but a licensed full corpus and macOS/Windows/Linux package matrix are still required for a release claim",
    },
    FormatLimitation {
        scope: "rar7_v6",
        status: "implemented_not_release_claimed",
        reason: "confirmed-unencrypted RAR7 v6 entry streams can use an optional user-installed unrar decoder after 7zz/7z listing and volume validation; encrypted input stays on the stdin-only 7zz/7z path, and a full three-platform corpus is still required",
    },
    FormatLimitation {
        scope: "damaged_repair",
        status: "unsupported",
        reason: "damaged RAR can be detected or rejected, but RAR repair is not implemented",
    },
];
