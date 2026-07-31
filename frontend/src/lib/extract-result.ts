export interface ExtractResultCounts {
  destination: string;
  selected_entries: number;
  created: number;
  directories: number;
  skipped: number;
  replaced: number;
  renamed: number;
  failed: number;
  output_bytes: number;
}

export interface ExtractResultOutcome {
  structured: boolean;
  skipped: number;
  failed: number;
}

function resultCount(value: unknown): number {
  return typeof value === "number" && Number.isFinite(value)
    ? Math.max(0, Math.trunc(value))
    : 0;
}

export function readExtractResultCounts(
  result: Record<string, unknown> | null | undefined,
): ExtractResultCounts | null {
  const raw = result?.counts;
  if (raw === null || typeof raw !== "object" || Array.isArray(raw)) return null;
  const counts = raw as Record<string, unknown>;
  return {
    destination: typeof counts.destination === "string" ? counts.destination : "",
    selected_entries: resultCount(counts.selected_entries),
    created: resultCount(counts.created),
    directories: resultCount(counts.directories),
    skipped: resultCount(counts.skipped),
    replaced: resultCount(counts.replaced),
    renamed: resultCount(counts.renamed),
    failed: resultCount(counts.failed),
    output_bytes: resultCount(counts.output_bytes),
  };
}

export function readExtractResultOutcome(
  result: Record<string, unknown> | null | undefined,
): ExtractResultOutcome {
  const counts = readExtractResultCounts(result);
  if (counts) {
    return {
      structured: true,
      skipped: counts.skipped,
      failed: counts.failed,
    };
  }
  return {
    structured: false,
    skipped: resultCount(result?.skipped),
    failed: 0,
  };
}

export function extractResultNeedsAttention(
  result: Record<string, unknown> | null | undefined,
): boolean {
  const outcome = readExtractResultOutcome(result);
  return outcome.structured && (outcome.skipped > 0 || outcome.failed > 0);
}
