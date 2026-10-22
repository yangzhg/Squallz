import { outputProtectionControls } from "./output-options-presentation";
import { tFallback as tr } from "./i18n.svelte";
import {
  createFormats, createFormatIds, createProfiles, createProfileIds,
  type CreateFormatId, type CreateProfileId,
} from "./ui-model";
import { desktopBasename } from "./desktop-path";
import { formatBytes } from "./format";
import { createContentPolicyLabel, createArchivePresetSummary, presetSqzInnerFormatLabel } from "./archive-option-labels";
import { archivePresetDisplayName } from "./archive-presets.svelte";
import type { CreateDraftIssue } from "./create-options-draft.svelte";
import { commonCreateSourceParent, type CreatePreflightStage } from "./create-preflight.svelte";
import type { CreateSourceKind } from "./create-sources";
import type { CreateWorkspaceSurface, CreateWorkspaceVariant } from "../components/CreateWorkspace.svelte";
import type { CreateSession } from "./create-session.svelte";
type CreatePreflightStepState = "pending" | "active" | "ready" | "blocked" | "cancelled";
const presetSqzInnerFormats = ["sqz", "zip", "tar", "7z", "zstd"] as const;
const archiveName = (base: string, extension: string) => `${base}.${extension}`;
const labelWithDisabledReason = (label: string, reason: string) => reason ? `${label} · ${reason}` : label;

/** Derived labels and component props; all mutations stay in the shared session. */
export class CreatePresentation {
  constructor(readonly session: CreateSession) {}
  issue = (issue: CreateDraftIssue, requestedTarget?: string): string => {
    switch (issue) {
      case "options_locked": return this.optionsLockedReason();
      case "format_unavailable": return tr("gui.presets.format_unavailable");
      case "task_format_unavailable": return tr("gui.create.review.format_unavailable");
      case "task_sfx_unavailable": return tr("gui.create.review.sfx_unavailable");
      case "sfx_loading": return tr("gui.create.sfx_capability_loading");
      case "sfx_unavailable": return this.sfxUnavailableMessage();
      case "sfx_target_unavailable": return tr("gui.presets.sfx_target_unavailable").replace("{target}", requestedTarget ?? "");
      case "sfx_zip_only": return tr("gui.create.sfx_zip_only_notice");
      case "encryption_format_change": return tr("gui.create.encryption_format_change");
      case "name_encryption_format_change": return tr("gui.create.name_encryption_format_change");
      case "sfx_no_split": return tr("gui.create.sfx_no_split_notice");
      case "native_layout_unavailable": return tr("gui.create.native_layout_unavailable");
      case "trash_excluded": return tr("gui.create.output.source.trash_disabled_excludes").replace("{trash}", this.session.port.trashName());
      case "completion_sfx": return tr("gui.create.output.completion.open_disabled_sfx");
      case "completion_split": return tr("gui.create.output.completion.open_disabled_split");
      case "trash_test_required": return tr("gui.create.output.integrity.required_notice").replace("{trash}", this.session.port.trashName());
      case "custom_level_invalid": return tr("gui.create.custom_level_invalid");
      case "password_required": return tr("gui.create.password_required");
      case "confirm_password_required": return tr("gui.create.confirm_password_required");
      case "passwords_do_not_match": return tr("gui.create.passwords_do_not_match");
      case "invalid_part_size": return tr("gui.create.invalid_part_size");
      case "native_zip_part_size_limit": return tr("gui.create.native_zip_part_size_limit");
      case "sfx_requires_zip": return tr("gui.create.sfx_requires_zip");
    }
  };

  profileData = (profileId: CreateProfileId) => {
    if (profileId === "custom") {
      return {
        label: tr("gui.create.profile.custom"),
        level: this.session.options.state.customLevel,
        detail: tr("gui.create.profile.custom.detail", "Choose an exact compression level"),
      };
    }
    return createProfiles[profileId];
  };

  trashSourceDisabledReason = (): string => {
    return this.session.options.trashExcluded ? this.issue("trash_excluded") : "";
  };

  openCompletionDisabledReason = (): string => {
    const restriction = this.session.options.completionRestriction;
    return restriction ? this.issue(restriction === "sfx" ? "completion_sfx" : "completion_split") : "";
  };

  sfxOutputLabel = (): string => {
    if (this.session.capability.target === "macos") return tr("gui.create.sfx_macos_output");
    if (this.session.capability.target === "windows") return tr("gui.create.sfx_windows_output");
    return tr("gui.create.sfx_linux_output");
  };

