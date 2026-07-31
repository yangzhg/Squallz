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
SQZ_HELPER="$APP/Contents/MacOS/sqz"
WORK="$ROOT/target/squallz-macos-packaged-quick-actions-smoke"
HOME_DIR="$WORK/home"
TRACE="$WORK/trace.jsonl"
FIXTURE="$WORK/fixture"
FAKE_BIN="$WORK/fake-bin"
REPORT="$ROOT/benches/MACOS_PACKAGED_QUICK_ACTIONS_SMOKE.md"

fail() {
  echo "macos_packaged_quick_actions_smoke: $*" >&2
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

declared_minimum="$(/usr/bin/plutil -extract LSMinimumSystemVersion raw "$APP/Contents/Info.plist")"
verify_binary_minimum() {
  local binary="$1"
  local minimum_versions
  minimum_versions="$(/usr/bin/xcrun vtool -show-build "$binary" \
    | awk '$1 == "minos" { print $2 }' \
    | sort -u)"
  if [[ -z "$minimum_versions" ]]; then
    fail "missing LC_BUILD_VERSION minos in $binary"
  fi
  if [[ "$minimum_versions" != "$declared_minimum" ]]; then
    fail "$binary requires macOS $minimum_versions but Info.plist declares $declared_minimum"
  fi
}
verify_binary_minimum "$EXE"
verify_binary_minimum "$SQZ_HELPER"

if pgrep -x squallz-gui >/dev/null; then
  fail "squallz-gui is already running; close it before running packaged Quick Actions smoke"
fi
mkdir -p "$WORK" "$ROOT/benches" "$HOME_DIR/Library/Application Support/Squallz" "$FIXTURE"
rm -f "$TRACE" "$REPORT"
rm -rf "$HOME_DIR/Library/Services" "$HOME_DIR/Library/Application Support/Squallz/context-actions" "$FIXTURE" "$FAKE_BIN"
mkdir -p "$HOME_DIR/Library/Application Support/Squallz" "$FIXTURE/source/sub" "$FAKE_BIN"

cat > "$HOME_DIR/Library/Application Support/Squallz/settings.json" <<'JSON'
{
  "theme": "light",
  "language": "en-US",
  "ui_mode": "modern"
}
JSON

printf 'alpha from packaged Quick Action smoke\n' > "$FIXTURE/source/a.txt"
printf 'nested payload\n' > "$FIXTURE/source/sub/b.txt"
cat > "$FAKE_BIN/sqz" <<'SH'
#!/usr/bin/env bash
echo "fake PATH sqz must not be used by packaged Quick Actions smoke" >&2
exit 97
SH
chmod +x "$FAKE_BIN/sqz"
python3 - "$FIXTURE/source.zip" "$FIXTURE/source" <<'PY'
import pathlib
import sys
import zipfile

archive = pathlib.Path(sys.argv[1])
source = pathlib.Path(sys.argv[2])
with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_DEFLATED) as z:
    for path in sorted(source.rglob("*")):
        if path.is_file():
            z.write(path, path.relative_to(source.parent).as_posix())
PY
mkdir -p "$FIXTURE/here-case" "$FIXTURE/folder-case" "$FIXTURE/compress-case/source/sub"
cp "$FIXTURE/source.zip" "$FIXTURE/here-case/here.zip"
cp "$FIXTURE/source.zip" "$FIXTURE/folder-case/folder.zip"
printf 'packaged helper compress payload\n' > "$FIXTURE/compress-case/source/c.txt"
printf 'nested compress payload\n' > "$FIXTURE/compress-case/source/sub/d.txt"

cleanup() {
  while IFS= read -r pid; do
    [[ -n "$pid" ]] && kill "$pid" >/dev/null 2>&1 || true
  done < <(pgrep -x squallz-gui || true)
}
trap cleanup EXIT

wait_for_file() {
  local path="$1"
  local description="$2"
  for _ in {1..200}; do
    [[ -f "$path" ]] && return 0
    sleep 0.05
  done
  fail "$description"
}

wait_for_archive() {
  local path="$1"
  for _ in {1..200}; do
    if [[ -f "$path" ]] && "$SQZ_HELPER" test "$path" >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.05
  done
  fail "compress-to-7z did not finish a readable archive"
}

open -n -a "$APP" \
  --env "HOME=$HOME_DIR" \
  --env "SQUALLZ_VALIDATION_TRACE=$TRACE" \
  --env "SQUALLZ_VALIDATION_INTEGRATION=1" \
  --env "SQUALLZ_VALIDATION_INTEGRATION_KEEP=1"

