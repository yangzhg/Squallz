import { resultOf } from "./job-result";
import { basename as pathBaseName, formatBytes } from "./format";
import { errorSummary } from "./error-presentation";
import { t, tFallback as tr } from "./i18n.svelte";
import { readCreateResult } from "./create-result";
import { readBatchExtractResult, batchExtractResultSummary } from "./batch-extract-result";
import { extractResultHasRecoveredZipStructure, extractResultNeedsAttention } from "./extract-result";
import { readRecoveryProtectionResult, recoveryResultBoolean, recoveryResultNumber, recoveryResultOk } from "./recovery-result";
import { readSourceCleanupSummary } from "./source-cleanup";
import { platformTrashName } from "./platform-labels";
import { resultProblemTotal } from "./problem-preview";
import type { TaskDialogModel } from "./task-model";

export type TaskResultStatus = "done" | "failed" | "info";

/** One result interpretation for dialogs, history and completion notifications. */
export function taskResultStatus(task: TaskDialogModel): TaskResultStatus {
  if (task.state === "failed") return "failed";
  if (task.state !== "done") return "info";
  const { spec, result } = task;
  if (!result) return "info";
  if ((result.kind === "test" || result.kind === "checksum_check") && result.ok === false) return "failed";
  if (isRecoveryDiagnosticTask(task) && recoveryResultOk(result) === false) return "failed";
  if (spec.kind === "batch_extract") {
    const batch = readBatchExtractResult(result);
    if (batch.failedArchives > 0) return "failed";
    if (batch.reviewCount > 0 || batch.skippedEntries > 0 || extractResultNeedsAttention(result)) return "info";
  }
  if ((spec.kind === "extract" || spec.kind === "extract_nested") && extractResultNeedsAttention(result)) return "info";
  if ((spec.kind === "compress" || spec.kind === "convert")
    && readCreateResult(result, spec.dest, spec.split_size !== null).preservedOutputs.length > 0) return "info";
  if (spec.kind === "compress" && spec.post_success === "trash_source") {
    const cleanup = readSourceCleanupSummary(result);
    if (!cleanup || cleanup.status !== "completed" || cleanup.kept > 0 || cleanup.recoveryRequired > 0) return "info";
  }
  return "done";
}

/** Output availability is derived from the result, including tasks recovered in another window. */
export function taskResultOutputPath(task: TaskDialogModel): string | null {
  if (task.state !== "done") return task.revealPath;
  const result = task.result;
  if (!result) return null;
  switch (result.kind) {
    case "compress": case "convert": case "publish_macos_sfx": case "export_sqz": return result.primary_output;
    case "extract": case "extract_nested": case "repair_sqz": case "repair_zip": return result.dest;
    case "batch_extract": return result.outputs[0]?.dest ?? null;
    case "protect": return readRecoveryProtectionResult(result, result.recovery).primaryOutput;
    case "repair_recovery": return result.ok ? result.output ?? result.archive : null;
    case "update": return result.archive;
    default: return null;
  }
}

export function sourceCleanupStatusLabel(status: string): string {
  if (status === "completed") {
    return tr(
      "gui.task.source_cleanup.completed",
      { trash: platformTrashName() },
    );
  }
  if (status === "partial") return tr("gui.task.source_cleanup.partial");
  if (status === "blocked") return tr("gui.task.source_cleanup.blocked");
  if (status === "cancelled") return tr("gui.task.source_cleanup.cancelled");
  if (status === "failed") return tr("gui.task.source_cleanup.failed");
  if (status === "not_requested") return tr("gui.task.source_cleanup.not_requested");
  return tr("gui.task.source_cleanup.unknown");
}

export function isRecoveryDiagnosticTask(task: TaskDialogModel): boolean {
  return task.spec.kind === "verify_recovery" || task.spec.kind === "repair_recovery";
}

export function recoveryStatusSummary(task: TaskDialogModel): string {
  const ok = recoveryResultOk(task.result);
  if (task.spec.kind === "repair_recovery") {
    return ok === true
      ? tr("gui.recovery.repair_completed")
      : tr("gui.recovery.repair_not_completed");
  }
  if (ok === true) return tr("gui.recovery.verification_passed");
  const repairPossible = recoveryResultBoolean(task.result, "repair_possible");
  if (repairPossible === true) return tr("gui.recovery.repairable");
  if (repairPossible === false) return tr("gui.recovery.not_repairable");
  return tr("gui.recovery.damage_detected");
}

function recoveryCapacitySummary(task: TaskDialogModel): string | null {
  const needed = recoveryResultNumber(task.result, "blocks_needed");
  const available = recoveryResultNumber(task.result, "recovery_blocks_available");
  if (needed === null || available === null) return null;
  return tr(
    "gui.recovery.capacity_summary",
    { needed: needed.toLocaleString(), available: available.toLocaleString() },
  );
}