  sfxTargetLabel = (): string => {
    if (this.session.capability.target === "macos") return tr("gui.create.sfx_target_macos");
    if (this.session.capability.target === "windows") return tr("gui.create.sfx_target_windows");
    return tr("gui.create.sfx_target_linux");
  };

  sfxSummary = (): string => {
    return this.session.options.state.sfxEnabled
      ? tr("gui.create.sfx_summary_enabled")
      : tr("gui.create.sfx_summary_disabled");
  };

  sfxSigningWarning = (): string => {
    if (this.session.capability.target === "macos") {
      return tr("gui.create.sfx_signing_macos");
    }
    if (this.session.capability.target === "windows") {
      return tr("gui.create.sfx_signing_windows");
    }
    return tr("gui.create.sfx_signing_linux");
  };

  sfxUnavailableMessage = (): string => {
    if (this.session.capability.status === "invalid") {
      return tr(
        "gui.create.sfx_template_invalid",
      );
    }
    return tr("gui.create.sfx_template_unavailable");
  };

  archivePreviewName = (base = this.session.outputBase()): string => {
    const extension = this.session.options.outputExtension(this.session.capability.extension);
    const suggested = this.session.options.state.destinationBase === "ask" ? this.session.suggestedOutputPath(this.session.options.state.format, extension) : null;
    return suggested ? desktopBasename(suggested, this.session.port.platform()) : archiveName(base, extension);
  };

  outputPreview = (base = this.session.outputBase()): string => {
    const name = this.archivePreviewName(base);
    if (this.session.options.state.destinationBase === "ask") {
      const suggested = this.session.suggestedOutputPath(this.session.options.state.format, this.session.options.outputExtension(this.session.capability.extension));
      if (suggested) {
        return tr("gui.create.output.preview_confirm").replace("{path}", suggested);
      }
      return tr("gui.create.output.preview_ask").replace("{name}", name);
    }
    if (this.session.options.state.destinationBase === "default_directory") {
      const folder = this.session.port.read().defaultDirectory;
      return folder
        ? this.session.joinFolder(folder, name)
        : tr("gui.create.output.preview_default_missing", "Default create folder is not set · Squallz will ask");
    }
    const droppedParent = commonCreateSourceParent(this.session.inputs, this.session.port.platform());
    return droppedParent
      ? this.session.joinFolder(droppedParent, name)
      : tr("gui.create.output.preview_source_parent").replace("{name}", name);
  };

  formatDisabledReason = (formatId: CreateFormatId): string => {
    const lockedReason = this.optionsLockedReason();
    if (lockedReason) return lockedReason;
    return this.session.options.state.sfxEnabled && formatId !== "zip"
      ? tr("gui.create.sfx_zip_only")
      : "";
  };

  methodLabel = (): string => {
    if (this.session.options.state.format === "wim") return this.formatMethod();
    if (this.session.options.state.format === "sqz") {
      return tr("gui.create.method_profile")
        .replace("{method}", this.formatMethod())
        .replace("{profile}", this.profileLabel(this.session.options.state.profile));
    }
    return tr("gui.create.method_level_profile")
      .replace("{method}", this.formatMethod())
      .replace("{level}", String(this.session.options.compressionLevel))
      .replace("{profile}", this.profileLabel(this.session.options.state.profile));
  };

  splitCapability = (): string => {
    return this.session.options.state.sfxEnabled
      ? tr("gui.create.sfx_no_split_capability")
      : this.formatSplit();
  };

  profileLabel = (profileId: CreateProfileId): string => {
    return tr(`gui.create.profile.${profileId}`, createProfiles[profileId].label);
  };

  profileDetail = (profileId: CreateProfileId): string => {
    return tr(`gui.create.profile.${profileId}.detail`, createProfiles[profileId].detail);
  };

  formatMethod = (formatId: CreateFormatId = this.session.options.state.format): string => {
    return tr(`gui.create.format.${formatId}.method`, createFormats[formatId].method);
  };

  formatSplit = (formatId: CreateFormatId = this.session.options.state.format): string => {
    return tr(`gui.create.format.${formatId}.split`, createFormats[formatId].split);
  };

  formatRecovery = (formatId: CreateFormatId = this.session.options.state.format): string => {
    return tr(`gui.create.format.${formatId}.recovery`, createFormats[formatId].recovery);
  };

  formatNoteFor = (formatId: CreateFormatId = this.session.options.state.format): string => {
    return tr(`gui.create.format.${formatId}.note`, createFormats[formatId].note);
  };

  sourceKindLabel = (kind: CreateSourceKind): string => {
    if (kind === "file") return tr("gui.create.sources.kind_file");
    if (kind === "folder") return tr("gui.create.sources.kind_folder");
    return tr("gui.create.sources.kind_item");
  };

