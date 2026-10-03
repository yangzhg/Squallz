import { ipc, type ArchiveInfo, type EntryDto, type EntryPreviewDto, type NestedArchivePasswords, type NestedArchivePreviewDto } from "./ipc";
import { createPreviewPasswordFlow } from "./preview-password.svelte";

export type PreviewOrigin = Readonly<{
  outerSource: string;
  outerDisplayPath: string;
  entryPath: string;
  virtualIndex: number | null;
}>;
export type PreviewPreparation = Readonly<{ generation: number; origin: PreviewOrigin }>;
export type PreviewFailure = {
  entryPath: string;
  entryType: EntryDto["entry_type"] | null;
  displayName: string;
  policyKind: "none" | "folder" | "nested" | "system-file";
  outerSource: string;
  outerDisplayPath: string;
  message: string;
  retryAction: "preview" | "open" | "extract";
};
type PreviewPhase = "idle" | "entry" | "nested";
type PreviewFileAction = Readonly<{
  previewGeneration: number;
  actionGeneration: number;
  previewId: string;
  archiveSource: string | null;
}>;

/** Owns prepared resources until a file is released or a nested archive is handed to the archive store. */
export class PreviewSession {
  #file = $state<EntryPreviewDto | null>(null);
  #nested = $state<NestedArchivePreviewDto | null>(null);
  #failure = $state<PreviewFailure | null>(null);
  #origin = $state<PreviewOrigin | null>(null);
  #phase = $state<PreviewPhase>("idle");
  #targetName = $state("");
  #generation = 0;
  #actionGeneration = 0;
  #preparation: PreviewPreparation | null = null;
  #preparationTail: Promise<void> = Promise.resolve();
  #passwordFlow = createPreviewPasswordFlow(ipc.cancelEntryPreview);
  #disposed = false;

