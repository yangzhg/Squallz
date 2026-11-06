<script lang="ts" module>
  import { createDeferredComponentLoader } from "../lib/deferred-component";

  type SettingsWorkspaceComponent = typeof import("./SettingsWorkspace.svelte").default;

  const workspaceLoader = createDeferredComponentLoader<SettingsWorkspaceComponent>(
    () => import("./SettingsWorkspace.svelte"),
  );
</script>

<script lang="ts">
  import DeferredView, { type DeferredViewMessages } from "./DeferredView.svelte";
  import type { SettingsWorkspaceProps } from "./SettingsWorkspace.svelte";

  let {
    workspace,
    loadingTitle,
    loadingBody,
    failureTitle,
    failureBody,
    retryLabel,
  }: {
    workspace: SettingsWorkspaceProps;
  } & DeferredViewMessages = $props();
</script>

<DeferredView loader={workspaceLoader} {loadingTitle} {loadingBody} {failureTitle} {failureBody} {retryLabel}>
  {#snippet children(Workspace)}<Workspace {...workspace} />{/snippet}
</DeferredView>
