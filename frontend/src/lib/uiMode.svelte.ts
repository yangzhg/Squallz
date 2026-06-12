// UI mode handling: the layout mode is a persisted user preference, separate
// from light/dark theme. `null` means first-run selection has not completed.

import { ipc } from "./ipc";

export type UiMode = "modern" | "classic";

const store = $state({ choice: null as UiMode | null });

function normalize(value: string | null): UiMode | null {
  return value === "modern" || value === "classic" ? value : null;
}

function apply(): void {
  document.documentElement.dataset.uiMode = store.choice ?? "unset";
}

/** Initializes from a persisted settings value. */
export function initUiMode(persisted: string | null): void {
  store.choice = normalize(persisted);
  apply();
}

/** Raw persisted choice; `null` means the first-run picker is still required. */
export function uiModeChoice(): UiMode | null {
  return store.choice;
}

/** Effective mode while first-run selection is open. */
export function activeUiMode(): UiMode {
  return store.choice ?? "modern";
}

/** Sets and persists the UI mode choice. */
export async function setUiMode(choice: UiMode): Promise<void> {
  store.choice = choice;
  apply();
  await ipc.setUiMode(choice);
}
