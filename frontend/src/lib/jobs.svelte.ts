// Job store: mirrors backend task events and feeds foreground progress,
// status text, and conflict/password prompts.

import {
  ipc,
  type AskConflictEvent,
  type AskPasswordEvent,
  type ErrorDto,
  type JobSpec,
  type ProgressEvent,
  type StateEvent,
} from "./ipc";
import { t, tError } from "./i18n.svelte";
import { jobTitleFor } from "./job-title";
import { pushToast } from "./toasts.svelte";
import { basename, formatBytes } from "./format";
import { recordOperation, type OperationStatus } from "./history.svelte";

export type JobStateName = StateEvent["state"];
export type TaskControlIntent = "cancel" | "pause" | "resume";

export interface Task {
  id: number;
  spec: JobSpec;
  title: string;
  state: JobStateName;
  done: number;
  total: number;
  current: string;
  currentDone: number;
  currentTotal: number;
  speed: number;
  error: ErrorDto | null;
  result: Record<string, unknown> | null;
  /** Path revealed by the "Reveal" button on completion */
  revealPath: string | null;
  /** Guard so replayed/duplicate terminal events do not double-write history. */
  historyRecorded: boolean;
  /** Local optimistic feedback for controls whose backend acknowledgement can lag. */
  controlIntent: TaskControlIntent | null;
  expanded: boolean;
}

const store = $state({
  tasks: [] as Task[],
  conflict: null as AskConflictEvent | null,
  password: null as AskPasswordEvent | null,
});

const pendingStates = new Map<number, StateEvent>();
const pendingProgress = new Map<number, ProgressEvent>();
let revealAfterExtract = $state(false);
const sampleRoot = "/Users/alex/Squallz Samples";
const sampleOutputRoot = "/Users/alex/Squallz Exports";

export function setRevealAfterExtractPreference(enabled: boolean): void {
  revealAfterExtract = enabled;
}

export function revealAfterExtractPreference(): boolean {
  return revealAfterExtract;
}

function revealPath(path: string): void {
  void import("@tauri-apps/plugin-opener")
    .then(({ revealItemInDir }) => revealItemInDir(path))
    .catch(() => undefined);
}

export function titleFor(spec: JobSpec): string {
  return jobTitleFor(spec);
}

function find(id: number): Task | undefined {
  return store.tasks.find((task) => task.id === id);
}

function redactedSpec(spec: JobSpec): JobSpec {
  if (spec.kind === "compress") {
    return { ...spec, password: null, encrypt_names: false };
  }
  if (spec.kind === "convert") {
    return { ...spec, src_password: null, dest_password: null, encrypt_names: false };
  }
  if (spec.kind === "export_sqz") {
    return { ...spec, dest_password: null };
  }
  if (spec.kind === "batch_extract") {
    return { ...spec, items: spec.items.map((item) => ({ ...item, password: null })) };
  }
  if (spec.kind === "extract" || spec.kind === "extract_nested" || spec.kind === "test" || spec.kind === "update") {
    return { ...spec, password: null };
  }
  return spec;
}

/** Submits a job and registers it in the local task list. */
export async function submitJob(spec: JobSpec): Promise<number> {
  const id = await ipc.submitJob(spec);
  if (!find(id)) {
    store.tasks.push({
      id,
      spec: redactedSpec(spec),
      title: titleFor(spec),
      state: "queued",
      done: 0,
      total: 0,
      current: "",
      currentDone: 0,
      currentTotal: 0,
      speed: 0,
      error: null,
      result: null,
      revealPath: null,
      historyRecorded: false,
      controlIntent: null,
      expanded: false,
    });
    replayPending(id);
  }
  return id;
}

function onState(ev: StateEvent): void {
  const task = find(ev.id);
  if (!task) {
    pendingStates.set(ev.id, ev);
    return;
  }
  task.state = ev.state;
  task.error = ev.error ?? null;
  task.result = (ev.result as Record<string, unknown> | null) ?? null;
  if (
    ev.state === "done" ||
    ev.state === "failed" ||
    ev.state === "cancelled" ||
    (ev.state === "paused" && task.controlIntent === "pause") ||
    (ev.state === "running" && task.controlIntent === "resume")
  ) {
    task.controlIntent = null;
  }
  if (ev.state === "done") {
    finishToast(task);
    recordTaskHistory(task);
  }
  if (ev.state === "failed") recordTaskHistory(task);
  if (ev.state === "cancelled") {
    pushToast({ kind: "info", title: t("gui.toast.cancelled") });
    // Cancelled rows leave the list.
    const i = store.tasks.findIndex((x) => x.id === ev.id);
    if (i >= 0) store.tasks.splice(i, 1);
  }
}

