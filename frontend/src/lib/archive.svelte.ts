// Archive browsing store: open/close, breadcrumb navigation, 500-per-page
// cached pagination for the virtual list, selection and archive-wide path search.
// Shared archive browse state for the desktop UI.

import {
  ipc,
  isErrorDto,
  type ArchiveInfo,
  type EntryDto,
  type ErrorDto,
  type Page,
  type PasswordBookStatus,
} from "./ipc";
import { t, tError } from "./i18n.svelte";
import { pushToast, removeToastByKey } from "./toasts.svelte";
import { cancelPasswordBookPreview, readPasswordBookPreview, savePasswordBookPreview, forgetPasswordBookPreview, preparedNestedPreviewRows } from "./dev-preview-data";

export const PAGE_SIZE = 500;
export type PasswordBookStatusState = "idle" | "checking" | "ready" | "error";
export interface PasswordSaveState {
  phase: "idle" | "verifying" | "cancelling" | "saved" | "cancelled" | "error";
  requestId?: string;
  error?: ErrorDto;
  cancelFailed?: boolean;
}
export type RowSelectionResult = "selected" | "stale" | "failed";
type ArchiveViewWindow = { start: number; end: number };
type ArchiveRefreshStatus = "idle" | "refreshing" | "error";
type SelectionAnchor = { index: number; generation: number };
const ARCHIVE_BROWSE_ERROR_TOAST_KEY = "archive-browse-error";
const ARCHIVE_REFRESH_ERROR_TOAST_KEY = "archive-refresh-error";
const ARCHIVE_DIRECTORY_CHANGED_TOAST_KEY = "archive-directory-changed";
const SELECTION_PAGE_CONCURRENCY = 4;

type ValidationArchiveCallKind = "openArchive" | "listEntries" | "searchEntries";
type ValidationArchiveCallCounters = Record<ValidationArchiveCallKind, number>;
type ValidationArchiveCallWindow = Window & {
  __squallzValidationArchiveCalls?: ValidationArchiveCallCounters;
  __squallzValidationArchiveCallSnapshot?: () => ValidationArchiveCallCounters;
  __squallzResetValidationArchiveCalls?: () => ValidationArchiveCallCounters;
};

function emptyValidationArchiveCallCounters(): ValidationArchiveCallCounters {
  return { openArchive: 0, listEntries: 0, searchEntries: 0 };
}

function installValidationArchiveCallCounters(): ValidationArchiveCallCounters | null {
  if (!import.meta.env.DEV || typeof window === "undefined") return null;
  if (!new URLSearchParams(window.location.search).has("validationTrace")) return null;
  const win = window as ValidationArchiveCallWindow;
  win.__squallzValidationArchiveCalls ??= emptyValidationArchiveCallCounters();
  win.__squallzValidationArchiveCallSnapshot ??= () => ({
    ...(win.__squallzValidationArchiveCalls ?? emptyValidationArchiveCallCounters()),
  });
  win.__squallzResetValidationArchiveCalls ??= () => {
    win.__squallzValidationArchiveCalls = emptyValidationArchiveCallCounters();
    return win.__squallzValidationArchiveCallSnapshot?.() ?? emptyValidationArchiveCallCounters();
  };
  return win.__squallzValidationArchiveCalls;
}

function markValidationArchiveCall(kind: ValidationArchiveCallKind): void {
  const counters = installValidationArchiveCallCounters();
  if (!counters) return;
  counters[kind] += 1;
}

const store = $state({
  info: null as ArchiveInfo | null,
  /** Breadcrumb segments below the archive name */
  dirs: [] as string[],
  total: 0,
  pages: new Map<number, EntryDto[]>(),
  loading: new Set<number>(),
  filter: "",
  filterPending: false,
  /** Selected full paths (dirs end with `/`) */
  selected: new Set<string>(),
  /** Selection was expanded across the complete current directory or search result. */
  selectedAllCurrentRows: false,
  /** Invalidates pending selection requests after any later selection change. */
  selectionGeneration: 0,
  selectionAnchor: null as SelectionAnchor | null,
  /** Dev preview-only full row tree used to exercise folder navigation without IPC. */
  previewRows: null as EntryDto[] | null,
  selectedSize: 0,
  /** Bumped on every navigation/filter change to drop stale responses */
  generation: 0,
  /** Most recent failure while listing a directory or searching the archive. */
  browseError: null as ErrorDto | null,
  refreshStatus: "idle" as ArchiveRefreshStatus,
  /** User-selected archive-wide file-name encoding. */
  encodingOverride: null as string | null,
  /** Pending open that needs a password (drives the password dialog) */
  passwordPrompt: null as {
    path: string; wrong: boolean; encoding: string | null;
    visibleWindow?: () => ArchiveViewWindow;
  } | null,
  /** Most recent structured non-password failure, bound to the attempted path. */
  openError: null as { path: string; error: ErrorDto } | null,
  /** Invalidates superseded open requests before they can publish state. */
  openGeneration: 0,
  passwordBookGeneration: 0,
  passwordBookState: "idle" as PasswordBookStatusState,
  passwordBookAvailable: false,
  passwordBookSaved: null as boolean | null,
  passwordBookSession: null as boolean | null,
  passwordSave: { phase: "idle" } as PasswordSaveState,
});

let filterTimer: ReturnType<typeof setTimeout> | undefined;
const pageRequests = new Map<number, { generation: number; promise: Promise<void> }>();
let pendingArchiveOpenRequestId: string | null = null;
let archiveOpenRequestSequence = 0;

