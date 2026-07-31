<script lang="ts">
  import Icon from "./Icon.svelte";
  import type { CreateSplitMode, CreateSplitPreset, CreateSplitUnit } from "../lib/ui-model";

  let {
    variant,
    classicSplitSectionId,
    password,
    passwordConfirmation,
    passwordVisible,
    encryptNames,
    canEncryptData,
    canEncryptNames,
    splitDisabled,
    splitPreset,
    splitMode,
    nativeSplitKind,
    customSplitAmount,
    customSplitUnit,
    passwordCapability,
    nameEncryptionCapability,
    splitCapability,
    splitSummary,
    passwordError,
    splitError,
    passwordTitle = "",
    splitTitle = "",
    disabled = false,
    disabledReason = "",
    tr,
    onPasswordInput,
    onPasswordConfirmationInput,
    onPasswordVisibleChange,
    onEncryptNamesChange,
    onSplitPresetChange,
    onSplitModeChange,
    onCustomSplitAmountInput,
    onCustomSplitUnitChange,
  }: {
    variant: "modern" | "classic";
    classicSplitSectionId?: string;
    password: string;
    passwordConfirmation: string;
    passwordVisible: boolean;
    encryptNames: boolean;
    canEncryptData: boolean;
    canEncryptNames: boolean;
    splitDisabled: boolean;
    splitPreset: CreateSplitPreset;
    splitMode: CreateSplitMode;
    nativeSplitKind: "zip" | "wim" | null;
    customSplitAmount: string;
    customSplitUnit: CreateSplitUnit;
    passwordCapability: string;
    nameEncryptionCapability: string;
    splitCapability: string;
    splitSummary: string;
    passwordError: string;
    splitError: string;
    passwordTitle?: string;
    splitTitle?: string;
    disabled?: boolean;
    disabledReason?: string;
    tr: (key: string, fallback: string) => string;
    onPasswordInput: (value: string) => void;
    onPasswordConfirmationInput: (value: string) => void;
    onPasswordVisibleChange: (visible: boolean) => void;
    onEncryptNamesChange: (enabled: boolean) => void;
    onSplitPresetChange: (preset: CreateSplitPreset) => void;
    onSplitModeChange: (mode: CreateSplitMode) => void;
    onCustomSplitAmountInput: (value: string) => void;
    onCustomSplitUnitChange: (unit: CreateSplitUnit) => void;
  } = $props();

  let passwordErrorId = $derived(`${variant}-create-password-error`);
  let splitErrorId = $derived(`${variant}-create-split-error`);

  function passwordType(): "text" | "password" {
    return passwordVisible ? "text" : "password";
  }

  function nativeSplitLabel(): string {
    if (nativeSplitKind === "zip") {
      return tr("gui.create.volume_layout_native_zip", "Native ZIP · .z01 … .zip");
    }
    if (nativeSplitKind === "wim") {
      return tr("gui.create.volume_layout_native_wim", "Native Split WIM · .swm, 2.swm, …");
    }
    return tr("gui.create.volume_layout_native_unavailable", "Native · ZIP or WIM only");
  }

  function nativeSplitWarning(): string {
    if (nativeSplitKind === "wim") {
      return tr(
        "gui.create.native_wim_parts_required",
        "Keep every .swm part together and open the first .swm. A single large file can make one part exceed the target size.",
      );
    }
    return tr("gui.create.native_zip_parts_required", "Keep every .zNN part with the final .zip file; open the .zip file.");
  }
</script>

