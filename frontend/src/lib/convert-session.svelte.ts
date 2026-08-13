import { tick } from "svelte";
import type { ConvertWorkspaceSurface } from "../components/ConvertWorkspace.svelte";
import { fat32CompatibleSplitSizeBytes, resolveSplitSizeBytes } from "./archive-output-options";
import { ensureConvertOutputExtension, sourceMatchesConvertTarget, suggestedConvertTargetFormat } from "./convert-format";
import { desktopDirname, sameDesktopPath } from "./desktop-path";
import { basename as pathBaseName, dirname as pathDir, formatBytes } from "./format";
import { ipc, isErrorDto, type ArchiveInfo, type CreateDestinationInspectionDto, type CreatePlanDto, type DiskSpaceDto, type ErrorDto, type JobSpec } from "./ipc";
import type { ConvertPreflightOutcome } from "./convert-preflight";
import type {
  ConvertPreflightEvent,
  ConvertRouteBridge,
  ConvertRouteHandle,
  ConvertRouteOwner,
  ConvertWorkspaceVariant,
} from "./convert-route";
import {
  createFormatIds,
  createFormats,
  createProfileIds,
  createProfiles,
  type CreateFormatId,
  type CreateProfileId,
  type CreateSplitMode,
  type CreateSplitPreset,
  type CreateSplitUnit,
} from "./ui-model";

type ConvertPreflightStage = "source" | "temp" | "destination" | "submit";
type ConvertPreflightPhase =
  | "idle"
  | "choosingDest"
  | "measuring"
  | "checkingTemp"
  | "checkingDest"
  | "reviewing"
  | "submitting"
  | "ready"
  | "cancelled"
  | "blocked";
type ConvertJobSpec = Extract<JobSpec, { kind: "convert" }>;
type PendingConvertSubmission = Readonly<{
  spec: ConvertJobSpec;
  targetFormat: CreateFormatId;
  profile: CreateProfileId;
  sourceTitle: string;
  splitSize: number | null;
}>;
type ConvertRequest = Readonly<{ id: string; kind: "plan" | "destination" }>;

class ConvertDestinationInspectionError extends Error {
  readonly detail: ErrorDto | null;
  readonly cancelled: boolean;

  constructor(error?: unknown, cancelled = false) {
    super("convert-destination-inspection-failed");
    this.detail = isErrorDto(error) ? error : null;
    this.cancelled = cancelled;
  }
}

function loadProfile(): CreateProfileId {
  try {
    const value = window.localStorage.getItem("squallz.createProfile");
    return createProfileIds.includes(value as CreateProfileId) ? value as CreateProfileId : "balanced";
  } catch {
    return "balanced";
  }
}

function clampLevel(value: number): number {
  if (!Number.isFinite(value)) return 6;
  return Math.min(9, Math.max(1, Math.round(value)));
}

function loadCustomLevel(): number {
  try {
    const value = window.localStorage.getItem("squallz.customCreateLevel");
    return value === null ? 6 : clampLevel(Number(value));
  } catch {
    return 6;
  }
}

export class ConvertSession implements ConvertRouteHandle {
  private archiveIdentity: string | null = null;
  private activeArchive = $state<ArchiveInfo | null>(null);
  private deferredArchive: { value: ArchiveInfo | null } | null = null;
  private targetFormat = $state<CreateFormatId>("zip");
  private profile = $state<CreateProfileId>(loadProfile());
  private customLevel = $state(loadCustomLevel());
  private customLevelError = $state("");
  private password = $state("");
  private passwordConfirmation = $state("");
  private passwordVisible = $state(false);
  private encryptNames = $state(false);
  private splitPreset = $state<CreateSplitPreset>("none");
  private splitMode = $state<CreateSplitMode>("generic");
  private customSplitAmount = $state("100");
  private customSplitUnit = $state<CreateSplitUnit>("mib");
  private validationAttempted = $state(false);
  private advancedOpen = $state(false);
  private plan = $state<CreatePlanDto | null>(null);
  private destinationDisk = $state<DiskSpaceDto | null>(null);
  private workspaceDisk = $state<DiskSpaceDto | null>(null);
  private systemTempDisk = $state<DiskSpaceDto | null>(null);
  private destination = $state<string | null>(null);
  private pending = $state<PendingConvertSubmission | null>(null);
  private phase = $state<ConvertPreflightPhase>("idle");
  private current = $state("");
  private issue = $state("");
  private issueStage = $state<ConvertPreflightStage | null>(null);
  private requestId: string | null = null;
  private requestKind = $state<"plan" | "destination" | null>(null);
  private cancelPending = $state(false);
  private generation = 0;

  constructor(
    private readonly owner: ConvertRouteOwner,
    private bridge: ConvertRouteBridge,
  ) {}

  updateBridge(bridge: ConvertRouteBridge): void {
    this.bridge = bridge;
  }

  private tr(key: string, fallback: string): string {
    return this.bridge.tr(key, fallback);
  }

  private archive(): ArchiveInfo | null {
    return this.activeArchive;
  }

  private applyArchive(archive: ArchiveInfo | null): void {
    const identity = archive ? `${archive.id}:${archive.source}` : null;
    this.activeArchive = archive;
    if (identity === this.archiveIdentity) return;
    this.archiveIdentity = identity;
    const request = this.activeRequest();
    this.resetPreflight(true);
    if (request) this.cancelRequest(request);
    this.targetFormat = suggestedConvertTargetFormat(archive?.format);
    this.customLevelError = "";
    this.resetOutputOptions();
  }

