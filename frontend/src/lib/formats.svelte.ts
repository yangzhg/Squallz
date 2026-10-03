// Registered format capabilities for editing, creation and drop-type detection.

import {
  archiveNameWithoutVolumeSuffix,
  isLegacyRarVolumeName,
  isNativeSplitZipVolumeName,
} from "./archive-names";
import { ipc, type FormatDto } from "./ipc";

export type FormatRegistryStatus = "loading" | "ready" | "error";

const store = $state({
  all: [] as FormatDto[],
  extensions: new Set<string>(),
  status: "loading" as FormatRegistryStatus,
});
let loadingRequest: Promise<void> | null = null;

function installFormats(formats: FormatDto[]): void {
  if (!Array.isArray(formats) || formats.length === 0) {
    throw new Error("Format capabilities are unavailable");
  }
  const ids = new Set<string>();
  const exts = new Set<string>();
  for (const f of formats) {
    if (!f || typeof f.id !== "string" || !f.id || ids.has(f.id)
      || typeof f.can_update !== "boolean") {
      throw new Error("Format capabilities are invalid");
    }
    ids.add(f.id);
    for (const e of f.extensions) exts.add(e.toLowerCase());
  }
  store.all = formats;
  store.extensions = exts;
}

/** Shares an in-flight read and keeps successful capabilities cached; failures can retry. */
export function loadFormats(): Promise<void> {
  if (loadingRequest) return loadingRequest;
  if (store.status === "ready") return Promise.resolve();
  store.status = "loading";
  const request = Promise.resolve().then(() => ipc.getFormats()).then((formats) => {
    installFormats(formats);
    store.status = "ready";
  }).catch((error: unknown) => {
    store.status = "error";
    throw error;
  }).finally(() => {
    if (loadingRequest === request) loadingRequest = null;
  });
  loadingRequest = request;
  return request;
}

export function formatRegistryStatus(): FormatRegistryStatus {
  return store.status;
}

export function allFormats(): FormatDto[] {
  return store.all;
}

/** Formats offered by the compress dialog (`can_create` archives). */
export function creatableFormats(): FormatDto[] {
  return store.all.filter((f) => f.kind === "archive" && f.can_create);
}

/** Whether a local path looks like an archive we can open. */
export function isArchivePath(path: string): boolean {
  const name = path.split("/").pop()?.toLowerCase() ?? "";
  if (isLegacyRarVolumeName(name) || isNativeSplitZipVolumeName(name)) return true;
  const unsplit = archiveNameWithoutVolumeSuffix(name);
  for (const ext of store.extensions) {
    if (unsplit.endsWith(`.${ext}`)) return true;
  }
  return false;
}

/** Whether a format id has a meaningful compression level (plain tar
 * containers do not. */
export function hasLevel(id: string): boolean {
  return id !== "tar";
}
