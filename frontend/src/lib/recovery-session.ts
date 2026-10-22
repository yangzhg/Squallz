import type { DesktopPathPlatform } from "./desktop-path";
import type { ArchiveInfo, JobSpec } from "./ipc";
import type { Task } from "./jobs.svelte";
import type { UiMode } from "./uiMode.svelte";

export type RecoveryOutputKind = "export_sqz" | "repair_sqz" | "repair_zip" | "repair_recovery";
export type RecoveryDialogs = typeof import("@tauri-apps/plugin-dialog");

export interface RecoverySessionPorts {
  read(): Readonly<{
    active: boolean;
    mode: UiMode;
    generation: number;
    archive: ArchiveInfo | null;
    tasks: readonly Task[];
    selection: string[] | null;
    platform: DesktopPathPlatform;
    level: number;
    profile: string;
  }>;
  dialogs(): Promise<RecoveryDialogs>;
  openDialog(purpose: string, open: RecoveryDialogs["open"], options: NonNullable<Parameters<RecoveryDialogs["open"]>[0]>): ReturnType<RecoveryDialogs["open"]>;
  saveDialog(purpose: string, save: RecoveryDialogs["save"], options: NonNullable<Parameters<RecoveryDialogs["save"]>[0]>): ReturnType<RecoveryDialogs["save"]>;
  submit(spec: JobSpec): Promise<number>;
  blocked(error: unknown): boolean;
  focusBlockingTask(): boolean;
  notice(message: string): void;
  extension(name: string): string | null;
  stem(name: string): string;
  extractDestination(path: string): string;
  mutationDisabledReason(): string;
  outputFilterName(format: "zip" | "7z" | "tar.zst" | "tar" | "sqz"): string;
  inspectionId(): string;
}
