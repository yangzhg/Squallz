# Squallz self-extracting archives

This document defines the SFX v1 boundary. It does not change the `.sqz`
container format.

## Product contract

A Squallz self-extractor can list, test or extract its payload. It never runs a
program or script from the archive. Extraction continues to use the shared
engine, including path traversal checks, symlink policy, overwrite handling,
resource limits, password handling and cancellation.

The target platform is always explicit. A stub built for one operating system
cannot be relabeled as another target.

## SFX v1 single-file layout

SFX v1 supports a Squallz-aware Windows PE or Linux ELF stub followed by one
complete ZIP-compatible payload and a fixed footer:

```text
[Squallz PE or ELF stub][ZIP payload][32-byte SQZSFX1 footer]
```

An unsigned Windows artifact ends at the footer. After final Authenticode
signing, the PE certificate table may follow the footer with up to seven bytes
of alignment padding. The runtime reads the certificate-table file offset from
the PE optional header and locates the footer immediately before it. Builders
reject an already signed stub; the correct order is assemble once, then sign
the completed artifact.

Packaged Windows NSIS installers and Linux AppImages ship a dedicated `sqz-sfx`
runtime as the data resource `bin/sqz-sfx.stub`. Standalone GUI binaries do not
contain bundle resources. The desktop app prefers the dedicated runtime over
the full `sqz` CLI. An unsigned legacy `sqz` remains a compatibility fallback
for older packages, but a signed Windows CLI cannot be used as a template
because it already has a PE certificate table. The dedicated runtime exposes
only runtime help/version and self-extraction actions; list, test or extract
without an embedded payload exits with an error.

The ZIP payload stays independently inspectable. Squallz exposes a bounded
view of the payload to the normal ZIP reader, so the reader cannot consume
stub or footer bytes. Conventional ZIP tools can also recover the payload;
Info-ZIP reports the executable prefix as an SFX warning while still
extracting and validating the files.

Footer fields are little-endian:

| Offset | Size | Field |
| ---: | ---: | --- |
| 0 | 8 | `SQZSFX1\0` magic |
| 8 | 1 | target: `1` Windows, `2` Linux, `3` reserved for macOS bundle metadata |
| 9 | 3 | flags/reserved; must be zero |
| 12 | 8 | ZIP payload offset |
| 20 | 8 | ZIP payload length |
| 28 | 4 | CRC-32 of the complete ZIP payload |

The payload must end immediately before the footer. Zero-length payloads,
overflowing offsets, nested SFX payloads and split `.001` payloads are
rejected.

## macOS app-bundle layout

macOS does not use the single-file layout. Apple states that appending data to
a Mach-O executable is prohibited and fails signature validation. Squallz
instead copies a Squallz GUI `.app` template and adds two data files:

```text
Package.app/
  Contents/
    Info.plist
    MacOS/squallz-gui
    Resources/squallz-sfx/payload.zip
    Resources/squallz-sfx/manifest.v1
```

The desktop-only `Contents/MacOS/sqz` sidecar and Quick Look extension are not
copied into the self-extractor. The generated app runs the shared extraction
task directly through its GUI runtime, so those components would only add size
and unrelated system integration.

`manifest.v1` is a fixed 64-byte record. It contains the `SQZSFXB1` magic,
version `1`, the payload length and the complete payload SHA-256. Reserved and
flag bytes must be zero. The payload and manifest must be regular files, not
symlinks.

The template XML `Info.plist` must declare a string
`LSMinimumSystemVersion`. The generated app inherits that value instead of
claiming compatibility below the template executable's deployment target.

For input-based creation, the worker prepares one template manifest before it
writes the ZIP payload. Planning and assembly consume that same entry set.
Regular files are opened without following the final symlink and must retain
their recorded filesystem identity, length and timestamps; directories and
symlinks are checked against their recorded identity and type. Entries added
after preparation are not copied, while a recorded entry that is replaced or
removed makes the build fail before publication. Generated paths such as
`Contents/Resources` must have real directory ancestors rather than symlinks.

The app launches the existing Squallz task window with an SFX extraction job.
That job verifies the SHA-256 before extraction, uses the shared password and
conflict dialogs, skips symlinks by default, keeps the normal safety limits and
extracts beside the `.app` into a same-name folder. It never auto-runs payload
content.

If the source app is signed, the builder omits its stale
`Contents/_CodeSignature` directory. The new bundle is intentionally unsigned:
sign nested executables and the outer bundle from the inside out. The outer app
signature must be last because it seals the payload and manifest as resources.

The desktop Create page exposes the current host target as an explicit
self-extracting option. It forces a single ZIP payload, runs archive creation
and SFX assembly as one cancellable task, and reports the unsigned result as a
distribution warning rather than a successful trust check.

## Safe output replacement

