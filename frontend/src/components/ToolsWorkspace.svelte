<script lang="ts" module>
  import type { ChecksumAlgorithmId } from "../lib/ui-model";
  import type { DuplicateReportSurface } from "./DuplicateReport.svelte";
  import type { OverwritePolicy, SymlinkPolicy } from "../lib/ipc";
  import type { CreateContentPolicy } from "../lib/ipc";
  import type { ArchiveUpdateReview } from "../lib/archive-update.svelte";

  export type ToolsWorkspaceKind = "batch" | "checksum" | "duplicates" | "update";
  export type ToolsWorkspaceVariant = "modern" | "classic";
  export type ChecksumResultKind = "checksum" | "checksum_check";

  type Tr = (key: string, fallback: string) => string;

  interface ArchiveReturnSurface {
    visible: boolean;
    title: string;
    detail: string;
    contextLabel: string;
    actionLabel: string;
    onReturn: () => void;
  }

  interface ChecksumResultRow {
    name: string;
    size: string;
    digest: string;
    status: string;
  }

  interface ChecksumVerificationRow {
    name: string;
    expected: string;
    actual: string;
    status: string;
  }

  interface ChecksumReportBase {
    context: { sources: string[]; algorithm: string } | null;
    summary: Array<{ label: string; value: string }>;
    totalRows: number;
    state: string;
    feedback: string | null;
    feedbackDanger: boolean;
    onCopy: () => void;
  }

  type ChecksumReportSurface = ChecksumReportBase & (
    | { kind: "checksum"; rows: ChecksumResultRow[] }
    | { kind: "checksum_check"; rows: ChecksumVerificationRow[] }
  );

  export interface ChecksumWorkspaceSurface {
    kind: "checksum";
    variant: ToolsWorkspaceVariant;
    title: string;
    tr: Tr;
    archiveReturn: ArchiveReturnSurface;
    target: {
      name: string;
      label: string;
      currentArchiveDisabledReason: string;
    };
    algorithm: {
      options: ChecksumAlgorithmId[];
      selected: ChecksumAlgorithmId;
      label: string;
      labelFor: (algorithm: ChecksumAlgorithmId) => string;
      hintFor: (algorithm: ChecksumAlgorithmId) => string;
      onSelect: (algorithm: ChecksumAlgorithmId) => void;
    };
    metrics: {
      filesHashed: string;
      bytesHashed: string;
      passed: string;
      checked: string;
      failed: string;
    };
    excludes: {
      value: string;
      rules: string[];
      countLabel: string;
      onInput: (value: string) => void;
    };
    manifestLabel: string;
    result: ChecksumReportSurface & { kind: "checksum" };
    verification: ChecksumReportSurface & { kind: "checksum_check" };
    actions: {
      onChooseFile: () => void;
      onChooseFolder: () => void;
      onUseCurrentArchive: () => void;
      onCalculate: () => void;
      onChooseManifest: () => void;
      onVerifyManifest: () => void;
      onOpenDuplicates: () => void;
      onOpenRecovery: () => void;
    };
    onPanelMount: (kind: ChecksumResultKind, node: HTMLElement | null) => void;
  }

  export interface DuplicatesWorkspaceSurface {
    kind: "duplicates";
    variant: ToolsWorkspaceVariant;
    title: string;
    tr: Tr;
    archiveReturn: ArchiveReturnSurface;
    target: {
      name: string;
      label: string;
    };
    minimumSize: {
      value: number;
      label: string;
      error: string;
      onInput: (event: Event) => void;
    };
    excludes: {
      value: string;
      rules: string[];
      countLabel: string;
      onInput: (value: string) => void;
    };
    report: DuplicateReportSurface;
    actions: {
      onChooseFolder: () => void;
      onUseArchiveFolder: () => void;
      onScan: () => void;
      onOpenCreate: () => void;
      onOpenBatch: () => void;
    };
  }

  export interface BatchWorkspaceSurface {
    kind: "batch";
    mode: "batch" | "nested";
    variant: ToolsWorkspaceVariant;
    title: string;
    tr: Tr;
    archiveReturn: ArchiveReturnSurface;
    onReady: () => void;
    locked: boolean;
    submitting: boolean;
    smart: boolean;
    onSmartChange: (value: boolean) => void;
    overwrite: OverwritePolicy;
    overwriteChoices: Array<{ id: OverwritePolicy; label: string }>;
    onOverwriteChange: (value: OverwritePolicy) => void;
    symlinks: SymlinkPolicy;
    symlinkChoices: Array<{ id: SymlinkPolicy; label: string }>;
    onSymlinksChange: (value: SymlinkPolicy) => void;
    rows: Array<{
      id: string;
      path: string;
      name: string;
      format: string;
      target: string;
      encoding: string;
      bestEffort: boolean;
      onBestEffortChange: ((value: boolean) => void) | null;
      onTargetInput: (value: string) => void;
      onChooseTarget: () => void;
      onRemove: (() => void) | null;
    }>;
    actions: {
      onStart: () => void;
      onAdd: (() => void) | null;
    };
  }

  export interface ArchiveUpdateWorkspaceSurface {
    kind: "update";
    variant: ToolsWorkspaceVariant;
    title: string;
    tr: Tr;
    archiveReturn: ArchiveReturnSurface;
    review: ArchiveUpdateReview;
    policyLabel: (policy: CreateContentPolicy) => string;
    onReady: () => void;
    onSubmit: () => Promise<void>;
    onOpenTasks: () => void;
  }

  export type ToolsWorkspaceSurface =
    | ArchiveUpdateWorkspaceSurface
    | BatchWorkspaceSurface
    | ChecksumWorkspaceSurface
    | DuplicatesWorkspaceSurface;
