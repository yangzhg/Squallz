<script lang="ts" module>
  import type { ComponentProps } from "svelte";
  import { createDeferredComponentLoader } from "../lib/deferred-component";

  type TaskProgressDialogComponent = typeof import("./TaskProgressDialog.svelte").default;
  export type TaskProgressDialogSurfaceProps = ComponentProps<TaskProgressDialogComponent>;

  const taskDialogLoader = createDeferredComponentLoader<TaskProgressDialogComponent>(
    () => import("./TaskProgressDialog.svelte"),
  );
</script>

<script lang="ts">
  import { tick } from "svelte";
  import Icon from "./Icon.svelte";
  import { cssVariables } from "../lib/css-variables";

  let {
    surface,
    loadingTitle,
    loadingBody,
    failureTitle,
    failureBody,
    retryLabel,
    backLabel,
  }: {
    surface: TaskProgressDialogSurfaceProps;
    loadingTitle: string;
    loadingBody: string;
    failureTitle: string;
    failureBody: string;
    retryLabel: string;
    backLabel: string;
  } = $props();

  let component = $state(taskDialogLoader.load());
  let fallbackCard = $state<HTMLElement | null>(null);
  let presentation = $derived(surface.presentation ?? "dialog");
  let loadingTitleId = $derived(`task-surface-${presentation}-loading-title`);
  let failureTitleId = $derived(`task-surface-${presentation}-load-failed-title`);

  $effect(() => {
    if (presentation !== "dialog" || fallbackCard === null) return;
    void tick().then(() => fallbackCard?.focus());
  });

  function retry(): void {
    component = taskDialogLoader.retry();
  }
</script>

{#await component}
  <section
    id={surface.rootId}
    class={`${surface.rootClass} task-surface-shell`}
    use:cssVariables={surface.rootVariables ?? {}}
    role={presentation === "dialog" ? "presentation" : undefined}
    data-task-presentation={presentation}
  >
    <div
      bind:this={fallbackCard}
      class="task-modal-card task-surface-load-card"
      role={presentation === "dialog" ? "dialog" : "region"}
      aria-modal={presentation === "dialog" ? "true" : undefined}
      aria-labelledby={loadingTitleId}
      tabindex="-1"
    >
      <section class="deferred-workspace-state task-surface-deferred" role="status" aria-live="polite" aria-busy="true">
        <Icon name="hourglass" size={20} />
        <div>
          <strong id={loadingTitleId}>{loadingTitle}</strong>
          <span>{loadingBody}</span>
        </div>
      </section>
    </div>
  </section>
{:then TaskProgressDialog}
  <TaskProgressDialog {...surface} />
{:catch}
  <section
    id={surface.rootId}
    class={`${surface.rootClass} task-surface-shell`}
    use:cssVariables={surface.rootVariables ?? {}}
    role={presentation === "dialog" ? "presentation" : undefined}
    data-task-presentation={presentation}
  >
    <div
      bind:this={fallbackCard}
      class="task-modal-card task-surface-load-card"
      role={presentation === "dialog" ? "dialog" : "region"}
      aria-modal={presentation === "dialog" ? "true" : undefined}
      aria-labelledby={failureTitleId}
      tabindex="-1"
    >
      <section class="deferred-workspace-state task-surface-deferred danger" role="alert">
        <Icon name="alert-triangle" size={20} />
        <div>
          <strong id={failureTitleId}>{failureTitle}</strong>
          <span>{failureBody}</span>
        </div>
        <div class="deferred-workspace-actions">
          {#if presentation === "panel"}
            <button type="button" onclick={() => void surface.onDismiss(surface.task)}>{backLabel}</button>
          {/if}
          <button type="button" class="primary-lite" onclick={retry}>
            <Icon name="rotate-cw" size={15} />{retryLabel}
          </button>
        </div>
      </section>
    </div>
  </section>
{/await}
