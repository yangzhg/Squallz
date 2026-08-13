// Job store: mirrors backend task events and feeds foreground progress,
// status text, and conflict/password prompts.

import {
  ipc,
  type AskConflictEvent,
  type AskPasswordEvent,
  type ErrorDto,
  type JobInteraction,
  type JobOrigin,
  type JobSnapshot,
  type JobSnapshotsDelta,
  type JobSpec,
  type ProgressEvent,
  type ProgressPhase,
  type QueueWaitReason,
  type StateEvent,
} from "./ipc";
import { errorSummary } from "./error-presentation";
import {
  isTerminalSnapshotState,
  localSubmissionPromotion,
  shouldApplyFullSnapshot,
  shouldApplySnapshotProgress,
  shouldApplySnapshotState,
} from "./job-snapshot";
import { t, tFallback } from "./i18n.svelte";
import { jobTitleFor } from "./job-title";
import { pushToast } from "./toasts.svelte";
import { basename, formatBytes } from "./format";
import { readCreateResult } from "./create-result";
import { openCreatedOutputWithFallback } from "./create-completion";
import { recordOperation, type OperationStatus } from "./history.svelte";
import {
  readRecoveryProtectionResult,
  recoveryResultBoolean,
  recoveryResultNumber,
  recoveryResultOk,
} from "./recovery-result";
import { platformTrashName } from "./platform-labels";
import {
  readSourceCleanupSummary,
  shouldRefreshSourceCleanupRecovery,
} from "./source-cleanup";
import {
  extractResultHasRecoveredZipStructure,
  extractResultNeedsAttention,
  readExtractResultOutcome,
} from "./extract-result";
import { resultProblemTotal } from "./problem-preview";
import { currentWebviewWindowListener } from "./tauri-events";

export type JobStateName = StateEvent["state"];
export type TaskControlIntent = "cancel" | "pause" | "resume";
export type TaskQueueMoveIntent = "earlier" | "later" | "position";

export interface Task {
  id: number;
  /** Last backend version applied to this task. */
  version: number;
  spec: JobSpec;
  title: string;
  origin: JobOrigin;
  ownedByRequester: boolean;
  interaction: JobInteraction | null;
  state: JobStateName;
  queuePosition: number | null;
  queueWaitReason: QueueWaitReason | null;
  cpuThreads: number;
  streamBufferLimitBytes: number | null;
  done: number;
  total: number;
  current: string;
  currentDone: number;
  currentTotal: number;
  scanEntries: number | null;
  speed: number;
  phase: ProgressPhase | null;
  interruptible: boolean;
  pausable: boolean;
  error: ErrorDto | null;
  result: Record<string, unknown> | null;
  /** Path revealed by the "Reveal" button on completion */
  revealPath: string | null;
  /** Guard so replayed/duplicate terminal events do not double-write history. */
  historyRecorded: boolean;
  /** Only tasks submitted by this WebView may trigger completion side effects. */
  localEffects: boolean;
  /** Whether the task has appeared in the authoritative snapshot feed. */
  snapshotSeen: boolean;
  /** Local optimistic feedback for controls whose backend acknowledgement can lag. */
  controlIntent: TaskControlIntent | null;
  /** Short-lived feedback while the shared queue applies a reorder request. */
  queueMoveIntent: TaskQueueMoveIntent | null;
  expanded: boolean;
}

export function jobSupportsPause(spec: JobSpec): boolean {
  return spec.kind !== "publish_macos_sfx"
    && spec.kind !== "protect"
    && spec.kind !== "verify_recovery"
    && spec.kind !== "repair_recovery";
}

const store = $state({
  tasks: [] as Task[],
  conflict: null as AskConflictEvent | null,
  password: null as AskPasswordEvent | null,
});

const pendingStates = new Map<number, StateEvent>();
const pendingProgress = new Map<number, ProgressEvent>();
const MAX_PENDING_EVENTS = 128;
const locallyDismissed = new Set<number>();
let snapshotRevision: number | null = null;
let snapshotGeneration = 0;
let revealAfterExtract = $state(false);
let createCompletionHandler: ((path: string) => Promise<boolean | void> | boolean | void) | null = null;
let sourceCleanupRecoveryRefreshHandler: (() => Promise<void> | void) | null = null;
const sampleRoot = "/Users/alex/Squallz Samples";
const sampleOutputRoot = "/Users/alex/Squallz Exports";

export function setRevealAfterExtractPreference(enabled: boolean): void {
  revealAfterExtract = enabled;
}

export function revealAfterExtractPreference(): boolean {
  return revealAfterExtract;
}

export function setCreateCompletionHandler(
  handler: ((path: string) => Promise<boolean | void> | boolean | void) | null,
): () => void {
  createCompletionHandler = handler;
  return () => {
    if (createCompletionHandler === handler) createCompletionHandler = null;
  };
}

export function setSourceCleanupRecoveryRefreshHandler(
  handler: (() => Promise<void> | void) | null,
): () => void {
  sourceCleanupRecoveryRefreshHandler = handler;
  return () => {
    if (sourceCleanupRecoveryRefreshHandler === handler) {
      sourceCleanupRecoveryRefreshHandler = null;
    }
  };
}

async function revealPath(path: string): Promise<boolean> {
  try {
    const { revealItemInDir } = await import("@tauri-apps/plugin-opener");
    await revealItemInDir(path);
    return true;
  } catch {
    pushToast({
      kind: "warning",
      title: tFallback("gui.toast.reveal_failed", "Could not reveal the created output"),
      body: tFallback(
        "gui.toast.reveal_failed_path",
        "Output: {path}. Copy this path and open it in your file manager.",
        { path },
      ),
    });
    return false;
  }
}

export function titleFor(spec: JobSpec): string {
  return jobTitleFor(spec);
}

function find(id: number): Task | undefined {
  return store.tasks.find((task) => task.id === id);
}

function rememberPending<T>(pending: Map<number, T>, id: number, event: T): void {
  pending.set(id, event);
  while (pending.size > MAX_PENDING_EVENTS) {
    const oldest = pending.keys().next().value;
    if (oldest === undefined) return;
    pending.delete(oldest);
  }
}

function redactedSpec(spec: JobSpec): JobSpec {
  if (spec.kind === "compress") {
    return { ...spec, password: null, replacement_guard: null };
  }
  if (spec.kind === "convert") {
    return { ...spec, src_password: null, dest_password: null, replacement_guard: null };
  }
  if (spec.kind === "export_sqz") {
    return { ...spec, dest_password: null, replacement_guard: null };
  }
  if (spec.kind === "batch_extract") {
    return { ...spec, items: spec.items.map((item) => ({ ...item, password: null })) };
  }
  if (spec.kind === "publish_macos_sfx") {
    return { ...spec, identity: "", notary_profile: "" };
  }
  if (spec.kind === "extract") {
    return { ...spec, password: null, expected_input_guard: null };
  }
  if (spec.kind === "extract_nested" || spec.kind === "test" || spec.kind === "update") {
    return { ...spec, password: null };
  }
  return spec;
}

/** Submits a job and registers it in the local task list. */
export async function submitJob(spec: JobSpec): Promise<number> {
  const id = await ipc.submitJob(spec);
  const existing = find(id);
  if (!existing) {
    store.tasks.push({
      id,
      version: 0,
      spec: redactedSpec(spec),
      title: titleFor(spec),
      origin: "app",
      ownedByRequester: true,
      interaction: null,
      state: "queued",
      queuePosition: null,
      queueWaitReason: null,
      cpuThreads: 1,
      streamBufferLimitBytes: null,
      done: 0,
      total: 0,
      current: "",
      currentDone: 0,
      currentTotal: 0,
      scanEntries: null,
      speed: 0,
      phase: null,
      interruptible: true,
      pausable: jobSupportsPause(spec),
      error: null,
      result: null,
      revealPath: null,
      historyRecorded: false,
      localEffects: true,
      snapshotSeen: false,
      controlIntent: null,
      queueMoveIntent: null,
      expanded: false,
    });
    replayPending(id);
  } else {
    const promotion = localSubmissionPromotion(existing.state, existing.localEffects);
    existing.localEffects = true;
    if (promotion.resetHistory) existing.historyRecorded = false;
    if (promotion.replayTerminal) runTerminalEffects(existing, "running");
  }
  return id;
}

function runTerminalEffects(task: Task, previousState: JobStateName): void {
  if (
    !task.localEffects ||
    isTerminalSnapshotState(previousState) ||
    !isTerminalSnapshotState(task.state)
  ) return;
  if (task.state === "done") {
    finishToast(task);
    reportSourceCleanup(task);
    if (
      shouldRefreshSourceCleanupRecovery(task.spec.kind, task.result) &&
      sourceCleanupRecoveryRefreshHandler
    ) {
      void sourceCleanupRecoveryRefreshHandler();
    }
    void runCreateCompletion(task);
    recordTaskHistory(task);
  } else if (task.state === "failed") {
    recordTaskHistory(task);
  } else {
    pushToast({ kind: "info", title: t("gui.toast.cancelled") });
  }
}

