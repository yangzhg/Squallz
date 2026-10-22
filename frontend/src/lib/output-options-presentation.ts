import type { OutputOptionEdit, OutputOptionsState } from "./output-options-draft.svelte";
import { createFormats, type CreateSplitMode, type CreateSplitPreset, type CreateSplitUnit } from "./ui-model";

/** Both output screens bind the same controls; their sessions still admit every edit. */
export function outputProtectionControls(
  state: Readonly<OutputOptionsState>,
  edit: (edit: OutputOptionEdit) => void,
  tr: (key: string, fallback?: string) => string,
  passwordError: string,
  splitError: string,
) {
  const format = createFormats[state.format];
  return {
    password: state.password,
    passwordConfirmation: state.passwordConfirmation,
    passwordVisible: state.passwordVisible,
    encryptionEnabled: state.encryptionEnabled,
    encryptNames: state.encryptNames,
    canEncryptData: format.can_encrypt_data,
    canEncryptNames: format.can_encrypt_names,
    splitPreset: state.splitPreset,
    splitMode: state.splitMode,
    customSplitAmount: state.customSplitAmount,
    customSplitUnit: state.customSplitUnit,
    passwordCapability: tr(`gui.create.format.${state.format}.password`, format.password),
    nameEncryptionCapability: format.can_encrypt_names
      ? tr("gui.create.name_encryption_available")
      : state.format === "zip"
        ? tr("gui.create.name_encryption_zip_visible", "ZIP names stay visible; use 7Z")
        : tr("gui.create.name_encryption_unavailable"),
    passwordError: state.validationAttempted || state.passwordConfirmation.length > 0
      || (state.encryptionEnabled && state.password.length === 0) ? passwordError : "",
    splitError: state.validationAttempted || state.customSplitAmount.length > 0 ? splitError : "",
    onPasswordInput: (value: string) => edit({ kind: "password", value }),
    onPasswordConfirmationInput: (value: string) => edit({ kind: "passwordConfirmation", value }),
    onPasswordVisibleChange: (value: boolean) => edit({ kind: "passwordVisible", value }),
    onEncryptionEnabledChange: (value: boolean) => edit({ kind: "encryptionEnabled", value }),
    onEncryptNamesChange: (value: boolean) => edit({ kind: "encryptNames", value }),
    onSplitPresetChange: (value: CreateSplitPreset) => edit({ kind: "splitPreset", value }),
    onSplitModeChange: (value: CreateSplitMode) => edit({ kind: "splitMode", value }),
    onCustomSplitAmountInput: (value: string) => edit({ kind: "customSplitAmount", value }),
    onCustomSplitUnitChange: (value: CreateSplitUnit) => edit({ kind: "customSplitUnit", value }),
  };
}
