<script lang="ts" module>
  import { createDeferredComponentLoader } from "../lib/deferred-component";

  type RecoveryWorkspaceComponent = typeof import("./RecoveryWorkspace.svelte").default;

  const workspaceLoader = createDeferredComponentLoader<RecoveryWorkspaceComponent>(
    () => import("./RecoveryWorkspace.svelte"),
  );
</script>

<script lang="ts">
  import DeferredView from "./DeferredView.svelte";
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
</script>

<DeferredView
  loader={workspaceLoader} loadingClass="recovery" classic={variant === "classic"}
  loadingTitle={tr("gui.recovery.workspace_loading")}
  loadingBody={tr("gui.recovery.workspace_loading_body")}
  failureTitle={tr("gui.recovery.workspace_load_failed")}
  failureBody={tr("gui.recovery.workspace_load_failed_body")}
  retryLabel={tr("gui.recovery.workspace_retry")}
>
  {#snippet children(Workspace)}<Workspace {variant} {session} {tr} />{/snippet}
</DeferredView>
