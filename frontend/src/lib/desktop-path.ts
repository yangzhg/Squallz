export type DesktopPathPlatform = "macos" | "windows" | "linux";

export function targetDesktopPlatform(): DesktopPathPlatform {
  return __SQUALLZ_TARGET_PLATFORM__;
}

function isSeparator(character: string, platform: DesktopPathPlatform): boolean {
  return character === "/" || (platform === "windows" && character === "\\");
}

function withoutTrailingSeparators(path: string, platform: DesktopPathPlatform): string {
  let end = path.length;
  while (end > 1 && isSeparator(path[end - 1], platform)) {
    if (platform === "windows" && end === 3 && path[1] === ":") break;
    end -= 1;
  }
  return path.slice(0, end);
}

function lastSeparatorIndex(path: string, platform: DesktopPathPlatform): number {
  const slash = path.lastIndexOf("/");
  return platform === "windows" ? Math.max(slash, path.lastIndexOf("\\")) : slash;
}

function canonicalWindowsPath(path: string): string {
  const replaced = path.replaceAll("\\", "/");
  const unc = replaced.startsWith("//");
  const collapsed = replaced.replace(/\/{2,}/g, "/");
  return unc ? `/${collapsed}` : collapsed;
}

function isAbsoluteDesktopPath(path: string, platform: DesktopPathPlatform): boolean {
  if (platform !== "windows") return path.startsWith("/");
  const normalized = canonicalWindowsPath(path);
  return /^[a-z]:\//iu.test(normalized) || /^\/\/[^/]+\/[^/]+/u.test(normalized);
}

export function desktopBasename(path: string, platform: DesktopPathPlatform): string {
  const source = withoutTrailingSeparators(path, platform);
  const index = lastSeparatorIndex(source, platform);
  return index < 0 ? source : source.slice(index + 1);
}

export function desktopDirname(path: string, platform: DesktopPathPlatform): string {
  const source = withoutTrailingSeparators(path, platform);
  const index = lastSeparatorIndex(source, platform);
  if (index < 0) return ".";
  if (index === 0) return source[0];
  if (platform === "windows" && index === 2 && source[1] === ":") {
    return source.slice(0, 3);
  }
  return source.slice(0, index);
}

export function sameDesktopPath(
  left: string,
  right: string,
  platform: DesktopPathPlatform,
): boolean {
  const normalize = (value: string) => withoutTrailingSeparators(
    platform === "windows" ? canonicalWindowsPath(value) : value,
    platform,
  );
  const leftPath = normalize(left);
  const rightPath = normalize(right);
  return platform === "windows"
    ? leftPath.toLowerCase() === rightPath.toLowerCase()
    : leftPath === rightPath;
}

export function normalizeDesktopFolder(
  value: string,
  platform: DesktopPathPlatform,
): string | null {
  const trimmed = value.trim();
  if (!trimmed || !isAbsoluteDesktopPath(trimmed, platform)) return null;
  const normalized = platform === "windows" ? canonicalWindowsPath(trimmed) : trimmed;
  return withoutTrailingSeparators(normalized, platform);
}

export function joinDesktopPath(
  folder: string,
  name: string,
  platform: DesktopPathPlatform,
): string {
  if (isSeparator(folder[folder.length - 1] ?? "", platform)) return `${folder}${name}`;
  const separator = platform === "windows" && folder.includes("\\") && !folder.includes("/")
    ? "\\"
    : "/";
  return `${folder}${separator}${name}`;
}

export function archiveBaseOrDefault(stem: string): string {
  return stem === "" || stem === "." || stem === ".." ? "archive" : stem;
}
