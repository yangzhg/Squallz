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
  import DeferredView, { type DeferredViewMessages } from "./DeferredView.svelte";

  let {
    surface,
    loadingTitle,
    loadingBody,
    failureTitle,
    failureBody,
    retryLabel,
  }: {
    surface: ModernArchiveBrowserSurfaceProps;
  } & DeferredViewMessages = $props();
</script>

<DeferredView loader={browserLoader} {loadingTitle} {loadingBody} {failureTitle} {failureBody} {retryLabel} class="modern-browser-deferred-state">
  {#snippet children(ModernArchiveBrowser)}<ModernArchiveBrowser {...surface} />{/snippet}
</DeferredView>
