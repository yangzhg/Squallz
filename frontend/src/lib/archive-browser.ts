import type { EntryDto, EntryPreviewDto } from "./ipc";
import type { ArchiveBrowseRecoveryState } from "../components/ArchiveBrowseRecovery.svelte";
import type { ArchiveEditingState } from "../components/ArchiveEditingStatus.svelte";
import type { MoveConflictReviewView } from "../components/MoveConflictReview.svelte";

export type BrowserEntry = {
  name: string;
  location: string;
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
  selected: boolean;
  previewing: boolean;
  previewBusy: boolean;
  selectionLabel: string;
  previewActionLabel: string;
  previewActionIcon: "external-link" | "folder-open" | "eye";
};

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
  onEntryKeydown: (event: KeyboardEvent, entry: BrowserEntry) => void;
  onOpenEntryContext: (event: MouseEvent, entry: BrowserEntry) => void;
  onToggleEntrySelection: (entry: BrowserEntry, event: MouseEvent) => void;
  onToggleAllEntries: () => void;
  onPreviewEntry: (entry: BrowserEntry) => void;
}
