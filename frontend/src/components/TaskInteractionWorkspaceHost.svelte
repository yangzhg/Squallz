<script lang="ts" module>
  import { createDeferredComponentLoader } from "../lib/deferred-component";
  import type { TaskInteractionWorkspaceSurface } from "./TaskInteractionWorkspace.svelte";

  type TaskInteractionWorkspaceComponent =
    typeof import("./TaskInteractionWorkspace.svelte").default;

  export type {
    ConflictInteractionSurface,
    PasswordInteractionSurface,
    TaskInteractionWorkspaceKind,
    TaskInteractionWorkspaceSurface,
    TaskInteractionWorkspaceVariant,
  } from "./TaskInteractionWorkspace.svelte";

  const workspaceLoader = createDeferredComponentLoader<TaskInteractionWorkspaceComponent>(
    () => import("./TaskInteractionWorkspace.svelte"),
  );
</script>

<script lang="ts">
  import Icon from "./Icon.svelte";
  import DeferredViewError from "./DeferredViewError.svelte";

  let {
    surface,
  }: {
    surface: TaskInteractionWorkspaceSurface;
  } = $props();

  let workspace = $state(workspaceLoader.load());
  let modernClass = $derived(
    surface.kind === "password"
      ? "password-workspace"
      : "conflict-view modern-conflict",
  );
  let classicClass = $derived(
    surface.kind === "password"
      ? "password-request"
      : "classic-extract-sheet classic-conflict",
  );

  function retry(): void {
    workspace = workspaceLoader.retry();
  }
</script>

{#await workspace}
  {#if surface.variant === "modern"}
    <div class={modernClass}>
      <section class="deferred-workspace-state" role="status" aria-live="polite" aria-busy="true">
        <Icon name="hourglass" size={20} />
        <div>
          <strong>{surface.tr("gui.task_surface.loading", "Loading task view")}</strong>
          <span>{surface.tr("gui.task_surface.loading_body", "Preparing live progress, results, and task controls.")}</span>
        </div>
      </section>
    </div>
  {:else}
    <div class="classic-dialog-body" class:password-workspace={surface.kind === "password"}>
      <section class={classicClass}>
        <div class="deferred-workspace-state" role="status" aria-live="polite" aria-busy="true">
          <Icon name="hourglass" size={20} />
          <div>
            <strong>{surface.tr("gui.task_surface.loading", "Loading task view")}</strong>
            <span>{surface.tr("gui.task_surface.loading_body", "Preparing live progress, results, and task controls.")}</span>
          </div>
        </div>
      </section>
    </div>
  {/if}
{:then Workspace}
  <Workspace {surface} />
{:catch error}
  {#if surface.variant === "modern"}
    <div class={modernClass}>
      <DeferredViewError
        {error} title={surface.tr("gui.task_surface.load_failed", "Task view could not be loaded")}
        body={surface.tr("gui.task_surface.load_failed_body", "The task is still safe. Retry loading its progress and controls.")}
        retryLabel={surface.tr("gui.task_surface.retry", "Retry view")} onRetry={retry}
      />
    </div>
  {:else}
    <div class="classic-dialog-body" class:password-workspace={surface.kind === "password"}>
      <DeferredViewError
        {error} title={surface.tr("gui.task_surface.load_failed", "Task view could not be loaded")}
        body={surface.tr("gui.task_surface.load_failed_body", "The task is still safe. Retry loading its progress and controls.")}
        retryLabel={surface.tr("gui.task_surface.retry", "Retry view")} onRetry={retry} classic
      />
    </div>
  {/if}
{/await}
