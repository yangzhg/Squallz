<script lang="ts" module>
  import { createDeferredComponentLoader } from "../lib/deferred-component";
  import type { PasswordWorkspaceSurface } from "./PasswordWorkspace.svelte";

  type PasswordWorkspaceComponent =
    typeof import("./PasswordWorkspace.svelte").default;

  export type {
    PasswordWorkspaceSurface,
    PasswordWorkspaceVariant,
  } from "./PasswordWorkspace.svelte";

  const workspaceLoader = createDeferredComponentLoader<PasswordWorkspaceComponent>(
    () => import("./PasswordWorkspace.svelte"),
  );
</script>

<script lang="ts">
  import Icon from "./Icon.svelte";
  import DeferredViewError from "./DeferredViewError.svelte";

  let {
    surface,
  }: {
    surface: PasswordWorkspaceSurface;
  } = $props();

  let workspace = $state(workspaceLoader.load());
  function retry(): void {
    workspace = workspaceLoader.retry();
  }
</script>

{#await workspace}
  {#if surface.variant === "modern"}
    <div class="password-workspace">
      <section class="deferred-workspace-state" role="status" aria-live="polite" aria-busy="true">
        <Icon name="hourglass" size={20} />
        <div>
          <strong>{surface.tr("gui.password.loading", "Loading password entry")}</strong>
          <span>{surface.tr("gui.password.loading_body", "Preparing the password form for this archive.")}</span>
        </div>
      </section>
    </div>
  {:else}
    <div class="classic-dialog-body password-workspace">
      <section class="password-request">
        <div class="deferred-workspace-state" role="status" aria-live="polite" aria-busy="true">
          <Icon name="hourglass" size={20} />
          <div>
            <strong>{surface.tr("gui.password.loading", "Loading password entry")}</strong>
            <span>{surface.tr("gui.password.loading_body", "Preparing the password form for this archive.")}</span>
          </div>
        </div>
      </section>
    </div>
  {/if}
{:then Workspace}
  <Workspace {surface} />
{:catch error}
  {#if surface.variant === "modern"}
    <div class="password-workspace">
      <DeferredViewError
        {error} title={surface.tr("gui.password.load_failed", "Password entry could not be loaded")}
        body={surface.tr("gui.password.load_failed_body", "Retry loading the password form to continue opening the archive.")}
        retryLabel={surface.tr("gui.task_surface.retry", "Retry view")} onRetry={retry}
      />
    </div>
  {:else}
    <div class="classic-dialog-body password-workspace">
      <DeferredViewError
        {error} title={surface.tr("gui.password.load_failed", "Password entry could not be loaded")}
        body={surface.tr("gui.password.load_failed_body", "Retry loading the password form to continue opening the archive.")}
        retryLabel={surface.tr("gui.task_surface.retry", "Retry view")} onRetry={retry} classic
      />
    </div>
  {/if}
{/await}
