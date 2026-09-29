import type { JobSpec } from "./ipc";
import { sameDesktopPath } from "./desktop-path";

type BatchJob = Extract<JobSpec, { kind: "batch_extract" }>;
export type BatchExtractItemDraft = Omit<BatchJob["items"][number], "password"> & { displayPath: string };
export type BatchExtractDraft = Omit<BatchJob, "kind" | "items"> & {
  items: BatchExtractItemDraft[];
};

export function batchExtractJob(draft: BatchExtractDraft): BatchJob {
  return { kind: "batch_extract", overwrite: draft.overwrite, symlinks: draft.symlinks,
    smart: draft.smart, items: draft.items.map((item) => ({
      path: item.path, dest: item.dest, encoding: item.encoding,
      best_effort: item.best_effort, password: null,
    })) };
}

export function reviewBatchExtract(
  spec: BatchJob,
  failures: unknown[] | null,
  failedCount: number,
  platform: "macos" | "windows" | "linux",
  displayedSpec: BatchJob = spec,
): BatchExtractDraft | null {
  if (spec.items.length !== displayedSpec.items.length) return null;
  let items = spec.items.map((item, index) => ({ ...item, displayPath: displayedSpec.items[index].path }));
  if (failures !== null) {
    if (failedCount < 1 || failures.length !== failedCount) return null;
    const selected = new Set<number>();
    for (const failure of failures) {
      if (!failure || typeof failure !== "object" || !("archive" in failure)
        || typeof failure.archive !== "string") return null;
      const archive = failure.archive;
      const matches = items.flatMap((item, index) =>
        sameDesktopPath(item.displayPath, archive, platform) ? [index] : []);
      // A path-only result cannot disambiguate repeated inputs with different destinations.
      if (matches.length !== 1 || selected.has(matches[0])) return null;
      selected.add(matches[0]);
    }
    items = items.filter((_, index) => selected.has(index));
  }
  if (items.length === 0) return null;
  return { overwrite: spec.overwrite, symlinks: spec.symlinks, smart: spec.smart,
    items: items.map((item) => ({ path: item.path, displayPath: item.displayPath, dest: item.dest,
      encoding: item.encoding, best_effort: item.best_effort })) };
}