function clearPendingQuestions(id: number): void {
  if (store.conflict?.id === id) store.conflict = null;
  if (store.password?.id === id) store.password = null;
}

function errorNeedsInspection(error: ErrorDto | null): boolean {
  return error?.key === "error.sfx_recovery"
    || error?.key === "error.recovery_cleanup_output_ready"
    || error?.key === "error.recovery_cleanup_unconfirmed"
    || error?.key === "error.recovery_cleanup_record";
}

function onState(ev: StateEvent): void {
  const task = find(ev.id);
  if (!task) {
    if (!locallyDismissed.has(ev.id)) rememberPending(pendingStates, ev.id, ev);
    return;
  }
  if (!shouldApplySnapshotState(task.version, task.state, ev.version, ev.state)) return;
  const previousState = task.state;
  task.version = ev.version;
  task.state = ev.state;
  task.error = ev.error ?? null;
  if (ev.result !== undefined) {
    task.result = (ev.result as Record<string, unknown> | null) ?? null;
  }
  if (ev.state === "failed" && errorNeedsInspection(task.error)) {
    task.expanded = true;
  }
  if (
    isTerminalSnapshotState(ev.state) ||
    (ev.state === "paused" && task.controlIntent === "pause") ||
    (ev.state === "running" && task.controlIntent === "resume")
  ) {
    task.controlIntent = null;
  }
  if (isTerminalSnapshotState(ev.state)) clearPendingQuestions(ev.id);
  runTerminalEffects(task, previousState);
}

async function runCreateCompletion(task: Task): Promise<void> {
  if (task.spec.kind !== "compress") return;
  const completion = task.spec.completion ?? "none";
  if (completion === "none") return;
  const result = readCreateResult(task.result, task.spec.dest, task.spec.split_size !== null);
  const output = result.primaryOutput;
  if (completion === "reveal_output" || task.spec.sfx_target || result.split) {
    await revealPath(output);
    return;
  }
  if (!createCompletionHandler) {
    if (await revealPath(output)) {
      pushToast({
        kind: "info",
        title: tFallback("gui.toast.open_in_squallz_unavailable", "Squallz could not open the archive here, so the output was revealed instead"),
      });
    }
    return;
  }
  const openResult = await openCreatedOutputWithFallback(
    () => createCompletionHandler?.(output),
    () => revealPath(output),
  );
  if (openResult === "revealed") {
    pushToast({
      kind: "warning",
      title: tFallback("gui.toast.open_in_squallz_failed", "The archive could not be opened in Squallz; the output was revealed instead"),
    });
  }
}

function reportSourceCleanup(task: Task): void {
  if (task.spec.kind !== "compress" || task.spec.post_success !== "trash_source") return;
  const cleanup = readSourceCleanupSummary(task.result);
  if (!cleanup) {
    pushToast({
      kind: "warning",
      title: tFallback("gui.toast.source_cleanup_unknown", "The archive was created, but Squallz could not confirm whether the originals moved; check them before deleting anything"),
    });
    return;
  }
  const { moved, kept, recoveryRequired, status } = cleanup;
  const trash = platformTrashName();
  if (!status || status === "not_requested") {
    pushToast({
      kind: "warning",
      title: tFallback("gui.toast.source_cleanup_unknown", "The archive was created, but Squallz could not confirm whether the originals moved; check them before deleting anything"),
    });
    return;
  }
  if (recoveryRequired > 0) {
    pushToast({
      kind: "warning",
      title: tFallback(
        "gui.toast.source_cleanup_recovery",
        "Archive created · Preserved outside {trash}: {count} · Recovery needed beside the previous location",
        { count: recoveryRequired.toLocaleString(), trash },
      ),
    });
    return;
  }
  if (status === "cancelled") {
    pushToast({
      kind: "warning",
      title: tFallback(
        "gui.toast.source_cleanup_cancelled",
        "Archive created · Moving originals stopped · Moved to {trash}: {moved} · Kept: {kept}",
        { moved: moved.toLocaleString(), kept: kept.toLocaleString(), trash },
      ),
    });
    return;
  }
  if (status !== "completed" || kept > 0) {
    pushToast({
      kind: "warning",
      title: tFallback(
        "gui.toast.source_cleanup_partial",
        "Moved to {trash}: {moved} · Left in place: {failed}",
        { moved: moved.toLocaleString(), failed: kept.toLocaleString(), trash },
      ),
    });
    return;
  }
  pushToast({
    kind: "success",
    title: tFallback(
      "gui.toast.source_cleanup_done",
      "Moved to {trash}: {count}",
      { count: moved.toLocaleString(), trash },
    ),
  });
}

function recordTaskHistory(task: Task): void {
  if (task.historyRecorded) return;
  const status = terminalHistoryStatus(task);
  task.historyRecorded = true;
  recordOperation({
    status,
    title: t(status === "failed"
      ? "gui.task.history.title_failed"
      : status === "info"
        ? "gui.task.history.title_attention"
        : "gui.task.history.title_finished", {
      title: task.title,
    }),
    detail: taskHistoryDetail(task, status),
  });
}

function terminalHistoryStatus(task: Task): Extract<OperationStatus, "done" | "failed" | "info"> {
  if (task.state === "failed") return "failed";
  if (task.spec.kind === "test" && task.result?.ok === false) return "failed";
  if (task.spec.kind === "checksum_check" && task.result?.ok === false) return "failed";
  if (task.spec.kind === "batch_extract" && Number(task.result?.failed ?? 0) > 0) return "failed";
  if (
    task.spec.kind === "batch_extract"
    && extractResultHasRecoveredZipStructure(task.result)
  ) return "info";
  if (
    (task.spec.kind === "extract" || task.spec.kind === "extract_nested") &&
    extractResultNeedsAttention(task.result)
  ) return "info";
  if (isRecoveryDiagnosticTask(task) && recoveryResultOk(task.result) === false) return "failed";
  if (
    task.spec.kind === "compress" &&
    readCreateResult(task.result, task.spec.dest, task.spec.split_size !== null)
      .preservedOutputs.length > 0
  ) {
    return "info";
  }
  if (task.spec.kind === "compress" && task.spec.post_success === "trash_source") {
    const cleanup = readSourceCleanupSummary(task.result);
    if (!cleanup || cleanup.status !== "completed" || cleanup.kept > 0) return "info";
  }
  return "done";
}

function isRecoveryDiagnosticTask(task: Task): boolean {
  return task.spec.kind === "verify_recovery" || task.spec.kind === "repair_recovery";
}

function recoveryCapacityDetail(task: Task): string | null {
  const needed = recoveryResultNumber(task.result, "blocks_needed");
  const available = recoveryResultNumber(task.result, "recovery_blocks_available");
  if (needed === null || available === null) return null;
  return tFallback(
    "gui.recovery.capacity_summary",
    "{needed} blocks needed · {available} recovery blocks available",
    { needed: needed.toLocaleString(), available: available.toLocaleString() },
  );
}

