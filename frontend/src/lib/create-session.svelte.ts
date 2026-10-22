import { tFallback as tr } from "./i18n.svelte";
import { recordOperation } from "./history.svelte";
import { formatBytes } from "./format";
import {
  createFormats, createFormatIds, createProfileIds,
  type CreateFormatId, type CreateProfileId, type FormatCapabilityCard,
} from "./ui-model";
import { desktopBasename, desktopDirname, joinDesktopPath, archiveBaseOrDefault } from "./desktop-path";
import {
  createSourcePaths, includesCreateSourcePath, mergeCreateSources, removeCreateSourcesByPaths,
  toggleCreateSourcePath, type CreateSourceKind, type CreateSourceRoot,
} from "./create-sources";
import { CreateOptionsDraft, type CreateDraftEdit, type CreateDraftChange } from "./create-options-draft.svelte";
import {
  CreatePreflightSession, type CreateRunDraft, type CreatePreflightServices,
  type CreatePreflightPresentation, type CreatePreflightEffect, type CreatePreflightEvent,
  type CreatePreflightSeed,
} from "./create-preflight.svelte";
import { ipc, type JobSpec, type NamedArchivePreset, type SfxCreateCapabilityDto } from "./ipc";
import type { UiMode } from "./uiMode.svelte";
import { CreatePresentation } from "./create-presentation";
import { type ArchivePresetSession, archivePresetDisplayName } from "./archive-presets.svelte";
import { createContentPolicyLabel } from "./archive-option-labels";
import { createPresetSaveIssue } from "./create-preset-validation";

export type ClassicCreateSection = "general" | "compression" | "content" | "security" | "volumes" | "recovery" | "preflight";
export interface CreateSessionPort extends Omit<CreatePreflightServices, "ensurePreflightListener"> {
  read: () => { screen: string; mode: UiMode; defaultDirectory: string | null };
  presets: () => ArchivePresetSession;
  notice: (message: string) => void;
  submit: (spec: JobSpec) => Promise<number>;
  blocked: () => boolean;
  tError: CreatePreflightPresentation["tError"];
  submissionBlockedMessage: CreatePreflightPresentation["submissionBlockedMessage"];
  emit: CreatePreflightPresentation["emit"];
  clearFocus: () => void;
  focusInvalid: () => void;
  focusPrimary: () => void;
  primaryFocusPending: () => boolean;
  scrollSection: (target: string) => void;
  openRecovery: () => void;
  fileManager: () => string;
  trashName: () => string;
  featuredFormats: () => FormatCapabilityCard[];
  forwardEvent?: (event: CreatePreflightEvent) => boolean;
  listen?: (receive: (event: CreatePreflightEvent) => void) => Promise<() => void>;
}

function stored(key: string): string | null {
  try {
    return window.localStorage.getItem(`squallz.${key}`);
  } catch {
    return null;
  }
}
function persist(key: string, value: string | number): void {
  try {
    window.localStorage.setItem(`squallz.${key}`, String(value));
  } catch {
    // Convenience preferences must not block archive tasks.
  }
}
const archiveName = (base: string, extension: string) => `${base}.${extension}`;

/** The sole Create workspace owner, composing option policy and native preflight. */
export class CreateSession {
  readonly options: CreateOptionsDraft;
  readonly preflight: CreatePreflightSession;
  readonly view: CreatePresentation;
  sources = $state<CreateSourceRoot[]>([]);
  selected = $state<string[]>([]);
  advancedOpen = $state(false);
  section = $state<ClassicCreateSection>("general");
  capability = $state<SfxCreateCapabilityDto>({
    target: "macos", extension: "app", available: true, status: "available", requires_signing: true,
  });
  capabilityReady = $state(false);
  closed = false;
  private cleanup: (() => void) | null = null;
  private listening: Promise<void> | null = null;
  private active = false;

