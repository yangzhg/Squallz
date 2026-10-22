import type { JobResult } from "./job-result";

export interface CreateResult {
  primaryOutput: string;
  outputs: string[];
  preservedOutputs: string[];
  totalBytes: number;
  volumeCount: number;
  split: boolean;
  requiresSigning: boolean;
  testedAfterCreate: boolean;
  entriesTestedAfterCreate: number;
}

export function readCreateResult(
  result: JobResult | null, fallbackOutput: string, fallbackSplit: boolean,
): CreateResult {
  const report = result?.kind === "compress" || result?.kind === "convert" ? result : null;
  return {
    primaryOutput: report?.primary_output ?? fallbackOutput,
    outputs: report?.outputs ?? (fallbackOutput ? [fallbackOutput] : []),
    preservedOutputs: report?.preserved_outputs ?? [],
    totalBytes: report?.total_bytes ?? 0,
    volumeCount: report?.volume_count ?? 1,
    split: report?.split ?? fallbackSplit,
    requiresSigning: report?.sfx?.requires_signing ?? false,
    testedAfterCreate: report?.entries_tested_after_create != null,
    entriesTestedAfterCreate: report?.entries_tested_after_create ?? 0,
  };
}
