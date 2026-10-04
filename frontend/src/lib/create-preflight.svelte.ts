import {
  archiveBaseOrDefault,
  desktopBasename,
  desktopDirname,
  joinDesktopPath,
  sameDesktopPath,
  type DesktopPathPlatform,
} from "./desktop-path";
import { createSourcePaths, mergeCreateSources, type CreateSourceRoot } from "./create-sources";
import { formatBytes } from "./format";
import {
  ipc,
  isErrorDto,
  type CreateDestinationBase,
  type CreateDestinationInspectionDto,
  type CreatePlanDto,
  type DiskSpaceDto,
  type ErrorDto,
  type JobSpec,
  type OverwritePolicy,
} from "./ipc";
import { applyCreateDestinationAuthorization } from "./task-model";
import { createFormats, type CreateFormatId, type CreateProfileId } from "./ui-model";
import type { UiMode } from "./uiMode.svelte";

type CompressJobSpec = Extract<JobSpec, { kind: "compress" }>;
type DialogModule = typeof import("@tauri-apps/plugin-dialog");

export type CreatePreflightStage = "source" | "temp" | "destination" | "submit";
export type CreatePreflightPhase =
  | "idle"
  | "selecting"
  | "measuring"
  | "checkingTemp"
  | "choosingDest"
  | "checkingDest"
  | "reviewing"
  | "submitting"
  | "ready"
  | "cancelled"
  | "blocked";

export type CreateRunDraft = Readonly<{
  format: CreateFormatId;
  profile: CreateProfileId;
  level: number;
  password: string | null;
  encryptNames: boolean;
  splitSize: number | null;
  splitMode: CompressJobSpec["split_mode"];
  contentPolicy: CompressJobSpec["content_policy"];
  excludes: readonly string[];
  sqzInnerFormat: CompressJobSpec["sqz_inner_format"];
  sfxEnabled: boolean;
  sfxTarget: CompressJobSpec["sfx_target"];
  outputExtension: string;
  suggestedDestination: string | null;
  destination: Readonly<{
    base: CreateDestinationBase;
    existing_output: OverwritePolicy;
  }>;
  completion: CompressJobSpec["completion"];
  postSuccess: CompressJobSpec["post_success"];
  testAfterCreate: boolean;
  defaultCreateDir: string | null;
  restoreCredentialPrompt: boolean;
  restoreEncryptNames: boolean;
}>;

export type CreateCredentialIntent = Pick<CreateRunDraft, "restoreCredentialPrompt" | "restoreEncryptNames">;
export type PendingCreateSubmission = Readonly<{
  spec: CompressJobSpec;
  source: "dialog" | "drop";
  format: CreateFormatId;
  profile: CreateProfileId;
  creatingSfx: boolean;
  artifactLabel: string;
  splitSize: number | null;
  confirmLateConflict: boolean;
  restoreCredentialPrompt: boolean;
  restoreEncryptNames: boolean;
}>;

export type CreatePreflightContext = Readonly<{
  active: boolean;
  mode: UiMode;
  sources: readonly CreateSourceRoot[];
}>;
type CreatePreparationOwner = Pick<CreatePreflightContext, "mode" | "sources">;
type CreateSourcePicker = CreatePreparationOwner & Readonly<{ kind: "files" | "folder" }>;
type ResolvedCreateDestination = Readonly<{
  path: string;
  replaceExisting: boolean;
  replacementGuard: string | null;
  confirmLateConflict: boolean;
}>;

export type CreatePreflightEvent = Readonly<{
  request_id?: string;
  phase?: string;
  scanned?: number;
  processed_bytes?: number;
  total_bytes?: number;
  current?: string;
}>;

export type CreatePreflightSeed = Readonly<{
  scanned: number;
  current: string;
  destinationBytes: number;
  destinationCurrent: string;
}>;

export type CreatePreflightEffect =
  | { kind: "notice"; message: string }
  | { kind: "sourcesPicked"; paths: string[]; sourceKind: "file" | "folder" }
  | { kind: "resetCredentials"; intent: CreateCredentialIntent | null }
  | { kind: "focus"; target: "primary" | "review" }
  | { kind: "prepareSubmit" }
  | { kind: "queued"; pending: PendingCreateSubmission; plan: CreatePlanDto; jobId: number };

