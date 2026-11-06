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
  import DeferredView, { type DeferredViewMessages } from "./DeferredView.svelte";

  let {
    surface,
    loadingTitle,
    loadingBody,
    failureTitle,
    failureBody,
    retryLabel,
  }: {
    surface: ClassicArchiveBrowserSurfaceProps;
  } & DeferredViewMessages = $props();
</script>

<DeferredView loader={browserLoader} {loadingTitle} {loadingBody} {failureTitle} {failureBody} {retryLabel} classic>
  {#snippet frame(content)}
    <div class="classic-body classic-browser-deferred">{@render content()}</div>
  {/snippet}
  {#snippet children(ClassicArchiveBrowser)}<ClassicArchiveBrowser {...surface} />{/snippet}
</DeferredView>
