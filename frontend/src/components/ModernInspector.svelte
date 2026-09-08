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
        kind: "batch";
        ready: number;
        archives: number;
        percent: number;
      }
    | {
        kind: "password";
        secretStore: string;
      }
    | {
        kind: "conflict";
      }
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
        renameTarget: string;
        renameStatus: string;
        canMove: boolean;
        moveTarget: string;
        normalizedMoveTarget: string;
        moveTargetPresets: readonly string[];
        moveStatus: string;
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
    onRenameTargetChange: (value: string) => void;
    onCommitRenameTarget: () => void;
    onMoveTargetChange: (value: string) => void;
    onCommitMoveTarget: (target?: string) => void;
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
    onRenameTargetChange,
    onCommitRenameTarget,
    onMoveTargetChange,
    onCommitMoveTarget,
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

{#if view.kind === "batch"}
  <div class="inspector-block">
    <span class="block-label">{tr("gui.batch.readiness", "Batch readiness")}</span>
    <div class="health-score"><strong>{view.ready} / {view.archives}</strong><span>{tr("gui.state.ready", "Ready")}</span></div>
    <progress
      class="meter meter-progress"
      value={view.percent}
      max="100"
      aria-label={tr("gui.batch.readiness", "Batch readiness")}
    ></progress>
    <p>{view.archives === 0 ? tr("gui.archive.open_first", "Open an archive first") : tr("gui.batch.ready_continue_hint", "Ready archives can continue without global failure.")}</p>
  </div>
  <div class="inspector-block">
    <span class="block-label">{tr("gui.batch.policy", "Batch policy")}</span>
    <dl>
      <div><dt>{tr("gui.batch.targets", "Targets")}</dt><dd>{tr("gui.batch.per_archive", "Per archive")}</dd></div>
      <div><dt>{tr("gui.extract.conflicts", "Conflicts")}</dt><dd>{tr("gui.extract.overwrite.ask", "Ask")}</dd></div>
      <div><dt>{tr("gui.batch.passwords", "Passwords")}</dt><dd>{tr("gui.batch.per_archive", "Per archive")}</dd></div>
      <div><dt>RAR</dt><dd>{tr("gui.format.extract_only", "Extract only")}</dd></div>
    </dl>
  </div>
{:else if view.kind === "password"}
  <div class="inspector-block">
    <span class="block-label">{tr("gui.password.boundary", "Password boundary")}</span>
    <strong>{tr("gui.password.no_plaintext_persistence", "No plaintext persistence")}</strong>
    <p>{tr("gui.password.saved_boundary_body", "Saved passwords stay behind the system secret-store boundary; task status and settings only show status.")}</p>
  </div>
  <div class="inspector-block">
    <span class="block-label">{tr("gui.password.fallback_order", "Fallback order")}</span>
    <dl>
      <div><dt>{tr("gui.password.manual", "Manual")}</dt><dd>{tr("gui.priority.first", "First")}</dd></div>
      <div><dt>{tr("gui.password.session", "Session")}</dt><dd>{tr("gui.priority.second", "Second")}</dd></div>
      <div><dt>{view.secretStore}</dt><dd>{tr("gui.priority.third", "Third")}</dd></div>
      <div><dt>{tr("gui.password.logs", "Logs")}</dt><dd>{tr("gui.priority.never", "Never")}</dd></div>
    </dl>
  </div>
{:else if view.kind === "conflict"}
  <div class="inspector-block">
    <span class="block-label">{tr("gui.extract.conflict_policy", "Conflict policy")}</span>
    <div class="health-score"><strong>3</strong><span>{tr("gui.conflict.items", "items")}</span></div>
    <p>{tr("gui.conflict.policy_body", "Decisions can apply per file or to the remaining conflict set; default is never silent overwrite.")}</p>
  </div>
  <div class="inspector-block">
    <span class="block-label">{tr("gui.conflict.available_actions", "Available actions")}</span>
    <dl>
      <div><dt>{tr("gui.conflict.overwrite", "Overwrite")}</dt><dd>{tr("gui.conflict.explicit", "Explicit")}</dd></div>
      <div><dt>{tr("gui.conflict.skip", "Skip")}</dt><dd>{tr("gui.conflict.safe", "Safe")}</dd></div>
      <div><dt>{tr("gui.conflict.rename", "Keep Both")}</dt><dd>{tr("gui.conflict.renames", "Renames")}</dd></div>
      <div><dt>{tr("gui.conflict.compare", "Compare")}</dt><dd>{tr("gui.conflict.metadata", "Metadata")}</dd></div>
    </dl>
  </div>
{:else if view.kind === "recovery"}
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
    class="inspector-block nested-preview-block"
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
    {#if view.preview.nested}
      <strong>{view.preview.nested.title}</strong>
      <p>{view.preview.nested.subtitle}</p>
      <div class="nested-preview-list">
        {#each view.preview.nested.rows as item}
          <div>
            <span>{item.entry_type === "dir" ? "DIR" : "FILE"}</span>
            <strong>{item.display}</strong>
            <small>{formatBytes(item.size)}</small>
          </div>
        {/each}
      </div>
      <div class="inline-actions">
        <button onclick={onOpenNestedPreview}><Icon name="folder-open" size={14} />{tr("gui.action.open_nested", "Open")}</button>
        <button onclick={onExtractNestedPreview}><Icon name="archive" size={14} />{tr("gui.action.extract_nested", "Extract")}</button>
      </div>
    {:else}
      <strong>{view.preview.title}</strong>
      <p>{view.preview.subtitle}</p>
      {#if view.preview.busy}
        <div class="preview-loading" role="status" aria-live="polite">
          <span>{tr("gui.preview.loading", "Preparing item")}</span>
          <small>{view.preview.subtitle}</small>
        </div>
      {:else if view.preview.failed}
        <div class="inline-actions">
          <button onclick={onRetryPreview}><Icon name="rotate-cw" size={14} />{tr("gui.preview.retry", "Retry")}</button>
          <button onclick={onExtractPreviewFailure}><Icon name="archive" size={14} />{tr("gui.preview.extract_instead", "Extract instead")}</button>
        </div>
      {:else if view.preview.entry}
        <div class="inline-actions">
          <button class="preview-system-action" onclick={onOpenPreview}><Icon name="external-link" size={14} />{tr("gui.action.open_preview", "Open")}</button>
          <button onclick={onRevealPreview}><Icon name="folder-open" size={14} />{tr("gui.toast.reveal", "Reveal")}</button>
        </div>
      {:else}
        <div class="inline-actions">
          <button
            disabled={!view.preview.canPreview}
            aria-busy={view.preview.busy}
            title={view.preview.disabledReason}
            aria-label={view.preview.disabledReason ? `${view.preview.actionLabel} — ${view.preview.disabledReason}` : view.preview.actionLabel}
            onclick={onPreviewSelection}
          ><Icon name={view.preview.actionIcon} size={14} />{view.preview.actionLabel}</button>
        </div>
      {/if}
    {/if}
  </div>

  {#if view.canRename}
    <div class="inspector-block move-target-block">
      <span class="block-label">{tr("gui.action.rename_target", "Rename target")}</span>
      <input
        class="move-target-input"
        aria-label={tr("gui.rename.target_name", "Rename target name")}
        value={view.renameTarget}
        oninput={(event) => onRenameTargetChange(event.currentTarget.value)}
        onblur={() => onCommitRenameTarget()}
      />
      <p>{view.renameStatus}</p>
    </div>
  {/if}

  {#if view.canMove}
    <div class="inspector-block move-target-block">
      <span class="block-label">{tr("gui.action.move_target", "Move target")}</span>
      <input
        class="move-target-input"
        aria-label={tr("gui.move.target_folder", "Move target folder")}
        value={view.moveTarget}
        oninput={(event) => onMoveTargetChange(event.currentTarget.value)}
        onblur={() => onCommitMoveTarget()}
      />
      <div class="move-target-presets" aria-label={tr("gui.move.target_presets", "Move target presets")}>
        {#each view.moveTargetPresets as target}
          <button class:active={view.normalizedMoveTarget === target} onclick={() => onCommitMoveTarget(target)}>{target}</button>
        {/each}
      </div>
      <p>{view.moveStatus}</p>
    </div>
  {/if}

  <div class="inspector-block">
    <span class="block-label">{tr("gui.inspector.health", "Health")}</span>
    <div class="health-score">
      <strong>{view.archive ? tr("gui.state.ready", "Ready") : tr("gui.state.idle", "Idle")}</strong>
      <span>{view.archive ? tr("gui.archive.zip_slip_guard_on", "Zip Slip guard on") : view.openArchiveFirst}</span>
    </div>
    <progress
      class="meter meter-progress"
      value={view.archive ? 84 : 0}
      max="100"
      aria-label={tr("gui.inspector.health", "Health")}
    ></progress>
  </div>

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
        disabled={!view.archive}
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
        disabled={Boolean(view.copyOutDisabledReason)}
        title={view.copyOutDisabledReason}
        aria-label={view.copyOutDisabledReason ? `${tr("gui.action.copy_out", "Copy out")} — ${view.copyOutDisabledReason}` : tr("gui.action.copy_out", "Copy out")}
        onclick={onCopyOutSelection}
      >{tr("gui.action.copy_out", "Copy out")}</button>
    </div>
  </div>
{/if}