</script>

<script lang="ts">
  import ArchiveReturnStrip from "./ArchiveReturnStrip.svelte";
  import ChecksumAlgorithmPicker from "./ChecksumAlgorithmPicker.svelte";
  import ExcludeRulesEditor from "./ExcludeRulesEditor.svelte";
  import DuplicateReport from "./DuplicateReport.svelte";
  import BatchExtractWorkspace from "./BatchExtractWorkspace.svelte";
  import ArchiveUpdateWorkspace from "./ArchiveUpdateWorkspace.svelte";
  import Icon from "./Icon.svelte";

  let {
    surface,
  }: {
    surface: ToolsWorkspaceSurface;
  } = $props();

  function registerChecksumPanel(node: HTMLElement, kind: ChecksumResultKind) {
    if (surface.kind !== "checksum") return;
    surface.onPanelMount(kind, node);
    return {
      destroy() {
        if (surface.kind === "checksum") {
          surface.onPanelMount(kind, null);
        }
      },
    };
  }

  function scrollChecksumResults(event: KeyboardEvent): void {
    const target = event.currentTarget;
    if (!(target instanceof HTMLElement) || event.target !== target
      || event.altKey || event.ctrlKey || event.metaKey
      || target.scrollWidth <= target.clientWidth) return;
    const step = target.clientWidth / 2;
    let next: number;
    if (event.key === "ArrowRight") next = target.scrollLeft + step;
    else if (event.key === "ArrowLeft") next = target.scrollLeft - step;
    else if (event.key === "Home") next = 0;
    else if (event.key === "End") next = target.scrollWidth;
    else return;
    event.preventDefault();
    target.scrollLeft = next;
  }
</script>

