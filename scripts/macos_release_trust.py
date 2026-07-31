#!/usr/bin/env python3
import argparse
import hashlib
import json
import os
import plistlib
import re
import shlex
import stat
import subprocess
import sys
import tempfile
import uuid
from datetime import datetime, timezone
from pathlib import Path
from typing import Optional


SCHEMA = "dev.squallz.macos.release-trust.v1"
ROOT = Path(__file__).resolve().parents[1]
TAURI_CONFIG = ROOT / "crates/squallz-gui/tauri.conf.json"
SUMMARY_NAME = "trust-summary.json"
QUICK_LOOK_RELATIVE = Path("Contents/PlugIns/SquallzQuickLook.appex")
QUICK_LOOK_EXECUTABLE_RELATIVE = (
    QUICK_LOOK_RELATIVE / "Contents/MacOS/SquallzQuickLook"
)
QUICK_LOOK_BUNDLE_ID = "dev.squallz.desktop.quicklook"
QUICK_LOOK_MINIMUM_VERSION = "12.0"
QUICK_LOOK_ARCHIVE_EXTENSIONS = frozenset(
    {
        "zip",
        "jar",
        "apk",
        "cbz",
        "ipa",
        "7z",
        "tar",
        "tgz",
        "tbz2",
        "txz",
        "tzst",
    }
)
QUICK_LOOK_ARCHIVE_TYPES = frozenset(
    {
        "public.zip-archive",
        "com.sun.java-archive",
        "dev.squallz.archive.apk",
        "dev.squallz.archive.cbz",
        "com.apple.itunes.ipa",
        "org.7-zip.7-zip-archive",
        "public.tar-archive",
        "org.gnu.gnu-zip-tar-archive",
        "public.tar-bzip2-archive",
        "org.tukaani.tar-xz-archive",
        "dev.squallz.archive.tar-zstd",
    }
)
APP_ARCHIVE_EXTENSIONS = QUICK_LOOK_ARCHIVE_EXTENSIONS | frozenset(
    {"001", "cbr", "rar", "wim", "swm"}
)
APP_ARCHIVE_TYPES = QUICK_LOOK_ARCHIVE_TYPES | frozenset(
    {
        "dev.squallz.archive.cbr",
        "dev.squallz.archive.rar",
        "dev.squallz.archive.split-volume",
        "dev.squallz.archive.wim",
        "dev.squallz.archive.split-wim",
    }
)
QUICK_LOOK_STREAM_EXTENSIONS = frozenset({"gz", "bz2", "xz", "zst", "lz4", "br"})
QUICK_LOOK_STREAM_TYPES = frozenset(
    {
        "org.gnu.gnu-zip-archive",
        "public.bzip2-archive",
        "org.tukaani.xz-archive",
        "dev.squallz.stream.zstd",
        "dev.squallz.stream.lz4",
        "dev.squallz.stream.brotli",
    }
)
QUICK_LOOK_DOCUMENT_TYPE_SPECS = {
    APP_ARCHIVE_EXTENSIONS: (
        APP_ARCHIVE_TYPES,
        "Viewer",
        "Alternate",
    ),
    frozenset({"sqz"}): (
        frozenset({"dev.squallz.sqz-archive"}),
        "Viewer",
        "Owner",
    ),
    QUICK_LOOK_STREAM_EXTENSIONS: (
        QUICK_LOOK_STREAM_TYPES,
        "Viewer",
        "Alternate",
    ),
}
QUICK_LOOK_IMPORTED_TYPE_SPECS = {
    "dev.squallz.archive.apk": (frozenset({"apk"}), frozenset({"public.zip-archive"})),
    "dev.squallz.archive.cbz": (frozenset({"cbz"}), frozenset({"public.zip-archive"})),
    "dev.squallz.archive.cbr": (frozenset({"cbr"}), frozenset({"public.archive"})),
    "dev.squallz.archive.rar": (frozenset({"rar"}), frozenset({"public.archive"})),
    "dev.squallz.archive.tar-zstd": (
        frozenset({"tzst"}),
        frozenset({"public.archive"}),
    ),
    "dev.squallz.archive.split-volume": (
        frozenset({"001"}),
        frozenset({"public.archive"}),
    ),
    "dev.squallz.archive.wim": (
        frozenset({"wim"}),
        frozenset({"public.archive"}),
    ),
    "dev.squallz.archive.split-wim": (
        frozenset({"swm"}),
        frozenset({"public.archive"}),
    ),
    "dev.squallz.stream.zstd": (frozenset({"zst"}), frozenset({"public.archive"})),
    "dev.squallz.stream.lz4": (frozenset({"lz4"}), frozenset({"public.archive"})),
    "dev.squallz.stream.brotli": (frozenset({"br"}), frozenset({"public.archive"})),
}
QUICK_LOOK_EXPORTED_TYPE_SPECS = {
    "dev.squallz.sqz-archive": (
        frozenset({"sqz"}),
        frozenset({"public.archive"}),
    )
}
QUICK_LOOK_SUPPORTED_TYPES = frozenset(
    {
        "dev.squallz.sqz-archive",
    }
    | QUICK_LOOK_ARCHIVE_TYPES
    | QUICK_LOOK_STREAM_TYPES
)
QUICK_LOOK_FORBIDDEN_PROCESS_SYMBOLS = frozenset(
    {
        "_fork",
        "_vfork",
        "_wait",
        "_waitpid",
        "_popen",
        "_system",
        "_pipe",
        "_pipe2",
        "_OBJC_CLASS_$_NSTask",
    }
)
QUICK_LOOK_FORBIDDEN_PROCESS_PREFIXES = (
    "_exec",
    "_posix_spawn",
)
MACHO_MAGICS = {
    b"\xce\xfa\xed\xfe",
    b"\xcf\xfa\xed\xfe",
    b"\xca\xfe\xba\xbe",
    b"\xca\xfe\xba\xbf",
    b"\xfe\xed\xfa\xce",
    b"\xfe\xed\xfa\xcf",
    b"\xbe\xba\xfe\xca",
    b"\xbf\xba\xfe\xca",
}
ARCHITECTURES = {
    "arm64": frozenset({"arm64"}),
    "aarch64": frozenset({"arm64"}),
    "x64": frozenset({"x86_64"}),
    "x86_64": frozenset({"x86_64"}),
    "amd64": frozenset({"x86_64"}),
}
ALLOWED_CODE_DIRECTORIES = {
    "Frameworks",
    "Helpers",
    "Library",
    "MacOS",
    "PlugIns",
    "XPCServices",
}


