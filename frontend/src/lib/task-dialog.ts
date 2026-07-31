import { basename as pathBaseName, formatBytes } from "./format";
import { errorSummary } from "./error-presentation";
import { t, tFallback } from "./i18n.svelte";
import { jobTitleFor } from "./job-title";
import { readCreateResult } from "./create-result";
import {
  readRecoveryProtectionResult,
  recoveryResultBoolean,
  recoveryResultNumber,
  recoveryResultOk,
} from "./recovery-result";
import { platformTrashName } from "./platform-labels";
import { readExtractResultCounts } from "./extract-result";
import {
  resultProblemMessages,
  resultProblemTotal,
} from "./problem-preview";
import {
  applyCreateDestinationAuthorization,
  checksumItemStatus,
  checksumItemText,
  checksumResultLine,
  isTaskActiveState,
  normalizeTaskConflictAnswer,
  sourceCleanupResult,
  taskChecksumItems,
  taskChecksumResultText,
  taskFailureReviewScreen,
  taskHasInlineResults,
  taskOutcomeNeedsAttention,
  taskOutputCanOpen,
  taskOutputIsFolder,
  taskOutputPath,
  taskPasswordReady,
  taskResultScreen,
  taskStateLabel,
  type SourceCleanupResult,
  type TaskConflictAnswer,
  type TaskConflictDecision,
  type TaskDialogModel,
  type TaskDialogState,
  type TaskResultScreen,
} from "./task-model";

export {
  applyCreateDestinationAuthorization,
  checksumItemStatus,
  checksumItemText,
  checksumResultLine,
  isTaskActiveState,
  normalizeTaskConflictAnswer,
  taskChecksumItems,
  taskChecksumResultText,
  taskFailureReviewScreen,
  taskHasInlineResults,
  taskOutcomeNeedsAttention,
  taskOutputCanOpen,
  taskOutputIsFolder,
  taskOutputPath,
  taskPasswordReady,
  taskResultScreen,
  taskStateLabel,
  type TaskConflictAnswer,
  type TaskConflictDecision,
  type TaskDialogModel,
  type TaskDialogState,
  type TaskResultScreen,
} from "./task-model";

export interface TaskResultDetailRow {
  label: string;
  value: string;
  scrollable?: boolean;
}

export function tr(key: string, fallback: string): string {
  return tFallback(key, fallback);
}

export function isTaskProgressingState(state: string | null | undefined): boolean {
  return state === "submitting" || state === "running" || state === "pausing";
}


function sourceCleanupStatusLabel(status: string): string {
  if (status === "completed") {
    return tFallback(
      "gui.task.source_cleanup.completed",
      "Originals moved to {trash}",
      { trash: platformTrashName() },
    );
  }
  if (status === "partial") return tr("gui.task.source_cleanup.partial", "Some originals were kept");
  if (status === "blocked") return tr("gui.task.source_cleanup.blocked", "Originals could not be moved");
  if (status === "cancelled") return tr("gui.task.source_cleanup.cancelled", "Moving originals was cancelled");
  if (status === "failed") return tr("gui.task.source_cleanup.failed", "Moving originals failed");
  if (status === "not_requested") return tr("gui.task.source_cleanup.not_requested", "Originals were left in place");
  return tr("gui.task.source_cleanup.unknown", "Could not confirm where the originals were left");
}

export function taskOutcomeStateLabel(task: TaskDialogModel): string {
  return taskOutcomeNeedsAttention(task)
    ? tr("gui.task.state.needs_attention", "Needs attention")
    : taskStateLabel(task.state);
}

export function taskOutcomeStateTone(task: TaskDialogModel): string {
  if (task.state === "failed") return "failed";
  return taskOutcomeNeedsAttention(task) ? "warning" : task.state;
}

export function taskProgressPercent(task: TaskDialogModel): number {
  if (task.total > 0) return Math.min(100, Math.round((task.done / task.total) * 100));
  if (task.state === "done") return 100;
  return 0;
}

export function taskOverallProgressIndeterminate(task: TaskDialogModel): boolean {
  return isTaskActiveState(task.state) && task.total === 0;
}

function isRecoveryProgressPhase(task: TaskDialogModel): boolean {
  return task.phase === "recovery_prepare"
    || task.phase === "recovery_verify"
    || task.phase === "recovery_process"
    || task.phase === "recovery_finalize";
}

export function taskProgressPhaseLabel(task: TaskDialogModel): string | null {
  if (task.phase === "recovery_prepare") return tr("gui.task.phase.recovery_prepare", "Preparing recovery data");
  if (task.phase === "recovery_verify") return tr("gui.task.phase.recovery_verify", "Verifying protected data");
  if (task.phase === "recovery_process") return tr("gui.task.phase.recovery_process", "Processing recovery blocks");
  if (task.phase === "recovery_finalize") return tr("gui.task.phase.recovery_finalize", "Finalizing recovery");
  if (task.phase === "output_recovery") return tr("gui.task.phase.output_recovery", "Recovering output");
  if (task.phase === "output_split") return tr("gui.task.phase.output_split", "Writing volume files");
  if (task.phase === "output_verify") return tr("gui.task.phase.output_verify", "Verifying output");
  if (task.phase === "output_commit") return tr("gui.task.phase.output_commit", "Publishing output");
  if (task.phase === "output_cleanup") return tr("gui.task.phase.output_cleanup", "Finalizing output");
  if (task.phase === "update_recovery") return tr("gui.task.phase.update_recovery", "Recovering update");
  if (task.phase === "update_rewrite") return tr("gui.task.phase.update_rewrite", "Rewriting archive");
  if (task.phase === "update_verify") return tr("gui.task.phase.update_verify", "Verifying packages");
  if (task.phase === "update_commit") return tr("gui.task.phase.update_commit", "Installing update");
  if (task.phase === "update_cleanup") return tr("gui.task.phase.update_cleanup", "Finalizing update");
  if (task.phase === "sfx_publish_verify") return tr("gui.task.phase.sfx_publish_verify", "Verifying self-extractor");
  if (task.phase === "sfx_publish_sign") return tr("gui.task.phase.sfx_publish_sign", "Signing for macOS");
  if (task.phase === "sfx_publish_notarize") return tr("gui.task.phase.sfx_publish_notarize", "Waiting for Apple notarization");
  if (task.phase === "sfx_publish_finalize") return tr("gui.task.phase.sfx_publish_finalize", "Stapling and validating");
  return null;
}

export function taskOverallProgressLabel(task: TaskDialogModel): string {
  return isTaskActiveState(task.state) && taskProgressPhaseLabel(task)
    ? tr("gui.task.current_phase", "Current phase")
    : tr("gui.task.overall_progress", "Overall progress");
}

