import { DestinationInspectionError, destinationInspectionCancelled, authorizeArchiveOutput } from "./archive-output-authorization";
import { outputProtectionControls } from "./output-options-presentation";
import { tick } from "svelte";
import type { ConvertWorkspaceSurface } from "../components/ConvertWorkspace.svelte";
import { OutputOptionsDraft, clampCustomLevel, type OutputOptionEdit, type OutputOptionIssue } from "./output-options-draft.svelte";
import { convertTaskTargetFormat, ensureConvertOutputExtension, sourceMatchesConvertTarget, suggestedConvertTargetFormat } from "./convert-format";
import { desktopDirname, sameDesktopPath } from "./desktop-path";
import { basename as pathBaseName, dirname as pathDir, formatBytes } from "./format";
import { isErrorDto, type ArchiveInfo, type JobSpec } from "./ipc";
import { ArchiveOutputWorkflow, type ArchiveOutputPreflightOutcome, type ArchiveOutputStage } from "./archive-output-workflow.svelte";
import type {
  ConvertPreflightEvent,
  ConvertRouteBridge,
  ConvertRouteHandle,
  ConvertRouteOwner,
  ConvertTaskDraft,
  ConvertWorkspaceVariant,
} from "./convert-route";
import {
  createFormatIds,
  createFormats,
  createProfileIds,
  createProfiles,
  type CreateFormatId,
  type CreateProfileId,
} from "./ui-model";

type ConvertJobSpec = Extract<JobSpec, { kind: "convert" }>;
type PendingConvertSubmission = Readonly<{
  spec: ConvertJobSpec;
  targetFormat: CreateFormatId;
  profile: CreateProfileId;
  sourceTitle: string;
  splitSize: number | null;
}>;

function loadProfile(): CreateProfileId {
  try {
    const value = window.localStorage.getItem("squallz.createProfile");
    return createProfileIds.includes(value as CreateProfileId) ? value as CreateProfileId : "balanced";
  } catch {
    return "balanced";
  }
}

function loadCustomLevel(): number {
  try {
    const value = window.localStorage.getItem("squallz.customCreateLevel");
    return value === null ? 6 : clampCustomLevel(Number(value));
  } catch {
    return 6;
  }
}

export class ConvertSession implements ConvertRouteHandle {
  private archiveIdentity: string | null = null;
  private activeArchive = $state<ArchiveInfo | null>(null);
  private deferredArchive: { value: ArchiveInfo | null } | null = null;
  private output = new OutputOptionsDraft<object>({ format: "zip", profile: loadProfile(), customLevel: loadCustomLevel() });
  private get draft() { return this.output.state; }
  private suggestedDestination = $state<string | null>(null);
  private advancedOpen = $state(false);
  private readonly workflow = new ArchiveOutputWorkflow<PendingConvertSubmission>({
    kind: "convert", ensureListener: () => this.bridge.ensurePreflightListener(), nextRequestId,
    getDialogModule: () => this.bridge.getDialogModule(), tr: (key) => this.tr(key),
    notice: (message) => this.bridge.showNotice(message), tError: (error) => this.bridge.tError(error),
  });

  private get data() { return this.workflow.state; }

  constructor(
    private readonly owner: ConvertRouteOwner,
    private bridge: ConvertRouteBridge,
  ) {}

  updateBridge(bridge: ConvertRouteBridge): void {
    this.bridge = bridge;
  }

  private tr(key: string, fallback?: string): string {
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
    this.resetPreflight(true);
    this.draft.format = suggestedConvertTargetFormat(archive?.format);
    this.draft.customLevelInvalid = false;
    this.resetOutputOptions();
  }

  private flushDeferredArchive(): void {
    if (this.data.phase === "submitting") return;
    const deferred = this.deferredArchive;
    if (!deferred) return;
    this.deferredArchive = null;
    this.applyArchive(deferred.value);
  }

  private openArchiveLabel(): string {
    return this.tr("gui.empty.open_archive_first");
  }

  private profileLabel(profile: CreateProfileId = this.draft.profile): string {
    return this.tr(`gui.create.profile.${profile}`, createProfiles[profile].label);
  }

