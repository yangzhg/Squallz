#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPORT="${SQUALLZ_WINDOWS_LIVE_UI_EXPLORER_REPORT:-"$ROOT/benches/WINDOWS_LIVE_UI_EXPLORER_SIGNOFF.md"}"
MANIFEST="${SQUALLZ_WINDOWS_LIVE_UI_EVIDENCE_MANIFEST:-}"

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

required_text_fields = [
    "package_artifact_path",
    "package_sha256",
    "package_kind",
    "machine_id",
    "windows_version",
    "explorer_version",
    "webview2_version",
    "install_log",
    "app_launch_log",
    "open_file_log",
    "explorer_actions_log",
]

required_bool_fields = [
    "target_os_windows",
    "packaged_launch",
    "archive_opened",
    "explorer_menu_archive_visible",
    "explorer_menu_folder_visible",
    "explorer_menu_multi_selection_visible",
    "extract_here_succeeded",
    "extract_to_folder_succeeded",
    "test_archive_succeeded",
    "compress_succeeded",
    "outputs_verified",
    "actions_invoked_packaged_binary",
    "visible_feedback",
    "no_dev_server",
]

required_screenshot_fields = [
    "app_launch_screenshot",
    "archive_context_menu_screenshot",
    "folder_context_menu_screenshot",
    "multi_selection_context_menu_screenshot",
    "task_window_screenshot",
]

required_action_names = [
    "extract_here",
    "extract_to_folder",
    "test_archive",
    "compress",
]

