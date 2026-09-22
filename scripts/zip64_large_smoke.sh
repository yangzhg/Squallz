#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK_ROOT="$ROOT/target/squallz-zip64-large-smoke"
mkdir -p "$WORK_ROOT"
WORK="$(mktemp -d "$WORK_ROOT/run.XXXXXX")"
REPORT="$WORK/report.md"
LOG="$WORK/cargo-test.log"
TIME_LOG="$WORK/time.log"
TMPROOT="${TMPDIR:-/tmp}"
RUN_TMP=""
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
  df -Pk "$TMPROOT" | awk 'NR == 2 { print $4 }'
}

cleanup_temp_dir() {
  if [[ -n "$RUN_TMP" ]]; then
    rm -rf -- "$RUN_TMP" || return 1
    RUN_TMP=""
  fi
}

peak_rss() {
  if [[ -f "$TIME_LOG" ]]; then
    awk '
      /maximum resident set size/ {
        print $1;
        exit;
      }
      /Maximum resident set size \(kbytes\):/ {
        printf "%.0f\n", $NF * 1024;
        exit;
      }
    ' "$TIME_LOG"
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

- Temporary archive data is isolated per run and removed on exit.
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
- Peak RSS bytes (Cargo invocation, including compilation): \`${peak_rss_bytes:-n/a}\`

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

trap cleanup_temp_dir EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

if [[ ! -d "$TMPROOT" ]]; then
  blocked "temp root does not exist"
fi
TMPROOT="$(cd "$TMPROOT" && pwd -P)"

free_kib="$(available_kib)"
if [[ -z "$free_kib" || "$free_kib" -lt "$MIN_FREE_KIB" ]]; then
  blocked "insufficient temporary space: ${free_kib:-unknown} KiB available, $MIN_FREE_KIB KiB required"
fi
add_row "disk preflight" "pass" "$free_kib KiB free; required $MIN_FREE_KIB KiB"

RUN_TMP="$(mktemp -d "$TMPROOT/squallz-zip64.XXXXXX")"
add_row "temporary directory isolation" "pass" "this run owns a private temporary directory"

start_seconds="$(date +%s)"
set +e
(
  cd "$ROOT"
  export TMPDIR="$RUN_TMP"
  if [[ -x /usr/bin/time ]]; then
    case "$(uname -s)" in
      Darwin) LC_ALL=C /usr/bin/time -l "${CMD[@]}" ;;
      Linux) LC_ALL=C /usr/bin/time -v "${CMD[@]}" ;;
      *) "${CMD[@]}" ;;
    esac
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

if cleanup_temp_dir; then
  add_row "post-run temp cleanup" "pass" "this run's temporary directory was removed"
else
  add_row "post-run temp cleanup" "fail" "could not remove this run's temporary directory"
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
