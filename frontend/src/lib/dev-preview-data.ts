import type {
  ArchiveInfo,
  EntryDto,
  EntryPreviewDto,
  NestedArchivePreviewDto,
} from "./ipc";

export interface ArchivePreview {
  info: ArchiveInfo;
  rows: EntryDto[];
  previewRows?: EntryDto[];
  total: number;
  selected: string[];
  pages?: Map<number, EntryDto[]>;
  nestedPreview: NestedArchivePreviewDto | null;
}

export interface RuntimePreviews {
  archive: ArchivePreview | null;
  batchPaths: string[];
  checksumPath: string;
  checksumManifestPath: string;
  duplicateScanPath: string;
  duplicateMinSize: number;
  dropPaths: string[];
  preflightScanned: number;
  preflightCurrent: string;
  completedTask: "compress" | "extract" | "extract_unknown_current" | "batch_extract" | "test" | "checksum" | "checksum_check" | null;
  activeTask: "compress" | "extract" | "extract_unknown_current" | "batch_extract" | "test" | "checksum" | "checksum_check" | null;
  jobSubmitDelayMs: number;
}

const emptyRuntimePreviews: RuntimePreviews = {
  archive: null,
  batchPaths: [],
  checksumPath: "",
  checksumManifestPath: "",
  duplicateScanPath: "",
  duplicateMinSize: 1024 * 1024,
  dropPaths: [],
  preflightScanned: 0,
  preflightCurrent: "",
  completedTask: null,
  activeTask: null,
  jobSubmitDelayMs: 0,
};

const sampleArchiveRoot = "/Users/alex/Squallz Samples";

const archivePreviewEntries: EntryDto[] = [
  {
    path: "reports/",
    display: "reports",
    entry_type: "dir",
    size: 0,
    compressed: null,
    modified: 1781199120,
    crc: null,
    encrypted: false,
    encoding: "utf-8",
  },
  {
    path: "screenshots/",
    display: "screenshots",
    entry_type: "dir",
    size: 0,
    compressed: null,
    modified: 1781112720,
    crc: null,
    encrypted: false,
    encoding: "utf-8",
  },
  {
    path: "Launch plan.pdf",
    display: "Launch plan.pdf",
    entry_type: "file",
    size: 3_800_000,
    compressed: 2_400_000,
    modified: 1781194440,
    crc: 0xA91E22F8,
    encrypted: false,
    encoding: "utf-8",
  },
  {
    path: "cover-preview.png",
    display: "cover-preview.png",
    entry_type: "file",
    size: 4_096,
    compressed: 1_024,
    modified: 1781194500,
    crc: 0xC0A7BEEF,
    encrypted: false,
    encoding: "utf-8",
  },
  {
    path: "reports/Launch plan.pdf",
    display: "Existing launch copy.pdf",
    entry_type: "file",
    size: 3_600_000,
    compressed: 2_200_000,
    modified: 1781109000,
    crc: 0xA91E22F9,
    encrypted: false,
    encoding: "utf-8",
  },
  {
    path: "财务报表.xlsx",
    display: "财务报表.xlsx",
    entry_type: "file",
    size: 928_000,
    compressed: 312_000,
    modified: 1781105520,
    crc: 0xB12977AF,
    encrypted: false,
    encoding: "utf-8",
  },
  {
    path: "locked-secrets.7z",
    display: "locked-secrets.7z",
    entry_type: "file",
    size: 8_200_000,
    compressed: 7_900_000,
    modified: 1780932720,
    crc: 0x1987EF20,
    encrypted: true,
    encoding: "utf-8",
  },
];

const nestedPreviewItems: EntryDto[] = [
  {
    path: "inner-readme.txt",
    display: "inner-readme.txt",
    entry_type: "file",
    size: 1_024,
    compressed: 512,
    modified: 1781199120,
    crc: 0xAABBCCDD,
    encrypted: false,
    encoding: "utf-8",
  },
  {
    path: "vault/",
    display: "vault",
    entry_type: "dir",
    size: 0,
    compressed: null,
    modified: 1781199120,
    crc: null,
    encrypted: false,
    encoding: "utf-8",
  },
];

export function readRuntimePreviews(params: URLSearchParams, pageSize: number): RuntimePreviews {
  if (!import.meta.env.DEV) return emptyRuntimePreviews;

  const duplicateMinSize = numericParam(params, "duplicateMinSize", 1024 * 1024);
  const preflightScanned = numericParam(params, "previewPreflightScan", 0);
  const preflightCurrent = preflightScanned > 0
    ? params.get("previewPreflightCurrent") ?? "project/src/main.rs"
    : "";
  const completedTask = completedTaskParam(params.get("previewCompletedTask"));
  const activeTask = completedTaskParam(params.get("previewActiveTask"));
  const jobSubmitDelayMs = Math.max(0, Math.min(1200, numericParam(params, "previewJobSubmitDelayMs", 0)));

  return {
    archive: readArchivePreview(params, pageSize),
    batchPaths: listParam(params, "batchPaths", "|"),
    checksumPath: (params.get("checksumPath") ?? "").trim(),
    checksumManifestPath: (params.get("checksumManifest") ?? "").trim(),
    duplicateScanPath: (params.get("duplicateScanPath") ?? "").trim(),
    duplicateMinSize,
    dropPaths: listParam(params, "dropPaths", "|"),
    preflightScanned,
    preflightCurrent,
    completedTask,
    activeTask,
    jobSubmitDelayMs,
  };
}

