import type { CreateSplitPreset, CreateSplitUnit } from "./ui-model";

const bytesPerMiB = 1024 ** 2;
const bytesPerGiB = 1024 ** 3;
const minimumSplitSize = Math.ceil(0.1 * bytesPerMiB);
export const fat32CompatibleSplitSizeBytes = 0xffff_ffff;

export function resolveSplitSizeBytes(
  preset: CreateSplitPreset,
  customAmount: string,
  customUnit: CreateSplitUnit,
  exactSizeBytes: string | null = null,
): number | null {
  if (preset === "none") return null;
  if (exactSizeBytes !== null) {
    const exact = Number(exactSizeBytes);
    return Number.isSafeInteger(exact) && exact >= minimumSplitSize ? exact : null;
  }
  if (preset === "25-mib") return 25 * bytesPerMiB;
  if (preset === "100-mib") return 100 * bytesPerMiB;
  if (preset === "700-mib") return 700 * bytesPerMiB;
  if (preset === "4-gib") return fat32CompatibleSplitSizeBytes;

  const amount = Number(customAmount);
  if (!Number.isFinite(amount) || amount <= 0) return null;
  const multiplier = customUnit === "gib" ? bytesPerGiB : bytesPerMiB;
  const bytes = Math.round(amount * multiplier);
  return Number.isSafeInteger(bytes) && bytes >= minimumSplitSize ? bytes : null;
}
