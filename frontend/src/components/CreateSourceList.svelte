<script lang="ts" module>
  export type CreateSourceListVariant = "modern" | "classic";
  export type CreateSourceListKind = "file" | "folder" | "unknown";

  export interface CreateSourceListAction {
    label: string;
    disabled: boolean;
    busy: boolean;
    title: string;
    onSelect: () => void;
  }

  export interface CreateSourceListRow {
    path: string;
    name: string;
    parent: string;
    kind: CreateSourceListKind;
    kindLabel: string;
    selected: boolean;
    selectLabel: string;
    removeLabel: string;
  }

  export interface CreateSourceListSurface {
    ariaLabel: string;
    heading: string;
    description: string;
    countLabel: string;
    selectionLabel: string;
    selectAllLabel: string;
    emptyTitle: string;
    emptyBody: string;
    removeSelectedLabel: string;
    keepUntilQueuedLabel: string;
    lockedReason: string;
    rows: CreateSourceListRow[];
    selectedCount: number;
    allSelected: boolean;
    mixedSelection: boolean;
    addFiles: CreateSourceListAction;
    addFolders: CreateSourceListAction;
    review: CreateSourceListAction;
    onToggleAll: (selected: boolean) => void;
    onToggleRow: (path: string) => void;
    onRemoveRow: (path: string) => void;
    onRemoveSelected: () => void;
    onClearSelection: () => void;
  }
</script>

<script lang="ts">
  import Icon from "./Icon.svelte";

  let {
    variant,
    surface,
  }: {
    variant: CreateSourceListVariant;
    surface: CreateSourceListSurface;
  } = $props();

  let selectAllInput = $state<HTMLInputElement | null>(null);

  $effect(() => {
    if (selectAllInput) selectAllInput.indeterminate = surface.mixedSelection;
  });

  function sourceIcon(kind: CreateSourceListKind): string {
    if (kind === "folder") return "folder";
    if (kind === "file") return "file";
    return "archive";
  }

  function handleKeydown(event: KeyboardEvent): void {
    if (
      !(event.target instanceof Element)
      || event.target.closest(".create-source-list") === null
    ) return;
    const key = event.key.toLowerCase();
    if ((event.metaKey || event.ctrlKey) && key === "a" && surface.rows.length > 0) {
      event.preventDefault();
      surface.onToggleAll(true);
      return;
    }
    if (event.key === "Escape" && surface.selectedCount > 0) {
      event.preventDefault();
      surface.onClearSelection();
      return;
    }
    if (
      (event.key === "Delete" || event.key === "Backspace")
      && surface.selectedCount > 0
      && !(event.target instanceof HTMLButtonElement)
    ) {
      event.preventDefault();
      surface.onRemoveSelected();
    }
  }
</script>

<svelte:window onkeydown={handleKeydown} />

<section
  class:classic={variant === "classic"}
  class="create-source-list"
  aria-label={surface.ariaLabel}
>
  <header class="create-source-list-head">
    <div class="create-source-list-title">
      <span class="create-source-list-mark"><Icon name="archive" size={18} /></span>
      <div>
        <h2>{surface.heading}</h2>
        <p>{surface.description}</p>
      </div>
    </div>
    <div class="create-source-list-add">
      <button
        type="button"
        disabled={surface.addFiles.disabled}
        aria-busy={surface.addFiles.busy}
        title={surface.addFiles.title}
        onclick={surface.addFiles.onSelect}
      ><Icon name="file" size={15} />{surface.addFiles.label}</button>
      <button
        type="button"
        disabled={surface.addFolders.disabled}
        aria-busy={surface.addFolders.busy}
        title={surface.addFolders.title}
        onclick={surface.addFolders.onSelect}
      ><Icon name="folder-open" size={15} />{surface.addFolders.label}</button>
    </div>
  </header>

  {#if surface.rows.length === 0}
    <div class="create-source-list-empty">
      <span class="create-source-list-empty-mark"><Icon name="folder-open" size={22} /></span>
      <div>
        <strong>{surface.emptyTitle}</strong>
        <span>{surface.emptyBody}</span>
      </div>
    </div>
  {:else}
    <div class="create-source-list-toolbar">
      <label class="create-source-select-all">
        <input
          bind:this={selectAllInput}
          type="checkbox"
          checked={surface.allSelected}
          disabled={Boolean(surface.lockedReason)}
          aria-label={surface.selectAllLabel}
          onchange={(event) => surface.onToggleAll(event.currentTarget.checked)}
        />
        <span>{surface.countLabel}</span>
      </label>
      <span class="create-source-selection">{surface.selectionLabel}</span>
      <button
        type="button"
        class="create-source-remove-selected"
        disabled={surface.selectedCount === 0 || Boolean(surface.lockedReason)}
        title={surface.lockedReason}
        onclick={surface.onRemoveSelected}
      >{surface.removeSelectedLabel}</button>
    </div>

    <div class="create-source-list-rows" role="list">
      {#each surface.rows as row (row.path)}
        <div class:selected={row.selected} class="create-source-row" role="listitem" title={row.path}>
          <label>
            <input
              type="checkbox"
              checked={row.selected}
              disabled={Boolean(surface.lockedReason)}
              aria-label={row.selectLabel}
              onchange={() => surface.onToggleRow(row.path)}
            />
            <span class="create-source-kind" role="img" aria-label={row.kindLabel}>
              <Icon name={sourceIcon(row.kind)} size={17} />
            </span>
            <span class="create-source-copy">
              <strong>{row.name}</strong>
              <small>{row.parent}</small>
            </span>
          </label>
          <button
            type="button"
            class="create-source-remove"
            disabled={Boolean(surface.lockedReason)}
            title={row.removeLabel}
            aria-label={row.removeLabel}
            onclick={() => surface.onRemoveRow(row.path)}
          ><Icon name="x" size={15} /></button>
        </div>
      {/each}
    </div>
  {/if}

  <footer class="create-source-list-footer">
    <span>{surface.keepUntilQueuedLabel}</span>
    <button
      id="create-primary-source-action"
      type="button"
      class:primary={variant === "modern"}
      class:sheet-action={variant === "modern"}
      class:classic-primary={variant === "classic"}
      class="create-source-review-action"
      disabled={surface.review.disabled}
      aria-busy={surface.review.busy}
      title={surface.review.title}
      onclick={surface.review.onSelect}
    ><Icon name="search" size={16} />{surface.review.label}</button>
  </footer>
</section>
