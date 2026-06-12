// Toast store: max 3 on screen, overflow queues; info /
// success auto-dismiss in 4 s, warning 6 s, danger stays.

export type ToastKind = "info" | "success" | "warning" | "danger";

export interface Toast {
  id: number;
  kind: ToastKind;
  title: string;
  body?: string;
  /** Optional action button (e.g. reveal in Finder) */
  action?: { label: string; run: () => void };
  /** Log-only detail for the details view */
  detail?: string;
}

const store = $state({ visible: [] as Toast[], queue: [] as Toast[] });
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
  const full: Toast = { ...toast, id: nextId++ };
  if (store.visible.length >= 3) {
    store.queue.push(full);
  } else {
    show(full);
  }
}

function show(toast: Toast): void {
  store.visible.push(toast);
  const ms = DURATION[toast.kind];
  if (ms > 0) {
    setTimeout(() => dismissToast(toast.id), ms);
  }
}

export function dismissToast(id: number): void {
  const i = store.visible.findIndex((t) => t.id === id);
  if (i >= 0) store.visible.splice(i, 1);
  const next = store.queue.shift();
  if (next) show(next);
}
