<script lang="ts" module>
  import type { EntryDto } from "../lib/ipc";

  type Translate = (key: string, fallback: string) => string;

  export type ModernBrowserEntry = {
    name: string;
    location: string;
    type: string;
    size: string;
    packed: string;
    ratio: string;
    modified: string;
    crc: string;
    method: string;
    attr: string;
    source?: EntryDto;
    virtualIndex?: number;
    selected: boolean;
    previewing: boolean;
    previewBusy: boolean;
    selectionLabel: string;
    previewActionLabel: string;
    previewActionIcon: "external-link" | "folder-open" | "eye";
  };

  type MoveConflictItem = {
    from: string;
    reason: string | null;
    to: string;
    keepBothTo: string | null;
  };

  export interface ModernArchiveBrowserProps {
    view: {
      archive: {
        title: string;
        format: string;
        summary: string;
        dirs: readonly string[];
        readOnly: boolean;
        canGoUp: boolean;
      };
      actions: {
        mutationDisabledReason: string;
        renameDisabledReason: string;
        deleteDisabledReason: string;
        moveDisabledReason: string;
        canRenameSelection: boolean;
        hasSelection: boolean;
        canPreviewSelection: boolean;
        previewBusy: boolean;
        previewDisabledReason: string;
        previewLabel: string;
        previewIcon: "external-link" | "folder-open" | "eye";
        extractDestinationHint: string;
        extractAllLabel: string;
        extractSelectedLabel: string;
        nestedPreview: boolean;
      };
      workbench: {
        renameTarget: string;
        renameStatus: string;
        moveTarget: string;
        normalizedMoveTarget: string;
        moveTargetPresets: readonly string[];
        moveStatus: string;
        newFolderName: string;
        newFolderStatus: string;
      };
      conflict: {
        count: number;
        readyCount: number;
        targetDir: string;
        items: readonly MoveConflictItem[];
      } | null;
      structureWarning: string | null;
      encodingWarning: string | null;
      totalRows: number;
      filterText: string;
      filterPending: boolean;
      filterStatus: string;
      selection: {
        checked: boolean;
        mixed: boolean;
        disabled: boolean;
        label: string;
        busy: boolean;
        busyLabel: string;
      };
      rows: readonly ModernBrowserEntry[];
      paddingTop: number;
      paddingBottom: number;
      emptyLabel: string;
    };
    tr: Translate;
    onOpenBreadcrumb: (index: number) => void;
    onGoUp: () => void;
    onOpenRoot: () => void;
    onExtractAll: () => void;
    onExtractSelection: () => void;
    onAddFiles: () => void;
    onOpenRecovery: () => void;
    onConvert: () => void;
    onOpenInfo: () => void;
    onRenameSelection: () => void;
    onDeleteSelection: () => void;
    onMoveSelection: () => void;
    onCreateFolder: () => void;
    onPreviewSelection: () => void;
    onOpenNestedPreview: () => void;
    onExtractNestedPreview: () => void;
    onRenameTargetChange: (value: string) => void;
    onCommitRenameTarget: () => void;
    onMoveTargetChange: (value: string) => void;
    onCommitMoveTarget: (target?: string) => void;
    onNewFolderChange: (value: string) => void;
    onCommitNewFolder: () => void;
    onCancelMoveConflict: () => void;
    onSubmitMoveReadyOnly: () => void;
    onSubmitMoveKeepBoth: () => void;
    onRepairEncoding: () => void;
    onSearchInputMount: (input: HTMLInputElement | null) => void;
    onSearchInput: (value: string) => void;
    onSearchKeydown: (event: KeyboardEvent) => void;
    onClearSearch: () => void;
    onBrowseScroll: (event: Event) => void;
    onSelectEntry: (entry: ModernBrowserEntry, event: MouseEvent | KeyboardEvent) => void;
    onActivateEntry: (entry: ModernBrowserEntry) => void;
    onEntryKeydown: (event: KeyboardEvent, entry: ModernBrowserEntry) => void;
    onOpenEntryContext: (event: MouseEvent, entry: ModernBrowserEntry) => void;
    onToggleEntrySelection: (entry: ModernBrowserEntry) => void;
    onToggleAllEntries: () => void;
    onPreviewEntry: (entry: ModernBrowserEntry) => void;
  }
</script>

