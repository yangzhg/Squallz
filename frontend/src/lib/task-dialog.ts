import { basename as pathBaseName, formatBytes } from "./format";
import { t } from "./i18n.svelte";
import { jobTitleFor } from "./job-title";
import type { Task } from "./jobs.svelte";
import type { Screen } from "./ui-model";

export type TaskDialogState = Task["state"] | "submitting";

export type TaskDialogModel = Omit<Task, "id" | "state"> & {
  id: number | null;
  state: TaskDialogState;
};

export interface TaskResultDetailRow {
  label: string;
  value: string;
}

export type TaskResultScreen = Extract<Screen, "checksum" | "duplicates" | "recovery" | "archiveInfo">;

export function tr(key: string, fallback: string): string {
  const value = t(key);
  return value === key ? fallback : value;
}

export function taskStateLabel(state: string | null | undefined): string {
  if (!state) return tr("gui.task.state.pending", "Pending");
  if (state === "submitting") return tr("gui.task.state.submitting", "Starting");
  if (state === "queued") return tr("gui.task.state.waiting", "Waiting");
  if (state === "running") return tr("gui.task.state.running", "Running");
  if (state === "paused") return tr("gui.task.state.paused", "Paused");
  if (state === "pausing") return tr("gui.task.state.pausing", "Pausing...");
  if (state === "done") return tr("gui.task.state.done", "Done");
  if (state === "failed") return tr("gui.task.state.failed", "Failed");
  if (state === "cancelled") return tr("gui.task.state.cancelled", "Cancelled");
  return state;
}

export function isTaskActiveState(state: string | null | undefined): boolean {
  return state === "submitting" || state === "queued" || state === "running" || state === "paused" || state === "pausing";
}

export function taskProgressPercent(task: TaskDialogModel): number {
  if (task.total > 0) return Math.min(100, Math.round((task.done / task.total) * 100));
  if (task.state === "done") return 100;
  return task.done > 0 || task.state === "submitting" || task.state === "running" ? 8 : 0;
}

export function hasTaskCurrentProgress(task: TaskDialogModel): boolean {
  return task.currentTotal > 0;
}

export function taskCurrentSectionVisible(task: TaskDialogModel): boolean {
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
  return task.speed > 0 ? t("gui.task.speed_per_second", { speed: formatBytes(task.speed) }) : taskStateLabel(task.state);
}

