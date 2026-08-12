import type { ArchiveInfo, JobSpec } from "./ipc";

export type ConvertWorkspaceVariant = "modern" | "classic";

export type ConvertPreflightEvent = Readonly<{
  request_id?: string;
  phase?: string;
  current?: string;
}>;

export type ConvertRouteStatus = Readonly<{
  sourceFormat: string;
  targetLabel: string;
  profileLabel: string;
  methodLabel: string;
  destination: string;
}>;

export interface ConvertRouteHandle {
  canLeave: () => boolean;
  leave: () => void;
  syncArchive: (archive: ArchiveInfo | null) => void;
  applyPreflightEvent: (event: ConvertPreflightEvent) => boolean;
  status: () => ConvertRouteStatus;
  dispose: () => void;
}

export interface ConvertRouteBridge {
  getArchive: () => ArchiveInfo | null;
  tr: (key: string, fallback: string) => string;
  tError: (error: unknown) => string;
  showNotice: (message: string) => void;
  ensurePreflightListener: () => Promise<void>;
  getDialogModule: () => Promise<typeof import("@tauri-apps/plugin-dialog")>;
  saveNativeDialog: (
    kind: string,
    save: typeof import("@tauri-apps/plugin-dialog")["save"],
    options: NonNullable<Parameters<typeof import("@tauri-apps/plugin-dialog")["save"]>[0]>,
  ) => Promise<string | null>;
  submitJob: (spec: JobSpec) => Promise<number>;
  focusBlockingTaskIfAny: () => boolean;
  isJobSubmitBlocked: (error: unknown) => boolean;
  jobSubmitBlockedMessage: (error: unknown) => string;
  recordQueuedOperation: (title: string, detail: string) => void;
  archiveStemName: (name: string) => string;
  platform: () => "macos" | "windows" | "linux";
  prepareSubmitFocus: () => void;
  shouldRestorePrimaryFocus: () => boolean;
  register: (handle: ConvertRouteHandle) => void;
}

export type ConvertRouteOwner = object;
