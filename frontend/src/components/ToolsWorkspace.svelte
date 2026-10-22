<script lang="ts" module>
  import type { ChecksumAlgorithmId } from "../lib/ui-model";
  import type { ToolsSession, ChecksumResultKind } from "../lib/tools-session.svelte";
  export type { ChecksumResultKind } from "../lib/tools-session.svelte";
  import type { ExtractReviewSession } from "../lib/extract-review.svelte";
  import type { CreateContentPolicy } from "../lib/ipc";
  import type { ArchiveUpdateReview } from "../lib/archive-update.svelte";

  export type ToolsWorkspaceKind = "batch" | "checksum" | "duplicates" | "update";
  export type ToolsWorkspaceVariant = "modern" | "classic";

  type Tr = (key: string, fallback?: string) => string;

  interface ArchiveReturnSurface {
    visible: boolean;
    title: string;
    detail: string;
    contextLabel: string;
    actionLabel: string;
    onReturn: () => void;
  }

  type LocalToolsSurface = {
    kind: "checksum" | "duplicates";
    variant: ToolsWorkspaceVariant;
    title: string;
    tr: Tr;
    archiveReturn: ArchiveReturnSurface;
    session: ToolsSession;
    onNavigate: (screen: "duplicates" | "recovery" | "create" | "batch") => void;
    onPanelMount: (kind: ChecksumResultKind | "duplicates", node: HTMLElement | null) => void;
  };
  export type ChecksumWorkspaceSurface = LocalToolsSurface & { kind: "checksum" };
  export type DuplicatesWorkspaceSurface = LocalToolsSurface & { kind: "duplicates" };
  type ChecksumReportSurface = ReturnType<ToolsSession["checksumReport"]>;

  export interface BatchWorkspaceSurface {
    kind: "batch";
    mode: "batch" | "nested";
    variant: ToolsWorkspaceVariant;
    title: string;
    tr: Tr;
    archiveReturn: ArchiveReturnSurface;
    onReady: () => void;
    session: ExtractReviewSession;
    format: (path: string) => string;
    onRemove: (index: number) => void;
    onBrowse: () => void;
  }

  export interface ArchiveUpdateWorkspaceSurface {
    kind: "update";
    variant: ToolsWorkspaceVariant;
    title: string;
    tr: Tr;
    archiveReturn: ArchiveReturnSurface;
    review: ArchiveUpdateReview;
    onChooseSource: (id: number, kind: "file" | "folder") => Promise<void>;
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
    ? surface.tr("gui.checksum.result")
    : surface.tr("gui.checksum.verification_result")}
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
          ? surface.tr("gui.checksum.result_preview_rows")
            .replace("{shown}", report.rows.length.toLocaleString())
            .replace("{total}", report.totalRows.toLocaleString())
          : surface.tr("gui.checksum.result_rows").replace("{count}", report.totalRows.toLocaleString())}</span>
      </div>
      <div class="checksum-result-copy">
        {#if report.feedback}
          <span class="checksum-copy-status" class:danger={report.feedbackDanger} role="status">{report.feedback}</span>
        {/if}
        <button type="button" class="primary-lite" disabled={report.rows.length === 0 || report.copyPending} aria-busy={report.copyPending} onclick={report.onCopy}>
          <Icon name="list" size={14} />{report.copyPending ? surface.tr("common.copying") : surface.tr("gui.checksum.copy_results")}
        </button>
      </div>
    </div>
    {#if report.context}
      <dl class="tool-report-context">
        <div>
          <dt>{report.kind === "checksum"
            ? surface.tr("gui.checksum.report_target")
            : surface.tr("gui.checksum.report_manifest")}</dt>
          <dd>{#each report.context.sources as source}<span>{source}</span>{/each}</dd>
        </div>
        <div><dt>{surface.tr("gui.checksum.algorithm", "Algorithm")}</dt><dd>{report.context.algorithm}</dd></div>
        {#each report.summary as item}
          <div><dt>{item.label}</dt><dd>{item.value}</dd></div>
        {/each}
        <div><dt>{surface.tr("common.status")}</dt><dd>{report.state}</dd></div>
      </dl>
    {/if}
    <!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions (focusable horizontal scroll region) -->
    <div
      class="checksum-result-table"
      role="region"
      tabindex="0"
      aria-label={surface.tr("gui.checksum.result_table")}
      onkeydown={scrollChecksumResults}
    >
      <table>
        <thead><tr>
          <th scope="col">{surface.tr("common.path")}</th>
          <th scope="col">{report.kind === "checksum" ? surface.tr("gui.table.size") : surface.tr("gui.checksum.expected")}</th>
          <th scope="col">{report.kind === "checksum" ? surface.tr("gui.checksum.digest") : surface.tr("gui.checksum.actual")}</th>
          <th scope="col">{surface.tr("common.status")}</th>
        </tr></thead>
        <tbody>
      {#if report.kind === "checksum"}
        {#each report.rows as row}
          <tr><td class="checksum-result-path">{row.name}</td><td>{row.size}</td><td><code class="checksum-digest">{row.digest}</code></td><td>{row.status}</td></tr>
        {:else}
          <tr><td colspan="4">{surface.tr("gui.checksum.no_result_yet")} · {report.state}</td></tr>
        {/each}
      {:else}
        {#each report.rows as row}
          <tr><td class="checksum-result-path">{row.name}</td><td><code class="checksum-digest">{row.expected}</code></td><td><code class="checksum-digest">{row.actual}</code></td><td>{row.status}</td></tr>
        {:else}
          <tr><td colspan="4">{surface.tr("gui.checksum.no_manifest_result_yet")} · {report.state}</td></tr>
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
          <span class="eyebrow">{surface.tr("gui.checksum.eyebrow")}</span>
          <h1>{surface.tr("gui.checksum.title")}</h1>
          <p>{surface.tr("gui.checksum.modern_subtitle", "Compute SHA-256, SHA-512, SHA-1, MD5, BLAKE3, or CRC32 with the shared core engine, or verify a checksum manifest.")}</p>
        </div>
        <button class="primary sheet-action" onclick={() => void surface.session.submit("checksum")}><Icon name="check-circle" size={17} />{surface.tr("gui.checksum.calculate")}</button>
      </div>

      <div class="settings-layout">
        <section class="settings-main-panel">
          <div class="settings-metric-grid">
            <div><span>{surface.tr("common.target")}</span><strong>{surface.session.targetName("checksum")}</strong><small>{surface.session.targetLabel("checksum")}</small></div>
            <div><span>{surface.tr("gui.checksum.algorithm", "Algorithm")}</span><strong>{surface.session.algorithmLabel()}</strong><small>{surface.tr("gui.checksum.matches_cli_algorithm")}</small></div>
            <div><span>{surface.tr("gui.checksum.hashed_files")}</span><strong>{surface.session.resultLabel(surface.session.result("checksum")?.files_hashed ?? 0)}</strong><small>{surface.session.resultLabel(surface.session.result("checksum")?.bytes_hashed ?? 0, true)}</small></div>
            <div><span>{surface.tr("gui.checksum.manifest_check")}</span><strong>{surface.session.resultLabel(surface.session.result("checksum_check")?.passed ?? 0)} / {surface.session.resultLabel(surface.session.result("checksum_check")?.checked ?? 0)}</strong><small>{surface.tr("gui.checksum.failed_count").replace("{count}", surface.session.resultLabel(surface.session.result("checksum_check")?.failed ?? 0))}</small></div>
          </div>

          <div class="level-control settings-slider">
            <div><strong>{surface.tr("gui.checksum.target")}</strong><span>{surface.session.targetLabel("checksum")}</span></div>
            <div class="path-preview">{surface.session.targetLabel("checksum")}</div>
            <div class="settings-actions-row">
              <button class="primary-lite" onclick={() => void surface.session.choose("file")}><Icon name="folder-open" size={15} />{surface.tr("gui.checksum.choose_file")}</button>
              <button onclick={() => void surface.session.choose("folder")}>{surface.tr("gui.checksum.choose_folder")}</button>
              <button
                disabled={Boolean(surface.session.currentArchiveDisabledReason)}
                title={surface.session.currentArchiveDisabledReason}
                aria-label={surface.session.currentArchiveDisabledReason
                  ? `${surface.tr("gui.checksum.use_current_archive")}: ${surface.session.currentArchiveDisabledReason}`
                  : surface.tr("gui.checksum.use_current_archive")}
                onclick={() => surface.session.useArchive("checksum")}
              >{surface.tr("gui.checksum.use_current_archive")}</button>
              <button class="primary-lite" onclick={() => void surface.session.submit("checksum")}><Icon name="check-circle" size={15} />{surface.tr("gui.checksum.calculate_now")}</button>
            </div>
            <div class="algorithm-field worker-field" role="group" aria-label={surface.tr("gui.checksum.algorithm", "Algorithm")}>
              <span>{surface.tr("gui.checksum.algorithm", "Algorithm")}</span>
              <ChecksumAlgorithmPicker
                algorithms={surface.session.algorithms}
                selected={surface.session.draft.algorithm}
                labelFor={(id) => surface.session.algorithmLabel(id)}
                hintFor={(id) => surface.session.algorithmHint(id)}
                onSelect={(algorithm) => surface.session.edit({ algorithm })}
              />
            </div>
          </div>

          <ExcludeRulesEditor
            title={surface.tr("gui.excludes.title")}
            hint={surface.tr("gui.excludes.folder_scan_hint")}
            countLabel={surface.tr("gui.excludes.count").replace("{count}", String(surface.session.excludes("checksum").length))}
            value={surface.session.draft.checksumExcludes}
            placeholder={surface.tr("gui.excludes.placeholder")}
            ariaLabel={surface.tr("gui.checksum.exclude_rules")}
            rules={surface.session.excludes("checksum")}
            emptyLabel={surface.tr("gui.create.no_rules")}
            onInput={(checksumExcludes) => surface.session.edit({ checksumExcludes })}
          />

          <div class="level-control settings-slider checksum-manifest-card">
            <div><strong>{surface.tr("gui.checksum.manifest_verification")}</strong><span>{surface.session.manifestLabel}</span></div>
            <div class="path-preview">{surface.session.manifestLabel}</div>
            <div class="settings-actions-row checksum-manifest-actions">
              <button class="primary-lite" onclick={() => void surface.session.choose("manifest")}><Icon name="folder-open" size={15} />{surface.tr("gui.checksum.choose_manifest")}</button>
              <button class="primary-lite" onclick={() => void surface.session.submit("checksum_check")}><Icon name="check-circle" size={15} />{surface.tr("gui.checksum.verify_manifest")}</button>
            </div>
          </div>

          {@render checksumReport(surface.session.checksumReport("checksum"))}
          {@render checksumReport(surface.session.checksumReport("checksum_check"))}
        </section>

        <aside class="settings-side-panel">
          <div class="panel-title"><Icon name="check-circle" size={16} />{surface.tr("gui.checksum.verification_contract")}</div>
          <div class="setting-callout">
            <strong>{surface.tr("gui.checksum.shared_with_cli")}</strong>
            <span>{surface.tr("gui.checksum.cli_contract_body")}</span>
          </div>
          <div class="settings-route-list">
            <button class="settings-route-card" onclick={() => surface.onNavigate("duplicates")}>
              <Icon name="search" size={16} />
              <span><strong>{surface.tr("gui.screen.duplicates")}</strong><small>{surface.tr("gui.duplicates.route_from_checksum")}</small></span>
            </button>
            <button class="settings-route-card" onclick={() => surface.onNavigate("recovery")}>
              <Icon name="shield-alert" size={16} />
              <span><strong>{surface.tr("gui.recovery.title")}</strong><small>{surface.tr("gui.recovery.route_from_checksum")}</small></span>
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
            <p>{surface.tr("gui.checksum.subtitle")}</p>
          </div>
          <div class="classic-button-row">
            <button onclick={() => void surface.session.choose("file")}>{surface.tr("gui.checksum.choose_file")}</button>
            <button onclick={() => void surface.session.choose("folder")}>{surface.tr("gui.checksum.choose_folder")}</button>
            <button class="classic-primary" onclick={() => void surface.session.submit("checksum")}>{surface.tr("gui.checksum.calculate")}</button>
          </div>
        </header>

        <div class="classic-batch-grid">
          <section>
            <h2>{surface.tr("gui.checksum.calculate_title")}</h2>
            <div class="classic-form-grid compact">
              <div class="classic-label">{surface.tr("common.target")}</div><div class="classic-input accent">{surface.session.targetLabel("checksum")}</div>
              <div class="classic-label">{surface.tr("gui.checksum.algorithm", "Algorithm")}</div>
              <ChecksumAlgorithmPicker
                algorithms={surface.session.algorithms}
                selected={surface.session.draft.algorithm}
                labelFor={(id) => surface.session.algorithmLabel(id)}
                hintFor={(id) => surface.session.algorithmHint(id)}
                onSelect={(algorithm) => surface.session.edit({ algorithm })}
                className="classic-algorithm-grid"
              />
              <div class="classic-label">{surface.tr("gui.create.excludes")}</div>
              <textarea
                class="classic-input"
                rows="4"
                value={surface.session.draft.checksumExcludes}
                aria-label={surface.tr("gui.checksum.exclude_rules")}
                oninput={(event) => surface.session.edit({ checksumExcludes: event.currentTarget.value })}
              ></textarea>
            </div>
            <button
              class="classic-color-route"
              disabled={Boolean(surface.session.currentArchiveDisabledReason)}
              title={surface.session.currentArchiveDisabledReason}
              aria-label={surface.session.currentArchiveDisabledReason
                ? `${surface.tr("gui.checksum.use_current_archive")}: ${surface.session.currentArchiveDisabledReason}`
                : surface.tr("gui.checksum.use_current_archive")}
              onclick={() => surface.session.useArchive("checksum")}
            ><Icon name="archive" size={15} />{surface.tr("gui.checksum.use_current_archive")}</button>
          </section>
          <aside>
            <h2>{surface.tr("gui.checksum.verify_manifest")}</h2>
            <div class="classic-form-grid compact no-pad">
              <div class="classic-label">{surface.tr("gui.checksum.manifest")}</div><div class="classic-input">{surface.session.manifestLabel}</div>
              <div class="classic-label">{surface.tr("gui.checksum.passed")}</div><div class="classic-input success">{surface.session.resultLabel(surface.session.result("checksum_check")?.passed ?? 0)}</div>
              <div class="classic-label">{surface.tr("gui.checksum.failed")}</div><div class="classic-input danger">{surface.session.resultLabel(surface.session.result("checksum_check")?.failed ?? 0)}</div>
              <div class="classic-label">{surface.tr("gui.checksum.checked")}</div><div class="classic-input">{surface.session.resultLabel(surface.session.result("checksum_check")?.checked ?? 0)}</div>
            </div>
            <div class="classic-button-row checksum-manifest-actions">
              <button onclick={() => void surface.session.choose("manifest")}>{surface.tr("gui.checksum.choose_manifest")}</button>
              <button class="classic-primary" onclick={() => void surface.session.submit("checksum_check")}>{surface.tr("gui.checksum.verify_manifest")}</button>
            </div>
          </aside>
        </div>

        {@render checksumReport(surface.session.checksumReport("checksum"))}
        {@render checksumReport(surface.session.checksumReport("checksum_check"))}
      </section>
    </div>
  {/if}
{:else if surface.kind === "duplicates"}
  {#if surface.variant === "modern"}
    <div class="settings-view modern-duplicates">
      <div class="sheet-head">
        <div>
          <span class="eyebrow">{surface.tr("gui.duplicates.eyebrow")}</span>
          <h1>{surface.tr("gui.duplicates.title")}</h1>
          <p>{surface.tr("gui.duplicates.modern_subtitle")}</p>
          <div class="duplicate-safety-strip" aria-label={surface.tr("gui.duplicates.safety_summary")}>
            <span><Icon name="list" size={14} />{surface.tr("gui.duplicates.grouped_review")}</span>
            <span><Icon name="check-circle" size={14} />{surface.tr("gui.duplicates.no_auto_delete")}</span>
          </div>
        </div>
        <button class="primary sheet-action" onclick={() => void surface.session.submit("duplicate_scan")}><Icon name="search" size={17} />{surface.tr("gui.duplicates.scan")}</button>
      </div>

      <div class="settings-layout">
        <section class="settings-main-panel">
          <div class="level-control settings-slider">
            <div><strong>{surface.tr("gui.duplicates.scan_target")}</strong><span>{surface.session.targetLabel("duplicate_scan")}</span></div>
            <div class="path-preview">{surface.session.targetLabel("duplicate_scan")}</div>
            <div class="settings-actions-row">
              <button class="primary-lite" onclick={() => void surface.session.choose("duplicates")}><Icon name="folder-open" size={15} />{surface.tr("gui.checksum.choose_folder")}</button>
              <button onclick={() => surface.session.useArchive("duplicate_scan")}>{surface.tr("gui.duplicates.use_archive_folder")}</button>
              <button class="primary-lite" onclick={() => void surface.session.submit("duplicate_scan")}><Icon name="search" size={15} />{surface.tr("gui.duplicates.scan_now")}</button>
            </div>
            <label class="number-field worker-field">
              <span>{surface.tr("gui.duplicates.minimum_hashed_size_bytes")}</span>
              <input
                type="number"
                min="0"
                step="1"
                value={surface.session.draft.minimumSize}
                class:invalid={surface.session.minimumSizeError.length > 0}
                aria-label={surface.tr("gui.duplicates.minimum_file_size")}
                aria-invalid={surface.session.minimumSizeError ? "true" : "false"}
                aria-describedby={surface.session.minimumSizeError ? "duplicate-min-size-error-modern" : undefined}
                oninput={(event) => surface.session.updateMinimumSize(event)}
              />
              {#if surface.session.minimumSizeError}
                <small id="duplicate-min-size-error-modern" class="duplicate-min-size-error" role="status" data-duplicate-min-size-error>{surface.session.minimumSizeError}</small>
              {/if}
            </label>
          </div>

          <ExcludeRulesEditor
            title={surface.tr("gui.excludes.title")}
            hint={surface.tr("gui.excludes.duplicate_hint", "Skip noisy folders before duplicate hashing.")}
            countLabel={surface.tr("gui.excludes.count").replace("{count}", String(surface.session.excludes("duplicate_scan").length))}
            value={surface.session.draft.duplicateExcludes}
            placeholder={surface.tr("gui.excludes.placeholder")}
            ariaLabel={surface.tr("gui.duplicates.exclude_rules", "Duplicate scan exclude rules")}
            rules={surface.session.excludes("duplicate_scan")}
            emptyLabel={surface.tr("gui.create.no_rules")}
            onInput={(duplicateExcludes) => surface.session.edit({ duplicateExcludes })}
          />

        </section>

        <aside class="settings-side-panel">
          <div class="panel-title"><Icon name="search" size={16} />{surface.tr("gui.duplicates.scan_contract")}</div>
          <div class="setting-callout">
            <strong>{surface.tr("gui.duplicates.non_destructive")}</strong>
            <span>{surface.tr("gui.duplicates.non_destructive_body", "The scan never cleans up, hard-links, deletes, moves, or modifies files automatically.")}</span>
          </div>
          <div class="settings-route-list">
            <button class="settings-route-card" onclick={() => surface.onNavigate("create")}>
              <Icon name="sparkles" size={16} />
              <span><strong>{surface.tr("gui.nav.create")}</strong><small>{surface.tr("gui.duplicates.route_to_create")}</small></span>
            </button>
            <button class="settings-route-card" onclick={() => surface.onNavigate("batch")}>
              <Icon name="list" size={16} />
              <span><strong>{surface.tr("gui.screen.batch", "Batch")}</strong><small>{surface.tr("gui.duplicates.route_to_batch", "Start archive work after reviewing targets")}</small></span>
            </button>
          </div>
        </aside>
      </div>
      {#key surface.session.duplicateReport.taskId}<DuplicateReport report={surface.session.duplicateReport} tr={surface.tr} onMount={(node) => surface.onPanelMount("duplicates", node)} />{/key}
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
            <p>{surface.tr("gui.duplicates.subtitle")}</p>
            <div class="duplicate-safety-strip classic-duplicate-safety" aria-label={surface.tr("gui.duplicates.safety_summary")}>
              <span><Icon name="list" size={13} />{surface.tr("gui.duplicates.grouped_review")}</span>
              <span><Icon name="check-circle" size={13} />{surface.tr("gui.duplicates.no_auto_delete")}</span>
            </div>
          </div>
          <div class="classic-button-row">
            <button onclick={() => void surface.session.choose("duplicates")}>{surface.tr("gui.checksum.choose_folder")}</button>
            <button onclick={() => surface.session.useArchive("duplicate_scan")}><Icon name="archive" size={15} />{surface.tr("gui.duplicates.use_archive_folder")}</button>
            <button class="classic-primary" onclick={() => void surface.session.submit("duplicate_scan")}>{surface.tr("gui.duplicates.scan")}</button>
          </div>
        </header>

        <div class="classic-batch-grid">
          <section>
            <h2>{surface.tr("gui.duplicates.scan_setup")}</h2>
            <div class="classic-form-grid compact">
              <div class="classic-label">{surface.tr("common.target")}</div><div class="classic-input accent">{surface.session.targetLabel("duplicate_scan")}</div>
              <div class="classic-label">{surface.tr("gui.duplicates.min_size")}</div>
              <input
                class="classic-input"
                class:invalid={surface.session.minimumSizeError.length > 0}
                type="number"
                min="0"
                step="1"
                value={surface.session.draft.minimumSize}
                oninput={(event) => surface.session.updateMinimumSize(event)}
                aria-label={surface.tr("gui.duplicates.minimum_file_size")}
                aria-invalid={surface.session.minimumSizeError ? "true" : "false"}
                aria-describedby={surface.session.minimumSizeError ? "duplicate-min-size-error-classic" : undefined}
              />
              {#if surface.session.minimumSizeError}
                <div></div>
                <small id="duplicate-min-size-error-classic" class="classic-input duplicate-min-size-error" role="status" data-duplicate-min-size-error>{surface.session.minimumSizeError}</small>
              {/if}
              <div class="classic-label">{surface.tr("gui.create.excludes")}</div>
              <textarea
                class="classic-input"
                rows="4"
                value={surface.session.draft.duplicateExcludes}
                aria-label={surface.tr("gui.duplicates.exclude_rules")}
                oninput={(event) => surface.session.edit({ duplicateExcludes: event.currentTarget.value })}
              ></textarea>
            </div>
          </section>
        </div>
        {#key surface.session.duplicateReport.taskId}<DuplicateReport report={surface.session.duplicateReport} tr={surface.tr} onMount={(node) => surface.onPanelMount("duplicates", node)} />{/key}
      </section>
    </div>
  {/if}
{:else if surface.kind === "update"}
  <ArchiveUpdateWorkspace {surface} />
{:else}
  <BatchExtractWorkspace {surface} />
{/if}
