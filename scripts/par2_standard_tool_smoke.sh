#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK="${TMPDIR:-/tmp}/squallz-par2-standard-smoke-$$"
REPORT="$ROOT/benches/PAR2_STANDARD_TOOL_SMOKE.md"

cleanup() {
  rm -rf "$WORK"
}
trap cleanup EXIT

find_par2() {
  local name
  for name in par2cmdline-turbo par2 par2cmdline; do
    if command -v "$name" >/dev/null 2>&1; then
      command -v "$name"
      return 0
    fi
  done
  return 1
}

hash_file() {
  python3 - "$1" <<'PY'
import hashlib
import pathlib
import sys

print(hashlib.sha256(pathlib.Path(sys.argv[1]).read_bytes()).hexdigest())
PY
}

assert_json_report() {
  python3 - "$@" <<'PY'
import json
import pathlib
import sys

path, expected_ok, expected_operation, expected_status, expected_redundancy = sys.argv[1:6]
data = json.loads(pathlib.Path(path).read_text())
assert data["ok"] is (expected_ok == "true"), data
assert data["operation"] == expected_operation, data
if expected_status != "*":
    assert data["status_code"] == int(expected_status), data
if expected_redundancy != "*":
    assert data["redundancy_percent"] == int(expected_redundancy), data
assert data["metrics"] is None, data
assert data["tool"], data
PY
}

summarize_json() {
  python3 - "$1" <<'PY'
import json
import pathlib
import sys

data = json.loads(pathlib.Path(sys.argv[1]).read_text())
summary = {
    "ok": data["ok"],
    "operation": data["operation"],
    "output": pathlib.Path(data["output"]).name if data.get("output") else None,
    "tool": pathlib.Path(data["tool"]).name,
    "redundancy_percent": data["redundancy_percent"],
    "status_code": data["status_code"],
    "metrics": data["metrics"],
}
print(json.dumps(summary, indent=2, sort_keys=True))
PY
}

PAR2_TOOL="$(find_par2 || true)"
if [[ -z "$PAR2_TOOL" ]]; then
  echo "No par2cmdline-compatible executable found in PATH." >&2
  echo "Install par2cmdline-turbo, par2, or par2cmdline before running this smoke." >&2
  exit 20
fi
PAR2_VERSION="$("$PAR2_TOOL" -V 2>&1 | head -n 1 || true)"

mkdir -p "$WORK" "$ROOT/benches"
cargo build --quiet -p squallz-cli
SQZ="$ROOT/target/debug/sqz"

SINGLE="$WORK/single"
mkdir -p "$SINGLE"
python3 - "$SINGLE/data.bin" <<'PY'
import pathlib
import sys

