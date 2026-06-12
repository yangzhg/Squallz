#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPORT="$ROOT/benches/WINDOWS_CREDENTIAL_MANAGER_SMOKE.md"
WORK="$ROOT/target/squallz-windows-credential-validation"
TEST_LOG="$WORK/test.log"
TEST_ERR_LOG="$WORK/test.stderr.log"

mkdir -p "$ROOT/benches" "$WORK"

write_blocked_report() {
  local message="$1"
  cat >"$REPORT" <<EOF
# Squallz Windows Credential Manager Smoke

Generated: $(date -u +"%Y-%m-%dT%H:%M:%SZ")

## Scope

This smoke check must run Squallz's real Windows \`SecretStore\` backend
against Windows Credential Manager. This host cannot provide that target
runtime.

## Inputs

- Test log: \`$TEST_LOG\`
- Test stderr log: \`$TEST_ERR_LOG\`

## Result

Status: blocked

$message

## Required Closure

Run \`scripts/windows_credential_manager_smoke.ps1\` on Windows and commit
\`benches/WINDOWS_CREDENTIAL_MANAGER_SMOKE.md\` only when it reports
\`Status: pass\`.
EOF
}

case "$(uname -s)" in
  MINGW*|MSYS*|CYGWIN*)
    powershell.exe -ExecutionPolicy Bypass -File "$ROOT/scripts/windows_credential_manager_smoke.ps1" "$@"
    ;;
  *)
    write_blocked_report "This smoke check only closes on Windows."
    echo "windows_credential_manager_smoke: blocked: this smoke check only closes on Windows" >&2
    echo "report=$REPORT"
    exit 2
    ;;
esac
