import type {
  ArchiveInfo,
  EntryDto,
  EntryPreviewDto,
  IntegrationStatusDto,
  IntegrationSystemDiagnosticsDto,
  NestedArchivePreviewDto,
  NestedArchivePasswords,
  QueueWaitReason,
  PasswordBookStatus,
} from "./ipc";
import { parseTaskPreviewKind, type PreviewTaskKind } from "./task-preview";

const passwordBookPreviewRequests = new Map<string, () => void>();
let passwordBookPreviewStatus: PasswordBookStatus | null = null;

export function readPasswordBookPreview(params: URLSearchParams): PasswordBookStatus | null {
  if (!import.meta.env.DEV || params.get("previewPasswordBook") !== "1") return null;
  if (!passwordBookPreviewStatus) {
    const state = params.get("passwordBookState");
    const locked = state === "locked";
    const unavailable = state === "unavailable";
    passwordBookPreviewStatus = {
      session: ["session", "locked", "unavailable", "saved"].includes(state ?? ""),
      available: !unavailable,
      saved: locked || unavailable ? null : state === "saved",
      error: locked ? { key: "error.secret_store", params: {}, detail: "" } : null,
    };
  }
  return { ...passwordBookPreviewStatus };
}

export function forgetPasswordBookPreview(params: URLSearchParams): PasswordBookStatus | null {
  const status = readPasswordBookPreview(params);
  if (!status) return null;
  passwordBookPreviewStatus = { ...status, session: false, saved: status.error || !status.available ? null : false };
  return { ...passwordBookPreviewStatus };
}

export function savePasswordBookPreview(params: URLSearchParams, requestId: string, password: string): Promise<PasswordBookStatus> | null {
  if (!import.meta.env.DEV || params.get("previewPasswordBook") !== "1") return null;
  const valid = password === "book-preview";
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      passwordBookPreviewRequests.delete(requestId);
      if (valid) {
        passwordBookPreviewStatus = { session: true, available: true, saved: true, error: null };
        resolve({ ...passwordBookPreviewStatus });
      }
      else reject({ key: "error.wrong_password", params: {}, detail: "" });
    }, 1800);
    passwordBookPreviewRequests.set(requestId, () => {
      clearTimeout(timer);
      passwordBookPreviewRequests.delete(requestId);
      reject({ key: "error.cancelled", params: {}, detail: "" });
    });
  });
}

export function cancelPasswordBookPreview(params: URLSearchParams, requestId: string): boolean {
  if (!import.meta.env.DEV || params.get("previewPasswordBook") !== "1") return false;
  passwordBookPreviewRequests.get(requestId)?.();
  return true;
}

export interface ArchivePreview {
  info: ArchiveInfo;
  rows: EntryDto[];
  previewRows?: EntryDto[];
  total: number;
  selected: string[];
  pages?: Map<number, EntryDto[]>;
  nestedPreview: NestedArchivePreviewDto | null;
}

export function nestedPasswordPreviewSample(
  params: URLSearchParams,
  outerSource: string,
  entryPath: string,
  passwords: NestedArchivePasswords,
): NestedArchivePreviewDto | null {
  if (!import.meta.env.DEV || params.get("previewEntryPassword") !== "1") return null;
  for (const scope of ["outer", "inner"] as const) {
    if (passwords[scope] !== `${scope}-preview`) {
      throw {
        key: passwords[scope] ? "error.wrong_password" : "error.password_required",
        params: { password_scope: scope }, detail: "",
      };
    }
  }
  return {
    outer_path: outerSource, entry_path: entryPath,
    archive: nestedPreviewArchiveInfo(outerSource, entryPath, nestedPreviewItems.length),
    truncated: false, items: nestedPreviewItems,
  };
}

function nestedPreviewArchiveInfo(outerSource: string, entryPath: string, count: number): ArchiveInfo {
  return {
    id: 9002, source: "archive://9002", path: `${outerSource} › ${entryPath}`,
    name: entryPath.split("/").at(-1) ?? entryPath, format: "7z", structure: "complete",
    read_only: true, entry_count: count, volumes: null,
    non_utf8_name_count: 0, garbled_count: 0, suggested_encoding: null, encoding_override: null,
  };
}