  private profileDetail(profile: CreateProfileId = this.draft.profile): string {
    if (profile === "custom") {
      return this.tr("gui.convert.custom_level_detail");
    }
    return this.tr(`gui.create.profile.${profile}.detail`, createProfiles[profile].detail);
  }

  private formatMethod(format: CreateFormatId = this.draft.format): string {
    return this.tr(`gui.create.format.${format}.method`, createFormats[format].method);
  }

  private methodLabel(): string {
    return this.tr("gui.convert.method_level")
      .replace("{method}", this.formatMethod())
      .replace("{level}", String(this.output.compressionLevel));
  }

  private canEncryptData(format: CreateFormatId = this.draft.format): boolean {
    return createFormats[format].can_encrypt_data;
  }

  private canEncryptNames(format: CreateFormatId = this.draft.format): boolean {
    return createFormats[format].can_encrypt_names;
  }

  private defaultDestination(format: CreateFormatId = this.draft.format): string {
    if (this.suggestedDestination) {
      const extension = this.output.outputExtension;
      const original = this.suggestedDestination;
      const suffix = [...createFormatIds.flatMap((id) => createFormats[id].extensions), "swm"]
        .sort((a, b) => b.length - a.length)
        .find((value) => original.toLowerCase().endsWith(`.${value}`));
      if (suffix && (extension === "swm" ? suffix === "swm" : createFormats[format].extensions.includes(suffix))) return original;
      return suffix ? `${original.slice(0, -suffix.length)}${extension}` : original;
    }
    const archive = this.archive();
    if (!archive) return this.openArchiveLabel();
    const base = this.bridge.archiveStemName(archive.name);
    const outputBase = sourceMatchesConvertTarget(archive.format, format)
      ? `${base}.converted`
      : base;
    return `${pathDir(archive.path)}/${outputBase}.${this.output.outputExtension}`;
  }

  private destinationPreview(): string {
    if (this.data.plan) return this.data.plan.primary_output;
    if (this.data.destination) return this.data.destination;
    const destination = this.defaultDestination();
    if (this.output.splitSize === null) return destination;
    if (this.draft.splitMode === "native") {
      return this.draft.format === "wim"
        ? this.tr(
            "gui.convert.native_split_wim_destination_preview",
          ).replace("{destination}", destination)
        : this.tr(
            "gui.convert.native_split_destination_preview",
          ).replace("{destination}", destination);
    }
    return this.tr(
      "gui.convert.split_destination_preview",
    ).replaceAll("{destination}", destination);
  }

  private volumePreview(): string {
    const splitSize = this.output.splitSize;
    if (splitSize === null) {
      return this.tr("gui.convert.single_archive_summary", "Single converted archive · no numbered parts");
    }
    const nativeWim = this.draft.splitMode === "native" && this.draft.format === "wim";
    const key = nativeWim
      ? "gui.convert.native_split_wim_summary"
      : this.draft.splitMode === "native"
        ? "gui.convert.native_split_summary"
        : "gui.convert.split_summary";
    const fallback = nativeWim
      ? "{size} target per part · native .swm set; one large file may exceed the target"
      : this.draft.splitMode === "native"
        ? "{size} per part · native ZIP set ending in .zip"
        : "{size} per part · the exact file list appears when conversion finishes";
    return this.tr(key, fallback).replace("{size}", formatBytes(splitSize));
  }

  private sourceSummary(): string {
    const archive = this.archive();
    if (!archive) return this.openArchiveLabel();
    const diagnostics = archive.garbled_count
      ? this.tr("gui.archive.names_review")
          .replace("{count}", archive.garbled_count.toLocaleString())
      : archive.non_utf8_name_count
        ? this.tr("gui.archive.non_utf8_names")
            .replace("{count}", archive.non_utf8_name_count.toLocaleString())
        : this.tr("gui.archive.names_clean");
    return this.tr("gui.convert.source_summary")
      .replace("{count}", archive.entry_count.toLocaleString())
      .replace("{diagnostics}", diagnostics);
  }