function cancelFilterReload(): void {
  if (filterTimer !== undefined) clearTimeout(filterTimer);
  filterTimer = undefined;
}

function nextArchiveOpenRequestId(): string {
  archiveOpenRequestSequence += 1;
  return globalThis.crypto?.randomUUID?.()
    ?? `${Date.now().toString(36)}-${archiveOpenRequestSequence.toString(36)}`;
}

function cancelActiveArchiveOpenRequest(): void {
  const requestId = pendingArchiveOpenRequestId;
  pendingArchiveOpenRequestId = null;
  if (requestId) {
    void ipc.cancelArchiveOpen(requestId).catch(() => {
      // Request invalidation below still prevents a late result from publishing.
    });
  }
}

export function archive(): ArchiveInfo | null {
  return store.info;
}

export function currentDirs(): string[] {
  return store.dirs;
}

export function currentPrefix(): string {
  return store.dirs.length ? store.dirs.join("/") + "/" : "";
}

export function totalRows(): number {
  return store.total;
}

export function loadedRows(): EntryDto[] {
  return [...store.pages.entries()]
    .sort(([left], [right]) => left - right)
    .flatMap(([, rows]) => rows);
}

export function findLoadedRow(path: string): EntryDto | null {
  for (const rows of store.pages.values()) {
    const row = rows.find((entry) => entry.path === path);
    if (row) return row;
  }
  return null;
}

export function loadedRowCount(): number {
  let count = 0;
  for (const rows of store.pages.values()) count += rows.length;
  return count;
}

export function allRowsLoaded(): boolean {
  if (store.filter.trim()) return false;
  return store.total === 0 || loadedRowCount() >= store.total;
}

export function filterText(): string {
  return store.filter;
}

export function filterPending(): boolean {
  return store.filterPending;
}

export function selectedPaths(): Set<string> {
  return store.selected;
}

export function allCurrentRowsSelected(): boolean {
  return store.selectedAllCurrentRows;
}

export function selectedSize(): number {
  return store.selectedSize;
}

export function openPasswordPrompt(): { path: string; wrong: boolean; encoding: string | null } | null {
  return store.passwordPrompt;
}

export function archiveOpenError(path?: string): ErrorDto | null {
  if (!store.openError || (path !== undefined && store.openError.path !== path)) return null;
  return store.openError.error;
}

export function archiveBrowseError(): ErrorDto | null {
  return store.browseError;
}

export function archiveRefreshStatus(): ArchiveRefreshStatus {
  return store.refreshStatus;
}

export function archivePasswordBookStatus(): {
  state: PasswordBookStatusState;
  available: boolean;
  saved: boolean | null;
  session: boolean | null;
} {
  return {
    state: store.passwordBookState,
    available: store.passwordBookAvailable,
    saved: store.passwordBookSaved,
    session: store.passwordBookSession,
  };
}

export function archivePasswordSave(): PasswordSaveState {
  return store.passwordSave;
}

export function archivePasswordSaveBusy(): boolean {
  return store.passwordSave.phase === "verifying" || store.passwordSave.phase === "cancelling";
}

async function cancelPasswordSaveRequest(requestId: string): Promise<void> {
  if (import.meta.env.DEV && typeof window !== "undefined"
    && cancelPasswordBookPreview(new URLSearchParams(window.location.search), requestId)) return;
  await ipc.cancelPasswordSave(requestId);
}

export async function cancelArchivePasswordSave(): Promise<void> {
  const requestId = store.passwordSave.requestId;
  if (!requestId || !archivePasswordSaveBusy()) return;
  store.passwordSave = { phase: "cancelling", requestId };
  try {
    await cancelPasswordSaveRequest(requestId);
    // The save response decides the outcome, including a write already in progress.
  } catch {
    if (store.passwordSave.requestId === requestId && archivePasswordSaveBusy()) {
      store.passwordSave = { phase: "verifying", requestId, cancelFailed: true };
    }
  }
}

export function clearArchivePasswordSave(): void {
  if (store.passwordSave.phase === "idle") return;
  const requestId = archivePasswordSaveBusy() ? store.passwordSave.requestId : undefined;
  store.passwordSave = { phase: "idle" };
  if (requestId) void cancelPasswordSaveRequest(requestId).catch(() => undefined);
}

/** Active archive-wide name encoding override, if the user selected one. */
export function archiveEncoding(): string | null {
  return store.encodingOverride;
}

/**
 * Opens an archive. A `error.password_required` / `error.wrong_password`
 * answer raises the password prompt instead of a toast; other errors toast.
 */
export async function openArchive(
  path: string,
  password?: string | null,
  encoding?: string | null,
): Promise<boolean> {
  const visibleWindow = store.passwordPrompt?.path === path ? store.passwordPrompt.visibleWindow : undefined;
  clearArchiveRefreshStatus();
  return performArchiveOpen(path, password ?? null, encoding ?? null, visibleWindow);
}

