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
WORK="$ROOT/target/squallz-native-screenshots"
OUT_DIR="$ROOT/benches/screenshots/i9-native"
REPORT="$ROOT/benches/MACOS_NATIVE_SCREENSHOTS.md"
ARCHIVE="$WORK/workspace-documents.zip"

write_blocked_report() {
  local reason="$1"
  mkdir -p "$ROOT/benches"
  cat > "$REPORT" <<EOF
# Squallz macOS Native Window Screenshots

Generated: $(date -u +"%Y-%m-%dT%H:%M:%SZ")

Status: blocked

## Scope

This check is intended to launch the macOS bundle twice with isolated HOME
directories, once in Modern mode and once in Classic mode, wait for backend
\`open_archive.ok\` plus frontend \`frontend.render.ready\`, then capture the
layer-0 native app window as PNG evidence. Captures must contain visible
content-area pixels below the native titlebar; titlebar/traffic-light pixels
alone are rejected.

## Result

Blocked in the current automation desktop session.

Reason: $reason

## Inputs

- App: \`$APP\`
- Archive: \`$ARCHIVE\`
- Output directory: \`$OUT_DIR\`

## Available Evidence

- Desktop permission diagnostics, if created: \`$ROOT/benches/MACOS_DESKTOP_PERMISSION_DIAGNOSTICS.md\`
- Modern trace/window metadata, if created: \`$WORK/modern-trace.jsonl\`, \`$WORK/modern-window.json\`
- Classic trace/window metadata, if created: \`$WORK/classic-trace.jsonl\`, \`$WORK/classic-window.json\`

## Next Step

Rerun \`scripts/macos_native_screenshots.sh\` from an interactive macOS desktop
session where Squallz can become visible in the active Space, or migrate this
check to ScreenCaptureKit with the required session permissions.
EOF
  if [[ -s "$ROOT/benches/MACOS_DESKTOP_PERMISSION_DIAGNOSTICS.md" ]]; then
    {
      echo
      echo "## Desktop Permission Diagnostics Summary"
      echo
      sed -n '1,90p' "$ROOT/benches/MACOS_DESKTOP_PERMISSION_DIAGNOSTICS.md" | sed 's/^Status:/Diagnostic status:/'
    } >>"$REPORT"
  fi
  for mode in modern classic; do
    local label="$mode"
    if [[ "$mode" == "modern" ]]; then
      label="Modern"
    elif [[ "$mode" == "classic" ]]; then
      label="Classic"
    fi
    local trace="$WORK/$mode-trace.jsonl"
    local window_json="$WORK/$mode-window.json"
    local capture_method="$WORK/$mode-capture-method.txt"
    local image_diag="$WORK/$mode-image-diagnostics.json"
    local rejected_image="$WORK/$mode-rejected.png"
    if [[ -s "$window_json" ]]; then
      {
        echo
        echo "## $label Window Metadata"
        echo
        echo '```json'
        cat "$window_json"
        echo
        echo '```'
      } >>"$REPORT"
    fi
    if [[ -s "$image_diag" ]]; then
      {
        echo
        echo "## $label Image Diagnostics"
        echo
        echo '```json'
        cat "$image_diag"
        echo
        echo '```'
      } >>"$REPORT"
    fi
    if [[ -s "$capture_method" ]]; then
      {
        echo
        echo "## $label Capture Method"
        echo
        echo '```text'
        cat "$capture_method"
        echo
        echo '```'
      } >>"$REPORT"
    fi
    if [[ -s "$rejected_image" ]]; then
      {
        echo
        echo "## $label Rejected Capture"
        echo
        echo "- Diagnostic PNG: \`$rejected_image\`"
        echo "- This file is intentionally kept under \`target/\`, not \`benches/screenshots/\`, because it failed the visible-pixel gate."
      } >>"$REPORT"
    fi
    if [[ -s "$trace" ]]; then
      {
        echo
        echo "## $label Trace Tail"
        echo
        echo '```jsonl'
        tail -n 12 "$trace"
        echo '```'
      } >>"$REPORT"
    fi
  done
}

fail() {
  write_blocked_report "$*"
  echo "macos_native_screenshots: $*" >&2
  exit 1
}

