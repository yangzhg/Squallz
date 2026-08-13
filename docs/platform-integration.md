# Squallz Platform File-Manager Integration

Updated: 2026-07-18

Squallz installs optional file-manager actions from the desktop app. The
installers are platform-specific, but every action hands work to the existing
GUI task window and shared job model. If the GUI cannot be started, the wrapper
falls back to the shared `sqz` CLI. Archive behavior does not live in the
workflow, registry entry, or launcher.

## Product rules

- Installation and removal require an explicit user action.
- Squallz does not silently become the default archive application.
- A healthy result only covers files and registrations managed by Squallz.
- Finder/Explorer visibility, default handlers, system permissions, signing,
  notarization, and operating-system trust are separate states.
- Installation is reversible. Partial writes remain visible as damage and the
  same installer repairs missing or outdated artifacts. Unexpected file types
  and symbolic links fail safely instead of being overwritten.
- GUI handoff and CLI fallback resolve the same file-manager preset. See
  [`PRESETS.md`](PRESETS.md).

The five actions are the same on each supported desktop:

- Checksum
- Extract Here
- Extract to Folder
- Compress to 7Z
- Test Archive

## Health contract

The settings page reports each action as `healthy`, `missing`, or `damaged`.
The overall result is derived from those five action results:

- `healthy`: every managed artifact matches this Squallz version.
- `needs_repair`: at least one action has a managed artifact but is incomplete,
  duplicated, non-executable, or out of date.
- `missing`: none of the expected managed artifacts exists.
- `unavailable`: the platform is unsupported or its status could not be read.

Install and Repair use the same idempotent backend. The interface reads status
again after the write and only reports success when all five actions are
healthy. Status uses only the per-action health records; aggregate state is
derived from that single source of truth.

This action-file check does not claim that a menu is visible in the current
file manager. Default applications are read by the separate system
diagnostic described below and are never changed by Install or Repair.

## System diagnostics

On macOS, Squallz asks the public `NSWorkspace` API which application currently
opens each of the 23 extensions declared in `tauri.conf.json`. The query uses
private, empty probe files in the system temporary directory and removes them
afterward. The IPC response contains only the extension, a classification, and
the application display name; application paths are not returned or logged.

The aggregate result is `squallz`, `mixed`, `other`, `unknown`, or
`unavailable`. A missing answer for one extension stays visible as `unknown`
instead of being guessed from the app bundle's `Alternate` handler rank.
Diagnostic failure is independent from action-file health and cannot turn a
healthy action installation into damage.

macOS does not provide a reliable public status proving that these Automator
Quick Actions are enabled and visible for the current Finder selection. Squallz
therefore reports `manual_check`, explains that the action files and menu
visibility are different facts, and links to Apple's current default-app and
Finder Quick Action guides. Windows and Linux return a structured unsupported
diagnostic until their native association readers are implemented.

## macOS

The app installs five Finder Services / Quick Actions under
`~/Library/Services`. Workflow directory names are stable
(`Squallz-{action-id}.workflow`); translated names are stored only in the
workflow metadata shown by macOS. Shell wrappers live under
`~/Library/Application Support/Squallz/context-actions`.

Status checks the expected script contents, executable bit, workflow count,
bundle metadata, Automator document, script target, and managed directory chain.
Repair removes stale Squallz workflows by bundle identifier and recreates the
fixed-name workflow.
After installation or removal, Squallz calls the public
`NSUpdateDynamicServices()` refresh function. A successful refresh is not
treated as evidence that Finder is showing or enabling an action.
Before replacement, the installer rejects symbolic links and unexpected file
types in the managed output or parent chain. Uninstall removes an exact owned
link without following it. This is a same-user safety check rather than a
kernel-atomic no-follow transaction; a process racing the check and write is
outside the current integration threat boundary.

Finder discovery and enablement remain outside this contract. The user may
still need to enable a Quick Action in System Settings, and File Provider
locations can apply their own menu restrictions. Finder Sync is not used: Apple
defines it for synchronised-folder integrations, not as a general-purpose
archive menu.