export interface CreatePreflightServices {
  getDialogModule: () => Promise<DialogModule>;
  openNativeDialog: (
    kind: string,
    open: DialogModule["open"],
    options: NonNullable<Parameters<DialogModule["open"]>[0]>,
  ) => ReturnType<DialogModule["open"]>;
  saveNativeDialog: (
    kind: string,
    save: DialogModule["save"],
    options: NonNullable<Parameters<DialogModule["save"]>[0]>,
  ) => ReturnType<DialogModule["save"]>;
  ensurePreflightListener: () => Promise<void>;
  nextRequestId: () => string;
  platform: () => DesktopPathPlatform;
  archiveStemName: (name: string) => string;
}

export interface CreatePreflightPresentation {
  tr: (key: string, fallback: string) => string;
  tError: (error: ErrorDto) => string;
  submissionBlockedMessage: (error: unknown) => string | null;
  emit: (effect: CreatePreflightEffect, isCurrent: () => boolean) => void | Promise<void>;
}

type CreatePreflightData = {
  phase: CreatePreflightPhase;
  scanned: number;
  current: string;
  excludeCount: number;
  issue: string;
  issueStage: CreatePreflightStage | null;
  creatingSfx: boolean;
  requestId: string | null;
  requestKind: "source" | "destination" | null;
  processedBytes: number;
  cancelPending: boolean;
  plan: CreatePlanDto | null;
  destination: string | null;
  destinationDisk: DiskSpaceDto | null;
  workspaceDisk: DiskSpaceDto | null;
  systemTempDisk: DiskSpaceDto | null;
  pending: PendingCreateSubmission | null;
};

export type CreatePreflightState = Readonly<CreatePreflightData & {
  picker: CreateSourcePicker | null;
  owner: CreatePreparationOwner | null;
}>;

export class CreateDestinationInspectionError extends Error {
  readonly detail: ErrorDto | null;
  readonly cancelled: boolean;

  constructor(error?: unknown, cancelled = false) {
    super("create-destination-inspection-failed");
    this.detail = isErrorDto(error) ? error : null;
    this.cancelled = cancelled;
  }
}

export function createDestinationInspectionCancelled(error: unknown): boolean {
  return error instanceof CreateDestinationInspectionError
    && (error.cancelled || error.detail?.key === "error.cancelled");
}

export function commonCreateSourceParent(
  inputs: readonly string[],
  platform: DesktopPathPlatform,
): string | null {
  const first = inputs[0];
  if (!first) return null;
  const parent = desktopDirname(first, platform);
  return inputs.every((input) => sameDesktopPath(desktopDirname(input, platform), parent, platform))
    ? parent
    : null;
}

function emptyData(): CreatePreflightData {
  return {
    phase: "idle",
    scanned: 0,
    current: "",
    excludeCount: 0,
    issue: "",
    issueStage: null,
    creatingSfx: false,
    requestId: null,
    requestKind: null,
    processedBytes: 0,
    cancelPending: false,
    plan: null,
    destination: null,
    destinationDisk: null,
    workspaceDisk: null,
    systemTempDisk: null,
    pending: null,
  };
}

export class CreatePreflightSession {
  private data = $state<CreatePreflightData>(emptyData());
  private picker = $state.raw<CreateSourcePicker | null>(null);
  private owner = $state.raw<CreatePreparationOwner | null>(null);
  private context: CreatePreflightContext = { active: false, mode: "modern", sources: [] };
  private submission: object | null = null;
  private generation = 0;
  private closed = false;
  private readonly previewDestinationRequestId: string | null;

  constructor(
    private readonly services: CreatePreflightServices,
    private readonly presentation: CreatePreflightPresentation,
    private readonly submit: (spec: CompressJobSpec) => Promise<number>,
    seed?: CreatePreflightSeed,
  ) {
    this.previewDestinationRequestId = import.meta.env.DEV && (seed?.destinationBytes ?? 0) > 0
      ? "dev-preview-destination" : null;
    if (seed) {
      this.data.phase = seed.destinationBytes > 0 ? "choosingDest" : seed.scanned > 0 ? "measuring" : "idle";
      this.data.scanned = seed.scanned;
      this.data.current = seed.destinationCurrent || seed.current;
      this.data.requestId = this.previewDestinationRequestId;
      this.data.requestKind = this.previewDestinationRequestId ? "destination" : null;
      this.data.processedBytes = seed.destinationBytes;
    }
  }

