# Squallz Fuzz Targets

This crate is intentionally excluded from the main workspace. Run the smoke gate
from the repository root:

```bash
scripts/zip_fuzz_smoke.sh
```

When `cargo-fuzz` is installed, the gate runs a short libFuzzer smoke for the
ZIP reader target. Without `cargo-fuzz`, it still compiles the fuzz target with
`cargo check` so CI and local agents can catch API drift.

Useful environment variables:

- `SQUALLZ_FUZZ_RUNS`: libFuzzer run count, default `64`.
- `SQUALLZ_FUZZ_MAX_LEN`: generated input cap, default `4096` bytes.
- `SQUALLZ_FUZZ_REPORT`: Markdown report path, default `benches/ZIP_FUZZ_CAMPAIGN.md`.
- `SQUALLZ_FUZZ_REQUIRE_CARGO_FUZZ=1`: fail instead of using the cargo-check fallback.

The gate writes its raw log to `target/squallz-zip-fuzz/zip_reader.log` and
updates the campaign report with toolchain, corpus, coverage, RSS, and artifact
counts.

## Nightly CI Campaign

The reviewable CI continuity contract lives in `.github/workflows/zip-fuzz.yml`.
It runs on a nightly schedule and can also be started manually with
`workflow_dispatch` inputs for `runs` and `max_len`.

The CI workflow sets `SQUALLZ_FUZZ_REQUIRE_CARGO_FUZZ=1`, installs nightly Rust
and `cargo-fuzz`, then calls the same repository entrypoint:

```bash
scripts/zip_fuzz_smoke.sh
```

Default CI bounds are intentionally stronger than the local smoke:

- `SQUALLZ_FUZZ_RUNS=1048576`
- `SQUALLZ_FUZZ_MAX_LEN=32768`
- `SQUALLZ_FUZZ_REPORT=benches/ZIP_FUZZ_CAMPAIGN.md`

The workflow uploads `benches/ZIP_FUZZ_CAMPAIGN.md`,
`target/squallz-zip-fuzz/zip_reader.log`, and any
`fuzz/artifacts/zip_reader` crash artifacts. It does not upload or commit the
random generated corpus.

The workflow file itself is the source for scheduled/manual CI configuration.
Do not treat a local contract scan as fuzz evidence. Release readiness requires
at least one completed uploaded CI fuzz report/log/artifact set to be reviewed
before the CI artifact blocker can close.

Generated corpus files are ignored by default. Promote only minimized crashes
or deliberate regression seeds into `tests/fixtures/` or force-add a reviewed
seed with a short explanation.
