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
  import DeferredView from "./DeferredView.svelte";

  let {
    surface,
  }: {
    surface: PasswordWorkspaceSurface;
  } = $props();
</script>

<DeferredView
  loader={workspaceLoader} classic={surface.variant === "classic"}
  loadingTag={surface.variant === "classic" ? "div" : "section"}
  loadingTitle={surface.tr("gui.password.loading")}
  loadingBody={surface.tr("gui.password.loading_body")}
  failureTitle={surface.tr("gui.password.load_failed")}
  failureBody={surface.tr("gui.password.load_failed_body")}
  retryLabel={surface.tr("gui.task_surface.retry")}
>
  {#snippet frame(content, failed)}
    <div class={surface.variant === "classic" ? "classic-dialog-body password-workspace" : "password-workspace"}>
      {#if surface.variant === "classic" && !failed}
        <section class="password-request">{@render content()}</section>
      {:else}
        {@render content()}
      {/if}
    </div>
  {/snippet}
  {#snippet children(Workspace)}<Workspace {surface} />{/snippet}
</DeferredView>