<script lang="ts">
  import { onMount, tick } from "svelte";
  import Icon from "./Icon.svelte";
  import ArchiveStructureWarning from "./ArchiveStructureWarning.svelte";
  import { cssVariables, type CssVariableMap } from "../lib/css-variables";

  let {
    view,
    tr,
    onOpenBreadcrumb,
    onGoUp,
    onOpenRoot,
    onExtractAll,
    onExtractSelection,
    onAddFiles,
    onOpenRecovery,
    onConvert,
    onOpenInfo,
    onRenameSelection,
    onDeleteSelection,
    onMoveSelection,
    onCreateFolder,
    onPreviewSelection,
    onOpenNestedPreview,
    onExtractNestedPreview,
    onRenameTargetChange,
    onCommitRenameTarget,
    onMoveTargetChange,
    onCommitMoveTarget,
    onNewFolderChange,
    onCommitNewFolder,
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
    onSearchInputMount(searchInput);
    return () => onSearchInputMount(null);
  });

  function virtualPadVariables(height: number): CssVariableMap {
    return { "--virtual-pad-height": `${height}px` };
  }

  function labelWithDisabledReason(label: string, reason: string): string {
    return reason ? `${label} · ${reason}` : label;
  }
</script>

<div class="archive-top">
  <div class="archive-hero">
    <div class="archive-object" aria-hidden="true">
      <div class="archive-lid"></div>
      <div class="archive-core">
        <span>{view.archive.format}</span>
        <i></i>
      </div>
    </div>
    <div class="archive-summary">
      <span class="eyebrow">{tr("gui.archive.secure_archive", "Secure archive")}</span>
      <h1>{view.archive.title}</h1>
      <p>{view.archive.summary}</p>
    </div>
    <div class="summary-actions">
      <button class="primary large" title={view.actions.extractDestinationHint} onclick={onExtractAll}>
        <Icon name="archive" size={17} />{view.actions.extractAllLabel}
      </button>
      {#if view.actions.hasSelection}
        <button class="ghost large" title={view.actions.extractDestinationHint} onclick={onExtractSelection}>
          <Icon name="archive" size={17} />{view.actions.extractSelectedLabel}
        </button>
      {/if}
      <button
        class="ghost large"
        disabled={view.archive.readOnly}
        title={view.actions.mutationDisabledReason}
        onclick={onAddFiles}
      ><Icon name="file" size={17} />{tr("gui.action.add_files", "Add files")}</button>
      <button
        class="ghost large"
        disabled={view.archive.readOnly}
        title={view.actions.mutationDisabledReason}
        onclick={onOpenRecovery}
      ><Icon name="shield-alert" size={17} />{tr("gui.action.protect", "Protect")}</button>
      <button class="ghost large" onclick={onConvert}>
        <Icon name="repeat" size={17} />{tr("gui.action.convert", "Convert")}
      </button>
      <button class="ghost large" onclick={onOpenInfo}>
        <Icon name="info" size={17} />{tr("gui.archive.info", "Info")}
      </button>
      <button
        class="ghost large"
        disabled={!view.actions.canRenameSelection}
        title={view.actions.renameDisabledReason}
        aria-label={labelWithDisabledReason(
          tr("gui.action.rename_selected", "Rename selected"),
          view.actions.renameDisabledReason,
        )}
        onclick={onRenameSelection}
      ><Icon name="repeat" size={17} />{tr("gui.action.rename_selected", "Rename selected")}</button>
      <button
        class="ghost large"
        disabled={Boolean(view.actions.mutationDisabledReason) || !view.actions.hasSelection}
        title={view.actions.deleteDisabledReason}
        aria-label={labelWithDisabledReason(
          tr("gui.action.delete_selected", "Delete selected"),
          view.actions.deleteDisabledReason,
        )}
        onclick={onDeleteSelection}
      ><Icon name="x-circle" size={17} />{tr("gui.action.delete_selected", "Delete selected")}</button>
      <button
        class="ghost large"
        disabled={Boolean(view.actions.mutationDisabledReason) || !view.actions.hasSelection}
        title={view.actions.moveDisabledReason}
        aria-label={labelWithDisabledReason(
          tr("gui.action.move_selected", "Move selected"),
          view.actions.moveDisabledReason,
        )}
        onclick={onMoveSelection}
      ><Icon name="repeat" size={17} />{tr("gui.action.move_selected", "Move selected")}</button>
      <button
        class="ghost large"
        disabled={view.archive.readOnly}
        title={view.actions.mutationDisabledReason}
        onclick={onCreateFolder}
      ><Icon name="folder-open" size={17} />{tr("gui.action.new_folder", "New folder")}</button>
      <button
        class="ghost large"
        disabled={!view.actions.canPreviewSelection}
        aria-busy={view.actions.previewBusy}
        title={view.actions.previewDisabledReason}
        aria-label={labelWithDisabledReason(
          view.actions.previewLabel,
          view.actions.previewDisabledReason,
        )}
        onclick={onPreviewSelection}
      ><Icon name={view.actions.previewIcon} size={17} />{view.actions.previewLabel}</button>
      {#if view.actions.nestedPreview}
        <button class="ghost large" onclick={onOpenNestedPreview}>
          <Icon name="folder-open" size={17} />{tr("gui.action.open_nested", "Open")}
        </button>
        <button class="ghost large" onclick={onExtractNestedPreview}>
          <Icon name="archive" size={17} />{tr("gui.action.extract_nested", "Extract")}
        </button>
      {/if}
    </div>
  </div>

  {#if view.actions.hasSelection}
    <div class="workbench-strip">
      <div class="update-safety-strip" aria-label={tr("gui.update.safety_summary", "Archive update safety summary")}>
        <span><Icon name="check-circle" size={14} />{tr("gui.update.selection_scoped", "Selection-scoped updates")}</span>
        <span><Icon name="list" size={14} />{tr("gui.update.target_review", "Review rename and move targets first")}</span>
        <span><Icon name="archive" size={14} />{tr("gui.update.format_boundaries", "Write-capable formats only")}</span>
      </div>
      <label>
        <span>{tr("gui.action.rename_target", "Rename target")}</span>
        <input
          aria-label={tr("gui.rename.target_name", "Rename target name")}
          value={view.workbench.renameTarget}
          disabled={!view.actions.canRenameSelection}
          title={view.actions.canRenameSelection ? "" : tr("gui.precondition.select_one_file", "Select exactly one file")}
          oninput={(event) => onRenameTargetChange(event.currentTarget.value)}
          onblur={onCommitRenameTarget}
        />
      </label>
      <label>
        <span>{tr("gui.action.move_target", "Move target")}</span>
        <input
          aria-label={tr("gui.move.target_folder", "Move target folder")}
          value={view.workbench.moveTarget}
          disabled={view.archive.readOnly || !view.actions.hasSelection}
          title={view.actions.moveDisabledReason}
          oninput={(event) => onMoveTargetChange(event.currentTarget.value)}
          onblur={() => onCommitMoveTarget()}
        />
      </label>
      <small>{view.workbench.renameStatus}</small>
      <div class="move-target-presets compact" aria-label={tr("gui.move.target_presets", "Move target presets")}>
        {#each view.workbench.moveTargetPresets as target}
          <button
            class:active={view.workbench.normalizedMoveTarget === target}
            disabled={view.archive.readOnly || !view.actions.hasSelection}
            onclick={() => onCommitMoveTarget(target)}
          >{target}</button>
        {/each}
      </div>
      <small>{view.workbench.moveStatus}</small>
      <label>
        <span>{tr("gui.action.new_folder", "New folder")}</span>
        <input
          aria-label={tr("gui.new_folder.name", "New folder name")}
          value={view.workbench.newFolderName}
          disabled={view.archive.readOnly}
          title={view.actions.mutationDisabledReason}
          oninput={(event) => onNewFolderChange(event.currentTarget.value)}
          onblur={onCommitNewFolder}
        />
      </label>
      <small class="workbench-note">{view.workbench.newFolderStatus}</small>
    </div>
  {:else}
    <div class="workbench-strip empty-workbench-strip">
      <span>{tr("gui.selection.select_entries_hint", "Select entries to open, rename, move, or extract.")}</span>
      <small>{tr("gui.preview.keyboard_hint", "Space or Return opens the focused item")}</small>
    </div>
  {/if}

  {#if view.conflict}
    <div class="move-conflict-review" role="dialog" aria-label={tr("gui.move.conflicts", "Move target conflicts")} tabindex="-1">
      <div>
        <span class="block-label">{tr("gui.move.conflicts", "Move target conflicts")}</span>
        <strong>
          {tr("gui.move.target_conflicts", "{count} target conflicts in {target}")
            .replace("{count}", String(view.conflict.count))
            .replace("{target}", view.conflict.targetDir)}
        </strong>
        <p>
          {tr("gui.move.ready_without_renaming", "{count} entries are ready to move without changing names.")
            .replace("{count}", String(view.conflict.readyCount))}
        </p>
      </div>
      <div class="move-conflict-list">
        {#each view.conflict.items as item}
          <div>
            <strong>{item.from}</strong>
            <span>{item.reason}</span>
            <em>{item.to}</em>
            <b>{item.keepBothTo}</b>
          </div>
        {/each}
      </div>
      <div class="move-conflict-actions">
        <button onclick={onCancelMoveConflict}>{tr("common.cancel", "Cancel")}</button>
        <button disabled={view.conflict.readyCount === 0} onclick={onSubmitMoveReadyOnly}>
          {tr("gui.move.ready_only", "Move ready only")}
        </button>
        <button class="primary-lite" onclick={onSubmitMoveKeepBoth}>
          {tr("gui.move.keep_both_all", "Keep both and move all")}
        </button>
      </div>
    </div>
  {/if}

  <div class="recovery-ribbon">
    <div>
      <Icon name="shield-alert" size={17} />
      <strong>{tr("gui.recovery.status_not_checked", "Recovery status not checked")}</strong>
      <span>{tr("gui.recovery.open_to_protect_or_verify", "Open Recovery to create PAR2 data or verify existing recovery data.")}</span>
    </div>
    <button onclick={onOpenRecovery}>{tr("gui.recovery.open_recovery", "Open Recovery")}</button>
  </div>

  {#if view.structureWarning}
    <ArchiveStructureWarning
      message={view.structureWarning}
      actionLabel={tr("gui.archive.open_zip_repair", "Open ZIP repair")}
      onRepair={onOpenRecovery}
    />
  {/if}

  {#if view.encodingWarning}
    <div class="warning-ribbon">
      <Icon name="alert-triangle" size={17} />
      <span>{view.encodingWarning}</span>
      <button onclick={onRepairEncoding}>{tr("gui.encoding.repair_with_gbk", "Repair with GBK")}</button>
    </div>
  {/if}
</div>

<div class="modern-list" data-total-rows={view.totalRows}>
  <div class="archive-pathline archive-list-navigation">
    <div class="archive-path-actions" aria-label={tr("gui.nav.archive_navigation", "Archive navigation")}>
      <button
        type="button"
        disabled={!view.archive.canGoUp}
        title={tr("gui.nav.up", "Up one level")}
        onclick={onGoUp}
      ><Icon name="chevron-up" size={14} />{tr("gui.nav.up_short", "Up")}</button>
      <button
        type="button"
        disabled={view.archive.dirs.length === 0}
        title={tr("gui.nav.root", "Archive root")}
        onclick={onOpenRoot}
      ><Icon name="archive" size={14} />{tr("gui.nav.root_short", "Root")}</button>
    </div>
    <div bind:this={breadcrumbTrail} class="archive-breadcrumbs" aria-label={tr("gui.nav.archive_breadcrumbs", "Archive breadcrumbs")}>
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
        aria-label={tr("gui.list.search_aria", "Search paths across the entire archive")}
        aria-busy={view.filterPending}
        title={tr("gui.list.search_shortcut", "Search the entire archive (⌘F / Ctrl+F)")}
        placeholder={tr("gui.list.search_placeholder", "Search the entire archive")}
        oninput={(event) => onSearchInput(event.currentTarget.value)}
        onkeydown={onSearchKeydown}
      />
      {#if view.filterText}
        <button
          type="button"
          aria-label={tr("gui.list.search_clear", "Clear search")}
          title={tr("gui.list.search_clear", "Clear search")}
          onclick={onClearSearch}
        ><Icon name="x-circle" size={14} /></button>
      {/if}
    </div>
    <span role="status" aria-live="polite">{view.filterStatus}</span>
  </div>
  <div
    class="modern-table"
    role="table"
    aria-label={tr("gui.table.archive", "Archive table")}
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
        <span>{tr("gui.list.col.name", "Name")}</span>
      </span>
      <span role="columnheader">{tr("gui.list.col.size", "Size")}</span>
      <span role="columnheader">{tr("gui.list.col.packed", "Packed")}</span>
      <span role="columnheader">{tr("gui.list.col.modified", "Modified")}</span>
    </div>
    <div
      class="virtual-scroll modern-virtual-scroll"
      role="rowgroup"
      data-virtual-list="modern"
      onscroll={onBrowseScroll}
    >
      <div class="virtual-pad" use:cssVariables={virtualPadVariables(view.paddingTop)}></div>
      {#each view.rows as entry}
        <div
          class="modern-row"
          class:selected={entry.selected}
          class:previewing={entry.previewing}
          role="row"
          aria-rowindex={(entry.virtualIndex ?? 0) + 2}
          aria-selected={entry.selected}
          tabindex="0"
          aria-keyshortcuts="Space Enter Backspace Meta+ArrowUp Alt+ArrowUp E"
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
                onToggleEntrySelection(entry);
              }}
            ></button>
            <span
              class="file-badge"
              class:type-folder={entry.type === "folder"}
              class:type-locked={entry.type === "locked"}
              class:type-warning={entry.type === "warning"}
            >
              {entry.type === "folder"
                ? "DIR"
                : entry.type === "pdf"
                  ? "PDF"
                  : entry.type === "sheet"
                    ? "XLS"
                    : entry.type === "locked"
                      ? "AES"
                      : entry.type === "warning"
                        ? "TXT"
                        : "FILE"}
            </span>
            <span class="archive-entry-label">
              <strong>{entry.name}</strong>
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
        <div class="modern-row empty-row" role="row" aria-rowindex="2">
          <div class="file-name" role="cell"><strong>{view.emptyLabel}</strong></div>
          <span role="cell">{view.emptyLabel}</span><span role="cell">-</span><span role="cell">-</span>
        </div>
      {/each}
      <div class="virtual-pad" use:cssVariables={virtualPadVariables(view.paddingBottom)}></div>
    </div>
  </div>
</div>
