<script lang="ts" module>
  import type { ComponentProps } from "svelte";
  import { createDeferredComponentLoader } from "../lib/deferred-component";

  type ClassicArchiveBrowserComponent = typeof import("./ClassicArchiveBrowser.svelte").default;
  export type ClassicArchiveBrowserSurfaceProps = ComponentProps<ClassicArchiveBrowserComponent>;

  const browserLoader = createDeferredComponentLoader<ClassicArchiveBrowserComponent>(
    () => import("./ClassicArchiveBrowser.svelte"),
  );
</script>

<script lang="ts">
  import Icon from "./Icon.svelte";

  let {
    surface,
    loadingTitle,
    loadingBody,
    failureTitle,
    failureBody,
    retryLabel,
  }: {
    surface: ClassicArchiveBrowserSurfaceProps;
    loadingTitle: string;
    loadingBody: string;
    failureTitle: string;
    failureBody: string;
    retryLabel: string;
  } = $props();

  let browser = $state(browserLoader.load());

  function retry(): void {
    browser = browserLoader.retry();
  }
</script>

{#await browser}
  <div class="classic-body classic-browser-deferred">
    <section class="deferred-workspace-state" role="status" aria-live="polite" aria-busy="true">
      <Icon name="hourglass" size={20} />
      <div>
        <strong>{loadingTitle}</strong>
        <span>{loadingBody}</span>
      </div>
    </section>
  </div>
{:then ClassicArchiveBrowser}
  <ClassicArchiveBrowser {...surface} />
{:catch}
  <div class="classic-body classic-browser-deferred">
    <section class="deferred-workspace-state danger" role="alert">
      <Icon name="alert-triangle" size={20} />
      <div>
        <strong>{failureTitle}</strong>
        <span>{failureBody}</span>
      </div>
      <button type="button" class="classic-primary" onclick={retry}>
        <Icon name="rotate-cw" size={15} />{retryLabel}
      </button>
    </section>
  </div>
{/await}
