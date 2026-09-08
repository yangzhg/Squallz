export type RecoveryResult = Record<string, unknown> | null;

export type RecoveryRepairGate = "verify_first" | "no_damage" | "over_capacity" | null;

export type RecoveryResultTone = "neutral" | "success" | "warning" | "danger";

export interface RecoveryProtectionResult {
  primaryOutput: string;
  outputs: string[];
}

export interface RecoveryRouteState {
  sourceMode: "none" | "current" | "selected";
  sourceOverride: string | null;
  par2Override: string | null;
}

export function recoveryRouteForOpen(
  source: "preserve" | "current",
  hasCurrentArchive: boolean,
  route: RecoveryRouteState,
): RecoveryRouteState {
  if (
    hasCurrentArchive
    && (source === "current" || (route.sourceMode === "none" && route.par2Override === null))
  ) {
    return { sourceMode: "current", sourceOverride: null, par2Override: null };
  }
  return { ...route };
}

function recordValue(value: unknown): Record<string, unknown> | null {
  return value && typeof value === "object" && !Array.isArray(value)
    ? value as Record<string, unknown>
    : null;
}

function nonEmptyString(value: unknown): string | null {
  return typeof value === "string" && value.length > 0 ? value : null;
}

export function readRecoveryProtectionResult(
  result: RecoveryResult,
  fallbackRecovery: string,
): RecoveryProtectionResult {
  const reported = Array.isArray(result?.outputs)
    ? result.outputs.filter((value): value is string => nonEmptyString(value) !== null)
    : [];
  const recovery = nonEmptyString(result?.recovery) ?? fallbackRecovery;
  const primaryOutput = reported.includes(recovery)
    ? recovery
    : reported[0] ?? recovery;
  const outputs = Array.from(new Set([primaryOutput, ...reported]));
  return { primaryOutput, outputs };
}

export function recoveryResultMetrics(result: RecoveryResult): Record<string, unknown> | null {
  return recordValue(result?.metrics);
}

export function recoveryResultNumber(result: RecoveryResult, key: string): number | null {
  const value = recoveryResultMetrics(result)?.[key];
  return typeof value === "number" && Number.isFinite(value) ? value : null;
}

export function recoveryResultBoolean(result: RecoveryResult, key: string): boolean | null {
  const value = recoveryResultMetrics(result)?.[key];
  return typeof value === "boolean" ? value : null;
}

export function recoveryResultOk(result: RecoveryResult): boolean | null {
  const value = result?.ok;
  return typeof value === "boolean" ? value : null;
}

export function recoveryResultOperation(result: RecoveryResult): string | null {
  const value = result?.operation;
  return typeof value === "string" ? value : null;
}

export function recoveryResultHasNoDamage(result: RecoveryResult): boolean {
  if (
    recoveryResultBoolean(result, "no_damage") === true
    || recoveryResultBoolean(result, "all_correct") === true
  ) {
    return true;
  }
  return recoveryResultOperation(result) === "verify"
    && recoveryResultOk(result) === true
    && recoveryResultMetrics(result) === null;
}

export function recoveryResultConfirmsRepairCapacity(result: RecoveryResult): boolean {
  return !recoveryResultHasNoDamage(result)
    && recoveryResultBoolean(result, "repair_possible") === true;
}

export function recoveryRepairGate(result: RecoveryResult): RecoveryRepairGate {
  if (!result) return "verify_first";
  if (recoveryResultHasNoDamage(result)) return "no_damage";
  if (recoveryResultBoolean(result, "repair_possible") === false) return "over_capacity";
  return null;
}

export function recoveryResultTone(result: RecoveryResult): RecoveryResultTone {
  if (!result) return "neutral";
  if (recoveryResultHasNoDamage(result)) return "success";
  if (recoveryResultBoolean(result, "repair_possible") === false) return "danger";

  const operation = recoveryResultOperation(result);
  const ok = recoveryResultOk(result);
  if (operation === "repair") return ok === true ? "success" : "warning";
  if (recoveryResultBoolean(result, "repair_possible") === true) return "warning";
  return ok === true ? "success" : "warning";
}

export function latestMatchingRecoveryTask<T>(
  tasks: readonly T[],
  matches: (task: T) => boolean,
): T | null {
  for (let index = tasks.length - 1; index >= 0; index -= 1) {
    const task = tasks[index];
    if (matches(task)) return task;
  }
  return null;
}
