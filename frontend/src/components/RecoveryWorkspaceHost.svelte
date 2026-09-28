<script lang="ts" module>
  import { createDeferredComponentLoader } from "../lib/deferred-component";

  type RecoveryWorkspaceComponent = typeof import("./RecoveryWorkspace.svelte").default;

  const workspaceLoader = createDeferredComponentLoader<RecoveryWorkspaceComponent>(
    () => import("./RecoveryWorkspace.svelte"),
  );
</script>

<script lang="ts">
  import Icon from "./Icon.svelte";
  import DeferredViewError from "./DeferredViewError.svelte";
  import type {
    RecoveryWorkspaceActions,
    RecoveryWorkspaceTranslate,
    RecoveryWorkspaceVariant,
    RecoveryWorkspaceView,
  } from "../lib/recovery-workspace";

  let {
    variant,
    view,
    actions,
    tr,
  }: {
    variant: RecoveryWorkspaceVariant;
    view: RecoveryWorkspaceView;
    actions: RecoveryWorkspaceActions;
    tr: RecoveryWorkspaceTranslate;
  } = $props();

  let workspace = $state(workspaceLoader.load());

  function retry(): void {
    workspace = workspaceLoader.retry();
  }
</script>

{#await workspace}
  <section class="deferred-workspace-state recovery" role="status" aria-live="polite" aria-busy="true">
    <Icon name="hourglass" size={20} />
    <div>
      <strong>{tr("gui.recovery.workspace_loading", "Loading Recovery")}</strong>
      <span>{tr("gui.recovery.workspace_loading_body", "Checking the selected archive and preparing its recovery actions.")}</span>
    </div>
  </section>
{:then Workspace}
  <Workspace {variant} {view} {actions} {tr} />
{:catch error}
  <DeferredViewError
    {error}
    title={tr("gui.recovery.workspace_load_failed", "Recovery could not be loaded")}
    body={tr("gui.recovery.workspace_load_failed_body", "Your archive was not changed. Retry loading the recovery workspace.")}
    retryLabel={tr("gui.recovery.workspace_retry", "Retry")}
    onRetry={retry} classic={variant === "classic"}
  />
{/await}
