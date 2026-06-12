#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPORT="$ROOT/benches/ZIP64_LARGE_SMOKE.md"
WORK="$ROOT/target/squallz-zip64-large-smoke"
LOG="$WORK/cargo-test.log"
TIME_LOG="$WORK/time.log"
TMPROOT="${TMPDIR:-/tmp}"
MIN_FREE_KIB="${SQUALLZ_ZIP64_MIN_FREE_KIB:-12582912}" # 12 GiB.
CMD=(cargo test -p squallz-formats --test zip_roundtrip zip64_store_5gib_roundtrip -- --ignored --exact --nocapture)

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

print_command() {
  local first=1
  local arg
  for arg in "$@"; do
    if [[ "$first" -eq 1 ]]; then
      printf '%q' "$arg"
      first=0
    else
      printf ' %q' "$arg"
    fi
  done
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

available_kib() {
  df -k "$TMPROOT" | awk 'NR == 2 { print $4 }'
}

count_temp_dirs() {
  find "$TMPROOT" -maxdepth 1 -type d -name 'squallz-zip-test-zip64-*' 2>/dev/null | wc -l | tr -d ' '
}

cleanup_temp_dirs() {
  find "$TMPROOT" -maxdepth 1 -type d -name 'squallz-zip-test-zip64-*' -exec rm -rf {} + 2>/dev/null || true
}

peak_rss() {
  if [[ -f "$TIME_LOG" ]]; then
    awk '/maximum resident set size/ {
      if (index($0, ":") > 0) {
        sub(/^[^:]*:[[:space:]]*/, "");
        print;
      } else {
        print $1;
      }
    }' "$TIME_LOG" | tail -1
  fi
}

write_report() {
  local status="$1"
  local summary="${2:-}"
  cat > "$REPORT" <<EOF
# Squallz ZIP64 Large Smoke

Generated: $(date -u +"%Y-%m-%dT%H:%M:%SZ")

Status: $status

## Scope

This gate runs the ignored ZIP64 5 GiB Store-mode round-trip test explicitly.
It validates that Squallz can stream-write a ZIP64 entry larger than 4 GiB,
reopen it, read the entry back without materializing it in memory, and clean up
the generated temporary archive.

## Inputs

- Temp root: \`$TMPROOT\`
- Minimum free space: \`$MIN_FREE_KIB KiB\`
- Cargo log: \`$(relpath "$LOG")\`
- Time log: \`$(relpath "$TIME_LOG")\`

## Command

\`\`\`bash
$(print_command "${CMD[@]}")
\`\`\`

## Results

| Check | Status | Evidence |
| ---- | ---- | ---- |
$(printf '%s\n' "${rows[@]}")

## Resource Observations

- Duration seconds: \`${duration_seconds:-n/a}\`
- Peak RSS bytes: \`${peak_rss_bytes:-n/a}\`

## Failures

$(if [[ "${#failures[@]}" -eq 0 ]]; then echo "- None."; else printf -- '- %s\n' "${failures[@]}"; fi)

## Summary

$summary
EOF
}

blocked() {
  add_row "preflight" "blocked" "$1"
  write_report "blocked" "$1"
  echo "zip64_large_smoke: blocked: $*" >&2
  echo "report=$REPORT"
  exit 2
}

mkdir -p "$ROOT/benches" "$WORK"

if [[ ! -d "$TMPROOT" ]]; then
  blocked "temp root does not exist: $TMPROOT"
fi

free_kib="$(available_kib)"
if [[ -z "$free_kib" || "$free_kib" -lt "$MIN_FREE_KIB" ]]; then
  blocked "insufficient free space in $TMPROOT: ${free_kib:-unknown} KiB available, $MIN_FREE_KIB KiB required"
fi
add_row "disk preflight" "pass" "$TMPROOT has $free_kib KiB free; required $MIN_FREE_KIB KiB"

cleanup_temp_dirs
before_count="$(count_temp_dirs)"
if [[ "$before_count" == "0" ]]; then
  add_row "pre-run temp cleanup" "pass" "no stale squallz ZIP64 temp directories remain"
else
  add_row "pre-run temp cleanup" "fail" "$before_count stale squallz ZIP64 temp directories remain"
fi

start_seconds="$(date +%s)"
set +e
(
  cd "$ROOT"
  if [[ -x /usr/bin/time ]]; then
    /usr/bin/time -l "${CMD[@]}"
  else
    "${CMD[@]}"
  fi
) >"$LOG" 2>"$TIME_LOG"
test_status="$?"
set -e
end_seconds="$(date +%s)"
duration_seconds="$((end_seconds - start_seconds))"
peak_rss_bytes="$(peak_rss)"

if [[ "$test_status" -eq 0 ]]; then
  add_row "ignored ZIP64 5 GiB test" "pass" "\`$(print_command "${CMD[@]}")\` exited 0"
else
  add_row "ignored ZIP64 5 GiB test" "fail" "\`$(print_command "${CMD[@]}")\` exited $test_status; see \`$(relpath "$LOG")\`"
fi

if grep -Fq "test zip64_store_5gib_roundtrip ... ok" "$LOG"; then
  add_row "test result marker" "pass" "cargo log contains \`zip64_store_5gib_roundtrip ... ok\`"
else
  add_row "test result marker" "fail" "cargo log is missing \`zip64_store_5gib_roundtrip ... ok\`"
fi

cleanup_temp_dirs
after_count="$(count_temp_dirs)"
if [[ "$after_count" == "0" ]]; then
  add_row "post-run temp cleanup" "pass" "no squallz ZIP64 temp directories remain"
else
  add_row "post-run temp cleanup" "fail" "$after_count squallz ZIP64 temp directories remain after cleanup"
fi

if [[ "$test_status" -ne 0 || "${#failures[@]}" -gt 0 ]]; then
  write_report "fail" "Failed."
  echo "report=$REPORT"
  echo "log=$LOG"
  exit 1
fi

write_report "pass" "Passed."
echo "report=$REPORT"
echo "log=$LOG"
