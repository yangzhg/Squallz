import type { ExternalTaskAction } from "./ipc";

export const externalOpenActions = [
  "checksum",
  "extract-here",
  "extract-to-folder",
  "extract-sfx",
  "compress-to-7z",
  "test-archive",
] as const satisfies readonly ExternalTaskAction[];

export type ExternalOpenAction = (typeof externalOpenActions)[number];
export const desktopIntegrationActions = [
  "checksum",
  "extract-here",
  "extract-to-folder",
  "compress-to-7z",
  "test-archive",
] as const satisfies readonly ExternalOpenAction[];
export type DesktopIntegrationAction = (typeof desktopIntegrationActions)[number];

export interface ExternalOpenActionCopy {
  labelKey: string;
  fallbackLabel: string;
}

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
  "extract-sfx": {
    labelKey: "gui.external_task.extract_sfx",
    fallbackLabel: "Extract self-extractor",
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
