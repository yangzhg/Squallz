import type { SfxPublishRequest } from "../components/SfxPublishDialog.svelte";
import type { JobSpec } from "./ipc";
import {
  taskOutputPath,
  type TaskDialogModel,
} from "./task-model";
import {
  desktopBasename,
  desktopDirname,
  joinDesktopPath,
  sameDesktopPath,
  type DesktopPathPlatform,
} from "./desktop-path";

export type PreparedMacosSfxPublish =
  | Readonly<{ kind: "same_output" }>
  | Readonly<{
      kind: "ready";
      output: string;
    }>;

export function suggestedMacosSfxPublishPath(
  source: string,
  platform: DesktopPathPlatform,
): string {
  const sourceName = desktopBasename(source, platform);
  const stem = sourceName.toLowerCase().endsWith(".app")
    ? sourceName.slice(0, -4)
    : sourceName;
  return joinDesktopPath(
    desktopDirname(source, platform),
    `${stem}-published.app`,
    platform,
  );
}

export function prepareMacosSfxPublish(
  source: string,
  selected: string,
  platform: DesktopPathPlatform,
): PreparedMacosSfxPublish {
  const output = desktopBasename(selected, platform).toLowerCase().endsWith(".app")
    ? selected
    : `${selected}.app`;
  if (sameDesktopPath(source, output, platform)) return { kind: "same_output" };
  return {
    kind: "ready",
    output,
  };
}

export function macosSfxPublishJobSpec(request: SfxPublishRequest): JobSpec {
  return {
    kind: "publish_macos_sfx",
    source: request.source,
    output: request.output,
    identity: request.identity,
    notary_profile: request.notaryProfile,
  };
}

export function taskCanPublishMacosSfx(task: TaskDialogModel): boolean {
  const unsignedCreate = task.state === "done"
    && task.spec.kind === "compress"
    && task.spec.sfx_target === "macos"
    && task.result?.requires_signing !== false
    && Boolean(taskOutputPath(task));
  const retry = (task.state === "failed" || task.state === "cancelled")
    && task.spec.kind === "publish_macos_sfx"
    && task.spec.source.length > 0;
  return unsignedCreate || retry;
}
