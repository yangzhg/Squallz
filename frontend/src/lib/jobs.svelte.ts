import { untrack } from "svelte";
import { parseJobResult, type JobResult } from "./job-result";
// Job store: reduces authoritative backend snapshots and feeds foreground progress,
// status text, and conflict/password prompts.

import {
  ipc,
  type JobConflictPrompt,
  type JobPasswordPrompt,
  type ErrorDto,
  type JobInteraction,
  type JobOrigin,
  type JobQuestion,
  type JobSnapshot,
  type JobSnapshotsDelta,
  type JobSpec,
  type ProgressPhase,
  type QueueWaitReason,
} from "./ipc";
import { errorSummary } from "./error-presentation";
import {
  isTerminalSnapshotState,
  localSubmissionPromotion,
  outputPasswordRequired,
  shouldApplyFullSnapshot,
  snapshotQuestion,
} from "./job-snapshot";
import { t, tFallback } from "./i18n.svelte";
import { jobTitleFor } from "./job-title";
import { pushToast, removeToastByKey, type Toast } from "./toasts.svelte";
import { taskResultPresentation } from "./task-outcome";
import { readCreateResult } from "./create-result";
import { openCreatedOutputWithFallback } from "./create-completion";
import { recordOperation } from "./history.svelte";
import { platformFileManagerName, platformTrashName } from "./platform-labels";
import {
  readSourceCleanupSummary,
  shouldRefreshSourceCleanupRecovery,
} from "./source-cleanup";
import { currentWebviewWindowListener } from "./tauri-events";
import { createTaskPreview, createTaskPreviewHistory, type PreviewTaskKind } from "./task-preview";

export type JobStateName = JobSnapshot["state"];
export type TaskControlIntent = "cancel" | "pause" | "resume";
export type TaskQueueMoveIntent = "earlier" | "later" | "position";

export interface Task {
  id: number;
  /** Last backend version applied to this task. */
  version: number;
  spec: JobSpec;
  outputPasswordRequired: boolean;
  title: string;
  origin: JobOrigin;
  ownedByRequester: boolean;
  interaction: JobInteraction | null;
  /** Last owned backend prompt, retained while an answer or cancellation is pending. */
  question: JobQuestion | null;
  actionFailure: TaskControlIntent | "answer" | null;
  /** The last known state is retained while snapshot synchronization is unavailable. */
  statusStale: boolean;
  answeredQuestionVersion: number;
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
  result: JobResult | null;
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

export type JobSnapshotStatus = "loading" | "ready" | "unavailable";

const store = $state({
  tasks: [] as Task[],
  snapshotStatus: "loading" as JobSnapshotStatus,
});

const locallyDismissed = new Set<number>();
let snapshotRevision: number | null = null;
let snapshotGeneration = 0;
let revealAfterExtract = $state(false);
let presentedTaskId: number | null = null;
let createCompletionHandler: ((path: string) => Promise<boolean | void> | boolean | void) | null = null;
let sourceCleanupRecoveryRefreshHandler: (() => Promise<void> | void) | null = null;

/** Results already presented in task details do not also need a notification. */
export function setPresentedTaskId(id: number | null): void {
  presentedTaskId = id;
  if (id === null) return;
  untrack(() => {
    removeToastByKey(`task:${id}:result`);
    removeToastByKey(`task:${id}:source-cleanup`);
  });
}

function pushTaskResultToast(
  task: Task,
  kind: "result" | "source-cleanup",
  toast: Omit<Toast, "id" | "key">,
): void {
  if (task.id === presentedTaskId) return;
  pushToast({ ...toast, key: `task:${task.id}:${kind}` });
}

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
      title: tFallback("gui.toast.reveal_failed"),
      body: tFallback(
        "gui.toast.reveal_failed_path",
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
  if (spec.kind === "update") {
    return { ...spec, password: null, expected_archive_id: null };
  }
  if (spec.kind === "extract_nested" || spec.kind === "test") {
    return { ...spec, password: null };
  }
  return spec;
}

/** Submits a job and registers it in the local task list. */
export async function submitJob(spec: JobSpec): Promise<number> {
  const snapshot = await ipc.submitJob(spec);
  applySnapshot(snapshot);
  const task = find(snapshot.id);
  if (task) {
    const promotion = localSubmissionPromotion(task.state, task.localEffects);
    task.localEffects = true;
    if (promotion.resetHistory) task.historyRecorded = false;
    if (promotion.replayTerminal) runTerminalEffects(task, "running");
  }
  return snapshot.id;
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
    pushTaskResultToast(task, "result", { kind: "info", title: t("gui.toast.cancelled") });
  }
}

