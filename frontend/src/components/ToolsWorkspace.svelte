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
  import { tick } from "svelte";
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

  async function changeChecksumView(event: MouseEvent, update: () => void): Promise<void> {
    const panel = (event.currentTarget as HTMLElement).closest(".checksum-result-panel");
    update();
    await tick();
    const table = panel?.querySelector<HTMLElement>(".checksum-result-table");
    if (table?.isConnected) {
      table.scrollTop = 0;
      table.focus({ preventScroll: true });
    }
  }
</script>

{#snippet checksumReport(report: ChecksumReportSurface)}
  {@const title = report.kind === "checksum"
    ? surface.tr("gui.checksum.result")
    : surface.tr("gui.checksum.verification_result")}
  {#if report.context}
  <section
    class="checksum-result-panel"
    use:registerChecksumPanel={report.kind}
    tabindex="-1"
    aria-label={title}
  >
    <div class="checksum-result-actions">
      <div class="checksum-result-title">
        <strong>{title}</strong>
        <span>{surface.tr("gui.checksum.result_rows").replace("{count}", report.totalRows.toLocaleString())}</span>
      </div>
      <div class="checksum-result-copy">
        {#if report.feedback}
          <span class="checksum-copy-status" class:danger={report.feedbackDanger} role="status">{report.feedback}</span>
        {/if}
        <button type="button" class="secondary-lite" disabled={report.totalRows === 0 || report.copyPending} aria-busy={report.copyPending} onclick={report.onCopy}>
          <Icon name="list" size={14} />{report.copyPending ? surface.tr("common.copying") : surface.tr("gui.checksum.copy_results")}
        </button>
      </div>
    </div>
    {#if !report.matchesInput}
      <p class="checksum-report-notice">{surface.tr("gui.checksum.previous_result")}</p>
    {/if}
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
    {#if report.kind === "checksum_check" && report.totalRows > 0}
      <div class="checksum-result-filters" role="group" aria-label={surface.tr("gui.checksum.filter_results")}>
        <button type="button" class="secondary-lite" aria-pressed={!report.failuresOnly} onclick={(event) => void changeChecksumView(event, () => report.onFilter(false))}>
          {surface.tr("gui.checksum.all_results").replace("{count}", report.totalRows.toLocaleString())}
        </button>
        <button type="button" class="secondary-lite" aria-pressed={report.failuresOnly} onclick={(event) => void changeChecksumView(event, () => report.onFilter(true))}>
          {surface.tr("gui.checksum.failed_results").replace("{count}", report.failedRows.toLocaleString())}
        </button>
      </div>
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
          <tr><td colspan="4">{report.failuresOnly ? surface.tr("gui.checksum.no_failed_results") : `${surface.tr("gui.checksum.no_manifest_result_yet")} · ${report.state}`}</td></tr>
        {/each}
      {/if}
        </tbody>
      </table>
    </div>
    {#if report.filteredRows > 0}
      <nav class="checksum-result-pagination" aria-label={surface.tr("gui.checksum.result_pages")}>
        {#if report.pageCount > 1}
          <button type="button" class="secondary-lite" disabled={report.page === 0} onclick={(event) => void changeChecksumView(event, () => report.onPage(report.page - 1))}>{surface.tr("gui.checksum.previous_page")}</button>
        {/if}
        <span role="status">{surface.tr("gui.checksum.result_range")
          .replace("{start}", report.start.toLocaleString()).replace("{end}", report.end.toLocaleString())
          .replace("{total}", report.filteredRows.toLocaleString())}</span>
        {#if report.pageCount > 1}
          <button type="button" class="secondary-lite" disabled={report.page + 1 >= report.pageCount} onclick={(event) => void changeChecksumView(event, () => report.onPage(report.page + 1))}>{surface.tr("gui.checksum.next_page")}</button>
        {/if}
      </nav>
    {/if}
  </section>
  {/if}
{/snippet}

{#if surface.kind === "checksum"}
  <div class={surface.variant === "modern" ? "settings-view modern-checksum" : "classic-dialog-body"} class:with-archive-return={surface.variant === "classic" && surface.archiveReturn.visible}>
    {#if surface.variant === "classic" && surface.archiveReturn.visible}
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
    <section class={surface.variant === "modern" ? "checksum-workspace" : "classic-extract-sheet classic-checksum"}>
      <header class:sheet-head={surface.variant === "modern"}>
        <div>
          <h1>{surface.title}</h1>
          <p>{surface.tr("gui.checksum.subtitle")}</p>
        </div>
      </header>
      <div class="tool-workspace-inputs">
        <div class="algorithm-field" role="group" aria-label={surface.tr("gui.checksum.algorithm")}>
          <span>{surface.tr("gui.checksum.algorithm")}</span>
          <ChecksumAlgorithmPicker
            algorithms={surface.session.algorithms}
            selected={surface.session.draft.algorithm}
            labelFor={(id) => surface.session.algorithmLabel(id)}
            hintFor={(id) => surface.session.algorithmHint(id)}
            onSelect={(algorithm) => surface.session.edit({ algorithm })}
          />
        </div>
        <div class="checksum-input-grid">
          <section class="tool-input-section" aria-label={surface.tr("gui.checksum.calculate_title")}>
            <h2>{surface.tr("gui.checksum.calculate_title")}</h2>
            <p class="tool-target-path">{surface.session.targetLabel("checksum")}</p>
            <div class="settings-actions-row">
              <button class="secondary-lite" onclick={() => void surface.session.choose("file")}><Icon name="folder-open" size={15} />{surface.tr("gui.checksum.choose_file")}</button>
              <button class="secondary-lite" onclick={() => void surface.session.choose("folder")}>{surface.tr("gui.checksum.choose_folder")}</button>
              {#if !surface.session.currentArchiveDisabledReason}
                <button class="secondary-lite" onclick={() => surface.session.useArchive("checksum")}>{surface.tr("gui.checksum.use_current_archive")}</button>
              {/if}
            </div>
            <details class="tool-advanced-options">
              <summary>{surface.tr("gui.excludes.title")}</summary>
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
            </details>
            <div class="tool-input-submit"><button class={surface.variant === "modern" ? "primary" : "classic-primary"} onclick={() => void surface.session.submit("checksum")}><Icon name="check-circle" size={15} />{surface.tr("gui.checksum.calculate")}</button></div>
          </section>
          <section class="tool-input-section" aria-label={surface.tr("gui.checksum.manifest_verification")}>
            <h2>{surface.tr("gui.checksum.manifest_verification")}</h2>
            <p class="tool-target-path">{surface.session.manifestLabel}</p>
            <div class="settings-actions-row">
              <button class="secondary-lite" onclick={() => void surface.session.choose("manifest")}><Icon name="folder-open" size={15} />{surface.tr("gui.checksum.choose_manifest")}</button>
            </div>
            <p class="tool-input-hint">{surface.tr("gui.checksum.manifest_hint")}</p>
            <div class="tool-input-submit"><button class={surface.variant === "modern" ? "primary" : "classic-primary"} onclick={() => void surface.session.submit("checksum_check")}><Icon name="check-circle" size={15} />{surface.tr("gui.checksum.verify_manifest")}</button></div>
          </section>
        </div>
      </div>
      {@render checksumReport(surface.session.checksumReport("checksum"))}
      {@render checksumReport(surface.session.checksumReport("checksum_check"))}
    </section>
  </div>
{:else if surface.kind === "duplicates"}
  <div class={surface.variant === "modern" ? "settings-view modern-duplicates" : "classic-dialog-body"} class:with-archive-return={surface.variant === "classic" && surface.archiveReturn.visible}>
    {#if surface.variant === "classic" && surface.archiveReturn.visible}
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
    <section class={surface.variant === "modern" ? "duplicates-workspace" : "classic-extract-sheet classic-duplicates"}>
      <header class:sheet-head={surface.variant === "modern"}>
        <div><h1>{surface.title}</h1><p>{surface.tr("gui.duplicates.subtitle")}</p></div>
      </header>
      <div class="tool-workspace-inputs">
        <section class="tool-input-section" aria-label={surface.tr("gui.duplicates.scan_setup")}>
          <h2>{surface.tr("gui.duplicates.scan_target")}</h2>
          <p class="tool-target-path">{surface.session.targetLabel("duplicate_scan")}</p>
          <div class="settings-actions-row">
            <button class="secondary-lite" onclick={() => void surface.session.choose("duplicates")}><Icon name="folder-open" size={15} />{surface.tr("gui.checksum.choose_folder")}</button>
            {#if surface.session.hasArchive}
              <button class="secondary-lite" onclick={() => surface.session.useArchive("duplicate_scan")}>{surface.tr("gui.duplicates.use_archive_folder")}</button>
            {/if}
          </div>
          <div class="duplicate-size-field">
            <label for="duplicate-minimum-size">{surface.tr("gui.duplicates.minimum_size")}</label>
            <div class="duplicate-size-control">
              <input
                id="duplicate-minimum-size"
                type="number" min="0" step="any"
                value={surface.session.minimumSizeValue}
                class:invalid={surface.session.minimumSizeError.length > 0}
                aria-invalid={surface.session.minimumSizeError ? "true" : "false"}
                aria-describedby="duplicate-minimum-size-hint"
                oninput={(event) => surface.session.updateMinimumSize(event)}
              />
              <select value={surface.session.minimumSizeUnit} aria-label={surface.tr("gui.duplicates.size_unit")} onchange={(event) => surface.session.changeMinimumSizeUnit(event.currentTarget.value)}>
                <option value="b">{surface.tr("gui.duplicates.size_unit_b")}</option>
                <option value="kib">{surface.tr("gui.duplicates.size_unit_kib")}</option>
                <option value="mib">{surface.tr("gui.duplicates.size_unit_mib")}</option>
                <option value="gib">{surface.tr("gui.duplicates.size_unit_gib")}</option>
              </select>
            </div>
            <small id="duplicate-minimum-size-hint" class:duplicate-min-size-error={Boolean(surface.session.minimumSizeError)} role="status">
              {surface.session.minimumSizeError || surface.tr("gui.duplicates.minimum_size_hint").replace("{size}", surface.session.minimumSizeLabel)}
            </small>
          </div>
          <details class="tool-advanced-options">
            <summary>{surface.tr("gui.excludes.title")}</summary>
            <ExcludeRulesEditor
              title={surface.tr("gui.excludes.title")}
              hint={surface.tr("gui.excludes.duplicate_hint")}
              countLabel={surface.tr("gui.excludes.count").replace("{count}", String(surface.session.excludes("duplicate_scan").length))}
              value={surface.session.draft.duplicateExcludes}
              placeholder={surface.tr("gui.excludes.placeholder")}
              ariaLabel={surface.tr("gui.duplicates.exclude_rules")}
              rules={surface.session.excludes("duplicate_scan")}
              emptyLabel={surface.tr("gui.create.no_rules")}
              onInput={(duplicateExcludes) => surface.session.edit({ duplicateExcludes })}
            />
          </details>
          <div class="tool-input-submit">
            <button class={surface.variant === "modern" ? "primary" : "classic-primary"} onclick={() => void surface.session.submit("duplicate_scan")}><Icon name="search" size={15} />{surface.tr("gui.duplicates.scan")}</button>
            <span class="tool-input-hint">{surface.tr("gui.duplicates.no_auto_delete")}</span>
          </div>
        </section>
      </div>
      {#key surface.session.duplicateReport.taskId}<DuplicateReport report={surface.session.duplicateReport} tr={surface.tr} onMount={(node) => surface.onPanelMount("duplicates", node)} />{/key}
    </section>
  </div>
{:else if surface.kind === "update"}
  <ArchiveUpdateWorkspace {surface} />
{:else}
  <BatchExtractWorkspace {surface} />
{/if}
