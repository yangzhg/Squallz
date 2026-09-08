# Squallz Format Support Contract

This document records the supported format boundary for core, CLI, and GUI.

## Product Target

Pack and unpack:

- 7z
- XZ
- BZIP2
- GZIP
- TAR
- ZIP
- WIM
- Squallz native `.sqz`

Unpack only:

- APFS
- AR
- ARJ
- CAB
- CHM
- CPIO
- CramFS
- DMG
- EXT
- FAT
- GPT
- HFS
- IHEX
- ISO
- LZH
- LZMA
- MBR
- MSI
- NSIS
- NTFS
- QCOW2
- RAR
- RPM
- SquashFS
- UDF
- UEFI
- VDI
- VHD
- VHDX
- VMDK
- XAR
- Z

Recovery and redundancy formats:

- `.par2` and `.vol*.par2` sidecars for standard archives.
- `.sqz` embedded recovery containers.
- `.sqz.001/.002/...` split volumes with Squallz `SQZV` headers.
- `.sqz.rev001/.rev002/.rev003` parity sidecars for Squallz split-volume recovery.

`.sqz.revNNN` is Squallz-owned parity metadata. It is not RAR `.rev`, and the
project must never claim RAR recovery-record compatibility.

Numbered volume boundary:

- `.001/.002/...` output for ZIP, 7z, TAR.ZST, and WIM uses the same
  contiguous byte-split layout as 7-Zip `-v`; it is not native ZIP
  `.z01/.zip` or Microsoft Split WIM. This remains the default layout.
- ZIP creation and conversion can explicitly select the built-in native
  PKWARE layout, which writes `.z01/.z02/…/.zip` and reports the final `.zip`
  as the primary member. Local headers and central-directory records do not
  cross physical volume boundaries; ZIP64 end records describe the complete
  disk set. Native volume sizes are 64 KiB through 4 GiB minus one byte.
- Existing native ZIP `.z01/.z02/…/.zip` sets use a separate read path.
  Squallz validates the final disk's EOCD or ZIP64 disk metadata, requires the
  complete numbered family, reports the first exact missing source name, and
  can start from the final `.zip` file or any present `.zNN` member. The set
  is copied with stable-identity and no-follow checks into a private
  0700/0600 staging area before 7zz/7z reads the normalized `.zip` primary.
  Ordinary single-file ZIP remains on the built-in Rust reader. Encrypted
  native split ZIP can also be created by the built-in writer, then listed,
  tested, and extracted through the same external reader; Squallz sends the
  read password through a short-lived stdin pipe, never through process
  arguments or environment variables.
- Native Split WIM `.swm` sets use a separate format-native path. Squallz can
  start from any standard member (`image.swm`, `image2.swm`, …), verifies the
  WIM GUID, part number, total-part count, complete family, stable file
  identities, and exact standard names, then copies the set through no-follow
  checks into a private 0700/0600 staging area before 7zz/7z reads the
  normalized first member. A known missing part is reported by its exact
  source path.
- Creation and conversion can select the native layout for a `.swm`
  destination. Squallz captures the authoritative WIM through the existing
  writer, copies it into a private workspace, invokes `wimlib-imagex split`,
  validates the complete generated GUID/part family, and publishes
  `image.swm`, `image2.swm`, … as one protected output transaction. The first
  `.swm` is the primary member. Cancellation or failure leaves the previous
  family intact and publishes no partial replacement. The requested part size
  is a target, not a hard maximum: one indivisible file resource can make a
  part larger.
- An output named `.swm` without both a split size and the native layout is
  rejected before source scanning or output reservation. Standalone WIM
  output continues to use `.wim`; generic splitting uses `.wim.001`,
  `.wim.002`, ….
- Squallz can open the set from any numbered member that is present, reports
  the first known gap, and reports an explicitly selected missing tail member.
  Discovery memory is bounded by the members found in the directory rather
  than the largest numeric suffix.
- Split creation is capped at 1,000,000 data volumes, and SQZV reading uses
  the same limit. A larger declared `total` is rejected as a resource limit
  before range-sized allocation or iteration.
- A remaining `.001` file in a headerless byte-split set cannot prove whether
  an unknown tail member once existed. Squallz does not invent that error.