export function taskProgressSummary(task: TaskDialogModel): string {
  if (task.state === "submitting") {
    return tr("gui.task.progress_submitting", "Opening the progress window before archive execution starts");
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
  return tr("gui.task.current_file", "Current file");
}

export function taskCurrentLabel(task: TaskDialogModel): string {
  return task.current || tr("gui.task.waiting_for_engine", "Preparing progress");
}

export function taskCurrentProgressBadge(task: TaskDialogModel): string {
  if (hasTaskCurrentProgress(task)) return `${taskCurrentProgressPercent(task)}%`;
  if (!isTaskActiveState(task.state)) return taskStateLabel(task.state);
  return tr("gui.task.current_progress_pending_badge", "In progress");
}

export function taskCurrentProgressSource(task: TaskDialogModel): string {
  return hasTaskCurrentProgress(task) ? "engine-bytes" : "pending";
}

export function taskCurrentProgressSummary(task: TaskDialogModel): string {
  if (task.state === "submitting") {
    return tr("gui.task.current_submitting", "Preparing the first item");
  }
  if (!isTaskActiveState(task.state)) {
    if (task.current) {
      if (task.state === "done") return tr("gui.task.current_progress_completed_short", "Complete");
      return taskStateLabel(task.state);
    }
    return tr("gui.task.current_progress_completed", "Task finished.");
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

export function taskChecksumItems(task: TaskDialogModel): Record<string, unknown>[] {
  const items = task.result?.items;
  if (!Array.isArray(items)) return [];
  return items.filter((item): item is Record<string, unknown> => item !== null && typeof item === "object" && !Array.isArray(item));
}

export function checksumItemText(item: Record<string, unknown>, key: string): string {
  const value = item[key];
  return typeof value === "string" ? value : "";
}

export function checksumItemStatus(item: Record<string, unknown>): string {
  const ok = item.ok;
  if (typeof ok === "boolean") return ok ? tr("gui.checksum.status_ok", "OK") : tr("gui.checksum.status_failed_caps", "FAILED");
  return tr("gui.checksum.status_hashed", "hashed");
}

export function shortDigest(value: string): string {
  return value.length > 28 ? `${value.slice(0, 18)}...${value.slice(-8)}` : value;
}

export function checksumResultLine(kind: "checksum" | "checksum_check", item: Record<string, unknown>): string {
  const path = checksumItemText(item, "path");
  if (kind === "checksum") return `${checksumItemText(item, "digest")}  ${path}`;
  return [
    checksumItemStatus(item),
    path,
    checksumItemText(item, "expected"),
    checksumItemText(item, "actual") || checksumItemText(item, "error"),
  ].join("\t");
}

export function taskChecksumResultText(task: TaskDialogModel): string {
  if (task.spec.kind !== "checksum" && task.spec.kind !== "checksum_check") return "";
  const kind = task.spec.kind;
  return taskChecksumItems(task)
    .map((item) => checksumResultLine(kind, item))
    .filter((line) => line.trim().length > 0)
    .join("\n");
}

function resultNumber(task: TaskDialogModel, key: string): number {
  const value = task.result?.[key];
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
  const problems = task.result?.problems;
  if (!Array.isArray(problems)) return;
  for (const [index, problem] of problems.slice(0, 6).entries()) {
    const value = compactResultDetail(problem);
    if (value) {
      rows.push({
        label: t("gui.task.result_problem_n", { index: index + 1 }),
        value,
      });
    }
  }
}

export function taskResultDetailTitle(task: TaskDialogModel): string {
  if (task.spec.kind === "checksum" || task.spec.kind === "checksum_check") {
    return tr("gui.task.checksum_results", "Checksum results");
  }
  if (task.spec.kind === "test") return tr("gui.task.archive_test_report", "Archive test report");
  return tr("gui.task.result_details", "Result details");
}

export function taskResultDetailRows(task: TaskDialogModel): TaskResultDetailRow[] {
  const rows: TaskResultDetailRow[] = [];
  if (task.state === "failed") {
    rows.push({
      label: tr("common.status", "Status"),
      value: task.error?.detail || tr("gui.task.result_failed", "The task failed before producing a result"),
    });
    return rows;
  }
  if (task.state === "cancelled") {
    rows.push({
      label: tr("common.status", "Status"),
      value: tr("gui.task.result_cancelled", "The task was cancelled"),
    });
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
        value: resultCount(task, "problems").toLocaleString(),
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
    rows.push(
      {
        label: tr("gui.task.result_extracted", "Extracted"),
        value: resultNumber(task, "extracted").toLocaleString(),
      },
      {
        label: tr("gui.task.result_archives", "Archives"),
        value: resultNumber(task, "archives").toLocaleString(),
      },
      {
        label: tr("gui.task.result_failed_count", "Failed"),
        value: resultNumber(task, "failed").toLocaleString(),
      },
    );
  }

  if (task.revealPath) {
    rows.push({
      label: tr("common.output", "Output"),
      value: task.revealPath,
    });
  }
  return rows;
}

export function taskResultScreen(task: TaskDialogModel): TaskResultScreen | null {
  switch (task.spec.kind) {
    case "checksum":
    case "checksum_check":
      return "checksum";
    case "duplicate_scan":
      return "duplicates";
    case "protect":
    case "verify_recovery":
    case "repair_recovery":
    case "repair_zip":
    case "repair_sqz":
    case "export_sqz":
      return "recovery";
    case "test":
      return "archiveInfo";
    default:
      return null;
  }
}

export function taskResultActionLabel(task: TaskDialogModel): string {
  const target = taskResultScreen(task);
  if (target === "checksum") return tr("gui.task.view_checksum_results", "View checksum results");
  if (target === "duplicates") return tr("gui.task.view_duplicate_results", "View duplicate results");
  if (target === "recovery") return tr("gui.task.view_recovery_results", "View recovery results");
  if (target === "archiveInfo") return tr("gui.task.view_archive_report", "View archive report");
  return tr("gui.task.view_results", "View results");
}

export function taskResultAvailableForSurface(task: TaskDialogModel, taskWindowMode: boolean): boolean {
  const hasResultScreen = taskResultScreen(task) !== null;
  return taskWindowMode ? hasResultScreen && !task.expanded : hasResultScreen;
}

export function taskOutputPath(task: TaskDialogModel): string | null {
  return task.revealPath;
}

export function taskOutputIsFolder(task: TaskDialogModel): boolean {
  return task.spec.kind === "extract" || task.spec.kind === "extract_nested" || task.spec.kind === "batch_extract";
}

export function taskOpenOutputLabel(task: TaskDialogModel): string {
  if (taskOutputIsFolder(task)) return tr("gui.task.open_output_folder", "Open output folder");
  if (task.spec.kind === "compress") return tr("gui.task.open_created_archive", "Open created archive");
  return tr("gui.task.open_output", "Open output");
}

export function taskNextStepDetail(task: TaskDialogModel, taskWindowMode: boolean): string {
  if ((task.spec.kind === "checksum" || task.spec.kind === "checksum_check") && taskChecksumItems(task).length > 0) {
    return taskWindowMode
      ? tr("gui.task.next_step_checksum_window", "Copy the checksum results from this window.")
      : tr("gui.task.next_step_checksum", "Copy the checksum results from this window or open the checksum tool page.");
  }
  if (taskOutputPath(task)) {
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
    return task.error?.detail || tr("gui.task.result_failed", "The task failed before producing a result");
  }
  if (task.state === "cancelled") return tr("gui.task.result_cancelled", "The task was cancelled");
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
    const problems = Array.isArray(task.result?.problems)
      ? task.result.problems.length
      : Number(task.result?.problems ?? 0);
    return ok
      ? t("gui.task.result_test_ok", { count: entries })
      : t("gui.task.result_test_failed", { count: problems });
  }
  if (task.spec.kind === "batch_extract") {
    const extracted = Number(task.result?.extracted ?? 0);
    const total = Number(task.result?.archives ?? task.spec.items.length);
    const failed = Number(task.result?.failed ?? 0);
    return t("gui.task.result_batch_extract", { extracted, total, failed });
  }
  if (task.revealPath) {
    return t("gui.task.result_output", { path: pathBaseName(task.revealPath) || task.revealPath });
  }
  return tr("gui.task.result_ready", "Task finished; result details are available in the related tool");
}
