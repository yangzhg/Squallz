#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPORT="$ROOT/benches/MACOS_FINDER_UI_PREFLIGHT.md"
APP="${1:-"$ROOT/target/release/bundle/macos/Squallz.app"}"
WORK="$ROOT/target/squallz-macos-finder-ui-preflight"
FIXTURE_DIR="$WORK/fixture"
FIXTURE_ARCHIVE="$FIXTURE_DIR/finder-preflight.zip"

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

run_osascript() {
  local name="$1"
  local script="$2"
  local out="$WORK/${name}.out"
  local err="$WORK/${name}.err"
  set +e
  /usr/bin/osascript -e "$script" >"$out" 2>"$err"
  local code="$?"
  set -e
  printf '%s\n' "$code"
}

result_text() {
  local name="$1"
  {
    [[ -s "$WORK/${name}.out" ]] && cat "$WORK/${name}.out"
    [[ -s "$WORK/${name}.err" ]] && cat "$WORK/${name}.err"
  } | tr '\n' ' ' | sed 's/[[:space:]][[:space:]]*/ /g; s/[[:space:]]$//'
}

mkdir -p "$ROOT/benches" "$WORK" "$FIXTURE_DIR"
rm -f "$REPORT" "$WORK"/*.out "$WORK"/*.err "$FIXTURE_ARCHIVE"
printf 'finder ui preflight fixture\n' >"$FIXTURE_DIR/payload.txt"

if [[ "$(uname -s)" == "Darwin" ]]; then
  pass "platform" "running on macOS"
else
  block "platform" "Finder UI preflight only runs on macOS"
fi

for tool in osascript open zip; do
  if command -v "$tool" >/dev/null 2>&1; then
    pass "tool:$tool" "\`$tool\` is available"
  else
    block "tool:$tool" "\`$tool\` is missing"
  fi
done

if [[ -x "$APP/Contents/MacOS/squallz-gui" ]]; then
  pass "packaged-app" "\`$APP\` contains executable Squallz app"
else
  block "packaged-app" "\`$APP\` is missing or not executable"
fi

if [[ "${#blockers[@]}" -eq 0 ]]; then
  (cd "$FIXTURE_DIR" && zip -q "$FIXTURE_ARCHIVE" payload.txt)
  if [[ -s "$FIXTURE_ARCHIVE" ]]; then
    pass "fixture-archive" "created \`$FIXTURE_ARCHIVE\`"
  else
    block "fixture-archive" "failed to create Finder UI fixture archive"
  fi
fi

FINDER_NAME=""
if [[ "${#blockers[@]}" -eq 0 ]]; then
  code="$(run_osascript finder-name 'tell application "Finder" to get name')"
  FINDER_NAME="$(cat "$WORK/finder-name.out" 2>/dev/null || true)"
  if [[ "$code" == "0" && "$FINDER_NAME" == "Finder" ]]; then
    pass "finder-appleevents" "AppleEvents can talk to Finder"
  else
    block "finder-appleevents" "cannot talk to Finder via AppleEvents: $(result_text finder-name)"
  fi
fi

UI_SCRIPTING=""
if [[ "${#blockers[@]}" -eq 0 ]]; then
  code="$(run_osascript ui-scripting 'tell application "System Events" to get UI elements enabled')"
  UI_SCRIPTING="$(cat "$WORK/ui-scripting.out" 2>/dev/null | tr -d '\r' || true)"
  if [[ "$code" == "0" && "$UI_SCRIPTING" == "true" ]]; then
    pass "accessibility-ui-scripting" "System Events UI scripting is enabled"
  elif [[ "$code" == "0" && "$UI_SCRIPTING" == "false" ]]; then
    block "accessibility-ui-scripting" "System Events reports UI elements enabled=false; enable Accessibility for the terminal/Codex host before real Finder menu click smoke"
  else
    block "accessibility-ui-scripting" "cannot query System Events UI scripting: $(result_text ui-scripting)"
  fi
fi

if [[ "${#blockers[@]}" -eq 0 ]]; then
  code="$(run_osascript reveal-fixture "tell application \"Finder\" to reveal POSIX file \"${FIXTURE_ARCHIVE}\"")"
  if [[ "$code" == "0" ]]; then
    pass "finder-reveal-fixture" "Finder can reveal the fixture archive through AppleEvents"
  else
    block "finder-reveal-fixture" "Finder could not reveal fixture archive: $(result_text reveal-fixture)"
  fi
fi

MENU_BAR_COUNT=""
if [[ "${#blockers[@]}" -eq 0 ]]; then
  code="$(run_osascript finder-ui-process 'tell application "System Events" to tell process "Finder" to count menu bars')"
  MENU_BAR_COUNT="$(cat "$WORK/finder-ui-process.out" 2>/dev/null | tr -d '\r' || true)"
  if [[ "$code" == "0" && "$MENU_BAR_COUNT" =~ ^[0-9]+$ && "$MENU_BAR_COUNT" -gt 0 ]]; then
    pass "finder-ui-process" "System Events can inspect Finder UI process"
  else
    block "finder-ui-process" "System Events cannot inspect Finder UI process: $(result_text finder-ui-process)"
  fi
fi

if [[ "${SQUALLZ_FINDER_UI_LIVE:-0}" == "1" ]]; then
  if [[ "${#blockers[@]}" -eq 0 ]]; then
    warn "live-click" "SQUALLZ_FINDER_UI_LIVE=1 is set, but this preflight currently stops before invasive Finder pointer/menu clicks; use it to confirm permissions before manual Finder UI sign-off"
  fi
else
  notes+=("Set SQUALLZ_FINDER_UI_LIVE=1 only in an attended visible desktop session after accepting Accessibility/Automation prompts; this preflight intentionally stops before invasive Finder pointer/menu clicks.")
fi

notes+=("Packaged Quick Action workflows are already exercised through Automator by scripts/macos_packaged_quick_actions_smoke.sh; this preflight covers the separate visible Finder UI permission layer.")
notes+=("A final macOS UX sign-off still needs a visible Finder context menu/right-click or physical drag session.")

status="pass"
if [[ "${#blockers[@]}" -gt 0 ]]; then
  status="blocked"
fi

{
  echo "# macOS Finder UI Preflight"
  echo
  echo "Generated: $(date -u '+%Y-%m-%dT%H:%M:%SZ')"
  echo
  echo "Status: $status"
  echo
  echo "## Scope"
  echo
  echo "This preflight checks whether the current macOS session can support a real"
  echo "visible Finder UI menu/right-click verification: Finder AppleEvents,"
  echo "System Events Accessibility UI scripting, and Finder UI process inspection."
  echo "It does not install user services, click the Finder context menu, or synthesize"
  echo "physical pointer drag."
  echo
  echo "## Inputs"
  echo
  echo "- App: \`$APP\`"
  echo "- Fixture archive: \`$FIXTURE_ARCHIVE\`"
  echo "- Work dir: \`$WORK\`"
  echo "- UI scripting reported: \`${UI_SCRIPTING:-unknown}\`"
  echo
  echo "## Results"
  echo
  echo "| Check | Status | Evidence |"
  echo "| ---- | ---- | ---- |"
  printf '%s\n' "${rows[@]}"
  echo
  echo "## Notes"
  printf -- '- %s\n' "${notes[@]}"
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
} >"$REPORT"

echo "report=$REPORT"
echo "status=$status"

if [[ "$status" == "blocked" ]]; then
  exit 2
fi
