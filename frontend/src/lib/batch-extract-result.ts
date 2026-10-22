import { resultOf, type JobResult, type BatchExtractOutput } from "./job-result";
import { readExtractResultOutcome } from "./extract-result";
import { t } from "./i18n.svelte";

export function readBatchExtractResult(result: JobResult | null | undefined) {
  const report = resultOf(result, "batch_extract");
  const outputs = report?.outputs ?? [];
  const incompleteOutputs: BatchExtractOutput[] = [];
  const partialOutputs: BatchExtractOutput[] = [];
  let failedEntries = 0;
  let skippedEntries = 0;
  for (const output of outputs) {
    const outcome = readExtractResultOutcome(output);
    failedEntries += outcome.failed;
    skippedEntries += outcome.skipped;
    if (outcome.failed > 0 || outcome.skipped > 0) incompleteOutputs.push(output);
    if (outcome.failed > 0) partialOutputs.push(output);
  }
  const failures = report?.failures ?? [];
  const failedArchives = report?.failed ?? 0;
  return {
    failedArchives,
    completeArchives: Math.max(0, (report?.extracted ?? 0) - incompleteOutputs.length),
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

export function batchExtractResultSummary(result: JobResult | null | undefined, fallbackTotal: number): string {
  const report = resultOf(result, "batch_extract");
  const total = report?.archives ?? fallbackTotal;
  const selected = report?.selected_archives ?? total;
  const outcome = readBatchExtractResult(result);
  const summary = t(selected > total ? "gui.task.result_batch_extract_grouped" : "gui.task.result_batch_extract", {
    extracted: outcome.completeArchives, incomplete: outcome.incompleteArchives, total, selected, failed: outcome.failedArchives,
  });
  return outcome.failedEntries > 0 || outcome.skippedEntries > 0
    ? `${summary} · ${batchExtractEntrySummary(outcome.failedEntries, outcome.skippedEntries)}`
    : summary;
}