  get state(): CreatePreflightState {
    return { ...this.data, picker: this.picker, owner: this.owner };
  }

  syncContext(context: CreatePreflightContext): boolean {
    const changed = context.active !== this.context.active
      || context.mode !== this.context.mode
      || context.sources !== this.context.sources;
    this.context = context;
    if (changed) this.dismissPreparation();
    return changed;
  }

  private sameContext(request: CreatePreparationOwner): boolean {
    return !this.closed && this.context.active && this.context.mode === request.mode
      && this.context.sources === request.sources;
  }

  private currentOwner(request: CreatePreparationOwner): boolean {
    return this.owner === request && this.sameContext(request);
  }

  private tr(key: string, fallback: string): string {
    return this.presentation.tr(key, fallback);
  }

  private notice(message: string): void {
    if (!this.closed) void this.presentation.emit({ kind: "notice", message }, () => !this.closed);
  }

  private clearRequest(): void {
    this.data.requestId = null;
    this.data.requestKind = null;
    this.data.processedBytes = 0;
    this.data.cancelPending = false;
    this.data.current = "";
  }

  busy(): boolean {
    return ["selecting", "measuring", "checkingTemp", "choosingDest", "checkingDest", "submitting"].includes(this.data.phase);
  }

  sourcesLocked(): boolean {
    return this.picker !== null || this.busy() || this.data.pending !== null;
  }

  canLeave(): boolean {
    return this.data.phase !== "submitting";
  }

  dismissPreparation(): void {
    this.generation += 1;
    this.picker = null;
    if (this.owner) this.invalidatePlan();
  }

  invalidatePlan(): void {
    if (this.busy() && this.owner === null) return;
    this.generation += 1;
    this.owner = null;
    if (this.data.phase === "idle") return;
    const { pending, excludeCount, creatingSfx } = this.data;
    this.data = { ...emptyData(), excludeCount, creatingSfx };
    if (pending && !this.closed) {
      void this.presentation.emit({ kind: "resetCredentials", intent: pending }, () => !this.closed);
    }
  }

  applyEvent(event: CreatePreflightEvent): boolean {
    if (this.closed || !this.data.requestId || event.request_id !== this.data.requestId) return false;
    if (event.phase === "destination" && this.data.requestKind === "destination") {
      const bytes = Number(event.processed_bytes ?? 0);
      if (Number.isFinite(bytes)) this.data.processedBytes = bytes;
    } else {
      const scanned = Number(event.scanned ?? 0);
      if (Number.isFinite(scanned)) this.data.scanned = scanned;
    }
    this.data.current = String(event.current ?? "");
    return true;
  }

  async chooseSources(kind: "files" | "folder"): Promise<void> {
    if (this.closed || !this.context.active || this.sourcesLocked()) return;
    const request: CreateSourcePicker = { kind, mode: this.context.mode, sources: this.context.sources };
    this.picker = request;
    const isCurrent = () => this.picker === request && this.sameContext(request);
    this.notice(kind === "files"
      ? this.tr("gui.create.opening_file_picker", "Opening file picker...")
      : this.tr("gui.create.opening_folder_picker", "Opening folder picker..."));
    try {
      const { open } = await this.services.getDialogModule();
      if (!isCurrent()) return;
      const selected = await this.services.openNativeDialog(`create.${kind}`, open, {
        title: kind === "files"
          ? this.tr("gui.create.choose_files_to_archive", "Choose files to archive")
          : this.tr("gui.create.choose_folder_to_archive", "Choose folder to archive"),
        multiple: true,
        directory: kind === "folder",
      });
      if (!isCurrent()) return;
      const paths = Array.isArray(selected) ? selected : selected ? [selected] : [];
      if (paths.length === 0) {
        this.notice(this.tr("gui.create.sources.picker_cancelled", "Source selection cancelled · the current list was kept"));
        return;
      }
      this.picker = null;
      const generation = this.generation;
      await this.presentation.emit({
        kind: "sourcesPicked", paths, sourceKind: kind === "files" ? "file" : "folder",
      }, () => this.sameContext(request) && this.generation === generation);
    } catch {
      if (isCurrent()) this.notice(this.tr("gui.create.requires_desktop_dialog", "Create archive requires the desktop file dialog"));
    } finally {
      if (this.picker === request) this.picker = null;
    }
  }

