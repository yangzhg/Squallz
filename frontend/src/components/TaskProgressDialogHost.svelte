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
  import DeferredView, { type DeferredViewMessages } from "./DeferredView.svelte";
  import { cssVariables } from "../lib/css-variables";
  import { trapModalFocus } from "../lib/modal-focus";

  let {
    surface,
    loadingTitle,
    loadingBody,
    failureTitle,
    failureBody,
    retryLabel,
    backLabel,
    onUnavailableBack,
  }: {
    surface: TaskProgressDialogSurfaceProps;
    backLabel: string;
    onUnavailableBack?: () => void;
  } & DeferredViewMessages = $props();

  let fallbackCard = $state<HTMLElement | null>(null);
  let presentation = $derived(surface.presentation ?? "dialog");
  let loadingTitleId = $derived(`task-surface-${presentation}-loading-title`);
  let failureTitleId = $derived(`task-surface-${presentation}-load-failed-title`);

  $effect(() => {
    if (fallbackCard === null) return;
    void tick().then(() => fallbackCard?.focus());
  });

  function onFallbackKeydown(event: KeyboardEvent): void {
    if (event.key === "Escape" && !event.defaultPrevented) {
      event.preventDefault();
      event.stopPropagation();
      onUnavailableBack?.();
      return;
    }
    if (presentation === "dialog") trapModalFocus(event, fallbackCard);
  }
</script>

{#snippet back()}
  <button type="button" aria-keyshortcuts="Escape" onclick={onUnavailableBack}>{backLabel}</button>
{/snippet}

<DeferredView
  loader={taskDialogLoader} {loadingTitle} {loadingBody} {failureTitle} {failureBody} {retryLabel}
  {loadingTitleId} {failureTitleId} class="task-surface-deferred"
  recovery={{ taskId: surface.task.id }} actions={onUnavailableBack ? back : undefined}
>
  {#snippet frame(content, failed)}
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
        aria-labelledby={failed ? failureTitleId : loadingTitleId}
        tabindex="-1"
        onkeydown={onFallbackKeydown}
      >
        {@render content()}
      </div>
    </section>
  {/snippet}
  {#snippet children(TaskProgressDialog)}<TaskProgressDialog {...surface} />{/snippet}
</DeferredView>
