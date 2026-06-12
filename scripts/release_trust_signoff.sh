#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPORT="${SQUALLZ_RELEASE_TRUST_SIGNOFF_REPORT:-"$ROOT/benches/RELEASE_TRUST_SIGNOFF.md"}"
MANIFEST="${SQUALLZ_RELEASE_TRUST_EVIDENCE_MANIFEST:-}"

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

sha256_re = re.compile(r"^[0-9a-fA-F]{64}$")
url_re = re.compile(r"^https://[^/]+/.+", re.I)
forbidden_tokens = [
    "ad-hoc",
    "unsigned",
    "notarization skipped",
    "notary skipped",
    "staple skipped",
    "spctl skipped",
    "smartscreen not tested",
    "smartscreen skipped",
    "self-signed",
    "same-host",
    "same host",
    "same-machine",
    "compile-only",
    "compile only",
    "developer checkout",
    "repo checkout",
    "repository checkout",
    "headless cargo",
    "cargo test",
    "npm run tauri",
    "dev server",
    "dev-server",
    "vite",
    "cli-only",
    "localhost",
    "127.0.0.1",
    "target/debug",
    "target\\debug",
    "draft only",
    "local target directory hash only",
]

sections = {
    "macos": {
        "text": [
            "signed_app_path",
            "signed_dmg_path",
            "artifact_sha256",
            "developer_id_application_identity",
            "codesign_verify_log",
            "notarytool_submit_log",
            "stapler_validate_log",
            "spctl_assess_log",
            "quarantined_download_path",
        ],
        "bool": [
            "developer_id_signature",
            "codesign_strict_verified",
            "notarization_accepted",
            "stapler_validated",
            "gatekeeper_accepted",
            "quarantine_path_verified",
            "handoff_hash_matched",
        ],
        "artifacts": ["signed_app_path", "signed_dmg_path"],
        "logs": [
            "codesign_verify_log",
            "notarytool_submit_log",
            "stapler_validate_log",
            "spctl_assess_log",
        ],
    },
    "windows": {
        "text": [
            "signed_package_path",
            "artifact_sha256",
            "publisher",
            "certificate_subject",
            "signtool_verify_log",
            "smartscreen_review_log",
            "clean_machine_install_log",
        ],
        "bool": [
            "authenticode_verified",
            "timestamp_present",
            "smartscreen_outcome_documented",
            "clean_machine_signed_install",
            "handoff_hash_matched",
        ],
        "artifacts": ["signed_package_path"],
        "logs": [
            "signtool_verify_log",
            "smartscreen_review_log",
            "clean_machine_install_log",
        ],
    },
    "distribution": {
        "text": [
            "website_url",
            "download_url",
            "checksums_url",
            "release_notes_url",
            "privacy_policy_url",
            "license_url",
            "download_log",
            "checksum_verification_log",
            "release_handoff_packet",
        ],
        "bool": [
            "public_https_download",
            "checksum_manifest_published",
            "download_hash_matched",
            "release_notes_published",
            "trust_docs_reachable",
            "handoff_hash_matched",
        ],
        "url": [
            "website_url",
            "download_url",
            "checksums_url",
            "release_notes_url",
            "privacy_policy_url",
            "license_url",
        ],
        "logs": [
            "download_log",
            "checksum_verification_log",
            "release_handoff_packet",
        ],
    },
}


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


def file_status(raw: Any, base: Path) -> tuple[str, Optional[Path], list[str], list[str]]:
    if not valid_text(raw):
        return "missing", None, ["missing path"], []
    path = resolve_manifest_path(str(raw), base)
    if not path.is_file() or path.stat().st_size == 0:
        return "missing", path, [f"missing or empty: {path}"], []
    return "present", path, [], []


status = "blocked"
manifest_path: Optional[Path] = None
manifest: Optional[dict[str, Any]] = None
blocked: list[str] = []
failures: list[str] = []
evidence_rows: list[str] = []

