import { tick } from "svelte";
import { cssVariables } from "./css-variables";
import { isElementAvailableForFocus } from "./modal-focus";

type Point = { x: number; y: number };
type AnchorBounds = { left: number; right: number; top: number; bottom: number };

export type PopupMenuOptions = {
  anchor?: HTMLElement | null;
  point?: Point;
  onClose: (restoreFocus: boolean) => void;
};

export function popupMenuPosition(
  size: { width: number; height: number },
  viewport: { width: number; height: number },
  padding: number,
  gap: number,
  anchor?: AnchorBounds,
  point?: Point,
): Point {
  const x = point?.x ?? (anchor ? anchor.right - size.width : padding);
  let y = point?.y ?? (anchor ? anchor.bottom + gap : padding);
  if (!point && anchor && y + size.height > viewport.height - padding
    && anchor.top - gap > viewport.height - anchor.bottom - gap) {
    y = anchor.top - gap - size.height;
  }
  return {
    x: Math.max(padding, Math.min(x, viewport.width - size.width - padding)),
    y: Math.max(padding, Math.min(y, viewport.height - size.height - padding)),
  };
}

/** Positions an anchored menu and keeps its keyboard interaction in that menu. */
export function popupMenu(node: HTMLElement, initial: PopupMenuOptions) {
  let options = initial;
  let destroyed = false;
  let closing = false;
  const document = node.ownerDocument;
  const view = document.defaultView;
  const variables = cssVariables(node, {});
  const originalTabIndexes = new Map<HTMLElement, string | null>();
  const originalRootTabIndex = node.getAttribute("tabindex");
  node.tabIndex = -1;

  function menuItems(): HTMLElement[] {
    return Array.from(node.querySelectorAll<HTMLElement>('[role="menuitem"]'))
      .filter((item) => isElementAvailableForFocus(item) && item.getAttribute("aria-disabled") !== "true");
  }

  function prepareItems(): void {
    for (const item of node.querySelectorAll<HTMLElement>('[role="menuitem"]')) {
      if (!originalTabIndexes.has(item)) originalTabIndexes.set(item, item.getAttribute("tabindex"));
      item.tabIndex = -1;
    }
  }

  function position(): void {
    if (destroyed || !view || !node.isConnected) return;
    const style = view.getComputedStyle(node);
    const padding = Number.parseFloat(style.getPropertyValue("--popup-viewport-padding")) || 0;
    const gap = Number.parseFloat(style.getPropertyValue("--popup-gap")) || 0;
    const { x, y } = popupMenuPosition(node.getBoundingClientRect(), {
      width: document.documentElement.clientWidth || view.innerWidth,
      height: document.documentElement.clientHeight || view.innerHeight,
    }, padding, gap, options.anchor?.getBoundingClientRect(), options.point);
    variables.update({ "--popup-left": `${x}px`, "--popup-top": `${y}px` });
  }

  function focusFirst(): void {
    prepareItems();
    (menuItems()[0] ?? node).focus({ preventScroll: true });
  }

  function tabDestination(backward: boolean): HTMLElement | undefined {
    const anchor = options.anchor;
    const elements = Array.from(document.querySelectorAll<HTMLElement>(
      'button, input, select, textarea, [href], [tabindex], summary, [contenteditable="true"]',
    )).filter((element) => element.tabIndex >= 0 && !node.contains(element)
      && isElementAvailableForFocus(element));
    // A context-menu opener may be a programmatically focused row outside the tab order.
    if (anchor && !elements.includes(anchor) && !node.contains(anchor)
      && isElementAvailableForFocus(anchor)) elements.push(anchor);
    elements.sort((left, right) => {
      if (left === right) return 0;
      const leftOrder = left.tabIndex > 0 ? left.tabIndex : Number.POSITIVE_INFINITY;
      const rightOrder = right.tabIndex > 0 ? right.tabIndex : Number.POSITIVE_INFINITY;
      if (leftOrder !== rightOrder) return leftOrder - rightOrder;
      return left.compareDocumentPosition(right) & 4 ? -1 : 1;
    });
    const origin = anchor ? elements.indexOf(anchor) : -1;
    if (origin < 0) return backward ? elements.at(-1) : elements[0];
    return elements[(origin + (backward ? -1 : 1) + elements.length) % elements.length];
  }

  function close(restoreFocus: boolean): void {
    if (closing || destroyed) return;
    closing = true;
    options.onClose(restoreFocus);
  }

  function onFocusIn(event: FocusEvent): void {
    const target = event.target as HTMLElement | null;
    if (!target || node.contains(target) || target === options.anchor) return;
    if (typeof target.focus === "function" && isElementAvailableForFocus(target)) close(false);
  }

  function onKeydown(event: KeyboardEvent): void {
    if (closing || event.defaultPrevented || event.altKey || event.ctrlKey || event.metaKey || event.isComposing) return;
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      close(true);
      return;
    }
    if (event.key === "Tab") {
      event.preventDefault();
      event.stopPropagation();
      const destination = tabDestination(event.shiftKey);
      close(false);
      destination?.focus({ preventScroll: true });
      return;
    }
    if (!["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)) return;
    event.preventDefault();
    event.stopPropagation();
    const items = menuItems();
    if (!items.length) return;
    const current = items.indexOf(document.activeElement as HTMLElement);
    let next: number;
    switch (event.key) {
      case "ArrowDown": next = (current + 1) % items.length; break;
      case "ArrowUp": next = current < 0 ? items.length - 1 : (current + items.length - 1) % items.length; break;
      case "Home": next = 0; break;
      case "End": next = items.length - 1; break;
      default: return;
    }
    items[next].focus({ preventScroll: true });
  }

  function onScroll(event: Event): void {
    if (!(event.target instanceof Node) || !node.contains(event.target)) position();
  }

  node.addEventListener("keydown", onKeydown);
  view?.addEventListener("resize", position);
  document.addEventListener("scroll", onScroll, true);
  document.addEventListener("focusin", onFocusIn);
  const observer = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(position);
  observer?.observe(node);
  if (options.anchor) observer?.observe(options.anchor);
  const openingFocus = document.activeElement;
  position();
  void tick().then(() => {
    if (destroyed || closing) return;
    position();
    if (document.activeElement === openingFocus || document.activeElement === document.body
      || node.contains(document.activeElement)) focusFirst();
  });

  return {
    update(next: PopupMenuOptions) {
      if (options.anchor !== next.anchor) {
        if (options.anchor) observer?.unobserve(options.anchor);
        if (next.anchor) observer?.observe(next.anchor);
      }
      options = next;
      prepareItems();
      position();
    },
    destroy() {
      destroyed = true;
      observer?.disconnect();
      node.removeEventListener("keydown", onKeydown);
      view?.removeEventListener("resize", position);
      document.removeEventListener("scroll", onScroll, true);
      document.removeEventListener("focusin", onFocusIn);
      variables.destroy();
      if (originalRootTabIndex === null) node.removeAttribute("tabindex");
      else node.setAttribute("tabindex", originalRootTabIndex);
      for (const [item, value] of originalTabIndexes) {
        if (value === null) item.removeAttribute("tabindex");
        else item.setAttribute("tabindex", value);
      }
    },
  };
}
