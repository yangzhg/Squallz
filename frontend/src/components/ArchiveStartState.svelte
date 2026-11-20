<script lang="ts">
  import AppIcon from "./AppIcon.svelte";
  import Icon from "./Icon.svelte";
  import workspaceArt from "../assets/archive-workspace.webp";
  import type { Snippet } from "svelte";

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
    children,
  }: {
    variant: "modern" | "classic";
    eyebrow?: string;
    title: string;
    body: string;
    openLabel: string;
    createLabel: string;
    openBusy?: boolean;
    onOpen: () => void;
    onCreate: () => void;
    children?: Snippet;
  } = $props();

  const componentId = $props.id();
  const headingId = `${componentId}-archive-start-title`;
</script>

<section class={`archive-start-state variant-${variant}`} aria-labelledby={headingId}>
  <div class="archive-start-mark" aria-hidden="true">
    {#if variant === "modern"}
      <img class="archive-start-art" src={workspaceArt} alt="" draggable="false" />
    {:else}
      <AppIcon class="archive-start-icon" />
    {/if}
  </div>
  <div class="archive-start-copy">
    {#if eyebrow}<span class="eyebrow">{eyebrow}</span>{/if}
    <h1 id={headingId}>{title}</h1>
    <p>{body}</p>
    <div class="archive-start-actions">
      <button type="button" class="primary" aria-busy={openBusy} disabled={openBusy} onclick={onOpen}>
        <Icon name="folder-open" />{openLabel}
      </button>
      <button type="button" onclick={onCreate}>
        <Icon name="sparkles" />{createLabel}
      </button>
    </div>
    {@render children?.()}
  </div>
</section>
