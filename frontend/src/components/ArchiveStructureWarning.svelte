<script lang="ts">
  import Icon from "./Icon.svelte";

  let {
    message,
    actionLabel,
    onRepair,
  }: {
    message: string;
    actionLabel: string;
    onRepair: () => void;
  } = $props();
</script>

<div class="warning-ribbon" role="status" data-archive-structure-warning>
  <Icon name="alert-triangle" size={17} />
  <span>{message}</span>
  <button data-archive-structure-repair onclick={onRepair}>{actionLabel}</button>
</div>
