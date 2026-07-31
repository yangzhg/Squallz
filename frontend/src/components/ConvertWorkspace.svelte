<script lang="ts" module>
  import type { ComponentProps } from "svelte";
  import type { CreatePlanDto, DiskSpaceDto } from "../lib/ipc";

  type CreateProtectionOptionsComponent = typeof import("./CreateProtectionOptions.svelte").default;
  type ConvertPreflightPhase =
    | "idle"
    | "choosingDest"
    | "measuring"
    | "checkingTemp"
    | "checkingDest"
    | "reviewing"
    | "submitting"
    | "ready"
    | "cancelled"
    | "blocked";
  type ConvertPreflightStage = "source" | "temp" | "destination" | "submit";

  export type ConvertWorkspaceVariant = "modern" | "classic";

  export interface ConvertWorkspaceChoice {
    id: string;
    label: string;
    selected: boolean;
    disabled: boolean;
    title: string;
    ariaLabel: string;
    onSelect: () => void;
  }

  export interface ConvertWorkspaceSurface {
    tr: (key: string, fallback: string) => string;
    start: {
      label: string;
      disabled: boolean;
      title: string;
      ariaLabel: string;
      busy: boolean;
      onSelect: () => void;
    };
    source: {
      path: string;
      format: string;
      summary: string;
    };
    destination: {
      path: string;
    };
    formats: ConvertWorkspaceChoice[];
    formatNote: string;
    profiles: ConvertWorkspaceChoice[];
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
    advanced: {
      open: boolean;
      detail: string;
      onToggle: (open: boolean) => void;
      onKeydown: (event: KeyboardEvent) => void;
    };
    protection: ComponentProps<CreateProtectionOptionsComponent>;
    contract: {
      title: string;
      body: string;
    };
    readiness: {
      title: string;
      state: string;
      body: string;
    };
    guard: {
      title: string;
      body: string;
    };
    showPreflight: boolean;
    preflight: {
      phase: ConvertPreflightPhase;
      requestKind: "plan" | "destination" | null;
      cancelPending: boolean;
      current: string;
      issue: string;
      issueStage: ConvertPreflightStage | null;
      lockedReason: string;
      cancellable: boolean;
      destination: string | null;
      plan: CreatePlanDto | null;
      workspaceDisk: DiskSpaceDto | null;
      systemTempDisk: DiskSpaceDto | null;
      destinationDisk: DiskSpaceDto | null;
      onCancel: () => void;
    };
    review: {
      plan: CreatePlanDto;
      splitSize: number | null;
      issue: string;
      busy: boolean;
      retry: boolean;
      onConfirm: () => void;
      onCancel: () => void;
    } | null;
  }
</script>

