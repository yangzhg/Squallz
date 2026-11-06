<script lang="ts" module>
  import type { ComponentProps } from "svelte";
  import { createDeferredComponentLoader } from "../lib/deferred-component";

  type TaskCenterComponent = typeof import("./TaskCenter.svelte").default;
  export type TaskCenterSurfaceProps = ComponentProps<TaskCenterComponent>;

  const taskCenterLoader = createDeferredComponentLoader<TaskCenterComponent>(
    () => import("./TaskCenter.svelte"),
  );
</script>

<script lang="ts">
  import DeferredView, { type DeferredViewMessages } from "./DeferredView.svelte";
  import { cssVariables } from "../lib/css-variables";

  let {
    surface,
    loadingTitle,
    loadingBody,
    failureTitle,
    failureBody,
    retryLabel,
    closeLabel,
  }: {
    surface: TaskCenterSurfaceProps;
    closeLabel: string;
  } & DeferredViewMessages = $props();
</script>

<DeferredView
  loader={taskCenterLoader} {loadingTitle} {loadingBody} {failureTitle} {failureBody} {retryLabel}
  loadingTitleId="task-center-loading-title" failureTitleId="task-center-load-failed-title"
  class="task-surface-deferred" recovery={{ taskCenter: true }}
>
  {#snippet frame(content, failed)}
    <aside
      id="squallz-task-center"
      class={`${surface.rootClass} task-surface-shell`}
      use:cssVariables={surface.rootVariables ?? {}}
      aria-labelledby={failed ? "task-center-load-failed-title" : "task-center-loading-title"}
    >
      {@render content()}
    </aside>
  {/snippet}
  {#snippet actions()}<button type="button" onclick={surface.onClose}>{closeLabel}</button>{/snippet}
  {#snippet children(TaskCenter)}<TaskCenter {...surface} />{/snippet}
</DeferredView>
