<script lang="ts">
  let {
    title,
    hint,
    countLabel,
    value,
    placeholder,
    ariaLabel,
    rules,
    emptyLabel,
    disabled = false,
    disabledReason = "",
    onInput,
  }: {
    title: string;
    hint: string;
    countLabel: string;
    value: string;
    placeholder: string;
    ariaLabel: string;
    rules: string[];
    emptyLabel: string;
    disabled?: boolean;
    disabledReason?: string;
    onInput: (value: string) => void;
  } = $props();
</script>

<div class="exclude-card">
  <div class="exclude-card-head">
    <div>
      <strong>{title}</strong>
      <span>{hint}</span>
    </div>
    <em>{countLabel}</em>
  </div>
  <textarea
    class="exclude-editor"
    rows="3"
    {value}
    {placeholder}
    {disabled}
    title={disabledReason}
    aria-label={disabledReason ? `${ariaLabel} · ${disabledReason}` : ariaLabel}
    oninput={(event) => onInput((event.currentTarget as HTMLTextAreaElement).value)}
  ></textarea>
  <div class="tag-row">
    {#each rules as rule}
      <span>{rule}</span>
    {:else}
      <span>{emptyLabel}</span>
    {/each}
  </div>
</div>
