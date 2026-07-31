<script lang="ts" module>
  import type { ComponentProps } from "svelte";
  import type { CreateSourceListSurface } from "./CreateSourceList.svelte";

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
    tr: (key: string, fallback: string) => string;
    sources: CreateSourceListSurface;
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
      featuredFormats: Array<{
        name: string;
        state: string;
        create: string;
        volumes: string;
        encrypt: string;
        note: string;
      }>;
    };
  }
</script>

<script lang="ts">
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
    surface,
  }: {
    variant: CreateWorkspaceVariant;
    surface: CreateWorkspaceSurface;
  } = $props();
</script>

{#if variant === "modern"}
  <div class="create-sheet modern-create">
    <div class="sheet-head">
      <div>
        <span class="eyebrow">{surface.tr("gui.create.eyebrow", "Create archive")}</span>
        <h1>{surface.tr("gui.create.real_preflight_title", "Create an archive")}</h1>
        <p>{surface.tr("gui.create.real_preflight_body", "Add sources, choose a format and options, then review safety checks.")}</p>
      </div>
    </div>
    <CreateSourceList variant="modern" surface={surface.sources} />

    <div class="create-workflow">
      <section class="create-main-panel">
        <div class="create-essentials-grid">
          <div class="create-core-settings">
            <div class="create-choice-line">
              <span class="create-choice-label" aria-hidden="true">
                {surface.tr("gui.create.compression_presets", "Compression level")}
              </span>
              <div class="preset-row" aria-label={surface.tr("gui.create.compression_presets", "Compression level")}>
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
                {surface.tr("gui.create.archive_format", "Archive format")}
              </span>
              <div class="format-segments" aria-label={surface.tr("gui.create.archive_format", "Archive format")}>
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
                  title={surface.tr("gui.create.rar_not_launch_claim", "Squallz does not create RAR archives")}
                >{surface.tr("gui.create.rar_read_only", "RAR · Extract only")}</span>
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
                <strong>{surface.tr("gui.create.compression_level", "Compression level {level}").replace("{level}", String(surface.compression.level))}</strong>
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

          <CreateSetupSummary {...surface.setupSummary} />
        </div>

        <ArchivePresetPicker {...surface.preset} />

        <details
          class="create-advanced-disclosure"
          open={surface.advanced.open}
          ontoggle={(event) => surface.advanced.onToggle(event.currentTarget.open)}
        >
          <summary onkeydown={surface.advanced.onKeydown}>
            <span class="create-advanced-summary-icon"><Icon name="settings" size={17} /></span>
            <span>
              <strong>{surface.tr("gui.create.advanced.title", "More options")}</strong>
              <small>{surface.tr("gui.create.advanced.detail", "Save location, completion, source handling, content rules, self-extracting, password, split volumes, and recovery")}</small>
            </span>
            <Icon name="chevron-down" size={16} class="create-advanced-chevron" />
          </summary>
          <div class="create-advanced-grid">
            <div class="create-advanced-column create-advanced-column-primary">
              <CreateOutputOptions {...surface.output} />
              <CreateContentPolicyOptions {...surface.content} />
              <div class="recovery-callout">
                <div>
                  <span class="block-label">{surface.tr("gui.recovery.title", "Recovery")}</span>
                  <strong>{surface.recovery.capability}</strong>
                  <p>{surface.tr("gui.create.recovery_separate_jobs", "Creating the archive and generating recovery data are separate jobs; use Recovery when you want PAR2 or SQZ repair evidence.")}</p>
                </div>
                <button
                  disabled={surface.recovery.disabled}
                  title={surface.recovery.disabledReason}
                  onclick={surface.recovery.onOpen}
                >{surface.tr("common.change", "Change")}</button>
              </div>
            </div>
            <aside class="create-advanced-column create-advanced-column-secondary">
              <CreateSfxOptions {...surface.sfx} />
              <CreateProtectionOptions {...surface.protection} />
            </aside>
          </div>
        </details>

        {#if surface.showPreflight && !surface.review}
          <CreatePreflightStatus {...surface.preflight} />
        {/if}

        {#if surface.review}
          <CreatePlanReview {...surface.review} />
        {/if}
      </section>
    </div>
  </div>
{:else}
  <div class="classic-dialog-body">
    <section class="classic-property-sheet classic-create">
      <header>
        <div>
          <h1>{surface.tr("gui.create.add_to_archive", "Add to archive")}</h1>
          <p>{surface.tr("gui.create.classic_intro", "Build the source list, set the format and options, then review before creating.")}</p>
        </div>
      </header>
      <CreateSourceList variant="classic" surface={surface.sources} />

      <div class="classic-preset-wrap">
        <ArchivePresetPicker {...surface.preset} />
      </div>

      <div class="classic-create-summary-wrap">
        <CreateSetupSummary {...surface.setupSummary} />
      </div>

      <div class="classic-tabs" role="group" aria-label={surface.tr("gui.create.sections", "Create sections")}>
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
          {surface.tr("gui.create.archive_name", "Archive name")}
        </div>
        <div class="classic-input">{surface.classic.archiveName}</div>
        <CreateOutputOptions {...surface.output} />
        <div class="classic-label">{surface.tr("gui.create.archive_format", "Archive format")}</div>
        <div class="classic-segments" aria-label={surface.tr("gui.create.classic_archive_format", "Classic archive format")}>
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
            title={surface.tr("gui.create.rar_not_launch_claim", "Squallz does not create RAR archives")}
          >{surface.tr("gui.create.rar_read_only", "RAR · Extract only")}</span>
        </div>
        <div class="classic-label">{surface.tr("gui.create.format_boundary", "Format boundary")}</div>
        <div class="classic-input accent">{surface.formatNote}</div>
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
          {surface.tr("gui.create.compression_profile", "Compression profile")}
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
        <div class="classic-label">{surface.tr("gui.create.compression_method", "Compression method")}</div>
        <div class="classic-input">{surface.compression.method}</div>
        {#if surface.compression.custom}
          <div class="classic-label">{surface.tr("gui.create.custom_level", "Custom level")}</div>
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
          {surface.tr("gui.recovery.title", "Recovery")}
        </div>
        <div class="classic-input accent">{surface.classic.recoveryCapability}</div>
        <div class="classic-label">{surface.tr("gui.create.update_mode", "Update mode")}</div>
        <div class="classic-input">{surface.classic.updateMode}</div>
        <div id="classic-create-preflight" class="classic-create-preflight classic-create-section-target">
          <CreatePreflightStatus {...surface.preflight} />
        </div>
      </div>

      {#if surface.review}
        <div class="classic-plan-review-wrap">
          <CreatePlanReview {...surface.review} />
        </div>
      {/if}

      <div class="classic-capability-grid">
        {#each surface.classic.featuredFormats as format (format.name)}
          <div>
            <strong>{format.name}</strong>
            <span>{format.state}</span>
            <small>{surface.tr("gui.format.card_capability_line", "Create {create} · Split {volumes} · Encrypt {encrypt}")
              .replace("{create}", format.create)
              .replace("{volumes}", format.volumes)
              .replace("{encrypt}", format.encrypt)}</small>
            <em>{format.note}</em>
          </div>
        {/each}
      </div>
    </section>
  </div>
{/if}
