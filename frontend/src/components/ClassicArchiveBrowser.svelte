<script lang="ts" module>

  import type { ArchiveBrowserActions, ArchiveBrowserView, ArchivePreviewActions, ArchivePreviewView } from "../lib/archive-browser";

  export interface ClassicArchiveBrowserProps extends ArchiveBrowserActions, ArchivePreviewActions {
    view: ArchiveBrowserView & {
      archiveTitle: string;
      archiveFormatSummary: string;
      archiveOpen: boolean;
      openArchiveFirst: string;
      preview: ArchivePreviewView;
      emptyName: string;
      emptyStatus: string;
    };
  }
</script>

<script lang="ts">
  import Icon from "./Icon.svelte";
  import ArchiveEntryPreview from "./ArchiveEntryPreview.svelte";
  import ArchiveStructureWarning from "./ArchiveStructureWarning.svelte";
  import ArchiveBrowseRecovery from "./ArchiveBrowseRecovery.svelte";
  import ArchiveEditingStatus from "./ArchiveEditingStatus.svelte";
  import MoveConflictReview from "./MoveConflictReview.svelte";
  import { cssVariables } from "../lib/css-variables";
  import { archiveTabStopPath } from "../lib/archive-keyboard";

  let {
    view,
    tr,
    onOpenRoot,
    onOpenRecovery,
    onRetryBrowse,
    onRetryFormats,
    onOpenNestedPreview,
    onExtractNestedPreview,
    onClearPreview,
    onRetryPreview,
    onExtractPreviewFailure,
    onOpenPreview,
    onRevealPreview,
    onPreviewSelection,
    onCancelMoveConflict,
    onSubmitMoveReadyOnly,
    onSubmitMoveKeepBoth,
    onBrowseScroll,
    onSelectEntry,
    onActivateEntry,
    onEntryKeydown,
    onOpenEntryContext,
    onToggleEntrySelection,
    onToggleAllEntries,
    onPreviewEntry,
  }: ClassicArchiveBrowserProps = $props();

  let table: HTMLDivElement | undefined = $state();
  let focusedPath = $state<string | null>(null);
  let detailsVisible = $state(false);
  const tableId = $props.id();
  const tabStopPath = $derived(archiveTabStopPath(view.rows, focusedPath));

  $effect(() => {
    if ((!detailsVisible || view.totalRows === 0) && table) table.scrollLeft = 0;
  });

</script>