  private begin(draft: CreateRunDraft): CreatePreparationOwner {
    this.dismissPreparation();
    this.data = emptyData();
    this.data.phase = "choosingDest";
    this.data.excludeCount = draft.contentPolicy === "cross_platform_clean" ? 3 : draft.excludes.length;
    this.data.creatingSfx = draft.sfxEnabled;
    const request = { mode: this.context.mode, sources: this.context.sources };
    this.owner = request;
    return request;
  }

  private finishIssue(
    stage: CreatePreflightStage,
    message: string,
    phase: "blocked" | "cancelled" = "blocked",
  ): void {
    this.clearRequest();
    this.data.issueStage = stage;
    this.data.issue = message;
    this.data.phase = phase;
    this.notice(message);
  }

  private async inspectDestination(
    path: string,
    split: boolean,
    sfxTarget: CompressJobSpec["sfx_target"],
    isCurrent: () => boolean,
  ): Promise<CreateDestinationInspectionDto> {
    await this.services.ensurePreflightListener();
    if (!isCurrent()) throw new CreateDestinationInspectionError(undefined, true);
    const requestId = this.services.nextRequestId();
    this.data.requestId = requestId;
    this.data.requestKind = "destination";
    this.data.processedBytes = 0;
    this.data.cancelPending = false;
    this.data.current = "";
    try {
      const inspection = await ipc.inspectCreateDestination(path, split, requestId, sfxTarget);
      if (!isCurrent()) throw new CreateDestinationInspectionError(undefined, true);
      if (this.data.requestId === requestId && this.data.cancelPending) {
        throw new CreateDestinationInspectionError(undefined, true);
      }
      return inspection;
    } catch (error) {
      const cancelled = this.data.requestId === requestId && this.data.cancelPending;
      if (error instanceof CreateDestinationInspectionError) {
        if (!cancelled || error.cancelled) throw error;
        throw new CreateDestinationInspectionError(error.detail ?? undefined, true);
      }
      throw new CreateDestinationInspectionError(error, cancelled);
    } finally {
      if (this.data.requestId === requestId) {
        this.data.requestId = null;
        this.data.requestKind = null;
        this.data.cancelPending = false;
        this.data.current = "";
      }
    }
  }

  async cancelDestination(
    options: { announce?: boolean; keepIntentOnFailure?: boolean } = {},
  ): Promise<void> {
    const announce = options.announce ?? true;
    const requestId = this.data.requestId;
    if (!requestId || this.data.requestKind !== "destination" || this.data.cancelPending) return;
    this.data.cancelPending = true;
    if (import.meta.env.DEV && requestId === this.previewDestinationRequestId) {
      const request = { mode: this.context.mode, sources: this.context.sources };
      this.owner = request;
      const isCurrent = () => this.currentOwner(request);
      try {
        await new Promise((resolve) => setTimeout(resolve, 180));
        if (!isCurrent() || this.data.requestId !== requestId || !this.data.cancelPending) return;
        this.finishIssue(
          "destination",
          this.tr("gui.create.destination_check_cancelled", "Output check cancelled · no archive was created"),
          "cancelled",
        );
        await this.presentation.emit(
          { kind: "focus", target: "primary" },
          () => isCurrent() && this.data.phase === "cancelled",
        );
      } finally {
        if (this.owner === request) this.owner = null;
      }
      return;
    }
    try {
      await ipc.cancelCreateDestinationInspection(requestId);
      if (announce && this.data.requestId === requestId && this.data.cancelPending) {
        this.notice(this.tr("gui.create.destination_check_cancel_requested", "Stopping the output check..."));
      }
    } catch {
      if (this.data.requestId !== requestId) return;
      if (!options.keepIntentOnFailure) this.data.cancelPending = false;
      if (announce) {
        this.notice(this.tr("gui.create.destination_check_cancel_failed", "Could not stop the output check. It will continue."));
      }
    }
  }

