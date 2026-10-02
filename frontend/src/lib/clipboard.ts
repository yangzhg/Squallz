export type ClipboardCopyResult = "copied" | "failed" | "superseded";

interface ClipboardRequest {
  text: string;
  isCurrent: () => boolean;
  resolve: (result: ClipboardCopyResult) => void;
}

interface ClipboardSelection {
  element: Element | null;
  input: { start: number; end: number; direction: "forward" | "backward" | "none" | null } | null;
  ranges: Range[];
}

let latestRequest: ClipboardRequest | null = null;
let queuedRequest: ClipboardRequest | null = null;
let writing = false;

function requestIsCurrent(request: ClipboardRequest): boolean {
  try {
    return latestRequest === request && request.isCurrent();
  } catch {
    return false;
  }
}

function captureSelection(): ClipboardSelection {
  const element = document.activeElement;
  let input: ClipboardSelection["input"] = null;
  if (element instanceof HTMLInputElement || element instanceof HTMLTextAreaElement) {
    const start = element.selectionStart;
    const end = element.selectionEnd;
    if (start !== null && end !== null) {
      input = { start, end, direction: element.selectionDirection };
    }
  }
  const selection = document.getSelection();
  const ranges: Range[] = [];
  if (selection) {
    for (let index = 0; index < selection.rangeCount; index += 1) {
      ranges.push(selection.getRangeAt(index).cloneRange());
    }
  }
  return { element, input, ranges };
}

function restoreSelection(proxy: HTMLTextAreaElement, saved: ClipboardSelection): void {
  const active = document.activeElement;
  const ownsFocus = active === proxy;
  if (!ownsFocus && active !== saved.element) return;
  const selection = document.getSelection();
  const ownsSelection = selection !== null
    && (proxy.contains(selection.anchorNode) || proxy.contains(selection.focusNode));
  if (!ownsFocus && !ownsSelection) return;
  if (ownsFocus && saved.element instanceof HTMLElement && saved.element.isConnected) {
    saved.element.focus({ preventScroll: true });
  }
  if (document.activeElement === saved.element && saved.input
    && (saved.element instanceof HTMLInputElement || saved.element instanceof HTMLTextAreaElement)) {
    saved.element.setSelectionRange(saved.input.start, saved.input.end, saved.input.direction ?? undefined);
  }
  if (ownsSelection && selection) {
    selection.removeAllRanges();
    for (const range of saved.ranges) {
      if (range.startContainer.isConnected && range.endContainer.isConnected) selection.addRange(range);
    }
  }
}

function copyWithSelection(request: ClipboardRequest): ClipboardCopyResult {
  if (!requestIsCurrent(request)) return "superseded";
  let proxy: HTMLTextAreaElement | null = null;
  let saved: ClipboardSelection | null = null;
  let result: ClipboardCopyResult = "failed";
  try {
    saved = captureSelection();
    proxy = document.createElement("textarea");
    proxy.value = request.text;
    proxy.setAttribute("readonly", "true");
    proxy.className = "clipboard-proxy";
    document.body.appendChild(proxy);
    proxy.select();
    result = requestIsCurrent(request)
      ? document.execCommand("copy") ? "copied" : "failed"
      : "superseded";
  } catch {
    result = "failed";
  } finally {
    if (proxy) {
      try {
        if (saved) restoreSelection(proxy, saved);
      } catch {
        // Focus restoration must not change the result of the clipboard write.
      }
      try {
        proxy.value = "";
      } catch {
        // Continue removing the proxy if clearing its value is unavailable.
      }
      try {
        proxy.remove();
      } catch {
        try {
          proxy.parentNode?.removeChild(proxy);
        } catch {
          // Proxy cleanup must not change the result of the clipboard write.
        }
      }
    }
  }
  return requestIsCurrent(request) ? result : "superseded";
}

async function writeRequest(request: ClipboardRequest): Promise<ClipboardCopyResult> {
  try {
    await navigator.clipboard.writeText(request.text);
    return requestIsCurrent(request) ? "copied" : "superseded";
  } catch {
    return copyWithSelection(request);
  }
}

function startNextRequest(): void {
  if (writing || !queuedRequest) return;
  const request = queuedRequest;
  queuedRequest = null;
  if (!requestIsCurrent(request)) {
    request.resolve("superseded");
    if (latestRequest === request) latestRequest = null;
    return;
  }
  writing = true;
  // Keep the slot until the native write and any fallback have actually finished.
  const finish = (result: ClipboardCopyResult) => {
    const outcome = requestIsCurrent(request) ? result : "superseded";
    writing = false;
    if (latestRequest === request) latestRequest = null;
    request.resolve(outcome);
    startNextRequest();
  };
  void writeRequest(request).then(finish, () => finish("failed"));
}

export function copyTextToClipboard(
  text: string,
  isCurrent: () => boolean = () => true,
): Promise<ClipboardCopyResult> {
  if (!text.trim()) return Promise.resolve("failed");
  return new Promise((resolve) => {
    const request = { text, isCurrent, resolve };
    latestRequest = request;
    queuedRequest?.resolve("superseded");
    queuedRequest = request;
    // An idle queue calls writeText synchronously, retaining the click's activation.
    startNextRequest();
  });
}
