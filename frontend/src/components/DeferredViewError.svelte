<script lang="ts">
  import { tick, type Snippet } from "svelte";
  import { DeferredComponentLoadError } from "../lib/deferred-component";
  import { tFallback } from "../lib/i18n.svelte";
  import { reloadWindow, type WindowRecoveryContext } from "../lib/window-recovery";
  import Icon from "./Icon.svelte";

  let {
    error,
    title,
    body,
    retryLabel,
    onRetry,
    titleId,
    class: className = "",
    classic = false,
    recovery = {},
    children,
  }: {
    error: unknown;
    title: string;
    body: string;
    retryLabel: string;
    onRetry: () => void;
    titleId?: string;
    class?: string;
    classic?: boolean;
    recovery?: WindowRecoveryContext;
    children?: Snippet;
  } = $props();

  let confirmingReload = $state(false);
  let reloadButton = $state<HTMLButtonElement | null>(null);
  let cancelButton = $state<HTMLButtonElement | null>(null);
  let reloadRequired = $derived(error instanceof DeferredComponentLoadError && error.retryFailed);

  async function confirmReload(): Promise<void> {
    confirmingReload = true;
    await tick();
    cancelButton?.focus();
  }

  async function cancelReload(): Promise<void> {
    confirmingReload = false;
    await tick();
    reloadButton?.focus();
  }

  function onConfirmationKeydown(event: KeyboardEvent): void {
    if (event.key !== "Escape") return;
    event.preventDefault();
    event.stopPropagation();
    void cancelReload();
  }
</script>

<section class={`deferred-workspace-state deferred-view-error danger ${className}`} role="alert">
  <Icon name="alert-triangle" size={20} />
  <div>
    <strong id={titleId}>{title}</strong>
    <span>
      {#if confirmingReload}
        {tFallback("gui.view_recovery.reload_confirmation", "Reloading resets this window's unsubmitted changes and current view. Submitted tasks continue running. Reload this window?")}
      {:else if reloadRequired}
        {tFallback("gui.view_recovery.retry_failed", "This view still could not be loaded. Reload this window to try again.")}
      {:else}
        {body}
      {/if}
    </span>
  </div>
  <div class="deferred-workspace-actions">
    {#if confirmingReload}
      <button bind:this={cancelButton} type="button" onclick={() => void cancelReload()} onkeydown={onConfirmationKeydown}>
        {tFallback("gui.common.cancel", "Cancel")}
      </button>
      <button type="button" class:classic-primary={classic} class:primary-lite={!classic} onclick={() => reloadWindow(recovery)} onkeydown={onConfirmationKeydown}>
        <Icon name="rotate-cw" size={15} />{tFallback("gui.view_recovery.reload", "Reload window")}
      </button>
    {:else}
      {@render children?.()}
      {#if reloadRequired}
        <button bind:this={reloadButton} type="button" class:classic-primary={classic} class:primary-lite={!classic} onclick={() => void confirmReload()}>
          <Icon name="rotate-cw" size={15} />{tFallback("gui.view_recovery.reload", "Reload window")}
        </button>
      {:else}
        <button type="button" class:classic-primary={classic} class:primary-lite={!classic} onclick={onRetry}>
          <Icon name="rotate-cw" size={15} />{retryLabel}
        </button>
      {/if}
    {/if}
  </div>
</section>
