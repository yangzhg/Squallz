import { tFallback } from "./i18n.svelte";
import { readCreateResult } from "./create-result";
import type { Task } from "./jobs.svelte";
import { recoveryResultOk } from "./recovery-result";
import type { Screen } from "./ui-model";
import type { JobSpec } from "./ipc";
import {
  extractResultNeedsAttention,
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

export type SourceCleanupResult = {
  status: string;
  moved: number;
  kept: number;
  recoveryRequired: number;
};

type CompressJobSpec = Extract<JobSpec, { kind: "compress" }>;

export function applyCreateDestinationAuthorization(
  spec: CompressJobSpec,
  guard: string | null,
): CompressJobSpec {
  return {
    ...spec,
    replace_existing: guard !== null,
    replacement_guard: guard,
  };
}

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
  if (!state) return tFallback("gui.task.state.pending", "Pending");
  if (state === "submitting") return tFallback("gui.task.state.submitting", "Starting");
  if (state === "queued") return tFallback("gui.task.state.waiting", "Waiting");
  if (state === "running") return tFallback("gui.task.state.running", "Running");
  if (state === "paused") return tFallback("gui.task.state.paused", "Paused");
  if (state === "pausing") return tFallback("gui.task.state.pausing", "Pausing...");
  if (state === "done") return tFallback("gui.task.state.done", "Done");
  if (state === "failed") return tFallback("gui.task.state.failed", "Failed");
  if (state === "cancelled") return tFallback("gui.task.state.cancelled", "Cancelled");
  return state;
}

export function isTaskActiveState(state: string | null | undefined): boolean {
  return state === "submitting"
    || state === "queued"
    || state === "running"
    || state === "paused"
    || state === "pausing";
}

export function sourceCleanupResult(task: TaskDialogModel): SourceCleanupResult | null {
  const raw = task.result?.source_cleanup;
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) return null;
  const cleanup = raw as Record<string, unknown>;
  return {
    status: String(cleanup.status ?? ""),
    moved: Math.max(0, Math.trunc(Number(cleanup.moved) || 0)),
    kept: Math.max(0, Math.trunc(Number(cleanup.kept) || 0)),
    recoveryRequired: Math.max(0, Math.trunc(Number(cleanup.recovery_required) || 0)),
  };
}

export function taskOutcomeNeedsAttention(task: TaskDialogModel): boolean {
  if (task.state !== "done") return false;
  if (task.spec.kind === "test" || task.spec.kind === "checksum_check") {
    return task.result?.ok === false;
  }
  if (task.spec.kind === "batch_extract") {
    return Number(task.result?.failed ?? 0) > 0;
  }
  if (task.spec.kind === "extract" || task.spec.kind === "extract_nested") {
    return extractResultNeedsAttention(task.result);
  }
  if (
    (task.spec.kind === "compress" || task.spec.kind === "convert")
    && readCreateResult(task.result, task.spec.dest, task.spec.split_size !== null)
      .preservedOutputs.length > 0
  ) {
    return true;
  }
  if (task.spec.kind === "compress" && task.spec.post_success === "trash_source") {
    const cleanup = sourceCleanupResult(task);
    return cleanup === null || cleanup.status !== "completed";
  }
  const recoveryDiagnostic = task.spec.kind === "verify_recovery"
    || task.spec.kind === "repair_recovery";
  return recoveryDiagnostic && recoveryResultOk(task.result) === false;
}

export function taskChecksumItems(task: TaskDialogModel): Record<string, unknown>[] {
  const items = task.result?.items;
  if (!Array.isArray(items)) return [];
  return items.filter(
    (item): item is Record<string, unknown> =>
      item !== null && typeof item === "object" && !Array.isArray(item),
  );
}

export function checksumItemText(item: Record<string, unknown>, key: string): string {
  const value = item[key];
  return typeof value === "string" ? value : "";
}

export function checksumItemStatus(item: Record<string, unknown>): string {
  const ok = item.ok;
  if (typeof ok === "boolean") {
    return ok
      ? tFallback("gui.checksum.status_ok", "OK")
      : tFallback("gui.checksum.status_failed_caps", "FAILED");
  }
  return tFallback("gui.checksum.status_hashed", "hashed");
}

export function checksumResultLine(
  kind: "checksum" | "checksum_check",
  item: Record<string, unknown>,
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
    || task.spec.kind === "convert"
    || task.spec.kind === "publish_macos_sfx"
    || task.spec.kind === "protect"
    || (
      (task.spec.kind === "extract" || task.spec.kind === "extract_nested")
      && readExtractResultCounts(task.result) !== null
    )
  ) && task.result !== null;
}

export function taskFailureReviewScreen(task: TaskDialogModel): Screen | null {
  if (task.state !== "failed") return null;
  switch (task.spec.kind) {
    case "compress":
      return "create";
    case "extract":
      return "extract";
    case "batch_extract":
      return "batch";
    case "extract_nested":
    case "update":
      return "browse";
    case "convert":
      return "convert";
    case "checksum":
    case "checksum_check":
      return "checksum";
    case "duplicate_scan":
      return "duplicates";
    case "test":
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
  return task.revealPath;
}

export function taskOutputCanOpen(task: TaskDialogModel): boolean {
  if (!task.revealPath) return false;
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
