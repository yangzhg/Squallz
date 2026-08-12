<script lang="ts">
  import { tick } from "svelte";
  import AppIcon from "./AppIcon.svelte";
  import Icon from "./Icon.svelte";
  import { cssVariables, type CssVariableMap } from "../lib/css-variables";
  import type { TaskConflictDecision, TaskDialogModel } from "../lib/task-dialog";
  import {
    checksumItemStatus,
    checksumItemText,
    hasTaskCurrentProgress,
    isTaskActiveState,
    isTaskProgressingState,
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
    taskErrorDetailsActionLabel,
    taskErrorDetailsAvailable,
    taskFailureReviewActionLabel,
    taskFailureReviewAvailable,
    taskFailureReviewScreen,
    taskTitleLabel,
    taskNextStepDetail,
    taskOpenOutputLabel,
    taskOutcomeNeedsAttention,
    taskOutcomeStateLabel,
    taskOutcomeStateTone,
    taskOverallProgressBadge,
    taskOverallProgressIndeterminate,
    taskOverallProgressLabel,
    taskOutputCanOpen,
    taskOutputIsFolder,
    taskPauseButtonLabel,
    taskProgressPercent,
    taskProgressSummary,
    taskPhaseControlNoticeDetail,
    taskPhaseControlNoticeTitle,
    taskPhaseControlNoticeVisible,
    taskResumeButtonLabel,
    taskResultActionLabel,
    taskResultAvailableForSurface,
    taskResultDetailRows,
    taskResultDetailTitle,
    taskHasInlineResults,
    tr,
  } from "../lib/task-dialog";
  import { taskCanPublishMacosSfx } from "../lib/macos-sfx-publish";
  import { basename as pathBaseName } from "../lib/format";

  type TaskAction = (task: TaskDialogModel) => void;
  type TaskAsyncAction = (task: TaskDialogModel) => void | Promise<void>;
  type PasswordQuestion = {
    name: string;
    detail: string;
    sessionDetail: string;
  };
  type ConflictQuestion = {
    path: string;
    existing: string;
    incoming: string;
  };

  let {
    task,
    rootClass,
    rootId = undefined,
    rootVariables = {},
    copyFeedback = null,
    copyFeedbackTone = null,
    passwordQuestion = null,
    passwordValue = "",
    passwordError = null,
    conflictQuestion = null,
    conflictApplyAll = false,
    presentation = "dialog",
    taskOutputPath,
    taskRevealOutputLabel,
    taskWindowMode,
    macosSfxPublishingAvailable,
    onPause,
    onResume,
    onCancel,
    onCopyChecksumResults,
    onOpenOutput,
    onPublishMacosSfx,
    onReviewFailure,
    onToggleDetails,
    onViewResults,
    onRevealOutput,
    onDismiss,
    onPasswordValueChange,
    onSubmitPassword,
    onCancelPassword,
    onConflictApplyAllChange,
    onAnswerConflict,
  }: {
    task: TaskDialogModel;
    rootClass: string;
    rootId?: string;
    rootVariables?: CssVariableMap;
    copyFeedback?: string | null;
    copyFeedbackTone?: "success" | "danger" | null;
    passwordQuestion?: PasswordQuestion | null;
    passwordValue?: string;
    passwordError?: string | null;
    conflictQuestion?: ConflictQuestion | null;
    conflictApplyAll?: boolean;
    presentation?: "dialog" | "panel";
    taskOutputPath: (task: TaskDialogModel) => string | null;
    taskRevealOutputLabel: () => string;
    taskWindowMode: boolean;
    macosSfxPublishingAvailable: boolean;
    onPause: TaskAction;
    onResume: TaskAction;
    onCancel: TaskAction;
    onCopyChecksumResults: TaskAsyncAction;
    onOpenOutput: TaskAsyncAction;
    onPublishMacosSfx: TaskAsyncAction;
    onReviewFailure: TaskAction;
    onToggleDetails: TaskAction;
    onViewResults: TaskAction;
    onRevealOutput: TaskAsyncAction;
    onDismiss: TaskAsyncAction;
    onPasswordValueChange: (value: string) => void;
    onSubmitPassword: () => void | Promise<void>;
    onCancelPassword: () => void;
    onConflictApplyAllChange: (applyAll: boolean) => void;
    onAnswerConflict: (decision: TaskConflictDecision, applyAll: boolean) => void;
  } = $props();

  let taskCard = $state<HTMLElement | null>(null);
  let passwordInput = $state<HTMLInputElement | null>(null);
  let titleId = $derived(taskElementId("title"));
  let errorDetailsId = $derived(taskElementId("error-details"));
  let createDetailsId = $derived(taskElementId("create-details"));
  let passwordErrorId = $derived(taskElementId("password-error"));

  function taskElementId(suffix: string): string {
    const taskId = task.id === null ? "submitting" : task.id.toString();
    return `task-${presentation}-${taskId}-${suffix}`;
  }

  function displayedTaskState(): string {
    return task.interaction === null
      ? taskOutcomeStateLabel(task)
      : tr("gui.task_center.needs_input", "Needs input");
  }

  function displayedTaskTone(): string {
    return task.interaction === null ? taskOutcomeStateTone(task) : "warning";
  }

  function resultIconName(): string {
    if (task.state === "failed" || task.state === "cancelled") return "x-circle";
    return taskOutcomeNeedsAttention(task) ? "alert-triangle" : "check-circle";
  }

  function interactionTitle(): string {
    return task.interaction === "password"
      ? tr("gui.task.interaction.password_title", "Password needed")
      : tr("gui.task.interaction.conflict_title", "File decision needed");
  }

  function interactionDetail(): string {
    if (task.interaction === "password") {
      return task.ownedByRequester
        ? tr("gui.task.interaction.password_waiting", "This task is waiting for a password.")
        : tr("gui.task.interaction.password_task_window", "Enter the password in the separate task window opened by your file manager.");
    }
    return task.ownedByRequester
      ? tr("gui.task.interaction.conflict_waiting", "This task is waiting for a file decision.")
      : tr("gui.task.interaction.conflict_task_window", "Choose how to handle the file in the separate task window opened by your file manager.");
  }

  function focusableElements(): HTMLElement[] {
    if (!taskCard) return [];
    return Array.from(
      taskCard.querySelectorAll<HTMLElement>(
        'button:not(:disabled), input:not(:disabled), select:not(:disabled), textarea:not(:disabled), [href], [tabindex]:not([tabindex="-1"])',
      ),
    ).filter((element) => !element.hasAttribute("hidden"));
  }

  function onTaskCardKeydown(event: KeyboardEvent): void {
    if (presentation !== "dialog" || event.key !== "Tab") return;
    const focusable = focusableElements();
    if (focusable.length === 0) {
      event.preventDefault();
      taskCard?.focus();
      return;
    }
    const first = focusable[0];
    const last = focusable[focusable.length - 1];
    if (document.activeElement === taskCard) {
      event.preventDefault();
      (event.shiftKey ? last : first).focus();
    } else if (event.shiftKey && document.activeElement === first) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault();
      first.focus();
    }
  }

  $effect(() => {
    const focusIdentity = `${presentation}:${task.id ?? "submitting"}:${Boolean(passwordQuestion)}:${Boolean(conflictQuestion)}`;
    void focusIdentity;
    void tick().then(() => (passwordInput ?? taskCard)?.focus());
  });

  function scrollResultList(event: KeyboardEvent): void {
    const target = event.currentTarget;
    if (!(target instanceof HTMLElement)) return;
    if (target.scrollHeight <= target.clientHeight) return;
    const lineStep = target.clientHeight / 4;
    let nextTop: number;
    switch (event.key) {
      case "ArrowDown":
        nextTop = target.scrollTop + lineStep;
        break;
      case "ArrowUp":
        nextTop = target.scrollTop - lineStep;
        break;
      case "PageDown":
        nextTop = target.scrollTop + target.clientHeight;
        break;
      case " ":
        nextTop = target.scrollTop + (event.shiftKey ? -target.clientHeight : target.clientHeight);
        break;
      case "PageUp":
        nextTop = target.scrollTop - target.clientHeight;
        break;
      case "Home":
        nextTop = 0;
        break;
      case "End":
        nextTop = target.scrollHeight;
        break;
      default:
        return;
    }
    event.preventDefault();
    event.stopPropagation();
    target.scrollTop = nextTop;
  }