APP_PID=""
for _ in {1..70}; do
  APP_PID="$(pgrep -n -x squallz-gui || true)"
  if [[ -n "$APP_PID" ]]; then
    break
  fi
  sleep 0.1
done
[[ -n "$APP_PID" ]] || fail "app process did not start"

for _ in {1..100}; do
  if grep -q '"event":"integration.keep.ok"' "$TRACE" 2>/dev/null; then
    break
  fi
  sleep 0.1
done
grep -q '"event":"integration.keep.ok"' "$TRACE" 2>/dev/null || {
  [[ -f "$TRACE" ]] && cat "$TRACE" >&2
  fail "integration keep smoke did not finish"
}

mapfile -t ACTIONS < <(python3 - "$TRACE" "$HOME_DIR" "$APP" <<'PY'
import json
import pathlib
import sys

trace = pathlib.Path(sys.argv[1])
home = pathlib.Path(sys.argv[2])
app = pathlib.Path(sys.argv[3])
events = [json.loads(line) for line in trace.read_text(encoding="utf-8").splitlines() if line.strip()]
matches = [item["payload"] for item in events if item.get("event") == "integration.apply.ok"]
assert matches, "missing integration.apply.ok"
apply = matches[-1]
diagnostic_matches = [item["payload"] for item in events if item.get("event") == "integration.system_diagnostics"]
assert diagnostic_matches, "missing integration.system_diagnostics"
diagnostics = diagnostic_matches[-1]
assert diagnostics.get("platform") == "macos", diagnostics
default_handlers = diagnostics.get("default_handlers", {})
handlers = default_handlers.get("handlers", [])
assert default_handlers.get("total") == 23, default_handlers
assert len(handlers) == 23, default_handlers
assert 0 <= default_handlers.get("checked", -1) <= 23, default_handlers
assert 0 <= default_handlers.get("squallz", -1) <= 23, default_handlers
assert default_handlers.get("state") in {"squallz", "mixed", "other", "unknown", "unavailable"}, default_handlers
for handler in handlers:
    assert handler.get("state") in {"squallz", "other", "unknown"}, handler
    application_name = handler.get("application_name")
    assert application_name is None or ("/" not in application_name and "\\" not in application_name), handler
visibility = diagnostics.get("file_manager_visibility", {})
assert visibility.get("state") == "manual_check", visibility
assert visibility.get("reason") == "not_exposed_by_platform", visibility
assert apply["platform"] == "macos", apply
installed = apply.get("installed", [])
assert len(installed) == 5, apply
for item in installed:
    script = pathlib.Path(item["script_path"])
    workflow = pathlib.Path(item["path"])
    assert str(script).startswith(str(home)), script
    assert str(workflow).startswith(str(home)), workflow
    assert script.is_file(), script
    body = script.read_text(encoding="utf-8")
    assert "Contents/MacOS/sqz" in body, script
    assert str(app) in body, f"script did not capture packaged app path: {script}"
    print(f"{item['id']}\t{script}\t{workflow}")
PY
)

declare -A SCRIPT_FOR=()
declare -A WORKFLOW_FOR=()
for action in "${ACTIONS[@]}"; do
  IFS=$'\t' read -r id script_path workflow_path <<<"$action"
  SCRIPT_FOR["$id"]="$script_path"
  WORKFLOW_FOR["$id"]="$workflow_path"
done

for id in checksum extract-here extract-to-folder compress-to-7z test-archive; do
  [[ -n "${SCRIPT_FOR[$id]:-}" ]] || fail "missing generated script for $id"
  [[ -n "${WORKFLOW_FOR[$id]:-}" ]] || fail "missing generated workflow for $id"
  /bin/zsh -n "${SCRIPT_FOR[$id]}"
  /usr/bin/plutil -lint "${WORKFLOW_FOR[$id]}/Contents/Info.plist" "${WORKFLOW_FOR[$id]}/Contents/document.wflow" >/dev/null
done

kill "$APP_PID" >/dev/null 2>&1 || true
for _ in {1..50}; do
  kill -0 "$APP_PID" >/dev/null 2>&1 || break
  sleep 0.05
done
APP_PID=""

run_workflow() {
  local workflow="$1"
  shift
  env -u SQUALLZ_CLI \
    -u SQUALLZ_APP_BUNDLE \
    HOME="$HOME_DIR" \
    SQUALLZ_DISABLE_GUI_HANDOFF=1 \
    PATH="$FAKE_BIN:/usr/bin:/bin:/usr/sbin:/sbin" \
    /usr/bin/automator -i "$1" "$workflow" >/dev/null
}

