<script lang="ts" module>
  import type { Component } from "svelte";
  import { createDeferredComponentLoader } from "../lib/deferred-component";
  import type {
    ConvertWorkspaceSurface,
    ConvertWorkspaceVariant,
  } from "./ConvertWorkspace.svelte";
  import type {
    CreateWorkspaceSurface,
    CreateWorkspaceVariant,
  } from "./CreateWorkspace.svelte";
  import type {
    ExtractWorkspaceSurface,
    ExtractWorkspaceVariant,
  } from "./ExtractWorkspace.svelte";

  export type {
    ConvertWorkspaceSurface,
    ConvertWorkspaceVariant,
  } from "./ConvertWorkspace.svelte";
  export type {
    CreateWorkspaceSurface,
    CreateWorkspaceVariant,
  } from "./CreateWorkspace.svelte";
  export type {
    ExtractWorkspaceSurface,
    ExtractWorkspaceVariant,
  } from "./ExtractWorkspace.svelte";

  type WorkspaceKind = "create" | "convert" | "extract";
  type WorkspaceVariant = CreateWorkspaceVariant | ConvertWorkspaceVariant | ExtractWorkspaceVariant;
  type WorkspaceSurface = CreateWorkspaceSurface | ConvertWorkspaceSurface | ExtractWorkspaceSurface;
  type WorkspaceProps = {
    loadingTitle: string;
    loadingBody: string;
    failureTitle: string;
    failureBody: string;
    retryLabel: string;
  } & (
    | { kind: "create"; variant: CreateWorkspaceVariant; surface: CreateWorkspaceSurface }
    | { kind: "convert"; variant: ConvertWorkspaceVariant; surface: ConvertWorkspaceSurface }
    | { kind: "extract"; variant: ExtractWorkspaceVariant; surface: ExtractWorkspaceSurface }
  );
  type WorkspaceComponent = Component<{
    variant: WorkspaceVariant;
    surface: WorkspaceSurface;
  }>;

  const workspaceLoaders = {
    create: createDeferredComponentLoader<WorkspaceComponent>(
      () => import("./CreateWorkspace.svelte") as Promise<{ default: WorkspaceComponent }>,
    ),
    convert: createDeferredComponentLoader<WorkspaceComponent>(
      () => import("./ConvertWorkspace.svelte") as Promise<{ default: WorkspaceComponent }>,
    ),
    extract: createDeferredComponentLoader<WorkspaceComponent>(
      () => import("./ExtractWorkspace.svelte") as Promise<{ default: WorkspaceComponent }>,
    ),
  } satisfies Record<WorkspaceKind, ReturnType<typeof createDeferredComponentLoader<WorkspaceComponent>>>;
</script>

<script lang="ts">
  import Icon from "./Icon.svelte";

  let props: WorkspaceProps = $props();

  let retryWorkspace = $state<{ kind: WorkspaceKind; promise: Promise<WorkspaceComponent> } | null>(null);
  let workspace = $derived(
    retryWorkspace?.kind === props.kind
      ? retryWorkspace.promise
      : workspaceLoaders[props.kind].load(),
  );
  let modernClass = $derived(
    props.kind === "extract" ? "extract-view modern-extract" : `create-sheet modern-${props.kind}`,
  );
  let classicClass = $derived(
    props.kind === "create" ? "classic-property-sheet classic-create" : `classic-extract-sheet classic-${props.kind}`,
  );

  function retry(): void {
    retryWorkspace = {
      kind: props.kind,
      promise: workspaceLoaders[props.kind].retry(),
    };
  }
</script>

{#await workspace}
  {#if props.variant === "modern"}
    <div class={modernClass}>
      <section class="deferred-workspace-state" role="status" aria-live="polite" aria-busy="true">
        <Icon name="hourglass" size={20} />
        <div>
          <strong>{props.loadingTitle}</strong>
          <span>{props.loadingBody}</span>
        </div>
      </section>
    </div>
  {:else}
    <div class="classic-dialog-body">
      <section class={classicClass}>
        <div class="deferred-workspace-state" role="status" aria-live="polite" aria-busy="true">
          <Icon name="hourglass" size={20} />
          <div>
            <strong>{props.loadingTitle}</strong>
            <span>{props.loadingBody}</span>
          </div>
        </div>
      </section>
    </div>
  {/if}
{:then Workspace}
  <Workspace variant={props.variant} surface={props.surface} />
{:catch}
  {#if props.variant === "modern"}
    <div class={modernClass}>
      <section class="deferred-workspace-state danger" role="alert">
        <Icon name="alert-triangle" size={20} />
        <div>
          <strong>{props.failureTitle}</strong>
          <span>{props.failureBody}</span>
        </div>
        <button type="button" class="primary-lite" onclick={retry}>
          <Icon name="rotate-cw" size={15} />{props.retryLabel}
        </button>
      </section>
    </div>
  {:else}
    <div class="classic-dialog-body">
      <section class={classicClass}>
        <div class="deferred-workspace-state danger" role="alert">
          <Icon name="alert-triangle" size={20} />
          <div>
            <strong>{props.failureTitle}</strong>
            <span>{props.failureBody}</span>
          </div>
          <button type="button" class="classic-primary" onclick={retry}>
            <Icon name="rotate-cw" size={15} />{props.retryLabel}
          </button>
        </div>
      </section>
    </div>
  {/if}
{/await}
