// Formatting helpers: sizes (IEC 1024, one decimal), dates, ETA and
// Shared formatting helpers for dates, byte counts, and middle-ellipsis paths.

const UNITS = ["B", "KB", "MB", "GB", "TB"];

/** Formats a byte count: `1.1 GB` (IEC 1024 base, 1 decimal). */
export function formatBytes(n: number | null | undefined, fallback = "--"): string {
  if (n == null || !Number.isFinite(n) || n < 0) return fallback;
  let value = n;
  let unit = 0;
  while (value >= 1024 && unit < UNITS.length - 1) {
    value /= 1024;
    unit++;
  }
  const text = unit === 0 ? String(Math.round(value)) : value.toFixed(1);
  return `${text} ${UNITS[unit]}`;
}

/** Formats a speed in bytes/second. */
export function formatSpeed(bps: number): string {
  return `${formatBytes(bps)}/s`;
}

/** `YYYY-MM-DD HH:mm`, year omitted within the current year. */
export function formatDate(unixSeconds: number | null): string {
  if (unixSeconds == null) return "--";
  const d = new Date(unixSeconds * 1000);
  const pad = (x: number) => String(x).padStart(2, "0");
  const md = `${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
  const hm = `${pad(d.getHours())}:${pad(d.getMinutes())}`;
  if (d.getFullYear() === new Date().getFullYear()) return `${md} ${hm}`;
  return `${d.getFullYear()}-${md} ${hm}`;
}

/** `mm:ss` ETA from remaining bytes and speed (`--:--` when unknown). */
export function formatEta(remaining: number, speed: number): string {
  if (speed <= 0 || remaining <= 0) return "--:--";
  const s = Math.round(remaining / speed);
  const pad = (x: number) => String(x).padStart(2, "0");
  if (s >= 3600) return `${Math.floor(s / 3600)}:${pad(Math.floor((s % 3600) / 60))}:${pad(s % 60)}`;
  return `${pad(Math.floor(s / 60))}:${pad(s % 60)}`;
}

/**
 * Middle-ellipsis: keeps the head segment and the whole trailing file name,
 * replacing the middle with `…`. Character-budget based —
 * cheap and good enough for mono/list contexts.
 */
export function ellipsisMiddle(text: string, maxChars: number): string {
  if (text.length <= maxChars) return text;
  const parts = text.split("/");
  const tail = parts[parts.length - 1];
  if (parts.length > 1) {
    const head = parts[0];
    const candidate = `${head}/…/${tail}`;
    if (candidate.length <= maxChars) return candidate;
  }
  // The name itself is too long: keep the extension and 6 chars before it.
  const dot = tail.lastIndexOf(".");
  if (dot > 6) {
    return `${tail.slice(0, 6)}…${tail.slice(dot)}`;
  }
  return `${text.slice(0, Math.max(1, maxChars - 1))}…`;
}

/** File-name extension (lowercase, without the dot). */
export function extensionOf(path: string): string {
  const name = path.split("/").pop() ?? "";
  const dot = name.lastIndexOf(".");
  return dot > 0 ? name.slice(dot + 1).toLowerCase() : "";
}

/** Directory part of an absolute path. */
export function dirname(path: string): string {
  const normalized = path.replaceAll("\\", "/");
  const i = normalized.lastIndexOf("/");
  if (i < 0) return ".";
  return i === 0 ? "/" : normalized.slice(0, i);
}

/** File-name part of an absolute path. */
export function basename(path: string): string {
  const normalized = path.replaceAll("\\", "/");
  const i = normalized.lastIndexOf("/");
  return i < 0 ? normalized : normalized.slice(i + 1);
}

/** Parses newline/comma/semicolon separated rules into a stable de-duplicated list. */
export function parseDelimitedRules(input: string): string[] {
  const seen = new Set<string>();
  for (const raw of input.split(/[\r\n,;]+/)) {
    const rule = raw.trim();
    if (rule.length > 0) seen.add(rule);
  }
  return [...seen];
}