  sourceCountLabel = (): string => {
    return tr("gui.create.sources.count")
      .replace("{count}", this.session.sources.length.toLocaleString());
  };

  sourceSelectionLabel = (): string => {
    const count = this.session.selectedCount();
    return count === 0
      ? tr("gui.create.sources.none_selected")
      : tr("gui.create.sources.selected_count")
        .replace("{count}", count.toLocaleString());
  };

  sourcesLockedReason = (): string => {
    if (this.session.preflight.state.picker) {
      return tr("gui.create.sources.picker_open");
    }
    if (this.session.preflight.busy()) {
      return tr("gui.create.sources.locked_preflight");
    }
    if (this.session.preflight.state.pending) {
      return tr("gui.create.sources.locked_review");
    }
    return "";
  };

  startDisabled = (): boolean => {
    return this.session.sources.length === 0
      || this.session.preflight.sourcesLocked()
      || this.session.configurationPending;
  };

  startLabel = (readyLabel: string): string => {
    if (this.session.configurationPending) return tr("gui.create.preparing_settings");
    if (this.session.preflight.state.picker) return tr("gui.create.sources.adding");
    if (this.session.preflight.busy()) return tr("gui.create.checking");
    if (this.session.preflight.state.pending) return tr("gui.create.review_plan_below");
    return readyLabel;
  };

  sourcePickerLabel = (sourceKind: "files" | "folder"): string => {
    if (this.session.preflight.state.picker?.kind === sourceKind) {
      return sourceKind === "files"
        ? tr("gui.create.opening_file_picker")
        : tr("gui.create.opening_folder_picker");
    }
    return sourceKind === "files"
      ? tr("gui.action.add_files")
      : tr("gui.create.sources.add_folders");
  };

  configurationPendingMessage = (): string => {
    return tr(
      "gui.create.wait_for_initial_settings",
    );
  };

  optionsLockedReason = (): string => {
    return this.session.preflight.busy()
      ? tr("gui.create.options_locked_preflight")
      : "";
  };

  preflightPhaseLabel = (): string => {
    const preflight = this.session.preflight.state;
    switch (preflight.phase) {
      case "selecting":
        return tr("gui.create.waiting_source_picker");
      case "measuring":
        return tr("gui.create.measuring_source_bytes");
      case "checkingTemp":
        return tr("gui.create.checking_temp_workspace");
      case "choosingDest":
        if (preflight.requestKind === "destination") {
          return preflight.cancelPending
            ? tr("gui.create.cancelling_destination_check")
            : tr("gui.create.checking_existing_destination", "Checking the current output before replacement");
        }
        return preflight.current
          || tr("gui.create.waiting_destination");
      case "checkingDest":
        return tr("gui.create.checking_destination_disk_short");
      case "reviewing":
        return tr("gui.create.ready_for_review");
      case "submitting":
        if (preflight.requestKind === "destination") {
          return preflight.cancelPending
            ? tr("gui.create.cancelling_destination_check")
            : tr("gui.create.rechecking_existing_destination");
        }
        if (preflight.current) return preflight.current;
        return preflight.creatingSfx
          ? tr("gui.create.submitting_sfx_job")
          : tr("gui.create.submitting_archive_job");
      case "ready":
        return tr("gui.create.preflight_ready");
      case "cancelled":
        return tr("gui.create.preflight_cancelled");
      case "blocked":
        return tr("gui.create.preflight_blocked");
      case "idle":
        return tr("gui.create.preflight_pending");
    }
  };

  preflightStepState = (stage: Exclude<CreatePreflightStage, "submit">): CreatePreflightStepState => {
    const preflight = this.session.preflight.state;
    if (preflight.issueStage === stage) {
      return preflight.phase === "cancelled" ? "cancelled" : "blocked";
    }
    if (stage === "source") {
      if (preflight.phase === "selecting" || preflight.phase === "measuring") return "active";
      if (preflight.plan && preflight.plan.entries > 0) return "ready";
      return "pending";
    }
    if (stage === "temp") {
      if (preflight.phase === "checkingTemp") return "active";
      if (preflight.workspaceDisk) {
        return preflight.workspaceDisk.ok && (preflight.systemTempDisk?.ok ?? true) ? "ready" : "blocked";
      }
      return "pending";
    }
    if (
      preflight.phase === "choosingDest"
      || preflight.phase === "checkingDest"
      || (preflight.phase === "submitting" && preflight.requestKind === "destination")
    ) return "active";
    if (preflight.destinationDisk) return preflight.destinationDisk.ok ? "ready" : "blocked";
    return "pending";
  };

