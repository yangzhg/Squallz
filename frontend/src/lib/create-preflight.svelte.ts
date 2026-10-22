import { ArchiveOutputWorkflow, type ArchiveOutputState, type ArchiveOutputPhase, type ArchiveOutputStage } from "./archive-output-workflow.svelte";
import { DestinationInspectionError, destinationInspectionCancelled, authorizeArchiveOutput } from "./archive-output-authorization";
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
  type CreateDestinationBase,
  type CreatePlanDto,
  type ErrorDto,
  type JobSpec,
  type OverwritePolicy,
} from "./ipc";
import { createFormats, type CreateFormatId, type CreateProfileId } from "./ui-model";
import type { UiMode } from "./uiMode.svelte";

type CompressJobSpec = Extract<JobSpec, { kind: "compress" }>;
type DialogModule = typeof import("@tauri-apps/plugin-dialog");

export type CreatePreflightStage = ArchiveOutputStage;
export type CreatePreflightPhase = ArchiveOutputPhase;

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
  tr: (key: string, fallback?: string) => string;
  tError: (error: ErrorDto) => string;
  submissionBlockedMessage: (error: unknown) => string | null;
  emit: (effect: CreatePreflightEffect, isCurrent: () => boolean) => void | Promise<void>;
}

export type CreatePreflightState = Readonly<ArchiveOutputState<PendingCreateSubmission> & {
  excludeCount: number;
  creatingSfx: boolean;
  picker: CreateSourcePicker | null;
  owner: CreatePreparationOwner | null;
}>;

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

export class CreatePreflightSession {
  private readonly workflow: ArchiveOutputWorkflow<PendingCreateSubmission>;
  private get data() { return this.workflow.state; }
  private excludeCount = $state(0);
  private creatingSfx = $state(false);
  private picker = $state.raw<CreateSourcePicker | null>(null);
  private owner = $state.raw<CreatePreparationOwner | null>(null);
  private context: CreatePreflightContext = { active: false, mode: "modern", sources: [] };
  private get generation() { return this.workflow.generation; }
  private closed = false;
  private readonly previewDestinationRequestId: string | null;

  constructor(
    private readonly services: CreatePreflightServices,
    private readonly presentation: CreatePreflightPresentation,
    private readonly submit: (spec: CompressJobSpec) => Promise<number>,
    seed?: CreatePreflightSeed,
  ) {
    this.workflow = new ArchiveOutputWorkflow({
      kind: "create", ensureListener: services.ensurePreflightListener,
      nextRequestId: services.nextRequestId, getDialogModule: services.getDialogModule,
      tr: (key) => this.tr(key), notice: (message) => this.notice(message), tError: presentation.tError,
    });
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
    return { ...this.data, excludeCount: this.excludeCount, creatingSfx: this.creatingSfx, picker: this.picker, owner: this.owner };
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

  private tr(key: string, fallback?: string): string {
    return this.presentation.tr(key, fallback);
  }

  private notice(message: string): void {
    if (!this.closed) void this.presentation.emit({ kind: "notice", message }, () => !this.closed);
  }

  busy(): boolean {
    return this.workflow.busy();
  }

  sourcesLocked(): boolean {
    return this.picker !== null || this.busy() || this.data.pending !== null;
  }

  canLeave(): boolean {
    return this.data.phase !== "submitting";
  }

  dismissPreparation(): void {
    if (this.data.phase === "submitting") return;
    this.workflow.generation += 1;
    this.picker = null;
    if (this.owner) this.invalidatePlan();
  }

  invalidatePlan(): void {
    if (this.busy() && this.owner === null) return;
    this.owner = null;
    const pending = this.workflow.reset();
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
      ? this.tr("gui.create.opening_file_picker")
      : this.tr("gui.create.opening_folder_picker"));
    try {
      const { open } = await this.services.getDialogModule();
      if (!isCurrent()) return;
      const selected = await this.services.openNativeDialog(`create.${kind}`, open, {
        title: kind === "files"
          ? this.tr("gui.create.choose_files_to_archive")
          : this.tr("gui.create.choose_folder_to_archive", "Choose folder to archive"),
        multiple: true,
        directory: kind === "folder",
      });
      if (!isCurrent()) return;
      const paths = Array.isArray(selected) ? selected : selected ? [selected] : [];
      if (paths.length === 0) {
        this.notice(this.tr("gui.create.sources.picker_cancelled"));
        return;
      }
      this.picker = null;
      const generation = this.generation;
      await this.presentation.emit({
        kind: "sourcesPicked", paths, sourceKind: kind === "files" ? "file" : "folder",
      }, () => this.sameContext(request) && this.generation === generation);
    } catch {
      if (isCurrent()) this.notice(this.tr("gui.create.requires_desktop_dialog"));
    } finally {
      if (this.picker === request) this.picker = null;
    }
  }

