//! Desktop/file-manager integration installers.
//!
//! macOS ships context-menu style actions through Finder Services / Quick
//! Actions. Linux file managers use user-local script/service-menu entries.
//! Windows uses user-local Explorer registry verbs plus wrapper scripts. All
//! routes launch the existing GUI task window first and only fall back to the
//! `sqz` CLI when the app handoff is unavailable.

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
use std::fs;
use std::io;
#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
use std::path::{Path, PathBuf};

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
use crate::dto::IntegrationActionDto;
use crate::dto::{IntegrationApplyResultDto, IntegrationRemoveResultDto, IntegrationStatusDto};
#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
use crate::open_files::{EXTERNAL_TASK_ACTION_ARG, EXTERNAL_TASK_OUTPUT_ARG};
#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
use squallz_i18n::Localizer;

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
  run_sqz extract "$item" -d "$dest" --smart
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
  run_sqz extract "$item" -d "$dest"
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
run_sqz compress "$@" -o "$output" --level 5
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
    candidates+=("${SQUALLZ_INSTALLED_APP_BUNDLE}/Contents/Resources/bin/sqz")
    candidates+=("${SQUALLZ_INSTALLED_APP_BUNDLE}/Contents/MacOS/sqz")
  fi
  if [[ -n "${SQUALLZ_APP_BUNDLE:-}" ]]; then
    candidates+=("${SQUALLZ_APP_BUNDLE}/Contents/Resources/bin/sqz")
    candidates+=("${SQUALLZ_APP_BUNDLE}/Contents/MacOS/sqz")
  fi
  candidates+=("/Applications/Squallz.app/Contents/Resources/bin/sqz")
  candidates+=("/Applications/Squallz.app/Contents/MacOS/sqz")
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
if run_gui_task "extract-here" "$@"; then
  exit 0
fi
for item in "$@"; do
  [[ -f "$item" ]] || continue
  dest="$(dirname -- "$item")"
  run_sqz extract "$item" -d "$dest" --smart
