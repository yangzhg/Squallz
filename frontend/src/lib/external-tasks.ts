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

const externalOpenActionLabelKeys: Record<ExternalOpenAction, string> = {
  checksum: "gui.settings.integration.context_action.checksum",
  "extract-here": "gui.settings.integration.context_action.extract_here",
  "extract-to-folder": "gui.settings.integration.context_action.extract_to_archive",
  "extract-sfx": "gui.external_task.extract_sfx",
  "compress-to-7z": "gui.settings.integration.context_action.compress_to_7z",
  "test-archive": "gui.settings.integration.context_action.test_archive",
};

export function externalOpenAction(value: string | null | undefined): ExternalOpenAction | null {
  return externalOpenActions.includes(value as ExternalOpenAction) ? (value as ExternalOpenAction) : null;
}

export function isExternalOpenAction(value: string | null | undefined): value is ExternalOpenAction {
  return externalOpenAction(value) !== null;
}

export function externalOpenActionLabelKey(action: ExternalOpenAction): string {
  return externalOpenActionLabelKeys[action];
}
