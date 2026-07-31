<script lang="ts">
  import Icon from "./Icon.svelte";

  type CreatePlanReviewItem = Readonly<{
    id: string;
    label: string;
    value: string;
  }>;

  let {
    variant,
    ariaLabel,
    eyebrow,
    heading,
    description,
    outputName,
    items,
    confirmLabel,
    cancelLabel,
    busy = false,
    onConfirm,
    onCancel,
  }: {
    variant: "modern" | "classic";
    ariaLabel: string;
    eyebrow: string;
    heading: string;
    description: string;
    outputName: string;
    items: CreatePlanReviewItem[];
    confirmLabel: string;
    cancelLabel: string;
    busy?: boolean;
    onConfirm: () => void;
    onCancel: () => void;
  } = $props();
</script>

<section
  class="create-plan-review"
  class:classic={variant === "classic"}
  aria-label={ariaLabel}
  tabindex="-1"
>
  <div class="create-plan-review-copy">
    <span>{eyebrow}</span>
    <h2>{heading}</h2>
    <p>{description}</p>
    <strong class="create-plan-review-output"><Icon name="archive" size={16} />{outputName}</strong>
  </div>
  <dl class="create-plan-review-metrics">
    {#each items as item (item.id)}
      <div>
        <dt>{item.label}</dt>
        <dd>{item.value}</dd>
      </div>
    {/each}
  </dl>
  <div class="create-plan-review-actions">
    <button type="button" disabled={busy} onclick={onCancel}>{cancelLabel}</button>
    <button
      type="button"
      class:primary={variant === "modern"}
      class:classic-primary={variant === "classic"}
      disabled={busy}
      aria-busy={busy}
      onclick={onConfirm}
    ><Icon name={busy ? "hourglass" : "play"} size={16} />{confirmLabel}</button>
  </div>
</section>