export function taskOverallProgressBadge(task: TaskDialogModel): string {
  if (task.state === "failed" || task.state === "cancelled") {
    return taskOutcomeStateLabel(task);
  }
  if (isTaskActiveState(task.state) && !isTaskProgressingState(task.state)) {
    return taskOutcomeStateLabel(task);
  }
  const phase = isTaskActiveState(task.state) ? taskProgressPhaseLabel(task) : null;
  if (phase && task.total === 0 && isTaskProgressingState(task.state)) return phase;
  if (task.total > 0 || task.state === "done") return `${taskProgressPercent(task)}%`;
  if (isTaskProgressingState(task.state) && task.scanEntries != null) {
    return tr("gui.task.scan_badge", "Scanning");
  }
  if (isTaskProgressingState(task.state)) {
    return tr("gui.task.current_progress_pending_badge", "In progress");
  }
  return taskOutcomeStateLabel(task);
}

export function hasTaskCurrentProgress(task: TaskDialogModel): boolean {
  return task.currentTotal > 0;
}

export function taskCurrentSectionVisible(task: TaskDialogModel): boolean {
  if (task.state === "done") return false;
  return isTaskActiveState(task.state) || Boolean(task.current) || hasTaskCurrentProgress(task);
}

function taskCurrentProgressDone(task: TaskDialogModel): number {
  if (!hasTaskCurrentProgress(task)) return 0;
  return Math.min(task.currentDone, task.currentTotal);
}

export function taskCurrentProgressPercent(task: TaskDialogModel): number {
  if (!hasTaskCurrentProgress(task)) return 0;
  return Math.min(100, Math.round((taskCurrentProgressDone(task) / task.currentTotal) * 100));
}

function taskSpeedLabel(task: TaskDialogModel): string {
  return task.speed > 0 ? t("gui.task.speed_per_second", { speed: formatBytes(task.speed) }) : taskOutcomeStateLabel(task);
}

export function taskProgressSummary(task: TaskDialogModel): string {
  if (task.state === "submitting") {
    return tr("gui.task.progress_submitting", "Opening the progress window before archive execution starts");
  }
  if (task.scanEntries != null) {
    return t("gui.task.progress_scan", { count: task.scanEntries });
  }
  const phase = isTaskActiveState(task.state) ? taskProgressPhaseLabel(task) : null;
  if (phase && task.total > 0 && isRecoveryProgressPhase(task)) {
    return t("gui.task.recovery_phase_progress_known", {
      phase,
      percent: taskProgressPercent(task),
    });
  }
  if (phase && task.total > 0) {
    return t("gui.task.phase_progress_known", {
      phase,
      percent: taskProgressPercent(task),
      done: formatBytes(task.done),
      total: formatBytes(task.total),
      speed: taskSpeedLabel(task),
    });
  }
  if (phase) {
    return t("gui.task.phase_progress_pending", { phase });
  }
  if (task.spec.kind === "batch_extract") {
    const total = Math.max(1, task.spec.items.length);
    const done = task.state === "done"
      ? Number(task.result?.extracted ?? total)
      : Math.min(total, Math.floor((taskProgressPercent(task) / 100) * total));
    return t("gui.task.progress_batch_extract", {
      percent: taskProgressPercent(task),
      done,
      total,
    });
  }
  if (task.total > 0) {
    return t("gui.task.progress_known", {
      percent: taskProgressPercent(task),
      done: formatBytes(task.done),
      total: formatBytes(task.total),
      speed: taskSpeedLabel(task),
    });
  }
  if (task.done > 0) {
    return t("gui.task.progress_unknown", {
      done: formatBytes(task.done),
      speed: taskSpeedLabel(task),
    });
  }
  return taskSpeedLabel(task);
}

export function taskCurrentSectionLabel(task: TaskDialogModel): string {
  if (!isTaskActiveState(task.state)) return tr("gui.task.last_item", "Last item");
  if (task.scanEntries != null) return tr("gui.task.current_input", "Current input");
  return tr("gui.task.current_file", "Current file");
}

export function taskCurrentLabel(task: TaskDialogModel): string {
  return task.current || tr("gui.task.waiting_for_engine", "Preparing progress");
}

export function taskCurrentProgressBadge(task: TaskDialogModel): string {
  if (isTaskActiveState(task.state) && !isTaskProgressingState(task.state)) {
    return taskOutcomeStateLabel(task);
  }
  if (hasTaskCurrentProgress(task)) return `${taskCurrentProgressPercent(task)}%`;
  if (!isTaskActiveState(task.state)) return taskOutcomeStateLabel(task);
  if (task.scanEntries != null) return tr("gui.task.scan_badge", "Scanning");
  return tr("gui.task.current_progress_pending_badge", "In progress");
}

export function taskCurrentProgressSource(task: TaskDialogModel): string {
  if (task.scanEntries != null) return "scan-entry";
  return hasTaskCurrentProgress(task) ? "engine-bytes" : "pending";
}

export function taskCurrentProgressSummary(task: TaskDialogModel): string {
  if (task.state === "submitting") {
    return tr("gui.task.current_submitting", "Preparing the first item");
  }
  if (!isTaskProgressingState(task.state)) {
    if (isTaskActiveState(task.state)) return taskStateLabel(task.state);
    if (task.current) {
      if (task.state === "done") {
        return taskOutcomeNeedsAttention(task)
          ? tr("gui.task.finished_with_issues", "Finished with issues")
          : tr("gui.task.current_progress_completed_short", "Complete");
      }
      return taskStateLabel(task.state);
    }
    return tr("gui.task.current_progress_completed", "Task finished.");
  }
  if (task.scanEntries != null) {
    return t("gui.task.current_scan_named", { name: taskCurrentLabel(task) });
  }
  if (!hasTaskCurrentProgress(task)) {
    if (task.current) {
      return t("gui.task.current_progress_pending_named", { name: taskCurrentLabel(task) });
    }
    return tr("gui.task.current_progress_pending", "Preparing the current item.");
  }
  return t("gui.task.current_progress_known", {
    name: taskCurrentLabel(task),
    done: formatBytes(taskCurrentProgressDone(task)),
    total: formatBytes(task.currentTotal),
  });
}

export function taskKindLabel(task: TaskDialogModel): string {
  if (task.spec.kind === "compress" && task.spec.sfx_target) {
    return tr("gui.task.kind.create_sfx", "Self-extractor");
  }
  if (task.spec.kind === "publish_macos_sfx") {
    return tr("gui.task.kind.publish_macos_sfx", "Trusted macOS app");
  }
  return tr(`gui.task.kind.${task.spec.kind}`, task.spec.kind.replaceAll("_", " "));
}

export function taskTitleLabel(task: TaskDialogModel): string {
  return jobTitleFor(task.spec);
}

export function taskDialogEyebrow(task: TaskDialogModel): string {
  if (task.state === "submitting") return tr("gui.task.dialog_starting_eyebrow", "Starting task");
  if (task.controlIntent === "cancel") return tr("gui.task.cancel_requested", "Cancel requested");
  if (task.controlIntent === "pause") return tr("gui.task.pause_requested", "Pause requested");
  if (task.controlIntent === "resume") return tr("gui.task.resume_requested", "Resume requested");
  return tr("gui.task.dialog_eyebrow", "Task progress");
}

export function taskControlCalloutVisible(task: TaskDialogModel): boolean {
  return task.controlIntent !== null;
}

