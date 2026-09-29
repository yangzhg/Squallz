<script lang="ts">
  import { onMount, tick } from "svelte";
  import type { ArchiveUpdateWorkspaceSurface } from "./ToolsWorkspace.svelte";
  import type { CreateContentPolicy } from "../lib/ipc";
  import type { UpdateOperation, UpdateIssue } from "../lib/archive-update.svelte";
  import ExcludeRulesEditor from "./ExcludeRulesEditor.svelte";
  import Icon from "./Icon.svelte";

  let { surface }: { surface: ArchiveUpdateWorkspaceSurface } = $props();
  let visibleCount = $state(50);
  let advancedOpen = $state(false);
  let review = $derived(surface.review);
  let draft = $derived(review.draft);
  const policies: CreateContentPolicy[] = ["keep_all_files", "cross_platform_clean", "custom"];
  onMount(() => surface.onReady());

  function operationLabel(kind: UpdateOperation["kind"]): string {
    if (kind === "add") return surface.tr("gui.action.add_files", "Add files");
    if (kind === "delete") return surface.tr("gui.update_review.delete", "Delete from archive");
    if (kind === "mkdir") return surface.tr("gui.action.new_folder", "New folder");
    return surface.tr("gui.update_review.rename", "Rename or move");
  }

  function issueLabel(issue: UpdateIssue): string {
    if (issue.kind === "selection") return surface.tr("gui.update_review.select", "Select at least one change to apply.");
    if (issue.kind === "level") return surface.tr("gui.update_review.level_invalid", "Enter a whole compression level from 0 to 9.");
    if (issue.kind === "unchanged") return surface.tr("gui.rename.target_must_differ", "Rename target must differ from source");
    if (issue.kind === "path") return surface.tr("gui.update_review.path_invalid", "Use a path inside the archive without parent references (..) or reserved file names and characters.");
    return surface.tr("gui.update_review.path_empty", "Enter a path for this change.");
  }

  async function submit() {
    await surface.onSubmit();
    if (!review.issue) return;
    if (review.issue.field === "update-level") advancedOpen = true;
    const index = draft?.operations.findIndex((row) => `update-operation-${row.id}` === review.issue?.field) ?? -1;
    visibleCount = Math.max(visibleCount, index + 1);
    await tick();
    document.getElementById(review.issue.field)?.focus();
  }
</script>

