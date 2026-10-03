import { archiveEditPathIssue, archiveSelectionRoots, normalizeArchivePath } from "./archive-editing";
import { basename } from "./format";
import { ipc, isErrorDto, type ErrorDto, type JobSpec } from "./ipc";

export type ArchiveEditKind = "rename" | "move" | "new-folder";
type EditingArchive = Readonly<{ id: number; source: string; encoding_override: string | null }>;
export type ArchiveEditContext = Readonly<{
  archive: EditingArchive | null;
  generation: number;
  opening: boolean;
  directory: string;
  selectedPaths: ReadonlySet<string>;
  mutationDisabledReason: string;
}>;
export type ArchiveEditOwner = Readonly<{
  kind: ArchiveEditKind;
  source: string;
  encoding: string | null;
  generation: number;
  id: number;
  directory: string;
  paths: ReadonlySet<string>;
}>;
type MovePlanItem = Readonly<{
  from: string;
  to: string;
  conflict: boolean;
  reason: string | null;
  keepBothTo: string | null;
}>;
export type ArchiveMoveReview = Readonly<{
  archiveId: number;
  generation: number;
  targetDir: string;
  items: readonly MovePlanItem[];
  view: Readonly<{ targetDir: string; readyCount: number; items: readonly MovePlanItem[] }>;
}>;
export type ArchiveEditEffect =
  | { kind: "editorClosed"; owner: ArchiveEditOwner }
  | { kind: "reviewCreated"; editor: ArchiveEditOwner; review: ArchiveMoveReview }
  | { kind: "reviewCleared"; review: ArchiveMoveReview; restoreFocus: boolean }
  | { kind: "notice"; message: string }
  | { kind: "queued"; title: string; detail: string };
type UpdateJob = Extract<JobSpec, { kind: "update" }>;
type QueueArchiveEdit = (
  spec: JobSpec,
  notice: string,
  missingArchiveNotice: string,
  onFailure?: (message: string) => void,
) => Promise<boolean>;

/** Owns archive-edit intent; the host owns admission, task submission and DOM focus. */
export class ArchiveEditSession {
  #draft = $state.raw<ArchiveEditOwner | null>(null);
  #review = $state.raw<ArchiveMoveReview | null>(null);
  #target = $state("");
  #checking = $state(false);
  #error = $state<string | null>(null);
  #lastMoveTarget = "moved/";
  #disposed = false;

  constructor(
    private readonly readContext: () => ArchiveEditContext,
    private readonly queue: QueueArchiveEdit,
    private readonly blockSelectionAction: () => boolean,
    private readonly presentation: {
      tr: (key: string, fallback: string) => string;
      tError: (error: ErrorDto) => string;
      emit: (effect: ArchiveEditEffect) => void;
    },
  ) {}