export function taskPhaseControlNoticeVisible(task: TaskDialogModel): boolean {
  return isTaskActiveState(task.state) && !task.interruptible;
}

export function taskPhaseControlNoticeTitle(task: TaskDialogModel): string {
  if (isRecoveryProgressPhase(task)) {
    return tr("gui.task.recovery_phase_control_title", "Completing recovery work");
  }
  if (task.phase === "output_recovery") {
    return tr("gui.task.output_recovery_control_title", "Recovering output");
  }
  const outputPublication = task.phase === "output_commit" || task.phase === "output_cleanup";
  if (outputPublication) {
    return task.state === "paused"
      ? tr("gui.task.output_phase_control_paused_title", "Ready to publish output")
      : tr("gui.task.output_phase_control_title", "Publishing output");
  }
  return task.state === "paused"
    ? tr("gui.task.phase_control_paused_title", "Ready to complete update")
    : tr("gui.task.phase_control_title", "Completing update");
}

export function taskPhaseControlNoticeDetail(task: TaskDialogModel): string {
  if (isRecoveryProgressPhase(task)) {
    return tr(
      "gui.task.recovery_phase_control_detail",
      "The recovery engine is completing a stage that cannot be safely interrupted. Pause and cancel are unavailable until it reaches a safe boundary.",
    );
  }
  if (task.phase === "output_recovery") {
    return tr(
      "gui.task.output_recovery_control_detail",
      "Squallz is completing a durable output recovery. Pause and cancel are unavailable until the destination is consistent.",
    );
  }
  const outputPublication = task.phase === "output_commit" || task.phase === "output_cleanup";
  if (outputPublication) {
    return task.state === "paused"
      ? tr(
        "gui.task.output_phase_control_paused_detail",
        "Resume to publish the verified output. Pause and cancel are unavailable after publication begins.",
      )
      : tr(
        "gui.task.output_phase_control_detail",
        "Squallz is publishing the verified output through a durable transaction. Pause and cancel are unavailable until it finishes.",
      );
  }
  if (task.state === "paused") {
    return tr(
      "gui.task.phase_control_paused_detail",
      "Resume to let the durable update finish. Pause and cancel are unavailable after installation begins.",
    );
  }
  return tr(
    "gui.task.phase_control_detail",
    "The replacement is being installed or recovered. Pause and cancel are unavailable while Squallz completes the durable transaction.",
  );
}

export function taskControlCalloutTitle(task: TaskDialogModel): string {
  if (task.controlIntent === "cancel") {
    return tr("gui.task.control_cancel_title", "Cancellation pending");
  }
  if (task.controlIntent === "pause") {
    return tr("gui.task.control_pause_title", "Pause pending");
  }
  if (task.controlIntent === "resume") {
    return tr("gui.task.control_resume_title", "Resume pending");
  }
  return tr("gui.task.control_title", "Task control");
}

export function taskControlCalloutDetail(task: TaskDialogModel): string {
  if (task.controlIntent === "cancel") {
    return tr("gui.task.control_cancel_detail", "Stopping at the next safe checkpoint. New archive actions stay blocked until the engine confirms cancellation.");
  }
  if (task.controlIntent === "pause") {
    return tr("gui.task.control_pause_detail", "Pausing at the next safe checkpoint. Progress stays visible while the engine finishes the current chunk.");
  }
  if (task.controlIntent === "resume") {
    return tr("gui.task.control_resume_detail", "Waiting for the engine to report running again.");
  }
  return tr("gui.task.control_detail", "The current control request is waiting for archive engine acknowledgement.");
}

export function taskCancelButtonLabel(task: TaskDialogModel): string {
  return task.controlIntent === "cancel"
    ? tr("gui.task.cancelling", "Cancelling...")
    : tr("gui.task.cancel", "Cancel");
}

export function taskPauseButtonLabel(task: TaskDialogModel): string {
  return task.controlIntent === "pause"
    ? tr("gui.task.pausing_action", "Pausing...")
    : tr("gui.task.pause", "Pause");
}

export function taskResumeButtonLabel(task: TaskDialogModel): string {
  return task.controlIntent === "resume"
    ? tr("gui.task.resuming_action", "Resuming...")
    : tr("gui.task.resume", "Resume");
}

export function shortDigest(value: string): string {
  return value.length > 28 ? `${value.slice(0, 18)}...${value.slice(-8)}` : value;
}

function resultNumber(task: TaskDialogModel, key: string): number {
  const value = task.result?.[key];
  return typeof value === "number" && Number.isFinite(value) ? value : 0;
}

function extractResultCounts(task: TaskDialogModel) {
  return readExtractResultCounts(task.result);
}

function extractResultNumber(task: TaskDialogModel, key: string): number {
  const counts = extractResultCounts(task);
  if (!counts) return 0;
  const value = counts[key as keyof typeof counts];
  return typeof value === "number" && Number.isFinite(value) ? value : 0;
}

function firstResultNumber(task: TaskDialogModel, keys: string[]): number {
  for (const key of keys) {
    const value = resultNumber(task, key);
    if (value !== 0) return value;
  }
  return 0;
}

function resultCount(task: TaskDialogModel, key: string): number {
  const value = task.result?.[key];
  if (Array.isArray(value)) return value.length;
  return resultNumber(task, key);
}

function resultBool(task: TaskDialogModel, key: string, fallback: boolean): boolean {
  const value = task.result?.[key];
  return typeof value === "boolean" ? value : fallback;
}

function isRecoveryDiagnosticTask(task: TaskDialogModel): boolean {
  return task.spec.kind === "verify_recovery" || task.spec.kind === "repair_recovery";
}

function recoveryStatusSummary(task: TaskDialogModel): string {
  const ok = recoveryResultOk(task.result);
  if (task.spec.kind === "repair_recovery") {
    return ok === true
      ? tr("gui.recovery.repair_completed", "Repair completed")
      : tr("gui.recovery.repair_not_completed", "Repair did not complete");
  }
  if (ok === true) return tr("gui.recovery.verification_passed", "Verification passed");
  const repairPossible = recoveryResultBoolean(task.result, "repair_possible");
  if (repairPossible === true) return tr("gui.recovery.repairable", "Repairable");
  if (repairPossible === false) return tr("gui.recovery.not_repairable", "Not repairable");
  return tr("gui.recovery.damage_detected", "Damage detected");
}

function recoveryCapacitySummary(task: TaskDialogModel): string | null {
  const needed = recoveryResultNumber(task.result, "blocks_needed");
  const available = recoveryResultNumber(task.result, "recovery_blocks_available");
  if (needed === null || available === null) return null;
  return tFallback(
    "gui.recovery.capacity_summary",
    "{needed} blocks needed · {available} recovery blocks available",
    { needed: needed.toLocaleString(), available: available.toLocaleString() },
  );
}

