<script lang="ts">
  import { onMount } from "svelte";
  import { cssVariables, type CssVariableMap } from "../lib/css-variables";
  import { trapModalFocus } from "../lib/modal-focus";

  let {
    title, label, value, placeholder = "", status, hint = "", cancelLabel,
    rootClass, rootVariables, onChange, onSubmit, onClose,
  }: {
    title: string;
    label: string;
    value: string;
    placeholder?: string;
    status: string;
    hint?: string;
    cancelLabel: string;
    rootClass: string;
    rootVariables: CssVariableMap;
    onChange: (value: string) => void;
    onSubmit: () => Promise<void>;
    onClose: () => void;
  } = $props();

  const id = $props.id();
  let panel: HTMLElement;
  let input: HTMLInputElement;
  let submitting = $state(false);

  onMount(() => {
    input.focus();
    input.select();
  });

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    if (submitting) return;
    submitting = true;
    try {
      await onSubmit();
    } finally {
      submitting = false;
    }
  }

  function onKeydown(event: KeyboardEvent) {
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      if (!submitting) onClose();
    } else {
      trapModalFocus(event, panel);
    }
  }
</script>

<div class={rootClass} use:cssVariables={rootVariables}>
  <div
    class="archive-editor-card"
    bind:this={panel}
    role="dialog"
    aria-modal="true"
    aria-labelledby={`${id}-title`}
    aria-busy={submitting}
    tabindex="-1"
    onkeydown={onKeydown}
  >
    <h2 id={`${id}-title`}>{title}</h2>
    <form onsubmit={submit}>
      <label class="archive-editor-field">
        <span>{label}</span>
        <input
          bind:this={input}
          {value}
          {placeholder}
          autocorrect="off"
          autocapitalize="off"
          autocomplete="off"
          spellcheck={false}
          disabled={submitting}
          aria-describedby={`${id}-status ${id}-hint`}
          oninput={(event) => onChange(event.currentTarget.value)}
        />
      </label>
      <p id={`${id}-status`} class="archive-editor-status" role="status">{status}</p>
      <p id={`${id}-hint`} class="archive-editor-hint">{hint}</p>
      <footer class="archive-editor-actions">
        <button type="button" disabled={submitting} onclick={onClose}>{cancelLabel}</button>
        <button class="primary" type="submit" disabled={submitting}>{title}</button>
      </footer>
    </form>
  </div>
</div>