- A real 7zz 26.01 test on the current macOS host covers both directions:
  Squallz-created 7z volumes tested by 7zz, and 7zz-created volumes opened and
  extracted by Squallz from a non-first member. The test skips when 7zz/7z is
  unavailable, so target-platform packaging still needs a required smoke job.

Executable archive boundary:

- ZIP self-extracting archives can be detected and read when a platform
  executable prefix appears before the ZIP data.
- SFX v1 can assemble a complete ZIP payload with a Squallz-aware Windows PE
  or Linux ELF stub. The runtime verifies the full payload CRC before list,
  test or extract and continues through the shared extraction engine.
- macOS uses a GUI `.app` template with the payload and SHA-256 manifest in
  `Contents/Resources`. Arbitrary data is not appended to Mach-O executables;
  the app launches the shared task window and extraction engine.
- Generated Windows/Linux artifacts require target-platform testing and final
  signing. Squallz does not claim that unsigned output will pass SmartScreen,
  mail or upload filters.
- Self-extractors never run archived programs or scripts automatically. See
  `docs/SELF_EXTRACTING.md` for the versioned layout and trust boundary.

## Cross-Platform Route

Squallz must not rely on macOS `/usr/bin/bsdtar`, Linux distribution tools, or a
developer machine PATH as product capabilities. External backends are acceptable
only when they are cross-platform, packageable or clearly user-installable, and
reported through `DependencyMissing` when absent.

If a format is hard to support safely in Rust, has unclear redistribution
terms, or is realistically platform-specific, Squallz may defer it, expose it
only on supported platforms, or require an external tool. In that case the
capability table, user documentation, and `sqz info --json` must make the boundary
visible instead of presenting the format as universally available.

Current route:

| Area | Route | Status |
| ---- | ---- | ---- |
| ZIP / ZIP64 | Rust `zip` crate | Implemented |
| Native ZIP `.z01/…/.zip` volumes | Built-in PKWARE/ZIP64 writer; validated private staging plus external 7zz/7z for reading | Create and convert can publish `.z01/.z02/…/.zip` with the final `.zip` as primary. Existing sets can open from any member with exact known-gap reporting, source-set listing, testing, encrypted-entry password handling, and shared safe extraction. Earlier-volume/cross-volume ZIP64 generator samples and the three-platform packaged-tool matrix remain outside the current claim |
| TAR | Rust `tar` crate | Implemented |
| 7z | Rust `sevenz-rust2` | Implemented |
| XZ / BZIP2 / GZIP | Rust-facing compressor crates | Implemented |
| `.sqz` | Squallz native container + Reed-Solomon | Implemented core capability; native SQZ plus zip/tar/7z/zstd inner profiles covered |
| PAR2 verify/repair | Rust fallback plus external PAR2 bridge | Implemented for single-file, split-volume, and multi-file sets. Safe-copy repair publishes one member as a new no-replace file or the exact described set as a new no-replace directory; source files stay unchanged, unsafe member paths are rejected, and external success is checksum-verified before publication |
| PAR2 create | External standard PAR2 bridge plus core publication transaction | Implemented when the tool exists. The backend creates beside the destination in a private directory with an explicit source base; Squallz parses and verifies the complete index/volume set, then publishes every bound file no-replace through a crash-resumable journal and reports the physical outputs. Packaging and license evidence remain required before bundling |
| Long-tail unpack-only | 7zz/7z bridge | Registry/CLI path plus generated real seed matrix pass on current macOS host; broader third-party corpus and target-platform package evidence remain |
| WIM create, standalone WIM/ESD read, and native Split WIM create/read | External wimlib-imagex and 7zz/7z bridges | Real local wimlib/7zz create/split/list/test/extract passes on the current macOS host. Native creation publishes a validated standard `.swm`, `2.swm`, … family transactionally with the first member primary; cancellation does not expose partial members. A complete existing family opens from any member after exact-name, GUID, part-count, stable-identity, and completeness validation, and missing parts are named precisely. Target-platform package/license and broader third-party corpus remain |
| RAR read | External 7zz/7z bridge; bsdtar for explicit use or a validated single-file decoder gap; optional user-installed unrar for confirmed-unencrypted RAR7 v6 streams | Squallz does not include a RAR decoder. The read-only path supports encrypted input through the stdin-only 7zz/7z password bridge. On Linux, a public compressed RAR5 sample that p7zip 16.02 can list but cannot decode passes list/test/extract through bsdtar only after both tools report exactly matching regular-file paths and sizes. RAR-format `partN.rar` and legacy `.rar/.r00`–`.r99` sets remain on isolated first-volume 7zz/7z staging and can open from any member. On macOS, real RAR5 and old-style RAR4 volume sets including header encryption pass list/extract, password, and missing-volume checks; a real two-volume RAR7 v6 set also passes list/test/extract from its second member with byte-identical output when unrar is configured. Broader historical RAR4, encrypted/solid RAR7 coverage, and the full three-platform package matrix are not release-claimed. Damaged repair is unsupported |

