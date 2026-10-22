# Code ownership and composition

Squallz has two desktop layouts and a CLI. Archive operations belong to the shared Rust engine; layouts and entry points compose those capabilities.

## Desktop frontend

| Responsibility | Owner | Consumers |
| --- | --- | --- |
| Create, extract, recovery and local-tool workflows | The corresponding session in `frontend/src/lib` | Classic and Modern workspaces |
| Checksum report ownership, filtering and pagination | `tools-session.svelte.ts` | The shared report in `ToolsWorkspace.svelte` |
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
| Modal focus containment and safe restoration | `modal-focus.ts` | Task, archive editor, publishing and first-run surfaces |
| Anchored menu positioning and keyboard movement | `popup-menu.ts` | Both quick-action menus and the archive context menu |
| Content fitting within the monitor work area | `task-window-size.ts` | Native task windows using the shared task card |

Each workflow has its own draft instance. Sharing the output-option implementation does not share credentials or editable state between creating and converting. Create retains SFX constraints; conversion retains its source archive, source credentials and preflight lifecycle. Both compose the same destination-option rules.

Each create or conversion session owns an independent `ArchiveOutputWorkflow` instance. That workflow owns request IDs, cancellation, pending plans, review and submission state. Sessions supply source selection, archive identity and credential policy. Every asynchronous check or confirmation is guarded by the current request owner. Overwrite authorization is bound to the inspected destination guard; a changed guard requires a new confirmation and a vanished destination clears authorization. Create retains its automatic-name policy, conversion rejects its source as an output, and recovery repair continues to prohibit replacement.

Task notifications, history and dialogs interpret results through one presentation model. History retains its compact, path-redacted text and notifications retain reveal actions and actionable warnings. Output availability is derived from the completed result even when a window recovers a task without owning its completion effects.

Application modals share centered placement and backdrop tokens in `design.css`. Task passwords and conflicts remain inside the task surface, while publishing replaces its application modal. Native dialogs defer incoming task questions until their promise settles. Escape is consumed by the current surface; closing a menu cannot also clear search or dismiss a preview. Escape never cancels an active job.

Menus measure their rendered size and anchor, clamp to the viewport, and provide arrow-key navigation. Escape restores available origin focus; outside clicks and navigation do not reclaim focus from the user's new target. Modal focus ignores hidden, disabled and inert controls, and deferred restoration does not steal focus from a newer surface.

Task control state and operation feedback appear inside the task surface. Showing a task's details consumes its queued completion and source-cleanup notifications without suppressing history, output actions or independent errors. Other notifications retain their remaining visible lifetime while a modal blocks them. Background output actions explicitly name the platform file manager.

Completed task surfaces prioritize the outcome and next step over detailed results and omit redundant completion progress bars. Failed or cancelled tasks retain their measured progress. Batch errors precede summary counts; long result and checksum lists scroll locally. Native task windows grow to fit measured content within the design-token height limit and monitor work area, preserving width and stopping automatic fitting after a manual resize. The outer body remains scrollable as a fallback for constrained windows.

Classic and Modern retain different layout, table columns and row heights. Their browser models derive rows and commands from the same state. Preview loading, failure, file and nested-archive actions have one implementation; compact and detailed nested-entry presentation remain explicit layout choices.

Both archive browsers keep one active row in the Tab order. Arrow and paging keys move between rows, Shift extends selection, Shift+Space toggles selection, and Space previews. Classic tables prioritize names and expose optional metadata columns. The row height used for virtual paging matches the centralized CSS token.

Create and conversion workspaces keep their primary review action in a shared bottom action area. Input acquisition, preflight, confirmation and task submission remain owned by their existing sessions. Configuration summaries and less frequent options use disclosures; output paths remain readable text.

Checksum inputs and completed reports have separate presentation. Reports retain their own targets, algorithm and statistics when inputs change. Verification defaults to failures when present; all rows remain available through filtering and pagination, and copying always uses the full report. Each report keeps its view state by task identity. Duplicate minimum-size units are a presentation choice over the existing byte-count contract.

The archive inspector is attached only to archive browsing and archive information. Tool workspaces own their relevant configuration and results. Main navigation exposes the current page, and palette selection uses native radio inputs. Primary buttons use a single background color with explicit foreground and hover tokens; theme previews do not imply an application-wide accessibility certification.

Page content uses opacity-only entry animation so layout ancestors do not change the containing block for anchored surfaces. Sticky action bars stay still; navigation and format choices use color feedback. Choice-label transitions and control movement are enabled only when motion is allowed. Reduced-motion tests resolve media conditions, selector specificity and source order, including hovered/pressed buttons and switch inputs.

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

`frontend/src/design.css` owns visual rules for both layouts, including shared controls, workflow surfaces, task windows and the application shell. Components select semantic classes; colors, dimensions, spacing, shadows, animation durations and stacking levels belong to centralized design tokens. Standalone inline styles and component-local visual rules are prohibited.

Shared controls and task/result surfaces define their common appearance and interaction states. Classic and Modern express layout differences through classes and tokens. Theme, viewport and reduced-motion rules remain in the same stylesheet. Selectors repeated under different media conditions can serve distinct behaviors and must be evaluated with their conditions, specificity and source order.

Visual changes require checks in both layouts, light and dark themes, custom accent colors, narrow windows and high-DPI displays. Hover, active, focus-visible, disabled, selected, loading and error states must remain clear. Reduced-motion behavior must cover entry animations and interaction feedback, and long results must scroll locally without making the page overflow horizontally.

## Change rules

- Add a business rule to its owning module and compose that module from each entry point. Do not duplicate workflow state for a second layout.
- Keep layout-specific markup and shared operations separate. Keep theme rules in centralized styles and design tokens.
- Store default user-facing copy in the locale catalogs. `tFallback(key)` resolves default English before locale loading; explicit alternate fallback text is reserved for an existing distinct contract. `t()` and `tError()` retain their missing-key signal for error classification.
- Preserve current archive formats, JSON envelopes, password lifetimes, cancellation, streaming limits and path safety when merging implementations.
- Count added shared modules when reporting source reductions. Report production source separately from tests, and code changes separately from repeated-copy removal. Compare both entry and total bundle sizes; source reduction alone does not establish faster startup.
- Verify Rust changes with formatting, strict Clippy and workspace tests; verify frontend changes with its type checks, tests and production build. Exercise affected contracts and inspect both desktop layouts. State environment limits in the review or task discussion.
