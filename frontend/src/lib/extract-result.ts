import type { JobResult, ExtractReport } from "./job-result";
export type { ExtractResultCounts } from "./job-result";

type Result = JobResult | ExtractReport | null | undefined;

export interface ExtractResultOutcome { skipped: number; failed: number }

export function extractResultHasRecoveredZipStructure(result: Result): boolean {
  if (!result) return false;
  return ("structure" in result && result.structure === "zip_local_headers_recovered")
    || ("kind" in result && result.kind === "batch_extract" && result.recovered_archives > 0);
}

export function recoveredZipArchivePaths(result: Result): string[] {
  return result && "kind" in result && result.kind === "batch_extract"
    ? result.outputs.filter((output) => output.structure === "zip_local_headers_recovered").map((output) => output.archive)
    : [];
}

export function readExtractResultCounts(result: Result) {
  return result && "counts" in result ? result.counts : null;
}

export function readExtractResultOutcome(result: Result): ExtractResultOutcome {
  const counts = readExtractResultCounts(result);
  return { skipped: counts?.skipped ?? 0, failed: counts?.failed ?? 0 };
}

export function extractResultNeedsAttention(result: Result): boolean {
  const outcome = readExtractResultOutcome(result);
  return extractResultHasRecoveredZipStructure(result) || outcome.skipped > 0 || outcome.failed > 0;
}
