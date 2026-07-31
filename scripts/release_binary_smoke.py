#!/usr/bin/env python3

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import subprocess
import sys
import tempfile
import zlib
from collections.abc import Callable, Sequence
from pathlib import Path
from typing import Any


PLATFORMS = ("macos-arm64", "macos-x64", "windows-x64", "linux-x64")
PROFILES = ("release", "debug")
TOML_SECTION = re.compile(r"^\s*\[([^\]]+)\]\s*(?:#.*)?$")
TOML_VERSION = re.compile(
    r"""^\s*version\s*=\s*(?:"([^"]+)"|'([^']+)')\s*(?:#.*)?$"""
)


class SmokeError(RuntimeError):
    pass


Runner = Callable[..., subprocess.CompletedProcess[str]]


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
    binary: Path,
) -> Path:
    if platform in {"macos-arm64", "macos-x64"}:
        return project_root / "target" / profile / "bundle/macos/Squallz.app"
    if platform in {"windows-x64", "linux-x64"}:
        return binary
    raise SmokeError(f"unsupported release platform: {platform}")


def sfx_target(platform: str) -> str:
    if platform in {"macos-arm64", "macos-x64"}:
        return "macos"
    if platform == "windows-x64":
        return "windows"
    if platform == "linux-x64":
        return "linux"
    raise SmokeError(f"unsupported release platform: {platform}")


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
    if not resolved.is_file():
        raise SmokeError(f"{target} SFX template is not a regular file")
    if os.name != "nt" and not os.access(resolved, os.X_OK):
        raise SmokeError(f"{target} SFX template is not executable")
    return resolved


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
    source = create_fixture(workspace)
    expected_files = source_files(source)
    archive = workspace / "release-smoke.zip"
    destination = workspace / "extracted"
    sfx_payload = workspace / "release-smoke-sfx-payload.zip"
    sfx_output = sfx_output_path(workspace, target)
    sfx_destination = workspace / "sfx-extracted"
    common = ("--lang", "en-US", "--quiet", "--color", "never")

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

    create_report = require_success_report(
        invoke_json(
            binary,
            "sfx create",
            (
                *common,
                "sfx",
                "create",
                sfx_payload,
                "--output",
                sfx_output,
                "--target",
                target,
                "--stub",
                host_template,
                "--json",
            ),
            workspace,
            runner,
        ),
        "sfx create",
    )
    require_sfx_create_report(create_report, sfx_payload, sfx_output, target)

    inspect_report = require_success_report(
        invoke_json(
            binary,
            "sfx inspect",
            (*common, "sfx", "inspect", sfx_output, "--json"),
            workspace,
            runner,
        ),
        "sfx inspect",
    )
    require_sfx_inspect_report(inspect_report, create_report, sfx_output)

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
        if (
            not isinstance(sfx_test.get("entries_tested"), int)
            or sfx_test["entries_tested"] < len(expected_files)
            or sfx_test.get("problems") != []
        ):
            raise SmokeError("sfx runtime test did not prove payload integrity")

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
            binary,
        )
        version = workspace_version(project_root / "Cargo.toml")
        with tempfile.TemporaryDirectory(prefix="squallz-release-smoke-") as tmp:
            count = run_smoke(
                binary,
                version,
                Path(tmp),
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
