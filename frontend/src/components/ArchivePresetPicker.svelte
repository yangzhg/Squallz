<script lang="ts">
  import Icon from "./Icon.svelte";

  type ArchivePresetOption = {
    id: string;
    name: string;
    summary: string;
  };

  type ArchivePresetStatus = "idle" | "applied" | "modified" | "saving" | "error";

  let {
    instanceId,
    variant,
    compact = false,
    kind,
    options,
    selectedId,
    draftName,
    summary,
    status,
    statusLabel,
    disabledReason = "",
    updateDisabledReason = "",
    deleteDisabledReason = "",
    fileManagerDisabledReason = "",
    isDefault,
    isFileManagerDefault,
    tr,
    onSelect,
    onDraftNameInput,
    onUpdate,
    onSaveAs,
    onDelete,
    onDefaultChange,
    onFileManagerDefaultChange,
  }: {
    instanceId: string;
    variant: "modern" | "classic";
    compact?: boolean;
    kind: "create" | "extract";
    options: ArchivePresetOption[];
    selectedId: string | null;
    draftName: string;
    summary: string;
    status: ArchivePresetStatus;
    statusLabel: string;
    disabledReason?: string;
    updateDisabledReason?: string;
    deleteDisabledReason?: string;
    fileManagerDisabledReason?: string;
    isDefault: boolean;
    isFileManagerDefault: boolean;
    tr: (key: string, fallback: string) => string;
    onSelect: (id: string | null) => void;
    onDraftNameInput: (name: string) => void;
    onUpdate: () => void;
    onSaveAs: () => void;
    onDelete: () => void;
    onDefaultChange: (enabled: boolean) => void;
    onFileManagerDefaultChange: (enabled: boolean) => void;
  } = $props();

  let manageOpen = $state(false);
  let deleteArmed = $state(false);
  let nameInput = $state<HTMLInputElement | null>(null);
  let selectId = $derived(`${instanceId}-preset-select`);
  let nameId = $derived(`${instanceId}-preset-name`);
  let disabledReasonId = $derived(`${instanceId}-preset-disabled-reason`);
  let updateReasonId = $derived(`${instanceId}-preset-update-reason`);
  let deleteReasonId = $derived(`${instanceId}-preset-delete-reason`);
  let fileManagerReasonId = $derived(`${instanceId}-preset-file-manager-reason`);
  let interactionDisabled = $derived(status === "saving" || Boolean(disabledReason));
  let statusIcon = $derived(
    status === "error"
      ? "x-circle"
      : status === "saving"
        ? "hourglass"
        : status === "modified"
          ? "info"
          : "check-circle",
  );

  function toggleManagement(focusName = false) {
    manageOpen = focusName || !manageOpen;
    deleteArmed = false;
    if (focusName) requestAnimationFrame(() => nameInput?.focus());
  }

  function selectPreset(event: Event) {
    const value = (event.currentTarget as HTMLSelectElement).value;
    deleteArmed = false;
    onSelect(value || null);
  }

  function requestDelete() {
    if (!deleteArmed) {
      deleteArmed = true;
      return;
    }
    deleteArmed = false;
    onDelete();
  }
</script>

<section
  class:classic={variant === "classic"}
  class:compact
  class="archive-preset-picker"
  aria-labelledby={`${instanceId}-preset-title`}
