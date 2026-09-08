<script lang="ts">
  import { onMount } from "svelte";
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

  let routeList: HTMLDivElement;
  let canScrollBefore = $state(false);
  let canScrollAfter = $state(false);
  let revealFrame: number | undefined;

  function updateScrollCues(): void {
    if (!routeList) return;
    const remaining = routeList.scrollWidth - routeList.clientWidth;
    canScrollBefore = routeList.scrollLeft > 1;
    canScrollAfter = routeList.scrollLeft < remaining - 1;
  }

  function revealActiveRoute(screen: Screen, items: ScreenAction[]): void {
    if (revealFrame !== undefined) cancelAnimationFrame(revealFrame);
    revealFrame = requestAnimationFrame(() => {
      revealFrame = undefined;
      const activeIndex = items.findIndex((item) => item.screen === screen);
      const activeRoute = routeList?.children.item(activeIndex);
      if (activeRoute instanceof HTMLElement) {
        activeRoute.scrollIntoView({ block: "nearest", inline: "center" });
        const listBounds = routeList.getBoundingClientRect();
        const activeBounds = activeRoute.getBoundingClientRect();
        if (activeBounds.left < listBounds.left) {
          routeList.scrollLeft -= listBounds.left - activeBounds.left;
        } else if (activeBounds.right > listBounds.right) {
          routeList.scrollLeft += activeBounds.right - listBounds.right;
        }
      }
      updateScrollCues();
    });
  }

  $effect(() => revealActiveRoute(active, sections));

  onMount(() => {
    const resizeObserver = new ResizeObserver(() => revealActiveRoute(active, sections));
    resizeObserver.observe(routeList);
    for (const item of routeList.children) {
      resizeObserver.observe(item);
    }
    revealActiveRoute(active, sections);
    return () => {
      resizeObserver.disconnect();
      if (revealFrame !== undefined) cancelAnimationFrame(revealFrame);
    };
  });
</script>

<div
  class:can-scroll-before={canScrollBefore}
  class:can-scroll-after={canScrollAfter}
  class="settings-route-scroll"
>
  <div class="settings-route-list" bind:this={routeList} onscroll={updateScrollCues}>
    {#each sections as item}
      <button
        type="button"
        class:active={active === item.screen}
        class="settings-route-card"
        aria-current={active === item.screen ? "page" : undefined}
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
</div>