  get file(): Readonly<EntryPreviewDto> | null { return this.#file; }
  get nested(): Readonly<NestedArchivePreviewDto> | null { return this.#nested; }
  get failure(): Readonly<PreviewFailure> | null { return this.#failure; }
  get origin(): PreviewOrigin | null { return this.#origin; }
  get phase(): PreviewPhase { return this.#phase; }
  get targetName(): string { return this.#targetName; }
  get passwordPrompt() { return this.#passwordFlow.prompt; }

  beginPreparation(origin: PreviewOrigin, phase: Exclude<PreviewPhase, "idle">, targetName: string): PreviewPreparation | null {
    if (this.#disposed) return null;
    this.clear();
    return this.startPreparation(origin, phase, targetName);
  }

  beginPreparedFileOpen(file: EntryPreviewDto, origin: PreviewOrigin): PreviewPreparation | null {
    if (this.#disposed || this.#file?.preview_id !== file.preview_id) return null;
    return this.startPreparation(origin, "entry", file.display_name);
  }

  isCurrent(preparation: PreviewPreparation): boolean {
    return !this.#disposed && preparation === this.#preparation && preparation.generation === this.#generation;
  }

  finishPreparation(preparation: PreviewPreparation): void {
    if (!this.isCurrent(preparation)) return;
    this.#phase = "idle";
    this.#targetName = "";
  }

  acceptFile(preparation: PreviewPreparation, file: EntryPreviewDto): boolean {
    if (!this.isCurrent(preparation)) {
      this.releaseFile(file);
      return false;
    }
    this.#file = file;
    this.#nested = null;
    this.#failure = null;
    return true;
  }

  acceptNested(preparation: PreviewPreparation, nested: NestedArchivePreviewDto): boolean {
    if (!this.isCurrent(preparation)) {
      void ipc.closeArchive(nested.archive.id).catch(() => undefined);
      return false;
    }
    this.#nested = nested;
    this.#file = null;
    this.#failure = null;
    return true;
  }

  failPreparation(preparation: PreviewPreparation, failure: PreviewFailure): void {
    if (this.isCurrent(preparation)) this.#failure = failure;
  }

  fileFor(source: string, displayPath: string, entryPath: string): Readonly<EntryPreviewDto> | null {
    const file = this.#file;
    return file && (file.outer_path === source || file.outer_path === displayPath) && file.entry_path === entryPath ? file : null;
  }

  hasPrepared(source: string, displayPath: string, entryPath: string): boolean {
    return Boolean(this.fileFor(source, displayPath, entryPath)
      || (this.#nested?.outer_path === source && this.#nested.entry_path === entryPath));
  }

  takeNested(source: string, entryPath: string): ArchiveInfo | null {
    if (this.#nested?.outer_path !== source || this.#nested.entry_path !== entryPath) return null;
    const archive = this.#nested.archive;
    this.#nested = null;
    return archive;
  }

  beginFileAction(file: EntryPreviewDto, archiveSource: string | null): PreviewFileAction | null {
    if (this.#disposed || this.#file?.preview_id !== file.preview_id) return null;
    return { previewGeneration: this.#generation, actionGeneration: ++this.#actionGeneration,
      previewId: file.preview_id, archiveSource };
  }

  fileActionIsCurrent(action: PreviewFileAction, archiveSource: string | null): boolean {
    return !this.#disposed && action.previewGeneration === this.#generation
      && action.actionGeneration === this.#actionGeneration
      && action.previewId === this.#file?.preview_id
      && action.archiveSource === archiveSource;
  }

  completeFileAction(action: PreviewFileAction, archiveSource: string | null): void {
    if (this.fileActionIsCurrent(action, archiveSource)) this.#failure = null;
  }

  failFileAction(action: PreviewFileAction, archiveSource: string | null, failure: PreviewFailure): void {
    if (this.fileActionIsCurrent(action, archiveSource)) this.#failure = failure;
  }

  async runPreparation<T>(
    preparation: PreviewPreparation,
    names: { outerName: string; innerName: string },
    operation: (passwords: NestedArchivePasswords, requestId: string) => Promise<T>,
    { serial = false } = {},
  ): Promise<T | null> {
    let releaseSlot: () => void = () => undefined;
    if (serial) {
      const previous = this.#preparationTail;
      this.#preparationTail = new Promise<void>((resolve) => { releaseSlot = resolve; });
      await previous;
    }
    try {
      if (!this.isCurrent(preparation)) return null;
      // Successful late results stay with the caller until acceptFile/acceptNested or store admission.
      return await this.#passwordFlow.run({ ...names, isCurrent: () => this.isCurrent(preparation) }, operation);
    } finally {
      releaseSlot();
    }
  }

  answerPassword(value: string): boolean {
    return !this.#disposed && this.#passwordFlow.answer(value);
  }

  clear(): PreviewOrigin | null {
    this.#passwordFlow.cancel();
    const origin = this.#origin;
    const file = this.#file;
    const nested = this.#nested;
    this.#generation += 1;
    this.#actionGeneration += 1;
    this.#preparation = null;
    this.#file = null;
    this.#nested = null;
    this.#failure = null;
    this.#origin = null;
    this.#phase = "idle";
    this.#targetName = "";
    if (file) this.releaseFile(file);
    if (nested) void ipc.closeArchive(nested.archive.id).catch(() => undefined);
    return origin;
  }

  dispose(): void {
    this.#disposed = true;
    this.clear();
  }

  private startPreparation(origin: PreviewOrigin, phase: Exclude<PreviewPhase, "idle">, targetName: string): PreviewPreparation {
    const snapshot = Object.freeze({ ...origin });
    const preparation = Object.freeze({ generation: ++this.#generation, origin: snapshot });
    this.#preparation = preparation;
    this.#origin = snapshot;
    this.#phase = phase;
    this.#targetName = targetName;
    return preparation;
  }

  private releaseFile(file: EntryPreviewDto): void {
    void ipc.releasePreviewSession(file.preview_id).catch(() => undefined);
  }
}