async function performArchiveOpen(
  path: string,
  password: string | null,
  encoding: string | null,
  visibleWindow?: () => ArchiveViewWindow,
): Promise<boolean> {
  cancelActiveArchiveOpenRequest();
  const requestId = nextArchiveOpenRequestId();
  pendingArchiveOpenRequestId = requestId;
  const requestGeneration = ++store.openGeneration;
  const retryingPrompt = store.passwordPrompt?.path === path;
  if (!retryingPrompt) store.passwordPrompt = null;
  store.openError = null;
  const previousId = store.info?.id;
  if (visibleWindow) {
    store.refreshStatus = "refreshing";
    removeToastByKey(ARCHIVE_REFRESH_ERROR_TOAST_KEY);
  }
  let pendingInfo: ArchiveInfo | null = null;
  try {
    markValidationArchiveCall("openArchive");
    const info = await ipc.openArchive(
      path,
      password,
      encoding,
      requestId,
    );
    pendingInfo = info;
    if (requestGeneration !== store.openGeneration) {
      void ipc.closeArchive(info.id);
      pendingInfo = null;
      return false;
    }
    const view = await loadOpenedArchiveView(info.id, requestGeneration, visibleWindow);
    if (!view || requestGeneration !== store.openGeneration) {
      void ipc.closeArchive(info.id);
      pendingInfo = null;
      return false;
    }
    if (store.info) void ipc.closeArchive(store.info.id);
    store.info = info;
    store.dirs = view.dirs;
    cancelFilterReload();
    store.filter = view.filter;
    store.filterPending = false;
    store.previewRows = null;
    clearBrowseError();
    clearArchiveRefreshStatus();
    store.generation += 1;
    store.pages = view.pages;
    store.loading = new Set();
    store.total = view.total;
    store.encodingOverride = info.encoding_override ?? encoding ?? null;
    store.passwordPrompt = null;
    store.openError = null;
    clearPasswordBookStatus();
    clearSelection();
    reportArchiveDirectoryChange(view.requestedDirs, view.dirs);
    pendingInfo = null;
    if (!info.read_only) refreshArchivePasswordBookStatusInBackground(info.path);
    return true;
  } catch (e) {
    if (pendingInfo) void ipc.closeArchive(pendingInfo.id);
    if (requestGeneration !== store.openGeneration) return false;
    if (isErrorDto(e) && (e.key === "error.password_required" || e.key === "error.wrong_password")) {
      store.passwordPrompt = {
        path,
        wrong: e.key === "error.wrong_password" || password != null,
        encoding,
        visibleWindow,
      };
      return false;
    }
    store.passwordPrompt = null;
    if (visibleWindow) {
      store.refreshStatus = "error";
      pushToast({
        key: ARCHIVE_REFRESH_ERROR_TOAST_KEY,
        kind: "danger",
        title: t("gui.archive.refresh_failed"),
        body: isErrorDto(e) ? tError(e) : t("gui.archive.open_failed_generic"),
        action: {
          label: t("gui.error.retry"),
          run: () => store.info?.id === previousId
            ? performArchiveOpen(path, password, encoding, visibleWindow)
            : false,
        },
      });
      return false;
    }
    if (isErrorDto(e)) {
      store.openError = { path, error: e };
      pushToast({ kind: "danger", title: tError(e) });
    } else {
      store.openError = {
        path,
        error: { key: "error.unknown", params: {}, detail: "" },
      };
      pushToast({ kind: "danger", title: t("gui.archive.open_failed_generic") });
    }
    return false;
  } finally {
    if (pendingArchiveOpenRequestId === requestId) {
      pendingArchiveOpenRequestId = null;
    }
  }
}

function viewPages(window: ArchiveViewWindow, total: number): number[] {
  const count = Math.max(1, window.end - window.start);
  const start = Math.max(0, Math.min(window.start, total - count));
  const end = Math.max(start, Math.min(total - 1, start + count - 1));
  const first = Math.floor(start / PAGE_SIZE);
  const last = Math.floor(end / PAGE_SIZE);
  return Array.from({ length: last - first + 1 }, (_, index) => first + index);
}

/** Loads the current browse context and visible pages before publishing them together. */
async function loadOpenedArchiveView(
  id: number,
  requestGeneration: number,
  visibleWindow?: () => ArchiveViewWindow,
) {
  while (requestGeneration === store.openGeneration) {
    const generation = store.generation;
    const requestedDirs = visibleWindow ? [...store.dirs] : [];
    const filter = visibleWindow ? store.filter : "";
    try {
      const dirs = requestedDirs.length ? await resolveArchiveDirs(id, requestedDirs) : requestedDirs;
      if (requestGeneration !== store.openGeneration) return null;
      if (visibleWindow && generation !== store.generation) continue;
      const prefix = dirs.length ? `${dirs.join("/")}/` : "";
      const window = visibleWindow?.() ?? { start: 0, end: PAGE_SIZE };
      const readPage = async (page: number): Promise<Page> => {
        if (filter.trim()) {
          const result = await searchArchivePage(id, page, filter.trim(), generation + 1);
          if (result) return result;
          throw { key: "error.cancelled", params: {}, detail: "" } satisfies ErrorDto;
        }
        markValidationArchiveCall("listEntries");
        return ipc.listEntries(id, page, prefix, null, PAGE_SIZE);
      };
      const first = await readPage(Math.max(0, Math.floor(window.start / PAGE_SIZE)));
      if (requestGeneration !== store.openGeneration) return null;
      if (visibleWindow && generation !== store.generation) continue;
      const pages = viewPages(window, first.total);
      const loaded = await Promise.all(pages.map((page) => page === first.page ? first : readPage(page)));
      if (requestGeneration !== store.openGeneration) return null;
      if (visibleWindow && (
        generation !== store.generation ||
        viewPages(visibleWindow(), first.total).some((page) => !pages.includes(page))
      )) continue;
      return {
        requestedDirs, dirs, filter, total: first.total,
        pages: new Map(loaded.map((page) => [page.page, page.items])),
      };
    } catch (error) {
      if (requestGeneration !== store.openGeneration) return null;
      if (visibleWindow && generation !== store.generation) continue;
      throw error;
    }
  }
  return null;
}

