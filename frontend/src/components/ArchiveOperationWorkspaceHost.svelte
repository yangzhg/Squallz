<script lang="ts" module>
  import type { Component } from "svelte";
  import { createDeferredComponentLoader } from "../lib/deferred-component";
  import type { DeferredViewMessages } from "./DeferredView.svelte";
  import type {
    ConvertRouteBridge,
    ConvertRouteOwner,
    ConvertWorkspaceVariant,
  } from "../lib/convert-route";
  import type { CreateSession } from "../lib/create-session.svelte";
  import type {
    CreateWorkspaceVariant,
  } from "./CreateWorkspace.svelte";
  import type {
    ExtractWorkspaceBridge,
    ExtractWorkspaceVariant,
  } from "./ExtractWorkspace.svelte";

  export type { ConvertWorkspaceVariant } from "../lib/convert-route";
  export type {
    CreateWorkspaceVariant,
  } from "./CreateWorkspace.svelte";
  export type {
    ExtractWorkspaceBridge,
    ExtractWorkspaceVariant,
  } from "./ExtractWorkspace.svelte";

  import type { ExtractWorkspaceSession } from "../lib/extract-workspace.svelte";
  import type { ExtractPresentation } from "../lib/extract-presentation";
  import type { ArchivePresetSession } from "../lib/archive-presets.svelte";

  type WorkspaceKind = "create" | "convert" | "extract";
  type WorkspaceProps = DeferredViewMessages & (
    | { kind: "create"; variant: CreateWorkspaceVariant; session: CreateSession }
    | {
        kind: "convert";
        variant: ConvertWorkspaceVariant;
        owner: ConvertRouteOwner;
        bridge: ConvertRouteBridge;
      }
    | {
        kind: "extract";
        variant: ExtractWorkspaceVariant;
        session: ExtractWorkspaceSession;
        view: ExtractPresentation;
        presets: ArchivePresetSession;
        bridge: ExtractWorkspaceBridge;
      }
  );
  type WorkspaceComponent = Component<Record<string, unknown>>;

  const workspaceLoaders = {
    create: createDeferredComponentLoader<WorkspaceComponent>(
      () => import("./CreateWorkspace.svelte") as Promise<{ default: WorkspaceComponent }>,
    ),
    convert: createDeferredComponentLoader<WorkspaceComponent>(
      () => import("./ConvertWorkspaceRoute.svelte") as Promise<{ default: WorkspaceComponent }>,
    ),
    extract: createDeferredComponentLoader<WorkspaceComponent>(
      () => import("./ExtractWorkspace.svelte") as Promise<{ default: WorkspaceComponent }>,
    ),
  } satisfies Record<WorkspaceKind, ReturnType<typeof createDeferredComponentLoader<WorkspaceComponent>>>;
</script>

<script lang="ts">
  import DeferredView from "./DeferredView.svelte";

  let props: WorkspaceProps = $props();

  let modernClass = $derived(
    props.kind === "extract" ? "extract-view modern-extract" : `create-sheet modern-${props.kind}`,
  );
  let classicClass = $derived(
    props.kind === "create" ? "classic-property-sheet classic-create" : `classic-extract-sheet classic-${props.kind}`,
  );
</script>

<DeferredView
  loader={workspaceLoaders[props.kind]} classic={props.variant === "classic"}
  loadingTag={props.variant === "classic" ? "div" : "section"}
  loadingTitle={props.loadingTitle} loadingBody={props.loadingBody}
  failureTitle={props.failureTitle} failureBody={props.failureBody} retryLabel={props.retryLabel}
>
  {#snippet frame(content, failed)}
    <div class={props.variant === "modern" ? modernClass : "classic-dialog-body"}>
      {#if props.variant === "classic" && !failed}
        <section class={classicClass}>{@render content()}</section>
      {:else}
        {@render content()}
      {/if}
    </div>
  {/snippet}
  {#snippet children(Workspace)}
    {#if props.kind === "convert"}
      <Workspace variant={props.variant} owner={props.owner} bridge={props.bridge} />
    {:else if props.kind === "extract"}
      <Workspace variant={props.variant} session={props.session} view={props.view} presets={props.presets} bridge={props.bridge} />
    {:else}
      <Workspace variant={props.variant} session={props.session} />
    {/if}
  {/snippet}
</DeferredView>
