import { isTauri } from "@tauri-apps/api/core";
import type { AppAction } from "./app-actions";
import { ipc } from "./ipc";
import { currentWebviewWindowListener } from "./tauri-events";

export interface NativeMenuSnapshot {
  language: string;
  enabled: AppAction[];
}

/** Coalesce pending states and serialize IPC so an older reply cannot win. */
export function createNativeMenuPublisher(
  send: (snapshot: NativeMenuSnapshot) => Promise<void>,
  onFailure: () => void,
) {
  let disposed = false;
  let running = false;
  let pending: NativeMenuSnapshot | null = null;
  let lastRequested = "";

  async function flush(): Promise<void> {
    if (running || disposed) return;
    running = true;
    try {
      while (pending && !disposed) {
        const snapshot = pending;
        pending = null;
        await send(snapshot);
      }
    } catch {
      lastRequested = "";
      if (!disposed) onFailure();
    } finally {
      running = false;
      if (pending && !disposed) void flush();
    }
  }

  return {
    publish(snapshot: NativeMenuSnapshot, force = false): void {
      if (disposed) return;
      const key = JSON.stringify(snapshot);
      if (!force && key === lastRequested) return;
      lastRequested = key;
      pending = snapshot;
      void flush();
    },
    dispose(): void {
      disposed = true;
      pending = null;
    },
  };
}

export type NativeMenuConnection = ReturnType<typeof createNativeMenuPublisher>;

export async function connectNativeMenu(
  onAction: (action: string) => void,
  onFailure: () => void,
  onSuccess: () => void,
): Promise<NativeMenuConnection | null> {
  if (!isTauri()) return null;
  const listen = await currentWebviewWindowListener();
  const unlisten: Array<() => void> = [];
  try {
    unlisten.push(await listen<string>("app://menu-action", (event) => onAction(event.payload)));
    unlisten.push(await listen("app://menu-error", onFailure));
    unlisten.push(await listen("app://menu-updated", onSuccess));
    const publisher = createNativeMenuPublisher(ipc.updateNativeMenu, onFailure);
    return {
      publish: publisher.publish,
      dispose() {
        publisher.dispose();
        unlisten.forEach((stop) => stop());
      },
    };
  } catch (error) {
    unlisten.forEach((stop) => stop());
    throw error;
  }
}
