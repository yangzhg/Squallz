<script lang="ts">
  import type { ChecksumAlgorithmId } from "../lib/ui-model";

  let {
    algorithms,
    selected,
    labelFor,
    hintFor,
    onSelect,
    className = "algorithm-grid",
  }: {
    algorithms: ChecksumAlgorithmId[];
    selected: ChecksumAlgorithmId;
    labelFor: (algorithm: ChecksumAlgorithmId) => string;
    hintFor: (algorithm: ChecksumAlgorithmId) => string;
    onSelect: (algorithm: ChecksumAlgorithmId) => void;
    className?: string;
  } = $props();
</script>

<div class={className}>
  {#each algorithms as algorithm}
    <button
      type="button"
      class:active={selected === algorithm}
      aria-pressed={selected === algorithm}
      onclick={() => onSelect(algorithm)}
    >
      <strong>{labelFor(algorithm)}</strong>
      <small>{hintFor(algorithm)}</small>
    </button>
  {/each}
</div>
