<script lang="ts">
  import Icon from "./Icon.svelte";
  import type { Screen, ScreenAction } from "../lib/ui-model";

  let {
    sections,
    active,
    labelFor,
    detailFor,
    onChoose,
  }: {
    sections: ScreenAction[];
    active: Screen;
    labelFor: (label: string) => string;
    detailFor: (label: string, detail: string) => string;
    onChoose: (screen: Screen) => void;
  } = $props();
</script>

<div class="settings-route-list">
  {#each sections as item}
    <button
      type="button"
      class:active={active === item.screen}
      class="settings-route-card"
      onclick={() => onChoose(item.screen)}
    >
      <Icon name={item.icon} size={16} />
      <span>
        <strong>{labelFor(item.label)}</strong>
        <small>{detailFor(item.label, item.detail)}</small>
      </span>
    </button>
  {/each}
</div>
