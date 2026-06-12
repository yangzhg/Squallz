#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPORT="${SQUALLZ_CLEAN_MACHINE_RELEASE_INSTALL_REPORT:-"$ROOT/benches/CLEAN_MACHINE_RELEASE_INSTALL.md"}"
MANIFEST="${SQUALLZ_CLEAN_MACHINE_EVIDENCE_MANIFEST:-}"

if [[ "$REPORT" != /* ]]; then
  REPORT="$ROOT/$REPORT"
fi

mkdir -p "$(dirname "$REPORT")"

python3 - "$ROOT" "$REPORT" "$MANIFEST" <<'PY'
import hashlib
import json
import re
import sys
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Optional

root = Path(sys.argv[1])
report_path = Path(sys.argv[2])
manifest_arg = sys.argv[3].strip()

platforms = ["macos", "windows", "linux"]
required_text_fields = [
    "artifact_path",
    "artifact_sha256",
    "install_log",
    "open_file_log",
    "screenshot",
    "package_kind",
    "machine_id",
]
required_bool_fields = ["clean_profile", "launched_from_package", "archive_opened"]
sha256_re = re.compile(r"^[0-9a-fA-F]{64}$")
forbidden_tokens = [
    "same-host",
    "same host",
    "isolated home",
    "clean-home",
    "clean home only",
    "dmg install smoke alone",
    "package contents inspection",
    "compile-only",
    "compile only",
    "developer checkout",
    "repo checkout",
    "repository checkout",
    "target/debug",
    "target\\debug",
    "cargo run",
    "npm run tauri",
    "dev-server",
    "dev server",
    "localhost",
    "127.0.0.1",
]


def md_cell(value: object) -> str:
    return str(value).replace("|", "\\|").replace("\n", " ")


def resolve_manifest_path(raw: str, base: Path) -> Path:
    path = Path(raw).expanduser()
    if path.is_absolute():
        return path
    return (base / path).resolve()


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def valid_text(value: Any) -> bool:
    return isinstance(value, str) and bool(value.strip())


def scan_forbidden_text(label: str, text: str) -> list[str]:
    lower = text.lower()
    return [f"{label} contains forbidden substitute token `{token}`" for token in forbidden_tokens if token in lower]


status = "blocked"
manifest_path: Optional[Path] = None
manifest: Optional[dict[str, Any]] = None
blocked: list[str] = []
failures: list[str] = []
rows: list[str] = []

if not manifest_arg:
    blocked.append(
        "SQUALLZ_CLEAN_MACHINE_EVIDENCE_MANIFEST is not set; run this script with a three-platform evidence manifest from clean macOS, Windows, and Linux machines."
    )
else:
    manifest_path = Path(manifest_arg).expanduser()
    if not manifest_path.is_absolute():
        manifest_path = (root / manifest_path).resolve()
    if not manifest_path.is_file():
        blocked.append(f"manifest file is missing: {manifest_path}")
    else:
        try:
            loaded = json.loads(manifest_path.read_text(encoding="utf-8"))
        except json.JSONDecodeError as error:
            failures.append(f"manifest JSON is invalid: {error}")
        else:
            if isinstance(loaded, dict):
                manifest = loaded
                failures.extend(scan_forbidden_text("manifest", json.dumps(loaded, ensure_ascii=False)))
            else:
                failures.append("manifest root must be a JSON object keyed by platform")

if manifest is not None and manifest_path is not None:
    manifest_base = manifest_path.parent
    for platform in platforms:
        entry = manifest.get(platform)
        platform_blocked: list[str] = []
        platform_failures: list[str] = []
        artifact_status = "missing"
        screenshot_status = "missing"
        log_status = "missing"

        if not isinstance(entry, dict):
            blocked.append(f"{platform}: missing platform object")
            rows.append(
                f"| {platform} | blocked | missing platform object | - | - | - | - |"
            )
            continue

        for field in required_text_fields:
            if not valid_text(entry.get(field)):
                platform_blocked.append(f"missing text field `{field}`")
            else:
                platform_failures.extend(scan_forbidden_text(f"{platform}.{field}", str(entry[field])))
        for field in required_bool_fields:
            if entry.get(field) is not True:
                platform_blocked.append(f"`{field}` must be true")

        hash_value = str(entry.get("artifact_sha256", "")).strip()
        if hash_value and not sha256_re.match(hash_value):
            platform_failures.append("artifact_sha256 must be a 64-character SHA-256 hex digest")

        artifact_path: Optional[Path] = None
        if valid_text(entry.get("artifact_path")):
            artifact_path = resolve_manifest_path(str(entry["artifact_path"]), manifest_base)
            if artifact_path.is_file():
                actual_hash = sha256_file(artifact_path)
                if hash_value and actual_hash.lower() == hash_value.lower():
                    artifact_status = "hash-pass"
                elif hash_value:
                    platform_failures.append(
                        f"artifact hash mismatch expected={hash_value.lower()} actual={actual_hash}"
                    )
                    artifact_status = "hash-fail"
                else:
                    artifact_status = "hash-missing"
            else:
                platform_blocked.append(f"artifact file missing: {artifact_path}")

        install_log = entry.get("install_log")
        open_file_log = entry.get("open_file_log")
        log_paths: list[Path] = []
        for label, raw in [("install_log", install_log), ("open_file_log", open_file_log)]:
            if valid_text(raw):
                path = resolve_manifest_path(str(raw), manifest_base)
                log_paths.append(path)
                if not path.is_file() or path.stat().st_size == 0:
                    platform_blocked.append(f"{label} missing or empty: {path}")
                else:
                    text = path.read_text(encoding="utf-8", errors="replace")[:200_000]
                    platform_failures.extend(scan_forbidden_text(f"{platform}.{label}", text))
        if log_paths and all(path.is_file() and path.stat().st_size > 0 for path in log_paths):
            log_status = "present"

        if valid_text(entry.get("screenshot")):
            screenshot = resolve_manifest_path(str(entry["screenshot"]), manifest_base)
            if screenshot.is_file() and screenshot.stat().st_size > 0:
                if screenshot.suffix.lower() not in {".png", ".jpg", ".jpeg"}:
                    platform_failures.append(f"screenshot must be PNG/JPEG: {screenshot}")
                    screenshot_status = "bad-extension"
                else:
                    screenshot_status = "present"
            else:
                platform_blocked.append(f"screenshot missing or empty: {screenshot}")

        row_status = "pass"
        detail_parts: list[str] = []
        if platform_failures:
            failures.extend(f"{platform}: {item}" for item in platform_failures)
            row_status = "fail"
            detail_parts.extend(platform_failures)
        if platform_blocked:
            blocked.extend(f"{platform}: {item}" for item in platform_blocked)
            if row_status != "fail":
                row_status = "blocked"
            detail_parts.extend(platform_blocked)
        if not detail_parts:
            detail_parts.append("clean-machine evidence complete")

        rows.append(
            "| "
            + " | ".join(
                [
                    platform,
                    row_status,
                    md_cell(entry.get("machine_id", "")),
                    md_cell(entry.get("package_kind", "")),
                    artifact_status,
                    log_status,
                    screenshot_status,
                    md_cell("; ".join(detail_parts)),
                ]
            )
            + " |"
        )

if failures:
    status = "fail"
elif blocked:
    status = "blocked"
else:
    status = "pass"

manifest_display = str(manifest_path) if manifest_path is not None else "(not provided)"

report_path.write_text(
    "\n".join(
        [
            "# Clean-Machine Release Install Signoff",
            "",
            f"Generated: {datetime.now(timezone.utc).strftime('%Y-%m-%dT%H:%M:%SZ')}",
            "",
            f"Status: {status}",
            "",
            "## Scope",
            "",
            "This report is the release blocker artifact for clean-machine install",
            "signoff. It validates a JSON evidence manifest produced from fresh",
            "macOS, Windows, and Linux machines or VMs. Each platform must provide",
            "the packaged artifact, SHA-256, install log, open-file log, visible",
            "screenshot, clean-profile assertion, packaged-launch assertion, and",
            "archive-open assertion.",
            "",
            "It does not install Squallz, create a VM, launch the app, sign,",
            "notarize, upload, distribute, or accept same-host DMG / clean-HOME",
            "surrogates as release signoff.",
            "",
            "## Manifest Format",
            "",
            "Set `SQUALLZ_CLEAN_MACHINE_EVIDENCE_MANIFEST=/path/to/manifest.json`.",
            "The manifest must be a JSON object with `macos`, `windows`, and",
            "`linux` objects. Relative paths are resolved from the manifest",
            "directory.",
            "",
            "Required fields per platform: `artifact_path`, `artifact_sha256`,",
            "`install_log`, `open_file_log`, `screenshot`, `package_kind`,",
            "`machine_id`, `clean_profile: true`, `launched_from_package: true`,",
            "and `archive_opened: true`.",
            "",
            "## Summary",
            "",
            f"- Manifest: `{md_cell(manifest_display)}`",
            f"- Platforms required: {len(platforms)}",
            f"- Platform rows: {len(rows)}",
            f"- Blocked conditions: {len(blocked)}",
            f"- Failures: {len(failures)}",
            "",
            "## Platform Evidence",
            "",
            "| Platform | Status | Machine ID | Package | Artifact | Logs | Screenshot | Detail |",
            "| ---- | ---- | ---- | ---- | ---- | ---- | ---- | ---- |",
            *(rows if rows else ["| macos/windows/linux | blocked | - | - | missing | missing | missing | no manifest supplied |"]),
            "",
            "## Blocked Conditions",
            "",
            "- None." if not blocked else "\n".join(f"- {item}" for item in blocked),
            "",
            "## Failures",
            "",
            "- None." if not failures else "\n".join(f"- {item}" for item in failures),
            "",
            "## Forbidden Substitutes",
            "",
            "- Same-host isolated HOME smoke.",
            "- macOS DMG install smoke alone.",
            "- Package contents inspection without clean-machine launch.",
            "- Compile-only Windows/Linux evidence.",
            "- Screenshots or logs that do not reference the validated package hash.",
            "",
        ]
    ),
    encoding="utf-8",
)

print(f"report={report_path}")
print(f"status={status}")
raise SystemExit(0 if status == "pass" else 2 if status == "blocked" else 1)
PY