function recoveryRepairCountDetail(task: Task): string | null {
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

function recoveryDiagnosticSummary(task: Task): string {
  const ok = recoveryResultOk(task.result);
  if (task.spec.kind === "repair_recovery") {
    return ok === true
      ? tFallback("gui.recovery.repair_completed", "Repair completed")
      : tFallback("gui.recovery.repair_not_completed", "Repair did not complete");
  }
  if (ok === true) {
    return tFallback("gui.recovery.verification_passed", "Verification passed");
  }
  const repairPossible = recoveryResultBoolean(task.result, "repair_possible");
  if (repairPossible === true) return t("gui.recovery.repairable");
  if (repairPossible === false) return t("gui.recovery.not_repairable");
  return tFallback("gui.recovery.damage_detected", "Damage detected");
}

function recoveryDiagnosticDetail(task: Task): string {
  const parts = [recoveryDiagnosticSummary(task)];
  const capacity = recoveryCapacityDetail(task);
  if (capacity) parts.push(capacity);
  if (task.spec.kind === "repair_recovery" && recoveryResultOk(task.result) === true) {
    const repaired = recoveryRepairCountDetail(task);
    if (repaired) parts.push(repaired);
  }
  return parts.join(" · ");
}

function recoveryRepairOutput(task: Task): string {
  if (task.spec.kind !== "repair_recovery") return "";
  return String(task.result?.output ?? task.spec.output ?? task.result?.archive ?? task.spec.path);
}

function cleanHistoryDetail(detail: string): string {
  const oneLine = detail
    .replace(/(?:[A-Za-z]:)?(?:\/[^/\s]+)+\/([^/\s]+)/g, "$1")
    .replace(/\s+/g, " ")
    .trim();
  if (oneLine.length <= 140) return oneLine;
  return `${oneLine.slice(0, 137)}...`;
}

function sourceCleanupStatusLabel(status: string): string {
  if (status === "completed") {
    return tFallback(
      "gui.task.source_cleanup.completed",
      "Originals moved to {trash}",
      { trash: platformTrashName() },
    );
  }
  if (status === "partial") return tFallback("gui.task.source_cleanup.partial", "Some originals were kept");
  if (status === "blocked") return tFallback("gui.task.source_cleanup.blocked", "Originals could not be moved");
  if (status === "cancelled") return tFallback("gui.task.source_cleanup.cancelled", "Moving originals was cancelled");
  if (status === "failed") return tFallback("gui.task.source_cleanup.failed", "Moving originals failed");
  return tFallback("gui.task.source_cleanup.unknown", "Could not confirm where the originals were left");
}

function sourceCleanupHistoryDetail(task: Task): string | null {
  if (task.spec.kind !== "compress" || task.spec.post_success !== "trash_source") return null;
  const cleanup = readSourceCleanupSummary(task.result);
  if (!cleanup) return sourceCleanupStatusLabel("unknown");
  return tFallback(
    "gui.task.history.source_cleanup_counts",
    "{status} · Moved: {moved} · Kept: {kept} · Recovery needed: {recovery}",
    {
      status: sourceCleanupStatusLabel(cleanup.status),
      moved: cleanup.moved.toLocaleString(),
      kept: cleanup.kept.toLocaleString(),
      recovery: cleanup.recoveryRequired.toLocaleString(),
    },
  );
}

function createIntegrityDetail(task: Task): string | null {
  if (task.spec.kind !== "compress") return null;
  const result = readCreateResult(task.result, task.spec.dest, task.spec.split_size !== null);
  if (!result.testedAfterCreate) return null;
  return tFallback(
    "gui.task.history.integrity_tested",
    "Integrity test passed · {count} entries read",
    { count: result.entriesTestedAfterCreate.toLocaleString() },
  );
}

function taskHistoryDetail(task: Task, status: Extract<OperationStatus, "done" | "failed" | "info">): string {
  if (status === "failed") {
    if (task.spec.kind === "test" && task.result?.ok === false) {
      const problems = resultProblemTotal(task.result);
      return t("gui.task.history.test_failed_detail", { count: problems });
    }
    if (isRecoveryDiagnosticTask(task) && recoveryResultOk(task.result) === false) {
      return cleanHistoryDetail(recoveryDiagnosticDetail(task));
    }
    return task.error ? errorSummary(task.error) : t("gui.task.history.engine_failed");
  }

  const spec = task.spec;
  switch (spec.kind) {
    case "compress": {
      const result = readCreateResult(task.result, spec.dest, spec.split_size !== null);
      const size = formatBytes(result.totalBytes || task.done);
      const cleanup = sourceCleanupHistoryDetail(task);
      const integrity = createIntegrityDetail(task);
      let detail: string;
      if (spec.sfx_target) {
        detail = t("gui.task.history.sfx", { name: basename(result.primaryOutput) });
      } else {
        detail = result.split
          ? t("gui.task.history.compress_split", {
              name: basename(result.primaryOutput),
              count: result.volumeCount,
              size,
            })
          : t("gui.task.history.compress", { name: basename(result.primaryOutput), size });
      }
      const preserved = result.preservedOutputs.length > 0
        ? t("gui.task.history.preserved_outputs", {
            count: result.preservedOutputs.length.toLocaleString(),
          })
        : null;
      return cleanHistoryDetail([detail, integrity, preserved, cleanup].filter(Boolean).join(" · "));
    }
    case "publish_macos_sfx":
      return cleanHistoryDetail(t("gui.task.history.publish_macos_sfx", {
        name: basename(String(task.result?.primary_output ?? spec.output)),
        team: String(task.result?.team_id ?? ""),
      }));
    case "extract": {
      const dest = String(task.result?.dest ?? spec.dest);
      const outcome = readExtractResultOutcome(task.result);
      return cleanHistoryDetail(
        outcome.failed > 0
          ? t("gui.task.history.extract_attention", {
              dest: basename(dest),
              failed: outcome.failed,
              skipped: outcome.skipped,
            })
          : outcome.skipped > 0
            ? t("gui.task.history.extract_skipped", { dest: basename(dest), count: outcome.skipped })
          : t("gui.task.history.extract", { dest: basename(dest) }),
      );
    }
    case "batch_extract": {
      const extracted = Number(task.result?.extracted ?? 0);
      const total = Number(task.result?.archives ?? spec.items.length);
      const selected = Number(task.result?.selected_archives ?? total);
      const failed = Number(task.result?.failed ?? 0);
      return cleanHistoryDetail(
        t(
          selected > total
            ? "gui.task.history.batch_extract_grouped"
            : "gui.task.history.batch_extract",
          { extracted, total, selected, failed },
        ),
      );
    }
    case "extract_nested": {
      const dest = String(task.result?.dest ?? spec.dest);
      const outcome = readExtractResultOutcome(task.result);
      return cleanHistoryDetail(
        outcome.failed > 0
          ? t("gui.task.history.extract_nested_attention", {
              name: basename(spec.entry_path),
              dest: basename(dest),
              failed: outcome.failed,
              skipped: outcome.skipped,
            })
          : outcome.skipped > 0
            ? t("gui.task.history.extract_nested_skipped", {
                name: basename(spec.entry_path),
                dest: basename(dest),
                count: outcome.skipped,
              })
          : t("gui.task.history.extract_nested", { name: basename(spec.entry_path), dest: basename(dest) }),
      );
    }
    case "test": {
      const entries = Number(task.result?.entries ?? 0);
      return t("gui.task.history.test", { count: entries });
    }
    case "convert":
      return cleanHistoryDetail(t("gui.task.history.created", { name: basename(spec.dest) }));
    case "export_sqz": {
      const dest = String(task.result?.dest ?? spec.dest);
      return cleanHistoryDetail(t("gui.task.history.exported", { name: basename(dest) }));
    }
    case "repair_sqz": {
      const dest = String(task.result?.dest ?? spec.dest);
      return cleanHistoryDetail(t("gui.task.history.repaired_into", { name: basename(dest) }));
    }
    case "repair_zip": {
      const dest = String(task.result?.dest ?? spec.dest);
      return cleanHistoryDetail(t("gui.task.history.rebuilt_zip", { name: basename(dest) }));
    }
    case "protect": {
      const result = readRecoveryProtectionResult(
        task.result,
        spec.recovery ?? `${spec.path}.par2`,
      );
      return cleanHistoryDetail(
        result.outputs.length > 1
          ? t("gui.task.history.recovery_data_set", {
              name: basename(result.primaryOutput),
              count: result.outputs.length.toLocaleString(),
            })
          : t("gui.task.history.recovery_data", {
              name: basename(result.primaryOutput),
            }),
      );
    }
    case "verify_recovery":
      return cleanHistoryDetail(recoveryDiagnosticDetail(task));
    case "repair_recovery": {
      if (status !== "done" || recoveryResultOk(task.result) !== true) {
        return cleanHistoryDetail(recoveryDiagnosticDetail(task));
      }
      const output = recoveryRepairOutput(task);
      const detail = t("gui.task.history.repaired", { name: basename(output) });
      const repaired = recoveryRepairCountDetail(task);
      return cleanHistoryDetail(repaired ? `${detail} · ${repaired}` : detail);
    }
    case "update":
      return cleanHistoryDetail(t("gui.task.history.updated", { name: basename(spec.path) }));
    case "checksum": {
      const files = Number(task.result?.files_hashed ?? 0);
      const bytes = Number(task.result?.bytes_hashed ?? 0);
      return cleanHistoryDetail(t("gui.task.history.checksum", { count: files, size: formatBytes(bytes) }));
    }
    case "checksum_check": {
      const passed = Number(task.result?.passed ?? 0);
      const checked = Number(task.result?.checked ?? 0);
      const failed = Number(task.result?.failed ?? 0);
      return cleanHistoryDetail(t("gui.task.history.checksum_check", { passed, checked, failed }));
    }
    case "duplicate_scan": {
      const groups = Number(task.result?.duplicate_groups ?? 0);
      const reclaimable = Number(task.result?.reclaimable_bytes ?? 0);
      return cleanHistoryDetail(t("gui.task.history.duplicate_scan", { count: groups, size: formatBytes(reclaimable) }));
    }
  }
}

function finishToast(task: Task): void {
  const spec = task.spec;
  if (spec.kind === "batch_extract") {
    const extracted = Number(task.result?.extracted ?? 0);
    const failed = Number(task.result?.failed ?? 0);
    const outputs = Array.isArray(task.result?.outputs) ? task.result.outputs : [];
    const firstOutput = outputs[0];
    const firstDest = typeof firstOutput === "object" && firstOutput && "dest" in firstOutput
      ? String((firstOutput as { dest?: unknown }).dest ?? "")
      : "";
    task.revealPath = firstDest || spec.items[0]?.dest || null;
    if (revealAfterExtract && task.revealPath) {
      revealPath(task.revealPath);
    }
    const recoveredStructure = extractResultHasRecoveredZipStructure(task.result);
    const toast = {
      kind: failed > 0 || recoveredStructure ? "warning" : "success",
      title: t("gui.toast.batch_extract_done", { extracted, failed }),
      body: recoveredStructure
        ? t("gui.archive.zip_local_headers_recovered")
        : undefined,
    } satisfies Parameters<typeof pushToast>[0];
    if (task.revealPath) {
      pushToast({
        ...toast,
        action: { label: t("gui.toast.reveal"), run: () => revealPath(task.revealPath || "") },
      });
    } else {
      pushToast(toast);
    }
  } else if (spec.kind === "extract" || spec.kind === "extract_nested") {
    const dest = String(task.result?.dest ?? spec.dest);
    const bestEffort = spec.best_effort || task.result?.best_effort === true;
    const outcome = readExtractResultOutcome(task.result);
    const recoveredStructure = extractResultHasRecoveredZipStructure(task.result);
    task.revealPath = dest;
    if (revealAfterExtract) {
      revealPath(dest);
    }
    pushToast({
      kind: recoveredStructure || outcome.skipped > 0 || outcome.failed > 0
        ? "warning"
        : "success",
      title: outcome.failed > 0
        ? t("gui.toast.best_effort_extract_attention", {
            failed: outcome.failed,
            skipped: outcome.skipped,
          })
        : bestEffort
          ? t("gui.toast.best_effort_extract_done", { count: outcome.skipped })
          : outcome.skipped > 0
            ? t("gui.toast.extract_done_skipped", { count: outcome.skipped })
        : t("gui.toast.extract_done", { path: dest }),
      body: recoveredStructure
        ? t("gui.archive.zip_local_headers_recovered")
        : undefined,
      action: { label: t("gui.toast.reveal"), run: () => revealPath(dest) },
    });
  } else if (spec.kind === "compress") {
    const result = readCreateResult(task.result, spec.dest, spec.split_size !== null);
    const output = result.primaryOutput;
    const integrity = createIntegrityDetail(task);
    task.revealPath = output;
    if (result.preservedOutputs.length > 0 && spec.sfx_target) {
      pushToast({
        kind: "warning",
        title: t(result.preservedOutputs.length === 1
          ? "gui.toast.sfx_done_unsigned_preserved_one"
          : "gui.toast.sfx_done_unsigned_preserved", {
          name: basename(output),
          count: result.preservedOutputs.length.toLocaleString(),
        }),
        body: spec.sfx_target === "macos"
          ? t("gui.toast.sfx_done_unsigned_preserved_detail_macos")
          : t("gui.toast.sfx_done_unsigned_preserved_detail"),
        action: {
          label: t("gui.toast.reveal"),
          run: () => revealPath(output),
        },
      });
      return;
    }
    if (result.preservedOutputs.length > 0) {
      pushToast({
        kind: "warning",
        title: t(result.preservedOutputs.length === 1
          ? "gui.toast.compress_done_preserved_one"
          : "gui.toast.compress_done_preserved", {
          name: basename(output),
          count: result.preservedOutputs.length.toLocaleString(),
        }),
        body: t("gui.toast.compress_done_preserved_detail"),
        action: {
          label: t("gui.toast.reveal"),
          run: () => revealPath(output),
        },
      });
      return;
    }
    if (spec.sfx_target) {
      pushToast({
        kind: "warning",
        title: t("gui.toast.sfx_done_unsigned", { name: basename(output) }),
        body: integrity ?? undefined,
        action: {
          label: t("gui.toast.reveal"),
          run: () => revealPath(output),
        },
      });
      return;
    }
    pushToast({
      kind: "success",
      title: result.split
        ? t("gui.toast.compress_done_split", {
            name: basename(output),
            count: result.volumeCount,
          })
        : t("gui.toast.compress_done", {
            name: basename(output),
            size: formatBytes(result.totalBytes || task.done),
          }),
      body: integrity ?? undefined,
      action: {
        label: t("gui.toast.reveal"),
        run: () => revealPath(output),
      },
    });
  } else if (spec.kind === "publish_macos_sfx") {
    const output = String(task.result?.primary_output ?? spec.output);
    task.revealPath = output;
    pushToast({
      kind: "success",
      title: t("gui.toast.sfx_published", { name: basename(output) }),
      body: t("gui.toast.sfx_published_detail", {
        team: String(task.result?.team_id ?? ""),
      }),
      action: {
        label: t("gui.toast.reveal"),
        run: () => revealPath(output),
      },
    });
  } else if (spec.kind === "test") {
    const ok = task.result?.ok !== false;
    const entries = Number(task.result?.entries ?? 0);
    const problems = resultProblemTotal(task.result);
    pushToast(
      ok
        ? { kind: "success", title: t("gui.toast.test_ok", { count: entries }) }
        : { kind: "warning", title: t("gui.toast.test_failed", { count: problems }) },
    );
  } else if (spec.kind === "convert") {
    const result = readCreateResult(task.result, spec.dest, spec.split_size !== null);
    const output = result.primaryOutput;
    task.revealPath = output;
    if (result.preservedOutputs.length > 0) {
      pushToast({
        kind: "warning",
        title: t(result.preservedOutputs.length === 1
          ? "gui.toast.convert_done_preserved_one"
          : "gui.toast.convert_done_preserved", {
          name: basename(output),
          count: result.preservedOutputs.length.toLocaleString(),
        }),
        body: t("gui.toast.convert_done_preserved_detail"),
        action: {
          label: t("gui.toast.reveal"),
          run: () => revealPath(output),
        },
      });
      return;
    }
    pushToast({
      kind: "success",
      title: result.split
        ? t("gui.toast.convert_done_split", {
            name: basename(output),
            count: result.volumeCount,
          })
        : t("gui.toast.convert_done", { name: basename(output) }),
      action: {
        label: t("gui.toast.reveal"),
        run: () => revealPath(output),
      },
    });
  } else if (spec.kind === "export_sqz") {
    const dest = String(task.result?.dest ?? spec.dest);
    task.revealPath = dest;
    pushToast({
      kind: "success",
      title: t("gui.toast.export_sqz_done", { name: basename(dest) }),
      action: {
        label: t("gui.toast.reveal"),
        run: () => revealPath(dest),
      },
    });
  } else if (spec.kind === "repair_sqz") {
    const dest = String(task.result?.dest ?? spec.dest);
    task.revealPath = dest;
    pushToast({
      kind: "success",
      title: t("gui.toast.repair_sqz_done", { name: basename(dest) }),
      action: {
        label: t("gui.toast.reveal"),
        run: () => revealPath(dest),
      },
    });
  } else if (spec.kind === "repair_zip") {
    const dest = String(task.result?.dest ?? spec.dest);
    task.revealPath = dest;
    pushToast({
      kind: "success",
      title: t("gui.toast.repair_zip_done", { name: basename(dest) }),
      action: {
        label: t("gui.toast.reveal"),
        run: () => revealPath(dest),
      },
    });
  } else if (spec.kind === "protect") {
    const result = readRecoveryProtectionResult(
      task.result,
      spec.recovery ?? `${spec.path}.par2`,
    );
    task.revealPath = result.primaryOutput;
    pushToast({
      kind: "success",
      title: result.outputs.length > 1
        ? t("gui.toast.recovery_protect_done_set", {
            name: basename(result.primaryOutput),
            count: result.outputs.length.toLocaleString(),
          })
        : t("gui.toast.recovery_protect_done", {
            name: basename(result.primaryOutput),
          }),
      body: result.outputs.length > 1
        ? t("gui.toast.recovery_protect_done_detail")
        : undefined,
      action: {
        label: t("gui.toast.reveal"),
        run: () => revealPath(result.primaryOutput),
      },
    });
  } else if (spec.kind === "verify_recovery") {
    const ok = recoveryResultOk(task.result);
    pushToast({
      kind: ok === false ? "warning" : "success",
      title: ok === false
        ? tFallback(
          "gui.toast.recovery_verify_damage",
          "Recovery verification found damage in {name}",
          { name: basename(spec.path) },
        )
        : t("gui.toast.recovery_verify_ok", { name: basename(spec.path) }),
      body: recoveryCapacityDetail(task) ?? undefined,
    });
  } else if (spec.kind === "repair_recovery") {
    const ok = recoveryResultOk(task.result);
    const output = recoveryRepairOutput(task);
    const repaired = ok === true;
    if (repaired) task.revealPath = output;
    pushToast({
      kind: repaired ? "success" : "warning",
      title: repaired
        ? t("gui.toast.recovery_repair_done", { name: basename(output) })
        : tFallback(
          "gui.toast.recovery_repair_incomplete",
          "Repair did not complete for {name}",
          { name: basename(spec.path) },
        ),
      body: repaired
        ? recoveryRepairCountDetail(task) ?? recoveryCapacityDetail(task) ?? undefined
        : recoveryCapacityDetail(task) ?? undefined,
      action: repaired
        ? {
          label: t("gui.toast.reveal"),
          run: () => revealPath(output),
        }
        : undefined,
    });
  } else if (spec.kind === "update") {
    task.revealPath = spec.path;
    pushToast({
      kind: "success",
      title: t("gui.toast.update_done", { name: basename(spec.path) }),
      action: {
        label: t("gui.toast.reveal"),
        run: () => revealPath(spec.path),
      },
    });
  } else if (spec.kind === "checksum") {
    const files = Number(task.result?.files_hashed ?? 0);
    const bytes = Number(task.result?.bytes_hashed ?? 0);
    pushToast({
      kind: "success",
      title: t("gui.toast.checksum_done", {
        files,
        bytes: formatBytes(bytes),
      }),
    });
  } else if (spec.kind === "checksum_check") {
    const failed = Number(task.result?.failed ?? 0);
    const passed = Number(task.result?.passed ?? 0);
    pushToast({
      kind: failed > 0 ? "warning" : "success",
      title: t("gui.toast.checksum_check_done", {
        passed,
        failed,
      }),
    });
  } else if (spec.kind === "duplicate_scan") {
    const groups = Number(task.result?.duplicate_groups ?? 0);
    const reclaimable = Number(task.result?.reclaimable_bytes ?? 0);
    pushToast({
      kind: groups > 0 ? "warning" : "success",
      title: t("gui.toast.duplicate_scan_done", {
        groups,
        reclaimable: formatBytes(reclaimable),
      }),
    });
  }
}

function onProgress(ev: ProgressEvent): void {
  const task = find(ev.id);
  if (!task) {
    if (!locallyDismissed.has(ev.id)) rememberPending(pendingProgress, ev.id, ev);
    return;
  }
  if (!shouldApplySnapshotProgress(task.version, task.state, ev.version)) return;
  task.version = ev.version;
  task.done = ev.done;
  task.total = ev.total;
  task.current = ev.current;
  task.currentDone = ev.current_done ?? 0;
  task.currentTotal = ev.current_total ?? 0;
  task.scanEntries = ev.scanned_entries ?? null;
  task.speed = ev.speed;
  task.phase = ev.phase ?? null;
  task.interruptible = ev.interruptible ?? true;
}

function replayPending(id: number): void {
  const state = pendingStates.get(id);
  if (state) {
    pendingStates.delete(id);
    onState(state);
  }
  const progress = pendingProgress.get(id);
  if (progress) {
    pendingProgress.delete(id);
    onProgress(progress);
  }
}

function taskFromSnapshot(snapshot: JobSnapshot): Task {
  return {
    id: snapshot.id,
    version: snapshot.version,
    spec: snapshot.spec,
    title: titleFor(snapshot.spec),
    origin: snapshot.origin,
    ownedByRequester: snapshot.owned_by_requester,
    interaction: snapshot.interaction,
    state: snapshot.state,
    queuePosition: snapshot.queue_position,
    queueWaitReason: snapshot.queue_wait_reason,
    cpuThreads: snapshot.cpu_threads,
    streamBufferLimitBytes: snapshot.stream_buffer_limit_bytes,
    done: snapshot.progress.done,
    total: snapshot.progress.total,
    current: snapshot.progress.current,
    currentDone: snapshot.progress.current_done,
    currentTotal: snapshot.progress.current_total,
    scanEntries: snapshot.progress.scanned_entries ?? null,
    speed: snapshot.progress.speed,
    phase: snapshot.progress.phase ?? null,
    interruptible: snapshot.progress.interruptible ?? true,
    pausable: jobSupportsPause(snapshot.spec),
    error: snapshot.error,
    result: snapshot.result,
    revealPath: null,
    historyRecorded: true,
    localEffects: false,
    snapshotSeen: true,
    controlIntent: null,
    queueMoveIntent: null,
    expanded: snapshot.state === "failed" && errorNeedsInspection(snapshot.error),
  };
}

function applySnapshot(snapshot: JobSnapshot): void {
  let task = find(snapshot.id);
  if (!task) {
    if (isTerminalSnapshotState(snapshot.state)) clearPendingQuestions(snapshot.id);
    task = taskFromSnapshot(snapshot);
    store.tasks.push(task);
    replayPending(snapshot.id);
    return;
  }

  task.snapshotSeen = true;
  if (!shouldApplyFullSnapshot(task.version, task.state, snapshot.version, snapshot.state)) return;

  const previousState = task.state;
  task.version = snapshot.version;
  task.spec = snapshot.spec;
  task.title = titleFor(snapshot.spec);
  task.origin = snapshot.origin;
  task.ownedByRequester = snapshot.owned_by_requester;
  task.interaction = snapshot.interaction;
  task.state = snapshot.state;
  task.queuePosition = snapshot.queue_position;
  task.queueWaitReason = snapshot.queue_wait_reason;
  task.cpuThreads = snapshot.cpu_threads;
  task.streamBufferLimitBytes = snapshot.stream_buffer_limit_bytes;
  task.done = snapshot.progress.done;
  task.total = snapshot.progress.total;
  task.current = snapshot.progress.current;
  task.currentDone = snapshot.progress.current_done;
  task.currentTotal = snapshot.progress.current_total;
  task.scanEntries = snapshot.progress.scanned_entries ?? null;
  task.speed = snapshot.progress.speed;
  task.phase = snapshot.progress.phase ?? null;
  task.interruptible = snapshot.progress.interruptible ?? true;
  task.error = snapshot.error;
  task.result = snapshot.result;
  if (snapshot.state === "failed" && errorNeedsInspection(snapshot.error)) {
    task.expanded = true;
  }
  if (
    isTerminalSnapshotState(snapshot.state) ||
    (snapshot.state === "paused" && task.controlIntent === "pause") ||
    (snapshot.state === "running" && task.controlIntent === "resume")
  ) {
    task.controlIntent = null;
  }
  if (isTerminalSnapshotState(snapshot.state)) clearPendingQuestions(snapshot.id);
  runTerminalEffects(task, previousState);
}

function applySnapshotDelta(delta: JobSnapshotsDelta): void {
  if (snapshotRevision !== null && delta.revision < snapshotRevision) return;
  const visible = new Set(delta.upserts.map((snapshot) => snapshot.id));
  const removed = new Set(delta.removed);
  for (const id of removed) locallyDismissed.delete(id);
  if (delta.reset) {
    for (const id of locallyDismissed) {
      if (!visible.has(id)) locallyDismissed.delete(id);
    }
  }
  store.tasks = store.tasks.filter((task) => {
    if (removed.has(task.id)) return false;
    if (!delta.reset || !task.snapshotSeen) return true;
    return visible.has(task.id) && !locallyDismissed.has(task.id);
  });
  for (const snapshot of delta.upserts) {
    if (!locallyDismissed.has(snapshot.id)) applySnapshot(snapshot);
  }
  snapshotRevision = delta.revision;
}

function waitForSnapshotPoll(delayMs: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, delayMs));
}

