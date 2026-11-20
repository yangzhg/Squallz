<script lang="ts">
  import { onMount, tick } from "svelte";
  import Icon from "./Icon.svelte";
  import { cssVariables, type CssVariableMap } from "../lib/css-variables";
  import type { Task } from "../lib/jobs.svelte";
  import { taskNeedsInput } from "../lib/task-model";
  import {
    taskCenterCounts,
    taskCenterRows,
    taskQueueDropBeforeId,
    taskQueuePositions,
    type TaskCenterFilter,
    type TaskCenterListPosition,
    type TaskQueuePosition,
    type TaskQueueDropEdge,
  } from "../lib/task-center";
  import {
    taskActionFailureMessage,
    taskCancelButtonLabel,
    taskDialogResultSummary,
    taskKindLabel,
    taskOutcomeStateLabel,
    taskOutcomeStateTone,
    taskStatusUnavailable,
    taskStatusUnavailableMessage,
    taskOverallProgressIndeterminate,
    taskPauseButtonLabel,
    taskProgressPercent,
    taskProgressSummary,
    taskResumeButtonLabel,
    taskTitleLabel,
    tr,
    type TaskDialogModel,
  } from "../lib/task-dialog";
  import { basename as pathBaseName, formatBytes } from "../lib/format";

  type TaskAction = (task: Task) => void;
  type TaskMoveBeforeAction = (task: Task, beforeTask: Task | null) => void;

  let {
    tasks,
    filter,
    onFilter,
    summary,
    submittingTask = null,
    notice = null,
    rootClass,
    rootVariables = {},
    focusTaskId = null,
    takeListPosition,
    onClose,
    onPause,
    onResume,
    onMoveEarlier,
    onMoveLater,
    onMoveBefore,
    onCancel,
    onDetails,
    onClear,
  }: {
    tasks: readonly Task[];
    filter: TaskCenterFilter;
    onFilter: (filter: TaskCenterFilter) => void;
    summary: string;
    submittingTask?: TaskDialogModel | null;
    notice?: string | null;
    rootClass: string;
    rootVariables?: CssVariableMap;
    focusTaskId?: number | null;
    takeListPosition: (id: number | null) => TaskCenterListPosition | null;
    onClose: () => void;
    onPause: TaskAction;
    onResume: TaskAction;
    onMoveEarlier: TaskAction;
    onMoveLater: TaskAction;
    onMoveBefore: TaskMoveBeforeAction;
    onCancel: TaskAction;
    onDetails: (task: Task, position: TaskCenterListPosition) => void;
    onClear: () => void;
  } = $props();

  let panel: HTMLElement | null = null;
  let list: HTMLElement | null = null;
  let closeButton: HTMLButtonElement | null = null;
  let orderedRows = $derived(taskCenterRows(tasks, filter));
  let counts = $derived(taskCenterCounts(tasks));
  let showSubmitting = $derived(submittingTask && (filter === "all" || filter === "queue"));
  const filters = ["all", "queue", "attention", "results"] as const;
  let filterCounts = $derived({
    all: counts.total + Number(Boolean(submittingTask)),
    queue: counts.active + counts.waiting + Number(Boolean(submittingTask)),
    attention: counts.attention,
    results: counts.completed,
  });
  let canClear = $derived(counts.clearable > 0 && (filter === "all" || filter === "results"));
  let queuePositions = $derived(taskQueuePositions(tasks));
  let draggedTaskId = $state<number | null>(null);
  let dropTargetId = $state<number | null>(null);
  let dropEdge = $state<TaskQueueDropEdge | null>(null);
  let dragPointerId: number | null = null;
  let dragHandle: HTMLElement | null = null;

  function queuedDetail(task: Task, position: TaskQueuePosition | null): string {
    if (!position) return tr("gui.task_center.waiting");
    if (task.queueWaitReason === "cpu_budget") {
      return tr(
        "gui.task_center.waiting_cpu",
      ).replace("{position}", position.position.toLocaleString());
    }
    if (task.queueWaitReason === "parallel_limit") {
      return tr(
        "gui.task_center.waiting_slot",
      ).replace("{position}", position.position.toLocaleString());
    }
    if (task.queueWaitReason === null) {
      return tr("gui.task_center.waiting_next");
    }
    return tr("gui.task_center.waiting_position")
      .replace("{position}", position.position.toLocaleString());
  }

  function taskCpuLabel(task: Task): string {
    return tr("gui.task_center.resource_cpu")
      .replace("{count}", task.cpuThreads.toLocaleString());
  }

  function taskBufferLabel(task: Task): string {
    return tr("gui.task_center.resource_buffer")
      .replace("{size}", formatBytes(task.streamBufferLimitBytes));
  }

  function interactionDetail(task: Task): string | null {
    if (!taskNeedsInput(task)) return null;
    if (task.interaction === "password") {
      return task.ownedByRequester
        ? tr("gui.task_center.password_waiting")
        : tr("gui.task_center.password_task_window");
    }
    if (task.interaction === "conflict") {
      return task.ownedByRequester
        ? tr("gui.task_center.conflict_waiting")
        : tr("gui.task_center.conflict_task_window");
    }
    return null;
  }

  function taskDetail(task: Task, queuePosition: TaskQueuePosition | null): string {
    if (task.state === "queued") return queuedDetail(task, queuePosition);
    if (task.state === "done" || task.state === "failed" || task.state === "cancelled") {
      return taskDialogResultSummary(task);
    }
    return taskProgressSummary(task);
  }

  function taskState(task: Task): string {
    if (taskStatusUnavailable(task)) return taskOutcomeStateLabel(task);
    if (task.controlIntent === "cancel") return taskCancelButtonLabel(task);
    return !taskNeedsInput(task)
      ? taskOutcomeStateLabel(task)
      : tr("gui.task_center.needs_input");
  }

  function taskTone(task: Task): string {
    return taskNeedsInput(task) ? "warning" : taskOutcomeStateTone(task);
  }

  function taskCurrentItem(task: Task): string | null {
    if (!task.current) return null;
    const last = taskStatusUnavailable(task) || task.state === "done" || task.state === "failed" || task.state === "cancelled";
    const key = last
      ? "gui.task_center.last_item"
      : "gui.task_center.current_item";
    return tr(key)
      .replace("{name}", pathBaseName(task.current) || task.current);
  }

  function taskControlsBusy(task: Task): boolean {
    return task.controlIntent !== null || task.queueMoveIntent !== null;
  }

  function canDragTask(task: Task): boolean {
    return (
      task.state === "queued" &&
      task.queuePosition !== null &&
      queuePositions.size > 1 &&
      !taskControlsBusy(task)
    );
  }

  function clearQueueDrag(): void {
    draggedTaskId = null;
    dropTargetId = null;
    dropEdge = null;
    dragPointerId = null;
    dragHandle = null;
  }

  function startQueueDrag(event: PointerEvent, task: Task): void {
    if (!canDragTask(task) || event.button !== 0) return;
    const handle = event.currentTarget;
    if (!(handle instanceof HTMLElement)) return;
    event.preventDefault();
    event.stopPropagation();
    draggedTaskId = task.id;
    dropTargetId = null;
    dropEdge = null;
    dragPointerId = event.pointerId;
    dragHandle = handle;
    handle.setPointerCapture(event.pointerId);
  }

  function updateQueueDrop(event: PointerEvent): void {
    if (draggedTaskId === null || event.pointerId !== dragPointerId) return;
    event.preventDefault();
    event.stopPropagation();
    const row = document
      .elementFromPoint(event.clientX, event.clientY)
      ?.closest<HTMLElement>("[data-queue-task-id]");
    const targetId = Number(row?.dataset.queueTaskId);
    const task = tasks.find((candidate) => candidate.id === targetId);
    if (!row || !task || !canDragTask(task) || task.id === draggedTaskId) {
      dropTargetId = null;
      dropEdge = null;
      return;
    }
    const bounds = row.getBoundingClientRect();
    dropTargetId = task.id;
    dropEdge = event.clientY < bounds.top + bounds.height / 2 ? "before" : "after";
  }

  function finishQueueDrag(event: PointerEvent): void {
    if (draggedTaskId === null || event.pointerId !== dragPointerId) return;
    event.preventDefault();
    event.stopPropagation();
    const sourceId = draggedTaskId;
    const targetId = dropTargetId;
    const edge = dropEdge;
    if (dragHandle?.hasPointerCapture(event.pointerId)) {
      dragHandle.releasePointerCapture(event.pointerId);
    }
    clearQueueDrag();
    if (targetId === null || !edge) return;
    const beforeId = taskQueueDropBeforeId(tasks, sourceId, targetId, edge);
    if (beforeId === undefined) return;
    const sourceTask = tasks.find((candidate) => candidate.id === sourceId);
    const beforeTask = beforeId === null
      ? null
      : tasks.find((candidate) => candidate.id === beforeId) ?? null;
    if (!sourceTask || (beforeId !== null && !beforeTask)) return;
    onMoveBefore(sourceTask, beforeTask);
  }

  function cancelQueueDrag(event: PointerEvent): void {
    if (event.pointerId !== dragPointerId) return;
    event.stopPropagation();
    clearQueueDrag();
  }

  function onWindowKeydown(event: KeyboardEvent): void {
    if (event.key !== "Escape" || !panel?.contains(document.activeElement)) return;
    event.preventDefault();
    event.stopPropagation();
    onClose();
  }

  function selectFilter(value: TaskCenterFilter): void {
    if (list) list.scrollTop = 0;
    onFilter(value);
  }

  $effect.pre(() => {
    void orderedRows;
    // A single listed task can finish without changing its row order.
    void counts;
    const focused = document.activeElement;
    if (!(focused instanceof HTMLElement) || !panel?.contains(focused)) return;
    const row = focused.closest(".task-center-row");
    void tick().then(() => {
      if (!panel?.isConnected || panel.closest("[inert]") || document.activeElement !== document.body) return;
      const details = row?.isConnected ? row.querySelector<HTMLButtonElement>("[data-task-details-id]") : null;
      (details ?? closeButton)?.focus({ preventScroll: true });
    });
  });

  function openDetails(task: Task, index: number, button: HTMLButtonElement): void {
    if (!list) return;
    onDetails(task, { taskId: task.id, index, offset: button.getBoundingClientRect().top - list.getBoundingClientRect().top });
  }

  async function focusList(): Promise<void> {
    await tick();
    if (!panel?.isConnected || !list) return;
    const position = takeListPosition(focusTaskId);
    let targetId = focusTaskId;
    if (targetId !== null && !orderedRows.some((task) => task.id === targetId)) {
      if (position) {
        targetId = orderedRows[Math.min(position.index, orderedRows.length - 1)]?.id ?? null;
      } else if (filter !== "all" && tasks.some((task) => task.id === targetId)) {
        onFilter("all");
        await tick();
      }
    }
    if (!panel?.isConnected || !list) return;
    const button = targetId === null ? null : list.querySelector<HTMLButtonElement>(`[data-task-details-id="${targetId}"]`);
    if (button && position) {
      const bounds = button.getBoundingClientRect();
      const offset = Math.max(0, Math.min(position.offset, list.clientHeight - bounds.height));
      list.scrollTop += bounds.top - list.getBoundingClientRect().top - offset;
      button.focus({ preventScroll: true });
    } else {
      const filterButton = position ? panel.querySelector<HTMLInputElement>(".task-center-filters input:checked") : null;
      (button ?? filterButton ?? closeButton)?.focus();
    }
  }

  onMount(() => { void focusList(); });