  private flushDeferredArchive(): void {
    const deferred = this.deferredArchive;
    if (!deferred) return;
    this.deferredArchive = null;
    this.applyArchive(deferred.value);
  }

  private openArchiveLabel(): string {
    return this.tr("gui.empty.open_archive_first", "Open archive");
  }

  private profileLabel(profile: CreateProfileId = this.profile): string {
    return this.tr(`gui.create.profile.${profile}`, createProfiles[profile].label);
  }

  private profileDetail(profile: CreateProfileId = this.profile): string {
    if (profile === "custom") {
      return this.tr("gui.convert.custom_level_detail", "Choose an exact level for this conversion");
    }
    return this.tr(`gui.create.profile.${profile}.detail`, createProfiles[profile].detail);
  }

  private compressionLevel(profile: CreateProfileId = this.profile): number {
    return profile === "custom" ? this.customLevel : createProfiles[profile].level;
  }

  private formatMethod(format: CreateFormatId = this.targetFormat): string {
    return this.tr(`gui.create.format.${format}.method`, createFormats[format].method);
  }

  private methodLabel(
    format: CreateFormatId = this.targetFormat,
    profile: CreateProfileId = this.profile,
  ): string {
    return this.tr("gui.convert.method_level", "{method} · Level {level}")
      .replace("{method}", this.formatMethod(format))
      .replace("{level}", String(this.compressionLevel(profile)));
  }

  private canEncryptData(format: CreateFormatId = this.targetFormat): boolean {
    return createFormats[format].can_encrypt_data;
  }

  private canEncryptNames(format: CreateFormatId = this.targetFormat): boolean {
    return createFormats[format].can_encrypt_names;
  }

  private nameEncryptionCapability(format: CreateFormatId = this.targetFormat): string {
    if (this.canEncryptNames(format)) {
      return this.tr("gui.create.name_encryption_available", "7Z can hide file names");
    }
    if (format === "zip") {
      return this.tr("gui.create.name_encryption_zip_visible", "ZIP names stay visible; use 7Z");
    }
    return this.tr("gui.create.name_encryption_unavailable", "File name encryption unavailable");
  }

  private nativeSplitKind(format: CreateFormatId = this.targetFormat): "zip" | "wim" | null {
    return format === "zip" || format === "wim" ? format : null;
  }

  private splitSize(): number | null {
    return resolveSplitSizeBytes(
      this.splitPreset,
      this.customSplitAmount,
      this.customSplitUnit,
    );
  }

  private outputExtension(format: CreateFormatId = this.targetFormat): string {
    return format === "wim" && this.splitSize() !== null && this.splitMode === "native"
      ? "swm"
      : createFormats[format].extension;
  }

  private defaultDestination(format: CreateFormatId = this.targetFormat): string {
    const archive = this.archive();
    if (!archive) return this.openArchiveLabel();
    const base = this.bridge.archiveStemName(archive.name);
    const outputBase = sourceMatchesConvertTarget(archive.format, format)
      ? `${base}.converted`
      : base;
    return `${pathDir(archive.path)}/${outputBase}.${this.outputExtension(format)}`;
  }

  private destinationPreview(): string {
    if (this.plan) return this.plan.primary_output;
    if (this.destination) return this.destination;
    const destination = this.defaultDestination();
    if (this.splitSize() === null) return destination;
    if (this.splitMode === "native") {
      return this.targetFormat === "wim"
        ? this.tr(
            "gui.convert.native_split_wim_destination_preview",
            "{destination} is the first part; following parts add 2, 3, … before .swm",
          ).replace("{destination}", destination)
        : this.tr(
            "gui.convert.native_split_destination_preview",
            "{destination} → .z01, .z02, …, final .zip",
          ).replace("{destination}", destination);
    }
    return this.tr(
      "gui.convert.split_destination_preview",
      "{destination} → {destination}.001, .002, …",
    ).replaceAll("{destination}", destination);
  }

  private volumePreview(): string {
    const splitSize = this.splitSize();
    if (splitSize === null) {
      return this.tr("gui.convert.single_archive_summary", "Single converted archive · no numbered parts");
    }
    const nativeWim = this.splitMode === "native" && this.targetFormat === "wim";
    const key = nativeWim
      ? "gui.convert.native_split_wim_summary"
      : this.splitMode === "native"
        ? "gui.convert.native_split_summary"
        : "gui.convert.split_summary";
    const fallback = nativeWim
      ? "{size} target per part · native .swm set; one large file may exceed the target"
      : this.splitMode === "native"
        ? "{size} per part · native ZIP set ending in .zip"
        : "{size} per part · the exact file list appears when conversion finishes";
    return this.tr(key, fallback).replace("{size}", formatBytes(splitSize));
  }

  private sourceSummary(): string {
    const archive = this.archive();
    if (!archive) return this.openArchiveLabel();
    const diagnostics = archive.garbled_count
      ? this.tr("gui.archive.names_review", "{count} names need review")
          .replace("{count}", archive.garbled_count.toLocaleString())
      : archive.non_utf8_name_count
        ? this.tr("gui.archive.non_utf8_names", "{count} non-UTF-8 names")
            .replace("{count}", archive.non_utf8_name_count.toLocaleString())
        : this.tr("gui.archive.names_clean", "Names decoded cleanly");
    return this.tr("gui.convert.source_summary", "{count} entries · {diagnostics}")
      .replace("{count}", archive.entry_count.toLocaleString())
      .replace("{diagnostics}", diagnostics);
  }

