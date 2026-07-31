#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SQZ="${SQUALLZ_BENCH_SQZ:-"$ROOT/target/release/sqz"}"
APP_TEMPLATE="${SQUALLZ_BENCH_APP_TEMPLATE:-"$ROOT/target/debug/bundle/macos/Squallz.app"}"
INPUT_MIB="${SQUALLZ_BENCH_INPUT_MIB:-128}"
ADDITION_MIB="${SQUALLZ_BENCH_ADDITION_MIB:-4}"
SPLIT_MIB="${SQUALLZ_BENCH_SPLIT_MIB:-32}"
REPETITIONS="${SQUALLZ_BENCH_REPETITIONS:-3}"
COLD_CACHE="${SQUALLZ_BENCH_COLD_CACHE:-0}"
CACHE_RESET="${SQUALLZ_BENCH_CACHE_RESET:-/usr/sbin/purge}"
REPORT="${SQUALLZ_BENCH_REPORT:-"$ROOT/benches/MACOS_CREATE_PIPELINE_BENCHMARK.md"}"
RUN_ROOT="${SQUALLZ_BENCH_RUN_ROOT:-"$ROOT/target/create-pipeline-benchmark"}"
RUN_ID="$(date -u +%Y%m%dT%H%M%SZ)-$$"
WORK="$RUN_ROOT/$RUN_ID"
INPUT="$WORK/input"
OUTPUT="$WORK/output"
RESULTS="$WORK/results.tsv"

fail() {
  echo "macos_create_pipeline_benchmark: $*" >&2
  exit 1
}

require_positive_integer() {
  local name="$1"
  local value="$2"
  [[ "$value" =~ ^[1-9][0-9]*$ ]] || fail "$name must be a positive integer"
}

require_positive_integer SQUALLZ_BENCH_INPUT_MIB "$INPUT_MIB"
require_positive_integer SQUALLZ_BENCH_ADDITION_MIB "$ADDITION_MIB"
require_positive_integer SQUALLZ_BENCH_SPLIT_MIB "$SPLIT_MIB"
require_positive_integer SQUALLZ_BENCH_REPETITIONS "$REPETITIONS"
((REPETITIONS % 2 == 1)) || fail "SQUALLZ_BENCH_REPETITIONS must be odd"
[[ "$COLD_CACHE" == "0" || "$COLD_CACHE" == "1" ]] \
  || fail "SQUALLZ_BENCH_COLD_CACHE must be 0 or 1"
[[ "$(uname -s)" == "Darwin" ]] || fail "this benchmark currently requires macOS"

if [[ ! -x "$SQZ" ]]; then
  (cd "$ROOT" && cargo build --release -p squallz-cli)
fi
[[ -x "$SQZ" ]] || fail "missing release CLI: $SQZ"
[[ -d "$APP_TEMPLATE" ]] \
  || fail "missing macOS app template: $APP_TEMPLATE; run 'make app-debug' first"

mkdir -p "$INPUT" "$OUTPUT" "$(dirname "$REPORT")"
printf 'case\tsample\treal_seconds\tuser_seconds\tsystem_seconds\toutput_bytes\n' >"$RESULTS"

PAYLOAD="$INPUT/payload.bin"
ADDITION="$INPUT/addition.bin"
dd if=/dev/urandom of="$PAYLOAD" bs=1048576 count="$INPUT_MIB" status=none
dd if=/dev/urandom of="$ADDITION" bs=1048576 count="$ADDITION_MIB" status=none

reset_cache() {
  if [[ "$COLD_CACHE" != "1" ]]; then
    return
  fi
  [[ -x "$CACHE_RESET" ]] \
    || fail "cache reset helper is not executable: $CACHE_RESET"
  "$CACHE_RESET" \
    || fail "cache reset failed; provide an executable privileged helper with SQUALLZ_BENCH_CACHE_RESET"
}

output_bytes() {
  local hint="$1"
  local total=0
  local path
  while IFS= read -r path; do
    if [[ -d "$path" ]]; then
      total=$((total + $(du -sk "$path" | awk '{print $1}') * 1024))
    elif [[ -f "$path" ]]; then
      total=$((total + $(stat -f '%z' "$path")))
    fi
  done < <(compgen -G "$hint" || true)
  printf '%s' "$total"
}

measure() {
  local label="$1"
  local sample="$2"
  local output_hint="$3"
  shift 3
  local timing="$WORK/$label-$sample.time"
  local stdout="$WORK/$label-$sample.json"

  reset_cache
  if ! /usr/bin/time -p "$@" >"$stdout" 2>"$timing"; then
    cat "$timing" >&2
    fail "$label failed"
  fi

  local real user system bytes
  real="$(awk '$1 == "real" { print $2; exit }' "$timing")"
  user="$(awk '$1 == "user" { print $2; exit }' "$timing")"
  system="$(awk '$1 == "sys" { print $2; exit }' "$timing")"
  bytes="$(output_bytes "$output_hint")"
  [[ -n "$real" && -n "$user" && -n "$system" ]] \
    || fail "$label did not produce portable time metrics"
  printf '%s\t%s\t%s\t%s\t%s\t%s\n' \
    "$label" "$sample" "$real" "$user" "$system" "$bytes" >>"$RESULTS"
}