  private begin(draft: CreateRunDraft): CreatePreparationOwner {
    this.dismissPreparation();
    this.workflow.begin();
    this.excludeCount = draft.contentPolicy === "cross_platform_clean" ? 3 : draft.excludes.length;
    this.creatingSfx = draft.sfxEnabled;
    const request = { mode: this.context.mode, sources: this.context.sources };
    this.owner = request;
    return request;
  }

  private finishIssue(
    stage: CreatePreflightStage,
    message: string,
    phase: "blocked" | "cancelled" = "blocked",
  ): void {
    this.workflow.issue(stage, message, phase);
  }

  async cancelDestination(
    options: { announce?: boolean; keepIntentOnFailure?: boolean } = {},
  ): Promise<void> {
    const announce = options.announce ?? true;
    const requestId = this.data.requestId;
    if (!requestId || this.data.requestKind !== "destination" || this.data.cancelPending) return;
    if (import.meta.env.DEV && requestId === this.previewDestinationRequestId) {
      this.data.cancelPending = true;
      const request = { mode: this.context.mode, sources: this.context.sources };
      this.owner = request;
      const isCurrent = () => this.currentOwner(request);
      try {
        await new Promise((resolve) => setTimeout(resolve, 180));
        if (!isCurrent() || this.data.requestId !== requestId || !this.data.cancelPending) return;
        this.finishIssue(
          "destination",
          this.tr("gui.create.destination_check_cancelled"),
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
    await this.workflow.cancel(announce, options.keepIntentOnFailure);
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
      filterName = this.tr("gui.create.sfx_filter");
    } else if (draft.outputExtension === "swm") {
      filterName = this.tr("gui.create.split_wim_filter");
    } else {
      filterName = this.tr(`gui.create.format.${draft.format}.filter`, createFormats[draft.format].filterName);
    }
    const platform = this.services.platform();
    const selected = await this.services.saveNativeDialog("create.save-archive", save, {
      title: source === "drop"
        ? this.tr("gui.create.save_dropped_items_as_archive")
        : this.tr("gui.create.save_archive_as"),
      defaultPath: draft.suggestedDestination ?? joinDesktopPath(
        desktopDirname(inputs[0], platform), `${base}.${draft.outputExtension}`, platform,
      ),
      filters: [{ name: filterName, extensions }],
    });
    if (!isCurrent() || !selected) return null;
    const path = extensions.some((extension) => selected.toLowerCase().endsWith(`.${extension.toLowerCase()}`))
      ? selected
      : `${selected}.${draft.outputExtension}`;
    const authorization = await authorizeArchiveOutput({
      path, inspect: () => this.workflow.inspect(path, draft.splitSize !== null, draft.sfxTarget, isCurrent),
      isCurrent, confirm: async () => confirm, tr: (key) => this.tr(key), message: "gui.create.replace_existing",
    });
    return authorization ? { path, ...authorization, confirmLateConflict: true } : null;
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
        ? this.tr("gui.create.output.source_parent_fallback")
        : this.tr("gui.create.output.default_folder_fallback"));
      return this.askDestination(inputs, base, draft, source, isCurrent);
    }
    const status = this.tr("gui.create.finding_available_destination");
    this.data.current = status;
    try {
      const proposed = joinDesktopPath(folder, `${base}.${draft.outputExtension}`, this.services.platform());
      const path = await ipc.uniqueCreateDestination(proposed, draft.splitSize !== null);
      if (!isCurrent()) return null;
      return { path, replaceExisting: false, replacementGuard: null, confirmLateConflict: false };
    } catch {
      if (!isCurrent()) return null;
      this.notice(draft.destination.base === "default_directory"
        ? this.tr("gui.create.output.default_folder_unavailable")
        : this.tr("gui.create.output.source_folder_unavailable"));
      return this.askDestination(inputs, base, draft, source, isCurrent);
    } finally {
      if (isCurrent() && this.data.current === status) this.data.current = "";
    }
  }

