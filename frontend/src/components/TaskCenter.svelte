<script lang="ts">
  import { onMount, tick } from "svelte";
  import Icon from "./Icon.svelte";
  import { cssVariables, type CssVariableMap } from "../lib/css-variables";
  import type { Task } from "../lib/jobs.svelte";
  import {
    taskCenterCounts,
    taskCenterRows,
    taskQueueDropBeforeId,
    taskQueuePosition,
    type TaskQueueDropEdge,
  } from "../lib/task-center";
  import {
    taskCancelButtonLabel,
    taskDialogResultSummary,
    taskKindLabel,
    taskOutcomeStateLabel,
    taskOutcomeStateTone,
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
    submittingTask = null,
    rootClass,
    rootVariables = {},
    focusTaskId = null,
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
    submittingTask?: TaskDialogModel | null;
    rootClass: string;
    rootVariables?: CssVariableMap;
    focusTaskId?: number | null;
    onClose: () => void;
    onPause: TaskAction;
    onResume: TaskAction;
    onMoveEarlier: TaskAction;
    onMoveLater: TaskAction;
    onMoveBefore: TaskMoveBeforeAction;
    onCancel: TaskAction;
    onDetails: TaskAction;
    onClear: () => void;
  } = $props();

  let panel: HTMLElement | null = null;
  let closeButton: HTMLButtonElement | null = null;
  let orderedRows = $derived(taskCenterRows(tasks));
  let counts = $derived(taskCenterCounts(tasks));
  let activeCount = $derived(counts.active + (submittingTask ? 1 : 0));
  let reorderableCount = $derived(
    tasks.filter((task) => task.state === "queued" && task.queuePosition !== null).length,
  );
  let draggedTaskId = $state<number | null>(null);
  let dropTargetId = $state<number | null>(null);
  let dropEdge = $state<TaskQueueDropEdge | null>(null);
  let dragPointerId: number | null = null;
  let dragHandle: HTMLElement | null = null;

  function countSummary(): string {
    if (counts.attention > 0) {
      return tr("gui.task_center.summary_attention", "{count} need attention")
        .replace("{count}", counts.attention.toLocaleString());
    }
    if (activeCount > 0 || counts.waiting > 0) {
      return tr("gui.task_center.summary_active", "{active} active · {waiting} waiting")
        .replace("{active}", activeCount.toLocaleString())
        .replace("{waiting}", counts.waiting.toLocaleString());
    }
    if (counts.completed > 0) {
      return tr("gui.task_center.summary_completed", "{count} recent tasks")
        .replace("{count}", counts.completed.toLocaleString());
    }
    return tr("gui.task_center.summary_idle", "No tasks");
  }

  function queuedDetail(task: Task): string {
    const position = taskQueuePosition(tasks, task.id);
    if (!position) return tr("gui.task_center.waiting", "Waiting in the queue");
    if (task.queueWaitReason === "cpu_budget") {
      return tr(
        "gui.task_center.waiting_cpu",
        "Waiting for CPU capacity · queue #{position}",
      ).replace("{position}", position.position.toLocaleString());
    }
    if (task.queueWaitReason === "parallel_limit") {
      return tr(
        "gui.task_center.waiting_slot",
        "Waiting for a task slot · queue #{position}",
      ).replace("{position}", position.position.toLocaleString());
    }
    if (task.queueWaitReason === null) {
      return tr("gui.task_center.waiting_next", "Ready to start");
    }
    return tr("gui.task_center.waiting_position", "Waiting · #{position} in the queue")
      .replace("{position}", position.position.toLocaleString());
  }

  function taskCpuLabel(task: Task): string {
    return tr("gui.task_center.resource_cpu", "CPU × {count}")
      .replace("{count}", task.cpuThreads.toLocaleString());
  }

  function taskBufferLabel(task: Task): string {
    return tr("gui.task_center.resource_buffer", "Stream buffer cap {size}")
      .replace("{size}", formatBytes(task.streamBufferLimitBytes));
  }

  function interactionDetail(task: Task): string | null {
    if (task.interaction === "password") {
      return task.ownedByRequester
        ? tr("gui.task_center.password_waiting", "Waiting for a password")
        : tr("gui.task_center.password_task_window", "Waiting for a password in its task window");
    }
    if (task.interaction === "conflict") {
      return task.ownedByRequester
        ? tr("gui.task_center.conflict_waiting", "Waiting for a file decision")
        : tr("gui.task_center.conflict_task_window", "Waiting for a file decision in its task window");
    }
    return null;
  }

  function taskDetail(task: Task): string {
    if (task.state === "queued") return queuedDetail(task);
    if (task.state === "done" || task.state === "failed" || task.state === "cancelled") {
      return taskDialogResultSummary(task);
    }
    return taskProgressSummary(task);
  }

  function taskState(task: Task): string {
    return task.interaction === null
      ? taskOutcomeStateLabel(task)
      : tr("gui.task_center.needs_input", "Needs input");
  }

  function taskTone(task: Task): string {
    return task.interaction === null ? taskOutcomeStateTone(task) : "warning";
  }

  function taskCurrentItem(task: Task): string | null {
    if (!task.current) return null;
    const key = task.state === "done" || task.state === "failed" || task.state === "cancelled"
      ? "gui.task_center.last_item"
      : "gui.task_center.current_item";
    const fallback = task.state === "done" || task.state === "failed" || task.state === "cancelled"
      ? "Last: {name}"
      : "Current: {name}";
    return tr(key, fallback)
      .replace("{name}", pathBaseName(task.current) || task.current);
  }

  function taskControlsBusy(task: Task): boolean {
    return task.controlIntent !== null || task.queueMoveIntent !== null;
  }

  function canDragTask(task: Task): boolean {
    return (
      task.state === "queued" &&
      task.queuePosition !== null &&
      reorderableCount > 1 &&
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

  onMount(() => {
    void tick().then(() => {
      const taskButton = focusTaskId === null
        ? null
        : panel?.querySelector<HTMLButtonElement>(`[data-task-details-id="${focusTaskId}"]`);
      (taskButton ?? closeButton)?.focus();
    });
  });
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
    <div>
      <span class="eyebrow">{tr("gui.task_center.eyebrow", "Smart scheduling")}</span>
      <h2 id="task-center-title">{tr("gui.task_center.title", "Task center")}</h2>
      <p aria-live="polite">{countSummary()}</p>
    </div>
    <button
      bind:this={closeButton}
      class="task-center-close"
      type="button"
      aria-label={tr("gui.task_center.close", "Close task center")}
      title={tr("gui.task_center.close", "Close task center")}
      aria-keyshortcuts="Escape"
      onclick={onClose}
    ><Icon name="x" size={17} /></button>
  </header>

  <div class="task-center-counts" aria-label={tr("gui.task_center.counts", "Task counts")}>
    <span><b>{activeCount}</b>{tr("gui.task_center.active", "Active")}</span>
    <span><b>{counts.waiting}</b>{tr("gui.task_center.waiting_count", "Waiting")}</span>
    <span class:attention={counts.attention > 0}><b>{counts.attention}</b>{tr("gui.task_center.attention", "Attention")}</span>
  </div>

  <p class="task-center-scope-note">
    <Icon name="info" size={14} />
    {tr("gui.task_center.scope", "App and file-manager tasks share one queue. Drag waiting tasks to reorder; arrow buttons remain available.")}
  </p>

  <div class="task-center-list" aria-label={tr("gui.task_center.list", "Tasks in this window")}>
    {#if submittingTask}
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
          <span>{tr("gui.task_center.submitting", "Adding to the queue...")}</span>
        </div>
      </article>
    {/if}

    {#each orderedRows as task (task.id)}
      {@const tone = taskTone(task)}
      {@const taskTitle = taskTitleLabel(task)}
      {@const taskKind = taskKindLabel(task)}
      {@const queuePosition = taskQueuePosition(tasks, task.id)}
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
            <strong class="task-center-title" title={taskTitle}>{taskTitle}</strong>
            <span class="task-center-kind" title={taskKind}>{taskKind}</span>
            {#if task.origin === "file_manager" || showCpuResource || showBufferResource}
              <div class="task-center-tags">
                {#if task.origin === "file_manager"}
                  <span class="task-center-origin">{tr("gui.task_center.origin_file_manager", "File manager")}</span>
                {/if}
                {#if showCpuResource}
                  <span class="task-center-resource">{taskCpuLabel(task)}</span>
                {/if}
                {#if showBufferResource}
                  <span class="task-center-resource">{taskBufferLabel(task)}</span>
                {/if}
              </div>
            {/if}
          </div>
          <small class={`state-${tone}`}>{taskState(task)}</small>
        </header>

        {#if (task.state === "running" || task.state === "paused") && task.total > 0}
          <div class="task-center-progress-line">
            <progress
              class="task-center-progress"
              value={taskProgressPercent(task)}
              max="100"
              aria-label={tr("gui.task_center.progress_for", "Progress for {name}").replace("{name}", taskTitleLabel(task))}
            ></progress>
            <strong>{taskProgressPercent(task)}%</strong>
          </div>
        {/if}

        <p>{taskDetail(task)}</p>
        {#if interactionDetail(task)}
          <p class="task-center-interaction">
            <Icon name={task.interaction === "password" ? "lock" : "alert-triangle"} size={14} />
            {interactionDetail(task)}
          </p>
        {/if}
        {#if taskCurrentItem(task)}
          <span class="task-center-current">{taskCurrentItem(task)}</span>
        {/if}

        <div
          class="task-center-actions"
          role="group"
          aria-label={tr("gui.task_center.actions_for", "Actions for {name}").replace("{name}", taskTitleLabel(task))}
        >
          {#if task.state === "running" && task.interruptible && task.pausable}
            <button type="button" disabled={taskControlsBusy(task)} onclick={() => onPause(task)}>
              <Icon name="pause" size={14} />{taskPauseButtonLabel(task)}
            </button>
          {:else if task.state === "paused"}
            <button type="button" disabled={taskControlsBusy(task)} onclick={() => onResume(task)}>
              <Icon name="play" size={14} />{taskResumeButtonLabel(task)}
            </button>
          {/if}
          {#if queuePosition && reorderableCount > 1}
            <div
              class="task-center-reorder"
              role="group"
              aria-label={tr("gui.task_center.reorder_for", "Queue position for {name}").replace("{name}", taskTitleLabel(task))}
            >
              <span
                class="task-center-drag-handle"
                class:busy={task.queueMoveIntent === "position"}
                aria-hidden="true"
                title={tr("gui.task_center.drag_handle", "Drag to reorder")}
                onpointerdown={(event) => startQueueDrag(event, task)}
              ><Icon name={task.queueMoveIntent === "position" ? "hourglass" : "grip-vertical"} size={14} /></span>
              <button
                type="button"
                disabled={!queuePosition?.canMoveEarlier || taskControlsBusy(task)}
                aria-label={tr("gui.task_center.move_earlier", "Move earlier")}
                title={tr("gui.task_center.move_earlier", "Move earlier")}
                onclick={() => onMoveEarlier(task)}
              ><Icon name={task.queueMoveIntent === "earlier" ? "hourglass" : "chevron-up"} size={14} /></button>
              <button
                type="button"
                disabled={!queuePosition?.canMoveLater || taskControlsBusy(task)}
                aria-label={tr("gui.task_center.move_later", "Move later")}
                title={tr("gui.task_center.move_later", "Move later")}
                onclick={() => onMoveLater(task)}
              ><Icon name={task.queueMoveIntent === "later" ? "hourglass" : "chevron-down"} size={14} /></button>
            </div>
          {/if}
          <button
            class="primary-lite"
            type="button"
            data-task-details-id={task.id}
            onclick={() => onDetails(task)}
          ><Icon name="external-link" size={14} />{tr("gui.task_center.details", "Details")}</button>
          {#if (task.state === "running" || task.state === "paused" || task.state === "queued") && task.interruptible}
            <button
              class="danger"
              type="button"
              disabled={taskControlsBusy(task)}
              onclick={() => onCancel(task)}
            ><Icon name={task.controlIntent === "cancel" ? "hourglass" : "x-circle"} size={14} />{taskCancelButtonLabel(task)}</button>
          {/if}
        </div>
      </article>
    {:else}
      {#if !submittingTask}
        <section class="task-center-empty">
          <span><Icon name="check-circle" size={20} /></span>
          <div>
            <strong>{tr("gui.task_center.empty_title", "No tasks yet")}</strong>
            <p>{tr("gui.task_center.empty_detail", "Compression, extraction, tests, and checksums appear here.")}</p>
          </div>
        </section>
      {/if}
    {/each}
  </div>

  <footer class="task-center-footer">
    {#if counts.clearable > 0}
      <button type="button" onclick={onClear}>
        {tr("gui.task_center.clear_completed", "Clear completed ({count})")
          .replace("{count}", counts.clearable.toLocaleString())}
      </button>
    {:else}
      <span>{tr("gui.task_center.attention_kept", "Failed tasks and results that need review stay here.")}</span>
    {/if}
  </footer>
</aside>