sha256_re = re.compile(r"^[0-9a-fA-F]{64}$")
forbidden_tokens = [
    "same-host",
    "same host",
    "isolated home",
    "clean-home",
    "clean home only",
    "package contents inspection",
    "static registry inspection",
    "static script inspection",
    "compile-only",
    "compile only",
    "cross-target compile",
    "headless cargo test",
    "headless cargo tests",
    "unit tests",
    "cli-only",
    "cli only",
    "developer checkout",
    "repo checkout",
    "repository checkout",
    "127.0.0.1",
    "localhost",
    "vite",
    "npm run dev",
    "npm run tauri",
    "dev-server",
    "dev server",
    "cargo run",
    "target/debug",
    "target\\debug",
    "/target/debug",
    "\\target\\debug",
    "/workspace/Squallz",
    "\\workspace\\Squallz",
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


def file_status(raw: Any, base: Path, *, screenshot: bool = False) -> tuple[str, Optional[Path], list[str], list[str]]:
    blocked: list[str] = []
    failures: list[str] = []
    if not valid_text(raw):
        return "missing", None, ["missing path"], []
    path = resolve_manifest_path(str(raw), base)
    if not path.is_file() or path.stat().st_size == 0:
        return "missing", path, [f"missing or empty: {path}"], []
    if screenshot and path.suffix.lower() not in {".png", ".jpg", ".jpeg"}:
        failures.append(f"screenshot must be PNG/JPEG: {path}")
        return "bad-extension", path, [], failures
    return "present", path, [], []


def scan_forbidden_text(label: str, text: str) -> list[str]:
    lower = text.lower()
    found = [token for token in forbidden_tokens if token.lower() in lower]
    return [f"{label} contains forbidden substitute token `{token}`" for token in found]


status = "blocked"
manifest_path: Optional[Path] = None
manifest: Optional[dict[str, Any]] = None
blocked: list[str] = []
failures: list[str] = []
evidence_rows: list[str] = []
action_rows: list[str] = []

if not manifest_arg:
    blocked.append(
        "SQUALLZ_WINDOWS_LIVE_UI_EVIDENCE_MANIFEST is not set; run this script with a manifest produced by a target Windows Explorer click signoff."
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
                failures.append("manifest root must be a JSON object")

if manifest is not None and manifest_path is not None:
    manifest_base = manifest_path.parent

    for field in required_text_fields:
        if not valid_text(manifest.get(field)):
            blocked.append(f"missing text field `{field}`")
        elif field != "package_sha256":
            failures.extend(scan_forbidden_text(field, str(manifest[field])))

    for field in required_bool_fields:
        value = manifest.get(field)
        if value is not True:
            if field in {"target_os_windows", "actions_invoked_packaged_binary", "no_dev_server"} and value is False:
                failures.append(f"`{field}` is false")
            else:
                blocked.append(f"`{field}` must be true")

    package_hash = str(manifest.get("package_sha256", "")).strip()
    if package_hash and not sha256_re.match(package_hash):
        failures.append("package_sha256 must be a 64-character SHA-256 hex digest")

    package_status, package_path, package_blocked, package_failures = file_status(
        manifest.get("package_artifact_path"),
        manifest_base,
    )
    blocked.extend(f"package artifact: {item}" for item in package_blocked)
    failures.extend(f"package artifact: {item}" for item in package_failures)
    if package_path is not None and package_path.is_file() and package_hash:
        actual_hash = sha256_file(package_path)
        if actual_hash.lower() == package_hash.lower():
            package_status = "hash-pass"
        else:
            package_status = "hash-fail"
            failures.append(
                f"package hash mismatch expected={package_hash.lower()} actual={actual_hash}"
            )

    evidence_rows.append(
        f"| package artifact | {package_status} | `{md_cell(package_path or manifest.get('package_artifact_path', ''))}` |"
    )

    for field in ["install_log", "app_launch_log", "open_file_log", "explorer_actions_log"]:
        file_state, path, path_blocked, path_failures = file_status(manifest.get(field), manifest_base)
        blocked.extend(f"{field}: {item}" for item in path_blocked)
        failures.extend(f"{field}: {item}" for item in path_failures)
        if path is not None and path.is_file():
            text = path.read_text(encoding="utf-8", errors="replace")[:200_000]
            failures.extend(scan_forbidden_text(field, text))
        evidence_rows.append(f"| {field} | {file_state} | `{md_cell(path or manifest.get(field, ''))}` |")

    for field in required_screenshot_fields:
        if valid_text(manifest.get(field)):
            failures.extend(scan_forbidden_text(field, str(manifest[field])))
        file_state, path, path_blocked, path_failures = file_status(
            manifest.get(field),
            manifest_base,
            screenshot=True,
        )
        blocked.extend(f"{field}: {item}" for item in path_blocked)
        failures.extend(f"{field}: {item}" for item in path_failures)
        evidence_rows.append(f"| {field} | {file_state} | `{md_cell(path or manifest.get(field, ''))}` |")

    actions = manifest.get("actions")
    if not isinstance(actions, dict):
        blocked.append("actions must be an object with extract_here, extract_to_folder, test_archive, and compress rows")
    else:
        for action_name in required_action_names:
            action = actions.get(action_name)
            action_blocked: list[str] = []
            action_failures: list[str] = []
            if not isinstance(action, dict):
                action_blocked.append("missing action object")
                action_rows.append(f"| {action_name} | blocked | missing action object |")
                blocked.extend(f"{action_name}: {item}" for item in action_blocked)
                continue
            for field in ["menu_visible", "invoked", "packaged_binary", "visible_feedback", "output_verified"]:
                if action.get(field) is not True:
                    action_blocked.append(f"`{field}` must be true")
            for field in ["log", "screenshot"]:
                if valid_text(action.get(field)):
                    action_failures.extend(scan_forbidden_text(f"{action_name}.{field}", str(action[field])))
                file_state, path, path_blocked, path_failures = file_status(
                    action.get(field),
                    manifest_base,
                    screenshot=(field == "screenshot"),
                )
                action_blocked.extend(f"{field}: {item}" for item in path_blocked)
                action_failures.extend(f"{field}: {item}" for item in path_failures)
                if path is not None and path.is_file() and field == "log":
                    text = path.read_text(encoding="utf-8", errors="replace")[:200_000]
                    action_failures.extend(scan_forbidden_text(f"{action_name}.{field}", text))
            row_status = "pass"
            details: list[str] = ["action evidence complete"]
            if action_failures:
                row_status = "fail"
                details = action_failures
                failures.extend(f"{action_name}: {item}" for item in action_failures)
            if action_blocked:
                if row_status != "fail":
                    row_status = "blocked"
                    details = action_blocked
                else:
                    details.extend(action_blocked)
                blocked.extend(f"{action_name}: {item}" for item in action_blocked)
            action_rows.append(f"| {action_name} | {row_status} | {md_cell('; '.join(details))} |")

if failures:
    status = "fail"
elif blocked:
    status = "blocked"
else:
    status = "pass"

manifest_display = str(manifest_path) if manifest_path is not None else "(not provided)"
now = datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")

if not evidence_rows:
    evidence_rows.append("| package/logs/screenshots | blocked | no manifest supplied |")
if not action_rows:
    action_rows.append("| extract_here/extract_to_folder/test_archive/compress | blocked | no manifest supplied |")

report_path.write_text(
    "\n".join(
        [
            "# Windows Live UI and Explorer Integration Signoff",
            "",
            f"Generated: {now}",
            "",
            f"Status: {status}",
            "",
            "## Scope",
            "",
            "This report is the formal release blocker artifact for Windows live UI",
            "and Explorer context-menu integration. It validates evidence from a",
            "target Windows desktop or VM using the packaged Squallz artifact.",
            "",
            "It does not accept dev-server live-clicks, cross-target compile,",
            "macOS package smokes, static registry inspection, or CLI-only archive",
            "operations as a substitute for visible Explorer clicks.",
            "",
            "## Manifest Format",
            "",
            "Set `SQUALLZ_WINDOWS_LIVE_UI_EVIDENCE_MANIFEST=/path/to/manifest.json`.",
            "Relative paths are resolved from the manifest directory.",
            "",
            "Required top-level fields: `package_artifact_path`, `package_sha256`,",
            "`package_kind`, `machine_id`, `windows_version`, `explorer_version`,",
            "`webview2_version`, `install_log`, `app_launch_log`, `open_file_log`,",
            "`explorer_actions_log`, five screenshot paths, and the required boolean",
            "assertions listed in this script.",
            "",
            "The `actions` object must contain `extract_here`, `extract_to_folder`,",
            "`test_archive`, and `compress`; each action needs `menu_visible`,",
            "`invoked`, `packaged_binary`, `visible_feedback`, `output_verified`,",
            "`log`, and `screenshot`.",
            "",
            "## Summary",
            "",
            f"- Manifest: `{md_cell(manifest_display)}`",
            f"- Required actions: {len(required_action_names)}",
            f"- Evidence rows: {len(evidence_rows)}",
            f"- Action rows: {len(action_rows)}",
            f"- Blocked conditions: {len(blocked)}",
            f"- Failures: {len(failures)}",
            "",
            "## Target Environment",
            "",
            f"- Machine ID: `{md_cell(manifest.get('machine_id', '-') if manifest else '-')}`",
            f"- Windows version: `{md_cell(manifest.get('windows_version', '-') if manifest else '-')}`",
            f"- Explorer version: `{md_cell(manifest.get('explorer_version', '-') if manifest else '-')}`",
            f"- WebView2 version: `{md_cell(manifest.get('webview2_version', '-') if manifest else '-')}`",
            f"- Package kind: `{md_cell(manifest.get('package_kind', '-') if manifest else '-')}`",
            "",
            "## Evidence Files",
            "",
            "| Evidence | Status | Path |",
            "| ---- | ---- | ---- |",
            *evidence_rows,
            "",
            "## Explorer Action Evidence",
            "",
            "| Action | Status | Detail |",
            "| ---- | ---- | ---- |",
            *action_rows,
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
            "- Frontend dev-server live-click evidence.",
            "- Same-host, clean-HOME, compile-only, developer checkout, repo checkout, debug binary, localhost, or npm-run evidence.",
            "- Cross-target compile or unit tests.",
            "- macOS package smoke, DMG install smoke, or package contents inspection.",
            "- Registry/script/service-file inspection without visible Explorer clicks.",
            "- CLI-only archive operations.",
            "- Screenshots or logs not tied to the package SHA-256 under test.",
            "",
        ]
    ),
    encoding="utf-8",
)

print(f"report={report_path}")
print(f"status={status}")
raise SystemExit(0 if status == "pass" else 2 if status == "blocked" else 1)
PY