The 7zz bridge lists entries with `7z l -slt` and streams one entry at a time
with `7z x -so`. The bridge output still flows through Squallz shared safe
extraction, so Zip Slip, symlink breakout, name sanitization, overwrite, and
resource limits remain centralized. Passwords use a piped stdin prompt and are
never appended to the tool command line or exported through its environment.

## Current Code Status

Implemented code paths:

- `crates/squallz-formats/src/sevenzip_bridge.rs` registers these read-only
  archive formats through the shared registry:
  `wim`, `apfs`, `ar`, `arj`, `cab`, `chm`, `cpio`, `cramfs`, `dmg`, `ext`,
  `fat`, `gpt`, `hfs`, `ihex`, `iso`, `lzh`, `lzma`, `mbr`, `msi`, `nsis`,
  `ntfs`, `qcow2`, `rpm`, `squashfs`, `udf`, `uefi`, `vdi`, `vhd`, `vhdx`,
  `vmdk`, `xar`, and `z`.
- `sqz info --json` exposes those formats as `kind=archive`,
  `can_extract=true`, `can_test=true`. WIM also exposes `can_create=true`
  because `sqz compress -o image.wim` can use an external `wimlib-imagex`
  writer when available.
- `sqz info --json` also exposes `implementation.status`,
  `implementation.bundled`, external tool candidates, environment overrides,
  release checks, and `implementation.availability` for the current machine.
  Availability is diagnostic: it reports whether the selected env/PATH tool
  exists now, but it does not replace real fixture-matrix compatibility tests.
  Formats whose implementation status is `external_required` are an external
  dependency in user documentation and GUI format capability surfaces; they must
  not be described as fully bundled unless packaging and license evidence is added.
  The plain classic `sqz info` table includes an ASCII `Capabilities` column
  and a `Backend` column so users can scan built-in versus external backends
  in logs. Modern `sqz info` adds a wrapped `Format coverage` table, a
  `Runtime inventory`, boxed grouped capability matrix, and separate
  `Read` / `Write` columns for the current machine while leaving JSON
  unchanged.
- `sqz doctor --json` exposes a compact runtime readiness report for built-in
  formats, the 7zz/7z read bridge, WIM writer, SQZ embedded recovery, PAR2
  create, PAR2 verify/repair fallback, and the RAR product boundary.
  `sqz doctor --strict` exits 8 when a runtime dependency required by a
  product-claimed capability is missing; explicit non-goals such as RAR
  creation remain a boundary rather than a strict failure.
- `sqz pack --inner-format` supports `sqz`, `zip`, `tar`, `7z`,
  and `zstd`. The zstd profile stores a protected `tar.zst` payload so
  `list`, `test`, `extract`, and `export` still expose normal multi-file
  archive entries.
- `sqz list/test/extract` can use `SQUALLZ_7Z` or PATH candidates
  `7zz`, `7z`, `7za` for bridge-backed archives.
- Standalone WIM/ESD files use that bridge. A path-backed WIM header marked as
  spanned, or carrying a part/total count greater than one, enters the native
  Split WIM reader. Squallz derives the standard family from the selected
  part, validates every member, exposes the ordered source set, and supports
  list, test, and shared safe extraction from any member. Stream-only callers
  without a source path still fail closed because sibling discovery is
  impossible.
- The 7zz `-slt` parser skips archive metadata blocks such as WIM's top-level
  `Path`/`Type`/`Physical Size` section, so the archive's own absolute temp
  path cannot leak into the entry list or trigger extraction path traversal.
- Typed 7zz entries such as XAR `Type = file` / `Type = directory` remain real
  entries; only `Type` combined with archive-level `Physical Size` is treated
  as a metadata block.
