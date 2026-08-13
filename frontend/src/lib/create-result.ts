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

function resultString(result: Record<string, unknown> | null, key: string): string | null {
  const value = result?.[key];
  return typeof value === "string" && value.length > 0 ? value : null;
}

function resultNumber(result: Record<string, unknown> | null, key: string): number | null {
  const value = result?.[key];
  return typeof value === "number" && Number.isFinite(value) && value >= 0 ? value : null;
}

export function readCreateResult(
  result: Record<string, unknown> | null,
  fallbackOutput: string,
  fallbackSplit: boolean,
): CreateResult {
  const primaryOutput = resultString(result, "primary_output")
    ?? fallbackOutput;
  const rawOutputs = result?.outputs;
  const outputs = Array.isArray(rawOutputs)
    ? rawOutputs.filter((value): value is string => typeof value === "string" && value.length > 0)
    : [];
  const rawPreservedOutputs = result?.preserved_outputs;
  const preservedOutputs = Array.isArray(rawPreservedOutputs)
    ? rawPreservedOutputs.filter(
        (value): value is string => typeof value === "string" && value.length > 0,
      )
    : [];
  if (outputs.length === 0 && primaryOutput) outputs.push(primaryOutput);
  const split = typeof result?.split === "boolean" ? result.split : fallbackSplit;
  const volumeCount = Math.max(
    1,
    Math.trunc(resultNumber(result, "volume_count") ?? (split ? outputs.length : 1)),
  );
  return {
    primaryOutput,
    outputs,
    preservedOutputs,
    totalBytes: resultNumber(result, "total_bytes") ?? 0,
    volumeCount,
    split,
    requiresSigning: result?.requires_signing === true,
    testedAfterCreate: result?.tested_after_create === true,
    entriesTestedAfterCreate: Math.trunc(
      resultNumber(result, "entries_tested_after_create") ?? 0,
    ),
  };
}
