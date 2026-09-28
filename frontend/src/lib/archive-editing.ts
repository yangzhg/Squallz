export type ArchiveEditPathIssue = {
  kind: "empty" | "parent" | "characters" | "trailing" | "reserved";
  segment: string;
};

/** Archive paths use forward slashes; parent references remain visible to validation. */
export function normalizeArchivePath(value: string, fallback = ""): string {
  const parts = value.trim().replaceAll("\\", "/").split("/")
    .filter((part) => part.length > 0 && part !== ".");
  return parts.join("/") || fallback;
}

export function archiveEditPathIssue(path: string, allowRoot = false): ArchiveEditPathIssue | null {
  if (!path && !allowRoot) return { kind: "empty", segment: "" };
  for (const segment of path.replaceAll("\\", "/").split("/").filter(Boolean)) {
    if (segment === "..") return { kind: "parent", segment };
    if (/[<>:"|?*\u0000-\u001F]/u.test(segment)) return { kind: "characters", segment };
    if (/[. ]$/u.test(segment)) return { kind: "trailing", segment };
    const stem = segment.split(".")[0];
    if (/^(?:CON|PRN|AUX|NUL|CONIN\$|CONOUT\$|COM[1-9]|LPT[1-9])$/iu.test(stem)) {
      return { kind: "reserved", segment };
    }
  }
  return null;
}

/** A selected directory already includes each selected descendant. */
export function archiveSelectionRoots(paths: ReadonlySet<string>): string[] {
  return [...paths].filter((path) => {
    for (let slash = path.indexOf("/"); slash >= 0 && slash < path.length - 1; slash = path.indexOf("/", slash + 1)) {
      if (paths.has(path.slice(0, slash + 1))) return false;
    }
    return true;
  });
}