- The 7zz `-slt` parser also skips root pseudo-entries reported as `.` or
  `./`, which real CPIO fixtures expose before actual file members.
- The bridge infers directory entries when real disk-image listings expose
  a path prefix as a zero-byte file before child paths, which real DMG/HFS+
  seed fixtures do for directory rows.
- `rar`/`cbr` use the same `SQUALLZ_7Z` / `7zz` / `7z` / `7za`
  priority path for listing and ordinary per-entry streaming.
  `SQUALLZ_BSDTAR` remains an explicit diagnostic or validated single-file compatibility fallback.
  If 7zz/7z positively reports at least one v6 entry and explicitly marks
  every readable entry block as unencrypted,
  `SQUALLZ_UNRAR` or `unrar` on PATH can stream the entry after the same
  listing and volume checks. Unknown encryption state, any encrypted item, or
  any supplied password stays on the stdin-only 7zz/7z path.
- Native ZIP data volumes use the ZIP registry path rather than the generic
  `.001` byte collector. Name matching only selects the candidate format; the
  final `.zip` EOCD/ZIP64 disk fields must prove a multi-disk archive before
  siblings are grouped. Physical members remain ordered `.z01` through
  `.zip`, while `.zip` is the preferred primary when a complete family is
  selected for batch extraction. Missing and extra members fail before 7zz is
  launched. The selected member stays bound to the engine-opened stream;
  siblings reuse the same no-follow, stable-identity and private-staging
  boundary as RAR-format volumes.
- A physical RAR file also retains a path hint inside the format layer.
  RAR5 `name.part1.rar` / fixed-width `name.part001.rar` and RAR4
  `name.rar` / `name.r00`…`name.r99` sets are discovered without changing
  the generic `.001` byte-split contract. Public RAR headers confirm the
  volume flag, the RAR5 volume index, and the next-volume state when that
  evidence is present. For RAR5 header encryption, every candidate must instead
  carry a valid public archive-encryption header. For RAR4 header encryption,
  the CRC-checked public main header must identify a volume, encryption, and
  whether the candidate is the first volume; parsing then stops before encrypted
  headers. After private staging and password submission, 7zz must report
  `Multivolume`, first-volume index 0, and an exact `Volumes` count matching the
  stable candidate set. For confirmed-unencrypted RAR7 v6 sets, the same
  staged first volume can then be passed to optional unrar for entry streaming;
  unrar is launched with configuration and list-file processing disabled,
  password prompting disabled, null stdin, and a switch terminator before the
  entry name. Entry names containing wildcard characters are rejected on this
  fallback instead of risking a mask match. Known gaps
  report a source-style expected name once the numbering shape is unambiguous.
  Members are streamed into an exclusively created
  directory, normalized to the canonical first-volume name, and only that
  staged path is given to 7zz/7z. On macOS/Linux the directory and member
  modes are 0700/0600. Windows rejects reparse points and verifies the volume
  serial number, file ID, creation time, and last-write time, but its inherited
  temporary-directory ACL still needs a real NTFS gate. The selected member
  comes from the already-open stream, sibling files are opened without
  following links, and every path binding is rechecked around copying. Reader
  disposal attempts to remove staging. A versioned registry, external owner
  lock, and in-directory marker let the next process reclaim only a released,
  exactly matching workspace after interruption; active or suspicious
  workspaces remain untouched. Backend errors redact the staging path.
- `sqz info --json` exposes RAR-specific machine-readable limitations under
  `implementation.limitations`: no RAR creation, no RAR recovery records or
  `.rev`; encrypted reading and native multi-volume orchestration are
  implemented. Real macOS RAR5 and old-style RAR4 volume smokes, including
  header encryption, are covered. Broader historical RAR4 and encrypted/solid
  fixtures, plus the three-platform package matrix, remain outside the release
  claim; damaged RAR repair is unsupported.
- `sqz info --json` keeps ordinary and native-split ZIP creation marked
  built-in while separately exposing native split ZIP reading as an optional
  7zz/7z dependency. Encrypted split ZIP reading uses the same external
  dependency with stdin-only credential transfer.
