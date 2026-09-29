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

{#if surface.kind === "password"}
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
          <p>{surface.tr("gui.password.no_active_request_body", "Password entry appears when opening an encrypted archive or when an extract or test task asks for credentials.")}</p>
        </div>
        <footer class="modal-actions password-request-actions">
          <button onclick={surface.onBack}>{surface.tr("gui.nav.back_to_archive", "Back to archive")}</button>
        </footer>
      {/if}
    </section>
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
