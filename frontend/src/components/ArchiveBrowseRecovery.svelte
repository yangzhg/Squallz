<script lang="ts" module>
  export type ArchiveBrowseRecoveryState = { message: string; busy: boolean } | null;
</script>

<script lang="ts">
  import Icon from "./Icon.svelte";

  let { state, retryLabel, onRetry }: {
    state: ArchiveBrowseRecoveryState;
    retryLabel: string;
    onRetry: () => void;
  } = $props();
  const messageId = $props.id();
</script>

{#if state}
  <div class="archive-browse-recovery" class:busy={state.busy} role={state.busy ? "status" : "alert"}>
    <Icon name={state.busy ? "rotate-cw" : "alert-triangle"} size={17} />
    <span id={messageId}>{state.message}</span>
    <button type="button" class="secondary-lite" disabled={state.busy} aria-busy={state.busy} aria-describedby={messageId} onclick={onRetry}>{retryLabel}</button>
  </div>
{/if}
