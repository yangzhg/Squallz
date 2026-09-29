import { createFormatIds, createFormats, type CreateFormatId } from "./ui-model";

export function convertTaskTargetFormat(destination: string): CreateFormatId | null {
  const path = destination.toLowerCase();
  if (path.endsWith(".swm")) return "wim";
  return createFormatIds.find((format) => createFormats[format].extensions
    .some((extension) => path.endsWith(`.${extension}`))) ?? null;
}

function normalizeFormat(value: string | null | undefined): string {
  return value?.trim().toLowerCase().replace(/^\./, "") ?? "";
}

export function suggestedConvertTargetFormat(
  sourceFormat: string | null | undefined,
): CreateFormatId {
  return normalizeFormat(sourceFormat) === "zip" ? "7z" : "zip";
}

export function sourceMatchesConvertTarget(
  sourceFormat: string | null | undefined,
  targetFormat: CreateFormatId,
): boolean {
  const normalized = normalizeFormat(sourceFormat);
  return normalized === normalizeFormat(targetFormat)
    || createFormats[targetFormat].extensions.some(
      (extension) => normalized === normalizeFormat(extension),
    );
}

export function ensureConvertOutputExtension(
  path: string,
  targetFormat: CreateFormatId,
  requiredExtension?: string,
): string {
  const normalizedPath = path.toLowerCase();
  const format = createFormats[targetFormat];
  const extensions = requiredExtension ? [requiredExtension] : format.extensions;
  if (
    extensions.some(
      (extension) => normalizedPath.endsWith(`.${extension.toLowerCase()}`),
    )
  ) {
    return path;
  }
  return `${path}.${requiredExtension ?? format.extension}`;
}