  private artifactLabel(draft: CreateRunDraft): string {
    if (!draft.sfxEnabled) return createFormats[draft.format].label;
    if (draft.sfxTarget === "macos") return this.tr("gui.create.sfx_macos_output");
    if (draft.sfxTarget === "windows") return this.tr("gui.create.sfx_windows_output");
    return this.tr("gui.create.sfx_linux_output");
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
      this.finishIssue("source", this.tr("gui.create.no_source_items"));
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
        if (destinationInspectionCancelled(error)) {
          this.finishIssue(
            "destination",
            this.tr("gui.create.destination_check_cancelled"),
            "cancelled",
          );
          await this.presentation.emit({ kind: "focus", target: "primary" }, isCurrent);
          return;
        }
        const message = error instanceof DestinationInspectionError
          ? error.detail
            ? this.presentation.tError(error.detail)
            : this.tr("gui.create.destination_recheck_failed", "Could not check the destination. Review it and try again.")
          : this.tr("gui.create.save_dialog_requires_desktop_dialog");
        this.finishIssue("destination", message);
        return;
      }
      if (!isCurrent()) return;
      if (!destination) {
        this.finishIssue(
          "destination",
          this.tr("gui.create.destination_selection_cancelled"),
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
      const outcome = await this.workflow.prepare(spec, desktopDirname(spec.dest, this.services.platform()), isCurrent);
      if (!isCurrent() || outcome.status === "stale") return;
      if (outcome.status === "cancelled") {
        this.finishIssue("source", this.tr("gui.create.preflight_cancelled"), "cancelled");
        return;
      }
      if (outcome.status === "error") {
        const messages = {
          plan: "gui.create.check_excludes_or_permissions",
          empty: "gui.create.no_entries_after_excludes",
          workspace_service: "gui.create.temp_preflight_requires_desktop_service",
          workspace_space: "gui.create.not_enough_temp_space",
          system_temp_space: "gui.create.not_enough_system_temp_space",
          destination_service: "gui.create.destination_preflight_requires_desktop_service",
          destination_space: "gui.create.not_enough_destination_space",
        };
        this.finishIssue(outcome.stage, this.tr(messages[outcome.code])
          .replace("{available}", formatBytes("availableBytes" in outcome ? outcome.availableBytes ?? 0 : 0)));
        return;
      }
      const plan = outcome.plan;
      this.workflow.review({
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
      });
      this.notice(plan.deduplicated_entries > 0
        ? this.tr(
            "gui.create.review_ready_overlap_notice",
          ).replace("{count}", plan.deduplicated_entries.toLocaleString())
        : this.tr("gui.create.review_ready_notice"));
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
    this.notice(this.tr("gui.create.review.cancelled"));
  }

  async confirm(): Promise<void> {
    if (this.closed || this.busy()) return;
    const pending = this.data.pending;
    const plan = this.data.plan;
    if (!pending || !plan) {
      this.notice(this.tr("gui.create.review.expired"));
      this.invalidatePlan();
      return;
    }
    await this.workflow.confirm({
      recheck: pending.confirmLateConflict || (pending.spec.replace_existing && Boolean(pending.spec.replacement_guard)),
      prepare: (isCurrent) => { void this.presentation.emit({ kind: "prepareSubmit" }, isCurrent); },
      submit: this.submit,
      blockedMessage: this.presentation.submissionBlockedMessage,
      queued: (queued, plan, jobId, isCurrent) => this.presentation.emit({ kind: "queued", pending: queued, plan, jobId }, isCurrent),
      focusReview: (isCurrent) => this.presentation.emit({ kind: "focus", target: "review" }, isCurrent),
    });
  }

  dispose(): void {
    this.workflow.dispose();
    this.closed = true;
    this.picker = null;
    this.owner = null;
    this.excludeCount = 0;
    this.creatingSfx = false;
  }
}
