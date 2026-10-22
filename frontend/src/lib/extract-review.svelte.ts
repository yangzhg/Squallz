import { BatchExtractDraft, type BatchExtractEdit } from "./batch-extract.svelte";
import { reviewNestedExtract, nestedExtractJob, type NestedExtractDraft } from "./nested-extract";
import { readBatchExtractResult } from "./batch-extract-result";
import type { ExtractWorkspacePort } from "./extract-workspace.svelte";
import { isErrorDto, type JobSpec } from "./ipc";
import { tFallback as tr, tError } from "./i18n.svelte";
import { basename, dirname } from "./format";
import { recordOperation } from "./history.svelte";
import type { TaskDialogModel } from "./task-model";

type ReviewKind = "batch" | "nested";

/** Batch and inner-archive review share native selection/submission ownership, not job schemas. */
export class ExtractReviewSession {
  readonly batch: BatchExtractDraft;
  #nested = $state<NestedExtractDraft | null>(null);
  #picker = $state.raw<{ kind: ReviewKind } | null>(null);
  #submitting = $state({ batch: false, nested: false });
  #generation = 0;
  #disposed = false;

  constructor(private readonly port: ExtractWorkspacePort) {
    this.batch = new BatchExtractDraft(() => port.read());
  }
  get nested(): Readonly<NestedExtractDraft> | null { return this.#nested; }
  locked(kind: ReviewKind): boolean { return this.#submitting[kind] || this.#picker?.kind === kind; }
  submitting(kind: ReviewKind): boolean { return this.#submitting[kind]; }
  leave(): void {
    this.#generation += 1;
    this.#picker = null;
  }
  dispose(): void {
    this.#disposed = true;
    this.leave();
  }

  replaceSources(paths: readonly string[]): boolean {
    if (this.#disposed) return false;
    if (this.locked("batch")) {
      this.waitNotice("batch");
      return false;
    }
    this.batch.replaceSources(paths);
    return true;
  }
  editBatch(change: BatchExtractEdit): void {
    if (!this.#disposed && !this.locked("batch")) this.batch.edit(change);
  }
  editNested(change: Partial<Pick<NestedExtractDraft, "dest" | "smart" | "overwrite" | "symlinks" | "best_effort">>): void {
    if (this.#disposed || !this.#nested || this.locked("nested")) return;
    this.#nested = { ...this.#nested, ...change };
    this.#generation += 1;
  }
  adoptNested(draft: NestedExtractDraft): boolean {
    if (this.#disposed) return false;
    if (this.locked("nested")) {
      this.waitNotice("nested");
      return false;
    }
    this.#nested = { ...draft };
    this.#generation += 1;
    return true;
  }
  prepareNested(outerSource: string, outerDisplayPath: string, entryPath: string): NestedExtractDraft {
    const { archive, defaultDirectory } = this.port.read();
    return reviewNestedExtract({
      kind: "extract_nested",
      outer_path: outerSource,
      entry_path: entryPath,
      dest: defaultDirectory ?? dirname(outerDisplayPath),
      overwrite: "ask",
      symlinks: "preserve",
      smart: true,
      encoding: archive?.source === outerSource ? archive.encoding_override ?? null : null,
      password: null,
      best_effort: false,
    }, outerDisplayPath);
  }
  adoptBatch(task: TaskDialogModel, displayedSpec: JobSpec): boolean {
    if (this.#disposed || task.spec.kind !== "batch_extract") return false;
    if (this.locked("batch")) {
      this.waitNotice("batch");
      return false;
    }
    const outcome = readBatchExtractResult(task.result);
    if (!this.batch.restoreTask(task.spec, task.state === "done" ? outcome.reviewFailures : null,
      task.state === "done" ? outcome.reviewCount : 0, displayedSpec.kind === "batch_extract" ? displayedSpec : task.spec)) {
      this.port.notice(tr("gui.batch.review_unavailable"));
      return false;
    }
    return true;
  }
  waitNotice(kind: ReviewKind): void {
    this.port.notice(kind === "batch"
      ? tr("gui.batch.wait_for_current")
      : tr("gui.nested_extract.wait_for_current"));
  }

  async choose(kind: ReviewKind, index: number | null = null): Promise<void> {
    const screen = kind === "batch" ? "batch" : "nestedExtract";
    if (this.#disposed || this.port.read().screen !== screen || this.locked(kind)) return;
    const batch = kind === "batch" ? this.batch.state : null;
    const nested = this.#nested;
    if (kind === "nested" ? !nested : index !== null && !batch?.items[index]) return;
    const generation = this.#generation;
    const revision = this.batch.revision;
    const request = { kind };
    this.#picker = request;
    const isCurrent = () => this.#picker === request && generation === this.#generation
      && (kind === "nested" || revision === this.batch.revision) && this.port.read().screen === screen;
    const sources = kind === "batch" && index === null;
    try {
      const selected = await this.port.choose(kind === "nested" ? "nested-extract.destination" : sources ? "batch.sources" : "batch.destination", {
        title: sources ? tr("gui.batch.add_archives") : tr("gui.batch.choose_destination"),
        multiple: sources,
        directory: !sources,
        defaultPath: kind === "nested" ? nested?.dest : index === null ? undefined : batch?.items[index]?.dest,
      }, isCurrent);
      if (!isCurrent()) return;
      const paths = Array.isArray(selected) ? selected : selected ? [selected] : [];
      if (kind === "nested" && paths[0] && nested) {
        this.#nested = { ...nested, dest: paths[0] };
        this.#generation += 1;
      } else if (batch && selected) {
        const added = this.batch.acceptSelection(paths, index, batch);
        if (sources && added === 0) this.port.notice(tr("gui.batch.already_added"));
      } else {
        this.port.notice(kind === "batch"
          ? tr("gui.batch.selection_cancelled")
          : tr("gui.nested_extract.selection_cancelled"));
      }
    } catch {
      if (isCurrent()) this.port.notice(sources
        ? tr("gui.batch.picker_unavailable")
        : tr("gui.batch.destination_picker_unavailable"));
    } finally {
      if (this.#picker === request) this.#picker = null;
    }
  }

  async start(kind: ReviewKind): Promise<boolean> {
    if (this.#disposed || this.locked(kind)) return false;
    const nested = this.#nested;
    if (kind === "nested" && !nested) return false;
    const spec = kind === "nested" && nested ? nestedExtractJob(nested) : this.batch.snapshotRun();
    const destinations = spec.kind === "batch_extract" ? spec.items.map((item) => item.dest) : [spec.dest];
    if (destinations.length === 0) {
      this.port.notice(tr("gui.batch.open_archives_before_start"));
      return false;
    }
    const invalid = destinations.findIndex((dest) => !dest.trim());
    if (invalid !== -1) {
      this.port.notice(kind === "batch" ? tr("gui.batch.destination_required")
        : tr("gui.nested_extract.destination_required"));
      this.port.focusDestination(invalid);
      return false;
    }
    if (this.port.blocked()) return false;
    const generation = this.#generation;
    const screen = this.port.read().screen;
    const isCurrent = () => generation === this.#generation && this.port.read().screen === screen;
    this.#submitting[kind] = true;
    try {
      await this.port.submit(spec);
      if (spec.kind === "batch_extract") {
        const count = spec.items.length.toLocaleString();
        if (isCurrent()) this.port.notice(tr("gui.batch.extract_job_queued").replace("{count}", count));
        recordOperation({
          status: "queued",
          title: tr("gui.batch.extract_queued"),
          detail: tr("gui.batch.archive_count").replace("{count}", count),
        });
      } else {
        if (isCurrent()) this.port.notice(tr("gui.preview.extract_nested_queued").replace("{name}", basename(spec.entry_path)));
        recordOperation({
          status: "queued",
          title: tr("gui.preview.nested_extract_queued_operation_title"),
          detail: `${basename(spec.entry_path)} -> ${basename(spec.dest)}`,
        });
      }
      return true;
    } catch (error) {
      if (isCurrent() && !this.port.isSubmitBlocked(error)) this.port.notice(isErrorDto(error) ? tError(error) : kind === "batch"
        ? tr("gui.batch.requires_desktop_service")
        : tr("gui.nested_extract.submit_failed"));
      return false;
    } finally {
      this.#submitting[kind] = false;
    }
  }
}