for sample in $(seq 1 "$REPETITIONS"); do
  SAMPLE_OUTPUT="$OUTPUT/sample-$sample"
  mkdir -p "$SAMPLE_OUTPUT"
  ORDINARY="$SAMPLE_OUTPUT/ordinary.zip"
  VERIFIED="$SAMPLE_OUTPUT/verified.zip"
  UPDATE_ARCHIVE="$SAMPLE_OUTPUT/update.zip"
  SPLIT_BASE="$SAMPLE_OUTPUT/split.zip"
  SFX_APP="$SAMPLE_OUTPUT/payload.app"

  measure create_only "$sample" "$ORDINARY" \
    "$SQZ" --lang en-US --quiet compress "$PAYLOAD" \
    --output "$ORDINARY" --level 0 --content-policy keep-all-files --json

  measure create_and_full_test "$sample" "$VERIFIED" \
    "$SQZ" --lang en-US --quiet compress "$PAYLOAD" \
    --output "$VERIFIED" --level 0 --content-policy keep-all-files \
    --test-after-create --json

  cp "$ORDINARY" "$UPDATE_ARCHIVE"
  measure zip_update "$sample" "$UPDATE_ARCHIVE" \
    "$SQZ" --lang en-US --quiet update "$UPDATE_ARCHIVE" \
    --add "$ADDITION" --level 0 --content-policy keep-all-files --json

  "$SQZ" --lang en-US --quiet compress "$PAYLOAD" \
    --output "$SPLIT_BASE" --level 0 --content-policy keep-all-files \
    --split "${SPLIT_MIB}m" --split-mode generic --json >/dev/null
  measure split_replace "$sample" "$SPLIT_BASE.*" \
    "$SQZ" --lang en-US --quiet compress "$PAYLOAD" \
    --output "$SPLIT_BASE" --level 0 --content-policy keep-all-files \
    --split "${SPLIT_MIB}m" --split-mode generic --json

  "$SQZ" --lang en-US --quiet sfx create "$ORDINARY" \
    --output "$SFX_APP" --target macos --stub "$APP_TEMPLATE" --force --json >/dev/null
  measure sfx_replace "$sample" "$SFX_APP" \
    "$SQZ" --lang en-US --quiet sfx create "$ORDINARY" \
    --output "$SFX_APP" --target macos --stub "$APP_TEMPLATE" --force --json
done

metric_values() {
  local label="$1"
  local column="$2"
  awk -F '\t' -v label="$label" -v column="$column" \
    'NR > 1 && $1 == label { print $column }' "$RESULTS" | sort -n
}

metric_at() {
  local label="$1"
  local column="$2"
  local position="$3"
  metric_values "$label" "$column" | sed -n "${position}p"
}

median_metric() {
  metric_at "$1" "$2" "$((REPETITIONS / 2 + 1))"
}

CACHE_MODE="warm"
if [[ "$COLD_CACHE" == "1" ]]; then
  CACHE_MODE="cold"
fi

{
  printf '# macOS Create Pipeline Benchmark\n\n'
  printf 'Status: pass\n\n'
  printf '## Scope\n\n'
  printf 'This benchmark separates archive creation, post-create full reading, ZIP update, generic split replacement, and macOS SFX replacement. It uses a release CLI and a real Squallz app template.\n\n'
  printf '## Environment\n\n'
  printf -- '- Date (UTC): `%s`\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  printf -- '- Host: `%s`\n' "$(uname -m)"
  printf -- '- macOS: `%s`\n' "$(sw_vers -productVersion)"
  printf -- '- Squallz: `%s`\n' "$("$SQZ" --version)"
  printf -- '- Cache mode: `%s`\n' "$CACHE_MODE"
  printf -- '- Repetitions per case: `%s` (median reported)\n' "$REPETITIONS"
  printf -- '- Input: `%s MiB` incompressible data\n' "$INPUT_MIB"
  printf -- '- Update addition: `%s MiB`\n' "$ADDITION_MIB"
  printf -- '- Generic split target: `%s MiB`\n' "$SPLIT_MIB"
  printf -- '- Run directory: `%s`\n\n' "${WORK#$ROOT/}"
  printf '## Results\n\n'
  printf '| Case | Median real (s) | Real range (s) | Median user (s) | Median system (s) | Output bytes |\n'
  printf '| ---- | --------------: | -------------: | --------------: | ----------------: | -----------: |\n'
  for label in create_only create_and_full_test zip_update split_replace sfx_replace; do
    printf '| %s | %s | %s–%s | %s | %s | %s |\n' \
      "$label" \
      "$(median_metric "$label" 3)" \
      "$(metric_at "$label" 3 1)" \
      "$(metric_at "$label" 3 "$REPETITIONS")" \
      "$(median_metric "$label" 4)" \
      "$(median_metric "$label" 5)" \
      "$(median_metric "$label" 6)"
  done
  printf '\n## Samples\n\n'
  printf '| Case | Sample | Real (s) | User (s) | System (s) | Output bytes |\n'
  printf '| ---- | -----: | -------: | -------: | ---------: | -----------: |\n'
  awk -F '\t' 'NR > 1 { printf "| %s | %s | %s | %s | %s | %s |\n", $1, $2, $3, $4, $5, $6 }' "$RESULTS"
  printf '\n## Boundaries\n\n'
  if [[ "$COLD_CACHE" == "1" ]]; then
    printf -- '- The configured cache-reset helper completed before every measured case.\n'
  else
    printf -- '- This run is warm-cache evidence. It must not be cited as cold-cache performance.\n'
    printf -- '- Set `SQUALLZ_BENCH_COLD_CACHE=1` and provide a permitted cache-reset helper to collect cold-cache evidence.\n'
  fi
  printf -- '- `create_and_full_test` includes the safety read; compare it with `create_only` instead of disabling integrity checks in production.\n'
  printf -- '- Replacement cases include destination inspection and durable publication work. Preparation of the previous output is outside the measured interval.\n'
} >"$REPORT"

printf 'report=%s\n' "$REPORT"
printf 'run=%s\n' "$WORK"
printf 'cache_mode=%s\n' "$CACHE_MODE"
