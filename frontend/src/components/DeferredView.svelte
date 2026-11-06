<script lang="ts" module>
  export interface DeferredViewMessages {
    loadingTitle: string;
    loadingBody: string;
    failureTitle: string;
    failureBody: string;
    retryLabel: string;
  }
</script>

<script lang="ts" generics="T">
  import type { Snippet } from "svelte";
  import type { DeferredComponentLoader } from "../lib/deferred-component";
  import type { WindowRecoveryContext } from "../lib/window-recovery";
  import DeferredViewError from "./DeferredViewError.svelte";
  import Icon from "./Icon.svelte";

  let {
    loader,
    loadingTitle,
    loadingBody,
    failureTitle,
    failureBody,
    retryLabel,
    loadingTitleId,
    failureTitleId,
    class: className = "",
    loadingClass,
    loadingTag = "section",
    classic = false,
    recovery = {},
    frame,
    actions,
    children,
  }: DeferredViewMessages & {
    loader: DeferredComponentLoader<T>;
    loadingTitleId?: string;
    failureTitleId?: string;
    class?: string;
    loadingClass?: string;
    loadingTag?: "section" | "div";
    classic?: boolean;
    recovery?: WindowRecoveryContext;
    frame?: Snippet<[content: Snippet, failed: boolean]>;
    actions?: Snippet;
    children: Snippet<[component: T]>;
  } = $props();

  let retried = $state.raw<{ loader: DeferredComponentLoader<T>; promise: Promise<T> } | null>(null);
  let component = $derived(retried?.loader === loader ? retried.promise : loader.load());

  function retry(): void {
    retried = { loader, promise: loader.retry() };
  }
</script>

{#snippet pending()}
  <svelte:element this={loadingTag} class={`deferred-workspace-state ${loadingClass ?? className}`.trim()} role="status" aria-live="polite" aria-busy="true">
    <Icon name="hourglass" size={20} />
    <div>
      <strong id={loadingTitleId}>{loadingTitle}</strong>
      <span>{loadingBody}</span>
    </div>
    {#if actions}
      <div class="deferred-workspace-actions">{@render actions()}</div>
    {/if}
  </svelte:element>
{/snippet}

{#await component}
  {#if frame}{@render frame(pending, false)}{:else}{@render pending()}{/if}
{:then Component}
  {@render children(Component)}
{:catch error}
  {#snippet failed()}
    <DeferredViewError
      {error} title={failureTitle} body={failureBody} {retryLabel} onRetry={retry}
      titleId={failureTitleId} class={className} {classic} {recovery}
    >
      {@render actions?.()}
    </DeferredViewError>
  {/snippet}
  {#if frame}{@render frame(failed, true)}{:else}{@render failed()}{/if}
{/await}
