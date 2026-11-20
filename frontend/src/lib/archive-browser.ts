import type { ArchiveSort, ArchiveSortKey, EntryDto, EntryPreviewDto } from "./ipc";
import type { ArchiveBrowseRecoveryState } from "../components/ArchiveBrowseRecovery.svelte";
import type { ArchiveEditingState } from "../components/ArchiveEditingStatus.svelte";
import type { MoveConflictReviewView } from "../components/MoveConflictReview.svelte";
import { formatBytes, formatModified } from "./format";

export type DisplayEntry = {
  name: string;
  location: { path: string; label: string } | null;
  type: string;
  size: string;
  packed: string;
  ratio: string;
  modified: string;
  crc: string;
  encoding: string;
  attr: string;
  source?: EntryDto;
  virtualIndex?: number;
};

export type BrowserEntry = DisplayEntry & {
  selected: boolean;
  previewing: boolean;
  previewBusy: boolean;
  selectionLabel: string;
  previewActionLabel: string;
  previewActionIcon: "external-link" | "folder-open" | "eye";
};

const entryAttributeKeys: Record<EntryDto["entry_type"], string> = {
  dir: "gui.attr.folder",
  file: "gui.attr.file",
  symlink: "gui.attr.symlink",
  hardlink: "gui.attr.hardlink",
  other: "gui.attr.other",
};

export function displayArchiveEntry(
  row: EntryDto,
  virtualIndex: number,
  showLocation: boolean,
  tr: (key: string) => string,
): DisplayEntry {
  const needsEncodingReview = row.encoding !== "utf-8" || row.display.includes("\uFFFD");
  const attributes = [tr(entryAttributeKeys[row.entry_type])];
  if (row.encrypted) attributes.push(tr("gui.attr.encrypted"));
  if (needsEncodingReview) attributes.push(tr("gui.attr.encoding_review"));

  let type = "file";
  if (row.entry_type === "dir") type = "folder";
  else if (row.encrypted) type = "locked";
  else if (needsEncodingReview) type = "warning";

  let location: DisplayEntry["location"] = null;
  if (showLocation) {
    const path = row.path.replaceAll("\\", "/").replace(/\/+$/g, "");
    const parent = path.slice(0, Math.max(0, path.lastIndexOf("/")));
    location = { path: parent, label: parent || tr("gui.list.archive_root") };
  }
  return {
    name: row.display,
    location,
    type,
    size: row.entry_type === "dir" ? "-" : formatBytes(row.size),
    packed: row.compressed == null ? "-" : formatBytes(row.compressed),
    ratio: row.compressed && row.size > 0 ? `${Math.round((row.compressed / row.size) * 100)}%` : "-",
    modified: formatModified(row.modified),
    crc: row.crc == null ? "" : row.crc.toString(16).toUpperCase().padStart(8, "0"),
    encoding: row.encoding.toUpperCase(),
    attr: attributes.join(" · "),
    source: row,
    virtualIndex,
  };
}

const OVERSCAN_ROWS = 12;

export function archiveRowWindow(total: number, rowHeight: number, scrollTop: number, viewportHeight: number) {
  const viewport = Math.max(viewportHeight || 360, rowHeight * 6);
  const visibleRows = Math.ceil(viewport / rowHeight);
  const rawStart = Math.min(Math.floor(scrollTop / rowHeight), Math.max(0, total - visibleRows));
  const start = Math.max(0, rawStart - OVERSCAN_ROWS);
  const end = Math.min(total, start + visibleRows + OVERSCAN_ROWS * 2);
  return { start, end, top: start * rowHeight, bottom: (total - end) * rowHeight };
}

export type ArchivePreviewView = {
  policyKind: string;
  policyCode: string;
  nested: { title: string; subtitle: string; rows: EntryDto[] } | null;
  title: string;
  subtitle: string;
  busy: boolean;
  failed: boolean;
  entry: EntryPreviewDto | null;
  canPreview: boolean;
  actionLabel: string;
  actionIcon: "external-link" | "folder-open" | "eye";
  disabledReason: string;
};

export interface ArchivePreviewActions {
  onOpenNestedPreview: () => void;
  onExtractNestedPreview: () => void;
  onClearPreview: (restoreEntryFocus?: boolean) => void;
  onRetryPreview: () => void;
  onExtractPreviewFailure: () => void;
  onOpenPreview: () => void;
  onRevealPreview: () => void;
  onPreviewSelection: () => void;
}

export interface ArchiveBrowserView {
  sort: { key: ArchiveSortKey | null; direction: ArchiveSort["direction"] };
  selectedSummary: string;
  editingStatus: ArchiveEditingState | null;
  conflict: MoveConflictReviewView | null;
  structureWarning: string | null;
  recovery: ArchiveBrowseRecoveryState;
  selection: {
    checked: boolean;
    mixed: boolean;
    disabled: boolean;
    label: string;
    busy: boolean;
    busyLabel: string;
  };
  totalRows: number;
  rows: readonly (BrowserEntry | null)[];
  startIndex: number;
  rowsPending: boolean;
  paddingTop: number;
  paddingBottom: number;
}

export interface ArchiveBrowserActions {
  onSort: (key: ArchiveSortKey) => void;
  tr: (key: string, fallback?: string) => string;
  onOpenRoot: () => void;
  onOpenRecovery: () => void;
  onRetryBrowse: () => void;
  onRetryFormats: () => void;
  onCancelMoveConflict: () => void;
  onSubmitMoveReadyOnly: () => void;
  onSubmitMoveKeepBoth: () => void;
  onBrowseScroll: (event: Event) => void;
  onSelectEntry: (entry: BrowserEntry, event: MouseEvent | KeyboardEvent) => void;
  onActivateEntry: (entry: BrowserEntry) => void;
  onOpenResultFolder: (entry: BrowserEntry) => void;
  onEntryKeydown: (event: KeyboardEvent, entry: BrowserEntry) => void;
  onOpenEntryContext: (event: MouseEvent, entry: BrowserEntry) => void;
  onToggleEntrySelection: (entry: BrowserEntry, event: MouseEvent) => void;
  onToggleAllEntries: () => void;
  onPreviewEntry: (entry: BrowserEntry) => void;
}
