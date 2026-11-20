<script lang="ts">
  import type { BrowserEntry } from "../lib/archive-browser";

  let { entry, tabStop, disabled, tr, onOpenResultFolder }: {
    entry: BrowserEntry;
    tabStop: boolean;
    disabled: boolean;
    tr: (key: string) => string;
    onOpenResultFolder: (entry: BrowserEntry) => void;
  } = $props();
</script>

{#key entry.source?.path ?? entry.name}
  <span class="archive-entry-label" tabindex="-1"
    onfocus={(event) => event.currentTarget.closest<HTMLElement>('[role="row"]')?.focus({ preventScroll: true })}>
    <strong title={entry.source?.path ?? entry.name}>{entry.name}</strong>
    {#if entry.location}
      {@const folder = entry.location.label}
      {@const actionLabel = tr("gui.list.open_result_folder").replace("{folder}", () => folder)}
      <button type="button" class="archive-entry-location" tabindex={tabStop ? 0 : -1}
        {disabled} title={actionLabel} aria-label={actionLabel}
        onclick={(event) => { event.stopPropagation(); onOpenResultFolder(entry); }}>
        {entry.location.label}
      </button>
    {/if}
  </span>
{/key}
