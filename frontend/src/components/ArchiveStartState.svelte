<script lang="ts">
  import AppIcon from "./AppIcon.svelte";
  import Icon from "./Icon.svelte";

  let {
    variant,
    eyebrow,
    title,
    body,
    openLabel,
    createLabel,
    openBusy = false,
    onOpen,
    onCreate,
  }: {
    variant: "modern" | "classic";
    eyebrow: string;
    title: string;
    body: string;
    openLabel: string;
    createLabel: string;
    openBusy?: boolean;
    onOpen: () => void;
    onCreate: () => void;
  } = $props();

  const componentId = $props.id();
  const headingId = `${componentId}-archive-start-title`;
</script>

<section class={`archive-start-state variant-${variant}`} aria-labelledby={headingId}>
  <div class="archive-start-mark" aria-hidden="true">
    <AppIcon class="archive-start-icon" />
  </div>
  <div class="archive-start-copy">
    <span class="eyebrow">{eyebrow}</span>
    <h1 id={headingId}>{title}</h1>
    <p>{body}</p>
    <div class="archive-start-actions">
      <button type="button" class="primary" aria-busy={openBusy} onclick={onOpen}>
        <Icon name="folder-open" />{openLabel}
      </button>
      <button type="button" onclick={onCreate}>
        <Icon name="sparkles" />{createLabel}
      </button>
    </div>
  </div>
</section>
