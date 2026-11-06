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
  import DeferredView from "./DeferredView.svelte";

  let {
    surface,
  }: {
    surface: ToolsWorkspaceSurface;
  } = $props();

  let loadingTitle = $derived(
    surface.tr("gui.tools.workspace_loading")
      .replace("{tool}", surface.title),
  );
  let failureTitle = $derived(
    surface.tr("gui.tools.workspace_load_failed")
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
</script>

<DeferredView
  loader={workspaceLoader} classic={surface.variant === "classic"}
  loadingTag={surface.variant === "classic" ? "div" : "section"}
  {loadingTitle} loadingBody={surface.tr("gui.tools.workspace_loading_body")}
  {failureTitle} failureBody={surface.tr("gui.tools.workspace_load_failed_body")}
  retryLabel={surface.tr("gui.tools.workspace_retry")}
>
  {#snippet frame(content, failed)}
    <div
      class={surface.variant === "modern" ? modernClass : "classic-dialog-body"}
      class:with-archive-return={surface.variant === "classic" && !failed && surface.archiveReturn.visible}
    >
      {#if surface.variant === "classic" && !failed}
        <section class="classic-extract-sheet">{@render content()}</section>
      {:else}
        {@render content()}
      {/if}
    </div>
  {/snippet}
  {#snippet children(Workspace)}<Workspace {surface} />{/snippet}
</DeferredView>
