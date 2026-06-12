<script lang="ts">
  import Icon from "./Icon.svelte";

  let {
    title,
    detail,
    contextLabel,
    actionLabel,
    buttonClass = "primary-lite",
    iconSize = 16,
    onReturn,
  }: {
    title: string;
    detail: string;
    contextLabel: string;
    actionLabel: string;
    buttonClass?: string;
    iconSize?: number;
    onReturn: () => void;
  } = $props();
</script>

<div class="archive-return-strip" data-tool-return>
  <div class="archive-return-copy">
    <span class="eyebrow">{contextLabel}</span>
    <strong>{title}</strong>
    <small>{detail}</small>
  </div>
  <button
    class={`${buttonClass} archive-return-button`}
    title={detail}
    aria-label={actionLabel}
    onclick={onReturn}
  ><Icon name="archive" size={iconSize} />{actionLabel}</button>
</div>
