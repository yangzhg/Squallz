<script lang="ts" module>
  export type PasswordWorkspaceVariant = "modern" | "classic";

  type Tr = (key: string, fallback?: string) => string;

  export interface PasswordWorkspaceSurface {
    variant: PasswordWorkspaceVariant;
    tr: Tr;
    active: boolean;
    name: string;
    detail: string;
    sessionDetail: string;
    failureDetail: string;
    secretStoreLabel: string;
    value: string;
    busy: boolean;
    rejected: boolean;
    error: string | null;
    onInputMount: (input: HTMLInputElement | null) => void;
    onValueChange: (value: string) => void;
    onSubmit: () => void | Promise<void>;
    onCancel: () => void;
    onBack: () => void;
  }
</script>

<script lang="ts">
  import Icon from "./Icon.svelte";

  let {
    surface,
  }: {
    surface: PasswordWorkspaceSurface;
  } = $props();

  function registerPasswordInput(input: HTMLInputElement) {
    const onInputMount = surface.onInputMount;
    onInputMount(input);
    return {
      destroy() {
        onInputMount(null);
      },
    };
  }

  function updatePassword(event: Event): void {
    surface.onValueChange((event.currentTarget as HTMLInputElement).value);
  }
</script>

<div class="password-workspace" class:classic-dialog-body={surface.variant === "classic"}>
  <section class="password-request" aria-labelledby="password-request-title">
    <header class="password-request-header">
      <div class="password-lock"><Icon name="lock" size={24} /></div>
      <div>
        {#if surface.active}
          <span class="eyebrow">{surface.tr("gui.password.required")}</span>
          <h1 id="password-request-title">{surface.name}</h1>
        {:else}
          <span class="eyebrow">{surface.tr("gui.password.empty_eyebrow")}</span>
          <h1 id="password-request-title">{surface.tr("gui.password.empty_title")}</h1>
        {/if}
      </div>
    </header>
    {#if surface.active}
      <form
        class="password-request-form"
        onsubmit={(event) => {
          event.preventDefault();
          void surface.onSubmit();
        }}
      >
        <div class="password-request-body">
          <p id="password-request-detail" class:password-inline-error={surface.rejected} role={surface.rejected ? "alert" : "status"}>{surface.detail}</p>
          <label class="password-request-field">
            <span class="secure-label">{surface.tr("gui.password.password")}</span>
            <input
              use:registerPasswordInput
              class="secure-input"
              type="password"
              value={surface.value}
              disabled={surface.busy}
              autocomplete="current-password"
              aria-label={surface.tr("gui.password.archive_password")}
              aria-invalid={surface.error || surface.rejected ? "true" : undefined}
              aria-describedby={surface.error ? "password-request-detail password-request-session password-request-error" : "password-request-detail password-request-session"}
              oninput={updatePassword}
            />
          </label>
          {#if surface.error}
            <small id="password-request-error" class="password-inline-error" role="alert">{surface.error}</small>
          {/if}
          <p id="password-request-session" class="password-request-session"><Icon name="info" size={14} /><span>{surface.sessionDetail}</span></p>
          <details class="password-request-details">
            <summary>{surface.tr("gui.password.handling_details")}</summary>
            <p>{surface.tr("gui.password.prompt_boundary_body")}</p>
            <dl>
              <div><dt>{surface.tr("gui.password.fallback")}</dt><dd>{surface.tr("gui.password.manual_wins_body")}</dd></div>
              <div><dt>{surface.tr("gui.password.on_failure")}</dt><dd>{surface.failureDetail}</dd></div>
              <div><dt>{surface.tr("gui.password.session")}</dt><dd>{surface.tr("gui.password.session_zeroize")}</dd></div>
              <div><dt>{surface.secretStoreLabel}</dt><dd>{surface.tr("gui.password.secret_store_supplies_directly")} {surface.tr("gui.password.keychain_opt_in").replace("{secretStore}", surface.secretStoreLabel)}</dd></div>
            </dl>
          </details>
        </div>
        <footer class="modal-actions password-request-actions">
          <button type="button" onclick={surface.onCancel}>{surface.tr("common.cancel")}</button>
          <button class="primary-lite" class:classic-primary={surface.variant === "classic"} type="submit" aria-busy={surface.busy} disabled={surface.busy}>
            {surface.busy ? surface.tr("gui.password.unlocking") : surface.tr("gui.password.unlock_continue")}
          </button>
        </footer>
      </form>
    {:else}
      <div class="password-request-body">
        <p>{surface.tr("gui.password.no_active_request", "No password request is active")}</p>
        <p>{surface.tr("gui.password.no_active_request_body")}</p>
      </div>
      <footer class="modal-actions password-request-actions">
        <button onclick={surface.onBack}>{surface.tr("gui.nav.back_to_archive")}</button>
      </footer>
    {/if}
  </section>
</div>
