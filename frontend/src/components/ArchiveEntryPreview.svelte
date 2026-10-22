<script lang="ts">
  import Icon from "./Icon.svelte";
  import { formatBytes } from "../lib/format";
  import type { ArchivePreviewActions, ArchivePreviewView } from "../lib/archive-browser";

  let { view, actions, tr, classic = false }: {
    view: ArchivePreviewView;
    actions: ArchivePreviewActions;
    tr: (key: string, fallback?: string) => string;
    classic?: boolean;
  } = $props();
  let active = $derived(Boolean(view.nested || view.busy || view.failed || view.entry));
  let iconSize = $derived(classic ? 13 : 14);
</script>

<div
  class={classic ? "tree-note nested-tree-note entry-preview-panel" : "inspector-block nested-preview-block entry-preview-panel"}
  class:preview-sheet-active={active}
  data-preview-policy={view.policyKind}
  data-preview-code={view.policyCode}
  role="region"
  aria-label={tr("gui.preview.panel")}
>
  <div class="preview-panel-heading">
    {#if classic}
      <strong>{tr("gui.preview.panel")}</strong>
    {:else}
      <span class="block-label">{tr("gui.preview.panel")}</span>
    {/if}
    {#if active}
      <button
        type="button"
        class="preview-panel-close"
        aria-label={tr("gui.preview.close")}
        title={tr("gui.preview.close")}
        onclick={(event) => actions.onClearPreview(event.detail === 0)}
      ><Icon name="x" size={iconSize} /></button>
    {/if}
  </div>
  <div class="entry-preview-body">
    {#if classic}
      {#if view.nested || !view.busy}
        <div class="entry-preview-summary" role={active ? "status" : undefined}>
          <span>{view.nested?.title ?? view.title}</span>
          <small>{view.nested?.subtitle ?? view.subtitle}</small>
        </div>
      {/if}
      {#if view.nested}
        {#each view.nested.rows as item}
          <em>{item.display}</em>
        {/each}
      {/if}
    {:else if view.nested}
      <div class="entry-preview-summary" role="status">
        <strong>{view.nested.title}</strong>
        <p>{view.nested.subtitle}</p>
      </div>
      <div class="nested-preview-list">
        {#each view.nested.rows as item}
          <div>
            {#if item.entry_type === "symlink" || item.entry_type === "hardlink"}
              {@const label = (item.entry_type === "symlink" ? tr("gui.attr.symlink") : tr("gui.attr.hardlink")) + (item.encrypted ? ` · ${tr("gui.attr.encrypted")}` : "")}
              <span role="img" aria-label={label} title={label}>
                <Icon name={item.encrypted ? "lock" : "link"} size={14} />
              </span>
            {:else}
              <span>{item.entry_type === "dir" ? "DIR" : "FILE"}</span>
            {/if}
            <strong>{item.display}</strong>
            <small>{formatBytes(item.size)}</small>
          </div>
        {/each}
      </div>
    {:else if !view.busy}
      <div class="entry-preview-summary" role={active ? "status" : undefined}>
        <strong>{view.title}</strong>
        <p>{view.subtitle}</p>
      </div>
    {/if}
    {#if !view.nested && view.busy}
      <div class="preview-loading" class:compact={classic} role="status" aria-live="polite">
        <span>{tr("gui.preview.loading")}</span>
        <small>{view.subtitle}</small>
      </div>
    {/if}
  </div>
  {#if view.nested || !view.busy}
    <div class="entry-preview-actions" class:inline-actions={!classic}>
      {#if view.nested}
        <button onclick={actions.onOpenNestedPreview}>
          <Icon name="folder-open" size={iconSize} />{tr("gui.action.open_nested")}
        </button>
        <button onclick={actions.onExtractNestedPreview}>
          <Icon name="archive" size={iconSize} />{tr("gui.action.extract_nested")}
        </button>
      {:else if view.failed}
        <button onclick={actions.onRetryPreview}>
          <Icon name="rotate-cw" size={iconSize} />{tr("gui.preview.retry")}
        </button>
        <button onclick={actions.onExtractPreviewFailure}>
          <Icon name="archive" size={iconSize} />{tr("gui.preview.extract_instead")}
        </button>
      {:else if view.entry}
        <button class="preview-system-action" onclick={actions.onOpenPreview}>
          <Icon name="external-link" size={iconSize} />{tr("gui.action.open_preview")}
        </button>
        <button onclick={actions.onRevealPreview}>
          <Icon name="folder-open" size={iconSize} />{tr("gui.toast.reveal")}
        </button>
      {:else}
        <button
          disabled={!view.canPreview}
          aria-busy={view.busy}
          title={view.disabledReason}
          aria-label={view.disabledReason ? `${view.actionLabel}${classic ? " · " : " — "}${view.disabledReason}` : view.actionLabel}
          onclick={actions.onPreviewSelection}
        ><Icon name={view.actionIcon} size={iconSize} />{view.actionLabel}</button>
      {/if}
    </div>
  {/if}
</div>