function completedTaskParam(value: string | null): RuntimePreviews["completedTask"] {
  if (
    value === "compress" ||
    value === "extract" ||
    value === "extract_unknown_current" ||
    value === "batch_extract" ||
    value === "test" ||
    value === "checksum" ||
    value === "checksum_check"
  ) {
    return value;
  }
  return null;
}

function readArchivePreview(params: URLSearchParams, pageSize: number): ArchivePreview | null {
  if (params.get("previewArchive") !== "1") return null;

  const format = (params.get("previewFormat") ?? "zip").toLowerCase();
  const name = `product-backup.${format}`;
  const selected = listParam(params, "previewSelected", ",");
  const largeEntryCount = numericParam(params, "previewLargeEntries", 0);
  const pages = largeEntryCount > 0 ? largePreviewPages(largeEntryCount, pageSize) : null;
  const rows = pages?.get(0) ?? archivePreviewEntries;
  const previewRows = pages ? undefined : archivePreviewEntries;
  const total = largeEntryCount > 0 ? largeEntryCount : archivePreviewEntries.length;

  return {
    info: {
      id: 9_001,
      path: `${sampleArchiveRoot}/${name}`,
      name,
      format,
      entry_count: total,
      volumes: null,
      legacy_encoding_count: 0,
      garbled_count: 0,
      suggested_encoding: null,
      encoding_override: null,
    },
    rows,
    previewRows,
    total,
    selected,
    pages: pages ?? undefined,
    nestedPreview: params.get("previewNestedPreview") === "1"
      ? {
          outer_path: `${sampleArchiveRoot}/${name}`,
          entry_path: "locked-secrets.7z",
          format: "7z",
          entry_count: nestedPreviewItems.length,
          truncated: false,
          items: nestedPreviewItems,
        }
      : null,
  };
}

export function previewSampleForEntry(
  outerPath: string,
  entryPath: string,
): EntryPreviewDto | null {
  if (!import.meta.env.DEV || !outerPath.startsWith(`${sampleArchiveRoot}/`)) return null;

  if (entryPath === "cover-preview.png") {
    return {
      outer_path: outerPath,
      entry_path: entryPath,
      display_name: "cover-preview.png",
      temp_path: "/tmp/squallz-nested-cover-preview.png",
      size: 4_096,
      archive_like: false,
      preview_mime: "image/png",
      preview_data_url:
        "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAABAAAAAKCAYAAAC9vt6cAAABQUlEQVR4nBXMIU5EUQxA0VnKWCwWi8ViebLy4RqCaAKiYkQFJBWEVJBQgUCwv3L/WcA5ndffXK3fuV4/c7N6btfX3K2a+/UxDytH1ts8rpindZmX5XNZr/O+bD7X83wvndNZCIRACIRACIRACIRACIRACIRACIRACOQINsEm2ASbYBNsgk2wCTbBJtgEm2ATbIJNsI9ACZRACZRACZRACZRACZRACZRACZRACfQIjMAIjMAIjMAIjMAIjMAIjMAIjMAIjMCOwAmcwAmcwAmcwAmcwAmcwAmcwAmcwAn8CIIgCIIgCIIgCIIgCIIgCIIgCIIgCIIgjiAJkiAJkiAJkiAJkiAJkiAJkiAJkiAJ8giKoAiKoAiKoAiKoAiKoAiKoAiKoAiKoI6gCZqgCZqgCZqgCZqgCZqgCZqgCZqgCVrnHw41jeA2BOxvAAAAAElFTkSuQmCC",
    };
  }

  if (entryPath === "Launch plan.pdf") {
    return {
      outer_path: outerPath,
      entry_path: entryPath,
      display_name: "Launch plan.pdf",
      temp_path: "/tmp/squallz-nested-launch-plan.pdf",
      size: 3_800_000,
      archive_like: false,
      preview_mime: null,
      preview_data_url: null,
    };
  }

  return null;
}

function largePreviewEntry(index: number): EntryDto {
  const name = `file_${String(index).padStart(6, "0")}.txt`;
  return {
    path: `files/${name}`,
    display: name,
    entry_type: "file",
    size: index % 2 === 0 ? 0 : 128,
    compressed: index % 2 === 0 ? 0 : 64,
    modified: 1781190000 + (index % 86400),
    crc: index,
    encrypted: false,
    encoding: "utf-8",
  };
}

function largePreviewPages(total: number, pageSize: number): Map<number, EntryDto[]> {
  const pages = new Map<number, EntryDto[]>();
  if (total <= 0) return pages;
  const last = Math.floor((total - 1) / pageSize);
  for (let pageNo = 0; pageNo <= last; pageNo += 1) {
    const start = Math.max(0, pageNo * pageSize);
    const end = Math.min(total, start + pageSize);
    const rows: EntryDto[] = [];
    for (let index = start; index < end; index += 1) {
      rows.push(largePreviewEntry(index));
    }
    pages.set(pageNo, rows);
  }
  return pages;
}

function numericParam(params: URLSearchParams, key: string, fallback: number): number {
  const value = Number(params.get(key) ?? fallback);
  return Number.isFinite(value) && value >= 0 ? Math.floor(value) : fallback;
}

function listParam(params: URLSearchParams, key: string, separator: string): string[] {
  return (params.get(key) ?? "")
    .split(separator)
    .map((item) => item.trim())
    .filter(Boolean);
}
