# Code ownership and composition

Squallz has two desktop layouts and a CLI. Archive operations belong to the shared Rust engine; layouts and entry points compose those capabilities.

## Desktop frontend

| Responsibility | Owner | Consumers |
| --- | --- | --- |
| Create, extract, recovery and local-tool workflows | The corresponding session in `frontend/src/lib` | Classic and Modern workspaces |
| Destination format, compression, encryption and split-volume draft | `output-options-draft.svelte.ts` | Create options and conversion session |
| Protection controls and shared option labels | `output-options-presentation.ts` | Create and conversion presentations |
| Output preparation, review, submission and cancellation lifecycle | `archive-output-workflow.svelte.ts` | Create preflight and conversion session |
| Workspace, system-temp and output disk budgets | `archive-disk-preflight.ts` | Create and conversion preflight |
| Output conflict validation, confirmation and renewed authorization | `archive-output-authorization.ts` | Create, conversion and recovery export |
| Archive rows, selection and browse commands | `archiveBrowserSurface()` in `App.svelte`, with contracts in `archive-browser.ts` | Both archive browsers |
| Prepared-entry state and lifecycle | `preview-session.svelte.ts` | Browser actions and inspector |
| Preview content and actions | `ArchiveEntryPreview.svelte` | Classic sidebar and Modern inspector |
| Translation lookup, fallback and interpolation | `i18n.svelte.ts` and `locales/*.json` | All frontend features |
| Job registration and authoritative snapshot reduction | `jobs.svelte.ts`, `job-snapshot.ts` | Every workflow and task window |
| Task result validation and discriminated result types | `job-result.ts` | Registration, live events and recovery before store mutation |
| Result status, summary and output availability | `task-outcome.ts` | Completion notifications, history and task dialogs |

Each workflow has its own draft instance. Sharing the output-option implementation does not share credentials or editable state between creating and converting. Create retains SFX constraints; conversion retains its source archive, source credentials and preflight lifecycle. Both compose the same destination-option rules.

Each create or conversion session owns an independent `ArchiveOutputWorkflow` instance. That workflow owns request IDs, cancellation, pending plans, review and submission state. Sessions supply source selection, archive identity and credential policy. Every asynchronous check or confirmation is guarded by the current request owner. Overwrite authorization is bound to the inspected destination guard; a changed guard requires a new confirmation and a vanished destination clears authorization. Create retains its automatic-name policy, conversion rejects its source as an output, and recovery repair continues to prohibit replacement.

Task notifications, history and dialogs interpret results through one presentation model. History retains its compact, path-redacted text and notifications retain reveal actions and actionable warnings. Output availability is derived from the completed result even when a window recovers a task without owning its completion effects.

Classic and Modern retain different layout, table columns and row heights. Their browser models derive rows and commands from the same state. Preview loading, failure, file and nested-archive actions have one implementation; compact and detailed nested-entry presentation remain explicit layout choices.

The application entry still owns navigation, desktop coordination and significant archive browsing logic. It is not a fully minimal composition root. Moving a function out of this entry alone is not evidence of reduced complexity.

## Rust engine and entry points

`squallz-core` orchestrates archive operations through `squallz-format-api`; `squallz-formats` supplies format implementations and `squallz-recovery` supplies recovery capabilities. GUI and CLI entry points retain input acquisition, progress delivery, cancellation, platform integration and presentation.

`squallz_core::reports` supplies typed reports for creation, integrity tests, checksum verification and duplicates. Extraction plans, extraction counts and checksum reports serialize from their domain types, including display-safe paths. Standalone CLI commands and batch scripts retain their existing JSON envelopes in the CLI report adapter. Report conversion does not perform I/O or move cancellation checks out of execution paths.

GUI execution returns one `JobResult` variant for each job kind. Completed snapshots require a result; creation, partial extraction, recovery and source cleanup retain their distinct outcomes. The same typed report drives source-path redaction, audit summaries and output selection. The frontend validates the complete report at the snapshot boundary, then presentation helpers consume its discriminated type without field coercion or legacy aliases. A malformed live report preserves the previous task and requests recovery; an invalid recovery batch cannot partially apply tasks or advance its revision. Contract tests serialize all 17 Rust variants and exercise the frontend decoder against those reports.

CLI convert and export, including batch scripts, compose one `ConversionRequest` and `ConversionOutcome`. The request owns path checks, options, destination authorization, credential retry and engine execution. Entry adapters retain argument parsing and envelope differences. Ordinary conversion keeps source and output passwords independent; batch conversion retains its explicit source-password inheritance rule.

Archive creation and conversion write through `archive_output::ArchiveOutput`. It owns the reserved file, writer handle, final size and identity check, cancellation boundary, publication and failure cleanup. Content writers receive a file and its format/options; they cannot publish it. Single outputs use the existing guarded replacement transaction, split outputs use the existing volume-set transaction, and caller-reserved SFX payloads remain available to the enclosing SFX build. Conversion drops its input reader before publication, including when source and destination are the same archive. SFX application packaging and durable recovery journals retain their own ownership contracts.