<div class="operation-workspace" class:classic-operation-workspace={surface.variant === "classic"}>
  <header class="operation-workspace-header">
    <div>
      <h1 id="update-review-heading" tabindex="-1">{surface.title}</h1>
      <p>{surface.tr("gui.update_review.count", "{count} changes selected").replace("{count}", String(review.selectedCount()))}</p>
      {#if draft}
        <p><strong>{surface.tr("gui.update_review.archive", "Archive to change")}</strong></p>
        <p class="operation-path">{draft.displayPath}</p>
      {/if}
    </div>
    <div class="operation-workspace-actions">
      {#if surface.archiveReturn.visible}
        <button class="secondary-lite" title={surface.archiveReturn.title} onclick={surface.archiveReturn.onReturn}>
          <Icon name="archive" size={16} />{surface.archiveReturn.actionLabel}
        </button>
      {/if}
      <button class="primary" disabled={review.pending || review.selectedCount() === 0} onclick={submit}>
        <Icon name="check" size={16} />{review.pending ? surface.tr("gui.task_center.submitting", "Adding to the queue…") : surface.tr("gui.update_review.apply", "Apply selected changes")}
      </button>
    </div>
  </header>
  <div class="operation-workspace-body">
    {#if draft}
      <p class="operation-workspace-hint">{surface.tr("gui.update_review.hint", "These changes modify this archive. Review every selected operation; the archive may have changed since the failed task. Entries and conflicts are checked again before writing. Passwords are requested when needed.")}</p>
      {#if review.issue}<p id="update-review-error" class="update-review-error" role="alert">{issueLabel(review.issue)}</p>{/if}
      <ol class="operation-list">
        {#each draft.operations.slice(0, visibleCount) as row (row.id)}
          <li class="operation-card">
            <label class="operation-toggle">
              <input type="checkbox" checked={row.enabled} disabled={review.pending}
                aria-label={`${operationLabel(row.kind)} · ${row.source || row.value}`}
                onchange={(event) => review.editOperation(row.id, {enabled:event.currentTarget.checked})} />
              <strong>{operationLabel(row.kind)}</strong>
            </label>
            {#if row.source}<p class="operation-path">{row.source}</p>{/if}
            {#if row.kind !== "delete"}
              <label class="operation-field" for={`update-operation-${row.id}`}>
                <span>{row.kind === "add" ? surface.tr("gui.update_review.source", "File to add") : surface.tr("gui.update_review.target", "Path inside archive")}</span>
                {#if row.kind === "add"}
                  <textarea class="operation-input" id={`update-operation-${row.id}`} rows="2" value={row.value}
                    disabled={review.pending || !row.enabled} aria-invalid={review.issue?.field === `update-operation-${row.id}`}
                    aria-describedby={review.issue?.field === `update-operation-${row.id}` ? "update-review-error" : undefined}
                    spellcheck={false} oninput={(event) => review.editOperation(row.id, {value:event.currentTarget.value})}></textarea>
                {:else}
                  <input class="operation-input" id={`update-operation-${row.id}`} value={row.value}
                    disabled={review.pending || !row.enabled} aria-invalid={review.issue?.field === `update-operation-${row.id}`}
                    aria-describedby={review.issue?.field === `update-operation-${row.id}` ? "update-review-error" : undefined}
                    spellcheck={false} oninput={(event) => review.editOperation(row.id, {value:event.currentTarget.value})} />
                {/if}
              </label>
            {/if}
          </li>
        {/each}
      </ol>
      {#if draft.operations.length > visibleCount}
        <button class="secondary-lite" onclick={() => visibleCount += 50}>{surface.tr("gui.update_review.more", "Show more changes")}</button>
      {/if}
      <details class="update-review-options" bind:open={advancedOpen}>
        <summary>{surface.tr("gui.update_review.options", "Encoding and compression options")}</summary>
        <div class="operation-policy-controls">
          <label class="operation-field">
            <span>{surface.tr("gui.archive.encoding", "Encoding")}</span>
            <input class="operation-input" value={draft.encoding} disabled={review.pending}
              placeholder={surface.tr("gui.extract.encoding.auto", "Auto-detect")}
              oninput={(event) => review.editSettings({encoding:event.currentTarget.value})} />
          </label>
          <label class="operation-field" for="update-level">
            <span>{surface.tr("gui.update_review.level", "Compression level (0–9)")}</span>
            <input class="operation-input" id="update-level" type="number" min="0" max="9" step="1" value={draft.level}
              disabled={review.pending} aria-invalid={review.issue?.field === "update-level"}
              aria-describedby={review.issue?.field === "update-level" ? "update-review-error" : undefined}
              oninput={(event) => review.editSettings({level:event.currentTarget.value})} />
          </label>
          <label class="operation-field">
            <span>{surface.tr("gui.create.content_policy.title", "Archive contents")}</span>
            <select class="operation-input" value={draft.contentPolicy} disabled={review.pending}
              onchange={(event) => review.editSettings({contentPolicy:event.currentTarget.value as CreateContentPolicy})}>
              {#each policies as policy}<option value={policy}>{surface.policyLabel(policy)}</option>{/each}
            </select>
          </label>
        </div>
        {#if draft.contentPolicy === "custom"}
          <ExcludeRulesEditor title={surface.tr("gui.excludes.title", "Excludes")}
            hint={surface.tr("gui.update_review.excludes_hint", "Exclusions apply to added files and new folders.")}
            countLabel={String(draft.excludes.length)} value={draft.excludesText}
            placeholder={surface.tr("gui.excludes.placeholder", ".git\nnode_modules\n.DS_Store")}
            ariaLabel={surface.tr("gui.excludes.title", "Excludes")} rules={draft.excludes}
            emptyLabel={surface.tr("gui.update_review.no_excludes", "No exclusion rules")}
            disabled={review.pending} onInput={(value) => review.editExcludes(value)} />
        {/if}
      </details>
    {:else}
      <div class="operation-empty-state">
        <Icon name="archive" size={32} />
        <strong>{surface.tr("gui.update_review.empty", "No archive changes to review")}</strong>
        <p>{surface.tr("gui.update_review.empty_hint", "Open a failed archive update in the task center and choose Review task settings.")}</p>
        <button class="secondary-lite" onclick={surface.onOpenTasks}>{surface.tr("gui.task_center.title", "Task center")}</button>
      </div>
    {/if}
  </div>
</div>
