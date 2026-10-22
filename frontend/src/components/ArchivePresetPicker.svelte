<script lang="ts">
  import Icon from "./Icon.svelte";
  import { tFallback as tr } from "../lib/i18n.svelte";
  import { archivePresetDisplayName, type ArchivePresetSession, type PresetKind } from "../lib/archive-presets.svelte";
  import { createArchivePresetSummary, extractArchivePresetSummary } from "../lib/archive-option-labels";

  let { instanceId, variant, compact = false, kind, session }: {
    instanceId: string;
    variant: "modern" | "classic";
    compact?: boolean;
    kind: PresetKind;
    session: ArchivePresetSession;
  } = $props();

  let selectedId = $derived(session.selection(kind).id);
  let draftName = $derived(session.selection(kind).name);
  let options = $derived(session.options(kind).map((preset) => ({
    id: preset.id, name: archivePresetDisplayName(preset),
    summary: preset.kind === "create" ? createArchivePresetSummary(preset.options) : extractArchivePresetSummary(preset.options),
  })));
  let summary = $derived(session.summary(kind));
  let status = $derived(session.status(kind));
  let statusLabel = $derived(session.statusLabel(kind));
  let disabledReason = $derived(session.disabledReason(kind));
  let updateDisabledReason = $derived(session.updateDisabledReason(kind));
  let deleteDisabledReason = $derived(session.readOnlyReason(kind));
  let fileManagerDisabledReason = $derived(kind === "create" ? session.finderDisabledReason() : "");
  let isDefault = $derived(session.isBound(kind, "app"));
  let isFileManagerDefault = $derived(session.isBound(kind, "file_manager"));

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
    const select = event.currentTarget as HTMLSelectElement;
    const value = select.value;
    // Keep the visible selection when the parent cannot apply the requested preset.
    select.value = selectedId ?? "";
    deleteArmed = false;
    session.apply(kind, value || null);
  }

  function requestDelete() {
    if (!deleteArmed) {
      deleteArmed = true;
      return;
    }
    deleteArmed = false;
    void session.remove(kind);
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
          ? tr("gui.presets.create_title")
          : tr("gui.presets.extract_title")}
      </span>
      <strong>{tr("gui.presets.heading")}</strong>
    </div>
    <div class={`archive-preset-status status-${status}`} role="status" aria-live="polite">
      <Icon name={statusIcon} size={14} />
      <span>{statusLabel}</span>
    </div>
  </div>

  <div class="archive-preset-toolbar">
    <label for={selectId}>{tr("gui.presets.choose")}</label>
    <select
      id={selectId}
      value={selectedId ?? ""}
      disabled={interactionDisabled}
      title={disabledReason}
      aria-describedby={disabledReason ? disabledReasonId : undefined}
      onchange={selectPreset}
    >
      <option value="">{tr("gui.presets.none")}</option>
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
      {tr("gui.presets.save_current")}
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
      {manageOpen ? tr("common.done", "Done") : tr("gui.presets.manage")}
    </button>
  </div>

  <p class="archive-preset-summary">{summary}</p>
  {#if disabledReason}
    <p id={disabledReasonId} class="archive-preset-guidance archive-preset-disabled-guidance" role="note">{disabledReason}</p>
  {/if}

  {#if manageOpen}
    <div class="archive-preset-manager">
      <label class="archive-preset-name" for={nameId}>
        <span>{tr("common.name")}</span>
        <input
          bind:this={nameInput}
          id={nameId}
          maxlength="40"
          value={draftName}
          disabled={interactionDisabled}
          autocomplete="off"
          oninput={(event) => session.editName(kind, (event.currentTarget as HTMLInputElement).value)}
        />
      </label>

      <div class="archive-preset-bindings">
        <label>
          <input
            type="checkbox"
            checked={Boolean(selectedId) && isDefault}
            disabled={!selectedId || interactionDisabled}
            onchange={(event) => void session.bind(kind, "app", (event.currentTarget as HTMLInputElement).checked)}
          />
          <span>{tr("gui.presets.app_default")}</span>
        </label>
        <label class:disabled={Boolean(fileManagerDisabledReason)} title={fileManagerDisabledReason}>
          <input
            type="checkbox"
            checked={Boolean(selectedId) && isFileManagerDefault}
            disabled={!selectedId || interactionDisabled || (Boolean(fileManagerDisabledReason) && !isFileManagerDefault)}
            aria-describedby={fileManagerDisabledReason ? fileManagerReasonId : undefined}
            onchange={(event) => void session.bind(kind, "file_manager", (event.currentTarget as HTMLInputElement).checked)}
          />
          <span>
            {kind === "create"
              ? tr("gui.presets.file_manager_create")
              : tr("gui.presets.file_manager_extract")}
          </span>
        </label>
      </div>

      <div class="archive-preset-manager-actions">
        <button
          type="button"
          disabled={!selectedId || interactionDisabled || Boolean(updateDisabledReason)}
          title={updateDisabledReason}
          aria-describedby={updateDisabledReason ? updateReasonId : undefined}
          onclick={() => void session.save(kind, "update")}
        >{tr("gui.presets.update")}</button>
        <button
          type="button"
          class="archive-preset-save"
          disabled={interactionDisabled || !draftName.trim()}
          onclick={() => void session.save(kind, "save_as")}
        >{tr("gui.presets.save_as_new")}</button>
        <button
          type="button"
          class:armed={deleteArmed}
          disabled={!selectedId || interactionDisabled || Boolean(deleteDisabledReason)}
          title={deleteDisabledReason}
          aria-describedby={deleteDisabledReason
            ? deleteDisabledReason === updateDisabledReason ? updateReasonId : deleteReasonId
            : undefined}
          onclick={requestDelete}
        >{deleteArmed ? tr("gui.presets.confirm_delete") : tr("common.delete")}</button>
      </div>

      {#if updateDisabledReason || deleteDisabledReason || fileManagerDisabledReason}
        <div class="archive-preset-guidance" role="note">
          {#if updateDisabledReason}<p id={updateReasonId}>{updateDisabledReason}</p>{/if}
          {#if deleteDisabledReason && deleteDisabledReason !== updateDisabledReason}<p id={deleteReasonId}>{deleteDisabledReason}</p>{/if}
          {#if fileManagerDisabledReason}<p id={fileManagerReasonId}>{fileManagerDisabledReason}</p>{/if}
        </div>
      {/if}
    </div>
  {/if}
</section>