<div class="classic-body">
  <aside class="classic-tree" aria-label={tr("gui.aria.archive_folders")}>
    <button
      type="button"
      class="classic-tree-item active"
      disabled={!view.archiveOpen}
      title={tr("gui.nav.root")}
      onclick={onOpenRoot}
    ><Icon name="archive" size={15} />{view.archiveTitle}</button>
    {#if view.archiveOpen}
      <button
        type="button"
        class="classic-tree-item"
        onclick={onOpenRoot}
      ><Icon name="folder" size={15} />{tr("gui.list.archive_root")}</button>
    {:else}
      <div class="classic-tree-item muted" title={view.openArchiveFirst} aria-label={view.openArchiveFirst}>
        <Icon name="folder-open" size={15} />{view.openArchiveFirst}
      </div>
    {/if}
    <div class="tree-note">
      <strong>{tr("gui.archive.format")}</strong>
      <span>{view.archiveFormatSummary}</span>
    </div>
    <ArchiveEntryPreview view={view.preview} {tr} classic
      actions={{ onOpenNestedPreview, onExtractNestedPreview, onClearPreview, onRetryPreview, onExtractPreviewFailure, onOpenPreview, onRevealPreview, onPreviewSelection }} />
  </aside>

  <section class="classic-table-wrap">
    <div class="classic-table-header">
      {#if view.editingStatus}
        <ArchiveEditingStatus state={view.editingStatus} onRetry={onRetryFormats} />
      {/if}
      <ArchiveBrowseRecovery state={view.recovery} retryLabel={tr("gui.error.retry")} onRetry={onRetryBrowse} />
      {#if view.structureWarning}
        <ArchiveStructureWarning
          message={view.structureWarning}
          actionLabel={tr("gui.archive.open_zip_repair")}
          onRepair={onOpenRecovery}
        />
      {/if}
      <div class="classic-workbench-strip empty-workbench-strip">
        <span>{view.archiveOpen ? view.selectedSummary : view.openArchiveFirst}</span>
        <small role="status" aria-live="polite">
          {view.rowsPending && !view.recovery
            ? view.totalRows === 0 ? view.emptyName : tr("gui.list.loading_rows")
            : view.archiveOpen
            ? tr("gui.list.classic_keyboard_hint")
            : tr("gui.classic.empty_workbench_hint")}
        </small>
        <button
          type="button"
          class="classic-columns-toggle"
          aria-expanded={detailsVisible}
          aria-controls={tableId}
          onclick={() => detailsVisible = !detailsVisible}
        >{tr(detailsVisible ? "gui.list.hide_detail_columns" : "gui.list.show_detail_columns")}</button>
      </div>
      {#if view.conflict}
        <MoveConflictReview review={view.conflict} {tr} classic onCancel={onCancelMoveConflict} onReadyOnly={onSubmitMoveReadyOnly} onKeepBoth={onSubmitMoveKeepBoth} />
      {/if}
    </div>
    <p class="archive-selection-hint">{tr("gui.selection.range_hint")}</p>
    <div
      class="classic-table"
      class:with-details={detailsVisible}
      id={tableId}
      bind:this={table}
      role="table"
      aria-label={tr("gui.table.archive")}
      aria-busy={view.rowsPending}
      aria-rowcount={Math.max(view.totalRows + 1, 2)}
      aria-colcount={detailsVisible ? 8 : 4}
      aria-keyshortcuts="Meta+A Control+A"
      data-total-rows={view.totalRows}
    >
      <div class="classic-head" role="row" aria-rowindex="1">
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
          <span>{tr("gui.table.name")}</span>
        </span>
        <span role="columnheader">{tr("gui.table.size")}</span>
        <span role="columnheader">{tr("gui.table.packed")}</span>
        <span role="columnheader">{tr("gui.table.modified")}</span>
        {#if detailsVisible}
          <span role="columnheader">{tr("gui.table.ratio")}</span>
          <span role="columnheader">{tr("gui.table.crc")}</span>
          <span role="columnheader">{tr("gui.table.filename_encoding")}</span>
          <span role="columnheader">{tr("gui.table.attr")}</span>
        {/if}
      </div>
      <div
        class="virtual-scroll classic-virtual-scroll"
        role="rowgroup"
        data-virtual-list="classic"
        onscroll={onBrowseScroll}
      >
        <div
          class="virtual-pad"
          use:cssVariables={{ "--virtual-pad-height": `${view.paddingTop}px` }}
        ></div>
        {#each view.rows as entry, index (view.startIndex + index)}
          {#if entry}
            <div
              class:selected={entry.selected}
              class:previewing={entry.previewing}
              class="classic-row"
              role="row"
              aria-rowindex={(entry.virtualIndex ?? 0) + 2}
              aria-selected={entry.selected}
              tabindex={entry.source?.path === tabStopPath ? 0 : -1}
              aria-keyshortcuts="ArrowUp ArrowDown ArrowLeft ArrowRight Home End PageUp PageDown Shift+ArrowUp Shift+ArrowDown Shift+Space Space Enter Backspace Meta+ArrowUp Alt+ArrowUp E"
              data-row-index={entry.virtualIndex ?? ""}
              onfocusin={() => focusedPath = entry.source?.path ?? null}
              onclick={(event) => onSelectEntry(entry, event)}
              ondblclick={(event) => {
                if (event.target instanceof Element && event.target.closest("button, input")) return;
                event.preventDefault();
                onActivateEntry(entry);
              }}
              onkeydown={(event) => onEntryKeydown(event, entry)}
              oncontextmenu={(event) => onOpenEntryContext(event, entry)}
            >
              <span class="table-name" role="cell">
                <button
                  type="button"
                  class="row-select-toggle"
                  tabindex="-1"
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
                <span class="archive-entry-label">
                  <strong title={entry.source?.path ?? entry.name}>{entry.name}</strong>
                  {#if entry.location}<small title={entry.source?.path}>{entry.location}</small>{/if}
                </span>
                {#if entry.source}
                  <button
                    class="row-preview-button compact"
                    tabindex="-1"
                    disabled={view.selection.busy}
                    aria-busy={entry.previewBusy}
                    title={view.selection.busy ? view.selection.busyLabel : entry.previewActionLabel}
                    aria-label={`${view.selection.busy ? view.selection.busyLabel : entry.previewActionLabel} ${entry.name}`}
                    onclick={(event) => {
                      event.stopPropagation();
                      onPreviewEntry(entry);
                    }}
                  ><Icon name={entry.previewActionIcon} size={12} /></button>
                {/if}
              </span>
              <span role="cell">{entry.size}</span>
              <span role="cell">{entry.packed}</span>
              <span role="cell">{entry.modified}</span>
              {#if detailsVisible}
                <span role="cell">{entry.ratio}</span>
                <span role="cell">{entry.crc}</span>
                <span role="cell">{entry.encoding}</span>
                <span role="cell" title={entry.attr}>{entry.attr}</span>
              {/if}
            </div>
          {:else}
            <div class="archive-row-placeholder classic-row-placeholder" class:pending={view.rowsPending} aria-hidden="true"></div>
          {/if}
        {/each}
        {#if view.totalRows === 0}
          <div class="classic-row empty-row" role="row" aria-rowindex="2">
            <span class="table-name" role="cell">{view.emptyName}</span>
            <span role="cell">{view.emptyStatus}</span>
            {#if detailsVisible}
              <span role="cell">-</span>
              <span role="cell">-</span>
              <span role="cell">-</span>
              <span role="cell">-</span>
            {/if}
            <span role="cell">-</span>
            <span role="cell">-</span>
          </div>
        {/if}
        <div
          class="virtual-pad"
          use:cssVariables={{ "--virtual-pad-height": `${view.paddingBottom}px` }}
        ></div>
      </div>
    </div>
  </section>
</div>
