# ZIP raw-copy preservation

Source: [crates.io zip 8.6.0](https://crates.io/crates/zip/8.6.0),
upstream [zip-rs/zip2](https://github.com/zip-rs/zip2), MIT license (see `LICENSE`).
Published crate SHA-256:
`2d04a6b5381502aa6087c94c669499eb1602eb9c5e8198e534de571f7154809b`.

The upstream manifest and source are retained. Production changes are confined to
`src/write.rs` and `src/types.rs`; the public API and feature/dependency contract
are unchanged.

- Internal entry creation takes name bytes. Raw copies and timestamp-only edits
  use the source bytes; explicit renames and newly created entries use UTF-8.
- Raw copies preserve the encryption state, data-descriptor requirement,
  originating platform, external attributes and version metadata. Descriptors
  are emitted after the unchanged compressed payload, including ZipCrypto's
  timestamp-based password-check case.
- The non-UTF-8 writer unit fixture uses byte names without creating invalid
  Rust strings.
- Raw copies and renames retain NTFS timestamps and Info-ZIP UTC modification
  times in the generated headers. Timestamp-only edits continue to use their
  explicitly supplied DOS time, without carrying stale UTC overrides.

Squallz exercises this behavior through `crates/squallz-formats/tests/zip_update.rs`,
including local/central name agreement, byte-for-byte compressed payloads,
legacy names, encryption, file types, cancellation and transaction behavior.
`crates/squallz-formats/tests/zip_timestamps.rs` covers UTC metadata preservation,
subsecond precision, out-of-DOS-range dates and Info-ZIP interoperability.
The root workspace and the independent fuzz crate both select this patch.

Replace this copy with an upstream stable release once equivalent preservation
is covered by the same integration tests, with ZIP64 and interoperability checks.
