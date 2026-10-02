<script lang="ts" module>
  export interface ArchiveEditingState {
    message: string;
    pending: boolean;
    retryLabel: string | null;
  }
</script>

<script lang="ts">
  import Icon from "./Icon.svelte";

  let { state, onRetry }: {
    state: ArchiveEditingState;
    onRetry: () => void;
  } = $props();
</script>

<div class="archive-editing-status" role="status" aria-live="polite" aria-busy={state.pending}>
  <Icon name={state.pending ? "hourglass" : "info"} size={15} />
  <span>{state.message}</span>
  {#if state.retryLabel}
    <button type="button" onclick={onRetry}><Icon name="rotate-cw" size={14} />{state.retryLabel}</button>
  {/if}
</div>
