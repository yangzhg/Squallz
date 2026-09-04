//! Desktop/file-manager integration installers.
//!
//! macOS ships context-menu style actions through Finder Services / Quick
//! Actions. Linux file managers use user-local script/service-menu entries.
//! Windows uses user-local Explorer registry verbs plus wrapper scripts. All
//! routes launch the existing GUI task window first and only fall back to the
//! `sqz` CLI when the app handoff is unavailable.

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
use std::fs::{self, File};
use std::io;
#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
use std::io::Read;
#[cfg(target_os = "macos")]
use std::os::unix::fs::DirBuilderExt;
use std::path::Path;
#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
use std::path::PathBuf;
#[cfg(target_os = "macos")]
use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
use crate::dto::IntegrationActionDto;
use crate::dto::{
    IntegrationActionHealthDto, IntegrationActionHealthStateDto, IntegrationApplyResultDto,
    IntegrationDefaultHandlersDto, IntegrationDefaultHandlersStateDto,
    IntegrationFileManagerVisibilityDto, IntegrationFileManagerVisibilityStateDto,
    IntegrationHealthStateDto, IntegrationRemoveResultDto, IntegrationStatusDto,
    IntegrationSystemDiagnosticsDto, RuntimeBackendSourceDto, RuntimeBackendStatusDto,
};
#[cfg(target_os = "macos")]
use crate::dto::{IntegrationDefaultHandlerDto, IntegrationDefaultHandlerStateDto};
#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
use crate::open_files::{EXTERNAL_TASK_ACTION_ARG, EXTERNAL_TASK_OUTPUT_ARG};
use squallz_formats::{SevenZipBackendSource, UnrarBackendSource, WimlibBackendSource};
#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
use squallz_i18n::Localizer;

#[cfg(target_os = "macos")]
use objc2::rc::autoreleasepool;
#[cfg(target_os = "macos")]
use objc2_app_kit::{NSUpdateDynamicServices, NSWorkspace};
#[cfg(target_os = "macos")]
use objc2_foundation::{NSBundle, NSString, NSURL};

#[cfg(target_os = "macos")]
const SQUALLZ_BUNDLE_IDENTIFIER: &str = "dev.squallz.desktop";

#[cfg(target_os = "macos")]
const MACOS_DECLARED_FILE_EXTENSIONS: &[&str] = &[
    "zip", "jar", "apk", "cbz", "cbr", "ipa", "7z", "rar", "tar", "tgz", "tbz2", "txz", "tzst",
    "001", "wim", "swm", "sqz", "gz", "bz2", "xz", "zst", "lz4", "br",
];

#[cfg(target_os = "macos")]
static DEFAULT_HANDLER_PROBE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[cfg(target_os = "macos")]
#[derive(Debug, Clone, Copy)]
struct FinderAction {
    id: &'static str,
    name_key: &'static str,
    script_name: &'static str,
    script_body: &'static str,
}

#[cfg(target_os = "macos")]
const FINDER_ACTIONS: &[FinderAction] = &[
    FinderAction {
        id: "checksum",
        name_key: "gui.integration.finder.action.checksum",
        script_name: "squallz-checksum.sh",
        script_body: r#"
if run_gui_task "checksum" "$@"; then
  exit 0
fi
run_sqz checksum "$@"
"#,
    },
    FinderAction {
        id: "extract-here",
        name_key: "gui.integration.finder.action.extract_here",
        script_name: "squallz-extract-here.sh",
        script_body: r#"
if run_gui_task "extract-here" "$@"; then
  exit 0
fi
for item in "$@"; do
  [[ -f "$item" ]] || continue
  dest="$(dirname "$item")"
  run_sqz extract "$item" -d "$dest" --smart --file-manager-preset
done
"#,
    },
    FinderAction {
        id: "extract-to-folder",
        name_key: "gui.integration.finder.action.extract_to_folder",
        script_name: "squallz-extract-to-folder.sh",
        script_body: r#"
if run_gui_task "extract-to-folder" "$@"; then
  exit 0
fi
archive_stem() {
  local base suffix
  base="$(basename "$1")"
  for suffix in ".tar.zst" ".tar.xz" ".tar.bz2" ".tar.gz" ".tbz2" ".tgz" ".txz" ".tzst" ".zip" ".7z" ".rar" ".sqz" ".tar" ".gz" ".bz2" ".xz" ".zst" ".br" ".lz4"; do
    if [[ "$base" == *"$suffix" ]]; then
      printf '%s\n' "${base%$suffix}"
      return 0
    fi
  done
  printf '%s\n' "${base%.*}"
}

for item in "$@"; do
  [[ -f "$item" ]] || continue
  dest="$(dirname "$item")/$(archive_stem "$item")"
  mkdir -p "$dest"
  run_sqz extract "$item" -d "$dest" --file-manager-preset
done
"#,
    },
    FinderAction {
        id: "compress-to-7z",
        name_key: "gui.integration.finder.action.compress_to_7z",
        script_name: "squallz-compress-to-7z.sh",
        script_body: r#"
unique_output() {
  local path base ext candidate n
  path="$1"
  if [[ ! -e "$path" ]]; then
    printf '%s\n' "$path"
    return 0
  fi
  base="${path:r}"
  ext="${path:e}"
  for n in {2..999}; do
    candidate="$base $n.$ext"
    if [[ ! -e "$candidate" ]]; then
      printf '%s\n' "$candidate"
      return 0
    fi
  done
  printf '%s\n' "$path"
}

[[ "$#" -gt 0 ]] || exit 0
parent="$(dirname "$1")"
first_name="$(basename "$1")"
if [[ "$#" -eq 1 ]]; then
output="$parent/${first_name%.*}.7z"
else
  output="$parent/Archive.7z"
fi
output="$(unique_output "$output")"
if run_gui_task_with_output "compress-to-7z" "$output" "$@"; then
  exit 0
fi
run_sqz compress "$@" -o "$output" --file-manager-preset
"#,
    },
    FinderAction {
        id: "test-archive",
        name_key: "gui.integration.finder.action.test_archive",
        script_name: "squallz-test-archive.sh",
        script_body: r#"
if run_gui_task "test-archive" "$@"; then
  exit 0
fi
for item in "$@"; do
  [[ -f "$item" ]] || continue
  run_sqz test "$item"
done
"#,
    },
];

#[cfg(target_os = "macos")]
const SCRIPT_PREAMBLE_TEMPLATE: &str = r#"#!/bin/zsh
set -euo pipefail

SQUALLZ_INSTALLED_APP_BUNDLE={installed_app_bundle}
CLI_NOT_FOUND_ALERT={cli_not_found_alert}
SQUALLZ_TASK_WINDOW_ACTION_ARG={task_window_action_arg}
SQUALLZ_TASK_WINDOW_OUTPUT_ARG={task_window_output_arg}

resolve_sqz() {
  if [[ -n "${SQUALLZ_CLI:-}" && -x "${SQUALLZ_CLI}" ]]; then
    printf '%s\n' "${SQUALLZ_CLI}"
    return 0
  fi

  local -a candidates
  candidates=()
  if [[ -n "${SQUALLZ_INSTALLED_APP_BUNDLE:-}" ]]; then
    candidates+=("${SQUALLZ_INSTALLED_APP_BUNDLE}/Contents/MacOS/sqz")
    candidates+=("${SQUALLZ_INSTALLED_APP_BUNDLE}/Contents/Resources/bin/sqz")
  fi
  if [[ -n "${SQUALLZ_APP_BUNDLE:-}" ]]; then
    candidates+=("${SQUALLZ_APP_BUNDLE}/Contents/MacOS/sqz")
    candidates+=("${SQUALLZ_APP_BUNDLE}/Contents/Resources/bin/sqz")
  fi
  candidates+=("/Applications/Squallz.app/Contents/MacOS/sqz")
  candidates+=("/Applications/Squallz.app/Contents/Resources/bin/sqz")
  candidates+=("$HOME/.cargo/bin/sqz")

  for candidate in "${candidates[@]}"; do
    if [[ -x "$candidate" ]]; then
      printf '%s\n' "$candidate"
      return 0
    fi
  done
  if command -v sqz >/dev/null 2>&1; then
    command -v sqz
    return 0
  fi
  osascript -e "$CLI_NOT_FOUND_ALERT" >/dev/null 2>&1 || true
  exit 127
}

resolve_app_bundle() {
  if [[ "${SQUALLZ_DISABLE_GUI_HANDOFF:-}" == "1" ]]; then
    return 1
  fi

  local -a candidates
  candidates=()
  if [[ -n "${SQUALLZ_INSTALLED_APP_BUNDLE:-}" ]]; then
    candidates+=("${SQUALLZ_INSTALLED_APP_BUNDLE}")
  fi
  if [[ -n "${SQUALLZ_APP_BUNDLE:-}" ]]; then
    candidates+=("${SQUALLZ_APP_BUNDLE}")
  fi
  candidates+=("/Applications/Squallz.app")

  local candidate
  for candidate in "${candidates[@]}"; do
    if [[ -d "$candidate" && -x "$candidate/Contents/MacOS/squallz-gui" ]]; then
      printf '%s\n' "$candidate"
      return 0
    fi
  done
  return 1
}

run_gui_task() {
  local action app
  action="$1"
  shift
  app="$(resolve_app_bundle 2>/dev/null || true)"
  [[ -n "$app" ]] || return 1
  /usr/bin/open -n "$app" --args "$SQUALLZ_TASK_WINDOW_ACTION_ARG" "$action" "$@" >/dev/null 2>&1
}

run_gui_task_with_output() {
  local action output app
  action="$1"
  output="$2"
  shift 2
  app="$(resolve_app_bundle 2>/dev/null || true)"
  [[ -n "$app" ]] || return 1
  /usr/bin/open -n "$app" --args "$SQUALLZ_TASK_WINDOW_ACTION_ARG" "$action" "$SQUALLZ_TASK_WINDOW_OUTPUT_ARG" "$output" "$@" >/dev/null 2>&1
}

SQZ=""
run_sqz() {
  if [[ -z "$SQZ" ]]; then
    SQZ="$(resolve_sqz)"
  fi
  "$SQZ" "$@"
}
"#;

#[cfg(any(target_os = "linux", all(test, target_os = "macos")))]
#[derive(Debug, Clone, Copy)]
struct LinuxFileManagerAction {
    id: &'static str,
    name_key: &'static str,
    script_name: &'static str,
    desktop_name: &'static str,
    script_body: &'static str,
}

#[cfg(any(target_os = "linux", all(test, target_os = "macos")))]
const LINUX_FILE_MANAGER_ACTIONS: &[LinuxFileManagerAction] = &[
    LinuxFileManagerAction {
        id: "checksum",
        name_key: "gui.integration.file_manager.action.checksum",
        script_name: "squallz-checksum.sh",
        desktop_name: "squallz-checksum.desktop",
        script_body: r#"
if run_gui_task "checksum" "$@"; then
  exit 0
fi
run_sqz checksum "$@"
"#,
    },
    LinuxFileManagerAction {
        id: "extract-here",
        name_key: "gui.integration.file_manager.action.extract_here",
        script_name: "squallz-extract-here.sh",
        desktop_name: "squallz-extract-here.desktop",
        script_body: r#"
collect_regular_file_inputs "$@" || exit 0
set -- "${SQUALLZ_REGULAR_FILE_INPUTS[@]}"
if run_gui_task "extract-here" "$@"; then
  exit 0
fi
for item in "$@"; do
  [[ -f "$item" ]] || continue
  dest="$(dirname -- "$item")"
  run_sqz extract "$item" -d "$dest" --smart --file-manager-preset
done
"#,
    },
    LinuxFileManagerAction {
        id: "extract-to-folder",
        name_key: "gui.integration.file_manager.action.extract_to_folder",
        script_name: "squallz-extract-to-folder.sh",
        desktop_name: "squallz-extract-to-folder.desktop",
        script_body: r#"
collect_regular_file_inputs "$@" || exit 0
set -- "${SQUALLZ_REGULAR_FILE_INPUTS[@]}"
if run_gui_task "extract-to-folder" "$@"; then
  exit 0
fi
archive_stem() {
  local base suffix
  base="$(basename -- "$1")"
  for suffix in ".tar.zst" ".tar.xz" ".tar.bz2" ".tar.gz" ".tbz2" ".tgz" ".txz" ".tzst" ".zip" ".7z" ".rar" ".sqz" ".tar" ".gz" ".bz2" ".xz" ".zst" ".br" ".lz4"; do
    if [[ "$base" == *"$suffix" ]]; then
      printf '%s\n' "${base%"$suffix"}"
      return 0
    fi
  done
  printf '%s\n' "${base%.*}"
}

for item in "$@"; do
  [[ -f "$item" ]] || continue
  dest="$(dirname -- "$item")/$(archive_stem "$item")"
  mkdir -p -- "$dest"
  run_sqz extract "$item" -d "$dest" --file-manager-preset
done
"#,
    },
    LinuxFileManagerAction {
        id: "compress-to-7z",
        name_key: "gui.integration.file_manager.action.compress_to_7z",
        script_name: "squallz-compress-to-7z.sh",
        desktop_name: "squallz-compress-to-7z.desktop",
        script_body: r#"
unique_output() {
  local path base candidate n
  path="$1"
  if [[ ! -e "$path" ]]; then
    printf '%s\n' "$path"
    return 0
  fi
  base="${path%.7z}"
  for n in $(seq 2 999); do
    candidate="$base $n.7z"
    if [[ ! -e "$candidate" ]]; then
      printf '%s\n' "$candidate"
      return 0
    fi
  done
  printf '%s\n' "$path"
}

[[ "$#" -gt 0 ]] || exit 0
parent="$(dirname -- "$1")"
first_name="$(basename -- "$1")"
if [[ "$#" -eq 1 ]]; then
  output="$parent/${first_name%.*}.7z"
else
  output="$parent/Archive.7z"
fi
output="$(unique_output "$output")"
if run_gui_task_with_output "compress-to-7z" "$output" "$@"; then
  exit 0
fi
run_sqz compress "$@" -o "$output" --file-manager-preset
"#,
    },
    LinuxFileManagerAction {
        id: "test-archive",
        name_key: "gui.integration.file_manager.action.test_archive",
        script_name: "squallz-test-archive.sh",
        desktop_name: "squallz-test-archive.desktop",
        script_body: r#"
collect_regular_file_inputs "$@" || exit 0
set -- "${SQUALLZ_REGULAR_FILE_INPUTS[@]}"
for item in "$@"; do
  if run_gui_task "test-archive" "$item"; then
    continue
  fi
  run_sqz test "$item"
done
"#,
    },
];

#[cfg(any(target_os = "linux", all(test, target_os = "macos")))]
const LINUX_SCRIPT_PREAMBLE_TEMPLATE: &str = r#"#!/usr/bin/env bash
set -euo pipefail

CLI_NOT_FOUND_TITLE={cli_not_found_title}
CLI_NOT_FOUND_MESSAGE={cli_not_found_message}
SQUALLZ_TASK_WINDOW_ACTION_ARG={task_window_action_arg}
SQUALLZ_TASK_WINDOW_OUTPUT_ARG={task_window_output_arg}
SQUALLZ_APPIMAGE={installed_appimage}
SQUALLZ_APPIMAGE_EXTRACT_AND_RUN={installed_appimage_extract_and_run}

notify_missing_cli() {
  if command -v notify-send >/dev/null 2>&1; then
    notify-send "$CLI_NOT_FOUND_TITLE" "$CLI_NOT_FOUND_MESSAGE" >/dev/null 2>&1 || true
  else
    printf '%s: %s\n' "$CLI_NOT_FOUND_TITLE" "$CLI_NOT_FOUND_MESSAGE" >&2
  fi
}

resolve_sqz() {
  if [[ -n "${SQUALLZ_CLI:-}" && -x "${SQUALLZ_CLI}" ]]; then
    printf '%s\n' "${SQUALLZ_CLI}"
    return 0
  fi
  if command -v sqz >/dev/null 2>&1; then
    command -v sqz
    return 0
  fi
  notify_missing_cli
  exit 127
}

resolve_gui() {
  if [[ "${SQUALLZ_DISABLE_GUI_HANDOFF:-}" == "1" ]]; then
    return 1
  fi

  if [[ -n "${SQUALLZ_GUI:-}" && -x "${SQUALLZ_GUI}" ]]; then
    printf '%s\n' "${SQUALLZ_GUI}"
    return 0
  fi

  if [[ -n "$SQUALLZ_APPIMAGE" && -f "$SQUALLZ_APPIMAGE" && ! -L "$SQUALLZ_APPIMAGE" && -x "$SQUALLZ_APPIMAGE" ]]; then
    printf '%s\n' "$SQUALLZ_APPIMAGE"
    return 0
  fi

  local -a candidates
  candidates=()
  if [[ -n "${APPDIR:-}" ]]; then
    candidates+=("${APPDIR}/usr/bin/squallz-gui")
    candidates+=("${APPDIR}/squallz-gui")
  fi
  candidates+=("/usr/bin/squallz-gui")
  candidates+=("/usr/local/bin/squallz-gui")

  local candidate
  for candidate in "${candidates[@]}"; do
    if [[ -x "$candidate" ]]; then
      printf '%s\n' "$candidate"
      return 0
    fi
  done
  if command -v squallz-gui >/dev/null 2>&1; then
    command -v squallz-gui
    return 0
  fi
  return 1
}

launch_gui() {
  local gui
  gui="$1"
  shift
  if [[ "$gui" == "$SQUALLZ_APPIMAGE" && "$SQUALLZ_APPIMAGE_EXTRACT_AND_RUN" == "1" ]]; then
    APPIMAGE_EXTRACT_AND_RUN=1 "$gui" "$@" >/dev/null 2>&1 &
  else
    "$gui" "$@" >/dev/null 2>&1 &
  fi
}

run_gui_task() {
  local action gui
  action="$1"
  shift
  gui="$(resolve_gui 2>/dev/null || true)"
  [[ -n "$gui" ]] || return 1
  launch_gui "$gui" "$SQUALLZ_TASK_WINDOW_ACTION_ARG" "$action" "$@"
}

run_gui_task_with_output() {
  local action output gui
  action="$1"
  output="$2"
  shift 2
  gui="$(resolve_gui 2>/dev/null || true)"
  [[ -n "$gui" ]] || return 1
  launch_gui "$gui" "$SQUALLZ_TASK_WINDOW_ACTION_ARG" "$action" "$SQUALLZ_TASK_WINDOW_OUTPUT_ARG" "$output" "$@"
}

SQZ=""
run_sqz() {
  if [[ -z "$SQZ" ]]; then
    SQZ="$(resolve_sqz)"
  fi
  "$SQZ" "$@"
}