</script>

<section
  id={rootId}
  class={rootClass}
  use:cssVariables={rootVariables}
  role={presentation === "dialog" ? "presentation" : undefined}
  data-task-presentation={presentation}
>
  <div
    bind:this={taskCard}
    class="task-modal-card"
    data-task-state={task.state}
    data-task-outcome={displayedTaskTone()}
    data-task-active={isTaskProgressingState(task.state) ? "true" : "false"}
    role={presentation === "dialog" ? "dialog" : "region"}
    aria-modal={presentation === "dialog" ? "true" : undefined}
    aria-labelledby={titleId}
    tabindex="-1"
    onkeydown={onTaskCardKeydown}
  >
    <header class="task-modal-head">
      <div>
        <span class="task-modal-eyebrow">
          {#if taskWindowMode}
            <AppIcon class="task-modal-brand-icon" />
          {/if}
          <span class="eyebrow">
            {taskWindowMode
              ? tr("gui.external_task.eyebrow", "Squallz task")
              : taskDialogEyebrow(task)}
          </span>
        </span>
        <h2 id={titleId}>{taskTitleLabel(task)}</h2>
      </div>
      <strong class={`task-modal-state state-${displayedTaskTone()}`}>{displayedTaskState()}</strong>
    </header>

    <div class="task-modal-body">
    <div class="task-modal-progress-stack">
      <section class="task-modal-progress-block" data-task-progress-kind="overall">
        <div class="task-modal-progress-line">
          <span>{taskOverallProgressLabel(task)}</span>
          <strong>{taskOverallProgressBadge(task)}</strong>
        </div>
        {#if taskOverallProgressIndeterminate(task)}
          <progress
            data-task-progress="overall"
            data-task-progress-source={task.scanEntries != null ? "scan-entry" : "pending"}
            max="100"
            aria-label={taskProgressSummary(task)}
          ></progress>
        {:else}
          <progress
            data-task-progress="overall"
            data-task-progress-source={task.scanEntries != null ? "scan-entry" : task.total > 0 ? "engine-bytes" : "pending"}
            value={taskProgressPercent(task)}
            max="100"
            aria-label={taskProgressSummary(task)}
          ></progress>
        {/if}
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
            <p>{taskCurrentProgressSummary(task)}</p>
          {:else}
            <div
              class="task-current-pending"
              class:active={isTaskProgressingState(task.state)}
              data-task-progress-source={taskCurrentProgressSource(task)}
              aria-live="polite"
            >
              <span title={taskCurrentLabel(task)}>{taskCurrentLabel(task)}</span>
            </div>
          {/if}
        </section>
      {/if}
    </div>

    {#if taskPhaseControlNoticeVisible(task)}
      <section class="task-control-callout" aria-live="polite">
        <Icon name="lock" size={16} />
        <div>
          <strong>{taskPhaseControlNoticeTitle(task)}</strong>
          <span>{taskPhaseControlNoticeDetail(task)}</span>
        </div>
      </section>
    {/if}

    {#if task.interaction !== null && !passwordQuestion && !conflictQuestion}
      <section class="task-control-callout task-interaction-callout attention" aria-live="polite">
        <Icon name={task.interaction === "password" ? "lock" : "alert-triangle"} size={16} />
        <div>
          <strong>{interactionTitle()}</strong>
          <span>{interactionDetail()}</span>
        </div>
      </section>
    {/if}

    {#if passwordQuestion}
      <form
        class="task-question-card"
        aria-label={tr("gui.password.required", "Password required")}
        onsubmit={(event) => {
          event.preventDefault();
          void onSubmitPassword();
        }}
      >
        <header class="task-question-head">
          <span class="task-question-icon"><Icon name="lock" size={18} /></span>
          <div>
            <strong>{tr("gui.password.unlock_name", "Unlock {name}").replace("{name}", passwordQuestion.name)}</strong>
            <span>{passwordQuestion.detail}</span>
          </div>
        </header>
        <label class="task-question-field">
          <span>{tr("gui.password.password", "Password")}</span>
          <input
            bind:this={passwordInput}
            class="secure-input"
            type="password"
            value={passwordValue}
            autocomplete="current-password"
            aria-label={tr("gui.password.archive_password", "Archive password")}
            aria-invalid={passwordError ? "true" : undefined}
            aria-describedby={passwordError ? passwordErrorId : undefined}
            oninput={(event) => onPasswordValueChange(event.currentTarget.value)}
          />
        </label>
        {#if passwordError}
          <small id={passwordErrorId} class="task-question-error" role="alert">{passwordError}</small>
        {/if}
        <p class="task-question-note"><Icon name="info" size={14} />{passwordQuestion.sessionDetail}</p>
        <footer class="task-question-actions">
          <button type="button" onclick={onCancelPassword}>{tr("common.cancel", "Cancel")}</button>
          <button class="primary-lite" type="submit">{tr("gui.password.unlock_continue", "Unlock and continue")}</button>
        </footer>
      </form>
    {:else if conflictQuestion}
      <section class="task-question-card" aria-label={tr("gui.screen.conflict", "Conflict handling") }>
        <header class="task-question-head">
          <span class="task-question-icon warning"><Icon name="alert-triangle" size={18} /></span>
          <div>
            <strong>{tr("gui.conflict.one_item_exists", "1 item already exists")}</strong>
            <span>{tr("gui.conflict.task_paused", "This task is waiting for your conflict choice.")}</span>
          </div>
        </header>
        <div class="task-question-conflict">
          <strong>{conflictQuestion.path}</strong>
          <span><b>{tr("gui.conflict.existing", "Existing")}</b>{conflictQuestion.existing}</span>
          <span><b>{tr("gui.conflict.incoming", "Incoming")}</b>{conflictQuestion.incoming}</span>
        </div>
        <label class="conflict-apply-all">
          <input
            type="checkbox"
            checked={conflictApplyAll}
            onchange={(event) => onConflictApplyAllChange(event.currentTarget.checked)}
          />
          <span>{tr("gui.conflict.apply_remaining", "Apply this decision to remaining conflicts")}</span>
        </label>
        <footer class="task-question-actions">
          <button type="button" onclick={() => onAnswerConflict("abort", false)}>{tr("gui.conflict.cancel_extraction", "Cancel extraction")}</button>
          <button type="button" onclick={() => onAnswerConflict("skip", conflictApplyAll)}>{tr("gui.conflict.skip", "Skip")}</button>
          <button class="conflict-danger" type="button" onclick={() => onAnswerConflict("overwrite", conflictApplyAll)}>{tr("gui.conflict.overwrite", "Replace")}</button>
          <button class="primary-lite" type="button" onclick={() => onAnswerConflict("rename", conflictApplyAll)}>{tr("gui.conflict.rename", "Keep both")}</button>
        </footer>
      </section>
    {/if}

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
      <section
        class="task-result-callout"
        class:danger={task.state === "failed"}
        class:cancelled={task.state === "cancelled"}
        class:attention={taskOutcomeNeedsAttention(task)}
      >
        <span class="task-result-mark"><Icon name={resultIconName()} size={18} /></span>
        <div class="task-result-copy">
          <strong>{tr("gui.task.result", "Result")}</strong>
          <span>{taskDialogResultSummary(task)}</span>
        </div>
      </section>
      {#if taskErrorDetailsAvailable(task)}
        <div class="task-error-disclosure">
          <button
            class="secondary-lite"
            type="button"
            aria-expanded={task.expanded}
            aria-controls={task.expanded ? errorDetailsId : undefined}
            onclick={() => onToggleDetails(task)}
          >
            <Icon name={task.expanded ? "chevron-down" : "chevron-right"} size={15} />{taskErrorDetailsActionLabel(task)}
          </button>
        </div>
      {/if}
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
        <section
          id={task.state === "failed" ? errorDetailsId : taskHasInlineResults(task) ? createDetailsId : undefined}
          class="task-result-details"
          aria-label={taskResultDetailTitle(task)}
        >
          <strong>{taskResultDetailTitle(task)}</strong>
          <div class="task-result-detail-list">
            {#each detailRows as row}
              <div>
                <span>{row.label}</span>
                {#if row.scrollable}
                  <!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions (focusable scroll region) -->
                  <div
                    class="scrollable"
                    role="region"
                    aria-label={row.label}
                    tabindex="0"
                    onkeydown={scrollResultList}
                  ><code>{row.value}</code></div>
                {:else}
                  <code>{row.value}</code>
                {/if}
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
    </div>

    {#if !passwordQuestion && !conflictQuestion}
    <footer class="task-modal-actions">
      {#if task.state === "submitting"}
        <button type="button" disabled>
          <Icon name="hourglass" size={15} />{tr("gui.task.starting", "Starting...")}
        </button>
      {:else}
        {#if task.state === "running" && task.interruptible && task.pausable}
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
          {#if task.interruptible}
            <button class="danger" type="button" disabled={task.controlIntent === "cancel"} onclick={() => onCancel(task)}>
              <Icon name={task.controlIntent === "cancel" ? "hourglass" : "x-circle"} size={15} />{taskCancelButtonLabel(task)}
            </button>
          {/if}
          {#if presentation === "panel"}
            <button type="button" onclick={() => void onDismiss(task)}>
              {tr("gui.task.back_to_tasks", "Back to tasks")}
            </button>
          {/if}
        {:else}
          {@const outputPath = taskOutputPath(task)}
          {@const outputCanOpen = taskOutputCanOpen(task) && !(taskWindowMode && task.spec.kind === "compress")}
          {@const sfxPublishAvailable = macosSfxPublishingAvailable && taskCanPublishMacosSfx(task)}
          {@const primaryActionAvailable = outputCanOpen || sfxPublishAvailable}
          {@const failureReviewAvailable = taskFailureReviewAvailable(task, taskWindowMode)}
          {#if sfxPublishAvailable}
            <button class="primary" type="button" onclick={() => void onPublishMacosSfx(task)}>
              <Icon name="shield-check" size={15} />{tr("gui.task.publish_macos_sfx", "Publish for macOS")}
            </button>
          {/if}
          {#if outputPath && outputCanOpen}
            <button class="primary" type="button" onclick={() => void onOpenOutput(task)}>
              <Icon name={taskOutputIsFolder(task) ? "folder-open" : "external-link"} size={15} />{taskOpenOutputLabel(task)}
            </button>
          {/if}
          {#if failureReviewAvailable}
            <button class={primaryActionAvailable ? "primary-lite" : "primary"} type="button" onclick={() => onReviewFailure(task)}>
              <Icon name={taskFailureReviewScreen(task) === "recovery" ? "shield-alert" : "settings"} size={15} />{taskFailureReviewActionLabel(task)}
            </button>
          {/if}
          {#if taskResultAvailableForSurface(task, taskWindowMode)}
            <button
              class={primaryActionAvailable ? undefined : "primary"}
              type="button"
              aria-expanded={taskHasInlineResults(task) ? task.expanded : undefined}
              aria-controls={taskHasInlineResults(task) ? createDetailsId : undefined}
              onclick={() => onViewResults(task)}
            >
              <Icon name="list" size={15} />{taskResultActionLabel(task)}
            </button>
          {/if}
          {#if task.revealPath}
            <button type="button" onclick={() => void onRevealOutput(task)}>
              <Icon name="folder-open" size={15} />{taskRevealOutputLabel()}
            </button>
          {/if}
          <button type="button" onclick={() => void onDismiss(task)}>
            {presentation === "panel"
              ? tr("gui.task.back_to_tasks", "Back to tasks")
              : tr("gui.task.close", "Close")}
          </button>
        {/if}
      {/if}
    </footer>
    {/if}
  </div>
</section>
