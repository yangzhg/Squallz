import { fat32CompatibleSplitSizeBytes, resolveSplitSizeBytes } from "./archive-output-options";
import type { CreateCredentialIntent, CreateRunDraft } from "./create-preflight.svelte";
import { parseDelimitedRules } from "./format";
import type {
  CreateArchivePresetOptions, CreateCompletionAction, CreateContentPolicy,
  CreateDestinationBase, JobSpec, OverwritePolicy, PostSuccessAction, SfxCreateCapabilityDto,
} from "./ipc";
import {
  createFormatIds, createFormats, createProfiles,
  type CreateFormatId, type CreateProfileId, type CreateSplitMode, type CreateSplitPreset, type CreateSplitUnit,
} from "./ui-model";

type CompressJob = Extract<JobSpec, { kind: "compress" }>;
type SfxTarget = Extract<CreateArchivePresetOptions["output"], { kind: "self_extracting" }>["target"];
type SqzInnerFormat = Extract<CreateArchivePresetOptions["format_options"], { kind: "sqz" }>["inner_format"];

export type CreateDraftIssue =
  | "options_locked" | "format_unavailable" | "task_format_unavailable"
  | "sfx_loading" | "sfx_unavailable" | "sfx_target_unavailable" | "task_sfx_unavailable"
  | "sfx_zip_only" | "encryption_format_change" | "name_encryption_format_change"
  | "sfx_no_split" | "native_layout_unavailable" | "trash_excluded" | "completion_sfx" | "completion_split"
  | "trash_test_required" | "custom_level_invalid" | "password_required" | "confirm_password_required" | "passwords_do_not_match"
  | "invalid_part_size" | "native_zip_part_size_limit" | "sfx_requires_zip";

export type CreateDraftContext = Readonly<{
  capabilityReady: boolean;
  sfxCapability: SfxCreateCapabilityDto;
  locked?: boolean;
}>;

export type CreateDraftEdit =
  | { kind: "format"; value: CreateFormatId }
  | { kind: "profile"; value: CreateProfileId }
  | { kind: "customLevel"; value: number | string }
  | { kind: "password"; value: string }
  | { kind: "passwordConfirmation"; value: string }
  | { kind: "customSplitAmount"; value: string }
  | { kind: "excludeText"; value: string }
  | { kind: "passwordVisible" | "encryptionEnabled" | "encryptNames" | "sfxEnabled" | "testAfterCreate"; value: boolean }
  | { kind: "splitPreset"; value: CreateSplitPreset }
  | { kind: "splitMode"; value: CreateSplitMode }
  | { kind: "customSplitUnit"; value: CreateSplitUnit }
  | { kind: "contentPolicy"; value: CreateContentPolicy }
  | { kind: "sqzInnerFormat"; value: SqzInnerFormat }
  | { kind: "destinationBase"; value: CreateDestinationBase }
  | { kind: "completion"; value: CreateCompletionAction }
  | { kind: "postSuccess"; value: PostSuccessAction };

export type CreateDraftChange = Readonly<{
  changed: boolean;
  issue?: CreateDraftIssue;
  postSuccessNormalized?: boolean;
  completionNormalized?: boolean;
  requestedSfxTarget?: SfxCreateCapabilityDto["target"];
}>;

type CreateOptionsState = {
  format: CreateFormatId;
  profile: CreateProfileId;
  customLevel: number;
  customLevelInvalid: boolean;
  password: string;
  passwordConfirmation: string;
  passwordVisible: boolean;
  encryptionEnabled: boolean;
  encryptNames: boolean;
  splitPreset: CreateSplitPreset;
  splitMode: CreateSplitMode;
  customSplitAmount: string;
  customSplitUnit: CreateSplitUnit;
  exactSplitSizeBytes: string | null;
  contentPolicy: CreateContentPolicy;
  excludeText: string;
  sfxEnabled: boolean;
  sfxTarget: SfxTarget;
  sqzInnerFormat: SqzInnerFormat;
  destinationBase: CreateDestinationBase;
  overwritePolicy: OverwritePolicy;
  suggestedDestination: string | null;
  completion: CreateCompletionAction;
  postSuccess: PostSuccessAction;
  testAfterCreate: boolean;
  validationAttempted: boolean;
  touched: boolean;
};

