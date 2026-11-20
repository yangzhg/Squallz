import { tick } from "svelte";
import type { BrowserEntry } from "./archive-browser";
import type { UiMode } from "./uiMode.svelte";

export function archiveTabStopPath(rows: readonly (BrowserEntry | null)[], focusedPath: string | null): string | null {
  const entries = rows.filter((entry) => entry?.source);
  return entries.find((entry) => entry?.source?.path === focusedPath)?.source?.path
    ?? entries.find((entry) => entry?.selected)?.source?.path
    ?? entries[0]?.source?.path
    ?? null;
}

export function archiveKeyboardTarget(key: string, index: number, total: number, pageRows: number): number | null {
  if (!Number.isInteger(index) || index < 0 || index >= total) return null;
  const page = Math.max(1, Math.floor(pageRows));
  let target: number;
  switch (key) {
    case "ArrowUp": target = index - 1; break;
    case "ArrowDown": target = index + 1; break;
    case "Home": target = 0; break;
    case "End": target = total - 1; break;
    case "PageUp": target = index - page; break;
    case "PageDown": target = index + page; break;
    default: return null;
  }
  return Math.max(0, Math.min(total - 1, target));
}

export function handleArchiveNavigationKey(event: KeyboardEvent, move: () => void): boolean {
  if (event.key === "ArrowLeft" || event.key === "ArrowRight") {
    const row = event.currentTarget as HTMLElement;
    const label = row.querySelector<HTMLElement>(".archive-entry-label");
    if (label && label.scrollWidth > label.clientWidth) {
      const offset = event.key === "ArrowLeft" ? -label.clientWidth : label.clientWidth;
      const next = Math.max(0, Math.min(label.scrollWidth - label.clientWidth, label.scrollLeft + offset));
      if (next !== label.scrollLeft) {
        event.preventDefault();
        label.scrollLeft = next;
      }
    }
    return true;
  }
  if (!["ArrowUp", "ArrowDown", "Home", "End", "PageUp", "PageDown"].includes(event.key)) return false;
  event.preventDefault();
  move();
  return true;
}

interface ArchiveRowFocus {
  mode: UiMode;
  rowHeight: number;
  canFocus: () => boolean;
  loadRowAt: (index: number) => Promise<{ path: string } | null>;
  setViewport: (scrollTop: number, height: number) => void;
}

export async function focusVirtualArchiveRow(index: number, expectedPath: string | null, view: ArchiveRowFocus): Promise<void> {
  await tick();
  if (!view.canFocus()) return;
  const list = document.querySelector<HTMLElement>(`[data-virtual-list="${view.mode}"]`);
  if (!list) return;
  const top = index * view.rowHeight;
  const bottom = top + view.rowHeight;
  const scrollTop = Math.max(0, Math.min(top, Math.max(list.scrollTop, bottom - list.clientHeight)));
  if (scrollTop !== list.scrollTop) {
    list.scrollTop = scrollTop;
    view.setViewport(list.scrollTop, list.clientHeight);
  }
  const row = await view.loadRowAt(index);
  if (!row || (expectedPath !== null && row.path !== expectedPath)) return;
  await tick();
  if (view.canFocus()) list.querySelector<HTMLElement>(`[data-row-index="${index}"]`)?.focus({ preventScroll: true });
}
