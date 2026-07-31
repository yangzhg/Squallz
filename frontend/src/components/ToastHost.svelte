<script lang="ts">
  import Icon from "./Icon.svelte";
  import { cssVariables, type CssVariableMap } from "../lib/css-variables";
  import { tFallback } from "../lib/i18n.svelte";
  import {
    pushToast,
    dismissToast,
    removeToast,
    toasts,
    type Toast,
    type ToastKind,
  } from "../lib/toasts.svelte";

  let {
    rootClass,
    rootVariables = {},
    blocked = false,
  }: {
    rootClass: string;
    rootVariables?: CssVariableMap;
    blocked?: boolean;
  } = $props();

  let runningActionIds = $state<number[]>([]);

  function toastIcon(kind: ToastKind): string {
    if (kind === "success") return "check-circle";
    if (kind === "warning") return "alert-triangle";
    if (kind === "danger") return "shield-alert";
    return "info";
  }

  async function runToastAction(toast: Toast): Promise<void> {
    if (!toast.action || runningActionIds.includes(toast.id)) return;
    runningActionIds = [...runningActionIds, toast.id];
    try {
      const completed = await toast.action.run();
      if (completed !== false) removeToast(toast.id);
    } catch {
      pushToast({
        kind: "warning",
        title: tFallback("gui.toast.action_failed", "Could not complete this action"),
      });
    } finally {
      runningActionIds = runningActionIds.filter((id) => id !== toast.id);
    }
  }
</script>

{#if toasts().length > 0}
  <section
    class={`toast-host ${rootClass}`}
    use:cssVariables={rootVariables}
    aria-label={tFallback("gui.toast.notifications", "Notifications")}
    aria-hidden={blocked ? "true" : undefined}
    inert={blocked}
  >
    {#each toasts() as toast (toast.id)}
      <article
        class={`toast-card kind-${toast.kind}`}
        role={toast.kind === "danger" ? "alert" : "status"}
        aria-live={toast.kind === "danger" ? "assertive" : "polite"}
        aria-atomic="true"
      >
        <span class="toast-icon"><Icon name={toastIcon(toast.kind)} size={16} /></span>
        <div class="toast-copy">
          <strong>{toast.title}</strong>
          {#if toast.body}<p>{toast.body}</p>{/if}
          {#if toast.action}
            <button
              class="toast-action"
              type="button"
              disabled={runningActionIds.includes(toast.id)}
              aria-busy={runningActionIds.includes(toast.id)}
              onclick={() => void runToastAction(toast)}
            >
              {toast.action.label}
            </button>
          {/if}
        </div>
        <button
          class="toast-dismiss"
          type="button"
          aria-label={`${tFallback("gui.common.close", "Close")}: ${toast.title}`}
          title={tFallback("gui.common.close", "Close")}
          onclick={() => dismissToast(toast.id)}
        ><Icon name="x" size={14} /></button>
      </article>
    {/each}
  </section>
{/if}
