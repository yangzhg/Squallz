<script lang="ts" module>
  import { createDeferredComponentLoader } from "../lib/deferred-component";

  type SettingsWorkspaceComponent = typeof import("./SettingsWorkspace.svelte").default;

  const workspaceLoader = createDeferredComponentLoader<SettingsWorkspaceComponent>(
    () => import("./SettingsWorkspace.svelte"),
  );
</script>

<script lang="ts">
  import Icon from "./Icon.svelte";
  import DeferredViewError from "./DeferredViewError.svelte";
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
    loadingTitle: string;
    loadingBody: string;
    failureTitle: string;
    failureBody: string;
    retryLabel: string;
  } = $props();

  let component = $state(workspaceLoader.load());

  function retry(): void {
    component = workspaceLoader.retry();
  }
</script>

{#await component}
  <section class="deferred-workspace-state" role="status" aria-live="polite" aria-busy="true">
    <Icon name="hourglass" size={20} />
    <div>
      <strong>{loadingTitle}</strong>
      <span>{loadingBody}</span>
    </div>
  </section>
{:then Workspace}
  <Workspace {...workspace} />
{:catch error}
  <DeferredViewError {error} title={failureTitle} body={failureBody} {retryLabel} onRetry={retry} />
{/await}
