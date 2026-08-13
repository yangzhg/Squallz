#!/usr/bin/env python3

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import stat
import subprocess
import sys
import tempfile
import zlib
from collections.abc import Callable, Sequence
from pathlib import Path
from typing import Any


PLATFORMS = ("macos-arm64", "macos-x64", "windows-x64", "linux-x64")
PROFILES = ("release", "debug")
SFX_RESOURCE_SOURCE = "../../target/release/sqz-sfx-template.stub"
SFX_RESOURCE_TARGET = "bin/sqz-sfx.stub"
LINUX_SFX_DATA_MAGIC = b"SQZSFXD1"
LINUX_SFX_DATA_HEADER_BYTES = 48
TOML_SECTION = re.compile(r"^\s*\[([^\]]+)\]\s*(?:#.*)?$")
TOML_VERSION = re.compile(
    r"""^\s*version\s*=\s*(?:"([^"]+)"|'([^']+)')\s*(?:#.*)?$"""
)


class SmokeError(RuntimeError):
    pass


Runner = Callable[..., subprocess.CompletedProcess[str]]


def linux_sfx_data_info(path: Path) -> tuple[int, int, bytes] | None:
    try:
        file_bytes = path.stat().st_size
        with path.open("rb") as handle:
            magic = handle.read(len(LINUX_SFX_DATA_MAGIC))
            if magic != LINUX_SFX_DATA_MAGIC:
                return None
            length = handle.read(8)
            expected_digest = handle.read(32)
            if len(length) != 8 or len(expected_digest) != 32:
                raise SmokeError("Linux SFX template data header is truncated")
            runtime_bytes = int.from_bytes(length, "little")
            if (
                runtime_bytes <= 0
                or LINUX_SFX_DATA_HEADER_BYTES + runtime_bytes != file_bytes
            ):
                raise SmokeError("Linux SFX template data has an invalid length")
            digest = hashlib.sha256()
            remaining = runtime_bytes
            while remaining > 0:
                chunk = handle.read(min(64 * 1024, remaining))
                if not chunk:
                    raise SmokeError("Linux SFX template data is truncated")
                digest.update(chunk)
                remaining -= len(chunk)
    except OSError as error:
        raise SmokeError("Linux SFX template data could not be inspected") from error
    if digest.digest() != expected_digest:
        raise SmokeError("Linux SFX template data failed its SHA-256 check")
    return LINUX_SFX_DATA_HEADER_BYTES, runtime_bytes, expected_digest


def extract_linux_sfx_runtime(template: Path, destination: Path) -> None:
    info = linux_sfx_data_info(template)
    if info is None:
        raise SmokeError("packaged Linux SFX template is not a data resource")
    offset, runtime_bytes, expected_digest = info
    digest = hashlib.sha256()
    try:
        with template.open("rb") as source, destination.open("xb") as output:
            source.seek(offset)
            remaining = runtime_bytes
            while remaining > 0:
                chunk = source.read(min(64 * 1024, remaining))
                if not chunk:
                    raise SmokeError("Linux SFX runtime probe source is truncated")
                output.write(chunk)
                digest.update(chunk)
                remaining -= len(chunk)
        destination.chmod(0o700)
    except (OSError, SmokeError) as error:
        destination.unlink(missing_ok=True)
        if isinstance(error, SmokeError):
            raise
        raise SmokeError("dedicated SFX runtime probe could not be prepared") from error
    if digest.digest() != expected_digest:
        destination.unlink(missing_ok=True)
        raise SmokeError("dedicated SFX runtime probe failed its SHA-256 check")


def release_binary_path(project_root: Path, platform: str, profile: str) -> Path:
    target = project_root / "target" / profile
    if platform in {"macos-arm64", "macos-x64"}:
        return target / "bundle/macos/Squallz.app/Contents/MacOS/sqz"
    if platform == "windows-x64":
        return target / "sqz.exe"
    if platform == "linux-x64":
        return target / "sqz"
    raise SmokeError(f"unsupported release platform: {platform}")


def release_sfx_template_path(
    project_root: Path,
    platform: str,
    profile: str,
) -> Path:
    if platform in {"macos-arm64", "macos-x64"}:
        return project_root / "target" / profile / "bundle/macos/Squallz.app"
    if platform in {"windows-x64", "linux-x64"}:
        return project_root / "target/release/sqz-sfx-template.stub"
    raise SmokeError(f"unsupported release platform: {platform}")