function recoveryRepairCountSummary(task: TaskDialogModel): string | null {
  const blocks = recoveryResultNumber(task.result, "blocks_repaired");
  const files = recoveryResultNumber(task.result, "files_repaired");
  if (blocks !== null && files !== null) {
    return tFallback(
      "gui.task.recovery.repaired_counts",
      "{blocks} blocks and {files} files repaired",
      { blocks: blocks.toLocaleString(), files: files.toLocaleString() },
    );
  }
  if (blocks !== null) {
    return tFallback(
      "gui.recovery.blocks_repaired",
      "{count} blocks repaired",
      { count: blocks.toLocaleString() },
    );
  }
  if (files !== null) {
    return tFallback(
      "gui.task.recovery.files_repaired_summary",
      "{count} files repaired",
      { count: files.toLocaleString() },
    );
  }
  return null;
}

function recoveryDialogSummary(task: TaskDialogModel): string {
  const parts = [recoveryStatusSummary(task)];
  const capacity = recoveryCapacitySummary(task);
  if (capacity) parts.push(capacity);
  if (task.spec.kind === "repair_recovery" && recoveryResultOk(task.result) === true) {
    const repaired = recoveryRepairCountSummary(task);
    if (repaired) parts.push(repaired);
  }
  return parts.join(" · ");
}

function appendRecoveryMetricRow(
  rows: TaskResultDetailRow[],
  label: string,
  value: number | null,
): void {
  if (value === null) return;
  rows.push({ label, value: value.toLocaleString() });
}

function resultDetailValue(value: unknown): string {
  if (typeof value === "string") return value;
  if (typeof value === "number" && Number.isFinite(value)) return value.toLocaleString();
  if (typeof value === "boolean") return value ? tr("common.yes", "Yes") : tr("common.no", "No");
  if (value === null || value === undefined) return "";
  try {
    const json = JSON.stringify(value);
    return typeof json === "string" ? json : String(value);
  } catch {
    return String(value);
  }
}

function compactResultDetail(value: unknown): string {
  const text = resultDetailValue(value).replace(/\s+/g, " ").trim();
  return text.length > 180 ? `${text.slice(0, 177)}...` : text;
}

function appendProblems(rows: TaskResultDetailRow[], task: TaskDialogModel): void {
  const problems = resultProblemMessages(task.result);
  const displayed = problems.slice(0, 6);
  for (const [index, problem] of displayed.entries()) {
    const value = compactResultDetail(problem);
    if (value) {
      rows.push({
        label: t("gui.task.result_problem_n", { index: index + 1 }),
        value,
      });
    }
  }
  const total = resultProblemTotal(task.result);
  const omitted = Math.max(0, total - displayed.length);
  if (omitted > 0) {
    rows.push({
      label: tr("gui.task.result_more_problems", "More problems"),
      value: t("gui.task.result_more_problems_detail", {
        count: omitted.toLocaleString(),
      }),
    });
  }
}

export function taskResultDetailTitle(task: TaskDialogModel): string {
  if (task.state === "failed") return tr("gui.task.error_details", "Error details");
  if (task.spec.kind === "publish_macos_sfx") {
    return tr("gui.task.sfx_publish_results", "Published app");
  }
  if (task.spec.kind === "protect") {
    return tr("gui.task.recovery_protect_results", "PAR2 files created");
  }
  if (isRecoveryDiagnosticTask(task)) return tr("gui.task.recovery_report", "Recovery report");
  if (task.spec.kind === "checksum" || task.spec.kind === "checksum_check") {
    return tr("gui.task.checksum_results", "Checksum results");
  }
  if (task.spec.kind === "test") return tr("gui.task.archive_test_report", "Archive test report");
  if (task.spec.kind === "compress") return tr("gui.task.create_results", "Files created");
  if (task.spec.kind === "convert") return tr("gui.task.convert_results", "Converted output");
  if (
    (task.spec.kind === "extract" || task.spec.kind === "extract_nested") &&
    extractResultCounts(task)
  ) {
    return tr("gui.task.extract_results", "Extraction results");
  }
  return tr("gui.task.result_details", "Result details");
}

