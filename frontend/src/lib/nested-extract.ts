import type { JobSpec } from "./ipc";

type NestedJob = Extract<JobSpec, { kind: "extract_nested" }>;
export type NestedExtractDraft = Omit<NestedJob, "password"> & { outerDisplayPath: string };

export function reviewNestedExtract(spec: NestedJob, outerDisplayPath: string): NestedExtractDraft {
  return { kind: "extract_nested", outer_path: spec.outer_path, outerDisplayPath,
    entry_path: spec.entry_path, dest: spec.dest, overwrite: spec.overwrite,
    symlinks: spec.symlinks, smart: spec.smart, encoding: spec.encoding,
    best_effort: spec.best_effort };
}

export function nestedExtractJob(draft: NestedExtractDraft): NestedJob {
  return { kind: "extract_nested", outer_path: draft.outer_path, entry_path: draft.entry_path,
    dest: draft.dest, overwrite: draft.overwrite, symlinks: draft.symlinks,
    smart: draft.smart, encoding: draft.encoding, best_effort: draft.best_effort, password: null };
}