def sfx_target(platform: str) -> str:
    if platform in {"macos-arm64", "macos-x64"}:
        return "macos"
    if platform == "windows-x64":
        return "windows"
    if platform == "linux-x64":
        return "linux"
    raise SmokeError(f"unsupported release platform: {platform}")


def require_desktop_bundle_config(
    project_root: Path,
    platform: str,
    template: Path,
) -> set[str] | None:
    if platform == "windows-x64":
        platform_name = "windows"
        bundle_target = "nsis"
    elif platform == "linux-x64":
        platform_name = "linux"
        bundle_target = "appimage"
    else:
        return None

    config_path = (
        project_root / f"crates/squallz-gui/tauri.{platform_name}.conf.json"
    )
    try:
        config = json.loads(config_path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as error:
        raise SmokeError(
            f"could not read the {platform_name} desktop bundle config"
        ) from error
    bundle = config.get("bundle")
    if not isinstance(bundle, dict) or bundle.get("targets") != [bundle_target]:
        raise SmokeError(
            f"{platform_name} desktop bundle must target {bundle_target}"
        )
    resources = bundle.get("resources")
    if (
        not isinstance(resources, dict)
        or resources.get(SFX_RESOURCE_SOURCE) != SFX_RESOURCE_TARGET
    ):
        raise SmokeError(
            f"{platform_name} desktop bundle does not install the dedicated SFX runtime"
        )
    configured_source = (config_path.parent / SFX_RESOURCE_SOURCE).resolve()
    if configured_source != template.resolve():
        raise SmokeError(
            f"{platform_name} desktop bundle uses a different SFX template source"
        )
    if platform != "linux-x64":
        return None

    file_associations = bundle.get("fileAssociations")
    if not isinstance(file_associations, list) or not file_associations:
        raise SmokeError("Linux desktop bundle config has no MIME associations")
    mime_types: set[str] = set()
    for association in file_associations:
        if not isinstance(association, dict):
            raise SmokeError("Linux desktop bundle config has an invalid MIME association")
        mime_type = association.get("mimeType")
        if not isinstance(mime_type, str) or not mime_type.strip():
            raise SmokeError("Linux desktop bundle config has an invalid MIME association")
        mime_types.add(mime_type.strip())
    if not mime_types:
        raise SmokeError("Linux desktop bundle config has no MIME associations")
    return mime_types


def require_single_desktop_bundle(
    project_root: Path,
    platform: str,
    profile: str,
) -> Path:
    if platform == "windows-x64":
        bundle_dir = project_root / "target" / profile / "bundle/nsis"
        pattern = "*.exe"
        description = "Windows NSIS installer"
    elif platform == "linux-x64":
        bundle_dir = project_root / "target" / profile / "bundle/appimage"
        pattern = "*.AppImage"
        description = "Linux AppImage"
    else:
        raise SmokeError(f"unsupported desktop bundle platform: {platform}")

    candidates = sorted(
        path
        for path in bundle_dir.glob(pattern)
        if path.is_file() and not path.is_symlink()
    )
    if len(candidates) != 1:
        raise SmokeError(
            f"expected exactly one {description}, found {len(candidates)}"
        )
    artifact = candidates[0].resolve()
    if platform == "linux-x64" and not os.access(artifact, os.X_OK):
        raise SmokeError("Linux AppImage is not executable")
    return artifact


def require_packaged_runtime_file(
    packaged_runtime: Path,
    template: Path,
    target: str,
) -> None:
    try:
        metadata = packaged_runtime.lstat()
    except OSError as error:
        raise SmokeError(
            f"{target} desktop bundle is missing {SFX_RESOURCE_TARGET}"
        ) from error
    if not stat.S_ISREG(metadata.st_mode) or packaged_runtime.is_symlink():
        raise SmokeError(
            f"{target} desktop bundle SFX runtime is not a regular file"
        )
    try:
        matches_source = file_digest(packaged_runtime) == file_digest(template)
    except OSError as error:
        raise SmokeError(
            f"{target} desktop bundle SFX runtime could not be inspected"
        ) from error
    if not matches_source:
        raise SmokeError(
            f"{target} desktop bundle SFX runtime differs from the build template"
        )
    if target == "linux" and os.name != "nt" and stat.S_IMODE(metadata.st_mode) != 0o644:
        raise SmokeError("Linux desktop bundle SFX runtime must use data mode 0644")
    if target == "linux" and linux_sfx_data_info(packaged_runtime) is None:
        raise SmokeError("Linux desktop bundle SFX runtime is not a data resource")
    if target == "windows" and windows_pe_certificate_table(packaged_runtime) is not None:
        raise SmokeError("Windows desktop bundle SFX runtime must remain unsigned")


def require_packaged_linux_mime_types(
    app_dir: Path,
    configured_mime_types: set[str],
) -> None:
    desktop_dir = app_dir / "usr/share/applications"
    desktop_files = sorted(
        path
        for path in desktop_dir.glob("*.desktop")
        if path.is_file() and not path.is_symlink()
    )
    packaged_mime_types: set[str] = set()
    try:
        for desktop_file in desktop_files:
            in_desktop_entry = False
            for raw_line in desktop_file.read_text(encoding="utf-8").splitlines():
                line = raw_line.strip()
                if line.startswith("[") and line.endswith("]"):
                    in_desktop_entry = line == "[Desktop Entry]"
                elif in_desktop_entry and line.startswith("MimeType="):
                    packaged_mime_types.update(
                        mime_type.strip()
                        for mime_type in line.removeprefix("MimeType=").split(";")
                        if mime_type.strip()
                    )
    except (OSError, UnicodeError) as error:
        raise SmokeError("Linux desktop bundle MIME associations could not be read") from error
    if not packaged_mime_types:
        raise SmokeError("Linux desktop bundle has no packaged MIME associations")
    if packaged_mime_types != configured_mime_types:
        raise SmokeError(
            "Linux desktop bundle MIME associations differ from the bundle config"
        )


def invoke_bundle_artifact(
    command: Sequence[os.PathLike[str] | str],
    phase: str,
    workspace: Path,
    runner: Runner,
) -> None:
    try:
        result = runner(
            [os.fspath(argument) for argument in command],
            cwd=workspace,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            check=False,
        )
    except OSError as error:
        raise SmokeError(f"{phase} could not start") from error
    if result.returncode != 0:
        detail = result.stderr.strip().replace(os.fspath(workspace), "<smoke>")
        suffix = f": {detail}" if detail else ""
        raise SmokeError(
            f"{phase} failed with exit code {result.returncode}{suffix}"
        )


def require_packaged_desktop_runtime(
    project_root: Path,
    platform: str,
    profile: str,
    template: Path,
    workspace: Path,
    runner: Runner = subprocess.run,
) -> Path:
    configured_mime_types = require_desktop_bundle_config(
        project_root,
        platform,
        template,
    )
    bundle = require_single_desktop_bundle(project_root, platform, profile)
    if platform == "linux-x64":
        extract_root = workspace / "appimage-extract"
        extract_root.mkdir(parents=True)
        invoke_bundle_artifact(
            (bundle, "--appimage-extract"),
            "Linux AppImage extraction",
            extract_root,
            runner,
        )
        app_dir = extract_root / "squashfs-root"
        packaged_runtime = app_dir / "usr/lib/Squallz" / SFX_RESOURCE_TARGET
        require_packaged_runtime_file(packaged_runtime, template, "linux")
        if configured_mime_types is None:
            raise SmokeError("Linux desktop bundle config has no MIME associations")
        require_packaged_linux_mime_types(app_dir, configured_mime_types)
        return bundle

    install_root = workspace / "nsis-install"
    invoke_bundle_artifact(
        (bundle, "/S", "/NS", f"/D={install_root}"),
        "Windows NSIS installation",
        workspace,
        runner,
    )
    uninstaller = install_root / "uninstall.exe"
    validation_error: SmokeError | None = None
    try:
        require_packaged_runtime_file(
            install_root / SFX_RESOURCE_TARGET,
            template,
            "windows",
        )
    except SmokeError as error:
        validation_error = error
    if not uninstaller.is_file():
        if validation_error is not None:
            raise validation_error
        raise SmokeError("Windows NSIS installation did not provide an uninstaller")
    try:
        invoke_bundle_artifact(
            (uninstaller, "/S"),
            "Windows NSIS cleanup",
            workspace,
            runner,
        )
    except SmokeError:
        if validation_error is None:
            raise
    if validation_error is not None:
        raise validation_error
    return bundle


def workspace_version(manifest: Path) -> str:
    try:
        lines = manifest.read_text(encoding="utf-8").splitlines()
    except (OSError, UnicodeError) as error:
        raise SmokeError(f"could not read workspace version from {manifest.name}") from error
    in_workspace_package = False
    for line in lines:
        section = TOML_SECTION.fullmatch(line)
        if section is not None:
            in_workspace_package = section.group(1).strip() == "workspace.package"
            continue
        if not in_workspace_package:
            continue
        match = TOML_VERSION.fullmatch(line)
        if match is not None:
            return match.group(1) or match.group(2)
    raise SmokeError(f"workspace version is missing from {manifest.name}")


def require_binary(path: Path) -> Path:
    try:
        resolved = path.resolve(strict=True)
    except OSError as error:
        raise SmokeError(f"release CLI is missing: {path}") from error
    if not resolved.is_file():
        raise SmokeError(f"release CLI is not a regular file: {path}")
    if os.name != "nt" and not os.access(resolved, os.X_OK):
        raise SmokeError(f"release CLI is not executable: {path}")
    return resolved


def require_sfx_template(path: Path, target: str) -> Path:
    try:
        resolved = path.resolve(strict=True)
    except OSError as error:
        raise SmokeError(f"{target} SFX template is missing: {path}") from error
    if target == "macos":
        if (
            not resolved.is_dir()
            or resolved.suffix.lower() != ".app"
            or not (resolved / "Contents/Info.plist").is_file()
        ):
            raise SmokeError("macOS SFX template is not a valid app bundle")
        return resolved
    try:
        metadata = path.lstat()
    except OSError as error:
        raise SmokeError(f"{target} SFX template could not be inspected") from error
    if not stat.S_ISREG(metadata.st_mode) or path.is_symlink():
        raise SmokeError(f"{target} SFX template is not a regular file")
    if os.name != "nt" and not os.access(resolved, os.X_OK):
        if target != "linux" or linux_sfx_data_info(resolved) is None:
            raise SmokeError(f"{target} SFX template is not executable")
    if target == "windows" and windows_pe_certificate_table(resolved) is not None:
        raise SmokeError("Windows SFX template must remain unsigned before assembly")
    return resolved


def require_dedicated_template_size(template: Path, binary: Path) -> None:
    try:
        template_bytes = template.stat().st_size
        linux_data = linux_sfx_data_info(template)
        if linux_data is not None:
            template_bytes = linux_data[1]
        binary_bytes = binary.stat().st_size
    except OSError as error:
        raise SmokeError("SFX runtime size could not be inspected") from error
    if template_bytes >= binary_bytes:
        raise SmokeError("dedicated SFX runtime is not smaller than the full release CLI")


def windows_pe_certificate_table(path: Path) -> tuple[int, int] | None:
    try:
        with path.open("rb") as handle:
            dos_header = handle.read(64)
            if not dos_header.startswith(b"MZ"):
                return None
            if len(dos_header) != 64:
                raise SmokeError("Windows SFX template has a truncated DOS header")
            pe_offset = int.from_bytes(dos_header[0x3C:0x40], "little")
            handle.seek(pe_offset)
            pe_header = handle.read(24)
            if len(pe_header) != 24 or pe_header[:4] != b"PE\0\0":
                raise SmokeError("Windows SFX template has an invalid PE header")
            optional_size = int.from_bytes(pe_header[20:22], "little")
            optional = handle.read(optional_size)
            if len(optional) != optional_size:
                raise SmokeError("Windows SFX template has a truncated optional header")
    except OSError as error:
        raise SmokeError("Windows SFX template could not be inspected") from error

    magic = int.from_bytes(optional[:2], "little")
    if magic == 0x10B:
        data_directories = 96
        directory_count = 92
    elif magic == 0x20B:
        data_directories = 112
        directory_count = 108
    else:
        raise SmokeError("Windows SFX template has an unsupported PE optional header")
    if directory_count + 4 > len(optional):
        raise SmokeError("Windows SFX template has an incomplete PE data directory")
    if int.from_bytes(optional[directory_count : directory_count + 4], "little") <= 4:
        return None
    certificate_entry = data_directories + 4 * 8
    if certificate_entry + 8 > len(optional):
        raise SmokeError("Windows SFX template has an incomplete certificate directory")
    offset = int.from_bytes(optional[certificate_entry : certificate_entry + 4], "little")
    size = int.from_bytes(optional[certificate_entry + 4 : certificate_entry + 8], "little")
    return None if offset == 0 and size == 0 else (offset, size)


def invoke(
    binary: Path,
    phase: str,
    arguments: Sequence[os.PathLike[str] | str],
    workspace: Path,
    runner: Runner,
) -> subprocess.CompletedProcess[str]:
    command = [os.fspath(binary), *(os.fspath(argument) for argument in arguments)]
    try:
        result = runner(
            command,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            check=False,
        )
    except OSError as error:
        raise SmokeError(f"{phase} could not start the release CLI") from error
    if result.returncode != 0:
        detail = result.stderr.strip().replace(os.fspath(workspace), "<smoke>")
        suffix = f": {detail}" if detail else ""
        raise SmokeError(f"{phase} failed with exit code {result.returncode}{suffix}")
    return result


def invoke_json(
    binary: Path,
    phase: str,
    arguments: Sequence[os.PathLike[str] | str],
    workspace: Path,
    runner: Runner,
) -> Any:
    result = invoke(binary, phase, arguments, workspace, runner)
    try:
        return json.loads(result.stdout)
    except json.JSONDecodeError as error:
        raise SmokeError(f"{phase} did not return valid JSON") from error


def require_success_report(value: Any, phase: str) -> dict[str, Any]:
    if not isinstance(value, dict) or value.get("ok") is not True:
        raise SmokeError(f"{phase} did not report success")
    return value


def file_digest(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(64 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def file_crc32(path: Path) -> str:
    checksum = 0
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(64 * 1024), b""):
            checksum = zlib.crc32(chunk, checksum)
    return f"{checksum & 0xFFFFFFFF:08x}"


def source_files(source: Path) -> dict[str, str]:
    return {
        (Path(source.name) / path.relative_to(source)).as_posix(): file_digest(path)
        for path in sorted(source.rglob("*"))
        if path.is_file()
    }


def extracted_files(destination: Path) -> dict[str, str]:
    return {
        path.relative_to(destination).as_posix(): file_digest(path)
        for path in sorted(destination.rglob("*"))
        if path.is_file()
    }


def create_fixture(workspace: Path) -> Path:
    source = workspace / "release smoke"
    (source / "资料").mkdir(parents=True)
    (source / "release smoke #1.txt").write_text(
        "Squallz release binary smoke\n",
        encoding="utf-8",
    )
    (source / "资料/数据.bin").write_bytes(bytes(range(256)) * 256)
    (source / "empty.dat").touch()
    return source


def require_generic_split_family(
    report: dict[str, Any],
    archive: Path,
) -> list[Path]:
    volume_count = report.get("volumes")
    if (
        report.get("split") is not True
        or type(volume_count) is not int
        or volume_count < 2
    ):
        raise SmokeError("compress report did not describe a split archive")

    expected = [
        archive.with_name(f"{archive.name}.{index:03d}")
        for index in range(1, volume_count + 1)
    ]
    outputs = report.get("outputs")
    if (
        not isinstance(outputs, list)
        or any(not isinstance(path, str) or not path for path in outputs)
        or [Path(path) for path in outputs] != expected
        or report.get("output") != os.fspath(expected[0])
        or report.get("primary_output") != os.fspath(expected[0])
    ):
        raise SmokeError("compress report did not identify the generic split output family")

    actual = sorted(archive.parent.glob(f"{archive.name}.*"))
    if (
        archive.exists()
        or actual != expected
        or any(not member.is_file() or member.stat().st_size == 0 for member in expected)
        or report.get("total_bytes")
        != sum(member.stat().st_size for member in expected)
    ):
        raise SmokeError("generic split output family is incomplete")
    return expected


def require_single_zip_payload(
    report: dict[str, Any],
    archive: Path,
    expected_entries: int,
) -> None:
    if (
        report.get("operation") != "compress"
        or report.get("split") is not False
        or report.get("volumes") != 1
        or report.get("output") != os.fspath(archive)
        or report.get("primary_output") != os.fspath(archive)
        or report.get("outputs") != [os.fspath(archive)]
        or report.get("tested_after_create") is not True
        or not isinstance(report.get("entries_tested_after_create"), int)
        or report["entries_tested_after_create"] < expected_entries
        or not archive.is_file()
        or report.get("total_bytes") != archive.stat().st_size
    ):
        raise SmokeError("SFX payload compress did not prove a tested single ZIP was created")


def sfx_output_path(workspace: Path, target: str) -> Path:
    if target == "macos":
        return workspace / "release-smoke-sfx.app"
    if target == "windows":
        return workspace / "release-smoke-sfx.exe"
    return workspace / "release-smoke-sfx.run"


def require_sfx_create_report(
    report: dict[str, Any],
    payload: Path,
    output: Path,
    target: str,
) -> None:
    layout = "macos_app" if target == "macos" else "single_file"
    expected_crc32 = "00000000" if target == "macos" else file_crc32(payload)
    expected_sha256 = file_digest(payload) if target == "macos" else None
    artifact_exists = output.is_dir() if target == "macos" else output.is_file()
    if (
        report.get("operation") != "sfx_create"
        or report.get("path") != os.fspath(output)
        or report.get("target") != target
        or report.get("layout") != layout
        or report.get("payload_bytes") != payload.stat().st_size
        or report.get("payload_crc32") != expected_crc32
        or report.get("payload_sha256") != expected_sha256
        or type(report.get("stub_bytes")) is not int
        or report["stub_bytes"] <= 0
        or type(report.get("total_bytes")) is not int
        or report["total_bytes"] <= report["payload_bytes"]
        or (
            target != "macos"
            and report["total_bytes"] != output.stat().st_size
        )
        or report.get("requires_signing") is not True
        or report.get("preserved_outputs") != []
        or report.get("auto_run") is not False
        or not artifact_exists
    ):
        raise SmokeError("sfx create report did not prove a valid host artifact")


def require_sfx_inspect_report(
    report: dict[str, Any],
    create_report: dict[str, Any],
    output: Path,
) -> None:
    stable_fields = (
        "target",
        "layout",
        "payload_bytes",
        "total_bytes",
        "payload_crc32",
        "payload_sha256",
        "auto_run",
    )
    if (
        report.get("operation") != "sfx_inspect"
        or report.get("path") != os.fspath(output)
        or report.get("checksum_verified") is not True
        or type(report.get("stub_bytes")) is not int
        or report["stub_bytes"] <= 0
        or any(report.get(field) != create_report.get(field) for field in stable_fields)
    ):
        raise SmokeError("sfx inspect did not verify the created artifact and payload checksum")


def create_and_inspect_sfx(
    binary: Path,
    payload: Path,
    output: Path,
    target: str,
    template: Path,
    common: Sequence[str],
    workspace: Path,
    runner: Runner,
    phase: str,
) -> dict[str, Any]:
    create_report = require_success_report(
        invoke_json(
            binary,
            f"{phase} create",
            (
                *common,
                "sfx",
                "create",
                payload,
                "--output",
                output,
                "--target",
                target,
                "--stub",
                template,
                "--json",
            ),
            workspace,
            runner,
        ),
        f"{phase} create",
    )
    require_sfx_create_report(create_report, payload, output, target)
    if target == "linux" and output.stat().st_mode & (
        stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH
    ) == 0:
        raise SmokeError(f"{phase} output is missing its Linux executable mode")

    inspect_report = require_success_report(
        invoke_json(
            binary,
            f"{phase} inspect",
            (*common, "sfx", "inspect", output, "--json"),
            workspace,
            runner,
        ),
        f"{phase} inspect",
    )
    require_sfx_inspect_report(inspect_report, create_report, output)
    return create_report


def require_runtime_test_report(
    report: dict[str, Any], expected_entries: int, phase: str
) -> None:
    if (
        not isinstance(report.get("entries_tested"), int)
        or report["entries_tested"] < expected_entries
        or report.get("problems") != []
    ):
        raise SmokeError(f"{phase} did not prove payload integrity")


def run_smoke(
    binary: Path,
    expected_version: str,
    workspace: Path,
    platform: str,
    host_template: Path,
    runner: Runner = subprocess.run,
) -> int:
    binary = require_binary(binary)
    target = sfx_target(platform)
    host_template = require_sfx_template(host_template, target)
    dedicated_template = target != "macos" and host_template != binary
    if dedicated_template:
        require_dedicated_template_size(host_template, binary)
    if (
        target == "linux"
        and dedicated_template
        and linux_sfx_data_info(host_template) is None
    ):
        raise SmokeError("Linux build SFX template must be a data resource")
    if (
        target == "linux"
        and dedicated_template
        and os.name != "nt"
        and stat.S_IMODE(host_template.lstat().st_mode) != 0o644
    ):
        raise SmokeError("Linux build SFX template must use data mode 0644")
    source = create_fixture(workspace)
    expected_files = source_files(source)
    archive = workspace / "release-smoke.zip"
    destination = workspace / "extracted"
    sfx_payload = workspace / "release-smoke-sfx-payload.zip"
    sfx_output = sfx_output_path(workspace, target)
    sfx_destination = workspace / "sfx-extracted"
    common = ("--lang", "en-US", "--quiet", "--color", "never")

    if dedicated_template:
        runtime_probe = host_template
        if target == "linux":
            runtime_probe = workspace / "sqz-sfx-runtime-probe"
            extract_linux_sfx_runtime(host_template, runtime_probe)
        runtime_version = invoke(
            runtime_probe,
            "dedicated sfx runtime version",
            ("--version",),
            workspace,
            runner,
        )
        if runtime_version.stdout.strip() != f"sqz-sfx {expected_version}":
            raise SmokeError("dedicated SFX template has the wrong runtime identity")

    version = invoke(binary, "version", ("--version",), workspace, runner)
    if version.stdout.strip() != f"sqz {expected_version}":
        raise SmokeError("release CLI version does not match Cargo.toml")

    compress = require_success_report(
        invoke_json(
            binary,
            "compress",
            (
                *common,
                "compress",
                source,
                "--output",
                archive,
                "--format",
                "zip",
                "--level",
                "0",
                "--split",
                "64k",
                "--split-mode",
                "generic",
                "--test-after-create",
                "--json",
            ),
            workspace,
            runner,
        ),
        "compress",
    )
    if (
        compress.get("operation") != "compress"
        or compress.get("tested_after_create") is not True
        or not isinstance(compress.get("entries_tested_after_create"), int)
        or compress["entries_tested_after_create"] < len(expected_files)
    ):
        raise SmokeError("compress report did not prove a tested split ZIP was created")
    volumes = require_generic_split_family(compress, archive)
    non_primary_volume = volumes[1]

    listing = invoke_json(
        binary,
        "list",
        (*common, "list", non_primary_volume, "--json"),
        workspace,
        runner,
    )
    if not isinstance(listing, list):
        raise SmokeError("list did not return an entry array")
    listed_files = [
        entry.get("path")
        for entry in listing
        if isinstance(entry, dict) and entry.get("type") == "file"
    ]
    if (
        any(not isinstance(path, str) for path in listed_files)
        or sorted(listed_files) != sorted(expected_files)
    ):
        raise SmokeError("list did not return the expected file set")

    test = require_success_report(
        invoke_json(
            binary,
            "test",
            (*common, "test", non_primary_volume, "--json"),
            workspace,
            runner,
        ),
        "test",
    )
    if (
        not isinstance(test.get("entries_tested"), int)
        or test["entries_tested"] < len(expected_files)
        or test.get("problems") != []
    ):
        raise SmokeError("test did not prove ZIP integrity")

    extract = require_success_report(
        invoke_json(
            binary,
            "extract",
            (
                *common,
                "extract",
                non_primary_volume,
                "--dest",
                destination,
                "--overwrite",
                "all",
                "--json",
            ),
            workspace,
            runner,
        ),
        "extract",
    )
    counts = extract.get("counts")
    if (
        extract.get("problems") != []
        or not isinstance(counts, dict)
        or counts.get("failed") != 0
    ):
        raise SmokeError("extract reported failed entries")
    if extracted_files(destination) != expected_files:
        raise SmokeError("extracted files do not match the source bytes")

    payload_report = require_success_report(
        invoke_json(
            binary,
            "sfx payload compress",
            (
                *common,
                "compress",
                source,
                "--output",
                sfx_payload,
                "--format",
                "zip",
                "--level",
                "0",
                "--test-after-create",
                "--json",
            ),
            workspace,
            runner,
        ),
        "sfx payload compress",
    )
    require_single_zip_payload(payload_report, sfx_payload, len(expected_files))

    create_and_inspect_sfx(
        binary,
        sfx_payload,
        sfx_output,
        target,
        host_template,
        common,
        workspace,
        runner,
        "sfx",
    )

    if target != "macos":
        sfx_listing = invoke_json(
            sfx_output,
            "sfx runtime list",
            ("--list", "--json"),
            workspace,
            runner,
        )
        if not isinstance(sfx_listing, list):
            raise SmokeError("sfx runtime list did not return an entry array")
        listed_sfx_files = [
            entry.get("path")
            for entry in sfx_listing
            if isinstance(entry, dict) and entry.get("type") == "file"
        ]
        if (
            any(not isinstance(path, str) for path in listed_sfx_files)
            or sorted(listed_sfx_files) != sorted(expected_files)
        ):
            raise SmokeError("sfx runtime list did not return the expected file set")

        sfx_test = require_success_report(
            invoke_json(
                sfx_output,
                "sfx runtime test",
                ("--test", "--json"),
                workspace,
                runner,
            ),
            "sfx runtime test",
        )
        require_runtime_test_report(sfx_test, len(expected_files), "sfx runtime test")

        sfx_extract = require_success_report(
            invoke_json(
                sfx_output,
                "sfx runtime extract",
                ("-d", sfx_destination, "--json"),
                workspace,
                runner,
            ),
            "sfx runtime extract",
        )
        sfx_counts = sfx_extract.get("counts")
        if (
            sfx_extract.get("problems") != []
            or not isinstance(sfx_counts, dict)
            or sfx_counts.get("failed") != 0
        ):
            raise SmokeError("sfx runtime extract reported failed entries")
        if extracted_files(sfx_destination) != expected_files:
            raise SmokeError("SFX-extracted files do not match the source bytes")

        legacy_template_signed = (
            target == "windows" and windows_pe_certificate_table(binary) is not None
        )
        if not legacy_template_signed:
            legacy_output = sfx_output.with_name(
                f"release-smoke-legacy-sfx{sfx_output.suffix}"
            )
            create_and_inspect_sfx(
                binary,
                sfx_payload,
                legacy_output,
                target,
                binary,
                common,
                workspace,
                runner,
                "legacy sfx",
            )
            legacy_test = require_success_report(
                invoke_json(
                    legacy_output,
                    "legacy sfx runtime test",
                    ("--test", "--json"),
                    workspace,
                    runner,
                ),
                "legacy sfx runtime test",
            )
            require_runtime_test_report(
                legacy_test,
                len(expected_files),
                "legacy sfx runtime test",
            )

    return len(expected_files)


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Smoke the packaged Squallz CLI before release asset collection."
    )
    parser.add_argument("--project-root", default=".")
    parser.add_argument("--platform", required=True, choices=PLATFORMS)
    parser.add_argument("--profile", required=True, choices=PROFILES)
    parser.add_argument(
        "--binary",
        help="Override the platform package path; relative paths use project-root.",
    )
    args = parser.parse_args()

    try:
        project_root = Path(args.project_root).resolve()
        if args.binary is None:
            binary = release_binary_path(project_root, args.platform, args.profile)
        else:
            binary = Path(args.binary)
            if not binary.is_absolute():
                binary = project_root / binary
        host_template = release_sfx_template_path(
            project_root,
            args.platform,
            args.profile,
        )
        version = workspace_version(project_root / "Cargo.toml")
        with tempfile.TemporaryDirectory(prefix="squallz-release-smoke-") as tmp:
            workspace = Path(tmp)
            if args.platform in {"windows-x64", "linux-x64"}:
                require_packaged_desktop_runtime(
                    project_root,
                    args.platform,
                    args.profile,
                    host_template,
                    workspace,
                )
            count = run_smoke(
                binary,
                version,
                workspace,
                args.platform,
                host_template,
            )
    except SmokeError as error:
        print(f"release binary smoke failed: {error}", file=sys.stderr)
        return 1

    print(
        f"release binary smoke passed: {args.platform}/{args.profile}, "
        f"{count} files verified"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