export function taskResultDetailRows(task: TaskDialogModel): TaskResultDetailRow[] {
  const rows: TaskResultDetailRow[] = [];
  if (task.state === "failed") {
    if (
      task.error?.key === "error.recovery_cleanup_output_ready"
      || task.error?.key === "error.recovery_cleanup_unconfirmed"
      || task.error?.key === "error.recovery_cleanup_record"
    ) {
      const target = task.error.params.target?.trim();
      const workspace = task.error.params.workspace?.trim();
      const journal = task.error.params.journal?.trim();
      if (target) {
        rows.push({
          label: tr("gui.task.failure.par2_repair_target", "Repaired-copy target"),
          value: target,
        });
      }
      if (workspace) {
        rows.push({
          label: tr("gui.task.failure.par2_cleanup_workspace", "Private repair workspace"),
          value: workspace,
          scrollable: true,
        });
      }
      if (journal) {
        rows.push({
          label: tr("gui.task.failure.par2_cleanup_journal", "Automatic recovery record"),
          value: journal,
          scrollable: true,
        });
      }
    }
    if (task.error?.key === "error.sfx_recovery") {
      const target = task.error.params.target?.trim();
      const journal = task.error.params.journal?.trim();
      const paths = task.error.params.paths?.trim();
      if (target) {
        rows.push({
          label: tr("gui.task.failure.sfx_target", "Self-extractor target"),
          value: target,
        });
      }
      if (journal) {
        rows.push({
          label: tr("gui.task.failure.sfx_journal", "Recovery journal"),
          value: journal,
          scrollable: true,
        });
      }
      if (paths) {
        rows.push({
          label: tr("gui.task.failure.sfx_recovery_paths", "Paths to keep and inspect"),
          value: paths,
          scrollable: true,
        });
      }
    }
    const detail = task.error?.detail.trim();
    if (detail) {
      rows.push({
        label: tr("gui.task.technical_detail", "Technical detail"),
        value: detail,
      });
    }
    return rows;
  }
  if (task.state === "cancelled") {
    rows.push({
      label: tr("common.status", "Status"),
      value: tr("gui.task.result_cancelled", "The task was cancelled"),
    });
    return rows;
  }

  if (task.spec.kind === "publish_macos_sfx") {
    const output = String(task.result?.primary_output ?? task.spec.output);
    const team = String(task.result?.team_id ?? "").trim();
    const submission = String(task.result?.submission_id ?? "").trim();
    rows.push(
      {
        label: tr("common.output", "Output"),
        value: output,
      },
      {
        label: tr("gui.task.sfx_publish_signature", "Signature"),
        value: tr("gui.task.sfx_publish_developer_id", "Developer ID Application"),
      },
      {
        label: tr("gui.task.sfx_publish_notarization", "Apple notarization"),
        value: tr("gui.task.sfx_publish_accepted", "Accepted and stapled"),
      },
    );
    if (team) {
      rows.push({
        label: tr("gui.task.sfx_publish_team", "Team ID"),
        value: team,
      });
    }
    if (submission) {
      rows.push({
        label: tr("gui.task.sfx_publish_submission", "Submission ID"),
        value: submission,
      });
    }
    rows.push({
      label: tr("gui.task.sfx_publish_source", "Unsigned source"),
      value: task.spec.source,
    });
    return rows;
  }

  if (task.spec.kind === "protect") {
    const fallback = task.spec.recovery ?? `${task.spec.path}.par2`;
    const result = readRecoveryProtectionResult(task.result, fallback);
    const outputNames = result.outputs.map((output) => pathBaseName(output) || output);
    rows.push(
      {
        label: tr("common.output", "Output"),
        value: result.primaryOutput,
      },
      {
        label: tr("gui.task.result_output_files", "Output files"),
        value: result.outputs.length.toLocaleString(),
      },
    );
    if (outputNames.length > 1) {
      rows.push({
        label: tr("gui.task.result_output_set", "File list"),
        value: outputNames.join("\n"),
        scrollable: true,
      });
    }
    return rows;
  }

  if (isRecoveryDiagnosticTask(task)) {
    const ok = recoveryResultOk(task.result);
    if (ok !== null) {
      rows.push({
        label: tr("common.status", "Status"),
        value: recoveryStatusSummary(task),
      });
    }
    appendRecoveryMetricRow(
      rows,
      tr("gui.task.recovery.blocks_needed", "Blocks needed"),
      recoveryResultNumber(task.result, "blocks_needed"),
    );
    appendRecoveryMetricRow(
      rows,
      tr("gui.task.recovery.blocks_available", "Recovery blocks available"),
      recoveryResultNumber(task.result, "recovery_blocks_available"),
    );
    const repairPossible = recoveryResultBoolean(task.result, "repair_possible");
    if (repairPossible !== null) {
      rows.push({
        label: tr("gui.task.recovery.repair_possible", "Repair possible"),
        value: repairPossible ? tr("common.yes", "Yes") : tr("common.no", "No"),
      });
    }
    appendRecoveryMetricRow(
      rows,
      tr("gui.task.recovery.blocks_repaired", "Blocks repaired"),
      recoveryResultNumber(task.result, "blocks_repaired"),
    );
    appendRecoveryMetricRow(
      rows,
      tr("gui.task.recovery.files_repaired", "Files repaired"),
      recoveryResultNumber(task.result, "files_repaired"),
    );
    if (task.spec.kind === "repair_recovery" && recoveryResultOk(task.result) === true) {
      const output = task.result?.output;
      const outputPath = typeof output === "string" && output ? output : task.revealPath;
      if (outputPath) {
        rows.push({ label: tr("common.output", "Output"), value: outputPath });
      }
    }
    return rows;
  }

  if (task.spec.kind === "compress" || task.spec.kind === "convert") {
    const result = readCreateResult(task.result, task.spec.dest, task.spec.split_size !== null);
    const outputNames = result.outputs.map((output) => pathBaseName(output) || output);
    rows.push(
      {
        label: tr("common.output", "Output"),
        value: result.primaryOutput,
      },
      {
        label: tr("common.total_size", "Total size"),
        value: formatBytes(result.totalBytes || task.done),
      },
      {
        label: tr("gui.task.result_output_files", "Output files"),
        value: result.outputs.length.toLocaleString(),
      },
    );
    if (outputNames.length > 1) {
      rows.push({
        label: tr("gui.task.result_output_set", "File list"),
        value: outputNames.join("\n"),
        scrollable: true,
      });
    }
    if (result.split) {
      rows.push({
        label: tr("common.volumes", "Volumes"),
        value: result.volumeCount.toLocaleString(),
      });
    }
    if (result.preservedOutputs.length > 0) {
      rows.push(
        {
          label: tr("gui.task.result_preserved_outputs", "Preserved previous outputs"),
          value: result.preservedOutputs.length.toLocaleString(),
        },
        {
          label: tr("gui.task.result_preserved_output_set", "Preserved file list"),
          value: result.preservedOutputs.join("\n"),
          scrollable: true,
        },
      );
    }
    const cleanup = task.spec.kind === "compress" ? sourceCleanupResult(task) : null;
    if (cleanup) {
      rows.push(
        {
          label: tr("gui.task.source_cleanup.label", "Original sources"),
          value: sourceCleanupStatusLabel(cleanup.status),
        },
        {
          label: tFallback(
            "gui.task.source_cleanup.moved",
            "Moved to {trash}",
            { trash: platformTrashName() },
          ),
          value: cleanup.moved.toLocaleString(),
        },
        {
          label: tFallback(
            "gui.task.source_cleanup.kept",
            "Not moved to {trash}",
            { trash: platformTrashName() },
          ),
          value: cleanup.kept.toLocaleString(),
        },
      );
      if (cleanup.recoveryRequired > 0) {
        rows.push({
          label: tr("gui.task.source_cleanup.recovery_required", "Recovery needed"),
          value: cleanup.recoveryRequired.toLocaleString(),
        });
      }
    }
    return rows;
  }

  if (
    (task.spec.kind === "extract" || task.spec.kind === "extract_nested") &&
    extractResultCounts(task)
  ) {
    const destination = typeof task.result?.dest === "string" && task.result.dest
      ? task.result.dest
      : task.spec.dest;
    rows.push(
      {
        label: tr("common.destination", "Destination"),
        value: destination,
      },
      {
        label: tr("gui.task.result_selected", "Selected entries"),
        value: extractResultNumber(task, "selected_entries").toLocaleString(),
      },
      {
        label: tr("common.created", "Created"),
        value: extractResultNumber(task, "created").toLocaleString(),
      },
      {
        label: tr("gui.task.result_directories", "Folders prepared"),
        value: extractResultNumber(task, "directories").toLocaleString(),
      },
      {
        label: tr("common.replaced", "Replaced"),
        value: extractResultNumber(task, "replaced").toLocaleString(),
      },
      {
        label: tr("common.renamed", "Renamed"),
        value: extractResultNumber(task, "renamed").toLocaleString(),
      },
      {
        label: tr("common.skipped", "Skipped"),
        value: extractResultNumber(task, "skipped").toLocaleString(),
      },
      {
        label: tr("gui.task.result_failed_count", "Failed"),
        value: extractResultNumber(task, "failed").toLocaleString(),
      },
      {
        label: tr("gui.task.result_bytes_written", "Data written"),
        value: formatBytes(extractResultNumber(task, "output_bytes")),
      },
    );
    appendProblems(rows, task);
    return rows;
  }

  if (task.spec.kind === "checksum") {
    rows.push(
      {
        label: tr("gui.checksum.algorithm", "Algorithm"),
        value: task.spec.algorithm.toUpperCase(),
      },
      {
        label: tr("gui.task.result_files_hashed", "Files hashed"),
        value: resultNumber(task, "files_hashed").toLocaleString(),
      },
      {
        label: tr("gui.task.result_bytes_hashed", "Bytes hashed"),
        value: formatBytes(resultNumber(task, "bytes_hashed")),
      },
    );
    return rows;
  }

  if (task.spec.kind === "checksum_check") {
    rows.push(
      {
        label: tr("gui.task.result_passed", "Passed"),
        value: resultNumber(task, "passed").toLocaleString(),
      },
      {
        label: tr("gui.task.result_checked", "Checked"),
        value: resultNumber(task, "checked").toLocaleString(),
      },
      {
        label: tr("gui.task.result_failed_count", "Failed"),
        value: resultNumber(task, "failed").toLocaleString(),
      },
    );
    return rows;
  }

  if (task.spec.kind === "test") {
    const ok = resultBool(task, "ok", true);
    rows.push(
      {
        label: tr("common.status", "Status"),
        value: ok ? tr("gui.checksum.status_ok", "OK") : tr("gui.checksum.status_failed_caps", "FAILED"),
      },
      {
        label: tr("gui.task.result_entries_checked", "Entries checked"),
        value: firstResultNumber(task, ["entries_tested", "entries"]).toLocaleString(),
      },
      {
        label: tr("common.problems", "Problems"),
        value: resultProblemTotal(task.result).toLocaleString(),
      },
    );
    appendProblems(rows, task);
    return rows;
  }

  if (task.spec.kind === "duplicate_scan") {
    rows.push(
      {
        label: tr("gui.duplicates.groups", "Duplicate groups"),
        value: resultNumber(task, "duplicate_groups").toLocaleString(),
      },
      {
        label: tr("gui.duplicates.files_scanned", "Files scanned"),
        value: resultNumber(task, "files_scanned").toLocaleString(),
      },
      {
        label: tr("gui.duplicates.reclaimable", "Reclaimable"),
        value: formatBytes(resultNumber(task, "reclaimable_bytes")),
      },
    );
    return rows;
  }

  if (task.spec.kind === "batch_extract") {
    const archives = resultNumber(task, "archives");
    const selectedArchives = resultNumber(task, "selected_archives");
    rows.push(
      {
        label: tr("gui.task.result_extracted", "Extracted"),
        value: resultNumber(task, "extracted").toLocaleString(),
      },
      {
        label: tr("gui.task.result_archives", "Archives"),
        value: archives.toLocaleString(),
      },
      {
        label: tr("gui.task.result_failed_count", "Failed"),
        value: resultNumber(task, "failed").toLocaleString(),
      },
    );
    if (selectedArchives > archives) {
      rows.splice(2, 0, {
        label: tr("gui.task.result_selected_files", "Selected files"),
        value: selectedArchives.toLocaleString(),
      });
    }
  }

  if (task.revealPath) {
    rows.push({
      label: tr("common.output", "Output"),
      value: task.revealPath,
    });
  }
  return rows;
}