<script lang="ts">
  import CreatePlanReview from "./CreatePlanReview.svelte";
  import CreatePreflightStatus from "./CreatePreflightStatus.svelte";
  import CreateProtectionOptions from "./CreateProtectionOptions.svelte";
  import Icon from "./Icon.svelte";
  import { basename as pathBaseName, formatBytes } from "../lib/format";

  let {
    variant,
    surface,
  }: {
    variant: ConvertWorkspaceVariant;
    surface: ConvertWorkspaceSurface;
  } = $props();

  let customLevelErrorId = $derived(`${variant}-convert-custom-level-error`);

  type StepState = "pending" | "active" | "ready" | "blocked" | "cancelled";

  function preflightPhaseLabel(): string {
    const preflight = surface.preflight;
    if (preflight.cancelPending) {
      return surface.tr("gui.convert.preflight_cancelling", "Stopping the current check");
    }
    switch (preflight.phase) {
      case "choosingDest":
        return preflight.requestKind === "destination"
          ? surface.tr("gui.convert.preflight_checking_output", "Checking the current output")
          : surface.tr("gui.convert.preflight_waiting_destination", "Waiting for destination");
      case "measuring":
        return surface.tr("gui.convert.preflight_reading_archive", "Reading archive metadata");
      case "checkingTemp":
        return surface.tr("gui.convert.preflight_checking_workspace", "Checking conversion workspace");
      case "checkingDest":
        return surface.tr("gui.convert.preflight_checking_destination", "Checking destination disk");
      case "reviewing":
        return surface.tr("gui.convert.preflight_ready_for_review", "Ready for review");
      case "submitting":
        return preflight.requestKind === "destination"
          ? surface.tr("gui.convert.preflight_rechecking_output", "Rechecking the current output")
          : surface.tr("gui.convert.preflight_submitting", "Adding conversion to the queue");
      case "ready":
        return surface.tr("gui.convert.preflight_ready", "Conversion queued");
      case "cancelled":
        return surface.tr("gui.convert.preflight_cancelled", "Checks cancelled");
      case "blocked":
        return surface.tr("gui.convert.preflight_blocked", "Checks need attention");
      case "idle":
        return surface.tr("gui.convert.preflight_pending", "Checks pending");
    }
  }

  function preflightStepState(
    stage: Exclude<ConvertPreflightStage, "submit">,
  ): StepState {
    const preflight = surface.preflight;
    if (preflight.issueStage === stage) {
      return preflight.phase === "cancelled" ? "cancelled" : "blocked";
    }
    if (stage === "source") {
      if (preflight.phase === "measuring") return "active";
      return preflight.plan ? "ready" : "pending";
    }
    if (stage === "temp") {
      if (preflight.phase === "checkingTemp") return "active";
      if (preflight.workspaceDisk) {
        return preflight.workspaceDisk.ok && (preflight.systemTempDisk?.ok ?? true)
          ? "ready"
          : "blocked";
      }
      return "pending";
    }
    if (
      preflight.phase === "choosingDest"
      || preflight.phase === "checkingDest"
      || (preflight.phase === "submitting" && preflight.requestKind === "destination")
    ) return "active";
    if (preflight.destinationDisk) return preflight.destinationDisk.ok ? "ready" : "blocked";
    return "pending";
  }

  function preflightStepStateLabel(state: StepState): string {
    if (state === "active") return surface.tr("gui.convert.preflight_stage_active", "In progress");
    if (state === "ready") return surface.tr("gui.convert.preflight_stage_ready", "Checked");
    if (state === "blocked") return surface.tr("gui.convert.preflight_stage_blocked", "Blocked");
    if (state === "cancelled") return surface.tr("gui.convert.preflight_stage_cancelled", "Cancelled");
    return surface.tr("gui.convert.preflight_stage_pending", "Pending");
  }

  function preflightIssueSummary(
    stage: Exclude<ConvertPreflightStage, "submit">,
  ): string | null {
    const preflight = surface.preflight;
    if (preflight.issueStage !== stage) return null;
    return preflight.phase === "cancelled"
      ? surface.tr("gui.convert.preflight_stage_cancelled_summary", "Cancelled before this check completed")
      : surface.tr("gui.convert.preflight_stage_blocked_summary", "This check could not finish");
  }

  function archivePreflightSummary(): string {
    const preflight = surface.preflight;
    const interrupted = preflightIssueSummary("source");
    if (interrupted) return interrupted;
    if (preflight.phase === "measuring") {
      return surface.tr("gui.convert.preflight_archive_metadata_pending", "Reading entries without extracting files…");
    }
    if (!preflight.plan) {
      return surface.tr("gui.convert.preflight_archive_pending", "Archive measurement pending");
    }
    return surface.tr("gui.convert.preflight_archive_value", "{size} · {entries} entries")
      .replace("{size}", formatBytes(preflight.plan.total_bytes))
      .replace("{entries}", preflight.plan.entries.toLocaleString());
  }

  function workspacePreflightSummary(): string {
    const preflight = surface.preflight;
    const interrupted = preflightIssueSummary("temp");
    if (interrupted) return interrupted;
    if (preflight.phase === "checkingTemp") {
      return surface.tr("gui.convert.preflight_workspace_checking", "Checking peak workspace…");
    }
    if (!preflight.workspaceDisk) {
      return surface.tr("gui.convert.preflight_workspace_pending", "Workspace check pending");
    }
    if (preflight.systemTempDisk) {
      const ok = preflight.workspaceDisk.ok && preflight.systemTempDisk.ok;
      return surface.tr(
        "gui.convert.preflight_workspace_split",
        "{status} · destination {destination} · temporary {temporary}",
      )
        .replace(
          "{status}",
          ok
            ? surface.tr("gui.convert.preflight_workspace_ok", "Workspace OK")
            : surface.tr("gui.convert.preflight_workspace_blocked", "Workspace blocked"),
        )
        .replace("{destination}", formatBytes(preflight.workspaceDisk.available_bytes))
        .replace("{temporary}", formatBytes(preflight.systemTempDisk.available_bytes));
    }
    return surface.tr("gui.convert.preflight_space_available", "{status} · {available} available")
      .replace(
        "{status}",
        preflight.workspaceDisk.ok
          ? surface.tr("gui.convert.preflight_workspace_ok", "Workspace OK")
          : surface.tr("gui.convert.preflight_workspace_blocked", "Workspace blocked"),
      )
      .replace("{available}", formatBytes(preflight.workspaceDisk.available_bytes));
  }

  function destinationPreflightSummary(): string {
    const preflight = surface.preflight;
    const interrupted = preflightIssueSummary("destination");
    if (interrupted) return interrupted;
    if (preflight.phase === "choosingDest") {
      return preflight.requestKind === "destination"
        ? surface.tr("gui.convert.preflight_output_checking", "Checking the existing output set…")
        : surface.tr("gui.convert.preflight_destination_waiting", "Waiting for a save location…");
    }
    if (preflight.phase === "checkingDest") {
      return surface.tr("gui.convert.preflight_destination_checking", "Checking final output space…");
    }
    if (!preflight.destinationDisk) {
      return surface.tr("gui.convert.preflight_destination_pending", "Destination check pending");
    }
    return surface.tr("gui.convert.preflight_space_available", "{status} · {available} available")
      .replace(
        "{status}",
        preflight.destinationDisk.ok
          ? surface.tr("gui.convert.preflight_destination_ok", "Destination OK")
          : surface.tr("gui.convert.preflight_destination_blocked", "Destination blocked"),
      )
      .replace("{available}", formatBytes(preflight.destinationDisk.available_bytes));
  }

  function preflightSteps() {
    const sourceState = preflightStepState("source");
    const workspaceState = preflightStepState("temp");
    const destinationState = preflightStepState("destination");
    return [
      {
        id: "source",
        label: surface.tr("gui.convert.preflight_archive", "Archive metadata"),
        summary: archivePreflightSummary(),
        detail: "",
        state: sourceState,
        stateLabel: preflightStepStateLabel(sourceState),
      },
      {
        id: "temp",
        label: surface.tr("gui.convert.preflight_workspace", "Workspace peak"),
        summary: workspacePreflightSummary(),
        detail: "",
        state: workspaceState,
        stateLabel: preflightStepStateLabel(workspaceState),
      },
      {
        id: "destination",
        label: surface.tr("gui.convert.preflight_destination", "Output destination"),
        summary: destinationPreflightSummary(),
        detail: surface.preflight.current
          ? surface.tr("gui.convert.preflight_current", "Current · {path}")
              .replace("{path}", surface.preflight.current)
          : surface.preflight.destination
            ? surface.tr("gui.convert.preflight_destination_value", "Destination · {path}")
                .replace("{path}", surface.preflight.destination)
            : "",
        state: destinationState,
        stateLabel: preflightStepStateLabel(destinationState),
      },
    ];
  }

  function preflightPresentation() {
    const preflight = surface.preflight;
    return {
      variant,
      phase: preflight.phase,
      ariaLabel: surface.tr("gui.convert.preflight_status", "Conversion preflight status"),
      heading: surface.tr("gui.convert.preflight_heading", "Before conversion"),
      statusLabel: preflightPhaseLabel(),
      lockMessage: preflight.lockedReason,
      actionLabel: preflight.cancellable
        ? preflight.cancelPending
          ? surface.tr("gui.convert.preflight_cancelling", "Stopping the current check")
          : surface.tr("gui.convert.preflight_cancel", "Cancel checks")
        : "",
      actionPending: preflight.cancelPending,
      issue: preflight.issue,
      steps: preflightSteps(),
      onAction: preflight.onCancel,
    };
  }

  function reviewLayout(): string {
    const review = surface.review;
    if (!review) return "";
    const count = review.plan.split_volume_count_budget;
    if (review.splitSize !== null && count !== null) {
      return surface.tr("gui.convert.review.numbered_volumes", "Numbered parts · up to {count} × {size}")
        .replace("{count}", Math.max(1, Math.trunc(count)).toLocaleString())
        .replace("{size}", formatBytes(review.splitSize));
    }
    return surface.tr("gui.convert.review.single_file", "One archive file");
  }

  function reviewItems() {
    const review = surface.review;
    if (!review) return [];
    const plan = review.plan;
    const workspace = plan.system_temp_budget_bytes > 0
      ? surface.tr(
          "gui.convert.review.workspace_split",
          "{destination} destination + {temporary} system temporary",
        )
          .replace("{destination}", formatBytes(plan.workspace_budget_bytes))
          .replace("{temporary}", formatBytes(plan.system_temp_budget_bytes))
      : surface.tr("gui.convert.review.workspace_destination", "{size} on the destination filesystem")
          .replace("{size}", formatBytes(plan.workspace_budget_bytes));
    return [
      {
        id: "source",
        label: surface.tr("gui.convert.review.source", "Measured source archive"),
        value: surface.tr("gui.convert.review.source_value", "{size} · {entries} entries")
          .replace("{size}", formatBytes(plan.total_bytes))
          .replace("{entries}", plan.entries.toLocaleString()),
      },
      {
        id: "layout",
        label: surface.tr("gui.convert.review.layout", "Output layout"),
        value: reviewLayout(),
      },
      {
        id: "budget",
        label: surface.tr("gui.convert.review.output_budget", "Final output space upper bound"),
        value: formatBytes(plan.final_output_budget_bytes),
      },
      {
        id: "workspace",
        label: surface.tr("gui.convert.review.workspace", "Peak conversion workspace"),
        value: workspace,
      },
    ];
  }

  function reviewPresentation() {
    const review = surface.review!;
    return {
      variant,
      ariaLabel: surface.tr("gui.convert.review.aria", "Conversion plan review"),
      eyebrow: surface.tr("gui.convert.review.eyebrow", "Checked and ready"),
      heading: surface.tr("gui.convert.review.heading", "Review before converting"),
      description: review.issue || surface.tr(
        "gui.convert.review.description",
        "Squallz read the source archive metadata and checked the required filesystems. These values are conservative safety bounds, not compressed-size predictions; the worker checks them again before writing.",
      ),
      outputName: pathBaseName(review.plan.primary_output),
      items: reviewItems(),
      confirmLabel: review.busy
        ? surface.tr("gui.convert.review.submitting", "Adding to queue")
        : review.retry
          ? surface.tr("gui.convert.review.retry", "Try conversion again")
          : surface.tr("gui.convert.review.confirm", "Start conversion"),
      cancelLabel: surface.tr("gui.convert.review.cancel", "Back to options"),
      busy: review.busy,
      onConfirm: review.onConfirm,
      onCancel: review.onCancel,
    };
  }