CLI compression and SQZ packing pass typed parsed arguments directly to their entry adapters. Ordinary and batch creation compose `CreationRequest`; its result includes the optional count from a successful integrity test, so adapters cannot accidentally present a failed test as a successful create. Batch path resolution and the existing JSON envelopes remain in batch adapters.

`Engine::resolve_external_task` owns file-manager selection filtering, preset policy, output naming, collision avoidance and extraction layout. Finder, Explorer and Linux integration scripts deliver raw actions and paths to the GUI, or the hidden `sqz file-manager` fallback. GUI and CLI adapt the resolved plan to their existing execution paths. The frontend requests that plan in both production and development; a missing desktop service produces an explicit unavailable state. Creation reserves a fresh output and refuses replacement if a destination appears after planning; preset split-volume choices participate in collision checks. System integration installation and desktop completion actions remain shell responsibilities.

GUI jobs publish `job://snapshot` through the requesting window's scoped sink. State, progress and questions are captured together under the snapshot lock and serialized after unlocking. Registration returns a full snapshot. Real-time updates omit the immutable, redacted job description; revision deltas include it on reset or for jobs created since the caller's revision. This bounds frequent progress traffic for large input lists. Frontend registration, events and recovery use one version reducer; unknown descriptionless events recover through registration or the next delta. Prompt versions remain independent identities, so progress never invalidates an answer. Only locally submitted tasks run completion effects, once.

ZIP, local-header ZIP recovery and TAR compose `squallz_format_api::test_entry_data` through the native formats' shared entry-test policy. The common reader owns observed-byte accounting, interruption retry, cancellation and byte progress; adapters own entry enumeration, metadata, EOF checksum validation and compound-stream tails. Credential and resource-limit failures remain hard errors instead of archive-damage reports.

SFX layouts share an exact-length copy pump for template and payload bytes, including progress, streaming buffer limits and detection of short or growing sources. Layout-specific callers retain source binding, seeking, digest verification, permissions, synchronization and publication. CRC32 and SHA256 remain chosen by their respective container contracts.

Temporary plaintext previews and stable format sources retain separate resource owners. Their lock scopes, identity checks, record formats and deletion rules differ. Any future shared mechanism must preserve those contracts without introducing more policy adapters than the repeated code it replaces. Recovery journals additionally represent durable transactions and are not interchangeable with temporary leases.

## Stylesheet responsibilities

`frontend/src/design.css` is the required central stylesheet for both layouts. Its current source has 15,334 physical lines, 13,299 nonblank lines, 2,040 CSS rules and 8,271 declarations; 592 declarations define custom properties and none use `!important`. These are source counts, including keyframe rules, not counts of independently rendered components. Rules whose selectors mention `classic` and `modern` number 304 and 162 respectively; other rules include shared controls, workflows, task windows and shell surfaces. Those selector groups do not partition the file.

This size is understandable for the current feature set and centralized-style constraint, but the ownership is not yet sufficiently clear. Shared control rules and layout variants are interleaved; for example, classic primary-button rules appear around lines 4,821 and 12,230, changing literal heights from 36px to 32px instead of expressing that variation with control-size tokens. Global theme application near the end lists many individual feature selectors. Adding a feature can therefore require changes both in its own rules and in distant theme/state lists. Responsive rules also span five viewport conditions before the hover/motion sections. Repeated selectors under different media conditions are often necessary and must not be removed as duplicates.

The next style refactor should define common control and task/result surfaces first, then express Classic/Modern layout differences and interaction states through those surfaces and tokens. Theme, viewport and reduced-motion behavior need explicit ownership inside the centralized stylesheet. A file split or minification alone would not reduce these responsibilities. Acceptance requires both layouts, light/dark/custom accent, narrow windows, focus/disabled/loading/error states and reduced motion; the Rust output refactor does not claim that visual work is complete.

## Change rules

- Add a business rule to its owning module and compose that module from each entry point. Do not duplicate workflow state for a second layout.
- Keep layout-specific markup and shared operations separate. Keep theme rules in centralized styles and design tokens.
- Store default user-facing copy in the locale catalogs. `tFallback(key)` resolves default English before locale loading; explicit alternate fallback text is reserved for an existing distinct contract. `t()` and `tError()` retain their missing-key signal for error classification.
- Preserve current archive formats, JSON envelopes, password lifetimes, cancellation, streaming limits and path safety when merging implementations.
- Count added shared modules when reporting source reductions. Report production source separately from tests, and code changes separately from repeated-copy removal. Compare both entry and total bundle sizes; source reduction alone does not establish faster startup.
- Verify changes with the repository checks in `AGENTS.md`, contract tests for all affected entry points, and real UI inspection for both layouts. Record environment limits explicitly.