function recoveryRepairCountSummary(task: TaskDialogModel): string | null {
  const blocks = recoveryResultNumber(task.result, "blocks_repaired");
  const files = recoveryResultNumber(task.result, "files_repaired");
  if (blocks !== null && files !== null) {
    return tr(
      "gui.task.recovery.repaired_counts",
      { blocks: blocks.toLocaleString(), files: files.toLocaleString() },
    );
  }
  if (blocks !== null) {
    return tr(
      "gui.recovery.blocks_repaired",
      { count: blocks.toLocaleString() },
    );
  }
  if (files !== null) {
    return tr(
      "gui.task.recovery.files_repaired_summary",
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

export function taskResultSummary(task: TaskDialogModel): string {
  if (task.state === "failed") {
    return errorSummary(task.error);
  }
  const report = task.result;
  if (task.state === "cancelled") return tr("gui.task.result_cancelled");
  if (isRecoveryDiagnosticTask(task) && task.result) return recoveryDialogSummary(task);
  if (report?.kind === "checksum") {
    const files = report.files_hashed;
    const bytes = report.bytes_hashed;
    return t("gui.task.result_checksum", { files, bytes: formatBytes(bytes) });
  }
  if (report?.kind === "checksum_check") {
    const passed = report.passed;
    const checked = report.checked;
    const failed = report.failed;
    return t("gui.task.result_checksum_check", { passed, checked, failed });
  }
  if (report?.kind === "duplicate_scan") {
    const groups = report.duplicate_groups;
    const reclaimable = report.reclaimable_bytes;
    return t("gui.task.result_duplicate_scan", { groups, size: formatBytes(reclaimable) });
  }
  if (report?.kind === "test") {
    const ok = report.ok;
    const entries = report.entries_tested;
    const problems = resultProblemTotal(task.result);
    return ok
      ? t("gui.task.result_test_ok", { count: entries })
      : t("gui.task.result_test_failed", { count: problems });
  }
  if (report?.kind === "extract" || report?.kind === "extract_nested") {
    const { created, directories, replaced, renamed, skipped, failed } = report.counts;
    return t("gui.task.result_extract", { completed: created + directories + replaced + renamed, skipped, failed });
  }
  if (task.spec.kind === "batch_extract") {
    return batchExtractResultSummary(task.result, task.spec.items.length);
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
  if (report?.kind === "publish_macos_sfx") {
    return t("gui.task.result_sfx_published", {
      name: pathBaseName(report.primary_output),
      team: report.team_id,
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
  const output = taskResultOutputPath(task);
  if (output) return t("gui.task.result_output", { path: pathBaseName(output) || output });
  return tr("gui.task.result_ready");
}

export function taskResultPresentation(task: TaskDialogModel) {
  const { spec, result } = task;
  const status = taskResultStatus(task);
  const output = taskResultOutputPath(task);
  const summary = taskResultSummary(task);
  const details: string[] = [];
  let body: string | undefined;
  let warning = status !== "done";
  if (task.state === "done" && (spec.kind === "compress" || spec.kind === "convert")) {
    const created = readCreateResult(result, spec.dest, spec.split_size !== null);
    if (created.testedAfterCreate) {
      details.push(tr("gui.task.history.integrity_tested", {
        count: created.entriesTestedAfterCreate.toLocaleString(),
      }));
    }
    if (created.preservedOutputs.length > 0) {
      body = spec.kind === "compress" && spec.sfx_target
        ? tr(spec.sfx_target === "macos"
          ? "gui.toast.sfx_done_unsigned_preserved_detail_macos"
          : "gui.toast.sfx_done_unsigned_preserved_detail")
        : tr(spec.kind === "compress"
          ? "gui.toast.compress_done_preserved_detail" : "gui.toast.convert_done_preserved_detail");
    }
    if (spec.kind === "compress") {
      warning ||= Boolean(spec.sfx_target);
      if (spec.post_success === "trash_source") {
        const cleanup = readSourceCleanupSummary(result);
        details.push(cleanup ? tr("gui.task.history.source_cleanup_counts", {
          status: sourceCleanupStatusLabel(cleanup.status),
          moved: cleanup.moved.toLocaleString(),
          kept: cleanup.kept.toLocaleString(),
          recovery: cleanup.recoveryRequired.toLocaleString(),
        }) : sourceCleanupStatusLabel("unknown"));
      }
    }
  }
  if ((spec.kind === "extract" || spec.kind === "extract_nested" || spec.kind === "batch_extract")
    && extractResultHasRecoveredZipStructure(result)) {
    body = tr("gui.archive.zip_local_headers_recovered");
  }
  if (spec.kind === "protect" && readRecoveryProtectionResult(result, spec.recovery ?? `${spec.path}.par2`).outputs.length > 1) {
    body = tr("gui.toast.recovery_protect_done_detail");
  }
  if (spec.kind === "duplicate_scan") warning ||= (resultOf(result, "duplicate_scan")?.duplicate_groups ?? 0) > 0;
  return {
    status,
    summary,
    output,
    toastKind: warning ? "warning" as const : "success" as const,
    body: body ?? (details.join(" · ") || undefined),
    historyDetail: [summary, output && !summary.includes(pathBaseName(output)) ? pathBaseName(output) : null, ...details]
      .filter(Boolean).join(" · "),
  };
}