export function preparedNestedPreviewRows(params: URLSearchParams, source: string): EntryDto[] | null {
  if (!import.meta.env.DEV || source !== "archive://9002") return null;
  if (params.get("previewEntryPassword") === "1") return nestedPreviewItems;
  return readArchivePreview(params, 500)?.nestedPreview?.items ?? null;
}

type TaskQueuePreview = Exclude<QueueWaitReason, "queue_order">;

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
  preflightDestinationBytes: number;
  preflightDestinationCurrent: string;
  extractAvailableBytes: number;
  toast: "warning" | "danger" | null;
  completedTask: PreviewTaskKind | null;
  activeTask: PreviewTaskKind | null;
  taskQueue: TaskQueuePreview | null;
  jobSubmitDelayMs: number;
  integrationStatus: IntegrationStatusDto | null;
  integrationDiagnostics: IntegrationSystemDiagnosticsDto | null;
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
  preflightDestinationBytes: 0,
  preflightDestinationCurrent: "",
  extractAvailableBytes: 0,
  toast: null,
  completedTask: null,
  activeTask: null,
  taskQueue: null,
  jobSubmitDelayMs: 0,
  integrationStatus: null,
  integrationDiagnostics: null,
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

const linkPreviewItems: EntryDto[] = ([
  { path: "report-link", display: "report-link", entry_type: "symlink", encrypted: false },
  { path: "report-copy", display: "report-copy", entry_type: "hardlink", encrypted: false },
  { path: "private-link", display: "private-link", entry_type: "symlink", encrypted: true },
] satisfies Pick<EntryDto, "path" | "display" | "entry_type" | "encrypted">[]).map((entry) => ({
  ...entry,
  size: 15,
  compressed: 15,
  modified: 1781199120,
  crc: null,
  encoding: "utf-8",
}));

const historicalTimePreviewEntries: EntryDto[] = import.meta.env.DEV ? ([
  ["Correspondence 1900.pdf", -2_208_943_800],
  ["Lunar mission 1969.pdf", -14_182_980],
  ["Archive index 1970.txt", 0],
  ["Undated manuscript.txt", null],
  ["Uncertain manuscript.txt", Number.MAX_SAFE_INTEGER],
] satisfies [string, number | null][]).map(([name, modified]) => ({
  ...archivePreviewEntries[2], path: name, display: name, modified,
})) : [];

const longNamePreviewEntries: EntryDto[] = [
  {
    ...archivePreviewEntries[2],
    path: "Quarterly financial report — consolidated statements and supporting documents.pdf",
    display: "Quarterly financial report — consolidated statements and supporting documents.pdf",
  },
  {
    ...archivePreviewEntries[2],
    path: "reports/2026/第三季度/交付资料与财务归档/季度财务报告与附件汇总（已审核最终版本）.pdf",
    display: "季度财务报告与附件汇总（已审核最终版本）.pdf",
  },
];

export function readRuntimePreviews(params: URLSearchParams, pageSize: number): RuntimePreviews {
  if (!import.meta.env.DEV) return emptyRuntimePreviews;

  const duplicateMinSize = numericParam(params, "duplicateMinSize", 1024 * 1024);
  const preflightScanned = numericParam(params, "previewPreflightScan", 0);
  const preflightCurrent = preflightScanned > 0
    ? params.get("previewPreflightCurrent") ?? "project/src/main.rs"
    : "";
  const preflightDestinationBytes = numericParam(params, "previewDestinationBytes", 0);
  const preflightDestinationCurrent = preflightDestinationBytes > 0
    ? params.get("previewDestinationCurrent") ?? "/Users/alex/Archives/project.zip"
    : "";
  const extractAvailableBytes = numericParam(
    params,
    "previewExtractAvailableBytes",
    256 * 1024 * 1024 * 1024,
  );
  const completedTask = parseTaskPreviewKind(params.get("previewCompletedTask"));
  const activeTask = parseTaskPreviewKind(params.get("previewActiveTask"));
  const taskQueueParam = params.get("previewTaskQueue");
  const taskQueue = taskQueueParam === "cpu"
    ? "cpu_budget"
    : taskQueueParam === "1" || taskQueueParam === "slot"
      ? "parallel_limit"
      : null;
  const jobSubmitDelayMs = Math.max(0, Math.min(1200, numericParam(params, "previewJobSubmitDelayMs", 0)));
  const toastParam = params.get("previewToast");
  const toast = toastParam === "warning" || toastParam === "danger" ? toastParam : null;

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
    preflightDestinationBytes,
    preflightDestinationCurrent,
    extractAvailableBytes,
    toast,
    completedTask,
    activeTask,
    taskQueue,
    jobSubmitDelayMs,
    integrationStatus: readIntegrationPreview(params),
    integrationDiagnostics: readIntegrationDiagnosticsPreview(params),
  };
}