  get owner() { return this.#draft; }
  get kind() { return this.#draft?.kind ?? null; }
  get source() { return this.#draft?.source ?? null; }
  get target() { return this.#target; }
  get checking() { return this.#checking; }
  get error() { return this.#error; }
  get review() { return this.#review; }
  get reviewView() { return this.#review?.view ?? null; }

  open(kind: ArchiveEditKind): ArchiveEditOwner | null {
    if (this.#disposed) return null;
    const context = this.readContext();
    if (!context.archive) return null;
    const archive = context.archive;
    const owner: ArchiveEditOwner = {
      kind, source: archive.source, encoding: archive.encoding_override,
      generation: context.generation, id: archive.id, directory: context.directory,
      paths: new Set(context.selectedPaths),
    };
    this.clearReview(false);
    this.#draft = owner;
    this.#target = kind === "move" ? this.#lastMoveTarget
      : kind === "rename" ? basename(this.renameSource(owner)?.replace(/\/+$/g, "") ?? "") : "";
    this.#checking = false;
    this.#error = null;
    return owner;
  }

  setTarget(value: string): void {
    if (!this.#draft) return;
    this.#target = value;
    if (this.kind === "move") this.#lastMoveTarget = value;
    this.#error = null;
  }

  close(): void {
    const owner = this.#draft;
    if (!owner) return;
    this.#draft = null;
    this.#checking = false;
    this.#error = null;
    this.presentation.emit({ kind: "editorClosed", owner });
  }

  cancelReview(): void { this.clearReview(true); }

  dispose(): void {
    this.#disposed = true;
    this.#draft = null;
    this.#review = null;
    this.#checking = false;
    this.#error = null;
  }

  browseContextChanged(): void {
    if (this.#draft) return;
    this.#error = null;
    this.clearReview(false);
  }

  get blockedReason(): string {
    const owner = this.#draft;
    if (!owner) return "";
    const context = this.readContext();
    if (!context.archive || context.opening || context.archive.source !== owner.source
      || context.archive.encoding_override !== owner.encoding || context.generation !== owner.generation) {
      return this.tr("gui.edit.archive_changed", "The archive changed. Your text is kept. Cancel and reopen the editor for the intended archive.");
    }
    return context.mutationDisabledReason;
  }

  get targetPath(): string {
    const owner = this.#draft;
    if (!owner) return "";
    const input = this.#target.trim().replaceAll("\\", "/");
    if (owner.kind === "move") {
      const path = normalizeArchivePath(input);
      return path ? `${path}/` : "";
    }
    if (owner.kind === "new-folder") {
      const name = normalizeArchivePath(input, this.tr("gui.new_folder.default_name", "New Folder"));
      const parent = input.startsWith("/") ? "" : owner.directory;
      return `${parent ? `${parent}/` : ""}${name}/`;
    }
    const source = this.renameSource(owner);
    const path = normalizeArchivePath(input);
    if (!path) return "";
    if (!source || input.includes("/")) return source?.endsWith("/") ? `${path}/` : path;
    const clean = source.replace(/\/$/u, "");
    const slash = clean.lastIndexOf("/");
    return `${slash >= 0 ? clean.slice(0, slash + 1) : ""}${path}${source.endsWith("/") ? "/" : ""}`;
  }

  get status(): string {
    const owner = this.#draft;
    if (!owner || !this.readContext().archive) return this.tr("gui.empty.open_archive_first", "Open archive");
    const target = this.targetPath;
    if (owner.kind === "rename") {
      const from = this.renameSource(owner);
      if (from === null) return this.tr("gui.rename.select_one_entry", "Select exactly one file or folder to rename");
      if (target === from) return this.unchangedName();
      const issue = this.renameIssue(from, target);
      if (issue.blocking) return this.tr("gui.rename.blocked_reason", "Blocked: {reason}").replace("{reason}", issue.blocking);
      return `${issue.warning ? `${issue.warning} · ` : ""}${from} -> ${target}` + this.destinationCheckSuffix();
    }
    const problem = owner.kind === "move" ? this.moveProblem(owner, target) : this.pathProblem(target);
    if (problem) return problem;
    if (owner.kind === "new-folder") {
      return this.tr("gui.new_folder.create_path", "Create {folder}").replace("{folder}", target) + this.destinationCheckSuffix();
    }
    if (!owner.paths.size) {
      return this.tr("gui.move.select_entries_to_move_into", "Select entries to move into {target}").replace("{target}", target || "/");
    }
    return this.tr("gui.move.selected_to_target", "{count} selected -> {target}")
      .replace("{count}", owner.paths.size.toLocaleString()).replace("{target}", target || "/")
      + this.tr("gui.move.check_before_submit", " · all target names will be checked before moving");
  }

  async submit(): Promise<void> {
    const owner = this.#draft;
    if (!owner || this.#checking) return;
    const archiveId = this.readContext().archive?.id;
    const target = this.targetPath;
    if (!await this.validateOriginalIntent(owner, archiveId, target) || !this.checkCurrent(owner, archiveId, target)) return;
    if (owner.kind !== "new-folder" && this.blockSelectionAction()) return;
    const archive = this.readContext().archive;
    if (!archive) return;
    if (owner.kind === "move") {
      await this.checkMove(owner, archive, target);
      return;
    }
    const from = this.renameSource(owner);
    if (owner.kind === "rename" && from === null) {
      this.notice(this.tr("gui.precondition.select_one_before_rename", "Select exactly one file or folder before renaming"));
      return;
    }
    const problem = owner.kind === "rename"
      ? target === from ? this.unchangedName() : this.renameIssue(from!, target).blocking
      : this.pathProblem(target);
    if (problem) { this.#error = problem; return; }
    if (owner.kind === "new-folder") this.#target = this.#target.trim().replaceAll("\\", "/");
    if (!await this.validateTarget(owner, archive.id, target) || !this.checkCurrent(owner, archiveId, target)) return;
    if (owner.kind === "rename") {
      await this.submitUpdate(archive, { rename: [{ from: from!, to: target }], mkdir: [] },
        this.tr("gui.rename.queued_notice", "Rename queued: {from} -> {to}").replace("{from}", from!).replace("{to}", target),
        this.tr("gui.precondition.open_before_rename", "Open an archive before renaming entries"),
        this.tr("gui.rename.queued", "Rename entry queued"), `${from} -> ${target}`);
    } else {
      await this.submitUpdate(archive, { rename: [], mkdir: [target] },
        this.tr("gui.new_folder.queued_notice", "New folder queued: {folder}").replace("{folder}", target),
        this.tr("gui.precondition.open_before_new_folder", "Open an archive before creating a folder"),
        this.tr("gui.new_folder.queued", "New folder queued"), target);
    }
  }

  async submitReviewedMove(decision: "ready-only" | "keep-both"): Promise<void> {
    const review = this.#review;
    if (!review) return;
    const rename = decision === "ready-only"
      ? review.items.filter((item) => !item.conflict).map(({ from, to }) => ({ from, to }))
      : review.items.map((item) => ({ from: item.from, to: item.conflict && item.keepBothTo ? item.keepBothTo : item.to }));
    await this.submitMovePlan(rename, review.targetDir);
  }

  private async validateOriginalIntent(owner: ArchiveEditOwner, id: number | undefined, target: string): Promise<boolean> {
    if (!this.checkCurrent(owner, id, target)) return false;
    if (id === owner.id) return true;
    const paths = owner.kind === "new-folder"
      ? this.#target.trim().replaceAll("\\", "/").startsWith("/") || !owner.directory ? [] : [`${owner.directory}/`]
      : archiveSelectionRoots(owner.paths);
    this.#checking = true;
    this.#error = null;
    try {
      const missing = paths.length ? await ipc.missingArchivePaths(id!, paths) : [];
      if (!this.checkCurrent(owner, id, target)) return false;
      if (missing.length) { this.#error = this.missingItems(missing); return false; }
      return true;
    } catch (error) {
      if (this.checkCurrent(owner, id, target)) this.#error = this.tr("gui.edit.check_failed", "Could not check the original items. Your text is kept. Try again.") + this.errorDetail(error);
      return false;
    } finally {
      if (this.#draft === owner) this.#checking = false;
    }
  }

  private checkCurrent(owner: ArchiveEditOwner, id: number | undefined, target: string): boolean {
    if (this.#draft !== owner || this.targetPath !== target) return false;
    const blocked = this.blockedReason;
    if (id !== undefined && this.readContext().archive?.id === id && !blocked) return true;
    this.#error = blocked || this.tr("gui.edit.check_changed", "The archive changed while checking. Your text is kept. Try again.");
    return false;
  }

  private async validateTarget(owner: ArchiveEditOwner, id: number, target: string): Promise<boolean> {
    this.#checking = true;
    this.#error = null;
    try {
      const inspection = await ipc.inspectArchiveTarget(id, target);
      if (!this.checkCurrent(owner, id, target)) return false;
      if (inspection.blocked_parent) { this.#error = this.parentIsFile(inspection.blocked_parent); return false; }
      if (inspection.exists) {
        this.#error = this.tr("gui.edit.target_exists", "{path} already exists. Choose a different name or path.").replace("{path}", target);
        return false;
      }
      return true;
    } catch (error) {
      if (this.checkCurrent(owner, id, target)) this.#error = this.tr("gui.edit.target_check_failed", "Could not check the destination. Your text is kept. Try again.") + this.errorDetail(error);
      return false;
    } finally {
      if (this.#draft === owner) this.#checking = false;
    }
  }

  private async checkMove(owner: ArchiveEditOwner, archive: EditingArchive, targetDir: string): Promise<void> {
    if (!owner.paths.size) { this.notice(this.tr("gui.precondition.select_entries_before_move", "Select entries before moving")); return; }
    const problem = this.moveProblem(owner, targetDir);
    if (problem) { this.#error = problem; return; }
    this.#checking = true;
    this.#error = null;
    try {
      const checked = await ipc.planArchiveMove(archive.id, archiveSelectionRoots(owner.paths), targetDir);
      if (!this.checkCurrent(owner, archive.id, targetDir)) return;
      if (checked.missing_sources.length) { this.#error = this.missingItems(checked.missing_sources); return; }
      if (checked.blocked_parent) { this.#error = this.parentIsFile(checked.blocked_parent); return; }
      const items = checked.items.map((item): MovePlanItem => ({
        from: item.from, to: item.to, conflict: item.conflict !== null,
        reason: item.conflict === "existing_target" ? this.tr("gui.move.target_already_exists", "Target already exists")
          : item.conflict === "duplicate_target" ? this.tr("gui.move.duplicate_target_name", "Multiple selected entries share this target name") : null,
        keepBothTo: item.keep_both_to,
      }));
      const conflicts = items.filter((item) => item.conflict);
      if (conflicts.length) {
        const review: ArchiveMoveReview = {
          archiveId: archive.id, generation: owner.generation, targetDir, items,
          view: { targetDir, readyCount: items.length - conflicts.length, items: conflicts },
        };
        this.#review = review;
        this.presentation.emit({ kind: "reviewCreated", editor: owner, review });
        this.close();
        return;
      }
      this.#checking = false;
      await this.submitMovePlan(items.map(({ from, to }) => ({ from, to })), targetDir);
    } catch (error) {
      if (this.checkCurrent(owner, archive.id, targetDir)) this.#error = this.tr("gui.move.check_failed", "Could not check move targets. Your destination is kept. Try again.") + this.errorDetail(error);
    } finally {
      if (this.#draft === owner) this.#checking = false;
    }
  }

  private async submitMovePlan(rename: UpdateJob["rename"], targetDir: string): Promise<void> {
    if (this.blockSelectionAction()) return;
    const context = this.readContext();
    const review = this.#review;
    if (review && (context.archive?.id !== review.archiveId || context.generation !== review.generation || context.opening)) {
      this.clearReview(false);
      this.notice(this.tr("gui.move.review_changed", "The archive changed. Select the items and check the move targets again."));
      return;
    }
    if (!context.archive) { this.notice(this.tr("gui.precondition.open_before_move", "Open an archive before moving entries")); return; }
    if (context.mutationDisabledReason) { this.notice(context.mutationDisabledReason); return; }
    if (!rename.length) { this.notice(this.tr("gui.move.no_non_conflicting_targets", "No non-conflicting move targets to submit")); return; }
    await this.submitUpdate(context.archive, { rename, mkdir: [] },
      rename.length === 1 ? this.tr("gui.move.operation_queued", "1 move operation queued")
        : this.tr("gui.move.operations_queued", "{count} move operations queued").replace("{count}", rename.length.toLocaleString()),
      this.tr("gui.precondition.open_before_move", "Open an archive before moving entries"),
      this.tr("gui.move.queued", "Move entries queued"),
      this.tr("gui.move.entries_to_target", "{count} entries to {target}").replace("{count}", rename.length.toLocaleString()).replace("{target}", targetDir || "/"));
  }

  private async submitUpdate(archive: EditingArchive, intent: Pick<UpdateJob, "rename" | "mkdir">,
    notice: string, missingArchiveNotice: string, title: string, detail: string): Promise<void> {
    const owner = this.#draft;
    const review = this.#review;
    this.#error = null;
    const queued = await this.queue({
      kind: "update", path: archive.source, expected_archive_id: archive.id, encoding: archive.encoding_override,
      add: [], delete: [], ...intent, excludes: [], content_policy: "keep_all_files", password: null, level: 6,
    }, notice, missingArchiveNotice, owner ? (message) => {
      if (this.#draft === owner) this.#error = message;
    } : undefined);
    if (!queued) return;
    if (this.#review === review) this.clearReview(false);
    if (owner && this.#draft === owner) this.close();
    this.presentation.emit({ kind: "queued", title, detail });
  }

  private clearReview(restoreFocus: boolean): void {
    const review = this.#review;
    if (!review) return;
    this.#review = null;
    this.presentation.emit({ kind: "reviewCleared", review, restoreFocus });
  }

  private renameSource(owner: ArchiveEditOwner): string | null { return owner.paths.size === 1 ? [...owner.paths][0] : null; }
  private tr(key: string, fallback: string): string { return this.presentation.tr(key, fallback); }
  private notice(message: string): void { this.presentation.emit({ kind: "notice", message }); }
  private errorDetail(error: unknown): string { return isErrorDto(error) ? ` ${this.presentation.tError(error)}` : ""; }
  private unchangedName(): string { return this.tr("gui.rename.target_must_differ", "The name is unchanged. Enter a different name or path."); }
  private destinationCheckSuffix(): string { return this.tr("gui.edit.destination_check_suffix", " · destination will be checked before queuing"); }
  private missingItems(paths: readonly string[]): string {
    return this.tr("gui.edit.items_missing", "These original items no longer exist: {paths}. Your text is kept. Cancel and select the intended items again.").replace("{paths}", paths.join(", "));
  }
  private parentIsFile(path: string): string {
    return this.tr("gui.edit.parent_is_file", "{path} is a file and cannot be used as a parent folder. Choose a different path.").replace("{path}", path);
  }

  private pathProblem(path: string, allowRoot = false): string {
    const issue = archiveEditPathIssue(path, allowRoot);
    if (!issue) return "";
    if (issue.kind === "empty") return this.tr("gui.edit.path_empty", "Enter an archive path.");
    if (issue.kind === "absolute") return this.tr("gui.edit.path_absolute", "Enter a relative path inside the archive without a leading slash.");
    if (issue.kind === "parent") return this.tr("gui.edit.path_parent", "Parent references (..) are not allowed. Enter a path inside the archive.");
    if (issue.kind === "characters") return this.tr("gui.edit.path_characters", "{name} contains characters that cannot be used on Windows. Remove them or choose another name.").replace("{name}", issue.segment);
    if (issue.kind === "trailing") return this.tr("gui.edit.path_trailing", "{name} ends with a space or dot. Choose a portable name.").replace("{name}", issue.segment);
    return this.tr("gui.edit.path_reserved", "{name} is reserved on Windows. Choose another name.").replace("{name}", issue.segment);
  }

  private renameIssue(source: string, target: string): { blocking: string | null; warning: string | null } {
    const blocking = this.pathProblem(target);
    if (blocking) return { blocking, warning: null };
    if (source.endsWith("/")) return {
      blocking: target.startsWith(source) ? this.tr("gui.move.inside_source", "Choose a destination outside the selected folder.") : null,
      warning: null,
    };
    const extension = (path: string) => {
      const base = basename(path);
      const dot = base.lastIndexOf(".");
      return dot > 0 && dot < base.length - 1 ? base.slice(dot) : "";
    };
    const from = extension(source), to = extension(target);
    return { blocking: null, warning: from.toLowerCase() === to.toLowerCase() ? null
      : this.tr("gui.rename.extension_change", "Extension changes {from} → {to}")
        .replace("{from}", from || this.tr("gui.rename.no_extension", "no extension"))
        .replace("{to}", to || this.tr("gui.rename.no_extension", "no extension")) };
  }

  private moveProblem(owner: ArchiveEditOwner, targetDir: string): string {
    const problem = this.pathProblem(targetDir, true);
    if (problem) return problem;
    for (const source of archiveSelectionRoots(owner.paths)) {
      if (source.endsWith("/") && targetDir.startsWith(source)) return this.tr("gui.move.inside_source", "Choose a destination outside the selected folder.");
      const isDir = source.endsWith("/");
      if (`${targetDir}${basename(source.replace(/\/$/u, ""))}${isDir ? "/" : ""}` === source) {
        return this.tr("gui.move.same_location", "An entry is already in this folder. Choose another destination.");
      }
    }
    return "";
  }
}