  constructor(readonly port: CreateSessionPort, format: string | null = null, seed?: CreatePreflightSeed) {
    const savedFormat = stored("createFormat");
    const savedProfile = stored("createProfile");
    const validFormat = (value: string | null): value is CreateFormatId => createFormatIds.includes(value as CreateFormatId);
    this.options = new CreateOptionsDraft({
      format: validFormat(format) ? format : validFormat(savedFormat) ? savedFormat : "7z",
      profile: createProfileIds.includes(savedProfile as CreateProfileId) ? savedProfile as CreateProfileId : "balanced",
      customLevel: stored("customCreateLevel") === null ? 6 : Number(stored("customCreateLevel")),
      touched: validFormat(format),
    });
    const target = port.platform();
    this.capability = {
      target, extension: target === "macos" ? "app" : target === "windows" ? "exe" : "run",
      available: true, status: "available", requires_signing: true,
    };
    this.view = new CreatePresentation(this);
    this.preflight = new CreatePreflightSession({ ...port, ensurePreflightListener: () => this.ensureEvents() }, {
      tr, tError: port.tError, submissionBlockedMessage: port.submissionBlockedMessage, emit: this.applyEffect,
    }, port.submit, seed);
    this.syncContext();
  }

  get inputs(): string[] { return createSourcePaths(this.sources); }
  get formatData() { return createFormats[this.options.state.format]; }
  get configurationPending(): boolean { return this.port.presets().loadState === "loading" || !this.capabilityReady; }
  draftContext = () => ({ capabilityReady: this.capabilityReady, sfxCapability: this.capability, locked: this.preflight.busy() });
  saveIssue = () => createPresetSaveIssue(this.options, this.view.issue);
  persistFormat = (value: CreateFormatId) => persist("createFormat", value);
  persistProfile = (value: CreateProfileId) => persist("createProfile", value);
  persistLevel = (value: number) => persist("customCreateLevel", value);
  joinFolder = (folder: string, name: string) => joinDesktopPath(folder, name, this.port.platform());

  syncContext = (): void => {
    const { screen, mode } = this.port.read();
    if (this.active && screen !== "create" && this.preflight.state.pending) {
      this.preflight.invalidatePlan();
      this.options.resetValidation();
    }
    if (!this.active && screen === "create") this.section = "general";
    this.active = screen === "create";
    if (this.preflight.syncContext({ active: screen === "create", mode, sources: this.sources })) this.port.clearFocus();
  };
  dismiss = (): void => { this.port.clearFocus(); this.preflight.dismissPreparation(); };
  leave = (): boolean => {
    if (!this.preflight.canLeave()) {
      this.port.notice(tr("gui.create.wait_for_submission_before_leaving"));
      return false;
    }
    if (this.preflight.state.phase === "choosingDest" && this.destinationCancellable()) {
      void this.preflight.cancelDestination({ announce: false, keepIntentOnFailure: true });
    }
    return true;
  };
  loadCapability = async (): Promise<void> => {
    try {
      const capability = await ipc.getSfxCreateCapability();
      if (this.closed) return;
      this.capability = capability;
      this.options.acceptSfxCapability(capability.available);
    } catch {
      if (this.closed) return;
      this.options.acceptSfxCapability(false);
      if (!import.meta.env.DEV) this.capability = { ...this.capability, available: false, status: "invalid" };
    }
    this.capabilityReady = true;
    this.port.presets().applyDefault("create");
  };
  ensureEvents = (): Promise<void> => {
    if (this.cleanup || this.closed) return Promise.resolve();
    if (this.listening) return this.listening;
    const receive = (event: CreatePreflightEvent) => {
      if (!this.closed && !this.port.forwardEvent?.(event)) this.preflight.applyEvent(event);
    };
    const listen = this.port.listen ?? (async (receive) => {
      const { listen } = await import("@tauri-apps/api/event");
      return listen<CreatePreflightEvent>("create://preflight", (event) => receive(event.payload));
    });
    this.listening = listen(receive).then((cleanup) => {
      if (this.closed) cleanup(); else this.cleanup = cleanup;
    }).catch(() => { /* Browser previews have no native event bus. */ }).finally(() => { this.listening = null; });
    return this.listening;
  };
  dispose = (): void => {
    this.closed = true;
    this.preflight.dispose();
    this.cleanup?.();
    this.cleanup = null;
    this.port.clearFocus();
  };
  showSection = (section: ClassicCreateSection, target: string): void => {
    this.section = section;
    this.port.scrollSection(target);
  };
  toggleAdvanced = (event: KeyboardEvent): void => {
    if (event.key !== "Enter" && event.key !== " " && event.key !== "Spacebar") return;
    event.preventDefault();
    this.advancedOpen = !this.advancedOpen;
  };
  updateLevelFromInput = (event: Event, commit = false) => this.updateCustomLevel((event.currentTarget as HTMLInputElement).value, commit);
  normalizeNotice = (change: CreateDraftChange): void => {
    if (change.postSuccessNormalized) {
      this.port.notice(tr("gui.create.output.source.changed_to_keep")
        .replace("{reason}", this.view.trashSourceDisabledReason()));
    }
    if (change.completionNormalized) {
      this.port.notice(tr("gui.create.output.completion.changed_to_reveal")
        .replace("{reason}", this.view.openCompletionDisabledReason()));
    }
  };
  validate = (): boolean => {
    const issue = this.options.validate(this.capability);
    if (!issue) return true;
    this.advancedOpen = true;
    this.section = issue === "invalid_part_size" || issue === "native_zip_part_size_limit" ? "volumes" : "security";
    this.port.notice(this.view.issue(issue));
    this.port.focusInvalid();
    return false;
  };
  touch = () => {
    this.options.touch();
    this.preflight.invalidatePlan();
  };