  private async askDestination(
    inputs: readonly string[],
    base: string,
    draft: CreateRunDraft,
    source: "dialog" | "drop",
    isCurrent: () => boolean,
  ): Promise<ResolvedCreateDestination | null> {
    const { confirm, save } = await this.services.getDialogModule();
    if (!isCurrent()) return null;
    const extensions = draft.outputExtension === "swm" || draft.sfxEnabled
      ? [draft.outputExtension]
      : createFormats[draft.format].extensions;
    let filterName: string;
    if (draft.sfxEnabled) {
      filterName = this.tr("gui.create.sfx_filter", "Self-extracting output");
    } else if (draft.outputExtension === "swm") {
      filterName = this.tr("gui.create.split_wim_filter", "Split WIM first part");
    } else {
      filterName = this.tr(`gui.create.format.${draft.format}.filter`, createFormats[draft.format].filterName);
    }
    const platform = this.services.platform();
    const selected = await this.services.saveNativeDialog("create.save-archive", save, {
      title: source === "drop"
        ? this.tr("gui.create.save_dropped_items_as_archive", "Save dropped items as archive")
        : this.tr("gui.create.save_archive_as", "Save archive as"),
      defaultPath: draft.suggestedDestination ?? joinDesktopPath(
        desktopDirname(inputs[0], platform), `${base}.${draft.outputExtension}`, platform,
      ),
      filters: [{ name: filterName, extensions }],
    });
    if (!isCurrent() || !selected) return null;
    const path = extensions.some((extension) => selected.toLowerCase().endsWith(`.${extension.toLowerCase()}`))
      ? selected
      : `${selected}.${draft.outputExtension}`;
    const inspection = await this.inspectDestination(path, draft.splitSize !== null, draft.sfxTarget, isCurrent);
    if (!isCurrent()) return null;
    if (inspection.conflict && inspection.guard === null) throw new CreateDestinationInspectionError();
    if (inspection.conflict) {
      const replace = await confirm(
        this.tr(
          "gui.create.replace_existing.body",
          "An output file or split volume set already exists for {path}. Replace the existing output set with the new archive?",
        ).replace("{path}", path),
        {
          title: this.tr("gui.create.replace_existing.title", "Replace existing output?"),
          kind: "warning",
          okLabel: this.tr("gui.create.replace_existing.action", "Replace"),
          cancelLabel: this.tr("gui.create.replace_existing.cancel", "Cancel"),
        },
      );
      if (!isCurrent() || !replace) return null;
    }
    return {
      path,
      replaceExisting: inspection.conflict,
      replacementGuard: inspection.guard,
      confirmLateConflict: true,
    };
  }

  private async resolveDestination(
    inputs: readonly string[],
    base: string,
    draft: CreateRunDraft,
    source: "dialog" | "drop",
    isCurrent: () => boolean,
  ): Promise<ResolvedCreateDestination | null> {
    if (draft.destination.base === "ask") return this.askDestination(inputs, base, draft, source, isCurrent);
    const folder = draft.destination.base === "source_parent"
      ? commonCreateSourceParent(inputs, this.services.platform())
      : draft.defaultCreateDir;
    if (!folder) {
      this.notice(draft.destination.base === "source_parent"
        ? this.tr("gui.create.output.source_parent_fallback", "The selected sources are in different folders. Choose where to save this archive.")
        : this.tr("gui.create.output.default_folder_fallback", "The default create folder is not set. Choose where to save this archive."));
      return this.askDestination(inputs, base, draft, source, isCurrent);
    }
    const status = this.tr("gui.create.finding_available_destination", "Finding an available output name...");
    this.data.current = status;
    try {
      const proposed = joinDesktopPath(folder, `${base}.${draft.outputExtension}`, this.services.platform());
      const path = await ipc.uniqueCreateDestination(proposed, draft.splitSize !== null);
      if (!isCurrent()) return null;
      return { path, replaceExisting: false, replacementGuard: null, confirmLateConflict: false };
    } catch {
      if (!isCurrent()) return null;
      this.notice(draft.destination.base === "default_directory"
        ? this.tr("gui.create.output.default_folder_unavailable", "The default create folder is unavailable. Choose another location.")
        : this.tr("gui.create.output.source_folder_unavailable", "The source folder cannot be used for output. Choose another location."));
      return this.askDestination(inputs, base, draft, source, isCurrent);
    } finally {
      if (isCurrent() && this.data.current === status) this.data.current = "";
    }
  }