screenshot_blockers=()

mode_blocked() {
  local reason="$1"
  screenshot_blockers+=("$reason")
  echo "macos_native_screenshots: $reason" >&2
  return 1
}

if [[ "$(uname -s)" != "Darwin" ]]; then
  fail "this screenshot check only runs on macOS"
fi
if [[ ! -x "$EXE" ]]; then
  fail "missing app executable: $EXE; run 'make app-debug' first, or pass a release app built with 'make app-macos'"
fi
if pgrep -x squallz-gui >/dev/null; then
  fail "squallz-gui is already running; close it before running native screenshots"
fi

mkdir -p "$WORK" "$OUT_DIR" "$ROOT/benches"
rm -f "$OUT_DIR"/modern.png "$OUT_DIR"/classic.png "$REPORT"
rm -f \
  "$WORK"/modern-trace.jsonl \
  "$WORK"/modern-window.json \
  "$WORK"/modern-capture-method.txt \
  "$WORK"/modern-image-diagnostics.json \
  "$WORK"/modern-rejected.png \
  "$WORK"/classic-trace.jsonl \
  "$WORK"/classic-window.json \
  "$WORK"/classic-capture-method.txt \
  "$WORK"/classic-image-diagnostics.json \
  "$WORK"/classic-rejected.png

python3 - "$ARCHIVE" <<'PY'
import pathlib, sys, zipfile
archive = pathlib.Path(sys.argv[1])
archive.parent.mkdir(parents=True, exist_ok=True)
with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_DEFLATED) as z:
    z.writestr("src/hello.txt", "hello from macOS desktop smoke\n")
    z.writestr("src/docs/readme.md", "# Workspace Documents\n")
    z.writestr("财务报表.xlsx", "spreadsheet placeholder\n")
PY

capture_window_coregraphics() {
  local window_id="$1"
  local output="$2"
  swift -Xfrontend -disable-availability-checking - "$window_id" "$output" <<'SWIFT' >/dev/null
import AppKit
import CoreGraphics
import Foundation

guard let rawId = UInt32(CommandLine.arguments[1]) else {
  exit(2)
}
let windowId = CGWindowID(rawId)
let output = URL(fileURLWithPath: CommandLine.arguments[2])
let options = CGWindowImageOption(arrayLiteral: .boundsIgnoreFraming, .bestResolution)

guard let image = CGWindowListCreateImage(.null, .optionIncludingWindow, windowId, options) else {
  exit(3)
}
let bitmap = NSBitmapImageRep(cgImage: image)
guard let data = bitmap.representation(using: .png, properties: [:]) else {
  exit(4)
}
do {
  try data.write(to: output, options: .atomic)
} catch {
  exit(5)
}
SWIFT
}

capture_window() {
  local window_id="$1"
  local output="$2"
  local method_file="$3"

  rm -f "$output" "$method_file"
  if /usr/sbin/screencapture -x -l "$window_id" "$output" >/dev/null 2>&1 && [[ -s "$output" ]]; then
    echo "screencapture-window-id" >"$method_file"
    return 0
  fi

  rm -f "$output"
  if capture_window_coregraphics "$window_id" "$output" && [[ -s "$output" ]]; then
    echo "coregraphics-window-image-fallback" >"$method_file"
    return 0
  fi

  return 1
}

