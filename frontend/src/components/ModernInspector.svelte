<script lang="ts" module>
  import type { ArchivePreviewActions, ArchivePreviewView } from "../lib/archive-browser";

  type Translate = (key: string, fallback?: string) => string;

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
        preview: ArchivePreviewView;
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

  export interface ModernInspectorProps extends ArchivePreviewActions {
    view: ModernInspectorView;
    tr: Translate;
    onRenameSelection: () => void;
    onMoveSelection: () => void;
    onOpenRecovery: () => void;
    onTestArchive: () => void;
    onCopyOutSelection: () => void;
  }
</script>

<script lang="ts">
  import ArchiveEntryPreview from "./ArchiveEntryPreview.svelte";

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

</script>

{#if view.kind === "recovery"}
  <div class="inspector-block">
    <span class="block-label">{tr("gui.recovery.repair_math")}</span>
    <div class={`health-score recovery-health-score tone-${view.tone}`}>
      <strong>{view.title}</strong>
      <span>{view.metricsAvailable ? view.detail : tr("gui.recovery.capacity_not_reported")}</span>
    </div>
    <p>{view.explanation}</p>
  </div>
  <div class="inspector-block">
    <span class="block-label">{tr("gui.recovery.compatibility")}</span>
    <dl>
      <div><dt>PAR2</dt><dd>{tr("gui.recovery.standard")}</dd></div>
      <div><dt>SQZ</dt><dd>Squallz</dd></div>
      <div><dt>{tr("common.export")}</dt><dd>7Z/ZIP</dd></div>
      <div><dt>RAR</dt><dd>{tr("gui.format.no_create")}</dd></div>
    </dl>
  </div>
{:else}
  <ArchiveEntryPreview view={view.preview} {tr}
    actions={{ onOpenNestedPreview, onExtractNestedPreview, onClearPreview, onRetryPreview, onExtractPreviewFailure, onOpenPreview, onRevealPreview, onPreviewSelection }} />

  {#if view.canRename || view.canMove}
    <div class="inspector-block">
      <div class="inline-actions">
        <button disabled={!view.canRename} onclick={onRenameSelection}>{tr("gui.action.rename_selected")}</button>
        <button disabled={!view.canMove} onclick={onMoveSelection}>{tr("gui.action.move_selected")}</button>
      </div>
    </div>
  {/if}

  <div class="inspector-block">
    <span class="block-label">{tr("gui.inspector.archive")}</span>
    <dl>
      <div><dt>{tr("gui.archive.format")}</dt><dd>{view.archive?.format ?? tr("common.none")}</dd></div>
      <div><dt>{tr("gui.table.entries")}</dt><dd>{view.archive?.entries.toLocaleString() ?? "0"}</dd></div>
      <div><dt>{tr("gui.archive.encoding")}</dt><dd>{view.archive?.encoding ?? tr("gui.archive.open_first")}</dd></div>
      <div><dt>{tr("gui.archive.volumes")}</dt><dd>{view.archive?.volumes ?? "-"}</dd></div>
    </dl>
  </div>

  <div class="inspector-block recovery-inspector">
    <span class="block-label">{tr("gui.inspector.recovery")}</span>
    <strong>{view.archive ? tr("gui.recovery.status_not_checked") : view.openArchiveFirst}</strong>
    <p>{view.archive ? tr("gui.recovery.requires_recovery_data") : view.openArchiveFirst}</p>
    <div class="inline-actions">
      <button
        disabled={!view.archive}
        title={view.archiveActionDisabledReason}
        aria-label={view.archiveActionDisabledReason ? `${tr("gui.action.protect")} — ${view.archiveActionDisabledReason}` : tr("gui.action.protect")}
        onclick={onOpenRecovery}
      >{tr("gui.action.protect")}</button>
      <button
        disabled={!view.canTest}
        title={view.archiveActionDisabledReason}
        aria-label={view.archiveActionDisabledReason ? `${tr("gui.action.test_archive")} — ${view.archiveActionDisabledReason}` : tr("gui.action.test_archive")}
        onclick={onTestArchive}
      >{tr("gui.action.test_archive")}</button>
    </div>
  </div>

  <div class="inspector-block">
    <span class="block-label">{tr("gui.inspector.selection")}</span>
    <strong>{view.selectionSummary}</strong>
    <p>{view.archive ? tr("gui.selection.actions_hint") : view.openArchiveFirst}</p>
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
        aria-label={view.copyOutDisabledReason ? `${tr("gui.action.copy_out")} — ${view.copyOutDisabledReason}` : tr("gui.action.copy_out")}
        onclick={onCopyOutSelection}
      >{tr("gui.action.copy_out")}</button>
    </div>
  </div>
{/if}