collect_regular_file_inputs() {
  local item
  SQUALLZ_REGULAR_FILE_INPUTS=()
  for item in "$@"; do
    if [[ -f "$item" ]]; then
      SQUALLZ_REGULAR_FILE_INPUTS+=("$item")
    fi
  done
  [[ ${#SQUALLZ_REGULAR_FILE_INPUTS[@]} -gt 0 ]]
}

SQUALLZ_ACTION_INPUTS=()
for SQUALLZ_ACTION_INPUT in "$@"; do
  [[ -n "$SQUALLZ_ACTION_INPUT" ]] || continue
  if [[ "$SQUALLZ_ACTION_INPUT" == /* ]]; then
    SQUALLZ_ACTION_INPUTS+=("$SQUALLZ_ACTION_INPUT")
  else
    SQUALLZ_ACTION_INPUTS+=("$PWD/$SQUALLZ_ACTION_INPUT")
  fi
done
if [[ ${#SQUALLZ_ACTION_INPUTS[@]} -eq 0 ]]; then
  exit 0
fi
set -- "${SQUALLZ_ACTION_INPUTS[@]}"
unset SQUALLZ_ACTION_INPUTS SQUALLZ_ACTION_INPUT
"#;

#[cfg(any(target_os = "windows", all(test, target_os = "macos")))]
#[derive(Debug, Clone, Copy)]
struct WindowsExplorerAction {
    id: &'static str,
    name_key: &'static str,
    script_name: &'static str,
    script_body: &'static str,
}

#[cfg(any(target_os = "windows", all(test, target_os = "macos")))]
const WINDOWS_EXPLORER_ACTIONS: &[WindowsExplorerAction] = &[
    WindowsExplorerAction {
        id: "checksum",
        name_key: "gui.integration.explorer.action.checksum",
        script_name: "squallz-checksum.ps1",
        script_body: r#"
$Selected = @(Select-ExistingPaths $Paths)
if ($Selected.Count -eq 0) { exit 0 }
if (Invoke-SquallzGuiTask -Action 'checksum' -Paths $Selected) { exit 0 }
$Arguments = @('checksum') + $Selected
Invoke-Sqz @Arguments
"#,
    },
    WindowsExplorerAction {
        id: "extract-here",
        name_key: "gui.integration.explorer.action.extract_here",
        script_name: "squallz-extract-here.ps1",
        script_body: r#"
$Selected = @(Select-ExistingFiles $Paths)
if ($Selected.Count -eq 0) { exit 0 }
if (Invoke-SquallzGuiTask -Action 'extract-here' -Paths $Selected) { exit 0 }
foreach ($Item in $Selected) {
  $Dest = Split-Path -Parent $Item
  Invoke-Sqz extract $Item -d $Dest --smart --file-manager-preset
}
"#,
    },
    WindowsExplorerAction {
        id: "extract-to-folder",
        name_key: "gui.integration.explorer.action.extract_to_folder",
        script_name: "squallz-extract-to-folder.ps1",
        script_body: r#"
$Selected = @(Select-ExistingFiles $Paths)
if ($Selected.Count -eq 0) { exit 0 }
if (Invoke-SquallzGuiTask -Action 'extract-to-folder' -Paths $Selected) { exit 0 }
foreach ($Item in $Selected) {
  $Parent = Split-Path -Parent $Item
  $Dest = Join-Path $Parent (Get-ArchiveStem $Item)
  New-Item -ItemType Directory -Force -Path $Dest | Out-Null
  Invoke-Sqz extract $Item -d $Dest --file-manager-preset
}
"#,
    },
    WindowsExplorerAction {
        id: "compress-to-7z",
        name_key: "gui.integration.explorer.action.compress_to_7z",
        script_name: "squallz-compress-to-7z.ps1",
        script_body: r#"
$Selected = @(Select-ExistingPaths $Paths)
if ($Selected.Count -eq 0) { exit 0 }
$Parent = Split-Path -Parent $Selected[0]
if ($Selected.Count -eq 1) {
  $Output = Join-Path $Parent "$(Get-ArchiveStem $Selected[0]).7z"
} else {
  $Output = Join-Path $Parent 'Archive.7z'
}
$Output = New-UniqueOutputPath $Output
if (Invoke-SquallzGuiTask -Action 'compress-to-7z' -Output $Output -Paths $Selected) { exit 0 }
$Arguments = @('compress') + $Selected + @('-o', $Output, '--file-manager-preset')
Invoke-Sqz @Arguments
"#,
    },
    WindowsExplorerAction {
        id: "test-archive",
        name_key: "gui.integration.explorer.action.test_archive",
        script_name: "squallz-test-archive.ps1",
        script_body: r#"
$Selected = @(Select-ExistingFiles $Paths)
if ($Selected.Count -eq 0) { exit 0 }
if (Invoke-SquallzGuiTask -Action 'test-archive' -Paths $Selected) { exit 0 }
foreach ($Item in $Selected) {
  Invoke-Sqz test $Item
}
"#,
    },
];

#[cfg(any(target_os = "windows", all(test, target_os = "macos")))]
const WINDOWS_SCRIPT_PREAMBLE_TEMPLATE: &str = r#"param(
  [Parameter(ValueFromRemainingArguments = $true)]
  [string[]]$Paths
)

$ErrorActionPreference = 'Stop'
$CliNotFoundTitle = {cli_not_found_title}
$CliNotFoundMessage = {cli_not_found_message}
$SquallzTaskWindowActionArg = {task_window_action_arg}
$SquallzTaskWindowOutputArg = {task_window_output_arg}

function Show-SquallzCliMissing {
  try {
    Add-Type -AssemblyName System.Windows.Forms
    [System.Windows.Forms.MessageBox]::Show($CliNotFoundMessage, $CliNotFoundTitle, 'OK', 'Warning') | Out-Null
  } catch {
    Write-Error "$CliNotFoundTitle. $CliNotFoundMessage"
  }
}

function Resolve-SquallzGui {
  if ($env:SQUALLZ_DISABLE_GUI_HANDOFF -eq '1') { return $null }
  $Candidates = @()
  if ($env:SQUALLZ_GUI) { $Candidates += $env:SQUALLZ_GUI }
  if ($env:LOCALAPPDATA) {
    $Candidates += Join-Path $env:LOCALAPPDATA 'Programs\Squallz\Squallz.exe'
    $Candidates += Join-Path $env:LOCALAPPDATA 'Programs\Squallz\squallz-gui.exe'
  }
  if ($env:ProgramFiles) {
    $Candidates += Join-Path $env:ProgramFiles 'Squallz\Squallz.exe'
    $Candidates += Join-Path $env:ProgramFiles 'Squallz\squallz-gui.exe'
  }
  foreach ($Candidate in $Candidates) {
    if ($Candidate -and (Test-Path -LiteralPath $Candidate -PathType Leaf)) { return $Candidate }
  }
  foreach ($Name in @('squallz-gui.exe', 'Squallz.exe', 'squallz-gui', 'Squallz')) {
    $Command = Get-Command $Name -ErrorAction SilentlyContinue
    if ($Command) { return $Command.Source }
  }
  return $null
}

function ConvertTo-CommandLineArgument {
  param([string]$Value)
  if ($null -eq $Value) { return '""' }
  if ($Value -notmatch '[\s"]') { return $Value }
  '"' + $Value.Replace('"', '\"') + '"'
}

function Invoke-SquallzGuiTask {
  param(
    [Parameter(Mandatory = $true)][string]$Action,
    [string]$Output = '',
    [string[]]$Paths = @()
  )
  $Gui = Resolve-SquallzGui
  if (-not $Gui) { return $false }
  $Arguments = @($SquallzTaskWindowActionArg, $Action)
  if ($Output) { $Arguments += @($SquallzTaskWindowOutputArg, $Output) }
  $Arguments += $Paths
  $ArgumentLine = ($Arguments | ForEach-Object { ConvertTo-CommandLineArgument $_ }) -join ' '
  Start-Process -FilePath $Gui -ArgumentList $ArgumentLine | Out-Null
  return $true
}

function Resolve-Sqz {
  if ($env:SQUALLZ_CLI -and (Test-Path -LiteralPath $env:SQUALLZ_CLI -PathType Leaf)) {
    return $env:SQUALLZ_CLI
  }
  $Candidates = @()
  if ($env:LOCALAPPDATA) {
    $Candidates += Join-Path $env:LOCALAPPDATA 'Programs\Squallz\resources\bin\sqz.exe'
    $Candidates += Join-Path $env:LOCALAPPDATA 'Programs\Squallz\sqz.exe'
  }
  if ($env:ProgramFiles) {
    $Candidates += Join-Path $env:ProgramFiles 'Squallz\resources\bin\sqz.exe'
    $Candidates += Join-Path $env:ProgramFiles 'Squallz\sqz.exe'
  }
  foreach ($Candidate in $Candidates) {
    if ($Candidate -and (Test-Path -LiteralPath $Candidate -PathType Leaf)) { return $Candidate }
  }
  foreach ($Name in @('sqz.exe', 'sqz')) {
    $Command = Get-Command $Name -ErrorAction SilentlyContinue
    if ($Command) { return $Command.Source }
  }
  Show-SquallzCliMissing
  exit 127
}

function Invoke-Sqz {
  param([Parameter(ValueFromRemainingArguments = $true)][string[]]$Arguments)
  $Sqz = Resolve-Sqz
  & $Sqz @Arguments
}

function Select-ExistingPaths {
  param([string[]]$InputPaths)
  foreach ($InputPath in $InputPaths) {
    if ($InputPath -and (Test-Path -LiteralPath $InputPath)) { $InputPath }
  }
}

function Select-ExistingFiles {
  param([string[]]$InputPaths)
  foreach ($InputPath in $InputPaths) {
    if ($InputPath -and (Test-Path -LiteralPath $InputPath -PathType Leaf)) { $InputPath }
  }
}

function Get-ArchiveStem {
  param([string]$Path)
  $Name = [System.IO.Path]::GetFileName($Path)
  foreach ($Suffix in @('.tar.zst', '.tar.xz', '.tar.bz2', '.tar.gz', '.tbz2', '.tgz', '.txz', '.tzst', '.zip', '.7z', '.rar', '.sqz', '.tar', '.gz', '.bz2', '.xz', '.zst', '.br', '.lz4')) {
    if ($Name.EndsWith($Suffix, [System.StringComparison]::OrdinalIgnoreCase)) {
      return $Name.Substring(0, $Name.Length - $Suffix.Length)
    }
  }
  return [System.IO.Path]::GetFileNameWithoutExtension($Name)
}

function New-UniqueOutputPath {
  param([string]$Path)
  if (-not (Test-Path -LiteralPath $Path)) { return $Path }
  $Parent = Split-Path -Parent $Path
  $Base = [System.IO.Path]::GetFileNameWithoutExtension($Path)
  $Extension = [System.IO.Path]::GetExtension($Path)
  foreach ($N in 2..999) {
    $Candidate = Join-Path $Parent "$Base $N$Extension"
    if (-not (Test-Path -LiteralPath $Candidate)) { return $Candidate }
  }
  return $Path
}
"#;

pub fn apply_visible_integrations() -> io::Result<IntegrationApplyResultDto> {
    apply_visible_integrations_for_language(None)
}

pub fn apply_visible_integrations_for_language(
    language: Option<&str>,
) -> io::Result<IntegrationApplyResultDto> {
    #[cfg(target_os = "macos")]
    {
        let home = macos_home_dir()?;
        let result = install_macos_finder_actions_at_with_language(&home, language)?;
        NSUpdateDynamicServices();
        Ok(result)
    }

    #[cfg(target_os = "linux")]
    {
        let home = linux_home_dir()?;
        install_linux_file_manager_actions_at_with_language(&home, language)
    }

    #[cfg(target_os = "windows")]
    {
        let data_dir = windows_data_dir()?;
        install_windows_explorer_actions_at_with_language(&data_dir, language)
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        let _ = language;
        Ok(IntegrationApplyResultDto {
            platform: std::env::consts::OS.to_owned(),
            services_dir: String::new(),
            script_dir: String::new(),
            installed: Vec::new(),
            unsupported: vec![
                "Desktop file-manager integration is not available on this platform".to_owned(),
            ],
        })
    }
}

pub fn integration_status() -> io::Result<IntegrationStatusDto> {
    integration_status_for_language(None)
}

pub fn integration_status_for_language(language: Option<&str>) -> io::Result<IntegrationStatusDto> {
    #[cfg(target_os = "macos")]
    {
        let home = macos_home_dir()?;
        macos_finder_actions_status_at_with_language(&home, language)
    }

    #[cfg(target_os = "linux")]
    {
        let home = linux_home_dir()?;
        linux_file_manager_actions_status_at_with_language(&home, language)
    }

    #[cfg(target_os = "windows")]
    {
        let data_dir = windows_data_dir()?;
        windows_explorer_actions_status_at_with_language(&data_dir, language)
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        let _ = language;
        Ok(IntegrationStatusDto {
            platform: std::env::consts::OS.to_owned(),
            services_dir: String::new(),
            script_dir: String::new(),
            health: IntegrationHealthStateDto::Unavailable,
            actions: Vec::new(),
            can_repair: false,
            can_remove: false,
            unsupported: vec![
                "Desktop file-manager integration is not available on this platform".to_owned(),
            ],
        })
    }
}

/// Reads system-owned integration evidence without changing user preferences.
pub fn system_integration_diagnostics() -> IntegrationSystemDiagnosticsDto {
    #[cfg(target_os = "macos")]
    {
        macos_system_integration_diagnostics()
    }

    #[cfg(not(target_os = "macos"))]
    IntegrationSystemDiagnosticsDto {
        platform: std::env::consts::OS.to_owned(),
        backends: runtime_backend_statuses(),
        default_handlers: unavailable_default_handlers(),
        file_manager_visibility: IntegrationFileManagerVisibilityDto {
            state: IntegrationFileManagerVisibilityStateDto::Unsupported,
            reason: "unsupported_platform".to_owned(),
        },
    }
}

fn runtime_backend_statuses() -> Vec<RuntimeBackendStatusDto> {
    let sevenzip = squallz_formats::sevenzip_backend_status();
    let sevenzip_source = sevenzip.source().map(|source| match source {
        SevenZipBackendSource::Application => RuntimeBackendSourceDto::Application,
        SevenZipBackendSource::Environment => RuntimeBackendSourceDto::Environment,
        SevenZipBackendSource::Path => RuntimeBackendSourceDto::Path,
    });
    let unrar = squallz_formats::unrar_backend_status();
    let unrar_source = unrar.source().map(|source| match source {
        UnrarBackendSource::Environment => RuntimeBackendSourceDto::Environment,
        UnrarBackendSource::Path => RuntimeBackendSourceDto::Path,
    });
    let wimlib = squallz_formats::wimlib_backend_status();
    let wimlib_source = wimlib.source().map(|source| match source {
        WimlibBackendSource::Application => RuntimeBackendSourceDto::Application,
        WimlibBackendSource::Environment => RuntimeBackendSourceDto::Environment,
        WimlibBackendSource::Path => RuntimeBackendSourceDto::Path,
    });
    vec![
        RuntimeBackendStatusDto {
            id: "sevenzip".to_owned(),
            available: sevenzip.available(),
            configured: sevenzip.configured(),
            source: sevenzip_source,
            tool: runtime_backend_tool_name(sevenzip.executable()),
        },
        RuntimeBackendStatusDto {
            id: "unrar".to_owned(),
            available: unrar.available(),
            configured: unrar.configured(),
            source: unrar_source,
            tool: runtime_backend_tool_name(unrar.executable()),
        },
        RuntimeBackendStatusDto {
            id: "wimlib".to_owned(),
            available: wimlib.available(),
            configured: wimlib.configured(),
            source: wimlib_source,
            tool: runtime_backend_tool_name(wimlib.executable()),
        },
    ]
}

fn runtime_backend_tool_name(executable: Option<&Path>) -> Option<String> {
    executable
        .and_then(Path::file_name)
        .map(|name| name.to_string_lossy().into_owned())
}

#[cfg(test)]
mod runtime_backend_tests {
    use super::*;

    #[test]
    fn runtime_backend_diagnostics_do_not_expose_executable_paths() {
        let backends = runtime_backend_statuses();
        assert_eq!(backends.len(), 3);
        assert_eq!(backends[0].id, "sevenzip");
        assert_eq!(backends[1].id, "unrar");
        assert_eq!(backends[2].id, "wimlib");
        for backend in backends {
            if let Some(tool) = backend.tool {
                assert_eq!(Path::new(&tool).components().count(), 1);
            }
        }
    }
}

fn unavailable_default_handlers() -> IntegrationDefaultHandlersDto {
    IntegrationDefaultHandlersDto {
        state: IntegrationDefaultHandlersStateDto::Unavailable,
        total: 0,
        checked: 0,
        squallz: 0,
        handlers: Vec::new(),
    }
}

#[cfg(target_os = "macos")]
fn summarize_default_handlers(
    handlers: Vec<IntegrationDefaultHandlerDto>,
) -> IntegrationDefaultHandlersDto {
    let total = handlers.len();
    let checked = handlers
        .iter()
        .filter(|handler| handler.state != IntegrationDefaultHandlerStateDto::Unknown)
        .count();
    let squallz = handlers
        .iter()
        .filter(|handler| handler.state == IntegrationDefaultHandlerStateDto::Squallz)
        .count();
    let state = if total == 0 {
        IntegrationDefaultHandlersStateDto::Unavailable
    } else if checked != total {
        IntegrationDefaultHandlersStateDto::Unknown
    } else if squallz == total {
        IntegrationDefaultHandlersStateDto::Squallz
    } else if squallz == 0 {
        IntegrationDefaultHandlersStateDto::Other
    } else {
        IntegrationDefaultHandlersStateDto::Mixed
    };

    IntegrationDefaultHandlersDto {
        state,
        total,
        checked,
        squallz,
        handlers,
    }
}

#[cfg(target_os = "macos")]
fn macos_system_integration_diagnostics() -> IntegrationSystemDiagnosticsDto {
    let default_handlers = macos_default_handler_diagnostics()
        .map(summarize_default_handlers)
        .unwrap_or_else(|_| unavailable_default_handlers());

    IntegrationSystemDiagnosticsDto {
        platform: "macos".to_owned(),
        backends: runtime_backend_statuses(),
        default_handlers,
        file_manager_visibility: IntegrationFileManagerVisibilityDto {
            state: IntegrationFileManagerVisibilityStateDto::ManualCheck,
            reason: "not_exposed_by_platform".to_owned(),
        },
    }
}

#[cfg(target_os = "macos")]
fn macos_default_handler_diagnostics() -> io::Result<Vec<IntegrationDefaultHandlerDto>> {
    let sequence = DEFAULT_HANDLER_PROBE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let probe_dir = std::env::temp_dir().join(format!(
        "squallz-default-handlers-{}-{sequence}",
        std::process::id()
    ));
    fs::DirBuilder::new().mode(0o700).create(&probe_dir)?;

    let result = (|| {
        let mut handlers = Vec::with_capacity(MACOS_DECLARED_FILE_EXTENSIONS.len());
        for extension in MACOS_DECLARED_FILE_EXTENSIONS {
            let probe_path = probe_dir.join(format!("probe.{extension}"));
            File::options()
                .write(true)
                .create_new(true)
                .open(&probe_path)?;
            handlers.push(macos_default_handler_for_probe(extension, &probe_path));
        }
        Ok(handlers)
    })();

    let _ = fs::remove_dir_all(&probe_dir);
    result
}

#[cfg(target_os = "macos")]
fn macos_default_handler_for_probe(
    extension: &str,
    probe_path: &Path,
) -> IntegrationDefaultHandlerDto {
    autoreleasepool(|_| {
        let probe_path = probe_path.to_string_lossy();
        let probe_url = NSURL::fileURLWithPath(&NSString::from_str(&probe_path));
        let Some(application_url) =
            NSWorkspace::sharedWorkspace().URLForApplicationToOpenURL(&probe_url)
        else {
            return unknown_default_handler(extension);
        };
        let Some(bundle_identifier) = NSBundle::bundleWithURL(&application_url)
            .and_then(|bundle| bundle.bundleIdentifier())
            .map(|identifier| identifier.to_string())
        else {
            return unknown_default_handler(extension);
        };
        let application_name = application_url
            .lastPathComponent()
            .map(|name| name.to_string())
            .and_then(|name| {
                let name = name.strip_suffix(".app").unwrap_or(&name).trim();
                (!name.is_empty()).then(|| name.to_owned())
            });

        IntegrationDefaultHandlerDto {
            extension: extension.to_owned(),
            state: if bundle_identifier == SQUALLZ_BUNDLE_IDENTIFIER {
                IntegrationDefaultHandlerStateDto::Squallz
            } else {
                IntegrationDefaultHandlerStateDto::Other
            },
            application_name,
        }
    })
}

#[cfg(target_os = "macos")]
fn unknown_default_handler(extension: &str) -> IntegrationDefaultHandlerDto {
    IntegrationDefaultHandlerDto {
        extension: extension.to_owned(),
        state: IntegrationDefaultHandlerStateDto::Unknown,
        application_name: None,
    }
}

fn integration_health_state(actions: &[IntegrationActionHealthDto]) -> IntegrationHealthStateDto {
    if actions.is_empty() {
        return IntegrationHealthStateDto::Unavailable;
    }
    if actions
        .iter()
        .all(|action| action.state == IntegrationActionHealthStateDto::Healthy)
    {
        return IntegrationHealthStateDto::Healthy;
    }
    if actions
        .iter()
        .all(|action| action.state == IntegrationActionHealthStateDto::Missing)
    {
        return IntegrationHealthStateDto::Missing;
    }
    IntegrationHealthStateDto::NeedsRepair
}

fn integration_action_health(
    id: &str,
    name: &str,
    state: IntegrationActionHealthStateDto,
    issue: Option<&str>,
) -> IntegrationActionHealthDto {
    IntegrationActionHealthDto {
        id: id.to_owned(),
        name: name.to_owned(),
        state,
        issue: issue.map(ToOwned::to_owned),
    }
}

pub fn remove_visible_integrations() -> io::Result<IntegrationRemoveResultDto> {
    remove_visible_integrations_for_language(None)
}

pub fn remove_visible_integrations_for_language(
    language: Option<&str>,
) -> io::Result<IntegrationRemoveResultDto> {
    #[cfg(target_os = "macos")]
    {
        let home = macos_home_dir()?;
        let result = remove_macos_finder_actions_at_with_language(&home, language)?;
        NSUpdateDynamicServices();
        Ok(result)
    }

    #[cfg(target_os = "linux")]
    {
        let home = linux_home_dir()?;
        remove_linux_file_manager_actions_at_with_language(&home, language)
    }

    #[cfg(target_os = "windows")]
    {
        let data_dir = windows_data_dir()?;
        remove_windows_explorer_actions_at_with_language(&data_dir, language)
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        let _ = language;
        Ok(IntegrationRemoveResultDto {
            platform: std::env::consts::OS.to_owned(),
            services_dir: String::new(),
            script_dir: String::new(),
            removed: Vec::new(),
            missing: Vec::new(),
            unsupported: vec![
                "Desktop file-manager integration is not available on this platform".to_owned(),
            ],
        })
    }
}

#[cfg(target_os = "macos")]
fn macos_home_dir() -> io::Result<std::path::PathBuf> {
    dirs::home_dir().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "cannot locate the macOS home directory",
        )
    })
}

#[cfg(target_os = "linux")]
fn linux_home_dir() -> io::Result<std::path::PathBuf> {
    dirs::home_dir().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "cannot locate the Linux home directory",
        )
    })
}

#[cfg(any(target_os = "linux", all(test, target_os = "macos")))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LinuxAppImageLaunchMode {
    Mounted,
    ExtractAndRun,
}

#[cfg(any(target_os = "linux", all(test, target_os = "macos")))]
#[derive(Clone, Debug, Eq, PartialEq)]
struct LinuxAppImageLaunch {
    path: PathBuf,
    mode: LinuxAppImageLaunchMode,
}

#[cfg(any(target_os = "linux", all(test, target_os = "macos")))]
fn current_linux_appimage_launch() -> Option<LinuxAppImageLaunch> {
    let appimage = std::env::var_os("APPIMAGE");
    let appdir = std::env::var_os("APPDIR");
    let current_exe = std::env::current_exe().ok()?;
    let temp_dir = std::env::temp_dir();
    validated_linux_appimage_launch(
        appimage.as_deref().map(Path::new),
        appdir.as_deref().map(Path::new),
        &current_exe,
        &temp_dir,
    )
}

#[cfg(any(target_os = "linux", all(test, target_os = "macos")))]
fn validated_linux_appimage_launch(
    appimage: Option<&Path>,
    appdir: Option<&Path>,
    current_exe: &Path,
    temp_dir: &Path,
) -> Option<LinuxAppImageLaunch> {
    let appimage = appimage?;
    let appdir = appdir?;
    if !appimage.is_absolute() || !appdir.is_absolute() || !current_exe.is_absolute() {
        return None;
    }
    if managed_path_kind(appimage).ok()? != ManagedPathKind::RegularFile
        || !path_is_executable(appimage)
        || managed_path_kind(appdir).ok()? != ManagedPathKind::Directory
    {
        return None;
    }

    let canonical_temp_dir = fs::canonicalize(temp_dir).ok()?;
    let canonical_appdir = fs::canonicalize(appdir).ok()?;
    let appdir_name = canonical_appdir.file_name()?.to_str()?;
    if canonical_appdir.parent() != Some(canonical_temp_dir.as_path()) {
        return None;
    }
    let mode = if appdir_name.starts_with(".mount_") {
        LinuxAppImageLaunchMode::Mounted
    } else if appdir_name.starts_with("appimage_extracted_") {
        LinuxAppImageLaunchMode::ExtractAndRun
    } else {
        return None;
    };

    let canonical_exe = fs::canonicalize(current_exe).ok()?;
    if canonical_exe == canonical_appdir || !canonical_exe.starts_with(&canonical_appdir) {
        return None;
    }

    let canonical_appimage = fs::canonicalize(appimage).ok()?;
    if managed_path_kind(appimage).ok()? != ManagedPathKind::RegularFile
        || !path_is_executable(appimage)
        || managed_path_kind(&canonical_appimage).ok()? != ManagedPathKind::RegularFile
        || !path_is_executable(&canonical_appimage)
        || canonical_appimage.to_str().is_none()
    {
        return None;
    }
    Some(LinuxAppImageLaunch {
        path: canonical_appimage,
        mode,
    })
}

#[cfg(target_os = "windows")]
fn windows_data_dir() -> io::Result<PathBuf> {
    dirs::data_dir()
        .map(|dir| dir.join("Squallz"))
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "cannot locate the Windows data directory",
            )
        })
}

#[cfg(target_os = "macos")]
pub(crate) fn install_macos_finder_actions_at_with_language(
    home: &Path,
    language: Option<&str>,
) -> io::Result<IntegrationApplyResultDto> {
    let loc = Localizer::load(language);
    install_macos_finder_actions_at_with_localizer(home, &loc)
}

#[cfg(target_os = "macos")]
fn install_macos_finder_actions_at_with_localizer(
    home: &Path,
    loc: &Localizer,
) -> io::Result<IntegrationApplyResultDto> {
    let (services_dir, script_dir) = macos_integration_dirs(home);
    create_managed_directory(home, &services_dir)?;
    create_managed_directory(home, &script_dir)?;
    let preamble = finder_script_preamble(current_app_bundle_path().as_deref(), loc);

    let mut installed = Vec::new();
    for action in FINDER_ACTIONS {
        let name = action_name(action, loc);
        let workflow_dir = workflow_path_for_action(&services_dir, action);
        remove_stale_workflows(&services_dir, action, &workflow_dir)?;

        let script_path = script_dir.join(action.script_name);
        replace_managed_file(&script_path)?;
        fs::write(
            &script_path,
            format!("{preamble}\n{}", action.script_body.trim_start()),
        )?;
        make_executable(&script_path)?;

        replace_managed_directory(&workflow_dir)?;
        let contents_dir = workflow_dir.join("Contents");
        fs::create_dir(&workflow_dir)?;
        fs::create_dir(&contents_dir)?;
        fs::write(contents_dir.join("Info.plist"), info_plist(action, &name))?;
        fs::write(
            contents_dir.join("document.wflow"),
            document_workflow(&name, &script_path),
        )?;

        installed.push(action_dto_with_name(
            action,
            &name,
            &services_dir,
            &script_dir,
        ));
    }

    Ok(IntegrationApplyResultDto {
        platform: "macos".to_owned(),
        services_dir: path_to_string(&services_dir),
        script_dir: path_to_string(&script_dir),
        installed,
        unsupported: vec![
            "Windows Explorer context menus are not installed by this macOS action".to_owned(),
            "Linux file-manager actions are not installed by this macOS action".to_owned(),
        ],
    })
}

#[cfg(target_os = "macos")]
fn finder_script_preamble(installed_app_bundle: Option<&Path>, loc: &Localizer) -> String {
    let installed_app_bundle = installed_app_bundle_literal(installed_app_bundle);
    let cli_not_found_alert = shell_single_quote_value(&cli_not_found_applescript(loc));
    SCRIPT_PREAMBLE_TEMPLATE
        .replace("{installed_app_bundle}", &installed_app_bundle)
        .replace("{cli_not_found_alert}", &cli_not_found_alert)
        .replace(
            "{task_window_action_arg}",
            &shell_single_quote_value(EXTERNAL_TASK_ACTION_ARG),
        )
        .replace(
            "{task_window_output_arg}",
            &shell_single_quote_value(EXTERNAL_TASK_OUTPUT_ARG),
        )
}

#[cfg(target_os = "macos")]
fn installed_app_bundle_literal(installed_app_bundle: Option<&Path>) -> String {
    match installed_app_bundle {
        Some(path) => shell_single_quote_value(&path_to_string(path)),
        None => "''".to_owned(),
    }
}

#[cfg(target_os = "macos")]
fn cli_not_found_applescript(loc: &Localizer) -> String {
    let title = loc.t("gui.integration.finder.cli_not_found.title");
    let message = loc.t("gui.integration.finder.cli_not_found.message");
    format!(
        "display alert {} message {}",
        applescript_string(&title),
        applescript_string(&message)
    )
}

#[cfg(target_os = "macos")]
fn applescript_string(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn shell_single_quote_value(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

#[cfg(target_os = "macos")]
fn current_app_bundle_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let macos_dir = exe.parent()?;
    let contents_dir = macos_dir.parent()?;
    let app_dir = contents_dir.parent()?;
    app_dir
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("app"))
        .then(|| app_dir.to_path_buf())
}

#[cfg(target_os = "macos")]
pub(crate) fn macos_finder_actions_status_at_with_language(
    home: &Path,
    language: Option<&str>,
) -> io::Result<IntegrationStatusDto> {
    let loc = Localizer::load(language);
    macos_finder_actions_status_at_with_localizer(home, &loc)
}

#[cfg(target_os = "macos")]
fn macos_finder_actions_status_at_with_localizer(
    home: &Path,
    loc: &Localizer,
) -> io::Result<IntegrationStatusDto> {
    let (services_dir, script_dir) = macos_integration_dirs(home);
    verify_managed_directory(home, &services_dir)?;
    verify_managed_directory(home, &script_dir)?;
    let preamble = finder_script_preamble(current_app_bundle_path().as_deref(), loc);
    let mut actions = Vec::new();
    let mut can_remove = false;
    for action in FINDER_ACTIONS {
        let name = action_name(action, loc);
        let script_path = script_dir.join(action.script_name);
        let workflow_path = workflow_path_for_action(&services_dir, action);
        let script_kind = managed_path_kind(&script_path)?;
        let workflow_kind = managed_path_kind(&workflow_path)?;
        let workflows = action_workflow_dirs(&services_dir, action)?;
        let script_artifact_exists = script_kind != ManagedPathKind::Missing;
        let workflow_artifact_exists =
            workflow_kind != ManagedPathKind::Missing || !workflows.is_empty();
        can_remove |= script_artifact_exists || workflow_artifact_exists;

        let expected_script = format!("{preamble}\n{}", action.script_body.trim_start());
        let (state, issue) = if !script_artifact_exists && !workflow_artifact_exists {
            (IntegrationActionHealthStateDto::Missing, None)
        } else if !script_artifact_exists {
            (
                IntegrationActionHealthStateDto::Damaged,
                Some("script_missing"),
            )
        } else if script_kind != ManagedPathKind::RegularFile {
            (
                IntegrationActionHealthStateDto::Damaged,
                Some("script_outdated"),
            )
        } else if !workflow_artifact_exists {
            (
                IntegrationActionHealthStateDto::Damaged,
                Some("launcher_missing"),
            )
        } else if workflow_kind != ManagedPathKind::Directory {
            (
                IntegrationActionHealthStateDto::Damaged,
                Some("launcher_outdated"),
            )
        } else if !path_is_executable(&script_path) {
            (
                IntegrationActionHealthStateDto::Damaged,
                Some("script_not_executable"),
            )
        } else if !file_matches(&script_path, expected_script.as_bytes()) {
            (
                IntegrationActionHealthStateDto::Damaged,
                Some("script_outdated"),
            )
        } else if workflows.len() != 1 || workflows.first() != Some(&workflow_path) {
            (
                IntegrationActionHealthStateDto::Damaged,
                Some("launcher_outdated"),
            )
        } else {
            let info = workflow_path.join("Contents").join("Info.plist");
            let document = workflow_path.join("Contents").join("document.wflow");
            if file_matches(&info, info_plist(action, &name).as_bytes())
                && file_matches(&document, document_workflow(&name, &script_path).as_bytes())
            {
                (IntegrationActionHealthStateDto::Healthy, None)
            } else {
                (
                    IntegrationActionHealthStateDto::Damaged,
                    Some("launcher_outdated"),
                )
            }
        };
        actions.push(integration_action_health(action.id, &name, state, issue));
    }

    let health = integration_health_state(&actions);

    Ok(IntegrationStatusDto {
        platform: "macos".to_owned(),
        services_dir: path_to_string(&services_dir),
        script_dir: path_to_string(&script_dir),
        health,
        actions,
        can_repair: true,
        can_remove,
        unsupported: Vec::new(),
    })
}

#[cfg(any(target_os = "linux", all(test, target_os = "macos")))]
pub(crate) fn install_linux_file_manager_actions_at_with_language(
    home: &Path,
    language: Option<&str>,
) -> io::Result<IntegrationApplyResultDto> {
    let loc = Localizer::load(language);
    install_linux_file_manager_actions_at_with_localizer(home, &loc)
}

#[cfg(any(target_os = "linux", all(test, target_os = "macos")))]
fn install_linux_file_manager_actions_at_with_localizer(
    home: &Path,
    loc: &Localizer,
) -> io::Result<IntegrationApplyResultDto> {
    install_linux_file_manager_actions_at_with_localizer_and_appimage(
        home,
        loc,
        current_linux_appimage_launch().as_ref(),
    )
}

#[cfg(any(target_os = "linux", all(test, target_os = "macos")))]
fn install_linux_file_manager_actions_at_with_localizer_and_appimage(
    home: &Path,
    loc: &Localizer,
    appimage: Option<&LinuxAppImageLaunch>,
) -> io::Result<IntegrationApplyResultDto> {
    let (services_dir, script_dir, nautilus_dir) = linux_integration_dirs(home);
    let data_home = linux_data_home(home);
    create_managed_directory(&data_home, &services_dir)?;
    create_managed_directory(&data_home, &script_dir)?;
    create_managed_directory(&data_home, &nautilus_dir)?;
    let preamble = linux_script_preamble(loc, appimage);

    let mut installed = Vec::new();
    for action in LINUX_FILE_MANAGER_ACTIONS {
        let name = linux_action_name(action, loc);
        let script_path = script_dir.join(action.script_name);
        replace_managed_file(&script_path)?;
        fs::write(
            &script_path,
            format!("{preamble}\n{}", action.script_body.trim_start()),
        )?;
        make_executable(&script_path)?;

        let service_path = linux_service_menu_path(&services_dir, action);
        replace_managed_file(&service_path)?;
        fs::write(
            &service_path,
            linux_service_menu(action, &name, &script_path),
        )?;
        make_executable(&service_path)?;

        let nautilus_path = linux_nautilus_action_path(&nautilus_dir, &name);
        remove_stale_nautilus_scripts(&nautilus_dir, action, &nautilus_path)?;
        replace_managed_file(&nautilus_path)?;
        fs::write(
            &nautilus_path,
            linux_nautilus_launcher(action, &script_path),
        )?;
        make_executable(&nautilus_path)?;

        installed.push(linux_action_dto_with_name(
            action,
            &name,
            &services_dir,
            &script_dir,
        ));
    }

    Ok(IntegrationApplyResultDto {
        platform: "linux".to_owned(),
        services_dir: path_to_string(&services_dir),
        script_dir: path_to_string(&script_dir),
        installed,
        unsupported: vec![
            "Windows Explorer context menus are not installed by this Linux action".to_owned(),
        ],
    })
}

#[cfg(any(target_os = "linux", all(test, target_os = "macos")))]
fn linux_script_preamble(loc: &Localizer, appimage: Option<&LinuxAppImageLaunch>) -> String {
    LINUX_SCRIPT_PREAMBLE_TEMPLATE
        .replace(
            "{cli_not_found_title}",
            &shell_single_quote_value(&loc.t("gui.integration.file_manager.cli_not_found.title")),
        )
        .replace(
            "{cli_not_found_message}",
            &shell_single_quote_value(&loc.t("gui.integration.file_manager.cli_not_found.message")),
        )
        .replace(
            "{task_window_action_arg}",
            &shell_single_quote_value(EXTERNAL_TASK_ACTION_ARG),
        )
        .replace(
            "{task_window_output_arg}",
            &shell_single_quote_value(EXTERNAL_TASK_OUTPUT_ARG),
        )
        .replace(
            "{installed_appimage}",
            &appimage
                .and_then(|launch| launch.path.to_str())
                .map(shell_single_quote_value)
                .unwrap_or_else(|| "''".to_owned()),
        )
        .replace(
            "{installed_appimage_extract_and_run}",
            if appimage.is_some_and(|launch| launch.mode == LinuxAppImageLaunchMode::ExtractAndRun)
            {
                "'1'"
            } else {
                "'0'"
            },
        )
}

#[cfg(any(target_os = "linux", all(test, target_os = "macos")))]
pub(crate) fn linux_file_manager_actions_status_at_with_language(
    home: &Path,
    language: Option<&str>,
) -> io::Result<IntegrationStatusDto> {
    let loc = Localizer::load(language);
    linux_file_manager_actions_status_at_with_localizer(home, &loc)
}

#[cfg(any(target_os = "linux", all(test, target_os = "macos")))]
fn linux_file_manager_actions_status_at_with_localizer(
    home: &Path,
    loc: &Localizer,
) -> io::Result<IntegrationStatusDto> {
    linux_file_manager_actions_status_at_with_localizer_and_appimage(
        home,
        loc,
        current_linux_appimage_launch().as_ref(),
    )
}

#[cfg(any(target_os = "linux", all(test, target_os = "macos")))]
fn linux_file_manager_actions_status_at_with_localizer_and_appimage(
    home: &Path,
    loc: &Localizer,
    appimage: Option<&LinuxAppImageLaunch>,
) -> io::Result<IntegrationStatusDto> {
    let (services_dir, script_dir, nautilus_dir) = linux_integration_dirs(home);
    let data_home = linux_data_home(home);
    verify_managed_directory(&data_home, &services_dir)?;
    verify_managed_directory(&data_home, &script_dir)?;
    verify_managed_directory(&data_home, &nautilus_dir)?;
    let preamble = linux_script_preamble(loc, appimage);
    let mut actions = Vec::new();
    let mut can_remove = false;
    for action in LINUX_FILE_MANAGER_ACTIONS {
        let name = linux_action_name(action, loc);
        let script_path = script_dir.join(action.script_name);
        let service_path = linux_service_menu_path(&services_dir, action);
        let expected_nautilus_path = linux_nautilus_action_path(&nautilus_dir, &name);
        let script_kind = managed_path_kind(&script_path)?;
        let service_kind = managed_path_kind(&service_path)?;
        let nautilus_kind = managed_path_kind(&expected_nautilus_path)?;
        let nautilus_scripts = action_nautilus_scripts(&nautilus_dir, action)?;
        let script_artifact_exists = script_kind != ManagedPathKind::Missing;
        let service_artifact_exists = service_kind != ManagedPathKind::Missing;
        let nautilus_artifact_exists =
            nautilus_kind != ManagedPathKind::Missing || !nautilus_scripts.is_empty();
        can_remove |= script_artifact_exists || service_artifact_exists || nautilus_artifact_exists;

        let expected_script = format!("{preamble}\n{}", action.script_body.trim_start());
        let (state, issue) =
            if !script_artifact_exists && !service_artifact_exists && !nautilus_artifact_exists {
                (IntegrationActionHealthStateDto::Missing, None)
            } else if !script_artifact_exists {
                (
                    IntegrationActionHealthStateDto::Damaged,
                    Some("script_missing"),
                )
            } else if script_kind != ManagedPathKind::RegularFile {
                (
                    IntegrationActionHealthStateDto::Damaged,
                    Some("script_outdated"),
                )
            } else if !service_artifact_exists || !nautilus_artifact_exists {
                (
                    IntegrationActionHealthStateDto::Damaged,
                    Some("launcher_missing"),
                )
            } else if service_kind != ManagedPathKind::RegularFile
                || nautilus_kind != ManagedPathKind::RegularFile
            {
                (
                    IntegrationActionHealthStateDto::Damaged,
                    Some("launcher_outdated"),
                )
            } else if !path_is_executable(&script_path)
                || !path_is_executable(&service_path)
                || !path_is_executable(&expected_nautilus_path)
            {
                (
                    IntegrationActionHealthStateDto::Damaged,
                    Some("script_not_executable"),
                )
            } else if !file_matches(&script_path, expected_script.as_bytes()) {
                (
                    IntegrationActionHealthStateDto::Damaged,
                    Some("script_outdated"),
                )
            } else if nautilus_scripts.len() != 1
                || nautilus_scripts.first() != Some(&expected_nautilus_path)
                || !file_matches(
                    &service_path,
                    linux_service_menu(action, &name, &script_path).as_bytes(),
                )
                || !file_matches(
                    &expected_nautilus_path,
                    linux_nautilus_launcher(action, &script_path).as_bytes(),
                )
            {
                (
                    IntegrationActionHealthStateDto::Damaged,
                    Some("launcher_outdated"),
                )
            } else {
                (IntegrationActionHealthStateDto::Healthy, None)
            };
        actions.push(integration_action_health(action.id, &name, state, issue));
    }

    let health = integration_health_state(&actions);

    Ok(IntegrationStatusDto {
        platform: "linux".to_owned(),
        services_dir: path_to_string(&services_dir),
        script_dir: path_to_string(&script_dir),
        health,
        actions,
        can_repair: true,
        can_remove,
        unsupported: Vec::new(),
    })
}

#[cfg(any(target_os = "linux", all(test, target_os = "macos")))]
pub(crate) fn remove_linux_file_manager_actions_at_with_language(
    home: &Path,
    language: Option<&str>,
) -> io::Result<IntegrationRemoveResultDto> {
    let loc = Localizer::load(language);
    remove_linux_file_manager_actions_at_with_localizer(home, &loc)
}

#[cfg(any(target_os = "linux", all(test, target_os = "macos")))]
fn remove_linux_file_manager_actions_at_with_localizer(
    home: &Path,
    loc: &Localizer,
) -> io::Result<IntegrationRemoveResultDto> {
    let (services_dir, script_dir, nautilus_dir) = linux_integration_dirs(home);
    let data_home = linux_data_home(home);
    verify_managed_directory(&data_home, &services_dir)?;
    verify_managed_directory(&data_home, &script_dir)?;
    verify_managed_directory(&data_home, &nautilus_dir)?;
    let mut removed = Vec::new();
    let mut missing = Vec::new();

    for action in LINUX_FILE_MANAGER_ACTIONS {
        let script = script_dir.join(action.script_name);
        let service = linux_service_menu_path(&services_dir, action);
        let name = linux_action_name(action, loc);
        let expected_nautilus = linux_nautilus_action_path(&nautilus_dir, &name);
        let mut existed = remove_owned_file(&script)?;
        existed |= remove_owned_file(&service)?;
        for nautilus in action_nautilus_scripts(&nautilus_dir, action)? {
            if nautilus != expected_nautilus {
                existed |= remove_owned_file(&nautilus)?;
            }
        }
        existed |= remove_owned_file(&expected_nautilus)?;

        if existed {
            removed.push(linux_action_dto_with_name(
                action,
                &name,
                &services_dir,
                &script_dir,
            ));
        } else {
            missing.push(name);
        }
    }

    if directory_is_empty(&nautilus_dir) {
        let _ = fs::remove_dir(&nautilus_dir);
    }
    if directory_is_empty(&script_dir) {
        let _ = fs::remove_dir(&script_dir);
    }

    Ok(IntegrationRemoveResultDto {
        platform: "linux".to_owned(),
        services_dir: path_to_string(&services_dir),
        script_dir: path_to_string(&script_dir),
        removed,
        missing,
        unsupported: Vec::new(),
    })
}

#[cfg(any(target_os = "windows", all(test, target_os = "macos")))]
pub(crate) fn install_windows_explorer_actions_at_with_language(
    data_dir: &Path,
    language: Option<&str>,
) -> io::Result<IntegrationApplyResultDto> {
    let loc = Localizer::load(language);
    install_windows_explorer_actions_at_with_localizer(data_dir, &loc)
}

#[cfg(any(target_os = "windows", all(test, target_os = "macos")))]
fn install_windows_explorer_actions_at_with_localizer(
    data_dir: &Path,
    loc: &Localizer,
) -> io::Result<IntegrationApplyResultDto> {
    let (services_dir, script_dir) = windows_integration_dirs(data_dir);
    create_managed_directory(data_dir, &script_dir)?;
    let preamble = windows_script_preamble(loc);

    let mut installed = Vec::new();
    for action in WINDOWS_EXPLORER_ACTIONS {
        let name = windows_action_name(action, loc);
        let script_path = script_dir.join(action.script_name);
        replace_managed_file(&script_path)?;
        fs::write(
            &script_path,
            format!("{preamble}\n{}", action.script_body.trim_start()),
        )?;
        installed.push(windows_action_dto_with_name(
            action,
            &name,
            &services_dir,
            &script_dir,
        ));
    }

    let manifest_path = windows_registry_manifest_path(&script_dir);
    replace_managed_file(&manifest_path)?;
    fs::write(manifest_path, windows_registry_manifest(&script_dir, loc))?;
    apply_windows_explorer_registry_entries(&script_dir, loc)?;

    Ok(IntegrationApplyResultDto {
        platform: "windows".to_owned(),
        services_dir,
        script_dir: path_to_string(&script_dir),
        installed,
        unsupported: Vec::new(),
    })
}

#[cfg(any(target_os = "windows", all(test, target_os = "macos")))]
pub(crate) fn windows_explorer_actions_status_at_with_language(
    data_dir: &Path,
    language: Option<&str>,
) -> io::Result<IntegrationStatusDto> {
    let loc = Localizer::load(language);
    windows_explorer_actions_status_at_with_localizer(data_dir, &loc)
}

#[cfg(any(target_os = "windows", all(test, target_os = "macos")))]
fn windows_explorer_actions_status_at_with_localizer(
    data_dir: &Path,
    loc: &Localizer,
) -> io::Result<IntegrationStatusDto> {
    let (services_dir, script_dir) = windows_integration_dirs(data_dir);
    verify_managed_directory(data_dir, &script_dir)?;
    let preamble = windows_script_preamble(loc);
    let mut actions = Vec::new();
    let manifest_path = windows_registry_manifest_path(&script_dir);
    let manifest_kind = managed_path_kind(&manifest_path)?;
    let manifest_exists = manifest_kind != ManagedPathKind::Missing;
    let manifest_matches = file_matches(
        &manifest_path,
        windows_registry_manifest(&script_dir, loc).as_bytes(),
    );
    let mut can_remove = manifest_exists;
    for action in WINDOWS_EXPLORER_ACTIONS {
        let name = windows_action_name(action, loc);
        let script_path = script_dir.join(action.script_name);
        let script_kind = managed_path_kind(&script_path)?;
        let script_artifact_exists = script_kind != ManagedPathKind::Missing;
        let registry_present = windows_explorer_registry_entries_present(&script_dir, action);
        let registry_matches = windows_explorer_registry_entries_match(&script_dir, action, loc);
        can_remove |= script_artifact_exists || registry_present;

        let expected_script = format!("{preamble}\n{}", action.script_body.trim_start());
        let (state, issue) = if !script_artifact_exists && !registry_present && !manifest_exists {
            (IntegrationActionHealthStateDto::Missing, None)
        } else if !script_artifact_exists {
            (
                IntegrationActionHealthStateDto::Damaged,
                Some("script_missing"),
            )
        } else if script_kind != ManagedPathKind::RegularFile {
            (
                IntegrationActionHealthStateDto::Damaged,
                Some("script_outdated"),
            )
        } else if !registry_present {
            (
                IntegrationActionHealthStateDto::Damaged,
                Some("registry_missing"),
            )
        } else if !file_matches(&script_path, expected_script.as_bytes()) {
            (
                IntegrationActionHealthStateDto::Damaged,
                Some("script_outdated"),
            )
        } else if !registry_matches || !manifest_matches {
            (
                IntegrationActionHealthStateDto::Damaged,
                Some("registry_outdated"),
            )
        } else {
            (IntegrationActionHealthStateDto::Healthy, None)
        };
        actions.push(integration_action_health(action.id, &name, state, issue));
    }

    let health = integration_health_state(&actions);

    Ok(IntegrationStatusDto {
        platform: "windows".to_owned(),
        services_dir,
        script_dir: path_to_string(&script_dir),
        health,
        actions,
        can_repair: true,
        can_remove,
        unsupported: Vec::new(),
    })
}

#[cfg(any(target_os = "windows", all(test, target_os = "macos")))]
pub(crate) fn remove_windows_explorer_actions_at_with_language(
    data_dir: &Path,
    language: Option<&str>,
) -> io::Result<IntegrationRemoveResultDto> {
    let loc = Localizer::load(language);
    remove_windows_explorer_actions_at_with_localizer(data_dir, &loc)
}

#[cfg(any(target_os = "windows", all(test, target_os = "macos")))]
fn remove_windows_explorer_actions_at_with_localizer(
    data_dir: &Path,
    loc: &Localizer,
) -> io::Result<IntegrationRemoveResultDto> {
    let (services_dir, script_dir) = windows_integration_dirs(data_dir);
    verify_managed_directory(data_dir, &script_dir)?;
    let mut removed = Vec::new();
    let mut missing = Vec::new();
    let manifest = windows_registry_manifest_path(&script_dir);
    let manifest_exists = managed_path_kind(&manifest)? != ManagedPathKind::Missing;

    for action in WINDOWS_EXPLORER_ACTIONS {
        let script = script_dir.join(action.script_name);
        let existed = managed_path_kind(&script)? != ManagedPathKind::Missing
            || windows_explorer_registry_entries_present(&script_dir, action)
            || manifest_exists;

        remove_windows_explorer_registry_entries(action)?;
        let script_removed = remove_owned_file(&script)?;

        let name = windows_action_name(action, loc);
        if existed || script_removed {
            removed.push(windows_action_dto_with_name(
                action,
                &name,
                &services_dir,
                &script_dir,
            ));
        } else {
            missing.push(name);
        }
    }

    let _ = remove_owned_file(&manifest)?;
    if directory_is_empty(&script_dir) {
        let _ = fs::remove_dir(&script_dir);
    }

    Ok(IntegrationRemoveResultDto {
        platform: "windows".to_owned(),
        services_dir,
        script_dir: path_to_string(&script_dir),
        removed,
        missing,
        unsupported: Vec::new(),
    })
}

#[cfg(target_os = "macos")]
pub(crate) fn remove_macos_finder_actions_at_with_language(
    home: &Path,
    language: Option<&str>,
) -> io::Result<IntegrationRemoveResultDto> {
    let loc = Localizer::load(language);
    remove_macos_finder_actions_at_with_localizer(home, &loc)
}

#[cfg(target_os = "macos")]
fn remove_macos_finder_actions_at_with_localizer(
    home: &Path,
    loc: &Localizer,
) -> io::Result<IntegrationRemoveResultDto> {
    let (services_dir, script_dir) = macos_integration_dirs(home);
    verify_managed_directory(home, &services_dir)?;
    verify_managed_directory(home, &script_dir)?;
    let mut removed = Vec::new();
    let mut missing = Vec::new();

    for action in FINDER_ACTIONS {
        let script = script_dir.join(action.script_name);
        let fixed_workflow = workflow_path_for_action(&services_dir, action);
        let mut existed = managed_path_kind(&script)? != ManagedPathKind::Missing;
        for workflow in action_workflow_dirs(&services_dir, action)? {
            if workflow != fixed_workflow {
                existed |= remove_owned_directory(&workflow)?;
            }
        }
        existed |= remove_owned_directory(&fixed_workflow)?;
        existed |= remove_owned_file(&script)?;
        if existed {
            let name = action_name(action, loc);
            removed.push(action_dto_with_name(
                action,
                &name,
                &services_dir,
                &script_dir,
            ));
        } else {
            missing.push(action_name(action, loc));
        }
    }

    if directory_is_empty(&script_dir) {
        let _ = fs::remove_dir(&script_dir);
    }

    Ok(IntegrationRemoveResultDto {
        platform: "macos".to_owned(),
        services_dir: path_to_string(&services_dir),
        script_dir: path_to_string(&script_dir),
        removed,
        missing,
        unsupported: Vec::new(),
    })
}

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ManagedPathKind {
    Missing,
    RegularFile,
    Directory,
    SymbolicLink,
    Other,
}

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
fn managed_path_kind(path: &Path) -> io::Result<ManagedPathKind> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(ManagedPathKind::Missing);
        }
        Err(error) => return Err(error),
    };
    let file_type = metadata.file_type();
    if file_type.is_symlink() {
        Ok(ManagedPathKind::SymbolicLink)
    } else if file_type.is_file() {
        Ok(ManagedPathKind::RegularFile)
    } else if file_type.is_dir() {
        Ok(ManagedPathKind::Directory)
    } else {
        Ok(ManagedPathKind::Other)
    }
}

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
fn replace_managed_file(path: &Path) -> io::Result<()> {
    match managed_path_kind(path)? {
        ManagedPathKind::Missing => Ok(()),
        ManagedPathKind::RegularFile => fs::remove_file(path),
        ManagedPathKind::Directory | ManagedPathKind::SymbolicLink | ManagedPathKind::Other => {
            Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "refusing to replace an unexpected integration file type",
            ))
        }
    }
}

#[cfg(target_os = "macos")]
fn replace_managed_directory(path: &Path) -> io::Result<()> {
    match managed_path_kind(path)? {
        ManagedPathKind::Missing => Ok(()),
        ManagedPathKind::Directory => fs::remove_dir_all(path),
        ManagedPathKind::RegularFile | ManagedPathKind::SymbolicLink | ManagedPathKind::Other => {
            Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "refusing to replace an unexpected integration directory type",
            ))
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
fn remove_owned_file(path: &Path) -> io::Result<bool> {
    match managed_path_kind(path)? {
        ManagedPathKind::Missing => Ok(false),
        ManagedPathKind::Directory => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "refusing to remove a directory where an integration file is expected",
        )),
        ManagedPathKind::RegularFile | ManagedPathKind::SymbolicLink | ManagedPathKind::Other => {
            fs::remove_file(path)?;
            Ok(true)
        }
    }
}

#[cfg(target_os = "macos")]
fn remove_owned_directory(path: &Path) -> io::Result<bool> {
    match managed_path_kind(path)? {
        ManagedPathKind::Missing => Ok(false),
        ManagedPathKind::Directory => fs::remove_dir_all(path).map(|()| true),
        ManagedPathKind::RegularFile | ManagedPathKind::SymbolicLink | ManagedPathKind::Other => {
            fs::remove_file(path).map(|()| true)
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
fn create_managed_directory(root: &Path, path: &Path) -> io::Result<()> {
    fs::create_dir_all(root)?;
    let relative = path.strip_prefix(root).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "integration directory is outside its managed root",
        )
    })?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        let std::path::Component::Normal(name) = component else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "integration directory contains an invalid component",
            ));
        };
        current.push(name);
        match managed_path_kind(&current)? {
            ManagedPathKind::Missing => fs::create_dir(&current)?,
            ManagedPathKind::Directory => {}
            ManagedPathKind::RegularFile
            | ManagedPathKind::SymbolicLink
            | ManagedPathKind::Other => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "integration directory contains an unexpected file type",
                ));
            }
        }
    }
    Ok(())
}

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
fn verify_managed_directory(root: &Path, path: &Path) -> io::Result<()> {
    let relative = path.strip_prefix(root).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "integration directory is outside its managed root",
        )
    })?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        let std::path::Component::Normal(name) = component else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "integration directory contains an invalid component",
            ));
        };
        current.push(name);
        match managed_path_kind(&current)? {
            ManagedPathKind::Missing => return Ok(()),
            ManagedPathKind::Directory => {}
            ManagedPathKind::RegularFile
            | ManagedPathKind::SymbolicLink
            | ManagedPathKind::Other => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "integration directory contains an unexpected file type",
                ));
            }
        }
    }
    Ok(())
}

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
fn file_matches(path: &Path, expected: &[u8]) -> bool {
    if !matches!(managed_path_kind(path), Ok(ManagedPathKind::RegularFile)) {
        return false;
    }
    let Ok(mut file) = File::open(path) else {
        return false;
    };
    let Ok(metadata) = file.metadata() else {
        return false;
    };
    let Ok(expected_len) = u64::try_from(expected.len()) else {
        return false;
    };
    if metadata.len() != expected_len {
        return false;
    }

    let mut actual = vec![0; expected.len()];
    file.read_exact(&mut actual).is_ok() && actual == expected
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn path_is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    matches!(managed_path_kind(path), Ok(ManagedPathKind::RegularFile))
        && fs::symlink_metadata(path)
            .is_ok_and(|metadata| metadata.permissions().mode() & 0o111 != 0)
}

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
fn directory_is_empty(path: &Path) -> bool {
    match path.read_dir() {
        Ok(mut entries) => entries.next().is_none(),
        Err(e) => {
            log::debug!(
                "integration cleanup: cannot inspect {}: {e}",
                path.display()
            );
            false
        }
    }
}

#[cfg(target_os = "macos")]
fn macos_integration_dirs(home: &Path) -> (std::path::PathBuf, std::path::PathBuf) {
    (
        home.join("Library").join("Services"),
        home.join("Library")
            .join("Application Support")
            .join("Squallz")
            .join("context-actions"),
    )
}

#[cfg(any(target_os = "linux", all(test, target_os = "macos")))]
fn linux_integration_dirs(home: &Path) -> (PathBuf, PathBuf, PathBuf) {
    let data_home = linux_data_home(home);
    (
        data_home.join("kio").join("servicemenus"),
        data_home.join("squallz").join("context-actions"),
        data_home.join("nautilus").join("scripts").join("Squallz"),
    )
}

#[cfg(any(target_os = "linux", all(test, target_os = "macos")))]
fn linux_data_home(home: &Path) -> PathBuf {
    let value = std::env::var_os("XDG_DATA_HOME");
    linux_data_home_from_env(home, value.as_deref())
}

#[cfg(any(target_os = "linux", all(test, target_os = "macos")))]
fn linux_data_home_from_env(home: &Path, value: Option<&std::ffi::OsStr>) -> PathBuf {
    match value
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
    {
        Some(path) => path,
        None => home.join(".local").join("share"),
    }
}

#[cfg(any(target_os = "linux", all(test, target_os = "macos")))]
fn linux_service_menu_path(services_dir: &Path, action: &LinuxFileManagerAction) -> PathBuf {
    services_dir.join(action.desktop_name)
}

#[cfg(any(target_os = "linux", all(test, target_os = "macos")))]
fn linux_nautilus_action_path(nautilus_dir: &Path, name: &str) -> PathBuf {
    nautilus_dir.join(safe_visible_file_name(name))
}

#[cfg(any(target_os = "linux", all(test, target_os = "macos")))]
fn safe_visible_file_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for ch in name.chars() {
        if ch == '/' || ch == '\0' {
            out.push('_');
        } else {
            out.push(ch);
        }
    }
    let trimmed = out.trim();
    if trimmed.is_empty() || matches!(trimmed, "." | "..") {
        "Squallz Action".to_owned()
    } else {
        out
    }
}

#[cfg(any(target_os = "linux", all(test, target_os = "macos")))]
fn linux_action_name(action: &LinuxFileManagerAction, loc: &Localizer) -> String {
    loc.t(action.name_key)
}

#[cfg(any(target_os = "linux", all(test, target_os = "macos")))]
fn linux_service_menu(action: &LinuxFileManagerAction, name: &str, script_path: &Path) -> String {
    let action_id = format!("squallz-{}", action.id);
    format!(
        r#"[Desktop Entry]
Type=Service
ServiceTypes=KonqPopupMenu/Plugin
MimeType=all/all;all/allfiles;inode/directory;
Actions={};
X-KDE-Priority=TopLevel
X-KDE-Submenu=Squallz

[Desktop Action {}]
Name={}
Icon=application-x-archive
Exec={} %F
"#,
        desktop_entry_escape(&action_id),
        desktop_entry_escape(&action_id),
        desktop_entry_escape(name),
        desktop_exec_argument(&path_to_string(script_path)),
    )
}

#[cfg(any(target_os = "linux", all(test, target_os = "macos")))]
fn linux_nautilus_launcher(action: &LinuxFileManagerAction, script_path: &Path) -> String {
    format!(
        r#"#!/usr/bin/env bash
set -euo pipefail
# SQUALLZ_ACTION_ID={}
exec {} "$@"
"#,
        action.id,
        shell_single_quote_value(&path_to_string(script_path)),
    )
}

#[cfg(any(target_os = "linux", all(test, target_os = "macos")))]
fn remove_stale_nautilus_scripts(
    nautilus_dir: &Path,
    action: &LinuxFileManagerAction,
    selected_script: &Path,
) -> io::Result<()> {
    for script in action_nautilus_scripts(nautilus_dir, action)? {
        if script == selected_script {
            continue;
        }
        fs::remove_file(script)?;
    }
    Ok(())
}

#[cfg(any(target_os = "linux", all(test, target_os = "macos")))]
fn action_nautilus_scripts(
    nautilus_dir: &Path,
    action: &LinuxFileManagerAction,
) -> io::Result<Vec<PathBuf>> {
    let entries = match fs::read_dir(nautilus_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let marker = format!("SQUALLZ_ACTION_ID={}", action.id);
    let mut scripts = Vec::new();
    for entry in entries {
        let path = entry?.path();
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            continue;
        }
        if file_contains(&path, &marker) {
            scripts.push(path);
        }
    }
    scripts.sort();
    Ok(scripts)
}

#[cfg(any(target_os = "linux", all(test, target_os = "macos")))]
fn linux_action_dto_with_name(
    action: &LinuxFileManagerAction,
    name: &str,
    services_dir: &Path,
    script_dir: &Path,
) -> IntegrationActionDto {
    IntegrationActionDto {
        id: action.id.to_owned(),
        name: name.to_owned(),
        kind: "linux_file_manager_action".to_owned(),
        path: path_to_string(&linux_service_menu_path(services_dir, action)),
        script_path: path_to_string(&script_dir.join(action.script_name)),
    }
}

#[cfg(any(target_os = "linux", all(test, target_os = "macos")))]
fn file_contains(path: &Path, needle: &str) -> bool {
    match fs::read_to_string(path) {
        Ok(contents) => contents.contains(needle),
        Err(e) => {
            log::debug!("integration status: cannot read {}: {e}", path.display());
            false
        }
    }
}

#[cfg(any(target_os = "linux", all(test, target_os = "macos")))]
fn desktop_entry_escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('\n', "\\n")
        .replace('\r', "")
}

#[cfg(any(target_os = "linux", all(test, target_os = "macos")))]
fn desktop_exec_argument(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len() + 2);
    escaped.push('"');
    for character in value.chars() {
        match character {
            '\\' => escaped.push_str("\\\\\\\\"),
            '"' | '$' | '`' => {
                escaped.push('\\');
                escaped.push('\\');
                escaped.push(character);
            }
            '%' => escaped.push_str("%%"),
            '\n' => escaped.push_str("\\n"),
            '\t' => escaped.push_str("\\t"),
            '\r' => escaped.push_str("\\r"),
            _ => escaped.push(character),
        }
    }
    escaped.push('"');
    escaped
}

#[cfg(any(target_os = "windows", all(test, target_os = "macos")))]
const WINDOWS_ARCHIVE_EXTENSIONS: &[&str] = &[
    ".zip", ".jar", ".apk", ".cbz", ".ipa", ".7z", ".rar", ".cbr", ".sqz", ".tar", ".tgz", ".tbz2",
    ".txz", ".tzst", ".tar.gz", ".tar.bz2", ".tar.xz", ".tar.zst", ".gz", ".bz2", ".xz", ".zst",
    ".br", ".lz4",
];

#[cfg(any(target_os = "windows", all(test, target_os = "macos")))]
fn windows_integration_dirs(data_dir: &Path) -> (String, PathBuf) {
    (
        windows_registry_root().to_owned(),
        data_dir.join("context-actions"),
    )
}

#[cfg(any(target_os = "windows", all(test, target_os = "macos")))]
fn windows_registry_root() -> &'static str {
    #[cfg(all(test, target_os = "windows"))]
    {
        static TEST_ROOT: std::sync::OnceLock<String> = std::sync::OnceLock::new();
        return TEST_ROOT
            .get_or_init(|| {
                format!(
                    "HKEY_CURRENT_USER\\Software\\Squallz\\Tests\\{}",
                    std::process::id()
                )
            })
            .as_str();
    }

    #[cfg(not(all(test, target_os = "windows")))]
    "HKEY_CURRENT_USER\\Software\\Classes"
}

#[cfg(any(target_os = "windows", all(test, target_os = "macos")))]
fn windows_registry_manifest_path(script_dir: &Path) -> PathBuf {
    script_dir.join("squallz-explorer-context.reg")
}

#[cfg(any(target_os = "windows", all(test, target_os = "macos")))]
fn windows_script_preamble(loc: &Localizer) -> String {
    WINDOWS_SCRIPT_PREAMBLE_TEMPLATE
        .replace(
            "{cli_not_found_title}",
            &powershell_single_quote_value(&loc.t("gui.integration.explorer.cli_not_found.title")),
        )
        .replace(
            "{cli_not_found_message}",
            &powershell_single_quote_value(
                &loc.t("gui.integration.explorer.cli_not_found.message"),
            ),
        )
        .replace(
            "{task_window_action_arg}",
            &powershell_single_quote_value(EXTERNAL_TASK_ACTION_ARG),
        )
        .replace(
            "{task_window_output_arg}",
            &powershell_single_quote_value(EXTERNAL_TASK_OUTPUT_ARG),
        )
}

#[cfg(any(target_os = "windows", all(test, target_os = "macos")))]
fn windows_action_name(action: &WindowsExplorerAction, loc: &Localizer) -> String {
    loc.t(action.name_key)
}

#[cfg(any(target_os = "windows", all(test, target_os = "macos")))]
fn windows_action_dto_with_name(
    action: &WindowsExplorerAction,
    name: &str,
    _services_dir: &str,
    script_dir: &Path,
) -> IntegrationActionDto {
    IntegrationActionDto {
        id: action.id.to_owned(),
        name: name.to_owned(),
        kind: "windows_explorer_context_verb".to_owned(),
        path: windows_registry_keys(action).join("; "),
        script_path: path_to_string(&script_dir.join(action.script_name)),
    }
}

#[cfg(any(target_os = "windows", all(test, target_os = "macos")))]
fn windows_registry_manifest(script_dir: &Path, loc: &Localizer) -> String {
    let mut lines = vec![
        "Windows Registry Editor Version 5.00".to_owned(),
        String::new(),
        "; Classic per-user Explorer verbs under HKCU. On Windows 11 these".to_owned(),
        "; remain a Show more options bridge until signed IExplorerCommand packaging lands."
            .to_owned(),
        String::new(),
    ];

    for action in WINDOWS_EXPLORER_ACTIONS {
        let name = windows_action_name(action, loc);
        let script_path = script_dir.join(action.script_name);
        let command = windows_registry_command(&script_path);
        for key in windows_registry_keys(action) {
            lines.push(format!("[{key}]"));
            lines.push(format!("@={}", windows_registry_value(&name)));
            lines.push(format!(
                "\"Icon\"={}",
                windows_registry_value("squallz-gui.exe")
            ));
            lines.push(format!(
                "\"MultiSelectModel\"={}",
                windows_registry_value("Player")
            ));
            lines.push(String::new());
            lines.push(format!("[{key}\\command]"));
            lines.push(format!("@={}", windows_registry_value(&command)));
            lines.push(String::new());
        }
    }

    lines.join("\n")
}

#[cfg(any(target_os = "windows", all(test, target_os = "macos")))]
fn windows_registry_keys(action: &WindowsExplorerAction) -> Vec<String> {
    let verb = windows_registry_verb(action);
    let root = windows_registry_root();
    match action.id {
        "checksum" => vec![format!("{root}\\*\\shell\\{verb}")],
        "compress-to-7z" => vec![
            format!("{root}\\*\\shell\\{verb}"),
            format!("{root}\\Directory\\shell\\{verb}"),
        ],
        _ => WINDOWS_ARCHIVE_EXTENSIONS
            .iter()
            .map(|ext| format!("{root}\\SystemFileAssociations\\{ext}\\shell\\{verb}"))
            .collect(),
    }
}

#[cfg(any(target_os = "windows", all(test, target_os = "macos")))]
fn windows_registry_verb(action: &WindowsExplorerAction) -> String {
    format!("Squallz.{}", action.id)
}

#[cfg(any(target_os = "windows", all(test, target_os = "macos")))]
fn windows_registry_command(script_path: &Path) -> String {
    format!(
        "powershell.exe -NoProfile -ExecutionPolicy Bypass -File {} \"%1\"",
        windows_command_argument(&path_to_string(script_path))
    )
}

#[cfg(any(target_os = "windows", all(test, target_os = "macos")))]
fn windows_command_argument(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\\\""))
}

#[cfg(any(target_os = "windows", all(test, target_os = "macos")))]
fn windows_registry_value(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

#[cfg(any(target_os = "windows", all(test, target_os = "macos")))]
fn powershell_single_quote_value(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

#[cfg(target_os = "windows")]
fn apply_windows_explorer_registry_entries(script_dir: &Path, loc: &Localizer) -> io::Result<()> {
    for action in WINDOWS_EXPLORER_ACTIONS {
        let name = windows_action_name(action, loc);
        let command = windows_registry_command(&script_dir.join(action.script_name));
        for key in windows_registry_keys(action) {
            windows_reg_add_default(&key, &name)?;
            windows_reg_add_value(&key, "Icon", "squallz-gui.exe")?;
            windows_reg_add_value(&key, "MultiSelectModel", "Player")?;
            windows_reg_add_default(&format!("{key}\\command"), &command)?;
        }
    }
    Ok(())
}

#[cfg(all(test, target_os = "macos"))]
fn apply_windows_explorer_registry_entries(_script_dir: &Path, _loc: &Localizer) -> io::Result<()> {
    Ok(())
}

#[cfg(target_os = "windows")]
fn remove_windows_explorer_registry_entries(action: &WindowsExplorerAction) -> io::Result<()> {
    for key in windows_registry_keys(action) {
        if windows_registry_key_exists(&key) {
            windows_reg_delete_key(&key)?;
        }
    }
    Ok(())
}

#[cfg(all(test, target_os = "macos"))]
fn remove_windows_explorer_registry_entries(_action: &WindowsExplorerAction) -> io::Result<()> {
    Ok(())
}

#[cfg(target_os = "windows")]
fn windows_explorer_registry_entries_present(
    _script_dir: &Path,
    action: &WindowsExplorerAction,
) -> bool {
    windows_registry_keys(action)
        .iter()
        .any(|key| windows_registry_key_exists(key))
}

#[cfg(all(test, target_os = "macos"))]
fn windows_explorer_registry_entries_present(
    script_dir: &Path,
    action: &WindowsExplorerAction,
) -> bool {
    let manifest = windows_registry_manifest_path(script_dir);
    let Ok(contents) = fs::read_to_string(manifest) else {
        return false;
    };
    windows_registry_keys(action)
        .iter()
        .any(|key| contents.contains(key))
}

#[cfg(target_os = "windows")]
fn windows_explorer_registry_entries_match(
    script_dir: &Path,
    action: &WindowsExplorerAction,
    loc: &Localizer,
) -> bool {
    let name = windows_action_name(action, loc);
    let command = windows_registry_command(&script_dir.join(action.script_name));
    windows_registry_keys(action).iter().all(|key| {
        windows_registry_value_matches(key, None, &name)
            && windows_registry_value_matches(key, Some("Icon"), "squallz-gui.exe")
            && windows_registry_value_matches(key, Some("MultiSelectModel"), "Player")
            && windows_registry_value_matches(&format!("{key}\\command"), None, &command)
    })
}

#[cfg(all(test, target_os = "macos"))]
fn windows_explorer_registry_entries_match(
    script_dir: &Path,
    action: &WindowsExplorerAction,
    _loc: &Localizer,
) -> bool {
    let manifest = windows_registry_manifest_path(script_dir);
    let Ok(contents) = fs::read_to_string(manifest) else {
        return false;
    };
    windows_registry_keys(action)
        .iter()
        .all(|key| contents.contains(&format!("[{key}]")))
}

#[cfg(target_os = "windows")]
fn windows_registry_key_exists(key: &str) -> bool {
    std::process::Command::new("reg.exe")
        .args(["query", key])
        .status()
        .is_ok_and(|status| status.success())
}

#[cfg(target_os = "windows")]
fn windows_registry_value_matches(key: &str, name: Option<&str>, expected: &str) -> bool {
    let mut command = std::process::Command::new("reg.exe");
    command.args(["query", key]);
    match name {
        Some(name) => {
            command.args(["/v", name]);
        }
        None => {
            command.arg("/ve");
        }
    }
    command
        .args(["/f", expected, "/d", "/e"])
        .status()
        .is_ok_and(|status| status.success())
}

#[cfg(target_os = "windows")]
fn windows_reg_add_default(key: &str, value: &str) -> io::Result<()> {
    windows_reg_command(["add", key, "/ve", "/d", value, "/f"])
}

#[cfg(target_os = "windows")]
fn windows_reg_add_value(key: &str, name: &str, value: &str) -> io::Result<()> {
    windows_reg_command(["add", key, "/v", name, "/d", value, "/f"])
}

#[cfg(target_os = "windows")]
fn windows_reg_delete_key(key: &str) -> io::Result<()> {
    windows_reg_command(["delete", key, "/f"])
}

#[cfg(target_os = "windows")]
fn windows_reg_command<const N: usize>(args: [&str; N]) -> io::Result<()> {
    let status = std::process::Command::new("reg.exe").args(args).status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "reg.exe failed with status {status}"
        )))
    }
}

#[cfg(target_os = "macos")]
fn workflow_path_for_action(services_dir: &Path, action: &FinderAction) -> std::path::PathBuf {
    services_dir.join(format!("Squallz-{}.workflow", action.id))
}

#[cfg(target_os = "macos")]
fn remove_stale_workflows(
    services_dir: &Path,
    action: &FinderAction,
    selected_workflow: &Path,
) -> io::Result<()> {
    for workflow in action_workflow_dirs(services_dir, action)? {
        if workflow == selected_workflow {
            continue;
        }
        let _ = remove_owned_directory(&workflow)?;
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn action_workflow_dirs(services_dir: &Path, action: &FinderAction) -> io::Result<Vec<PathBuf>> {
    let entries = match fs::read_dir(services_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let bundle_id = action_bundle_id(action);
    let mut workflows = Vec::new();
    for entry in entries {
        let path = entry?.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("workflow") {
            continue;
        }
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            continue;
        }
        let document = path.join("Contents").join("document.wflow");
        if !document.is_file() {
            continue;
        }
        if workflow_has_bundle_id(&path, &bundle_id) {
            workflows.push(path);
        }
    }
    workflows.sort();
    Ok(workflows)
}

#[cfg(target_os = "macos")]
fn workflow_has_bundle_id(workflow: &Path, bundle_id: &str) -> bool {
    let info = workflow.join("Contents").join("Info.plist");
    match fs::read_to_string(info) {
        Ok(contents) => contents.contains(&format!("<string>{}</string>", xml_escape(bundle_id))),
        Err(e) => {
            log::debug!(
                "integration status: cannot read workflow {}: {e}",
                workflow.display()
            );
            false
        }
    }
}

#[cfg(target_os = "macos")]
fn action_dto_with_name(
    action: &FinderAction,
    name: &str,
    services_dir: &Path,
    script_dir: &Path,
) -> IntegrationActionDto {
    IntegrationActionDto {
        id: action.id.to_owned(),
        name: name.to_owned(),
        kind: "macos_finder_quick_action".to_owned(),
        path: path_to_string(&workflow_path_for_action(services_dir, action)),
        script_path: path_to_string(&script_dir.join(action.script_name)),
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn make_executable(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = fs::metadata(path)?.permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms)
}

#[cfg(target_os = "macos")]
fn action_bundle_id(action: &FinderAction) -> String {
    format!("dev.squallz.desktop.quick-action.{}", action.id)
}

#[cfg(target_os = "macos")]
fn action_name(action: &FinderAction, loc: &Localizer) -> String {
    loc.t(action.name_key)
}

#[cfg(target_os = "macos")]
fn info_plist(action: &FinderAction, name: &str) -> String {
    let bundle_id = action_bundle_id(action);
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleIdentifier</key>
  <string>{}</string>
  <key>CFBundleName</key>
  <string>{}</string>
  <key>CFBundleVersion</key>
  <string>1</string>
  <key>NSServices</key>
  <array>
    <dict>
      <key>NSMenuItem</key>
      <dict>
        <key>default</key>
        <string>{}</string>
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
"#,
        xml_escape(&bundle_id),
        xml_escape(name),
        xml_escape(name),
    )
}

#[cfg(target_os = "macos")]
fn document_workflow(name: &str, script_path: &Path) -> String {
    let command = format!("/bin/zsh {} \"$@\"", shell_quote(script_path));
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
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
          <string>{}</string>
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
  <string>{}</string>
</dict>
</plist>
"#,
        xml_escape(&command),
        xml_escape(name),
    )
}

#[cfg(target_os = "macos")]
fn shell_quote(path: &Path) -> String {
    let value = path.to_string_lossy();
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(target_os = "macos")]
fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
fn path_to_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[cfg(test)]
#[cfg(target_os = "macos")]
mod tests {
    use super::{
        directory_is_empty, finder_script_preamble, install_macos_finder_actions_at_with_language,
        install_macos_finder_actions_at_with_localizer,
        macos_finder_actions_status_at_with_language,
        macos_finder_actions_status_at_with_localizer, macos_integration_dirs,
        remove_macos_finder_actions_at_with_language,
        remove_macos_finder_actions_at_with_localizer, summarize_default_handlers,
        MACOS_DECLARED_FILE_EXTENSIONS, SQUALLZ_BUNDLE_IDENTIFIER,
    };
    use crate::dto::{
        IntegrationActionHealthStateDto, IntegrationDefaultHandlerDto,
        IntegrationDefaultHandlerStateDto, IntegrationDefaultHandlersStateDto,
        IntegrationHealthStateDto,
    };
    use squallz_i18n::Localizer;
    use std::fs;
    use std::os::unix::fs::{symlink, PermissionsExt};
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn default_handler_summary_keeps_unknown_and_mixed_states_distinct() {
        let all_squallz = summarize_default_handlers(vec![
            default_handler("zip", IntegrationDefaultHandlerStateDto::Squallz),
            default_handler("7z", IntegrationDefaultHandlerStateDto::Squallz),
        ]);
        assert_eq!(
            all_squallz.state,
            IntegrationDefaultHandlersStateDto::Squallz
        );
        assert_eq!(all_squallz.checked, 2);
        assert_eq!(all_squallz.squallz, 2);

        let mixed = summarize_default_handlers(vec![
            default_handler("zip", IntegrationDefaultHandlerStateDto::Other),
            default_handler("sqz", IntegrationDefaultHandlerStateDto::Squallz),
        ]);
        assert_eq!(mixed.state, IntegrationDefaultHandlersStateDto::Mixed);
        assert_eq!(mixed.squallz, 1);

        let all_other = summarize_default_handlers(vec![default_handler(
            "zip",
            IntegrationDefaultHandlerStateDto::Other,
        )]);
        assert_eq!(all_other.state, IntegrationDefaultHandlersStateDto::Other);

        let partial = summarize_default_handlers(vec![
            default_handler("zip", IntegrationDefaultHandlerStateDto::Other),
            default_handler("sqz", IntegrationDefaultHandlerStateDto::Unknown),
        ]);
        assert_eq!(partial.state, IntegrationDefaultHandlersStateDto::Unknown);
        assert_eq!(partial.total, 2);
        assert_eq!(partial.checked, 1);

        let unavailable = summarize_default_handlers(Vec::new());
        assert_eq!(
            unavailable.state,
            IntegrationDefaultHandlersStateDto::Unavailable
        );
    }

    #[test]
    fn default_handler_probe_extensions_match_the_bundle_declaration() {
        let config: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        let identifier = config
            .get("identifier")
            .and_then(serde_json::Value::as_str)
            .unwrap();
        assert_eq!(identifier, SQUALLZ_BUNDLE_IDENTIFIER);

        let declared = config
            .pointer("/bundle/fileAssociations")
            .and_then(serde_json::Value::as_array)
            .unwrap()
            .iter()
            .flat_map(|association| {
                association
                    .get("ext")
                    .and_then(serde_json::Value::as_array)
                    .into_iter()
                    .flatten()
            })
            .filter_map(serde_json::Value::as_str)
            .collect::<Vec<_>>();
        assert_eq!(declared, MACOS_DECLARED_FILE_EXTENSIONS);
    }

    #[test]
    fn bundle_declares_complete_cross_platform_icon_set() {
        let config: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        let icons = config
            .pointer("/bundle/icon")
            .and_then(serde_json::Value::as_array)
            .unwrap()
            .iter()
            .filter_map(serde_json::Value::as_str)
            .collect::<Vec<_>>();

        assert_eq!(
            icons,
            [
                "icons/32x32.png",
                "icons/128x128.png",
                "icons/128x128@2x.png",
                "icons/icon.icns",
                "icons/icon.ico",
            ]
        );
    }

    fn default_handler(
        extension: &str,
        state: IntegrationDefaultHandlerStateDto,
    ) -> IntegrationDefaultHandlerDto {
        IntegrationDefaultHandlerDto {
            extension: extension.to_owned(),
            state,
            application_name: None,
        }
    }

    #[test]
    fn finder_script_preamble_localizes_cli_error_alert() {
        let loc = Localizer::with_user_dir(Some("zh-CN"), None);
        let preamble = finder_script_preamble(None, &loc);

        assert!(preamble.contains("找不到 Squallz 命令行工具"));
        assert!(preamble.contains("访达快捷操作"));
        assert!(preamble.contains("CLI_NOT_FOUND_ALERT='display alert"));
        assert!(preamble.contains("SQUALLZ_DISABLE_GUI_HANDOFF"));
    }

    #[test]
    fn directory_is_empty_distinguishes_empty_missing_and_nonempty_dirs() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("squallz-integration-empty-dir-{nonce}"));
        let empty = root.join("empty");
        let nonempty = root.join("nonempty");
        fs::create_dir_all(&empty).unwrap();
        fs::create_dir_all(&nonempty).unwrap();
        fs::write(nonempty.join("script.sh"), b"echo ok").unwrap();

        assert!(directory_is_empty(&empty));
        assert!(!directory_is_empty(&nonempty));
        assert!(!directory_is_empty(&root.join("missing")));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn installs_macos_finder_workflows_and_scripts() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let home = std::env::temp_dir().join(format!("squallz-integration-test-{nonce}"));
        let result = install_macos_finder_actions_at_with_language(&home, None).unwrap();

        assert_eq!(result.platform, "macos");
        assert_eq!(result.installed.len(), 5);
        assert!(result.installed.iter().any(|item| item.id == "checksum"));
        assert!(result
            .installed
            .iter()
            .any(|item| item.id == "extract-here"));
        assert!(result
            .installed
            .iter()
            .any(|item| item.id == "compress-to-7z"));

        for action in &result.installed {
            let workflow = std::path::Path::new(&action.path);
            let script = std::path::Path::new(&action.script_path);
            let info = workflow.join("Contents").join("Info.plist");
            let document = workflow.join("Contents").join("document.wflow");
            assert!(info.is_file());
            let wflow = fs::read_to_string(&document).unwrap();
            assert!(wflow.contains("com.apple.RunShellScript"));
            assert!(wflow.contains(&action.name));
            assert!(script.is_file());
            let body = fs::read_to_string(script).unwrap();
            assert!(body.contains("resolve_sqz"));
            assert!(body.contains("Contents/MacOS/sqz"));
            assert!(body.contains("Contents/Resources/bin/sqz"));
            assert!(body.contains("run_gui_task"));
            assert!(body.contains("--squallz-action"));
            assert!(body.contains("$SQUALLZ_TASK_WINDOW_ACTION_ARG"));
            assert!(body.contains("run_sqz"));
            assert!(Command::new("/bin/zsh")
                .arg("-n")
                .arg(script)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .unwrap()
                .success());
            assert!(Command::new("/usr/bin/plutil")
                .arg("-lint")
                .arg(&info)
                .arg(&document)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .unwrap()
                .success());
        }

        let status = macos_finder_actions_status_at_with_language(&home, None).unwrap();
        assert_eq!(status.actions.len(), 5);
        assert_eq!(status.health, IntegrationHealthStateDto::Healthy);
        assert!(status.can_repair);
        assert!(status.can_remove);
        assert!(status
            .actions
            .iter()
            .all(|action| action.state == IntegrationActionHealthStateDto::Healthy));

        let checksum_script = result
            .installed
            .iter()
            .find(|action| action.id == "checksum")
            .map(|action| action.script_path.clone())
            .unwrap();
        fs::write(&checksum_script, b"#!/bin/zsh\nexit 1\n").unwrap();
        let damaged = macos_finder_actions_status_at_with_language(&home, None).unwrap();
        assert_eq!(damaged.health, IntegrationHealthStateDto::NeedsRepair);
        let checksum_health = damaged
            .actions
            .iter()
            .find(|action| action.id == "checksum")
            .unwrap();
        assert_eq!(
            checksum_health.state,
            IntegrationActionHealthStateDto::Damaged
        );
        assert_eq!(checksum_health.issue.as_deref(), Some("script_outdated"));
        install_macos_finder_actions_at_with_language(&home, None).unwrap();
        let repaired = macos_finder_actions_status_at_with_language(&home, None).unwrap();
        assert_eq!(repaired.health, IntegrationHealthStateDto::Healthy);

        let fake_sqz = home.join("fake-sqz");
        let log = home.join("sqz-args.log");
        write_fake_sqz(&fake_sqz);

        let sample = home.join("samples");
        fs::create_dir_all(sample.join("folder input")).unwrap();
        fs::write(sample.join("one.zip"), b"archive").unwrap();
        fs::write(sample.join("two.7z"), b"archive").unwrap();
        fs::write(sample.join("plain file.txt"), b"plain").unwrap();
        fs::write(sample.join("folder input/nested.txt"), b"nested").unwrap();

        let script_for = |id: &str| {
            result
                .installed
                .iter()
                .find(|item| item.id == id)
                .map(|item| item.script_path.clone())
                .unwrap_or_else(|| panic!("missing script for {id}"))
        };
        run_action_script(
            &script_for("extract-here"),
            &fake_sqz,
            &log,
            &[sample.join("one.zip"), sample.join("two.7z")],
        );
        run_action_script(
            &script_for("extract-to-folder"),
            &fake_sqz,
            &log,
            &[sample.join("one.zip"), sample.join("two.7z")],
        );
        run_action_script(
            &script_for("compress-to-7z"),
            &fake_sqz,
            &log,
            &[sample.join("plain file.txt"), sample.join("folder input")],
        );
        run_action_script(
            &script_for("checksum"),
            &fake_sqz,
            &log,
            &[sample.join("plain file.txt"), sample.join("one.zip")],
        );
        run_action_script(
            &script_for("test-archive"),
            &fake_sqz,
            &log,
            &[
                sample.join("one.zip"),
                sample.join("folder input"),
                sample.join("two.7z"),
            ],
        );
        let fake_app = home.join("Fake Squallz.app");
        let bundled_sqz = fake_app
            .join("Contents")
            .join("Resources")
            .join("bin")
            .join("sqz");
        write_fake_sqz(&bundled_sqz);
        run_action_script_from_bundle(
            &script_for("test-archive"),
            &fake_app,
            &log,
            &[sample.join("one.zip")],
        );

        assert!(sample.join("one").is_dir());
        assert!(sample.join("two").is_dir());

        let one = sample.join("one.zip").to_string_lossy().into_owned();
        let two = sample.join("two.7z").to_string_lossy().into_owned();
        let plain = sample.join("plain file.txt").to_string_lossy().into_owned();
        let folder = sample.join("folder input").to_string_lossy().into_owned();
        let parent = sample.to_string_lossy().into_owned();
        let log = fs::read_to_string(&log).unwrap();
        assert!(
            log.contains(&format!(
                "<extract><{one}><-d><{parent}><--smart><--file-manager-preset>"
            )),
            "log: {log}"
        );
        assert!(
            log.contains(&format!(
                "<extract><{two}><-d><{parent}><--smart><--file-manager-preset>"
            )),
            "log: {log}"
        );
        assert!(
            log.contains(&format!(
                "<extract><{one}><-d><{parent}/one><--file-manager-preset>"
            )),
            "log: {log}"
        );
        assert!(
            log.contains(&format!(
                "<extract><{two}><-d><{parent}/two><--file-manager-preset>"
            )),
            "log: {log}"
        );
        assert!(
            log.contains(&format!(
                "<compress><{plain}><{folder}><-o><{parent}/Archive.7z><--file-manager-preset>"
            )),
            "log: {log}"
        );
        assert!(
            log.contains(&format!("<checksum><{plain}><{one}>")),
            "log: {log}"
        );
        assert!(log.contains(&format!("<test><{one}>")), "log: {log}");
        assert!(log.contains(&format!("<test><{two}>")), "log: {log}");
        assert!(
            !log.contains(&format!("<test><{folder}>")),
            "directory inputs should be skipped by archive-test action; log: {log}"
        );

        let removed = remove_macos_finder_actions_at_with_language(&home, None).unwrap();
        assert_eq!(removed.removed.len(), 5);
        assert!(removed.missing.is_empty());

        let status = macos_finder_actions_status_at_with_language(&home, None).unwrap();
        assert!(status
            .actions
            .iter()
            .all(|action| action.state == IntegrationActionHealthStateDto::Missing));
        assert_eq!(status.health, IntegrationHealthStateDto::Missing);
        assert!(!status.can_remove);

        let _ = fs::remove_dir_all(home);
    }

    #[test]
    fn custom_language_pack_names_finder_workflows_without_code_changes() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let home =
            std::env::temp_dir().join(format!("squallz-integration-custom-locale-home-{nonce}"));
        let locale_dir =
            std::env::temp_dir().join(format!("squallz-integration-custom-locale-pack-{nonce}"));
        fs::create_dir_all(&locale_dir).unwrap();
        fs::write(
            locale_dir.join("xx-XX.json"),
            r#"{
  "meta.name": "XX",
  "gui.integration.finder.action.checksum": "Squallz XX Checksum",
  "gui.integration.finder.action.extract_here": "Squallz XX Extract",
  "gui.integration.finder.action.extract_to_folder": "Squallz XX Folder",
  "gui.integration.finder.action.compress_to_7z": "Squallz XX 7Z",
  "gui.integration.finder.action.test_archive": "Squallz XX Test",
  "gui.integration.finder.cli_not_found.title": "XX CLI missing",
  "gui.integration.finder.cli_not_found.message": "XX install CLI"
}"#,
        )
        .unwrap();
        let loc = Localizer::with_user_dir(Some("xx-XX"), Some(&locale_dir));

        let result = install_macos_finder_actions_at_with_localizer(&home, &loc).unwrap();
        assert_eq!(result.installed.len(), 5);
        let extract = result
            .installed
            .iter()
            .find(|item| item.id == "extract-here")
            .unwrap();
        assert_eq!(extract.name, "Squallz XX Extract");
        assert!(Path::new(&extract.path).is_dir());

        let script_text = fs::read_to_string(&extract.script_path).unwrap();
        assert!(script_text.contains("XX CLI missing"));
        assert!(script_text.contains("XX install CLI"));

        let status = macos_finder_actions_status_at_with_localizer(&home, &loc).unwrap();
        assert_eq!(status.actions.len(), 5);
        assert!(status
            .actions
            .iter()
            .any(|item| item.name == "Squallz XX Extract"));

        let removed = remove_macos_finder_actions_at_with_localizer(&home, &loc).unwrap();
        assert_eq!(removed.removed.len(), 5);
        assert!(removed.missing.is_empty());

        let _ = fs::remove_dir_all(home);
        let _ = fs::remove_dir_all(locale_dir);
    }

    #[test]
    fn finder_workflow_paths_do_not_depend_on_localized_names() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let home = std::env::temp_dir().join(format!("squallz-integration-safe-name-{nonce}"));
        let locale_dir =
            std::env::temp_dir().join(format!("squallz-integration-safe-locale-{nonce}"));
        fs::create_dir_all(&locale_dir).unwrap();
        fs::write(
            locale_dir.join("xx-XX.json"),
            r#"{
  "meta.name": "XX",
  "gui.integration.finder.action.checksum": "../Outside",
  "gui.integration.finder.action.extract_here": "/tmp/Outside",
  "gui.integration.finder.action.extract_to_folder": "Nested/Outside",
  "gui.integration.finder.action.compress_to_7z": ".",
  "gui.integration.finder.action.test_archive": ".."
}"#,
        )
        .unwrap();
        let loc = Localizer::with_user_dir(Some("xx-XX"), Some(&locale_dir));

        let result = install_macos_finder_actions_at_with_localizer(&home, &loc).unwrap();
        let (services_dir, _) = macos_integration_dirs(&home);
        assert_eq!(result.installed.len(), 5);
        for action in result.installed {
            let workflow = Path::new(&action.path);
            assert_eq!(workflow.parent(), Some(services_dir.as_path()));
            assert!(workflow
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("Squallz-") && name.ends_with(".workflow")));
        }
        assert!(!home.join("Library/Outside.workflow").exists());

        let _ = fs::remove_dir_all(home);
        let _ = fs::remove_dir_all(locale_dir);
    }

    #[test]
    fn finder_install_refuses_managed_output_symlinks() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let home = std::env::temp_dir().join(format!("squallz-integration-symlink-{nonce}"));
        let (services_dir, script_dir) = macos_integration_dirs(&home);
        fs::create_dir_all(&services_dir).unwrap();
        fs::create_dir_all(&script_dir).unwrap();
        let victim = home.join("victim.txt");
        fs::write(&victim, b"keep me").unwrap();
        symlink(&victim, script_dir.join("squallz-checksum.sh")).unwrap();

        let error = install_macos_finder_actions_at_with_language(&home, None).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert_eq!(fs::read(&victim).unwrap(), b"keep me");

        let _ = fs::remove_dir_all(home);
    }

    #[test]
    fn finder_install_refuses_symlinked_managed_directories() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let home = std::env::temp_dir().join(format!("squallz-integration-parent-link-{nonce}"));
        let outside = home.join("outside-services");
        fs::create_dir_all(home.join("Library")).unwrap();
        fs::create_dir_all(&outside).unwrap();
        symlink(&outside, home.join("Library/Services")).unwrap();

        let error = install_macos_finder_actions_at_with_language(&home, None).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert_eq!(fs::read_dir(&outside).unwrap().count(), 0);

        let _ = fs::remove_dir_all(home);
    }

    #[test]
    fn damaged_fixed_workflow_stays_repairable_and_removable() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let home =
            std::env::temp_dir().join(format!("squallz-integration-damaged-workflow-{nonce}"));
        let installed = install_macos_finder_actions_at_with_language(&home, None).unwrap();
        let checksum = installed
            .installed
            .iter()
            .find(|action| action.id == "checksum")
            .unwrap();
        fs::remove_file(Path::new(&checksum.path).join("Contents/Info.plist")).unwrap();

        let status = macos_finder_actions_status_at_with_language(&home, None).unwrap();
        assert_eq!(status.health, IntegrationHealthStateDto::NeedsRepair);
        assert!(status.can_remove);
        let checksum_health = status
            .actions
            .iter()
            .find(|action| action.id == "checksum")
            .unwrap();
        assert_eq!(
            checksum_health.state,
            IntegrationActionHealthStateDto::Damaged
        );
        assert_eq!(checksum_health.issue.as_deref(), Some("launcher_outdated"));

        let removed = remove_macos_finder_actions_at_with_language(&home, None).unwrap();
        assert_eq!(removed.removed.len(), 5);
        let after_remove = macos_finder_actions_status_at_with_language(&home, None).unwrap();
        assert_eq!(after_remove.health, IntegrationHealthStateDto::Missing);

        let _ = fs::remove_dir_all(home);
    }

    #[test]
    fn finder_install_does_not_recursively_delete_a_script_directory() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let home = std::env::temp_dir().join(format!("squallz-integration-script-dir-{nonce}"));
        let (_, script_dir) = macos_integration_dirs(&home);
        let unexpected = script_dir.join("squallz-checksum.sh");
        fs::create_dir_all(&unexpected).unwrap();
        fs::write(unexpected.join("keep.txt"), b"keep me").unwrap();

        let error = install_macos_finder_actions_at_with_language(&home, None).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert_eq!(fs::read(unexpected.join("keep.txt")).unwrap(), b"keep me");

        let _ = fs::remove_dir_all(home);
    }

    #[test]
    fn finder_language_change_updates_the_managed_workflows() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let home = std::env::temp_dir().join(format!("squallz-integration-locale-test-{nonce}"));

        let english = install_macos_finder_actions_at_with_language(&home, Some("en-US")).unwrap();
        let english_extract = english
            .installed
            .iter()
            .find(|item| item.id == "extract-here")
            .unwrap();
        assert_eq!(english_extract.name, "Squallz Extract Here");
        assert!(Path::new(&english_extract.path).is_dir());

        let localized =
            install_macos_finder_actions_at_with_language(&home, Some("zh-CN")).unwrap();
        assert_eq!(localized.installed.len(), 5);
        let localized_extract = localized
            .installed
            .iter()
            .find(|item| item.id == "extract-here")
            .unwrap();
        assert_eq!(localized_extract.name, "Squallz 就地解压");
        assert!(Path::new(&localized_extract.path).is_dir());
        assert_eq!(localized_extract.path, english_extract.path);

        let info = Path::new(&localized_extract.path)
            .join("Contents")
            .join("Info.plist");
        let document = Path::new(&localized_extract.path)
            .join("Contents")
            .join("document.wflow");
        let info_text = fs::read_to_string(&info).unwrap();
        let document_text = fs::read_to_string(&document).unwrap();
        assert!(info_text.contains("Squallz 就地解压"));
        assert!(document_text.contains("Squallz 就地解压"));
        assert!(Command::new("/usr/bin/plutil")
            .arg("-lint")
            .arg(&info)
            .arg(&document)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap()
            .success());

        let script_text = fs::read_to_string(&localized_extract.script_path).unwrap();
        assert!(script_text.contains("找不到 Squallz 命令行工具"));
        assert!(script_text.contains("访达快捷操作"));
        assert!(Command::new("/bin/zsh")
            .arg("-n")
            .arg(&localized_extract.script_path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap()
            .success());

        let localized_status =
            macos_finder_actions_status_at_with_language(&home, Some("zh-CN")).unwrap();
        assert_eq!(localized_status.actions.len(), 5);
        assert!(localized_status
            .actions
            .iter()
            .any(|item| item.name == "Squallz 就地解压"));

        let default_language_status =
            macos_finder_actions_status_at_with_language(&home, None).unwrap();
        assert_eq!(default_language_status.actions.len(), 5);

        let removed = remove_macos_finder_actions_at_with_language(&home, Some("zh-CN")).unwrap();
        assert_eq!(removed.removed.len(), 5);
        assert!(removed.missing.is_empty());
        assert!(!Path::new(&localized_extract.path).exists());

        let status_after_remove =
            macos_finder_actions_status_at_with_language(&home, Some("zh-CN")).unwrap();
        assert!(status_after_remove
            .actions
            .iter()
            .all(|action| action.state == IntegrationActionHealthStateDto::Missing));

        let _ = fs::remove_dir_all(home);
    }

    fn run_action_script(script: &str, fake_sqz: &Path, log: &Path, inputs: &[std::path::PathBuf]) {
        let mut command = Command::new("/bin/zsh");
        command
            .arg(script)
            .env("SQUALLZ_CLI", fake_sqz)
            .env("SQUALLZ_QA_LOG", log)
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        for input in inputs {
            command.arg(input);
        }
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "script {script} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn run_action_script_from_bundle(
        script: &str,
        app_bundle: &Path,
        log: &Path,
        inputs: &[PathBuf],
    ) {
        let mut command = Command::new("/bin/zsh");
        command
            .arg(script)
            .env_remove("SQUALLZ_CLI")
            .env("SQUALLZ_APP_BUNDLE", app_bundle)
            .env("SQUALLZ_QA_LOG", log)
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        for input in inputs {
            command.arg(input);
        }
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "script {script} failed from bundled helper: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn write_fake_sqz(path: &Path) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(
            path,
            r#"#!/bin/zsh
for arg in "$@"; do
  printf '<%s>' "$arg" >> "$SQUALLZ_QA_LOG"
done
printf '\n' >> "$SQUALLZ_QA_LOG"
"#,
        )
        .unwrap();
        let mut perms = fs::metadata(path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(path, perms).unwrap();
    }
}

#[cfg(test)]
#[cfg(any(target_os = "macos", target_os = "linux"))]
mod linux_file_manager_tests {
    use super::{
        desktop_exec_argument, install_linux_file_manager_actions_at_with_language,
        install_linux_file_manager_actions_at_with_localizer_and_appimage,
        linux_data_home_from_env, linux_file_manager_actions_status_at_with_language,
        linux_file_manager_actions_status_at_with_localizer_and_appimage, linux_integration_dirs,
        remove_linux_file_manager_actions_at_with_language, safe_visible_file_name,
        validated_linux_appimage_launch, LinuxAppImageLaunchMode,
    };
    use crate::dto::{IntegrationActionHealthStateDto, IntegrationHealthStateDto};
    use squallz_i18n::Localizer;
    use std::fs;
    use std::os::unix::fs::{symlink, PermissionsExt};
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};
    use std::thread;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    #[test]
    fn installs_linux_file_manager_actions_that_reuse_task_window_handoff() {
        let home = temp_home("squallz-linux-integration-test");
        let result =
            install_linux_file_manager_actions_at_with_language(&home, Some("en-US")).unwrap();

        assert_eq!(result.platform, "linux");
        assert_eq!(result.installed.len(), 5);
        assert!(result
            .unsupported
            .iter()
            .any(|item| item.contains("Windows Explorer")));

        let (_, _, nautilus_dir) = linux_integration_dirs(&home);
        for action in &result.installed {
            let service = Path::new(&action.path);
            let script = Path::new(&action.script_path);
            assert!(service.is_file());
            assert!(script.is_file());
            assert_ne!(
                fs::metadata(service).unwrap().permissions().mode() & 0o111,
                0
            );

            let service_text = fs::read_to_string(service).unwrap();
            assert!(service_text.contains("ServiceTypes=KonqPopupMenu/Plugin"));
            assert!(service_text.contains("Actions=squallz-"));
            assert!(service_text.contains("Exec=\""));
            assert!(service_text.contains(" %F"));
            assert!(service_text.contains(&action.name));

            let script_text = fs::read_to_string(script).unwrap();
            assert!(script_text.contains("run_gui_task"));
            assert!(script_text.contains("--squallz-action"));
            assert!(script_text.contains("SQUALLZ_TASK_WINDOW_ACTION_ARG='--squallz-action'"));
            assert!(script_text.contains("$SQUALLZ_TASK_WINDOW_ACTION_ARG"));
            assert!(script_text.contains("run_sqz"));
            assert!(Command::new("/bin/bash")
                .arg("-n")
                .arg(script)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .unwrap()
                .success());
        }
        assert_eq!(fs::read_dir(&nautilus_dir).unwrap().count(), 5);

        let sample = home.join("samples");
        fs::create_dir_all(sample.join("folder input")).unwrap();
        fs::write(sample.join("one.zip"), b"archive").unwrap();
        fs::write(sample.join("two.7z"), b"archive").unwrap();
        fs::write(sample.join("plain file.txt"), b"plain").unwrap();
        fs::write(sample.join("folder input/nested.txt"), b"nested").unwrap();
        let sample_cwd = fs::canonicalize(&sample).unwrap();

        let fake_gui = home.join("fake-squallz-gui");
        let gui_log = home.join("gui-args.log");
        write_fake_sh_tool(&fake_gui);
        run_linux_action_script_with_gui(
            &script_for(&result, "checksum"),
            &fake_gui,
            &gui_log,
            &[sample.join("plain file.txt"), sample.join("one.zip")],
        );
        let gui_log = wait_for_log_contains(&gui_log, "<--squallz-action><checksum>");
        assert!(
            gui_log.contains("<--squallz-action><checksum>"),
            "log: {gui_log}"
        );

        let relative_gui_log = home.join("relative-gui-args.log");
        run_linux_action_script_with_gui_from(
            &script_for(&result, "test-archive"),
            &fake_gui,
            &relative_gui_log,
            &sample_cwd,
            &["one.zip", "folder input", "two.7z"],
        );
        let _ = wait_for_log_contains(
            &relative_gui_log,
            &format!("<{}>", sample_cwd.join("one.zip").display()),
        );
        let relative_gui_log = wait_for_log_contains(
            &relative_gui_log,
            &format!("<{}>", sample_cwd.join("two.7z").display()),
        );
        assert!(
            relative_gui_log.contains(&format!(
                "<--squallz-action><test-archive><{}>",
                sample_cwd.join("one.zip").display()
            )),
            "log: {relative_gui_log}"
        );
        assert!(
            relative_gui_log.contains(&format!(
                "<--squallz-action><test-archive><{}>",
                sample_cwd.join("two.7z").display()
            )),
            "log: {relative_gui_log}"
        );
        assert!(
            !relative_gui_log.contains(&format!("<{}>", sample_cwd.join("folder input").display())),
            "directories must not be handed to archive-test jobs: {relative_gui_log}"
        );

        let relative_extract_log = home.join("relative-extract-gui-args.log");
        run_linux_action_script_with_gui_from(
            &script_for(&result, "extract-here"),
            &fake_gui,
            &relative_extract_log,
            &sample_cwd,
            &["one.zip", "folder input"],
        );
        let relative_extract_log = wait_for_log_contains(
            &relative_extract_log,
            &format!("<{}>", sample_cwd.join("one.zip").display()),
        );
        assert!(
            !relative_extract_log
                .contains(&format!("<{}>", sample_cwd.join("folder input").display())),
            "directories must not be handed to archive-extract jobs: {relative_extract_log}"
        );

        let relative_compress_log = home.join("relative-compress-gui-args.log");
        fs::write(sample.join("plain file.7z"), b"existing output").unwrap();
        run_linux_action_script_with_gui_from(
            &script_for(&result, "compress-to-7z"),
            &fake_gui,
            &relative_compress_log,
            &sample_cwd,
            &["plain file.txt"],
        );
        let relative_compress_log = wait_for_log_contains(
            &relative_compress_log,
            &format!("<{}>", sample_cwd.join("plain file.txt").display()),
        );
        assert!(
            relative_compress_log.contains(&format!(
                "<--squallz-action><compress-to-7z><--squallz-output><{}><{}>",
                sample_cwd.join("plain file 2.7z").display(),
                sample_cwd.join("plain file.txt").display()
            )),
            "log: {relative_compress_log}"
        );

        let empty_gui_log = home.join("empty-gui-args.log");
        run_linux_action_script_with_gui_from(
            &script_for(&result, "test-archive"),
            &fake_gui,
            &empty_gui_log,
            &sample_cwd,
            &[],
        );
        assert!(
            !empty_gui_log.exists(),
            "an empty file-manager selection must not launch a task"
        );

        let fake_sqz = home.join("fake-sqz");
        let cli_log = home.join("sqz-args.log");
        write_fake_sh_tool(&fake_sqz);
        run_linux_action_script(
            &script_for(&result, "extract-here"),
            &fake_sqz,
            &cli_log,
            &[sample.join("one.zip"), sample.join("two.7z")],
        );
        run_linux_action_script(
            &script_for(&result, "extract-to-folder"),
            &fake_sqz,
            &cli_log,
            &[sample.join("one.zip"), sample.join("two.7z")],
        );
        run_linux_action_script(
            &script_for(&result, "compress-to-7z"),
            &fake_sqz,
            &cli_log,
            &[sample.join("plain file.txt"), sample.join("folder input")],
        );
        run_linux_action_script(
            &script_for(&result, "test-archive"),
            &fake_sqz,
            &cli_log,
            &[
                sample.join("one.zip"),
                sample.join("folder input"),
                sample.join("two.7z"),
            ],
        );

        let one = sample.join("one.zip").to_string_lossy().into_owned();
        let two = sample.join("two.7z").to_string_lossy().into_owned();
        let plain = sample.join("plain file.txt").to_string_lossy().into_owned();
        let folder = sample.join("folder input").to_string_lossy().into_owned();
        let parent = sample.to_string_lossy().into_owned();
        let cli_log = fs::read_to_string(&cli_log).unwrap();
        assert!(
            cli_log.contains(&format!(
                "<extract><{one}><-d><{parent}><--smart><--file-manager-preset>"
            )),
            "log: {cli_log}"
        );
        assert!(
            cli_log.contains(&format!(
                "<extract><{two}><-d><{parent}><--smart><--file-manager-preset>"
            )),
            "log: {cli_log}"
        );
        assert!(
            cli_log.contains(&format!(
                "<extract><{one}><-d><{parent}/one><--file-manager-preset>"
            )),
            "log: {cli_log}"
        );
        assert!(
            cli_log.contains(&format!(
                "<compress><{plain}><{folder}><-o><{parent}/Archive.7z><--file-manager-preset>"
            )),
            "log: {cli_log}"
        );
        assert!(
            cli_log.contains(&format!("<test><{one}>")),
            "log: {cli_log}"
        );
        assert!(
            cli_log.contains(&format!("<test><{two}>")),
            "log: {cli_log}"
        );
        assert!(
            !cli_log.contains(&format!("<test><{folder}>")),
            "directory inputs should be skipped by archive-test action; log: {cli_log}"
        );

        let status =
            linux_file_manager_actions_status_at_with_language(&home, Some("en-US")).unwrap();
        assert_eq!(status.actions.len(), 5);
        assert_eq!(status.health, IntegrationHealthStateDto::Healthy);
        assert!(status
            .actions
            .iter()
            .all(|action| action.state == IntegrationActionHealthStateDto::Healthy));

        let removed =
            remove_linux_file_manager_actions_at_with_language(&home, Some("en-US")).unwrap();
        assert_eq!(removed.removed.len(), 5);
        assert!(removed.missing.is_empty());

        let status_after_remove =
            linux_file_manager_actions_status_at_with_language(&home, Some("en-US")).unwrap();
        assert!(status_after_remove
            .actions
            .iter()
            .all(|action| action.state == IntegrationActionHealthStateDto::Missing));
        assert_eq!(
            status_after_remove.health,
            IntegrationHealthStateDto::Missing
        );

        let _ = fs::remove_dir_all(home);
    }

    #[test]
    fn linux_language_change_replaces_stale_nautilus_script_names() {
        let home = temp_home("squallz-linux-integration-locale-test");

        let english =
            install_linux_file_manager_actions_at_with_language(&home, Some("en-US")).unwrap();
        let english_extract = english
            .installed
            .iter()
            .find(|item| item.id == "extract-here")
            .unwrap();
        let (_, _, nautilus_dir) = linux_integration_dirs(&home);
        let english_nautilus = nautilus_dir.join(&english_extract.name);
        assert!(english_nautilus.is_file());

        let localized =
            install_linux_file_manager_actions_at_with_language(&home, Some("zh-CN")).unwrap();
        let localized_extract = localized
            .installed
            .iter()
            .find(|item| item.id == "extract-here")
            .unwrap();
        assert_eq!(localized_extract.name, "Squallz 就地解压");
        assert!(nautilus_dir.join(&localized_extract.name).is_file());
        assert!(!english_nautilus.exists());

        let status =
            linux_file_manager_actions_status_at_with_language(&home, Some("zh-CN")).unwrap();
        assert_eq!(status.actions.len(), 5);

        let removed =
            remove_linux_file_manager_actions_at_with_language(&home, Some("zh-CN")).unwrap();
        assert_eq!(removed.removed.len(), 5);

        let _ = fs::remove_dir_all(home);
    }

    #[test]
    fn nautilus_action_names_cannot_escape_the_managed_directory() {
        assert_eq!(safe_visible_file_name(".."), "Squallz Action");
        assert_eq!(safe_visible_file_name("."), "Squallz Action");
        assert_eq!(safe_visible_file_name("../Outside"), ".._Outside");
        assert_eq!(safe_visible_file_name("/tmp/Outside"), "_tmp_Outside");
    }

    #[test]
    fn linux_data_home_ignores_relative_xdg_paths() {
        let home = Path::new("/home/squallz");
        assert_eq!(
            linux_data_home_from_env(home, Some(std::ffi::OsStr::new("relative/data"))),
            home.join(".local/share")
        );
        assert_eq!(
            linux_data_home_from_env(home, Some(std::ffi::OsStr::new("/var/lib/squallz"))),
            PathBuf::from("/var/lib/squallz")
        );
    }

    #[test]
    fn corrupted_nautilus_launcher_remains_visible_to_status_and_cleanup() {
        let home = temp_home("squallz-linux-integration-damaged-launcher");
        let installed =
            install_linux_file_manager_actions_at_with_language(&home, Some("en-US")).unwrap();
        let extract = installed
            .installed
            .iter()
            .find(|action| action.id == "extract-here")
            .unwrap();
        let (_, _, nautilus_dir) = linux_integration_dirs(&home);
        let launcher = nautilus_dir.join(&extract.name);
        fs::write(&launcher, b"#!/usr/bin/env bash\nexit 1\n").unwrap();
        let mut permissions = fs::metadata(&launcher).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&launcher, permissions).unwrap();

        let status =
            linux_file_manager_actions_status_at_with_language(&home, Some("en-US")).unwrap();
        assert_eq!(status.health, IntegrationHealthStateDto::NeedsRepair);
        assert!(status.can_remove);
        let extract_health = status
            .actions
            .iter()
            .find(|action| action.id == "extract-here")
            .unwrap();
        assert_eq!(
            extract_health.state,
            IntegrationActionHealthStateDto::Damaged
        );
        assert_eq!(extract_health.issue.as_deref(), Some("launcher_outdated"));

        remove_linux_file_manager_actions_at_with_language(&home, Some("en-US")).unwrap();
        assert!(!launcher.exists());
        let _ = fs::remove_dir_all(home);
    }

    #[test]
    fn installed_actions_keep_a_valid_appimage_path_and_require_repair_after_it_moves() {
        let root = temp_home("squallz-linux-appimage-actions");
        let home = root.join("home");
        let mount = root.join(".mount_Squallz");
        let current_exe = mount.join("usr/bin/squallz-gui");
        let original_appimage = root.join("Squallz user's.AppImage");
        let moved_appimage = root.join("Squallz moved.AppImage");
        write_fake_sh_tool(&current_exe);
        write_fake_sh_tool(&original_appimage);

        let installed_appimage = validated_linux_appimage_launch(
            Some(&original_appimage),
            Some(&mount),
            &current_exe,
            &root,
        )
        .unwrap();
        let loc = Localizer::load(Some("en-US"));
        let installed = install_linux_file_manager_actions_at_with_localizer_and_appimage(
            &home,
            &loc,
            Some(&installed_appimage),
        )
        .unwrap();
        let checksum_script = script_for(&installed, "checksum");
        let script_text = fs::read_to_string(&checksum_script).unwrap();
        let escaped_path = installed_appimage
            .path
            .to_string_lossy()
            .replace('\'', "'\"'\"'");
        assert!(script_text.contains(&format!("SQUALLZ_APPIMAGE='{escaped_path}'")));
        assert!(script_text.contains("SQUALLZ_APPIMAGE_EXTRACT_AND_RUN='0'"));

        let sample = root.join("sample file.txt");
        fs::write(&sample, b"sample").unwrap();
        let gui_log = root.join("appimage-gui.log");
        let output = Command::new("/bin/bash")
            .arg(&checksum_script)
            .arg(&sample)
            .env_remove("APPDIR")
            .env_remove("APPIMAGE")
            .env_remove("SQUALLZ_CLI")
            .env_remove("SQUALLZ_GUI")
            .env("SQUALLZ_QA_LOG", &gui_log)
            .env("PATH", "/usr/bin:/bin")
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "persistent AppImage handoff failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let gui_log = wait_for_log_contains(&gui_log, "<--squallz-action><checksum>");
        assert!(gui_log.contains("<--squallz-action><checksum>"));
        assert!(gui_log.contains(&format!("<{}>", sample.display())));

        fs::rename(&original_appimage, &moved_appimage).unwrap();
        let moved_appimage = validated_linux_appimage_launch(
            Some(&moved_appimage),
            Some(&mount),
            &current_exe,
            &root,
        )
        .unwrap();
        let moved_status = linux_file_manager_actions_status_at_with_localizer_and_appimage(
            &home,
            &loc,
            Some(&moved_appimage),
        )
        .unwrap();
        assert_eq!(moved_status.health, IntegrationHealthStateDto::NeedsRepair);
        assert!(moved_status.can_repair);
        assert!(moved_status.actions.iter().all(|action| {
            action.state == IntegrationActionHealthStateDto::Damaged
                && action.issue.as_deref() == Some("script_outdated")
        }));

        install_linux_file_manager_actions_at_with_localizer_and_appimage(
            &home,
            &loc,
            Some(&moved_appimage),
        )
        .unwrap();
        let repaired = linux_file_manager_actions_status_at_with_localizer_and_appimage(
            &home,
            &loc,
            Some(&moved_appimage),
        )
        .unwrap();
        assert_eq!(repaired.health, IntegrationHealthStateDto::Healthy);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn extract_and_run_appimage_actions_preserve_the_runtime_mode() {
        let root = temp_home("squallz-linux-extracted-appimage-actions");
        let home = root.join("home");
        let extracted = root.join("appimage_extracted_0123456789abcdef");
        let current_exe = extracted.join("usr/bin/squallz-gui");
        let appimage = root.join("Squallz.AppImage");
        write_fake_sh_tool(&current_exe);
        write_fake_sh_tool(&appimage);

        let launch =
            validated_linux_appimage_launch(Some(&appimage), Some(&extracted), &current_exe, &root)
                .unwrap();
        assert_eq!(launch.mode, LinuxAppImageLaunchMode::ExtractAndRun);

        let loc = Localizer::load(Some("en-US"));
        let installed = install_linux_file_manager_actions_at_with_localizer_and_appimage(
            &home,
            &loc,
            Some(&launch),
        )
        .unwrap();
        let checksum_script = script_for(&installed, "checksum");
        let script_text = fs::read_to_string(&checksum_script).unwrap();
        assert!(script_text.contains("SQUALLZ_APPIMAGE_EXTRACT_AND_RUN='1'"));

        let sample = root.join("sample.txt");
        fs::write(&sample, b"sample").unwrap();
        let gui_log = root.join("appimage-extracted-gui.log");
        let output = Command::new("/bin/bash")
            .arg(&checksum_script)
            .arg(&sample)
            .env_remove("APPDIR")
            .env_remove("APPIMAGE")
            .env_remove("APPIMAGE_EXTRACT_AND_RUN")
            .env_remove("SQUALLZ_CLI")
            .env_remove("SQUALLZ_GUI")
            .env("SQUALLZ_QA_LOG", &gui_log)
            .env("PATH", "/usr/bin:/bin")
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "persistent extracted AppImage handoff failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let gui_log = wait_for_log_contains(&gui_log, "<--squallz-action><checksum>");
        assert!(gui_log.contains("<APPIMAGE_EXTRACT_AND_RUN=1>"));

        let mount = root.join(".mount_Squallz");
        let mounted_exe = mount.join("usr/bin/squallz-gui");
        write_fake_sh_tool(&mounted_exe);
        let mounted_launch =
            validated_linux_appimage_launch(Some(&appimage), Some(&mount), &mounted_exe, &root)
                .unwrap();
        assert_eq!(mounted_launch.mode, LinuxAppImageLaunchMode::Mounted);
        let mounted_status = linux_file_manager_actions_status_at_with_localizer_and_appimage(
            &home,
            &loc,
            Some(&mounted_launch),
        )
        .unwrap();
        assert_eq!(
            mounted_status.health,
            IntegrationHealthStateDto::NeedsRepair
        );
        assert!(mounted_status.actions.iter().all(|action| {
            action.state == IntegrationActionHealthStateDto::Damaged
                && action.issue.as_deref() == Some("script_outdated")
        }));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn appimage_path_validation_rejects_environment_injection_and_symlinks() {
        let root = temp_home("squallz-linux-appimage-validation");
        let mount = root.join(".mount_Squallz");
        let current_exe = mount.join("usr/bin/squallz-gui");
        let outside_exe = root.join("outside/squallz-gui");
        let appimage = root.join("Squallz.AppImage");
        let appimage_link = root.join("Squallz-link.AppImage");
        write_fake_sh_tool(&current_exe);
        write_fake_sh_tool(&outside_exe);
        write_fake_sh_tool(&appimage);
        symlink(&appimage, &appimage_link).unwrap();

        assert_eq!(
            validated_linux_appimage_launch(Some(&appimage), Some(&mount), &current_exe, &root,),
            Some(super::LinuxAppImageLaunch {
                path: fs::canonicalize(&appimage).unwrap(),
                mode: LinuxAppImageLaunchMode::Mounted,
            })
        );
        assert!(validated_linux_appimage_launch(
            Some(Path::new("Squallz.AppImage")),
            Some(&mount),
            &current_exe,
            &root,
        )
        .is_none());
        assert!(validated_linux_appimage_launch(
            Some(&appimage_link),
            Some(&mount),
            &current_exe,
            &root,
        )
        .is_none());
        assert!(validated_linux_appimage_launch(
            Some(&appimage),
            Some(&mount),
            &outside_exe,
            &root,
        )
        .is_none());

        let injected_appdir = root.join("not-an-appimage-mount");
        fs::create_dir_all(&injected_appdir).unwrap();
        assert!(validated_linux_appimage_launch(
            Some(&appimage),
            Some(&injected_appdir),
            &outside_exe,
            &root,
        )
        .is_none());

        let mut permissions = fs::metadata(&appimage).unwrap().permissions();
        permissions.set_mode(0o644);
        fs::set_permissions(&appimage, permissions).unwrap();
        assert!(validated_linux_appimage_launch(
            Some(&appimage),
            Some(&mount),
            &current_exe,
            &root,
        )
        .is_none());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn non_executable_kde_service_requires_repair() {
        let home = temp_home("squallz-linux-integration-kde-permissions");
        let installed =
            install_linux_file_manager_actions_at_with_language(&home, Some("en-US")).unwrap();
        let service = Path::new(&installed.installed[0].path);
        let mut permissions = fs::metadata(service).unwrap().permissions();
        permissions.set_mode(0o644);
        fs::set_permissions(service, permissions).unwrap();

        let status =
            linux_file_manager_actions_status_at_with_language(&home, Some("en-US")).unwrap();
        assert_eq!(status.health, IntegrationHealthStateDto::NeedsRepair);
        assert!(status.can_repair);
        let damaged = status
            .actions
            .iter()
            .find(|action| action.id == installed.installed[0].id)
            .unwrap();
        assert_eq!(damaged.state, IntegrationActionHealthStateDto::Damaged);
        assert_eq!(damaged.issue.as_deref(), Some("script_not_executable"));

        install_linux_file_manager_actions_at_with_language(&home, Some("en-US")).unwrap();
        assert_ne!(
            fs::metadata(service).unwrap().permissions().mode() & 0o111,
            0
        );
        let repaired =
            linux_file_manager_actions_status_at_with_language(&home, Some("en-US")).unwrap();
        assert_eq!(repaired.health, IntegrationHealthStateDto::Healthy);

        let _ = fs::remove_dir_all(home);
    }

    #[test]
    fn desktop_exec_argument_escapes_both_parsing_layers_and_field_codes() {
        let escaped = desktop_exec_argument("/tmp/Squallz\\tools %F\n$tool`\"");
        assert!(escaped.starts_with('"') && escaped.ends_with('"'));
        assert!(escaped.contains("\\\\\\\\"));
        assert!(escaped.contains("%%F"));
        assert!(escaped.contains("\\n"));
        assert!(!escaped.contains('\n'));
        assert!(escaped.contains("\\\\$"));
        assert!(escaped.contains("\\\\`"));
        assert!(escaped.contains("\\\\\""));
    }

    fn temp_home(prefix: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("{prefix}-{nonce}"))
    }

    fn script_for(result: &super::IntegrationApplyResultDto, id: &str) -> String {
        result
            .installed
            .iter()
            .find(|item| item.id == id)
            .map(|item| item.script_path.clone())
            .unwrap_or_else(|| panic!("missing script for {id}"))
    }

    fn run_linux_action_script(script: &str, fake_sqz: &Path, log: &Path, inputs: &[PathBuf]) {
        let mut command = Command::new("/bin/bash");
        command
            .arg(script)
            .env("SQUALLZ_CLI", fake_sqz)
            .env("SQUALLZ_QA_LOG", log)
            .env("SQUALLZ_DISABLE_GUI_HANDOFF", "1")
            .env("PATH", "/usr/bin:/bin")
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        for input in inputs {
            command.arg(input);
        }
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "script {script} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn run_linux_action_script_with_gui(
        script: &str,
        fake_gui: &Path,
        log: &Path,
        inputs: &[PathBuf],
    ) {
        let mut command = Command::new("/bin/bash");
        command
            .arg(script)
            .env("SQUALLZ_GUI", fake_gui)
            .env("SQUALLZ_QA_LOG", log)
            .env("PATH", "/usr/bin:/bin")
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        for input in inputs {
            command.arg(input);
        }
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "script {script} failed with gui: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn run_linux_action_script_with_gui_from(
        script: &str,
        fake_gui: &Path,
        log: &Path,
        current_dir: &Path,
        inputs: &[&str],
    ) {
        let mut command = Command::new("/bin/bash");
        command
            .arg(script)
            .current_dir(current_dir)
            .env("SQUALLZ_GUI", fake_gui)
            .env("SQUALLZ_QA_LOG", log)
            .env("PATH", "/usr/bin:/bin")
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .args(inputs);
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "script {script} failed with relative gui inputs: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn wait_for_log_contains(path: &Path, needle: &str) -> String {
        for _ in 0..300 {
            if let Ok(contents) = fs::read_to_string(path) {
                if contents.contains(needle) {
                    return contents;
                }
            }
            thread::sleep(Duration::from_millis(10));
        }
        fs::read_to_string(path).unwrap_or_default()
    }

    fn write_fake_sh_tool(path: &Path) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(
            path,
            r#"#!/bin/sh
lock="${SQUALLZ_QA_LOG}.lock"
while ! mkdir "$lock" 2>/dev/null; do
  sleep 0.01
done
trap 'rmdir "$lock"' EXIT
{
  printf '<APPIMAGE_EXTRACT_AND_RUN=%s>' "${APPIMAGE_EXTRACT_AND_RUN:-}"
  for arg in "$@"; do
    printf '<%s>' "$arg"
  done
  printf '\n'
} >> "$SQUALLZ_QA_LOG"
"#,
        )
        .unwrap();
        let mut perms = fs::metadata(path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(path, perms).unwrap();
    }
}

#[cfg(test)]
#[cfg(any(target_os = "macos", target_os = "windows"))]
mod windows_explorer_tests {
    use super::{
        install_windows_explorer_actions_at_with_language,
        install_windows_explorer_actions_at_with_localizer,
        remove_windows_explorer_actions_at_with_language,
        windows_explorer_actions_status_at_with_language,
        windows_explorer_actions_status_at_with_localizer, windows_integration_dirs,
        windows_registry_manifest_path,
    };
    use crate::dto::{IntegrationActionHealthStateDto, IntegrationHealthStateDto};
    use squallz_core::lock_unpoisoned;
    use squallz_i18n::Localizer;
    use std::fs;
    use std::path::{Path, PathBuf};
    #[cfg(target_os = "windows")]
    use std::process::Command;
    use std::sync::Mutex;
    use std::time::{SystemTime, UNIX_EPOCH};

    static WINDOWS_EXPLORER_TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn installs_windows_explorer_actions_that_reuse_task_window_handoff() {
        let _test_lock = lock_unpoisoned(&WINDOWS_EXPLORER_TEST_LOCK);
        let _registry = IsolatedTestRegistry::new();
        let data_dir = temp_dir("squallz-windows-explorer-test");
        let result =
            install_windows_explorer_actions_at_with_language(&data_dir, Some("en-US")).unwrap();

        assert_eq!(result.platform, "windows");
        assert_eq!(result.installed.len(), 5);
        assert!(result.unsupported.is_empty());

        let (_, script_dir) = windows_integration_dirs(&data_dir);
        let manifest = windows_registry_manifest_path(&script_dir);
        assert!(manifest.is_file());
        let manifest_text = fs::read_to_string(&manifest).unwrap();
        assert!(manifest_text.contains("Windows Registry Editor Version 5.00"));
        assert!(manifest_text.contains(
            "HKEY_CURRENT_USER\\Software\\Classes\\SystemFileAssociations\\.zip\\shell\\Squallz.extract-here"
        ));
        assert!(manifest_text
            .contains("HKEY_CURRENT_USER\\Software\\Classes\\*\\shell\\Squallz.checksum"));
        assert!(manifest_text.contains(
            "HKEY_CURRENT_USER\\Software\\Classes\\Directory\\shell\\Squallz.compress-to-7z"
        ));
        assert!(manifest_text.contains("\"MultiSelectModel\"=\"Player\""));
        assert!(manifest_text.contains("powershell.exe -NoProfile -ExecutionPolicy Bypass -File"));
        assert!(manifest_text.contains("%1"));

        for action in &result.installed {
            let script = Path::new(&action.script_path);
            assert!(script.is_file());
            let script_text = fs::read_to_string(script).unwrap();
            assert!(script_text.contains("[string[]]$Paths"));
            assert!(script_text.contains("Invoke-SquallzGuiTask"));
            assert!(script_text.contains("--squallz-action"));
            assert!(script_text.contains("$SquallzTaskWindowActionArg = '--squallz-action'"));
            assert!(script_text.contains("$Arguments = @($SquallzTaskWindowActionArg, $Action)"));
            assert!(script_text.contains("Start-Process"));
            assert!(script_text.contains("Resolve-Sqz"));
            assert!(script_text.contains("SQUALLZ_CLI"));
            assert_powershell_syntax(script);
            assert!(manifest_text.contains(&action.name));
            assert!(manifest_text.contains(&path_fragment(script)));
            if matches!(
                action.id.as_str(),
                "extract-here" | "extract-to-folder" | "compress-to-7z"
            ) {
                assert!(script_text.contains("--file-manager-preset"));
            }
            if action.id == "compress-to-7z" {
                assert!(script_text.contains("--squallz-output"));
                assert!(script_text.contains("$SquallzTaskWindowOutputArg = '--squallz-output'"));
                assert!(script_text.contains("Archive.7z"));
            }
        }

        let status =
            windows_explorer_actions_status_at_with_language(&data_dir, Some("en-US")).unwrap();
        assert_eq!(status.actions.len(), 5);
        assert_eq!(status.health, IntegrationHealthStateDto::Healthy);
        assert!(status
            .actions
            .iter()
            .all(|action| action.state == IntegrationActionHealthStateDto::Healthy));

        let removed =
            remove_windows_explorer_actions_at_with_language(&data_dir, Some("en-US")).unwrap();
        assert_eq!(removed.removed.len(), 5);
        assert!(removed.missing.is_empty());

        let status_after_remove =
            windows_explorer_actions_status_at_with_language(&data_dir, Some("en-US")).unwrap();
        assert!(status_after_remove
            .actions
            .iter()
            .all(|action| action.state == IntegrationActionHealthStateDto::Missing));
        assert_eq!(
            status_after_remove.health,
            IntegrationHealthStateDto::Missing
        );
        assert!(!manifest.exists());

        let _ = fs::remove_dir_all(data_dir);
    }

    #[test]
    fn altered_windows_registry_manifest_requires_repair_and_can_be_removed() {
        let _test_lock = lock_unpoisoned(&WINDOWS_EXPLORER_TEST_LOCK);
        let _registry = IsolatedTestRegistry::new();
        let data_dir = temp_dir("squallz-windows-explorer-damaged-registry");
        install_windows_explorer_actions_at_with_language(&data_dir, Some("en-US")).unwrap();
        let (_, script_dir) = windows_integration_dirs(&data_dir);
        let manifest = windows_registry_manifest_path(&script_dir);
        let contents = fs::read_to_string(&manifest).unwrap();
        fs::write(&manifest, contents.replace("\"Player\"", "\"Single\"")).unwrap();

        let status =
            windows_explorer_actions_status_at_with_language(&data_dir, Some("en-US")).unwrap();
        assert_eq!(status.health, IntegrationHealthStateDto::NeedsRepair);
        assert!(status.can_remove);
        assert!(status.actions.iter().all(|action| {
            action.state == IntegrationActionHealthStateDto::Damaged
                && action.issue.as_deref() == Some("registry_outdated")
        }));

        let removed =
            remove_windows_explorer_actions_at_with_language(&data_dir, Some("en-US")).unwrap();
        assert_eq!(removed.removed.len(), 5);
        let after_remove =
            windows_explorer_actions_status_at_with_language(&data_dir, Some("en-US")).unwrap();
        assert_eq!(after_remove.health, IntegrationHealthStateDto::Missing);
        assert!(!manifest.exists());

        let _ = fs::remove_dir_all(data_dir);
    }

    #[test]
    fn custom_language_pack_names_windows_explorer_verbs_without_code_changes() {
        let _test_lock = lock_unpoisoned(&WINDOWS_EXPLORER_TEST_LOCK);
        let _registry = IsolatedTestRegistry::new();
        let data_dir = temp_dir("squallz-windows-explorer-locale-home");
        let locale_dir = temp_dir("squallz-windows-explorer-locale-pack");
        fs::create_dir_all(&locale_dir).unwrap();
        fs::write(
            locale_dir.join("xx-XX.json"),
            r#"{
  "meta.name": "XX",
  "gui.integration.explorer.action.checksum": "Squallz XX Checksum",
  "gui.integration.explorer.action.extract_here": "Squallz XX Extract",
  "gui.integration.explorer.action.extract_to_folder": "Squallz XX Folder",
  "gui.integration.explorer.action.compress_to_7z": "Squallz XX 7Z",
  "gui.integration.explorer.action.test_archive": "Squallz XX Test",
  "gui.integration.explorer.cli_not_found.title": "XX CLI missing",
  "gui.integration.explorer.cli_not_found.message": "XX install CLI"
}"#,
        )
        .unwrap();
        let loc = Localizer::with_user_dir(Some("xx-XX"), Some(&locale_dir));

        let result = install_windows_explorer_actions_at_with_localizer(&data_dir, &loc).unwrap();
        let extract = result
            .installed
            .iter()
            .find(|item| item.id == "extract-here")
            .unwrap();
        assert_eq!(extract.name, "Squallz XX Extract");

        let (_, script_dir) = windows_integration_dirs(&data_dir);
        let manifest_text =
            fs::read_to_string(windows_registry_manifest_path(&script_dir)).unwrap();
        assert!(manifest_text.contains("Squallz XX Extract"));
        assert!(!manifest_text.contains("Squallz Extract Here"));

        let script_text = fs::read_to_string(&extract.script_path).unwrap();
        assert!(script_text.contains("XX CLI missing"));
        assert!(script_text.contains("XX install CLI"));

        let status = windows_explorer_actions_status_at_with_localizer(&data_dir, &loc).unwrap();
        assert_eq!(status.actions.len(), 5);
        assert!(status
            .actions
            .iter()
            .any(|item| item.name == "Squallz XX Extract"));

        let removed =
            remove_windows_explorer_actions_at_with_language(&data_dir, Some("xx-XX")).unwrap();
        assert_eq!(removed.removed.len(), 5);

        let _ = fs::remove_dir_all(data_dir);
        let _ = fs::remove_dir_all(locale_dir);
    }

    fn temp_dir(prefix: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("{prefix}-{nonce}"))
    }

    fn path_fragment(path: &Path) -> String {
        let value = path.to_string_lossy().into_owned();
        if cfg!(target_os = "windows") {
            value.replace('\\', "\\\\")
        } else {
            value
        }
    }

    struct IsolatedTestRegistry;

    impl IsolatedTestRegistry {
        fn new() -> Self {
            clear_test_registry();
            Self
        }
    }

    impl Drop for IsolatedTestRegistry {
        fn drop(&mut self) {
            clear_test_registry();
        }
    }

    #[cfg(target_os = "windows")]
    fn clear_test_registry() {
        let _ = Command::new("reg.exe")
            .args(["delete", super::windows_registry_root(), "/f"])
            .output();
    }

    #[cfg(not(target_os = "windows"))]
    fn clear_test_registry() {}

    #[cfg(target_os = "windows")]
    fn assert_powershell_syntax(script: &Path) {
        let status = Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-Command",
                "$errors = $null; $null = [System.Management.Automation.Language.Parser]::ParseFile($env:SQUALLZ_SCRIPT_TO_PARSE, [ref]$null, [ref]$errors); if ($errors.Count -ne 0) { $errors | Out-String | Write-Error; exit 1 }",
            ])
            .env("SQUALLZ_SCRIPT_TO_PARSE", script)
            .status()
            .expect("PowerShell must be available on Windows");
        assert!(status.success(), "PowerShell rejected {}", script.display());
    }

    #[cfg(not(target_os = "windows"))]
    fn assert_powershell_syntax(_script: &Path) {}
}
