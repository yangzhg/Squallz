// Archive browsing store: open/close, breadcrumb navigation, 500-per-page
// cached pagination for the virtual list, selection and filtering
// Shared archive browse state for the desktop UI.

import {
  ipc,
  isErrorDto,
  type ArchiveInfo,
  type EntryDto,
} from "./ipc";
import { t, tError } from "./i18n.svelte";
import { pushToast } from "./toasts.svelte";

export const PAGE_SIZE = 500;

type ValidationArchiveCallKind = "openArchive" | "listEntries";
type ValidationArchiveCallCounters = Record<ValidationArchiveCallKind, number>;
type ValidationArchiveCallWindow = Window & {
  __squallzValidationArchiveCalls?: ValidationArchiveCallCounters;
  __squallzValidationArchiveCallSnapshot?: () => ValidationArchiveCallCounters;
  __squallzResetValidationArchiveCalls?: () => ValidationArchiveCallCounters;
};

function emptyValidationArchiveCallCounters(): ValidationArchiveCallCounters {
  return { openArchive: 0, listEntries: 0 };
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
  /** Selected full paths (dirs end with `/`) */
  selected: new Set<string>(),
  /** Dev preview-only full row tree used to exercise folder navigation without IPC. */
  previewRows: null as EntryDto[] | null,
  selectedSize: 0,
  /** Bumped on every navigation/filter change to drop stale responses */
  generation: 0,
  /** The current archive was opened with a user-entered password. */
  sessionPasswordKnown: false,
  /** User-selected archive-wide file-name encoding. */
  encodingOverride: null as string | null,
  /** Pending open that needs a password (drives the password dialog) */
  passwordPrompt: null as { path: string; wrong: boolean; encoding: string | null } | null,
  passwordBookAvailable: false,
  passwordBookSaved: false,
});

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

export function loadedRowCount(): number {
  let count = 0;
  for (const rows of store.pages.values()) count += rows.length;
  return count;
}

export function allRowsLoaded(): boolean {
  return store.total === 0 || loadedRowCount() >= store.total;
}

export function filterText(): string {
  return store.filter;
}

export function selectedPaths(): Set<string> {
  return store.selected;
}

export function selectedSize(): number {
  return store.selectedSize;
}

export function openPasswordPrompt(): { path: string; wrong: boolean; encoding: string | null } | null {
  return store.passwordPrompt;
}

/** Whether the current UI session knows this archive used a password. */
export function archiveHasSessionPassword(): boolean {
  return store.sessionPasswordKnown;
}

