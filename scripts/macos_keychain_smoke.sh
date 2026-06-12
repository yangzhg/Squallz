#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPORT="$ROOT/benches/MACOS_KEYCHAIN_SMOKE.md"
WORK="$ROOT/target/squallz-keychain-validation"
ARCHIVE="$WORK/keychain smoke #1.7z"
TEST_LOG="$WORK/test.log"
SERVICE="com.squallz.archive-password"
ACCOUNT="archive:$ARCHIVE"
PASSWORD="squallz-keychain-validation-secret"
CARGO_TEST=(cargo test -p squallz-gui secrets::tests::macos_keychain_write_read_delete_validation -- --ignored --exact --nocapture)

rows=()
failures=()

relpath() {
  local path="$1"
  if [[ "$path" == "$ROOT/"* ]]; then
    printf '%s' "${path#$ROOT/}"
  else
    printf '%s' "$path"
  fi
}

add_row() {
  local check="$1"
  local status="$2"
  local evidence="$3"
  rows+=("| $check | $status | $evidence |")
  if [[ "$status" != "pass" ]]; then
    failures+=("$check: $evidence")
  fi
}

keychain_item_exists() {
  /usr/bin/security find-generic-password -s "$SERVICE" -a "$ACCOUNT" -w >/dev/null 2>&1
}

password_marker_in_file() {
  local file="$1"
  [[ -n "$PASSWORD" ]] && [[ -f "$file" ]] && grep -Fq -- "$PASSWORD" "$file"
}

mkdir -p "$ROOT/benches"

write_report() {
  local status="$1"
  local result="${2:-}"
  cat > "$REPORT" <<EOF
# Squallz macOS Keychain Smoke

Generated: $(date -u +"%Y-%m-%dT%H:%M:%SZ")

## Scope

This smoke check runs Squallz's real macOS \`SecretStore\` backend against the
user login Keychain, using a throwaway archive path and a non-user password.

Status: $status

## Inputs

- Archive path: \`$(relpath "$ARCHIVE")\`
- Archive account: \`$ACCOUNT\`
- Keychain service: \`$SERVICE\`
- Test password: \`<redacted>\`
- Test log: \`$(relpath "$TEST_LOG")\`

## Command

\`\`\`bash
SQUALLZ_KEYCHAIN_VALIDATION=1 SQUALLZ_KEYCHAIN_VALIDATION_ARCHIVE="$(relpath "$ARCHIVE")" SQUALLZ_KEYCHAIN_VALIDATION_PASSWORD="<redacted>" ${CARGO_TEST[*]}
\`\`\`

## Checks

- Existing test item for the archive account is deleted before the run.
- \`MacOsKeychainSecretStore::set_archive_password\` writes a generic password.
- \`has_archive_password\` reports the item as saved.
- \`get_archive_password\` reads the saved password back through Squallz's \`Password\` wrapper.
- \`delete_archive_password\` removes the item.
- A direct \`security find-generic-password\` check confirms no test item remains.
- The generated report and captured cargo log do not contain the configured plaintext password marker.

## Results

| Check | Status | Evidence |
| ---- | ---- | ---- |
$(printf '%s\n' "${rows[@]}")

## Failures

$(if [[ "${#failures[@]}" -eq 0 ]]; then echo "- None."; else printf -- '- %s\n' "${failures[@]}"; fi)

## Summary

$result
EOF
}

blocked() {
  add_row "platform preflight" "blocked" "$1"
  write_report "blocked" "$1"
  echo "macos_keychain_smoke: blocked: $*" >&2
  echo "report=$REPORT"
  exit 2
}

fail() {
  write_report "fail" "$1"
  echo "macos_keychain_smoke: $*" >&2
  echo "report=$REPORT"
  exit 1
}

if [[ "$(uname -s)" != "Darwin" ]]; then
  blocked "this smoke check only runs on macOS"
fi
if [[ ! -x /usr/bin/security ]]; then
  blocked "missing /usr/bin/security"
fi

mkdir -p "$WORK"
printf "keychain smoke placeholder\n" > "$ARCHIVE"

cleanup() {
  /usr/bin/security delete-generic-password -s "$SERVICE" -a "$ACCOUNT" >/dev/null 2>&1 || true
}
trap cleanup EXIT
cleanup
if keychain_item_exists; then
  add_row "pre-run cleanup" "fail" "existing test Keychain item still exists after cleanup"
else
  add_row "pre-run cleanup" "pass" "no test item for \`$ACCOUNT\` remains before the run"
fi

set +e
(
  cd "$ROOT"
  SQUALLZ_KEYCHAIN_VALIDATION=1 \
  SQUALLZ_KEYCHAIN_VALIDATION_ARCHIVE="$ARCHIVE" \
  SQUALLZ_KEYCHAIN_VALIDATION_PASSWORD="$PASSWORD" \
    "${CARGO_TEST[@]}"
) >"$TEST_LOG" 2>&1
test_status="$?"
set -e

if [[ "$test_status" -eq 0 ]]; then
  add_row "ignored Rust validation test" "pass" "\`${CARGO_TEST[*]}\` exited 0"
else
  add_row "ignored Rust validation test" "fail" "\`${CARGO_TEST[*]}\` exited $test_status; see \`$(relpath "$TEST_LOG")\`"
fi

if keychain_item_exists; then
  add_row "post-test direct residue check" "fail" "test Keychain item still exists before final cleanup"
else
  add_row "post-test direct residue check" "pass" "\`security find-generic-password\` cannot read the test item after the Rust test"
fi

cleanup
if keychain_item_exists; then
  add_row "post-cleanup residue check" "fail" "test Keychain item still exists after final cleanup"
else
  add_row "post-cleanup residue check" "pass" "final cleanup left no test Keychain item behind"
fi

write_report "pass" "Pending plaintext-marker scan."
if password_marker_in_file "$TEST_LOG" || password_marker_in_file "$REPORT"; then
  add_row "plaintext password marker scan" "fail" "configured password marker appeared in the captured log or report"
else
  add_row "plaintext password marker scan" "pass" "configured password marker absent from \`$(relpath "$TEST_LOG")\` and \`$(relpath "$REPORT")\`"
fi

if [[ "$test_status" -ne 0 || "${#failures[@]}" -gt 0 ]]; then
  write_report "fail" "Failed."
  echo "report=$REPORT"
  echo "log=$TEST_LOG"
  exit 1
fi

write_report "pass" "Passed."

echo "report=$REPORT"
echo "log=$TEST_LOG"