  private formatNote(format: CreateFormatId = this.targetFormat): string {
    const archive = this.archive();
    if (archive && sourceMatchesConvertTarget(archive.format, format)) {
      return this.tr("gui.convert.same_format_note", "Target format equals the source; it will be recompressed.");
    }
    return this.tr(`gui.create.format.${format}.note`, createFormats[format].note);
  }

  private requiredReason(): string {
    if (!this.archive()) {
      return this.tr("gui.precondition.open_before_convert", "Open an archive before converting");
    }
    return this.profile === "custom" ? this.customLevelError : "";
  }

  private busy(): boolean {
    return ["choosingDest", "measuring", "checkingTemp", "checkingDest", "submitting"]
      .includes(this.phase);
  }

  private lockedReason(): string {
    if (this.busy()) {
      return this.tr("gui.convert.options_locked", "Conversion options are locked while checks are running");
    }
    return this.pending
      ? this.tr("gui.convert.options_locked_review", "Cancel the current plan before changing conversion options")
      : "";
  }

  private passwordError(): string {
    if (!this.canEncryptData() || this.password.length === 0) return "";
    if (this.passwordConfirmation.length === 0) {
      return this.tr("gui.convert.confirm_password_required", "Confirm the destination password before starting");
    }
    return this.password === this.passwordConfirmation
      ? ""
      : this.tr("gui.convert.passwords_do_not_match", "The destination passwords do not match");
  }

  private splitError(): string {
    const splitSize = this.splitSize();
    if (this.splitPreset === "custom" && splitSize === null) {
      return this.tr("gui.convert.invalid_part_size", "Enter a part size of at least 0.1 MiB");
    }
    if (
      this.targetFormat === "zip"
      && this.splitMode === "native"
      && splitSize !== null
      && splitSize > fat32CompatibleSplitSizeBytes
    ) {
      return this.tr("gui.create.native_zip_part_size_limit", "Native ZIP parts cannot exceed 4 GiB − 1 byte");
    }
    return "";
  }

  private visiblePasswordError(): string {
    const error = this.passwordError();
    return this.validationAttempted || this.passwordConfirmation.length > 0 ? error : "";
  }

  private visibleSplitError(): string {
    const error = this.splitError();
    return this.validationAttempted || this.customSplitAmount.length > 0 ? error : "";
  }

  private clearPassword(): void {
    this.password = "";
    this.passwordConfirmation = "";
    this.passwordVisible = false;
    this.encryptNames = false;
  }

  private resetOutputOptions(): void {
    this.clearPassword();
    this.splitPreset = "none";
    this.splitMode = "generic";
    this.customSplitAmount = "100";
    this.customSplitUnit = "mib";
    this.validationAttempted = false;
    this.advancedOpen = false;
  }

  private activeRequest(): ConvertRequest | null {
    return this.requestId && this.requestKind
      ? { id: this.requestId, kind: this.requestKind }
      : null;
  }

  private cancelRequest(request: ConvertRequest): void {
    const promise = request.kind === "plan"
      ? ipc.cancelConvertPlan(request.id)
      : ipc.cancelCreateDestinationInspection(request.id);
    void promise.catch(() => {});
  }

  private resetPreflight(clearPassword: boolean): void {
    this.generation += 1;
    this.phase = "idle";
    this.current = "";
    this.issue = "";
    this.issueStage = null;
    this.requestId = null;
    this.requestKind = null;
    this.cancelPending = false;
    this.plan = null;
    this.destinationDisk = null;
    this.workspaceDisk = null;
    this.systemTempDisk = null;
    this.destination = null;
    this.pending = null;
    if (clearPassword) this.clearPassword();
  }

  private beginPreflight(): number {
    this.generation += 1;
    this.phase = "choosingDest";
    this.current = "";
    this.issue = "";
    this.issueStage = null;
    this.requestId = null;
    this.requestKind = null;
    this.cancelPending = false;
    this.plan = null;
    this.destinationDisk = null;
    this.workspaceDisk = null;
    this.systemTempDisk = null;
    this.destination = null;
    this.pending = null;
    return this.generation;
  }

  private isCurrent(generation: number): boolean {
    return generation === this.generation;
  }

  private finishWithIssue(
    stage: ConvertPreflightStage,
    message: string,
    phase: "blocked" | "cancelled" = "blocked",
  ): void {
    this.issueStage = stage;
    this.issue = message;
    this.current = "";
    this.requestId = null;
    this.requestKind = null;
    this.cancelPending = false;
    this.phase = phase;
    this.pending = null;
    this.bridge.showNotice(message);
  }

  private validate(): boolean {
    this.validationAttempted = true;
    const error = this.passwordError() || this.splitError();
    if (!error) return true;
    this.advancedOpen = true;
    this.bridge.showNotice(error);
    return false;
  }

  private async inspectDestination(path: string, split: boolean): Promise<CreateDestinationInspectionDto> {
    await this.bridge.ensurePreflightListener();
    const requestId = nextRequestId();
    this.requestId = requestId;
    this.requestKind = "destination";
    this.cancelPending = false;
    this.current = "";
    try {
      const inspection = await ipc.inspectCreateDestination(path, split, requestId, null);
      if (this.requestId === requestId && this.cancelPending) {
        throw new ConvertDestinationInspectionError(undefined, true);
      }
      return inspection;
    } catch (error) {
      const cancelled = this.requestId === requestId && this.cancelPending;
      if (error instanceof ConvertDestinationInspectionError) {
        if (!cancelled || error.cancelled) throw error;
        throw new ConvertDestinationInspectionError(error.detail ?? undefined, true);
      }
      throw new ConvertDestinationInspectionError(error, cancelled);
    } finally {
      if (this.requestId === requestId) {
        this.requestId = null;
        this.requestKind = null;
        this.cancelPending = false;
        this.current = "";
      }
    }
  }