</script>

<svelte:window
  onkeydown={onWindowKeydown}
  onpointermove={updateQueueDrop}
  onpointerup={finishQueueDrag}
  onpointercancel={cancelQueueDrag}
/>

<aside
  id="squallz-task-center"
  bind:this={panel}
  class={rootClass}
  use:cssVariables={rootVariables}
  aria-labelledby="task-center-title"
>
  <header class="task-center-head">
    <span class="task-center-mark"><Icon name="list" size={21} /></span>
    <div>
      <h2 id="task-center-title">{tr("gui.task_center.title")}</h2>
      {#if filterCounts.all}<p aria-live="polite">{summary}</p>{/if}
    </div>
    <button
      bind:this={closeButton}
      class="task-center-close"
      type="button"
      aria-label={tr("gui.task_center.close")}
      title={tr("gui.task_center.close")}
      aria-keyshortcuts="Escape"
      onclick={onClose}
    ><Icon name="x" size={17} /></button>
  </header>

  {#if filterCounts.all || filter !== "all"}
    <div class="task-center-filters" role="radiogroup" aria-label={tr("gui.task_center.filters")}>
      {#each filters as option}
        <label>
          <input type="radio" name="task-center-filter" value={option} checked={filter === option} onchange={() => selectFilter(option)} />
          <span>{tr(`gui.task_center.filter_${option}`)}</span><b>{filterCounts[option]}</b>
        </label>
      {/each}
    </div>
  {/if}

  {#if tasks.some(taskStatusUnavailable)}
    <section class="task-status-notice" role="status">
      <Icon name="alert-triangle" size={16} />
      <p>{taskStatusUnavailableMessage()}</p>
    </section>
  {/if}

  <section bind:this={list} class="task-center-list" aria-label={tr("gui.task_center.list")}>
    {#if submittingTask && showSubmitting}
      {@const submittingTitle = taskTitleLabel(submittingTask)}
      {@const submittingKind = taskKindLabel(submittingTask)}
      <article class="task-center-row" data-task-state="submitting">
        <header>
          <div>
            <strong class="task-center-title" title={submittingTitle}>{submittingTitle}</strong>
            <span class="task-center-kind" title={submittingKind}>{submittingKind}</span>
          </div>
          <small>{taskOutcomeStateLabel(submittingTask)}</small>
        </header>
        <div class="task-center-pending">
          <Icon name="hourglass" size={15} />
          <span>{tr("gui.task_center.submitting")}</span>
        </div>
      </article>
    {/if}

    {#each orderedRows as task, index (task.id)}
      {@const tone = taskTone(task)}
      {@const taskTitle = taskTitleLabel(task)}
      {@const taskKind = taskKindLabel(task)}
      {@const queuePosition = queuePositions.get(task.id) ?? null}
      {@const showCpuResource = task.cpuThreads > 1 && (
        task.state === "queued" || task.state === "running" || task.state === "paused"
      )}
      {@const showBufferResource = task.streamBufferLimitBytes !== null && (
        task.state === "queued" || task.state === "running" || task.state === "paused"
      )}
      <article
        class="task-center-row"
        class:task-center-row-dragging={draggedTaskId === task.id}
        class:task-center-row-drop-before={dropTargetId === task.id && dropEdge === "before"}
        class:task-center-row-drop-after={dropTargetId === task.id && dropEdge === "after"}
        data-task-state={tone}
        data-queue-task-id={queuePosition ? task.id : undefined}
        aria-busy={taskControlsBusy(task) ? "true" : undefined}
      >
        <header>
          <div>
            <button class="task-center-title task-center-open" type="button" data-task-details-id={task.id}
              aria-label={tr("gui.task_center.open_details").replace("{name}", taskTitle)}
              onclick={(event) => openDetails(task, index, event.currentTarget)}>
              <span>{taskTitle}</span><Icon name="chevron-right" size={14} />
            </button>
          </div>
          <small class={`state-${tone}`}>{taskState(task)}</small>
        </header>
        <div class="task-center-tags">
          <span>{taskKind}</span>
          {#if task.origin === "file_manager"}
            <span>{tr("gui.task_center.origin_file_manager")}</span>
          {/if}
          {#if showCpuResource}
            <span>{taskCpuLabel(task)}</span>
          {/if}
          {#if showBufferResource}
            <span>{taskBufferLabel(task)}</span>
          {/if}
        </div>

        {#if (task.state === "running" || task.state === "paused") && task.total > 0 && !taskOverallProgressIndeterminate(task)}
          <progress
            class="task-center-progress"
            value={taskProgressPercent(task)}
            max="100"
            aria-label={tr("gui.task_center.progress_for").replace("{name}", taskTitle)}
          ></progress>
        {/if}

        <p>{taskDetail(task, queuePosition)}</p>
        {#if interactionDetail(task)}
          <p class="task-center-interaction">
            <Icon name={task.interaction === "password" ? "lock" : "alert-triangle"} size={14} />
            {interactionDetail(task)}
          </p>
        {/if}
        {#if taskCurrentItem(task)}
          <span class="task-center-current">{taskCurrentItem(task)}</span>
        {/if}
        {#if taskActionFailureMessage(task)}
          <p class="task-action-error" role="alert">{taskActionFailureMessage(task)}</p>
        {/if}

        <div
          class="task-center-actions"
          role="group"
          aria-label={tr("gui.task_center.actions_for").replace("{name}", taskTitleLabel(task))}
        >
          {#if (task.state === "running" && task.interruptible && task.pausable) || task.state === "paused"}
            {@const paused = task.state === "paused"}
            <button
              type="button"
              aria-disabled={taskControlsBusy(task) || undefined}
              onclick={() => { if (!taskControlsBusy(task)) (paused ? onResume : onPause)(task); }}
            >
              <Icon name={paused ? "play" : "pause"} size={14} />{paused ? taskResumeButtonLabel(task) : taskPauseButtonLabel(task)}
            </button>
          {/if}
          {#if queuePosition && queuePositions.size > 1}
            <div
              class="task-center-reorder"
              role="group"
              aria-label={tr("gui.task_center.reorder_for").replace("{name}", taskTitleLabel(task))}
            >
              <span
                class="task-center-drag-handle"
                class:busy={task.queueMoveIntent === "position"}
                aria-hidden="true"
                title={tr("gui.task_center.drag_handle")}
                onpointerdown={(event) => startQueueDrag(event, task)}
              ><Icon name={task.queueMoveIntent === "position" ? "hourglass" : "grip-vertical"} size={14} /></span>
              <button
                type="button"
                disabled={!queuePosition?.canMoveEarlier || taskControlsBusy(task)}
                aria-label={tr("gui.task_center.move_earlier")}
                title={tr("gui.task_center.move_earlier")}
                onclick={() => onMoveEarlier(task)}
              ><Icon name={task.queueMoveIntent === "earlier" ? "hourglass" : "chevron-up"} size={14} /></button>
              <button
                type="button"
                disabled={!queuePosition?.canMoveLater || taskControlsBusy(task)}
                aria-label={tr("gui.task_center.move_later")}
                title={tr("gui.task_center.move_later")}
                onclick={() => onMoveLater(task)}
              ><Icon name={task.queueMoveIntent === "later" ? "hourglass" : "chevron-down"} size={14} /></button>
            </div>
          {/if}
          {#if (task.state === "running" || task.state === "paused" || task.state === "queued") && task.interruptible}
            <button
              class="danger"
              type="button"
              aria-disabled={taskControlsBusy(task) || undefined}
              onclick={() => { if (!taskControlsBusy(task)) onCancel(task); }}
            ><Icon name={task.controlIntent === "cancel" ? "hourglass" : "x-circle"} size={14} />{taskCancelButtonLabel(task)}</button>
          {/if}
        </div>
      </article>
    {:else}
      {#if !showSubmitting}
        <section class="task-center-empty">
          <span><Icon name="check-circle" size={20} /></span>
          <div>
            {#if filterCounts.all}
              <strong>{tr("gui.task_center.empty_filter")}</strong>
            {:else}
              <strong>{tr("gui.task_center.empty_title")}</strong>
              <p>{tr("gui.task_center.empty_detail")}</p>
            {/if}
          </div>
        </section>
      {/if}
    {/each}
  </section>

  {#if notice || canClear}
    <footer class="task-center-footer">
      {#if notice}<div class="surface-notice" role="status">{notice}</div>{/if}
      {#if canClear}
        <button type="button" onclick={onClear}>
          {tr("gui.task_center.clear_completed")
            .replace("{count}", counts.clearable.toLocaleString())}
        </button>
      {/if}
    </footer>
  {/if}
</aside>
