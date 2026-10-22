<script lang="ts" module>
  import type { ExtractWorkspaceSession } from "../lib/extract-workspace.svelte";
  import type { ExtractPresentation } from "../lib/extract-presentation";
  import type { ArchivePresetSession } from "../lib/archive-presets.svelte";
  export type ExtractWorkspaceVariant = "modern" | "classic";
  export interface ExtractWorkspaceBridge {
    archiveTitle: string;
    archiveLine: string;
    password: string;
    warning: string;
    testEnabled: boolean;
    onReady: () => void;
    onBatch: () => void;
    onTest: () => void;
  }
</script>

<script lang="ts">
  import { onMount } from "svelte";
  import ArchivePresetPicker from "./ArchivePresetPicker.svelte";
  import ExtractPlanSummary from "./ExtractPlanSummary.svelte";
  import Icon from "./Icon.svelte";
  import { tFallback as tr } from "../lib/i18n.svelte";
  import { extractOverwriteLabel, extractSymlinkLabel } from "../lib/archive-option-labels";
  import type { ExtractDestinationMode, ExtractOverwriteMode, ExtractSymlinkMode } from "../lib/extract-options-draft.svelte";
  let { variant, session, view, presets, bridge }: {
    variant: ExtractWorkspaceVariant;
    session: ExtractWorkspaceSession;
    view: ExtractPresentation;
    presets: ArchivePresetSession;
    bridge: ExtractWorkspaceBridge;
  } = $props();
  const extractDestinationModes: ExtractDestinationMode[] = ["smart", "archive", "same", "choose"];
  const extractOverwriteModes: ExtractOverwriteMode[] = ["ask", "skip", "overwrite", "rename"];
  const extractSymlinkModes: ExtractSymlinkMode[] = ["preserve", "skip", "follow"];
  const labelWithDisabledReason = (label: string, reason: string) => reason ? `${label} · ${reason}` : label;
  let archiveRequiredReason = $derived(session.archiveRequiredReason);
  let startBlockedReason = $derived(view.extractStartBlockedReason());
  let startLabel = $derived(variant === "modern" ? view.extractActionLabel() : tr("gui.extract.start"));
  let batchLabel = $derived(variant === "modern"
    ? tr("gui.extract.review_batch")
    : tr("gui.extract.batch_review"));
  let title = $derived(variant === "modern" ? view.extractSafeTitle() : tr("gui.classic.command.extract_to"));
  let verification = $derived(session.state.verifySfx
    ? tr("gui.extract.verify_sfx")
    : null);
  let destinations = $derived(extractDestinationModes.map((mode) => {
    const busy = mode === "choose" && session.choosing;
    const reason = archiveRequiredReason || (busy ? session.pickerWait : "");
    const label = busy ? tr("gui.extract.choosing_destination") : view.extractDestinationTitle(mode);
    return { mode, busy, reason, label, detail: busy ? reason : view.extractDestinationDetail(mode) };
  }));
  onMount(() => bridge.onReady());
</script>

