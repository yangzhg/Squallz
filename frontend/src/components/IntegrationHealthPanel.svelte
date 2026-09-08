<script lang="ts">
  import Icon from "./Icon.svelte";

  type IntegrationActionViewState =
    | "healthy"
    | "missing"
    | "damaged"
    | "checking"
    | "unavailable";

  interface IntegrationActionView {
    id: string;
    label: string;
    state: IntegrationActionViewState;
    stateLabel: string;
    detail: string | null;
  }

  type IntegrationDiagnosticTone = "neutral" | "info" | "warning";

  interface IntegrationDiagnosticView {
    id: string;
    label: string;
    stateLabel: string;
    detail: string;
    tone: IntegrationDiagnosticTone;
    actionLabel?: string | null;
  }

  let {
    panelTitle,
    healthTitle,
    platform,
    scope,
    summary,
    detail,
    state,
    busy,
    actions,
    diagnosticsTitle = null,
    diagnostics = [],
    onDiagnosticAction,
    removeLabel,
    removeDisabledReason,
    removeAriaLabel,
    onRemove,
  }: {
    panelTitle: string;
    healthTitle: string;
    platform: string;
    scope: string;
    summary: string;
    detail: string;
    state: string;
    busy: boolean;
    actions: IntegrationActionView[];
    diagnosticsTitle?: string | null;
    diagnostics?: IntegrationDiagnosticView[];
    onDiagnosticAction?: (id: string) => void;
    removeLabel: string;
    removeDisabledReason: string;
    removeAriaLabel: string;
    onRemove: () => void;
  } = $props();

  function actionIcon(actionState: IntegrationActionViewState): string {
    if (actionState === "healthy") return "check-circle";
    if (actionState === "damaged") return "alert-triangle";
    if (actionState === "missing") return "x-circle";
    if (actionState === "checking") return "rotate-cw";
    return "info";
  }
</script>

<aside class="context-panel integration-health-panel" aria-busy={busy}>
  <div class={`integration-health-card state-${state}`} role="status" aria-live="polite" aria-atomic="true">
    <div class="integration-health-heading">
      <Icon name={state === "healthy" ? "check-circle" : state === "needs-repair" ? "alert-triangle" : state === "missing" ? "x-circle" : state === "unavailable" ? "info" : "rotate-cw"} size={18} />
      <div>
        <span>{healthTitle}</span>
        <strong>{summary}</strong>
      </div>
    </div>
    <p>{detail}</p>
  </div>

  <div class="panel-title integration-action-title"><Icon name="list" size={16} />{panelTitle}</div>
  <ul class="integration-action-list">
    {#each actions as action (action.id)}
      <li class={`integration-action-row state-${action.state}`}>
        <Icon name={actionIcon(action.state)} size={14} />
        <div class="integration-action-copy">
          <strong>{action.label}</strong>
          {#if action.detail}<small>{action.detail}</small>{/if}
        </div>
        <span class={`integration-action-status state-${action.state}`}>{action.stateLabel}</span>
      </li>
    {/each}
  </ul>

  {#if diagnosticsTitle && diagnostics.length > 0}
    <section class="integration-diagnostics">
      <h2 class="panel-title integration-diagnostics-title"><Icon name="info" size={16} />{diagnosticsTitle}</h2>
      <dl class="integration-diagnostic-list" aria-live="polite" aria-atomic="true">
        {#each diagnostics as diagnostic (diagnostic.id)}
          <div class={`integration-diagnostic-row tone-${diagnostic.tone}`}>
            <dt>{diagnostic.label}</dt>
            <dd class={`integration-diagnostic-state tone-${diagnostic.tone}`}>{diagnostic.stateLabel}</dd>
            <dd class="integration-diagnostic-detail">{diagnostic.detail}</dd>
            {#if diagnostic.actionLabel && onDiagnosticAction}
              <dd class="integration-diagnostic-action">
                <button type="button" onclick={() => onDiagnosticAction?.(diagnostic.id)}>
                  <span>{diagnostic.actionLabel}</span><Icon name="external-link" size={14} />
                </button>
              </dd>
            {/if}
          </div>
        {/each}
      </dl>
    </section>
  {/if}

  <div class="platform-note integration-scope-note">
    <strong>{platform}</strong>
    <span>{scope}</span>
  </div>
  <button
    type="button"
    class="integration-remove-action"
    disabled={Boolean(removeDisabledReason)}
    title={removeDisabledReason}
    aria-label={removeAriaLabel}
    onclick={onRemove}
  ><Icon name="x-circle" size={15} />{removeLabel}</button>
</aside>
