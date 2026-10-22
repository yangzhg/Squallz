<script lang="ts" module>
  import type { FormatCapabilityCard } from "../lib/ui-model";
  import type { ComponentProps } from "svelte";

  type ArchivePresetPickerComponent = typeof import("./ArchivePresetPicker.svelte").default;
  type CreateContentPolicyOptionsComponent = typeof import("./CreateContentPolicyOptions.svelte").default;
  type CreateOutputOptionsComponent = typeof import("./CreateOutputOptions.svelte").default;
  type CreatePlanReviewComponent = typeof import("./CreatePlanReview.svelte").default;
  type CreatePreflightStatusComponent = typeof import("./CreatePreflightStatus.svelte").default;
  type CreateProtectionOptionsComponent = typeof import("./CreateProtectionOptions.svelte").default;
  type CreateSetupSummaryComponent = typeof import("./CreateSetupSummary.svelte").default;
  type CreateSfxOptionsComponent = typeof import("./CreateSfxOptions.svelte").default;

  export type CreateWorkspaceVariant = "modern" | "classic";

  export interface CreateWorkspaceChoice {
    id: string;
    label: string;
    selected: boolean;
    disabled: boolean;
    title: string;
    ariaLabel: string;
    onSelect: () => void;
  }

  export interface CreateWorkspaceSurface {
    onReady?: () => void;
    tr: (key: string, fallback?: string) => string;
    profiles: CreateWorkspaceChoice[];
    formats: CreateWorkspaceChoice[];
    formatNote: string;
    sqzPayload: {
      label: string;
      options: CreateWorkspaceChoice[];
    } | null;
    compression: {
      level: number;
      detail: string;
      method: string;
      custom: {
        value: number;
        error: string;
        disabled: boolean;
        title: string;
        rangeAriaLabel: string;
        numberAriaLabel: string;
        onInput: (event: Event) => void;
        onChange: (event: Event) => void;
      } | null;
    };
    setupSummary: ComponentProps<CreateSetupSummaryComponent>;
    preset: ComponentProps<ArchivePresetPickerComponent>;
    advanced: {
      open: boolean;
      onToggle: (open: boolean) => void;
      onKeydown: (event: KeyboardEvent) => void;
    };
    output: ComponentProps<CreateOutputOptionsComponent>;
    content: ComponentProps<CreateContentPolicyOptionsComponent>;
    recovery: {
      capability: string;
      disabled: boolean;
      disabledReason: string;
      onOpen: () => void;
    };
    sfx: ComponentProps<CreateSfxOptionsComponent>;
    protection: ComponentProps<CreateProtectionOptionsComponent>;
    showPreflight: boolean;
    preflight: ComponentProps<CreatePreflightStatusComponent>;
    review: ComponentProps<CreatePlanReviewComponent> | null;
    classic: {
      archiveName: string;
      activeSection: string;
      sections: Array<{
        id: string;
        label: string;
        targetId: string;
        onSelect: () => void;
      }>;
      recoveryCapability: string;
      updateMode: string;
      featuredFormats: FormatCapabilityCard[];
    };
  }
</script>

<script lang="ts">
  import { onMount } from "svelte";
  import type { CreateSession } from "../lib/create-session.svelte";
  import ArchivePresetPicker from "./ArchivePresetPicker.svelte";
  import CreateContentPolicyOptions from "./CreateContentPolicyOptions.svelte";
  import CreateOutputOptions from "./CreateOutputOptions.svelte";
  import CreatePlanReview from "./CreatePlanReview.svelte";
  import CreatePreflightStatus from "./CreatePreflightStatus.svelte";
  import CreateProtectionOptions from "./CreateProtectionOptions.svelte";
  import CreateSetupSummary from "./CreateSetupSummary.svelte";
  import CreateSourceList from "./CreateSourceList.svelte";
  import CreateSfxOptions from "./CreateSfxOptions.svelte";
  import Icon from "./Icon.svelte";

  let {
    variant,
    session,
  }: {
    variant: CreateWorkspaceVariant;
    session: CreateSession;
  } = $props();
  let surface = $derived(session.view.surface(variant));
  let reviewReason = $derived(session.sources.length === 0
    ? surface.tr("gui.create.sources.add_before_review")
    : session.configurationPending
      ? session.view.configurationPendingMessage()
      : session.view.sourcesLockedReason());
  onMount(() => surface.onReady?.());
