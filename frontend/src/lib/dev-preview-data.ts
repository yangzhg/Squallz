import type {
  ArchiveInfo,
  EntryDto,
  EntryPreviewDto,
  IntegrationStatusDto,
  IntegrationSystemDiagnosticsDto,
  NestedArchivePreviewDto,
  QueueWaitReason,
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

type PreviewTaskKind =
  | "compress"
  | "compress_split"
  | "compress_sfx"
  | "compress_sfx_failure"
  | "recovery_cleanup_ready"
  | "recovery_cleanup_unconfirmed"
  | "recovery_cleanup_record"
  | "extract"
  | "extract_unknown_current"
  | "batch_extract"
  | "test"
  | "checksum"
  | "checksum_check"
  | "recovery_protect"
  | "recovery_verify_repairable"
  | "recovery_verify_multi_file_repairable"
  | "recovery_verify_over_capacity"
  | "update_scan"
  | "update_verify"
  | "update_commit";
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
  const completedTask = completedTaskParam(params.get("previewCompletedTask"));
  const activeTask = completedTaskParam(params.get("previewActiveTask"));
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
  const installed = states
    .filter((action) => action.state !== "missing")
    .map((action) => ({
      id: action.id,
      name: action.name,
      kind: "macos_finder_quick_action",
      path: `/Users/alex/Library/Services/Squallz-${action.id}.workflow`,
      script_path: `/Users/alex/Library/Application Support/Squallz/context-actions/${action.id}.sh`,
    }));

  return {
    platform: "macos",
    services_dir: "/Users/alex/Library/Services",
    script_dir: "/Users/alex/Library/Application Support/Squallz/context-actions",
    health: preview === "healthy" ? "healthy" : preview === "missing" ? "missing" : "needs_repair",
    actions: states,
    can_repair: true,
    can_remove: preview !== "missing",
    installed,
    missing: states.filter((action) => action.state === "missing").map((action) => action.name),
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

function completedTaskParam(value: string | null): RuntimePreviews["completedTask"] {
  if (
    value === "compress" ||
    value === "compress_split" ||
    value === "compress_sfx" ||
    value === "compress_sfx_failure" ||
    value === "recovery_cleanup_ready" ||
    value === "recovery_cleanup_unconfirmed" ||
    value === "recovery_cleanup_record" ||
    value === "extract" ||
    value === "extract_unknown_current" ||
    value === "batch_extract" ||
    value === "test" ||
    value === "checksum" ||
    value === "checksum_check" ||
    value === "recovery_protect" ||
    value === "recovery_verify_repairable" ||
    value === "recovery_verify_multi_file_repairable" ||
    value === "recovery_verify_over_capacity" ||
    value === "update_scan" ||
    value === "update_verify" ||
    value === "update_commit"
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
      source: `${sampleArchiveRoot}/${name}`,
      name,
      read_only: false,
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
