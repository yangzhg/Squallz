import type { JobResult } from "./job-result";

export interface SourceCleanupSummary {
  moved: number;
  kept: number;
  recoveryRequired: number;
  status: string;
}

export function readSourceCleanupSummary(
  result: JobResult | null,
): SourceCleanupSummary | null {
  if (!result || !("source_cleanup" in result)) return null;
  const cleanup = result.source_cleanup;
  return { moved: cleanup.moved, kept: cleanup.kept, recoveryRequired: cleanup.recovery_required, status: cleanup.status };
}

export function shouldRefreshSourceCleanupRecovery(
  kind: string,
  result: JobResult | null,
): boolean {
  if (kind !== "compress") return false;
  const status = readSourceCleanupSummary(result)?.status;
  return status !== "completed" && status !== "not_requested";
}

export function isNewSourceCleanupRecoveryGeneration(
  lastGeneration: number,
  generation: number,
): boolean {
  return Number.isSafeInteger(generation) && generation > lastGeneration;
}