{#snippet checksumReport(report: ChecksumReportSurface)}
  {@const title = report.kind === "checksum"
    ? surface.tr("gui.checksum.result", "Checksum result")
    : surface.tr("gui.checksum.verification_result", "Verification result")}
  <section
    class="checksum-result-panel"
    use:registerChecksumPanel={report.kind}
    tabindex="-1"
    aria-label={title}
  >
    <div class="checksum-result-actions">
      <div class="checksum-result-title">
        <strong>{title}</strong>
        <span>{report.totalRows > report.rows.length
          ? surface.tr("gui.checksum.result_preview_rows", "Showing {shown} of {total} rows; copy includes all available rows.")
            .replace("{shown}", report.rows.length.toLocaleString())
            .replace("{total}", report.totalRows.toLocaleString())
          : surface.tr("gui.checksum.result_rows", "{count} rows").replace("{count}", report.totalRows.toLocaleString())}</span>
      </div>
      <div class="checksum-result-copy">
        {#if report.feedback}
          <span class="checksum-copy-status" class:danger={report.feedbackDanger} role="status">{report.feedback}</span>
        {/if}
        <button type="button" class="primary-lite" disabled={report.rows.length === 0} onclick={report.onCopy}>
          <Icon name="list" size={14} />{surface.tr("gui.checksum.copy_results", "Copy results")}
        </button>
      </div>
    </div>
    {#if report.context}
      <dl class="tool-report-context">
        <div>
          <dt>{report.kind === "checksum"
            ? surface.tr("gui.checksum.report_target", "Report target")
            : surface.tr("gui.checksum.report_manifest", "Verified manifest")}</dt>
          <dd>{#each report.context.sources as source}<span>{source}</span>{/each}</dd>
        </div>
        <div><dt>{surface.tr("gui.checksum.algorithm", "Algorithm")}</dt><dd>{report.context.algorithm}</dd></div>
        {#each report.summary as item}
          <div><dt>{item.label}</dt><dd>{item.value}</dd></div>
        {/each}
        <div><dt>{surface.tr("common.status", "Status")}</dt><dd>{report.state}</dd></div>
      </dl>
    {/if}
    <!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions (focusable horizontal scroll region) -->
    <div
      class="checksum-result-table"
      role="region"
      tabindex="0"
      aria-label={surface.tr("gui.checksum.result_table", "Result rows")}
      onkeydown={scrollChecksumResults}
    >
      <table>
        <thead><tr>
          <th scope="col">{surface.tr("common.path", "Path")}</th>
          <th scope="col">{report.kind === "checksum" ? surface.tr("gui.table.size", "Size") : surface.tr("gui.checksum.expected", "Expected")}</th>
          <th scope="col">{report.kind === "checksum" ? surface.tr("gui.checksum.digest", "Digest") : surface.tr("gui.checksum.actual", "Actual")}</th>
          <th scope="col">{surface.tr("common.status", "Status")}</th>
        </tr></thead>
        <tbody>
      {#if report.kind === "checksum"}
        {#each report.rows as row}
          <tr><td class="checksum-result-path">{row.name}</td><td>{row.size}</td><td><code class="checksum-digest">{row.digest}</code></td><td>{row.status}</td></tr>
        {:else}
          <tr><td colspan="4">{surface.tr("gui.checksum.no_result_yet", "No checksum result yet")} · {report.state}</td></tr>
        {/each}
      {:else}
        {#each report.rows as row}
          <tr><td class="checksum-result-path">{row.name}</td><td><code class="checksum-digest">{row.expected}</code></td><td><code class="checksum-digest">{row.actual}</code></td><td>{row.status}</td></tr>
        {:else}
          <tr><td colspan="4">{surface.tr("gui.checksum.no_manifest_result_yet", "No manifest result yet")} · {report.state}</td></tr>
        {/each}
      {/if}
        </tbody>
      </table>
    </div>
  </section>
{/snippet}

{#if surface.kind === "checksum"}
  {#if surface.variant === "modern"}
    <div class="settings-view modern-checksum">
      <div class="sheet-head">
        <div>
          <span class="eyebrow">{surface.tr("gui.checksum.eyebrow", "Tools / Checksum")}</span>
          <h1>{surface.tr("gui.checksum.title", "Verify files without changing them")}</h1>
          <p>{surface.tr("gui.checksum.modern_subtitle", "Compute SHA-256, SHA-512, SHA-1, MD5, BLAKE3, or CRC32 with the shared core engine, or verify a checksum manifest.")}</p>
        </div>
        <button class="primary sheet-action" onclick={surface.actions.onCalculate}><Icon name="check-circle" size={17} />{surface.tr("gui.checksum.calculate", "Calculate checksum")}</button>
      </div>

      <div class="settings-layout">
        <section class="settings-main-panel">
          <div class="settings-metric-grid">
            <div><span>{surface.tr("common.target", "Target")}</span><strong>{surface.target.name}</strong><small>{surface.target.label}</small></div>
            <div><span>{surface.tr("gui.checksum.algorithm", "Algorithm")}</span><strong>{surface.algorithm.label}</strong><small>{surface.tr("gui.checksum.matches_cli_algorithm", "Matches sqz checksum --algorithm")}</small></div>
            <div><span>{surface.tr("gui.checksum.hashed_files", "Files hashed")}</span><strong>{surface.metrics.filesHashed}</strong><small>{surface.metrics.bytesHashed}</small></div>
            <div><span>{surface.tr("gui.checksum.manifest_check", "Manifest check")}</span><strong>{surface.metrics.passed} / {surface.metrics.checked}</strong><small>{surface.tr("gui.checksum.failed_count", "{count} failed").replace("{count}", surface.metrics.failed)}</small></div>
          </div>

          <div class="level-control settings-slider">
            <div><strong>{surface.tr("gui.checksum.target", "Checksum target")}</strong><span>{surface.target.label}</span></div>
            <div class="path-preview">{surface.target.label}</div>
            <div class="settings-actions-row">
              <button class="primary-lite" onclick={surface.actions.onChooseFile}><Icon name="folder-open" size={15} />{surface.tr("gui.checksum.choose_file", "Choose file")}</button>
              <button onclick={surface.actions.onChooseFolder}>{surface.tr("gui.checksum.choose_folder", "Choose folder")}</button>
              <button
                disabled={Boolean(surface.target.currentArchiveDisabledReason)}
                title={surface.target.currentArchiveDisabledReason}
                aria-label={surface.target.currentArchiveDisabledReason
                  ? `${surface.tr("gui.checksum.use_current_archive", "Use current archive")}: ${surface.target.currentArchiveDisabledReason}`
                  : surface.tr("gui.checksum.use_current_archive", "Use current archive")}
                onclick={surface.actions.onUseCurrentArchive}
              >{surface.tr("gui.checksum.use_current_archive", "Use current archive")}</button>
              <button class="primary-lite" onclick={surface.actions.onCalculate}><Icon name="check-circle" size={15} />{surface.tr("gui.checksum.calculate_now", "Calculate now")}</button>
            </div>
            <div class="algorithm-field worker-field" role="group" aria-label={surface.tr("gui.checksum.algorithm", "Algorithm")}>
              <span>{surface.tr("gui.checksum.algorithm", "Algorithm")}</span>
              <ChecksumAlgorithmPicker
                algorithms={surface.algorithm.options}
                selected={surface.algorithm.selected}
                labelFor={surface.algorithm.labelFor}
                hintFor={surface.algorithm.hintFor}
                onSelect={surface.algorithm.onSelect}
              />
            </div>
          </div>

          <ExcludeRulesEditor
            title={surface.tr("gui.excludes.title", "Excludes")}
            hint={surface.tr("gui.excludes.folder_scan_hint", "Applied only when the target is a folder.")}
            countLabel={surface.excludes.countLabel}
            value={surface.excludes.value}
            placeholder={surface.tr("gui.excludes.placeholder", ".git\nnode_modules\n.DS_Store")}
            ariaLabel={surface.tr("gui.checksum.exclude_rules", "Checksum exclude rules")}
            rules={surface.excludes.rules}
            emptyLabel={surface.tr("gui.create.no_rules", "No rules")}
            onInput={surface.excludes.onInput}
          />

          <div class="level-control settings-slider checksum-manifest-card">
            <div><strong>{surface.tr("gui.checksum.manifest_verification", "Manifest verification")}</strong><span>{surface.manifestLabel}</span></div>
            <div class="path-preview">{surface.manifestLabel}</div>
            <div class="settings-actions-row checksum-manifest-actions">
              <button class="primary-lite" onclick={surface.actions.onChooseManifest}><Icon name="folder-open" size={15} />{surface.tr("gui.checksum.choose_manifest", "Choose manifest")}</button>
              <button class="primary-lite" onclick={surface.actions.onVerifyManifest}><Icon name="check-circle" size={15} />{surface.tr("gui.checksum.verify_manifest", "Verify manifest")}</button>
            </div>
          </div>

          {@render checksumReport(surface.result)}
          {@render checksumReport(surface.verification)}
        </section>

        <aside class="settings-side-panel">
          <div class="panel-title"><Icon name="check-circle" size={16} />{surface.tr("gui.checksum.verification_contract", "Verification scope")}</div>
          <div class="setting-callout">
            <strong>{surface.tr("gui.checksum.shared_with_cli", "Shared with CLI")}</strong>
            <span>{surface.tr("gui.checksum.cli_contract_body", "This page maps to sqz checksum and sqz checksum --check; JSON result fields stay aligned with automation output.")}</span>
          </div>
          <div class="settings-route-list">
            <button class="settings-route-card" onclick={surface.actions.onOpenDuplicates}>
              <Icon name="search" size={16} />
              <span><strong>{surface.tr("gui.screen.duplicates", "Duplicate Finder")}</strong><small>{surface.tr("gui.duplicates.route_from_checksum", "Find identical local files with BLAKE3")}</small></span>
            </button>
            <button class="settings-route-card" onclick={surface.actions.onOpenRecovery}>
              <Icon name="shield-alert" size={16} />
              <span><strong>{surface.tr("gui.recovery.title", "Recovery")}</strong><small>{surface.tr("gui.recovery.route_from_checksum", "Test, protect, repair, and export archives")}</small></span>
            </button>
          </div>
        </aside>
      </div>
    </div>
  {:else}
    <div class="classic-dialog-body" class:with-archive-return={surface.archiveReturn.visible}>
      {#if surface.archiveReturn.visible}
        <ArchiveReturnStrip
          title={surface.archiveReturn.title}
          detail={surface.archiveReturn.detail}
          contextLabel={surface.archiveReturn.contextLabel}
          actionLabel={surface.archiveReturn.actionLabel}
          buttonClass="classic-primary"
          iconSize={15}
          onReturn={surface.archiveReturn.onReturn}
        />
      {/if}
      <section class="classic-extract-sheet classic-checksum">
        <header>
          <div>
            <h1>{surface.title}</h1>
            <p>{surface.tr("gui.checksum.subtitle", "Calculate local file digests or verify a manifest with the same engine exposed by sqz checksum.")}</p>
          </div>
          <div class="classic-button-row">
            <button onclick={surface.actions.onChooseFile}>{surface.tr("gui.checksum.choose_file", "Choose file")}</button>
            <button onclick={surface.actions.onChooseFolder}>{surface.tr("gui.checksum.choose_folder", "Choose folder")}</button>
            <button class="classic-primary" onclick={surface.actions.onCalculate}>{surface.tr("gui.checksum.calculate", "Calculate checksum")}</button>
          </div>
        </header>

        <div class="classic-batch-grid">
          <section>
            <h2>{surface.tr("gui.checksum.calculate_title", "Calculate")}</h2>
            <div class="classic-form-grid compact">
              <div class="classic-label">{surface.tr("common.target", "Target")}</div><div class="classic-input accent">{surface.target.label}</div>
              <div class="classic-label">{surface.tr("gui.checksum.algorithm", "Algorithm")}</div>
              <ChecksumAlgorithmPicker
                algorithms={surface.algorithm.options}
                selected={surface.algorithm.selected}
                labelFor={surface.algorithm.labelFor}
                hintFor={surface.algorithm.hintFor}
                onSelect={surface.algorithm.onSelect}
                className="classic-algorithm-grid"
              />
              <div class="classic-label">{surface.tr("gui.create.excludes", "Excludes")}</div>
              <textarea
                class="classic-input"
                rows="4"
                value={surface.excludes.value}
                aria-label={surface.tr("gui.checksum.exclude_rules", "Checksum exclude rules")}
                oninput={(event) => surface.excludes.onInput(event.currentTarget.value)}
              ></textarea>
            </div>
            <button
              class="classic-color-route"
              disabled={Boolean(surface.target.currentArchiveDisabledReason)}
              title={surface.target.currentArchiveDisabledReason}
              aria-label={surface.target.currentArchiveDisabledReason
                ? `${surface.tr("gui.checksum.use_current_archive", "Use current archive")}: ${surface.target.currentArchiveDisabledReason}`
                : surface.tr("gui.checksum.use_current_archive", "Use current archive")}
              onclick={surface.actions.onUseCurrentArchive}
            ><Icon name="archive" size={15} />{surface.tr("gui.checksum.use_current_archive", "Use current archive")}</button>
          </section>
          <aside>
            <h2>{surface.tr("gui.checksum.verify_manifest", "Verify manifest")}</h2>
            <div class="classic-form-grid compact no-pad">
              <div class="classic-label">{surface.tr("gui.checksum.manifest", "Manifest")}</div><div class="classic-input">{surface.manifestLabel}</div>
              <div class="classic-label">{surface.tr("gui.checksum.passed", "Passed")}</div><div class="classic-input success">{surface.metrics.passed}</div>
              <div class="classic-label">{surface.tr("gui.checksum.failed", "Failed")}</div><div class="classic-input danger">{surface.metrics.failed}</div>
              <div class="classic-label">{surface.tr("gui.checksum.checked", "Checked")}</div><div class="classic-input">{surface.metrics.checked}</div>
            </div>
            <div class="classic-button-row checksum-manifest-actions">
              <button onclick={surface.actions.onChooseManifest}>{surface.tr("gui.checksum.choose_manifest", "Choose manifest")}</button>
              <button class="classic-primary" onclick={surface.actions.onVerifyManifest}>{surface.tr("gui.checksum.verify_manifest", "Verify manifest")}</button>
            </div>
          </aside>
        </div>

        {@render checksumReport(surface.result)}
        {@render checksumReport(surface.verification)}
      </section>
    </div>
  {/if}
{:else if surface.kind === "duplicates"}
  {#if surface.variant === "modern"}
    <div class="settings-view modern-duplicates">
      <div class="sheet-head">
        <div>
          <span class="eyebrow">{surface.tr("gui.duplicates.eyebrow", "Tools / Duplicate Finder")}</span>
          <h1>{surface.tr("gui.duplicates.title", "Find duplicate local files")}</h1>
          <p>{surface.tr("gui.duplicates.modern_subtitle", "Find identical files in local folders and review every copy before deciding what to keep.")}</p>
          <div class="duplicate-safety-strip" aria-label={surface.tr("gui.duplicates.safety_summary", "Duplicate scan safety summary")}>
            <span><Icon name="list" size={14} />{surface.tr("gui.duplicates.grouped_review", "Grouped review before cleanup")}</span>
            <span><Icon name="check-circle" size={14} />{surface.tr("gui.duplicates.no_auto_delete", "No automatic deletion")}</span>
          </div>
        </div>
        <button class="primary sheet-action" onclick={surface.actions.onScan}><Icon name="search" size={17} />{surface.tr("gui.duplicates.scan", "Scan duplicates")}</button>
      </div>

      <div class="settings-layout">
        <section class="settings-main-panel">
          <div class="level-control settings-slider">
            <div><strong>{surface.tr("gui.duplicates.scan_target", "Scan target")}</strong><span>{surface.target.label}</span></div>
            <div class="path-preview">{surface.target.label}</div>
            <div class="settings-actions-row">
              <button class="primary-lite" onclick={surface.actions.onChooseFolder}><Icon name="folder-open" size={15} />{surface.tr("gui.checksum.choose_folder", "Choose folder")}</button>
              <button onclick={surface.actions.onUseArchiveFolder}>{surface.tr("gui.duplicates.use_archive_folder", "Use archive folder")}</button>
              <button class="primary-lite" onclick={surface.actions.onScan}><Icon name="search" size={15} />{surface.tr("gui.duplicates.scan_now", "Scan now")}</button>
            </div>
            <label class="number-field worker-field">
              <span>{surface.tr("gui.duplicates.minimum_hashed_size_bytes", "Minimum hashed size in bytes")}</span>
              <input
                type="number"
                min="0"
                step="1"
                value={surface.minimumSize.value}
                class:invalid={surface.minimumSize.error.length > 0}
                aria-label={surface.tr("gui.duplicates.minimum_file_size", "Duplicate minimum file size")}
                aria-invalid={surface.minimumSize.error ? "true" : "false"}
                aria-describedby={surface.minimumSize.error ? "duplicate-min-size-error-modern" : undefined}
                oninput={surface.minimumSize.onInput}
              />
              {#if surface.minimumSize.error}
                <small id="duplicate-min-size-error-modern" class="duplicate-min-size-error" role="status" data-duplicate-min-size-error>{surface.minimumSize.error}</small>
              {/if}
            </label>
          </div>

          <ExcludeRulesEditor
            title={surface.tr("gui.excludes.title", "Excludes")}
            hint={surface.tr("gui.excludes.duplicate_hint", "Skip noisy folders before duplicate hashing.")}
            countLabel={surface.excludes.countLabel}
            value={surface.excludes.value}
            placeholder={surface.tr("gui.excludes.placeholder", ".git\nnode_modules\n.DS_Store")}
            ariaLabel={surface.tr("gui.duplicates.exclude_rules", "Duplicate scan exclude rules")}
            rules={surface.excludes.rules}
            emptyLabel={surface.tr("gui.create.no_rules", "No rules")}
            onInput={surface.excludes.onInput}
          />

        </section>

        <aside class="settings-side-panel">
          <div class="panel-title"><Icon name="search" size={16} />{surface.tr("gui.duplicates.scan_contract", "Safe scan scope")}</div>
          <div class="setting-callout">
            <strong>{surface.tr("gui.duplicates.non_destructive", "Reads and marks duplicates only")}</strong>
            <span>{surface.tr("gui.duplicates.non_destructive_body", "The scan never cleans up, hard-links, deletes, moves, or modifies files automatically.")}</span>
          </div>
          <div class="settings-route-list">
            <button class="settings-route-card" onclick={surface.actions.onOpenCreate}>
              <Icon name="sparkles" size={16} />
              <span><strong>{surface.tr("gui.nav.create", "Create")}</strong><small>{surface.tr("gui.duplicates.route_to_create", "Use the same exclude semantics before compression")}</small></span>
            </button>
            <button class="settings-route-card" onclick={surface.actions.onOpenBatch}>
              <Icon name="list" size={16} />
              <span><strong>{surface.tr("gui.screen.batch", "Batch")}</strong><small>{surface.tr("gui.duplicates.route_to_batch", "Start archive work after reviewing targets")}</small></span>
            </button>
          </div>
        </aside>
      </div>
      {#key surface.report.taskId}<DuplicateReport report={surface.report} tr={surface.tr} />{/key}
    </div>
  {:else}
    <div class="classic-dialog-body" class:with-archive-return={surface.archiveReturn.visible}>
      {#if surface.archiveReturn.visible}
        <ArchiveReturnStrip
          title={surface.archiveReturn.title}
          detail={surface.archiveReturn.detail}
          contextLabel={surface.archiveReturn.contextLabel}
          actionLabel={surface.archiveReturn.actionLabel}
          buttonClass="classic-primary"
          iconSize={15}
          onReturn={surface.archiveReturn.onReturn}
        />
      {/if}
      <section class="classic-extract-sheet classic-duplicates">
        <header>
          <div>
            <h1>{surface.title}</h1>
            <p>{surface.tr("gui.duplicates.subtitle", "Find identical files in local folders and review every copy before deciding what to keep.")}</p>
            <div class="duplicate-safety-strip classic-duplicate-safety" aria-label={surface.tr("gui.duplicates.safety_summary", "Duplicate scan safety summary")}>
              <span><Icon name="list" size={13} />{surface.tr("gui.duplicates.grouped_review", "Grouped review before cleanup")}</span>
              <span><Icon name="check-circle" size={13} />{surface.tr("gui.duplicates.no_auto_delete", "No automatic deletion")}</span>
            </div>
          </div>
          <div class="classic-button-row">
            <button onclick={surface.actions.onChooseFolder}>{surface.tr("gui.checksum.choose_folder", "Choose folder")}</button>
            <button onclick={surface.actions.onUseArchiveFolder}><Icon name="archive" size={15} />{surface.tr("gui.duplicates.use_archive_folder", "Use archive folder")}</button>
            <button class="classic-primary" onclick={surface.actions.onScan}>{surface.tr("gui.duplicates.scan", "Scan duplicates")}</button>
          </div>
        </header>

        <div class="classic-batch-grid">
          <section>
            <h2>{surface.tr("gui.duplicates.scan_setup", "Scan setup")}</h2>
            <div class="classic-form-grid compact">
              <div class="classic-label">{surface.tr("common.target", "Target")}</div><div class="classic-input accent">{surface.target.label}</div>
              <div class="classic-label">{surface.tr("gui.duplicates.min_size", "Min size")}</div>
              <input
                class="classic-input"
                class:invalid={surface.minimumSize.error.length > 0}
                type="number"
                min="0"
                step="1"
                value={surface.minimumSize.value}
                oninput={surface.minimumSize.onInput}
                aria-label={surface.tr("gui.duplicates.minimum_file_size", "Duplicate minimum file size")}
                aria-invalid={surface.minimumSize.error ? "true" : "false"}
                aria-describedby={surface.minimumSize.error ? "duplicate-min-size-error-classic" : undefined}
              />
              {#if surface.minimumSize.error}
                <div></div>
                <small id="duplicate-min-size-error-classic" class="classic-input duplicate-min-size-error" role="status" data-duplicate-min-size-error>{surface.minimumSize.error}</small>
              {/if}
              <div class="classic-label">{surface.tr("gui.create.excludes", "Excludes")}</div>
              <textarea
                class="classic-input"
                rows="4"
                value={surface.excludes.value}
                aria-label={surface.tr("gui.duplicates.exclude_rules", "Duplicate exclude rules")}
                oninput={(event) => surface.excludes.onInput(event.currentTarget.value)}
              ></textarea>
            </div>
          </section>
        </div>
        {#key surface.report.taskId}<DuplicateReport report={surface.report} tr={surface.tr} />{/key}
      </section>
    </div>
  {/if}
{:else if surface.kind === "update"}
  <ArchiveUpdateWorkspace {surface} />
{:else}
  <BatchExtractWorkspace {surface} />
{/if}
