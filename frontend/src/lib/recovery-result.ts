import type { JobResult } from "./job-result";

export type RecoveryResult = JobResult | null;

export function recoveryReport(result: RecoveryResult) {
  return result?.kind === "protect" || result?.kind === "verify_recovery" || result?.kind === "repair_recovery" ? result : null;
}

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

export function readRecoveryProtectionResult(
  result: RecoveryResult,
  fallbackRecovery: string,
): RecoveryProtectionResult {
  const report = recoveryReport(result);
  const reported = report?.outputs ?? [];
  const recovery = report?.recovery ?? fallbackRecovery;
  const primaryOutput = reported.includes(recovery)
    ? recovery
    : reported[0] ?? recovery;
  const outputs = Array.from(new Set([primaryOutput, ...reported]));
  return { primaryOutput, outputs };
}

export function recoveryResultMetrics(result: RecoveryResult) { return recoveryReport(result)?.metrics ?? null; }
export function recoveryResultNumber(result: RecoveryResult, key: "blocks_needed" | "recovery_blocks_available" | "blocks_repaired" | "files_repaired"): number | null { return recoveryResultMetrics(result)?.[key] ?? null; }
export function recoveryResultBoolean(result: RecoveryResult, key: "no_damage" | "all_correct" | "repair_possible"): boolean | null { return recoveryResultMetrics(result)?.[key] ?? null; }
export function recoveryResultOk(result: RecoveryResult): boolean | null { return recoveryReport(result)?.ok ?? null; }
export function recoveryResultOperation(result: RecoveryResult): string | null { return recoveryReport(result)?.operation ?? null; }

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
