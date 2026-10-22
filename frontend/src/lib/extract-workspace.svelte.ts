import { ExtractOptionsDraft, type ExtractDestinationMode, type ExtractScope, type ExtractTaskDraft } from "./extract-options-draft.svelte";
import { ExtractPlanSession, type ExtractPlanInput } from "./extract-plan.svelte";
import { ExtractReviewSession } from "./extract-review.svelte";
import type { ArchiveInfo, ExtractArchivePresetOptions, ExtractPlanPreflightDto, JobSpec } from "./ipc";
import type { DesktopPathPlatform } from "./desktop-path";
import { dirname, formatBytes } from "./format";
import { tFallback as tr } from "./i18n.svelte";
import { recordOperation } from "./history.svelte";

export interface ExtractWorkspacePort {
  read: () => {
    archive: ArchiveInfo | null;
    screen: string;
    selection: ReadonlySet<string>;
    selectionBusy: string;
    defaultDirectory: string | null;
    fallbackPaths: readonly string[];
    platform: DesktopPathPlatform;
    presetId: string | null;
    archiveTitle: string;
  };
  choose: (purpose: string, options: {
    title: string;
    multiple: boolean;
    directory: boolean;
    defaultPath?: string;
  }, isCurrent: () => boolean) => Promise<string | string[] | null>;
  submit: (spec: JobSpec) => Promise<number>;
  submitArchive: (spec: JobSpec, success: string, missing: string) => Promise<boolean>;
  blocked: () => boolean;
  isSubmitBlocked: (error: unknown) => boolean;
  notice: (message: string) => void;
  focusDestination: (index: number) => void;
  archiveFolder: () => string;
  requestId: () => string;
  preview: (input: ExtractPlanInput) => ExtractPlanPreflightDto | null;
  clearPreset: () => void;
}

/** One ordinary extraction owner; both skins read this instance and its review workspace. */
export class ExtractWorkspaceSession {
  readonly options: ExtractOptionsDraft;
  readonly plan: ExtractPlanSession;
  readonly review: ExtractReviewSession;
  #picker = $state.raw<object | null>(null);
  #disposed = false;

  constructor(private readonly port: ExtractWorkspacePort) {
    this.options = new ExtractOptionsDraft(() => port.read().platform);
    this.plan = new ExtractPlanSession(port.requestId, port.preview);
    this.review = new ExtractReviewSession(port);
  }

