<script lang="ts">
  import { tFallback as tr } from "../lib/i18n.svelte";
  import { desktopBasename, desktopDirname } from "../lib/desktop-path";
  import type { CreateSession } from "../lib/create-session.svelte";
  import type { CreateSourceKind } from "../lib/create-sources";
  import Icon from "./Icon.svelte";

  let { variant, session }: { variant: "modern" | "classic"; session: CreateSession } = $props();
  let selectedCount = $derived(session.selectedCount());
  let allSelected = $derived(session.allSelected());
  let lockedReason = $derived(session.view.sourcesLockedReason());
  let reviewReason = $derived(session.sources.length === 0
    ? tr("gui.create.sources.add_before_review")
    : session.configurationPending ? session.view.configurationPendingMessage() : lockedReason);

  let selectAllInput = $state<HTMLInputElement | null>(null);

  $effect(() => {
    if (selectAllInput) selectAllInput.indeterminate = selectedCount > 0 && !allSelected;
  });

  function sourceIcon(kind: CreateSourceKind): string {
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
    if ((event.metaKey || event.ctrlKey) && key === "a" && session.sources.length > 0) {
      event.preventDefault();
      session.selectAll(true);
      return;
    }
    if (event.key === "Escape" && selectedCount > 0) {
      event.preventDefault();
      session.clearSelection();
      return;
    }
    if (
      (event.key === "Delete" || event.key === "Backspace")
      && selectedCount > 0
      && !(event.target instanceof HTMLButtonElement)
    ) {
      event.preventDefault();
      session.removeSelected();
    }
  }
</script>

<svelte:window onkeydown={handleKeydown} />

<section
  class:classic={variant === "classic"}
  class="create-source-list"
  aria-label={tr("gui.create.sources.aria")}
>
  <header class="create-source-list-head">
    <div class="create-source-list-title">
      <span class="create-source-list-mark"><Icon name="archive" size={18} /></span>
      <div>
        <h2>{tr("gui.create.sources.heading")}</h2>
        <p>{tr("gui.create.sources.description")}</p>
      </div>
    </div>
    <div class="create-source-list-add">
      <button
        type="button"
        disabled={session.preflight.sourcesLocked()}
        aria-busy={session.preflight.state.picker?.kind === "files"}
        title={lockedReason}
        onclick={() => void session.chooseSources("files")}
      ><Icon name="file" size={15} />{session.view.sourcePickerLabel("files")}</button>
      <button
        type="button"
        disabled={session.preflight.sourcesLocked()}
        aria-busy={session.preflight.state.picker?.kind === "folder"}
        title={lockedReason}
        onclick={() => void session.chooseSources("folder")}
      ><Icon name="folder-open" size={15} />{session.view.sourcePickerLabel("folder")}</button>
    </div>
  </header>

  {#if session.sources.length === 0}
    <div class="create-source-list-empty">
      <span class="create-source-list-empty-mark"><Icon name="folder-open" size={22} /></span>
      <div>
        <strong>{tr("gui.create.sources.empty_title")}</strong>
        <span>{tr("gui.create.sources.empty_body")}</span>
      </div>
    </div>
  {:else}
    <div class="create-source-list-toolbar">
      <label class="create-source-select-all">
        <input
          bind:this={selectAllInput}
          type="checkbox"
          checked={allSelected}
          disabled={Boolean(lockedReason)}
          aria-label={tr("gui.create.sources.select_all")}
          onchange={(event) => session.selectAll(event.currentTarget.checked)}
        />
        <span>{session.view.sourceCountLabel()}</span>
      </label>
      <span class="create-source-selection">{session.view.sourceSelectionLabel()}</span>
      <button
        type="button"
        class="create-source-remove-selected"
        disabled={selectedCount === 0 || Boolean(lockedReason)}
        title={lockedReason}
        onclick={session.removeSelected}
      >{tr("gui.create.sources.remove_selected")}</button>
    </div>

    <div class="create-source-list-rows" role="list">
      {#each session.sources as row (row.path)}
        {@const name = desktopBasename(row.path, session.port.platform())}
        {@const selected = session.isSelected(row.path)}
        {@const removeLabel = tr("gui.create.sources.remove_item").replace("{name}", name)}
        <div class:selected={selected} class="create-source-row" role="listitem" title={row.path}>
          <label>
            <input
              type="checkbox"
              checked={selected}
              disabled={Boolean(lockedReason)}
              aria-label={tr("gui.create.sources.select_item").replace("{name}", name)}
              onchange={() => session.toggleSelection(row.path)}
            />
            <span class="create-source-kind" role="img" aria-label={session.view.sourceKindLabel(row.kind)}>
              <Icon name={sourceIcon(row.kind)} size={17} />
            </span>
            <span class="create-source-copy">
              <strong>{name}</strong>
              <small>{desktopDirname(row.path, session.port.platform())}</small>
            </span>
          </label>
          <button
            type="button"
            class="create-source-remove"
            disabled={Boolean(lockedReason)}
            title={removeLabel}
            aria-label={removeLabel}
            onclick={() => session.removeSource(row.path)}
          ><Icon name="x" size={15} /></button>
        </div>
      {/each}
    </div>
  {/if}

  <footer class="create-source-list-footer">
    <span>{tr("gui.create.sources.keep_until_queued")}</span>
    <button
      id="create-primary-source-action"
      type="button"
      class:primary={variant === "modern"}
      class:sheet-action={variant === "modern"}
      class:classic-primary={variant === "classic"}
      class="create-source-review-action"
      disabled={session.view.startDisabled()}
      aria-busy={session.preflight.busy()}
      title={reviewReason}
      onclick={() => void session.review()}
    ><Icon name="search" size={16} />{session.view.startLabel(tr("gui.create.sources.review"))}</button>
  </footer>
</section>