  preflightStepStateLabel = (state: CreatePreflightStepState): string => {
    if (state === "active") return tr("gui.create.preflight_stage_active");
    if (state === "ready") return tr("gui.create.preflight_stage_ready");
    if (state === "blocked") return tr("gui.create.preflight_stage_blocked");
    if (state === "cancelled") return tr("gui.create.preflight_stage_cancelled");
    return tr("gui.create.preflight_stage_pending");
  };

  preflightCurrentDetail = (): string => {
    if (this.session.preflight.state.phase !== "measuring" || !this.session.preflight.state.current) return "";
    return tr("gui.create.scanning_current_item").replace("{path}", this.session.preflight.state.current);
  };

  destinationPreflightDetail = (): string => {
    if (this.session.preflight.state.requestKind === "destination" && this.session.preflight.state.current) {
      return tr("gui.create.destination_check_current")
        .replace("{path}", this.session.preflight.state.current);
    }
    return this.session.preflight.state.destination
      ? tr("gui.create.destination_path_checked").replace("{path}", this.session.preflight.state.destination)
      : "";
  };

  destinationInspectionCancelLabel = (): string => {
    return this.session.preflight.state.cancelPending
      ? tr("gui.create.cancelling_destination_check")
      : tr("gui.create.cancel_destination_check");
  };

  preflightStageIssueSummary = (stage: Exclude<CreatePreflightStage, "submit">): string | null => {
    if (this.session.preflight.state.issueStage !== stage) return null;
    return this.session.preflight.state.phase === "cancelled"
      ? tr("gui.create.preflight_stage_cancelled_summary")
      : tr("gui.create.preflight_stage_blocked_summary");
  };

  preflightSteps = () => {
    const sourceState = this.preflightStepState("source");
    const tempState = this.preflightStepState("temp");
    const destinationState = this.preflightStepState("destination");
    return [
      {
        id: "source",
        label: tr("gui.create.input_preflight"),
        summary: this.estimateStatusbar(),
        detail: this.preflightCurrentDetail(),
        state: sourceState,
        stateLabel: this.preflightStepStateLabel(sourceState),
      },
      {
        id: "temp",
        label: tr("gui.create.temp_preflight"),
        summary: this.tempPreflightStatusbar(),
        detail: "",
        state: tempState,
        stateLabel: this.preflightStepStateLabel(tempState),
      },
      {
        id: "destination",
        label: tr("gui.create.disk_preflight"),
        summary: this.diskPreflightStatusbar(),
        detail: this.destinationPreflightDetail(),
        state: destinationState,
        stateLabel: this.preflightStepStateLabel(destinationState),
      },
    ];
  };

  volumePreview = (): string => {
    const splitSize = this.session.options.splitSize;
    if (splitSize === null) {
      return tr("gui.create.single_archive_summary");
    }
    const size = formatBytes(splitSize);
    if (!this.session.preflight.state.plan) {
      const nativeWim = this.session.options.state.splitMode === "native" && this.session.options.state.format === "wim";
      const key = nativeWim
        ? "gui.create.native_split_wim_summary_pending"
        : this.session.options.state.splitMode === "native"
          ? "gui.create.native_split_summary_pending"
          : "gui.create.split_summary_pending";
      const fallback = nativeWim
        ? "{size} target per part · native .swm set; one large file may exceed the target"
        : this.session.options.state.splitMode === "native"
          ? "{size} per part · native ZIP set ending in .zip"
          : "{size} per part · final count appears after preflight and write";
      return tr(key, fallback)
        .replace("{size}", size);
    }
    const count = Math.max(1, Math.trunc(this.session.preflight.state.plan.split_volume_count_budget ?? 1));
    return tr("gui.create.volume_output_budget_guide")
      .replace("{size}", size)
      .replace("{count}", String(count));
  };

  setupPresetLabel = (): string => {
    const selected = this.session.port.presets().selected("create");
    return selected
      ? archivePresetDisplayName(selected)
      : tr("gui.create.setup.custom");
  };

  completionSummaryLabel = (): string => {
    if (this.session.options.state.completion === "reveal_output") {
      return tr("gui.create.output.completion.reveal")
        .replace("{fileManager}", this.session.port.fileManager());
    }
    if (this.session.options.state.completion === "open_in_squallz") {
      return tr("gui.create.output.completion.open");
    }
    return tr("gui.create.output.completion.none");
  };

  sourceHandlingSummaryLabel = (): string => {
    return this.session.options.state.postSuccess === "trash_source"
      ? tr("gui.create.output.source.trash").replace("{trash}", this.session.port.trashName())
      : tr("gui.create.output.source.keep");
  };