  private inspectionCancelled(error: unknown): boolean {
    return error instanceof ConvertDestinationInspectionError
      && (error.cancelled || error.detail?.key === "error.cancelled");
  }

  private async authorizeDestination(
    path: string,
    confirm: typeof import("@tauri-apps/plugin-dialog")["confirm"],
    split: boolean,
  ): Promise<{ replaceExisting: boolean; replacementGuard: string | null } | null> {
    const inspection = await this.inspectDestination(path, split);
    if (inspection.conflict !== (inspection.guard !== null)) {
      throw new ConvertDestinationInspectionError();
    }
    if (inspection.conflict) {
      const replace = await confirm(
        (split
          ? this.tr(
              "gui.output.replace_existing_split.body",
              "An archive or numbered volume set already exists for {path}. Replace only this exact output set? If another app changes it before the task finishes, Squallz will keep it.",
            )
          : this.tr(
              "gui.output.replace_existing.body",
              "An archive already exists at {path}. Replace only this exact version? If another app changes it before the task finishes, Squallz will keep it.",
            )).replace("{path}", path),
        {
          title: split
            ? this.tr("gui.output.replace_existing_split.title", "Replace existing output set?")
            : this.tr("gui.output.replace_existing.title", "Replace existing archive?"),
          kind: "warning",
          okLabel: this.tr("gui.output.replace_existing.action", "Replace"),
          cancelLabel: this.tr("gui.output.replace_existing.cancel", "Cancel"),
        },
      );
      if (!replace) return null;
    }
    return {
      replaceExisting: inspection.conflict,
      replacementGuard: inspection.guard,
    };
  }

  private preflightError(outcome: Extract<ConvertPreflightOutcome, { status: "error" }>): string {
    switch (outcome.code) {
      case "plan":
        return isErrorDto(outcome.error)
          ? this.bridge.tError(outcome.error)
          : this.tr("gui.convert.plan_failed", "Could not read the archive metadata. Check the password and archive, then try again.");
      case "workspace_space":
        return this.tr("gui.convert.not_enough_workspace_space", "Not enough destination space for the conversion workspace · {available} available")
          .replace("{available}", formatBytes(outcome.availableBytes ?? 0));
      case "system_temp_space":
        return this.tr("gui.convert.not_enough_system_temp_space", "Not enough space in the system temporary directory · {available} available")
          .replace("{available}", formatBytes(outcome.availableBytes ?? 0));
      case "destination_space":
        return this.tr("gui.convert.not_enough_destination_space", "Not enough free space in the destination · {available} available")
          .replace("{available}", formatBytes(outcome.availableBytes ?? 0));
      case "destination_service":
        return this.tr("gui.convert.destination_check_requires_desktop_service", "Destination disk check requires the desktop service");
      case "workspace_service":
        return this.tr("gui.convert.workspace_check_requires_desktop_service", "Workspace check requires the desktop service");
    }
  }