path = pathlib.Path(sys.argv[1])
payload = bytes((i * 37 + (i // 97)) % 251 for i in range(256 * 1024))
path.write_bytes(payload)
PY

SINGLE_ARCHIVE="$SINGLE/data.bin"
SINGLE_RECOVERY="$SINGLE/data.bin.par2"
SINGLE_ORIG_HASH="$(hash_file "$SINGLE_ARCHIVE")"

env SQUALLZ_PAR2="$PAR2_TOOL" "$SQZ" protect "$SINGLE_ARCHIVE" \
  --redundancy 20 \
  --recovery "$SINGLE_RECOVERY" \
  --json > "$WORK/single-protect.json"
assert_json_report "$WORK/single-protect.json" true protect 0 20
test -f "$SINGLE_RECOVERY"

env SQUALLZ_PAR2="$PAR2_TOOL" "$SQZ" verify "$SINGLE_ARCHIVE" \
  --use-recovery \
  --recovery "$SINGLE_RECOVERY" \
  --json > "$WORK/single-verify-intact.json"
assert_json_report "$WORK/single-verify-intact.json" true verify 0 "*"

python3 - "$SINGLE_ARCHIVE" <<'PY'
import pathlib
import sys

path = pathlib.Path(sys.argv[1])
data = bytearray(path.read_bytes())
data[4096] ^= 0x5A
path.write_bytes(data)
PY

set +e
env SQUALLZ_PAR2="$PAR2_TOOL" "$SQZ" verify "$SINGLE_ARCHIVE" \
  --use-recovery \
  --recovery "$SINGLE_RECOVERY" \
  --json > "$WORK/single-verify-damaged.json"
SINGLE_VERIFY_DAMAGED=$?
set -e
if [[ "$SINGLE_VERIFY_DAMAGED" != "3" ]]; then
  echo "expected damaged verify exit 3, got $SINGLE_VERIFY_DAMAGED" >&2
  cat "$WORK/single-verify-damaged.json" >&2
  exit 21
fi
assert_json_report "$WORK/single-verify-damaged.json" false verify "*" "*"
SINGLE_DAMAGED_HASH="$(hash_file "$SINGLE_ARCHIVE")"

SINGLE_COPY_REPAIR="$SINGLE/repaired-copy.bin"
env SQUALLZ_PAR2="$PAR2_TOOL" "$SQZ" repair "$SINGLE_ARCHIVE" \
  --use-recovery \
  --recovery "$SINGLE_RECOVERY" \
  --output "$SINGLE_COPY_REPAIR" \
  --json > "$WORK/single-repair-output.json"
assert_json_report "$WORK/single-repair-output.json" true repair 0 "*"
test -f "$SINGLE_COPY_REPAIR"

SINGLE_AFTER_COPY_HASH="$(hash_file "$SINGLE_ARCHIVE")"
SINGLE_COPY_REPAIR_HASH="$(hash_file "$SINGLE_COPY_REPAIR")"
if [[ "$SINGLE_AFTER_COPY_HASH" != "$SINGLE_DAMAGED_HASH" ]]; then
  echo "output-copy repair mutated the source archive" >&2
  exit 27
fi
if [[ "$SINGLE_ORIG_HASH" != "$SINGLE_COPY_REPAIR_HASH" ]]; then
  echo "output-copy repair hash mismatch" >&2
  exit 28
fi

env SQUALLZ_PAR2="$PAR2_TOOL" "$SQZ" repair "$SINGLE_ARCHIVE" \
  --use-recovery \
  --recovery "$SINGLE_RECOVERY" \
  --json > "$WORK/single-repair.json"
assert_json_report "$WORK/single-repair.json" true repair 0 "*"

env SQUALLZ_PAR2="$PAR2_TOOL" "$SQZ" verify "$SINGLE_ARCHIVE" \
  --use-recovery \
  --recovery "$SINGLE_RECOVERY" \
  --json > "$WORK/single-verify-repaired.json"
assert_json_report "$WORK/single-verify-repaired.json" true verify 0 "*"

SINGLE_REPAIRED_HASH="$(hash_file "$SINGLE_ARCHIVE")"
if [[ "$SINGLE_ORIG_HASH" != "$SINGLE_REPAIRED_HASH" ]]; then
  echo "single-file repair hash mismatch" >&2
  exit 22
fi

OVER="$WORK/over-limit"
mkdir -p "$OVER"
python3 - "$OVER/data.bin" <<'PY'
import pathlib
import sys

path = pathlib.Path(sys.argv[1])
payload = bytes((i * 17 + (i // 11)) % 251 for i in range(256 * 1024))
path.write_bytes(payload)
PY

OVER_ARCHIVE="$OVER/data.bin"
OVER_RECOVERY="$OVER/data.bin.par2"
env SQUALLZ_PAR2="$PAR2_TOOL" "$SQZ" protect "$OVER_ARCHIVE" \
  --redundancy 1 \
  --recovery "$OVER_RECOVERY" \
  --json > "$WORK/over-protect.json"
assert_json_report "$WORK/over-protect.json" true protect 0 1

python3 - "$OVER_ARCHIVE" <<'PY'
import pathlib
import sys

path = pathlib.Path(sys.argv[1])
data = bytearray(path.read_bytes())
for offset in range(0, len(data), 1024):
    data[offset] ^= 0x5A
path.write_bytes(data)
PY

set +e
env SQUALLZ_PAR2="$PAR2_TOOL" "$SQZ" verify "$OVER_ARCHIVE" \
  --use-recovery \
  --recovery "$OVER_RECOVERY" \
  --json > "$WORK/over-verify.json"
OVER_VERIFY=$?
env SQUALLZ_PAR2="$PAR2_TOOL" "$SQZ" repair "$OVER_ARCHIVE" \
  --use-recovery \
  --recovery "$OVER_RECOVERY" \
  --json > "$WORK/over-repair.json"
OVER_REPAIR=$?
set -e
if [[ "$OVER_VERIFY" != "3" ]]; then
  echo "expected over-limit verify exit 3, got $OVER_VERIFY" >&2
  cat "$WORK/over-verify.json" >&2
  exit 25
fi
if [[ "$OVER_REPAIR" != "3" ]]; then
  echo "expected over-limit repair exit 3, got $OVER_REPAIR" >&2
  cat "$WORK/over-repair.json" >&2
  exit 26
fi
assert_json_report "$WORK/over-verify.json" false verify "*" "*"
assert_json_report "$WORK/over-repair.json" false repair "*" "*"

SPLIT="$WORK/split"
mkdir -p "$SPLIT"
python3 - "$SPLIT" <<'PY'
import pathlib
import sys

root = pathlib.Path(sys.argv[1])
(root / "split.zip.001").write_bytes(b"A" * 4096)
(root / "split.zip.002").write_bytes(b"B" * 4096)
(root / "split.zip.003").write_bytes(b"C" * 4096)
PY

SPLIT_FIRST="$SPLIT/split.zip.001"
SPLIT_SECOND="$SPLIT/split.zip.002"
SPLIT_RECOVERY="$SPLIT/split.zip.par2"
SPLIT_SECOND_HASH="$(hash_file "$SPLIT_SECOND")"

env SQUALLZ_PAR2="$PAR2_TOOL" "$SQZ" protect "$SPLIT_FIRST" \
  --tolerate-loss 1volume \
  --recovery "$SPLIT_RECOVERY" \
  --json > "$WORK/split-protect.json"
assert_json_report "$WORK/split-protect.json" true protect 0 34
test -f "$SPLIT_RECOVERY"

rm "$SPLIT_SECOND"
set +e
env SQUALLZ_PAR2="$PAR2_TOOL" "$SQZ" verify "$SPLIT_FIRST" \
  --use-recovery \
  --recovery "$SPLIT_RECOVERY" \
  --json > "$WORK/split-verify-missing.json"
SPLIT_VERIFY_MISSING=$?
set -e
if [[ "$SPLIT_VERIFY_MISSING" != "3" ]]; then
  echo "expected split missing verify exit 3, got $SPLIT_VERIFY_MISSING" >&2
  cat "$WORK/split-verify-missing.json" >&2
  exit 23
fi
assert_json_report "$WORK/split-verify-missing.json" false verify "*" "*"

env SQUALLZ_PAR2="$PAR2_TOOL" "$SQZ" repair "$SPLIT_FIRST" \
  --use-recovery \
  --recovery "$SPLIT_RECOVERY" \
  --json > "$WORK/split-repair.json"
assert_json_report "$WORK/split-repair.json" true repair 0 "*"
test -f "$SPLIT_SECOND"

SPLIT_SECOND_REPAIRED_HASH="$(hash_file "$SPLIT_SECOND")"
if [[ "$SPLIT_SECOND_HASH" != "$SPLIT_SECOND_REPAIRED_HASH" ]]; then
  echo "split-volume repair hash mismatch" >&2
  exit 24
fi

cat > "$REPORT" <<EOF
# PAR2 Standard Tool Smoke

Date: $(date -u +"%Y-%m-%dT%H:%M:%SZ")

Scope: I10-S2 standard PAR2 create/verify/repair interoperability through the Squallz CLI.

Tool:
- Path: \`$PAR2_TOOL\`
- Version: \`$PAR2_VERSION\`

Commands:

\`\`\`bash
bash -n scripts/par2_standard_tool_smoke.sh
scripts/par2_standard_tool_smoke.sh
\`\`\`

Results:

| Case | Command path | Expected CLI behavior | Result |
| ---- | ---- | ---- | ---- |
| Single protected file | \`sqz protect --redundancy 20\` then verify | PAR2 sidecar created and intact verify exits 0 | Passed |
| Single damaged file | Flip one byte, then \`sqz verify --use-recovery\` | CLI exits 3 and JSON reports \`ok=false\` | Passed |
| Single damaged output-copy repair | \`sqz repair --use-recovery --output repaired-copy.bin\` | CLI exits 0, original stays damaged, output SHA-256 matches original | Passed |
| Single damaged repair | \`sqz repair --use-recovery\` then verify | CLI exits 0 and SHA-256 returns to original | Passed |
| Over-limit damage | \`sqz protect --redundancy 1\`, corrupt too many blocks | Verify and repair both exit 3 with \`ok=false\` | Passed |
| Split missing volume | \`sqz protect --tolerate-loss 1volume\`, delete \`.002\` | Verify exits 3, repair recreates missing volume, SHA-256 matches | Passed |

Representative JSON summaries:

Single protect:
\`\`\`json
$(summarize_json "$WORK/single-protect.json")
\`\`\`

Single damaged verify:
\`\`\`json
$(summarize_json "$WORK/single-verify-damaged.json")
\`\`\`

Single repair:
\`\`\`json
$(summarize_json "$WORK/single-repair.json")
\`\`\`

Single output-copy repair:
\`\`\`json
$(summarize_json "$WORK/single-repair-output.json")
\`\`\`

Over-limit repair:
\`\`\`json
$(summarize_json "$WORK/over-repair.json")
\`\`\`

Split protect:
\`\`\`json
$(summarize_json "$WORK/split-protect.json")
\`\`\`

Split missing repair:
\`\`\`json
$(summarize_json "$WORK/split-repair.json")
\`\`\`

Boundary:

- External standard PAR2 output remains unstructured; Squallz exposes \`metrics: null\` for this bridge.
- The built-in \`rust-par2\` fallback still covers verify/repair only; standard PAR2 create still requires an external tool.
- This smoke closes the standard \`par2\` create/verify/repair gate for local development. Packaging a GPL PAR2 executable remains a release/legal decision for I15.
EOF

echo "Wrote $REPORT"