if not manifest_arg:
    blocked.append(
        "SQUALLZ_RELEASE_TRUST_EVIDENCE_MANIFEST is not set; run this script with a release signing/notarization/distribution evidence manifest."
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
    base = manifest_path.parent
    for section_name, contract in sections.items():
        section = manifest.get(section_name)
        section_blocked: list[str] = []
        section_failures: list[str] = []
        section_passed = 0
        artifact_states: list[str] = []
        log_states: list[str] = []

        if not isinstance(section, dict):
            blocked.append(f"{section_name}: missing section object")
            evidence_rows.append(f"| {section_name} | blocked | 0 | missing section object |")
            continue

        for field in contract.get("text", []):
            value = section.get(field)
            if not valid_text(value):
                section_blocked.append(f"missing text field `{field}`")
            else:
                section_failures.extend(scan_forbidden_text(f"{section_name}.{field}", str(value)))
                section_passed += 1

        for field in contract.get("bool", []):
            if section.get(field) is True:
                section_passed += 1
            else:
                section_blocked.append(f"`{field}` must be true")

        for field in contract.get("url", []):
            value = str(section.get(field, "")).strip()
            if value and not url_re.match(value):
                section_failures.append(f"`{field}` must be a public https URL")

        artifact_hash = str(section.get("artifact_sha256", "")).strip()
        if "artifact_sha256" in contract.get("text", []):
            if artifact_hash and not sha256_re.match(artifact_hash):
                section_failures.append("artifact_sha256 must be a 64-character SHA-256 hex digest")
            for field in contract.get("artifacts", []):
                state, path, path_blocked, path_failures = file_status(section.get(field), base)
                section_blocked.extend(f"{field}: {item}" for item in path_blocked)
                section_failures.extend(f"{field}: {item}" for item in path_failures)
                if path is not None and path.is_file() and artifact_hash:
                    actual_hash = sha256_file(path)
                    if actual_hash.lower() == artifact_hash.lower():
                        state = "hash-pass"
                    else:
                        state = "hash-fail"
                        section_failures.append(
                            f"{field}: artifact hash mismatch expected={artifact_hash.lower()} actual={actual_hash}"
                        )
                artifact_states.append(f"{field}={state}")

        for field in contract.get("logs", []):
            state, path, path_blocked, path_failures = file_status(section.get(field), base)
            section_blocked.extend(f"{field}: {item}" for item in path_blocked)
            section_failures.extend(f"{field}: {item}" for item in path_failures)
            if path is not None and path.is_file():
                text = path.read_text(encoding="utf-8", errors="replace")[:200_000]
                section_failures.extend(scan_forbidden_text(f"{section_name}.{field}", text))
            log_states.append(f"{field}={state}")

        row_status = "pass"
        details: list[str] = []
        if section_failures:
            row_status = "fail"
            details.extend(section_failures)
            failures.extend(f"{section_name}: {item}" for item in section_failures)
        if section_blocked:
            if row_status != "fail":
                row_status = "blocked"
            details.extend(section_blocked)
            blocked.extend(f"{section_name}: {item}" for item in section_blocked)
        if not details:
            details.append("release trust evidence complete")
        if artifact_states:
            details.append("; ".join(artifact_states))
        if log_states:
            details.append("; ".join(log_states))
        evidence_rows.append(
            f"| {section_name} | {row_status} | {section_passed} | {md_cell('; '.join(details))} |"
        )

if failures:
    status = "fail"
elif blocked:
    status = "blocked"
else:
    status = "pass"

if not evidence_rows:
    evidence_rows.extend(
        [
            "| macos | blocked | 0 | no manifest supplied |",
            "| windows | blocked | 0 | no manifest supplied |",
            "| distribution | blocked | 0 | no manifest supplied |",
        ]
    )

manifest_display = str(manifest_path) if manifest_path is not None else "(not provided)"
now = datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")

report_path.write_text(
    "\n".join(
        [
            "# Release Trust Signoff",
            "",
            f"Generated: {now}",
            "",
            f"Status: {status}",
            "",
            "## Scope",
            "",
            "This report is the formal release blocker artifact for public release",
            "trust. It validates a manifest produced from the release signing,",
            "notarization, Windows Authenticode/SmartScreen, and public download",
            "distribution environments.",
            "",
            "It does not sign, notarize, staple, publish, upload, download, install,",
            "or claim release candidate readiness by itself. It also refuses",
            "ad-hoc signing, unsigned packages, skipped notarization, local dev",
            "paths, same-host or repo-checkout smoke runs, local-only hashes,",
            "self-signed certificates, and draft-only download pages as substitutes.",
            "",
            "## Manifest Format",
            "",
            "Set `SQUALLZ_RELEASE_TRUST_EVIDENCE_MANIFEST=/path/to/manifest.json`.",
            "The manifest must contain `macos`, `windows`, and `distribution`",
            "objects. Relative artifact and log paths resolve from the manifest",
            "directory.",
            "",
            "Required macOS evidence: Developer ID signed app and DMG, SHA-256,",
            "Developer ID Application identity, strict codesign log, notarytool",
            "submission log, stapler validate log, spctl assessment log, quarantined",
            "download path, and true Developer ID/notarization/stapler/Gatekeeper",
            "handoff-hash assertions.",
            "",
            "Required Windows evidence: signed package path, SHA-256, publisher,",
            "certificate subject, signtool verify log, SmartScreen review log,",
            "clean-machine signed install log, and true Authenticode/timestamp/",
            "SmartScreen/handoff-hash assertions.",
            "",
            "Required distribution evidence: public HTTPS website/download/checksum/",
            "release-notes/privacy/license URLs, download log, checksum verification",
            "log, release handoff packet, and true public-download/checksum/",
            "download-hash/trust-doc/handoff-hash assertions.",
            "",
            "## Summary",
            "",
            f"- Manifest: `{md_cell(manifest_display)}`",
            f"- Sections required: {len(sections)}",
            f"- Evidence rows: {len(evidence_rows)}",
            f"- Blocked conditions: {len(blocked)}",
            f"- Failures: {len(failures)}",
            "",
            "## Evidence Matrix",
            "",
            "| Section | Status | Passed field count | Detail |",
            "| ---- | ---- | ----: | ---- |",
            *evidence_rows,
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
            "- Ad-hoc signing smoke or unsigned DMG/package gates.",
            "- Local Gatekeeper diagnostics without Developer ID notarization and stapling.",
            "- Windows package hash without Authenticode verification and SmartScreen outcome.",
            "- Local target directory hashes without public HTTPS download verification.",
            "- Draft-only website/CDN pages, localhost URLs, or dev-server paths.",
            "- Same-host, repo-checkout, compile-only, target/debug, headless cargo, or CLI-only release smoke runs.",
            "",
        ]
    ),
    encoding="utf-8",
)

print(f"report={report_path}")
print(f"status={status}")
raise SystemExit(0 if status == "pass" else 2 if status == "blocked" else 1)
PY