  integritySummaryLabel = (): string => {
    return this.session.options.effectiveTestAfterCreate
      ? tr("gui.create.output.integrity.enabled_summary")
      : tr("gui.create.output.integrity.disabled_summary");
  };

  protectionSummaryLabel = (): string => {
    if (this.session.options.state.password.length > 0 && this.session.options.state.encryptNames) {
      return tr("gui.create.setup.password_and_names");
    }
    if (this.session.options.state.password.length > 0) {
      return tr("gui.create.setup.password_on");
    }
    if (this.session.options.state.encryptionEnabled) {
      return tr("gui.create.setup.password_required");
    }
    return tr("gui.create.setup.no_password");
  };

  setupSummaryItems = () => {
    const outputValue = this.session.options.state.sfxEnabled
      ? this.sfxOutputLabel()
      : this.session.formatData.label;
    const outputDetail = this.session.options.state.sfxEnabled ? this.sfxSummary() : this.methodLabel();
    return [
      {
        id: "preset",
        icon: "sparkles",
        label: tr("gui.create.setup.preset"),
        value: this.setupPresetLabel(),
        detail: createArchivePresetSummary(this.session.options.snapshotPreset()),
      },
      {
        id: "artifact",
        icon: "archive",
        label: tr("gui.create.setup.artifact"),
        value: outputValue,
        detail: outputDetail,
      },
      {
        id: "destination",
        icon: "folder-open",
        label: tr("gui.create.setup.destination"),
        value: this.outputPreview(),
        detail: tr(
          "gui.create.setup.afterwards_with_integrity",
        )
          .replace("{completion}", this.completionSummaryLabel())
          .replace("{sources}", this.sourceHandlingSummaryLabel())
          .replace("{integrity}", this.integritySummaryLabel()),
      },
      {
        id: "protection",
        icon: "lock",
        label: tr("gui.create.setup.protection"),
        value: this.protectionSummaryLabel(),
        detail: tr("gui.create.setup.volume_and_content")
          .replace("{volumes}", this.volumePreview())
          .replace("{content}", createContentPolicyLabel(this.session.options.state.contentPolicy)),
      },
    ];
  };

  planLayoutSummary = (): string => {
    const pending = this.session.preflight.state.pending;
    const plan = this.session.preflight.state.plan;
    if (!pending || !plan) return "";
    if (pending.creatingSfx) return pending.artifactLabel;
    const count = plan.split_volume_count_budget;
    if (pending.splitSize !== null && count !== null) {
      const volumeSummary = tr("gui.create.review.numbered_volumes")
        .replace("{count}", Math.max(1, Math.trunc(count)).toLocaleString());
      return pending.format === "sqz"
        ? tr("gui.create.review.sqz_recovery_separate")
          .replace("{volumes}", volumeSummary)
        : volumeSummary;
    }
    return tr("gui.create.review.single_file");
  };

  planReviewItems = () => {
    const plan = this.session.preflight.state.plan;
    if (!plan) return [];
    const workspace = plan.system_temp_budget_bytes > 0
      ? tr("gui.create.review.workspace_split")
        .replace("{destination}", formatBytes(plan.workspace_budget_bytes))
        .replace("{temporary}", formatBytes(plan.system_temp_budget_bytes))
      : tr("gui.create.review.workspace_destination")
        .replace("{size}", formatBytes(plan.workspace_budget_bytes));
    return [
      {
        id: "inputs",
        label: tr("gui.create.review.inputs"),
        value: tr("gui.create.review.input_value")
          .replace("{size}", formatBytes(plan.total_bytes))
          .replace("{entries}", plan.entries.toLocaleString()),
      },
      ...(plan.deduplicated_entries > 0
        ? [{
            id: "overlap",
            label: tr("gui.create.review.overlap"),
            value: tr(
              "gui.create.review.overlap_value",
            ).replace("{count}", plan.deduplicated_entries.toLocaleString()),
          }]
        : []),
      {
        id: "layout",
        label: tr("gui.create.review.layout"),
        value: this.planLayoutSummary(),
      },
      {
        id: "budget",
        label: tr("gui.create.review.output_budget"),
        value: formatBytes(plan.final_output_budget_bytes),
      },
      {
        id: "workspace",
        label: tr("gui.create.review.workspace"),
        value: workspace,
      },
    ];
  };