function clearPendingQuestions(id: number): void {
  const task = find(id);
  if (task) {
    task.question = null;
    task.interaction = null;
    task.actionFailure = null;
  }
}

function errorNeedsInspection(error: ErrorDto | null): boolean {
  return error?.key === "error.sfx_recovery"
    || error?.key === "error.recovery_cleanup_output_ready"
    || error?.key === "error.recovery_cleanup_unconfirmed"
    || error?.key === "error.recovery_cleanup_record";
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
        title: tFallback("gui.toast.open_in_squallz_unavailable"),
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
      title: tFallback("gui.toast.open_in_squallz_failed"),
    });
  }
}

function reportSourceCleanup(task: Task): void {
  if (task.spec.kind !== "compress" || task.spec.post_success !== "trash_source") return;
  const cleanup = readSourceCleanupSummary(task.result);
  if (!cleanup) {
    pushTaskResultToast(task, "source-cleanup", {
      kind: "warning",
      title: tFallback("gui.toast.source_cleanup_unknown"),
    });
    return;
  }
  const { moved, kept, recoveryRequired, status } = cleanup;
  const trash = platformTrashName();
  if (!status || status === "not_requested") {
    pushTaskResultToast(task, "source-cleanup", {
      kind: "warning",
      title: tFallback("gui.toast.source_cleanup_unknown"),
    });
    return;
  }
  if (recoveryRequired > 0) {
    pushTaskResultToast(task, "source-cleanup", {
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
    pushTaskResultToast(task, "source-cleanup", {
      kind: "warning",
      title: tFallback(
        "gui.toast.source_cleanup_cancelled",
        { moved: moved.toLocaleString(), kept: kept.toLocaleString(), trash },
      ),
    });
    return;
  }
  if (status !== "completed" || kept > 0) {
    pushTaskResultToast(task, "source-cleanup", {
      kind: "warning",
      title: tFallback(
        "gui.toast.source_cleanup_partial",
        { moved: moved.toLocaleString(), failed: kept.toLocaleString(), trash },
      ),
    });
    return;
  }
  pushTaskResultToast(task, "source-cleanup", {
    kind: "success",
    title: tFallback(
      "gui.toast.source_cleanup_done",
      { count: moved.toLocaleString(), trash },
    ),
  });
}

function recordTaskHistory(task: Task): void {
  if (task.historyRecorded) return;
  const outcome = taskResultPresentation(task);
  task.historyRecorded = true;
  recordOperation({
    status: outcome.status,
    title: t(outcome.status === "failed"
      ? "gui.task.history.title_failed"
      : outcome.status === "info"
        ? "gui.task.history.title_attention"
        : "gui.task.history.title_finished", { title: task.title }),
    detail: cleanHistoryDetail(outcome.historyDetail),
  });
}

function cleanHistoryDetail(detail: string): string {
  const oneLine = detail
    .replace(/(?:[A-Za-z]:)?(?:\/[^/\s]+)+\/([^/\s]+)/g, "$1")
    .replace(/\s+/g, " ")
    .trim();
  return oneLine.length <= 140 ? oneLine : `${oneLine.slice(0, 137)}...`;
}

function finishToast(task: Task): void {
  const outcome = taskResultPresentation(task);
  const output = outcome.output;
  task.revealPath = output;
  if (revealAfterExtract && output && (
    task.spec.kind === "extract" || task.spec.kind === "extract_nested" || task.spec.kind === "batch_extract"
  )) void revealPath(output);
  pushTaskResultToast(task, "result", {
    kind: outcome.toastKind,
    title: outcome.summary,
    body: outcome.body,
    action: output
      ? { label: t("gui.task.show_in_file_manager", { fileManager: platformFileManagerName() }), run: () => revealPath(output) }
      : undefined,
  });
}

type DecodedSnapshot = Omit<JobSnapshot, "result"> & { result: JobResult | null };

function decodeSnapshot(snapshot: JobSnapshot): DecodedSnapshot | null {
  const task = find(snapshot.id);
  if (task && !shouldApplyFullSnapshot(task.version, task.state, snapshot.version, snapshot.state)) return null;
  const spec = snapshot.spec ?? task?.spec;
  if (!spec) return null;
  return { ...snapshot, result: parseJobResult(snapshot.result, spec.kind, snapshot.state) };
}

function applySnapshot(snapshot: JobSnapshot): void {
  const decoded = decodeSnapshot(snapshot);
  if (decoded) applyDecodedSnapshot(decoded);
}

function taskFromSnapshot(snapshot: DecodedSnapshot, spec: JobSpec): Task {
  return {
    id: snapshot.id,
    version: snapshot.version,
    spec,
    outputPasswordRequired: snapshot.output_password_required,
    title: titleFor(spec),
    origin: snapshot.origin,
    ownedByRequester: snapshot.owned_by_requester,
    interaction: snapshot.interaction,
    question: snapshotQuestion(snapshot),
    actionFailure: null,
    statusStale: store.snapshotStatus === "unavailable" && !isTerminalSnapshotState(snapshot.state),
    answeredQuestionVersion: 0,
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
    speed: store.snapshotStatus === "unavailable" ? 0 : snapshot.progress.speed,
    phase: snapshot.progress.phase ?? null,
    interruptible: snapshot.progress.interruptible ?? true,
    pausable: jobSupportsPause(spec),
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

function applyDecodedSnapshot(snapshot: DecodedSnapshot): void {
  let task = find(snapshot.id);
  if (!task) {
    if (isTerminalSnapshotState(snapshot.state)) clearPendingQuestions(snapshot.id);
    if (!snapshot.spec) return; // Registration/full recovery supplies the immutable description.
    task = taskFromSnapshot(snapshot, snapshot.spec);
    store.tasks.push(task);
    return;
  }

  task.snapshotSeen = true;
  if (!shouldApplyFullSnapshot(task.version, task.state, snapshot.version, snapshot.state)) return;

  const previousState = task.state;
  task.version = snapshot.version;
  task.spec = snapshot.spec ?? task.spec;
  task.outputPasswordRequired = snapshot.output_password_required;
  task.title = titleFor(task.spec);
  task.origin = snapshot.origin;
  task.ownedByRequester = snapshot.owned_by_requester;
  task.interaction = snapshot.interaction;
  const question = snapshotQuestion(snapshot, task.question);
  if (question !== task.question || snapshot.state !== previousState) task.actionFailure = null;
  task.question = question;
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
  task.speed = task.statusStale ? 0 : snapshot.progress.speed;
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
    ((snapshot.state === "running" || snapshot.state === "queued") && task.controlIntent === "resume")
  ) {
    task.controlIntent = null;
  }
  if (isTerminalSnapshotState(snapshot.state)) {
    task.statusStale = false;
    clearPendingQuestions(snapshot.id);
  }
  runTerminalEffects(task, previousState);
}

function applySnapshotDelta(delta: JobSnapshotsDelta): void {
  if (snapshotRevision !== null && delta.revision < snapshotRevision) return;
  const decoded = delta.upserts.map(decodeSnapshot);
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
    // A recovery request can finish after a newer registration or live update.
    if (!delta.reset || !task.snapshotSeen || task.version > delta.revision) return true;
    return visible.has(task.id) && !locallyDismissed.has(task.id);
  });
  for (const snapshot of decoded) {
    if (snapshot && !locallyDismissed.has(snapshot.id)) applyDecodedSnapshot(snapshot);
  }
  snapshotRevision = delta.revision;
}

function waitForSnapshotPoll(delayMs: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, delayMs));
}