  edit = (edit: CreateDraftEdit): boolean => {
    const change = this.options.edit(edit, this.draftContext(), this.touch);
    if (change.issue) {
      this.port.notice(this.view.issue(change.issue, change.requestedSfxTarget));
      return false;
    }
    if (!change.changed) return false;
    if (edit.kind === "contentPolicy") {
      this.port.notice(tr("gui.create.content_policy.changed")
        .replace("{policy}", createContentPolicyLabel(edit.value)));
    }
    this.normalizeNotice(change);
    if (edit.kind === "sfxEnabled") {
      if (edit.value) this.persistFormat("zip");
      this.port.notice(edit.value
        ? tr("gui.create.sfx_enabled_notice")
        : tr("gui.create.sfx_disabled_notice"));
    } else if (edit.kind === "postSuccess" && edit.value === "trash_source") {
      this.port.notice(tr("gui.create.output.source.trash_selected")
        .replace("{trash}", this.port.trashName()));
    }
    return true;
  };

  chooseProfile = (next: CreateProfileId) => {
    if (!this.edit({ kind: "profile", value: next })) return;
    this.persistProfile(next);
    const profile = this.view.profileData(next);
    recordOperation({
      status: "info",
      title: tr("gui.create.profile_selected_operation"),
      detail: `${profile.label} · level ${profile.level}`,
    });
    this.port.notice(tr("gui.create.profile_selected_notice").replace("{profile}", profile.label));
  };

  chooseFormat = (next: CreateFormatId) => {
    if (!this.edit({ kind: "format", value: next })) return;
    this.persistFormat(next);
    const format = this.formatData;
    recordOperation({
      status: "info",
      title: tr("gui.create.format_selected_operation"),
      detail: `${format.label} · .${format.extension}`,
    });
    this.port.notice(tr("gui.create.format_selected_notice").replace("{format}", format.label));
  };

  captureRun = (): CreateRunDraft | null => {
    if (this.options.normalizePostSuccess()) {
      this.normalizeNotice({ changed: true, postSuccessNormalized: true });
    }
    if (!this.validate()) return null;
    return this.options.snapshotRun({
      sfxCapability: this.capability,
      suggestedDestination: this.suggestedOutputPath(this.options.state.format, this.options.outputExtension(this.capability.extension)),
      defaultCreateDir: this.port.read().defaultDirectory,
    });
  };

  updateCustomLevel = (value: number | string, commit = false) => {
    const previousProfile = this.options.state.profile;
    if (!this.edit({ kind: "customLevel", value })) return;
    const next = this.options.state.customLevel;
    this.persistLevel(next);
    if (previousProfile !== "custom") {
      this.persistProfile("custom");
    }
    if (commit) {
      recordOperation({
        status: "info",
        title: tr("gui.create.custom_level_updated_operation"),
        detail: tr("gui.create.level_detail").replace("{level}", String(next)),
      });
      this.port.notice(
        tr("gui.create.custom_level_saved_notice")
          .replace("{level}", String(next)),
      );
    }
  };

