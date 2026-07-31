const legacyRarVolumeSuffix = /\.r\d{2}$/i;
const nativeSplitZipVolumeSuffix = /\.z(?=\d{2,}$)\d*[1-9]\d*$/i;

export const legacyRarVolumeExtensions = Array.from(
  { length: 100 },
  (_, index) => `r${index.toString().padStart(2, "0")}`,
);

export const nativeSplitZipVolumeExtensions = Array.from(
  { length: 99 },
  (_, index) => `z${(index + 1).toString().padStart(2, "0")}`,
);

export function archiveNameWithoutVolumeSuffix(name: string): string {
  return name.replace(/\.\d{3,}$/i, "").replace(nativeSplitZipVolumeSuffix, "");
}

export function isLegacyRarVolumeName(name: string): boolean {
  return legacyRarVolumeSuffix.test(name.trimEnd());
}

export function stripLegacyRarVolumeSuffix(name: string): string {
  return isLegacyRarVolumeName(name) ? name.slice(0, -4) : name;
}

export function isNativeSplitZipVolumeName(name: string): boolean {
  return nativeSplitZipVolumeSuffix.test(name.trimEnd());
}

export function stripNativeSplitZipVolumeSuffix(name: string): string {
  return isNativeSplitZipVolumeName(name) ? name.replace(nativeSplitZipVolumeSuffix, "") : name;
}

export function archiveVolumeFamilyKey(name: string): string {
  const genericBase = name.replace(/\.\d{3,}$/i, "");
  if (genericBase !== name) {
    return `generic:${genericBase}`;
  }
  return `single:${name}`;
}

export function archiveVolumeFamilyKeys(names: readonly string[]): string[] {
  return names.map(archiveVolumeFamilyKey);
}