async function reconcileSnapshotFeed(stopped: () => boolean): Promise<void> {
  while (!stopped()) {
    try {
      const generation = snapshotGeneration;
      const delta = await ipc.jobSnapshots(snapshotRevision);
      if (stopped()) return;
      if (generation === snapshotGeneration) applySnapshotDelta(delta);
    } catch {
      // Native startup and shutdown can briefly race the WebView. The next
      // bounded poll requests the same revision again.
    }
    await waitForSnapshotPoll(runningCount() > 0 ? 400 : 1_000);
  }
}

/** Wires this window's job event listeners once at startup. */
export async function initJobEvents(): Promise<() => void> {
  const listen = await currentWebviewWindowListener();
  const cleanup: Array<() => void> = [];
  let stopped = false;
  try {
    cleanup.push(await listen<ProgressEvent>("job://progress", (e) => onProgress(e.payload)));
    cleanup.push(await listen<StateEvent>("job://state", (e) => onState(e.payload)));
    cleanup.push(await listen<AskConflictEvent>("job://ask-conflict", (e) => {
      store.conflict = e.payload;
    }));
    cleanup.push(await listen<AskPasswordEvent>("job://ask-password", (e) => {
      store.password = e.payload;
    }));
    void reconcileSnapshotFeed(() => stopped);
  } catch (error) {
    stopped = true;
    for (const dispose of cleanup) dispose();
    throw error;
  }
  return () => {
    stopped = true;
    for (const dispose of cleanup) dispose();
  };
}

