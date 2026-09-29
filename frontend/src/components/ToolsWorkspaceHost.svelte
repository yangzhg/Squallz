<script lang="ts" module>
  import { createDeferredComponentLoader } from "../lib/deferred-component";
  import type { ToolsWorkspaceSurface } from "./ToolsWorkspace.svelte";

  type ToolsWorkspaceComponent = typeof import("./ToolsWorkspace.svelte").default;

  export type {
    ArchiveUpdateWorkspaceSurface,
    BatchWorkspaceSurface,
    ChecksumResultKind,
    ChecksumWorkspaceSurface,
    DuplicatesWorkspaceSurface,
    ToolsWorkspaceKind,
    ToolsWorkspaceSurface,
    ToolsWorkspaceVariant,
  } from "./ToolsWorkspace.svelte";

  const workspaceLoader = createDeferredComponentLoader<ToolsWorkspaceComponent>(
    () => import("./ToolsWorkspace.svelte"),
  );
</script>

<script lang="ts">
  import Icon from "./Icon.svelte";
  import DeferredViewError from "./DeferredViewError.svelte";

  let {
    surface,
  }: {
    surface: ToolsWorkspaceSurface;
  } = $props();

  let workspace = $state(workspaceLoader.load());

  let loadingTitle = $derived(
    surface.tr("gui.tools.workspace_loading", "Loading {tool}")
      .replace("{tool}", surface.title),
  );
  let failureTitle = $derived(
    surface.tr("gui.tools.workspace_load_failed", "{tool} could not be loaded")
      .replace("{tool}", surface.title),
  );
  let modernClass = $derived(
    surface.kind === "update"
      ? "operation-workspace"
      : surface.kind === "batch"
      ? "batch-view modern-batch"
      : surface.kind === "checksum"
        ? "settings-view modern-checksum"
        : "settings-view modern-duplicates",
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
          <strong>{loadingTitle}</strong>
          <span>{surface.tr("gui.tools.workspace_loading_body", "Preparing this tool and its current options.")}</span>
        </div>
      </section>
    </div>
  {:else}
    <div class="classic-dialog-body" class:with-archive-return={surface.archiveReturn.visible}>
      <section class="classic-extract-sheet">
        <div class="deferred-workspace-state" role="status" aria-live="polite" aria-busy="true">
          <Icon name="hourglass" size={20} />
          <div>
            <strong>{loadingTitle}</strong>
            <span>{surface.tr("gui.tools.workspace_loading_body", "Preparing this tool and its current options.")}</span>
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
        {error} title={failureTitle}
        body={surface.tr("gui.tools.workspace_load_failed_body", "Your selected archive, target, and options were not changed. Retry loading this workspace.")}
        retryLabel={surface.tr("gui.tools.workspace_retry", "Retry")} onRetry={retry}
      />
    </div>
  {:else}
    <div class="classic-dialog-body">
      <DeferredViewError
        {error} title={failureTitle}
        body={surface.tr("gui.tools.workspace_load_failed_body", "Your selected archive, target, and options were not changed. Retry loading this workspace.")}
        retryLabel={surface.tr("gui.tools.workspace_retry", "Retry")} onRetry={retry} classic
      />
    </div>
  {/if}
{/await}