  private async start(): Promise<void> {
    const archive = this.archive();
    if (!archive) {
      this.bridge.showNotice(this.tr("gui.precondition.open_before_convert", "Open an archive before converting"));
      return;
    }
    if (this.profile === "custom" && this.customLevelError) {
      this.bridge.showNotice(this.customLevelError);
      return;
    }
    if (!this.validate()) return;
    if (this.busy() || this.pending || this.bridge.focusBlockingTaskIfAny()) return;
    const targetFormat = this.targetFormat;
    const profile = this.profile;
    const defaultDest = this.defaultDestination(targetFormat);
    const level = this.compressionLevel(profile);
    const splitSize = this.splitSize();
    const destPassword = this.canEncryptData(targetFormat) && this.password.length > 0
      ? this.password
      : null;
    const encryptNames = Boolean(destPassword) && this.encryptNames && this.canEncryptNames(targetFormat);
    const sourceTitle = archive.name;
    const generation = this.beginPreflight();
    try {
      const { confirm, save } = await this.bridge.getDialogModule();
      if (!this.isCurrent(generation)) return;
      const selected = await this.bridge.saveNativeDialog("convert.save-archive", save, {
        title: this.tr("gui.convert.save_as", "Convert archive as"),
        defaultPath: defaultDest,
        filters: [{
          name: this.tr(`gui.create.format.${targetFormat}.filter`, createFormats[targetFormat].filterName),
          extensions: this.outputExtension(targetFormat) === "swm"
            ? ["swm"]
            : createFormats[targetFormat].extensions,
        }],
      });
      if (!this.isCurrent(generation)) return;
      if (!selected) {
        this.clearPassword();
        this.finishWithIssue(
          "destination",
          this.tr("gui.convert.destination_selection_cancelled", "Destination selection cancelled · no task was added"),
          "cancelled",
        );
        return;
      }
      const dest = ensureConvertOutputExtension(
        selected,
        targetFormat,
        this.outputExtension(targetFormat) === "swm" ? "swm" : undefined,
      );
      this.destination = dest;
      if (sameDesktopPath(dest, archive.path, this.bridge.platform())) {
        this.finishWithIssue("destination", this.tr("gui.convert.same_path", "Target cannot be the same as the source"));
        return;
      }
      const authorization = await this.authorizeDestination(dest, confirm, splitSize !== null);
      if (!this.isCurrent(generation)) return;
      if (!authorization) {
        this.clearPassword();
        this.finishWithIssue(
          "destination",
          this.tr("gui.convert.replacement_cancelled", "Existing output kept · no task was added"),
          "cancelled",
        );
        return;
      }
      const spec: ConvertJobSpec = {
        kind: "convert",
        src: archive.source,
        dest,
        level,
        src_encoding: archive.encoding_override,
        src_password: null,
        dest_password: destPassword,
        encrypt_names: encryptNames,
        split_size: splitSize,
        split_mode: splitSize === null ? "generic" : this.splitMode,
        replace_existing: authorization.replaceExisting,
        replacement_guard: authorization.replacementGuard,
      };
      const requestId = nextRequestId();
      this.requestId = requestId;
      this.requestKind = "plan";
      this.cancelPending = false;
      this.phase = "measuring";
      const { runConvertPreflight } = await import("./convert-preflight");
      if (!this.isCurrent(generation)) return;
      const outcome = await runConvertPreflight({
        spec,
        requestId,
        destinationDirectory: desktopDirname(dest, this.bridge.platform()),
        isCurrent: () => this.isCurrent(generation),
        cancelRequested: () => this.requestId === requestId && this.cancelPending,
        onPlanRequestComplete: () => {
          if (this.requestId !== requestId) return;
          this.requestId = null;
          this.requestKind = null;
          this.cancelPending = false;
        },
        onPhase: (phase) => (this.phase = phase),
        onPlan: (plan) => (this.plan = plan),
        onTempDisk: (disk) => (this.workspaceDisk = disk),
        onSystemTempDisk: (disk) => (this.systemTempDisk = disk),
        onDestinationDisk: (disk) => (this.destinationDisk = disk),
      });
      if (outcome.status === "stale") return;
      if (outcome.status === "cancelled") {
        this.clearPassword();
        this.finishWithIssue(
          "source",
          this.tr("gui.convert.preflight_cancelled_notice", "Conversion checks cancelled · no task was added"),
          "cancelled",
        );
        return;
      }
      if (outcome.status === "error") {
        this.finishWithIssue(outcome.stage, this.preflightError(outcome));
        return;
      }
      this.pending = { spec, targetFormat, profile, sourceTitle, splitSize };
      this.issue = "";
      this.issueStage = null;
      this.phase = "reviewing";
      this.bridge.showNotice(this.tr("gui.convert.review_ready_notice", "Checks complete · review before converting"));
      void tick().then(() => {
        if (this.isCurrent(generation) && typeof document !== "undefined") {
          document.querySelector<HTMLElement>(".create-plan-review")?.focus({ preventScroll: false });
        }
      });
    } catch (error) {
      if (!this.isCurrent(generation)) return;
      if (this.inspectionCancelled(error)) {
        this.clearPassword();
        this.finishWithIssue(
          "destination",
          this.tr("gui.convert.preflight_cancelled_notice", "Conversion checks cancelled · no task was added"),
          "cancelled",
        );
      } else if (error instanceof ConvertDestinationInspectionError) {
        this.finishWithIssue(
          "destination",
          error.detail
            ? this.bridge.tError(error.detail)
            : this.tr("gui.output.inspect_failed", "Could not check the output. Review the destination and try again."),
        );
      } else {
        this.finishWithIssue(
          "destination",
          this.tr("gui.convert.requires_desktop_service", "Conversion requires the desktop service"),
        );
      }
    }
  }

  private async refreshDestination(spec: ConvertJobSpec): Promise<ConvertJobSpec | null> {
    const inspection = await this.inspectDestination(spec.dest, spec.split_size !== null);
    if (!inspection.conflict) {
      return { ...spec, replace_existing: false, replacement_guard: null };
    }
    if (inspection.guard === null) throw new ConvertDestinationInspectionError();
    if (spec.replace_existing && inspection.guard === spec.replacement_guard) return spec;
    const { confirm } = await this.bridge.getDialogModule();
    const replace = await confirm(
      this.tr(
        "gui.convert.replace_changed.body",
        "The output at {path} changed after the plan was checked. Replace the current output with the converted archive?",
      ).replace("{path}", spec.dest),
      {
        title: this.tr("gui.convert.replace_changed.title", "Destination changed · replace current output?"),
        kind: "warning",
        okLabel: this.tr("gui.convert.replace_changed.action", "Replace current output"),
        cancelLabel: this.tr("gui.convert.replace_changed.cancel", "Keep current output"),
      },
    );
    return replace
      ? { ...spec, replace_existing: true, replacement_guard: inspection.guard }
      : null;
  }

