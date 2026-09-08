export type RecoveryWorkspaceVariant = "modern" | "classic";

export type RecoveryWorkspaceTone = "neutral" | "success" | "warning" | "danger";

export type RecoveryWorkspaceMetrics = Readonly<{
  blocksNeeded: string;
  recoveryBlocksAvailable: string;
  remainingMargin: string;
}>;

export type RecoveryWorkspaceView = Readonly<{
  archiveName: string | null;
  par2Name: string | null;
  currentArchiveAvailable: boolean;
  usesCurrentArchive: boolean;
  usesDefaultPar2: boolean;
  pickerBusy: boolean;
  pickerBusyReason: string;
  testDisabledReason: string;
  sourceName: string;
  requestedRedundancy: string;
  redundancyDraft: string;
  redundancyError: string;
  protectedSourceCount: number;
  repairCapacity: string;
  repairOutputMode: string;
  plannedIndex: string;
  resultTone: RecoveryWorkspaceTone;
  resultTitle: string;
  resultDetail: string;
  resultExplanation: string;
  resultFooter: string;
  resultAvailable: boolean;
  metrics: RecoveryWorkspaceMetrics | null;
  beyondCapacity: boolean;
  formatWorkflowTitle: string;
  formatWorkflowBody: string;
  protectDisabledReason: string;
  verifyDisabledReason: string;
  repairDisabledReason: string;
  zipDisabledReason: string;
  sqzRepairDisabledReason: string;
  sqzExportDisabledReason: string;
  bestEffortDisabledReason: string;
  verifyRecommended: boolean;
  repairRecommended: boolean;
}>;

export type RecoveryWorkspaceActions = Readonly<{
  chooseArchive: () => void;
  choosePar2: () => void;
  useCurrentArchive: () => void;
  useDefaultPar2: () => void;
  testArchive: () => void;
  setRedundancy: (value: string) => void;
  protect: () => void;
  verify: () => void;
  repair: () => void;
  repairZip: () => void;
  repairSqz: () => void;
  exportSqz: () => void;
  extractReadable: () => void;
}>;

export type RecoveryWorkspaceTranslate = (key: string, fallback: string) => string;