function readIntegrationPreview(params: URLSearchParams): IntegrationStatusDto | null {
  const preview = params.get("previewIntegration");
  if (preview !== "healthy" && preview !== "repair" && preview !== "missing") return null;

  const actionNames: Array<[string, string]> = [
    ["checksum", "Checksum"],
    ["extract-here", "Extract Here"],
    ["extract-to-folder", "Extract to <archive>/"],
    ["compress-to-7z", "Compress to 7Z"],
    ["test-archive", "Test archive"],
  ];
  const states = actionNames.map(([id, name], index) => {
    if (preview === "missing") return { id, name, state: "missing" as const, issue: null };
    if (preview === "repair" && index === 1) {
      return { id, name, state: "damaged" as const, issue: "script_outdated" };
    }
    if (preview === "repair" && index === 2) {
      return { id, name, state: "missing" as const, issue: null };
    }
    return { id, name, state: "healthy" as const, issue: null };
  });
  return {
    platform: "macos",
    services_dir: "/Users/alex/Library/Services",
    script_dir: "/Users/alex/Library/Application Support/Squallz/context-actions",
    health: preview === "healthy" ? "healthy" : preview === "missing" ? "missing" : "needs_repair",
    actions: states,
    can_repair: true,
    can_remove: preview !== "missing",
    unsupported: [],
  };
}

function readIntegrationDiagnosticsPreview(params: URLSearchParams): IntegrationSystemDiagnosticsDto | null {
  const preview = params.get("previewIntegration");
  if (preview !== "healthy" && preview !== "repair" && preview !== "missing") return null;

  const requestedState = params.get("previewDefaultHandlers");
  const summaryState = requestedState === "squallz" || requestedState === "mixed" || requestedState === "other" || requestedState === "unknown" || requestedState === "unavailable"
    ? requestedState
    : preview === "missing"
      ? "other"
      : "mixed";
  const extensions = [
    "zip", "jar", "apk", "cbz", "cbr", "ipa", "7z", "rar", "sqz", "tar",
    "tgz", "tbz2", "txz", "tzst", "gz", "bz2", "xz", "zst", "lz4", "br",
    "001", "wim", "swm",
  ];
  const handlers = summaryState === "unavailable"
    ? []
    : extensions.map((extension, index) => {
        const state = summaryState === "squallz"
          ? "squallz" as const
          : summaryState === "unknown" && index === extensions.length - 1
            ? "unknown" as const
            : summaryState === "mixed" && (extension === "rar" || extension === "sqz")
              ? "squallz" as const
              : "other" as const;
        return {
          extension,
          state,
          application_name: state === "squallz"
            ? "Squallz"
            : state === "other"
              ? extension === "7z" ? "Keka" : "Archive Utility"
              : null,
        };
      });
  const sevenZipPreview = params.get("previewSevenZip");
  const sevenZipAvailable = sevenZipPreview !== "missing" && sevenZipPreview !== "misconfigured";
  const sevenZipConfigured = sevenZipPreview === "misconfigured";
  const sevenZipSource = sevenZipPreview === "application"
    ? "application" as const
    : sevenZipConfigured
      ? "environment" as const
      : sevenZipAvailable
        ? "path" as const
        : null;
  const wimlibPreview = params.get("previewWimlib");
  const wimlibAvailable = wimlibPreview !== "missing" && wimlibPreview !== "misconfigured";
  const wimlibConfigured = wimlibPreview === "misconfigured";
  const wimlibSource = wimlibPreview === "application"
    ? "application" as const
    : wimlibConfigured
      ? "environment" as const
      : wimlibAvailable
        ? "path" as const
        : null;
  const unrarPreview = params.get("previewUnrar");
  const unrarAvailable = unrarPreview !== "missing" && unrarPreview !== "misconfigured";
  const unrarConfigured = unrarPreview === "misconfigured";
  const unrarSource = unrarConfigured
    ? "environment" as const
    : unrarAvailable
      ? "path" as const
      : null;

  return {
    platform: "macos",
    backends: [
      {
        id: "sevenzip",
        available: sevenZipAvailable,
        configured: sevenZipConfigured,
        source: sevenZipSource,
        tool: sevenZipAvailable ? "7zz" : null,
      },
      ...(wimlibPreview === "checking" ? [] : [{
        id: "wimlib",
        available: wimlibAvailable,
        configured: wimlibConfigured,
        source: wimlibSource,
        tool: wimlibAvailable ? "wimlib-imagex" : null,
      }]),
      {
        id: "unrar",
        available: unrarAvailable,
        configured: unrarConfigured,
        source: unrarSource,
        tool: unrarAvailable ? "unrar" : null,
      },
    ],
    default_handlers: {
      state: summaryState,
      total: handlers.length,
      checked: handlers.filter((handler) => handler.state !== "unknown").length,
      squallz: handlers.filter((handler) => handler.state === "squallz").length,
      handlers,
    },
    file_manager_visibility: {
      state: "manual_check",
      reason: "not_exposed_by_platform",
    },
  };
}

