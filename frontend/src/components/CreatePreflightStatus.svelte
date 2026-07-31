<script lang="ts">
  import Icon from "./Icon.svelte";

  type StepState = "pending" | "active" | "ready" | "blocked" | "cancelled";

  let {
    variant,
    phase,
    ariaLabel,
    heading,
    statusLabel,
    lockMessage,
    actionLabel,
    actionPending,
    issue,
    steps,
    onAction,
  }: {
    variant: "modern" | "classic";
    phase: string;
    ariaLabel: string;
    heading: string;
    statusLabel: string;
    lockMessage: string;
    actionLabel: string;
    actionPending: boolean;
    issue: string;
    steps: Array<{
      id: string;
      label: string;
      summary: string;
      detail: string;
      state: StepState;
      stateLabel: string;
    }>;
    onAction: () => void;
  } = $props();

  function stepIcon(state: StepState): string {
    if (state === "active") return "hourglass";
    if (state === "ready") return "check-circle";
    if (state === "blocked" || state === "cancelled") return "x-circle";
    return "info";
  }
</script>

<section
  class:classic={variant === "classic"}
  class={`create-preflight-status phase-${phase}`}
  aria-label={ariaLabel}
>
  <header class="create-preflight-heading">
    <div>
      <span>{heading}</span>
      <strong role="status" aria-live="polite">{statusLabel}</strong>
    </div>
    <div class="create-preflight-heading-actions">
      {#if lockMessage}
        <p class="create-preflight-lock" role="note">
          <Icon name="lock" size={14} />
          <span>{lockMessage}</span>
        </p>
      {/if}
      {#if actionLabel}
        <button
          type="button"
          class="create-preflight-action"
          disabled={actionPending}
          aria-busy={actionPending}
          onclick={onAction}
        ><Icon name="x" size={14} />{actionLabel}</button>
      {/if}
    </div>
  </header>

  <ol class="create-preflight-steps">
    {#each steps as step}
      <li class={`state-${step.state}`}>
        <span class="create-preflight-step-icon" aria-hidden="true">
          <Icon name={stepIcon(step.state)} size={16} />
        </span>
        <div class="create-preflight-step-copy">
          <div>
            <span>{step.label}</span>
            <em>{step.stateLabel}</em>
          </div>
          <strong>{step.summary}</strong>
          {#if step.detail}
            <small>{step.detail}</small>
          {/if}
        </div>
      </li>
    {/each}
  </ol>

  {#if issue}
    <p class="create-preflight-issue" role="alert">
      <Icon name="alert-triangle" size={15} />
      <span>{issue}</span>
    </p>
  {/if}
</section>