async function resolveArchiveDirs(id: number, dirs: string[]): Promise<string[]> {
  const prefix = await ipc.resolveArchiveDirectory(id, `${dirs.join("/")}/`);
  return prefix.split("/").filter(Boolean);
}

function reportArchiveDirectoryChange(previous: string[], current: string[]): void {
  if (previous.join("/") === current.join("/")) return;
  pushToast({
    key: ARCHIVE_DIRECTORY_CHANGED_TOAST_KEY,
    kind: "warning",
    title: t("gui.archive.folder_unavailable", {
      folder: previous.join("/"),
      parent: current.join("/") || t("gui.list.archive_root"),
    }),
  });
}

function clearArchiveRefreshStatus(): void {
  store.refreshStatus = "idle";
  removeToastByKey(ARCHIVE_REFRESH_ERROR_TOAST_KEY);
}

/** Publishes an opened archive only while its initiating action is still current. */
export async function adoptOpenedArchive(info: ArchiveInfo, isCurrent: () => boolean): Promise<boolean> {
  if (!isCurrent()) {
    void ipc.closeArchive(info.id);
    return false;
  }
  cancelActiveArchiveOpenRequest();
  const requestGeneration = ++store.openGeneration;
  const previewRows = import.meta.env.DEV && typeof window !== "undefined"
    ? preparedNestedPreviewRows(new URLSearchParams(window.location.search), info.source) : null;
  let page: Awaited<ReturnType<typeof ipc.listEntries>>;
  try {
    markValidationArchiveCall("listEntries");
    const rootRows = previewRows?.filter((row) => !row.path.replace(/\/+$/g, "").includes("/"));
    page = rootRows ? { items: rootRows.slice(0, PAGE_SIZE), total: rootRows.length, page: 0 }
      : await ipc.listEntries(info.id, 0, "", null, PAGE_SIZE);
  } catch (error) {
    void ipc.closeArchive(info.id);
    if (requestGeneration !== store.openGeneration || !isCurrent()) return false;
    throw error;
  }
  if (requestGeneration !== store.openGeneration || !isCurrent()) {
    void ipc.closeArchive(info.id);
    return false;
  }
  if (store.info && store.info.id !== info.id) void ipc.closeArchive(store.info.id).catch(() => undefined);
  store.info = info;
  store.dirs = [];
  cancelFilterReload();
  store.filter = "";
  store.filterPending = false;
  store.previewRows = previewRows;
  clearBrowseError();
  clearArchiveRefreshStatus();
  store.generation += 1;
  store.pages = new Map([[0, page.items]]);
  store.loading = new Set();
  store.total = page.total;
  store.encodingOverride = info.encoding_override ?? null;
  store.passwordPrompt = null;
  store.openError = null;
  clearPasswordBookStatus();
  clearSelection();
  if (!info.read_only) refreshArchivePasswordBookStatusInBackground(info.path);
  return true;
}

/** Invalidates an in-flight open and dismisses its pending state. */
export function cancelPendingArchiveOpen(): void {
  cancelActiveArchiveOpenRequest();
  store.openGeneration += 1;
  store.passwordPrompt = null;
  store.openError = null;
  clearArchiveRefreshStatus();
}

/** Dismisses the open-time password prompt. */
export function cancelPasswordPrompt(): void {
  cancelPendingArchiveOpen();
}

export function closeArchive(): void {
  cancelPendingArchiveOpen();
  store.generation += 1;
  if (store.info) void ipc.closeArchive(store.info.id);
  store.info = null;
  store.dirs = [];
  cancelFilterReload();
  store.filter = "";
  store.filterPending = false;
  store.pages = new Map();
  store.loading = new Set();
  store.total = 0;
  store.previewRows = null;
  clearBrowseError();
  store.encodingOverride = null;
  store.passwordPrompt = null;
  store.openError = null;
  clearPasswordBookStatus();
  clearSelection();
}

function clearPasswordBookStatus(): void {
  clearArchivePasswordSave();
  store.passwordBookGeneration += 1;
  store.passwordBookState = "idle";
  store.passwordBookAvailable = false;
  store.passwordBookSaved = null;
  store.passwordBookSession = null;
}

function applyPasswordBookStatus(status: PasswordBookStatus): void {
  store.passwordBookAvailable = status.available;
  store.passwordBookSaved = status.saved;
  store.passwordBookSession = status.session;
  store.passwordBookState = status.error ? "error" : "ready";
}

/** Reopens the current archive with a user-selected file-name encoding. */
export async function reopenWithEncoding(encoding: string | null): Promise<boolean> {
  const current = store.info;
  if (!current) return false;
  return performArchiveOpen(current.source, null, encoding, () => ({ start: 0, end: PAGE_SIZE }));
}

/** Reopens the current archive after an in-place update and refreshes rows. */
export async function refreshCurrentArchive(
  visibleWindow: () => ArchiveViewWindow = () => ({ start: 0, end: PAGE_SIZE }),
): Promise<boolean> {
  const current = store.info;
  if (!current) return false;
  return performArchiveOpen(current.source, null, store.encodingOverride, visibleWindow);
}