System references:

- [NSWorkspace default-application query](https://developer.apple.com/documentation/appkit/nsworkspace/urlforapplication%28toopen%3A%29-95cvp)
- [Change the default app for a file](https://support.apple.com/guide/mac-help/mh35597/mac)
- [Use Quick Actions in Finder](https://support.apple.com/guide/mac-help/mchl97ff9142/mac)
- [Login Items & Extensions settings](https://support.apple.com/guide/mac-help/mtusr003/mac)

Packaged workflow validation:

- `scripts/macos_packaged_quick_actions_smoke.sh` runs all five workflows
  through `/usr/bin/automator` against the packaged first-party CLI.
- `scripts/macos_packaged_integration_cleanup_smoke.sh` checks installation and
  complete cleanup of all five actions.
- `scripts/macos_finder_ui_preflight.sh` checks whether an attended visible
  Finder-menu test can run.
- `scripts/macos_finder_context_menu_smoke.sh` performs the final visible
  Finder context-menu check. A hidden or locked desktop must produce a blocked
  report, never a pass.

## Windows

The app installs per-user classic Explorer verbs under
`HKCU\Software\Classes` and PowerShell wrappers in the Squallz user data
directory. No administrator access is required. Status compares each wrapper
and registry manifest with generated content, then verifies every action name,
icon, multi-selection value, and command stored in the registry. Partial key
sets remain removable. Removal clears registry entries before deleting wrappers
so an error cannot leave a menu pointing at an already removed script.

These entries appear in the classic / Show more options menu on Windows 11.
Top-level Windows 11 integration needs `IExplorerCommand`, package identity
(MSIX or a sparse package), signing, and target-platform testing; none of those
is implied by the current health result.

Remaining Windows release work:

- validate registry values and command invocation on a real Windows host;
- add package lifecycle integration for the verbs;
- test multi-selection, PowerShell policy, SmartScreen, and signed installers;
- decide whether a packaged `IExplorerCommand` implementation is justified.

## Linux

The app installs user-local artifacts below `XDG_DATA_HOME` (or
`~/.local/share`): five KDE service-menu entries, five Nautilus launchers, and
five shared action scripts. Status compares generated contents, checks
executable bits, and treats duplicate launchers as repairable damage. Desktop
Entry `Exec` arguments use Desktop Entry quoting rather than shell quoting.

Squallz currently installs both KDE and Nautilus entries because it does not
yet have a reliable desktop/file-manager capability probe. A healthy result
therefore means that the managed files are correct, not that Nautilus or Dolphin
is installed, running, or displaying them. There is no Thunar installer and no
`xdg-mime` or default-application claim.

Official `linux-x64` packages are built on Ubuntu 22.04, and the release job
rejects a packaging host whose glibc baseline is not 2.35. This bounds the
glibc version available to Squallz binaries at build time; it does not claim
that every Linux distribution can run the AppImage. WebKitGTK, the graphics
stack, AppImage runtime behavior, and file-manager integration still require
native package smoke tests on each supported distribution.

Remaining Linux release work:

- run native Nautilus and Dolphin smoke tests on supported distributions;
- validate service menus with the target distribution's tooling;
- complete AppImage/deb/rpm and WebKitGTK packaging checks;
- document sandbox-specific paths for Flatpak or other confined builds before
  offering in-app installation there.

## Task-window boundary

Platform wrappers use `--squallz-action` and, where needed,
`--squallz-output`. The contract is owned by
`crates/squallz-gui/src/open_files.rs`; wrappers must reuse those constants.

The external `task` window has a separate minimal Tauri capability. It can
listen for job/password/conflict events, show or close itself, and open or
reveal a completed result. It does not receive the main window's file-dialog
permission.

Windows and Linux logic is covered by host-independent tests on macOS, but that
does not prove native shell behavior. Release readiness still requires tests on
the target operating system and a clean user account.
