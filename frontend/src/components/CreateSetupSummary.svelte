<script lang="ts">
  import Icon from "./Icon.svelte";

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
    eyebrow,
    heading,
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
  <header>
    <span>{eyebrow}</span>
    <strong>{heading}</strong>
  </header>
  <div class="create-setup-summary-grid">
    {#each items as item (item.id)}
      <div class="create-setup-summary-item">
        <span class="create-setup-summary-icon" aria-hidden="true">
          <Icon name={item.icon} size={16} />
        </span>
        <div>
          <span>{item.label}</span>
          <strong>{item.value}</strong>
          <small>{item.detail}</small>
        </div>
      </div>
    {/each}
  </div>
</section>
