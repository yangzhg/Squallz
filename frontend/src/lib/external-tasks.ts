import { basename, dirname } from "./format";
import type { JobSpec } from "./ipc";

export const externalOpenActions = [
  "checksum",
  "extract-here",
  "extract-to-folder",
  "compress-to-7z",
  "test-archive",
] as const;

export type ExternalOpenAction = (typeof externalOpenActions)[number];
export type ArchiveStemName = (name: string) => string;

export interface ExternalOpenActionCopy {
  labelKey: string;
  fallbackLabel: string;
}

export interface ExternalTaskJobOptions {
  paths: readonly unknown[];
  output: string | null;
  checksumAlgorithm: string;
  checksumExcludes: string[];
  archiveStemName?: ArchiveStemName;
}

const externalArchiveExtensions = [
  "tar.zst",
  "tar.gz",
  "tar.xz",
  "tar.bz2",
  "zip",
  "jar",
  "apk",
  "cbz",
  "ipa",
  "7z",
  "sqz",
  "rar",
  "cbr",
  "wim",
  "dmg",
  "iso",
  "tar",
  "tgz",
  "tbz2",
  "txz",
  "tzst",
];

const externalOpenActionCopyByAction: Record<ExternalOpenAction, ExternalOpenActionCopy> = {
  checksum: {
    labelKey: "gui.settings.integration.context_action.checksum",
    fallbackLabel: "Checksum",
  },
  "extract-here": {
    labelKey: "gui.settings.integration.context_action.extract_here",
    fallbackLabel: "Extract Here",
  },
  "extract-to-folder": {
    labelKey: "gui.settings.integration.context_action.extract_to_archive",
    fallbackLabel: "Extract to <archive>/",
  },
  "compress-to-7z": {
    labelKey: "gui.settings.integration.context_action.compress_to_7z",
    fallbackLabel: "Compress to 7Z",
  },
  "test-archive": {
    labelKey: "gui.settings.integration.context_action.test_archive",
    fallbackLabel: "Test archive",
  },
};

export function externalOpenAction(value: string | null | undefined): ExternalOpenAction | null {
  return externalOpenActions.includes(value as ExternalOpenAction) ? (value as ExternalOpenAction) : null;
}

export function isExternalOpenAction(value: string | null | undefined): value is ExternalOpenAction {
  return externalOpenAction(value) !== null;
}

export function externalOpenActionCopy(action: ExternalOpenAction): ExternalOpenActionCopy {
  return externalOpenActionCopyByAction[action];
}

export function normalizeExternalTaskPaths(paths: readonly unknown[]): string[] {
  return paths.filter((item): item is string => typeof item === "string" && item.length > 0);
}

export function defaultExternalArchiveStemName(name = "archive"): string {
  const unsplit = name.replace(/\.\d{3,}$/i, "");
  const lower = unsplit.toLowerCase().trimEnd();
  const extension = externalArchiveExtensions.find((item) => lower.endsWith(`.${item}`));
  if (extension) return unsplit.slice(0, -(extension.length + 1));
  const dot = unsplit.lastIndexOf(".");
  return dot > 0 ? unsplit.slice(0, dot) : unsplit;
}

export function externalExtractDest(
  action: Extract<ExternalOpenAction, "extract-here" | "extract-to-folder">,
  path: string,
  stemName: ArchiveStemName = defaultExternalArchiveStemName,
): string {
  const parent = dirname(path);
  if (action === "extract-here") return parent;
  return joinPath(parent, stemName(basename(path)));
}

export function externalCompressOutput(
  paths: readonly string[],
  output: string | null,
  stemName: ArchiveStemName = defaultExternalArchiveStemName,
): string {
  const requestedOutput = output?.trim();
  if (requestedOutput) return requestedOutput;
  const first = paths[0] ?? "Archive";
  const parent = dirname(first);
  const base = paths.length === 1 ? stemName(basename(first)) : "Archive";
  return joinPath(parent, `${base}.7z`);
}

export function buildExternalTaskJobSpec(
  action: ExternalOpenAction,
  options: ExternalTaskJobOptions,
): JobSpec | null {
  const validPaths = normalizeExternalTaskPaths(options.paths);
  const firstPath = validPaths[0];
  if (!firstPath) return null;
  const stemName = options.archiveStemName ?? defaultExternalArchiveStemName;
  if (action === "checksum") {
    return {
      kind: "checksum",
      inputs: validPaths,
      excludes: options.checksumExcludes,
      algorithm: options.checksumAlgorithm,
    };
  }
  if (action === "extract-here" || action === "extract-to-folder") {
    return extractJobSpec(action, validPaths, stemName);
  }
  if (action === "compress-to-7z") {
    return {
      kind: "compress",
      inputs: validPaths,
      dest: externalCompressOutput(validPaths, options.output, stemName),
      level: 5,
      password: null,
      encrypt_names: false,
      split_size: null,
      excludes: [],
    };
  }
  return {
    kind: "test",
    path: firstPath,
    encoding: null,
    password: null,
  };
}

function extractJobSpec(
  action: Extract<ExternalOpenAction, "extract-here" | "extract-to-folder">,
  paths: string[],
  stemName: ArchiveStemName,
): JobSpec {
  if (paths.length === 1) {
    const path = paths[0] ?? "";
    return {
      kind: "extract",
      path,
      dest: externalExtractDest(action, path, stemName),
      selection: null,
      overwrite: "ask",
      symlinks: "preserve",
      smart: true,
      encoding: null,
      password: null,
      best_effort: false,
    };
  }
  return {
    kind: "batch_extract",
    items: paths.map((path) => ({
      path,
      dest: externalExtractDest(action, path, stemName),
      encoding: null,
      password: null,
      best_effort: false,
    })),
    overwrite: "ask",
    symlinks: "preserve",
    smart: true,
  };
}

function joinPath(parent: string, child: string): string {
  if (parent === "/" || parent === "") return `${parent}${child}`;
  return `${parent}/${child}`;
}
