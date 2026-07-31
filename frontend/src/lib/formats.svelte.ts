// Format capability cache (drives the compress dialog and drop-type
// detection). Loaded once at startup from `get_formats`.

import {
  archiveNameWithoutVolumeSuffix,
  isLegacyRarVolumeName,
  isNativeSplitZipVolumeName,
} from "./archive-names";
import { ipc, type FormatDto } from "./ipc";

const store = $state({
  all: [] as FormatDto[],
  extensions: new Set<string>(),
});

function installFormats(formats: FormatDto[]): void {
  store.all = formats;
  const exts = new Set<string>();
  for (const f of store.all) {
    for (const e of f.extensions) exts.add(e.toLowerCase());
  }
  store.extensions = exts;
}

/** Loads the registry once. */
export async function loadFormats(): Promise<void> {
  installFormats(await ipc.getFormats());
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
