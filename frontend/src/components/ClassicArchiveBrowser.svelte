<script lang="ts" module>
  import type { EntryDto, EntryPreviewDto } from "../lib/ipc";

  type Translate = (key: string, fallback: string) => string;

  export type ClassicBrowserEntry = {
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
    to: string;
    keepBothTo: string | null;
  };

  export interface ClassicArchiveBrowserProps {
    view: {
      archiveTitle: string;
      archiveFormatSummary: string;
      archiveOpen: boolean;
      archiveReadOnly: boolean;
      openArchiveFirst: string;
      selection: {
        checked: boolean;
        mixed: boolean;
        disabled: boolean;
        label: string;
        busy: boolean;
        busyLabel: string;
      };
      preview: {
        policyKind: string;
        policyCode: string;
        nestedTitle: string | null;
        nestedSubtitle: string;
        nestedRows: readonly string[];
        title: string;
        subtitle: string;
        busy: boolean;
        entry: EntryPreviewDto | null;
        failed: boolean;
        canPreview: boolean;
        actionLabel: string;
        actionIcon: "external-link" | "folder-open" | "eye";
        disabledReason: string;
        ariaLabel: string;
      };
      rename: {
        visible: boolean;
        value: string;
        status: string;
      };
      move: {
        visible: boolean;
        value: string;
        status: string;
        disabledReason: string;
      };
      newFolder: {
        value: string;
        status: string;
      };
      workbenchVisible: boolean;
      selectedSummary: string;
      conflict: {
        count: number;
        readyCount: number;
        targetDir: string;
        items: readonly MoveConflictItem[];
      } | null;
      structureWarning: string | null;
      totalRows: number;
      rows: readonly ClassicBrowserEntry[];
      paddingTop: number;
      paddingBottom: number;
      emptyName: string;
      emptyStatus: string;
    };
    tr: Translate;
    onOpenRoot: () => void;
    onOpenRecovery: () => void;
    onOpenNestedPreview: () => void;
    onExtractNestedPreview: () => void;
    onClearPreview: (restoreEntryFocus?: boolean) => void;
    onRetryPreview: () => void;
    onExtractPreviewFailure: () => void;
    onOpenPreview: () => void;
    onRevealPreview: () => void;
    onPreviewSelection: () => void;
    onRenameTargetChange: (value: string) => void;
    onCommitRenameTarget: () => void;
    onMoveTargetChange: (value: string) => void;
    onCommitMoveTarget: () => void;
    onNewFolderChange: (value: string) => void;
    onCommitNewFolder: () => void;
    onCancelMoveConflict: () => void;
    onSubmitMoveReadyOnly: () => void;
    onSubmitMoveKeepBoth: () => void;
    onBrowseScroll: (event: Event) => void;
    onSelectEntry: (entry: ClassicBrowserEntry, event: MouseEvent | KeyboardEvent) => void;
    onActivateEntry: (entry: ClassicBrowserEntry) => void;
    onEntryKeydown: (event: KeyboardEvent, entry: ClassicBrowserEntry) => void;
    onOpenEntryContext: (event: MouseEvent, entry: ClassicBrowserEntry) => void;
    onToggleEntrySelection: (entry: ClassicBrowserEntry) => void;
    onToggleAllEntries: () => void;
    onPreviewEntry: (entry: ClassicBrowserEntry) => void;
  }
</script>

