#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPORT="$ROOT/benches/LINUX_SECRET_SERVICE_SMOKE.md"
WORK="$ROOT/target/squallz-linux-secret-service-validation"
ARCHIVE="$WORK/secret-service-validation.7z"
TEST_LOG="$WORK/test.log"
SERVICE="com.squallz.archive-password"
ACCOUNT="archive:$ARCHIVE"
PASSWORD="squallz-secret-service-validation-secret"

mkdir -p "$ROOT/benches"

write_report() {
  local status="$1"
  local result="$2"
  local tool="${3:-}"
  cat > "$REPORT" <<EOF
# Squallz Linux Secret Service Smoke

Generated: $(date -u +"%Y-%m-%dT%H:%M:%SZ")

## Scope

This smoke check runs Squallz's real Linux \`SecretStore\` backend through
\`secret-tool\`, using a throwaway archive path and a non-user password.

## Inputs

- Archive account path: \`$ARCHIVE\`
- Secret Service attributes: \`service=$SERVICE account=$ACCOUNT\`
- secret-tool: \`${tool:-unavailable}\`
- Test log: \`$TEST_LOG\`

## Checks

- Existing test item for the archive account is deleted before the run.
- \`LinuxSecretServiceStore::set_archive_password\` writes a Secret Service item.
- \`has_archive_password\` reports the item as saved.
- \`get_archive_password\` reads the saved password back through Squallz's \`Password\` wrapper.
- \`delete_archive_password\` removes the item.
- A direct \`secret-tool lookup\` check confirms no test item remains.

## Result

Status: $status

$result
EOF
}

blocked() {
  write_report "blocked" "$1" "${2:-}"
  echo "linux_secret_service_smoke: blocked: $1" >&2
  echo "report=$REPORT"
  exit 2
}

fail() {
  write_report "failed" "$1" "${2:-}"
  echo "linux_secret_service_smoke: $1" >&2
  exit 1
}

if [[ "$(uname -s)" != "Linux" ]]; then
  blocked "this smoke check only runs on Linux"
fi

SECRET_TOOL="$(command -v secret-tool || true)"
if [[ -z "$SECRET_TOOL" ]]; then
  blocked "missing secret-tool; install libsecret-tools or the distro equivalent"
fi

mkdir -p "$WORK"
printf "secret service smoke placeholder\n" > "$ARCHIVE"

cleanup() {
  "$SECRET_TOOL" clear service "$SERVICE" account "$ACCOUNT" >/dev/null 2>&1 || true
}
trap cleanup EXIT
cleanup

set +e
(
  cd "$ROOT"
  SQUALLZ_SECRET_SERVICE_VALIDATION=1 \
  SQUALLZ_SECRET_SERVICE_VALIDATION_ARCHIVE="$ARCHIVE" \
  SQUALLZ_SECRET_SERVICE_VALIDATION_PASSWORD="$PASSWORD" \
    cargo test -p squallz-gui secrets::tests::linux_secret_service_write_read_delete_validation -- --ignored --exact --nocapture
) | tee "$TEST_LOG"
test_status="${PIPESTATUS[0]}"
set -e

if [[ "$test_status" -ne 0 ]]; then
  fail "Linux Secret Service ignored test failed; see $TEST_LOG" "$SECRET_TOOL"
fi

if "$SECRET_TOOL" lookup service "$SERVICE" account "$ACCOUNT" >/dev/null 2>&1; then
  fail "test Secret Service item was not deleted" "$SECRET_TOOL"
fi

write_report "pass" "Passed." "$SECRET_TOOL"

echo "report=$REPORT"
echo "log=$TEST_LOG"