export async function refreshArchivePasswordBookStatus(path = store.info?.path): Promise<void> {
  if (!path || store.info?.read_only) {
    clearPasswordBookStatus();
    return;
  }
  if (store.info?.path !== path || archivePasswordSaveBusy()) return;
  const generation = ++store.passwordBookGeneration;
  store.passwordBookState = "checking";
  let status: PasswordBookStatus;
  try {
    const preview = import.meta.env.DEV && typeof window !== "undefined"
      ? readPasswordBookPreview(new URLSearchParams(window.location.search)) : null;
    status = preview ?? await ipc.archivePasswordStatus(path);
  } catch (error) {
    if (store.info?.path === path && store.passwordBookGeneration === generation) {
      store.passwordBookState = "error";
      store.passwordBookSaved = null;
      store.passwordBookSession = null;
    }
    throw error;
  }
  if (store.info?.path !== path || store.passwordBookGeneration !== generation) return;
  applyPasswordBookStatus(status);
  if (status.error) throw status.error;
}

function refreshArchivePasswordBookStatusInBackground(path: string): void {
  void refreshArchivePasswordBookStatus(path).catch(() => undefined);
}

export async function rememberArchivePassword(
  path: string,
  password: string,
  encoding?: string | null,
): Promise<boolean> {
  const current = store.info;
  if (!password || !current || current.path !== path || current.read_only
    || store.passwordBookState !== "ready" || !store.passwordBookAvailable || archivePasswordSaveBusy()) return false;
  const requestId = nextArchiveOpenRequestId();
  const isCurrent = () => store.info?.id === current.id && store.info?.path === path
    && store.passwordSave.requestId === requestId;
  const generation = ++store.passwordBookGeneration;
  store.passwordSave = { phase: "verifying", requestId };
  try {
    const preview = import.meta.env.DEV && typeof window !== "undefined"
      ? savePasswordBookPreview(new URLSearchParams(window.location.search), requestId, password) : null;
    const status = await (preview ?? ipc.rememberArchivePassword(path, password, encoding ?? null, requestId));
    if (store.info?.id === current.id && store.info?.path === path && store.passwordBookGeneration === generation) {
      store.passwordBookGeneration += 1;
      applyPasswordBookStatus(status);
    }
    if (!isCurrent()) return false;
    store.passwordSave = { phase: "saved", requestId };
    return true;
  } catch (e) {
    if (!isCurrent()) return false;
    store.passwordSave = isErrorDto(e) && e.key === "error.cancelled"
      ? { phase: "cancelled", requestId }
      : { phase: "error", requestId, error: isErrorDto(e) ? e : { key: "gui.settings.password_book.save_failed", params: {}, detail: "" } };
    return false;
  }
}

export async function forgetCurrentArchivePassword(): Promise<boolean> {
  const current = store.info;
  if (!current || current.read_only || archivePasswordSaveBusy() || store.passwordBookState === "checking") return false;
  const { path, id } = current;
  const toastKey = `archive-password-forget:${id}`;
  const generation = ++store.passwordBookGeneration;
  store.passwordBookState = "checking";
  clearArchivePasswordSave();
  try {
    const preview = import.meta.env.DEV && typeof window !== "undefined"
      ? forgetPasswordBookPreview(new URLSearchParams(window.location.search)) : null;
    const status = preview ?? await ipc.forgetArchivePassword(path);
    if (store.info?.id !== id || store.passwordBookGeneration !== generation) return false;
    if (store.info?.path === path) {
      store.passwordBookGeneration += 1;
      applyPasswordBookStatus(status);
    }
    if (status.saved !== false || status.error) {
      pushToast({
        key: toastKey,
        kind: "warning",
        title: t("gui.password.session_forgotten"),
        body: t("gui.password.saved_forget_unconfirmed"),
        persistent: true,
      });
      return false;
    }
    pushToast({ key: toastKey, kind: "success", title: t("gui.password.forgotten") });
    return true;
  } catch (e) {
    if (store.info?.id !== id || store.passwordBookGeneration !== generation) return false;
    store.passwordBookState = "error";
    store.passwordBookSaved = null;
    store.passwordBookSession = null;
    if (isErrorDto(e)) {
      pushToast({ key: toastKey, kind: "danger", title: tError(e), detail: e.detail });
    } else {
      pushToast({ key: toastKey, kind: "danger", title: String(e) });
    }
    return false;
  }
}

/** Reloads page 0 of the current level. */
async function reload(): Promise<void> {
  if (!store.info) {
    store.filterPending = false;
    return;
  }
  store.filterPending = true;
  const generation = ++store.generation;
  const id = store.info.id;
  const previousDirs = [...store.dirs];
  clearBrowseError();
  store.pages = new Map();
  store.loading = new Set();
  store.total = 0;
  try {
    const previewRows = previewRowsForCurrentLevel();
    if (previewRows) {
      if (generation !== store.generation) return;
      store.pages = new Map([[0, previewRows.slice(0, PAGE_SIZE)]]);
      store.total = previewRows.length;
      return;
    }
    const query = store.filter.trim();
    let page = query
      ? await searchArchivePage(id, 0, query, generation)
      : await listArchiveLevelPage(id, 0, generation);
    if (generation !== store.generation || page === null) return;
    if (!query && page.total === 0 && previousDirs.length) {
      const dirs = await resolveArchiveDirs(id, previousDirs);
      if (generation !== store.generation) return;
      if (dirs.join("/") !== previousDirs.join("/")) {
        markValidationArchiveCall("listEntries");
        page = await ipc.listEntries(id, 0, dirs.length ? `${dirs.join("/")}/` : "", null, PAGE_SIZE);
        if (generation !== store.generation) return;
        store.dirs = dirs;
        clearSelection();
        reportArchiveDirectoryChange(previousDirs, dirs);
      }
    }
    store.pages = new Map([[0, page.items]]);
    store.total = page.total;
  } catch (error) {
    publishBrowseError(error, generation);
  } finally {
    if (generation === store.generation) store.filterPending = false;
  }
}