Replacing an existing self-extractor is a guarded, recoverable transaction.
Before the build starts, Squallz binds the exact artifact the caller approved.
After staging is complete, it coordinates publishers per output directory,
locks the exact destination, recovers any older transaction, and verifies that
binding again at the final publication boundary. A single-file SFX is bound by
its filesystem identity, selected stable metadata, and full content. A macOS
SFX binds the complete bundle tree, including member names, entry types, file
contents, metadata covered by the tree digest, and symlink targets. If the
final guard check observes that the approved artifact no longer matches,
publication stops with `destination_changed` before a new journal is written
or an output is moved. A path race after that check belongs to the recoverable
transaction boundary and can instead produce a recovery error that requires
inspection.

After the guard passes, Squallz writes a fixed
`.squallz-sfx-transaction.json` record beside the destination. New records use
transaction version 4 and keep the lossless destination spelling, any verified
filesystem alias, the identities of the holder, replacement, and previous
output, and fixed content-state digests for both old and new artifacts. Those
digests are checked before and after transaction moves and while recovery is
resumed. The record file itself keeps its publication handle and a BLAKE3
digest of the exact serialized bytes. Squallz rechecks both before destructive
moves and cleanup, after the active-to-completed rename, and before returning
from recovery; changing a record in place does not preserve its authority just
because the filesystem identity stayed the same. Each move is synced before
the next one begins. Once the replacement is installed, the active record is
atomically renamed to `.squallz-sfx-completed.json`. This happens before the
build reports success, so a crash cannot hide an unacknowledged backup.

The reader remains compatible with transaction versions 1 through 3. Those
legacy records do not contain the version 4 content digests and are recovered
under their original identity-bound rules; Squallz never rewrites or invents
missing digests in an old record. Cleanup records are written as version 2 with
a state digest, while version 1 remains readable. These are private recovery
record versions and do not change the public SFX v1 container layout.

If a build is interrupted, the next build for that destination resumes the
recorded transaction, including after a case-insensitive alias was used.
Only one completed replacement can await confirmation in a directory. Squallz
reports the retained copy at
`.squallz-sfx-holder-<pid>-<sequence>/previous`. Test the current output first,
then delete that exact `previous` path when it is no longer needed. The next
run removes the empty holder and completion record before starting another
replacement. If the current destination is missing or has changed identity or
content, do not delete the retained copy; inspect the paths reported by the CLI
or GUI.

Staging cleanup uses `.squallz-sfx-cleanup.json` and an identity-bound
quarantine. Current cleanup records also bind the quarantined content state.
Before removal, version 2 cleanup moves the quarantine without replacement to
a fresh, currently absent isolation name in the reserved namespace, syncs the
parent directory, then checks its identity and full content digest again. If
any check fails, Squallz keeps the record and the current objects for
inspection. The active, completed, cleanup, holder, stage, payload, and
quarantine names form a reserved internal namespace. A strict internal name
without a valid owning record is treated as recovery debt, not as source data.
This keeps a crashed build out of a later archive.

The final rename or removal on POSIX still uses the checked path. Another
process owned by the same user can race a path replacement between the last
check and that system call. Later identity, record, and content checks stop a
subsequent destructive step only after they observe a change. An object rebound
inside the final check-to-syscall window is not guaranteed to be preserved;
these checks do not provide handle-based atomic rename or deletion.

Squallz opens the fixed records directly; it does not scan the directory for
transaction candidates. When the destination is inside the source tree, paths
bound to a valid transaction or cleanup record are excluded from the payload.
Invalid records stop creation and are left untouched for inspection.

References:

