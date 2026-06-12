#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP="${1:-"$ROOT/target/debug/bundle/macos/Squallz.app"}"
if [[ "$APP" != /* ]]; then
  APP="$ROOT/$APP"
fi
EXE="$APP/Contents/MacOS/squallz-gui"
WORK="$ROOT/target/squallz-macos-smoke"
HOME_DIR="$WORK/home"
TRACE="$WORK/trace.jsonl"
ARCHIVE_EXT="${SQUALLZ_VALIDATION_ARCHIVE_EXT:-zip}"
DEFAULT_ARCHIVE="$WORK/space 中文 #1.$ARCHIVE_EXT"
ARCHIVE="${SQUALLZ_VALIDATION_ARCHIVE:-$DEFAULT_ARCHIVE}"
if [[ "$ARCHIVE" != /* ]]; then
  ARCHIVE="$ROOT/$ARCHIVE"
fi
WINDOW_JSON="$WORK/window.json"
TIMING_JSON="$WORK/timing.json"
I14_MAX_PROCESS_MS="${SQUALLZ_I14_MAX_PROCESS_MS:-300}"
I14_MAX_OPEN_ARCHIVE_MS="${SQUALLZ_I14_MAX_OPEN_ARCHIVE_MS:-1000}"
I14_MAX_RENDER_READY_MS="${SQUALLZ_I14_MAX_RENDER_READY_MS:-$I14_MAX_OPEN_ARCHIVE_MS}"
I14_MAX_WINDOW_MS="${SQUALLZ_I14_MAX_WINDOW_MS:-}"
if [[ -n "${SQUALLZ_VALIDATION_REPORT:-}" ]]; then
  REPORT="$SQUALLZ_VALIDATION_REPORT"
elif [[ "$ARCHIVE_EXT" == "zip" ]]; then
  REPORT="$ROOT/benches/MACOS_APP_SMOKE.md"
else
  REPORT="$ROOT/benches/MACOS_APP_SMOKE_$(printf '%s' "$ARCHIVE_EXT" | tr '[:lower:]' '[:upper:]').md"
fi
if [[ "$REPORT" != /* ]]; then
  REPORT="$ROOT/$REPORT"
fi

fail() {
  echo "macos_app_smoke: $*" >&2
  exit 1
}

now_ms() {
  python3 - <<'PY'
import time
print(time.time_ns() // 1_000_000)
PY
}

if [[ ! -x "$EXE" ]]; then
  fail "missing app executable: $EXE; run 'make app-debug' first, or pass a release app built with 'make app-macos'"
fi

mkdir -p "$WORK" "$ROOT/benches" "$HOME_DIR/Library/Application Support/Squallz"
rm -f "$TRACE" "$WINDOW_JSON" "$TIMING_JSON"

python3 - "$APP/Contents/Info.plist" <<'PY'
import plistlib, sys
path = sys.argv[1]
with open(path, "rb") as f:
    p = plistlib.load(f)
assert p["CFBundleExecutable"] == "squallz-gui", p.get("CFBundleExecutable")
assert p["CFBundleIdentifier"] == "dev.squallz.desktop", p.get("CFBundleIdentifier")
types = p.get("CFBundleDocumentTypes", [])
exts = {e for t in types for e in t.get("CFBundleTypeExtensions", [])}
for ext in ["zip", "7z", "sqz", "tar", "tgz", "gz", "xz", "zst", "br"]:
    assert ext in exts, f"missing {ext}"
for t in types:
    assert t.get("CFBundleTypeRole") == "Viewer", t
    assert t.get("LSHandlerRank") == "Alternate", t
PY

cat > "$HOME_DIR/Library/Application Support/Squallz/settings.json" <<'JSON'
{
  "theme": "light",
  "language": "en-US",
  "ui_mode": "modern"
}
JSON

if [[ -n "${SQUALLZ_VALIDATION_ARCHIVE:-}" ]]; then
  [[ -f "$ARCHIVE" ]] || fail "missing validation archive: $ARCHIVE"
elif [[ "$ARCHIVE_EXT" == "sqz" ]]; then
  SRC="$WORK/sqz-src"
  rm -rf "$SRC"
  mkdir -p "$SRC/src/docs"
  printf 'hello from native sqz app validation\n' > "$SRC/src/hello.txt"
  printf '# SQZ Validation\n' > "$SRC/src/docs/readme.md"
  if [[ ! -x "$ROOT/target/debug/sqz" ]]; then
    (cd "$ROOT" && cargo build -p squallz-cli)
  fi
  "$ROOT/target/debug/sqz" --lang en-US compress "$SRC" -o "$ARCHIVE" >/dev/null
else
  python3 - "$ARCHIVE" <<'PY'
import pathlib, sys, zipfile
archive = pathlib.Path(sys.argv[1])
archive.parent.mkdir(parents=True, exist_ok=True)
with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_DEFLATED) as z:
    z.writestr("src/hello.txt", "hello from native app validation\n")
    z.writestr("src/docs/readme.md", "# Validation\n")
PY
fi

if pgrep -x squallz-gui >/dev/null; then
  fail "squallz-gui is already running; close it before running native validation"
fi

cleanup() {
  if [[ -n "${APP_PID:-}" ]]; then
    kill "$APP_PID" >/dev/null 2>&1 || true
  fi
}
trap cleanup EXIT

LAUNCH_MS="$(now_ms)"
APP_PID_MS=""
OPEN_OK_MS=""
WINDOW_MS=""

open -n -a "$APP" \
  --env "HOME=$HOME_DIR" \
  --env "SQUALLZ_VALIDATION_TRACE=$TRACE" \
  "$ARCHIVE"

APP_PID=""
for _ in {1..50}; do
  APP_PID="$(pgrep -n -x squallz-gui || true)"
  if [[ -n "$APP_PID" ]]; then
    APP_PID_MS="$(now_ms)"
    break
  fi
  sleep 0.1
done
[[ -n "$APP_PID" ]] || fail "app process did not start"

for _ in {1..80}; do
  if grep -q '"event":"open_archive.ok"' "$TRACE" 2>/dev/null; then
    OPEN_OK_MS="$(now_ms)"
    break
  fi
  sleep 0.1
done
grep -q '"event":"open_archive.ok"' "$TRACE" 2>/dev/null || {
  [[ -f "$TRACE" ]] && cat "$TRACE" >&2
  fail "frontend did not open archive successfully"
}

python3 - "$TRACE" <<'PY'
import json, sys
focus_ok = False
for line in open(sys.argv[1], encoding="utf-8"):
    item = json.loads(line)
    if item.get("event") != "window.focus":
        continue
    payload = item.get("payload", {})
    if payload.get("found") and payload.get("show_ok") and payload.get("focus_ok"):
        focus_ok = True
        break
assert focus_ok, "main window was not shown and focused"
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
  let onscreen = w[kCGWindowIsOnscreen as String] as? Bool ?? false
  let payload: [String: Any] = [
    "owner": w[kCGWindowOwnerName as String] as? String ?? "",
    "name": w[kCGWindowName as String] as? String ?? "",
    "pid": Int(pid),
    "onscreen": onscreen,
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
    WINDOW_MS="$(now_ms)"
    break
  fi
  sleep 0.2
done
[[ -s "$WINDOW_JSON" ]] || fail "no app window found for pid $APP_PID"

TRACE_SUMMARY="$(python3 - "$TRACE" <<'PY'
import json, sys
for line in open(sys.argv[1], encoding="utf-8"):
    item = json.loads(line)
    process_ms = item.get("process_ms", "?")
    print(f"- +{process_ms}ms {item['event']}: {item['payload']}")
PY
)"
TIMING_SUMMARY="$(python3 - "$TRACE" "$TIMING_JSON" "$LAUNCH_MS" "$APP_PID_MS" "$OPEN_OK_MS" "$WINDOW_MS" "$ARCHIVE" "$I14_MAX_PROCESS_MS" "$I14_MAX_OPEN_ARCHIVE_MS" "$I14_MAX_RENDER_READY_MS" "$I14_MAX_WINDOW_MS" <<'PY'
import json
import sys
from pathlib import Path

trace_path, timing_json, launch_s, pid_s, open_s, window_s, archive_path, max_process_s, max_open_s, max_render_s, max_window_s = sys.argv[1:]

def as_int(value):
    try:
        if value == "":
            return None
        return int(value)
    except (TypeError, ValueError):
        return None

launch_ms = as_int(launch_s)
max_process_ms = as_int(max_process_s)
max_open_ms = as_int(max_open_s)
max_render_ms = as_int(max_render_s)
max_window_ms = as_int(max_window_s)

events = []
if Path(trace_path).exists():
    with open(trace_path, encoding="utf-8") as f:
        events = [json.loads(line) for line in f if line.strip()]

def delta_from_launch(unix_ms):
    value = as_int(unix_ms)
    if value is None or launch_ms is None:
        return None
    return max(0, value - launch_ms)

def shell_delta(value):
    value = as_int(value)
    if value is None or launch_ms is None:
        return None
    return max(0, value - launch_ms)

def first_event_delta(event_name, predicate=lambda payload: True):
    for item in events:
        if item.get("event") != event_name:
            continue
        payload = item.get("payload") or {}
        if predicate(payload):
            return delta_from_launch(item.get("unix_ms"))
    return None

def first_event_payload(event_name):
    for item in events:
        if item.get("event") == event_name:
            return item.get("payload") or {}
    return None

expected_archive = Path(archive_path).name
render_payload = first_event_payload("frontend.render.ready")

content_checks = []

def add_content_check(name, passed, detail):
    content_checks.append({
        "name": name,
        "status": "pass" if passed else "fail",
        "detail": detail,
    })

if render_payload is None:
    add_content_check("render payload present", False, "frontend.render.ready event is missing")
else:
    text_sample = str(render_payload.get("text_sample") or "")
    archive_name = render_payload.get("archive")
    entry_count = render_payload.get("entry_count")
    viewport_width = render_payload.get("viewport_width")
    viewport_height = render_payload.get("viewport_height")
    add_content_check(
        "reason",
        render_payload.get("reason") == "archive-open:open-file",
        f"reason={render_payload.get('reason')!r}",
    )
    add_content_check("screen", render_payload.get("screen") == "browse", f"screen={render_payload.get('screen')!r}")
    add_content_check("ui mode", render_payload.get("ui_mode") == "modern", f"ui_mode={render_payload.get('ui_mode')!r}")
    add_content_check("archive name", archive_name == expected_archive, f"archive={archive_name!r}; expected={expected_archive!r}")
    add_content_check(
        "entry count",
        isinstance(entry_count, int) and entry_count >= 2,
        f"entry_count={entry_count!r}",
    )
    add_content_check(
        "viewport",
        isinstance(viewport_width, int)
        and isinstance(viewport_height, int)
        and viewport_width >= 600
        and viewport_height >= 400,
        f"viewport={viewport_width!r}x{viewport_height!r}",
    )
    for token in ["Squallz", expected_archive, "entries"]:
        add_content_check(
            f"text token {token}",
            token in text_sample,
            f"text_sample contains {token!r}",
        )
    for forbidden in ["Choose your interface", "Archive open failed", "needs a password", "Open an archive first"]:
        add_content_check(
            f"no {forbidden}",
            forbidden not in text_sample,
            f"text_sample excludes {forbidden!r}",
        )

content_failures = [item for item in content_checks if item["status"] != "pass"]
content_delta = first_event_delta("frontend.render.ready") if not content_failures else None
content_status = "pass" if render_payload is not None and not content_failures else "fail"

first_trace = min(
    (delta_from_launch(item.get("unix_ms")) for item in events),
    default=None,
)
first_trace = None if first_trace is None else first_trace
archive_delta = first_event_delta("open_archive.ok")
if archive_delta is None:
    archive_delta = shell_delta(open_s)

metrics = [
    {
        "name": "Process observed",
        "ms": shell_delta(pid_s),
        "target_ms": max_process_ms,
        "source": "pgrep after LaunchServices open",
    },
    {
        "name": "First validation trace",
        "ms": first_trace,
        "target_ms": None,
        "source": "trace unix_ms",
    },
    {
        "name": "Open files queued",
        "ms": first_event_delta("open_files.push"),
        "target_ms": None,
        "source": "open_files.push",
    },
    {
        "name": "Frontend took open files",
        "ms": first_event_delta("open_files.take"),
        "target_ms": None,
        "source": "open_files.take",
    },
    {
        "name": "Main window focus ok",
        "ms": first_event_delta(
            "window.focus",
            lambda payload: bool(payload.get("found"))
            and bool(payload.get("show_ok"))
            and bool(payload.get("focus_ok")),
        ),
        "target_ms": None,
        "source": "window.focus",
    },
    {
        "name": "Archive opened",
        "ms": archive_delta,
        "target_ms": max_open_ms,
        "source": "open_archive.ok",
    },
    {
        "name": "Frontend render ready",
        "ms": first_event_delta("frontend.render.ready"),
        "target_ms": max_render_ms,
        "source": "frontend.render.ready",
    },
    {
        "name": "Frontend content ready",
        "ms": content_delta,
        "target_ms": max_render_ms,
        "source": "frontend.render.ready payload",
    },
    {
        "name": "CoreGraphics window found",
        "ms": shell_delta(window_s),
        "target_ms": max_window_ms,
        "source": "CGWindowListCopyWindowInfo",
    },
]

def metric_status(metric):
    if metric["ms"] is None:
        return "missing" if metric["target_ms"] is None else "fail"
    if metric["target_ms"] is None:
        return "recorded"
    return "pass" if metric["ms"] <= metric["target_ms"] else "fail"

overall = "pass"
for metric in metrics:
    metric["status"] = metric_status(metric)
    if metric["target_ms"] is not None and metric["status"] != "pass":
        overall = "fail"

Path(timing_json).write_text(
    json.dumps(
        {
            "status": overall,
            "thresholds": {
                "process_observed_ms": max_process_ms,
                "open_archive_ms": max_open_ms,
                "render_ready_ms": max_render_ms,
                "window_found_ms": max_window_ms,
            },
            "metrics": metrics,
            "first_frame_content": {
                "status": content_status,
                "expected_archive": expected_archive,
                "checks": content_checks,
                "payload": render_payload,
            },
        },
        ensure_ascii=False,
        indent=2,
    )
    + "\n",
    encoding="utf-8",
)

print(
    "| Metric | Actual | Target | Status | Source |\n"
    "| ---- | ----: | ----: | ---- | ---- |"
)
for metric in metrics:
    actual = "n/a" if metric["ms"] is None else f"{metric['ms']} ms"
    target = "record" if metric["target_ms"] is None else f"{metric['target_ms']} ms"
    print(
        f"| {metric['name']} | {actual} | {target} | {metric['status']} | {metric['source']} |"
    )
print(f"\nOverall timing gate: **{overall.upper()}**")
print("\n## First Frame Content\n")
print(f"- Expected archive: `{expected_archive}`")
print(f"- Content status: **{content_status}**")
print("")
print("| Check | Status | Detail |")
print("| ---- | ---- | ---- |")
for check in content_checks:
    print(f"| {check['name']} | {check['status']} | {check['detail']} |")
PY
)"
WINDOW_SUMMARY="$(cat "$WINDOW_JSON")"
SMOKE_STATUS="$(
python3 - "$TIMING_JSON" <<'PY'
import json
import sys

status = "unknown"
try:
    with open(sys.argv[1], encoding="utf-8") as f:
        timing = json.load(f)
    candidate = timing.get("status")
    if isinstance(candidate, str) and candidate:
        status = candidate
except (OSError, json.JSONDecodeError):
    pass

print(status)
PY
)"

cat > "$REPORT" <<EOF
# Squallz macOS Native App Smoke

Generated: $(date -u +"%Y-%m-%dT%H:%M:%SZ")

Status: $SMOKE_STATUS

## Scope

This validation check launches the macOS bundle through LaunchServices with an
archive file argument, isolated HOME, and a temporary trace file.

## Inputs

- App: \`$APP\`
- Archive: \`$ARCHIVE\`
- Isolated HOME: \`$HOME_DIR\`
- Trace: \`$TRACE\`
- Timing JSON: \`$TIMING_JSON\`

## Checks

- Info.plist document types include archive/stream extensions and use Viewer + Alternate.
- \`squallz-gui\` process starts from the bundle.
- Frontend drains the OS open-file path and calls \`open_archive\` successfully.
- First WebView content frame is the archive browser for the opened file, not
  the first-run chooser, an empty shell, a password/error page, or a generic
  "open an archive first" state.
- Main window show/focus succeeds, and a layer-0 app window with desktop size is present.
- The report records CoreGraphics on-screen state; CI/automation sessions may keep a launched app in a non-active Space.
- Real user \`~/Library/Application Support/Squallz/settings.json\` is not used.

## Timing Gate

$TIMING_SUMMARY

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
echo "timing=$TIMING_JSON"

if python3 - "$TIMING_JSON" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as f:
    timing = json.load(f)
sys.exit(0 if timing.get("status") == "pass" else 1)
PY
then
  :
else
  fail "timing gate failed; see $TIMING_JSON"
fi
