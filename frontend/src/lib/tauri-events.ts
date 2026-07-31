import type {
  EventCallback,
  EventName,
  UnlistenFn,
} from "@tauri-apps/api/event";

export async function currentWebviewWindowListener(): Promise<
  <T>(event: EventName, handler: EventCallback<T>) => Promise<UnlistenFn>
> {
  const [{ listen }, { getCurrentWindow }] = await Promise.all([
    import("@tauri-apps/api/event"),
    import("@tauri-apps/api/window"),
  ]);
  const target = {
    kind: "WebviewWindow" as const,
    // Squallz creates one webview per native window, so both labels are the same.
    label: getCurrentWindow().label,
  };
  return <T>(event: EventName, handler: EventCallback<T>) =>
    listen<T>(event, handler, { target });
}