done
"#,
    },
    LinuxFileManagerAction {
        id: "extract-to-folder",
        name_key: "gui.integration.file_manager.action.extract_to_folder",
        script_name: "squallz-extract-to-folder.sh",
        desktop_name: "squallz-extract-to-folder.desktop",
        script_body: r#"
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
  run_sqz extract "$item" -d "$dest"
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
run_sqz compress "$@" -o "$output" --level 5
"#,
    },
    LinuxFileManagerAction {
        id: "test-archive",
        name_key: "gui.integration.file_manager.action.test_archive",
        script_name: "squallz-test-archive.sh",
        desktop_name: "squallz-test-archive.desktop",
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

#[cfg(any(target_os = "linux", all(test, target_os = "macos")))]
const LINUX_SCRIPT_PREAMBLE_TEMPLATE: &str = r#"#!/usr/bin/env bash
set -euo pipefail

CLI_NOT_FOUND_TITLE={cli_not_found_title}
CLI_NOT_FOUND_MESSAGE={cli_not_found_message}
SQUALLZ_TASK_WINDOW_ACTION_ARG={task_window_action_arg}
SQUALLZ_TASK_WINDOW_OUTPUT_ARG={task_window_output_arg}

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

run_gui_task() {
  local action gui
  action="$1"
  shift
  gui="$(resolve_gui 2>/dev/null || true)"
  [[ -n "$gui" ]] || return 1
  "$gui" "$SQUALLZ_TASK_WINDOW_ACTION_ARG" "$action" "$@" >/dev/null 2>&1 &
}

run_gui_task_with_output() {
  local action output gui
  action="$1"
  output="$2"
  shift 2
  gui="$(resolve_gui 2>/dev/null || true)"
  [[ -n "$gui" ]] || return 1
  "$gui" "$SQUALLZ_TASK_WINDOW_ACTION_ARG" "$action" "$SQUALLZ_TASK_WINDOW_OUTPUT_ARG" "$output" "$@" >/dev/null 2>&1 &
}

SQZ=""
run_sqz() {
  if [[ -z "$SQZ" ]]; then
    SQZ="$(resolve_sqz)"
  fi
  "$SQZ" "$@"
}
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
  Invoke-Sqz extract $Item -d $Dest --smart
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
  Invoke-Sqz extract $Item -d $Dest
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
$Arguments = @('compress') + $Selected + @('-o', $Output, '--level', '5')
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
        install_macos_finder_actions_at_with_language(&home, language)
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
            installed: Vec::new(),
            missing: Vec::new(),
            unsupported: vec![
                "Desktop file-manager integration is not available on this platform".to_owned(),
            ],
        })
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
        remove_macos_finder_actions_at_with_language(&home, language)
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
    fs::create_dir_all(&services_dir)?;
    fs::create_dir_all(&script_dir)?;
    let preamble = finder_script_preamble(current_app_bundle_path().as_deref(), loc);

    let mut installed = Vec::new();
    for action in FINDER_ACTIONS {
        let name = action_name(action, loc);
        let workflow_dir = workflow_path_for_name(&services_dir, &name);
        remove_stale_workflows(&services_dir, action, &workflow_dir)?;

        let script_path = script_dir.join(action.script_name);
        fs::write(
            &script_path,
            format!("{preamble}\n{}", action.script_body.trim_start()),
        )?;
        make_executable(&script_path)?;

        let contents_dir = workflow_dir.join("Contents");
        fs::create_dir_all(&contents_dir)?;
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
    let mut installed = Vec::new();
    let mut missing = Vec::new();
    for action in FINDER_ACTIONS {
        let script = script_dir.join(action.script_name);
        if script.is_file() {
            if let Some(workflow) = installed_workflow_dir(&services_dir, action)? {
                if let Some(name) = workflow_display_name(&workflow) {
                    installed.push(action_dto_with_name(
                        action,
                        &name,
                        &services_dir,
                        &script_dir,
                    ));
                    continue;
                }
            }
        }
        missing.push(action_name(action, loc));
    }

    Ok(IntegrationStatusDto {
        platform: "macos".to_owned(),
        services_dir: path_to_string(&services_dir),
        script_dir: path_to_string(&script_dir),
        installed,
        missing,
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
    let (services_dir, script_dir, nautilus_dir) = linux_integration_dirs(home);
    fs::create_dir_all(&services_dir)?;
    fs::create_dir_all(&script_dir)?;
    fs::create_dir_all(&nautilus_dir)?;
    let preamble = linux_script_preamble(loc);

    let mut installed = Vec::new();
    for action in LINUX_FILE_MANAGER_ACTIONS {
        let name = linux_action_name(action, loc);
        let script_path = script_dir.join(action.script_name);
        fs::write(
            &script_path,
            format!("{preamble}\n{}", action.script_body.trim_start()),
        )?;
        make_executable(&script_path)?;

        let service_path = linux_service_menu_path(&services_dir, action);
        fs::write(
            &service_path,
            linux_service_menu(action, &name, &script_path),
        )?;

        let nautilus_path = linux_nautilus_action_path(&nautilus_dir, &name);
        remove_stale_nautilus_scripts(&nautilus_dir, action, &nautilus_path)?;
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
fn linux_script_preamble(loc: &Localizer) -> String {
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
    let (services_dir, script_dir, nautilus_dir) = linux_integration_dirs(home);
    let mut installed = Vec::new();
    let mut missing = Vec::new();
    for action in LINUX_FILE_MANAGER_ACTIONS {
        let name = linux_action_name(action, loc);
        let script = script_dir.join(action.script_name);
        let service = linux_service_menu_path(&services_dir, action);
        let nautilus = installed_nautilus_script(&nautilus_dir, action)?;
        if script.is_file() && service.is_file() && nautilus.is_some() {
            installed.push(linux_action_dto_with_name(
                action,
                &name,
                &services_dir,
                &script_dir,
            ));
        } else {
            missing.push(name);
        }
    }

    Ok(IntegrationStatusDto {
        platform: "linux".to_owned(),
        services_dir: path_to_string(&services_dir),
        script_dir: path_to_string(&script_dir),
        installed,
        missing,
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
    let mut removed = Vec::new();
    let mut missing = Vec::new();

    for action in LINUX_FILE_MANAGER_ACTIONS {
        let script = script_dir.join(action.script_name);
        let service = linux_service_menu_path(&services_dir, action);
        let mut existed = script.exists() || service.exists();

        if script.exists() {
            fs::remove_file(&script)?;
        }
        if service.exists() {
            fs::remove_file(&service)?;
        }
        for nautilus in action_nautilus_scripts(&nautilus_dir, action)? {
            fs::remove_file(nautilus)?;
            existed = true;
        }

        let name = linux_action_name(action, loc);
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
    fs::create_dir_all(&script_dir)?;
    let preamble = windows_script_preamble(loc);

    let mut installed = Vec::new();
    for action in WINDOWS_EXPLORER_ACTIONS {
        let name = windows_action_name(action, loc);
        let script_path = script_dir.join(action.script_name);
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

    fs::write(
        windows_registry_manifest_path(&script_dir),
        windows_registry_manifest(&script_dir, loc),
    )?;
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
    let mut installed = Vec::new();
    let mut missing = Vec::new();
    for action in WINDOWS_EXPLORER_ACTIONS {
        let name = windows_action_name(action, loc);
        let script = script_dir.join(action.script_name);
        if script.is_file() && windows_explorer_registry_entries_installed(&script_dir, action) {
            installed.push(windows_action_dto_with_name(
                action,
                &name,
                &services_dir,
                &script_dir,
            ));
        } else {
            missing.push(name);
        }
    }

    Ok(IntegrationStatusDto {
        platform: "windows".to_owned(),
        services_dir,
        script_dir: path_to_string(&script_dir),
        installed,
        missing,
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
    let mut removed = Vec::new();
    let mut missing = Vec::new();

    for action in WINDOWS_EXPLORER_ACTIONS {
        let script = script_dir.join(action.script_name);
        let existed =
            script.exists() || windows_explorer_registry_entries_installed(&script_dir, action);

        if script.exists() {
            fs::remove_file(&script)?;
        }
        remove_windows_explorer_registry_entries(action)?;

        let name = windows_action_name(action, loc);
        if existed {
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

    let manifest = windows_registry_manifest_path(&script_dir);
    if manifest.exists() {
        fs::remove_file(manifest)?;
    }
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
    let mut removed = Vec::new();
    let mut missing = Vec::new();

    for action in FINDER_ACTIONS {
        let script = script_dir.join(action.script_name);
        let mut existed = script.exists();
        for workflow in action_workflow_dirs(&services_dir, action)? {
            fs::remove_dir_all(&workflow)?;
            existed = true;
        }
        if script.exists() {
            fs::remove_file(&script)?;
        }
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
    match std::env::var_os("XDG_DATA_HOME").filter(|value| !value.is_empty()) {
        Some(path) => PathBuf::from(path),
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
    if out.trim().is_empty() {
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
        shell_single_quote_value(&path_to_string(script_path)),
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
fn installed_nautilus_script(
    nautilus_dir: &Path,
    action: &LinuxFileManagerAction,
) -> io::Result<Option<PathBuf>> {
    Ok(action_nautilus_scripts(nautilus_dir, action)?
        .into_iter()
        .next())
}

#[cfg(any(target_os = "linux", all(test, target_os = "macos")))]
fn action_nautilus_scripts(
    nautilus_dir: &Path,
    action: &LinuxFileManagerAction,
) -> io::Result<Vec<PathBuf>> {
    let Ok(entries) = fs::read_dir(nautilus_dir) else {
        return Ok(Vec::new());
    };
    let marker = format!("SQUALLZ_ACTION_ID={}", action.id);
    let mut scripts = Vec::new();
    for entry in entries {
        let path = entry?.path();
        if !path.is_file() {
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
fn windows_explorer_registry_entries_installed(
    _script_dir: &Path,
    action: &WindowsExplorerAction,
) -> bool {
    windows_registry_keys(action)
        .iter()
        .all(|key| windows_registry_key_exists(key))
}

#[cfg(all(test, target_os = "macos"))]
fn windows_explorer_registry_entries_installed(
    script_dir: &Path,
    action: &WindowsExplorerAction,
) -> bool {
    let manifest = windows_registry_manifest_path(script_dir);
    let Ok(contents) = fs::read_to_string(manifest) else {
        return false;
    };
    windows_registry_keys(action)
        .iter()
        .all(|key| contents.contains(key))
}

#[cfg(target_os = "windows")]
fn windows_registry_key_exists(key: &str) -> bool {
    std::process::Command::new("reg.exe")
        .args(["query", key])
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
fn workflow_path_for_name(services_dir: &Path, name: &str) -> std::path::PathBuf {
    services_dir.join(format!("{name}.workflow"))
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
        fs::remove_dir_all(workflow)?;
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn installed_workflow_dir(
    services_dir: &Path,
    action: &FinderAction,
) -> io::Result<Option<PathBuf>> {
    Ok(action_workflow_dirs(services_dir, action)?
        .into_iter()
        .next())
}

#[cfg(target_os = "macos")]
fn action_workflow_dirs(services_dir: &Path, action: &FinderAction) -> io::Result<Vec<PathBuf>> {
    let Ok(entries) = fs::read_dir(services_dir) else {
        return Ok(Vec::new());
    };
    let bundle_id = action_bundle_id(action);
    let mut workflows = Vec::new();
    for entry in entries {
        let path = entry?.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("workflow") {
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
fn workflow_display_name(workflow: &Path) -> Option<String> {
    workflow
        .file_stem()
        .and_then(|name| name.to_str())
        .map(ToOwned::to_owned)
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
        path: path_to_string(&workflow_path_for_name(services_dir, name)),
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
        macos_finder_actions_status_at_with_localizer,
        remove_macos_finder_actions_at_with_language,
        remove_macos_finder_actions_at_with_localizer,
    };
    use squallz_i18n::Localizer;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn finder_script_preamble_localizes_cli_error_alert() {
        let loc = Localizer::with_user_dir(Some("zh-CN"), None);
        let preamble = finder_script_preamble(None, &loc);

        assert!(preamble.contains("找不到 Squallz 命令行工具"));
        assert!(preamble.contains("访达快捷操作"));
        assert!(preamble.contains("CLI_NOT_FOUND_ALERT='display alert"));
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
        assert_eq!(status.installed.len(), 5);
        assert!(status.missing.is_empty());

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
            log.contains(&format!("<extract><{one}><-d><{parent}><--smart>")),
            "log: {log}"
        );
        assert!(
            log.contains(&format!("<extract><{two}><-d><{parent}><--smart>")),
            "log: {log}"
        );
        assert!(
            log.contains(&format!("<extract><{one}><-d><{parent}/one>")),
            "log: {log}"
        );
        assert!(
            log.contains(&format!("<extract><{two}><-d><{parent}/two>")),
            "log: {log}"
        );
        assert!(
            log.contains(&format!(
                "<compress><{plain}><{folder}><-o><{parent}/Archive.7z><--level><5>"
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
        assert!(status.installed.is_empty());
        assert_eq!(status.missing.len(), 5);

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
        assert_eq!(status.installed.len(), 5);
        assert!(status.missing.is_empty());
        assert!(status
            .installed
            .iter()
            .any(|item| item.name == "Squallz XX Extract"));

        let removed = remove_macos_finder_actions_at_with_localizer(&home, &loc).unwrap();
        assert_eq!(removed.removed.len(), 5);
        assert!(removed.missing.is_empty());

        let _ = fs::remove_dir_all(home);
        let _ = fs::remove_dir_all(locale_dir);
    }

    #[test]
    fn localized_finder_install_replaces_legacy_english_workflows() {
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
        assert!(!Path::new(&english_extract.path).exists());

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
        assert_eq!(localized_status.installed.len(), 5);
        assert!(localized_status.missing.is_empty());
        assert!(localized_status
            .installed
            .iter()
            .any(|item| item.name == "Squallz 就地解压"));

        let legacy_status = macos_finder_actions_status_at_with_language(&home, None).unwrap();
        assert_eq!(legacy_status.installed.len(), 5);
        assert!(legacy_status.missing.is_empty());

        let removed = remove_macos_finder_actions_at_with_language(&home, Some("zh-CN")).unwrap();
        assert_eq!(removed.removed.len(), 5);
        assert!(removed.missing.is_empty());
        assert!(!Path::new(&localized_extract.path).exists());

        let status_after_remove =
            macos_finder_actions_status_at_with_language(&home, Some("zh-CN")).unwrap();
        assert!(status_after_remove.installed.is_empty());
        assert_eq!(status_after_remove.missing.len(), 5);

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
        install_linux_file_manager_actions_at_with_language,
        linux_file_manager_actions_status_at_with_language, linux_integration_dirs,
        remove_linux_file_manager_actions_at_with_language,
    };
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
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

            let service_text = fs::read_to_string(service).unwrap();
            assert!(service_text.contains("ServiceTypes=KonqPopupMenu/Plugin"));
            assert!(service_text.contains("Actions=squallz-"));
            assert!(service_text.contains("Exec='"));
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
            cli_log.contains(&format!("<extract><{one}><-d><{parent}><--smart>")),
            "log: {cli_log}"
        );
        assert!(
            cli_log.contains(&format!("<extract><{two}><-d><{parent}><--smart>")),
            "log: {cli_log}"
        );
        assert!(
            cli_log.contains(&format!("<extract><{one}><-d><{parent}/one>")),
            "log: {cli_log}"
        );
        assert!(
            cli_log.contains(&format!(
                "<compress><{plain}><{folder}><-o><{parent}/Archive.7z><--level><5>"
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
        assert_eq!(status.installed.len(), 5);
        assert!(status.missing.is_empty());

        let removed =
            remove_linux_file_manager_actions_at_with_language(&home, Some("en-US")).unwrap();
        assert_eq!(removed.removed.len(), 5);
        assert!(removed.missing.is_empty());

        let status_after_remove =
            linux_file_manager_actions_status_at_with_language(&home, Some("en-US")).unwrap();
        assert!(status_after_remove.installed.is_empty());
        assert_eq!(status_after_remove.missing.len(), 5);

        let _ = fs::remove_dir_all(home);
    }

    #[test]
    fn localized_linux_install_replaces_stale_nautilus_script_names() {
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
        assert_eq!(status.installed.len(), 5);
        assert!(status.missing.is_empty());

        let removed =
            remove_linux_file_manager_actions_at_with_language(&home, Some("zh-CN")).unwrap();
        assert_eq!(removed.removed.len(), 5);

        let _ = fs::remove_dir_all(home);
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
        fs::write(
            path,
            r#"#!/bin/sh
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
#[cfg(target_os = "macos")]
mod windows_explorer_tests {
    use super::{
        install_windows_explorer_actions_at_with_language,
        install_windows_explorer_actions_at_with_localizer,
        remove_windows_explorer_actions_at_with_language,
        windows_explorer_actions_status_at_with_language,
        windows_explorer_actions_status_at_with_localizer, windows_integration_dirs,
        windows_registry_manifest_path,
    };
    use squallz_i18n::Localizer;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn installs_windows_explorer_actions_that_reuse_task_window_handoff() {
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
            assert!(manifest_text.contains(&action.name));
            assert!(manifest_text.contains(&path_fragment(script)));
            if action.id == "compress-to-7z" {
                assert!(script_text.contains("--squallz-output"));
                assert!(script_text.contains("$SquallzTaskWindowOutputArg = '--squallz-output'"));
                assert!(script_text.contains("Archive.7z"));
            }
        }

        let status =
            windows_explorer_actions_status_at_with_language(&data_dir, Some("en-US")).unwrap();
        assert_eq!(status.installed.len(), 5);
        assert!(status.missing.is_empty());

        let removed =
            remove_windows_explorer_actions_at_with_language(&data_dir, Some("en-US")).unwrap();
        assert_eq!(removed.removed.len(), 5);
        assert!(removed.missing.is_empty());

        let status_after_remove =
            windows_explorer_actions_status_at_with_language(&data_dir, Some("en-US")).unwrap();
        assert!(status_after_remove.installed.is_empty());
        assert_eq!(status_after_remove.missing.len(), 5);
        assert!(!manifest.exists());

        let _ = fs::remove_dir_all(data_dir);
    }

    #[test]
    fn custom_language_pack_names_windows_explorer_verbs_without_code_changes() {
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
        assert_eq!(status.installed.len(), 5);
        assert!(status
            .installed
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
        path.to_string_lossy().into_owned()
    }
}