  surface = (variant: CreateWorkspaceVariant): CreateWorkspaceSurface => {
    const preflight = this.session.preflight.state;
    const preflightBusy = this.session.preflight.busy();
    const lockedReason = this.optionsLockedReason();
    const sqzPayloadLabel = tr("gui.presets.sqz_inner_format");
    const review = preflight.pending && preflight.plan
      ? {
          variant,
          ariaLabel: tr("gui.create.review.aria"),
          eyebrow: tr("gui.create.review.eyebrow"),
          heading: tr("gui.create.review.heading"),
          description: preflight.issue || tr("gui.create.review.description"),
          outputName: preflight.plan.primary_output,
          items: this.planReviewItems(),
          confirmLabel: this.planConfirmLabel(),
          cancelLabel: tr("gui.create.review.cancel"),
          busy: preflight.phase === "submitting",
          onConfirm: () => void this.session.confirm(),
          onCancel: this.session.cancelReview,
        }
      : null;

    return {
      tr,
      onReady: () => {
        if (this.session.port.primaryFocusPending()) this.session.port.focusPrimary();
      },
      profiles: createProfileIds.map((profileId) => ({
        id: profileId,
        label: this.profileLabel(profileId),
        selected: this.session.options.state.profile === profileId,
        disabled: preflightBusy,
        title: lockedReason,
        ariaLabel: labelWithDisabledReason(this.profileLabel(profileId), lockedReason),
        onSelect: () => this.session.chooseProfile(profileId),
      })),
      formats: createFormatIds.map((formatId) => {
        const disabledReason = this.formatDisabledReason(formatId);
        return {
          id: formatId,
          label: createFormats[formatId].label,
          selected: this.session.options.state.format === formatId,
          disabled: Boolean(disabledReason),
          title: disabledReason || this.formatNoteFor(formatId),
          ariaLabel: labelWithDisabledReason(createFormats[formatId].label, disabledReason),
          onSelect: () => this.session.chooseFormat(formatId),
        };
      }),
      formatNote: this.formatNoteFor(),
      sqzPayload: this.session.options.state.format === "sqz"
        ? {
            label: sqzPayloadLabel,
            options: presetSqzInnerFormats.map((innerFormat) => ({
              id: innerFormat,
              label: presetSqzInnerFormatLabel(innerFormat),
              selected: this.session.options.state.sqzInnerFormat === innerFormat,
              disabled: preflightBusy,
              title: lockedReason,
              ariaLabel: labelWithDisabledReason(presetSqzInnerFormatLabel(innerFormat), lockedReason),
              onSelect: () => this.session.edit({ kind: "sqzInnerFormat", value: innerFormat }),
            })),
          }
        : null,
      compression: {
        level: this.session.options.compressionLevel,
        detail: this.profileDetail(this.session.options.state.profile),
        method: this.methodLabel(),
        custom: this.session.options.state.profile === "custom"
          ? {
              value: this.session.options.state.customLevel,
              error: this.session.options.state.customLevelInvalid ? this.issue("custom_level_invalid") : "",
              disabled: preflightBusy,
              title: lockedReason,
              rangeAriaLabel: labelWithDisabledReason(
                variant === "classic"
                  ? tr("gui.create.classic_custom_compression_level")
                  : tr("gui.create.custom_compression_level"),
                lockedReason,
              ),
              numberAriaLabel: labelWithDisabledReason(
                variant === "classic"
                  ? tr("gui.create.classic_custom_compression_level_number")
                  : tr("gui.create.custom_compression_level_number"),
                lockedReason,
              ),
              onInput: (event) => this.session.updateLevelFromInput(event),
              onChange: (event) => this.session.updateLevelFromInput(event, true),
            }
          : null,
      },
      setupSummary: {
        variant,
        ariaLabel: tr("gui.create.setup.aria"),
        eyebrow: tr("gui.create.setup.eyebrow"),
        heading: tr("gui.create.setup.heading"),
        items: this.setupSummaryItems(),
      },
      preset: { instanceId: `${variant}-create`, variant, compact: false, kind: "create", session: this.session.port.presets() },
      advanced: {
        open: this.session.advancedOpen,
        onToggle: (open) => (this.session.advancedOpen = open),
        onKeydown: this.session.toggleAdvanced,
      },
      output: {
        instanceId: `${variant}-create-output`,
        variant,
        destination: this.session.options.state.destinationBase,
        completion: this.session.options.state.completion,
        postSuccess: this.session.options.state.postSuccess,
        testAfterCreate: this.session.options.effectiveTestAfterCreate,
        testAfterCreateRequired: this.session.options.state.postSuccess === "trash_source",
        outputPreview: this.outputPreview(),
        defaultFolder: this.session.port.read().defaultDirectory ?? "",
        fileManager: this.session.port.fileManager(),
        trashName: this.session.port.trashName(),
        disabled: preflightBusy,
        disabledReason: lockedReason,
        openDisabledReason: this.openCompletionDisabledReason(),
        trashDisabledReason: this.trashSourceDisabledReason(),
        tr,
        onDestinationChange: (value) => this.session.edit({ kind: "destinationBase", value }),
        onCompletionChange: (value) => this.session.edit({ kind: "completion", value }),
        onPostSuccessChange: (value) => this.session.edit({ kind: "postSuccess", value }),
        onTestAfterCreateChange: (value) => this.session.edit({ kind: "testAfterCreate", value }),
      },
      content: {
        variant,
        classicSectionId: variant === "classic" ? "classic-create-content" : undefined,
        value: this.session.options.state.contentPolicy,
        rulesText: this.session.options.state.excludeText,
        rules: this.session.options.excludeRules,
        disabled: preflightBusy,
        disabledReason: lockedReason,
        tr,
        onChange: (value) => this.session.edit({ kind: "contentPolicy", value }),
        onRulesInput: (value) => this.session.edit({ kind: "excludeText", value }),
      },
      recovery: {
        capability: this.formatRecovery(),
        disabled: preflightBusy,
        disabledReason: lockedReason,
        onOpen: this.session.port.openRecovery,
      },
      sfx: {
        variant,
        classicSectionId: variant === "classic" ? "classic-create-security" : undefined,
        enabled: this.session.options.state.sfxEnabled,
        available: this.session.capability.available,
        targetLabel: this.sfxTargetLabel(),
        outputLabel: this.sfxOutputLabel(),
        summary: this.sfxSummary(),
        signingWarning: this.sfxSigningWarning(),
        unavailableMessage: this.sfxUnavailableMessage(),
        disabled: preflightBusy || !this.session.capabilityReady,
        disabledReason: lockedReason || (!this.session.capabilityReady
          ? tr("gui.create.sfx_capability_loading")
          : ""),
        loading: !this.session.capabilityReady,
        tr,
        onEnabledChange: (value) => this.session.edit({ kind: "sfxEnabled", value }),
      },
      protection: {
        variant,
        classicSplitSectionId: variant === "classic" ? "classic-create-volumes" : undefined,
        ...outputProtectionControls(this.session.options.state, (edit) => this.session.edit(edit), tr,
          this.session.options.passwordIssue ? this.issue(this.session.options.passwordIssue) : "",
          this.session.options.splitIssue ? this.issue(this.session.options.splitIssue) : ""),
        splitDisabled: this.session.options.state.sfxEnabled,
        nativeSplitKind: this.session.options.nativeSplitKind,
        splitCapability: this.splitCapability(),
        splitSummary: this.volumePreview(),
        disabled: preflightBusy,
        disabledReason: lockedReason,
        tr,
      },
      showPreflight: preflight.phase !== "idle",
      preflight: {
        variant,
        phase: preflight.phase,
        ariaLabel: tr("gui.create.preflight_status"),
        heading: tr("gui.create.preflight_heading"),
        statusLabel: this.preflightPhaseLabel(),
        lockMessage: lockedReason,
        actionLabel: this.session.destinationCancellable()
          ? this.destinationInspectionCancelLabel()
          : "",
        actionPending: preflight.cancelPending,
        issue: preflight.issue,
        steps: this.preflightSteps(),
        onAction: () => void this.session.preflight.cancelDestination(),
      },
      review,
      classic: {
        archiveName: this.archivePreviewName(),
        activeSection: this.session.section,
        sections: [
          {
            id: "general",
            label: tr("gui.settings.section.general"),
            targetId: "classic-create-general",
            onSelect: () => void this.session.showSection("general", "classic-create-general"),
          },
          {
            id: "compression",
            label: tr("gui.create.section_compression"),
            targetId: "classic-create-compression",
            onSelect: () => void this.session.showSection("compression", "classic-create-compression"),
          },
          {
            id: "content",
            label: tr("gui.create.section_content"),
            targetId: "classic-create-content",
            onSelect: () => void this.session.showSection("content", "classic-create-content"),
          },
          {
            id: "security",
            label: tr("gui.settings.section.security"),
            targetId: "classic-create-security",
            onSelect: () => void this.session.showSection("security", "classic-create-security"),
          },
          {
            id: "volumes",
            label: tr("gui.create.section_volumes"),
            targetId: "classic-create-volumes",
            onSelect: () => void this.session.showSection("volumes", "classic-create-volumes"),
          },
          {
            id: "recovery",
            label: tr("gui.recovery.title"),
            targetId: "classic-create-recovery",
            onSelect: () => void this.session.showSection("recovery", "classic-create-recovery"),
          },
          {
            id: "preflight",
            label: tr("gui.create.input_preflight", "Preflight"),
            targetId: "classic-create-preflight",
            onSelect: () => void this.session.showSection("preflight", "classic-create-preflight"),
          },
        ],
        recoveryCapability: this.formatRecovery(),
        updateMode: tr("gui.create.add_and_replace_files"),
        featuredFormats: this.session.port.featuredFormats(),
      },
    };
  };

