import { tFallback as tr } from "./i18n.svelte";
import { formatBytes } from "./format";
import type { CreateArchivePresetOptions, ExtractArchivePresetOptions, CreateContentPolicy } from "./ipc";
import { createFormatIds, createFormats, type CreateFormatId } from "./ui-model";
import type { ExtractOverwriteMode, ExtractSymlinkMode } from "./extract-options-draft.svelte";
type PresetSqzInnerFormat = Extract<CreateArchivePresetOptions["format_options"], { kind: "sqz" }>["inner_format"];

function isCreateFormatId(value: string): value is CreateFormatId {
  return createFormatIds.includes(value as CreateFormatId);
}

export function archivePresetPartSummary(volumes: CreateArchivePresetOptions["volumes"]): string {
  if (volumes.kind === "single") return tr("gui.presets.single_archive");
  const value = Number(volumes.size_bytes);
  const size = Number.isSafeInteger(value) ? formatBytes(value) : `${volumes.size_bytes} B`;
  return tr("gui.presets.split_archive").replace("{size}", size);
}

export function presetSqzInnerFormatLabel(innerFormat: PresetSqzInnerFormat): string {
  if (innerFormat === "zip") return tr("gui.presets.sqz_inner_zip");
  if (innerFormat === "tar") return tr("gui.presets.sqz_inner_tar");
  if (innerFormat === "7z") return tr("gui.presets.sqz_inner_7z");
  if (innerFormat === "zstd") return tr("gui.presets.sqz_inner_zstd");
  return tr("gui.presets.sqz_inner_sqz");
}

export function createContentPolicyLabel(policy: CreateContentPolicy): string {
  if (policy === "cross_platform_clean") {
    return tr("gui.create.content_policy.clean");
  }
  if (policy === "keep_all_files") {
    return tr("gui.create.content_policy.keep_all");
  }
  return tr("gui.create.content_policy.custom");
}

export function createArchivePresetSummary(options: CreateArchivePresetOptions): string {
  let format = isCreateFormatId(options.format)
    ? createFormats[options.format].label
    : options.format.toUpperCase();
  if (options.format_options.kind === "sqz") {
    format = tr("gui.presets.sqz_format_summary")
      .replace("{format}", format)
      .replace("{inner}", presetSqzInnerFormatLabel(options.format_options.inner_format));
  }
  const protection = options.credential.kind === "none"
    ? tr("gui.presets.no_password")
    : tr("gui.presets.ask_password");
  const summary = tr("gui.presets.create_summary")
    .replace("{format}", format)
    .replace("{level}", String(options.level))
    .replace("{parts}", archivePresetPartSummary(options.volumes))
    .replace("{protection}", protection)
    .replace("{content}", createContentPolicyLabel(options.content_policy));
  return options.test_after_create
    ? `${summary} · ${tr("gui.presets.integrity_test")}`
    : summary;
}

export function extractArchivePresetSummary(options: ExtractArchivePresetOptions): string {
  const destination = options.destination.layout === "smart"
    ? tr("gui.presets.smart_destination")
    : options.destination.layout === "archive_folder"
      ? tr("gui.presets.archive_folder_destination")
      : options.destination.base === "ask"
        ? tr("gui.presets.choose_destination")
        : tr("gui.presets.same_folder_destination");
  return tr("gui.presets.extract_summary")
    .replace("{destination}", destination)
    .replace("{conflicts}", extractOverwriteLabel(options.existing_output))
    .replace("{links}", extractSymlinkLabel(options.symlinks));
}

export function extractOverwriteLabel(mode: ExtractOverwriteMode): string {
  if (mode === "skip") return tr("gui.extract.overwrite.skip");
  if (mode === "overwrite") return tr("gui.extract.overwrite.overwrite");
  if (mode === "rename") return tr("gui.extract.overwrite.rename");
  return tr("gui.extract.overwrite.ask");
}

export function extractSymlinkLabel(mode: ExtractSymlinkMode): string {
  if (mode === "skip") return tr("gui.extract.symlink.skip");
  if (mode === "follow") return tr("gui.extract.symlink.follow");
  return tr("gui.extract.symlink.preserve");
}