export function tasks(): Task[] {
  return store.tasks;
}

type PreviewTaskKind =
  | "compress"
  | "compress_split"
  | "compress_sfx"
  | "compress_sfx_failure"
  | "recovery_cleanup_ready"
  | "recovery_cleanup_unconfirmed"
  | "recovery_cleanup_record"
  | "extract"
  | "extract_unknown_current"
  | "batch_extract"
  | "test"
  | "checksum"
  | "checksum_check"
  | "recovery_protect"
  | "recovery_verify_repairable"
  | "recovery_verify_multi_file_repairable"
  | "recovery_verify_over_capacity"
  | "update_scan"
  | "update_verify"
  | "update_commit";

function previewPhase(kind: PreviewTaskKind): ProgressPhase | null {
  if (kind === "recovery_protect") return "recovery_finalize";
  if (isRecoveryCleanupPreview(kind)) return "recovery_finalize";
  if (isRecoveryPreview(kind)) return "recovery_verify";
  if (kind === "compress_split") return "output_split";
  if (kind === "update_verify") return "update_verify";
  if (kind === "update_commit") return "update_commit";
  return null;
}

function isUpdatePreview(kind: PreviewTaskKind): boolean {
  return kind === "update_scan" || kind === "update_verify" || kind === "update_commit";
}