{#snippet planSummary()}
  <ExtractPlanSummary
    {variant} phase={session.plan.phase}
    ariaLabel={tr("gui.extract.plan_aria")}
    eyebrow={tr("gui.extract.plan_eyebrow")}
    heading={tr("gui.extract.plan_heading")}
    statusLabel={view.extractPlanStatusLabel()} description={view.extractPlanDescription()}
    destinationLabel={tr("gui.extract.plan_destination")}
    destination={session.plan.plan?.destination ?? session.destination} metrics={view.extractPlanMetrics()}
    note={tr("gui.extract.plan_snapshot_note")}
    error={session.planError}
    retryLabel={session.plan.phase === "blocked" ? tr("gui.extract.plan_recheck_space") : tr("gui.extract.plan_retry")}
    onRetry={() => void session.retry()}
  />
{/snippet}
{#if variant === "modern"}
  <div class="extract-view modern-extract">
    <div class="sheet-head">
      <div>
        <span class="eyebrow">{tr("gui.extract.eyebrow")}</span>
        <h1 id="extract-workspace-heading" tabindex="-1">{title}</h1>
        <p>{tr("gui.extract.safe_subtitle")}</p>
      </div>
      <button
        class="primary sheet-action"
        disabled={Boolean(startBlockedReason)}
        title={startBlockedReason || view.extractDestinationHint()}
        aria-label={labelWithDisabledReason(startLabel, startBlockedReason)}
        onclick={() => void session.start()}
      ><Icon name="archive" size={17} />{startLabel}</button>
    </div>

    <div class="extract-layout">
      <section class="extract-main-panel">
        <div class="extract-essentials-grid">
          <div class="extract-destination-setup">
            <div class="path-decision">
              <span class="block-label">{view.extractDestinationFieldLabel()}</span>
              <strong title={session.destination}>{session.destination}</strong>
              <p>{tr("gui.extract.snapshot_hint")}</p>
            </div>
            <div class="destination-grid">
              {#each destinations as choice (choice.mode)}
                <button
                  class:selected={session.state.destinationMode === choice.mode}
                  aria-pressed={session.state.destinationMode === choice.mode}
                  aria-busy={choice.busy}
                  disabled={Boolean(choice.reason)}
                  title={choice.reason}
                  aria-label={labelWithDisabledReason(choice.label, choice.reason)}
                  onclick={() => void session.selectDestination(choice.mode)}
                >
                  <strong>{choice.label}</strong>
                  <span>{choice.detail}</span>
                </button>
              {/each}
            </div>
            <div class="extract-essentials-facts">
              <div class="extract-essentials-fact">
                <span>{tr("gui.inspector.archive")}</span>
                <strong>{bridge.archiveTitle}</strong>
                <p>{bridge.archiveLine} · {view.extractSelectionLabel()}</p>
              </div>
              <div class="extract-essentials-fact">
                <span>{tr("gui.extract.password")}</span>
                <strong>{bridge.password}</strong>
                <p>{tr("gui.extract.snapshot_hint")}</p>
              </div>
            </div>
          </div>
          {@render planSummary()}
        </div>

        <ArchivePresetPicker instanceId={`${variant}-extract`} {variant} compact kind="extract" session={presets} />

        <section class="extract-policy-panel" aria-label={tr("gui.extract.safety")}>
          <div class="extract-policy-section">
            <div class="extract-policy-heading">
              <span class="block-label">{tr("gui.extract.conflict_policy")}</span>
              <strong>{extractOverwriteLabel(session.state.overwrite)}</strong>
            </div>
            <div class="extract-policy-grid" aria-label={tr("gui.extract.conflict_policy")}>
              {#each extractOverwriteModes as mode (mode)}
                <button
                  class:selected={session.state.overwrite === mode}
                  aria-pressed={session.state.overwrite === mode}
                  onclick={() => session.selectPolicy({ kind: "overwrite", value: mode })}
                >{extractOverwriteLabel(mode)}</button>
              {/each}
            </div>
          </div>
          <div class="extract-policy-section">
            <div class="extract-policy-heading">
              <span class="block-label">{tr("gui.extract.symlink_policy")}</span>
              <strong>{extractSymlinkLabel(session.state.symlinks)}</strong>
            </div>
            <div
              class="extract-symlink-grid"
              role="group"
              aria-label={tr("gui.extract.symlink_policy")}
            >
              {#each extractSymlinkModes as mode (mode)}
                <button
                  class:selected={session.state.symlinks === mode}
                  aria-pressed={session.state.symlinks === mode}
                  onclick={() => session.selectPolicy({ kind: "symlinks", value: mode })}
                >{extractSymlinkLabel(mode)}</button>
              {/each}
            </div>
          </div>
        </section>

        <div class="extract-context-strip">
          <div class="extract-context-item">
            <span>{tr("gui.archive.encoding")}</span>
            <strong>{view.extractEncodingLabel()}</strong>
            <p>{session.archive ? bridge.warning : session.destination}</p>
          </div>
          <div class="extract-context-item">
            <span>{tr("gui.extract.blocked_conditions")}</span>
            <strong>{tr("gui.extract.safety_guards_on")}</strong>
            <p>{tr("gui.extract.blocked_conditions_body")}</p>
            {#if verification}<p>{verification}</p>{/if}
          </div>
        </div>

        <div class="extract-flow-actions">
          <button
            disabled={Boolean(archiveRequiredReason)}
            title={archiveRequiredReason}
            aria-label={labelWithDisabledReason(batchLabel, archiveRequiredReason)}
            onclick={bridge.onBatch}
          ><Icon name="list" size={16} />{batchLabel}</button>
        </div>
      </section>
    </div>
  </div>
{:else}
  <div class="classic-dialog-body">
    <section class="classic-extract-sheet classic-extract">
      <header>
        <div>
          <h1 id="extract-workspace-heading" tabindex="-1">{title}</h1>
          <p>{tr("gui.extract.classic_subtitle")}</p>
        </div>
        <div class="classic-button-row">
          <button onclick={bridge.onBatch}>{batchLabel}</button>
          <button
            class="classic-primary"
            disabled={Boolean(startBlockedReason)}
            title={startBlockedReason || view.extractDestinationHint()}
            aria-label={labelWithDisabledReason(startLabel, startBlockedReason)}
            onclick={() => void session.start()}
          >{startLabel}</button>
        </div>
      </header>

      <div class="classic-extract-grid">
        <section class="classic-extract-form">
          <h2>{tr("gui.batch.destination")}</h2>
          <div class="classic-form-grid compact">
            <div class="classic-label">{view.extractDestinationFieldLabel()}</div>
            <div class="classic-input accent classic-extract-destination-path">{session.destination}</div>
            <div class="classic-label">{tr("common.mode")}</div>
            <div class="classic-segments">
              {#each destinations as choice (choice.mode)}
                <button
                  class:active={session.state.destinationMode === choice.mode}
                  aria-pressed={session.state.destinationMode === choice.mode}
                  aria-busy={choice.busy}
                  disabled={Boolean(choice.reason)}
                  title={choice.reason}
                  aria-label={labelWithDisabledReason(choice.label, choice.reason)}
                  onclick={() => void session.selectDestination(choice.mode)}
                >{choice.label}</button>
              {/each}
            </div>
            <div class="classic-label">{tr("gui.inspector.archive")}</div>
            <div class="classic-input classic-copy-wrap">{bridge.archiveLine}</div>
            <div class="classic-label">{tr("common.selection")}</div>
            <div class="classic-input accent classic-copy-wrap">{view.extractSelectionLabel()}</div>
            <div class="classic-label">{tr("gui.extract.password")}</div>
            <div class="classic-input classic-copy-wrap">{bridge.password}</div>
            {#if verification}
              <div class="classic-label">{tr("gui.extract.verification")}</div>
              <div class="classic-input classic-copy-wrap">{verification}</div>
            {/if}
            <div class="classic-label">{tr("gui.extract.conflicts")}</div>
            <div class="classic-segments">
              {#each extractOverwriteModes as mode (mode)}
                <button
                  class:active={session.state.overwrite === mode}
                  aria-pressed={session.state.overwrite === mode}
                  onclick={() => session.selectPolicy({ kind: "overwrite", value: mode })}
                >{extractOverwriteLabel(mode)}</button>
              {/each}
            </div>
            <div class="classic-label">{tr("gui.extract.symlink_policy")}</div>
            <div
              class="classic-segments"
              role="group"
              aria-label={tr("gui.extract.symlink_policy")}
            >
              {#each extractSymlinkModes as mode (mode)}
                <button
                  class:active={session.state.symlinks === mode}
                  aria-pressed={session.state.symlinks === mode}
                  onclick={() => session.selectPolicy({ kind: "symlinks", value: mode })}
                >{extractSymlinkLabel(mode)}</button>
              {/each}
            </div>
            <div class="classic-label">{tr("gui.archive.encoding")}</div>
            <div class="classic-input classic-copy-wrap">{view.extractEncodingLabel()}</div>
            <div class="classic-label">{tr("gui.extract.safety")}</div>
            <div class="classic-input accent classic-copy-wrap classic-extract-safety">{tr("gui.extract.safety_blocked")}</div>
          </div>
          <div class="classic-extract-actions">
            <button
              disabled={!bridge.testEnabled}
              title={archiveRequiredReason}
              aria-label={labelWithDisabledReason(tr("gui.extract.test_first"), archiveRequiredReason)}
              onclick={bridge.onTest}
            ><Icon name="check-circle" size={15} />{tr("gui.extract.test_first")}</button>
          </div>
        </section>

        <aside class="classic-extract-preview">
          {@render planSummary()}
        </aside>
      </div>

      <div class="classic-preset-wrap">
        <ArchivePresetPicker instanceId={`${variant}-extract`} {variant} compact kind="extract" session={presets} />
      </div>
    </section>
  </div>
{/if}