/** Returns a row by absolute index, fetching its page on demand. */
export function rowAt(index: number): EntryDto | null {
  if (!Number.isInteger(index) || index < 0 || index >= store.total) return null;
  const pageNo = Math.floor(index / PAGE_SIZE);
  const page = store.pages.get(pageNo);
  if (page) return page[index % PAGE_SIZE] ?? null;
  void fetchPage(pageNo);
  return null;
}

/** Returns a row by absolute index after its page is available. */
export async function loadRowAt(index: number): Promise<EntryDto | null> {
  if (!Number.isInteger(index) || index < 0 || index >= store.total || store.filterPending) {
    return null;
  }
  const pageNo = Math.floor(index / PAGE_SIZE);
  const generation = store.generation;
  await fetchPage(pageNo);
  if (generation !== store.generation) return null;
  return store.pages.get(pageNo)?.[index % PAGE_SIZE] ?? null;
}

/** Prefetches `count` pages starting at the one containing `index`. */
export function prefetchAround(index: number, count = 2): void {
  if (store.filterPending) return;
  const pageNo = Math.floor(index / PAGE_SIZE);
  for (let p = pageNo; p <= pageNo + count; p++) {
    if (p * PAGE_SIZE < Math.max(store.total, 1)) void fetchPage(p);
  }
}

async function fetchPage(pageNo: number): Promise<void> {
  if (!store.info || store.filterPending) return;
  if (store.pages.has(pageNo)) return;
  const generation = store.generation;
  const pending = pageRequests.get(pageNo);
  if (pending?.generation === generation) {
    await pending.promise;
    return;
  }
  const loading = store.loading;
  loading.add(pageNo);
  const promise = loadPage(pageNo, generation).catch((error) => {
    publishBrowseError(error, generation);
  });
  pageRequests.set(pageNo, { generation, promise });
  try {
    await promise;
  } finally {
    loading.delete(pageNo);
    if (pageRequests.get(pageNo)?.promise === promise) pageRequests.delete(pageNo);
  }
}

async function loadPage(pageNo: number, generation: number): Promise<void> {
  const previewRows = previewRowsForCurrentLevel();
  if (previewRows) {
    if (generation !== store.generation) return;
    const pages = new Map(store.pages);
    pages.set(pageNo, previewRows.slice(pageNo * PAGE_SIZE, (pageNo + 1) * PAGE_SIZE));
    store.pages = pages;
    store.total = previewRows.length;
    return;
  }
  const info = store.info;
  if (!info) return;
  const query = store.filter.trim();
  const page = query
    ? await searchArchivePage(info.id, pageNo, query, generation)
    : await listArchiveLevelPage(info.id, pageNo, generation);
  if (generation !== store.generation || page === null) return;
  const pages = new Map(store.pages);
  pages.set(pageNo, page.items);
  store.pages = pages;
  store.total = page.total;
}

function previewRowsForCurrentLevel(): EntryDto[] | null {
  if (!import.meta.env.DEV || !store.previewRows) return null;
  const filter = store.filter.trim().toLowerCase();
  if (filter) {
    return store.previewRows
      .filter(
        (row) =>
          row.path.toLowerCase().includes(filter) || row.display.toLowerCase().includes(filter),
      );
  }
  const prefix = currentPrefix();
  return store.previewRows.filter((row) => {
    if (!row.path.startsWith(prefix)) return false;
    const remainder = row.path.slice(prefix.length);
    if (!remainder) return false;
    const visibleName = row.entry_type === "dir" ? remainder.replace(/\/+$/g, "") : remainder;
    if (!visibleName || visibleName.includes("/")) return false;
    return true;
  });
}

async function listArchiveLevelPage(id: number, page: number, generation: number) {
  const dirPrefix = currentPrefix();
  await ipc.cancelArchiveSearch(id, generation);
  markValidationArchiveCall("listEntries");
  return ipc.listEntries(id, page, dirPrefix, null, PAGE_SIZE);
}

async function searchArchivePage(id: number, page: number, query: string, generation: number) {
  markValidationArchiveCall("searchEntries");
  return ipc.searchEntries(id, page, query, PAGE_SIZE, generation);
}

function clearBrowseError(): void {
  store.browseError = null;
  removeToastByKey(ARCHIVE_BROWSE_ERROR_TOAST_KEY);
  removeToastByKey(ARCHIVE_DIRECTORY_CHANGED_TOAST_KEY);
}

function publishBrowseError(error: unknown, generation: number): void {
  if (generation !== store.generation) return;
  const browseError = isErrorDto(error)
    ? error
    : { key: "gui.error.other.title", params: {}, detail: "" };
  store.browseError = browseError;
  pushToast({
    key: ARCHIVE_BROWSE_ERROR_TOAST_KEY,
    kind: "danger",
    title: store.filter.trim() ? t("gui.list.search_failed") : t("gui.list.browse_failed"),
    action: { label: t("gui.error.retry"), run: retryArchiveBrowse },
  });
}

/** Retries the current directory listing or archive-wide search. */
export async function retryArchiveBrowse(): Promise<void> {
  if (!store.info) return;
  store.filterPending = true;
  await reload();
}

/** Enters a directory row. */
export async function enterDir(name: string): Promise<void> {
  await enterDirPath(`${currentPrefix()}${name}/`);
}

