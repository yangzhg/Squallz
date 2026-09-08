<script lang="ts" module>
  import type { ComponentProps } from "svelte";
  import { createDeferredComponentLoader } from "../lib/deferred-component";

  type ModernArchiveBrowserComponent = typeof import("./ModernArchiveBrowser.svelte").default;
  export type ModernArchiveBrowserSurfaceProps = ComponentProps<ModernArchiveBrowserComponent>;

  const browserLoader = createDeferredComponentLoader<ModernArchiveBrowserComponent>(
    () => import("./ModernArchiveBrowser.svelte"),
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
    surface: ModernArchiveBrowserSurfaceProps;
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
  <section
    class="deferred-workspace-state modern-browser-deferred-state"
    role="status"
    aria-live="polite"
    aria-busy="true"
  >
    <Icon name="hourglass" size={20} />
    <div>
      <strong>{loadingTitle}</strong>
      <span>{loadingBody}</span>
    </div>
  </section>
{:then ModernArchiveBrowser}
  <ModernArchiveBrowser {...surface} />
{:catch}
  <section class="deferred-workspace-state modern-browser-deferred-state danger" role="alert">
    <Icon name="alert-triangle" size={20} />
    <div>
      <strong>{failureTitle}</strong>
      <span>{failureBody}</span>
    </div>
    <div class="deferred-workspace-actions">
      <button type="button" class="primary-lite" onclick={retry}>
        <Icon name="rotate-cw" size={15} />{retryLabel}
      </button>
    </div>
  </section>
{/await}