function setSnapshotStatus(status: JobSnapshotStatus): void {
  store.snapshotStatus = status;
  for (const task of store.tasks) {
    // Development previews have no live task feed of their own.
    if (import.meta.env.DEV && !task.snapshotSeen && !task.localEffects) continue;
    task.statusStale = status === "unavailable" && !isTerminalSnapshotState(task.state);
    if (task.statusStale) task.speed = 0;
  }
}

async function reconcileSnapshotFeed(stopped: () => boolean): Promise<void> {
  while (!stopped()) {
    try {
      const generation = snapshotGeneration;
      const delta = await ipc.jobSnapshots(snapshotRevision);
      if (stopped()) return;
      if (generation === snapshotGeneration) {
        applySnapshotDelta(delta);
        setSnapshotStatus("ready");
      }
    } catch {
      if (!stopped()) setSnapshotStatus("unavailable");
      // Native startup and shutdown can briefly race the WebView. The next
      // bounded poll requests the same revision again.
    }
    await waitForSnapshotPoll(runningCount() > 0 ? 400 : 1_000);
  }
}

async function refreshQuestionTask(id: number): Promise<void> {
  try {
    const snapshot = await ipc.jobSnapshot(id);
    if (snapshot && !locallyDismissed.has(id)) applySnapshot(snapshot);
  } catch {
    // The snapshot feed retries automatically if a wake-up races a reload.
  }
}