type RestoredOptions = Pick<CreateOptionsState,
  "format" | "customLevel" | "encryptionEnabled" | "encryptNames" | "contentPolicy" | "excludeText"
  | "sfxEnabled" | "sfxTarget" | "sqzInnerFormat" | "destinationBase" | "suggestedDestination"
  | "completion" | "postSuccess" | "testAfterCreate" | "splitMode"> & {
  volumes: CreateArchivePresetOptions["volumes"];
};

const rejected = (issue: CreateDraftIssue): CreateDraftChange => ({ changed: false, issue });
const clampCustomLevel = (value: number): number => Number.isFinite(value) ? Math.min(9, Math.max(1, Math.round(value))) : 6;

/** Owns editable create options; native plans and submission belong to CreatePreflightSession. */
export class CreateOptionsDraft {
  #state = $state<CreateOptionsState>({
    format: "7z", profile: "balanced", customLevel: 6, customLevelInvalid: false,
    password: "", passwordConfirmation: "", passwordVisible: false, encryptionEnabled: false, encryptNames: false,
    splitPreset: "none", splitMode: "generic", customSplitAmount: "100", customSplitUnit: "mib", exactSplitSizeBytes: null,
    contentPolicy: "cross_platform_clean", excludeText: "", sfxEnabled: false, sfxTarget: "current_platform",
    sqzInnerFormat: "sqz", destinationBase: "ask", overwritePolicy: "ask", suggestedDestination: null,
    completion: "none", postSuccess: "keep_source", testAfterCreate: false, validationAttempted: false, touched: false,
  });

  constructor(initial: Pick<CreateOptionsState, "format" | "profile" | "customLevel"> & { touched?: boolean }) {
    Object.assign(this.#state, initial, { customLevel: clampCustomLevel(initial.customLevel) });
  }

  get state(): Readonly<CreateOptionsState> { return this.#state; }
  get compressionLevel(): number {
    const state = this.#state;
    return state.profile === "custom" ? state.customLevel : createProfiles[state.profile].level;
  }
  get splitSize(): number | null {
    const state = this.#state;
    return resolveSplitSizeBytes(state.splitPreset, state.customSplitAmount, state.customSplitUnit, state.exactSplitSizeBytes);
  }
  get excludeRules(): string[] { return parseDelimitedRules(this.#state.excludeText); }
  get effectiveTestAfterCreate(): boolean { return this.#state.testAfterCreate || this.#state.postSuccess === "trash_source"; }
  get trashExcluded(): boolean {
    return this.#state.contentPolicy === "cross_platform_clean" || (this.#state.contentPolicy === "custom" && this.excludeRules.length > 0);
  }
  get completionRestriction(): "sfx" | "split" | null {
    if (this.#state.sfxEnabled) return "sfx";
    return this.splitSize !== null ? "split" : null;
  }
  get nativeSplitKind(): "zip" | "wim" | null {
    const { format, sfxEnabled } = this.#state;
    return !sfxEnabled && (format === "zip" || format === "wim") ? format : null;
  }
  get passwordIssue(): CreateDraftIssue | null {
    const state = this.#state;
    if (!createFormats[state.format].can_encrypt_data) return null;
    if (state.encryptionEnabled && !state.password) return "password_required";
    if (!state.password) return null;
    if (!state.passwordConfirmation) return "confirm_password_required";
    return state.password === state.passwordConfirmation ? null : "passwords_do_not_match";
  }
  get splitIssue(): CreateDraftIssue | null {
    const size = this.splitSize;
    if (this.#state.splitPreset === "custom" && size === null) return "invalid_part_size";
    return this.#state.format === "zip" && this.#state.splitMode === "native" && size !== null && size > fat32CompatibleSplitSizeBytes
      ? "native_zip_part_size_limit" : null;
  }

  outputExtension(sfxExtension: string): string {
    if (this.#state.sfxEnabled) return sfxExtension;
    return this.#state.format === "wim" && this.splitSize !== null && this.#state.splitMode === "native"
      ? "swm" : createFormats[this.#state.format].extension;
  }

  edit(edit: CreateDraftEdit, context: CreateDraftContext, beforeEdit: () => void): CreateDraftChange {
    const issue = this.#editIssue(edit, context);
    if (issue) return rejected(issue);
    if (edit.kind === "customLevel" && typeof edit.value === "string") {
      const raw = edit.value.trim();
      const value = Number(raw);
      if (!raw || !Number.isInteger(value) || value < 1 || value > 9) {
        this.#state.customLevelInvalid = true;
        return rejected("custom_level_invalid");
      }
      edit = { kind: "customLevel", value };
    }
    const state = this.#state;
    if ((edit.kind === "contentPolicy" || edit.kind === "destinationBase" || edit.kind === "completion"
      || edit.kind === "postSuccess" || edit.kind === "testAfterCreate") && state[edit.kind] === edit.value) {
      return { changed: false };
    }
    // The old plan can reset credentials synchronously when it is invalidated.
    if (edit.kind !== "passwordVisible") beforeEdit();
    switch (edit.kind) {
      case "format":
        state.format = edit.value;
        if (this.nativeSplitKind === null) state.splitMode = "generic";
        if (edit.value === "sqz") state.sqzInnerFormat = "sqz";
        if (!createFormats[edit.value].can_encrypt_data) this.#clearCredentials();
        else if (!createFormats[edit.value].can_encrypt_names) state.encryptNames = false;
        state.validationAttempted = false;
        break;
      case "profile": state.profile = edit.value; break;
      case "customLevel":
        state.customLevel = clampCustomLevel(Number(edit.value));
        state.customLevelInvalid = false;
        state.profile = "custom";
        break;
      case "password":
        state.password = edit.value;
        if (edit.value) state.encryptionEnabled = true;
        state.validationAttempted = !edit.value && state.encryptionEnabled;
        if (!edit.value) state.passwordConfirmation = "";
        break;
      case "passwordConfirmation": state.passwordConfirmation = edit.value; state.validationAttempted = false; break;
      case "passwordVisible":
        state.passwordVisible = edit.value;
        return { changed: true };
      case "encryptionEnabled":
        state.encryptionEnabled = edit.value && createFormats[state.format].can_encrypt_data;
        if (!state.encryptionEnabled) this.#clearCredentials();
        state.validationAttempted = state.encryptionEnabled;
        break;
      case "encryptNames": state.encryptNames = edit.value && createFormats[state.format].can_encrypt_names && state.encryptionEnabled; break;
      case "splitPreset":
        state.splitPreset = edit.value;
        state.exactSplitSizeBytes = null;
        if (edit.value === "none") state.splitMode = "generic";
        state.validationAttempted = false;
        break;
      case "splitMode":
        state.splitMode = edit.value;
        state.validationAttempted = false;
        break;
      case "customSplitAmount": case "customSplitUnit":
        if (edit.kind === "customSplitAmount") state.customSplitAmount = edit.value;
        else state.customSplitUnit = edit.value;
        state.exactSplitSizeBytes = null;
        state.validationAttempted = false;
        break;
      case "sfxEnabled":
        state.sfxEnabled = edit.value;
        state.sfxTarget = "current_platform";
        state.validationAttempted = false;
        if (edit.value) {
          state.format = "zip";
          state.splitPreset = "none";
          state.splitMode = "generic";
          state.exactSplitSizeBytes = null;
          state.encryptNames = false;
        }
        break;
      case "destinationBase":
        state.destinationBase = edit.value;
        state.overwritePolicy = edit.value === "ask" ? "ask" : "rename";
        break;
      case "completion":
        state.completion = edit.value;
        break;
      case "postSuccess":
        state.postSuccess = edit.value;
        break;
      case "testAfterCreate":
        state.testAfterCreate = edit.value;
        break;
      case "contentPolicy":
        state.contentPolicy = edit.value;
        break;
      case "excludeText": state.excludeText = edit.value; break;
      case "sqzInnerFormat": state.sqzInnerFormat = edit.value; break;
    }
    state.touched = true;
    return this.#normalize(edit.kind === "contentPolicy" || edit.kind === "excludeText",
      edit.kind === "splitPreset" || edit.kind === "customSplitAmount" || edit.kind === "customSplitUnit"
      || (edit.kind === "sfxEnabled" && edit.value));
  }

  touch(): void { this.#state.touched = true; }
  resetValidation(): void { this.#state.validationAttempted = false; }
  clearSuggestedDestination(): void { this.#state.suggestedDestination = null; }
  acceptSfxCapability(available: boolean): void { if (!available) this.#state.sfxEnabled = false; }
  normalizePostSuccess(): boolean { return Boolean(this.#normalize(true, false).postSuccessNormalized); }
  resetCredentials(intent: CreateCredentialIntent | null): void {
    this.#clearCredentials();
    if (intent?.restoreCredentialPrompt) {
      this.#state.encryptionEnabled = true;
      this.#state.encryptNames = intent.restoreEncryptNames;
    }
  }
  validate(capability: SfxCreateCapabilityDto): CreateDraftIssue | null {
    this.#state.validationAttempted = true;
    const inputIssue = this.passwordIssue ?? this.splitIssue;
    if (inputIssue) return inputIssue;
    if (this.#state.sfxEnabled) {
      if (!capability.available) return "sfx_unavailable";
      if (this.#state.format !== "zip" || this.#state.splitPreset !== "none") return "sfx_requires_zip";
    }
    return null;
  }

  applyPreset(options: CreateArchivePresetOptions, context: CreateDraftContext, beforeApply: () => void, touch = true): CreateDraftChange {
    if (context.locked) return rejected("options_locked");
    if (!createFormatIds.includes(options.format as CreateFormatId)) return rejected("format_unavailable");
    if (options.output.kind === "self_extracting") {
      if (!context.capabilityReady) return rejected("sfx_loading");
      if (!context.sfxCapability.available) return rejected("sfx_unavailable");
      const target = this.#resolveSfxTarget(options.output.target, context.sfxCapability);
      if (target !== context.sfxCapability.target) {
        return { ...rejected("sfx_target_unavailable"), requestedSfxTarget: target };
      }
    }
    beforeApply();
    const change = this.#restore({
      format: options.format as CreateFormatId, customLevel: options.level,
      encryptionEnabled: options.credential.kind !== "none", encryptNames: options.encrypt_names,
      volumes: options.volumes, splitMode: "generic", contentPolicy: options.content_policy, excludeText: options.excludes.join("\n"),
      sfxEnabled: options.output.kind === "self_extracting", sfxTarget: options.output.kind === "self_extracting" ? options.output.target : "current_platform",
      sqzInnerFormat: options.format_options.kind === "sqz" ? options.format_options.inner_format : "sqz",
      destinationBase: options.destination.base, suggestedDestination: null,
      completion: options.completion, postSuccess: options.post_success, testAfterCreate: options.test_after_create,
    });
    if (touch) this.touch();
    return change;
  }

  restoreTask(spec: CompressJob, passwordRequired: boolean, context: CreateDraftContext, beforeApply: () => void): CreateDraftChange {
    if (context.locked) return rejected("options_locked");
    const path = spec.dest.toLowerCase();
    let format: CreateFormatId | undefined;
    if (spec.sfx_target) format = "zip";
    else if (path.endsWith(".swm")) format = "wim";
    else format = createFormatIds.find((candidate) =>
      createFormats[candidate].extensions.some((extension) => path.endsWith(`.${extension}`)));
    if (!format) return rejected("task_format_unavailable");
    if (spec.sfx_target && (!context.capabilityReady || !context.sfxCapability.available
      || spec.sfx_target !== context.sfxCapability.target)) {
      return rejected("task_sfx_unavailable");
    }
    beforeApply();
    const change = this.#restore({
      format, customLevel: spec.level, encryptionEnabled: passwordRequired || spec.encrypt_names, encryptNames: spec.encrypt_names,
      volumes: spec.split_size === null ? { kind: "single" } : { kind: "split", size_bytes: String(spec.split_size) },
      splitMode: spec.split_mode, contentPolicy: spec.content_policy, excludeText: spec.excludes.join("\n"),
      sfxEnabled: spec.sfx_target !== null, sfxTarget: spec.sfx_target ?? "current_platform", sqzInnerFormat: spec.sqz_inner_format ?? "sqz",
      destinationBase: "ask", suggestedDestination: spec.dest,
      completion: spec.completion, postSuccess: spec.post_success, testAfterCreate: spec.test_after_create,
    });
    this.touch();
    this.#state.validationAttempted = this.#state.encryptionEnabled;
    return change;
  }

  snapshotPreset(): CreateArchivePresetOptions {
    const state = this.#state;
    const values = this.#snapshotValues();
    return {
      format: state.format, level: values.level, credential: { kind: state.encryptionEnabled ? "prompt" : "none" },
      encrypt_names: state.encryptionEnabled && state.encryptNames,
      volumes: values.splitSize === null ? { kind: "single" } : { kind: "split", size_bytes: String(values.splitSize) },
      content_policy: state.contentPolicy, excludes: values.excludes,
      output: state.sfxEnabled ? { kind: "self_extracting", target: state.sfxTarget } : { kind: "archive" },
      format_options: state.format === "sqz" ? { kind: "sqz", inner_format: state.sqzInnerFormat } : { kind: "none" },
      destination: { base: state.destinationBase, existing_output: state.overwritePolicy },
      completion: state.completion, post_success: state.postSuccess, test_after_create: values.testAfterCreate,
    };
  }

  snapshotRun(context: Readonly<{ sfxCapability: SfxCreateCapabilityDto; defaultCreateDir: string | null; suggestedDestination: string | null }>): CreateRunDraft {
    const state = this.#state;
    const values = this.#snapshotValues();
    const password = createFormats[state.format].can_encrypt_data && state.password ? state.password : null;
    return {
      format: state.format, profile: state.profile, level: values.level, password,
      encryptNames: Boolean(password) && state.encryptNames && createFormats[state.format].can_encrypt_names,
      splitSize: values.splitSize, splitMode: values.splitSize === null ? "generic" : state.splitMode,
      contentPolicy: state.contentPolicy, excludes: values.excludes, sqzInnerFormat: state.format === "sqz" ? state.sqzInnerFormat : null,
      sfxEnabled: state.sfxEnabled, sfxTarget: state.sfxEnabled ? this.#resolveSfxTarget(state.sfxTarget, context.sfxCapability) : null,
      outputExtension: this.outputExtension(context.sfxCapability.extension), suggestedDestination: context.suggestedDestination,
      destination: { base: state.destinationBase, existing_output: state.overwritePolicy },
      completion: state.completion, postSuccess: state.postSuccess, testAfterCreate: values.testAfterCreate,
      defaultCreateDir: context.defaultCreateDir, restoreCredentialPrompt: state.encryptionEnabled, restoreEncryptNames: state.encryptNames,
    };
  }

  #snapshotValues() {
    return { level: this.compressionLevel, splitSize: this.splitSize,
      excludes: this.#state.contentPolicy === "custom" ? this.excludeRules : [], testAfterCreate: this.effectiveTestAfterCreate };
  }
  #clearCredentials(): void {
    Object.assign(this.#state, { password: "", passwordConfirmation: "", passwordVisible: false, encryptNames: false, encryptionEnabled: false });
  }
  #resolveSfxTarget(target: SfxTarget, capability: SfxCreateCapabilityDto): SfxCreateCapabilityDto["target"] {
    return target === "current_platform" ? capability.target : target;
  }
  #restore(options: RestoredOptions): CreateDraftChange {
    const { volumes, ...fields } = options;
    this.#clearCredentials();
    Object.assign(this.#state, fields, { profile: "custom", customLevelInvalid: false,
      overwritePolicy: options.destinationBase === "ask" ? "ask" : "rename", validationAttempted: false });
    this.#restoreVolumes(volumes);
    return this.#normalize(true, true);
  }
  #restoreVolumes(volumes: CreateArchivePresetOptions["volumes"]): void {
    const state = this.#state;
    if (volumes.kind === "single") {
      state.splitPreset = "none";
      state.exactSplitSizeBytes = null;
      return;
    }
    const bytes = Number(volumes.size_bytes);
    const bytesPerMiB = 1024 ** 2;
    const bytesPerGiB = 1024 ** 3;
    state.exactSplitSizeBytes = volumes.size_bytes;
    if (bytes === 25 * bytesPerMiB) state.splitPreset = "25-mib";
    else if (bytes === 100 * bytesPerMiB) state.splitPreset = "100-mib";
    else if (bytes === 700 * bytesPerMiB) state.splitPreset = "700-mib";
    else if (bytes === fat32CompatibleSplitSizeBytes) state.splitPreset = "4-gib";
    else {
      state.splitPreset = "custom";
      state.customSplitUnit = bytes >= bytesPerGiB ? "gib" : "mib";
      state.customSplitAmount = String(Number((bytes / (state.customSplitUnit === "gib" ? bytesPerGiB : bytesPerMiB)).toPrecision(9)));
    }
  }
  #editIssue(edit: CreateDraftEdit, context: CreateDraftContext): CreateDraftIssue | null {
    if (context.locked) return "options_locked";
    const state = this.#state;
    switch (edit.kind) {
      case "format":
        if (state.sfxEnabled && edit.value !== "zip") return "sfx_zip_only";
        if (state.encryptionEnabled && !createFormats[edit.value].can_encrypt_data) return "encryption_format_change";
        if (state.encryptNames && !createFormats[edit.value].can_encrypt_names) return "name_encryption_format_change";
        break;
      case "sfxEnabled":
        if (edit.value && state.encryptNames) return "name_encryption_format_change";
        if (edit.value && !context.capabilityReady) return "sfx_loading";
        if (edit.value && !context.sfxCapability.available) return "sfx_unavailable";
        break;
      case "splitPreset":
        if (state.sfxEnabled && edit.value !== "none") return "sfx_no_split";
        break;
      case "splitMode":
        if (edit.value === "native" && this.nativeSplitKind === null) return "native_layout_unavailable";
        break;
      case "completion":
        if (edit.value === "open_in_squallz" && this.completionRestriction) {
          return this.completionRestriction === "sfx" ? "completion_sfx" : "completion_split";
        }
        break;
      case "postSuccess":
        if (edit.value === "trash_source" && this.trashExcluded) return "trash_excluded";
        break;
      case "testAfterCreate":
        if (state.postSuccess === "trash_source") return "trash_test_required";
        break;
    }
    return null;
  }
  #normalize(postSuccess: boolean, completion: boolean): CreateDraftChange {
    const postSuccessNormalized = postSuccess && this.trashExcluded && this.#state.postSuccess === "trash_source";
    const completionNormalized = completion && this.completionRestriction !== null && this.#state.completion === "open_in_squallz";
    if (postSuccessNormalized) this.#state.postSuccess = "keep_source";
    if (completionNormalized) this.#state.completion = "reveal_output";
    return { changed: true, postSuccessNormalized, completionNormalized };
  }
}
