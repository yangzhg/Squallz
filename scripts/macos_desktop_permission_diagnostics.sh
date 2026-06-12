#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPORT="${SQUALLZ_MACOS_DESKTOP_PERMISSION_REPORT:-"$ROOT/benches/MACOS_DESKTOP_PERMISSION_DIAGNOSTICS.md"}"
WORK="${SQUALLZ_MACOS_DESKTOP_PERMISSION_WORK:-"$ROOT/target/squallz-macos-desktop-permission-diagnostics"}"
if [[ "$REPORT" != /* ]]; then
  REPORT="$ROOT/$REPORT"
fi
if [[ "$WORK" != /* ]]; then
  WORK="$ROOT/$WORK"
fi
SWIFT_MODULE_CACHE="$WORK/swift-module-cache"
SWIFT_CLANG_SCANNER_CACHE="$SWIFT_MODULE_CACHE/clang-scanner"
SWIFT_SDK_MODULE_CACHE="$SWIFT_MODULE_CACHE/sdk"
APP="${1:-"$ROOT/target/release/bundle/macos/Squallz.app"}"
if [[ "$APP" != /* ]]; then
  APP="$ROOT/$APP"
fi

rows=()
blockers=()
warnings=()
notes=()

add_row() {
  rows+=("| $1 | $2 | $3 |")
}

pass() {
  add_row "$1" "pass" "$2"
}

warn() {
  add_row "$1" "warn" "$2"
  warnings+=("$1: $2")
}

block() {
  add_row "$1" "blocked" "$2"
  blockers+=("$1: $2")
}

json_get() {
  local key="$1"
  python3 - "$WORK/session.json" "$key" <<'PY'
import json
import sys
from pathlib import Path

path = Path(sys.argv[1])
key = sys.argv[2]
try:
    value = json.loads(path.read_text(encoding="utf-8")).get(key)
except Exception:
    value = None
if isinstance(value, bool):
    print("true" if value else "false")
elif value is None:
    print("")
else:
    print(value)
PY
}

run_osascript() {
  local name="$1"
  local script="$2"
  local out="$WORK/$name.out"
  local err="$WORK/$name.err"
  set +e
  /usr/bin/osascript -e "$script" >"$out" 2>"$err"
  local code="$?"
  set -e
  printf '%s\n' "$code"
}

result_text() {
  local name="$1"
  {
    [[ -s "$WORK/$name.out" ]] && cat "$WORK/$name.out"
    [[ -s "$WORK/$name.err" ]] && cat "$WORK/$name.err"
  } | tr '\n' ' ' | sed 's/[[:space:]][[:space:]]*/ /g; s/[[:space:]]$//'
}

write_report() {
  local status="pass"
  if [[ "${#blockers[@]}" -gt 0 ]]; then
    status="blocked"
  fi
  {
    echo "# macOS Desktop Permission Diagnostics"
    echo
    echo "Generated: $(date -u '+%Y-%m-%dT%H:%M:%SZ')"
    echo
    echo "Status: $status"
    echo
    echo "## Scope"
    echo
    echo "This diagnostic records whether the current macOS session can support"
    echo "attended desktop evidence: Screen Recording capture, Accessibility UI"
    echo "scripting, Finder Automation, console-session visibility, and active"
    echo "frontmost-process state. It does not click Finder menus, capture"
    echo "release screenshots, sign, notarize, or replace human signoff."
    echo
    echo "## Inputs"
    echo
    echo "- App: \`$APP\`"
    echo "- Work dir: \`$WORK\`"
    echo "- Swift module cache: \`$SWIFT_MODULE_CACHE\`"
    echo "- Console user: \`${CONSOLE_USER:-unknown}\`"
    echo "- Effective user: \`${USER:-unknown}\`"
    echo
    echo "## Results"
    echo
    echo "| Check | Status | Evidence |"
    echo "| ---- | ---- | ---- |"
    printf '%s\n' "${rows[@]}"
    echo
    if [[ -s "$WORK/session.json" ]]; then
      echo "## Session JSON"
      echo
      echo '```json'
      cat "$WORK/session.json"
      echo
      echo '```'
      echo
    fi
    echo "## Notes"
    if [[ "${#notes[@]}" -eq 0 ]]; then
      echo "- None."
    else
      printf -- '- %s\n' "${notes[@]}"
    fi
    echo
    echo "## Warnings"
    if [[ "${#warnings[@]}" -eq 0 ]]; then
      echo "- None."
    else
      printf -- '- %s\n' "${warnings[@]}"
    fi
    echo
    echo "## Blockers"
    if [[ "${#blockers[@]}" -eq 0 ]]; then
      echo "- None."
    else
      printf -- '- %s\n' "${blockers[@]}"
    fi
    echo
    echo "## Next Step"
    echo
    if [[ "$status" == "blocked" ]]; then
      echo "Resolve the blocked rows above, then rerun"
      echo "\`scripts/macos_finder_context_menu_smoke.sh\`. If the blocked"
      echo "row is \`session-unlocked\`, unlock and use the active console desktop"
      echo "session; if it is a TCC row, grant that permission to the terminal/Codex host."
    else
      echo "Permission preflight is clear. If native screenshots or Finder menu"
      echo "clicks still block, the remaining issue is likely active Space,"
      echo "WindowServer privacy redaction, or Finder menu timing."
    fi
  } >"$REPORT"

  echo "report=$REPORT"
  echo "status=$status"
  [[ "$status" == "pass" ]] || exit 2
}

mkdir -p "$ROOT/benches" "$WORK" "$SWIFT_MODULE_CACHE" "$SWIFT_CLANG_SCANNER_CACHE" "$SWIFT_SDK_MODULE_CACHE"
rm -f "$REPORT" "$WORK"/*.out "$WORK"/*.err "$WORK/session.json"

if [[ "$(uname -s)" == "Darwin" ]]; then
  pass "platform" "running on macOS"
else
  block "platform" "macOS desktop diagnostics only run on macOS"
  write_report
fi

for tool in swift osascript stat; do
  if command -v "$tool" >/dev/null 2>&1; then
    pass "tool:$tool" "\`$tool\` is available"
  else
    block "tool:$tool" "\`$tool\` is missing"
  fi
done

if [[ -x "$APP/Contents/MacOS/squallz-gui" ]]; then
  pass "packaged-app" "\`$APP\` contains executable Squallz app"
else
  warn "packaged-app" "\`$APP\` is missing or not executable; permission diagnostics can still run"
fi

CONSOLE_USER="$(stat -f '%Su' /dev/console 2>/dev/null || true)"
if [[ -n "${CONSOLE_USER:-}" && "$CONSOLE_USER" == "${USER:-}" ]]; then
  pass "console-user" "current user owns /dev/console"
else
  block "console-user" "current user \`${USER:-unknown}\` does not own /dev/console user \`${CONSOLE_USER:-unknown}\`"
fi

if command -v swift >/dev/null 2>&1; then
  set +e
  swift \
    -module-cache-path "$SWIFT_MODULE_CACHE" \
    -clang-scanner-module-cache-path "$SWIFT_CLANG_SCANNER_CACHE" \
    -sdk-module-cache-path "$SWIFT_SDK_MODULE_CACHE" \
    - "$WORK/session.json" <<'SWIFT' >/dev/null
import ApplicationServices
import CoreGraphics
import Foundation

let output = URL(fileURLWithPath: CommandLine.arguments[1])
let session = (CGSessionCopyCurrentDictionary() as? [String: Any]) ?? [:]

func boolValue(_ key: String) -> Any {
    if let value = session[key] as? Bool { return value }
    if let value = session[key] as? NSNumber { return value.boolValue }
    return NSNull()
}

func stringValue(_ key: String) -> Any {
    if let value = session[key] as? String { return value }
    return NSNull()
}

let payload: [String: Any] = [
    "screen_capture_preflight": CGPreflightScreenCaptureAccess(),
    "accessibility_trusted": AXIsProcessTrusted(),
    "session_on_console": boolValue(kCGSessionOnConsoleKey as String),
    "session_screen_locked": boolValue("CGSSessionScreenIsLocked"),
    "session_user": stringValue(kCGSessionUserNameKey as String),
    "session_login_done": boolValue("CGSSessionLoginDone"),
]
let data = try JSONSerialization.data(withJSONObject: payload, options: [.prettyPrinted, .sortedKeys])
try data.write(to: output)
SWIFT
  swift_code="$?"
  set -e
  if [[ "$swift_code" -ne 0 ]]; then
    warn "session-json" "Swift desktop diagnostic exited $swift_code"
  fi
fi

if [[ -s "$WORK/session.json" ]]; then
  if [[ "$(json_get screen_capture_preflight)" == "true" ]]; then
    pass "screen-recording" "CGPreflightScreenCaptureAccess=true"
  else
    block "screen-recording" "CGPreflightScreenCaptureAccess=false; grant Screen Recording to the terminal/Codex host"
  fi

  if [[ "$(json_get accessibility_trusted)" == "true" ]]; then
    pass "accessibility-trusted" "AXIsProcessTrusted=true"
  else
    block "accessibility-trusted" "AXIsProcessTrusted=false; grant Accessibility to the terminal/Codex host"
  fi

  case "$(json_get session_on_console)" in
    true) pass "session-on-console" "CGSession reports this is an on-console session" ;;
    false) block "session-on-console" "CGSession reports this is not the active console session" ;;
    *) warn "session-on-console" "CGSession did not expose on-console state" ;;
  esac

  case "$(json_get session_screen_locked)" in
    true) block "session-unlocked" "CGSession reports the screen is locked" ;;
    false) pass "session-unlocked" "CGSession reports the screen is unlocked" ;;
    *) warn "session-unlocked" "CGSession did not expose lock state" ;;
  esac
else
  block "session-json" "could not collect CGSession / TCC diagnostics with Swift"
fi

code="$(run_osascript finder-automation 'tell application "Finder" to get name')"
if [[ "$code" == "0" && "$(cat "$WORK/finder-automation.out" 2>/dev/null || true)" == "Finder" ]]; then
  pass "finder-automation" "AppleEvents can talk to Finder"
else
  block "finder-automation" "cannot talk to Finder via AppleEvents: $(result_text finder-automation)"
fi

code="$(run_osascript ui-scripting 'tell application "System Events" to get UI elements enabled')"
ui_enabled="$(cat "$WORK/ui-scripting.out" 2>/dev/null | tr -d '\r' || true)"
if [[ "$code" == "0" && "$ui_enabled" == "true" ]]; then
  pass "system-events-ui-scripting" "System Events UI scripting is enabled"
elif [[ "$code" == "0" && "$ui_enabled" == "false" ]]; then
  block "system-events-ui-scripting" "System Events reports UI elements enabled=false"
else
  block "system-events-ui-scripting" "cannot query System Events UI scripting: $(result_text ui-scripting)"
fi

code="$(run_osascript frontmost-process 'tell application "System Events" to get name of first process whose frontmost is true')"
frontmost="$(cat "$WORK/frontmost-process.out" 2>/dev/null | tr -d '\r' || true)"
if [[ "$code" == "0" && -n "$frontmost" ]]; then
  pass "frontmost-process" "frontmost process is \`$frontmost\`"
else
  warn "frontmost-process" "could not read frontmost process: $(result_text frontmost-process)"
fi

notes+=("A pass here does not prove native screenshots or Finder menu clicks; it only proves the current session has the permissions those gates need.")
notes+=("If screen-recording is pass but screenshots are blank, prefer active Space / WindowServer redaction diagnostics over changing product code.")
notes+=("If Finder Automation and Accessibility are pass but context-menu click times out, rerun from an attended visible desktop with Finder and the fixture folder in the active Space.")

write_report
