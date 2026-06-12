#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RUNS="${SQUALLZ_FUZZ_RUNS:-64}"
MAX_LEN="${SQUALLZ_FUZZ_MAX_LEN:-4096}"
REQUIRE_CARGO_FUZZ="${SQUALLZ_FUZZ_REQUIRE_CARGO_FUZZ:-0}"
WORK="$ROOT/target/squallz-zip-fuzz"
LOG="$WORK/zip_reader.log"
REPORT="${SQUALLZ_FUZZ_REPORT:-"$ROOT/benches/ZIP_FUZZ_CAMPAIGN.md"}"
ARTIFACT_DIR="$ROOT/fuzz/artifacts/zip_reader"
CORPUS_DIR="$ROOT/fuzz/corpus/zip_reader"

fail() {
  echo "zip_fuzz_smoke: $*" >&2
  exit 1
}

[[ "$RUNS" =~ ^[0-9]+$ ]] || fail "SQUALLZ_FUZZ_RUNS must be a positive integer"
(( RUNS > 0 )) || fail "SQUALLZ_FUZZ_RUNS must be > 0"
[[ "$MAX_LEN" =~ ^[0-9]+$ ]] || fail "SQUALLZ_FUZZ_MAX_LEN must be a positive integer"
(( MAX_LEN > 0 )) || fail "SQUALLZ_FUZZ_MAX_LEN must be > 0"
if [[ "$REPORT" != /* ]]; then
  REPORT="$ROOT/$REPORT"
fi
mkdir -p "$WORK" "$(dirname "$REPORT")" "$ARTIFACT_DIR"
rm -f "$LOG"

count_files() {
  local dir="$1"
  if [[ ! -d "$dir" ]]; then
    printf '0'
    return
  fi
  find "$dir" -type f | wc -l | tr -d ' '
}

write_report() {
  local mode="$1"
  local status="$2"
  local command="$3"
  local exit_code="$4"
  local artifact_count="$5"
  local corpus_count="$6"

  python3 - "$REPORT" "$LOG" "$mode" "$status" "$command" "$exit_code" "$artifact_count" "$corpus_count" "$RUNS" "$MAX_LEN" "$ROOT" <<'PY'
import re
import sys
from datetime import datetime
from pathlib import Path

report, log_path, mode, status, command, exit_code, artifact_count, corpus_count, runs, max_len, root = sys.argv[1:]
log = Path(log_path).read_text(encoding="utf-8", errors="replace") if Path(log_path).exists() else ""

def sh(command):
    import subprocess
    try:
        return subprocess.check_output(command, cwd=root, text=True, stderr=subprocess.STDOUT).strip()
    except Exception as exc:
        return f"unavailable ({exc})"

seed = "n/a"
match = re.search(r"seed corpus: files: (\d+).*?total: (\d+)b", log)
if match:
    seed = f"{match.group(1)} files, {match.group(2)} bytes total"

done_lines = [line for line in log.splitlines() if "DONE" in line or "INITED" in line]
final_line = done_lines[-1] if done_lines else ""
final_corpus = "n/a"
match = re.search(r"corp: (\d+)/([0-9]+(?:[KMG]?b)?)", final_line)
if match:
    final_corpus = f"{match.group(1)} files, {match.group(2)} total"

coverage = "n/a"
match = re.search(r"cov: (\d+).*?ft: (\d+)", final_line)
if match:
    coverage = f"cov: {match.group(1)}, ft: {match.group(2)}"

executed = "n/a"
numbers = re.findall(r"#(\d+)\b", log)
if numbers:
    executed = numbers[-1]

rss_values = [int(value) for value in re.findall(r"rss: (\d+)Mb", log)]
peak_rss = "n/a" if not rss_values else f"{max(rss_values)} MiB"

lines = [
    "# ZIP Reader Fuzz Campaign",
    "",
    f"- Date: {datetime.now().astimezone().strftime('%Y-%m-%d %H:%M %Z')}",
    "- Target: `fuzz/fuzz_targets/zip_reader.rs`",
    f"- Command: `{command}`",
    f"- Mode: `{mode}`",
    f"- Result: {status}.",
    "",
    "## Toolchain",
    "",
    f"- `{sh(['cargo', 'fuzz', '--version'])}`",
    f"- `{sh(['cargo', '+nightly', '--version'])}`",
    f"- `{sh(['rustc', '+nightly', '--version'])}`",
    f"- Host stable cargo: `{sh(['cargo', '--version'])}`",
    f"- Host stable rustc: `{sh(['rustc', '--version'])}`",
    "",
    "## Target Bounds",
    "",
    "The campaign uses the bounded ZIP reader fuzz target:",
    "",
    "- Maximum input accepted by the harness: 2 MiB.",
    f"- libFuzzer max generated input in this run: {max_len} bytes.",
    "- Entry listing cap: first 32 entries.",
    "- File read cap: first 8 file entries, up to 64 KiB each.",
    "- Archive `test()` is invoked only when declared file bytes are <= 256 KiB.",
    "",
    "These bounds keep the campaign focused on parser robustness instead of turning a fuzz gate into a decompression-bomb or throughput benchmark.",
    "",
    "## Run Summary",
    "",
    f"- Requested runs: {runs}.",
    f"- Executed units reported by libFuzzer: {executed}.",
    f"- Seed corpus at start: {seed}.",
    f"- Final corpus reported by libFuzzer: {final_corpus}.",
    f"- Corpus files on disk after run: {corpus_count}.",
    f"- Final coverage: `{coverage}`.",
    f"- Peak RSS reported by libFuzzer: {peak_rss}.",
    f"- Fuzz command exit code: {exit_code}.",
    f"- Artifacts directory after run: `fuzz/artifacts/zip_reader/` contained {artifact_count} crash files.",
    f"- Raw log: `{Path(log_path)}`.",
    "",
    "Randomly generated corpus files remain ignored. Only minimized crashes or deliberate regression seeds should be promoted to tracked fixtures after review.",
    "",
    "## Follow-up",
    "",
    "- This campaign is stronger than the default smoke but still not a substitute for continuous overnight/CI fuzzing.",
    "- If a future campaign finds a crash, minimize it first and add a deterministic regression fixture before expanding the corpus.",
    "",
]
Path(report).write_text("\n".join(lines), encoding="utf-8")
print(report)
PY
}

if cargo fuzz --version >/dev/null 2>&1 && cargo +nightly --version >/dev/null 2>&1; then
  command="SQUALLZ_FUZZ_REQUIRE_CARGO_FUZZ=$REQUIRE_CARGO_FUZZ SQUALLZ_FUZZ_RUNS=$RUNS SQUALLZ_FUZZ_MAX_LEN=$MAX_LEN scripts/zip_fuzz_smoke.sh"
  printf 'running cargo-fuzz ZIP reader smoke (%s runs, max_len=%s)\n' "$RUNS" "$MAX_LEN"
  set +e
  (cd "$ROOT" && cargo +nightly fuzz run zip_reader -- -runs="$RUNS" -max_len="$MAX_LEN") 2>&1 | tee "$LOG"
  status_code=${PIPESTATUS[0]}
  set -e

  artifact_count="$(count_files "$ARTIFACT_DIR")"
  corpus_count="$(count_files "$CORPUS_DIR")"
  if [[ "$status_code" -eq 0 && "$artifact_count" -eq 0 ]]; then
    write_report "cargo-fuzz" "passed, no crash artifact produced" "$command" "$status_code" "$artifact_count" "$corpus_count"
  else
    write_report "cargo-fuzz" "failed" "$command" "$status_code" "$artifact_count" "$corpus_count"
    exit 1
  fi
else
  if [[ "$REQUIRE_CARGO_FUZZ" == "1" ]]; then
    fail "cargo-fuzz and nightly Rust are required for this campaign"
  fi
  printf 'cargo-fuzz or nightly Rust is not available; compiling ZIP fuzz target with cargo check fallback\n'
  set +e
  cargo check --manifest-path "$ROOT/fuzz/Cargo.toml" --bins 2>&1 | tee "$LOG"
  status_code=${PIPESTATUS[0]}
  set -e
  artifact_count="$(count_files "$ARTIFACT_DIR")"
  corpus_count="$(count_files "$CORPUS_DIR")"
  if [[ "$status_code" -eq 0 ]]; then
    write_report "cargo-check-fallback" "passed; cargo-fuzz was unavailable" "scripts/zip_fuzz_smoke.sh" "$status_code" "$artifact_count" "$corpus_count"
  else
    write_report "cargo-check-fallback" "failed" "scripts/zip_fuzz_smoke.sh" "$status_code" "$artifact_count" "$corpus_count"
    exit 1
  fi
fi
