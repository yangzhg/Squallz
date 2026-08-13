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

export function extractResultHasRecoveredZipStructure(
  result: Record<string, unknown> | null | undefined,
): boolean {
  if (result?.structure === "zip_local_headers_recovered") return true;
  const outputs = result?.outputs;
  return Array.isArray(outputs) && outputs.some(
    (output) => output !== null
      && typeof output === "object"
      && !Array.isArray(output)
      && (output as Record<string, unknown>).structure === "zip_local_headers_recovered",
  );
}

export function recoveredZipArchivePaths(
  result: Record<string, unknown> | null | undefined,
): string[] {
  const outputs = result?.outputs;
  if (!Array.isArray(outputs)) return [];
  return outputs.flatMap((output) => {
    if (output === null || typeof output !== "object" || Array.isArray(output)) return [];
    const record = output as Record<string, unknown>;
    return record.structure === "zip_local_headers_recovered"
      && typeof record.archive === "string"
      ? [record.archive]
      : [];
  });
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
  return extractResultHasRecoveredZipStructure(result)
    || (outcome.structured && (outcome.skipped > 0 || outcome.failed > 0));
}
