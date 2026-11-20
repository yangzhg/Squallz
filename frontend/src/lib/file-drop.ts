import { isTauri } from "@tauri-apps/api/core";
import { isAbsoluteDesktopPath, type DesktopPathPlatform } from "./desktop-path";
import { currentWebviewWindowListener } from "./tauri-events";

function pathFromLine(line: string, platform: DesktopPathPlatform): string | null {
  if (isAbsoluteDesktopPath(line, platform)) return line;
  if (!/^file:\/\//iu.test(line)) return null;
  try {
    const url = new URL(line);
    let path = decodeURIComponent(url.pathname);
    if (url.hostname) {
      if (platform !== "windows") return null;
      path = `//${url.hostname}${path}`;
    } else if (platform === "windows" && /^\/[a-z]:\//iu.test(path)) {
      path = path.slice(1);
    }
    return isAbsoluteDesktopPath(path, platform) ? path : null;
  } catch {
    return null;
  }
}

export function domDropPaths(transfer: DataTransfer, platform: DesktopPathPlatform): string[] {
  const files = Array.from(transfer.files, (file) => (file as File & { path?: string }).path);
  const paths = files.filter((path): path is string => Boolean(path && isAbsoluteDesktopPath(path, platform)));
  const text = transfer.getData("text/uri-list") || transfer.getData("text/plain");
  for (const line of text.split(/\r?\n/u)) {
    const path = pathFromLine(line, platform);
    if (path) paths.push(path);
  }
  return paths;
}

export function listenForFileDrops(options: {
  platform: () => DesktopPathPlatform;
  available: () => boolean;
  hover: (active: boolean) => void;
  drop: (paths: string[], source: "dom" | "native") => void;
  missingPath: () => void;
}) {
  let native = isTauri();
  let disposed = false;
  const nativeListeners: Array<() => void> = [];

  function clearNativeListeners() {
    for (const stop of nativeListeners.splice(0)) stop();
  }

  async function subscribeNative() {
    try {
      const listen = await currentWebviewWindowListener();
      if (disposed) return;
      for (const event of ["tauri://drag-enter", "tauri://drag-leave", "tauri://drag-drop"]) {
        const stop = await listen<{ paths: string[] } | null>(event, ({ payload }) => {
          if (disposed || !native) return;
          const paths = payload?.paths ?? [];
          options.hover(event === "tauri://drag-enter" && paths.length > 0 && options.available());
          if (event === "tauri://drag-drop" && paths.length) options.drop(paths, "native");
        });
        if (disposed) {
          stop();
          return;
        }
        nativeListeners.push(stop);
      }
    } catch {
      native = false;
      clearNativeListeners();
      if (!disposed) options.hover(false);
    }
  }

  if (native) void subscribeNative();

  function handleDrag(event: DragEvent) {
    const transfer = event.dataTransfer;
    if (!transfer) return;
    const files = transfer.types.includes("Files");
    const editingText = event.target instanceof Element && event.target.closest("input, textarea, [contenteditable=true]");
    if (!files && editingText) return;
    event.preventDefault();
    if (native) return;
    if (event.type === "dragover") {
      const available = options.available();
      options.hover(files && available);
      if (!available) transfer.dropEffect = "none";
      return;
    }
    options.hover(false);
    const paths = domDropPaths(transfer, options.platform());
    if (paths.length) options.drop(paths, "dom");
    else if (files && options.available()) options.missingPath();
  }

  function leave(event: DragEvent) {
    if (!native && !(event.relatedTarget instanceof Node && document.body.contains(event.relatedTarget))) options.hover(false);
  }
  const controller = new AbortController();
  window.addEventListener("dragover", handleDrag, { signal: controller.signal });
  window.addEventListener("drop", handleDrag, { signal: controller.signal });
  window.addEventListener("dragleave", leave, { signal: controller.signal });
  return () => {
    disposed = true;
    clearNativeListeners();
    controller.abort();
  };
}
