import {
  sameDesktopPath,
  type DesktopPathPlatform,
} from "./desktop-path";

export type CreateSourceKind = "file" | "folder" | "unknown";

export interface CreateSourceRoot {
  path: string;
  kind: CreateSourceKind;
}

function createSourceIndex(
  sources: readonly CreateSourceRoot[],
  path: string,
  platform: DesktopPathPlatform,
): number {
  return sources.findIndex((source) => sameDesktopPath(source.path, path, platform));
}

export function mergeCreateSources(
  existing: readonly CreateSourceRoot[],
  additions: readonly CreateSourceRoot[],
  platform: DesktopPathPlatform,
): CreateSourceRoot[] {
  const merged: CreateSourceRoot[] = [];

  for (const source of [...existing, ...additions]) {
    if (source.path.length === 0) continue;

    const index = createSourceIndex(merged, source.path, platform);
    if (index < 0) {
      merged.push({ ...source });
      continue;
    }

    if (merged[index].kind === "unknown" && source.kind !== "unknown") {
      merged[index] = { ...merged[index], kind: source.kind };
    }
  }

  return merged;
}

export function includesCreateSourcePath(
  paths: readonly string[],
  path: string,
  platform: DesktopPathPlatform,
): boolean {
  return paths.some((candidate) => sameDesktopPath(candidate, path, platform));
}

export function toggleCreateSourcePath(
  paths: readonly string[],
  path: string,
  platform: DesktopPathPlatform,
): string[] {
  if (!includesCreateSourcePath(paths, path, platform)) return [...paths, path];
  return paths.filter((candidate) => !sameDesktopPath(candidate, path, platform));
}

export function removeCreateSourcesByPaths(
  sources: readonly CreateSourceRoot[],
  paths: readonly string[],
  platform: DesktopPathPlatform,
): CreateSourceRoot[] {
  if (paths.length === 0) return [...sources];
  return sources.filter(
    (source) => !includesCreateSourcePath(paths, source.path, platform),
  );
}

export function createSourcePaths(
  sources: readonly CreateSourceRoot[],
): string[] {
  return sources.map((source) => source.path);
}
