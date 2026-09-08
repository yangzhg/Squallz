// Toast store: max 3 on screen, overflow queues. Actionable toasts and danger
// stay until dismissed; other info/success toasts last 4 s and warnings 6 s.

export type ToastKind = "info" | "success" | "warning" | "danger";

export interface Toast {
  id: number;
  /** Stable identity for a condition that should be replaced or cleared in place. */
  key?: string;
  kind: ToastKind;
  title: string;
  body?: string;
  /** Optional action button (e.g. reveal in Finder) */
  action?: {
    label: string;
    /** Return false when the action could not complete and the toast should stay visible. */
    run: () => boolean | void | Promise<boolean | void>;
  };
  /** Keep visible until the user dismisses it or its keyed condition clears. */
  persistent?: boolean;
  /** Log-only detail for the details view */
  detail?: string;
}

const store = $state({ visible: [] as Toast[], queue: [] as Toast[] });
const timers = new Map<number, ReturnType<typeof setTimeout>>();
let nextId = 1;

const DURATION: Record<ToastKind, number> = {
  info: 4000,
  success: 4000,
  warning: 6000,
  danger: 0,
};

export function toasts(): Toast[] {
  return store.visible;
}

export function pushToast(toast: Omit<Toast, "id">): void {
  if (toast.key) removeToastByKey(toast.key);
  const full: Toast = { ...toast, id: nextId++ };
  if (store.visible.length >= 3) {
    const candidate = preemptionCandidate(full.kind);
    if (candidate >= 0) {
      const [displaced] = store.visible.splice(candidate, 1);
      clearToastTimer(displaced.id);
      enqueue(displaced);
      show(full);
    } else {
      enqueue(full);
    }
  } else {
    show(full);
  }
}

function priority(kind: ToastKind): number {
  if (kind === "danger") return 2;
  if (kind === "warning") return 1;
  return 0;
}

function preemptionCandidate(incoming: ToastKind): number {
  const incomingPriority = priority(incoming);
  let candidate = -1;
  let candidatePriority = incomingPriority;
  for (let i = 0; i < store.visible.length; i += 1) {
    const visible = store.visible[i];
    const visiblePriority = priority(visible.kind);
    const replacesActionableAtSamePriority =
      candidate >= 0 &&
      visiblePriority === candidatePriority &&
      store.visible[candidate].action !== undefined &&
      visible.action === undefined;
    if (visiblePriority < candidatePriority || replacesActionableAtSamePriority) {
      candidate = i;
      candidatePriority = visiblePriority;
    }
  }
  return candidate;
}

function enqueue(toast: Toast): void {
  const toastPriority = priority(toast.kind);
  const nextLower = store.queue.findIndex((queued) => priority(queued.kind) < toastPriority);
  if (nextLower < 0) store.queue.push(toast);
  else store.queue.splice(nextLower, 0, toast);
}

function show(toast: Toast): void {
  store.visible.push(toast);
  const ms = toast.action || toast.persistent ? 0 : DURATION[toast.kind];
  if (ms > 0) {
    timers.set(toast.id, setTimeout(() => dismissToast(toast.id), ms));
  }
}

function clearToastTimer(id: number): void {
  const timer = timers.get(id);
  if (timer === undefined) return;
  clearTimeout(timer);
  timers.delete(id);
}

export function dismissToast(id: number): void {
  const i = store.visible.findIndex((t) => t.id === id);
  if (i < 0) return;
  clearToastTimer(id);
  store.visible.splice(i, 1);
  const next = store.queue.shift();
  if (next) show(next);
}

/** Removes a completed action even if a higher-priority toast displaced it into the queue. */
export function removeToast(id: number): void {
  const visibleIndex = store.visible.findIndex((toast) => toast.id === id);
  if (visibleIndex >= 0) {
    dismissToast(id);
    return;
  }
  const queueIndex = store.queue.findIndex((toast) => toast.id === id);
  if (queueIndex >= 0) store.queue.splice(queueIndex, 1);
}

export function removeToastByKey(key: string): void {
  for (let i = store.queue.length - 1; i >= 0; i -= 1) {
    if (store.queue[i].key === key) store.queue.splice(i, 1);
  }
  const visible = store.visible.find((toast) => toast.key === key);
  if (visible) dismissToast(visible.id);
}