export function taskResultActionLabel(task: TaskDialogModel): string {
  if (taskHasInlineResults(task)) {
    return task.expanded
      ? tr("gui.task.hide_results", "Hide results")
      : tr("gui.task.view_results", "View results");
  }
  const target = taskResultScreen(task);
  if (target === "checksum") return tr("gui.task.view_checksum_results", "View checksum results");
  if (target === "duplicates") return tr("gui.task.view_duplicate_results", "View duplicate results");
  if (target === "recovery") return tr("gui.task.view_recovery_results", "View recovery results");
  if (target === "archiveInfo") return tr("gui.task.view_archive_report", "View archive report");
  return tr("gui.task.view_results", "View results");
}

export function taskResultAvailableForSurface(task: TaskDialogModel, taskWindowMode: boolean): boolean {
  if (task.state !== "done") return false;
  if (taskHasInlineResults(task)) return taskWindowMode ? !task.expanded : true;
  const hasResultScreen = taskResultScreen(task) !== null;
  return taskWindowMode ? hasResultScreen && !task.expanded : hasResultScreen;
}

export function taskErrorSummary(task: TaskDialogModel): string {
  return errorSummary(task.error);
}

export function taskErrorDetailsAvailable(task: TaskDialogModel): boolean {
  return task.state === "failed" && taskResultDetailRows(task).length > 0;
}

export function taskErrorDetailsActionLabel(task: TaskDialogModel): string {
  return task.expanded
    ? tr("gui.task.hide_error_details", "Hide error details")
    : tr("gui.task.show_error_details", "Show error details");
}

export function taskFailureReviewAvailable(task: TaskDialogModel, taskWindowMode: boolean): boolean {
  return !taskWindowMode && taskFailureReviewScreen(task) !== null;
}

export function taskFailureReviewActionLabel(task: TaskDialogModel): string {
  const target = taskFailureReviewScreen(task);
  if (target === "recovery") return tr("gui.task.open_recovery", "Open Recovery");
  if (target === "browse") return tr("gui.task.return_to_archive", "Return to archive");
  return tr("gui.task.review_settings", "Review task settings");
}

export function taskOpenOutputLabel(task: TaskDialogModel): string {
  if (taskOutputIsFolder(task)) return tr("gui.task.open_output_folder", "Open output folder");
  if (task.spec.kind === "publish_macos_sfx") {
    return tr("gui.task.open_published_app", "Open published app");
  }
  if (task.spec.kind === "compress" && task.spec.sfx_target) {
    return tr("gui.task.open_self_extractor", "Open self-extractor");
  }
  if (task.spec.kind === "compress") return tr("gui.task.open_created_archive", "Open created archive");
  if (task.spec.kind === "convert") return tr("gui.task.open_converted_archive", "Open converted archive");
  return tr("gui.task.open_output", "Open output");
}