class TrustError(RuntimeError):
    pass


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def write_json(path: Path, value: object) -> None:
    path.write_text(
        json.dumps(value, ensure_ascii=False, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )


def relative_file_ref(path: Path) -> dict[str, object]:
    return {
        "name": path.name,
        "sha256": sha256_file(path),
        "size_bytes": path.stat().st_size,
    }


def normalized_architecture(value: str) -> frozenset[str]:
    architecture = ARCHITECTURES.get(value.lower())
    if architecture is None:
        choices = ", ".join(sorted(ARCHITECTURES))
        raise TrustError(f"unsupported architecture '{value}'; expected one of {choices}")
    return architecture


def version_tuple(value: str) -> tuple[int, ...]:
    if not re.fullmatch(r"\d+(?:\.\d+){1,2}", value):
        raise TrustError(f"invalid macOS version '{value}'")
    return tuple(int(part) for part in value.split("."))


def is_macho(path: Path) -> bool:
    if not path.is_file() or path.is_symlink():
        return False
    try:
        with path.open("rb") as handle:
            return handle.read(4) in MACHO_MAGICS
    except OSError:
        return False


def discover_macho(app: Path) -> list[Path]:
    found: list[Path] = []
    for base_raw, directory_names, file_names in os.walk(app, followlinks=False):
        base = Path(base_raw)
        directory_names[:] = [
            name for name in directory_names if not (base / name).is_symlink()
        ]
        for name in file_names:
            candidate = base / name
            if is_macho(candidate):
                found.append(candidate)
    return sorted(found)


def bundle_content_sha256(app: Path) -> str:
    digest = hashlib.sha256()

    def update_field(value: bytes) -> None:
        digest.update(len(value).to_bytes(8, "big"))
        digest.update(value)

    def visit(directory: Path) -> None:
        try:
            entries = sorted(os.scandir(directory), key=lambda entry: entry.name)
        except OSError as error:
            raise TrustError("app bundle contents could not be read") from error
        for entry in entries:
            path = Path(entry.path)
            relative = path.relative_to(app)
            try:
                metadata = entry.stat(follow_symlinks=False)
                mode = stat.S_IMODE(metadata.st_mode)
                if entry.is_symlink():
                    kind = b"symlink"
                    payload = os.fsencode(os.readlink(entry.path))
                elif entry.is_dir(follow_symlinks=False):
                    kind = b"directory"
                    payload = b""
                elif entry.is_file(follow_symlinks=False):
                    kind = b"file"
                    payload = b""
                else:
                    raise TrustError(
                        f"unsupported file type in app bundle: {relative}"
                    )
            except OSError as error:
                raise TrustError(
                    f"app bundle entry could not be inspected: {relative}"
                ) from error

            update_field(os.fsencode(relative.as_posix()))
            update_field(kind)
            update_field(f"{mode:o}".encode("ascii"))
            if kind == b"symlink":
                update_field(payload)
            elif kind == b"file":
                try:
                    update_field(str(metadata.st_size).encode("ascii"))
                    with path.open("rb") as handle:
                        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
                            digest.update(chunk)
                except OSError as error:
                    raise TrustError(
                        f"app bundle file could not be read: {relative}"
                    ) from error
            else:
                visit(path)

    visit(app)
    return digest.hexdigest()


def parse_codesign_details(text: str) -> dict[str, object]:
    authorities: list[str] = []
    values: dict[str, str] = {}
    flags: set[str] = set()
    for raw_line in text.splitlines():
        line = raw_line.strip()
        if line.startswith("Authority="):
            authorities.append(line.partition("=")[2])
        elif "=" in line:
            key, _, value = line.partition("=")
            values.setdefault(key, value)
        flags_match = re.search(r"flags=0x[0-9a-fA-F]+\(([^)]*)\)", line)
        if flags_match:
            flags.update(
                item.strip() for item in flags_match.group(1).split(",") if item.strip()
            )
    return {
        "authorities": authorities,
        "cdhash": values.get("CDHash"),
        "identifier": values.get("Identifier"),
        "runtime": "runtime" in flags,
        "signature": values.get("Signature"),
        "team_id": values.get("TeamIdentifier"),
        "timestamp": values.get("Timestamp"),
    }


def gatekeeper_accepts_notarized_developer_id(text: str) -> bool:
    lowered = text.lower()
    return re.search(r"\baccepted\b", lowered) is not None and re.search(
        r"source\s*=\s*notarized developer id", lowered
    ) is not None


def get_task_allow_enabled(text: str) -> bool:
    return re.search(
        r"<key>com\.apple\.security\.get-task-allow</key>\s*<true\s*/>",
        text,
    ) is not None


def signature_problems(
    details: dict[str, object],
    entitlements: str,
    *,
    expected_identity: Optional[str],
    expected_team_id: Optional[str],
    require_runtime: bool,
) -> list[str]:
    problems: list[str] = []
    authorities = details.get("authorities")
    first_authority = authorities[0] if isinstance(authorities, list) and authorities else None
    if details.get("signature") == "adhoc":
        problems.append("signature is ad-hoc")
    if expected_identity is not None:
        if first_authority != expected_identity:
            problems.append("Developer ID authority does not match the release identity")
    elif not isinstance(first_authority, str) or not first_authority.startswith(
        "Developer ID Application: "
    ):
        problems.append("Developer ID Application authority is missing")
    team_id = details.get("team_id")
    if expected_team_id is not None:
        if team_id != expected_team_id:
            problems.append("TeamIdentifier does not match the release team")
    elif not isinstance(team_id, str) or team_id in {"", "not set"}:
        problems.append("TeamIdentifier is missing")
    if require_runtime and details.get("runtime") is not True:
        problems.append("hardened runtime flag is missing")
    if not details.get("timestamp"):
        problems.append("secure signing timestamp is missing")
    if get_task_allow_enabled(entitlements):
        problems.append("get-task-allow entitlement is enabled")
    return problems


def parse_notary_evidence(
    submit: object, log: object
) -> tuple[str, dict[str, object]]:
    if not isinstance(submit, dict):
        raise TrustError("notary submit output must be a JSON object")
    submission_id = submit.get("id")
    try:
        parsed_id = str(uuid.UUID(str(submission_id)))
    except (ValueError, TypeError, AttributeError) as error:
        raise TrustError("notary submit output has no valid submission id") from error
    if submit.get("status") != "Accepted":
        raise TrustError(
            f"notary submission was not accepted: {submit.get('status', 'missing status')}"
        )
    if not isinstance(log, dict):
        raise TrustError("notary log must be a JSON object")
    if str(log.get("jobId")) != parsed_id:
        raise TrustError("notary log jobId does not match the submission id")
    if log.get("status") != "Accepted":
        raise TrustError("notary log status is not Accepted")
    issues = log.get("issues")
    if issues not in (None, []):
        raise TrustError("notary log contains issues that require review")
    return parsed_id, log


class EvidenceStore:
    def __init__(self, directory: Path) -> None:
        self.directory = directory
        if self.directory.exists():
            if self.directory.is_symlink() or not self.directory.is_dir():
                raise TrustError("evidence path must be a regular directory")
            try:
                if any(self.directory.iterdir()):
                    raise TrustError("evidence directory must be empty")
            except OSError as error:
                raise TrustError("evidence directory could not be inspected") from error
        else:
            try:
                self.directory.mkdir(parents=True)
            except OSError as error:
                raise TrustError("evidence directory could not be created") from error

    @staticmethod
    def _log_name(label: str) -> str:
        cleaned = re.sub(r"[^0-9A-Za-z._-]+", "-", label).strip("-")
        return f"{cleaned or 'command'}.log"

    @staticmethod
    def _display_command(command: list[str]) -> str:
        displayed = list(command)
        for index, value in enumerate(displayed[:-1]):
            if value == "--key":
                displayed[index + 1] = "<private-key-path>"
        return shlex.join(displayed)

    @staticmethod
    def _redact_output(command: list[str], value: str) -> str:
        redacted = value
        for index, argument in enumerate(command[:-1]):
            if argument == "--key":
                redacted = redacted.replace(
                    command[index + 1], "<private-key-path>"
                )
        return redacted

    def run(
        self,
        label: str,
        command: list[str],
        *,
        check: bool = True,
    ) -> subprocess.CompletedProcess[str]:
        log_path = self.directory / self._log_name(label)
        try:
            result = subprocess.run(
                command,
                capture_output=True,
                text=True,
                check=False,
            )
        except OSError as error:
            error_text = self._redact_output(command, str(error))
            log_path.write_text(
                f"$ {self._display_command(command)}\nerror: {error_text}\n",
                encoding="utf-8",
            )
            raise TrustError(f"{label} could not start; see {log_path.name}") from error
        logged_stdout = self._redact_output(command, result.stdout)
        logged_stderr = self._redact_output(command, result.stderr)
        log_path.write_text(
            "\n".join(
                [
                    f"$ {self._display_command(command)}",
                    f"exit_code={result.returncode}",
                    "",
                    "[stdout]",
                    logged_stdout,
                    "[stderr]",
                    logged_stderr,
                ]
            ),
            encoding="utf-8",
        )
        if check and result.returncode != 0:
            raise TrustError(f"{label} failed; see {log_path.name}")
        return result

    def logs(self) -> list[dict[str, object]]:
        return [
            relative_file_ref(path)
            for path in sorted(self.directory.iterdir())
            if path.is_file() and path.name != SUMMARY_NAME
        ]


def read_release_config() -> tuple[str, str]:
    try:
        config = json.loads(TAURI_CONFIG.read_text(encoding="utf-8"))
        bundle_id = config["identifier"]
        minimum_version = config["bundle"]["macOS"]["minimumSystemVersion"]
    except (OSError, KeyError, TypeError, json.JSONDecodeError) as error:
        raise TrustError("Tauri config has no macOS release contract") from error
    if not isinstance(bundle_id, str) or not isinstance(minimum_version, str):
        raise TrustError("Tauri macOS release contract is invalid")
    version_tuple(minimum_version)
    return bundle_id, minimum_version


def _quick_look_string_set(value: object, label: str) -> frozenset[str]:
    if (
        not isinstance(value, list)
        or not value
        or any(not isinstance(item, str) or not item for item in value)
        or len(value) != len(set(value))
    ):
        raise TrustError(f"{label} must be a non-empty list of unique strings")
    return frozenset(value)


def _validate_quick_look_type_declarations(
    app_info: dict[str, object],
    *,
    key: str,
    expected: dict[str, tuple[frozenset[str], frozenset[str]]],
) -> None:
    declarations = app_info.get(key)
    if not isinstance(declarations, list) or len(declarations) != len(expected):
        raise TrustError(f"app bundle {key} does not match the Quick Look contract")

    seen: set[str] = set()
    for declaration in declarations:
        if not isinstance(declaration, dict):
            raise TrustError(f"app bundle {key} contains an invalid declaration")
        identifier = declaration.get("UTTypeIdentifier")
        if not isinstance(identifier, str) or identifier in seen:
            raise TrustError(f"app bundle {key} contains an invalid type identifier")
        specification = expected.get(identifier)
        if specification is None:
            raise TrustError(f"app bundle {key} contains an unexpected type identifier")
        seen.add(identifier)

        extensions = _quick_look_string_set(
            declaration.get("UTTypeTagSpecification", {}).get(
                "public.filename-extension"
            )
            if isinstance(declaration.get("UTTypeTagSpecification"), dict)
            else None,
            f"app bundle {identifier} filename extensions",
        )
        conformances = _quick_look_string_set(
            declaration.get("UTTypeConformsTo"),
            f"app bundle {identifier} conformances",
        )
        if (extensions, conformances) != specification:
            raise TrustError(
                f"app bundle {identifier} declaration does not match the Quick Look contract"
            )
        description = declaration.get("UTTypeDescription")
        if not isinstance(description, str) or not description.strip():
            raise TrustError(f"app bundle {identifier} has no type description")

    if seen != set(expected):
        raise TrustError(f"app bundle {key} is missing a Quick Look type declaration")


def validate_quick_look_host_types(
    app_info: object,
) -> dict[str, object]:
    if not isinstance(app_info, dict):
        raise TrustError("app bundle Info.plist root must be a dictionary")
    document_types = app_info.get("CFBundleDocumentTypes")
    if not isinstance(document_types, list) or len(document_types) != len(
        QUICK_LOOK_DOCUMENT_TYPE_SPECS
    ):
        raise TrustError(
            "app bundle document types do not match the Quick Look contract"
        )

    seen: set[frozenset[str]] = set()
    for document_type in document_types:
        if not isinstance(document_type, dict):
            raise TrustError("app bundle contains an invalid document type")
        extensions = _quick_look_string_set(
            document_type.get("CFBundleTypeExtensions"),
            "app bundle document type extensions",
        )
        if extensions in seen:
            raise TrustError("app bundle contains a duplicate document type")
        specification = QUICK_LOOK_DOCUMENT_TYPE_SPECS.get(extensions)
        if specification is None:
            raise TrustError("app bundle contains an unexpected document type")
        content_types, role, rank = specification
        if (
            _quick_look_string_set(
                document_type.get("LSItemContentTypes"),
                "app bundle document content types",
            )
            != content_types
            or document_type.get("CFBundleTypeRole") != role
            or document_type.get("LSHandlerRank") != rank
        ):
            raise TrustError(
                "app bundle document type does not match the Quick Look contract"
            )
        seen.add(extensions)

    if seen != set(QUICK_LOOK_DOCUMENT_TYPE_SPECS):
        raise TrustError("app bundle is missing a Quick Look document type")
    _validate_quick_look_type_declarations(
        app_info,
        key="UTImportedTypeDeclarations",
        expected=QUICK_LOOK_IMPORTED_TYPE_SPECS,
    )
    _validate_quick_look_type_declarations(
        app_info,
        key="UTExportedTypeDeclarations",
        expected=QUICK_LOOK_EXPORTED_TYPE_SPECS,
    )
    return {
        "document_type_groups": len(document_types),
        "imported_type_declarations": len(QUICK_LOOK_IMPORTED_TYPE_SPECS),
        "exported_type_declarations": len(QUICK_LOOK_EXPORTED_TYPE_SPECS),
    }


def validate_quick_look_info(
    info: object,
    *,
    app_short_version: object,
    app_bundle_version: object,
) -> dict[str, object]:
    if not isinstance(info, dict):
        raise TrustError("Quick Look extension Info.plist root must be a dictionary")
    expected = {
        "CFBundleIdentifier": QUICK_LOOK_BUNDLE_ID,
        "CFBundleExecutable": "SquallzQuickLook",
        "CFBundlePackageType": "XPC!",
        "LSMinimumSystemVersion": QUICK_LOOK_MINIMUM_VERSION,
        "CFBundleShortVersionString": app_short_version,
        "CFBundleVersion": app_bundle_version,
    }
    for key, value in expected.items():
        if not isinstance(value, str) or not value:
            raise TrustError(f"app bundle has no valid {key} for Quick Look")
        if info.get(key) != value:
            raise TrustError(f"Quick Look extension {key} does not match its contract")
    if info.get("CFBundleInfoDictionaryVersion") != "6.0":
        raise TrustError("Quick Look extension Info.plist version is invalid")
    if info.get("CFBundleSupportedPlatforms") != ["MacOSX"]:
        raise TrustError("Quick Look extension supported platforms are invalid")

    extension = info.get("NSExtension")
    if not isinstance(extension, dict):
        raise TrustError("Quick Look extension has no NSExtension dictionary")
    if extension.get("NSExtensionPointIdentifier") != "com.apple.quicklook.preview":
        raise TrustError("Quick Look extension point identifier is invalid")
    if (
        extension.get("NSExtensionPrincipalClass")
        != "SquallzQuickLook.PreviewProvider"
    ):
        raise TrustError("Quick Look extension principal class is invalid")
    attributes = extension.get("NSExtensionAttributes")
    if not isinstance(attributes, dict):
        raise TrustError("Quick Look extension attributes are missing")
    if attributes.get("QLIsDataBasedPreview") is not True:
        raise TrustError("Quick Look extension must use a data-based preview")
    if attributes.get("QLSupportsSearchableItems") is not False:
        raise TrustError("Quick Look extension searchable item support is invalid")
    content_types = attributes.get("QLSupportedContentTypes")
    if (
        not isinstance(content_types, list)
        or any(not isinstance(value, str) for value in content_types)
        or len(content_types) != len(set(content_types))
        or frozenset(content_types) != QUICK_LOOK_SUPPORTED_TYPES
    ):
        raise TrustError("Quick Look supported content types do not match the contract")

    return {
        "bundle_id": QUICK_LOOK_BUNDLE_ID,
        "minimum_system_version": QUICK_LOOK_MINIMUM_VERSION,
        "supported_content_types": sorted(QUICK_LOOK_SUPPORTED_TYPES),
    }


def inspect_quick_look_extension(
    app: Path,
    app_info: dict[str, object],
) -> dict[str, object]:
    host_types = validate_quick_look_host_types(app_info)
    extension_path = app / QUICK_LOOK_RELATIVE
    if not extension_path.is_dir() or extension_path.is_symlink():
        raise TrustError("app bundle has no regular Squallz Quick Look extension")
    info_path = extension_path / "Contents/Info.plist"
    if not info_path.is_file() or info_path.is_symlink():
        raise TrustError("Quick Look extension has no regular Info.plist")
    try:
        info = plistlib.loads(info_path.read_bytes())
    except (OSError, plistlib.InvalidFileException) as error:
        raise TrustError("Quick Look extension Info.plist is invalid") from error
    record = validate_quick_look_info(
        info,
        app_short_version=app_info.get("CFBundleShortVersionString"),
        app_bundle_version=app_info.get("CFBundleVersion"),
    )
    record["host_types"] = host_types

    executable = app / QUICK_LOOK_EXECUTABLE_RELATIVE
    if (
        not executable.is_file()
        or executable.is_symlink()
        or not os.access(executable, os.X_OK)
    ):
        raise TrustError("Quick Look extension executable is missing or invalid")
    for localization in ("en.lproj", "zh-Hans.lproj"):
        strings = (
            extension_path
            / "Contents/Resources"
            / localization
            / "InfoPlist.strings"
        )
        if not strings.is_file() or strings.is_symlink():
            raise TrustError(
                f"Quick Look extension localization is missing: {localization}"
            )
    return record


def quick_look_entitlement_problems(entitlements: str) -> list[str]:
    try:
        parsed = plistlib.loads(entitlements.encode("utf-8"))
    except plistlib.InvalidFileException:
        return ["Quick Look entitlements are not a valid property list"]
    if not isinstance(parsed, dict):
        return ["Quick Look entitlements root is not a dictionary"]

    problems: list[str] = []
    required = {
        "com.apple.security.app-sandbox",
        "com.apple.security.files.user-selected.read-only",
    }
    for key in sorted(required):
        if parsed.get(key) is not True:
            problems.append(f"required Quick Look entitlement is missing: {key}")

    forbidden = {
        "com.apple.security.files.user-selected.read-write",
        "com.apple.security.files.downloads.read-write",
        "com.apple.security.files.all",
        "com.apple.security.network.client",
        "com.apple.security.network.server",
        "com.apple.security.get-task-allow",
    }
    for key in sorted(forbidden):
        if parsed.get(key):
            problems.append(f"forbidden Quick Look entitlement is enabled: {key}")
    for key, value in sorted(parsed.items()):
        if (
            value
            and isinstance(key, str)
            and "temporary-exception" in key
        ):
            problems.append(f"Quick Look temporary exception is enabled: {key}")
    return problems


def quick_look_process_symbol_problems(undefined_symbols: str) -> list[str]:
    imported = {
        fields[-1]
        for line in undefined_symbols.splitlines()
        if (fields := line.split())
    }
    forbidden = sorted(
        symbol
        for symbol in imported
        if symbol in QUICK_LOOK_FORBIDDEN_PROCESS_SYMBOLS
        or symbol.startswith(QUICK_LOOK_FORBIDDEN_PROCESS_PREFIXES)
    )
    return [
        f"Quick Look executable imports forbidden process symbol: {symbol}"
        for symbol in forbidden
    ]


def is_quick_look_code(relative: Path) -> bool:
    return relative.parts[: len(QUICK_LOOK_RELATIVE.parts)] == (
        QUICK_LOOK_RELATIVE.parts
    )


def inspect_bundle(
    app: Path,
    architecture: frozenset[str],
    store: EvidenceStore,
    *,
    label_prefix: str = "",
) -> tuple[dict[str, object], list[Path]]:
    if not app.is_dir() or app.is_symlink():
        raise TrustError("app path must be a regular bundle directory")
    info_path = app / "Contents/Info.plist"
    if not info_path.is_file() or info_path.is_symlink():
        raise TrustError("app bundle has no regular Contents/Info.plist")
    try:
        info = plistlib.loads(info_path.read_bytes())
    except (OSError, plistlib.InvalidFileException) as error:
        raise TrustError("app bundle Info.plist is invalid") from error
    if not isinstance(info, dict):
        raise TrustError("app bundle Info.plist root must be a dictionary")
    expected_bundle_id, expected_minimum = read_release_config()
    if info.get("CFBundleIdentifier") != expected_bundle_id:
        raise TrustError("app bundle identifier does not match Tauri config")
    if info.get("LSMinimumSystemVersion") != expected_minimum:
        raise TrustError("app minimum system version does not match Tauri config")
    quick_look = inspect_quick_look_extension(app, info)

    executable_name = info.get("CFBundleExecutable")
    if not isinstance(executable_name, str) or not executable_name:
        raise TrustError("app bundle has no CFBundleExecutable")
    required = {
        Path("Contents/MacOS") / executable_name,
        Path("Contents/MacOS/sqz"),
        QUICK_LOOK_EXECUTABLE_RELATIVE,
    }
    for relative in required:
        path = app / relative
        if not path.is_file() or path.is_symlink() or not os.access(path, os.X_OK):
            raise TrustError(f"required executable is missing or invalid: {relative}")

    code_paths = discover_macho(app)
    code_relatives = {path.relative_to(app) for path in code_paths}
    missing_code = sorted(str(path) for path in required - code_relatives)
    if missing_code:
        raise TrustError(f"required executable is not Mach-O: {', '.join(missing_code)}")
    for relative in code_relatives:
        if relative.parts[:2] == ("Contents", "Resources"):
            raise TrustError(f"Mach-O code is not allowed in Resources: {relative}")
        if (
            len(relative.parts) < 3
            or relative.parts[0] != "Contents"
            or relative.parts[1] not in ALLOWED_CODE_DIRECTORIES
        ):
            raise TrustError(f"Mach-O code is outside a standard code directory: {relative}")

    app_declared_version = version_tuple(expected_minimum)
    quick_look_declared_version = version_tuple(QUICK_LOOK_MINIMUM_VERSION)
    code_records: list[dict[str, object]] = []
    for index, path in enumerate(code_paths, start=1):
        relative = path.relative_to(app)
        label = f"{label_prefix}code-{index:02d}-{path.name}"
        arch_result = store.run(f"{label}-architectures", ["lipo", "-archs", str(path)])
        actual_architecture = frozenset(arch_result.stdout.split())
        if actual_architecture != architecture:
            raise TrustError(
                f"{relative} has architecture {sorted(actual_architecture)}, "
                f"expected {sorted(architecture)}"
            )
        build_result = store.run(
            f"{label}-deployment", ["xcrun", "vtool", "-show-build", str(path)]
        )
        minimum_versions = re.findall(
            r"^\s*minos\s+(\d+(?:\.\d+){1,2})\s*$",
            build_result.stdout,
            re.MULTILINE,
        )
        if not minimum_versions:
            raise TrustError(f"{relative} has no LC_BUILD_VERSION minos")
        declared_version = (
            quick_look_declared_version
            if is_quick_look_code(relative)
            else app_declared_version
        )
        if any(version_tuple(value) > declared_version for value in minimum_versions):
            raise TrustError(
                f"{relative} requires a newer macOS version than Info.plist declares"
            )
        process_launch_imports: list[str] | None = None
        if relative == QUICK_LOOK_EXECUTABLE_RELATIVE:
            symbol_result = store.run(
                f"{label}-undefined-symbols", ["nm", "-u", str(path)]
            )
            process_problems = quick_look_process_symbol_problems(
                symbol_result.stdout
            )
            if process_problems:
                raise TrustError("; ".join(process_problems))
            process_launch_imports = []
        code_records.append(
            {
                "path": str(relative),
                "architectures": sorted(actual_architecture),
                "minimum_versions": sorted(set(minimum_versions)),
                **(
                    {"forbidden_process_imports": process_launch_imports}
                    if process_launch_imports is not None
                    else {}
                ),
            }
        )
    return (
        {
            "bundle_id": expected_bundle_id,
            "minimum_system_version": expected_minimum,
            "content_sha256": bundle_content_sha256(app),
            "quick_look": quick_look,
            "code": code_records,
        },
        code_paths,
    )


def inspect_dmg(dmg: Path, store: EvidenceStore, *, label_prefix: str = "") -> str:
    if not dmg.is_file() or dmg.is_symlink():
        raise TrustError("DMG path must be a regular file")
    store.run(f"{label_prefix}dmg-verify", ["hdiutil", "verify", str(dmg)])
    result = store.run(
        f"{label_prefix}dmg-imageinfo", ["hdiutil", "imageinfo", "-plist", str(dmg)]
    )
    try:
        image_info = plistlib.loads(result.stdout.encode("utf-8"))
    except plistlib.InvalidFileException as error:
        raise TrustError("hdiutil returned invalid image metadata") from error
    if not isinstance(image_info, dict):
        raise TrustError("hdiutil image metadata root must be a dictionary")
    image_format = image_info.get("Format")
    if image_format != "UDZO":
        raise TrustError(f"DMG must use read-only UDZO format, found {image_format}")
    properties = image_info.get("Properties")
    if isinstance(properties, dict) and properties.get("Writable") is True:
        raise TrustError("DMG is writable")
    return image_format


def code_signature_record(
    path: Path,
    label: str,
    store: EvidenceStore,
) -> tuple[dict[str, object], str, bool]:
    details_result = store.run(
        f"{label}-signature-details",
        ["codesign", "-d", "--verbose=4", str(path)],
        check=False,
    )
    details_text = details_result.stdout + details_result.stderr
    details = parse_codesign_details(details_text)
    entitlements_result = store.run(
        f"{label}-entitlements",
        ["codesign", "-d", "--entitlements", "-", "--xml", str(path)],
        check=False,
    )
    entitlements = entitlements_result.stdout
    entitlements_valid = entitlements_result.returncode == 0
    if entitlements_valid and entitlements.strip():
        try:
            parsed_entitlements = plistlib.loads(entitlements.encode("utf-8"))
        except plistlib.InvalidFileException:
            entitlements_valid = False
        else:
            entitlements_valid = isinstance(parsed_entitlements, dict)
    verify_result = store.run(
        f"{label}-signature-verify",
        ["codesign", "--verify", "--strict", "--verbose=4", str(path)],
        check=False,
    )
    verified = (
        details_result.returncode == 0
        and entitlements_valid
        and verify_result.returncode == 0
    )
    return details, entitlements, verified


def validate_distribution_root(mount: Path) -> None:
    expected = {".DS_Store", ".VolumeIcon.icns", "Applications", "Squallz.app"}
    try:
        entries = {entry.name: entry for entry in os.scandir(mount)}
    except OSError as error:
        raise TrustError("mounted DMG root could not be read") from error
    if set(entries) != expected:
        unexpected = sorted(set(entries) - expected)
        missing = sorted(expected - set(entries))
        details: list[str] = []
        if unexpected:
            details.append(f"unexpected: {', '.join(unexpected)}")
        if missing:
            details.append(f"missing: {', '.join(missing)}")
        raise TrustError(f"mounted DMG root layout is invalid ({'; '.join(details)})")

    app = entries["Squallz.app"]
    if app.is_symlink() or not app.is_dir(follow_symlinks=False):
        raise TrustError("mounted DMG Squallz.app is not a regular directory")
    applications = entries["Applications"]
    try:
        target = os.readlink(applications.path) if applications.is_symlink() else None
    except OSError as error:
        raise TrustError("mounted DMG Applications link could not be read") from error
    if target != "/Applications":
        raise TrustError("mounted DMG Applications must link to /Applications")
    for name in (".DS_Store", ".VolumeIcon.icns"):
        entry = entries[name]
        if entry.is_symlink() or not entry.is_file(follow_symlinks=False):
            raise TrustError(f"mounted DMG {name} is not a regular file")


def verify_signatures(
    app: Path,
    dmg: Path,
    code_paths: list[Path],
    store: EvidenceStore,
    *,
    identity: Optional[str],
    team_id: Optional[str],
    strict: bool,
    label_prefix: str = "",
) -> tuple[list[dict[str, object]], dict[str, object], list[str]]:
    problems: list[str] = []
    records: list[dict[str, object]] = []
    for index, path in enumerate(code_paths, start=1):
        relative = path.relative_to(app)
        label = f"{label_prefix}code-{index:02d}-{path.name}"
        details, entitlements, verified = code_signature_record(path, label, store)
        code_problems = [] if verified else ["strict code signature verification failed"]
        code_problems.extend(
            signature_problems(
                details,
                entitlements,
                expected_identity=identity,
                expected_team_id=team_id,
                require_runtime=True,
            )
        )
        problems.extend(f"{relative}: {problem}" for problem in code_problems)
        records.append(
            {
                "path": str(relative),
                "cdhash": details.get("cdhash"),
                "identifier": details.get("identifier"),
                "runtime": details.get("runtime"),
                "team_id": details.get("team_id"),
                "timestamped": bool(details.get("timestamp")),
                "verified": verified,
            }
        )

    quick_look_path = app / QUICK_LOOK_RELATIVE
    quick_look_details, quick_look_entitlements, quick_look_verified = (
        code_signature_record(
            quick_look_path,
            f"{label_prefix}quick-look-extension",
            store,
        )
    )
    quick_look_problems = (
        []
        if quick_look_verified
        else ["strict Quick Look extension signature verification failed"]
    )
    quick_look_problems.extend(
        signature_problems(
            quick_look_details,
            quick_look_entitlements,
            expected_identity=identity,
            expected_team_id=team_id,
            require_runtime=True,
        )
    )
    quick_look_problems.extend(
        quick_look_entitlement_problems(quick_look_entitlements)
    )
    problems.extend(
        f"{QUICK_LOOK_RELATIVE}: {problem}" for problem in quick_look_problems
    )
    records.append(
        {
            "path": str(QUICK_LOOK_RELATIVE),
            "kind": "app-extension",
            "cdhash": quick_look_details.get("cdhash"),
            "identifier": quick_look_details.get("identifier"),
            "runtime": quick_look_details.get("runtime"),
            "team_id": quick_look_details.get("team_id"),
            "timestamped": bool(quick_look_details.get("timestamp")),
            "verified": quick_look_verified,
        }
    )

    app_details, app_entitlements, app_verified = code_signature_record(
        app, f"{label_prefix}app", store
    )
    deep_result = store.run(
        f"{label_prefix}app-deep-signature-verify",
        ["codesign", "--verify", "--deep", "--strict", "--verbose=4", str(app)],
        check=False,
    )
    app_problems = [] if app_verified and deep_result.returncode == 0 else [
        "strict deep app signature verification failed"
    ]
    app_problems.extend(
        signature_problems(
            app_details,
            app_entitlements,
            expected_identity=identity,
            expected_team_id=team_id,
            require_runtime=True,
        )
    )
    problems.extend(f"app: {problem}" for problem in app_problems)

    dmg_details, dmg_entitlements, dmg_verified = code_signature_record(
        dmg, f"{label_prefix}dmg", store
    )
    dmg_problems = [] if dmg_verified else ["strict DMG signature verification failed"]
    dmg_problems.extend(
        signature_problems(
            dmg_details,
            dmg_entitlements,
            expected_identity=identity,
            expected_team_id=team_id,
            require_runtime=False,
        )
    )
    problems.extend(f"DMG: {problem}" for problem in dmg_problems)
    dmg_record = {
        "identifier": dmg_details.get("identifier"),
        "team_id": dmg_details.get("team_id"),
        "timestamped": bool(dmg_details.get("timestamp")),
        "verified": dmg_verified,
    }
    if strict and problems:
        raise TrustError("; ".join(problems))
    return records, dmg_record, problems


def inspect_mounted_distribution(
    app_bundle: dict[str, object],
    dmg: Path,
    architecture: frozenset[str],
    store: EvidenceStore,
    *,
    identity: Optional[str] = None,
    team_id: Optional[str] = None,
) -> None:
    with tempfile.TemporaryDirectory(prefix="squallz-release-mount-") as mount_raw:
        mount = Path(mount_raw)
        attached = False
        try:
            store.run(
                "dmg-attach-readonly",
                [
                    "hdiutil",
                    "attach",
                    "-readonly",
                    "-nobrowse",
                    "-mountpoint",
                    str(mount),
                    str(dmg),
                ],
            )
            attached = True
            validate_distribution_root(mount)
            mounted_app = mount / "Squallz.app"
            mounted_bundle, mounted_code = inspect_bundle(
                mounted_app,
                architecture,
                store,
                label_prefix="mounted-",
            )
            if mounted_bundle != app_bundle:
                raise TrustError(
                    "mounted DMG app does not match the inspected app bundle"
                )
            if identity is not None and team_id is not None:
                verify_signatures(
                    mounted_app,
                    dmg,
                    mounted_code,
                    store,
                    identity=identity,
                    team_id=team_id,
                    strict=True,
                    label_prefix="mounted-",
                )
                mounted_gatekeeper = store.run(
                    "mounted-app-gatekeeper",
                    [
                        "spctl",
                        "-a",
                        "-t",
                        "exec",
                        "-vvv",
                        str(mounted_app),
                    ],
                )
                mounted_gatekeeper_text = (
                    mounted_gatekeeper.stdout + mounted_gatekeeper.stderr
                )
                if not gatekeeper_accepts_notarized_developer_id(
                    mounted_gatekeeper_text
                ):
                    raise TrustError(
                        "Gatekeeper did not accept the app mounted from the DMG"
                    )
        finally:
            if attached:
                store.run(
                    "dmg-detach",
                    ["hdiutil", "detach", str(mount)],
                    check=False,
                )


def safe_artifact(path: Path) -> dict[str, object]:
    artifact: dict[str, object] = {"name": path.name}
    try:
        if path.is_file():
            artifact.update(
                {
                    "sha256": sha256_file(path),
                    "size_bytes": path.stat().st_size,
                }
            )
    except OSError:
        pass
    return artifact


def base_summary(args: argparse.Namespace) -> dict[str, object]:
    return {
        "schema": SCHEMA,
        "status": "blocked" if args.command == "inspect" else "failed",
        "generated_at_utc": datetime.now(timezone.utc)
        .replace(microsecond=0)
        .isoformat(),
        "architecture": args.architecture,
        "artifact": safe_artifact(Path(args.dmg)),
        "notarization": {"status": "not-submitted"},
        "stapled": False,
        "gatekeeper": False,
    }


def write_summary(
    store: EvidenceStore, summary: dict[str, object]
) -> Path:
    summary["logs"] = store.logs()
    path = store.directory / SUMMARY_NAME
    write_json(path, summary)
    return path


def runtime_error_message(error: BaseException) -> str:
    if isinstance(error, TrustError):
        return str(error)
    return (
        "release trust operation could not access required package data "
        f"({type(error).__name__})"
    )


def report_failure(
    store: EvidenceStore,
    summary: dict[str, object],
    error: BaseException,
    *,
    label: str,
) -> int:
    message = runtime_error_message(error)
    summary.update({"status": "failed", "errors": [message]})
    try:
        path = write_summary(store, summary)
    except (OSError, UnicodeError) as summary_error:
        print(
            f"{label} failed and its summary could not be written "
            f"({type(summary_error).__name__})",
            file=sys.stderr,
        )
        print(f"{label} failed: {message}", file=sys.stderr)
        return 1
    print(f"summary={path}", file=sys.stderr)
    print(f"{label} failed: {message}", file=sys.stderr)
    return 1


def inspect_command(args: argparse.Namespace) -> int:
    try:
        store = EvidenceStore(Path(args.evidence_dir))
    except (OSError, TrustError) as error:
        print(
            "macOS release inspection could not initialize its evidence directory: "
            f"{runtime_error_message(error)}",
            file=sys.stderr,
        )
        return 1
    summary = base_summary(args)
    try:
        architecture = normalized_architecture(args.architecture)
        app = Path(args.app)
        dmg = Path(args.dmg)
        bundle, code_paths = inspect_bundle(app, architecture, store)
        image_format = inspect_dmg(dmg, store)
        signatures, dmg_signature, blockers = verify_signatures(
            app,
            dmg,
            code_paths,
            store,
            identity=args.identity,
            team_id=args.team_id,
            strict=False,
        )
        inspect_mounted_distribution(bundle, dmg, architecture, store)
        blockers.append("notarization evidence was not supplied")
        summary.update(
            {
                "status": "blocked",
                "packaging_valid": True,
                "bundle": bundle,
                "dmg": {"format": image_format, "signature": dmg_signature},
                "signatures": signatures,
                "blockers": blockers,
            }
        )
    except (
        TrustError,
        OSError,
        UnicodeError,
        json.JSONDecodeError,
        plistlib.InvalidFileException,
        subprocess.SubprocessError,
    ) as error:
        summary.update(
            {
                "status": "failed",
                "packaging_valid": False,
            }
        )
        return report_failure(
            store, summary, error, label="macOS release inspection"
        )
    try:
        path = write_summary(store, summary)
    except (OSError, UnicodeError) as error:
        print(
            "macOS release inspection could not write its summary "
            f"({type(error).__name__})",
            file=sys.stderr,
        )
        return 1
    print(f"summary={path}")
    print("status=blocked")
    return 2


def notarize_command(args: argparse.Namespace) -> int:
    try:
        store = EvidenceStore(Path(args.evidence_dir))
    except (OSError, TrustError) as error:
        print(
            "macOS release trust could not initialize its evidence directory: "
            f"{runtime_error_message(error)}",
            file=sys.stderr,
        )
        return 1
    summary = base_summary(args)
    app = Path(args.app)
    dmg = Path(args.dmg)
    try:
        architecture = normalized_architecture(args.architecture)
        api_key_path = Path(args.api_key_path)
        if not api_key_path.is_file() or api_key_path.is_symlink():
            raise TrustError("App Store Connect API key path is not a regular file")
        if not args.identity.startswith("Developer ID Application: "):
            raise TrustError("release identity must be a Developer ID Application identity")
        if not args.team_id or args.team_id not in args.identity:
            raise TrustError("release identity does not contain the expected team id")

        bundle, code_paths = inspect_bundle(app, architecture, store)
        image_format = inspect_dmg(dmg, store)
        signatures, dmg_signature, _ = verify_signatures(
            app,
            dmg,
            code_paths,
            store,
            identity=args.identity,
            team_id=args.team_id,
            strict=True,
        )
        store.run(
            "app-notary-submission-policy",
            ["syspolicy_check", "notary-submission", str(app), "--json"],
        )
        submitted_sha256 = sha256_file(dmg)

        auth_args = [
            "--key",
            str(api_key_path),
            "--key-id",
            args.api_key_id,
            "--issuer",
            args.api_issuer,
        ]
        submit_result = store.run(
            "notary-submit-command",
            [
                "xcrun",
                "notarytool",
                "submit",
                *auth_args,
                "--wait",
                "--timeout",
                "60m",
                "--output-format",
                "json",
                str(dmg),
            ],
        )
        submit_path = store.directory / "notary-submit.json"
        submit_path.write_text(submit_result.stdout, encoding="utf-8")
        try:
            submit = json.loads(submit_result.stdout)
        except json.JSONDecodeError as error:
            raise TrustError("notary submit output is not valid JSON") from error
        submission_id = submit.get("id") if isinstance(submit, dict) else None
        try:
            normalized_id = str(uuid.UUID(str(submission_id)))
        except (ValueError, TypeError, AttributeError) as error:
            raise TrustError("notary submit output has no valid submission id") from error

        notary_log_path = store.directory / "notary-log.json"
        store.run(
            "notary-log-command",
            [
                "xcrun",
                "notarytool",
                "log",
                *auth_args,
                normalized_id,
                str(notary_log_path),
            ],
        )
        try:
            notary_log = json.loads(notary_log_path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as error:
            raise TrustError("notary log is not valid JSON") from error
        normalized_id, _ = parse_notary_evidence(submit, notary_log)

        store.run("dmg-staple", ["xcrun", "stapler", "staple", "-v", str(dmg)])
        store.run("dmg-stapler-validate", ["xcrun", "stapler", "validate", "-v", str(dmg)])
        inspect_dmg(dmg, store, label_prefix="final-")
        _, _, _ = verify_signatures(
            app,
            dmg,
            code_paths,
            store,
            identity=args.identity,
            team_id=args.team_id,
            strict=True,
            label_prefix="final-",
        )
        gatekeeper_result = store.run(
            "dmg-gatekeeper",
            [
                "spctl",
                "-a",
                "-t",
                "open",
                "--context",
                "context:primary-signature",
                "-vvv",
                str(dmg),
            ],
        )
        gatekeeper_text = (gatekeeper_result.stdout + gatekeeper_result.stderr).lower()
        if not gatekeeper_accepts_notarized_developer_id(gatekeeper_text):
            raise TrustError("Gatekeeper did not report Notarized Developer ID")

        inspect_mounted_distribution(
            bundle,
            dmg,
            architecture,
            store,
            identity=args.identity,
            team_id=args.team_id,
        )

        final_sha256 = sha256_file(dmg)
        summary.update(
            {
                "status": "pass",
                "packaging_valid": True,
                "artifact": {
                    "name": dmg.name,
                    "sha256": final_sha256,
                    "size_bytes": dmg.stat().st_size,
                },
                "submitted_dmg_sha256": submitted_sha256,
                "bundle": bundle,
                "dmg": {"format": image_format, "signature": dmg_signature},
                "signatures": signatures,
                "identity": {
                    "authority": args.identity,
                    "team_id": args.team_id,
                },
                "notarization": {
                    "id": normalized_id,
                    "status": "Accepted",
                    "submit": relative_file_ref(submit_path),
                    "log": relative_file_ref(notary_log_path),
                },
                "stapled": True,
                "gatekeeper": True,
            }
        )
    except (
        TrustError,
        OSError,
        UnicodeError,
        json.JSONDecodeError,
        plistlib.InvalidFileException,
        subprocess.SubprocessError,
    ) as error:
        return report_failure(store, summary, error, label="macOS release trust")
    try:
        path = write_summary(store, summary)
    except (OSError, UnicodeError) as error:
        print(
            "macOS release trust could not write its summary "
            f"({type(error).__name__})",
            file=sys.stderr,
        )
        return 1
    print(f"summary={path}")
    print("status=pass")
    return 0


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Inspect or notarize the official Squallz macOS release DMG."
    )
    subparsers = parser.add_subparsers(dest="command", required=True)

    inspect_parser = subparsers.add_parser(
        "inspect", description="Inspect package structure without claiming release trust."
    )
    inspect_parser.add_argument("--app", required=True)
    inspect_parser.add_argument("--dmg", required=True)
    inspect_parser.add_argument("--evidence-dir", required=True)
    inspect_parser.add_argument("--architecture", required=True)
    inspect_parser.add_argument("--identity")
    inspect_parser.add_argument("--team-id")
    inspect_parser.set_defaults(handler=inspect_command)

    notarize_parser = subparsers.add_parser(
        "notarize", description="Verify, notarize, staple, and assess a release DMG."
    )
    notarize_parser.add_argument("--app", required=True)
    notarize_parser.add_argument("--dmg", required=True)
    notarize_parser.add_argument("--evidence-dir", required=True)
    notarize_parser.add_argument("--architecture", required=True)
    notarize_parser.add_argument("--identity", required=True)
    notarize_parser.add_argument("--team-id", required=True)
    notarize_parser.add_argument("--api-key-id", required=True)
    notarize_parser.add_argument("--api-issuer", required=True)
    notarize_parser.add_argument("--api-key-path", required=True)
    notarize_parser.set_defaults(handler=notarize_command)
    return parser


def main() -> int:
    args = build_parser().parse_args()
    return args.handler(args)


if __name__ == "__main__":
    raise SystemExit(main())
