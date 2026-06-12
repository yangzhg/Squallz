<script lang="ts">
  import Icon from "./Icon.svelte";
  import type { TaskDialogModel } from "../lib/task-dialog";
  import {
    checksumItemStatus,
    checksumItemText,
    hasTaskCurrentProgress,
    isTaskActiveState,
    taskCancelButtonLabel,
    taskChecksumItems,
    taskControlCalloutDetail,
    taskControlCalloutTitle,
    taskControlCalloutVisible,
    taskCurrentLabel,
    taskCurrentProgressBadge,
    taskCurrentProgressPercent,
    taskCurrentProgressSource,
    taskCurrentProgressSummary,
    taskCurrentSectionVisible,
    taskCurrentSectionLabel,
    taskDialogEyebrow,
    taskDialogResultSummary,
    taskKindLabel,
    taskTitleLabel,
    taskNextStepDetail,
    taskOpenOutputLabel,
    taskOutputIsFolder,
    taskPauseButtonLabel,
    taskProgressPercent,
    taskProgressSummary,
    taskResumeButtonLabel,
    taskResultActionLabel,
    taskResultAvailableForSurface,
    taskResultDetailRows,
    taskResultDetailTitle,
    taskStateLabel,
    tr,
  } from "../lib/task-dialog";
  import { basename as pathBaseName } from "../lib/format";

  type TaskAction = (task: TaskDialogModel) => void;
  type TaskAsyncAction = (task: TaskDialogModel) => void | Promise<void>;

  let {
    task,
    rootClass,
    rootStyle = "",
    copyFeedback = null,
    copyFeedbackTone = null,
    taskOutputPath,
    taskRevealOutputLabel,
    taskWindowMode,
    onPause,
    onResume,
    onCancel,
    onCopyChecksumResults,
    onOpenOutput,
    onViewResults,
    onRevealOutput,
    onDismiss,
  }: {
    task: TaskDialogModel;
    rootClass: string;
    rootStyle?: string;
    copyFeedback?: string | null;
    copyFeedbackTone?: "success" | "danger" | null;
    taskOutputPath: (task: TaskDialogModel) => string | null;
    taskRevealOutputLabel: () => string;
    taskWindowMode: boolean;
    onPause: TaskAction;
    onResume: TaskAction;
    onCancel: TaskAction;
    onCopyChecksumResults: TaskAsyncAction;
    onOpenOutput: TaskAsyncAction;
    onViewResults: TaskAction;
    onRevealOutput: TaskAsyncAction;
    onDismiss: TaskAsyncAction;
  } = $props();
</script>