</script>

{#snippet createAction()}
  {#if !surface.review}
    <footer class="workspace-action-bar" class:classic={variant === "classic"}>
      <div class="workspace-action-summary">
        <span>{surface.tr("gui.create.output.preview")}</span>
        <strong>{surface.output.outputPreview}</strong>
        <small>{reviewReason || surface.tr("gui.create.sources.review_hint")}</small>
      </div>
      <button
        id="create-primary-source-action"
        type="button"
        class:primary={variant === "modern"}
        class:classic-primary={variant === "classic"}
        disabled={session.view.startDisabled()}
        aria-busy={session.preflight.busy()}
        title={reviewReason}
        onclick={() => void session.review()}
      ><Icon name="search" size={16} />{session.view.startLabel(surface.tr("gui.create.sources.review"))}</button>
    </footer>
  {/if}
{/snippet}

{#if variant === "modern"}
  <div class="create-sheet modern-create">
    <div class="sheet-head">
      <div>
        <h1>{surface.tr("gui.create.real_preflight_title")}</h1>
        <p>{surface.tr("gui.create.real_preflight_body")}</p>
      </div>
    </div>
    <CreateSourceList variant="modern" {session} />

    <div class="create-workflow">
      <section class="create-main-panel">
        <div class="create-settings-section">
          <div class="create-core-settings">
            <div class="create-choice-line">
              <span class="create-choice-label" aria-hidden="true">
                {surface.tr("gui.create.compression_presets")}
              </span>
              <div class="preset-row" aria-label={surface.tr("gui.create.compression_presets")}>
                {#each surface.profiles as profile (profile.id)}
                  <button
                    class:selected={profile.selected}
                    aria-pressed={profile.selected}
                    disabled={profile.disabled}
                    title={profile.title}
                    aria-label={profile.ariaLabel}
                    onclick={profile.onSelect}
                  >{profile.label}</button>
                {/each}
              </div>
            </div>

            <div class="create-choice-line">
              <span class="create-choice-label" aria-hidden="true">
                {surface.tr("gui.create.archive_format")}
              </span>
              <div class="format-segments" aria-label={surface.tr("gui.create.archive_format")}>
                {#each surface.formats as format (format.id)}
                  <button
                    class:selected={format.selected}
                    aria-pressed={format.selected}
                    disabled={format.disabled}
                    title={format.title}
                    aria-label={format.ariaLabel}
                    onclick={format.onSelect}
                  >{format.label}</button>
                {/each}
                <span
                  class="format-boundary-pill"
                  role="note"
                  title={surface.tr("gui.create.rar_not_launch_claim")}
                >{surface.tr("gui.create.rar_read_only")}</span>
              </div>
            </div>
            <div class="format-note">{surface.formatNote}</div>

            {#if surface.sqzPayload}
              <div class="field-label">{surface.sqzPayload.label}</div>
              <div class="format-segments" role="group" aria-label={surface.sqzPayload.label}>
                {#each surface.sqzPayload.options as option (option.id)}
                  <button
                    type="button"
                    class:selected={option.selected}
                    aria-pressed={option.selected}
                    disabled={option.disabled}
                    title={option.title}
                    aria-label={option.ariaLabel}
                    onclick={option.onSelect}
                  >{option.label}</button>
                {/each}
              </div>
            {/if}

            <div class="level-control">
              <div>
                <strong>{surface.tr("gui.create.compression_level").replace("{level}", String(surface.compression.level))}</strong>
                <span>{surface.compression.detail}</span>
              </div>
              {#if surface.compression.custom}
                <div class="custom-level-row">
                  <input
                    class="custom-level-range"
                    type="range"
                    min="1"
                    max="9"
                    value={surface.compression.custom.value}
                    disabled={surface.compression.custom.disabled}
                    title={surface.compression.custom.title}
                    aria-label={surface.compression.custom.rangeAriaLabel}
                    oninput={surface.compression.custom.onInput}
                    onchange={surface.compression.custom.onChange}
                  />
                  <input
                    class="custom-level-number"
                    class:invalid={surface.compression.custom.error.length > 0}
                    type="number"
                    min="1"
                    max="9"
                    step="1"
                    inputmode="numeric"
                    value={surface.compression.custom.value}
                    disabled={surface.compression.custom.disabled}
                    title={surface.compression.custom.title}
                    aria-label={surface.compression.custom.numberAriaLabel}
                    aria-invalid={surface.compression.custom.error ? "true" : "false"}
                    aria-describedby={surface.compression.custom.error ? "custom-create-level-error-modern" : undefined}
                    oninput={surface.compression.custom.onInput}
                    onchange={surface.compression.custom.onChange}
                  />
                </div>
                {#if surface.compression.custom.error}
                  <small id="custom-create-level-error-modern" class="custom-level-error" role="status" data-custom-level-error>
                    {surface.compression.custom.error}
                  </small>
                {/if}
              {/if}
            </div>
          </div>

        </div>

        <CreateOutputOptions {...surface.output} />

        <details
          class="create-advanced-disclosure"
          open={surface.advanced.open}
          ontoggle={(event) => surface.advanced.onToggle(event.currentTarget.open)}
        >
          <summary onkeydown={surface.advanced.onKeydown}>
            <span class="create-advanced-summary-icon"><Icon name="settings" size={17} /></span>
            <span>
              <strong>{surface.tr("gui.create.advanced.title")}</strong>
              <small>{surface.tr("gui.create.advanced.detail")}</small>
            </span>
            <Icon name="chevron-down" size={16} class="create-advanced-chevron" />
          </summary>
          <div class="create-advanced-grid">
            <div class="create-advanced-column create-advanced-column-primary">
              <CreateProtectionOptions {...surface.protection} />
              <CreateContentPolicyOptions {...surface.content} />
              <div class="recovery-callout">
                <div>
                  <span class="block-label">{surface.tr("gui.recovery.title")}</span>
                  <strong>{surface.recovery.capability}</strong>
                  <p>{surface.tr("gui.create.recovery_separate_jobs", "Creating the archive and generating recovery data are separate jobs; use Recovery when you want PAR2 or SQZ repair evidence.")}</p>
                </div>
                <button
                  disabled={surface.recovery.disabled}
                  title={surface.recovery.disabledReason}
                  onclick={surface.recovery.onOpen}
                >{surface.tr("common.change")}</button>
              </div>
            </div>
            <aside class="create-advanced-column create-advanced-column-secondary">
              <CreateSfxOptions {...surface.sfx} />
              <ArchivePresetPicker {...surface.preset} compact />
            </aside>
          </div>
        </details>

        <details class="workspace-summary-disclosure">
          <summary>{surface.setupSummary.heading}</summary>
          <CreateSetupSummary {...surface.setupSummary} />
        </details>

        {#if surface.showPreflight && (!surface.review || surface.review.busy)}
          <CreatePreflightStatus {...surface.preflight} />
        {/if}

        {#if surface.review}
          <CreatePlanReview {...surface.review} />
        {/if}
      </section>
    </div>
    {@render createAction()}
  </div>
{:else}
  <div class="classic-dialog-body">
    <section class="classic-property-sheet classic-create">
      <header>
        <div>
          <h1>{surface.tr("gui.create.add_to_archive")}</h1>
          <p>{surface.tr("gui.create.classic_intro")}</p>
        </div>
      </header>
      <CreateSourceList variant="classic" {session} />

      <div class="classic-tabs" role="group" aria-label={surface.tr("gui.create.sections")}>
        {#each surface.classic.sections as section (section.id)}
          <button
            type="button"
            class:active={surface.classic.activeSection === section.id}
            aria-pressed={surface.classic.activeSection === section.id}
            onclick={section.onSelect}
          >{section.label}</button>
        {/each}
      </div>

      <div class="classic-form-grid">
        <div id="classic-create-general" class="classic-label classic-create-section-target">
          {surface.tr("gui.create.archive_name")}
        </div>
        <div class="classic-input classic-copy-wrap">{surface.classic.archiveName}</div>
        <CreateOutputOptions {...surface.output} />
        <div class="classic-label">{surface.tr("gui.create.archive_format")}</div>
        <div class="classic-segments" aria-label={surface.tr("gui.create.classic_archive_format")}>
          {#each surface.formats as format (format.id)}
            <button
              class:active={format.selected}
              aria-pressed={format.selected}
              disabled={format.disabled}
              title={format.title}
              aria-label={format.ariaLabel}
              onclick={format.onSelect}
            >{format.label}</button>
          {/each}
          <span
            class="format-boundary-pill"
            role="note"
            title={surface.tr("gui.create.rar_not_launch_claim")}
          >{surface.tr("gui.create.rar_read_only")}</span>
        </div>
        <div class="classic-label">{surface.tr("gui.create.format_boundary")}</div>
        <div class="classic-input accent classic-copy-wrap">{surface.formatNote}</div>
        {#if surface.sqzPayload}
          <div class="classic-label">{surface.sqzPayload.label}</div>
          <div class="classic-segments" role="group" aria-label={surface.sqzPayload.label}>
            {#each surface.sqzPayload.options as option (option.id)}
              <button
                type="button"
                class:active={option.selected}
                aria-pressed={option.selected}
                disabled={option.disabled}
                title={option.title}
                aria-label={option.ariaLabel}
                onclick={option.onSelect}
              >{option.label}</button>
            {/each}
          </div>
        {/if}
        <div id="classic-create-compression" class="classic-label classic-create-section-target">
          {surface.tr("gui.create.compression_profile")}
        </div>
        <div class="classic-segments classic-profile-segments">
          {#each surface.profiles as profile (profile.id)}
            <button
              class:active={profile.selected}
              aria-pressed={profile.selected}
              disabled={profile.disabled}
              title={profile.title}
              aria-label={profile.ariaLabel}
              onclick={profile.onSelect}
            >{profile.label}</button>
          {/each}
        </div>
        <div class="classic-label">{surface.tr("gui.create.compression_method")}</div>
        <div class="classic-input classic-copy-wrap">{surface.compression.method}</div>
        {#if surface.compression.custom}
          <div class="classic-label">{surface.tr("gui.create.custom_level")}</div>
          <div class="classic-input classic-custom-level">
            <input
              type="range"
              min="1"
              max="9"
              value={surface.compression.custom.value}
              disabled={surface.compression.custom.disabled}
              title={surface.compression.custom.title}
              aria-label={surface.compression.custom.rangeAriaLabel}
              oninput={surface.compression.custom.onInput}
              onchange={surface.compression.custom.onChange}
            />
            <input
              type="number"
              class:invalid={surface.compression.custom.error.length > 0}
              min="1"
              max="9"
              step="1"
              inputmode="numeric"
              value={surface.compression.custom.value}
              disabled={surface.compression.custom.disabled}
              title={surface.compression.custom.title}
              aria-label={surface.compression.custom.numberAriaLabel}
              aria-invalid={surface.compression.custom.error ? "true" : "false"}
              aria-describedby={surface.compression.custom.error ? "custom-create-level-error-classic" : undefined}
              oninput={surface.compression.custom.onInput}
              onchange={surface.compression.custom.onChange}
            />
          </div>
          {#if surface.compression.custom.error}
            <div></div>
            <small
              id="custom-create-level-error-classic"
              class="classic-input custom-level-error"
              role="status"
              data-custom-level-error
            >{surface.compression.custom.error}</small>
          {/if}
        {/if}
        <CreateContentPolicyOptions {...surface.content} />
        <CreateSfxOptions {...surface.sfx} />
        <CreateProtectionOptions {...surface.protection} />
        <div id="classic-create-recovery" class="classic-label classic-create-section-target">
          {surface.tr("gui.recovery.title")}
        </div>
        <div class="classic-input accent classic-copy-wrap">{surface.classic.recoveryCapability}</div>
        <div class="classic-label">{surface.tr("gui.create.update_mode")}</div>
        <div class="classic-input classic-copy-wrap">{surface.classic.updateMode}</div>
        <div id="classic-create-preflight" class="classic-create-preflight classic-create-section-target">
          <CreatePreflightStatus {...surface.preflight} />
        </div>
      </div>

      {#if surface.review}
        <div class="classic-plan-review-wrap">
          <CreatePlanReview {...surface.review} />
        </div>
      {/if}

      <div class="classic-preset-wrap">
        <ArchivePresetPicker {...surface.preset} compact />
      </div>
      <details class="workspace-summary-disclosure classic-create-summary-wrap">
        <summary>{surface.setupSummary.heading}</summary>
        <CreateSetupSummary {...surface.setupSummary} />
      </details>
      {@render createAction()}
    </section>
  </div>
{/if}