  estimateStatusbar = (): string => {
    const preflight = this.session.preflight.state;
    const interrupted = this.preflightStageIssueSummary("source");
    if (interrupted) return interrupted;
    if (preflight.phase === "selecting") return tr("gui.create.waiting_source_picker");
    if (preflight.phase === "measuring") {
      return preflight.scanned > 0
        ? tr("gui.create.scanning_inputs_count").replace("{count}", preflight.scanned.toLocaleString())
        : tr("gui.create.measuring_input_bytes");
    }
    if (preflight.phase === "blocked" && preflight.plan?.entries === 0) return tr("gui.create.no_entries_after_excludes");
    if (!preflight.plan) return tr("gui.create.input_estimate_pending");
    return tr("gui.create.estimate_status")
      .replace("{size}", formatBytes(preflight.plan.total_bytes))
      .replace("{entries}", preflight.plan.entries.toLocaleString())
      .replace(
        "{excludes}",
        tr("gui.create.rule_count").replace("{count}", preflight.excludeCount.toLocaleString()),
      );
  };

  diskPreflightStatusbar = (): string => {
    const preflight = this.session.preflight.state;
    const interrupted = this.preflightStageIssueSummary("destination");
    if (interrupted) return interrupted;
    if (preflight.phase === "choosingDest") {
      if (preflight.requestKind === "destination") {
        return tr("gui.create.destination_check_progress")
          .replace("{bytes}", formatBytes(preflight.processedBytes));
      }
      return preflight.current
        || tr("gui.create.waiting_destination_picker");
    }
    if (preflight.phase === "submitting" && preflight.requestKind === "destination") {
      return tr("gui.create.destination_recheck_progress")
        .replace("{bytes}", formatBytes(preflight.processedBytes));
    }
    if (preflight.phase === "checkingDest") return tr("gui.create.checking_destination_disk");
    if (!preflight.destinationDisk) return tr("gui.create.destination_disk_pending");
    return tr("gui.create.disk_status_available")
      .replace("{status}", preflight.destinationDisk.ok ? tr("gui.create.disk_ok") : tr("gui.create.disk_blocked"))
      .replace("{available}", formatBytes(preflight.destinationDisk.available_bytes));
  };

