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
  import Icon from "./Icon.svelte";
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
    loadingTitle: string;
    loadingBody: string;
    failureTitle: string;
    failureBody: string;
    retryLabel: string;
    closeLabel: string;
  } = $props();

  let component = $state(taskCenterLoader.load());

  function retry(): void {
    component = taskCenterLoader.retry();
  }
</script>

{#await component}
  <aside
    id="squallz-task-center"
    class={`${surface.rootClass} task-surface-shell`}
    use:cssVariables={surface.rootVariables ?? {}}
    aria-labelledby="task-center-loading-title"
  >
    <section class="deferred-workspace-state task-surface-deferred" role="status" aria-live="polite" aria-busy="true">
      <Icon name="hourglass" size={20} />
      <div>
        <strong id="task-center-loading-title">{loadingTitle}</strong>
        <span>{loadingBody}</span>
      </div>
    </section>
  </aside>
{:then TaskCenter}
  <TaskCenter {...surface} />
{:catch}
  <aside
    id="squallz-task-center"
    class={`${surface.rootClass} task-surface-shell`}
    use:cssVariables={surface.rootVariables ?? {}}
    aria-labelledby="task-center-load-failed-title"
  >
    <section class="deferred-workspace-state task-surface-deferred danger" role="alert">
      <Icon name="alert-triangle" size={20} />
      <div>
        <strong id="task-center-load-failed-title">{failureTitle}</strong>
        <span>{failureBody}</span>
      </div>
      <div class="deferred-workspace-actions">
        <button type="button" onclick={surface.onClose}>{closeLabel}</button>
        <button type="button" class="primary-lite" onclick={retry}>
          <Icon name="rotate-cw" size={15} />{retryLabel}
        </button>
      </div>
    </section>
  </aside>
{/await}
