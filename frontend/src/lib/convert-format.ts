import { createFormats, type CreateFormatId } from "./ui-model";

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
