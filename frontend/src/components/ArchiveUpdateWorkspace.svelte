<script lang="ts">
  import { onMount, tick } from "svelte";
  import type { ArchiveUpdateWorkspaceSurface } from "./ToolsWorkspace.svelte";
  import type { CreateContentPolicy } from "../lib/ipc";
  import type { ArchiveUpdateReview, UpdateOperation, UpdateIssue } from "../lib/archive-update.svelte";
  import ExcludeRulesEditor from "./ExcludeRulesEditor.svelte";
  import Icon from "./Icon.svelte";

  let { surface }: { surface: ArchiveUpdateWorkspaceSurface } = $props();
  const pageSize = 50;
  let pageSelection = $state.raw<{ draft: NonNullable<ArchiveUpdateReview["draft"]>; index: number } | null>(null);
  let advancedOpen = $state(false);
  let heading = $state<HTMLHeadingElement | null>(null);
  let body = $state<HTMLDivElement | null>(null);
  let review = $derived(surface.review);
  let draft = $derived(review.draft);
  let pageCount = $derived(Math.ceil((draft?.operations.length ?? 0) / pageSize));
  let page = $derived(pageSelection?.draft === draft && pageSelection
    ? Math.min(pageSelection.index, Math.max(0, pageCount - 1)) : 0);
  let start = $derived(page * pageSize);
  let visibleOperations = $derived(draft?.operations.slice(start, start + pageSize) ?? []);
  let range = $derived(surface.tr("gui.update_review.range", "Changes {start}–{end} of {count} · selections are kept across pages")
    .replace("{start}", (start + 1).toLocaleString())
    .replace("{end}", (start + visibleOperations.length).toLocaleString())
    .replace("{count}", (draft?.operations.length ?? 0).toLocaleString()));
  const policies: CreateContentPolicy[] = ["keep_all_files", "cross_platform_clean", "custom"];
  onMount(() => surface.onReady());

  $effect(() => {
    const restoredDraft = draft;
    pageSelection = null;
    advancedOpen = false;
    void tick().then(() => {
      if (review.draft === restoredDraft && body?.isConnected) body.scrollTop = 0;
    });
  });

  function operationLabel(kind: UpdateOperation["kind"]): string {
    if (kind === "add") return surface.tr("gui.action.add_files", "Add files");
    if (kind === "delete") return surface.tr("gui.update_review.delete", "Delete from archive");
    if (kind === "mkdir") return surface.tr("gui.action.new_folder", "New folder");
    return surface.tr("gui.update_review.rename", "Rename or move");
  }

  function issueLabel(issue: UpdateIssue): string {
    if (issue.kind === "selection") return surface.tr("gui.update_review.select", "Select at least one change to apply.");
    if (issue.kind === "level") return surface.tr("gui.update_review.level_invalid", "Enter a whole compression level from 0 to 9.");
    if (issue.kind === "unchanged") return surface.tr("gui.rename.target_must_differ", "The name is unchanged. Enter a different name or path.");
    if (issue.kind === "path") return surface.tr("gui.update_review.path_invalid", "Use a relative path inside the archive without a leading slash, parent references (..), or reserved file names and characters.");
    return surface.tr("gui.update_review.path_empty", "Enter a path for this change.");
  }

  async function submit() {
    const submittedDraft = draft;
    await surface.onSubmit();
    const issue = review.issue;
    if (!issue || !submittedDraft || review.draft !== submittedDraft || !body?.isConnected) return;
    if (issue.field === "update-level") advancedOpen = true;
    const index = submittedDraft.operations.findIndex((row) => `update-operation-${row.id}` === issue.field);
    if (index >= 0) pageSelection = { draft: submittedDraft, index: Math.floor(index / pageSize) };
    await tick();
    if (review.draft === submittedDraft && body?.isConnected && review.issue === issue) {
      document.getElementById(issue.field)?.focus();
    }
  }

  async function changePage(next: number) {
    const currentDraft = draft;
    if (!currentDraft) return;
    pageSelection = { draft: currentDraft, index: Math.max(0, Math.min(next, pageCount - 1)) };
    await tick();
    if (review.draft !== currentDraft || !body?.isConnected) return;
    body.scrollTop = 0;
    heading?.focus({ preventScroll: true });
  }

  async function chooseSource(id: number, kind: "file" | "folder") {
    const trigger = document.activeElement instanceof HTMLButtonElement ? document.activeElement : null;
    await surface.onChooseSource(id, kind);
    await tick();
    if (trigger?.isConnected && !trigger.disabled && document.activeElement === document.body) {
      trigger.focus({ preventScroll: true });
    }
  }

  function sourceFeedback(id: number): string {
    if (review.sourcePicking === id) return surface.tr("gui.update_review.source_choosing", "Choose a replacement source in the file dialog.");
    if (review.sourceFeedback?.id !== id) return "";
    if (review.sourceFeedback.kind === "failed") return surface.tr("gui.update_review.source_failed", "Could not open the chooser. The source was kept. Try again or edit the path here.");
    if (review.sourceFeedback.kind === "cancelled") return surface.tr("gui.update_review.source_cancelled", "Selection cancelled. The source was kept.");
    return surface.tr("gui.update_review.source_selected", "Source replaced. Review the changes before applying them.");
  }
</script>

