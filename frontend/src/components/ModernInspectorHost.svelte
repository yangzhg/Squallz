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
  import Icon from "./Icon.svelte";

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
    loadingTitle: string;
    loadingBody: string;
    failureTitle: string;
    failureBody: string;
    retryLabel: string;
  } = $props();

  let component = $state(inspectorLoader.load());

  function retry(): void {
    component = inspectorLoader.retry();
  }
</script>

<aside class="modern-inspector" aria-label={ariaLabel}>
  {#await component}
    <section class="deferred-workspace-state" role="status" aria-live="polite" aria-busy="true">
      <Icon name="hourglass" size={20} />
      <div>
        <strong>{loadingTitle}</strong>
        <span>{loadingBody}</span>
      </div>
    </section>
  {:then ModernInspector}
    <ModernInspector {...surface} />
  {:catch}
    <section class="deferred-workspace-state danger" role="alert">
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
</aside>
