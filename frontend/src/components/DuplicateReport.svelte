<script lang="ts" module>
  import type { DuplicateGroupSummary } from "../lib/task-model";

  export interface DuplicateReportSurface {
    taskId: number | null;
    state: string;
    context: { sources: string[]; minimumSize: string; excludes: string[] } | null;
    summary: Array<{ label: string; value: string }>;
    groups: DuplicateGroupSummary[];
    emptyMessage: string;
    onMount: (node: HTMLElement | null) => void;
  }
</script>

<script lang="ts">
  import { tick } from "svelte";
  import { formatBytes } from "../lib/format";

  let { report, tr }: { report: DuplicateReportSurface; tr: (key: string, fallback: string) => string } = $props();
  const pageSize = 20;
  const pathPageSize = 50;
  let page = $state(0);
  let openGroup = $state<number | null>(0);
  let visiblePaths = $state(pathPageSize);
  let heading = $state<HTMLHeadingElement | null>(null);
  let pathList = $state<HTMLUListElement | null>(null);
  let start = $derived(page * pageSize);
  let groups = $derived(report.groups.slice(start, start + pageSize));

  function registerPanel(node: HTMLElement) {
    report.onMount(node);
    return { destroy: () => report.onMount(null) };
  }

  async function changePage(next: number) {
    page = next;
    openGroup = next * pageSize;
    visiblePaths = pathPageSize;
    await tick();
    heading?.scrollIntoView({ block: "start", inline: "nearest" });
    heading?.focus({ preventScroll: true });
  }

  async function showMorePaths() {
    const firstNewPath = visiblePaths;
    visiblePaths += pathPageSize;
    await tick();
    const item = pathList?.children.item(firstNewPath);
    if (item instanceof HTMLElement) item.focus();
  }
</script>

<section class="duplicate-report" tabindex="-1" aria-label={tr("gui.duplicates.report_title", "Duplicate scan report")} use:registerPanel>
  <header class="duplicate-report-heading">
    <h2 bind:this={heading} tabindex="-1">{tr("gui.duplicates.report_title", "Duplicate scan report")}</h2>
    {#if report.context}<span>{report.state}</span>{/if}
  </header>
  {#if report.context}
    <dl class="tool-report-context">
      <div><dt>{tr("gui.duplicates.report_target", "Scanned locations")}</dt><dd>{#each report.context.sources as source}<span>{source}</span>{/each}</dd></div>
      <div><dt>{tr("gui.duplicates.minimum_size", "Minimum size")}</dt><dd>{report.context.minimumSize}</dd></div>
      <div><dt>{tr("gui.create.excludes", "Excludes")}</dt><dd>{#each report.context.excludes as rule}<span>{rule}</span>{:else}{tr("gui.create.no_rules", "No rules")}{/each}</dd></div>
      {#each report.summary as item}<div><dt>{item.label}</dt><dd>{item.value}</dd></div>{/each}
    </dl>
  {/if}
  {#if groups.length > 0}
    <p class="duplicate-report-hint">{tr("gui.duplicates.potential_space", "Potential space if one copy per group remains")}</p>
    <div class="duplicate-report-groups">
      {#each groups as group, index}
        {@const groupIndex = start + index}
        <details
          class="duplicate-report-group"
          open={openGroup === groupIndex}
          ontoggle={(event) => {
            if (event.currentTarget.open) { openGroup = groupIndex; visiblePaths = pathPageSize; }
            else if (openGroup === groupIndex) openGroup = null;
          }}
        >
          <summary>
            <strong>{tr("gui.duplicates.group_number", "Group {number}").replace("{number}", String(groupIndex + 1))}</strong>
            <span>{tr("gui.duplicates.group_files_size", "{count} files · {size} each")
              .replace("{count}", group.paths.length.toLocaleString()).replace("{size}", formatBytes(group.size))}</span>
          </summary>
          {#if openGroup === groupIndex}
            <div class="duplicate-report-group-body">
              <div class="duplicate-report-digest"><span>BLAKE3</span><code>{group.hash}</code></div>
              <ul bind:this={pathList}>{#each group.paths.slice(0, visiblePaths) as path}<li tabindex="-1">{path}</li>{/each}</ul>
              {#if group.paths.length > visiblePaths}
                <button type="button" class="secondary-lite" onclick={() => void showMorePaths()}>
                  {tr("gui.duplicates.show_more_paths", "Show more files ({shown} of {total})")
                    .replace("{shown}", visiblePaths.toLocaleString()).replace("{total}", group.paths.length.toLocaleString())}
                </button>
              {/if}
            </div>
          {/if}
        </details>
      {/each}
    </div>
    {#if report.groups.length > pageSize}
      <nav class="duplicate-report-pagination" aria-label={tr("gui.duplicates.report_pages", "Duplicate report pages")}>
        <button type="button" class="secondary-lite" disabled={page === 0} onclick={() => void changePage(page - 1)}>{tr("gui.duplicates.previous_groups", "Previous groups")}</button>
        <span>{tr("gui.duplicates.group_range", "Groups {start}–{end} of {total}")
          .replace("{start}", String(start + 1)).replace("{end}", String(start + groups.length)).replace("{total}", report.groups.length.toLocaleString())}</span>
        <button type="button" class="secondary-lite" disabled={start + pageSize >= report.groups.length} onclick={() => void changePage(page + 1)}>{tr("gui.duplicates.next_groups", "Next groups")}</button>
      </nav>
    {/if}
  {:else}
    <p class="duplicate-report-hint" role="status">{report.emptyMessage}</p>
  {/if}
</section>