/** Enters a directory from an archive-wide search result. */
export async function enterDirPath(path: string): Promise<void> {
  clearSelection();
  store.dirs = path
    .replaceAll("\\", "/")
    .replace(/^\/+|\/+$/g, "")
    .split("/")
    .filter(Boolean);
  cancelFilterReload();
  store.filter = "";
  store.filterPending = false;
  await reload();
}

/** Jumps to a breadcrumb level (`-1` = archive root). */
export async function gotoBreadcrumb(level: number): Promise<void> {
  clearSelection();
  store.dirs = store.dirs.slice(0, level + 1);
  cancelFilterReload();
  store.filter = "";
  store.filterPending = false;
  await reload();
}

/** Goes one level up (Cmd+↑). */
export async function goUp(): Promise<void> {
  if (store.dirs.length === 0) return;
  clearSelection();
  store.dirs.pop();
  cancelFilterReload();
  store.filter = "";
  store.filterPending = false;
  await reload();
}

/** Sets the filter text with the 300 ms engine debounce. */
export function setFilter(text: string): void {
  cancelFilterReload();
  clearBrowseError();
  store.filter = text;
  store.filterPending = true;
  store.generation += 1;
  cancelBackendSearch(store.generation);
  store.pages = new Map();
  store.loading = new Set();
  store.total = 0;
  clearSelection();
  filterTimer = setTimeout(() => {
    filterTimer = undefined;
    void reload();
  }, 300);
}

function cancelBackendSearch(generation: number): void {
  if (!store.info || (import.meta.env.DEV && store.previewRows)) return;
  void ipc.cancelArchiveSearch(store.info.id, generation).catch(() => undefined);
}

/* ---- Selection ---- */

function selectionCoversLoadedCurrentRows(selected: ReadonlySet<string>): boolean {
  if (store.total === 0 || loadedRowCount() < store.total) return false;
  for (const rows of store.pages.values()) {
    if (rows.some((row) => !selected.has(row.path))) return false;
  }
  return true;
}

function selectionRowIndex(row: EntryDto, index?: number): number | null {
  if (
    index !== undefined && Number.isInteger(index) && index >= 0 && index < store.total
    && store.pages.get(Math.floor(index / PAGE_SIZE))?.[index % PAGE_SIZE]?.path === row.path
  ) return index;
  for (const [pageNumber, rows] of store.pages) {
    const offset = rows.findIndex((entry) => entry.path === row.path);
    if (offset >= 0) return pageNumber * PAGE_SIZE + offset;
  }
  return null;
}

function setSelectionAnchor(row: EntryDto, index?: number): void {
  const rowIndex = selectionRowIndex(row, index);
  store.selectionAnchor = rowIndex === null ? null : { index: rowIndex, generation: store.generation };
}

/** Replaces the selection with a single row and starts a new range anchor. */
export function selectRow(row: EntryDto, index?: number): void {
  if (store.filterPending) return;
  store.selectionGeneration += 1;
  store.selected = new Set([row.path]);
  store.selectedSize = row.size;
  store.selectedAllCurrentRows = selectionCoversLoadedCurrentRows(store.selected);
  setSelectionAnchor(row, index);
}

export function toggleSelect(row: EntryDto, index?: number): void {
  if (store.filterPending) return;
  store.selectionGeneration += 1;
  const selected = new Set(store.selected);
  if (selected.has(row.path)) {
    selected.delete(row.path);
    store.selectedSize -= row.size;
  } else {
    selected.add(row.path);
    store.selectedSize += row.size;
  }
  store.selected = selected;
  store.selectedAllCurrentRows = selectionCoversLoadedCurrentRows(selected);
  setSelectionAnchor(row, index);
}

export function clearSelection(): void {
  store.selectionGeneration += 1;
  store.selected = new Set();
  store.selectedAllCurrentRows = false;
  store.selectedSize = 0;
  store.selectionAnchor = null;
}

/** Selects every row already cached for the current level. */
export function selectAllLoaded(): void {
  if (store.filterPending) return;
  store.selectionGeneration += 1;
  const selected = new Set(store.selected);
  let selectedSize = store.selectedSize;
  for (const page of store.pages.values()) {
    for (const row of page) {
      if (!selected.has(row.path)) {
        selected.add(row.path);
        selectedSize += row.size;
      }
    }
  }
  store.selected = selected;
  store.selectedAllCurrentRows = selectionCoversLoadedCurrentRows(selected);
  store.selectedSize = selectedSize;
}

/** Selects the current directory or search result using bounded page batches. */
export async function selectAllRows(
  onProgress?: (loaded: number, total: number) => void,
): Promise<RowSelectionResult> {
  return selectRowsInRange(0, store.total - 1, false, null, onProgress);
}

/** Extends from the last directly selected row, including uncached pages. */
export async function selectRangeTo(
  row: EntryDto,
  index?: number,
  additive = false,
  onProgress?: (loaded: number, total: number) => void,
): Promise<RowSelectionResult> {
  if (store.filterPending) return "stale";
  const target = selectionRowIndex(row, index);
  if (target === null) return "stale";
  const anchor = store.selectionAnchor?.generation === store.generation
    ? store.selectionAnchor
    : { index: target, generation: store.generation };
  return selectRowsInRange(
    Math.min(anchor.index, target), Math.max(anchor.index, target), additive, anchor, onProgress,
  );
}