>
  <div class="archive-preset-heading">
    <div>
      <span id={`${instanceId}-preset-title`} class="archive-preset-kicker">
        {kind === "create"
          ? tr("gui.presets.create_title", "Create preset")
          : tr("gui.presets.extract_title", "Extract preset")}
      </span>
      <strong>{tr("gui.presets.heading", "Saved setup")}</strong>
    </div>
    <div class={`archive-preset-status status-${status}`} role="status" aria-live="polite">
      <Icon name={statusIcon} size={14} />
      <span>{statusLabel}</span>
    </div>
  </div>

  <div class="archive-preset-toolbar">
    <label for={selectId}>{tr("gui.presets.choose", "Preset")}</label>
    <select
      id={selectId}
      value={selectedId ?? ""}
      disabled={interactionDisabled}
      title={disabledReason}
      aria-describedby={disabledReason ? disabledReasonId : undefined}
      onchange={selectPreset}
    >
      <option value="">{tr("gui.presets.none", "Current settings")}</option>
      {#each options as option (option.id)}
        <option value={option.id}>{option.name}</option>
      {/each}
    </select>
    <button
      type="button"
      disabled={interactionDisabled}
      title={disabledReason}
      aria-describedby={disabledReason ? disabledReasonId : undefined}
      onclick={() => toggleManagement(true)}
    >
      <Icon name="archive" size={14} />
      {tr("gui.presets.save_current", "Save as preset")}
    </button>
    <button
      type="button"
      aria-expanded={manageOpen}
      disabled={interactionDisabled}
      title={disabledReason}
      aria-describedby={disabledReason ? disabledReasonId : undefined}
      onclick={() => toggleManagement()}
    >
      <Icon name="settings" size={14} />
      {manageOpen ? tr("common.done", "Done") : tr("gui.presets.manage", "Manage")}
    </button>
  </div>

  <p class="archive-preset-summary">{summary}</p>
  {#if disabledReason}
    <p id={disabledReasonId} class="archive-preset-guidance archive-preset-disabled-guidance" role="note">{disabledReason}</p>
  {/if}

  {#if manageOpen}
    <div class="archive-preset-manager">
      <label class="archive-preset-name" for={nameId}>
        <span>{tr("common.name", "Name")}</span>
        <input
          bind:this={nameInput}
          id={nameId}
          maxlength="40"
          value={draftName}
          disabled={interactionDisabled}
          autocomplete="off"
          oninput={(event) => onDraftNameInput((event.currentTarget as HTMLInputElement).value)}
        />
      </label>

      <div class="archive-preset-bindings">
        <label>
          <input
            type="checkbox"
            checked={Boolean(selectedId) && isDefault}
            disabled={!selectedId || interactionDisabled}
            onchange={(event) => onDefaultChange((event.currentTarget as HTMLInputElement).checked)}
          />
          <span>{tr("gui.presets.app_default", "Use by default in Squallz")}</span>
        </label>
        <label class:disabled={Boolean(fileManagerDisabledReason)} title={fileManagerDisabledReason}>
          <input
            type="checkbox"
            checked={Boolean(selectedId) && isFileManagerDefault}
            disabled={!selectedId || interactionDisabled || (Boolean(fileManagerDisabledReason) && !isFileManagerDefault)}
            aria-describedby={fileManagerDisabledReason ? fileManagerReasonId : undefined}
            onchange={(event) => onFileManagerDefaultChange((event.currentTarget as HTMLInputElement).checked)}
          />
          <span>
            {kind === "create"
              ? tr("gui.presets.file_manager_create", "Use for file-manager compression")
              : tr("gui.presets.file_manager_extract", "Use conflict, link, and encoding policies for file-manager extraction")}
          </span>
        </label>
      </div>

      <div class="archive-preset-manager-actions">
        <button
          type="button"
          disabled={!selectedId || interactionDisabled || Boolean(updateDisabledReason)}
          title={updateDisabledReason}
          aria-describedby={updateDisabledReason ? updateReasonId : undefined}
          onclick={onUpdate}
        >{tr("gui.presets.update", "Update preset")}</button>
        <button
          type="button"
          class="archive-preset-save"
          disabled={interactionDisabled || !draftName.trim()}
          onclick={onSaveAs}
        >{tr("gui.presets.save_as_new", "Save as new")}</button>
        <button
          type="button"
          class:armed={deleteArmed}
          disabled={!selectedId || interactionDisabled || Boolean(deleteDisabledReason)}
          title={deleteDisabledReason}
          aria-describedby={deleteDisabledReason ? deleteReasonId : undefined}
          onclick={requestDelete}
        >{deleteArmed ? tr("gui.presets.confirm_delete", "Delete this preset") : tr("common.delete", "Delete")}</button>
      </div>

      {#if updateDisabledReason || deleteDisabledReason || fileManagerDisabledReason}
        <div class="archive-preset-guidance" role="note">
          {#if updateDisabledReason}<p id={updateReasonId}>{updateDisabledReason}</p>{/if}
          {#if deleteDisabledReason}<p id={deleteReasonId}>{deleteDisabledReason}</p>{/if}
          {#if fileManagerDisabledReason}<p id={fileManagerReasonId}>{fileManagerDisabledReason}</p>{/if}
        </div>
      {/if}
    </div>
  {/if}
</section>