/** Wires this window's job event listeners once at startup. */
export async function initJobEvents(): Promise<() => void> {
  const cleanup: Array<() => void> = [];
  let stopped = false;
  try {
    const listen = await currentWebviewWindowListener();
    cleanup.push(await listen<JobSnapshot>("job://snapshot", (e) => {
      try { applySnapshot(e.payload); } catch { setSnapshotStatus("unavailable"); }
    }));
  } catch {
    setSnapshotStatus("unavailable");
    for (const dispose of cleanup) dispose();
    cleanup.length = 0;
  }
  // Snapshots remain authoritative when native event listeners cannot start.
  void reconcileSnapshotFeed(() => stopped);
  return () => {
    stopped = true;
    for (const dispose of cleanup) dispose();
  };
}

export function tasks(): Task[] {
  return store.tasks;
}

export function jobSnapshotStatus(): JobSnapshotStatus {
  return store.snapshotStatus;
}

const previewReviewSpecs = new Map<number, JobSpec>();

export function previewTaskSpecForReview(id: number): JobSpec | null {
  return import.meta.env.DEV ? previewReviewSpecs.get(id) ?? null : null;
}

function installTaskPreview(kind: PreviewTaskKind, state: Extract<JobStateName, "done" | "running">): number | null {
  if (!import.meta.env.DEV) return null;
  const seed = createTaskPreview(kind, state);
  if (find(seed.id)) return seed.id;

  const { spec, progress, question } = seed;
  previewReviewSpecs.set(seed.id, redactedSpec(spec));
  store.tasks.push({
    id: seed.id,
    version: 0,
    spec,
    title: titleFor(spec),
    origin: "app",
    ownedByRequester: true,
    interaction: question?.kind ?? null,
    question,
    actionFailure: null,
    statusStale: false,
    answeredQuestionVersion: 0,
    state: seed.state,
    outputPasswordRequired: seed.outputPasswordRequired,
    queuePosition: null,
    queueWaitReason: null,
    cpuThreads: seed.cpuThreads,
    streamBufferLimitBytes: seed.streamBufferLimitBytes,
    ...progress,
    scanEntries: seed.scanEntries,
    phase: seed.phase,
    interruptible: seed.interruptible,
    pausable: jobSupportsPause(spec),
    error: seed.error,
    result: parseJobResult(seed.result, spec.kind, seed.state),
    revealPath: seed.revealPath,
    historyRecorded: true,
    localEffects: false,
    snapshotSeen: false,
    controlIntent: null,
    queueMoveIntent: null,
    expanded: true,
  });
  return seed.id;
}

export function installCompletedTaskPreview(kind: PreviewTaskKind, includeReportHistory = false): number | null {
  const id = installTaskPreview(kind, "done");
  if (!import.meta.env.DEV || !includeReportHistory || id === null) return id;
  if (kind === "duplicate_scan") {
    installTaskPreview("duplicate_scan_clean", "done");
    return id;
  }
  const previous = find(id);
  if (!previous || (kind !== "checksum" && kind !== "checksum_check") || find(id + 2000)) return id;
  const history = createTaskPreviewHistory(kind);
  store.tasks.push({
    ...previous,
    id: id + 2000,
    spec: history.spec,
    title: titleFor(history.spec),
    ...history.progress,
    revealPath: history.revealPath,
    result: parseJobResult(history.result, history.spec.kind, "done"),
  });
  return id;
}

