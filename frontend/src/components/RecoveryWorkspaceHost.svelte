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
  import type { RecoverySession } from "../lib/recovery-session.svelte";
  import type {
    RecoveryWorkspaceTranslate,
    RecoveryWorkspaceVariant,
  } from "../lib/recovery-workspace";

  let {
    variant,
    session,
    tr,
  }: {
    variant: RecoveryWorkspaceVariant;
    session: RecoverySession;
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
      <strong>{tr("gui.recovery.workspace_loading")}</strong>
      <span>{tr("gui.recovery.workspace_loading_body")}</span>
    </div>
  </section>
{:then Workspace}
  <Workspace {variant} {session} {tr} />
{:catch error}
  <DeferredViewError
    {error}
    title={tr("gui.recovery.workspace_load_failed")}
    body={tr("gui.recovery.workspace_load_failed_body")}
    retryLabel={tr("gui.recovery.workspace_retry")}
    onRetry={retry} classic={variant === "classic"}
  />
{/await}
