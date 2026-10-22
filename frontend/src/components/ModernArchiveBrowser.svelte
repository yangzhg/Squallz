<script lang="ts" module>
  import type { AppActionAvailability } from "../lib/app-actions";

  import type { ArchiveBrowserActions, ArchiveBrowserView, ArchivePreviewActions } from "../lib/archive-browser";

  export interface ModernArchiveBrowserProps extends ArchiveBrowserActions, ArchivePreviewActions {
    view: ArchiveBrowserView & {
      archive: {
        title: string;
        format: string;
        summary: string;
        dirs: readonly string[];
        canGoUp: boolean;
      };
      actions: {
        mutationDisabledReason: string;
        testDisabledReason: string;
        renameDisabledReason: string;
        deleteDisabledReason: string;
        moveDisabledReason: string;
        copyOutDisabledReason: string;
        enabled: AppActionAvailability;
        previewBusy: boolean;
        previewDisabledReason: string;
        previewLabel: string;
        previewIcon: "external-link" | "folder-open" | "eye";
        extractDestinationHint: string;
        extractAllLabel: string;
        extractSelectedLabel: string;
        nestedPreview: boolean;
      };
      encodingWarning: string | null;
      filterText: string;
      filterPending: boolean;
      filterStatus: string;
      emptyLabel: string;
    };
    onOpenBreadcrumb: (index: number) => void;
    onGoUp: () => void;
    onExtractAll: () => void;
    onExtractSelection: () => void;
    onTestArchive: () => void;
    onAddFiles: () => void;
    onConvert: () => void;
    onOpenInfo: () => void;
    onRenameSelection: () => void;
    onDeleteSelection: () => void;
    onMoveSelection: () => void;
    onCopyOutSelection: () => void;
    onCreateFolder: () => void;
    onRepairEncoding: () => void;
    onSearchInputMount: (input: HTMLInputElement | null) => void;
    onSearchInput: (value: string) => void;
    onSearchKeydown: (event: KeyboardEvent) => void;
    onClearSearch: () => void;
  }
</script>

<script lang="ts">
  import { onMount, tick } from "svelte";
  import Icon from "./Icon.svelte";
  import ArchiveStructureWarning from "./ArchiveStructureWarning.svelte";
  import ArchiveBrowseRecovery from "./ArchiveBrowseRecovery.svelte";
  import ArchiveEditingStatus from "./ArchiveEditingStatus.svelte";
  import MoveConflictReview from "./MoveConflictReview.svelte";
  import { cssVariables, type CssVariableMap } from "../lib/css-variables";

  let {
    view,
    tr,
    onOpenBreadcrumb,
    onGoUp,
    onOpenRoot,
    onExtractAll,
    onExtractSelection,
    onTestArchive,
    onAddFiles,
    onOpenRecovery,
    onRetryBrowse,
    onRetryFormats,
    onConvert,
    onOpenInfo,
    onRenameSelection,
    onDeleteSelection,
    onMoveSelection,
    onCopyOutSelection,
    onCreateFolder,
    onPreviewSelection,
    onOpenNestedPreview,
    onExtractNestedPreview,
    onCancelMoveConflict,
    onSubmitMoveReadyOnly,
    onSubmitMoveKeepBoth,
    onRepairEncoding,
    onSearchInputMount,
    onSearchInput,
    onSearchKeydown,
    onClearSearch,
    onBrowseScroll,
    onSelectEntry,
    onActivateEntry,
    onEntryKeydown,
    onOpenEntryContext,
    onToggleEntrySelection,
    onToggleAllEntries,
    onPreviewEntry,
  }: ModernArchiveBrowserProps = $props();

  let searchInput = $state<HTMLInputElement | null>(null);
  let breadcrumbTrail = $state<HTMLDivElement | null>(null);
  let actionsOpen = $state(false);
  let actionSummary = $state<HTMLElement | null>(null);
  let tools = $state<HTMLDetailsElement | null>(null);

  $effect(() => {
    const trail = breadcrumbTrail;
    view.archive.dirs.join("\u0000");
    if (!trail) return;
    void tick().then(() => {
      if (breadcrumbTrail !== trail) return;
      trail.scrollLeft = trail.scrollWidth;
    });
  });

  onMount(() => {
    const details = tools;
    details?.addEventListener("keydown", onToolsKeydown);
    onSearchInputMount(searchInput);
    return () => {
      details?.removeEventListener("keydown", onToolsKeydown);
      onSearchInputMount(null);
    };
  });

  function virtualPadVariables(height: number): CssVariableMap {
    return { "--virtual-pad-height": `${height}px` };
  }

  function labelWithDisabledReason(label: string, reason: string): string {
    return reason ? `${label} · ${reason}` : label;
  }

  function runToolAction(action: () => void): void {
    actionsOpen = false;
    actionSummary?.focus({ preventScroll: true });
    action();
  }

  function onToolsKeydown(event: KeyboardEvent): void {
    if (event.key !== "Escape" || !actionsOpen) return;
    event.preventDefault();
    event.stopPropagation();
    actionsOpen = false;
    actionSummary?.focus({ preventScroll: true });
  }