function isRecoveryPreview(kind: PreviewTaskKind): boolean {
  return kind === "recovery_verify_repairable"
    || kind === "recovery_verify_multi_file_repairable"
    || kind === "recovery_verify_over_capacity";
}

function isRecoveryCleanupPreview(kind: PreviewTaskKind): boolean {
  return kind === "recovery_cleanup_ready"
    || kind === "recovery_cleanup_unconfirmed"
    || kind === "recovery_cleanup_record";
}

function previewTaskSpec(kind: PreviewTaskKind): JobSpec {
  if (isUpdatePreview(kind)) {
    return {
      kind: "update",
      path: `${sampleRoot}/product-backup.zip`,
      add: [`${sampleRoot}/incoming-assets`],
      delete: [],
      rename: [],
      mkdir: [],
      excludes: [".DS_Store"],
      content_policy: "keep_all_files",
      password: null,
      level: 5,
    };
  }
  if (kind === "compress" || kind === "compress_split" || kind === "compress_sfx" || kind === "compress_sfx_failure") {
    const sfx = kind === "compress_sfx" || kind === "compress_sfx_failure";
    return {
      kind: "compress",
      inputs: [`${sampleRoot}/reports`, `${sampleRoot}/photos`],
      dest: sfx ? `${sampleOutputRoot}/Installer.app` : `${sampleOutputRoot}/product-backup.zip`,
      level: 5,
      password: null,
      encrypt_names: false,
      split_size: kind === "compress_split" ? 8 * 1024 * 1024 : null,
      split_mode: "generic",
      excludes: [],
      content_policy: "keep_all_files",
      sqz_inner_format: null,
      sfx_target: sfx ? "macos" : null,
      replace_existing: kind !== "compress",
      replacement_guard: null,
      completion: "none",
      post_success: "keep_source",
      test_after_create: false,
    };
  }
  if (kind === "extract" || kind === "extract_unknown_current") {
    return {
      kind: "extract",
      path: `${sampleRoot}/product-backup.zip`,
      dest: `${sampleOutputRoot}/product-backup`,
      expected_destination: null,
      expected_input_guard: null,
      selection: null,
      overwrite: "ask",
      symlinks: "preserve",
      smart: true,
      encoding: null,
      password: null,
      verify_sfx: false,
      best_effort: false,
    };
  }
  if (kind === "recovery_protect") {
    return {
      kind: "protect",
      path: `${sampleRoot}/product-backup.zip`,
      redundancy: 10,
      recovery: `${sampleOutputRoot}/product-backup.zip.par2`,
    };
  }
  if (isRecoveryCleanupPreview(kind)) {
    return {
      kind: "repair_recovery",
      path: `${sampleRoot}/product-backup.zip`,
      output: `${sampleOutputRoot}/product-backup.repaired.zip`,
      output_directory: false,
      recovery: `${sampleRoot}/product-backup.zip.par2`,
    };
  }
  if (isRecoveryPreview(kind)) {
    return {
      kind: "verify_recovery",
      path: `${sampleRoot}/product-backup.zip`,
      recovery: `${sampleRoot}/product-backup.zip.par2`,
    };
  }
  if (kind === "test") {
    return {
      kind: "test",
      path: `${sampleRoot}/product-backup.zip`,
      encoding: null,
      password: null,
    };
  }
  if (kind === "checksum") {
    return {
      kind: "checksum",
      inputs: [`${sampleRoot}/photos`],
      excludes: [],
      algorithm: "sha256",
    };
  }
  if (kind === "checksum_check") {
    return {
      kind: "checksum_check",
      manifest: `${sampleRoot}/photos/SHA256SUMS`,
      algorithm: "sha256",
    };
  }
  return {
    kind: "batch_extract",
    items: [
      {
        path: `${sampleRoot}/client-data.zip`,
        dest: `${sampleOutputRoot}/client-data`,
        encoding: null,
        password: null,
        best_effort: false,
      },
      {
        path: `${sampleRoot}/photos.7z`,
        dest: `${sampleOutputRoot}/photos`,
        encoding: null,
        password: null,
        best_effort: false,
      },
    ],
    overwrite: "ask",
    symlinks: "preserve",
    smart: true,
  };
}

function previewTaskResult(kind: PreviewTaskKind): Record<string, unknown> {
  if (isUpdatePreview(kind)) return { operation: "update" };
  if (kind === "recovery_protect") {
    const recovery = `${sampleOutputRoot}/product-backup.zip.par2`;
    return {
      operation: "protect",
      ok: true,
      archive: `${sampleRoot}/product-backup.zip`,
      recovery,
      outputs: [
        recovery,
        `${sampleOutputRoot}/product-backup.zip.vol00+01.par2`,
        `${sampleOutputRoot}/product-backup.zip.vol01+02.par2`,
        `${sampleOutputRoot}/product-backup.zip.vol03+04.par2`,
        `${sampleOutputRoot}/product-backup.zip.vol07+08.par2`,
        `${sampleOutputRoot}/product-backup.zip.vol15+16.par2`,
      ],
      source_file_count: 1,
      redundancy_percent: 10,
    };
  }
  if (isRecoveryPreview(kind)) {
    const overCapacity = kind === "recovery_verify_over_capacity";
    return {
      operation: "verify",
      ok: false,
      archive: `${sampleRoot}/product-backup.zip`,
      recovery: `${sampleRoot}/product-backup.zip.par2`,
      output: null,
      tool: "rust-par2",
      redundancy_percent: null,
      source_file_count: kind === "recovery_verify_multi_file_repairable" ? 2 : 1,
      status_code: 1,
      metrics: {
        all_correct: false,
        repair_possible: !overCapacity,
        blocks_needed: overCapacity ? 12 : 3,
        recovery_blocks_available: overCapacity ? 4 : 8,
        blocks_repaired: null,
        files_repaired: null,
        no_damage: false,
      },
      stdout: "",
      stderr: "damage found",
    };
  }
  if (kind === "batch_extract") {
    return {
      operation: "batch_extract",
      archives: 2,
      extracted: 2,
      failed: 0,
      skipped: 0,
      outputs: [
        { path: `${sampleRoot}/client-data.zip`, dest: `${sampleOutputRoot}/client-data` },
        { path: `${sampleRoot}/photos.7z`, dest: `${sampleOutputRoot}/photos` },
      ],
    };
  }
  if (kind === "extract" || kind === "extract_unknown_current") {
    const problems = Array.from(
      { length: 20 },
      (_, index) => `damaged/item-${String(index + 1).padStart(2, "0")}.bin: checksum mismatch`,
    );
    return {
      operation: "extract",
      dest: "/tmp/squallz-output/product-backup",
      best_effort: true,
      skipped: 30,
      problems,
      problems_total: 30,
      problems_truncated: true,
      counts: {
        destination: "/tmp/squallz-output/product-backup",
        selected_entries: 42,
        created: 10,
        directories: 2,
        skipped: 0,
        replaced: 0,
        renamed: 0,
        failed: 30,
        output_bytes: 48_000_000,
      },
    };
  }
  if (kind === "test") {
    const problemMessages = Array.from(
      { length: 20 },
      (_, index) => `damaged/item-${String(index + 1).padStart(2, "0")}.bin: checksum mismatch`,
    );
    return {
      operation: "test",
      ok: false,
      entries: 42,
      entries_tested: 42,
      problems: problemMessages,
      problems_total: 30,
      problems_truncated: true,
    };
  }
  if (kind === "checksum") {
    return {
      operation: "checksum",
      algorithm: "sha256",
      files_hashed: 12,
      bytes_hashed: 86_000_000,
      items: [
        {
          path: `${sampleRoot}/photos/DSC_1930.JPG`,
          size: 18_200_000,
          digest: "9bc1b2a288b3f53f0c448c9a6fe2c7e97e0d8bb74f7e7f548d3f1ad4020cc714",
        },
        {
          path: `${sampleRoot}/photos/DSC_1488.JPG`,
          size: 9_200_000,
          digest: "37166b84dfd4083c0f6fb7b99d892bc3ef8ff07c9a1714ad9f323bdb37e9f9a2",
        },
      ],
    };
  }
  if (kind === "checksum_check") {
    return {
      operation: "checksum_check",
      passed: 12,
      checked: 12,
      failed: 0,
      items: [
        {
          path: `${sampleRoot}/photos/DSC_1930.JPG`,
          expected: "9bc1b2a288b3f53f0c448c9a6fe2c7e97e0d8bb74f7e7f548d3f1ad4020cc714",
          actual: "9bc1b2a288b3f53f0c448c9a6fe2c7e97e0d8bb74f7e7f548d3f1ad4020cc714",
          ok: true,
        },
      ],
    };
  }
  if (kind === "compress_split") {
    const output = `${sampleOutputRoot}/product-backup.zip`;
    const outputs = Array.from(
      { length: 12 },
      (_, index) => `${output}.${String(index + 1).padStart(3, "0")}`,
    );
    const preservedOutputs = outputs.slice(0, 3).map((path, index) => {
      const name = basename(path);
      return `${sampleOutputRoot}/.${name}.split-backup-940008-${index}.tmp.${name}`;
    });
    return {
      operation: "create",
      primary_output: outputs[0],
      outputs,
      preserved_outputs: preservedOutputs,
      total_bytes: 92_760_416,
      volume_count: outputs.length,
      split: true,
    };
  }
  if (kind === "compress_sfx") {
    const output = `${sampleOutputRoot}/Installer.app`;
    return {
      operation: "create_sfx",
      primary_output: output,
      outputs: [output],
      preserved_outputs: [
        `${sampleOutputRoot}/.squallz-sfx-holder-940009-1/previous`,
      ],
      total_bytes: 48_000_000,
      volume_count: 1,
      split: false,
      requires_signing: true,
      sfx_target: "macos",
      layout: "macos_app",
    };
  }
  const output = `${sampleOutputRoot}/product-backup.zip`;
  return {
    operation: "create",
    primary_output: output,
    outputs: [output],
    total_bytes: 24_000_000,
    volume_count: 1,
    split: false,
  };
}