  private artifactLabel(draft: CreateRunDraft): string {
    if (!draft.sfxEnabled) return createFormats[draft.format].label;
    if (draft.sfxTarget === "macos") return this.tr("gui.create.sfx_macos_output", "macOS app (.app)");
    if (draft.sfxTarget === "windows") return this.tr("gui.create.sfx_windows_output", "Windows app (.exe)");
    return this.tr("gui.create.sfx_linux_output", "Linux executable (.run)");
  }

  async start(
    inputs: readonly string[],
    source: "dialog" | "drop",
    draft: CreateRunDraft,
  ): Promise<void> {
    if (this.closed || !this.context.active || this.picker || this.data.phase === "submitting") return;
    const normalizedInputs = createSourcePaths(mergeCreateSources(
      [], inputs.map((path) => ({ path, kind: "unknown" })), this.services.platform(),
    ));
    if (normalizedInputs.length === 0) {
      this.finishIssue("source", this.tr("gui.create.no_source_items", "No source items selected"));
      return;
    }
    const request = this.begin(draft);
    const isCurrent = () => this.currentOwner(request);
    try {
      const base = normalizedInputs.length === 1
        ? archiveBaseOrDefault(this.services.archiveStemName(desktopBasename(normalizedInputs[0], this.services.platform())))
        : "archive";
      let destination: ResolvedCreateDestination | null;
      try {
        destination = await this.resolveDestination(normalizedInputs, base, draft, source, isCurrent);
      } catch (error) {
        if (!isCurrent()) return;
        if (createDestinationInspectionCancelled(error)) {
          this.finishIssue(
            "destination",
            this.tr("gui.create.destination_check_cancelled", "Output check cancelled · no archive was created"),
            "cancelled",
          );
          await this.presentation.emit({ kind: "focus", target: "primary" }, isCurrent);
          return;
        }
        const message = error instanceof CreateDestinationInspectionError
          ? error.detail
            ? this.presentation.tError(error.detail)
            : this.tr("gui.create.destination_recheck_failed", "Could not check the destination. Review it and try again.")
          : this.tr("gui.create.save_dialog_requires_desktop_dialog", "Save dialog requires the desktop file dialog");
        this.finishIssue("destination", message);
        return;
      }
      if (!isCurrent()) return;
      if (!destination) {
        this.finishIssue(
          "destination",
          this.tr("gui.create.destination_selection_cancelled", "Destination selection cancelled · no archive was created"),
          "cancelled",
        );
        return;
      }
      this.data.destination = destination.path;
      const spec: CompressJobSpec = {
        kind: "compress",
        inputs: normalizedInputs,
        dest: destination.path,
        level: draft.level,
        password: draft.password,
        encrypt_names: draft.encryptNames,
        split_size: draft.splitSize,
        split_mode: draft.splitMode,
        excludes: [...draft.excludes],
        content_policy: draft.contentPolicy,
        sqz_inner_format: draft.sqzInnerFormat,
        sfx_target: draft.sfxTarget,
        replace_existing: destination.replaceExisting,
        replacement_guard: destination.replacementGuard,
        completion: draft.completion,
        post_success: draft.postSuccess,
        test_after_create: draft.testAfterCreate,
      };
      this.data.phase = "measuring";
      let requestId: string | null = null;
      let plan: CreatePlanDto;
      try {
        await this.services.ensurePreflightListener();
        if (!isCurrent()) return;
        requestId = this.services.nextRequestId();
        this.data.requestId = requestId;
        this.data.requestKind = "source";
        this.data.processedBytes = 0;
        plan = await ipc.planCreate(spec, requestId);
        if (!isCurrent()) return;
      } catch {
        if (!isCurrent()) return;
        this.finishIssue("source", this.tr(
          "gui.create.check_excludes_or_permissions",
          "Make sure the output is not selected as a source, then check exclude rules and permissions.",
        ));
        return;
      } finally {
        if (requestId !== null && this.data.requestId === requestId) {
          this.data.requestId = null;
          this.data.requestKind = null;
        }
      }
      this.data.plan = plan;
      if (plan.entries === 0) {
        this.finishIssue("source", this.tr("gui.create.no_entries_after_excludes", "No entries after excludes"));
        return;
      }
      this.data.scanned = plan.entries + plan.deduplicated_entries;
      this.data.current = "";
      let workspace: DiskSpaceDto;
      try {
        this.data.phase = "checkingTemp";
        workspace = await ipc.checkDiskSpace(
          desktopDirname(spec.dest, this.services.platform()), plan.workspace_budget_bytes,
        );
        if (!isCurrent()) return;
      } catch {
        if (!isCurrent()) return;
        this.finishIssue("temp", this.tr("gui.create.temp_preflight_requires_desktop_service", "Workspace check requires the desktop service"));
        return;
      }
      this.data.workspaceDisk = workspace;
      if (!workspace.ok) {
        this.finishIssue("temp", this.tr(
          "gui.create.not_enough_temp_space",
          "Not enough destination space for the creation workspace · {available} available",
        ).replace("{available}", formatBytes(workspace.available_bytes)));
        return;
      }
      if (plan.system_temp_budget_bytes > 0) {
        let systemTemp: DiskSpaceDto;
        try {
          const path = await ipc.tempDir();
          if (!isCurrent()) return;
          systemTemp = await ipc.checkDiskSpace(path, plan.system_temp_budget_bytes);
          if (!isCurrent()) return;
        } catch {
          if (!isCurrent()) return;
          this.finishIssue("temp", this.tr("gui.create.temp_preflight_requires_desktop_service", "Workspace check requires the desktop service"));
          return;
        }
        this.data.systemTempDisk = systemTemp;
        if (!systemTemp.ok) {
          this.finishIssue("temp", this.tr(
            "gui.create.not_enough_system_temp_space",
            "Not enough space in the system temporary directory · {available} available",
          ).replace("{available}", formatBytes(systemTemp.available_bytes)));
          return;
        }
      }
      let disk: DiskSpaceDto;
      try {
        this.data.phase = "checkingDest";
        disk = await ipc.checkDiskSpace(
          desktopDirname(spec.dest, this.services.platform()), plan.final_output_budget_bytes,
        );
        if (!isCurrent()) return;
      } catch {
        if (!isCurrent()) return;
        this.finishIssue("destination", this.tr(
          "gui.create.destination_preflight_requires_desktop_service",
          "Destination disk preflight requires the desktop service",
        ));
        return;
      }
      this.data.destinationDisk = disk;
      if (!disk.ok) {
        this.finishIssue("destination", this.tr(
          "gui.create.not_enough_destination_space",
          "Not enough free space in destination · {available} available",
        ).replace("{available}", formatBytes(disk.available_bytes)));
        return;
      }
      this.data.pending = {
        spec,
        source,
        format: draft.format,
        profile: draft.profile,
        creatingSfx: draft.sfxEnabled,
        artifactLabel: this.artifactLabel(draft),
        splitSize: draft.splitSize,
        confirmLateConflict: destination.confirmLateConflict,
        restoreCredentialPrompt: draft.restoreCredentialPrompt,
        restoreEncryptNames: draft.restoreEncryptNames,
      };
      this.data.phase = "reviewing";
      this.notice(plan.deduplicated_entries > 0
        ? this.tr(
            "gui.create.review_ready_overlap_notice",
            "Checks complete · {count} repeated entries merged · review before creating",
          ).replace("{count}", plan.deduplicated_entries.toLocaleString())
        : this.tr("gui.create.review_ready_notice", "Checks complete · review before creating"));
      await this.presentation.emit({ kind: "focus", target: "review" }, isCurrent);
    } finally {
      if (this.owner === request) {
        if (!isCurrent()) this.dismissPreparation();
        else this.owner = null;
      }
    }
  }