</script>

{#snippet formatChoices(classic = false)}
  <div
    class={classic ? "classic-segments" : "format-segments"}
    role="group"
    aria-label={surface.tr("gui.convert.target_format", "Target format")}
  >
    {#each surface.formats as format (format.id)}
      <button
        type="button"
        class:active={classic && format.selected}
        class:selected={!classic && format.selected}
        aria-pressed={format.selected}
        disabled={format.disabled}
        title={format.title}
        aria-label={format.ariaLabel}
        onclick={format.onSelect}
      >{format.label}</button>
    {/each}
  </div>
{/snippet}

{#snippet profileChoices(classic = false)}
  <div
    class={classic ? "classic-segments classic-profile-segments" : "preset-row"}
    role="group"
    aria-label={surface.tr("gui.convert.profile", "Compression profile")}
  >
    {#each surface.profiles as profile (profile.id)}
      <button
        type="button"
        class:active={classic && profile.selected}
        class:selected={!classic && profile.selected}
        aria-pressed={profile.selected}
        disabled={profile.disabled}
        title={profile.title}
        aria-label={profile.ariaLabel}
        onclick={profile.onSelect}
      >{profile.label}</button>
    {/each}
  </div>
{/snippet}

{#snippet customLevel(classic = false)}
  {#if surface.compression.custom}
    <div class={classic ? "classic-input classic-custom-level" : "custom-level-row"}>
      <input
        class:custom-level-range={!classic}
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
        class:custom-level-number={!classic}
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
        aria-describedby={surface.compression.custom.error ? customLevelErrorId : undefined}
        oninput={surface.compression.custom.onInput}
        onchange={surface.compression.custom.onChange}
      />
    </div>
    {#if surface.compression.custom.error}
      <small
        id={customLevelErrorId}
        class={classic ? "classic-input custom-level-error convert-copy" : "custom-level-error"}
        role="status"
      >{surface.compression.custom.error}</small>
    {/if}
  {/if}
{/snippet}

{#if variant === "modern"}
  <div class="create-sheet modern-convert">
    <div class="sheet-head">
      <div>
        <span class="eyebrow">{surface.tr("gui.convert.eyebrow", "Archive / Convert")}</span>
        <h1>{surface.tr("gui.convert.title", "Convert archive")}</h1>
        <p>{surface.tr("gui.convert.subtitle", "Choose the output format, compression, protection, and volume layout. Squallz converts without modifying the source.")}</p>
      </div>
      <button
        type="button"
        class="primary sheet-action"
        disabled={surface.start.disabled}
        title={surface.start.title}
        aria-label={surface.start.ariaLabel}
        aria-busy={surface.start.busy}
        onclick={surface.start.onSelect}
      ><Icon name="repeat" size={17} />{surface.start.label}</button>
    </div>

    <div class="create-grid">
      <section class="create-main-panel convert-main-panel">
        <div class="convert-route-grid">
          <div class="convert-route-node">
            <span class="field-label">{surface.tr("gui.convert.source", "Source")}</span>
            <div class="path-preview convert-path-preview">{surface.source.path}</div>
          </div>
          <span class="convert-route-arrow" aria-hidden="true"><Icon name="repeat" size={17} /></span>
          <div class="convert-route-node">
            <span class="field-label">{surface.tr("gui.convert.destination", "Destination")}</span>
            <div class="path-preview convert-path-preview" aria-live="polite">{surface.destination.path}</div>
          </div>
        </div>

        <div class="convert-choice-stack">
          <section class="convert-choice-group">
            <span class="field-label">{surface.tr("gui.convert.target_format", "Target format")}</span>
            {@render formatChoices()}
            <p class="format-note">{surface.formatNote}</p>
          </section>
          <section class="convert-choice-group">
            <span class="field-label">{surface.tr("gui.convert.profile", "Compression profile")}</span>
            {@render profileChoices()}
            <div class="level-control">
              <div>
                <strong>{surface.tr("gui.convert.level", "Compression level {level}").replace("{level}", String(surface.compression.level))}</strong>
                <span>{surface.compression.detail}</span>
              </div>
              {@render customLevel()}
            </div>
          </section>
        </div>

        <div class="settings-metric-grid two-column convert-metric-grid">
          <div>
            <span>{surface.tr("gui.convert.source_format", "Source format")}</span>
            <strong>{surface.source.format}</strong>
            <small>{surface.source.summary}</small>
          </div>
          <div>
            <span>{surface.tr("gui.convert.method", "Output method")}</span>
            <strong>{surface.compression.method}</strong>
            <small>{surface.tr("gui.convert.streaming_note", "Conversion is streamed; no temporary extraction")}</small>
          </div>
        </div>

        <details
          class="create-advanced-disclosure convert-advanced-disclosure"
          open={surface.advanced.open}
          ontoggle={(event) => surface.advanced.onToggle(event.currentTarget.open)}
        >
          <summary onkeydown={surface.advanced.onKeydown}>
            <span class="create-advanced-summary-icon"><Icon name="settings" size={17} /></span>
            <span>
              <strong>{surface.tr("gui.convert.advanced.title", "Output protection and volumes")}</strong>
              <small>{surface.advanced.detail}</small>
            </span>
            <Icon name="chevron-down" size={16} class="create-advanced-chevron" />
          </summary>
          <div class="convert-advanced-grid">
            <CreateProtectionOptions {...surface.protection} />
          </div>
        </details>

        <div class="setting-callout">
          <strong>{surface.contract.title}</strong>
          <span>{surface.contract.body}</span>
        </div>

        {#if surface.showPreflight}
          <CreatePreflightStatus {...preflightPresentation()} />
        {/if}

        {#if surface.review}
          <CreatePlanReview {...reviewPresentation()} />
        {/if}
      </section>

      <aside class="create-side-panel">
        <section>
          <div class="panel-title"><Icon name="check-circle" size={16} />{surface.readiness.title}</div>
          <strong>{surface.readiness.state}</strong>
          <p>{surface.readiness.body}</p>
        </section>
        <section>
          <div class="panel-title"><Icon name="shield-alert" size={16} />{surface.guard.title}</div>
          <p>{surface.guard.body}</p>
        </section>
      </aside>
    </div>
  </div>
{:else}
  <div class="classic-dialog-body">
    <section class="classic-extract-sheet classic-convert">
      <header>
        <div>
          <h1>{surface.tr("gui.convert.title", "Convert archive")}</h1>
          <p>{surface.tr("gui.convert.classic_intro", "Choose the target format, compression, protection, and volume layout before saving the converted archive.")}</p>
        </div>
        <button
          type="button"
          class="classic-primary"
          disabled={surface.start.disabled}
          title={surface.start.title}
          aria-label={surface.start.ariaLabel}
          aria-busy={surface.start.busy}
          onclick={surface.start.onSelect}
        >{surface.start.label}</button>
      </header>

      <div class="classic-form-grid">
        <div class="classic-label">{surface.tr("gui.convert.source", "Source")}</div>
        <div class="classic-input convert-copy">{surface.source.path}</div>
        <div class="classic-label">{surface.tr("gui.convert.source_format", "Source format")}</div>
        <div class="classic-input convert-copy">{surface.source.format} · {surface.source.summary}</div>
        <div class="classic-label">{surface.tr("gui.convert.target_format", "Target format")}</div>
        {@render formatChoices(true)}
        <div class="classic-label">{surface.tr("gui.create.format_boundary", "Format boundary")}</div>
        <div class="classic-input accent convert-copy">{surface.formatNote}</div>
        <div class="classic-label">{surface.tr("gui.convert.profile", "Compression profile")}</div>
        {@render profileChoices(true)}
        <div class="classic-label">{surface.tr("gui.convert.method", "Output method")}</div>
        <div class="classic-input">{surface.compression.method}</div>
        {#if surface.compression.custom}
          <div class="classic-label">{surface.tr("gui.convert.custom_level", "Custom level")}</div>
          <div class="convert-custom-level-classic">
            {@render customLevel(true)}
          </div>
        {/if}
        <div class="classic-label">{surface.tr("gui.convert.destination", "Destination")}</div>
        <div class="classic-input accent convert-copy" aria-live="polite">{surface.destination.path}</div>
        <CreateProtectionOptions {...surface.protection} />
        <div class="classic-label">{surface.contract.title}</div>
        <div class="classic-input convert-copy">{surface.contract.body}</div>
        <div class="classic-label">{surface.readiness.title}</div>
        <div class="classic-input">{surface.readiness.state}</div>
        <div class="classic-label">{surface.guard.title}</div>
        <div class="classic-input convert-copy">{surface.guard.body}</div>
      </div>

      {#if surface.showPreflight}
        <div class="classic-workflow-status-wrap">
          <CreatePreflightStatus {...preflightPresentation()} />
        </div>
      {/if}

      {#if surface.review}
        <div class="classic-plan-review-wrap">
          <CreatePlanReview {...reviewPresentation()} />
        </div>
      {/if}
    </section>
  </div>
{/if}
