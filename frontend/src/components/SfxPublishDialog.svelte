<script lang="ts" module>
  export interface SfxPublishRequest {
    source: string;
    output: string;
    identity: string;
    notaryProfile: string;
  }
</script>

<script lang="ts">
  import { onMount, tick } from "svelte";
  import AppIcon from "./AppIcon.svelte";
  import Icon from "./Icon.svelte";
  import { cssVariables, type CssVariableMap } from "../lib/css-variables";
  import { errorSummary } from "../lib/error-presentation";
  import { basename as pathBaseName } from "../lib/format";
  import { currentLang, tFallback } from "../lib/i18n.svelte";
  import {
    ipc,
    isErrorDto,
    type MacosSfxPublisherStatusDto,
  } from "../lib/ipc";

  let {
    rootClass,
    rootVariables = {},
    source,
    output,
    initialIdentity = "",
    initialProfile = "",
    onSubmit,
    onCancel,
  }: {
    rootClass: string;
    rootVariables?: CssVariableMap;
    source: string;
    output: string;
    initialIdentity?: string;
    initialProfile?: string;
    onSubmit: (request: SfxPublishRequest) => Promise<string | null>;
    onCancel: () => void;
  } = $props();

  type PublisherState = "checking" | "available" | "missing_identity" | "unsupported" | "error";

  let card = $state<HTMLElement | null>(null);
  let identitySelect = $state<HTMLSelectElement | null>(null);
  let profileInput = $state<HTMLInputElement | null>(null);
  let publisherState = $state<PublisherState>("checking");
  let identities = $state<string[]>([]);
  let identity = $state("");
  let notaryProfile = $state("");
  let statusError = $state<string | null>(null);
  let submitError = $state<string | null>(null);
  let validationAttempted = $state(false);
  let submitting = $state(false);
  let statusGeneration = 0;

  let titleId = $derived(`sfx-publish-title-${currentLang()}`);
  let descriptionId = $derived(`sfx-publish-description-${currentLang()}`);
  let profileError = $derived(
    validationAttempted && notaryProfile.trim().length === 0
      ? tr("gui.sfx_publish.profile_required", "Enter an existing notarytool Keychain profile.")
      : null,
  );
  let canSubmit = $derived(
    publisherState === "available"
      && identity.length > 0
      && notaryProfile.trim().length > 0
      && !submitting,
  );

  function tr(key: string, fallback: string): string {
    return tFallback(key, fallback);
  }

  function previewStatus(): MacosSfxPublisherStatusDto | null {
    if (!import.meta.env.DEV || typeof window === "undefined") return null;
    const preview = new URLSearchParams(window.location.search).get("previewSfxPublisher");
    if (preview === "available") {
      return {
        available: true,
        status: "available",
        identities: ["Developer ID Application: Acme Studio (A1B2C3D4E5)"],
      };
    }
    if (preview === "missing") {
      return { available: false, status: "missing_identity", identities: [] };
    }
    return null;
  }

  async function refreshStatus(): Promise<void> {
    const generation = ++statusGeneration;
    publisherState = "checking";
    statusError = null;
    try {
      const result = previewStatus() ?? await ipc.getMacosSfxPublisherStatus();
      if (generation !== statusGeneration) return;
      identities = result.identities;
      if (!result.available || result.identities.length === 0) {
        identity = "";
        publisherState = result.status === "unsupported" ? "unsupported" : "missing_identity";
        return;
      }
      identity = result.identities.includes(identity) ? identity : result.identities[0];
      publisherState = "available";
      await tick();
      (identities.length > 1 ? identitySelect : profileInput)?.focus({ preventScroll: true });
    } catch (error) {
      if (generation !== statusGeneration) return;
      publisherState = "error";
      statusError = isErrorDto(error)
        ? errorSummary(error)
        : tr("gui.sfx_publish.status_failed", "Could not read signing identities from Keychain.");
    }
  }

  function focusableElements(): HTMLElement[] {
    if (!card) return [];
    return Array.from(
      card.querySelectorAll<HTMLElement>(
        'button:not(:disabled), input:not(:disabled), select:not(:disabled), [href], [tabindex]:not([tabindex="-1"])',
      ),
    ).filter((element) => !element.hasAttribute("hidden"));
  }

  function onKeydown(event: KeyboardEvent): void {
    if (event.key === "Escape" && !submitting) {
      event.preventDefault();
      onCancel();
      return;
    }
    if (event.key !== "Tab") return;
    const focusable = focusableElements();
    if (focusable.length === 0) {
      event.preventDefault();
      card?.focus();
      return;
    }
    const first = focusable[0];
    const last = focusable[focusable.length - 1];
    if (document.activeElement === card) {
      event.preventDefault();
      (event.shiftKey ? last : first).focus();
    } else if (event.shiftKey && document.activeElement === first) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault();
      first.focus();
    }
  }

  function updateIdentity(event: Event): void {
    identity = (event.currentTarget as HTMLSelectElement).value;
    submitError = null;
  }

  function updateProfile(event: Event): void {
    notaryProfile = (event.currentTarget as HTMLInputElement).value;
    submitError = null;
  }

  async function submit(): Promise<void> {
    validationAttempted = true;
    submitError = null;
    if (!canSubmit) {
      if (publisherState === "available" && identity.length === 0) {
        submitError = tr("gui.sfx_publish.identity_required", "Choose a Developer ID Application identity.");
      }
      return;
    }
    submitting = true;
    const failure = await onSubmit({
      source,
      output,
      identity,
      notaryProfile: notaryProfile.trim(),
    });
    if (failure) {
      submitError = failure;
      submitting = false;
    }
  }

  function stateTitle(): string {
    if (publisherState === "checking") return tr("gui.sfx_publish.checking", "Checking signing readiness");
    if (publisherState === "available") return tr("gui.sfx_publish.ready", "Developer ID identity found");
    if (publisherState === "missing_identity") return tr("gui.sfx_publish.missing_identity", "Developer ID certificate not found");
    if (publisherState === "unsupported") return tr("gui.sfx_publish.unsupported", "Publishing is available only on macOS");
    return tr("gui.sfx_publish.status_failed_title", "Signing readiness could not be checked");
  }

  function stateDetail(): string {
    if (publisherState === "checking") {
      return tr("gui.sfx_publish.checking_detail", "Reading code-signing identities from your login Keychain.");
    }
    if (publisherState === "available") {
      return tr("gui.sfx_publish.ready_detail", "Choose the identity that should sign this distributable copy.");
    }
    if (publisherState === "missing_identity") {
      return tr(
        "gui.sfx_publish.missing_identity_detail",
        "Install a Developer ID Application certificate in Keychain, then refresh this check.",
      );
    }
    if (publisherState === "unsupported") {
      return tr("gui.sfx_publish.unsupported_detail", "Open this self-extractor on a Mac configured for Developer ID distribution.");
    }
    return statusError ?? tr("gui.sfx_publish.status_failed", "Could not read signing identities from Keychain.");
  }

  onMount(() => {
    identity = initialIdentity;
    notaryProfile = initialProfile;
    void tick().then(() => card?.focus({ preventScroll: true }));
    void refreshStatus();
    return () => {
      statusGeneration += 1;
    };
  });