export function archivePasswordBookStatus(): { available: boolean; saved: boolean } {
  return {
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
  try {
    const hadSessionPassword =
      store.info?.path === path && store.sessionPasswordKnown;
    markValidationArchiveCall("openArchive");
    const info = await ipc.openArchive(path, password ?? null, encoding ?? null);
    if (store.info) void ipc.closeArchive(store.info.id);
    store.info = info;
    store.dirs = [];
    store.filter = "";
    store.previewRows = null;
    store.sessionPasswordKnown = password != null || hadSessionPassword;
    store.encodingOverride = info.encoding_override ?? encoding ?? null;
    store.passwordPrompt = null;
    clearPasswordBookStatus();
    clearSelection();
    await reload();
    refreshArchivePasswordBookStatusInBackground(path);
    return true;
  } catch (e) {
    if (isErrorDto(e)) {
      if (e.key === "error.password_required" || e.key === "error.wrong_password") {
        store.passwordPrompt = {
          path,
          wrong: e.key === "error.wrong_password" || password != null,
          encoding: encoding ?? null,
        };
        return false;
      }
      pushToast({ kind: "danger", title: tError(e), detail: e.detail });
    } else {
      pushToast({ kind: "danger", title: String(e) });
    }
    return false;
  }
}

/** Adopts an archive that was already opened by an archive command. */
export async function adoptOpenedArchive(info: ArchiveInfo): Promise<void> {
  if (store.info && store.info.id !== info.id) void ipc.closeArchive(store.info.id);
  store.info = info;
  store.dirs = [];
  store.filter = "";
  store.previewRows = null;
  store.sessionPasswordKnown = false;
  store.encodingOverride = info.encoding_override ?? null;
  store.passwordPrompt = null;
  clearPasswordBookStatus();
  clearSelection();
  await reload();
  refreshArchivePasswordBookStatusInBackground(info.path);
}

/** Dismisses the open-time password prompt. */
export function cancelPasswordPrompt(): void {
  store.passwordPrompt = null;
}

export function closeArchive(): void {
  if (store.info) void ipc.closeArchive(store.info.id);
  store.info = null;
  store.dirs = [];
  store.pages = new Map();
  store.total = 0;
  store.previewRows = null;
  store.sessionPasswordKnown = false;
  store.encodingOverride = null;
  store.passwordBookAvailable = false;
  store.passwordBookSaved = false;
  clearSelection();
}

function clearPasswordBookStatus(): void {
  store.passwordBookAvailable = false;
  store.passwordBookSaved = false;
}

/** Reopens the current archive with a user-selected file-name encoding. */
export async function reopenWithEncoding(encoding: string | null): Promise<boolean> {
  const current = store.info;
  if (!current) return false;
  const dirs = [...store.dirs];
  const filter = store.filter;
  const ok = await openArchive(current.path, null, encoding);
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
  const ok = await openArchive(current.path, null, store.encodingOverride);
  if (!ok) return false;
  store.dirs = dirs;
  store.filter = filter;
  await reload();
  clearSelection();
  return true;
}

export async function refreshArchivePasswordBookStatus(path = store.info?.path): Promise<void> {
  if (!path) {
    clearPasswordBookStatus();
    return;
  }
  const status = await ipc.archivePasswordStatus(path);
  if (store.info?.path !== path) return;
  store.passwordBookAvailable = status.available;
  store.passwordBookSaved = status.saved;
}

function refreshArchivePasswordBookStatusInBackground(path: string): void {
  void refreshArchivePasswordBookStatus(path).catch(() => {
    if (store.info?.path === path) clearPasswordBookStatus();
  });
}

export async function rememberArchivePassword(
  path: string,
  password: string,
  encoding?: string | null,
): Promise<boolean> {
  try {
    const status = await ipc.rememberArchivePassword(path, password, encoding ?? null);
    if (store.info?.path === path) {
      store.passwordBookAvailable = status.available;
      store.passwordBookSaved = status.saved;
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
  if (!path) return false;
  try {
    const status = await ipc.forgetArchivePassword(path);
    store.sessionPasswordKnown = false;
    store.passwordBookAvailable = status.available;
    store.passwordBookSaved = status.saved;
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
  if (!store.info) return;
  const generation = ++store.generation;
  store.pages = new Map();
  store.loading = new Set();
  const previewRows = previewRowsForCurrentLevel();
  if (previewRows) {
    if (generation !== store.generation) return;
    store.pages = new Map([[0, previewRows.slice(0, PAGE_SIZE)]]);
    store.total = previewRows.length;
    return;
  }
  markValidationArchiveCall("listEntries");
  const page = await ipc.listEntries(
    store.info.id,
    0,
    currentPrefix(),
    store.filter || null,
    PAGE_SIZE,
  );
  if (generation !== store.generation) return;
  store.pages = new Map([[0, page.items]]);
  store.total = page.total;
}

/** Returns a row by absolute index, fetching its page on demand. */
export function rowAt(index: number): EntryDto | null {
  const pageNo = Math.floor(index / PAGE_SIZE);
  const page = store.pages.get(pageNo);
  if (page) return page[index % PAGE_SIZE] ?? null;
  void fetchPage(pageNo);
  return null;
}

/** Prefetches `count` pages starting at the one containing `index`. */
export function prefetchAround(index: number, count = 2): void {
  const pageNo = Math.floor(index / PAGE_SIZE);
  for (let p = pageNo; p <= pageNo + count; p++) {
    if (p * PAGE_SIZE < Math.max(store.total, 1)) void fetchPage(p);
  }
}

async function fetchPage(pageNo: number): Promise<void> {
  if (!store.info) return;
  if (store.pages.has(pageNo) || store.loading.has(pageNo)) return;
  store.loading.add(pageNo);
  const generation = store.generation;
  try {
    const previewRows = previewRowsForCurrentLevel();
    if (previewRows) {
      if (generation !== store.generation) return;
      const pages = new Map(store.pages);
      pages.set(pageNo, previewRows.slice(pageNo * PAGE_SIZE, (pageNo + 1) * PAGE_SIZE));
      store.pages = pages;
      store.total = previewRows.length;
      return;
    }
    markValidationArchiveCall("listEntries");
    const page = await ipc.listEntries(
      store.info.id,
      pageNo,
      currentPrefix(),
      store.filter || null,
      PAGE_SIZE,
    );
    if (generation !== store.generation) return;
    const pages = new Map(store.pages);
    pages.set(pageNo, page.items);
    store.pages = pages;
    store.total = page.total;
  } finally {
    store.loading.delete(pageNo);
  }
}

function previewRowsForCurrentLevel(): EntryDto[] | null {
  if (!import.meta.env.DEV || !store.previewRows) return null;
  const prefix = currentPrefix();
  const filter = store.filter.trim().toLowerCase();
  return store.previewRows.filter((row) => {
    if (!row.path.startsWith(prefix)) return false;
    const remainder = row.path.slice(prefix.length);
    if (!remainder) return false;
    const visibleName = row.entry_type === "dir" ? remainder.replace(/\/+$/g, "") : remainder;
    if (!visibleName || visibleName.includes("/")) return false;
    if (!filter) return true;
    return row.display.toLowerCase().includes(filter) || row.path.toLowerCase().includes(filter);
  });
}

/** Enters a directory row. */
export async function enterDir(name: string): Promise<void> {
  store.dirs.push(name);
  store.filter = "";
  await reload();
}

/** Jumps to a breadcrumb level (`-1` = archive root). */
export async function gotoBreadcrumb(level: number): Promise<void> {
  store.dirs = store.dirs.slice(0, level + 1);
  store.filter = "";
  await reload();
}

/** Goes one level up (Cmd+↑). */
export async function goUp(): Promise<void> {
  if (store.dirs.length === 0) return;
  store.dirs.pop();
  await reload();
}

let filterTimer: ReturnType<typeof setTimeout> | undefined;

/** Sets the filter text with the 300 ms engine debounce. */
export function setFilter(text: string): void {
  store.filter = text;
  clearTimeout(filterTimer);
  filterTimer = setTimeout(() => void reload(), 300);
}

/* ---- Selection ---- */

export function toggleSelect(row: EntryDto): void {
  const selected = new Set(store.selected);
  if (selected.has(row.path)) {
    selected.delete(row.path);
    store.selectedSize -= row.size;
  } else {
    selected.add(row.path);
    store.selectedSize += row.size;
  }
  store.selected = selected;
}

export function clearSelection(): void {
  store.selected = new Set();
  store.selectedSize = 0;
}

/** Selects every loaded row of the current level (Cmd+A / header box). */
export function selectAllLoaded(): void {
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
  store.selectedSize = selectedSize;
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
  installValidationArchiveCallCounters();
  store.info = info;
  store.dirs = options?.dirs ?? [];
  store.total = options?.total ?? rows.length;
  store.pages = options?.pages ? new Map(options.pages) : new Map([[0, rows]]);
  store.loading = new Set();
  store.filter = options?.filter ?? "";
  store.selected = new Set(options?.selected ?? []);
  store.previewRows = options?.previewRows ?? null;
  store.selectedSize =
    options?.selectedSize ??
    rows
      .filter((row) => store.selected.has(row.path))
      .reduce((sum, row) => sum + row.size, 0);
  store.generation += 1;
  store.sessionPasswordKnown = false;
  store.encodingOverride = info.encoding_override;
  store.passwordPrompt = null;
  store.passwordBookAvailable = true;
  store.passwordBookSaved = false;
}
