import { tick } from "svelte";

export function isElementAvailableForFocus(element: HTMLElement): boolean {
  if (element.isConnected === false || element.matches(":disabled")) return false;
  const view = element.ownerDocument.defaultView;
  for (let current: HTMLElement | null = element; current; current = current.parentElement) {
    if (current.hidden || current.inert || current.hasAttribute("hidden")
      || current.hasAttribute("inert") || current.getAttribute("aria-hidden") === "true") return false;
    const style = view?.getComputedStyle(current);
    if (style?.display === "none" || style?.visibility === "hidden"
      || style?.visibility === "collapse") return false;
  }
  return typeof element.getClientRects !== "function" || element.getClientRects().length > 0;
}

export function trapModalFocus(event: KeyboardEvent, root: HTMLElement | null): void {
  if (event.key !== "Tab" || !root) return;
  const elements = Array.from(root.querySelectorAll<HTMLElement>(
    'button:not(:disabled), input:not(:disabled), select:not(:disabled), textarea:not(:disabled), [href], [tabindex]:not([tabindex="-1"])',
  )).filter((element) => element.tabIndex >= 0 && isElementAvailableForFocus(element));
  if (elements.length === 0) {
    event.preventDefault();
    root.focus();
    return;
  }
  const first = elements[0];
  const last = elements[elements.length - 1];
  const active = root.ownerDocument.activeElement;
  if (active === root || !elements.includes(active as HTMLElement)) {
    event.preventDefault();
    (event.shiftKey ? last : first).focus();
  } else if (event.shiftKey && active === first) {
    event.preventDefault();
    last.focus();
  } else if (!event.shiftKey && active === last) {
    event.preventDefault();
    first.focus();
  }
}

export function restoreModalFocus(root: HTMLElement | null, target: HTMLElement | null): void {
  if (!root || !target) return;
  void tick().then(() => {
    const document = root.ownerDocument;
    const active = document.activeElement;
    if (active && active !== document.body && !root.contains(active)) return;
    if (!isElementAvailableForFocus(target)) return;
    const modals = typeof document.querySelectorAll === "function"
      ? document.querySelectorAll<HTMLElement>('[aria-modal="true"]')
      : [];
    if (Array.from(modals).some((modal) => modal !== root && !root.contains(modal)
      && !modal.contains(target) && isElementAvailableForFocus(modal))) return;
    target.focus({ preventScroll: true });
  });
}