  private async confirm(): Promise<void> {
    if (this.busy()) return;
    const generation = this.generation;
    const pending = this.pending;
    const plan = this.plan;
    if (!pending || !plan) {
      this.bridge.showNotice(this.tr("gui.convert.review.expired", "This conversion plan is no longer current. Start the checks again."));
      this.resetPreflight(true);
      return;
    }
    if (this.bridge.focusBlockingTaskIfAny()) return;
    this.issue = "";
    this.issueStage = null;
    this.phase = "submitting";
    this.bridge.prepareSubmitFocus();
    let submissionSpec: ConvertJobSpec;
    try {
      const refreshed = await this.refreshDestination(pending.spec);
      if (!this.isCurrent(generation)) return;
      if (!refreshed) {
        this.phase = "reviewing";
        this.bridge.showNotice(this.tr("gui.convert.replace_changed.kept", "Current output kept · nothing was added to the queue"));
        this.flushDeferredArchive();
        return;
      }
      submissionSpec = refreshed;
      if (refreshed !== pending.spec) this.pending = { ...pending, spec: refreshed };
    } catch (error) {
      if (!this.isCurrent(generation)) return;
      if (this.inspectionCancelled(error)) {
        this.issueStage = "destination";
        this.issue = this.tr(
          "gui.convert.destination_recheck_cancelled",
          "Output recheck cancelled · the conversion plan was not submitted",
        );
        this.current = "";
        this.requestId = null;
        this.requestKind = null;
        this.cancelPending = false;
        this.phase = "reviewing";
        this.bridge.showNotice(this.issue);
        const archiveChanged = this.deferredArchive !== null;
        this.flushDeferredArchive();
        if (!archiveChanged) {
          void tick().then(() => {
            if (this.isCurrent(generation) && typeof document !== "undefined") {
              document.querySelector<HTMLElement>(".create-plan-review")?.focus({ preventScroll: false });
            }
          });
        }
        return;
      }
      this.issueStage = "destination";
      this.issue = error instanceof ConvertDestinationInspectionError && error.detail
        ? this.bridge.tError(error.detail)
        : this.tr("gui.convert.destination_recheck_failed", "Could not recheck the destination. Review it and try again.");
      this.phase = "blocked";
      this.bridge.showNotice(this.issue);
      this.flushDeferredArchive();
      return;
    }
    try {
      if (!this.isCurrent(generation)) return;
      await this.bridge.submitJob(submissionSpec);
      if (!this.isCurrent(generation)) return;
    } catch (error) {
      if (!this.isCurrent(generation)) return;
      this.issueStage = "submit";
      this.issue = this.bridge.isJobSubmitBlocked(error)
        ? this.bridge.jobSubmitBlockedMessage(error)
        : this.tr("gui.convert.submission_requires_desktop_service", "Conversion submission requires the desktop service");
      this.phase = "blocked";
      this.bridge.showNotice(this.issue);
      this.flushDeferredArchive();
      return;
    }
    const restoreFocus = this.bridge.shouldRestorePrimaryFocus();
    this.pending = null;
    this.clearPassword();
    this.validationAttempted = false;
    this.phase = "ready";
    this.bridge.showNotice(
      this.tr("gui.convert.queued_notice", "Conversion added to queue · {size} source")
        .replace("{size}", formatBytes(plan.total_bytes)),
    );
    const volumeDetail = pending.splitSize === null
      ? ""
      : this.tr("gui.convert.history_split_size", " · {size} parts")
          .replace("{size}", formatBytes(pending.splitSize));
    this.bridge.recordQueuedOperation(
      this.tr("gui.convert.queued", "Conversion added to queue"),
      `${pending.sourceTitle} -> ${pathBaseName(plan.primary_output)} · ${createFormats[pending.targetFormat].label} · ${this.profileLabel(pending.profile)}${volumeDetail}`,
    );
    this.flushDeferredArchive();
    if (restoreFocus) {
      await tick();
      if (typeof document !== "undefined") {
        document.querySelector<HTMLElement>(".modern-convert .sheet-action, .classic-convert .classic-primary")?.focus();
      }
    }
  }

  private cancelReview(): void {
    if (!this.pending || this.busy()) return;
    this.resetPreflight(true);
    this.validationAttempted = false;
    this.bridge.showNotice(
      this.tr(
        "gui.convert.review.cancelled",
        "Conversion plan cancelled · the destination password was cleared and no task was added",
      ),
    );
    void tick().then(() => {
      if (typeof document !== "undefined") {
        document.querySelector<HTMLElement>(".modern-convert .sheet-action, .classic-convert .classic-primary")?.focus();
      }
    });
  }

  private cancellable(): boolean {
    return this.requestId !== null
      && (this.requestKind === "plan" || this.requestKind === "destination")
      && (this.phase === "measuring" || this.phase === "choosingDest" || this.phase === "submitting");
  }

  private async cancel(announce = true): Promise<void> {
    const request = this.activeRequest();
    if (!request || this.cancelPending) return;
    this.cancelPending = true;
    try {
      if (request.kind === "plan") await ipc.cancelConvertPlan(request.id);
      else await ipc.cancelCreateDestinationInspection(request.id);
      if (announce && this.requestId === request.id && this.cancelPending) {
        this.bridge.showNotice(this.tr("gui.convert.preflight_cancel_requested", "Stopping conversion checks…"));
      }
    } catch {
      if (this.requestId !== request.id) return;
      this.cancelPending = false;
      if (announce) {
        this.bridge.showNotice(this.tr("gui.convert.preflight_cancel_failed", "Could not stop the current check. It will continue."));
      }
    }
  }

  canLeave(): boolean {
    if (this.phase !== "submitting") return true;
    this.bridge.showNotice(
      this.tr(
        "gui.convert.wait_for_submission_before_leaving",
        "Wait until Squallz finishes adding this conversion to the queue",
      ),
    );
    return false;
  }

  leave(): void {
    const request = this.activeRequest();
    this.resetPreflight(true);
    if (request) this.cancelRequest(request);
  }

  syncArchive(archive: ArchiveInfo | null): void {
    const identity = archive ? `${archive.id}:${archive.source}` : null;
    if (this.phase === "submitting") {
      this.deferredArchive = identity === this.archiveIdentity ? null : { value: archive };
      return;
    }
    this.deferredArchive = null;
    this.applyArchive(archive);
  }

