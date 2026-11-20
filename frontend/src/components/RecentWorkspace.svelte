<script lang="ts">
  import Icon from "./Icon.svelte";
  import { desktopBasename, sameDesktopPath, type DesktopPathPlatform } from "../lib/desktop-path";
  import { tFallback as tr } from "../lib/i18n.svelte";
  import type { ArchiveInfo } from "../lib/ipc";

  let { paths, current, platform, canOpen, opening, onOpen, onReopen, onReturn }: {
    paths: string[];
    current: ArchiveInfo | null;
    platform: DesktopPathPlatform;
    canOpen: boolean;
    opening: boolean;
    onOpen: () => void;
    onReopen: (path: string) => void;
    onReturn: () => void;
  } = $props();

  const otherPaths = $derived.by(() => {
    const activePath = current?.path;
    return activePath ? paths.filter(path => !sameDesktopPath(path, activePath, platform)) : paths;
  });
</script>

{#snippet archiveRow(path: string, name: string, active: boolean)}
  <button
    type="button"
    class="recent-file"
    class:current-file={active}
    disabled={!canOpen}
    onclick={() => active ? onReturn() : onReopen(path)}
  >
    <span class="recent-file-mark"><Icon name="archive" size={20} /></span>
    <span class="recent-file-copy"><strong>{name}</strong><small>{path}</small></span>
    <span class="recent-file-action">{active ? tr("gui.archive.back_to_current") : tr("gui.recent.reopen")}<Icon name="chevron-right" size={16} /></span>
  </button>
{/snippet}

<section class="recent-workspace">
  <header class="recent-heading">
    <h1>{tr("gui.recent.title")}</h1>
    {#if current || otherPaths.length}
      <button type="button" class="primary-lite" disabled={!canOpen} aria-busy={opening} onclick={onOpen}>
        <Icon name="folder-open" size={16} />{tr(opening ? "gui.toolbar.opening" : "gui.archive.open_dialog_title")}
      </button>
    {/if}
  </header>
  {#if current}
    <section class="recent-section">
      <h2>{tr("gui.recent.current_title")}</h2>
      {@render archiveRow(current.path, current.name, true)}
    </section>
  {/if}
  {#if otherPaths.length}
    <section class="recent-section">
      {#if current}<h2>{tr("gui.recent.list_title")}</h2>{/if}
      <ul class="recent-files">
        {#each otherPaths as path (path)}
          <li>{@render archiveRow(path, desktopBasename(path, platform), false)}</li>
        {/each}
      </ul>
    </section>
  {:else if !current}
    <div class="recent-empty">
      <span class="recent-empty-mark"><Icon name="folder-open" size={32} /></span>
      <h2>{tr("gui.recent.none")}</h2>
      <p>{tr("gui.recent.open_to_start")}</p>
      <button type="button" class="primary-lite" disabled={!canOpen} aria-busy={opening} onclick={onOpen}>
        <Icon name="folder-open" size={16} />{tr(opening ? "gui.toolbar.opening" : "gui.archive.open_dialog_title")}
      </button>
    </div>
  {/if}
  <p class="recent-privacy"><Icon name="lock" size={14} />{tr("gui.recent.privacy_body")}</p>
</section>
