<script lang="ts" module>
  import type { TaskConflictDecision } from "../lib/task-dialog";

  export type TaskInteractionWorkspaceKind = "password" | "conflict";
  export type TaskInteractionWorkspaceVariant = "modern" | "classic";

  type Tr = (key: string, fallback: string) => string;

  export interface PasswordInteractionSurface {
    kind: "password";
    variant: TaskInteractionWorkspaceVariant;
    tr: Tr;
    active: boolean;
    name: string;
    detail: string;
    sessionDetail: string;
    failureDetail: string;
    secretStoreLabel: string;
    value: string;
    busy: boolean;
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

  export interface ConflictInteractionSurface {
    kind: "conflict";
    variant: TaskInteractionWorkspaceVariant;
    tr: Tr;
    active: boolean;
    title: string;
    detail: string;
    rows: Array<{
      path: string;
      existing: string;
      incoming: string;
      decision: string;
    }>;
    applyAll: boolean;
    onApplyAllChange: (value: boolean) => void;
    onAnswer: (decision: TaskConflictDecision, applyAll: boolean) => void;
    onCancel: () => void;
    onBack: () => void;
  }

  export type TaskInteractionWorkspaceSurface =
    | PasswordInteractionSurface
    | ConflictInteractionSurface;
</script>

<script lang="ts">
  import Icon from "./Icon.svelte";

  let {
    surface,
  }: {
    surface: TaskInteractionWorkspaceSurface;
  } = $props();

  function registerPasswordInput(input: HTMLInputElement) {
    const onInputMount = surface.kind === "password" ? surface.onInputMount : null;
    onInputMount?.(input);
    return {
      destroy() {
        onInputMount?.(null);
      },
    };
  }

  function updatePassword(event: Event): void {
    if (surface.kind !== "password") return;
    surface.onValueChange((event.currentTarget as HTMLInputElement).value);
  }

  function updateApplyAll(event: Event): void {
    if (surface.kind !== "conflict") return;
    surface.onApplyAllChange((event.currentTarget as HTMLInputElement).checked);
  }
</script>

{#if surface.kind === "password" && surface.variant === "modern"}
  <div class="password-view modern-password">
    <div class="sheet-head compact-head">
      <div>
        {#if surface.active}
          <span class="eyebrow">{surface.tr("gui.password.required", "Password required")}</span>
          <h1>{surface.tr("gui.password.unlock_name", "Unlock {name}").replace("{name}", surface.name)}</h1>
          <p>{surface.detail}</p>
        {:else}
          <span class="eyebrow">{surface.tr("gui.password.empty_eyebrow", "Password")}</span>
          <h1>{surface.tr("gui.password.empty_title", "Password entry")}</h1>
          <p>{surface.tr("gui.password.empty_detail", "Squallz asks here only when an archive or task needs a password.")}</p>
        {/if}
      </div>
    </div>
    {#if surface.active}
      <form
        class="modal-preview password-sheet"
        onsubmit={(event) => {
          event.preventDefault();
          void surface.onSubmit();
        }}
      >
        <div class="password-lock"><Icon name="lock" size={24} /></div>
        <div>
          <span class="secure-label">{surface.tr("gui.password.password", "Password")}</span>
          <input
            use:registerPasswordInput
            class="secure-input"
            type="password"
            value={surface.value}
            disabled={surface.busy}
            autocomplete="current-password"
            aria-label={surface.tr("gui.password.archive_password", "Archive password")}
            aria-invalid={surface.error ? "true" : undefined}
            aria-describedby={surface.error ? "password-error-modern" : undefined}
            oninput={updatePassword}
          />
          {#if surface.error}
            <small id="password-error-modern" class="password-inline-error" role="alert">{surface.error}</small>
          {/if}
        </div>
        <div class="check-row"><Icon name="info" size={14} />{surface.sessionDetail}</div>
        <div class="password-policy">
          <strong>{surface.tr("gui.password.security_boundary", "Security boundary")}</strong>
          <span>{surface.tr("gui.password.manual_wins_body", "Manual password wins over saved password. Failed saved passwords fall back to this prompt.")}</span>
        </div>
        <div class="modal-actions">
          <button type="button" onclick={surface.onCancel}>{surface.tr("common.cancel", "Cancel")}</button>
          <button class="primary-lite" type="submit" aria-busy={surface.busy} disabled={surface.busy}>{surface.tr("gui.password.unlock_continue", "Unlock and continue")}</button>
        </div>
      </form>
    {:else}
      <div class="modal-preview empty-task-state">
        <div class="password-lock"><Icon name="lock" size={24} /></div>
        <div>
          <strong>{surface.tr("gui.password.no_active_request", "No password request is active")}</strong>
          <span>{surface.tr("gui.password.no_active_request_body", "Password entry appears when opening an encrypted archive or when an extract or test task asks for credentials.")}</span>
        </div>
        <div class="modal-actions">
          <button onclick={surface.onBack}>{surface.tr("gui.nav.back_to_archive", "Back to archive")}</button>
        </div>
      </div>
    {/if}
  </div>
{:else if surface.kind === "conflict" && surface.variant === "modern"}
  <div class="conflict-view modern-conflict">
    <div class="sheet-head compact-head">
      <div>
        <span class="eyebrow">{surface.tr("gui.screen.conflict", "Conflict handling")}</span>
        <h1>{surface.title}</h1>
        <p>{surface.detail}</p>
      </div>
    </div>
    {#if surface.active}
      <div class="conflict-table">
        <div class="conflict-head"><span>{surface.tr("common.path", "Path")}</span><span>{surface.tr("gui.conflict.existing", "Existing")}</span><span>{surface.tr("gui.conflict.incoming", "Incoming")}</span><span>{surface.tr("gui.conflict.decision", "Decision")}</span></div>
        {#each surface.rows as row}
          <div class="conflict-row">
            <strong>{row.path}</strong><span>{row.existing}</span><span>{row.incoming}</span><span class="decision-pill">{row.decision}</span>
          </div>
        {/each}
      </div>
      <label class="conflict-apply-all modern-conflict-apply-all">
        <input type="checkbox" checked={surface.applyAll} onchange={updateApplyAll} />
        <span>{surface.tr("gui.conflict.apply_remaining", "Apply this decision to remaining conflicts")}</span>
      </label>
      <div class="conflict-actions">
        <button onclick={surface.onCancel}>{surface.tr("gui.conflict.cancel_extraction", "Cancel extraction")}</button>
        <button onclick={() => surface.onAnswer("skip", surface.applyAll)}>{surface.tr("gui.conflict.skip", "Skip")}</button>
        <button class="conflict-danger" onclick={() => surface.onAnswer("overwrite", surface.applyAll)}>{surface.tr("gui.conflict.overwrite", "Replace")}</button>
        <button class="primary-lite" onclick={() => surface.onAnswer("rename", surface.applyAll)}>{surface.tr("gui.conflict.rename", "Keep both")}</button>
      </div>
    {:else}
      <div class="modal-preview empty-task-state">
        <div class="password-lock"><Icon name="file" size={24} /></div>
        <div>
          <strong>{surface.tr("gui.conflict.no_active_request", "No conflict request is active")}</strong>
          <span>{surface.tr("gui.conflict.no_active_request_body", "Conflict choices appear only when an extract task finds an existing file.")}</span>
        </div>
        <div class="modal-actions">
          <button onclick={surface.onBack}>{surface.tr("gui.nav.back_to_extract", "Back to Extract")}</button>
        </div>
      </div>
    {/if}
  </div>
{:else if surface.kind === "password"}
  <div class="classic-dialog-body">
    <section class="classic-extract-sheet classic-password">
      <header>
        <div>
          {#if surface.active}
            <h1>{surface.tr("gui.screen.password", "Password Required")}</h1>
            <p>{surface.tr("gui.password.prompt_boundary_body", "Unlock only the archive that requested credentials. No password is written to logs, settings, or task status.")}</p>
          {:else}
            <h1>{surface.tr("gui.password.empty_title", "Password entry")}</h1>
            <p>{surface.tr("gui.password.empty_detail", "Squallz asks here only when an archive or task needs a password.")}</p>
          {/if}
        </div>
        {#if surface.active}
          <button class="classic-primary" type="submit" form="classic-password-request" aria-busy={surface.busy} disabled={surface.busy}>{surface.tr("gui.password.unlock", "Unlock")}</button>
        {:else}
          <button onclick={surface.onBack}>{surface.tr("gui.nav.back_to_archive", "Back to archive")}</button>
        {/if}
      </header>

      {#if surface.active}
        <form
          id="classic-password-request"
          class="classic-password-grid"
          onsubmit={(event) => {
            event.preventDefault();
            void surface.onSubmit();
          }}
        >
          <section class="classic-password-panel">
            <h2>{surface.name}</h2>
            <div class="classic-form-grid compact">
              <div class="classic-label">{surface.tr("gui.password.password", "Password")}</div>
              <div class="classic-password-field">
                <input
                  use:registerPasswordInput
                  class="classic-input password-obscured"
                  type="password"
                  value={surface.value}
                  disabled={surface.busy}
                  autocomplete="current-password"
                  aria-label={surface.tr("gui.password.archive_password", "Archive password")}
                  aria-invalid={surface.error ? "true" : undefined}
                  aria-describedby={surface.error ? "password-error-classic" : undefined}
                  oninput={updatePassword}
                />
                {#if surface.error}
                  <small id="password-error-classic" class="password-inline-error" role="alert">{surface.error}</small>
                {/if}
              </div>
              <div class="classic-label">{surface.tr("gui.password.remember_short", "Remember")}</div><div class="classic-input">{surface.sessionDetail}</div>
              <div class="classic-label">{surface.tr("gui.password.fallback", "Fallback")}</div><div class="classic-input">{surface.tr("gui.password.manual_overrides_saved", "Manual input overrides saved password")}</div>
              <div class="classic-label">{surface.tr("gui.password.on_failure", "On failure")}</div><div class="classic-input accent">{surface.failureDetail}</div>
            </div>
            <div class="classic-extract-actions">
              <button type="button" onclick={surface.onCancel}>{surface.tr("common.cancel", "Cancel")}</button>
              {#if surface.forgetVisible}
                <button
                  type="button"
                  disabled={Boolean(surface.forgetDisabledReason)}
                  title={surface.forgetDisabledReason}
                  aria-label={surface.forgetAriaLabel}
                  onclick={() => void surface.onForget()}
                >{surface.tr("gui.settings.password_book.forget_current", "Forget current archive")}</button>
              {/if}
            </div>
          </section>
          <aside class="classic-password-panel">
            <h2>{surface.tr("gui.password.security_boundary", "Security boundary")}</h2>
            <div class="classic-mode-note no-margin">
              <strong>{surface.tr("gui.password.frontend_never_owns_saved", "Saved passwords stay in the system secret store.")}</strong>
              <span>{surface.tr("gui.password.secret_store_supplies_directly", "Squallz shows only their status; archive operations retrieve saved passwords when needed.")}</span>
            </div>
            <div class="repair-log">
              <span>{surface.tr("gui.password.manual_transient", "Manual password: user-entered, transient.")}</span>
              <span>{surface.tr("gui.password.session_zeroize", "Session cache: cleared on exit or when forgotten.")}</span>
              <span>{surface.tr("gui.password.keychain_opt_in", "{secretStore}: opt-in, per archive account.").replace("{secretStore}", surface.secretStoreLabel)}</span>
            </div>
          </aside>
        </form>
      {:else}
        <div class="classic-mode-note classic-task-empty">
          <strong>{surface.tr("gui.password.no_active_request", "No password request is active")}</strong>
          <span>{surface.tr("gui.password.no_active_request_body", "Password entry appears when opening an encrypted archive or when an extract or test task asks for credentials.")}</span>
        </div>
      {/if}
    </section>
  </div>
{:else}
  <div class="classic-dialog-body">
    <section class="classic-extract-sheet classic-conflict">
      <header>
        <div>
          <h1>{surface.tr("gui.screen.conflict", "Conflict Handling")}</h1>
          <p>{surface.detail}</p>
        </div>
        <div class="classic-button-row">
          {#if surface.active}
            <button onclick={surface.onCancel}>{surface.tr("gui.conflict.cancel_extraction", "Cancel extraction")}</button>
          {:else}
            <button onclick={surface.onBack}>{surface.tr("gui.nav.back_to_extract", "Back to Extract")}</button>
          {/if}
        </div>
      </header>

      {#if surface.active}
        <div class="classic-conflict-grid">
          <section>
            <h2>{surface.tr("gui.conflict.existing_files", "Existing files")}</h2>
            <div class="classic-conflict-table">
              <div><b>{surface.tr("common.path", "Path")}</b><b>{surface.tr("gui.conflict.existing", "Existing")}</b><b>{surface.tr("gui.conflict.incoming", "Incoming")}</b><b>{surface.tr("gui.conflict.decision", "Decision")}</b></div>
              {#each surface.rows as row}
                <div><strong>{row.path}</strong><span>{row.existing}</span><span>{row.incoming}</span><span class="decision-pill">{row.decision}</span></div>
              {/each}
            </div>
          </section>
          <aside>
            <h2>{surface.tr("gui.conflict.policy", "Policy")}</h2>
            <div class="classic-segments conflict-policy">
              <button onclick={() => surface.onAnswer("skip", surface.applyAll)}>{surface.tr("gui.conflict.skip", "Skip")}</button><button class="conflict-danger" onclick={() => surface.onAnswer("overwrite", surface.applyAll)}>{surface.tr("gui.conflict.overwrite", "Replace")}</button><button class="active" onclick={() => surface.onAnswer("rename", surface.applyAll)}>{surface.tr("gui.conflict.rename", "Keep both")}</button><span class="format-boundary-pill" role="note">{surface.tr("gui.extract.overwrite.ask", "Ask")}</span>
            </div>
            <label class="conflict-apply-all classic-conflict-apply-all">
              <input type="checkbox" checked={surface.applyAll} onchange={updateApplyAll} />
              <span>{surface.tr("gui.conflict.apply_remaining", "Apply this decision to remaining conflicts")}</span>
            </label>
            <div class="classic-mode-note no-margin">
              <strong>{surface.tr("gui.conflict.apply_all_explicit", "Apply to all is explicit.")}</strong>
              <span>{surface.tr("gui.conflict.dialog_boundary_body", "The decision never silently escapes this dialog; batch jobs preserve per-archive conflict state.")}</span>
            </div>
          </aside>
        </div>
      {:else}
        <div class="classic-mode-note classic-task-empty">
          <strong>{surface.tr("gui.conflict.no_active_request", "No conflict request is active")}</strong>
          <span>{surface.tr("gui.conflict.no_active_request_body", "Conflict choices appear only when an extract task finds an existing file.")}</span>
        </div>
      {/if}
    </section>
  </div>
{/if}