function previewRevealPath(kind: PreviewTaskKind): string | null {
  if (isUpdatePreview(kind)) return `${sampleRoot}/product-backup.zip`;
  if (kind === "recovery_protect") return `${sampleOutputRoot}/product-backup.zip.par2`;
  if (isRecoveryPreview(kind)) return null;
  if (kind === "compress") return `${sampleOutputRoot}/product-backup.zip`;
  if (kind === "compress_split") return `${sampleOutputRoot}/product-backup.zip.001`;
  if (kind === "compress_sfx") return `${sampleOutputRoot}/Installer.app`;
  if (kind === "extract" || kind === "extract_unknown_current") return `${sampleOutputRoot}/product-backup`;
  if (kind === "test") return null;
  if (kind === "checksum") return `${sampleRoot}/photos`;
  if (kind === "checksum_check") return `${sampleRoot}/photos/SHA256SUMS`;
  return `${sampleOutputRoot}/client-data`;
}

function previewProgress(kind: PreviewTaskKind, state: Extract<JobStateName, "done" | "running">) {
  if (kind === "recovery_protect") {
    return {
      done: state === "done" ? 1 : 0,
      total: 1,
      current: "product-backup.zip.par2",
      currentDone: 0,
      currentTotal: 0,
      speed: 0,
    };
  }
  if (kind === "update_scan") {
    return {
      done: 0,
      total: 0,
      current: "incoming-assets/icons/app-icon@2x.png",
      currentDone: 0,
      currentTotal: 0,
      speed: 0,
    };
  }
  if (kind === "update_verify") {
    return {
      done: 438_000_000,
      total: 730_000_000,
      current: "product-backup.zip",
      currentDone: 0,
      currentTotal: 0,
      speed: 820_000_000,
    };
  }
  if (kind === "update_commit") {
    return {
      done: 0,
      total: 0,
      current: "product-backup.zip",
      currentDone: 0,
      currentTotal: 0,
      speed: 0,
    };
  }
  if (kind === "batch_extract") {
    return {
      done: state === "done" ? 2 : 1,
      total: 2,
      current: "photos/IMG_2042.dng",
      currentDone: state === "done" ? 3_200_000 : 1_280_000,
      currentTotal: 3_200_000,
      speed: state === "running" ? 18_400_000 : 0,
    };
  }
  if (isRecoveryPreview(kind)) {
    return {
      done: state === "done" ? 1_000 : 380,
      total: 1_000,
      current: "product-backup.zip",
      currentDone: 0,
      currentTotal: 0,
      speed: 0,
    };
  }
  if (kind === "compress" || kind === "compress_split" || kind === "compress_sfx" || kind === "compress_sfx_failure") {
    const total = kind === "compress_split" ? 92_760_416 : kind === "compress" ? 24_000_000 : 48_000_000;
    return {
      done: state === "done" ? total : Math.floor(total * 0.4),
      total,
      current: kind === "compress_split" ? "product-backup.zip.002" : "reports/Launch plan.pdf",
      currentDone: kind === "compress_split" ? 0 : state === "done" ? 3_800_000 : 1_420_000,
      currentTotal: kind === "compress_split" ? 0 : 3_800_000,
      speed: state === "running" ? 12_800_000 : 0,
    };
  }
  if (kind === "extract_unknown_current") {
    return {
      done: state === "done" ? 48_000_000 : 19_200_000,
      total: 48_000_000,
      current: "reports/Launch plan.pdf",
      currentDone: 0,
      currentTotal: 0,
      speed: state === "running" ? 21_000_000 : 0,
    };
  }
  if (kind === "test") {
    return {
      done: state === "done" ? 48_000_000 : 19_200_000,
      total: 48_000_000,
      current: "reports/Launch plan.pdf",
      currentDone: 0,
      currentTotal: 0,
      speed: state === "running" ? 17_600_000 : 0,
    };
  }
  if (kind === "checksum") {
    return {
      done: state === "done" ? 86_000_000 : 34_400_000,
      total: 86_000_000,
      current: "photos/DSC_1930.JPG",
      currentDone: 0,
      currentTotal: 0,
      speed: state === "running" ? 24_000_000 : 0,
    };
  }
  if (kind === "checksum_check") {
    return {
      done: state === "done" ? 86_000_000 : 34_400_000,
      total: 86_000_000,
      current: "photos/DSC_1930.JPG",
      currentDone: 0,
      currentTotal: 0,
      speed: state === "running" ? 22_000_000 : 0,
    };
  }
  return {
    done: state === "done" ? 48_000_000 : 19_200_000,
    total: 48_000_000,
    current: "reports/Launch plan.pdf",
    currentDone: state === "done" ? 4_096_000 : 1_920_000,
    currentTotal: 4_096_000,
    speed: state === "running" ? 21_000_000 : 0,
  };
}

function previewTaskOffset(kind: PreviewTaskKind): number {
  if (kind === "recovery_protect") return 17;
  if (kind === "update_scan") return 11;
  if (kind === "update_verify") return 12;
  if (kind === "update_commit") return 13;
  if (kind === "recovery_verify_repairable") return 14;
  if (kind === "recovery_verify_multi_file_repairable") return 16;
  if (kind === "recovery_verify_over_capacity") return 15;
  if (kind === "compress") return 1;
  if (kind === "compress_split") return 8;
  if (kind === "compress_sfx") return 9;
  if (kind === "compress_sfx_failure") return 10;
  if (kind === "recovery_cleanup_ready") return 18;
  if (kind === "recovery_cleanup_unconfirmed") return 19;
  if (kind === "recovery_cleanup_record") return 20;
  if (kind === "extract") return 2;
  if (kind === "extract_unknown_current") return 4;
  if (kind === "test") return 5;
  if (kind === "checksum") return 6;
  if (kind === "checksum_check") return 7;
  return 3;
}