  cancelReview(): void {
    if (!this.data.pending || this.busy()) return;
    this.invalidatePlan();
    const generation = this.generation;
    void this.presentation.emit(
      { kind: "focus", target: "primary" },
      () => !this.closed && this.context.active && this.generation === generation,
    );
    this.notice(this.tr("gui.create.review.cancelled", "Create plan cancelled · no task was added"));
  }

  private async refreshConfirmedDestination(
    spec: CompressJobSpec,
    confirmLateConflict: boolean,
    isCurrent: () => boolean,
  ): Promise<CompressJobSpec | null> {
    if (!confirmLateConflict && (spec.replace_existing !== true || !spec.replacement_guard)) return spec;
    const inspection = await this.inspectDestination(spec.dest, spec.split_size !== null, spec.sfx_target, isCurrent);
    if (!isCurrent()) return null;
    if (!inspection.conflict) return applyCreateDestinationAuthorization(spec, null);
    if (inspection.guard === null) throw new CreateDestinationInspectionError();
    if (spec.replace_existing && inspection.guard === spec.replacement_guard) return spec;
    const { confirm } = await this.services.getDialogModule();
    if (!isCurrent()) return null;
    const replace = await confirm(
      this.tr(
        "gui.create.replace_changed.body",
        "The output at {path} changed after your earlier confirmation. Replace the current output with the new archive?",
      ).replace("{path}", spec.dest),
      {
        title: this.tr("gui.create.replace_changed.title", "Destination changed · replace current output?"),
        kind: "warning",
        okLabel: this.tr("gui.create.replace_changed.action", "Replace current output"),
        cancelLabel: this.tr("gui.create.replace_changed.cancel", "Keep current output"),
      },
    );
    if (!isCurrent() || !replace) return null;
    return applyCreateDestinationAuthorization(spec, inspection.guard);
  }

