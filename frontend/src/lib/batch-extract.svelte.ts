import type { JobSpec } from "./ipc";
import { sameDesktopPath, type DesktopPathPlatform } from "./desktop-path";
import { createSourcePaths, mergeCreateSources } from "./create-sources";
import { dirname } from "./format";

type BatchJob = Extract<JobSpec, { kind: "batch_extract" }>;
type BatchExtractItemDraft = Readonly<Omit<BatchJob["items"][number], "password">> & { readonly displayPath: string };
export type BatchExtractDraftState = Readonly<Omit<BatchJob, "kind" | "items">> & {
  readonly items: readonly BatchExtractItemDraft[];
};
type BatchExtractContext = {
  fallbackPaths: readonly string[];
  archive: { source: string; path: string; encoding_override?: string | null } | null;
  defaultDirectory: string | null;
  platform: DesktopPathPlatform;
};
export type BatchExtractEdit =
  | { kind: "smart"; value: boolean }
  | { kind: "overwrite"; value: BatchJob["overwrite"] }
  | { kind: "symlinks"; value: BatchJob["symlinks"] }
  | { kind: "destination"; index: number; value: string }
  | { kind: "remove"; index: number };

/** Owns batch draft writes and revision; the host admits native replies and submits jobs. */
export class BatchExtractDraft {
  #draft = $state<BatchExtractDraftState | null>(null);
  #revision = 0;

  constructor(private readonly context: () => BatchExtractContext) {}

  get state(): BatchExtractDraftState {
    if (this.#draft !== null) return this.#draft;
    const context = this.context();
    return this.#newDraft(context.fallbackPaths, context);
  }

  get revision(): number { return this.#revision; }

  replaceSources(paths: readonly string[]): void {
    this.#commit(this.#newDraft(paths, this.context()));
  }

  edit(change: BatchExtractEdit): void {
    this.#commit(this.#edited(this.state, change));
  }

  acceptSelection(paths: readonly string[], index: number | null, captured: BatchExtractDraftState): number {
    if (index !== null) {
      // An empty native selection still advances revision without materializing the fallback.
      const draft = paths[0]
        ? this.#edited(captured, { kind: "destination", index, value: paths[0] })
        : this.#draft;
      this.#commit(draft);
      return 0;
    }
    const context = this.context();
    const additions = this.#newDraft(paths, context).items.filter((item) =>
      !captured.items.some((existing) => sameDesktopPath(existing.path, item.path, context.platform)));
    this.#commit({ ...captured, items: [...captured.items, ...additions] });
    return additions.length;
  }

  restoreTask(spec: BatchJob, failures: unknown[] | null, failedCount: number, displayedSpec: BatchJob): boolean {
    if (spec.items.length !== displayedSpec.items.length) return false;
    const platform = this.context().platform;
    let items = spec.items.map((item, index) => ({ ...item, displayPath: displayedSpec.items[index].path }));
    if (failures !== null) {
      if (failedCount < 1 || failures.length !== failedCount) return false;
      const selected = new Set<number>();
      for (const failure of failures) {
        if (!failure || typeof failure !== "object" || !("archive" in failure)
          || typeof failure.archive !== "string") return false;
        const archive = failure.archive;
        const matches = items.flatMap((item, index) =>
          sameDesktopPath(item.displayPath, archive, platform) ? [index] : []);
        // A path-only result cannot disambiguate repeated inputs with different destinations.
        if (matches.length !== 1 || selected.has(matches[0])) return false;
        selected.add(matches[0]);
      }
      items = items.filter((_, index) => selected.has(index));
    }
    if (items.length === 0) return false;
    this.#commit({ overwrite: spec.overwrite, symlinks: spec.symlinks, smart: spec.smart,
      items: items.map((item) => ({ path: item.path, displayPath: item.displayPath, dest: item.dest,
        encoding: item.encoding, best_effort: item.best_effort })) });
    return true;
  }

  snapshotRun(): BatchJob {
    const draft = this.state;
    return { kind: "batch_extract", overwrite: draft.overwrite, symlinks: draft.symlinks,
      smart: draft.smart, items: draft.items.map((item) => ({
        path: item.path, dest: item.dest, encoding: item.encoding,
        best_effort: item.best_effort, password: null,
      })) };
  }

  #commit(draft: BatchExtractDraftState | null): void {
    this.#draft = draft;
    this.#revision += 1;
  }

  #edited(draft: BatchExtractDraftState, change: BatchExtractEdit): BatchExtractDraftState {
    if (change.kind === "destination") {
      return { ...draft, items: draft.items.map((item, index) =>
        index === change.index ? { ...item, dest: change.value } : item) };
    }
    if (change.kind === "remove") {
      return { ...draft, items: draft.items.filter((_, index) => index !== change.index) };
    }
    return { ...draft, [change.kind]: change.value };
  }

  #newDraft(paths: readonly string[], context: BatchExtractContext): BatchExtractDraftState {
    const sources = createSourcePaths(mergeCreateSources(
      [], paths.map((path) => ({ path, kind: "unknown" })), context.platform,
    ));
    return {
      overwrite: "ask", symlinks: "preserve", smart: true,
      items: sources.map((path) => {
        const displayPath = context.archive?.source === path ? context.archive.path : path;
        return {
          path, displayPath, dest: context.defaultDirectory ?? dirname(displayPath),
          encoding: context.archive && sameDesktopPath(context.archive.source, path, context.platform)
            ? context.archive.encoding_override ?? null : null,
          best_effort: false,
        };
      }),
    };
  }
}
