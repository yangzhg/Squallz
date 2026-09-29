<script lang="ts" module>
  import type { EntryDto, EntryPreviewDto } from "../lib/ipc";

  type Translate = (key: string, fallback: string) => string;

  type NestedPreviewView = {
    title: string;
    subtitle: string;
    rows: EntryDto[];
  };

  type EntryPreviewView = {
    policyKind: string;
    policyCode: string;
    nested: NestedPreviewView | null;
    title: string;
    subtitle: string;
    busy: boolean;
    failed: boolean;
    entry: EntryPreviewDto | null;
    canPreview: boolean;
    actionLabel: string;
    actionIcon: "external-link" | "folder-open" | "eye";
    disabledReason: string;
  };

  type ArchiveSummaryView = {
    format: string;
    entries: number;
    encoding: string;
    volumes: string;
  };

  export type ModernInspectorView =
    | {
        kind: "recovery";
        tone: string;
        title: string;
        detail: string;
        metricsAvailable: boolean;
        explanation: string;
      }
    | {
        kind: "archive";
        preview: EntryPreviewView;
        canRename: boolean;
        canMove: boolean;
        canTest: boolean;
        canCopyOut: boolean;
        archive: ArchiveSummaryView | null;
        openArchiveFirst: string;
        archiveActionDisabledReason: string;
        selectionSummary: string;
        copyOutDisabledReason: string;
      };

  export interface ModernInspectorProps {
    view: ModernInspectorView;
    tr: Translate;
    onOpenNestedPreview: () => void;
    onExtractNestedPreview: () => void;
    onClearPreview: (restoreEntryFocus?: boolean) => void;
    onRetryPreview: () => void;
    onExtractPreviewFailure: () => void;
    onOpenPreview: () => void;
    onRevealPreview: () => void;
    onPreviewSelection: () => void;
    onRenameSelection: () => void;
    onMoveSelection: () => void;
    onOpenRecovery: () => void;
    onTestArchive: () => void;
    onCopyOutSelection: () => void;
  }
</script>

<script lang="ts">
  import Icon from "./Icon.svelte";
  import { formatBytes } from "../lib/format";

  let {
    view,
    tr,
    onOpenNestedPreview,
    onExtractNestedPreview,
    onClearPreview,
    onRetryPreview,
    onExtractPreviewFailure,
    onOpenPreview,
    onRevealPreview,
    onPreviewSelection,
    onRenameSelection,
    onMoveSelection,
    onOpenRecovery,
    onTestArchive,
    onCopyOutSelection,
  }: ModernInspectorProps = $props();

  let previewActive = $derived(
    view.kind === "archive" &&
      Boolean(
        view.preview.nested ||
          view.preview.busy ||
          view.preview.failed ||
          view.preview.entry,
      ),
  );
</script>

