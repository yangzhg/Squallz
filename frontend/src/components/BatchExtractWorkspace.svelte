<script lang="ts">
  import { onMount } from "svelte";
  import type { BatchWorkspaceSurface } from "./ToolsWorkspace.svelte";
  import type { OverwritePolicy, SymlinkPolicy } from "../lib/ipc";
  import { basename } from "../lib/format";
  import { extractOverwriteLabel, extractSymlinkLabel } from "../lib/archive-option-labels";
  import type { BatchExtractEdit } from "../lib/batch-extract.svelte";
  import ArchiveReturnStrip from "./ArchiveReturnStrip.svelte";
  import Icon from "./Icon.svelte";

  let { surface }: { surface: BatchWorkspaceSurface } = $props();
  const overwriteChoices: OverwritePolicy[] = ["ask", "skip", "overwrite", "rename"];
  const symlinkChoices: SymlinkPolicy[] = ["preserve", "skip", "follow"];
  let nested = $derived(surface.session.nested);
  let draft = $derived(surface.mode === "batch" ? surface.session.batch.state : nested);
  let locked = $derived(surface.session.locked(surface.mode));
  let rows = $derived(surface.mode === "batch" ? surface.session.batch.state.items : nested ? [{
    ...nested, path: nested.outer_path, displayPath: `${nested.outerDisplayPath} › ${nested.entry_path}`,
  }] : []);
  function itemName(displayPath: string): string {
    return basename(surface.mode === "nested" && nested ? nested.entry_path : displayPath);
  }
  function edit(change: Exclude<BatchExtractEdit, { kind: "remove" }>): void {
    if (surface.mode === "batch") surface.session.editBatch(change);
    else if (change.kind === "destination") surface.session.editNested({ dest: change.value });
    else if (change.kind === "smart") surface.session.editNested({ smart: change.value });
    else if (change.kind === "overwrite") surface.session.editNested({ overwrite: change.value });
    else surface.session.editNested({ symlinks: change.value });
  }
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
        ? surface.tr("gui.nested_extract.scope")
        : surface.tr("gui.batch.archive_count").replace("{count}", rows.length.toLocaleString())}</p>
    </div>
    <div class="operation-workspace-actions">
      {#if surface.mode === "batch" || !nested}
        <button class="secondary-lite" disabled={locked} onclick={() => surface.mode === "batch" ? void surface.session.choose("batch") : surface.onBrowse()}>
          <Icon name="folder-open" size={16} />{surface.mode === "nested"
            ? surface.tr("gui.archive.back_to_current")
            : surface.tr("gui.batch.add_archives")}
        </button>
      {/if}
      <button class="primary" disabled={locked || rows.length === 0} onclick={() => void surface.session.start(surface.mode)}>
        <Icon name="archive" size={16} />{surface.session.submitting(surface.mode)
          ? surface.tr("gui.task_center.submitting")
          : surface.mode === "nested" ? surface.tr("gui.extract.start")
          : surface.tr("gui.batch.start_batch")}
      </button>
    </div>
  </header>
  <div class="operation-workspace-body">
  {#if rows.length === 0}
    <div class="operation-empty-state">
      <Icon name="archive" size={32} />
      <strong>{surface.mode === "nested" ? surface.tr("gui.nested_extract.empty") : surface.tr("gui.batch.no_archives_queued")}</strong>
      <p>{surface.mode === "nested" ? surface.tr("gui.nested_extract.empty_hint") : surface.tr("gui.batch.empty_hint")}</p>
    </div>
  {:else}
    <div class="operation-policy-controls">
      <label class="operation-toggle">
        <input type="checkbox" checked={draft?.smart ?? true} disabled={locked}
          onchange={(event) => edit({ kind: "smart", value: event.currentTarget.checked })} />
        <span>{surface.tr("gui.batch.mode_smart")}</span>
      </label>
      <label class="operation-field">
        <span>{surface.tr("gui.batch.overwrite")}</span>
        <select class="operation-input" value={draft?.overwrite ?? "ask"} disabled={locked}
          onchange={(event) => edit({ kind: "overwrite", value: event.currentTarget.value as OverwritePolicy })}>
          {#each overwriteChoices as choice}<option value={choice}>{extractOverwriteLabel(choice)}</option>{/each}
        </select>
      </label>
      <label class="operation-field">
        <span>{surface.tr("gui.extract.symlinks")}</span>
        <select class="operation-input" value={draft?.symlinks ?? "preserve"} disabled={locked}
          onchange={(event) => edit({ kind: "symlinks", value: event.currentTarget.value as SymlinkPolicy })}>
          {#each symlinkChoices as choice}<option value={choice}>{extractSymlinkLabel(choice)}</option>{/each}
        </select>
      </label>
    </div>
    <p class="operation-workspace-hint" id="batch-destination-hint">{surface.mode === "nested"
      ? draft?.smart ?? true
        ? surface.tr("gui.nested_extract.smart_destination_hint")
        : surface.tr("gui.nested_extract.direct_destination_hint")
      : draft?.smart ?? true
      ? surface.tr("gui.batch.smart_destination_hint")
      : surface.tr("gui.batch.direct_destination_hint")}</p>
    <ol class="operation-list">
      {#each rows as row, index (`${index}:${row.path}`)}
        <li class="operation-card">
          <div class="operation-card-header">
            <div class="operation-identity">
              <strong>{itemName(row.displayPath)}</strong>
              <span>{surface.format(surface.mode === "nested" && nested ? nested.entry_path : row.displayPath)} · {(surface.mode === "nested"
                ? surface.tr("gui.nested_extract.outer_encoding")
                : surface.tr("gui.batch.encoding")).replace("{encoding}", row.encoding ?? surface.tr("gui.extract.encoding.auto"))}</span>
              {#if row.best_effort && surface.mode === "batch"}<span>{surface.tr("gui.batch.best_effort")}</span>{/if}
            </div>
            {#if surface.mode === "batch"}
              <button class="secondary-lite" id={`batch-remove-${index}`} disabled={locked}
                aria-label={surface.tr("gui.batch.remove_archive").replace("{name}", itemName(row.displayPath))} onclick={() => surface.onRemove(index)}>
                <Icon name="x" size={16} />{surface.tr("gui.batch.remove")}
              </button>
            {/if}
          </div>
          <p class="operation-path">{row.displayPath}</p>
          <label class="operation-destination-field" for={`batch-destination-${index}`}>
            {surface.tr("gui.batch.destination")}
          </label>
          <div class="operation-control">
            <input class="operation-input" id={`batch-destination-${index}`} value={row.dest}
              aria-label={surface.tr("gui.batch.destination_for").replace("{name}", itemName(row.displayPath))}
              aria-describedby="batch-destination-hint" disabled={locked}
              oninput={(event) => edit({ kind: "destination", index, value: event.currentTarget.value })} />
            <button class="secondary-lite" disabled={locked} onclick={() => void surface.session.choose(surface.mode, index)}
              aria-label={surface.tr("gui.batch.browse_destination_for").replace("{name}", itemName(row.displayPath))}>
              <Icon name="folder" size={16} />{surface.tr("gui.compress.browse")}
            </button>
          </div>
          {#if surface.mode === "nested"}
            <label class="operation-toggle">
              <input type="checkbox" checked={row.best_effort} disabled={locked}
                onchange={(event) => surface.session.editNested({ best_effort: event.currentTarget.checked })} />
              <span>{surface.tr("gui.nested_extract.best_effort")}</span>
            </label>
          {/if}
        </li>
      {/each}
    </ol>
    <p class="operation-workspace-hint">{surface.mode === "nested"
      ? surface.tr("gui.nested_extract.execution_hint")
      : surface.tr("gui.batch.execution_hint")}</p>
  {/if}
  </div>
</div>