function installTaskPreview(kind: PreviewTaskKind, state: Extract<JobStateName, "done" | "running">): number | null {
  if (!import.meta.env.DEV) return null;
  const id = 940_000 + (state === "running" ? 100 : 0) + previewTaskOffset(kind);
  if (find(id)) return id;

  const spec = previewTaskSpec(kind);
  const progress = previewProgress(kind, state);
  const previewState = kind === "compress_sfx_failure" || isRecoveryCleanupPreview(kind)
    ? "failed"
    : state;
  const target = isRecoveryCleanupPreview(kind)
    ? `${sampleOutputRoot}/product-backup.repaired.zip`
    : `${sampleOutputRoot}/Installer.app`;
  const journal = `${sampleOutputRoot}/.squallz-sfx-transaction.json`;
  const holder = `${sampleOutputRoot}/.squallz-sfx-holder-940010-3`;
  const workspace =
    `${sampleOutputRoot}/.product-backup.repaired.zip.sqz-par2-repair-940018-1.work`;
  const recoveryJournal =
    `${sampleOutputRoot}/.squallz-par2-repair-8f3d4a9e1c7b2d5f.json`;
  const error: ErrorDto = isRecoveryCleanupPreview(kind)
    ? {
      key: kind === "recovery_cleanup_ready"
        ? "error.recovery_cleanup_output_ready"
        : kind === "recovery_cleanup_unconfirmed"
          ? "error.recovery_cleanup_unconfirmed"
          : "error.recovery_cleanup_record",
      params: kind === "recovery_cleanup_record"
        ? { target, journal: recoveryJournal }
        : { target, workspace, journal: recoveryJournal },
      detail: kind === "recovery_cleanup_ready"
        ? `PAR2 repair completed and the repaired copy is ready at ${target}, but its private workspace could not be removed; automatic recovery record: ${recoveryJournal}; exact workspace: ${workspace}`
        : kind === "recovery_cleanup_unconfirmed"
          ? `PAR2 repair was not confirmed, and its private workspace could not be removed; automatic recovery record: ${recoveryJournal}; exact workspace: ${workspace}`
          : `The target-bound PAR2 recovery record at ${recoveryJournal} is damaged; no workspace path was trusted or removed.`,
    }
    : {
      key: "error.sfx_recovery",
      params: {
        target,
        journal,
        count: "4",
        paths: [journal, holder, `${holder}/previous`, `${holder}/replacement`].join("\n"),
      },
      detail: `SFX replacement requires manual recovery. Inspect target ${target} and the listed transaction paths.`,
    };

  store.tasks.push({
    id,
    version: 0,
    spec,
    title: titleFor(spec),
    origin: "app",
    ownedByRequester: true,
    interaction: null,
    state: previewState,
    queuePosition: null,
    queueWaitReason: null,
    cpuThreads: kind.startsWith("compress") ? 8 : 1,
    streamBufferLimitBytes: kind.startsWith("compress") ? 512 * 1024 * 1024 : null,
    done: progress.done,
    total: progress.total,
    current: progress.current,
    currentDone: progress.currentDone,
    currentTotal: progress.currentTotal,
    scanEntries: kind === "update_scan" && state === "running" ? 128 : null,
    speed: progress.speed,
    phase: previewPhase(kind),
    interruptible: kind !== "update_commit",
    pausable: jobSupportsPause(spec),
    error: previewState === "failed" ? error : null,
    result: previewState === "done" ? previewTaskResult(kind) : null,
    revealPath: previewState === "done" ? previewRevealPath(kind) : null,
    historyRecorded: true,
    localEffects: false,
    snapshotSeen: false,
    controlIntent: null,
    queueMoveIntent: null,
    expanded: true,
  });
  return id;
}

export function installCompletedTaskPreview(kind: PreviewTaskKind): number | null {
  return installTaskPreview(kind, "done");
}

export function installActiveTaskPreview(kind: PreviewTaskKind): number | null {
  return installTaskPreview(kind, "running");
}

export function installTaskQueuePreview(
  waitReason: Exclude<QueueWaitReason, "queue_order"> = "parallel_limit",
): number | null {
  if (!import.meta.env.DEV) return null;
  const runningId = installTaskPreview("compress", "running");
  const runningTask = runningId === null ? null : find(runningId);
  if (runningTask) {
    runningTask.origin = "file_manager";
    runningTask.ownedByRequester = false;
    runningTask.interaction = "password";
  }
  installTaskPreview("compress_split", "done");
  installTaskPreview("compress_sfx_failure", "done");

  const firstWaitingKind: PreviewTaskKind = waitReason === "cpu_budget" ? "compress" : "extract";
  const waitingKinds: PreviewTaskKind[] = [firstWaitingKind, "checksum"];
  waitingKinds.forEach((kind, index) => {
    const id = 941_000 + index;
    if (find(id)) return;
    const spec = previewTaskSpec(kind);
    store.tasks.push({
      id,
      version: 0,
      spec,
      title: titleFor(spec),
      origin: "app",
      ownedByRequester: true,
      interaction: null,
      state: "queued",
      queuePosition: index + 1,
      queueWaitReason: index === 0 ? waitReason : "queue_order",
      cpuThreads: index === 0 && waitReason === "cpu_budget" ? 8 : 1,
      streamBufferLimitBytes: kind === "compress" ? 512 * 1024 * 1024 : null,
      done: 0,
      total: 0,
      current: "",
      currentDone: 0,
      currentTotal: 0,
      scanEntries: null,
      speed: 0,
      phase: null,
      interruptible: true,
      pausable: jobSupportsPause(spec),
      error: null,
      result: null,
      revealPath: null,
      historyRecorded: false,
      localEffects: false,
      snapshotSeen: false,
      controlIntent: null,
      queueMoveIntent: null,
      expanded: false,
    });
  });

  return runningId;
}

/** The task mirrored by foreground progress: running, else paused, else waiting. */
export function activeTask(): Task | null {
  return (
    store.tasks.find((x) => x.state === "running") ??
    store.tasks.find((x) => x.state === "paused") ??
    store.tasks.find((x) => x.state === "queued") ??
    null
  );
}

export function queuedCount(): number {
  return store.tasks.filter((x) => x.state === "queued").length;
}

export function runningCount(): number {
  return store.tasks.filter(
    (x) => x.state === "running" || x.state === "queued" || x.state === "paused",
  ).length;
}

async function requestTaskControl(
  id: number,
  intent: TaskControlIntent,
  request: () => Promise<void>,
): Promise<void> {
  const task = find(id);
  if (task) task.controlIntent = intent;
  try {
    await request();
  } catch {
    const current = find(id);
    if (current?.controlIntent === intent) current.controlIntent = null;
    pushToast({
      kind: "warning",
      title: t("gui.task.control_failed"),
    });
  }
}

export function pauseTask(id: number): void {
  void requestTaskControl(id, "pause", () => ipc.pauseJob(id));
}

export function resumeTask(id: number): void {
  void requestTaskControl(id, "resume", () => ipc.resumeJob(id));
}

async function requestQueueMove(
  id: number,
  intent: TaskQueueMoveIntent,
  request: () => Promise<void>,
): Promise<void> {
  const task = find(id);
  if (!task || task.state !== "queued" || task.queueMoveIntent !== null) return;
  task.queueMoveIntent = intent;
  try {
    await request();
  } catch {
    pushToast({
      kind: "warning",
      title: t("gui.task.queue_move_failed"),
    });
    return;
  } finally {
    const current = find(id);
    if (current?.queueMoveIntent === intent) current.queueMoveIntent = null;
  }
  try {
    const delta = await ipc.jobSnapshots(snapshotRevision);
    applySnapshotDelta(delta);
  } catch {
    // The bounded snapshot poll will retry with the same revision.
  }
}

export function moveTaskEarlier(id: number): void {
  void requestQueueMove(id, "earlier", () => ipc.moveJobEarlier(id));
}

export function moveTaskLater(id: number): void {
  void requestQueueMove(id, "later", () => ipc.moveJobLater(id));
}

export function moveTaskBefore(id: number, beforeId: number | null): void {
  void requestQueueMove(id, "position", () => ipc.moveJobBefore(id, beforeId));
}

export function setTaskExpanded(id: number, expanded: boolean): void {
  const task = find(id);
  if (task) task.expanded = expanded;
}

export function cancelTask(id: number): void {
  // Cancelling also dismisses an open question modal of this job.
  clearPendingQuestions(id);
  void requestTaskControl(id, "cancel", () => ipc.cancelJob(id));
}

export function retryTask(task: Task): void {
  const i = store.tasks.findIndex((x) => x.id === task.id);
  if (i >= 0) store.tasks.splice(i, 1);
  void submitJob(task.spec);
}

/** Removes selected terminal rows after the backend records their dismissal. */
export async function clearFinished(ids: readonly number[]): Promise<boolean> {
  const rollback = store.tasks
    .map((task, index) => ({ task, index }))
    .filter(({ task }) => ids.includes(task.id));
  for (const id of ids) locallyDismissed.add(id);
  snapshotGeneration += 1;
  snapshotRevision = null;
  try {
    await ipc.dismissJobSnapshots([...ids]);
  } catch {
    for (const id of ids) locallyDismissed.delete(id);
    snapshotGeneration += 1;
    snapshotRevision = null;
    for (const { task, index } of rollback) {
      if (!find(task.id)) store.tasks.splice(Math.min(index, store.tasks.length), 0, task);
    }
    pushToast({ kind: "warning", title: t("gui.task.clear_failed") });
    return false;
  }
  const selected = new Set(ids);
  store.tasks = store.tasks.filter((task) => {
    if (task.state === "queued" || task.state === "running" || task.state === "paused") {
      return true;
    }
    return !selected.has(task.id);
  });
  return true;
}

/* ---- Conflict modal ---- */

export function pendingConflict(): AskConflictEvent | null {
  return store.conflict;
}

export function answerConflict(decision: string, applyAll: boolean): void {
  const c = store.conflict;
  if (!c) return;
  store.conflict = null;
  void ipc.answerConflict(c.id, decision, applyAll).catch(() => {
    pushToast({ kind: "warning", title: t("gui.task.answer_failed") });
  });
}

/* ---- Password modal for running jobs ---- */

export function pendingPassword(): AskPasswordEvent | null {
  return store.password;
}

export function answerPassword(password: string | null): void {
  const p = store.password;
  if (!p) return;
  store.password = null;
  void ipc.answerPassword(p.id, password).catch(() => {
    pushToast({ kind: "warning", title: t("gui.task.answer_failed") });
  });
}

/** Localized error text for a failed task row. */
export function taskErrorText(task: Task): string {
  return task.error ? errorSummary(task.error) : "";
}
