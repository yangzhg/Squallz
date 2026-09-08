export interface SourceCleanupSummary {
  moved: number;
  kept: number;
  recoveryRequired: number;
  status: string;
}

export function readSourceCleanupSummary(
  result: Record<string, unknown> | null,
): SourceCleanupSummary | null {
  const raw = result?.source_cleanup;
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) return null;
  const cleanup = raw as Record<string, unknown>;
  return {
    moved: Math.max(0, Math.trunc(Number(cleanup.moved) || 0)),
    kept: Math.max(0, Math.trunc(Number(cleanup.kept) || 0)),
    recoveryRequired: Math.max(0, Math.trunc(Number(cleanup.recovery_required) || 0)),
    status: String(cleanup.status ?? ""),
  };
}

export function shouldRefreshSourceCleanupRecovery(
  kind: string,
  result: Record<string, unknown> | null,
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