  private formatNote(format: CreateFormatId = this.draft.format): string {
    const archive = this.archive();
    if (archive && sourceMatchesConvertTarget(archive.format, format)) {
      return this.tr("gui.convert.same_format_note");
    }
    return this.tr(`gui.create.format.${format}.note`, createFormats[format].note);
  }

  private requiredReason(): string {
    if (!this.archive()) {
      return this.tr("gui.precondition.open_before_convert");
    }
    return this.draft.profile === "custom" ? this.levelError() : "";
  }

  private busy(): boolean {
    return this.workflow.busy();
  }

  private lockedReason(): string {
    if (this.busy()) {
      return this.tr("gui.convert.options_locked");
    }
    return this.data.pending
      ? this.tr("gui.convert.options_locked_review")
      : "";
  }

  private optionIssue(issue: OutputOptionIssue | null): string {
    if (!issue) return "";
    const messages: Record<OutputOptionIssue, [string, string]> = {
      password_required: ["gui.convert.password_required", "Enter a new destination password, or turn off file content encryption."],
      confirm_password_required: ["gui.convert.confirm_password_required", "Confirm the destination password before starting"],
      passwords_do_not_match: ["gui.convert.passwords_do_not_match", "The destination passwords do not match"],
      invalid_part_size: ["gui.convert.invalid_part_size", "Enter a part size of at least 0.1 MiB"],
      native_zip_part_size_limit: ["gui.create.native_zip_part_size_limit", "Native ZIP parts cannot exceed 4 GiB − 1 byte"],
      custom_level_invalid: ["gui.create.custom_level_invalid", "Use a compression level from 1 to 9"],
      encryption_format_change: ["gui.create.encryption_format_change", "Turn off file content encryption before choosing a format that cannot encrypt files."],
      name_encryption_format_change: ["gui.create.name_encryption_format_change", "Turn off file name encryption before choosing a format that cannot hide names."],
      native_layout_unavailable: ["gui.create.native_layout_unavailable", "Native volume layout is available for ZIP and WIM; self-extracting output must remain a single ZIP."],
    };
    return this.tr(...messages[issue]);
  }

  private levelError(): string {
    return this.draft.customLevelInvalid ? this.optionIssue("custom_level_invalid") : "";
  }

  private editOutput(edit: OutputOptionEdit): void {
    const issue = edit.kind === "format" ? this.output.formatIssue(edit.value)
      : edit.kind === "splitMode" && edit.value === "native" && this.output.nativeSplitKind === null
        ? "native_layout_unavailable" : null;
    if (issue) { this.bridge.showNotice(this.optionIssue(issue)); return; }
    if (edit.kind === "customLevel" && !this.output.validateLevel(edit.value)) return;
    this.output.edit(edit);
    if (edit.kind === "profile") this.draft.customLevelInvalid = false;
  }

  private resetOutputOptions(): void {
    this.output.resetProtection();
    this.suggestedDestination = null;
    this.advancedOpen = false;
  }

  private resetPreflight(clearPassword: boolean): void {
    this.workflow.reset();
    if (clearPassword) this.output.clearPassword();
  }

  private finishWithIssue(stage: ArchiveOutputStage, message: string, phase: "blocked" | "cancelled" = "blocked"): void {
    this.workflow.issue(stage, message, phase);
  }

  private validate(): boolean {
    this.draft.validationAttempted = true;
    const error = this.optionIssue(this.output.passwordIssue) || this.optionIssue(this.output.splitIssue);
    if (!error) return true;
    this.advancedOpen = true;
    this.bridge.showNotice(error);
    void tick().then(() => {
      if (typeof document === "undefined" || !this.draft.validationAttempted || this.busy() || this.data.pending) return;
      document.querySelector<HTMLInputElement>(".modern-convert .create-option-input[aria-invalid='true'], .classic-convert .create-option-input[aria-invalid='true']")?.focus();
    });
    return false;
  }

