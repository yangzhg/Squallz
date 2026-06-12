#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPORT="${SQUALLZ_RELEASE_READINESS_REPORT:-"$ROOT/benches/RELEASE_READINESS_GATE.md"}"

if [[ "$REPORT" != /* ]]; then
  REPORT="$ROOT/$REPORT"
fi

evidence_rows=()
blocker_rows=()
local_failures=()
formal_blockers=()

report_status() {
  local path="$1"
  local line value boundary_re='($|[[:space:],.:;-])'
  line="$(grep -E -m1 '^(- )?(Status|Result):[[:space:]]*' "$path" 2>/dev/null || true)"
  line="${line#- }"

  case "$line" in
    Status:*) value="${line#Status:}" ;;
    Result:*) value="${line#Result:}" ;;
    *) printf 'unknown'; return ;;
  esac

  value="$(printf '%s' "$value" | sed 's/^[[:space:]]*//;s/[[:space:]]*$//' | tr '[:upper:]' '[:lower:]')"
  if [[ "$value" =~ ^(pass|passed)$boundary_re ]]; then
    printf 'pass'
  elif [[ "$value" =~ ^blocked$boundary_re ]]; then
    printf 'blocked'
  elif [[ "$value" =~ ^(fail|failed)$boundary_re ]]; then
    printf 'fail'
  else
    printf 'unknown'
  fi
}

add_evidence_row() {
  evidence_rows+=("| $1 | $2 | $3 |")
}

add_blocker_row() {
  blocker_rows+=("| $1 | $2 | $3 | $4 |")
  if [[ "$2" != "pass" ]]; then
    formal_blockers+=("$1: $3")
  fi
}

require_pass_report() {
  local item="$1" rel="$2" path status
  path="$ROOT/$rel"
  if [[ ! -s "$path" ]]; then
    add_evidence_row "$item" "missing" "\`$rel\` is missing or empty"
    local_failures+=("$item: missing $rel")
    return
  fi

  status="$(report_status "$path")"
  if [[ "$status" == "pass" ]]; then
    add_evidence_row "$item" "pass" "\`$rel\` reports pass"
  else
    add_evidence_row "$item" "$status" "\`$rel\` does not report pass"
    local_failures+=("$item: $rel status $status")
  fi
}

require_blocker_report() {
  local item="$1" rel="$2" resolution="$3" path status
  path="$ROOT/$rel"
  if [[ ! -s "$path" ]]; then
    add_blocker_row "$item" "missing" "\`$rel\` is missing or empty" "$resolution"
    return
  fi

  status="$(report_status "$path")"
  if [[ "$status" == "pass" ]]; then
    add_blocker_row "$item" "pass" "\`$rel\` reports pass" "Closed."
  else
    add_blocker_row "$item" "$status" "\`$rel\` currently reports $status" "$resolution"
  fi
}

mkdir -p "$(dirname "$REPORT")"

require_pass_report "macOS app open-file smoke" "benches/MACOS_APP_SMOKE.md"
require_pass_report "macOS native screenshot smoke" "benches/MACOS_NATIVE_SCREENSHOTS.md"
require_pass_report "macOS Keychain runtime smoke" "benches/MACOS_KEYCHAIN_SMOKE.md"
require_pass_report "ZIP reader fuzz campaign" "benches/ZIP_FUZZ_CAMPAIGN.md"
require_pass_report "ZIP64 5 GiB large-file smoke" "benches/ZIP64_LARGE_SMOKE.md"

require_blocker_report \
  "macOS Finder context-menu visible click" \
  "benches/MACOS_FINDER_CONTEXT_MENU_SMOKE.md" \
  "Resolve Finder/TCC/session blockers, then rerun the macOS Finder context-menu smoke from the active visible desktop."

require_blocker_report \
  "macOS desktop permission diagnostics" \
  "benches/MACOS_DESKTOP_PERMISSION_DIAGNOSTICS.md" \
  "Resolve blocked TCC/session rows, then rerun the desktop permission diagnostics."

require_blocker_report \
  "real RAR sample matrix" \
  "benches/RAR_REAL_MATRIX.md" \
  "Provide licensed plain/encrypted/solid/multi-volume/damaged RAR fixtures and rerun the real RAR matrix in strict mode."

require_blocker_report \
  "Windows Credential Manager runtime smoke" \
  "benches/WINDOWS_CREDENTIAL_MANAGER_SMOKE.md" \
  "Run \`scripts/windows_credential_manager_smoke.ps1\` on Windows and commit the report."

require_blocker_report \
  "Linux Secret Service runtime smoke" \
  "benches/LINUX_SECRET_SERVICE_SMOKE.md" \
  "Run \`scripts/linux_secret_service_smoke.sh\` in a real Linux desktop session and commit the report."

require_blocker_report \
  "Windows live UI and Explorer integration" \
  "benches/WINDOWS_LIVE_UI_EXPLORER_SIGNOFF.md" \
  "Run \`scripts/windows_live_ui_explorer_signoff.sh\` with \`SQUALLZ_WINDOWS_LIVE_UI_EVIDENCE_MANIFEST\` pointing at a target Windows packaged-app Explorer click manifest."

require_blocker_report \
  "Linux live UI and file-manager integration" \
  "benches/LINUX_LIVE_UI_FILE_MANAGER_SIGNOFF.md" \
  "Run \`scripts/linux_live_ui_file_manager_signoff.sh\` with \`SQUALLZ_LINUX_LIVE_UI_EVIDENCE_MANIFEST\` pointing at target GNOME/KDE packaged-app file-manager click manifests."

require_blocker_report \
  "clean-machine release install" \
  "benches/CLEAN_MACHINE_RELEASE_INSTALL.md" \
  "Provide macOS, Windows, and Linux clean-machine evidence in one manifest and rerun \`scripts/clean_machine_release_install.sh\` with \`SQUALLZ_CLEAN_MACHINE_EVIDENCE_MANIFEST\` set."

require_blocker_report \
  "signing, notarization, and distribution trust" \
  "benches/RELEASE_TRUST_SIGNOFF.md" \
  "Run \`scripts/release_trust_signoff.sh\` with \`SQUALLZ_RELEASE_TRUST_EVIDENCE_MANIFEST\` pointing at Developer ID notarization, Windows Authenticode/SmartScreen, and public website/CDN checksum evidence."

status="pass"
exit_code=0
if [[ "${#local_failures[@]}" -gt 0 ]]; then
  status="fail"
  exit_code=1
elif [[ "${#formal_blockers[@]}" -gt 0 ]]; then
  status="blocked"
  exit_code=2
fi

{
  printf '# Squallz Release Readiness Gate\n\n'
  printf 'Status: %s\n\n' "$status"
  printf '## Scope\n\n'
  printf 'This gate aggregates product-named release evidence only. It intentionally does not depend on numbered iteration reports or self-referential gate artifacts.\n\n'
  printf '## Summary\n\n'
  printf -- '- Local evidence regressions: %s\n' "${#local_failures[@]}"
  printf -- '- Formal release blockers: %s\n' "${#formal_blockers[@]}"
  printf -- '- Report path: `%s`\n' "${REPORT#$ROOT/}"
  printf -- '- Exit code: `%s`\n\n' "$exit_code"

  printf '## Local Evidence\n\n'
  printf '| Evidence | Status | Source |\n'
  printf '| ---- | ---- | ---- |\n'
  printf '%s\n' "${evidence_rows[@]}"

  printf '\n## Formal Release Blockers\n\n'
  printf '| Blocker | Status | Evidence | Resolution |\n'
  printf '| ---- | ---- | ---- | ---- |\n'
  printf '%s\n' "${blocker_rows[@]}"

  if [[ "${#local_failures[@]}" -gt 0 ]]; then
    printf '\n## Local Evidence Regressions\n\n'
    printf -- '- %s\n' "${local_failures[@]}"
  fi

  printf '\n## Notes\n\n'
  printf -- '- A blocked status is expected until all formal external signoff reports pass.\n'
  printf -- '- Iteration traces belong in `PROGRESS.md` and `ITERATION_LOG.md`, not in the release evidence graph.\n'
} >"$REPORT"

printf 'report=%s\n' "$REPORT"
printf 'status=%s\n' "$status"
exit "$exit_code"