  async confirm(): Promise<void> {
    if (this.closed || this.busy()) return;
    const pending = this.data.pending;
    const plan = this.data.plan;
    if (!pending || !plan) {
      this.notice(this.tr("gui.create.review.expired", "This create plan is no longer current. Choose the sources again."));
      this.invalidatePlan();
      return;
    }
    const request = {};
    this.submission = request;
    const isCurrent = () => !this.closed && this.submission === request;
    this.data.issue = "";
    this.data.issueStage = null;
    this.data.phase = "submitting";
    void this.presentation.emit({ kind: "prepareSubmit" }, isCurrent);
    try {
      let spec: CompressJobSpec;
      try {
        const refreshed = await this.refreshConfirmedDestination(pending.spec, pending.confirmLateConflict, isCurrent);
        if (!isCurrent()) return;
        if (!refreshed) {
          this.data.phase = "reviewing";
          this.notice(this.tr("gui.create.replace_changed.kept", "Current output kept · nothing was added to the queue"));
          return;
        }
        spec = refreshed;
        if (refreshed !== pending.spec) this.data.pending = { ...pending, spec: refreshed };
      } catch (error) {
        if (!isCurrent()) return;
        if (createDestinationInspectionCancelled(error)) {
          this.clearRequest();
          this.data.issueStage = "destination";
          this.data.issue = this.tr(
            "gui.create.destination_recheck_cancelled",
            "Output recheck cancelled · the plan was not submitted",
          );
          this.data.phase = "reviewing";
          this.notice(this.data.issue);
          await this.presentation.emit({ kind: "focus", target: "review" }, isCurrent);
          return;
        }
        this.finishIssue("destination", error instanceof CreateDestinationInspectionError && error.detail
          ? this.presentation.tError(error.detail)
          : this.tr("gui.create.destination_recheck_failed", "Could not recheck the destination. Review it and try again."));
        return;
      }
      let jobId: number;
      try {
        jobId = await this.submit(spec);
        if (!isCurrent()) return;
      } catch (error) {
        if (!isCurrent()) return;
        const blocked = this.presentation.submissionBlockedMessage(error);
        if (blocked !== null) {
          this.data.issueStage = "submit";
          this.data.issue = blocked;
          this.data.phase = "blocked";
          return;
        }
        this.finishIssue("submit", this.tr(
          "gui.create.submission_requires_desktop_service",
          "Create archive submission requires the desktop service",
        ));
        return;
      }
      this.data.pending = null;
      this.data.phase = "ready";
      await this.presentation.emit({ kind: "queued", pending, plan, jobId }, isCurrent);
    } finally {
      if (this.submission === request) this.submission = null;
    }
  }

  dispose(): void {
    this.closed = true;
    this.generation += 1;
    this.picker = null;
    this.owner = null;
    this.submission = null;
    this.data = emptyData();
  }
}