</script>

<section class={rootClass} use:cssVariables={rootVariables} role="presentation">
  <div
    bind:this={card}
    class="sfx-publish-card"
    role="dialog"
    aria-modal="true"
    aria-labelledby={titleId}
    aria-describedby={descriptionId}
    tabindex="-1"
    onkeydown={onKeydown}
  >
    <form
      class="sfx-publish-form"
      onsubmit={(event) => {
        event.preventDefault();
        void submit();
      }}
    >
    <header class="sfx-publish-head">
      <span class="sfx-publish-brand" aria-hidden="true">
        <AppIcon size={34} />
        <span><Icon name="shield-check" size={15} /></span>
      </span>
      <div>
        <span class="eyebrow">{tr("gui.sfx_publish.eyebrow", "Trusted distribution")}</span>
        <h2 id={titleId}>{tr("gui.sfx_publish.title", "Publish for macOS")}</h2>
        <p id={descriptionId}>
          {tr(
            "gui.sfx_publish.body",
            "Create a separate signed and notarized copy that macOS can verify before launch.",
          )}
        </p>
      </div>
      <button
        class="sfx-publish-close"
        type="button"
        disabled={submitting}
        aria-label={tr("common.cancel", "Cancel")}
        onclick={onCancel}
      ><Icon name="x" size={16} /></button>
    </header>

    <div class="sfx-publish-body">
      <ol class="sfx-publish-steps" aria-label={tr("gui.sfx_publish.steps", "Publishing stages")}>
        <li>
          <span><Icon name="shield-check" size={16} /></span>
          <div>
            <strong>{tr("gui.sfx_publish.step_sign", "Developer ID sign")}</strong>
            <small>{tr("gui.sfx_publish.step_sign_detail", "Nested runtime, then app")}</small>
          </div>
        </li>
        <li>
          <span><Icon name="external-link" size={16} /></span>
          <div>
            <strong>{tr("gui.sfx_publish.step_notarize", "Apple notarize")}</strong>
            <small>{tr("gui.sfx_publish.step_notarize_detail", "Wait for an Accepted result")}</small>
          </div>
        </li>
        <li>
          <span><Icon name="check-circle" size={16} /></span>
          <div>
            <strong>{tr("gui.sfx_publish.step_verify", "Staple and verify")}</strong>
            <small>{tr("gui.sfx_publish.step_verify_detail", "Gatekeeper and payload checks")}</small>
          </div>
        </li>
      </ol>

      <section
        class="sfx-publish-status"
        class:ready={publisherState === "available"}
        class:warning={publisherState === "missing_identity" || publisherState === "unsupported"}
        class:danger={publisherState === "error"}
        role={publisherState === "error" ? "alert" : "status"}
        aria-live="polite"
        aria-busy={publisherState === "checking"}
      >
        <span class="sfx-publish-status-icon">
          <Icon
            name={publisherState === "checking"
              ? "hourglass"
              : publisherState === "available"
                ? "check-circle"
                : publisherState === "error"
                  ? "x-circle"
                  : "alert-triangle"}
            size={17}
          />
        </span>
        <div>
          <strong>{stateTitle()}</strong>
          <span>{stateDetail()}</span>
        </div>
        {#if publisherState === "missing_identity" || publisherState === "error"}
          <button type="button" disabled={submitting} onclick={() => void refreshStatus()}>
            <Icon name="rotate-cw" size={14} />{tr("common.refresh", "Refresh")}
          </button>
        {/if}
      </section>

      <div class="sfx-publish-fields">
        <label class="sfx-publish-field">
          <span>{tr("gui.sfx_publish.identity", "Developer ID identity")}</span>
          <select
            bind:this={identitySelect}
            value={identity}
            disabled={publisherState !== "available" || submitting}
            onchange={updateIdentity}
          >
            {#if identities.length === 0}
              <option value="">{tr("gui.sfx_publish.no_identity", "No identity available")}</option>
            {:else}
              {#each identities as candidate}
                <option value={candidate}>{candidate}</option>
              {/each}
            {/if}
          </select>
          <small>{tr("gui.sfx_publish.identity_help", "Only Developer ID Application identities are shown.")}</small>
        </label>

        <label class="sfx-publish-field">
          <span>{tr("gui.sfx_publish.profile", "Notary Keychain profile")}</span>
          <input
            bind:this={profileInput}
            type="text"
            value={notaryProfile}
            disabled={publisherState !== "available" || submitting}
            autocomplete="off"
            spellcheck="false"
            placeholder={tr("gui.sfx_publish.profile_placeholder", "Example: squallz-notary")}
            aria-invalid={profileError ? "true" : undefined}
            aria-describedby={profileError ? "sfx-publish-profile-error" : "sfx-publish-profile-help"}
            oninput={updateProfile}
          />
          {#if profileError}
            <small id="sfx-publish-profile-error" class="sfx-publish-field-error" role="alert">{profileError}</small>
          {:else}
            <small id="sfx-publish-profile-help">
              {tr("gui.sfx_publish.profile_help", "Use a profile already stored by notarytool. It is verified when the task starts.")}
            </small>
          {/if}
        </label>
      </div>

      <dl class="sfx-publish-paths">
        <div>
          <dt>{tr("gui.sfx_publish.unsigned_source", "Unsigned source")}</dt>
          <dd><strong>{pathBaseName(source)}</strong><span>{source}</span></dd>
        </div>
        <div>
          <dt>{tr("gui.sfx_publish.published_copy", "Published copy")}</dt>
          <dd><strong>{pathBaseName(output)}</strong><span>{output}</span></dd>
        </div>
      </dl>

      <p class="sfx-publish-security-note">
        <Icon name="lock" size={14} />
        {tr(
          "gui.sfx_publish.security_note",
          "Squallz references your existing certificate and Keychain profile. Certificate passwords and Apple credentials are never requested or stored here.",
        )}
      </p>

      {#if submitError}
        <p class="sfx-publish-submit-error" role="alert">
          <Icon name="x-circle" size={15} />{submitError}
        </p>
      {/if}
    </div>

    <footer class="sfx-publish-actions">
      <button type="button" disabled={submitting} onclick={onCancel}>
        {tr("common.cancel", "Cancel")}
      </button>
      <button class="primary" type="submit" disabled={!canSubmit} aria-busy={submitting}>
        <Icon name={submitting ? "hourglass" : "shield-check"} size={15} />
        {submitting
          ? tr("gui.sfx_publish.starting", "Starting secure publication…")
          : tr("gui.sfx_publish.submit", "Sign, notarize, and publish")}
      </button>
    </footer>
    </form>
  </div>
</section>
