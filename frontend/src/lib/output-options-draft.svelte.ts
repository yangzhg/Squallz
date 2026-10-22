import { fat32CompatibleSplitSizeBytes, resolveSplitSizeBytes } from "./archive-output-options";
import {
  createFormats, createProfiles,
  type CreateFormatId, type CreateProfileId, type CreateSplitMode, type CreateSplitPreset, type CreateSplitUnit,
} from "./ui-model";

export type OutputOptionsState = {
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
  validationAttempted: boolean;
};

export type OutputOptionEdit = {
  [K in Exclude<keyof OutputOptionsState, "customLevelInvalid" | "exactSplitSizeBytes" | "validationAttempted">]:
    { kind: K; value: K extends "customLevel" ? number | string : OutputOptionsState[K] }
}[Exclude<keyof OutputOptionsState, "customLevelInvalid" | "exactSplitSizeBytes" | "validationAttempted">];

export type OutputOptionIssue = "encryption_format_change" | "name_encryption_format_change"
  | "native_layout_unavailable" | "custom_level_invalid" | "password_required"
  | "confirm_password_required" | "passwords_do_not_match" | "invalid_part_size" | "native_zip_part_size_limit";

export const clampCustomLevel = (value: number): number => Number.isFinite(value) ? Math.min(9, Math.max(1, Math.round(value))) : 6;

/** Shared output draft; source credentials and operation lifecycles belong to the caller. */
export class OutputOptionsDraft<Extra extends object = object> {
  readonly state: OutputOptionsState & Extra;

  constructor(initial: Partial<OutputOptionsState> & Extra) {
    this.state = $state({
      format: "7z", profile: "balanced", customLevel: 6, customLevelInvalid: false,
      password: "", passwordConfirmation: "", passwordVisible: false, encryptionEnabled: false, encryptNames: false,
      splitPreset: "none", splitMode: "generic", customSplitAmount: "100", customSplitUnit: "mib", exactSplitSizeBytes: null,
      validationAttempted: false, ...initial,
    } as OutputOptionsState & Extra);
  }

  get compressionLevel(): number {
    return this.state.profile === "custom" ? this.state.customLevel : createProfiles[this.state.profile].level;
  }
  get splitSize(): number | null {
    const state = this.state;
    return resolveSplitSizeBytes(state.splitPreset, state.customSplitAmount, state.customSplitUnit, state.exactSplitSizeBytes);
  }
  get nativeSplitKind(): "zip" | "wim" | null {
    const format = this.state.format;
    return format === "zip" || format === "wim" ? format : null;
  }
  get passwordIssue(): OutputOptionIssue | null {
    const state = this.state;
    if (!createFormats[state.format].can_encrypt_data) return null;
    if (state.encryptionEnabled && !state.password) return "password_required";
    if (!state.password) return null;
    if (!state.passwordConfirmation) return "confirm_password_required";
    return state.password === state.passwordConfirmation ? null : "passwords_do_not_match";
  }
  get splitIssue(): OutputOptionIssue | null {
    const size = this.splitSize;
    if (this.state.splitPreset === "custom" && size === null) return "invalid_part_size";
    return this.state.format === "zip" && this.state.splitMode === "native" && size !== null && size > fat32CompatibleSplitSizeBytes
      ? "native_zip_part_size_limit" : null;
  }
  get outputExtension(): string {
    return this.state.format === "wim" && this.splitSize !== null && this.state.splitMode === "native"
      ? "swm" : createFormats[this.state.format].extension;
  }

  formatIssue(format: CreateFormatId): OutputOptionIssue | null {
    if (this.state.encryptionEnabled && !createFormats[format].can_encrypt_data) return "encryption_format_change";
    return this.state.encryptNames && !createFormats[format].can_encrypt_names ? "name_encryption_format_change" : null;
  }

  validateLevel(value: number | string): boolean {
    const valid = typeof value === "number" || (value.trim() !== "" && Number.isInteger(Number(value)) && Number(value) >= 1 && Number(value) <= 9);
    if (!valid) this.state.customLevelInvalid = true;
    return valid;
  }

  edit(edit: OutputOptionEdit): void {
    const state = this.state;
    switch (edit.kind) {
      case "format":
        state.format = edit.value;
        if (this.nativeSplitKind === null) state.splitMode = "generic";
        if (!createFormats[edit.value].can_encrypt_data) this.clearCredentials();
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
      case "passwordVisible": state.passwordVisible = edit.value; break;
      case "encryptionEnabled":
        state.encryptionEnabled = edit.value && createFormats[state.format].can_encrypt_data;
        if (!state.encryptionEnabled) this.clearCredentials();
        state.validationAttempted = state.encryptionEnabled;
        break;
      case "encryptNames": state.encryptNames = edit.value && createFormats[state.format].can_encrypt_names && state.encryptionEnabled; break;
      case "splitPreset":
        state.splitPreset = edit.value;
        state.exactSplitSizeBytes = null;
        if (edit.value === "none") state.splitMode = "generic";
        state.validationAttempted = false;
        break;
      case "splitMode": state.splitMode = edit.value; state.validationAttempted = false; break;
      case "customSplitAmount": case "customSplitUnit":
        if (edit.kind === "customSplitAmount") state.customSplitAmount = edit.value;
        else state.customSplitUnit = edit.value;
        state.exactSplitSizeBytes = null;
        state.validationAttempted = false;
        break;
    }
  }

  clearPassword(): void {
    Object.assign(this.state, { password: "", passwordConfirmation: "", passwordVisible: false });
  }
  clearCredentials(): void {
    this.clearPassword();
    this.state.encryptNames = false;
    this.state.encryptionEnabled = false;
  }
  resetProtection(): void {
    this.clearCredentials();
    Object.assign(this.state, { splitPreset: "none", splitMode: "generic", customSplitAmount: "100",
      customSplitUnit: "mib", exactSplitSizeBytes: null, validationAttempted: false });
  }
  restoreSplitSize(sizeBytes: string | null, recognizePreset: boolean): void {
    const state = this.state;
    state.exactSplitSizeBytes = sizeBytes;
    state.splitPreset = "none";
    if (sizeBytes === null) return;
    state.splitPreset = "custom";
    if (recognizePreset) {
      const presets: CreateSplitPreset[] = ["25-mib", "100-mib", "700-mib", "4-gib"];
      const preset = presets.find((preset) => resolveSplitSizeBytes(preset, "", "mib") === Number(sizeBytes));
      if (preset) { state.splitPreset = preset; return; }
    }
    const bytes = Number(sizeBytes);
    state.customSplitUnit = bytes >= 1024 ** 3 ? "gib" : "mib";
    state.customSplitAmount = String(Number((bytes / (state.customSplitUnit === "gib" ? 1024 ** 3 : 1024 ** 2)).toPrecision(9)));
  }
}
