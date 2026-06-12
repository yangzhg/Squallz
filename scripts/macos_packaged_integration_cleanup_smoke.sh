#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DEFAULT_RELEASE="$ROOT/target/release/bundle/macos/Squallz.app"
DEFAULT_DEBUG="$ROOT/target/debug/bundle/macos/Squallz.app"
if [[ -d "$DEFAULT_RELEASE" ]]; then
  APP="${1:-"$DEFAULT_RELEASE"}"
else
  APP="${1:-"$DEFAULT_DEBUG"}"
fi
if [[ "$APP" != /* ]]; then
  APP="$ROOT/$APP"
fi
EXE="$APP/Contents/MacOS/squallz-gui"
SQZ_HELPER="$APP/Contents/Resources/bin/sqz"
WORK="$ROOT/target/squallz-macos-packaged-integration-cleanup-smoke"
HOME_DIR="$WORK/home"
TRACE="$WORK/trace.jsonl"
REPORT="$ROOT/benches/MACOS_PACKAGED_INTEGRATION_CLEANUP_SMOKE.md"

fail() {
  echo "macos_packaged_integration_cleanup_smoke: $*" >&2
  exit 1
}

if [[ "$(uname -s)" != "Darwin" ]]; then
  fail "this smoke check only runs on macOS"
fi
if [[ ! -x "$EXE" ]]; then
  fail "missing app executable: $EXE; run 'make app-macos' first"
fi
if [[ ! -x "$SQZ_HELPER" ]]; then
  fail "missing bundled sqz helper: $SQZ_HELPER"
fi
if pgrep -x squallz-gui >/dev/null; then
  fail "squallz-gui is already running; close it before running packaged integration cleanup smoke"
fi

mkdir -p "$WORK" "$ROOT/benches"
rm -rf "$HOME_DIR" "$TRACE" "$REPORT"
mkdir -p "$HOME_DIR/Library/Application Support/Squallz"

cat > "$HOME_DIR/Library/Application Support/Squallz/settings.json" <<'JSON'
{
  "theme": "light",
  "language": "en-US",
  "ui_mode": "modern"
}
JSON

cleanup() {
  if [[ -n "${APP_PID:-}" ]]; then
    kill "$APP_PID" >/dev/null 2>&1 || true
  fi
}
trap cleanup EXIT

open -n -a "$APP" \
  --env "HOME=$HOME_DIR" \
  --env "SQUALLZ_VALIDATION_TRACE=$TRACE" \
  --env "SQUALLZ_VALIDATION_INTEGRATION=1"

APP_PID=""
for _ in {1..70}; do
  APP_PID="$(pgrep -n -x squallz-gui || true)"
  if [[ -n "$APP_PID" ]]; then
    break
  fi
  sleep 0.1
done
[[ -n "$APP_PID" ]] || fail "app process did not start"

for _ in {1..120}; do
  if grep -q '"event":"integration.status.after_remove"' "$TRACE" 2>/dev/null; then
    break
  fi
  sleep 0.1
done
grep -q '"event":"integration.status.after_remove"' "$TRACE" 2>/dev/null || {
  [[ -f "$TRACE" ]] && cat "$TRACE" >&2
  fail "integration cleanup smoke did not finish"
}

SUMMARY="$(python3 - "$TRACE" "$HOME_DIR" <<'PY'
import json
import pathlib
import sys

trace = pathlib.Path(sys.argv[1])
home = pathlib.Path(sys.argv[2])
events = [json.loads(line) for line in trace.read_text(encoding="utf-8").splitlines() if line.strip()]
payloads = {event: [item["payload"] for item in events if item.get("event") == event] for event in {
    "integration.apply.ok",
    "integration.status.after_apply",
    "integration.remove.ok",
    "integration.status.after_remove",
}}
for key in payloads:
    assert payloads[key], f"missing {key}"
apply = payloads["integration.apply.ok"][-1]
after_apply = payloads["integration.status.after_apply"][-1]
remove = payloads["integration.remove.ok"][-1]
after_remove = payloads["integration.status.after_remove"][-1]
assert apply["platform"] == "macos", apply
assert len(apply.get("installed", [])) == 4, apply
assert len(after_apply.get("installed", [])) == 4 and not after_apply.get("missing"), after_apply
assert len(remove.get("removed", [])) == 4 and not remove.get("missing"), remove
assert not after_remove.get("installed"), after_remove
assert len(after_remove.get("missing", [])) == 4, after_remove
services_dir = pathlib.Path(remove["services_dir"])
script_dir = pathlib.Path(remove["script_dir"])
assert str(services_dir).startswith(str(home)), services_dir
assert str(script_dir).startswith(str(home)), script_dir
for item in apply["installed"]:
    workflow = pathlib.Path(item["path"])
    script = pathlib.Path(item["script_path"])
    assert not workflow.exists(), f"workflow still exists: {workflow}"
    assert not script.exists(), f"script still exists: {script}"
print(f"installed={len(apply['installed'])}")
print(f"removed={len(remove['removed'])}")
print(f"after_remove_installed={len(after_remove.get('installed', []))}")
print(f"after_remove_missing={len(after_remove.get('missing', []))}")
print(f"services_dir={services_dir}")
print(f"script_dir={script_dir}")
PY
)"

TRACE_SUMMARY="$(python3 - "$TRACE" <<'PY'
import json
import sys
for line in open(sys.argv[1], encoding="utf-8"):
    item = json.loads(line)
    payload = item["payload"]
    if item["event"] == "integration.apply.ok":
        detail = f"installed={len(payload.get('installed', []))}"
    elif item["event"] == "integration.status.after_apply":
        detail = f"installed={len(payload.get('installed', []))} missing={len(payload.get('missing', []))}"
    elif item["event"] == "integration.remove.ok":
        detail = f"removed={len(payload.get('removed', []))} missing={len(payload.get('missing', []))}"
    elif item["event"] == "integration.status.after_remove":
        detail = f"installed={len(payload.get('installed', []))} missing={len(payload.get('missing', []))}"
    else:
        detail = str(payload)
    print(f"- +{item.get('process_ms', '?')}ms {item['event']}: {detail}")
PY
)"

cat > "$REPORT" <<EOF
# Squallz macOS Packaged Integration Cleanup Smoke

Generated: $(date -u +"%Y-%m-%dT%H:%M:%SZ")

## Scope

This smoke check launches the packaged macOS app through LaunchServices with an
isolated HOME, lets the real Tauri backend install Finder Quick Actions, then
lets the same smoke path remove them. It proves packaged integration install
and uninstall paths are paired and do not leave Squallz workflow/script files
behind in the isolated user profile.

## Inputs

- App: \`$APP\`
- Bundled sqz: \`$SQZ_HELPER\`
- Isolated HOME: \`$HOME_DIR\`
- Trace: \`$TRACE\`

## Checks

- \`squallz-gui\` starts from the bundle.
- \`integration.apply.ok\` installs exactly four Finder actions.
- \`integration.status.after_apply\` reports four installed actions and no missing actions.
- \`integration.remove.ok\` removes exactly four Finder actions and reports no missing actions.
- \`integration.status.after_remove\` reports zero installed actions and four missing actions.
- Every workflow/script path installed by the packaged app is absent after removal.

## Cleanup Summary

\`\`\`text
$SUMMARY
\`\`\`

## Trace Summary

$TRACE_SUMMARY

## Boundary

This smoke uses an isolated HOME and does not modify the real user's
\`~/Library/Services\`. It does not click Finder UI, refresh the system Services
cache, or validate Windows/Linux shell integration uninstallers.
EOF

echo "report=$REPORT"
echo "trace=$TRACE"
echo "status=pass"