  applyPreflightEvent(event: ConvertPreflightEvent): boolean {
    if (
      !this.requestId
      || event.request_id !== this.requestId
      || this.requestKind !== "destination"
      || event.phase !== "destination"
    ) return false;
    this.current = String(event.current ?? "");
    return true;
  }

  status() {
    return {
      sourceFormat: this.archive()?.format.toUpperCase() ?? "-",
      targetLabel: createFormats[this.targetFormat].label,
      profileLabel: this.profileLabel(),
      methodLabel: this.methodLabel(),
      destination: this.destinationPreview(),
    };
  }

  dispose(): void {
    const request = this.activeRequest();
    this.resetPreflight(true);
    if (request) this.cancelRequest(request);
    this.deferredArchive = null;
    this.activeArchive = null;
    this.archiveIdentity = null;
    sessions.delete(this.owner);
  }

  surface(variant: ConvertWorkspaceVariant): ConvertWorkspaceSurface {
    const requiredReason = this.requiredReason();
    const optionIssue = this.visiblePasswordError() || this.visibleSplitError();
    const lockedReason = this.lockedReason();
    const disabledReason = requiredReason || lockedReason;
    const startLabel = this.pending
      ? this.tr("gui.convert.review_plan_below", "Review plan below")
      : this.busy()
        ? this.tr("gui.convert.checking", "Checking")
        : this.tr("gui.convert.start", "Convert");
    const readinessState = this.pending
      ? this.tr("gui.convert.review_ready", "Review ready")
      : this.busy()
        ? this.tr("gui.convert.checking", "Checking")
        : this.issue
          ? this.tr("gui.state.needs_attention", "Needs attention")
          : this.archive() && !requiredReason && !optionIssue
            ? this.tr("gui.state.ready", "Ready")
            : optionIssue
              ? this.tr("gui.state.needs_attention", "Needs attention")
              : requiredReason || this.openArchiveLabel();
    const review = this.pending && this.plan
      ? {
          plan: this.plan,
          splitSize: this.pending.splitSize,
          issue: this.issue,
          busy: this.phase === "submitting",
          retry: this.issueStage === "submit" || this.issueStage === "destination",
          onConfirm: () => void this.confirm(),
          onCancel: () => this.cancelReview(),
        }
      : null;
    return {
      tr: (key, fallback) => this.tr(key, fallback),
      start: {
        label: startLabel,
        disabled: Boolean(disabledReason),
        title: disabledReason,
        ariaLabel: labelWithReason(startLabel, disabledReason),
        busy: this.busy(),
        onSelect: () => void this.start(),
      },
      source: {
        path: this.archive()?.path ?? this.openArchiveLabel(),
        format: this.archive()?.format.toUpperCase() ?? "-",
        summary: this.sourceSummary(),
      },
      destination: { path: this.destinationPreview() },
      formats: createFormatIds.map((format) => ({
        id: format,
        label: createFormats[format].label,
        selected: this.targetFormat === format,
        disabled: Boolean(lockedReason),
        title: lockedReason || this.formatNote(format),
        ariaLabel: labelWithReason(createFormats[format].label, lockedReason),
        onSelect: () => {
          if (this.busy() || this.pending) return;
          this.targetFormat = format;
          if (this.nativeSplitKind(format) === null) this.splitMode = "generic";
          if (!this.canEncryptData(format)) this.clearPassword();
          else if (!this.canEncryptNames(format)) this.encryptNames = false;
          this.validationAttempted = false;
        },
      })),
      formatNote: this.formatNote(),
      profiles: createProfileIds.map((profile) => ({
        id: profile,
        label: this.profileLabel(profile),
        selected: this.profile === profile,
        disabled: Boolean(lockedReason),
        title: lockedReason,
        ariaLabel: labelWithReason(this.profileLabel(profile), lockedReason),
        onSelect: () => {
          if (this.busy() || this.pending) return;
          this.profile = profile;
          this.customLevelError = "";
        },
      })),
      compression: {
        level: this.compressionLevel(),
        detail: this.profileDetail(),
        method: this.methodLabel(),
        custom: this.profile === "custom"
          ? {
              value: this.customLevel,
              error: this.customLevelError,
              disabled: Boolean(lockedReason),
              title: lockedReason,
              rangeAriaLabel: labelWithReason(
                variant === "classic"
                  ? this.tr("gui.convert.classic_custom_level", "Classic conversion compression level")
                  : this.tr("gui.convert.custom_level", "Conversion compression level"),
                lockedReason,
              ),
              numberAriaLabel: labelWithReason(
                variant === "classic"
                  ? this.tr("gui.convert.classic_custom_level_number", "Classic conversion compression level number")
                  : this.tr("gui.convert.custom_level_number", "Conversion compression level number"),
                lockedReason,
              ),
              onInput: (event) => this.updateCustomLevel(event),
              onChange: (event) => this.updateCustomLevel(event),
            }
          : null,
      },
      advanced: {
        open: this.advancedOpen,
        detail: this.tr("gui.convert.advanced.detail", "Optional destination password and numbered volume size; the source password is requested only when needed."),
        onToggle: (open) => (this.advancedOpen = open),
        onKeydown: (event) => {
          if (event.key !== "Enter" && event.key !== " " && event.key !== "Spacebar") return;
          event.preventDefault();
          this.advancedOpen = !this.advancedOpen;
        },
      },
      protection: {
        variant,
        password: this.password,
        passwordConfirmation: this.passwordConfirmation,
        passwordVisible: this.passwordVisible,
        encryptNames: this.encryptNames,
        canEncryptData: this.canEncryptData(),
        canEncryptNames: this.canEncryptNames(),
        splitDisabled: false,
        splitPreset: this.splitPreset,
        splitMode: this.splitMode,
        nativeSplitKind: this.nativeSplitKind(),
        customSplitAmount: this.customSplitAmount,
        customSplitUnit: this.customSplitUnit,
        passwordCapability: this.tr(`gui.create.format.${this.targetFormat}.password`, createFormats[this.targetFormat].password),
        nameEncryptionCapability: this.nameEncryptionCapability(),
        splitCapability: this.tr(`gui.create.format.${this.targetFormat}.split`, createFormats[this.targetFormat].split),
        splitSummary: this.volumePreview(),
        passwordError: this.visiblePasswordError(),
        splitError: this.visibleSplitError(),
        passwordTitle: this.tr("gui.convert.destination_password", "Destination password"),
        splitTitle: this.tr("gui.convert.output_volumes", "Output volumes"),
        disabled: Boolean(lockedReason),
        disabledReason: lockedReason,
        tr: (key, fallback) => this.tr(key, fallback),
        onPasswordInput: (value) => {
          this.password = value;
          this.validationAttempted = false;
          if (!value) {
            this.passwordConfirmation = "";
            this.encryptNames = false;
          }
        },
        onPasswordConfirmationInput: (value) => {
          this.passwordConfirmation = value;
          this.validationAttempted = false;
        },
        onPasswordVisibleChange: (visible) => (this.passwordVisible = visible),
        onEncryptNamesChange: (enabled) => {
          this.encryptNames = enabled && this.canEncryptNames() && this.password.length > 0;
        },
        onSplitPresetChange: (preset) => {
          this.splitPreset = preset;
          if (preset === "none") this.splitMode = "generic";
          this.validationAttempted = false;
        },
        onSplitModeChange: (mode) => {
          if (mode === "native" && this.nativeSplitKind() === null) {
            this.bridge.showNotice(this.tr("gui.create.native_layout_unavailable", "Native volume layout is available for ZIP and WIM; self-extracting output must remain a single ZIP."));
            return;
          }
          this.splitMode = mode;
          this.validationAttempted = false;
        },
        onCustomSplitAmountInput: (value) => {
          this.customSplitAmount = value;
          this.validationAttempted = false;
        },
        onCustomSplitUnitChange: (unit) => {
          this.customSplitUnit = unit;
          this.validationAttempted = false;
        },
      },
      contract: {
        title: this.tr("gui.convert.contract_title", "Conversion scope"),
        body: this.tr("gui.convert.contract_body", "The task uses the shared archive engine, keeps the source unchanged, and confirms before replacing an existing output."),
      },
      readiness: {
        title: this.tr("gui.convert.readiness", "Readiness"),
        state: readinessState,
        body: this.archive()
          ? this.pending
            ? this.tr("gui.convert.review_ready_body", "Checks are complete. Review the measured source, output layout, and safety bounds before starting.")
            : this.busy()
              ? this.tr("gui.convert.checking_body", "Squallz is reading archive metadata and checking the required filesystems.")
              : this.issue || requiredReason || optionIssue
                ? this.issue || this.tr("gui.convert.fix_options_body", "Correct the highlighted option before starting.")
                : this.tr("gui.convert.ready_body", "Choose the format, profile, protection, and volume settings, then select a destination and review the conversion plan.")
          : this.tr("gui.convert.open_archive_first_body", "Open an archive before converting."),
      },
      guard: {
        title: this.tr("gui.settings.security.guard", "Guard"),
        body: this.tr("gui.convert.guard_body", "The source stays unchanged. Passwords remain in memory only, and replacement consent covers the complete numbered output set."),
      },
      showPreflight: this.phase !== "idle",
      preflight: {
        phase: this.phase,
        requestKind: this.requestKind,
        cancelPending: this.cancelPending,
        current: this.current,
        issue: this.issue,
        issueStage: this.issueStage,
        lockedReason,
        cancellable: this.cancellable(),
        destination: this.destination,
        plan: this.plan,
        workspaceDisk: this.workspaceDisk,
        systemTempDisk: this.systemTempDisk,
        destinationDisk: this.destinationDisk,
        onCancel: () => void this.cancel(),
      },
      review,
    };
  }

  private updateCustomLevel(event: Event): void {
    const input = event.currentTarget as HTMLInputElement;
    const raw = input.value.trim();
    const value = Number(raw);
    if (!raw || !Number.isInteger(value) || value < 1 || value > 9) {
      this.customLevelError = this.tr("gui.create.custom_level_invalid", "Use a compression level from 1 to 9");
      return;
    }
    this.customLevel = clampLevel(value);
    this.customLevelError = "";
    this.profile = "custom";
  }
}

const sessions = new WeakMap<ConvertRouteOwner, ConvertSession>();

export function convertSessionFor(
  owner: ConvertRouteOwner,
  bridge: ConvertRouteBridge,
): ConvertSession {
  const existing = sessions.get(owner);
  if (existing) {
    existing.updateBridge(bridge);
    return existing;
  }
  const session = new ConvertSession(owner, bridge);
  sessions.set(owner, session);
  return session;
}

function nextRequestId(): string {
  return globalThis.crypto?.randomUUID?.()
    ?? `${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}`;
}

function labelWithReason(label: string, reason: string): string {
  return reason ? `${label} · ${reason}` : label;
}