- `sqz info --json` also exposes `implementation.policy` for RAR so GUI,
  scripts, and release checks can distinguish the actual product boundary from
  runtime availability: RAR is read-only, not bundled, primary read uses
  `SQUALLZ_7Z` / `7zz` / `7z` / `7za`; `SQUALLZ_BSDTAR` / `bsdtar` is an
  explicit or validated single-file compatibility fallback, while
  `SQUALLZ_UNRAR` / `unrar` is an optional decoder for confirmed-unencrypted
  RAR7 v6 entry streams. Neither fallback is a bundled cross-platform promise;
  `fallback_scopes` is the exhaustive machine-readable policy.
- `sqz compress <inputs...> -o image.wim` can use `SQUALLZ_WIMLIB` or
  `wimlib-imagex` from PATH. The writer stages entries in a temporary
  directory, calls `wimlib-imagex capture`, then copies the WIM image into the
  normal Squallz destination writer.
- Create output goes through a same-directory temporary file before
  replacing the target, so a missing WIM writer or failed create does not leave
  an empty archive at the requested destination.

Open product boundaries:

- Real Developer ID/Apple-service acceptance testing, smaller host runtimes,
  signed target stubs, and release tests on real Windows/Linux targets. SFX v1
  core, CLI and desktop creation support PE/ELF single files and macOS app
  bundles; CLI and desktop share the macOS signing/notarization publisher.

- WIM packaging/license review and broader third-party WIM corpus coverage
  across target platforms. The current macOS host has real 7zz/wimlib coverage
  for Squallz-created standalone WIMs, native Split WIM creation, arbitrary
  member entry, byte-identical extraction, exact missing-part reporting, and
  cancelled-create cleanup. This does not replace Windows/Linux packaged-tool
  verification.
- Curated generated long-tail seed coverage is current pass evidence, not a
  claim of broad third-party corpus compatibility. The current generated macOS
  seed covers `apfs`, `ar`, `arj`, `cab`, `chm`, `cpio`, `cramfs`,
  `dmg`, `ext`, `fat`, `gpt`, `hfs`, `ihex`, `iso`, `lzh`, `lzma`, `mbr`,
  `msi`, `nsis`, `ntfs`, `qcow2`, `rpm`, `squashfs`, `udf`, `uefi`, `vdi`, `vhd`, `vhdx`, `vmdk`, `wim`, `xar`, and `z` through the same public
  `sqz list/test/extract` path. The RPM seed is a minimal RPM v3 package whose
  gzip-compressed cpio payload is exposed by 7zz as
  `squallz-rpm-fixture-1.0-1.noarch.cpio`; it does not claim automatic
  same-layer expansion of files inside that cpio. The SquashFS seed is a
  minimal SquashFS 4.0 image with uncompressed metadata/data and one file
  member, shaped to satisfy the real 7zz SquashFS handler's table-order
  checks. The VDI seed is a dynamic VirtualBox VDI wrapper around the FAT32
  seed image with 1 MiB blocks. The VHDX seed is a fixed VHDX wrapper around
  the FAT32 seed image with CRC32C-checked headers, region tables, BAT, and
  metadata. The UEFI seed is a minimal UEFIf firmware volume with `_FVH`, FFS2
  GUID, valid FV/FFS checksums, and one raw section. The NTFS seed is a minimal
  NTFS image with a boot sector, non-resident MFT stream, root directory record,
  and resident `hello.txt` data. The MSI seed is a minimal MSI/Compound storage
  fixture with a CFBF header, FAT, directory stream, and one normal `hello.txt`
  stream; it covers the 7zz Compound storage unpack path for `.msi`, not
  Windows Installer execution semantics. The NSIS seed is a minimal non-solid
  installer payload with one stored `hello.txt` file; it covers the 7zz NSIS
  unpack path, not installer execution semantics. The CHM seed is a minimal
  high-level ITSF/ITSP/PMGL fixture with NameList and one stored `hello.txt`
  file; it covers the 7zz CHM unpack path, not broad CHM corpus behavior.
  The current seed report has no explicit deferrals. VHD/QCOW2/VMDK/VDI/VHDX/UEFI/NTFS/MSI/NSIS/CHM rows are also
  checked against the 7zz/7z `-slt` top-level backend type (`VHD`, `QCOW`,
  `VMDK`, `VDI`, `VHDX`, `UEFIc`, `UEFIf`, `NTFS`, `Compound`, `Nsis`, or `Chm`) rather than
  passing because 7zz scanned an embedded FAT/MBR payload, unrelated bytes, or
  a non-matching file with an installer extension.
  Broader per-format third-party corpus coverage remains separate product
  evidence and should not be implied by the generated seed matrix alone.
