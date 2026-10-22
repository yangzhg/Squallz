import { isTauri } from "@tauri-apps/api/core";

type Size = { width: number; height: number };
type Position = { x: number; y: number };
export type TaskWindowGeometry = {
  inner: Size;
  outer: Size;
  position: Position;
  scale: number;
  viewportHeight: number;
  workArea: { position: Position; size: Size };
  maximized: boolean;
  fullscreen: boolean;
};
type FitRequest = { height: number; margin: number; maxHeight: number };
type FitTarget = { size: Size; position: Position | null };

export function taskWindowFitTarget(geometry: TaskWindowGeometry, request: FitRequest): FitTarget | null {
  if (geometry.maximized || geometry.fullscreen || !(geometry.scale > 0)
    || !(geometry.viewportHeight > 0) || !Number.isFinite(request.height)
    || !(request.maxHeight > 0)) return null;
  const margin = Math.max(0, request.margin) * geometry.scale;
  const decoration = Math.max(0, geometry.outer.height - geometry.inner.height);
  const viewportInset = Math.max(0, geometry.inner.height - geometry.viewportHeight * geometry.scale);
  const availableHeight = geometry.workArea.size.height - margin * 2 - decoration;
  const height = Math.min(Math.ceil(request.height * geometry.scale + viewportInset),
    Math.floor(Math.min(request.maxHeight * geometry.scale, availableHeight)));
  if (height <= geometry.inner.height) return null;
  const top = geometry.workArea.position.y + margin;
  const bottom = geometry.workArea.position.y + geometry.workArea.size.height - margin;
  const y = Math.round(Math.max(top, Math.min(geometry.position.y, bottom - height - decoration)));
  return {
    size: { width: geometry.inner.width, height },
    position: y === geometry.position.y ? null : { x: geometry.position.x, y },
  };
}

type WindowAdapter = {
  read: () => Promise<TaskWindowGeometry | null>;
  size: () => Promise<Size>;
  setSize: (size: Size) => Promise<void>;
  setPosition: (position: Position) => Promise<void>;
};

/** Serializes growth and stops automatically fitting after the user resizes. */
export function createTaskWindowFitter(adapter: WindowAdapter, initialSize: Size | null = null) {
  let stopped = false;
  let running = false;
  let pending: FitRequest | null = null;
  let expected: Size | null = null;
  let known: Size | null = initialSize;
  const sameSize = (left: Size, right: Size) => left.width === right.width && left.height === right.height;

  return {
    async fit(request: FitRequest): Promise<void> {
      if (stopped) return;
      pending = request;
      if (running) return;
      running = true;
      try {
        while (pending && !stopped) {
          const next = pending;
          pending = null;
          const geometry = await adapter.read();
          if (stopped || !geometry) continue;
          if (known && !sameSize(known, geometry.inner)) { stopped = true; break; }
          known = geometry.inner;
          const target = taskWindowFitTarget(geometry, pending ?? next);
          pending = null;
          if (!target) continue;
          const currentSize = await adapter.size();
          if (stopped) break;
          if (!sameSize(currentSize, geometry.inner)) { stopped = true; break; }
          expected = target.size;
          await adapter.setSize(target.size);
          if (stopped) break;
          known = target.size;
          if (target.position) await adapter.setPosition(target.position);
        }
      } catch {
        // Scrollable task content remains usable if the window manager cannot resize it.
        stopped = true;
      } finally {
        running = false;
      }
    },
    resized(size: Size): void {
      if (expected && sameSize(size, expected)) {
        known = size;
        expected = null;
      } else if (!known) {
        known = size;
      } else if (!sameSize(size, known)) {
        stopped = true;
        pending = null;
      }
    },
    destroy(): void {
      stopped = true;
      pending = null;
    },
  };
}

function pixels(style: CSSStyleDeclaration, name: string): number {
  return Number.parseFloat(style.getPropertyValue(name)) || 0;
}

