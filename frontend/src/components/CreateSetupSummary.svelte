<script lang="ts">
  type CreateSetupSummaryItem = Readonly<{
    id: string;
    icon: string;
    label: string;
    value: string;
    detail: string;
  }>;

  let {
    variant,
    ariaLabel,
    items,
  }: {
    variant: "modern" | "classic";
    ariaLabel: string;
    eyebrow: string;
    heading: string;
    items: CreateSetupSummaryItem[];
  } = $props();
</script>

<section
  class="create-setup-summary"
  class:classic={variant === "classic"}
  aria-label={ariaLabel}
>
  <dl class="create-setup-summary-grid">
    {#each items as item (item.id)}
      <div class="create-setup-summary-item">
        <dt>{item.label}</dt>
        <dd>
          <strong>{item.value}</strong>
          <small>{item.detail}</small>
        </dd>
      </div>
    {/each}
  </dl>
</section>
