<script lang="ts">
  import type { Snippet } from "svelte";
  import Icon from "./Icon.svelte";
  import type { ArchiveBrowserView, ArchiveBrowserActions } from "../lib/archive-browser";
  import type { ArchiveSortKey } from "../lib/ipc";

  let { column, label, sort, onSort, disabled = false, children }: {
    column: ArchiveSortKey;
    label: string;
    sort: ArchiveBrowserView["sort"];
    onSort: ArchiveBrowserActions["onSort"];
    disabled?: boolean;
    children?: Snippet;
  } = $props();
  const active = $derived(sort.key === column);
</script>

<span role="columnheader" class="archive-sort-heading"
  aria-sort={active ? (sort.direction === "asc" ? "ascending" : "descending") : undefined}>
  {@render children?.()}
  <button type="button" class="archive-sort-button" {disabled} onclick={() => onSort(column)}>
    {label}
    <Icon name={active ? (sort.direction === "asc" ? "chevron-up" : "chevron-down") : "chevrons-up-down"} size={13} />
  </button>
</span>
