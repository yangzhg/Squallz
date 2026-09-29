<script lang="ts" module>
  export type PasswordWorkspaceVariant = "modern" | "classic";

  type Tr = (key: string, fallback: string) => string;

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
    forgetVisible: boolean;
    forgetDisabledReason: string;
    forgetAriaLabel: string;
    onInputMount: (input: HTMLInputElement | null) => void;
    onValueChange: (value: string) => void;
    onSubmit: () => void | Promise<void>;
    onCancel: () => void;
    onForget: () => void | Promise<void>;
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
          <span class="eyebrow">{surface.tr("gui.password.required", "Password required")}</span>
          <h1 id="password-request-title">{surface.name}</h1>
        {:else}
          <span class="eyebrow">{surface.tr("gui.password.empty_eyebrow", "Password")}</span>
          <h1 id="password-request-title">{surface.tr("gui.password.empty_title", "Password entry")}</h1>
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
            <span class="secure-label">{surface.tr("gui.password.password", "Password")}</span>
            <input
              use:registerPasswordInput
              class="secure-input"
              type="password"
              value={surface.value}
              disabled={surface.busy}
              autocomplete="current-password"
              aria-label={surface.tr("gui.password.archive_password", "Archive password")}
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
            <summary>{surface.tr("gui.password.handling_details", "How passwords are handled")}</summary>
            <p>{surface.tr("gui.password.prompt_boundary_body", "Unlock only the archive that requested credentials. No password is written to logs, settings, or task status.")}</p>
            <dl>
              <div><dt>{surface.tr("gui.password.fallback", "Fallback")}</dt><dd>{surface.tr("gui.password.manual_wins_body", "Manual input takes priority, followed by the session password, then the saved password. Rejected saved passwords return to this prompt.")}</dd></div>
              <div><dt>{surface.tr("gui.password.on_failure", "On failure")}</dt><dd>{surface.failureDetail}</dd></div>
              <div><dt>{surface.tr("gui.password.session", "Session")}</dt><dd>{surface.tr("gui.password.session_zeroize", "Session cache: cleared on exit or when forgotten.")}</dd></div>
              <div><dt>{surface.secretStoreLabel}</dt><dd>{surface.tr("gui.password.secret_store_supplies_directly", "Squallz shows only their status; archive operations retrieve saved passwords when needed.")} {surface.tr("gui.password.keychain_opt_in", "{secretStore}: opt-in, per archive account.").replace("{secretStore}", surface.secretStoreLabel)}</dd></div>
            </dl>
          </details>
        </div>
        <footer class="modal-actions password-request-actions">
          <button type="button" onclick={surface.onCancel}>{surface.tr("common.cancel", "Cancel")}</button>
          {#if surface.forgetVisible}
            <button type="button" disabled={Boolean(surface.forgetDisabledReason)} title={surface.forgetDisabledReason}
              aria-label={surface.forgetAriaLabel} onclick={() => void surface.onForget()}
            >{surface.tr("gui.settings.password_book.forget_current", "Forget current archive")}</button>
          {/if}
          <button class="primary-lite" class:classic-primary={surface.variant === "classic"} type="submit" aria-busy={surface.busy} disabled={surface.busy}>
            {surface.busy ? surface.tr("gui.password.unlocking", "Unlocking…") : surface.tr("gui.password.unlock_continue", "Unlock and continue")}
          </button>
        </footer>
      </form>
    {:else}
      <div class="password-request-body">
        <p>{surface.tr("gui.password.no_active_request", "No password request is active")}</p>
        <p>{surface.tr("gui.password.no_active_request_body", "Enter the password here when opening an encrypted archive or previewing a protected item.")}</p>
      </div>
      <footer class="modal-actions password-request-actions">
        <button onclick={surface.onBack}>{surface.tr("gui.nav.back_to_archive", "Back to archive")}</button>
      </footer>
    {/if}
  </section>
</div>
