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
| TAR | Rust `tar` crate | Implemented |
| 7z | Rust `sevenz-rust2` | Implemented |
| XZ / BZIP2 / GZIP | Rust-facing compressor crates | Implemented |
| `.sqz` | Squallz native container + Reed-Solomon | Implemented core capability; entry-set plus zip/tar/7z/zstd inner profiles covered |
| PAR2 verify/repair | Rust fallback plus external PAR2 bridge | Implemented |
| PAR2 create | External standard PAR2 bridge | Implemented when the tool exists; packaging and license evidence remain required before bundling |
| Long-tail unpack-only | 7zz/7z bridge | Registry/CLI path plus generated real seed matrix pass on current macOS host; broader third-party corpus and target-platform package evidence remain |
| WIM create | External wimlib-imagex bridge | Real local wimlib/7zz create/list/test/extract smoke pass on current macOS host; target-platform package/license and broader corpus remain |
| RAR read | 7zz/7z bridge, bsdtar diagnostic fallback for RAR5 v6, explicit override | Read-only public-sample path implemented; encrypted/multi-volume/damaged repair claims remain outside current support |

The 7zz bridge lists entries with `7z l -slt` and streams one entry at a time
with `7z x -so`. The bridge output still flows through Squallz shared safe
extraction, so Zip Slip, symlink breakout, name sanitization, overwrite, and
resource limits remain centralized.

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
- `sqz pack --inner-format` supports `sqz` / `entry-set`, `zip`, `tar`, `7z`,
  and `zstd`. The zstd profile stores a protected `tar.zst` payload so
  `list`, `test`, `extract`, and `export` still expose normal multi-file
  archive entries; raw remains a deferred profile because it has no directory
  or multi-file archive semantics.
- `sqz list/test/extract` can use `SQUALLZ_7Z` or PATH candidates
  `7zz`, `7z`, `7za` for bridge-backed archives.
- The 7zz `-slt` parser skips archive metadata blocks such as WIM's top-level
  `Path`/`Type`/`Physical Size` section, so the archive's own absolute temp
  path cannot leak into the entry list or trigger extraction path traversal.
- Typed 7zz entries such as XAR `Type = file` / `Type = directory` remain real
  entries; only `Type` combined with archive-level `Physical Size` is treated
  as a metadata block.
- The 7zz `-slt` parser also skips root pseudo-entries reported as `.` or
  `./`, which real CPIO fixtures expose before actual file members.
- The bridge now infers directory entries when real disk-image listings expose
  a path prefix as a zero-byte file before child paths, which real DMG/HFS+
  seed fixtures do for directory rows.
- `rar`/`cbr` now use the same `SQUALLZ_7Z` / `7zz` / `7z` / `7za`
  priority path for listing and per-entry streaming. `SQUALLZ_BSDTAR`
  remains an explicit diagnostic fallback, not the primary cross-platform
  product route.
- `sqz info --json` exposes RAR-specific machine-readable limitations under
  `implementation.limitations`: no RAR creation, no RAR recovery records or
  `.rev`, encrypted RAR and multi-volume RAR are not release-claimed without a
  licensed/full fixture matrix, and damaged RAR repair is unsupported.
- `sqz info --json` also exposes `implementation.policy` for RAR so GUI,
  scripts, and release checks can distinguish the actual product boundary from
  runtime availability: RAR is read-only, not bundled, primary read uses
  `SQUALLZ_7Z` / `7zz` / `7z` / `7za`, and `SQUALLZ_BSDTAR` / `bsdtar` is a
  diagnostic or RAR5-v6 fallback rather than a cross-platform bundled promise.
- `sqz compress <inputs...> -o image.wim` can use `SQUALLZ_WIMLIB` or
  `wimlib-imagex` from PATH. The writer stages entries in a temporary
  directory, calls `wimlib-imagex capture`, then copies the WIM image into the
  normal Squallz destination writer.
- Create output now goes through a same-directory temporary file before
  replacing the target, so a missing WIM writer or failed create does not leave
  an empty archive at the requested destination.

Open product boundaries:

- WIM packaging/license review and broader third-party WIM corpus coverage
  across target platforms. The current macOS host has a real 7zz/wimlib smoke
  proving Squallz-created WIMs and independently-created wimlib WIMs through
  the public CLI.
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
  The current seed report has no explicit deferrals. VHD/QCOW2/VMDK/VDI/VHDX/UEFI/NTFS/MSI/NSIS/CHM rows are now also
  checked against the 7zz/7z `-slt` top-level backend type (`VHD`, `QCOW`,
  `VMDK`, `VDI`, `VHDX`, `UEFIc`, `UEFIf`, `NTFS`, `Compound`, `Nsis`, or `Chm`) rather than
  passing because 7zz scanned an embedded FAT/MBR payload, unrelated bytes, or
  a non-matching file with an installer extension.
  Broader per-format third-party corpus coverage remains separate product
  evidence and should not be implied by the generated seed matrix alone.
- Real RAR sample matrix for plain, encrypted, solid, multi-volume, and
  damaged archives through the chosen packageable backend. Current public
  samples cover RAR4/RAR5 stored, multiple-file, solid, CBR alias, and
  damaged-header rejection. Encrypted RAR and multi-volume RAR remain
  `not_release_claimed` in `sqz info --json`; damaged RAR repair remains
  unsupported.
- Password-protected long-tail extraction through the 7zz bridge.
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
  sample subset, with encrypted/multi-volume marked `not_release_claimed` and
  creation/recovery-record/damaged repair marked `unsupported` in
  `sqz info --json`.

## External References

- 7-Zip homepage supported formats:
  https://www.7-zip.org/
- 7-Zip license:
  https://www.7-zip.org/license.txt
- wimlib repository and cross-platform WIM scope:
  https://github.com/ebiggers/wimlib