export function taskWindowContentHeight(card: HTMLElement): number {
  const view = card.ownerDocument.defaultView;
  if (!view) return 0;
  const style = view.getComputedStyle(card);
  const children = Array.from(card.children).filter((child): child is HTMLElement =>
    "offsetHeight" in child && child.getClientRects().length > 0);
  const content = children.reduce((total, child) => {
    const childStyle = view.getComputedStyle(child);
    let height = child.classList.contains("task-modal-body")
      ? child.scrollHeight + pixels(childStyle, "border-top-width") + pixels(childStyle, "border-bottom-width")
      : child.getBoundingClientRect().height;
    for (const notice of child.querySelectorAll<HTMLElement>(":scope > .surface-notice")) {
      const noticeStyle = view.getComputedStyle(notice);
      height -= notice.getBoundingClientRect().height + pixels(noticeStyle, "margin-top")
        + pixels(noticeStyle, "margin-bottom") + pixels(childStyle, "row-gap");
    }
    height = Math.max(0, height);
    return total + height + pixels(childStyle, "margin-top") + pixels(childStyle, "margin-bottom");
  }, 0);
  return Math.ceil(content + Math.max(0, children.length - 1) * pixels(style, "row-gap")
    + pixels(style, "padding-top") + pixels(style, "padding-bottom")
    + pixels(style, "border-top-width") + pixels(style, "border-bottom-width"));
}

/** Measures the existing task card; its nested result lists keep their own scroll limits. */
export function fitTaskWindow(card: HTMLElement, enabled: boolean) {
  let stop: (() => void) | null = null;
  let generation = 0;

  async function connect(active: boolean): Promise<void> {
    const current = ++generation;
    stop?.();
    stop = null;
    if (!active || !isTauri()) return;
    const view = card.ownerDocument.defaultView;
    if (!view) return;
    try {
      const { getCurrentWindow, currentMonitor, PhysicalSize, PhysicalPosition } = await import("@tauri-apps/api/window");
      if (current !== generation) return;
      const window = getCurrentWindow();
      const initialSize = await window.innerSize();
      if (current !== generation) return;
      const fitter = createTaskWindowFitter({
        async read() {
          const [inner, outer, position, scale, monitor, maximized, fullscreen] = await Promise.all([
            window.innerSize(), window.outerSize(), window.outerPosition(), window.scaleFactor(),
            currentMonitor(), window.isMaximized(), window.isFullscreen(),
          ]);
          return monitor ? { inner, outer, position, scale, viewportHeight: view.innerHeight,
            workArea: monitor.workArea, maximized, fullscreen } : null;
        },
        size: () => window.innerSize(),
        setSize: (size) => window.setSize(new PhysicalSize(size.width, size.height)),
        setPosition: (position) => window.setPosition(new PhysicalPosition(position.x, position.y)),
      }, initialSize);
      let frame: number | null = null;
      let lastHeight = 0;
      const measure = () => {
        frame = null;
        if (current !== generation || !card.isConnected || card.closest("[inert]")) return;
        const height = taskWindowContentHeight(card);
        if (height === lastHeight) return;
        lastHeight = height;
        const style = view.getComputedStyle(card);
        void fitter.fit({ height, margin: pixels(style, "--task-window-fit-margin"),
          maxHeight: pixels(style, "--task-window-fit-max-height") });
      };
      const schedule = () => {
        if (frame === null) frame = view.requestAnimationFrame(measure);
      };
      const observer = new ResizeObserver(schedule);
      const observeContent = () => {
        observer.disconnect();
        observer.observe(card);
        for (const child of card.children) observer.observe(child);
        for (const child of card.querySelector(".task-modal-body")?.children ?? []) observer.observe(child);
      };
      const mutations = new MutationObserver(() => { observeContent(); schedule(); });
      const unlisten = await window.onResized(({ payload }) => fitter.resized(payload));
      const cleanup = () => {
        fitter.destroy();
        unlisten();
        observer.disconnect();
        mutations.disconnect();
        if (frame !== null) view.cancelAnimationFrame(frame);
      };
      if (current !== generation) { cleanup(); return; }
      stop = cleanup;
      observeContent();
      mutations.observe(card, { childList: true, characterData: true, subtree: true });
      schedule();
    } catch {
      // This action also runs in previews without desktop window capabilities.
    }
  }

  void connect(enabled);
  return {
    update(next: boolean) {
      if (next === enabled) return;
      enabled = next;
      void connect(next);
    },
    destroy() {
      generation += 1;
      stop?.();
    },
  };
}