<section class={rootClass} style={rootStyle} role="presentation">
  <div
    class="task-modal-card"
    data-task-state={task.state}
    data-task-active={isTaskActiveState(task.state) ? "true" : "false"}
    role="dialog"
    aria-modal="true"
    aria-labelledby="task-modal-title"
  >
    <header class="task-modal-head">
      <div>
        <span class="eyebrow">{taskDialogEyebrow(task)}</span>
        <h2 id="task-modal-title">{taskTitleLabel(task)}</h2>
        <p>{taskKindLabel(task)} · {taskStateLabel(task.state)}</p>
      </div>
      <strong class={`task-modal-state state-${task.state}`}>{taskStateLabel(task.state)}</strong>
    </header>

    <div class="task-modal-progress-stack">
      <section class="task-modal-progress-block" data-task-progress-kind="overall">
        <div class="task-modal-progress-line">
          <span>{tr("gui.task.overall_progress", "Overall progress")}</span>
          <strong>{taskProgressPercent(task)}%</strong>
        </div>
        <progress
          data-task-progress="overall"
          value={taskProgressPercent(task)}
          max="100"
          aria-label={tr("gui.task.overall_progress", "Overall progress")}
        ></progress>
        <p>{taskProgressSummary(task)}</p>
      </section>

      {#if taskCurrentSectionVisible(task)}
        <section
          class="task-modal-progress-block"
          data-task-progress-kind="current-file"
          data-task-progress-source={taskCurrentProgressSource(task)}
        >
          <div class="task-modal-progress-line">
            <span>{taskCurrentSectionLabel(task)}</span>
            <strong>{taskCurrentProgressBadge(task)}</strong>
          </div>
          {#if hasTaskCurrentProgress(task)}
            <progress
              data-task-progress="current-file"
              data-task-progress-source="engine-bytes"
              value={taskCurrentProgressPercent(task)}
              max="100"
              aria-label={taskCurrentSectionLabel(task)}
            ></progress>
          {:else}
            <div
              class="task-current-pending"
              class:active={isTaskActiveState(task.state)}
              data-task-progress-source="pending"
            aria-live="polite"
          >
            <span>{taskCurrentLabel(task)}</span>
            {#if isTaskActiveState(task.state)}
              <small>{taskCurrentProgressBadge(task)}</small>
            {/if}
          </div>
          {/if}
          <p>{taskCurrentProgressSummary(task)}</p>
        </section>
      {/if}
    </div>

    {#if taskControlCalloutVisible(task)}
      <section class="task-control-callout" class:attention={task.controlIntent !== null} aria-live="polite">
        <Icon name={task.controlIntent === "cancel" ? "hourglass" : "info"} size={16} />
        <div>
          <strong>{taskControlCalloutTitle(task)}</strong>
          <span>{taskControlCalloutDetail(task)}</span>
        </div>
      </section>
    {/if}

    {#if !isTaskActiveState(task.state)}
      <section class="task-result-callout" class:danger={task.state === "failed"}>
        <strong>{tr("gui.task.result", "Result")}</strong>
        <span>{taskDialogResultSummary(task)}</span>
      </section>
      {#if (task.spec.kind === "checksum" || task.spec.kind === "checksum_check") && taskChecksumItems(task).length > 0}
        {@const checksumRows = taskChecksumItems(task).slice(0, task.expanded ? 20 : 6)}
        <section class="task-checksum-result" aria-label={tr("gui.task.checksum_results", "Checksum results")}>
          <div class="task-checksum-head">
            <div>
              <strong>{tr("gui.task.checksum_results", "Checksum results")}</strong>
              <span>{tr("gui.checksum.result_rows", "{count} rows").replace("{count}", taskChecksumItems(task).length.toLocaleString())}</span>
              {#if copyFeedback}
                <small class="checksum-copy-status" class:danger={copyFeedbackTone === "danger"} role="status">{copyFeedback}</small>
              {/if}
            </div>
            <button type="button" class="primary-lite" onclick={() => void onCopyChecksumResults(task)}>
              <Icon name="list" size={14} />{tr("gui.checksum.copy_results", "Copy results")}
            </button>
          </div>
          <div class="task-checksum-table">
            <div><b>{tr("gui.checksum.result", "Checksum result")}</b><b>{tr("gui.checksum.digest", "Digest")}</b><b>{tr("common.status", "Status")}</b></div>
            {#each checksumRows as item}
              <div>
                <span>{pathBaseName(checksumItemText(item, "path")) || checksumItemText(item, "path")}</span>
                <code class="checksum-digest">{checksumItemText(item, task.spec.kind === "checksum" ? "digest" : "actual") || checksumItemText(item, "expected") || checksumItemText(item, "error")}</code>
                <strong>{checksumItemStatus(item)}</strong>
              </div>
            {/each}
          </div>
        </section>
      {/if}
      {@const detailRows = taskResultDetailRows(task)}
      {#if task.expanded && detailRows.length > 0}
        <section class="task-result-details" aria-label={taskResultDetailTitle(task)}>
          <strong>{taskResultDetailTitle(task)}</strong>
          <div class="task-result-detail-list">
            {#each detailRows as row}
              <div>
                <span>{row.label}</span>
                <code>{row.value}</code>
              </div>
            {/each}
          </div>
        </section>
      {/if}
      <section class="task-next-step" aria-label={tr("gui.task.next_step", "Next step")}>
        <div>
          <strong>{tr("gui.task.next_step", "Next step")}</strong>
          <span>{taskNextStepDetail(task, taskWindowMode)}</span>
        </div>
      </section>
    {/if}

    <footer class="task-modal-actions">
      {#if task.state === "submitting"}
        <button type="button" disabled>
          <Icon name="hourglass" size={15} />{tr("gui.task.starting", "Starting...")}
        </button>
      {:else}
        {#if task.state === "running"}
          <button type="button" disabled={task.controlIntent !== null} onclick={() => onPause(task)}>
            <Icon name="pause" size={15} />{taskPauseButtonLabel(task)}
          </button>
        {/if}
        {#if task.state === "paused"}
          <button type="button" disabled={task.controlIntent !== null} onclick={() => onResume(task)}>
            <Icon name="play" size={15} />{taskResumeButtonLabel(task)}
          </button>
        {/if}
        {#if isTaskActiveState(task.state)}
          <button class="danger" type="button" disabled={task.controlIntent === "cancel"} onclick={() => onCancel(task)}>
            <Icon name={task.controlIntent === "cancel" ? "hourglass" : "x-circle"} size={15} />{taskCancelButtonLabel(task)}
          </button>
        {:else}
          {@const outputPath = taskOutputPath(task)}
          {#if outputPath}
            <button class="primary" type="button" onclick={() => void onOpenOutput(task)}>
              <Icon name={taskOutputIsFolder(task) ? "folder-open" : "external-link"} size={15} />{taskOpenOutputLabel(task)}
            </button>
          {/if}
          {#if taskResultAvailableForSurface(task, taskWindowMode)}
            <button class={outputPath ? "primary-lite" : "primary"} type="button" onclick={() => onViewResults(task)}>
              <Icon name="list" size={15} />{taskResultActionLabel(task)}
            </button>
          {/if}
          {#if task.revealPath}
            <button type="button" onclick={() => void onRevealOutput(task)}>
              <Icon name="folder-open" size={15} />{taskRevealOutputLabel()}
            </button>
          {/if}
          <button type="button" onclick={() => void onDismiss(task)}>
            {tr("gui.task.close", "Close")}
          </button>
        {/if}
      {/if}
    </footer>
  </div>
</section>