{#if view.kind === "recovery"}
  <div class="inspector-block">
    <span class="block-label">{tr("gui.recovery.repair_math", "Repair math")}</span>
    <div class={`health-score recovery-health-score tone-${view.tone}`}>
      <strong>{view.title}</strong>
      <span>{view.metricsAvailable ? view.detail : tr("gui.recovery.capacity_not_reported", "Capacity not reported")}</span>
    </div>
    <p>{view.explanation}</p>
  </div>
  <div class="inspector-block">
    <span class="block-label">{tr("gui.recovery.compatibility", "Compatibility")}</span>
    <dl>
      <div><dt>PAR2</dt><dd>{tr("gui.recovery.standard", "Standard")}</dd></div>
      <div><dt>SQZ</dt><dd>Squallz</dd></div>
      <div><dt>{tr("common.export", "Export")}</dt><dd>7Z/ZIP</dd></div>
      <div><dt>RAR</dt><dd>{tr("gui.format.no_create", "No create")}</dd></div>
    </dl>
  </div>
{:else}
  <div
    class="inspector-block nested-preview-block entry-preview-panel"
    class:preview-sheet-active={previewActive}
    data-preview-policy={view.preview.policyKind}
    data-preview-code={view.preview.policyCode}
    role="region"
    aria-label={tr("gui.preview.panel", "Entry actions")}
  >
    <div class="preview-panel-heading">
      <span class="block-label">{tr("gui.preview.panel", "Entry actions")}</span>
      {#if previewActive}
        <button
          type="button"
          class="preview-panel-close"
          aria-label={tr("gui.preview.close", "Close item actions")}
          title={tr("gui.preview.close", "Close item actions")}
          onclick={(event) => onClearPreview(event.detail === 0)}
        ><Icon name="x" size={14} /></button>
      {/if}
    </div>
    <div class="entry-preview-body">
      {#if view.preview.nested}
        <strong>{view.preview.nested.title}</strong>
        <p>{view.preview.nested.subtitle}</p>
        <div class="nested-preview-list">
          {#each view.preview.nested.rows as item}
            <div>
              {#if item.entry_type === "symlink" || item.entry_type === "hardlink"}
                {@const label = (item.entry_type === "symlink" ? tr("gui.attr.symlink", "Symbolic link") : tr("gui.attr.hardlink", "Hard link")) + (item.encrypted ? ` · ${tr("gui.attr.encrypted", "Encrypted")}` : "")}
                <span role="img" aria-label={label} title={label}><Icon name={item.encrypted ? "lock" : "link"} size={14} /></span>
              {:else}
                <span>{item.entry_type === "dir" ? "DIR" : "FILE"}</span>
              {/if}
              <strong>{item.display}</strong>
              <small>{formatBytes(item.size)}</small>
            </div>
          {/each}
        </div>
      {:else if view.preview.busy}
        <div class="preview-loading" role="status" aria-live="polite">
          <span>{tr("gui.preview.loading", "Preparing item")}</span>
          <small>{view.preview.subtitle}</small>
        </div>
      {:else}
        <strong>{view.preview.title}</strong>
        <p>{view.preview.subtitle}</p>
      {/if}
    </div>
    {#if view.preview.nested || !view.preview.busy}
      <div class="inline-actions entry-preview-actions">
        {#if view.preview.nested}
          <button onclick={onOpenNestedPreview}><Icon name="folder-open" size={14} />{tr("gui.action.open_nested", "Open")}</button>
          <button onclick={onExtractNestedPreview}><Icon name="archive" size={14} />{tr("gui.action.extract_nested", "Extract")}</button>
        {:else if view.preview.failed}
          <button onclick={onRetryPreview}><Icon name="rotate-cw" size={14} />{tr("gui.preview.retry", "Retry")}</button>
          <button onclick={onExtractPreviewFailure}><Icon name="archive" size={14} />{tr("gui.preview.extract_instead", "Extract instead")}</button>
        {:else if view.preview.entry}
          <button class="preview-system-action" onclick={onOpenPreview}><Icon name="external-link" size={14} />{tr("gui.action.open_preview", "Open")}</button>
          <button onclick={onRevealPreview}><Icon name="folder-open" size={14} />{tr("gui.toast.reveal", "Reveal")}</button>
        {:else}
          <button
            disabled={!view.preview.canPreview}
            aria-busy={view.preview.busy}
            title={view.preview.disabledReason}
            aria-label={view.preview.disabledReason ? `${view.preview.actionLabel} — ${view.preview.disabledReason}` : view.preview.actionLabel}
            onclick={onPreviewSelection}
          ><Icon name={view.preview.actionIcon} size={14} />{view.preview.actionLabel}</button>
        {/if}
      </div>
    {/if}
  </div>

  {#if view.canRename || view.canMove}
    <div class="inspector-block">
      <div class="inline-actions">
        <button disabled={!view.canRename} onclick={onRenameSelection}>{tr("gui.action.rename_selected", "Rename selected")}</button>
        <button disabled={!view.canMove} onclick={onMoveSelection}>{tr("gui.action.move_selected", "Move selected")}</button>
      </div>
    </div>
  {/if}

  <div class="inspector-block">
    <span class="block-label">{tr("gui.inspector.archive", "Archive")}</span>
    <dl>
      <div><dt>{tr("gui.archive.format", "Format")}</dt><dd>{view.archive?.format ?? tr("common.none", "None")}</dd></div>
      <div><dt>{tr("gui.table.entries", "Entries")}</dt><dd>{view.archive?.entries.toLocaleString() ?? "0"}</dd></div>
      <div><dt>{tr("gui.archive.encoding", "Encoding")}</dt><dd>{view.archive?.encoding ?? tr("gui.archive.open_first", "Open first")}</dd></div>
      <div><dt>{tr("gui.archive.volumes", "Volumes")}</dt><dd>{view.archive?.volumes ?? "-"}</dd></div>
    </dl>
  </div>

  <div class="inspector-block recovery-inspector">
    <span class="block-label">{tr("gui.inspector.recovery", "Recovery")}</span>
    <strong>{view.archive ? tr("gui.recovery.status_not_checked", "Recovery status not checked") : view.openArchiveFirst}</strong>
    <p>{view.archive ? tr("gui.recovery.requires_recovery_data", "Verify can detect corruption, but repair requires PAR2 or SQZ recovery data created earlier.") : view.openArchiveFirst}</p>
    <div class="inline-actions">
      <button
        disabled={!view.archive}
        title={view.archiveActionDisabledReason}
        aria-label={view.archiveActionDisabledReason ? `${tr("gui.action.protect", "Protect")} — ${view.archiveActionDisabledReason}` : tr("gui.action.protect", "Protect")}
        onclick={onOpenRecovery}
      >{tr("gui.action.protect", "Protect")}</button>
      <button
        disabled={!view.canTest}
        title={view.archiveActionDisabledReason}
        aria-label={view.archiveActionDisabledReason ? `${tr("gui.action.test_archive", "Test archive")} — ${view.archiveActionDisabledReason}` : tr("gui.action.test_archive", "Test archive")}
        onclick={onTestArchive}
      >{tr("gui.action.test_archive", "Test archive")}</button>
    </div>
  </div>

  <div class="inspector-block">
    <span class="block-label">{tr("gui.inspector.selection", "Selection")}</span>
    <strong>{view.selectionSummary}</strong>
    <p>{view.archive ? tr("gui.selection.actions_hint", "Extract, open, or copy the selected files without leaving the archive.") : view.openArchiveFirst}</p>
    <div class="inline-actions">
      <button
        disabled={!view.preview.canPreview}
        aria-busy={view.preview.busy}
        title={view.preview.disabledReason}
        aria-label={view.preview.disabledReason ? `${view.preview.actionLabel} — ${view.preview.disabledReason}` : view.preview.actionLabel}
        onclick={onPreviewSelection}
      >{view.preview.actionLabel}</button>
      <button
        disabled={!view.canCopyOut}
        title={view.copyOutDisabledReason}
        aria-label={view.copyOutDisabledReason ? `${tr("gui.action.copy_out", "Copy out")} — ${view.copyOutDisabledReason}` : tr("gui.action.copy_out", "Copy out")}
        onclick={onCopyOutSelection}
      >{tr("gui.action.copy_out", "Copy out")}</button>
    </div>
  </div>
{/if}
