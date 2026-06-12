#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP="${1:-"$ROOT/target/debug/bundle/macos/Squallz.app"}"
if [[ "$APP" != /* ]]; then
  APP="$ROOT/$APP"
fi
EXE="$APP/Contents/MacOS/squallz-gui"
WORK="$ROOT/target/squallz-macos-multi-open-smoke"
HOME_DIR="$WORK/home"
TRACE="$WORK/trace.jsonl"
WINDOW_JSON="$WORK/window.json"
REPORT="$ROOT/benches/MACOS_MULTI_OPEN_SMOKE.md"
ARCHIVE_A="$WORK/client-data.tar.gz"
ARCHIVE_B="$WORK/photos-2024.zip"
ARCHIVE_C="$WORK/vendor-assets.7z"

fail() {
  echo "macos_multi_open_smoke: $*" >&2
  exit 1
}

if [[ "$(uname -s)" != "Darwin" ]]; then
  fail "this smoke check only runs on macOS"
fi
if [[ ! -x "$EXE" ]]; then
  fail "missing app executable: $EXE; run 'make app-debug' first, or pass a release app built with 'make app-macos'"
fi
if pgrep -x squallz-gui >/dev/null; then
  fail "squallz-gui is already running; close it before running native smoke"
fi

mkdir -p "$WORK" "$ROOT/benches" "$HOME_DIR/Library/Application Support/Squallz"
rm -f "$TRACE" "$WINDOW_JSON" "$REPORT"

cat > "$HOME_DIR/Library/Application Support/Squallz/settings.json" <<'JSON'
{
  "theme": "light",
  "language": "en-US",
  "ui_mode": "modern"
}
JSON

python3 - "$WORK" "$ARCHIVE_A" "$ARCHIVE_B" "$ARCHIVE_C" <<'PY'
import pathlib
import sys
import tarfile
import zipfile

work = pathlib.Path(sys.argv[1])
archive_a = pathlib.Path(sys.argv[2])
archive_b = pathlib.Path(sys.argv[3])
archive_c = pathlib.Path(sys.argv[4])
src = work / "src"
src.mkdir(parents=True, exist_ok=True)
(src / "hello.txt").write_text("hello from multi-open smoke\n", encoding="utf-8")
(src / "docs").mkdir(exist_ok=True)
(src / "docs" / "readme.md").write_text("# Multi Open\n", encoding="utf-8")

with tarfile.open(archive_a, "w:gz") as t:
    t.add(src / "hello.txt", arcname="client-data/hello.txt")
    t.add(src / "docs" / "readme.md", arcname="client-data/docs/readme.md")

with zipfile.ZipFile(archive_b, "w", compression=zipfile.ZIP_DEFLATED) as z:
    z.writestr("photos-2024/cover.txt", "zip placeholder\n")

with zipfile.ZipFile(archive_c, "w", compression=zipfile.ZIP_DEFLATED) as z:
    z.writestr("vendor-assets/logo.txt", "7z-named zip placeholder\n")
PY

cleanup() {
  if [[ -n "${APP_PID:-}" ]]; then
    kill "$APP_PID" >/dev/null 2>&1 || true
  fi
}
trap cleanup EXIT

open -n -a "$APP" \
  --env "HOME=$HOME_DIR" \
  --env "SQUALLZ_VALIDATION_TRACE=$TRACE" \
  "$ARCHIVE_A" "$ARCHIVE_B" "$ARCHIVE_C"

APP_PID=""
for _ in {1..50}; do
  APP_PID="$(pgrep -n -x squallz-gui || true)"
  if [[ -n "$APP_PID" ]]; then
    break
  fi
  sleep 0.1
done
[[ -n "$APP_PID" ]] || fail "app process did not start"

for _ in {1..100}; do
  if grep -q '"event":"open_archive.ok"' "$TRACE" 2>/dev/null; then
    break
  fi
  sleep 0.1
done
grep -q '"event":"open_archive.ok"' "$TRACE" 2>/dev/null || {
  [[ -f "$TRACE" ]] && cat "$TRACE" >&2
  fail "frontend did not open first archive successfully"
}

python3 - "$TRACE" "$ARCHIVE_A" "$ARCHIVE_B" "$ARCHIVE_C" <<'PY'
import json
import sys

trace, a, b, c = sys.argv[1:]
expected = [a, b, c]
events = [json.loads(line) for line in open(trace, encoding="utf-8")]

pushes = [item["payload"].get("paths", []) for item in events if item.get("event") == "open_files.push"]
takes = [item["payload"].get("paths", []) for item in events if item.get("event") == "open_files.take"]
opened = [item["payload"] for item in events if item.get("event") == "open_archive.ok"]
focus = [item["payload"] for item in events if item.get("event") == "window.focus"]

assert any(paths == expected for paths in pushes), pushes
assert any(paths == expected for paths in takes), takes
assert opened and opened[-1].get("path") == a, opened
assert opened[-1].get("entry_count", 0) >= 2, opened[-1]
assert any(item.get("found") and item.get("show_ok") and item.get("focus_ok") for item in focus), focus
PY

for _ in {1..50}; do
  swift - "$APP_PID" "$WINDOW_JSON" <<'SWIFT' >/dev/null 2>&1 || true
import CoreGraphics
import Foundation

let pid = Int32(CommandLine.arguments[1])!
let output = CommandLine.arguments[2]
let options = CGWindowListOption(arrayLiteral: .optionAll, .excludeDesktopElements)
guard let windows = CGWindowListCopyWindowInfo(options, kCGNullWindowID) as? [[String: Any]] else {
  exit(1)
}
for w in windows {
  let ownerPid = w[kCGWindowOwnerPID as String] as? Int32
  let layer = w[kCGWindowLayer as String] as? Int ?? -1
  guard ownerPid == pid, layer == 0 else { continue }
  guard let bounds = w[kCGWindowBounds as String] as? [String: Any] else { continue }
  let width = bounds["Width"] as? Double ?? 0
  let height = bounds["Height"] as? Double ?? 0
  guard width >= 600, height >= 400 else { continue }
  let payload: [String: Any] = [
    "owner": w[kCGWindowOwnerName as String] as? String ?? "",
    "name": w[kCGWindowName as String] as? String ?? "",
    "pid": Int(pid),
    "onscreen": w[kCGWindowIsOnscreen as String] as? Bool ?? false,
    "width": width,
    "height": height,
    "x": bounds["X"] as? Double ?? 0,
    "y": bounds["Y"] as? Double ?? 0,
  ]
  let data = try! JSONSerialization.data(withJSONObject: payload, options: [.prettyPrinted, .sortedKeys])
  try! data.write(to: URL(fileURLWithPath: output))
  exit(0)
}
exit(1)
SWIFT
  if [[ -s "$WINDOW_JSON" ]]; then
    break
  fi
  sleep 0.2
done
[[ -s "$WINDOW_JSON" ]] || fail "no app window found for pid $APP_PID"

TRACE_SUMMARY="$(python3 - "$TRACE" <<'PY'
import json
import sys
for line in open(sys.argv[1], encoding="utf-8"):
    item = json.loads(line)
    print(f"- {item['event']}: {item['payload']}")
PY
)"
WINDOW_SUMMARY="$(cat "$WINDOW_JSON")"

cat > "$REPORT" <<EOF
# Squallz macOS Multi-Open App Smoke

Generated: $(date -u +"%Y-%m-%dT%H:%M:%SZ")

## Scope

This smoke check launches the supplied macOS bundle through LaunchServices with
three archive file arguments, isolated HOME, and a temporary trace file. It
verifies the packaged app receives and drains all opened paths before opening
the first archive.

## Inputs

- App: \`$APP\`
- Archives:
  - \`$ARCHIVE_A\`
  - \`$ARCHIVE_B\`
  - \`$ARCHIVE_C\`
- Isolated HOME: \`$HOME_DIR\`
- Trace: \`$TRACE\`

## Checks

- \`squallz-gui\` process starts from the bundle.
- \`open_files.push\` records the three LaunchServices paths in order.
- \`open_files.take\` drains the same three paths after frontend startup.
- Frontend opens the first archive successfully via \`open_archive.ok\`.
- Main window show/focus succeeds and a layer-0 app window is present.

## Trace

$TRACE_SUMMARY

## Window

\`\`\`json
$WINDOW_SUMMARY
\`\`\`
EOF

echo "report=$REPORT"
echo "trace=$TRACE"
echo "window=$WINDOW_JSON"
