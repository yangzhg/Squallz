import { resultOf, type ChecksumItem, type DuplicateGroup } from "./job-result";
import { tFallback } from "./i18n.svelte";
import { readCreateResult } from "./create-result";
import { readBatchExtractResult } from "./batch-extract-result";
import type { Task } from "./jobs.svelte";
import { taskResultOutputPath, taskResultStatus } from "./task-outcome";
import { readSourceCleanupSummary, type SourceCleanupSummary } from "./source-cleanup";
import type { Screen } from "./ui-model";
import type { JobSpec } from "./ipc";
import {
  readExtractResultCounts,
} from "./extract-result";

export type TaskDialogState = Task["state"] | "submitting";

export type TaskDialogModel = Omit<Task, "id" | "state"> & {
  id: number | null;
  state: TaskDialogState;
};

export type TaskResultScreen = Extract<
  Screen,
  "checksum" | "duplicates" | "recovery" | "archiveInfo"
>;

export type TaskConflictDecision = "abort" | "skip" | "overwrite" | "rename";

export interface TaskConflictAnswer {
  decision: TaskConflictDecision;
  applyAll: boolean;
}

export type SourceCleanupResult = SourceCleanupSummary;

export function taskPasswordReady(value: string): boolean {
  return value.length > 0;
}

export function normalizeTaskConflictAnswer(
  decision: TaskConflictDecision,
  applyAll: boolean,
): TaskConflictAnswer {
  return {
    decision,
    applyAll: decision === "abort" ? false : applyAll,
  };
}

export function taskStateLabel(state: string | null | undefined): string {
  if (!state) return tFallback("gui.task.state.pending");
  if (state === "submitting") return tFallback("gui.task.state.submitting");
  if (state === "queued") return tFallback("gui.task.state.waiting");
  if (state === "running") return tFallback("gui.task.state.running");
  if (state === "paused") return tFallback("gui.task.state.paused");
  if (state === "pausing") return tFallback("gui.task.state.pausing", "Pausing...");
  if (state === "done") return tFallback("gui.task.state.done");
  if (state === "failed") return tFallback("gui.task.state.failed");
  if (state === "cancelled") return tFallback("gui.task.state.cancelled");
  return state;
}

export function isTaskActiveState(state: string | null | undefined): boolean {
  return state === "submitting"
    || state === "queued"
    || state === "running"
    || state === "paused"
    || state === "pausing";
}

export function taskNeedsInput(task: TaskDialogModel): boolean {
  return isTaskActiveState(task.state) && task.controlIntent !== "cancel" && task.interaction !== null;
}

export function taskStatusUnavailable(task: TaskDialogModel): boolean {
  return Boolean(task.statusStale) && isTaskActiveState(task.state);
}

export function taskOutcomeStateLabel(task: TaskDialogModel): string {
  if (taskStatusUnavailable(task)) return tFallback("gui.task.status_unavailable");
  return taskOutcomeNeedsAttention(task)
    ? tFallback("gui.task.state.needs_attention")
    : taskStateLabel(task.state);
}

export function sourceCleanupResult(task: TaskDialogModel): SourceCleanupResult | null {
  return readSourceCleanupSummary(task.result);
}

export function taskOutcomeNeedsAttention(task: TaskDialogModel): boolean {
  return task.state === "done" && taskResultStatus(task) !== "done";
}

export function taskChecksumItems(task: TaskDialogModel): ChecksumItem[] {
  const result = task.result;
  return result?.kind === "checksum" || result?.kind === "checksum_check" ? result.items : [];
}

export type DuplicateGroupSummary = Pick<DuplicateGroup, "hash" | "size" | "paths">;

export function taskDuplicateGroups(task: TaskDialogModel | null): DuplicateGroupSummary[] {
  return resultOf(task?.result, "duplicate_scan")?.groups ?? [];
}

export function checksumItemText(item: ChecksumItem, key: "path" | "digest" | "expected" | "actual" | "error"): string {
  if (key === "path") return item.path;
  if (key === "digest") return "digest" in item ? item.digest : "";
  return "expected" in item ? item[key] ?? "" : "";
}

export function checksumItemStatus(item: ChecksumItem): string {
  return "ok" in item
    ? tFallback(item.ok ? "gui.checksum.status_ok" : "gui.checksum.status_failed_caps")
    : tFallback("gui.checksum.status_hashed");
}

export function checksumResultLine(
  kind: "checksum" | "checksum_check",
  item: ChecksumItem,
): string {
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
  return taskChecksumItems(task)
    .map((item) => checksumResultLine(task.spec.kind as "checksum" | "checksum_check", item))
    .filter((line) => line.trim().length > 0)
    .join("\n");
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

export function taskHasInlineResults(task: TaskDialogModel): boolean {
  return (
    task.spec.kind === "compress"
    || task.spec.kind === "batch_extract"
    || task.spec.kind === "convert"
    || task.spec.kind === "publish_macos_sfx"
    || task.spec.kind === "protect"
    || task.spec.kind === "test"
    || (
      (task.spec.kind === "extract" || task.spec.kind === "extract_nested")
      && readExtractResultCounts(task.result) !== null
    )
  ) && task.result !== null;
}

export function taskReviewScreen(task: TaskDialogModel): Screen | null {
  if (task.state === "done" && task.spec.kind === "extract_nested" && (readExtractResultCounts(task.result)?.failed ?? 0) > 0) {
    return "nestedExtract";
  }
  if (task.state === "done" && task.spec.kind === "batch_extract" && readBatchExtractResult(task.result).reviewCount > 0) {
    return "batch";
  }
  if (task.state === "done" && task.spec.kind === "test" && resultOf(task.result, "test")?.ok === false) {
    return "recovery";
  }
  if (task.state !== "failed" && task.state !== "cancelled") return null;
  switch (task.spec.kind) {
    case "compress":
      return "create";
    case "extract":
      return "extract";
    case "batch_extract":
      return "batch";
    case "extract_nested":
      return "nestedExtract";
    case "update":
      return "updateReview";
    case "convert":
      return "convert";
  }
  if (task.state !== "failed") return null;
  switch (task.spec.kind) {
    case "checksum":
    case "checksum_check":
      return "checksum";
    case "duplicate_scan":
      return "duplicates";
    case "test":
      if (task.error?.key === "error.resource_limit") return "settingsSecurity";
      return task.error?.key === "error.corrupt_archive" ? "recovery" : "archiveInfo";
    case "export_sqz":
    case "protect":
    case "verify_recovery":
    case "repair_recovery":
    case "repair_zip":
    case "repair_sqz":
      return "recovery";
    default:
      return null;
  }
}

export function taskOutputPath(task: TaskDialogModel): string | null {
  return taskResultOutputPath(task);
}

export function taskOutputCanOpen(task: TaskDialogModel): boolean {
  if (!taskOutputPath(task)) return false;
  if (task.spec.kind === "protect") return false;
  if (task.spec.kind !== "compress" && task.spec.kind !== "convert") return true;
  if (task.spec.kind === "compress" && task.spec.sfx_target) return false;
  return !readCreateResult(
    task.result,
    task.spec.dest,
    task.spec.split_size !== null,
  ).split;
}

export function taskOutputIsFolder(task: TaskDialogModel): boolean {
  return task.spec.kind === "extract"
    || task.spec.kind === "extract_nested"
    || task.spec.kind === "batch_extract";
}
