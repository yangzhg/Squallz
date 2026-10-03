<script lang="ts" module>
  export interface MoveConflictReviewView {
    targetDir: string;
    readyCount: number;
    items: readonly {
      from: string;
      reason: string | null;
      to: string;
      keepBothTo: string | null;
    }[];
  }
</script>

<script lang="ts">
  import { onMount, tick } from "svelte";

  let {
    review, tr, classic = false, onCancel, onReadyOnly, onKeepBoth,
  }: {
    review: MoveConflictReviewView;
    tr: (key: string, fallback: string) => string;
    classic?: boolean;
    onCancel: () => void;
    onReadyOnly: () => void;
    onKeepBoth: () => void;
  } = $props();

  const id = $props.id();
  const pageSize = 25;
  let selectedPage = $state.raw<{ review: MoveConflictReviewView; index: number } | null>(null);
  let root = $state<HTMLElement | null>(null);
  let heading = $state<HTMLHeadingElement | null>(null);
  let list = $state<HTMLDivElement | null>(null);
  let page = $derived(selectedPage?.review === review ? selectedPage.index : 0);
  let pageCount = $derived(Math.ceil(review.items.length / pageSize));
  let start = $derived(page * pageSize);
  let items = $derived(review.items.slice(start, start + pageSize));
  let range = $derived(tr("gui.move.conflict_range", "Conflicts {start}–{end} of {count}")
    .replace("{start}", (start + 1).toLocaleString())
    .replace("{end}", (start + items.length).toLocaleString())
    .replace("{count}", review.items.length.toLocaleString()));

  onMount(() => {
    root?.scrollIntoView({ block: "nearest", inline: "nearest" });
    heading?.focus({ preventScroll: true });
  });

  async function changePage(next: number) {
    selectedPage = { review, index: Math.max(0, Math.min(next, pageCount - 1)) };
    await tick();
    if (list) list.scrollTop = 0;
    heading?.focus({ preventScroll: true });
  }

  function onReviewKeydown(event: KeyboardEvent) {
    if (event.key !== "Escape") return;
    event.preventDefault();
    event.stopPropagation();
    onCancel();
  }
</script>

<!-- svelte-ignore a11y_no_noninteractive_element_interactions (Escape cancels the review from its focused heading, scroll region or controls) -->
<section
  class="move-conflict-review"
  class:classic-move-conflict-review={classic}
  bind:this={root}
  aria-labelledby={`${id}-title`}
  tabindex="-1"
  onkeydown={onReviewKeydown}
>
  <header class="move-conflict-heading">
    <h2 id={`${id}-title`} bind:this={heading} tabindex="-1">
      {tr("gui.move.target_conflicts", "Move conflicts in {target}: {count}")
        .replace("{count}", review.items.length.toLocaleString())
        .replace("{target}", review.targetDir || "/")}
    </h2>
    <p>
      {tr("gui.move.ready_without_renaming", "{count} entries are ready to move without changing names.")
        .replace("{count}", review.readyCount.toLocaleString())}
      {tr("gui.move.whole_selection", "Your choice applies to the whole selection.")}
    </p>
  </header>
  <!-- svelte-ignore a11y_no_noninteractive_tabindex (focusable scroll region) -->
  <div
    class="move-conflict-list"
    bind:this={list}
    role="region"
    aria-label={tr("gui.move.conflict_paths", "Conflict paths")}
    aria-describedby={`${id}-range`}
    tabindex="0"
  >
    <table class="move-conflict-table" aria-labelledby={`${id}-title`} aria-rowcount={review.items.length + 1}>
      <thead>
        <tr aria-rowindex="1">
          <th scope="col">{tr("common.source", "Source")}</th>
          <th scope="col">{tr("gui.move.requested_target", "Requested target")}</th>
          <th scope="col">{tr("gui.move.keep_both_target", "Target when keeping both")}</th>
        </tr>
      </thead>
      <tbody>
        {#each items as item, index}
          <tr aria-rowindex={start + index + 2}>
            <td><strong class="move-conflict-path">{item.from}</strong>{#if item.reason}<span class="move-conflict-reason">{item.reason}</span>{/if}</td>
            <td><span class="move-conflict-path">{item.to}</span></td>
            <td class="move-conflict-alternative"><span class="move-conflict-path">{item.keepBothTo}</span></td>
          </tr>
        {/each}
      </tbody>
    </table>
  </div>
  <div class="move-conflict-page-controls">
    <p id={`${id}-range`} class="move-conflict-range" role="status">{range}</p>
    {#if pageCount > 1}
      <nav class="move-conflict-pagination" aria-label={tr("gui.move.conflict_pages", "Move conflict pages")}>
        <button type="button" class="secondary-lite" disabled={page === 0} onclick={() => void changePage(0)}>{tr("gui.move.first_page", "First page")}</button>
        <button type="button" class="secondary-lite" disabled={page === 0} onclick={() => void changePage(page - 1)}>{tr("gui.move.previous_page", "Previous page")}</button>
        <button type="button" class="secondary-lite" disabled={page === pageCount - 1} onclick={() => void changePage(page + 1)}>{tr("gui.move.next_page", "Next page")}</button>
        <button type="button" class="secondary-lite" disabled={page === pageCount - 1} onclick={() => void changePage(pageCount - 1)}>{tr("gui.move.last_page", "Last page")}</button>
      </nav>
    {/if}
  </div>
  <div class="move-conflict-actions" class:classic-button-row={classic}>
    <button type="button" onclick={onCancel}>{tr("common.cancel", "Cancel")}</button>
    <button type="button" disabled={review.readyCount === 0} onclick={onReadyOnly}>{tr("gui.move.ready_only", "Move items without conflicts")}</button>
    <button type="button" class={classic ? "classic-primary" : "primary-lite"} onclick={onKeepBoth}>{tr("gui.move.keep_both_all", "Keep both and move all")}</button>
  </div>
</section>
