<script lang="ts">
  import type { Snippet } from "svelte";
  import Icon from "./Icon.svelte";
  import { navigationGroups, settingsNavigation, type Screen, type ScreenAction } from "../lib/ui-model";

  let { compact = false, current, tr, disabled, disabledReason, onChoose, children }: {
    compact?: boolean;
    current: Screen;
    tr: (key: string) => string;
    disabled: (screen: Screen) => boolean;
    disabledReason: (screen: Screen) => string;
    onChoose: (screen: Screen) => void;
    children?: Snippet;
  } = $props();

  function chooseFromMenu(event: Event & { currentTarget: HTMLSelectElement }) {
    const select = event.currentTarget;
    const next = select.value as Screen;
    // Navigation may require confirmation. Keep showing the active page until it changes.
    select.value = current;
    onChoose(next);
  }
</script>

{#snippet routeButton(item: ScreenAction)}
  <button type="button" class="sidebar-route" class:current={item.screen === current}
    aria-current={item.screen === current ? "page" : undefined}
    disabled={disabled(item.screen)} title={disabledReason(item.screen) || undefined}
    onclick={() => onChoose(item.screen)}>
    <Icon name={item.icon} size={18} />
    <span>{tr(item.labelKey)}</span>
  </button>
{/snippet}

{#if compact}
  <label class="compact-route-picker">
    <span>{tr("gui.aria.navigation")}</span>
    <select value={current} onchange={chooseFromMenu}>
      {#each navigationGroups as group}
        <optgroup label={tr(group.labelKey)}>
          {#each group.items as item}
            <option value={item.screen} disabled={disabled(item.screen)}>{tr(item.labelKey)}</option>
          {/each}
        </optgroup>
      {/each}
      <option value={settingsNavigation.screen} disabled={disabled(settingsNavigation.screen)}>{tr(settingsNavigation.labelKey)}</option>
    </select>
  </label>
{:else}
  <nav class="modern-sidebar" aria-label={tr("gui.aria.navigation")}>
    <div class="workspace-routes">
      {#each navigationGroups as group}
        <div class="sidebar-section" role="group" aria-label={tr(group.labelKey)}>
          <span class="sidebar-section-title" aria-hidden="true">{tr(group.labelKey)}</span>
          {#each group.items as item}
            {@render routeButton(item)}
          {/each}
        </div>
      {/each}
      <div class="sidebar-footer">
        {@render children?.()}
        {@render routeButton(settingsNavigation)}
      </div>
    </div>
  </nav>
{/if}