run_workflow "${WORKFLOW_FOR[checksum]}" "$FIXTURE/source/a.txt"
run_workflow "${WORKFLOW_FOR[test-archive]}" "$FIXTURE/source.zip"
run_workflow "${WORKFLOW_FOR[extract-here]}" "$FIXTURE/here-case/here.zip"
run_workflow "${WORKFLOW_FOR[extract-to-folder]}" "$FIXTURE/folder-case/folder.zip"
run_workflow "${WORKFLOW_FOR[compress-to-7z]}" "$FIXTURE/compress-case/source"

wait_for_file "$FIXTURE/here-case/source/a.txt" "extract-here did not create here-case/source/a.txt"
wait_for_file "$FIXTURE/folder-case/folder/source/a.txt" "extract-to-folder did not create folder-case/folder/source/a.txt"
wait_for_archive "$FIXTURE/compress-case/source.7z"
run_workflow "${WORKFLOW_FOR[test-archive]}" "$FIXTURE/compress-case/source.7z"

TRACE_SUMMARY="$(python3 - "$TRACE" <<'PY'
import json
import sys
for line in open(sys.argv[1], encoding="utf-8"):
    item = json.loads(line)
    payload = item["payload"]
    if item["event"] == "integration.apply.ok":
        detail = f"installed={len(payload.get('installed', []))} script_dir={payload.get('script_dir')}"
    elif item["event"] == "integration.status.after_apply":
        detail = f"installed={len(payload.get('installed', []))} missing={len(payload.get('missing', []))}"
    elif item["event"] == "integration.system_diagnostics":
        handlers = payload.get("default_handlers", {})
        detail = f"default_handlers={handlers.get('squallz', 0)}/{handlers.get('total', 0)} state={handlers.get('state')} finder={payload.get('file_manager_visibility', {}).get('state')}"
    else:
        detail = str(payload)
    print(f"- +{item.get('process_ms', '?')}ms {item['event']}: {detail}")
PY
)"

cat > "$REPORT" <<EOF
# Squallz macOS Packaged Quick Actions Smoke

Generated: $(date -u +"%Y-%m-%dT%H:%M:%SZ")

## Scope

This smoke check launches the packaged macOS app through LaunchServices with an
isolated HOME, lets the real Tauri backend install Finder Quick Actions, then
runs the generated \`.workflow\` bundles through \`/usr/bin/automator\`. It sets
the workflow's supported GUI-handoff test switch, forcing every workflow to
resolve the first-party
\`Contents/MacOS/sqz\` helper and perform real archive operations without
a developer PATH.

## Inputs

- App: \`$APP\`
- Bundled sqz: \`$SQZ_HELPER\`
- Isolated HOME: \`$HOME_DIR\`
- Trace: \`$TRACE\`
- Fixture: \`$FIXTURE\`
- Fake PATH sqz: \`$FAKE_BIN/sqz\`

## Checks

- \`squallz-gui\` starts from the bundle.
- \`integration.apply.ok\` installs exactly five Finder actions into isolated HOME.
- \`integration.system_diagnostics\` reports all 23 declared file types without
  exposing application paths and keeps Finder visibility as a manual check.
- Every generated script captures the packaged app path and contains the
  \`Contents/MacOS/sqz\` resolver candidate.
- Scripts pass \`/bin/zsh -n\`.
- Workflow plists pass \`plutil -lint\`.
- Workflows run through \`/usr/bin/automator\` with \`SQUALLZ_CLI\` and
  \`SQUALLZ_APP_BUNDLE\` unset and a
  failing fake \`sqz\` first on \`PATH\`. GUI handoff is disabled, so Automator's
  exit status is the bundled CLI action's terminal status.
- \`Squallz Checksum\` completes against a real file.
- \`Squallz Test Archive\` succeeds on a ZIP made for this smoke.
- \`Squallz Extract Here\` extracts the ZIP next to the archive.
- \`Squallz Extract to Folder\` extracts into the derived folder.
- \`Squallz Compress to 7Z\` creates a real \`.7z\` archive through bundled \`sqz\`.
- \`Squallz Test Archive\` succeeds on the generated \`.7z\`.

## Trace Summary

$TRACE_SUMMARY

## Boundary

This smoke executes the packaged Quick Action workflows through Automator but
does not click the Finder context menu. A visible desktop Finder menu test is
still required for final macOS UX sign-off.
EOF

echo "report=$REPORT"
echo "trace=$TRACE"
echo "status=pass"