<div class="operation-workspace" class:classic-operation-workspace={surface.variant === "classic"}>
  <header class="operation-workspace-header">
    <div>
      <h1 id="update-review-heading" bind:this={heading} tabindex="-1">{surface.title}</h1>
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
      <button class="primary" disabled={review.pending || review.sourcePicking !== null || review.selectedCount() === 0} onclick={submit}>
        <Icon name="check" size={16} />{review.pending ? surface.tr("gui.task_center.submitting", "Adding to the queue…") : surface.tr("gui.update_review.apply", "Apply selected changes")}
      </button>
    </div>
  </header>
  {#if draft && pageCount > 1}
    <div class="operation-page-controls">
      <p id="update-review-range" class="operation-range" role="status">{range}</p>
      <nav class="operation-pagination" aria-label={surface.tr("gui.update_review.pages", "Archive change pages")}>
        <button type="button" class="secondary-lite" disabled={page === 0} onclick={() => void changePage(0)}>{surface.tr("gui.update_review.first_page", "First page")}</button>
        <button type="button" class="secondary-lite" disabled={page === 0} onclick={() => void changePage(page - 1)}>{surface.tr("gui.update_review.previous_page", "Previous page")}</button>
        <button type="button" class="secondary-lite" disabled={page === pageCount - 1} onclick={() => void changePage(page + 1)}>{surface.tr("gui.update_review.next_page", "Next page")}</button>
        <button type="button" class="secondary-lite" disabled={page === pageCount - 1} onclick={() => void changePage(pageCount - 1)}>{surface.tr("gui.update_review.last_page", "Last page")}</button>
      </nav>
    </div>
  {/if}
  <div class="operation-workspace-body" bind:this={body}>
    {#if draft}
      <p class="operation-workspace-hint">{surface.tr("gui.update_review.hint", "These changes modify this archive. Review every selected operation; the archive may have changed since the original task. Entries and conflicts are checked again before writing. Passwords are requested when needed.")}</p>
      {#if review.issue?.kind === "selection"}<p id="update-review-error" class="update-review-error" role="alert">{issueLabel(review.issue)}</p>{/if}
      <ol class="operation-list" start={start + 1}>
        {#each visibleOperations as row, index (row.id)}
          <li class="operation-card" aria-posinset={start + index + 1} aria-setsize={draft.operations.length}>
            <label class="operation-toggle">
              <input type="checkbox" checked={row.enabled} disabled={review.pending || review.sourcePicking === row.id}
                aria-label={`${operationLabel(row.kind)} · ${row.source || row.value}`}
                onchange={(event) => review.editOperation(row.id, {enabled:event.currentTarget.checked})} />
              <strong>{operationLabel(row.kind)}</strong>
            </label>
            {#if row.source}<p class="operation-path">{row.source}</p>{/if}
            {#if row.kind !== "delete"}
              <label class="operation-field" for={`update-operation-${row.id}`}>
                <span>{row.kind === "add" ? surface.tr("gui.update_review.source", "File or folder to add") : surface.tr("gui.update_review.target", "Path inside archive")}</span>
                {#if row.kind === "add"}
                  <textarea class="operation-input" id={`update-operation-${row.id}`} rows="2" value={row.value}
                    disabled={review.pending || review.sourcePicking === row.id || !row.enabled} aria-invalid={review.issue?.field === `update-operation-${row.id}`}
                    aria-describedby={[review.issue?.field === `update-operation-${row.id}` ? "update-review-error" : "", sourceFeedback(row.id) ? `update-source-feedback-${row.id}` : ""].filter(Boolean).join(" ") || undefined}
                    spellcheck={false} oninput={(event) => review.editOperation(row.id, {value:event.currentTarget.value})}></textarea>
                {:else}
                  <input class="operation-input" id={`update-operation-${row.id}`} value={row.value}
                    disabled={review.pending || !row.enabled} aria-invalid={review.issue?.field === `update-operation-${row.id}`}
                    aria-describedby={review.issue?.field === `update-operation-${row.id}` ? "update-review-error" : undefined}
                    spellcheck={false} oninput={(event) => review.editOperation(row.id, {value:event.currentTarget.value})} />
                {/if}
              </label>
              {#if review.issue?.field === `update-operation-${row.id}`}
                <p id="update-review-error" class="update-review-error" role="alert">{issueLabel(review.issue)}</p>
              {/if}
              {#if row.kind === "add"}
                <div class="operation-workspace-actions">
                  <button class="secondary-lite" disabled={review.pending || review.sourcePicking !== null || !row.enabled} onclick={() => void chooseSource(row.id, "file")}>
                    <Icon name="file" size={16} />{surface.tr("gui.update_review.choose_file", "Choose file…")}
                  </button>
                  <button class="secondary-lite" disabled={review.pending || review.sourcePicking !== null || !row.enabled} onclick={() => void chooseSource(row.id, "folder")}>
                    <Icon name="folder" size={16} />{surface.tr("gui.update_review.choose_folder", "Choose folder…")}
                  </button>
                </div>
                {#if sourceFeedback(row.id)}
                  <p id={`update-source-feedback-${row.id}`} class={review.sourceFeedback?.id === row.id && review.sourceFeedback.kind === "failed" ? "update-review-error" : "operation-workspace-hint"}
                    role={review.sourceFeedback?.id === row.id && review.sourceFeedback.kind === "failed" ? "alert" : "status"}>{sourceFeedback(row.id)}</p>
                {/if}
              {/if}
            {/if}
          </li>
        {/each}
      </ol>
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
            {#if review.issue?.field === "update-level"}
              <span id="update-review-error" class="update-review-error" role="alert">{issueLabel(review.issue)}</span>
            {/if}
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
        <p>{surface.tr("gui.update_review.empty_hint", "Open a failed or cancelled archive update in the task center and choose Review task settings.")}</p>
        <button class="secondary-lite" onclick={surface.onOpenTasks}>{surface.tr("gui.task_center.title", "Task center")}</button>
      </div>
    {/if}
  </div>
</div>