  tempPreflightStatusbar = (): string => {
    const preflight = this.session.preflight.state;
    const interrupted = this.preflightStageIssueSummary("temp");
    if (interrupted) return interrupted;
    if (preflight.phase === "checkingTemp") return tr("gui.create.checking_temporary_space");
    if (!preflight.workspaceDisk) return tr("gui.create.temp_preflight_pending");
    if (preflight.systemTempDisk) {
      const ok = preflight.workspaceDisk.ok && preflight.systemTempDisk.ok;
      return tr("gui.create.temp_status_destination_and_system")
        .replace("{status}", ok ? tr("gui.create.temp_ok") : tr("gui.create.temp_blocked"))
        .replace("{destination}", formatBytes(preflight.workspaceDisk.available_bytes))
        .replace("{temporary}", formatBytes(preflight.systemTempDisk.available_bytes));
    }
    return tr("gui.create.temp_status_available")
      .replace("{status}", preflight.workspaceDisk.ok ? tr("gui.create.temp_ok") : tr("gui.create.temp_blocked"))
      .replace("{available}", formatBytes(preflight.workspaceDisk.available_bytes));
  };

  planConfirmLabel = (): string => {
    if (this.session.preflight.state.phase === "submitting") {
      return tr("gui.create.review.submitting");
    }
    if (this.session.preflight.state.issueStage === "submit" || this.session.preflight.state.issueStage === "destination") {
      return tr("gui.create.review.retry");
    }
    return tr("gui.create.review.confirm");
  };

  sourcesAdded = (result: { added: number; total: number }): void => {
    this.session.port.notice(
      result.added > 0
        ? tr("gui.create.sources.added")
          .replace("{added}", result.added.toLocaleString())
          .replace("{total}", result.total.toLocaleString())
        : tr("gui.create.sources.already_added"),
    );
  };
}