/** Publishes the complete selection atomically; fetched pages stay transient. */
async function selectRowsInRange(
  first: number,
  last: number,
  additive: boolean,
  anchor: SelectionAnchor | null,
  onProgress?: (loaded: number, total: number) => void,
): Promise<RowSelectionResult> {
  const info = store.info;
  if (!info || store.filterPending) return "stale";

  const generation = store.generation;
  const selectionGeneration = ++store.selectionGeneration;
  const query = store.filter.trim();
  const dirPrefix = currentPrefix();
  const total = store.total;
  const rangeCount = last - first + 1;
  const coversAll = (first === 0 && last === total - 1)
    || (additive && store.selectedAllCurrentRows);
  const selected = new Set<string>(additive ? store.selected : []);
  let selectedSize = additive ? store.selectedSize : 0;
  let loaded = 0;
  const isCurrent = () => generation === store.generation
    && selectionGeneration === store.selectionGeneration;

  const addRows = (rows: readonly EntryDto[]) => {
    for (const row of rows) {
      if (selected.has(row.path)) continue;
      selected.add(row.path);
      selectedSize += row.size;
    }
    loaded += rows.length;
    onProgress?.(loaded, rangeCount);
  };

  onProgress?.(0, rangeCount);
  if (!isCurrent()) return "stale";
  try {
    const previewRows = previewRowsForCurrentLevel();
    if (previewRows) {
      addRows(previewRows.slice(first, last + 1));
    } else {
      const lastPage = Math.floor(last / PAGE_SIZE);
      let cancelledSearch = false;
      for (let start = Math.floor(first / PAGE_SIZE); start <= lastPage; start += SELECTION_PAGE_CONCURRENCY) {
        if (!isCurrent()) return "stale";
        const pageNumbers = Array.from(
          { length: Math.min(SELECTION_PAGE_CONCURRENCY, lastPage - start + 1) },
          (_, index) => start + index,
        );
        if (!query && !cancelledSearch && pageNumbers.some((page) => !store.pages.has(page))) {
          await ipc.cancelArchiveSearch(info.id, generation);
          cancelledSearch = true;
          if (!isCurrent()) return "stale";
        }
        const pages = await Promise.all(
          pageNumbers.map(async (pageNumber) => {
            const cached = store.pages.get(pageNumber);
            if (cached) return { total, page: pageNumber, items: cached };
            if (query) {
              markValidationArchiveCall("searchEntries");
              return ipc.searchEntries(info.id, pageNumber, query, PAGE_SIZE, generation);
            }
            markValidationArchiveCall("listEntries");
            return ipc.listEntries(info.id, pageNumber, dirPrefix, null, PAGE_SIZE);
          }),
        );
        if (!isCurrent()) return "stale";
        for (let index = 0; index < pages.length; index += 1) {
          const page = pages[index];
          if (page === null || page.total !== total) return "stale";
          const offset = pageNumbers[index] * PAGE_SIZE;
          const startOffset = Math.max(first - offset, 0);
          const endOffset = Math.min(last + 1 - offset, PAGE_SIZE, total - offset);
          const pageRows = page.items.slice(startOffset, endOffset);
          if (pageRows.length !== endOffset - startOffset) return "failed";
          addRows(pageRows);
          if (!isCurrent()) return "stale";
        }
      }
    }
  } catch {
    return isCurrent() ? "failed" : "stale";
  }

  if (!isCurrent()) return "stale";
  if (loaded !== rangeCount) return "failed";
  store.selected = selected;
  store.selectedAllCurrentRows = coversAll || selectionCoversLoadedCurrentRows(selected);
  store.selectedSize = selectedSize;
  store.selectionAnchor = anchor;
  store.selectionGeneration += 1;
  return "selected";
}

/* ---- Recent files (frontend-local, max 5) ---- */

const RECENT_KEY = "squallz.recent";

export function recentFiles(): string[] {
  try {
    const raw = localStorage.getItem(RECENT_KEY);
    return raw ? (JSON.parse(raw) as string[]) : [];
  } catch {
    return [];
  }
}

export function rememberRecent(path: string): void {
  const list = recentFiles().filter((p) => p !== path);
  list.unshift(path);
  localStorage.setItem(RECENT_KEY, JSON.stringify(list.slice(0, 5)));
}

export function installArchivePreview(
  info: ArchiveInfo,
  rows: EntryDto[],
  options?: {
    dirs?: string[];
    selected?: string[];
    selectedSize?: number;
    filter?: string;
    total?: number;
    pages?: Map<number, EntryDto[]>;
    previewRows?: EntryDto[];
  },
): void {
  clearArchivePasswordSave();
  store.openGeneration += 1;
  installValidationArchiveCallCounters();
  store.info = info;
  store.dirs = options?.dirs ?? [];
  store.total = options?.total ?? rows.length;
  store.pages = options?.pages ? new Map(options.pages) : new Map([[0, rows]]);
  store.loading = new Set();
  cancelFilterReload();
  store.filter = options?.filter ?? "";
  store.filterPending = false;
  store.selected = new Set(options?.selected ?? []);
  store.selectedAllCurrentRows = false;
  store.selectionGeneration += 1;
  store.selectionAnchor = null;
  store.previewRows = options?.previewRows ?? null;
  clearBrowseError();
  clearArchiveRefreshStatus();
  store.selectedSize =
    options?.selectedSize ??
    rows
      .filter((row) => store.selected.has(row.path))
      .reduce((sum, row) => sum + row.size, 0);
  store.selectedAllCurrentRows = selectionCoversLoadedCurrentRows(store.selected);
  store.generation += 1;
  store.encodingOverride = info.encoding_override;
  store.passwordPrompt = null;
  store.openError = null;
  store.passwordBookGeneration += 1;
  store.passwordBookState = "ready";
  store.passwordBookAvailable = true;
  store.passwordBookSaved = false;
  store.passwordBookSession = false;
}