function readArchivePreview(params: URLSearchParams, pageSize: number): ArchivePreview | null {
  if (params.get("previewArchive") !== "1") return null;

  const format = (params.get("previewFormat") ?? "zip").toLowerCase();
  const name = params.get("previewArchiveName") || `product-backup.${format}`;
  const entries = [
    ...archivePreviewEntries,
    ...(params.get("previewLinks") === "1" ? linkPreviewItems : []),
    ...(params.get("previewLongNames") === "1" ? longNamePreviewEntries : []),
    ...(params.get("previewHistoricalTimes") === "1" ? historicalTimePreviewEntries : []),
  ];
  const nestedItems = [
    ...nestedPreviewItems,
    ...(params.get("previewLinks") === "1" ? linkPreviewItems : []),
    ...(params.get("previewLongNames") === "1" ? longNamePreviewEntries : []),
  ];
  const selected = listParam(params, "previewSelected", ",");
  const largeEntryCount = numericParam(params, "previewLargeEntries", 0);
  const pages = largeEntryCount > 0 ? largePreviewPages(largeEntryCount, pageSize) : null;
  const rows = pages?.get(0) ?? entries.filter(
    (entry) => !entry.path.replace(/\/+$/g, "").includes("/"),
  );
  const previewRows = pages ? undefined : entries;
  const total = largeEntryCount > 0 ? largeEntryCount : rows.length;

  return {
    info: {
      id: 9_001,
      path: `${sampleArchiveRoot}/${name}`,
      source: `${sampleArchiveRoot}/${name}`,
      name,
      read_only: false,
      format,
      structure: params.get("previewRecoveredZip") === "1"
        ? "zip_local_headers_recovered"
        : "complete",
      entry_count: largeEntryCount > 0 ? largeEntryCount : entries.length,
      volumes: null,
      non_utf8_name_count: 0,
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
          archive: nestedPreviewArchiveInfo(`${sampleArchiveRoot}/${name}`, "locked-secrets.7z", nestedItems.length),
          truncated: false,
          items: nestedItems,
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
      preview_id: "preview-dev-cover",
      size: 4_096,
      archive_like: false,
    };
  }

  if (entryPath === "Launch plan.pdf") {
    return {
      outer_path: outerPath,
      entry_path: entryPath,
      display_name: "Launch plan.pdf",
      preview_id: "preview-dev-launch-plan",
      size: 3_800_000,
      archive_like: false,
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
