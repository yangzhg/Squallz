<script lang="ts">
  import Icon from "./Icon.svelte";

  export type ExtractPlanSummaryMetric = Readonly<{
    id: string;
    label: string;
    value: string;
    tone?: "default" | "warning";
  }>;

  let {
    variant,
    phase,
    ariaLabel,
    eyebrow,
    heading,
    statusLabel,
    description,
    destinationLabel,
    destination,
    metrics,
    note,
    error,
    retryLabel,
    onRetry,
  }: {
    variant: "modern" | "classic";
    phase: "idle" | "loading" | "ready" | "blocked" | "error";
    ariaLabel: string;
    eyebrow: string;
    heading: string;
    statusLabel: string;
    description: string;
    destinationLabel: string;
    destination: string;
    metrics: ExtractPlanSummaryMetric[];
    note: string;
    error: string;
    retryLabel: string;
    onRetry: () => void;
  } = $props();

  const statusIcon = $derived(
    phase === "ready"
      ? "check-circle"
      : phase === "error" || phase === "blocked"
        ? "alert-triangle"
        : phase === "loading"
          ? "hourglass"
          : "archive",
  );
</script>

<section
  class="extract-plan-summary"
  class:classic={variant === "classic"}
  class:state-ready={phase === "ready"}
  class:state-blocked={phase === "blocked"}
  class:state-error={phase === "error"}
  aria-label={ariaLabel}
  aria-busy={phase === "loading"}
  aria-live="polite"
>
  <header>
    <div>
      <span>{eyebrow}</span>
      <h2>{heading}</h2>
    </div>
    <strong class="extract-plan-status"><Icon name={statusIcon} size={15} />{statusLabel}</strong>
  </header>

  {#if phase === "ready" || phase === "blocked"}
    <p>{description}</p>
    <div class="extract-plan-destination">
      <span>{destinationLabel}</span>
      <strong>{destination}</strong>
    </div>
    <dl class="extract-plan-metrics">
      {#each metrics as metric (metric.id)}
        <div class:warning={metric.tone === "warning"}>
          <dt>{metric.label}</dt>
          <dd>{metric.value}</dd>
        </div>
      {/each}
    </dl>
    <p class="extract-plan-note">{note}</p>
    {#if phase === "blocked"}
      <button type="button" class="extract-plan-retry" onclick={onRetry}>
        <Icon name="rotate-cw" size={15} />{retryLabel}
      </button>
    {/if}
  {:else if phase === "error"}
    <p class="extract-plan-error">{error}</p>
    <button type="button" class="extract-plan-retry" onclick={onRetry}>
      <Icon name="rotate-cw" size={15} />{retryLabel}
    </button>
  {:else}
    <p>{description}</p>
    {#if phase === "loading"}
      <div class="extract-plan-loading" aria-hidden="true">
        <span></span><span></span><span></span>
      </div>
    {:else}
      <div class="extract-plan-idle" aria-hidden="true">
        <Icon name="archive" size={28} />
      </div>
    {/if}
  {/if}
</section>