export function installActiveTaskPreview(kind: PreviewTaskKind, statusUnavailable = false): number | null {
  const id = installTaskPreview(kind, "running");
  const task = id === null ? null : find(id);
  if (task && statusUnavailable) {
    task.statusStale = true;
    task.speed = 0;
  }
  return id;
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
    const { spec } = createTaskPreview(kind, "running");
    store.tasks.push({
      id,
      version: 0,
      spec,
      title: titleFor(spec),
      origin: "app",
      ownedByRequester: true,
      interaction: null,
      question: null,
      actionFailure: null,
      statusStale: false,
      answeredQuestionVersion: 0,
      state: "queued",
      queuePosition: index + 1,
      outputPasswordRequired: outputPasswordRequired(spec),
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

const controlRequests = new WeakMap<Task, object>();

async function requestTaskControl(
  id: number,
  intent: TaskControlIntent,
  request: () => Promise<void>,
): Promise<boolean> {
  const task = find(id);
  if (!task || isTerminalSnapshotState(task.state) || task.controlIntent === "cancel"
    || (task.controlIntent !== null && intent !== "cancel")) return false;
  const token = {};
  controlRequests.set(task, token);
  task.controlIntent = intent;
  task.actionFailure = null;
  try {
    await request();
    return controlRequests.get(task) === token;
  } catch {
    const current = find(id);
    if (current !== task || controlRequests.get(task) !== token || task.controlIntent !== intent) return false;
    task.controlIntent = null;
    task.actionFailure = intent;
    return false;
  } finally {
    if (controlRequests.get(task) === token) controlRequests.delete(task);
  }
}

export function pauseTask(id: number): Promise<boolean> {
  return requestTaskControl(id, "pause", () => ipc.pauseJob(id));
}

export function resumeTask(id: number): Promise<boolean> {
  return requestTaskControl(id, "resume", () => ipc.resumeJob(id));
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

export function cancelTask(id: number): Promise<boolean> {
  return requestTaskControl(id, "cancel", () => ipc.cancelJob(id));
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

function pendingQuestion(kind: JobQuestion["kind"]): JobQuestion | null {
  for (const task of store.tasks) {
    const question = task.question;
    if (task.ownedByRequester && !isTerminalSnapshotState(task.state)
      && task.controlIntent !== "cancel" && question?.kind === kind
      && question.prompt.version > task.answeredQuestionVersion) return question;
  }
  return null;
}

export function pendingConflict(): JobConflictPrompt | null {
  const question = pendingQuestion("conflict");
  return question?.kind === "conflict" ? question.prompt : null;
}

export async function answerConflict(decision: string, applyAll: boolean): Promise<boolean> {
  const c = pendingConflict();
  if (!c) return false;
  return answerQuestion(c, () => ipc.answerConflict(c.id, c.version, decision, applyAll));
}

/* ---- Password modal for running jobs ---- */

export function pendingPassword(): JobPasswordPrompt | null {
  const question = pendingQuestion("password");
  return question?.kind === "password" ? question.prompt : null;
}

export async function answerPassword(password: string | null): Promise<boolean> {
  const p = pendingPassword();
  if (!p) return false;
  return answerQuestion(p, () => ipc.answerPassword(p.id, p.version, password));
}

async function answerQuestion(prompt: JobPasswordPrompt | JobConflictPrompt, send: () => Promise<void>): Promise<boolean> {
  const task = find(prompt.id);
  if (!task) return false;
  // Keep the authoritative question until the backend clears or replaces it.
  // Hiding a pending answer must not discard the retry path if IPC fails.
  task.answeredQuestionVersion = prompt.version;
  task.actionFailure = null;
  try {
    await send();
    return true;
  } catch {
    if (task.answeredQuestionVersion === prompt.version) task.answeredQuestionVersion = 0;
    if (task.question?.prompt.version === prompt.version) {
      task.actionFailure = "answer";
    } else {
      pushToast({ kind: "warning", title: t("gui.task.answer_failed"), body: t("gui.task.answer_failed_detail") });
    }
    return false;
  } finally {
    void refreshQuestionTask(prompt.id);
  }
}

/** Localized error text for a failed task row. */
export function taskErrorText(task: Task): string {
  return task.error ? errorSummary(task.error) : "";
}