image_has_visible_pixels() {
  local image="$1"
  python3 - "$image" <<'PY'
import struct
import sys
import zlib

path = sys.argv[1]
data = open(path, "rb").read()
if not data.startswith(b"\x89PNG\r\n\x1a\n"):
    raise SystemExit(2)

pos = 8
width = height = bit_depth = color_type = None
idat = bytearray()
while pos + 8 <= len(data):
    length = struct.unpack(">I", data[pos : pos + 4])[0]
    chunk_type = data[pos + 4 : pos + 8]
    chunk_data = data[pos + 8 : pos + 8 + length]
    pos += 12 + length
    if chunk_type == b"IHDR":
        width, height, bit_depth, color_type, _, _, _ = struct.unpack(">IIBBBBB", chunk_data)
    elif chunk_type == b"IDAT":
        idat.extend(chunk_data)
    elif chunk_type == b"IEND":
        break

if not width or not height or bit_depth != 8 or color_type not in {0, 2, 4, 6}:
    raise SystemExit(3)

channels = {0: 1, 2: 3, 4: 2, 6: 4}[color_type]
bytes_per_pixel = channels
row_bytes = width * channels
raw = zlib.decompress(bytes(idat))

def paeth(a, b, c):
    p = a + b - c
    pa = abs(p - a)
    pb = abs(p - b)
    pc = abs(p - c)
    if pa <= pb and pa <= pc:
        return a
    if pb <= pc:
        return b
    return c

visible = 0
seen = 0
content_top = max(90, height // 12)
prev = [0] * row_bytes
offset = 0
sample_stride_x = max(1, width // 160)
sample_stride_y = max(1, height // 100)
for y in range(height):
    filter_type = raw[offset]
    offset += 1
    scan = list(raw[offset : offset + row_bytes])
    offset += row_bytes
    recon = [0] * row_bytes
    for i, value in enumerate(scan):
        left = recon[i - bytes_per_pixel] if i >= bytes_per_pixel else 0
        up = prev[i]
        up_left = prev[i - bytes_per_pixel] if i >= bytes_per_pixel else 0
        if filter_type == 0:
            out = value
        elif filter_type == 1:
            out = value + left
        elif filter_type == 2:
            out = value + up
        elif filter_type == 3:
            out = value + ((left + up) // 2)
        elif filter_type == 4:
            out = value + paeth(left, up, up_left)
        else:
            raise SystemExit(4)
        recon[i] = out & 0xFF
    prev = recon
    if y < content_top or y % sample_stride_y != 0:
        continue
    for x in range(0, width, sample_stride_x):
        base = x * channels
        if color_type == 0:
            color_values = recon[base : base + 1]
            alpha = 255
        elif color_type == 2:
            color_values = recon[base : base + 3]
            alpha = 255
        elif color_type == 4:
            color_values = recon[base : base + 1]
            alpha = recon[base + 1]
        else:
            color_values = recon[base : base + 3]
            alpha = recon[base + 3]
        seen += 1
        if alpha > 16 and min(color_values) < 245:
            visible += 1

if seen == 0 or visible < max(25, seen // 250):
    raise SystemExit(5)
PY
}

write_image_diagnostics() {
  local image="$1"
  local output="$2"
  python3 - "$image" "$output" <<'PY'
import json
import struct
import sys
import zlib
from pathlib import Path

image = Path(sys.argv[1])
output = Path(sys.argv[2])
payload = {
    "image": str(image),
    "exists": image.is_file(),
    "bytes": image.stat().st_size if image.is_file() else 0,
}

try:
    data = image.read_bytes()
    if not data.startswith(b"\x89PNG\r\n\x1a\n"):
        raise ValueError("not a PNG")

    pos = 8
    width = height = bit_depth = color_type = None
    idat = bytearray()
    while pos + 8 <= len(data):
        length = struct.unpack(">I", data[pos : pos + 4])[0]
        chunk_type = data[pos + 4 : pos + 8]
        chunk_data = data[pos + 8 : pos + 8 + length]
        pos += 12 + length
        if chunk_type == b"IHDR":
            width, height, bit_depth, color_type, _, _, _ = struct.unpack(">IIBBBBB", chunk_data)
        elif chunk_type == b"IDAT":
            idat.extend(chunk_data)
        elif chunk_type == b"IEND":
            break

    payload.update(
        {
            "width": width,
            "height": height,
            "bit_depth": bit_depth,
            "color_type": color_type,
            "idat_bytes": len(idat),
        }
    )
    if not width or not height or bit_depth != 8 or color_type not in {0, 2, 4, 6}:
        raise ValueError("unsupported PNG layout")

    channels = {0: 1, 2: 3, 4: 2, 6: 4}[color_type]
    bytes_per_pixel = channels
    row_bytes = width * channels
    raw = zlib.decompress(bytes(idat))
    payload["raw_bytes"] = len(raw)
    payload["raw_prefix_unique_values"] = len(set(raw[: min(len(raw), 100000)]))

    def paeth(a, b, c):
        p = a + b - c
        pa = abs(p - a)
        pb = abs(p - b)
        pc = abs(p - c)
        if pa <= pb and pa <= pc:
            return a
        if pb <= pc:
            return b
        return c

    visible = 0
    content_visible = 0
    seen = 0
    content_seen = 0
    nonzero = 0
    alpha_visible = 0
    max_channel = 0
    content_top = max(90, height // 12)
    prev = [0] * row_bytes
    offset = 0
    sample_stride_x = max(1, width // 160)
    sample_stride_y = max(1, height // 100)
    for y in range(height):
        filter_type = raw[offset]
        offset += 1
        scan = list(raw[offset : offset + row_bytes])
        offset += row_bytes
        recon = [0] * row_bytes
        for i, value in enumerate(scan):
            left = recon[i - bytes_per_pixel] if i >= bytes_per_pixel else 0
            up = prev[i]
            up_left = prev[i - bytes_per_pixel] if i >= bytes_per_pixel else 0
            if filter_type == 0:
                out = value
            elif filter_type == 1:
                out = value + left
            elif filter_type == 2:
                out = value + up
            elif filter_type == 3:
                out = value + ((left + up) // 2)
            elif filter_type == 4:
                out = value + paeth(left, up, up_left)
            else:
                raise ValueError(f"unsupported PNG filter {filter_type}")
            recon[i] = out & 0xFF
        prev = recon
        if y % sample_stride_y != 0:
            continue
        for x in range(0, width, sample_stride_x):
            base = x * channels
            if color_type == 0:
                color_values = recon[base : base + 1]
                alpha = 255
            elif color_type == 2:
                color_values = recon[base : base + 3]
                alpha = 255
            elif color_type == 4:
                color_values = recon[base : base + 1]
                alpha = recon[base + 1]
            else:
                color_values = recon[base : base + 3]
                alpha = recon[base + 3]
            seen += 1
            max_channel = max(max_channel, max(color_values), alpha)
            if alpha > 16:
                alpha_visible += 1
            if any(value > 0 for value in color_values) or alpha > 0:
                nonzero += 1
            if alpha > 16 and max(color_values) > 16:
                visible += 1
            if y >= content_top:
                content_seen += 1
                if alpha > 16 and min(color_values) < 245:
                    content_visible += 1

    threshold = max(10, seen // 200)
    content_threshold = max(25, content_seen // 250)
    payload.update(
        {
            "sampled_pixels": seen,
            "visible_pixels": visible,
            "visible_threshold": threshold,
            "content_top": content_top,
            "content_sampled_pixels": content_seen,
            "content_visible_pixels": content_visible,
            "content_visible_threshold": content_threshold,
            "nonzero_samples": nonzero,
            "alpha_visible_samples": alpha_visible,
            "max_sample_channel": max_channel,
            "visible_gate": content_seen > 0 and content_visible >= content_threshold,
        }
    )
except Exception as exc:
    payload["error"] = str(exc)

output.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8")
PY
}

find_window() {
  local pid="$1"
  local output="$2"
  swift - "$pid" "$output" <<'SWIFT' >/dev/null
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
  let onscreen = w[kCGWindowIsOnscreen as String] as? Bool
  let payload: [String: Any] = [
    "window_id": w[kCGWindowNumber as String] as? Int ?? 0,
    "owner": w[kCGWindowOwnerName as String] as? String ?? "",
    "name": w[kCGWindowName as String] as? String ?? "",
    "pid": Int(pid),
    "onscreen": onscreen as Any? ?? NSNull(),
    "onscreen_available": onscreen != nil,
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
}

force_window_frontmost() {
  local pid="$1"
  python3 - "$APP" "$pid" <<'PY' >/dev/null 2>&1 || true
import subprocess
import sys

script = r'''
on run argv
  set appPath to item 1 of argv
  set targetPid to (item 2 of argv as integer)
  try
    tell application (POSIX file appPath as alias) to activate
  end try
  delay 0.2
  tell application "System Events"
    try
      set procRef to first application process whose unix id is targetPid
      set frontmost of procRef to true
      try
        perform action "AXRaise" of window 1 of procRef
      end try
      try
        set position of window 1 of procRef to {80, 80}
      end try
    end try
  end tell
end run
'''
try:
    subprocess.run(
        ["/usr/bin/osascript", "-", sys.argv[1], sys.argv[2]],
        input=script.encode("utf-8"),
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        timeout=2,
    )
except subprocess.TimeoutExpired:
    pass
PY
}

window_is_onscreen() {
  local window_json="$1"
  python3 - "$window_json" <<'PY'
import json
import sys
try:
    value = json.load(open(sys.argv[1]))
except Exception:
    raise SystemExit(1)
raise SystemExit(0 if value.get("onscreen") is True else 1)
PY
}

window_onscreen_state() {
  local window_json="$1"
  python3 - "$window_json" <<'PY'
import json
import sys
try:
    value = json.load(open(sys.argv[1]))
except Exception:
    print("unknown")
    raise SystemExit(0)
if value.get("onscreen") is True:
    print("true")
elif value.get("onscreen") is False:
    print("false")
else:
    print("unknown")
PY
}

run_mode() {
  local mode="$1"
  local palette="$2"
  local home_dir="$WORK/home-$mode"
  local settings_dir="$home_dir/Library/Application Support/Squallz"
  local trace="$WORK/$mode-trace.jsonl"
  local window_json="$WORK/$mode-window.json"
  local screenshot="$OUT_DIR/$mode.png"
  local capture_method="$WORK/$mode-capture-method.txt"
  local image_diag="$WORK/$mode-image-diagnostics.json"
  local rejected_image="$WORK/$mode-rejected.png"

  rm -rf "$home_dir"
  mkdir -p "$settings_dir"
  rm -f "$trace" "$window_json" "$screenshot" "$capture_method" "$image_diag" "$rejected_image"
  python3 - "$settings_dir/settings.json" "$mode" "$palette" <<'PY'
import json, pathlib, sys
path = pathlib.Path(sys.argv[1])
payload = {
    "theme": "light",
    "language": "en-US",
    "ui_mode": sys.argv[2],
    "accent_palette": sys.argv[3],
}
path.write_text(json.dumps(payload, indent=2), encoding="utf-8")
PY

  open -n -a "$APP" \
    --env "HOME=$home_dir" \
    --env "SQUALLZ_VALIDATION_TRACE=$trace" \
    "$ARCHIVE"

  local pid=""
  for _ in {1..50}; do
    pid="$(pgrep -n -x squallz-gui || true)"
    if [[ -n "$pid" ]]; then
      break
    fi
    sleep 0.1
  done
  if [[ -z "$pid" ]]; then
    mode_blocked "$mode app process did not start"
    return 1
  fi

  local cleanup_pid="$pid"
  for _ in {1..80}; do
    if grep -q '"event":"open_archive.ok"' "$trace" 2>/dev/null; then
      break
    fi
    sleep 0.1
  done
  if ! grep -q '"event":"open_archive.ok"' "$trace" 2>/dev/null; then
    kill "$cleanup_pid" >/dev/null 2>&1 || true
    [[ -f "$trace" ]] && cat "$trace" >&2
    mode_blocked "$mode frontend did not open archive successfully"
    return 1
  fi

  for _ in {1..80}; do
    if grep -q '"event":"frontend.render.ready"' "$trace" 2>/dev/null; then
      break
    fi
    sleep 0.1
  done
  if ! grep -q '"event":"frontend.render.ready"' "$trace" 2>/dev/null; then
    kill "$cleanup_pid" >/dev/null 2>&1 || true
    [[ -f "$trace" ]] && cat "$trace" >&2
    mode_blocked "$mode frontend did not report rendered archive content"
    return 1
  fi

  for _ in {1..8}; do
    find_window "$pid" "$window_json" >/dev/null 2>&1 || true
    if [[ -s "$window_json" ]] && window_is_onscreen "$window_json"; then
      break
    fi
    force_window_frontmost "$pid"
    sleep 0.3
  done
  [[ -s "$window_json" ]] || {
    kill "$cleanup_pid" >/dev/null 2>&1 || true
    mode_blocked "$mode app window not found"
    return 1
  }

  local window_id
  window_id="$(python3 - "$window_json" <<'PY'
import json, sys
print(json.load(open(sys.argv[1]))["window_id"])
PY
)"
  if ! capture_window "$window_id" "$screenshot" "$capture_method"; then
    rm -f "$screenshot"
    kill "$cleanup_pid" >/dev/null 2>&1 || true
    mode_blocked "$mode screenshot capture failed"
    return 1
  fi
  if [[ ! -s "$screenshot" ]]; then
    rm -f "$screenshot"
    kill "$cleanup_pid" >/dev/null 2>&1 || true
    mode_blocked "$mode screenshot is empty"
    return 1
  fi
  if ! image_has_visible_pixels "$screenshot"; then
    mv "$screenshot" "$rejected_image" 2>/dev/null || rm -f "$screenshot"
    write_image_diagnostics "$rejected_image" "$image_diag" || true
    kill "$cleanup_pid" >/dev/null 2>&1 || true
    case "$(window_onscreen_state "$window_json")" in
      false)
        mode_blocked "$mode screenshot is blank because the app window is not onscreen after activation attempts"
        ;;
      true)
        mode_blocked "$mode screenshot is blank or privacy-redacted despite onscreen WindowServer metadata"
        ;;
      *)
        mode_blocked "$mode screenshot is blank or privacy-redacted; WindowServer did not expose reliable onscreen metadata"
        ;;
    esac
    return 1
  fi

  kill "$cleanup_pid" >/dev/null 2>&1 || true
  wait "$cleanup_pid" 2>/dev/null || true
}

if ! run_mode modern aqua; then
  :
fi
if ! run_mode classic nordic; then
  :
fi

if [[ "${#screenshot_blockers[@]}" -gt 0 ]]; then
  blocked_reason="$(printf '%s; ' "${screenshot_blockers[@]}")"
  write_blocked_report "${blocked_reason%; }"
  exit 1
fi

MODERN_DIM="$(sips -g pixelWidth -g pixelHeight "$OUT_DIR/modern.png" 2>/dev/null | awk '/pixel/{print $1"="$2}' | paste -sd ', ' -)"
CLASSIC_DIM="$(sips -g pixelWidth -g pixelHeight "$OUT_DIR/classic.png" 2>/dev/null | awk '/pixel/{print $1"="$2}' | paste -sd ', ' -)"
MODERN_CAPTURE_METHOD="$(cat "$WORK/modern-capture-method.txt" 2>/dev/null || echo "unknown")"
CLASSIC_CAPTURE_METHOD="$(cat "$WORK/classic-capture-method.txt" 2>/dev/null || echo "unknown")"

cat > "$REPORT" <<EOF
# Squallz macOS Native Window Screenshots

Generated: $(date -u +"%Y-%m-%dT%H:%M:%SZ")

Status: pass

## Scope

This check launches the macOS app bundle twice with isolated HOME directories:
once in Modern mode and once in Classic mode. Each launch receives a real archive
file through LaunchServices, waits for \`open_archive.ok\` and frontend
\`frontend.render.ready\`, finds the layer-0 native app window, and captures the
window image through the native window server path. It uses
\`screencapture -l\` first and falls back to a Swift/CoreGraphics window image
capture when the current automation Space refuses \`screencapture\`. Captures
must also pass a content-area pixel gate below the native titlebar so traffic
lights alone cannot produce a false pass.

## Inputs

- App: \`$APP\`
- Archive: \`$ARCHIVE\`
- Output directory: \`$OUT_DIR\`

## Outputs

- Modern screenshot: \`$OUT_DIR/modern.png\` ($MODERN_DIM), method: \`$MODERN_CAPTURE_METHOD\`
- Classic screenshot: \`$OUT_DIR/classic.png\` ($CLASSIC_DIM), method: \`$CLASSIC_CAPTURE_METHOD\`
- Modern trace/window metadata: \`$WORK/modern-trace.jsonl\`, \`$WORK/modern-window.json\`
- Classic trace/window metadata: \`$WORK/classic-trace.jsonl\`, \`$WORK/classic-window.json\`

## Result

Passed.
EOF

echo "report=$REPORT"
echo "modern=$OUT_DIR/modern.png"
echo "classic=$OUT_DIR/classic.png"
