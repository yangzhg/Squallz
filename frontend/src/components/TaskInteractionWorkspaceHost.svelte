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

  let {
    surface,
  }: {
    surface: TaskInteractionWorkspaceSurface;
  } = $props();

  let workspace = $state(workspaceLoader.load());
  let modernClass = $derived(
    surface.kind === "password"
      ? "password-view modern-password"
      : "conflict-view modern-conflict",
  );
  let classicClass = $derived(
    surface.kind === "password"
      ? "classic-extract-sheet classic-password"
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
    <div class="classic-dialog-body">
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
{:catch}
  {#if surface.variant === "modern"}
    <div class={modernClass}>
      <section class="deferred-workspace-state danger" role="alert">
        <Icon name="alert-triangle" size={20} />
        <div>
          <strong>{surface.tr("gui.task_surface.load_failed", "Task view could not be loaded")}</strong>
          <span>{surface.tr("gui.task_surface.load_failed_body", "The task is still safe. Retry loading its progress and controls.")}</span>
        </div>
        <button type="button" class="primary-lite" onclick={retry}>
          <Icon name="rotate-cw" size={15} />{surface.tr("gui.task_surface.retry", "Retry view")}
        </button>
      </section>
    </div>
  {:else}
    <div class="classic-dialog-body">
      <section class={classicClass}>
        <div class="deferred-workspace-state danger" role="alert">
          <Icon name="alert-triangle" size={20} />
          <div>
            <strong>{surface.tr("gui.task_surface.load_failed", "Task view could not be loaded")}</strong>
            <span>{surface.tr("gui.task_surface.load_failed_body", "The task is still safe. Retry loading its progress and controls.")}</span>
          </div>
          <button type="button" class="classic-primary" onclick={retry}>
            <Icon name="rotate-cw" size={15} />{surface.tr("gui.task_surface.retry", "Retry view")}
          </button>
        </div>
      </section>
    </div>
  {/if}
{/await}
