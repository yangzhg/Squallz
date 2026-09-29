<script lang="ts">
  import { onMount } from "svelte";
  import type { BatchWorkspaceSurface } from "./ToolsWorkspace.svelte";
  import type { OverwritePolicy, SymlinkPolicy } from "../lib/ipc";
  import ArchiveReturnStrip from "./ArchiveReturnStrip.svelte";
  import Icon from "./Icon.svelte";

  let { surface }: { surface: BatchWorkspaceSurface } = $props();
  onMount(() => surface.onReady());
</script>

<div class="operation-workspace" class:classic-operation-workspace={surface.variant === "classic"}>
  {#if surface.archiveReturn.visible}
    <ArchiveReturnStrip {...surface.archiveReturn} />
  {/if}
  <header class="operation-workspace-header">
    <div>
      <h1 id="batch-workspace-heading" tabindex="-1">{surface.title}</h1>
      <p>{surface.mode === "nested"
        ? surface.tr("gui.nested_extract.scope", "Extract all contents of the inner archive")
        : surface.tr("gui.batch.archive_count", "Archives: {count}").replace("{count}", surface.rows.length.toLocaleString())}</p>
    </div>
    <div class="operation-workspace-actions">
      {#if surface.actions.onAdd}
        <button class="secondary-lite" disabled={surface.locked} onclick={surface.actions.onAdd}>
          <Icon name="folder-open" size={16} />{surface.mode === "nested"
            ? surface.tr("gui.archive.back_to_current", "Back to current archive")
            : surface.tr("gui.batch.add_archives", "Add archives")}
        </button>
      {/if}
      <button class="primary" disabled={surface.locked || surface.rows.length === 0} onclick={surface.actions.onStart}>
        <Icon name="archive" size={16} />{surface.submitting
          ? surface.tr("gui.task_center.submitting", "Adding to the queue…")
          : surface.mode === "nested" ? surface.tr("gui.extract.start", "Extract")
          : surface.tr("gui.batch.start_batch", "Start batch")}
      </button>
    </div>
  </header>
  <div class="operation-workspace-body">
  {#if surface.rows.length === 0}
    <div class="operation-empty-state">
      <Icon name="archive" size={32} />
      <strong>{surface.mode === "nested" ? surface.tr("gui.nested_extract.empty", "No inner archive selected") : surface.tr("gui.batch.no_archives_queued", "No archives selected")}</strong>
      <p>{surface.mode === "nested" ? surface.tr("gui.nested_extract.empty_hint", "Select an archive inside the current archive, then choose Extract in its preview.") : surface.tr("gui.batch.empty_hint", "Add archives, review their destinations, then start extraction.")}</p>
    </div>
  {:else}
    <div class="operation-policy-controls">
      <label class="operation-toggle">
        <input type="checkbox" checked={surface.smart} disabled={surface.locked}
          onchange={(event) => surface.onSmartChange(event.currentTarget.checked)} />
        <span>{surface.tr("gui.batch.mode_smart", "Smart extract")}</span>
      </label>
      <label class="operation-field">
        <span>{surface.tr("gui.batch.overwrite", "Conflicts")}</span>
        <select class="operation-input" value={surface.overwrite} disabled={surface.locked}
          onchange={(event) => surface.onOverwriteChange(event.currentTarget.value as OverwritePolicy)}>
          {#each surface.overwriteChoices as choice}<option value={choice.id}>{choice.label}</option>{/each}
        </select>
      </label>
      <label class="operation-field">
        <span>{surface.tr("gui.extract.symlinks", "Symbolic links")}</span>
        <select class="operation-input" value={surface.symlinks} disabled={surface.locked}
          onchange={(event) => surface.onSymlinksChange(event.currentTarget.value as SymlinkPolicy)}>
          {#each surface.symlinkChoices as choice}<option value={choice.id}>{choice.label}</option>{/each}
        </select>
      </label>
    </div>
    <p class="operation-workspace-hint" id="batch-destination-hint">{surface.mode === "nested"
      ? surface.smart
        ? surface.tr("gui.nested_extract.smart_destination_hint", "Smart extract uses this destination as a base folder. A subfolder is added only when the inner archive contents need one.")
        : surface.tr("gui.nested_extract.direct_destination_hint", "All contents of the inner archive are extracted directly into this destination folder.")
      : surface.smart
      ? surface.tr("gui.batch.smart_destination_hint", "Smart extract uses each destination as a base folder. A subfolder is added only when the archive contents need one.")
      : surface.tr("gui.batch.direct_destination_hint", "Each archive is extracted directly into its destination folder.")}</p>
    <ol class="operation-list">
      {#each surface.rows as row, index (row.id)}
        <li class="operation-card">
          <div class="operation-card-header">
            <div class="operation-identity">
              <strong>{row.name}</strong>
              <span>{row.format} · {(surface.mode === "nested"
                ? surface.tr("gui.nested_extract.outer_encoding", "Outer archive encoding: {encoding}")
                : surface.tr("gui.batch.encoding", "Encoding: {encoding}")).replace("{encoding}", row.encoding)}</span>
              {#if row.bestEffort && !row.onBestEffortChange}<span>{surface.tr("gui.batch.best_effort", "Best-effort extraction enabled")}</span>{/if}
            </div>
            {#if row.onRemove}
              <button class="secondary-lite" id={`batch-remove-${index}`} disabled={surface.locked}
                aria-label={surface.tr("gui.batch.remove_archive", "Remove {name}").replace("{name}", row.name)} onclick={row.onRemove}>
                <Icon name="x" size={16} />{surface.tr("gui.batch.remove", "Remove")}
              </button>
            {/if}
          </div>
          <p class="operation-path">{row.path}</p>
          <label class="operation-destination-field" for={`batch-destination-${index}`}>
            {surface.tr("gui.batch.destination", "Destination")}
          </label>
          <div class="operation-control">
            <input class="operation-input" id={`batch-destination-${index}`} value={row.target}
              aria-label={surface.tr("gui.batch.destination_for", "Destination for {name}").replace("{name}", row.name)}
              aria-describedby="batch-destination-hint" disabled={surface.locked}
              oninput={(event) => row.onTargetInput(event.currentTarget.value)} />
            <button class="secondary-lite" disabled={surface.locked} onclick={row.onChooseTarget}
              aria-label={surface.tr("gui.batch.browse_destination_for", "Choose destination for {name}").replace("{name}", row.name)}>
              <Icon name="folder" size={16} />{surface.tr("gui.compress.browse", "Browse…")}
            </button>
          </div>
          {#if row.onBestEffortChange}
            <label class="operation-toggle">
              <input type="checkbox" checked={row.bestEffort} disabled={surface.locked}
                onchange={(event) => row.onBestEffortChange?.(event.currentTarget.checked)} />
              <span>{surface.tr("gui.nested_extract.best_effort", "Continue extracting readable files if some entries are damaged")}</span>
            </label>
          {/if}
        </li>
      {/each}
    </ol>
    <p class="operation-workspace-hint">{surface.mode === "nested"
      ? surface.tr("gui.nested_extract.execution_hint", "The outer archive is opened first. The inner archive and destination are checked before extraction. Each archive's password is requested separately when needed.")
      : surface.tr("gui.batch.execution_hint", "Archives are checked when the task starts. Passwords are requested when needed. If an archive fails, the remaining archives continue.")}</p>
  {/if}
  </div>
</div>