function taskFailureNextStepDetail(task: TaskDialogModel): string {
  const key = task.error?.key;
  if (key === "error.recovery_cleanup_output_ready") {
    return tFallback(
      "gui.task.failure.next_recovery_cleanup_output_ready",
      "Test the repaired copy at {target}. When you are satisfied, close any app using the private workspace and remove only {workspace}, then retry so Squallz can clear {journal}. Do not delete adjacent hidden folders.",
      {
        target: task.error?.params.target || "the repaired-copy target",
        workspace: task.error?.params.workspace || "the listed private workspace",
        journal: task.error?.params.journal || "the listed automatic recovery record",
      },
    );
  }
  if (key === "error.recovery_cleanup_unconfirmed") {
    return tFallback(
      "gui.task.failure.next_recovery_cleanup_unconfirmed",
      "The repair result is not confirmed. Inspect {target} before retrying because a late filesystem error may have left an output there. Keep or copy {workspace} until inspection is complete, then remove only that workspace and retry; Squallz will clear {journal}.",
      {
        target: task.error?.params.target || "the repaired-copy target",
        workspace: task.error?.params.workspace || "the listed private workspace",
        journal: task.error?.params.journal || "the listed automatic recovery record",
      },
    );
  }
  if (key === "error.recovery_cleanup_record") {
    return tFallback(
      "gui.task.failure.next_recovery_cleanup_record",
      "Close other Squallz windows and retry the same repaired-copy target. If this remains, keep or copy {journal} and read the technical detail. Do not edit the record or delete adjacent hidden folders because no workspace path was trusted.",
      {
        journal: task.error?.params.journal || "the listed automatic recovery record",
      },
    );
  }
  if (key === "error.sfx_recovery") {
    return tFallback(
      "gui.task.failure.next_sfx_recovery",
      "Leave the target and every listed recovery path untouched. Copy them to a safe folder, then verify {target}. If the target is unusable, restore the listed previous output. Review the journal and every listed path before trying again.",
      {
        target: task.error?.params.target
          || (task.spec.kind === "compress" ? task.spec.dest : "the target"),
      },
    );
  }
  if (key === "error.password_required" || key === "error.wrong_password") {
    return tr("gui.task.failure.next_password", "Return to the task settings and enter the password again.");
  }
  if (key === "error.disk_full") {
    return tr("gui.task.failure.next_disk_full", "Free space on the destination and temporary volumes, then review the destination before restarting.");
  }
  if (key === "error.destination_changed") {
    return tr(
      "gui.task.failure.next_destination_changed",
      "Review the current destination and confirm replacement again, or choose a different output. Squallz did not replace it.",
    );
  }
  if (key === "error.input_changed") {
    return tr(
      "gui.task.failure.next_input_changed",
      "Reopen the archive, review the selected files and destination, then start extraction again. Squallz did not extract anything.",
    );
  }
  if (key === "error.output_exists") {
    return tr("gui.task.failure.next_output_exists", "Choose a different output name. Squallz left the existing item unchanged.");
  }
  if (key === "error.dependency_missing") {
    return tFallback(
      "gui.task.failure.next_dependency",
      "Squallz needs {name} for this task. Check the technical details and the platform package before trying again; Squallz will not download components automatically.",
      { name: task.error?.params.name || tr("gui.task.failure.required_component", "the required component") },
    );
  }
  if (key === "gui.error.corrupt.volume_missing") {
    return tFallback(
      "gui.error.corrupt.volume_missing",
      "Volume {name} is missing. Keep all volumes in the same folder.",
      { name: task.error?.params.name ?? "" },
    );
  }
  if (key === "error.unsupported_split_wim") {
    return tFallback(
      "error.unsupported_split_wim",
      "This Split WIM stream has no source folder. Open any .swm member from disk and keep every part together.",
    );
  }
  if (key === "error.unsupported_split_wim_create") {
    return tFallback(
      "error.unsupported_split_wim_create",
      "Creating native Split WIM (.swm) is not supported. Create a standalone .wim instead.",
    );
  }
  if (key === "error.corrupt_archive") {
    return tr("gui.task.failure.next_corrupt", "Keep every split volume together, then test the archive or open Recovery.");
  }
  if (key === "error.path_traversal" || key === "error.symlink_breakout" || key === "error.unsafe_filename") {
    return tr("gui.task.failure.next_unsafe", "Do not bypass this protection. Verify the source before trying a recovery workflow.");
  }
  if (key === "error.resource_limit") {
    return tr("gui.task.failure.next_resource", "Review the technical details before changing settings or trying another workflow.");
  }
  if (key === "error.unsupported") {
    return tr("gui.task.failure.next_unsupported", "Check the source and output formats. The technical details show why this workflow is unavailable.");
  }
  if (key === "error.io") {
    return tr("gui.task.failure.next_io", "Check that the source and destination still exist and that Squallz can read and write them.");
  }
  return tr("gui.task.failure.next_generic", "Review the task settings and error details before starting it again.");
}

export function taskNextStepDetail(task: TaskDialogModel, taskWindowMode: boolean): string {
  if (task.state === "failed") {
    const detail = taskFailureNextStepDetail(task);
    return taskWindowMode
      ? tFallback(
        "gui.task.failure.next_window",
        "{detail} Make changes in the main Squallz window; this task window will not restart the job.",
        { detail },
      )
      : detail;
  }
  if (isRecoveryDiagnosticTask(task) && recoveryResultOk(task.result) === false) {
    if (task.spec.kind === "repair_recovery") {
      return tr(
        "gui.task.next_step_recovery_repair_failed",
        "Keep the source and recovery files together, then review the verification report before another repair attempt.",
      );
    }
    const repairPossible = recoveryResultBoolean(task.result, "repair_possible");
    if (repairPossible === true) {
      return tr(
        "gui.task.next_step_recovery_repairable",
        "The available recovery data is sufficient. Review the report, then start Repair from Recovery.",
      );
    }
    if (repairPossible === false) {
      return tr(
        "gui.task.next_step_recovery_over_capacity",
        "Keep the source unchanged. More recovery data is required for a full repair.",
      );
    }
    return tr(
      "gui.task.next_step_recovery_verify_failed",
      "Review the recovery report before deciding whether to repair or extract readable files.",
    );
  }
  if (task.spec.kind === "compress" || task.spec.kind === "convert") {
    const result = readCreateResult(task.result, task.spec.dest, task.spec.split_size !== null);
    if (result.preservedOutputs.length > 0) {
      const onePreservedOutput = result.preservedOutputs.length === 1;
      if (task.spec.kind === "compress" && task.spec.sfx_target) {
        if (task.spec.sfx_target === "macos") {
          return onePreservedOutput
            ? tr(
              "gui.task.next_step_sfx_preserved_macos_one",
              "Test the new app and review the preserved previous output. Keep that backup until the app passes testing, signing, and notarization.",
            )
            : tr(
              "gui.task.next_step_sfx_preserved_macos",
              "Test the new app and review every preserved previous output. Keep those backups until the app passes testing, signing, and notarization.",
            );
        }
        return onePreservedOutput
          ? tr(
            "gui.task.next_step_sfx_preserved_one",
            "Test the executable on its target system and review the preserved previous output. Keep that backup until testing and signing are complete.",
          )
          : tr(
            "gui.task.next_step_sfx_preserved",
            "Test the executable on its target system and review every preserved previous output. Keep those backups until testing and signing are complete.",
          );
      }
      return onePreservedOutput
        ? tr(
          "gui.task.next_step_preserved_output_one",
          "Test the new archive, then review the preserved previous output. Delete it only after the new archive is verified.",
        )
        : tr(
          "gui.task.next_step_preserved_outputs",
          "Test the new archive, then review the paths for the preserved previous outputs. Delete those files only after the new archive is verified.",
        );
    }
  }
  if (task.spec.kind === "compress" && task.spec.sfx_target) {
    return task.spec.sfx_target === "macos"
      ? tr("gui.task.next_step_sfx_macos", "Test the app locally, then sign and notarize it before distribution.")
      : tr("gui.task.next_step_sfx", "Test the executable on its target system and sign it before distribution.");
  }
  if (
    (task.spec.kind === "compress" || task.spec.kind === "convert") &&
    readCreateResult(task.result, task.spec.dest, task.spec.split_size !== null).split
  ) {
    return tr(
      "gui.task.next_step_split",
      "Keep every numbered volume in the same folder. Reveal the output set before sharing or extracting it.",
    );
  }
  if (task.spec.kind === "protect") {
    return tr(
      "gui.task.next_step_recovery_protect",
      "Keep every file in this PAR2 set together. Reveal the complete set before moving or sharing it.",
    );
  }
  if (task.spec.kind === "publish_macos_sfx") {
    return tr(
      "gui.task.next_step_sfx_published",
      "Distribute the verified app. Keep the unsigned source if you may need to rebuild or publish it again.",
    );
  }
  if ((task.spec.kind === "checksum" || task.spec.kind === "checksum_check") && taskChecksumItems(task).length > 0) {
    return taskWindowMode
      ? tr("gui.task.next_step_checksum_window", "Copy the checksum results from this window.")
      : tr("gui.task.next_step_checksum", "Copy the checksum results from this window or open the checksum tool page.");
  }
  if (taskOutputPath(task)) {
    if (taskWindowMode && (task.spec.kind === "compress" || task.spec.kind === "convert")) {
      return tr(
        "gui.task.next_step_create_window",
        "Reveal the created output in the file manager, or close this window.",
      );
    }
    return taskOutputIsFolder(task)
      ? tr("gui.task.next_step_folder", "Open the destination folder or reveal it in the file manager.")
      : tr("gui.task.next_step_file", "Open the generated file or reveal it in the file manager.");
  }
  if (taskResultScreen(task)) {
    return taskWindowMode
      ? tr("gui.task.next_step_window_results", "Review the result details in this window, then close it.")
      : tr("gui.task.next_step_results", "Review the finished report in its tool page.");
  }
  return tr("gui.task.next_step_done", "The task is finished; close this window to continue.");
}