  applyPreset = (preset: Extract<NamedArchivePreset, { kind: "create" }> | null, announce: boolean): boolean => {
    if (this.preflight.busy()) {
      if (announce) this.port.notice(tr("gui.presets.busy"));
      return false;
    }
    if (!preset) {
      if (announce) this.touch();
      return true;
    }
    const change = this.options.applyPreset(preset.options, this.draftContext(), () => {
      this.dismiss();
      if (announce) this.touch();
      this.preflight.invalidatePlan();
    }, announce);
    if (change.issue) {
      this.port.notice(this.view.issue(change.issue, change.requestedSfxTarget));
      return false;
    }
    if (announce) {
      this.normalizeNotice({ ...change, postSuccessNormalized: false });
      this.port.notice(change.postSuccessNormalized
        ? tr("gui.create.output.source.preset_changed_to_keep")
          .replace("{name}", archivePresetDisplayName(preset))
        : tr("gui.presets.applied_notice").replace("{name}", archivePresetDisplayName(preset)));
    }
    return true;
  };

  appendSources = (
    paths: readonly string[],
    kind: CreateSourceKind,
  ): { added: number; total: number } => {
    const previousCount = this.sources.length;
    const merged = mergeCreateSources(
      this.sources,
      paths.map((path) => ({ path, kind })),
      this.port.platform(),
    );
    if (merged.length !== previousCount || merged.some((source, index) =>
      source.path !== this.sources[index]?.path || source.kind !== this.sources[index]?.kind)) {
      this.dismiss();
      this.sources = merged;
      this.syncContext();
    }
    return {
      added: this.sources.length - previousCount,
      total: this.sources.length,
    };
  };

  isSelected = (path: string): boolean => {
    return includesCreateSourcePath(this.selected, path, this.port.platform());
  };

  selectedCount = (): number => {
    return this.sources.filter((source) => this.isSelected(source.path)).length;
  };

  allSelected = (): boolean => {
    return this.sources.length > 0 && this.selectedCount() === this.sources.length;
  };

  selectAll = (selected: boolean): void => {
    if (this.preflight.sourcesLocked()) return;
    this.selected = selected ? [...this.inputs] : [];
  };

  toggleSelection = (path: string): void => {
    if (this.preflight.sourcesLocked()) return;
    this.selected = toggleCreateSourcePath(
      this.selected,
      path,
      this.port.platform(),
    );
  };

  clearSelection = (): void => {
    if (this.preflight.sourcesLocked()) return;
    this.selected = [];
  };

  removeSources = (paths: readonly string[]): number => {
    if (this.preflight.sourcesLocked() || paths.length === 0) return 0;
    const previousCount = this.sources.length;
    const remaining = removeCreateSourcesByPaths(this.sources, paths, this.port.platform());
    if (remaining.length === previousCount) return 0;
    this.dismiss();
    this.sources = remaining;
    this.syncContext();
    this.selected = this.selected.filter(
      (selected) => !includesCreateSourcePath(paths, selected, this.port.platform()),
    );
    return previousCount - this.sources.length;
  };

  removeSource = (path: string): void => {
    if (this.removeSources([path]) === 0) return;
    this.port.notice(
      tr("gui.create.sources.removed_one")
        .replace("{name}", desktopBasename(path, this.port.platform())),
    );
  };

  removeSelected = (): void => {
    const removed = this.removeSources(this.selected);
    if (removed === 0) return;
    this.port.notice(
      tr("gui.create.sources.removed_many")
        .replace("{count}", removed.toLocaleString()),
    );
  };

  clearSources = (): void => {
    if (this.sources.length === 0 && this.selected.length === 0 && this.options.state.suggestedDestination === null) return;
    this.dismiss();
    this.sources = [];
    this.selected = [];
    this.options.clearSuggestedDestination();
    this.syncContext();
  };

  review = async (): Promise<void> => {
    if (this.inputs.length === 0) {
      this.port.notice(tr("gui.create.no_source_items"));
      return;
    }
    await this.start([...this.inputs], "dialog");
  };

  chooseSources = async (sourceKind: "files" | "folder") => {
    if (this.port.read().screen !== "create" || this.closed) return;
    if (this.port.blocked()) return;
    if (this.preflight.sourcesLocked()) {
      this.port.notice(this.view.sourcesLockedReason());
      return;
    }
    this.syncContext();
    await this.preflight.chooseSources(sourceKind);
  };