<script lang="ts">
  import Icon from "./Icon.svelte";
  import ArchiveStructureWarning from "./ArchiveStructureWarning.svelte";
  import { cssVariables } from "../lib/css-variables";

  let {
    view,
    tr,
    onOpenRoot,
    onOpenRecovery,
    onOpenNestedPreview,
    onExtractNestedPreview,
    onClearPreview,
    onRetryPreview,
    onExtractPreviewFailure,
    onOpenPreview,
    onRevealPreview,
    onPreviewSelection,
    onRenameTargetChange,
    onCommitRenameTarget,
    onMoveTargetChange,
    onCommitMoveTarget,
    onNewFolderChange,
    onCommitNewFolder,
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

  let previewActive = $derived(
    Boolean(
      view.preview.nestedTitle ||
        view.preview.busy ||
        view.preview.failed ||
        view.preview.entry,
    ),
  );
</script>

<div class="classic-body">
  <aside class="classic-tree" aria-label={tr("gui.aria.archive_folders", "Archive folders")}>
    <button
      type="button"
      class="classic-tree-item active"
      disabled={!view.archiveOpen}
      title={tr("gui.nav.root", "Archive root")}
      onclick={onOpenRoot}
    ><Icon name="archive" size={15} />{view.archiveTitle}</button>
    {#if view.archiveOpen}
      <button
        type="button"
        class="classic-tree-item"
        onclick={onOpenRoot}
      ><Icon name="folder" size={15} />{tr("gui.list.archive_root", "Archive root")}</button>
    {:else}
      <div class="classic-tree-item muted" title={view.openArchiveFirst} aria-label={view.openArchiveFirst}>
        <Icon name="folder-open" size={15} />{view.openArchiveFirst}
      </div>
    {/if}
    <div class="tree-note">
      <strong>{tr("gui.archive.format", "Format")}</strong>
      <span>{view.archiveFormatSummary}</span>
    </div>
    <div
      class="tree-note nested-tree-note"
      class:preview-sheet-active={previewActive}
      data-preview-policy={view.preview.policyKind}
      data-preview-code={view.preview.policyCode}
      role="region"
      aria-label={tr("gui.preview.panel", "Entry actions")}
    >
      <div class="preview-panel-heading">
        <strong>{tr("gui.preview.panel", "Entry actions")}</strong>
        {#if previewActive}
          <button
            type="button"
            class="preview-panel-close"
            aria-label={tr("gui.preview.close", "Close item actions")}
            title={tr("gui.preview.close", "Close item actions")}
            onclick={(event) => onClearPreview(event.detail === 0)}
          ><Icon name="x" size={13} /></button>
        {/if}
      </div>
      <span>{view.preview.nestedTitle ?? view.preview.title}</span>
      <small>{view.preview.nestedTitle ? view.preview.nestedSubtitle : view.preview.subtitle}</small>
      {#if view.preview.nestedTitle}
        {#each view.preview.nestedRows as item}
          <em>{item}</em>
        {/each}
        <button onclick={onOpenNestedPreview}>
          <Icon name="folder-open" size={13} />{tr("gui.action.open_nested", "Open")}
        </button>
        <button onclick={onExtractNestedPreview}>
          <Icon name="archive" size={13} />{tr("gui.action.extract_nested", "Extract")}
        </button>
      {:else if view.preview.busy}
        <div class="preview-loading compact" role="status" aria-live="polite">
          <span>{tr("gui.preview.loading", "Preparing item")}</span>
          <small>{view.preview.subtitle}</small>
        </div>
      {:else if view.preview.failed}
        <button onclick={onRetryPreview}>
          <Icon name="rotate-cw" size={13} />{tr("gui.preview.retry", "Retry")}
        </button>
        <button onclick={onExtractPreviewFailure}>
          <Icon name="archive" size={13} />{tr("gui.preview.extract_instead", "Extract instead")}
        </button>
      {:else if view.preview.entry}
        <button class="preview-system-action" onclick={onOpenPreview}>
          <Icon name="external-link" size={13} />{tr("gui.action.open_preview", "Open")}
        </button>
        <button onclick={onRevealPreview}>
          <Icon name="folder-open" size={13} />{tr("gui.toast.reveal", "Reveal")}
        </button>
      {:else}
        <button
          disabled={!view.preview.canPreview}
          aria-busy={view.preview.busy}
          title={view.preview.disabledReason}
          aria-label={view.preview.ariaLabel}
          onclick={onPreviewSelection}
        ><Icon name={view.preview.actionIcon} size={13} />{view.preview.actionLabel}</button>
      {/if}
    </div>
    {#if view.rename.visible}
      <div class="tree-note move-tree-note">
        <strong>{tr("gui.action.rename_target", "Rename target")}</strong>
        <input
          class="classic-input"
          aria-label={tr("gui.rename.classic_target_name", "Classic rename target name")}
          value={view.rename.value}
          oninput={(event) => onRenameTargetChange(event.currentTarget.value)}
          onblur={() => onCommitRenameTarget()}
        />
        <small>{view.rename.status}</small>
      </div>
    {/if}
    {#if view.move.visible}
      <div class="tree-note move-tree-note">
        <strong>{tr("gui.action.move_target", "Move target")}</strong>
        <input
          class="classic-input"
          aria-label={tr("gui.move.classic_target_folder", "Classic move target folder")}
          value={view.move.value}
          disabled={view.archiveReadOnly}
          title={view.move.disabledReason}
          oninput={(event) => onMoveTargetChange(event.currentTarget.value)}
          onblur={() => onCommitMoveTarget()}
        />
        <small>{view.move.status}</small>
      </div>
    {/if}
  </aside>

  <section class="classic-table-wrap">
    <div class="classic-table-header">
      {#if view.structureWarning}
        <ArchiveStructureWarning
          message={view.structureWarning}
          actionLabel={tr("gui.archive.open_zip_repair", "Open ZIP repair")}
          onRepair={onOpenRecovery}
        />
      {/if}
      {#if view.workbenchVisible}
        <div class="classic-workbench-strip">
          <label>
            <span>{tr("gui.action.rename_to", "Rename to")}</span>
            <input
              aria-label={tr("gui.rename.classic_table_target_name", "Classic table rename target name")}
              value={view.rename.value}
              disabled={!view.rename.visible}
              title={view.rename.visible ? "" : tr("gui.precondition.select_one_file", "Select exactly one file")}
              oninput={(event) => onRenameTargetChange(event.currentTarget.value)}
              onblur={() => onCommitRenameTarget()}
            />
          </label>
          <label>
            <span>{tr("gui.action.move_to", "Move to")}</span>
            <input
              aria-label={tr("gui.move.classic_table_target_folder", "Classic table move target folder")}
              value={view.move.value}
              disabled={view.archiveReadOnly}
              title={view.move.disabledReason}
              oninput={(event) => onMoveTargetChange(event.currentTarget.value)}
              onblur={() => onCommitMoveTarget()}
            />
          </label>
          <label>
            <span>{tr("gui.action.new_folder", "New folder")}</span>
            <input
              aria-label={tr("gui.new_folder.classic_name", "Classic new folder name")}
              value={view.newFolder.value}
              disabled={view.archiveReadOnly}
              title={view.move.disabledReason}
              oninput={(event) => onNewFolderChange(event.currentTarget.value)}
              onblur={() => onCommitNewFolder()}
            />
          </label>
          <small>{view.rename.status} · {view.move.status} · {view.newFolder.status}</small>
        </div>
      {:else}
        <div class="classic-workbench-strip empty-workbench-strip">
          <span>{view.archiveOpen ? view.selectedSummary : view.openArchiveFirst}</span>
          <small>
            {view.archiveOpen
              ? tr("gui.preview.keyboard_hint", "Space or Return opens the focused item")
              : tr("gui.classic.empty_workbench_hint", "Archive editing controls appear after an archive is open.")}
          </small>
        </div>
      {/if}
      {#if view.conflict}
        <div
          class="classic-move-conflict-review"
          role="dialog"
          aria-label={tr("gui.move.conflicts", "Move target conflicts")}
          tabindex="-1"
        >
          <header>
            <strong>
              {tr("gui.move.conflict_count", "{count} move conflicts").replace("{count}", String(view.conflict.count))}
            </strong>
            <span>
              {tr("gui.move.ready_target", "{count} ready · target {target}")
                .replace("{count}", String(view.conflict.readyCount))
                .replace("{target}", view.conflict.targetDir)}
            </span>
          </header>
          <div class="classic-move-conflict-table">
            <div>
              <b>{tr("common.source", "Source")}</b>
              <b>{tr("gui.move.existing_target", "Existing target")}</b>
              <b>{tr("gui.move.keep_both_target", "Keep both target")}</b>
            </div>
            {#each view.conflict.items as item}
              <div><strong>{item.from}</strong><span>{item.to}</span><em>{item.keepBothTo}</em></div>
            {/each}
          </div>
          <div class="classic-button-row compact-row">
            <button onclick={onCancelMoveConflict}>{tr("gui.common.cancel", "Cancel")}</button>
            <button disabled={view.conflict.readyCount === 0} onclick={onSubmitMoveReadyOnly}>
              {tr("gui.move.ready_only", "Move ready only")}
            </button>
            <button class="classic-primary" onclick={onSubmitMoveKeepBoth}>
              {tr("gui.move.keep_both_all", "Keep both and move all")}
            </button>
          </div>
        </div>
      {/if}
    </div>
    <div
      class="classic-table"
      role="table"
      aria-label={tr("gui.table.archive", "Archive table")}
      aria-rowcount={Math.max(view.totalRows + 1, 2)}
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
          <span>{tr("gui.table.name", "Name")}</span>
        </span>
        <span role="columnheader">{tr("gui.table.size", "Size")}</span>
        <span role="columnheader">{tr("gui.table.packed", "Packed")}</span>
        <span role="columnheader">{tr("gui.table.ratio", "Ratio")}</span>
        <span role="columnheader">{tr("gui.table.modified", "Modified")}</span>
        <span role="columnheader">{tr("gui.table.crc", "CRC")}</span>
        <span role="columnheader">{tr("gui.table.method", "Method")}</span>
        <span role="columnheader">{tr("gui.table.attr", "Attr")}</span>
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
        {#each view.rows as entry}
          <div
            class:selected={entry.selected}
            class:previewing={entry.previewing}
            class="classic-row"
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
            <span class="table-name" role="cell">
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
              <span class="archive-entry-label">
                <strong>{entry.name}</strong>
                {#if entry.location}<small title={entry.source?.path}>{entry.location}</small>{/if}
              </span>
              {#if entry.source}
                <button
                  class="row-preview-button compact"
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
            <span role="cell">{entry.ratio}</span>
            <span role="cell">{entry.modified}</span>
            <span role="cell">{entry.crc}</span>
            <span role="cell">{entry.method}</span>
            <span role="cell">{entry.attr}</span>
          </div>
        {:else}
          <div class="classic-row empty-row" role="row" aria-rowindex="2">
            <span class="table-name" role="cell">{view.emptyName}</span>
            <span role="cell">{view.emptyStatus}</span>
            <span role="cell">-</span>
            <span role="cell">-</span>
            <span role="cell">-</span>
            <span role="cell">-</span>
            <span role="cell">-</span>
            <span role="cell">-</span>
          </div>
        {/each}
        <div
          class="virtual-pad"
          use:cssVariables={{ "--virtual-pad-height": `${view.paddingBottom}px` }}
        ></div>
      </div>
    </div>
  </section>
</div>
