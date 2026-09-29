import { readExtractResultOutcome } from "./extract-result";
import { t } from "./i18n.svelte";

function count(value: unknown): number {
  return typeof value === "number" && Number.isFinite(value) ? Math.max(0, Math.trunc(value)) : 0;
}

export function readBatchExtractResult(result: Record<string, unknown> | null | undefined) {
  const outputs = Array.isArray(result?.outputs)
    ? result.outputs.filter((item): item is Record<string, unknown> =>
      item !== null && typeof item === "object" && !Array.isArray(item))
    : [];
  const incompleteOutputs: Record<string, unknown>[] = [];
  const partialOutputs: Record<string, unknown>[] = [];
  let failedEntries = 0;
  let skippedEntries = 0;
  for (const output of outputs) {
    const outcome = readExtractResultOutcome(output);
    failedEntries += outcome.failed;
    skippedEntries += outcome.skipped;
    if (outcome.failed > 0 || outcome.skipped > 0) incompleteOutputs.push(output);
    if (outcome.failed > 0) partialOutputs.push(output);
  }
  const failures: unknown[] = Array.isArray(result?.failures) ? result.failures : [];
  const failedArchives = count(result?.failed);
  return {
    failedArchives,
    completeArchives: Math.max(0, count(result?.extracted) - incompleteOutputs.length),
    incompleteArchives: incompleteOutputs.length,
    failedEntries,
    skippedEntries,
    failures,
    incompleteOutputs,
    reviewFailures: [...failures, ...partialOutputs],
    reviewCount: failedArchives + partialOutputs.length,
  };
}

export function batchExtractEntrySummary(failed: number, skipped: number): string {
  return t("gui.task.batch_entry_outcome", { failed, skipped });
}

export function batchExtractResultSummary(result: Record<string, unknown> | null | undefined, fallbackTotal: number): string {
  const total = typeof result?.archives === "number" ? count(result.archives) : fallbackTotal;
  const selected = typeof result?.selected_archives === "number" ? count(result.selected_archives) : total;
  const outcome = readBatchExtractResult(result);
  const summary = t(selected > total ? "gui.task.result_batch_extract_grouped" : "gui.task.result_batch_extract", {
    extracted: outcome.completeArchives, incomplete: outcome.incompleteArchives, total, selected, failed: outcome.failedArchives,
  });
  return outcome.failedEntries > 0 || outcome.skippedEntries > 0
    ? `${summary} · ${batchExtractEntrySummary(outcome.failedEntries, outcome.skippedEntries)}`
    : summary;
}