  get state() { return this.options.state; }
  get choosing() { return this.#picker !== null; }
  get archive() { return this.port.read().archive; }
  get destination(): string {
    const { archive, defaultDirectory } = this.port.read();
    return archive ? this.options.requestedDestination({
      archiveFolder: this.port.archiveFolder(),
      archiveParent: dirname(archive.path),
      defaultDirectory,
    }) : tr("gui.empty.open_archive_first");
  }
  get smartBase(): string {
    const { archive, defaultDirectory } = this.port.read();
    return this.options.smartBase(defaultDirectory, archive ? dirname(archive.path) : this.destination);
  }
  get effectiveDestination(): string {
    if (this.archive && this.state.destinationMode === "smart") {
      if ((this.plan.phase === "ready" || this.plan.phase === "blocked") && this.plan.plan) return this.plan.plan.destination;
      return tr("gui.extract.smart_destination_value")
        .replace("{base}", this.destination);
    }
    return this.destination;
  }
  get input(): ExtractPlanInput | null {
    const { archive, screen } = this.port.read();
    return archive && screen === "extract" ? this.options.snapshotPlan(archive, this.destination) : null;
  }

  sync(): void {
    if (this.#disposed) return;
    this.adoptArchive();
    const input = this.input;
    if (!input) this.plan.reset();
    else if (!this.plan.matches(input)) void this.plan.request(input, { debounce: true });
  }
  adoptArchive(): void {
    if (this.options.syncArchive(this.archive)) this.invalidatePicker();
  }
  retry(): Promise<void> {
    const input = this.input;
    return input ? this.plan.request(input, { force: true }) : Promise.resolve();
  }
  invalidatePicker(): void { this.#picker = null; }
  leave(): void {
    this.invalidatePicker();
    this.plan.reset();
    this.review.leave();
  }
  dispose(): void {
    this.#disposed = true;
    this.leave();
    this.plan.dispose();
    this.review.dispose();
  }

  open(scope: ExtractScope, selection: string[] | null): boolean {
    const archive = this.archive;
    return !!archive && this.options.openScope(archive, scope, selection, () => this.invalidatePicker());
  }
  restore(draft: ExtractTaskDraft): boolean {
    const archive = this.archive;
    return !!archive && this.options.restoreTask(archive, draft, () => {
      this.invalidatePicker();
      this.plan.reset();
      this.port.clearPreset();
    });
  }
  applyPreset(options: ExtractArchivePresetOptions | null): void {
    this.invalidatePicker();
    if (options) this.options.applyPreset(options);
  }
  async selectDestination(mode: ExtractDestinationMode): Promise<void> {
    if (!this.archive) {
      this.port.notice(this.archiveRequiredReason);
      return;
    }
    if (mode === "choose") {
      await this.chooseDestination();
      return;
    }
    this.invalidatePicker();
    this.options.selectDestination(mode);
  }
  selectPolicy(change: Parameters<ExtractOptionsDraft["selectPolicy"]>[0]): void {
    if (!this.archive) {
      this.port.notice(this.archiveRequiredReason);
      return;
    }
    this.options.selectPolicy(change);
  }
  get archiveRequiredReason(): string {
    return this.archive ? "" : tr("gui.precondition.open_before_extract");
  }
  get pickerWait(): string {
    return tr("gui.extract.destination_picker_wait");
  }

  async chooseDestination(): Promise<boolean> {
    if (this.#disposed) return false;
    const { archive, screen, selection } = this.port.read();
    if (!archive) {
      this.port.notice(this.archiveRequiredReason);
      return false;
    }
    if (this.choosing) {
      this.port.notice(this.pickerWait);
      return false;
    }
    const selected = screen === "browse" ? new Set(selection) : null;
    const { id, source, path: displayPath } = archive;
    const request = {};
    this.#picker = request;
    const isCurrent = () => {
      const current = this.port.read();
      return !this.#disposed && this.#picker === request && current.screen === screen
        && current.archive?.id === id && current.archive.source === source
        && current.archive.path === displayPath && (!selected || (selected.size === current.selection.size
          && [...selected].every((path) => current.selection.has(path))));
    };
    try {
      const path = await this.port.choose("extract.destination", {
        title: tr("gui.extract.choose_destination_title"),
        multiple: false,
        directory: true,
        defaultPath: this.state.customDest.trim() || this.destination,
      }, isCurrent);
      if (!isCurrent()) return false;
      if (typeof path === "string" && this.options.acceptDestination(path)) {
        this.port.notice(tr("gui.extract.destination_selected"));
        return true;
      }
      this.port.notice(tr("gui.extract.destination_picker_cancelled"));
      return false;
    } catch {
      if (isCurrent()) this.port.notice(tr("gui.extract.destination_picker_requires_desktop_service"));
      return false;
    } finally {
      if (this.#picker === request) this.#picker = null;
    }
  }

  async start(): Promise<void> {
    if (this.#disposed) return;
    const reason = this.archiveRequiredReason || (this.choosing ? this.pickerWait : "")
      || (this.state.scope === "selection" && this.state.selection.length === 0
        ? tr("gui.precondition.select_before_extract") : "");
    if (reason) {
      this.port.notice(reason);
      return;
    }
    if (this.plan.phase === "loading" || this.plan.phase === "blocked") {
      this.port.notice(this.plan.phase === "blocked" ? this.spaceFailure : this.planWait);
      return;
    }
    if (this.state.destinationMode === "choose" && !this.state.customDest.trim()) {
      const before = this.port.read();
      const { scope, selection } = this.state;
      if (!await this.chooseDestination()) return;
      const after = this.port.read();
      if (after.archive !== before.archive || after.screen !== before.screen || this.state.scope !== scope
        || this.state.selection !== selection || after.presetId !== before.presetId
        || this.state.destinationMode !== "choose" || !this.state.customDest.trim() || this.choosing) {
        return;
      }
    }
    const { archive, screen, archiveTitle } = this.port.read();
    if (!archive || screen !== "extract") return;
    const snapshot = this.options.snapshotRun(archive, this.destination);
    await this.plan.request(snapshot.input);
    if (this.#disposed || this.archive?.id !== snapshot.input.archiveId || this.port.read().screen !== "extract") {
      return;
    }
    if (!this.plan.matches(snapshot.input) || !this.plan.matches(this.input) || !this.options.runOptionsMatch(snapshot)) {
      this.port.notice(tr("gui.extract.plan_changed_before_start"));
      return;
    }
    const plan = this.plan.plan;
    if (this.plan.phase !== "ready" || !plan) {
      this.port.notice(this.planError || this.planWait);
      return;
    }
    const action = snapshot.input.selection ? tr("gui.extract.selected_queued") : tr("gui.extract.all_queued");
    const success = tr("gui.extract.started_to_destination")
      .replace("{action}", action).replace("{destination}", plan.destination);
    if (await this.port.submitArchive(this.options.extractJob(snapshot, plan), success,
      tr("gui.precondition.open_before_extract"))) {
      recordOperation({ status: "queued", title: action, detail: `${archiveTitle} -> ${plan.destination}` });
    }
  }

  async copyOut(): Promise<void> {
    if (this.#disposed) return;
    if (this.choosing) {
      this.port.notice(this.pickerWait);
      return;
    }
    const before = this.port.read();
    if (before.selectionBusy) {
      this.port.notice(before.selectionBusy);
      return;
    }
    const selection = [...before.selection];
    if (!before.archive || selection.length === 0) {
      this.port.notice(before.archive
        ? tr("gui.precondition.select_entries_before_copy_out")
        : tr("gui.precondition.open_before_copy_out"));
      return;
    }
    if (this.state.destinationMode === "choose" && !this.state.customDest.trim()) {
      if (!await this.chooseDestination()) return;
      const after = this.port.read();
      if (after.archive !== before.archive || after.screen !== before.screen || after.selectionBusy
        || after.selection.size !== selection.length || selection.some((path) => !after.selection.has(path))
        || this.state.destinationMode !== "choose" || !this.state.customDest.trim() || this.choosing) {
        return;
      }
    }
    const destination = this.effectiveDestination;
    const action = tr("gui.copy_out.selected_queued");
    const success = tr("gui.copy_out.started_to_destination")
      .replace("{action}", action).replace("{destination}", destination);
    if (await this.port.submitArchive(this.options.copyOutJob(before.archive, this.destination, selection), success,
      tr("gui.precondition.open_before_copy_out"))) {
      recordOperation({ status: "queued", title: action, detail: `${before.archiveTitle} -> ${destination}` });
    }
  }

  get planWait() { return tr("gui.extract.plan_wait_before_start"); }
  get planError() {
    return this.plan.errorKey ? tr(this.plan.errorKey, "Squallz could not refresh this preview. Check the archive and destination, then try again.") : "";
  }
  get spaceFailure(): string {
    const plan = this.plan.plan;
    return plan ? tr("gui.error.disk_full.body")
      .replace("{required}", formatBytes(plan.required_free_bytes)).replace("{available}", formatBytes(plan.available_bytes))
      : tr("error.disk_full");
  }
}