  private preflightError(outcome: Extract<ArchiveOutputPreflightOutcome, { status: "error" }>): string {
    switch (outcome.code) {
      case "empty":
      case "plan":
        return isErrorDto(outcome.error)
          ? this.bridge.tError(outcome.error)
          : this.tr("gui.convert.plan_failed");
      case "workspace_space":
        return this.tr("gui.convert.not_enough_workspace_space")
          .replace("{available}", formatBytes(outcome.availableBytes ?? 0));
      case "system_temp_space":
        return this.tr("gui.convert.not_enough_system_temp_space")
          .replace("{available}", formatBytes(outcome.availableBytes ?? 0));
      case "destination_space":
        return this.tr("gui.convert.not_enough_destination_space")
          .replace("{available}", formatBytes(outcome.availableBytes ?? 0));
      case "destination_service":
        return this.tr("gui.convert.destination_check_requires_desktop_service");
      case "workspace_service":
        return this.tr("gui.convert.workspace_check_requires_desktop_service");
    }
  }

  private async start(): Promise<void> {
    const archive = this.archive();
    if (!archive) {
      this.bridge.showNotice(this.tr("gui.precondition.open_before_convert"));
      return;
    }
    if (this.draft.profile === "custom" && this.levelError()) {
      this.bridge.showNotice(this.levelError());
      return;
    }
    if (!this.validate()) return;
    if (this.busy() || this.data.pending || this.bridge.focusBlockingTaskIfAny()) return;
    const targetFormat = this.draft.format;
    const profile = this.draft.profile;
    const defaultDest = this.defaultDestination(targetFormat);
    const level = this.output.compressionLevel;
    const splitSize = this.output.splitSize;
    const destPassword = this.canEncryptData(targetFormat) && this.draft.password.length > 0
      ? this.draft.password
      : null;
    const encryptNames = Boolean(destPassword) && this.draft.encryptNames && this.canEncryptNames(targetFormat);
    const sourceTitle = archive.name;
    const isCurrent = this.workflow.begin();
    try {
      const { confirm, save } = await this.bridge.getDialogModule();
      if (!isCurrent()) return;
      const selected = await this.bridge.saveNativeDialog("convert.save-archive", save, {
        title: this.tr("gui.convert.save_as"),
        defaultPath: defaultDest,
        filters: [{
          name: this.tr(`gui.create.format.${targetFormat}.filter`, createFormats[targetFormat].filterName),
          extensions: this.output.outputExtension === "swm"
            ? ["swm"]
            : createFormats[targetFormat].extensions,
        }],
      });
      if (!isCurrent()) return;
      if (!selected) {
        this.output.clearPassword();
        this.finishWithIssue(
          "destination",
          this.tr("gui.convert.destination_selection_cancelled"),
          "cancelled",
        );
        return;
      }
      const dest = ensureConvertOutputExtension(
        selected,
        targetFormat,
        this.output.outputExtension === "swm" ? "swm" : undefined,
      );
      this.data.destination = dest;
      if (sameDesktopPath(dest, archive.path, this.bridge.platform())) {
        this.finishWithIssue("destination", this.tr("gui.convert.same_path"));
        return;
      }
      const authorization = await authorizeArchiveOutput({
        path: dest, inspect: () => this.workflow.inspect(dest, splitSize !== null, null, isCurrent),
        isCurrent, confirm: async () => confirm, tr: (key) => this.tr(key),
        message: splitSize === null ? "gui.output.replace_existing" : "gui.output.replace_existing_split",
      });
      if (!isCurrent()) return;
      if (!authorization) {
        this.output.clearPassword();
        this.finishWithIssue(
          "destination",
          this.tr("gui.convert.replacement_cancelled"),
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
        split_mode: splitSize === null ? "generic" : this.draft.splitMode,
        replace_existing: authorization.replaceExisting,
        replacement_guard: authorization.replacementGuard,
      };
      const outcome = await this.workflow.prepare(spec, desktopDirname(dest, this.bridge.platform()), isCurrent);
      if (!isCurrent() || outcome.status === "stale") return;
      if (outcome.status === "cancelled") {
        this.output.clearPassword();
        this.finishWithIssue(
          "source",
          this.tr("gui.convert.preflight_cancelled_notice"),
          "cancelled",
        );
        return;
      }
      if (outcome.status === "error") {
        this.finishWithIssue(outcome.stage, this.preflightError(outcome));
        return;
      }
      this.workflow.review({ spec, targetFormat, profile, sourceTitle, splitSize });
      this.bridge.showNotice(this.tr("gui.convert.review_ready_notice"));
      void tick().then(() => {
        if (isCurrent() && typeof document !== "undefined") {
          document.querySelector<HTMLElement>(".create-plan-review")?.focus({ preventScroll: false });
        }
      });
    } catch (error) {
      if (!isCurrent()) return;
      if (destinationInspectionCancelled(error)) {
        this.output.clearPassword();
        this.finishWithIssue(
          "destination",
          this.tr("gui.convert.preflight_cancelled_notice"),
          "cancelled",
        );
      } else if (error instanceof DestinationInspectionError) {
        this.finishWithIssue(
          "destination",
          error.detail
            ? this.bridge.tError(error.detail)
            : this.tr("gui.output.inspect_failed"),
        );
      } else {
        this.finishWithIssue(
          "destination",
          this.tr("gui.convert.requires_desktop_service"),
        );
      }
    }
  }

  private async confirm(): Promise<void> {
    if (this.busy()) return;
    if (!this.data.pending || !this.data.plan) {
      this.bridge.showNotice(this.tr("gui.convert.review.expired"));
      this.resetPreflight(true);
      return;
    }
    if (this.bridge.focusBlockingTaskIfAny()) return;
    await this.workflow.confirm({
      recheck: true,
      prepare: () => this.bridge.prepareSubmitFocus(),
      submit: (spec) => this.bridge.submitJob(spec),
      blockedMessage: (error) => this.bridge.isJobSubmitBlocked(error) ? this.bridge.jobSubmitBlockedMessage(error) : null,
      focusReview: async (isCurrent) => {
        this.flushDeferredArchive();
        await tick();
        if (isCurrent() && typeof document !== "undefined") {
          document.querySelector<HTMLElement>(".create-plan-review")?.focus({ preventScroll: false });
        }
      },
      queued: async (pending, plan) => {
        const restoreFocus = this.bridge.shouldRestorePrimaryFocus();
        this.output.clearPassword();
        this.draft.validationAttempted = false;
        this.bridge.showNotice(this.tr("gui.convert.queued_notice").replace("{size}", formatBytes(plan.total_bytes)));
        const volumeDetail = pending.splitSize === null ? "" : this.tr("gui.convert.history_split_size")
          .replace("{size}", formatBytes(pending.splitSize));
        this.bridge.recordQueuedOperation(this.tr("gui.convert.queued"),
          `${pending.sourceTitle} -> ${pathBaseName(plan.primary_output)} · ${createFormats[pending.targetFormat].label} · ${this.profileLabel(pending.profile)}${volumeDetail}`);
        this.flushDeferredArchive();
        if (restoreFocus) {
          const isCurrent = this.workflow.current();
          await tick();
          if (isCurrent() && typeof document !== "undefined") {
            document.querySelector<HTMLElement>(".modern-convert .sheet-action, .classic-convert .classic-primary")?.focus();
          }
        }
      },
    });
    this.flushDeferredArchive();
  }

  private cancelReview(): void {
    if (!this.data.pending || this.busy()) return;
    this.resetPreflight(true);
    this.draft.validationAttempted = false;
    this.bridge.showNotice(
      this.tr(
        "gui.convert.review.cancelled",
      ),
    );
    const isCurrent = this.workflow.current();
    void tick().then(() => {
      if (isCurrent() && typeof document !== "undefined") {
        document.querySelector<HTMLElement>(".modern-convert .sheet-action, .classic-convert .classic-primary")?.focus();
      }
    });
  }

  private cancellable(): boolean {
    return this.data.requestId !== null
      && (this.data.requestKind === "plan" || this.data.requestKind === "destination")
      && (this.data.phase === "measuring" || this.data.phase === "choosingDest" || this.data.phase === "submitting");
  }

  canLeave(): boolean {
    if (this.data.phase !== "submitting") return true;
    this.bridge.showNotice(
      this.tr(
        "gui.convert.wait_for_submission_before_leaving",
      ),
    );
    return false;
  }

  canReviewTask(draft: ConvertTaskDraft): boolean {
    const locked = this.lockedReason();
    if (locked) {
      this.bridge.showNotice(locked);
      return false;
    }
    if (!convertTaskTargetFormat(draft.dest)) {
      this.bridge.showNotice(this.tr("gui.convert.review.format_unavailable"));
      return false;
    }
    return true;
  }

  restoreTaskDraft(draft: ConvertTaskDraft): boolean {
    if (!this.canReviewTask(draft)) return false;
    const archive = this.bridge.getArchive();
    if (!archive || !sameDesktopPath(archive.source, draft.src, this.bridge.platform())
      || (archive.encoding_override ?? null) !== draft.src_encoding) return false;
    const format = convertTaskTargetFormat(draft.dest);
    if (!format) return false;
    this.syncArchive(archive);
    this.resetPreflight(true);
    this.resetOutputOptions();
    this.draft.format = format;
    this.draft.profile = "custom";
    this.draft.customLevel = draft.level;
    this.draft.customLevelInvalid = false;
    this.suggestedDestination = draft.dest;
    this.draft.splitMode = draft.split_mode;
    this.output.restoreSplitSize(draft.split_size === null ? null : String(draft.split_size), false);
    this.draft.encryptNames = draft.encrypt_names;
    this.draft.encryptionEnabled = draft.outputPasswordRequired || draft.encrypt_names;
    this.draft.validationAttempted = this.draft.encryptionEnabled;
    this.advancedOpen = true;
    return true;
  }

  leave(): void {
    this.resetPreflight(true);
  }

  syncArchive(archive: ArchiveInfo | null): void {
    const identity = archive ? `${archive.id}:${archive.source}` : null;
    if (this.data.phase === "submitting") {
      this.deferredArchive = identity === this.archiveIdentity ? null : { value: archive };
      return;
    }
    this.deferredArchive = null;
    this.applyArchive(archive);
  }

  applyPreflightEvent(event: ConvertPreflightEvent): boolean {
    if (
      !this.data.requestId
      || event.request_id !== this.data.requestId
      || this.data.requestKind !== "destination"
      || event.phase !== "destination"
    ) return false;
    this.data.current = String(event.current ?? "");
    return true;
  }

  status() {
    return {
      sourceFormat: this.archive()?.format.toUpperCase() ?? "-",
      targetLabel: createFormats[this.draft.format].label,
      profileLabel: this.profileLabel(),
      methodLabel: this.methodLabel(),
      destination: this.destinationPreview(),
    };
  }

  dispose(): void {
    this.workflow.dispose();
    this.output.clearPassword();
    this.deferredArchive = null;
    this.activeArchive = null;
    this.archiveIdentity = null;
    sessions.delete(this.owner);
  }

  surface(variant: ConvertWorkspaceVariant): ConvertWorkspaceSurface {
    const requiredReason = this.requiredReason();
    const protection = outputProtectionControls(this.draft, (edit) => this.editOutput(edit),
      (key, fallback) => this.tr(key, fallback),
      this.optionIssue(this.output.passwordIssue), this.optionIssue(this.output.splitIssue));
    const optionIssue = protection.passwordError || protection.splitError;
    const lockedReason = this.lockedReason();
    const disabledReason = requiredReason || lockedReason;
    const startLabel = this.data.pending
      ? this.tr("gui.convert.review_plan_below")
      : this.busy()
        ? this.tr("gui.convert.checking")
        : this.tr("gui.convert.start");
    const readinessState = this.data.pending
      ? this.tr("gui.convert.review_ready")
      : this.busy()
        ? this.tr("gui.convert.checking")
        : this.data.issue
          ? this.tr("gui.state.needs_attention")
          : this.archive() && !requiredReason && !optionIssue
            ? this.tr("gui.state.ready")
            : optionIssue
              ? this.tr("gui.state.needs_attention")
              : requiredReason || this.openArchiveLabel();
    const review = this.data.pending && this.data.plan
      ? {
          plan: this.data.plan,
          splitSize: this.data.pending.splitSize,
          issue: this.data.issue,
          busy: this.data.phase === "submitting",
          retry: this.data.issueStage === "submit" || this.data.issueStage === "destination",
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
        selected: this.draft.format === format,
        disabled: Boolean(lockedReason),
        title: lockedReason || this.formatNote(format),
        ariaLabel: labelWithReason(createFormats[format].label, lockedReason),
        onSelect: () => {
          if (this.busy() || this.data.pending) return;
          this.editOutput({ kind: "format", value: format });
        },
      })),
      formatNote: this.formatNote(),
      profiles: createProfileIds.map((profile) => ({
        id: profile,
        label: this.profileLabel(profile),
        selected: this.draft.profile === profile,
        disabled: Boolean(lockedReason),
        title: lockedReason,
        ariaLabel: labelWithReason(this.profileLabel(profile), lockedReason),
        onSelect: () => {
          if (this.busy() || this.data.pending) return;
          this.editOutput({ kind: "profile", value: profile });
        },
      })),
      compression: {
        level: this.output.compressionLevel,
        detail: this.profileDetail(),
        method: this.methodLabel(),
        custom: this.draft.profile === "custom"
          ? {
              value: this.draft.customLevel,
              error: this.levelError(),
              disabled: Boolean(lockedReason),
              title: lockedReason,
              rangeAriaLabel: labelWithReason(
                variant === "classic"
                  ? this.tr("gui.convert.classic_custom_level")
                  : this.tr("gui.convert.custom_level", "Conversion compression level"),
                lockedReason,
              ),
              numberAriaLabel: labelWithReason(
                variant === "classic"
                  ? this.tr("gui.convert.classic_custom_level_number")
                  : this.tr("gui.convert.custom_level_number"),
                lockedReason,
              ),
              onInput: (event) => this.editOutput({ kind: "customLevel", value: (event.currentTarget as HTMLInputElement).value }),
              onChange: (event) => this.editOutput({ kind: "customLevel", value: (event.currentTarget as HTMLInputElement).value }),
            }
          : null,
      },
      advanced: {
        open: this.advancedOpen,
        detail: this.tr("gui.convert.advanced.detail"),
        onToggle: (open) => (this.advancedOpen = open),
        onKeydown: (event) => {
          if (event.key !== "Enter" && event.key !== " " && event.key !== "Spacebar") return;
          event.preventDefault();
          this.advancedOpen = !this.advancedOpen;
        },
      },
      protection: {
        variant,
        ...protection,
        splitDisabled: false,
        nativeSplitKind: this.output.nativeSplitKind,
        splitCapability: this.tr(`gui.create.format.${this.draft.format}.split`, createFormats[this.draft.format].split),
        splitSummary: this.volumePreview(),
        passwordTitle: this.tr("gui.convert.destination_password"),
        splitTitle: this.tr("gui.convert.output_volumes"),
        disabled: Boolean(lockedReason),
        disabledReason: lockedReason,
        tr: (key, fallback) => this.tr(key, fallback),
      },
      contract: {
        title: this.tr("gui.convert.contract_title"),
        body: this.tr("gui.convert.contract_body"),
      },
      readiness: {
        title: this.tr("gui.convert.readiness"),
        state: readinessState,
        body: this.archive()
          ? this.data.pending
            ? this.tr("gui.convert.review_ready_body")
            : this.busy()
              ? this.tr("gui.convert.checking_body")
              : this.data.issue || requiredReason || optionIssue
                ? this.data.issue || this.tr("gui.convert.fix_options_body")
                : this.tr("gui.convert.ready_body")
          : this.tr("gui.convert.open_archive_first_body"),
      },
      guard: {
        title: this.tr("gui.settings.security.guard"),
        body: this.tr("gui.convert.guard_body"),
      },
      showPreflight: this.data.phase !== "idle",
      preflight: {
        phase: this.data.phase,
        requestKind: this.data.requestKind,
        cancelPending: this.data.cancelPending,
        current: this.data.current,
        issue: this.data.issue,
        issueStage: this.data.issueStage,
        lockedReason,
        cancellable: this.cancellable(),
        destination: this.data.destination,
        plan: this.data.plan,
        workspaceDisk: this.data.workspaceDisk,
        systemTempDisk: this.data.systemTempDisk,
        destinationDisk: this.data.destinationDisk,
        onCancel: () => void this.workflow.cancel(),
      },
      review,
    };
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
