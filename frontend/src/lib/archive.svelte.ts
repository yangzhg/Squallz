// Archive browsing store: open/close, breadcrumb navigation, 500-per-page
// cached pagination for the virtual list, selection and archive-wide path search.
// Shared archive browse state for the desktop UI.

import {
  ipc,
  isErrorDto,
  type ArchiveInfo,
  type EntryDto,
  type ErrorDto,
} from "./ipc";
import { t, tError } from "./i18n.svelte";
import { pushToast, removeToastByKey } from "./toasts.svelte";

export const PAGE_SIZE = 500;
export type PasswordBookStatusState = "idle" | "checking" | "ready" | "error";
export type SelectAllRowsResult = "selected" | "stale" | "failed";
const ARCHIVE_BROWSE_ERROR_TOAST_KEY = "archive-browse-error";
const SELECT_ALL_PAGE_CONCURRENCY = 4;

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
  /** Invalidates a pending full-selection request after any later selection change. */
  selectionGeneration: 0,
  /** Dev preview-only full row tree used to exercise folder navigation without IPC. */
  previewRows: null as EntryDto[] | null,
  selectedSize: 0,
  /** Bumped on every navigation/filter change to drop stale responses */
  generation: 0,
  /** Most recent failure while listing a directory or searching the archive. */
  browseError: null as ErrorDto | null,
  /** The current archive was opened with a user-entered password. */
  sessionPasswordKnown: false,
  /** User-selected archive-wide file-name encoding. */
  encodingOverride: null as string | null,
  /** Pending open that needs a password (drives the password dialog) */
  passwordPrompt: null as { path: string; wrong: boolean; encoding: string | null } | null,
  /** Most recent structured non-password failure, bound to the attempted path. */
  openError: null as { path: string; error: ErrorDto } | null,
  /** Invalidates superseded open requests before they can publish state. */
  openGeneration: 0,
  passwordBookGeneration: 0,
  passwordBookState: "idle" as PasswordBookStatusState,
  passwordBookAvailable: false,
  passwordBookSaved: false,
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

/** Whether the current UI session knows this archive used a password. */
export function archiveHasSessionPassword(): boolean {
  return store.sessionPasswordKnown;
}