  start = async (inputs: string[], source: "dialog" | "drop", capturedDraft?: CreateRunDraft) => {
    if (this.port.read().screen !== "create" || this.closed) return;
    if (this.preflight.state.picker) {
      this.port.notice(this.view.sourcesLockedReason());
      return;
    }
    if (!capturedDraft && this.configurationPending) {
      this.port.notice(this.view.configurationPendingMessage());
      return;
    }
    if (!this.preflight.canLeave() || (!capturedDraft && this.preflight.busy())) {
      this.port.notice(tr("gui.create.preflight_already_running"));
      return;
    }
    const draft = capturedDraft ?? this.captureRun();
    if (!draft) {
      this.preflight.invalidatePlan();
      return;
    }
    if (this.port.blocked()) return;
    this.options.touch();
    this.port.clearFocus();
    this.syncContext();
    await this.preflight.start(inputs, source, draft);
  };

  cancelReview = (): void => {
    if (!this.preflight.state.pending || this.preflight.busy()) return;
    this.options.resetValidation();
    this.preflight.cancelReview();
  };

  confirm = async (): Promise<void> => {
    if (this.preflight.busy()) return;
    if (this.preflight.state.pending && this.preflight.state.plan && this.port.blocked()) return;
    await this.preflight.confirm();
  };

  restoreTask = (spec: Extract<JobSpec, { kind: "compress" }>, passwordRequired: boolean): boolean => {
    if (this.preflight.sourcesLocked()) {
      this.port.notice(this.view.sourcesLockedReason());
      return false;
    }
    if (this.configurationPending) {
      this.port.notice(this.view.configurationPendingMessage());
      return false;
    }
    const change = this.options.restoreTask(spec, passwordRequired, this.draftContext(), () => {
      this.dismiss();
      this.touch();
      this.port.presets().clearSelection("create");
      this.sources = mergeCreateSources([], spec.inputs.map((path) => ({ path, kind: "unknown" })), this.port.platform());
      this.syncContext();
      this.selected = [];
    });
    if (change.issue) {
      this.port.notice(this.view.issue(change.issue, change.requestedSfxTarget));
      return false;
    }
    this.section = "general";
    this.advancedOpen = true;
    this.port.notice(tr("gui.create.review.restored"));
    return true;
  };

  outputBase = (): string => {
    return this.inputs.length === 1
      ? archiveBaseOrDefault(this.port.archiveStemName(desktopBasename(this.inputs[0], this.port.platform())))
      : "archive";
  };

  suggestedOutputPath = (format: CreateFormatId, outputExtension: string): string | null => {
    const path = this.options.state.suggestedDestination;
    if (!path) return null;
    const extensions = outputExtension === createFormats[format].extension
      ? createFormats[format].extensions
      : [outputExtension];
    if (extensions.some((extension) => path.toLowerCase().endsWith(`.${extension}`))) return path;
    return this.joinFolder(
      desktopDirname(path, this.port.platform()),
      archiveName(this.port.archiveStemName(desktopBasename(path, this.port.platform())), outputExtension),
    );
  };

  destinationCancellable = (): boolean => {
    return this.preflight.state.requestKind === "destination"
      && this.preflight.state.requestId !== null
      && (this.preflight.state.phase === "choosingDest" || this.preflight.state.phase === "submitting");
  };

  private applyEffect = async (effect: CreatePreflightEffect, current: () => boolean): Promise<void> => {
    if (!current()) return;
    switch (effect.kind) {
      case "notice": this.port.notice(effect.message); return;
      case "sourcesPicked": this.view.sourcesAdded(this.appendSources(effect.paths, effect.sourceKind)); return;
      case "resetCredentials": this.options.resetCredentials(effect.intent); return;
      case "queued": {
        const { pending, plan } = effect;
        this.clearSources();
        this.options.resetCredentials(pending);
        this.options.resetValidation();
        this.port.notice((pending.creatingSfx
          ? tr("gui.create.sfx_queued_notice")
          : tr("gui.create.queued_notice")).replace("{size}", formatBytes(plan.total_bytes)));
        recordOperation({
          status: "queued",
          title: pending.creatingSfx ? tr("gui.create.sfx_queued")
            : pending.source === "drop" ? tr("gui.create.dropped_items_queued") : tr("gui.create.queued"),
          detail: tr("gui.create.operation_detail")
            .replace("{name}", desktopBasename(plan.primary_output, this.port.platform()))
            .replace("{profile}", this.view.profileLabel(pending.profile)).replace("{size}", formatBytes(plan.total_bytes)),
        });
      }
    }
    await this.port.emit(effect, current);
  };
}
