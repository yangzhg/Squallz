import definitions from "./app-actions.json";

export type AppAction =
  | "open_archive" | "create_archive" | "extract_all" | "extract_selection"
  | "add_files" | "new_folder" | "rename_entry" | "move_entries" | "delete_entries"
  | "preview_entry" | "copy_entries" | "test_archive" | "convert_archive" | "archive_info"
  | "search_archive" | "go_up" | "task_center" | "select_all" | "settings";

export type AppActionAvailability = Record<AppAction, boolean>;
export type AppActionHandlers = Record<AppAction, () => unknown | Promise<unknown>>;

export interface AppActionContext {
  blocked: boolean;
  taskWindow: boolean;
  opening: boolean;
  archive: boolean;
  writable: boolean;
  browsing: boolean;
  selectionBusy: boolean;
  hasSelection: boolean;
  canRename: boolean;
  canPreview: boolean;
  canSelectAll: boolean;
  hasParent: boolean;
  textSelection: boolean;
  taskCenterAvailable: boolean;
}

export function appActionAvailability(context: AppActionContext): AppActionAvailability {
  const available = !context.blocked && !context.taskWindow;
  const archive = available && context.archive && !context.opening;
  const selection = archive && !context.selectionBusy && context.hasSelection;
  const mutable = archive && context.writable && !context.selectionBusy;
  const browsing = available && context.archive && context.browsing;
  return {
    open_archive: available && !context.opening,
    create_archive: available,
    extract_all: archive && !context.selectionBusy,
    extract_selection: selection,
    add_files: mutable,
    new_folder: mutable,
    rename_entry: mutable && context.canRename,
    move_entries: mutable && selection,
    delete_entries: mutable && selection,
    preview_entry: archive && !context.selectionBusy && context.canPreview,
    copy_entries: selection,
    test_archive: archive,
    convert_archive: archive,
    archive_info: available,
    search_archive: browsing,
    go_up: browsing && context.hasParent,
    task_center: !context.taskWindow && context.taskCenterAvailable,
    select_all: context.textSelection || !browsing || context.canSelectAll,
    settings: available,
  };
}

export function isAppAction(value: string): value is AppAction {
  return definitions.some((action) => action.id === value);
}

/** Recheck the current state when an action arrives from any input surface. */
export async function dispatchAppAction(
  action: string,
  availability: AppActionAvailability,
  handlers: AppActionHandlers,
): Promise<boolean> {
  if (!isAppAction(action) || !availability[action]) return false;
  await handlers[action]();
  return true;
}

export function isTextEditingTarget(target: EventTarget | null): boolean {
  if (!target || !("tagName" in target)) return false;
  const element = target as HTMLElement;
  if (element.isContentEditable || element.tagName === "TEXTAREA" || element.tagName === "SELECT") return true;
  return element.tagName === "INPUT"
    && !["checkbox", "radio", "button", "submit", "reset", "file", "range", "color", "hidden"].includes((element as HTMLInputElement).type);
}

type ShortcutEvent = Pick<KeyboardEvent,
  "key" | "metaKey" | "ctrlKey" | "altKey" | "shiftKey" | "repeat" | "isComposing" | "defaultPrevented"
>;

/** Browser fallback uses the same accelerator definitions as the native menu. */
export function appActionForShortcut(event: ShortcutEvent, platform: string): AppAction | null {
  if (event.defaultPrevented || event.repeat || event.isComposing) return null;
  for (const action of definitions) {
    const accelerator = platform === "macos" && action.macos_accelerator
      ? action.macos_accelerator : action.accelerator;
    if (!accelerator) continue;
    const parts = accelerator.toLowerCase().split("+");
    const key = parts.pop();
    const modifiers = new Set(parts.map((part) => part === "cmdorctrl" ? platform === "macos" ? "cmd" : "ctrl" : part));
    if (event.key.toLowerCase() === key
      && event.metaKey === modifiers.has("cmd")
      && event.ctrlKey === modifiers.has("ctrl")
      && event.altKey === modifiers.has("alt")
      && event.shiftKey === modifiers.has("shift")) return action.id as AppAction;
  }
  return null;
}
