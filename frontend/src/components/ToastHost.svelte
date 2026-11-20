<script lang="ts">
  import { untrack } from "svelte";
  import Icon from "./Icon.svelte";
  import { cssVariables, type CssVariableMap } from "../lib/css-variables";
  import { tFallback } from "../lib/i18n.svelte";
  import {
    dismissToast,
    runToastAction,
    setToastReading,
    setToastsBlocked,
    toasts,
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

  let host = $state<HTMLElement | null>(null);
  let leadingToastId: number | undefined;

  $effect.pre(() => {
    const nextBlocked = blocked;
    untrack(() => setToastsBlocked(nextBlocked));
    return () => setToastsBlocked(true);
  });

  $effect(() => {
    const leading = toasts()[0];
    if (!host || leading?.id === leadingToastId) return;
    leadingToastId = leading?.id;
    if (leading?.kind === "danger" || leading?.kind === "warning") host.scrollTop = 0;
  });

  function toastIcon(kind: ToastKind): string {
    if (kind === "success") return "check-circle";
    if (kind === "warning") return "alert-triangle";
    if (kind === "danger") return "shield-alert";
    return "info";
  }

  function trackReading(node: HTMLElement, id: number) {
    let hovered = node.matches(":hover") && matchMedia("(any-hover: hover)").matches;
    let focused = node.contains(node.ownerDocument.activeElement);
    const sync = () => setToastReading(id, hovered || focused);
    const controller = new AbortController();
    const options = { signal: controller.signal };
    function onPointer(event: PointerEvent) {
      if (event.pointerType === "touch") return;
      hovered = event.type === "pointerenter";
      sync();
    }
    node.addEventListener("pointerenter", onPointer, options);
    node.addEventListener("pointerleave", onPointer, options);
    node.addEventListener("focusin", () => { focused = true; sync(); }, options);
    node.addEventListener("focusout", (event) => {
      focused = event.relatedTarget instanceof Node && node.contains(event.relatedTarget);
      sync();
    }, options);
    sync();
    return { destroy() { controller.abort(); setToastReading(id, false); } };
  }
</script>

{#if !blocked && toasts().length > 0}
  <section
    bind:this={host}
    class={`toast-host ${rootClass}`}
    use:cssVariables={rootVariables}
    aria-label={tFallback("gui.toast.notifications")}
  >
    {#each toasts() as toast (toast.id)}
      <article
        use:trackReading={toast.id}
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
              disabled={toast.actionPending}
              aria-busy={toast.actionPending ?? false}
              onclick={() => void runToastAction(toast.id)}
            >
              {toast.action.label}
            </button>
          {/if}
        </div>
        <button
          class="toast-dismiss"
          type="button"
          aria-label={`${tFallback("gui.common.close")}: ${toast.title}`}
          title={tFallback("gui.common.close")}
          onclick={() => dismissToast(toast.id)}
        ><Icon name="x" size={14} /></button>
      </article>
    {/each}
  </section>
{/if}
