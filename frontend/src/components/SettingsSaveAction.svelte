<script lang="ts">
  import Icon from "./Icon.svelte";

  type SaveState = "saved" | "dirty" | "saving" | "session" | "error";

  let {
    state,
    statusLabel,
    actionLabel,
    savingLabel,
    disabledReason = "",
    icon = "check-circle",
    onSave,
  }: {
    state: SaveState;
    statusLabel: string;
    actionLabel: string;
    savingLabel: string;
    disabledReason?: string;
    icon?: string;
    onSave: () => void;
  } = $props();

  let saving = $derived(state === "saving");
  let disabled = $derived(Boolean(disabledReason) || saving || state === "saved");
  let statusIcon = $derived(
    state === "error"
      ? "x-circle"
      : state === "saving"
        ? "hourglass"
        : state === "dirty" || state === "session"
          ? "info"
          : "check-circle",
  );
</script>

<div class="settings-save-cluster">
  <div class={`settings-save-state state-${state}`} role="status" aria-live="polite">
    <Icon name={statusIcon} size={15} />
    <span>{statusLabel}</span>
  </div>
  <button
    class="primary sheet-action"
    disabled={disabled}
    aria-busy={saving}
    aria-label={saving ? savingLabel : disabledReason ? `${actionLabel} · ${disabledReason}` : actionLabel}
    title={disabledReason}
    onclick={onSave}
  ><Icon name={icon} size={17} />{saving ? savingLabel : actionLabel}</button>
</div>
