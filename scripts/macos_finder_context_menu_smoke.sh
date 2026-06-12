#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPORT="$ROOT/benches/MACOS_FINDER_CONTEXT_MENU_SMOKE.md"
APP="${1:-"$ROOT/target/release/bundle/macos/Squallz.app"}"
if [[ "$APP" != /* ]]; then
  APP="$ROOT/$APP"
fi
SQZ_HELPER="$APP/Contents/Resources/bin/sqz"
WORK="$ROOT/target/squallz-macos-finder-context-menu-smoke"
FIXTURE_DIR="$WORK/fixture"
FIXTURE_ARCHIVE="$FIXTURE_DIR/finder-context-menu-smoke.zip"
ACTION_NAME="Squallz Finder Context Smoke"
WORKFLOW="$HOME/Library/Services/$ACTION_NAME.workflow"
SCRIPT_PATH="$WORK/run-finder-context-smoke.zsh"
ACTION_STDOUT="$WORK/action.stdout"
ACTION_ARGS="$WORK/action.args"
ACTION_MARKER="$WORK/action.ok"
AS_OUT="$WORK/finder-context-menu.out"
AS_ERR="$WORK/finder-context-menu.err"
OSA_SCRIPT="$WORK/finder-context-menu.applescript"

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

cleanup() {
  if [[ -d "${WORKFLOW:-}" && "${SQUALLZ_FINDER_CONTEXT_KEEP_WORKFLOW:-0}" != "1" ]]; then
    rm -rf "$WORKFLOW"
    /System/Library/CoreServices/pbs -flush English >/dev/null 2>&1 || true
    /System/Library/CoreServices/pbs -update English >/dev/null 2>&1 || true
  fi
}
trap cleanup EXIT

xml_escape() {
  python3 - "$1" <<'PY'
import html
import sys
print(html.escape(sys.argv[1], quote=True))
PY
}

shell_quote() {
  python3 - "$1" <<'PY'
import shlex
import sys
print(shlex.quote(sys.argv[1]))
PY
}

write_report() {
  local status="pass"
  if [[ "${#blockers[@]}" -gt 0 ]]; then
    status="blocked"
  fi
  {
    echo "# macOS Finder Context Menu Smoke"
    echo
    echo "Generated: $(date -u '+%Y-%m-%dT%H:%M:%SZ')"
    echo
    echo "Status: $status"
    echo
    echo "## Scope"
    echo
    echo "This smoke check installs a temporary Squallz-labeled Finder Quick Action into"
    echo "the real user-visible \`~/Library/Services\`, reveals a fixture ZIP in Finder,"
    echo "opens the visible Finder context menu through System Events, clicks the"
    echo "temporary action, and verifies that the action ran the packaged first-party"
    echo "\`Contents/Resources/bin/sqz test\` helper against the selected archive."
    echo
    echo "## Inputs"
    echo
    echo "- App: \`$APP\`"
    echo "- Bundled sqz: \`$SQZ_HELPER\`"
    echo "- Temporary workflow: \`$WORKFLOW\`"
    echo "- Fixture archive: \`$FIXTURE_ARCHIVE\`"
    echo "- Work dir: \`$WORK\`"
    echo
    echo "## Results"
    echo
    echo "| Check | Status | Evidence |"
    echo "| ---- | ---- | ---- |"
    printf '%s\n' "${rows[@]}"
    echo
    echo "## Finder Script Output"
    if [[ -s "$AS_OUT" || -s "$AS_ERR" ]]; then
      echo
      echo '```text'
      [[ -s "$AS_OUT" ]] && cat "$AS_OUT"
      [[ -s "$AS_ERR" ]] && cat "$AS_ERR"
      echo '```'
    else
      echo
      echo "- None."
    fi
    echo
    echo "## Action Output"
    if [[ -s "$ACTION_STDOUT" ]]; then
      echo
      echo '```json'
      cat "$ACTION_STDOUT"
      echo
      echo '```'
    else
      echo
      echo "- None."
    fi
    echo
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
    echo "## Cleanup"
    if [[ "${SQUALLZ_FINDER_CONTEXT_KEEP_WORKFLOW:-0}" == "1" ]]; then
      echo "- Temporary workflow kept because \`SQUALLZ_FINDER_CONTEXT_KEEP_WORKFLOW=1\`."
    else
      echo "- Temporary workflow is removed on exit."
    fi
  } >"$REPORT"

  echo "report=$REPORT"
  echo "status=$status"
  if [[ "$status" == "blocked" ]]; then
    exit 2
  fi
}

mkdir -p "$ROOT/benches" "$WORK" "$FIXTURE_DIR"
rm -rf "$FIXTURE_DIR" "$ACTION_STDOUT" "$ACTION_ARGS" "$ACTION_MARKER" "$AS_OUT" "$AS_ERR" "$OSA_SCRIPT"
mkdir -p "$FIXTURE_DIR"

if [[ "$(uname -s)" == "Darwin" ]]; then
  pass "platform" "running on macOS"
else
  block "platform" "Finder context menu smoke only runs on macOS"
fi

for tool in osascript open zip python3; do
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

if [[ -x "$SQZ_HELPER" ]]; then
  pass "first-party-sqz-helper" "\`$SQZ_HELPER\` is executable"
else
  block "first-party-sqz-helper" "\`$SQZ_HELPER\` is missing or not executable"
fi

if [[ -e "$WORKFLOW" && "${SQUALLZ_FINDER_CONTEXT_OVERWRITE:-0}" != "1" ]]; then
  block "temporary-workflow-path" "\`$WORKFLOW\` already exists; set SQUALLZ_FINDER_CONTEXT_OVERWRITE=1 only in an attended test session"
fi

if [[ "${#blockers[@]}" -gt 0 ]]; then
  write_report
fi

printf 'finder context menu smoke\n' >"$FIXTURE_DIR/payload.txt"
(cd "$FIXTURE_DIR" && zip -q "$FIXTURE_ARCHIVE" payload.txt)
rm -f "$FIXTURE_DIR/payload.txt"
if [[ -s "$FIXTURE_ARCHIVE" ]]; then
  pass "fixture-archive" "created \`$FIXTURE_ARCHIVE\`"
else
  block "fixture-archive" "failed to create fixture archive"
  write_report
fi

mkdir -p "$(dirname "$WORKFLOW")" "$WORKFLOW/Contents"
cat >"$SCRIPT_PATH" <<EOF
#!/usr/bin/env zsh
set -euo pipefail
printf '%s\n' "\$@" > "${ACTION_ARGS}"
"${SQZ_HELPER}" --lang=en-US test "\$1" --json > "${ACTION_STDOUT}"
touch "${ACTION_MARKER}"
EOF
chmod +x "$SCRIPT_PATH"

bundle_id="dev.squallz.desktop.finder-context-smoke"
escaped_action="$(xml_escape "$ACTION_NAME")"
escaped_bundle="$(xml_escape "$bundle_id")"
escaped_command="$(xml_escape "/bin/zsh $(shell_quote "$SCRIPT_PATH") \"\$@\"")"
cat >"$WORKFLOW/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleIdentifier</key>
  <string>${escaped_bundle}</string>
  <key>CFBundleName</key>
  <string>${escaped_action}</string>
  <key>CFBundleVersion</key>
  <string>1</string>
  <key>NSServices</key>
  <array>
    <dict>
      <key>NSMenuItem</key>
      <dict>
        <key>default</key>
        <string>${escaped_action}</string>
      </dict>
      <key>NSMessage</key>
      <string>runWorkflowAsService</string>
      <key>NSSendFileTypes</key>
      <array>
        <string>public.item</string>
      </array>
    </dict>
  </array>
</dict>
</plist>
EOF
cat >"$WORKFLOW/Contents/document.wflow" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>AMApplicationBuild</key>
  <string>Squallz</string>
  <key>AMDocumentVersion</key>
  <string>2</string>
  <key>actions</key>
  <array>
    <dict>
      <key>action</key>
      <dict>
        <key>AMAccepts</key>
        <dict>
          <key>Container</key>
          <string>List</string>
          <key>Optional</key>
          <true/>
          <key>Types</key>
          <array>
            <string>com.apple.cocoa.path</string>
          </array>
        </dict>
        <key>ActionBundlePath</key>
        <string>/System/Library/Automator/Run Shell Script.action</string>
        <key>ActionName</key>
        <string>Run Shell Script</string>
        <key>ActionParameters</key>
        <dict>
          <key>COMMAND_STRING</key>
          <string>${escaped_command}</string>
          <key>CheckedForUserDefaultShell</key>
          <true/>
          <key>inputMethod</key>
          <integer>1</integer>
          <key>shell</key>
          <string>/bin/zsh</string>
        </dict>
        <key>BundleIdentifier</key>
        <string>com.apple.RunShellScript</string>
      </dict>
      <key>isViewVisible</key>
      <integer>1</integer>
    </dict>
  </array>
  <key>connectors</key>
  <dict/>
  <key>workflowMetaData</key>
  <dict>
    <key>inputTypeIdentifier</key>
    <string>com.apple.Automator.fileSystemObject</string>
    <key>outputTypeIdentifier</key>
    <string>com.apple.Automator.nothing</string>
    <key>processesInput</key>
    <integer>1</integer>
    <key>serviceInputTypeIdentifier</key>
    <string>com.apple.Automator.fileSystemObject</string>
    <key>serviceProcessesInput</key>
    <integer>1</integer>
    <key>workflowTypeIdentifier</key>
    <string>com.apple.Automator.servicesMenu</string>
  </dict>
  <key>workflowName</key>
  <string>${escaped_action}</string>
</dict>
</plist>
EOF

/usr/bin/plutil -lint "$WORKFLOW/Contents/Info.plist" "$WORKFLOW/Contents/document.wflow" >/dev/null
pass "temporary-workflow" "installed temporary workflow \`$WORKFLOW\`"

if /System/Library/CoreServices/pbs -flush English >/dev/null 2>&1 && /System/Library/CoreServices/pbs -update English >/dev/null 2>&1; then
  pass "services-refresh" "LaunchServices Services cache refreshed"
else
  warn "services-refresh" "could not refresh Services cache with pbs; Finder may still discover the workflow lazily"
fi

cat >"$OSA_SCRIPT" <<'OSA'
on collectMenuNames(theMenu)
  set output to ""
  tell application "System Events"
    try
      repeat with itemRef in menu items of theMenu
        try
          set output to output & (name of itemRef as text) & linefeed
        end try
        try
          repeat with subMenuRef in menus of itemRef
            set output to output & collectMenuNames(subMenuRef)
          end repeat
        end try
      end repeat
    end try
  end tell
  return output
end collectMenuNames

on clickActionInMenu(theMenu, actionName)
  tell application "System Events"
    try
      click menu item actionName of theMenu
      return true
    end try
    try
      repeat with itemRef in menu items of theMenu
        try
          repeat with subMenuRef in menus of itemRef
            if clickActionInMenu(subMenuRef, actionName) then return true
          end repeat
        end try
      end repeat
    end try
  end tell
  return false
end clickActionInMenu

on run argv
  set archivePath to item 1 of argv
  set actionName to item 2 of argv
  set markerPath to item 3 of argv
  set folderPath to item 4 of argv
  set archiveAlias to POSIX file archivePath as alias
  set folderAlias to POSIX file folderPath as alias
  set clickStrategy to "unknown"
  set clickPoint to missing value
  set iconFallbackPoint to missing value
  set selectedFinderPath to ""
  set selectionStatus to "Finder AppleEvents selection not verified"

  tell application "Finder"
    open folderAlias
    set finderWindowCount to count windows
    try
      if finderWindowCount > 0 then
        set bounds of window 1 to {80, 80, 980, 680}
        set current view of window 1 to icon view
        set icon size of icon view options of window 1 to 64
      end if
    end try
    reveal archiveAlias
    set selection to archiveAlias
    activate
  end tell
  delay 1.0

  tell application "Finder"
    try
      if (count selection) is 1 then
        set selectedFinderPath to POSIX path of (item 1 of selection as alias)
      end if
    end try
    if selectedFinderPath is archivePath then
      set selectionStatus to "Finder AppleEvents selection verified"
    else
      set selectionStatus to "Finder AppleEvents selection not verified; selected=" & selectedFinderPath
    end if
    try
      set targetFinderItem to item (name of archiveAlias) of window 1
      set position of targetFinderItem to {160, 120}
      set iconPosition to position of targetFinderItem
      set finderBounds to bounds of window 1
      set iconFallbackPoint to {((item 1 of finderBounds) + (item 1 of iconPosition)), ((item 2 of finderBounds) + (item 2 of iconPosition))}
    end try
  end tell

  tell application "System Events"
    tell process "Finder"
      set frontmost to true
      if (count windows) is 0 then
        set menuBarCount to count menu bars
        return "BLOCKED: Finder has no visible Accessibility windows after explicit folder open + reveal; Finder window count=" & finderWindowCount & ", AX window count=0, menu bars=" & menuBarCount & ". Run from an attended visible desktop session in the active Space."
      end if
      set targetElement to missing value
      try
        set targetElement to first UI element of window 1 whose selected is true
      end try
      if targetElement is missing value then
        try
          set targetElement to first UI element of entire contents of window 1 whose selected is true
        end try
      end if
      if targetElement is missing value then
        if iconFallbackPoint is missing value then
          return "BLOCKED: selected Finder item is not exposed through Accessibility and Finder icon-view fallback coordinates are unavailable; " & selectionStatus
        end if
        set clickPoint to iconFallbackPoint
        set clickStrategy to "Finder icon-view coordinate fallback"
      else
        set itemPosition to position of targetElement
        set itemSize to size of targetElement
        set clickPoint to {((item 1 of itemPosition) + ((item 1 of itemSize) div 2)), ((item 2 of itemPosition) + ((item 2 of itemSize) div 2))}
        set clickStrategy to "Accessibility selected element"
      end if
      key down control
      click at clickPoint
      key up control
      delay 1.0
      if (count menus) is 0 then
        return "BLOCKED: Finder context menu did not open via " & clickStrategy & " at " & (item 1 of clickPoint as text) & "," & (item 2 of clickPoint as text) & "; " & selectionStatus
      end if
      set menuRef to menu 1
      set menuNames to collectMenuNames(menuRef)
      if menuNames does not contain actionName then
        return "BLOCKED: context menu opened via " & clickStrategy & " but did not expose " & actionName & "; " & selectionStatus & linefeed & menuNames
      end if
      if clickActionInMenu(menuRef, actionName) is false then
        return "BLOCKED: action was visible via " & clickStrategy & " but could not be clicked; " & selectionStatus
      end if
    end tell
  end tell

  repeat with i from 1 to 80
    do shell script "test -f " & quoted form of markerPath
    if result is "" then return "PASS: Finder context menu action clicked and marker appeared"
    delay 0.25
  end repeat
  return "BLOCKED: Finder action was clicked but marker file did not appear"
end run
OSA

set +e
python3 - "$OSA_SCRIPT" "$FIXTURE_ARCHIVE" "$ACTION_NAME" "$ACTION_MARKER" "$FIXTURE_DIR" "$AS_OUT" "$AS_ERR" <<'PY'
import subprocess
import sys

script, archive, action, marker, folder, out_path, err_path = sys.argv[1:]
with open(out_path, "wb") as out, open(err_path, "wb") as err:
    try:
        proc = subprocess.run(
            ["/usr/bin/osascript", script, archive, action, marker, folder],
            stdout=out,
            stderr=err,
            timeout=20,
        )
    except subprocess.TimeoutExpired:
        err.write(
            b"BLOCKED: osascript timed out after 20 seconds while opening Finder/context menu; run from an attended visible desktop session in the active Space.\n"
        )
        raise SystemExit(124)
raise SystemExit(proc.returncode)
PY
osascript_code="$?"
set -e

finder_output="$(cat "$AS_OUT" "$AS_ERR" 2>/dev/null | tr '\n' ' ' | sed 's/[[:space:]][[:space:]]*/ /g; s/[[:space:]]$//')"
if [[ "$osascript_code" -eq 0 && "$finder_output" == PASS:* ]]; then
  pass "finder-context-menu-click" "$finder_output"
else
  block "finder-context-menu-click" "${finder_output:-osascript exited $osascript_code}"
fi

if [[ -f "$ACTION_MARKER" && -s "$ACTION_STDOUT" ]]; then
  if python3 - "$ACTION_STDOUT" "$FIXTURE_ARCHIVE" <<'PY'
import json
import pathlib
import sys

value = json.loads(pathlib.Path(sys.argv[1]).read_text(encoding="utf-8"))
archive = pathlib.Path(sys.argv[2])
assert value["ok"] is True, value
assert value["archive"] == str(archive), value
PY
  then
    pass "action-ran-packaged-sqz" "temporary Finder action ran packaged sqz helper against selected fixture"
  else
    block "action-ran-packaged-sqz" "action output was present but did not match expected sqz test JSON"
  fi
else
  block "action-ran-packaged-sqz" "temporary Finder action did not create marker/output"
fi

notes+=("This script intentionally uses a temporary single-purpose Squallz workflow so it can prove Finder menu execution without leaving product actions installed in the user's real Services directory.")
notes+=("Packaged product Quick Actions remain covered by scripts/macos_packaged_quick_actions_smoke.sh; this script covers the visible Finder context menu layer.")

write_report