export function taskDialogResultSummary(task: TaskDialogModel): string {
  if (task.state === "failed") {
    return taskErrorSummary(task);
  }
  if (task.state === "cancelled") return tr("gui.task.result_cancelled", "The task was cancelled");
  if (isRecoveryDiagnosticTask(task) && task.result) return recoveryDialogSummary(task);
  if (task.spec.kind === "checksum") {
    const files = Number(task.result?.files_hashed ?? 0);
    const bytes = Number(task.result?.bytes_hashed ?? task.done);
    return t("gui.task.result_checksum", { files, bytes: formatBytes(bytes) });
  }
  if (task.spec.kind === "checksum_check") {
    const passed = Number(task.result?.passed ?? 0);
    const checked = Number(task.result?.checked ?? 0);
    const failed = Number(task.result?.failed ?? 0);
    return t("gui.task.result_checksum_check", { passed, checked, failed });
  }
  if (task.spec.kind === "duplicate_scan") {
    const groups = Number(task.result?.duplicate_groups ?? 0);
    const reclaimable = Number(task.result?.reclaimable_bytes ?? 0);
    return t("gui.task.result_duplicate_scan", { groups, size: formatBytes(reclaimable) });
  }
  if (task.spec.kind === "test") {
    const ok = task.result?.ok !== false;
    const entries = Number(task.result?.entries_tested ?? task.result?.entries ?? 0);
    const problems = resultProblemTotal(task.result);
    return ok
      ? t("gui.task.result_test_ok", { count: entries })
      : t("gui.task.result_test_failed", { count: problems });
  }
  if (
    (task.spec.kind === "extract" || task.spec.kind === "extract_nested") &&
    extractResultCounts(task)
  ) {
    const completed = extractResultNumber(task, "created")
      + extractResultNumber(task, "directories")
      + extractResultNumber(task, "replaced")
      + extractResultNumber(task, "renamed");
    const skipped = extractResultNumber(task, "skipped");
    const failed = extractResultNumber(task, "failed");
    return t("gui.task.result_extract", { completed, skipped, failed });
  }
  if (task.spec.kind === "batch_extract") {
    const extracted = Number(task.result?.extracted ?? 0);
    const total = Number(task.result?.archives ?? task.spec.items.length);
    const selected = Number(task.result?.selected_archives ?? total);
    const failed = Number(task.result?.failed ?? 0);
    return t(
      selected > total
        ? "gui.task.result_batch_extract_grouped"
        : "gui.task.result_batch_extract",
      { extracted, total, selected, failed },
    );
  }
  if (task.spec.kind === "protect") {
    const fallback = task.spec.recovery ?? `${task.spec.path}.par2`;
    const result = readRecoveryProtectionResult(task.result, fallback);
    return result.outputs.length > 1
      ? t("gui.task.result_recovery_protect_set", {
          name: pathBaseName(result.primaryOutput),
          count: result.outputs.length.toLocaleString(),
        })
      : t("gui.task.result_recovery_protect", {
          name: pathBaseName(result.primaryOutput),
        });
  }
  if (task.spec.kind === "publish_macos_sfx") {
    return t("gui.task.result_sfx_published", {
      name: pathBaseName(String(task.result?.primary_output ?? task.spec.output)),
      team: String(task.result?.team_id ?? ""),
    });
  }
  if (task.spec.kind === "compress") {
    const result = readCreateResult(task.result, task.spec.dest, task.spec.split_size !== null);
    if (result.preservedOutputs.length > 0) {
      if (task.spec.sfx_target) {
        return t(result.preservedOutputs.length === 1
          ? "gui.task.result_sfx_unsigned_preserved_one"
          : "gui.task.result_sfx_unsigned_preserved", {
          name: pathBaseName(result.primaryOutput),
          size: formatBytes(result.totalBytes || task.done),
          count: result.preservedOutputs.length.toLocaleString(),
        });
      }
      return t(result.preservedOutputs.length === 1
        ? "gui.task.result_create_preserved_one"
        : "gui.task.result_create_preserved", {
        name: pathBaseName(result.primaryOutput),
        count: result.preservedOutputs.length.toLocaleString(),
      });
    }
  }
  if (task.spec.kind === "compress" && task.spec.sfx_target) {
    const result = readCreateResult(task.result, task.spec.dest, false);
    const size = result.totalBytes || task.done;
    return t("gui.task.result_sfx_unsigned", {
      name: pathBaseName(result.primaryOutput),
      size: formatBytes(size),
    });
  }
  if (task.spec.kind === "compress") {
    const result = readCreateResult(task.result, task.spec.dest, task.spec.split_size !== null);
    const size = formatBytes(result.totalBytes || task.done);
    return result.split
      ? t("gui.task.result_create_split", {
          name: pathBaseName(result.primaryOutput),
          count: result.volumeCount,
          size,
        })
      : t("gui.task.result_create", {
          name: pathBaseName(result.primaryOutput),
          size,
        });
  }
  if (task.spec.kind === "convert") {
    const result = readCreateResult(task.result, task.spec.dest, task.spec.split_size !== null);
    const size = formatBytes(result.totalBytes || task.done);
    if (result.preservedOutputs.length > 0) {
      return t(result.preservedOutputs.length === 1
        ? "gui.task.result_convert_preserved_one"
        : "gui.task.result_convert_preserved", {
        name: pathBaseName(result.primaryOutput),
        count: result.preservedOutputs.length.toLocaleString(),
      });
    }
    return result.split
      ? t("gui.task.result_convert_split", {
          name: pathBaseName(result.primaryOutput),
          count: result.volumeCount,
          size,
        })
      : t("gui.task.result_convert", {
          name: pathBaseName(result.primaryOutput),
          size,
        });
  }
  if (task.revealPath) {
    return t("gui.task.result_output", { path: pathBaseName(task.revealPath) || task.revealPath });
  }
  return tr("gui.task.result_ready", "Task finished; result details are available in the related tool");
}