- Real RAR sample matrix for plain, encrypted, solid, multi-volume, and
  damaged archives through the chosen packageable backend. Current public
  samples cover RAR4/RAR5 stored, multiple-file, solid, CBR alias, and
  damaged-header rejection; local macOS RAR5 multi-volume sets also pass
  list from the first and a later member, exact missing-middle reporting, and
  extraction through 7zz. A RARLAB 7.23-generated, header-encrypted stored
  four-volume set additionally passes open from its third member, no-password
  and wrong-password typing, byte-identical extraction, exact missing-middle
  and missing-tail diagnostics, and external volume-count verification.
  RARLAB 6.24-generated old-style RAR4 `.rar/.r00/.r01/.r02` stored sets,
  both plain and main-header encrypted, pass open from later members,
  password typing, byte-identical extraction, exact missing-member diagnostics,
  rejection of an extra same-family member, first-volume-flag validation, and
  external volume-count verification. Native multi-volume orchestration is
  implemented. A RARLAB 7.23 default-compression RAR7 v6 two-volume set
  also passes list, test, and extraction from its second member through
  7zz/7z plus configured unrar; the 5,896-byte output matches its source
  SHA-256, and removing the second volume reports the exact missing name.
  Without unrar, the same host's 7zz 26.01 still fails the entry stream rather
  than being misreported as supported. Header-encrypted and data-encrypted
  inputs remain on the 7zz/7z password path. Broader historical RAR4,
  encrypted/solid fixtures, and macOS/Windows/Linux package evidence remain
  outside the release claim.
  Damaged RAR repair remains unsupported.
- Broader RAR4 coverage must still exercise pre-3.0 archives, historical CRC
  exceptions, compressed/solid archives, and large service records. Selecting
  one member and dragging an entire confirmed native volume family both
  collapse to one logical extraction today.
- Native ZIP needs real ZIP64 multi-disk samples whose central directory and
  ZIP64 end records cross volume boundaries and required macOS/Windows/Linux
  packaged-7z smoke. The current Info-ZIP interoperability tests cover stored
  multi-volume creation, opening from a middle member, list/test/extract, byte
  equality, exact missing-middle reporting, plus AES-encrypted split
  list/test/extract with missing, wrong, and correct passwords on the macOS
  host.
- Broader password-protected long-tail samples through the stdin-only 7zz
  bridge.
- License and redistribution decision for bundling wimlib/7zz. Until that is
  closed, WIM create is an external-tool capability, not a bundled guarantee.

## Before Bundling External Tools

- Real 7zz availability checks for macOS, Windows, and Linux packaging.
- Broader third-party sample matrices for target unpack-only formats where
  Squallz wants to claim more than generated seed compatibility.
- Broader WIM corpus coverage and target-platform packaging/license evidence.
  The current macOS host has a real 7zz/wimlib create/list/test/extract smoke.
- A strict/full RAR read matrix for plain, encrypted, solid, multi-volume, and
  damaged samples only if Squallz wants to claim WinRAR-level/full RAR
  compatibility. The current support scope is the reduced read-only public
  sample subset plus implemented encrypted reading and native volume
  orchestration. Header-encrypted RAR5 and old-style RAR4 volumes have real
  macOS smokes, and confirmed-unencrypted RAR7 v6 has a real optional-unrar
  two-volume smoke; broader historical RAR4, encrypted/solid, and full
  multi-volume corpus coverage remains `not_release_claimed`, while
  creation/recovery-record/damaged repair is marked `unsupported` in
  `sqz info --json`.

## External References

- 7-Zip homepage supported formats:
  https://www.7-zip.org/
- 7-Zip license:
  https://www.7-zip.org/license.txt
- RARLAB RAR5 format and volume-header specification:
  https://www.rarlab.com/technote.htm
- RARLAB current multi-volume naming:
  https://www.rarlab.com/rar_file.htm
- RARLAB UnRAR 7 implementation notes:
  https://www.rarlab.com/unrar7notes.htm
- RARLAB UnRAR downloads/source:
  https://www.rarlab.com/rar_add.htm
- RARLAB license:
  https://www.rarlab.com/license.htm
- wimlib repository and cross-platform WIM scope:
  https://github.com/ebiggers/wimlib
