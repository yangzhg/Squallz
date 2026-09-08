<script lang="ts">
  import Icon from "./Icon.svelte";

  let {
    variant,
    classicSectionId,
    enabled,
    available,
    targetLabel,
    outputLabel,
    summary,
    signingWarning,
    unavailableMessage,
    disabled = false,
    disabledReason = "",
    loading = false,
    tr,
    onEnabledChange,
  }: {
    variant: "modern" | "classic";
    classicSectionId?: string;
    enabled: boolean;
    available: boolean;
    targetLabel: string;
    outputLabel: string;
    summary: string;
    signingWarning: string;
    unavailableMessage: string;
    disabled?: boolean;
    disabledReason?: string;
    loading?: boolean;
    tr: (key: string, fallback: string) => string;
    onEnabledChange: (enabled: boolean) => void;
  } = $props();
</script>

{#snippet fields()}
  {@const toggleLabel = tr("gui.create.sfx_toggle", "Create self-extracting file")}
  <label class="create-option-check" class:disabled={!available || disabled} title={disabledReason}>
    <input
      type="checkbox"
      checked={enabled}
      disabled={!available || disabled}
      aria-describedby={`${variant}-create-sfx-detail`}
      aria-label={disabledReason ? `${toggleLabel} · ${disabledReason}` : toggleLabel}
      onchange={(event) => onEnabledChange((event.currentTarget as HTMLInputElement).checked)}
    />
    <span>{toggleLabel}</span>
  </label>
  {#if !loading}
    <div
      class="create-sfx-target"
      aria-label={`${tr("gui.create.sfx_target_platform", "Target platform")}: ${targetLabel}`}
    >
      <Icon name="panel-top" size={16} />
      <div>
        <span>{tr("gui.create.sfx_target_platform", "Target platform")}</span>
        <strong>{targetLabel} · {outputLabel}</strong>
        <small>{tr(
          "gui.create.sfx_current_runtime_note",
          "Uses this platform's bundled runtime. Build each desktop target on that platform.",
        )}</small>
      </div>
    </div>
  {/if}
  <div id={`${variant}-create-sfx-detail`} class="volume-preview">{summary}</div>
  {#if loading}
    <small class="create-option-help" role="status">{disabledReason}</small>
  {:else if enabled}
    <p class="create-output-warning create-sfx-signing-warning" role="status" aria-live="polite">
      <Icon name="shield-alert" size={16} />
      <span>
        <strong>{tr("gui.create.sfx_unsigned_heading", "Unsigned executable")}</strong>
        <span>{signingWarning}</span>
      </span>
    </p>
  {:else if !available}
    <small class="create-option-error" role="status">{unavailableMessage}</small>
  {:else}
    <small class="create-option-help">{tr("gui.create.sfx_enable_hint", "Recipients can extract without installing Squallz.")}</small>
  {/if}
{/snippet}

{#if variant === "modern"}
  <section class="create-option-card">
    <h2><Icon name="archive" size={16} />{tr("gui.create.self_extracting", "Self-extracting")}</h2>
    {@render fields()}
  </section>
{:else}
  <div class="classic-label">{tr("gui.create.self_extracting", "Self-extracting")}</div>
  <div id={classicSectionId} class="classic-input create-option-classic classic-create-section-target">{@render fields()}</div>
{/if}