- [Apple TN2206: macOS Code Signing In Depth](https://developer.apple.com/library/archive/technotes/tn2206/)
- [Apple: Code Signing Tasks](https://developer.apple.com/library/archive/documentation/Security/Conceptual/CodeSigningGuide/Procedures/Procedures.html)
- [Microsoft: code signing options for Windows app developers](https://learn.microsoft.com/en-us/windows/apps/package-and-deploy/code-signing-options)

## CLI surface

Create a target artifact from a complete ZIP payload:

```text
sqz sfx create payload.zip --target windows --stub sqz-sfx.stub -o package.exe
sqz sfx create payload.zip --target linux --stub sqz-sfx.stub -o package.run
sqz sfx create payload.zip --target macos --stub Squallz.app -o Package.app
```

Creation is no-replace by default. `--force` authorizes replacement only for
the artifact observed when the command starts. If the final guard check sees a
different artifact, the command reports `destination_changed` before moving an
output. A later active path race can instead produce a recovery error with
paths to inspect.

When Windows/Linux host and target match, `sqz` first looks for the packaged
`sqz-sfx.stub` and falls back to the current unsigned `sqz` binary for older
or development layouts. A packaged macOS `sqz` automatically finds its
enclosing `Squallz.app`; development builds use `--stub Squallz.app`. The
builder checks the executable format and the correct Squallz runtime marker
before writing.

Inspect and verify an artifact without executing it:

```text
sqz sfx inspect package.exe
```

On macOS, publish a new Developer ID-signed and notarized copy without changing
the unsigned source:

```text
sqz sfx publish-macos Package.app \
  -o Package-published.app \
  --identity "Developer ID Application: Example Corp (TEAMID1234)" \
  --notary-profile "squallz-sfx"
```

The identity must already be available to `codesign`, and the profile must
already exist in the login Keychain (for example, after an explicit
`xcrun notarytool store-credentials "squallz-sfx"` setup). Squallz receives
only the identity selector and profile name. It does not accept, read or store
the certificate password, Apple ID password, app-specific password or API
private key.

The desktop app exposes the same workflow from a completed, still-verifiable
unsigned macOS SFX result. It lists the Developer ID Application identities
available to `codesign`, asks for a separate output and an existing Keychain
profile, then runs the request through the normal task queue. The task
snapshot, history, audit record and user-visible error omit the selected
identity and profile.

`publish-macos` is deliberately no-replace. It verifies and snapshots the
source bundle, copies it into a private sibling workspace, signs a legacy
`Contents/MacOS/sqz` sidecar when present and the outer app last, then requires
all of the following before publishing the requested output:

1. a timestamped hardened-runtime Developer ID Application signature;
2. strict recursive `codesign` verification;
3. an `Accepted` JSON result and a matching accepted notarization log;
4. successful ticket staple and staple validation;
5. a successful Gatekeeper execution assessment;
6. an unchanged, fully verified SFX payload.

The submission ZIP and private app copy are removed before the final
same-filesystem, no-replace move. A failed or cancelled run leaves the source
unchanged and does not replace a path that appeared while notarization was in
progress. Pressing Ctrl-C terminates the active local tool; a submission that
Apple already received may continue processing on the service.

When the self-extractor itself runs, its default action is safe extraction to
a same-name folder in the current directory. It also accepts:

```text
package.exe --list
package.exe --test
package.exe -d destination --overwrite ask
```

The runtime verifies the full payload CRC before list, test or extract. It
does not accept a password argument; encrypted payloads use the existing
interactive password flow so secrets are not added to the process command
line.

## Release requirements

- Assemble first, then sign the final PE/ELF artifact or macOS app bundle.
- Use an unsigned Windows stub. Authenticode signing belongs after SFX
  assembly, and the signed result must be retested with `sqz sfx inspect` and
  executed on Windows. A typical publisher command is
  `signtool sign /fd SHA256 /tr <RFC3161-timestamp-URL> /td SHA256 package.exe`.
  Never sign `sqz-sfx-template.stub` itself. Its extension must also stay
  `.stub` in the bundler source path so Windows package signing does not treat
  it as an executable resource.
- Preserve the executable mode on the packaged Linux template. SFX assembly
  adds the owner execute bit to the final `.run`; release smoke must verify the
  mode and launch that exact output on Linux.
- Publish the NSIS installer as the exact Windows desktop update asset. Publish
  a tar archive containing `Squallz.AppImage` as the exact Linux desktop update
  asset. Do not put a standalone GUI executable under either update name: it
  has no bundled SFX runtime. Windows and Linux packages remain explicitly
  marked as unsigned previews until their platform signing pipelines exist.
- A signed macOS source app is accepted, but its outer signature is not copied.
  The generated SFX omits the desktop CLI sidecar and Quick Look extension.
  Sign the outer `.app` with hardened runtime and a secure timestamp, verify with
  `codesign --verify --deep --strict`, then notarize the distribution artifact.
  `sqz sfx publish-macos` automates this sequence for a separate output app
  using the publisher's existing Developer ID identity and Keychain profile.
- Treat an unsigned artifact as untrusted in CLI and GUI messaging.
- Test the actual output on the target operating system; a synthetic stub test
  proves container layout, not executable-loader behavior. Release smoke must
  install the generated NSIS package or extract the generated AppImage and
  compare its `bin/sqz-sfx.stub` with the build template. It must then use the
  dedicated template while retaining the full CLI version, split create, list,
  test, extract and payload-create checks. It must also verify the dedicated
  runtime identity, require it to be strictly smaller than the full `sqz` CLI,
  and exercise an unsigned legacy `sqz` fallback where that fallback is valid.
- Benchmark guarded replacement with cold caches and large macOS bundles. The
  full-tree digest is intentionally stronger than a metadata-only check, but
  its release-scale read amplification has not yet been qualified.
- Exercise transaction replay and directory durability on real Windows
  NTFS/ReFS/SMB targets and representative Linux filesystems before claiming
  those platform paths are release-qualified.
- Keep ZIP Slip, symlink breakout, decompression-bomb and disk-space failures
  fatal.
- Do not add an auto-run option.