export function archivePasswordBookStatus(): {
  state: PasswordBookStatusState;
  available: boolean;
  saved: boolean;
} {
  return {
    state: store.passwordBookState,
    available: store.passwordBookAvailable,
    saved: store.passwordBookSaved,
  };
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
  cancelActiveArchiveOpenRequest();
  const requestId = nextArchiveOpenRequestId();
  pendingArchiveOpenRequestId = requestId;
  const requestGeneration = ++store.openGeneration;
  const retryingPrompt = store.passwordPrompt?.path === path;
  if (!retryingPrompt) store.passwordPrompt = null;
  store.openError = null;
  let pendingInfo: ArchiveInfo | null = null;
  try {
    const hadSessionPassword =
      store.info?.source === path && store.sessionPasswordKnown;
    markValidationArchiveCall("openArchive");
    const info = await ipc.openArchive(
      path,
      password ?? null,
      encoding ?? null,
      requestId,
    );
    pendingInfo = info;
    if (requestGeneration !== store.openGeneration) {
      void ipc.closeArchive(info.id);
      pendingInfo = null;
      return false;
    }
    markValidationArchiveCall("listEntries");
    const page = await ipc.listEntries(info.id, 0, "", null, PAGE_SIZE);
    if (requestGeneration !== store.openGeneration) {
      void ipc.closeArchive(info.id);
      pendingInfo = null;
      return false;
    }
    if (store.info) void ipc.closeArchive(store.info.id);
    store.info = info;
    store.dirs = [];
    cancelFilterReload();
    store.filter = "";
    store.filterPending = false;
    store.previewRows = null;
    clearBrowseError();
    store.generation += 1;
    store.pages = new Map([[0, page.items]]);
    store.loading = new Set();
    store.total = page.total;
    store.sessionPasswordKnown = password != null || hadSessionPassword;
    store.encodingOverride = info.encoding_override ?? encoding ?? null;
    store.passwordPrompt = null;
    store.openError = null;
    clearPasswordBookStatus();
    clearSelection();
    pendingInfo = null;
    if (!info.read_only) refreshArchivePasswordBookStatusInBackground(info.path);
    return true;
  } catch (e) {
    if (pendingInfo) void ipc.closeArchive(pendingInfo.id);
    if (requestGeneration !== store.openGeneration) return false;
    if (isErrorDto(e)) {
      if (e.key === "error.password_required" || e.key === "error.wrong_password") {
        store.openError = null;
        store.passwordPrompt = {
          path,
          wrong: e.key === "error.wrong_password" || password != null,
          encoding: encoding ?? null,
        };
        return false;
      }
      store.passwordPrompt = null;
      store.openError = { path, error: e };
      pushToast({ kind: "danger", title: tError(e) });
    } else {
      store.passwordPrompt = null;
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

/** Adopts an archive that was already opened by an archive command. */
export async function adoptOpenedArchive(info: ArchiveInfo): Promise<boolean> {
  cancelActiveArchiveOpenRequest();
  const requestGeneration = ++store.openGeneration;
  let page: Awaited<ReturnType<typeof ipc.listEntries>>;
  try {
    markValidationArchiveCall("listEntries");
    page = await ipc.listEntries(info.id, 0, "", null, PAGE_SIZE);
  } catch (error) {
    void ipc.closeArchive(info.id);
    throw error;
  }
  if (requestGeneration !== store.openGeneration) {
    void ipc.closeArchive(info.id);
    return false;
  }
  if (store.info && store.info.id !== info.id) void ipc.closeArchive(store.info.id);
  store.info = info;
  store.dirs = [];
  cancelFilterReload();
  store.filter = "";
  store.filterPending = false;
  store.previewRows = null;
  clearBrowseError();
  store.generation += 1;
  store.pages = new Map([[0, page.items]]);
  store.loading = new Set();
  store.total = page.total;
  store.sessionPasswordKnown = false;
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
  store.sessionPasswordKnown = false;
  store.encodingOverride = null;
  store.passwordPrompt = null;
  store.openError = null;
  clearPasswordBookStatus();
  clearSelection();
}

function clearPasswordBookStatus(): void {
  store.passwordBookGeneration += 1;
  store.passwordBookState = "idle";
  store.passwordBookAvailable = false;
  store.passwordBookSaved = false;
}

/** Reopens the current archive with a user-selected file-name encoding. */
export async function reopenWithEncoding(encoding: string | null): Promise<boolean> {
  const current = store.info;
  if (!current) return false;
  const dirs = [...store.dirs];
  const filter = store.filter;
  const ok = await openArchive(current.source, null, encoding);
  if (!ok) return false;
  store.dirs = dirs;
  store.filter = filter;
  await reload();
  clearSelection();
  return true;
}

/** Reopens the current archive after an in-place update and refreshes rows. */
export async function refreshCurrentArchive(): Promise<boolean> {
  const current = store.info;
  if (!current) return false;
  const dirs = [...store.dirs];
  const filter = store.filter;
  const ok = await openArchive(current.source, null, store.encodingOverride);
  if (!ok) return false;
  store.dirs = dirs;
  store.filter = filter;
  await reload();
  clearSelection();
  return true;
}

export async function refreshArchivePasswordBookStatus(path = store.info?.path): Promise<void> {
  if (!path || store.info?.read_only) {
    clearPasswordBookStatus();
    return;
  }
  if (store.info?.path !== path) return;
  const generation = ++store.passwordBookGeneration;
  store.passwordBookState = "checking";
  try {
    const status = await ipc.archivePasswordStatus(path);
    if (store.info?.path !== path || store.passwordBookGeneration !== generation) return;
    store.passwordBookAvailable = status.available;
    store.passwordBookSaved = status.saved;
    store.passwordBookState = "ready";
  } catch (error) {
    if (store.info?.path === path && store.passwordBookGeneration === generation) {
      store.passwordBookState = "error";
    }
    throw error;
  }
}

function refreshArchivePasswordBookStatusInBackground(path: string): void {
  void refreshArchivePasswordBookStatus(path).catch(() => undefined);
}

export async function rememberArchivePassword(
  path: string,
  password: string,
  encoding?: string | null,
): Promise<boolean> {
  try {
    const status = await ipc.rememberArchivePassword(path, password, encoding ?? null);
    if (store.info?.path === path) {
      store.passwordBookGeneration += 1;
      store.passwordBookAvailable = status.available;
      store.passwordBookSaved = status.saved;
      store.passwordBookState = "ready";
    }
    pushToast({ kind: "success", title: t("gui.password.saved") });
    return true;
  } catch (e) {
    if (isErrorDto(e)) {
      pushToast({ kind: "danger", title: tError(e), detail: e.detail });
    } else {
      pushToast({ kind: "danger", title: String(e) });
    }
    return false;
  }
}

export async function forgetCurrentArchivePassword(): Promise<boolean> {
  const path = store.info?.path;
  if (!path || store.info?.read_only) return false;
  try {
    const status = await ipc.forgetArchivePassword(path);
    if (store.info?.path === path) {
      store.sessionPasswordKnown = false;
      store.passwordBookGeneration += 1;
      store.passwordBookAvailable = status.available;
      store.passwordBookSaved = status.saved;
      store.passwordBookState = "ready";
    }
    pushToast({ kind: "success", title: t("gui.password.forgotten") });
    return true;
  } catch (e) {
    if (isErrorDto(e)) {
      pushToast({ kind: "danger", title: tError(e), detail: e.detail });
    } else {
      pushToast({ kind: "danger", title: String(e) });
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
    const page = query
      ? await searchArchivePage(store.info.id, 0, query, generation)
      : await listArchiveLevelPage(store.info.id, 0, generation);
    if (generation !== store.generation || page === null) return;
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
  store.dirs = store.dirs.slice(0, level + 1);
  cancelFilterReload();
  store.filter = "";
  store.filterPending = false;
  await reload();
}

/** Goes one level up (Cmd+↑). */
export async function goUp(): Promise<void> {
  if (store.dirs.length === 0) return;
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

export function toggleSelect(row: EntryDto): void {
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
}

export function clearSelection(): void {
  store.selectionGeneration += 1;
  store.selected = new Set();
  store.selectedAllCurrentRows = false;
  store.selectedSize = 0;
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

/**
 * Selects the complete current directory or archive-wide search result without
 * retaining every fetched page in the WebView.
 */
export async function selectAllRows(
  onProgress?: (loaded: number, total: number) => void,
): Promise<SelectAllRowsResult> {
  const info = store.info;
  if (!info || store.filterPending) return "stale";

  const generation = store.generation;
  const selectionGeneration = store.selectionGeneration;
  const query = store.filter.trim();
  const dirPrefix = currentPrefix();
  const total = store.total;
  const selected = new Set<string>();
  let selectedSize = 0;
  let loaded = 0;

  const addRows = (rows: readonly EntryDto[]) => {
    for (const row of rows) {
      if (selected.has(row.path)) continue;
      selected.add(row.path);
      selectedSize += row.size;
    }
    loaded += rows.length;
    onProgress?.(Math.min(loaded, total), total);
  };

  const previewRows = previewRowsForCurrentLevel();
  if (previewRows) {
    if (
      generation !== store.generation
      || selectionGeneration !== store.selectionGeneration
    ) return "stale";
    addRows(previewRows);
    store.selected = selected;
    store.selectedAllCurrentRows = true;
    store.selectedSize = selectedSize;
    store.selectionGeneration += 1;
    return "selected";
  }

  const pageCount = Math.ceil(total / PAGE_SIZE);
  try {
    if (!query) {
      await ipc.cancelArchiveSearch(info.id, generation);
    }
    for (let start = 0; start < pageCount; start += SELECT_ALL_PAGE_CONCURRENCY) {
      const pageNumbers = Array.from(
        { length: Math.min(SELECT_ALL_PAGE_CONCURRENCY, pageCount - start) },
        (_, index) => start + index,
      );
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
      if (
        generation !== store.generation
        || selectionGeneration !== store.selectionGeneration
      ) return "stale";
      for (const page of pages) {
        if (page === null || page.total !== total) return "stale";
        addRows(page.items);
      }
    }
  } catch {
    return generation === store.generation
      && selectionGeneration === store.selectionGeneration
      ? "failed"
      : "stale";
  }

  if (
    generation !== store.generation
    || selectionGeneration !== store.selectionGeneration
    || loaded < total
  ) return "stale";
  store.selected = selected;
  store.selectedAllCurrentRows = true;
  store.selectedSize = selectedSize;
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
  store.previewRows = options?.previewRows ?? null;
  clearBrowseError();
  store.selectedSize =
    options?.selectedSize ??
    rows
      .filter((row) => store.selected.has(row.path))
      .reduce((sum, row) => sum + row.size, 0);
  store.selectedAllCurrentRows = selectionCoversLoadedCurrentRows(store.selected);
  store.generation += 1;
  store.sessionPasswordKnown = false;
  store.encodingOverride = info.encoding_override;
  store.passwordPrompt = null;
  store.openError = null;
  store.passwordBookGeneration += 1;
  store.passwordBookState = "ready";
  store.passwordBookAvailable = true;
  store.passwordBookSaved = false;
}