{#snippet passwordFields()}
  {#if canEncryptData}
    <label class="create-option-field">
      <span>{tr("gui.create.password", "Password")}</span>
      <div class="secure-input-row">
        <input
          class="create-option-input"
          type={passwordType()}
          autocomplete="new-password"
          value={password}
          disabled={disabled}
          title={disabledReason}
          aria-label={disabledReason ? `${tr("gui.create.archive_password", "Archive password")} · ${disabledReason}` : tr("gui.create.archive_password", "Archive password")}
          aria-invalid={passwordError ? "true" : "false"}
          aria-describedby={passwordError ? passwordErrorId : undefined}
          oninput={(event) => onPasswordInput((event.currentTarget as HTMLInputElement).value)}
        />
        <button
          class="secure-input-action"
          type="button"
          disabled={disabled}
          title={disabledReason}
          aria-label={passwordVisible ? tr("gui.create.hide_password", "Hide password") : tr("gui.create.show_password", "Show password")}
          aria-pressed={passwordVisible}
          onclick={() => onPasswordVisibleChange(!passwordVisible)}
        >{passwordVisible ? tr("common.hide", "Hide") : tr("common.show", "Show")}</button>
      </div>
    </label>
    <label class="create-option-field">
      <span>{tr("gui.create.confirm_password", "Confirm password")}</span>
      <input
        class="create-option-input"
        type={passwordType()}
        autocomplete="new-password"
        value={passwordConfirmation}
        disabled={disabled}
        title={disabledReason}
        aria-label={disabledReason ? `${tr("gui.create.confirm_archive_password", "Confirm archive password")} · ${disabledReason}` : tr("gui.create.confirm_archive_password", "Confirm archive password")}
        aria-invalid={passwordError ? "true" : "false"}
        aria-describedby={passwordError ? passwordErrorId : undefined}
        oninput={(event) => onPasswordConfirmationInput((event.currentTarget as HTMLInputElement).value)}
      />
    </label>
    {#if passwordError}
      <small id={passwordErrorId} class="create-option-error" role="status">{passwordError}</small>
    {/if}
    <label class="create-option-check" class:disabled={disabled || !canEncryptNames || password.length === 0} title={disabledReason}>
      <input
        type="checkbox"
        checked={encryptNames}
        disabled={disabled || !canEncryptNames || password.length === 0}
        onchange={(event) => onEncryptNamesChange((event.currentTarget as HTMLInputElement).checked)}
      />
      <span>{tr("gui.create.encrypt_file_names", "Encrypt file names")} · {nameEncryptionCapability}</span>
    </label>
    <small class="create-option-help">{passwordCapability}</small>
  {:else}
    <p class="create-option-capability">{passwordCapability}</p>
  {/if}
{/snippet}

{#snippet splitFields()}
  <label class="create-option-field">
    <span>{tr("gui.create.part_size", "Part size")}</span>
    <select
      value={splitPreset}
      disabled={disabled || splitDisabled}
      title={disabledReason}
      aria-label={disabledReason ? `${tr("gui.create.split_volume_size", "Split volume size")} · ${disabledReason}` : tr("gui.create.split_volume_size", "Split volume size")}
      onchange={(event) => onSplitPresetChange((event.currentTarget as HTMLSelectElement).value as CreateSplitPreset)}
    >
      <option value="none">{tr("gui.create.single_archive", "Single archive")}</option>
      <option value="25-mib">25 MiB</option>
      <option value="100-mib">100 MiB</option>
      <option value="700-mib">700 MiB</option>
      <option value="4-gib">{tr("gui.compress.split.fat32", "FAT32 limit (4 GiB − 1 B)")}</option>
      <option value="custom">{tr("common.custom", "Custom")}</option>
    </select>
  </label>
  {#if splitPreset === "custom"}
    <div class="custom-split-row">
      <label class="create-option-field">
        <span>{tr("gui.create.custom_part_size", "Custom part size")}</span>
        <input
          type="number"
          min="0.1"
          step="0.1"
          inputmode="decimal"
          value={customSplitAmount}
          disabled={disabled || splitDisabled}
          title={disabledReason}
          aria-label={disabledReason ? `${tr("gui.create.custom_part_size", "Custom part size")} · ${disabledReason}` : tr("gui.create.custom_part_size", "Custom part size")}
          aria-invalid={splitError ? "true" : "false"}
          aria-describedby={splitError ? splitErrorId : undefined}
          oninput={(event) => onCustomSplitAmountInput((event.currentTarget as HTMLInputElement).value)}
        />
      </label>
      <label class="create-option-field create-option-unit">
        <span>{tr("common.unit", "Unit")}</span>
        <select
          value={customSplitUnit}
          disabled={disabled || splitDisabled}
          title={disabledReason}
          aria-label={disabledReason ? `${tr("gui.create.custom_part_size_unit", "Custom part size unit")} · ${disabledReason}` : tr("gui.create.custom_part_size_unit", "Custom part size unit")}
          onchange={(event) => onCustomSplitUnitChange((event.currentTarget as HTMLSelectElement).value as CreateSplitUnit)}
        >
          <option value="mib">MiB</option>
          <option value="gib">GiB</option>
        </select>
      </label>
    </div>
  {/if}
  {#if splitPreset !== "none"}
    <label class="create-option-field">
      <span>{tr("gui.create.volume_layout", "Volume layout")}</span>
      <select
        value={splitMode}
        disabled={disabled || splitDisabled}
        title={disabledReason}
        aria-label={disabledReason ? `${tr("gui.create.volume_layout_aria", "Split volume layout")} · ${disabledReason}` : tr("gui.create.volume_layout_aria", "Split volume layout")}
        onchange={(event) => onSplitModeChange((event.currentTarget as HTMLSelectElement).value as CreateSplitMode)}
      >
        <option value="generic">{tr("gui.create.volume_layout_generic", "Generic · .001, .002, …")}</option>
        <option value="native" disabled={nativeSplitKind === null}>
          {nativeSplitLabel()}
        </option>
      </select>
    </label>
  {/if}
  {#if splitError}
    <small id={splitErrorId} class="create-option-error" role="status">{splitError}</small>
  {/if}
  <div class="volume-preview">{splitSummary}</div>
  <small class="create-option-help">{splitCapability}</small>
  {#if splitPreset !== "none"}
    <small class="create-option-warning">
      {splitMode === "native"
        ? nativeSplitWarning()
        : tr("gui.create.generic_parts_required", "Keep every numbered .001 part in the same folder; open the .001 file.")}
    </small>
  {/if}
{/snippet}

{#if variant === "modern"}
  <section class="create-option-card">
    <h2><Icon name="lock" size={16} />{passwordTitle || tr("gui.create.password", "Password")}</h2>
    {@render passwordFields()}
  </section>
  <section class="create-option-card">
    <h2><Icon name="panel-top" size={16} />{splitTitle || tr("gui.create.split_volumes", "Split volumes")}</h2>
    {@render splitFields()}
  </section>
{:else}
  <div class="classic-label">{passwordTitle || tr("gui.create.password", "Password")}</div>
  <div class="classic-input create-option-classic">{@render passwordFields()}</div>
  <div class="classic-label">{splitTitle || tr("gui.create.split_to_volumes", "Split to volumes")}</div>
  <div id={classicSplitSectionId} class="classic-input create-option-classic classic-create-section-target">{@render splitFields()}</div>
{/if}
