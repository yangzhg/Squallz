<script lang="ts" module>
  import type { ComponentProps } from "svelte";
  import { createDeferredComponentLoader } from "../lib/deferred-component";

  type ModernInspectorComponent = typeof import("./ModernInspector.svelte").default;
  export type ModernInspectorSurfaceProps = ComponentProps<ModernInspectorComponent>;

  const inspectorLoader = createDeferredComponentLoader<ModernInspectorComponent>(
    () => import("./ModernInspector.svelte"),
  );
</script>

<script lang="ts">
  import DeferredView, { type DeferredViewMessages } from "./DeferredView.svelte";

  let {
    surface,
    ariaLabel,
    loadingTitle,
    loadingBody,
    failureTitle,
    failureBody,
    retryLabel,
  }: {
    surface: ModernInspectorSurfaceProps;
    ariaLabel: string;
  } & DeferredViewMessages = $props();
  let previewOpen = $derived(
    surface.view.kind === "archive" && Boolean(
      surface.view.preview.nested || surface.view.preview.busy ||
      surface.view.preview.failed || surface.view.preview.entry,
    ),
  );
</script>

<aside class="modern-inspector" class:preview-open={previewOpen} aria-label={ariaLabel}>
  <DeferredView loader={inspectorLoader} {loadingTitle} {loadingBody} {failureTitle} {failureBody} {retryLabel}>
    {#snippet children(ModernInspector)}<ModernInspector {...surface} />{/snippet}
  </DeferredView>
</aside>