function recordTaskHistory(task: Task): void {
  if (task.historyRecorded) return;
  const status = terminalHistoryStatus(task);
  task.historyRecorded = true;
  recordOperation({
    status,
    title: t(status === "failed" ? "gui.task.history.title_failed" : "gui.task.history.title_finished", {
      title: task.title,
    }),
    detail: taskHistoryDetail(task, status),
  });
}

function terminalHistoryStatus(task: Task): Extract<OperationStatus, "done" | "failed"> {
  if (task.state === "failed") return "failed";
  if (task.spec.kind === "test" && task.result?.ok === false) return "failed";
  if (task.spec.kind === "checksum_check" && task.result?.ok === false) return "failed";
  if (task.spec.kind === "batch_extract" && Number(task.result?.failed ?? 0) > 0) return "failed";
  return "done";
}

function cleanHistoryDetail(detail: string): string {
  const oneLine = detail
    .replace(/(?:[A-Za-z]:)?(?:\/[^/\s]+)+\/([^/\s]+)/g, "$1")
    .replace(/\s+/g, " ")
    .trim();
  if (oneLine.length <= 140) return oneLine;
  return `${oneLine.slice(0, 137)}...`;
}

function taskHistoryDetail(task: Task, status: Extract<OperationStatus, "done" | "failed">): string {
  if (status === "failed") {
    if (task.spec.kind === "test" && task.result?.ok === false) {
      const problems = Number(task.result?.problems ?? 0);
      return t("gui.task.history.test_failed_detail", { count: problems });
    }
    return cleanHistoryDetail(task.error ? tError(task.error) : t("gui.task.history.engine_failed"));
  }

  const spec = task.spec;
  switch (spec.kind) {
    case "compress":
      return cleanHistoryDetail(
        spec.split_size
          ? t("gui.task.history.compress_split", { name: basename(spec.dest) })
          : t("gui.task.history.compress", { name: basename(spec.dest), size: formatBytes(task.done) }),
      );
    case "extract": {
      const dest = String(task.result?.dest ?? spec.dest);
      const skipped = Number(task.result?.skipped ?? 0);
      return cleanHistoryDetail(
        skipped > 0
          ? t("gui.task.history.extract_skipped", { dest: basename(dest), count: skipped })
          : t("gui.task.history.extract", { dest: basename(dest) }),
      );
    }
    case "batch_extract": {
      const extracted = Number(task.result?.extracted ?? 0);
      const total = Number(task.result?.archives ?? spec.items.length);
      const failed = Number(task.result?.failed ?? 0);
      return cleanHistoryDetail(t("gui.task.history.batch_extract", { extracted, total, failed }));
    }
    case "extract_nested": {
      const dest = String(task.result?.dest ?? spec.dest);
      const skipped = Number(task.result?.skipped ?? 0);
      return cleanHistoryDetail(
        skipped > 0
          ? t("gui.task.history.extract_nested_skipped", { name: basename(spec.entry_path), dest: basename(dest), count: skipped })
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
      const recovery = String(task.result?.recovery ?? spec.recovery ?? `${spec.path}.par2`);
      return cleanHistoryDetail(t("gui.task.history.recovery_data", { name: basename(recovery) }));
    }
    case "verify_recovery":
      return cleanHistoryDetail(t("gui.task.history.recovery_verified", { name: basename(spec.path) }));
    case "repair_recovery": {
      const output = String(task.result?.archive ?? spec.output ?? spec.path);
      return cleanHistoryDetail(t("gui.task.history.repaired", { name: basename(output) }));
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
    const toast = {
      kind: failed > 0 ? "warning" : "success",
      title: t("gui.toast.batch_extract_done", { extracted, failed }),
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
    const skipped = Number(task.result?.skipped ?? 0);
    task.revealPath = dest;
    if (revealAfterExtract) {
      revealPath(dest);
    }
    pushToast({
      kind: bestEffort && skipped > 0 ? "warning" : "success",
      title: bestEffort
        ? t("gui.toast.best_effort_extract_done", { count: skipped })
        : t("gui.toast.extract_done", { path: dest }),
      action: { label: t("gui.toast.reveal"), run: () => revealPath(dest) },
    });
  } else if (spec.kind === "compress") {
    task.revealPath = spec.dest;
    pushToast({
      kind: "success",
      title: spec.split_size
        ? t("gui.toast.compress_done_split", {
            name: basename(spec.dest),
            count: "?",
          })
        : t("gui.toast.compress_done", {
            name: basename(spec.dest),
            size: formatBytes(task.done),
          }),
      action: {
        label: t("gui.toast.reveal"),
        run: () => revealPath(spec.dest),
      },
    });
  } else if (spec.kind === "test") {
    const ok = task.result?.ok !== false;
    const entries = Number(task.result?.entries ?? 0);
    const problems = Number(task.result?.problems ?? 0);
    pushToast(
      ok
        ? { kind: "success", title: t("gui.toast.test_ok", { count: entries }) }
        : { kind: "warning", title: t("gui.toast.test_failed", { count: problems }) },
    );
  } else if (spec.kind === "convert") {
    task.revealPath = spec.dest;
    pushToast({
      kind: "success",
      title: t("gui.toast.convert_done", { name: basename(spec.dest) }),
      action: {
        label: t("gui.toast.reveal"),
        run: () => revealPath(spec.dest),
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
    const recovery = String(task.result?.recovery ?? spec.recovery ?? `${spec.path}.par2`);
    task.revealPath = recovery;
    pushToast({
      kind: "success",
      title: t("gui.toast.recovery_protect_done", { name: basename(recovery) }),
      action: {
        label: t("gui.toast.reveal"),
        run: () => revealPath(recovery),
      },
    });
  } else if (spec.kind === "verify_recovery") {
    pushToast({
      kind: "success",
      title: t("gui.toast.recovery_verify_ok", { name: basename(spec.path) }),
    });
  } else if (spec.kind === "repair_recovery") {
    const output = String(task.result?.archive ?? spec.output ?? spec.path);
    task.revealPath = output;
    pushToast({
      kind: "success",
      title: t("gui.toast.recovery_repair_done", { name: basename(output) }),
      action: {
        label: t("gui.toast.reveal"),
        run: () => revealPath(output),
      },
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
    pendingProgress.set(ev.id, ev);
    return;
  }
  task.done = ev.done;
  task.total = ev.total;
  task.current = ev.current;
  task.currentDone = ev.current_done ?? 0;
  task.currentTotal = ev.current_total ?? 0;
  task.speed = ev.speed;
}

function replayPending(id: number): void {
  const progress = pendingProgress.get(id);
  if (progress) {
    pendingProgress.delete(id);
    onProgress(progress);
  }
  const state = pendingStates.get(id);
  if (state) {
    pendingStates.delete(id);
    onState(state);
  }
}

/** Wires the global event listeners once at startup. */
export async function initJobEvents(): Promise<() => void> {
  const { listen } = await import("@tauri-apps/api/event");
  const cleanup: Array<() => void> = [];
  cleanup.push(await listen<ProgressEvent>("job://progress", (e) => onProgress(e.payload)));
  cleanup.push(await listen<StateEvent>("job://state", (e) => onState(e.payload)));
  cleanup.push(await listen<AskConflictEvent>("job://ask-conflict", (e) => {
    store.conflict = e.payload;
  }));
  cleanup.push(await listen<AskPasswordEvent>("job://ask-password", (e) => {
    store.password = e.payload;
  }));
  return () => {
    for (const dispose of cleanup) dispose();
  };
}

export function tasks(): Task[] {
  return store.tasks;
}

type PreviewTaskKind = "compress" | "extract" | "extract_unknown_current" | "batch_extract" | "test" | "checksum" | "checksum_check";

function previewTaskSpec(kind: PreviewTaskKind): JobSpec {
  if (kind === "compress") {
    return {
      kind: "compress",
      inputs: [`${sampleRoot}/reports`, `${sampleRoot}/photos`],
      dest: `${sampleOutputRoot}/product-backup.zip`,
      level: 5,
      password: null,
      encrypt_names: false,
      split_size: null,
      excludes: [],
    };
  }
  if (kind === "extract" || kind === "extract_unknown_current") {
    return {
      kind: "extract",
      path: `${sampleRoot}/product-backup.zip`,
      dest: `${sampleOutputRoot}/product-backup`,
      selection: null,
      overwrite: "ask",
      symlinks: "preserve",
      smart: true,
      encoding: null,
      password: null,
      best_effort: false,
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
    return { operation: "extract", dest: "/tmp/squallz-output/product-backup", skipped: 0 };
  }
  if (kind === "test") {
    return { operation: "test", ok: true, entries_tested: 42, problems: [] };
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
  return { operation: "compress", dest: `${sampleOutputRoot}/product-backup.zip` };
}

function previewRevealPath(kind: PreviewTaskKind): string | null {
  if (kind === "compress") return `${sampleOutputRoot}/product-backup.zip`;
  if (kind === "extract" || kind === "extract_unknown_current") return `${sampleOutputRoot}/product-backup`;
  if (kind === "test") return null;
  if (kind === "checksum") return `${sampleRoot}/photos`;
  if (kind === "checksum_check") return `${sampleRoot}/photos/SHA256SUMS`;
  return `${sampleOutputRoot}/client-data`;
}

function previewProgress(kind: PreviewTaskKind, state: Extract<JobStateName, "done" | "running">) {
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
  if (kind === "compress") {
    return {
      done: state === "done" ? 24_000_000 : 9_600_000,
      total: 24_000_000,
      current: "reports/Launch plan.pdf",
      currentDone: state === "done" ? 3_800_000 : 1_420_000,
      currentTotal: 3_800_000,
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
  if (kind === "compress") return 1;
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

  store.tasks.push({
    id,
    spec,
    title: titleFor(spec),
    state,
    done: progress.done,
    total: progress.total,
    current: progress.current,
    currentDone: progress.currentDone,
    currentTotal: progress.currentTotal,
    speed: progress.speed,
    error: null,
    result: state === "done" ? previewTaskResult(kind) : null,
    revealPath: state === "done" ? previewRevealPath(kind) : null,
    historyRecorded: true,
    controlIntent: null,
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

export function pauseTask(id: number): void {
  const task = find(id);
  if (task) task.controlIntent = "pause";
  void ipc.pauseJob(id).catch(() => undefined);
}

export function resumeTask(id: number): void {
  const task = find(id);
  if (task) task.controlIntent = "resume";
  void ipc.resumeJob(id).catch(() => undefined);
}

export function setTaskExpanded(id: number, expanded: boolean): void {
  const task = find(id);
  if (task) task.expanded = expanded;
}

export function cancelTask(id: number): void {
  // Cancelling also dismisses an open question modal of this job.
  if (store.conflict?.id === id) store.conflict = null;
  if (store.password?.id === id) store.password = null;
  const task = find(id);
  if (task) task.controlIntent = "cancel";
  void ipc.cancelJob(id).catch(() => undefined);
}

export function retryTask(task: Task): void {
  const i = store.tasks.findIndex((x) => x.id === task.id);
  if (i >= 0) store.tasks.splice(i, 1);
  void submitJob(task.spec);
}

/** Removes finished rows (Clear All). */
export function clearFinished(): void {
  store.tasks = store.tasks.filter(
    (x) => x.state === "queued" || x.state === "running" || x.state === "paused",
  );
}

/* ---- Conflict modal ---- */

export function pendingConflict(): AskConflictEvent | null {
  return store.conflict;
}

export function answerConflict(decision: string, applyAll: boolean): void {
  const c = store.conflict;
  if (!c) return;
  store.conflict = null;
  void ipc.answerConflict(c.id, decision, applyAll);
}

/* ---- Password modal for running jobs ---- */

export function pendingPassword(): AskPasswordEvent | null {
  return store.password;
}

export function answerPassword(password: string | null): void {
  const p = store.password;
  if (!p) return;
  store.password = null;
  void ipc.answerPassword(p.id, password);
}

/** Localized error text for a failed task row. */
export function taskErrorText(task: Task): string {
  return task.error ? tError(task.error) : "";
}