</script>

<div class="archive-top">
  <div class="archive-hero">
    <div class="archive-heading">
      <span class="archive-format-mark" aria-hidden="true">{view.archive.format}</span>
      <div class="archive-summary">
        <h1>{view.archive.title}</h1>
        <p>{view.archive.summary}</p>
      </div>
    </div>
    <div class="summary-actions">
      <button class="primary large" disabled={!view.actions.enabled.extract_all} title={view.actions.extractDestinationHint} onclick={onExtractAll}>
        <Icon name="archive" size={17} />{view.actions.extractAllLabel}
      </button>
      <button class="ghost large" disabled={!view.actions.enabled.extract_selection} title={view.actions.extractDestinationHint} onclick={onExtractSelection}>
        <Icon name="archive" size={17} />{view.actions.extractSelectedLabel}
      </button>
      <button
        class="ghost large"
        disabled={!view.actions.enabled.test_archive}
        title={view.actions.testDisabledReason}
        aria-label={labelWithDisabledReason(
          tr("gui.action.test_archive"),
          view.actions.testDisabledReason,
        )}
        onclick={onTestArchive}
      ><Icon name="check-circle" size={17} />{tr("gui.action.test_archive")}</button>
      <button
        class="ghost large"
        disabled={!view.actions.enabled.preview_entry}
        aria-busy={view.actions.previewBusy}
        title={view.actions.previewDisabledReason}
        aria-label={labelWithDisabledReason(
          view.actions.previewLabel,
          view.actions.previewDisabledReason,
        )}
        onclick={onPreviewSelection}
      ><Icon name={view.actions.previewIcon} size={17} />{view.actions.previewLabel}</button>
    </div>
    <details class="archive-tools" bind:this={tools} bind:open={actionsOpen}>
      <summary bind:this={actionSummary}>
        <span><Icon name="settings" size={15} />{tr("gui.archive.more_actions")}</span>
        <small>{view.selectedSummary}</small>
        <Icon name="chevron-down" size={14} />
      </summary>
      <div class="archive-tool-groups">
        <section>
          <h2>{tr("gui.inspector.archive")}</h2>
          <div class="archive-tool-actions">
            <button disabled={!view.actions.enabled.add_files} title={view.actions.mutationDisabledReason} onclick={() => runToolAction(onAddFiles)}><Icon name="file" size={15} />{tr("gui.action.add_files")}</button>
            <button disabled={!view.actions.enabled.new_folder} title={view.actions.mutationDisabledReason} onclick={() => runToolAction(onCreateFolder)}><Icon name="folder-open" size={15} />{tr("gui.action.new_folder")}</button>
            <button disabled={!view.actions.enabled.convert_archive} onclick={() => runToolAction(onConvert)}><Icon name="repeat" size={15} />{tr("gui.action.convert")}</button>
            <button disabled={!view.actions.enabled.archive_info} onclick={() => runToolAction(onOpenInfo)}><Icon name="info" size={15} />{tr("gui.archive.info")}</button>
          </div>
        </section>
        <section>
          <h2>{tr("gui.context.selection_actions")}</h2>
          <div class="archive-tool-actions">
            <button disabled={!view.actions.enabled.rename_entry} title={view.actions.renameDisabledReason} aria-label={labelWithDisabledReason(tr("gui.action.rename_selected"), view.actions.renameDisabledReason)} onclick={() => runToolAction(onRenameSelection)}><Icon name="repeat" size={15} />{tr("gui.action.rename_selected")}</button>
            <button disabled={!view.actions.enabled.move_entries} title={view.actions.moveDisabledReason} aria-label={labelWithDisabledReason(tr("gui.action.move_selected"), view.actions.moveDisabledReason)} onclick={() => runToolAction(onMoveSelection)}><Icon name="repeat" size={15} />{tr("gui.action.move_selected")}</button>
            <button disabled={!view.actions.enabled.delete_entries} title={view.actions.deleteDisabledReason} aria-label={labelWithDisabledReason(tr("gui.action.delete_selected"), view.actions.deleteDisabledReason)} onclick={() => runToolAction(onDeleteSelection)}><Icon name="x-circle" size={15} />{tr("gui.action.delete_selected")}</button>
            <button disabled={!view.actions.enabled.copy_entries} title={view.actions.copyOutDisabledReason} aria-label={labelWithDisabledReason(tr("gui.action.copy_out"), view.actions.copyOutDisabledReason)} onclick={() => runToolAction(onCopyOutSelection)}><Icon name="external-link" size={15} />{tr("gui.action.copy_out")}</button>
            {#if view.actions.nestedPreview}
              <button onclick={() => runToolAction(onOpenNestedPreview)}><Icon name="folder-open" size={15} />{tr("gui.action.open_nested")}</button>
              <button onclick={() => runToolAction(onExtractNestedPreview)}><Icon name="archive" size={15} />{tr("gui.action.extract_nested")}</button>
            {/if}
          </div>
          <p>{tr("gui.selection.range_hint")}</p>
          <p>{tr("gui.preview.keyboard_hint")}</p>
        </section>
        <section>
          <h2>{tr("gui.inspector.recovery")}</h2>
          <strong>{tr("gui.recovery.status_not_checked")}</strong>
          <p>{tr("gui.recovery.requires_recovery_data")}</p>
          <div class="archive-tool-actions">
            <button onclick={() => runToolAction(onOpenRecovery)}><Icon name="shield-alert" size={15} />{tr("gui.recovery.open_recovery")}</button>
          </div>
        </section>
      </div>
    </details>
  </div>

  {#if view.editingStatus}
    <ArchiveEditingStatus state={view.editingStatus} onRetry={onRetryFormats} />
  {/if}

  {#if view.conflict}
    <MoveConflictReview review={view.conflict} {tr} onCancel={onCancelMoveConflict} onReadyOnly={onSubmitMoveReadyOnly} onKeepBoth={onSubmitMoveKeepBoth} />
  {/if}

  {#if view.structureWarning}
    <ArchiveStructureWarning
      message={view.structureWarning}
      actionLabel={tr("gui.archive.open_zip_repair")}
      onRepair={onOpenRecovery}
    />
  {/if}

  {#if view.encodingWarning}
    <div class="warning-ribbon">
      <Icon name="alert-triangle" size={17} />
      <span>{view.encodingWarning}</span>
      <button onclick={onRepairEncoding}>{tr("gui.encoding.repair_with_gbk")}</button>
    </div>
  {/if}
</div>

<div class="modern-list" data-total-rows={view.totalRows}>
  <div class="archive-pathline archive-list-navigation">
    <div class="archive-path-actions" aria-label={tr("gui.nav.archive_navigation")}>
      <button
        type="button"
        disabled={!view.archive.canGoUp}
        title={tr("gui.nav.up")}
        onclick={onGoUp}
      ><Icon name="chevron-up" size={14} />{tr("gui.nav.up_short")}</button>
      <button
        type="button"
        disabled={view.archive.dirs.length === 0}
        title={tr("gui.nav.root")}
        onclick={onOpenRoot}
      ><Icon name="archive" size={14} />{tr("gui.nav.root_short")}</button>
    </div>
    <div bind:this={breadcrumbTrail} class="archive-breadcrumbs" aria-label={tr("gui.nav.archive_breadcrumbs")}>
      <button type="button" title={view.archive.title} onclick={() => onOpenBreadcrumb(-1)}>{view.archive.title}</button>
      {#each view.archive.dirs as dir, index}
        <i>/</i><button type="button" title={dir} onclick={() => onOpenBreadcrumb(index)}>{dir}</button>
      {/each}
    </div>
  </div>
  <div class="archive-filter-bar" role="search">
    <div class="archive-filter-field" class:searching={Boolean(view.filterText.trim())}>
      <Icon name="search" size={15} />
      <input
        bind:this={searchInput}
        value={view.filterText}
        aria-label={tr("gui.list.search_aria")}
        aria-busy={view.filterPending}
        title={tr("gui.list.search_shortcut")}
        placeholder={tr("gui.list.search_placeholder")}
        oninput={(event) => onSearchInput(event.currentTarget.value)}
        onkeydown={onSearchKeydown}
      />
      {#if view.filterText}
        <button
          type="button"
          aria-label={tr("gui.list.search_clear")}
          title={tr("gui.list.search_clear")}
          onclick={onClearSearch}
        ><Icon name="x-circle" size={14} /></button>
      {/if}
    </div>
    <span role="status" aria-live="polite">{view.rowsPending && !view.filterPending && !view.recovery ? tr("gui.list.loading_rows") : view.filterStatus}</span>
  </div>
  <div class="archive-list-feedback">
    <ArchiveBrowseRecovery state={view.recovery} retryLabel={tr("gui.error.retry")} onRetry={onRetryBrowse} />
  </div>
  <div
    class="modern-table"
    role="table"
    aria-label={tr("gui.table.archive")}
    aria-busy={view.rowsPending}
    aria-rowcount={Math.max(view.totalRows + 1, 2)}
    aria-keyshortcuts="Meta+A Control+A"
  >
    <div class="list-head" role="row" aria-rowindex="1">
      <span class="table-select-heading" role="columnheader">
        <button
          type="button"
          class="row-select-toggle"
          class:checked={view.selection.checked}
          class:mixed={view.selection.mixed}
          role="checkbox"
          aria-checked={view.selection.mixed ? "mixed" : view.selection.checked}
          aria-label={view.selection.label}
          title={view.selection.label}
          disabled={view.selection.disabled}
          onclick={onToggleAllEntries}
        ></button>
        <span>{tr("gui.list.col.name")}</span>
      </span>
      <span role="columnheader">{tr("gui.list.col.size")}</span>
      <span role="columnheader">{tr("gui.list.col.packed")}</span>
      <span role="columnheader">{tr("gui.list.col.modified")}</span>
    </div>
    <div
      class="virtual-scroll modern-virtual-scroll"
      role="rowgroup"
      data-virtual-list="modern"
      onscroll={onBrowseScroll}
    >
      <div class="virtual-pad" use:cssVariables={virtualPadVariables(view.paddingTop)}></div>
      {#each view.rows as entry, index (view.startIndex + index)}
        {#if entry}
          <div
            class="modern-row"
            class:selected={entry.selected}
            class:previewing={entry.previewing}
            role="row"
            aria-rowindex={(entry.virtualIndex ?? 0) + 2}
            aria-selected={entry.selected}
            tabindex="0"
            aria-keyshortcuts="ArrowUp ArrowDown Home End PageUp PageDown Shift+ArrowUp Shift+ArrowDown Space Enter Backspace Meta+ArrowUp Alt+ArrowUp E"
            data-row-index={entry.virtualIndex ?? ""}
            onclick={(event) => onSelectEntry(entry, event)}
            ondblclick={(event) => {
              if (event.target instanceof Element && event.target.closest("button, input")) return;
              event.preventDefault();
              onActivateEntry(entry);
            }}
            onkeydown={(event) => onEntryKeydown(event, entry)}
            oncontextmenu={(event) => onOpenEntryContext(event, entry)}
          >
            <div class="file-name" role="cell">
              <button
                type="button"
                class="row-select-toggle"
                class:checked={entry.selected}
                role="checkbox"
                aria-checked={entry.selected}
                aria-label={view.selection.busy ? view.selection.busyLabel : entry.selectionLabel}
                title={view.selection.busy ? view.selection.busyLabel : entry.selectionLabel}
                disabled={!entry.source || view.selection.busy}
                onclick={(event) => {
                  event.stopPropagation();
                  onToggleEntrySelection(entry, event);
                }}
              ></button>
              <span
                class="file-badge"
                class:type-folder={entry.type === "folder"}
                class:type-locked={entry.type === "locked"}
                class:type-warning={entry.type === "warning"}
                role="img"
                aria-label={entry.attr}
                title={entry.attr}
              >
                {#if entry.type === "locked"}
                  <Icon name="lock" size={14} />
                {:else if entry.source?.entry_type === "symlink" || entry.source?.entry_type === "hardlink"}
                  <Icon name="link" size={14} />
                {:else}
                  {entry.type === "folder"
                    ? "DIR"
                    : entry.type === "pdf"
                      ? "PDF"
                      : entry.type === "sheet"
                        ? "XLS"
                        : entry.type === "warning"
                          ? "TXT"
                          : "FILE"}
                {/if}
              </span>
              <span class="archive-entry-label">
                <strong title={entry.source?.path ?? entry.name}>{entry.name}</strong>
                {#if entry.location}<small title={entry.source?.path}>{entry.location}</small>{/if}
              </span>
              {#if entry.source}
                <button
                  class="row-preview-button"
                  disabled={view.selection.busy}
                  aria-busy={entry.previewBusy}
                  title={view.selection.busy ? view.selection.busyLabel : entry.previewActionLabel}
                  aria-label={`${view.selection.busy ? view.selection.busyLabel : entry.previewActionLabel} ${entry.name}`}
                  onclick={(event) => {
                    event.stopPropagation();
                    onPreviewEntry(entry);
                  }}
                ><Icon name={entry.previewActionIcon} size={13} /></button>
              {/if}
            </div>
            <span role="cell">{entry.size}</span>
            <span role="cell">{entry.packed}</span>
            <span role="cell">{entry.modified}</span>
          </div>
        {:else}
          <div class="archive-row-placeholder modern-row-placeholder" class:pending={view.rowsPending} aria-hidden="true"></div>
        {/if}
      {/each}
      {#if view.totalRows === 0}
        <div class="modern-row empty-row" role="row" aria-rowindex="2">
          <div class="file-name" role="cell" aria-colspan="4"><strong>{view.emptyLabel}</strong></div>
        </div>
      {/if}
      <div class="virtual-pad" use:cssVariables={virtualPadVariables(view.paddingBottom)}></div>
    </div>
  </div>
</div>
